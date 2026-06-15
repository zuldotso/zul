//! Wrapped SPL tokens. A deposit of an L1 SPL token credits a deterministic L2
//! *wrapped* token: an SPL mint (classic Token program) whose address is derived
//! from the L1 mint, plus the holder's associated token account. The node
//! synthesizes these account states directly (the same way native deposits write
//! lamports) rather than running token-program CPIs — the byte layouts here are
//! exact, so once written the SVM Token program accepts them and the wrapped
//! balance is spendable like any other SPL token. The shielded pool uses the
//! same helpers to move wrapped balances in/out of its per-mint vault.
//!
//! The wrapped mint's `mint_authority` is a reserved pubkey nobody holds the
//! key for, so no one can `MintTo` through the SVM; issuance happens only in the
//! node's deposit synthesis, backed 1:1 by tokens locked in the L1 vault.

use crate::blake3_32;
use solana_sdk::pubkey::Pubkey;

/// Classic SPL Token program (well-known address). L2 wrapped tokens always use
/// it, regardless of whether the L1 mint was Token or Token-2022.
pub const TOKEN_PROGRAM_ID: Pubkey = Pubkey::new_from_array([
    6, 221, 246, 225, 215, 101, 161, 147, 217, 203, 225, 70, 206, 235, 121, 172, 28, 180, 133, 237,
    95, 91, 55, 145, 58, 140, 245, 133, 126, 255, 0, 169,
]);

/// Associated Token Account program (well-known address).
pub const ATA_PROGRAM_ID: Pubkey = Pubkey::new_from_array([
    140, 151, 37, 143, 78, 36, 137, 241, 187, 61, 16, 41, 20, 142, 13, 131, 11, 90, 19, 153, 218,
    255, 16, 132, 4, 142, 123, 216, 219, 233, 248, 89,
]);

const WRAPPED_MINT_DOMAIN: &[u8] = b"PL2:bridge:wrapped-mint:v1";

/// Solana's canonical wrapped-SOL mint. When a gas mint is configured (mainnet),
/// a native-SOL deposit is credited as the wrapped token of this mint, so SOL is
/// a normal bridged asset, not the gas token.
pub fn native_sol_mint() -> Pubkey {
    "So11111111111111111111111111111111111111112"
        .parse()
        .expect("valid wSOL mint")
}

/// Reserved authority recorded as the wrapped mint's `mint_authority`. It is
/// not a real keypair (no one can sign for it), so the only way wrapped supply
/// can grow is the node's deposit synthesis — never an SVM `MintTo`.
pub const BRIDGE_MINT_AUTHORITY: Pubkey = Pubkey::new_from_array([
    0x06, 0x21, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0x02,
]);

pub const MINT_LEN: usize = 82;
pub const ACCOUNT_LEN: usize = 165;

/// Deterministic L2 address of the wrapped mint for an L1 mint.
pub fn wrapped_mint_address(l1_mint: &Pubkey) -> Pubkey {
    Pubkey::new_from_array(blake3_32(WRAPPED_MINT_DOMAIN, l1_mint.as_ref()))
}

/// Associated token account address for `(owner, mint)` under the classic Token
/// program — the standard derivation wallets and the SDK use.
pub fn associated_token_address(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[owner.as_ref(), TOKEN_PROGRAM_ID.as_ref(), mint.as_ref()],
        &ATA_PROGRAM_ID,
    )
    .0
}

/// Pack an SPL `Mint` (82 bytes): COption authority, supply, decimals,
/// is_initialized, COption freeze authority (none).
pub fn pack_mint(mint_authority: &Pubkey, supply: u64, decimals: u8) -> Vec<u8> {
    let mut data = vec![0u8; MINT_LEN];
    data[0..4].copy_from_slice(&1u32.to_le_bytes()); // COption::Some
    data[4..36].copy_from_slice(mint_authority.as_ref());
    data[36..44].copy_from_slice(&supply.to_le_bytes());
    data[44] = decimals;
    data[45] = 1; // is_initialized
    // freeze_authority COption::None — tag bytes [46..50] already zero.
    data
}

/// Read the `supply` field of a packed SPL `Mint`.
pub fn mint_supply(data: &[u8]) -> Option<u64> {
    data.get(36..44)
        .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
}

/// Read the `decimals` field of a packed SPL `Mint`.
pub fn mint_decimals(data: &[u8]) -> Option<u8> {
    data.get(44).copied()
}

/// Set the `supply` field in place, leaving every other byte untouched.
pub fn set_mint_supply(data: &mut [u8], supply: u64) {
    data[36..44].copy_from_slice(&supply.to_le_bytes());
}

/// Pack an SPL token `Account` (165 bytes) in the `Initialized` state holding
/// `amount` of `mint`, owned by `owner`. No delegate / close authority; not a
/// native (wrapped-SOL) account.
pub fn pack_token_account(mint: &Pubkey, owner: &Pubkey, amount: u64) -> Vec<u8> {
    let mut data = vec![0u8; ACCOUNT_LEN];
    data[0..32].copy_from_slice(mint.as_ref());
    data[32..64].copy_from_slice(owner.as_ref());
    data[64..72].copy_from_slice(&amount.to_le_bytes());
    // delegate COption::None [72..76] zero.
    data[108] = 1; // AccountState::Initialized
    // is_native COption::None [109..113] zero; delegated_amount [121..129] zero;
    // close_authority COption::None [129..133] zero.
    data
}

/// Read the `amount` field of a packed SPL token `Account`.
pub fn token_account_amount(data: &[u8]) -> Option<u64> {
    data.get(64..72)
        .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
}

/// Set the `amount` field in place, leaving delegate / state / close-authority
/// (which the SVM may have set) untouched.
pub fn set_token_account_amount(data: &mut [u8], amount: u64) {
    data[64..72].copy_from_slice(&amount.to_le_bytes());
}

/// Rent-exempt minimum lamports for `data_len` bytes under `Rent::default()`
/// (`(ACCOUNT_STORAGE_OVERHEAD + len) * lamports_per_byte_year * 2`). Matches
/// Solana mainnet's rent, computed without pulling in the rent crate.
pub const fn rent_exempt_lamports(data_len: usize) -> u64 {
    (128 + data_len as u64) * 3480 * 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_program_ids() {
        assert_eq!(
            TOKEN_PROGRAM_ID.to_string(),
            "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
        );
        assert_eq!(
            ATA_PROGRAM_ID.to_string(),
            "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"
        );
    }

    #[test]
    fn mint_layout_roundtrips() {
        let auth = Pubkey::new_unique();
        let data = pack_mint(&auth, 1_000, 6);
        assert_eq!(data.len(), MINT_LEN);
        assert_eq!(mint_supply(&data), Some(1_000));
        assert_eq!(data[44], 6); // decimals
        assert_eq!(data[45], 1); // is_initialized
        assert_eq!(&data[4..36], auth.as_ref());
    }

    #[test]
    fn account_layout_roundtrips() {
        let mint = Pubkey::new_unique();
        let owner = Pubkey::new_unique();
        let data = pack_token_account(&mint, &owner, 42);
        assert_eq!(data.len(), ACCOUNT_LEN);
        assert_eq!(&data[0..32], mint.as_ref());
        assert_eq!(&data[32..64], owner.as_ref());
        assert_eq!(token_account_amount(&data), Some(42));
        assert_eq!(data[108], 1); // Initialized
    }
}
