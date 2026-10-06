//! AWENET defense-in-depth controls for hostile peers and abusive workloads.
//! These controls are deliberately deterministic, bounded, and independent from
//! cryptographic authentication. They reduce blast radius rather than claiming
//! that attacks are impossible.

use std::collections::BTreeMap;

pub const DEFAULT_MAX_STRIKES: u32 = 8;
pub const DEFAULT_QUARANTINE_SECS: u64 = 300;
pub const DEFAULT_MAX_ENTRIES: usize = 65_536;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PeerDefenseState {
    pub strikes: u32,
    pub quarantined_until: u64,
    pub last_seen: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefenseDecision {
    Allow,
    Quarantined,
    RateLimited,
    Banned,
}

#[derive(Debug)]
pub struct PeerDefense {
    states: BTreeMap<[u8; 32], PeerDefenseState>,
    max_entries: usize,
    max_strikes: u32,
    quarantine_secs: u64,
}

impl Default for PeerDefense {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_ENTRIES, DEFAULT_MAX_STRIKES, DEFAULT_QUARANTINE_SECS)
    }
}

impl PeerDefense {
    pub fn new(max_entries: usize, max_strikes: u32, quarantine_secs: u64) -> Self {
        Self {
            states: BTreeMap::new(),
            max_entries: max_entries.max(1),
            max_strikes: max_strikes.max(1),
            quarantine_secs,
        }
    }

    fn ensure(&mut self, peer: [u8; 32], now: u64) {
        if self.states.contains_key(&peer) { return; }
        if self.states.len() >= self.max_entries {
            if let Some((oldest, _)) = self.states.iter().min_by_key(|(_, s)| s.last_seen) {
                let oldest = *oldest;
                self.states.remove(&oldest);
            }
        }
        self.states.insert(peer, PeerDefenseState { last_seen: now, ..Default::default() });
    }

    pub fn check(&mut self, peer: [u8; 32], now: u64) -> DefenseDecision {
        self.ensure(peer, now);
        let state = self.states.get_mut(&peer).expect("peer state exists");
        state.last_seen = now;
        if state.strikes >= self.max_strikes { return DefenseDecision::Banned; }
        if state.quarantined_until > now { return DefenseDecision::Quarantined; }
        DefenseDecision::Allow
    }

    pub fn strike(&mut self, peer: [u8; 32], now: u64) -> DefenseDecision {
        self.ensure(peer, now);
        let state = self.states.get_mut(&peer).expect("peer state exists");
        state.last_seen = now;
        state.strikes = state.strikes.saturating_add(1);
        if state.strikes >= self.max_strikes {
            return DefenseDecision::Banned;
        }
        state.quarantined_until = now.saturating_add(self.quarantine_secs);
        DefenseDecision::Quarantined
    }

    pub fn clear_quarantine(&mut self, peer: [u8; 32]) {
        if let Some(state) = self.states.get_mut(&peer) { state.quarantined_until = 0; }
    }

    pub fn strikes(&self, peer: &[u8; 32]) -> u32 {
        self.states.get(peer).map(|s| s.strikes).unwrap_or(0)
    }

    pub fn tracked_peers(&self) -> usize { self.states.len() }
}

/// Sliding-window circuit breaker for expensive operations. A caller is
/// temporarily isolated after too many failures, preventing one bad peer from
/// consuming unlimited CPU/storage/relay resources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CircuitBreaker {
    pub failures: u32,
    pub opened_until: u64,
    pub threshold: u32,
    pub cooldown_secs: u64,
}

impl CircuitBreaker {
    pub fn new(threshold: u32, cooldown_secs: u64) -> Self {
        Self { failures: 0, opened_until: 0, threshold: threshold.max(1), cooldown_secs }
    }
    pub fn allow(&mut self, now: u64) -> bool {
        if self.opened_until > now { return false; }
        if self.opened_until != 0 { self.failures = 0; self.opened_until = 0; }
        true
    }
    pub fn record_failure(&mut self, now: u64) {
        self.failures = self.failures.saturating_add(1);
        if self.failures >= self.threshold {
            self.opened_until = now.saturating_add(self.cooldown_secs);
        }
    }
    pub fn record_success(&mut self) { self.failures = 0; }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_bad_peer_is_quarantined_then_banned() {
        let mut d = PeerDefense::new(2, 3, 10);
        let peer = [7u8; 32];
        assert_eq!(d.check(peer, 1), DefenseDecision::Allow);
        assert_eq!(d.strike(peer, 1), DefenseDecision::Quarantined);
        assert_eq!(d.check(peer, 2), DefenseDecision::Quarantined);
        d.clear_quarantine(peer);
        assert_eq!(d.strike(peer, 3), DefenseDecision::Quarantined);
        d.clear_quarantine(peer);
        assert_eq!(d.strike(peer, 4), DefenseDecision::Banned);
        assert_eq!(d.check(peer, 20), DefenseDecision::Banned);
    }

    #[test]
    fn defense_state_is_bounded() {
        let mut d = PeerDefense::new(4, 3, 10);
        for i in 0..100u64 {
            let mut p = [0u8; 32];
            p[..8].copy_from_slice(&i.to_be_bytes());
            assert_eq!(d.check(p, i), DefenseDecision::Allow);
        }
        assert!(d.tracked_peers() <= 4);
    }

    #[test]
    fn circuit_breaker_reopens_after_cooldown() {
        let mut c = CircuitBreaker::new(2, 10);
        c.record_failure(1);
        assert!(c.allow(2));
        c.record_failure(2);
        assert!(!c.allow(3));
        assert!(c.allow(12));
    }
}
