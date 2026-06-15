//! ZUL sequencer node binary.
//!
//! Usage: `zul-node --config <path/to/node.toml>` (default ./config/node.toml).

mod bridge;
mod keys;
mod l1;
mod mempool;
mod metrics;
mod node;
mod pool;
mod withdraw_smt;

use node::{Node, NodeOptions};
use zul_primitives::{genesis::GenesisConfig, hash, NodeConfig};
use zul_store::Store;
use std::sync::Arc;
use std::time::Duration;

/// Load the pool verifying keys from `<data_dir>/params/{transfer,unshield}.vk.bin`
/// (copy them there from circuits/artifacts after building the circuits).
fn load_pool_keys(data_dir: &std::path::Path) -> zul_privacy::VerifyingKeys {
    let params = data_dir.join("params");
    let transfer = params.join("transfer.vk.bin");
    let unshield = params.join("unshield.vk.bin");
    match (std::fs::read(&transfer), std::fs::read(&unshield)) {
        (Ok(t), Ok(u)) => match zul_privacy::VerifyingKeys::load(&t, &u) {
            Ok(keys) => {
                tracing::info!("loaded shielded-pool verifying keys");
                keys
            }
            Err(e) => {
                tracing::warn!(error = %e, "invalid pool verifying keys; pool transfers disabled");
                zul_privacy::VerifyingKeys::unconfigured()
            }
        },
        _ => {
            tracing::warn!(
                "no pool verifying keys at {}; transfers/unshields disabled until provided",
                params.display()
            );
            zul_privacy::VerifyingKeys::unconfigured()
        }
    }
}

/// Resolve when the process is asked to stop. Handles SIGINT (ctrl-c) on all
/// platforms and SIGTERM on Unix — systemd sends SIGTERM on `restart`/`stop`,
/// so without this the graceful path never runs in production.
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
    // Genesis bootstrap loads the SVM program runtime and walks the
    // depth-256 state SMT, which is stack-hungry. The OS default main-thread
    // stack (1 MB on Windows, 8 MB on Linux) can overflow, so run everything
    // on an explicit large-stack thread and give tokio workers the same.
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
        network = %config.node.network,
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

    // Shielded-pool verifying keys: load from the circuit artifacts if
    // present, otherwise stay unconfigured (transfers/unshields rejected;
    // shields still work). Path defaults to ./params next to the binary.
    let pool_verifying_keys = load_pool_keys(&config.node.data_dir);

    let store = Arc::new(Store::open(&config.node.data_dir.join("zul.redb"))?);
    let options = NodeOptions {
        slot_duration: Duration::from_millis(config.node.slot_duration_ms),
        heartbeat: Duration::from_millis(config.node.heartbeat_ms),
        mempool_capacity: config.node.mempool_capacity,
        max_per_payer: config.node.max_per_payer,
        faucet,
        faucet_max_lamports: config.rpc.faucet_max_lamports,
        pool_verifying_keys,
        ..Default::default()
    };
    let node = Arc::new(Node::new(store.clone(), &genesis, sequencer, vec![], options)?);

    // Shared L1 metrics, written by the batcher thread and read by /metrics.
    let l1_metrics = Arc::new(metrics::L1Metrics::new());

    // L1 settlement batcher (Tier 3): when enabled, post batches of L2 blocks
    // to the devnet settlement/DA programs on a dedicated thread.
    let _batcher = if config.l1.enabled {
        Some(l1::spawn(
            store.clone(),
            config.l1.clone(),
            config.node.slot_duration_ms,
            l1_metrics.clone(),
        )?)
    } else {
        tracing::info!("L1 settlement disabled (l1.enabled = false)");
        None
    };

    // L1 -> L2 deposit watcher: credits SOL deposited into the L1 vault.
    let _deposit_watcher = if config.l1.enabled {
        Some(bridge::spawn(node.clone(), config.l1.clone())?)
    } else {
        None
    };

    // JSON-RPC: HTTP on the main port, WebSocket subscriptions on port+1.
    let rpc = zul_rpc::serve(
        config.rpc.http_addr,
        config.rpc.ws_addr,
        node.clone(),
        config.rpc.faucet_enabled,
    )
    .await?;
    tracing::info!(http = %rpc.http_addr, ws = %rpc.ws_addr, "RPC listening");

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
                // Block production is synchronous store/SVM work.
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
    // Graceful: flip health to draining (load balancers stop routing), let the
    // current block finish, then stop the RPC server.
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
