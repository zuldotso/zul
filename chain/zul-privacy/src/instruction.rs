//! Shielded pool instruction wire format.
//!
//! The pool is an enshrined builtin program. Instructions are bincode of
//! `PoolInstruction`. The first account is always the pool state account;
//! value-moving instructions also reference the pool vault and the user's
//! token/lamport accounts (resolved by the node's privacy processor).

use serde::{Deserialize, Serialize};

use crate::field::FieldBytes;

/// Asset reference at the public boundary (shield / unshield).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Asset {
    /// Native ZUL (lamports).
    Native,
    /// SPL token identified by mint (e.g. wrapped SOL on L2).
    Spl([u8; 32]),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PoolInstruction {
    /// Move public funds into the pool, creating one note commitment.
    /// `amount` of `asset` is debited from the depositor's public account.
    /// The node computes the commitment as
    /// `Poseidon3(asset_id, amount, secret)` so the committed value is bound
    /// to the public amount — the depositor only supplies the opaque
    /// `secret = Poseidon2(owner_pk, blinding)`.
    Shield {
        asset: Asset,
        amount: u64,
        secret: FieldBytes,
        /// Encrypted note payload for the recipient (ECDH + AEAD).
        encrypted_note: Vec<u8>,
    },
    /// Private 2-in/2-out transfer. Consumes two nullifiers, creates two
    /// commitments. No public value moves; an optional public fee is paid
    /// out of the pool to the fee collector.
    Transfer {
        proof: Vec<u8>,
        root: FieldBytes,
        nullifiers: [FieldBytes; 2],
        commitments: [FieldBytes; 2],
        fee: u64,
        encrypted_notes: [Vec<u8>; 2],
    },
    /// Spend a note to a public recipient. Consumes one nullifier, creates
    /// one change commitment, releases `amount` of `asset` publicly.
    Unshield {
        proof: Vec<u8>,
        root: FieldBytes,
        nullifier: FieldBytes,
        change_commitment: FieldBytes,
        asset: Asset,
        amount: u64,
        /// Bound into the proof's public inputs to stop front-running.
        recipient: [u8; 32],
        encrypted_change_note: Vec<u8>,
    },
}

impl PoolInstruction {
    pub fn try_from_slice(data: &[u8]) -> Result<Self, InstructionError> {
        bincode::deserialize(data).map_err(|_| InstructionError::Malformed)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        bincode::serialize(self).expect("instruction serialization is infallible")
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum InstructionError {
    #[error("malformed pool instruction")]
    Malformed,
}

impl Asset {
    pub fn is_native(&self) -> bool {
        matches!(self, Asset::Native)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instruction_roundtrips() {
        let ix = PoolInstruction::Shield {
            asset: Asset::Native,
            amount: 1_000,
            secret: [3u8; 32],
            encrypted_note: vec![1, 2, 3],
        };
        let bytes = ix.to_bytes();
        assert_eq!(PoolInstruction::try_from_slice(&bytes).unwrap(), ix);

        let transfer = PoolInstruction::Transfer {
            proof: vec![9u8; 256],
            root: [1u8; 32],
            nullifiers: [[2u8; 32], [3u8; 32]],
            commitments: [[4u8; 32], [5u8; 32]],
            fee: 5_000,
            encrypted_notes: [vec![0u8; 80], vec![1u8; 80]],
        };
        let bytes = transfer.to_bytes();
        assert_eq!(PoolInstruction::try_from_slice(&bytes).unwrap(), transfer);
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(
            PoolInstruction::try_from_slice(&[0xff, 0xff, 0xff, 0xff]),
            Err(InstructionError::Malformed)
        );
    }
}
