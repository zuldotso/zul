# Zul Production-Hardening Changelog (2026-06)

A summary of the single-sequencer production-hardening + bridge work: the order
it was done in and the bugs/issues fixed along the way. Each item was
implemented → unit-tested → deployed to the live OVH node → verified on devnet.
Plans: `PRODUCTION-READINESS.md`, `PRIVACY-HARDENING.md`.

## Order of work

1. **Mempool anti-spam** — per-fee-payer in-flight cap so one key can't fill the
   queue and starve others (`zul-node/src/mempool.rs`).
2. **Config knobs** — exposed `heartbeat_ms`, `mempool_capacity`, `max_per_payer`
   in `node.toml` `[node]` (tunable without a rebuild).
3. **Graceful shutdown** — a `draining` flag flips `getHealth` unhealthy, the
   producer loop exits cleanly (in-flight block finishes), then RPC stops.
   Handles **SIGTERM** (systemd) as well as SIGINT.
4. **L1 batcher robustness** — re-derive the batcher from L1 on a post failure
   (reorg self-heal) + pause posting below a min authority balance.
5. **Prometheus metrics** — `/metrics` (+ `/health`) on a dedicated port; 11
   gauges incl. slot, mempool depth, block age, L1 authority balance, batcher lag.
   *(Deployed #1–#5 as one batch and verified live.)*
6. **RPC gaps** — `getSignaturesForAddress` `before`/`until` pagination + new
   `getProgramAccounts` (dataSize/memcmp filters).
7. **Disaster-recovery tool** — `zul-recover`: refetch all settlement batches +
   their DA chunks from L1, verify `da_hash` integrity, recover every block/tx.
8. **WebSocket subscriptions** — `slotSubscribe` / `signatureSubscribe` /
   `accountSubscribe` (HTTP on the main port, WS on port+1 per web3.js).
9. **Deposit bridge (L1→L2)** — a watcher credits SOL deposited into the L1 vault;
   exactly-once via a *receipt account* (existence = credited) committed
   atomically with the credit.
10. **Withdraw bridge (L2→L1)** — a withdraw tx burns the L2 balance + commits a
    settlement-compatible withdrawal leaf; `getWithdrawalProof` serves the proof;
    `zul-l1-claim` claims on L1.
11. **Withdraw CU fix** — replaced the depth-256 verify with a separate **depth-32
    withdrawals SMT** so `claim_withdrawal` fits the compute budget; payout lands.

Result: operational P0 done + most of P1; the bridge works **both directions** on
devnet (deposit: 0.05 SOL → L2; withdraw: L2 5M burned → **L1 received 5,000,000**).

## Bugs / issues fixed

- **jsonrpsee 0.26 limits** — `max_connections`/body sizes are not on
  `Server::builder()`; they live on `ServerConfig::builder()` → `.set_config()`.
- **Graceful shutdown never ran under systemd** — the code only caught SIGINT
  (ctrl-c), but `systemctl restart` sends **SIGTERM**. Added a `cfg(unix)` SIGTERM
  handler; verified `draining → shutdown complete` on restart.
- **DR verify assumed contiguous batches** — the batcher skips all-empty windows,
  so batch slot ranges have **gaps**. Changed the check to monotonic, non-overlapping.
- **DR `state_root` mismatch** — a batch's `state_root` is the *tip* slot's root
  (includes empty-block Clock-sysvar updates the DA omits), so it ≠ the payload's
  last-block root. Dropped that equality check; `da_hash` is the integrity tie.
- **DR test helper panic** — `Block::seal` asserts the header's `transactions_root`;
  the test passed `[0;32]`. Fixed to compute the real root.
- **WS subscription API + endpoint** — jsonrpsee `sink.send` takes a `Box<RawValue>`
  (wrap only the result payload); web3.js derives the WS endpoint at **port+1**, so
  the server binds both ports and `ufw allow 8900/tcp` was needed.
- **Deposit watcher import** — `fetch_deposits` is an `L1Client` *trait* method, not
  inherent; needed `use zul_bridge::L1Client`.
- **Withdraw claim exceeded the compute budget** — the depth-256 pure-Rust-blake3
  SMT verify (~256–514 hashes) blew Solana's 1.4M CU ceiling (`sol_blake3` syscall
  is unavailable on devnet). Precomputing the empty-subtree hashes (`empties.bin`)
  halved it (514→258); the real fix was the depth-32 withdrawals tree (→ **229k CU**).
- **Withdraw proof was ephemeral** — the L2 state root changes **every block**
  (sysvars), so a proof against it rarely matched a posted batch root. The separate
  withdrawals tree fixed this: its root changes only on withdrawals.
- **`include_bytes!` size cycle** — switching the settlement to depth-32 made the
  embedded `empties.bin` the wrong size to compile; regenerated it (1056 B) from the
  node side (same hashes) to break the cycle.
- **Batch account layout change** — adding `withdrawals_root` made new batches 132 B
  vs old 100 B; `fetch_batch_record` now parses **both** layouts so old batches don't
  break the scan.
- **Devnet RPC lag** — after a successful claim the public devnet `getBalance` still
  read 0; Helius showed the recipient actually received the 5,000,000 (the tx had
  `err: null`, consumed 229k CU). Use Helius for ground truth.

## Notes / follow-ons

- The withdrawals tree is **in-memory**, rebuilt on boot by scanning blocks for
  successful withdraw txs (O(blocks) startup). Persisting it is a production follow-on.
- `heartbeat_ms` was temporarily set high during withdraw debugging (to freeze the
  state root); restored to 10s once the depth-32 tree removed that constraint.
- Pending external input: a domain (RPC TLS/proxy), an off-box backup target, an
  upgrade-authority multisig. Remaining axis: privacy (denominations + relayer).
