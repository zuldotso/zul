//! Shielded pool on-chain state: an incremental (append-only) Merkle tree
//! of note commitments plus a ring buffer of recent roots, stored in the
//! pool state account. Layout is fixed-size so the account never reallocs.

use ark_bn254::Fr;
use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;

use crate::field::{fr_from_bytes, fr_to_bytes, FieldBytes};
use crate::poseidon::tree_node;

/// Tree depth: 2^26 (~67M) notes capacity.
pub const TREE_DEPTH: usize = 26;
/// How many historical roots remain valid for proofs.
pub const ROOT_HISTORY: usize = 64;

/// Precomputed all-empty subtree hashes, leaf level first.
pub fn zero_hashes() -> &'static [Fr; TREE_DEPTH + 1] {
    use std::sync::OnceLock;
    static ZEROS: OnceLock<[Fr; TREE_DEPTH + 1]> = OnceLock::new();
    ZEROS.get_or_init(|| {
        let mut zeros = [Fr::from(0u64); TREE_DEPTH + 1];
        // Domain-separated empty leaf: keccak-free, just a fixed constant.
        zeros[0] = Fr::from(770_212_417_220_141_184u64); // "PL2:empty" tag (pinned preimage; unchanged for circuit compat)
        for i in 1..=TREE_DEPTH {
            zeros[i] = tree_node(zeros[i - 1], zeros[i - 1]);
        }
        zeros
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolState {
    /// Filled left siblings per level (leaf level first).
    pub filled_subtrees: [FieldBytes; TREE_DEPTH],
    /// Ring buffer of recent roots.
    #[serde(with = "BigArray")]
    pub roots: [FieldBytes; ROOT_HISTORY],
    pub current_root_index: u32,
    /// Index the next inserted leaf will occupy.
    pub next_leaf_index: u32,
}

impl Default for PoolState {
    fn default() -> Self {
        Self::new()
    }
}

impl PoolState {
    pub fn new() -> Self {
        let zeros = zero_hashes();
        let mut state = Self {
            filled_subtrees: [[0u8; 32]; TREE_DEPTH],
            roots: [[0u8; 32]; ROOT_HISTORY],
            current_root_index: 0,
            next_leaf_index: 0,
        };
        for (level, slot) in state.filled_subtrees.iter_mut().enumerate() {
            *slot = fr_to_bytes(&zeros[level]);
        }
        state.roots[0] = fr_to_bytes(&zeros[TREE_DEPTH]);
        state
    }

    pub fn capacity() -> u64 {
        1u64 << TREE_DEPTH
    }

    pub fn current_root(&self) -> FieldBytes {
        self.roots[self.current_root_index as usize]
    }

    pub fn is_known_root(&self, candidate: &FieldBytes) -> bool {
        // Zero bytes can never be a valid root (root of empty tree is a
        // chained Poseidon hash, never zero).
        if candidate == &[0u8; 32] {
            return false;
        }
        self.roots.iter().any(|r| r == candidate)
    }

    /// Append a commitment leaf; returns its leaf index.
    pub fn insert(&mut self, leaf: Fr) -> Result<u32, PoolStateError> {
        let index = self.next_leaf_index;
        if u64::from(index) >= Self::capacity() {
            return Err(PoolStateError::TreeFull);
        }
        let zeros = zero_hashes();
        let mut current = leaf;
        let mut node_index = index;
        for level in 0..TREE_DEPTH {
            if node_index % 2 == 0 {
                // Left child: remember it, pair with the empty subtree.
                self.filled_subtrees[level] = fr_to_bytes(&current);
                current = tree_node(current, zeros[level]);
            } else {
                let left = fr_from_bytes(&self.filled_subtrees[level])
                    .ok_or(PoolStateError::Corrupt)?;
                current = tree_node(left, current);
            }
            node_index /= 2;
        }
        self.current_root_index = (self.current_root_index + 1) % ROOT_HISTORY as u32;
        self.roots[self.current_root_index as usize] = fr_to_bytes(&current);
        self.next_leaf_index += 1;
        Ok(index)
    }

    pub fn serialized_size() -> usize {
        bincode::serialized_size(&Self::new()).expect("fixed size") as usize
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        bincode::serialize(self).expect("pool state serialization is infallible")
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PoolStateError> {
        bincode::deserialize(bytes).map_err(|_| PoolStateError::Corrupt)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PoolStateError {
    #[error("commitment tree is full")]
    TreeFull,
    #[error("pool state account data is corrupt")]
    Corrupt,
}

/// Reference (slow) root computation for testing: builds the whole tree
/// from the given leaves.
#[cfg(test)]
pub fn reference_root(leaves: &[Fr]) -> Fr {
    let zeros = zero_hashes();
    let mut level_nodes: Vec<Fr> = leaves.to_vec();
    for level in 0..TREE_DEPTH {
        let mut next: Vec<Fr> = Vec::with_capacity(level_nodes.len() / 2 + 1);
        let mut i = 0;
        while i < level_nodes.len() {
            let left = level_nodes[i];
            let right = if i + 1 < level_nodes.len() {
                level_nodes[i + 1]
            } else {
                zeros[level]
            };
            next.push(tree_node(left, right));
            i += 2;
        }
        if next.is_empty() {
            next.push(tree_node(zeros[level], zeros[level]));
        }
        level_nodes = next;
    }
    level_nodes[0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::fr_from_bytes;

    #[test]
    fn empty_root_matches_zero_chain() {
        let state = PoolState::new();
        let zeros = zero_hashes();
        assert_eq!(state.current_root(), fr_to_bytes(&zeros[TREE_DEPTH]));
        assert!(state.is_known_root(&state.current_root()));
    }

    #[test]
    fn inserts_match_reference_tree() {
        let mut state = PoolState::new();
        let leaves: Vec<Fr> = (1..=5u64).map(Fr::from).collect();
        for (i, leaf) in leaves.iter().enumerate() {
            let index = state.insert(*leaf).unwrap();
            assert_eq!(index as usize, i);
            let expected = reference_root(&leaves[..=i]);
            assert_eq!(
                fr_from_bytes(&state.current_root()).unwrap(),
                expected,
                "root mismatch after {} inserts",
                i + 1
            );
        }
        assert_eq!(state.next_leaf_index, 5);
    }

    #[test]
    fn root_history_keeps_recent_roots() {
        let mut state = PoolState::new();
        let mut seen_roots = Vec::new();
        for i in 0..10u64 {
            state.insert(Fr::from(i + 100)).unwrap();
            seen_roots.push(state.current_root());
        }
        for root in &seen_roots {
            assert!(state.is_known_root(root));
        }
        assert!(!state.is_known_root(&[0u8; 32]));
        assert!(!state.is_known_root(&fr_to_bytes(&Fr::from(123u64))));
    }

    #[test]
    fn root_history_evicts_after_capacity() {
        let mut state = PoolState::new();
        let initial_root = state.current_root();
        for i in 0..ROOT_HISTORY as u64 {
            state.insert(Fr::from(i + 1)).unwrap();
        }
        // The genesis root has been rotated out exactly now.
        assert!(!state.is_known_root(&initial_root));
    }

    #[test]
    fn serde_roundtrip_is_stable() {
        let mut state = PoolState::new();
        state.insert(Fr::from(77u64)).unwrap();
        let bytes = state.to_bytes();
        assert_eq!(bytes.len(), PoolState::serialized_size());
        let back = PoolState::from_bytes(&bytes).unwrap();
        assert_eq!(back, state);
    }
}
