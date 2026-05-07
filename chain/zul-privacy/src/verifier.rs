//! Groth16 (BN254) verification.
//!
//! Wire format (shared with snarkjs tooling and the TS SDK): uncompressed
//! big-endian affine coordinates, EVM-style. G1 = x||y (64 bytes); G2 =
//! x_c1||x_c0||y_c1||y_c0 (128 bytes, imaginary component first). Proof =
//! A(G1) || B(G2) || C(G1) = 256 bytes. The verifying key blob is:
//! u32-le n_public || alpha(G1) || beta(G2) || gamma(G2) || delta(G2) ||
//! ic[(n_public+1) * G1].

use ark_bn254::{Bn254, Fq, Fq2, Fr, G1Affine, G2Affine};
use ark_ec::AffineRepr;
use ark_ff::PrimeField;
use ark_groth16::{prepare_verifying_key, Groth16, Proof, VerifyingKey};

pub const PROOF_BYTES: usize = 256;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VerifierError {
    #[error("malformed curve point")]
    BadPoint,
    #[error("malformed verifying key")]
    BadVerifyingKey,
    #[error("malformed proof")]
    BadProof,
    #[error("public input count mismatch: vk expects {expected}, got {actual}")]
    InputCount { expected: usize, actual: usize },
}

fn fq_from_be(bytes: &[u8]) -> Result<Fq, VerifierError> {
    if bytes.len() != 32 {
        return Err(VerifierError::BadPoint);
    }
    let fq = Fq::from_be_bytes_mod_order(bytes);
    // Reject non-canonical encodings (value >= field modulus).
    let mut canonical = [0u8; 32];
    let le = ark_ff::BigInteger::to_bytes_le(&fq.into_bigint());
    canonical[..le.len()].copy_from_slice(&le);
    let mut be = canonical;
    be.reverse();
    if be != bytes {
        return Err(VerifierError::BadPoint);
    }
    Ok(fq)
}

fn g1_from_be(bytes: &[u8]) -> Result<G1Affine, VerifierError> {
    if bytes.len() != 64 {
        return Err(VerifierError::BadPoint);
    }
    let x = fq_from_be(&bytes[..32])?;
    let y = fq_from_be(&bytes[32..])?;
    if x == Fq::from(0u64) && y == Fq::from(0u64) {
        return Ok(G1Affine::identity());
    }
    let point = G1Affine::new_unchecked(x, y);
    if !point.is_on_curve() || !point.is_in_correct_subgroup_assuming_on_curve() {
        return Err(VerifierError::BadPoint);
    }
    Ok(point)
}

fn g2_from_be(bytes: &[u8]) -> Result<G2Affine, VerifierError> {
    if bytes.len() != 128 {
        return Err(VerifierError::BadPoint);
    }
    let x_c1 = fq_from_be(&bytes[..32])?;
    let x_c0 = fq_from_be(&bytes[32..64])?;
    let y_c1 = fq_from_be(&bytes[64..96])?;
    let y_c0 = fq_from_be(&bytes[96..128])?;
    let x = Fq2::new(x_c0, x_c1);
    let y = Fq2::new(y_c0, y_c1);
    if x == Fq2::new(Fq::from(0u64), Fq::from(0u64))
        && y == Fq2::new(Fq::from(0u64), Fq::from(0u64))
    {
        return Ok(G2Affine::identity());
    }
    let point = G2Affine::new_unchecked(x, y);
    if !point.is_on_curve() || !point.is_in_correct_subgroup_assuming_on_curve() {
        return Err(VerifierError::BadPoint);
    }
    Ok(point)
}

pub fn parse_proof(bytes: &[u8]) -> Result<Proof<Bn254>, VerifierError> {
    if bytes.len() != PROOF_BYTES {
        return Err(VerifierError::BadProof);
    }
    Ok(Proof {
        a: g1_from_be(&bytes[..64]).map_err(|_| VerifierError::BadProof)?,
        b: g2_from_be(&bytes[64..192]).map_err(|_| VerifierError::BadProof)?,
        c: g1_from_be(&bytes[192..256]).map_err(|_| VerifierError::BadProof)?,
    })
}

pub fn parse_verifying_key(bytes: &[u8]) -> Result<VerifyingKey<Bn254>, VerifierError> {
    if bytes.len() < 4 + 64 + 128 * 3 + 64 {
        return Err(VerifierError::BadVerifyingKey);
    }
    let n_public = u32::from_le_bytes(
        bytes[..4]
            .try_into()
            .map_err(|_| VerifierError::BadVerifyingKey)?,
    ) as usize;
    let expected_len = 4 + 64 + 128 * 3 + (n_public + 1) * 64;
    if bytes.len() != expected_len {
        return Err(VerifierError::BadVerifyingKey);
    }
    let mut offset = 4;
    let alpha_g1 = g1_from_be(&bytes[offset..offset + 64])?;
    offset += 64;
    let beta_g2 = g2_from_be(&bytes[offset..offset + 128])?;
    offset += 128;
    let gamma_g2 = g2_from_be(&bytes[offset..offset + 128])?;
    offset += 128;
    let delta_g2 = g2_from_be(&bytes[offset..offset + 128])?;
    offset += 128;
    let mut gamma_abc_g1 = Vec::with_capacity(n_public + 1);
    for _ in 0..=n_public {
        gamma_abc_g1.push(g1_from_be(&bytes[offset..offset + 64])?);
        offset += 64;
    }
    Ok(VerifyingKey {
        alpha_g1,
        beta_g2,
        gamma_g2,
        delta_g2,
        gamma_abc_g1,
    })
}

/// Verify a proof against LE-encoded public inputs.
pub fn verify(
    vk: &VerifyingKey<Bn254>,
    proof_bytes: &[u8],
    public_inputs: &[Fr],
) -> Result<bool, VerifierError> {
    if public_inputs.len() + 1 != vk.gamma_abc_g1.len() {
        return Err(VerifierError::InputCount {
            expected: vk.gamma_abc_g1.len().saturating_sub(1),
            actual: public_inputs.len(),
        });
    }
    let proof = parse_proof(proof_bytes)?;
    let pvk = prepare_verifying_key(vk);
    Groth16::<Bn254>::verify_proof(&pvk, &proof, public_inputs)
        .map_err(|_| VerifierError::BadProof)
}

// ----- serialization helpers (tests + key tooling) ------------------------

pub fn fq_to_be(fq: &Fq) -> [u8; 32] {
    let mut out = [0u8; 32];
    let le = ark_ff::BigInteger::to_bytes_le(&fq.into_bigint());
    out[..le.len()].copy_from_slice(&le);
    out.reverse();
    out
}

pub fn g1_to_be(point: &G1Affine) -> [u8; 64] {
    let mut out = [0u8; 64];
    if let Some((x, y)) = point.xy() {
        out[..32].copy_from_slice(&fq_to_be(&x));
        out[32..].copy_from_slice(&fq_to_be(&y));
    }
    out
}

pub fn g2_to_be(point: &G2Affine) -> [u8; 128] {
    let mut out = [0u8; 128];
    if let Some((x, y)) = point.xy() {
        out[..32].copy_from_slice(&fq_to_be(&x.c1));
        out[32..64].copy_from_slice(&fq_to_be(&x.c0));
        out[64..96].copy_from_slice(&fq_to_be(&y.c1));
        out[96..128].copy_from_slice(&fq_to_be(&y.c0));
    }
    out
}

pub fn proof_to_bytes(proof: &Proof<Bn254>) -> [u8; PROOF_BYTES] {
    let mut out = [0u8; PROOF_BYTES];
    out[..64].copy_from_slice(&g1_to_be(&proof.a));
    out[64..192].copy_from_slice(&g2_to_be(&proof.b));
    out[192..].copy_from_slice(&g1_to_be(&proof.c));
    out
}

pub fn verifying_key_to_bytes(vk: &VerifyingKey<Bn254>) -> Vec<u8> {
    let n_public = vk.gamma_abc_g1.len() - 1;
    let mut out = Vec::with_capacity(4 + 64 + 128 * 3 + (n_public + 1) * 64);
    out.extend_from_slice(&(n_public as u32).to_le_bytes());
    out.extend_from_slice(&g1_to_be(&vk.alpha_g1));
    out.extend_from_slice(&g2_to_be(&vk.beta_g2));
    out.extend_from_slice(&g2_to_be(&vk.gamma_g2));
    out.extend_from_slice(&g2_to_be(&vk.delta_g2));
    for ic in &vk.gamma_abc_g1 {
        out.extend_from_slice(&g1_to_be(ic));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_ff::UniformRand;
    use ark_snark::SNARK;
    use ark_relations::lc;
    use ark_relations::r1cs::{
        ConstraintSynthesizer, ConstraintSystemRef, SynthesisError, Variable,
    };

    /// Toy circuit: prove knowledge of a, b with a * b = c (c public).
    #[derive(Clone)]
    struct MulCircuit {
        a: Option<Fr>,
        b: Option<Fr>,
        c: Option<Fr>,
    }

    impl ConstraintSynthesizer<Fr> for MulCircuit {
        fn generate_constraints(
            self,
            cs: ConstraintSystemRef<Fr>,
        ) -> Result<(), SynthesisError> {
            let a = cs.new_witness_variable(|| self.a.ok_or(SynthesisError::AssignmentMissing))?;
            let b = cs.new_witness_variable(|| self.b.ok_or(SynthesisError::AssignmentMissing))?;
            let c = cs.new_input_variable(|| self.c.ok_or(SynthesisError::AssignmentMissing))?;
            cs.enforce_constraint(lc!() + a, lc!() + b, lc!() + c)?;
            // Make the system non-degenerate.
            cs.enforce_constraint(lc!() + a, lc!() + Variable::One, lc!() + a)?;
            Ok(())
        }
    }

    #[test]
    fn wire_format_roundtrip_and_verification() {
        // CryptoRng-implementing deterministic RNG for the Groth16 setup.
        use ark_std::rand::SeedableRng;
        let mut rng = ark_std::rand::rngs::StdRng::seed_from_u64(42);
        let blank = MulCircuit {
            a: None,
            b: None,
            c: None,
        };
        let (pk, vk) =
            Groth16::<Bn254>::circuit_specific_setup(blank, &mut rng).expect("setup");

        let a = Fr::rand(&mut rng);
        let b = Fr::rand(&mut rng);
        let c = a * b;
        let proof = Groth16::<Bn254>::prove(
            &pk,
            MulCircuit {
                a: Some(a),
                b: Some(b),
                c: Some(c),
            },
            &mut rng,
        )
        .expect("prove");

        // Through our byte format end-to-end.
        let vk_bytes = verifying_key_to_bytes(&vk);
        let parsed_vk = parse_verifying_key(&vk_bytes).expect("vk parse");
        let proof_bytes = proof_to_bytes(&proof);

        assert_eq!(verify(&parsed_vk, &proof_bytes, &[c]), Ok(true));
        // Wrong public input fails.
        assert_eq!(verify(&parsed_vk, &proof_bytes, &[c + Fr::from(1u64)]), Ok(false));
        // Tampered proof fails (bit flip in A.y keeps length valid).
        let mut bad = proof_bytes;
        bad[40] ^= 0x01;
        assert!(matches!(
            verify(&parsed_vk, &bad, &[c]),
            Err(VerifierError::BadProof) | Ok(false)
        ));
        // Wrong input count is rejected.
        assert_eq!(
            verify(&parsed_vk, &proof_bytes, &[c, c]),
            Err(VerifierError::InputCount {
                expected: 1,
                actual: 2
            })
        );
    }
}
