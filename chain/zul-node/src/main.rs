//! ZUL sequencer node binary.
//!
//! Usage: `zul-node --config <path/to/node.toml>` (default ./config/node.toml).

mod keys;

fn main() -> anyhow::Result<()> {
    // Wiring (config -> store -> node -> rpc -> block loop) comes next.
    println!("zul-node: sequencer not wired up yet");
    Ok(())
}
