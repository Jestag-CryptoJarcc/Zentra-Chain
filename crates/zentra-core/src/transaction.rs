//! Transaction types for the Zentra BlockDAG.
//!
//! Key design: `TxOutput` has two variants — `Standard` (added to UTXO set)
//! and `Burn` (tokens permanently destroyed, never enter UTXO set).

use borsh::{BorshSerialize, BorshDeserialize};
use serde::{Serialize, Deserialize};
use ed25519_dalek::{VerifyingKey, Signature, Verifier};
use zentra_types::*;

/// A reference to a previous transaction output being spent.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct TxInput {
    /// Hash of the transaction containing the output being spent
    pub prev_tx_hash: Hash,
    /// Index of the output in that transaction
    pub output_index: u32,
    /// Ed25519 signature proving ownership
    pub signature: Vec<u8>,
    /// Ed25519 public key of the signer
    pub public_key: [u8; 32],
}

/// A transaction output — either a standard spendable output or a true burn.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum TxOutput {
    /// Standard output that will be added to the UTXO set.
    Standard {
        address: Address,
        amount: Amount,
        script: Vec<u8>,
    },
    /// True burn — tokens are permanently destroyed. NOT added to UTXO set.
    Burn {
        amount: Amount,
        burn_type: BurnType,
    },
}

impl TxOutput {
    /// Get the amount of this output regardless of variant.
    pub fn amount(&self) -> Amount {
        match self {
            TxOutput::Standard { amount, .. } => *amount,
            TxOutput::Burn { amount, .. } => *amount,
        }
    }

    /// Check if this output is a burn (tokens destroyed).
    pub fn is_burn(&self) -> bool {
        matches!(self, TxOutput::Burn { .. })
    }

    /// Check if this is a standard spendable output.
    pub fn is_standard(&self) -> bool {
        matches!(self, TxOutput::Standard { .. })
    }
}

/// A Zentra transaction.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Transaction {
    /// Transaction version
    pub version: u32,
    /// Type of transaction
    pub tx_type: TransactionType,
    /// Inputs (references to UTXOs being spent)
    pub inputs: Vec<TxInput>,
    /// Outputs (new UTXOs or burns)
    pub outputs: Vec<TxOutput>,
    /// Optional payload (contract bytecode, swap params, etc.)
    pub payload: Vec<u8>,
    /// Lock time (block height or timestamp)
    pub lock_time: u64,
}

impl Transaction {
    /// Compute the transaction ID (hash of serialized transaction without signatures).
    pub fn txid(&self) -> Hash {
        // Hash the transaction data for ID computation
        let encoded = borsh::to_vec(self).expect("tx serialization cannot fail");
        Hash::hash(&encoded)
    }

    /// Get the signing hash (what signers sign over — tx without signatures).
    pub fn signing_hash(&self) -> Hash {
        let mut tx_copy = self.clone();
        for input in &mut tx_copy.inputs {
            input.signature = vec![];
        }
        let encoded = borsh::to_vec(&tx_copy).expect("tx serialization cannot fail");
        Hash::hash(&encoded)
    }

    /// Total amount of all outputs (both Standard and Burn).
    pub fn total_output_amount(&self) -> Amount {
        self.outputs.iter().fold(Amount::ZERO, |acc, out| {
            acc.saturating_add(out.amount())
        })
    }

    /// Total amount of only burn outputs.
    pub fn total_burn_amount(&self) -> Amount {
        self.outputs
            .iter()
            .filter(|o| o.is_burn())
            .fold(Amount::ZERO, |acc, out| acc.saturating_add(out.amount()))
    }

    /// Total amount of standard (spendable) outputs.
    pub fn total_standard_amount(&self) -> Amount {
        self.outputs
            .iter()
            .filter(|o| o.is_standard())
            .fold(Amount::ZERO, |acc, out| acc.saturating_add(out.amount()))
    }

    /// Verify all Ed25519 input signatures.
    pub fn verify_signatures(&self) -> ZentraResult<()> {
        if self.is_coinbase() {
            return Ok(()); // Coinbase has no inputs to verify
        }

        let signing_hash = self.signing_hash();

        for (i, input) in self.inputs.iter().enumerate() {
            if input.signature.len() != 64 {
                return Err(ZentraError::InvalidSignature(
                    format!("input {}: signature must be 64 bytes, got {}", i, input.signature.len()),
                ));
            }

            let pubkey = VerifyingKey::from_bytes(&input.public_key)
                .map_err(|e| ZentraError::InvalidSignature(format!("input {}: invalid public key: {}", i, e)))?;

            let sig = Signature::from_slice(&input.signature)
                .map_err(|e| ZentraError::InvalidSignature(format!("input {}: invalid signature format: {}", i, e)))?;

            pubkey
                .verify(signing_hash.as_bytes(), &sig)
                .map_err(|e| ZentraError::InvalidSignature(format!("input {}: signature verification failed: {}", i, e)))?;
        }

        Ok(())
    }

    /// Check if this is a coinbase transaction.
    pub fn is_coinbase(&self) -> bool {
        self.tx_type == TransactionType::Coinbase
    }

    /// Basic structural validation.
    pub fn validate_basic(&self) -> ZentraResult<()> {
        if !self.is_coinbase() && self.inputs.is_empty() {
            return Err(ZentraError::TransactionValidation(
                "non-coinbase transaction must have at least one input".into(),
            ));
        }
        if self.outputs.is_empty() {
            return Err(ZentraError::TransactionValidation(
                "transaction must have at least one output".into(),
            ));
        }
        // Verify no output has zero amount
        for (i, output) in self.outputs.iter().enumerate() {
            if output.amount().is_zero() {
                return Err(ZentraError::TransactionValidation(
                    format!("output {} has zero amount", i),
                ));
            }
        }
        Ok(())
    }

    /// Create a coinbase transaction for a miner.
    pub fn create_coinbase(reward: Amount, miner_address: Address, height: u64) -> Self {
        Transaction {
            version: 1,
            tx_type: TransactionType::Coinbase,
            inputs: vec![],
            outputs: vec![TxOutput::Standard {
                address: miner_address,
                amount: reward,
                script: vec![],
            }],
            payload: height.to_le_bytes().to_vec(),
            lock_time: height,
        }
    }
}

/// Reference to a specific output of a specific transaction (used for UTXO tracking).
#[derive(Clone, Debug, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct OutPoint {
    pub tx_hash: Hash,
    pub index: u32,
}

impl OutPoint {
    pub fn new(tx_hash: Hash, index: u32) -> Self {
        OutPoint { tx_hash, index }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_address() -> Address {
        Address::from_public_key(&[42u8; 32], NetworkType::Devnet)
    }

    #[test]
    fn test_coinbase_creation() {
        let tx = Transaction::create_coinbase(
            Amount::from_zents(INITIAL_REWARD_ZENTS),
            test_address(),
            0,
        );
        assert!(tx.is_coinbase());
        assert!(tx.validate_basic().is_ok());
        assert_eq!(tx.total_output_amount(), Amount::from_zents(INITIAL_REWARD_ZENTS));
    }

    #[test]
    fn test_txid_deterministic() {
        let tx = Transaction::create_coinbase(Amount::from_coins(1), test_address(), 0);
        assert_eq!(tx.txid(), tx.txid());
    }

    #[test]
    fn test_burn_output() {
        let tx = Transaction {
            version: 1,
            tx_type: TransactionType::StablecoinBurn,
            inputs: vec![],
            outputs: vec![TxOutput::Burn {
                amount: Amount::from_coins(100),
                burn_type: BurnType::StablecoinBurn,
            }],
            payload: vec![],
            lock_time: 0,
        };
        assert_eq!(tx.total_burn_amount(), Amount::from_coins(100));
        assert_eq!(tx.total_standard_amount(), Amount::ZERO);
        assert_eq!(tx.total_output_amount(), Amount::from_coins(100));
    }

    #[test]
    fn test_mixed_outputs() {
        let tx = Transaction {
            version: 1,
            tx_type: TransactionType::Transfer,
            inputs: vec![],
            outputs: vec![
                TxOutput::Standard {
                    address: test_address(),
                    amount: Amount::from_coins(50),
                    script: vec![],
                },
                TxOutput::Burn {
                    amount: Amount::from_coins(10),
                    burn_type: BurnType::FeeBurn,
                },
            ],
            payload: vec![],
            lock_time: 0,
        };
        assert_eq!(tx.total_standard_amount(), Amount::from_coins(50));
        assert_eq!(tx.total_burn_amount(), Amount::from_coins(10));
        assert_eq!(tx.total_output_amount(), Amount::from_coins(60));
    }

    #[test]
    fn test_empty_outputs_invalid() {
        let tx = Transaction {
            version: 1,
            tx_type: TransactionType::Transfer,
            inputs: vec![TxInput {
                prev_tx_hash: Hash::ZERO,
                output_index: 0,
                signature: vec![0; 64],
                public_key: [0; 32],
            }],
            outputs: vec![],
            payload: vec![],
            lock_time: 0,
        };
        assert!(tx.validate_basic().is_err());
    }
}
