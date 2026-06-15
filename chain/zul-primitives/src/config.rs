//! Node configuration, loaded from TOML (see config/node.example.toml).

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;

use crate::constants::DEFAULT_SLOT_DURATION_MS;
use crate::PrimitivesError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    pub node: NodeSection,
    pub rpc: RpcSection,
    #[serde(default)]
    pub l1: L1Section,
}

/// Which deployment a node belongs to. Settlement target and operational
/// defaults differ; mainnet additionally forbids the built-in faucet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Network {
    #[default]
    Testnet,
    Mainnet,
}

impl std::fmt::Display for Network {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Network::Testnet => "testnet",
            Network::Mainnet => "mainnet",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSection {
    /// Deployment this node serves. Defaults to testnet so existing configs
    /// keep working; mainnet must be opted into explicitly.
    #[serde(default)]
    pub network: Network,
    pub data_dir: PathBuf,
    pub genesis_path: PathBuf,
    pub sequencer_key_path: PathBuf,
    #[serde(default = "default_slot_duration")]
    pub slot_duration_ms: u64,
    /// Produce an empty heartbeat block after this long with no transactions,
    /// keeping the chain visibly alive. Higher = less idle storage growth.
    #[serde(default = "default_heartbeat")]
    pub heartbeat_ms: u64,
    /// Mempool capacity (total queued transactions).
    #[serde(default = "default_mempool_capacity")]
    pub mempool_capacity: usize,
    /// Anti-spam: max in-flight transactions per fee payer.
    #[serde(default = "default_max_per_payer")]
    pub max_per_payer: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcSection {
    pub http_addr: SocketAddr,
    pub ws_addr: SocketAddr,
    #[serde(default)]
    pub faucet_enabled: bool,
    #[serde(default = "default_faucet_max")]
    pub faucet_max_lamports: u64,
    /// Keypair holding the faucet allocation; required when the faucet is
    /// enabled so airdrops are real, signed System transfers.
    #[serde(default)]
    pub faucet_key_path: Option<PathBuf>,
    /// When set, serve a Prometheus `/metrics` (+ `/health`) endpoint here.
    /// Bind to localhost and scrape via the reverse proxy. Omitted = disabled.
    #[serde(default)]
    pub metrics_addr: Option<SocketAddr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L1Section {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub rpc_url: String,
    #[serde(default)]
    pub settlement_program_id: String,
    #[serde(default)]
    pub da_log_program_id: String,
    #[serde(default)]
    pub bridge_authority_key_path: String,
    #[serde(default = "default_batch_interval")]
    pub batch_interval_slots: u64,
}

impl Default for L1Section {
    /// Must mirror the serde field defaults so an omitted `[l1]` section and
    /// an empty `[l1]` section produce identical configs.
    fn default() -> Self {
        Self {
            enabled: false,
            rpc_url: String::new(),
            settlement_program_id: String::new(),
            da_log_program_id: String::new(),
            bridge_authority_key_path: String::new(),
            batch_interval_slots: default_batch_interval(),
        }
    }
}

fn default_slot_duration() -> u64 {
    DEFAULT_SLOT_DURATION_MS
}

fn default_heartbeat() -> u64 {
    10_000
}

fn default_mempool_capacity() -> usize {
    10_000
}

fn default_max_per_payer() -> usize {
    128
}

fn default_faucet_max() -> u64 {
    10_000_000_000
}

fn default_batch_interval() -> u64 {
    120
}

impl NodeConfig {
    pub fn from_toml_str(raw: &str) -> Result<Self, PrimitivesError> {
        let cfg: Self = toml::from_str(raw)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn load(path: &std::path::Path) -> Result<Self, PrimitivesError> {
        let raw = std::fs::read_to_string(path).map_err(|source| PrimitivesError::Io {
            path: path.display().to_string(),
            source,
        })?;
        Self::from_toml_str(&raw)
    }

    fn validate(&self) -> Result<(), PrimitivesError> {
        if self.node.slot_duration_ms == 0 {
            return Err(PrimitivesError::InvalidConfig(
                "slot_duration_ms must be positive".into(),
            ));
        }
        if self.rpc.faucet_enabled && self.rpc.faucet_key_path.is_none() {
            return Err(PrimitivesError::InvalidConfig(
                "faucet_enabled requires faucet_key_path".into(),
            ));
        }
        if self.node.network == Network::Mainnet && self.rpc.faucet_enabled {
            return Err(PrimitivesError::InvalidConfig(
                "mainnet must not enable the faucet (ZUL is real gas)".into(),
            ));
        }
        if self.l1.enabled {
            if self.l1.rpc_url.is_empty() {
                return Err(PrimitivesError::InvalidConfig(
                    "l1.enabled requires l1.rpc_url".into(),
                ));
            }
            if self.l1.settlement_program_id.is_empty() {
                return Err(PrimitivesError::InvalidConfig(
                    "l1.enabled requires l1.settlement_program_id".into(),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
[node]
data_dir = "./data"
genesis_path = "./config/genesis.toml"
sequencer_key_path = "./keys/sequencer.json"

[rpc]
http_addr = "127.0.0.1:8899"
ws_addr = "127.0.0.1:8900"
faucet_enabled = true
faucet_max_lamports = 10000000000
faucet_key_path = "./keys/faucet.json"
"#;

    #[test]
    fn parses_with_defaults() {
        let cfg = NodeConfig::from_toml_str(EXAMPLE).unwrap();
        assert_eq!(cfg.node.slot_duration_ms, DEFAULT_SLOT_DURATION_MS);
        assert!(!cfg.l1.enabled);
        assert_eq!(cfg.l1.batch_interval_slots, 120);
    }

    #[test]
    fn faucet_requires_key() {
        let broken = EXAMPLE.replace("faucet_key_path = \"./keys/faucet.json\"\n", "");
        assert!(NodeConfig::from_toml_str(&broken).is_err());
    }

    #[test]
    fn l1_enabled_requires_endpoints() {
        let with_l1 = format!("{EXAMPLE}\n[l1]\nenabled = true\n");
        assert!(NodeConfig::from_toml_str(&with_l1).is_err());
    }

    #[test]
    fn network_defaults_to_testnet() {
        let cfg = NodeConfig::from_toml_str(EXAMPLE).unwrap();
        assert_eq!(cfg.node.network, Network::Testnet);
    }

    #[test]
    fn mainnet_forbids_faucet() {
        let mainnet = EXAMPLE.replace("[node]", "[node]\nnetwork = \"mainnet\"");
        // EXAMPLE enables the faucet, which is illegal on mainnet.
        assert!(NodeConfig::from_toml_str(&mainnet).is_err());
    }
}
