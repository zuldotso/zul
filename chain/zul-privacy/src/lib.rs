//! Multi-asset shielded pool for ZUL.
//!
//! UTXO notes carry an asset id (native ZUL or a bridged SPL mint such as
//! wSOL). Commitments form an append-only Poseidon Merkle tree; spends
//! reveal nullifiers; a Groth16 proof over BN254 enforces ownership, value
//! conservation, and asset-id consistency. Verification runs natively in
//! the node (no CU budget concerns) — this crate owns that logic and the
//! pool state machine; the executor registers the pool as an enshrined
//! builtin and the node applies the resulting public fund movements.

pub mod field;
pub mod instruction;
pub mod poseidon;
pub mod processor;
pub mod service;
pub mod state;
pub mod verifier;
pub mod verifying_keys;

pub use field::{fr_from_bytes, fr_to_bytes, FieldBytes};
pub use instruction::{Asset, InstructionError, PoolInstruction};
pub use processor::{
    NoteEvent, NullifierSet, PoolEffect, PoolError, PoolOutcome, PoolProcessor, ProofVerifier,
};
pub use service::{PoolBlockResult, PoolConfig, PoolService, PoolServiceError};
pub use state::{PoolState, PoolStateError, ROOT_HISTORY, TREE_DEPTH};
pub use verifier::VerifierError;
pub use verifying_keys::{Groth16Verifier, VerifyingKeys};

use solana_sdk::pubkey::Pubkey;

/// Enshrined program id of the shielded pool (off-curve, ZUL-reserved).
pub const POOL_PROGRAM_ID: Pubkey = Pubkey::new_from_array([
    0x05, 0x21, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

/// Name registered for the pool builtin.
pub const POOL_PROGRAM_NAME: &str = "zul_shielded_pool";

/// Account holding the serialized `PoolState` (commitment tree + roots).
pub const POOL_STATE_ADDRESS: Pubkey = Pubkey::new_from_array([
    0x05, 0x21, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
]);

/// Native (ZUL) vault holding shielded lamports.
pub const POOL_VAULT_ADDRESS: Pubkey = Pubkey::new_from_array([
    0x05, 0x21, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02,
]);
