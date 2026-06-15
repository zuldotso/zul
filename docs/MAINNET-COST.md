# Zul mainnet — settlement cost model

Every L1 action on mainnet-beta spends real SOL: one-time program rent, per-batch
rent + transaction fees. This sizes the funder / bridge-authority balance and the
`batch_interval_slots`. All rent figures use `Rent::default()`:
`minimum_balance(n) = (n + 128) · 3480 · 2` lamports.

## TL;DR — how much SOL the funder wallet needs

- **Deployment (one-time): ~5 SOL** — `zul_settlement` ≈ 2.79 + `zul_da_log` ≈
  1.24 program rent (recoverable only by closing the programs) + deploy fees/buffer.
- **Operating: a few SOL goes a long way.** The only recurring on-chain cost is
  the per-batch `Batch` PDA rent (~0.00187 SOL, accumulating) + trivial tx fees;
  DA bytes are nearly free (logged, not stored). Ceiling ≈ **0.54 SOL/day** at
  full activity (`batch_interval 600`); a quiet chain is well under 0.1 SOL/day.

**Recommendation: fund with ~10 SOL.** That covers deployment (~5), withdrawal-
claim rent, and months of operating runway with headroom. You can start as low as
**~6–7 SOL** (deploy + a small buffer) and top up. The batcher pauses below
**0.02 SOL**, so alert well above that and refill from a cold reserve (keep the
hot bridge-authority balance modest — see `MAINNET-CUSTODY.md`).

## One-time: program deployment (recoverable only by closing the program)

Deploy with `--max-len = exact .so size` — the upgradeable loader otherwise
allocates 2× the programdata and doubles this rent.

| Program | `.so` size | programdata bytes (`45 + size`) | rent-exempt |
|---|---|---|---|
| `zul_settlement` | 406,832 B | 406,877 | **≈ 2.83 SOL** |
| `zul_da_log` | 177,912 B | 177,957 | **≈ 1.24 SOL** |

(`zul_settlement` grew 308,880 → ~407 KB after adding `anchor-spl` for the SPL
bridge + the Token-2022 extension safety checks — recompute with
`stat -c%s programs/target/deploy/*.so` before deploying, since the figures drift
with code changes.)

Budget **~5 SOL** for deployment with headroom.

## Recurring: per batch

Each `post_batch` creates a `Batch` PDA (8 + 132 = 140 B):

- Batch PDA rent: `(140 + 128)·6960` = **0.00187 SOL** (locked per batch — this
  is the only account a batch creates, and it is never closed, so it accumulates).
- Transaction fees: the `post_batch` tx + one tx per DA chunk, 5,000 lamports
  each (+ any priority fee). **DA chunks create NO account** — `zul_da_log` stores
  nothing, it logs the chunk as instruction data (it lives in the ledger), so a
  chunk costs only its tx fee, not rent. A batch is a handful of ≤900 B chunks.

So a batch ≈ **0.00187 SOL locked rent** + ~0.00001–0.00003 SOL fees. The Batch
PDA rent dominates and accumulates; the DA bytes are nearly free (just fees).

Rough cost at `batch_interval_slots = 600` (≈ 288 batch windows/day). The batcher
**skips all-empty windows**, so this is a ceiling for a continuously-active chain:

```
288 batches/day · 0.00187 SOL ≈ 0.54 SOL/day   (worst case, continuous activity)
```

A quiet early mainnet posts far fewer batches (only non-empty windows), so expect
well under 0.1 SOL/day in practice. Tune the knob against traffic:
- Higher `batch_interval_slots` → fewer, larger batches → less accumulated PDA
  rent, higher settlement latency (longer until a withdrawal is claimable).
- 600 is the mainnet default (vs 120 on testnet). Raise it (e.g. 1200–2400) if
  cost matters more than latency.

## Per withdrawal claim (paid by the claim submitter)

`claim_withdrawal[_spl]` creates a `WithdrawalClaim` PDA (~64 B →
`(64+128)·6960` ≈ **0.00134 SOL**, locked) and, for SPL, may init the recipient
ATA (≈ **0.00204 SOL**) — both paid by the tx payer (the bridge authority in the
ops bins). Deposits cost the *depositor*, not the funder: `deposit_spl` inits the
per-mint vault ATA on first use, paid by the depositor.

## Bridge-authority balance + backpressure

The bridge authority (`bridge_authority_key_path`) signs every `post_batch`,
`post_chunk`, and `claim_withdrawal[_spl]`. The batcher **pauses** when its
balance drops below **0.02 SOL** (`l1.rs` MIN backpressure) rather than spamming
failing posts. Keep it funded well above that — top up so it covers at least a
week of the rate above, and alert on low balance (Prometheus
`zul_l1_authority_lamports`).

## Wrapped-token account rent (L2, not L1)

Each new SPL depositor/mint synthesizes L2 accounts whose rent-exempt lamports
(`(82+128)·6960` mint ≈ 0.00146 ZUL, `(165+128)·6960` ATA ≈ 0.00204 ZUL) are
minted from native supply — a negligible, Stage-0-acceptable native-supply
inflation per first-time account, not an L1/SOL cost.
