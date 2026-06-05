//! Merkle tree construction and verification using Blake2b-256.

use zentra_types::Hash;

/// Compute the Merkle root of a list of transaction hashes.
///
/// Uses a balanced binary tree with Blake2b-256 hashing.
/// If the number of leaves is odd, the last leaf is duplicated.
pub fn compute_merkle_root(tx_hashes: &[Hash]) -> Hash {
    if tx_hashes.is_empty() {
        return Hash::ZERO;
    }
    if tx_hashes.len() == 1 {
        return tx_hashes[0];
    }

    let mut current_level: Vec<Hash> = tx_hashes.to_vec();

    while current_level.len() > 1 {
        // If odd number, duplicate the last element
        if current_level.len() % 2 != 0 {
            let last = *current_level.last().unwrap();
            current_level.push(last);
        }

        let mut next_level = Vec::with_capacity(current_level.len() / 2);
        for chunk in current_level.chunks(2) {
            next_level.push(Hash::combine(&chunk[0], &chunk[1]));
        }
        current_level = next_level;
    }

    current_level[0]
}

/// Compute a Merkle proof for a specific transaction at the given index.
///
/// Returns a vector of (sibling_hash, is_right) pairs.
/// `is_right` indicates whether the sibling is on the right side.
pub fn compute_merkle_proof(tx_hashes: &[Hash], index: usize) -> Vec<(Hash, bool)> {
    if tx_hashes.len() <= 1 || index >= tx_hashes.len() {
        return vec![];
    }

    let mut proof = Vec::new();
    let mut current_level: Vec<Hash> = tx_hashes.to_vec();
    let mut idx = index;

    while current_level.len() > 1 {
        if current_level.len() % 2 != 0 {
            let last = *current_level.last().unwrap();
            current_level.push(last);
        }

        let sibling_idx = if idx % 2 == 0 { idx + 1 } else { idx - 1 };
        let is_right = idx % 2 == 0; // sibling is on the right if we're on the left
        proof.push((current_level[sibling_idx], is_right));

        // Move up to the next level
        let mut next_level = Vec::with_capacity(current_level.len() / 2);
        for chunk in current_level.chunks(2) {
            next_level.push(Hash::combine(&chunk[0], &chunk[1]));
        }
        current_level = next_level;
        idx /= 2;
    }

    proof
}

/// Verify a Merkle proof for a given transaction hash against a root.
pub fn verify_merkle_proof(root: &Hash, txid: &Hash, proof: &[(Hash, bool)]) -> bool {
    let mut current = *txid;

    for (sibling, is_right) in proof {
        if *is_right {
            current = Hash::combine(&current, sibling);
        } else {
            current = Hash::combine(sibling, &current);
        }
    }

    current == *root
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_merkle() {
        assert_eq!(compute_merkle_root(&[]), Hash::ZERO);
    }

    #[test]
    fn test_single_leaf() {
        let h = Hash::hash(b"tx0");
        assert_eq!(compute_merkle_root(&[h]), h);
    }

    #[test]
    fn test_two_leaves() {
        let h1 = Hash::hash(b"tx0");
        let h2 = Hash::hash(b"tx1");
        let root = compute_merkle_root(&[h1, h2]);
        assert_eq!(root, Hash::combine(&h1, &h2));
    }

    #[test]
    fn test_merkle_deterministic() {
        let hashes: Vec<Hash> = (0..8).map(|i| Hash::hash(format!("tx{}", i).as_bytes())).collect();
        assert_eq!(compute_merkle_root(&hashes), compute_merkle_root(&hashes));
    }

    #[test]
    fn test_merkle_proof_verification() {
        let hashes: Vec<Hash> = (0..8).map(|i| Hash::hash(format!("tx{}", i).as_bytes())).collect();
        let root = compute_merkle_root(&hashes);

        for i in 0..hashes.len() {
            let proof = compute_merkle_proof(&hashes, i);
            assert!(verify_merkle_proof(&root, &hashes[i], &proof), "proof failed for index {}", i);
        }
    }

    #[test]
    fn test_invalid_proof() {
        let hashes: Vec<Hash> = (0..4).map(|i| Hash::hash(format!("tx{}", i).as_bytes())).collect();
        let root = compute_merkle_root(&hashes);
        let proof = compute_merkle_proof(&hashes, 0);
        let fake_tx = Hash::hash(b"fake");
        assert!(!verify_merkle_proof(&root, &fake_tx, &proof));
    }

    #[test]
    fn test_odd_number_of_leaves() {
        let hashes: Vec<Hash> = (0..5).map(|i| Hash::hash(format!("tx{}", i).as_bytes())).collect();
        let root = compute_merkle_root(&hashes);
        assert!(!root.is_zero());

        // Verify all proofs still work
        for i in 0..hashes.len() {
            let proof = compute_merkle_proof(&hashes, i);
            assert!(verify_merkle_proof(&root, &hashes[i], &proof));
        }
    }
}
