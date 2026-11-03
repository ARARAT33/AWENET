//! Bounded authorization primitives for developer-facing APIs.
//!
//! The registry never stores bearer tokens. Callers hash a presented token at
//! the transport boundary and pass only the 32-byte digest here. Authorization
//! is explicit, scoped, tick-based, and bounded so an exposed API cannot grow
//! untracked key or request state indefinitely.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

pub const MAX_API_KEYS: usize = 4_096;
pub const DEFAULT_WINDOW_TICKS: u64 = 60;
pub const DEFAULT_REQUESTS_PER_WINDOW: u32 = 120;

#[derive(Clone, Copy, Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeveloperScope {
    Read,
    Write,
    Storage,
    Network,
    Diagnostics,
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum DeveloperApiError {
    #[error("API key registry is full")]
    RegistryFull,
    #[error("API key digest cannot be all zeroes")]
    InvalidDigest,
    #[error("API key is unknown or revoked")]
    UnknownKey,
    #[error("API key lacks the requested scope")]
    Forbidden,
    #[error("API key request window is exhausted")]
    RateLimited,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeveloperKeyInfo {
    pub label: String,
    pub scopes: BTreeSet<DeveloperScope>,
    pub enabled: bool,
    pub window_started: u64,
    pub requests_in_window: u32,
    pub total_requests: u64,
    pub denied_requests: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeveloperApiConfig {
    pub window_ticks: u64,
    pub requests_per_window: u32,
    pub max_keys: usize,
}

impl Default for DeveloperApiConfig {
    fn default() -> Self {
        Self {
            window_ticks: DEFAULT_WINDOW_TICKS,
            requests_per_window: DEFAULT_REQUESTS_PER_WINDOW,
            max_keys: MAX_API_KEYS,
        }
    }
}

impl DeveloperApiConfig {
    fn normalized(self) -> Self {
        Self {
            window_ticks: self.window_ticks.max(1),
            requests_per_window: self.requests_per_window.max(1),
            max_keys: self.max_keys.clamp(1, MAX_API_KEYS),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeveloperApiRegistry {
    config: DeveloperApiConfig,
    keys: BTreeMap<String, DeveloperKeyInfo>,
}

impl DeveloperApiRegistry {
    pub fn new(config: DeveloperApiConfig) -> Self {
        Self {
            config: config.normalized(),
            keys: BTreeMap::new(),
        }
    }

    pub fn config(&self) -> DeveloperApiConfig {
        self.config
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    fn key_id(digest: &[u8; 32]) -> String {
        hex::encode(digest)
    }

    pub fn register(
        &mut self,
        digest: [u8; 32],
        label: &str,
        scopes: BTreeSet<DeveloperScope>,
        now_tick: u64,
    ) -> Result<(), DeveloperApiError> {
        if digest == [0; 32] {
            return Err(DeveloperApiError::InvalidDigest);
        }
        let key_id = Self::key_id(&digest);
        if !self.keys.contains_key(&key_id) && self.keys.len() >= self.config.max_keys {
            return Err(DeveloperApiError::RegistryFull);
        }
        self.keys.insert(
            key_id,
            DeveloperKeyInfo {
                label: label.trim().to_string(),
                scopes,
                enabled: true,
                window_started: now_tick,
                requests_in_window: 0,
                total_requests: 0,
                denied_requests: 0,
            },
        );
        Ok(())
    }

    pub fn revoke(&mut self, digest: &[u8; 32]) -> Result<(), DeveloperApiError> {
        let key = self
            .keys
            .get_mut(&Self::key_id(digest))
            .ok_or(DeveloperApiError::UnknownKey)?;
        key.enabled = false;
        Ok(())
    }

    /// Authorizes one request and increments its audit counters atomically.
    pub fn authorize(
        &mut self,
        digest: &[u8; 32],
        scope: DeveloperScope,
        now_tick: u64,
    ) -> Result<(), DeveloperApiError> {
        let key = self
            .keys
            .get_mut(&Self::key_id(digest))
            .ok_or(DeveloperApiError::UnknownKey)?;
        if !key.enabled {
            key.denied_requests = key.denied_requests.saturating_add(1);
            return Err(DeveloperApiError::UnknownKey);
        }
        if !key.scopes.contains(&scope) {
            key.denied_requests = key.denied_requests.saturating_add(1);
            return Err(DeveloperApiError::Forbidden);
        }
        if now_tick.saturating_sub(key.window_started) >= self.config.window_ticks {
            key.window_started = now_tick;
            key.requests_in_window = 0;
        }
        if key.requests_in_window >= self.config.requests_per_window {
            key.denied_requests = key.denied_requests.saturating_add(1);
            return Err(DeveloperApiError::RateLimited);
        }
        key.requests_in_window += 1;
        key.total_requests = key.total_requests.saturating_add(1);
        Ok(())
    }

    pub fn inspect(&self, digest: &[u8; 32]) -> Result<&DeveloperKeyInfo, DeveloperApiError> {
        self.keys
            .get(&Self::key_id(digest))
            .ok_or(DeveloperApiError::UnknownKey)
    }
}

impl Default for DeveloperApiRegistry {
    fn default() -> Self {
        Self::new(DeveloperApiConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scopes() -> BTreeSet<DeveloperScope> {
        BTreeSet::from([DeveloperScope::Read, DeveloperScope::Diagnostics])
    }

    #[test]
    fn scopes_and_rate_limits_are_enforced() {
        let mut registry = DeveloperApiRegistry::new(DeveloperApiConfig {
            window_ticks: 10,
            requests_per_window: 2,
            ..Default::default()
        });
        let digest = [7; 32];
        registry.register(digest, "build-bot", scopes(), 0).unwrap();
        assert!(registry.authorize(&digest, DeveloperScope::Read, 1).is_ok());
        assert_eq!(
            registry.authorize(&digest, DeveloperScope::Write, 1),
            Err(DeveloperApiError::Forbidden)
        );
        assert!(registry.authorize(&digest, DeveloperScope::Read, 2).is_ok());
        assert_eq!(
            registry.authorize(&digest, DeveloperScope::Read, 3),
            Err(DeveloperApiError::RateLimited)
        );
        assert!(registry
            .authorize(&digest, DeveloperScope::Read, 10)
            .is_ok());
    }

    #[test]
    fn revoke_and_registry_bounds_are_fail_closed() {
        let mut registry = DeveloperApiRegistry::new(DeveloperApiConfig {
            max_keys: 1,
            ..Default::default()
        });
        registry.register([1; 32], "one", scopes(), 0).unwrap();
        assert_eq!(
            registry.register([2; 32], "two", scopes(), 0),
            Err(DeveloperApiError::RegistryFull)
        );
        registry.revoke(&[1; 32]).unwrap();
        assert_eq!(
            registry.authorize(&[1; 32], DeveloperScope::Read, 0),
            Err(DeveloperApiError::UnknownKey)
        );
        assert_eq!(
            registry.register([0; 32], "bad", scopes(), 0),
            Err(DeveloperApiError::InvalidDigest)
        );
    }

    #[test]
    fn persistence_round_trip_keeps_audit_state() {
        let mut registry = DeveloperApiRegistry::default();
        let digest = [9; 32];
        registry.register(digest, "ops", scopes(), 4).unwrap();
        registry
            .authorize(&digest, DeveloperScope::Read, 5)
            .unwrap();
        let bytes = serde_json::to_vec(&registry).unwrap();
        let restored: DeveloperApiRegistry = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(restored.inspect(&digest).unwrap().total_requests, 1);
    }
}
