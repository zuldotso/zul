//! Core ZUL types shared across the node: hashing, blocks, genesis,
//! transaction metadata, and node configuration.
//!
//! This crate deliberately contains no storage or execution logic so that
//! every other crate can depend on it without dependency cycles.

pub mod block;
pub mod config;
pub mod constants;
pub mod genesis;
pub mod hash;
pub mod merkle;
pub mod meta;

pub use block::{Block, BlockHeader};
pub use config::NodeConfig;
pub use genesis::{Allocation, GenesisConfig};
pub use hash::{blake3_32, H256};
pub use meta::ExecutedTransactionMeta;

use thiserror::Error;

/// Errors produced while parsing or validating primitive structures.
#[derive(Debug, Error)]
pub enum PrimitivesError {
    #[error("invalid TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("io error reading {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("invalid genesis: {0}")]
    InvalidGenesis(String),
    #[error("invalid config: {0}")]
    InvalidConfig(String),
}
