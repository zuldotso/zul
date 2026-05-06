//! Poseidon hashing (circom-compatible via light-poseidon) and the note
//! commitment / nullifier scheme.
//!
//! Scheme (mirrored exactly by circuits/ and the TS SDK):
//! - spending key `s` (field), public key `pk = Poseidon1(s)`
//! - note = (asset_id, amount, pk, blinding)
//! - secret     = Poseidon2(pk, blinding)        (hides owner + blinding)
//! - commitment = Poseidon3(asset_id, amount, secret)
//! - nullifier  = Poseidon3(commitment, leaf_index, s)
//! - tree node  = Poseidon2(left, right)
//! - asset_id: 0 for native ZUL; for SPL assets, the mint pubkey reduced
//!   into the field via little-endian interpretation mod p.
//!
//! The two-level commitment is what lets `shield` be proofless yet sound:
//! the depositor reveals `secret` (which hides pk and blinding) and the
//! node computes `commitment = Poseidon3(asset_id, amount, secret)` from the
//! PUBLIC asset and amount — so the committed value is bound to the funds
//! actually deposited. A flat `Poseidon(asset, amount, pk, blinding)` could
//! not be recomputed on-chain (pk/blinding are secret), which would let a
//! deposit of 1 mint a note worth 1000.

use ark_bn254::Fr;
use ark_ff::PrimeField;
use light_poseidon::{Poseidon, PoseidonHasher};
use solana_sdk::pubkey::Pubkey;

/// Native ZUL asset id.
pub fn native_asset_id() -> Fr {
    Fr::from(0u64)
}

/// SPL mint -> field asset id (LE bytes reduced mod p).
pub fn asset_id_for_mint(mint: &Pubkey) -> Fr {
    Fr::from_le_bytes_mod_order(mint.as_ref())
}

pub fn hash1(a: Fr) -> Fr {
    Poseidon::<Fr>::new_circom(1)
        .expect("poseidon params")
        .hash(&[a])
        .expect("poseidon hash")
}

pub fn hash2(a: Fr, b: Fr) -> Fr {
    Poseidon::<Fr>::new_circom(2)
        .expect("poseidon params")
        .hash(&[a, b])
        .expect("poseidon hash")
}

pub fn hash3(a: Fr, b: Fr, c: Fr) -> Fr {
    Poseidon::<Fr>::new_circom(3)
        .expect("poseidon params")
        .hash(&[a, b, c])
        .expect("poseidon hash")
}

pub fn hash4(a: Fr, b: Fr, c: Fr, d: Fr) -> Fr {
    Poseidon::<Fr>::new_circom(4)
        .expect("poseidon params")
        .hash(&[a, b, c, d])
        .expect("poseidon hash")
}

/// Owner+blinding secret, revealed publicly at shield time (hides both via
/// Poseidon preimage resistance).
pub fn note_secret(owner_pk: Fr, blinding: Fr) -> Fr {
    hash2(owner_pk, blinding)
}

/// Commitment from the public (asset, amount) and the opaque secret. This is
/// the form the node recomputes at shield time to bind the amount.
pub fn note_commitment_from_secret(asset_id: Fr, amount: u64, secret: Fr) -> Fr {
    hash3(asset_id, Fr::from(amount), secret)
}

/// Full-knowledge commitment (used by wallets/tests that hold pk + blinding).
pub fn note_commitment(asset_id: Fr, amount: u64, owner_pk: Fr, blinding: Fr) -> Fr {
    note_commitment_from_secret(asset_id, amount, note_secret(owner_pk, blinding))
}

pub fn nullifier(commitment: Fr, leaf_index: u64, spending_key: Fr) -> Fr {
    hash3(commitment, Fr::from(leaf_index), spending_key)
}

pub fn tree_node(left: Fr, right: Fr) -> Fr {
    hash2(left, right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashing_is_deterministic_and_arity_separated() {
        let a = Fr::from(1u64);
        let b = Fr::from(2u64);
        assert_eq!(hash2(a, b), hash2(a, b));
        assert_ne!(hash2(a, b), hash2(b, a));
        assert_ne!(hash1(a), hash2(a, Fr::from(0u64)));
    }

    #[test]
    fn commitment_binds_all_fields() {
        let pk = hash1(Fr::from(42u64));
        let base = note_commitment(native_asset_id(), 100, pk, Fr::from(7u64));
        assert_ne!(
            base,
            note_commitment(native_asset_id(), 101, pk, Fr::from(7u64))
        );
        assert_ne!(
            base,
            note_commitment(native_asset_id(), 100, pk, Fr::from(8u64))
        );
        let mint = Pubkey::new_unique();
        assert_ne!(
            base,
            note_commitment(asset_id_for_mint(&mint), 100, pk, Fr::from(7u64))
        );
    }

    #[test]
    fn nullifiers_differ_per_leaf_and_key() {
        let c = Fr::from(9u64);
        let s = Fr::from(11u64);
        assert_ne!(nullifier(c, 0, s), nullifier(c, 1, s));
        assert_ne!(nullifier(c, 0, s), nullifier(c, 0, Fr::from(12u64)));
    }
}
