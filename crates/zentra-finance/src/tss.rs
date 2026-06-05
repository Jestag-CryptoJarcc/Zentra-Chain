//! # Threshold Signature Scheme (TSS) Interface
//!
//! Provides an abstract framework for threshold signature operations used in
//! cross-chain vault management. In production, this would use a full
//! Feldman VSS / FROST DKG protocol. For now, it uses simulated key generation
//! with Ed25519 signing for testability.
//!
//! ## Key Concepts
//! - **Threshold (t)**: Minimum number of signers required to produce a valid signature
//! - **Total participants (n)**: Total number of key holders
//! - **Group public key**: The combined public key that verifies threshold signatures
//! - **DKG**: Distributed Key Generation — each participant gets a secret share
//!   without any single party knowing the full secret

use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Serialize, Deserialize};
use zentra_types::{Hash, ZentraError};
use zentra_types::error::ZentraResult;

/// A participant's key material in the TSS scheme.
///
/// In a real FROST/Feldman deployment, `secret_share` would be a Shamir share
/// of the group secret. Here we simulate with per-participant Ed25519 keys.
#[derive(Clone, Serialize, Deserialize)]
pub struct TssKeyPair {
    /// The participant's public key share (32-byte Ed25519 public key).
    pub public_share: [u8; 32],
    /// The participant's secret key share (serialized Ed25519 secret key).
    pub secret_share: Vec<u8>,
    /// Zero-indexed participant identifier.
    pub participant_index: u16,
    /// Minimum number of signers needed.
    pub threshold: u16,
    /// Total number of participants.
    pub total_participants: u16,
}

impl std::fmt::Debug for TssKeyPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TssKeyPair")
            .field("participant_index", &self.participant_index)
            .field("threshold", &self.threshold)
            .field("total_participants", &self.total_participants)
            .field("public_share", &hex::encode(self.public_share))
            .field("secret_share", &"<redacted>")
            .finish()
    }
}

/// Manager for threshold signature operations.
///
/// Holds all participants' key pairs and the combined group public key.
/// Orchestrates distributed key generation, threshold signing, and
/// signature verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TssManager {
    /// Key material for each participant.
    pub participants: Vec<TssKeyPair>,
    /// The group's combined public key (used for verification).
    pub group_public_key: [u8; 32],
    /// Minimum number of signers for a valid threshold signature.
    pub threshold: u16,
}

impl TssManager {
    /// Create a new TSS manager with the given threshold and total participant count.
    ///
    /// Keys are not generated until [`generate_keys`] is called.
    ///
    /// # Arguments
    /// - `threshold`: Minimum signers required (must be ≥ 1 and ≤ total).
    /// - `total`: Total number of participants.
    pub fn new(threshold: u16, total: u16) -> Self {
        tracing::info!(threshold, total, "creating new TSS manager");
        Self {
            participants: Vec::with_capacity(total as usize),
            group_public_key: [0u8; 32],
            threshold,
        }
    }

    /// Run a simulated Distributed Key Generation (DKG) ceremony.
    ///
    /// Generates an Ed25519 key pair for each participant. The group public key
    /// is derived by hashing all individual public keys together. In a real
    /// FROST implementation, the group key would be the Lagrange-interpolated
    /// combination of public shares.
    ///
    /// # Returns
    /// The 32-byte group public key.
    ///
    /// # Errors
    /// - `ZentraError::TssError` if threshold > total or threshold is zero.
    pub fn generate_keys(&mut self) -> ZentraResult<[u8; 32]> {
        let total = self.participants.capacity().max(1) as u16;
        if self.threshold == 0 {
            return Err(ZentraError::TssError(
                "threshold must be at least 1".into(),
            ));
        }
        if self.threshold > total {
            return Err(ZentraError::TssError(format!(
                "threshold {} exceeds total participants {}",
                self.threshold, total
            )));
        }

        self.participants.clear();
        let mut public_keys: Vec<[u8; 32]> = Vec::with_capacity(total as usize);

        for i in 0..total {
            let signing_key = SigningKey::generate(&mut OsRng);
            let verifying_key = signing_key.verifying_key();
            let public_bytes: [u8; 32] = verifying_key.to_bytes();

            public_keys.push(public_bytes);

            self.participants.push(TssKeyPair {
                public_share: public_bytes,
                secret_share: signing_key.to_bytes().to_vec(),
                participant_index: i,
                threshold: self.threshold,
                total_participants: total,
            });
        }

        // Derive group public key by hashing all individual public keys.
        // In real FROST, this would be a point addition on the Ed25519 curve.
        let mut combined = Vec::with_capacity(public_keys.len() * 32);
        for pk in &public_keys {
            combined.extend_from_slice(pk);
        }
        self.group_public_key = Hash::hash(&combined).0;

        tracing::info!(
            total,
            threshold = self.threshold,
            group_key = hex::encode(self.group_public_key),
            "DKG completed — group public key derived"
        );

        Ok(self.group_public_key)
    }

    /// Produce a threshold signature over a message.
    ///
    /// Requires at least `threshold` signer indices. Each signer produces a
    /// partial Ed25519 signature; the combined signature is the hash of all
    /// partial signatures concatenated. In a real FROST scheme, the partial
    /// signatures would be Lagrange-interpolated into a single Ed25519 signature.
    ///
    /// # Arguments
    /// - `message`: The raw bytes to sign.
    /// - `signers`: Indices of the participating signers (must have length ≥ threshold).
    ///
    /// # Errors
    /// - `ZentraError::TssError` if fewer than `threshold` signers are provided.
    /// - `ZentraError::TssError` if a signer index is out of bounds.
    /// - `ZentraError::TssError` if keys have not been generated.
    pub fn sign(&self, message: &[u8], signers: &[u16]) -> ZentraResult<Vec<u8>> {
        if self.participants.is_empty() {
            return Err(ZentraError::TssError(
                "keys not generated — call generate_keys() first".into(),
            ));
        }
        if (signers.len() as u16) < self.threshold {
            return Err(ZentraError::TssError(format!(
                "need {} signers but only {} provided",
                self.threshold,
                signers.len()
            )));
        }

        let mut partial_sigs: Vec<u8> = Vec::new();
        for &idx in signers {
            let participant = self
                .participants
                .get(idx as usize)
                .ok_or_else(|| {
                    ZentraError::TssError(format!("signer index {} out of range", idx))
                })?;

            // Reconstruct the signing key from the stored secret share
            let secret_bytes: [u8; 32] = participant
                .secret_share
                .as_slice()
                .try_into()
                .map_err(|_| {
                    ZentraError::TssError(format!(
                        "invalid secret share length for participant {}",
                        idx
                    ))
                })?;
            let signing_key = SigningKey::from_bytes(&secret_bytes);
            let sig = signing_key.sign(message);
            partial_sigs.extend_from_slice(&sig.to_bytes());
        }

        // Combine partial signatures into a single "threshold signature".
        // In real FROST: Lagrange interpolation of partial sigs.
        // Simulated: hash of all partial sigs + group public key + message.
        let mut combined = Vec::new();
        combined.extend_from_slice(&partial_sigs);
        combined.extend_from_slice(&self.group_public_key);
        combined.extend_from_slice(message);
        let threshold_sig = Hash::hash(&combined);

        // Return the threshold sig followed by the partial signatures for auditing
        let mut result = Vec::with_capacity(32 + partial_sigs.len());
        result.extend_from_slice(threshold_sig.as_bytes());
        result.extend_from_slice(&partial_sigs);

        tracing::debug!(
            num_signers = signers.len(),
            threshold = self.threshold,
            sig_len = result.len(),
            "threshold signature produced"
        );

        Ok(result)
    }

    /// Verify a threshold signature against the group public key.
    ///
    /// Reconstructs the expected threshold hash from the partial signatures
    /// embedded in the signature blob and checks it matches the leading 32 bytes.
    ///
    /// # Arguments
    /// - `message`: The original message that was signed.
    /// - `signature`: The signature blob returned by [`sign`].
    pub fn verify(&self, message: &[u8], signature: &[u8]) -> bool {
        if signature.len() < 32 {
            tracing::warn!(sig_len = signature.len(), "signature too short to verify");
            return false;
        }

        let claimed_hash = &signature[..32];
        let partial_sigs = &signature[32..];

        // Re-derive the threshold hash
        let mut combined = Vec::new();
        combined.extend_from_slice(partial_sigs);
        combined.extend_from_slice(&self.group_public_key);
        combined.extend_from_slice(message);
        let expected = Hash::hash(&combined);

        let valid = claimed_hash == expected.as_bytes();
        if !valid {
            tracing::warn!("threshold signature verification FAILED");
        }
        valid
    }

    /// Identify which participant submitted an invalid partial signature.
    ///
    /// Given a set of `(participant_index, partial_signature)` pairs and the
    /// original message, verifies each partial signature against the
    /// participant's individual public key. Returns the index of the first
    /// participant whose partial signature is invalid, or `None` if all are valid.
    ///
    /// # Arguments
    /// - `partial_sigs`: Slice of `(participant_index, partial_signature_bytes)`.
    /// - `message`: The original message.
    pub fn identify_abort(
        &self,
        partial_sigs: &[(u16, Vec<u8>)],
        message: &[u8],
    ) -> Option<u16> {
        for (idx, sig_bytes) in partial_sigs {
            let participant = match self.participants.get(*idx as usize) {
                Some(p) => p,
                None => {
                    tracing::warn!(idx, "abort check: participant index out of range");
                    return Some(*idx);
                }
            };

            // Try to verify the partial signature against the participant's public key
            let verifying_key = match VerifyingKey::from_bytes(&participant.public_share) {
                Ok(vk) => vk,
                Err(_) => {
                    tracing::warn!(idx, "abort check: invalid public share");
                    return Some(*idx);
                }
            };

            if sig_bytes.len() != 64 {
                tracing::warn!(
                    idx,
                    len = sig_bytes.len(),
                    "abort check: invalid partial signature length"
                );
                return Some(*idx);
            }

            let sig_array: [u8; 64] = match sig_bytes.as_slice().try_into() {
                Ok(a) => a,
                Err(_) => return Some(*idx),
            };

            let signature = ed25519_dalek::Signature::from_bytes(&sig_array);
            if verifying_key.verify(message, &signature).is_err() {
                tracing::warn!(idx, "abort check: participant submitted invalid signature");
                return Some(*idx);
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_manager(threshold: u16, total: u16) -> TssManager {
        let mut mgr = TssManager::new(threshold, total);
        mgr.generate_keys().expect("DKG should succeed");
        mgr
    }

    #[test]
    fn test_new_manager() {
        let mgr = TssManager::new(3, 5);
        assert_eq!(mgr.threshold, 3);
        assert!(mgr.participants.is_empty());
        assert_eq!(mgr.group_public_key, [0u8; 32]);
    }

    #[test]
    fn test_generate_keys() {
        let mut mgr = TssManager::new(2, 3);
        let gpk = mgr.generate_keys().unwrap();
        assert_ne!(gpk, [0u8; 32]);
        assert_eq!(mgr.participants.len(), 3);
        for (i, p) in mgr.participants.iter().enumerate() {
            assert_eq!(p.participant_index, i as u16);
            assert_eq!(p.threshold, 2);
            assert_eq!(p.total_participants, 3);
            assert_ne!(p.public_share, [0u8; 32]);
            assert_eq!(p.secret_share.len(), 32);
        }
    }

    #[test]
    fn test_generate_keys_threshold_zero() {
        let mut mgr = TssManager::new(0, 3);
        assert!(mgr.generate_keys().is_err());
    }

    #[test]
    fn test_generate_keys_threshold_exceeds_total() {
        let mut mgr = TssManager::new(5, 3);
        assert!(mgr.generate_keys().is_err());
    }

    #[test]
    fn test_sign_and_verify() {
        let mgr = create_manager(2, 3);
        let message = b"validate cross-chain ingest tx_hash_abc123";

        let signers: Vec<u16> = vec![0, 1];
        let sig = mgr.sign(message, &signers).unwrap();
        assert!(sig.len() > 32);
        assert!(mgr.verify(message, &sig));
    }

    #[test]
    fn test_verify_wrong_message_fails() {
        let mgr = create_manager(2, 3);
        let sig = mgr.sign(b"correct message", &[0, 2]).unwrap();
        assert!(!mgr.verify(b"wrong message", &sig));
    }

    #[test]
    fn test_sign_insufficient_signers() {
        let mgr = create_manager(3, 5);
        let result = mgr.sign(b"msg", &[0, 1]); // only 2, need 3
        assert!(result.is_err());
    }

    #[test]
    fn test_sign_invalid_signer_index() {
        let mgr = create_manager(2, 3);
        let result = mgr.sign(b"msg", &[0, 99]); // 99 is out of range
        assert!(result.is_err());
    }

    #[test]
    fn test_sign_without_keygen() {
        let mgr = TssManager::new(2, 3);
        let result = mgr.sign(b"msg", &[0, 1]);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_short_signature() {
        let mgr = create_manager(2, 3);
        assert!(!mgr.verify(b"msg", &[0u8; 16])); // too short
    }

    #[test]
    fn test_verify_tampered_signature() {
        let mgr = create_manager(2, 3);
        let msg = b"important message";
        let mut sig = mgr.sign(msg, &[0, 1]).unwrap();
        // Tamper with the threshold hash
        sig[0] ^= 0xFF;
        assert!(!mgr.verify(msg, &sig));
    }

    #[test]
    fn test_identify_abort_all_valid() {
        let mgr = create_manager(2, 3);
        let message = b"test message";

        // Produce valid partial signatures
        let mut partials: Vec<(u16, Vec<u8>)> = Vec::new();
        for i in 0..2u16 {
            let p = &mgr.participants[i as usize];
            let secret_bytes: [u8; 32] = p.secret_share.as_slice().try_into().unwrap();
            let signing_key = SigningKey::from_bytes(&secret_bytes);
            let sig = signing_key.sign(message);
            partials.push((i, sig.to_bytes().to_vec()));
        }

        assert_eq!(mgr.identify_abort(&partials, message), None);
    }

    #[test]
    fn test_identify_abort_invalid_sig() {
        let mgr = create_manager(2, 3);
        let message = b"test message";

        // Participant 0: valid signature
        let p0 = &mgr.participants[0];
        let secret_bytes: [u8; 32] = p0.secret_share.as_slice().try_into().unwrap();
        let signing_key = SigningKey::from_bytes(&secret_bytes);
        let sig0 = signing_key.sign(message);

        // Participant 1: garbage signature
        let bad_sig = vec![0xABu8; 64];

        let partials = vec![
            (0u16, sig0.to_bytes().to_vec()),
            (1u16, bad_sig),
        ];

        let abort = mgr.identify_abort(&partials, message);
        assert_eq!(abort, Some(1));
    }

    #[test]
    fn test_identify_abort_wrong_length() {
        let mgr = create_manager(2, 3);
        let partials = vec![(0u16, vec![0u8; 32])]; // 32 bytes instead of 64
        let abort = mgr.identify_abort(&partials, b"msg");
        assert_eq!(abort, Some(0));
    }

    #[test]
    fn test_identify_abort_out_of_range_index() {
        let mgr = create_manager(2, 3);
        let partials = vec![(99u16, vec![0u8; 64])];
        let abort = mgr.identify_abort(&partials, b"msg");
        assert_eq!(abort, Some(99));
    }

    #[test]
    fn test_deterministic_group_key() {
        // Two separate DKG runs should produce different group keys (random)
        let mgr1 = create_manager(2, 3);
        let mgr2 = create_manager(2, 3);
        assert_ne!(mgr1.group_public_key, mgr2.group_public_key);
    }

    #[test]
    fn test_threshold_1_of_1() {
        let mgr = create_manager(1, 1);
        let msg = b"single signer";
        let sig = mgr.sign(msg, &[0]).unwrap();
        assert!(mgr.verify(msg, &sig));
    }
}
