//! Minimal but real peer-to-peer block sync over TCP.
//!
//! This is a pragmatic, dependency-light networking layer that lets two (or
//! more) Zentra nodes on the same network — or across the internet with the
//! listener port reachable — discover each other's blocks and stay in sync.
//!
//! ## Protocol (length-prefixed JSON frames)
//!
//! Each frame on the wire is: `[u32 big-endian length][JSON payload]`.
//! Blocks are Borsh-serialized and hex-encoded inside the JSON.
//!
//!   hello     {"t":"hello","net":"devnet","genesis":"<hex>","height":N}
//!   getblocks {"t":"getblocks","from":N}
//!   blocks    {"t":"blocks","blocks":["<hexborsh>",...]}
//!   newblock  {"t":"newblock","block":"<hexborsh>"}
//!
//! ## Model
//!
//! - Every node runs a **listener** that serves blocks on request and accepts
//!   announced blocks.
//! - For every configured peer, a **dialer** periodically connects, performs a
//!   genesis handshake, asks for blocks above its current height, and inserts
//!   them. Add the same peer on both sides for bidirectional sync.
//!
//! Peers come from `node.manual_peers` (populated from `peers.txt`, the
//! `--connect` flag, and the `addPeer` RPC).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;
use serde_json::json;
use tracing::{info, debug};
use crate::node::{ZentraNode, PeerMinerStat};

const MAX_FRAME: usize = 64 * 1024 * 1024; // 64 MB safety cap
const SYNC_BATCH: usize = 500;

/// Baked-in seed nodes that a freshly-downloaded wallet auto-connects to.
/// These are the always-on bootstrap nodes for the network — put your seed
/// server's public IP(s) here (port 16110 must be reachable / port-forwarded).
/// Until a public seed is set, users can still connect manually via the
/// wallet's "Add Peer" box, a `peers.txt` file, or the `--connect` flag.
pub const DEFAULT_SEED_PEERS: &[&str] = &[
    "5.230.155.52:16110",
];

// ── framing ──────────────────────────────────────────────────────────────────

fn write_frame(s: &mut TcpStream, payload: &[u8]) -> std::io::Result<()> {
    s.write_all(&(payload.len() as u32).to_be_bytes())?;
    s.write_all(payload)?;
    s.flush()
}

fn read_frame(s: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    s.read_exact(&mut len)?;
    let n = u32::from_be_bytes(len) as usize;
    if n > MAX_FRAME {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut buf = vec![0u8; n];
    s.read_exact(&mut buf)?;
    Ok(buf)
}

fn send_json(s: &mut TcpStream, v: &serde_json::Value) -> std::io::Result<()> {
    write_frame(s, v.to_string().as_bytes())
}

fn recv_json(s: &mut TcpStream) -> std::io::Result<serde_json::Value> {
    let buf = read_frame(s)?;
    serde_json::from_slice(&buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

fn encode_block(b: &zentra_core::block::Block) -> String {
    hex::encode(borsh::to_vec(b).unwrap_or_default())
}

fn decode_block(hexs: &str) -> Option<zentra_core::block::Block> {
    let bytes = hex::decode(hexs).ok()?;
    borsh::from_slice::<zentra_core::block::Block>(&bytes).ok()
}

fn encode_tx(t: &zentra_core::transaction::Transaction) -> String {
    hex::encode(borsh::to_vec(t).unwrap_or_default())
}

fn decode_tx(hexs: &str) -> Option<zentra_core::transaction::Transaction> {
    let bytes = hex::decode(hexs).ok()?;
    borsh::from_slice::<zentra_core::transaction::Transaction>(&bytes).ok()
}

// ── listener (server) ────────────────────────────────────────────────────────

/// Start the inbound P2P listener. Serves block requests and accepts announces.
pub fn start_listener(node: Arc<ZentraNode>, port: u16) {
    std::thread::spawn(move || {
        let listener = match TcpListener::bind(("0.0.0.0", port)) {
            Ok(l) => l,
            Err(e) => { tracing::error!(err = %e, port, "P2P listener bind failed"); return; }
        };
        info!(port, "P2P listener started");
        for stream in listener.incoming() {
            match stream {
                Ok(s) => {
                    let node = Arc::clone(&node);
                    std::thread::spawn(move || {
                        if let Err(e) = handle_inbound(node, s) {
                            debug!(err = %e, "inbound peer closed");
                        }
                    });
                }
                Err(e) => debug!(err = %e, "accept failed"),
            }
        }
    });
}

fn handle_inbound(node: Arc<ZentraNode>, mut s: TcpStream) -> std::io::Result<()> {
    s.set_read_timeout(Some(Duration::from_secs(120)))?;
    let peer = s.peer_addr().map(|a| a.to_string()).unwrap_or_default();
    let our_net = node.config.network.to_string();
    let our_genesis = node.genesis_hash.to_hex();

    loop {
        let msg = recv_json(&mut s)?;
        match msg["t"].as_str().unwrap_or("") {
            "hello" => {
                if msg["net"].as_str() != Some(our_net.as_str())
                    || msg["genesis"].as_str() != Some(our_genesis.as_str())
                {
                    debug!(%peer, "peer genesis/network mismatch — dropping");
                    let _ = send_json(&mut s, &json!({"t":"bye","reason":"genesis mismatch"}));
                    return Ok(());
                }
                send_json(&mut s, &json!({
                    "t":"hello","net":our_net,"genesis":our_genesis,
                    "height": node.current_height()
                }))?;
            }
            "getblocks" => {
                let from = msg["from"].as_u64().unwrap_or(0);
                let blocks = node.blocks_above(from, SYNC_BATCH);
                let hexes: Vec<String> = blocks.iter().map(encode_block).collect();
                send_json(&mut s, &json!({"t":"blocks","blocks":hexes}))?;
            }
            "newblock" => {
                if let Some(b) = msg["block"].as_str().and_then(decode_block) {
                    node.accept_external_block(&b);
                }
            }
            // Peer-exchange (like Bitcoin's `addr` message): peer sends us addresses
            // of OTHER nodes it knows about. We add any new ones to our peer list.
            // This lets the network self-discover — wallets find nodes organically.
            "addrs" => {
                if let Some(arr) = msg["addrs"].as_array() {
                    let mut mp = node.manual_peers.lock();
                    let added: Vec<String> = arr.iter()
                        .filter_map(|v| v.as_str())
                        .filter(|a| !a.is_empty() && a.contains(':') && !mp.contains(&a.to_string()))
                        .map(|a| a.to_string())
                        .take(50) // cap to avoid abuse
                        .collect();
                    for a in &added { mp.push(a.clone()); }
                    if !added.is_empty() {
                        tracing::info!(count = added.len(), "peer-exchange: discovered new peers");
                    }
                }
            }
            // Mining stats broadcast: peer tells us their hashrate + pool status.
            // We aggregate this into combined network hashrate + pool accounting.
            "stats" => {
                let stat = PeerMinerStat {
                    peer_addr: peer.clone(),
                    hashrate: msg["hashrate"].as_f64().unwrap_or(0.0),
                    height: msg["height"].as_u64().unwrap_or(0),
                    pool_mining: msg["pool_mining"].as_bool().unwrap_or(false),
                    payout_address: msg["payout_address"].as_str().unwrap_or("").to_string(),
                    last_seen_ms: crate::node::now_ms(),
                };
                node.apply_peer_stats(stat);
                // Reply with our own stats so they can see us too.
                send_json(&mut s, &json!({
                    "t":"stats_ack",
                    "hashrate": node.combined_network_hashrate(),
                    "peer_count": node.peer_stats.lock().len(),
                    "pool_active_miners": node.pool.lock().active_count(),
                }))?;
            }
            // Transaction relay: a peer asks for our pending txs.
            "getmempool" => {
                let txs: Vec<String> = node.mempool_snapshot().iter().map(encode_tx).collect();
                send_json(&mut s, &json!({"t":"mempool","txs":txs}))?;
            }
            // Transaction relay: a peer pushes us pending txs to add to our mempool.
            "addtxs" => {
                if let Some(arr) = msg["txs"].as_array() {
                    let mut added = 0u32;
                    for h in arr {
                        if let Some(tx) = h.as_str().and_then(decode_tx) {
                            if node.accept_external_tx(tx) { added += 1; }
                        }
                    }
                    if added > 0 { debug!(added, "accepted relayed transactions"); }
                }
                send_json(&mut s, &json!({"t":"addtxs_ack"}))?;
            }
            "bye" => return Ok(()),
            _ => {}
        }
    }
}

// ── dialer (client) ──────────────────────────────────────────────────────────

/// Start the outbound dialer. Periodically syncs from every configured peer.
pub fn start_dialer(node: Arc<ZentraNode>) {
    std::thread::spawn(move || {
        info!("P2P dialer started");
        loop {
            let peers = node.manual_peers.lock().clone();
            for addr in peers {
                let n = Arc::clone(&node);
                let a = addr.clone();
                // Each peer gets its own short-lived sync attempt.
                std::thread::spawn(move || {
                    if let Err(e) = sync_from_peer(&n, &a) {
                        debug!(peer = %a, err = %e, "peer sync failed");
                    }
                });
            }
            std::thread::sleep(Duration::from_secs(6));
        }
    });
}

/// Announce a freshly-produced block to every known peer (fire-and-forget).
/// Gives near-instant propagation on top of the periodic poll-based sync.
pub fn broadcast_block(node: &Arc<ZentraNode>, block: &zentra_core::block::Block) {
    let peers = node.manual_peers.lock().clone();
    if peers.is_empty() { return; }
    let hexb = encode_block(block);
    let net = node.config.network.to_string();
    let genesis = node.genesis_hash.to_hex();
    for addr in peers {
        let (hexb, net, genesis) = (hexb.clone(), net.clone(), genesis.clone());
        std::thread::spawn(move || {
            let sa = match addr.parse() { Ok(a) => a, Err(_) => return };
            if let Ok(mut s) = TcpStream::connect_timeout(&sa, Duration::from_secs(4)) {
                let _ = s.set_write_timeout(Some(Duration::from_secs(5)));
                let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
                let _ = send_json(&mut s, &json!({"t":"hello","net":net,"genesis":genesis,"height":0}));
                let _ = recv_json(&mut s); // consume their hello
                let _ = send_json(&mut s, &json!({"t":"newblock","block":hexb}));
            }
        });
    }
}

fn sync_from_peer(node: &Arc<ZentraNode>, addr: &str) -> std::io::Result<()> {
    let mut s = TcpStream::connect_timeout(
        &addr.parse().map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "bad peer addr"))?,
        Duration::from_secs(5),
    )?;
    s.set_read_timeout(Some(Duration::from_secs(30)))?;
    s.set_write_timeout(Some(Duration::from_secs(15)))?;

    let our_net = node.config.network.to_string();
    let our_genesis = node.genesis_hash.to_hex();

    // Handshake
    send_json(&mut s, &json!({
        "t":"hello","net":our_net,"genesis":our_genesis,"height":node.current_height()
    }))?;
    let hello = recv_json(&mut s)?;
    if hello["t"].as_str() != Some("hello") {
        return Ok(()); // mismatch or bye
    }
    if hello["genesis"].as_str() != Some(our_genesis.as_str()) {
        debug!(peer = %addr, "genesis mismatch — not syncing");
        return Ok(());
    }

    // If the peer is BEHIND us, push our history up to them (in order). This is
    // how a public seed node gets the full chain from miners behind NAT: the
    // miner always initiates the connection, so it pushes its blocks here.
    let peer_height = hello["height"].as_u64().unwrap_or(0);
    let mut our_height = node.current_height();
    if our_height > peer_height {
        let mut from = peer_height;
        loop {
            let batch = node.blocks_above(from, SYNC_BATCH);
            if batch.is_empty() { break; }
            for b in &batch {
                send_json(&mut s, &json!({"t":"newblock","block":encode_block(b)}))?;
                from = b.header.blue_score;
            }
            debug!(peer = %addr, pushed = batch.len(), "pushed history to peer");
            if batch.len() < SYNC_BATCH { break; }
        }
        // tiny pause so the peer finishes inserting before we pull
        std::thread::sleep(Duration::from_millis(300));
        let _ = our_height; // (height may have advanced; pull loop re-reads it)
    }

    // Peer-exchange: share our known peers with the remote and request theirs.
    // This is how Bitcoin/LTC nodes self-discover the network organically.
    {
        let known: Vec<String> = node.manual_peers.lock().iter().take(20).cloned().collect();
        let _ = send_json(&mut s, &json!({"t":"addrs","addrs":known}));
    }

    // Broadcast our mining stats so the peer can aggregate them.
    // This is what makes combined network hashrate work across all nodes.
    let is_mining = node.is_mining.load(std::sync::atomic::Ordering::Relaxed);
    let pool_mining = node.pool_mode.load(std::sync::atomic::Ordering::Relaxed);
    // Report our OWN payout address when pooling (so the operator credits us, not
    // the pool wallet). Falls back to the miner address for solo.
    let payout_addr = {
        let member = node.pool_member_payout.lock().clone();
        if pool_mining && !member.is_empty() { member }
        else { node.miner_address.lock().as_ref().map(|a| a.to_string()).unwrap_or_default() }
    };
    let our_hashrate = {
        let hashes = node.mining_hashes.load(std::sync::atomic::Ordering::Relaxed);
        let started = node.mining_started_ms.load(std::sync::atomic::Ordering::Relaxed);
        if is_mining && started > 0 {
            let elapsed = (crate::node::now_ms().saturating_sub(started) as f64 / 1000.0).max(0.001);
            hashes as f64 / elapsed
        } else { 0.0 }
    };
    let _ = send_json(&mut s, &json!({
        "t": "stats",
        "hashrate": our_hashrate,
        "height": node.current_height(),
        "pool_mining": pool_mining,
        "payout_address": payout_addr,
    }));
    // Read stats_ack. The seed node is the pool operator, so we trust ITS roster
    // size + combined hashrate to show the whole pool on member wallets.
    if let Ok(ack) = recv_json(&mut s) {
        if ack["t"] == "stats_ack" && DEFAULT_SEED_PEERS.contains(&addr) {
            if let Some(pm) = ack["pool_active_miners"].as_u64() {
                node.learned_pool_miners.store(pm, std::sync::atomic::Ordering::Relaxed);
            }
            if let Some(h) = ack["hashrate"].as_f64() {
                *node.learned_pool_hashrate.lock() = h;
            }
        }
    }

    // Pull blocks above our height, repeatedly, until we stop making progress.
    loop {
        let from = node.current_height();
        send_json(&mut s, &json!({"t":"getblocks","from":from}))?;
        let resp = recv_json(&mut s)?;
        let arr = match resp["blocks"].as_array() { Some(a) => a.clone(), None => break };
        if arr.is_empty() { break; }

        let mut accepted = 0u32;
        for hb in &arr {
            if let Some(b) = hb.as_str().and_then(decode_block) {
                if node.accept_external_block(&b) { accepted += 1; }
            }
        }
        debug!(peer = %addr, accepted, "synced batch from peer");
        // If the peer sent a partial-but-no-progress batch, stop to avoid looping.
        if accepted == 0 { break; }
        if arr.len() < SYNC_BATCH { break; }
    }

    // ── Transaction relay ──────────────────────────────────────────────────
    // Pull the peer's pending transactions into our mempool, then push ours to
    // them. This propagates txs created on one node (faucet claims, pool
    // payouts, wallet sends) to every node, so whoever is mining can include
    // them in a block. Without this, a tx only ever gets mined by the node it
    // was created on.
    let _ = send_json(&mut s, &json!({"t":"getmempool"}));
    if let Ok(resp) = recv_json(&mut s) {
        if let Some(arr) = resp["txs"].as_array() {
            for h in arr {
                if let Some(tx) = h.as_str().and_then(decode_tx) { node.accept_external_tx(tx); }
            }
        }
    }
    let ours: Vec<String> = node.mempool_snapshot().iter().map(encode_tx).collect();
    if !ours.is_empty() {
        let _ = send_json(&mut s, &json!({"t":"addtxs","txs":ours}));
        let _ = recv_json(&mut s); // consume ack
    }
    Ok(())
}
