//! # Encrypted Mempool for MEV Protection
//!
//! Implements a commit-reveal transaction ordering scheme to prevent
//! Miner Extractable Value (MEV) attacks. Transactions are encrypted before
//! submission and only decrypted after ordering commitments are finalized.
//!
//! ## Flow
//! 1. User encrypts their transaction with the current epoch key
//! 2. Encrypted transaction + commitment hash are submitted to the mempool
//! 3. Block proposer orders transactions by commitment (cannot see contents)
//! 4. After ordering is locked, the epoch decryption key is revealed
//! 5. All transactions are decrypted and executed in the committed order
//!
//! ## Encryption
//! Uses XOR-based encryption for development/testing. In production, this
//! would use threshold encryption (e.g., DRAND-based) where the decryption
//! key is revealed by a committee after the ordering deadline.

use serde::{Serialize, Deserialize};
use zentra_types::{Address, Hash, ZentraError, NetworkType};
use zentra_types::error::ZentraResult;

/// An encrypted transaction awaiting decryption in the mempool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedTransaction {
    /// The encrypted transaction payload.
    pub ciphertext: Vec<u8>,
    /// Blake2b-256 commitment hash of the plaintext transaction.
    /// Used for ordering before decryption.
    pub commitment: Hash,
    /// Address of the transaction submitter.
    pub submitter: Address,
    /// Epoch or block height at which this was submitted.
    pub submitted_at: u64,
}

/// Encrypted mempool that holds transactions until the decryption key is revealed.
///
/// Prevents MEV by ensuring block proposers cannot see transaction contents
/// during the ordering phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedMempool {
    /// Queue of encrypted transactions awaiting decryption.
    pending: Vec<EncryptedTransaction>,
    /// The epoch decryption key (set when the ordering phase ends).
    decryption_key: Option<[u8; 32]>,
}

impl EncryptedMempool {
    /// Create a new empty encrypted mempool.
    pub fn new() -> Self {
        tracing::info!("encrypted mempool initialized — MEV protection active");
        Self {
            pending: Vec::new(),
            decryption_key: None,
        }
    }

    /// Submit an encrypted transaction to the mempool.
    ///
    /// The transaction is added to the pending queue. Duplicate commitments
    /// are rejected to prevent replay attacks.
    ///
    /// # Errors
    /// - `ZentraError::TransactionValidation` if the ciphertext is empty.
    /// - `ZentraError::TransactionValidation` if a duplicate commitment exists.
    pub fn submit_encrypted(
        &mut self,
        encrypted_tx: EncryptedTransaction,
    ) -> ZentraResult<()> {
        if encrypted_tx.ciphertext.is_empty() {
            return Err(ZentraError::TransactionValidation(
                "encrypted transaction ciphertext must be non-empty".into(),
            ));
        }

        // Check for duplicate commitments
        let is_dup = self
            .pending
            .iter()
            .any(|tx| tx.commitment == encrypted_tx.commitment);
        if is_dup {
            return Err(ZentraError::TransactionValidation(
                "duplicate transaction commitment".into(),
            ));
        }

        tracing::debug!(
            commitment = %encrypted_tx.commitment,
            submitter = %encrypted_tx.submitter,
            submitted_at = encrypted_tx.submitted_at,
            ciphertext_len = encrypted_tx.ciphertext.len(),
            "encrypted transaction submitted to mempool"
        );

        self.pending.push(encrypted_tx);
        Ok(())
    }

    /// Encrypt a raw transaction payload with the given epoch key.
    ///
    /// Uses XOR-based stream cipher for development. The key is extended
    /// via Blake2b hashing to cover the full plaintext length.
    ///
    /// # Production Note
    /// In production, replace with threshold encryption (e.g., DRAND-based
    /// IBE or PVSS scheme) where the decryption key is jointly revealed by
    /// a validator committee.
    ///
    /// # Arguments
    /// - `tx`: Raw transaction bytes to encrypt.
    /// - `epoch_key`: 32-byte encryption key for this epoch.
    ///
    /// # Returns
    /// An `EncryptedTransaction` with the ciphertext and commitment hash.
    pub fn encrypt_transaction(
        tx: &[u8],
        epoch_key: &[u8; 32],
    ) -> EncryptedTransaction {
        let commitment = Hash::hash(tx);

        // XOR-based stream cipher: extend key to match plaintext length
        let keystream = generate_keystream(epoch_key, tx.len());
        let ciphertext: Vec<u8> = tx
            .iter()
            .zip(keystream.iter())
            .map(|(p, k)| p ^ k)
            .collect();

        // Use a deterministic submitter address derived from the epoch key
        // (in production, this would be the real sender's address)
        let submitter = Address::from_payload(
            Hash::hash(epoch_key).0,
            NetworkType::Devnet,
        );

        tracing::debug!(
            commitment = %commitment,
            plaintext_len = tx.len(),
            ciphertext_len = ciphertext.len(),
            "transaction encrypted for MEV-protected submission"
        );

        EncryptedTransaction {
            ciphertext,
            commitment,
            submitter,
            submitted_at: 0,
        }
    }

    /// Set the epoch decryption key.
    ///
    /// Called after the ordering phase is complete and commitments are locked.
    /// Once set, `decrypt_all` can be called to reveal all transaction contents.
    pub fn set_decryption_key(&mut self, key: [u8; 32]) {
        tracing::info!("decryption key set — transactions can now be revealed");
        self.decryption_key = Some(key);
    }

    /// Decrypt all pending transactions and drain the mempool.
    ///
    /// Returns the decrypted raw transaction bytes in their committed order.
    /// If no decryption key has been set, returns an empty vector.
    /// After decryption, the pending queue is cleared and the key is consumed.
    pub fn decrypt_all(&mut self) -> Vec<Vec<u8>> {
        let key = match self.decryption_key.take() {
            Some(k) => k,
            None => {
                tracing::warn!("decrypt_all called but no decryption key is set");
                return Vec::new();
            }
        };

        let transactions: Vec<Vec<u8>> = self
            .pending
            .drain(..)
            .map(|enc_tx| {
                let keystream = generate_keystream(&key, enc_tx.ciphertext.len());
                enc_tx
                    .ciphertext
                    .iter()
                    .zip(keystream.iter())
                    .map(|(c, k)| c ^ k)
                    .collect()
            })
            .collect();

        tracing::info!(
            count = transactions.len(),
            "all transactions decrypted and mempool drained"
        );

        transactions
    }

    /// Get the commitment hashes of all pending transactions.
    ///
    /// Used during the ordering phase to establish transaction order
    /// without revealing transaction contents.
    pub fn get_commitments(&self) -> Vec<Hash> {
        self.pending.iter().map(|tx| tx.commitment).collect()
    }

    /// Get the number of pending encrypted transactions.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Check if a decryption key has been set.
    pub fn has_decryption_key(&self) -> bool {
        self.decryption_key.is_some()
    }

    /// Clear all pending transactions without decrypting.
    ///
    /// Used for epoch transitions where pending transactions should be dropped.
    pub fn clear(&mut self) {
        let count = self.pending.len();
        self.pending.clear();
        self.decryption_key = None;
        tracing::info!(count, "encrypted mempool cleared");
    }
}

impl Default for EncryptedMempool {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate a deterministic keystream from a 32-byte key using iterated Blake2b hashing.
///
/// Produces a keystream of at least `length` bytes by hashing the key repeatedly.
/// Each 32-byte block of the keystream is `H(key || block_index)`.
fn generate_keystream(key: &[u8; 32], length: usize) -> Vec<u8> {
    let mut keystream = Vec::with_capacity(length);
    let mut block_index: u64 = 0;

    while keystream.len() < length {
        let mut input = Vec::with_capacity(40);
        input.extend_from_slice(key);
        input.extend_from_slice(&block_index.to_le_bytes());
        let block = Hash::hash(&input);
        keystream.extend_from_slice(block.as_bytes());
        block_index += 1;
    }

    keystream.truncate(length);
    keystream
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_epoch_key() -> [u8; 32] {
        [0x42u8; 32]
    }

    fn test_address() -> Address {
        Address::from_payload([0xAA; 32], NetworkType::Devnet)
    }

    fn sample_encrypted_tx(data: &[u8], key: &[u8; 32]) -> EncryptedTransaction {
        let mut tx = EncryptedMempool::encrypt_transaction(data, key);
        tx.submitter = test_address();
        tx.submitted_at = 100;
        tx
    }

    #[test]
    fn test_new_mempool() {
        let mempool = EncryptedMempool::new();
        assert_eq!(mempool.pending_count(), 0);
        assert!(!mempool.has_decryption_key());
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = test_epoch_key();
        let plaintext = b"transfer 100 ZTR to Alice";

        let encrypted = EncryptedMempool::encrypt_transaction(plaintext, &key);
        assert_ne!(encrypted.ciphertext, plaintext);
        assert_eq!(encrypted.commitment, Hash::hash(plaintext));

        // Decrypt
        let keystream = generate_keystream(&key, encrypted.ciphertext.len());
        let decrypted: Vec<u8> = encrypted
            .ciphertext
            .iter()
            .zip(keystream.iter())
            .map(|(c, k)| c ^ k)
            .collect();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_submit_and_decrypt() {
        let key = test_epoch_key();
        let mut mempool = EncryptedMempool::new();

        let tx1 = sample_encrypted_tx(b"tx_one", &key);
        let tx2 = sample_encrypted_tx(b"tx_two", &key);

        mempool.submit_encrypted(tx1).unwrap();
        mempool.submit_encrypted(tx2).unwrap();
        assert_eq!(mempool.pending_count(), 2);

        mempool.set_decryption_key(key);
        assert!(mempool.has_decryption_key());

        let decrypted = mempool.decrypt_all();
        assert_eq!(decrypted.len(), 2);
        assert_eq!(decrypted[0], b"tx_one");
        assert_eq!(decrypted[1], b"tx_two");
        assert_eq!(mempool.pending_count(), 0);
        assert!(!mempool.has_decryption_key()); // Key consumed
    }

    #[test]
    fn test_submit_empty_ciphertext_fails() {
        let mut mempool = EncryptedMempool::new();
        let bad_tx = EncryptedTransaction {
            ciphertext: vec![],
            commitment: Hash::ZERO,
            submitter: test_address(),
            submitted_at: 0,
        };
        assert!(mempool.submit_encrypted(bad_tx).is_err());
    }

    #[test]
    fn test_submit_duplicate_commitment_fails() {
        let key = test_epoch_key();
        let mut mempool = EncryptedMempool::new();

        let tx = sample_encrypted_tx(b"same_data", &key);
        mempool.submit_encrypted(tx.clone()).unwrap();
        assert!(mempool.submit_encrypted(tx).is_err());
    }

    #[test]
    fn test_decrypt_without_key_returns_empty() {
        let key = test_epoch_key();
        let mut mempool = EncryptedMempool::new();
        mempool
            .submit_encrypted(sample_encrypted_tx(b"hello", &key))
            .unwrap();

        let decrypted = mempool.decrypt_all();
        assert!(decrypted.is_empty());
        // Transactions should still be pending
        assert_eq!(mempool.pending_count(), 1);
    }

    #[test]
    fn test_get_commitments() {
        let key = test_epoch_key();
        let mut mempool = EncryptedMempool::new();

        let data1 = b"first_tx";
        let data2 = b"second_tx";
        mempool
            .submit_encrypted(sample_encrypted_tx(data1, &key))
            .unwrap();
        mempool
            .submit_encrypted(sample_encrypted_tx(data2, &key))
            .unwrap();

        let commitments = mempool.get_commitments();
        assert_eq!(commitments.len(), 2);
        assert_eq!(commitments[0], Hash::hash(data1));
        assert_eq!(commitments[1], Hash::hash(data2));
    }

    #[test]
    fn test_clear() {
        let key = test_epoch_key();
        let mut mempool = EncryptedMempool::new();
        mempool
            .submit_encrypted(sample_encrypted_tx(b"data", &key))
            .unwrap();
        mempool.set_decryption_key(key);

        mempool.clear();
        assert_eq!(mempool.pending_count(), 0);
        assert!(!mempool.has_decryption_key());
    }

    #[test]
    fn test_keystream_deterministic() {
        let key = [0x99u8; 32];
        let ks1 = generate_keystream(&key, 100);
        let ks2 = generate_keystream(&key, 100);
        assert_eq!(ks1, ks2);
    }

    #[test]
    fn test_keystream_different_keys() {
        let key1 = [0x01u8; 32];
        let key2 = [0x02u8; 32];
        let ks1 = generate_keystream(&key1, 100);
        let ks2 = generate_keystream(&key2, 100);
        assert_ne!(ks1, ks2);
    }

    #[test]
    fn test_encrypt_different_keys_different_ciphertext() {
        let key1 = [0x01u8; 32];
        let key2 = [0x02u8; 32];
        let data = b"same plaintext";
        let enc1 = EncryptedMempool::encrypt_transaction(data, &key1);
        let enc2 = EncryptedMempool::encrypt_transaction(data, &key2);
        assert_ne!(enc1.ciphertext, enc2.ciphertext);
        // But commitments should be the same (hash of plaintext)
        assert_eq!(enc1.commitment, enc2.commitment);
    }

    #[test]
    fn test_large_transaction_roundtrip() {
        let key = test_epoch_key();
        let large_tx: Vec<u8> = (0..10_000).map(|i| (i % 256) as u8).collect();

        let encrypted = EncryptedMempool::encrypt_transaction(&large_tx, &key);
        assert_eq!(encrypted.ciphertext.len(), large_tx.len());

        let keystream = generate_keystream(&key, encrypted.ciphertext.len());
        let decrypted: Vec<u8> = encrypted
            .ciphertext
            .iter()
            .zip(keystream.iter())
            .map(|(c, k)| c ^ k)
            .collect();
        assert_eq!(decrypted, large_tx);
    }

    #[test]
    fn test_full_mev_protection_flow() {
        let key = test_epoch_key();
        let mut mempool = EncryptedMempool::new();

        // 1. Users encrypt and submit transactions
        let tx_data = vec![
            b"buy 100 ZTR".to_vec(),
            b"sell 50 ZTR".to_vec(),
            b"swap 200 zUSD".to_vec(),
        ];

        for data in &tx_data {
            let enc = sample_encrypted_tx(data, &key);
            mempool.submit_encrypted(enc).unwrap();
        }

        // 2. Get commitments for ordering (can't see contents)
        let commitments = mempool.get_commitments();
        assert_eq!(commitments.len(), 3);

        // 3. After ordering is locked, reveal decryption key
        mempool.set_decryption_key(key);

        // 4. Decrypt all transactions
        let decrypted = mempool.decrypt_all();
        assert_eq!(decrypted.len(), 3);
        assert_eq!(decrypted[0], b"buy 100 ZTR");
        assert_eq!(decrypted[1], b"sell 50 ZTR");
        assert_eq!(decrypted[2], b"swap 200 zUSD");
    }

    #[test]
    fn test_commitment_integrity() {
        let key = test_epoch_key();
        let data = b"important transaction";
        let enc = EncryptedMempool::encrypt_transaction(data, &key);

        // Commitment should match the hash of the original plaintext
        assert_eq!(enc.commitment, Hash::hash(data));
    }
}
