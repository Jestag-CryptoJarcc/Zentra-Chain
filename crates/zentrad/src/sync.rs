//! Chain synchronization protocol.

use tracing::info;

/// Sync state of the node.
#[derive(Debug, Clone, PartialEq)]
pub enum SyncState {
    /// Node is syncing from peers (Initial Block Download).
    Syncing { current_height: u64, target_height: u64 },
    /// Node is fully synced.
    Synced,
    /// Node is idle / not connected.
    Idle,
}

/// Chain sync manager.
pub struct SyncManager {
    pub state: SyncState,
}

impl SyncManager {
    pub fn new() -> Self {
        SyncManager { state: SyncState::Idle }
    }

    pub fn is_synced(&self) -> bool {
        self.state == SyncState::Synced
    }

    pub fn start_sync(&mut self, target_height: u64) {
        info!(target_height, "starting chain sync");
        self.state = SyncState::Syncing {
            current_height: 0,
            target_height,
        };
    }

    pub fn update_progress(&mut self, height: u64) {
        if let SyncState::Syncing { target_height, .. } = self.state {
            if height >= target_height {
                self.state = SyncState::Synced;
                info!("chain sync complete");
            } else {
                self.state = SyncState::Syncing {
                    current_height: height,
                    target_height,
                };
            }
        }
    }
}

impl Default for SyncManager {
    fn default() -> Self {
        Self::new()
    }
}
