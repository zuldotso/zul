# Zul mainnet — trusted setup ceremony

The shielded-pool proving keys must come from a real multi-party ceremony before
mainnet. The dev keys (`circuits/scripts/setup.mjs`, fixed entropy, single
contributor) are **forgeable** — anyone who knows the toxic waste can mint fake
shielded value. This is the #1 launch blocker.

> **Status (2026-06-16):** a first ceremony has been run and its output shipped
> (`circuits/artifacts/CEREMONY-TRANSCRIPT.md`) — Hermez phase-1, one local
> phase-2 contributor, Bitcoin-block beacon. That clears the forgeable dev keys.
> For a multi-party guarantee, re-run the steps below with several independent
> contributors and redeploy `vk.bin` before the pool holds large value.

Security assumption: **at least one honest contributor** never reveals or stores
their randomness. So run it with several unrelated participants on separate
machines. Tooling: `circuits/scripts/ceremony.mjs`.

## Phase 1 (powers of tau) — reuse, don't self-generate

Self-generating phase-1 is itself a single-party trusted setup. Instead use the
**Hermez perpetual powers of tau** (a large, public, multi-contributor phase-1).
`ceremony.mjs init` downloads `powersOfTau28_hez_final_<power>.ptau` matching the
circuits' compiled size. **Verify its hash out-of-band** against the published
Hermez attestation before using it.

## Phase 2 (per-circuit) — many independent contributions

For each circuit (`transfer`, `unshield`):

1. **Coordinator:** `node scripts/ceremony.mjs init` → produces
   `build/<circuit>/<circuit>_0000.zkey`. Publish its hash.
2. **Each contributor in turn**, on their own clean machine:
   ```sh
   node scripts/ceremony.mjs contribute <circuit> <in.zkey> <out_N.zkey> "alice"
   ```
   snarkjs prompts for entropy interactively (never passed on argv, so it is not
   captured in shell history). The contributor types fresh secret randomness,
   then **destroys it**. They publish the output hash and hand `out_N.zkey` to
   the next contributor (or back to the coordinator).
3. **Coordinator finalize** with a public random beacon (a value nobody could
   predict at contribution time — e.g. a future Bitcoin block hash):
   ```sh
   ZUL_BEACON=<btc-block-hash-hex> node scripts/ceremony.mjs finalize <circuit> <last.zkey>
   ```
   This applies the beacon, **verifies the full transcript** (`zkey verify`), and
   exports `verification_key.json`.

## Ship the result

```sh
npm run export-keys     # circuits/scripts/export-vk.mjs → artifacts/<c>.vk.bin
```

Commit the new `circuits/artifacts/{transfer,unshield}.vk.bin` (public params).
`scripts/init-live.sh` copies them into `data/<net>/params/`, and the node loads
them at boot ("loaded shielded-pool verifying keys"). The cross-test
(`chain/zul-privacy/tests/snarkjs_crosstest.rs`) still pins circom↔Rust
equivalence — re-run it after to confirm a real snarkjs proof verifies in the
node verifier with the ceremony keys.

## Transcript

Publish, for auditability: every intermediate `.zkey` hash, each contributor's
name + attestation that they destroyed their entropy, the phase-1 ptau hash, and
the beacon source + value. Anyone can then re-run `zkey verify` to confirm the
final key descends from the published chain.
