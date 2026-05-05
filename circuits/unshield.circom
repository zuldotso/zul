pragma circom 2.1.6;

include "lib/note.circom";
include "lib/merkle.circom";
include "node_modules/circomlib/circuits/comparators.circom";

// ZUL unshield: spend one input note to a public recipient + amount, with a
// private change note returning the remainder to the spender.
//
// Public signals — declared as inputs in this exact order to match
// chain/zul-privacy/src/processor.rs (UNSHIELD_PUBLIC_INPUTS = 6):
//   [root, nullifier, change_commitment, asset_id, amount, recipient]
//
// Enforced:
//  - the input note is in the tree under `root`
//  - nullifier derives correctly (ownership + no forgery)
//  - change commitment is well-formed and shares the input's asset id
//  - value conservation: in_amount == amount + change_amount
//  - amounts are 64-bit
//  - `recipient` is bound into the proof so the public withdrawal cannot be
//    front-run to a different address

template Unshield(depth) {
    // ----- public inputs (fixed order) ------------------------------------
    signal input root;
    signal input nullifier;
    signal input change_commitment;
    signal input asset_id;
    signal input amount;            // released publicly
    signal input recipient;         // bound, not otherwise constrained

    // ----- private --------------------------------------------------------
    signal input in_amount;
    signal input in_s;
    signal input in_blinding;
    signal input in_leaf_index;
    signal input in_path[depth];

    signal input change_amount;
    signal input change_pk;
    signal input change_blinding;

    component in_range = AmountCheck();
    in_range.amount <== in_amount;
    component amount_range = AmountCheck();
    amount_range.amount <== amount;
    component change_range = AmountCheck();
    change_range.amount <== change_amount;

    component in_pk = OwnerPk();
    in_pk.s <== in_s;

    component in_cm = NoteCommitment();
    in_cm.asset_id <== asset_id;
    in_cm.amount <== in_amount;
    in_cm.pk <== in_pk.pk;
    in_cm.blinding <== in_blinding;

    component in_nf = Nullifier();
    in_nf.commitment <== in_cm.commitment;
    in_nf.leaf_index <== in_leaf_index;
    in_nf.s <== in_s;
    nullifier === in_nf.nullifier;

    component inclusion = MerkleInclusion(depth);
    inclusion.leaf <== in_cm.commitment;
    inclusion.leaf_index <== in_leaf_index;
    for (var j = 0; j < depth; j++) {
        inclusion.path_elements[j] <== in_path[j];
    }
    inclusion.root <== root;
    inclusion.enabled <== 1;

    component change_cm = NoteCommitment();
    change_cm.asset_id <== asset_id;
    change_cm.amount <== change_amount;
    change_cm.pk <== change_pk;
    change_cm.blinding <== change_blinding;
    change_commitment === change_cm.commitment;

    // Value conservation.
    in_amount === amount + change_amount;

    // Bind recipient into the constraint system (multiply by itself so the
    // signal cannot be optimized away).
    signal recipient_sq;
    recipient_sq <== recipient * recipient;
}

component main {
    public [root, nullifier, change_commitment, asset_id, amount, recipient]
} = Unshield(26);
