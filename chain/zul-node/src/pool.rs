//! Node-side glue for the enshrined shielded pool.
//!
//! Pool transactions carry a single instruction to `POOL_PROGRAM_ID`. They
//! are pulled out of the SVM batch and handled by `PoolService` so proof
//! verification runs natively. The resulting account writes are layered on
//! top of the SVM's, and pool nullifiers are committed with the block.

use zul_primitives::constants::LAMPORTS_PER_SIGNATURE;
use zul_primitives::meta::ExecutedTransactionMeta;
use zul_privacy::{PoolInstruction, POOL_PROGRAM_ID};
use solana_sdk::{pubkey::Pubkey, transaction::VersionedTransaction};

/// A transaction recognized as a single pool instruction.
pub struct PoolTx {
    pub transaction: VersionedTransaction,
    pub fee_payer: Pubkey,
    pub instruction: PoolInstruction,
}

/// Split a drained batch into pool transactions and the rest. A pool tx has
/// exactly one instruction, targeting the pool program, with parseable data.
pub fn partition(
    transactions: Vec<VersionedTransaction>,
) -> (Vec<PoolTx>, Vec<VersionedTransaction>) {
    let mut pool = Vec::new();
    let mut regular = Vec::new();
    for tx in transactions {
        match try_parse_pool_tx(&tx) {
            Some((fee_payer, instruction)) => pool.push(PoolTx {
                transaction: tx,
                fee_payer,
                instruction,
            }),
            None => regular.push(tx),
        }
    }
    (pool, regular)
}

fn try_parse_pool_tx(tx: &VersionedTransaction) -> Option<(Pubkey, PoolInstruction)> {
    let message = &tx.message;
    let instructions = message.instructions();
    if instructions.len() != 1 {
        return None;
    }
    let keys = message.static_account_keys();
    let ix = &instructions[0];
    let program_id = keys.get(ix.program_id_index as usize)?;
    if *program_id != POOL_PROGRAM_ID {
        return None;
    }
    let fee_payer = *keys.first()?;
    let instruction = PoolInstruction::try_from_slice(&ix.data).ok()?;
    Some((fee_payer, instruction))
}

/// Synthetic metadata for an included pool transaction.
pub fn pool_meta(
    success: bool,
    status_err: Option<String>,
    fee: u64,
    pre: u64,
    post: u64,
    logs: Vec<String>,
) -> ExecutedTransactionMeta {
    ExecutedTransactionMeta {
        status: if success {
            Ok(())
        } else {
            // Pool rejections aren't SVM TransactionErrors; encode as a
            // custom instruction error so the shape stays Solana-compatible.
            Err(solana_sdk::transaction::TransactionError::InstructionError(
                0,
                solana_sdk::instruction::InstructionError::Custom(1),
            ))
        },
        fee,
        compute_units_consumed: 0,
        log_messages: status_err
            .map(|e| vec![format!("Program log: pool rejected: {e}")])
            .unwrap_or(logs),
        pre_balances: vec![pre],
        post_balances: vec![post],
    }
}

/// The flat fee a successful pool transaction pays.
pub const POOL_TX_FEE: u64 = LAMPORTS_PER_SIGNATURE;

/// Helper to read an account's lamports through a writes map.
pub fn lamports_in_writes(
    writes: &[(Pubkey, Option<zul_store::StoredAccount>)],
    key: &Pubkey,
) -> Option<u64> {
    writes
        .iter()
        .find(|(pk, _)| pk == key)
        .map(|(_, acc)| acc.as_ref().map(|a| a.lamports).unwrap_or(0))
}
