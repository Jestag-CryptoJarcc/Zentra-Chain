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
    #[arg(long, default_value = "mainnet")]
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
        // 1. Baked-in seed nodes (auto-connect for downloaded wallets)
        for s in p2p_sync::DEFAULT_SEED_PEERS { peers.push(s.to_string()); }
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
    tokio::spawn(async move {
        if let Err(e) = start_web_server(web_port).await {
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
async fn forward_rpc(rpc_port: u16, body: &str) -> Option<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", rpc_port)).await.ok()?;
    let req = format!(
        "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.as_bytes().len(), body
    );
    s.write_all(req.as_bytes()).await.ok()?;
    let mut resp = Vec::new();
    s.read_to_end(&mut resp).await.ok()?;
    let text = String::from_utf8_lossy(&resp);
    let idx = text.find("\r\n\r\n")?;
    Some(text[idx + 4..].to_string())
}

async fn start_web_server(port: u16) -> anyhow::Result<()> {
    // This is the node's PUBLIC API endpoint, NOT a website host. It exposes a
    // read-only JSON-RPC proxy at /rpc for explorers/sites to read live chain
    // data. The website itself is a separate static bundle, hosted independently
    // (e.g. nginx serving the web/ folder + proxying /rpc to this node).
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    tracing::info!(port, "Node API listening on http://localhost:{}/rpc", port);

    loop {
        match listener.accept().await {
            Ok((mut socket, _)) => {
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
                        const ALLOWED: &[&str] = &[
                            // chain · blocks · transactions · addresses (read-only)
                            "getDagInfo", "getRecentBlocks", "getBlockByHash", "getBlockDetail",
                            "getTransaction", "getTransactionDetail", "getAddressDetail",
                            "getMempool", "getBalance",
                            // network · mining
                            "getNetworkInfo", "getMiningStatus", "getMiningInfo",
                            // AMM
                            "getPoolState",
                            // mining pool
                            "poolGetInfo", "poolGetMiners", "poolGetPayouts",
                            "poolJoin", "poolHeartbeat",
                            // faucet
                            "faucetInfo", "faucetClaim",
                        ];
                        let out = if ALLOWED.contains(&method.as_str()) {
                            forward_rpc(port - 1, body).await.unwrap_or_else(||
                                "{\"jsonrpc\":\"2.0\",\"error\":{\"code\":-32603,\"message\":\"node unavailable\"},\"id\":null}".to_string())
                        } else {
                            "{\"jsonrpc\":\"2.0\",\"error\":{\"code\":-32601,\"message\":\"method not allowed via public API\"},\"id\":null}".to_string()
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
