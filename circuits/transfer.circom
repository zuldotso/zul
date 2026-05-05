pragma circom 2.1.6;

include "lib/note.circom";
include "lib/merkle.circom";
include "node_modules/circomlib/circuits/comparators.circom";

// ZUL shielded transfer: 2-in / 2-out join-split.
//
// Public signals — declared as inputs in this exact order so snarkjs emits
// them in the order chain/zul-privacy/src/processor.rs reads
// (TRANSFER_PUBLIC_INPUTS = 6):
//   [root, nullifier0, nullifier1, out_commitment0, out_commitment1, fee]
//
// We take nullifiers and commitments as public inputs and constrain them to
// the values the circuit derives. (circom orders public signals as outputs
// first then public inputs; using inputs only gives a deterministic order.)
//
// Enforced:
//  - real input notes are in the tree under `root`; dummy inputs
//    (amount == 0) skip the membership check
//  - nullifiers and commitments equal their derived values
//  - per-asset value conservation: sum(in) == sum(out) + fee
//  - all notes share one private asset id (never leaked publicly)
//  - fee may be non-zero only for native ZUL (asset_id == 0)
//  - amounts are 64-bit

template Transfer(depth) {
    // ----- public inputs (fixed order) ------------------------------------
    signal input root;
    signal input nullifier[2];
    signal input out_commitment[2];
    signal input fee;

    // ----- private: shared ------------------------------------------------
    signal input asset_id;

    // ----- private: inputs (2) --------------------------------------------
    signal input in_amount[2];
    signal input in_s[2];
    signal input in_blinding[2];
    signal input in_leaf_index[2];
    signal input in_path[2][depth];

    // ----- private: outputs (2) -------------------------------------------
    signal input out_amount[2];
    signal input out_pk[2];
    signal input out_blinding[2];

    component in_pk[2];
    component in_cm[2];
    component in_nf[2];
    component in_inclusion[2];
    component in_zero[2];
    component in_range[2];
    component out_cm[2];
    component out_range[2];

    for (var i = 0; i < 2; i++) {
        in_range[i] = AmountCheck();
        in_range[i].amount <== in_amount[i];

        in_pk[i] = OwnerPk();
        in_pk[i].s <== in_s[i];

        in_cm[i] = NoteCommitment();
        in_cm[i].asset_id <== asset_id;
        in_cm[i].amount <== in_amount[i];
        in_cm[i].pk <== in_pk[i].pk;
        in_cm[i].blinding <== in_blinding[i];

        in_nf[i] = Nullifier();
        in_nf[i].commitment <== in_cm[i].commitment;
        in_nf[i].leaf_index <== in_leaf_index[i];
        in_nf[i].s <== in_s[i];
        // Bind the public nullifier to the derived one.
        nullifier[i] === in_nf[i].nullifier;

        in_zero[i] = IsZero();
        in_zero[i].in <== in_amount[i];

        in_inclusion[i] = MerkleInclusion(depth);
        in_inclusion[i].leaf <== in_cm[i].commitment;
        in_inclusion[i].leaf_index <== in_leaf_index[i];
        for (var j = 0; j < depth; j++) {
            in_inclusion[i].path_elements[j] <== in_path[i][j];
        }
        in_inclusion[i].root <== root;
        in_inclusion[i].enabled <== 1 - in_zero[i].out;
    }

    for (var i = 0; i < 2; i++) {
        out_range[i] = AmountCheck();
        out_range[i].amount <== out_amount[i];

        out_cm[i] = NoteCommitment();
        out_cm[i].asset_id <== asset_id;
        out_cm[i].amount <== out_amount[i];
        out_cm[i].pk <== out_pk[i];
        out_cm[i].blinding <== out_blinding[i];
        // Bind the public commitment to the derived one.
        out_commitment[i] === out_cm[i].commitment;
    }

    // Value conservation (single asset).
    in_amount[0] + in_amount[1] === out_amount[0] + out_amount[1] + fee;

    // Fee only in the native asset: asset_id != 0 => fee == 0.
    component asset_is_native = IsZero();
    asset_is_native.in <== asset_id;
    fee * (1 - asset_is_native.out) === 0;

    // Distinct nullifiers (closes "spend same note twice in one proof").
    component same_nf = IsEqual();
    same_nf.in[0] <== nullifier[0];
    same_nf.in[1] <== nullifier[1];
    same_nf.out === 0;
}

component main { public [root, nullifier, out_commitment, fee] } = Transfer(26);
