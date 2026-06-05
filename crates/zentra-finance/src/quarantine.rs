//! # Cryptographic Quarantine System
//!
//! Implements a penalty mechanism for validators and nodes that violate protocol
//! rules. Quarantined nodes have their collateral locked for an extended period
//! (~2 years) and lose mining weight for the duration.
//!
//! ## Quarantine Reasons
//! - **InvalidSignature**: Submitting forged or incorrect threshold signatures
//! - **MaliciousRelease**: Attempting unauthorized releases from the vault
//! - **ProtocolViolation**: Any other serious protocol breach
//!
//! ## Lock Duration
//! The quarantine lock lasts `QUARANTINE_LOCK_BLOCKS` (~2 years at 20 BPS).
//! After expiry, the node can reclaim its collateral.

use std::collections::HashMap;

use serde::{Serialize, Deserialize};
use zentra_types::{ZentraError, QUARANTINE_LOCK_BLOCKS};
use zentra_types::error::ZentraResult;

/// Reason a node was placed into quarantine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QuarantineReason {
    /// The node submitted a forged or invalid threshold signature.
    InvalidSignature,
    /// The node attempted an unauthorized release from the vault.
    MaliciousRelease,
    /// The node committed a generic serious protocol violation.
    ProtocolViolation,
}

impl std::fmt::Display for QuarantineReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSignature => write!(f, "invalid signature"),
            Self::MaliciousRelease => write!(f, "malicious release attempt"),
            Self::ProtocolViolation => write!(f, "protocol violation"),
        }
    }
}

/// A record of a quarantined node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuarantineEntry {
    /// The 32-byte node identifier (public key hash).
    pub node_id: [u8; 32],
    /// Block height at which the quarantine was imposed.
    pub quarantined_at_height: u64,
    /// Block height at which the quarantine expires and collateral can be unlocked.
    pub unlock_height: u64,
    /// Amount of collateral locked (in zents).
    pub locked_collateral: u128,
    /// Reason for the quarantine.
    pub reason: QuarantineReason,
}

/// Manages quarantined nodes and their collateral locks.
///
/// Provides methods to quarantine nodes, check quarantine status,
/// and process unlocks after the lock period expires.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuarantineManager {
    /// Map from node ID to quarantine entry.
    quarantined_nodes: HashMap<[u8; 32], QuarantineEntry>,
}

impl QuarantineManager {
    /// Create a new empty quarantine manager.
    pub fn new() -> Self {
        tracing::info!("quarantine manager initialized");
        Self {
            quarantined_nodes: HashMap::new(),
        }
    }

    /// Place a node into quarantine.
    ///
    /// The node's collateral is locked for `QUARANTINE_LOCK_BLOCKS` (~2 years).
    /// During quarantine, the node loses all mining weight and cannot participate
    /// in consensus or TSS signing.
    ///
    /// If the node is already quarantined, the quarantine is extended from the
    /// current height and the collateral amounts are combined.
    ///
    /// # Arguments
    /// - `node_id`: 32-byte identifier of the offending node.
    /// - `current_height`: Current blockchain height.
    /// - `collateral`: Amount of collateral to lock (in zents).
    /// - `reason`: The reason for quarantine.
    ///
    /// # Returns
    /// The quarantine entry that was created.
    pub fn execute_quarantine(
        &mut self,
        node_id: [u8; 32],
        current_height: u64,
        collateral: u128,
        reason: QuarantineReason,
    ) -> QuarantineEntry {
        let unlock_height = current_height.saturating_add(QUARANTINE_LOCK_BLOCKS);

        let entry = if let Some(existing) = self.quarantined_nodes.get_mut(&node_id) {
            // Already quarantined — extend and add collateral
            tracing::warn!(
                node_id = hex::encode(node_id),
                reason = %reason,
                "node re-quarantined — extending lock and adding collateral"
            );
            existing.quarantined_at_height = current_height;
            existing.unlock_height = unlock_height;
            existing.locked_collateral = existing.locked_collateral.saturating_add(collateral);
            existing.reason = reason;
            existing.clone()
        } else {
            let entry = QuarantineEntry {
                node_id,
                quarantined_at_height: current_height,
                unlock_height,
                locked_collateral: collateral,
                reason,
            };
            self.quarantined_nodes.insert(node_id, entry.clone());

            tracing::warn!(
                node_id = hex::encode(node_id),
                current_height,
                unlock_height,
                collateral,
                reason = %reason,
                "node quarantined — collateral locked, mining weight stripped"
            );

            entry
        };

        entry
    }

    /// Check if a node is currently quarantined.
    ///
    /// Returns `true` if the node has an active quarantine entry,
    /// regardless of whether the lock period has expired. Use
    /// [`can_unlock`] to check if the quarantine can be lifted.
    pub fn is_quarantined(&self, node_id: &[u8; 32]) -> bool {
        self.quarantined_nodes.contains_key(node_id)
    }

    /// Check if a quarantined node's lock period has expired and can be unlocked.
    ///
    /// Returns `false` if the node is not quarantined or if the lock has not
    /// yet expired.
    pub fn can_unlock(&self, node_id: &[u8; 32], current_height: u64) -> bool {
        match self.quarantined_nodes.get(node_id) {
            Some(entry) => current_height >= entry.unlock_height,
            None => false,
        }
    }

    /// Process the unlock of a quarantined node.
    ///
    /// The node must be quarantined and the lock period must have expired.
    /// Upon unlock, the quarantine entry is removed and the locked collateral
    /// amount is returned.
    ///
    /// # Returns
    /// The amount of collateral unlocked (in zents).
    ///
    /// # Errors
    /// - `ZentraError::QuarantineError` if the node is not quarantined.
    /// - `ZentraError::QuarantineError` if the lock period has not expired.
    pub fn process_unlock(
        &mut self,
        node_id: &[u8; 32],
        current_height: u64,
    ) -> ZentraResult<u128> {
        let entry = self
            .quarantined_nodes
            .get(node_id)
            .ok_or_else(|| {
                ZentraError::QuarantineError(format!(
                    "node {} is not quarantined",
                    hex::encode(node_id)
                ))
            })?;

        if current_height < entry.unlock_height {
            let blocks_remaining = entry.unlock_height - current_height;
            return Err(ZentraError::QuarantineError(format!(
                "quarantine lock has not expired — {} blocks remaining (current: {}, unlock: {})",
                blocks_remaining, current_height, entry.unlock_height
            )));
        }

        let collateral = entry.locked_collateral;
        self.quarantined_nodes.remove(node_id);

        tracing::info!(
            node_id = hex::encode(node_id),
            collateral,
            current_height,
            "quarantine lifted — collateral unlocked"
        );

        Ok(collateral)
    }

    /// Get the quarantine entry for a specific node, if it exists.
    pub fn get_quarantine_info(&self, node_id: &[u8; 32]) -> Option<&QuarantineEntry> {
        self.quarantined_nodes.get(node_id)
    }

    /// Get the total number of currently quarantined nodes.
    pub fn quarantined_count(&self) -> usize {
        self.quarantined_nodes.len()
    }
}

impl Default for QuarantineManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_NODE: [u8; 32] = [0xAAu8; 32];
    const OTHER_NODE: [u8; 32] = [0xBBu8; 32];
    const COLLATERAL: u128 = 100_000_000_000; // 1000 ZTR in zents

    #[test]
    fn test_new_manager() {
        let mgr = QuarantineManager::new();
        assert_eq!(mgr.quarantined_count(), 0);
    }

    #[test]
    fn test_execute_quarantine() {
        let mut mgr = QuarantineManager::new();
        let entry = mgr.execute_quarantine(
            TEST_NODE,
            1000,
            COLLATERAL,
            QuarantineReason::InvalidSignature,
        );

        assert_eq!(entry.node_id, TEST_NODE);
        assert_eq!(entry.quarantined_at_height, 1000);
        assert_eq!(entry.unlock_height, 1000 + QUARANTINE_LOCK_BLOCKS);
        assert_eq!(entry.locked_collateral, COLLATERAL);
        assert_eq!(entry.reason, QuarantineReason::InvalidSignature);
        assert_eq!(mgr.quarantined_count(), 1);
    }

    #[test]
    fn test_is_quarantined() {
        let mut mgr = QuarantineManager::new();
        assert!(!mgr.is_quarantined(&TEST_NODE));

        mgr.execute_quarantine(TEST_NODE, 0, COLLATERAL, QuarantineReason::ProtocolViolation);
        assert!(mgr.is_quarantined(&TEST_NODE));
        assert!(!mgr.is_quarantined(&OTHER_NODE));
    }

    #[test]
    fn test_can_unlock_before_expiry() {
        let mut mgr = QuarantineManager::new();
        mgr.execute_quarantine(TEST_NODE, 1000, COLLATERAL, QuarantineReason::MaliciousRelease);

        // Before unlock height
        assert!(!mgr.can_unlock(&TEST_NODE, 1000));
        assert!(!mgr.can_unlock(&TEST_NODE, 1000 + QUARANTINE_LOCK_BLOCKS - 1));
    }

    #[test]
    fn test_can_unlock_at_expiry() {
        let mut mgr = QuarantineManager::new();
        mgr.execute_quarantine(TEST_NODE, 1000, COLLATERAL, QuarantineReason::InvalidSignature);

        let unlock_height = 1000 + QUARANTINE_LOCK_BLOCKS;
        assert!(mgr.can_unlock(&TEST_NODE, unlock_height));
        assert!(mgr.can_unlock(&TEST_NODE, unlock_height + 1));
    }

    #[test]
    fn test_can_unlock_not_quarantined() {
        let mgr = QuarantineManager::new();
        assert!(!mgr.can_unlock(&TEST_NODE, u64::MAX));
    }

    #[test]
    fn test_process_unlock_success() {
        let mut mgr = QuarantineManager::new();
        mgr.execute_quarantine(TEST_NODE, 0, COLLATERAL, QuarantineReason::ProtocolViolation);

        let unlock_height = QUARANTINE_LOCK_BLOCKS;
        let collateral = mgr.process_unlock(&TEST_NODE, unlock_height).unwrap();
        assert_eq!(collateral, COLLATERAL);
        assert!(!mgr.is_quarantined(&TEST_NODE));
        assert_eq!(mgr.quarantined_count(), 0);
    }

    #[test]
    fn test_process_unlock_too_early() {
        let mut mgr = QuarantineManager::new();
        mgr.execute_quarantine(TEST_NODE, 1000, COLLATERAL, QuarantineReason::InvalidSignature);

        let result = mgr.process_unlock(&TEST_NODE, 2000);
        assert!(result.is_err());
        assert!(mgr.is_quarantined(&TEST_NODE)); // Still quarantined
    }

    #[test]
    fn test_process_unlock_not_quarantined() {
        let mut mgr = QuarantineManager::new();
        let result = mgr.process_unlock(&TEST_NODE, 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_re_quarantine_extends_lock() {
        let mut mgr = QuarantineManager::new();
        mgr.execute_quarantine(TEST_NODE, 100, COLLATERAL, QuarantineReason::InvalidSignature);

        // Re-quarantine at a later height with additional collateral
        let entry = mgr.execute_quarantine(
            TEST_NODE,
            5000,
            COLLATERAL * 2,
            QuarantineReason::MaliciousRelease,
        );

        // Lock should be extended from new height
        assert_eq!(entry.quarantined_at_height, 5000);
        assert_eq!(entry.unlock_height, 5000 + QUARANTINE_LOCK_BLOCKS);
        // Collateral should be combined
        assert_eq!(entry.locked_collateral, COLLATERAL + COLLATERAL * 2);
        assert_eq!(entry.reason, QuarantineReason::MaliciousRelease);
        // Still only 1 entry
        assert_eq!(mgr.quarantined_count(), 1);
    }

    #[test]
    fn test_get_quarantine_info() {
        let mut mgr = QuarantineManager::new();
        assert!(mgr.get_quarantine_info(&TEST_NODE).is_none());

        mgr.execute_quarantine(TEST_NODE, 42, COLLATERAL, QuarantineReason::ProtocolViolation);
        let info = mgr.get_quarantine_info(&TEST_NODE).unwrap();
        assert_eq!(info.quarantined_at_height, 42);
        assert_eq!(info.locked_collateral, COLLATERAL);
    }

    #[test]
    fn test_multiple_nodes() {
        let mut mgr = QuarantineManager::new();
        mgr.execute_quarantine(TEST_NODE, 0, COLLATERAL, QuarantineReason::InvalidSignature);
        mgr.execute_quarantine(OTHER_NODE, 0, COLLATERAL / 2, QuarantineReason::MaliciousRelease);

        assert_eq!(mgr.quarantined_count(), 2);
        assert!(mgr.is_quarantined(&TEST_NODE));
        assert!(mgr.is_quarantined(&OTHER_NODE));

        // Unlock one
        mgr.process_unlock(&TEST_NODE, QUARANTINE_LOCK_BLOCKS).unwrap();
        assert_eq!(mgr.quarantined_count(), 1);
        assert!(!mgr.is_quarantined(&TEST_NODE));
        assert!(mgr.is_quarantined(&OTHER_NODE));
    }

    #[test]
    fn test_quarantine_reason_display() {
        assert_eq!(
            format!("{}", QuarantineReason::InvalidSignature),
            "invalid signature"
        );
        assert_eq!(
            format!("{}", QuarantineReason::MaliciousRelease),
            "malicious release attempt"
        );
        assert_eq!(
            format!("{}", QuarantineReason::ProtocolViolation),
            "protocol violation"
        );
    }

    #[test]
    fn test_unlock_height_saturating() {
        let mut mgr = QuarantineManager::new();
        // Very high current height — should not overflow
        let entry = mgr.execute_quarantine(
            TEST_NODE,
            u64::MAX - 100,
            COLLATERAL,
            QuarantineReason::ProtocolViolation,
        );
        // unlock_height should saturate at u64::MAX
        assert_eq!(entry.unlock_height, u64::MAX);
    }
}
