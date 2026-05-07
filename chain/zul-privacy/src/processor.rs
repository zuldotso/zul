//! Shielded pool state machine.
//!
//! Pure over its inputs: given the current pool state, the nullifier set,
//! the verifying keys, and a `PoolInstruction`, it validates the operation
//! and returns the resulting `PoolEffect` (public fund movements the caller
//! must apply) plus the note events to emit. The SVM builtin adapter and
//! the node wire it to real accounts; this module is fully unit-testable
//! without the runtime.
//!
//! Proof verification is abstracted behind `ProofVerifier` so the state
//! transitions can be exercised with a mock, while the real Groth16 path
//! (`verifier::verify`) is tested independently and bound to the circuits
//! by a cross-language fixture test in Phase 4.

use crate::field::{fr_from_bytes, FieldBytes};
use crate::instruction::{Asset, PoolInstruction};
use crate::poseidon::{asset_id_for_mint, native_asset_id};
use crate::state::PoolState;
use ark_bn254::Fr;
use ark_ff::PrimeField;
use solana_sdk::pubkey::Pubkey;

/// Number of public inputs each circuit exposes, in the documented order.
pub const TRANSFER_PUBLIC_INPUTS: usize = 6; // root, nf0, nf1, cm0, cm1, fee
pub const UNSHIELD_PUBLIC_INPUTS: usize = 6; // root, nf, change_cm, asset_id, amount, recipient

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PoolError {
    #[error("malformed field element in instruction")]
    BadField,
    #[error("unknown or stale merkle root")]
    UnknownRoot,
    #[error("nullifier already spent (double spend)")]
    DoubleSpend,
    #[error("duplicate nullifier within one transaction")]
    DuplicateNullifier,
    #[error("zero amount is not allowed")]
    ZeroAmount,
    #[error("commitment tree is full")]
    TreeFull,
    #[error("proof verification failed")]
    InvalidProof,
    #[error("verifying key unavailable for this operation")]
    NoVerifyingKey,
}

/// A public fund movement the caller applies against real accounts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolEffect {
    /// Move `amount` of `asset` from the depositor into the pool vault.
    DepositToVault { asset: Asset, amount: u64 },
    /// Release `amount` of `asset` from the pool vault to the recipient.
    WithdrawFromVault {
        asset: Asset,
        amount: u64,
        recipient: Pubkey,
    },
    /// Pay a public fee in native ZUL from the vault to the fee collector.
    PayFee { amount: u64 },
}

/// Note ciphertext emitted for wallet discovery (logged by the node).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteEvent {
    pub leaf_index: u32,
    pub commitment: FieldBytes,
    pub encrypted_note: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolOutcome {
    pub effects: Vec<PoolEffect>,
    pub note_events: Vec<NoteEvent>,
}

/// The set of spent nullifiers. On-chain this is a set of PDAs (existence =
/// spent); in tests an in-memory set. The processor only needs membership
/// and insertion.
pub trait NullifierSet {
    fn contains(&self, nullifier: &FieldBytes) -> bool;
    fn insert(&mut self, nullifier: FieldBytes);
}

/// Abstracts Groth16 verification so state logic is testable in isolation.
pub trait ProofVerifier {
    fn verify_transfer(&self, proof: &[u8], public_inputs: &[Fr]) -> bool;
    fn verify_unshield(&self, proof: &[u8], public_inputs: &[Fr]) -> bool;
}

pub struct PoolProcessor<'a, V: ProofVerifier> {
    pub verifier: &'a V,
}

impl<'a, V: ProofVerifier> PoolProcessor<'a, V> {
    pub fn new(verifier: &'a V) -> Self {
        Self { verifier }
    }

    fn asset_id_fr(asset: &Asset) -> Fr {
        match asset {
            Asset::Native => native_asset_id(),
            Asset::Spl(mint) => asset_id_for_mint(&Pubkey::new_from_array(*mint)),
        }
    }

    pub fn process(
        &self,
        state: &mut PoolState,
        nullifiers: &mut dyn NullifierSet,
        instruction: &PoolInstruction,
    ) -> Result<PoolOutcome, PoolError> {
        match instruction {
            PoolInstruction::Shield {
                asset,
                amount,
                secret,
                encrypted_note,
            } => self.shield(state, *asset, *amount, secret, encrypted_note),
            PoolInstruction::Transfer {
                proof,
                root,
                nullifiers: nfs,
                commitments,
                fee,
                encrypted_notes,
            } => self.transfer(
                state,
                nullifiers,
                proof,
                root,
                nfs,
                commitments,
                *fee,
                encrypted_notes,
            ),
            PoolInstruction::Unshield {
                proof,
                root,
                nullifier,
                change_commitment,
                asset,
                amount,
                recipient,
                encrypted_change_note,
            } => self.unshield(
                state,
                nullifiers,
                proof,
                root,
                nullifier,
                change_commitment,
                *asset,
                *amount,
                recipient,
                encrypted_change_note,
            ),
        }
    }

    fn shield(
        &self,
        state: &mut PoolState,
        asset: Asset,
        amount: u64,
        secret: &FieldBytes,
        encrypted_note: &[u8],
    ) -> Result<PoolOutcome, PoolError> {
        if amount == 0 {
            return Err(PoolError::ZeroAmount);
        }
        // Bind the commitment to the PUBLIC asset + amount. The depositor
        // cannot mint a note worth more than they deposit.
        let secret_fr = fr_from_bytes(secret).ok_or(PoolError::BadField)?;
        let asset_id = Self::asset_id_fr(&asset);
        let commitment_fr =
            crate::poseidon::note_commitment_from_secret(asset_id, amount, secret_fr);
        let commitment = crate::field::fr_to_bytes(&commitment_fr);
        let leaf_index = state.insert(commitment_fr).map_err(|_| PoolError::TreeFull)?;
        Ok(PoolOutcome {
            effects: vec![PoolEffect::DepositToVault { asset, amount }],
            note_events: vec![NoteEvent {
                leaf_index,
                commitment,
                encrypted_note: encrypted_note.to_vec(),
            }],
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn transfer(
        &self,
        state: &mut PoolState,
        nullifiers: &mut dyn NullifierSet,
        proof: &[u8],
        root: &FieldBytes,
        nfs: &[FieldBytes; 2],
        commitments: &[FieldBytes; 2],
        fee: u64,
        encrypted_notes: &[Vec<u8>; 2],
    ) -> Result<PoolOutcome, PoolError> {
        if !state.is_known_root(root) {
            return Err(PoolError::UnknownRoot);
        }
        if nfs[0] == nfs[1] {
            return Err(PoolError::DuplicateNullifier);
        }
        for n in nfs {
            if nullifiers.contains(n) {
                return Err(PoolError::DoubleSpend);
            }
        }
        let public_inputs = [
            fr_from_bytes(root).ok_or(PoolError::BadField)?,
            fr_from_bytes(&nfs[0]).ok_or(PoolError::BadField)?,
            fr_from_bytes(&nfs[1]).ok_or(PoolError::BadField)?,
            fr_from_bytes(&commitments[0]).ok_or(PoolError::BadField)?,
            fr_from_bytes(&commitments[1]).ok_or(PoolError::BadField)?,
            Fr::from(fee),
        ];
        if !self.verifier.verify_transfer(proof, &public_inputs) {
            return Err(PoolError::InvalidProof);
        }

        // State mutation only after all checks pass.
        for n in nfs {
            nullifiers.insert(*n);
        }
        let mut note_events = Vec::with_capacity(2);
        for (commitment, encrypted) in commitments.iter().zip(encrypted_notes) {
            let leaf = fr_from_bytes(commitment).ok_or(PoolError::BadField)?;
            let leaf_index = state.insert(leaf).map_err(|_| PoolError::TreeFull)?;
            note_events.push(NoteEvent {
                leaf_index,
                commitment: *commitment,
                encrypted_note: encrypted.clone(),
            });
        }
        let effects = if fee > 0 {
            vec![PoolEffect::PayFee { amount: fee }]
        } else {
            vec![]
        };
        Ok(PoolOutcome {
            effects,
            note_events,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn unshield(
        &self,
        state: &mut PoolState,
        nullifiers: &mut dyn NullifierSet,
        proof: &[u8],
        root: &FieldBytes,
        nullifier: &FieldBytes,
        change_commitment: &FieldBytes,
        asset: Asset,
        amount: u64,
        recipient: &[u8; 32],
        encrypted_change_note: &[u8],
    ) -> Result<PoolOutcome, PoolError> {
        if amount == 0 {
            return Err(PoolError::ZeroAmount);
        }
        if !state.is_known_root(root) {
            return Err(PoolError::UnknownRoot);
        }
        if nullifiers.contains(nullifier) {
            return Err(PoolError::DoubleSpend);
        }
        let recipient_fr = Fr::from_le_bytes_mod_order(recipient);
        let public_inputs = [
            fr_from_bytes(root).ok_or(PoolError::BadField)?,
            fr_from_bytes(nullifier).ok_or(PoolError::BadField)?,
            fr_from_bytes(change_commitment).ok_or(PoolError::BadField)?,
            Self::asset_id_fr(&asset),
            Fr::from(amount),
            recipient_fr,
        ];
        if !self.verifier.verify_unshield(proof, &public_inputs) {
            return Err(PoolError::InvalidProof);
        }

        nullifiers.insert(*nullifier);
        let leaf = fr_from_bytes(change_commitment).ok_or(PoolError::BadField)?;
        let leaf_index = state.insert(leaf).map_err(|_| PoolError::TreeFull)?;
        Ok(PoolOutcome {
            effects: vec![PoolEffect::WithdrawFromVault {
                asset,
                amount,
                recipient: Pubkey::new_from_array(*recipient),
            }],
            note_events: vec![NoteEvent {
                leaf_index,
                commitment: *change_commitment,
                encrypted_note: encrypted_change_note.to_vec(),
            }],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::fr_to_bytes;
    use crate::poseidon::{note_commitment, nullifier as make_nullifier};
    use std::collections::HashSet;

    #[derive(Default)]
    struct MemNullifiers(HashSet<FieldBytes>);
    impl NullifierSet for MemNullifiers {
        fn contains(&self, n: &FieldBytes) -> bool {
            self.0.contains(n)
        }
        fn insert(&mut self, n: FieldBytes) {
            self.0.insert(n);
        }
    }

    /// Accepts any proof; used to exercise state logic. Records calls.
    struct AcceptVerifier;
    impl ProofVerifier for AcceptVerifier {
        fn verify_transfer(&self, _p: &[u8], _i: &[Fr]) -> bool {
            true
        }
        fn verify_unshield(&self, _p: &[u8], _i: &[Fr]) -> bool {
            true
        }
    }

    struct RejectVerifier;
    impl ProofVerifier for RejectVerifier {
        fn verify_transfer(&self, _p: &[u8], _i: &[Fr]) -> bool {
            false
        }
        fn verify_unshield(&self, _p: &[u8], _i: &[Fr]) -> bool {
            false
        }
    }

    fn commit(amount: u64, blinding: u64) -> FieldBytes {
        let pk = crate::poseidon::hash1(Fr::from(1u64));
        fr_to_bytes(&note_commitment(native_asset_id(), amount, pk, Fr::from(blinding)))
    }

    fn secret(blinding: u64) -> FieldBytes {
        let pk = crate::poseidon::hash1(Fr::from(1u64));
        fr_to_bytes(&crate::poseidon::note_secret(pk, Fr::from(blinding)))
    }

    #[test]
    fn shield_inserts_commitment_and_emits_deposit() {
        let v = AcceptVerifier;
        let proc = PoolProcessor::new(&v);
        let mut state = PoolState::new();
        let mut nfs = MemNullifiers::default();

        let out = proc
            .process(
                &mut state,
                &mut nfs,
                &PoolInstruction::Shield {
                    asset: Asset::Native,
                    amount: 1_000,
                    secret: secret(7),
                    encrypted_note: vec![0xab; 64],
                },
            )
            .unwrap();

        // The emitted commitment binds the public amount to the secret.
        let expected = fr_to_bytes(&crate::poseidon::note_commitment_from_secret(
            native_asset_id(),
            1_000,
            crate::field::fr_from_bytes(&secret(7)).unwrap(),
        ));
        assert_eq!(out.note_events[0].commitment, expected);

        assert_eq!(
            out.effects,
            vec![PoolEffect::DepositToVault {
                asset: Asset::Native,
                amount: 1_000
            }]
        );
        assert_eq!(out.note_events.len(), 1);
        assert_eq!(out.note_events[0].leaf_index, 0);
        assert_eq!(state.next_leaf_index, 1);
    }

    #[test]
    fn shield_rejects_zero_amount() {
        let v = AcceptVerifier;
        let proc = PoolProcessor::new(&v);
        let mut state = PoolState::new();
        let mut nfs = MemNullifiers::default();
        let err = proc
            .process(
                &mut state,
                &mut nfs,
                &PoolInstruction::Shield {
                    asset: Asset::Native,
                    amount: 0,
                    secret: secret(1),
                    encrypted_note: vec![],
                },
            )
            .unwrap_err();
        assert_eq!(err, PoolError::ZeroAmount);
    }

    fn fund_tree(state: &mut PoolState) -> FieldBytes {
        // Put two notes in the tree so a transfer's root is valid.
        state.insert(Fr::from(111u64)).unwrap();
        state.insert(Fr::from(222u64)).unwrap();
        state.current_root()
    }

    #[test]
    fn transfer_spends_nullifiers_and_adds_commitments() {
        let v = AcceptVerifier;
        let proc = PoolProcessor::new(&v);
        let mut state = PoolState::new();
        let mut nfs = MemNullifiers::default();
        let root = fund_tree(&mut state);
        let n0 = fr_to_bytes(&make_nullifier(Fr::from(1u64), 0, Fr::from(9u64)));
        let n1 = fr_to_bytes(&make_nullifier(Fr::from(2u64), 1, Fr::from(9u64)));

        let ix = PoolInstruction::Transfer {
            proof: vec![0u8; 256],
            root,
            nullifiers: [n0, n1],
            commitments: [commit(600, 1), commit(400, 2)],
            fee: 0,
            encrypted_notes: [vec![1u8; 32], vec![2u8; 32]],
        };
        let out = proc.process(&mut state, &mut nfs, &ix).unwrap();
        assert!(out.effects.is_empty());
        assert_eq!(out.note_events.len(), 2);
        assert!(nfs.contains(&n0) && nfs.contains(&n1));

        // Replaying the same nullifiers is a double spend.
        let err = proc.process(&mut state, &mut nfs, &ix).unwrap_err();
        assert_eq!(err, PoolError::DoubleSpend);
    }

    #[test]
    fn transfer_with_fee_emits_pay_fee() {
        let v = AcceptVerifier;
        let proc = PoolProcessor::new(&v);
        let mut state = PoolState::new();
        let mut nfs = MemNullifiers::default();
        let root = fund_tree(&mut state);
        let out = proc
            .process(
                &mut state,
                &mut nfs,
                &PoolInstruction::Transfer {
                    proof: vec![0u8; 256],
                    root,
                    nullifiers: [[1u8; 32], [2u8; 32]],
                    commitments: [commit(600, 1), commit(395, 2)],
                    fee: 5_000,
                    encrypted_notes: [vec![], vec![]],
                },
            )
            .unwrap();
        assert_eq!(out.effects, vec![PoolEffect::PayFee { amount: 5_000 }]);
    }

    #[test]
    fn transfer_rejects_unknown_root_and_duplicate_nullifier() {
        let v = AcceptVerifier;
        let proc = PoolProcessor::new(&v);
        let mut state = PoolState::new();
        let mut nfs = MemNullifiers::default();
        let valid_root = fund_tree(&mut state);

        let unknown = proc.process(
            &mut state,
            &mut nfs,
            &PoolInstruction::Transfer {
                proof: vec![0u8; 256],
                root: [0xaa; 32],
                nullifiers: [[1u8; 32], [2u8; 32]],
                commitments: [commit(1, 1), commit(1, 2)],
                fee: 0,
                encrypted_notes: [vec![], vec![]],
            },
        );
        assert_eq!(unknown.unwrap_err(), PoolError::UnknownRoot);

        let dup = proc.process(
            &mut state,
            &mut nfs,
            &PoolInstruction::Transfer {
                proof: vec![0u8; 256],
                root: valid_root,
                nullifiers: [[7u8; 32], [7u8; 32]],
                commitments: [commit(1, 1), commit(1, 2)],
                fee: 0,
                encrypted_notes: [vec![], vec![]],
            },
        );
        assert_eq!(dup.unwrap_err(), PoolError::DuplicateNullifier);
    }

    #[test]
    fn transfer_rejects_invalid_proof_without_mutating_state() {
        let v = RejectVerifier;
        let proc = PoolProcessor::new(&v);
        let mut state = PoolState::new();
        let mut nfs = MemNullifiers::default();
        let root = fund_tree(&mut state);
        let leaves_before = state.next_leaf_index;

        let err = proc
            .process(
                &mut state,
                &mut nfs,
                &PoolInstruction::Transfer {
                    proof: vec![0u8; 256],
                    root,
                    nullifiers: [[1u8; 32], [2u8; 32]],
                    commitments: [commit(1, 1), commit(1, 2)],
                    fee: 0,
                    encrypted_notes: [vec![], vec![]],
                },
            )
            .unwrap_err();
        assert_eq!(err, PoolError::InvalidProof);
        assert_eq!(state.next_leaf_index, leaves_before);
        assert!(!nfs.contains(&[1u8; 32]));
    }

    #[test]
    fn unshield_releases_funds_and_records_nullifier() {
        let v = AcceptVerifier;
        let proc = PoolProcessor::new(&v);
        let mut state = PoolState::new();
        let mut nfs = MemNullifiers::default();
        let root = fund_tree(&mut state);
        let recipient = Pubkey::new_unique();

        let out = proc
            .process(
                &mut state,
                &mut nfs,
                &PoolInstruction::Unshield {
                    proof: vec![0u8; 256],
                    root,
                    nullifier: [3u8; 32],
                    change_commitment: commit(100, 5),
                    asset: Asset::Native,
                    amount: 900,
                    recipient: recipient.to_bytes(),
                    encrypted_change_note: vec![9u8; 16],
                },
            )
            .unwrap();

        assert_eq!(
            out.effects,
            vec![PoolEffect::WithdrawFromVault {
                asset: Asset::Native,
                amount: 900,
                recipient,
            }]
        );
        assert!(nfs.contains(&[3u8; 32]));
    }

    #[test]
    fn unshield_rejects_zero_amount_and_double_spend() {
        let v = AcceptVerifier;
        let proc = PoolProcessor::new(&v);
        let mut state = PoolState::new();
        let mut nfs = MemNullifiers::default();
        let root = fund_tree(&mut state);

        let zero = proc.process(
            &mut state,
            &mut nfs,
            &PoolInstruction::Unshield {
                proof: vec![0u8; 256],
                root,
                nullifier: [3u8; 32],
                change_commitment: commit(1, 5),
                asset: Asset::Native,
                amount: 0,
                recipient: [1u8; 32],
                encrypted_change_note: vec![],
            },
        );
        assert_eq!(zero.unwrap_err(), PoolError::ZeroAmount);

        nfs.insert([4u8; 32]);
        let spent = proc.process(
            &mut state,
            &mut nfs,
            &PoolInstruction::Unshield {
                proof: vec![0u8; 256],
                root,
                nullifier: [4u8; 32],
                change_commitment: commit(1, 5),
                asset: Asset::Native,
                amount: 10,
                recipient: [1u8; 32],
                encrypted_change_note: vec![],
            },
        );
        assert_eq!(spent.unwrap_err(), PoolError::DoubleSpend);
    }
}
