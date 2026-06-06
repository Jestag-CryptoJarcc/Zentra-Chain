//! # zentrad — Zentra L1 Node Daemon
//!
//! The full node daemon for the Zentra L1 BlockDAG network.
//! Provides P2P networking, JSON-RPC server, mining, and wallet functionality.

mod config;
mod p2p;
mod p2p_sync;
mod rpc;
mod sync;
mod node;
mod pool;

use clap::Parser;
use tracing_subscriber::EnvFilter;
use std::sync::Arc;

/// Zentra L1 Node Daemon
#[derive(Parser, Debug)]
#[command(name = "zentrad", version, about = "Zentra L1 BlockDAG Node")]
struct Cli {
    /// Enable mining on the specified lane (0-4)
    #[arg(long)]
    mine: bool,

    /// Mining lane: 0=CPU, 1=GPU, 2=BTC_ASIC, 3=LTC_ASIC, 4=FPGA
    #[arg(long, default_value = "0")]
    lane: u8,

    /// P2P listening port
    #[arg(long, default_value_t = zentra_types::DEFAULT_P2P_PORT)]
    p2p_port: u16,

    /// JSON-RPC server port
    #[arg(long, default_value_t = zentra_types::DEFAULT_RPC_PORT)]
    rpc_port: u16,

    /// Data directory
    #[arg(long)]
    data_dir: Option<String>,

    /// Network: mainnet, testnet, devnet
    #[arg(long, default_value = "devnet")]
    network: String,

    /// Enable wallet mode
    #[arg(long)]
    wallet: bool,

    /// Run as a mining pool operator (block rewards go to the pool wallet and
    /// are distributed to registered miners every 30 minutes).
    #[arg(long)]
    pool: bool,

    /// Connect to a peer node (host:port). Repeatable: --connect a --connect b
    #[arg(long)]
    connect: Vec<String>,

    /// Do NOT auto-connect to the baked-in seed nodes. Use for an isolated
    /// local devnet or testing, where you only want --connect / peers.txt peers.
    #[arg(long)]
    no_seeds: bool,

    /// Number of mining threads (default: physical core count). Use 1 for a VPS.
    #[arg(long)]
    mine_threads: Option<usize>,

    /// Low-power keep-alive mining: sleep this many ms after each block so the
    /// CPU stays idle. e.g. 20000 = ~one block / 20s, near-zero CPU on devnet.
    #[arg(long, default_value_t = 0)]
    mine_throttle_ms: u64,

    /// Config file path
    #[arg(long)]
    config: Option<String>,
}

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    println!("╔══════════════════════════════════════════╗");
    println!("║     ZENTRA L1 — BlockDAG Network         ║");
    println!("║     Node Daemon v{}                 ║", env!("CARGO_PKG_VERSION"));
    println!("╚══════════════════════════════════════════╝");
    println!();

    let network = match cli.network.as_str() {
        "testnet" => zentra_types::NetworkType::Testnet,
        "devnet" => zentra_types::NetworkType::Devnet,
        _ => zentra_types::NetworkType::Mainnet,
    };

    tracing::info!(
        network = %network,
        p2p_port = cli.p2p_port,
        rpc_port = cli.rpc_port,
        mining = cli.mine,
        lane = cli.lane,
        "starting zentrad"
    );

    let data_dir_path = cli.data_dir
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|dir| dir.join("zentra-data")))
                .unwrap_or_else(|| std::path::PathBuf::from("./zentra-data"))
        });

    // Read or generate the RPC auth token
    let token_path = data_dir_path.join("rpc_auth.token");
    let token = if token_path.exists() {
        std::fs::read_to_string(&token_path).unwrap_or_default().trim().to_string()
    } else {
        use rand::RngCore;
        let mut key = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut key);
        let hex_token = hex::encode(key);
        let _ = std::fs::create_dir_all(&data_dir_path);
        let _ = std::fs::write(&token_path, &hex_token);
        hex_token
    };

    let config = config::NodeConfig {
        network,
        data_dir: data_dir_path.clone(),
        p2p_port: cli.p2p_port,
        rpc_port: cli.rpc_port,
        mining: config::MiningConfig {
            enabled: cli.mine,
            lane: cli.lane,
            threads: cli.mine_threads.unwrap_or_else(|| num_cpus::get_physical().max(1)),
        },
        wallet: config::WalletConfig {
            enabled: cli.wallet,
            keystore_path: data_dir_path.join("wallet"),
        },
    };

    // Initialize node
    let node = match node::ZentraNode::new(config) {
        Ok(node) => {
            let node_arc = Arc::new(node);
            tracing::info!(
                genesis = %node_arc.genesis_hash,
                tips = node_arc.tip_count(),
                "node ready"
            );
            println!("Genesis: {}", node_arc.genesis_hash);
            println!("Tips: {}", node_arc.tip_count());
            if cli.pool {
                node_arc.pool_mode.store(true, std::sync::atomic::Ordering::SeqCst);
                let addr = node_arc.pool.lock().address.clone();
                println!("Pool operator mode ON — pool wallet: {}", addr);
            }
            if cli.mine_throttle_ms > 0 {
                node_arc.mine_throttle_ms.store(cli.mine_throttle_ms, std::sync::atomic::Ordering::SeqCst);
                println!("Low-power mining throttle: {} ms between blocks", cli.mine_throttle_ms);
            }
            println!("Faucet address (fund/donate here): {}", node_arc.faucet_address);
            println!("Node is running. Press Ctrl+C to stop.");
            node_arc
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to initialize node");
            std::process::exit(1);
        }
    };

    // Create shutdown trigger channel
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);

    // Start background mining thread
    node.start_mining_worker();

    // Start pool payout worker
    node.start_pool_payout_worker();

    // ── P2P peers: load from peers.txt + --connect flags, then start sync ──
    {
        let mut peers: Vec<String> = Vec::new();
        // 1. Baked-in seed nodes (auto-connect for downloaded wallets), unless
        //    --no-seeds was passed for an isolated local devnet / test.
        if !cli.no_seeds {
            for s in p2p_sync::DEFAULT_SEED_PEERS { peers.push(s.to_string()); }
        }
        // 2. peers.txt in the data dir (one host:port per line, # = comment)
        let peers_file = node.config.data_dir.join("peers.txt");
        if let Ok(txt) = std::fs::read_to_string(&peers_file) {
            for line in txt.lines() {
                let l = line.trim();
                if !l.is_empty() && !l.starts_with('#') { peers.push(l.to_string()); }
            }
        }
        // 3. --connect flags
        for c in &cli.connect {
            let c = c.trim().to_string();
            if !c.is_empty() { peers.push(c); }
        }
        if !peers.is_empty() {
            let mut mp = node.manual_peers.lock();
            for p in peers {
                if !mp.contains(&p) { mp.push(p); }
            }
            println!("Loaded {} peer(s) to connect to", mp.len());
        }
    }
    // Inbound listener + outbound dialer
    p2p_sync::start_listener(Arc::clone(&node), node.config.p2p_port);
    p2p_sync::start_dialer(Arc::clone(&node));

    // Start RPC server
    let rpc_node = Arc::clone(&node);
    let rpc_port = cli.rpc_port;
    let rpc_shutdown_tx = shutdown_tx.clone();
    tokio::spawn(async move {
        if let Err(e) = rpc::start_rpc_server(rpc_port, rpc_node, rpc_shutdown_tx).await {
            tracing::error!(error = %e, "JSON-RPC server failed");
        }
    });

    // Start Web Dashboard server
    let web_port = rpc_port + 1;
    let web_token = token.clone();
    tokio::spawn(async move {
        if let Err(e) = start_web_server(web_port, web_token).await {
            tracing::error!(error = %e, "Web dashboard server failed");
        }
    });

    // Keep running until Ctrl+C or RPC shutdown
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Ctrl+C received, shutting down zentrad");
        }
        _ = shutdown_rx.recv() => {
            tracing::info!("Shutdown command received, shutting down zentrad");
        }
    }

    // Stop mining worker
    node.is_mining.store(false, std::sync::atomic::Ordering::SeqCst);
    tracing::info!("shutting down zentrad cleanly");
}

/// Forward a JSON-RPC request body to the private local RPC and return the
/// response body. Used by the public website's same-origin `/rpc` proxy.
// ── Public faucet rate-limiter ──────────────────────────────────────────────
// The faucet is the ONE state-mutating method we still expose publicly, so it
// needs its own throttle or a script drains it instantly. Three independent
// limits, mirroring how real faucets defend themselves:
//   • per-IP cooldown  — one claim per IP every 6h
//   • global min-gap   — at most one claim every 3s across ALL callers
//   • daily cap        — a hard ceiling of claims per UTC day
const FAUCET_IP_COOLDOWN_MS: u64 = 6 * 60 * 60 * 1000;
const FAUCET_GLOBAL_GAP_MS: u64 = 3_000;
const FAUCET_DAILY_CAP: u32 = 500;

struct FaucetLimiter {
    per_ip: std::collections::HashMap<String, u64>,
    last_global_ms: u64,
    day: u64,
    today_count: u32,
}

static FAUCET_LIMITER: std::sync::LazyLock<std::sync::Mutex<FaucetLimiter>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(FaucetLimiter {
        per_ip: std::collections::HashMap::new(),
        last_global_ms: 0,
        day: 0,
        today_count: 0,
    }));

fn now_ms_wall() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Decide whether `ip` may claim from the faucet right now. On success the
/// claim is recorded (counters advance) so the caller MUST forward the claim.
fn faucet_allow(ip: &str) -> Result<(), &'static str> {
    let now = now_ms_wall();
    let mut l = FAUCET_LIMITER.lock().unwrap_or_else(|e| e.into_inner());
    let day = now / 86_400_000;
    if day != l.day { l.day = day; l.today_count = 0; }
    if l.today_count >= FAUCET_DAILY_CAP { return Err("faucet daily limit reached, try tomorrow"); }
    if now.saturating_sub(l.last_global_ms) < FAUCET_GLOBAL_GAP_MS {
        return Err("faucet busy, try again in a few seconds");
    }
    if let Some(&last) = l.per_ip.get(ip) {
        if now.saturating_sub(last) < FAUCET_IP_COOLDOWN_MS {
            return Err("this IP already claimed recently — one claim per 6 hours");
        }
    }
    // Bound memory: drop stale entries once the map grows large.
    if l.per_ip.len() > 100_000 {
        l.per_ip.retain(|_, &mut t| now.saturating_sub(t) < FAUCET_IP_COOLDOWN_MS);
    }
    l.per_ip.insert(ip.to_string(), now);
    l.last_global_ms = now;
    l.today_count += 1;
    Ok(())
}

/// Client IP for faucet rate-limiting.
///
/// `cf-connecting-ip` / `x-forwarded-for` are trusted ONLY when the operator
/// runs behind a real reverse proxy and opts in with `ZENTRA_TRUST_PROXY_HEADERS=1`.
/// Otherwise a directly-exposed node would let anyone spoof these headers to mint
/// a fresh per-IP bucket on every request and drain the faucet. Default: trust
/// the real socket peer.
fn client_ip(req: &str, peer: std::net::SocketAddr) -> String {
    let trust_headers = std::env::var("ZENTRA_TRUST_PROXY_HEADERS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if trust_headers {
        for line in req.lines() {
            let ll = line.to_ascii_lowercase();
            if let Some(v) = ll.strip_prefix("cf-connecting-ip:") {
                let v = v.trim();
                if !v.is_empty() { return v.to_string(); }
            }
            if let Some(v) = ll.strip_prefix("x-forwarded-for:") {
                if let Some(first) = v.split(',').next() {
                    let first = first.trim();
                    if !first.is_empty() { return first.to_string(); }
                }
            }
        }
    }
    peer.ip().to_string()
}

async fn forward_rpc(rpc_port: u16, body: &str, token: &str) -> Option<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", rpc_port)).await.ok()?;
    let req = format!(
        "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        token, body.as_bytes().len(), body
    );
    s.write_all(req.as_bytes()).await.ok()?;
    let mut resp = Vec::new();
    s.read_to_end(&mut resp).await.ok()?;
    let text = String::from_utf8_lossy(&resp);
    let idx = text.find("\r\n\r\n")?;
    Some(text[idx + 4..].to_string())
}

async fn start_web_server(port: u16, token: String) -> anyhow::Result<()> {
    // This is the node's PUBLIC API endpoint, NOT a website host. It exposes a
    // read-only JSON-RPC proxy at /rpc for explorers/sites to read live chain
    // data. The website itself is a separate static bundle, hosted independently
    // (e.g. nginx serving the web/ folder + proxying /rpc to this node).
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    tracing::info!(port, "Node API listening on http://localhost:{}/rpc", port);

    loop {
        match listener.accept().await {
            Ok((mut socket, peer_addr)) => {
                let token_clone = token.clone();
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    // Read the full request: keep reading until we have the
                    // headers AND the complete Content-Length body (a single
                    // read often returns only the headers before the POST body
                    // arrives, which broke the /rpc proxy method parsing).
                    let mut data: Vec<u8> = Vec::new();
                    let mut tmp = [0u8; 4096];
                    loop {
                        match socket.read(&mut tmp).await {
                            Ok(0) => break,
                            Ok(n) => data.extend_from_slice(&tmp[..n]),
                            Err(_) => break,
                        }
                        if let Some(hpos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&data[..hpos]);
                            let clen = head.lines().find_map(|l| {
                                let ll = l.to_ascii_lowercase();
                                ll.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            }).unwrap_or(0);
                            if data.len() >= hpos + 4 + clen { break; }
                        }
                        if data.len() > 2_000_000 { break; } // safety cap
                    }
                    let req = String::from_utf8_lossy(&data).into_owned();
                    let req = req.as_str();

                    let path = req.lines().next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("/");

                    // ── Same-origin JSON-RPC proxy for the public website ──────
                    // The website (served here on the public web port) must read
                    // chain data from THIS node, not the visitor's localhost. We
                    // forward an ALLOWLISTED set of safe, read-mostly methods to
                    // the private RPC on 127.0.0.1; admin methods (stopNode,
                    // sendTransfer, addPeer, …) are never exposed.
                    if path == "/rpc" {
                        let cors = "Access-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: content-type\r\nAccess-Control-Allow-Methods: POST, OPTIONS\r\n";
                        if req.starts_with("OPTIONS") {
                            let _ = socket.write_all(format!("HTTP/1.1 204 No Content\r\n{}Content-Length: 0\r\nConnection: close\r\n\r\n", cors).as_bytes()).await;
                            return;
                        }
                        let body = req.find("\r\n\r\n").map(|i| &req[i+4..]).unwrap_or("");
                        let method = serde_json::from_str::<serde_json::Value>(body).ok()
                             .and_then(|v| v.get("method").and_then(|m| m.as_str()).map(|s| s.to_string()))
                             .unwrap_or_default();
                        // PUBLIC allowlist = READ-ONLY chain/network/pool data,
                        // the rate-limited faucet, and pool registration.
                        const ALLOWED: &[&str] = &[
                            // chain · blocks · transactions · addresses (read-only)
                            "getDagInfo", "getRecentBlocks", "getBlockByHash", "getBlockDetail",
                            "getTransaction", "getTransactionDetail", "getAddressDetail",
                            "getMempool", "getBalance",
                            // network · mining
                            "getNetworkInfo", "getMiningStatus", "getMiningInfo",
                            // AMM
                            "getPoolState",
                            // mining pool (READ-ONLY views only). poolJoin /
                            // poolHeartbeat are intentionally NOT public: they
                            // credit payout shares from a self-reported hashrate,
                            // so exposing them unauthenticated let any remote
                            // attacker claim the whole pool payout. They are now
                            // private-RPC only (operator-managed) until share
                            // accounting is replaced with verified PoW shares.
                            "poolGetInfo", "poolGetMiners", "poolGetPayouts",
                            // faucet (faucetClaim is rate-limited below)
                            "faucetInfo", "faucetClaim",
                        ];
                        let out = if !ALLOWED.contains(&method.as_str()) {
                            "{\"jsonrpc\":\"2.0\",\"error\":{\"code\":-32601,\"message\":\"method not allowed via public API\"},\"id\":null}".to_string()
                        } else if method == "faucetClaim" {
                            // Throttle BEFORE forwarding so abuse never reaches the node.
                            let ip = client_ip(req, peer_addr);
                            match faucet_allow(&ip) {
                                Ok(()) => forward_rpc(port - 1, body, &token_clone).await.unwrap_or_else(||
                                    "{\"jsonrpc\":\"2.0\",\"error\":{\"code\":-32603,\"message\":\"node unavailable\"},\"id\":null}".to_string()),
                                Err(reason) => format!(
                                    "{{\"jsonrpc\":\"2.0\",\"error\":{{\"code\":-32005,\"message\":\"{}\"}},\"id\":null}}", reason),
                            }
                        } else {
                            forward_rpc(port - 1, body, &token_clone).await.unwrap_or_else(||
                                "{\"jsonrpc\":\"2.0\",\"error\":{\"code\":-32603,\"message\":\"node unavailable\"},\"id\":null}".to_string())
                        };
                        let _ = socket.write_all(format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                            cors, out.as_bytes().len(), out).as_bytes()).await;
                        return;
                    }

                    // Anything that isn't /rpc gets a tiny status payload. This
                    // node does NOT host the website — it only answers API calls.
                    let body = "{\"service\":\"zentra-node\",\"api\":\"POST JSON-RPC to /rpc\",\"network\":\"devnet\"}";
                    let _ = socket.write_all(format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(), body).as_bytes()).await;
                    let _ = socket.flush().await;
                });
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to accept connection");
            }
        }
    }
}
