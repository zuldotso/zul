//! Deposit processing: turn observed L1 deposits into L2 credit
//! transactions, exactly once.
//!
//! - SOL deposited into the L1 vault mints wrapped-SOL (an L2 SPL token
//!   whose mint authority is the bridge authority).
//! - SPL-ZUL deposited on L1 credits native ZUL lamports on L2 (a System
//!   transfer from the bridge reserve allocation).
//!
//! Processed L1 signatures are persisted by the caller (node meta) through
//! the `DepositLedger` trait so restarts never double-credit.

use crate::client::{DepositEvent, L1Client, L1Error};
use solana_sdk::pubkey::Pubkey;

/// Persistence for the dedup cursor + processed set.
pub trait DepositLedger {
    fn is_processed(&self, l1_signature: &str) -> bool;
    fn mark_processed(&mut self, l1_signature: &str);
    /// Last fully processed signature, for the L1 fetch cursor.
    fn cursor(&self) -> Option<String>;
}

/// An L2 credit the node must execute for a deposit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreditAction {
    /// Credit native ZUL from the bridge reserve to the recipient.
    CreditNative { recipient: Pubkey, lamports: u64 },
    /// Mint wrapped-SOL (L2 SPL) to the recipient's token account.
    MintWrappedSol { recipient: Pubkey, amount: u64 },
}

pub struct DepositProcessor<C: L1Client> {
    client: C,
    /// L1 mint of SPL-ZUL (deposits of it become native ZUL on L2).
    zul_mint_on_l1: Pubkey,
}

impl<C: L1Client> DepositProcessor<C> {
    pub fn new(client: C, zul_mint_on_l1: Pubkey) -> Self {
        Self {
            client,
            zul_mint_on_l1,
        }
    }

    /// Fetch new deposits and map them to credit actions. The caller
    /// executes each action as an L2 transaction and then marks it
    /// processed (in that order: crediting twice is worse than retrying).
    pub fn poll(
        &self,
        ledger: &mut dyn DepositLedger,
    ) -> Result<Vec<(DepositEvent, CreditAction)>, L1Error> {
        let cursor = ledger.cursor();
        let events = self.client.fetch_deposits(cursor.as_deref())?;
        let mut actions = Vec::new();
        for event in events {
            if ledger.is_processed(&event.l1_signature) {
                continue;
            }
            let action = match event.mint {
                None => CreditAction::MintWrappedSol {
                    recipient: event.l2_recipient,
                    amount: event.amount,
                },
                Some(mint) if mint == self.zul_mint_on_l1 => CreditAction::CreditNative {
                    recipient: event.l2_recipient,
                    lamports: event.amount,
                },
                // Unknown mints are ignored in v1 (no generic token bridge).
                Some(_) => continue,
            };
            actions.push((event, action));
        }
        Ok(actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::BatchRecord;
    use std::collections::HashSet;
    use std::sync::Mutex;

    struct MockL1 {
        deposits: Mutex<Vec<DepositEvent>>,
    }

    impl L1Client for MockL1 {
        fn post_da_chunk(&self, _: u64, _: u32, _: u32, _: &[u8]) -> Result<String, L1Error> {
            unreachable!()
        }
        fn post_batch_record(&self, _: &BatchRecord) -> Result<String, L1Error> {
            unreachable!()
        }
        fn latest_batch_no(&self) -> Result<Option<u64>, L1Error> {
            Ok(None)
        }
        fn fetch_deposits(&self, after: Option<&str>) -> Result<Vec<DepositEvent>, L1Error> {
            let all = self.deposits.lock().unwrap();
            let start = after
                .and_then(|cursor| all.iter().position(|d| d.l1_signature == cursor))
                .map(|i| i + 1)
                .unwrap_or(0);
            Ok(all[start..].to_vec())
        }
    }

    #[derive(Default)]
    struct MemLedger {
        processed: HashSet<String>,
        cursor: Option<String>,
    }

    impl DepositLedger for MemLedger {
        fn is_processed(&self, sig: &str) -> bool {
            self.processed.contains(sig)
        }
        fn mark_processed(&mut self, sig: &str) {
            self.processed.insert(sig.to_string());
            self.cursor = Some(sig.to_string());
        }
        fn cursor(&self) -> Option<String> {
            self.cursor.clone()
        }
    }

    fn event(sig: &str, mint: Option<Pubkey>, amount: u64) -> DepositEvent {
        DepositEvent {
            l1_signature: sig.into(),
            mint,
            amount,
            l2_recipient: Pubkey::new_unique(),
        }
    }

    #[test]
    fn maps_sol_and_zul_deposits_and_skips_unknown_mints() {
        let zul_mint = Pubkey::new_unique();
        let unknown = Pubkey::new_unique();
        let client = MockL1 {
            deposits: Mutex::new(vec![
                event("sig-1", None, 500),
                event("sig-2", Some(zul_mint), 700),
                event("sig-3", Some(unknown), 900),
            ]),
        };
        let processor = DepositProcessor::new(client, zul_mint);
        let mut ledger = MemLedger::default();

        let actions = processor.poll(&mut ledger).unwrap();
        assert_eq!(actions.len(), 2);
        assert!(matches!(
            actions[0].1,
            CreditAction::MintWrappedSol { amount: 500, .. }
        ));
        assert!(matches!(
            actions[1].1,
            CreditAction::CreditNative { lamports: 700, .. }
        ));
    }

    #[test]
    fn exactly_once_via_ledger_cursor() {
        let zul_mint = Pubkey::new_unique();
        let client = MockL1 {
            deposits: Mutex::new(vec![
                event("sig-1", None, 100),
                event("sig-2", None, 200),
            ]),
        };
        let processor = DepositProcessor::new(client, zul_mint);
        let mut ledger = MemLedger::default();

        let first = processor.poll(&mut ledger).unwrap();
        assert_eq!(first.len(), 2);
        for (e, _) in &first {
            ledger.mark_processed(&e.l1_signature);
        }

        // Second poll from the cursor: nothing new.
        assert!(processor.poll(&mut ledger).unwrap().is_empty());

        // Even with a reset cursor, the processed set blocks re-credit.
        ledger.cursor = None;
        assert!(processor.poll(&mut ledger).unwrap().is_empty());
    }
}
