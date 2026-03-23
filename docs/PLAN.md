# Zul — Privacy Layer 2 on Solana

Implementation plan + build status. Work in progress.

## 1. Goal

A real Layer 2 chain that settles to Solana:

- Own block production: single PoA sequencer, fixed slot time (500 ms, configurable)
- Real SVM execution: runs standard Solana transactions and programs (System, SPL Token,
  Token-2022, user-deployed BPF programs)
- Native gas token `ZUL`: the chain's lamport unit. SOL does not exist natively on the L2;
  all fees are paid in ZUL
- Privacy: zk shielded pool for ZUL and bridged SOL (shield / private transfer /
  unshield), enshrined as a builtin program in the node
- Settlement and bridge: Anchor programs on Solana (devnet first), optimistic model
- Own explorer: Next.js + Postgres, fed by a dedicated indexer

### Non-goals (v1)

- Decentralized sequencer / multi-validator consensus
- On-chain-enforced fraud proofs or validity proofs of execution
- General private smart contracts. Privacy scope = shielded value transfer
- Mainnet deployment, audits, tokenomics beyond a minimal genesis

## 2. Key decisions

- Execution engine: embed Agave's `solana-svm` crate in our own Rust node
- Consensus: single PoA sequencer, slot-based production
- State commitment: sparse Merkle tree over (pubkey -> account hash), blake3
- Settlement: optimistic, Stage 0 (procedural challenge window)
- Privacy: UTXO shielded pool, Groth16 over BN254, circom circuits, native verify
- RPC: Solana JSON-RPC subset (HTTP + WebSocket)

## 7. Roadmap

- Phase 0 — Scaffold + SVM spike
- Phase 1 — Chain core
- Phase 2 — Explorer + indexer MVP
- Phase 3 — Settlement + bridge on devnet
- Phase 4 — Privacy
- Phase 5 — Deploy + polish
