//! Block reward emission schedule with halving.

use zentra_types::*;

/// Emission schedule controlling block reward halvings and supply cap.
#[derive(Debug, Clone)]
pub struct EmissionSchedule {
    pub initial_reward: u64,
    pub halving_interval: u64,
    pub max_supply: u64,
}

impl EmissionSchedule {
    /// Create an emission schedule for the given network.
    pub fn new(network: NetworkType) -> Self {
        EmissionSchedule {
            initial_reward: INITIAL_REWARD_ZENTS,
            halving_interval: network.halving_interval(),
            max_supply: MAX_SUPPLY_ZENTS,
        }
    }

    /// Get the block reward for a given block height.
    /// Uses bit-shift for efficient halving: reward >> (height / interval)
    pub fn block_reward(&self, height: u64) -> Amount {
        let epoch = height / self.halving_interval;
        if epoch >= MAX_HALVINGS as u64 {
            return Amount::ZERO;
        }

        let reward = self.initial_reward >> epoch as u32;
        if reward == 0 {
            return Amount::ZERO;
        }

        // Enforce max supply cap
        let total_emitted = self.total_emitted_at_height(height).as_zents();
        if total_emitted >= self.max_supply {
            return Amount::ZERO;
        }

        let remaining = self.max_supply - total_emitted;
        Amount::from_zents(reward.min(remaining))
    }

    /// Calculate the total tokens emitted up to (but not including) the given height.
    pub fn total_emitted_at_height(&self, height: u64) -> Amount {
        let mut total: u64 = 0;
        let mut remaining_blocks = height;
        let mut epoch: u32 = 0;

        while remaining_blocks > 0 && epoch < MAX_HALVINGS {
            let reward = match self.initial_reward.checked_shr(epoch) {
                Some(r) if r > 0 => r,
                _ => break,
            };

            let blocks_in_epoch = remaining_blocks.min(self.halving_interval);
            total = total.saturating_add(blocks_in_epoch.saturating_mul(reward));
            remaining_blocks -= blocks_in_epoch;
            epoch += 1;
        }

        Amount::from_zents(total.min(self.max_supply))
    }

    /// Check if the chain is fully mined at a given height.
    pub fn is_fully_mined(&self, height: u64) -> bool {
        self.block_reward(height).is_zero()
    }

    /// Number of halvings that have occurred at a given height.
    pub fn halvings_occurred(&self, height: u64) -> u32 {
        (height / self.halving_interval) as u32
    }

    /// Blocks remaining until the next halving.
    pub fn blocks_until_next_halving(&self, height: u64) -> u64 {
        self.halving_interval - (height % self.halving_interval)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_reward() {
        let sched = EmissionSchedule::new(NetworkType::Mainnet);
        assert_eq!(sched.block_reward(0).as_zents(), INITIAL_REWARD_ZENTS);
    }

    #[test]
    fn test_halving() {
        let sched = EmissionSchedule::new(NetworkType::Devnet);
        // Halving every HALVING_INTERVAL_BLOCKS on ALL networks (no devnet shortcut)
        let reward_0 = sched.block_reward(0).as_zents();
        let reward_1 = sched.block_reward(HALVING_INTERVAL_BLOCKS).as_zents();
        let reward_2 = sched.block_reward(HALVING_INTERVAL_BLOCKS * 2).as_zents();

        assert_eq!(reward_0, INITIAL_REWARD_ZENTS);
        assert_eq!(reward_1, INITIAL_REWARD_ZENTS / 2);
        assert_eq!(reward_2, INITIAL_REWARD_ZENTS / 4);
    }

    #[test]
    fn test_emission_converges_to_max_supply() {
        let sched = EmissionSchedule::new(NetworkType::Mainnet);
        let very_high = HALVING_INTERVAL_BLOCKS * 64;
        let total = sched.total_emitted_at_height(very_high);
        // Integer halving floors fractional zents away, so the actual cap remains
        // slightly under the 50M headline cap.
        assert_eq!(total.as_zents(), 4_999_999_990_471_200);
    }

    #[test]
    fn test_fully_mined() {
        let sched = EmissionSchedule::new(NetworkType::Devnet);
        assert!(!sched.is_fully_mined(0));
        // After enough halvings to exhaust supply (525,600 * 64 >> max)
        assert!(sched.is_fully_mined(HALVING_INTERVAL_BLOCKS * 64));
    }

    #[test]
    fn test_reward_never_negative() {
        let sched = EmissionSchedule::new(NetworkType::Mainnet);
        for epoch in 0..70 {
            let height = HALVING_INTERVAL_BLOCKS * epoch;
            let reward = sched.block_reward(height);
            assert!(reward.as_zents() <= INITIAL_REWARD_ZENTS);
        }
    }

    #[test]
    fn test_halvings_occurred() {
        let sched = EmissionSchedule::new(NetworkType::Devnet);
        assert_eq!(sched.halvings_occurred(0), 0);
        assert_eq!(sched.halvings_occurred(HALVING_INTERVAL_BLOCKS - 1), 0);
        assert_eq!(sched.halvings_occurred(HALVING_INTERVAL_BLOCKS), 1);
        assert_eq!(sched.halvings_occurred(HALVING_INTERVAL_BLOCKS * 2 + 500), 2);
    }

    #[test]
    fn test_blocks_until_halving() {
        let sched = EmissionSchedule::new(NetworkType::Devnet);
        assert_eq!(sched.blocks_until_next_halving(0), HALVING_INTERVAL_BLOCKS);
        assert_eq!(sched.blocks_until_next_halving(HALVING_INTERVAL_BLOCKS - 1), 1);
        assert_eq!(sched.blocks_until_next_halving(HALVING_INTERVAL_BLOCKS), HALVING_INTERVAL_BLOCKS);
    }
}
