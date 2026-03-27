//! Solana-compatible JSON-RPC server for the ZUL L2.
//!
//! The server is generic over `RpcBackend`, which the node implements to
//! provide store access and the transaction submission/simulation paths.

pub mod backend;
pub mod encode;

pub use backend::{BackendError, BlockNotification, RpcBackend, SimulationResult};
