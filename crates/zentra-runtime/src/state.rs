//! # Contract State Storage
//!
//! Provides a per-contract key-value store backed by an in-memory
//! `HashMap<ContractId, HashMap<Vec<u8>, Vec<u8>>>`.
//!
//! This is the canonical state layer that smart contracts read from and
//! write to during execution. A Merkle state root can be computed
//! over the entire storage for consensus commitment.

use std::collections::HashMap;

use zentra_types::error::ZentraResult;
use zentra_types::Hash;

use crate::engine::ContractId;

/// Per-contract key-value storage.
///
/// Each contract has its own isolated namespace of `Vec<u8> → Vec<u8>` mappings.
/// State is kept in memory; a persistent backend (e.g., RocksDB) can wrap this
/// struct in production.
#[derive(Debug, Clone)]
pub struct StateMap {
    /// Outer key: contract identifier.  Inner map: key → value.
    storage: HashMap<ContractId, HashMap<Vec<u8>, Vec<u8>>>,
}

impl Default for StateMap {
    fn default() -> Self {
        Self::new()
    }
}

impl StateMap {
    /// Create an empty state map.
    pub fn new() -> Self {
        StateMap {
            storage: HashMap::new(),
        }
    }

    /// Read a value from a contract's storage.
    ///
    /// Returns `None` if the contract has no entry for `key`.
    pub fn get(&self, contract: &ContractId, key: &[u8]) -> Option<Vec<u8>> {
        self.storage
            .get(contract)
            .and_then(|m| m.get(key).cloned())
    }

    /// Set a value in a contract's storage.
    ///
    /// Creates the contract's namespace if it doesn't already exist.
    pub fn set(&mut self, contract: &ContractId, key: &[u8], value: &[u8]) {
        tracing::trace!(
            contract = %contract,
            key_len = key.len(),
            value_len = value.len(),
            "State write"
        );

        self.storage
            .entry(contract.clone())
            .or_default()
            .insert(key.to_vec(), value.to_vec());
    }

    /// Delete a key from a contract's storage.
    ///
    /// Does nothing if the key doesn't exist.
    pub fn delete(&mut self, contract: &ContractId, key: &[u8]) {
        if let Some(map) = self.storage.get_mut(contract) {
            map.remove(key);
            // Clean up empty namespaces
            if map.is_empty() {
                self.storage.remove(contract);
            }
        }
    }

    /// Check whether a contract has any state entries.
    pub fn has_state(&self, contract: &ContractId) -> bool {
        self.storage
            .get(contract)
            .is_some_and(|m| !m.is_empty())
    }

    /// Return the number of keys stored for a particular contract.
    pub fn key_count(&self, contract: &ContractId) -> usize {
        self.storage.get(contract).map_or(0, |m| m.len())
    }

    /// Return the total number of contracts that have state entries.
    pub fn contract_count(&self) -> usize {
        self.storage.len()
    }

    /// Remove all state for a contract.
    pub fn clear_contract(&mut self, contract: &ContractId) {
        self.storage.remove(contract);
    }

    /// Compute a Merkle state root over all contract state.
    ///
    /// Algorithm:
    /// 1. For each contract, sort its keys lexicographically.
    /// 2. Hash each `(key, value)` pair: `H(key ‖ value)`.
    /// 3. Build a binary Merkle tree over the sorted hashes.
    /// 4. Combine per-contract roots using the contract ID as a domain separator.
    /// 5. Build a final Merkle tree over the sorted contract roots.
    ///
    /// Returns [`Hash::ZERO`] for an empty state.
    pub fn state_root(&self) -> Hash {
        if self.storage.is_empty() {
            return Hash::ZERO;
        }

        // Collect per-contract roots
        let mut contract_hashes: Vec<(ContractId, Hash)> = self
            .storage
            .iter()
            .map(|(cid, kv_map)| {
                let root = Self::merkle_root_for_contract(cid, kv_map);
                (cid.clone(), root)
            })
            .collect();

        // Sort by contract ID for determinism
        contract_hashes.sort_by(|a, b| a.0 .0.cmp(&b.0 .0));

        // Build final Merkle tree from contract roots
        let leaves: Vec<Hash> = contract_hashes.into_iter().map(|(_, h)| h).collect();
        Self::build_merkle_tree(&leaves)
    }

    /// Compute the Merkle root for a single contract's key-value pairs.
    fn merkle_root_for_contract(
        contract_id: &ContractId,
        kv_map: &HashMap<Vec<u8>, Vec<u8>>,
    ) -> Hash {
        if kv_map.is_empty() {
            return Hash::ZERO;
        }

        // Sort keys for deterministic ordering
        let mut entries: Vec<(&Vec<u8>, &Vec<u8>)> = kv_map.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));

        // Hash each entry: H(contract_id ‖ key ‖ value)
        let leaves: Vec<Hash> = entries
            .iter()
            .map(|(k, v)| {
                let mut data = Vec::with_capacity(32 + k.len() + v.len());
                data.extend_from_slice(&contract_id.0);
                data.extend_from_slice(k);
                data.extend_from_slice(v);
                Hash::hash(&data)
            })
            .collect();

        Self::build_merkle_tree(&leaves)
    }

    /// Build a binary Merkle tree from a slice of leaf hashes.
    ///
    /// If only one leaf exists, returns it directly.
    /// Unpaired leaves are combined with themselves (duplicated).
    fn build_merkle_tree(leaves: &[Hash]) -> Hash {
        if leaves.is_empty() {
            return Hash::ZERO;
        }
        if leaves.len() == 1 {
            return leaves[0];
        }

        let mut current_level = leaves.to_vec();

        while current_level.len() > 1 {
            let mut next_level = Vec::with_capacity((current_level.len() + 1) / 2);

            let mut i = 0;
            while i < current_level.len() {
                let left = &current_level[i];
                let right = if i + 1 < current_level.len() {
                    &current_level[i + 1]
                } else {
                    left // duplicate the last leaf if odd
                };
                next_level.push(Hash::combine(left, right));
                i += 2;
            }

            current_level = next_level;
        }

        current_level[0]
    }
}

// ─── Unit Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn cid(n: u8) -> ContractId {
        ContractId([n; 32])
    }

    #[test]
    fn test_new_state_map() {
        let sm = StateMap::new();
        assert_eq!(sm.contract_count(), 0);
        assert_eq!(sm.state_root(), Hash::ZERO);
    }

    #[test]
    fn test_set_and_get() {
        let mut sm = StateMap::new();
        let c = cid(1);

        sm.set(&c, b"key1", b"value1");
        assert_eq!(sm.get(&c, b"key1"), Some(b"value1".to_vec()));
        assert_eq!(sm.get(&c, b"nonexistent"), None);
    }

    #[test]
    fn test_overwrite() {
        let mut sm = StateMap::new();
        let c = cid(1);

        sm.set(&c, b"key", b"v1");
        sm.set(&c, b"key", b"v2");
        assert_eq!(sm.get(&c, b"key"), Some(b"v2".to_vec()));
        assert_eq!(sm.key_count(&c), 1);
    }

    #[test]
    fn test_delete() {
        let mut sm = StateMap::new();
        let c = cid(1);

        sm.set(&c, b"key", b"value");
        sm.delete(&c, b"key");
        assert_eq!(sm.get(&c, b"key"), None);
        // Contract namespace should be cleaned up
        assert!(!sm.has_state(&c));
    }

    #[test]
    fn test_delete_nonexistent() {
        let mut sm = StateMap::new();
        let c = cid(1);
        // Should not panic
        sm.delete(&c, b"nothing");
    }

    #[test]
    fn test_has_state() {
        let mut sm = StateMap::new();
        let c = cid(1);

        assert!(!sm.has_state(&c));
        sm.set(&c, b"k", b"v");
        assert!(sm.has_state(&c));
    }

    #[test]
    fn test_key_count() {
        let mut sm = StateMap::new();
        let c = cid(1);

        assert_eq!(sm.key_count(&c), 0);
        sm.set(&c, b"a", b"1");
        sm.set(&c, b"b", b"2");
        sm.set(&c, b"c", b"3");
        assert_eq!(sm.key_count(&c), 3);

        sm.delete(&c, b"b");
        assert_eq!(sm.key_count(&c), 2);
    }

    #[test]
    fn test_contract_count() {
        let mut sm = StateMap::new();

        sm.set(&cid(1), b"k", b"v");
        sm.set(&cid(2), b"k", b"v");
        sm.set(&cid(3), b"k", b"v");
        assert_eq!(sm.contract_count(), 3);
    }

    #[test]
    fn test_clear_contract() {
        let mut sm = StateMap::new();
        let c = cid(1);

        sm.set(&c, b"a", b"1");
        sm.set(&c, b"b", b"2");
        sm.clear_contract(&c);

        assert!(!sm.has_state(&c));
        assert_eq!(sm.key_count(&c), 0);
    }

    #[test]
    fn test_isolation_between_contracts() {
        let mut sm = StateMap::new();
        let c1 = cid(1);
        let c2 = cid(2);

        sm.set(&c1, b"key", b"from-c1");
        sm.set(&c2, b"key", b"from-c2");

        assert_eq!(sm.get(&c1, b"key"), Some(b"from-c1".to_vec()));
        assert_eq!(sm.get(&c2, b"key"), Some(b"from-c2".to_vec()));
    }

    #[test]
    fn test_state_root_deterministic() {
        let mut sm = StateMap::new();
        let c = cid(1);

        sm.set(&c, b"a", b"1");
        sm.set(&c, b"b", b"2");

        let root1 = sm.state_root();
        let root2 = sm.state_root();
        assert_eq!(root1, root2, "State root must be deterministic");
    }

    #[test]
    fn test_state_root_changes_on_write() {
        let mut sm = StateMap::new();
        let c = cid(1);

        sm.set(&c, b"key", b"v1");
        let root1 = sm.state_root();

        sm.set(&c, b"key", b"v2");
        let root2 = sm.state_root();

        assert_ne!(root1, root2, "State root must change when state changes");
    }

    #[test]
    fn test_state_root_insertion_order_independent() {
        let c = cid(1);

        let mut sm1 = StateMap::new();
        sm1.set(&c, b"a", b"1");
        sm1.set(&c, b"b", b"2");

        let mut sm2 = StateMap::new();
        sm2.set(&c, b"b", b"2");
        sm2.set(&c, b"a", b"1");

        assert_eq!(
            sm1.state_root(),
            sm2.state_root(),
            "State root must be independent of insertion order"
        );
    }

    #[test]
    fn test_state_root_empty() {
        let sm = StateMap::new();
        assert_eq!(sm.state_root(), Hash::ZERO);
    }

    #[test]
    fn test_state_root_single_entry() {
        let mut sm = StateMap::new();
        sm.set(&cid(1), b"key", b"value");
        let root = sm.state_root();
        assert_ne!(root, Hash::ZERO);
    }

    #[test]
    fn test_state_root_with_multiple_contracts() {
        let mut sm = StateMap::new();
        sm.set(&cid(1), b"a", b"1");
        sm.set(&cid(2), b"b", b"2");

        let root = sm.state_root();
        assert_ne!(root, Hash::ZERO);

        // Adding a third contract should change the root
        sm.set(&cid(3), b"c", b"3");
        let root2 = sm.state_root();
        assert_ne!(root, root2);
    }

    #[test]
    fn test_default() {
        let sm = StateMap::default();
        assert_eq!(sm.contract_count(), 0);
    }
}
