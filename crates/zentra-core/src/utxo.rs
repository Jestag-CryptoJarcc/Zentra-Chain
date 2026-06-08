//! UTXO (Unspent Transaction Output) set management.
//!
//! CRITICAL: Burn outputs are NEVER added to the UTXO set.
//! When a transaction has a `TxOutput::Burn` variant, the tokens
//! are validated (sender has funds) but permanently destroyed —
//! they simply don't enter the UTXO set.

use borsh::{BorshSerialize, BorshDeserialize};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use zentra_types::*;
use crate::transaction::{Transaction, TxOutput, OutPoint};
use crate::block::Block;

/// Blocks a coinbase output must age before it can be spent (Bitcoin-style).
/// Mirrors the value enforced in block validation and mining selection.
pub const COINBASE_MATURITY: u64 = 10;

/// A single unspent transaction output entry.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct UtxoEntry {
    pub amount: Amount,
    pub address: Address,
    pub block_height: u64,
    pub is_coinbase: bool,
}

/// Undo data for a transaction's spent inputs.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct TxUndoData {
    pub inputs_undo: Vec<UtxoEntry>,
}

/// Undo data for all spent inputs in a block's transactions.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct BlockUndoData {
    pub txs_undo: Vec<TxUndoData>,
    /// Outpoints created by the coinbases of blue MERGE blocks that were paid
    /// alongside this selected-chain block (DAG reward fairness). Tracked so a
    /// reorg that disconnects this block also removes those merge rewards.
    #[serde(default)]
    pub merge_coinbases: Vec<OutPoint>,
}

/// In-memory UTXO set for fast lookups.
/// In production, this would be backed by RocksDB via ZentraDb.
pub struct UtxoSet {
    utxos: HashMap<OutPoint, UtxoEntry>,
    /// Track total burned amounts for metrics
    total_burned: Amount,
}

impl UtxoSet {
    /// Create an empty UTXO set.
    pub fn new() -> Self {
        UtxoSet {
            utxos: HashMap::new(),
            total_burned: Amount::ZERO,
        }
    }

    /// Apply a block to the UTXO set.
    ///
    /// - Removes spent UTXOs (inputs)
    /// - Adds new UTXOs from Standard outputs ONLY
    /// - Burn outputs are validated but NOT added (tokens destroyed)
    pub fn apply_block(&mut self, block: &Block, height: u64) -> ZentraResult<BlockUndoData> {
        let mut txs_undo = Vec::new();
        for tx in &block.transactions {
            let tx_undo = self.apply_transaction(tx, height)?;
            txs_undo.push(tx_undo);
        }
        Ok(BlockUndoData { txs_undo, merge_coinbases: Vec::new() })
    }

    /// Credit ONLY the coinbase reward of a blue MERGE block — a valid block that
    /// solved the PoW but lost the selected-tip race. In a BlockDAG every block
    /// that does real work should be paid (unlike Bitcoin, which orphans the
    /// loser). We apply just the coinbase outputs (not the merge block's regular
    /// transactions, which are settled on the selected chain) so the miner who
    /// found it gets their reward. Returns the outpoints created, for undo.
    /// No-op (returns empty) if the coinbase was already credited.
    pub fn apply_merge_coinbase(&mut self, block: &Block, height: u64) -> Vec<OutPoint> {
        let mut created = Vec::new();
        let cb = match block.transactions.first() {
            Some(tx) if tx.is_coinbase() => tx,
            _ => return created,
        };
        let txid = cb.txid();
        for (idx, output) in cb.outputs.iter().enumerate() {
            if let TxOutput::Standard { address, amount, .. } = output {
                let outpoint = OutPoint::new(txid, idx as u32);
                if self.utxos.contains_key(&outpoint) { continue; } // already credited
                self.utxos.insert(outpoint.clone(), UtxoEntry {
                    amount: *amount,
                    address: address.clone(),
                    block_height: height,
                    is_coinbase: true,
                });
                created.push(outpoint);
            }
        }
        created
    }

    /// Apply a single transaction to the UTXO set.
    fn apply_transaction(&mut self, tx: &Transaction, height: u64) -> ZentraResult<TxUndoData> {
        let txid = tx.txid();
        let mut inputs_undo = Vec::new();

        // Remove spent UTXOs (skip for coinbase — has no inputs)
        if !tx.is_coinbase() {
            for input in &tx.inputs {
                let outpoint = OutPoint::new(input.prev_tx_hash, input.output_index);
                let entry = self.utxos.remove(&outpoint).ok_or_else(|| {
                    ZentraError::DoubleSpend(format!("{}:{}", outpoint.tx_hash, outpoint.index))
                })?;
                inputs_undo.push(entry);
            }
        }

        // Add new UTXOs — ONLY from Standard outputs, NOT from Burns
        for (idx, output) in tx.outputs.iter().enumerate() {
            match output {
                TxOutput::Standard { address, amount, .. } => {
                    let outpoint = OutPoint::new(txid, idx as u32);
                    let entry = UtxoEntry {
                        amount: *amount,
                        address: address.clone(),
                        block_height: height,
                        is_coinbase: tx.is_coinbase(),
                    };
                    self.utxos.insert(outpoint, entry);
                }
                TxOutput::Burn { amount, burn_type } => {
                    // TRUE BURN: tokens are permanently destroyed.
                    // They are NOT added to the UTXO set.
                    // They simply cease to exist on-chain.
                    tracing::info!(
                        amount = %amount,
                        burn_type = ?burn_type,
                        tx = %txid,
                        "tokens permanently burned — removed from chain"
                    );
                    self.total_burned = self.total_burned.saturating_add(*amount);
                }
            }
        }

        Ok(TxUndoData { inputs_undo })
    }

    /// Rollback/disconnect a block from the UTXO set using its undo data.
    pub fn disconnect_block(&mut self, block: &Block, undo: &BlockUndoData) -> ZentraResult<()> {
        if block.transactions.len() != undo.txs_undo.len() {
            return Err(ZentraError::Database(format!(
                "block transactions count {} does not match undo data count {}",
                block.transactions.len(), undo.txs_undo.len()
            )));
        }

        // Process transactions in reverse order to undo updates correctly
        for (tx, tx_undo) in block.transactions.iter().zip(&undo.txs_undo).rev() {
            let txid = tx.txid();

            // 1. Remove outputs created by this transaction
            for (idx, output) in tx.outputs.iter().enumerate() {
                if let TxOutput::Standard { .. } = output {
                    let outpoint = OutPoint::new(txid, idx as u32);
                    self.utxos.remove(&outpoint);
                } else if let TxOutput::Burn { amount, .. } = output {
                    // Reverse the burn statistic tracking
                    self.total_burned = self.total_burned.saturating_sub(*amount);
                }
            }

            // 2. Restore inputs spent by this transaction
            for (input, entry) in tx.inputs.iter().zip(&tx_undo.inputs_undo) {
                let outpoint = OutPoint::new(input.prev_tx_hash, input.output_index);
                self.utxos.insert(outpoint, entry.clone());
            }
        }

        // 3. Remove any blue merge-block coinbase rewards that were credited
        //    alongside this block.
        for outpoint in &undo.merge_coinbases {
            self.utxos.remove(outpoint);
        }

        Ok(())
    }

    /// Get the balance for an address (sum of ALL UTXOs, including immature
    /// coinbase). Prefer `get_spendable_balance` for anything user-facing.
    pub fn get_balance(&self, address: &Address) -> Amount {
        self.utxos
            .values()
            .filter(|entry| entry.address == *address)
            .fold(Amount::ZERO, |acc, entry| acc.saturating_add(entry.amount))
    }

    /// Spendable balance at `current_height`: excludes coinbase outputs that
    /// haven't reached COINBASE_MATURITY. This is what a wallet should show and
    /// spend — counting immature coinbase lets you build a transaction that
    /// every miner rejects (the coins aren't spendable yet), which then sits
    /// stuck in the mempool. Excluding it at the source prevents that.
    pub fn get_spendable_balance(&self, address: &Address, current_height: u64) -> Amount {
        self.utxos
            .values()
            .filter(|e| e.address == *address)
            .filter(|e| !e.is_coinbase || current_height >= e.block_height.saturating_add(COINBASE_MATURITY))
            .fold(Amount::ZERO, |acc, e| acc.saturating_add(e.amount))
    }

    /// Get all UTXOs belonging to an address (including immature coinbase).
    pub fn get_utxos_for_address(&self, address: &Address) -> Vec<(OutPoint, UtxoEntry)> {
        self.utxos
            .iter()
            .filter(|(_, entry)| entry.address == *address)
            .map(|(op, entry)| (op.clone(), entry.clone()))
            .collect()
    }

    /// Only the UTXOs that can actually be spent at `current_height` — mature
    /// coinbase + all regular outputs. Transaction building must use this so it
    /// never selects an immature coinbase as an input.
    pub fn get_spendable_utxos_for_address(&self, address: &Address, current_height: u64) -> Vec<(OutPoint, UtxoEntry)> {
        self.utxos
            .iter()
            .filter(|(_, e)| e.address == *address)
            .filter(|(_, e)| !e.is_coinbase || current_height >= e.block_height.saturating_add(COINBASE_MATURITY))
            .map(|(op, e)| (op.clone(), e.clone()))
            .collect()
    }

    /// Check if a UTXO exists.
    pub fn has_utxo(&self, outpoint: &OutPoint) -> bool {
        self.utxos.contains_key(outpoint)
    }

    /// Get a specific UTXO entry.
    pub fn get_utxo(&self, outpoint: &OutPoint) -> Option<&UtxoEntry> {
        self.utxos.get(outpoint)
    }

    /// Total number of UTXOs in the set.
    pub fn size(&self) -> usize {
        self.utxos.len()
    }

    /// Total amount of tokens that have been permanently burned.
    pub fn total_burned(&self) -> Amount {
        self.total_burned
    }
}

impl Default for UtxoSet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;

    fn test_addr() -> Address {
        Address::from_public_key(&[1u8; 32], NetworkType::Devnet)
    }

    fn test_addr2() -> Address {
        Address::from_public_key(&[2u8; 32], NetworkType::Devnet)
    }

    #[test]
    fn test_apply_genesis_block() {
        let mut utxo_set = UtxoSet::new();
        let block = Block::genesis(NetworkType::Devnet);
        utxo_set.apply_block(&block, 0).unwrap();

        // Genesis coinbase creates one UTXO
        assert_eq!(utxo_set.size(), 1);
    }

    #[test]
    fn test_burn_output_not_in_utxo_set() {
        let mut utxo_set = UtxoSet::new();

        let tx = Transaction {
            version: 1,
            tx_type: TransactionType::Coinbase,
            inputs: vec![],
            outputs: vec![
                TxOutput::Standard {
                    address: test_addr(),
                    amount: Amount::from_coins(10),
                    script: vec![],
                },
                TxOutput::Burn {
                    amount: Amount::from_coins(5),
                    burn_type: BurnType::StablecoinBurn,
                },
            ],
            payload: vec![],
            lock_time: 0,
        };

        let block = Block {
            header: crate::header::Header::genesis(NetworkType::Devnet),
            transactions: vec![tx],
        };

        utxo_set.apply_block(&block, 0).unwrap();

        // Only the Standard output should be in the UTXO set (1 entry)
        // The Burn output is destroyed — not in UTXO set
        assert_eq!(utxo_set.size(), 1);
        assert_eq!(utxo_set.get_balance(&test_addr()), Amount::from_coins(10));
        assert_eq!(utxo_set.total_burned(), Amount::from_coins(5));
    }

    #[test]
    fn test_balance_tracking() {
        let mut utxo_set = UtxoSet::new();

        let tx = Transaction {
            version: 1,
            tx_type: TransactionType::Coinbase,
            inputs: vec![],
            outputs: vec![
                TxOutput::Standard {
                    address: test_addr(),
                    amount: Amount::from_coins(50),
                    script: vec![],
                },
                TxOutput::Standard {
                    address: test_addr2(),
                    amount: Amount::from_coins(30),
                    script: vec![],
                },
            ],
            payload: vec![],
            lock_time: 0,
        };

        let block = Block {
            header: crate::header::Header::genesis(NetworkType::Devnet),
            transactions: vec![tx],
        };

        utxo_set.apply_block(&block, 0).unwrap();

        assert_eq!(utxo_set.get_balance(&test_addr()), Amount::from_coins(50));
        assert_eq!(utxo_set.get_balance(&test_addr2()), Amount::from_coins(30));
        assert_eq!(utxo_set.size(), 2);
    }

    #[test]
    fn test_coinbase_rewards_accumulate_for_same_miner() {
        let mut utxo_set = UtxoSet::new();
        let address = test_addr();
        let reward = Amount::from_zents(INITIAL_REWARD_ZENTS);

        let tx1 = Transaction::create_coinbase(reward, address.clone(), 1);
        let tx2 = Transaction::create_coinbase(reward, address.clone(), 2);
        assert_ne!(tx1.txid(), tx2.txid());

        let block1 = Block {
            header: crate::header::Header::genesis(NetworkType::Devnet),
            transactions: vec![tx1],
        };
        let block2 = Block {
            header: crate::header::Header::genesis(NetworkType::Devnet),
            transactions: vec![tx2],
        };

        utxo_set.apply_block(&block1, 1).unwrap();
        utxo_set.apply_block(&block2, 2).unwrap();

        assert_eq!(utxo_set.size(), 2);
        assert_eq!(
            utxo_set.get_balance(&address),
            Amount::from_zents(INITIAL_REWARD_ZENTS * 2)
        );
    }

    #[test]
    fn test_double_spend_detection() {
        let mut utxo_set = UtxoSet::new();

        // Spend a UTXO that doesn't exist
        let tx = Transaction {
            version: 1,
            tx_type: TransactionType::Transfer,
            inputs: vec![crate::transaction::TxInput {
                prev_tx_hash: Hash::hash(b"nonexistent"),
                output_index: 0,
                signature: vec![0; 64],
                public_key: [0; 32],
            }],
            outputs: vec![TxOutput::Standard {
                address: test_addr(),
                amount: Amount::from_coins(1),
                script: vec![],
            }],
            payload: vec![],
            lock_time: 0,
        };

        let block = Block {
            header: crate::header::Header::genesis(NetworkType::Devnet),
            transactions: vec![
                Transaction::create_coinbase(Amount::from_coins(1), test_addr(), 0),
                tx,
            ],
        };

        assert!(utxo_set.apply_block(&block, 0).is_err());
    }
}
