//! Transaction type enumeration for all Zentra transaction kinds.

use borsh::{BorshSerialize, BorshDeserialize};
use serde::{Serialize, Deserialize};
use std::fmt;

/// The type of a transaction, determining how it is processed by the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
#[repr(u8)]
#[borsh(use_discriminant = true)]
pub enum TransactionType {
    /// Standard value transfer between addresses
    Transfer = 0,
    /// Coinbase (block reward) transaction — created by miners
    Coinbase = 1,
    /// DEX swap (ZTR <-> zUSD) through the embedded AMM
    DexSwap = 2,
    /// Add liquidity to the AMM pool
    DexAddLiquidity = 3,
    /// Cross-chain asset ingest (minting zUSD from external deposit)
    CrossChainIngest = 4,
    /// Deploy a new Wasm smart contract
    ContractDeploy = 5,
    /// Call an existing Wasm smart contract
    ContractCall = 6,
    /// Quarantine action (lock a malicious node's collateral)
    Quarantine = 7,
    /// TSS key generation ceremony participation
    TssKeyGen = 8,
    /// TSS signing ceremony participation
    TssSign = 9,
    /// Governance parameter update
    GovernanceUpdate = 10,
    /// True burn of zUSD — permanently removes tokens from the chain
    StablecoinBurn = 11,
    /// True burn of LP tokens — permanent protocol-owned liquidity
    LpBurn = 12,
}

impl TransactionType {
    /// Convert from u8.
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(TransactionType::Transfer),
            1 => Some(TransactionType::Coinbase),
            2 => Some(TransactionType::DexSwap),
            3 => Some(TransactionType::DexAddLiquidity),
            4 => Some(TransactionType::CrossChainIngest),
            5 => Some(TransactionType::ContractDeploy),
            6 => Some(TransactionType::ContractCall),
            7 => Some(TransactionType::Quarantine),
            8 => Some(TransactionType::TssKeyGen),
            9 => Some(TransactionType::TssSign),
            10 => Some(TransactionType::GovernanceUpdate),
            11 => Some(TransactionType::StablecoinBurn),
            12 => Some(TransactionType::LpBurn),
            _ => None,
        }
    }

    /// Whether this transaction type requires a fee.
    pub fn requires_fee(&self) -> bool {
        match self {
            TransactionType::Coinbase => false, // Coinbase creates coins, no fee
            _ => true,
        }
    }

    /// Whether this transaction type modifies the AMM state.
    pub fn modifies_amm(&self) -> bool {
        matches!(self, TransactionType::DexSwap | TransactionType::DexAddLiquidity | TransactionType::CrossChainIngest)
    }

    /// Whether this transaction type burns (permanently destroys) tokens.
    pub fn is_burn(&self) -> bool {
        matches!(self, TransactionType::StablecoinBurn | TransactionType::LpBurn)
    }
}

impl fmt::Display for TransactionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransactionType::Transfer => write!(f, "Transfer"),
            TransactionType::Coinbase => write!(f, "Coinbase"),
            TransactionType::DexSwap => write!(f, "DEX Swap"),
            TransactionType::DexAddLiquidity => write!(f, "DEX Add Liquidity"),
            TransactionType::CrossChainIngest => write!(f, "Cross-Chain Ingest"),
            TransactionType::ContractDeploy => write!(f, "Contract Deploy"),
            TransactionType::ContractCall => write!(f, "Contract Call"),
            TransactionType::Quarantine => write!(f, "Quarantine"),
            TransactionType::TssKeyGen => write!(f, "TSS KeyGen"),
            TransactionType::TssSign => write!(f, "TSS Sign"),
            TransactionType::GovernanceUpdate => write!(f, "Governance Update"),
            TransactionType::StablecoinBurn => write!(f, "zUSD Burn"),
            TransactionType::LpBurn => write!(f, "LP Token Burn"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_u8() {
        assert_eq!(TransactionType::from_u8(0), Some(TransactionType::Transfer));
        assert_eq!(TransactionType::from_u8(7), Some(TransactionType::Quarantine));
        assert_eq!(TransactionType::from_u8(99), None);
    }

    #[test]
    fn test_requires_fee() {
        assert!(!TransactionType::Coinbase.requires_fee());
        assert!(TransactionType::Transfer.requires_fee());
        assert!(TransactionType::DexSwap.requires_fee());
    }

    #[test]
    fn test_modifies_amm() {
        assert!(TransactionType::DexSwap.modifies_amm());
        assert!(TransactionType::CrossChainIngest.modifies_amm());
        assert!(!TransactionType::Transfer.modifies_amm());
    }
}
