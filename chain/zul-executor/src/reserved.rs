//! Reserved account keys: addresses that transactions may reference but
//! never write-lock (builtins, sysvars, precompiles). Mirrors the L1 set
//! for the programs ZUL ships.

use solana_sdk::pubkey::Pubkey;
use std::collections::HashSet;

pub fn reserved_account_keys() -> HashSet<Pubkey> {
    use solana_sdk_ids::*;
    let mut keys = HashSet::new();
    // Builtin programs and loaders.
    keys.insert(system_program::id());
    keys.insert(bpf_loader::id());
    keys.insert(bpf_loader_deprecated::id());
    keys.insert(bpf_loader_upgradeable::id());
    keys.insert(loader_v4::id());
    keys.insert(compute_budget::id());
    keys.insert(native_loader::id());
    keys.insert(stake::id());
    keys.insert(vote::id());
    keys.insert(config::id());
    keys.insert(address_lookup_table::id());
    // Precompiles.
    keys.insert(ed25519_program::id());
    keys.insert(secp256k1_program::id());
    keys.insert(secp256r1_program::id());
    // Sysvars.
    keys.insert(sysvar::id());
    keys.insert(sysvar::clock::id());
    keys.insert(sysvar::epoch_schedule::id());
    keys.insert(sysvar::epoch_rewards::id());
    keys.insert(sysvar::instructions::id());
    keys.insert(sysvar::last_restart_slot::id());
    keys.insert(sysvar::recent_blockhashes::id());
    keys.insert(sysvar::rent::id());
    keys.insert(sysvar::slot_hashes::id());
    keys.insert(sysvar::slot_history::id());
    keys.insert(sysvar::stake_history::id());
    keys
}
