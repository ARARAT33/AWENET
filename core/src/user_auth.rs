//! AWEID proof-of-possession login and node-local bearer sessions.
//!
//! Sessions are deliberately memory-only: restarting a node invalidates every
//! session and requires the user to authenticate again. Only a SHA-256
//! fingerprint of each random bearer token is retained in memory.

use crate::{
    identity::Identity,
    user_registry::{UserAccount, UserRegistry, UserStatus},
};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt};
use thiserror::Error;
use zeroize::Zeroize;

const LOGIN_CONTEXT: &[u8] = b"AWENET-USER-LOGIN/v1\0";
const SESSION_FINGERPRINT_CONTEXT: &[u8] = b"AWENET-USER-SESSION-FINGERPRINT/v1\0";
const MAX_CHALLENGE_TTL_SECS: u64 = 300;
const MAX_SESSION_TTL_SECS: u64 = 24 * 60 * 60;
const MAX_CHALLENGES: usize = 4_096;
const MAX_SESSIONS: usize = 4_096;
const MAX_SESSIONS_PER_ACCOUNT: usize = 16;

/// Public challenge fields. The client signs user_login_message with the
/// private key corresponding to this AWEID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserLoginChallenge {
    pub challenge_id: [u8; 32],
    pub awe_id: String,
    pub nonce: [u8; 32],
    pub issued_at_unix: u64,
    pub expires_at_unix: u64,
    pub account_sequence: u64,
}

/// Domain-separated, unambiguous login proof message.
pub fn user_login_message(challenge: &UserLoginChallenge) -> Vec<u8> {
    let mut message =
        Vec::with_capacity(LOGIN_CONTEXT.len() + 32 + 8 + challenge.awe_id.len() + 32 + 24);
    message.extend_from_slice(LOGIN_CONTEXT);
    message.extend_from_slice(&challenge.challenge_id);
    message.extend_from_slice(&(challenge.awe_id.len() as u64).to_be_bytes());
    message.extend_from_slice(challenge.awe_id.as_bytes());
    message.extend_from_slice(&challenge.nonce);
    message.extend_from_slice(&challenge.issued_at_unix.to_be_bytes());
    message.extend_from_slice(&challenge.expires_at_unix.to_be_bytes());
    message.extend_from_slice(&challenge.account_sequence.to_be_bytes());
    message
}

#[derive(Clone, Debug)]
struct ChallengeRecord {
    challenge: UserLoginChallenge,
    public_key: [u8; 32],
}

#[derive(Clone, Debug)]
struct SessionRecord {
    awe_id: String,
    account_sequence: u64,
    expires_at_unix: u64,
}

/// In-memory, bounded login challenge and bearer-session store.
#[derive(Debug, Default)]
pub struct UserLogin {
    challenges: BTreeMap<String, ChallengeRecord>,
    sessions: BTreeMap<String, SessionRecord>,
}

/// A newly issued bearer token. Its Debug output is redacted and the token
/// bytes are zeroized when this value is dropped.
pub struct IssuedUserSession {
    pub token: [u8; 32],
    pub awe_id: String,
    pub expires_at_unix: u64,
}

impl fmt::Debug for IssuedUserSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IssuedUserSession")
            .field("token", &"[REDACTED]")
            .field("awe_id", &self.awe_id)
            .field("expires_at_unix", &self.expires_at_unix)
            .finish()
    }
}

impl Drop for IssuedUserSession {
    fn drop(&mut self) {
        self.token.zeroize();
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum UserLoginError {
    #[error("account is unknown or inactive")]
    UnknownOrInactive,
    #[error("login challenge is invalid, expired, or no longer matches the account")]
    InvalidChallenge,
    #[error("login proof signature is invalid")]
    InvalidSignature,
    #[error("session token is invalid or expired")]
    InvalidToken,
    #[error("login challenge capacity is exhausted")]
    ChallengeCapacity,
    #[error("session capacity is exhausted")]
    SessionCapacity,
    #[error("timestamp is outside the supported range")]
    TimestampOverflow,
    #[error("cryptographic randomness is unavailable")]
    RandomnessUnavailable,
}

impl UserLogin {
    /// Creates one active challenge per AWEID. Re-requesting a challenge for
    /// the same account invalidates its previous outstanding challenge.
    pub fn request_challenge(
        &mut self,
        registry: &UserRegistry,
        awe_id: &str,
        now_unix: u64,
        ttl_secs: u64,
    ) -> Result<UserLoginChallenge, UserLoginError> {
        let account = registry
            .get(awe_id)
            .filter(|account| account.status == UserStatus::Active)
            .ok_or(UserLoginError::UnknownOrInactive)?;

        self.sweep(now_unix);
        self.challenges
            .retain(|_, record| record.challenge.awe_id != awe_id);
        if self.challenges.len() >= MAX_CHALLENGES {
            return Err(UserLoginError::ChallengeCapacity);
        }

        let expires_at_unix = now_unix
            .checked_add(ttl_secs.clamp(1, MAX_CHALLENGE_TTL_SECS))
            .ok_or(UserLoginError::TimestampOverflow)?;

        for _ in 0..8 {
            let mut challenge_id = [0u8; 32];
            let mut nonce = [0u8; 32];
            OsRng
                .try_fill_bytes(&mut challenge_id)
                .map_err(|_| UserLoginError::RandomnessUnavailable)?;
            OsRng
                .try_fill_bytes(&mut nonce)
                .map_err(|_| UserLoginError::RandomnessUnavailable)?;

            let key = hex::encode(challenge_id);
            if self.challenges.contains_key(&key) {
                continue;
            }

            let challenge = UserLoginChallenge {
                challenge_id,
                awe_id: account.awe_id.clone(),
                nonce,
                issued_at_unix: now_unix,
                expires_at_unix,
                account_sequence: account.sequence,
            };
            self.challenges.insert(
                key,
                ChallengeRecord {
                    challenge: challenge.clone(),
                    public_key: account.public_key,
                },
            );
            return Ok(challenge);
        }

        Err(UserLoginError::RandomnessUnavailable)
    }

    /// Consumes a challenge exactly once and issues a random node-local
    /// bearer token after verifying proof of possession against the current
    /// active account record.
    pub fn complete_login(
        &mut self,
        registry: &UserRegistry,
        challenge_id: &[u8; 32],
        signature: &[u8; 64],
        now_unix: u64,
        session_ttl_secs: u64,
    ) -> Result<IssuedUserSession, UserLoginError> {
        let key = hex::encode(challenge_id);
        let record = self
            .challenges
            .remove(&key)
            .ok_or(UserLoginError::InvalidChallenge)?;

        if now_unix < record.challenge.issued_at_unix
            || now_unix >= record.challenge.expires_at_unix
        {
            return Err(UserLoginError::InvalidChallenge);
        }

        let account = registry
            .get(&record.challenge.awe_id)
            .filter(|account| {
                account.status == UserStatus::Active
                    && account.sequence == record.challenge.account_sequence
                    && account.public_key == record.public_key
            })
            .ok_or(UserLoginError::InvalidChallenge)?;

        let message = user_login_message(&record.challenge);
        if !Identity::verify(&record.public_key, &message, signature) {
            return Err(UserLoginError::InvalidSignature);
        }

        self.sweep(now_unix);
        if self.sessions.len() >= MAX_SESSIONS
            || self
                .sessions
                .values()
                .filter(|session| session.awe_id == account.awe_id)
                .count()
                >= MAX_SESSIONS_PER_ACCOUNT
        {
            return Err(UserLoginError::SessionCapacity);
        }

        let expires_at_unix = now_unix
            .checked_add(session_ttl_secs.clamp(1, MAX_SESSION_TTL_SECS))
            .ok_or(UserLoginError::TimestampOverflow)?;

        for _ in 0..8 {
            let mut token = [0u8; 32];
            OsRng
                .try_fill_bytes(&mut token)
                .map_err(|_| UserLoginError::RandomnessUnavailable)?;
            let fingerprint = token_fingerprint(&token);
            if self.sessions.contains_key(&fingerprint) {
                token.zeroize();
                continue;
            }

            self.sessions.insert(
                fingerprint,
                SessionRecord {
                    awe_id: account.awe_id.clone(),
                    account_sequence: account.sequence,
                    expires_at_unix,
                },
            );
            return Ok(IssuedUserSession {
                token,
                awe_id: account.awe_id.clone(),
                expires_at_unix,
            });
        }

        Err(UserLoginError::RandomnessUnavailable)
    }

    /// Validates a bearer token against the current account status and
    /// account-local sequence. Role changes and suspension therefore revoke
    /// existing sessions without invalidating unrelated users' sessions.
    pub fn validate_session(
        &mut self,
        registry: &UserRegistry,
        token: &[u8; 32],
        now_unix: u64,
    ) -> Result<UserAccount, UserLoginError> {
        let fingerprint = token_fingerprint(token);
        let record = self
            .sessions
            .get(&fingerprint)
            .cloned()
            .ok_or(UserLoginError::InvalidToken)?;

        if now_unix >= record.expires_at_unix {
            self.sessions.remove(&fingerprint);
            return Err(UserLoginError::InvalidToken);
        }

        let account = registry.get(&record.awe_id).filter(|account| {
            account.status == UserStatus::Active && account.sequence == record.account_sequence
        });
        match account {
            Some(account) => Ok(account.clone()),
            None => {
                self.sessions.remove(&fingerprint);
                Err(UserLoginError::InvalidToken)
            }
        }
    }

    /// Revokes one bearer token. The raw token is never stored.
    pub fn revoke_session(&mut self, token: &[u8; 32]) -> bool {
        self.sessions.remove(&token_fingerprint(token)).is_some()
    }

    /// Removes expired challenges and sessions.
    pub fn sweep(&mut self, now_unix: u64) {
        self.challenges
            .retain(|_, record| record.challenge.expires_at_unix > now_unix);
        self.sessions
            .retain(|_, session| session.expires_at_unix > now_unix);
    }

    #[cfg(test)]
    fn session_count(&self) -> usize {
        self.sessions.len()
    }
}

fn token_fingerprint(token: &[u8; 32]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(SESSION_FINGERPRINT_CONTEXT);
    hasher.update(token);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        identity::{Identity, Username},
        user_registry::{UserRegistry, UserRole, UserStatus},
    };

    fn identity(name: &str) -> Identity {
        Identity::generate(Username::new(name.to_string()).expect("valid username"))
    }

    fn register(registry: &mut UserRegistry, identity: &Identity, at_unix: u64) {
        let message = UserRegistry::registration_message(
            identity.public.username.as_str(),
            &identity.public.public_key,
        );
        let signature = identity.sign(&message);
        registry
            .register(
                identity.public.username.as_str(),
                &identity.public.public_key,
                &signature,
                at_unix,
            )
            .expect("register account");
    }

    fn login(
        manager: &mut UserLogin,
        registry: &UserRegistry,
        identity: &Identity,
        now_unix: u64,
        ttl_secs: u64,
    ) -> IssuedUserSession {
        let awe_id = identity.public.awe_id.to_hex();
        let challenge = manager
            .request_challenge(registry, &awe_id, now_unix, 120)
            .expect("issue login challenge");
        let signature = identity.sign(&user_login_message(&challenge));
        manager
            .complete_login(
                registry,
                &challenge.challenge_id,
                &signature,
                now_unix,
                ttl_secs,
            )
            .expect("complete login")
    }

    #[test]
    fn proof_of_possession_issues_a_valid_session() {
        let founder = identity("founder");
        let user = identity("member");
        let mut registry = UserRegistry::new(&founder, 1);
        register(&mut registry, &user, 2);
        let mut manager = UserLogin::default();

        let session = login(&mut manager, &registry, &user, 10, 600);
        assert_eq!(session.awe_id, user.public.awe_id.to_hex());
        assert_eq!(session.expires_at_unix, 610);
        let account = manager
            .validate_session(&registry, &session.token, 11)
            .expect("validate issued session");
        assert_eq!(account.awe_id, session.awe_id);
    }

    #[test]
    fn challenge_is_single_use() {
        let founder = identity("founder");
        let user = identity("member");
        let mut registry = UserRegistry::new(&founder, 1);
        register(&mut registry, &user, 2);
        let mut manager = UserLogin::default();
        let awe_id = user.public.awe_id.to_hex();
        let challenge = manager
            .request_challenge(&registry, &awe_id, 10, 120)
            .expect("challenge");
        let signature = user.sign(&user_login_message(&challenge));
        manager
            .complete_login(&registry, &challenge.challenge_id, &signature, 11, 600)
            .expect("first login");
        assert_eq!(
            manager
                .complete_login(&registry, &challenge.challenge_id, &signature, 11, 600)
                .unwrap_err(),
            UserLoginError::InvalidChallenge
        );
    }

    #[test]
    fn invalid_proof_consumes_the_challenge() {
        let founder = identity("founder");
        let user = identity("member");
        let impostor = identity("impostor");
        let mut registry = UserRegistry::new(&founder, 1);
        register(&mut registry, &user, 2);
        let mut manager = UserLogin::default();
        let awe_id = user.public.awe_id.to_hex();
        let challenge = manager
            .request_challenge(&registry, &awe_id, 10, 120)
            .expect("challenge");
        let invalid_signature = impostor.sign(&user_login_message(&challenge));
        assert_eq!(
            manager
                .complete_login(
                    &registry,
                    &challenge.challenge_id,
                    &invalid_signature,
                    11,
                    600
                )
                .unwrap_err(),
            UserLoginError::InvalidSignature
        );
        let valid_signature = user.sign(&user_login_message(&challenge));
        assert_eq!(
            manager
                .complete_login(
                    &registry,
                    &challenge.challenge_id,
                    &valid_signature,
                    11,
                    600
                )
                .unwrap_err(),
            UserLoginError::InvalidChallenge
        );
    }

    #[test]
    fn expired_challenge_is_rejected() {
        let founder = identity("founder");
        let user = identity("member");
        let mut registry = UserRegistry::new(&founder, 1);
        register(&mut registry, &user, 2);
        let mut manager = UserLogin::default();
        let awe_id = user.public.awe_id.to_hex();
        let challenge = manager
            .request_challenge(&registry, &awe_id, 10, 120)
            .expect("challenge");
        let signature = user.sign(&user_login_message(&challenge));
        assert_eq!(
            manager
                .complete_login(
                    &registry,
                    &challenge.challenge_id,
                    &signature,
                    challenge.expires_at_unix,
                    600
                )
                .unwrap_err(),
            UserLoginError::InvalidChallenge
        );
    }

    #[test]
    fn requesting_a_new_challenge_invalidates_the_previous_one() {
        let founder = identity("founder");
        let user = identity("member");
        let mut registry = UserRegistry::new(&founder, 1);
        register(&mut registry, &user, 2);
        let mut manager = UserLogin::default();
        let awe_id = user.public.awe_id.to_hex();
        let first = manager
            .request_challenge(&registry, &awe_id, 10, 120)
            .expect("first challenge");
        let second = manager
            .request_challenge(&registry, &awe_id, 11, 120)
            .expect("replacement challenge");
        let first_signature = user.sign(&user_login_message(&first));
        assert_eq!(
            manager
                .complete_login(&registry, &first.challenge_id, &first_signature, 12, 600)
                .unwrap_err(),
            UserLoginError::InvalidChallenge
        );
        let second_signature = user.sign(&user_login_message(&second));
        manager
            .complete_login(&registry, &second.challenge_id, &second_signature, 12, 600)
            .expect("replacement challenge succeeds");
    }

    #[test]
    fn suspension_revokes_existing_sessions() {
        let founder = identity("founder");
        let user = identity("member");
        let mut registry = UserRegistry::new(&founder, 1);
        register(&mut registry, &user, 2);
        let mut manager = UserLogin::default();
        let session = login(&mut manager, &registry, &user, 10, 600);

        let target = user.public.awe_id.to_hex();
        let at_unix = 20;
        let message = registry
            .status_change_message(&target, UserStatus::Suspended, at_unix)
            .expect("status message");
        let signature = founder.sign(&message);
        registry
            .set_status(&target, UserStatus::Suspended, at_unix, &signature)
            .expect("suspend user");

        assert_eq!(
            manager.validate_session(&registry, &session.token, 21),
            Err(UserLoginError::InvalidToken)
        );
    }

    #[test]
    fn role_change_revokes_existing_sessions() {
        let founder = identity("founder");
        let user = identity("member");
        let mut registry = UserRegistry::new(&founder, 1);
        register(&mut registry, &user, 2);
        let mut manager = UserLogin::default();
        let session = login(&mut manager, &registry, &user, 10, 600);

        let target = user.public.awe_id.to_hex();
        let at_unix = 20;
        let message = registry
            .role_change_message(&target, UserRole::Admin, at_unix)
            .expect("role message");
        let signature = founder.sign(&message);
        registry
            .set_role(&target, UserRole::Admin, at_unix, &signature)
            .expect("promote user");

        assert_eq!(
            manager.validate_session(&registry, &session.token, 21),
            Err(UserLoginError::InvalidToken)
        );
    }

    #[test]
    fn unrelated_registry_changes_do_not_revoke_a_session() {
        let founder = identity("founder");
        let user = identity("member");
        let other = identity("other");
        let mut registry = UserRegistry::new(&founder, 1);
        register(&mut registry, &user, 2);
        let mut manager = UserLogin::default();
        let session = login(&mut manager, &registry, &user, 10, 600);

        register(&mut registry, &other, 20);
        assert!(manager
            .validate_session(&registry, &session.token, 21)
            .is_ok());
    }

    #[test]
    fn logout_and_expiry_invalidate_tokens() {
        let founder = identity("founder");
        let user = identity("member");
        let mut registry = UserRegistry::new(&founder, 1);
        register(&mut registry, &user, 2);
        let mut manager = UserLogin::default();
        let session = login(&mut manager, &registry, &user, 10, 10);
        assert!(manager
            .validate_session(&registry, &session.token, 19)
            .is_ok());
        assert_eq!(
            manager.validate_session(&registry, &session.token, 20),
            Err(UserLoginError::InvalidToken)
        );

        let second = login(&mut manager, &registry, &user, 30, 100);
        assert!(manager.revoke_session(&second.token));
        assert!(!manager.revoke_session(&second.token));
        assert_eq!(
            manager.validate_session(&registry, &second.token, 31),
            Err(UserLoginError::InvalidToken)
        );
    }

    #[test]
    fn unknown_and_suspended_accounts_cannot_request_challenges() {
        let founder = identity("founder");
        let user = identity("member");
        let mut registry = UserRegistry::new(&founder, 1);
        register(&mut registry, &user, 2);
        let mut manager = UserLogin::default();
        assert_eq!(
            manager.request_challenge(&registry, "unknown", 10, 120),
            Err(UserLoginError::UnknownOrInactive)
        );

        let target = user.public.awe_id.to_hex();
        let message = registry
            .status_change_message(&target, UserStatus::Suspended, 20)
            .expect("status message");
        let signature = founder.sign(&message);
        registry
            .set_status(&target, UserStatus::Suspended, 20, &signature)
            .expect("suspend user");
        assert_eq!(
            manager.request_challenge(&registry, &target, 21, 120),
            Err(UserLoginError::UnknownOrInactive)
        );
    }

    #[test]
    fn session_debug_output_never_contains_the_bearer_token() {
        let founder = identity("founder");
        let user = identity("member");
        let mut registry = UserRegistry::new(&founder, 1);
        register(&mut registry, &user, 2);
        let mut manager = UserLogin::default();
        let session = login(&mut manager, &registry, &user, 10, 600);
        let token_hex = hex::encode(session.token);
        assert!(!format!("{session:?}").contains(&token_hex));
        assert_eq!(manager.session_count(), 1);
    }
}
