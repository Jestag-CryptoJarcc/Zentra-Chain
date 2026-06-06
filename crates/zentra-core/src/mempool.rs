//! Transaction mempool with priority queue ordering by fee rate.

use std::collections::BTreeMap;
use parking_lot::RwLock;
use dashmap::DashMap;
use zentra_types::*;
use crate::transaction::Transaction;

/// Transaction mempool — holds unconfirmed transactions ordered by fee rate.
pub struct Mempool {
    /// Transactions indexed by txid
    transactions: DashMap<Hash, MempoolEntry>,
    /// Priority index keyed by (reversed fee_rate, txid) so transactions that
    /// share a fee rate do NOT collide/evict each other in the map.
    priority_index: RwLock<BTreeMap<(std::cmp::Reverse<u64>, Hash), ()>>,
    /// Maximum number of transactions
    max_size: usize,
}

/// A mempool entry with metadata.
#[derive(Clone, Debug)]
pub struct MempoolEntry {
    pub transaction: Transaction,
    pub fee: Amount,
    pub fee_rate: u64,
    pub added_at: u64, // timestamp
}

impl Mempool {
    /// Create a new mempool with a maximum size.
    pub fn new(max_size: usize) -> Self {
        Mempool {
            transactions: DashMap::new(),
            priority_index: RwLock::new(BTreeMap::new()),
            max_size,
        }
    }

    /// Add a transaction to the mempool.
    pub fn add_transaction(&self, tx: Transaction, fee: Amount) -> ZentraResult<()> {
        let txid = tx.txid();

        if self.transactions.contains_key(&txid) {
            return Err(ZentraError::TransactionValidation(
                "transaction already in mempool".into(),
            ));
        }

        if self.transactions.len() >= self.max_size {
            return Err(ZentraError::TransactionValidation(
                "mempool is full".into(),
            ));
        }

        // Estimate fee rate (fee per byte)
        let tx_size = borsh::to_vec(&tx).map(|v| v.len() as u64).unwrap_or(1);
        let fee_rate = fee.as_zents() / tx_size.max(1);

        let entry = MempoolEntry {
            transaction: tx,
            fee,
            fee_rate,
            added_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        };

        self.transactions.insert(txid, entry);
        self.priority_index.write().insert((std::cmp::Reverse(fee_rate), txid), ());

        tracing::debug!(txid = %txid, fee_rate, "transaction added to mempool");
        Ok(())
    }

    /// Remove a transaction by txid.
    pub fn remove_transaction(&self, txid: &Hash) -> Option<Transaction> {
        if let Some((_, entry)) = self.transactions.remove(txid) {
            self.priority_index.write().retain(|(_, v), _| v != txid);
            Some(entry.transaction)
        } else {
            None
        }
    }

    /// Get the highest-fee transactions for block inclusion.
    pub fn get_transactions_for_block(&self, max_count: usize) -> Vec<Transaction> {
        let priority = self.priority_index.read();
        priority
            .keys()
            .take(max_count)
            .filter_map(|(_, txid)| self.transactions.get(txid).map(|e| e.transaction.clone()))
            .collect()
    }

    /// Get the fee currently associated with a transaction.
    pub fn get_fee(&self, txid: &Hash) -> Option<Amount> {
        self.transactions.get(txid).map(|entry| entry.fee)
    }

    /// Check if a transaction is in the mempool.
    pub fn contains(&self, txid: &Hash) -> bool {
        self.transactions.contains_key(txid)
    }

    /// Get the current mempool size.
    pub fn size(&self) -> usize {
        self.transactions.len()
    }

    /// Remove transactions that have been confirmed in a block.
    pub fn remove_confirmed(&self, txids: &[Hash]) {
        for txid in txids {
            self.remove_transaction(txid);
        }
    }

    /// Evict transactions older than `max_age_ms`. A valid transaction is mined
    /// within a couple of blocks; anything still sitting here long after that is
    /// stuck — typically rejected by miners (e.g. it spends an output that never
    /// confirmed) — and would otherwise clog the mempool forever. Returns how
    /// many were dropped.
    pub fn evict_older_than(&self, max_age_ms: u64) -> usize {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let stale: Vec<Hash> = self.transactions.iter()
            .filter(|e| now.saturating_sub(e.value().added_at) > max_age_ms)
            .map(|e| *e.key())
            .collect();
        for txid in &stale {
            self.remove_transaction(txid);
        }
        if !stale.is_empty() {
            tracing::info!(count = stale.len(), "evicted stuck transactions from mempool");
        }
        stale.len()
    }

    /// Clear all transactions from the mempool.
    pub fn clear(&self) {
        self.transactions.clear();
        self.priority_index.write().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::{Transaction, TxOutput};

    fn make_test_tx(nonce: u64) -> Transaction {
        Transaction {
            version: 1,
            tx_type: TransactionType::Transfer,
            inputs: vec![crate::transaction::TxInput {
                prev_tx_hash: Hash::hash(format!("prev{}", nonce).as_bytes()),
                output_index: 0,
                signature: vec![0; 64],
                public_key: [0; 32],
            }],
            outputs: vec![TxOutput::Standard {
                address: Address::from_public_key(&[1u8; 32], NetworkType::Devnet),
                amount: Amount::from_coins(1),
                script: vec![],
            }],
            payload: vec![],
            lock_time: nonce, // use nonce to make txid unique
        }
    }

    #[test]
    fn test_add_and_contains() {
        let pool = Mempool::new(100);
        let tx = make_test_tx(0);
        let txid = tx.txid();
        pool.add_transaction(tx, Amount::from_zents(1000)).unwrap();
        assert!(pool.contains(&txid));
        assert_eq!(pool.size(), 1);
    }

    #[test]
    fn test_remove() {
        let pool = Mempool::new(100);
        let tx = make_test_tx(0);
        let txid = tx.txid();
        pool.add_transaction(tx, Amount::from_zents(1000)).unwrap();
        let removed = pool.remove_transaction(&txid);
        assert!(removed.is_some());
        assert_eq!(pool.size(), 0);
    }

    #[test]
    fn test_duplicate_rejected() {
        let pool = Mempool::new(100);
        let tx = make_test_tx(0);
        pool.add_transaction(tx.clone(), Amount::from_zents(1000)).unwrap();
        assert!(pool.add_transaction(tx, Amount::from_zents(1000)).is_err());
    }

    #[test]
    fn test_full_mempool() {
        let pool = Mempool::new(2);
        pool.add_transaction(make_test_tx(0), Amount::from_zents(1000)).unwrap();
        pool.add_transaction(make_test_tx(1), Amount::from_zents(2000)).unwrap();
        assert!(pool.add_transaction(make_test_tx(2), Amount::from_zents(500)).is_err());
    }

    #[test]
    fn test_get_for_block() {
        let pool = Mempool::new(100);
        for i in 0..5 {
            pool.add_transaction(make_test_tx(i), Amount::from_zents((i + 1) * 1000)).unwrap();
        }
        let txs = pool.get_transactions_for_block(3);
        assert_eq!(txs.len(), 3);
    }

    #[test]
    fn test_remove_confirmed() {
        let pool = Mempool::new(100);
        let tx1 = make_test_tx(0);
        let tx2 = make_test_tx(1);
        let txid1 = tx1.txid();
        let txid2 = tx2.txid();
        pool.add_transaction(tx1, Amount::from_zents(1000)).unwrap();
        pool.add_transaction(tx2, Amount::from_zents(2000)).unwrap();
        pool.remove_confirmed(&[txid1]);
        assert!(!pool.contains(&txid1));
        assert!(pool.contains(&txid2));
    }
}
