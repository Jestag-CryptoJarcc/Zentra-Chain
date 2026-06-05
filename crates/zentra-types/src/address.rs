//! Bech32m-encoded Zentra addresses.
//!
//! Format: `zentra1qr3fz5gv0d5s0wz...` (mainnet)
//!         `zentratest1qr3fz5gv...`     (testnet)
//!
//! ## Burn Mechanism
//! Zentra uses TRUE burns — tokens sent to burn outputs are permanently
//! removed from the UTXO set and deducted from circulating supply.
//! There is no "dead address" that holds burned tokens; they simply
//! cease to exist on-chain.

use crate::hash::Hash;
use crate::constants::{ADDRESS_PREFIX_MAINNET, ADDRESS_PREFIX_TESTNET, ADDRESS_PREFIX_DEVNET, NetworkType};
use bech32::{Bech32m, Hrp};
use borsh::{BorshSerialize, BorshDeserialize};
use serde::{Serialize, Deserialize};
use std::fmt;

/// A Zentra address — a Bech32m-encoded Blake2b-256 hash of a public key.
///
/// Internally stores the 32-byte public key hash. The Bech32m encoding
/// is only used for display/serialization.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Address {
    /// The 32-byte Blake2b-256 hash of the Ed25519 public key
    pub payload: [u8; 32],
    /// Network type (determines the Bech32m prefix)
    pub network: NetworkType,
}

/// Represents a burn output — tokens destroyed permanently.
/// When a transaction output is marked as a burn, the tokens are:
/// 1. Validated normally (ensuring the sender has the funds)
/// 2. NOT added to the UTXO set (they vanish from the chain)
/// 3. Deducted from total circulating supply tracking
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct BurnOutput {
    /// Amount being permanently destroyed (in zents)
    pub amount: u64,
    /// Reason for the burn (for logging/auditing)
    pub burn_type: BurnType,
}

/// The reason a burn is occurring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum BurnType {
    /// LP tokens burned for permanent protocol-owned liquidity
    LpTokenBurn,
    /// zUSD burned during vault sell operations
    StablecoinBurn,
    /// ZTR burned as part of fee mechanism
    FeeBurn,
    /// Explicit user-initiated burn
    UserBurn,
}

impl Address {
    /// Create a burn output that will permanently destroy tokens.
    /// Unlike a "dead address" pattern, these tokens are truly removed
    /// from the chain — they are validated but never enter the UTXO set.
    pub fn new_burn(amount: u64, burn_type: BurnType) -> BurnOutput {
        BurnOutput { amount, burn_type }
    }

    /// Create an address from a public key (Ed25519).
    /// Hashes the public key with Blake2b-256 to produce the address payload.
    pub fn from_public_key(pubkey: &[u8; 32], network: NetworkType) -> Self {
        let hash = Hash::hash(pubkey);
        Address {
            payload: hash.0,
            network,
        }
    }

    /// Create an address directly from a payload hash.
    pub fn from_payload(payload: [u8; 32], network: NetworkType) -> Self {
        Address { payload, network }
    }

    /// Encode this address as a Bech32m string.
    pub fn to_bech32(&self) -> String {
        let hrp = Hrp::parse(self.network.address_prefix()).expect("valid HRP");
        bech32::encode::<Bech32m>(hrp, &self.payload).expect("valid encoding")
    }

    /// Decode a Bech32m string into an Address.
    pub fn from_bech32(s: &str) -> Result<Self, AddressError> {
        let (hrp, data) = bech32::decode(s)
            .map_err(|e| AddressError::Bech32(e.to_string()))?;

        let hrp_str = hrp.as_str();
        let network = match hrp_str {
            ADDRESS_PREFIX_MAINNET => NetworkType::Mainnet,
            ADDRESS_PREFIX_TESTNET => NetworkType::Testnet,
            ADDRESS_PREFIX_DEVNET => NetworkType::Devnet,
            _ => return Err(AddressError::UnknownPrefix(hrp_str.to_string())),
        };

        if data.len() != 32 {
            return Err(AddressError::InvalidLength(data.len()));
        }

        let mut payload = [0u8; 32];
        payload.copy_from_slice(&data);

        Ok(Address { payload, network })
    }

    /// Check if this is an uninitialized/zero address (should not be used as destination).
    pub fn is_zero(&self) -> bool {
        self.payload == [0u8; 32]
    }

    /// Get the raw payload bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.payload
    }
}

/// Errors that can occur during address operations.
#[derive(Debug, thiserror::Error)]
pub enum AddressError {
    #[error("Bech32 decoding error: {0}")]
    Bech32(String),

    #[error("Unknown address prefix: {0}")]
    UnknownPrefix(String),

    #[error("Invalid payload length: expected 32, got {0}")]
    InvalidLength(usize),
}

impl fmt::Debug for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Address({})", self.to_bech32())
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_bech32())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_address_from_pubkey() {
        let pubkey = [42u8; 32];
        let addr = Address::from_public_key(&pubkey, NetworkType::Mainnet);
        assert!(!addr.is_zero());
        let bech32 = addr.to_bech32();
        assert!(bech32.starts_with("zentra1"));
    }

    #[test]
    fn test_address_roundtrip() {
        let pubkey = [7u8; 32];
        let addr = Address::from_public_key(&pubkey, NetworkType::Mainnet);
        let encoded = addr.to_bech32();
        let decoded = Address::from_bech32(&encoded).unwrap();
        assert_eq!(addr, decoded);
    }

    #[test]
    fn test_burn_output() {
        let burn = Address::new_burn(1_000_000, BurnType::StablecoinBurn);
        assert_eq!(burn.amount, 1_000_000);
        assert_eq!(burn.burn_type, BurnType::StablecoinBurn);
    }

    #[test]
    fn test_zero_address() {
        let addr = Address::from_payload([0u8; 32], NetworkType::Mainnet);
        assert!(addr.is_zero());
    }

    #[test]
    fn test_testnet_prefix() {
        let pubkey = [1u8; 32];
        let addr = Address::from_public_key(&pubkey, NetworkType::Testnet);
        let bech32 = addr.to_bech32();
        assert!(bech32.starts_with("zentratest1"));
    }

    #[test]
    fn test_devnet_prefix() {
        let pubkey = [1u8; 32];
        let addr = Address::from_public_key(&pubkey, NetworkType::Devnet);
        let bech32 = addr.to_bech32();
        assert!(bech32.starts_with("zentradev1"));
    }
}
