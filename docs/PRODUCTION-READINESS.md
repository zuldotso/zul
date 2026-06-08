# Zul Production Readiness — single-sequencer operator model

**Scope / trust model (deliberate).** ZUL runs as a single Proof-of-Authority
sequencer. Fraud proofs, validity proofs, and sequencer decentralization are
**out of scope by design** — the operator is trusted. "Production" here therefore
means the system must:

1. **Not lose or corrupt state** (durability + disaster recovery),
2. **Stay up** (liveness, monitoring, fast recovery),
3. **Resist external attackers** (DoS, abuse, key theft),
4. **Be operable** (observability, runbooks, safe upgrades).

This is the bar early Arbitrum / Optimism / most app-chains shipped mainnet at.
The items below are grounded in the current code; each names the file to change.

**Companion:** the privacy axis (external-observer unlinkability) has its own ordered
plan in `docs/PRIVACY-HARDENING.md`. This file covers everything else.

Legend: **P0** = fix before securing real value · **P1** = operate safely ·
**P2** = maturity/polish · ⚡ = quick win (high value / low effort).

---

## P0 — must fix before real value

- [ ] **RPC has no connection/body/rate limits, no auth, plaintext, public.** ⚡
  `chain/zul-rpc/src/server.rs:484` builds `Server::builder().build(addr)` with all
  jsonrpsee defaults and binds `0.0.0.0:8899` (`config/node.toml`). Anyone can open
  unlimited connections / large bodies → trivial DoS; admin-ish methods are open.
  **Fix:** set `max_connections`, `max_request_body_size`, `max_response_body_size`,
  `max_subscriptions_per_connection`; add a tower rate-limit layer via
  `RpcServiceBuilder`. Bind RPC to `127.0.0.1` and terminate TLS + per-IP rate limit
  at nginx/Caddy. Keep `:8899` firewalled to the proxy only.
  **Partially shipped 2026-06-11:** `serve()` applies `ServerConfig` caps —
  `max_connections=1000`, request body 2 MiB, response body 16 MiB.
  **Still open:** per-IP rate limit + TLS at a reverse proxy (needs a domain), and
  binding RPC to `127.0.0.1`.

- [ ] **Mempool spam starves legitimate transactions.**
  `chain/zul-node/src/mempool.rs` is a bounded FIFO (10k) with signature dedup, but
  **no per-payer cap, no minimum fee, no fee-priority eviction**. 10k cheap valid txs
  from one key fill it → `MempoolError::Full` rejects everyone. **Fix:** per-payer
  in-flight cap, a minimum-fee floor, and priority-fee ordering with eviction of the
  lowest-fee tx when full (the module header already anticipates this).

- [x] **Faucet + `requestAirdrop` exposed in production.** ⚡ — **SHIPPED 2026-06-11.**
  `build_module` now registers `requestAirdrop` only when `faucet_enabled`. The live
  testnet keeps the faucet on by choice; set `faucet_enabled = false` for mainnet and
  the method disappears entirely.

- [ ] **Single hot key is upgrade authority AND bridge authority.**
  The devnet deployer `5FBJz6…` is the L1 program upgrade authority *and* the bridge
  `config.authority` *and* lives unencrypted at
  `/home/ubuntu/ZUL/keys/bridge-authority.json` on the sequencer box. Theft = drain +
  malicious program upgrade. **Fix:** upgrade authority → **multisig + timelock**
  (Squads), separate from the batch-posting key; keep the batch-posting key low-balance
  and rotate; store secrets in an encrypted store / HSM, not a flat file.

- [ ] **No disaster recovery — box loss = chain loss.**
  `chain/zul-store/src/store.rs:86` opens a single redb file with default durability
  (`Immediate`, so each commit fsyncs — single-commit durability is fine), but there is
  **no backup job and no off-box copy**. The L1 DA batches are a recovery anchor, but
  **no reconstruction tool exists**. **Fix:** (a) periodic snapshot/backup of the redb
  file to off-box storage (S3/B2) + restore runbook; (b) a `reconstruct-from-l1` tool
  that replays posted DA batches into a fresh store (validates against on-chain
  `state_root`/`da_hash`). This is the single-sequencer substitute for redundancy.
  **Shipped 2026-06-11:** `scripts/backup-redb.sh` (crash-consistent gzip snapshot +
  14-day rotation, integrity-verified) on a nightly cron (04:00 UTC); restore steps in
  the script header.
  **Shipped 2026-06-12:** `zul-recover` (`zul-node/src/bin/zul-recover.rs` +
  `zul-bridge/src/recover.rs`) fetches every settlement batch + reassembles its DA chunks
  from L1, verifies `da_hash` integrity + structure, and recovers the full block/tx
  history. Live-verified: 4 batches / 8 blocks / 8 txs recovered from devnet. **Finding:**
  the DA omits empty heartbeat blocks, so a *bit-identical live-store* rebuild needs those
  hashes too (a DA change or a recency-skipping recovery executor) — data recovery +
  settlement integrity is complete; store re-execution is the remaining half.
  **Still open:** off-box backup push (set `ZUL_BACKUP_REMOTE` to an rclone remote — until
  then the nightly snapshot is same-box only).

- [x] **`getHealth` always returns "ok".**  ⚡ — **SHIPPED 2026-06-11.**
  `RpcBackend::is_healthy()` ties `getHealth` to last-block age (unhealthy after 3
  heartbeats); the `/metrics` server mirrors it at `GET /health`. Verified live ("ok").

---

## P1 — operate safely

- [ ] **No observability.** Logs only (journald) + the explorer; no metrics, no alerts.
  **Fix:** Prometheus metrics (slot height, block interval, mempool depth, RPC rate/latency/errors,
  batcher lag, L1 authority balance) + Grafana + alerts on: no block in N s, batcher
  `run_once` failing, L1 balance below threshold, disk %.
  **Partially shipped 2026-06-11:** `zul-node/src/metrics.rs` serves Prometheus `/metrics`
  (`zul_slot`, `zul_mempool_depth`, `zul_last_block_age_ms`, `zul_healthy`) on
  `127.0.0.1:9899`. **Still open:** RPC + batcher + L1-balance gauges, Grafana, and alerting.

- [x] **L1 batcher robustness.** — **SHIPPED 2026-06-12.** `l1.rs` now re-derives the
  batcher from L1 on a post failure (reorg self-heal) and pauses posting below a
  `MIN_AUTHORITY_LAMPORTS` (0.02 SOL) balance via `RpcL1Client::authority_balance`. The L1
  authority balance is also a Prometheus gauge for alerting. Catch-up stays bounded by
  `batch_interval_slots`. (Boot re-scan of empty windows is harmless — posts nothing.)

- [~] **Bridge deposit/withdraw.** **DEPOSIT SHIPPED 2026-06-12 (live-verified on devnet):**
  `zul-node/src/bridge.rs` watches the settlement program for `Deposited` events and queues
  them; `Node::apply_deposits` mints the matching ZUL to the recipient and writes a *receipt
  account* (existence = credited) atomically with the credit, so a crash never double-credits
  or loses a deposit, and a fresh store re-credits from L1 automatically. Trigger tool:
  `zul-l1-deposit`. Verified: 0.05 SOL deposited on devnet → L2 recipient credited 50,000,000
  in one watcher poll.
  **WITHDRAW FULLY WORKING 2026-06-13 (round-trip lands SOL on devnet).** A bridge withdraw tx
  (`bridge::partition_withdraws`) burns the L2 balance + commits a withdrawal leaf
  (`withdrawal_pubkey`/`withdrawal_account_hash`, cross-pinned byte-identical with the settlement
  program) into a separate **depth-32 withdrawals SMT** (`zul-node/src/withdraw_smt.rs`, in-memory,
  rebuilt from blocks on boot); its root is posted per batch (`BatchRecord.withdrawals_root`).
  `getWithdrawalProof` serves the depth-32 inclusion proof; `zul-l1-claim` finds the batch by
  withdrawals root + submits `claim_withdrawal`, which verifies at `SMT_DEPTH = 32` — **229k CU**
  (depth-256 exceeded the 1.4M ceiling). The shallow tree also fixes root churn (the withdrawals
  root changes only on withdrawals, not every block). Live e2e: J1Zfq withdrew 5,000,000 on L2
  (burned) → on-chain verify succeeded → **L1 recipient received 5,000,000 lamports**. The bridge
  now works **both directions** on devnet.

- [x] **RPC gaps break some wallets/dapps.** — **SHIPPED 2026-06-12.** `getSignaturesForAddress`
  now honours `before`/`until` cursors; added `getProgramAccounts` (dataSize/memcmp filters);
  added WebSocket `slotSubscribe`/`signatureSubscribe`/`accountSubscribe` (HTTP on the main
  port, WS on port+1 per web3.js convention). Live-verified: `confirmTransaction` confirms via
  signatureSubscribe push, slot + account notifications fire. (`ufw allow 8900/tcp` on OVH.)

- [ ] **Trusted setup is a dev ceremony.** The shielded pool secures value but the Groth16
  keys come from a dev `ptau` (2^16) in `circuits/scripts/`. **Fix:** run a real
  multi-contributor Powers-of-Tau / phase-2 ceremony (or switch to a transparent system),
  publish transcripts, re-pin vk blobs.

- [x] **Graceful shutdown / readiness.** — **SHIPPED 2026-06-12.** SIGTERM/SIGINT flip
  `Node.draining` (health → unhealthy so the LB drains), the producer loop exits cleanly
  (in-flight block finishes), then both RPC servers stop. Verified live on systemd restart.

---

## P2 — maturity / polish

- [ ] **Load + scale untested.** Validate under sustained tx load; add state pruning / snapshots
  / compaction strategy as the redb file grows; benchmark block production at target TPS.
- [ ] **Security audits.** Independent audit of circuits, the two Anchor programs, and the node
  (especially the enshrined pool value paths and the SMT verify).
- [ ] **Upgrade & key-rotation runbooks.** Documented migration, key rotation, incident response,
  on-call. Tie L1 program upgrades to the multisig/timelock.
- [ ] **Blockhash window at 500 ms slots.** `BLOCKHASH_QUEUE_CAPACITY = 150`
  (`constants.rs:15`) ≈ 75 s of validity at 500 ms slots vs Solana's ~90 s; confirm wallet
  retry windows are comfortable (heartbeat-only periods stretch real-time validity, so likely fine).
- [ ] **Config hardening.** RPC → localhost behind the proxy; tighten `ufw`; disable the faucet;
  set `RUST_LOG` retention/rotation policy; document every `[l1]`/`[rpc]` knob.

---

## Quick wins (shipped 2026-06-11)

1. ◑ jsonrpsee limits **done**; bind `127.0.0.1` + nginx TLS/rate-limit **pending domain** (P0 #1)
2. ✅ Gate `requestAirdrop` on `faucet_enabled` (P0 #3)
3. ✅ Real `getHealth` tied to tip liveness (P0 #6)
4. ◑ Nightly redb backup + rotation **done**; off-box push **pending an rclone remote** (P0 #5a)
5. ✅ Prometheus `/metrics` core gauges (P1 #1)

**Pending external input to finish #1 + #4:** a domain (TLS/proxy) and an off-box backup
target (S3/B2/rclone remote).

**Next code slices (no external input needed):** mempool anti-spam (P0 #2), key custody →
multisig/timelock (P0 #4), `reconstruct-from-l1` tool (P0 #5b), L1 batcher robustness (P1).

## What is already solid (don't re-touch)

- Atomic two-phase block commit with fsync durability (`store.rs`).
- Bounded mempool with signature dedup; store-level replay protection.
- State persists + tip restores across restart (verified live).
- L1 DA posting with `da_hash` integrity + sequential, idempotent batch records.
- solana-CLI / web3.js wire compatibility for the implemented method subset.
