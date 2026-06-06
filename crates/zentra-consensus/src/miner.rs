//! Mining engine — block template construction and nonce search.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use zentra_types::*;
use zentra_core::header::Header;
use zentra_core::block::Block;
use zentra_core::transaction::Transaction;
use zentra_core::merkle::compute_merkle_root;
use crate::lanes::get_verifier;
use crate::emission::EmissionSchedule;

/// Returns the number of logical processors visible to the OS.
fn logical_core_count() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2)
}

/// Returns true if the CPU appears to be hyperthreaded (logical == 2 × physical heuristic).
fn is_hyperthreaded() -> bool {
    let logical = logical_core_count();
    logical > 1 && logical % 2 == 0
}

/// Returns the estimated physical core count.
/// On a hyperthreaded CPU, this is logical/2. Otherwise, it equals logical.
pub fn physical_core_count() -> usize {
    let logical = logical_core_count();
    if is_hyperthreaded() { (logical / 2).max(1) } else { logical }
}

/// Pin the calling thread to the given **logical** core index.
/// On HT systems, use even logical IDs (0, 2, 4, …) to land on distinct physical cores.
fn set_thread_affinity(logical_core_id: usize) {
    #[cfg(target_os = "windows")]
    {
        extern "system" {
            fn GetCurrentThread() -> *mut std::ffi::c_void;
            fn SetThreadAffinityMask(hThread: *mut std::ffi::c_void, dwThreadAffinityMask: usize) -> usize;
        }
        unsafe {
            let handle = GetCurrentThread();
            let mask = 1usize << logical_core_id;
            SetThreadAffinityMask(handle, mask);
        }
    }

    #[cfg(target_os = "linux")]
    {
        extern "C" {
            fn sched_setaffinity(pid: i32, cpusetsize: usize, mask: *const usize) -> i32;
        }
        unsafe {
            let mut mask: usize = 0;
            mask |= 1 << logical_core_id;
            sched_setaffinity(0, std::mem::size_of::<usize>(), &mask);
        }
    }

    // On other platforms, affinity is a no-op — threads still run in parallel.
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    let _ = logical_core_id;
}

/// Mining engine for a specific hardware lane.
pub struct Miner {
    pub lane: LaneId,
    pub address: Address,
    pub is_mining: Arc<AtomicBool>,
    pub hashes_done: Option<Arc<AtomicU64>>,
}

impl Miner {
    /// Create a new miner for a specific lane.
    pub fn new(lane: LaneId, address: Address) -> Self {
        Miner {
            lane,
            address,
            is_mining: Arc::new(AtomicBool::new(true)),
            hashes_done: None,
        }
    }

    /// Build a block template ready for mining.
    ///
    /// `blue_score`/`blue_work` MUST be the GhostDAG values computed for this
    /// block's parent set (via `GhostdagManager::process_block`), so the header
    /// the miner signs is exactly what every validator independently recomputes.
    /// Deriving them from a naive `selected_tip + 1` height was the source of the
    /// "blue_score mismatch" rejections on merge blocks.
    pub fn build_block_template(
        &self,
        parent_hashes: Vec<Hash>,
        transactions: Vec<Transaction>,
        difficulty_bits: u32,
        blue_score: u64,
        blue_work: u128,
        fees: Amount,
        emission: &EmissionSchedule,
    ) -> Block {
        // The block's height for emission/coinbase purposes is its blue_score,
        // matching how the validator applies the block (apply_block uses
        // header.blue_score as the UTXO height).
        let height = blue_score;
        let reward = emission.block_reward(height).saturating_add(fees);
        let coinbase = Transaction::create_coinbase(reward, self.address.clone(), height);

        let mut all_txs = vec![coinbase];
        all_txs.extend(transactions);

        let tx_hashes: Vec<Hash> = all_txs.iter().map(|tx| tx.txid()).collect();
        let merkle_root = compute_merkle_root(&tx_hashes);

        let header = Header {
            version: BLOCK_VERSION,
            parents: parent_hashes,
            merkle_root,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            nonce: 0,
            lane_id: self.lane,
            bits: difficulty_bits,
            blue_score,
            blue_work,
            pruning_point: Hash::ZERO,
        };

        Block {
            header,
            transactions: all_txs,
        }
    }

    /// Mine a block by searching for a valid nonce.
    ///
    /// `threads` is the number of **physical cores** to use (1 thread per core).
    /// Each thread is pinned to a distinct physical core via `SetThreadAffinityMask`.
    /// On HT CPUs the stride is 2 (logical cores 0, 2, 4, …); on non-HT it is 1.
    /// Returns true if a valid nonce was found, false if mining was stopped.
    pub fn mine_block(&self, template: &mut Block, threads: usize) -> bool {
        let target = Header::target_from_bits(template.header.bits);
        let header_bytes = borsh::to_vec(&template.header).unwrap_or_default();

        // Use exactly the requested number of threads (at least 1).
        let num_threads = threads.max(1);

        // Compute the logical-core stride so each thread lands on a distinct
        // physical core: stride=2 for HT CPUs, stride=1 otherwise.
        let logical_total = logical_core_count();
        let stride = if is_hyperthreaded() { 2usize } else { 1usize };

        tracing::info!(
            lane = %self.lane,
            threads = num_threads,
            logical_cores = logical_total,
            affinity_stride = stride,
            "starting mining…"
        );

        let local_stop = Arc::new(AtomicBool::new(false));
        let found_nonce = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let found = Arc::new(AtomicBool::new(false));

        let mut handles = vec![];

        for thread_idx in 0..num_threads {
            let verifier = get_verifier(self.lane);
            let header_bytes = header_bytes.clone();
            let local_stop = Arc::clone(&local_stop);
            let global_is_mining = Arc::clone(&self.is_mining);
            let found_nonce = Arc::clone(&found_nonce);
            let found = Arc::clone(&found);
            let hashes_done = self.hashes_done.as_ref().map(Arc::clone);
            let lane_id = self.lane;

            // Pin this thread to logical core (thread_idx × stride) % logical_total.
            // This guarantees one thread per physical core on HT systems.
            let affinity_core = (thread_idx * stride) % logical_total;

            let handle = std::thread::spawn(move || {
                set_thread_affinity(affinity_core);

                let mut nonce = thread_idx as u64;
                let step = num_threads as u64;
                let mut local_hashes = 0u64;
                while !local_stop.load(Ordering::Relaxed) && global_is_mining.load(Ordering::Relaxed) {
                    let pow_hash = verifier.compute_pow_hash(&header_bytes, nonce);
                    local_hashes += 1;
                    if local_hashes >= 1024 {
                        if let Some(counter) = &hashes_done {
                            counter.fetch_add(local_hashes, Ordering::Relaxed);
                        }
                        local_hashes = 0;
                    }
                    if pow_hash.meets_target(&target) {
                        if local_hashes > 0 {
                            if let Some(counter) = &hashes_done {
                                counter.fetch_add(local_hashes, Ordering::Relaxed);
                            }
                        }
                        found_nonce.store(nonce, Ordering::SeqCst);
                        found.store(true, Ordering::SeqCst);
                        local_stop.store(true, Ordering::SeqCst);
                        tracing::info!(
                            lane = %lane_id,
                            thread = thread_idx,
                            nonce,
                            hash = %pow_hash,
                            "block mined!"
                        );
                        break;
                    }
                    nonce = nonce.wrapping_add(step);
                }
                if local_hashes > 0 {
                    if let Some(counter) = &hashes_done {
                        counter.fetch_add(local_hashes, Ordering::Relaxed);
                    }
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            let _ = handle.join();
        }

        if found.load(Ordering::Relaxed) {
            template.header.nonce = found_nonce.load(Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// Stop the miner.
    pub fn stop(&self) {
        self.is_mining.store(false, Ordering::SeqCst);
    }

    /// Check if the miner is currently running.
    pub fn is_running(&self) -> bool {
        self.is_mining.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_template() {
        let miner = Miner::new(
            LaneId::Cpu,
            Address::from_public_key(&[1u8; 32], NetworkType::Devnet),
        );
        let emission = EmissionSchedule::new(NetworkType::Devnet);

        let genesis_hash = Block::genesis(NetworkType::Devnet).hash();
        let template = miner.build_block_template(
            vec![genesis_hash],
            vec![],
            Header::easiest_bits(),
            1,
            1,
            Amount::ZERO,
            &emission,
        );

        assert_eq!(template.header.lane_id, LaneId::Cpu);
        assert_eq!(template.transaction_count(), 1); // just coinbase
        assert!(template.transactions[0].is_coinbase());
    }

    #[test]
    fn test_mine_easy_block() {
        let miner = Miner::new(
            LaneId::BtcAsic, // SHA-256 lane
            Address::from_public_key(&[1u8; 32], NetworkType::Devnet),
        );
        let emission = EmissionSchedule::new(NetworkType::Devnet);

        let mut template = miner.build_block_template(
            vec![Hash::ZERO],
            vec![],
            Header::easiest_bits(), // very easy difficulty
            0,
            0,
            Amount::ZERO,
            &emission,
        );

        // Should find a nonce quickly with easiest difficulty
        let found = miner.mine_block(&mut template, 1);
        assert!(found, "should find a valid nonce with easiest difficulty");
    }
}
