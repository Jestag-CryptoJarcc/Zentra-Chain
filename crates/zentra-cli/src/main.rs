//! # zentra-cli — Zentra L1 Command-Line Client
//!
//! A command-line client to interact with the zentrad node daemon over JSON-RPC.

use std::io::{Read, Write};
use std::net::TcpStream;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        interactive_shell()?;
    } else {
        run_cli_command(&args)?;
    }
    Ok(())
}

fn interactive_shell() -> anyhow::Result<()> {
    println!("============================================================");
    println!("        Zentra L1 CLI Interactive Console v0.1.0            ");
    println!("============================================================");
    println!("Connecting to zentrad daemon at 127.0.0.1:16111...");
    match send_rpc("getDagInfo", serde_json::json!([])) {
        Ok(_) => println!("✅ Connected to zentrad node daemon!"),
        Err(_) => {
            println!("❌ Warning: Could not connect to zentrad daemon.");
            println!("   Please make sure zentrad is running in another window.");
        }
    }
    println!();
    println!("Type commands (e.g., 'getdaginfo', 'getbalance <address>') to interact.");
    println!("Type 'help' to see usage details, and 'exit' or 'quit' to close.");
    println!();

    let stdin = std::io::stdin();
    loop {
        print!("zentra-cli> ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        if stdin.read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "exit" || line == "quit" {
            break;
        }

        let parts = match parse_command_line(line) {
            Ok(p) => p,
            Err(e) => {
                println!("❌ Error parsing arguments: {}", e);
                continue;
            }
        };

        if parts.is_empty() {
            continue;
        }

        if parts[0] == "help" {
            print_usage();
            continue;
        }

        let mut cmd_args = vec!["zentra-cli".to_string()];
        cmd_args.extend(parts);

        if let Err(e) = run_cli_command(&cmd_args) {
            println!("❌ Error: {}", e);
        }
        println!();
    }
    Ok(())
}

fn parse_command_line(line: &str) -> anyhow::Result<Vec<String>> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    
    for c in line.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
            }
            ' ' | '\t' if !in_quotes => {
                if !current.is_empty() {
                    args.push(current.clone());
                    current.clear();
                }
            }
            _ => {
                current.push(c);
            }
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    if in_quotes {
        anyhow::bail!("unclosed double quotes");
    }
    Ok(args)
}

fn run_cli_command(args: &[String]) -> anyhow::Result<()> {
    let command = args[1].as_str();
    match command {
        "generatewallet" => {
            let res = send_rpc("generateMnemonic", serde_json::json!([]))?;
            println!("Mnemonic seed phrase (save this privately!):\n{}", res.as_str().unwrap_or(""));
        }
        "deriveaddress" => {
            if args.len() < 3 {
                anyhow::bail!("Usage: zentra-cli deriveaddress \"<mnemonic>\"");
            }
            let res = send_rpc("deriveAddress", serde_json::json!([args[2]]))?;
            println!("Address: {}", res.as_str().unwrap_or(""));
        }
        "getbalance" => {
            if args.len() < 3 {
                anyhow::bail!("Usage: zentra-cli getbalance <address>");
            }
            let res = send_rpc("getBalance", serde_json::json!([args[2]]))?;
            let zents = res.as_u64().unwrap_or(0);
            println!("Balance: {} ZTR", zents as f64 / 100_000_000.0);
        }
        "sendtoaddress" => {
            if args.len() < 5 {
                anyhow::bail!("Usage: zentra-cli sendtoaddress \"<mnemonic>\" <to_address> <amount_ztr>");
            }
            let val: f64 = args[4].parse()?;
            let zents = (val * 100_000_000.0) as u64;
            let fee = 1000;
            let res = send_rpc("sendTransfer", serde_json::json!([args[2], args[3], zents, fee]))?;
            println!("Transaction Broadcasted! TxID: {}", res.as_str().unwrap_or(""));
        }
        "getmininginfo" => {
            let res = send_rpc("getMiningInfo", serde_json::json!([]))?;
            println!("{}", serde_json::to_string_pretty(&res)?);
        }
        "startmining" => {
            if args.len() < 4 {
                anyhow::bail!("Usage: zentra-cli startmining <lane_id> <payout_address>");
            }
            let lane: u8 = args[2].parse()?;
            let res = send_rpc("startMining", serde_json::json!([lane, args[3]]))?;
            println!("Mining status: {}", res.as_str().unwrap_or(""));
        }
        "stopmining" => {
            let res = send_rpc("stopMining", serde_json::json!([]))?;
            println!("Mining status: {}", res.as_str().unwrap_or(""));
        }
        "getminingstatus" => {
            let res = send_rpc("getMiningStatus", serde_json::json!([]))?;
            println!("{}", serde_json::to_string_pretty(&res)?);
        }
        "getdaginfo" => {
            let res = send_rpc("getDagInfo", serde_json::json!([]))?;
            println!("{}", serde_json::to_string_pretty(&res)?);
        }
        "getpoolstate" => {
            let res = send_rpc("getPoolState", serde_json::json!([]))?;
            println!("{}", serde_json::to_string_pretty(&res)?);
        }
        "getrecentblocks" => {
            let res = send_rpc("getRecentBlocks", serde_json::json!([]))?;
            println!("{}", serde_json::to_string_pretty(&res)?);
        }
        "vaultdeposit" => {
            if args.len() < 4 {
                anyhow::bail!("Usage: zentra-cli vaultdeposit <tx_hash> <amount_usdt>");
            }
            let val: f64 = args[3].parse()?;
            let amount_micro = (val * 1_000_000.0) as u64;
            let res = send_rpc("vaultDeposit", serde_json::json!([args[2], amount_micro]))?;
            println!("{}", serde_json::to_string_pretty(&res)?);
        }
        "vaultwithdraw" => {
            if args.len() < 4 {
                anyhow::bail!("Usage: zentra-cli vaultwithdraw <address> <amount_zusd>");
            }
            let val: f64 = args[3].parse()?;
            let amount_micro = (val * 1_000_000.0) as u64;
            let res = send_rpc("vaultWithdraw", serde_json::json!([args[2], amount_micro]))?;
            println!("Redeem Status: {}", res.as_str().unwrap_or(""));
        }
        _ => {
            println!("Unknown command: {}", command);
            print_usage();
        }
    }
    Ok(())
}

fn print_usage() {
    println!("Zentra L1 CLI Client v0.1.0");
    println!("Usage: zentra-cli <command> [args]");
    println!();
    println!("Commands:");
    println!("  generatewallet                              Generate a fresh BIP-39 mnemonic");
    println!("  deriveaddress \"<mnemonic>\"                  Derive Bech32 address from mnemonic");
    println!("  getbalance <address>                        Retrieve balance for an address");
    println!("  sendtoaddress \"<mnemonic>\" <to> <amount>    Send ZTR to an address");
    println!("  getmininginfo                               Get current difficulty and lane info");
    println!("  startmining <lane_id> <payout_address>      Start background mining loop");
    println!("  stopmining                                  Stop background mining loop");
    println!("  getminingstatus                             Get current mining status");
    println!("  getdaginfo                                  Get current DAG state summary");
    println!("  getpoolstate                                Get AMM DEX pool reserves and stats");
    println!("  getrecentblocks                             Get recent block history from DAG");
    println!("  vaultdeposit <tx_hash> <amount_usdt>        Simulate cross-chain USDT deposit to mint zUSD");
    println!("  vaultwithdraw <address> <amount_zusd>       Simulate zUSD burn to withdraw to Ethereum");
}

fn send_rpc(method: &str, params: serde_json::Value) -> Result<serde_json::Value, anyhow::Error> {
    let mut stream = TcpStream::connect("127.0.0.1:16111")
        .map_err(|_| anyhow::anyhow!("Could not connect to zentrad daemon at 127.0.0.1:16111. Make sure the daemon is running!"))?;
    
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
        "id": 1
    });
    
    let body = serde_json::to_string(&payload)?;
    let request = format!(
        "POST / HTTP/1.1\r\n\
         Host: 127.0.0.1:16111\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n\
         {}",
        body.len(),
        body
    );
    
    stream.write_all(request.as_bytes())?;
    stream.flush()?;
    
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    
    if let Some(pos) = response.find("\r\n\r\n") {
        let json_part = &response[pos + 4..];
        let res: serde_json::Value = serde_json::from_str(json_part)?;
        if let Some(error) = res.get("error") {
            anyhow::bail!("RPC Error: {}", error["message"]);
        }
        if let Some(result) = res.get("result") {
            Ok(result.clone())
        } else {
            anyhow::bail!("Invalid RPC response format")
        }
    } else {
        anyhow::bail!("Invalid HTTP response from server")
    }
}
