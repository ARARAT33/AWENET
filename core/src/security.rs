//! AWEP2P Shield: protocol-level defense-in-depth for hostile networks.
//!
//! This is intentionally independent of the encrypted transport. It binds every
//! application packet to a protocol domain, session, sequence, expiry and payload
//! commitment. The keyed tag is a second integrity boundary; it is not a replacement
//! for authenticated encryption or identity signatures.

use blake3::Hasher;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const SHIELD_VERSION: u16 = 1;
pub const MAX_TTL: u8 = 32;
pub const MAX_PAYLOAD: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PacketClass {
    Control,
    Lookup,
    Shard,
    CallSignal,
    Site,
    Message,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShieldPacket {
    pub version: u16,
    pub class: PacketClass,
    pub session_id: [u8; 16],
    pub request_id: [u8; 16],
    pub sequence: u64,
    pub issued_at_unix: u64,
    pub ttl: u8,
    pub payload_hash: [u8; 32],
    pub payload: Vec<u8>,
    pub tag: [u8; 32],
}

impl ShieldPacket {
    pub fn seal(
        key: &[u8; 32],
        class: PacketClass,
        session_id: [u8; 16],
        request_id: [u8; 16],
        sequence: u64,
        issued_at_unix: u64,
        ttl: u8,
        payload: Vec<u8>,
    ) -> Result<Self, &'static str> {
        if ttl == 0 || ttl > MAX_TTL {
            return Err("invalid ttl");
        }
        if payload.len() > MAX_PAYLOAD {
            return Err("payload too large");
        }
        let payload_hash = *blake3::hash(&payload).as_bytes();
        let mut packet = Self {
            version: SHIELD_VERSION,
            class,
            session_id,
            request_id,
            sequence,
            issued_at_unix,
            ttl,
            payload_hash,
            payload,
            tag: [0; 32],
        };
        packet.tag = packet.compute_tag(key);
        Ok(packet)
    }

    fn authenticated_bytes(&self) -> Vec<u8> {
        let mut h = Hasher::new();
        h.update(b"AWE/SHIELD/v1");
        h.update(&self.version.to_be_bytes());
        h.update(&[self.class as u8]);
        h.update(&self.session_id);
        h.update(&self.request_id);
        h.update(&self.sequence.to_be_bytes());
        h.update(&self.issued_at_unix.to_be_bytes());
        h.update(&[self.ttl]);
        h.update(&self.payload_hash);
        h.finalize().as_bytes().to_vec()
    }

    fn compute_tag(&self, key: &[u8; 32]) -> [u8; 32] {
        let mut h = blake3::Hasher::new_keyed(key);
        h.update(&self.authenticated_bytes());
        h.finalize().into()
    }

    pub fn verify(&self, key: &[u8; 32], now_unix: u64, max_age_secs: u64) -> bool {
        if self.version != SHIELD_VERSION
            || self.ttl == 0
            || self.ttl > MAX_TTL
            || self.payload.len() > MAX_PAYLOAD
            || self.issued_at_unix > now_unix.saturating_add(30)
            || now_unix.saturating_sub(self.issued_at_unix) > max_age_secs
            || self.payload_hash != *blake3::hash(&self.payload).as_bytes()
        {
            return false;
        }
        let expected = self.compute_tag(key);
        crate::crypto::constant_time_eq(&self.tag, &expected)
    }

    /// Decrement the hop TTL and immediately re-authenticate the mutable hop envelope.
    /// The relay must possess the hop key; this prevents an intermediary from silently
    /// rewriting TTL without proving possession of the per-hop authorization key.
    pub fn decrement_ttl(&mut self, key: &[u8; 32]) -> bool {
        if self.ttl <= 1 {
            return false;
        }
        self.ttl -= 1;
        self.tag = self.compute_tag(key);
        true
    }
}

/// Replay defense for SHIELD requests. Sequence numbers are scoped to the
/// session/request tuple, so a packet captured on one request cannot be replayed
/// into another request.
pub const SHIELD_REPLAY_WINDOW: u8 = 64;
pub const SHIELD_MAX_TRACKED_REQUESTS: usize = 65_536;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ReplayState {
    highest: u64,
    bitmap: u64,
    generation: u64,
}

pub struct ShieldReplayGuard {
    states: BTreeMap<([u8; 16], [u8; 16]), ReplayState>,
    generation: u64,
}
impl Default for ShieldReplayGuard {
    fn default() -> Self {
        Self {
            states: BTreeMap::new(),
            generation: 0,
        }
    }
}

impl ShieldReplayGuard {
    pub fn accept(&mut self, session_id: [u8; 16], request_id: [u8; 16], sequence: u64) -> bool {
        let key = (session_id, request_id);

        if self.states.contains_key(&key) {
            let highest = self.states.get(&key).expect("replay state exists").highest;
            if sequence > highest {
                let shift = sequence - highest;
                self.generation = self.generation.wrapping_add(1);
                let generation = self.generation;
                let state = self.states.get_mut(&key).expect("replay state exists");
                state.bitmap = if shift >= SHIELD_REPLAY_WINDOW as u64 {
                    1
                } else {
                    (state.bitmap << shift) | 1
                };
                state.highest = sequence;
                state.generation = generation;
                return true;
            }

            let delta = highest - sequence;
            if delta >= SHIELD_REPLAY_WINDOW as u64 {
                return false;
            }

            let bit = 1u64 << delta;
            self.generation = self.generation.wrapping_add(1);
            let generation = self.generation;
            let state = self.states.get_mut(&key).expect("replay state exists");
            if state.bitmap & bit != 0 {
                return false;
            }
            state.bitmap |= bit;
            state.generation = generation;
            return true;
        }

        if self.states.len() >= SHIELD_MAX_TRACKED_REQUESTS {
            if let Some((oldest, _)) = self.states.iter().min_by_key(|(_, state)| state.generation) {
                let oldest = *oldest;
                self.states.remove(&oldest);
            }
        }

        self.generation = self.generation.wrapping_add(1);
        self.states.insert(
            key,
            ReplayState {
                highest: sequence,
                bitmap: 1,
                generation: self.generation,
            },
        );
        true
    }

    pub fn tracked_requests(&self) -> usize {
        self.states.len()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SecurityCounters {
    pub accepted: u64,
    pub rejected: u64,
    pub expired: u64,
    pub replayed: u64,
    pub bad_tag: u64,
}

/// Lock-free security telemetry suitable for hot packet paths.
/// Counters are intentionally monotonic and never gate packet processing.
#[derive(Debug, Default)]
pub struct SecurityMetrics {
    accepted: std::sync::atomic::AtomicU64,
    rejected: std::sync::atomic::AtomicU64,
    expired: std::sync::atomic::AtomicU64,
    replayed: std::sync::atomic::AtomicU64,
    bad_tag: std::sync::atomic::AtomicU64,
}

impl SecurityMetrics {
    pub fn record_accepted(&self) {
        self.accepted
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn record_rejected(&self) {
        self.rejected
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn record_expired(&self) {
        self.expired
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn record_replayed(&self) {
        self.replayed
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn record_bad_tag(&self) {
        self.bad_tag
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> SecurityCounters {
        SecurityCounters {
            accepted: self.accepted.load(std::sync::atomic::Ordering::Relaxed),
            rejected: self.rejected.load(std::sync::atomic::Ordering::Relaxed),
            expired: self.expired.load(std::sync::atomic::Ordering::Relaxed),
            replayed: self.replayed.load(std::sync::atomic::Ordering::Relaxed),
            bad_tag: self.bad_tag.load(std::sync::atomic::Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_metrics_snapshot_is_consistent() {
        let metrics = SecurityMetrics::default();
        metrics.record_accepted();
        metrics.record_accepted();
        metrics.record_rejected();
        metrics.record_expired();
        metrics.record_replayed();
        metrics.record_bad_tag();

        assert_eq!(
            metrics.snapshot(),
            SecurityCounters {
                accepted: 2,
                rejected: 1,
                expired: 1,
                replayed: 1,
                bad_tag: 1,
            }
        );
    }

    #[test]
    fn replay_window_allows_reordering_but_blocks_duplicates() {
        let mut g = ShieldReplayGuard::default();
        let s = [1u8; 16];
        let r = [2u8; 16];
        assert!(g.accept(s, r, 10));
        assert!(g.accept(s, r, 8));
        assert!(g.accept(s, r, 9));
        assert!(!g.accept(s, r, 8));
        assert!(!g.accept(s, r, 10));
        assert!(!g.accept(s, r, 10u64.saturating_sub(SHIELD_REPLAY_WINDOW as u64),));
    }

    #[test]
    fn replay_state_is_memory_bounded() {
        let mut g = ShieldReplayGuard::default();
        for i in 0..(SHIELD_MAX_TRACKED_REQUESTS + 32) {
            let mut s = [0u8; 16];
            s[..8].copy_from_slice(&(i as u64).to_be_bytes());
            assert!(g.accept(s, [3u8; 16], 1));
        }
        assert!(g.tracked_requests() <= SHIELD_MAX_TRACKED_REQUESTS);
    }

    #[test]
    fn shield_roundtrip_and_tamper_detection() {
        let key = [7u8; 32];
        let mut p = ShieldPacket::seal(
            &key,
            PacketClass::Shard,
            [1; 16],
            [2; 16],
            9,
            1_000,
            8,
            b"secret shard".to_vec(),
        ).unwrap();
        assert!(p.verify(&key, 1_010, 60));
        p.payload[0] ^= 1;
        assert!(!p.verify(&key, 1_010, 60));
    }

    #[test]
    fn ttl_is_bounded_and_cannot_reach_zero() {
        let key = [1u8; 32];
        let mut p = ShieldPacket::seal(
            &key,
            PacketClass::Control,
            [0; 16],
            [0; 16],
            1,
            10,
            2,
            vec![],
        )
        .unwrap();
        assert!(p.decrement_ttl(&key));
        assert!(p.verify(&key, 10, 60));
        assert!(!p.decrement_ttl(&key));
    }

    #[test]
    fn future_packets_have_clock_skew_guard() {
        let key = [2u8; 32];
        let p = ShieldPacket::seal(
            &key,
            PacketClass::Message,
            [0; 16],
            [0; 16],
            1,
            2_000,
            2,
            vec![],
        )
        .unwrap();
        assert!(!p.verify(&key, 1_900, 300));
    }
}
