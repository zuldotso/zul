//! ZUL persistent state: accounts and chain metadata in a redb database.
//! Blocks, the blockhash queue, and the state SMT land on top of this.

pub mod account;
pub mod error;
pub mod store;

pub use account::{StoredAccount, RENT_EXEMPT_RENT_EPOCH};
pub use error::{Result, StoreError};
pub use store::Store;
