//! L1 → L2 deposit watcher. Polls the settlement program for `Deposited`
//! events and queues each new deposit for crediting on the L2.
//!
//! Exactly-once is enforced where the credit is applied (`Node::apply_deposits`):
//! crediting a deposit also writes a *receipt account* whose existence marks
//! the L1 signature as credited, committed atomically with the credit. So this
//! watcher can safely re-enqueue — and on a fresh store (disaster recovery) it
//! re-credits every L1 deposit automatically, since no receipts exist yet.

use std::str::FromStr;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use zul_bridge::{L1Client, RpcL1Client};
use zul_primitives::config::L1Section;
use zul_primitives::hash::{blake3_concat, H256};
use zul_rpc::RpcBackend;
use solana_sdk::{pubkey::Pubkey, signature::Keypair, transaction::VersionedTransaction};

use crate::node::Node;

/// Owner of deposit-receipt marker accounts (a reserved, non-executable id;
/// shares the pool's `0x06,0x21` reserved prefix).
pub const BRIDGE_RECEIPT_OWNER: Pubkey = Pubkey::new_from_array([
    0x06, 0x21, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0,
]);

const RECEIPT_DOMAIN: &[u8] = b"PL2:bridge:deposit-receipt:v1";

/// Deterministic L2 address whose existence marks an L1 deposit (identified by
/// its L1 transaction signature) as already credited.
pub fn deposit_receipt_address(l1_signature: &str) -> Pubkey {
    Pubkey::new_from_array(zul_primitives::blake3_32(RECEIPT_DOMAIN, l1_signature.as_bytes()))
}

// ----- withdraw (L2 -> L1) ------------------------------------------------

/// Reserved program id a withdrawal transaction targets (single instruction,
/// `recipient(32) || amount(8 LE) || nonce(8 LE)`).
pub const BRIDGE_WITHDRAW_PROGRAM_ID: Pubkey = Pubkey::new_from_array([
    0x06, 0x21, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0x01,
]);

// These MUST stay byte-identical to programs/zul-settlement/src/smt.rs.
const WITHDRAW_DOMAIN: &[u8] = b"PL2:bridge:withdraw:v1";
const WITHDRAW_ADDR_DOMAIN: &[u8] = b"PL2:bridge:withdraw-addr:v1";

/// L2 SMT address of the withdrawal commitment for `(recipient, nonce)`.
pub fn withdrawal_pubkey(recipient: &[u8; 32], nonce: u64) -> Pubkey {
    Pubkey::new_from_array(blake3_concat(
        WITHDRAW_ADDR_DOMAIN,
        &[recipient, &nonce.to_le_bytes()],
    ))
}

/// Account hash committed at the withdrawal leaf; the L1 `claim_withdrawal`
/// recomputes this and verifies its inclusion against a posted state root.
pub fn withdrawal_account_hash(recipient: &[u8; 32], amount: u64, nonce: u64) -> H256 {
    blake3_concat(
        WITHDRAW_DOMAIN,
        &[recipient, &amount.to_le_bytes(), &nonce.to_le_bytes()],
    )
}

/// A transaction recognized as a single bridge-withdraw instruction.
pub struct WithdrawTx {
    pub transaction: VersionedTransaction,
    pub fee_payer: Pubkey,
    pub recipient: [u8; 32],
    pub amount: u64,
    pub nonce: u64,
}

/// Split out bridge-withdraw transactions: exactly one instruction, targeting
/// `BRIDGE_WITHDRAW_PROGRAM_ID`, data = recipient(32) || amount(8) || nonce(8).
pub fn partition_withdraws(
    transactions: Vec<VersionedTransaction>,
) -> (Vec<WithdrawTx>, Vec<VersionedTransaction>) {
    let mut withdraws = Vec::new();
    let mut regular = Vec::new();
    for tx in transactions {
        match parse_withdraw(&tx) {
            Some((fee_payer, recipient, amount, nonce)) => withdraws.push(WithdrawTx {
                transaction: tx,
                fee_payer,
                recipient,
                amount,
                nonce,
            }),
            None => regular.push(tx),
        }
    }
    (withdraws, regular)
}

fn parse_withdraw(tx: &VersionedTransaction) -> Option<(Pubkey, [u8; 32], u64, u64)> {
    let message = &tx.message;
    let instructions = message.instructions();
    if instructions.len() != 1 {
        return None;
    }
    let keys = message.static_account_keys();
    let ix = &instructions[0];
    if keys.get(ix.program_id_index as usize)? != &BRIDGE_WITHDRAW_PROGRAM_ID {
        return None;
    }
    if ix.data.len() != 48 {
        return None;
    }
    let fee_payer = *keys.first()?;
    let recipient: [u8; 32] = ix.data[0..32].try_into().ok()?;
    let amount = u64::from_le_bytes(ix.data[32..40].try_into().ok()?);
    let nonce = u64::from_le_bytes(ix.data[40..48].try_into().ok()?);
    Some((fee_payer, recipient, amount, nonce))
}

/// Spawn the deposit watcher thread. Local config is validated up front (bad
/// config fails node boot); network errors are retried in the loop.
pub fn spawn(node: Arc<Node>, l1: L1Section) -> anyhow::Result<JoinHandle<()>> {
    let settlement = Pubkey::from_str(&l1.settlement_program_id)
        .map_err(|e| anyhow::anyhow!("invalid l1.settlement_program_id: {e}"))?;
    // da_log is unused for deposits; reuse settlement as a harmless placeholder.
    let da_log = Pubkey::from_str(&l1.da_log_program_id).unwrap_or(settlement);
    let rpc_url = l1.rpc_url.clone();

    let handle = std::thread::Builder::new()
        .name("zul-deposit-watcher".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            // Read-only: the watcher never signs, so a throwaway authority is fine.
            let client = match RpcL1Client::new(rpc_url, Keypair::new(), settlement, da_log) {
                Ok(client) => client,
                Err(e) => {
                    tracing::error!(error = %e, "deposit watcher: client init failed");
                    return;
                }
            };
            tracing::info!("L1 deposit watcher running");
            loop {
                match client.fetch_deposits(None) {
                    Ok(deposits) => {
                        // Enqueue only deposits not yet credited (no receipt).
                        let fresh: Vec<_> = deposits
                            .into_iter()
                            .filter(|d| {
                                let receipt = deposit_receipt_address(&d.l1_signature);
                                !matches!(node.store().get_account(&receipt), Ok(Some(_)))
                            })
                            .collect();
                        if !fresh.is_empty() {
                            tracing::info!(count = fresh.len(), "queuing new L1 deposits for L2 credit");
                            node.enqueue_deposits(fresh);
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "deposit watcher: fetch_deposits failed"),
                }
                std::thread::sleep(Duration::from_secs(10));
            }
        })?;
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::{
        hash::Hash, instruction::Instruction, message::Message, signature::Keypair,
        signer::Signer, transaction::Transaction,
    };

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn withdrawal_hashes_cross_pinned_with_settlement() {
        // Pinned in programs/zul-settlement/src/smt.rs::withdrawal_hashes_match_l2.
        let recipient = [7u8; 32];
        assert_eq!(
            hex(&withdrawal_account_hash(&recipient, 1000, 5)),
            "6925634e356b691780b4a731d5faad074613a8502daf904911ee18d143c120c7"
        );
        assert_eq!(
            hex(withdrawal_pubkey(&recipient, 5).as_ref()),
            "fc7e27778b8a6c585cfdbfb66ea01e059a935d09ee5646d46b9658573c9cf920"
        );
    }

    #[test]
    fn parses_a_withdraw_transaction() {
        let payer = Keypair::new();
        let recipient = [9u8; 32];
        let (amount, nonce) = (5_000_000u64, 42u64);
        let mut data = Vec::new();
        data.extend_from_slice(&recipient);
        data.extend_from_slice(&amount.to_le_bytes());
        data.extend_from_slice(&nonce.to_le_bytes());
        let ix = Instruction {
            program_id: BRIDGE_WITHDRAW_PROGRAM_ID,
            accounts: vec![solana_sdk::instruction::AccountMeta::new(payer.pubkey(), true)],
            data,
        };
        let tx = VersionedTransaction::from(Transaction::new(
            &[&payer],
            Message::new(&[ix], Some(&payer.pubkey())),
            Hash::new_from_array([1u8; 32]),
        ));
        let (mut withdraws, regular) = partition_withdraws(vec![tx]);
        assert!(regular.is_empty());
        assert_eq!(withdraws.len(), 1);
        let w = withdraws.pop().unwrap();
        assert_eq!(w.fee_payer, payer.pubkey());
        assert_eq!(w.recipient, recipient);
        assert_eq!(w.amount, amount);
        assert_eq!(w.nonce, nonce);
    }
}
