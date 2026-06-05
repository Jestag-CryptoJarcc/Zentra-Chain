//! 32-byte Blake2b hash type used throughout the Zentra network.

use blake2::{Blake2b, Digest};
use blake2::digest::consts::U32;
use borsh::{BorshSerialize, BorshDeserialize};
use serde::{Serialize, Deserialize};
use std::fmt;

/// Type alias for Blake2b with 32-byte output
type Blake2b256 = Blake2b<U32>;

/// A 32-byte Blake2b-256 hash used for block hashes, transaction IDs, and merkle nodes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Hash(pub [u8; 32]);

impl Hash {
    /// The zero hash (all zeros) — used as a sentinel/null value.
    pub const ZERO: Hash = Hash([0u8; 32]);

    /// Create a hash from raw bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Hash(bytes)
    }

    /// Create a hash from a byte slice. Panics if slice is not 32 bytes.
    pub fn from_slice(slice: &[u8]) -> Self {
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(slice);
        Hash(bytes)
    }

    /// Compute the Blake2b-256 hash of arbitrary data.
    pub fn hash(data: &[u8]) -> Self {
        let mut hasher = Blake2b256::new();
        hasher.update(data);
        let result = hasher.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&result);
        Hash(bytes)
    }

    /// Compute the double-hash (hash of hash) for extra security.
    pub fn double_hash(data: &[u8]) -> Self {
        let first = Self::hash(data);
        Self::hash(&first.0)
    }

    /// Combine two hashes (used in merkle tree construction).
    pub fn combine(left: &Hash, right: &Hash) -> Self {
        let mut combined = Vec::with_capacity(64);
        combined.extend_from_slice(&left.0);
        combined.extend_from_slice(&right.0);
        Self::hash(&combined)
    }

    /// Check if this is the zero hash.
    pub fn is_zero(&self) -> bool {
        self.0 == [0u8; 32]
    }

    /// Get the raw bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Convert to hex string.
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Parse from hex string.
    pub fn from_hex(s: &str) -> Result<Self, hex::FromHexError> {
        let bytes = hex::decode(s)?;
        if bytes.len() != 32 {
            return Err(hex::FromHexError::InvalidStringLength);
        }
        Ok(Self::from_slice(&bytes))
    }

    /// Check if the hash meets a difficulty target.
    /// The hash must be lexicographically less than or equal to the target.
    pub fn meets_target(&self, target: &Hash) -> bool {
        self.0 <= target.0
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash({})", &self.to_hex()[..16])
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

impl Default for Hash {
    fn default() -> Self {
        Hash::ZERO
    }
}

impl AsRef<[u8]> for Hash {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<[u8; 32]> for Hash {
    fn from(bytes: [u8; 32]) -> Self {
        Hash(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_deterministic() {
        let data = b"Zentra L1 BlockDAG";
        let h1 = Hash::hash(data);
        let h2 = Hash::hash(data);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_different_inputs() {
        let h1 = Hash::hash(b"block1");
        let h2 = Hash::hash(b"block2");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_zero_hash() {
        assert!(Hash::ZERO.is_zero());
        assert!(!Hash::hash(b"data").is_zero());
    }

    #[test]
    fn test_hex_roundtrip() {
        let hash = Hash::hash(b"test data");
        let hex_str = hash.to_hex();
        let parsed = Hash::from_hex(&hex_str).unwrap();
        assert_eq!(hash, parsed);
    }

    #[test]
    fn test_combine() {
        let left = Hash::hash(b"left");
        let right = Hash::hash(b"right");
        let combined = Hash::combine(&left, &right);
        assert_ne!(combined, left);
        assert_ne!(combined, right);
    }

    #[test]
    fn test_meets_target() {
        // All zeros hash meets any target
        let easy_target = Hash([0xFF; 32]);
        let hash = Hash::hash(b"test");
        assert!(hash.meets_target(&easy_target));

        // All zeros target — only zero hash meets it
        let hard_target = Hash::ZERO;
        assert!(!Hash::hash(b"test").meets_target(&hard_target));
    }
}
