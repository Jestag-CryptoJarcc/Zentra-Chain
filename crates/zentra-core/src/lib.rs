//! # Zentra Core
//!
//! Core BlockDAG primitives, data structures, and database layer for the Zentra L1 network.
//! Provides the structural foundation including blocks, transactions, the DAG graph,
//! UTXO management, and the mempool.

pub mod header;
pub mod transaction;
pub mod block;
pub mod merkle;
pub mod dag;
pub mod database;
pub mod utxo;
pub mod mempool;
pub mod genesis;
