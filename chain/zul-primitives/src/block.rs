//! Block and header types.
//!
//! The header hash doubles as the L2 "blockhash" exposed over Solana RPC
//! (`getLatestBlockhash`), so wallets sign transactions against it exactly
//! as they would against an L1 recent blockhash.

use serde::{Deserialize, Serialize};
use solana_sdk::{
    hash::Hash,
    pubkey::Pubkey,
    signature::{Keypair, Signature},
    signer::Signer,
    transaction::VersionedTransaction,
};

use crate::hash::{blake3_32, H256};
use crate::merkle;

const HEADER_DOMAIN: &[u8] = b"PL2:block:header:v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockHeader {
    /// Monotonic slot number; one block per slot.
    pub slot: u64,
    /// Header hash of the previous block; genesis hash for slot 0.
    pub parent_hash: H256,
    /// Sparse-merkle root over all accounts after executing this block.
    pub state_root: H256,
    /// Merkle root over the serialized transactions in this block.
    pub transactions_root: H256,
    /// Wall-clock time at production, unix millis.
    pub timestamp_ms: u64,
    /// Number of transactions in the block body.
    pub transaction_count: u32,
}

impl BlockHeader {
    /// Canonical header hash. bincode of this struct is deterministic:
    /// fixed-width integers and fixed-size arrays only.
    pub fn hash(&self) -> H256 {
        let bytes = bincode::serialize(self).expect("header serialization is infallible");
        blake3_32(HEADER_DOMAIN, &bytes)
    }

    /// The header hash viewed as a Solana `Hash` for RPC interop.
    pub fn blockhash(&self) -> Hash {
        Hash::new_from_array(self.hash())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub transactions: Vec<VersionedTransaction>,
    /// Ed25519 signature by the sequencer over the header hash.
    pub sequencer_signature: Signature,
}

impl Block {
    /// Compute the transactions root for a body.
    pub fn transactions_root(transactions: &[VersionedTransaction]) -> H256 {
        let serialized: Vec<Vec<u8>> = transactions
            .iter()
            .map(|tx| bincode::serialize(tx).expect("tx serialization is infallible"))
            .collect();
        merkle::root_from_data(serialized.iter().map(|v| v.as_slice()))
    }

    /// Assemble and sign a block. The header must already commit to the
    /// given transactions (`transactions_root`, `transaction_count`).
    pub fn seal(
        header: BlockHeader,
        transactions: Vec<VersionedTransaction>,
        sequencer: &Keypair,
    ) -> Self {
        debug_assert_eq!(header.transaction_count as usize, transactions.len());
        debug_assert_eq!(
            header.transactions_root,
            Self::transactions_root(&transactions)
        );
        let sequencer_signature = sequencer.sign_message(&header.hash());
        Self {
            header,
            transactions,
            sequencer_signature,
        }
    }

    /// Verify body integrity and the sequencer signature.
    pub fn verify(&self, sequencer: &Pubkey) -> bool {
        if self.header.transaction_count as usize != self.transactions.len() {
            return false;
        }
        if self.header.transactions_root != Self::transactions_root(&self.transactions) {
            return false;
        }
        self.sequencer_signature
            .verify(sequencer.as_ref(), &self.header.hash())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::ZERO_HASH;

    fn header() -> BlockHeader {
        BlockHeader {
            slot: 7,
            parent_hash: ZERO_HASH,
            state_root: [1u8; 32],
            transactions_root: Block::transactions_root(&[]),
            timestamp_ms: 1_760_000_000_000,
            transaction_count: 0,
        }
    }

    #[test]
    fn header_hash_is_deterministic_and_sensitive() {
        let h = header();
        assert_eq!(h.hash(), h.hash());
        let mut h2 = header();
        h2.slot += 1;
        assert_ne!(h.hash(), h2.hash());
    }

    #[test]
    fn seal_and_verify() {
        let kp = Keypair::new();
        let block = Block::seal(header(), vec![], &kp);
        assert!(block.verify(&kp.pubkey()));
        assert!(!block.verify(&Keypair::new().pubkey()));
    }

    #[test]
    fn tampered_header_fails_verification() {
        let kp = Keypair::new();
        let mut block = Block::seal(header(), vec![], &kp);
        block.header.state_root = [9u8; 32];
        assert!(!block.verify(&kp.pubkey()));
    }

    #[test]
    fn block_bincode_roundtrip() {
        let kp = Keypair::new();
        let block = Block::seal(header(), vec![], &kp);
        let bytes = bincode::serialize(&block).unwrap();
        let back: Block = bincode::deserialize(&bytes).unwrap();
        assert_eq!(back.header, block.header);
        assert_eq!(back.sequencer_signature, block.sequencer_signature);
    }
}
