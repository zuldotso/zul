pragma circom 2.1.6;

include "../node_modules/circomlib/circuits/poseidon.circom";
include "../node_modules/circomlib/circuits/bitify.circom";

// ZUL note scheme — must mirror chain/zul-privacy/src/poseidon.rs:
//   pk         = Poseidon1(s)
//   secret     = Poseidon2(pk, blinding)
//   commitment = Poseidon3(asset_id, amount, secret)
//   nullifier  = Poseidon3(commitment, leaf_index, s)
//
// The two-level commitment lets `shield` be proofless yet sound: the node
// recomputes commitment from the public (asset_id, amount) and the revealed
// secret, binding the committed value to the deposited funds.

template OwnerPk() {
    signal input s;
    signal output pk;
    component h = Poseidon(1);
    h.inputs[0] <== s;
    pk <== h.out;
}

template NoteSecret() {
    signal input pk;
    signal input blinding;
    signal output secret;
    component h = Poseidon(2);
    h.inputs[0] <== pk;
    h.inputs[1] <== blinding;
    secret <== h.out;
}

template NoteCommitment() {
    signal input asset_id;
    signal input amount;
    signal input pk;
    signal input blinding;
    signal output commitment;
    component s = NoteSecret();
    s.pk <== pk;
    s.blinding <== blinding;
    component h = Poseidon(3);
    h.inputs[0] <== asset_id;
    h.inputs[1] <== amount;
    h.inputs[2] <== s.secret;
    commitment <== h.out;
}

template Nullifier() {
    signal input commitment;
    signal input leaf_index;
    signal input s;
    signal output nullifier;
    component h = Poseidon(3);
    h.inputs[0] <== commitment;
    h.inputs[1] <== leaf_index;
    h.inputs[2] <== s;
    nullifier <== h.out;
}

// Enforce amount < 2^64 so sums cannot wrap the field.
template AmountCheck() {
    signal input amount;
    component bits = Num2Bits(64);
    bits.in <== amount;
}
