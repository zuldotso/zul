//! The L1 surface the bridge needs, kept narrow so a mock covers it.

use zul_primitives::hash::H256;
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

/// One batch record as stored by the settlement program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchRecord {
    pub batch_no: u64,
    pub first_slot: u64,
    pub last_slot: u64,
    pub state_root: H256,
    /// Root of the L2 shallow withdrawals tree at this batch — what
    /// `claim_withdrawal` verifies inclusion proofs against.
    pub withdrawals_root: H256,
    /// blake3 of the zstd-compressed batch payload (DA integrity anchor).
    pub da_hash: H256,
    pub chunk_count: u32,
}

/// A deposit observed on L1 (vault credit with an L2 recipient memo).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepositEvent {
    /// L1 transaction signature (dedup key), base58.
    pub l1_signature: String,
    /// Asset: None = SOL into the vault; Some(mint) = SPL deposit.
    pub mint: Option<Pubkey>,
    pub amount: u64,
    pub l2_recipient: Pubkey,
}

#[derive(Debug, thiserror::Error)]
pub enum L1Error {
    #[error("l1 rpc error: {0}")]
    Rpc(String),
    #[error("l1 transaction failed: {0}")]
    TransactionFailed(String),
}

/// Synchronous trait: the bridge runs on its own blocking task.
pub trait L1Client: Send + Sync {
    /// Post one DA chunk; returns the L1 signature. `chunk_count` is the
    /// total number of chunks in the batch (the DA-log program validates
    /// `chunk_index < chunk_count`).
    fn post_da_chunk(
        &self,
        batch_no: u64,
        chunk_index: u32,
        chunk_count: u32,
        chunk: &[u8],
    ) -> Result<String, L1Error>;

    /// Post the settlement batch record after its chunks.
    fn post_batch_record(&self, record: &BatchRecord) -> Result<String, L1Error>;

    /// The latest batch number recorded on L1 (resume point).
    fn latest_batch_no(&self) -> Result<Option<u64>, L1Error>;

    /// Deposits newer than the given L1 signature cursor, oldest first.
    fn fetch_deposits(&self, after: Option<&str>) -> Result<Vec<DepositEvent>, L1Error>;
}
