//! The sequencer node: owns the store, executor, mempool, and chain tip;
//! produces blocks on a fixed slot cadence; implements `RpcBackend`.

use zul_executor::{BlockExecutionInput, Executor};
use zul_primitives::block::{Block, BlockHeader};
use zul_primitives::genesis::GenesisConfig;
use zul_primitives::hash::H256;
use zul_rpc::{BackendError, BlockNotification, RpcBackend, SimulationResult};
use zul_store::{BlockhashQueue, LeafUpdate, Store};
use solana_sdk::{
    hash::Hash,
    pubkey::Pubkey,
    signature::{Keypair, Signature},
    signer::Signer,
    transaction::VersionedTransaction,
};
use solana_system_interface::instruction as system_instruction;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::sync::broadcast;

pub struct NodeOptions {
    pub slot_duration: Duration,
    /// Produce an empty heartbeat block if this much time passed without
    /// one (keeps the chain visibly alive without spamming empty slots).
    pub heartbeat: Duration,
    pub mempool_capacity: usize,
    /// Max transactions one fee payer may hold in the mempool at once.
    pub max_per_payer: usize,
    pub max_block_transactions: usize,
    pub faucet: Option<Keypair>,
    pub faucet_max_lamports: u64,
}

impl Default for NodeOptions {
    fn default() -> Self {
        Self {
            slot_duration: Duration::from_millis(
                zul_primitives::constants::DEFAULT_SLOT_DURATION_MS,
            ),
            heartbeat: Duration::from_secs(10),
            mempool_capacity: 10_000,
            max_per_payer: 128,
            max_block_transactions: 1_024,
            faucet: None,
            faucet_max_lamports: 10_000_000_000,
        }
    }
}

struct ChainTip {
    queue: BlockhashQueue,
    slot: u64,
    blockhash: H256,
    last_block_unix_ms: u64,
}

pub struct Node {
    store: Arc<Store>,
    executor: Executor,
    sequencer: Keypair,
    tip: RwLock<ChainTip>,
    mempool: Mutex<crate::mempool::Mempool>,
    options: NodeOptions,
    /// Set during shutdown so `is_healthy` reports false and a load balancer
    /// drains this node before the process exits.
    draining: AtomicBool,
    total_txs: AtomicU64,
    notifier: broadcast::Sender<Arc<BlockNotification>>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_millis() as u64
}

impl Node {
    /// Open the store, apply genesis (idempotent), and restore the tip.
    pub fn new(
        store: Arc<Store>,
        genesis: &GenesisConfig,
        sequencer: Keypair,
        options: NodeOptions,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            sequencer.pubkey() == genesis.sequencer,
            "sequencer keypair {} does not match genesis sequencer {}",
            sequencer.pubkey(),
            genesis.sequencer
        );

        let executor = Executor::new(genesis.fee_collector, vec![])?;
        let genesis_accounts = executor.genesis_accounts();
        store.initialize_genesis(genesis, &genesis_accounts)?;
        let genesis_hash = genesis.genesis_hash();

        let (queue, slot, blockhash) = match store.latest_slot()? {
            Some(latest) => {
                let queue = store
                    .load_blockhash_queue()?
                    .ok_or_else(|| anyhow::anyhow!("store has blocks but no blockhash queue"))?;
                let block = store
                    .get_block(latest)?
                    .ok_or_else(|| anyhow::anyhow!("latest slot {latest} missing"))?;
                (queue, latest, block.header.hash())
            }
            None => {
                let mut queue = BlockhashQueue::new();
                queue.register(0, genesis_hash);
                (queue, 0, genesis_hash)
            }
        };

        let mempool =
            crate::mempool::Mempool::new(options.mempool_capacity, options.max_per_payer);
        Ok(Self {
            store,
            executor,
            sequencer,
            tip: RwLock::new(ChainTip {
                queue,
                slot,
                blockhash,
                last_block_unix_ms: now_ms(),
            }),
            mempool: Mutex::new(mempool),
            options,
            draining: AtomicBool::new(false),
            total_txs: AtomicU64::new(0),
            notifier: broadcast::channel(1024).0,
        })
    }

    /// Flip the node into draining: `is_healthy` reports false so a monitor or
    /// load balancer stops routing to it before the process exits.
    pub fn begin_draining(&self) {
        self.draining.store(true, Ordering::Relaxed);
    }

    pub fn slot_duration(&self) -> Duration {
        self.options.slot_duration
    }

    pub fn last_block_age_ms(&self) -> u64 {
        let tip = self.tip.read().unwrap();
        now_ms().saturating_sub(tip.last_block_unix_ms)
    }

    pub fn mempool_depth(&self) -> usize {
        self.mempool.lock().unwrap().len()
    }

    pub fn total_txs(&self) -> u64 {
        self.total_txs.load(Ordering::Relaxed)
    }

    /// Produce one block if there is work (or a heartbeat is due).
    /// Returns the new slot if a block was committed.
    pub fn try_produce_block(&self) -> anyhow::Result<Option<u64>> {
        let transactions = {
            let mut mempool = self.mempool.lock().unwrap();
            mempool.drain(self.options.max_block_transactions)
        };

        let timestamp_ms = now_ms();
        if transactions.is_empty() {
            let tip = self.tip.read().unwrap();
            let due = timestamp_ms.saturating_sub(tip.last_block_unix_ms)
                >= self.options.heartbeat.as_millis() as u64;
            if !due {
                return Ok(None);
            }
        }

        // Single producer: hold the tip lock across execute+commit so RPC
        // reads see a consistent tip. Execution is fast at L2 scale.
        let mut tip = self.tip.write().unwrap();
        let slot = tip.slot + 1;
        let parent_hash = tip.blockhash;

        let output = self.executor.execute_block(
            &self.store,
            BlockExecutionInput {
                slot,
                timestamp_ms,
                parent_blockhash: Hash::new_from_array(parent_hash),
                transactions,
                queue: &tip.queue,
            },
        )?;

        for (sig, err) in &output.dropped {
            tracing::warn!(%sig, ?err, "transaction dropped at block production");
        }

        let account_writes = output.account_writes;
        let leaf_updates: Vec<LeafUpdate> = account_writes
            .iter()
            .map(|(pubkey, account)| (*pubkey, account.as_ref().map(|a| a.hash(pubkey))))
            .collect();
        let state_update = self.store.prepare_state_update(&leaf_updates)?;

        let (transactions, metas): (Vec<VersionedTransaction>, Vec<_>) =
            output.included.into_iter().unzip();
        let header = BlockHeader {
            slot,
            parent_hash,
            state_root: state_update.new_root,
            transactions_root: Block::transactions_root(&transactions),
            timestamp_ms,
            transaction_count: transactions.len() as u32,
        };
        let block = Block::seal(header, transactions, &self.sequencer);
        let blockhash = block.header.hash();

        let mut queue = tip.queue.clone();
        queue.register(slot, blockhash);
        self.store
            .commit_block(&block, &metas, &account_writes, &state_update, &queue, &[])?;

        tip.queue = queue;
        tip.slot = slot;
        tip.blockhash = blockhash;
        tip.last_block_unix_ms = timestamp_ms;

        self.total_txs
            .fetch_add(block.header.transaction_count as u64, Ordering::Relaxed);

        if self.notifier.receiver_count() > 0 {
            let signatures = block
                .transactions
                .iter()
                .zip(metas.iter())
                .filter_map(|(tx, meta)| {
                    tx.signatures
                        .first()
                        .map(|sig| (*sig, meta.status.as_ref().err().map(|e| format!("{e:?}"))))
                })
                .collect();
            let _ = self.notifier.send(Arc::new(BlockNotification {
                slot,
                parent_slot: slot.saturating_sub(1),
                signatures,
                accounts: account_writes.clone(),
            }));
        }

        tracing::info!(
            slot,
            txs = block.header.transaction_count,
            fees = output.fees_collected,
            "block committed"
        );
        Ok(Some(slot))
    }

    /// Admission checks shared by sendTransaction and the faucet.
    fn admit(&self, tx: &VersionedTransaction) -> Result<Signature, BackendError> {
        let Some(sig) = tx.signatures.first().copied() else {
            return Err(BackendError::Rejected("missing signature".into()));
        };
        if tx.verify_and_hash_message().is_err() {
            return Err(BackendError::Rejected("signature verification failed".into()));
        }
        {
            let tip = self.tip.read().unwrap();
            if !tip.queue.is_valid_hash(tx.message.recent_blockhash()) {
                return Err(BackendError::Rejected("blockhash not found".into()));
            }
        }
        if self
            .store
            .get_transaction(&sig)
            .map_err(|e| BackendError::Internal(e.to_string()))?
            .is_some()
        {
            return Err(BackendError::Rejected("already processed".into()));
        }
        Ok(sig)
    }
}

#[async_trait::async_trait]
impl RpcBackend for Node {
    fn store(&self) -> &Arc<Store> {
        &self.store
    }

    fn current_slot(&self) -> u64 {
        self.tip.read().unwrap().slot
    }

    fn latest_blockhash(&self) -> (u64, H256) {
        let tip = self.tip.read().unwrap();
        (tip.slot, tip.blockhash)
    }

    fn is_blockhash_valid(&self, blockhash: &H256) -> bool {
        self.tip.read().unwrap().queue.is_valid(blockhash)
    }

    fn is_healthy(&self) -> bool {
        if self.draining.load(Ordering::Relaxed) {
            return false;
        }
        let limit = (self.options.heartbeat.as_millis() as u64)
            .saturating_mul(3)
            .max(30_000);
        self.last_block_age_ms() < limit
    }

    fn subscribe(&self) -> broadcast::Receiver<Arc<BlockNotification>> {
        self.notifier.subscribe()
    }

    fn withdrawal_proof(
        &self,
        _recipient: &[u8; 32],
        _amount: u64,
        _nonce: u64,
    ) -> Result<(H256, zul_store::SmtProof), String> {
        // Bridge withdrawals are not wired up yet.
        Err("withdrawal proofs not available".into())
    }

    async fn submit_transaction(
        &self,
        tx: VersionedTransaction,
    ) -> Result<Signature, BackendError> {
        let sig = self.admit(&tx)?;
        let mut mempool = self.mempool.lock().unwrap();
        match mempool.push(tx) {
            Ok(_) => Ok(sig),
            Err(crate::mempool::MempoolError::Duplicate) => Ok(sig),
            Err(crate::mempool::MempoolError::Full) => {
                Err(BackendError::Rejected("mempool full".into()))
            }
            Err(crate::mempool::MempoolError::PayerLimit) => Err(BackendError::Rejected(
                "fee payer has too many pending transactions".into(),
            )),
            Err(crate::mempool::MempoolError::Unsigned) => {
                Err(BackendError::Rejected("missing signature".into()))
            }
        }
    }

    async fn simulate_transaction(
        &self,
        tx: VersionedTransaction,
    ) -> Result<SimulationResult, BackendError> {
        let (slot, parent, queue) = {
            let tip = self.tip.read().unwrap();
            (tip.slot + 1, tip.blockhash, tip.queue.clone())
        };
        let output = self
            .executor
            .execute_block(
                &self.store,
                BlockExecutionInput {
                    slot,
                    timestamp_ms: now_ms(),
                    parent_blockhash: Hash::new_from_array(parent),
                    transactions: vec![tx],
                    queue: &queue,
                },
            )
            .map_err(|e| BackendError::Internal(e.to_string()))?;

        if let Some((_, meta)) = output.included.into_iter().next() {
            return Ok(SimulationResult {
                err: meta.status.err().map(|e| format!("{e:?}")),
                logs: meta.log_messages,
                units_consumed: meta.compute_units_consumed,
            });
        }
        if let Some((_, err)) = output.dropped.into_iter().next() {
            return Ok(SimulationResult {
                err: Some(format!("{err:?}")),
                logs: vec![],
                units_consumed: 0,
            });
        }
        Err(BackendError::Internal("empty simulation output".into()))
    }

    async fn request_airdrop(
        &self,
        pubkey: Pubkey,
        lamports: u64,
    ) -> Result<Signature, BackendError> {
        let Some(faucet) = self.options.faucet.as_ref() else {
            return Err(BackendError::FaucetDisabled);
        };
        if lamports == 0 || lamports > self.options.faucet_max_lamports {
            return Err(BackendError::FaucetLimit);
        }
        let blockhash = {
            let tip = self.tip.read().unwrap();
            Hash::new_from_array(tip.blockhash)
        };
        let ix = system_instruction::transfer(&faucet.pubkey(), &pubkey, lamports);
        let tx = solana_sdk::transaction::Transaction::new_signed_with_payer(
            &[ix],
            Some(&faucet.pubkey()),
            &[faucet],
            blockhash,
        );
        self.submit_transaction(VersionedTransaction::from(tx))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zul_primitives::genesis::Allocation;

    const ZUL: u64 = 1_000_000_000;

    struct Harness {
        _dir: tempfile::TempDir,
        node: Node,
        payer: Keypair,
    }

    fn setup() -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("zul.redb")).unwrap());
        let sequencer = Keypair::new();
        let faucet = Keypair::new();
        let payer = Keypair::new();
        let genesis = GenesisConfig {
            chain_name: "zul-node-test".into(),
            creation_time_ms: 1_760_000_000_000,
            sequencer: sequencer.pubkey(),
            fee_collector: Pubkey::new_unique(),
            allocations: vec![
                Allocation {
                    pubkey: faucet.pubkey(),
                    lamports: 1_000 * ZUL,
                },
                Allocation {
                    pubkey: payer.pubkey(),
                    lamports: 100 * ZUL,
                },
            ],
        };
        let node = Node::new(
            store,
            &genesis,
            sequencer,
            NodeOptions {
                faucet: Some(faucet),
                heartbeat: Duration::from_secs(3600),
                ..Default::default()
            },
        )
        .unwrap();
        Harness {
            _dir: dir,
            node,
            payer,
        }
    }

    fn transfer(payer: &Keypair, to: &Pubkey, lamports: u64, blockhash: H256) -> VersionedTransaction {
        let ix = system_instruction::transfer(&payer.pubkey(), to, lamports);
        VersionedTransaction::from(solana_sdk::transaction::Transaction::new_signed_with_payer(
            &[ix],
            Some(&payer.pubkey()),
            &[payer],
            Hash::new_from_array(blockhash),
        ))
    }

    #[tokio::test]
    async fn submit_then_produce_block_and_query() {
        let h = setup();
        let recipient = Pubkey::new_unique();
        let (_, blockhash) = h.node.latest_blockhash();

        let tx = transfer(&h.payer, &recipient, ZUL, blockhash);
        let sig = h.node.submit_transaction(tx).await.unwrap();
        assert_eq!(h.node.current_slot(), 0);

        let slot = h.node.try_produce_block().unwrap().expect("block");
        assert_eq!(slot, 1);

        let (loc, _, meta) = h.node.store().get_transaction(&sig).unwrap().unwrap();
        assert_eq!(loc.slot, 1);
        assert!(meta.status.is_ok());
        let recipient_account = h.node.store().get_account(&recipient).unwrap().unwrap();
        assert_eq!(recipient_account.lamports, ZUL);
    }

    #[tokio::test]
    async fn empty_mempool_produces_no_block_until_heartbeat() {
        let h = setup();
        assert_eq!(h.node.try_produce_block().unwrap(), None);
        assert_eq!(h.node.current_slot(), 0);
    }

    #[tokio::test]
    async fn airdrop_funds_account_via_real_transfer() {
        let h = setup();
        let target = Pubkey::new_unique();
        let sig = h.node.request_airdrop(target, 2 * ZUL).await.unwrap();
        h.node.try_produce_block().unwrap().expect("block");
        let account = h.node.store().get_account(&target).unwrap().unwrap();
        assert_eq!(account.lamports, 2 * ZUL);
        let (_, _, meta) = h.node.store().get_transaction(&sig).unwrap().unwrap();
        assert!(meta.status.is_ok());
    }
}
