//! # Zentra Types
//!
//! Core types, constants, and primitives for the Zentra L1 BlockDAG network.
//! This crate is the foundation that all other Zentra crates depend on.

pub mod address;
pub mod amount;
pub mod constants;
pub mod hash;
pub mod lane;
pub mod tx_type;
pub mod error;

// Re-export key types at crate root for convenience
pub use address::{Address, BurnOutput, BurnType};
pub use amount::Amount;
pub use constants::*;
pub use hash::Hash;
pub use lane::LaneId;
pub use tx_type::TransactionType;
pub use error::{ZentraError, ZentraResult};
