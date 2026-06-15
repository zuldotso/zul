# Zul live deployment

Bring the chain up on the OVHcloud server (sequencer + RPC), then point the
explorer/indexer at it. Tiered to match the rollout — start at Tier 1.

Prereqs already handled by the OVH post-install script: `build-essential
pkg-config libssl-dev protobuf-compiler clang llvm git curl ufw` + an 8G
swapfile + ufw allowing SSH.

## Tier 1 — sequencer + RPC (no bridge, no proofs needed for shields)

```sh
# 1. (recommended) a dedicated non-root user that owns the node
sudo adduser --disabled-password --gecos "" zul
sudo usermod -aG sudo zul
sudo rsync --archive ~/.ssh /home/zul/ && sudo chown -R zul:zul /home/zul/.ssh
sudo su - zul

# 2. Rust toolchain (matches chain/rust-toolchain.toml = 1.93.1)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
. "$HOME/.cargo/env"

# 3. clone + build the node (FIRST build is slow: Agave deps, ~20–40 min;
#    the swapfile prevents OOM on 8G boxes)
git clone <your-repo-url> ZUL && cd ZUL
cargo build --release -p zul-node --manifest-path chain/Cargo.toml

# 4. generate keys + genesis + node.toml + copy pool verifying keys.
#    Pass the devnet RPC now if you have it (used later by the bridge):
#    ZUL_L1_RPC_URL='https://your-l1-rpc-host/?api-key=XXXX' ./scripts/init-live.sh
./scripts/init-live.sh

# 5. open the RPC port (wallets/explorer reach this)
sudo ufw allow 8899/tcp

# 6. smoke-test in the foreground
./chain/target/release/zul-node --config "$PWD/config/node.toml"
```

In another shell, confirm it answers Solana RPC:

```sh
curl -s http://localhost:8899 -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}'
# -> {"jsonrpc":"2.0","result":"ok","id":1}

solana -u http://localhost:8899 block-height
solana -u http://localhost:8899 airdrop 1 <some-pubkey>   # uses the built-in faucet
```

Then run it as a service:

```sh
sudo cp deploy/zul-node.service /etc/systemd/system/
# edit User/paths in the unit if your checkout differs from /home/zul/ZUL
sudo systemctl daemon-reload && sudo systemctl enable --now zul-node
journalctl -u zul-node -f
```

## Web stack (Railway) — explorer + indexer + Postgres

Point both at the server's public RPC and a shared Postgres.

```
DATABASE_URL = ${{Postgres.DATABASE_URL}}
ZUL_RPC_URL  = http://<server-ip>:8899
```

- `db`: `prisma migrate deploy` at start time (build phase can't reach the DB).
- `indexer`: long-running worker (`node dist/main.js`).
- `explorer`: `next start`; reads `ZUL_RPC_URL` for the live slot ticker.

## Tier 2 — enable private transfers

`init-live.sh` already copied `circuits/artifacts/*.vk.bin` to
`data/params/`, so the node logs "loaded shielded-pool verifying keys" and
accepts transfer/unshield proofs. Host the proving artifacts
(`circuits/build/<c>/<c>_final.zkey` + `<c>_js/<c>.wasm`) somewhere clients
can fetch them to generate proofs in the browser.

> The dev ceremony (fixed entropy in `circuits/scripts/setup.mjs`) is fine
> for a testnet but forgeable — redo it as a real MPC ceremony before any
> value-bearing launch.

## Tier 3 — devnet settlement + bridge

> **Redeploy required (consensus change).** The withdrawal commitment now binds
> an `asset_id` (for the arbitrary-SPL bridge), so the settlement program's
> withdrawal hash changed. **A previously-deployed devnet settlement program is
> incompatible** — native *and* SPL withdrawal claims will fail against the
> updated node until you `cargo-build-sbf` + redeploy the settlement program and
> update `settlement_program_id`. Deposits/batches are unaffected; only the
> withdraw→claim path needs the matching program.

```sh
# deploy wallet with devnet SOL
solana-keygen new -o ~/.config/solana/id.json
solana config set --url <your-devnet-rpc>
solana airdrop 5

# deploy the Anchor programs (keypairs already in programs/target/deploy/)
cd programs && anchor deploy --provider.cluster <your-devnet-rpc>
```

Then in `config/node.toml` set `l1.enabled = true`, fill
`settlement_program_id` / `da_log_program_id` with the deployed ids, set
`bridge_authority_key_path`, and restart the node. The batcher posts state
roots + DA, and the deposit watcher credits L2 from devnet deposits.
