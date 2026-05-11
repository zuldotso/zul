//! zul-recover: rebuild the L2 transaction history from L1 and verify it
//! against the settlement commitments — single-sequencer disaster recovery.
//!
//!   zul-recover --config ./config/node.toml [--out recovered.bin]
//!
//! Reads the `[l1]` section (rpc_url + program ids), fetches every posted
//! settlement batch and reassembles its DA chunks, verifies DA-hash integrity
//! and the state-root chain, and (with --out) writes the recovered blocks as
//! bincode. The DA contains only non-empty blocks, so this recovers the full
//! transaction history + settled state roots; a bit-identical live-store
//! rebuild additionally needs the empty-block hashes (a DA design change).

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

fn hex32(b: &[u8; 32]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn main() -> anyhow::Result<()> {
    let config_path = arg("--config").unwrap_or_else(|| "./config/node.toml".into());
    let config = NodeConfig::load(std::path::Path::new(&config_path))?;
    let l1 = config.l1;
    anyhow::ensure!(
        !l1.rpc_url.is_empty() && !l1.settlement_program_id.is_empty() && !l1.da_log_program_id.is_empty(),
        "[l1] not configured in {config_path} (need rpc_url + settlement_program_id + da_log_program_id)"
    );
    let settlement = Pubkey::from_str(&l1.settlement_program_id)?;
    let da_log = Pubkey::from_str(&l1.da_log_program_id)?;

    // Read-only recovery: nothing is signed, so a throwaway authority is fine.
    let client = zul_bridge::RpcL1Client::new(l1.rpc_url, Keypair::new(), settlement, da_log)?;

    eprintln!("fetching settlement batches from L1…");
    let batches = client.fetch_settlement_batches()?;
    let summary = zul_bridge::verify(&batches)?;

    println!("=== L1 recovery verified ===");
    println!("batches      : {}", summary.batches);
    println!("blocks       : {}", summary.blocks);
    println!("transactions : {}", summary.transactions);
    println!("slot range   : {}..={}", summary.first_slot, summary.last_slot);
    println!("state root   : {}", hex32(&summary.final_state_root));

    if let Some(out) = arg("--out") {
        let blocks: Vec<_> = batches
            .iter()
            .flat_map(|b| b.payload.blocks.iter().cloned())
            .collect();
        let bytes = bincode::serialize(&blocks)?;
        std::fs::write(&out, &bytes)?;
        println!(
            "wrote {} recovered blocks to {out} ({} bytes)",
            blocks.len(),
            bytes.len()
        );
    }
    Ok(())
}
