//! Scoped API key lifecycle: issue, verify, rotate and revoke.
//!
//! Deterministic and replay-friendly: pure data, no wall clock and no
//! threads. Time is an explicit tick supplied by the caller. Keys carry a
//! label, a scope set and an optional expiry tick; verification is a pure
//! lookup that reports unknown, revoked or expired keys as typed errors.

use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_KEYS: usize = 10_000;

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum KeyError {
    #[error("key registry capacity must be between 1 and {MAX_KEYS}")]
    InvalidCapacity,
    #[error("key registry is full (max {MAX_KEYS} keys)")]
    RegistryFull,
    #[error("unknown key {0}")]
    UnknownKey(String),
    #[error("key {0} is revoked")]
    Revoked(String),
    #[error("key {0} expired at tick {1}")]
    Expired(String, u64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiKey {
    pub key: String,
    pub label: String,
    pub scopes: Vec<String>,
    pub created_tick: u64,
    pub expires_tick: Option<u64>,
    pub revoked: bool,
    pub rotated_from: Option<String>,
}

#[derive(Debug)]
pub struct ApiKeys {
    max_keys: usize,
    next_id: u64,
    keys: BTreeMap<String, ApiKey>,
}

impl ApiKeys {
    pub fn new(max_keys: usize) -> Result<Self, KeyError> {
        if max_keys == 0 || max_keys > MAX_KEYS {
            return Err(KeyError::InvalidCapacity);
        }
        Ok(Self {
            max_keys,
            next_id: 1,
            keys: BTreeMap::new(),
        })
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn get(&self, key: &str) -> Option<&ApiKey> {
        self.keys.get(key)
    }

    /// Issues a new active key with the given label, scopes and optional
    /// expiry tick. Key material is derived deterministically from the
    /// internal counter, so issuance is replay-friendly.
    pub fn issue(
        &mut self,
        label: &str,
        scopes: Vec<String>,
        created_tick: u64,
        expires_tick: Option<u64>,
    ) -> Result<ApiKey, KeyError> {
        if self.keys.len() >= self.max_keys {
            return Err(KeyError::RegistryFull);
        }
        let id = self.next_id;
        self.next_id += 1;
        let key = format!("awenet-{:06x}-{:016x}", id, token(id));
        let record = ApiKey {
            key: key.clone(),
            label: label.to_string(),
            scopes,
            created_tick,
            expires_tick,
            revoked: false,
            rotated_from: None,
        };
        self.keys.insert(key.clone(), record.clone());
        Ok(record)
    }

    /// Verifies key material at `now`: unknown, revoked and expired keys
    /// are typed errors; a valid key is returned by reference.
    pub fn verify(&self, key: &str, now: u64) -> Result<&ApiKey, KeyError> {
        let record = self
            .keys
            .get(key)
            .ok_or_else(|| KeyError::UnknownKey(key.to_string()))?;
        if record.revoked {
            return Err(KeyError::Revoked(key.to_string()));
        }
        if let Some(expiry) = record.expires_tick {
            if expiry <= now {
                return Err(KeyError::Expired(key.to_string(), expiry));
            }
        }
        Ok(record)
    }

    /// Revokes a key. Revoking an already revoked key is a no-op.
    pub fn revoke(&mut self, key: &str) -> Result<(), KeyError> {
        let record = self
            .keys
            .get_mut(key)
            .ok_or_else(|| KeyError::UnknownKey(key.to_string()))?;
        record.revoked = true;
        Ok(())
    }

    /// Rotates a key: revokes the old material and issues fresh material
    /// with the same label, scopes and expiry tick. Returns the new key.
    pub fn rotate(&mut self, key: &str, now: u64) -> Result<ApiKey, KeyError> {
        let old = self.verify(key, now).cloned()?;
        self.revoke(key)?;
        let mut fresh = self.issue(&old.label, old.scopes.clone(), now, old.expires_tick)?;
        fresh.rotated_from = Some(key.to_string());
        self.keys.insert(fresh.key.clone(), fresh.clone());
        Ok(fresh)
    }

    /// Lists keys that verify as valid at `now`, ordered by key material.
    pub fn active(&self, now: u64) -> Vec<&ApiKey> {
        self.keys
            .values()
            .filter(|record| self.verify(&record.key, now).is_ok())
            .collect()
    }
}

fn token(id: u64) -> u64 {
    let mut z = id.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scopes() -> Vec<String> {
        vec!["storage".into(), "messenger".into()]
    }

    #[test]
    fn validates_capacity() {
        assert_eq!(ApiKeys::new(0).unwrap_err(), KeyError::InvalidCapacity);
        assert_eq!(
            ApiKeys::new(MAX_KEYS + 1).unwrap_err(),
            KeyError::InvalidCapacity
        );
        assert!(ApiKeys::new(1).is_ok());
    }

    #[test]
    fn issue_and_verify() {
        let mut registry = ApiKeys::new(4).unwrap();
        let issued = registry.issue("node-cli", scopes(), 0, None).unwrap();
        let verified = registry.verify(&issued.key, 10).unwrap();
        assert_eq!(verified.label, "node-cli");
        assert_eq!(verified.scopes, scopes());
        assert!(!verified.revoked);
    }

    #[test]
    fn unknown_key_is_typed_error() {
        let registry = ApiKeys::new(4).unwrap();
        assert_eq!(
            registry
                .verify("awenet-000000-0000000000000000", 0)
                .unwrap_err(),
            KeyError::UnknownKey("awenet-000000-0000000000000000".into())
        );
    }

    #[test]
    fn expiry_and_revocation() {
        let mut registry = ApiKeys::new(4).unwrap();
        let expiring = registry.issue("temp", scopes(), 0, Some(10)).unwrap();
        assert_eq!(
            registry.verify(&expiring.key, 10).unwrap_err(),
            KeyError::Expired(expiring.key.clone(), 10)
        );
        assert!(registry.verify(&expiring.key, 9).is_ok());

        let permanent = registry.issue("service", scopes(), 0, None).unwrap();
        registry.revoke(&permanent.key).unwrap();
        assert_eq!(
            registry.verify(&permanent.key, 1).unwrap_err(),
            KeyError::Revoked(permanent.key.clone())
        );
    }

    #[test]
    fn rotation_keeps_scopes_and_revolves_old() {
        let mut registry = ApiKeys::new(4).unwrap();
        let old = registry.issue("ci-bot", scopes(), 0, Some(100)).unwrap();
        let fresh = registry.rotate(&old.key, 20).unwrap();
        assert_eq!(fresh.label, "ci-bot");
        assert_eq!(fresh.scopes, old.scopes);
        assert_eq!(fresh.expires_tick, Some(100));
        assert_eq!(fresh.rotated_from.as_deref(), Some(old.key.as_str()));
        assert_eq!(
            registry.verify(&old.key, 21).unwrap_err(),
            KeyError::Revoked(old.key.clone())
        );
        assert!(registry.verify(&fresh.key, 21).is_ok());
    }

    #[test]
    fn registry_full_and_active_list() {
        let mut registry = ApiKeys::new(1).unwrap();
        registry.issue("only", scopes(), 0, None).unwrap();
        assert_eq!(
            registry.issue("extra", scopes(), 0, None).unwrap_err(),
            KeyError::RegistryFull
        );
        assert_eq!(registry.active(0).len(), 1);
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
    }
}
