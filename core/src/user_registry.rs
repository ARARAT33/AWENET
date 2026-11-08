//! Durable local account directory for immutable AWE IDs.
//!
//! Registration proves possession of the Ed25519 identity key. The founder
//! signs every role/status change, and the complete registry is persisted as
//! a founder-signed atomic snapshot. This module is an account directory;
//! it does not by itself grant HTTP/API capabilities to an Admin role.

use crate::identity::{AweId, Identity, Username};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};
use thiserror::Error;
use zeroize::Zeroize;

pub const USER_REGISTRY_VERSION: u16 = 1;
pub const MAX_USERS: usize = 100_000;
const REGISTER_CONTEXT: &[u8] = b"AWENET-USER-REGISTER/v1\0";
const MUTATION_CONTEXT: &[u8] = b"AWENET-USER-MUTATION/v1\0";
const STORE_CONTEXT: &[u8] = b"AWENET-USER-REGISTRY-STORE/v1\0";

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UserRole {
    Founder,
    Admin,
    User,
}

impl UserRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Founder => "founder",
            Self::Admin => "admin",
            Self::User => "user",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UserStatus {
    Active,
    Suspended,
}

impl UserStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Suspended => "suspended",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserAccount {
    pub awe_id: String,
    pub username: String,
    pub public_key: [u8; 32],
    pub role: UserRole,
    pub status: UserStatus,
    pub created_at_unix: u64,
    pub updated_at_unix: u64,
    pub sequence: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserRegistry {
    version: u16,
    founder_awe_id: String,
    founder_public_key: [u8; 32],
    sequence: u64,
    accounts: BTreeMap<String, UserAccount>,
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum UserRegistryError {
    #[error("username must contain 1 to 64 ASCII letters, digits, '_' or '-'")]
    InvalidUsername,
    #[error("identity public key must be 32 bytes")]
    InvalidPublicKey,
    #[error("identity proof signature is invalid")]
    InvalidSignature,
    #[error("AWE ID does not match the supplied public key")]
    IdentityMismatch,
    #[error("username is already registered")]
    UsernameTaken,
    #[error("account is already registered")]
    AlreadyRegistered,
    #[error("account was not found")]
    UnknownUser,
    #[error("the founder account cannot be demoted or suspended")]
    FounderProtected,
    #[error("registry has reached its user limit")]
    RegistryFull,
    #[error("user registry sequence is exhausted")]
    SequenceExhausted,
    #[error("user mutation timestamp cannot move backwards")]
    InvalidTimestamp,
    #[error("unsupported user registry version")]
    InvalidStoreVersion,
    #[error("user registry belongs to another founder")]
    WrongFounder,
    #[error("user registry signature verification failed")]
    InvalidStoreSignature,
    #[error("user registry snapshot is inconsistent")]
    InconsistentRegistry,
    #[error("user registry serialization failed: {0}")]
    Serialization(String),
    #[error("user registry I/O failed: {0}")]
    Io(String),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct SignedRegistryFile {
    version: u16,
    registry: UserRegistry,
    #[serde(with = "crate::serde_bytes_64")]
    signature: [u8; 64],
}

impl UserRegistry {
    /// Installs the local node identity as the immutable founder account.
    pub fn new(founder: &Identity, at_unix: u64) -> Self {
        let awe_id = founder.public.awe_id.to_hex();
        let username = founder.public.username.as_str().to_string();
        let founder_account = UserAccount {
            awe_id: awe_id.clone(),
            username,
            public_key: founder.public.public_key,
            role: UserRole::Founder,
            status: UserStatus::Active,
            created_at_unix: at_unix,
            updated_at_unix: at_unix,
            sequence: 1,
        };
        let mut accounts = BTreeMap::new();
        accounts.insert(awe_id.clone(), founder_account);
        Self {
            version: USER_REGISTRY_VERSION,
            founder_awe_id: awe_id,
            founder_public_key: founder.public.public_key,
            sequence: 1,
            accounts,
        }
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn founder_awe_id(&self) -> &str {
        &self.founder_awe_id
    }

    pub fn len(&self) -> usize {
        self.accounts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty()
    }

    pub fn get(&self, awe_id: &str) -> Option<&UserAccount> {
        self.accounts.get(awe_id)
    }

    pub fn accounts(&self) -> Vec<UserAccount> {
        self.accounts.values().cloned().collect()
    }

    /// Canonical proof-of-possession message for a new account.
    pub fn registration_message(username: &str, public_key: &[u8; 32]) -> Vec<u8> {
        let mut message = Vec::with_capacity(REGISTER_CONTEXT.len() + 2 + username.len() + 32);
        message.extend_from_slice(REGISTER_CONTEXT);
        message.extend_from_slice(&(username.len() as u16).to_be_bytes());
        message.extend_from_slice(username.as_bytes());
        message.extend_from_slice(public_key);
        message
    }

    /// Registers a self-certifying AWE ID after verifying its owner's signature.
    /// Repeating the exact same registration is idempotent and does not advance
    /// the registry sequence.
    pub fn register(
        &mut self,
        username: &str,
        public_key: &[u8; 32],
        signature: &[u8; 64],
        at_unix: u64,
    ) -> Result<UserAccount, UserRegistryError> {
        let username = Username::new(username.to_string())
            .map_err(|_| UserRegistryError::InvalidUsername)?
            .as_str()
            .to_string();
        let awe_id = AweId::from_public_key(public_key).to_hex();
        let message = Self::registration_message(&username, public_key);
        if !Identity::verify(public_key, &message, signature) {
            return Err(UserRegistryError::InvalidSignature);
        }

        if let Some(existing) = self.accounts.get(&awe_id) {
            if existing.username == username && existing.public_key == *public_key {
                return Ok(existing.clone());
            }
            return Err(UserRegistryError::IdentityMismatch);
        }
        if self.accounts.values().any(|account| {
            account.username.eq_ignore_ascii_case(&username)
        }) {
            return Err(UserRegistryError::UsernameTaken);
        }
        if self.accounts.len() >= MAX_USERS {
            return Err(UserRegistryError::RegistryFull);
        }

        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or(UserRegistryError::SequenceExhausted)?;
        let account = UserAccount {
            awe_id: awe_id.clone(),
            username,
            public_key: *public_key,
            role: UserRole::User,
            status: UserStatus::Active,
            created_at_unix: at_unix,
            updated_at_unix: at_unix,
            sequence,
        };
        self.accounts.insert(awe_id, account.clone());
        self.sequence = sequence;
        Ok(account)
    }

    /// Returns the exact message a founder must sign for a role change.
    pub fn role_change_message(
        &self,
        target_awe_id: &str,
        role: UserRole,
        at_unix: u64,
    ) -> Result<Vec<u8>, UserRegistryError> {
        self.mutation_message("set_role", target_awe_id, role.as_str(), at_unix)
    }

    /// Returns the exact message a founder must sign for a status change.
    pub fn status_change_message(
        &self,
        target_awe_id: &str,
        status: UserStatus,
        at_unix: u64,
    ) -> Result<Vec<u8>, UserRegistryError> {
        self.mutation_message("set_status", target_awe_id, status.as_str(), at_unix)
    }

    pub fn set_role(
        &mut self,
        target_awe_id: &str,
        role: UserRole,
        at_unix: u64,
        founder_signature: &[u8; 64],
    ) -> Result<UserAccount, UserRegistryError> {
        let current = self
            .accounts
            .get(target_awe_id)
            .ok_or(UserRegistryError::UnknownUser)?;
        if target_awe_id == self.founder_awe_id && role != UserRole::Founder {
            return Err(UserRegistryError::FounderProtected);
        }
        if target_awe_id != self.founder_awe_id && role == UserRole::Founder {
            return Err(UserRegistryError::FounderProtected);
        }

        if at_unix < current.updated_at_unix {
            return Err(UserRegistryError::InvalidTimestamp);
        }
        let message = self.role_change_message(target_awe_id, role, at_unix)?;
        if !Identity::verify(&self.founder_public_key, &message, founder_signature) {
            return Err(UserRegistryError::InvalidSignature);
        }
        if current.role == role {
            return Ok(current.clone());
        }

        let sequence = self.next_sequence()?;
        let account = self.accounts.get_mut(target_awe_id).ok_or(UserRegistryError::UnknownUser)?;
        account.role = role;
        account.updated_at_unix = at_unix;
        account.sequence = sequence;
        let updated = account.clone();
        self.sequence = sequence;
        Ok(updated)
    }

    pub fn set_status(
        &mut self,
        target_awe_id: &str,
        status: UserStatus,
        at_unix: u64,
        founder_signature: &[u8; 64],
    ) -> Result<UserAccount, UserRegistryError> {
        let current = self
            .accounts
            .get(target_awe_id)
            .ok_or(UserRegistryError::UnknownUser)?;
        if target_awe_id == self.founder_awe_id && status != UserStatus::Active {
            return Err(UserRegistryError::FounderProtected);
        }

        if at_unix < current.updated_at_unix {
            return Err(UserRegistryError::InvalidTimestamp);
        }
        let message = self.status_change_message(target_awe_id, status, at_unix)?;
        if !Identity::verify(&self.founder_public_key, &message, founder_signature) {
            return Err(UserRegistryError::InvalidSignature);
        }
        if current.status == status {
            return Ok(current.clone());
        }

        let sequence = self.next_sequence()?;
        let account = self.accounts.get_mut(target_awe_id).ok_or(UserRegistryError::UnknownUser)?;
        account.status = status;
        account.updated_at_unix = at_unix;
        account.sequence = sequence;
        let updated = account.clone();
        self.sequence = sequence;
        Ok(updated)
    }

    fn mutation_message(
        &self,
        action: &str,
        target_awe_id: &str,
        value: &str,
        at_unix: u64,
    ) -> Result<Vec<u8>, UserRegistryError> {
        let sequence = self.next_sequence()?;
        let mut message = Vec::new();
        message.extend_from_slice(MUTATION_CONTEXT);
        for field in [action, target_awe_id, value] {
            let length = u16::try_from(field.len())
                .map_err(|_| UserRegistryError::InconsistentRegistry)?;
            message.extend_from_slice(&length.to_be_bytes());
            message.extend_from_slice(field.as_bytes());
        }
        message.extend_from_slice(&sequence.to_be_bytes());
        message.extend_from_slice(&at_unix.to_be_bytes());
        Ok(message)
    }

    fn next_sequence(&self) -> Result<u64, UserRegistryError> {
        self.sequence
            .checked_add(1)
            .ok_or(UserRegistryError::SequenceExhausted)
    }

    fn validate(&self) -> Result<(), UserRegistryError> {
        if self.version != USER_REGISTRY_VERSION
            || self.sequence == 0
            || self.accounts.is_empty()
            || self.accounts.len() > MAX_USERS
        {
            return Err(UserRegistryError::InconsistentRegistry);
        }
        let founder = self
            .accounts
            .get(&self.founder_awe_id)
            .ok_or(UserRegistryError::InconsistentRegistry)?;
        if founder.role != UserRole::Founder
            || founder.status != UserStatus::Active
            || founder.public_key != self.founder_public_key
        {
            return Err(UserRegistryError::InconsistentRegistry);
        }

        let mut usernames = std::collections::BTreeSet::new();
        for (id, account) in &self.accounts {
            if id != &account.awe_id
                || AweId::from_public_key(&account.public_key).to_hex() != account.awe_id
                || account.sequence == 0
                || account.sequence > self.sequence
                || Username::new(account.username.clone()).is_err()
                || !usernames.insert(account.username.to_ascii_lowercase())
                || (account.role == UserRole::Founder && id != &self.founder_awe_id)
                || account.created_at_unix > account.updated_at_unix
            {
                return Err(UserRegistryError::InconsistentRegistry);
            }
        }
        Ok(())
    }
}

fn signing_bytes(registry: &UserRegistry) -> Result<Vec<u8>, UserRegistryError> {
    let serialized = serde_json::to_vec(registry)
        .map_err(|error| UserRegistryError::Serialization(error.to_string()))?;
    let mut bytes = Vec::with_capacity(STORE_CONTEXT.len() + serialized.len());
    bytes.extend_from_slice(STORE_CONTEXT);
    bytes.extend_from_slice(&serialized);
    Ok(bytes)
}

/// Atomically saves a founder-signed registry snapshot. The raw signing key is
/// never written to this file; only the signed public account directory is.
pub fn save(
    path: &Path,
    registry: &UserRegistry,
    founder: &Identity,
) -> Result<(), UserRegistryError> {
    if registry.founder_awe_id != founder.public.awe_id.to_hex()
        || registry.founder_public_key != founder.public.public_key
    {
        return Err(UserRegistryError::WrongFounder);
    }
    registry.validate()?;
    let file = SignedRegistryFile {
        version: USER_REGISTRY_VERSION,
        registry: registry.clone(),
        signature: founder.sign(&signing_bytes(registry)?),
    };
    let encoded = serde_json::to_vec_pretty(&file)
        .map_err(|error| UserRegistryError::Serialization(error.to_string()))?;
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|error| UserRegistryError::Io(error.to_string()))?;
    }

    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut opened = None;
    for _ in 0..8 {
        let mut nonce = [0u8; 16];
        let mut rng = OsRng;
        if rng.try_fill_bytes(&mut nonce).is_err() {
            nonce.zeroize();
            return Err(UserRegistryError::Io("secure random generation failed".into()));
        }
        let temporary = parent.join(format!(
            ".awenet-users-{}-{}.tmp",
            std::process::id(),
            hex::encode(nonce)
        ));
        nonce.zeroize();
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&temporary) {
            Ok(file) => {
                opened = Some((temporary, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(UserRegistryError::Io(error.to_string())),
        }
    }
    let Some((temporary, mut file)) = opened else {
        return Err(UserRegistryError::Io(
            "could not allocate a unique temporary registry file".into(),
        ));
    };
    if let Err(error) = file.write_all(&encoded).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(&temporary);
        return Err(UserRegistryError::Io(error.to_string()));
    }
    drop(file);
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(UserRegistryError::Io(error.to_string()));
    }
    #[cfg(unix)]
    if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

/// Loads a registry only when its founder signature and internal invariants
/// match the identity already installed on this node.
pub fn load(path: &Path, founder: &Identity) -> Result<UserRegistry, UserRegistryError> {
    let bytes = fs::read(path).map_err(|error| UserRegistryError::Io(error.to_string()))?;
    let file: SignedRegistryFile = serde_json::from_slice(&bytes)
        .map_err(|error| UserRegistryError::Serialization(error.to_string()))?;
    if file.version != USER_REGISTRY_VERSION || file.registry.version != USER_REGISTRY_VERSION {
        return Err(UserRegistryError::InvalidStoreVersion);
    }
    if file.registry.founder_awe_id != founder.public.awe_id.to_hex()
        || file.registry.founder_public_key != founder.public.public_key
    {
        return Err(UserRegistryError::WrongFounder);
    }
    if !Identity::verify(
        &founder.public.public_key,
        &signing_bytes(&file.registry)?,
        &file.signature,
    ) {
        return Err(UserRegistryError::InvalidStoreSignature);
    }
    file.registry.validate()?;
    Ok(file.registry)
}

/// Loads an existing registry or creates the first founder-owned snapshot.
/// A corrupt or differently-owned existing file fails closed; it is never
/// silently replaced with a fresh empty registry.
pub fn load_or_create(
    path: &Path,
    founder: &Identity,
    at_unix: u64,
) -> Result<UserRegistry, UserRegistryError> {
    if path.exists() {
        load(path, founder)
    } else {
        let registry = UserRegistry::new(founder, at_unix);
        save(path, &registry, founder)?;
        Ok(registry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Username;
    use std::path::PathBuf;

    fn identity(name: &str) -> Identity {
        Identity::generate(Username::new(name).unwrap())
    }

    fn test_path(label: &str) -> PathBuf {
        let mut nonce = [0u8; 8];
        OsRng.fill_bytes(&mut nonce);
        std::env::temp_dir().join(format!(
            "awenet-user-registry-{label}-{}-{}.json",
            std::process::id(),
            hex::encode(nonce)
        ))
    }

    fn register(registry: &mut UserRegistry, user: &Identity, username: &str, at: u64) -> UserAccount {
        let message = UserRegistry::registration_message(username, &user.public.public_key);
        let signature = user.sign(&message);
        registry
            .register(username, &user.public.public_key, &signature, at)
            .unwrap()
    }

    #[test]
    fn founder_is_seeded_and_cannot_be_demoted_or_suspended() {
        let founder = identity("founder");
        let mut registry = UserRegistry::new(&founder, 10);
        let founder_id = founder.public.awe_id.to_hex();
        let account = registry.get(&founder_id).unwrap();
        assert_eq!(account.role, UserRole::Founder);
        assert_eq!(account.status, UserStatus::Active);

        let role_message = registry
            .role_change_message(&founder_id, UserRole::User, 11)
            .unwrap();
        assert_eq!(
            registry
                .set_role(&founder_id, UserRole::User, 11, &founder.sign(&role_message))
                .unwrap_err(),
            UserRegistryError::FounderProtected
        );
        let status_message = registry
            .status_change_message(&founder_id, UserStatus::Suspended, 11)
            .unwrap();
        assert_eq!(
            registry
                .set_status(
                    &founder_id,
                    UserStatus::Suspended,
                    11,
                    &founder.sign(&status_message)
                )
                .unwrap_err(),
            UserRegistryError::FounderProtected
        );
        assert_eq!(registry.sequence(), 1);
    }

    #[test]
    fn registration_requires_identity_proof_and_unique_username() {
        let founder = identity("founder");
        let alice = identity("alice-key");
        let mut registry = UserRegistry::new(&founder, 1);
        let account = register(&mut registry, &alice, "Alice", 2);
        assert_eq!(account.awe_id, alice.public.awe_id.to_hex());
        assert_eq!(account.role, UserRole::User);
        assert_eq!(account.status, UserStatus::Active);

        let repeated = register(&mut registry, &alice, "Alice", 3);
        assert_eq!(repeated, account);
        assert_eq!(registry.sequence(), 2);

        let bob = identity("bob-key");
        let message = UserRegistry::registration_message("alice", &bob.public.public_key);
        let signature = bob.sign(&message);
        assert_eq!(
            registry
                .register("alice", &bob.public.public_key, &signature, 4)
                .unwrap_err(),
            UserRegistryError::UsernameTaken
        );

        let invalid = [0u8; 64];
        assert_eq!(
            registry
                .register("bob", &bob.public.public_key, &invalid, 4)
                .unwrap_err(),
            UserRegistryError::InvalidSignature
        );
    }

    #[test]
    fn founder_signature_controls_role_and_status_changes() {
        let founder = identity("founder");
        let alice = identity("alice-key");
        let mut registry = UserRegistry::new(&founder, 1);
        let account = register(&mut registry, &alice, "alice", 2);

        let role_message = registry
            .role_change_message(&account.awe_id, UserRole::Admin, 3)
            .unwrap();
        let role_signature = founder.sign(&role_message);
        let promoted = registry
            .set_role(&account.awe_id, UserRole::Admin, 3, &role_signature)
            .unwrap();
        assert_eq!(promoted.role, UserRole::Admin);
        assert_eq!(registry.sequence(), 3);

        let status_message = registry
            .status_change_message(&account.awe_id, UserStatus::Suspended, 4)
            .unwrap();
        let impostor = identity("impostor");
        assert_eq!(
            registry
                .set_status(
                    &account.awe_id,
                    UserStatus::Suspended,
                    4,
                    &impostor.sign(&status_message)
                )
                .unwrap_err(),
            UserRegistryError::InvalidSignature
        );
        let status_signature = founder.sign(&status_message);
        let suspended = registry
            .set_status(
                &account.awe_id,
                UserStatus::Suspended,
                4,
                &status_signature
            )
            .unwrap();
        assert_eq!(suspended.status, UserStatus::Suspended);
        assert_eq!(registry.sequence(), 4);
    }

    #[test]
    fn signed_registry_roundtrips_and_rejects_tampering_or_wrong_founder() {
        let founder = identity("founder");
        let wrong = identity("wrong-founder");
        let alice = identity("alice-key");
        let path = test_path("roundtrip");
        let mut registry = UserRegistry::new(&founder, 1);
        register(&mut registry, &alice, "alice", 2);

        save(&path, &registry, &founder).unwrap();
        let loaded = load(&path, &founder).unwrap();
        assert_eq!(loaded, registry);
        assert_eq!(load(&path, &wrong).unwrap_err(), UserRegistryError::WrongFounder);

        let mut bytes = fs::read(&path).unwrap();
        let index = bytes.len() / 2;
        bytes[index] ^= 1;
        fs::write(&path, bytes).unwrap();
        assert!(load(&path, &founder).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn sequence_exhaustion_does_not_partially_register_a_user() {
        let founder = identity("founder");
        let alice = identity("alice-key");
        let mut registry = UserRegistry::new(&founder, 1);
        registry.sequence = u64::MAX;
        let message = UserRegistry::registration_message("alice", &alice.public.public_key);
        let signature = alice.sign(&message);

        assert_eq!(
            registry
                .register("alice", &alice.public.public_key, &signature, 2)
                .unwrap_err(),
            UserRegistryError::SequenceExhausted
        );
        assert_eq!(registry.sequence(), u64::MAX);
        assert_eq!(registry.len(), 1);
        assert!(registry.get(&alice.public.awe_id.to_hex()).is_none());
    }

    #[test]
    fn atomic_save_leaves_no_temporary_files() {
        let founder = identity("founder");
        let registry = UserRegistry::new(&founder, 1);
        let dir = test_path("atomic-dir");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("users.json");
        save(&path, &registry, &founder).unwrap();
        let entries = fs::read_dir(&dir).unwrap();
        assert_eq!(entries.count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
}
