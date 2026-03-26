//! Builtin program registry for the ZUL chain.
//!
//! These are the native programs every ZUL block can invoke: the system
//! program, the BPF loaders (so users can deploy real Solana programs),
//! and the compute budget program. The privacy pool registers itself as an
//! additional builtin through `BuiltinSpec` from zul-privacy.

use solana_program_runtime::{
    invoke_context::BuiltinFunctionWithContext, loaded_programs::ProgramCacheEntry,
};
use solana_sdk::{
    account::{Account, AccountSharedData},
    pubkey::Pubkey,
};

/// One native program to register at startup.
pub struct BuiltinSpec {
    pub program_id: Pubkey,
    pub name: &'static str,
    pub entrypoint: BuiltinFunctionWithContext,
}

/// The account placed at a builtin's address (native-loader owned).
pub fn builtin_account(name: &str) -> AccountSharedData {
    AccountSharedData::from(Account {
        lamports: 1,
        data: name.as_bytes().to_vec(),
        owner: solana_sdk_ids::native_loader::id(),
        executable: true,
        rent_epoch: zul_store::RENT_EXEMPT_RENT_EPOCH,
    })
}

/// Default builtin set (everything except the privacy pool).
pub fn default_builtins() -> Vec<BuiltinSpec> {
    vec![
        BuiltinSpec {
            program_id: solana_sdk_ids::system_program::id(),
            name: "system_program",
            entrypoint: solana_system_program::system_processor::Entrypoint::vm,
        },
        BuiltinSpec {
            program_id: solana_sdk_ids::bpf_loader_upgradeable::id(),
            name: "solana_bpf_loader_upgradeable_program",
            entrypoint: solana_bpf_loader_program::Entrypoint::vm,
        },
        BuiltinSpec {
            program_id: solana_sdk_ids::bpf_loader::id(),
            name: "solana_bpf_loader_program",
            entrypoint: solana_bpf_loader_program::Entrypoint::vm,
        },
        BuiltinSpec {
            program_id: solana_sdk_ids::loader_v4::id(),
            name: "solana_loader_v4_program",
            entrypoint: solana_loader_v4_program::Entrypoint::vm,
        },
        BuiltinSpec {
            program_id: solana_sdk_ids::compute_budget::id(),
            name: "compute_budget_program",
            entrypoint: solana_compute_budget_program::Entrypoint::vm,
        },
    ]
}

impl BuiltinSpec {
    pub fn cache_entry(&self) -> ProgramCacheEntry {
        ProgramCacheEntry::new_builtin(0, self.name.len(), self.entrypoint)
    }
}
