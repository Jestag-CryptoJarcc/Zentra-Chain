//! # Address Utilities
//!
//! Convenience functions for generating, validating, and inspecting Zentra
//! addresses. These build on top of the core [`zentra_types::Address`] type
//! and the wallet's HD key generation.

use zentra_types::address::Address;
use zentra_types::constants::{
    NetworkType, ADDRESS_PREFIX_MAINNET, ADDRESS_PREFIX_TESTNET, ADDRESS_PREFIX_DEVNET,
};

use crate::keygen::{MasterKey, WalletKeypair};

/// Generate a fresh random keypair and its corresponding on-chain address.
///
/// This creates a brand-new 24-word mnemonic, derives account 0 / index 0,
/// and returns the keypair together with the address on the specified network.
///
/// # Example
///
/// ```no_run
/// use zentra_wallet::address::generate_address;
/// use zentra_types::constants::NetworkType;
///
/// let (keypair, address) = generate_address(NetworkType::Testnet);
/// println!("Address: {}", address);
/// ```
pub fn generate_address(network: NetworkType) -> (WalletKeypair, Address) {
    let master = MasterKey::generate();
    let keypair = master.derive_keypair(0, 0);
    let address = keypair.address(network);

    tracing::info!(
        address = %address,
        network = %network,
        "Generated new address"
    );

    (keypair, address)
}

/// Validate a Bech32m-encoded Zentra address string.
///
/// Returns `true` if the string is a well-formed Bech32m address with a
/// recognised Zentra HRP (`zentra`, `zentratest`, or `zentradev`) and a
/// 32-byte payload.
///
/// # Example
///
/// ```no_run
/// use zentra_wallet::address::validate_address;
///
/// assert!(validate_address("zentra1qr3fz5gv0d5s0wz..."));
/// ```
pub fn validate_address(address_str: &str) -> bool {
    Address::from_bech32(address_str).is_ok()
}

/// Determine the [`NetworkType`] from a Bech32m address string.
///
/// Returns `None` if the address is malformed or has an unrecognised prefix.
pub fn get_network_from_address(address_str: &str) -> Option<NetworkType> {
    // Fast path: check the prefix before doing a full decode
    let lower = address_str.to_lowercase();

    // Try to determine prefix from the part before '1' separator
    let sep_pos = lower.rfind('1')?;
    let hrp = &lower[..sep_pos];

    match hrp {
        ADDRESS_PREFIX_MAINNET => {
            // Validate fully before confirming
            Address::from_bech32(address_str).ok()?;
            Some(NetworkType::Mainnet)
        }
        ADDRESS_PREFIX_TESTNET => {
            Address::from_bech32(address_str).ok()?;
            Some(NetworkType::Testnet)
        }
        ADDRESS_PREFIX_DEVNET => {
            Address::from_bech32(address_str).ok()?;
            Some(NetworkType::Devnet)
        }
        _ => None,
    }
}

/// Convert a public key (32-byte Ed25519) to an [`Address`] for the given network.
///
/// This is a thin wrapper around [`Address::from_public_key`].
pub fn address_from_pubkey(pubkey: &[u8; 32], network: NetworkType) -> Address {
    Address::from_public_key(pubkey, network)
}

// ─── Unit Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_address_mainnet() {
        let (kp, addr) = generate_address(NetworkType::Mainnet);
        let bech32 = addr.to_bech32();
        assert!(bech32.starts_with("zentra1"));
        assert!(!addr.is_zero());
        assert_ne!(kp.public_key_bytes(), [0u8; 32]);
    }

    #[test]
    fn test_generate_address_testnet() {
        let (_kp, addr) = generate_address(NetworkType::Testnet);
        let bech32 = addr.to_bech32();
        assert!(bech32.starts_with("zentratest1"));
    }

    #[test]
    fn test_generate_address_devnet() {
        let (_kp, addr) = generate_address(NetworkType::Devnet);
        let bech32 = addr.to_bech32();
        assert!(bech32.starts_with("zentradev1"));
    }

    #[test]
    fn test_validate_address_valid() {
        let (_kp, addr) = generate_address(NetworkType::Mainnet);
        let bech32 = addr.to_bech32();
        assert!(validate_address(&bech32));
    }

    #[test]
    fn test_validate_address_invalid() {
        assert!(!validate_address(""));
        assert!(!validate_address("not_a_valid_address"));
        assert!(!validate_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4")); // Bitcoin address
    }

    #[test]
    fn test_get_network_from_address_mainnet() {
        let (_kp, addr) = generate_address(NetworkType::Mainnet);
        let bech32 = addr.to_bech32();
        assert_eq!(get_network_from_address(&bech32), Some(NetworkType::Mainnet));
    }

    #[test]
    fn test_get_network_from_address_testnet() {
        let (_kp, addr) = generate_address(NetworkType::Testnet);
        let bech32 = addr.to_bech32();
        assert_eq!(get_network_from_address(&bech32), Some(NetworkType::Testnet));
    }

    #[test]
    fn test_get_network_from_address_devnet() {
        let (_kp, addr) = generate_address(NetworkType::Devnet);
        let bech32 = addr.to_bech32();
        assert_eq!(get_network_from_address(&bech32), Some(NetworkType::Devnet));
    }

    #[test]
    fn test_get_network_from_address_invalid() {
        assert_eq!(get_network_from_address("garbage"), None);
        assert_eq!(get_network_from_address(""), None);
    }

    #[test]
    fn test_address_from_pubkey() {
        let pubkey = [42u8; 32];
        let addr = address_from_pubkey(&pubkey, NetworkType::Mainnet);
        assert!(!addr.is_zero());

        // Should match the direct method
        let addr2 = Address::from_public_key(&pubkey, NetworkType::Mainnet);
        assert_eq!(addr, addr2);
    }

    #[test]
    fn test_address_roundtrip() {
        let (_kp, addr) = generate_address(NetworkType::Mainnet);
        let encoded = addr.to_bech32();
        let decoded = Address::from_bech32(&encoded).unwrap();
        assert_eq!(addr, decoded);
    }

    #[test]
    fn test_unique_addresses() {
        let (_kp1, addr1) = generate_address(NetworkType::Mainnet);
        let (_kp2, addr2) = generate_address(NetworkType::Mainnet);
        assert_ne!(addr1, addr2, "Two random addresses should differ");
    }
}
