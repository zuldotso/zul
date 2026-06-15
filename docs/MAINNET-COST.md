# Zul mainnet — settlement cost model

Every L1 action on mainnet-beta spends real SOL: one-time program rent, per-batch
rent + fees, and per-DA-chunk rent. This sizes the bridge-authority balance and
the `batch_interval_slots`. All rent figures use `Rent::default()`:
`minimum_balance(n) = (n + 128) · 3480 · 2` lamports.

## One-time: program deployment (recoverable only by closing the program)

Deploy with `--max-len = exact .so size` — the upgradeable loader otherwise
allocates 2× the programdata and doubles this rent.

| Program | `.so` size | programdata bytes (`45 + size`) | rent-exempt |
|---|---|---|---|
| `zul_settlement` | 400,648 B | 400,693 | **≈ 2.79 SOL** |
| `zul_da_log` | 177,912 B | 177,957 | **≈ 1.24 SOL** |

(`zul_settlement` grew from 308,880 B after adding `anchor-spl` for the SPL
bridge — recompute these with `stat -c%s programs/target/deploy/*.so` before
deploying, since the figures drift with code changes.)

Budget **~5 SOL** for deployment with headroom.

## Recurring: per batch

Each `post_batch` creates a `Batch` PDA (8 + 132 = 140 B):

- Batch PDA rent: `(140 + 128)·6960` = **0.00187 SOL** (locked per batch).
- Transaction fee: 5,000 lamports base (+ any priority fee) = **~0.000005 SOL**.

Each batch also posts its DA payload as `post_chunk` accounts (≤ 800 B/chunk).
A chunk account of ~828 B costs `(828+128)·6960` ≈ **0.00665 SOL** rent each, plus
its tx fee. An empty/low-traffic batch is 1 chunk; busy batches scale with L2
bytes.

**Dominant term is rent, and it accumulates** (the settlement is append-only —
PDAs are not closed). Rough monthly cost at `batch_interval_slots = 600` (~1
batch / 5 min on mainnet-beta ≈ 288 batches/day):

```
288 batches/day · (0.00187 batch + ~0.00665 chunk) SOL ≈ 2.45 SOL/day  (mostly idle, 1 chunk)
```

Tune the knob against traffic:
- Higher `batch_interval_slots` → fewer, larger batches → less rent/fee overhead,
  higher settlement latency (longer until a withdrawal is claimable).
- 600 is the mainnet default (vs 120 on testnet). Raise it (e.g. 1200–2400) if
  cost matters more than settlement latency.

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
