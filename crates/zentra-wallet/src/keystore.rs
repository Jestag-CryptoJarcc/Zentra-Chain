//! # Encrypted Keystore
//!
//! Provides password-protected storage for wallet master keys.
//!
//! ## Security Design
//!
//! - **KDF**: Argon2id with 64 MiB memory cost, 3 iterations, 4 parallelism
//! - **Cipher**: AES-256-GCM with a random 12-byte nonce
//! - **Salt**: 32 random bytes per keystore (stored alongside the ciphertext)
//! - **Format**: JSON file on disk for portability
//!
//! The encrypted keystore never exposes the raw seed without the correct password.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::Argon2;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::keygen::MasterKey;
use zentra_types::error::{ZentraError, ZentraResult};

/// Current keystore format version.
const KEYSTORE_VERSION: u32 = 1;

/// Argon2id memory cost in KiB (64 MiB).
const ARGON2_MEM_COST_KIB: u32 = 65_536;
/// Argon2id iteration count.
const ARGON2_ITERATIONS: u32 = 3;
/// Argon2id parallelism.
const ARGON2_PARALLELISM: u32 = 4;

/// An encrypted keystore that holds a wallet's master seed protected by a password.
///
/// The seed is encrypted with AES-256-GCM; the encryption key is derived from
/// the user's password via Argon2id. The keystore can be serialized to / from
/// a JSON file on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keystore {
    /// AES-256-GCM ciphertext of the 64-byte seed + mnemonic phrase.
    pub encrypted_seed: Vec<u8>,
    /// 12-byte GCM nonce (unique per encryption).
    pub nonce: [u8; 12],
    /// 32-byte random salt for Argon2id.
    pub salt: [u8; 32],
    /// Keystore metadata (version, creation time, primary address).
    pub metadata: KeystoreMetadata,
}

/// Metadata stored alongside the encrypted keystore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeystoreMetadata {
    /// Keystore format version.
    pub version: u32,
    /// Unix timestamp (seconds) when the keystore was created.
    pub created_at: u64,
    /// Bech32m address of the first derived keypair (for display).
    pub address: String,
}

impl Keystore {
    /// Encrypt a [`MasterKey`] with the given password and return a new [`Keystore`].
    ///
    /// The seed and mnemonic phrase are concatenated, then encrypted with
    /// AES-256-GCM. The encryption key is derived from `password` via Argon2id
    /// with a fresh random salt.
    ///
    /// # Errors
    ///
    /// Returns [`ZentraError::Wallet`] if encryption fails.
    pub fn encrypt(master_key: &MasterKey, password: &str) -> ZentraResult<Self> {
        // Generate random salt and nonce
        let mut salt = [0u8; 32];
        OsRng.fill_bytes(&mut salt);

        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);

        // Derive 32-byte encryption key from password + salt
        let mut enc_key = [0u8; 32];
        Self::derive_key(password, &salt, &mut enc_key)?;

        // Build plaintext: seed (64 bytes) ‖ length(4 bytes LE) ‖ mnemonic phrase
        let mnemonic_bytes = master_key.mnemonic_phrase().as_bytes();
        let mnemonic_len = (mnemonic_bytes.len() as u32).to_le_bytes();

        let mut plaintext =
            Vec::with_capacity(64 + 4 + mnemonic_bytes.len());
        plaintext.extend_from_slice(master_key.seed());
        plaintext.extend_from_slice(&mnemonic_len);
        plaintext.extend_from_slice(mnemonic_bytes);

        // Encrypt with AES-256-GCM
        let cipher = Aes256Gcm::new_from_slice(&enc_key)
            .map_err(|e| ZentraError::Wallet(format!("Failed to create cipher: {}", e)))?;
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_slice())
            .map_err(|e| ZentraError::Wallet(format!("Encryption failed: {}", e)))?;

        // Zeroize sensitive intermediaries
        plaintext.zeroize();
        enc_key.zeroize();

        // Derive the primary address for metadata
        let kp = master_key.derive_keypair(0, 0);
        let addr = kp.address(zentra_types::constants::NetworkType::Mainnet);

        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        tracing::info!("Encrypted keystore created for address {}", addr);

        Ok(Keystore {
            encrypted_seed: ciphertext,
            nonce: nonce_bytes,
            salt,
            metadata: KeystoreMetadata {
                version: KEYSTORE_VERSION,
                created_at,
                address: addr.to_bech32(),
            },
        })
    }

    /// Decrypt this keystore with the given password and recover the [`MasterKey`].
    ///
    /// # Errors
    ///
    /// Returns [`ZentraError::Wallet`] if the password is incorrect or the
    /// data is corrupted (GCM authentication tag failure).
    pub fn decrypt(&self, password: &str) -> ZentraResult<MasterKey> {
        // Derive encryption key from password + stored salt
        let mut enc_key = [0u8; 32];
        Self::derive_key(password, &self.salt, &mut enc_key)?;

        // Decrypt with AES-256-GCM
        let cipher = Aes256Gcm::new_from_slice(&enc_key)
            .map_err(|e| ZentraError::Wallet(format!("Failed to create cipher: {}", e)))?;
        let nonce = Nonce::from_slice(&self.nonce);
        let mut plaintext = cipher
            .decrypt(nonce, self.encrypted_seed.as_slice())
            .map_err(|_| ZentraError::Wallet("Decryption failed: incorrect password or corrupted data".to_string()))?;

        enc_key.zeroize();

        // Parse plaintext: seed (64 bytes) ‖ length (4 bytes LE) ‖ mnemonic phrase
        if plaintext.len() < 68 {
            plaintext.zeroize();
            return Err(ZentraError::Wallet(
                "Decrypted data too short to contain seed and mnemonic".to_string(),
            ));
        }

        let mut seed = [0u8; 64];
        seed.copy_from_slice(&plaintext[..64]);

        let mut len_bytes = [0u8; 4];
        len_bytes.copy_from_slice(&plaintext[64..68]);
        let mnemonic_len = u32::from_le_bytes(len_bytes) as usize;

        if plaintext.len() < 68 + mnemonic_len {
            plaintext.zeroize();
            seed.zeroize();
            return Err(ZentraError::Wallet(
                "Decrypted data too short for mnemonic phrase".to_string(),
            ));
        }

        let mnemonic_phrase = String::from_utf8(plaintext[68..68 + mnemonic_len].to_vec())
            .map_err(|_| ZentraError::Wallet("Invalid UTF-8 in mnemonic".to_string()))?;

        plaintext.zeroize();

        tracing::info!("Keystore decrypted successfully");

        // Re-derive via MasterKey::from_mnemonic to validate the mnemonic
        let restored = MasterKey::from_mnemonic(&mnemonic_phrase)?;

        // Verify the seed matches what was stored (defense in depth)
        if restored.seed() != &seed {
            seed.zeroize();
            return Err(ZentraError::Wallet(
                "Mnemonic and seed mismatch after decryption".to_string(),
            ));
        }

        seed.zeroize();
        Ok(restored)
    }

    /// Save the keystore to a JSON file.
    ///
    /// # Errors
    ///
    /// Returns [`ZentraError::Wallet`] on I/O failure.
    pub fn save_to_file(&self, path: &Path) -> ZentraResult<()> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| ZentraError::Wallet(format!("JSON serialization failed: {}", e)))?;

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ZentraError::Wallet(format!("Failed to create directory: {}", e)))?;
        }

        std::fs::write(path, json.as_bytes())
            .map_err(|e| ZentraError::Wallet(format!("Failed to write keystore: {}", e)))?;

        tracing::info!(path = %path.display(), "Keystore saved to file");
        Ok(())
    }

    /// Load a keystore from a JSON file.
    ///
    /// # Errors
    ///
    /// Returns [`ZentraError::Wallet`] if the file cannot be read or parsed.
    pub fn load_from_file(path: &Path) -> ZentraResult<Self> {
        let data = std::fs::read_to_string(path)
            .map_err(|e| ZentraError::Wallet(format!("Failed to read keystore: {}", e)))?;

        let keystore: Keystore = serde_json::from_str(&data)
            .map_err(|e| ZentraError::Wallet(format!("Failed to parse keystore: {}", e)))?;

        tracing::info!(path = %path.display(), "Keystore loaded from file");
        Ok(keystore)
    }

    /// Derive a 32-byte encryption key from a password and salt using Argon2id.
    fn derive_key(password: &str, salt: &[u8; 32], output: &mut [u8; 32]) -> ZentraResult<()> {
        let params = argon2::Params::new(
            ARGON2_MEM_COST_KIB,
            ARGON2_ITERATIONS,
            ARGON2_PARALLELISM,
            Some(32),
        )
        .map_err(|e| ZentraError::Wallet(format!("Argon2 params error: {}", e)))?;

        let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);

        argon2
            .hash_password_into(password.as_bytes(), salt, output)
            .map_err(|e| ZentraError::Wallet(format!("Argon2 key derivation failed: {}", e)))?;

        Ok(())
    }
}

// ─── Unit Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keygen::MasterKey;
    use tempfile::TempDir;

    fn make_master() -> MasterKey {
        MasterKey::generate()
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let master = make_master();
        let password = "super-secret-password-123!";

        let keystore = Keystore::encrypt(&master, password).expect("encrypt");
        let restored = keystore.decrypt(password).expect("decrypt");

        assert_eq!(master.seed(), restored.seed());
        assert_eq!(master.mnemonic_phrase(), restored.mnemonic_phrase());
    }

    #[test]
    fn test_decrypt_wrong_password() {
        let master = make_master();
        let keystore = Keystore::encrypt(&master, "correct-password").expect("encrypt");

        let result = keystore.decrypt("wrong-password");
        assert!(result.is_err());
    }

    #[test]
    fn test_keystore_metadata() {
        let master = make_master();
        let keystore = Keystore::encrypt(&master, "pass").expect("encrypt");

        assert_eq!(keystore.metadata.version, KEYSTORE_VERSION);
        assert!(keystore.metadata.created_at > 0);
        assert!(keystore.metadata.address.starts_with("zentra1"));
    }

    #[test]
    fn test_save_and_load() {
        let master = make_master();
        let password = "file-test-password";

        let keystore = Keystore::encrypt(&master, password).expect("encrypt");

        let tmp_dir = TempDir::new().expect("tempdir");
        let file_path = tmp_dir.path().join("test-keystore.json");

        keystore.save_to_file(&file_path).expect("save");
        assert!(file_path.exists());

        let loaded = Keystore::load_from_file(&file_path).expect("load");
        let restored = loaded.decrypt(password).expect("decrypt");

        assert_eq!(master.seed(), restored.seed());
        assert_eq!(master.mnemonic_phrase(), restored.mnemonic_phrase());
    }

    #[test]
    fn test_save_creates_parent_dirs() {
        let master = make_master();
        let keystore = Keystore::encrypt(&master, "pass").expect("encrypt");

        let tmp_dir = TempDir::new().expect("tempdir");
        let file_path = tmp_dir.path().join("subdir").join("nested").join("keystore.json");

        keystore.save_to_file(&file_path).expect("save");
        assert!(file_path.exists());
    }

    #[test]
    fn test_load_nonexistent_file() {
        let result = Keystore::load_from_file(Path::new("/nonexistent/path/keystore.json"));
        assert!(result.is_err());
    }

    #[test]
    fn test_nonce_and_salt_are_random() {
        let master = make_master();
        let ks1 = Keystore::encrypt(&master, "pass").expect("encrypt 1");
        let ks2 = Keystore::encrypt(&master, "pass").expect("encrypt 2");

        // The ciphertext, nonce, and salt should all differ
        assert_ne!(ks1.nonce, ks2.nonce, "Nonces must differ");
        assert_ne!(ks1.salt, ks2.salt, "Salts must differ");
        assert_ne!(ks1.encrypted_seed, ks2.encrypted_seed, "Ciphertext must differ");
    }

    #[test]
    fn test_keystore_json_serialization() {
        let master = make_master();
        let keystore = Keystore::encrypt(&master, "pass").expect("encrypt");

        let json = serde_json::to_string(&keystore).expect("serialize");
        let parsed: Keystore = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(parsed.nonce, keystore.nonce);
        assert_eq!(parsed.salt, keystore.salt);
        assert_eq!(parsed.encrypted_seed, keystore.encrypted_seed);
        assert_eq!(parsed.metadata.version, keystore.metadata.version);
    }

    #[test]
    fn test_empty_password() {
        let master = make_master();
        let keystore = Keystore::encrypt(&master, "").expect("encrypt with empty password");
        let restored = keystore.decrypt("").expect("decrypt with empty password");
        assert_eq!(master.seed(), restored.seed());
    }
}
