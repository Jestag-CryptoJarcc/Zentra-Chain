//! # Protocol-Owned Liquidity (POL) Engine
//!
//! Manages protocol-owned liquidity for the ZTR/zUSD AMM pool.
//!
//! ## Revenue Streams
//! - **Cross-chain ingest fees**: 0.5% of all inbound stablecoin deposits
//! - **Swap fees**: 0.2% of every AMM swap
//!
//! ## TRUE BURN Mechanism
//! All LP tokens generated from protocol liquidity injection are immediately and
//! permanently destroyed. They are never sent to a dead address — they simply
//! cease to exist. This ensures the protocol's liquidity is permanent and
//! irrevocable.

use serde::{Serialize, Deserialize};
use zentra_types::{CROSS_CHAIN_INGEST_FEE_BPS, BPS_DENOMINATOR};

use crate::amm::LiquidityPool;

/// Fee in basis points for cross-chain ingests (0.5% = 50 bps).
const INGEST_FEE_BPS: u128 = CROSS_CHAIN_INGEST_FEE_BPS as u128;

/// Basis points denominator.
const BPS_DENOM: u128 = BPS_DENOMINATOR as u128;

/// Result of processing a cross-chain stablecoin ingest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestResult {
    /// Net zUSD amount credited to the user after fee deduction.
    pub net_amount: u128,
    /// Fee deducted from the ingest (0.5% of the gross amount).
    pub fee_deducted: u128,
    /// LP tokens that were TRUE BURNED from the fee injection.
    pub lp_tokens_burned: u128,
}

/// Aggregate metrics for the protocol-owned liquidity engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolMetrics {
    /// Total ZTR reserves held in the AMM pool.
    pub total_reserves_ztr: u128,
    /// Total zUSD reserves held in the AMM pool.
    pub total_reserves_zusd: u128,
    /// Total LP tokens that have been permanently destroyed.
    pub total_lp_burned: u128,
    /// Total fees collected (ingest fees + swap fees combined).
    pub total_fees_collected: u128,
}

/// Protocol-Owned Liquidity engine.
///
/// Wraps a [`LiquidityPool`] and adds protocol revenue collection logic.
/// All fees collected are injected back into the pool as permanent (burned-LP)
/// liquidity, strengthening the ZTR/zUSD market.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolOwnedLiquidity {
    /// The underlying AMM liquidity pool.
    pub pool: LiquidityPool,
    /// Cumulative 0.5% fees from cross-chain stablecoin ingests (in zUSD micro-units).
    pub total_ingest_fees_collected: u128,
    /// Cumulative 0.2% swap fees (in input-token denomination).
    pub total_swap_fees_collected: u128,
    /// Total LP tokens that have been truly destroyed by the POL engine.
    pub total_lp_tokens_burned: u128,
}

impl ProtocolOwnedLiquidity {
    /// Create a new POL engine with an empty AMM pool.
    pub fn new() -> Self {
        tracing::info!("initializing protocol-owned liquidity engine");
        Self {
            pool: LiquidityPool::new(),
            total_ingest_fees_collected: 0,
            total_swap_fees_collected: 0,
            total_lp_tokens_burned: 0,
        }
    }

    /// Process an inbound cross-chain stablecoin deposit.
    ///
    /// 1. Deducts 0.5% of the inbound zUSD as a fee.
    /// 2. Injects the fee into the AMM pool as zUSD-only protocol liquidity.
    /// 3. LP tokens from the injection are TRUE BURNED (permanently destroyed).
    /// 4. Returns the net zUSD amount credited to the depositor.
    ///
    /// The fee-to-liquidity injection uses only the zUSD side. In a real
    /// deployment, the protocol would pair this with ZTR from gas fees to
    /// balance both sides. For the single-sided injection, the pool absorbs
    /// the zUSD with a proportional LP burn.
    pub fn process_cross_chain_ingest(&mut self, zusd_amount: u128) -> IngestResult {
        if zusd_amount == 0 {
            return IngestResult {
                net_amount: 0,
                fee_deducted: 0,
                lp_tokens_burned: 0,
            };
        }

        // Deduct the 0.5% ingest fee
        let fee = zusd_amount * INGEST_FEE_BPS / BPS_DENOM;
        let net = zusd_amount.saturating_sub(fee);

        // Inject the fee into the pool as protocol-owned liquidity.
        // We inject zUSD only; in production the protocol would pair with ZTR.
        // LP tokens minted here are immediately burned.
        let lp_burned = self.pool.inject_protocol_liquidity(0, fee);

        // Update cumulative counters
        self.total_ingest_fees_collected = self.total_ingest_fees_collected.saturating_add(fee);
        self.total_lp_tokens_burned = self.total_lp_tokens_burned.saturating_add(lp_burned);

        tracing::info!(
            zusd_amount,
            fee,
            net,
            lp_burned,
            "cross-chain ingest processed — fee TRUE BURNED into pool"
        );

        IngestResult {
            net_amount: net,
            fee_deducted: fee,
            lp_tokens_burned: lp_burned,
        }
    }

    /// Process a swap fee and inject it as permanent protocol liquidity.
    ///
    /// Takes the 0.2% fee captured from a swap (in ZTR) and an equivalent
    /// amount of ZTR sourced from gas fees, then injects both sides into
    /// the pool. The resulting LP tokens are TRUE BURNED.
    ///
    /// # Arguments
    /// - `fee_ztr`: The 0.2% fee captured from the swap, denominated in ZTR zents.
    /// - `equivalent_zusd`: An equivalent zUSD amount to pair with the ZTR fee
    ///   for balanced liquidity injection.
    pub fn process_swap_fee(&mut self, fee_ztr: u128, equivalent_zusd: u128) {
        if fee_ztr == 0 && equivalent_zusd == 0 {
            return;
        }

        let lp_burned = self
            .pool
            .inject_protocol_liquidity(fee_ztr, equivalent_zusd);

        self.total_swap_fees_collected = self.total_swap_fees_collected.saturating_add(fee_ztr);
        self.total_lp_tokens_burned = self.total_lp_tokens_burned.saturating_add(lp_burned);

        tracing::info!(
            fee_ztr,
            equivalent_zusd,
            lp_burned,
            "swap fee injected as POL — LP tokens TRUE BURNED"
        );
    }

    /// Get a snapshot of all POL metrics.
    pub fn get_pol_metrics(&self) -> PolMetrics {
        let (ztr, zusd) = self.pool.get_reserves();
        PolMetrics {
            total_reserves_ztr: ztr,
            total_reserves_zusd: zusd,
            total_lp_burned: self.total_lp_tokens_burned,
            total_fees_collected: self
                .total_ingest_fees_collected
                .saturating_add(self.total_swap_fees_collected),
        }
    }
}

impl Default for ProtocolOwnedLiquidity {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_pol() {
        let pol = ProtocolOwnedLiquidity::new();
        assert_eq!(pol.total_ingest_fees_collected, 0);
        assert_eq!(pol.total_swap_fees_collected, 0);
        assert_eq!(pol.total_lp_tokens_burned, 0);
        let (ztr, zusd) = pol.pool.get_reserves();
        assert_eq!(ztr, 0);
        assert_eq!(zusd, 0);
    }

    #[test]
    fn test_ingest_fee_calculation() {
        let mut pol = ProtocolOwnedLiquidity::new();
        // Seed the pool so injection has something to pair against
        pol.pool.add_liquidity(1_000_000_000, 1_000_000).unwrap();

        let deposit = 1_000_000u128; // 1 zUSD
        let result = pol.process_cross_chain_ingest(deposit);

        // 0.5% fee on 1,000,000 = 5,000
        assert_eq!(result.fee_deducted, 5_000);
        assert_eq!(result.net_amount, 995_000);
        assert_eq!(result.fee_deducted + result.net_amount, deposit);
    }

    #[test]
    fn test_ingest_zero_amount() {
        let mut pol = ProtocolOwnedLiquidity::new();
        let result = pol.process_cross_chain_ingest(0);
        assert_eq!(result.net_amount, 0);
        assert_eq!(result.fee_deducted, 0);
        assert_eq!(result.lp_tokens_burned, 0);
    }

    #[test]
    fn test_ingest_lp_tokens_burned() {
        let mut pol = ProtocolOwnedLiquidity::new();
        pol.pool.add_liquidity(1_000_000_000, 1_000_000).unwrap();
        let lp_before = pol.pool.total_lp_tokens;

        let result = pol.process_cross_chain_ingest(10_000_000);

        // LP tokens from injection are burned, not added to circulating supply
        assert_eq!(pol.pool.total_lp_tokens, lp_before);
        assert!(result.lp_tokens_burned > 0 || result.fee_deducted == 0);
        assert_eq!(pol.total_lp_tokens_burned, result.lp_tokens_burned);
    }

    #[test]
    fn test_swap_fee_processing() {
        let mut pol = ProtocolOwnedLiquidity::new();
        pol.pool.add_liquidity(1_000_000_000, 1_000_000).unwrap();
        let lp_before = pol.pool.total_lp_tokens;

        pol.process_swap_fee(200_000, 200);

        assert_eq!(pol.total_swap_fees_collected, 200_000);
        // LP tokens should NOT increase (they're burned)
        assert_eq!(pol.pool.total_lp_tokens, lp_before);
        assert!(pol.total_lp_tokens_burned > 0);
    }

    #[test]
    fn test_swap_fee_zero_noop() {
        let mut pol = ProtocolOwnedLiquidity::new();
        pol.process_swap_fee(0, 0);
        assert_eq!(pol.total_swap_fees_collected, 0);
        assert_eq!(pol.total_lp_tokens_burned, 0);
    }

    #[test]
    fn test_pol_metrics() {
        let mut pol = ProtocolOwnedLiquidity::new();
        pol.pool.add_liquidity(500_000_000, 500_000).unwrap();

        pol.process_cross_chain_ingest(100_000);
        pol.process_swap_fee(10_000, 10);

        let metrics = pol.get_pol_metrics();
        assert!(metrics.total_reserves_ztr > 0);
        assert!(metrics.total_reserves_zusd > 0);
        assert_eq!(
            metrics.total_fees_collected,
            pol.total_ingest_fees_collected + pol.total_swap_fees_collected
        );
        assert_eq!(metrics.total_lp_burned, pol.total_lp_tokens_burned);
    }

    #[test]
    fn test_cumulative_ingest_fees() {
        let mut pol = ProtocolOwnedLiquidity::new();
        pol.pool.add_liquidity(1_000_000_000, 1_000_000).unwrap();

        pol.process_cross_chain_ingest(1_000_000);
        pol.process_cross_chain_ingest(2_000_000);

        // 0.5% of 1M = 5000, 0.5% of 2M = 10000
        assert_eq!(pol.total_ingest_fees_collected, 15_000);
    }

    #[test]
    fn test_reserves_grow_with_fees() {
        let mut pol = ProtocolOwnedLiquidity::new();
        pol.pool.add_liquidity(1_000_000_000, 1_000_000).unwrap();
        let (_, zusd_before) = pol.pool.get_reserves();

        pol.process_cross_chain_ingest(10_000_000);
        let (_, zusd_after) = pol.pool.get_reserves();

        // zUSD reserves should grow by the fee amount injected
        assert!(zusd_after > zusd_before);
    }
}
