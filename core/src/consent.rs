//! Bounded consent ledger for data-sharing scopes.
//!
//! Every access to shared data is checked against an explicit grant. Grants
//! are per (subject, scope), can carry an expiry tick, and can be revoked
//! at any time. All operations run on an explicit tick counter supplied by
//! the caller, so decisions replay deterministically. A bounded audit log
//! records grants and revocations in order.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use thiserror::Error;

pub const MAX_GRANTS: usize = 1024;
pub const MAX_LOG_ENTRIES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ConsentError {
    #[error("consent ledger is full (max {MAX_GRANTS})")]
    LedgerFull,
    #[error("empty subject or scope is not allowed")]
    EmptyKey,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub subject: String,
    pub scope: String,
    pub granted_tick: u64,
    pub expires_tick: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConsentAction {
    Grant,
    Revoke,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub action: ConsentAction,
    pub subject: String,
    pub scope: String,
    pub tick: u64,
}

/// Bounded consent ledger with an ordered audit log.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConsentLedger {
    grants: BTreeMap<(String, String), Grant>,
    log: VecDeque<LogEntry>,
}

impl ConsentLedger {
    pub fn new() -> Self {
        ConsentLedger {
            grants: BTreeMap::new(),
            log: VecDeque::new(),
        }
    }

    /// Grants a subject consent for a scope, optionally expiring at a tick.
    /// Re-granting an existing scope replaces the previous grant and is
    /// logged again.
    pub fn grant(
        &mut self,
        subject: &str,
        scope: &str,
        now_tick: u64,
        expires_tick: Option<u64>,
    ) -> Result<(), ConsentError> {
        if subject.is_empty() || scope.is_empty() {
            return Err(ConsentError::EmptyKey);
        }
        if !self.grants.contains_key(&(subject.to_string(), scope.to_string()))
            && self.grants.len() >= MAX_GRANTS
        {
            return Err(ConsentError::LedgerFull);
        }
        self.grants.insert(
            (subject.to_string(), scope.to_string()),
            Grant {
                subject: subject.to_string(),
                scope: scope.to_string(),
                granted_tick: now_tick,
                expires_tick,
            },
        );
        self.push_log(ConsentAction::Grant, subject, scope, now_tick);
        Ok(())
    }

    /// Revokes consent for a subject and scope. Revoking an unknown grant
    /// is a no-op that is still logged, so the audit trail stays truthful.
    pub fn revoke(&mut self, subject: &str, scope: &str, now_tick: u64) -> Result<(), ConsentError> {
        if subject.is_empty() || scope.is_empty() {
            return Err(ConsentError::EmptyKey);
        }
        self.grants.remove(&(subject.to_string(), scope.to_string()));
        self.push_log(ConsentAction::Revoke, subject, scope, now_tick);
        Ok(())
    }

    /// True when the subject has a non-expired grant for the scope.
    pub fn is_allowed(&self, subject: &str, scope: &str, now_tick: u64) -> bool {
        match self.grants.get(&(subject.to_string(), scope.to_string())) {
            Some(g) => match g.expires_tick {
                Some(expiry) => now_tick < expiry,
                None => true,
            },
            None => false,
        }
    }

    pub fn get(&self, subject: &str, scope: &str) -> Option<&Grant> {
        self.grants.get(&(subject.to_string(), scope.to_string()))
    }

    /// Removes every grant that has expired by `now_tick`.
    pub fn sweep(&mut self, now_tick: u64) -> usize {
        let expired: Vec<(String, String)> = self
            .grants
            .iter()
            .filter(|(_, g)| g.expires_tick.is_some_and(|e| now_tick >= e))
            .map(|(k, _)| k.clone())
            .collect();
        for key in &expired {
            self.grants.remove(key);
        }
        expired.len()
    }

    pub fn len(&self) -> usize {
        self.grants.len()
    }

    pub fn is_empty(&self) -> bool {
        self.grants.is_empty()
    }

    pub fn log_len(&self) -> usize {
        self.log.len()
    }

    pub fn log_entries(&self) -> Vec<LogEntry> {
        self.log.iter().cloned().collect()
    }

    /// Scopes a subject currently allows, sorted for stable output.
    pub fn scopes_for(&self, subject: &str) -> Vec<String> {
        self.grants
            .keys()
            .filter(|(s, _)| s == subject)
            .map(|(_, scope)| scope.clone())
            .collect()
    }

    fn push_log(&mut self, action: ConsentAction, subject: &str, scope: &str, tick: u64) {
        self.log.push_back(LogEntry {
            action,
            subject: subject.to_string(),
            scope: scope.to_string(),
            tick,
        });
        while self.log.len() > MAX_LOG_ENTRIES {
            self.log.pop_front();
        }
    }
}

impl Default for ConsentLedger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grants_and_checks() {
        let mut ledger = ConsentLedger::new();
        ledger.grant("ararat", "profile.public", 10, None).unwrap();
        assert!(ledger.is_allowed("ararat", "profile.public", 100));
        assert!(!ledger.is_allowed("ararat", "profile.private", 100));
        assert!(!ledger.is_allowed("guest", "profile.public", 100));
        assert_eq!(ledger.scopes_for("ararat"), vec!["profile.public".to_string()]);
    }

    #[test]
    fn expiry_is_respected() {
        let mut ledger = ConsentLedger::new();
        ledger.grant("ararat", "data.export", 0, Some(100)).unwrap();
        assert!(ledger.is_allowed("ararat", "data.export", 99));
        assert!(!ledger.is_allowed("ararat", "data.export", 100));
        assert_eq!(ledger.sweep(100), 1);
        assert!(ledger.is_empty());
    }

    #[test]
    fn revoke_blocks_access_and_is_logged() {
        let mut ledger = ConsentLedger::new();
        ledger.grant("ararat", "messenger.read", 0, None).unwrap();
        ledger.revoke("ararat", "messenger.read", 5).unwrap();
        assert!(!ledger.is_allowed("ararat", "messenger.read", 6));
        let entries = ledger.log_entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].action, ConsentAction::Grant);
        assert_eq!(entries[1].action, ConsentAction::Revoke);
        assert_eq!(entries[1].tick, 5);
    }

    #[test]
    fn regrant_replaces_previous() {
        let mut ledger = ConsentLedger::new();
        ledger.grant("ararat", "s", 0, Some(10)).unwrap();
        ledger.grant("ararat", "s", 20, None).unwrap();
        assert!(ledger.is_allowed("ararat", "s", 100));
        assert_eq!(ledger.len(), 1);
        assert_eq!(ledger.log_len(), 2);
    }

    #[test]
    fn validates_keys() {
        let mut ledger = ConsentLedger::new();
        assert_eq!(
            ledger.grant("", "s", 0, None).unwrap_err(),
            ConsentError::EmptyKey
        );
        assert_eq!(
            ledger.grant("a", "", 0, None).unwrap_err(),
            ConsentError::EmptyKey
        );
    }

    #[test]
    fn ledger_is_bounded() {
        let mut ledger = ConsentLedger::new();
        for i in 0..MAX_GRANTS {
            ledger.grant(&format!("s{i}"), "scope", 0, None).unwrap();
        }
        assert!(matches!(
            ledger.grant("new", "scope", 0, None),
            Err(ConsentError::LedgerFull)
        ));
    }

    #[test]
    fn serializes_to_json() {
        let mut ledger = ConsentLedger::new();
        ledger.grant("ararat", "profile.public", 1, Some(50)).unwrap();
        let json = serde_json::to_string(&ledger).unwrap();
        let back: ConsentLedger = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 1);
        assert!(back.is_allowed("ararat", "profile.public", 10));
        assert_eq!(back.log_len(), 1);
    }
}
