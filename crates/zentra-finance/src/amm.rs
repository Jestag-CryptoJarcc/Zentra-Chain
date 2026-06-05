//! # Constant Product AMM (x × y = k)
//!
//! Implements the core automated market maker for the ZTR/zUSD trading pair.
//! Uses the constant product formula with a 0.2% swap fee (20 basis points).
//!
//! ## Fee Mechanism
//! - 0.2% of every swap is captured as a fee
//! - Fees are combined with gas fee ZTR for protocol liquidity injection
//! - LP tokens from protocol injection are TRUE BURNED (permanently destroyed)
//!
//! ## Integer Arithmetic
//! All math uses u128 integers — NO floating point anywhere.

use serde::{Serialize, Deserialize};
use thiserror::Error;
use zentra_types::{AMM_SWAP_FEE_BPS, BPS_DENOMINATOR};

/// Swap fee in basis points (0.2% = 20 bps).
const FEE_BPS: u128 = AMM_SWAP_FEE_BPS as u128;

/// Basis points denominator (10,000).
const BPS_DENOM: u128 = BPS_DENOMINATOR as u128;

/// Error type for AMM operations.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AmmError {
    /// The pool does not have enough reserves to fulfil the swap.
    #[error("insufficient liquidity: requested output exceeds reserves")]
    InsufficientLiquidity,

    /// An input amount of zero was provided.
    #[error("input amount must be non-zero")]
    ZeroAmount,

    /// The actual output would be below the caller's minimum acceptable amount.
    #[error("output amount {actual} is below slippage tolerance {minimum}")]
    SlippageExceeded {
        /// What the swap would produce.
        actual: u128,
        /// Caller's minimum acceptable output.
        minimum: u128,
    },

    /// Arithmetic overflow during calculation.
    #[error("arithmetic overflow in AMM calculation")]
    Overflow,
}

/// Result of a successful swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapResult {
    /// Net tokens received by the user after fee deduction.
    pub amount_out: u128,
    /// Fee captured from this swap (in the input token denomination).
    pub fee_captured: u128,
    /// Reserve of the input token after the swap.
    pub new_reserve_a: u128,
    /// Reserve of the output token after the swap.
    pub new_reserve_b: u128,
}

/// Constant-product liquidity pool for ZTR/zUSD.
///
/// Maintains the invariant `reserve_ztr * reserve_zusd = k` (modulo fees).
/// All reserves, LP token counts, and volume accumulators use u128 to prevent
/// overflow across the pool's lifetime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiquidityPool {
    /// ZTR reserves in zents (1 ZTR = 10^8 zents).
    pub reserve_ztr: u128,
    /// zUSD reserves in micro-units (1 zUSD = 10^6 micro-units).
    pub reserve_zusd: u128,
    /// Total LP tokens currently in circulation (not including burned ones).
    pub total_lp_tokens: u128,
    /// Cumulative LP tokens that have been permanently destroyed.
    pub total_lp_burned: u128,
    /// Lifetime trading volume denominated in ZTR zents.
    pub total_volume_ztr: u128,
    /// Lifetime fees captured denominated in the input token of each swap.
    pub total_fees_captured: u128,
}

impl LiquidityPool {
    /// Create a new empty liquidity pool with zero reserves.
    pub fn new() -> Self {
        tracing::info!("creating new empty liquidity pool");
        Self {
            reserve_ztr: 0,
            reserve_zusd: 0,
            total_lp_tokens: 0,
            total_lp_burned: 0,
            total_volume_ztr: 0,
            total_fees_captured: 0,
        }
    }

    /// Swap ZTR for zUSD using the constant product formula.
    ///
    /// Applies a 0.2% fee on the ZTR input before computing the output.
    ///
    /// # Errors
    /// - `AmmError::ZeroAmount` if `ztr_in` is zero
    /// - `AmmError::InsufficientLiquidity` if the pool has zero reserves
    /// - `AmmError::Overflow` if intermediate math overflows u128
    pub fn swap_ztr_to_zusd(&mut self, ztr_in: u128) -> Result<SwapResult, AmmError> {
        if ztr_in == 0 {
            return Err(AmmError::ZeroAmount);
        }
        if self.reserve_ztr == 0 || self.reserve_zusd == 0 {
            return Err(AmmError::InsufficientLiquidity);
        }

        // Fee = ztr_in * FEE_BPS / BPS_DENOM
        let fee = ztr_in
            .checked_mul(FEE_BPS)
            .ok_or(AmmError::Overflow)?
            / BPS_DENOM;
        let ztr_in_after_fee = ztr_in.checked_sub(fee).ok_or(AmmError::Overflow)?;

        // Constant product: zusd_out = reserve_zusd - (reserve_ztr * reserve_zusd) / (reserve_ztr + ztr_in_after_fee)
        use primitive_types::U256;
        let reserve_zusd_256 = U256::from(self.reserve_zusd);
        let ztr_in_after_fee_256 = U256::from(ztr_in_after_fee);
        let reserve_ztr_256 = U256::from(self.reserve_ztr);

        let numerator = reserve_zusd_256
            .checked_mul(ztr_in_after_fee_256)
            .ok_or(AmmError::Overflow)?;
        let denominator = reserve_ztr_256
            .checked_add(ztr_in_after_fee_256)
            .ok_or(AmmError::Overflow)?;
        let zusd_out_256 = numerator / denominator;
        let zusd_out = u128::try_from(zusd_out_256).map_err(|_| AmmError::Overflow)?;

        if zusd_out == 0 {
            return Err(AmmError::InsufficientLiquidity);
        }
        if zusd_out >= self.reserve_zusd {
            return Err(AmmError::InsufficientLiquidity);
        }

        // Update state
        self.reserve_ztr = self
            .reserve_ztr
            .checked_add(ztr_in)
            .ok_or(AmmError::Overflow)?;
        self.reserve_zusd = self
            .reserve_zusd
            .checked_sub(zusd_out)
            .ok_or(AmmError::Overflow)?;
        self.total_volume_ztr = self.total_volume_ztr.saturating_add(ztr_in);
        self.total_fees_captured = self.total_fees_captured.saturating_add(fee);

        tracing::debug!(
            ztr_in,
            zusd_out,
            fee,
            reserve_ztr = self.reserve_ztr,
            reserve_zusd = self.reserve_zusd,
            "swap ZTR -> zUSD executed"
        );

        Ok(SwapResult {
            amount_out: zusd_out,
            fee_captured: fee,
            new_reserve_a: self.reserve_ztr,
            new_reserve_b: self.reserve_zusd,
        })
    }

    /// Swap zUSD for ZTR using the constant product formula.
    ///
    /// Applies a 0.2% fee on the zUSD input before computing the output.
    ///
    /// # Errors
    /// - `AmmError::ZeroAmount` if `zusd_in` is zero
    /// - `AmmError::InsufficientLiquidity` if the pool has zero reserves
    /// - `AmmError::Overflow` if intermediate math overflows u128
    pub fn swap_zusd_to_ztr(&mut self, zusd_in: u128) -> Result<SwapResult, AmmError> {
        if zusd_in == 0 {
            return Err(AmmError::ZeroAmount);
        }
        if self.reserve_ztr == 0 || self.reserve_zusd == 0 {
            return Err(AmmError::InsufficientLiquidity);
        }

        // Fee = zusd_in * FEE_BPS / BPS_DENOM
        let fee = zusd_in
            .checked_mul(FEE_BPS)
            .ok_or(AmmError::Overflow)?
            / BPS_DENOM;
        let zusd_in_after_fee = zusd_in.checked_sub(fee).ok_or(AmmError::Overflow)?;

        // Constant product: ztr_out = reserve_ztr * zusd_in_after_fee / (reserve_zusd + zusd_in_after_fee)
        use primitive_types::U256;
        let reserve_ztr_256 = U256::from(self.reserve_ztr);
        let zusd_in_after_fee_256 = U256::from(zusd_in_after_fee);
        let reserve_zusd_256 = U256::from(self.reserve_zusd);

        let numerator = reserve_ztr_256
            .checked_mul(zusd_in_after_fee_256)
            .ok_or(AmmError::Overflow)?;
        let denominator = reserve_zusd_256
            .checked_add(zusd_in_after_fee_256)
            .ok_or(AmmError::Overflow)?;
        let ztr_out_256 = numerator / denominator;
        let ztr_out = u128::try_from(ztr_out_256).map_err(|_| AmmError::Overflow)?;

        if ztr_out == 0 {
            return Err(AmmError::InsufficientLiquidity);
        }
        if ztr_out >= self.reserve_ztr {
            return Err(AmmError::InsufficientLiquidity);
        }

        // Update state
        self.reserve_zusd = self
            .reserve_zusd
            .checked_add(zusd_in)
            .ok_or(AmmError::Overflow)?;
        self.reserve_ztr = self
            .reserve_ztr
            .checked_sub(ztr_out)
            .ok_or(AmmError::Overflow)?;
        // Track volume in ZTR-equivalent
        self.total_volume_ztr = self.total_volume_ztr.saturating_add(ztr_out);
        self.total_fees_captured = self.total_fees_captured.saturating_add(fee);

        tracing::debug!(
            zusd_in,
            ztr_out,
            fee,
            reserve_ztr = self.reserve_ztr,
            reserve_zusd = self.reserve_zusd,
            "swap zUSD -> ZTR executed"
        );

        Ok(SwapResult {
            amount_out: ztr_out,
            fee_captured: fee,
            new_reserve_a: self.reserve_zusd,
            new_reserve_b: self.reserve_ztr,
        })
    }

    /// Add liquidity to the pool.
    ///
    /// If the pool is empty, the initial LP token count equals `sqrt(ztr * zusd)`.
    /// Otherwise LP tokens are minted proportionally to the smaller ratio of the
    /// two deposit sides vs existing reserves.
    ///
    /// # Returns
    /// The number of LP tokens minted.
    ///
    /// # Errors
    /// - `AmmError::ZeroAmount` if either amount is zero
    /// - `AmmError::Overflow` if intermediate math overflows
    pub fn add_liquidity(
        &mut self,
        ztr_amount: u128,
        zusd_amount: u128,
    ) -> Result<u128, AmmError> {
        if ztr_amount == 0 || zusd_amount == 0 {
            return Err(AmmError::ZeroAmount);
        }

        let lp_tokens = if self.total_lp_tokens == 0 {
            // First liquidity provision — geometric mean
            let product = ztr_amount
                .checked_mul(zusd_amount)
                .ok_or(AmmError::Overflow)?;
            integer_sqrt(product)
        } else {
            // Mint proportional to the lesser side
            let lp_from_ztr = ztr_amount
                .checked_mul(self.total_lp_tokens)
                .ok_or(AmmError::Overflow)?
                / self.reserve_ztr;
            let lp_from_zusd = zusd_amount
                .checked_mul(self.total_lp_tokens)
                .ok_or(AmmError::Overflow)?
                / self.reserve_zusd;
            lp_from_ztr.min(lp_from_zusd)
        };

        if lp_tokens == 0 {
            return Err(AmmError::InsufficientLiquidity);
        }

        self.reserve_ztr = self
            .reserve_ztr
            .checked_add(ztr_amount)
            .ok_or(AmmError::Overflow)?;
        self.reserve_zusd = self
            .reserve_zusd
            .checked_add(zusd_amount)
            .ok_or(AmmError::Overflow)?;
        self.total_lp_tokens = self
            .total_lp_tokens
            .checked_add(lp_tokens)
            .ok_or(AmmError::Overflow)?;

        tracing::info!(
            ztr_amount,
            zusd_amount,
            lp_tokens,
            total_lp = self.total_lp_tokens,
            "liquidity added to pool"
        );

        Ok(lp_tokens)
    }

    /// Get the current price as a rational number `(ztr_per_zusd_numerator, denominator)`.
    ///
    /// Price = reserve_ztr / reserve_zusd. Returns `(0, 1)` if the pool is empty.
    pub fn get_price(&self) -> (u128, u128) {
        if self.reserve_zusd == 0 {
            return (0, 1);
        }
        (self.reserve_ztr, self.reserve_zusd)
    }

    /// Get the current reserves `(reserve_ztr, reserve_zusd)`.
    pub fn get_reserves(&self) -> (u128, u128) {
        (self.reserve_ztr, self.reserve_zusd)
    }

    /// Inject protocol-owned liquidity into the pool.
    ///
    /// LP tokens minted from this injection are immediately and permanently
    /// destroyed (TRUE BURN). They are added to `total_lp_burned` and are
    /// **not** added to `total_lp_tokens`, ensuring the liquidity can never
    /// be withdrawn.
    ///
    /// # Returns
    /// The number of LP tokens that were burned.
    pub fn inject_protocol_liquidity(
        &mut self,
        ztr_amount: u128,
        zusd_amount: u128,
    ) -> u128 {
        if ztr_amount == 0 && zusd_amount == 0 {
            return 0;
        }

        let lp_tokens = if self.total_lp_tokens == 0 && self.reserve_ztr == 0 {
            // Bootstrap: geometric mean
            let product = ztr_amount.saturating_mul(zusd_amount);
            integer_sqrt(product)
        } else if self.total_lp_tokens == 0 {
            // Edge case: all LP tokens have been burned previously but reserves remain.
            // Mint based on the proportion of new liquidity to existing reserves.
            // Use a synthetic baseline of 1 LP token per unit to bootstrap.
            integer_sqrt(ztr_amount.saturating_mul(zusd_amount))
        } else {
            let lp_from_ztr = if self.reserve_ztr > 0 {
                ztr_amount.saturating_mul(self.total_lp_tokens) / self.reserve_ztr
            } else {
                0
            };
            let lp_from_zusd = if self.reserve_zusd > 0 {
                zusd_amount.saturating_mul(self.total_lp_tokens) / self.reserve_zusd
            } else {
                0
            };
            if lp_from_ztr == 0 && lp_from_zusd == 0 {
                0
            } else if lp_from_ztr == 0 {
                lp_from_zusd
            } else if lp_from_zusd == 0 {
                lp_from_ztr
            } else {
                lp_from_ztr.min(lp_from_zusd)
            }
        };

        // Add to reserves
        self.reserve_ztr = self.reserve_ztr.saturating_add(ztr_amount);
        self.reserve_zusd = self.reserve_zusd.saturating_add(zusd_amount);

        // TRUE BURN: LP tokens are destroyed immediately.
        // They never enter circulation — only the burned counter is incremented.
        self.total_lp_burned = self.total_lp_burned.saturating_add(lp_tokens);

        tracing::info!(
            ztr_amount,
            zusd_amount,
            lp_tokens_burned = lp_tokens,
            total_lp_burned = self.total_lp_burned,
            "protocol liquidity injected — LP tokens TRUE BURNED"
        );

        lp_tokens
    }
}

impl Default for LiquidityPool {
    fn default() -> Self {
        Self::new()
    }
}

/// Integer square root via Newton's method (Babylonian method).
///
/// Returns `floor(sqrt(n))` using only integer arithmetic.
fn integer_sqrt(n: u128) -> u128 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_pool_is_empty() {
        let pool = LiquidityPool::new();
        assert_eq!(pool.reserve_ztr, 0);
        assert_eq!(pool.reserve_zusd, 0);
        assert_eq!(pool.total_lp_tokens, 0);
        assert_eq!(pool.total_lp_burned, 0);
    }

    #[test]
    fn test_add_initial_liquidity() {
        let mut pool = LiquidityPool::new();
        // 100 ZTR (in zents) + 100 zUSD (in micro-units)
        let ztr = 100 * 100_000_000u128; // 10 billion zents
        let zusd = 100 * 1_000_000u128; // 100 million micro-units
        let lp = pool.add_liquidity(ztr, zusd).unwrap();
        assert!(lp > 0);
        assert_eq!(pool.reserve_ztr, ztr);
        assert_eq!(pool.reserve_zusd, zusd);
        assert_eq!(pool.total_lp_tokens, lp);
    }

    #[test]
    fn test_add_liquidity_zero_fails() {
        let mut pool = LiquidityPool::new();
        assert_eq!(pool.add_liquidity(0, 100), Err(AmmError::ZeroAmount));
        assert_eq!(pool.add_liquidity(100, 0), Err(AmmError::ZeroAmount));
    }

    #[test]
    fn test_swap_ztr_to_zusd() {
        let mut pool = LiquidityPool::new();
        let ztr_reserve = 1_000_000_000_000u128; // 10,000 ZTR
        let zusd_reserve = 10_000_000_000u128; // 10,000 zUSD
        pool.add_liquidity(ztr_reserve, zusd_reserve).unwrap();

        let swap_in = 100_000_000u128; // 1 ZTR
        let result = pool.swap_ztr_to_zusd(swap_in).unwrap();

        // Must receive some zUSD
        assert!(result.amount_out > 0);
        // Fee should be 0.2% of input
        let expected_fee = swap_in * 20 / 10_000;
        assert_eq!(result.fee_captured, expected_fee);
        // Reserves should be updated
        assert_eq!(result.new_reserve_a, pool.reserve_ztr);
        assert_eq!(result.new_reserve_b, pool.reserve_zusd);
    }

    #[test]
    fn test_swap_zusd_to_ztr() {
        let mut pool = LiquidityPool::new();
        let ztr_reserve = 1_000_000_000_000u128;
        let zusd_reserve = 10_000_000_000u128;
        pool.add_liquidity(ztr_reserve, zusd_reserve).unwrap();

        let swap_in = 1_000_000u128; // 1 zUSD
        let result = pool.swap_zusd_to_ztr(swap_in).unwrap();

        assert!(result.amount_out > 0);
        let expected_fee = swap_in * 20 / 10_000;
        assert_eq!(result.fee_captured, expected_fee);
    }

    #[test]
    fn test_swap_on_empty_pool_fails() {
        let mut pool = LiquidityPool::new();
        assert_eq!(
            pool.swap_ztr_to_zusd(100),
            Err(AmmError::InsufficientLiquidity)
        );
        assert_eq!(
            pool.swap_zusd_to_ztr(100),
            Err(AmmError::InsufficientLiquidity)
        );
    }

    #[test]
    fn test_swap_zero_fails() {
        let mut pool = LiquidityPool::new();
        pool.add_liquidity(1_000_000, 1_000_000).unwrap();
        assert_eq!(pool.swap_ztr_to_zusd(0), Err(AmmError::ZeroAmount));
        assert_eq!(pool.swap_zusd_to_ztr(0), Err(AmmError::ZeroAmount));
    }

    #[test]
    fn test_constant_product_invariant() {
        let mut pool = LiquidityPool::new();
        let ztr_reserve = 1_000_000_000_000u128;
        let zusd_reserve = 10_000_000_000u128;
        pool.add_liquidity(ztr_reserve, zusd_reserve).unwrap();

        let k_before = pool.reserve_ztr * pool.reserve_zusd;

        // After a swap, k should increase or stay the same (fees increase k)
        pool.swap_ztr_to_zusd(100_000_000).unwrap();
        let k_after = pool.reserve_ztr * pool.reserve_zusd;
        assert!(k_after >= k_before, "k must not decrease after a fee-inclusive swap");
    }

    #[test]
    fn test_inject_protocol_liquidity_burns_lp() {
        let mut pool = LiquidityPool::new();
        pool.add_liquidity(1_000_000_000, 1_000_000).unwrap();
        let lp_before = pool.total_lp_tokens;

        let burned = pool.inject_protocol_liquidity(500_000_000, 500_000);
        assert!(burned > 0);
        // LP tokens from injection should NOT increase total_lp_tokens
        assert_eq!(pool.total_lp_tokens, lp_before);
        // But burned counter should increase
        assert_eq!(pool.total_lp_burned, burned);
    }

    #[test]
    fn test_inject_protocol_liquidity_bootstrap() {
        let mut pool = LiquidityPool::new();
        let burned = pool.inject_protocol_liquidity(1_000_000_000, 1_000_000);
        assert!(burned > 0);
        assert_eq!(pool.total_lp_tokens, 0); // No circulating LP
        assert_eq!(pool.total_lp_burned, burned);
        assert_eq!(pool.reserve_ztr, 1_000_000_000);
        assert_eq!(pool.reserve_zusd, 1_000_000);
    }

    #[test]
    fn test_get_price_empty_pool() {
        let pool = LiquidityPool::new();
        assert_eq!(pool.get_price(), (0, 1));
    }

    #[test]
    fn test_get_price_with_liquidity() {
        let mut pool = LiquidityPool::new();
        pool.add_liquidity(2_000_000, 1_000_000).unwrap();
        let (num, den) = pool.get_price();
        // Price should be 2:1
        assert_eq!(num, 2_000_000);
        assert_eq!(den, 1_000_000);
    }

    #[test]
    fn test_get_reserves() {
        let mut pool = LiquidityPool::new();
        pool.add_liquidity(500, 300).unwrap();
        assert_eq!(pool.get_reserves(), (500, 300));
    }

    #[test]
    fn test_integer_sqrt() {
        assert_eq!(integer_sqrt(0), 0);
        assert_eq!(integer_sqrt(1), 1);
        assert_eq!(integer_sqrt(4), 2);
        assert_eq!(integer_sqrt(9), 3);
        assert_eq!(integer_sqrt(10), 3); // floor
        assert_eq!(integer_sqrt(100), 10);
        assert_eq!(integer_sqrt(1_000_000), 1_000);
    }

    #[test]
    fn test_volume_tracking() {
        let mut pool = LiquidityPool::new();
        pool.add_liquidity(1_000_000_000_000, 10_000_000_000).unwrap();

        let swap_amount = 100_000_000u128;
        pool.swap_ztr_to_zusd(swap_amount).unwrap();
        assert_eq!(pool.total_volume_ztr, swap_amount);

        pool.swap_ztr_to_zusd(swap_amount).unwrap();
        assert_eq!(pool.total_volume_ztr, swap_amount * 2);
    }

    #[test]
    fn test_fee_tracking() {
        let mut pool = LiquidityPool::new();
        pool.add_liquidity(1_000_000_000_000, 10_000_000_000).unwrap();

        let swap_amount = 10_000_000u128;
        let result = pool.swap_ztr_to_zusd(swap_amount).unwrap();
        assert_eq!(pool.total_fees_captured, result.fee_captured);

        let result2 = pool.swap_zusd_to_ztr(1_000_000u128).unwrap();
        assert_eq!(
            pool.total_fees_captured,
            result.fee_captured + result2.fee_captured
        );
    }

    #[test]
    fn test_proportional_liquidity_addition() {
        let mut pool = LiquidityPool::new();
        let lp1 = pool.add_liquidity(1_000_000, 2_000_000).unwrap();

        // Adding same ratio should mint same LP tokens
        let lp2 = pool.add_liquidity(1_000_000, 2_000_000).unwrap();
        assert_eq!(lp1, lp2);
    }

    #[test]
    fn test_large_swap_does_not_overflow() {
        let mut pool = LiquidityPool::new();
        pool.reserve_ztr = u128::MAX / 4;
        pool.reserve_zusd = u128::MAX / 4;
        pool.total_lp_tokens = 1_000_000;

        // A small swap should still work without overflow
        let result = pool.swap_ztr_to_zusd(1_000_000);
        assert!(result.is_ok());
    }

    #[test]
    fn test_swap_cannot_drain_pool() {
        let mut pool = LiquidityPool::new();
        pool.add_liquidity(1_000, 1_000).unwrap();

        // Trying to swap more than the pool has should not drain it
        // The constant product formula naturally limits output
        let result = pool.swap_ztr_to_zusd(1_000_000_000);
        assert!(result.is_ok());
        // Output should be less than total reserves
        let r = result.unwrap();
        assert!(r.amount_out < 1_000);
    }
}
