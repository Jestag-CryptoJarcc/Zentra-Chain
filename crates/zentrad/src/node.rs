//! Node orchestrator — wires all components together.

use std::sync::Arc;
use tracing::info;
use zentra_types::*;
use zentra_core::database::ZentraDb;
use zentra_core::dag::DagGraph;
use zentra_core::mempool::Mempool;
use zentra_consensus::emission::EmissionSchedule;
use zentra_consensus::difficulty::DifficultyManager;
use crate::config::NodeConfig;
use crate::sync::SyncManager;
use crate::pool::MiningPool;

/// Live mining stats received from a remote peer via P2P stats broadcast.
#[derive(Clone)]
pub struct PeerMinerStat {
    pub peer_addr: String,
    pub hashrate: f64,
    pub height: u64,
    pub pool_mining: bool,
    pub payout_address: String,
    pub last_seen_ms: u64,
}

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64};
use zentra_core::block::Block;
use zentra_core::header::Header;
use zentra_core::utxo::UtxoSet;
use zentra_finance::amm::LiquidityPool;
use zentra_finance::vault::OmniVault;
use zentra_finance::quarantine::QuarantineManager;
use zentra_finance::encrypted_mempool::EncryptedMempool;

/// Minimum fee (in zents) a transaction must pay to be relayed/mined. Mirrors
/// Bitcoin's minRelayTxFee — it stops zero-fee dust from flooding the mempool.
/// MUST stay ≤ the faucet fee (1000) so faucet/wallet sends still relay.
pub const MIN_RELAY_FEE_ZENTS: u64 = 1000;

/// The Zentra node — orchestrates all subsystems.
pub struct ZentraNode {
    pub config: NodeConfig,
    pub dag: DagGraph,
    pub mempool: Mempool,
    pub emission: EmissionSchedule,
    pub difficulty: Arc<parking_lot::Mutex<DifficultyManager>>,
    pub sync: SyncManager,
    pub genesis_hash: Hash,

    // Active state models
    pub utxo_set: Arc<parking_lot::Mutex<UtxoSet>>,
    pub amm_pool: Arc<parking_lot::Mutex<LiquidityPool>>,
    pub vault: Arc<parking_lot::Mutex<OmniVault>>,
    pub quarantine: Arc<parking_lot::Mutex<QuarantineManager>>,
    pub encrypted_mempool: Arc<parking_lot::Mutex<EncryptedMempool>>,

    // Mined blocks history tracking (recent blocks)
    pub block_history: Arc<parking_lot::Mutex<Vec<Block>>>,

    // Dynamic mining state
    pub is_mining: Arc<AtomicBool>,
    pub is_syncing: Arc<AtomicBool>,
    /// Number of peer-sync operations currently in flight. The miner pauses while
    /// this is > 0. A plain bool flag was racy: with several concurrent peer
    /// threads the first to finish cleared it while others were still syncing.
    pub active_syncs: Arc<std::sync::atomic::AtomicUsize>,
    pub miner_lane: Arc<AtomicU8>,
    pub miner_threads: Arc<AtomicU8>,
    pub miner_address: Arc<parking_lot::Mutex<Option<Address>>>,
    pub mining_hashes: Arc<AtomicU64>,
    pub mined_blocks: Arc<AtomicU64>,
    pub mining_started_ms: Arc<AtomicU64>,

    // Mining pool coordinator
    pub pool: Arc<parking_lot::Mutex<MiningPool>>,
    /// When true, mined blocks pay out to the pool wallet (pool-operator mode).
    pub pool_mode: Arc<AtomicBool>,
    /// This miner's OWN payout address when participating in the pool, reported
    /// to the operator over P2P so it credits the right person (not the pool).
    pub pool_member_payout: Arc<parking_lot::Mutex<String>>,
    /// Pool stats learned from the operator via stats_ack (for member display).
    pub learned_pool_miners: Arc<AtomicU64>,
    pub learned_pool_hashrate: Arc<parking_lot::Mutex<f64>>,
    /// Highest block height any peer has advertised — the mining worker won't
    /// mine while we are far below this (so a fresh/behind node syncs the
    /// existing chain instead of mining its own competing fork).
    pub max_peer_height: Arc<AtomicU64>,
    /// Blocks received whose parents we don't have yet, keyed by block hash.
    pub orphans: Arc<parking_lot::Mutex<std::collections::HashMap<Hash, Block>>>,
    /// Parent hashes we are missing and should fetch from peers (getblock).
    pub wanted: Arc<parking_lot::Mutex<std::collections::HashSet<Hash>>>,

    /// Manually-added peer addresses (host:port).
    pub manual_peers: Arc<parking_lot::Mutex<Vec<String>>>,

    /// Live stats received from each peer (keyed by peer addr string).
    pub peer_stats: Arc<parking_lot::Mutex<std::collections::HashMap<String, PeerMinerStat>>>,

    /// Sleep this many ms after each mined block (low-power keep-alive mining).
    /// 0 = mine flat-out (normal). Used by seed nodes to sip CPU.
    pub mine_throttle_ms: Arc<AtomicU64>,

    // ── Test faucet ──────────────────────────────────────────────────────────
    /// Faucet wallet seed (its address is also the donation address).
    pub faucet_mnemonic: String,
    pub faucet_address: String,
    /// address -> last claim timestamp (ms), for cooldown rate-limiting.
    pub faucet_claims: Arc<parking_lot::Mutex<std::collections::HashMap<String, u64>>>,
    pub faucet_total_claims: Arc<AtomicU64>,
}

impl ZentraNode {
    /// Initialize a new node from configuration.
    pub fn new(config: NodeConfig) -> anyhow::Result<Self> {
        info!(network = %config.network, "initializing Zentra node");

        // Open database
        std::fs::create_dir_all(&config.data_dir)?;
        let db_path = config.data_dir.join("db");
        let db = Arc::new(ZentraDb::open(&db_path)?);

        // Initialize DAG
        let dag = DagGraph::new(db);
        let genesis_hash = dag.init_genesis(config.network)?;

        // Create subsystems
        let mempool = Mempool::new(MAX_TXS_PER_BLOCK * 10);
        let emission = EmissionSchedule::new(config.network);
        let difficulty = Arc::new(parking_lot::Mutex::new(DifficultyManager::new_with_network(config.network)));
        let sync = SyncManager::new();
        let amm_pool = LiquidityPool::new();
        let vault = OmniVault::new(2, 3);
        let quarantine = QuarantineManager::new();
        let encrypted_mempool = EncryptedMempool::new();

        // Reconstruct selected chain to restore UTXO set and block history
        let mut utxo_set = UtxoSet::new();
        let mut history = vec![];

        let selected_tip = dag.get_selected_tip().ok().flatten();
        let mut path = vec![];
        let mut curr = selected_tip;
        while let Some(hash) = curr {
            if let Ok(Some(block)) = dag.get_block(&hash) {
                path.push(block.clone());
                curr = block.header.parents.first().cloned();
            } else {
                break;
            }
        }
        path.reverse();

        if path.is_empty() {
            // Fallback to genesis block if no path found
            let genesis_block = Block::genesis(config.network);
            let _ = utxo_set.apply_block(&genesis_block, 0);
            history.push(genesis_block);
        } else {
            for block in path {
                let height = block.header.blue_score;
                let _ = utxo_set.apply_block(&block, height);
                
                // Record block in difficulty manager to restore difficulty tracking state
                difficulty.lock().record_block(
                    block.header.lane_id,
                    block.header.timestamp,
                    block.header.bits,
                );
                
                history.push(block);
            }
        }

        // Limit block history cache size to 100
        if history.len() > 100 {
            let drain_len = history.len() - 100;
            history.drain(0..drain_len);
        }

        let block_history = Arc::new(parking_lot::Mutex::new(history));

        // Mining controls
        let is_mining = Arc::new(AtomicBool::new(config.mining.enabled));
        let miner_lane = Arc::new(AtomicU8::new(0));
        let miner_threads = Arc::new(AtomicU8::new(config.mining.threads as u8));
        let miner_address = Arc::new(parking_lot::Mutex::new(None));
        let mining_hashes = Arc::new(AtomicU64::new(0));
        let mined_blocks = Arc::new(AtomicU64::new(0));
        let mining_started_ms = Arc::new(AtomicU64::new(if config.mining.enabled {
            now_ms()
        } else {
            0
        }));

        // ── Mining pool wallet: load or generate, persist to data dir ──
        let (pool_mnemonic, pool_address) = {
            use zentra_wallet::keygen::MasterKey;
            let path = config.data_dir.join("pool_wallet.txt");
            let phrase = if let Ok(s) = std::fs::read_to_string(&path) {
                let t = s.trim().to_string();
                if t.split_whitespace().count() >= 12 { t } else { String::new() }
            } else { String::new() };
            let phrase = if phrase.is_empty() {
                let m = MasterKey::generate();
                let p = m.mnemonic_phrase().to_string();
                let _ = std::fs::write(&path, &p);
                p
            } else { phrase };
            let addr = MasterKey::from_mnemonic(&phrase)
                .map(|m| m.derive_keypair(0, 0).address(config.network).to_string())
                .unwrap_or_default();
            (phrase, addr)
        };
        // ── Faucet wallet: load or generate, persist to data dir ──
        let (faucet_mnemonic, faucet_address) = {
            use zentra_wallet::keygen::MasterKey;
            let path = config.data_dir.join("faucet_wallet.txt");
            let phrase = std::fs::read_to_string(&path).ok()
                .map(|s| s.trim().to_string())
                .filter(|t| t.split_whitespace().count() >= 12)
                .unwrap_or_default();
            let phrase = if phrase.is_empty() {
                let m = MasterKey::generate();
                let p = m.mnemonic_phrase().to_string();
                let _ = std::fs::write(&path, &p);
                p
            } else { phrase };
            let addr = MasterKey::from_mnemonic(&phrase)
                .map(|m| m.derive_keypair(0, 0).address(config.network).to_string())
                .unwrap_or_default();
            (phrase, addr)
        };

        // The pool address is derived from the operator's local pool seed.
        // This allows anyone to run their own pool on their own seed.
        let mut pool_inner = MiningPool::new(pool_mnemonic, pool_address.clone());
        // Load a previously-saved operator fee address, if any.
        if let Ok(op) = std::fs::read_to_string(config.data_dir.join("pool_operator.txt")) {
            let op = op.trim().to_string();
            if !op.is_empty() { pool_inner.operator_address = op; }
        }
        let pool = Arc::new(parking_lot::Mutex::new(pool_inner));
        let pool_mode = Arc::new(AtomicBool::new(false));

        info!(
            genesis = %genesis_hash,
            pool_address = %pool_address,
            "node initialized"
        );

        Ok(ZentraNode {
            config,
            dag,
            mempool,
            emission,
            difficulty,
            sync,
            genesis_hash,
            utxo_set: Arc::new(parking_lot::Mutex::new(utxo_set)),
            amm_pool: Arc::new(parking_lot::Mutex::new(amm_pool)),
            vault: Arc::new(parking_lot::Mutex::new(vault)),
            quarantine: Arc::new(parking_lot::Mutex::new(quarantine)),
            encrypted_mempool: Arc::new(parking_lot::Mutex::new(encrypted_mempool)),
            block_history,
            is_mining,
            is_syncing: Arc::new(AtomicBool::new(false)),
            active_syncs: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            miner_lane,
            miner_threads,
            miner_address,
            mining_hashes,
            mined_blocks,
            mining_started_ms,
            pool,
            pool_mode,
            pool_member_payout: Arc::new(parking_lot::Mutex::new(String::new())),
            learned_pool_miners: Arc::new(AtomicU64::new(0)),
            learned_pool_hashrate: Arc::new(parking_lot::Mutex::new(0.0)),
            max_peer_height: Arc::new(AtomicU64::new(0)),
            orphans: Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
            wanted: Arc::new(parking_lot::Mutex::new(std::collections::HashSet::new())),
            manual_peers: Arc::new(parking_lot::Mutex::new(Vec::new())),
            mine_throttle_ms: Arc::new(AtomicU64::new(0)),
            peer_stats: Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
            faucet_mnemonic,
            faucet_address,
            faucet_claims: Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
            faucet_total_claims: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Get the current tip count.
    pub fn tip_count(&self) -> usize {
        self.dag.tip_count()
    }

    /// Get the current tips.
    pub fn tips(&self) -> Vec<Hash> {
        self.dag.get_tips()
    }

    /// Start background mining worker.
    pub fn start_mining_worker(self: &Arc<Self>) {
        let self_clone = Arc::clone(self);
        std::thread::spawn(move || {
            info!("Background mining worker thread spawned");
            loop {
                if self_clone.is_mining.load(std::sync::atomic::Ordering::Relaxed) {
                    // Don't mine while ANY peer sync is in flight.
                    if self_clone.active_syncs.load(std::sync::atomic::Ordering::Relaxed) > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(500));
                        continue;
                    }
                    let pool_active = self_clone.pool_mode.load(std::sync::atomic::Ordering::Relaxed);
                    let payout_address = if pool_active {
                        // Pool-operator mode: every block reward goes to the pool wallet.
                        let pool = self_clone.pool.lock();
                        Address::from_bech32(&pool.address).unwrap_or_else(|_| {
                            Address::from_public_key(&[0u8; 32], self_clone.config.network)
                        })
                    } else {
                        let addr_opt = self_clone.miner_address.lock();
                        addr_opt.clone().unwrap_or_else(|| {
                            Address::from_public_key(&[0u8; 32], self_clone.config.network)
                        })
                    };

                    let parents = self_clone.dag.get_tips();
                    let lane_u8 = self_clone.miner_lane.load(std::sync::atomic::Ordering::Relaxed);
                    let lane = LaneId::from_u8(lane_u8).unwrap_or(LaneId::Cpu);

                    // Compute this block's GhostDAG data over its REAL parent set,
                    // using exactly the same algorithm the validator runs in
                    // connect_block. Stamping the header from this (instead of a
                    // naive selected_tip+1) is what makes merge blocks validate.
                    let (blue_score, blue_work) = {
                        let get_ghostdag = |h: &Hash| -> Option<zentra_consensus::ghostdag::GhostdagData> {
                            if let Ok(Some(bytes)) = self_clone.dag.get_ghostdag_raw(h) {
                                borsh::BorshDeserialize::try_from_slice(&bytes).ok()
                            } else {
                                None
                            }
                        };
                        let manager = zentra_consensus::ghostdag::GhostdagManager::default_k();
                        let gd = manager.process_block(&Hash::ZERO, &parents, &get_ghostdag);
                        (gd.blue_score, gd.blue_work)
                    };
                    let height = blue_score;

                    // Build the block's tx list by validating EACH candidate
                    // individually against the current UTXO set (mirroring
                    // validate_full_block). A tx that is invalid from this node's
                    // view is EVICTED from the mempool instead of poisoning the
                    // whole block — so any single valid tx is always mineable and
                    // a node never gets stuck unable to confirm anything.
                    let (txs, fees) = {
                        use zentra_core::transaction::{OutPoint, TxOutput};
                        let candidates = self_clone.mempool.get_transactions_for_block(20);
                        let utxo = self_clone.utxo_set.lock();
                        let mut spent: std::collections::HashSet<OutPoint> = std::collections::HashSet::new();
                        let mut chosen: Vec<zentra_core::transaction::Transaction> = Vec::new();
                        let mut fee_sum = Amount::ZERO;
                        const MATURITY: u64 = 10;
                        for tx in candidates {
                            let txid = tx.txid();
                            if tx.is_coinbase() { self_clone.mempool.remove_transaction(&txid); continue; }
                            if tx.verify_signatures().is_err() { self_clone.mempool.remove_transaction(&txid); continue; }
                            let mut ok = true; let mut in_sum = 0u64;
                            for inp in &tx.inputs {
                                let op = OutPoint::new(inp.prev_tx_hash, inp.output_index);
                                if spent.contains(&op) { ok = false; break; }
                                match utxo.get_utxo(&op) {
                                    Some(e) if (!e.is_coinbase || height >= e.block_height.saturating_add(MATURITY))
                                        && Address::from_public_key(&inp.public_key, self_clone.config.network) == e.address => {
                                        in_sum = in_sum.saturating_add(e.amount.as_zents());
                                    }
                                    _ => { ok = false; break; }
                                }
                            }
                            if !ok { self_clone.mempool.remove_transaction(&txid); continue; }
                            let out_sum: u64 = tx.outputs.iter().map(|o| match o {
                                TxOutput::Standard { amount, .. } | TxOutput::Burn { amount, .. } => amount.as_zents(),
                            }).sum();
                            if out_sum > in_sum { self_clone.mempool.remove_transaction(&txid); continue; }
                            for inp in &tx.inputs { spent.insert(OutPoint::new(inp.prev_tx_hash, inp.output_index)); }
                            fee_sum = fee_sum.saturating_add(Amount::from_zents(in_sum - out_sum));
                            chosen.push(tx);
                            if chosen.len() >= 10 { break; }
                        }
                        (chosen, fee_sum)
                    };

                    let bits = self_clone.difficulty.lock().get_next_difficulty(lane);

                    let miner = zentra_consensus::miner::Miner {
                        lane,
                        address: payout_address.clone(),
                        is_mining: Arc::clone(&self_clone.is_mining),
                        hashes_done: Some(Arc::clone(&self_clone.mining_hashes)),
                    };

                    let mut template = miner.build_block_template(
                        parents,
                        txs,
                        bits,
                        blue_score,
                        blue_work,
                        fees,
                        &self_clone.emission,
                    );

                    // Mine block
                    let threads = self_clone.miner_threads.load(std::sync::atomic::Ordering::Relaxed) as usize;
                    let found = miner.mine_block(&mut template, threads);

                    if found {
                        if self_clone.connect_block(&template) {
                            self_clone.mined_blocks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            crate::p2p_sync::broadcast_block(&self_clone, &template);
                            info!(
                                height = template.header.blue_score,
                                hash = %template.hash(),
                                txs = template.transaction_count(),
                                "Mined new block!"
                            );
                        } else {
                            tracing::error!("self-mined block failed to connect");
                        }
                    }
                    // Low-power keep-alive throttle: nap after each attempt so a
                    // seed node sips CPU instead of pegging a core.
                    let throttle = self_clone.mine_throttle_ms.load(std::sync::atomic::Ordering::Relaxed);
                    if throttle > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(throttle));
                    }
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        });
    }

    /// Background pool payout worker. Every cycle it prunes offline miners and,
    /// once the payout interval elapses, distributes the pool wallet balance to
    /// miners in proportion to their accumulated shares (minus the operator fee).
    pub fn start_pool_payout_worker(self: &Arc<Self>) {
        let node = Arc::clone(self);
        std::thread::spawn(move || {
            info!("Pool payout worker thread spawned");
            loop {
                std::thread::sleep(std::time::Duration::from_secs(20));

                // Always keep the miner table tidy.
                node.pool.lock().prune_offline();

                // Only the pool operator (pool_mode on) performs payouts.
                if !node.pool_mode.load(std::sync::atomic::Ordering::Relaxed) {
                    continue;
                }

                let due = node.pool.lock().ms_until_payout() == 0;
                if due {
                    node.run_pool_payout();
                }
            }
        });
    }

    /// Execute a single payout round: read the pool wallet balance, split it by
    /// shares, build one multi-output transaction, and submit it to the mempool.
    pub fn run_pool_payout(self: &Arc<Self>) {
        use zentra_wallet::keygen::MasterKey;
        use zentra_core::transaction::{TxInput, TxOutput, Transaction};
        use ed25519_dalek::Signer;

        let (mnemonic, pool_addr_str, operator_addr_str) = {
            let p = self.pool.lock();
            (p.mnemonic.clone(), p.address.clone(), p.operator_address.clone())
        };

        let master = match MasterKey::from_mnemonic(&mnemonic) {
            Ok(m) => m, Err(_) => return,
        };
        let kp = master.derive_keypair(0, 0);
        let pool_address = match Address::from_bech32(&pool_addr_str) {
            Ok(a) => a, Err(_) => return,
        };

        // Only the real pool operator can pay out: its local seed must derive to
        // the shared pool address. Member nodes (different seed) skip silently,
        // so we never produce an invalid spend of the shared pool wallet.
        if kp.address(self.config.network) != pool_address {
            self.pool.lock().last_payout_ms = now_ms();
            return;
        }

        // Pool wallet balance — only MATURE UTXOs. Coinbase outputs can't be
        // spent until they mature, so a payout that includes an immature
        // coinbase would be rejected by the validator and stick in the mempool
        // forever. Filtering here keeps every payout transaction valid.
        let cur_h = self.current_height();
        const COINBASE_MATURITY: u64 = 10;
        let utxos: Vec<_> = self.utxo_set.lock().get_utxos_for_address(&pool_address)
            .into_iter()
            .filter(|(_, e)| !e.is_coinbase || cur_h >= e.block_height.saturating_add(COINBASE_MATURITY))
            .collect();
        let balance: u64 = utxos.iter().map(|(_, e)| e.amount.as_zents()).sum();
        if balance <= crate::pool::PAYOUT_TX_FEE_ZENTS {
            // Nothing to pay; just reset the timer so we don't spin.
            self.pool.lock().last_payout_ms = now_ms();
            return;
        }

        // Operator fee + reserve the on-chain tx fee.
        let after_txfee = balance - crate::pool::PAYOUT_TX_FEE_ZENTS;
        let operator_fee = after_txfee * crate::pool::POOL_FEE_BPS / 10_000;
        let distributable = after_txfee.saturating_sub(operator_fee);

        let distribution = self.pool.lock().compute_distribution(distributable);
        if distribution.is_empty() {
            // No eligible miners this round — keep funds, reset timer.
            self.pool.lock().last_payout_ms = now_ms();
            return;
        }

        let total_payout: u64 = distribution.iter().map(|(_, a)| *a).sum();
        // Pay the operator fee out to the operator's own address if one is set,
        // so fees are cleanly separated from miners' pending balances.
        let operator_out = Address::from_bech32(&operator_addr_str).ok()
            .map(|a| (a, operator_fee))
            .filter(|(_, amt)| *amt > 0);
        let operator_out_amt = operator_out.as_ref().map(|(_, a)| *a).unwrap_or(0);
        let total_needed = total_payout + operator_out_amt + crate::pool::PAYOUT_TX_FEE_ZENTS;

        // Select pool UTXOs to cover the payout.
        let mut selected = Vec::new();
        let mut accumulated = 0u64;
        for (op, entry) in &utxos {
            selected.push((op.clone(), entry.clone()));
            accumulated += entry.amount.as_zents();
            if accumulated >= total_needed { break; }
        }
        if accumulated < total_needed { return; }

        let inputs: Vec<TxInput> = selected.iter().map(|(op, _)| TxInput {
            prev_tx_hash: op.tx_hash,
            output_index: op.index,
            signature: vec![],
            public_key: kp.public_key_bytes(),
        }).collect();

        // One output per miner.
        let mut outputs: Vec<TxOutput> = distribution.iter().filter_map(|(addr, amt)| {
            Address::from_bech32(addr).ok().map(|a| TxOutput::Standard {
                address: a, amount: Amount::from_zents(*amt), script: vec![],
            })
        }).collect();

        // Operator fee → operator's own address (if configured).
        if let Some((op_addr, op_amt)) = operator_out {
            outputs.push(TxOutput::Standard {
                address: op_addr, amount: Amount::from_zents(op_amt), script: vec![],
            });
        }

        // Change (leftover dust) back to the pool wallet.
        let change = accumulated - total_needed;
        if change > 0 {
            outputs.push(TxOutput::Standard {
                address: pool_address.clone(),
                amount: Amount::from_zents(change),
                script: vec![],
            });
        }

        let mut tx = Transaction {
            version: 1,
            tx_type: TransactionType::Transfer,
            inputs,
            outputs,
            payload: vec![],
            lock_time: 0,
        };
        let signing_hash = tx.signing_hash();
        let signature = kp.signing_key().sign(signing_hash.as_bytes()).to_bytes().to_vec();
        for input in &mut tx.inputs { input.signature = signature.clone(); }

        let txid = tx.txid();
        match self.mempool.add_transaction(tx, Amount::from_zents(crate::pool::PAYOUT_TX_FEE_ZENTS)) {
            Ok(_) => {
                self.pool.lock().apply_payout(&distribution, operator_fee);
                info!(
                    txid = %txid.to_hex(),
                    miners = distribution.len(),
                    total = total_payout,
                    fee = operator_fee,
                    "Pool payout submitted"
                );
            }
            Err(e) => tracing::error!(err = %e, "Pool payout tx rejected"),
        }
    }
}

impl ZentraNode {
    /// Combined network hashrate = our hashrate + all live peer hashrates.
    pub fn combined_network_hashrate(&self) -> f64 {
        let now = now_ms();
        let our = {
            let hashes = self.mining_hashes.load(std::sync::atomic::Ordering::Relaxed);
            let started = self.mining_started_ms.load(std::sync::atomic::Ordering::Relaxed);
            let mining = self.is_mining.load(std::sync::atomic::Ordering::Relaxed);
            if mining && started > 0 {
                let elapsed = (now.saturating_sub(started) as f64 / 1000.0).max(0.001);
                hashes as f64 / elapsed
            } else { 0.0 }
        };
        let peers: f64 = self.peer_stats.lock().values()
            .filter(|s| now.saturating_sub(s.last_seen_ms) < 30_000) // only peers seen in last 30s
            .map(|s| s.hashrate)
            .sum();
        our + peers
    }

    /// Process incoming peer stats: update the stats map and register the peer
    /// as a pool miner if this node is the pool operator.
    pub fn apply_peer_stats(&self, stat: PeerMinerStat) {
        // NOTE: we deliberately do NOT credit pool shares from a peer's
        // self-reported P2P stats. Crediting an unverified, attacker-chosen
        // hashrate let any remote node claim the pool payout. Pool shares are now
        // only driven by the operator's authenticated private RPC until they are
        // replaced by verified proof-of-work shares. The stats below are used for
        // (display-only) network-hashrate aggregation, never for payouts.
        // Key by a STABLE identity, NOT the ephemeral ip:port. A miner reconnects
        // every few seconds with a new source port; keying by ip:port would make
        // a fresh entry each time and multi-count its hashrate. Prefer the payout
        // address (one per miner); fall back to the peer IP for non-miners.
        let key = if !stat.payout_address.is_empty() {
            stat.payout_address.clone()
        } else {
            stat.peer_addr.rsplit_once(':').map(|(ip, _)| ip.to_string())
                .unwrap_or_else(|| stat.peer_addr.clone())
        };
        self.peer_stats.lock().insert(key, stat);
    }

    /// Current chain height (blue score of the selected tip).
    pub fn current_height(&self) -> u64 {
        self.dag.get_selected_tip().ok().flatten()
            .and_then(|t| self.dag.get_header(&t).ok().flatten())
            .map(|h| h.blue_score)
            .unwrap_or(0)
    }

    /// Accept a block received from a peer: validate, insert into the DAG,
    /// apply to the UTXO set, and record it. Returns true if newly accepted.
    /// Full consensus validation of a block against our current state. This is
    /// the gate that stops a peer from forging blocks, minting extra coins, or
    /// double-spending. It does NOT mutate any state.
    ///
    /// Checks, in order:
    ///  1. structural sanity (sizes, non-empty, first tx is coinbase)
    ///  2. proof-of-work meets the claimed difficulty target
    ///  3. merkle root matches the transactions
    ///  4. blue_score strictly increases over the heaviest parent (anti
    ///     subsidy-inflation: you can't claim an early/high subsidy by lying)
    ///  5. every non-coinbase input exists (in the UTXO set OR created earlier in
    ///     this same block) and is spent at most once across the whole block
    ///  6. signatures verify; outputs never exceed inputs (fee = inputs−outputs ≥ 0)
    ///  7. the coinbase pays at most subsidy(height) + total fees — never more
    pub fn get_selected_chain(&self, start: Hash) -> Vec<Hash> {
        use borsh::BorshDeserialize;
        let mut chain = Vec::new();
        let mut current = start;
        while current != Hash::ZERO {
            chain.push(current);
            if let Ok(Some(bytes)) = self.dag.get_ghostdag_raw(&current) {
                if let Ok(data) = zentra_consensus::ghostdag::GhostdagData::try_from_slice(&bytes) {
                    current = data.selected_parent;
                    continue;
                }
            }
            if let Ok(Some(header)) = self.dag.get_header(&current) {
                if let Some(p) = header.parents.first() {
                    current = *p;
                } else {
                    current = Hash::ZERO;
                }
            } else {
                break;
            }
        }
        chain
    }

    pub fn calculate_expected_difficulty(&self, parents: &[Hash], lane_id: LaneId) -> Result<u32, String> {
        use borsh::BorshDeserialize;
        let mut engine = zentra_consensus::difficulty::DifficultyEngine::new_with_network(lane_id, self.config.network);

        // Find the heaviest parent to walk back along
        let mut selected_parent = Hash::ZERO;
        let mut max_score = 0;
        for p in parents {
            if let Ok(Some(h)) = self.dag.get_header(p) {
                if h.blue_score >= max_score {
                    max_score = h.blue_score;
                    selected_parent = *p;
                }
            }
        }

        if selected_parent == Hash::ZERO {
            // Genesis region
            return Ok(zentra_consensus::difficulty::DifficultyEngine::genesis_difficulty());
        }

        // Walk back from selected_parent to collect up to 20 blocks in this lane
        let mut history = Vec::new();
        let mut current = selected_parent;

        while current != Hash::ZERO && history.len() < 20 {
            if let Ok(Some(header)) = self.dag.get_header(&current) {
                if header.lane_id == lane_id {
                    history.push((header.timestamp, header.bits));
                }
                // Walk back selected parent from ghostdag
                if let Ok(Some(bytes)) = self.dag.get_ghostdag_raw(&current) {
                    if let Ok(data) = zentra_consensus::ghostdag::GhostdagData::try_from_slice(&bytes) {
                        current = data.selected_parent;
                        continue;
                    }
                }
                // Fallback
                if let Some(p) = header.parents.first() {
                    current = *p;
                } else {
                    current = Hash::ZERO;
                }
            } else {
                break;
            }
        }

        // Push into engine in chronological order
        for (timestamp, bits) in history.into_iter().rev() {
            engine.record_block(timestamp, bits);
        }

        Ok(engine.next_difficulty())
    }

    pub fn validate_block_header(&self, header: &Header) -> Result<(), String> {
        // structural validation
        header.validate_basic().map_err(|e| format!("structure: {e}"))?;

        // proof-of-work target checks
        zentra_consensus::lanes::verify_block_pow(header)
            .map_err(|e| format!("pow: {e}"))?;

        // timestamp checks (future time)
        const MAX_FUTURE_MS: u64 = 2 * 60 * 60 * 1000; // 2 hours
        if header.timestamp > now_ms().saturating_add(MAX_FUTURE_MS) {
            return Err("block timestamp too far in the future".into());
        }

        // difficulty retargeting verification
        let expected_bits = self.calculate_expected_difficulty(&header.parents, header.lane_id)?;
        if header.bits != expected_bits {
            return Err(format!(
                "difficulty target mismatch: block header has {:#010X}, expected {:#010X}",
                header.bits, expected_bits
            ));
        }

        // median-time-past checks
        {
            let mut times: Vec<u64> = Vec::with_capacity(11);
            let mut cursor: Vec<Hash> = header.parents.clone();
            let mut visited: std::collections::HashSet<Hash> = std::collections::HashSet::new();
            while times.len() < 11 {
                let mut next: Option<(Hash, Header)> = None;
                for p in &cursor {
                    if !visited.contains(p) {
                        if let Ok(Some(h)) = self.dag.get_header(p) {
                            if next.as_ref().map_or(true, |(_, nh)| h.blue_score > nh.blue_score) {
                                next = Some((*p, h));
                            }
                        }
                    }
                }
                match next {
                    Some((ph, hdr)) => {
                        visited.insert(ph);
                        times.push(hdr.timestamp);
                        cursor = hdr.parents.clone();
                    }
                    None => break,
                }
            }
            if times.len() >= 11 {
                times.sort_unstable();
                let mtp = times[times.len() / 2];
                if header.timestamp <= mtp {
                    return Err(format!(
                        "block timestamp {} not after median-time-past {}",
                        header.timestamp, mtp));
                }
            }
        }

        Ok(())
    }

    /// Validation that depends ONLY on the block itself (not on which chain it is
    /// on), so it is correct for every accepted block — including non-selected
    /// side blocks. Checks merkle root, coinbase position, signatures on every
    /// non-coinbase input, and intra-block double-spends. UTXO-dependent checks
    /// (ownership, fee/subsidy, coinbase maturity) are applied separately when a
    /// block joins the selected chain in `reorganize`.
    pub fn validate_block_self_contained(&self, block: &Block) -> Result<(), String> {
        use std::collections::HashSet;
        use zentra_core::transaction::OutPoint;

        if !block.validate_merkle_root() {
            return Err("merkle root mismatch".into());
        }
        if block.transactions.is_empty() {
            return Err("block has no transactions".into());
        }

        let mut seen_inputs: HashSet<OutPoint> = HashSet::new();
        for (i, tx) in block.transactions.iter().enumerate() {
            let is_cb = tx.is_coinbase();
            if i == 0 && !is_cb { return Err("first transaction must be coinbase".into()); }
            if i > 0 && is_cb { return Err("only the first transaction may be coinbase".into()); }
            if is_cb { continue; }

            tx.verify_signatures().map_err(|e| format!("signature: {e}"))?;
            for inp in &tx.inputs {
                let op = OutPoint::new(inp.prev_tx_hash, inp.output_index);
                if !seen_inputs.insert(op.clone()) {
                    return Err(format!("double-spend within block: {}:{}", op.tx_hash, op.index));
                }
            }
        }
        Ok(())
    }

    pub fn validate_block_transactions(&self, block: &Block, utxo: &UtxoSet) -> Result<(), String> {
        use std::collections::{HashMap, HashSet};
        use zentra_core::transaction::{OutPoint, TxOutput};

        // Merkle root check
        if !block.validate_merkle_root() {
            return Err("merkle root mismatch".into());
        }

        let height = block.header.blue_score;
        let mut spent: HashSet<OutPoint> = HashSet::new();
        // outpoint -> (amount, owner address) for outputs created earlier in THIS block
        let mut created: HashMap<OutPoint, (u64, Address)> = HashMap::new();
        let mut total_fees: u64 = 0;

        for (i, tx) in block.transactions.iter().enumerate() {
            let is_cb = tx.is_coinbase();
            if i == 0 && !is_cb { return Err("first transaction must be coinbase".into()); }
            if i > 0 && is_cb { return Err("only the first transaction may be coinbase".into()); }

            if !is_cb {
                tx.verify_signatures().map_err(|e| format!("signature: {e}"))?;
                let mut in_sum: u64 = 0;
                for inp in &tx.inputs {
                    let op = OutPoint::new(inp.prev_tx_hash, inp.output_index);
                    if spent.contains(&op) {
                        return Err(format!("double-spend within block: {}:{}", op.tx_hash, op.index));
                    }

                    let (val, owner) = if let Some(e) = utxo.get_utxo(&op) {
                        const COINBASE_MATURITY: u64 = 10;
                        if e.is_coinbase && height < e.block_height.saturating_add(COINBASE_MATURITY) {
                            return Err(format!(
                                "spends immature coinbase (needs {} confirmations)", COINBASE_MATURITY));
                        }
                        (e.amount.as_zents(), e.address.clone())
                    } else if let Some((v, a)) = created.get(&op) {
                        (*v, a.clone())
                    } else {
                        return Err(format!("input not found / already spent: {}:{}", op.tx_hash, op.index));
                    };

                    if Address::from_public_key(&inp.public_key, self.config.network) != owner {
                        return Err(format!("input {}:{} not owned by the signing key", op.tx_hash, op.index));
                    }
                    in_sum = in_sum.saturating_add(val);
                    spent.insert(op);
                }
                // saturating fold (not .sum()) so a crafted peer tx with huge
                // output amounts can't overflow u64 (panic in debug / wrap in
                // release, which could hide outputs > inputs and mint value).
                let out_sum: u64 = tx.outputs.iter().map(|o| match o {
                    TxOutput::Standard { amount, .. } => amount.as_zents(),
                    TxOutput::Burn { amount, .. } => amount.as_zents(),
                }).fold(0u64, |a, b| a.saturating_add(b));
                if out_sum > in_sum {
                    return Err(format!("outputs {} exceed inputs {}", out_sum, in_sum));
                }
                total_fees = total_fees.saturating_add(in_sum - out_sum);
            }

            // Record this tx's standard outputs
            let txid = tx.txid();
            for (idx, o) in tx.outputs.iter().enumerate() {
                if let TxOutput::Standard { address, amount, .. } = o {
                    created.insert(OutPoint::new(txid, idx as u32), (amount.as_zents(), address.clone()));
                }
            }
        }

        // coinbase reward check
        if let Some(cb) = block.transactions.first() {
            if cb.is_coinbase() {
                let subsidy = self.emission.block_reward(height).as_zents();
                let cb_out = cb.total_output_amount().as_zents();
                let cap = subsidy.saturating_add(total_fees);
                if cb_out > cap {
                    return Err(format!(
                        "coinbase {} exceeds subsidy {} + fees {} at height {}",
                        cb_out, subsidy, total_fees, height
                    ));
                }
            }
        }

        Ok(())
    }

    pub fn validate_full_block(&self, block: &Block) -> Result<(), String> {
        self.validate_block_header(&block.header)?;
        let utxo = self.utxo_set.lock();
        self.validate_block_transactions(block, &utxo)?;
        Ok(())
    }

    fn reorganize(&self, old_tip_opt: Option<Hash>, new_tip: Hash) -> Result<(), String> {
        use borsh::BorshDeserialize;
        let old_tip = match old_tip_opt {
            Some(t) => t,
            None => {
                let mut utxo = self.utxo_set.lock();
                let block = self.dag.get_block(&new_tip)
                    .map_err(|e| format!("database error: {e}"))?
                    .ok_or_else(|| format!("missing block data for block {new_tip}"))?;
                self.validate_block_transactions(&block, &utxo)?;
                let undo = utxo.apply_block(&block, block.header.blue_score)
                    .map_err(|e| format!("failed to apply block {new_tip}: {e}"))?;
                self.dag.put_undo(&new_tip, &undo)
                    .map_err(|e| format!("failed to store undo: {e}"))?;
                return Ok(());
            }
        };

        if old_tip == new_tip {
            return Ok(());
        }

        let old_chain = self.get_selected_chain(old_tip);
        let new_chain = self.get_selected_chain(new_tip);

        let mut common_ancestor = Hash::ZERO;
        let mut old_idx = None;
        let mut new_idx = None;

        let old_set: std::collections::HashSet<Hash> = old_chain.iter().copied().collect();
        for (i, hash) in new_chain.iter().enumerate() {
            if old_set.contains(hash) {
                common_ancestor = *hash;
                new_idx = Some(i);
                break;
            }
        }

        if new_idx.is_some() {
            for (i, hash) in old_chain.iter().enumerate() {
                if *hash == common_ancestor {
                    old_idx = Some(i);
                    break;
                }
            }
        }

        let to_disconnect = match old_idx {
            Some(idx) => old_chain[0..idx].to_vec(),
            None => old_chain,
        };

        let mut to_connect = match new_idx {
            Some(idx) => new_chain[0..idx].to_vec(),
            None => new_chain,
        };
        to_connect.reverse();

        let mut utxo = self.utxo_set.lock();
        let mut disconnected_so_far = Vec::new();
        let mut connected_so_far = Vec::new();
        let mut success = true;
        let mut err_reason = String::new();

        for hash in &to_disconnect {
            if let Ok(Some(undo)) = self.dag.get_undo(hash) {
                if let Ok(Some(b)) = self.dag.get_block(hash) {
                    if let Err(e) = utxo.disconnect_block(&b, &undo) {
                        success = false;
                        err_reason = format!("failed to disconnect {hash}: {e}");
                        break;
                    }
                    disconnected_so_far.push(*hash);
                } else {
                    success = false;
                    err_reason = format!("missing block to disconnect: {hash}");
                    break;
                }
            } else {
                success = false;
                err_reason = format!("missing undo data to disconnect: {hash}");
                break;
            }
        }

        if success {
            for hash in &to_connect {
                if let Ok(Some(b)) = self.dag.get_block(hash) {
                    if let Err(e) = self.validate_block_transactions(&b, &utxo) {
                        success = false;
                        err_reason = format!("transaction validation failed for block {hash}: {e}");
                        break;
                    }
                    match utxo.apply_block(&b, b.header.blue_score) {
                        Ok(undo) => {
                            if let Err(e) = self.dag.put_undo(hash, &undo) {
                                success = false;
                                err_reason = format!("failed to store undo for block {hash}: {e}");
                                break;
                            }
                            connected_so_far.push(*hash);
                        }
                        Err(e) => {
                            success = false;
                            err_reason = format!("failed to apply block {hash} to UTXO: {e}");
                            break;
                        }
                    }
                } else {
                    success = false;
                    err_reason = format!("missing block to connect: {hash}");
                    break;
                }
            }
        }

        if !success {
            for hash in connected_so_far.iter().rev() {
                if let Ok(Some(undo)) = self.dag.get_undo(hash) {
                    if let Ok(Some(b)) = self.dag.get_block(hash) {
                        let _ = utxo.disconnect_block(&b, &undo);
                    }
                }
            }
            for hash in disconnected_so_far.iter().rev() {
                if let Ok(Some(b)) = self.dag.get_block(hash) {
                    if let Ok(undo) = utxo.apply_block(&b, b.header.blue_score) {
                        let _ = self.dag.put_undo(hash, &undo);
                    }
                }
            }
            return Err(err_reason);
        }

        Ok(())
    }

    pub fn accept_external_block(&self, block: &Block) -> bool {
        let hash = block.hash();
        if matches!(self.dag.get_block(&hash), Ok(Some(_))) {
            return false;
        }

        // Basic header PoW check to prevent spamming the orphan pool with zero-PoW garbage
        if let Err(e) = zentra_consensus::lanes::verify_block_pow(&block.header) {
            tracing::warn!(hash = %hash, reason = %e, "rejected orphan — invalid PoW");
            return false;
        }

        let mut missing = Vec::new();
        for parent in &block.header.parents {
            if !matches!(self.dag.get_block(parent), Ok(Some(_))) {
                missing.push(*parent);
            }
        }
        if !missing.is_empty() {
            {
                let mut orphans = self.orphans.lock();
                if orphans.len() < 10_000 { orphans.insert(hash, block.clone()); }
            }
            let mut w = self.wanted.lock();
            if w.len() < 10_000 {
                for m in missing { w.insert(m); }
            }
            tracing::debug!(height = block.header.blue_score, "parked orphan — requesting parents");
            return false;
        }
        let connected = self.connect_block(block);
        if connected { self.try_connect_orphans(); }
        connected
    }

    pub fn connect_block(&self, block: &Block) -> bool {
        use borsh::BorshSerialize;
        let hash = block.hash();
        if matches!(self.dag.get_block(&hash), Ok(Some(_))) { return false; }

        // Get old selected tip BEFORE we modify the DAG
        let old_selected_tip = self.dag.get_selected_tip().ok().flatten();

        if let Err(e) = self.validate_block_header(&block.header) {
            tracing::warn!(height = block.header.blue_score, hash = %hash, reason = %e, "rejected invalid peer block header");
            return false;
        }

        // Self-contained transaction validation runs for EVERY accepted block,
        // including side/parallel blocks that don't become the selected tip.
        // Previously these were inserted and relayed with no tx validation at all
        // (full UTXO validation only happened in reorganize when a block joined
        // the selected chain), so a forged-signature/malformed-coinbase side block
        // could propagate network-wide. The UTXO-dependent checks (ownership,
        // fees, maturity) still happen on selection in reorganize.
        if let Err(e) = self.validate_block_self_contained(block) {
            tracing::warn!(height = block.header.blue_score, hash = %hash, reason = %e, "rejected peer block — invalid transactions");
            return false;
        }

        // Compute GhostDAG data
        let get_ghostdag = |h: &Hash| -> Option<zentra_consensus::ghostdag::GhostdagData> {
            if let Ok(Some(bytes)) = self.dag.get_ghostdag_raw(h) {
                borsh::BorshDeserialize::try_from_slice(&bytes).ok()
            } else {
                None
            }
        };
        let manager = zentra_consensus::ghostdag::GhostdagManager::default_k();
        let expected_data = manager.process_block(&hash, &block.header.parents, &get_ghostdag);

        if block.header.blue_score != expected_data.blue_score {
            tracing::warn!(hash = %hash, "rejected block — blue_score mismatch (header has {}, expected {})", block.header.blue_score, expected_data.blue_score);
            return false;
        }
        if block.header.blue_work != expected_data.blue_work {
            tracing::warn!(hash = %hash, "rejected block — blue_work mismatch (header has {}, expected {})", block.header.blue_work, expected_data.blue_work);
            return false;
        }

        // Persist the GhostDAG data FIRST, then the block. Ordering matters for
        // crash-safety: if we die between the two writes, a stray ghostdag entry
        // with no block is harmless (get_block returns None → the block is just
        // re-fetched and the ghostdag overwritten). The reverse — a block with no
        // ghostdag — silently bricks every descendant (they resolve the parent to
        // genesis → blue_score 1 → permanent "blue_score mismatch"). Both writes
        // are mandatory; a failure aborts the connect.
        let gd_bytes = match borsh::to_vec(&expected_data) {
            Ok(b) => b,
            Err(e) => { tracing::error!(err = %e, "failed to serialize ghostdag — block rejected"); return false; }
        };
        if let Err(e) = self.dag.put_ghostdag_raw(&hash, &gd_bytes) {
            tracing::error!(err = %e, "failed to persist ghostdag — block rejected");
            return false;
        }
        if let Err(e) = self.dag.insert_block(block) {
            tracing::debug!(err = %e, "rejected peer block — dag insert failed");
            return false;
        }

        // Get new selected tip AFTER DAG insertion
        let new_selected_tip = match self.dag.get_selected_tip() {
            Ok(Some(t)) => t,
            _ => hash,
        };

        // If the tip changed, reorganize
        if Some(new_selected_tip) != old_selected_tip {
            if let Err(e) = self.reorganize(old_selected_tip, new_selected_tip) {
                tracing::warn!(err = %e, hash = %hash, "reorg to new tip failed — block rejected");
                return false;
            }

            // Update difficulty manager history from the new selected chain
            self.rebuild_difficulty_history(new_selected_tip);

            // Interrupt any active mining loop to prevent mining on stale parents
            if self.is_mining.load(std::sync::atomic::Ordering::Relaxed) {
                self.is_mining.store(false, std::sync::atomic::Ordering::Relaxed);
                // Allow a tiny window for mining threads to notice and abort
                std::thread::sleep(std::time::Duration::from_millis(10));
                self.is_mining.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }

        // Success updates (mempool, history, etc.)
        let txids: Vec<Hash> = block.transactions.iter().map(|t| t.txid()).collect();
        self.mempool.remove_confirmed(&txids);
        let mut h = self.block_history.lock();
        h.push(block.clone());
        if h.len() > 100 { h.remove(0); }
        drop(h);

        // Pool block accounting
        if self.pool_mode.load(std::sync::atomic::Ordering::Relaxed) {
            let pool_addr = self.pool.lock().address.clone();
            let paid_pool = block.transactions.first()
                .filter(|tx| tx.is_coinbase())
                .map(|cb| cb.outputs.iter().any(|o| matches!(o,
                    zentra_core::transaction::TxOutput::Standard { address, .. } if address.to_string() == pool_addr)))
                .unwrap_or(false);
            if paid_pool { self.pool.lock().blocks_found += 1; }
        }

        tracing::info!(height = block.header.blue_score, hash = %hash, "accepted block");
        true
    }

    pub fn try_connect_orphans(&self) {
        loop {
            let ready: Vec<Block> = {
                let orphans = self.orphans.lock();
                orphans.values()
                    .filter(|b| b.header.parents.iter()
                        .all(|p| matches!(self.dag.get_block(p), Ok(Some(_)))))
                    .cloned().collect()
            };
            if ready.is_empty() { break; }
            for b in ready {
                let h = b.hash();
                self.orphans.lock().remove(&h);
                self.wanted.lock().remove(&h);
                self.connect_block(&b);
            }
        }
    }

    fn rebuild_difficulty_history(&self, tip_hash: Hash) {
        use borsh::BorshDeserialize;
        let mut diff = self.difficulty.lock();
        diff.clear();

        let mut blocks_to_record = Vec::new();
        let mut current = tip_hash;

        while current != Hash::ZERO {
            if let Ok(Some(header)) = self.dag.get_header(&current) {
                blocks_to_record.push((header.lane_id, header.timestamp, header.bits));
                if let Ok(Some(bytes)) = self.dag.get_ghostdag_raw(&current) {
                    if let Ok(data) = zentra_consensus::ghostdag::GhostdagData::try_from_slice(&bytes) {
                        current = data.selected_parent;
                        continue;
                    }
                }
                if let Some(p) = header.parents.first() {
                    current = *p;
                } else {
                    current = Hash::ZERO;
                }
            } else {
                break;
            }
        }

        for (lane_id, timestamp, bits) in blocks_to_record.into_iter().rev() {
            diff.record_block(lane_id, timestamp, bits);
        }
    }

    /// Snapshot of current mempool transactions (for P2P relay).
    pub fn mempool_snapshot(&self) -> Vec<zentra_core::transaction::Transaction> {
        self.mempool.get_transactions_for_block(1000)
    }

    /// Accept a transaction relayed from a peer: validate it the same way the
    /// block validator would (signatures, inputs exist & mature, outputs ≤
    /// inputs), compute its fee, and add it to the mempool. Returns true if it
    /// was newly added. This is what lets a pending tx created on one node
    /// (faucet claim, pool payout, a wallet send) reach the miners on OTHER
    /// nodes so it can actually be included in a block.
    pub fn accept_external_tx(&self, tx: zentra_core::transaction::Transaction) -> bool {
        use zentra_core::transaction::{OutPoint, TxOutput};
        let txid = tx.txid();
        if tx.is_coinbase() { return false; }            // coinbase only inside blocks
        if self.mempool.contains(&txid) { return false; } // already have it
        if tx.verify_signatures().is_err() { return false; }

        let cur_h = self.current_height();
        const COINBASE_MATURITY: u64 = 10;
        let utxo = self.utxo_set.lock();
        let mut in_sum: u64 = 0;
        for inp in &tx.inputs {
            let op = OutPoint::new(inp.prev_tx_hash, inp.output_index);
            match utxo.get_utxo(&op) {
                Some(e) => {
                    if e.is_coinbase && cur_h < e.block_height.saturating_add(COINBASE_MATURITY) {
                        return false; // spends immature coinbase — would never validate
                    }
                    // Ownership: the signing key must own the output being spent.
                    if Address::from_public_key(&inp.public_key, self.config.network) != e.address {
                        return false;
                    }
                    in_sum = in_sum.saturating_add(e.amount.as_zents());
                }
                None => return false, // we don't have the input — can't validate/mine it
            }
        }
        drop(utxo);
        let out_sum: u64 = tx.outputs.iter().map(|o| match o {
            TxOutput::Standard { amount, .. } => amount.as_zents(),
            TxOutput::Burn { amount, .. } => amount.as_zents(),
        }).sum();
        if out_sum > in_sum { return false; }
        let fee = in_sum - out_sum;
        if fee < MIN_RELAY_FEE_ZENTS { return false; } // reject dust / zero-fee flood
        self.mempool.add_transaction(tx, Amount::from_zents(fee)).is_ok()
    }

    /// Return the OLDEST `limit` blocks on the selected chain with
    /// blue_score > from_height, in ascending order (parents first). Returning
    /// the oldest slice (not the newest) guarantees a syncing peer can insert
    /// them in order and make progress every round, even across big gaps.
    pub fn blocks_above(&self, from_height: u64, limit: usize) -> Vec<Block> {
        use std::collections::{HashSet, VecDeque};
        // BFS from ALL tips over EVERY parent — not just the first-parent chain —
        // so side/merge blocks travel too. A receiver that only got the
        // first-parent chain would stall the moment a block referenced a side
        // parent it never received. (This is the DAG equivalent of Bitcoin
        // serving every ancestor a peer is missing.)
        let mut seen: HashSet<Hash> = HashSet::new();
        let mut collected: Vec<Block> = Vec::new();
        let mut q: VecDeque<Hash> = self.dag.get_tips().into_iter().collect();
        while let Some(h) = q.pop_front() {
            if !seen.insert(h) { continue; }
            if let Ok(Some(b)) = self.dag.get_block(&h) {
                if b.header.blue_score <= from_height { continue; }
                for p in &b.header.parents { if !seen.contains(p) { q.push_back(*p); } }
                collected.push(b);
            }
        }
        // Ascending by blue_score so a receiver can connect parents-first.
        collected.sort_by_key(|b| b.header.blue_score);
        collected.truncate(limit);
        collected
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
