//! zul-l1-deposit: deposit SOL into the L1 bridge vault to credit an L2
//! recipient — a bridge ops/test tool for the deposit flow.
//!
//!   zul-l1-deposit --config ./config/node.toml --amount <lamports> --recipient <pubkey>
//!
//! Signs with the configured `[l1] bridge_authority_key_path`, so that key
//! must hold enough devnet SOL to cover the deposit + fees.

use zul_primitives::NodeConfig;
use solana_sdk::{pubkey::Pubkey, signature::Keypair};
use std::str::FromStr;

fn arg(name: &str) -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == name {
            return args.next();
        }
    }
    None
}

fn load_keypair(path: &str) -> anyhow::Result<Keypair> {
    let raw = std::fs::read_to_string(path)?;
    let bytes: Vec<u8> = serde_json::from_str(&raw)?;
    Keypair::try_from(bytes.as_slice()).map_err(|e| anyhow::anyhow!("invalid keypair {path}: {e}"))
}

fn main() -> anyhow::Result<()> {
    let config_path = arg("--config").unwrap_or_else(|| "./config/node.toml".into());
    let amount: u64 = arg("--amount")
        .ok_or_else(|| anyhow::anyhow!("--amount <lamports> required"))?
        .parse()?;
    let recipient = Pubkey::from_str(
        &arg("--recipient").ok_or_else(|| anyhow::anyhow!("--recipient <pubkey> required"))?,
    )?;

    let l1 = NodeConfig::load(std::path::Path::new(&config_path))?.l1;
    anyhow::ensure!(
        !l1.rpc_url.is_empty() && !l1.settlement_program_id.is_empty(),
        "[l1] not configured in {config_path}"
    );
    let settlement = Pubkey::from_str(&l1.settlement_program_id)?;
    let da_log = Pubkey::from_str(&l1.da_log_program_id).unwrap_or(settlement);
    let authority = load_keypair(&l1.bridge_authority_key_path)?;

    let client = zul_bridge::RpcL1Client::new(l1.rpc_url, authority, settlement, da_log)?;
    println!("depositing {amount} lamports for L2 recipient {recipient}…");
    let sig = client.deposit_sol(amount, recipient)?;
    println!("L1 deposit confirmed: {sig}");
    Ok(())
}
