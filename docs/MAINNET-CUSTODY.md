# Zul mainnet — key custody

Three keys hold real authority on mainnet. A single hot key for any of them is a
single point of catastrophic loss. This is the target custody model; adopt it
before a value-bearing launch.

## The keys

| Key | Power if stolen | Custody target |
|---|---|---|
| **Program upgrade authority** | Replace the settlement/DA program with arbitrary code → drain the vault. | **Squads multisig** (m-of-n), or burn it (immutable program). |
| **Bridge authority** | Sign batches + withdrawal claims. Cannot mint native ZUL, but a forged withdrawals root → drain the vault over the challenge window. | Squads multisig for the *upgrade/rotation*; the live signing key is necessarily hot on the sequencer (see below). |
| **Sequencer key** | Produce L2 blocks (it *is* the Stage-0 trust root). Cannot directly move L1 funds. | Hot on the box (unavoidable for a single sequencer); rotate-able via genesis only, so protect the box. |

## Recommended setup

1. **Upgrade authority → Squads.** After deploying (`docs/DEPLOY-MAINNET.md` §3),
   transfer the program upgrade authority to a Squads multisig:
   ```sh
   solana program set-upgrade-authority <program-id> \
     --new-upgrade-authority <squads-vault-pubkey>
   ```
   Now no single key can push a malicious upgrade. Keep the multisig threshold
   ≥ 2-of-3 with signers on separate hardware.

2. **Bridge authority hot key, minimal balance.** The batcher must sign
   continuously, so its key lives on the sequencer. Mitigations:
   - Fund it only to the operating buffer (`docs/MAINNET-COST.md`); top up from a
     cold reserve, never park large SOL on it.
   - File perms `chmod 600`, `keys/mainnet/` `chmod 700`, owned by `zul`.
   - It is **not** the native-ZUL mint authority — wrapped-SPL issuance is
     `BRIDGE_MINT_AUTHORITY`, a pubkey with **no private key** (issuance happens
     only inside the node's deposit synthesis, never via an SVM `MintTo`).
   - Plan a rotation runbook (redeploy `initialize` authority is not rotatable
     in-place today — track as a hardening item).

3. **Sequencer box hardening.** Since the sequencer key is the Stage-0 trust
   root, the security of the chain ≈ the security of the box: SSH keys only,
   `ufw` locked to RPC ports, no shared accounts, off-box encrypted backups
   (`scripts/backup-redb.sh` with `ZUL_BACKUP_REMOTE`).

## Out of scope for v1 (documented honestly)

Stage-0 trust model (`docs/PLAN.md` §5): state roots are not yet challenged
on-chain by re-execution; the challenge window is procedural. A compromised
sequencer or bridge authority can post a bad root and, after the window, claim
against it. Decentralizing the sequencer + enforcing fraud/validity proofs is the
post-launch roadmap, not a launch deliverable — do not market mainnet as
trustless until it lands.
