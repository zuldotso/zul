//! The persistent store. One redb database holds accounts, blocks,
//! transaction indices, the state SMT, and chain metadata.
//!
//! Block commit is two-phase by design: the node first calls
//! `prepare_state_update` to learn the post-block state root (which must be
//! sealed into the signed header), then `commit_block` persists the block,
//! account writes, SMT nodes, indices, and metadata in one atomic redb
//! write transaction. The sequencer is the only writer, so nothing can
//! interleave between the two phases.

use zul_primitives::{
    block::Block,
    genesis::GenesisConfig,
    hash::H256,
    meta::ExecutedTransactionMeta,
};
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use solana_sdk::{pubkey::Pubkey, signature::Signature, transaction::VersionedTransaction};
use std::path::Path;

use crate::account::StoredAccount;
use crate::error::{Result, StoreError};
use crate::queue::BlockhashQueue;
use crate::smt::{self, LeafUpdate, NodeKey, NodeSource, SmtProof, StateUpdate};

const T_ACCOUNTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("accounts");
const T_BLOCKS: TableDefinition<u64, &[u8]> = TableDefinition::new("blocks");
const T_TX_LOC: TableDefinition<&[u8], &[u8]> = TableDefinition::new("tx_locator");
const T_TX_META: TableDefinition<&[u8], &[u8]> = TableDefinition::new("tx_meta");
const T_ADDR_SIGS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("addr_sigs");
const T_SMT: TableDefinition<&[u8], &[u8]> = TableDefinition::new("smt_nodes");
const T_NULLIFIERS: TableDefinition<&[u8], u64> = TableDefinition::new("nullifiers");
const T_META: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");

const META_GENESIS_HASH: &str = "genesis_hash";
const META_STATE_ROOT: &str = "state_root";
const META_LATEST_SLOT: &str = "latest_slot";
const META_BLOCKHASH_QUEUE: &str = "blockhash_queue";
const META_WITHDRAWALS_ROOT: &str = "withdrawals_root";

/// Where a transaction lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxLocator {
    pub slot: u64,
    pub index: u32,
}

pub struct Store {
    db: Database,
}

/// Snapshot read access to committed SMT nodes.
pub struct SmtSnapshot {
    table: redb::ReadOnlyTable<&'static [u8], &'static [u8]>,
}

impl NodeSource for SmtSnapshot {
    fn get_node(&self, key: &NodeKey) -> Option<H256> {
        let guard = self.table.get(key.as_slice()).ok()??;
        let value = guard.value();
        if value.len() != 32 {
            return None;
        }
        let mut h = [0u8; 32];
        h.copy_from_slice(value);
        Some(h)
    }
}

/// addr_sigs key: pubkey || !slot (BE) || !index (BE) — ascending iteration
/// yields newest-first.
fn addr_sig_key(pubkey: &Pubkey, slot: u64, index: u32) -> [u8; 44] {
    let mut key = [0u8; 44];
    key[..32].copy_from_slice(pubkey.as_ref());
    key[32..40].copy_from_slice(&(!slot).to_be_bytes());
    key[40..44].copy_from_slice(&(!index).to_be_bytes());
    key
}

impl Store {
    /// Open (or create) the database and ensure all tables exist.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let db = Database::create(path)?;
        let txn = db.begin_write()?;
        {
            txn.open_table(T_ACCOUNTS)?;
            txn.open_table(T_BLOCKS)?;
            txn.open_table(T_TX_LOC)?;
            txn.open_table(T_TX_META)?;
            txn.open_table(T_ADDR_SIGS)?;
            txn.open_table(T_SMT)?;
            txn.open_table(T_NULLIFIERS)?;
            txn.open_table(T_META)?;
        }
        txn.commit()?;
        Ok(Self { db })
    }

    /// Whether a shielded-pool nullifier has been spent. Existence in the
    /// table means spent; the value is the slot it was spent at.
    pub fn is_nullifier_spent(&self, nullifier: &[u8; 32]) -> Result<bool> {
        let rtx = self.db.begin_read()?;
        let table = rtx.open_table(T_NULLIFIERS)?;
        Ok(table.get(nullifier.as_slice())?.is_some())
    }

    // ----- metadata ------------------------------------------------------

    fn read_meta(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let rtx = self.db.begin_read()?;
        let table = rtx.open_table(T_META)?;
        Ok(table.get(key)?.map(|g| g.value().to_vec()))
    }

    fn read_meta_h256(&self, key: &str) -> Result<Option<H256>> {
        Ok(self.read_meta(key)?.and_then(|v| {
            (v.len() == 32).then(|| {
                let mut h = [0u8; 32];
                h.copy_from_slice(&v);
                h
            })
        }))
    }

    pub fn genesis_hash(&self) -> Result<Option<H256>> {
        self.read_meta_h256(META_GENESIS_HASH)
    }

    pub fn state_root(&self) -> Result<Option<H256>> {
        self.read_meta_h256(META_STATE_ROOT)
    }

    pub fn latest_slot(&self) -> Result<Option<u64>> {
        Ok(self.read_meta(META_LATEST_SLOT)?.and_then(|v| {
            v.try_into().ok().map(u64::from_le_bytes)
        }))
    }

    pub fn load_blockhash_queue(&self) -> Result<Option<BlockhashQueue>> {
        match self.read_meta(META_BLOCKHASH_QUEUE)? {
            Some(bytes) => Ok(Some(BlockhashQueue::from_bytes(&bytes)?)),
            None => Ok(None),
        }
    }

    /// Root of the shallow withdrawals SMT, last written by the node. The
    /// batcher posts this to L1 so `claim_withdrawal` can verify against it.
    /// Defaults to all-zero before the node first writes it.
    pub fn withdrawals_root(&self) -> Result<H256> {
        Ok(self.read_meta_h256(META_WITHDRAWALS_ROOT)?.unwrap_or([0u8; 32]))
    }

    pub fn set_withdrawals_root(&self, root: &H256) -> Result<()> {
        let txn = self.db.begin_write()?;
        {
            let mut meta = txn.open_table(T_META)?;
            meta.insert(META_WITHDRAWALS_ROOT, root.as_slice())?;
        }
        txn.commit()?;
        Ok(())
    }

    // ----- genesis -------------------------------------------------------

    /// Initialize chain state from genesis. Idempotent: re-running with the
    /// same genesis is a no-op; a different genesis is an error.
    ///
    /// `extra_accounts` carries runtime bootstrap accounts (builtin
    /// programs, static sysvars) provided by the executor; they are part of
    /// the genesis state and the genesis state root.
    pub fn initialize_genesis(
        &self,
        genesis: &GenesisConfig,
        extra_accounts: &[(Pubkey, StoredAccount)],
    ) -> Result<H256> {
        let expected = genesis.genesis_hash();
        if let Some(existing) = self.genesis_hash()? {
            if existing != expected {
                return Err(StoreError::Invariant(
                    "database was initialized with a different genesis".into(),
                ));
            }
            return self
                .state_root()?
                .ok_or_else(|| StoreError::Invariant("genesis present but no state root".into()));
        }

        let mut account_writes: Vec<(Pubkey, Option<StoredAccount>)> = Vec::new();
        let mut leaf_updates: Vec<LeafUpdate> = Vec::new();
        for alloc in &genesis.allocations {
            let account = StoredAccount::new_system(alloc.lamports);
            leaf_updates.push((alloc.pubkey, Some(account.hash(&alloc.pubkey))));
            account_writes.push((alloc.pubkey, Some(account)));
        }
        for (pubkey, account) in extra_accounts {
            leaf_updates.push((*pubkey, Some(account.hash(pubkey))));
            account_writes.push((*pubkey, Some(account.clone())));
        }
        let update = self.prepare_state_update(&leaf_updates)?;

        let txn = self.db.begin_write()?;
        {
            let mut accounts = txn.open_table(T_ACCOUNTS)?;
            for (pubkey, account) in &account_writes {
                let bytes = bincode::serialize(account.as_ref().expect("genesis only creates"))?;
                accounts.insert(pubkey.as_ref(), bytes.as_slice())?;
            }
            let mut smt_table = txn.open_table(T_SMT)?;
            Self::write_smt_nodes(&mut smt_table, &update)?;
            let mut meta = txn.open_table(T_META)?;
            meta.insert(META_GENESIS_HASH, expected.as_slice())?;
            meta.insert(META_STATE_ROOT, update.new_root.as_slice())?;
        }
        txn.commit()?;
        Ok(update.new_root)
    }

    // ----- state ---------------------------------------------------------

    pub fn get_account(&self, pubkey: &Pubkey) -> Result<Option<StoredAccount>> {
        let rtx = self.db.begin_read()?;
        let table = rtx.open_table(T_ACCOUNTS)?;
        match table.get(pubkey.as_ref())? {
            Some(guard) => Ok(Some(bincode::deserialize(guard.value())?)),
            None => Ok(None),
        }
    }

    /// Full-scan filter by owner. O(total accounts) — acceptable at
    /// prototype scale; the indexer/Postgres serves this at volume.
    pub fn scan_accounts_by_owner(
        &self,
        owner: &Pubkey,
        limit: usize,
    ) -> Result<Vec<(Pubkey, StoredAccount)>> {
        let rtx = self.db.begin_read()?;
        let table = rtx.open_table(T_ACCOUNTS)?;
        let mut out = Vec::new();
        for entry in table.iter()? {
            let (k, v) = entry?;
            let account: StoredAccount = bincode::deserialize(v.value())?;
            if account.owner == *owner {
                let pubkey = Pubkey::try_from(k.value())
                    .map_err(|_| StoreError::Invariant("malformed account key".into()))?;
                out.push((pubkey, account));
                if out.len() >= limit {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// Snapshot for SMT reads (proofs, prepare).
    pub fn smt_snapshot(&self) -> Result<SmtSnapshot> {
        let rtx = self.db.begin_read()?;
        let table = rtx.open_table(T_SMT)?;
        Ok(SmtSnapshot { table })
    }

    pub fn prepare_state_update(&self, updates: &[LeafUpdate]) -> Result<StateUpdate> {
        let snapshot = self.smt_snapshot()?;
        Ok(smt::apply_updates(&snapshot, updates))
    }

    /// Account + inclusion (or absence) proof against the committed root.
    pub fn prove_account(&self, pubkey: &Pubkey) -> Result<(Option<StoredAccount>, SmtProof)> {
        let account = self.get_account(pubkey)?;
        let snapshot = self.smt_snapshot()?;
        Ok((account, smt::prove(&snapshot, pubkey)))
    }

    // ----- blocks --------------------------------------------------------

    pub fn get_block(&self, slot: u64) -> Result<Option<Block>> {
        let rtx = self.db.begin_read()?;
        let table = rtx.open_table(T_BLOCKS)?;
        match table.get(slot)? {
            Some(guard) => Ok(Some(bincode::deserialize(guard.value())?)),
            None => Ok(None),
        }
    }

    /// Slots with blocks in `[from, to]`, ascending, capped at `limit`.
    pub fn block_slots_in_range(&self, from: u64, to: u64, limit: usize) -> Result<Vec<u64>> {
        let rtx = self.db.begin_read()?;
        let table = rtx.open_table(T_BLOCKS)?;
        let mut slots = Vec::new();
        for entry in table.range(from..=to)? {
            let (k, _) = entry?;
            slots.push(k.value());
            if slots.len() >= limit {
                break;
            }
        }
        Ok(slots)
    }

    pub fn get_transaction(
        &self,
    sig: &Signature,
    ) -> Result<Option<(TxLocator, VersionedTransaction, ExecutedTransactionMeta)>> {
        let rtx = self.db.begin_read()?;
        let loc_table = rtx.open_table(T_TX_LOC)?;
        let locator: TxLocator = match loc_table.get(sig.as_ref())? {
            Some(guard) => bincode::deserialize(guard.value())?,
            None => return Ok(None),
        };
        let meta_table = rtx.open_table(T_TX_META)?;
        let meta: ExecutedTransactionMeta = match meta_table.get(sig.as_ref())? {
            Some(guard) => bincode::deserialize(guard.value())?,
            None => return Err(StoreError::Invariant("locator without meta".into())),
        };
        let block_table = rtx.open_table(T_BLOCKS)?;
        let block: Block = match block_table.get(locator.slot)? {
            Some(guard) => bincode::deserialize(guard.value())?,
            None => return Err(StoreError::Invariant("locator points at missing block".into())),
        };
        let tx = block
            .transactions
            .get(locator.index as usize)
            .cloned()
            .ok_or_else(|| StoreError::Invariant("locator index out of range".into()))?;
        Ok(Some((locator, tx, meta)))
    }

    /// Newest-first signatures touching `address`, optionally strictly older
    /// than `before` = (slot, index).
    pub fn signatures_for_address(
        &self,
        address: &Pubkey,
        before: Option<(u64, u32)>,
        limit: usize,
    ) -> Result<Vec<(Signature, u64, u32)>> {
        let rtx = self.db.begin_read()?;
        let table = rtx.open_table(T_ADDR_SIGS)?;
        let start: [u8; 44] = match before {
            Some((slot, index)) => addr_sig_key(address, slot, index),
            None => {
                let mut k = [0u8; 44];
                k[..32].copy_from_slice(address.as_ref());
                k
            }
        };
        let mut end = [0xffu8; 44];
        end[..32].copy_from_slice(address.as_ref());

        let bounds = (
            std::ops::Bound::Excluded(start.as_slice()),
            std::ops::Bound::Included(end.as_slice()),
        );
        let mut out = Vec::new();
        for entry in table.range::<&[u8]>(bounds)? {
            let (k, v) = entry?;
            let key = k.value();
            let mut slot_bytes = [0u8; 8];
            slot_bytes.copy_from_slice(&key[32..40]);
            let mut idx_bytes = [0u8; 4];
            idx_bytes.copy_from_slice(&key[40..44]);
            let sig = Signature::try_from(v.value())
                .map_err(|_| StoreError::Invariant("malformed signature in index".into()))?;
            out.push((sig, !u64::from_be_bytes(slot_bytes), !u32::from_be_bytes(idx_bytes)));
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    // ----- commit --------------------------------------------------------

    fn write_smt_nodes(
        table: &mut redb::Table<'_, &'static [u8], &'static [u8]>,
        update: &StateUpdate,
    ) -> Result<()> {
        let empties = smt::empty_hashes();
        for (key, value) in &update.node_writes {
            let depth = u16::from_be_bytes([key[0], key[1]]) as usize;
            if *value == empties[depth] {
                table.remove(key.as_slice())?;
            } else {
                table.insert(key.as_slice(), value.as_slice())?;
            }
        }
        Ok(())
    }

    /// Persist a sealed block atomically: accounts, SMT nodes, the block,
    /// transaction indices, spent shielded-pool nullifiers, and chain
    /// metadata. `nullifiers` are the pool nullifiers spent in this block.
    pub fn commit_block(
        &self,
        block: &Block,
        metas: &[ExecutedTransactionMeta],
        account_writes: &[(Pubkey, Option<StoredAccount>)],
        state_update: &StateUpdate,
        queue: &BlockhashQueue,
        nullifiers: &[[u8; 32]],
    ) -> Result<()> {
        if metas.len() != block.transactions.len() {
            return Err(StoreError::Invariant(
                "metas must parallel block transactions".into(),
            ));
        }
        if block.header.state_root != state_update.new_root {
            return Err(StoreError::Invariant(
                "header state root does not match prepared update".into(),
            ));
        }
        if let Some(latest) = self.latest_slot()? {
            if block.header.slot <= latest {
                return Err(StoreError::Invariant(format!(
                    "non-monotonic slot {} (latest {})",
                    block.header.slot, latest
                )));
            }
        }

        let txn = self.db.begin_write()?;
        {
            let mut accounts = txn.open_table(T_ACCOUNTS)?;
            for (pubkey, account) in account_writes {
                match account {
                    Some(acc) => {
                        let bytes = bincode::serialize(acc)?;
                        accounts.insert(pubkey.as_ref(), bytes.as_slice())?;
                    }
                    None => {
                        accounts.remove(pubkey.as_ref())?;
                    }
                }
            }

            let mut smt_table = txn.open_table(T_SMT)?;
            Self::write_smt_nodes(&mut smt_table, state_update)?;

            let mut blocks = txn.open_table(T_BLOCKS)?;
            let block_bytes = bincode::serialize(block)?;
            blocks.insert(block.header.slot, block_bytes.as_slice())?;

            let mut loc_table = txn.open_table(T_TX_LOC)?;
            let mut meta_table = txn.open_table(T_TX_META)?;
            let mut addr_table = txn.open_table(T_ADDR_SIGS)?;
            for (index, (tx, meta)) in block.transactions.iter().zip(metas).enumerate() {
                let Some(sig) = tx.signatures.first() else {
                    return Err(StoreError::Invariant("transaction without signature".into()));
                };
                let locator = TxLocator {
                    slot: block.header.slot,
                    index: index as u32,
                };
                loc_table.insert(sig.as_ref(), bincode::serialize(&locator)?.as_slice())?;
                meta_table.insert(sig.as_ref(), bincode::serialize(meta)?.as_slice())?;
                for address in tx.message.static_account_keys() {
                    let key = addr_sig_key(address, block.header.slot, index as u32);
                    addr_table.insert(key.as_slice(), sig.as_ref())?;
                }
            }

            let mut nullifier_table = txn.open_table(T_NULLIFIERS)?;
            for nullifier in nullifiers {
                nullifier_table.insert(nullifier.as_slice(), block.header.slot)?;
            }

            let mut meta = txn.open_table(T_META)?;
            meta.insert(META_LATEST_SLOT, block.header.slot.to_le_bytes().as_slice())?;
            meta.insert(META_STATE_ROOT, state_update.new_root.as_slice())?;
            meta.insert(META_BLOCKHASH_QUEUE, queue.to_bytes().as_slice())?;
        }
        txn.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zul_primitives::block::BlockHeader;
    use solana_sdk::{
        hash::Hash, signature::Keypair, signer::Signer, transaction::Transaction,
    };
    use solana_system_interface::instruction as system_instruction;

    const GENESIS: &str = r#"
chain_name = "zul-store-test"
creation_time_ms = 1760000000000
sequencer = "E1qwDui9Muq3gfyGwdk1qZXnNyy2WzgHkESJGr4KwJpm"
fee_collector = "6PaVUAPQqyFWxYaLbnVaiwcAkDqiqjCvMMmFoGmNdCFM"

[[allocations]]
pubkey = "Hvd9JUwyRzdm7m7yRcSQBphQtpPNWf5GmCEZRKoWJGSp"
lamports = 1000000000000000000
"#;

    fn temp_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("zul.redb")).unwrap();
        (dir, store)
    }

    fn faucet_pubkey() -> Pubkey {
        "Hvd9JUwyRzdm7m7yRcSQBphQtpPNWf5GmCEZRKoWJGSp"
            .parse()
            .unwrap()
    }

    #[test]
    fn genesis_roundtrip_and_idempotence() {
        let (_dir, store) = temp_store();
        let genesis = GenesisConfig::from_toml_str(GENESIS).unwrap();
        let root = store.initialize_genesis(&genesis, &[]).unwrap();

        assert_eq!(store.state_root().unwrap(), Some(root));
        assert_eq!(store.genesis_hash().unwrap(), Some(genesis.genesis_hash()));
        let faucet = store.get_account(&faucet_pubkey()).unwrap().unwrap();
        assert_eq!(faucet.lamports, 1_000_000_000_000_000_000);

        // Idempotent re-init.
        assert_eq!(store.initialize_genesis(&genesis, &[]).unwrap(), root);

        // Different genesis is rejected.
        let mut other = genesis.clone();
        other.chain_name = "different".into();
        assert!(store.initialize_genesis(&other, &[]).is_err());
    }

    #[test]
    fn account_proofs_verify_against_root() {
        let (_dir, store) = temp_store();
        let genesis = GenesisConfig::from_toml_str(GENESIS).unwrap();
        let root = store.initialize_genesis(&genesis, &[]).unwrap();

        let faucet = faucet_pubkey();
        let (account, proof) = store.prove_account(&faucet).unwrap();
        let account = account.unwrap();
        assert!(smt::verify(&root, &faucet, Some(&account.hash(&faucet)), &proof));

        let absent = Pubkey::new_unique();
        let (none, absence) = store.prove_account(&absent).unwrap();
        assert!(none.is_none());
        assert!(smt::verify(&root, &absent, None, &absence));
    }

    #[test]
    fn commit_block_persists_everything_atomically() {
        let (_dir, store) = temp_store();
        let genesis = GenesisConfig::from_toml_str(GENESIS).unwrap();
        store.initialize_genesis(&genesis, &[]).unwrap();
        let genesis_hash = genesis.genesis_hash();

        // A real signed transfer so signatures and address keys exist.
        let payer = Keypair::new();
        let recipient = Pubkey::new_unique();
        let ix = system_instruction::transfer(&payer.pubkey(), &recipient, 1_000);
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&payer.pubkey()),
            &[&payer],
            Hash::new_from_array(genesis_hash),
        );
        let vtx = VersionedTransaction::from(tx);
        let sig = vtx.signatures[0];

        // Simulated post-execution state.
        let payer_account = StoredAccount::new_system(10_000);
        let recipient_account = StoredAccount::new_system(1_000);
        let account_writes = vec![
            (payer.pubkey(), Some(payer_account.clone())),
            (recipient, Some(recipient_account.clone())),
        ];
        let leaf_updates: Vec<LeafUpdate> = account_writes
            .iter()
            .map(|(pk, acc)| (*pk, acc.as_ref().map(|a| a.hash(pk))))
            .collect();
        let update = store.prepare_state_update(&leaf_updates).unwrap();

        let transactions = vec![vtx];
        let header = BlockHeader {
            slot: 0,
            parent_hash: genesis_hash,
            state_root: update.new_root,
            transactions_root: Block::transactions_root(&transactions),
            timestamp_ms: 1,
            transaction_count: 1,
        };
        let sequencer = Keypair::new();
        let block = Block::seal(header, transactions, &sequencer);
        let blockhash = block.header.hash();

        let meta = ExecutedTransactionMeta {
            status: Ok(()),
            fee: 5_000,
            compute_units_consumed: 150,
            log_messages: vec![],
            pre_balances: vec![16_000, 0, 0],
            post_balances: vec![10_000, 1_000, 0],
        };

        let mut queue = BlockhashQueue::new();
        queue.register(0, blockhash);

        store
            .commit_block(&block, &[meta.clone()], &account_writes, &update, &queue, &[])
            .unwrap();

        assert_eq!(store.latest_slot().unwrap(), Some(0));
        assert_eq!(store.state_root().unwrap(), Some(update.new_root));

        let fetched = store.get_block(0).unwrap().unwrap();
        assert_eq!(fetched.header, block.header);

        let (locator, fetched_tx, fetched_meta) =
            store.get_transaction(&sig).unwrap().unwrap();
        assert_eq!(locator, TxLocator { slot: 0, index: 0 });
        assert_eq!(fetched_tx.signatures[0], sig);
        assert_eq!(fetched_meta, meta);

        let sigs = store
            .signatures_for_address(&payer.pubkey(), None, 10)
            .unwrap();
        assert_eq!(sigs.len(), 1);
        assert_eq!(sigs[0], (sig, 0, 0));

        let restored_queue = store.load_blockhash_queue().unwrap().unwrap();
        assert!(restored_queue.is_valid(&blockhash));

        // Non-monotonic slot is rejected.
        let err = store
            .commit_block(&block, &[meta], &account_writes, &update, &queue, &[])
            .unwrap_err();
        assert!(matches!(err, StoreError::Invariant(_)));
    }

    #[test]
    fn signatures_for_address_pagination_is_newest_first() {
        let (_dir, store) = temp_store();
        let genesis = GenesisConfig::from_toml_str(GENESIS).unwrap();
        store.initialize_genesis(&genesis, &[]).unwrap();
        let genesis_hash = genesis.genesis_hash();
        let sequencer = Keypair::new();
        let payer = Keypair::new();
        let mut queue = BlockhashQueue::new();

        let mut sigs = Vec::new();
        for slot in 0..3u64 {
            let ix = system_instruction::transfer(&payer.pubkey(), &Pubkey::new_unique(), 1);
            let tx = Transaction::new_signed_with_payer(
                &[ix],
                Some(&payer.pubkey()),
                &[&payer],
                Hash::new_from_array([slot as u8 + 1; 32]),
            );
            let vtx = VersionedTransaction::from(tx);
            sigs.push(vtx.signatures[0]);
            let transactions = vec![vtx];
            let update = store.prepare_state_update(&[]).unwrap();
            let header = BlockHeader {
                slot,
                parent_hash: genesis_hash,
                state_root: update.new_root,
                transactions_root: Block::transactions_root(&transactions),
                timestamp_ms: slot,
                transaction_count: 1,
            };
            let block = Block::seal(header, transactions, &sequencer);
            queue.register(slot, block.header.hash());
            let meta = ExecutedTransactionMeta {
                status: Ok(()),
                fee: 5_000,
                compute_units_consumed: 0,
                log_messages: vec![],
                pre_balances: vec![],
                post_balances: vec![],
            };
            store
                .commit_block(&block, &[meta], &[], &update, &queue, &[])
                .unwrap();
        }

        let newest_first = store
            .signatures_for_address(&payer.pubkey(), None, 10)
            .unwrap();
        let slots: Vec<u64> = newest_first.iter().map(|(_, s, _)| *s).collect();
        assert_eq!(slots, vec![2, 1, 0]);

        // Pagination: strictly older than slot 2.
        let older = store
            .signatures_for_address(&payer.pubkey(), Some((2, 0)), 10)
            .unwrap();
        let slots: Vec<u64> = older.iter().map(|(_, s, _)| *s).collect();
        assert_eq!(slots, vec![1, 0]);
    }
}
