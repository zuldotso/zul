use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Database(#[from] redb::DatabaseError),
    #[error("transaction error: {0}")]
    Transaction(#[from] redb::TransactionError),
    #[error("table error: {0}")]
    Table(#[from] redb::TableError),
    #[error("storage error: {0}")]
    Storage(#[from] redb::StorageError),
    #[error("commit error: {0}")]
    Commit(#[from] redb::CommitError),
    #[error("codec error: {0}")]
    Codec(#[from] bincode::Error),
    #[error("store invariant violated: {0}")]
    Invariant(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;
