//! SVM execution layer: wraps Agave's `solana-svm` transaction batch
//! processor over the ZUL store.
//!
//! Spike scope: stand up a `TransactionBatchProcessor`, register the system
//! program + BPF loaders as builtins, and execute a sanitized batch against
//! the account store. Builtins, sysvars, and fee handling get pulled into
//! their own modules as this grows.

pub mod callback;
pub mod reserved;

use zul_primitives::constants::LAMPORTS_PER_SIGNATURE;
use zul_primitives::meta::ExecutedTransactionMeta;
use zul_store::{BlockhashQueue, Store, StoredAccount};
use solana_compute_budget_instruction::instructions_processor::process_compute_budget_instructions;
use solana_fee_structure::FeeDetails;
use solana_program_runtime::execution_budget::SVMTransactionExecutionBudget;
use solana_program_runtime::invoke_context::BuiltinFunctionWithContext;
use solana_program_runtime::loaded_programs::{BlockRelation, ForkGraph, ProgramCacheEntry};
use solana_sdk::{
    account::{Account, AccountSharedData, ReadableAccount},
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

struct Builtin {
    program_id: Pubkey,
    name: &'static str,
    entrypoint: BuiltinFunctionWithContext,
}

fn default_builtins() -> Vec<Builtin> {
    vec![
        Builtin {
            program_id: solana_sdk_ids::system_program::id(),
            name: "system_program",
            entrypoint: solana_system_program::system_processor::Entrypoint::vm,
        },
        Builtin {
            program_id: solana_sdk_ids::bpf_loader_upgradeable::id(),
            name: "solana_bpf_loader_upgradeable_program",
            entrypoint: solana_bpf_loader_program::Entrypoint::vm,
        },
        Builtin {
            program_id: solana_sdk_ids::bpf_loader::id(),
            name: "solana_bpf_loader_program",
            entrypoint: solana_bpf_loader_program::Entrypoint::vm,
        },
        Builtin {
            program_id: solana_sdk_ids::compute_budget::id(),
            name: "compute_budget_program",
            entrypoint: solana_compute_budget_program::Entrypoint::vm,
        },
    ]
}

pub struct BlockExecutionInput<'a> {
    pub slot: u64,
    pub timestamp_ms: u64,
    pub parent_blockhash: Hash,
    pub transactions: Vec<VersionedTransaction>,
    pub queue: &'a BlockhashQueue,
}

pub struct BlockExecutionOutput {
    pub included: Vec<(VersionedTransaction, ExecutedTransactionMeta)>,
    pub dropped: Vec<(Signature, TransactionError)>,
    pub account_writes: Vec<(Pubkey, Option<StoredAccount>)>,
}

pub struct Executor {
    _fork_graph: Arc<RwLock<SequencerForkGraph>>,
    base_processor: TransactionBatchProcessor<SequencerForkGraph>,
    feature_set: SVMFeatureSet,
    agave_features: agave_feature_set::FeatureSet,
    builtins: Vec<Builtin>,
    reserved_keys: HashSet<Pubkey>,
}

impl Executor {
    pub fn new() -> Result<Self, ExecutorError> {
        let feature_set = SVMFeatureSet::all_enabled();
        let agave_features = agave_feature_set::FeatureSet::all_enabled();
        let fork_graph = Arc::new(RwLock::new(SequencerForkGraph));

        let environment = agave_syscalls::create_program_runtime_environment_v1(
            &feature_set,
            &SVMTransactionExecutionBudget::new_with_defaults(
                feature_set.raise_cpi_nesting_limit_to_8,
            ),
            false,
            false,
        )
        .map_err(|e| ExecutorError::Setup(format!("program runtime environment: {e}")))?;

        let mut base_processor =
            TransactionBatchProcessor::<SequencerForkGraph>::new_uninitialized(0, 0);
        base_processor
            .global_program_cache
            .write()
            .unwrap()
            .set_fork_graph(Arc::downgrade(&fork_graph));
        base_processor.environments.program_runtime_v1 = Arc::new(environment);

        let builtins = default_builtins();
        for b in &builtins {
            base_processor.add_builtin(
                b.program_id,
                ProgramCacheEntry::new_builtin(0, b.name.len(), b.entrypoint),
            );
        }

        Ok(Self {
            _fork_graph: fork_graph,
            base_processor,
            feature_set,
            agave_features,
            builtins,
            reserved_keys: reserved::reserved_account_keys(),
        })
    }

    /// Accounts that must exist in genesis: the builtin program accounts and
    /// the preloaded SPL programs. TODO: move sysvars into their own module.
    pub fn genesis_accounts(&self) -> Vec<(Pubkey, StoredAccount)> {
        let mut out: Vec<(Pubkey, StoredAccount)> = Vec::new();
        for b in &self.builtins {
            let account = AccountSharedData::from(Account {
                lamports: 1,
                data: b.name.as_bytes().to_vec(),
                owner: solana_sdk_ids::native_loader::id(),
                executable: true,
                rent_epoch: zul_store::RENT_EXEMPT_RENT_EPOCH,
            });
            out.push((b.program_id, StoredAccount::from_shared(&account)));
        }
        let rent = solana_rent::Rent::default();
        for (pubkey, account) in solana_program_binaries::spl_programs(&rent) {
            out.push((pubkey, StoredAccount::from_shared(&account)));
        }
        out
    }

    /// Build the Clock sysvar account for this slot. Lives here until the
    /// sysvar maintenance grows into its own module.
    fn clock_account(slot: u64, timestamp_ms: u64) -> (Pubkey, AccountSharedData) {
        let clock = solana_clock::Clock {
            slot,
            epoch_start_timestamp: (timestamp_ms / 1000) as i64,
            epoch: 0,
            leader_schedule_epoch: 0,
            unix_timestamp: (timestamp_ms / 1000) as i64,
        };
        let account = solana_sysvar::clock::create_account(&clock, 1);
        (solana_sdk_ids::sysvar::clock::id(), account)
    }

    /// Execute one block's transactions against committed state. Nothing is
    /// persisted; the caller commits `account_writes` with the sealed block.
    pub fn execute_block(
        &self,
        store: &Store,
        input: BlockExecutionInput<'_>,
    ) -> Result<BlockExecutionOutput, ExecutorError> {
        let callback = StoreCallback::new(store);
        let (clock_key, clock_account) = Self::clock_account(input.slot, input.timestamp_ms);
        callback.set_account(clock_key, clock_account);

        let mut dropped: Vec<(Signature, TransactionError)> = Vec::new();
        let mut sanitized_batch: Vec<SanitizedTransaction> = Vec::new();
        let mut check_results: Vec<TransactionCheckResult> = Vec::new();
        let mut originals: Vec<VersionedTransaction> = Vec::new();

        for tx in input.transactions {
            let Some(signature) = tx.signatures.first().copied() else {
                continue;
            };
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
            let budget_limits = process_compute_budget_instructions(
                SVMMessage::program_instructions_iter(&sanitized),
                &self.agave_features,
            )
            .map_err(|e| ExecutorError::Setup(format!("compute budget: {e}")))?;
            let fee_details = FeeDetails::new(LAMPORTS_PER_SIGNATURE, 0);
            let budget = budget_limits.get_compute_budget_and_limits(
                budget_limits.loaded_accounts_bytes,
                fee_details,
                self.feature_set.raise_cpi_nesting_limit_to_8,
            );
            check_results.push(Ok(CheckedTransactionDetails::new(None, budget)));
            sanitized_batch.push(sanitized);
            originals.push(tx);
        }

        let epoch = 0;
        let processor = self.base_processor.new_from(input.slot, epoch);
        processor.fill_missing_sysvar_cache_entries(&callback);

        let environment = TransactionProcessingEnvironment {
            blockhash: input.parent_blockhash,
            blockhash_lamports_per_signature: LAMPORTS_PER_SIGNATURE,
            feature_set: self.feature_set.clone(),
            program_runtime_environments_for_execution: processor.get_environments_for_epoch(epoch),
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

        let mut final_accounts: HashMap<Pubkey, AccountSharedData> = HashMap::new();
        let mut included: Vec<(VersionedTransaction, ExecutedTransactionMeta)> = Vec::new();

        for ((result, sanitized), original) in output
            .processing_results
            .into_iter()
            .zip(&sanitized_batch)
            .zip(originals)
        {
            let signature = *sanitized.signature();
            match result {
                Err(e) => dropped.push((signature, e)),
                // TODO: fees-only (load failures still pay) once fee handling lands.
                Ok(ProcessedTransaction::FeesOnly(_)) => {}
                Ok(ProcessedTransaction::Executed(executed)) => {
                    for (index, (pubkey, account)) in
                        executed.loaded_transaction.accounts.iter().enumerate()
                    {
                        if sanitized.is_writable(index) {
                            final_accounts.insert(*pubkey, account.clone());
                        }
                    }
                    let details = executed.execution_details;
                    let meta = ExecutedTransactionMeta {
                        status: details.status.clone(),
                        fee: executed.loaded_transaction.fee_details.total_fee(),
                        compute_units_consumed: details.executed_units,
                        log_messages: details.log_messages.unwrap_or_default(),
                        pre_balances: vec![],
                        post_balances: vec![],
                    };
                    included.push((original, meta));
                }
            }
        }

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
        })
    }
}
