//! # Forkless Governance
//!
//! On-chain governance engine that allows network participants to propose
//! and vote on parameter changes without requiring a hard fork.
//!
//! ## Workflow
//!
//! 1. A participant calls [`GovernanceEngine::propose`] with new parameters
//!    and the current block height. A voting window opens.
//! 2. Participants call [`GovernanceEngine::vote`] with their voting power
//!    (typically proportional to mining hashrate or stake).
//! 3. At each block, [`GovernanceEngine::process_proposals`] is called. If a
//!    proposal has passed its expiry height and `votes_for > votes_against`,
//!    the new parameters take effect immediately.

use serde::{Deserialize, Serialize};

use zentra_types::constants::{MAX_BLOCK_SIZE, MAX_TXS_PER_BLOCK, MIN_TX_FEE_ZENTS};
use zentra_types::error::{ZentraError, ZentraResult};

/// Default voting window in blocks (~1 day at 20 BPS = 1,728,000 blocks).
const DEFAULT_VOTING_WINDOW_BLOCKS: u64 = 1_728_000;

// ─── GovernanceParams ──────────────────────────────────────────────────────────

/// Tunable network parameters managed by on-chain governance.
///
/// These parameters control block limits, fee thresholds, and gas pricing.
/// Changes take effect after a successful governance vote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GovernanceParams {
    /// Multiplier applied to the base gas price (in basis points; 10_000 = 1x).
    pub gas_price_multiplier: u64,
    /// Maximum block size in bytes.
    pub max_block_size: usize,
    /// Maximum number of transactions per block.
    pub max_txs_per_block: usize,
    /// Minimum transaction fee in zents (anti-spam).
    pub min_tx_fee: u64,
}

impl Default for GovernanceParams {
    fn default() -> Self {
        GovernanceParams {
            gas_price_multiplier: 10_000, // 1.0x
            max_block_size: MAX_BLOCK_SIZE,
            max_txs_per_block: MAX_TXS_PER_BLOCK,
            min_tx_fee: MIN_TX_FEE_ZENTS,
        }
    }
}

impl GovernanceParams {
    /// Validate that the parameters are within sane bounds.
    pub fn validate(&self) -> ZentraResult<()> {
        if self.gas_price_multiplier == 0 {
            return Err(ZentraError::WasmRuntime(
                "Gas price multiplier cannot be zero".to_string(),
            ));
        }
        if self.max_block_size == 0 {
            return Err(ZentraError::WasmRuntime(
                "Max block size cannot be zero".to_string(),
            ));
        }
        if self.max_txs_per_block == 0 {
            return Err(ZentraError::WasmRuntime(
                "Max txs per block cannot be zero".to_string(),
            ));
        }
        // Reasonable upper bounds
        if self.max_block_size > 100 * 1_048_576 {
            return Err(ZentraError::WasmRuntime(
                "Max block size exceeds 100 MB limit".to_string(),
            ));
        }
        if self.max_txs_per_block > 1_000_000 {
            return Err(ZentraError::WasmRuntime(
                "Max txs per block exceeds 1M limit".to_string(),
            ));
        }
        Ok(())
    }
}

// ─── GovernanceProposal ────────────────────────────────────────────────────────

/// A governance proposal to change network parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceProposal {
    /// Unique proposal identifier (monotonically increasing).
    pub id: u64,
    /// The proposed new parameters.
    pub proposed_params: GovernanceParams,
    /// Total voting power in favor.
    pub votes_for: u64,
    /// Total voting power against.
    pub votes_against: u64,
    /// Block height at which the proposal was created.
    pub proposed_at_height: u64,
    /// Block height at which the voting window closes.
    pub expires_at_height: u64,
}

impl GovernanceProposal {
    /// Check whether the proposal has passed (more votes for than against).
    pub fn is_approved(&self) -> bool {
        self.votes_for > self.votes_against && self.votes_for > 0
    }

    /// Check whether the voting window has expired at the given height.
    pub fn is_expired(&self, current_height: u64) -> bool {
        current_height >= self.expires_at_height
    }
}

// ─── GovernanceEngine ──────────────────────────────────────────────────────────

/// The governance engine manages the current parameters and pending proposals.
#[derive(Debug, Clone)]
pub struct GovernanceEngine {
    /// The currently active network parameters.
    current_params: GovernanceParams,
    /// List of pending (not yet expired) proposals.
    pending_proposals: Vec<GovernanceProposal>,
    /// Counter for assigning unique proposal IDs.
    next_proposal_id: u64,
    /// History of applied parameter changes (proposal_id, height).
    applied_history: Vec<(u64, u64)>,
}

impl Default for GovernanceEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl GovernanceEngine {
    /// Create a new governance engine with default parameters.
    pub fn new() -> Self {
        tracing::info!("Governance engine initialised with default parameters");
        GovernanceEngine {
            current_params: GovernanceParams::default(),
            pending_proposals: Vec::new(),
            next_proposal_id: 1,
            applied_history: Vec::new(),
        }
    }

    /// Create a governance engine with custom initial parameters.
    pub fn with_params(params: GovernanceParams) -> Self {
        GovernanceEngine {
            current_params: params,
            pending_proposals: Vec::new(),
            next_proposal_id: 1,
            applied_history: Vec::new(),
        }
    }

    /// Submit a new governance proposal.
    ///
    /// The voting window is `DEFAULT_VOTING_WINDOW_BLOCKS` blocks long. Returns
    /// the assigned proposal ID.
    ///
    /// # Errors
    ///
    /// Returns [`ZentraError::WasmRuntime`] if the proposed parameters are invalid.
    pub fn propose(&mut self, params: GovernanceParams, height: u64) -> ZentraResult<u64> {
        params.validate()?;

        let id = self.next_proposal_id;
        self.next_proposal_id += 1;

        let proposal = GovernanceProposal {
            id,
            proposed_params: params,
            votes_for: 0,
            votes_against: 0,
            proposed_at_height: height,
            expires_at_height: height + DEFAULT_VOTING_WINDOW_BLOCKS,
        };

        tracing::info!(
            proposal_id = id,
            proposed_at = height,
            expires_at = proposal.expires_at_height,
            "New governance proposal submitted"
        );

        self.pending_proposals.push(proposal);
        Ok(id)
    }

    /// Cast a vote on an existing proposal.
    ///
    /// `voting_power` is the weight of the vote (e.g., proportional to hashrate).
    ///
    /// # Errors
    ///
    /// - [`ZentraError::WasmRuntime`] if the proposal ID is not found.
    /// - [`ZentraError::WasmRuntime`] if `voting_power` is zero.
    pub fn vote(
        &mut self,
        proposal_id: u64,
        approve: bool,
        voting_power: u64,
    ) -> ZentraResult<()> {
        if voting_power == 0 {
            return Err(ZentraError::WasmRuntime(
                "Voting power must be greater than zero".to_string(),
            ));
        }

        let proposal = self
            .pending_proposals
            .iter_mut()
            .find(|p| p.id == proposal_id)
            .ok_or_else(|| {
                ZentraError::WasmRuntime(format!("Proposal {} not found", proposal_id))
            })?;

        if approve {
            proposal.votes_for = proposal.votes_for.saturating_add(voting_power);
        } else {
            proposal.votes_against = proposal.votes_against.saturating_add(voting_power);
        }

        tracing::debug!(
            proposal_id,
            approve,
            voting_power,
            votes_for = proposal.votes_for,
            votes_against = proposal.votes_against,
            "Vote recorded"
        );

        Ok(())
    }

    /// Process all pending proposals at the given block height.
    ///
    /// Proposals whose voting window has closed are evaluated:
    /// - **Approved** (votes_for > votes_against): parameters are updated.
    /// - **Rejected**: proposal is discarded.
    ///
    /// Only the first approved proposal in a batch takes effect to avoid
    /// conflicting parameter changes.
    pub fn process_proposals(&mut self, current_height: u64) {
        let mut applied_this_round = false;

        // Partition proposals into expired and still-pending
        let mut still_pending = Vec::new();

        for proposal in self.pending_proposals.drain(..) {
            if proposal.is_expired(current_height) {
                if proposal.is_approved() && !applied_this_round {
                    tracing::info!(
                        proposal_id = proposal.id,
                        votes_for = proposal.votes_for,
                        votes_against = proposal.votes_against,
                        "Governance proposal APPROVED — applying new parameters"
                    );
                    self.current_params = proposal.proposed_params;
                    self.applied_history.push((proposal.id, current_height));
                    applied_this_round = true;
                } else if proposal.is_approved() {
                    tracing::warn!(
                        proposal_id = proposal.id,
                        "Governance proposal approved but skipped (another proposal already applied this round)"
                    );
                } else {
                    tracing::info!(
                        proposal_id = proposal.id,
                        votes_for = proposal.votes_for,
                        votes_against = proposal.votes_against,
                        "Governance proposal REJECTED"
                    );
                }
            } else {
                still_pending.push(proposal);
            }
        }

        self.pending_proposals = still_pending;
    }

    /// Get a reference to the currently active governance parameters.
    pub fn get_current_params(&self) -> &GovernanceParams {
        &self.current_params
    }

    /// Get the number of pending (not yet expired) proposals.
    pub fn pending_count(&self) -> usize {
        self.pending_proposals.len()
    }

    /// Get a reference to a specific pending proposal by ID.
    pub fn get_proposal(&self, proposal_id: u64) -> Option<&GovernanceProposal> {
        self.pending_proposals.iter().find(|p| p.id == proposal_id)
    }

    /// Get the history of applied governance changes as `(proposal_id, applied_at_height)`.
    pub fn applied_history(&self) -> &[(u64, u64)] {
        &self.applied_history
    }
}

// ─── Unit Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_params() {
        let params = GovernanceParams::default();
        assert_eq!(params.gas_price_multiplier, 10_000);
        assert_eq!(params.max_block_size, MAX_BLOCK_SIZE);
        assert_eq!(params.max_txs_per_block, MAX_TXS_PER_BLOCK);
        assert_eq!(params.min_tx_fee, MIN_TX_FEE_ZENTS);
    }

    #[test]
    fn test_params_validation_ok() {
        GovernanceParams::default().validate().unwrap();
    }

    #[test]
    fn test_params_validation_zero_gas() {
        let params = GovernanceParams {
            gas_price_multiplier: 0,
            ..Default::default()
        };
        assert!(params.validate().is_err());
    }

    #[test]
    fn test_params_validation_zero_block_size() {
        let params = GovernanceParams {
            max_block_size: 0,
            ..Default::default()
        };
        assert!(params.validate().is_err());
    }

    #[test]
    fn test_params_validation_zero_txs() {
        let params = GovernanceParams {
            max_txs_per_block: 0,
            ..Default::default()
        };
        assert!(params.validate().is_err());
    }

    #[test]
    fn test_params_validation_too_large_block() {
        let params = GovernanceParams {
            max_block_size: 200 * 1_048_576,
            ..Default::default()
        };
        assert!(params.validate().is_err());
    }

    #[test]
    fn test_new_engine() {
        let engine = GovernanceEngine::new();
        assert_eq!(engine.pending_count(), 0);
        assert_eq!(*engine.get_current_params(), GovernanceParams::default());
    }

    #[test]
    fn test_propose() {
        let mut engine = GovernanceEngine::new();
        let new_params = GovernanceParams {
            min_tx_fee: 5_000,
            ..Default::default()
        };

        let id = engine.propose(new_params, 1000).unwrap();
        assert_eq!(id, 1);
        assert_eq!(engine.pending_count(), 1);

        let proposal = engine.get_proposal(id).unwrap();
        assert_eq!(proposal.proposed_at_height, 1000);
        assert_eq!(proposal.expires_at_height, 1000 + DEFAULT_VOTING_WINDOW_BLOCKS);
    }

    #[test]
    fn test_propose_invalid_params() {
        let mut engine = GovernanceEngine::new();
        let bad_params = GovernanceParams {
            gas_price_multiplier: 0,
            ..Default::default()
        };
        assert!(engine.propose(bad_params, 1000).is_err());
        assert_eq!(engine.pending_count(), 0);
    }

    #[test]
    fn test_vote_for() {
        let mut engine = GovernanceEngine::new();
        let id = engine.propose(GovernanceParams::default(), 100).unwrap();

        engine.vote(id, true, 500).unwrap();
        let p = engine.get_proposal(id).unwrap();
        assert_eq!(p.votes_for, 500);
        assert_eq!(p.votes_against, 0);
    }

    #[test]
    fn test_vote_against() {
        let mut engine = GovernanceEngine::new();
        let id = engine.propose(GovernanceParams::default(), 100).unwrap();

        engine.vote(id, false, 300).unwrap();
        let p = engine.get_proposal(id).unwrap();
        assert_eq!(p.votes_for, 0);
        assert_eq!(p.votes_against, 300);
    }

    #[test]
    fn test_vote_multiple() {
        let mut engine = GovernanceEngine::new();
        let id = engine.propose(GovernanceParams::default(), 100).unwrap();

        engine.vote(id, true, 100).unwrap();
        engine.vote(id, true, 200).unwrap();
        engine.vote(id, false, 50).unwrap();

        let p = engine.get_proposal(id).unwrap();
        assert_eq!(p.votes_for, 300);
        assert_eq!(p.votes_against, 50);
    }

    #[test]
    fn test_vote_zero_power() {
        let mut engine = GovernanceEngine::new();
        let id = engine.propose(GovernanceParams::default(), 100).unwrap();
        assert!(engine.vote(id, true, 0).is_err());
    }

    #[test]
    fn test_vote_nonexistent_proposal() {
        let mut engine = GovernanceEngine::new();
        assert!(engine.vote(999, true, 100).is_err());
    }

    #[test]
    fn test_process_approved() {
        let mut engine = GovernanceEngine::new();
        let new_params = GovernanceParams {
            min_tx_fee: 42_000,
            ..Default::default()
        };
        let id = engine.propose(new_params.clone(), 100).unwrap();
        engine.vote(id, true, 1000).unwrap();

        // Process at expiry height
        engine.process_proposals(100 + DEFAULT_VOTING_WINDOW_BLOCKS);

        assert_eq!(engine.get_current_params().min_tx_fee, 42_000);
        assert_eq!(engine.pending_count(), 0);
        assert_eq!(engine.applied_history().len(), 1);
    }

    #[test]
    fn test_process_rejected() {
        let mut engine = GovernanceEngine::new();
        let id = engine.propose(GovernanceParams {
            min_tx_fee: 999_999,
            ..Default::default()
        }, 100).unwrap();

        // More votes against
        engine.vote(id, false, 1000).unwrap();
        engine.vote(id, true, 500).unwrap();

        engine.process_proposals(100 + DEFAULT_VOTING_WINDOW_BLOCKS);

        // Should NOT have changed
        assert_eq!(engine.get_current_params().min_tx_fee, MIN_TX_FEE_ZENTS);
        assert_eq!(engine.pending_count(), 0);
        assert!(engine.applied_history().is_empty());
    }

    #[test]
    fn test_process_no_votes_rejected() {
        let mut engine = GovernanceEngine::new();
        engine.propose(GovernanceParams {
            min_tx_fee: 123,
            ..Default::default()
        }, 100).unwrap();

        // No votes cast
        engine.process_proposals(100 + DEFAULT_VOTING_WINDOW_BLOCKS);

        // Should NOT change (zero votes means not approved)
        assert_eq!(engine.get_current_params().min_tx_fee, MIN_TX_FEE_ZENTS);
    }

    #[test]
    fn test_process_not_yet_expired() {
        let mut engine = GovernanceEngine::new();
        let id = engine.propose(GovernanceParams::default(), 100).unwrap();
        engine.vote(id, true, 1000).unwrap();

        // Process BEFORE expiry — should remain pending
        engine.process_proposals(100 + 1);
        assert_eq!(engine.pending_count(), 1);
    }

    #[test]
    fn test_multiple_proposals() {
        let mut engine = GovernanceEngine::new();

        let id1 = engine.propose(GovernanceParams {
            min_tx_fee: 2_000,
            ..Default::default()
        }, 100).unwrap();

        let id2 = engine.propose(GovernanceParams {
            min_tx_fee: 3_000,
            ..Default::default()
        }, 200).unwrap();

        assert_eq!(engine.pending_count(), 2);
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_proposal_ids_increment() {
        let mut engine = GovernanceEngine::new();
        let id1 = engine.propose(GovernanceParams::default(), 1).unwrap();
        let id2 = engine.propose(GovernanceParams::default(), 2).unwrap();
        let id3 = engine.propose(GovernanceParams::default(), 3).unwrap();
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(id3, 3);
    }

    #[test]
    fn test_default_impl() {
        let engine = GovernanceEngine::default();
        assert_eq!(engine.pending_count(), 0);
    }

    #[test]
    fn test_with_custom_params() {
        let params = GovernanceParams {
            gas_price_multiplier: 20_000,
            max_block_size: 2_000_000,
            max_txs_per_block: 20_000,
            min_tx_fee: 5_000,
        };
        let engine = GovernanceEngine::with_params(params.clone());
        assert_eq!(*engine.get_current_params(), params);
    }

    #[test]
    fn test_only_first_approved_proposal_applies() {
        let mut engine = GovernanceEngine::new();

        let id1 = engine.propose(GovernanceParams {
            min_tx_fee: 2_000,
            ..Default::default()
        }, 100).unwrap();

        let id2 = engine.propose(GovernanceParams {
            min_tx_fee: 3_000,
            ..Default::default()
        }, 100).unwrap();

        engine.vote(id1, true, 100).unwrap();
        engine.vote(id2, true, 200).unwrap();

        engine.process_proposals(100 + DEFAULT_VOTING_WINDOW_BLOCKS);

        // Only the first approved proposal should have taken effect
        assert_eq!(engine.get_current_params().min_tx_fee, 2_000);
        assert_eq!(engine.applied_history().len(), 1);
    }

    #[test]
    fn test_proposal_is_approved() {
        let p = GovernanceProposal {
            id: 1,
            proposed_params: GovernanceParams::default(),
            votes_for: 100,
            votes_against: 50,
            proposed_at_height: 0,
            expires_at_height: 100,
        };
        assert!(p.is_approved());
    }

    #[test]
    fn test_proposal_not_approved_equal_votes() {
        let p = GovernanceProposal {
            id: 1,
            proposed_params: GovernanceParams::default(),
            votes_for: 100,
            votes_against: 100,
            proposed_at_height: 0,
            expires_at_height: 100,
        };
        assert!(!p.is_approved(), "Equal votes should NOT count as approved");
    }

    #[test]
    fn test_proposal_expired() {
        let p = GovernanceProposal {
            id: 1,
            proposed_params: GovernanceParams::default(),
            votes_for: 0,
            votes_against: 0,
            proposed_at_height: 0,
            expires_at_height: 100,
        };
        assert!(!p.is_expired(99));
        assert!(p.is_expired(100));
        assert!(p.is_expired(101));
    }
}
