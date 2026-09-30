//! Lightweight in-memory admission controls for hostile P2P traffic.

#[derive(Debug, Clone, Copy)]
pub struct TokenBucket {
    capacity: u64,
    tokens: u64,
    refill_per_second: u64,
    last_second: u64,
}

impl TokenBucket {
    pub fn new(capacity: u64, refill_per_second: u64, now_second: u64) -> Self {
        let capacity = capacity.max(1);
        Self { capacity, tokens: capacity, refill_per_second: refill_per_second.max(1), last_second: now_second }
    }

    pub fn allow(&mut self, cost: u64, now_second: u64) -> bool {
        if cost == 0 { return true; }
        if cost > self.capacity { return false; }

        let elapsed = now_second.saturating_sub(self.last_second);
        if elapsed > 0 {
            let refill = elapsed.saturating_mul(self.refill_per_second);
            self.tokens = self.tokens.saturating_add(refill).min(self.capacity);
            self.last_second = now_second;
        }

        if self.tokens < cost { return false; }
        self.tokens -= cost;
        true
    }

    pub fn remaining(&self) -> u64 { self.tokens }
}

#[derive(Debug)]
pub struct PeerAdmission {
    buckets: std::collections::BTreeMap<[u8; 32], TokenBucket>,
    max_peers: usize,
    capacity: u64,
    refill_per_second: u64,
}

impl PeerAdmission {
    pub fn new(max_peers: usize, capacity: u64, refill_per_second: u64) -> Self {
        Self {
            buckets: std::collections::BTreeMap::new(),
            max_peers: max_peers.max(1),
            capacity: capacity.max(1),
            refill_per_second: refill_per_second.max(1),
        }
    }

    pub fn allow(&mut self, peer: [u8; 32], cost: u64, now_second: u64) -> bool {
        if !self.buckets.contains_key(&peer) {
            if self.buckets.len() >= self.max_peers {
                if let Some(oldest) = self.buckets.keys().next().copied() {
                    self.buckets.remove(&oldest);
                }
            }
            self.buckets.insert(
                peer,
                TokenBucket::new(self.capacity, self.refill_per_second, now_second),
            );
        }
        self.buckets.get_mut(&peer).expect("peer bucket inserted").allow(cost, now_second)
    }

    pub fn tracked_peers(&self) -> usize { self.buckets.len() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_refills_and_caps() {
        let mut b = TokenBucket::new(10, 5, 0);
        assert!(b.allow(10, 0));
        assert!(!b.allow(1, 0));
        assert!(b.allow(5, 1));
        assert!(!b.allow(6, 1));
        assert!(b.allow(5, 2));
        assert_eq!(b.remaining(), 0);
    }

    #[test]
    fn expensive_request_is_rejected_without_underflow() {
        let mut b = TokenBucket::new(10, 10, 0);
        assert!(!b.allow(11, 0));
        assert_eq!(b.remaining(), 10);
    }

    #[test]
    fn admission_state_is_bounded() {
        let mut a = PeerAdmission::new(4, 10, 10);
        for i in 0..100u64 {
            let mut peer = [0u8; 32];
            peer[..8].copy_from_slice(&i.to_be_bytes());
            assert!(a.allow(peer, 1, 0));
        }
        assert!(a.tracked_peers() <= 4);
    }
}
