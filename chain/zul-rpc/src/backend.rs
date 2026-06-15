//! The interface the RPC server needs from the node: read access to the
//! store plus the ability to submit and simulate transactions. The node
//! implements this; the RPC server is generic over it so it can be tested
//! with a mock.

use zul_primitives::hash::H256;
use zul_store::{SmtProof, Store, StoredAccount};
use solana_sdk::{pubkey::Pubkey, signature::Signature, transaction::VersionedTransaction};
use std::sync::Arc;
use tokio::sync::broadcast;

/// Broadcast after each committed block so WebSocket subscriptions
/// (`signatureSubscribe`, `slotSubscribe`, `accountSubscribe`) can push
/// updates instead of making clients poll.
#[derive(Debug)]
pub struct BlockNotification {
    pub slot: u64,
    pub parent_slot: u64,
    /// (signature, error) for every transaction in the block.
    pub signatures: Vec<(Signature, Option<String>)>,
    /// (pubkey, new account or None if deleted) for every account written.
    pub accounts: Vec<(Pubkey, Option<StoredAccount>)>,
}

/// Outcome of simulating a transaction without committing it.
#[derive(Debug, Clone)]
pub struct SimulationResult {
    pub err: Option<String>,
    pub logs: Vec<String>,
    pub units_consumed: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("transaction rejected: {0}")]
    Rejected(String),
    #[error("faucet disabled")]
    FaucetDisabled,
    #[error("faucet limit exceeded")]
    FaucetLimit,
    #[error("internal error: {0}")]
    Internal(String),
}

#[async_trait::async_trait]
pub trait RpcBackend: Send + Sync + 'static {
    /// Shared store for all read queries.
    fn store(&self) -> &Arc<Store>;

    /// Current chain tip slot.
    fn current_slot(&self) -> u64;

    /// Most recent (slot, blockhash) for `getLatestBlockhash`.
    fn latest_blockhash(&self) -> (u64, H256);

    /// Whether a given blockhash is still within the recency window.
    fn is_blockhash_valid(&self, blockhash: &H256) -> bool;

    /// Liveness: true when the sequencer is still producing blocks. Drives
    /// `getHealth` so a load balancer / monitor can detect a stalled producer
    /// (a constant "ok" would hide an outage).
    fn is_healthy(&self) -> bool;

    /// Subscribe to per-block notifications (WebSocket subscription methods).
    fn subscribe(&self) -> broadcast::Receiver<Arc<BlockNotification>>;

    /// SMT inclusion proof + state root for a withdrawal commitment, so a user
    /// can `claim_withdrawal` on L1. Errors if the commitment isn't committed.
    fn withdrawal_proof(
        &self,
        recipient: &[u8; 32],
        asset_id: &[u8; 32],
        amount: u64,
        nonce: u64,
    ) -> Result<(H256, SmtProof), String>;

    /// Submit a transaction to the mempool. Returns its signature once
    /// admitted (signature-verified, recent blockhash); execution happens
    /// when the next block is produced.
    async fn submit_transaction(
        &self,
        tx: VersionedTransaction,
    ) -> Result<Signature, BackendError>;

    /// Simulate against current committed state without mutating it.
    async fn simulate_transaction(
        &self,
        tx: VersionedTransaction,
    ) -> Result<SimulationResult, BackendError>;

    /// Fund `pubkey` with `lamports` from the faucet (a real signed System
    /// transfer). Returns the funding transaction signature.
    async fn request_airdrop(
        &self,
        pubkey: solana_sdk::pubkey::Pubkey,
        lamports: u64,
    ) -> Result<Signature, BackendError>;
}
