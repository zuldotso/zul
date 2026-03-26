//! SVM execution layer: wraps Agave's `solana-svm` transaction batch
//! processor over the ZUL store.
//!
//! Responsibilities per block: sanitize + signature-verify transactions,
//! enforce blockhash recency, build fee/compute budgets, execute the batch,
//! collect final account states (including fee crediting to the collector),
//! and report per-transaction metadata for the block body.

pub mod builtins;
pub mod callback;
pub mod reserved;
pub mod sysvars;

use zul_primitives::constants::LAMPORTS_PER_SIGNATURE;
use zul_primitives::meta::ExecutedTransactionMeta;
use zul_store::{BlockhashQueue, Store, StoredAccount};
use solana_compute_budget_instruction::instructions_processor::process_compute_budget_instructions;
use solana_fee_structure::FeeDetails;
use solana_program_runtime::execution_budget::SVMTransactionExecutionBudget;
use solana_program_runtime::loaded_programs::{BlockRelation, ForkGraph};
use solana_sdk::{
    account::{AccountSharedData, ReadableAccount, WritableAccount},
    clock::Slot,
    hash::Hash,
    pubkey::Pubkey,
    signature::Signature,
    transaction::{SanitizedTransaction, TransactionError, VersionedTransaction},
};
use solana_message::SimpleAddressLoader;
use solana_svm::account_loader::{CheckedTransactionDetails, TransactionCheckResult};
use solana_svm::transaction_processing_result::ProcessedTransaction;
use solana_svm::transaction_processor::{
    ExecutionRecordingConfig, TransactionBatchProcessor, TransactionProcessingConfig,
    TransactionProcessingEnvironment,
};
use solana_svm_feature_set::SVMFeatureSet;
use solana_svm_transaction::svm_message::SVMMessage;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use thiserror::Error;

use crate::builtins::BuiltinSpec;
use crate::callback::StoreCallback;

#[derive(Debug, Error)]
pub enum ExecutorError {
    #[error("store error: {0}")]
    Store(#[from] zul_store::StoreError),
    #[error("svm setup error: {0}")]
    Setup(String),
}

/// Single-sequencer fork graph: slots form one line, no forks.
pub struct SequencerForkGraph;

impl ForkGraph for SequencerForkGraph {
    fn relationship(&self, a: Slot, b: Slot) -> BlockRelation {
        match a.cmp(&b) {
            std::cmp::Ordering::Less => BlockRelation::Ancestor,
            std::cmp::Ordering::Equal => BlockRelation::Equal,
            std::cmp::Ordering::Greater => BlockRelation::Descendant,
        }
    }
}

pub struct BlockExecutionInput<'a> {
    pub slot: u64,
    pub timestamp_ms: u64,
    /// Hash recorded for durable nonce advancement (the parent blockhash).
    pub parent_blockhash: Hash,
    pub transactions: Vec<VersionedTransaction>,
    pub queue: &'a BlockhashQueue,
}

pub struct BlockExecutionOutput {
    /// Transactions included in the block, in order, with their metadata.
    pub included: Vec<(VersionedTransaction, ExecutedTransactionMeta)>,
    /// Transactions rejected before/at execution (not part of the block).
    pub dropped: Vec<(Signature, TransactionError)>,
    /// Final account writes for the store commit (None = deletion).
    pub account_writes: Vec<(Pubkey, Option<StoredAccount>)>,
    /// Total fees credited to the collector this block.
    pub fees_collected: u64,
}

pub struct Executor {
    /// Kept alive so the processor's weak reference stays valid.
    _fork_graph: Arc<RwLock<SequencerForkGraph>>,
    base_processor: TransactionBatchProcessor<SequencerForkGraph>,
    feature_set: SVMFeatureSet,
    /// Full feature set view for components that take the agave type.
    agave_features: agave_feature_set::FeatureSet,
    fee_collector: Pubkey,
    builtin_specs: Vec<BuiltinSpec>,
    reserved_keys: HashSet<Pubkey>,
}

impl Executor {
    /// `extra_builtins` lets the node register enshrined programs (the
    /// privacy pool) alongside the defaults.
    pub fn new(fee_collector: Pubkey, extra_builtins: Vec<BuiltinSpec>) -> Result<Self, ExecutorError> {
        let feature_set = SVMFeatureSet::all_enabled();
        let agave_features = agave_feature_set::FeatureSet::all_enabled();
        let fork_graph = Arc::new(RwLock::new(SequencerForkGraph));

        let environment = agave_syscalls::create_program_runtime_environment_v1(
            &feature_set,
            &SVMTransactionExecutionBudget::new_with_defaults(
                feature_set.raise_cpi_nesting_limit_to_8,
            ),
            false, // reject_deployment_of_broken_elfs: applied at deploy time
            false, // debugging_features
        )
        .map_err(|e| ExecutorError::Setup(format!("program runtime environment: {e}")))?;

        // `TransactionBatchProcessor::new` is dev-gated; replicate it.
        let mut base_processor =
            TransactionBatchProcessor::<SequencerForkGraph>::new_uninitialized(0, 0);
        base_processor
            .global_program_cache
            .write()
            .unwrap()
            .set_fork_graph(Arc::downgrade(&fork_graph));
        base_processor.environments.program_runtime_v1 = Arc::new(environment);

        let mut builtin_specs = builtins::default_builtins();
        builtin_specs.extend(extra_builtins);
        for spec in &builtin_specs {
            base_processor.add_builtin(spec.program_id, spec.cache_entry());
        }

        Ok(Self {
            _fork_graph: fork_graph,
            base_processor,
            feature_set,
            agave_features,
            fee_collector,
            builtin_specs,
            reserved_keys: reserved::reserved_account_keys(),
        })
    }

    /// Accounts that must exist in genesis state: builtin program accounts,
    /// static sysvars, and preloaded BPF programs (SPL Token, Token-2022,
    /// Memo, ATA, plus core BPF programs like address-lookup-table).
    pub fn genesis_accounts(&self) -> Vec<(Pubkey, StoredAccount)> {
        let mut out: Vec<(Pubkey, StoredAccount)> = Vec::new();
        for spec in &self.builtin_specs {
            let account = builtins::builtin_account(spec.name);
            out.push((spec.program_id, StoredAccount::from_shared(&account)));
        }
        for (pubkey, account) in sysvars::static_sysvar_accounts() {
            out.push((pubkey, StoredAccount::from_shared(&account)));
        }
        let rent = solana_rent::Rent::default();
        for (pubkey, account) in solana_program_binaries::spl_programs(&rent) {
            out.push((pubkey, StoredAccount::from_shared(&account)));
        }
        for (pubkey, account) in
            solana_program_binaries::core_bpf_programs(&rent, |_feature| true)
        {
            out.push((pubkey, StoredAccount::from_shared(&account)));
        }
        out
    }

    /// Execute one block's worth of transactions against committed state.
    /// Nothing is persisted: the caller commits `account_writes` together
    /// with the sealed block.
    pub fn execute_block(
        &self,
        store: &Store,
        input: BlockExecutionInput<'_>,
    ) -> Result<BlockExecutionOutput, ExecutorError> {
        let callback = StoreCallback::new(store);

        // Refresh the clock for this slot; committed with the block.
        let (clock_key, clock_account) = sysvars::clock_account(input.slot, input.timestamp_ms);
        callback.set_account(clock_key, clock_account);

        // --- sanitize, verify, and budget ---------------------------------
        let mut dropped: Vec<(Signature, TransactionError)> = Vec::new();
        let mut sanitized_batch: Vec<SanitizedTransaction> = Vec::new();
        let mut check_results: Vec<TransactionCheckResult> = Vec::new();
        let mut originals: Vec<VersionedTransaction> = Vec::new();
        let mut seen_signatures: HashSet<Signature> = HashSet::new();

        for tx in input.transactions {
            let Some(signature) = tx.signatures.first().copied() else {
                continue; // unsigned garbage carries no identity to report
            };
            if !seen_signatures.insert(signature) {
                dropped.push((signature, TransactionError::AlreadyProcessed));
                continue;
            }
            if store.get_transaction(&signature)?.is_some() {
                dropped.push((signature, TransactionError::AlreadyProcessed));
                continue;
            }
            let message_hash = match tx.verify_and_hash_message() {
                Ok(hash) => hash,
                Err(_) => {
                    dropped.push((signature, TransactionError::SignatureFailure));
                    continue;
                }
            };
            if !input.queue.is_valid_hash(tx.message.recent_blockhash()) {
                dropped.push((signature, TransactionError::BlockhashNotFound));
                continue;
            }
            let sanitized = match SanitizedTransaction::try_create(
                tx.clone(),
                message_hash,
                Some(false),
                SimpleAddressLoader::Disabled,
                &self.reserved_keys,
            ) {
                Ok(s) => s,
                Err(e) => {
                    dropped.push((signature, e));
                    continue;
                }
            };
            let budget_limits = match process_compute_budget_instructions(
                SVMMessage::program_instructions_iter(&sanitized),
                &self.agave_features,
            ) {
                Ok(limits) => limits,
                Err(e) => {
                    dropped.push((signature, e));
                    continue;
                }
            };
            let signature_count = sanitized
                .num_transaction_signatures()
                .saturating_add(sanitized.num_ed25519_signatures())
                .saturating_add(sanitized.num_secp256k1_signatures())
                .saturating_add(sanitized.num_secp256r1_signatures());
            let fee_details = FeeDetails::new(
                signature_count.saturating_mul(LAMPORTS_PER_SIGNATURE),
                budget_limits.get_prioritization_fee(),
            );
            let budget = budget_limits.get_compute_budget_and_limits(
                budget_limits.loaded_accounts_bytes,
                fee_details,
                self.feature_set.raise_cpi_nesting_limit_to_8,
            );
            check_results.push(Ok(CheckedTransactionDetails::new(None, budget)));
            sanitized_batch.push(sanitized);
            originals.push(tx);
        }

        // --- execute -------------------------------------------------------
        let epoch = sysvars::epoch_for_slot(input.slot);
        let processor = self.base_processor.new_from(input.slot, epoch);
        processor.fill_missing_sysvar_cache_entries(&callback);

        let environment = TransactionProcessingEnvironment {
            blockhash: input.parent_blockhash,
            blockhash_lamports_per_signature: LAMPORTS_PER_SIGNATURE,
            feature_set: self.feature_set.clone(),
            program_runtime_environments_for_execution: processor
                .get_environments_for_epoch(epoch),
            program_runtime_environments_for_deployment: processor
                .get_environments_for_epoch(epoch),
            ..TransactionProcessingEnvironment::default()
        };
        let config = TransactionProcessingConfig {
            recording_config: ExecutionRecordingConfig {
                enable_log_recording: true,
                enable_return_data_recording: false,
                enable_cpi_recording: false,
                enable_transaction_balance_recording: false,
            },
            ..Default::default()
        };

        let output = processor.load_and_execute_sanitized_transactions(
            &callback,
            &sanitized_batch,
            check_results,
            &environment,
            &config,
        );

        // --- collect results ------------------------------------------------
        // Running lamport view for pre/post balances across the batch.
        let mut final_accounts: HashMap<Pubkey, AccountSharedData> = HashMap::new();
        let lamports_of = |final_accounts: &HashMap<Pubkey, AccountSharedData>,
                           callback: &StoreCallback,
                           key: &Pubkey| {
            final_accounts
                .get(key)
                .map(|a| a.lamports())
                .unwrap_or_else(|| callback.lamports(key))
        };

        let mut included: Vec<(VersionedTransaction, ExecutedTransactionMeta)> = Vec::new();
        let mut fees_collected: u64 = 0;

        for ((result, sanitized), original) in output
            .processing_results
            .into_iter()
            .zip(&sanitized_batch)
            .zip(originals)
        {
            let signature = *sanitized.signature();
            let account_keys: Vec<Pubkey> = sanitized
                .message()
                .account_keys()
                .iter()
                .copied()
                .collect();
            let pre_balances: Vec<u64> = account_keys
                .iter()
                .map(|k| lamports_of(&final_accounts, &callback, k))
                .collect();

            match result {
                Err(e) => {
                    dropped.push((signature, e));
                }
                Ok(ProcessedTransaction::FeesOnly(fees_only)) => {
                    let fee = fees_only.fee_details.total_fee();
                    fees_collected = fees_collected.saturating_add(fee);
                    for (pubkey, account) in fees_only.rollback_accounts.iter() {
                        final_accounts.insert(*pubkey, account.clone());
                    }
                    let post_balances: Vec<u64> = account_keys
                        .iter()
                        .map(|k| lamports_of(&final_accounts, &callback, k))
                        .collect();
                    let meta = ExecutedTransactionMeta {
                        status: Err(fees_only.load_error.clone()),
                        fee,
                        compute_units_consumed: 0,
                        log_messages: Vec::new(),
                        pre_balances,
                        post_balances,
                    };
                    included.push((original, meta));
                }
                Ok(ProcessedTransaction::Executed(executed)) => {
                    let fee = executed.loaded_transaction.fee_details.total_fee();
                    fees_collected = fees_collected.saturating_add(fee);
                    for (index, (pubkey, account)) in
                        executed.loaded_transaction.accounts.iter().enumerate()
                    {
                        if sanitized.is_writable(index) {
                            final_accounts.insert(*pubkey, account.clone());
                        }
                    }
                    let post_balances: Vec<u64> = account_keys
                        .iter()
                        .map(|k| lamports_of(&final_accounts, &callback, k))
                        .collect();
                    let details = executed.execution_details;
                    let meta = ExecutedTransactionMeta {
                        status: details.status.clone(),
                        fee,
                        compute_units_consumed: details.executed_units,
                        log_messages: details.log_messages.unwrap_or_default(),
                        pre_balances,
                        post_balances,
                    };
                    included.push((original, meta));
                }
            }
        }

        // Credit fees to the collector.
        if fees_collected > 0 {
            let mut collector = final_accounts
                .get(&self.fee_collector)
                .cloned()
                .or_else(|| callback.get(&self.fee_collector))
                .unwrap_or_else(|| StoredAccount::new_system(0).to_shared());
            collector.set_lamports(collector.lamports().saturating_add(fees_collected));
            final_accounts.insert(self.fee_collector, collector);
        }

        // Merge: per-block overlay (clock) first, then execution results.
        let mut writes: HashMap<Pubkey, Option<StoredAccount>> = HashMap::new();
        for (pubkey, account) in callback.take_overlay_writes() {
            writes.insert(pubkey, account);
        }
        for (pubkey, account) in final_accounts {
            if account.lamports() == 0 {
                writes.insert(pubkey, None);
            } else {
                writes.insert(pubkey, Some(StoredAccount::from_shared(&account)));
            }
        }

        Ok(BlockExecutionOutput {
            included,
            dropped,
            account_writes: writes.into_iter().collect(),
            fees_collected,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zul_primitives::genesis::{Allocation, GenesisConfig};
    use solana_sdk::{
        signature::Keypair, signer::Signer, transaction::Transaction,
    };
    use solana_system_interface::instruction as system_instruction;

    const ZUL: u64 = 1_000_000_000;

    struct Harness {
        _dir: tempfile::TempDir,
        store: Store,
        executor: Executor,
        queue: BlockhashQueue,
        genesis_hash: zul_primitives::hash::H256,
        payer: Keypair,
        fee_collector: Pubkey,
    }

    fn setup() -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("zul.redb")).unwrap();
        let payer = Keypair::new();
        let fee_collector = Pubkey::new_unique();
        let genesis = GenesisConfig {
            chain_name: "zul-executor-test".into(),
            creation_time_ms: 1_760_000_000_000,
            sequencer: Pubkey::new_unique(),
            fee_collector,
            allocations: vec![Allocation {
                pubkey: payer.pubkey(),
                lamports: 100 * ZUL,
            }],
        };
        let executor = Executor::new(fee_collector, vec![]).unwrap();
        store
            .initialize_genesis(&genesis, &executor.genesis_accounts())
            .unwrap();
        let genesis_hash = genesis.genesis_hash();
        let mut queue = BlockhashQueue::new();
        queue.register(0, genesis_hash);
        Harness {
            _dir: dir,
            store,
            executor,
            queue,
            genesis_hash,
            payer,
            fee_collector,
        }
    }

    fn transfer_tx(from: &Keypair, to: &Pubkey, lamports: u64, blockhash: Hash) -> VersionedTransaction {
        let ix = system_instruction::transfer(&from.pubkey(), to, lamports);
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&from.pubkey()),
            &[from],
            blockhash,
        );
        VersionedTransaction::from(tx)
    }

    fn write_of<'a>(
        writes: &'a [(Pubkey, Option<StoredAccount>)],
        key: &Pubkey,
    ) -> Option<&'a StoredAccount> {
        writes
            .iter()
            .find(|(pk, _)| pk == key)
            .and_then(|(_, acc)| acc.as_ref())
    }

    #[test]
    fn system_transfer_executes_and_pays_fees() {
        let h = setup();
        let recipient = Pubkey::new_unique();
        let blockhash = Hash::new_from_array(h.genesis_hash);
        let tx = transfer_tx(&h.payer, &recipient, ZUL, blockhash);

        let out = h
            .executor
            .execute_block(
                &h.store,
                BlockExecutionInput {
                    slot: 1,
                    timestamp_ms: 1_760_000_000_500,
                    parent_blockhash: blockhash,
                    transactions: vec![tx],
                    queue: &h.queue,
                },
            )
            .unwrap();

        assert_eq!(out.dropped.len(), 0, "dropped: {:?}", out.dropped);
        assert_eq!(out.included.len(), 1);
        let (_, meta) = &out.included[0];
        assert_eq!(meta.status, Ok(()));
        assert_eq!(meta.fee, LAMPORTS_PER_SIGNATURE);
        assert!(meta.compute_units_consumed > 0);

        let payer_after = write_of(&out.account_writes, &h.payer.pubkey()).unwrap();
        assert_eq!(payer_after.lamports, 100 * ZUL - ZUL - LAMPORTS_PER_SIGNATURE);
        let recipient_after = write_of(&out.account_writes, &recipient).unwrap();
        assert_eq!(recipient_after.lamports, ZUL);
        let collector_after = write_of(&out.account_writes, &h.fee_collector).unwrap();
        assert_eq!(collector_after.lamports, LAMPORTS_PER_SIGNATURE);
        assert_eq!(out.fees_collected, LAMPORTS_PER_SIGNATURE);

        // Balances bookkeeping: payer is index 0, recipient index 1.
        assert_eq!(meta.pre_balances[0], 100 * ZUL);
        assert_eq!(meta.post_balances[0], 100 * ZUL - ZUL - LAMPORTS_PER_SIGNATURE);
        assert_eq!(meta.pre_balances[1], 0);
        assert_eq!(meta.post_balances[1], ZUL);
    }

    #[test]
    fn failed_transfer_is_included_and_charged() {
        let h = setup();
        let recipient = Pubkey::new_unique();
        let blockhash = Hash::new_from_array(h.genesis_hash);
        // More than the payer holds: executes and fails inside the system
        // program, but the fee is still charged.
        let tx = transfer_tx(&h.payer, &recipient, 1_000 * ZUL, blockhash);

        let out = h
            .executor
            .execute_block(
                &h.store,
                BlockExecutionInput {
                    slot: 1,
                    timestamp_ms: 1_760_000_000_500,
                    parent_blockhash: blockhash,
                    transactions: vec![tx],
                    queue: &h.queue,
                },
            )
            .unwrap();

        assert_eq!(out.included.len(), 1);
        let (_, meta) = &out.included[0];
        assert!(meta.status.is_err());
        assert_eq!(meta.fee, LAMPORTS_PER_SIGNATURE);

        let payer_after = write_of(&out.account_writes, &h.payer.pubkey()).unwrap();
        assert_eq!(payer_after.lamports, 100 * ZUL - LAMPORTS_PER_SIGNATURE);
        assert!(write_of(&out.account_writes, &recipient).is_none());
    }

    #[test]
    fn stale_blockhash_and_bad_signature_are_dropped() {
        let h = setup();
        let recipient = Pubkey::new_unique();

        // Stale blockhash.
        let stale = transfer_tx(&h.payer, &recipient, 1, Hash::new_from_array([9u8; 32]));
        let stale_sig = stale.signatures[0];

        // Corrupted signature.
        let blockhash = Hash::new_from_array(h.genesis_hash);
        let mut forged = transfer_tx(&h.payer, &recipient, 1, blockhash);
        forged.signatures[0] = Signature::from([42u8; 64]);
        let forged_sig = forged.signatures[0];

        let out = h
            .executor
            .execute_block(
                &h.store,
                BlockExecutionInput {
                    slot: 1,
                    timestamp_ms: 1_760_000_000_500,
                    parent_blockhash: blockhash,
                    transactions: vec![stale, forged],
                    queue: &h.queue,
                },
            )
            .unwrap();

        assert_eq!(out.included.len(), 0);
        assert_eq!(out.dropped.len(), 2);
        assert!(out
            .dropped
            .iter()
            .any(|(s, e)| *s == stale_sig && *e == TransactionError::BlockhashNotFound));
        assert!(out
            .dropped
            .iter()
            .any(|(s, e)| *s == forged_sig && *e == TransactionError::SignatureFailure));
    }

    #[test]
    fn spl_token_mint_creation_runs_on_preloaded_bpf_program() {
        let h = setup();
        let blockhash = Hash::new_from_array(h.genesis_hash);
        let token_program = spl_generic_token::token::ID;
        let mint = Keypair::new();

        const MINT_SPACE: u64 = 82;
        let rent_exempt = solana_rent::Rent::default().minimum_balance(MINT_SPACE as usize);
        let create_ix = system_instruction::create_account(
            &h.payer.pubkey(),
            &mint.pubkey(),
            rent_exempt,
            MINT_SPACE,
            &token_program,
        );

        // InitializeMint2: tag 20, decimals, mint_authority, no freeze auth.
        let mut data = vec![20u8, 9u8];
        data.extend_from_slice(h.payer.pubkey().as_ref());
        data.push(0);
        let init_ix = solana_sdk::instruction::Instruction {
            program_id: token_program,
            accounts: vec![solana_sdk::instruction::AccountMeta::new(
                mint.pubkey(),
                false,
            )],
            data,
        };

        let tx = Transaction::new_signed_with_payer(
            &[create_ix, init_ix],
            Some(&h.payer.pubkey()),
            &[&h.payer, &mint],
            blockhash,
        );

        let out = h
            .executor
            .execute_block(
                &h.store,
                BlockExecutionInput {
                    slot: 1,
                    timestamp_ms: 1_760_000_000_500,
                    parent_blockhash: blockhash,
                    transactions: vec![VersionedTransaction::from(tx)],
                    queue: &h.queue,
                },
            )
            .unwrap();

        assert_eq!(out.dropped.len(), 0, "dropped: {:?}", out.dropped);
        let (_, meta) = &out.included[0];
        assert_eq!(meta.status, Ok(()), "logs: {:?}", meta.log_messages);

        let mint_account = write_of(&out.account_writes, &mint.pubkey()).unwrap();
        assert_eq!(mint_account.owner, token_program);
        assert_eq!(mint_account.data.len(), MINT_SPACE as usize);
        // is_initialized flag sits at offset 45 in the Mint layout.
        assert_eq!(mint_account.data[45], 1);
    }

    #[test]
    fn sequential_transfers_in_one_batch_chain_balances() {
        let h = setup();
        let middle = Keypair::new();
        let end = Pubkey::new_unique();
        let blockhash = Hash::new_from_array(h.genesis_hash);

        // payer -> middle (2 ZUL), then middle -> end (1 ZUL): the second
        // transfer only works if SIMD-83 style intra-batch state is live.
        let tx1 = transfer_tx(&h.payer, &middle.pubkey(), 2 * ZUL, blockhash);
        let tx2 = transfer_tx(&middle, &end, ZUL, blockhash);

        let out = h
            .executor
            .execute_block(
                &h.store,
                BlockExecutionInput {
                    slot: 1,
                    timestamp_ms: 1_760_000_000_500,
                    parent_blockhash: blockhash,
                    transactions: vec![tx1, tx2],
                    queue: &h.queue,
                },
            )
            .unwrap();

        assert_eq!(out.dropped.len(), 0, "dropped: {:?}", out.dropped);
        assert_eq!(out.included.len(), 2);
        assert!(out.included.iter().all(|(_, m)| m.status.is_ok()));

        let end_account = write_of(&out.account_writes, &end).unwrap();
        assert_eq!(end_account.lamports, ZUL);
        let middle_account = write_of(&out.account_writes, &middle.pubkey()).unwrap();
        assert_eq!(middle_account.lamports, ZUL - LAMPORTS_PER_SIGNATURE);
    }
}
