//! Persistent account representation and the account hash committed into
//! the state SMT.

use zul_primitives::hash::{blake3_concat, H256};
use serde::{Deserialize, Serialize};
use solana_sdk::{
    account::{Account, AccountSharedData, ReadableAccount},
    pubkey::Pubkey,
};

const ACCOUNT_DOMAIN: &[u8] = b"PL2:account:v1";

/// Rent epoch marker for rent-exempt accounts (all ZUL accounts are; the L2
/// charges no rent collection beyond requiring rent-exempt balances).
pub const RENT_EXEMPT_RENT_EPOCH: u64 = u64::MAX;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredAccount {
    pub lamports: u64,
    pub data: Vec<u8>,
    pub owner: Pubkey,
    pub executable: bool,
    pub rent_epoch: u64,
}

impl StoredAccount {
    pub fn new_system(lamports: u64) -> Self {
        Self {
            lamports,
            data: Vec::new(),
            owner: solana_system_interface::program::id(),
            executable: false,
            rent_epoch: RENT_EXEMPT_RENT_EPOCH,
        }
    }

    pub fn from_shared(account: &AccountSharedData) -> Self {
        Self {
            lamports: account.lamports(),
            data: account.data().to_vec(),
            owner: *account.owner(),
            executable: account.executable(),
            rent_epoch: account.rent_epoch(),
        }
    }

    pub fn to_shared(&self) -> AccountSharedData {
        AccountSharedData::from(Account {
            lamports: self.lamports,
            data: self.data.clone(),
            owner: self.owner,
            executable: self.executable,
            rent_epoch: self.rent_epoch,
        })
    }

    /// Hash committed into the state SMT leaf for this account.
    pub fn hash(&self, pubkey: &Pubkey) -> H256 {
        blake3_concat(
            ACCOUNT_DOMAIN,
            &[
                pubkey.as_ref(),
                &self.lamports.to_le_bytes(),
                self.owner.as_ref(),
                &[self.executable as u8],
                &self.rent_epoch.to_le_bytes(),
                &(self.data.len() as u64).to_le_bytes(),
                &self.data,
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_roundtrip() {
        let a = StoredAccount {
            lamports: 42,
            data: vec![1, 2, 3],
            owner: Pubkey::new_unique(),
            executable: true,
            rent_epoch: RENT_EXEMPT_RENT_EPOCH,
        };
        let back = StoredAccount::from_shared(&a.to_shared());
        assert_eq!(back, a);
    }

    #[test]
    fn hash_depends_on_every_field() {
        let pk = Pubkey::new_unique();
        let base = StoredAccount::new_system(100);
        let h = base.hash(&pk);

        let mut v = base.clone();
        v.lamports = 101;
        assert_ne!(v.hash(&pk), h);

        let mut v = base.clone();
        v.data = vec![0];
        assert_ne!(v.hash(&pk), h);

        let mut v = base.clone();
        v.owner = Pubkey::new_unique();
        assert_ne!(v.hash(&pk), h);

        let mut v = base.clone();
        v.executable = true;
        assert_ne!(v.hash(&pk), h);

        assert_ne!(base.hash(&Pubkey::new_unique()), h);
    }
}
