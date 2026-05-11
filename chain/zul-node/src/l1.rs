//! L1 settlement driver: posts batches of L2 blocks to the devnet
//! settlement/DA programs. Runs on a dedicated OS thread because the bridge
//! client is synchronous (blocking RPC) and batching must never stall block
//! production. The thread polls the store tip and lets the `Batcher` decide
//! what to settle; L1 is the source of truth for the resume cursor.

use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use zul_bridge::{Batcher, L1Client, RpcL1Client};
use zul_primitives::config::L1Section;
use zul_store::Store;
use solana_sdk::pubkey::Pubkey;

use crate::metrics::L1Metrics;

/// Pause batch posting below this authority balance (~0.02 SOL) so the bridge
/// key is never fully drained and we don't spam failing posts when out of funds.
const MIN_AUTHORITY_LAMPORTS: u64 = 20_000_000;

/// Validate config locally (so bad config fails node boot, not a background
/// thread), then run the batcher loop. Network setup is retried inside the
/// thread so a transient L1 outage never brings the sequencer down.
pub fn spawn(
    store: Arc<Store>,
    l1: L1Section,
    slot_duration_ms: u64,
    metrics: Arc<L1Metrics>,
) -> anyhow::Result<JoinHandle<()>> {
    let settlement = Pubkey::from_str(&l1.settlement_program_id)
        .map_err(|e| anyhow::anyhow!("invalid l1.settlement_program_id: {e}"))?;
    let da_log = Pubkey::from_str(&l1.da_log_program_id)
        .map_err(|e| anyhow::anyhow!("invalid l1.da_log_program_id: {e}"))?;
    let authority = crate::keys::load_keypair(Path::new(&l1.bridge_authority_key_path))?;
    let rpc_url = l1.rpc_url.clone();
    let interval = l1.batch_interval_slots.max(1);
    // Time to accumulate roughly `interval` slots before the next sweep.
    let idle = Duration::from_millis(slot_duration_ms.saturating_mul(interval).max(1_000));

    let handle = std::thread::Builder::new()
        .name("zul-batcher".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            let mut batcher = loop {
                match build_batcher(&rpc_url, &authority, settlement, da_log) {
                    Ok(batcher) => break batcher,
                    Err(e) => {
                        tracing::error!(error = %e, "L1 batcher init failed; retrying in 10s");
                        std::thread::sleep(Duration::from_secs(10));
                    }
                }
            };
            tracing::info!(
                cursor = batcher.cursor_slot(),
                interval,
                "L1 settlement batcher running"
            );
            metrics.enabled.store(true, Ordering::Relaxed);
            // Seed the last-batch gauge from L1 so it reflects real settlement
            // history, not just posts made since this process started.
            if let Ok(Some(n)) = batcher.client().latest_batch_no() {
                metrics.last_batch_no.store(n as i64, Ordering::Relaxed);
            }
            loop {
                metrics
                    .cursor_slot
                    .store(batcher.cursor_slot(), Ordering::Relaxed);
                let latest = match store.latest_slot() {
                    Ok(Some(slot)) => slot,
                    Ok(None) => 0,
                    Err(e) => {
                        tracing::error!(error = %e, "batcher: reading tip");
                        std::thread::sleep(idle);
                        continue;
                    }
                };
                let cursor = batcher.cursor_slot();
                if latest >= cursor {
                    // Backpressure: pause posting (don't advance the cursor or
                    // spam failing txs) when the authority is nearly out of
                    // funds. A balance-check hiccup is non-fatal — proceed.
                    if let Ok(balance) = batcher.client().authority_balance() {
                        metrics.authority_lamports.store(balance, Ordering::Relaxed);
                        if balance < MIN_AUTHORITY_LAMPORTS {
                            tracing::warn!(
                                balance_lamports = balance,
                                min = MIN_AUTHORITY_LAMPORTS,
                                "L1 batcher authority low on funds; pausing batch posting"
                            );
                            std::thread::sleep(idle);
                            continue;
                        }
                    }
                    // Bound each batch to `interval` slots so the very first
                    // batch on a long-lived chain doesn't balloon.
                    let tip = latest.min(cursor + interval - 1);
                    match batcher.run_once(&store, tip) {
                        Ok(Some(record)) => {
                            metrics
                                .last_batch_no
                                .store(record.batch_no as i64, Ordering::Relaxed);
                            tracing::info!(
                                batch_no = record.batch_no,
                                first_slot = record.first_slot,
                                last_slot = record.last_slot,
                                chunks = record.chunk_count,
                                "posted settlement batch to L1"
                            );
                        }
                        Ok(None) => {}
                        Err(e) => {
                            metrics.post_failures.fetch_add(1, Ordering::Relaxed);
                            // A failed post can mean L1 diverged from our
                            // in-memory cursor (reorg, or a batch we thought
                            // unposted now exists). Re-derive the batcher from
                            // L1 — the source of truth — instead of retrying
                            // the same batch number forever.
                            tracing::error!(error = %e, "batcher: run_once failed; re-deriving from L1 in 10s");
                            std::thread::sleep(Duration::from_secs(10));
                            match build_batcher(&rpc_url, &authority, settlement, da_log) {
                                Ok(fresh) => {
                                    tracing::info!(cursor = fresh.cursor_slot(), "batcher re-derived from L1");
                                    batcher = fresh;
                                }
                                Err(e) => tracing::error!(error = %e, "batcher re-derive failed; will retry"),
                            }
                            continue;
                        }
                    }
                    // Keep catching up without idling while behind the tip.
                    if tip < latest {
                        continue;
                    }
                }
                std::thread::sleep(idle);
            }
        })?;
    Ok(handle)
}

fn build_batcher(
    rpc_url: &str,
    authority: &solana_sdk::signature::Keypair,
    settlement: Pubkey,
    da_log: Pubkey,
) -> anyhow::Result<Batcher<RpcL1Client>> {
    let client = RpcL1Client::new(rpc_url.to_string(), authority.insecure_clone(), settlement, da_log)?;
    client.ensure_initialized()?;
    let cursor = client.resume_cursor_slot()?;
    Ok(Batcher::new(client, cursor)?)
}
