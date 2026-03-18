//! Core ZUL types shared across the node. Starts with hashing; blocks,
//! genesis, and config land here as the chain takes shape.
//!
//! This crate deliberately contains no storage or execution logic so that
//! every other crate can depend on it without dependency cycles.

pub mod hash;

pub use hash::{blake3_32, H256};

use thiserror::Error;

/// Errors produced while parsing or validating primitive structures.
#[derive(Debug, Error)]
pub enum PrimitivesError {
    #[error("invalid genesis: {0}")]
    InvalidGenesis(String),
    #[error("invalid config: {0}")]
    InvalidConfig(String),
}
