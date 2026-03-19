//! Execution results stored alongside each transaction, consumed by the
//! RPC layer (`getTransaction`) and the indexer/explorer.

use serde::{Deserialize, Serialize};
use solana_sdk::transaction::TransactionError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutedTransactionMeta {
    /// `Ok(())` for success, the SVM error otherwise. Failed transactions
    /// are still included in blocks and charged fees, like on L1.
    pub status: Result<(), TransactionError>,
    /// Fee charged in ZUL lamports.
    pub fee: u64,
    /// Compute units consumed during execution.
    pub compute_units_consumed: u64,
    /// Program log messages (already truncated by the executor's limit).
    pub log_messages: Vec<String>,
    /// Lamport balances of every account in the message, before execution.
    pub pre_balances: Vec<u64>,
    /// Lamport balances of every account in the message, after execution.
    pub post_balances: Vec<u64>,
}

impl ExecutedTransactionMeta {
    pub fn is_success(&self) -> bool {
        self.status.is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_bincode_roundtrip() {
        let meta = ExecutedTransactionMeta {
            status: Err(TransactionError::AccountNotFound),
            fee: 5_000,
            compute_units_consumed: 150,
            log_messages: vec!["Program log: hi".into()],
            pre_balances: vec![10, 20],
            post_balances: vec![5, 25],
        };
        let bytes = bincode::serialize(&meta).unwrap();
        let back: ExecutedTransactionMeta = bincode::deserialize(&bytes).unwrap();
        assert_eq!(back, meta);
        assert!(!back.is_success());
    }
}
