# Zul shielded-pool trusted setup — transcript

Groth16 phase-2 ceremony for the mainnet shielded pool (`transfer`, `unshield`).
Run 2026-06-16. Every value below is a public commitment; anyone can re-run
`snarkjs zkey verify <circuit>.r1cs <ptau> <circuit>_final.zkey` and compare.

These keys **replace the forgeable dev keys** (`scripts/setup.mjs`, fixed
entropy). The committed `artifacts/{transfer,unshield}.vk.bin` are the output.

## Phase 1 — powers of tau (reused, not self-generated)

Hermez perpetual powers of tau, `powersOfTau28_hez_final_16.ptau`.

| | |
|---|---|
| source | `https://storage.googleapis.com/zkevm/ptau/powersOfTau28_hez_final_16.ptau` |
| size | 75,580,568 bytes |
| sha256 | `1c401abb57c9ce531370f3015c3e75c0892e0f32b8b1e94ace0f6682d9695922` |

Verify this hash out-of-band against the published Hermez attestation before
trusting it.

## Phase 2 — per-circuit contributions

### Circuit hashes (r1cs commitments)

```
transfer   3e838763 924db39d 1e86c808 98640e02
           8821b8a3 aa2e8c6d bbd7c18e 2c1373cb
           a85d1813 d90747da e8db4367 49305f51
           623d77de 42129c70 3a91c551 5847bf03

unshield   a2462d40 cd0f2816 baa57956 fc0db29e
           d38297d9 7780f75a be562aaa 10dd078a
           e46365bc bf88526d 1229ed9a da46cd8a
           18cb99b4 59071758 0677a748 62a78cd2
```

### Contribution #1 — "zul-local-1"

Single local contributor. Entropy was 64 bytes from the OS CSPRNG
(`crypto.randomBytes`), piped into snarkjs over stdin — never on argv, never
written to disk, never printed — and discarded when the process exited.

```
transfer   e9c8c273 8961df1c 554dd4d8 561c2cd3
           f1e3dbe9 72613895 525202aa a3692c6d
           a16533b7 1e2d6633 9f7ef7b7 6f8998ed
           f6cd5c16 a12f6d69 79b92e6a d24f0930

unshield   10324f04 b8dc2640 3d976863 f37702a7
           681a556b 8f697afb 644b45e3 5077a66f
           d8deb3c0 3f7c14a9 9fe242d3 68ed8efe
           239dc8b2 d5b51d0f f530677c c991be6f
```

### Contribution #2 — public beacon (final)

Beacon = Bitcoin block **#953,837** hash (unpredictable at contribution time),
applied with iterations exponent 10.

```
beacon generator   000000000000000000013b56e163c21647bbb335d7599a7f55a8bfdf4a0cf9b0

transfer (beacon)  c44044d6 1c197533 719a0574 6302d370
                   667467f0 7ad9d9df b50ec1ec 8b571f69
                   8a13b7e2 2c3c6c88 58765621 bb39c4f4
                   94a15bf1 6792decd 75c47754 c926c2c4

unshield (beacon)  e93b11a8 dde7ff6a 51c2169d 817717b3
                   229bd8b9 d37931fe 490f0483 94bc09fe
                   56222c76 0e936e27 c6153dd6 9caa3f18
                   b128548c 89e9520e eca7bb93 0dd21bad
```

`snarkjs zkey verify` reported **ZKey Ok!** for both circuits — the final keys
provably descend from the r1cs + Hermez phase-1 + contribution #1 + the beacon.

## Output — committed verifying keys

| file | sha256 |
|---|---|
| `artifacts/transfer.vk.bin` | `89c7d5037eb8d457445d40a94c4f45d6b6760dbc7d42b26c589bccaf5954b6ab` |
| `artifacts/unshield.vk.bin` | `1d779daea39fdc71fcf93c822d5f3201c714194a763267a62365e7c82e66e188` |

Node binding re-checked after the ceremony: a fresh snarkjs proof made with the
ceremony proving key verifies in the in-node arkworks verifier with the above
`vk.bin` (`chain/zul-privacy` cross-test `snarkjs_transfer_proof_verifies_in_node`
passes; tampered proofs/inputs are rejected).

## Trust model — read before relying on this

The security assumption is **"at least one honest phase-2 contributor"**. This
run had **one** contributor, on one machine. So the real assumption here is:
*that machine was not compromised and the ephemeral entropy was truly
destroyed.* That is strictly stronger than the prior fixed-entropy dev keys
(trivially forgeable) and rests on the genuinely multi-party Hermez phase-1 —
but it is **not** a multi-party phase-2.

For a higher-assurance launch before the pool holds significant value, re-run
phase-2 with several independent contributors on separate machines
(`ceremony.mjs contribute` in turn, each destroying their entropy) ahead of the
beacon, then redeploy the resulting `vk.bin`.
