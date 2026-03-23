//! Recent-blockhash queue, mirroring Solana's transaction recency rule:
//! a transaction is accepted only if its `recent_blockhash` is among the
//! last `BLOCKHASH_QUEUE_CAPACITY` block hashes (or the genesis hash while
//! the chain is younger than that).

use zul_primitives::constants::BLOCKHASH_QUEUE_CAPACITY;
use zul_primitives::hash::H256;
use serde::{Deserialize, Serialize};
use solana_sdk::hash::Hash;
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockhashQueue {
    capacity: usize,
    entries: VecDeque<(u64, H256)>,
    #[serde(skip)]
    index: HashMap<H256, u64>,
}

impl BlockhashQueue {
    pub fn new() -> Self {
        Self::with_capacity(BLOCKHASH_QUEUE_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        assert!(capacity > 0);
        Self {
            capacity,
            entries: VecDeque::with_capacity(capacity + 1),
            index: HashMap::with_capacity(capacity + 1),
        }
    }

    /// Register the hash produced at `slot`. Slots must be registered in
    /// increasing order.
    pub fn register(&mut self, slot: u64, hash: H256) {
        if let Some((last_slot, _)) = self.entries.back() {
            debug_assert!(*last_slot < slot, "slots must be registered in order");
        }
        self.entries.push_back((slot, hash));
        self.index.insert(hash, slot);
        while self.entries.len() > self.capacity {
            if let Some((_, evicted)) = self.entries.pop_front() {
                self.index.remove(&evicted);
            }
        }
    }

    pub fn is_valid(&self, hash: &H256) -> bool {
        self.index.contains_key(hash)
    }

    pub fn is_valid_hash(&self, hash: &Hash) -> bool {
        self.is_valid(&hash.to_bytes())
    }

    /// Most recently registered (slot, hash).
    pub fn latest(&self) -> Option<(u64, H256)> {
        self.entries.back().copied()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        bincode::serialize(self).expect("queue serialization is infallible")
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, bincode::Error> {
        let mut queue: Self = bincode::deserialize(bytes)?;
        queue.index = queue.entries.iter().map(|(s, h)| (*h, *s)).collect();
        Ok(queue)
    }
}

impl Default for BlockhashQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(n: u8) -> H256 {
        [n; 32]
    }

    #[test]
    fn registers_and_validates() {
        let mut q = BlockhashQueue::with_capacity(3);
        q.register(0, h(0));
        q.register(1, h(1));
        assert!(q.is_valid(&h(0)));
        assert!(q.is_valid(&h(1)));
        assert!(!q.is_valid(&h(9)));
        assert_eq!(q.latest(), Some((1, h(1))));
    }

    #[test]
    fn evicts_beyond_capacity() {
        let mut q = BlockhashQueue::with_capacity(2);
        q.register(0, h(0));
        q.register(1, h(1));
        q.register(2, h(2));
        assert!(!q.is_valid(&h(0)));
        assert!(q.is_valid(&h(1)));
        assert!(q.is_valid(&h(2)));
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn serde_roundtrip_rebuilds_index() {
        let mut q = BlockhashQueue::with_capacity(5);
        q.register(10, h(1));
        q.register(11, h(2));
        let restored = BlockhashQueue::from_bytes(&q.to_bytes()).unwrap();
        assert!(restored.is_valid(&h(1)));
        assert!(restored.is_valid(&h(2)));
        assert_eq!(restored.latest(), Some((11, h(2))));
    }
}
