//! JSON-RPC server for zentrad.

use std::sync::Arc;
use jsonrpsee::server::ServerBuilder;
use jsonrpsee::core::RpcResult;
use jsonrpsee::proc_macros::rpc;
use zentra_types::*;
use zentra_consensus::difficulty::bits_to_difficulty;
use zentra_consensus::miner::physical_core_count;
use crate::node::ZentraNode;
use ed25519_dalek::Signer;

/// Helper to map internal errors to JSON-RPC custom errors.
fn map_rpc_err<E: std::fmt::Display>(e: E) -> jsonrpsee::types::ErrorObjectOwned {
    jsonrpsee::types::ErrorObjectOwned::owned(1, e.to_string(), None::<()>)
}

// ── Faucet parameters ────────────────────────────────────────────────────────
/// Amount handed out per claim: 1 ZTR (1 × 10^8 zents).
const FAUCET_CLAIM_ZENTS: u64 = 1 * 100_000_000;
/// On-chain fee for the faucet payout transaction.
const FAUCET_FEE_ZENTS: u64 = 1000;
/// Per-address cooldown between claims (6 hours).
const FAUCET_COOLDOWN_MS: u64 = 6 * 60 * 60 * 1000;

/// RPC API trait definition.
#[rpc(server)]
pub trait ZentraRpc {
    /// Get the current block DAG info.
    #[method(name = "getDagInfo")]
    async fn get_dag_info(&self) -> RpcResult<serde_json::Value>;

    /// Get a block by hash.
    #[method(name = "getBlockByHash")]
    async fn get_block_by_hash(&self, hash: String) -> RpcResult<serde_json::Value>;

    /// Submit a raw transaction.
    #[method(name = "submitTransaction")]
    async fn submit_transaction(&self, tx_hex: String) -> RpcResult<String>;

    /// Get balance for an address.
    #[method(name = "getBalance")]
    async fn get_balance(&self, address: String) -> RpcResult<u64>;

    /// Get current mining info.
    #[method(name = "getMiningInfo")]
    async fn get_mining_info(&self) -> RpcResult<serde_json::Value>;

    /// Get AMM pool state.
    #[method(name = "getPoolState")]
    async fn get_pool_state(&self) -> RpcResult<serde_json::Value>;

    /// Get network info (peer count, sync status).
    #[method(name = "getNetworkInfo")]
    async fn get_network_info(&self) -> RpcResult<serde_json::Value>;

    /// Start mining dynamically.
    #[method(name = "startMining")]
    async fn start_mining(&self, lane_id: u8, payout_address: String) -> RpcResult<String>;

    /// Stop mining.
    #[method(name = "stopMining")]
    async fn stop_mining(&self) -> RpcResult<String>;

    /// Get mining status.
    #[method(name = "getMiningStatus")]
    async fn get_mining_status(&self) -> RpcResult<serde_json::Value>;

    /// Generate a fresh BIP-39 mnemonic phrase.
    #[method(name = "generateMnemonic")]
    async fn generate_mnemonic(&self) -> RpcResult<String>;

    /// Derive address from mnemonic phrase.
    #[method(name = "deriveAddress")]
    async fn derive_address(&self, mnemonic: String) -> RpcResult<String>;

    /// Construct, sign and submit a transfer transaction.
    #[method(name = "sendTransfer")]
    async fn send_transfer(
        &self,
        from_mnemonic: String,
        to_address_str: String,
        amount_zents: u64,
        fee_zents: u64,
    ) -> RpcResult<String>;

    /// Swap ZTR and zUSD tokens.
    #[method(name = "swapTokens")]
    async fn swap_tokens(
        &self,
        token_in: String,
        amount_in: u64,
        min_amount_out: u64,
    ) -> RpcResult<serde_json::Value>;

    /// Simulate a vault deposit (USDT -> zUSD minting).
    #[method(name = "vaultDeposit")]
    async fn vault_deposit(&self, tx_hash: String, amount: u64) -> RpcResult<serde_json::Value>;

    /// Simulate a vault withdrawal (zUSD burn).
    #[method(name = "vaultWithdraw")]
    async fn vault_withdraw(&self, address: String, amount: u64) -> RpcResult<String>;

    /// Get recent blocks history.
    #[method(name = "getRecentBlocks")]
    async fn get_recent_blocks(&self) -> RpcResult<serde_json::Value>;

    /// Get ALL blocks that involve an address (full-chain wallet history, read
    /// from the persisted DAG so it survives restarts).
    #[method(name = "getAddressBlocks")]
    async fn get_address_blocks(&self, address: String) -> RpcResult<serde_json::Value>;

    /// Stop the node daemon cleanly.
    #[method(name = "stopNode")]
    async fn stop_node(&self) -> RpcResult<String>;

    /// Set the number of active mining threads.
    #[method(name = "setMiningThreads")]
    async fn set_mining_threads(&self, threads: u8) -> RpcResult<String>;

    /// Get transaction details by TxID.
    #[method(name = "getTransaction")]
    async fn get_transaction(&self, txid: String) -> RpcResult<serde_json::Value>;

    /// Get all transactions currently in the mempool.
    #[method(name = "getMempool")]
    async fn get_mempool(&self) -> RpcResult<serde_json::Value>;

    /// Get detailed address information including balance, UTXOs, and transaction history.
    #[method(name = "getAddressDetail")]
    async fn get_address_detail(&self, address: String) -> RpcResult<serde_json::Value>;

    /// Get detailed transaction information including inputs, outputs, and confirmations.
    #[method(name = "getTransactionDetail")]
    async fn get_transaction_detail(&self, txid: String) -> RpcResult<serde_json::Value>;

    /// Get detailed block information including all transactions and mining details.
    #[method(name = "getBlockDetail")]
    async fn get_block_detail(&self, block_id: String) -> RpcResult<serde_json::Value>;

    // ── Mining pool ──────────────────────────────────────────────────────────
    /// Get pool overview: wallet address, fee, payout interval, miners, hashrate.
    #[method(name = "poolGetInfo")]
    async fn pool_get_info(&self) -> RpcResult<serde_json::Value>;

    /// Register a miner address with the pool.
    #[method(name = "poolJoin")]
    async fn pool_join(&self, miner_address: String) -> RpcResult<String>;

    /// Report a miner's current hashrate (heartbeat). Accumulates shares.
    #[method(name = "poolHeartbeat")]
    async fn pool_heartbeat(&self, miner_address: String, hashrate: f64) -> RpcResult<serde_json::Value>;

    /// List all miners with hashrate, shares and share percentage.
    #[method(name = "poolGetMiners")]
    async fn pool_get_miners(&self) -> RpcResult<serde_json::Value>;

    /// Recent payout history.
    #[method(name = "poolGetPayouts")]
    async fn pool_get_payouts(&self) -> RpcResult<serde_json::Value>;

    /// Enable/disable pool-operator mode on this node (mines to pool wallet).
    #[method(name = "poolSetMode")]
    async fn pool_set_mode(&self, enabled: bool) -> RpcResult<String>;

    /// Set the operator address that receives the 1% fee on each payout.
    #[method(name = "poolSetOperatorAddress")]
    async fn pool_set_operator_address(&self, address: String) -> RpcResult<String>;

    /// Connect to a specific pool: set the operator's pool wallet this node mines
    /// into as a member (instead of waiting to learn it from the seed). Empty
    /// string clears it (back to auto-learn).
    #[method(name = "poolSetTarget")]
    async fn pool_set_target(&self, pool_address: String) -> RpcResult<String>;

    /// Manually add a peer (host:port) to connect to.
    #[method(name = "addPeer")]
    async fn add_peer(&self, address: String) -> RpcResult<String>;

    /// Remove a manually-added peer.
    #[method(name = "removePeer")]
    async fn remove_peer(&self, address: String) -> RpcResult<String>;

    // ── Test faucet ──────────────────────────────────────────────────────────
    /// Faucet info: donation address, balance, claim amount, cooldown.
    #[method(name = "faucetInfo")]
    async fn faucet_info(&self) -> RpcResult<serde_json::Value>;

    /// Claim test ZTR from the faucet to the given address.
    #[method(name = "faucetClaim")]
    async fn faucet_claim(&self, address: String) -> RpcResult<serde_json::Value>;
}

/// RPC server implementation.
pub struct RpcServer {
    pub node: Arc<ZentraNode>,
    pub shutdown_tx: tokio::sync::mpsc::Sender<()>,
}

impl RpcServer {
    fn emission_timing_json(&self) -> serde_json::Value {
        let selected_tip = self.node.dag.get_selected_tip().ok().flatten();
        let height = if let Some(ref tip) = selected_tip {
            self.node
                .dag
                .get_header(tip)
                .ok()
                .flatten()
                .map(|h| h.blue_score)
                .unwrap_or(0)
        } else {
            0
        };

        let halving_interval = self.node.emission.halving_interval;
        let blocks_until_halving = self.node.emission.blocks_until_next_halving(height);
        let target_ms = zentra_types::TARGET_BLOCK_TIME_MS;
        let seconds_until_halving = blocks_until_halving as f64 * target_ms as f64 / 1000.0;
        let seconds_per_halving = halving_interval as f64 * target_ms as f64 / 1000.0;

        serde_json::json!({
            "network": format!("{}", self.node.config.network),
            "height": height,
            "target_block_time_ms": target_ms,
            "target_block_time_seconds": target_ms as f64 / 1000.0,
            "target_blocks_per_second": 1000.0 / target_ms as f64,
            "halving_interval_blocks": halving_interval,
            "blocks_until_next_halving": blocks_until_halving,
            "seconds_until_next_halving": seconds_until_halving,
            "days_until_next_halving": seconds_until_halving / 86_400.0,
            "years_until_next_halving": seconds_until_halving / 31_557_600.0,
            "seconds_per_full_halving_epoch": seconds_per_halving,
            "days_per_full_halving_epoch": seconds_per_halving / 86_400.0,
            "years_per_full_halving_epoch": seconds_per_halving / 31_557_600.0,
            "initial_reward_ztr": zentra_types::INITIAL_REWARD_ZENTS as f64 / zentra_types::COIN as f64,
            "max_supply_ztr": zentra_types::MAX_SUPPLY_COINS,
        })
    }
}

#[async_trait::async_trait]
impl ZentraRpcServer for RpcServer {
    async fn get_dag_info(&self) -> RpcResult<serde_json::Value> {
        let selected_tip = self.node.dag.get_selected_tip().ok().flatten();
        let blue_score = if let Some(ref tip) = selected_tip {
            self.node.dag.get_header(tip).ok().flatten().map(|h| h.blue_score).unwrap_or(0)
        } else {
            0
        };
        Ok(serde_json::json!({
            "tips": self.node.dag.get_tips().iter().map(|h| h.to_hex()).collect::<Vec<_>>(),
            "selected_tip": selected_tip.map(|h| h.to_hex()),
            "blue_score": blue_score,
            "network": format!("{}", self.node.config.network),
            "version": env!("CARGO_PKG_VERSION"),
        }))
    }

    async fn get_block_by_hash(&self, hash_str: String) -> RpcResult<serde_json::Value> {
        let hash = Hash::from_hex(&hash_str)
            .map_err(map_rpc_err)?;
        let block_opt = self.node.dag.get_block(&hash)
            .map_err(map_rpc_err)?;
        if let Some(block) = block_opt {
            let txs: Vec<serde_json::Value> = block.transactions.iter().map(|tx| {
                serde_json::json!({
                    "txid": tx.txid().to_hex(),
                    "type": format!("{:?}", tx.tx_type),
                    "outputs": tx.outputs.iter().map(|o| {
                        match o {
                            zentra_core::transaction::TxOutput::Standard { address, amount, .. } => {
                                serde_json::json!({
                                    "type": "standard",
                                    "address": address.to_string(),
                                    "amount": amount.as_zents(),
                                })
                            }
                            zentra_core::transaction::TxOutput::Burn { amount, burn_type } => {
                                serde_json::json!({
                                    "type": "burn",
                                    "amount": amount.as_zents(),
                                    "burn_type": format!("{:?}", burn_type),
                                })
                            }
                        }
                    }).collect::<Vec<_>>()
                })
            }).collect();

            Ok(serde_json::json!({
                "hash": block.hash().to_hex(),
                "version": block.header.version,
                "parents": block.header.parents.iter().map(|p| p.to_hex()).collect::<Vec<_>>(),
                "merkle_root": block.header.merkle_root.to_hex(),
                "timestamp": block.header.timestamp,
                "nonce": block.header.nonce,
                "lane_id": block.header.lane_id as u8,
                "bits": block.header.bits,
                "blue_score": block.header.blue_score,
                "transactions": txs,
                "tx_count": block.transaction_count(),
            }))
        } else {
            Err(map_rpc_err("block not found"))
        }
    }

    async fn submit_transaction(&self, tx_hex: String) -> RpcResult<String> {
        let tx_bytes = hex::decode(tx_hex)
            .map_err(map_rpc_err)?;
        let tx: zentra_core::transaction::Transaction = borsh::from_slice(&tx_bytes)
            .map_err(map_rpc_err)?;
        
        let txid = tx.txid();
        tx.validate_basic()
            .map_err(map_rpc_err)?;
        tx.verify_signatures()
            .map_err(map_rpc_err)?;

        let mut inputs_sum = 0u64;
        {
            let utxos = self.node.utxo_set.lock();
            let current_height = self.node.current_height();
            for input in &tx.inputs {
                let outpoint = zentra_core::transaction::OutPoint::new(input.prev_tx_hash, input.output_index);
                let entry = utxos.get_utxo(&outpoint).ok_or_else(|| {
                    map_rpc_err(format!("input not found / already spent: {}:{}",
                        input.prev_tx_hash, input.output_index))
                })?;
                // OWNERSHIP: the spending key must derive to the address that owns
                // the UTXO. Same rule as block validation — a valid signature over
                // an attacker's own key must NOT spend someone else's coins.
                if Address::from_public_key(&input.public_key, self.node.config.network) != entry.address {
                    return Err(map_rpc_err(format!(
                        "input {}:{} not owned by the signing key",
                        input.prev_tx_hash, input.output_index)));
                }
                
                // COINBASE MATURITY: Spends of coinbase outputs are locked until they mature
                const COINBASE_MATURITY: u64 = 10;
                if entry.is_coinbase && current_height < entry.block_height.saturating_add(COINBASE_MATURITY) {
                    return Err(map_rpc_err(format!(
                        "spends immature coinbase {}:{} (needs {} confirmations, current height {})",
                        input.prev_tx_hash, input.output_index, COINBASE_MATURITY, current_height
                    )));
                }

                inputs_sum = inputs_sum.saturating_add(entry.amount.as_zents());
            }
        }
        let outputs_sum = tx.total_output_amount().as_zents();
        if outputs_sum > inputs_sum {
            return Err(map_rpc_err(format!(
                "outputs {} exceed inputs {}", outputs_sum, inputs_sum)));
        }
        let fee = inputs_sum - outputs_sum;
        if fee < crate::node::MIN_RELAY_FEE_ZENTS {
            return Err(map_rpc_err(format!(
                "fee {} below minimum relay fee {}", fee, crate::node::MIN_RELAY_FEE_ZENTS)));
        }

        self.node.mempool.add_transaction(tx, Amount::from_zents(fee))
            .map_err(map_rpc_err)?;

        Ok(txid.to_hex())
    }

    async fn get_balance(&self, address_str: String) -> RpcResult<u64> {
        let address = Address::from_bech32(&address_str)
            .map_err(map_rpc_err)?;
        // Spendable balance only — excludes coinbase that hasn't matured yet, so
        // the figure shown always matches what can actually be sent.
        let h = self.node.current_height();
        let balance = self.node.utxo_set.lock().get_spendable_balance(&address, h);
        Ok(balance.as_zents())
    }

    async fn get_mining_info(&self) -> RpcResult<serde_json::Value> {
        let is_mining = self.node.is_mining.load(std::sync::atomic::Ordering::Relaxed);
        let lane = self.node.miner_lane.load(std::sync::atomic::Ordering::Relaxed);
        let address = self.node.miner_address.lock().as_ref().map(|a| a.to_string());
        let hashes = self.node.mining_hashes.load(std::sync::atomic::Ordering::Relaxed);
        let blocks = self.node.mined_blocks.load(std::sync::atomic::Ordering::Relaxed);
        let bits = self.node.difficulty.lock().get_next_difficulty(LaneId::Cpu);
        let target = zentra_core::header::Header::target_from_bits(bits);
        let started_ms = self.node.mining_started_ms.load(std::sync::atomic::Ordering::Relaxed);
        let elapsed = if is_mining && started_ms > 0 {
            (crate::node::now_ms().saturating_sub(started_ms) as f64 / 1000.0).max(0.001)
        } else {
            0.0
        };
        let hashrate = if elapsed > 0.0 { hashes as f64 / elapsed } else { 0.0 };
        let difficulty = bits_to_difficulty(bits);
        // Combined network hashrate = our hashrate + all live peers' hashrates via P2P stats.
        let network_hashrate = self.node.combined_network_hashrate()
            .max(if hashrate > 0.0 { hashrate } else { 0.0 });
        let threads = self.node.miner_threads.load(std::sync::atomic::Ordering::Relaxed);
        let max_threads = physical_core_count() as u8;
        Ok(serde_json::json!({
            "is_mining": is_mining,
            "lane": lane,
            "address": address,
            "hashes": hashes,
            "hashrate": hashrate,
            "mined_blocks": blocks,
            "difficulty": difficulty,
            "network_hashrate": network_hashrate,
            "difficulty_bits": format!("0x{:08X}", bits),
            "target": target.to_hex(),
            "target_block_time_ms": zentra_types::TARGET_BLOCK_TIME_MS,
            "emission": self.emission_timing_json(),
            "threads": threads,
            "max_threads": max_threads,
            "lanes": [
                { "id": 0, "algorithm": "Zentra CPU PoW v1", "difficulty": difficulty },
            ]
        }))
    }

    async fn get_pool_state(&self) -> RpcResult<serde_json::Value> {
        let pool = self.node.amm_pool.lock();
        Ok(serde_json::json!({
            "reserve_ztr": pool.reserve_ztr,
            "reserve_zusd": pool.reserve_zusd,
            "total_lp_tokens": pool.total_lp_tokens,
            "total_lp_burned": pool.total_lp_burned,
            "total_volume_ztr": pool.total_volume_ztr,
            "total_fees_captured": pool.total_fees_captured,
        }))
    }

    async fn get_network_info(&self) -> RpcResult<serde_json::Value> {
        let mempool_size = self.node.mempool.size();
        let selected_tip = self.node.dag.get_selected_tip().ok().flatten();
        let blue_score = if let Some(ref tip) = selected_tip {
            self.node.dag.get_header(tip).ok().flatten().map(|h| h.blue_score).unwrap_or(0)
        } else {
            0
        };

        // Build peer list from both manual_peers config and live P2P stats.
        let manual = self.node.manual_peers.lock().clone();
        let live_stats = self.node.peer_stats.lock().clone();
        let now = crate::node::now_ms();
        let mut peers: Vec<serde_json::Value> = Vec::new();
        let mut id = 1u64;
        // Known manual peers
        for addr in &manual {
            let stat = live_stats.get(addr);
            let last_seen = stat.map(|s| now.saturating_sub(s.last_seen_ms)).unwrap_or(99999);
            peers.push(serde_json::json!({
                "id": id,
                "address": addr,
                "version": concat!("/ZentraCore:", env!("CARGO_PKG_VERSION"), "/"),
                "ping_ms": if last_seen < 10_000 { last_seen } else { 0 },
                "height": stat.map(|s| s.height).unwrap_or(blue_score),
                "direction": "manual",
                "hashrate": stat.map(|s| s.hashrate).unwrap_or(0.0),
                "online": last_seen < 30_000,
            }));
            id += 1;
        }
        // Peers seen via P2P that aren't in manual list
        for (addr, stat) in &live_stats {
            if !manual.contains(addr) && now.saturating_sub(stat.last_seen_ms) < 60_000 {
                peers.push(serde_json::json!({
                    "id": id,
                    "address": addr,
                    "version": concat!("/ZentraCore:", env!("CARGO_PKG_VERSION"), "/"),
                    "ping_ms": 0,
                    "height": stat.height,
                    "direction": "inbound",
                    "hashrate": stat.hashrate,
                    "online": true,
                }));
                id += 1;
            }
        }

        Ok(serde_json::json!({
            "peer_count": peers.len(),
            "synced": true,
            "protocol_version": zentra_types::PROTOCOL_VERSION,
            "mempool_size": mempool_size,
            "network_hashrate": self.node.combined_network_hashrate(),
            "peers": peers,
        }))
    }

    async fn start_mining(&self, lane_id: u8, payout_address: String) -> RpcResult<String> {
        if lane_id != 0 {
            return Err(map_rpc_err("Zentra mining is CPU-only right now. Use lane 0."));
        }
        let addr = Address::from_bech32(&payout_address)
            .map_err(map_rpc_err)?;
        self.node.miner_lane.store(lane_id, std::sync::atomic::Ordering::SeqCst);
        *self.node.miner_address.lock() = Some(addr);
        self.node.mining_hashes.store(0, std::sync::atomic::Ordering::SeqCst);
        self.node.mining_started_ms.store(crate::node::now_ms(), std::sync::atomic::Ordering::SeqCst);
        self.node.is_mining.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok("started".to_string())
    }

    async fn stop_mining(&self) -> RpcResult<String> {
        self.node.is_mining.store(false, std::sync::atomic::Ordering::SeqCst);
        Ok("stopped".to_string())
    }

    async fn get_mining_status(&self) -> RpcResult<serde_json::Value> {
        let is_mining = self.node.is_mining.load(std::sync::atomic::Ordering::Relaxed);
        let lane = self.node.miner_lane.load(std::sync::atomic::Ordering::Relaxed);
        let address = self.node.miner_address.lock().as_ref().map(|a| a.to_string());
        let hashes = self.node.mining_hashes.load(std::sync::atomic::Ordering::Relaxed);
        let blocks = self.node.mined_blocks.load(std::sync::atomic::Ordering::Relaxed);
        let bits = self.node.difficulty.lock().get_next_difficulty(LaneId::Cpu);
        let target = zentra_core::header::Header::target_from_bits(bits);
        let started_ms = self.node.mining_started_ms.load(std::sync::atomic::Ordering::Relaxed);
        let elapsed = if is_mining && started_ms > 0 {
            (crate::node::now_ms().saturating_sub(started_ms) as f64 / 1000.0).max(0.001)
        } else {
            0.0
        };
        let hashrate = if elapsed > 0.0 { hashes as f64 / elapsed } else { 0.0 };
        let difficulty = bits_to_difficulty(bits);
        // Combined network hashrate = our hashrate + all live peers' hashrates via P2P stats.
        let network_hashrate = self.node.combined_network_hashrate()
            .max(if hashrate > 0.0 { hashrate } else { 0.0 });
        let threads = self.node.miner_threads.load(std::sync::atomic::Ordering::Relaxed);
        let max_threads = physical_core_count() as u8;
        Ok(serde_json::json!({
            "is_mining": is_mining,
            "lane": lane,
            "address": address,
            "hashes": hashes,
            "hashrate": hashrate,
            "mined_blocks": blocks,
            "difficulty": difficulty,
            "network_hashrate": network_hashrate,
            "difficulty_bits": format!("0x{:08X}", bits),
            "target": target.to_hex(),
            "target_block_time_ms": zentra_types::TARGET_BLOCK_TIME_MS,
            "emission": self.emission_timing_json(),
            "threads": threads,
            "max_threads": max_threads,
        }))
    }

    async fn generate_mnemonic(&self) -> RpcResult<String> {
        use zentra_wallet::keygen::MasterKey;
        let master = MasterKey::generate();
        Ok(master.mnemonic_phrase().to_string())
    }

    async fn derive_address(&self, mnemonic: String) -> RpcResult<String> {
        use zentra_wallet::keygen::MasterKey;
        let master = MasterKey::from_mnemonic(&mnemonic)
            .map_err(map_rpc_err)?;
        let kp = master.derive_keypair(0, 0);
        let addr = kp.address(self.node.config.network);
        Ok(addr.to_string())
    }

    async fn send_transfer(
        &self,
        from_mnemonic: String,
        to_address_str: String,
        amount_zents: u64,
        fee_zents: u64,
    ) -> RpcResult<String> {
        use zentra_wallet::keygen::MasterKey;
        use zentra_core::transaction::{TxInput, TxOutput};

        let network = self.node.config.network;
        let master = MasterKey::from_mnemonic(&from_mnemonic)
            .map_err(map_rpc_err)?;
        
        let kp = master.derive_keypair(0, 0);
        let from_address = kp.address(network);

        let to_address = Address::from_bech32(&to_address_str)
            .map_err(map_rpc_err)?;

        // Only spend MATURE outputs — selecting an immature coinbase would build
        // a transaction every miner rejects, leaving it stuck in the mempool.
        let cur_h = self.node.current_height();
        let utxos = self.node.utxo_set.lock().get_spendable_utxos_for_address(&from_address, cur_h);
        let total_needed = amount_zents + fee_zents;
        let mut selected = Vec::new();
        let mut accumulated = 0;

        for (op, entry) in &utxos {
            selected.push((op.clone(), entry.clone()));
            accumulated += entry.amount.as_zents();
            if accumulated >= total_needed {
                break;
            }
        }

        if accumulated < total_needed {
            return Err(map_rpc_err(format!(
                "Insufficient funds. Need: {} zents, available: {} zents.",
                total_needed, accumulated
            )));
        }

        let inputs: Vec<TxInput> = selected.iter().map(|(op, _)| {
            TxInput {
                prev_tx_hash: op.tx_hash,
                output_index: op.index,
                signature: vec![],
                public_key: kp.public_key_bytes(),
            }
        }).collect();

        let mut outputs = vec![TxOutput::Standard {
            address: to_address,
            amount: Amount::from_zents(amount_zents),
            script: vec![],
        }];

        let change = accumulated - total_needed;
        if change > 0 {
            outputs.push(TxOutput::Standard {
                address: from_address.clone(),
                amount: Amount::from_zents(change),
                script: vec![],
            });
        }

        let mut tx = zentra_core::transaction::Transaction {
            version: 1,
            tx_type: TransactionType::Transfer,
            inputs,
            outputs,
            payload: vec![],
            lock_time: 0,
        };

        let signing_hash = tx.signing_hash();
        let signature = kp.signing_key().sign(signing_hash.as_bytes()).to_bytes().to_vec();
        for input in &mut tx.inputs {
            input.signature = signature.clone();
        }

        let txid = tx.txid();
        self.node.mempool.add_transaction(tx, Amount::from_zents(fee_zents))
            .map_err(map_rpc_err)?;

        Ok(txid.to_hex())
    }

    async fn swap_tokens(
        &self,
        token_in: String,
        amount_in: u64,
        min_amount_out: u64,
    ) -> RpcResult<serde_json::Value> {
        let mut pool = self.node.amm_pool.lock();
        if token_in.to_lowercase() == "ztr" {
            let res = pool.swap_ztr_to_zusd(amount_in as u128)
                .map_err(map_rpc_err)?;
            
            if res.amount_out < min_amount_out as u128 {
                return Err(map_rpc_err("slippage tolerance exceeded"));
            }

            Ok(serde_json::json!({
                "amount_out": res.amount_out,
                "fee_captured": res.fee_captured,
                "new_reserve_a": res.new_reserve_a,
                "new_reserve_b": res.new_reserve_b,
            }))
        } else {
            let res = pool.swap_zusd_to_ztr(amount_in as u128)
                .map_err(map_rpc_err)?;

            if res.amount_out < min_amount_out as u128 {
                return Err(map_rpc_err("slippage tolerance exceeded"));
            }

            Ok(serde_json::json!({
                "amount_out": res.amount_out,
                "fee_captured": res.fee_captured,
                "new_reserve_a": res.new_reserve_a,
                "new_reserve_b": res.new_reserve_b,
            }))
        }
    }

    async fn vault_deposit(&self, tx_hash: String, amount: u64) -> RpcResult<serde_json::Value> {
        use zentra_finance::vault::IngestRequest;
        use zentra_finance::vault::IngestStatus;

        let tx_hash_bytes = hex::decode(&tx_hash).unwrap_or_else(|_| vec![0xAA; 32]);
        let depositor = Address::from_public_key(&[0u8; 32], self.node.config.network);

        let request_idx = {
            let mut vault = self.node.vault.lock();
            let req = IngestRequest {
                external_chain: "ethereum".into(),
                external_tx_hash: tx_hash_bytes.clone(),
                depositor_address: depositor.clone(),
                stablecoin_amount: amount as u128,
                status: IngestStatus::Pending,
            };
            vault.submit_ingest(req)
                .map_err(map_rpc_err)?;
            vault.pending_ingests.len() - 1
        };

        let mut vault = self.node.vault.lock();
        
        // Manually build message
        let mut message = Vec::new();
        message.extend_from_slice(b"ethereum");
        message.extend_from_slice(&tx_hash_bytes);
        message.extend_from_slice(&depositor.payload);
        message.extend_from_slice(&(amount as u128).to_le_bytes());

        let mut sigs = Vec::new();
        for i in 0..3u16 {
            let p = &vault.tss.participants[i as usize];
            let secret_bytes: [u8; 32] = p.secret_share.as_slice().try_into().unwrap();
            let signing_key = ed25519_dalek::SigningKey::from_bytes(&secret_bytes);
            let sig = signing_key.sign(&message);
            sigs.push((i, sig.to_bytes().to_vec()));
        }

        vault.validate_ingest(request_idx, sigs)
            .map_err(map_rpc_err)?;

        let res = vault.mint_zusd(request_idx)
            .map_err(map_rpc_err)?;

        let mut pool = self.node.amm_pool.lock();
        pool.inject_protocol_liquidity(0, res.fee_deducted);

        Ok(serde_json::json!({
            "status": "minted",
            "zusd_minted": res.zusd_minted,
            "fee_deducted": res.fee_deducted,
            "lp_burned": res.lp_burned,
            "new_reserve_zusd": pool.reserve_zusd,
        }))
    }

    async fn vault_withdraw(&self, address: String, amount: u64) -> RpcResult<String> {
        let addr = Address::from_bech32(&address)
            .map_err(map_rpc_err)?;
        let mut vault = self.node.vault.lock();
        vault.burn_zusd(amount as u128, &addr)
            .map_err(map_rpc_err)?;
        Ok("burned".to_string())
    }

    async fn get_recent_blocks(&self) -> RpcResult<serde_json::Value> {
        let network = self.node.config.network;

        // Current tip for confirmation count
        let selected_tip = self.node.dag.get_selected_tip().ok().flatten();
        let tip_score = selected_tip
            .and_then(|tip| self.node.dag.get_header(&tip).ok().flatten())
            .map(|h| h.blue_score)
            .unwrap_or(0);

        let selected_chain: std::collections::HashSet<Hash> = if let Some(tip) = selected_tip {
            self.node.get_selected_chain(tip).into_iter().collect()
        } else {
            std::collections::HashSet::new()
        };

        let history = self.node.block_history.lock();
        let blocks_json: Vec<serde_json::Value> = history.iter().map(|block| {
            let hash = block.hash();
            let is_selected = selected_chain.contains(&hash);
            let txs: Vec<serde_json::Value> = block.transactions.iter().map(|tx| {
                // Derive sender address from public_key in each input
                let inputs_json: Vec<serde_json::Value> = tx.inputs.iter().map(|i| {
                    let mut pk = [0u8; 32];
                    let len = i.public_key.len().min(32);
                    pk[..len].copy_from_slice(&i.public_key[..len]);
                    let sender = Address::from_public_key(&pk, network);
                    serde_json::json!({
                        "sender_address": sender.to_string(),
                        "prev_tx_hash": i.prev_tx_hash.to_hex(),
                        "output_index": i.output_index,
                    })
                }).collect();

                let outputs_json: Vec<serde_json::Value> = tx.outputs.iter().map(|o| {
                    match o {
                        zentra_core::transaction::TxOutput::Standard { address, amount, .. } => {
                            serde_json::json!({
                                "type": "standard",
                                "address": address.to_string(),
                                "amount": amount.as_zents(),
                            })
                        }
                        zentra_core::transaction::TxOutput::Burn { amount, burn_type } => {
                            serde_json::json!({
                                "type": "burn",
                                "amount": amount.as_zents(),
                                "burn_type": format!("{:?}", burn_type),
                            })
                        }
                    }
                }).collect();

                serde_json::json!({
                    "txid": tx.txid().to_hex(),
                    "type": format!("{:?}", tx.tx_type),
                    "inputs": inputs_json,
                    "outputs": outputs_json,
                })
            }).collect();

            let confirmations = tip_score.saturating_sub(block.header.blue_score) + 1;

            serde_json::json!({
                "hash": hash.to_hex(),
                "version": block.header.version,
                "parents": block.header.parents.iter().map(|h| h.to_hex()).collect::<Vec<_>>(),
                "merkle_root": block.header.merkle_root.to_hex(),
                "timestamp": block.header.timestamp,
                "nonce": block.header.nonce,
                "lane_id": block.header.lane_id as u8,
                "bits": block.header.bits,
                "blue_score": block.header.blue_score,
                "confirmations": confirmations,
                "is_selected": is_selected,
                "transactions": txs,
            })
        }).collect();
        Ok(serde_json::json!(blocks_json))
    }

    async fn get_address_blocks(&self, address_str: String) -> RpcResult<serde_json::Value> {
        let address = Address::from_bech32(&address_str).map_err(map_rpc_err)?;
        let network = self.node.config.network;
        let selected_tip = self.node.dag.get_selected_tip().ok().flatten();
        let tip_score = selected_tip
            .and_then(|tip| self.node.dag.get_header(&tip).ok().flatten())
            .map(|h| h.blue_score)
            .unwrap_or(0);
        let selected_chain: std::collections::HashSet<Hash> = if let Some(tip) = selected_tip {
            self.node.get_selected_chain(tip).into_iter().collect()
        } else {
            std::collections::HashSet::new()
        };

        // Walk the WHOLE DAG (BFS from tips over all parents) and keep blocks that
        // involve this address — its coinbase/received outputs or sent inputs.
        // Reads from the persisted chain, so a wallet always sees its full history
        // regardless of restarts.
        let involves = |block: &zentra_core::block::Block| -> bool {
            for tx in &block.transactions {
                for o in &tx.outputs {
                    if let zentra_core::transaction::TxOutput::Standard { address: a, .. } = o {
                        if a == &address { return true; }
                    }
                }
                for i in &tx.inputs {
                    let mut pk = [0u8; 32];
                    let len = i.public_key.len().min(32);
                    pk[..len].copy_from_slice(&i.public_key[..len]);
                    if Address::from_public_key(&pk, network) == address { return true; }
                }
            }
            false
        };

        let mut visited: std::collections::HashSet<Hash> = std::collections::HashSet::new();
        let mut queue: Vec<Hash> = self.node.dag.get_tips();
        let mut matched: Vec<zentra_core::block::Block> = Vec::new();
        let mut scanned = 0usize;
        while let Some(hash) = queue.pop() {
            if !visited.insert(hash) { continue; }
            scanned += 1;
            if scanned > 200_000 || matched.len() >= 1000 { break; }
            if let Ok(Some(block)) = self.node.dag.get_block(&hash) {
                if involves(&block) { matched.push(block.clone()); }
                for p in &block.header.parents {
                    if !visited.contains(p) { queue.push(*p); }
                }
            }
        }
        // Oldest first (same ordering as getRecentBlocks, which the wallet expects).
        matched.sort_by(|a, b| a.header.blue_score.cmp(&b.header.blue_score));

        let blocks_json: Vec<serde_json::Value> = matched.iter().map(|block| {
            let hash = block.hash();
            let is_selected = selected_chain.contains(&hash);
            let txs: Vec<serde_json::Value> = block.transactions.iter().map(|tx| {
                let inputs_json: Vec<serde_json::Value> = tx.inputs.iter().map(|i| {
                    let mut pk = [0u8; 32];
                    let len = i.public_key.len().min(32);
                    pk[..len].copy_from_slice(&i.public_key[..len]);
                    let sender = Address::from_public_key(&pk, network);
                    serde_json::json!({
                        "sender_address": sender.to_string(),
                        "prev_tx_hash": i.prev_tx_hash.to_hex(),
                        "output_index": i.output_index,
                    })
                }).collect();
                let outputs_json: Vec<serde_json::Value> = tx.outputs.iter().map(|o| match o {
                    zentra_core::transaction::TxOutput::Standard { address, amount, .. } =>
                        serde_json::json!({ "type": "standard", "address": address.to_string(), "amount": amount.as_zents() }),
                    zentra_core::transaction::TxOutput::Burn { amount, burn_type } =>
                        serde_json::json!({ "type": "burn", "amount": amount.as_zents(), "burn_type": format!("{:?}", burn_type) }),
                }).collect();
                serde_json::json!({
                    "txid": tx.txid().to_hex(),
                    "type": format!("{:?}", tx.tx_type),
                    "inputs": inputs_json,
                    "outputs": outputs_json,
                })
            }).collect();
            let confirmations = tip_score.saturating_sub(block.header.blue_score) + 1;
            serde_json::json!({
                "hash": hash.to_hex(),
                "version": block.header.version,
                "parents": block.header.parents.iter().map(|h| h.to_hex()).collect::<Vec<_>>(),
                "merkle_root": block.header.merkle_root.to_hex(),
                "timestamp": block.header.timestamp,
                "nonce": block.header.nonce,
                "lane_id": block.header.lane_id as u8,
                "bits": block.header.bits,
                "blue_score": block.header.blue_score,
                "confirmations": confirmations,
                "is_selected": is_selected,
                "transactions": txs,
            })
        }).collect();
        Ok(serde_json::json!(blocks_json))
    }

    async fn stop_node(&self) -> RpcResult<String> {
        tracing::info!("RPC request to stop node received");
        let tx = self.shutdown_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            let _ = tx.send(()).await;
        });
        Ok("stopping".to_string())
    }

    async fn set_mining_threads(&self, threads: u8) -> RpcResult<String> {
        tracing::info!("Setting mining threads to {}", threads);
        self.node.miner_threads.store(threads, std::sync::atomic::Ordering::SeqCst);
        // If mining is currently active, interrupt it so it restarts with the new thread count immediately
        if self.node.is_mining.load(std::sync::atomic::Ordering::SeqCst) {
            self.node.is_mining.store(false, std::sync::atomic::Ordering::SeqCst);
            tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;
            self.node.is_mining.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Ok("success".to_string())
    }

    async fn get_transaction(&self, txid_str: String) -> RpcResult<serde_json::Value> {
        let target_txid = Hash::from_hex(&txid_str)
            .map_err(map_rpc_err)?;

        // Search the block history or walk DAG blocks
        let selected_tip = self.node.dag.get_selected_tip().ok().flatten();
        let mut curr = selected_tip;
        let mut visited = std::collections::HashSet::new();
        let mut queue = Vec::new();
        if let Some(hash) = curr {
            queue.push(hash);
        }

        while let Some(hash) = queue.pop() {
            if !visited.insert(hash) {
                continue;
            }
            if let Ok(Some(block)) = self.node.dag.get_block(&hash) {
                for tx in &block.transactions {
                    if tx.txid() == target_txid {
                        return Ok(serde_json::json!({
                            "txid": tx.txid().to_hex(),
                            "block_hash": block.hash().to_hex(),
                            "block_height": block.header.blue_score,
                            "timestamp": block.header.timestamp,
                            "type": format!("{:?}", tx.tx_type),
                            "inputs": tx.inputs.iter().map(|i| {
                                let mut pubkey_bytes = [0u8; 32];
                                let len = i.public_key.len().min(32);
                                pubkey_bytes[..len].copy_from_slice(&i.public_key[..len]);
                                let sender_address = Address::from_public_key(&pubkey_bytes, self.node.config.network);
                                serde_json::json!({
                                    "prev_tx_hash": i.prev_tx_hash.to_hex(),
                                    "output_index": i.output_index,
                                    "public_key": hex::encode(&i.public_key),
                                    "address": sender_address.to_string()
                                })
                            }).collect::<Vec<_>>(),
                            "outputs": tx.outputs.iter().map(|o| {
                                match o {
                                    zentra_core::transaction::TxOutput::Standard { address, amount, .. } => {
                                        serde_json::json!({
                                            "type": "standard",
                                            "address": address.to_string(),
                                            "amount": amount.as_zents(),
                                        })
                                    }
                                    zentra_core::transaction::TxOutput::Burn { amount, burn_type } => {
                                        serde_json::json!({
                                            "type": "burn",
                                            "amount": amount.as_zents(),
                                            "burn_type": format!("{:?}", burn_type),
                                        })
                                    }
                                }
                            }).collect::<Vec<_>>()
                        }));
                    }
                }
                for parent in &block.header.parents {
                    queue.push(*parent);
                }
            }
        }

        Err(map_rpc_err("transaction not found"))
    }

    async fn get_mempool(&self) -> RpcResult<serde_json::Value> {
        let mempool_txs = self.node.mempool.get_transactions_for_block(1000);
        let txs_json: Vec<serde_json::Value> = mempool_txs.iter().map(|tx| {
            let outputs: Vec<serde_json::Value> = tx.outputs.iter().map(|o| {
                match o {
                    zentra_core::transaction::TxOutput::Standard { address, amount, .. } => {
                        serde_json::json!({
                            "type": "standard",
                            "address": address.to_string(),
                            "amount": amount.as_zents(),
                        })
                    }
                    zentra_core::transaction::TxOutput::Burn { amount, burn_type } => {
                        serde_json::json!({
                            "type": "burn",
                            "amount": amount.as_zents(),
                            "burn_type": format!("{:?}", burn_type),
                        })
                    }
                }
            }).collect();

            serde_json::json!({
                "txid": tx.txid().to_hex(),
                "type": format!("{:?}", tx.tx_type),
                "inputs": tx.inputs.iter().map(|i| {
                    let mut pubkey_bytes = [0u8; 32];
                    let len = i.public_key.len().min(32);
                    pubkey_bytes[..len].copy_from_slice(&i.public_key[..len]);
                    let sender_address = Address::from_public_key(&pubkey_bytes, self.node.config.network);
                    serde_json::json!({
                        "prev_tx_hash": i.prev_tx_hash.to_hex(),
                        "output_index": i.output_index,
                        "public_key": hex::encode(&i.public_key),
                        "address": sender_address.to_string()
                    })
                }).collect::<Vec<_>>(),
                "outputs": outputs,
            })
        }).collect();

        Ok(serde_json::json!(txs_json))
    }

    async fn get_address_detail(&self, address_str: String) -> RpcResult<serde_json::Value> {
        let address = Address::from_bech32(&address_str)
            .map_err(map_rpc_err)?;

        // Get confirmed balance (sum of mature UTXOs)
        let confirmed_balance = self.node.utxo_set.lock().get_balance(&address);

        // Collect UTXOs for this address
        let utxos_data: Vec<serde_json::Value> = {
            let utxo_set = self.node.utxo_set.lock();
            let mut utxos = Vec::new();
            // Note: Since UtxoSet doesn't expose an iterator for a specific address,
            // we'll collect some representative UTXOs
            // In a production system, you'd want to add a method to UtxoSet to query by address
            utxos
        };

        // Get transactions involving this address (last 50)
        let mut address_transactions = Vec::new();
        let selected_tip = self.node.dag.get_selected_tip().ok().flatten();
        let current_height = if let Some(ref tip) = selected_tip {
            self.node
                .dag
                .get_header(tip)
                .ok()
                .flatten()
                .map(|h| h.blue_score)
                .unwrap_or(0)
        } else {
            0
        };

        // Walk the DAG to find transactions
        let mut visited = std::collections::HashSet::new();
        let mut queue = Vec::new();
        if let Some(hash) = selected_tip {
            queue.push(hash);
        }

        let mut total_received: u64 = 0;
        let mut mined_blocks_count: u32 = 0;

        while let Some(hash) = queue.pop() {
            if !visited.insert(hash) {
                continue;
            }
            if let Ok(Some(block)) = self.node.dag.get_block(&hash) {
                // Count blocks mined by this address (check coinbase transaction)
                if !block.transactions.is_empty() {
                    if let Some(first_tx) = block.transactions.first() {
                        if first_tx.tx_type == TransactionType::Coinbase {
                            for output in &first_tx.outputs {
                                if let zentra_core::transaction::TxOutput::Standard { address: out_addr, .. } = output {
                                    if out_addr == &address {
                                        mined_blocks_count += 1;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }

                // Find transactions involving this address
                for tx in &block.transactions {
                    let mut is_involved = false;
                    let mut tx_amount: i64 = 0;
                    let mut direction = "none".to_string();

                    // Check outputs (receiving)
                    for output in &tx.outputs {
                        if let zentra_core::transaction::TxOutput::Standard { address: out_addr, amount, .. } = output {
                            if out_addr == &address {
                                is_involved = true;
                                tx_amount += amount.as_zents() as i64;
                                total_received += amount.as_zents();
                                direction = "receive".to_string();
                            }
                        }
                    }

                    // Check inputs (sending) by deriving sender address
                    for input in &tx.inputs {
                        let mut pubkey_bytes = [0u8; 32];
                        pubkey_bytes.copy_from_slice(&input.public_key);
                        let sender_address = Address::from_public_key(&pubkey_bytes, self.node.config.network);
                        if sender_address == address {
                            is_involved = true;
                            // We'd need to look up the input amount to calculate this correctly
                            // For now, mark as send
                            if direction != "receive".to_string() {
                                direction = "send".to_string();
                            }
                        }
                    }

                    if is_involved && address_transactions.len() < 50 {
                        address_transactions.push(serde_json::json!({
                            "txid": tx.txid().to_hex(),
                            "timestamp": block.header.timestamp,
                            "amount": tx_amount.abs() as u64,
                            "direction": direction,
                        }));
                    }
                }

                for parent in &block.header.parents {
                    queue.push(*parent);
                }
            }
        }

        // Determine address type (P2PKH)
        let address_type = "P2PKH".to_string();

        Ok(serde_json::json!({
            "address": address.to_string(),
            "confirmed_balance": confirmed_balance.as_zents(),
            "pending_balance": 0u64,  // Would require mempool analysis
            "utxos": utxos_data,
            "transactions": address_transactions,
            "total_received": total_received,
            "mined_blocks_count": mined_blocks_count,
            "address_type": address_type,
        }))
    }

    async fn get_transaction_detail(&self, txid_str: String) -> RpcResult<serde_json::Value> {
        let target_txid = Hash::from_hex(&txid_str)
            .map_err(map_rpc_err)?;

        // Get current height for confirmation calculation
        let selected_tip = self.node.dag.get_selected_tip().ok().flatten();
        let current_height = if let Some(ref tip) = selected_tip {
            self.node
                .dag
                .get_header(tip)
                .ok()
                .flatten()
                .map(|h| h.blue_score)
                .unwrap_or(0)
        } else {
            0
        };

        // Search for the transaction
        let mut visited = std::collections::HashSet::new();
        let mut queue = Vec::new();
        if let Some(hash) = selected_tip {
            queue.push(hash);
        }

        while let Some(hash) = queue.pop() {
            if !visited.insert(hash) {
                continue;
            }
            if let Ok(Some(block)) = self.node.dag.get_block(&hash) {
                for tx in &block.transactions {
                    if tx.txid() == target_txid {
                        // Build detailed inputs
                        let inputs: Vec<serde_json::Value> = tx.inputs.iter().map(|i| {
                            let mut pubkey_bytes = [0u8; 32];
                            let len = i.public_key.len().min(32);
                            pubkey_bytes[..len].copy_from_slice(&i.public_key[..len]);
                            let sender_address = Address::from_public_key(&pubkey_bytes, self.node.config.network);

                            // Try to get the amount from the UTXO set (would be more accurate with historical UTXOs)
                            let amount = {
                                let utxo_set = self.node.utxo_set.lock();
                                let outpoint = zentra_core::transaction::OutPoint::new(i.prev_tx_hash, i.output_index);
                                utxo_set.get_utxo(&outpoint).map(|e| e.amount.as_zents()).unwrap_or(0)
                            };

                            serde_json::json!({
                                "previous_txid": i.prev_tx_hash.to_hex(),
                                "previous_output_index": i.output_index,
                                "sender_address": sender_address.to_string(),
                                "amount": amount,
                            })
                        }).collect();

                        // Build outputs
                        let outputs: Vec<serde_json::Value> = tx.outputs.iter().map(|o| {
                            match o {
                                zentra_core::transaction::TxOutput::Standard { address, amount, .. } => {
                                    serde_json::json!({
                                        "recipient_address": address.to_string(),
                                        "amount_zents": amount.as_zents(),
                                    })
                                }
                                zentra_core::transaction::TxOutput::Burn { amount, .. } => {
                                    serde_json::json!({
                                        "recipient_address": "burn".to_string(),
                                        "amount_zents": amount.as_zents(),
                                    })
                                }
                            }
                        }).collect();

                        // Calculate fee
                        let inputs_sum: u64 = inputs.iter().map(|i| i["amount"].as_u64().unwrap_or(0)).sum();
                        let outputs_sum: u64 = tx.total_output_amount().as_zents();
                        let fee_zents = if inputs_sum >= outputs_sum {
                            inputs_sum - outputs_sum
                        } else {
                            0
                        };

                        // Calculate confirmations
                        let block_height = block.header.blue_score;
                        let confirmations = if current_height >= block_height {
                            (current_height - block_height + 1) as u32
                        } else {
                            1u32
                        };

                        return Ok(serde_json::json!({
                            "txid": tx.txid().to_hex(),
                            "timestamp": block.header.timestamp,
                            "block_height": block_height,
                            "confirmations": confirmations,
                            "inputs": inputs,
                            "outputs": outputs,
                            "fee_zents": fee_zents,
                            "tx_size_bytes": borsh::to_vec(tx).unwrap_or_default().len() as u32,
                        }));
                    }
                }
                for parent in &block.header.parents {
                    queue.push(*parent);
                }
            }
        }

        Err(map_rpc_err("transaction not found"))
    }

    async fn get_block_detail(&self, block_id: String) -> RpcResult<serde_json::Value> {
        let mut target_hash: Option<Hash> = None;

        // Try parsing as hash first
        if let Ok(hash) = Hash::from_hex(&block_id) {
            target_hash = Some(hash);
        } else {
            // Try parsing as height
            if let Ok(height) = block_id.parse::<u64>() {
                // Search for block at this height
                let selected_tip = self.node.dag.get_selected_tip().ok().flatten();
                let mut visited = std::collections::HashSet::new();
                let mut queue = Vec::new();
                if let Some(hash) = selected_tip {
                    queue.push(hash);
                }

                while let Some(hash) = queue.pop() {
                    if !visited.insert(hash) {
                        continue;
                    }
                    if let Ok(Some(block)) = self.node.dag.get_block(&hash) {
                        if block.header.blue_score == height {
                            target_hash = Some(hash);
                            break;
                        }
                        for parent in &block.header.parents {
                            queue.push(*parent);
                        }
                    }
                }
            }
        }

        if let Some(hash) = target_hash {
            if let Ok(Some(block)) = self.node.dag.get_block(&hash) {
                // Get miner address from the coinbase transaction
                let mut miner_address_str = "unknown".to_string();
                if !block.transactions.is_empty() {
                    if let Some(first_tx) = block.transactions.first() {
                        if first_tx.tx_type == TransactionType::Coinbase {
                            if let Some(output) = first_tx.outputs.first() {
                                if let zentra_core::transaction::TxOutput::Standard { address, .. } = output {
                                    miner_address_str = address.to_string();
                                }
                            }
                        }
                    }
                }

                // Get transactions list
                let transactions: Vec<String> = block.transactions.iter()
                    .map(|tx| tx.txid().to_hex())
                    .collect();

                // Get difficulty from bits
                let difficulty = zentra_consensus::difficulty::bits_to_difficulty(block.header.bits);

                return Ok(serde_json::json!({
                    "block_hash": block.hash().to_hex(),
                    "height": block.header.blue_score,
                    "timestamp": block.header.timestamp,
                    "miner_address": miner_address_str,
                    "transaction_count": block.transaction_count(),
                    "transactions": transactions,
                    "difficulty": difficulty,
                    "cumulative_work": "0".to_string(),  // Would need to calculate from chain
                }));
            }
        }

        Err(map_rpc_err("block not found"))
    }

    // ── Mining pool ──────────────────────────────────────────────────────────

    async fn pool_get_info(&self) -> RpcResult<serde_json::Value> {
        let pool = self.node.pool.lock();
        let pool_mode = self.node.pool_mode.load(std::sync::atomic::Ordering::Relaxed);
        // Pool wallet balance (accumulated, unpaid rewards).
        let balance = Address::from_bech32(&pool.address)
            .map(|a| self.node.utxo_set.lock().get_balance(&a).as_zents())
            .unwrap_or(0);
        // For member nodes, the authoritative roster lives on the operator (seed).
        // Show the larger of our local count and what the operator reported, and
        // likewise for combined pool hashrate, so every wallet sees the whole pool.
        let learned_miners = self.node.learned_pool_miners.load(std::sync::atomic::Ordering::Relaxed) as usize;
        let active_miners = pool.active_count().max(learned_miners);
        let learned_hash = *self.node.learned_pool_hashrate.lock();
        let total_hashrate = pool.total_hashrate().max(learned_hash);
        // Members display the OPERATOR's shared pool wallet (learned over P2P) so
        // the wallet shows the same pool address as the website/operator.
        let display_pool_addr = {
            let op = self.node.learned_operator_pool.lock().clone();
            if self.node.pool_member.load(std::sync::atomic::Ordering::Relaxed) && !op.is_empty() {
                op
            } else {
                pool.address.clone()
            }
        };
        Ok(serde_json::json!({
            "pool_mode": pool_mode,
            "address": display_pool_addr,
            "operator_address": pool.operator_address,
            "fee_bps": crate::pool::POOL_FEE_BPS,
            "fee_percent": crate::pool::POOL_FEE_BPS as f64 / 100.0,
            "payout_interval_ms": crate::pool::PAYOUT_INTERVAL_MS,
            "min_payout_zents": crate::pool::MIN_PAYOUT_ZENTS,
            "ms_until_payout": pool.ms_until_payout(),
            "active_miners": active_miners,
            "total_miners": pool.miners.len(),
            "total_hashrate": total_hashrate,
            "blocks_found": pool.blocks_found,
            "pending_balance_zents": balance,
            "total_paid_zents": pool.total_paid_zents,
            "total_fees_zents": pool.total_fees_zents,
            "created_ms": pool.created_ms,
        }))
    }

    async fn pool_join(&self, miner_address: String) -> RpcResult<String> {
        Address::from_bech32(&miner_address).map_err(map_rpc_err)?;
        self.node.pool.lock().join(&miner_address);
        // Remember our OWN payout address so P2P stats report it to the operator
        // (who then credits us, not the shared pool wallet), and switch this node
        // into pool-MEMBER mode so the miner pays the operator's shared pool wallet.
        *self.node.pool_member_payout.lock() = miner_address.clone();
        self.node.pool_member.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok("joined".to_string())
    }

    async fn pool_heartbeat(&self, miner_address: String, hashrate: f64) -> RpcResult<serde_json::Value> {
        Address::from_bech32(&miner_address).map_err(map_rpc_err)?;
        let mut pool = self.node.pool.lock();
        pool.heartbeat(&miner_address, hashrate);
        let total = pool.total_shares();
        let m = pool.miners.get(&miner_address);
        let (shares, paid) = m.map(|m| (m.shares, m.total_paid_zents)).unwrap_or((0.0, 0));
        // Estimate our share of the WHOLE pool using the operator-reported total
        // hashrate when available (a member's local pool only knows itself).
        let learned_hash = *self.node.learned_pool_hashrate.lock();
        let share_pct = if learned_hash > 0.0 {
            (hashrate / learned_hash * 100.0).min(100.0)
        } else if total > 0.0 {
            shares / total * 100.0
        } else { 0.0 };
        Ok(serde_json::json!({
            "accepted": true,
            "shares": shares,
            "share_percent": share_pct,
            "total_paid_zents": paid,
            "pool_address": pool.address,
            "ms_until_payout": pool.ms_until_payout(),
        }))
    }

    async fn pool_get_miners(&self) -> RpcResult<serde_json::Value> {
        let pool = self.node.pool.lock();
        let total = pool.total_shares();
        let now = crate::node::now_ms();
        let mut miners: Vec<serde_json::Value> = pool.miners.values().map(|m| {
            let online = now.saturating_sub(m.last_seen_ms) < crate::pool::MINER_TIMEOUT_MS;
            serde_json::json!({
                "address": m.address,
                "hashrate": m.hashrate,
                "shares": m.shares,
                "share_percent": if total > 0.0 { m.shares / total * 100.0 } else { 0.0 },
                "total_paid_zents": m.total_paid_zents,
                "online": online,
                "last_seen_ms": m.last_seen_ms,
                "joined_ms": m.joined_ms,
            })
        }).collect();
        // Sort by share descending.
        miners.sort_by(|a, b| b["shares"].as_f64().unwrap_or(0.0)
            .partial_cmp(&a["shares"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
        Ok(serde_json::json!(miners))
    }

    async fn pool_get_payouts(&self) -> RpcResult<serde_json::Value> {
        let pool = self.node.pool.lock();
        let payouts: Vec<serde_json::Value> = pool.payouts.iter().rev().map(|p| {
            serde_json::json!({
                "timestamp_ms": p.timestamp_ms,
                "total_zents": p.total_zents,
                "fee_zents": p.fee_zents,
                "miner_count": p.miner_count,
            })
        }).collect();
        Ok(serde_json::json!(payouts))
    }

    async fn pool_set_mode(&self, enabled: bool) -> RpcResult<String> {
        self.node.pool_mode.store(enabled, std::sync::atomic::Ordering::SeqCst);
        // Disabling pool mode also leaves member mode (back to solo).
        if !enabled { self.node.pool_member.store(false, std::sync::atomic::Ordering::SeqCst); }
        Ok(if enabled { "pool mode enabled" } else { "pool mode disabled" }.to_string())
    }

    async fn pool_set_operator_address(&self, address: String) -> RpcResult<String> {
        Address::from_bech32(&address).map_err(map_rpc_err)?;
        self.node.pool.lock().operator_address = address.clone();
        // Persist so it survives restarts.
        let _ = std::fs::write(self.node.config.data_dir.join("pool_operator.txt"), &address);
        Ok("operator address set".to_string())
    }

    async fn pool_set_target(&self, pool_address: String) -> RpcResult<String> {
        let addr = pool_address.trim().to_string();
        if addr.is_empty() {
            *self.node.learned_operator_pool.lock() = String::new();
            let _ = std::fs::remove_file(self.node.config.data_dir.join("pool_target.txt"));
            return Ok("pool target cleared".to_string());
        }
        Address::from_bech32(&addr).map_err(map_rpc_err)?;
        *self.node.learned_operator_pool.lock() = addr.clone();
        // Persist so the chosen pool survives restarts.
        let _ = std::fs::write(self.node.config.data_dir.join("pool_target.txt"), &addr);
        Ok("pool target set".to_string())
    }

    async fn add_peer(&self, address: String) -> RpcResult<String> {
        let addr = address.trim().to_string();
        if addr.is_empty() || !addr.contains(':') {
            return Err(map_rpc_err("Peer must be in host:port form, e.g. 1.2.3.4:16110"));
        }
        {
            let mut peers = self.node.manual_peers.lock();
            if !peers.contains(&addr) {
                peers.push(addr.clone());
            }
        }
        // Persist to peers.txt so it survives restarts and is picked up on boot.
        let peers_file = self.node.config.data_dir.join("peers.txt");
        let current = self.node.manual_peers.lock().clone();
        let body = current.join("\n") + "\n";
        let _ = std::fs::write(&peers_file, body);
        tracing::info!(peer = %addr, "manual peer added + persisted");
        Ok(format!("peer {} added", addr))
    }

    async fn remove_peer(&self, address: String) -> RpcResult<String> {
        let addr = address.trim().to_string();
        self.node.manual_peers.lock().retain(|p| p != &addr);
        let peers_file = self.node.config.data_dir.join("peers.txt");
        let current = self.node.manual_peers.lock().clone();
        let body = current.join("\n") + "\n";
        let _ = std::fs::write(&peers_file, body);
        Ok(format!("peer {} removed", addr))
    }

    // ── Test faucet ──────────────────────────────────────────────────────────

    async fn faucet_info(&self) -> RpcResult<serde_json::Value> {
        let balance = Address::from_bech32(&self.node.faucet_address)
            .map(|a| self.node.utxo_set.lock().get_balance(&a).as_zents())
            .unwrap_or(0);
        Ok(serde_json::json!({
            "address": self.node.faucet_address,
            "balance_zents": balance,
            "claim_zents": FAUCET_CLAIM_ZENTS,
            "claims_count": self.node.faucet_total_claims.load(std::sync::atomic::Ordering::Relaxed),
            "cooldown_secs": FAUCET_COOLDOWN_MS / 1000,
        }))
    }

    async fn faucet_claim(&self, address: String) -> RpcResult<serde_json::Value> {
        use zentra_wallet::keygen::MasterKey;
        use zentra_core::transaction::{TxInput, TxOutput, Transaction};

        let to_address = Address::from_bech32(&address).map_err(map_rpc_err)?;

        // Cooldown check (per address)
        {
            let claims = self.node.faucet_claims.lock();
            if let Some(&last) = claims.get(&address) {
                let elapsed = crate::node::now_ms().saturating_sub(last);
                if elapsed < FAUCET_COOLDOWN_MS {
                    let wait_min = (FAUCET_COOLDOWN_MS - elapsed) / 60_000 + 1;
                    return Err(map_rpc_err(format!(
                        "This address already claimed. Try again in ~{} min.", wait_min
                    )));
                }
            }
        }

        // Build the transaction from the faucet wallet
        let master = MasterKey::from_mnemonic(&self.node.faucet_mnemonic).map_err(map_rpc_err)?;
        let kp = master.derive_keypair(0, 0);
        let faucet_addr = Address::from_bech32(&self.node.faucet_address).map_err(map_rpc_err)?;

        let cur_h = self.node.current_height();
        let utxos = self.node.utxo_set.lock().get_spendable_utxos_for_address(&faucet_addr, cur_h);
        let needed = FAUCET_CLAIM_ZENTS + FAUCET_FEE_ZENTS;
        let mut selected = Vec::new();
        let mut acc = 0u64;
        for (op, e) in &utxos {
            selected.push((op.clone(), e.clone()));
            acc += e.amount.as_zents();
            if acc >= needed { break; }
        }
        if acc < needed {
            return Err(map_rpc_err("The faucet is empty right now — please donate to refill it."));
        }

        let inputs: Vec<TxInput> = selected.iter().map(|(op, _)| TxInput {
            prev_tx_hash: op.tx_hash, output_index: op.index,
            signature: vec![], public_key: kp.public_key_bytes(),
        }).collect();

        let mut outputs = vec![TxOutput::Standard {
            address: to_address, amount: Amount::from_zents(FAUCET_CLAIM_ZENTS), script: vec![],
        }];
        let change = acc - needed;
        if change > 0 {
            outputs.push(TxOutput::Standard {
                address: faucet_addr, amount: Amount::from_zents(change), script: vec![],
            });
        }

        let mut tx = Transaction {
            version: 1, tx_type: TransactionType::Transfer,
            inputs, outputs, payload: vec![], lock_time: 0,
        };
        let sh = tx.signing_hash();
        let sig = kp.signing_key().sign(sh.as_bytes()).to_bytes().to_vec();
        for i in &mut tx.inputs { i.signature = sig.clone(); }

        let txid = tx.txid();
        self.node.mempool.add_transaction(tx, Amount::from_zents(FAUCET_FEE_ZENTS))
            .map_err(map_rpc_err)?;

        self.node.faucet_claims.lock().insert(address, crate::node::now_ms());
        self.node.faucet_total_claims.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        Ok(serde_json::json!({
            "txid": txid.to_hex(),
            "amount_zents": FAUCET_CLAIM_ZENTS,
        }))
    }
}

use tower::{Layer, Service};
use std::task::{Context, Poll};
use hyper::{Request, Response, StatusCode};
use std::future::Future;
use std::pin::Pin;

#[derive(Clone)]
pub struct AuthLayer {
    token: String,
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthService {
            inner,
            token: self.token.clone(),
        }
    }
}

#[derive(Clone)]
pub struct AuthService<S> {
    inner: S,
    token: String,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for AuthService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    ReqBody: Send + 'static,
    ResBody: Default + Send + 'static,
{
    type Response = Response<ResBody>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let auth_header = req.headers().get("authorization")
            .and_then(|h| h.to_str().ok());
        
        let expected_auth = format!("Bearer {}", self.token);
        if auth_header == Some(expected_auth.as_str()) {
            let mut inner = self.inner.clone();
            Box::pin(async move {
                inner.call(req).await
            })
        } else {
            Box::pin(async move {
                let res = Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .body(ResBody::default())
                    .unwrap();
                Ok(res)
            })
        }
    }
}

/// Start the RPC server on the given port.
pub async fn start_rpc_server(
    port: u16,
    node: Arc<ZentraNode>,
    shutdown_tx: tokio::sync::mpsc::Sender<()>,
) -> anyhow::Result<()> {
    // Read the RPC auth token from the data directory
    let token_path = node.config.data_dir.join("rpc_auth.token");
    let token = std::fs::read_to_string(&token_path)
        .map(|t| t.trim().to_string())
        .unwrap_or_else(|_| "invalid_token".to_string());

    let server = ServerBuilder::default()
        .set_http_middleware(tower::ServiceBuilder::new().layer(AuthLayer { token }))
        .build(format!("127.0.0.1:{}", port))
        .await?;

    let addr = server.local_addr()?;
    tracing::info!(addr = %addr, "private JSON-RPC server started (localhost, with token auth)");

    let rpc_impl = RpcServer { node, shutdown_tx };
    let handle = server.start(rpc_impl.into_rpc());
    handle.stopped().await;
    Ok(())
}
