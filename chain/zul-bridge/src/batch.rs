//! Settlement batching: collect non-empty L2 blocks since the last batch,
//! compress their transactions, chunk for DA, and post the batch record.

use zul_primitives::block::Block;
use zul_primitives::hash::{blake3_32, H256};
use zul_store::Store;
use serde::{Deserialize, Serialize};

use crate::client::{BatchRecord, L1Client, L1Error};

/// Instruction data budget per DA transaction (Solana tx limit is 1232
/// bytes total; leave room for accounts/signature/discriminator).
pub const DA_CHUNK_BYTES: usize = 800;

const DA_DOMAIN: &[u8] = b"PL2:da:batch:v1";

/// What gets compressed and published per batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchPayload {
    pub batch_no: u64,
    pub first_slot: u64,
    pub last_slot: u64,
    /// Non-empty blocks in slot order (headers + full transactions).
    pub blocks: Vec<Block>,
}

impl BatchPayload {
    pub fn compress(&self) -> Vec<u8> {
        let raw = bincode::serialize(self).expect("payload serialization is infallible");
        zstd::encode_all(raw.as_slice(), 3).expect("zstd encode")
    }

    pub fn decompress(bytes: &[u8]) -> Result<Self, BatchError> {
        let raw = zstd::decode_all(bytes).map_err(|_| BatchError::Corrupt)?;
        bincode::deserialize(&raw).map_err(|_| BatchError::Corrupt)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BatchError {
    #[error("store error: {0}")]
    Store(#[from] zul_store::StoreError),
    #[error("l1 error: {0}")]
    L1(#[from] L1Error),
    #[error("corrupt batch payload")]
    Corrupt,
}

pub fn da_hash(compressed: &[u8]) -> H256 {
    blake3_32(DA_DOMAIN, compressed)
}

pub fn chunk_payload(compressed: &[u8]) -> Vec<&[u8]> {
    compressed.chunks(DA_CHUNK_BYTES).collect()
}

/// Drives batch creation and posting. Tracks the next batch boundary by
/// slot cursor; resumes from the L1 record on restart.
pub struct Batcher<C: L1Client> {
    client: C,
    /// First slot not yet batched.
    cursor_slot: u64,
    next_batch_no: u64,
}

impl<C: L1Client> Batcher<C> {
    /// Resume from L1: the next batch number and a caller-persisted slot
    /// cursor (the node stores it alongside chain meta).
    pub fn new(client: C, cursor_slot: u64) -> Result<Self, BatchError> {
        let next_batch_no = client.latest_batch_no()?.map(|n| n + 1).unwrap_or(0);
        Ok(Self {
            client,
            cursor_slot,
            next_batch_no,
        })
    }

    pub fn cursor_slot(&self) -> u64 {
        self.cursor_slot
    }

    /// Borrow the L1 client (e.g. to read the authority balance for
    /// backpressure).
    pub fn client(&self) -> &C {
        &self.client
    }

    /// Batch all non-empty blocks in `[cursor_slot, tip_slot]`. Returns the
    /// posted record, or None if there was nothing to post (cursor still
    /// advances past empty blocks).
    pub fn run_once(
        &mut self,
        store: &Store,
        tip_slot: u64,
    ) -> Result<Option<BatchRecord>, BatchError> {
        if tip_slot < self.cursor_slot {
            return Ok(None);
        }
        let slots = store.block_slots_in_range(self.cursor_slot, tip_slot, 10_000)?;
        let mut blocks = Vec::new();
        let mut last_state_root = None;
        for slot in slots {
            if let Some(block) = store.get_block(slot)? {
                last_state_root = Some(block.header.state_root);
                if block.header.transaction_count > 0 {
                    blocks.push(block);
                }
            }
        }
        // Nothing executed since the last batch: just advance the cursor.
        let Some(state_root) = last_state_root else {
            self.cursor_slot = tip_slot + 1;
            return Ok(None);
        };
        if blocks.is_empty() {
            self.cursor_slot = tip_slot + 1;
            return Ok(None);
        }

        let payload = BatchPayload {
            batch_no: self.next_batch_no,
            first_slot: self.cursor_slot,
            last_slot: tip_slot,
            blocks,
        };
        let compressed = payload.compress();
        let chunks = chunk_payload(&compressed);
        let chunk_count = chunks.len() as u32;
        for (index, chunk) in chunks.iter().enumerate() {
            self.client
                .post_da_chunk(payload.batch_no, index as u32, chunk_count, chunk)?;
        }
        let record = BatchRecord {
            batch_no: payload.batch_no,
            first_slot: payload.first_slot,
            last_slot: payload.last_slot,
            state_root,
            withdrawals_root: store.withdrawals_root()?,
            da_hash: da_hash(&compressed),
            chunk_count: chunks.len() as u32,
        };
        self.client.post_batch_record(&record)?;

        self.cursor_slot = tip_slot + 1;
        self.next_batch_no += 1;
        Ok(Some(record))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::DepositEvent;
    use zul_primitives::block::BlockHeader;
    use zul_primitives::genesis::{Allocation, GenesisConfig};
    use solana_sdk::{
        hash::Hash, pubkey::Pubkey, signature::Keypair, signer::Signer,
        transaction::{Transaction, VersionedTransaction},
    };
    use solana_system_interface::instruction as system_instruction;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MockL1 {
        pub chunks: Mutex<Vec<(u64, u32, Vec<u8>)>>,
        pub records: Mutex<Vec<BatchRecord>>,
    }

    impl L1Client for MockL1 {
        fn post_da_chunk(&self, b: u64, i: u32, _n: u32, c: &[u8]) -> Result<String, L1Error> {
            self.chunks.lock().unwrap().push((b, i, c.to_vec()));
            Ok(format!("chunk-{b}-{i}"))
        }
        fn post_batch_record(&self, r: &BatchRecord) -> Result<String, L1Error> {
            self.records.lock().unwrap().push(r.clone());
            Ok(format!("record-{}", r.batch_no))
        }
        fn latest_batch_no(&self) -> Result<Option<u64>, L1Error> {
            Ok(self.records.lock().unwrap().last().map(|r| r.batch_no))
        }
        fn fetch_deposits(&self, _after: Option<&str>) -> Result<Vec<DepositEvent>, L1Error> {
            Ok(vec![])
        }
    }

    fn store_with_blocks(tx_counts: &[u32]) -> (tempfile::TempDir, Store, u64) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("zul.redb")).unwrap();
        let payer = Keypair::new();
        let genesis = GenesisConfig {
            chain_name: "zul-batch-test".into(),
            creation_time_ms: 1,
            sequencer: Pubkey::new_unique(),
            fee_collector: Pubkey::new_unique(),
            allocations: vec![Allocation {
                pubkey: payer.pubkey(),
                lamports: 1_000_000_000,
            }],
        };
        store.initialize_genesis(&genesis, &[]).unwrap();
        let sequencer = Keypair::new();
        let mut queue = zul_store::BlockhashQueue::new();
        queue.register(0, genesis.genesis_hash());

        let mut parent = genesis.genesis_hash();
        for (i, &count) in tx_counts.iter().enumerate() {
            let slot = (i + 1) as u64;
            let transactions: Vec<VersionedTransaction> = (0..count)
                .map(|n| {
                    let ix = system_instruction::transfer(
                        &payer.pubkey(),
                        &Pubkey::new_unique(),
                        (n + 1) as u64,
                    );
                    VersionedTransaction::from(Transaction::new_signed_with_payer(
                        &[ix],
                        Some(&payer.pubkey()),
                        &[&payer],
                        Hash::new_from_array([slot as u8, n as u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
                    ))
                })
                .collect();
            let metas: Vec<_> = transactions
                .iter()
                .map(|_| zul_primitives::meta::ExecutedTransactionMeta {
                    status: Ok(()),
                    fee: 5000,
                    compute_units_consumed: 150,
                    log_messages: vec![],
                    pre_balances: vec![],
                    post_balances: vec![],
                })
                .collect();
            let update = store.prepare_state_update(&[]).unwrap();
            let header = BlockHeader {
                slot,
                parent_hash: parent,
                state_root: update.new_root,
                transactions_root: Block::transactions_root(&transactions),
                timestamp_ms: slot,
                transaction_count: count,
            };
            let block = Block::seal(header, transactions, &sequencer);
            parent = block.header.hash();
            queue.register(slot, parent);
            store
                .commit_block(&block, &metas, &[], &update, &queue, &[])
                .unwrap();
        }
        let tip = tx_counts.len() as u64;
        (dir, store, tip)
    }

    #[test]
    fn batches_only_nonempty_blocks_and_roundtrips_payload() {
        let (_dir, store, tip) = store_with_blocks(&[2, 0, 1]);
        let mut batcher = Batcher::new(MockL1::default(), 1).unwrap();

        let record = batcher.run_once(&store, tip).unwrap().expect("record");
        assert_eq!(record.batch_no, 0);
        assert_eq!(record.first_slot, 1);
        assert_eq!(record.last_slot, 3);
        assert!(record.chunk_count >= 1);
        assert_eq!(batcher.cursor_slot(), tip + 1);

        // Reassemble DA chunks and verify payload + hash integrity.
        let chunks = batcher.client.chunks.lock().unwrap();
        let mut compressed = Vec::new();
        for (_, _, c) in chunks.iter() {
            compressed.extend_from_slice(c);
        }
        assert_eq!(da_hash(&compressed), record.da_hash);
        let payload = BatchPayload::decompress(&compressed).unwrap();
        assert_eq!(payload.blocks.len(), 2); // the empty block is excluded
        assert_eq!(
            payload.blocks.iter().map(|b| b.header.slot).collect::<Vec<_>>(),
            vec![1, 3]
        );
    }

    #[test]
    fn empty_range_advances_cursor_without_posting() {
        let (_dir, store, tip) = store_with_blocks(&[0, 0]);
        let mut batcher = Batcher::new(MockL1::default(), 1).unwrap();
        assert!(batcher.run_once(&store, tip).unwrap().is_none());
        assert_eq!(batcher.cursor_slot(), tip + 1);
        assert!(batcher.client.records.lock().unwrap().is_empty());
    }

    #[test]
    fn resumes_batch_numbering_from_l1() {
        let l1 = MockL1::default();
        l1.records.lock().unwrap().push(BatchRecord {
            batch_no: 7,
            first_slot: 0,
            last_slot: 9,
            state_root: [1u8; 32],
            withdrawals_root: [3u8; 32],
            da_hash: [2u8; 32],
            chunk_count: 1,
        });
        let (_dir, store, tip) = store_with_blocks(&[1]);
        let mut batcher = Batcher::new(l1, 1).unwrap();
        let record = batcher.run_once(&store, tip).unwrap().unwrap();
        assert_eq!(record.batch_no, 8);
    }

    #[test]
    fn chunking_respects_budget() {
        let data = vec![7u8; DA_CHUNK_BYTES * 2 + 5];
        let chunks = chunk_payload(&data);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| c.len() <= DA_CHUNK_BYTES));
        assert_eq!(chunks[2].len(), 5);
    }
}
