//! On-chain verification of ZUL state-SMT proofs. This MUST stay
//! byte-for-byte identical to chain/zul-store/src/smt.rs and
//! chain/zul-store/src/account.rs — the L1 reconstructs the exact same
//! blake3 hashes the L2 committed, so a proof the node produced verifies
//! here against a posted state root.

// Shallow tree: the settlement only verifies WITHDRAWAL inclusion proofs (the
// L2's separate depth-32 withdrawals tree), not the depth-256 state SMT. At 32
// levels the on-chain blake3 verify (~32 hashes) fits Solana's compute budget;
// depth-256 did not. Must match chain/zul-node/src/withdraw_smt.rs.
pub const SMT_DEPTH: usize = 32;

const LEAF_DOMAIN: &[u8] = b"PL2:smt:leaf";
const NODE_DOMAIN: &[u8] = b"PL2:smt:node";
const EMPTY_DOMAIN: &[u8] = b"PL2:smt:empty";
const KEY_DOMAIN: &[u8] = b"PL2:smt:key";

pub type H256 = [u8; 32];

/// blake3 over the concatenated slices, via the pure-Rust `blake3` crate
/// rather than the `sol_blake3` syscall (which devnet's loader will not
/// resolve at deploy time). Byte-identical to the L2's `blake3_32`, so a
/// proof the node produced still verifies here — the `*_matches_l2_constant`
/// tests below pin that equivalence.
fn hashv(slices: &[&[u8]]) -> H256 {
    let mut hasher = blake3::Hasher::new();
    for slice in slices {
        hasher.update(slice);
    }
    *hasher.finalize().as_bytes()
}

fn node_hash(left: &H256, right: &H256) -> H256 {
    hashv(&[NODE_DOMAIN, left, right])
}

/// Precomputed empty-subtree hashes (index 256 = empty leaf, 0 = empty root),
/// embedded so `claim_withdrawal` reads them instead of recomputing 256 blake3
/// hashes on-chain — that recomputation alone exceeds Solana's compute budget
/// (pure-Rust blake3, no syscall). Regenerate with the `dump_empties` test if
/// the hashing domains ever change; `empties_match_computed` guards staleness.
static EMPTIES: &[u8; (SMT_DEPTH + 1) * 32] = include_bytes!("empties.bin");

/// Empty-subtree hashes, read from the embedded table (cheap).
fn empty_hashes() -> Vec<H256> {
    EMPTIES
        .chunks_exact(32)
        .map(|c| {
            let mut h = [0u8; 32];
            h.copy_from_slice(c);
            h
        })
        .collect()
}

/// The same table computed from scratch (256 blake3 hashes) — used only to
/// regenerate / validate the embedded `empties.bin`, never on-chain.
#[inline(never)]
fn compute_empty_hashes() -> Vec<H256> {
    let mut e = vec![[0u8; 32]; SMT_DEPTH + 1];
    e[SMT_DEPTH] = hashv(&[EMPTY_DOMAIN, &[]]);
    let mut d = SMT_DEPTH;
    while d > 0 {
        d -= 1;
        e[d] = node_hash(&e[d + 1], &e[d + 1]);
    }
    e
}

pub fn key_path(pubkey: &[u8; 32]) -> H256 {
    hashv(&[KEY_DOMAIN, pubkey])
}

pub fn leaf_hash(pubkey: &[u8; 32], account_hash: &H256) -> H256 {
    hashv(&[LEAF_DOMAIN, pubkey, account_hash])
}

#[inline]
fn path_bit(path: &H256, bit: usize) -> bool {
    (path[bit / 8] >> (7 - (bit % 8))) & 1 == 1
}

/// Compact proof: default siblings elided via the bitmap (bit i, MSB-first,
/// covers the sibling at depth 256 - i).
pub struct SmtProof<'a> {
    pub bitmap: [u8; 32],
    pub siblings: &'a [H256],
}

/// Verify inclusion of `(pubkey, account_hash)` under `root`.
pub fn verify(root: &H256, pubkey: &[u8; 32], account_hash: &H256, proof: &SmtProof) -> bool {
    let empties = empty_hashes();
    let path = key_path(pubkey);
    let mut current = leaf_hash(pubkey, account_hash);
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

/// Withdrawal commitment: the account hash the L2 bridge commits for a
/// pending withdrawal `(recipient, amount, nonce)`. Mirrors the L2 side.
const WITHDRAW_DOMAIN: &[u8] = b"PL2:bridge:withdraw:v1";
const WITHDRAW_ADDR_DOMAIN: &[u8] = b"PL2:bridge:withdraw-addr:v1";

pub fn withdrawal_account_hash(
    recipient: &[u8; 32],
    asset_id: &[u8; 32],
    amount: u64,
    nonce: u64,
) -> H256 {
    hashv(&[
        WITHDRAW_DOMAIN,
        recipient,
        asset_id,
        &amount.to_le_bytes(),
        &nonce.to_le_bytes(),
    ])
}

/// Deterministic L2 address of the withdrawal account for
/// `(recipient, nonce)`. The L2 bridge inserts a leaf here whose
/// account hash is `withdrawal_account_hash`.
pub fn withdrawal_pubkey(recipient: &[u8; 32], nonce: u64) -> [u8; 32] {
    hashv(&[WITHDRAW_ADDR_DOMAIN, recipient, &nonce.to_le_bytes()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_root_is_stable() {
        let e1 = empty_hashes();
        let e2 = empty_hashes();
        assert_eq!(e1[0], e2[0]);
        assert_ne!(e1[0], e1[SMT_DEPTH]);
    }

    // One-off: regenerate src/empties.bin (the precomputed empty-subtree
    // hashes the program embeds so `claim_withdrawal` doesn't recompute 256
    // blake3 hashes and blow the compute budget). Run with --ignored to refresh.
    #[test]
    #[ignore]
    fn dump_empties() {
        let flat: Vec<u8> = compute_empty_hashes().iter().flatten().copied().collect();
        assert_eq!(flat.len(), (SMT_DEPTH + 1) * 32);
        std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/src/empties.bin"), &flat).unwrap();
    }

    #[test]
    fn empties_match_computed() {
        // The embedded table must equal the from-scratch computation.
        assert_eq!(empty_hashes(), compute_empty_hashes());
    }

    #[test]
    fn absent_leaf_does_not_verify_against_empty_root() {
        let empties = empty_hashes();
        let pubkey = [3u8; 32];
        let account_hash = [9u8; 32];
        let proof = SmtProof {
            bitmap: [0u8; 32],
            siblings: &[],
        };
        // Against the empty root, a real leaf must NOT verify.
        assert!(!verify(&empties[0], &pubkey, &account_hash, &proof));
    }

    #[test]
    fn withdrawal_hash_binds_all_fields() {
        let r = [1u8; 32];
        let native = [0u8; 32];
        let base = withdrawal_account_hash(&r, &native, 100, 0);
        assert_ne!(base, withdrawal_account_hash(&r, &native, 101, 0));
        assert_ne!(base, withdrawal_account_hash(&r, &native, 100, 1));
        assert_ne!(base, withdrawal_account_hash(&[2u8; 32], &native, 100, 0));
        // The asset id is bound: a different mint yields a different commitment.
        assert_ne!(base, withdrawal_account_hash(&r, &[9u8; 32], 100, 0));
    }

    // Cross-pinned with chain/zul-node/src/bridge.rs: the L2 computes these
    // exact bytes when committing a withdrawal leaf, so a node-produced proof
    // verifies here against a posted state root. If either side's domain or
    // encoding drifts, this breaks.
    #[test]
    fn withdrawal_hashes_match_l2() {
        let recipient = [7u8; 32];
        // Native asset id ([0u8;32]); SPL withdrawals bind the L1 mint instead.
        assert_eq!(
            withdrawal_account_hash(&recipient, &[0u8; 32], 1000, 5),
            hex32("29002aa04988b6423f4ac5f5ba397f68a7a49dbc6eb0c985fe9dbf72301c6729")
        );
        assert_eq!(
            withdrawal_pubkey(&recipient, 5),
            hex32("fc7e27778b8a6c585cfdbfb66ea01e059a935d09ee5646d46b9658573c9cf920")
        );
    }

    // Pinned to chain/zul-store/src/smt.rs::crosscheck. If the L1 and L2
    // blake3 SMT implementations ever diverge, one of these breaks. This is
    // what makes the on-chain withdrawal verify trustworthy against an
    // L2-produced proof.
    fn hex32(s: &str) -> H256 {
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap();
        }
        out
    }

    #[test]
    fn empty_root_matches_l2_withdraw_tree() {
        // Depth-32 empty root, cross-pinned with chain/zul-node/src/withdraw_smt.rs.
        let expected =
            hex32("3bab6c9cd18a2e6b4051115ca91d5cf4058e657824acf03763ff557bd60b3305");
        assert_eq!(empty_hashes()[0], expected);
    }

    #[test]
    fn sample_leaf_matches_l2_constant() {
        let expected =
            hex32("1cae1d9580f6c55d17103fa75bffa94cb045df11939631c7b2dfe0816d22e315");
        assert_eq!(leaf_hash(&[7u8; 32], &[9u8; 32]), expected);
    }
}
