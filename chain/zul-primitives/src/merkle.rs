//! Binary blake3 merkle tree used for the per-block transactions root.
//!
//! Leaves and internal nodes are domain-separated to rule out
//! second-preimage tricks (an internal node can never be reinterpreted as a
//! leaf). An odd node at any level is promoted unchanged to the next level;
//! this is unambiguous because node positions are fixed by the leaf count,
//! which is committed in the block header.

use crate::hash::{blake3_concat, blake3_32, H256};

const LEAF_DOMAIN: &[u8] = b"PL2:merkle:leaf";
const NODE_DOMAIN: &[u8] = b"PL2:merkle:node";
const EMPTY_DOMAIN: &[u8] = b"PL2:merkle:empty";

/// Hash raw leaf content into a leaf node.
pub fn leaf_hash(data: &[u8]) -> H256 {
    blake3_32(LEAF_DOMAIN, data)
}

/// Root of an empty tree (e.g. a block with no transactions).
pub fn empty_root() -> H256 {
    blake3_32(EMPTY_DOMAIN, &[])
}

/// Compute the merkle root over already-hashed leaves.
pub fn root_from_leaves(leaves: &[H256]) -> H256 {
    if leaves.is_empty() {
        return empty_root();
    }
    let mut level: Vec<H256> = leaves.to_vec();
    while level.len() > 1 {
        let mut next = Vec::with_capacity(level.len() / 2 + 1);
        for pair in level.chunks(2) {
            match pair {
                [a, b] => next.push(blake3_concat(NODE_DOMAIN, &[a, b])),
                [a] => next.push(*a),
                _ => unreachable!(),
            }
        }
        level = next;
    }
    level[0]
}

/// Convenience: root over raw (unhashed) leaf payloads.
pub fn root_from_data<'a, I: IntoIterator<Item = &'a [u8]>>(items: I) -> H256 {
    let leaves: Vec<H256> = items.into_iter().map(leaf_hash).collect();
    root_from_leaves(&leaves)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_stable_and_distinct() {
        assert_eq!(empty_root(), empty_root());
        assert_ne!(empty_root(), root_from_data([b"x".as_slice()]));
    }

    #[test]
    fn single_leaf_root_is_leaf() {
        let l = leaf_hash(b"only");
        assert_eq!(root_from_leaves(&[l]), l);
    }

    #[test]
    fn order_matters() {
        let a = root_from_data([b"a".as_slice(), b"b".as_slice()]);
        let b = root_from_data([b"b".as_slice(), b"a".as_slice()]);
        assert_ne!(a, b);
    }

    #[test]
    fn odd_counts_are_deterministic() {
        let r1 = root_from_data([b"a".as_slice(), b"b".as_slice(), b"c".as_slice()]);
        let r2 = root_from_data([b"a".as_slice(), b"b".as_slice(), b"c".as_slice()]);
        assert_eq!(r1, r2);
        // and differ from the 2-leaf tree
        let r3 = root_from_data([b"a".as_slice(), b"b".as_slice()]);
        assert_ne!(r1, r3);
    }

    #[test]
    fn leaf_cannot_impersonate_node() {
        // A leaf whose content equals the concatenation of two node hashes
        // must not collide with their parent, thanks to domain separation.
        let a = leaf_hash(b"a");
        let b = leaf_hash(b"b");
        let parent = root_from_leaves(&[a, b]);
        let mut concat = Vec::new();
        concat.extend_from_slice(&a);
        concat.extend_from_slice(&b);
        assert_ne!(leaf_hash(&concat), parent);
    }
}
