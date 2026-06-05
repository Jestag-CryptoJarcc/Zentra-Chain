//! # Zentra Consensus
//!
//! Multi-Algorithm PoW consensus engine with 5 parallel mining lanes,
//! GhostDAG ordering, difficulty adjustment, and emission schedule.

pub mod lanes;
pub mod difficulty;
pub mod ghostdag;
pub mod emission;
pub mod validator;
pub mod miner;
