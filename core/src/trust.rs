//! Deterministic peer trust lifecycle with explicit evidence and operator controls.
//!
//! Trust is an admission signal, not an authority grant. A peer can become
//! trusted through repeated successful observations, lose trust after failures,
//! and enter quarantine after abuse. Ban and reinstatement are explicit state
//! transitions so automated scoring cannot silently override operator action.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_PEERS: usize = 4096;
pub const TRUST_THRESHOLD: u16 = 700;
pub const PROBATION_THRESHOLD: u16 = 300;
pub const QUARANTINE_THRESHOLD: u16 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustState {
    Unknown,
    Probation,
    Trusted,
    Quarantined,
    Banned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustEventKind {
    Success,
    Failure,
    Abuse,
    Ban,
    Reinstate,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustRecord {
    pub peer_id: [u8; 32],
    pub state: TrustState,
    pub score: u16,
    pub successes: u64,
    pub failures: u64,
    pub abuse_events: u32,
    pub last_tick: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustDecision {
    pub peer_id: [u8; 32],
    pub state: TrustState,
    pub score: u16,
    pub event: TrustEventKind,
    pub tick: u64,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum TrustError {
    #[error("trust store is full")]
    TooManyPeers,
    #[error("peer is already banned")]
    Banned,
    #[error("peer is not eligible for reinstatement")]
    NotEligibleForReinstatement,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TrustLifecycle {
    peers: BTreeMap<String, TrustRecord>,
}

impl TrustLifecycle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    pub fn get(&self, peer_id: &[u8; 32]) -> Option<&TrustRecord> {
        self.peers.get(&Self::key(peer_id))
    }

    pub fn observe_success(
        &mut self,
        peer_id: [u8; 32],
        tick: u64,
    ) -> Result<TrustDecision, TrustError> {
        self.observe(peer_id, tick, TrustEventKind::Success)
    }

    pub fn observe_failure(
        &mut self,
        peer_id: [u8; 32],
        tick: u64,
    ) -> Result<TrustDecision, TrustError> {
        self.observe(peer_id, tick, TrustEventKind::Failure)
    }

    pub fn observe_abuse(
        &mut self,
        peer_id: [u8; 32],
        tick: u64,
    ) -> Result<TrustDecision, TrustError> {
        self.observe(peer_id, tick, TrustEventKind::Abuse)
    }

    pub fn ban(&mut self, peer_id: [u8; 32], tick: u64) -> Result<TrustDecision, TrustError> {
        let record = self.record_mut(peer_id)?;
        if record.state == TrustState::Banned {
            return Err(TrustError::Banned);
        }
        record.state = TrustState::Banned;
        record.last_tick = tick;
        Ok(Self::decision(record, TrustEventKind::Ban, tick))
    }

    /// Reinstatement is deliberately explicit and requires a healthy score and
    /// no abuse evidence. Automated observations never perform this transition.
    pub fn reinstate(&mut self, peer_id: [u8; 32], tick: u64) -> Result<TrustDecision, TrustError> {
        let record = self.record_mut(peer_id)?;
        if record.state != TrustState::Quarantined || record.score < TRUST_THRESHOLD {
            return Err(TrustError::NotEligibleForReinstatement);
        }
        record.abuse_events = 0;
        record.state = TrustState::Trusted;
        record.last_tick = tick;
        Ok(Self::decision(record, TrustEventKind::Reinstate, tick))
    }

    fn observe(
        &mut self,
        peer_id: [u8; 32],
        tick: u64,
        event: TrustEventKind,
    ) -> Result<TrustDecision, TrustError> {
        let record = self.record_mut(peer_id)?;
        if record.state == TrustState::Banned {
            return Err(TrustError::Banned);
        }
        match event {
            TrustEventKind::Success => {
                record.successes = record.successes.saturating_add(1);
                record.score = record.score.saturating_add(10).min(1000);
            }
            TrustEventKind::Failure => {
                record.failures = record.failures.saturating_add(1);
                record.score = record.score.saturating_sub(50);
            }
            TrustEventKind::Abuse => {
                record.abuse_events = record.abuse_events.saturating_add(1);
                record.score = record.score.saturating_sub(200);
            }
            TrustEventKind::Ban | TrustEventKind::Reinstate => unreachable!(),
        }
        record.state = next_state(record.state, record.score, record.abuse_events, event);
        record.last_tick = tick;
        Ok(Self::decision(record, event, tick))
    }

    fn record_mut(&mut self, peer_id: [u8; 32]) -> Result<&mut TrustRecord, TrustError> {
        let key = Self::key(&peer_id);
        if !self.peers.contains_key(&key) {
            if self.peers.len() >= MAX_PEERS {
                return Err(TrustError::TooManyPeers);
            }
            self.peers.insert(
                key.clone(),
                TrustRecord {
                    peer_id,
                    state: TrustState::Unknown,
                    score: 500,
                    successes: 0,
                    failures: 0,
                    abuse_events: 0,
                    last_tick: 0,
                },
            );
        }
        Ok(self.peers.get_mut(&key).expect("record was inserted"))
    }

    fn key(peer_id: &[u8; 32]) -> String {
        hex::encode(peer_id)
    }

    fn decision(record: &TrustRecord, event: TrustEventKind, tick: u64) -> TrustDecision {
        TrustDecision {
            peer_id: record.peer_id,
            state: record.state,
            score: record.score,
            event,
            tick,
        }
    }
}

fn next_state(
    current: TrustState,
    score: u16,
    abuse_events: u32,
    event: TrustEventKind,
) -> TrustState {
    if current == TrustState::Quarantined || event == TrustEventKind::Abuse && abuse_events > 0 {
        return TrustState::Quarantined;
    }
    if score <= QUARANTINE_THRESHOLD {
        TrustState::Quarantined
    } else if score >= TRUST_THRESHOLD {
        TrustState::Trusted
    } else if score < PROBATION_THRESHOLD {
        TrustState::Probation
    } else {
        TrustState::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_evidence_promotes_peer() {
        let id = [1; 32];
        let mut trust = TrustLifecycle::new();
        for tick in 0..20 {
            trust.observe_success(id, tick).unwrap();
        }
        let record = trust.get(&id).unwrap();
        assert_eq!(record.state, TrustState::Trusted);
        assert_eq!(record.score, 700);
    }

    #[test]
    fn failures_demote_and_abuse_quarantines() {
        let id = [2; 32];
        let mut trust = TrustLifecycle::new();
        for tick in 0..5 {
            trust.observe_failure(id, tick).unwrap();
        }
        assert_eq!(trust.get(&id).unwrap().state, TrustState::Probation);
        let decision = trust.observe_abuse(id, 10).unwrap();
        assert_eq!(decision.state, TrustState::Quarantined);
    }

    #[test]
    fn ban_is_sticky_until_explicit_eligible_reinstatement() {
        let id = [3; 32];
        let mut trust = TrustLifecycle::new();
        trust.ban(id, 1).unwrap();
        assert_eq!(trust.observe_success(id, 2), Err(TrustError::Banned));
        assert_eq!(
            trust.reinstate(id, 3),
            Err(TrustError::NotEligibleForReinstatement)
        );
    }

    #[test]
    fn operator_can_reinstate_a_rehabilitated_quarantine() {
        let id = [5; 32];
        let mut trust = TrustLifecycle::new();
        trust.observe_abuse(id, 1).unwrap();
        for tick in 2..42 {
            trust.observe_success(id, tick).unwrap();
        }
        let decision = trust.reinstate(id, 42).unwrap();
        assert_eq!(decision.state, TrustState::Trusted);
        assert_eq!(trust.get(&id).unwrap().abuse_events, 0);
    }

    #[test]
    fn serialization_round_trip_preserves_state() {
        let id = [4; 32];
        let mut trust = TrustLifecycle::new();
        trust.observe_success(id, 7).unwrap();
        let json = serde_json::to_vec(&trust).unwrap();
        let restored: TrustLifecycle = serde_json::from_slice(&json).unwrap();
        assert_eq!(restored.get(&id), trust.get(&id));
    }
}
