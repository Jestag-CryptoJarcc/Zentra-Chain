//! Independent per-lane difficulty adjustment engines.
//!
//! Each mining lane has its own isolated difficulty engine. Zentra currently
//! exposes CPU mining only, but keeping lane isolation preserves the consensus
//! shape for future lanes.

use std::collections::VecDeque;
use zentra_core::header::Header;
use zentra_types::*;

/// Convert compact bits to a human-readable difficulty number.
///
/// Difficulty 1.0 = genesis (easiest). Higher = harder.
/// Formula: difficulty = genesis_target / current_target
pub fn bits_to_difficulty(bits: u32) -> f64 {
    let genesis_bits = DifficultyEngine::genesis_difficulty();
    let g_exp = (genesis_bits >> 24) as i32;
    let g_mantissa = (genesis_bits & 0x00FF_FFFF) as f64;
    let c_exp = (bits >> 24) as i32;
    let c_mantissa = ((bits & 0x00FF_FFFF) as f64).max(1.0);

    let mantissa_ratio = g_mantissa / c_mantissa;
    // Each exponent unit is 256× (one byte shift in the 256-bit target space)
    let exp_diff = g_exp - c_exp;
    let scale = 256f64.powi(exp_diff);
    (mantissa_ratio * scale).max(1.0)
}

/// Per-lane difficulty adjustment engine using Dark Gravity Wave v3 (DGW v3).
///
/// DGW v3 averages target values over a sliding window, and scales the target
/// based on the ratio of actual time elapsed over target time.
/// Clamps the adjustment factor per block to [1/3, 3] to prevent extreme swings,
/// ensuring highly stable block times that respond dynamically to network hashrate.
#[derive(Debug, Clone)]
pub struct DifficultyEngine {
    pub lane_id: LaneId,
    /// History of (timestamp_ms, bits) for the last recorded blocks.
    history: VecDeque<(u64, u32)>,
    target_block_time_ms: u64,
    window_size: usize,
}

impl DifficultyEngine {
    /// Create a new difficulty engine for a specific lane (defaults to devnet).
    pub fn new(lane_id: LaneId) -> Self {
        Self::new_with_network(lane_id, NetworkType::Devnet)
    }

    /// Create a new difficulty engine for a specific lane and network.
    pub fn new_with_network(lane_id: LaneId, network: NetworkType) -> Self {
        // Devnet: 10-block window → fast convergence for local testing.
        // Mainnet: 12-block window (AdventureCoin standard).
        let window_size = match network {
            NetworkType::Devnet => 10,
            _ => 12,
        };
        DifficultyEngine {
            lane_id,
            history: VecDeque::new(),
            target_block_time_ms: TARGET_BLOCK_TIME_MS,
            window_size,
        }
    }

    /// Record a new block arrival for this lane.
    /// `timestamp_ms` is the block header timestamp.
    /// `bits` is the compact difficulty target the block was mined at.
    pub fn record_block(&mut self, timestamp_ms: u64, bits: u32) {
        self.history.push_back((timestamp_ms, bits));
        // Keep at most window_size + 1 elements so we have window_size intervals
        while self.history.len() > self.window_size + 1 {
            self.history.pop_front();
        }
    }

    /// Calculate the next difficulty target bits using Dark Gravity Wave v3 (DGW v3).
    pub fn next_difficulty(&self) -> u32 {
        let n = self.history.len().saturating_sub(1); // number of intervals
        if n == 0 {
            return Self::genesis_difficulty();
        }

        // 1. Calculate the average target of the last `n` blocks (skipping the oldest index 0)
        let mut target_sum = Hash::ZERO;
        for (_, bits) in self.history.iter().skip(1) {
            let target = Header::target_from_bits(*bits);
            target_sum = add_targets(target_sum, target);
        }
        let target_avg = div_target_by_u64(target_sum, n as u64);

        // 2. Calculate actual timespan between oldest and newest block in window
        let last_ts = self.history.back().unwrap().0;
        let first_ts = self.history.front().unwrap().0;
        let actual_timespan = last_ts.saturating_sub(first_ts);

        // 3. Calculate target timespan
        let target_timespan = (n as u64) * self.target_block_time_ms;

        // 4. Clamp actual timespan between 1/3 and 3x of target timespan to prevent massive swings
        let min_timespan = target_timespan / 3;
        let max_timespan = target_timespan * 3;
        let clamped_timespan = actual_timespan.clamp(min_timespan, max_timespan);

        // 5. Scale down num and den to prevent 256-bit overflow during target multiplication
        let mut num = clamped_timespan;
        let mut den = target_timespan;
        if num > 100 {
            let scale = num / 100;
            num /= scale;
            den = (den / scale).max(1);
        }

        // 6. Adjust target: new_target = target_avg * clamped_timespan / target_timespan
        let adjusted = mul_div_target(target_avg, num, den);

        // 7. Clamp: never easier than genesis, never zero
        let max_target = Header::target_from_bits(Self::genesis_difficulty());
        let clamped = if adjusted > max_target || adjusted.is_zero() {
            max_target
        } else {
            adjusted
        };

        target_to_bits(clamped)
    }

    /// Get the compact bits of the most recently recorded block.
    pub fn current_difficulty(&self) -> u32 {
        self.history.back().map(|(_, bits)| *bits).unwrap_or_else(Self::genesis_difficulty)
    }

    /// Genesis / easiest CPU difficulty (compact bits).
    pub fn genesis_difficulty() -> u32 {
        0x1F0FFFFF
    }

    /// Number of blocks recorded in the engine history.
    pub fn window_len(&self) -> usize {
        self.history.len()
    }
}

/// Manager holding all independent lane engines.
pub struct DifficultyManager {
    engines: Vec<DifficultyEngine>,
}

impl DifficultyManager {
    /// Create a new manager with all lane engines.
    pub fn new() -> Self {
        Self::new_with_network(NetworkType::Devnet)
    }

    /// Create a new manager with all lane engines for the given network.
    pub fn new_with_network(network: NetworkType) -> Self {
        let engines = LaneId::ALL
            .iter()
            .map(|lane| DifficultyEngine::new_with_network(*lane, network))
            .collect();
        DifficultyManager { engines }
    }

    /// Get the next difficulty for a specific lane.
    pub fn get_next_difficulty(&self, lane: LaneId) -> u32 {
        self.engines[lane.as_u8() as usize].next_difficulty()
    }

    /// Record a block arrival for a specific lane.
    pub fn record_block(&mut self, lane: LaneId, timestamp_ms: u64, bits: u32) {
        self.engines[lane.as_u8() as usize].record_block(timestamp_ms, bits);
    }

    /// Get a reference to a specific engine.
    pub fn engine(&self, lane: LaneId) -> &DifficultyEngine {
        &self.engines[lane.as_u8() as usize]
    }
}

impl Default for DifficultyManager {
    fn default() -> Self {
        Self::new()
    }
}

fn add_targets(a: Hash, b: Hash) -> Hash {
    let mut result = [0u8; 32];
    let mut carry = 0u16;
    for i in (0..32).rev() {
        let val = a.0[i] as u16 + b.0[i] as u16 + carry;
        result[i] = (val & 0xFF) as u8;
        carry = val >> 8;
    }
    Hash::from_bytes(result)
}

fn div_target_by_u64(target: Hash, divisor: u64) -> Hash {
    if divisor == 0 {
        return target;
    }
    let mut result = [0u8; 32];
    let mut rem = 0u128;
    let div = divisor as u128;
    for i in 0..32 {
        let val = (rem << 8) + target.0[i] as u128;
        result[i] = (val / div) as u8;
        rem = val % div;
    }
    Hash::from_bytes(result)
}

fn mul_div_target(target: Hash, num: u64, den: u64) -> Hash {
    let mut multiplied = [0u8; 32];
    let mut carry = 0u128;

    for i in (0..32).rev() {
        let value = target.0[i] as u128 * num as u128 + carry;
        multiplied[i] = (value & 0xFF) as u8;
        carry = value >> 8;
    }

    if carry > 0 {
        return Hash::from_bytes([0xFF; 32]);
    }

    let mut divided = [0u8; 32];
    let mut rem = 0u128;
    let den = den as u128;
    for i in 0..32 {
        let value = (rem << 8) + multiplied[i] as u128;
        divided[i] = (value / den) as u8;
        rem = value % den;
    }

    Hash::from_bytes(divided)
}

fn target_to_bits(target: Hash) -> u32 {
    let bytes = target.0;
    let first_non_zero = match bytes.iter().position(|b| *b != 0) {
        Some(pos) => pos,
        None => return 0,
    };

    let mut exponent = (32 - first_non_zero) as u32;
    let mut mantissa = if exponent <= 3 {
        let mut value = 0u32;
        for b in &bytes[first_non_zero..] {
            value = (value << 8) | *b as u32;
        }
        value << (8 * (3 - exponent))
    } else {
        ((bytes[first_non_zero] as u32) << 16)
            | ((*bytes.get(first_non_zero + 1).unwrap_or(&0) as u32) << 8)
            | (*bytes.get(first_non_zero + 2).unwrap_or(&0) as u32)
    };

    if mantissa & 0x0080_0000 != 0 {
        mantissa >>= 8;
        exponent += 1;
    }

    (exponent << 24) | (mantissa & 0x007F_FFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_genesis_difficulty() {
        let engine = DifficultyEngine::new(LaneId::Cpu);
        assert_eq!(engine.next_difficulty(), DifficultyEngine::genesis_difficulty());
    }

    #[test]
    fn test_bits_round_trip_target() {
        let bits = DifficultyEngine::genesis_difficulty();
        let target = Header::target_from_bits(bits);
        assert_eq!(target_to_bits(target), bits);
    }

    #[test]
    fn test_fast_blocks_increase_difficulty() {
        let mut engine = DifficultyEngine::new(LaneId::Cpu);
        let bits = DifficultyEngine::genesis_difficulty();
        // 10 blocks every 10 ms → 600× faster than 60 s target → difficulty should rise
        for i in 0..11 {
            engine.record_block(i * 10, bits);
        }
        let new_bits = engine.next_difficulty();
        assert!(Header::target_from_bits(new_bits) < Header::target_from_bits(bits),
            "difficulty should increase when blocks arrive fast");
    }

    #[test]
    fn test_slow_blocks_decrease_difficulty_but_not_past_genesis() {
        let mut engine = DifficultyEngine::new(LaneId::Cpu);
        // Start harder than genesis
        let bits = 0x1E0FFFFF;
        for i in 0..11 {
            engine.record_block(i * 600_000, bits); // 10-minute blocks
        }
        let new_bits = engine.next_difficulty();
        assert!(Header::target_from_bits(new_bits) > Header::target_from_bits(bits),
            "difficulty should ease when blocks are slow");
        assert!(Header::target_from_bits(new_bits) <= Header::target_from_bits(DifficultyEngine::genesis_difficulty()),
            "difficulty must not go easier than genesis");
    }

    #[test]
    fn test_spike_resistance() {
        // DGW v3 with a 10-block window.
        // Scenario: 5 super-fast spike blocks (100 ms each), then 10 slower blocks.
        // After the 10 slower blocks fully displace the spike from the window, the
        // computed DGW should reflect the slower blocks, easing the difficulty.
        let mut engine = DifficultyEngine::new(LaneId::Cpu);
        let bits = DifficultyEngine::genesis_difficulty();
        let target_ms = TARGET_BLOCK_TIME_MS;

        // Spike: 6 timestamps → 5 intervals at 100 ms each
        let mut ts = 0u64;
        for _ in 0..6 {
            engine.record_block(ts, bits);
            ts += 100;
        }
        let after_spike_bits = engine.next_difficulty();
        // Spike should have made difficulty harder (target smaller)
        assert!(
            Header::target_from_bits(after_spike_bits) < Header::target_from_bits(bits),
            "difficulty should increase during spike"
        );

        // Normal: 11 more blocks at slow speed (2x target) — this expels the spike from the window
        // and simulates that the hashrate spike subsided, so blocks take longer to solve at the high difficulty.
        for _ in 0..11 {
            engine.record_block(ts, after_spike_bits);
            ts += target_ms * 2;
        }
        let after_normal_bits = engine.next_difficulty();
        // After slower blocks fill the window, difficulty should ease back (target increases)
        assert!(
            Header::target_from_bits(after_normal_bits) > Header::target_from_bits(after_spike_bits),
            "difficulty should ease back after spike subsides and window fills with slower blocks"
        );
    }

    #[test]
    fn test_bits_to_difficulty() {
        let genesis = DifficultyEngine::genesis_difficulty();
        assert!((bits_to_difficulty(genesis) - 1.0).abs() < 0.01,
            "genesis difficulty should be ~1.0");
        let harder_bits = 0x1E0FFFFF;
        assert!(bits_to_difficulty(harder_bits) > 200.0,
            "0x1E0FFFFF should be >> 1.0 difficulty");
    }

    #[test]
    fn test_lane_isolation() {
        let mut mgr = DifficultyManager::new();
        let bits = DifficultyEngine::genesis_difficulty();
        for i in 0..15 {
            mgr.record_block(LaneId::BtcAsic, i * 10, bits);
        }
        assert_eq!(mgr.get_next_difficulty(LaneId::Cpu), bits);
        assert_ne!(mgr.get_next_difficulty(LaneId::BtcAsic), bits);
    }

    #[test]
    fn test_all_five_engines_exist() {
        let mgr = DifficultyManager::new();
        for lane in LaneId::ALL {
            let engine = mgr.engine(lane);
            assert_eq!(engine.lane_id, lane);
        }
    }
}
