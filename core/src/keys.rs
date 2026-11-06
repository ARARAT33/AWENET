//! Scoped API key lifecycle: issue, verify, rotate and revoke.
//!
//! Key lifecycle decisions use explicit ticks for deterministic tests, but
//! bearer secrets are deliberately non-deterministic: each key is generated
//! from the operating system CSPRNG. Only a domain-separated SHA-256
//! fingerprint is retained in the registry; the secret is returned once
//! to the caller and is never included in Debug output or error messages.

use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt};
use thiserror::Error;
use zeroize::Zeroize;

pub const MAX_KEYS: usize = 10_000;
const KEY_PREFIX: &str = "awenet_";
const KEY_BYTES: usize = 32;
const MAX_GENERATION_ATTEMPTS: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum KeyError {
    #[error("key registry capacity must be between 1 and {MAX_KEYS}")]
    InvalidCapacity,
    #[error("key registry is full (max {MAX_KEYS} keys)")]
    RegistryFull,
    #[error("API key label must contain 1 to 128 non-whitespace bytes")]
    InvalidLabel,
    #[error("API key must contain between 1 and 64 non-empty scopes")]
    InvalidScopes,
    #[error("API key expiry must be later than its creation tick")]
    InvalidExpiry,
    #[error("operating-system secure random generation failed")]
    EntropyUnavailable,
    #[error("could not allocate a unique API key after bounded retries")]
    KeyCollision,
    #[error("API key format is invalid")]
    InvalidFormat,
    #[error("unknown API key")]
    UnknownKey,
    #[error("API key {0} is revoked")]
    Revoked(String),
    #[error("API key {0} expired at tick {1}")]
    Expired(String, u64),
}

/// Public metadata for a key. The bearer secret is intentionally absent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiKey {
    /// Full SHA-256 fingerprint of the bearer secret, safe to use as an id.
    pub id: String,
    pub label: String,
    pub scopes: Vec<String>,
    pub created_tick: u64,
    pub expires_tick: Option<u64>,
    pub revoked: bool,
    /// Fingerprint of the key replaced during rotation, never its secret.
    pub rotated_from: Option<String>,
}

/// The bearer secret is returned only by issue/rotate. Do not persist it in
/// logs or telemetry; the Debug implementation redacts it.
#[derive(Clone, PartialEq, Eq)]
pub struct IssuedApiKey {
    pub secret: String,
    pub record: ApiKey,
}

impl fmt::Debug for IssuedApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IssuedApiKey")
            .field("secret", &"[REDACTED]")
            .field("record", &self.record)
            .finish()
    }
}

impl Drop for IssuedApiKey {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

#[derive(Debug)]
pub struct ApiKeys {
    max_keys: usize,
    /// Indexed by a one-way fingerprint; raw bearer secrets are not retained.
    keys: BTreeMap<String, ApiKey>,
}

impl ApiKeys {
    pub fn new(max_keys: usize) -> Result<Self, KeyError> {
        if max_keys == 0 || max_keys > MAX_KEYS {
            return Err(KeyError::InvalidCapacity);
        }
        Ok(Self {
            max_keys,
            keys: BTreeMap::new(),
        })
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Looks up a key by its presented bearer secret without retaining it.
    pub fn get(&self, secret: &str) -> Option<&ApiKey> {
        let id = fingerprint(secret).ok()?;
        self.keys.get(&id)
    }

    /// Issues a 256-bit OS-CSPRNG bearer secret. The returned secret must be
    /// shown to the caller once; only its fingerprint is retained here.
    pub fn issue(
        &mut self,
        label: &str,
        scopes: Vec<String>,
        created_tick: u64,
        expires_tick: Option<u64>,
    ) -> Result<IssuedApiKey, KeyError> {
        let (label, scopes) = validate_metadata(label, scopes, created_tick, expires_tick)?;
        self.issue_record(label, scopes, created_tick, expires_tick, None)
    }

    /// Verifies a presented bearer secret at now. Errors never echo the
    /// supplied secret, preventing accidental credential disclosure in logs.
    pub fn verify(&self, secret: &str, now: u64) -> Result<&ApiKey, KeyError> {
        let id = fingerprint(secret)?;
        let record = self.keys.get(&id).ok_or(KeyError::UnknownKey)?;
        if record.revoked {
            return Err(KeyError::Revoked(record.id.clone()));
        }
        if let Some(expiry) = record.expires_tick {
            if expiry <= now {
                return Err(KeyError::Expired(record.id.clone(), expiry));
            }
        }
        Ok(record)
    }

    /// Revokes a key by its presented bearer secret. Re-revocation is a no-op.
    pub fn revoke(&mut self, secret: &str) -> Result<(), KeyError> {
        let id = fingerprint(secret)?;
        let record = self.keys.get_mut(&id).ok_or(KeyError::UnknownKey)?;
        record.revoked = true;
        Ok(())
    }

    /// Creates the replacement before revoking the old key. If capacity or
    /// entropy prevents issuance, the old key remains valid.
    pub fn rotate(&mut self, secret: &str, now: u64) -> Result<IssuedApiKey, KeyError> {
        let old = self.verify(secret, now)?.clone();
        let fresh = self.issue_record(
            old.label.clone(),
            old.scopes.clone(),
            now,
            old.expires_tick,
            Some(old.id.clone()),
        )?;
        if let Some(previous) = self.keys.get_mut(&old.id) {
            previous.revoked = true;
        }
        Ok(fresh)
    }

    /// Lists keys that are active at now, ordered by fingerprint.
    pub fn active(&self, now: u64) -> Vec<&ApiKey> {
        self.keys
            .values()
            .filter(|record| {
                !record.revoked && record.expires_tick.is_none_or(|expiry| now < expiry)
            })
            .collect()
    }

    fn issue_record(
        &mut self,
        label: String,
        scopes: Vec<String>,
        created_tick: u64,
        expires_tick: Option<u64>,
        rotated_from: Option<String>,
    ) -> Result<IssuedApiKey, KeyError> {
        if self.keys.len() >= self.max_keys {
            return Err(KeyError::RegistryFull);
        }

        for _ in 0..MAX_GENERATION_ATTEMPTS {
            let mut secret = generate_secret()?;
            let id = fingerprint(&secret)?;
            if self.keys.contains_key(&id) {
                secret.zeroize();
                continue;
            }

            let record = ApiKey {
                id: id.clone(),
                label: label.clone(),
                scopes: scopes.clone(),
                created_tick,
                expires_tick,
                revoked: false,
                rotated_from: rotated_from.clone(),
            };
            self.keys.insert(id, record.clone());
            return Ok(IssuedApiKey { secret, record });
        }

        Err(KeyError::KeyCollision)
    }
}

fn validate_metadata(
    label: &str,
    scopes: Vec<String>,
    created_tick: u64,
    expires_tick: Option<u64>,
) -> Result<(String, Vec<String>), KeyError> {
    let label = label.trim();
    if label.is_empty() || label.len() > 128 {
        return Err(KeyError::InvalidLabel);
    }
    if scopes.is_empty() || scopes.len() > 64 {
        return Err(KeyError::InvalidScopes);
    }

    let mut scopes = scopes
        .into_iter()
        .map(|scope| scope.trim().to_string())
        .collect::<Vec<_>>();
    if scopes
        .iter()
        .any(|scope| scope.is_empty() || scope.len() > 128)
    {
        return Err(KeyError::InvalidScopes);
    }
    scopes.sort();
    scopes.dedup();

    if matches!(expires_tick, Some(expiry) if expiry <= created_tick) {
        return Err(KeyError::InvalidExpiry);
    }

    Ok((label.to_string(), scopes))
}

fn generate_secret() -> Result<String, KeyError> {
    let mut bytes = [0u8; KEY_BYTES];
    let mut rng = OsRng;
    if rng.try_fill_bytes(&mut bytes).is_err() {
        bytes.zeroize();
        return Err(KeyError::EntropyUnavailable);
    }
    let encoded = hex::encode(bytes);
    bytes.zeroize();
    Ok(format!("{KEY_PREFIX}{encoded}"))
}

fn fingerprint(secret: &str) -> Result<String, KeyError> {
    let material = secret
        .strip_prefix(KEY_PREFIX)
        .ok_or(KeyError::InvalidFormat)?;
    if material.len() != KEY_BYTES * 2 || !material.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(KeyError::InvalidFormat);
    }

    let mut hasher = Sha256::new();
    hasher.update(b"AWENET/API-KEY/v1\0");
    hasher.update(secret.as_bytes());
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scopes() -> Vec<String> {
        vec!["storage".into(), "messenger".into()]
    }

    fn valid_unknown_secret() -> String {
        format!("{KEY_PREFIX}{}", "00".repeat(KEY_BYTES))
    }

    #[test]
    fn validates_capacity_and_metadata() {
        assert_eq!(ApiKeys::new(0).unwrap_err(), KeyError::InvalidCapacity);
        assert_eq!(
            ApiKeys::new(MAX_KEYS + 1).unwrap_err(),
            KeyError::InvalidCapacity
        );
        assert_eq!(
            ApiKeys::new(1)
                .unwrap()
                .issue(" ", scopes(), 0, None)
                .unwrap_err(),
            KeyError::InvalidLabel
        );
        assert_eq!(
            ApiKeys::new(1)
                .unwrap()
                .issue("service", vec![" ".into()], 0, None)
                .unwrap_err(),
            KeyError::InvalidScopes
        );
        assert_eq!(
            ApiKeys::new(1)
                .unwrap()
                .issue("service", scopes(), 10, Some(10))
                .unwrap_err(),
            KeyError::InvalidExpiry
        );
        assert!(ApiKeys::new(1).is_ok());
    }

    #[test]
    fn issues_random_secret_and_stores_only_fingerprint() {
        let mut registry = ApiKeys::new(4).unwrap();
        let first = registry.issue("node-cli", scopes(), 0, None).unwrap();
        let second = registry.issue("node-cli", scopes(), 0, None).unwrap();

        assert_ne!(first.secret, second.secret);
        assert!(first.secret.starts_with(KEY_PREFIX));
        assert_eq!(first.secret.len(), KEY_PREFIX.len() + KEY_BYTES * 2);
        assert_ne!(first.record.id, first.secret);
        assert!(!registry.keys.contains_key(&first.secret));
        assert!(registry.keys.contains_key(&first.record.id));
        assert_eq!(registry.get(&first.secret), Some(&first.record));
    }

    #[test]
    fn issue_and_verify() {
        let mut registry = ApiKeys::new(4).unwrap();
        let issued = registry.issue("node-cli", scopes(), 0, None).unwrap();
        let verified = registry.verify(&issued.secret, 10).unwrap();
        assert_eq!(verified.label, "node-cli");
        let mut expected_scopes = scopes();
        expected_scopes.sort();
        assert_eq!(verified.scopes, expected_scopes);
        assert!(!verified.revoked);
    }

    #[test]
    fn malformed_and_unknown_keys_are_typed_errors_without_echoing_secret() {
        let registry = ApiKeys::new(4).unwrap();
        let malformed = "do-not-log-this-secret";
        assert_eq!(
            registry.verify(malformed, 0).unwrap_err(),
            KeyError::InvalidFormat
        );
        assert_eq!(
            registry.verify(&valid_unknown_secret(), 0).unwrap_err(),
            KeyError::UnknownKey
        );
        assert!(!KeyError::UnknownKey.to_string().contains(malformed));
    }

    #[test]
    fn expiry_and_revocation() {
        let mut registry = ApiKeys::new(4).unwrap();
        let expiring = registry.issue("temp", scopes(), 0, Some(10)).unwrap();
        assert_eq!(
            registry.verify(&expiring.secret, 10).unwrap_err(),
            KeyError::Expired(expiring.record.id.clone(), 10)
        );
        assert!(registry.verify(&expiring.secret, 9).is_ok());

        let permanent = registry.issue("service", scopes(), 0, None).unwrap();
        registry.revoke(&permanent.secret).unwrap();
        assert_eq!(
            registry.verify(&permanent.secret, 1).unwrap_err(),
            KeyError::Revoked(permanent.record.id.clone())
        );
    }

    #[test]
    fn rotation_preserves_metadata_and_revokes_old_secret() {
        let mut registry = ApiKeys::new(4).unwrap();
        let old = registry.issue("ci-bot", scopes(), 0, Some(100)).unwrap();
        let fresh = registry.rotate(&old.secret, 20).unwrap();

        assert_eq!(fresh.record.label, "ci-bot");
        assert_eq!(fresh.record.scopes, old.record.scopes);
        assert_eq!(fresh.record.expires_tick, Some(100));
        assert_eq!(
            fresh.record.rotated_from.as_deref(),
            Some(old.record.id.as_str())
        );
        assert_eq!(
            registry.verify(&old.secret, 21).unwrap_err(),
            KeyError::Revoked(old.record.id.clone())
        );
        assert!(registry.verify(&fresh.secret, 21).is_ok());
    }

    #[test]
    fn failed_rotation_keeps_the_old_key_active() {
        let mut registry = ApiKeys::new(1).unwrap();
        let old = registry.issue("only", scopes(), 0, None).unwrap();
        assert_eq!(
            registry.rotate(&old.secret, 1).unwrap_err(),
            KeyError::RegistryFull
        );
        assert!(registry.verify(&old.secret, 1).is_ok());
        assert_eq!(registry.active(1).len(), 1);
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

    #[test]
    fn issued_secret_debug_output_is_redacted() {
        let mut registry = ApiKeys::new(1).unwrap();
        let issued = registry.issue("debug", scopes(), 0, None).unwrap();
        let debug = format!("{issued:?}");
        assert!(!debug.contains(&issued.secret));
        assert!(debug.contains("[REDACTED]"));
    }
}
