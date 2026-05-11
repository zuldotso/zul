//! zul-l1-claim: claim an L2 withdrawal on L1. Fetches the withdrawal's SMT
//! inclusion proof from the L2 RPC, finds the settlement batch whose posted
//! state root the proof targets, and submits `claim_withdrawal`.
//!
//!   zul-l1-claim --config ./config/node.toml \
//!       --recipient <L1_pubkey> --amount <lamports> --nonce <n> \
//!       [--l2-rpc http://127.0.0.1:8899]

use zul_primitives::NodeConfig;
use serde_json::{json, Value};
use solana_sdk::{pubkey::Pubkey, signature::Keypair};
use std::str::FromStr;
use std::time::Duration;

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

fn b58_32(s: &str) -> anyhow::Result<[u8; 32]> {
    let v = bs58::decode(s).into_vec()?;
    v.try_into().map_err(|_| anyhow::anyhow!("expected 32 bytes, got a different length"))
}

fn main() -> anyhow::Result<()> {
    let config_path = arg("--config").unwrap_or_else(|| "./config/node.toml".into());
    let recipient = Pubkey::from_str(
        &arg("--recipient").ok_or_else(|| anyhow::anyhow!("--recipient required"))?,
    )?;
    let amount: u64 = arg("--amount").ok_or_else(|| anyhow::anyhow!("--amount required"))?.parse()?;
    let nonce: u64 = arg("--nonce").ok_or_else(|| anyhow::anyhow!("--nonce required"))?.parse()?;
    let l2_rpc = arg("--l2-rpc").unwrap_or_else(|| "http://127.0.0.1:8899".into());

    let l1 = NodeConfig::load(std::path::Path::new(&config_path))?.l1;
    let settlement = Pubkey::from_str(&l1.settlement_program_id)?;
    let da_log = Pubkey::from_str(&l1.da_log_program_id).unwrap_or(settlement);
    let authority = load_keypair(&l1.bridge_authority_key_path)?;
    let client = zul_bridge::RpcL1Client::new(l1.rpc_url, authority, settlement, da_log)?;

    // 1. Fetch the inclusion proof from the L2.
    println!("fetching withdrawal proof from {l2_rpc}…");
    let body = json!({
        "jsonrpc": "2.0", "id": 1, "method": "getWithdrawalProof",
        "params": [recipient.to_string(), amount, nonce],
    });
    let text = ureq::post(&l2_rpc)
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())?
        .into_string()?;
    let resp: Value = serde_json::from_str(&text)?;
    if let Some(err) = resp.get("error") {
        anyhow::bail!("getWithdrawalProof: {err}");
    }
    let result = resp.get("result").ok_or_else(|| anyhow::anyhow!("no result"))?;
    // The proof targets the L2 withdrawals-tree root.
    let withdrawals_root = b58_32(result["stateRoot"].as_str().unwrap_or(""))?;
    let bitmap = b58_32(result["bitmap"].as_str().unwrap_or(""))?;
    let siblings: Vec<[u8; 32]> = result["siblings"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("siblings missing"))?
        .iter()
        .map(|s| b58_32(s.as_str().unwrap_or("")))
        .collect::<anyhow::Result<_>>()?;
    println!("  state root {} ({} siblings)", &result["stateRoot"].as_str().unwrap()[..8], siblings.len());

    // 2. Find the settlement batch that posted this withdrawals root (wait for it).
    println!("waiting for the withdrawals root to be settled on L1…");
    let batch_no = loop {
        match client.find_batch_for_withdrawals_root(&withdrawals_root)? {
            Some(n) => break n,
            None => {
                std::thread::sleep(Duration::from_secs(10));
            }
        }
    };
    println!("  settled in batch {batch_no}");

    // 3. Claim on L1.
    println!("claiming {amount} lamports for {recipient}…");
    let sig = client.claim_withdrawal(batch_no, amount, nonce, recipient, bitmap, &siblings)?;
    println!("withdrawal claimed on L1: {sig}");
    Ok(())
}
