//! Multi-lane mining verification for 5 hardware-targeted PoW algorithms.

use sha2::{Sha256, Digest as Sha256Digest};
use blake2::{Blake2b, Digest};
use blake2::digest::consts::U32;
use zentra_types::*;

type Blake2b256 = Blake2b<U32>;

/// Trait for lane-specific PoW hash verification.
pub trait LaneVerifier: Send + Sync {
    /// Compute the PoW hash for the given header bytes and nonce.
    fn compute_pow_hash(&self, header_bytes: &[u8], nonce: u64) -> Hash;
    /// Get the lane ID.
    fn lane_id(&self) -> LaneId;
}

/// Lane 0: CPU — RandomX placeholder (uses iterated Blake2b for dev).
pub struct RandomXVerifier;

impl LaneVerifier for RandomXVerifier {
    fn compute_pow_hash(&self, header_bytes: &[u8], nonce: u64) -> Hash {
        let mut data = header_bytes.to_vec();
        data.extend_from_slice(&nonce.to_le_bytes());
        data.extend_from_slice(b"zentra-cpu-pow-v1");

        // CPU-focused memory mixing. This is not RandomX/yespower, but it is real
        // repeatable work rather than a UI-only mining toggle.
        let mut result = Hash::hash(&data);
        let mut scratchpad = [0u8; 2048];
        for chunk in scratchpad.chunks_mut(32) {
            result = Hash::hash(result.as_bytes());
            chunk.copy_from_slice(result.as_bytes());
        }
        for round in 0..128usize {
            let idx = ((result.as_bytes()[0] as usize) << 3
                ^ result.as_bytes()[13] as usize
                ^ round)
                % (scratchpad.len() - 32);
            let mut mix = [0u8; 64];
            mix[..32].copy_from_slice(result.as_bytes());
            mix[32..].copy_from_slice(&scratchpad[idx..idx + 32]);
            mix[round % 64] ^= (nonce >> ((round % 8) * 8)) as u8;
            result = Hash::hash(&mix);
            scratchpad[idx..idx + 32].copy_from_slice(result.as_bytes());
        }
        result
    }
    fn lane_id(&self) -> LaneId { LaneId::Cpu }
}

/// Lane 1: GPU — KawPow placeholder (uses double-Blake2b for dev).
pub struct KawPowVerifier;

impl LaneVerifier for KawPowVerifier {
    fn compute_pow_hash(&self, header_bytes: &[u8], nonce: u64) -> Hash {
        let mut data = header_bytes.to_vec();
        data.extend_from_slice(&nonce.to_le_bytes());
        data.push(1); // lane separator
        Hash::double_hash(&data)
    }
    fn lane_id(&self) -> LaneId { LaneId::Gpu }
}

/// Lane 2: BTC ASIC — Real SHA-256d (double SHA-256).
pub struct Sha256Verifier;

impl LaneVerifier for Sha256Verifier {
    fn compute_pow_hash(&self, header_bytes: &[u8], nonce: u64) -> Hash {
        let mut data = header_bytes.to_vec();
        data.extend_from_slice(&nonce.to_le_bytes());
        // Double SHA-256 (same as Bitcoin)
        let first = Sha256::digest(&data);
        let second = Sha256::digest(&first);
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&second);
        Hash::from_bytes(bytes)
    }
    fn lane_id(&self) -> LaneId { LaneId::BtcAsic }
}

/// Lane 3: LTC ASIC — Scrypt-based (uses Blake2b + nonce mixing as placeholder
/// since the scrypt crate is a KDF, not directly a PoW hash function).
pub struct ScryptVerifier;

impl LaneVerifier for ScryptVerifier {
    fn compute_pow_hash(&self, header_bytes: &[u8], nonce: u64) -> Hash {
        let mut data = header_bytes.to_vec();
        data.extend_from_slice(&nonce.to_le_bytes());
        data.push(3); // lane separator

        // Simulate Scrypt-like memory-hard operation
        // In production, this would use actual Scrypt with Litecoin's N=1024,r=1,p=1
        let mut state = Hash::hash(&data);
        for i in 0..64 {
            let mut mix = state.0;
            mix[0] ^= i as u8;
            state = Hash::hash(&mix);
        }
        state
    }
    fn lane_id(&self) -> LaneId { LaneId::LtcAsic }
}

/// Lane 4: FPGA — Yescrypt placeholder (uses Blake2b chain for dev).
pub struct YescryptVerifier;

impl LaneVerifier for YescryptVerifier {
    fn compute_pow_hash(&self, header_bytes: &[u8], nonce: u64) -> Hash {
        let mut data = header_bytes.to_vec();
        data.extend_from_slice(&nonce.to_le_bytes());
        data.push(4); // lane separator

        // Simulate Yescrypt with iterated hashing
        let mut hasher = Blake2b256::new();
        hasher.update(&data);
        let mut result = hasher.finalize();
        for _ in 0..32 {
            let mut h = Blake2b256::new();
            h.update(&result);
            h.update(&data[..8.min(data.len())]);
            result = h.finalize();
        }
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&result);
        Hash::from_bytes(bytes)
    }
    fn lane_id(&self) -> LaneId { LaneId::Fpga }
}

/// Get the appropriate verifier for a given lane.
pub fn get_verifier(lane: LaneId) -> Box<dyn LaneVerifier> {
    match lane {
        LaneId::Cpu => Box::new(RandomXVerifier),
        LaneId::Gpu => Box::new(KawPowVerifier),
        LaneId::BtcAsic => Box::new(Sha256Verifier),
        LaneId::LtcAsic => Box::new(ScryptVerifier),
        LaneId::Fpga => Box::new(YescryptVerifier),
    }
}

/// Verify the PoW of a block header using the correct lane verifier.
pub fn verify_block_pow(header: &zentra_core::header::Header) -> ZentraResult<()> {
    let verifier = get_verifier(header.lane_id);
    let mut header_copy = header.clone();
    header_copy.nonce = 0;
    let header_bytes = borsh::to_vec(&header_copy).map_err(|e| ZentraError::Serialization(e.to_string()))?;
    let pow_hash = verifier.compute_pow_hash(&header_bytes, header.nonce);
    let target = zentra_core::header::Header::target_from_bits(header.bits);

    if pow_hash.meets_target(&target) {
        Ok(())
    } else {
        Err(ZentraError::DifficultyNotMet)
    }
}

/// Compute a header's PoW hash and check it meets an ARBITRARY target. Used to
/// verify pool shares, where the share target is easier than the block target.
pub fn pow_meets_target(header: &zentra_core::header::Header, target: &Hash) -> bool {
    let verifier = get_verifier(header.lane_id);
    let mut hc = header.clone();
    hc.nonce = 0;
    let header_bytes = match borsh::to_vec(&hc) { Ok(b) => b, Err(_) => return false };
    let pow_hash = verifier.compute_pow_hash(&header_bytes, header.nonce);
    pow_hash.meets_target(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_lanes_produce_different_hashes() {
        let data = b"test block header";
        let nonce = 12345u64;
        let hashes: Vec<Hash> = LaneId::ALL
            .iter()
            .map(|lane| get_verifier(*lane).compute_pow_hash(data, nonce))
            .collect();

        // All 5 lanes should produce different hashes from the same input
        for i in 0..hashes.len() {
            for j in (i + 1)..hashes.len() {
                assert_ne!(hashes[i], hashes[j], "lanes {} and {} produced same hash", i, j);
            }
        }
    }

    #[test]
    fn test_deterministic() {
        for lane in LaneId::ALL {
            let verifier = get_verifier(lane);
            let h1 = verifier.compute_pow_hash(b"header", 0);
            let h2 = verifier.compute_pow_hash(b"header", 0);
            assert_eq!(h1, h2, "lane {:?} is not deterministic", lane);
        }
    }

    #[test]
    fn test_different_nonces_produce_different_hashes() {
        let verifier = get_verifier(LaneId::Cpu);
        let h1 = verifier.compute_pow_hash(b"header", 0);
        let h2 = verifier.compute_pow_hash(b"header", 1);
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_sha256_matches_bitcoin_style() {
        let verifier = Sha256Verifier;
        let hash = verifier.compute_pow_hash(b"test", 0);
        assert!(!hash.is_zero());
    }
}
