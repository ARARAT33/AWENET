//! Device-bound founder login challenges and revocable sessions.
//!
//! This core never authenticates an unapproved device. Passwords unlock the
//! founder key locally; the remote protocol still requires a signature from a
//! device already admitted by FounderDevices. Persist this state securely and
//! deliver the challenge to the same device before exposing the Control Center.

use crate::{crypto::hash, founder::FounderDevices, founder_admin::FounderEmergencyLock};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt};
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
    #[error("founder account is emergency-locked")]
    AccountLocked,
    #[error("founder session is invalid, expired, revoked, or restricted")]
    InvalidSession,
    #[error("operating-system secure random generation failed")]
    EntropyUnavailable,
    #[error("authentication tick arithmetic overflowed")]
    TickOverflow,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FounderLoginChallenge {
    pub challenge_id: [u8; 32],
    pub device_id: [u8; 32],
    pub nonce: [u8; 32],
    pub issued_tick: u64,
    pub expires_tick: u64,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssuedFounderSession {
    /// Treat this as a bearer secret. Never log it or send it to another device.
    pub token: [u8; 32],
    pub device_id: [u8; 32],
    pub expires_tick: u64,
}

impl fmt::Debug for IssuedFounderSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IssuedFounderSession")
            .field("token", &"[REDACTED]")
            .field("device_id", &self.device_id)
            .field("expires_tick", &self.expires_tick)
            .finish()
    }
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
        let expires_tick = now
            .checked_add(ttl.max(1))
            .ok_or(FounderAuthError::TickOverflow)?;
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
        let mut rng = OsRng;
        if rng.try_fill_bytes(&mut nonce).is_err() {
            nonce.fill(0);
            return Err(FounderAuthError::EntropyUnavailable);
        }
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

    /// Emergency-lock-aware entry point for login APIs. Callers should use
    /// this instead of request_challenge when the network has an emergency state.
    pub fn request_challenge_guarded(
        &mut self,
        devices: &FounderDevices,
        emergency: &FounderEmergencyLock,
        requested_device_id: [u8; 32],
        now: u64,
        ttl: u64,
    ) -> Result<FounderLoginChallenge, FounderAuthError> {
        emergency
            .ensure_login_allowed()
            .map_err(|_| FounderAuthError::AccountLocked)?;
        self.request_challenge(devices, requested_device_id, now, ttl)
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
        let expires_tick = now
            .checked_add(session_ttl.max(1))
            .ok_or(FounderAuthError::TickOverflow)?;
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
        let mut rng = OsRng;
        if rng.try_fill_bytes(&mut token).is_err() {
            token.fill(0);
            return Err(FounderAuthError::EntropyUnavailable);
        }
        let token_hash = hash(b"AWENET/FOUNDER-SESSION/v1", &token);
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
        let digest = crate::founder_identity::bootstrap_digest(&secret);
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
    fn emergency_account_lock_blocks_login_challenges() {
        let (founder, device, devices, path) = setup();
        let mut emergency = crate::founder_admin::FounderEmergencyLock::default();
        let command = crate::founder_admin::FounderEmergencyCommand::sign(
            &founder,
            &FounderGenesis::load(&path).unwrap(),
            &emergency,
            true,
            false,
            1,
        )
        .unwrap();
        emergency
            .apply(&FounderGenesis::load(&path).unwrap(), command)
            .unwrap();
        let mut auth = FounderLogin::new();
        assert_eq!(
            auth.request_challenge_guarded(
                &devices,
                &emergency,
                device_id(&device.public.public_key),
                2,
                20,
            ),
            Err(FounderAuthError::AccountLocked)
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn copied_founder_authority_key_is_not_a_device_login_key() {
        let (founder, _device, devices, path) = setup();
        let mut auth = FounderLogin::new();
        assert_eq!(
            auth.request_challenge(&devices, device_id(&founder.public.public_key), 1, 10),
            Err(FounderAuthError::UnapprovedDevice)
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
    fn blocking_a_device_invalidates_its_existing_sessions() {
        let (_founder, first_device, mut devices, path) = setup();
        let second = Identity::generate(Username::new("second-device").unwrap());
        let request = devices
            .request_device(second.public.public_key, "Second device", 1)
            .unwrap();
        devices
            .prove_device_request(
                request.request_id,
                second.sign(&crate::founder_identity::device_proof_message(&request)),
            )
            .unwrap();
        let approval = first_device.sign(&crate::founder_identity::approval_message(
            &request,
            &crate::founder_identity::DeviceDecision::Approved,
        ));
        devices
            .decide_request(
                request.request_id,
                device_id(&first_device.public.public_key),
                crate::founder_identity::DeviceDecision::Approved,
                approval,
                2,
            )
            .unwrap();

        let second_id = device_id(&second.public.public_key);
        let mut auth = FounderLogin::new();
        let challenge = auth.request_challenge(&devices, second_id, 3, 20).unwrap();
        let session = auth
            .complete_login(
                &devices,
                challenge.challenge_id,
                second.sign(&login_message(&challenge)),
                4,
                100,
            )
            .unwrap();
        let block_signature = first_device.sign(&crate::founder_identity::device_control_message(
            second_id, true, 5,
        ));
        devices
            .block_device(
                second_id,
                true,
                device_id(&first_device.public.public_key),
                5,
                block_signature,
            )
            .unwrap();
        assert_eq!(
            auth.validate_session(&devices, &session.token, 6),
            Err(FounderAuthError::InvalidSession)
        );
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

    #[test]
    fn issued_session_debug_redacts_bearer_token() {
        let session = IssuedFounderSession {
            token: [0xA5; 32],
            device_id: [0x5A; 32],
            expires_tick: 42,
        };
        let debug = format!("{session:?}");
        assert!(!debug.contains(&hex::encode(session.token)));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn challenge_expiry_overflow_is_rejected_without_mutation() {
        let (_founder, device, devices, path) = setup();
        let mut auth = FounderLogin::new();
        assert_eq!(
            auth.request_challenge(&devices, device_id(&device.public.public_key), u64::MAX, 1),
            Err(FounderAuthError::TickOverflow)
        );
        assert!(auth.challenges.is_empty());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn session_expiry_overflow_does_not_issue_a_session() {
        let (_founder, device, devices, path) = setup();
        let mut auth = FounderLogin::new();
        let challenge = auth
            .request_challenge(
                &devices,
                device_id(&device.public.public_key),
                u64::MAX - 10,
                10,
            )
            .unwrap();
        let signature = device.sign(&login_message(&challenge));
        assert_eq!(
            auth.complete_login(&devices, challenge.challenge_id, signature, u64::MAX - 1, 2,),
            Err(FounderAuthError::TickOverflow)
        );
        assert_eq!(auth.active_session_count(u64::MAX - 1), 0);
        fs::remove_file(path).unwrap();
    }
}
