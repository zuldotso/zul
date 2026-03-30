//! Load Solana-CLI-format keypairs (a JSON array of 64 bytes). Generated
//! locally with `solana-keygen`; never committed.

use solana_sdk::signature::Keypair;
use std::path::Path;

pub fn load_keypair(path: &Path) -> anyhow::Result<Keypair> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("reading keypair {}: {e}", path.display()))?;
    let bytes: Vec<u8> = serde_json::from_str(&raw)
        .map_err(|e| anyhow::anyhow!("parsing keypair {}: {e}", path.display()))?;
    Keypair::try_from(bytes.as_slice())
        .map_err(|e| anyhow::anyhow!("invalid keypair {}: {e}", path.display()))
}
