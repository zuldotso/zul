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

The setup here is a single-contributor (dev) ceremony with fixed entropy —
fine for local nets and testnets. Before any value-bearing deployment,
replace `scripts/setup.mjs` with a real multi-party ceremony and publish the
transcript.
