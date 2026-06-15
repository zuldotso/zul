# Zul mainnet deployment

Bring up a **second OVH server** as the mainnet sequencer, settling to Solana
**mainnet-beta**. This mirrors the testnet runbook (`docs/DEPLOY.md`) but every
value here is real money — read `docs/MAINNET-COST.md` and
`docs/MAINNET-CUSTODY.md` first. The testnet keeps running, untouched, on its own
box; the two share only this source tree (network-scoped config under
`config/<net>/`).

> **Launch blockers** (do not skip for a value-bearing chain):
> 1. **Trusted setup.** `circuits/scripts/setup.mjs` uses fixed entropy → the
>    proving keys are forgeable. Run the real ceremony first
>    (`docs/MAINNET-CEREMONY.md`) and ship the resulting `*.vk.bin`.
> 2. **Key custody.** The sequencer / bridge-authority keys move real funds.
>    Put them behind the multisig in `docs/MAINNET-CUSTODY.md` before launch.

## 0. Prereqs (new server)

Same toolchain as testnet (`docs/DEPLOY.md` §Tier-1): a non-root `zul` user, Rust
1.93.1, build deps, an 8G+ swapfile, `ufw` allowing SSH. Use a **paid mainnet-beta
RPC** (public `api.mainnet-beta.solana.com` is heavily rate-limited and will stall
the batcher); keep the API key out of git (pass via `ZUL_L1_RPC_URL`).

## 1. Build + init the mainnet chain

```sh
git clone <repo-url> ZUL && cd ZUL
cargo build --release -p zul-node --manifest-path chain/Cargo.toml

# Generate mainnet keys + genesis + node.toml (faucet forced OFF, mainnet-beta
# RPC default, batch_interval 600). Pass the paid RPC so it lands in node.toml
# (server-only; node.toml is gitignored):
ZUL_NET=mainnet ZUL_L1_RPC_URL='https://...mainnet...?api-key=XXXX' ./scripts/init-live.sh
```

This writes `keys/mainnet/{sequencer,fee_collector,ops}.json`,
`config/mainnet/genesis.toml` (chain identity — written once), and
`config/mainnet/node.toml`. No faucet key (mainnet has no faucet).

## 2. Run as a service

```sh
sudo cp deploy/zul-node@.service /etc/systemd/system/
# edit User / WorkingDirectory in the unit to match the checkout
sudo systemctl daemon-reload && sudo systemctl enable --now zul-node@mainnet
journalctl -u zul-node@mainnet -f   # expect: network=mainnet … loading ZUL node
# Publish the RPC as https://rpc-mainnet.zul.so (8899/8900 stay on localhost).
# Point DNS A record rpc-mainnet.zul.so -> this box first, then:
sudo ufw allow 80,443/tcp                       # Caddy; do NOT open 8899/8900
sudo cp deploy/Caddyfile.mainnet /etc/caddy/Caddyfile && sudo systemctl reload caddy
```

The boot log prints `network=mainnet`; the config validator refuses to start if
the faucet is enabled on mainnet.

## 3. Deploy the settlement + DA programs to mainnet-beta

Needs the **SOL-funded deploy key** (`keys/mainnet/deployer.json`, gitignored).
Costs real SOL (see `docs/MAINNET-COST.md` for current `.so` sizes / rent).

```sh
solana config set --url <paid-mainnet-rpc> --keypair keys/mainnet/deployer.json
solana balance   # must cover program rent + headroom

cd programs && cargo-build-sbf
# Deploy with --max-len = EXACT .so size to halve programdata rent:
solana program deploy --max-len $(stat -c%s target/deploy/zul_settlement.so) \
  --program-id target/deploy/zul_settlement-keypair.json target/deploy/zul_settlement.so
solana program deploy --max-len $(stat -c%s target/deploy/zul_da_log.so) \
  --program-id target/deploy/zul_da_log-keypair.json target/deploy/zul_da_log.so
```

> Mainnet program ids must be **fresh** — generate new program keypairs for
> mainnet (do not reuse the devnet `2hwZW…` / `3E3dF…`). Record the new ids; the
> upgrade authority is the deployer key until you transfer it to the multisig.

## 4. Fund the bridge authority + enable settlement

```sh
# The bridge authority signs every batch post + withdrawal claim; keep it funded
# (the batcher pauses below ~0.02 SOL).
solana transfer <bridge-authority-pubkey> <amount> --allow-unfunded-recipient
```

Edit `config/mainnet/node.toml` `[l1]`:

```toml
enabled = true
rpc_url = "https://...mainnet...?api-key=XXXX"   # server-only
settlement_program_id = "<new mainnet settlement id>"
da_log_program_id     = "<new mainnet da_log id>"
bridge_authority_key_path = "/home/zul/ZUL/keys/mainnet/deployer.json"
batch_interval_slots = 600
```

`sudo systemctl restart zul-node@mainnet`. The batcher auto-runs `initialize`
on first boot, then posts state roots + DA; the deposit watcher credits L2 from
mainnet-beta deposits.

## 5. Gas model + bridge assets

**Gas = native ZUL = bridged ZUL token, 1:1.** ZUL is a fixed-supply token on
Solana mainnet (9 decimals, matching the L2 native unit; ~1B × 1e9 ≈ 1e18 lamports
< u64 max). There is **no native pre-mine and no faucet** — every unit of L2 gas
exists only because the matching ZUL is locked in the bridge vault, so native
supply is always fully backed and never exceeds the L1 supply.

The live gas mint (`config/mainnet` `gas_mint`):

```
okTCeDRb7NeExmvf3RZxEs29i9nfw8Cd51oFVB4FZUL
```

Verified on mainnet-beta: **Token-2022**, **9 decimals**, supply ~999,994,095 ZUL,
**mint authority revoked** (fixed supply), extensions **metadataPointer +
tokenMetadata only** (name "Zul", symbol "ZUL") — no transfer-fee/hook/permanent-
delegate/etc., so it passes `require_safe_mint` and bridges cleanly. (The mint is
Token-2022, which is exactly why `deposit_spl` uses the token interface + the
extension safety check.)

Asset routing:

- **ZUL-SPL deposit** → native ZUL (gas), 1:1. This is the only path that mints
  native lamports. Withdraw native ZUL → releases ZUL-SPL from the vault.
- **SOL deposit** → a *wrapped* SOL token (asset, not gas). Withdraw → real SOL.
- **Any other SPL deposit** → a deterministic L2 *wrapped* token at
  `wrapped_mint_address(l1_mint)`. Withdraw → the original SPL. **Token and
  Token-2022 both work** (the program uses the token interface).
  - *Token-2022 safety:* `deposit_spl` credits the **actually-received** amount
    (so a transfer-fee mint can't leave the vault under-collateralized), and
    **rejects** mints with vault-unsafe extensions — transfer-hook, permanent
    delegate, non-transferable, default-frozen, confidential transfer
    (`SettlementError::UnsupportedMintExtension`). Plain Token-2022 and
    transfer-fee mints are accepted; the L2 wrapped token is always a plain
    classic-SPL token.
- Bridged assets (wrapped SOL / SPL) can be shielded privately in the pool.
- Withdrawals burn on L2 and commit an asset-bound leaf; once settled,
  `claim_withdrawal` (native SOL) / `claim_withdrawal_spl` (any token incl. the
  ZUL-SPL gas exit) releases from the per-mint vault.

> **Consequence:** a user with no ZUL holds no gas and cannot transact — exactly
> like needing ETH on an Ethereum L2. They acquire ZUL on Solana (buy the SPL)
> and bridge it in. The chain is **unusable until** the ZUL-SPL mint exists, the
> bridge `gas_mint` is set, and some is bridged in (see §0 launch order).

> **Fix the L1 supply:** after minting exactly 1B ZUL-SPL, **revoke its mint
> authority** — otherwise the supply isn't actually fixed and the 1:1 cap is a
> promise, not a guarantee.

## 6. Web stack + backups

Explorer/indexer on Railway, pointed at the mainnet RPC + a separate Postgres
(do not share the testnet DB). Nightly backups:

```
0 4 * * *  ZUL_NET=mainnet ZUL_BACKUP_REMOTE=<remote> /home/zul/ZUL/scripts/backup-redb.sh
```
