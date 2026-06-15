# Zul circuits

circom 2 circuits for the shielded pool, with a dev-grade Groth16 setup.

## Circuits

- `transfer.circom` — 2-in/2-out join-split. Public signals (fixed order):
  `[root, nullifier0, nullifier1, out_commitment0, out_commitment1, fee]`.
  Asset id is private; the circuit enforces all notes share it and that
  value is conserved per asset. Fee is allowed only for native ZUL.
- `unshield.circom` — spend one note to a public recipient + amount with a
  private change note. Public signals:
  `[root, nullifier, change_commitment, asset_id, amount, recipient]`.

The note scheme (`lib/note.circom`) and Merkle tree (`lib/merkle.circom`)
mirror `chain/zul-privacy` exactly. The cross-language test
`chain/zul-privacy/tests/snarkjs_crosstest.rs` proves a real snarkjs proof
verifies in the node.

## Build

```sh
npm install
npm run build          # compile + trusted setup + export verifying keys
node scripts/gen-fixture.mjs   # regenerate the Rust cross-test fixture
```

Outputs:
- `build/<circuit>/` — r1cs, wasm, final zkey (gitignored; large)
- `artifacts/<circuit>.vk.bin` — verifying key blob the node loads (committed)
- `chain/zul-privacy/tests/fixtures/transfer_proof.json` — cross-test fixture

## Trust

`scripts/setup.mjs` is a single-contributor (dev) ceremony with fixed entropy —
fine for local nets and testnets, but **forgeable**. For a value-bearing
(mainnet) deployment, run the real multi-party ceremony instead:

```sh
npm run ceremony            # init | contribute | finalize (see the runbook)
```

`scripts/ceremony.mjs` uses a trusted Hermez phase-1, takes many independent
phase-2 contributions, and closes with a public beacon + transcript
verification. Full procedure: [docs/MAINNET-CEREMONY.md](../docs/MAINNET-CEREMONY.md).
