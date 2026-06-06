//! DAG Graph manager for the Zentra BlockDAG.

use std::sync::Arc;
use dashmap::DashMap;
use parking_lot::RwLock;
use zentra_types::*;
use crate::header::Header;
use crate::block::Block;
use crate::database::ZentraDb;
use crate::utxo::BlockUndoData;

/// Thread-safe BlockDAG graph manager with in-memory caches backed by RocksDB.
pub struct DagGraph {
    db: Arc<ZentraDb>,
    /// In-memory cache of block tips (blocks with no children)
    tips: RwLock<Vec<Hash>>,
    /// In-memory children lookup cache
    children_cache: DashMap<Hash, Vec<Hash>>,
    /// In-memory header cache for hot blocks
    header_cache: DashMap<Hash, Header>,
}

impl DagGraph {
    /// Create a new DAG graph manager backed by the given database.
    pub fn new(db: Arc<ZentraDb>) -> Self {
        DagGraph {
            db,
            tips: RwLock::new(vec![]),
            children_cache: DashMap::new(),
            header_cache: DashMap::new(),
        }
    }

    /// Insert a block into the DAG.
    pub fn insert_block(&self, block: &Block) -> ZentraResult<()> {
        let hash = block.hash();
        tracing::debug!(block_hash = %hash, parents = block.header.parents.len(), "inserting block into DAG");

        // Store the block in the database
        self.db.put_block(&hash, block)?;

        // If genesis (no parents), store genesis GhostDAG data (64 bytes of zero)
        if block.header.parents.is_empty() {
            self.db.put_ghostdag_raw(&hash, &vec![0u8; 64])?;
        }

        // Cache the header
        self.header_cache.insert(hash, block.header.clone());

        // Update parent-child relationships
        for parent_hash in &block.header.parents {
            self.add_child_relation(parent_hash, &hash)?;
        }

        // Update tips: this block is a new tip, its parents are no longer tips
        {
            let mut tips = self.tips.write();
            tips.retain(|tip| !block.header.parents.contains(tip));
            tips.push(hash);

            // Persist updated tips to database metadata
            if let Ok(serialized) = borsh::to_vec(&*tips) {
                let _ = self.db.put_metadata("dag_tips", &serialized);
            }
        }

        Ok(())
    }

    /// Get a full block by hash.
    pub fn get_block(&self, hash: &Hash) -> ZentraResult<Option<Block>> {
        self.db.get_block(hash)
    }

    /// Get a block header by hash (checks cache first).
    pub fn get_header(&self, hash: &Hash) -> ZentraResult<Option<Header>> {
        // Check cache first
        if let Some(header) = self.header_cache.get(hash) {
            return Ok(Some(header.clone()));
        }
        // Fall back to database
        let header = self.db.get_header(hash)?;
        if let Some(ref h) = header {
            self.header_cache.insert(*hash, h.clone());
        }
        Ok(header)
    }

    /// Get the parent hashes of a block.
    pub fn get_parents(&self, hash: &Hash) -> ZentraResult<Vec<Hash>> {
        if let Some(header) = self.get_header(hash)? {
            Ok(header.parents.clone())
        } else {
            Ok(vec![])
        }
    }

    /// Get the children of a block (blocks that reference this one as a parent).
    pub fn get_children(&self, hash: &Hash) -> ZentraResult<Vec<Hash>> {
        // Check cache
        if let Some(children) = self.children_cache.get(hash) {
            return Ok(children.clone());
        }
        // Fall back to database
        let children = self.db.get_children(hash)?;
        self.children_cache.insert(*hash, children.clone());
        Ok(children)
    }

    /// Add a parent → child relationship.
    pub fn add_child_relation(&self, parent: &Hash, child: &Hash) -> ZentraResult<()> {
        let mut children = self.get_children(parent)?;
        if !children.contains(child) {
            children.push(*child);
            self.db.put_children(parent, &children)?;
            self.children_cache.insert(*parent, children);
        }
        Ok(())
    }

    /// Get the current DAG tips (blocks with no children).
    pub fn get_tips(&self) -> Vec<Hash> {
        self.tips.read().clone()
    }

    /// Get the selected tip (tip with highest blue score).
    pub fn get_selected_tip(&self) -> ZentraResult<Option<Hash>> {
        let tips = self.get_tips();
        // Deterministic selection so every node picks the SAME tip given the same
        // DAG: highest blue_score, then highest blue_work, then lowest hash as a
        // canonical tie-break. Without the tie-break, equal-score tips resolved to
        // whichever happened to be first in the (per-node, restart-varying) tips
        // list — a silent fork source.
        let mut best: Option<(u64, u128, Hash)> = None; // (blue_score, blue_work, hash)
        for tip in &tips {
            if let Some(header) = self.get_header(tip)? {
                let cand = (header.blue_score, header.blue_work, *tip);
                let better = match &best {
                    None => true,
                    Some((bs, bw, bh)) => {
                        cand.0 > *bs
                            || (cand.0 == *bs && cand.1 > *bw)
                            || (cand.0 == *bs && cand.1 == *bw && cand.2 < *bh)
                    }
                };
                if better { best = Some(cand); }
            }
        }
        Ok(best.map(|(_, _, hash)| hash))
    }

    /// Check if `ancestor` is an ancestor of `descendant` in the DAG.
    /// Uses BFS traversal through parent pointers.
    pub fn is_ancestor(&self, ancestor: &Hash, descendant: &Hash) -> bool {
        if ancestor == descendant {
            return true;
        }

        let mut queue = vec![*descendant];
        let mut visited = std::collections::HashSet::new();
        visited.insert(*descendant);

        while let Some(current) = queue.pop() {
            let parents = match self.get_parents(&current) {
                Ok(p) => p,
                Err(_) => continue,
            };

            for parent in parents {
                if parent == *ancestor {
                    return true;
                }
                if visited.insert(parent) {
                    queue.push(parent);
                }
            }
        }

        false
    }

    /// Initialize the DAG with the genesis block.
    /// This is idempotent — safe to call on every node startup.
    pub fn init_genesis(&self, network: NetworkType) -> ZentraResult<Hash> {
        let genesis = Block::genesis(network);
        let hash = genesis.hash();

        // Only insert if the genesis block is not already stored
        if self.db.get_header(&hash)?.is_none() {
            self.insert_block(&genesis)?;
            tracing::info!(hash = %hash, "genesis block inserted");
        } else {
            // Genesis already exists — restore tips from database if present, otherwise fall back to genesis
            let mut restored = false;
            if let Ok(Some(data)) = self.db.get_metadata("dag_tips") {
                use borsh::BorshDeserialize;
                if let Ok(saved_tips) = Vec::<Hash>::try_from_slice(&data) {
                    if !saved_tips.is_empty() {
                        let mut tips = self.tips.write();
                        *tips = saved_tips;
                        tracing::info!(tips = ?tips, "restored DAG tips from database metadata");
                        restored = true;
                    }
                }
            }

            if !restored {
                let mut tips = self.tips.write();
                if tips.is_empty() {
                    tips.push(hash);
                    tracing::info!(hash = %hash, "genesis already present, restored as tip seed");
                }
            }
        }

        Ok(hash)
    }

    /// Get the total number of tips.
    pub fn tip_count(&self) -> usize {
        self.tips.read().len()
    }

    pub fn get_ghostdag_raw(&self, hash: &Hash) -> ZentraResult<Option<Vec<u8>>> {
        self.db.get_ghostdag_raw(hash)
    }

    pub fn put_ghostdag_raw(&self, hash: &Hash, value: &[u8]) -> ZentraResult<()> {
        self.db.put_ghostdag_raw(hash, value)
    }

    pub fn get_undo(&self, hash: &Hash) -> ZentraResult<Option<BlockUndoData>> {
        self.db.get_undo(hash)
    }

    pub fn put_undo(&self, hash: &Hash, undo: &BlockUndoData) -> ZentraResult<()> {
        self.db.put_undo(hash, undo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dag() -> DagGraph {
        let db = ZentraDb::open_temp().unwrap();
        DagGraph::new(Arc::new(db))
    }

    #[test]
    fn test_init_genesis() {
        let dag = test_dag();
        let hash = dag.init_genesis(NetworkType::Devnet).unwrap();
        assert!(!hash.is_zero());
        assert_eq!(dag.tip_count(), 1);
        assert_eq!(dag.get_tips()[0], hash);
    }

    #[test]
    fn test_insert_and_retrieve() {
        let dag = test_dag();
        let block = Block::genesis(NetworkType::Devnet);
        let hash = block.hash();
        dag.insert_block(&block).unwrap();

        let loaded = dag.get_block(&hash).unwrap().unwrap();
        assert_eq!(block, loaded);

        let header = dag.get_header(&hash).unwrap().unwrap();
        assert_eq!(block.header, header);
    }

    #[test]
    fn test_parent_child_relations() {
        let dag = test_dag();
        let genesis_hash = dag.init_genesis(NetworkType::Devnet).unwrap();

        // Create a child block referencing genesis
        let mut child_header = Header::genesis(NetworkType::Devnet);
        child_header.parents = vec![genesis_hash];
        child_header.nonce = 1; // different nonce to get different hash

        let child_block = Block {
            header: child_header,
            transactions: vec![crate::transaction::Transaction::create_coinbase(
                Amount::from_zents(INITIAL_REWARD_ZENTS),
                Address::from_public_key(&[1u8; 32], NetworkType::Devnet),
                1,
            )],
        };
        let child_hash = child_block.hash();
        dag.insert_block(&child_block).unwrap();

        // Genesis should have the child as a child
        let children = dag.get_children(&genesis_hash).unwrap();
        assert!(children.contains(&child_hash));

        // Child should have genesis as parent
        let parents = dag.get_parents(&child_hash).unwrap();
        assert!(parents.contains(&genesis_hash));

        // Tips should only have the child now
        assert_eq!(dag.tip_count(), 1);
        assert_eq!(dag.get_tips()[0], child_hash);
    }

    #[test]
    fn test_is_ancestor() {
        let dag = test_dag();
        let genesis_hash = dag.init_genesis(NetworkType::Devnet).unwrap();

        let mut child_header = Header::genesis(NetworkType::Devnet);
        child_header.parents = vec![genesis_hash];
        child_header.nonce = 42;
        let child_block = Block {
            header: child_header,
            transactions: vec![crate::transaction::Transaction::create_coinbase(
                Amount::from_zents(INITIAL_REWARD_ZENTS),
                Address::from_public_key(&[1u8; 32], NetworkType::Devnet),
                1,
            )],
        };
        let child_hash = child_block.hash();
        dag.insert_block(&child_block).unwrap();

        assert!(dag.is_ancestor(&genesis_hash, &child_hash));
        assert!(!dag.is_ancestor(&child_hash, &genesis_hash));
        assert!(dag.is_ancestor(&genesis_hash, &genesis_hash)); // self
    }
}
