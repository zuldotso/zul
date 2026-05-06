//! BN254 scalar field <-> byte conversions.
//!
//! Wire convention everywhere in ZUL (instructions, account state, SDK):
//! field elements travel as 32-byte little-endian values, matching ark's
//! `into_bigint().to_bytes_le()`. The TS SDK mirrors this.

use ark_bn254::Fr;
use ark_ff::{BigInteger, PrimeField};

pub type FieldBytes = [u8; 32];

/// Parse 32 LE bytes into Fr, rejecting non-canonical (>= modulus) values.
/// `from_le_bytes_mod_order` reduces mod p; re-serializing and comparing
/// detects inputs that were not already canonical.
pub fn fr_from_bytes(bytes: &FieldBytes) -> Option<Fr> {
    let fr = Fr::from_le_bytes_mod_order(bytes);
    (fr_to_bytes(&fr) == *bytes).then_some(fr)
}

pub fn fr_to_bytes(fr: &Fr) -> FieldBytes {
    let mut out = [0u8; 32];
    let bytes = fr.into_bigint().to_bytes_le();
    out[..bytes.len()].copy_from_slice(&bytes);
    out
}

/// u64 into the field (amounts, indices).
pub fn fr_from_u64(value: u64) -> Fr {
    Fr::from(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_ff::UniformRand;

    #[test]
    fn roundtrip_random() {
        let mut rng = ark_std::test_rng();
        for _ in 0..32 {
            let fr = Fr::rand(&mut rng);
            let bytes = fr_to_bytes(&fr);
            assert_eq!(fr_from_bytes(&bytes), Some(fr));
        }
    }

    #[test]
    fn rejects_modulus_overflow() {
        // The field modulus itself is non-canonical.
        let modulus_le: FieldBytes = [
            0x01, 0x00, 0x00, 0xf0, 0x93, 0xf5, 0xe1, 0x43, 0x91, 0x70, 0xb9, 0x79, 0x48, 0xe8,
            0x33, 0x28, 0x5d, 0x58, 0x81, 0x81, 0xb6, 0x45, 0x50, 0xb8, 0x29, 0xa0, 0x31, 0xe1,
            0x72, 0x4e, 0x64, 0x30,
        ];
        assert!(fr_from_bytes(&modulus_le).is_none());
        // All 0xff is far above the modulus.
        assert!(fr_from_bytes(&[0xff; 32]).is_none());
    }

    #[test]
    fn zero_and_small_values() {
        assert_eq!(fr_from_bytes(&[0u8; 32]), Some(Fr::from(0u64)));
        let mut one = [0u8; 32];
        one[0] = 1;
        assert_eq!(fr_from_bytes(&one), Some(Fr::from(1u64)));
        assert_eq!(fr_to_bytes(&fr_from_u64(7)), {
            let mut b = [0u8; 32];
            b[0] = 7;
            b
        });
    }
}
