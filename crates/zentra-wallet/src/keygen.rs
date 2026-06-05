//! # HD Wallet Key Generation
//!
//! Implements BIP-39 mnemonic generation and a simplified BIP-32 key derivation
//! scheme compatible with Ed25519 using HMAC-SHA512 key chaining.
//!
//! ## Derivation Path
//!
//! Keys are derived using the path `m / purpose' / coin_type' / account' / index'`
//! where all levels use hardened derivation (Ed25519 requirement).
//!
//! The HMAC chain is: `seed → "ed25519 zentra seed" → account → index`

use bip39::{Language, Mnemonic};
use ed25519_dalek::{SigningKey, VerifyingKey};
use hmac::{Hmac, Mac};
use sha2::Sha512;
use zeroize::Zeroize;

use zentra_types::address::Address;
use zentra_types::constants::NetworkType;
use zentra_types::error::{ZentraError, ZentraResult};
use zentra_types::Hash;

/// HMAC domain separator for the Zentra key derivation chain.
const ZENTRA_SEED_DOMAIN: &[u8] = b"ed25519 zentra seed";

/// Type alias for HMAC-SHA512.
type HmacSha512 = Hmac<Sha512>;

/// An Ed25519 keypair derived from an HD wallet hierarchy.
///
/// Each keypair is uniquely identified by its derivation path (account + index)
/// and can generate a corresponding on-chain [`Address`].
pub struct WalletKeypair {
    /// Ed25519 signing (private) key.
    signing_key: SigningKey,
    /// Ed25519 verification (public) key, derived from `signing_key`.
    verifying_key: VerifyingKey,
    /// Human-readable derivation path, e.g. `"m/44'/99999'/0'/0"`.
    pub derivation_path: String,
}

impl WalletKeypair {
    /// Generate the on-chain [`Address`] for this keypair on the given network.
    ///
    /// The address payload is the Blake2b-256 hash of the raw Ed25519 public key bytes.
    pub fn address(&self, network: NetworkType) -> Address {
        let pubkey_bytes = self.public_key_bytes();
        Address::from_public_key(&pubkey_bytes, network)
    }

    /// Return the raw 32-byte Ed25519 public key.
    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.verifying_key.to_bytes()
    }

    /// Return a reference to the [`SigningKey`].
    pub fn signing_key(&self) -> &SigningKey {
        &self.signing_key
    }

    /// Return a reference to the [`VerifyingKey`].
    pub fn verifying_key(&self) -> &VerifyingKey {
        &self.verifying_key
    }
}

impl std::fmt::Debug for WalletKeypair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WalletKeypair")
            .field("derivation_path", &self.derivation_path)
            .field("public_key", &hex::encode(self.public_key_bytes()))
            .finish()
    }
}

/// The master key derived from a BIP-39 mnemonic.
///
/// Holds the 64-byte seed and the mnemonic phrase. Both are sensitive
/// material and are zeroized on drop.
#[derive(Zeroize)]
#[zeroize(drop)]
pub struct MasterKey {
    /// The 64-byte seed derived from the mnemonic + empty passphrase.
    seed: [u8; 64],
    /// The original mnemonic phrase (space-separated words).
    mnemonic_phrase: String,
}

impl MasterKey {
    /// Generate a fresh 24-word BIP-39 mnemonic and derive the master seed.
    ///
    /// Uses the OS CSPRNG via `rand::thread_rng()` for entropy generation.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use zentra_wallet::keygen::MasterKey;
    /// let master = MasterKey::generate();
    /// println!("Mnemonic: {}", master.mnemonic_phrase());
    /// ```
    pub fn generate() -> Self {
        // Generate 256 bits of entropy for a 24-word mnemonic
        let mut rng = rand::thread_rng();
        let mnemonic = Mnemonic::generate_in_with(&mut rng, Language::English, 24)
            .expect("valid word count");

        tracing::info!("Generated new 24-word BIP-39 mnemonic");

        let seed = Self::mnemonic_to_seed(&mnemonic);

        MasterKey {
            seed,
            mnemonic_phrase: mnemonic.to_string(),
        }
    }

    /// Restore a [`MasterKey`] from an existing BIP-39 mnemonic phrase.
    ///
    /// # Errors
    ///
    /// Returns [`ZentraError::Wallet`] if the phrase is invalid (wrong word count,
    /// unknown words, or bad checksum).
    pub fn from_mnemonic(phrase: &str) -> ZentraResult<Self> {
        let mnemonic = Mnemonic::parse_in(Language::English, phrase)
            .map_err(|e| ZentraError::Wallet(format!("Invalid mnemonic: {}", e)))?;

        tracing::info!("Restored master key from mnemonic");

        let seed = Self::mnemonic_to_seed(&mnemonic);

        Ok(MasterKey {
            seed,
            mnemonic_phrase: mnemonic.to_string(),
        })
    }

    /// Derive a child [`WalletKeypair`] for the given `account` and `index`.
    ///
    /// The derivation chain is:
    /// 1. HMAC-SHA512(key=`"ed25519 zentra seed"`, data=`seed`) → master chain key
    /// 2. HMAC-SHA512(key=`master_chain_code`, data=`master_secret ‖ account_bytes`) → account key
    /// 3. HMAC-SHA512(key=`account_chain_code`, data=`account_secret ‖ index_bytes`) → child key
    /// 4. First 32 bytes of child output → Ed25519 signing key
    ///
    /// All derivation is hardened (required for Ed25519).
    pub fn derive_keypair(&self, account: u32, index: u32) -> WalletKeypair {
        // Step 1: Master chain key from seed
        let mut mac = HmacSha512::new_from_slice(ZENTRA_SEED_DOMAIN)
            .expect("HMAC accepts any key length");
        mac.update(&self.seed);
        let master_output = mac.finalize().into_bytes();

        let (master_secret, master_chain_code) = master_output.split_at(32);

        // Step 2: Account-level derivation
        let mut mac = HmacSha512::new_from_slice(master_chain_code)
            .expect("HMAC accepts any key length");
        mac.update(master_secret);
        mac.update(&account.to_be_bytes());
        let account_output = mac.finalize().into_bytes();

        let (account_secret, account_chain_code) = account_output.split_at(32);

        // Step 3: Index-level derivation
        let mut mac = HmacSha512::new_from_slice(account_chain_code)
            .expect("HMAC accepts any key length");
        mac.update(account_secret);
        mac.update(&index.to_be_bytes());
        let child_output = mac.finalize().into_bytes();

        // Step 4: Build the Ed25519 keypair from the first 32 bytes
        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(&child_output[..32]);

        let signing_key = SigningKey::from_bytes(&key_bytes);
        let verifying_key = signing_key.verifying_key();

        // Zeroize intermediate material
        key_bytes.zeroize();

        let path = format!("m/44'/99999'/{}'/{}", account, index);

        tracing::debug!(
            path = %path,
            pubkey = %hex::encode(verifying_key.to_bytes()),
            "Derived child keypair"
        );

        WalletKeypair {
            signing_key,
            verifying_key,
            derivation_path: path,
        }
    }

    /// Return the mnemonic phrase (for backup display).
    pub fn mnemonic_phrase(&self) -> &str {
        &self.mnemonic_phrase
    }

    /// Return a reference to the raw 64-byte seed.
    pub fn seed(&self) -> &[u8; 64] {
        &self.seed
    }

    /// Derive a 64-byte seed from a BIP-39 mnemonic using an empty passphrase.
    fn mnemonic_to_seed(mnemonic: &Mnemonic) -> [u8; 64] {
        // BIP-39 specifies PBKDF2-HMAC-SHA512 with the mnemonic as password
        // and "mnemonic" + passphrase as salt. The bip39 crate provides this.
        // We use an empty passphrase to keep things simple.
        let seed_bytes = mnemonic.to_seed("");
        let mut seed = [0u8; 64];
        seed.copy_from_slice(&seed_bytes);
        seed
    }
}

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MasterKey")
            .field("seed", &"[REDACTED]")
            .field("mnemonic_phrase", &"[REDACTED]")
            .finish()
    }
}

// ─── Unit Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_master_key() {
        let master = MasterKey::generate();
        // BIP-39 24-word mnemonic has exactly 24 words
        let word_count = master.mnemonic_phrase().split_whitespace().count();
        assert_eq!(word_count, 24, "Expected 24-word mnemonic");
        // Seed should be 64 bytes, non-zero
        assert_ne!(master.seed(), &[0u8; 64]);
    }

    #[test]
    fn test_from_mnemonic_roundtrip() {
        let master1 = MasterKey::generate();
        let phrase = master1.mnemonic_phrase().to_string();

        let master2 = MasterKey::from_mnemonic(&phrase).expect("valid mnemonic");
        assert_eq!(master1.seed(), master2.seed(), "Seeds must match for same mnemonic");
    }

    #[test]
    fn test_from_mnemonic_invalid() {
        let result = MasterKey::from_mnemonic("invalid mnemonic phrase that should fail");
        assert!(result.is_err());
    }

    #[test]
    fn test_derive_keypair_deterministic() {
        let master = MasterKey::generate();
        let kp1 = master.derive_keypair(0, 0);
        let kp2 = master.derive_keypair(0, 0);

        assert_eq!(
            kp1.public_key_bytes(),
            kp2.public_key_bytes(),
            "Same derivation path must produce same key"
        );
    }

    #[test]
    fn test_derive_different_accounts() {
        let master = MasterKey::generate();
        let kp0 = master.derive_keypair(0, 0);
        let kp1 = master.derive_keypair(1, 0);

        assert_ne!(
            kp0.public_key_bytes(),
            kp1.public_key_bytes(),
            "Different accounts must produce different keys"
        );
    }

    #[test]
    fn test_derive_different_indices() {
        let master = MasterKey::generate();
        let kp0 = master.derive_keypair(0, 0);
        let kp1 = master.derive_keypair(0, 1);

        assert_ne!(
            kp0.public_key_bytes(),
            kp1.public_key_bytes(),
            "Different indices must produce different keys"
        );
    }

    #[test]
    fn test_derivation_path_format() {
        let master = MasterKey::generate();
        let kp = master.derive_keypair(3, 7);
        assert_eq!(kp.derivation_path, "m/44'/99999'/3'/7");
    }

    #[test]
    fn test_address_generation() {
        let master = MasterKey::generate();
        let kp = master.derive_keypair(0, 0);

        let addr = kp.address(NetworkType::Mainnet);
        let bech32 = addr.to_bech32();
        assert!(bech32.starts_with("zentra1"), "Mainnet address must start with zentra1");

        let addr_test = kp.address(NetworkType::Testnet);
        let bech32_test = addr_test.to_bech32();
        assert!(
            bech32_test.starts_with("zentratest1"),
            "Testnet address must start with zentratest1"
        );
    }

    #[test]
    fn test_public_key_is_32_bytes() {
        let master = MasterKey::generate();
        let kp = master.derive_keypair(0, 0);
        let pk = kp.public_key_bytes();
        assert_eq!(pk.len(), 32);
    }

    #[test]
    fn test_different_mnemonics_different_keys() {
        let m1 = MasterKey::generate();
        let m2 = MasterKey::generate();

        let kp1 = m1.derive_keypair(0, 0);
        let kp2 = m2.derive_keypair(0, 0);

        assert_ne!(
            kp1.public_key_bytes(),
            kp2.public_key_bytes(),
            "Different mnemonics must produce different keys"
        );
    }

    #[test]
    fn test_address_not_zero() {
        let master = MasterKey::generate();
        let kp = master.derive_keypair(0, 0);
        let addr = kp.address(NetworkType::Mainnet);
        assert!(!addr.is_zero(), "Derived address must not be zero");
    }

    #[test]
    fn test_master_key_debug_redacted() {
        let master = MasterKey::generate();
        let debug_str = format!("{:?}", master);
        assert!(debug_str.contains("REDACTED"));
        assert!(!debug_str.contains(&master.mnemonic_phrase().to_string()));
    }

    #[test]
    fn test_known_mnemonic_produces_valid_key() {
        // Use a well-known test vector mnemonic (all "abandon" words)
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
        let master = MasterKey::from_mnemonic(phrase).expect("valid mnemonic");
        let kp = master.derive_keypair(0, 0);

        // Just verify we get a valid non-zero key
        assert_ne!(kp.public_key_bytes(), [0u8; 32]);

        // And that the address is valid
        let addr = kp.address(NetworkType::Devnet);
        let bech32 = addr.to_bech32();
        assert!(bech32.starts_with("zentradev1"));
    }
}
