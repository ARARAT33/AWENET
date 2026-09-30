//! AWEP2P Shield: protocol-level defense-in-depth for hostile networks.
//!
//! This is intentionally independent of the encrypted transport. It binds every
//! application packet to a protocol domain, session, sequence, expiry and payload
//! commitment. The keyed tag is a second integrity boundary; it is not a replacement
//! for authenticated encryption or identity signatures.

use blake3::Hasher;
use serde::{Deserialize, Serialize};

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

    pub fn decrement_ttl(&mut self) -> bool {
        if self.ttl <= 1 {
            return false;
        }
        self.ttl -= 1;
        true
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut p = ShieldPacket::seal(&key, PacketClass::Control, [0;16], [0;16], 1, 10, 2, vec![]).unwrap();
        assert!(p.decrement_ttl());
        assert!(!p.decrement_ttl());
    }

    #[test]
    fn future_packets_have_clock_skew_guard() {
        let key = [2u8; 32];
        let p = ShieldPacket::seal(&key, PacketClass::Message, [0;16], [0;16], 1, 2_000, 2, vec![]).unwrap();
        assert!(!p.verify(&key, 1_900, 300));
    }
}
