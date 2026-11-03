//! Crash-safe, founder-signed snapshots for the Founder Authority runtime.
//!
//! Snapshot files are immutable and atomically published without overwriting an existing sequence number. Each snapshot
//! is signed by the founder authority key; loading verifies the signature and
//! falls back to the newest earlier valid snapshot if the latest one is damaged.
//! The caller must invoke save() after every successful state mutation.

use crate::{
    founder::{FounderDevices, FounderError, FounderGenesis, FounderRules},
    founder_admin::{FounderEmergencyLock, NeterRegistry},
    founder_auth::FounderLogin,
    identity::Identity,
};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use thiserror::Error;

const SNAPSHOT_DOMAIN: &[u8] = b"AWENET/FOUNDER-STATE-SNAPSHOT/v2\0";
const MAX_SNAPSHOT_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum FounderStateError {
    #[error("founder state is already initialized")]
    AlreadyInitialized,
    #[error("founder state is not initialized")]
    NotInitialized,
    #[error("founder state snapshot is missing or invalid")]
    InvalidSnapshot,
    #[error("founder signing key does not match the pinned Genesis")]
    WrongFounderKey,
    #[error("founder state storage failed: {0}")]
    Storage(String),
    #[error(transparent)]
    Genesis(#[from] FounderError),
    #[error("founder authorization rejected: {0}")]
    Authorization(String),
    #[error("Neter registry could not be initialized")]
    NeterRegistry,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FounderSnapshotPayload {
    sequence: u64,
    devices: FounderDevices,
    neters: NeterRegistry,
    emergency: FounderEmergencyLock,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SignedFounderSnapshot {
    payload: FounderSnapshotPayload,
    #[serde(with = "crate::serde_bytes_64")]
    signature: [u8; 64],
}

/// Live founder state. Private signing keys are never stored in this structure
/// or in its snapshots; they remain in the founder's local credential vault.
pub struct FounderPersistentState {
    pub genesis: FounderGenesis,
    pub devices: FounderDevices,
    pub login: FounderLogin,
    pub neters: NeterRegistry,
    pub emergency: FounderEmergencyLock,
    sequence: u64,
    directory: PathBuf,
}

impl FounderPersistentState {
    pub fn initialize(
        directory: &Path,
        founder: &Identity,
        initial_device_public_key: [u8; 32],
        bootstrap_secret: &[u8],
        expected_bootstrap_digest: [u8; 32],
        device_label: &str,
        rules: FounderRules,
    ) -> Result<Self, FounderStateError> {
        fs::create_dir_all(directory).map_err(storage_error)?;
        let genesis_path = directory.join("genesis.json");
        if genesis_path.exists() {
            return Err(FounderStateError::AlreadyInitialized);
        }
        let genesis = FounderGenesis::initialize_once(
            &genesis_path,
            founder,
            initial_device_public_key,
            bootstrap_secret,
            expected_bootstrap_digest,
            rules,
        )?;
        let devices = FounderDevices::new(&genesis, device_label)?;
        let neters = NeterRegistry::new(&genesis).map_err(|_| FounderStateError::NeterRegistry)?;
        let mut state = Self {
            genesis,
            devices,
            login: FounderLogin::new(),
            neters,
            emergency: FounderEmergencyLock::default(),
            sequence: 0,
            directory: directory.to_path_buf(),
        };
        state.save(founder)?;
        Ok(state)
    }

    pub fn load(directory: &Path) -> Result<Self, FounderStateError> {
        let genesis_path = directory.join("genesis.json");
        if !genesis_path.exists() {
            return Err(FounderStateError::NotInitialized);
        }
        let genesis = FounderGenesis::load(&genesis_path)?;
        let mut candidates = Vec::new();
        for entry in fs::read_dir(directory).map_err(storage_error)? {
            let entry = entry.map_err(storage_error)?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(raw) = name
                .strip_prefix("snapshot-")
                .and_then(|v| v.strip_suffix(".json"))
            {
                if let Ok(sequence) = raw.parse::<u64>() {
                    candidates.push((sequence, entry.path()));
                }
            }
        }
        candidates.sort_by(|a, b| b.0.cmp(&a.0));
        for (sequence, path) in candidates {
            if let Ok(snapshot) = read_snapshot(&path) {
                if snapshot.payload.sequence != sequence || !verify_snapshot(&genesis, &snapshot) {
                    continue;
                }
                return Ok(Self {
                    genesis,
                    devices: snapshot.payload.devices,
                    login: FounderLogin::new(),
                    neters: snapshot.payload.neters,
                    emergency: snapshot.payload.emergency,
                    sequence,
                    directory: directory.to_path_buf(),
                });
            }
        }
        Err(FounderStateError::InvalidSnapshot)
    }

    /// Persist a signed immutable snapshot after a successful mutation.
    pub fn save(&mut self, founder: &Identity) -> Result<u64, FounderStateError> {
        if founder.public.public_key != self.genesis.payload.founder_public_key {
            return Err(FounderStateError::WrongFounderKey);
        }
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| FounderStateError::Storage("snapshot sequence exhausted".into()))?;
        let payload = FounderSnapshotPayload {
            sequence,
            devices: self.devices.clone(),
            neters: self.neters.clone(),
            emergency: self.emergency.clone(),
        };
        let payload_bytes = serde_json::to_vec(&payload).map_err(storage_error)?;
        let mut signing_bytes = SNAPSHOT_DOMAIN.to_vec();
        signing_bytes.extend_from_slice(&payload_bytes);
        let snapshot = SignedFounderSnapshot {
            payload,
            signature: founder.sign(&signing_bytes),
        };
        let bytes = serde_json::to_vec(&snapshot).map_err(storage_error)?;
        let final_path = self.directory.join(format!("snapshot-{sequence:020}.json"));
        write_new_snapshot(&final_path, &bytes)?;
        self.sequence = sequence;
        Ok(sequence)
    }

    /// Validate a founder session and the account emergency lock in one shared guard.
    /// All founder-only handlers must call this before performing protected work.
    pub fn authorize_founder_session(
        &self,
        token: &[u8; 32],
        now: u64,
    ) -> Result<[u8; 32], FounderStateError> {
        self.emergency
            .ensure_login_allowed()
            .map_err(|error| FounderStateError::Authorization(error.to_string()))?;
        self.login
            .validate_session(&self.devices, token, now)
            .map_err(|error| FounderStateError::Authorization(error.to_string()))
    }

    /// Governance operations must pass both the session guard and governance lock.
    pub fn authorize_governance_session(
        &self,
        token: &[u8; 32],
        now: u64,
    ) -> Result<[u8; 32], FounderStateError> {
        let device_id = self.authorize_founder_session(token, now)?;
        self.emergency
            .ensure_governance_allowed()
            .map_err(|error| FounderStateError::Authorization(error.to_string()))?;
        Ok(device_id)
    }

    pub fn snapshot_sequence(&self) -> u64 {
        self.sequence
    }

    pub fn founder_id(&self) -> String {
        self.genesis.founder_id_hex()
    }
}

fn read_snapshot(path: &Path) -> Result<SignedFounderSnapshot, FounderStateError> {
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(storage_error)?;
    let mut bytes = Vec::new();
    file.take(MAX_SNAPSHOT_BYTES)
        .read_to_end(&mut bytes)
        .map_err(storage_error)?;
    if bytes.len() as u64 >= MAX_SNAPSHOT_BYTES {
        return Err(FounderStateError::InvalidSnapshot);
    }
    serde_json::from_slice(&bytes).map_err(|_| FounderStateError::InvalidSnapshot)
}

fn verify_snapshot(genesis: &FounderGenesis, snapshot: &SignedFounderSnapshot) -> bool {
    let Ok(payload_bytes) = serde_json::to_vec(&snapshot.payload) else {
        return false;
    };
    let Ok(key) = VerifyingKey::from_bytes(&genesis.payload.founder_public_key) else {
        return false;
    };
    let mut signing_bytes = SNAPSHOT_DOMAIN.to_vec();
    signing_bytes.extend_from_slice(&payload_bytes);
    key.verify(&signing_bytes, &Signature::from_bytes(&snapshot.signature))
        .is_ok()
}

fn write_new_snapshot(path: &Path, bytes: &[u8]) -> Result<(), FounderStateError> {
    let parent = path
        .parent()
        .ok_or_else(|| FounderStateError::Storage("snapshot directory missing".into()))?;
    let mut nonce = [0u8; 16];
    OsRng.fill_bytes(&mut nonce);
    let temp_path = parent.join(format!(".snapshot-{}.tmp", hex::encode(nonce)));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
        .map_err(storage_error)?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(&temp_path);
        return Err(storage_error(error));
    }
    drop(file);
    // hard_link is an atomic no-overwrite publication on supported local
    // filesystems. A stale concurrent writer cannot replace an existing
    // sequence number with an older snapshot.
    if let Err(error) = fs::hard_link(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(storage_error(error));
    }
    let _ = fs::remove_file(&temp_path);
    #[cfg(unix)]
    if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn storage_error(error: impl std::fmt::Display) -> FounderStateError {
    FounderStateError::Storage(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{founder_auth::login_message, identity::Username};

    fn temporary_directory() -> PathBuf {
        let mut random = [0u8; 16];
        OsRng.fill_bytes(&mut random);
        std::env::temp_dir().join(format!("awenet-founder-state-{}", hex::encode(random)))
    }

    #[test]
    fn shared_founder_authorization_guard_accepts_only_active_device_sessions() {
        let directory = temporary_directory();
        let founder = Identity::generate(Username::new("AWENeter").unwrap());
        let device = Identity::generate(Username::new("founder-device").unwrap());
        let secret = [0x4Du8; 48];
        let digest = crate::founder::bootstrap_digest(&secret);
        let mut state = FounderPersistentState::initialize(
            &directory,
            &founder,
            device.public.public_key,
            &secret,
            digest,
            "Founder device",
            FounderRules::default(),
        ).unwrap();
        let device_id = state.devices.approved_devices()[0].device_id;
        let challenge = state.login.request_challenge_guarded(
            &state.devices,
            &state.emergency,
            device_id,
            10,
            60,
        ).unwrap();
        let signature = device.sign(&login_message(&challenge));
        let session = state.login.complete_login(
            &state.devices,
            challenge.challenge_id,
            signature,
            11,
            300,
        ).unwrap();
        assert_eq!(state.authorize_founder_session(&session.token, 12).unwrap(), device_id);
        state.login.revoke_session(&session.token).unwrap();
        assert!(state.authorize_founder_session(&session.token, 13).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn founder_state_round_trips_with_signed_snapshot() {
        let directory = temporary_directory();
        let founder = Identity::generate(Username::new("AWENeter").unwrap());
        let device = Identity::generate(Username::new("founder-device").unwrap());
        let secret = [0x5Au8; 48];
        let digest = crate::founder::bootstrap_digest(&secret);
        let mut state = FounderPersistentState::initialize(
            &directory,
            &founder,
            device.public.public_key,
            &secret,
            digest,
            "Founder device",
            FounderRules::default(),
        )
        .unwrap();
        assert_eq!(state.snapshot_sequence(), 1);
        state.login.revoke_all_sessions();
        assert_eq!(state.save(&founder).unwrap(), 2);
        let loaded = FounderPersistentState::load(&directory).unwrap();
        assert_eq!(loaded.snapshot_sequence(), 2);
        assert_eq!(loaded.founder_id(), founder.public.awe_id.to_hex());
        assert_eq!(loaded.devices.approved_devices().len(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn stale_writer_cannot_overwrite_an_existing_snapshot_sequence() {
        let directory = temporary_directory();
        let founder = Identity::generate(Username::new("AWENeter").unwrap());
        let device = Identity::generate(Username::new("founder-device").unwrap());
        let secret = [0x3Cu8; 48];
        let digest = crate::founder::bootstrap_digest(&secret);
        let mut current = FounderPersistentState::initialize(
            &directory,
            &founder,
            device.public.public_key,
            &secret,
            digest,
            "Founder device",
            FounderRules::default(),
        )
        .unwrap();
        let mut stale = FounderPersistentState::load(&directory).unwrap();
        current.save(&founder).unwrap();
        assert!(stale.save(&founder).is_err());
        assert_eq!(
            FounderPersistentState::load(&directory)
                .unwrap()
                .snapshot_sequence(),
            2
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn damaged_latest_snapshot_falls_back_to_previous_signed_snapshot() {
        let directory = temporary_directory();
        let founder = Identity::generate(Username::new("AWENeter").unwrap());
        let device = Identity::generate(Username::new("founder-device").unwrap());
        let secret = [0x6Bu8; 48];
        let digest = crate::founder::bootstrap_digest(&secret);
        let mut state = FounderPersistentState::initialize(
            &directory,
            &founder,
            device.public.public_key,
            &secret,
            digest,
            "Founder device",
            FounderRules::default(),
        )
        .unwrap();
        state.save(&founder).unwrap();
        fs::write(
            directory.join("snapshot-00000000000000000002.json"),
            b"broken",
        )
        .unwrap();
        let loaded = FounderPersistentState::load(&directory).unwrap();
        assert_eq!(loaded.snapshot_sequence(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}
