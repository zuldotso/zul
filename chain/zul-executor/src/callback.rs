//! The SVM account-loading callback backed by the ZUL store plus a
//! per-block overlay (sysvar refreshes, builtin accounts, and results of
//! earlier blocks in the same process lifetime are visible through it).

use zul_store::{Store, StoredAccount};
use solana_sdk::{
    account::{AccountSharedData, ReadableAccount},
    clock::Slot,
    pubkey::Pubkey,
};
use solana_svm_callback::{InvokeContextCallback, TransactionProcessingCallback};
use std::collections::HashMap;
use std::sync::RwLock;

pub struct StoreCallback<'a> {
    store: &'a Store,
    /// Pending account states layered over the committed store.
    overlay: RwLock<HashMap<Pubkey, AccountSharedData>>,
}

impl<'a> StoreCallback<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self {
            store,
            overlay: RwLock::new(HashMap::new()),
        }
    }

    pub fn set_account(&self, pubkey: Pubkey, account: AccountSharedData) {
        self.overlay.write().unwrap().insert(pubkey, account);
    }

    pub fn get(&self, pubkey: &Pubkey) -> Option<AccountSharedData> {
        if let Some(account) = self.overlay.read().unwrap().get(pubkey) {
            return Some(account.clone());
        }
        self.store
            .get_account(pubkey)
            .ok()
            .flatten()
            .map(|stored| stored.to_shared())
    }

    /// Current lamports as seen through the overlay (0 if absent).
    pub fn lamports(&self, pubkey: &Pubkey) -> u64 {
        self.get(pubkey).map(|a| a.lamports()).unwrap_or(0)
    }

    /// Drain the overlay into (pubkey, Option<StoredAccount>) writes for the
    /// store commit. Zero-lamport accounts become deletions.
    pub fn take_overlay_writes(&self) -> Vec<(Pubkey, Option<StoredAccount>)> {
        let mut overlay = self.overlay.write().unwrap();
        overlay
            .drain()
            .map(|(pubkey, account)| {
                if account.lamports() == 0 {
                    (pubkey, None)
                } else {
                    (pubkey, Some(StoredAccount::from_shared(&account)))
                }
            })
            .collect()
    }
}

impl InvokeContextCallback for StoreCallback<'_> {}

impl TransactionProcessingCallback for StoreCallback<'_> {
    fn get_account_shared_data(&self, pubkey: &Pubkey) -> Option<(AccountSharedData, Slot)> {
        self.get(pubkey).map(|account| (account, 0))
    }
}
