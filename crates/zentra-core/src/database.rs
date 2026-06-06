//! RocksDB database adapter for the Zentra BlockDAG.

use std::path::Path;
use std::sync::Arc;
use rocksdb::{DB, Options, ColumnFamilyDescriptor};
use borsh::{BorshSerialize, BorshDeserialize};
use zentra_types::*;
use crate::header::Header;
use crate::block::Block;
use crate::transaction::OutPoint;
use crate::utxo::{UtxoEntry, BlockUndoData};

/// Column family names for logical data separation.
const CF_HEADERS: &str = "headers";
const CF_BLOCKS: &str = "blocks";
const CF_TRANSACTIONS: &str = "transactions";
const CF_DAG_CHILDREN: &str = "dag_children";
const CF_GHOSTDAG: &str = "ghostdag";
const CF_UTXO: &str = "utxo";
const CF_BLOCK_INDEX: &str = "block_index";
const CF_METADATA: &str = "metadata";

const ALL_CFS: [&str; 8] = [
    CF_HEADERS, CF_BLOCKS, CF_TRANSACTIONS, CF_DAG_CHILDREN,
    CF_GHOSTDAG, CF_UTXO, CF_BLOCK_INDEX, CF_METADATA,
];

/// RocksDB-backed storage for the Zentra blockchain.
pub struct ZentraDb {
    db: Arc<DB>,
    _temp_dir: Option<tempfile::TempDir>,
}

impl ZentraDb {
    /// Open (or create) a database at the given path.
    pub fn open(path: &Path) -> ZentraResult<Self> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);
        opts.set_max_open_files(256);
        opts.set_keep_log_file_num(3);

        let cfs: Vec<ColumnFamilyDescriptor> = ALL_CFS
            .iter()
            .map(|name| ColumnFamilyDescriptor::new(*name, Options::default()))
            .collect();

        let db = DB::open_cf_descriptors(&opts, path, cfs)
            .map_err(|e| ZentraError::Database(format!("failed to open database: {}", e)))?;

        Ok(ZentraDb {
            db: Arc::new(db),
            _temp_dir: None,
        })
    }

    /// Open a temporary database (for testing).
    pub fn open_temp() -> ZentraResult<Self> {
        let temp_dir = tempfile::tempdir()
            .map_err(|e| ZentraError::Database(format!("failed to create temp dir: {}", e)))?;

        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);

        let cfs: Vec<ColumnFamilyDescriptor> = ALL_CFS
            .iter()
            .map(|name| ColumnFamilyDescriptor::new(*name, Options::default()))
            .collect();

        let db = DB::open_cf_descriptors(&opts, temp_dir.path(), cfs)
            .map_err(|e| ZentraError::Database(format!("failed to open temp database: {}", e)))?;

        Ok(ZentraDb {
            db: Arc::new(db),
            _temp_dir: Some(temp_dir),
        })
    }

    // --- Header operations ---

    pub fn put_header(&self, hash: &Hash, header: &Header) -> ZentraResult<()> {
        let cf = self.db.cf_handle(CF_HEADERS).ok_or_else(|| ZentraError::Database("missing CF".into()))?;
        let value = borsh::to_vec(header).map_err(|e| ZentraError::Serialization(e.to_string()))?;
        self.db.put_cf(&cf, hash.as_bytes(), &value)
            .map_err(|e| ZentraError::Database(e.to_string()))
    }

    pub fn get_header(&self, hash: &Hash) -> ZentraResult<Option<Header>> {
        let cf = self.db.cf_handle(CF_HEADERS).ok_or_else(|| ZentraError::Database("missing CF".into()))?;
        match self.db.get_cf(&cf, hash.as_bytes()) {
            Ok(Some(data)) => {
                let header = Header::try_from_slice(&data)
                    .map_err(|e| ZentraError::Serialization(e.to_string()))?;
                Ok(Some(header))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(ZentraError::Database(e.to_string())),
        }
    }

    // --- Block operations ---

    pub fn put_block(&self, hash: &Hash, block: &Block) -> ZentraResult<()> {
        let cf = self.db.cf_handle(CF_BLOCKS).ok_or_else(|| ZentraError::Database("missing CF".into()))?;
        let value = borsh::to_vec(block).map_err(|e| ZentraError::Serialization(e.to_string()))?;
        self.db.put_cf(&cf, hash.as_bytes(), &value)
            .map_err(|e| ZentraError::Database(e.to_string()))?;

        // Also store the header separately for quick access
        self.put_header(hash, &block.header)?;
        Ok(())
    }

    pub fn get_block(&self, hash: &Hash) -> ZentraResult<Option<Block>> {
        let cf = self.db.cf_handle(CF_BLOCKS).ok_or_else(|| ZentraError::Database("missing CF".into()))?;
        match self.db.get_cf(&cf, hash.as_bytes()) {
            Ok(Some(data)) => {
                let block = Block::try_from_slice(&data)
                    .map_err(|e| ZentraError::Serialization(e.to_string()))?;
                Ok(Some(block))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(ZentraError::Database(e.to_string())),
        }
    }

    // --- UTXO operations ---

    pub fn put_utxo(&self, outpoint: &OutPoint, entry: &UtxoEntry) -> ZentraResult<()> {
        let cf = self.db.cf_handle(CF_UTXO).ok_or_else(|| ZentraError::Database("missing CF".into()))?;
        let key = borsh::to_vec(outpoint).map_err(|e| ZentraError::Serialization(e.to_string()))?;
        let value = borsh::to_vec(entry).map_err(|e| ZentraError::Serialization(e.to_string()))?;
        self.db.put_cf(&cf, &key, &value)
            .map_err(|e| ZentraError::Database(e.to_string()))
    }

    pub fn delete_utxo(&self, outpoint: &OutPoint) -> ZentraResult<()> {
        let cf = self.db.cf_handle(CF_UTXO).ok_or_else(|| ZentraError::Database("missing CF".into()))?;
        let key = borsh::to_vec(outpoint).map_err(|e| ZentraError::Serialization(e.to_string()))?;
        self.db.delete_cf(&cf, &key)
            .map_err(|e| ZentraError::Database(e.to_string()))
    }

    pub fn get_utxo(&self, outpoint: &OutPoint) -> ZentraResult<Option<UtxoEntry>> {
        let cf = self.db.cf_handle(CF_UTXO).ok_or_else(|| ZentraError::Database("missing CF".into()))?;
        let key = borsh::to_vec(outpoint).map_err(|e| ZentraError::Serialization(e.to_string()))?;
        match self.db.get_cf(&cf, &key) {
            Ok(Some(data)) => {
                let entry = UtxoEntry::try_from_slice(&data)
                    .map_err(|e| ZentraError::Serialization(e.to_string()))?;
                Ok(Some(entry))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(ZentraError::Database(e.to_string())),
        }
    }

    // --- DAG child relations ---

    pub fn put_children(&self, parent: &Hash, children: &[Hash]) -> ZentraResult<()> {
        let cf = self.db.cf_handle(CF_DAG_CHILDREN).ok_or_else(|| ZentraError::Database("missing CF".into()))?;
        let value = borsh::to_vec(children).map_err(|e| ZentraError::Serialization(e.to_string()))?;
        self.db.put_cf(&cf, parent.as_bytes(), &value)
            .map_err(|e| ZentraError::Database(e.to_string()))
    }

    pub fn get_children(&self, parent: &Hash) -> ZentraResult<Vec<Hash>> {
        let cf = self.db.cf_handle(CF_DAG_CHILDREN).ok_or_else(|| ZentraError::Database("missing CF".into()))?;
        match self.db.get_cf(&cf, parent.as_bytes()) {
            Ok(Some(data)) => {
                let children = Vec::<Hash>::try_from_slice(&data)
                    .map_err(|e| ZentraError::Serialization(e.to_string()))?;
                Ok(children)
            }
            Ok(None) => Ok(vec![]),
            Err(e) => Err(ZentraError::Database(e.to_string())),
        }
    }

    // --- GhostDAG raw operations ---

    pub fn put_ghostdag_raw(&self, hash: &Hash, value: &[u8]) -> ZentraResult<()> {
        let cf = self.db.cf_handle(CF_GHOSTDAG).ok_or_else(|| ZentraError::Database("missing CF".into()))?;
        self.db.put_cf(&cf, hash.as_bytes(), value)
            .map_err(|e| ZentraError::Database(e.to_string()))
    }

    pub fn get_ghostdag_raw(&self, hash: &Hash) -> ZentraResult<Option<Vec<u8>>> {
        let cf = self.db.cf_handle(CF_GHOSTDAG).ok_or_else(|| ZentraError::Database("missing CF".into()))?;
        self.db.get_cf(&cf, hash.as_bytes())
            .map_err(|e| ZentraError::Database(e.to_string()))
    }

    // --- Undo operations ---

    pub fn put_undo(&self, hash: &Hash, undo: &BlockUndoData) -> ZentraResult<()> {
        let cf = self.db.cf_handle(CF_METADATA).ok_or_else(|| ZentraError::Database("missing CF".into()))?;
        let key = format!("undo:{}", hash.to_hex());
        let value = borsh::to_vec(undo).map_err(|e| ZentraError::Serialization(e.to_string()))?;
        self.db.put_cf(&cf, key.as_bytes(), &value)
            .map_err(|e| ZentraError::Database(e.to_string()))
    }

    pub fn get_undo(&self, hash: &Hash) -> ZentraResult<Option<BlockUndoData>> {
        let cf = self.db.cf_handle(CF_METADATA).ok_or_else(|| ZentraError::Database("missing CF".into()))?;
        let key = format!("undo:{}", hash.to_hex());
        match self.db.get_cf(&cf, key.as_bytes()) {
            Ok(Some(data)) => {
                let undo = BlockUndoData::try_from_slice(&data)
                    .map_err(|e| ZentraError::Serialization(e.to_string()))?;
                Ok(Some(undo))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(ZentraError::Database(e.to_string())),
        }
    }

    // --- Metadata ---

    pub fn put_metadata(&self, key: &str, value: &[u8]) -> ZentraResult<()> {
        let cf = self.db.cf_handle(CF_METADATA).ok_or_else(|| ZentraError::Database("missing CF".into()))?;
        self.db.put_cf(&cf, key.as_bytes(), value)
            .map_err(|e| ZentraError::Database(e.to_string()))
    }

    pub fn get_metadata(&self, key: &str) -> ZentraResult<Option<Vec<u8>>> {
        let cf = self.db.cf_handle(CF_METADATA).ok_or_else(|| ZentraError::Database("missing CF".into()))?;
        self.db.get_cf(&cf, key.as_bytes())
            .map_err(|e| ZentraError::Database(e.to_string()))
    }

    /// Get inner DB reference (for advanced use / testing).
    pub fn inner(&self) -> &Arc<DB> {
        &self.db
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_temp() {
        let db = ZentraDb::open_temp().unwrap();
        assert!(db.get_metadata("test").unwrap().is_none());
    }

    #[test]
    fn test_header_roundtrip() {
        let db = ZentraDb::open_temp().unwrap();
        let header = Header::genesis(NetworkType::Devnet);
        let hash = header.hash();
        db.put_header(&hash, &header).unwrap();
        let loaded = db.get_header(&hash).unwrap().unwrap();
        assert_eq!(header, loaded);
    }

    #[test]
    fn test_block_roundtrip() {
        let db = ZentraDb::open_temp().unwrap();
        let block = Block::genesis(NetworkType::Devnet);
        let hash = block.hash();
        db.put_block(&hash, &block).unwrap();
        let loaded = db.get_block(&hash).unwrap().unwrap();
        assert_eq!(block, loaded);
    }

    #[test]
    fn test_utxo_operations() {
        let db = ZentraDb::open_temp().unwrap();
        let outpoint = OutPoint::new(Hash::hash(b"tx1"), 0);
        let entry = UtxoEntry {
            amount: Amount::from_coins(10),
            address: Address::from_public_key(&[1u8; 32], NetworkType::Devnet),
            block_height: 1,
            is_coinbase: false,
        };

        db.put_utxo(&outpoint, &entry).unwrap();
        assert_eq!(db.get_utxo(&outpoint).unwrap().unwrap(), entry);

        db.delete_utxo(&outpoint).unwrap();
        assert!(db.get_utxo(&outpoint).unwrap().is_none());
    }

    #[test]
    fn test_children_operations() {
        let db = ZentraDb::open_temp().unwrap();
        let parent = Hash::hash(b"parent");
        let children = vec![Hash::hash(b"child1"), Hash::hash(b"child2")];

        db.put_children(&parent, &children).unwrap();
        let loaded = db.get_children(&parent).unwrap();
        assert_eq!(children, loaded);
    }

    #[test]
    fn test_metadata() {
        let db = ZentraDb::open_temp().unwrap();
        db.put_metadata("genesis_hash", b"some_hash").unwrap();
        let data = db.get_metadata("genesis_hash").unwrap().unwrap();
        assert_eq!(data, b"some_hash");
    }
}
