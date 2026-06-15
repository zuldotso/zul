# Zul mainnet launch — the one checklist

> **✅ LAUNCHED 2026-06-16.** Live on Solana mainnet-beta from OVH `127.0.0.1`
> (`zul-mainnet-1`). Programs deployed — settlement `Po6o…ZUL`, da_log `by9f…ZUL`;
> L1 settlement initialized; gas seeded; public RPC **`https://rpc-mainnet.zul.so`**
> (+ `wss://`), TLS via Caddy. Remaining hand-off: move the programs' upgrade
> authority to a multisig (step 6) and sweep excess SOL/ZUL off the hot key.

Single source of truth for going live on Solana mainnet-beta. Detailed mechanics
live in `DEPLOY-MAINNET.md`, `MAINNET-COST.md`, `MAINNET-CUSTODY.md`,
`MAINNET-CEREMONY.md`; this file is the ordered sequence + decisions.

The whole settlement program (deposit + withdraw, SOL + arbitrary SPL + the real
Token-2022 ZUL gas mint) is **verified on-chain on devnet** with the exact same
bytecode mainnet will run — see `HARDENING-CHANGELOG.md` (2026-06-16). Devnet ≡
mainnet for program correctness.

## Fixed identities (already decided / generated)

| Thing | Value |
|---|---|
| Gas token (L1) | `okTCeDRb7NeExmvf3RZxEs29i9nfw8Cd51oFVB4FZUL` — Token-2022, 9 dec, ~1B fixed supply, mint authority revoked, safe extensions (verified) |
| Settlement program (mainnet) | `Po6oySWHr9oxFWVhZJ1Jca2HkvfzXxsK91n3R6mqZUL` (vanity) — keypair `keys/mainnet/zul_settlement_mainnet-keypair.json` |
| DA-log program (mainnet) | `by9fpRVSYN9ib7qwXJuak37d78cKUMFgNAWtjK3pZUL` (vanity) — keypair `keys/mainnet/zul_da_log_mainnet-keypair.json` |
| Deployer / bridge authority | `ETCUTxbKrCTQST1eWAEKc9gPtsiNQhQ9BAHwxwjQMDbZ` — keypair `keys/mainnet/deployer.json` (gitignored). Verified funded: 18.99 SOL + ~3.15M ZUL |
| L1 RPC (settlement) | Helius (`https://your-l1-rpc-host/?api-key=…`) — pass via `ZUL_L1_RPC_URL`, never commit |
| Public L2 RPC | `https://rpc-mainnet.zul.so` → Caddy (`deploy/Caddyfile.mainnet`) → node `127.0.0.1:8899` (+ ws `:8900`) |
| Gas model | native ZUL = bridged ZUL-SPL 1:1; **zero native pre-mine**; SOL → wrapped |

The mainnet program ids are wired everywhere: cfg-gated `declare_id!`
(`--features mainnet`), `Anchor.toml [programs.mainnet]`, `config/mainnet/`,
`init-live.sh`. Build the mainnet artifacts with **`cargo-build-sbf --features
mainnet`** (default build = devnet ids).

## Pre-launch decisions

- [x] **Funder/bridge-authority key** — `ETCUTxbKrCTQST1eWAEKc9gPtsiNQhQ9BAHwxwjQMDbZ`,
      at `keys/mainnet/deployer.json` (gitignored). Verified on mainnet: 18.99 SOL
      (deploy ~4 + operating) **and** ~3.15M ZUL to seed the first gas.
- [x] **Trusted setup (shielded pool).** Ceremony run 2026-06-16 — real Groth16
      keys now in `circuits/artifacts/{transfer,unshield}.vk.bin`; transcript at
      `circuits/artifacts/CEREMONY-TRANSCRIPT.md` (Hermez phase-1 + a secret
      contribution + BTC-block-#953,837 beacon; `zkey verify` → Ok; node cross-test
      passes). ⚠️ Phase-2 had a **single local contributor**, so the assumption is
      "this machine was honest + the entropy was destroyed." For higher assurance
      before the pool holds large value, re-run phase-2 with several independent
      contributors and redeploy `vk.bin`.
- [ ] **Upgrade-authority custody.** After deploy, move both programs' upgrade
      authority to a Squads multisig (`MAINNET-CUSTODY.md`). Until then the
      deployer key is the upgrade authority. (Step 6.)

## Launch execution (run when the funder key is in place)

On the **second OVH box** (separate from testnet), as a `zul` user with the Rust
toolchain (see `DEPLOY-MAINNET.md` §0). Provision the box with the deploy SSH key
`keys/mainnet/ssh/zul-mainnet.pub` (hostname `zul-mainnet-1`) and install Caddy
for the public RPC:

1. **Build + init** the chain (zero pre-mine genesis, faucet off):
   ```sh
   cargo build --release -p zul-node --manifest-path chain/Cargo.toml
   ZUL_NET=mainnet ZUL_L1_RPC_URL='https://your-l1-rpc-host/?api-key=…' \
     ./scripts/init-live.sh            # writes keys/mainnet, config/mainnet/{genesis,node}.toml
   ```
2. **Service up** (templated unit), then publish the RPC via Caddy as
   `https://rpc-mainnet.zul.so` (8899/8900 stay on localhost — never opened):
   ```sh
   sudo cp deploy/zul-node@.service /etc/systemd/system/
   sudo systemctl daemon-reload && sudo systemctl enable --now zul-node@mainnet
   journalctl -u zul-node@mainnet -f   # expect network=mainnet, getHealth ok
   # Public endpoint: point DNS A record rpc-mainnet.zul.so -> this box first.
   sudo ufw allow 80,443/tcp           # for Caddy; NOT 8899/8900
   sudo cp deploy/Caddyfile.mainnet /etc/caddy/Caddyfile
   sudo systemctl reload caddy         # auto-provisions the Let's Encrypt cert
   curl -s https://rpc-mainnet.zul.so -X POST -H 'content-type: application/json' \
     -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}'   # -> {"result":"ok"}
   ```
3. **Deploy the programs** (mainnet build, real SOL — `MAINNET-COST.md`: ~2.83 +
   ~1.24 SOL rent, deploy `--max-len` exact). Funder = `keys/mainnet/deployer.json`:
   ```sh
   cd programs && cargo-build-sbf --features mainnet
   solana program deploy --url <helius-mainnet> --keypair keys/mainnet/deployer.json \
     --program-id keys/mainnet/zul_settlement_mainnet-keypair.json \
     --max-len $(stat -c%s target/deploy/zul_settlement.so) target/deploy/zul_settlement.so
   solana program deploy --url <helius-mainnet> --keypair keys/mainnet/deployer.json \
     --program-id keys/mainnet/zul_da_log_mainnet-keypair.json \
     --max-len $(stat -c%s target/deploy/zul_da_log.so) target/deploy/zul_da_log.so
   ```
4. **Fund the bridge authority** + **enable settlement**: fund the deployer/
   authority on mainnet, then edit `config/mainnet/node.toml` `[l1]`:
   `enabled = true`, `rpc_url` = the Helius mainnet url, `bridge_authority_key_path
   = …/keys/mainnet/deployer.json` (program ids + `gas_mint` are pre-filled).
   `sudo systemctl restart zul-node@mainnet`. The batcher auto-runs `initialize`.
5. **Seed gas + smoke test**: bridge some ZUL-SPL in (`zul-l1-deposit --asset
   okTCe… --token-2022`) → confirm the recipient gets native ZUL; do a transfer →
   confirm a settlement batch posts (`config.batch_count` increments). Bridge an
   SPL + withdraw round-trip if desired (same flow proven on devnet).
6. **Custody**: transfer both programs' upgrade authority to the multisig.
7. **Web stack**: explorer/indexer on Railway pointed at `https://rpc-mainnet.zul.so`
   + a **separate** Postgres (do not share the testnet DB). Nightly backups
   (`ZUL_NET=mainnet … scripts/backup-redb.sh`).

## Reminder: the chain is unusable until step 5

Zero pre-mine means no gas exists until the ZUL-SPL is bridged in. Sequence:
programs deployed → `[l1]` enabled → ZUL-SPL bridged → native gas exists → live.
