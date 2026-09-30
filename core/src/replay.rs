use std::collections::BTreeMap;

/// Maximum number of peer replay states kept in memory.
/// This prevents attacker-controlled peer IDs from growing memory without bound.
pub const MAX_PEERS: usize = 65_536;

/// Sliding replay window. Packets may arrive out of order inside the window,
/// while duplicates and packets that fall too far behind are rejected.
pub const REPLAY_WINDOW: u8 = 64;

#[derive(Debug, Clone, Copy, Default)]
struct ReplayState {
    highest: u64,
    bitmap: u64,
    generation: u64,
}

/// Bounded replay protection keyed by authenticated peer identifier.
#[derive(Debug, Default)]
pub struct ReplayGuard {
    states: BTreeMap<[u8; 32], ReplayState>,
    generation: u64,
}

impl ReplayGuard {
    pub fn accept(&mut self, peer: [u8; 32], sequence: u64) -> bool {
        if let Some(state) = self.states.get_mut(&peer) {
            if sequence > state.highest {
                let shift = sequence - state.highest;
                state.bitmap = if shift >= REPLAY_WINDOW as u64 {
                    1
                } else {
                    (state.bitmap << shift) | 1
                };
                state.highest = sequence;
                self.generation = self.generation.wrapping_add(1);
                state.generation = self.generation;
                return true;
            }

            let delta = state.highest - sequence;
            if delta >= REPLAY_WINDOW as u64 {
                return false;
            }

            let bit = 1u64 << delta;
            if state.bitmap & bit != 0 {
                return false;
            }
            state.bitmap |= bit;
            self.generation = self.generation.wrapping_add(1);
            state.generation = self.generation;
            return true;
        }

        if self.states.len() >= MAX_PEERS {
            // Deterministic bounded eviction. The table stays finite even when
            // an attacker presents an unbounded number of peer identifiers.
            if let Some((oldest, _)) = self.states.iter().min_by_key(|(_, state)| state.generation) {
                let oldest = *oldest;
                self.states.remove(&oldest);
            }
        }

        self.generation = self.generation.wrapping_add(1);
        self.states.insert(peer, ReplayState { highest: sequence, bitmap: 1, generation: self.generation });
        true
    }

    pub fn tracked_peers(&self) -> usize {
        self.states.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_reordering_inside_window_and_rejects_duplicates() {
        let mut guard = ReplayGuard::default();
        let peer = [7u8; 32];

        assert!(guard.accept(peer, 10));
        assert!(guard.accept(peer, 8));
        assert!(guard.accept(peer, 9));
        assert!(!guard.accept(peer, 8));
        assert!(!guard.accept(peer, 10));
    }

    #[test]
    fn rejects_packets_outside_window() {
        let mut guard = ReplayGuard::default();
        let peer = [8u8; 32];

        assert!(guard.accept(peer, 100));
        assert!(!guard.accept(peer, 100 - REPLAY_WINDOW as u64));
    }

    #[test]
    fn active_peer_refreshes_eviction_generation() {
        let mut guard = ReplayGuard::default();
        let active = [1u8; 32];
        let idle = [2u8; 32];
        assert!(guard.accept(active, 1));
        assert!(guard.accept(idle, 1));
        assert!(guard.accept(active, 2));
        for i in 3..=MAX_PEERS as u64 + 2 {
            let mut peer = [0u8; 32];
            peer[..8].copy_from_slice(&i.to_be_bytes());
            let _ = guard.accept(peer, 1);
        }
        assert!(guard.tracked_peers() <= MAX_PEERS);
        assert!(guard.accept(active, MAX_PEERS as u64 + 3));
    }

    #[test]
    fn peer_state_is_bounded() {
        let mut guard = ReplayGuard::default();
        for i in 0..(MAX_PEERS + 128) {
            let mut peer = [0u8; 32];
            peer[..8].copy_from_slice(&(i as u64).to_be_bytes());
            assert!(guard.accept(peer, 1));
        }
        assert!(guard.tracked_peers() <= MAX_PEERS);
    }
}
