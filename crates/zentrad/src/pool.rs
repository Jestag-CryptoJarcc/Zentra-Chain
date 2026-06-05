//! Mining pool coordinator.
//!
//! # How the Zentra pool works
//!
//! A mining pool lets many miners combine their hashrate so they find blocks
//! more steadily, then split the reward in proportion to the work each miner
//! contributed. Zentra implements a **hashrate-weighted proportional** scheme:
//!
//! 1. The pool operator runs `zentrad --pool`. The node mines every block to the
//!    pool's own wallet address, so **all** block rewards accumulate in the pool
//!    wallet rather than going to individual miners.
//! 2. Each miner runs their own node/wallet in "pool" mode. Their node mines to
//!    the pool address and sends a periodic **heartbeat** (`poolHeartbeat`)
//!    reporting its measured hashrate.
//! 3. The pool integrates `hashrate × time` into a **share count** for every
//!    miner. A miner running twice as fast for the same time earns twice the
//!    shares.
//! 4. Every `PAYOUT_INTERVAL_MS` (30 minutes) the pool takes its wallet balance,
//!    keeps a small operator fee (`POOL_FEE_BPS`), and pays each miner
//!    `(their_shares / total_shares) × distributable` via a single multi-output
//!    transaction. Share counters then reset for the next round.
//!
//! ## Trust model
//!
//! This implementation trusts the hashrate reported by each miner (there is no
//! stratum share-submission/verification protocol yet). It is suitable for
//! trusted/known miner sets, testnets, and education. A production pool would
//! verify contributed work via low-difficulty share submissions — see the
//! roadmap. The proportional math is identical either way; only the unit of
//! "work" changes from reported-hashrate-seconds to verified shares.

use std::collections::HashMap;
use crate::node::now_ms;

/// Operator fee in basis points (100 = 1.00%).
pub const POOL_FEE_BPS: u64 = 100;
/// Payout cycle length — 30 minutes.
pub const PAYOUT_INTERVAL_MS: u64 = 30 * 60 * 1000;
/// Minimum per-miner payout (1 ZTR) — smaller amounts roll over to the next round.
pub const MIN_PAYOUT_ZENTS: u64 = 100_000_000;
/// A miner is considered offline after 5 minutes without a heartbeat.
pub const MINER_TIMEOUT_MS: u64 = 5 * 60 * 1000;
/// Keep a small balance reserve in the pool wallet for the payout transaction fee.
pub const PAYOUT_TX_FEE_ZENTS: u64 = 10_000;

/// Default operator address — the 1% pool fee is paid here every payout.
/// (Devnet address; set a mainnet address before mainnet launch.)
pub const DEFAULT_OPERATOR_ADDRESS: &str =
    "zentradev1ua4derzhu02jhvazzvu0n3kr4q2k39ps98k90c3g2qff0kc083hqj7vdl5";

/// The ONE shared pool wallet address. Every pool miner — on every node —
/// mines its block reward to THIS address, so all pool rewards land in a single
/// wallet that the operator (the node holding the matching seed in
/// `pool_wallet.txt`, i.e. the VPS started with `--pool`) splits by hashrate.
///
/// This makes the pool genuinely shared across the whole network: a miner on
/// machine A and a miner on machine B contribute to the same pot and are both
/// paid from it. Only the node whose `pool_wallet.txt` seed derives to this
/// address can spend it, so only the real operator performs payouts.
pub const DEFAULT_POOL_ADDRESS: &str =
    "zentradev12cflspzvf8fd8wapcpt38mrkn5t02nyc2c93kcy22ddvk0cevftq933dgw";

/// Per-miner accounting record.
#[derive(Clone)]
pub struct MinerStat {
    pub address: String,
    /// Most recently reported hashrate (H/s).
    pub hashrate: f64,
    /// Accumulated work this round = Σ hashrate × seconds.
    pub shares: f64,
    pub last_seen_ms: u64,
    pub joined_ms: u64,
    pub total_paid_zents: u64,
}

/// A historical payout event.
#[derive(Clone)]
pub struct PayoutRecord {
    pub timestamp_ms: u64,
    pub total_zents: u64,
    pub fee_zents: u64,
    pub miner_count: usize,
}

/// The pool coordinator state.
pub struct MiningPool {
    pub mnemonic: String,
    pub address: String,
    /// Operator's personal address — the 1% fee is paid here on every payout.
    /// If empty, the fee stays in the pool wallet (collect via the pool seed).
    pub operator_address: String,
    pub miners: HashMap<String, MinerStat>,
    pub blocks_found: u64,
    pub total_paid_zents: u64,
    pub total_fees_zents: u64,
    pub last_payout_ms: u64,
    pub created_ms: u64,
    pub payouts: Vec<PayoutRecord>,
}

impl MiningPool {
    pub fn new(mnemonic: String, address: String) -> Self {
        let now = now_ms();
        MiningPool {
            mnemonic,
            address,
            operator_address: DEFAULT_OPERATOR_ADDRESS.to_string(),
            miners: HashMap::new(),
            blocks_found: 0,
            total_paid_zents: 0,
            total_fees_zents: 0,
            last_payout_ms: now,
            created_ms: now,
            payouts: Vec::new(),
        }
    }

    /// Register a miner (idempotent).
    pub fn join(&mut self, addr: &str) {
        let now = now_ms();
        self.miners.entry(addr.to_string()).or_insert_with(|| MinerStat {
            address: addr.to_string(),
            hashrate: 0.0,
            shares: 0.0,
            last_seen_ms: now,
            joined_ms: now,
            total_paid_zents: 0,
        });
    }

    /// Record a hashrate heartbeat and accumulate shares for the elapsed interval.
    pub fn heartbeat(&mut self, addr: &str, hashrate: f64) {
        let now = now_ms();
        let m = self.miners.entry(addr.to_string()).or_insert_with(|| MinerStat {
            address: addr.to_string(),
            hashrate: 0.0,
            shares: 0.0,
            last_seen_ms: now,
            joined_ms: now,
            total_paid_zents: 0,
        });
        // Integrate the PREVIOUS hashrate over the elapsed time, then update.
        // Cap dt so a long gap (sleep/offline) can't inflate shares.
        let dt = ((now.saturating_sub(m.last_seen_ms)) as f64 / 1000.0).min(120.0);
        m.shares += m.hashrate * dt;
        m.hashrate = hashrate.max(0.0);
        m.last_seen_ms = now;
    }

    /// Drop miners that have not been seen within the timeout (their accrued
    /// shares are retained until payout so they still get paid for past work).
    pub fn prune_offline(&mut self) {
        let now = now_ms();
        self.miners.retain(|_, m| {
            // Keep if recently seen OR still has unpaid shares.
            now.saturating_sub(m.last_seen_ms) < MINER_TIMEOUT_MS || m.shares > 0.0
        });
    }

    /// Miners considered currently active (recent heartbeat).
    pub fn active_count(&self) -> usize {
        let now = now_ms();
        self.miners.values()
            .filter(|m| now.saturating_sub(m.last_seen_ms) < MINER_TIMEOUT_MS)
            .count()
    }

    /// Sum of currently active miners' last-reported hashrate.
    pub fn total_hashrate(&self) -> f64 {
        let now = now_ms();
        self.miners.values()
            .filter(|m| now.saturating_sub(m.last_seen_ms) < MINER_TIMEOUT_MS)
            .map(|m| m.hashrate)
            .sum()
    }

    /// Total accumulated shares this round.
    pub fn total_shares(&self) -> f64 {
        self.miners.values().map(|m| m.shares).sum()
    }

    /// Compute the proportional split of `distributable_zents`.
    /// Returns (miner_address, amount_zents) for miners over the minimum.
    pub fn compute_distribution(&self, distributable_zents: u64) -> Vec<(String, u64)> {
        let total = self.total_shares();
        if total <= 0.0 || distributable_zents == 0 {
            return vec![];
        }
        self.miners.values()
            .filter(|m| m.shares > 0.0)
            .map(|m| {
                let amt = ((m.shares / total) * distributable_zents as f64) as u64;
                (m.address.clone(), amt)
            })
            .filter(|(_, amt)| *amt >= MIN_PAYOUT_ZENTS)
            .collect()
    }

    /// Apply a completed payout: credit miners, reset their shares, record history.
    pub fn apply_payout(&mut self, distribution: &[(String, u64)], fee_zents: u64) {
        let now = now_ms();
        let mut total = 0u64;
        for (addr, amt) in distribution {
            if let Some(m) = self.miners.get_mut(addr) {
                m.total_paid_zents = m.total_paid_zents.saturating_add(*amt);
            }
            total = total.saturating_add(*amt);
        }
        // Reset all share counters for the next round.
        for m in self.miners.values_mut() {
            m.shares = 0.0;
        }
        self.total_paid_zents = self.total_paid_zents.saturating_add(total);
        self.total_fees_zents = self.total_fees_zents.saturating_add(fee_zents);
        self.last_payout_ms = now;
        self.payouts.push(PayoutRecord {
            timestamp_ms: now,
            total_zents: total,
            fee_zents,
            miner_count: distribution.len(),
        });
        if self.payouts.len() > 50 {
            self.payouts.remove(0);
        }
    }

    /// Milliseconds until the next scheduled payout.
    pub fn ms_until_payout(&self) -> u64 {
        let elapsed = now_ms().saturating_sub(self.last_payout_ms);
        PAYOUT_INTERVAL_MS.saturating_sub(elapsed)
    }
}
