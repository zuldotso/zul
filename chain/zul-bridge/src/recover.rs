//! Disaster recovery from L1: re-fetch the posted settlement batches + their
//! DA chunks, verify integrity against the on-chain commitments, and recover
//! every L2 block/transaction.
//!
//! Trust model (single trusted operator): the on-chain `da_hash` (blake3 of
//! the compressed payload, checked during fetch) is the cryptographic tie —
//! verifying it proves each recovered payload is **exactly** what the
//! sequencer published and committed on L1. This module additionally checks
//! the structural invariants (sequential batches, non-overlapping ranges,
//! blocks within range) and surfaces the settled state roots.
//!
//! Note: the record's `state_root` is the state at the batch's *tip* slot,
//! which includes per-block sysvar (Clock) updates from empty heartbeat
//! blocks the DA omits — so it is not the last *payload* block's root and
//! cannot be reconstructed from DA alone. A bit-identical store rebuild would
//! need the empty-block hashes too (a DA design change, or a recency-skipping
//! recovery executor).

use crate::client::BatchRecord;
use crate::BatchPayload;

/// One batch as recovered from L1: the settlement record plus its decoded,
/// DA-hash-verified payload.
pub struct RecoveredBatch {
    pub record: BatchRecord,
    pub payload: BatchPayload,
}

#[derive(Debug)]
pub struct RecoverySummary {
    pub batches: u64,
    pub blocks: u64,
    pub transactions: u64,
    pub first_slot: u64,
    pub last_slot: u64,
    pub final_state_root: [u8; 32],
}

#[derive(Debug, thiserror::Error)]
pub enum RecoverError {
    #[error("no batches found on L1")]
    Empty,
    #[error("batch numbering gap: expected {expected}, got {got}")]
    SequenceGap { expected: u64, got: u64 },
    #[error("batch {batch} first_slot {first} does not follow previous last_slot {prev}")]
    NonContiguous { batch: u64, first: u64, prev: u64 },
    #[error("batch {batch}: block at slot {slot} is outside the batch's slot range")]
    OutOfRange { batch: u64, slot: u64 },
    #[error("batch {batch}: empty payload (a posted batch must carry blocks)")]
    EmptyPayload { batch: u64 },
}

/// Verify recovered batches form an intact, contiguous, settlement-matching
/// chain. DA-hash integrity is checked during fetch; this checks the
/// structural + commitment invariants. Returns a summary on success.
pub fn verify(batches: &[RecoveredBatch]) -> Result<RecoverySummary, RecoverError> {
    if batches.is_empty() {
        return Err(RecoverError::Empty);
    }
    let mut blocks = 0u64;
    let mut transactions = 0u64;
    let mut prev_last_slot: Option<u64> = None;
    let mut first_slot = 0u64;
    let mut last_slot = 0u64;
    let mut final_state_root = [0u8; 32];

    for (i, rb) in batches.iter().enumerate() {
        let r = &rb.record;
        // Batches are posted sequentially from 0.
        let expected = i as u64;
        if r.batch_no != expected {
            return Err(RecoverError::SequenceGap {
                expected,
                got: r.batch_no,
            });
        }
        // Batch ranges are strictly increasing and non-overlapping. Gaps are
        // expected: the batcher advances its cursor past all-empty windows
        // (heartbeat blocks) without posting, so consecutive batches need only
        // start after the previous one ends, not immediately after.
        if let Some(prev) = prev_last_slot {
            if r.first_slot <= prev {
                return Err(RecoverError::NonContiguous {
                    batch: r.batch_no,
                    first: r.first_slot,
                    prev,
                });
            }
        } else {
            first_slot = r.first_slot;
        }
        // A posted batch must carry blocks (the batcher only posts non-empty).
        if rb.payload.blocks.is_empty() {
            return Err(RecoverError::EmptyPayload { batch: r.batch_no });
        }
        // Every block lands within the batch's declared slot range.
        for block in &rb.payload.blocks {
            let slot = block.header.slot;
            if slot < r.first_slot || slot > r.last_slot {
                return Err(RecoverError::OutOfRange {
                    batch: r.batch_no,
                    slot,
                });
            }
        }

        blocks += rb.payload.blocks.len() as u64;
        transactions += rb
            .payload
            .blocks
            .iter()
            .map(|b| b.transactions.len() as u64)
            .sum::<u64>();
        prev_last_slot = Some(r.last_slot);
        last_slot = r.last_slot;
        final_state_root = r.state_root;
    }

    Ok(RecoverySummary {
        batches: batches.len() as u64,
        blocks,
        transactions,
        first_slot,
        last_slot,
        final_state_root,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zul_primitives::block::{Block, BlockHeader};
    use solana_sdk::{signature::Keypair, transaction::VersionedTransaction};

    fn block(slot: u64, txs: usize, state_root: [u8; 32]) -> Block {
        let transactions: Vec<VersionedTransaction> =
            (0..txs).map(|_| VersionedTransaction::default()).collect();
        let header = BlockHeader {
            slot,
            parent_hash: [0u8; 32],
            state_root,
            transactions_root: Block::transactions_root(&transactions),
            timestamp_ms: slot,
            transaction_count: txs as u32,
        };
        Block::seal(header, transactions, &Keypair::new())
    }

    fn batch(no: u64, first: u64, last: u64, blocks: Vec<Block>) -> RecoveredBatch {
        let state_root = blocks.last().unwrap().header.state_root;
        RecoveredBatch {
            record: BatchRecord {
                batch_no: no,
                first_slot: first,
                last_slot: last,
                state_root,
                withdrawals_root: [0u8; 32],
                da_hash: [0u8; 32],
                chunk_count: 1,
            },
            payload: BatchPayload {
                batch_no: no,
                first_slot: first,
                last_slot: last,
                blocks,
            },
        }
    }

    #[test]
    fn verifies_a_contiguous_chain() {
        let batches = vec![
            batch(0, 1, 20, vec![block(1, 2, [1u8; 32]), block(3, 1, [2u8; 32])]),
            batch(1, 21, 40, vec![block(25, 3, [3u8; 32])]),
        ];
        let s = verify(&batches).unwrap();
        assert_eq!(s.batches, 2);
        assert_eq!(s.blocks, 3);
        assert_eq!(s.transactions, 6);
        assert_eq!(s.final_state_root, [3u8; 32]);
    }

    #[test]
    fn allows_slot_gaps_but_rejects_overlap() {
        // Gaps between batches are normal (empty windows the batcher skipped).
        let with_gap = vec![
            batch(0, 1, 20, vec![block(1, 1, [1u8; 32])]),
            batch(1, 61, 80, vec![block(65, 1, [2u8; 32])]), // gap 21..60 is fine
        ];
        assert!(verify(&with_gap).is_ok());

        // Overlap (next batch starts at/before the previous end) is not.
        let overlap = vec![
            batch(0, 1, 20, vec![block(1, 1, [1u8; 32])]),
            batch(1, 15, 40, vec![block(25, 1, [2u8; 32])]),
        ];
        assert!(matches!(
            verify(&overlap),
            Err(RecoverError::NonContiguous { .. })
        ));
    }

    #[test]
    fn rejects_block_outside_batch_range() {
        // A block whose slot is outside [first_slot, last_slot].
        let b = batch(0, 1, 20, vec![block(99, 1, [1u8; 32])]);
        assert!(matches!(
            verify(&[b]),
            Err(RecoverError::OutOfRange { slot: 99, .. })
        ));
    }

    #[test]
    fn rejects_batch_number_gap() {
        let batches = vec![batch(1, 1, 20, vec![block(1, 1, [1u8; 32])])];
        assert!(matches!(
            verify(&batches),
            Err(RecoverError::SequenceGap { expected: 0, got: 1 })
        ));
    }
}
