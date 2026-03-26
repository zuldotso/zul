//! Sysvar account maintenance. The sequencer refreshes Clock each slot and
//! seeds the static sysvars (Rent, EpochSchedule, a placeholder
//! RecentBlockhashes for nonce instructions) at genesis.

use solana_clock::Clock;
use solana_epoch_schedule::EpochSchedule;
use solana_rent::Rent;
use solana_sdk::{
    account::{Account, AccountSharedData},
    pubkey::Pubkey,
};
use solana_sysvar_id::SysvarId;

fn sysvar_account<T: serde::Serialize>(value: &T) -> AccountSharedData {
    let data = bincode::serialize(value).expect("sysvar serialization is infallible");
    AccountSharedData::from(Account {
        lamports: 1,
        data,
        owner: solana_sdk_ids::sysvar::id(),
        executable: false,
        rent_epoch: zul_store::RENT_EXEMPT_RENT_EPOCH,
    })
}

/// Epoch schedule for ZUL: no warmup; epochs exist only because the SVM
/// program cache is epoch-aware.
pub fn epoch_schedule() -> EpochSchedule {
    EpochSchedule::without_warmup()
}

/// The clock account for a slot being produced.
pub fn clock_account(slot: u64, timestamp_ms: u64) -> (Pubkey, AccountSharedData) {
    let schedule = epoch_schedule();
    let epoch = schedule.get_epoch(slot);
    let clock = Clock {
        slot,
        epoch_start_timestamp: (timestamp_ms / 1000) as i64,
        epoch,
        leader_schedule_epoch: epoch,
        unix_timestamp: (timestamp_ms / 1000) as i64,
    };
    (Clock::id(), sysvar_account(&clock))
}

/// Static sysvars seeded once at genesis.
pub fn static_sysvar_accounts() -> Vec<(Pubkey, AccountSharedData)> {
    let mut out = Vec::new();
    out.push((Rent::id(), sysvar_account(&Rent::default())));
    out.push((EpochSchedule::id(), sysvar_account(&epoch_schedule())));

    // SystemInstruction::AdvanceNonceAccount requires a non-empty
    // RecentBlockhashes sysvar but reads the actual hash elsewhere.
    #[allow(deprecated)]
    {
        use solana_sysvar::recent_blockhashes::{Entry, RecentBlockhashes};
        let entries = vec![Entry::default()];
        out.push((RecentBlockhashes::id(), sysvar_account(&entries)));
    }
    out
}

/// Epoch of a slot under the ZUL schedule.
pub fn epoch_for_slot(slot: u64) -> u64 {
    epoch_schedule().get_epoch(slot)
}
