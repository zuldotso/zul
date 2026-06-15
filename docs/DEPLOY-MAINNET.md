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
sudo ufw allow 8899/tcp && sudo ufw allow 8900/tcp
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

## 5. Bridge assets (SOL + arbitrary SPL)

- **SOL deposit** → native ZUL (gas), 1:1. `deposit_sol`.
- **SPL deposit** → a deterministic L2 *wrapped* token (SPL, classic Token
  program) at `wrapped_mint_address(l1_mint)`, credited to the recipient's ATA.
  `deposit_spl` (works for Token + Token-2022 mints). Any SPL mint is accepted.
- **Withdraw** (both): an L2 burn commits an asset-bound withdrawal leaf; once
  the batch settles, `claim_withdrawal` (SOL) / `claim_withdrawal_spl` (SPL)
  releases the asset from the per-mint vault. `getWithdrawalProof` takes an
  optional 4th param (the L1 mint) for SPL withdrawals.

> **Gas:** ZUL is the gas token, like SOL on Solana. A user who only bridged an
> SPL token holds no ZUL and cannot transact until they also bridge some SOL
> (→ native ZUL). This is intended — there is no faucet on mainnet.

## 6. Web stack + backups

Explorer/indexer on Railway, pointed at the mainnet RPC + a separate Postgres
(do not share the testnet DB). Nightly backups:

```
0 4 * * *  ZUL_NET=mainnet ZUL_BACKUP_REMOTE=<remote> /home/zul/ZUL/scripts/backup-redb.sh
```
