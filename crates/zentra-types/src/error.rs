//! Global error types for the Zentra network.

use thiserror::Error;

/// Top-level error type for Zentra operations.
#[derive(Debug, Error)]
pub enum ZentraError {
    #[error("Block validation failed: {0}")]
    BlockValidation(String),

    #[error("Transaction validation failed: {0}")]
    TransactionValidation(String),

    #[error("Invalid signature: {0}")]
    InvalidSignature(String),

    #[error("Insufficient funds: required {required}, available {available}")]
    InsufficientFunds { required: u64, available: u64 },

    #[error("Double spend detected: UTXO {0} already spent")]
    DoubleSpend(String),

    #[error("Invalid block parent: {0}")]
    InvalidParent(String),

    #[error("Difficulty target not met")]
    DifficultyNotMet,

    #[error("Invalid mining lane: {0}")]
    InvalidLane(u8),

    #[error("Block size exceeds maximum: {size} > {max}")]
    BlockTooLarge { size: usize, max: usize },

    #[error("Transaction count exceeds maximum: {count} > {max}")]
    TooManyTransactions { count: usize, max: usize },

    #[error("Invalid merkle root")]
    InvalidMerkleRoot,

    #[error("Block timestamp out of range: {0}")]
    InvalidTimestamp(String),

    #[error("Max supply exceeded")]
    MaxSupplyExceeded,

    #[error("AMM error: {0}")]
    AmmError(String),

    #[error("TSS error: {0}")]
    TssError(String),

    #[error("Quarantine error: {0}")]
    QuarantineError(String),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("Wallet error: {0}")]
    Wallet(String),

    #[error("Wasm runtime error: {0}")]
    WasmRuntime(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

/// Result type alias using ZentraError.
pub type ZentraResult<T> = Result<T, ZentraError>;
