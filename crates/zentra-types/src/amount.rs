//! Amount type for ZTR values in zents (smallest unit).

use borsh::{BorshSerialize, BorshDeserialize};
use serde::{Serialize, Deserialize};
use std::fmt;
use crate::constants::COIN;

/// An amount of ZTR expressed in zents (the smallest indivisible unit).
/// 1 ZTR = 100,000,000 zents (10^8).
///
/// Uses u64 internally — sufficient for max supply of 5,000,000,000,000,000 zents
/// (u64 max is ~18.4 × 10^18).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default,
         BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Amount(pub u64);

impl Amount {
    /// Zero amount.
    pub const ZERO: Amount = Amount(0);

    /// One full ZTR coin in zents.
    pub const ONE_COIN: Amount = Amount(COIN);

    /// Create an Amount from zents.
    pub fn from_zents(zents: u64) -> Self {
        Amount(zents)
    }

    /// Create an Amount from whole coins (approximate — avoid in consensus code).
    pub fn from_coins(coins: u64) -> Self {
        Amount(coins.saturating_mul(COIN))
    }

    /// Get the value in zents.
    pub fn as_zents(&self) -> u64 {
        self.0
    }

    /// Get the whole coin part (integer division).
    pub fn whole_coins(&self) -> u64 {
        self.0 / COIN
    }

    /// Get the fractional zents (remainder after whole coins).
    pub fn fractional_zents(&self) -> u64 {
        self.0 % COIN
    }

    /// Checked addition — returns None on overflow.
    pub fn checked_add(&self, other: Amount) -> Option<Amount> {
        self.0.checked_add(other.0).map(Amount)
    }

    /// Checked subtraction — returns None on underflow.
    pub fn checked_sub(&self, other: Amount) -> Option<Amount> {
        self.0.checked_sub(other.0).map(Amount)
    }

    /// Checked multiplication — returns None on overflow.
    pub fn checked_mul(&self, factor: u64) -> Option<Amount> {
        self.0.checked_mul(factor).map(Amount)
    }

    /// Saturating addition — caps at u64::MAX instead of overflowing.
    pub fn saturating_add(&self, other: Amount) -> Amount {
        Amount(self.0.saturating_add(other.0))
    }

    /// Saturating subtraction — caps at 0 instead of underflowing.
    pub fn saturating_sub(&self, other: Amount) -> Amount {
        Amount(self.0.saturating_sub(other.0))
    }

    /// Check if the amount is zero.
    pub fn is_zero(&self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let whole = self.whole_coins();
        let frac = self.fractional_zents();
        if frac == 0 {
            write!(f, "{} ZTR", whole)
        } else {
            write!(f, "{}.{:08} ZTR", whole, frac)
        }
    }
}

impl fmt::Debug for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Amount({} zents = {})", self.0, self)
    }
}

impl std::ops::Add for Amount {
    type Output = Amount;
    fn add(self, rhs: Amount) -> Amount {
        Amount(self.0 + rhs.0)
    }
}

impl std::ops::Sub for Amount {
    type Output = Amount;
    fn sub(self, rhs: Amount) -> Amount {
        Amount(self.0 - rhs.0)
    }
}

impl std::ops::AddAssign for Amount {
    fn add_assign(&mut self, rhs: Amount) {
        self.0 += rhs.0;
    }
}

impl std::ops::SubAssign for Amount {
    fn sub_assign(&mut self, rhs: Amount) {
        self.0 -= rhs.0;
    }
}

impl From<u64> for Amount {
    fn from(zents: u64) -> Self {
        Amount(zents)
    }
}

impl From<Amount> for u64 {
    fn from(amount: Amount) -> u64 {
        amount.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_coin_conversion() {
        let one = Amount::from_coins(1);
        assert_eq!(one.as_zents(), 100_000_000);
        assert_eq!(one.whole_coins(), 1);
        assert_eq!(one.fractional_zents(), 0);
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", Amount::from_coins(42)), "42 ZTR");
        assert_eq!(format!("{}", Amount::from_zents(100_000_001)), "1.00000001 ZTR");
    }

    #[test]
    fn test_checked_arithmetic() {
        let a = Amount::from_coins(10);
        let b = Amount::from_coins(5);
        assert_eq!(a.checked_sub(b), Some(Amount::from_coins(5)));
        assert_eq!(b.checked_sub(a), None); // underflow
    }

    #[test]
    fn test_zero() {
        assert!(Amount::ZERO.is_zero());
        assert!(!Amount::from_zents(1).is_zero());
    }
}
