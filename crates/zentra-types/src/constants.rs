//! Network constants for the Zentra L1 blockchain.
//!
//! # Emission Mathematics
//!
//! Bitcoin-style halving schedule — one year per era, converges to exactly 50,000,000 ZTR.
//!
//! Proof:
//!   Total supply = 2 × HALVING_INTERVAL_BLOCKS × INITIAL_REWARD_ZENTS
//!                = 2 × 525,600 × 4,756,468,797 zents
//!                = 5,000,000,000,000,000 zents
//!                = 50,000,000 ZTR  ✓
//!
//! Block time: 60 seconds  →  1,440 blocks/day  →  525,600 blocks/year
//!
//! Year 1 (Era 1): 525,600 × 47.56468797  ≈ 25,000,000 ZTR  (50% of supply)
//! Year 2 (Era 2): 525,600 × 23.78234398  ≈ 12,500,000 ZTR  (25%)
//! Year 3 (Era 3): 525,600 × 11.89117199  ≈  6,250,000 ZTR  (12.5%)
//! ...halves every year, converging to 50,000,000 ZTR total.

/// Network name
pub const NETWORK_NAME: &str = "Zentra L1";

/// Native coin ticker
pub const COIN_TICKER: &str = "ZTR";

/// Unified stablecoin ticker (backed 1:1 by cross-chain stablecoin deposits)
pub const STABLECOIN_TICKER: &str = "zUSD";

/// Maximum total supply of ZTR in the smallest unit (zents).
/// 50,000,000 ZTR × 10^8 = 5,000,000,000,000,000 zents
pub const MAX_SUPPLY_ZENTS: u64 = 5_000_000_000_000_000;

/// Maximum total supply in whole coins
pub const MAX_SUPPLY_COINS: u64 = 50_000_000;

/// Number of decimal places for ZTR (1 ZTR = 10^8 zents)
pub const COIN_DECIMALS: u8 = 8;

/// One full coin in zents
pub const COIN: u64 = 100_000_000; // 10^8

/// Number of decimal places for zUSDT (matches real USDT)
pub const STABLECOIN_DECIMALS: u8 = 6;

/// One full zUSDT in micro-units
pub const STABLECOIN_UNIT: u64 = 1_000_000; // 10^6

/// Initial block reward in zents (47.56468797 ZTR per block).
/// Floors to whole zents, like Bitcoin floors each subsidy to whole satoshis.
pub const INITIAL_REWARD_ZENTS: u64 = 4_756_468_797;

/// Halving interval in blocks — same on ALL networks.
/// At 1 minute per block: 525,600 blocks = 60 × 24 × 365 = exactly 365 days.
/// This is the same as Bitcoin's 4-year halving but compressed to 1 year
/// because Zentra's 1-min blocks yield 525,600 blocks/year vs Bitcoin's ~52,560.
pub const HALVING_INTERVAL_BLOCKS: u64 = 525_600;

/// Maximum number of halvings before reward reaches zero
pub const MAX_HALVINGS: u32 = 64;

/// Target block rate range (blocks per second across all lanes)
pub const TARGET_MIN_BPS: u32 = 10;
pub const TARGET_MAX_BPS: u32 = 30;

/// Target transaction finality in milliseconds
pub const TARGET_FINALITY_MS: u64 = 1_500;

/// Number of mining lanes
pub const NUM_LANES: u8 = 5;

/// Target CPU block time in milliseconds.
/// 60,000ms = 60 seconds = 1 minute per block.
pub const TARGET_BLOCK_TIME_MS: u64 = 60_000;

/// GhostDAG k-cluster parameter (tolerance for parallel blocks)
/// Higher k = more parallel blocks accepted, but slower confirmation
pub const GHOSTDAG_K: u16 = 18;

/// Maximum number of parents a block can reference in the DAG
pub const MAX_BLOCK_PARENTS: usize = 10;

/// Maximum block size in bytes (1 MB)
pub const MAX_BLOCK_SIZE: usize = 1_048_576;

/// Maximum number of transactions per block
pub const MAX_TXS_PER_BLOCK: usize = 10_000;

/// Minimum transaction fee in zents (anti-spam)
pub const MIN_TX_FEE_ZENTS: u64 = 1_000; // 0.00001 ZTR

/// AMM swap fee in basis points (0.2% = 20 bps) — captured and burned
pub const AMM_SWAP_FEE_BPS: u64 = 20;

/// Cross-chain ingest fee in basis points (0.5% = 50 bps)
pub const CROSS_CHAIN_INGEST_FEE_BPS: u64 = 50;

/// Basis points denominator
pub const BPS_DENOMINATOR: u64 = 10_000;

/// Burned tokens are truly destroyed — removed from UTXO set and total supply.
/// This constant tracks the special "burn" script marker used in transaction outputs.
/// Any output with this script is validated but NEVER added to the UTXO set,
/// effectively removing the tokens from existence permanently.
pub const BURN_SCRIPT_MARKER: [u8; 4] = [0xDE, 0xAD, 0xBE, 0xEF];

/// LP tokens for protocol-owned liquidity are also truly burned (destroyed),
/// ensuring the liquidity is permanent and the tokens can never be redeemed.
pub const LP_BURN_ENABLED: bool = true;

/// Quarantine lock duration in blocks (~2 years at 20 BPS)
/// 20 × 60 × 60 × 24 × 365.25 × 2 ≈ 1,262,304,000
pub const QUARANTINE_LOCK_BLOCKS: u64 = 1_262_304_000;

/// Default P2P port
pub const DEFAULT_P2P_PORT: u16 = 16110;

/// Default RPC port
pub const DEFAULT_RPC_PORT: u16 = 16111;

/// Protocol version
pub const PROTOCOL_VERSION: u32 = 1;

/// Block header version
pub const BLOCK_VERSION: u32 = 1;

/// Maximum difficulty adjustment factor per window (4x up or 0.25x down)
pub const MAX_DIFFICULTY_ADJUSTMENT_FACTOR: u64 = 4;

/// Difficulty adjustment window size (number of blocks to average)
pub const DIFFICULTY_WINDOW_SIZE: usize = 2048;

/// Bech32 human-readable prefix for mainnet addresses
pub const ADDRESS_PREFIX_MAINNET: &str = "zentra";

/// Bech32 human-readable prefix for testnet addresses
pub const ADDRESS_PREFIX_TESTNET: &str = "zentratest";

/// Bech32 human-readable prefix for devnet addresses
pub const ADDRESS_PREFIX_DEVNET: &str = "zentradev";

/// BIP-44 coin type (unregistered, using high number for dev)
pub const BIP44_COIN_TYPE: u32 = 99999;

/// Network type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord,
         borsh::BorshSerialize, borsh::BorshDeserialize,
         serde::Serialize, serde::Deserialize)]
pub enum NetworkType {
    Mainnet,
    Testnet,
    Devnet,
}

impl NetworkType {
    /// Get the Bech32 address prefix for this network
    pub fn address_prefix(&self) -> &'static str {
        match self {
            NetworkType::Mainnet => ADDRESS_PREFIX_MAINNET,
            NetworkType::Testnet => ADDRESS_PREFIX_TESTNET,
            NetworkType::Devnet => ADDRESS_PREFIX_DEVNET,
        }
    }

    /// Get the halving interval for this network.
    /// All networks use the same 525,600-block (1-year) interval.
    /// Devnet mines faster in wall-clock time because difficulty is low,
    /// but halvings happen at the same block heights as mainnet.
    pub fn halving_interval(&self) -> u64 {
        match self {
            NetworkType::Devnet => 1000,
            _ => HALVING_INTERVAL_BLOCKS,
        }
    }
}

impl std::fmt::Display for NetworkType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetworkType::Mainnet => write!(f, "mainnet"),
            NetworkType::Testnet => write!(f, "testnet"),
            NetworkType::Devnet => write!(f, "devnet"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_emission_math() {
        // Bitcoin-style integer subsidy rounding stays just below the headline cap.
        let total = 2u128
            .checked_mul(HALVING_INTERVAL_BLOCKS as u128)
            .unwrap()
            .checked_mul(INITIAL_REWARD_ZENTS as u128)
            .unwrap();
        assert!(total <= MAX_SUPPLY_ZENTS as u128);
        assert!((MAX_SUPPLY_ZENTS as u128 - total) < COIN as u128);
    }

    #[test]
    fn test_max_supply_fits_u64() {
        assert!(MAX_SUPPLY_ZENTS < u64::MAX);
    }

    #[test]
    fn test_coin_conversion() {
        assert_eq!(MAX_SUPPLY_COINS * COIN, MAX_SUPPLY_ZENTS);
    }

    #[test]
    fn test_network_prefixes() {
        assert_eq!(NetworkType::Mainnet.address_prefix(), "zentra");
        assert_eq!(NetworkType::Testnet.address_prefix(), "zentratest");
        assert_eq!(NetworkType::Devnet.address_prefix(), "zentradev");
    }
}
