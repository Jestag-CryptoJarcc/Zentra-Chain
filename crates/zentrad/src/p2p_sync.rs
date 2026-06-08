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
use zentra_types::Hash;

const MAX_FRAME: usize = 4 * 1024 * 1024; // 4 MB safety cap (a block is < 1 MB)
const SYNC_BATCH: usize = 500;

/// RAII marker that counts an in-flight peer sync. Increments `node.active_syncs`
/// on creation and decrements on drop, so the count is correct even when
/// `sync_from_peer` returns early via `?`. The miner pauses while the count > 0.
struct SyncGuard {
    count: Arc<std::sync::atomic::AtomicUsize>,
}
impl SyncGuard {
    fn new(node: &Arc<ZentraNode>) -> Self {
        node.active_syncs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        SyncGuard { count: Arc::clone(&node.active_syncs) }
    }
}
impl Drop for SyncGuard {
    fn drop(&mut self) {
        self.count.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

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

/// Hard ceiling on simultaneous inbound P2P connections. Without it a single
/// host can open thousands of sockets and exhaust our threads/memory (a trivial
/// DoS). Bitcoin caps inbound peers the same way (-maxconnections).
const MAX_INBOUND: usize = 256;
static INBOUND_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

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
                    use std::sync::atomic::Ordering;
                    // Reject once we're at the inbound ceiling. fetch_add returns
                    // the PRIOR value, so >= MAX means this one is over the line.
                    if INBOUND_COUNT.fetch_add(1, Ordering::SeqCst) >= MAX_INBOUND {
                        INBOUND_COUNT.fetch_sub(1, Ordering::SeqCst);
                        drop(s); // close immediately
                        continue;
                    }
                    let node = Arc::clone(&node);
                    std::thread::spawn(move || {
                        if let Err(e) = handle_inbound(node, s) {
                            debug!(err = %e, "inbound peer closed");
                        }
                        INBOUND_COUNT.fetch_sub(1, Ordering::SeqCst);
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
                // Learn this peer's REACHABLE address = its socket IP + the listening
                // port it advertised, and remember it so we dial + gossip it. This is
                // how a node anyone runs (with its P2P port open) becomes discoverable
                // to the whole network instead of being seen only as an ephemeral port.
                if let Some(port) = msg["p2p_port"].as_u64() {
                    if let Ok(sa) = peer.parse::<std::net::SocketAddr>() {
                        let reachable = format!("{}:{}", sa.ip(), port);
                        let mut mp = node.manual_peers.lock();
                        if mp.len() < 256 && !mp.contains(&reachable) {
                            mp.push(reachable.clone());
                            tracing::info!(peer = %reachable, "discovered reachable peer from handshake");
                        }
                    }
                }
                // Learn peers the dialer shared (backward-compatible field).
                merge_peers(&node, &msg["peers"]);
                let our_tip = node.dag.get_selected_tip().ok().flatten().map(|h| h.to_hex()).unwrap_or_default();
                let our_peers: Vec<String> = node.manual_peers.lock().iter().take(50).cloned().collect();
                send_json(&mut s, &json!({
                    "t":"hello","net":our_net,"genesis":our_genesis,
                    "height": node.current_height(), "tip": our_tip, "peers": our_peers
                }))?;
            }
            "getblocks" => {
                let locator = msg["locator"].as_array();
                let mut common_height = 0;
                if let Some(arr) = locator {
                    for h_val in arr {
                        if let Some(h_str) = h_val.as_str() {
                            if let Ok(h) = Hash::from_hex(h_str) {
                                if let Ok(Some(b)) = node.dag.get_block(&h) {
                                    common_height = b.header.blue_score;
                                    break;
                                }
                            }
                        }
                    }
                } else {
                    common_height = msg["from"].as_u64().unwrap_or(0);
                }

                let blocks = node.blocks_above(common_height, SYNC_BATCH);
                let hexes: Vec<String> = blocks.iter().map(encode_block).collect();
                send_json(&mut s, &json!({"t":"blocks","blocks":hexes}))?;
            }
            // Fetch a single block by hash — used to pull missing parents of an
            // orphan (Bitcoin's getdata MSG_BLOCK / Kaspa's missing-ancestor req).
            "getblock" => {
                let blk = msg["hash"].as_str()
                    .and_then(|hx| Hash::from_hex(hx).ok())
                    .and_then(|h| node.dag.get_block(&h).ok().flatten());
                match blk {
                    Some(b) => send_json(&mut s, &json!({"t":"block","block":encode_block(&b)}))?,
                    None => send_json(&mut s, &json!({"t":"block","block":serde_json::Value::Null}))?,
                }
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
                    // Hard cap the peer table so gossip can't grow it without
                    // bound (a flooding peer could otherwise feed us endless
                    // junk addresses). Bitcoin bounds its addrman the same way.
                    const MAX_PEERS: usize = 256;
                    let room = MAX_PEERS.saturating_sub(mp.len());
                    let added: Vec<String> = arr.iter()
                        .filter_map(|v| v.as_str())
                        .filter(|a| !a.is_empty() && a.contains(':') && !mp.contains(&a.to_string()))
                        .map(|a| a.to_string())
                        .take(50.min(room)) // cap per-message AND overall table size
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
                // Reply with our own stats so they can see us too. Include our pool
                // wallet + operator flag so members can mine into the shared pool.
                let op_mode = node.pool_mode.load(std::sync::atomic::Ordering::Relaxed);
                let op_pool = node.pool.lock().address.clone();
                send_json(&mut s, &json!({
                    "t":"stats_ack",
                    "hashrate": node.combined_network_hashrate(),
                    "peer_count": node.peer_stats.lock().len(),
                    "pool_active_miners": node.pool.lock().active_count(),
                    "pool_mode": op_mode,
                    "pool_address": op_pool,
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
            // Pool member submitting verified-PoW shares. We (the operator) verify
            // each share's PoW + coinbase tag and credit the tagged member. No reply
            // (one-way), so older nodes that don't send this are unaffected.
            "poolshare" => {
                if let Some(arr) = msg["shares"].as_array() {
                    let mut credited = 0u32;
                    for h in arr.iter().take(500) {
                        if let Some(b) = h.as_str().and_then(decode_block) {
                            if node.verify_and_credit_share(&b) { credited += 1; }
                        }
                    }
                    if credited > 0 { debug!(credited, "credited verified pool shares"); }
                }
            }
            "bye" => return Ok(()),
            _ => {}
        }
    }
}

// ── dialer (client) ──────────────────────────────────────────────────────────

/// Start the outbound dialer. Periodically syncs from every configured peer.
pub fn start_dialer(node: Arc<ZentraNode>) {
    // A transaction that hasn't been mined within this window is stuck (rejected
    // by miners — e.g. it chains off an output that never confirmed) and is
    // dropped so it can't clog the mempool forever. Runs on EVERY node, mining
    // or not, so a relay-only seed clears its mempool too.
    const MEMPOOL_EXPIRY_MS: u64 = 20 * 60 * 1000; // 20 minutes
    std::thread::spawn(move || {
        info!("P2P dialer started");
        loop {
            node.mempool.evict_older_than(MEMPOOL_EXPIRY_MS);
            let peers = node.manual_peers.lock().clone();
            // Persist the current peer set so discovered peers are remembered across
            // restarts (main.rs reloads peers.txt on startup). Skip the loopback
            // self-entry. Best-effort; written every cycle (small file).
            {
                let lines: Vec<String> = peers.iter()
                    .filter(|p| !p.starts_with("127.0.0.1") && !p.is_empty())
                    .cloned().collect();
                if !lines.is_empty() {
                    let path = node.config.data_dir.join("peers.txt");
                    let body = format!("# Auto-saved known peers — edit freely (one host:port per line)\n{}\n", lines.join("\n"));
                    let _ = std::fs::write(&path, body);
                }
            }
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

/// Merge a JSON array of "host:port" peer addresses into our peer table (bounded,
/// deduped). Used to learn peers another node shares in its `hello` — this is the
/// backward-compatible discovery path (old nodes just omit the field).
fn merge_peers(node: &Arc<ZentraNode>, val: &serde_json::Value) {
    if let Some(arr) = val.as_array() {
        let mut mp = node.manual_peers.lock();
        for v in arr.iter().filter_map(|v| v.as_str()) {
            if mp.len() >= 256 { break; }
            let a = v.to_string();
            if !a.is_empty() && a.contains(':') && !mp.contains(&a) { mp.push(a); }
        }
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

    // Handshake (include our selected-tip hash so each side can detect a divergent chain)
    let our_tip = node.dag.get_selected_tip().ok().flatten().map(|h| h.to_hex()).unwrap_or_default();
    let our_peers: Vec<String> = node.manual_peers.lock().iter().take(50).cloned().collect();
    send_json(&mut s, &json!({
        "t":"hello","net":our_net,"genesis":our_genesis,"height":node.current_height(),"tip":our_tip,
        "p2p_port": node.config.p2p_port, "peers": our_peers
    }))?;
    let hello = recv_json(&mut s)?;
    if hello["t"].as_str() != Some("hello") {
        return Ok(()); // mismatch or bye
    }
    if hello["genesis"].as_str() != Some(our_genesis.as_str()) {
        debug!(peer = %addr, "genesis mismatch — not syncing");
        return Ok(());
    }
    // Learn the peers this node knows (backward-compatible: absent on old nodes).
    merge_peers(node, &hello["peers"]);

    // If the peer is BEHIND us, push our history up to them (in order). This is
    // how a public seed node gets the full chain from miners behind NAT: the
    // miner always initiates the connection, so it pushes its blocks here.
    let peer_height = hello["height"].as_u64().unwrap_or(0);
    // Do we recognize the peer's selected tip? If not, the peer is on a DIFFERENT
    // (divergent) chain, and pushing only blocks above its height would orphan on
    // its side. In that case push from genesis so it receives our whole chain and
    // can reorg to it when ours is heavier (this is what makes divergent chains —
    // e.g. a NAT'd miner that's ahead — converge).
    let peer_tip_known = hello["tip"].as_str()
        .and_then(|h| Hash::from_hex(h).ok())
        .map(|h| matches!(node.dag.get_block(&h), Ok(Some(_))))
        .unwrap_or(false);
    // Track the highest chain a peer has — the miner uses this to avoid mining
    // a competing fork while we're still behind.
    node.max_peer_height.fetch_max(peer_height, std::sync::atomic::Ordering::Relaxed);
    let mut our_height = node.current_height();
    if our_height > peer_height {
        let mut from = if peer_tip_known { peer_height } else { 0 };
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

    // Peer-exchange: push our known peers to the remote (one-way, fire-and-forget
    // so it stays compatible with older nodes that don't reply). Pulling the
    // remote's peers happens via the `hello` handshake (see below), which old
    // nodes simply omit — no protocol desync across versions.
    {
        let known: Vec<String> = node.manual_peers.lock().iter().take(50).cloned().collect();
        let _ = send_json(&mut s, &json!({"t":"addrs","addrs":known}));
    }

    // Broadcast our mining stats so the peer can aggregate them.
    // This is what makes combined network hashrate work across all nodes.
    let is_mining = node.is_mining.load(std::sync::atomic::Ordering::Relaxed);
    let pool_mining = node.pool_mode.load(std::sync::atomic::Ordering::Relaxed)
        || node.pool_member.load(std::sync::atomic::Ordering::Relaxed);
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
                // The seed reports the whole-network hashrate here; remember it so
                // our (NAT'd) wallet can show the real network total, not just us.
                *node.learned_network_hashrate.lock() = h;
            }
            // Learn the operator's pool wallet so we (as a member) mine into it.
            if ack["pool_mode"].as_bool().unwrap_or(false) {
                if let Some(pa) = ack["pool_address"].as_str() {
                    if !pa.is_empty() { *node.learned_operator_pool.lock() = pa.to_string(); }
                }
            }
        }
    }

    // Submit any pool shares we've found. Sent to whichever peer we're syncing
    // with: only the pool OPERATOR will credit them (it verifies the PoW + coinbase
    // tag); any non-operator safely ignores them. One-way + best-effort, and old
    // nodes ignore the unknown message — so it's fully backward-compatible.
    {
        let shares: Vec<zentra_core::block::Block> = {
            let mut p = node.pending_shares.lock();
            if p.is_empty() { Vec::new() } else { std::mem::take(&mut *p) }
        };
        if !shares.is_empty() {
            let hexes: Vec<String> = shares.iter().take(200).map(encode_block).collect();
            let _ = send_json(&mut s, &json!({"t":"poolshare","shares":hexes}));
        }
    }

    // RAII sync marker: increments active_syncs now and decrements on EVERY exit
    // path (including the `?`/`break` early returns below), so the miner only
    // resumes once the last concurrent peer sync has finished.
    let _sync_guard = SyncGuard::new(node);

    // Pull blocks above our height, repeatedly, until we stop making progress.
    loop {
        let mut locator = Vec::new();
        if let Ok(Some(tip)) = node.dag.get_selected_tip() {
            let chain = node.get_selected_chain(tip);
            for h in chain.into_iter().take(32) {
                locator.push(h.to_hex());
            }
        }
        if let Err(_) = send_json(&mut s, &json!({"t":"getblocks","locator":locator})) { break; }
        let resp = match recv_json(&mut s) { Ok(r) => r, Err(_) => break };
        let arr = match resp["blocks"].as_array() { Some(a) => a.clone(), None => break };
        if arr.is_empty() { break; }

        let mut accepted = 0u32;
        for hb in &arr {
            if let Some(b) = hb.as_str().and_then(decode_block) {
                if node.accept_external_block(&b) { accepted += 1; }
            }
        }
        debug!(peer = %addr, accepted, "synced batch from peer");
        if accepted == 0 { break; }
        if arr.len() < SYNC_BATCH { break; }
    }

    // ── Orphan-parent resolution ───────────────────────────────────────────
    for _round in 0..SYNC_BATCH {
        let want: Vec<Hash> = { node.wanted.lock().iter().copied().collect() };
        if want.is_empty() { break; }
        let mut got_any = false;
        for h in want {
            if let Err(_) = send_json(&mut s, &json!({"t":"getblock","hash":h.to_hex()})) { continue; }
            let resp = match recv_json(&mut s) { Ok(r) => r, Err(_) => continue };
            if let Some(b) = resp["block"].as_str().and_then(decode_block) {
                if b.hash() == h {
                    node.accept_external_block(&b);
                    node.wanted.lock().remove(&h);
                    got_any = true;
                } else {
                    node.wanted.lock().remove(&h);
                }
            } else {
                node.wanted.lock().remove(&h);
            }
        }
        node.try_connect_orphans();
        if !got_any { break; }
    }
    // (_sync_guard is dropped at function exit, clearing this peer's sync marker.)

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
