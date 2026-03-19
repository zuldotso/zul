//! Chain-wide constants.

/// Decimals of the native ZUL token (mirrors SOL so SPL tooling behaves).
pub const NATIVE_DECIMALS: u8 = 9;

/// Lamports per whole ZUL.
pub const LAMPORTS_PER_ZUL: u64 = 1_000_000_000;

/// Default slot duration when the node config does not override it.
pub const DEFAULT_SLOT_DURATION_MS: u64 = 500;

/// How many recent blockhashes remain valid for transaction recency checks.
/// Matches Solana's `MAX_RECENT_BLOCKHASHES` so wallet retry logic behaves
/// identically against the L2.
pub const BLOCKHASH_QUEUE_CAPACITY: usize = 150;

/// Base fee in lamports per signature (Solana mainnet default).
pub const LAMPORTS_PER_SIGNATURE: u64 = 5_000;
