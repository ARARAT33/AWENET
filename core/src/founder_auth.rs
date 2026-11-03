//! Device-bound founder login challenges and revocable sessions.
//!
//! This core never authenticates an unapproved device. Passwords unlock the
//! founder key locally; the remote protocol still requires a signature from a
//! device already admitted by FounderDevices. Persist this state securely and
//! deliver the challenge to the same device before exposing the Control Center.

use crate::{crypto::hash, founder::FounderDevices};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_LOGIN_CHALLENGES: usize = 128;
pub const MAX_FOUNDER_SESSIONS: usize = 4096;
pub const MAX_FAILED_LOGINS: u8 = 5;
pub const LOGIN_LOCKOUT_TICKS: u64 = 60;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FounderAuthError {
    #[error("device is not approved or is blocked")]
    UnapprovedDevice,
    #[error("login challenge registry is full")]
    ChallengeLimit,
    #[error("login challenge is unknown, expired, or already used")]
    InvalidChallenge,
    #[error("login signature is invalid")]
    InvalidSignature,
    #[error("device login is temporarily locked")]
    TemporarilyLocked,
    #[error("founder session registry is full")]
    SessionLimit,
    #[error("founder session is invalid, expired, revoked, or restricted")]
    InvalidSession,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FounderLoginChallenge {
    pub challenge_id: [u8; 32],
    pub device_id: [u8; 32],
    pub nonce: [u8; 32],
    pub issued_tick: u64,
    pub expires_tick: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssuedFounderSession {
    /// Treat this as a bearer secret. Never log it or send it to another device.
    pub token: [u8; 32],
    pub device_id: [u8; 32],
    pub expires_tick: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct SessionRecord {
    device_id: [u8; 32],
    expires_tick: u64,
    revoked: bool,
    restricted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
struct LoginFailureState {
    failures: u8,
    locked_until: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct FounderLogin {
    challenges: BTreeMap<String, FounderLoginChallenge>,
    sessions: BTreeMap<String, SessionRecord>,
    failures: BTreeMap<String, LoginFailureState>,
}

#[derive(Serialize)]
struct LoginStatement {
    domain: &'static str,
    challenge_id: [u8; 32],
    device_id: [u8; 32],
    nonce: [u8; 32],
    issued_tick: u64,
    expires_tick: u64,
}

pub fn login_message(challenge: &FounderLoginChallenge) -> Vec<u8> {
    serde_json::to_vec(&LoginStatement {
        domain: "AWENET/FOUNDER-LOGIN/v1",
        challenge_id: challenge.challenge_id,
        device_id: challenge.device_id,
        nonce: challenge.nonce,
        issued_tick: challenge.issued_tick,
        expires_tick: challenge.expires_tick,
    })
    .expect("login statement serialization is infallible")
}

impl FounderLogin {
    pub fn new() -> Self {
        Self::default()
    }

    /// Issues a short-lived challenge only to a currently approved device.
    pub fn request_challenge(
        &mut self,
        devices: &FounderDevices,
        requested_device_id: [u8; 32],
        now: u64,
        ttl: u64,
    ) -> Result<FounderLoginChallenge, FounderAuthError> {
        if !devices
            .approved_devices()
            .iter()
            .any(|device| device.device_id == requested_device_id)
        {
            return Err(FounderAuthError::UnapprovedDevice);
        }
        let failure = self
            .failures
            .get(&hex::encode(requested_device_id))
            .cloned()
            .unwrap_or_default();
        if now < failure.locked_until {
            return Err(FounderAuthError::TemporarilyLocked);
        }
        if self.challenges.len() >= MAX_LOGIN_CHALLENGES {
            self.sweep_challenges(now);
            if self.challenges.len() >= MAX_LOGIN_CHALLENGES {
                return Err(FounderAuthError::ChallengeLimit);
            }
        }
        let mut nonce = [0u8; 32];
        OsRng.fill_bytes(&mut nonce);
        let expires_tick = now.saturating_add(ttl.max(1));
        let challenge_id = hash(
            b"AWENET/FOUNDER-LOGIN-CHALLENGE/v1",
            &[
                requested_device_id.as_slice(),
                nonce.as_slice(),
                &now.to_be_bytes(),
            ]
            .concat(),
        );
        let challenge = FounderLoginChallenge {
            challenge_id,
            device_id: requested_device_id,
            nonce,
            issued_tick: now,
            expires_tick,
        };
        self.challenges
            .insert(hex::encode(challenge_id), challenge.clone());
        Ok(challenge)
    }

    /// Verifies proof of possession of the approved device key and creates a
    /// device-bound session. A challenge is consumed even after a bad proof.
    pub fn complete_login(
        &mut self,
        devices: &FounderDevices,
        challenge_id: [u8; 32],
        signature: [u8; 64],
        now: u64,
        session_ttl: u64,
    ) -> Result<IssuedFounderSession, FounderAuthError> {
        let key = hex::encode(challenge_id);
        let challenge = self
            .challenges
            .remove(&key)
            .ok_or(FounderAuthError::InvalidChallenge)?;
        if now >= challenge.expires_tick {
            return Err(FounderAuthError::InvalidChallenge);
        }
        if self
            .failures
            .get(&hex::encode(challenge.device_id))
            .is_some_and(|failure| now < failure.locked_until)
        {
            return Err(FounderAuthError::TemporarilyLocked);
        }
        let device = devices
            .approved_devices()
            .into_iter()
            .find(|d| d.device_id == challenge.device_id)
            .ok_or(FounderAuthError::UnapprovedDevice)?;
        let device_key = VerifyingKey::from_bytes(&device.public_key)
            .map_err(|_| FounderAuthError::InvalidSignature)?;
        if device_key
            .verify(
                &login_message(&challenge),
                &Signature::from_bytes(&signature),
            )
            .is_err()
        {
            let failure = self
                .failures
                .entry(hex::encode(challenge.device_id))
                .or_default();
            failure.failures = failure.failures.saturating_add(1);
            if failure.failures >= MAX_FAILED_LOGINS {
                failure.locked_until = now.saturating_add(LOGIN_LOCKOUT_TICKS);
                failure.failures = 0;
            }
            return Err(FounderAuthError::InvalidSignature);
        }

        if self.sessions.len() >= MAX_FOUNDER_SESSIONS {
            self.sweep_sessions(now);
            if self.sessions.len() >= MAX_FOUNDER_SESSIONS {
                return Err(FounderAuthError::SessionLimit);
            }
        }
        self.failures.remove(&hex::encode(challenge.device_id));
        let mut token = [0u8; 32];
        OsRng.fill_bytes(&mut token);
        let token_hash = hash(b"AWENET/FOUNDER-SESSION/v1", &token);
        let expires_tick = now.saturating_add(session_ttl.max(1));
        self.sessions.insert(
            hex::encode(token_hash),
            SessionRecord {
                device_id: challenge.device_id,
                expires_tick,
                revoked: false,
                restricted: false,
            },
        );
        Ok(IssuedFounderSession {
            token,
            device_id: challenge.device_id,
            expires_tick,
        })
    }

    /// Returns the device bound to a valid, unrestricted session.
    pub fn validate_session(
        &self,
        devices: &FounderDevices,
        token: &[u8; 32],
        now: u64,
    ) -> Result<[u8; 32], FounderAuthError> {
        let key = hex::encode(hash(b"AWENET/FOUNDER-SESSION/v1", token));
        let session = self
            .sessions
            .get(&key)
            .ok_or(FounderAuthError::InvalidSession)?;
        if session.revoked
            || session.restricted
            || now >= session.expires_tick
            || !devices
                .approved_devices()
                .iter()
                .any(|device| device.device_id == session.device_id)
        {
            return Err(FounderAuthError::InvalidSession);
        }
        Ok(session.device_id)
    }

    pub fn restrict_session(&mut self, token: &[u8; 32]) -> Result<(), FounderAuthError> {
        let key = hex::encode(hash(b"AWENET/FOUNDER-SESSION/v1", token));
        let session = self
            .sessions
            .get_mut(&key)
            .ok_or(FounderAuthError::InvalidSession)?;
        session.restricted = true;
        Ok(())
    }

    pub fn revoke_session(&mut self, token: &[u8; 32]) -> Result<(), FounderAuthError> {
        let key = hex::encode(hash(b"AWENET/FOUNDER-SESSION/v1", token));
        let session = self
            .sessions
            .get_mut(&key)
            .ok_or(FounderAuthError::InvalidSession)?;
        session.revoked = true;
        Ok(())
    }

    /// Global logout: revokes every session, including sessions on other devices.
    pub fn revoke_all_sessions(&mut self) -> usize {
        let mut changed = 0;
        for session in self.sessions.values_mut() {
            if !session.revoked {
                session.revoked = true;
                changed += 1;
            }
        }
        changed
    }

    pub fn active_session_count(&self, now: u64) -> usize {
        self.sessions
            .values()
            .filter(|s| !s.revoked && !s.restricted && now < s.expires_tick)
            .count()
    }

    pub fn sweep_challenges(&mut self, now: u64) -> usize {
        let before = self.challenges.len();
        self.challenges.retain(|_, c| now < c.expires_tick);
        before - self.challenges.len()
    }

    pub fn sweep_sessions(&mut self, now: u64) -> usize {
        let before = self.sessions.len();
        self.sessions.retain(|_, s| now < s.expires_tick);
        before - self.sessions.len()
    }

    pub fn record_suspicious_login(&mut self, token: &[u8; 32]) -> Result<(), FounderAuthError> {
        self.restrict_session(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        founder::{device_id, FounderDevices, FounderGenesis, FounderRules},
        identity::{Identity, Username},
    };
    use rand::rngs::OsRng;
    use rand::RngCore;
    use std::{fs, path::PathBuf};

    fn setup() -> (Identity, Identity, FounderDevices, PathBuf) {
        let founder = Identity::generate(Username::new("AWENeter").unwrap());
        let first_device = Identity::generate(Username::new("founder-device").unwrap());
        let secret = vec![0xB7; 48];
        let digest = crate::founder::bootstrap_digest(&secret);
        let mut random = [0u8; 8];
        OsRng.fill_bytes(&mut random);
        let path = std::env::temp_dir().join(format!("awenet-auth-{}.json", hex::encode(random)));
        let genesis = FounderGenesis::initialize_once(
            &path,
            &founder,
            first_device.public.public_key,
            &secret,
            digest,
            FounderRules::default(),
        )
        .unwrap();
        let devices = FounderDevices::new(&genesis, "Founder device").unwrap();
        (founder, first_device, devices, path)
    }

    #[test]
    fn approved_device_can_login_and_session_expires() {
        let (_founder, device, devices, path) = setup();
        let id = device_id(&device.public.public_key);
        let mut auth = FounderLogin::new();
        let challenge = auth.request_challenge(&devices, id, 10, 20).unwrap();
        let signature = device.sign(&login_message(&challenge));
        let session = auth
            .complete_login(&devices, challenge.challenge_id, signature, 11, 50)
            .unwrap();
        assert_eq!(
            auth.validate_session(&devices, &session.token, 12).unwrap(),
            id
        );
        assert_eq!(
            auth.validate_session(&devices, &session.token, 61),
            Err(FounderAuthError::InvalidSession)
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn unknown_device_cannot_request_login_challenge() {
        let (_founder, _device, devices, path) = setup();
        let mut auth = FounderLogin::new();
        assert_eq!(
            auth.request_challenge(&devices, [9; 32], 1, 10),
            Err(FounderAuthError::UnapprovedDevice)
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn bad_signatures_trigger_temporary_lockout() {
        let (_founder, device, devices, path) = setup();
        let id = device_id(&device.public.public_key);
        let attacker = Identity::generate(Username::new("attacker").unwrap());
        let mut auth = FounderLogin::new();
        for tick in 1..=MAX_FAILED_LOGINS {
            let c = auth
                .request_challenge(&devices, id, tick as u64, 20)
                .unwrap();
            let bad = attacker.sign(&login_message(&c));
            assert_eq!(
                auth.complete_login(&devices, c.challenge_id, bad, tick as u64, 100),
                Err(FounderAuthError::InvalidSignature)
            );
        }
        assert_eq!(
            auth.request_challenge(&devices, id, 20, 20),
            Err(FounderAuthError::TemporarilyLocked)
        );
        assert!(auth.request_challenge(&devices, id, 100, 20).is_ok());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn global_logout_and_suspicious_restriction_work() {
        let (_founder, device, devices, path) = setup();
        let id = device_id(&device.public.public_key);
        let mut auth = FounderLogin::new();
        let c1 = auth.request_challenge(&devices, id, 1, 20).unwrap();
        let s1 = auth
            .complete_login(
                &devices,
                c1.challenge_id,
                device.sign(&login_message(&c1)),
                2,
                100,
            )
            .unwrap();
        let c2 = auth.request_challenge(&devices, id, 3, 20).unwrap();
        let s2 = auth
            .complete_login(
                &devices,
                c2.challenge_id,
                device.sign(&login_message(&c2)),
                4,
                100,
            )
            .unwrap();
        auth.record_suspicious_login(&s1.token).unwrap();
        assert_eq!(
            auth.validate_session(&devices, &s1.token, 5),
            Err(FounderAuthError::InvalidSession)
        );
        assert!(auth.validate_session(&devices, &s2.token, 5).is_ok());
        assert_eq!(auth.revoke_all_sessions(), 2);
        assert_eq!(auth.active_session_count(5), 0);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn challenge_cannot_be_replayed() {
        let (_founder, device, devices, path) = setup();
        let id = device_id(&device.public.public_key);
        let mut auth = FounderLogin::new();
        let c = auth.request_challenge(&devices, id, 1, 20).unwrap();
        let sig = device.sign(&login_message(&c));
        auth.complete_login(&devices, c.challenge_id, sig, 2, 100)
            .unwrap();
        assert_eq!(
            auth.complete_login(&devices, c.challenge_id, sig, 3, 100),
            Err(FounderAuthError::InvalidChallenge)
        );
        fs::remove_file(path).unwrap();
    }
}
