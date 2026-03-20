//! Genesis specification. The genesis hash is the chain identifier and the
//! parent hash of block 0.

use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

use crate::hash::{blake3_concat, H256};
use crate::PrimitivesError;

const GENESIS_DOMAIN: &[u8] = b"PL2:genesis:v1";

/// Serde helpers rendering `Pubkey` as base58 strings in TOML/JSON.
pub mod pubkey_b58 {
    use serde::{Deserialize, Deserializer, Serializer};
    use solana_sdk::pubkey::Pubkey;
    use std::str::FromStr;

    pub fn serialize<S: Serializer>(pk: &Pubkey, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&pk.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Pubkey, D::Error> {
        let raw = String::deserialize(d)?;
        Pubkey::from_str(&raw).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Allocation {
    #[serde(with = "pubkey_b58")]
    pub pubkey: Pubkey,
    pub lamports: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenesisConfig {
    pub chain_name: String,
    pub creation_time_ms: u64,
    #[serde(with = "pubkey_b58")]
    pub sequencer: Pubkey,
    #[serde(with = "pubkey_b58")]
    pub fee_collector: Pubkey,
    pub allocations: Vec<Allocation>,
}

impl GenesisConfig {
    pub fn from_toml_str(raw: &str) -> Result<Self, PrimitivesError> {
        let parsed: Self = toml::from_str(raw)?;
        parsed.validate()?;
        Ok(parsed)
    }

    pub fn load(path: &std::path::Path) -> Result<Self, PrimitivesError> {
        let raw = std::fs::read_to_string(path).map_err(|source| PrimitivesError::Io {
            path: path.display().to_string(),
            source,
        })?;
        Self::from_toml_str(&raw)
    }

    fn validate(&self) -> Result<(), PrimitivesError> {
        if self.chain_name.is_empty() {
            return Err(PrimitivesError::InvalidGenesis(
                "chain_name must not be empty".into(),
            ));
        }
        if self.allocations.is_empty() {
            return Err(PrimitivesError::InvalidGenesis(
                "at least one allocation is required".into(),
            ));
        }
        let mut seen = std::collections::HashSet::new();
        for a in &self.allocations {
            if a.lamports == 0 {
                return Err(PrimitivesError::InvalidGenesis(format!(
                    "zero-lamport allocation for {}",
                    a.pubkey
                )));
            }
            if !seen.insert(a.pubkey) {
                return Err(PrimitivesError::InvalidGenesis(format!(
                    "duplicate allocation for {}",
                    a.pubkey
                )));
            }
        }
        self.total_supply().ok_or_else(|| {
            PrimitivesError::InvalidGenesis("total supply overflows u64".into())
        })?;
        Ok(())
    }

    /// Sum of all allocations; the fixed native supply of the chain.
    pub fn total_supply(&self) -> Option<u64> {
        self.allocations
            .iter()
            .try_fold(0u64, |acc, a| acc.checked_add(a.lamports))
    }

    /// Canonical genesis hash. Allocations are sorted by pubkey before
    /// hashing so semantically identical files (differing only in entry
    /// order) identify the same chain.
    pub fn genesis_hash(&self) -> H256 {
        let mut sorted = self.allocations.clone();
        sorted.sort_by_key(|a| a.pubkey.to_bytes());

        let mut segments: Vec<Vec<u8>> = Vec::with_capacity(4 + sorted.len() * 2);
        segments.push(self.chain_name.as_bytes().to_vec());
        segments.push(self.creation_time_ms.to_le_bytes().to_vec());
        segments.push(self.sequencer.to_bytes().to_vec());
        segments.push(self.fee_collector.to_bytes().to_vec());
        for a in &sorted {
            segments.push(a.pubkey.to_bytes().to_vec());
            segments.push(a.lamports.to_le_bytes().to_vec());
        }
        let refs: Vec<&[u8]> = segments.iter().map(|s| s.as_slice()).collect();
        blake3_concat(GENESIS_DOMAIN, &refs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
chain_name = "zul-test"
creation_time_ms = 1760000000000
sequencer = "E1qwDui9Muq3gfyGwdk1qZXnNyy2WzgHkESJGr4KwJpm"
fee_collector = "6PaVUAPQqyFWxYaLbnVaiwcAkDqiqjCvMMmFoGmNdCFM"

[[allocations]]
pubkey = "Hvd9JUwyRzdm7m7yRcSQBphQtpPNWf5GmCEZRKoWJGSp"
lamports = 1000000000000000000

[[allocations]]
pubkey = "9jhBSsexL7jmQraCz1knhxBGcEE4gTwrKmE5m8ivhLx"
lamports = 500000000000000000
"#;

    #[test]
    fn parses_example() {
        let g = GenesisConfig::from_toml_str(EXAMPLE).unwrap();
        assert_eq!(g.chain_name, "zul-test");
        assert_eq!(g.allocations.len(), 2);
        assert_eq!(g.total_supply(), Some(1_500_000_000_000_000_000));
    }

    #[test]
    fn hash_ignores_allocation_order() {
        let g1 = GenesisConfig::from_toml_str(EXAMPLE).unwrap();
        let mut g2 = g1.clone();
        g2.allocations.reverse();
        assert_eq!(g1.genesis_hash(), g2.genesis_hash());
    }

    #[test]
    fn hash_changes_with_content() {
        let g1 = GenesisConfig::from_toml_str(EXAMPLE).unwrap();
        let mut g2 = g1.clone();
        g2.chain_name = "zul-other".into();
        assert_ne!(g1.genesis_hash(), g2.genesis_hash());
        let mut g3 = g1.clone();
        g3.allocations[0].lamports += 1;
        assert_ne!(g1.genesis_hash(), g3.genesis_hash());
    }

    #[test]
    fn rejects_duplicates_and_zero() {
        let dup = EXAMPLE.replace(
            "9jhBSsexL7jmQraCz1knhxBGcEE4gTwrKmE5m8ivhLx",
            "Hvd9JUwyRzdm7m7yRcSQBphQtpPNWf5GmCEZRKoWJGSp",
        );
        assert!(GenesisConfig::from_toml_str(&dup).is_err());

        let zero = EXAMPLE.replace("lamports = 500000000000000000", "lamports = 0");
        assert!(GenesisConfig::from_toml_str(&zero).is_err());
    }

    #[test]
    fn rejects_bad_pubkey() {
        let bad = EXAMPLE.replace(
            "E1qwDui9Muq3gfyGwdk1qZXnNyy2WzgHkESJGr4KwJpm",
            "not-a-pubkey",
        );
        assert!(GenesisConfig::from_toml_str(&bad).is_err());
    }
}
