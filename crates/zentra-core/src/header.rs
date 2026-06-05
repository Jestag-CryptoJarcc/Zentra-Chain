//! Block header for the Zentra BlockDAG.

use borsh::{BorshSerialize, BorshDeserialize};
use serde::{Serialize, Deserialize};
use zentra_types::*;

/// Block header containing all metadata for DAG positioning and PoW verification.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Header {
    /// Block version
    pub version: u32,
    /// Parent block hashes (multiple for DAG structure)
    pub parents: Vec<Hash>,
    /// Merkle root of all transactions in this block
    pub merkle_root: Hash,
    /// Unix timestamp in milliseconds
    pub timestamp: u64,
    /// PoW nonce
    pub nonce: u64,
    /// Which mining lane produced this block
    pub lane_id: LaneId,
    /// Compact difficulty target (Bitcoin-style nBits encoding)
    pub bits: u32,
    /// GhostDAG blue score
    pub blue_score: u64,
    /// Cumulative blue work
    pub blue_work: u128,
    /// DAG pruning reference point
    pub pruning_point: Hash,
}

impl Header {
    /// Compute the Blake2b-256 hash of this header.
    pub fn hash(&self) -> Hash {
        let encoded = borsh::to_vec(self).expect("header serialization cannot fail");
        Hash::hash(&encoded)
    }

    /// Convert compact nBits to a 256-bit target hash.
    ///
    /// Format: The first byte is the exponent, the remaining 3 bytes are the mantissa.
    /// target = mantissa × 2^(8 × (exponent - 3))
    pub fn target_from_bits(bits: u32) -> Hash {
        let exponent = (bits >> 24) as usize;
        let mantissa = bits & 0x007FFFFF;

        let mut target = [0u8; 32];
        if exponent == 0 {
            return Hash::from_bytes(target);
        }

        // Place mantissa bytes at the correct position
        if exponent <= 3 {
            let shifted = mantissa >> (8 * (3 - exponent));
            target[31] = (shifted & 0xFF) as u8;
            if exponent >= 2 {
                target[30] = ((shifted >> 8) & 0xFF) as u8;
            }
            if exponent >= 3 {
                target[29] = ((shifted >> 16) & 0xFF) as u8;
            }
        } else {
            let pos = 32usize.saturating_sub(exponent);
            let b0 = ((mantissa >> 16) & 0xFF) as u8;
            let b1 = ((mantissa >> 8) & 0xFF) as u8;
            let b2 = (mantissa & 0xFF) as u8;
            if pos < 32 {
                target[pos] = b0;
            }
            if pos + 1 < 32 {
                target[pos + 1] = b1;
            }
            if pos + 2 < 32 {
                target[pos + 2] = b2;
            }
        }

        Hash::from_bytes(target)
    }

    /// Check if this header's hash meets the difficulty target.
    pub fn meets_difficulty(&self) -> bool {
        let target = Self::target_from_bits(self.bits);
        self.hash().meets_target(&target)
    }

    /// Perform basic header validation (not PoW, just structure).
    pub fn validate_basic(&self) -> ZentraResult<()> {
        if self.version != BLOCK_VERSION {
            return Err(ZentraError::BlockValidation(
                format!("invalid version: expected {}, got {}", BLOCK_VERSION, self.version),
            ));
        }
        if self.parents.len() > MAX_BLOCK_PARENTS {
            return Err(ZentraError::BlockValidation(
                format!("too many parents: {} > {}", self.parents.len(), MAX_BLOCK_PARENTS),
            ));
        }
        // Genesis block has no parents, all others need at least one
        // (caller should enforce this contextually)
        Ok(())
    }

    /// Easiest difficulty bits (for genesis / devnet).
    pub fn easiest_bits() -> u32 {
        // Exponent=32, mantissa=0x7FFFFF → target is essentially all 0xFF
        0x207FFFFF
    }

    /// Create the genesis block header.
    pub fn genesis(network: NetworkType) -> Self {
        let _ = network; // may be used for different genesis params later
        Header {
            version: BLOCK_VERSION,
            parents: vec![],
            merkle_root: Hash::ZERO, // will be set after adding coinbase tx
            timestamp: 1_717_372_800_000, // 2024-06-03 00:00:00 UTC in ms
            nonce: 0,
            lane_id: LaneId::Cpu,
            bits: Self::easiest_bits(),
            blue_score: 0,
            blue_work: 0,
            pruning_point: Hash::ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_header_hash_deterministic() {
        let h = Header::genesis(NetworkType::Devnet);
        assert_eq!(h.hash(), h.hash());
    }

    #[test]
    fn test_target_from_bits() {
        let easy = Header::target_from_bits(Header::easiest_bits());
        assert!(!easy.is_zero());
    }

    #[test]
    fn test_genesis_validates() {
        let h = Header::genesis(NetworkType::Devnet);
        assert!(h.validate_basic().is_ok());
    }

    #[test]
    fn test_too_many_parents() {
        let mut h = Header::genesis(NetworkType::Devnet);
        h.parents = vec![Hash::ZERO; MAX_BLOCK_PARENTS + 1];
        assert!(h.validate_basic().is_err());
    }
}
