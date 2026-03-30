//! Solana-compatible JSON-RPC server for the ZUL L2.
//!
//! Implements the method subset web3.js / wallet-adapter / the solana CLI
//! need: getAccountInfo, getBalance, getMultipleAccounts, getLatestBlockhash,
//! isBlockhashValid, getSlot, getBlock, getTransaction, getSignatureStatuses,
//! getSignaturesForAddress, getTokenAccountsByOwner, sendTransaction,
//! simulateTransaction, requestAirdrop, plus getHealth/getVersion/getGenesisHash.
//!
//! The server is generic over `RpcBackend`, which the node implements to
//! provide store access and the transaction submission/simulation paths.

pub mod backend;
pub mod encode;
pub mod server;

pub use backend::{BackendError, BlockNotification, RpcBackend, SimulationResult};
pub use server::{build_module, serve, ServeHandles};
