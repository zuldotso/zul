pragma circom 2.1.6;

include "../node_modules/circomlib/circuits/poseidon.circom";
include "../node_modules/circomlib/circuits/bitify.circom";
include "../node_modules/circomlib/circuits/mux1.circom";
include "../node_modules/circomlib/circuits/comparators.circom";

// Poseidon merkle inclusion proof for the ZUL commitment tree
// (chain/zul-privacy/src/state.rs). Path bits come from the leaf index,
// LSB = leaf level (matches the incremental tree's index arithmetic).
//
// `enabled` switches the root equality check off for dummy (zero-amount)
// inputs: the prover supplies an arbitrary path and the constraint
// root_computed == root is skipped, exactly like Tornado Nova.

template MerkleInclusion(depth) {
    signal input leaf;
    signal input leaf_index;
    signal input path_elements[depth];
    signal input root;
    signal input enabled; // boolean (checked by caller)

    component index_bits = Num2Bits(depth);
    index_bits.in <== leaf_index;

    component hashers[depth];
    component muxes[depth];
    signal levels[depth + 1];
    levels[0] <== leaf;

    for (var i = 0; i < depth; i++) {
        // bit = 0 -> current is left child, sibling right.
        muxes[i] = MultiMux1(2);
        muxes[i].c[0][0] <== levels[i];
        muxes[i].c[0][1] <== path_elements[i];
        muxes[i].c[1][0] <== path_elements[i];
        muxes[i].c[1][1] <== levels[i];
        muxes[i].s <== index_bits.out[i];

        hashers[i] = Poseidon(2);
        hashers[i].inputs[0] <== muxes[i].out[0];
        hashers[i].inputs[1] <== muxes[i].out[1];
        levels[i + 1] <== hashers[i].out;
    }

    // (root_computed - root) * enabled === 0
    signal diff;
    diff <== levels[depth] - root;
    diff * enabled === 0;
}
