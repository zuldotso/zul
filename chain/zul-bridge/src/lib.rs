//! L1 bridge: settlement batching and deposit watching.
//!
//! All L1 effects go through the `L1Client` trait so the batching and
//! deposit logic is fully testable offline; `RpcL1Client` (the real
//! implementation over Solana RPC) is wired when devnet keys/endpoints are
//! configured. Wire formats here are the source of truth — the Anchor
//! settlement program implements the matching account layouts.

pub mod batch;
pub mod client;
pub mod deposit;
pub mod recover;
pub mod rpc;

pub use batch::{BatchPayload, Batcher, DA_CHUNK_BYTES};
pub use client::{BatchRecord, DepositEvent, L1Client, L1Error};
pub use deposit::DepositProcessor;
pub use recover::{verify, RecoveredBatch, RecoverError, RecoverySummary};
pub use rpc::RpcL1Client;
