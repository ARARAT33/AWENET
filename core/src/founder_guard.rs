//! Founder account protection: passkey credentials, MFA step-up and recovery.
//!
//! The founder rights in founder.rs are exclusive but still rest on one
//! key. This module adds product-level account protection around them:
//! passkey credentials (ed25519 key pairs whose public keys are enrolled
//! with a founder-signed authorization), mandatory MFA step-up sessions
//! for founder actions, and single-use recovery codes that replace all
//! enrolled credentials. The desktop product performs the WebAuthn
//! ceremony with the platform authenticator and hands the credential
//! signature to these checks; the core never stores private key material.

use crate::founder::{FounderAction, FounderAuthority, FounderError};
use crate::identity::{AweId, Identity};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_PASSKEYS: usize = 8;
pub const MAX_RECOVERY_CODES: usize = 16;
pub const DEFAULT_STEP_UP_TTL_SECS: u64 = 900;
pub const MAX_CREDENTIAL_LABEL_CHARS: usize = 64;

pub const ENROLLMENT_CONTEXT: &[u8] = b"AWENET-FOUNDER-PASSKEY-ENROLL/v1";
pub const STEP_UP_CONTEXT: &[u8] = b"AWENET-FOUNDER-STEPUP/v1";
pub const RECOVERY_CONTEXT: &[u8] = b"AWENET-FOUNDER-RECOVERY-SET/v1";

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum FounderGuardError {
    #[error(transparent)]
    Founder(#[from] FounderError),
    #[error("founder passkey registry is full (max {MAX_PASSKEYS} credentials)")]
    PasskeyLimitReached,
    #[error("credential is already enrolled")]
    AlreadyEnrolled,
    #[error("no passkey credential is enrolled")]
    NoPasskeyEnrolled,
    #[error("credential is not enrolled")]
    UnknownCredential,
    #[error("a step-up challenge must be started first")]
    NoChallenge,
    #[error("step-up signature is invalid")]
    InvalidStepUp,
    #[error("founder signature is invalid")]
    InvalidFounderSignature,
    #[error("the step-up session expired")]
    StepUpExpired,
    #[error("recovery code is invalid")]
    InvalidRecoveryCode,
    #[error("recovery code was already used")]
    RecoveryCodeUsed,
    #[error("recovery code set is too large (max {MAX_RECOVERY_CODES} codes)")]
    TooManyRecoveryCodes,
    #[error("recovery requires at least one code")]
    EmptyRecoverySet,
    #[error("credential label exceeds {MAX_CREDENTIAL_LABEL_CHARS} characters")]
    LabelTooLong,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PasskeyCredential {
    pub credential_id: [u8; 32],
    pub public_key: [u8; 32],
    pub label: String,
    pub enrolled_at_unix: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StepUpSession {
    pub nonce: [u8; 32],
    pub granted_at_unix: u64,
    pub expires_at_unix: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FounderGuard {
    founder: AweId,
    founder_public_key: [u8; 32],
    installed_at_unix: u64,
    passkeys: Vec<PasskeyCredential>,
    recovery_codes: Vec<[u8; 32]>,
    used_recovery_codes: Vec<[u8; 32]>,
    challenge_counter: u64,
    pending_challenge: Option<[u8; 32]>,
    session: Option<StepUpSession>,
    step_up_ttl_secs: u64,
}

impl FounderGuard {
    pub fn install(founder: &Identity, at_unix: u64) -> Self {
        Self {
            founder: founder.public.awe_id.clone(),
            founder_public_key: founder.public.public_key,
            installed_at_unix: at_unix,
            passkeys: Vec::new(),
            recovery_codes: Vec::new(),
            used_recovery_codes: Vec::new(),
            challenge_counter: 0,
            pending_challenge: None,
            session: None,
            step_up_ttl_secs: DEFAULT_STEP_UP_TTL_SECS,
        }
    }

    pub fn founder_awe_id(&self) -> &AweId {
        &self.founder
    }

    pub fn installed_at_unix(&self) -> u64 {
        self.installed_at_unix
    }

    pub fn passkeys(&self) -> &[PasskeyCredential] {
        &self.passkeys
    }

    pub fn session(&self) -> Option<&StepUpSession> {
        self.session.as_ref()
    }

    pub fn set_step_up_ttl_secs(&mut self, ttl_secs: u64) {
        self.step_up_ttl_secs = ttl_secs.max(1);
    }

    pub fn enrollment_message(credential_public_key: &[u8; 32]) -> Vec<u8> {
        let mut message = Vec::new();
        message.extend_from_slice(ENROLLMENT_CONTEXT);
        message.extend_from_slice(credential_public_key);
        message
    }

    /// Enrolls a passkey credential. The enrollment is authorized by a
    /// signature of the founder key over the credential public key, so a
    /// leaked session or profile edit can never add credentials.
    pub fn enroll_passkey(
        &mut self,
        founder_signature: &[u8; 64],
        credential: &Identity,
        label: &str,
        at_unix: u64,
    ) -> Result<(), FounderGuardError> {
        if label.len() > MAX_CREDENTIAL_LABEL_CHARS {
            return Err(FounderGuardError::LabelTooLong);
        }
        let message = Self::enrollment_message(&credential.public.public_key);
        if !Identity::verify(&self.founder_public_key, &message, founder_signature) {
            return Err(FounderGuardError::InvalidFounderSignature);
        }
        if self.passkeys.len() >= MAX_PASSKEYS {
            return Err(FounderGuardError::PasskeyLimitReached);
        }
        if self
            .passkeys
            .iter()
            .any(|enrolled| enrolled.public_key == credential.public.public_key)
        {
            return Err(FounderGuardError::AlreadyEnrolled);
        }
        self.passkeys.push(PasskeyCredential {
            credential_id: *blake3::hash(&credential.public.public_key).as_bytes(),
            public_key: credential.public.public_key,
            label: label.to_string(),
            enrolled_at_unix: at_unix,
        });
        Ok(())
    }

    /// Starts an MFA step-up and returns the challenge nonce that an
    /// enrolled passkey must sign.
    pub fn begin_step_up(&mut self) -> [u8; 32] {
        self.challenge_counter = self.challenge_counter.saturating_add(1);
        let mut input = Vec::new();
        input.extend_from_slice(STEP_UP_CONTEXT);
        input.extend_from_slice(&self.founder_public_key);
        input.extend_from_slice(&self.challenge_counter.to_be_bytes());
        let nonce = *blake3::hash(&input).as_bytes();
        self.pending_challenge = Some(nonce);
        nonce
    }

    pub fn step_up_message(nonce: &[u8; 32]) -> Vec<u8> {
        let mut message = Vec::new();
        message.extend_from_slice(STEP_UP_CONTEXT);
        message.extend_from_slice(nonce);
        message
    }

    /// Completes the step-up with a signature from an enrolled passkey and
    /// opens a short-lived MFA session. An unknown credential leaves the
    /// challenge standing; a failed assertion consumes it.
    pub fn complete_step_up(
        &mut self,
        credential_public_key: &[u8; 32],
        signature: &[u8; 64],
        at_unix: u64,
    ) -> Result<(), FounderGuardError> {
        let nonce = self
            .pending_challenge
            .ok_or(FounderGuardError::NoChallenge)?;
        if !self
            .passkeys
            .iter()
            .any(|credential| credential.public_key == *credential_public_key)
        {
            return Err(FounderGuardError::UnknownCredential);
        }
        if !Identity::verify(
            credential_public_key,
            &Self::step_up_message(&nonce),
            signature,
        ) {
            self.pending_challenge = None;
            return Err(FounderGuardError::InvalidStepUp);
        }
        self.pending_challenge = None;
        self.session = Some(StepUpSession {
            nonce,
            granted_at_unix: at_unix,
            expires_at_unix: at_unix.saturating_add(self.step_up_ttl_secs),
        });
        Ok(())
    }

    pub fn session_active(&self, now_unix: u64) -> bool {
        matches!(&self.session, Some(session) if now_unix < session.expires_at_unix)
    }

    pub fn revoke_session(&mut self) {
        self.session = None;
    }

    /// Authorizes a founder action with MFA: the step-up session must be
    /// unexpired and the founder key itself must pass the backend check.
    pub fn authorize(
        &mut self,
        authority: &mut FounderAuthority,
        action: FounderAction,
        at_unix: u64,
        detail: &str,
    ) -> Result<u64, FounderGuardError> {
        if !self.session_active(at_unix) {
            self.session = None;
            return Err(FounderGuardError::StepUpExpired);
        }
        let sequence = authority.authorize(&self.founder_public_key, action, at_unix, detail)?;
        Ok(sequence)
    }

    /// Installs a fresh set of single-use recovery codes, authorized by a
    /// founder signature over the codes and the rotation time.
    pub fn set_recovery_codes(
        &mut self,
        founder_signature: &[u8; 64],
        codes: &[[u8; 32]],
        at_unix: u64,
    ) -> Result<(), FounderGuardError> {
        if codes.is_empty() {
            return Err(FounderGuardError::EmptyRecoverySet);
        }
        if codes.len() > MAX_RECOVERY_CODES {
            return Err(FounderGuardError::TooManyRecoveryCodes);
        }
        let mut message = Vec::new();
        message.extend_from_slice(RECOVERY_CONTEXT);
        for code in codes {
            message.extend_from_slice(code);
        }
        message.extend_from_slice(&at_unix.to_be_bytes());
        if !Identity::verify(&self.founder_public_key, &message, founder_signature) {
            return Err(FounderGuardError::InvalidFounderSignature);
        }
        self.recovery_codes = codes.iter().map(|code| *blake3::hash(code).as_bytes()).collect();
        self.used_recovery_codes.clear();
        Ok(())
    }

    pub fn recovery_codes_remaining(&self) -> usize {
        self.recovery_codes
            .len()
            .saturating_sub(self.used_recovery_codes.len())
    }

    /// Recovers founder access with a single-use recovery code: every
    /// enrolled passkey and the current session are revoked and one
    /// replacement credential is enrolled.
    pub fn recover_with_code(
        &mut self,
        code: &[u8; 32],
        replacement: &Identity,
        label: &str,
        at_unix: u64,
    ) -> Result<(), FounderGuardError> {
        if label.len() > MAX_CREDENTIAL_LABEL_CHARS {
            return Err(FounderGuardError::LabelTooLong);
        }
        let code_hash = *blake3::hash(code).as_bytes();
        if !self.recovery_codes.contains(&code_hash) {
            return Err(FounderGuardError::InvalidRecoveryCode);
        }
        if self.used_recovery_codes.contains(&code_hash) {
            return Err(FounderGuardError::RecoveryCodeUsed);
        }
        self.used_recovery_codes.push(code_hash);
        self.passkeys.clear();
        self.session = None;
        self.pending_challenge = None;
        self.passkeys.push(PasskeyCredential {
            credential_id: *blake3::hash(&replacement.public.public_key).as_bytes(),
            public_key: replacement.public.public_key,
            label: label.to_string(),
            enrolled_at_unix: at_unix,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Username;

    fn identity(name: &str) -> Identity {
        Identity::generate(Username::new(name).unwrap())
    }

    #[test]
    fn enrollment_requires_the_founder_signature() {
        let founder = identity("guard-founder");
        let passkey = identity("guard-passkey");
        let impostor = identity("guard-impostor");
        let mut guard = FounderGuard::install(&founder, 0);
        let message = FounderGuard::enrollment_message(&passkey.public.public_key);
        let bad = impostor.sign(&message);
        assert_eq!(
            guard
                .enroll_passkey(&bad, &passkey, "yubikey", 1)
                .unwrap_err(),
            FounderGuardError::InvalidFounderSignature
        );
        let good = founder.sign(&message);
        guard.enroll_passkey(&good, &passkey, "yubikey", 1).unwrap();
        assert_eq!(guard.passkeys().len(), 1);
        assert_eq!(guard.passkeys()[0].label, "yubikey");
        assert_eq!(guard.founder_awe_id(), &founder.public.awe_id);
        assert_eq!(guard.installed_at_unix(), 0);
        assert_eq!(
            guard
                .enroll_passkey(&good, &passkey, "again", 2)
                .unwrap_err(),
            FounderGuardError::AlreadyEnrolled
        );
    }

    #[test]
    fn passkey_registry_and_labels_are_bounded() {
        let founder = identity("bound-founder");
        let mut guard = FounderGuard::install(&founder, 0);
        for i in 0..MAX_PASSKEYS {
            let credential = identity(&format!("bound-pass-{i}"));
            let message = FounderGuard::enrollment_message(&credential.public.public_key);
            let signature = founder.sign(&message);
            guard.enroll_passkey(&signature, &credential, "key", 1).unwrap();
        }
        let extra = identity("bound-pass-extra");
        let message = FounderGuard::enrollment_message(&extra.public.public_key);
        let signature = founder.sign(&message);
        assert_eq!(
            guard.enroll_passkey(&signature, &extra, "key", 2).unwrap_err(),
            FounderGuardError::PasskeyLimitReached
        );
        let long = "x".repeat(MAX_CREDENTIAL_LABEL_CHARS + 1);
        assert_eq!(
            guard
                .enroll_passkey(&signature, &extra, &long, 3)
                .unwrap_err(),
            FounderGuardError::LabelTooLong
        );
    }

    #[test]
    fn authorize_requires_a_fresh_step_up() {
        let founder = identity("stepup-founder");
        let passkey = identity("stepup-passkey");
        let mut authority = FounderAuthority::install(&founder, 0);
        let mut guard = FounderGuard::install(&founder, 0);
        let enroll = founder.sign(&FounderGuard::enrollment_message(&passkey.public.public_key));
        guard.enroll_passkey(&enroll, &passkey, "key", 0).unwrap();
        assert_eq!(
            guard
                .authorize(&mut authority, FounderAction::SetOfficialPrice, 100, "price")
                .unwrap_err(),
            FounderGuardError::StepUpExpired
        );
        // an unknown credential cannot answer the challenge
        let nonce = guard.begin_step_up();
        let stranger = identity("stranger");
        let stranger_signature = stranger.sign(&FounderGuard::step_up_message(&nonce));
        assert_eq!(
            guard
                .complete_step_up(&stranger.public.public_key, &stranger_signature, 100)
                .unwrap_err(),
            FounderGuardError::UnknownCredential
        );
        // a failed assertion consumes the challenge
        let wrong = founder.sign(b"other");
        assert_eq!(
            guard
                .complete_step_up(&passkey.public.public_key, &wrong, 100)
                .unwrap_err(),
            FounderGuardError::InvalidStepUp
        );
        // a fresh challenge signed by the enrolled passkey opens a session
        let nonce = guard.begin_step_up();
        let signature = passkey.sign(&FounderGuard::step_up_message(&nonce));
        guard
            .complete_step_up(&passkey.public.public_key, &signature, 100)
            .unwrap();
        assert!(guard.session_active(100));
        let sequence = guard
            .authorize(&mut authority, FounderAction::SetOfficialPrice, 200, "price")
            .unwrap();
        assert_eq!(sequence, 1);
        // the session expires after the TTL and later actions are refused
        let past_ttl = 100 + DEFAULT_STEP_UP_TTL_SECS + 1;
        assert!(!guard.session_active(past_ttl));
        assert_eq!(
            guard
                .authorize(&mut authority, FounderAction::SetOfficialPrice, past_ttl, "price")
                .unwrap_err(),
            FounderGuardError::StepUpExpired
        );
        assert_eq!(authority.log_len(), 1);
    }

    #[test]
    fn recovery_codes_replace_credentials_exactly_once() {
        let founder = identity("recovery-founder");
        let passkey = identity("recovery-passkey");
        let replacement = identity("recovery-replacement");
        let mut guard = FounderGuard::install(&founder, 0);
        let enroll = founder.sign(&FounderGuard::enrollment_message(&passkey.public.public_key));
        guard.enroll_passkey(&enroll, &passkey, "key", 0).unwrap();
        let code = [7u8; 32];
        let mut message = Vec::new();
        message.extend_from_slice(RECOVERY_CONTEXT);
        message.extend_from_slice(&code);
        message.extend_from_slice(&5u64.to_be_bytes());
        let signature = founder.sign(&message);
        // empty and oversized sets are refused
        assert_eq!(
            guard
                .set_recovery_codes(&signature, &[], 5)
                .unwrap_err(),
            FounderGuardError::EmptyRecoverySet
        );
        let too_many: Vec<[u8; 32]> = (0..=MAX_RECOVERY_CODES)
            .map(|i| [i as u8; 32])
            .collect();
        assert_eq!(
            guard
                .set_recovery_codes(&signature, &too_many, 5)
                .unwrap_err(),
            FounderGuardError::TooManyRecoveryCodes
        );
        guard
            .set_recovery_codes(&signature, std::slice::from_ref(&code), 5)
            .unwrap();
        assert_eq!(guard.recovery_codes_remaining(), 1);
        assert_eq!(
            guard
                .recover_with_code(&[1; 32], &replacement, "new", 6)
                .unwrap_err(),
            FounderGuardError::InvalidRecoveryCode
        );
        guard.recover_with_code(&code, &replacement, "new", 6).unwrap();
        assert_eq!(guard.passkeys().len(), 1);
        assert_eq!(guard.passkeys()[0].public_key, replacement.public.public_key);
        assert_eq!(guard.recovery_codes_remaining(), 0);
        assert_eq!(
            guard
                .recover_with_code(&code, &replacement, "again", 7)
                .unwrap_err(),
            FounderGuardError::RecoveryCodeUsed
        );
        // the revoked passkey can no longer answer challenges
        let nonce = guard.begin_step_up();
        let old_signature = passkey.sign(&FounderGuard::step_up_message(&nonce));
        assert_eq!(
            guard
                .complete_step_up(&passkey.public.public_key, &old_signature, 7)
                .unwrap_err(),
            FounderGuardError::UnknownCredential
        );
    }
}
