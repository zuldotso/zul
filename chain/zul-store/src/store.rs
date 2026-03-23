//! The persistent store. One redb database holds accounts and chain
//! metadata. The state SMT and block/tx tables come next; for now this is
//! enough to seed genesis and serve accounts to the executor.

use zul_primitives::{genesis::GenesisConfig, hash::H256};
use redb::{Database, ReadableTable, TableDefinition};
use solana_sdk::pubkey::Pubkey;
use std::path::Path;

use crate::account::StoredAccount;
use crate::error::{Result, StoreError};

const T_ACCOUNTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("accounts");
const T_META: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");

const META_GENESIS_HASH: &str = "genesis_hash";
const META_STATE_ROOT: &str = "state_root";

pub struct Store {
    db: Database,
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
            txn.open_table(T_META)?;
        }
        txn.commit()?;
        Ok(Self { db })
    }

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

    pub fn get_account(&self, pubkey: &Pubkey) -> Result<Option<StoredAccount>> {
        let rtx = self.db.begin_read()?;
        let table = rtx.open_table(T_ACCOUNTS)?;
        match table.get(pubkey.as_ref())? {
            Some(guard) => Ok(Some(bincode::deserialize(guard.value())?)),
            None => Ok(None),
        }
    }

    /// Seed accounts from genesis. The real state root (over the SMT) is not
    /// computed yet; for now we record the genesis hash so re-init is
    /// idempotent. TODO: commit account hashes into the state SMT.
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
            return Ok(expected);
        }

        let txn = self.db.begin_write()?;
        {
            let mut accounts = txn.open_table(T_ACCOUNTS)?;
            for alloc in &genesis.allocations {
                let account = StoredAccount::new_system(alloc.lamports);
                let bytes = bincode::serialize(&account)?;
                accounts.insert(alloc.pubkey.as_ref(), bytes.as_slice())?;
            }
            for (pubkey, account) in extra_accounts {
                let bytes = bincode::serialize(account)?;
                accounts.insert(pubkey.as_ref(), bytes.as_slice())?;
            }
            let mut meta = txn.open_table(T_META)?;
            meta.insert(META_GENESIS_HASH, expected.as_slice())?;
            meta.insert(META_STATE_ROOT, expected.as_slice())?;
        }
        txn.commit()?;
        Ok(expected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GENESIS: &str = r#"
chain_name = "zul-store-test"
creation_time_ms = 1760000000000
sequencer = "E1qwDui9Muq3gfyGwdk1qZXnNyy2WzgHkESJGr4KwJpm"
fee_collector = "6PaVUAPQqyFWxYaLbnVaiwcAkDqiqjCvMMmFoGmNdCFM"

[[allocations]]
pubkey = "Hvd9JUwyRzdm7m7yRcSQBphQtpPNWf5GmCEZRKoWJGSp"
lamports = 1000000000000000000
"#;

    #[test]
    fn genesis_seeds_accounts_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("zul.redb")).unwrap();
        let genesis = GenesisConfig::from_toml_str(GENESIS).unwrap();
        let root = store.initialize_genesis(&genesis, &[]).unwrap();

        let faucet: Pubkey = "Hvd9JUwyRzdm7m7yRcSQBphQtpPNWf5GmCEZRKoWJGSp"
            .parse()
            .unwrap();
        let account = store.get_account(&faucet).unwrap().unwrap();
        assert_eq!(account.lamports, 1_000_000_000_000_000_000);
        assert_eq!(store.initialize_genesis(&genesis, &[]).unwrap(), root);
    }
}
