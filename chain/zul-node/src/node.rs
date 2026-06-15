//! The sequencer node: owns the store, executor, mempool, and chain tip;
//! produces blocks on a fixed slot cadence; implements `RpcBackend`.

use zul_executor::{BlockExecutionInput, Executor};
use zul_primitives::block::{Block, BlockHeader};
use zul_primitives::genesis::GenesisConfig;
use zul_primitives::hash::H256;
use zul_primitives::meta::ExecutedTransactionMeta;
use zul_privacy::{
    Groth16Verifier, PoolConfig, PoolService, VerifyingKeys, POOL_STATE_ADDRESS,
    POOL_VAULT_ADDRESS,
};
use zul_bridge::DepositEvent;
use zul_rpc::{BackendError, BlockNotification, RpcBackend, SimulationResult};
use zul_store::smt::NodeKey;
use zul_store::{BlockhashQueue, LeafUpdate, Store, StoredAccount};
use solana_sdk::{
    hash::Hash,
    pubkey::Pubkey,
    signature::{Keypair, Signature},
    signer::Signer,
    transaction::VersionedTransaction,
};
use solana_system_interface::instruction as system_instruction;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::sync::broadcast;

use crate::pool;

pub struct NodeOptions {
    pub slot_duration: Duration,
    /// Produce an empty heartbeat block if this much time passed without
    /// one (keeps the chain visibly alive without spamming empty slots).
    pub heartbeat: Duration,
    pub mempool_capacity: usize,
    /// Max transactions one fee payer may hold in the mempool at once
    /// (anti-spam: stops a single key from filling the queue).
    pub max_per_payer: usize,
    pub max_block_transactions: usize,
    pub faucet: Option<Keypair>,
    pub faucet_max_lamports: u64,
    /// Shielded-pool Groth16 verifying keys. Unconfigured (default) rejects
    /// every transfer/unshield proof — the pool is closed-but-safe until
    /// keys are loaded; shields (no proof) still work.
    pub pool_verifying_keys: VerifyingKeys,
    /// L1 mint whose deposits credit the native gas token 1:1 (mainnet ZUL-SPL).
    /// `None` = native ZUL is faucet/SOL-backed (testnet); `Some` = native ZUL
    /// is the bridged gas mint, and SOL/other assets are wrapped tokens.
    pub gas_mint: Option<Pubkey>,
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
            pool_verifying_keys: VerifyingKeys::unconfigured(),
            gas_mint: None,
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
    pool: PoolService<Groth16Verifier>,
    sequencer: Keypair,
    fee_collector: Pubkey,
    tip: RwLock<ChainTip>,
    mempool: Mutex<crate::mempool::Mempool>,
    options: NodeOptions,
    /// Set during shutdown so `is_healthy` reports false and a load balancer
    /// drains this node before the process exits.
    draining: AtomicBool,
    /// Transactions committed since boot (metrics counter).
    total_txs: AtomicU64,
    /// Per-block notifications for WebSocket subscriptions.
    notifier: broadcast::Sender<Arc<BlockNotification>>,
    /// L1 deposits awaiting an L2 credit (enqueued by the deposit watcher,
    /// applied + deduplicated atomically in `try_produce_block`).
    pending_deposits: Mutex<VecDeque<DepositEvent>>,
    /// In-memory shallow (depth-32) sparse Merkle tree of withdrawal
    /// commitments; its root is posted to L1 for `claim_withdrawal`.
    withdrawals: Mutex<HashMap<NodeKey, H256>>,
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
        extra_builtins: Vec<zul_executor::builtins::BuiltinSpec>,
        options: NodeOptions,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            sequencer.pubkey() == genesis.sequencer,
            "sequencer keypair {} does not match genesis sequencer {}",
            sequencer.pubkey(),
            genesis.sequencer
        );

        let executor = Executor::new(genesis.fee_collector, extra_builtins)?;

        // The shielded pool is enshrined in the node (processed outside the
        // SVM). Its state account is part of genesis.
        let pool_config =
            PoolConfig::new(POOL_STATE_ADDRESS, POOL_VAULT_ADDRESS, genesis.fee_collector);
        let pool = PoolService::new(
            Groth16Verifier::new(options.pool_verifying_keys.clone()),
            pool_config,
        );

        let mut genesis_accounts = executor.genesis_accounts();
        genesis_accounts.push(pool.genesis_pool_account());
        store.initialize_genesis(genesis, &genesis_accounts)?;
        let genesis_hash = genesis.genesis_hash();

        // Restore tip state. The queue is persisted with every commit; on
        // first boot register the genesis hash as the slot-0 "blockhash".
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
        let node = Self {
            store,
            executor,
            pool,
            sequencer,
            fee_collector: genesis.fee_collector,
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
            pending_deposits: Mutex::new(VecDeque::new()),
            withdrawals: Mutex::new(HashMap::new()),
        };
        node.rebuild_withdrawals()?;
        Ok(node)
    }

    /// Rebuild the in-memory withdrawals tree from committed blocks (the tree
    /// is not persisted; the blocks are its source of truth). Only successful
    /// withdrawals committed a leaf, so each is checked against its tx status.
    fn rebuild_withdrawals(&self) -> anyhow::Result<()> {
        let latest = self.store.latest_slot()?.unwrap_or(0);
        let mut tree = self.withdrawals.lock().unwrap();
        if latest > 0 {
            for slot in self.store.block_slots_in_range(1, latest, usize::MAX)? {
                let Some(block) = self.store.get_block(slot)? else { continue };
                let (withdraws, _) = crate::bridge::partition_withdraws(block.transactions.clone());
                for w in withdraws {
                    let succeeded = w
                        .transaction
                        .signatures
                        .first()
                        .and_then(|sig| self.store.get_transaction(sig).ok().flatten())
                        .map(|(_, _, meta)| meta.status.is_ok())
                        .unwrap_or(false);
                    if succeeded {
                        crate::withdraw_smt::apply(
                            &mut tree,
                            &crate::bridge::withdrawal_pubkey(&w.recipient, w.nonce),
                            &crate::bridge::withdrawal_account_hash(&w.recipient, &w.asset_id, w.amount, w.nonce),
                        );
                    }
                }
            }
        }
        let root = crate::withdraw_smt::root(&tree);
        drop(tree);
        self.store.set_withdrawals_root(&root)?;
        Ok(())
    }

    /// Queue L1 deposits for crediting on the L2 (called by the deposit
    /// watcher). Credits are applied + deduplicated in `try_produce_block`.
    pub fn enqueue_deposits(&self, deposits: Vec<DepositEvent>) {
        self.pending_deposits.lock().unwrap().extend(deposits);
    }

    /// Apply queued deposits into this block's account writes, exactly once.
    /// Each deposit mints its `amount` to the recipient on the L2 (backed by
    /// the SOL held in the L1 vault) and writes a receipt account whose
    /// existence marks the L1 signature as credited — committed atomically
    /// with the credit, so a crash never double-credits or loses a deposit
    /// (the watcher re-fetches uncredited deposits from L1).
    fn apply_deposits(
        &self,
        deposits: &[DepositEvent],
        writes: &mut HashMap<Pubkey, Option<StoredAccount>>,
    ) -> anyhow::Result<usize> {
        let mut credited = 0;
        for deposit in deposits {
            let receipt = crate::bridge::deposit_receipt_address(&deposit.l1_signature);
            // Skip if already credited in this block or in a committed block.
            if writes.contains_key(&receipt) || self.store.get_account(&receipt)?.is_some() {
                continue;
            }
            // Asset routing depends on whether a gas mint is configured:
            //  - gas_mint deposit            → credit NATIVE ZUL 1:1 (gas)
            //  - SOL (None), no gas_mint set  → credit native ZUL (testnet legacy)
            //  - SOL (None), gas_mint set     → wrap as wSOL (SOL is not gas on mainnet)
            //  - any other SPL                → wrapped token
            let gas_mint = self.options.gas_mint;
            let credits_native_gas = match deposit.mint {
                Some(m) => Some(m) == gas_mint,
                None => gas_mint.is_none(),
            };
            if credits_native_gas {
                let mut account = writes
                    .get(&deposit.l2_recipient)
                    .cloned()
                    .flatten()
                    .or(self.store.get_account(&deposit.l2_recipient)?)
                    .unwrap_or_else(|| StoredAccount::new_system(0));
                account.lamports = account.lamports.saturating_add(deposit.amount);
                writes.insert(deposit.l2_recipient, Some(account));
            } else {
                // Wrapped: SOL (gas_mint set) uses the canonical wSOL mint;
                // every other SPL uses its own L1 mint.
                let (mint, decimals) = match deposit.mint {
                    Some(m) => (m, deposit.decimals),
                    None => (zul_primitives::wrapped_spl::native_sol_mint(), 9),
                };
                self.credit_wrapped_spl(&mint, &deposit.l2_recipient, deposit.amount, decimals, writes)?;
            }

            let mut data = Vec::with_capacity(40);
            data.extend_from_slice(&deposit.amount.to_le_bytes());
            data.extend_from_slice(deposit.l2_recipient.as_ref());
            writes.insert(
                receipt,
                Some(StoredAccount {
                    lamports: 0,
                    data,
                    owner: crate::bridge::BRIDGE_RECEIPT_OWNER,
                    executable: false,
                    rent_epoch: u64::MAX,
                }),
            );
            tracing::info!(
                recipient = %deposit.l2_recipient,
                amount = deposit.amount,
                l1_sig = %deposit.l1_signature,
                "credited L1 deposit on L2"
            );
            credited += 1;
        }
        Ok(credited)
    }

    /// Mint `amount` of the wrapped token for `l1_mint` to `recipient`'s
    /// associated token account, creating the wrapped mint and/or the ATA on
    /// first use. Synthesizes SPL account states directly (consistent with how
    /// native deposits write lamports); the byte layouts are exact, so the
    /// balance is spendable through the SVM Token program.
    fn credit_wrapped_spl(
        &self,
        l1_mint: &Pubkey,
        recipient: &Pubkey,
        amount: u64,
        decimals: u8,
        writes: &mut HashMap<Pubkey, Option<StoredAccount>>,
    ) -> anyhow::Result<()> {
        use zul_primitives::wrapped_spl as w;
        let wmint = w::wrapped_mint_address(l1_mint);

        // Wrapped mint: bump supply in place, or create it on first deposit.
        let mint_acc = writes
            .get(&wmint)
            .cloned()
            .flatten()
            .or(self.store.get_account(&wmint)?);
        let mint_acc = match mint_acc {
            Some(mut acc) => {
                let supply = w::mint_supply(&acc.data).unwrap_or(0).saturating_add(amount);
                w::set_mint_supply(&mut acc.data, supply);
                acc
            }
            None => StoredAccount {
                lamports: w::rent_exempt_lamports(w::MINT_LEN),
                data: w::pack_mint(&w::BRIDGE_MINT_AUTHORITY, amount, decimals),
                owner: w::TOKEN_PROGRAM_ID,
                executable: false,
                rent_epoch: u64::MAX,
            },
        };
        writes.insert(wmint, Some(mint_acc));

        // Recipient ATA: bump amount in place, or create it.
        let ata = w::associated_token_address(recipient, &wmint);
        let ata_acc = writes
            .get(&ata)
            .cloned()
            .flatten()
            .or(self.store.get_account(&ata)?);
        let ata_acc = match ata_acc {
            Some(mut acc) => {
                let bal = w::token_account_amount(&acc.data)
                    .unwrap_or(0)
                    .saturating_add(amount);
                w::set_token_account_amount(&mut acc.data, bal);
                acc
            }
            None => StoredAccount {
                lamports: w::rent_exempt_lamports(w::ACCOUNT_LEN),
                data: w::pack_token_account(&wmint, recipient, amount),
                owner: w::TOKEN_PROGRAM_ID,
                executable: false,
                rent_epoch: u64::MAX,
            },
        };
        writes.insert(ata, Some(ata_acc));
        Ok(())
    }

    /// Burn `amount` of `l1_mint`'s wrapped token from `owner`'s ATA (and reduce
    /// the wrapped mint supply) for a withdrawal. Returns false if the balance
    /// is insufficient or the account is missing.
    fn burn_wrapped_spl(
        &self,
        l1_mint: &Pubkey,
        owner: &Pubkey,
        amount: u64,
        writes: &mut HashMap<Pubkey, Option<StoredAccount>>,
    ) -> anyhow::Result<bool> {
        use zul_primitives::wrapped_spl as w;
        if amount == 0 {
            return Ok(false);
        }
        let wmint = w::wrapped_mint_address(l1_mint);
        let ata = w::associated_token_address(owner, &wmint);
        let Some(mut acc) = writes
            .get(&ata)
            .cloned()
            .flatten()
            .or(self.store.get_account(&ata)?)
        else {
            return Ok(false);
        };
        let bal = w::token_account_amount(&acc.data).unwrap_or(0);
        if bal < amount {
            return Ok(false);
        }
        w::set_token_account_amount(&mut acc.data, bal - amount);
        writes.insert(ata, Some(acc));

        if let Some(mut macc) = writes
            .get(&wmint)
            .cloned()
            .flatten()
            .or(self.store.get_account(&wmint)?)
        {
            let supply = w::mint_supply(&macc.data).unwrap_or(0).saturating_sub(amount);
            w::set_mint_supply(&mut macc.data, supply);
            writes.insert(wmint, Some(macc));
        }
        Ok(true)
    }

    /// Apply bridge withdrawals: burn `amount + fee` from the withdrawer.
    /// Returns the `(withdrawal_pubkey, account_hash)` leaves to insert into
    /// the withdrawals tree once the block commits — the L1 `claim_withdrawal`
    /// recomputes that hash and proves its inclusion.
    fn apply_withdraws(
        &self,
        withdraws: Vec<crate::bridge::WithdrawTx>,
        writes: &mut HashMap<Pubkey, Option<StoredAccount>>,
        included: &mut Vec<(VersionedTransaction, ExecutedTransactionMeta)>,
    ) -> Vec<(Pubkey, H256)> {
        let mut leaves = Vec::new();
        for w in withdraws {
            // A withdrawal burns native ZUL lamports when it exits the native
            // asset: the reserved native id (testnet, → SOL on claim) OR the
            // configured gas mint (mainnet native ZUL, → ZUL-SPL on claim). Any
            // other asset id is a wrapped token, burned from its ATA.
            let is_native = w.asset_id == crate::bridge::NATIVE_ASSET_ID
                || self
                    .options
                    .gas_mint
                    .map(|m| m.to_bytes() == w.asset_id)
                    .unwrap_or(false);
            // Every withdrawal pays the native fee; a native withdrawal also
            // burns `amount` lamports, an SPL one burns the wrapped token.
            let needed_native = if is_native {
                w.amount.saturating_add(pool::POOL_TX_FEE)
            } else {
                pool::POOL_TX_FEE
            };
            let payer = writes
                .get(&w.fee_payer)
                .cloned()
                .flatten()
                .or(self.store.get_account(&w.fee_payer).ok().flatten());

            let post = match payer {
                Some(mut acc) if acc.lamports >= needed_native && w.amount > 0 => {
                    // Burn the SPL token first so a shortfall doesn't take the fee.
                    let token_ok = is_native
                        || self
                            .burn_wrapped_spl(
                                &Pubkey::new_from_array(w.asset_id),
                                &w.fee_payer,
                                w.amount,
                                writes,
                            )
                            .unwrap_or(false);
                    if token_ok {
                        acc.lamports -= needed_native;
                        let post = acc.lamports;
                        writes.insert(w.fee_payer, Some(acc));
                        Some(post)
                    } else {
                        None
                    }
                }
                _ => None,
            };

            match post {
                Some(post) => {
                    // Fee to the collector (the withdrawn value is burned on L2;
                    // the matching asset is released from the L1 vault on claim).
                    let mut collector = writes
                        .get(&self.fee_collector)
                        .cloned()
                        .flatten()
                        .or(self.store.get_account(&self.fee_collector).ok().flatten())
                        .unwrap_or_else(|| StoredAccount::new_system(0));
                    collector.lamports = collector.lamports.saturating_add(pool::POOL_TX_FEE);
                    writes.insert(self.fee_collector, Some(collector));

                    leaves.push((
                        crate::bridge::withdrawal_pubkey(&w.recipient, w.nonce),
                        crate::bridge::withdrawal_account_hash(
                            &w.recipient,
                            &w.asset_id,
                            w.amount,
                            w.nonce,
                        ),
                    ));
                    tracing::info!(
                        amount = w.amount,
                        native = is_native,
                        nonce = w.nonce,
                        "committed L2 withdrawal (claimable on L1 once settled)"
                    );
                    included.push((
                        w.transaction,
                        pool::pool_meta(
                            true,
                            None,
                            pool::POOL_TX_FEE,
                            0,
                            post,
                            vec![format!(
                                "Program log: withdraw {} to L1, nonce {}",
                                w.amount, w.nonce
                            )],
                        ),
                    ));
                }
                None => included.push((
                    w.transaction,
                    pool::pool_meta(
                        false,
                        Some("insufficient balance for withdrawal".into()),
                        0,
                        0,
                        0,
                        vec![],
                    ),
                )),
            }
        }
        leaves
    }

    /// SMT inclusion proof for a withdrawal commitment against the current
    /// state root — serves `getWithdrawalProof` so a user can claim on L1.
    pub fn compute_withdrawal_proof(
        &self,
        recipient: &[u8; 32],
        asset_id: &[u8; 32],
        amount: u64,
        nonce: u64,
    ) -> anyhow::Result<(H256, zul_store::SmtProof)> {
        let leaf = crate::bridge::withdrawal_pubkey(recipient, nonce);
        let account_hash = crate::bridge::withdrawal_account_hash(recipient, asset_id, amount, nonce);
        let tree = self.withdrawals.lock().unwrap();
        let proof = crate::withdraw_smt::prove(&tree, &leaf);
        let root = crate::withdraw_smt::root(&tree);
        drop(tree);
        // Sanity: the proof must verify against the current withdrawals root,
        // so the caller gets a usable proof or a clear error.
        anyhow::ensure!(
            crate::withdraw_smt::verify(&root, &leaf, &account_hash, &proof),
            "withdrawal commitment not found (not yet committed?)"
        );
        Ok((root, proof))
    }

    /// Flip the node into draining: `is_healthy` reports false so a monitor or
    /// load balancer stops routing to it before the process exits.
    pub fn begin_draining(&self) {
        self.draining.store(true, Ordering::Relaxed);
    }

    pub fn slot_duration(&self) -> Duration {
        self.options.slot_duration
    }

    /// Milliseconds since the last block was committed — the liveness signal
    /// behind `getHealth` and the `/metrics` gauges.
    pub fn last_block_age_ms(&self) -> u64 {
        let tip = self.tip.read().unwrap();
        now_ms().saturating_sub(tip.last_block_unix_ms)
    }

    /// Transactions currently waiting in the mempool.
    pub fn mempool_depth(&self) -> usize {
        self.mempool.lock().unwrap().len()
    }

    /// Transactions committed since boot (metrics counter).
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
        let deposits: Vec<DepositEvent> = {
            let mut queue = self.pending_deposits.lock().unwrap();
            queue.drain(..).collect()
        };

        let timestamp_ms = now_ms();
        if transactions.is_empty() && deposits.is_empty() {
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

        // Bridge withdrawals and pool transactions are handled natively,
        // outside the SVM batch.
        let (withdraw_txs, rest) = crate::bridge::partition_withdraws(transactions);
        let (pool_txs, regular_txs) = pool::partition(rest);

        let output = self.executor.execute_block(
            &self.store,
            BlockExecutionInput {
                slot,
                timestamp_ms,
                parent_blockhash: Hash::new_from_array(parent_hash),
                transactions: regular_txs,
                queue: &tip.queue,
            },
        )?;

        for (sig, err) in &output.dropped {
            tracing::warn!(%sig, ?err, "transaction dropped at block production");
        }

        // Merge SVM writes and pool writes into one account-write set, with
        // pool processed on top of (seeded by) the SVM results.
        let mut writes: HashMap<Pubkey, Option<zul_store::StoredAccount>> =
            output.account_writes.iter().cloned().collect();
        let mut included = output.included;
        let mut pool_nullifiers: Vec<[u8; 32]> = Vec::new();

        if !pool_txs.is_empty() {
            let seed: HashMap<Pubkey, u64> = writes
                .iter()
                .filter_map(|(pk, acc)| acc.as_ref().map(|a| (*pk, a.lamports)))
                .collect();
            let pool_instructions: Vec<(Pubkey, zul_privacy::PoolInstruction)> = pool_txs
                .iter()
                .map(|pt| (pt.fee_payer, pt.instruction.clone()))
                .collect();
            let pool_result =
                self.pool
                    .process_block_seeded(&self.store, &pool_instructions, &seed)?;

            for (pubkey, pool_account) in pool_result.account_writes {
                // The pool only moves lamports. If the SVM already wrote this
                // account in the same block (e.g. a data change), keep the
                // SVM's fields and take only the pool's lamports — otherwise
                // the pool's view (built from pre-block committed state) would
                // clobber the SVM's update.
                let merged = match (writes.get(&pubkey), pool_account) {
                    (Some(Some(svm_account)), Some(pool_account)) => {
                        let mut acc = svm_account.clone();
                        acc.lamports = pool_account.lamports;
                        Some(acc)
                    }
                    (_, other) => other,
                };
                writes.insert(pubkey, merged);
            }
            pool_nullifiers = pool_result.nullifiers;

            for (pt, status) in pool_txs.into_iter().zip(pool_result.statuses) {
                let ok = status.is_ok();
                let fee = if ok { pool::POOL_TX_FEE } else { 0 };
                let post = pool::lamports_in_writes(
                    &writes.iter().map(|(k, v)| (*k, v.clone())).collect::<Vec<_>>(),
                    &pt.fee_payer,
                )
                .unwrap_or(0);
                let meta = pool::pool_meta(ok, status.err(), fee, 0, post, vec![]);
                included.push((pt.transaction, meta));
            }
        }

        // Credit any queued L1 deposits into this block's writes (exactly once).
        self.apply_deposits(&deposits, &mut writes)?;

        // Bridge withdrawals: burn the L2 balance now; the withdrawal leaves go
        // into the shallow withdrawals tree once the block commits (below).
        let withdrawal_leaves = self.apply_withdraws(withdraw_txs, &mut writes, &mut included);

        let account_writes: Vec<(Pubkey, Option<zul_store::StoredAccount>)> =
            writes.into_iter().collect();
        let leaf_updates: Vec<LeafUpdate> = account_writes
            .iter()
            .map(|(pubkey, account)| (*pubkey, account.as_ref().map(|a| a.hash(pubkey))))
            .collect();
        let state_update = self.store.prepare_state_update(&leaf_updates)?;

        let (transactions, metas): (Vec<VersionedTransaction>, Vec<_>) =
            included.into_iter().unzip();
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
        self.store.commit_block(
            &block,
            &metas,
            &account_writes,
            &state_update,
            &queue,
            &pool_nullifiers,
        )?;

        tip.queue = queue;
        tip.slot = slot;
        tip.blockhash = blockhash;
        tip.last_block_unix_ms = timestamp_ms;

        // Insert committed withdrawal leaves into the shallow withdrawals tree
        // and publish its root for the batcher to post to L1.
        if !withdrawal_leaves.is_empty() {
            let mut tree = self.withdrawals.lock().unwrap();
            for (leaf_pubkey, account_hash) in &withdrawal_leaves {
                crate::withdraw_smt::apply(&mut tree, leaf_pubkey, account_hash);
            }
            let root = crate::withdraw_smt::root(&tree);
            drop(tree);
            if let Err(e) = self.store.set_withdrawals_root(&root) {
                tracing::error!(error = %e, "failed to persist withdrawals root");
            }
        }

        self.total_txs
            .fetch_add(block.header.transaction_count as u64, Ordering::Relaxed);

        // Notify WebSocket subscribers (skip the work if nobody is listening).
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
        // Empty load still produces a heartbeat block, so a gap of several
        // heartbeats means the producer is stuck. Floor at 30s for tiny
        // heartbeat configs.
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
        recipient: &[u8; 32],
        asset_id: &[u8; 32],
        amount: u64,
        nonce: u64,
    ) -> Result<(H256, zul_store::SmtProof), String> {
        self.compute_withdrawal_proof(recipient, asset_id, amount, nonce)
            .map_err(|e| e.to_string())
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
        // Execute against committed state and discard everything.
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
        fee_collector: Pubkey,
    }

    fn setup() -> Harness {
        setup_with(|_| {})
    }

    fn setup_with(modify: impl FnOnce(&mut NodeOptions)) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("zul.redb")).unwrap());
        let sequencer = Keypair::new();
        let faucet = Keypair::new();
        let payer = Keypair::new();
        let fee_collector = Pubkey::new_unique();
        let genesis = GenesisConfig {
            chain_name: "zul-node-test".into(),
            creation_time_ms: 1_760_000_000_000,
            sequencer: sequencer.pubkey(),
            fee_collector,
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
        let mut options = NodeOptions {
            faucet: Some(faucet),
            heartbeat: Duration::from_secs(3600),
            ..Default::default()
        };
        modify(&mut options);
        let node = Node::new(store, &genesis, sequencer, vec![], options).unwrap();
        Harness {
            _dir: dir,
            node,
            payer,
            fee_collector,
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
    async fn gas_mint_deposit_credits_native_other_spl_wrapped() {
        use zul_primitives::wrapped_spl as w;
        let gas_mint = Pubkey::new_unique();
        let other_mint = Pubkey::new_unique();
        let h = setup_with(|o| o.gas_mint = Some(gas_mint));

        let gas_recipient = Pubkey::new_unique();
        let spl_recipient = Pubkey::new_unique();
        h.node.enqueue_deposits(vec![
            // The gas mint → native ZUL lamports (1:1, the gas-onboarding path).
            DepositEvent {
                l1_signature: "gasSig".into(),
                mint: Some(gas_mint),
                decimals: 9,
                amount: 3 * ZUL,
                l2_recipient: gas_recipient,
            },
            // Any other SPL → a wrapped token, NOT native gas.
            DepositEvent {
                l1_signature: "otherSig".into(),
                mint: Some(other_mint),
                decimals: 6,
                amount: 1_000_000,
                l2_recipient: spl_recipient,
            },
        ]);
        h.node.try_produce_block().unwrap().expect("deposit block");

        // Gas mint credited NATIVE lamports (no wrapped mint created for it).
        let gas_bal = h.node.store().get_account(&gas_recipient).unwrap().unwrap().lamports;
        assert_eq!(gas_bal, 3 * ZUL);
        let gas_wmint = w::wrapped_mint_address(&gas_mint);
        assert!(h.node.store().get_account(&gas_wmint).unwrap().is_none());

        // Other SPL credited a WRAPPED token, not native lamports.
        let other_wmint = w::wrapped_mint_address(&other_mint);
        let spl_ata = w::associated_token_address(&spl_recipient, &other_wmint);
        assert_eq!(
            w::token_account_amount(&h.node.store().get_account(&spl_ata).unwrap().unwrap().data),
            Some(1_000_000)
        );
        // The SPL recipient got no native gas (must bridge the gas mint for that).
        assert!(h.node.store().get_account(&spl_recipient).unwrap().is_none());
    }

    #[tokio::test]
    async fn spl_deposit_mints_wrapped_token_spendable_via_svm() {
        use zul_primitives::wrapped_spl as w;
        let h = setup();
        let l1_mint = Pubkey::new_unique();
        let owner2 = Pubkey::new_unique();
        let decimals = 6u8;

        // Two SPL deposits of the same L1 mint: one to the (native-funded)
        // payer, one to owner2 — so both associated token accounts exist.
        h.node.enqueue_deposits(vec![
            DepositEvent {
                l1_signature: "splSig1".into(),
                mint: Some(l1_mint),
                decimals,
                amount: 1_000_000,
                l2_recipient: h.payer.pubkey(),
            },
            DepositEvent {
                l1_signature: "splSig2".into(),
                mint: Some(l1_mint),
                decimals,
                amount: 500_000,
                l2_recipient: owner2,
            },
        ]);
        h.node.try_produce_block().unwrap().expect("deposit block");

        let wmint = w::wrapped_mint_address(&l1_mint);
        let payer_ata = w::associated_token_address(&h.payer.pubkey(), &wmint);
        let owner2_ata = w::associated_token_address(&owner2, &wmint);

        // Wrapped mint + both ATAs were synthesized with the right state.
        let mint_acc = h.node.store().get_account(&wmint).unwrap().unwrap();
        assert_eq!(mint_acc.owner, w::TOKEN_PROGRAM_ID);
        assert_eq!(w::mint_supply(&mint_acc.data), Some(1_500_000));
        assert_eq!(w::mint_decimals(&mint_acc.data), Some(decimals));
        let payer_tok = h.node.store().get_account(&payer_ata).unwrap().unwrap();
        assert_eq!(w::token_account_amount(&payer_tok.data), Some(1_000_000));
        assert_eq!(payer_tok.owner, w::TOKEN_PROGRAM_ID);

        // Now SPEND it through the SVM: an SPL Token `TransferChecked` (tag 12)
        // of 200_000 from the payer's ATA to owner2's ATA, signed by the payer.
        // TransferChecked re-derives + checks the mint's decimals, so this also
        // proves the synthesized *mint* account is byte-valid (not just the
        // token accounts). If anything were malformed the program would reject.
        let (_, blockhash) = h.node.latest_blockhash();
        let mut data = vec![12u8];
        data.extend_from_slice(&200_000u64.to_le_bytes());
        data.push(decimals);
        let ix = solana_sdk::instruction::Instruction {
            program_id: w::TOKEN_PROGRAM_ID,
            accounts: vec![
                solana_sdk::instruction::AccountMeta::new(payer_ata, false),
                solana_sdk::instruction::AccountMeta::new_readonly(wmint, false),
                solana_sdk::instruction::AccountMeta::new(owner2_ata, false),
                solana_sdk::instruction::AccountMeta::new_readonly(h.payer.pubkey(), true),
            ],
            data,
        };
        let tx = VersionedTransaction::from(
            solana_sdk::transaction::Transaction::new_signed_with_payer(
                &[ix],
                Some(&h.payer.pubkey()),
                &[&h.payer],
                Hash::new_from_array(blockhash),
            ),
        );
        h.node.submit_transaction(tx).await.unwrap();
        h.node.try_produce_block().unwrap().expect("transfer block");

        // The SVM moved wrapped-token balances between the synthesized accounts.
        let payer_tok = h.node.store().get_account(&payer_ata).unwrap().unwrap();
        let owner2_tok = h.node.store().get_account(&owner2_ata).unwrap().unwrap();
        assert_eq!(w::token_account_amount(&payer_tok.data), Some(800_000));
        assert_eq!(w::token_account_amount(&owner2_tok.data), Some(700_000));
    }

    #[tokio::test]
    async fn spl_withdraw_burns_wrapped_token_and_commits_asset_bound_leaf() {
        use zul_primitives::wrapped_spl as w;
        let h = setup();
        let l1_mint = Pubkey::new_unique();
        let l1_recipient = [4u8; 32]; // where the tokens go on L1
        let nonce = 1u64;

        // Deposit 1_000_000 wrapped tokens to the payer.
        h.node.enqueue_deposits(vec![DepositEvent {
            l1_signature: "splSigW".into(),
            mint: Some(l1_mint),
            decimals: 6,
            amount: 1_000_000,
            l2_recipient: h.payer.pubkey(),
        }]);
        h.node.try_produce_block().unwrap().expect("deposit block");

        // Withdraw 300_000 of it: SPL withdraw tx (80B) to the bridge program.
        let (_, blockhash) = h.node.latest_blockhash();
        let mut data = Vec::with_capacity(80);
        data.extend_from_slice(&l1_recipient);
        data.extend_from_slice(l1_mint.as_ref()); // asset id = L1 mint
        data.extend_from_slice(&300_000u64.to_le_bytes());
        data.extend_from_slice(&nonce.to_le_bytes());
        let ix = solana_sdk::instruction::Instruction {
            program_id: crate::bridge::BRIDGE_WITHDRAW_PROGRAM_ID,
            accounts: vec![solana_sdk::instruction::AccountMeta::new(
                h.payer.pubkey(),
                true,
            )],
            data,
        };
        let tx = VersionedTransaction::from(
            solana_sdk::transaction::Transaction::new_signed_with_payer(
                &[ix],
                Some(&h.payer.pubkey()),
                &[&h.payer],
                Hash::new_from_array(blockhash),
            ),
        );
        h.node.submit_transaction(tx).await.unwrap();
        h.node.try_produce_block().unwrap().expect("withdraw block");

        // Wrapped balance + supply dropped by the withdrawn amount.
        let wmint = w::wrapped_mint_address(&l1_mint);
        let payer_ata = w::associated_token_address(&h.payer.pubkey(), &wmint);
        let payer_tok = h.node.store().get_account(&payer_ata).unwrap().unwrap();
        assert_eq!(w::token_account_amount(&payer_tok.data), Some(700_000));
        let mint_acc = h.node.store().get_account(&wmint).unwrap().unwrap();
        assert_eq!(w::mint_supply(&mint_acc.data), Some(700_000));

        // The asset-bound withdrawal commitment is provable (claimable on L1).
        let (_root, _proof) = h
            .node
            .compute_withdrawal_proof(&l1_recipient, &l1_mint.to_bytes(), 300_000, nonce)
            .expect("asset-bound withdrawal proof");
        // A native-asset proof for the same (recipient, nonce) must NOT exist.
        assert!(h
            .node
            .compute_withdrawal_proof(&l1_recipient, &crate::bridge::NATIVE_ASSET_ID, 300_000, nonce)
            .is_err());
    }

    #[tokio::test]
    async fn submit_then_produce_block_and_query() {
        let h = setup();
        let recipient = Pubkey::new_unique();
        let (_, blockhash) = h.node.latest_blockhash();

        let tx = transfer(&h.payer, &recipient, ZUL, blockhash);
        let sig = h.node.submit_transaction(tx).await.unwrap();

        // No block yet (loop hasn't ticked).
        assert_eq!(h.node.current_slot(), 0);

        let slot = h.node.try_produce_block().unwrap().expect("block");
        assert_eq!(slot, 1);
        assert_eq!(h.node.current_slot(), 1);

        // The transaction is queryable with success status.
        let (loc, _, meta) = h.node.store().get_transaction(&sig).unwrap().unwrap();
        assert_eq!(loc.slot, 1);
        assert!(meta.status.is_ok());

        let recipient_account = h.node.store().get_account(&recipient).unwrap().unwrap();
        assert_eq!(recipient_account.lamports, ZUL);

        // New blockhash registered and valid; old genesis hash still valid.
        let (tip_slot, tip_hash) = h.node.latest_blockhash();
        assert_eq!(tip_slot, 1);
        assert!(h.node.is_blockhash_valid(&tip_hash));
        assert!(h.node.is_blockhash_valid(&blockhash));
    }

    #[tokio::test]
    async fn empty_mempool_produces_no_block_until_heartbeat() {
        let h = setup();
        assert_eq!(h.node.try_produce_block().unwrap(), None);
        assert_eq!(h.node.current_slot(), 0);
    }

    #[tokio::test]
    async fn l1_deposit_credits_recipient_exactly_once() {
        let h = setup();
        let recipient = Pubkey::new_unique();
        let deposit = DepositEvent {
            l1_signature: "depositSig1".into(),
            mint: None,
            decimals: 9,
            amount: 7 * ZUL,
            l2_recipient: recipient,
        };

        // A queued deposit produces a block (even with an empty mempool) and
        // credits the recipient; a receipt account marks it processed.
        h.node.enqueue_deposits(vec![deposit.clone()]);
        h.node.try_produce_block().unwrap().expect("block");
        assert_eq!(
            h.node.store().get_account(&recipient).unwrap().unwrap().lamports,
            7 * ZUL
        );
        let receipt = crate::bridge::deposit_receipt_address("depositSig1");
        assert!(h.node.store().get_account(&receipt).unwrap().is_some());

        // Re-enqueuing the same deposit must not double-credit.
        h.node.enqueue_deposits(vec![deposit]);
        h.node.try_produce_block().unwrap();
        assert_eq!(
            h.node.store().get_account(&recipient).unwrap().unwrap().lamports,
            7 * ZUL
        );
    }

    #[tokio::test]
    async fn withdrawal_burns_and_produces_a_claimable_proof() {
        use solana_sdk::instruction::{AccountMeta, Instruction};
        use solana_sdk::message::Message;

        let h = setup();
        let (_, blockhash) = h.node.latest_blockhash();
        let recipient = [9u8; 32];
        let (amount, nonce) = (3 * ZUL, 1u64);

        // A withdraw tx: single instruction to the bridge-withdraw program,
        // data = recipient(32) || amount(8) || nonce(8).
        let mut data = Vec::new();
        data.extend_from_slice(&recipient);
        data.extend_from_slice(&amount.to_le_bytes());
        data.extend_from_slice(&nonce.to_le_bytes());
        let ix = Instruction {
            program_id: crate::bridge::BRIDGE_WITHDRAW_PROGRAM_ID,
            accounts: vec![AccountMeta::new(h.payer.pubkey(), true)],
            data,
        };
        let tx = VersionedTransaction::from(solana_sdk::transaction::Transaction::new(
            &[&h.payer],
            Message::new(&[ix], Some(&h.payer.pubkey())),
            Hash::new_from_array(blockhash),
        ));
        h.node.submit_transaction(tx).await.unwrap();
        h.node.try_produce_block().unwrap().expect("block");

        // Withdrawer burned amount + fee; collector got the fee.
        let payer_bal = h.node.store().get_account(&h.payer.pubkey()).unwrap().unwrap().lamports;
        assert_eq!(payer_bal, 100 * ZUL - amount - pool::POOL_TX_FEE);
        assert_eq!(
            h.node.store().get_account(&h.fee_collector).unwrap().unwrap().lamports,
            pool::POOL_TX_FEE
        );

        // The node produces a settlement-valid inclusion proof for the
        // withdrawal commitment against the shallow withdrawals-tree root.
        let native = crate::bridge::NATIVE_ASSET_ID;
        let (root, proof) = h
            .node
            .compute_withdrawal_proof(&recipient, &native, amount, nonce)
            .unwrap();
        assert_eq!(root, h.node.store().withdrawals_root().unwrap());
        let leaf = crate::bridge::withdrawal_pubkey(&recipient, nonce);
        let account_hash = crate::bridge::withdrawal_account_hash(&recipient, &native, amount, nonce);
        assert!(crate::withdraw_smt::verify(&root, &leaf, &account_hash, &proof));

        // A different (amount, nonce) is absent — no false proof.
        assert!(h
            .node
            .compute_withdrawal_proof(&recipient, &native, amount, 999)
            .is_err());
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

        // Limit enforced.
        let too_much = h
            .node
            .request_airdrop(target, h.node.options.faucet_max_lamports + 1)
            .await;
        assert!(matches!(too_much, Err(BackendError::FaucetLimit)));
    }

    #[tokio::test]
    async fn stale_blockhash_rejected_at_submission() {
        let h = setup();
        let tx = transfer(&h.payer, &Pubkey::new_unique(), 1, [9u8; 32]);
        let err = h.node.submit_transaction(tx).await.unwrap_err();
        assert!(matches!(err, BackendError::Rejected(_)));
    }

    #[tokio::test]
    async fn simulation_does_not_commit_state() {
        let h = setup();
        let recipient = Pubkey::new_unique();
        let (_, blockhash) = h.node.latest_blockhash();
        let tx = transfer(&h.payer, &recipient, ZUL, blockhash);

        let result = h.node.simulate_transaction(tx).await.unwrap();
        assert!(result.err.is_none());
        assert!(result.units_consumed > 0);
        // Nothing persisted.
        assert_eq!(h.node.current_slot(), 0);
        assert!(h.node.store().get_account(&recipient).unwrap().is_none());
    }

    #[tokio::test]
    async fn shield_transaction_flows_through_block_production() {
        use zul_privacy::{
            field::fr_to_bytes,
            instruction::{Asset, PoolInstruction},
            poseidon::{hash1, note_secret},
            POOL_PROGRAM_ID, POOL_STATE_ADDRESS, POOL_VAULT_ADDRESS,
        };
        use solana_sdk::instruction::{AccountMeta, Instruction};
        use solana_sdk::message::Message;

        let h = setup();
        let (_, blockhash) = h.node.latest_blockhash();
        let amount = 5 * ZUL;

        // Build a shield instruction (no proof needed for shield); the node
        // binds the amount to the commitment from the opaque secret.
        let pk = hash1(ark_bn254::Fr::from(1u64));
        let secret = fr_to_bytes(&note_secret(pk, ark_bn254::Fr::from(123u64)));
        let ix_data = PoolInstruction::Shield {
            asset: Asset::Native,
            amount,
            secret,
            encrypted_note: vec![0xab; 32],
        }
        .to_bytes();
        let ix = Instruction {
            program_id: POOL_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(h.payer.pubkey(), true),
                AccountMeta::new(POOL_STATE_ADDRESS, false),
                AccountMeta::new(POOL_VAULT_ADDRESS, false),
            ],
            data: ix_data,
        };
        let message = Message::new(&[ix], Some(&h.payer.pubkey()));
        let tx = solana_sdk::transaction::Transaction::new(
            &[&h.payer],
            message,
            Hash::new_from_array(blockhash),
        );

        let sig = h
            .node
            .submit_transaction(VersionedTransaction::from(tx))
            .await
            .unwrap();
        h.node.try_produce_block().unwrap().expect("block");

        // The shield is recorded and succeeded.
        let (_, _, meta) = h.node.store().get_transaction(&sig).unwrap().unwrap();
        assert!(meta.status.is_ok(), "shield meta: {:?}", meta);

        // Vault funded, payer debited amount + fee.
        let vault = h.node.store().get_account(&POOL_VAULT_ADDRESS).unwrap().unwrap();
        assert_eq!(vault.lamports, amount);
        let payer = h.node.store().get_account(&h.payer.pubkey()).unwrap().unwrap();
        assert_eq!(
            payer.lamports,
            100 * ZUL - amount - pool::POOL_TX_FEE
        );

        // Pool state grew by one commitment.
        let state = zul_privacy::PoolState::from_bytes(
            &h.node.store().get_account(&POOL_STATE_ADDRESS).unwrap().unwrap().data,
        )
        .unwrap();
        assert_eq!(state.next_leaf_index, 1);
    }

    #[tokio::test]
    async fn regular_and_pool_txs_compose_in_one_block() {
        use zul_privacy::{
            field::fr_to_bytes,
            instruction::{Asset, PoolInstruction},
            poseidon::{hash1, note_secret},
            POOL_PROGRAM_ID, POOL_STATE_ADDRESS, POOL_VAULT_ADDRESS,
        };
        use solana_sdk::instruction::{AccountMeta, Instruction};
        use solana_sdk::message::Message;

        let h = setup();
        let (_, blockhash) = h.node.latest_blockhash();
        let recipient = Pubkey::new_unique();
        let shield_amount = 4 * ZUL;
        let transfer_amount = 3 * ZUL;

        // Regular SVM transfer payer -> recipient.
        let transfer = transfer(&h.payer, &recipient, transfer_amount, blockhash);
        h.node.submit_transaction(transfer).await.unwrap();

        // Pool shield from the same payer.
        let pk = hash1(ark_bn254::Fr::from(1u64));
        let secret = fr_to_bytes(&note_secret(pk, ark_bn254::Fr::from(99u64)));
        let shield = PoolInstruction::Shield {
            asset: Asset::Native,
            amount: shield_amount,
            secret,
            encrypted_note: vec![],
        }
        .to_bytes();
        let ix = Instruction {
            program_id: POOL_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(h.payer.pubkey(), true),
                AccountMeta::new(POOL_STATE_ADDRESS, false),
                AccountMeta::new(POOL_VAULT_ADDRESS, false),
            ],
            data: shield,
        };
        let shield_tx = solana_sdk::transaction::Transaction::new(
            &[&h.payer],
            Message::new(&[ix], Some(&h.payer.pubkey())),
            Hash::new_from_array(blockhash),
        );
        h.node
            .submit_transaction(VersionedTransaction::from(shield_tx))
            .await
            .unwrap();

        // One block holds both.
        h.node.try_produce_block().unwrap().expect("block");

        let fee = zul_primitives::constants::LAMPORTS_PER_SIGNATURE;
        // Payer paid: transfer amount + its SVM fee + shield amount + pool fee.
        let payer_bal = h.node.store().get_account(&h.payer.pubkey()).unwrap().unwrap().lamports;
        assert_eq!(
            payer_bal,
            100 * ZUL - transfer_amount - fee - shield_amount - pool::POOL_TX_FEE
        );
        // Regular recipient got the transfer.
        let recipient_bal = h.node.store().get_account(&recipient).unwrap().unwrap().lamports;
        assert_eq!(recipient_bal, transfer_amount);
        // Vault holds the shielded amount.
        let vault_bal = h.node.store().get_account(&POOL_VAULT_ADDRESS).unwrap().unwrap().lamports;
        assert_eq!(vault_bal, shield_amount);
        // Collector accumulated BOTH fees (SVM seed + pool fee).
        let collector_bal = h.node.store().get_account(&h.fee_collector).unwrap().unwrap().lamports;
        assert_eq!(collector_bal, fee + pool::POOL_TX_FEE);
    }

    #[tokio::test]
    async fn node_restarts_with_persisted_tip() {
        let dir = tempfile::tempdir().unwrap();
        let sequencer = Keypair::new();
        let faucet = Keypair::new();
        let genesis = GenesisConfig {
            chain_name: "zul-restart-test".into(),
            creation_time_ms: 1_760_000_000_000,
            sequencer: sequencer.pubkey(),
            fee_collector: Pubkey::new_unique(),
            allocations: vec![Allocation {
                pubkey: faucet.pubkey(),
                lamports: 1_000 * ZUL,
            }],
        };

        let target = Pubkey::new_unique();
        {
            let store = Arc::new(Store::open(&dir.path().join("zul.redb")).unwrap());
            let node = Node::new(
                store,
                &genesis,
                sequencer.insecure_clone(),
                vec![],
                NodeOptions {
                    faucet: Some(faucet.insecure_clone()),
                    ..Default::default()
                },
            )
            .unwrap();
            node.request_airdrop(target, ZUL).await.unwrap();
            node.try_produce_block().unwrap().expect("block");
            assert_eq!(node.current_slot(), 1);
        }

        // Reopen: tip, queue, and state survive.
        let store = Arc::new(Store::open(&dir.path().join("zul.redb")).unwrap());
        let node = Node::new(store, &genesis, sequencer, vec![], NodeOptions::default()).unwrap();
        assert_eq!(node.current_slot(), 1);
        assert_eq!(
            node.store().get_account(&target).unwrap().unwrap().lamports,
            ZUL
        );
        let (_, tip_hash) = node.latest_blockhash();
        assert!(node.is_blockhash_valid(&tip_hash));
    }
}
