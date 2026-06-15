//! zul-l1-deposit: deposit SOL (or an SPL token) into the L1 bridge to credit
//! an L2 recipient — a bridge ops/test tool for the deposit flow.
//!
//!   # native SOL → native ZUL
//!   zul-l1-deposit --config ./config/node.toml --amount <lamports> --recipient <pubkey>
//!   # SPL token → wrapped L2 token (--asset = L1 mint; --token-2022 for Token-2022 mints)
//!   zul-l1-deposit --config ... --amount <units> --recipient <pubkey> --asset <mint> [--token-2022]
//!
//! Signs with the configured `[l1] bridge_authority_key_path`, so that key
//! must hold enough SOL to cover fees (and, for SPL, the tokens being
//! deposited in its associated token account).

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

fn flag(name: &str) -> bool {
    std::env::args().skip(1).any(|a| a == name)
}

/// Classic SPL Token program (default) and Token-2022.
const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const TOKEN_2022_PROGRAM: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";

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
    let sig = match arg("--asset") {
        // SPL deposit → wrapped L2 token.
        Some(mint_str) => {
            let mint = Pubkey::from_str(&mint_str)?;
            let token_program = Pubkey::from_str(if flag("--token-2022") {
                TOKEN_2022_PROGRAM
            } else {
                TOKEN_PROGRAM
            })?;
            println!("depositing {amount} of SPL {mint} for L2 recipient {recipient}…");
            client.deposit_spl(amount, mint, token_program, recipient)?
        }
        // Native SOL deposit → native ZUL.
        None => {
            println!("depositing {amount} lamports for L2 recipient {recipient}…");
            client.deposit_sol(amount, recipient)?
        }
    };
    println!("L1 deposit confirmed: {sig}");
    Ok(())
}
