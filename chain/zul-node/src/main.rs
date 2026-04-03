//! ZUL sequencer node binary.
//!
//! Usage: `zul-node --config <path/to/node.toml>` (default ./config/node.toml).

mod keys;
mod mempool;
mod metrics;
mod node;

use node::{Node, NodeOptions};
use zul_primitives::{genesis::GenesisConfig, hash, NodeConfig};
use zul_store::Store;
use std::sync::Arc;
use std::time::Duration;

/// Resolve when the process is asked to stop. Handles SIGINT (ctrl-c) on all
/// platforms and SIGTERM on Unix — systemd sends SIGTERM on `restart`/`stop`.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn config_path() -> std::path::PathBuf {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--config" {
            if let Some(path) = args.next() {
                return path.into();
            }
        }
    }
    "./config/node.toml".into()
}

fn main() -> anyhow::Result<()> {
    // Genesis bootstrap loads the SVM program runtime and walks the state SMT,
    // which is stack-hungry. Run everything on an explicit large-stack thread.
    const STACK: usize = 32 * 1024 * 1024;
    std::thread::Builder::new()
        .name("zul-node".into())
        .stack_size(STACK)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_stack_size(STACK)
                .build()?
                .block_on(run())
        })?
        .join()
        .map_err(|_| anyhow::anyhow!("node thread panicked"))?
}

async fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config_path = config_path();
    let config = NodeConfig::load(&config_path)?;
    let genesis = GenesisConfig::load(&config.node.genesis_path)?;
    tracing::info!(
        chain = genesis.chain_name,
        genesis_hash = hash::to_base58(&genesis.genesis_hash()),
        "loading ZUL node"
    );

    let sequencer = keys::load_keypair(&config.node.sequencer_key_path)?;
    let faucet = if config.rpc.faucet_enabled {
        let path = config
            .rpc
            .faucet_key_path
            .as_ref()
            .expect("validated by config");
        Some(keys::load_keypair(path)?)
    } else {
        None
    };

    let store = Arc::new(Store::open(&config.node.data_dir.join("zul.redb"))?);
    let options = NodeOptions {
        slot_duration: Duration::from_millis(config.node.slot_duration_ms),
        heartbeat: Duration::from_millis(config.node.heartbeat_ms),
        mempool_capacity: config.node.mempool_capacity,
        max_per_payer: config.node.max_per_payer,
        faucet,
        faucet_max_lamports: config.rpc.faucet_max_lamports,
        ..Default::default()
    };
    let node = Arc::new(Node::new(store.clone(), &genesis, sequencer, options)?);

    // JSON-RPC: HTTP on the main port, WebSocket subscriptions on port+1.
    let rpc = zul_rpc::serve(
        config.rpc.http_addr,
        config.rpc.ws_addr,
        node.clone(),
        config.rpc.faucet_enabled,
    )
    .await?;
    tracing::info!(http = %rpc.http_addr, ws = %rpc.ws_addr, "RPC listening");

    // Shared L1 metrics; the settlement batcher will write these once L1 is
    // wired up. For now they stay at zero.
    let l1_metrics = Arc::new(metrics::L1Metrics::new());

    // Prometheus /metrics + /health, when configured (bind localhost).
    if let Some(metrics_addr) = config.rpc.metrics_addr {
        metrics::spawn(node.clone(), l1_metrics.clone(), metrics_addr)?;
        tracing::info!(%metrics_addr, "metrics listening");
    }

    // Block production loop. Exits cleanly on shutdown so an in-flight block
    // finishes rather than being abandoned mid-commit.
    let shutdown = Arc::new(tokio::sync::Notify::new());
    let producer = {
        let node = node.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(node.slot_duration());
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = interval.tick() => {}
                    _ = shutdown.notified() => break,
                }
                let node = node.clone();
                let result =
                    tokio::task::spawn_blocking(move || node.try_produce_block()).await;
                match result {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => tracing::error!(error = %e, "block production failed"),
                    Err(e) => tracing::error!(error = %e, "block production task panicked"),
                }
            }
        })
    };

    shutdown_signal().await;
    tracing::info!("draining for shutdown");
    node.begin_draining();
    shutdown.notify_one();
    let _ = producer.await;
    let _ = rpc.http.stop();
    let _ = rpc.ws.stop();
    rpc.http.stopped().await;
    rpc.ws.stopped().await;
    tracing::info!("shutdown complete");
    Ok(())
}
