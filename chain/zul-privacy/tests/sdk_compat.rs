//! TS-SDK <-> Rust instruction compatibility. The SDK encodes
//! PoolInstructions with a hand-rolled bincode writer; this test decodes its
//! output with the real serde/bincode path and asserts every field, so the
//! two never silently drift. Regenerate with `node test/emit-fixture.mjs`
//! in sdk/ after building the SDK.

use zul_privacy::instruction::{Asset, PoolInstruction};

const FIXTURE: &str = include_str!("fixtures/sdk_instructions.json");

fn field_hex(key: &str) -> Vec<u8> {
    let needle = format!("\"{key}\"");
    let start = FIXTURE.find(&needle).expect("key present") + needle.len();
    let rest = &FIXTURE[start..];
    let q1 = rest.find('"').unwrap();
    let q2 = rest[q1 + 1..].find('"').unwrap() + q1 + 1;
    hex::decode(&rest[q1 + 1..q2]).expect("valid hex")
}

#[test]
fn decodes_sdk_shield() {
    let ix = PoolInstruction::try_from_slice(&field_hex("shield")).expect("decode shield");
    match ix {
        PoolInstruction::Shield {
            asset,
            amount,
            secret,
            encrypted_note,
        } => {
            assert_eq!(asset, Asset::Native);
            assert_eq!(amount, 1_234_567_890);
            assert_eq!(secret, [3u8; 32]);
            assert_eq!(encrypted_note, vec![0xaa, 0xbb, 0xcc]);
        }
        other => panic!("expected Shield, got {other:?}"),
    }
}

#[test]
fn decodes_sdk_transfer() {
    let ix = PoolInstruction::try_from_slice(&field_hex("transfer")).expect("decode transfer");
    match ix {
        PoolInstruction::Transfer {
            proof,
            root,
            nullifiers,
            commitments,
            fee,
            encrypted_notes,
        } => {
            assert_eq!(proof, vec![2u8; 256]);
            assert_eq!(root, [1u8; 32]);
            assert_eq!(nullifiers, [[4u8; 32], [5u8; 32]]);
            assert_eq!(commitments, [[6u8; 32], [7u8; 32]]);
            assert_eq!(fee, 5000);
            assert_eq!(encrypted_notes[0], vec![1, 2]);
            assert_eq!(encrypted_notes[1], vec![3, 4, 5]);
        }
        other => panic!("expected Transfer, got {other:?}"),
    }
}

#[test]
fn decodes_sdk_unshield_with_spl_asset() {
    let ix = PoolInstruction::try_from_slice(&field_hex("unshield")).expect("decode unshield");
    match ix {
        PoolInstruction::Unshield {
            proof,
            root,
            nullifier,
            change_commitment,
            asset,
            amount,
            recipient,
            encrypted_change_note,
        } => {
            assert_eq!(proof, vec![8u8; 256]);
            assert_eq!(root, [1u8; 32]);
            assert_eq!(nullifier, [4u8; 32]);
            assert_eq!(change_commitment, [5u8; 32]);
            assert_eq!(asset, Asset::Spl([9u8; 32]));
            assert_eq!(amount, 900);
            assert_eq!(recipient, [8u8; 32]);
            assert_eq!(encrypted_change_note, vec![7]);
        }
        other => panic!("expected Unshield, got {other:?}"),
    }
}
