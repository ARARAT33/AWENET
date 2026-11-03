//! Founder genesis and founder-approved device admission.
//!
//! Genesis is one-time and uses an atomic create_new file operation. A public
//! first-run page alone is not a secure founder election: the operator must
//! provision a high-entropy bootstrap secret out of band. The signed genesis
//! record pins the founder key for independent verification by every node.
//! Callers must persist device state and enforce these checks at privileged
//! protocol boundaries; this module is not a remote login server.

use crate::{
    crypto::hash,
    identity::{AweId, Identity},
};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
};
use thiserror::Error;

pub const FOUNDER_USERNAME: &str = "AWENeter";
pub const GENESIS_FORMAT: &str = "awenet-founder-genesis";
pub const GENESIS_VERSION: u16 = 1;
pub const MAX_DEVICES: usize = 64;
pub const MAX_PENDING_REQUESTS: usize = 128;
pub const MAX_AUDIT_RECORDS: usize = 4096;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FounderRules {
    pub founder_is_irremovable: bool,
    pub community_votes_can_replace_founder: bool,
    pub new_devices_require_existing_device_approval: bool,
    pub offline_device_requests_remain_pending: bool,
    pub nodes_must_verify_founder_signatures: bool,
    pub neter_delegation_requires_explicit_capability: bool,
}
impl Default for FounderRules {
    fn default() -> Self {
        Self {
            founder_is_irremovable: true,
            community_votes_can_replace_founder: false,
            new_devices_require_existing_device_approval: true,
            offline_device_requests_remain_pending: true,
            nodes_must_verify_founder_signatures: true,
            neter_delegation_requires_explicit_capability: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenesisPayload {
    pub format: String,
    pub version: u16,
    pub founder_username: String,
    pub founder_id: [u8; 32],
    pub founder_public_key: [u8; 32],
    pub network_id: [u8; 32],
    pub created_nonce: [u8; 32],
    pub rules: FounderRules,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FounderGenesis {
    pub payload: GenesisPayload,
    #[serde(with = "crate::serde_bytes_64")]
    pub signature: [u8; 64],
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FounderError {
    #[error("founder genesis has already been initialized")]
    AlreadyInitialized,
    #[error("bootstrap secret is invalid")]
    InvalidBootstrapSecret,
    #[error("founder username must be AWENeter")]
    InvalidFounderUsername,
    #[error("genesis record is invalid")]
    InvalidGenesis,
    #[error("storage operation failed: {0}")]
    Storage(String),
    #[error("device registry is full")]
    DeviceLimit,
    #[error("pending device request registry is full")]
    PendingLimit,
    #[error("device is already registered")]
    DeviceAlreadyRegistered,
    #[error("device request is unknown")]
    UnknownRequest,
    #[error("approval signer is not an approved device")]
    UnapprovedApprover,
    #[error("device approval signature is invalid")]
    InvalidApproval,
    #[error("new device has not proved possession of its private key")]
    InvalidDeviceProof,
    #[error("device is unknown or blocked")]
    DeviceBlocked,
    #[error("governance sequence is invalid")]
    InvalidSequence,
    #[error("governance signature or immutable rules are invalid")]
    InvalidGovernanceSignature,
}

#[derive(Serialize)]
struct BootstrapCheck<'a> {
    domain: &'static str,
    secret: &'a [u8],
}
pub fn bootstrap_digest(secret: &[u8]) -> [u8; 32] {
    let input = serde_json::to_vec(&BootstrapCheck {
        domain: "AWENET/FOUNDER-BOOTSTRAP/v1",
        secret,
    })
    .expect("serializing a byte slice cannot fail");
    hash(b"AWENET/BOOTSTRAP-DIGEST/v1", &input)
}

impl FounderGenesis {
    /// Atomically initializes genesis once. The expected digest must be
    /// provisioned by the operator and must never be published with the UI.
    pub fn initialize_once(
        path: &Path,
        founder: &Identity,
        bootstrap_secret: &[u8],
        expected_bootstrap_digest: [u8; 32],
        rules: FounderRules,
    ) -> Result<Self, FounderError> {
        if path.exists() {
            return Err(FounderError::AlreadyInitialized);
        }
        if founder.public.username.as_str() != FOUNDER_USERNAME {
            return Err(FounderError::InvalidFounderUsername);
        }
        if bootstrap_secret.len() < 32
            || !crate::crypto::constant_time_eq(
                &bootstrap_digest(bootstrap_secret),
                &expected_bootstrap_digest,
            )
        {
            return Err(FounderError::InvalidBootstrapSecret);
        }

        let mut nonce = [0u8; 32];
        OsRng.fill_bytes(&mut nonce);
        let payload = GenesisPayload {
            format: GENESIS_FORMAT.into(),
            version: GENESIS_VERSION,
            founder_username: FOUNDER_USERNAME.into(),
            founder_id: *founder.public.awe_id.as_bytes(),
            founder_public_key: founder.public.public_key,
            network_id: hash(b"AWENET/NETWORK-ID/v1", &nonce),
            created_nonce: nonce,
            rules,
        };
        let bytes =
            serde_json::to_vec(&payload).map_err(|e| FounderError::Storage(e.to_string()))?;
        let record = Self {
            signature: founder.sign(&bytes),
            payload,
        };
        record.verify()?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| FounderError::Storage(e.to_string()))?;
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                FounderError::AlreadyInitialized
            } else {
                FounderError::Storage(e.to_string())
            }
        })?;
        let bytes =
            serde_json::to_vec_pretty(&record).map_err(|e| FounderError::Storage(e.to_string()))?;
        if let Err(error) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
            drop(file);
            let _ = fs::remove_file(path);
            return Err(FounderError::Storage(error.to_string()));
        }
        Ok(record)
    }

    pub fn load(path: &Path) -> Result<Self, FounderError> {
        let file = OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|e| FounderError::Storage(e.to_string()))?;
        let mut bytes = Vec::new();
        file.take(1024 * 1024)
            .read_to_end(&mut bytes)
            .map_err(|e| FounderError::Storage(e.to_string()))?;
        let record: Self =
            serde_json::from_slice(&bytes).map_err(|_| FounderError::InvalidGenesis)?;
        record.verify()?;
        Ok(record)
    }

    pub fn verify(&self) -> Result<(), FounderError> {
        let p = &self.payload;
        if p.format != GENESIS_FORMAT
            || p.version != GENESIS_VERSION
            || p.founder_username != FOUNDER_USERNAME
            || p.founder_id != *AweId::from_public_key(&p.founder_public_key).as_bytes()
            || p.network_id != hash(b"AWENET/NETWORK-ID/v1", &p.created_nonce)
            || !p.rules.founder_is_irremovable
            || p.rules.community_votes_can_replace_founder
            || !p.rules.new_devices_require_existing_device_approval
            || !p.rules.nodes_must_verify_founder_signatures
            || !p.rules.offline_device_requests_remain_pending
            || !p.rules.neter_delegation_requires_explicit_capability
        {
            return Err(FounderError::InvalidGenesis);
        }
        let key = VerifyingKey::from_bytes(&p.founder_public_key)
            .map_err(|_| FounderError::InvalidGenesis)?;
        let bytes = serde_json::to_vec(p).map_err(|_| FounderError::InvalidGenesis)?;
        key.verify(&bytes, &Signature::from_bytes(&self.signature))
            .map_err(|_| FounderError::InvalidGenesis)
    }

    pub fn founder_id_hex(&self) -> String {
        hex::encode(self.payload.founder_id)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceRecord {
    pub device_id: [u8; 32],
    pub public_key: [u8; 32],
    pub label: String,
    pub approved_at: u64,
    pub blocked: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceRequest {
    pub request_id: [u8; 32],
    pub public_key: [u8; 32],
    pub label: String,
    pub requested_at: u64,
    pub nonce: [u8; 32],
    pub proof_verified: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeviceDecision {
    Approved,
    Denied,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FounderAuditRecord {
    pub sequence: u64,
    pub tick: u64,
    pub action: String,
    pub actor_device: [u8; 32],
    pub target_device: Option<[u8; 32]>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FounderDevices {
    devices: BTreeMap<String, DeviceRecord>,
    pending: BTreeMap<String, DeviceRequest>,
    audit: VecDeque<FounderAuditRecord>,
    next_audit_sequence: u64,
}
impl FounderDevices {
    pub fn new(genesis: &FounderGenesis, first_device_label: &str) -> Result<Self, FounderError> {
        genesis.verify()?;
        let id = device_id(&genesis.payload.founder_public_key);
        let mut devices = BTreeMap::new();
        devices.insert(
            hex::encode(id),
            DeviceRecord {
                device_id: id,
                public_key: genesis.payload.founder_public_key,
                label: first_device_label.chars().take(80).collect(),
                approved_at: 0,
                blocked: false,
            },
        );
        Ok(Self {
            devices,
            pending: BTreeMap::new(),
            audit: VecDeque::new(),
            next_audit_sequence: 1,
        })
    }
    pub fn approved_devices(&self) -> Vec<&DeviceRecord> {
        self.devices.values().filter(|d| !d.blocked).collect()
    }
    pub fn request_device(
        &mut self,
        public_key: [u8; 32],
        label: &str,
        tick: u64,
    ) -> Result<DeviceRequest, FounderError> {
        let id = device_id(&public_key);
        if self.devices.contains_key(&hex::encode(id)) {
            return Err(FounderError::DeviceAlreadyRegistered);
        }
        if self.pending.len() >= MAX_PENDING_REQUESTS {
            return Err(FounderError::PendingLimit);
        }
        let mut nonce = [0u8; 32];
        OsRng.fill_bytes(&mut nonce);
        let request_id = hash(
            b"AWENET/DEVICE-REQUEST/v1",
            &[id.as_slice(), nonce.as_slice()].concat(),
        );
        let request = DeviceRequest {
            request_id,
            public_key,
            label: label.chars().take(80).collect(),
            requested_at: tick,
            nonce,
            proof_verified: false,
        };
        self.pending
            .insert(hex::encode(request_id), request.clone());
        Ok(request)
    }
    /// The new device must first prove possession of the private key bound to
    /// its submitted public key. This prevents an attacker from registering a
    /// victim's public key and asking the founder to approve it.
    pub fn prove_device_request(
        &mut self,
        request_id: [u8; 32],
        proof_signature: [u8; 64],
    ) -> Result<(), FounderError> {
        let request = self
            .pending
            .get_mut(&hex::encode(request_id))
            .ok_or(FounderError::UnknownRequest)?;
        let key = VerifyingKey::from_bytes(&request.public_key)
            .map_err(|_| FounderError::InvalidDeviceProof)?;
        key.verify(
            &device_proof_message(request),
            &Signature::from_bytes(&proof_signature),
        )
        .map_err(|_| FounderError::InvalidDeviceProof)?;
        request.proof_verified = true;
        Ok(())
    }

    /// The signed approval statement is bound to the exact request and decision.
    /// Only a currently approved device may approve or deny another device.
    pub fn decide_request(
        &mut self,
        request_id: [u8; 32],
        approver_id: [u8; 32],
        decision: DeviceDecision,
        signature: [u8; 64],
        tick: u64,
    ) -> Result<(), FounderError> {
        let request_key = hex::encode(request_id);
        let request = self
            .pending
            .get(&request_key)
            .ok_or(FounderError::UnknownRequest)?;
        if !request.proof_verified {
            return Err(FounderError::InvalidDeviceProof);
        }
        let approver = self
            .devices
            .get(&hex::encode(approver_id))
            .filter(|d| !d.blocked)
            .ok_or(FounderError::UnapprovedApprover)?;
        let key = VerifyingKey::from_bytes(&approver.public_key)
            .map_err(|_| FounderError::InvalidApproval)?;
        let request = self
            .pending
            .get(&request_key)
            .ok_or(FounderError::UnknownRequest)?;
        key.verify(
            &approval_message(request, &decision),
            &Signature::from_bytes(&signature),
        )
        .map_err(|_| FounderError::InvalidApproval)?;
        if matches!(decision, DeviceDecision::Approved) && self.devices.len() >= MAX_DEVICES {
            return Err(FounderError::DeviceLimit);
        }
        let request = self
            .pending
            .remove(&request_key)
            .ok_or(FounderError::UnknownRequest)?;
        match decision {
            DeviceDecision::Approved => {
                let id = device_id(&request.public_key);
                self.devices.insert(
                    hex::encode(id),
                    DeviceRecord {
                        device_id: id,
                        public_key: request.public_key,
                        label: request.label,
                        approved_at: tick,
                        blocked: false,
                    },
                );
                self.push_audit(tick, "device_approved", approver_id, Some(id));
            }
            DeviceDecision::Denied => {
                self.push_audit(
                    tick,
                    "device_denied",
                    approver_id,
                    Some(device_id(&request.public_key)),
                );
            }
        }
        Ok(())
    }
    pub fn block_device(
        &mut self,
        device: [u8; 32],
        blocked: bool,
        actor: [u8; 32],
        tick: u64,
        signature: [u8; 64],
    ) -> Result<(), FounderError> {
        let actor_key = self
            .devices
            .get(&hex::encode(actor))
            .filter(|d| !d.blocked)
            .map(|d| d.public_key)
            .ok_or(FounderError::UnapprovedApprover)?;
        let key =
            VerifyingKey::from_bytes(&actor_key).map_err(|_| FounderError::InvalidApproval)?;
        key.verify(
            &device_control_message(device, blocked, tick),
            &Signature::from_bytes(&signature),
        )
        .map_err(|_| FounderError::InvalidApproval)?;
        let record = self
            .devices
            .get_mut(&hex::encode(device))
            .ok_or(FounderError::DeviceBlocked)?;
        record.blocked = blocked;
        self.push_audit(
            tick,
            if blocked {
                "device_blocked"
            } else {
                "device_unblocked"
            },
            actor,
            Some(device),
        );
        Ok(())
    }
    pub fn device_was_denied(&self, public_key: &[u8; 32]) -> bool {
        let target = device_id(public_key);
        self.audit.iter().any(|entry| {
            entry.action == "device_denied" && entry.target_device == Some(target)
        })
    }

    pub fn audit(&self) -> impl Iterator<Item = &FounderAuditRecord> {
        self.audit.iter()
    }
    fn push_audit(&mut self, tick: u64, action: &str, actor: [u8; 32], target: Option<[u8; 32]>) {
        if self.audit.len() == MAX_AUDIT_RECORDS {
            self.audit.pop_front();
        }
        self.audit.push_back(FounderAuditRecord {
            sequence: self.next_audit_sequence,
            tick,
            action: action.into(),
            actor_device: actor,
            target_device: target,
        });
        self.next_audit_sequence = self.next_audit_sequence.saturating_add(1);
    }
}
#[derive(Serialize)]
struct ApprovalStatement<'a> {
    domain: &'static str,
    request: &'a DeviceRequest,
    decision: &'a DeviceDecision,
}
pub fn approval_message(request: &DeviceRequest, decision: &DeviceDecision) -> Vec<u8> {
    serde_json::to_vec(&ApprovalStatement {
        domain: "AWENET/DEVICE-APPROVAL/v1",
        request,
        decision,
    })
    .expect("approval statement serialization is infallible")
}
#[derive(Serialize)]
struct DeviceProofStatement<'a> {
    domain: &'static str,
    request_id: [u8; 32],
    public_key: [u8; 32],
    label: &'a str,
    requested_at: u64,
    nonce: [u8; 32],
}
pub fn device_proof_message(request: &DeviceRequest) -> Vec<u8> {
    serde_json::to_vec(&DeviceProofStatement {
        domain: "AWENET/DEVICE-PROOF/v1",
        request_id: request.request_id,
        public_key: request.public_key,
        label: &request.label,
        requested_at: request.requested_at,
        nonce: request.nonce,
    })
    .expect("device proof statement serialization is infallible")
}
pub fn device_control_message(device: [u8; 32], blocked: bool, tick: u64) -> Vec<u8> {
    #[derive(Serialize)]
    struct DeviceControlStatement {
        domain: &'static str,
        device: [u8; 32],
        blocked: bool,
        tick: u64,
    }
    serde_json::to_vec(&DeviceControlStatement {
        domain: "AWENET/DEVICE-CONTROL/v1",
        device,
        blocked,
        tick,
    })
    .expect("device control statement serialization is infallible")
}
pub fn device_id(public_key: &[u8; 32]) -> [u8; 32] {
    hash(b"AWENET/DEVICE-ID/v1", public_key)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignedGovernanceUpdate {
    pub sequence: u64,
    pub previous_hash: [u8; 32],
    pub rules: FounderRules,
    #[serde(with = "crate::serde_bytes_64")]
    pub signature: [u8; 64],
}
#[derive(Serialize)]
struct GovernanceStatement<'a> {
    domain: &'static str,
    sequence: u64,
    previous_hash: [u8; 32],
    rules: &'a FounderRules,
}
impl SignedGovernanceUpdate {
    pub fn sign(
        founder: &Identity,
        sequence: u64,
        previous_hash: [u8; 32],
        rules: FounderRules,
    ) -> Result<Self, FounderError> {
        if founder.public.username.as_str() != FOUNDER_USERNAME || sequence == 0 {
            return Err(FounderError::InvalidSequence);
        }
        let statement = GovernanceStatement {
            domain: "AWENET/GOVERNANCE-UPDATE/v1",
            sequence,
            previous_hash,
            rules: &rules,
        };
        let bytes =
            serde_json::to_vec(&statement).map_err(|_| FounderError::InvalidGovernanceSignature)?;
        Ok(Self {
            sequence,
            previous_hash,
            rules,
            signature: founder.sign(&bytes),
        })
    }
    pub fn verify(
        &self,
        genesis: &FounderGenesis,
        expected_sequence: u64,
        expected_previous_hash: [u8; 32],
    ) -> Result<[u8; 32], FounderError> {
        genesis.verify()?;
        if self.sequence != expected_sequence || self.previous_hash != expected_previous_hash {
            return Err(FounderError::InvalidSequence);
        }
        if !self.rules.founder_is_irremovable
            || self.rules.community_votes_can_replace_founder
            || !self.rules.new_devices_require_existing_device_approval
            || !self.rules.offline_device_requests_remain_pending
            || !self.rules.nodes_must_verify_founder_signatures
            || !self.rules.neter_delegation_requires_explicit_capability
        {
            return Err(FounderError::InvalidGovernanceSignature);
        }
        let statement = GovernanceStatement {
            domain: "AWENET/GOVERNANCE-UPDATE/v1",
            sequence: self.sequence,
            previous_hash: self.previous_hash,
            rules: &self.rules,
        };
        let bytes =
            serde_json::to_vec(&statement).map_err(|_| FounderError::InvalidGovernanceSignature)?;
        let key = VerifyingKey::from_bytes(&genesis.payload.founder_public_key)
            .map_err(|_| FounderError::InvalidGovernanceSignature)?;
        key.verify(&bytes, &Signature::from_bytes(&self.signature))
            .map_err(|_| FounderError::InvalidGovernanceSignature)?;
        let encoded =
            serde_json::to_vec(self).map_err(|_| FounderError::InvalidGovernanceSignature)?;
        Ok(hash(b"AWENET/GOVERNANCE-UPDATE-HASH/v1", &encoded))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Username;
    fn founder() -> Identity {
        Identity::generate(Username::new(FOUNDER_USERNAME).unwrap())
    }
    fn bootstrap() -> (Vec<u8>, [u8; 32]) {
        let secret = vec![0xA5; 48];
        let digest = bootstrap_digest(&secret);
        (secret, digest)
    }
    fn path(label: &str) -> std::path::PathBuf {
        let mut n = [0u8; 8];
        OsRng.fill_bytes(&mut n);
        std::env::temp_dir().join(format!("awenet-{label}-{}.json", hex::encode(n)))
    }
    #[test]
    fn genesis_is_signed_persisted_and_one_time() {
        let p = path("genesis");
        let id = founder();
        let (s, d) = bootstrap();
        let g = FounderGenesis::initialize_once(&p, &id, &s, d, FounderRules::default()).unwrap();
        g.verify().unwrap();
        assert_eq!(FounderGenesis::load(&p).unwrap(), g);
        assert_eq!(
            FounderGenesis::initialize_once(&p, &id, &s, d, FounderRules::default()).unwrap_err(),
            FounderError::AlreadyInitialized
        );
        fs::remove_file(p).unwrap();
    }
    #[test]
    fn bootstrap_and_username_are_required() {
        let p = path("bootstrap");
        let id = founder();
        let (s, d) = bootstrap();
        assert_eq!(
            FounderGenesis::initialize_once(&p, &id, b"short", d, FounderRules::default())
                .unwrap_err(),
            FounderError::InvalidBootstrapSecret
        );
        let wrong = Identity::generate(Username::new("someone").unwrap());
        assert_eq!(
            FounderGenesis::initialize_once(&p, &wrong, &s, d, FounderRules::default())
                .unwrap_err(),
            FounderError::InvalidFounderUsername
        );
        assert!(!p.exists());
    }
    #[test]
    fn tampered_genesis_is_rejected() {
        let p = path("tamper");
        let id = founder();
        let (s, d) = bootstrap();
        let mut g =
            FounderGenesis::initialize_once(&p, &id, &s, d, FounderRules::default()).unwrap();
        g.payload.rules.community_votes_can_replace_founder = true;
        assert_eq!(g.verify(), Err(FounderError::InvalidGenesis));
        fs::remove_file(p).unwrap();
    }
    #[test]
    fn second_device_requires_existing_device_signature() {
        let p = path("device");
        let id = founder();
        let (s, d) = bootstrap();
        let g = FounderGenesis::initialize_once(&p, &id, &s, d, FounderRules::default()).unwrap();
        let mut devices = FounderDevices::new(&g, "Founder phone").unwrap();
        let second = Identity::generate(Username::new("laptop").unwrap());
        let req = devices
            .request_device(second.public.public_key, "Laptop", 10)
            .unwrap();
        let proof = second.sign(&device_proof_message(&req));
        devices.prove_device_request(req.request_id, proof).unwrap();
        let sig = id.sign(&approval_message(&req, &DeviceDecision::Approved));
        devices
            .decide_request(
                req.request_id,
                device_id(&id.public.public_key),
                DeviceDecision::Approved,
                sig,
                11,
            )
            .unwrap();
        assert_eq!(devices.approved_devices().len(), 2);
        assert_eq!(devices.audit().count(), 1);
        fs::remove_file(p).unwrap();
    }
    #[test]
    fn forged_approval_fails_without_consuming_request() {
        let p = path("forged");
        let id = founder();
        let (s, d) = bootstrap();
        let g = FounderGenesis::initialize_once(&p, &id, &s, d, FounderRules::default()).unwrap();
        let mut devices = FounderDevices::new(&g, "Founder phone").unwrap();
        let second = Identity::generate(Username::new("laptop").unwrap());
        let attacker = Identity::generate(Username::new("attacker").unwrap());
        let req = devices
            .request_device(second.public.public_key, "Laptop", 10)
            .unwrap();
        let proof = second.sign(&device_proof_message(&req));
        devices.prove_device_request(req.request_id, proof).unwrap();
        let sig = attacker.sign(&approval_message(&req, &DeviceDecision::Approved));
        assert_eq!(
            devices.decide_request(
                req.request_id,
                device_id(&id.public.public_key),
                DeviceDecision::Approved,
                sig,
                11
            ),
            Err(FounderError::InvalidApproval)
        );
        assert_eq!(devices.pending.len(), 1);
        let target = devices.approved_devices()[1].device_id;
        let block_signature = id.sign(&device_control_message(target, true, 12));
        devices
            .block_device(
                target,
                true,
                device_id(&id.public.public_key),
                12,
                block_signature,
            )
            .unwrap();
        assert!(devices
            .approved_devices()
            .iter()
            .all(|d| d.device_id != target));
        fs::remove_file(p).unwrap();
    }
    #[test]
    fn governance_updates_are_signed_and_chained() {
        let p = path("rules");
        let id = founder();
        let (s, d) = bootstrap();
        let g = FounderGenesis::initialize_once(&p, &id, &s, d, FounderRules::default()).unwrap();
        let u = SignedGovernanceUpdate::sign(&id, 1, [0; 32], FounderRules::default()).unwrap();
        let h = u.verify(&g, 1, [0; 32]).unwrap();
        assert_ne!(h, [0; 32]);
        assert_eq!(u.verify(&g, 2, [0; 32]), Err(FounderError::InvalidSequence));
        fs::remove_file(p).unwrap();
    }
}
