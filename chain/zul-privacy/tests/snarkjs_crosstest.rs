//! Cross-language binding test: a real Groth16 proof produced by snarkjs
//! (circuits/transfer.circom) must verify against the in-node arkworks
//! verifier with the exact verifying key, proof encoding, and public-input
//! ordering. This is what proves the privacy stack is consistent
//! end-to-end; regenerate the fixture with `npm run build` + gen-fixture in
//! circuits/ if the circuit or scheme changes.
//!
//! The fixture is committed under tests/fixtures/. If it is absent (a fresh
//! checkout that hasn't run the circuit build), the test self-skips.

use ark_bn254::Fr;
use ark_ff::PrimeField;
use num_bigint::BigUint;
use zul_privacy::verifier::{self, VerifierError};
use std::str::FromStr;

const FIXTURE: &str = include_str!("fixtures/transfer_proof.json");

fn fr_from_decimal(s: &str) -> Fr {
    let big = BigUint::from_str(s).expect("decimal public input");
    Fr::from_le_bytes_mod_order(&big.to_bytes_le())
}

struct Fixture {
    vk: Vec<u8>,
    proof: Vec<u8>,
    public_inputs: Vec<Fr>,
}

fn load() -> Option<Fixture> {
    // Minimal hand-roll JSON pull to avoid a serde_json dev-dep here.
    let json = FIXTURE.trim();
    if json.is_empty() {
        return None;
    }
    let vk_hex = extract_str(json, "vk_hex")?;
    let proof_hex = extract_str(json, "proof_hex")?;
    let public_inputs = extract_array(json, "public_inputs")?
        .iter()
        .map(|s| fr_from_decimal(s))
        .collect();
    Some(Fixture {
        vk: hex::decode(vk_hex).expect("vk hex"),
        proof: hex::decode(proof_hex).expect("proof hex"),
        public_inputs,
    })
}

fn extract_str(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let start = json.find(&needle)? + needle.len();
    let rest = &json[start..];
    let q1 = rest.find('"')?;
    let q2 = rest[q1 + 1..].find('"')? + q1 + 1;
    Some(rest[q1 + 1..q2].to_string())
}

fn extract_array(json: &str, key: &str) -> Option<Vec<String>> {
    let needle = format!("\"{key}\"");
    let start = json.find(&needle)? + needle.len();
    let rest = &json[start..];
    let open = rest.find('[')?;
    let close = rest.find(']')?;
    Some(
        rest[open + 1..close]
            .split(',')
            .map(|s| s.trim().trim_matches('"').to_string())
            .filter(|s| !s.is_empty())
            .collect(),
    )
}

#[test]
fn snarkjs_transfer_proof_verifies_in_node() {
    let Some(fixture) = load() else {
        eprintln!("fixture absent; run circuits build to enable cross-test");
        return;
    };

    let vk = verifier::parse_verifying_key(&fixture.vk).expect("parse vk");
    assert_eq!(fixture.public_inputs.len(), 6, "transfer exposes 6 public inputs");

    // The real proof verifies.
    assert_eq!(
        verifier::verify(&vk, &fixture.proof, &fixture.public_inputs),
        Ok(true),
        "snarkjs proof must verify against the node verifier"
    );

    // Tampering with any public input breaks it.
    let mut tampered = fixture.public_inputs.clone();
    tampered[0] += Fr::from(1u64);
    assert_eq!(
        verifier::verify(&vk, &fixture.proof, &tampered),
        Ok(false),
        "modified public inputs must fail"
    );

    // Tampering with the proof bytes breaks it (bad point or false).
    let mut bad_proof = fixture.proof.clone();
    bad_proof[10] ^= 0x01;
    let result = verifier::verify(&vk, &bad_proof, &fixture.public_inputs);
    assert!(
        matches!(result, Ok(false) | Err(VerifierError::BadProof)),
        "tampered proof must not verify, got {result:?}"
    );
}
