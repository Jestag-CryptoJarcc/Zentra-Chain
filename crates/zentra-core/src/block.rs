//! Block structure combining header and transactions.

use borsh::{BorshSerialize, BorshDeserialize};
use serde::{Serialize, Deserialize};
use zentra_types::*;
use crate::header::Header;
use crate::transaction::Transaction;
use crate::merkle::compute_merkle_root;

/// A complete Zentra block containing a header and list of transactions.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Block {
    pub header: Header,
    pub transactions: Vec<Transaction>,
}

impl Block {
    /// Get the block hash (delegates to header hash).
    pub fn hash(&self) -> Hash {
        self.header.hash()
    }

    /// Compute the merkle root from the block's transactions.
    pub fn compute_merkle_root(&self) -> Hash {
        let tx_hashes: Vec<Hash> = self.transactions.iter().map(|tx| tx.txid()).collect();
        compute_merkle_root(&tx_hashes)
    }

    /// Check if the header's merkle root matches the computed one.
    pub fn validate_merkle_root(&self) -> bool {
        self.header.merkle_root == self.compute_merkle_root()
    }

    /// Basic structural validation.
    pub fn validate_basic(&self) -> ZentraResult<()> {
        self.header.validate_basic()?;

        if self.transactions.len() > MAX_TXS_PER_BLOCK {
            return Err(ZentraError::TooManyTransactions {
                count: self.transactions.len(),
                max: MAX_TXS_PER_BLOCK,
            });
        }

        let size = self.size_bytes();
        if size > MAX_BLOCK_SIZE {
            return Err(ZentraError::BlockTooLarge {
                size,
                max: MAX_BLOCK_SIZE,
            });
        }

        if !self.validate_merkle_root() {
            return Err(ZentraError::InvalidMerkleRoot);
        }

        // Validate each transaction
        for tx in &self.transactions {
            tx.validate_basic()?;
        }

        // First transaction should be coinbase (if block has transactions)
        if !self.transactions.is_empty() && !self.transactions[0].is_coinbase() {
            return Err(ZentraError::BlockValidation(
                "first transaction must be coinbase".into(),
            ));
        }

        // Only one coinbase allowed
        let coinbase_count = self.transactions.iter().filter(|tx| tx.is_coinbase()).count();
        if coinbase_count > 1 {
            return Err(ZentraError::BlockValidation(
                format!("only one coinbase transaction allowed, found {}", coinbase_count),
            ));
        }

        Ok(())
    }

    /// Number of transactions in this block.
    pub fn transaction_count(&self) -> usize {
        self.transactions.len()
    }

    /// Approximate serialized size in bytes.
    pub fn size_bytes(&self) -> usize {
        borsh::to_vec(self).map(|v| v.len()).unwrap_or(0)
    }

    /// Create the genesis block for a given network.
    pub fn genesis(network: NetworkType) -> Self {
        let miner_address = Address::from_public_key(&[0u8; 32], network);
        let coinbase = Transaction::create_coinbase(
            Amount::from_zents(INITIAL_REWARD_ZENTS),
            miner_address,
            0,
        );

        let merkle_root = compute_merkle_root(&[coinbase.txid()]);
        let mut header = Header::genesis(network);
        header.merkle_root = merkle_root;

        Block {
            header,
            transactions: vec![coinbase],
        }
    }

    /// Total fees available from non-coinbase transactions (output amounts are not known without UTXO context,
    /// but we can sum all burn amounts which represent fees/burns).
    pub fn total_burn_amount(&self) -> Amount {
        self.transactions
            .iter()
            .fold(Amount::ZERO, |acc, tx| acc.saturating_add(tx.total_burn_amount()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_genesis_block() {
        let block = Block::genesis(NetworkType::Devnet);
        assert_eq!(block.transaction_count(), 1);
        assert!(block.transactions[0].is_coinbase());
        assert!(block.validate_merkle_root());
        assert!(block.validate_basic().is_ok());
    }

    #[test]
    fn test_genesis_hash_deterministic() {
        let b1 = Block::genesis(NetworkType::Mainnet);
        let b2 = Block::genesis(NetworkType::Mainnet);
        assert_eq!(b1.hash(), b2.hash());
    }

    #[test]
    fn test_size_bytes() {
        let block = Block::genesis(NetworkType::Devnet);
        assert!(block.size_bytes() > 0);
        assert!(block.size_bytes() < MAX_BLOCK_SIZE);
    }
}
