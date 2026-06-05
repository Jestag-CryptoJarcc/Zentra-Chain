//! Genesis block generation for each Zentra network.

use zentra_types::*;
use crate::block::Block;

/// Create the genesis block for a given network.
///
/// The genesis block has:
/// - No parents (root of the DAG)
/// - One coinbase transaction with the initial block reward
/// - A deterministic genesis address (derived from all-zeros pubkey)
/// - A fixed genesis timestamp
/// - Lane 0 (CPU) with minimum difficulty
pub fn create_genesis_block(network: NetworkType) -> Block {
    Block::genesis(network)
}

/// Get the genesis block hash for a given network.
pub fn genesis_hash(network: NetworkType) -> Hash {
    create_genesis_block(network).hash()
}

/// Genesis timestamp (2024-06-03 00:00:00 UTC in milliseconds)
pub const GENESIS_TIMESTAMP_MS: u64 = 1_717_372_800_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_genesis_deterministic() {
        let h1 = genesis_hash(NetworkType::Mainnet);
        let h2 = genesis_hash(NetworkType::Mainnet);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_genesis_different_per_network() {
        // Genesis blocks for different networks should be identical
        // (same params), but the address prefixes differ in the coinbase output
        let mainnet = create_genesis_block(NetworkType::Mainnet);
        let devnet = create_genesis_block(NetworkType::Devnet);

        // They'll have different hashes because the coinbase address differs
        assert_ne!(mainnet.hash(), devnet.hash());
    }

    #[test]
    fn test_genesis_validates() {
        let block = create_genesis_block(NetworkType::Devnet);
        // Basic validation should pass (no parent check for genesis)
        assert!(block.validate_basic().is_ok());
        assert!(block.validate_merkle_root());
        assert_eq!(block.transaction_count(), 1);
        assert!(block.transactions[0].is_coinbase());
    }

    #[test]
    fn test_genesis_reward() {
        let block = create_genesis_block(NetworkType::Devnet);
        let coinbase = &block.transactions[0];
        assert_eq!(
            coinbase.total_output_amount(),
            Amount::from_zents(INITIAL_REWARD_ZENTS),
        );
    }
}
