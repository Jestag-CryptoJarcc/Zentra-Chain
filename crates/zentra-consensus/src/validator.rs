//! Block validation pipeline.

use zentra_types::*;
use zentra_core::header::Header;
use zentra_core::block::Block;
use zentra_core::transaction::TxOutput;
use crate::lanes::verify_block_pow;
use crate::emission::EmissionSchedule;

/// Block validation — verifies structure, PoW, merkle root, and transactions.
pub struct BlockValidator;

impl BlockValidator {
    /// Full block validation.
    pub fn validate_block(
        block: &Block,
        _parent_headers: &[Header],
        emission: &EmissionSchedule,
        height: u64,
    ) -> ZentraResult<()> {
        // 1. Basic structural validation
        block.validate_basic()?;

        // 2. Verify PoW for the correct lane
        verify_block_pow(&block.header)?;

        // 3. Merkle root
        if !block.validate_merkle_root() {
            return Err(ZentraError::InvalidMerkleRoot);
        }

        // 4. Validate coinbase reward
        if let Some(coinbase) = block.transactions.first() {
            if coinbase.is_coinbase() {
                let block_subsidy = emission.block_reward(height);
                let coinbase_amount = coinbase.total_output_amount();

                // Sum transaction fees from all non-coinbase transactions.
                // Fee = sum of all Standard outputs that are explicitly marked as fee burns.
                // For now, fees are implicit: miners include them in their coinbase.
                // We allow coinbase up to subsidy + total_fees_in_block.
                let total_fees: Amount = block.transactions.iter().skip(1).fold(
                    Amount::ZERO,
                    |acc, tx| {
                        // Fees are any burn outputs of type FeeBurn
                        let tx_fees = tx.outputs.iter().fold(Amount::ZERO, |inner_acc, out| {
                            match out {
                                TxOutput::Burn { amount, burn_type }
                                    if matches!(burn_type, BurnType::FeeBurn) =>
                                    inner_acc.saturating_add(*amount),
                                _ => inner_acc,
                            }
                        });
                        acc.saturating_add(tx_fees)
                    },
                );

                let max_coinbase = block_subsidy.saturating_add(total_fees);

                // Coinbase can claim subsidy + fees, but never more
                if coinbase_amount > max_coinbase {
                    return Err(ZentraError::BlockValidation(format!(
                        "coinbase reward {} exceeds subsidy+fees {} at height {}",
                        coinbase_amount, max_coinbase, height
                    )));
                }
            }
        }

        // 5. Validate each transaction
        for (i, tx) in block.transactions.iter().enumerate() {
            if i == 0 && tx.is_coinbase() {
                continue; // coinbase validated above
            }
            Self::validate_transaction(tx)?;
        }

        Ok(())
    }

    /// Validate a single transaction.
    pub fn validate_transaction(tx: &zentra_core::transaction::Transaction) -> ZentraResult<()> {
        tx.validate_basic()?;
        if !tx.is_coinbase() {
            tx.verify_signatures()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_genesis_validates() {
        let block = Block::genesis(NetworkType::Devnet);
        let emission = EmissionSchedule::new(NetworkType::Devnet);
        // Genesis won't pass PoW verification since we don't mine it,
        // but basic validation should work
        assert!(block.validate_basic().is_ok());
    }
}
