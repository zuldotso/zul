//! Bounded FIFO mempool. Single sequencer, so ordering is arrival order;
//! priority-fee ordering can slot in here later without touching callers.
//!
//! Anti-spam: besides the global capacity and signature dedup, each fee
//! payer may hold at most `max_per_payer` transactions in flight, so one key
//! cannot fill the queue and starve everyone else (fees are fixed per
//! signature, so a per-payer cap is the effective lever).

use solana_sdk::{pubkey::Pubkey, signature::Signature, transaction::VersionedTransaction};
use std::collections::{HashMap, HashSet, VecDeque};

pub struct Mempool {
    capacity: usize,
    max_per_payer: usize,
    queue: VecDeque<VersionedTransaction>,
    pending: HashSet<Signature>,
    per_payer: HashMap<Pubkey, usize>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum MempoolError {
    Full,
    Duplicate,
    Unsigned,
    PayerLimit,
}

/// The fee payer is the first static account key.
fn fee_payer(tx: &VersionedTransaction) -> Option<Pubkey> {
    tx.message.static_account_keys().first().copied()
}

impl Mempool {
    pub fn new(capacity: usize, max_per_payer: usize) -> Self {
        Self {
            capacity,
            max_per_payer: max_per_payer.max(1),
            queue: VecDeque::new(),
            pending: HashSet::new(),
            per_payer: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn push(&mut self, tx: VersionedTransaction) -> Result<Signature, MempoolError> {
        let Some(sig) = tx.signatures.first().copied() else {
            return Err(MempoolError::Unsigned);
        };
        let Some(payer) = fee_payer(&tx) else {
            return Err(MempoolError::Unsigned);
        };
        if self.pending.contains(&sig) {
            return Err(MempoolError::Duplicate);
        }
        if self.per_payer.get(&payer).copied().unwrap_or(0) >= self.max_per_payer {
            return Err(MempoolError::PayerLimit);
        }
        if self.queue.len() >= self.capacity {
            return Err(MempoolError::Full);
        }
        self.pending.insert(sig);
        *self.per_payer.entry(payer).or_insert(0) += 1;
        self.queue.push_back(tx);
        Ok(sig)
    }

    /// Remove up to `max` transactions for the next block.
    pub fn drain(&mut self, max: usize) -> Vec<VersionedTransaction> {
        let take = max.min(self.queue.len());
        let drained: Vec<VersionedTransaction> = self.queue.drain(..take).collect();
        for tx in &drained {
            if let Some(sig) = tx.signatures.first() {
                self.pending.remove(sig);
            }
            if let Some(payer) = fee_payer(tx) {
                if let Some(count) = self.per_payer.get_mut(&payer) {
                    *count -= 1;
                    if *count == 0 {
                        self.per_payer.remove(&payer);
                    }
                }
            }
        }
        drained
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::{
        hash::Hash, pubkey::Pubkey, signature::Keypair, signer::Signer,
        transaction::Transaction,
    };
    use solana_system_interface::instruction as system_instruction;

    fn tx(n: u64) -> VersionedTransaction {
        tx_from(&Keypair::new(), n)
    }

    fn tx_from(kp: &Keypair, n: u64) -> VersionedTransaction {
        let ix = system_instruction::transfer(&kp.pubkey(), &Pubkey::new_unique(), n);
        VersionedTransaction::from(Transaction::new_signed_with_payer(
            &[ix],
            Some(&kp.pubkey()),
            &[kp],
            // Vary the blockhash so each tx has a distinct signature.
            Hash::new_from_array([n as u8; 32]),
        ))
    }

    #[test]
    fn fifo_order_and_pending_tracking() {
        let mut pool = Mempool::new(10, 64);
        let t1 = tx(1);
        let t2 = tx(2);
        let s1 = pool.push(t1.clone()).unwrap();
        pool.push(t2.clone()).unwrap();
        assert_eq!(pool.len(), 2);

        // Duplicate rejected while pending.
        assert_eq!(pool.push(t1.clone()), Err(MempoolError::Duplicate));

        let drained = pool.drain(1);
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].signatures[0], s1);
        assert_eq!(pool.len(), 1);

        // After draining, the signature may re-enter (store-level replay
        // protection is the executor's job).
        assert!(pool.push(t1).is_ok());
    }

    #[test]
    fn capacity_enforced() {
        let mut pool = Mempool::new(2, 64);
        pool.push(tx(1)).unwrap();
        pool.push(tx(2)).unwrap();
        assert_eq!(pool.push(tx(3)), Err(MempoolError::Full));
    }

    #[test]
    fn unsigned_rejected() {
        let mut pool = Mempool::new(2, 64);
        let unsigned = VersionedTransaction::default();
        assert_eq!(pool.push(unsigned), Err(MempoolError::Unsigned));
    }

    #[test]
    fn per_payer_cap_stops_one_key_flooding() {
        let mut pool = Mempool::new(1000, 3);
        let spammer = Keypair::new();
        for n in 0..3 {
            pool.push(tx_from(&spammer, n)).unwrap();
        }
        // The 4th from the same payer is rejected even though the queue has
        // room (1000 capacity).
        assert_eq!(pool.push(tx_from(&spammer, 99)), Err(MempoolError::PayerLimit));
        // A different payer is unaffected.
        assert!(pool.push(tx_from(&Keypair::new(), 0)).is_ok());
        // Draining the spammer's txs frees their slots again.
        pool.drain(3);
        assert!(pool.push(tx_from(&spammer, 100)).is_ok());
    }
}
