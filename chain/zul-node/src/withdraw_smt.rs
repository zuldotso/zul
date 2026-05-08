//! A shallow (depth-32) sparse Merkle tree of withdrawal commitments, kept in
//! memory by the node. Its root is posted to L1 so `claim_withdrawal` verifies
//! an inclusion proof against it — at depth 32 that's ~32 blake3 hashes, well
//! within Solana's compute budget (the depth-256 state SMT is not). It also
//! sidesteps the state root's churn: this root only changes when a withdrawal
//! is committed, not every block.
//!
//! Reuses the state SMT's hash primitives (`leaf_hash`, `node_hash`,
//! `key_path`, `node_key`) so the leaf/node conventions are identical; only
//! the depth differs. The settlement program mirrors this at `SMT_DEPTH = 32`.

use std::collections::HashMap;
use std::sync::OnceLock;

use zul_primitives::hash::{blake3_32, H256};
use zul_store::smt::{key_path, leaf_hash, node_hash, node_key, NodeKey};
use zul_store::SmtProof;
use solana_sdk::pubkey::Pubkey;

pub const WITHDRAW_DEPTH: usize = 32;
const EMPTY_DOMAIN: &[u8] = b"PL2:smt:empty";

fn empties() -> &'static [H256; WITHDRAW_DEPTH + 1] {
    static E: OnceLock<[H256; WITHDRAW_DEPTH + 1]> = OnceLock::new();
    E.get_or_init(|| {
        let mut e = [[0u8; 32]; WITHDRAW_DEPTH + 1];
        e[WITHDRAW_DEPTH] = blake3_32(EMPTY_DOMAIN, &[]);
        for d in (0..WITHDRAW_DEPTH).rev() {
            e[d] = node_hash(&e[d + 1], &e[d + 1]);
        }
        e
    })
}

fn path_bit(path: &H256, bit: usize) -> bool {
    (path[bit / 8] >> (7 - (bit % 8))) & 1 == 1
}

fn flip_bit(path: &H256, bit: usize) -> H256 {
    let mut p = *path;
    p[bit / 8] ^= 1 << (7 - (bit % 8));
    p
}

/// Insert (or overwrite) a withdrawal leaf and recompute its path to the root.
pub fn apply(tree: &mut HashMap<NodeKey, H256>, pubkey: &Pubkey, account_hash: &H256) {
    let e = empties();
    let path = key_path(pubkey);
    let mut current = leaf_hash(pubkey, account_hash);
    tree.insert(node_key(WITHDRAW_DEPTH, &path), current);
    for depth in (1..=WITHDRAW_DEPTH).rev() {
        let bit = depth - 1;
        let sibling = tree
            .get(&node_key(depth, &flip_bit(&path, bit)))
            .copied()
            .unwrap_or(e[depth]);
        current = if path_bit(&path, bit) {
            node_hash(&sibling, &current)
        } else {
            node_hash(&current, &sibling)
        };
        tree.insert(node_key(depth - 1, &path), current);
    }
}

pub fn root(tree: &HashMap<NodeKey, H256>) -> H256 {
    tree.get(&node_key(0, &[0u8; 32]))
        .copied()
        .unwrap_or(empties()[0])
}

/// Compact inclusion proof (default siblings elided via the bitmap), in the
/// exact format `claim_withdrawal` expects.
pub fn prove(tree: &HashMap<NodeKey, H256>, pubkey: &Pubkey) -> SmtProof {
    let e = empties();
    let path = key_path(pubkey);
    let mut bitmap = [0u8; 32];
    let mut siblings = Vec::new();
    for i in 0..WITHDRAW_DEPTH {
        let depth = WITHDRAW_DEPTH - i;
        let bit = depth - 1;
        let sibling = tree
            .get(&node_key(depth, &flip_bit(&path, bit)))
            .copied()
            .unwrap_or(e[depth]);
        if sibling != e[depth] {
            bitmap[i / 8] |= 1 << (7 - (i % 8));
            siblings.push(sibling);
        }
    }
    SmtProof { bitmap, siblings }
}

/// Verify an inclusion proof against `root` — the same walk the settlement
/// program does on-chain (at `SMT_DEPTH = 32`).
pub fn verify(root: &H256, pubkey: &Pubkey, account_hash: &H256, proof: &SmtProof) -> bool {
    let e = empties();
    let path = key_path(pubkey);
    let mut current = leaf_hash(pubkey, account_hash);
    let mut cursor = 0;
    for i in 0..WITHDRAW_DEPTH {
        let depth = WITHDRAW_DEPTH - i;
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
            e[depth]
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

    // One-off: regenerate the settlement program's depth-32 empties.bin (same
    // hashes; the settlement verifies this exact tree). Run with --ignored.
    #[test]
    #[ignore]
    fn dump_empties_for_settlement() {
        let e = empties();
        let flat: Vec<u8> = e.iter().flatten().copied().collect();
        assert_eq!(flat.len(), (WITHDRAW_DEPTH + 1) * 32);
        std::fs::write(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../programs/programs/zul-settlement/src/empties.bin"
            ),
            &flat,
        )
        .unwrap();
        let root: String = e[0].iter().map(|b| format!("{b:02x}")).collect();
        println!("WITHDRAW_EMPTY_ROOT={root}");
    }

    #[test]
    fn insert_then_prove_roundtrips() {
        let mut tree = HashMap::new();
        let pk = Pubkey::new_unique();
        let ah = [7u8; 32];
        apply(&mut tree, &pk, &ah);
        let r = root(&tree);
        let proof = prove(&tree, &pk);
        // Re-walk the proof to the root (mirrors the settlement's verify).
        let e = empties();
        let path = key_path(&pk);
        let mut current = leaf_hash(&pk, &ah);
        let mut cursor = 0;
        for i in 0..WITHDRAW_DEPTH {
            let depth = WITHDRAW_DEPTH - i;
            let bit = depth - 1;
            let sibling = if (proof.bitmap[i / 8] >> (7 - (i % 8))) & 1 == 1 {
                let s = proof.siblings[cursor];
                cursor += 1;
                s
            } else {
                e[depth]
            };
            current = if path_bit(&path, bit) {
                node_hash(&sibling, &current)
            } else {
                node_hash(&current, &sibling)
            };
        }
        assert_eq!(cursor, proof.siblings.len());
        assert_eq!(current, r);
    }
}
