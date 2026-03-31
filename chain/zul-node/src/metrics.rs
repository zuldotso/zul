//! Prometheus metrics + a plain liveness endpoint, on a dedicated port.
//!
//! `GET /metrics` → Prometheus text exposition of the core node gauges.
//! `GET /health`  → 200 when the producer is live, 503 when stalled (a simple
//! check for load balancers that don't speak JSON-RPC).
//!
//! Runs on its own blocking thread (tiny_http); bind to localhost and scrape
//! through the reverse proxy.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;

use zul_rpc::RpcBackend;

use crate::node::Node;

/// Shared L1-batcher state, written by the batcher thread (`l1.rs`) and read
/// by the metrics endpoint. `last_batch_no` is -1 until the first post.
pub struct L1Metrics {
    pub enabled: AtomicBool,
    pub last_batch_no: AtomicI64,
    pub cursor_slot: AtomicU64,
    pub authority_lamports: AtomicU64,
    pub post_failures: AtomicU64,
}

impl L1Metrics {
    pub fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            last_batch_no: AtomicI64::new(-1),
            cursor_slot: AtomicU64::new(0),
            authority_lamports: AtomicU64::new(0),
            post_failures: AtomicU64::new(0),
        }
    }
}

pub fn spawn(node: Arc<Node>, l1: Arc<L1Metrics>, addr: SocketAddr) -> anyhow::Result<()> {
    let server = tiny_http::Server::http(addr)
        .map_err(|e| anyhow::anyhow!("metrics server bind {addr}: {e}"))?;
    std::thread::Builder::new()
        .name("zul-metrics".into())
        .spawn(move || {
            for request in server.incoming_requests() {
                let (status, body) = match request.url() {
                    "/metrics" => (200, render(&node, &l1)),
                    "/health" => {
                        if node.is_healthy() {
                            (200, "ok\n".to_string())
                        } else {
                            (503, "unhealthy\n".to_string())
                        }
                    }
                    _ => (404, "not found\n".to_string()),
                };
                let header = tiny_http::Header::from_bytes(
                    &b"Content-Type"[..],
                    &b"text/plain; version=0.0.4"[..],
                )
                .expect("static header");
                let response = tiny_http::Response::from_string(body)
                    .with_status_code(status)
                    .with_header(header);
                let _ = request.respond(response);
            }
        })?;
    Ok(())
}

fn render(node: &Node, l1: &L1Metrics) -> String {
    let slot = node.current_slot();
    let depth = node.mempool_depth();
    let age = node.last_block_age_ms();
    let healthy = node.is_healthy() as u8;
    let total_txs = node.total_txs();
    let l1_enabled = l1.enabled.load(Ordering::Relaxed) as u8;
    let l1_last_batch = l1.last_batch_no.load(Ordering::Relaxed);
    let l1_cursor = l1.cursor_slot.load(Ordering::Relaxed);
    let l1_lamports = l1.authority_lamports.load(Ordering::Relaxed);
    let l1_failures = l1.post_failures.load(Ordering::Relaxed);
    format!(
        "# HELP zul_slot Current chain tip slot (block height).\n\
         # TYPE zul_slot gauge\n\
         zul_slot {slot}\n\
         # HELP zul_total_txs Transactions committed since genesis.\n\
         # TYPE zul_total_txs counter\n\
         zul_total_txs {total_txs}\n\
         # HELP zul_mempool_depth Transactions waiting in the mempool.\n\
         # TYPE zul_mempool_depth gauge\n\
         zul_mempool_depth {depth}\n\
         # HELP zul_last_block_age_ms Milliseconds since the last committed block.\n\
         # TYPE zul_last_block_age_ms gauge\n\
         zul_last_block_age_ms {age}\n\
         # HELP zul_healthy 1 if the producer is live, else 0.\n\
         # TYPE zul_healthy gauge\n\
         zul_healthy {healthy}\n\
         # HELP zul_l1_enabled 1 if L1 settlement batching is on.\n\
         # TYPE zul_l1_enabled gauge\n\
         zul_l1_enabled {l1_enabled}\n\
         # HELP zul_l1_last_batch_no Last settlement batch number posted to L1 (-1 = none).\n\
         # TYPE zul_l1_last_batch_no gauge\n\
         zul_l1_last_batch_no {l1_last_batch}\n\
         # HELP zul_l1_cursor_slot First L2 slot not yet settled to L1.\n\
         # TYPE zul_l1_cursor_slot gauge\n\
         zul_l1_cursor_slot {l1_cursor}\n\
         # HELP zul_l1_authority_lamports Balance of the batch-posting authority on L1.\n\
         # TYPE zul_l1_authority_lamports gauge\n\
         zul_l1_authority_lamports {l1_lamports}\n\
         # HELP zul_l1_post_failures Batch-post failures (triggers a re-derive from L1).\n\
         # TYPE zul_l1_post_failures counter\n\
         zul_l1_post_failures {l1_failures}\n"
    )
}
