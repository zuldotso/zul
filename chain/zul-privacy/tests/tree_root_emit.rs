//! Pool commitment-tree root regression, pinned to the TS SDK
//! (sdk/test/merkle.test.ts). The chain uses an incremental (Tornado-style)
//! tree; the SDK mirrors it with a full-layer tree. They MUST produce
//! identical roots or browser-generated proofs reference the wrong paths and
//! fail on-chain. These constants are asserted on both sides.

use ark_bn254::Fr;
use zul_privacy::state::PoolState;

const EMPTY_ROOT: &str =
    "755ece909e258a8989e9d803927048844745460d568389cffc6f34b7cd800a16";
const ROOT_123: &str =
    "9f17becb707814be908f2d692fc58c5fee0b54d06a97da98d4d94d07bf82470e";

#[test]
fn empty_tree_root_matches_sdk() {
    assert_eq!(hex::encode(PoolState::new().current_root()), EMPTY_ROOT);
}

#[test]
fn three_leaf_root_matches_sdk() {
    let mut state = PoolState::new();
    for n in 1..=3u64 {
        state.insert(Fr::from(n)).unwrap();
    }
    assert_eq!(hex::encode(state.current_root()), ROOT_123);
}
