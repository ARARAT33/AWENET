//! One-time, cryptographically signed AWENET founder genesis.
//!
//! This module separates a local first-write guarantee from network-wide
//! consensus. The bootstrap token must be high entropy and its digest pinned
//! by a trusted release/configuration channel.
use crate::identity::{AweId, Identity, Username};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};
use thiserror::Error;

pub const FOUNDER_USERNAME: &str = "AWENeter";
pub const GENESIS_VERSION: u16 = 1;
const BOOTSTRAP_DOMAIN: &[u8] = b"AWENET/FOUNDER-BOOTSTRAP/v1:";
const GENESIS_DOMAIN: &[u8] = b"AWENET/FOUNDER-GENESIS/v1\0";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FounderGenesisPayload {
    pub version: u16,
    pub network_id: [u8; 32],
    pub founder_id: String,
    pub founder_public_key: [u8; 32],
    pub founder_username: String,
    pub protocol_version: u16,
    pub authority_epoch: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FounderGenesis {
    pub payload: FounderGenesisPayload,
    #[serde(with = "crate::serde_bytes_64")]
    pub signature: [u8; 64],
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FounderGenesisError {
    #[error("founder username must be AWENeter")]
    InvalidFounderUsername,
    #[error("bootstrap token must contain at least 32 bytes")]
    WeakBootstrapToken,
    #[error("bootstrap token does not match the trusted bootstrap commitment")]
    InvalidBootstrapToken,
    #[error("a founder genesis already exists; a second founder is forbidden")]
    AlreadyInitialized,
    #[error("founder genesis file is invalid or corrupted")]
    InvalidGenesis,
    #[error("founder genesis I/O failed: {0}")]
    Io(String),
    #[error("founder genesis serialization failed")]
    Serialization,
}

impl FounderGenesis {
    /// Hash a 256-bit-or-stronger bootstrap token for pinning in a trusted
    /// release/configuration. Never publish the token itself.
    pub fn bootstrap_commitment(token: &[u8]) -> Result<[u8; 32], FounderGenesisError> {
        if token.len() < 32 {
            return Err(FounderGenesisError::WeakBootstrapToken);
        }
        let mut hasher = Sha256::new();
        hasher.update(BOOTSTRAP_DOMAIN);
        hasher.update(token);
        Ok(hasher.finalize().into())
    }

    /// Create and persist the only genesis record. create_new is atomic for
    /// competing processes on the same filesystem; distributed deployments
    /// must additionally pin the bootstrap commitment and genesis trust root.
    pub fn initialize_once(
        path: &Path,
        founder: &Identity,
        network_id: [u8; 32],
        bootstrap_token: &[u8],
        trusted_bootstrap_commitment: &[u8; 32],
    ) -> Result<Self, FounderGenesisError> {
        if founder.public.username.as_str() != FOUNDER_USERNAME {
            return Err(FounderGenesisError::InvalidFounderUsername);
        }
        let actual = Self::bootstrap_commitment(bootstrap_token)?;
        if !crate::crypto::constant_time_eq(&actual, trusted_bootstrap_commitment) {
            return Err(FounderGenesisError::InvalidBootstrapToken);
        }

        let payload = FounderGenesisPayload {
            version: GENESIS_VERSION,
            network_id,
            founder_id: founder.public.awe_id.to_hex(),
            founder_public_key: founder.public.public_key,
            founder_username: FOUNDER_USERNAME.to_string(),
            protocol_version: 1,
            authority_epoch: 0,
        };
        let mut message = GENESIS_DOMAIN.to_vec();
        message
            .extend(serde_json::to_vec(&payload).map_err(|_| FounderGenesisError::Serialization)?);
        let record = Self {
            payload,
            signature: founder.sign(&message),
        };
        record.verify()?;

        let bytes =
            serde_json::to_vec_pretty(&record).map_err(|_| FounderGenesisError::Serialization)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = match options.open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(FounderGenesisError::AlreadyInitialized)
            }
            Err(error) => return Err(FounderGenesisError::Io(error.to_string())),
        };
        if let Err(error) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
            drop(file);
            let _ = fs::remove_file(path);
            return Err(FounderGenesisError::Io(error.to_string()));
        }
        Ok(record)
    }

    pub fn load_and_verify(path: &Path) -> Result<Self, FounderGenesisError> {
        let bytes = fs::read(path).map_err(|error| FounderGenesisError::Io(error.to_string()))?;
        let record: Self =
            serde_json::from_slice(&bytes).map_err(|_| FounderGenesisError::InvalidGenesis)?;
        record.verify()?;
        Ok(record)
    }

    pub fn verify(&self) -> Result<(), FounderGenesisError> {
        let payload = &self.payload;
        if payload.version != GENESIS_VERSION
            || payload.protocol_version == 0
            || payload.founder_username != FOUNDER_USERNAME
            || payload.founder_id != AweId::from_public_key(&payload.founder_public_key).to_hex()
        {
            return Err(FounderGenesisError::InvalidGenesis);
        }
        let mut message = GENESIS_DOMAIN.to_vec();
        message
            .extend(serde_json::to_vec(payload).map_err(|_| FounderGenesisError::Serialization)?);
        if !Identity::verify(&payload.founder_public_key, &message, &self.signature) {
            return Err(FounderGenesisError::InvalidGenesis);
        }
        Ok(())
    }

    pub fn founder_username() -> Result<Username, FounderGenesisError> {
        Username::new(FOUNDER_USERNAME).map_err(|_| FounderGenesisError::InvalidFounderUsername)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "awenet-{label}-{}-{nonce}.json",
            std::process::id()
        ))
    }
    fn founder() -> Identity {
        Identity::generate(Username::new(FOUNDER_USERNAME).unwrap())
    }

    #[test]
    fn genesis_is_signed_and_round_trips() {
        let path = temp_path("genesis-roundtrip");
        let founder = founder();
        let token = [7u8; 32];
        let commitment = FounderGenesis::bootstrap_commitment(&token).unwrap();
        let created =
            FounderGenesis::initialize_once(&path, &founder, [9u8; 32], &token, &commitment)
                .unwrap();
        assert_eq!(created.payload.founder_id, founder.public.awe_id.to_hex());
        assert_eq!(created, FounderGenesis::load_and_verify(&path).unwrap());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn genesis_can_only_be_created_once_on_a_filesystem() {
        let path = temp_path("genesis-once");
        let founder = founder();
        let token = [8u8; 32];
        let commitment = FounderGenesis::bootstrap_commitment(&token).unwrap();
        FounderGenesis::initialize_once(&path, &founder, [1; 32], &token, &commitment).unwrap();
        assert_eq!(
            FounderGenesis::initialize_once(&path, &founder, [2; 32], &token, &commitment),
            Err(FounderGenesisError::AlreadyInitialized)
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn invalid_bootstrap_token_is_rejected_before_file_creation() {
        let path = temp_path("genesis-token");
        let founder = founder();
        let token = [3u8; 32];
        let wrong = [4u8; 32];
        let commitment = FounderGenesis::bootstrap_commitment(&wrong).unwrap();
        assert_eq!(
            FounderGenesis::initialize_once(&path, &founder, [1; 32], &token, &commitment),
            Err(FounderGenesisError::InvalidBootstrapToken)
        );
        assert!(!path.exists());
    }

    #[test]
    fn founder_username_is_fixed() {
        let path = temp_path("genesis-username");
        let impostor = Identity::generate(Username::new("someone_else").unwrap());
        let token = [5u8; 32];
        let commitment = FounderGenesis::bootstrap_commitment(&token).unwrap();
        assert_eq!(
            FounderGenesis::initialize_once(&path, &impostor, [1; 32], &token, &commitment),
            Err(FounderGenesisError::InvalidFounderUsername)
        );
        assert!(!path.exists());
    }

    #[test]
    fn tampered_genesis_is_rejected() {
        let path = temp_path("genesis-tamper");
        let founder = founder();
        let token = [6u8; 32];
        let commitment = FounderGenesis::bootstrap_commitment(&token).unwrap();
        let mut record =
            FounderGenesis::initialize_once(&path, &founder, [1; 32], &token, &commitment).unwrap();
        record.payload.network_id = [2; 32];
        assert_eq!(record.verify(), Err(FounderGenesisError::InvalidGenesis));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn weak_bootstrap_token_is_rejected() {
        assert_eq!(
            FounderGenesis::bootstrap_commitment(b"short"),
            Err(FounderGenesisError::WeakBootstrapToken)
        );
    }
}
