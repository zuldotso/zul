//! Sparse merkle tree over all accounts: (pubkey -> account hash).
//!
//! Depth-256 binary tree addressed by `blake3(pubkey)` path bits, with
//! precomputed empty-subtree hashes so only touched nodes are stored.
//! The root is the per-block `state_root`; inclusion proofs back withdrawal
//! claims on L1 (the settlement program re-verifies with the same
//! conventions via Solana's blake3 syscall).
//!
//! All functions here are pure over a `NodeSource`; persistence lives in
//! the `Store`.

use zul_primitives::hash::{blake3_32, blake3_concat, H256};
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::sync::OnceLock;

pub const SMT_DEPTH: usize = 256;

/// 2-byte BE depth || path masked to `depth` bits.
pub type NodeKey = [u8; 34];

const LEAF_DOMAIN: &[u8] = b"PL2:smt:leaf";
const NODE_DOMAIN: &[u8] = b"PL2:smt:node";
const EMPTY_DOMAIN: &[u8] = b"PL2:smt:empty";
const KEY_DOMAIN: &[u8] = b"PL2:smt:key";

/// `empty_hashes()[d]` = root hash of an empty subtree at depth `d`.
/// Index `SMT_DEPTH` is the empty leaf, index 0 the root of an empty tree.
pub fn empty_hashes() -> &'static [H256; SMT_DEPTH + 1] {
    static EMPTIES: OnceLock<[H256; SMT_DEPTH + 1]> = OnceLock::new();
    EMPTIES.get_or_init(|| {
        let mut e = [[0u8; 32]; SMT_DEPTH + 1];
        e[SMT_DEPTH] = blake3_32(EMPTY_DOMAIN, &[]);
        for d in (0..SMT_DEPTH).rev() {
            e[d] = node_hash(&e[d + 1], &e[d + 1]);
        }
        e
    })
}

pub fn empty_root() -> H256 {
    empty_hashes()[0]
}

pub fn node_hash(left: &H256, right: &H256) -> H256 {
    blake3_concat(NODE_DOMAIN, &[left, right])
}

/// Tree position of an account: uniformly distributed path bits.
pub fn key_path(pubkey: &Pubkey) -> H256 {
    blake3_32(KEY_DOMAIN, pubkey.as_ref())
}

/// Leaf node value for a live account.
pub fn leaf_hash(pubkey: &Pubkey, account_hash: &H256) -> H256 {
    blake3_concat(LEAF_DOMAIN, &[pubkey.as_ref(), account_hash])
}

#[inline]
fn path_bit(path: &H256, bit: usize) -> bool {
    (path[bit / 8] >> (7 - (bit % 8))) & 1 == 1
}

#[inline]
fn flip_bit(path: &H256, bit: usize) -> H256 {
    let mut p = *path;
    p[bit / 8] ^= 1 << (7 - (bit % 8));
    p
}

/// Storage key for the node at `depth` covering `path`'s prefix.
pub fn node_key(depth: usize, path: &H256) -> NodeKey {
    debug_assert!(depth <= SMT_DEPTH);
    let mut key = [0u8; 34];
    key[..2].copy_from_slice(&(depth as u16).to_be_bytes());
    let full = depth / 8;
    key[2..2 + full].copy_from_slice(&path[..full]);
    let rem = depth % 8;
    if rem > 0 {
        key[2 + full] = path[full] & (0xffu8 << (8 - rem));
    }
    key
}

/// Read access to persisted (committed) SMT nodes.
pub trait NodeSource {
    fn get_node(&self, key: &NodeKey) -> Option<H256>;
}

impl NodeSource for HashMap<NodeKey, H256> {
    fn get_node(&self, key: &NodeKey) -> Option<H256> {
        self.get(key).copied()
    }
}

/// One account's new leaf state: `Some(account_hash)` or `None` (deleted).
pub type LeafUpdate = (Pubkey, Option<H256>);

/// Result of applying a batch of leaf updates.
pub struct StateUpdate {
    pub new_root: H256,
    /// Dirty nodes to persist. A value equal to the empty hash for its
    /// depth means the node should be deleted from storage (keeps the
    /// persisted tree sparse).
    pub node_writes: Vec<(NodeKey, H256)>,
}

/// Apply `updates` over the committed nodes in `source`. Pure — nothing is
/// written; the caller persists `node_writes` atomically with the block.
pub fn apply_updates<S: NodeSource>(source: &S, updates: &[LeafUpdate]) -> StateUpdate {
    let empties = empty_hashes();
    let mut overlay: HashMap<NodeKey, H256> = HashMap::new();

    let read = |overlay: &HashMap<NodeKey, H256>, depth: usize, path: &H256| -> H256 {
        let k = node_key(depth, path);
        if let Some(h) = overlay.get(&k) {
            return *h;
        }
        source.get_node(&k).unwrap_or(empties[depth])
    };

    for (pubkey, leaf) in updates {
        let path = key_path(pubkey);
        let mut current = match leaf {
            Some(account_hash) => leaf_hash(pubkey, account_hash),
            None => empties[SMT_DEPTH],
        };
        overlay.insert(node_key(SMT_DEPTH, &path), current);
        for depth in (1..=SMT_DEPTH).rev() {
            let bit = depth - 1;
            let sibling = read(&overlay, depth, &flip_bit(&path, bit));
            current = if path_bit(&path, bit) {
                node_hash(&sibling, &current)
            } else {
                node_hash(&current, &sibling)
            };
            overlay.insert(node_key(depth - 1, &path), current);
        }
    }

    let root_key = node_key(0, &[0u8; 32]);
    let new_root = overlay
        .get(&root_key)
        .copied()
        .or_else(|| source.get_node(&root_key))
        .unwrap_or(empties[0]);

    StateUpdate {
        new_root,
        node_writes: overlay.into_iter().collect(),
    }
}

/// Compact inclusion/non-inclusion proof: default siblings are elided via
/// the bitmap. Bit `i` (MSB-first) covers the sibling at depth `256 - i`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmtProof {
    pub bitmap: [u8; 32],
    pub siblings: Vec<H256>,
}

/// Build a proof for `pubkey` against the committed tree in `source`.
pub fn prove<S: NodeSource>(source: &S, pubkey: &Pubkey) -> SmtProof {
    let empties = empty_hashes();
    let path = key_path(pubkey);
    let mut bitmap = [0u8; 32];
    let mut siblings = Vec::new();
    for i in 0..SMT_DEPTH {
        let depth = SMT_DEPTH - i;
        let bit = depth - 1;
        let sibling = source
            .get_node(&node_key(depth, &flip_bit(&path, bit)))
            .unwrap_or(empties[depth]);
        if sibling != empties[depth] {
            bitmap[i / 8] |= 1 << (7 - (i % 8));
            siblings.push(sibling);
        }
    }
    SmtProof { bitmap, siblings }
}

/// Verify a proof against `root`. `account_hash = None` proves absence.
pub fn verify(
    root: &H256,
    pubkey: &Pubkey,
    account_hash: Option<&H256>,
    proof: &SmtProof,
) -> bool {
    let empties = empty_hashes();
    let path = key_path(pubkey);
    let mut current = match account_hash {
        Some(ah) => leaf_hash(pubkey, ah),
        None => empties[SMT_DEPTH],
    };
    let mut cursor = 0usize;
    for i in 0..SMT_DEPTH {
        let depth = SMT_DEPTH - i;
        let bit = depth - 1;
        let sibling = if (proof.bitmap[i / 8] >> (7 - (i % 8))) & 1 == 1 {
            match proof.siblings.get(cursor) {
                Some(s) => {
                    cursor += 1;
                    *s
                }
                None => return false,
            }
        } else {
            empties[depth]
        };
        current = if path_bit(&path, bit) {
            node_hash(&sibling, &current)
        } else {
            node_hash(&current, &sibling)
        };
    }
    cursor == proof.siblings.len() && current == *root
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(n: u8) -> H256 {
        [n; 32]
    }

    /// Persist a StateUpdate into the test HashMap, honoring deletions.
    fn persist(map: &mut HashMap<NodeKey, H256>, update: &StateUpdate) {
        let empties = empty_hashes();
        for (key, value) in &update.node_writes {
            let depth = u16::from_be_bytes([key[0], key[1]]) as usize;
            if *value == empties[depth] {
                map.remove(key);
            } else {
                map.insert(*key, *value);
            }
        }
    }

    #[test]
    fn empty_tree_root_is_stable() {
        assert_eq!(empty_root(), empty_root());
        let map = HashMap::new();
        let u = apply_updates(&map, &[]);
        assert_eq!(u.new_root, empty_root());
    }

    #[test]
    fn insert_then_verify_inclusion_and_absence() {
        let mut map = HashMap::new();
        let pk = Pubkey::new_unique();
        let other = Pubkey::new_unique();

        let u = apply_updates(&map, &[(pk, Some(h(7)))]);
        assert_ne!(u.new_root, empty_root());
        persist(&mut map, &u);

        let proof = prove(&map, &pk);
        assert!(verify(&u.new_root, &pk, Some(&h(7)), &proof));
        assert!(!verify(&u.new_root, &pk, Some(&h(8)), &proof));
        assert!(!verify(&u.new_root, &pk, None, &proof));

        let absent = prove(&map, &other);
        assert!(verify(&u.new_root, &other, None, &absent));
        assert!(!verify(&u.new_root, &other, Some(&h(7)), &absent));
    }

    #[test]
    fn root_is_insertion_order_independent() {
        let a = Pubkey::new_unique();
        let b = Pubkey::new_unique();

        let mut m1 = HashMap::new();
        let u = apply_updates(&m1, &[(a, Some(h(1))), (b, Some(h(2)))]);
        persist(&mut m1, &u);
        let r1 = u.new_root;

        let mut m2 = HashMap::new();
        let u_first = apply_updates(&m2, &[(b, Some(h(2)))]);
        persist(&mut m2, &u_first);
        let u_second = apply_updates(&m2, &[(a, Some(h(1)))]);
        persist(&mut m2, &u_second);

        assert_eq!(r1, u_second.new_root);
    }

    #[test]
    fn delete_restores_previous_root() {
        let a = Pubkey::new_unique();
        let b = Pubkey::new_unique();

        let mut map = HashMap::new();
        let u_a = apply_updates(&map, &[(a, Some(h(1)))]);
        persist(&mut map, &u_a);
        let root_a_only = u_a.new_root;

        let u_ab = apply_updates(&map, &[(b, Some(h(2)))]);
        persist(&mut map, &u_ab);
        assert_ne!(u_ab.new_root, root_a_only);

        let u_del = apply_updates(&map, &[(b, None)]);
        persist(&mut map, &u_del);
        assert_eq!(u_del.new_root, root_a_only);
    }

    #[test]
    fn update_changes_root_deterministically() {
        let a = Pubkey::new_unique();
        let mut m = HashMap::new();
        let u1 = apply_updates(&m, &[(a, Some(h(1)))]);
        persist(&mut m, &u1);
        let u2 = apply_updates(&m, &[(a, Some(h(2)))]);
        assert_ne!(u1.new_root, u2.new_root);

        // Same content from scratch yields the same root.
        let fresh = HashMap::new();
        let u3 = apply_updates(&fresh, &[(a, Some(h(2)))]);
        assert_eq!(u2.new_root, u3.new_root);
    }

    #[test]
    fn node_key_masks_path_below_depth() {
        let mut p1 = [0u8; 32];
        p1[0] = 0b1010_0000;
        let mut p2 = p1;
        p2[31] = 0xff; // differs only beyond the prefix
        assert_eq!(node_key(3, &p1), node_key(3, &p2));
        assert_ne!(node_key(3, &p1), node_key(4, &p1));
        // Root key collapses every path.
        assert_eq!(node_key(0, &p1), node_key(0, &[7u8; 32]));
    }

    #[test]
    fn proof_bitmap_compression_is_consistent() {
        let mut map = HashMap::new();
        let keys: Vec<Pubkey> = (0..8).map(|_| Pubkey::new_unique()).collect();
        let updates: Vec<LeafUpdate> =
            keys.iter().map(|k| (*k, Some(h(3)))).collect();
        let u = apply_updates(&map, &updates);
        persist(&mut map, &u);

        for k in &keys {
            let proof = prove(&map, k);
            assert_eq!(
                proof.siblings.len(),
                proof
                    .bitmap
                    .iter()
                    .map(|b| b.count_ones() as usize)
                    .sum::<usize>()
            );
            assert!(verify(&u.new_root, k, Some(&h(3)), &proof));
        }
    }
}

#[cfg(test)]
mod crosscheck {
    use super::*;

    // These constants are ALSO asserted by the L1 settlement program
    // (programs/zul-settlement/src/smt.rs). They must never diverge: the L1
    // verifies proofs the L2 produces, so the two blake3 SMT implementations
    // have to agree byte-for-byte. If either side changes a domain string or
    // the hashing order, one of these assertions breaks first.
    const EMPTY_ROOT: &str =
        "eb350f1a665b18b44daedd2d6935bb02635aca12bf9999f372a5afd80ef42abb";
    const SAMPLE_LEAF: &str =
        "1cae1d9580f6c55d17103fa75bffa94cb045df11939631c7b2dfe0816d22e315";

    #[test]
    fn empty_root_matches_l1_constant() {
        assert_eq!(hex::encode(empty_root()), EMPTY_ROOT);
    }

    #[test]
    fn sample_leaf_matches_l1_constant() {
        let leaf = leaf_hash(&Pubkey::new_from_array([7u8; 32]), &[9u8; 32]);
        assert_eq!(hex::encode(leaf), SAMPLE_LEAF);
    }
}
