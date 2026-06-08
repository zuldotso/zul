# Zul Privacy Hardening — external-observer model

Companion to `docs/PRODUCTION-READINESS.md`, focused only on the privacy axis.

## Threat model (scoped)

- **The centralized operator (sequencer) is a trusted party.** Privacy *against the
  operator* is explicitly out of scope. This removes off-chain / network vectors:
  IP addresses, pre-inclusion mempool visibility, submission ordering.
- **The adversary is an external observer of the public ledger** — anyone reading the
  L2 chain, the explorer, public account state, and the L1 DA batches (which contain
  the full, zstd-compressed block transactions). The chain is public, so on-chain
  footprints are visible to this adversary regardless of operator trust.
- **The question:** can an external observer link a user's *entry* (shield) to their
  *exit* (unshield), or reconstruct the internal transfer graph, from public data alone?

Matches the project's stated model (PLAN.md §5: "privacy holds against outside observers").

---

## What already holds — do NOT re-touch

- **Internal transfer-graph unlinkability.** An observer sees only nullifiers, output
  commitments, the proof, the Merkle root, and (opaque) encrypted notes. Inputs↔outputs
  and amounts are not recoverable. `nf = Poseidon3(commitment, leaf_index, s)` cannot be
  walked back to its commitment without `s`. This is the core win and it works.
- **Zero-knowledge survives trusted-setup compromise.** Groth16 toxic waste breaks
  *soundness* (forgery → theft), **not** zero-knowledge. So the trusted setup is a
  **funds-safety** risk, not an external-observer *privacy* risk — track it in
  `PRODUCTION-READINESS.md`, not here.
- **Note encryption** (X25519 + XChaCha20-Poly1305): recipients find notes by trial
  decryption; amounts/owners are not public.

## What still leaks to an external observer (the work)

Public, on-chain, in scope even with a trusted operator:

| Stage | Visible to observer | Hidden |
|------|---------------------|--------|
| shield   | depositor address + **amount** + fee payer | note secret |
| transfer | nullifiers · commitments · proof + **fee payer** | amounts, sender↔recipient |
| unshield | recipient address + **amount** + fee payer | change note |

Resulting deanonymization vectors: amount correlation (public edges + arbitrary values),
public fee-payer linkage, timing correlation, small anonymity set, per-asset set
fragmentation.

---

## Improvements, in priority order

Ordering is by impact on entry↔exit unlinkability against an external observer. The two
P0 items are **complementary — neither delivers the property alone** (amount matching
defeats a hidden fee payer; a public fee payer defeats hidden amounts), so they ship
together.

### P0 — without these, entry↔exit is linkable today

1. **Amount privacy at the public edges.**
   - Finding: commitments bind an arbitrary `amount`
     (`note_commitment = Poseidon3(asset_id, amount, secret)`, `chain/zul-privacy/src/poseidon.rs`);
     shields publish the amount (`processor.rs` binds the public deposit), unshields pay a
     public `amount` to a public recipient (`instruction.rs` / `processor.rs`). So
     `shield X` ↔ `unshield ≈X` matches by value.
   - **Fix (recommended):** restrict the **public edges to fixed denominations**
     (e.g. 0.1 / 1 / 10 / 100 per asset); keep **internal transfers arbitrary-amount**
     (already hidden). Shielding/unshielding then leaks only "which denomination," not an
     exact, matchable value. Change-making happens privately inside transfers.
   - Alternatives: amount bucketing/quantization (weaker, simpler) or a mandatory
     re-split transfer before exit (no exit amount equals any entry amount).
   - Touches: note/instruction format, `processor.rs` shield/unshield validation, the
     SDK builders, and the explorer's shielded stats.

2. **Relayer / private fee path (remove the public fee payer).**
   - Finding: pool txs are ordinary Solana txs with a **public** fee payer paying
     `POOL_TX_FEE` (`chain/zul-node/src/pool.rs`, `node.rs` pool handling; the e2e script
     funds a fresh payer per op). The fee payer is on-chain, so an external observer links
     ops that share one — and "use a fresh payer" only moves the link to the payer's public
     funding edge. Already flagged as a v1 caveat (PLAN.md:72) with a v2 relayer planned.
   - **Fix:** in-circuit fee output — the proof pays the fee to a relayer address, so the
     user's account never appears as fee payer. **Under the trusted-operator model the
     operator can BE the relayer**, so this adds no new trusted party. This is the missing
     piece that makes the fee path private *to external observers*, not just to the operator.
   - Touches: transfer/unshield circuits (fee output signal), verifier wire format,
     `processor.rs`, SDK, a relayer endpoint.

### P1 — raise the quality once P0 lands

3. **Withdrawal timing decorrelation.** Shield-then-immediate-unshield links by slot
   timestamp, especially with few users. **Fix:** randomized delays and/or a batched
   withdrawal queue so exits don't track entries in time.

4. **Anonymity-set bootstrapping.** An empty pool gives ~0 privacy regardless of crypto.
   **Fix:** track + surface the live anonymity-set size; optionally gate withdrawal until
   the set has grown by ≥ N members since the deposit; seed liquidity / incentives. This is
   a product/liquidity problem, not only engineering.

5. **Asset-set fragmentation.** `asset_id` lives in the commitment, so each asset mixes
   only with itself → smaller sets. **Fix:** evaluate a shared-denomination layer or a
   wrapped common unit; or accept and document the tradeoff and concentrate liquidity.

### P2 — metadata hygiene & completeness

6. **Constant-size encrypted notes / tx shapes.** Variable `encrypted_note` length or tx
   size can fingerprint operations. **Fix:** fixed-size ciphertexts and uniform tx layout.

7. **Client-side privacy UX.** Fresh addresses per shield/unshield, guidance on funding
   sources, and wallet flows that prevent linkage footguns (the protocol can't fix a user
   who withdraws to their KYC'd deposit address).

8. **Anonymity-set instrumentation.** Metrics for set size, denomination occupancy, and
   shield→unshield time deltas, so "is privacy actually working" is measurable, not assumed.

---

## Definition of done (external-observer privacy)

The property "an external observer cannot link a user's shield to their unshield" holds
when: **P0 #1 (denominated edges) + P0 #2 (relayer fees)** are live, **P1 #3 (timing)**
is in place, and **P1 #4 (anonymity set)** is non-trivial. Until all four, assume entry↔exit
is linkable for a meaningful fraction of flows.

Sequencing: **#1 and #2 first and together**, then #3, then #4; P2 items any time after P0.
