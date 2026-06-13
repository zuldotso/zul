<div align="center">

<img src="banner.png" alt="Zul — Privacy Layer 2 on Solana" width="720" />

<p><strong>Privacy Layer 2 on Solana</strong> — real SVM execution, a native <code>ZUL</code> gas token, and a multi-asset zk shielded pool.</p>

<p>
  <a href="https://zul.so"><b>zul.so</b></a>
  &nbsp;·&nbsp;
  <a href="https://x.com/zuldotso"><b>@zuldotso</b></a>
</p>

</div>

---

A real Layer 2 chain that settles to Solana: own block production (single PoA
sequencer), real SVM execution via Agave's `solana-svm`, native gas token `ZUL`,
and a multi-asset zk shielded pool (ZUL + bridged SOL).

See [docs/PLAN.md](docs/PLAN.md) for the full implementation plan and trust model.

## Layout

```
chain/        Rust workspace: the ZUL node
  zul-primitives   blocks, genesis, hashing, config
  zul-store        accounts (redb), blocks, blockhash queue, state SMT
  zul-executor     solana-svm integration (transaction execution)
  zul-privacy      shielded pool builtin (Groth16 + Poseidon)
  zul-rpc          Solana-compatible JSON-RPC (HTTP + WS)
  zul-bridge       L1 deposit watcher + settlement batcher
  zul-node         the sequencer binary
programs/     Anchor workspace: settlement, bridge, da_log (Solana L1)
circuits/     circom 2 circuits + build/setup scripts
sdk/          TypeScript SDK (bridge + shielded client)
indexer/      TypeScript indexer worker (node -> Postgres)
explorer/     Next.js 16 block explorer
docs/         plan and protocol notes
```

## Quickstart (local)

```sh
# chain node (Rust)
cd chain
cargo test                  # unit + integration tests
cargo run -p zul-node -- --config ../config/node.example.toml

# web stack (TypeScript)
pnpm install
pnpm -r build
```

Keys, RPC endpoints, and SSH material are intentionally absent from this repo;
copy `config/node.example.toml` and `.env.example` files and fill them locally.
