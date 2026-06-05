//! GhostDAG ordering algorithm for the Zentra BlockDAG.

use borsh::{BorshSerialize, BorshDeserialize};
use serde::{Serialize, Deserialize};
use std::collections::HashSet;
use zentra_types::*;

/// GhostDAG data for a single block in the DAG.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct GhostdagData {
    /// Number of blue blocks in this block's past set
    pub blue_score: u64,
    /// Cumulative blue work (sum of difficulties of blue ancestors)
    pub blue_work: u128,
    /// The selected parent (parent with highest blue score)
    pub selected_parent: Hash,
    /// Blocks in the mergeset classified as blue (honest, well-connected)
    pub mergeset_blues: Vec<Hash>,
    /// Blocks in the mergeset classified as red (outside k-cluster)
    pub mergeset_reds: Vec<Hash>,
}

impl GhostdagData {
    /// GhostDAG data for the genesis block.
    pub fn genesis() -> Self {
        GhostdagData {
            blue_score: 0,
            blue_work: 0,
            selected_parent: Hash::ZERO,
            mergeset_blues: vec![],
            mergeset_reds: vec![],
        }
    }
}

/// GhostDAG ordering manager.
pub struct GhostdagManager {
    /// The k parameter: tolerance for parallel blocks (anticone size limit)
    pub k: u16,
}

impl GhostdagManager {
    /// Create a new GhostDAG manager with the given k parameter.
    pub fn new(k: u16) -> Self {
        GhostdagManager { k }
    }

    /// Create with default k from constants.
    pub fn default_k() -> Self {
        Self::new(GHOSTDAG_K)
    }

    /// Process a new block and compute its GhostDAG data.
    ///
    /// `get_ghostdag` is a closure that retrieves GhostDAG data for any known block hash.
    pub fn process_block(
        &self,
        _block_hash: &Hash,
        parent_hashes: &[Hash],
        get_ghostdag: impl Fn(&Hash) -> Option<GhostdagData>,
    ) -> GhostdagData {
        if parent_hashes.is_empty() {
            return GhostdagData::genesis();
        }

        // Step 1: Select the parent with the highest blue score
        let selected_parent = self.select_parent(parent_hashes, &get_ghostdag);
        let selected_data = get_ghostdag(&selected_parent).unwrap_or_else(GhostdagData::genesis);

        // Step 2: Classify mergeset (other parents) as blue or red
        let other_parents: Vec<Hash> = parent_hashes
            .iter()
            .filter(|p| **p != selected_parent)
            .copied()
            .collect();

        let (mergeset_blues, mergeset_reds) = self.classify_mergeset(
            &other_parents,
            &selected_data,
            &get_ghostdag,
        );

        // Step 3: Calculate blue score and blue work
        let blue_score = selected_data.blue_score + 1 + mergeset_blues.len() as u64;
        let blue_work = selected_data.blue_work + 1; // simplified: +1 per blue block

        GhostdagData {
            blue_score,
            blue_work,
            selected_parent,
            mergeset_blues,
            mergeset_reds,
        }
    }

    /// Select the parent with the highest blue score.
    fn select_parent(
        &self,
        parents: &[Hash],
        get_ghostdag: &impl Fn(&Hash) -> Option<GhostdagData>,
    ) -> Hash {
        parents
            .iter()
            .max_by_key(|p| {
                get_ghostdag(p).map(|d| d.blue_score).unwrap_or(0)
            })
            .copied()
            .unwrap_or(Hash::ZERO)
    }

    /// Classify mergeset blocks as blue or red.
    ///
    /// A block is blue if its anticone size relative to the blue set is <= k.
    /// Otherwise it's red (potential attacker / poorly connected).
    fn classify_mergeset(
        &self,
        other_parents: &[Hash],
        selected_data: &GhostdagData,
        get_ghostdag: &impl Fn(&Hash) -> Option<GhostdagData>,
    ) -> (Vec<Hash>, Vec<Hash>) {
        let mut blues = Vec::new();
        let mut reds = Vec::new();

        // Build the current blue set from selected parent's data
        let mut blue_set: HashSet<Hash> = HashSet::new();
        blue_set.insert(selected_data.selected_parent);
        for b in &selected_data.mergeset_blues {
            blue_set.insert(*b);
        }

        for parent in other_parents {
            let parent_data = get_ghostdag(parent).unwrap_or_else(GhostdagData::genesis);

            // Simplified anticone calculation:
            // Count how many blue blocks are NOT in this parent's past
            let anticone_size = self.estimate_anticone_size(parent, &blue_set, get_ghostdag);

            if anticone_size <= self.k as usize {
                blues.push(*parent);
                blue_set.insert(*parent);
                // Also add this parent's blues to the set
                for b in &parent_data.mergeset_blues {
                    blue_set.insert(*b);
                }
            } else {
                reds.push(*parent);
            }
        }

        (blues, reds)
    }

    /// Estimate the anticone size of a block relative to the blue set.
    ///
    /// The anticone is the set of blocks that are neither in the block's past
    /// nor in the block's future — they are "parallel" blocks.
    fn estimate_anticone_size(
        &self,
        block: &Hash,
        blue_set: &HashSet<Hash>,
        get_ghostdag: &impl Fn(&Hash) -> Option<GhostdagData>,
    ) -> usize {
        let block_data = get_ghostdag(block).unwrap_or_else(GhostdagData::genesis);

        // Build the block's known past set
        let mut past: HashSet<Hash> = HashSet::new();
        past.insert(block_data.selected_parent);
        for b in &block_data.mergeset_blues {
            past.insert(*b);
        }
        for r in &block_data.mergeset_reds {
            past.insert(*r);
        }

        // Anticone = blue blocks NOT in the block's past
        blue_set.iter().filter(|b| !past.contains(b) && *b != block).count()
    }

    /// Walk the selected parent chain from a tip back to genesis.
    pub fn get_selected_chain(
        &self,
        tip: &Hash,
        get_ghostdag: impl Fn(&Hash) -> Option<GhostdagData>,
    ) -> Vec<Hash> {
        let mut chain = vec![*tip];
        let mut current = *tip;

        loop {
            match get_ghostdag(&current) {
                Some(data) if data.selected_parent != Hash::ZERO => {
                    chain.push(data.selected_parent);
                    current = data.selected_parent;
                }
                _ => break,
            }
        }

        chain
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_genesis_ghostdag() {
        let mgr = GhostdagManager::default_k();
        let data = mgr.process_block(&Hash::hash(b"genesis"), &[], |_| None);
        assert_eq!(data.blue_score, 0);
        assert_eq!(data.selected_parent, Hash::ZERO);
    }

    #[test]
    fn test_single_parent_chain() {
        let mgr = GhostdagManager::default_k();
        let mut store: HashMap<Hash, GhostdagData> = HashMap::new();

        let genesis = Hash::hash(b"genesis");
        store.insert(genesis, GhostdagData::genesis());

        let block1 = Hash::hash(b"block1");
        let data1 = mgr.process_block(&block1, &[genesis], |h| store.get(h).cloned());
        assert_eq!(data1.blue_score, 1);
        assert_eq!(data1.selected_parent, genesis);
        store.insert(block1, data1);

        let block2 = Hash::hash(b"block2");
        let data2 = mgr.process_block(&block2, &[block1], |h| store.get(h).cloned());
        assert_eq!(data2.blue_score, 2);
        assert_eq!(data2.selected_parent, block1);
    }

    #[test]
    fn test_parallel_blocks_merge() {
        let mgr = GhostdagManager::new(10); // high k to accept parallel blocks
        let mut store: HashMap<Hash, GhostdagData> = HashMap::new();

        let genesis = Hash::hash(b"genesis");
        store.insert(genesis, GhostdagData::genesis());

        // Two parallel blocks, both referencing genesis
        let block_a = Hash::hash(b"block_a");
        let data_a = mgr.process_block(&block_a, &[genesis], |h| store.get(h).cloned());
        store.insert(block_a, data_a);

        let block_b = Hash::hash(b"block_b");
        let data_b = mgr.process_block(&block_b, &[genesis], |h| store.get(h).cloned());
        store.insert(block_b, data_b);

        // Merge block referencing both parallel blocks
        let merge = Hash::hash(b"merge");
        let merge_data = mgr.process_block(&merge, &[block_a, block_b], |h| store.get(h).cloned());

        // Blue score should include both parallel blocks
        assert!(merge_data.blue_score >= 2, "merge blue score: {}", merge_data.blue_score);
    }

    #[test]
    fn test_selected_chain() {
        let mgr = GhostdagManager::default_k();
        let mut store: HashMap<Hash, GhostdagData> = HashMap::new();

        let genesis = Hash::hash(b"genesis");
        store.insert(genesis, GhostdagData::genesis());

        let block1 = Hash::hash(b"b1");
        let d1 = mgr.process_block(&block1, &[genesis], |h| store.get(h).cloned());
        store.insert(block1, d1);

        let block2 = Hash::hash(b"b2");
        let d2 = mgr.process_block(&block2, &[block1], |h| store.get(h).cloned());
        store.insert(block2, d2);

        let chain = mgr.get_selected_chain(&block2, |h| store.get(h).cloned());
        assert_eq!(chain, vec![block2, block1, genesis]);
    }
}
