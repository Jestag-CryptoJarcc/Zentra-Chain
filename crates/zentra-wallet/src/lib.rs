//! # Zentra Wallet
//!
//! HD wallet generation (BIP-39/32/44), Bech32m address encoding,
//! encrypted keystore, and transaction signing for the Zentra network.

pub mod keygen;
pub mod address;
pub mod keystore;
pub mod signer;
