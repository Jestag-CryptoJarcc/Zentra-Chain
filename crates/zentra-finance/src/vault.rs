//! # Omni-Vault Cross-Chain Ingest
//!
//! Manages the ingestion of cross-chain stablecoins (USDT, USDC, DAI, etc.)
//! into the Zentra network as zUSD — the unified on-chain stablecoin.
//!
//! ## Flow
//! 1. User deposits stablecoin to a TSS-controlled vault on the external chain
//! 2. Validators observe the deposit and submit an `IngestRequest`
//! 3. Threshold validators sign to validate the ingest
//! 4. zUSD is minted at 1:1 minus a 0.5% POL fee
//! 5. The 0.5% fee is injected into the AMM pool; LP tokens are TRUE BURNED
//!
//! ## TRUE BURN
//! When users burn zUSD (to exit back to external chains), the tokens are
//! permanently removed from the chain — not sent to a dead address.

use serde::{Serialize, Deserialize};
use zentra_types::{Address, BurnOutput, BurnType, ZentraError};
use zentra_types::error::ZentraResult;

use crate::pol::ProtocolOwnedLiquidity;
use crate::tss::TssManager;

/// Status of an ingest request as it moves through the validation pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IngestStatus {
    /// Submitted but not yet validated by threshold signers.
    Pending,
    /// Validated by sufficient threshold signatures.
    Validated,
    /// zUSD has been minted and credited to the depositor.
    Minted,
    /// Rejected due to invalid proof or failed validation.
    Rejected,
}

/// A request to ingest stablecoins from an external chain into Zentra as zUSD.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestRequest {
    /// Name of the source blockchain (e.g., "ethereum", "tron", "bsc").
    pub external_chain: String,
    /// Transaction hash on the external chain proving the deposit.
    pub external_tx_hash: Vec<u8>,
    /// The Zentra address that should receive the minted zUSD.
    pub depositor_address: Address,
    /// Amount of stablecoin deposited (in external chain's smallest unit).
    pub stablecoin_amount: u128,
    /// Current processing status.
    pub status: IngestStatus,
}

/// Result of a successful zUSD minting operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintResult {
    /// Gross zUSD minted before fee deduction.
    pub zusd_minted: u128,
    /// The 0.5% POL fee deducted.
    pub fee_deducted: u128,
    /// LP tokens that were TRUE BURNED from the fee injection.
    pub lp_burned: u128,
}

/// Aggregate statistics for the Omni-Vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultStats {
    /// Total zUSD ever minted through the vault.
    pub total_minted: u128,
    /// Total zUSD that has been TRUE BURNED (permanently destroyed).
    pub total_burned: u128,
    /// Currently circulating zUSD (minted - burned).
    pub circulating_zusd: u128,
    /// AMM pool reserves `(reserve_ztr, reserve_zusd)`.
    pub pol_reserves: (u128, u128),
}

/// The Omni-Vault: gateway for cross-chain stablecoin ingestion.
///
/// Combines TSS-based validation with POL fee collection to provide a secure,
/// decentralized bridge for bringing stablecoins onto Zentra as zUSD.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OmniVault {
    /// Threshold signature manager for validating cross-chain proofs.
    pub tss: TssManager,
    /// Protocol-owned liquidity engine (receives ingest fees).
    pub pol: ProtocolOwnedLiquidity,
    /// Queue of ingest requests in various stages.
    pub pending_ingests: Vec<IngestRequest>,
    /// Cumulative zUSD minted across all ingests.
    pub total_zusd_minted: u128,
    /// Cumulative zUSD that has been TRUE BURNED (permanently removed from chain).
    pub total_zusd_burned: u128,
}

impl OmniVault {
    /// Create a new Omni-Vault with the specified TSS parameters.
    ///
    /// The TSS keys are NOT generated automatically — call
    /// `tss.generate_keys()` before attempting validation.
    ///
    /// # Arguments
    /// - `threshold`: Minimum validators required to confirm an ingest.
    /// - `total_validators`: Total number of vault validators.
    pub fn new(threshold: u16, total_validators: u16) -> Self {
        tracing::info!(
            threshold,
            total_validators,
            "creating Omni-Vault for cross-chain ingest"
        );
        Self {
            tss: TssManager::new(threshold, total_validators),
            pol: ProtocolOwnedLiquidity::new(),
            pending_ingests: Vec::new(),
            total_zusd_minted: 0,
            total_zusd_burned: 0,
        }
    }

    /// Submit a new ingest request to the vault.
    ///
    /// The request is added to the pending queue with status `Pending`.
    /// Duplicate detection is based on the `external_tx_hash`.
    ///
    /// # Errors
    /// - `ZentraError::TransactionValidation` if the amount is zero.
    /// - `ZentraError::TransactionValidation` if a duplicate tx hash exists.
    pub fn submit_ingest(&mut self, request: IngestRequest) -> ZentraResult<()> {
        if request.stablecoin_amount == 0 {
            return Err(ZentraError::TransactionValidation(
                "ingest amount must be non-zero".into(),
            ));
        }

        // Check for duplicate external tx hash
        let is_dup = self
            .pending_ingests
            .iter()
            .any(|r| r.external_tx_hash == request.external_tx_hash);
        if is_dup {
            return Err(ZentraError::TransactionValidation(
                "duplicate external transaction hash".into(),
            ));
        }

        tracing::info!(
            chain = %request.external_chain,
            amount = request.stablecoin_amount,
            depositor = %request.depositor_address,
            "ingest request submitted"
        );

        self.pending_ingests.push(request);
        Ok(())
    }

    /// Validate a pending ingest request using threshold signatures.
    ///
    /// Requires at least `threshold` valid partial signatures from vault
    /// validators. Upon successful validation, the request status transitions
    /// from `Pending` to `Validated`.
    ///
    /// # Arguments
    /// - `request_index`: Index into the pending_ingests queue.
    /// - `validator_sigs`: Pairs of `(validator_index, partial_signature_bytes)`.
    ///
    /// # Errors
    /// - `ZentraError::TransactionValidation` if index is out of bounds.
    /// - `ZentraError::TransactionValidation` if request is not in `Pending` state.
    /// - `ZentraError::TssError` if threshold is not met or signatures are invalid.
    pub fn validate_ingest(
        &mut self,
        request_index: usize,
        validator_sigs: Vec<(u16, Vec<u8>)>,
    ) -> ZentraResult<()> {
        let request = self
            .pending_ingests
            .get(request_index)
            .ok_or_else(|| {
                ZentraError::TransactionValidation(format!(
                    "ingest request index {} out of range",
                    request_index
                ))
            })?;

        if request.status != IngestStatus::Pending {
            return Err(ZentraError::TransactionValidation(format!(
                "ingest request is not pending (status: {:?})",
                request.status
            )));
        }

        // Verify we have enough signers
        if (validator_sigs.len() as u16) < self.tss.threshold {
            return Err(ZentraError::TssError(format!(
                "need {} validator signatures, got {}",
                self.tss.threshold,
                validator_sigs.len()
            )));
        }

        // Build the message to verify: hash of (chain || tx_hash || amount || depositor)
        let message = build_ingest_message(request);

        // Check for any abort (invalid partial signature)
        if let Some(bad_idx) = self.tss.identify_abort(&validator_sigs, &message) {
            return Err(ZentraError::TssError(format!(
                "validator {} submitted an invalid partial signature",
                bad_idx
            )));
        }

        // Mark as validated
        self.pending_ingests[request_index].status = IngestStatus::Validated;

        tracing::info!(
            request_index,
            num_validators = validator_sigs.len(),
            "ingest request validated by threshold signers"
        );

        Ok(())
    }

    /// Mint zUSD for a validated ingest request.
    ///
    /// 1. Mints zUSD at a 1:1 ratio with the deposited stablecoin amount.
    /// 2. Deducts a 0.5% POL fee from the minted amount.
    /// 3. The fee is injected into the AMM pool; resulting LP tokens are TRUE BURNED.
    /// 4. Request status transitions to `Minted`.
    ///
    /// # Arguments
    /// - `request_index`: Index of the validated request.
    ///
    /// # Errors
    /// - `ZentraError::TransactionValidation` if index is out of bounds.
    /// - `ZentraError::TransactionValidation` if request is not `Validated`.
    pub fn mint_zusd(&mut self, request_index: usize) -> ZentraResult<MintResult> {
        let request = self
            .pending_ingests
            .get(request_index)
            .ok_or_else(|| {
                ZentraError::TransactionValidation(format!(
                    "ingest request index {} out of range",
                    request_index
                ))
            })?;

        if request.status != IngestStatus::Validated {
            return Err(ZentraError::TransactionValidation(format!(
                "ingest request must be validated before minting (status: {:?})",
                request.status
            )));
        }

        let gross_amount = request.stablecoin_amount;

        // Process through POL: deducts 0.5% fee, injects into pool, burns LP tokens
        let ingest_result = self.pol.process_cross_chain_ingest(gross_amount);

        // Update vault counters
        self.total_zusd_minted = self
            .total_zusd_minted
            .saturating_add(ingest_result.net_amount);

        // Mark as minted
        self.pending_ingests[request_index].status = IngestStatus::Minted;

        tracing::info!(
            request_index,
            gross = gross_amount,
            net = ingest_result.net_amount,
            fee = ingest_result.fee_deducted,
            lp_burned = ingest_result.lp_tokens_burned,
            "zUSD minted — POL fee TRUE BURNED into AMM"
        );

        Ok(MintResult {
            zusd_minted: ingest_result.net_amount,
            fee_deducted: ingest_result.fee_deducted,
            lp_burned: ingest_result.lp_tokens_burned,
        })
    }

    /// TRUE BURN zUSD — permanently remove tokens from the chain.
    ///
    /// Used when a user wants to exit back to an external chain. The zUSD
    /// is **not** sent to a dead address; it is simply removed from existence.
    /// The vault tracks the burn for accounting purposes and creates a
    /// `BurnOutput` record.
    ///
    /// # Arguments
    /// - `amount`: The amount of zUSD to burn (in micro-units).
    /// - `burner`: The address of the user burning the zUSD.
    ///
    /// # Errors
    /// - `ZentraError::TransactionValidation` if amount is zero.
    /// - `ZentraError::TransactionValidation` if amount exceeds circulating supply.
    pub fn burn_zusd(&mut self, amount: u128, burner: &Address) -> ZentraResult<()> {
        if amount == 0 {
            return Err(ZentraError::TransactionValidation(
                "burn amount must be non-zero".into(),
            ));
        }

        let circulating = self
            .total_zusd_minted
            .saturating_sub(self.total_zusd_burned);
        if amount > circulating {
            return Err(ZentraError::TransactionValidation(format!(
                "burn amount {} exceeds circulating zUSD supply {}",
                amount, circulating
            )));
        }

        // TRUE BURN: permanently destroy the tokens.
        // Create a BurnOutput record for auditing. The amount here is
        // truncated to u64 for the BurnOutput struct; for very large
        // burns we cap at u64::MAX for the record but track the full u128 internally.
        let burn_record_amount = if amount > u64::MAX as u128 {
            u64::MAX
        } else {
            amount as u64
        };
        let _burn = BurnOutput {
            amount: burn_record_amount,
            burn_type: BurnType::StablecoinBurn,
        };

        self.total_zusd_burned = self.total_zusd_burned.saturating_add(amount);

        tracing::info!(
            amount,
            burner = %burner,
            total_burned = self.total_zusd_burned,
            circulating = self.total_zusd_minted.saturating_sub(self.total_zusd_burned),
            "zUSD TRUE BURNED — tokens permanently removed from chain"
        );

        Ok(())
    }

    /// Get a snapshot of vault statistics.
    pub fn get_vault_stats(&self) -> VaultStats {
        let reserves = self.pol.pool.get_reserves();
        VaultStats {
            total_minted: self.total_zusd_minted,
            total_burned: self.total_zusd_burned,
            circulating_zusd: self.total_zusd_minted.saturating_sub(self.total_zusd_burned),
            pol_reserves: reserves,
        }
    }
}

/// Build the deterministic message bytes for an ingest request.
///
/// Used for TSS signing/verification. Concatenates chain name, tx hash,
/// depositor address payload, and stablecoin amount.
fn build_ingest_message(request: &IngestRequest) -> Vec<u8> {
    let mut msg = Vec::new();
    msg.extend_from_slice(request.external_chain.as_bytes());
    msg.extend_from_slice(&request.external_tx_hash);
    msg.extend_from_slice(&request.depositor_address.payload);
    msg.extend_from_slice(&request.stablecoin_amount.to_le_bytes());
    msg
}

#[cfg(test)]
mod tests {
    use super::*;
    use zentra_types::NetworkType;

    fn test_address() -> Address {
        Address::from_payload([42u8; 32], NetworkType::Devnet)
    }

    fn test_request(amount: u128) -> IngestRequest {
        IngestRequest {
            external_chain: "ethereum".into(),
            external_tx_hash: vec![0xAA; 32],
            depositor_address: test_address(),
            stablecoin_amount: amount,
            status: IngestStatus::Pending,
        }
    }

    fn seeded_vault() -> OmniVault {
        let mut vault = OmniVault::new(2, 3);
        vault.tss.generate_keys().unwrap();
        // Seed the POL pool so fee injection has reserves to pair against
        vault.pol.pool.add_liquidity(1_000_000_000, 1_000_000).unwrap();
        vault
    }

    #[test]
    fn test_new_vault() {
        let vault = OmniVault::new(2, 3);
        assert_eq!(vault.total_zusd_minted, 0);
        assert_eq!(vault.total_zusd_burned, 0);
        assert!(vault.pending_ingests.is_empty());
    }

    #[test]
    fn test_submit_ingest() {
        let mut vault = seeded_vault();
        vault.submit_ingest(test_request(1_000_000)).unwrap();
        assert_eq!(vault.pending_ingests.len(), 1);
        assert_eq!(vault.pending_ingests[0].status, IngestStatus::Pending);
    }

    #[test]
    fn test_submit_zero_amount_fails() {
        let mut vault = seeded_vault();
        let result = vault.submit_ingest(test_request(0));
        assert!(result.is_err());
    }

    #[test]
    fn test_submit_duplicate_tx_hash_fails() {
        let mut vault = seeded_vault();
        vault.submit_ingest(test_request(1_000_000)).unwrap();
        let result = vault.submit_ingest(test_request(2_000_000)); // same tx hash
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_ingest() {
        let mut vault = seeded_vault();
        vault.submit_ingest(test_request(1_000_000)).unwrap();

        // Build valid partial signatures
        let message = build_ingest_message(&vault.pending_ingests[0]);
        let mut sigs = Vec::new();
        for i in 0..2u16 {
            let p = &vault.tss.participants[i as usize];
            let secret_bytes: [u8; 32] = p.secret_share.as_slice().try_into().unwrap();
            let signing_key = ed25519_dalek::SigningKey::from_bytes(&secret_bytes);
            let sig = ed25519_dalek::Signer::sign(&signing_key, &message);
            sigs.push((i, sig.to_bytes().to_vec()));
        }

        vault.validate_ingest(0, sigs).unwrap();
        assert_eq!(vault.pending_ingests[0].status, IngestStatus::Validated);
    }

    #[test]
    fn test_validate_insufficient_sigs() {
        let mut vault = seeded_vault();
        vault.submit_ingest(test_request(1_000_000)).unwrap();

        // Only 1 sig when threshold is 2
        let message = build_ingest_message(&vault.pending_ingests[0]);
        let p = &vault.tss.participants[0];
        let secret_bytes: [u8; 32] = p.secret_share.as_slice().try_into().unwrap();
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&secret_bytes);
        let sig = ed25519_dalek::Signer::sign(&signing_key, &message);
        let sigs = vec![(0u16, sig.to_bytes().to_vec())];

        let result = vault.validate_ingest(0, sigs);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_out_of_range_index() {
        let mut vault = seeded_vault();
        let result = vault.validate_ingest(0, vec![]);
        assert!(result.is_err());
    }

    #[test]
    fn test_mint_zusd() {
        let mut vault = seeded_vault();
        vault.submit_ingest(test_request(10_000_000)).unwrap();

        // Validate first
        let message = build_ingest_message(&vault.pending_ingests[0]);
        let mut sigs = Vec::new();
        for i in 0..2u16 {
            let p = &vault.tss.participants[i as usize];
            let secret_bytes: [u8; 32] = p.secret_share.as_slice().try_into().unwrap();
            let signing_key = ed25519_dalek::SigningKey::from_bytes(&secret_bytes);
            let sig = ed25519_dalek::Signer::sign(&signing_key, &message);
            sigs.push((i, sig.to_bytes().to_vec()));
        }
        vault.validate_ingest(0, sigs).unwrap();

        // Now mint
        let result = vault.mint_zusd(0).unwrap();
        assert!(result.zusd_minted > 0);
        assert!(result.fee_deducted > 0);
        assert_eq!(
            result.zusd_minted + result.fee_deducted,
            10_000_000
        );
        assert_eq!(vault.pending_ingests[0].status, IngestStatus::Minted);
        assert_eq!(vault.total_zusd_minted, result.zusd_minted);
    }

    #[test]
    fn test_mint_without_validation_fails() {
        let mut vault = seeded_vault();
        vault.submit_ingest(test_request(1_000_000)).unwrap();
        let result = vault.mint_zusd(0);
        assert!(result.is_err());
    }

    #[test]
    fn test_burn_zusd() {
        let mut vault = seeded_vault();
        // Mint some zUSD first
        vault.submit_ingest(test_request(10_000_000)).unwrap();
        let message = build_ingest_message(&vault.pending_ingests[0]);
        let mut sigs = Vec::new();
        for i in 0..2u16 {
            let p = &vault.tss.participants[i as usize];
            let secret_bytes: [u8; 32] = p.secret_share.as_slice().try_into().unwrap();
            let signing_key = ed25519_dalek::SigningKey::from_bytes(&secret_bytes);
            let sig = ed25519_dalek::Signer::sign(&signing_key, &message);
            sigs.push((i, sig.to_bytes().to_vec()));
        }
        vault.validate_ingest(0, sigs).unwrap();
        let mint_result = vault.mint_zusd(0).unwrap();

        // Now burn half
        let burn_amount = mint_result.zusd_minted / 2;
        vault.burn_zusd(burn_amount, &test_address()).unwrap();

        assert_eq!(vault.total_zusd_burned, burn_amount);
        let stats = vault.get_vault_stats();
        assert_eq!(stats.circulating_zusd, mint_result.zusd_minted - burn_amount);
    }

    #[test]
    fn test_burn_zero_fails() {
        let mut vault = seeded_vault();
        let result = vault.burn_zusd(0, &test_address());
        assert!(result.is_err());
    }

    #[test]
    fn test_burn_exceeds_circulating_fails() {
        let mut vault = seeded_vault();
        let result = vault.burn_zusd(1_000_000, &test_address());
        assert!(result.is_err()); // No zUSD minted yet
    }

    #[test]
    fn test_vault_stats() {
        let vault = OmniVault::new(2, 3);
        let stats = vault.get_vault_stats();
        assert_eq!(stats.total_minted, 0);
        assert_eq!(stats.total_burned, 0);
        assert_eq!(stats.circulating_zusd, 0);
    }

    #[test]
    fn test_full_lifecycle() {
        let mut vault = seeded_vault();

        // Submit
        vault.submit_ingest(test_request(5_000_000)).unwrap();

        // Validate
        let message = build_ingest_message(&vault.pending_ingests[0]);
        let mut sigs = Vec::new();
        for i in 0..2u16 {
            let p = &vault.tss.participants[i as usize];
            let secret_bytes: [u8; 32] = p.secret_share.as_slice().try_into().unwrap();
            let signing_key = ed25519_dalek::SigningKey::from_bytes(&secret_bytes);
            let sig = ed25519_dalek::Signer::sign(&signing_key, &message);
            sigs.push((i, sig.to_bytes().to_vec()));
        }
        vault.validate_ingest(0, sigs).unwrap();

        // Mint
        let mint_result = vault.mint_zusd(0).unwrap();

        // Burn
        vault
            .burn_zusd(mint_result.zusd_minted, &test_address())
            .unwrap();

        // Check final state
        let stats = vault.get_vault_stats();
        assert_eq!(stats.circulating_zusd, 0);
        assert!(stats.total_minted > 0);
        assert_eq!(stats.total_minted, stats.total_burned);
    }

    #[test]
    fn test_build_ingest_message_deterministic() {
        let req = test_request(42);
        let msg1 = build_ingest_message(&req);
        let msg2 = build_ingest_message(&req);
        assert_eq!(msg1, msg2);
    }
}
