//! ZUL data-availability log.
//!
//! Batch transaction data is published as instruction data to this program;
//! the bytes live permanently in the L1 ledger. The program itself stores
//! nothing — it only validates the chunk header and emits an event so the
//! data can be located and reassembled (verified against the batch's
//! `da_hash` recorded by the settlement program).

use anchor_lang::prelude::*;

declare_id!("3E3dFuiYPi8BXGQo2SUzU5ctZ41U9ARCiqAuNTUjnk2f");

/// Solana caps transaction size at 1232 bytes; keep chunks well under it.
pub const MAX_CHUNK_BYTES: usize = 900;

#[program]
pub mod zul_da_log {
    use super::*;

    /// Publish one DA chunk. `data` is the raw (already zstd-compressed)
    /// slice of the batch payload; the program asserts only its size.
    pub fn post_chunk(
        _ctx: Context<PostChunk>,
        batch_no: u64,
        chunk_index: u32,
        chunk_count: u32,
        data: Vec<u8>,
    ) -> Result<()> {
        require!(!data.is_empty(), DaError::EmptyChunk);
        require!(data.len() <= MAX_CHUNK_BYTES, DaError::ChunkTooLarge);
        require!(chunk_index < chunk_count, DaError::IndexOutOfRange);

        emit!(ChunkPosted {
            batch_no,
            chunk_index,
            chunk_count,
            len: data.len() as u32,
        });
        Ok(())
    }
}

#[derive(Accounts)]
pub struct PostChunk<'info> {
    /// The sequencer (or anyone — DA is permissionless to publish).
    pub poster: Signer<'info>,
}

#[event]
pub struct ChunkPosted {
    pub batch_no: u64,
    pub chunk_index: u32,
    pub chunk_count: u32,
    pub len: u32,
}

#[error_code]
pub enum DaError {
    #[msg("chunk must not be empty")]
    EmptyChunk,
    #[msg("chunk exceeds the size limit")]
    ChunkTooLarge,
    #[msg("chunk index out of range")]
    IndexOutOfRange,
}
