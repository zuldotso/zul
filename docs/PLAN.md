# Zul — Privacy Layer 2 on Solana

Implementation plan + build status.

## Update (2026-06-16)

Since the original plan below, three things changed materially (the plan text
is kept for context but read these first where they differ):

1. **Renamed PL2 → Zul** and relaunched a fresh chain (devnet + OVH).
2. **Bridge generalized to arbitrary SPL, bidirectional.** Not just SOL/wSOL:
   any L1 SPL mint (Token or Token-2022) bridges to a deterministic L2 *wrapped*
   token, deposit **and** withdraw. The withdrawal commitment now binds an
   `asset_id` so the L1 claim releases the right asset. See §4.2.
3. **Bridged SPL can be shielded privately.** The shielded pool was native-only
   in the first cut; it now moves wrapped SPL in/out of a per-mint pool vault, so
   shield / private transfer / unshield work for any bridged asset (§4.3, §6).

A parallel **mainnet** system was also added (per-network `config/{testnet,
mainnet}/`, settling to mainnet-beta). Mainnet runbook + the value-bearing
launch blockers (real MPC ceremony, key custody, cost) live in
`DEPLOY-MAINNET.md`, `MAINNET-CEREMONY.md`, `MAINNET-CUSTODY.md`,
`MAINNET-COST.md`. Chain workspace is now ~117 Rust tests (125 incl. the L1
program).

## Build status (2026-06-10)

All components scaffolded and unit/integration-tested locally. Runtime
integration that needs real keys / the OVH sequencer / Solana devnet is
deliberately deferred (see §5, §9). Test totals: chain workspace 100 Rust
tests; SDK 16; indexer 2; Anchor 6 host tests + BPF `.so` build; circuits
compile + a real snarkjs↔node proof cross-test; explorer `next build` +
renders.

| Component | State | Verified by |
|-----------|-------|-------------|
| `chain/zul-primitives` | done | 21 tests |
| `chain/zul-store` | done | 18 tests (SMT proofs, atomic commit, nullifiers) |
| `chain/zul-executor` | done | 5 tests (real solana-svm 3.1; SPL mint runs in-VM) |
| `chain/zul-privacy` | done | 28 + 4 cross-tests (Groth16, Poseidon, pool service) |
| `chain/zul-rpc` | done | 8 tests (Solana JSON-RPC subset) |
| `chain/zul-bridge` | done | 6 tests (batcher, deposits; L1 client mockable) |
| `chain/zul-node` | done | 10 tests (block loop, faucet, shield e2e, restart) |
| `programs/` (Anchor) | done | BPF build + 6 host tests; SMT cross-pinned to L2 |
| `circuits/` | done | compile + snarkjs↔arkworks proof cross-test |
| `sdk/` | done | 16 tests; bincode cross-test with Rust |
| `indexer/` | done | 2 tests; idempotent slot-walk |
| `explorer/` | done | next build + live render |

Cross-language bindings proven (the parts most likely to silently drift):
- snarkjs Groth16 proof verifies in the arkworks node verifier
  (`zul-privacy/tests/snarkjs_crosstest.rs`)
- TS SDK bincode == Rust serde (`zul-privacy/tests/sdk_compat.rs`)
- L1 (Anchor) blake3 SMT == L2 store SMT, pinned by shared constants

Deferred to on-server / devnet (needs real keys): full pool transfer/unshield
runtime with browser-generated proofs; settlement/bridge against devnet;
multi-batch DA reconstruction. *(The SPL pool value path, once listed here, is
done — see the 2026-06-16 update above.)*

## 1. Goal

A real Layer 2 chain that settles to Solana:

- Own block production: single PoA sequencer, fixed slot time (500 ms, configurable)
- Real SVM execution: runs standard Solana transactions and programs (System, SPL Token,
  Token-2022, user-deployed BPF programs)
- Native gas token `ZUL`: the chain's lamport unit. SOL does not exist natively on the L2;
  all fees are paid in ZUL
- Privacy: zk shielded pool for ZUL and any bridged asset — native SOL and
  arbitrary SPL tokens (shield / private transfer / unshield), enshrined as a
  builtin program in the node
- Settlement and bridge: Anchor programs on Solana (devnet first), optimistic model
- Data availability: compressed batch data logged to the Solana ledger
- Own explorer: Next.js 16 + Prisma 6 + PostgreSQL, fed by a dedicated indexer
- TypeScript SDK + web wallet flows (wallet-adapter; no hardware wallets)

### Non-goals (v1)

- Decentralized sequencer / multi-validator consensus
- On-chain-enforced fraud proofs or validity (zkSVM) proofs of execution
- General private smart contracts (Aztec-style). Privacy scope = shielded value transfer
- Mainnet deployment, audits, tokenomics beyond a minimal genesis

## 2. Key decisions

| # | Topic | Decision | Why / alternatives |
|---|-------|----------|--------------------|
| 1 | Execution engine | Embed Agave's `solana-svm` crate (Anza "SVM API") in our own Rust node, implementing `TransactionProcessingCallback` over our account store | The same pipeline real validators use — a genuine SVM L2 (the MagicBlock / Sonic approach). Fallback: LiteSVM if integration friction is too high. Rejected: custom VM (would not run Solana programs) |
| 2 | Consensus | Single PoA sequencer, slot-based production | The honest v1 for a solo-built L2; a sequencer set is roadmap |
| 3 | State commitment | Sparse Merkle tree over (pubkey → account hash), blake3; root in every block header | SVM has no native state trie — every SVM rollup must fill this gap itself. Lattice hash rejected: no inclusion proofs for withdrawals |
| 4 | Settlement | Optimistic, Stage 0: sequencer posts batch records (state root + DA hash); challenge window is procedural, not yet enforced by on-chain re-execution | On-chain SVM fraud proofs are out of v1 scope; trust model disclosed in §5 |
| 5 | Data availability | Batch tx data zstd-compressed, chunked into noop-log instructions on Solana; hash anchored in the settlement record | Cheapest way to make data public; the node also serves batches over RPC |
| 6 | Privacy | UTXO shielded pool, multi-asset (native ZUL + any bridged SPL): notes carry an asset id; Poseidon note commitments, nullifiers, Merkle tree; Groth16 over BN254; circuits in circom 2; proving via snarkjs WASM in the browser; verifying natively in the node via arkworks | Verification is native — our chain, no syscall/CU budget. Rejected: Token-2022 confidential transfer (hides amounts only, not graph); Aztec-style private compute (research-team scale) |
| 7 | Private tx fees | v1: public fee payer (linkability caveat documented). v2: relayer with in-circuit fee output | Simplicity first |
| 8 | RPC | Solana JSON-RPC subset (HTTP + WebSocket) | `@solana/web3.js`, wallet-adapter, and the `solana` CLI work against the L2 unchanged — huge leverage |
| 9 | Explorer | Next.js 16 + Prisma 6 + Postgres + separate TS indexer worker | Per global stack rules; deployed on Railway |
| 10 | Dev + hosting | Rust node developed, built, and hosted on the user's OVHcloud Linux server (remote dev over SSH); web stack developed on Windows, deployed to Railway | Native Linux removes MSVC/WSL2 friction; dev environment = production sequencer environment; Railway keeps the web stack per global rules |

## 3. Architecture

```
   wallet + TS SDK        explorer (Next.js)       indexer worker
          \                      |                      /
           v                     v                     v
   +-----------------------------------------------------------+
   | ZUL node (Rust, single sequencer)                          |
   |   JSON-RPC server (Solana RPC subset + WebSocket)          |
   |   mempool -> SVM executor (solana-svm) -> block producer   |
   |   account store (redb) + state SMT (blake3)                |
   |   shielded pool builtin (Groth16 verify, Poseidon notes)   |
   |   bridge watcher (L1 deposits)  |  batcher (L1 posting)    |
   +-----------------------------------------------------------+
              |  state roots + DA batches          ^  deposits
              v                                    |
   +-----------------------------------------------------------+
   | Solana L1 (devnet first)                                   |
   |   settlement program  |  bridge vaults  |  DA log          |
   +-----------------------------------------------------------+
```

## 4. Components

### 4.1 `chain/` — Zul node (Rust workspace)

- `node` — binary: config, genesis, block production loop (tokio), shutdown
- `executor` — solana-svm integration: `TransactionBatchProcessor`,
  `TransactionProcessingCallback` over the account store; sysvar maintenance (Clock per
  slot, Rent, …); builtins + preloaded BPF programs (System, SPL Token, Token-2022, ATA,
  Memo, Compute Budget)
- `store` — accounts KV (redb), block store, blockhash queue (last 150 blockhashes),
  state SMT with incremental updates for touched accounts
- `rpc` — jsonrpsee HTTP + WS. Methods: sendTransaction, simulateTransaction,
  getAccountInfo, getMultipleAccounts, getBalance, getLatestBlockhash, getSlot,
  getBlock(s), getTransaction, getSignaturesForAddress, getSignatureStatuses,
  getTokenAccountsByOwner, requestAirdrop (built-in faucet); slot + signature WS
  subscriptions
- `privacy` — shielded pool builtin program: instruction parsing, Groth16 verification
  (ark-groth16 / ark-bn254), Poseidon hashing (light-poseidon), commitment Merkle tree
  (depth 26), nullifier set; a native-ZUL lamport vault and a per-mint wrapped-SPL vault
  ATA (shield/unshield move the wrapped token in/out of it)
- `bridge` — L1 deposit watcher (deposit events → L2 mint txs) and batcher (posts state
  roots + DA chunks via the settlement program)

Genesis: total ZUL supply minted as native lamports (9 decimals) to faucet, ops, and
bridge-reserve accounts. Fees use standard SVM fee handling into a collector account.

Block: `{ slot, parent_hash, blockhash, timestamp, state_root, txs[], sequencer_sig }`.
Blocks are produced every slot locally; only non-empty data is batched to L1.

### 4.2 `programs/` — Solana L1 programs (Anchor)

- `settlement` — sequencer authority; batch records `{batch_no, l2_block_range,
  state_root, withdrawals_root, da_hash}`; withdrawal claims verified against the posted
  shallow withdrawals-tree root (depth-32 SMT); challenge window (procedural in v1)
- Bridge instructions:
  - `deposit_sol(amount, l2_recipient)` — SOL into the vault PDA
  - `deposit_spl(amount, l2_recipient)` — any SPL token (Token or Token-2022) into a
    per-mint vault ATA via a checked token transfer; the `Deposited` event carries the
    mint + decimals so the L2 mints the matching wrapped token
  - `claim_withdrawal` / `claim_withdrawal_spl` — release SOL / the SPL token from the
    vault against an inclusion proof. The withdrawal commitment binds an `asset_id`
    (`[0u8;32]` for native, the L1 mint for SPL), so the claim can only release the asset
    the L2 actually burned
- `da_log` — noop-style program; instruction data carries zstd batch chunks

Asset mapping:

- ZUL — native on L2 (gas). SOL deposit → native ZUL lamports 1:1; that is also how a
  user funds gas (there is no faucet on mainnet)
- Any SPL token — deposit → locked in the per-mint L1 vault ATA; the L2 credits a
  deterministic *wrapped* SPL token (classic Token program, address derived from the L1
  mint, decimals copied). Withdraw burns the wrapped token on L2 and releases the
  original from the vault on claim. The wrapped mint's authority is a keyless reserved
  pubkey, so wrapped supply can only grow through a real deposit, never an SVM `MintTo`

### 4.3 `circuits/` — circom 2

- Note = `(asset_id, amount, owner_pk, blinding)`; commitment = Poseidon over all
  fields. `asset_id` is public at shield/unshield (the vault movement reveals it anyway)
  but stays private inside transfers — the circuit only enforces equality across all
  input/output notes, so private transfers do not leak which asset moved
- `transfer.circom` — 2-in/2-out join-split: Merkle inclusion (depth 26), ownership
  (sk → pk), nullifier correctness, asset-id equality, per-asset value conservation,
  optional public fee output
- `unshield.circom` — spend note(s) to a public recipient + amount
- Shield needs no proof (public inputs bind the commitment)
- Groth16 keys: Hermez powers-of-tau + local phase-2 ceremony (dev-grade, documented)
- Note discovery: notes encrypted to recipient (X25519 ECDH + XChaCha20-Poly1305) in the
  tx; wallets trial-decrypt to find incoming notes

### 4.4 `indexer/` — TypeScript worker

Subscribes to node WS slot notifications, fetches blocks via getBlock, writes to
Postgres via Prisma: blocks, transactions (logs / CU / decoded instructions), touched
accounts, token balances, shielded-pool public stats (pool TVL, commitment count,
nullifier count). Idempotent backfill from slot 0.

### 4.5 `explorer/` — Next.js 16

Pages: home (slot height, TPS, recent blocks/txs), block, transaction (decoded
instructions incl. the privacy program), account (balance, tokens, history), token,
shielded pool stats, faucet, bridge UI (deposit/withdraw against devnet), network
status. Reads Postgres via Prisma. Design via the frontend-design skill at build time.

### 4.6 `sdk/` — TypeScript package

- Thin wrapper over `@solana/web3.js` `Connection` pointed at the L2 RPC
- Bridge client: L1 deposit, L2 withdraw, L1 claim (proof fetched from the node)
- Shielded client: encrypted local note store, block scanning / trial decryption, proof
  generation via snarkjs WASM, shield / transfer / unshield builders
- Used by the explorer (faucet / bridge UI) and any future wallet UI

## 5. Trust model (honest disclosure)

v1 is a Stage-0 rollup with a single trusted sequencer:

- The sequencer can censor or reorder transactions
- A malicious sequencer could post an invalid state root; the challenge window is
  procedural (all data is public, anyone can recompute and raise an alarm) but invalid
  roots are not yet rejected on-chain
- Shielded-pool privacy holds against outside observers; the v1 fee path can link the
  fee payer (fixed by the v2 relayer)

These are roadmap items, stated openly rather than papered over.

## 6. Repository layout

```
ZUL/
  chain/        Rust workspace: node, executor, store, rpc, privacy, bridge
  programs/     Anchor workspace: settlement (+ bridge), da_log
  circuits/     circom sources, build + setup scripts (generated keys gitignored)
  sdk/          TypeScript SDK
  indexer/      TypeScript indexer worker
  explorer/     Next.js 16 app (Prisma schema lives here)
  docs/         PLAN.md (this file), protocol notes
  docker/       Dockerfiles, compose for the local stack
```

## 7. Roadmap

Each phase ends with an acceptance check; a phase is not done until the check passes.

### Phase 0 — Scaffold + SVM spike (1–2 days)
Monorepo scaffolding, WSL2/Docker toolchain, pin crate versions (verify current APIs via
Context7/docs first). Spike: execute a System transfer through solana-svm against an
in-memory account store in a Rust test.
- Check: `cargo test` proves a transfer executes and balances change

### Phase 1 — Chain core (1.5–2 weeks)
Genesis, persistent account store, blockhash queue, sysvars, mempool, block production
loop, state SMT, JSON-RPC subset + WS, built-in faucet, preloaded SPL programs.
- Check: a web3.js script airdrops ZUL, transfers between two keypairs, and creates an
  SPL mint; `solana balance -u <l2-rpc>` works; node restart preserves state

### Phase 2 — Explorer + indexer MVP (3–5 days)
Indexer → Postgres → explorer with home / block / tx / account pages, live updates.
- Check: Phase-1 script traffic is browsable in the explorer in near-real-time

### Phase 3 — Settlement + bridge on devnet (1–1.5 weeks)
Anchor programs, batcher, deposit watcher, withdraw + claim with SMT proofs, DA
chunking, explorer bridge / rollup-status pages.
- Check: a devnet SOL deposit appears on L2; a ZUL withdrawal is claimed on devnet after
  the window; every batch's state root is visible on devnet and in the explorer

### Phase 4 — Privacy (1.5–2 weeks)
Circuits + setup scripts, shielded pool builtin in the node, SDK note management + WASM
proving, explorer shielded stats, end-to-end flows.
- Check: shield → private transfer (A→B) → unshield round-trip passes for native ZUL
  and for a bridged SPL token; double-spend and invalid-proof transactions are rejected,
  with tests proving it

### Phase 5 — Deploy + polish (3–5 days)
Node as a systemd service (or Docker) on the OVHcloud server — firewalled RPC ports,
sequencer keys outside git; explorer + indexer + Postgres on Railway (PORT env, prisma
migrate at start time) pointed at the OVH RPC endpoint; faucet rate limits, docs, e2e
smoke script.
- Check: public RPC (OVH) + explorer (Railway) live; smoke script green from a clean
  machine

### Phase 6 — Roadmap (not v1)
Relayer for private fees, enforced fraud proofs, sequencer set, encrypted mempool.
*(The generalized token bridge, once a roadmap item, shipped — see the 2026-06-16
update. Mainnet bring-up is tracked in `DEPLOY-MAINNET.md`.)*

## 8. Risks

| Risk | Mitigation |
|------|------------|
| Agave crate API churn, version pinning pain | Pin exact versions in Phase 0; consult Context7 + Agave source before coding; LiteSVM fallback |
| Windows build friction for the Rust node | Build and run `chain/` on the OVHcloud Linux server over SSH; web stays on Windows |
| SMT over all accounts too slow later | Fine at prototype scale; update only touched accounts per block |
| Browser proving too slow (snarkjs) | 2-in/2-out Groth16 is seconds-scale; native prover sidecar as fallback |
| Wallets balk at a custom chain | `signTransaction` is chain-agnostic; SDK + explorer UI carry the UX |
| Devnet ledger pruning breaks DA reconstruction | Acceptable for v1; the node serves batch data; archive later |

## 9. Decision log

Confirmed 2026-06-10:

1. Execution engine: real SVM via `solana-svm`
2. Shielded pool covers native ZUL and any bridged asset (native SOL + arbitrary
   SPL tokens); the first cut shipped native-only and SPL landed 2026-06-16
3. Sequencer node is developed, built, and hosted on the user's OVHcloud server;
   explorer / indexer / Postgres deploy to Railway
4. Token `ZUL`, 9 decimals (working default — can rename any time before genesis)

To verify on the OVH server during Phase 0:

- OS: Ubuntu 22.04+ (or comparable) assumed
- RAM ≥ 8 GB for Rust builds of Agave-derived crates — add swap if below
- Toolchain: rustup, build-essential, pkg-config, libssl-dev, protobuf-compiler, clang
- SSH access set up for remote development; sequencer keypair generated on the server
  and never committed
