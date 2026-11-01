//! Bounded authenticated session manager.
//!
//! Deterministic and replay-friendly: sessions live and expire on an
//! explicit tick counter supplied by the caller, never a wall clock. The
//! registry is hard-bounded so sessions can never grow memory without
//! limit, and every session can be revoked explicitly.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_SESSIONS: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum SessionError {
    #[error("session registry is full (max {MAX_SESSIONS})")]
    RegistryFull,
    #[error("unknown session {0}")]
    UnknownSession(u64),
    #[error("session {0} is expired")]
    Expired(u64),
    #[error("session {0} is revoked")]
    Revoked(u64),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: u64,
    pub principal: String,
    pub created_tick: u64,
    pub expires_tick: u64,
    pub revoked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    Active,
    Expired,
    Revoked,
    Unknown,
}

/// Bounded session registry with monotonically increasing ids.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionManager {
    next_id: u64,
    sessions: BTreeMap<u64, Session>,
}

impl SessionManager {
    pub fn new() -> Self {
        SessionManager {
            next_id: 1,
            sessions: BTreeMap::new(),
        }
    }

    /// Creates a session for a principal that stays valid until
    /// `expires_tick`. Returns the new session.
    pub fn create(
        &mut self,
        principal: &str,
        now_tick: u64,
        ttl_ticks: u64,
    ) -> Result<Session, SessionError> {
        if self.sessions.len() >= MAX_SESSIONS {
            self.sweep(now_tick);
            if self.sessions.len() >= MAX_SESSIONS {
                return Err(SessionError::RegistryFull);
            }
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let session = Session {
            id,
            principal: principal.to_string(),
            created_tick: now_tick,
            expires_tick: now_tick + ttl_ticks,
            revoked: false,
        };
        self.sessions.insert(id, session.clone());
        Ok(session)
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    pub fn get(&self, id: u64) -> Option<&Session> {
        self.sessions.get(&id)
    }

    pub fn state(&self, id: u64, now_tick: u64) -> SessionState {
        match self.sessions.get(&id) {
            None => SessionState::Unknown,
            Some(s) if s.revoked => SessionState::Revoked,
            Some(s) if now_tick >= s.expires_tick => SessionState::Expired,
            Some(_) => SessionState::Active,
        }
    }

    /// Validates a session at `now_tick` and returns its principal.
    pub fn validate(&self, id: u64, now_tick: u64) -> Result<&str, SessionError> {
        match self.state(id, now_tick) {
            SessionState::Active => Ok(self.sessions.get(&id).unwrap().principal.as_str()),
            SessionState::Expired => Err(SessionError::Expired(id)),
            SessionState::Revoked => Err(SessionError::Revoked(id)),
            SessionState::Unknown => Err(SessionError::UnknownSession(id)),
        }
    }

    /// Revokes a session immediately, even if it has not expired yet.
    pub fn revoke(&mut self, id: u64) -> Result<(), SessionError> {
        let session = self
            .sessions
            .get_mut(&id)
            .ok_or(SessionError::UnknownSession(id))?;
        session.revoked = true;
        Ok(())
    }

    /// Removes every session that is expired at `now_tick`. Revoked
    /// sessions are kept until they expire so revocation stays auditable.
    pub fn sweep(&mut self, now_tick: u64) -> usize {
        let expired: Vec<u64> = self
            .sessions
            .iter()
            .filter(|(_, s)| now_tick >= s.expires_tick)
            .map(|(id, _)| *id)
            .collect();
        for id in &expired {
            self.sessions.remove(id);
        }
        expired.len()
    }

    pub fn active_count(&self, now_tick: u64) -> usize {
        self.sessions
            .values()
            .filter(|s| !s.revoked && now_tick < s.expires_tick)
            .count()
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_and_validates_sessions() {
        let mut manager = SessionManager::new();
        let session = manager.create("ararat", 10, 100).unwrap();
        assert_eq!(session.id, 1);
        assert_eq!(manager.validate(1, 50).unwrap(), "ararat");
        assert_eq!(manager.state(1, 50), SessionState::Active);
    }

    #[test]
    fn expires_on_tick_boundary() {
        let mut manager = SessionManager::new();
        manager.create("ararat", 0, 10).unwrap();
        assert_eq!(manager.state(1, 9), SessionState::Active);
        assert_eq!(manager.state(1, 10), SessionState::Expired);
        assert!(matches!(
            manager.validate(1, 10),
            Err(SessionError::Expired(1))
        ));
    }

    #[test]
    fn revocation_beats_expiry_window() {
        let mut manager = SessionManager::new();
        manager.create("ararat", 0, 100).unwrap();
        manager.revoke(1).unwrap();
        assert_eq!(manager.state(1, 1), SessionState::Revoked);
        assert!(matches!(
            manager.validate(1, 1),
            Err(SessionError::Revoked(1))
        ));
    }

    #[test]
    fn unknown_sessions_are_typed_errors() {
        let manager = SessionManager::new();
        assert_eq!(manager.state(999, 0), SessionState::Unknown);
        assert!(matches!(
            manager.validate(999, 0),
            Err(SessionError::UnknownSession(999))
        ));
    }

    #[test]
    fn sweep_removes_expired_only() {
        let mut manager = SessionManager::new();
        manager.create("a", 0, 10).unwrap();
        manager.create("b", 0, 50).unwrap();
        manager.revoke(2).unwrap();
        assert_eq!(manager.sweep(10), 1);
        assert_eq!(manager.len(), 1);
        assert_eq!(manager.active_count(10), 0);
        assert_eq!(manager.sweep(50), 1);
        assert!(manager.is_empty());
    }

    #[test]
    fn registry_is_bounded() {
        let mut manager = SessionManager::new();
        for i in 0..MAX_SESSIONS {
            manager.create("p", 0, 1000 + i as u64).unwrap();
        }
        assert_eq!(manager.len(), MAX_SESSIONS);
        // All sessions still valid: creation must fail cleanly.
        assert!(matches!(
            manager.create("p", 0, 1000),
            Err(SessionError::RegistryFull)
        ));
        // Expiring one session frees a slot for the sweep path.
        manager.sweep(1000);
        assert!(manager.create("p", 1000, 10).is_ok());
    }

    #[test]
    fn serializes_to_json() {
        let mut manager = SessionManager::new();
        manager.create("ararat", 0, 100).unwrap();
        let json = serde_json::to_string(&manager).unwrap();
        let back: SessionManager = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back.validate(1, 10).unwrap(), "ararat");
    }
}
