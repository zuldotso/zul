//! Loaded Groth16 verifying keys and the production `ProofVerifier`.
//!
//! Keys are produced by the circuit setup (circuits/) and shipped as the
//! byte blob `verifier::verifying_key_to_bytes` understands. The node loads
//! them at startup; until circuits exist, `VerifyingKeys::unconfigured`
//! yields a verifier that rejects every proof (so the pool is safe by
//! default rather than open).

use ark_bn254::{Bn254, Fr};
use ark_groth16::VerifyingKey;

use crate::processor::ProofVerifier;
use crate::verifier::{self, VerifierError};

#[derive(Clone, Default)]
pub struct VerifyingKeys {
    pub transfer: Option<VerifyingKey<Bn254>>,
    pub unshield: Option<VerifyingKey<Bn254>>,
}

impl VerifyingKeys {
    /// No keys configured: every proof is rejected.
    pub fn unconfigured() -> Self {
        Self::default()
    }

    pub fn load(transfer_vk: &[u8], unshield_vk: &[u8]) -> Result<Self, VerifierError> {
        Ok(Self {
            transfer: Some(verifier::parse_verifying_key(transfer_vk)?),
            unshield: Some(verifier::parse_verifying_key(unshield_vk)?),
        })
    }

    pub fn is_configured(&self) -> bool {
        self.transfer.is_some() && self.unshield.is_some()
    }
}

pub struct Groth16Verifier {
    keys: VerifyingKeys,
}

impl Groth16Verifier {
    pub fn new(keys: VerifyingKeys) -> Self {
        Self { keys }
    }
}

impl ProofVerifier for Groth16Verifier {
    fn verify_transfer(&self, proof: &[u8], public_inputs: &[Fr]) -> bool {
        match &self.keys.transfer {
            Some(vk) => verifier::verify(vk, proof, public_inputs).unwrap_or(false),
            None => false,
        }
    }

    fn verify_unshield(&self, proof: &[u8], public_inputs: &[Fr]) -> bool {
        match &self.keys.unshield {
            Some(vk) => verifier::verify(vk, proof, public_inputs).unwrap_or(false),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unconfigured_rejects_all_proofs() {
        let verifier = Groth16Verifier::new(VerifyingKeys::unconfigured());
        assert!(!verifier.verify_transfer(&[0u8; 256], &[Fr::from(1u64)]));
        assert!(!verifier.verify_unshield(&[0u8; 256], &[Fr::from(1u64)]));
    }
}
