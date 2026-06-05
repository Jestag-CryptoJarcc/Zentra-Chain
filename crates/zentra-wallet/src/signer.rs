//! # Transaction Signing
//!
//! Builds, signs, and verifies UTXO-based Zentra transactions using Ed25519
//! digital signatures.
//!
//! ## Transaction Types
//!
//! This module defines lightweight transaction primitives (`Transaction`,
//! `OutPoint`, `UtxoEntry`, etc.) for the wallet layer. These will be
//! superseded by `zentra_core::transaction` types once that crate is built;
//! the signing logic will remain the same.

use ed25519_dalek::{Signature, Signer, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use zentra_types::address::Address;
use zentra_types::amount::Amount;
use zentra_types::error::{ZentraError, ZentraResult};
use zentra_types::Hash;

use crate::keygen::WalletKeypair;

// ─── Transaction Primitives ────────────────────────────────────────────────────

/// A reference to a specific output in a previous transaction.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OutPoint {
    /// Hash of the transaction containing the output.
    pub txid: Hash,
    /// Index of the output within that transaction.
    pub index: u32,
}

/// A UTXO entry — the value and owner of an unspent output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UtxoEntry {
    /// The amount held by this UTXO (in zents).
    pub amount: Amount,
    /// The address that owns this UTXO.
    pub address: Address,
    /// The block height at which this UTXO was created.
    pub block_height: u64,
}

/// A transaction input — consumes a UTXO.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxInput {
    /// The outpoint being spent.
    pub outpoint: OutPoint,
    /// Ed25519 signature proving ownership of the UTXO.
    pub signature: Vec<u8>,
    /// The public key of the signer (32 bytes).
    pub public_key: [u8; 32],
}

/// A transaction output — creates a new UTXO.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxOutput {
    /// Amount sent to the recipient (in zents).
    pub amount: Amount,
    /// Recipient address.
    pub address: Address,
}

/// A Zentra transaction (wallet-layer representation).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    /// Transaction version.
    pub version: u32,
    /// Inputs consuming existing UTXOs.
    pub inputs: Vec<TxInput>,
    /// Outputs creating new UTXOs.
    pub outputs: Vec<TxOutput>,
    /// Fee paid by this transaction (in zents).
    pub fee: Amount,
    /// Optional transaction memo / payload (for contract calls, etc.).
    pub payload: Vec<u8>,
}

impl Transaction {
    /// Compute the canonical transaction hash (txid).
    ///
    /// The hash covers version, outputs, fee, and payload. Inputs' signatures
    /// are excluded (they sign the hash, so including them would be circular).
    pub fn hash(&self) -> Hash {
        let mut data = Vec::new();

        // Version
        data.extend_from_slice(&self.version.to_le_bytes());

        // Number of inputs (outpoints only, not signatures)
        data.extend_from_slice(&(self.inputs.len() as u32).to_le_bytes());
        for input in &self.inputs {
            data.extend_from_slice(input.outpoint.txid.as_bytes());
            data.extend_from_slice(&input.outpoint.index.to_le_bytes());
        }

        // Outputs
        data.extend_from_slice(&(self.outputs.len() as u32).to_le_bytes());
        for output in &self.outputs {
            data.extend_from_slice(&output.amount.as_zents().to_le_bytes());
            data.extend_from_slice(output.address.as_bytes());
        }

        // Fee
        data.extend_from_slice(&self.fee.as_zents().to_le_bytes());

        // Payload
        data.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        data.extend_from_slice(&self.payload);

        Hash::hash(&data)
    }
}

// ─── Signing Functions ─────────────────────────────────────────────────────────

/// Sign a transaction's hash with the given keypair and return the 64-byte
/// Ed25519 signature.
///
/// The signature covers `tx.hash()`, which excludes signatures themselves
/// to avoid circular dependencies.
pub fn sign_transaction(tx: &Transaction, keypair: &WalletKeypair) -> Vec<u8> {
    let hash = tx.hash();
    let signature = keypair.signing_key().sign(hash.as_bytes());

    tracing::debug!(
        txid = %hash,
        pubkey = %hex::encode(keypair.public_key_bytes()),
        "Signed transaction"
    );

    signature.to_bytes().to_vec()
}

/// Verify an Ed25519 signature on a transaction hash.
///
/// Returns `true` if the signature is valid for the given public key and
/// transaction content.
pub fn verify_transaction_signature(
    tx: &Transaction,
    signature: &[u8],
    public_key: &[u8; 32],
) -> bool {
    let hash = tx.hash();

    let Ok(verifying_key) = VerifyingKey::from_bytes(public_key) else {
        tracing::warn!("Invalid public key bytes during signature verification");
        return false;
    };

    let Ok(sig) = Signature::from_slice(signature) else {
        tracing::warn!("Invalid signature bytes (expected 64 bytes)");
        return false;
    };

    verifying_key.verify(hash.as_bytes(), &sig).is_ok()
}

/// Build a signed transfer transaction from available UTXOs.
///
/// Implements a simple greedy UTXO selection: sorts UTXOs by value descending,
/// then picks until `amount + fee` is covered. Creates the recipient output,
/// a change output (if needed), and signs all inputs.
///
/// # Errors
///
/// - [`ZentraError::InsufficientFunds`] if the UTXOs don't cover `amount + fee`.
/// - [`ZentraError::Wallet`] on arithmetic overflow.
pub fn build_transfer(
    from_keypair: &WalletKeypair,
    to_address: &Address,
    amount: Amount,
    fee: Amount,
    utxos: &[(OutPoint, UtxoEntry)],
) -> ZentraResult<Transaction> {
    let total_needed = amount
        .checked_add(fee)
        .ok_or_else(|| ZentraError::Wallet("Amount + fee overflow".to_string()))?;

    // Sort UTXOs by value descending for greedy selection
    let mut sorted_utxos: Vec<_> = utxos.iter().collect();
    sorted_utxos.sort_by(|a, b| b.1.amount.cmp(&a.1.amount));

    // Select UTXOs
    let mut selected: Vec<&(OutPoint, UtxoEntry)> = Vec::new();
    let mut accumulated = Amount::ZERO;

    for utxo in &sorted_utxos {
        selected.push(utxo);
        accumulated = accumulated
            .checked_add(utxo.1.amount)
            .ok_or_else(|| ZentraError::Wallet("UTXO sum overflow".to_string()))?;

        if accumulated >= total_needed {
            break;
        }
    }

    if accumulated < total_needed {
        return Err(ZentraError::InsufficientFunds {
            required: total_needed.as_zents(),
            available: accumulated.as_zents(),
        });
    }

    // Build outputs
    let mut outputs = vec![TxOutput {
        amount,
        address: to_address.clone(),
    }];

    // Change output
    let change = accumulated
        .checked_sub(total_needed)
        .ok_or_else(|| ZentraError::Wallet("Change calculation underflow".to_string()))?;
    if !change.is_zero() {
        let change_address = from_keypair.address(to_address.network);
        outputs.push(TxOutput {
            amount: change,
            address: change_address,
        });
    }

    // Build inputs (signatures will be filled after computing the tx hash)
    let inputs: Vec<TxInput> = selected
        .iter()
        .map(|(outpoint, _entry)| TxInput {
            outpoint: outpoint.clone(),
            signature: Vec::new(), // placeholder
            public_key: from_keypair.public_key_bytes(),
        })
        .collect();

    let mut tx = Transaction {
        version: 1,
        inputs,
        outputs,
        fee,
        payload: Vec::new(),
    };

    // Sign all inputs
    let sig_bytes = sign_transaction(&tx, from_keypair);
    for input in &mut tx.inputs {
        input.signature = sig_bytes.clone();
    }

    tracing::info!(
        txid = %tx.hash(),
        amount = %amount,
        fee = %fee,
        inputs = selected.len(),
        change = %change,
        "Built transfer transaction"
    );

    Ok(tx)
}

// ─── Unit Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keygen::MasterKey;
    use zentra_types::constants::NetworkType;

    fn test_keypair() -> WalletKeypair {
        let master = MasterKey::generate();
        master.derive_keypair(0, 0)
    }

    fn test_utxos(
        keypair: &WalletKeypair,
        amounts: &[u64],
    ) -> Vec<(OutPoint, UtxoEntry)> {
        amounts
            .iter()
            .enumerate()
            .map(|(i, &amount)| {
                let outpoint = OutPoint {
                    txid: Hash::hash(format!("tx-{}", i).as_bytes()),
                    index: 0,
                };
                let entry = UtxoEntry {
                    amount: Amount::from_zents(amount),
                    address: keypair.address(NetworkType::Mainnet),
                    block_height: i as u64,
                };
                (outpoint, entry)
            })
            .collect()
    }

    #[test]
    fn test_sign_and_verify() {
        let keypair = test_keypair();

        let tx = Transaction {
            version: 1,
            inputs: vec![TxInput {
                outpoint: OutPoint {
                    txid: Hash::hash(b"test-input"),
                    index: 0,
                },
                signature: Vec::new(),
                public_key: keypair.public_key_bytes(),
            }],
            outputs: vec![TxOutput {
                amount: Amount::from_coins(1),
                address: keypair.address(NetworkType::Mainnet),
            }],
            fee: Amount::from_zents(1_000),
            payload: Vec::new(),
        };

        let sig = sign_transaction(&tx, &keypair);
        assert_eq!(sig.len(), 64, "Ed25519 signature must be 64 bytes");

        let valid = verify_transaction_signature(&tx, &sig, &keypair.public_key_bytes());
        assert!(valid, "Signature must verify");
    }

    #[test]
    fn test_verify_wrong_key() {
        let kp1 = test_keypair();
        let kp2 = test_keypair();

        let tx = Transaction {
            version: 1,
            inputs: vec![],
            outputs: vec![TxOutput {
                amount: Amount::from_coins(1),
                address: kp1.address(NetworkType::Mainnet),
            }],
            fee: Amount::ZERO,
            payload: Vec::new(),
        };

        let sig = sign_transaction(&tx, &kp1);
        let valid = verify_transaction_signature(&tx, &sig, &kp2.public_key_bytes());
        assert!(!valid, "Signature from different key must fail");
    }

    #[test]
    fn test_verify_tampered_transaction() {
        let kp = test_keypair();

        let tx = Transaction {
            version: 1,
            inputs: vec![],
            outputs: vec![TxOutput {
                amount: Amount::from_coins(1),
                address: kp.address(NetworkType::Mainnet),
            }],
            fee: Amount::ZERO,
            payload: Vec::new(),
        };

        let sig = sign_transaction(&tx, &kp);

        // Tamper: change the fee
        let tampered_tx = Transaction {
            version: 1,
            inputs: vec![],
            outputs: vec![TxOutput {
                amount: Amount::from_coins(1),
                address: kp.address(NetworkType::Mainnet),
            }],
            fee: Amount::from_zents(999),
            payload: Vec::new(),
        };

        let valid = verify_transaction_signature(&tampered_tx, &sig, &kp.public_key_bytes());
        assert!(!valid, "Tampered transaction must fail verification");
    }

    #[test]
    fn test_verify_invalid_signature_bytes() {
        let kp = test_keypair();
        let tx = Transaction {
            version: 1,
            inputs: vec![],
            outputs: vec![],
            fee: Amount::ZERO,
            payload: Vec::new(),
        };

        // Too short
        assert!(!verify_transaction_signature(&tx, &[0u8; 32], &kp.public_key_bytes()));
        // Too long
        assert!(!verify_transaction_signature(&tx, &[0u8; 128], &kp.public_key_bytes()));
    }

    #[test]
    fn test_build_transfer_exact_amount() {
        let kp = test_keypair();
        let to_addr = Address::from_public_key(&[99u8; 32], NetworkType::Mainnet);

        let utxos = test_utxos(&kp, &[100_000_000]); // 1 ZTR
        let amount = Amount::from_zents(99_999_000);
        let fee = Amount::from_zents(1_000);

        let tx = build_transfer(&kp, &to_addr, amount, fee, &utxos).expect("build");

        // With exact amount: 1 output for recipient, no change
        assert_eq!(tx.outputs.len(), 1);
        assert_eq!(tx.outputs[0].amount, amount);
        assert_eq!(tx.outputs[0].address, to_addr);
        assert_eq!(tx.fee, fee);
        assert_eq!(tx.inputs.len(), 1);

        // Verify all input signatures
        for input in &tx.inputs {
            assert!(verify_transaction_signature(&tx, &input.signature, &input.public_key));
        }
    }

    #[test]
    fn test_build_transfer_with_change() {
        let kp = test_keypair();
        let to_addr = Address::from_public_key(&[99u8; 32], NetworkType::Mainnet);

        let utxos = test_utxos(&kp, &[500_000_000]); // 5 ZTR
        let amount = Amount::from_coins(2);
        let fee = Amount::from_zents(1_000);

        let tx = build_transfer(&kp, &to_addr, amount, fee, &utxos).expect("build");

        // Should have 2 outputs: recipient + change
        assert_eq!(tx.outputs.len(), 2);
        assert_eq!(tx.outputs[0].amount, amount);

        let expected_change = Amount::from_zents(500_000_000 - 200_000_000 - 1_000);
        assert_eq!(tx.outputs[1].amount, expected_change);
    }

    #[test]
    fn test_build_transfer_multiple_utxos() {
        let kp = test_keypair();
        let to_addr = Address::from_public_key(&[99u8; 32], NetworkType::Mainnet);

        // Three small UTXOs
        let utxos = test_utxos(&kp, &[50_000_000, 30_000_000, 25_000_000]);
        let amount = Amount::from_zents(90_000_000);
        let fee = Amount::from_zents(1_000);

        let tx = build_transfer(&kp, &to_addr, amount, fee, &utxos).expect("build");

        // Greedy selects largest first: 50M + 30M + 25M = 105M, need 90M + 1K
        assert!(tx.inputs.len() >= 2);
        assert_eq!(tx.outputs[0].amount, amount);
    }

    #[test]
    fn test_build_transfer_insufficient_funds() {
        let kp = test_keypair();
        let to_addr = Address::from_public_key(&[99u8; 32], NetworkType::Mainnet);

        let utxos = test_utxos(&kp, &[10_000]);
        let amount = Amount::from_coins(1); // 100M zents - way more than available
        let fee = Amount::from_zents(1_000);

        let result = build_transfer(&kp, &to_addr, amount, fee, &utxos);
        assert!(result.is_err());

        match result {
            Err(ZentraError::InsufficientFunds { required, available }) => {
                assert_eq!(required, 100_001_000);
                assert_eq!(available, 10_000);
            }
            other => panic!("Expected InsufficientFunds, got {:?}", other),
        }
    }

    #[test]
    fn test_build_transfer_empty_utxos() {
        let kp = test_keypair();
        let to_addr = Address::from_public_key(&[99u8; 32], NetworkType::Mainnet);

        let result = build_transfer(
            &kp,
            &to_addr,
            Amount::from_coins(1),
            Amount::from_zents(1_000),
            &[],
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_transaction_hash_deterministic() {
        let tx = Transaction {
            version: 1,
            inputs: vec![TxInput {
                outpoint: OutPoint {
                    txid: Hash::hash(b"in"),
                    index: 0,
                },
                signature: vec![0u8; 64],
                public_key: [1u8; 32],
            }],
            outputs: vec![TxOutput {
                amount: Amount::from_coins(1),
                address: Address::from_payload([2u8; 32], NetworkType::Mainnet),
            }],
            fee: Amount::from_zents(1_000),
            payload: Vec::new(),
        };

        let h1 = tx.hash();
        let h2 = tx.hash();
        assert_eq!(h1, h2, "Hash must be deterministic");
    }

    #[test]
    fn test_transaction_hash_excludes_signatures() {
        let kp = test_keypair();
        let tx1 = Transaction {
            version: 1,
            inputs: vec![TxInput {
                outpoint: OutPoint {
                    txid: Hash::hash(b"in"),
                    index: 0,
                },
                signature: vec![0u8; 64], // all zeros
                public_key: kp.public_key_bytes(),
            }],
            outputs: vec![],
            fee: Amount::ZERO,
            payload: Vec::new(),
        };

        let tx2 = Transaction {
            version: 1,
            inputs: vec![TxInput {
                outpoint: OutPoint {
                    txid: Hash::hash(b"in"),
                    index: 0,
                },
                signature: vec![0xFF; 64], // all 0xFF — different signature
                public_key: kp.public_key_bytes(),
            }],
            outputs: vec![],
            fee: Amount::ZERO,
            payload: Vec::new(),
        };

        assert_eq!(
            tx1.hash(),
            tx2.hash(),
            "Signatures must NOT affect the transaction hash"
        );
    }

    #[test]
    fn test_outpoint_equality() {
        let op1 = OutPoint {
            txid: Hash::hash(b"tx"),
            index: 0,
        };
        let op2 = OutPoint {
            txid: Hash::hash(b"tx"),
            index: 0,
        };
        assert_eq!(op1, op2);

        let op3 = OutPoint {
            txid: Hash::hash(b"tx"),
            index: 1,
        };
        assert_ne!(op1, op3);
    }
}
