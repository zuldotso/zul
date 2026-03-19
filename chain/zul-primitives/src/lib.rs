//! Core ZUL types shared across the node: hashing, constants, transaction
//! metadata, and (soon) blocks, genesis, and config.
//!
//! This crate deliberately contains no storage or execution logic so that
//! every other crate can depend on it without dependency cycles.

pub mod constants;
pub mod hash;
pub mod meta;

pub use hash::{blake3_32, H256};
pub use meta::ExecutedTransactionMeta;

use thiserror::Error;

/// Errors produced while parsing or validating primitive structures.
#[derive(Debug, Error)]
pub enum PrimitivesError {
    #[error("invalid genesis: {0}")]
    InvalidGenesis(String),
    #[error("invalid config: {0}")]
    InvalidConfig(String),
}
