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

        // The pool ADDRESS is always the single shared pool wallet, so every
        // miner on every node pools into the same pot. The local `pool_mnemonic`
        // (from this node's pool_wallet.txt) only matters for the operator: a
        // node can pay the pool out only if its seed derives to this address,
        // which is true exactly on the VPS that holds the real pool seed.
        let shared_pool_address = crate::pool::DEFAULT_POOL_ADDRESS.to_string();
        let is_pool_operator = pool_address == shared_pool_address;
        let mut pool_inner = MiningPool::new(pool_mnemonic, shared_pool_address.clone());
        // Load a previously-saved operator fee address, if any.
        if let Ok(op) = std::fs::read_to_string(config.data_dir.join("pool_operator.txt")) {
            let op = op.trim().to_string();
            if !op.is_empty() { pool_inner.operator_address = op; }
        }
        let _ = is_pool_operator; // (informational; payout is guarded at spend time)
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
                    let txs = self_clone.mempool.get_transactions_for_block(10);
                    let fees = txs
                        .iter()
                        .filter_map(|tx| self_clone.mempool.get_fee(&tx.txid()))
                        .fold(Amount::ZERO, |acc, fee| acc.saturating_add(fee));
                    let lane_u8 = self_clone.miner_lane.load(std::sync::atomic::Ordering::Relaxed);
                    let lane = LaneId::from_u8(lane_u8).unwrap_or(LaneId::Cpu);

                    let height = if let Ok(Some(selected_tip)) = self_clone.dag.get_selected_tip() {
                        if let Ok(Some(header)) = self_clone.dag.get_header(&selected_tip) {
                            header.blue_score + 1
                        } else {
                            1
                        }
                    } else {
                        1
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
                        height,
                        fees,
                        &self_clone.emission,
                    );

                    // Mine block
                    let threads = self_clone.miner_threads.load(std::sync::atomic::Ordering::Relaxed) as usize;
                    let found = miner.mine_block(&mut template, threads);

                    if found {
                        // Self-check our own block through the SAME consensus gate
                        // peers will use. If a race made it invalid (e.g. a tx we
                        // included was just spent by an incoming block), drop it
                        // instead of mining an invalid block onto the chain.
                        if let Err(e) = self_clone.validate_full_block(&template) {
                            tracing::warn!(err = %e, "discarding self-mined block — failed validation");
                        } else {
                            // Apply to the UTXO set FIRST; only commit to the DAG
                            // if that succeeds, so the two never diverge.
                            let utxo_ok = self_clone.utxo_set.lock().apply_block(&template, height).is_ok();
                            if !utxo_ok {
                                tracing::error!("self-mined block failed UTXO apply — discarded");
                            } else if let Err(e) = self_clone.dag.insert_block(&template) {
                                tracing::error!(err = %e, "Failed to insert mined block into DAG");
                            } else {
                                // Record block in difficulty manager to adjust mining speed
                                self_clone.difficulty.lock().record_block(
                                    template.header.lane_id,
                                    template.header.timestamp,
                                    template.header.bits,
                                );
                                // Success! Clean mempool
                                let txids: Vec<Hash> = template.transactions.iter().map(|t| t.txid()).collect();
                                self_clone.mempool.remove_confirmed(&txids);

                                // Add to block history
                                let mut history = self_clone.block_history.lock();
                                history.push(template.clone());
                                if history.len() > 100 {
                                    history.remove(0);
                                }
                                self_clone.mined_blocks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                                // Credit the pool if mining in pool-operator mode.
                                if self_clone.pool_mode.load(std::sync::atomic::Ordering::Relaxed) {
                                    self_clone.pool.lock().blocks_found += 1;
                                }

                                // Announce the new block to all peers for fast propagation.
                                crate::p2p_sync::broadcast_block(&self_clone, &template);

                                info!(
                                    height,
                                    hash = %template.hash(),
                                    txs = template.transaction_count(),
                                    "Mined new block!"
                                );
                            }
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
        // Register as pool miner if we're running pool-operator mode.
        if self.pool_mode.load(std::sync::atomic::Ordering::Relaxed)
            && stat.pool_mining && !stat.payout_address.is_empty()
        {
            let mut pool = self.pool.lock();
            pool.heartbeat(&stat.payout_address, stat.hashrate);
        }
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
    pub fn validate_full_block(&self, block: &Block) -> Result<(), String> {
        use std::collections::{HashMap, HashSet};
        use zentra_core::transaction::{OutPoint, TxOutput};

        // 1. structure
        block.validate_basic().map_err(|e| format!("structure: {e}"))?;

        // 2. proof-of-work
        zentra_consensus::lanes::verify_block_pow(&block.header)
            .map_err(|e| format!("pow: {e}"))?;

        // 2b. difficulty floor — the claimed target may not be EASIER than the
        // network minimum. (Full retarget-window validation is required before
        // mainnet; this floor blocks the trivial "mine at easiest bits" abuse.)
        let floor_target = Header::target_from_bits(Header::easiest_bits());
        let block_target = Header::target_from_bits(block.header.bits);
        if block_target > floor_target {
            return Err("difficulty easier than the network floor".into());
        }

        // 2c. timestamp may not be too far in the future (anti time-warp).
        const MAX_FUTURE_MS: u64 = 2 * 60 * 60 * 1000; // 2 hours
        if block.header.timestamp > now_ms().saturating_add(MAX_FUTURE_MS) {
            return Err("block timestamp too far in the future".into());
        }

        // 3. merkle root
        if !block.validate_merkle_root() {
            return Err("merkle root mismatch".into());
        }

        // 4. blue_score must strictly exceed the heaviest known parent
        let mut max_parent_score = 0u64;
        let mut saw_parent = false;
        for p in &block.header.parents {
            if let Ok(Some(h)) = self.dag.get_header(p) {
                saw_parent = true;
                max_parent_score = max_parent_score.max(h.blue_score);
            }
        }
        if saw_parent && block.header.blue_score <= max_parent_score {
            return Err(format!(
                "blue_score {} not greater than parent {}",
                block.header.blue_score, max_parent_score
            ));
        }
        let height = block.header.blue_score;

        // 5/6. per-transaction UTXO + signature + fee checks
        let utxo = self.utxo_set.lock();
        let mut spent: HashSet<OutPoint> = HashSet::new();
        let mut created: HashMap<OutPoint, u64> = HashMap::new();
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
                    let val = if let Some(e) = utxo.get_utxo(&op) {
                        // Coinbase outputs are locked until they mature, exactly
                        // like Bitcoin's 100-confirmation coinbase rule.
                        const COINBASE_MATURITY: u64 = 10;
                        if e.is_coinbase && height < e.block_height.saturating_add(COINBASE_MATURITY) {
                            return Err(format!(
                                "spends immature coinbase (needs {} confirmations)", COINBASE_MATURITY));
                        }
                        e.amount.as_zents()
                    } else if let Some(v) = created.get(&op) {
                        *v
                    } else {
                        return Err(format!("input not found / already spent: {}:{}", op.tx_hash, op.index));
                    };
                    in_sum = in_sum.saturating_add(val);
                    spent.insert(op);
                }
                let out_sum: u64 = tx.outputs.iter().map(|o| match o {
                    TxOutput::Standard { amount, .. } => amount.as_zents(),
                    TxOutput::Burn { amount, .. } => amount.as_zents(),
                }).sum();
                if out_sum > in_sum {
                    return Err(format!("outputs {} exceed inputs {}", out_sum, in_sum));
                }
                total_fees = total_fees.saturating_add(in_sum - out_sum);
            }

            // Record this tx's standard outputs so a later tx in the same block
            // may legitimately spend them.
            let txid = tx.txid();
            for (idx, o) in tx.outputs.iter().enumerate() {
                if let TxOutput::Standard { amount, .. } = o {
                    created.insert(OutPoint::new(txid, idx as u32), amount.as_zents());
                }
            }
        }
        drop(utxo);

        // 7. coinbase cannot mint more than subsidy + fees
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

    pub fn accept_external_block(&self, block: &Block) -> bool {
        let hash = block.hash();
        // Already known?
        if matches!(self.dag.get_block(&hash), Ok(Some(_))) {
            return false;
        }
        // Reject blocks whose parents we don't have yet — prevents chain GAPS.
        // The sync loop delivers blocks in ascending order, so the parent will
        // already be present for a valid in-order block.
        for parent in &block.header.parents {
            if !matches!(self.dag.get_block(parent), Ok(Some(_))) {
                tracing::debug!(height = block.header.blue_score, "deferring block — parent not yet present");
                return false;
            }
        }

        // FULL consensus validation BEFORE we commit anything. A block that fails
        // here is dropped entirely — it never enters the DAG or the UTXO set.
        if let Err(e) = self.validate_full_block(block) {
            tracing::warn!(height = block.header.blue_score, hash = %hash, reason = %e, "rejected invalid peer block");
            return false;
        }

        let height = block.header.blue_score;
        // Apply to the UTXO set FIRST. If this fails (e.g. a double-spend that
        // slipped past the read-only check due to a race), we do NOT insert the
        // block into the DAG, keeping DAG and UTXO state consistent.
        if let Err(e) = self.utxo_set.lock().apply_block(block, height) {
            tracing::warn!(err = %e, hash = %hash, "rejected peer block — utxo apply failed");
            return false;
        }
        if let Err(e) = self.dag.insert_block(block) {
            tracing::debug!(err = %e, "rejected peer block — dag insert failed");
            return false;
        }
        self.difficulty.lock().record_block(
            block.header.lane_id,
            block.header.timestamp,
            block.header.bits,
        );
        let txids: Vec<Hash> = block.transactions.iter().map(|t| t.txid()).collect();
        self.mempool.remove_confirmed(&txids);
        let mut h = self.block_history.lock();
        h.push(block.clone());
        if h.len() > 100 { h.remove(0); }
        drop(h);

        // Pool block accounting: if we're the operator and this block's coinbase
        // paid the shared pool wallet, count it as a pool block (remote miners
        // find these, so the local mining worker never sees them).
        if self.pool_mode.load(std::sync::atomic::Ordering::Relaxed) {
            let pool_addr = self.pool.lock().address.clone();
            let paid_pool = block.transactions.first()
                .filter(|tx| tx.is_coinbase())
                .map(|cb| cb.outputs.iter().any(|o| matches!(o,
                    zentra_core::transaction::TxOutput::Standard { address, .. } if address.to_string() == pool_addr)))
                .unwrap_or(false);
            if paid_pool { self.pool.lock().blocks_found += 1; }
        }

        tracing::info!(height, hash = %hash, "accepted block from peer");
        true
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
        self.mempool.add_transaction(tx, Amount::from_zents(fee)).is_ok()
    }

    /// Return the OLDEST `limit` blocks on the selected chain with
    /// blue_score > from_height, in ascending order (parents first). Returning
    /// the oldest slice (not the newest) guarantees a syncing peer can insert
    /// them in order and make progress every round, even across big gaps.
    pub fn blocks_above(&self, from_height: u64, limit: usize) -> Vec<Block> {
        let mut all: Vec<Block> = Vec::new();
        let mut cur = self.dag.get_selected_tip().ok().flatten();
        while let Some(h) = cur {
            match self.dag.get_block(&h) {
                Ok(Some(b)) => {
                    if b.header.blue_score <= from_height { break; }
                    let parent = b.header.parents.first().copied();
                    all.push(b);
                    cur = parent;
                }
                _ => break,
            }
        }
        all.reverse();          // ascending: oldest first
        all.truncate(limit);    // keep the oldest `limit` above from_height
        all
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
