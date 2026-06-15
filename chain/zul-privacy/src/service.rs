//! Node-integrated shielded pool.
//!
//! The pool is enshrined in the node: pool transactions (a single
//! instruction to `POOL_PROGRAM_ID`) are processed here rather than inside
//! the SVM, so verification runs natively with no compute-unit ceiling. The
//! service loads the on-chain `PoolState` account, checks nullifiers against
//! the store, runs the `PoolProcessor` state machine, and returns the
//! resulting account writes + spent nullifiers for the node to commit
//! atomically with the block.
//!
//! v1 applies the native (ZUL) value path directly via lamport moves. The
//! SPL (wSOL) path is recognized and validated but its vault token movement
//! is a documented runtime-integration item (needs a token CPI); such
//! instructions are rejected here rather than half-applied.

use crate::instruction::{Asset, PoolInstruction};
use crate::processor::{NoteEvent, NullifierSet, PoolEffect, PoolProcessor, ProofVerifier};
use crate::state::PoolState;
use zul_primitives::constants::LAMPORTS_PER_SIGNATURE;
use zul_primitives::wrapped_spl as w;
use zul_store::{StoredAccount, Store, RENT_EXEMPT_RENT_EPOCH};
use solana_sdk::pubkey::Pubkey;
use std::collections::{HashMap, HashSet};

/// Addresses the pool operates over.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Account holding the serialized `PoolState`.
    pub pool_state: Pubkey,
    /// Native vault holding shielded ZUL lamports.
    pub vault: Pubkey,
    /// Where pool transaction fees accrue.
    pub fee_collector: Pubkey,
    /// Flat fee charged to the fee payer per pool transaction (mirrors the
    /// one-signature SVM fee so pool and regular txs cost the same).
    pub flat_fee: u64,
}

impl PoolConfig {
    pub fn new(pool_state: Pubkey, vault: Pubkey, fee_collector: Pubkey) -> Self {
        Self {
            pool_state,
            vault,
            fee_collector,
            flat_fee: LAMPORTS_PER_SIGNATURE,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PoolServiceError {
    #[error("store error: {0}")]
    Store(#[from] zul_store::StoreError),
    #[error("pool state account is corrupt")]
    CorruptState,
    #[error("pool error: {0}")]
    Pool(#[from] crate::processor::PoolError),
    #[error("insufficient funds for {0}")]
    InsufficientFunds(&'static str),
}

/// Outcome of processing a block's pool transactions.
#[derive(Debug, Default)]
pub struct PoolBlockResult {
    pub account_writes: Vec<(Pubkey, Option<StoredAccount>)>,
    pub nullifiers: Vec<[u8; 32]>,
    pub note_events: Vec<NoteEvent>,
    /// Per-input-transaction result, in order: Ok or the rejection reason.
    pub statuses: Vec<Result<(), String>>,
}

/// Nullifier membership bridging the committed store and within-block spends.
struct BlockNullifiers<'a> {
    store: &'a Store,
    within: HashSet<[u8; 32]>,
    newly: Vec<[u8; 32]>,
}

impl NullifierSet for BlockNullifiers<'_> {
    fn contains(&self, nullifier: &[u8; 32]) -> bool {
        self.within.contains(nullifier)
            || self.store.is_nullifier_spent(nullifier).unwrap_or(false)
    }
    fn insert(&mut self, nullifier: [u8; 32]) {
        self.within.insert(nullifier);
        self.newly.push(nullifier);
    }
}

pub struct PoolService<V: ProofVerifier> {
    verifier: V,
    config: PoolConfig,
}

impl<V: ProofVerifier> PoolService<V> {
    pub fn new(verifier: V, config: PoolConfig) -> Self {
        Self { verifier, config }
    }

    /// The genesis account for the pool state (empty tree). Added to genesis
    /// alongside the executor's builtin accounts.
    pub fn genesis_pool_account(&self) -> (Pubkey, StoredAccount) {
        let state = PoolState::new();
        let account = StoredAccount {
            lamports: 1,
            data: state.to_bytes(),
            owner: crate::POOL_PROGRAM_ID,
            executable: false,
            rent_epoch: zul_store::RENT_EXEMPT_RENT_EPOCH,
        };
        (self.config.pool_state, account)
    }

    fn load_state(&self, store: &Store) -> Result<PoolState, PoolServiceError> {
        match store.get_account(&self.config.pool_state)? {
            Some(account) => {
                PoolState::from_bytes(&account.data).map_err(|_| PoolServiceError::CorruptState)
            }
            None => Ok(PoolState::new()),
        }
    }

    /// Process every pool transaction in a block. `txs` pairs each pool
    /// instruction with its fee payer. Failing transactions are skipped (no
    /// state change, no fee) and reported in `statuses`.
    pub fn process_block(
        &self,
        store: &Store,
        txs: &[(Pubkey, PoolInstruction)],
    ) -> Result<PoolBlockResult, PoolServiceError> {
        self.process_block_seeded(store, txs, &HashMap::new())
    }

    /// Like `process_block`, but layered over `seed` lamport balances (the
    /// post-SVM account states for this block), so pool effects see and
    /// extend the regular transactions' results within the same block.
    pub fn process_block_seeded(
        &self,
        store: &Store,
        txs: &[(Pubkey, PoolInstruction)],
        seed: &HashMap<Pubkey, u64>,
    ) -> Result<PoolBlockResult, PoolServiceError> {
        let mut result = PoolBlockResult::default();
        if txs.is_empty() {
            return Ok(result);
        }

        let mut state = self.load_state(store)?;
        let mut nullifiers = BlockNullifiers {
            store,
            within: HashSet::new(),
            newly: Vec::new(),
        };
        // Lamport overlay seeded with this block's SVM results.
        let mut overlay: HashMap<Pubkey, u64> = seed.clone();
        // Wrapped-SPL token-account overlay (the evolving token states this
        // block touches; assembled into account writes at the end).
        let mut token_overlay: HashMap<Pubkey, StoredAccount> = HashMap::new();
        let mut state_dirty = false;

        for (fee_payer, instruction) in txs {
            match self.apply_one(
                store,
                &mut state,
                &mut nullifiers,
                &mut overlay,
                &mut token_overlay,
                fee_payer,
                instruction,
                &mut result.note_events,
            ) {
                Ok(()) => {
                    state_dirty = true;
                    result.statuses.push(Ok(()));
                }
                Err(e) => result.statuses.push(Err(e.to_string())),
            }
        }

        // Assemble account writes from the overlay.
        for (pubkey, lamports) in overlay {
            let write = self.lamport_write(store, pubkey, lamports)?;
            result.account_writes.push((pubkey, write));
        }
        // Assemble wrapped-SPL token-account writes.
        for (pubkey, account) in token_overlay {
            result.account_writes.push((pubkey, Some(account)));
        }
        // Persist the pool state account if anything changed.
        if state_dirty {
            let mut account = store
                .get_account(&self.config.pool_state)?
                .unwrap_or_else(|| self.genesis_pool_account().1);
            account.data = state.to_bytes();
            result
                .account_writes
                .push((self.config.pool_state, Some(account)));
        }
        result.nullifiers = nullifiers.newly;
        Ok(result)
    }

    /// Apply a single pool transaction against the running overlay. On error
    /// nothing in `state`/`overlay`/`nullifiers` is left partially applied:
    /// fund pre-checks and SPL rejection happen before any mutation, and the
    /// processor itself does not mutate on failure.
    #[allow(clippy::too_many_arguments)]
    fn apply_one(
        &self,
        store: &Store,
        state: &mut PoolState,
        nullifiers: &mut BlockNullifiers,
        overlay: &mut HashMap<Pubkey, u64>,
        token_overlay: &mut HashMap<Pubkey, StoredAccount>,
        fee_payer: &Pubkey,
        instruction: &PoolInstruction,
        note_events: &mut Vec<NoteEvent>,
    ) -> Result<(), PoolServiceError> {
        // Pre-check public funds so effect application cannot fail midway. The
        // native per-tx fee is always paid in ZUL lamports; the shielded value
        // is native lamports (Asset::Native) or a wrapped SPL token (Asset::Spl).
        let fee = self.config.flat_fee;
        match instruction {
            PoolInstruction::Shield { asset, amount, .. } => {
                let fee_need = match asset {
                    Asset::Native => amount.saturating_add(fee),
                    Asset::Spl(_) => fee,
                };
                if self.balance(store, overlay, fee_payer) < fee_need {
                    return Err(PoolServiceError::InsufficientFunds("shield fee"));
                }
                if let Asset::Spl(mint) = asset {
                    if self.token_balance(store, token_overlay, mint, fee_payer) < *amount {
                        return Err(PoolServiceError::InsufficientFunds("shield token"));
                    }
                }
            }
            PoolInstruction::Transfer { fee: tfee, .. } => {
                if self.balance(store, overlay, fee_payer) < fee {
                    return Err(PoolServiceError::InsufficientFunds("transfer fee"));
                }
                if self.balance(store, overlay, &self.config.vault) < *tfee {
                    return Err(PoolServiceError::InsufficientFunds("pool fee"));
                }
            }
            PoolInstruction::Unshield { asset, amount, .. } => {
                if self.balance(store, overlay, fee_payer) < fee {
                    return Err(PoolServiceError::InsufficientFunds("unshield fee"));
                }
                match asset {
                    Asset::Native => {
                        if self.balance(store, overlay, &self.config.vault) < *amount {
                            return Err(PoolServiceError::InsufficientFunds("vault"));
                        }
                    }
                    Asset::Spl(mint) => {
                        if self.token_balance(store, token_overlay, mint, &self.config.vault)
                            < *amount
                        {
                            return Err(PoolServiceError::InsufficientFunds("vault token"));
                        }
                    }
                }
            }
        }

        // Run the verified state machine.
        let processor = PoolProcessor::new(&self.verifier);
        let outcome = processor.process(state, nullifiers, instruction)?;

        // Charge the per-tx fee to the collector.
        self.move_lamports(store, overlay, fee_payer, &self.config.fee_collector, fee);

        // Apply value effects. Native moves lamports; SPL moves the wrapped
        // token between the holder's ATA and the pool's per-mint vault ATA.
        for effect in outcome.effects {
            match effect {
                PoolEffect::DepositToVault { asset, amount } => match asset {
                    Asset::Native => {
                        self.move_lamports(store, overlay, fee_payer, &self.config.vault, amount);
                    }
                    Asset::Spl(mint) => {
                        self.move_wrapped(
                            store,
                            token_overlay,
                            &mint,
                            fee_payer,
                            &self.config.vault,
                            amount,
                        );
                    }
                },
                PoolEffect::WithdrawFromVault { asset, amount, recipient } => match asset {
                    Asset::Native => {
                        self.move_lamports(store, overlay, &self.config.vault, &recipient, amount);
                    }
                    Asset::Spl(mint) => {
                        self.move_wrapped(
                            store,
                            token_overlay,
                            &mint,
                            &self.config.vault,
                            &recipient,
                            amount,
                        );
                    }
                },
                PoolEffect::PayFee { amount } => {
                    self.move_lamports(
                        store,
                        overlay,
                        &self.config.vault,
                        &self.config.fee_collector,
                        amount,
                    );
                }
            }
        }
        note_events.extend(outcome.note_events);
        Ok(())
    }

    fn balance(&self, store: &Store, overlay: &HashMap<Pubkey, u64>, key: &Pubkey) -> u64 {
        if let Some(v) = overlay.get(key) {
            return *v;
        }
        store
            .get_account(key)
            .ok()
            .flatten()
            .map(|a| a.lamports)
            .unwrap_or(0)
    }

    fn move_lamports(
        &self,
        store: &Store,
        overlay: &mut HashMap<Pubkey, u64>,
        from: &Pubkey,
        to: &Pubkey,
        amount: u64,
    ) {
        let from_bal = self.balance(store, overlay, from);
        let to_bal = self.balance(store, overlay, to);
        overlay.insert(*from, from_bal.saturating_sub(amount));
        overlay.insert(*to, to_bal.saturating_add(amount));
    }

    /// Wrapped-token balance of `owner`'s ATA for `mint` (overlay then store).
    fn token_balance(
        &self,
        store: &Store,
        token_overlay: &HashMap<Pubkey, StoredAccount>,
        mint: &[u8; 32],
        owner: &Pubkey,
    ) -> u64 {
        let wmint = w::wrapped_mint_address(&Pubkey::new_from_array(*mint));
        let ata = w::associated_token_address(owner, &wmint);
        if let Some(acc) = token_overlay.get(&ata) {
            return w::token_account_amount(&acc.data).unwrap_or(0);
        }
        store
            .get_account(&ata)
            .ok()
            .flatten()
            .and_then(|a| w::token_account_amount(&a.data))
            .unwrap_or(0)
    }

    /// Load `owner`'s wrapped-token ATA for `wmint` (overlay, then store, then a
    /// freshly synthesized empty rent-exempt account).
    fn token_account(
        &self,
        store: &Store,
        token_overlay: &HashMap<Pubkey, StoredAccount>,
        wmint: &Pubkey,
        owner: &Pubkey,
    ) -> StoredAccount {
        let ata = w::associated_token_address(owner, wmint);
        if let Some(acc) = token_overlay.get(&ata) {
            return acc.clone();
        }
        store.get_account(&ata).ok().flatten().unwrap_or_else(|| StoredAccount {
            lamports: w::rent_exempt_lamports(w::ACCOUNT_LEN),
            data: w::pack_token_account(wmint, owner, 0),
            owner: w::TOKEN_PROGRAM_ID,
            executable: false,
            rent_epoch: RENT_EXEMPT_RENT_EPOCH,
        })
    }

    /// Move `amount` of the wrapped token for `mint` from `from_owner`'s ATA to
    /// `to_owner`'s ATA, creating either ATA on first touch. Total supply is
    /// unchanged (the tokens are moved, not minted/burned).
    fn move_wrapped(
        &self,
        store: &Store,
        token_overlay: &mut HashMap<Pubkey, StoredAccount>,
        mint: &[u8; 32],
        from_owner: &Pubkey,
        to_owner: &Pubkey,
        amount: u64,
    ) {
        let wmint = w::wrapped_mint_address(&Pubkey::new_from_array(*mint));
        let from_ata = w::associated_token_address(from_owner, &wmint);
        let to_ata = w::associated_token_address(to_owner, &wmint);

        let mut from = self.token_account(store, token_overlay, &wmint, from_owner);
        let fb = w::token_account_amount(&from.data).unwrap_or(0);
        w::set_token_account_amount(&mut from.data, fb.saturating_sub(amount));
        token_overlay.insert(from_ata, from);

        // Re-read in case from_ata == to_ata (degenerate self-move).
        let mut to = self.token_account(store, token_overlay, &wmint, to_owner);
        let tb = w::token_account_amount(&to.data).unwrap_or(0);
        w::set_token_account_amount(&mut to.data, tb.saturating_add(amount));
        token_overlay.insert(to_ata, to);
    }

    /// Build the store write for an overlay balance, preserving non-lamport
    /// fields of any existing account.
    fn lamport_write(
        &self,
        store: &Store,
        pubkey: Pubkey,
        lamports: u64,
    ) -> Result<Option<StoredAccount>, PoolServiceError> {
        if lamports == 0 {
            // Keep the vault/collector alive even at zero; only drop unknown
            // accounts that never existed.
            if store.get_account(&pubkey)?.is_none() {
                return Ok(None);
            }
        }
        let mut account = store
            .get_account(&pubkey)?
            .unwrap_or_else(|| StoredAccount::new_system(0));
        account.lamports = lamports;
        Ok(Some(account))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::fr_to_bytes;
    use crate::poseidon::{native_asset_id, note_commitment};
    use crate::processor::ProofVerifier;
    use ark_bn254::Fr;
    use zul_primitives::genesis::{Allocation, GenesisConfig};
    use solana_sdk::signature::Keypair;
    use solana_sdk::signer::Signer;

    struct AcceptAll;
    impl ProofVerifier for AcceptAll {
        fn verify_transfer(&self, _p: &[u8], _i: &[Fr]) -> bool {
            true
        }
        fn verify_unshield(&self, _p: &[u8], _i: &[Fr]) -> bool {
            true
        }
    }

    const ZUL: u64 = 1_000_000_000;

    fn config() -> PoolConfig {
        PoolConfig::new(
            crate::POOL_PROGRAM_ID,
            Pubkey::new_from_array([0x0a; 32]),
            Pubkey::new_from_array([0x0c; 32]),
        )
    }

    fn setup() -> (tempfile::TempDir, Store, Keypair, PoolConfig) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("zul.redb")).unwrap();
        let user = Keypair::new();
        let cfg = config();
        let service = PoolService::new(AcceptAll, cfg.clone());
        let genesis = GenesisConfig {
            chain_name: "zul-pool-test".into(),
            creation_time_ms: 1,
            sequencer: Pubkey::new_unique(),
            fee_collector: cfg.fee_collector,
            allocations: vec![Allocation {
                pubkey: user.pubkey(),
                lamports: 100 * ZUL,
            }],
        };
        store
            .initialize_genesis(&genesis, &[service.genesis_pool_account()])
            .unwrap();
        (dir, store, user, cfg)
    }

    fn commit(result: &PoolBlockResult, store: &Store) {
        // Apply the result's writes directly (test shortcut for what the
        // node does inside commit_block).
        use zul_primitives::block::{Block, BlockHeader};
        use zul_store::LeafUpdate;
        let leaf_updates: Vec<LeafUpdate> = result
            .account_writes
            .iter()
            .map(|(pk, acc)| (*pk, acc.as_ref().map(|a| a.hash(pk))))
            .collect();
        let update = store.prepare_state_update(&leaf_updates).unwrap();
        let latest = store.latest_slot().unwrap();
        let slot = latest.map(|s| s + 1).unwrap_or(0);
        let header = BlockHeader {
            slot,
            parent_hash: [0u8; 32],
            state_root: update.new_root,
            transactions_root: Block::transactions_root(&[]),
            timestamp_ms: slot,
            transaction_count: 0,
        };
        let block = Block::seal(header, vec![], &Keypair::new());
        let mut queue = zul_store::BlockhashQueue::new();
        queue.register(slot, block.header.hash());
        store
            .commit_block(&block, &[], &result.account_writes, &update, &queue, &result.nullifiers)
            .unwrap();
    }

    fn native_commitment(amount: u64, blinding: u64) -> [u8; 32] {
        let pk = crate::poseidon::hash1(Fr::from(1u64));
        fr_to_bytes(&note_commitment(native_asset_id(), amount, pk, Fr::from(blinding)))
    }

    /// Canonical field element from a small integer (raw [n; 32] bytes can
    /// exceed the BN254 modulus and are rejected as non-canonical).
    fn field(n: u64) -> [u8; 32] {
        fr_to_bytes(&Fr::from(n))
    }

    /// The opaque shield secret for a note with the given blinding.
    fn native_secret(blinding: u64) -> [u8; 32] {
        let pk = crate::poseidon::hash1(Fr::from(1u64));
        fr_to_bytes(&crate::poseidon::note_secret(pk, Fr::from(blinding)))
    }

    #[test]
    fn shield_moves_funds_to_vault_and_grows_tree() {
        let (_dir, store, user, cfg) = setup();
        let service = PoolService::new(AcceptAll, cfg.clone());

        let amount = 10 * ZUL;
        let ix = PoolInstruction::Shield {
            asset: Asset::Native,
            amount,
            secret: native_secret(1),
            encrypted_note: vec![0xab; 48],
        };
        let result = service.process_block(&store, &[(user.pubkey(), ix)]).unwrap();
        assert_eq!(result.statuses, vec![Ok(())]);
        assert_eq!(result.note_events.len(), 1);
        assert_eq!(result.nullifiers.len(), 0);
        commit(&result, &store);

        // User debited amount + fee; vault credited amount.
        let user_bal = store.get_account(&user.pubkey()).unwrap().unwrap().lamports;
        assert_eq!(user_bal, 100 * ZUL - amount - LAMPORTS_PER_SIGNATURE);
        let vault_bal = store.get_account(&cfg.vault).unwrap().unwrap().lamports;
        assert_eq!(vault_bal, amount);
        let collector_bal = store.get_account(&cfg.fee_collector).unwrap().unwrap().lamports;
        assert_eq!(collector_bal, LAMPORTS_PER_SIGNATURE);

        // The pool state account now has one commitment.
        let state = PoolState::from_bytes(
            &store.get_account(&cfg.pool_state).unwrap().unwrap().data,
        )
        .unwrap();
        assert_eq!(state.next_leaf_index, 1);
    }

    #[test]
    fn shield_rejected_when_underfunded() {
        let (_dir, store, user, cfg) = setup();
        let service = PoolService::new(AcceptAll, cfg);
        let ix = PoolInstruction::Shield {
            asset: Asset::Native,
            amount: 1_000 * ZUL, // more than the user holds
            secret: native_secret(1),
            encrypted_note: vec![],
        };
        let result = service.process_block(&store, &[(user.pubkey(), ix)]).unwrap();
        assert!(result.statuses[0].is_err());
        assert!(result.account_writes.is_empty());
        assert!(result.nullifiers.is_empty());
    }

    /// Commitment for an SPL note (asset id = the mint), for unshield change.
    fn spl_commitment(mint: &[u8; 32], amount: u64, blinding: u64) -> [u8; 32] {
        use crate::poseidon::asset_id_for_mint;
        let pk = crate::poseidon::hash1(Fr::from(1u64));
        let asset_id = asset_id_for_mint(&Pubkey::new_from_array(*mint));
        fr_to_bytes(&note_commitment(asset_id, amount, pk, Fr::from(blinding)))
    }

    /// Seed a wrapped-token ATA into the store (stands in for a prior bridge
    /// deposit) so the holder can shield it.
    fn seed_wrapped(store: &Store, mint: &[u8; 32], owner: &Pubkey, amount: u64) {
        let wmint = w::wrapped_mint_address(&Pubkey::new_from_array(*mint));
        let ata = w::associated_token_address(owner, &wmint);
        let account = StoredAccount {
            lamports: w::rent_exempt_lamports(w::ACCOUNT_LEN),
            data: w::pack_token_account(&wmint, owner, amount),
            owner: w::TOKEN_PROGRAM_ID,
            executable: false,
            rent_epoch: RENT_EXEMPT_RENT_EPOCH,
        };
        let result = PoolBlockResult {
            account_writes: vec![(ata, Some(account))],
            ..Default::default()
        };
        commit(&result, store);
    }

    fn wrapped_balance(store: &Store, mint: &[u8; 32], owner: &Pubkey) -> u64 {
        let wmint = w::wrapped_mint_address(&Pubkey::new_from_array(*mint));
        let ata = w::associated_token_address(owner, &wmint);
        store
            .get_account(&ata)
            .unwrap()
            .and_then(|a| w::token_account_amount(&a.data))
            .unwrap_or(0)
    }

    #[test]
    fn spl_shield_and_unshield_move_wrapped_tokens_privately() {
        let (_dir, store, user, cfg) = setup();
        let service = PoolService::new(AcceptAll, cfg.clone());
        let mint = [7u8; 32];

        // The user already holds 1000 wrapped tokens (from a bridge deposit).
        seed_wrapped(&store, &mint, &user.pubkey(), 1_000);

        // Shield 600 of the SPL token: it moves into the pool's per-mint vault,
        // the commitment enters the tree, and only the native FEE leaves the
        // user's lamports (not the token amount).
        let shield = PoolInstruction::Shield {
            asset: Asset::Spl(mint),
            amount: 600,
            secret: native_secret(1),
            encrypted_note: vec![0xab; 48],
        };
        let r0 = service.process_block(&store, &[(user.pubkey(), shield)]).unwrap();
        assert_eq!(r0.statuses, vec![Ok(())]);
        assert_eq!(r0.note_events.len(), 1);
        commit(&r0, &store);

        assert_eq!(wrapped_balance(&store, &mint, &user.pubkey()), 400);
        assert_eq!(wrapped_balance(&store, &mint, &cfg.vault), 600);
        // User paid only the native fee in ZUL, no token-as-lamports leak.
        let user_lamports = store.get_account(&user.pubkey()).unwrap().unwrap().lamports;
        assert_eq!(user_lamports, 100 * ZUL - LAMPORTS_PER_SIGNATURE);

        // Unshield 400 of the SPL token to a fresh recipient.
        let root = PoolState::from_bytes(
            &store.get_account(&cfg.pool_state).unwrap().unwrap().data,
        )
        .unwrap()
        .current_root();
        let recipient = Pubkey::new_unique();
        let unshield = PoolInstruction::Unshield {
            proof: vec![0u8; 256],
            root,
            nullifier: field(0x55),
            change_commitment: spl_commitment(&mint, 200, 7),
            asset: Asset::Spl(mint),
            amount: 400,
            recipient: recipient.to_bytes(),
            encrypted_change_note: vec![],
        };
        let r1 = service.process_block(&store, &[(user.pubkey(), unshield)]).unwrap();
        assert_eq!(r1.statuses, vec![Ok(())]);
        commit(&r1, &store);

        // The vault released 400 wrapped tokens to the recipient's ATA.
        assert_eq!(wrapped_balance(&store, &mint, &cfg.vault), 200);
        assert_eq!(wrapped_balance(&store, &mint, &recipient), 400);
    }

    #[test]
    fn spl_shield_rejected_without_wrapped_balance() {
        let (_dir, store, user, cfg) = setup();
        let service = PoolService::new(AcceptAll, cfg);
        // No wrapped tokens seeded → shield must fail the token pre-check.
        let ix = PoolInstruction::Shield {
            asset: Asset::Spl([7u8; 32]),
            amount: 600,
            secret: native_secret(1),
            encrypted_note: vec![],
        };
        let result = service.process_block(&store, &[(user.pubkey(), ix)]).unwrap();
        assert!(result.statuses[0].as_ref().err().unwrap().contains("token"));
        assert!(result.nullifiers.is_empty());
    }

    #[test]
    fn double_spend_across_blocks_is_rejected() {
        let (_dir, store, user, cfg) = setup();
        let service = PoolService::new(AcceptAll, cfg.clone());

        // Seed the vault and tree with a shield so a transfer has a valid root.
        let shield = PoolInstruction::Shield {
            asset: Asset::Native,
            amount: 20 * ZUL,
            secret: native_secret(9),
            encrypted_note: vec![],
        };
        let r0 = service.process_block(&store, &[(user.pubkey(), shield)]).unwrap();
        commit(&r0, &store);

        let root = {
            let s = PoolState::from_bytes(
                &store.get_account(&cfg.pool_state).unwrap().unwrap().data,
            )
            .unwrap();
            s.current_root()
        };
        let transfer = PoolInstruction::Transfer {
            proof: vec![0u8; 256],
            root,
            nullifiers: [field(0x33), field(0x34)],
            commitments: [native_commitment(10 * ZUL, 1), native_commitment(10 * ZUL, 2)],
            fee: 0,
            encrypted_notes: [vec![], vec![]],
        };
        let r1 = service.process_block(&store, &[(user.pubkey(), transfer.clone())]).unwrap();
        assert_eq!(r1.statuses, vec![Ok(())]);
        assert_eq!(r1.nullifiers.len(), 2);
        commit(&r1, &store);

        // Replaying the same nullifiers in a later block is rejected.
        let r2 = service.process_block(&store, &[(user.pubkey(), transfer)]).unwrap();
        assert!(r2.statuses[0].as_ref().err().unwrap().contains("double spend"));
        assert!(r2.nullifiers.is_empty());
    }

    #[test]
    fn unshield_releases_native_funds_to_recipient() {
        let (_dir, store, user, cfg) = setup();
        let service = PoolService::new(AcceptAll, cfg.clone());

        // Fund the vault via a shield first.
        let shield = PoolInstruction::Shield {
            asset: Asset::Native,
            amount: 30 * ZUL,
            secret: native_secret(5),
            encrypted_note: vec![],
        };
        let r0 = service.process_block(&store, &[(user.pubkey(), shield)]).unwrap();
        commit(&r0, &store);
        let root = PoolState::from_bytes(
            &store.get_account(&cfg.pool_state).unwrap().unwrap().data,
        )
        .unwrap()
        .current_root();

        let recipient = Pubkey::new_unique();
        let unshield = PoolInstruction::Unshield {
            proof: vec![0u8; 256],
            root,
            nullifier: field(0x44),
            change_commitment: native_commitment(5 * ZUL, 7),
            asset: Asset::Native,
            amount: 25 * ZUL,
            recipient: recipient.to_bytes(),
            encrypted_change_note: vec![],
        };
        let r1 = service.process_block(&store, &[(user.pubkey(), unshield)]).unwrap();
        assert_eq!(r1.statuses, vec![Ok(())]);
        commit(&r1, &store);

        let recipient_bal = store.get_account(&recipient).unwrap().unwrap().lamports;
        assert_eq!(recipient_bal, 25 * ZUL);
        let vault_bal = store.get_account(&cfg.vault).unwrap().unwrap().lamports;
        assert_eq!(vault_bal, 30 * ZUL - 25 * ZUL);
    }
}
