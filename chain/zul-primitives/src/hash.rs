//! Hashing helpers. All ZUL-internal commitments (block hashes, merkle
//! roots, state roots) use blake3 with domain separation prefixes.

/// A 32-byte hash. Stored raw; rendered as base58 at API boundaries to match
/// Solana conventions.
pub type H256 = [u8; 32];

/// The all-zero hash, used as the parent of the genesis block.
pub const ZERO_HASH: H256 = [0u8; 32];

/// blake3 of `domain || data`. Domain separation keeps hashes from one
/// context (e.g. merkle leaves) from being valid in another (e.g. headers).
pub fn blake3_32(domain: &[u8], data: &[u8]) -> H256 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hasher.update(data);
    *hasher.finalize().as_bytes()
}

/// blake3 over multiple segments under one domain.
pub fn blake3_concat(domain: &[u8], segments: &[&[u8]]) -> H256 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    for s in segments {
        hasher.update(s);
    }
    *hasher.finalize().as_bytes()
}

/// Render a hash as base58 (Solana-style).
pub fn to_base58(h: &H256) -> String {
    bs58::encode(h).into_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_separate() {
        assert_ne!(blake3_32(b"a", b"x"), blake3_32(b"b", b"x"));
        assert_ne!(blake3_32(b"a", b"x"), blake3_32(b"a", b"y"));
    }

    #[test]
    fn concat_matches_manual() {
        let joined = blake3_32(b"d", b"helloworld");
        let parts = blake3_concat(b"d", &[b"hello", b"world"]);
        assert_eq!(joined, parts);
    }

    #[test]
    fn base58_roundtrip() {
        let h = blake3_32(b"t", b"v");
        let s = to_base58(&h);
        let back = bs58::decode(&s).into_vec().unwrap();
        assert_eq!(back.as_slice(), h.as_slice());
    }
}
