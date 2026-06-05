//! Configuration management for zentrad.

use serde::{Serialize, Deserialize};
use zentra_types::*;
use std::path::PathBuf;

/// Node configuration loaded from TOML or CLI flags.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    pub network: NetworkType,
    pub data_dir: PathBuf,
    pub p2p_port: u16,
    pub rpc_port: u16,
    pub mining: MiningConfig,
    pub wallet: WalletConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MiningConfig {
    pub enabled: bool,
    pub lane: u8,
    pub threads: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletConfig {
    pub enabled: bool,
    pub keystore_path: PathBuf,
}

impl Default for NodeConfig {
    fn default() -> Self {
        NodeConfig {
            network: NetworkType::Mainnet,
            data_dir: PathBuf::from("./zentra-data"),
            p2p_port: DEFAULT_P2P_PORT,
            rpc_port: DEFAULT_RPC_PORT,
            mining: MiningConfig { enabled: false, lane: 0, threads: 1 },
            wallet: WalletConfig { enabled: false, keystore_path: PathBuf::from("./zentra-wallet") },
        }
    }
}

impl NodeConfig {
    pub fn load_or_default(path: Option<&std::path::Path>) -> Self {
        if let Some(p) = path {
            if let Ok(content) = std::fs::read_to_string(p) {
                if let Ok(config) = toml::from_str(&content) {
                    return config;
                }
            }
        }
        Self::default()
    }

    pub fn save(&self, path: &std::path::Path) -> anyhow::Result<()> {
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}
