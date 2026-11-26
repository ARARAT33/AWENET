//! Founder-authorized, durable network topology control plane.
//!
//! This module wraps the topology model in a monotonic, signed mutation
//! protocol and a founder-signed snapshot store. Mutations are applied to a
//! candidate copy, validated, durably written, and only then published to
//! the live in-memory state. It is a local control-plane primitive; HTTP
//! authorization and cross-node replication are separate integration work.

use crate::identity::{AweId, Identity};
use crate::network_topology::{
    AweNet, CentreGroup, CentreLinkMode, DataCentre, DataGroup, Node, PeerMetrics,
};
use crate::permissions::Capability;
use crate::user_registry::UserRegistry;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::Read,
    path::Path,
};
use thiserror::Error;

pub const NETWORK_CONTROL_VERSION: u16 = 1;
pub const NETWORK_CONTROL_CHECKPOINT_VERSION: u16 = 1;
pub const MAX_NETWORK_CONTROL_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_NETWORK_CONTROL_CHECKPOINT_FILE_BYTES: u64 = 4 * 1024;
pub const MAX_NETWORK_ACTION_BYTES: usize = 64 * 1024;
pub const MAX_NETWORK_ID_BYTES: usize = 128;
pub const MAX_CLOCK_SKEW_SECS: u64 = 300;

const MUTATION_CONTEXT: &[u8] = b"AWENET-NETWORK-CONTROL-MUTATION/v1\0";
const ADMIN_MUTATION_CONTEXT: &[u8] = b"AWENET-NETWORK-CONTROL-ADMIN-MUTATION/v1\0";
const STORE_CONTEXT: &[u8] = b"AWENET-NETWORK-CONTROL-STORE/v1\0";
const NODE_ADMISSION_CONTEXT: &[u8] = b"AWENET-NODE-ADMISSION/v1\0";
const NETWORK_CONTROL_CHECKPOINT_CONTEXT: &[u8] =
    b"AWENET-NETWORK-CONTROL-CHECKPOINT/v1\0";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum NetworkAction {
    CreateCentre {
        centre_id: String,
    },
    AddNode {
        centre_id: String,
        node_id: String,
    },
    AdmitNode {
        centre_id: String,
        public_key: [u8; 32],
        #[serde(with = "crate::serde_bytes_64")]
        proof_signature: [u8; 64],
    },
    SetNodeHealth {
        node_id: String,
        alive: bool,
    },
    UpdateNodeMetrics {
        node_id: String,
        metrics: PeerMetrics,
    },
    CreateDataGroup {
        group_id: String,
        centre_ids: Vec<String>,
    },
    CreateCentreGroup {
        group_id: String,
        group_ids: Vec<String>,
    },
    ConnectCentres {
        left: String,
        right: String,
        mode: CentreLinkMode,
    },
    FailoverRelay {
        left: String,
        right: String,
    },
    DisconnectCentres {
        left: String,
        right: String,
    },
    RemoveNode {
        node_id: String,
    },
    RemoveCentre {
        centre_id: String,
    },
    RemoveDataGroup {
        group_id: String,
    },
    RemoveCentreGroup {
        group_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum NetworkControlError {
    #[error("only the installed AWENET founder may mutate the network topology")]
    NotFounder,
    #[error("the founder mutation signature is invalid")]
    InvalidSignature,
    #[error("the joining node's proof-of-possession signature is invalid")]
    InvalidNodeProof,
    #[error("the account registry does not belong to this network founder")]
    RegistryMismatch,
    #[error("the requested actor account was not found")]
    UnknownActor,
    #[error("the actor does not have permission to manage network topology")]
    InsufficientPrivileges,
    #[error("the requested topology action is reserved for the founder or runtime telemetry")]
    ProtectedAction,
    #[error("the requested mutation sequence is exhausted")]
    SequenceExhausted,
    #[error("the mutation timestamp is older than the last committed mutation")]
    TimestampWentBackwards,
    #[error("the mutation timestamp is too far in the future")]
    TimestampInFuture,
    #[error("the network action is too large")]
    ActionTooLarge,
    #[error("the network identifier is invalid")]
    InvalidIdentifier,
    #[error("the supplied node metrics are invalid")]
    InvalidMetrics,
    #[error("network topology mutation failed: {0}")]
    Topology(String),
    #[error("network topology snapshot is inconsistent")]
    InvalidState,
    #[error("network control store belongs to another founder")]
    WrongFounder,
    #[error("network control snapshot is older than the active snapshot")]
    StaleSnapshot,
    #[error("different network control snapshots share the same sequence")]
    SnapshotEquivocation,
    #[error("the follower rollback checkpoint is missing; explicitly bootstrap this follower once")]
    CheckpointMissing,
    #[error("the follower rollback checkpoint is inconsistent with this Node or its active snapshot")]
    InvalidCheckpoint,
    #[error("network control store version is unsupported")]
    InvalidStoreVersion,
    #[error("network control snapshot exceeds the maximum file size")]
    StoreTooLarge,
    #[error("network control serialization failed: {0}")]
    Serialization(String),
    #[error("network control storage failed: {0}")]
    Io(String),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkControlPlane {
    version: u16,
    founder_awe_id: String,
    founder_public_key: [u8; 32],
    sequence: u64,
    updated_at_unix: u64,
    #[serde(default)]
    last_actor_awe_id: Option<String>,
    topology: AweNet,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SignedNetworkControlFile {
    version: u16,
    founder_awe_id: String,
    founder_public_key: [u8; 32],
    snapshot: String,
    #[serde(with = "crate::serde_bytes_64")]
    signature: [u8; 64],
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct NetworkControlCheckpointPayload {
    version: u16,
    founder_awe_id: String,
    founder_public_key: [u8; 32],
    node_awe_id: String,
    node_public_key: [u8; 32],
    sequence: u64,
    updated_at_unix: u64,
    snapshot_digest: [u8; 32],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SignedNetworkControlCheckpointFile {
    payload: String,
    #[serde(with = "crate::serde_bytes_64")]
    signature: [u8; 64],
}

/// An opaque snapshot that has passed signature, Founder, size and topology validation.
/// Its private field prevents callers from constructing one from untrusted JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedNetworkControlSnapshot {
    control: NetworkControlPlane,
    /// Digest of the exact snapshot string covered by the Founder signature.
    digest: [u8; 32],
}

impl NetworkControlPlane {
    pub fn new(founder: &Identity, at_unix: u64) -> Self {
        let founder_awe_id = founder.public.awe_id.to_hex();
        Self {
            version: NETWORK_CONTROL_VERSION,
            founder_awe_id: founder_awe_id.clone(),
            founder_public_key: founder.public.public_key,
            sequence: 0,
            updated_at_unix: at_unix,
            last_actor_awe_id: Some(founder_awe_id),
            topology: AweNet::default(),
        }
    }

    pub fn founder_awe_id(&self) -> &str {
        &self.founder_awe_id
    }

    pub fn founder_public_key(&self) -> &[u8; 32] {
        &self.founder_public_key
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn next_sequence(&self) -> Option<u64> {
        self.sequence.checked_add(1)
    }

    fn verify_node_admission(
        &self,
        action: &NetworkAction,
        at_unix: u64,
    ) -> Result<(), NetworkControlError> {
        if let NetworkAction::AdmitNode {
            centre_id,
            public_key,
            proof_signature,
        } = action
        {
            let next_sequence = self
                .sequence
                .checked_add(1)
                .ok_or(NetworkControlError::SequenceExhausted)?;
            let message = node_admission_message(
                &self.founder_awe_id,
                centre_id,
                public_key,
                next_sequence,
                at_unix,
            )?;
            if !Identity::verify(public_key, &message, proof_signature) {
                return Err(NetworkControlError::InvalidNodeProof);
            }
        }
        Ok(())
    }

    pub fn updated_at_unix(&self) -> u64 {
        self.updated_at_unix
    }

    pub fn last_actor_awe_id(&self) -> Option<&str> {
        self.last_actor_awe_id.as_deref()
    }

    pub fn topology(&self) -> &AweNet {
        &self.topology
    }

    /// Returns true only for a proof-backed, active node record whose AWEID
    /// and public key match the authenticated transport peer.
    ///
    /// Legacy AddNode placeholders are intentionally not sufficient for
    /// transport admission.
    pub fn is_node_admitted(&self, node_awe_id: &str, public_key: &[u8; 32]) -> bool {
        if AweId::from_public_key(public_key).to_hex() != node_awe_id {
            return false;
        }

        self.topology.centres.values().any(|centre| {
            centre.nodes.get(node_awe_id).is_some_and(|node| {
                node.id.as_str() == node_awe_id
                    && node.public_key == Some(*public_key)
                    && node.admitted_at_unix.is_some()
            })
        })
    }

    /// Applies a candidate snapshot that has already passed load_verified.
    ///
    /// A follower never rolls its active control plane backwards. Exact
    /// duplicates are harmless, while two different founder-signed snapshots
    /// with the same sequence are treated as equivocation and rejected.
    pub fn replace_with_verified_snapshot(
        &mut self,
        candidate: VerifiedNetworkControlSnapshot,
    ) -> Result<bool, NetworkControlError> {
        let candidate = candidate.control;
        candidate.validate()?;
        if candidate.founder_awe_id != self.founder_awe_id
            || candidate.founder_public_key != self.founder_public_key
        {
            return Err(NetworkControlError::WrongFounder);
        }
        if candidate.sequence < self.sequence {
            return Err(NetworkControlError::StaleSnapshot);
        }
        if candidate.sequence == self.sequence {
            return if candidate == *self {
                Ok(false)
            } else {
                Err(NetworkControlError::SnapshotEquivocation)
            };
        }
        if candidate.updated_at_unix < self.updated_at_unix {
            return Err(NetworkControlError::TimestampWentBackwards);
        }

        *self = candidate;
        Ok(true)
    }

    /// Applies a verified Founder snapshot and durably advances this follower's
    /// locally signed rollback checkpoint before publishing the new live state.
    pub fn replace_with_verified_snapshot_and_checkpoint(
        &mut self,
        candidate: VerifiedNetworkControlSnapshot,
        checkpoint_path: &Path,
        trusted_founder_public_key: &[u8; 32],
        local_node_identity: &Identity,
    ) -> Result<bool, NetworkControlError> {
        let candidate_control = candidate.control;
        candidate_control.validate()?;
        if candidate_control.founder_awe_id != self.founder_awe_id
            || candidate_control.founder_public_key != self.founder_public_key
            || self.founder_public_key != *trusted_founder_public_key
        {
            return Err(NetworkControlError::WrongFounder);
        }

        let checkpoint = load_checkpoint(
            checkpoint_path,
            trusted_founder_public_key,
            local_node_identity,
        )?
        .ok_or(NetworkControlError::CheckpointMissing)?;

        // The checkpoint is written before a candidate becomes active. A
        // mismatch here therefore indicates local state rollback or corruption.
        if checkpoint.sequence != self.sequence
            || checkpoint.updated_at_unix != self.updated_at_unix
        {
            return Err(NetworkControlError::InvalidCheckpoint);
        }

        if candidate_control.sequence < checkpoint.sequence {
            return Err(NetworkControlError::StaleSnapshot);
        }
        if candidate_control.sequence == checkpoint.sequence {
            return if candidate.digest == checkpoint.snapshot_digest
                && candidate_control == *self
            {
                Ok(false)
            } else {
                Err(NetworkControlError::SnapshotEquivocation)
            };
        }
        if candidate_control.updated_at_unix < self.updated_at_unix {
            return Err(NetworkControlError::TimestampWentBackwards);
        }

        save_checkpoint(
            checkpoint_path,
            &candidate_control,
            &candidate.digest,
            local_node_identity,
        )?;
        *self = candidate_control;
        Ok(true)
    }

    /// Validates the complete restored state before it is trusted or served.
    pub fn validate(&self) -> Result<(), NetworkControlError> {
        if self.version != NETWORK_CONTROL_VERSION
            || self.founder_awe_id != AweId::from_public_key(&self.founder_public_key).to_hex()
        {
            return Err(NetworkControlError::InvalidState);
        }
        if self.last_actor_awe_id.as_deref().is_some_and(|actor| {
            actor.is_empty()
                || actor.len() > MAX_NETWORK_ID_BYTES
                || actor
                    .chars()
                    .any(|character| character.is_control() || character.is_whitespace())
        }) {
            return Err(NetworkControlError::InvalidState);
        }
        let node_count = self
            .topology
            .centres
            .values()
            .fold(0usize, |count, centre| {
                count.saturating_add(centre.nodes.len())
            });
        if self.topology.centres.len() > 4_096
            || node_count > 65_536
            || self.topology.data_groups.len() > 4_096
            || self.topology.centre_groups.len() > 1_024
        {
            return Err(NetworkControlError::InvalidState);
        }
        self.topology
            .validate()
            .map_err(NetworkControlError::Topology)?;
        // Admission is part of this signed snapshot's history. A node cannot
        // claim it was admitted after the snapshot's latest committed mutation.
        if self.topology.centres.values().any(|centre| {
            centre.nodes.values().any(|node| {
                node.admitted_at_unix
                    .is_some_and(|admitted_at| admitted_at > self.updated_at_unix)
            })
        }) {
            return Err(NetworkControlError::InvalidState);
        }
        Ok(())
    }

    /// Returns the exact domain-separated bytes that a founder must sign for
    /// the next mutation. The current sequence is included, so an old signed
    /// request cannot be replayed after another mutation commits.
    pub fn signing_message(
        &self,
        action: &NetworkAction,
        at_unix: u64,
    ) -> Result<Vec<u8>, NetworkControlError> {
        let next_sequence = self
            .sequence
            .checked_add(1)
            .ok_or(NetworkControlError::SequenceExhausted)?;
        let payload = serde_json::to_vec(action)
            .map_err(|error| NetworkControlError::Serialization(error.to_string()))?;
        if payload.len() > MAX_NETWORK_ACTION_BYTES {
            return Err(NetworkControlError::ActionTooLarge);
        }

        let founder_len = u16::try_from(self.founder_awe_id.len())
            .map_err(|_| NetworkControlError::InvalidState)?;
        let payload_len =
            u32::try_from(payload.len()).map_err(|_| NetworkControlError::ActionTooLarge)?;
        let mut message = Vec::with_capacity(
            MUTATION_CONTEXT.len() + 2 + 2 + self.founder_awe_id.len() + 8 + 8 + 4 + payload.len(),
        );
        message.extend_from_slice(MUTATION_CONTEXT);
        message.extend_from_slice(&self.version.to_be_bytes());
        message.extend_from_slice(&founder_len.to_be_bytes());
        message.extend_from_slice(self.founder_awe_id.as_bytes());
        message.extend_from_slice(&next_sequence.to_be_bytes());
        message.extend_from_slice(&at_unix.to_be_bytes());
        message.extend_from_slice(&payload_len.to_be_bytes());
        message.extend_from_slice(&payload);
        Ok(message)
    }

    /// Returns the canonical message an active Founder/Admin account signs for
    /// a structural topology mutation. The actor ID and current sequence are
    /// bound to prevent actor substitution and replay.
    pub fn admin_signing_message(
        &self,
        registry_sequence: u64,
        actor_awe_id: &str,
        action: &NetworkAction,
        at_unix: u64,
    ) -> Result<Vec<u8>, NetworkControlError> {
        validate_id(actor_awe_id)?;
        let next_sequence = self
            .sequence
            .checked_add(1)
            .ok_or(NetworkControlError::SequenceExhausted)?;
        let payload = serde_json::to_vec(action)
            .map_err(|error| NetworkControlError::Serialization(error.to_string()))?;
        if payload.len() > MAX_NETWORK_ACTION_BYTES {
            return Err(NetworkControlError::ActionTooLarge);
        }
        let founder_len = u16::try_from(self.founder_awe_id.len())
            .map_err(|_| NetworkControlError::InvalidState)?;
        let actor_len = u16::try_from(actor_awe_id.len())
            .map_err(|_| NetworkControlError::InvalidIdentifier)?;
        let payload_len =
            u32::try_from(payload.len()).map_err(|_| NetworkControlError::ActionTooLarge)?;
        let mut message = Vec::with_capacity(
            ADMIN_MUTATION_CONTEXT.len()
                + 2
                + 2
                + self.founder_awe_id.len()
                + 2
                + actor_awe_id.len()
                + 8
                + 8
                + 8
                + 4
                + payload.len(),
        );
        message.extend_from_slice(ADMIN_MUTATION_CONTEXT);
        message.extend_from_slice(&self.version.to_be_bytes());
        message.extend_from_slice(&founder_len.to_be_bytes());
        message.extend_from_slice(self.founder_awe_id.as_bytes());
        message.extend_from_slice(&actor_len.to_be_bytes());
        message.extend_from_slice(actor_awe_id.as_bytes());
        message.extend_from_slice(&registry_sequence.to_be_bytes());
        message.extend_from_slice(&next_sequence.to_be_bytes());
        message.extend_from_slice(&at_unix.to_be_bytes());
        message.extend_from_slice(&payload_len.to_be_bytes());
        message.extend_from_slice(&payload);
        Ok(message)
    }

    /// Applies a signed topology mutation from an active Founder/Admin account.
    /// Administrative actions are limited to membership and topology structure;
    /// node health and metrics remain founder/runtime-owned telemetry.
    pub fn apply_authorized(
        &mut self,
        registry: &UserRegistry,
        actor_awe_id: &str,
        action: &NetworkAction,
        at_unix: u64,
        now_unix: u64,
        signature: &[u8; 64],
    ) -> Result<u64, NetworkControlError> {
        self.validate()?;
        if registry.founder_awe_id() != self.founder_awe_id {
            return Err(NetworkControlError::RegistryMismatch);
        }
        let account = registry
            .get(actor_awe_id)
            .ok_or(NetworkControlError::UnknownActor)?;
        if AweId::from_public_key(&account.public_key).to_hex() != actor_awe_id {
            return Err(NetworkControlError::InvalidState);
        }
        let capabilities = registry
            .capabilities_for(actor_awe_id)
            .ok_or(NetworkControlError::UnknownActor)?;
        if !capabilities.allows(&Capability::TopologyManage) {
            return Err(NetworkControlError::InsufficientPrivileges);
        }
        if !admin_topology_action_allowed(action) {
            return Err(NetworkControlError::ProtectedAction);
        }
        if at_unix < self.updated_at_unix {
            return Err(NetworkControlError::TimestampWentBackwards);
        }
        if at_unix > now_unix.saturating_add(MAX_CLOCK_SKEW_SECS) {
            return Err(NetworkControlError::TimestampInFuture);
        }
        let message =
            self.admin_signing_message(registry.sequence(), actor_awe_id, action, at_unix)?;
        if !Identity::verify(&account.public_key, &message, signature) {
            return Err(NetworkControlError::InvalidSignature);
        }
        self.verify_node_admission(action, at_unix)?;

        let mut candidate = self.topology.clone();
        apply_action(&mut candidate, action, at_unix)?;
        candidate
            .validate()
            .map_err(NetworkControlError::Topology)?;
        let next_sequence = self
            .sequence
            .checked_add(1)
            .ok_or(NetworkControlError::SequenceExhausted)?;
        self.topology = candidate;
        self.sequence = next_sequence;
        self.updated_at_unix = at_unix;
        self.last_actor_awe_id = Some(actor_awe_id.to_string());
        Ok(next_sequence)
    }

    /// Applies a founder-signed mutation in memory. Callers that need
    /// durability should use apply_and_save instead.
    pub fn apply_signed(
        &mut self,
        founder: &Identity,
        action: &NetworkAction,
        at_unix: u64,
        now_unix: u64,
        signature: &[u8; 64],
    ) -> Result<u64, NetworkControlError> {
        self.validate()?;
        if founder.public.awe_id.to_hex() != self.founder_awe_id
            || founder.public.public_key != self.founder_public_key
        {
            return Err(NetworkControlError::NotFounder);
        }
        if at_unix < self.updated_at_unix {
            return Err(NetworkControlError::TimestampWentBackwards);
        }
        if at_unix > now_unix.saturating_add(MAX_CLOCK_SKEW_SECS) {
            return Err(NetworkControlError::TimestampInFuture);
        }
        let message = self.signing_message(action, at_unix)?;
        if !Identity::verify(&self.founder_public_key, &message, signature) {
            return Err(NetworkControlError::InvalidSignature);
        }
        self.verify_node_admission(action, at_unix)?;

        let mut candidate = self.topology.clone();
        apply_action(&mut candidate, action, at_unix)?;
        candidate
            .validate()
            .map_err(NetworkControlError::Topology)?;

        let next_sequence = self
            .sequence
            .checked_add(1)
            .ok_or(NetworkControlError::SequenceExhausted)?;
        self.topology = candidate;
        self.sequence = next_sequence;
        self.updated_at_unix = at_unix;
        self.last_actor_awe_id = Some(self.founder_awe_id.clone());
        Ok(next_sequence)
    }

    /// Commits a signed mutation only after its candidate snapshot has been
    /// successfully written. A disk failure leaves the live state untouched.
    pub fn apply_and_save(
        &mut self,
        path: &Path,
        founder: &Identity,
        action: &NetworkAction,
        at_unix: u64,
        now_unix: u64,
        signature: &[u8; 64],
    ) -> Result<u64, NetworkControlError> {
        let mut candidate = self.clone();
        let sequence = candidate.apply_signed(founder, action, at_unix, now_unix, signature)?;
        save(path, &candidate, founder)?;
        *self = candidate;
        Ok(sequence)
    }

    /// Commits an admin-signed structural mutation. The resulting snapshot is
    /// still signed by the installed founder key, while the actor AWEID is
    /// retained in the snapshot for operator visibility.
    pub fn apply_and_save_authorized(
        &mut self,
        path: &Path,
        founder: &Identity,
        registry: &UserRegistry,
        actor_awe_id: &str,
        action: &NetworkAction,
        at_unix: u64,
        now_unix: u64,
        signature: &[u8; 64],
    ) -> Result<u64, NetworkControlError> {
        let mut candidate = self.clone();
        let sequence = candidate.apply_authorized(
            registry,
            actor_awe_id,
            action,
            at_unix,
            now_unix,
            signature,
        )?;
        save(path, &candidate, founder)?;
        *self = candidate;
        Ok(sequence)
    }
}

/// Returns the exact message a joining node must sign before an Admin or
/// Founder can add it to the topology. The next control-plane sequence and
/// timestamp prevent a proof from being replayed after another mutation.
pub fn node_admission_message(
    founder_awe_id: &str,
    centre_id: &str,
    public_key: &[u8; 32],
    next_sequence: u64,
    at_unix: u64,
) -> Result<Vec<u8>, NetworkControlError> {
    validate_id(founder_awe_id)?;
    validate_id(centre_id)?;
    let node_awe_id = AweId::from_public_key(public_key).to_hex();
    let founder_len =
        u16::try_from(founder_awe_id.len()).map_err(|_| NetworkControlError::InvalidIdentifier)?;
    let centre_len =
        u16::try_from(centre_id.len()).map_err(|_| NetworkControlError::InvalidIdentifier)?;
    let node_len =
        u16::try_from(node_awe_id.len()).map_err(|_| NetworkControlError::InvalidIdentifier)?;
    let mut message = Vec::with_capacity(
        NODE_ADMISSION_CONTEXT.len()
            + 2
            + founder_awe_id.len()
            + 2
            + centre_id.len()
            + 2
            + node_awe_id.len()
            + 32
            + 8
            + 8,
    );
    message.extend_from_slice(NODE_ADMISSION_CONTEXT);
    message.extend_from_slice(&founder_len.to_be_bytes());
    message.extend_from_slice(founder_awe_id.as_bytes());
    message.extend_from_slice(&centre_len.to_be_bytes());
    message.extend_from_slice(centre_id.as_bytes());
    message.extend_from_slice(&node_len.to_be_bytes());
    message.extend_from_slice(node_awe_id.as_bytes());
    message.extend_from_slice(public_key);
    message.extend_from_slice(&next_sequence.to_be_bytes());
    message.extend_from_slice(&at_unix.to_be_bytes());
    Ok(message)
}

fn admin_topology_action_allowed(action: &NetworkAction) -> bool {
    matches!(
        action,
        NetworkAction::CreateCentre { .. }
            | NetworkAction::AdmitNode { .. }
            | NetworkAction::CreateDataGroup { .. }
            | NetworkAction::CreateCentreGroup { .. }
            | NetworkAction::ConnectCentres { .. }
            | NetworkAction::FailoverRelay { .. }
            | NetworkAction::DisconnectCentres { .. }
            | NetworkAction::RemoveNode { .. }
            | NetworkAction::RemoveCentre { .. }
            | NetworkAction::RemoveDataGroup { .. }
            | NetworkAction::RemoveCentreGroup { .. }
    )
}

fn validate_id(value: &str) -> Result<(), NetworkControlError> {
    if value.is_empty()
        || value.len() > MAX_NETWORK_ID_BYTES
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(NetworkControlError::InvalidIdentifier);
    }
    Ok(())
}

fn apply_action(
    topology: &mut AweNet,
    action: &NetworkAction,
    at_unix: u64,
) -> Result<(), NetworkControlError> {
    match action {
        NetworkAction::CreateCentre { centre_id } => {
            validate_id(centre_id)?;
            topology
                .add_centre(DataCentre::new(centre_id.clone()))
                .map_err(NetworkControlError::Topology)?;
        }
        NetworkAction::AddNode { centre_id, node_id } => {
            validate_id(centre_id)?;
            validate_id(node_id)?;
            topology
                .centres
                .get_mut(centre_id)
                .ok_or_else(|| NetworkControlError::Topology("data centre not found".into()))?
                .add_node(Node::new(node_id.clone()))
                .map_err(NetworkControlError::Topology)?;
        }
        NetworkAction::AdmitNode {
            centre_id,
            public_key,
            ..
        } => {
            validate_id(centre_id)?;
            let node = Node::admitted(*public_key, at_unix);
            topology
                .centres
                .get_mut(centre_id)
                .ok_or_else(|| NetworkControlError::Topology("data centre not found".into()))?
                .add_node(node)
                .map_err(NetworkControlError::Topology)?;
        }
        NetworkAction::SetNodeHealth { node_id, alive } => {
            validate_id(node_id)?;
            topology
                .set_node_health(node_id, *alive)
                .map_err(NetworkControlError::Topology)?;
        }
        NetworkAction::UpdateNodeMetrics { node_id, metrics } => {
            validate_id(node_id)?;
            if metrics.load_percent > 100 || metrics.reliability_percent > 100 {
                return Err(NetworkControlError::InvalidMetrics);
            }
            if metrics.last_seen_unix > at_unix.saturating_add(MAX_CLOCK_SKEW_SECS) {
                return Err(NetworkControlError::TimestampInFuture);
            }
            topology
                .update_node_metrics(node_id, metrics.clone())
                .map_err(NetworkControlError::Topology)?;
        }
        NetworkAction::CreateDataGroup {
            group_id,
            centre_ids,
        } => {
            validate_id(group_id)?;
            for centre_id in centre_ids {
                validate_id(centre_id)?;
            }
            let group = DataGroup::new(group_id.clone(), centre_ids.clone())
                .map_err(NetworkControlError::Topology)?;
            topology
                .add_data_group(group)
                .map_err(NetworkControlError::Topology)?;
        }
        NetworkAction::CreateCentreGroup {
            group_id,
            group_ids,
        } => {
            validate_id(group_id)?;
            for group_id in group_ids {
                validate_id(group_id)?;
            }
            let group = CentreGroup::new(group_id.clone(), group_ids.clone())
                .map_err(NetworkControlError::Topology)?;
            topology
                .add_centre_group(group)
                .map_err(NetworkControlError::Topology)?;
        }
        NetworkAction::ConnectCentres { left, right, mode } => {
            validate_id(left)?;
            validate_id(right)?;
            if let CentreLinkMode::Relay { relay_node } = mode {
                validate_id(relay_node)?;
            }
            topology
                .connect_centres(left, right, mode.clone())
                .map_err(NetworkControlError::Topology)?;
        }
        NetworkAction::FailoverRelay { left, right } => {
            validate_id(left)?;
            validate_id(right)?;
            topology
                .failover_relay(left, right)
                .map_err(NetworkControlError::Topology)?;
        }
        NetworkAction::DisconnectCentres { left, right } => {
            validate_id(left)?;
            validate_id(right)?;
            topology
                .disconnect_centres(left, right)
                .map_err(NetworkControlError::Topology)?;
        }
        NetworkAction::RemoveNode { node_id } => {
            validate_id(node_id)?;
            topology
                .remove_node(node_id)
                .map_err(NetworkControlError::Topology)?;
        }
        NetworkAction::RemoveCentre { centre_id } => {
            validate_id(centre_id)?;
            topology
                .remove_centre(centre_id)
                .map_err(NetworkControlError::Topology)?;
        }
        NetworkAction::RemoveDataGroup { group_id } => {
            validate_id(group_id)?;
            topology
                .remove_data_group(group_id)
                .map_err(NetworkControlError::Topology)?;
        }
        NetworkAction::RemoveCentreGroup { group_id } => {
            validate_id(group_id)?;
            topology
                .remove_centre_group(group_id)
                .map_err(NetworkControlError::Topology)?;
        }
    }
    Ok(())
}

fn store_signing_bytes(
    version: u16,
    founder_awe_id: &str,
    founder_public_key: &[u8; 32],
    snapshot: &str,
) -> Result<Vec<u8>, NetworkControlError> {
    let founder_len =
        u16::try_from(founder_awe_id.len()).map_err(|_| NetworkControlError::InvalidState)?;
    let snapshot_len =
        u64::try_from(snapshot.len()).map_err(|_| NetworkControlError::StoreTooLarge)?;
    let mut bytes = Vec::with_capacity(
        STORE_CONTEXT.len() + 2 + 2 + founder_awe_id.len() + 32 + 8 + snapshot.len(),
    );
    bytes.extend_from_slice(STORE_CONTEXT);
    bytes.extend_from_slice(&version.to_be_bytes());
    bytes.extend_from_slice(&founder_len.to_be_bytes());
    bytes.extend_from_slice(founder_awe_id.as_bytes());
    bytes.extend_from_slice(founder_public_key);
    bytes.extend_from_slice(&snapshot_len.to_be_bytes());
    bytes.extend_from_slice(snapshot.as_bytes());
    Ok(bytes)
}

fn checkpoint_signing_bytes(payload: &str) -> Result<Vec<u8>, NetworkControlError> {
    let payload_len =
        u64::try_from(payload.len()).map_err(|_| NetworkControlError::StoreTooLarge)?;
    let mut bytes = Vec::with_capacity(NETWORK_CONTROL_CHECKPOINT_CONTEXT.len() + 8 + payload.len());
    bytes.extend_from_slice(NETWORK_CONTROL_CHECKPOINT_CONTEXT);
    bytes.extend_from_slice(&payload_len.to_be_bytes());
    bytes.extend_from_slice(payload.as_bytes());
    Ok(bytes)
}

fn save_checkpoint(
    path: &Path,
    control: &NetworkControlPlane,
    snapshot_digest: &[u8; 32],
    local_node_identity: &Identity,
) -> Result<(), NetworkControlError> {
    let payload = NetworkControlCheckpointPayload {
        version: NETWORK_CONTROL_CHECKPOINT_VERSION,
        founder_awe_id: control.founder_awe_id.clone(),
        founder_public_key: control.founder_public_key,
        node_awe_id: local_node_identity.public.awe_id.to_hex(),
        node_public_key: local_node_identity.public.public_key,
        sequence: control.sequence,
        updated_at_unix: control.updated_at_unix,
        snapshot_digest: *snapshot_digest,
    };
    let payload_json = serde_json::to_string(&payload)
        .map_err(|error| NetworkControlError::Serialization(error.to_string()))?;
    let signing_bytes = checkpoint_signing_bytes(&payload_json)?;
    let envelope = SignedNetworkControlCheckpointFile {
        payload: payload_json,
        signature: local_node_identity.sign(&signing_bytes),
    };
    let encoded = serde_json::to_vec_pretty(&envelope)
        .map_err(|error| NetworkControlError::Serialization(error.to_string()))?;
    if encoded.len() as u64 > MAX_NETWORK_CONTROL_CHECKPOINT_FILE_BYTES {
        return Err(NetworkControlError::StoreTooLarge);
    }
    crate::storage::save_manifest(path, &encoded)
        .map_err(|error| NetworkControlError::Io(error.to_string()))
}

fn load_checkpoint(
    path: &Path,
    trusted_founder_public_key: &[u8; 32],
    local_node_identity: &Identity,
) -> Result<Option<NetworkControlCheckpointPayload>, NetworkControlError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(NetworkControlError::Io(error.to_string())),
    };
    let size = file
        .metadata()
        .map_err(|error| NetworkControlError::Io(error.to_string()))?
        .len();
    if size > MAX_NETWORK_CONTROL_CHECKPOINT_FILE_BYTES {
        return Err(NetworkControlError::StoreTooLarge);
    }
    let mut bytes = Vec::new();
    file.take(MAX_NETWORK_CONTROL_CHECKPOINT_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| NetworkControlError::Io(error.to_string()))?;
    if bytes.len() as u64 > MAX_NETWORK_CONTROL_CHECKPOINT_FILE_BYTES {
        return Err(NetworkControlError::StoreTooLarge);
    }

    let envelope: SignedNetworkControlCheckpointFile = serde_json::from_slice(&bytes)
        .map_err(|error| NetworkControlError::Serialization(error.to_string()))?;
    let signing_bytes = checkpoint_signing_bytes(&envelope.payload)?;
    if !Identity::verify(
        &local_node_identity.public.public_key,
        &signing_bytes,
        &envelope.signature,
    ) {
        return Err(NetworkControlError::InvalidCheckpoint);
    }
    let payload: NetworkControlCheckpointPayload = serde_json::from_str(&envelope.payload)
        .map_err(|error| NetworkControlError::Serialization(error.to_string()))?;
    if payload.version != NETWORK_CONTROL_CHECKPOINT_VERSION {
        return Err(NetworkControlError::InvalidStoreVersion);
    }

    let expected_founder_awe_id =
        AweId::from_public_key(trusted_founder_public_key).to_hex();
    if payload.founder_awe_id != expected_founder_awe_id
        || payload.founder_public_key != *trusted_founder_public_key
    {
        return Err(NetworkControlError::WrongFounder);
    }
    if payload.node_awe_id != local_node_identity.public.awe_id.to_hex()
        || payload.node_public_key != local_node_identity.public.public_key
    {
        return Err(NetworkControlError::InvalidCheckpoint);
    }
    Ok(Some(payload))
}

/// Saves a founder-signed snapshot through the shared atomic manifest writer.
/// The snapshot is embedded as its exact serialized string, so verification
/// uses the exact bytes that were signed rather than reserializing HashMaps.
pub fn save(
    path: &Path,
    control: &NetworkControlPlane,
    founder: &Identity,
) -> Result<(), NetworkControlError> {
    control.validate()?;
    if control.founder_awe_id != founder.public.awe_id.to_hex()
        || control.founder_public_key != founder.public.public_key
    {
        return Err(NetworkControlError::WrongFounder);
    }
    let snapshot = serde_json::to_string(control)
        .map_err(|error| NetworkControlError::Serialization(error.to_string()))?;
    if snapshot.len() as u64 > MAX_NETWORK_CONTROL_FILE_BYTES {
        return Err(NetworkControlError::StoreTooLarge);
    }
    let signing_bytes = store_signing_bytes(
        NETWORK_CONTROL_VERSION,
        &control.founder_awe_id,
        &control.founder_public_key,
        &snapshot,
    )?;
    let envelope = SignedNetworkControlFile {
        version: NETWORK_CONTROL_VERSION,
        founder_awe_id: control.founder_awe_id.clone(),
        founder_public_key: control.founder_public_key,
        snapshot,
        signature: founder.sign(&signing_bytes),
    };
    let encoded = serde_json::to_vec_pretty(&envelope)
        .map_err(|error| NetworkControlError::Serialization(error.to_string()))?;
    if encoded.len() as u64 > MAX_NETWORK_CONTROL_FILE_BYTES {
        return Err(NetworkControlError::StoreTooLarge);
    }
    crate::storage::save_manifest(path, &encoded)
        .map_err(|error| NetworkControlError::Io(error.to_string()))
}

/// Loads a founder-authenticated snapshot using the installer's Founder identity.
pub fn load(path: &Path, founder: &Identity) -> Result<NetworkControlPlane, NetworkControlError> {
    let control = load_verified(path, &founder.public.public_key)?;
    if control.founder_awe_id != founder.public.awe_id.to_hex() {
        return Err(NetworkControlError::WrongFounder);
    }
    Ok(control)
}

fn load_verified_snapshot_data(
    path: &Path,
    trusted_founder_public_key: &[u8; 32],
) -> Result<(NetworkControlPlane, [u8; 32]), NetworkControlError> {
    let file = File::open(path).map_err(|error| NetworkControlError::Io(error.to_string()))?;
    let size = file
        .metadata()
        .map_err(|error| NetworkControlError::Io(error.to_string()))?
        .len();
    if size > MAX_NETWORK_CONTROL_FILE_BYTES {
        return Err(NetworkControlError::StoreTooLarge);
    }
    let mut bytes = Vec::new();
    file.take(MAX_NETWORK_CONTROL_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| NetworkControlError::Io(error.to_string()))?;
    if bytes.len() as u64 > MAX_NETWORK_CONTROL_FILE_BYTES {
        return Err(NetworkControlError::StoreTooLarge);
    }
    let envelope: SignedNetworkControlFile = serde_json::from_slice(&bytes)
        .map_err(|error| NetworkControlError::Serialization(error.to_string()))?;
    if envelope.version != NETWORK_CONTROL_VERSION {
        return Err(NetworkControlError::InvalidStoreVersion);
    }
    let trusted_founder_awe_id = AweId::from_public_key(trusted_founder_public_key).to_hex();
    if envelope.founder_awe_id != trusted_founder_awe_id
        || envelope.founder_public_key != *trusted_founder_public_key
    {
        return Err(NetworkControlError::WrongFounder);
    }
    let signing_bytes = store_signing_bytes(
        envelope.version,
        &envelope.founder_awe_id,
        &envelope.founder_public_key,
        &envelope.snapshot,
    )?;
    if !Identity::verify(
        trusted_founder_public_key,
        &signing_bytes,
        &envelope.signature,
    ) {
        return Err(NetworkControlError::InvalidSignature);
    }
    let snapshot_digest = *blake3::hash(envelope.snapshot.as_bytes()).as_bytes();
    let control: NetworkControlPlane = serde_json::from_str(&envelope.snapshot)
        .map_err(|error| NetworkControlError::Serialization(error.to_string()))?;
    control.validate()?;
    if control.founder_awe_id != envelope.founder_awe_id
        || control.founder_public_key != envelope.founder_public_key
    {
        return Err(NetworkControlError::WrongFounder);
    }
    Ok((control, snapshot_digest))
}

/// Loads a founder-authenticated snapshot using an explicitly pinned public key.
/// The caller supplies the trust root out-of-band; the key embedded in the file
/// is never trusted by itself.
pub fn load_verified(
    path: &Path,
    trusted_founder_public_key: &[u8; 32],
) -> Result<NetworkControlPlane, NetworkControlError> {
    Ok(load_verified_snapshot_data(path, trusted_founder_public_key)?.0)
}

/// Loads a verified snapshot and preserves the digest of the exact signed bytes.
pub fn load_verified_snapshot(
    path: &Path,
    trusted_founder_public_key: &[u8; 32],
) -> Result<VerifiedNetworkControlSnapshot, NetworkControlError> {
    let (control, digest) = load_verified_snapshot_data(path, trusted_founder_public_key)?;
    Ok(VerifiedNetworkControlSnapshot { control, digest })
}

/// Loads a Founder snapshot while enforcing a Node-local, signed monotonic checkpoint.
///
/// Every follower must explicitly opt into its first checkpoint bootstrap,
/// including sequence-zero installations. Once the checkpoint exists, a
/// missing checkpoint always fails closed.
pub fn load_verified_with_checkpoint(
    path: &Path,
    checkpoint_path: &Path,
    trusted_founder_public_key: &[u8; 32],
    local_node_identity: &Identity,
    allow_checkpoint_bootstrap: bool,
) -> Result<NetworkControlPlane, NetworkControlError> {
    let candidate = load_verified_snapshot(path, trusted_founder_public_key)?;
    let control = candidate.control.clone();

    match load_checkpoint(
        checkpoint_path,
        trusted_founder_public_key,
        local_node_identity,
    )? {
        Some(checkpoint) => {
            if control.sequence < checkpoint.sequence {
                return Err(NetworkControlError::StaleSnapshot);
            }
            if control.sequence == checkpoint.sequence {
                if candidate.digest != checkpoint.snapshot_digest
                    || control.updated_at_unix != checkpoint.updated_at_unix
                {
                    return Err(NetworkControlError::SnapshotEquivocation);
                }
            } else {
                save_checkpoint(
                    checkpoint_path,
                    &control,
                    &candidate.digest,
                    local_node_identity,
                )?;
            }
        }
        None => {
            if !allow_checkpoint_bootstrap {
                return Err(NetworkControlError::CheckpointMissing);
            }
            save_checkpoint(
                checkpoint_path,
                &control,
                &candidate.digest,
                local_node_identity,
            )?;
        }
    }

    Ok(control)
}

/// Creates the first snapshot only when the path is absent. Existing corrupt
/// or differently-owned state fails closed and is never reset to an empty net.
pub fn load_or_create(
    path: &Path,
    founder: &Identity,
    at_unix: u64,
) -> Result<NetworkControlPlane, NetworkControlError> {
    match fs::metadata(path) {
        Ok(_) => load(path, founder),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let control = NetworkControlPlane::new(founder, at_unix);
            save(path, &control, founder)?;
            Ok(control)
        }
        Err(error) => Err(NetworkControlError::Io(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Username;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn identity(name: &str) -> Identity {
        Identity::generate(Username::new(name).unwrap())
    }

    fn temporary_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "awenet-network-control-{label}-{}-{}.json",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn apply(
        control: &mut NetworkControlPlane,
        founder: &Identity,
        action: NetworkAction,
        at_unix: u64,
    ) -> Result<u64, NetworkControlError> {
        let message = control.signing_message(&action, at_unix)?;
        let signature = founder.sign(&message);
        control.apply_signed(founder, &action, at_unix, at_unix, &signature)
    }

    #[test]
    fn active_admin_can_commit_durable_structural_topology_changes() {
        use crate::user_registry::{UserRegistry, UserRole};

        let founder = identity("founder");
        let admin = identity("network-admin");
        let mut registry = UserRegistry::new(&founder, 1);
        let registration =
            UserRegistry::registration_message("network-admin", &admin.public.public_key);
        let account = registry
            .register(
                "network-admin",
                &admin.public.public_key,
                &admin.sign(&registration),
                2,
            )
            .unwrap();
        let promotion = registry
            .role_change_message(&account.awe_id, UserRole::Admin, 3)
            .unwrap();
        registry
            .set_role(
                &account.awe_id,
                UserRole::Admin,
                3,
                &founder.sign(&promotion),
            )
            .unwrap();

        let path = temporary_path("admin-commit");
        let mut control = NetworkControlPlane::new(&founder, 100);
        save(&path, &control, &founder).unwrap();
        let action = NetworkAction::CreateCentre {
            centre_id: "dc-admin".into(),
        };
        let message = control
            .admin_signing_message(registry.sequence(), &account.awe_id, &action, 101)
            .unwrap();
        let signature = admin.sign(&message);
        assert_eq!(
            control.apply_and_save_authorized(
                &path,
                &founder,
                &registry,
                &account.awe_id,
                &action,
                101,
                101,
                &signature,
            ),
            Ok(1)
        );
        let joining_node = identity("durable-joining-node");
        let admission_at = 102;
        let proof_message = node_admission_message(
            control.founder_awe_id(),
            "dc-admin",
            &joining_node.public.public_key,
            control.next_sequence().unwrap(),
            admission_at,
        )
        .unwrap();
        let admit = NetworkAction::AdmitNode {
            centre_id: "dc-admin".into(),
            public_key: joining_node.public.public_key,
            proof_signature: joining_node.sign(&proof_message),
        };
        let admit_message = control
            .admin_signing_message(registry.sequence(), &account.awe_id, &admit, admission_at)
            .unwrap();
        let admit_signature = admin.sign(&admit_message);
        assert_eq!(
            control.apply_and_save_authorized(
                &path,
                &founder,
                &registry,
                &account.awe_id,
                &admit,
                admission_at,
                admission_at,
                &admit_signature,
            ),
            Ok(2)
        );
        let joining_id = joining_node.public.awe_id.to_hex();
        assert_eq!(
            control.topology().centres["dc-admin"].nodes[&joining_id].public_key,
            Some(joining_node.public.public_key)
        );
        assert_eq!(control.last_actor_awe_id(), Some(account.awe_id.as_str()));
        assert!(control.topology().centres.contains_key("dc-admin"));

        let restored = load(&path, &founder).unwrap();
        assert_eq!(restored, control);
        assert_eq!(restored.last_actor_awe_id(), Some(account.awe_id.as_str()));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn follower_loads_founder_signed_snapshot_with_pinned_public_key() {
        let founder = identity("snapshot-founder");
        let follower = identity("snapshot-follower");
        let admitted_node = identity("snapshot-admitted-node");
        let path = temporary_path("follower-pinned-key");
        let mut control = NetworkControlPlane::new(&founder, 100);

        apply(
            &mut control,
            &founder,
            NetworkAction::CreateCentre {
                centre_id: "dc-follower".into(),
            },
            101,
        )
        .unwrap();
        let admitted_at = 102;
        let proof_message = node_admission_message(
            control.founder_awe_id(),
            "dc-follower",
            &admitted_node.public.public_key,
            control.next_sequence().unwrap(),
            admitted_at,
        )
        .unwrap();
        let action = NetworkAction::AdmitNode {
            centre_id: "dc-follower".into(),
            public_key: admitted_node.public.public_key,
            proof_signature: admitted_node.sign(&proof_message),
        };
        apply(&mut control, &founder, action, admitted_at).unwrap();
        save(&path, &control, &founder).unwrap();

        // A follower can validate the Founder-signed state using only the
        // public trust root; it does not need the Founder's private key.
        let restored = load_verified(&path, &founder.public.public_key).unwrap();
        assert_eq!(restored, control);
        assert!(restored.is_node_admitted(
            &admitted_node.public.awe_id.to_hex(),
            &admitted_node.public.public_key
        ));

        // A different pinned key cannot substitute its own identity or trust
        // the public key embedded in an attacker-supplied snapshot.
        assert!(matches!(
            load_verified(&path, &follower.public.public_key),
            Err(NetworkControlError::WrongFounder)
        ));
        assert!(matches!(
            load(&path, &follower),
            Err(NetworkControlError::WrongFounder)
        ));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn verified_snapshot_replacement_is_monotonic_and_idempotent() {
        let founder = identity("snapshot-refresh-founder");
        let initial_path = temporary_path("snapshot-refresh-initial");
        let updated_path = temporary_path("snapshot-refresh-updated");

        let initial = NetworkControlPlane::new(&founder, 100);
        save(&initial_path, &initial, &founder).unwrap();
        let mut follower = load_verified(&initial_path, &founder.public.public_key).unwrap();
        assert!(!follower
            .replace_with_verified_snapshot(
                load_verified_snapshot(&initial_path, &founder.public.public_key).unwrap()
            )
            .unwrap());

        let mut updated = initial;
        let action = NetworkAction::CreateCentre {
            centre_id: "dc-refreshed".into(),
        };
        let message = updated.signing_message(&action, 101).unwrap();
        updated
            .apply_signed(&founder, &action, 101, 101, &founder.sign(&message))
            .unwrap();
        save(&updated_path, &updated, &founder).unwrap();

        let candidate = load_verified_snapshot(&updated_path, &founder.public.public_key).unwrap();
        assert!(follower
            .replace_with_verified_snapshot(candidate.clone())
            .unwrap());
        assert_eq!(follower.sequence(), 1);
        assert!(follower.topology().centres.contains_key("dc-refreshed"));
        assert!(!follower.replace_with_verified_snapshot(candidate).unwrap());

        let _ = fs::remove_file(initial_path);
        let _ = fs::remove_file(updated_path);
    }

    #[test]
    fn verified_snapshot_replacement_rejects_rollback_equivocation_and_other_founders() {
        let founder = identity("snapshot-rejection-founder");
        let other_founder = identity("snapshot-rejection-other");
        let stale_path = temporary_path("snapshot-rejection-stale");
        let equivocation_path = temporary_path("snapshot-rejection-equivocation");
        let backwards_path = temporary_path("snapshot-rejection-backwards");
        let other_path = temporary_path("snapshot-rejection-other");

        let stale = NetworkControlPlane::new(&founder, 100);
        save(&stale_path, &stale, &founder).unwrap();

        let mut active = NetworkControlPlane::new(&founder, 100);
        let active_action = NetworkAction::CreateCentre {
            centre_id: "dc-active".into(),
        };
        let active_message = active.signing_message(&active_action, 101).unwrap();
        active
            .apply_signed(
                &founder,
                &active_action,
                101,
                101,
                &founder.sign(&active_message),
            )
            .unwrap();

        let mut conflicting = NetworkControlPlane::new(&founder, 100);
        let conflicting_action = NetworkAction::CreateCentre {
            centre_id: "dc-conflicting".into(),
        };
        let conflicting_message = conflicting
            .signing_message(&conflicting_action, 101)
            .unwrap();
        conflicting
            .apply_signed(
                &founder,
                &conflicting_action,
                101,
                101,
                &founder.sign(&conflicting_message),
            )
            .unwrap();
        save(&equivocation_path, &conflicting, &founder).unwrap();

        let mut backwards = active.clone();
        backwards.sequence += 1;
        backwards.updated_at_unix = 100;
        save(&backwards_path, &backwards, &founder).unwrap();

        let other = NetworkControlPlane::new(&other_founder, 100);
        save(&other_path, &other, &other_founder).unwrap();

        let before = active.clone();
        assert!(matches!(
            active.replace_with_verified_snapshot(
                load_verified_snapshot(&stale_path, &founder.public.public_key).unwrap()
            ),
            Err(NetworkControlError::StaleSnapshot)
        ));
        assert_eq!(active, before);

        assert!(matches!(
            active.replace_with_verified_snapshot(
                load_verified_snapshot(&equivocation_path, &founder.public.public_key).unwrap()
            ),
            Err(NetworkControlError::SnapshotEquivocation)
        ));
        assert_eq!(active, before);

        assert!(matches!(
            active.replace_with_verified_snapshot(
                load_verified_snapshot(&backwards_path, &founder.public.public_key).unwrap()
            ),
            Err(NetworkControlError::TimestampWentBackwards)
        ));
        assert_eq!(active, before);

        assert!(matches!(
            active.replace_with_verified_snapshot(
                load_verified_snapshot(&other_path, &other_founder.public.public_key).unwrap()
            ),
            Err(NetworkControlError::WrongFounder)
        ));
        assert_eq!(active, before);

        let _ = fs::remove_file(stale_path);
        let _ = fs::remove_file(equivocation_path);
        let _ = fs::remove_file(backwards_path);
        let _ = fs::remove_file(other_path);
    }

    #[test]
    fn topology_admin_denials_are_fail_closed_and_leave_state_unchanged() {
        use crate::user_registry::{UserRegistry, UserRole, UserStatus};

        let founder = identity("founder");
        let admin = identity("network-admin");
        let regular = identity("regular-user");
        let stranger_founder = identity("other-founder");
        let mut registry = UserRegistry::new(&founder, 1);

        let register = |registry: &mut UserRegistry, user: &Identity, username: &str, at: u64| {
            let message = UserRegistry::registration_message(username, &user.public.public_key);
            registry
                .register(username, &user.public.public_key, &user.sign(&message), at)
                .unwrap()
        };
        let admin_account = register(&mut registry, &admin, "network-admin", 2);
        let regular_account = register(&mut registry, &regular, "regular-user", 3);
        let promotion = registry
            .role_change_message(&admin_account.awe_id, UserRole::Admin, 4)
            .unwrap();
        registry
            .set_role(
                &admin_account.awe_id,
                UserRole::Admin,
                4,
                &founder.sign(&promotion),
            )
            .unwrap();

        let mut control = NetworkControlPlane::new(&founder, 100);
        let action = NetworkAction::CreateCentre {
            centre_id: "dc-protected".into(),
        };
        let regular_message = control
            .admin_signing_message(registry.sequence(), &regular_account.awe_id, &action, 101)
            .unwrap();
        let regular_signature = regular.sign(&regular_message);
        assert_eq!(
            control.apply_authorized(
                &registry,
                &regular_account.awe_id,
                &action,
                101,
                101,
                &regular_signature,
            ),
            Err(NetworkControlError::InsufficientPrivileges)
        );

        let unverified_add = NetworkAction::AddNode {
            centre_id: "dc-protected".into(),
            node_id: "unverified-node".into(),
        };
        let unverified_message = control
            .admin_signing_message(
                registry.sequence(),
                &admin_account.awe_id,
                &unverified_add,
                101,
            )
            .unwrap();
        let unverified_signature = admin.sign(&unverified_message);
        assert_eq!(
            control.apply_authorized(
                &registry,
                &admin_account.awe_id,
                &unverified_add,
                101,
                101,
                &unverified_signature,
            ),
            Err(NetworkControlError::ProtectedAction)
        );

        let telemetry = NetworkAction::SetNodeHealth {
            node_id: "node-0".into(),
            alive: false,
        };
        let telemetry_message = control
            .admin_signing_message(registry.sequence(), &admin_account.awe_id, &telemetry, 101)
            .unwrap();
        let telemetry_signature = admin.sign(&telemetry_message);
        assert_eq!(
            control.apply_authorized(
                &registry,
                &admin_account.awe_id,
                &telemetry,
                101,
                101,
                &telemetry_signature,
            ),
            Err(NetworkControlError::ProtectedAction)
        );

        let forged_message = control
            .admin_signing_message(registry.sequence(), &admin_account.awe_id, &action, 101)
            .unwrap();
        let forged_signature = regular.sign(&forged_message);
        assert_eq!(
            control.apply_authorized(
                &registry,
                &admin_account.awe_id,
                &action,
                101,
                101,
                &forged_signature,
            ),
            Err(NetworkControlError::InvalidSignature)
        );

        let wrong_registry = UserRegistry::new(&stranger_founder, 1);
        let wrong_registry_message = control
            .admin_signing_message(registry.sequence(), &admin_account.awe_id, &action, 101)
            .unwrap();
        let wrong_registry_signature = admin.sign(&wrong_registry_message);
        assert_eq!(
            control.apply_authorized(
                &wrong_registry,
                &admin_account.awe_id,
                &action,
                101,
                101,
                &wrong_registry_signature,
            ),
            Err(NetworkControlError::RegistryMismatch)
        );

        let suspension = registry
            .status_change_message(&admin_account.awe_id, UserStatus::Suspended, 5)
            .unwrap();
        registry
            .set_status(
                &admin_account.awe_id,
                UserStatus::Suspended,
                5,
                &founder.sign(&suspension),
            )
            .unwrap();
        let suspended_message = control
            .admin_signing_message(registry.sequence(), &admin_account.awe_id, &action, 101)
            .unwrap();
        let suspended_signature = admin.sign(&suspended_message);
        assert_eq!(
            control.apply_authorized(
                &registry,
                &admin_account.awe_id,
                &action,
                101,
                101,
                &suspended_signature,
            ),
            Err(NetworkControlError::InsufficientPrivileges)
        );

        let reactivate = registry
            .status_change_message(&admin_account.awe_id, UserStatus::Active, 6)
            .unwrap();
        registry
            .set_status(
                &admin_account.awe_id,
                UserStatus::Active,
                6,
                &founder.sign(&reactivate),
            )
            .unwrap();
        // The pre-suspension signature is still cryptographically valid for
        // the admin key, but the changed registry sequence makes it unusable.
        assert_eq!(
            control.apply_authorized(
                &registry,
                &admin_account.awe_id,
                &action,
                101,
                101,
                &suspended_signature,
            ),
            Err(NetworkControlError::InvalidSignature)
        );

        assert_eq!(control.sequence(), 0);
        assert!(control.topology().centres.is_empty());
        let founder_id = founder.public.awe_id.to_hex();
        assert_eq!(control.last_actor_awe_id(), Some(founder_id.as_str()));
    }

    #[test]
    fn founder_signature_and_sequence_gate_every_mutation() {
        let founder = identity("founder");
        let stranger = identity("stranger");
        let mut control = NetworkControlPlane::new(&founder, 100);
        let action = NetworkAction::CreateCentre {
            centre_id: "dc-a".into(),
        };
        let message = control.signing_message(&action, 101).unwrap();
        let forged = stranger.sign(&message);
        assert_eq!(
            control.apply_signed(&founder, &action, 101, 101, &forged),
            Err(NetworkControlError::InvalidSignature)
        );
        assert_eq!(control.sequence(), 0);
        assert!(control.topology().centres.is_empty());

        let signature = founder.sign(&message);
        assert_eq!(
            control
                .apply_signed(&founder, &action, 101, 101, &signature)
                .unwrap(),
            1
        );
        assert_eq!(control.topology().centres.len(), 1);
        assert_eq!(
            control.apply_signed(&founder, &action, 101, 102, &signature),
            Err(NetworkControlError::InvalidSignature)
        );
        assert_eq!(control.sequence(), 1);
    }

    #[test]
    fn signed_mutations_build_centres_nodes_and_group_membership() {
        let founder = identity("founder");
        let mut control = NetworkControlPlane::new(&founder, 100);
        for (index, centre) in ["dc-a", "dc-b", "dc-c"].iter().enumerate() {
            apply(
                &mut control,
                &founder,
                NetworkAction::CreateCentre {
                    centre_id: (*centre).into(),
                },
                101 + index as u64,
            )
            .unwrap();
        }
        for (index, centre) in ["dc-a", "dc-b", "dc-c"].iter().enumerate() {
            apply(
                &mut control,
                &founder,
                NetworkAction::AddNode {
                    centre_id: (*centre).into(),
                    node_id: format!("node-{centre}"),
                },
                104 + index as u64,
            )
            .unwrap();
        }
        apply(
            &mut control,
            &founder,
            NetworkAction::ConnectCentres {
                left: "dc-a".into(),
                right: "dc-b".into(),
                mode: CentreLinkMode::FullMesh,
            },
            110,
        )
        .unwrap();
        apply(
            &mut control,
            &founder,
            NetworkAction::CreateDataGroup {
                group_id: "dg-one".into(),
                centre_ids: vec!["dc-a".into(), "dc-b".into(), "dc-c".into()],
            },
            111,
        )
        .unwrap();
        assert_eq!(
            control
                .topology()
                .route(&"node-dc-a".into(), &"node-dc-b".into()),
            Some(vec!["node-dc-a".into(), "node-dc-b".into()])
        );
        assert_eq!(control.topology().data_groups.len(), 1);
        control.topology().validate().unwrap();
    }

    #[test]
    fn node_admission_proof_is_required_and_bound_to_the_next_sequence() {
        let founder = identity("founder");
        let joining_node = identity("joining-node");
        let attacker = identity("attacker");
        let mut control = NetworkControlPlane::new(&founder, 100);
        apply(
            &mut control,
            &founder,
            NetworkAction::CreateCentre {
                centre_id: "dc-admission".into(),
            },
            101,
        )
        .unwrap();

        let at_unix = 102;
        let next_sequence = control.next_sequence().unwrap();
        let proof_message = node_admission_message(
            control.founder_awe_id(),
            "dc-admission",
            &joining_node.public.public_key,
            next_sequence,
            at_unix,
        )
        .unwrap();
        let invalid_action = NetworkAction::AdmitNode {
            centre_id: "dc-admission".into(),
            public_key: joining_node.public.public_key,
            proof_signature: attacker.sign(&proof_message),
        };
        let invalid_message = control.signing_message(&invalid_action, at_unix).unwrap();
        let invalid_admin_signature = founder.sign(&invalid_message);
        let before_invalid = control.clone();
        assert_eq!(
            control.apply_signed(
                &founder,
                &invalid_action,
                at_unix,
                at_unix,
                &invalid_admin_signature,
            ),
            Err(NetworkControlError::InvalidNodeProof)
        );
        assert_eq!(control, before_invalid);

        let valid_proof = joining_node.sign(&proof_message);
        let valid_action = NetworkAction::AdmitNode {
            centre_id: "dc-admission".into(),
            public_key: joining_node.public.public_key,
            proof_signature: valid_proof,
        };
        let valid_message = control.signing_message(&valid_action, at_unix).unwrap();
        let valid_admin_signature = founder.sign(&valid_message);
        assert_eq!(
            control.apply_signed(
                &founder,
                &valid_action,
                at_unix,
                at_unix,
                &valid_admin_signature,
            ),
            Ok(2)
        );
        let node_id = joining_node.public.awe_id.to_hex();
        let admitted = &control.topology().centres["dc-admission"].nodes[&node_id];
        assert_eq!(admitted.public_key, Some(joining_node.public.public_key));
        assert_eq!(admitted.admitted_at_unix, Some(at_unix));

        apply(
            &mut control,
            &founder,
            NetworkAction::CreateCentre {
                centre_id: "dc-replay".into(),
            },
            103,
        )
        .unwrap();
        let replay_action = NetworkAction::AdmitNode {
            centre_id: "dc-replay".into(),
            public_key: joining_node.public.public_key,
            proof_signature: valid_proof,
        };
        let replay_at = 104;
        let replay_message = control.signing_message(&replay_action, replay_at).unwrap();
        let replay_admin_signature = founder.sign(&replay_message);
        let before_replay = control.clone();
        assert_eq!(
            control.apply_signed(
                &founder,
                &replay_action,
                replay_at,
                replay_at,
                &replay_admin_signature,
            ),
            Err(NetworkControlError::InvalidNodeProof)
        );
        assert_eq!(control, before_replay);
    }

    #[test]
    fn admitted_node_timestamp_cannot_exceed_control_plane_timestamp() {
        let founder = identity("founder");
        let joining_node = identity("joining-node");
        let mut control = NetworkControlPlane::new(&founder, 100);
        apply(
            &mut control,
            &founder,
            NetworkAction::CreateCentre {
                centre_id: "dc-time-bound".into(),
            },
            101,
        )
        .unwrap();

        let admitted_at = 102;
        let proof_message = node_admission_message(
            control.founder_awe_id(),
            "dc-time-bound",
            &joining_node.public.public_key,
            control.next_sequence().unwrap(),
            admitted_at,
        )
        .unwrap();
        let action = NetworkAction::AdmitNode {
            centre_id: "dc-time-bound".into(),
            public_key: joining_node.public.public_key,
            proof_signature: joining_node.sign(&proof_message),
        };
        let message = control.signing_message(&action, admitted_at).unwrap();
        let signature = founder.sign(&message);
        control
            .apply_signed(&founder, &action, admitted_at, admitted_at, &signature)
            .unwrap();
        assert!(control.validate().is_ok());

        let node_id = joining_node.public.awe_id.to_hex();
        let future_admission_time = control.updated_at_unix + 1;
        control
            .topology
            .centres
            .get_mut("dc-time-bound")
            .unwrap()
            .nodes
            .get_mut(&node_id)
            .unwrap()
            .admitted_at_unix = Some(future_admission_time);

        assert_eq!(control.validate(), Err(NetworkControlError::InvalidState));
    }

    #[test]
    fn centre_groups_require_two_existing_data_groups() {
        let founder = identity("founder");
        let mut control = NetworkControlPlane::new(&founder, 1);
        for index in 0..6 {
            apply(
                &mut control,
                &founder,
                NetworkAction::CreateCentre {
                    centre_id: format!("dc-{index}"),
                },
                2 + index,
            )
            .unwrap();
        }
        apply(
            &mut control,
            &founder,
            NetworkAction::CreateDataGroup {
                group_id: "dg-a".into(),
                centre_ids: vec!["dc-0".into(), "dc-1".into(), "dc-2".into()],
            },
            10,
        )
        .unwrap();
        apply(
            &mut control,
            &founder,
            NetworkAction::CreateDataGroup {
                group_id: "dg-b".into(),
                centre_ids: vec!["dc-3".into(), "dc-4".into(), "dc-5".into()],
            },
            11,
        )
        .unwrap();
        apply(
            &mut control,
            &founder,
            NetworkAction::CreateCentreGroup {
                group_id: "cg-main".into(),
                group_ids: vec!["dg-a".into(), "dg-b".into()],
            },
            12,
        )
        .unwrap();
        assert_eq!(control.topology().centre_groups.len(), 1);
        control.topology().validate().unwrap();
    }

    #[test]
    fn invalid_membership_does_not_advance_sequence_or_mutate_topology() {
        let founder = identity("founder");
        let mut control = NetworkControlPlane::new(&founder, 10);
        let action = NetworkAction::CreateDataGroup {
            group_id: "dg-bad".into(),
            centre_ids: vec![
                "dc-missing-a".into(),
                "dc-missing-b".into(),
                "dc-missing-c".into(),
            ],
        };
        let message = control.signing_message(&action, 11).unwrap();
        let signature = founder.sign(&message);
        assert!(matches!(
            control.apply_signed(&founder, &action, 11, 11, &signature),
            Err(NetworkControlError::Topology(_))
        ));
        assert_eq!(control.sequence(), 0);
        assert!(control.topology().data_groups.is_empty());
    }

    #[test]
    fn snapshot_roundtrips_and_rejects_wrong_founder_or_tampering() {
        let founder = identity("founder");
        let stranger = identity("stranger");
        let path = temporary_path("roundtrip");
        let mut control = NetworkControlPlane::new(&founder, 100);
        apply(
            &mut control,
            &founder,
            NetworkAction::CreateCentre {
                centre_id: "dc-a".into(),
            },
            101,
        )
        .unwrap();
        save(&path, &control, &founder).unwrap();
        assert_eq!(load(&path, &founder).unwrap(), control);
        assert_eq!(
            load(&path, &stranger),
            Err(NetworkControlError::WrongFounder)
        );

        let mut envelope: SignedNetworkControlFile =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        envelope.snapshot = envelope
            .snapshot
            .replace("\"sequence\":1", "\"sequence\":9");
        fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert_eq!(
            load(&path, &founder),
            Err(NetworkControlError::InvalidSignature)
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn failed_durable_commit_keeps_live_state_unchanged() {
        let founder = identity("founder");
        let path = temporary_path("blocked");
        fs::create_dir_all(&path).unwrap();
        let mut control = NetworkControlPlane::new(&founder, 100);
        let action = NetworkAction::CreateCentre {
            centre_id: "dc-a".into(),
        };
        let message = control.signing_message(&action, 101).unwrap();
        let signature = founder.sign(&message);
        assert!(control
            .apply_and_save(&path, &founder, &action, 101, 101, &signature)
            .is_err());
        assert_eq!(control.sequence(), 0);
        assert!(control.topology().centres.is_empty());
        let leftovers = fs::read_dir(&path).unwrap().count();
        assert_eq!(leftovers, 0);
        let _ = fs::remove_dir(path);
    }

    #[test]
    fn mutation_timestamps_are_monotonic_and_bounded() {
        let founder = identity("founder");
        let mut control = NetworkControlPlane::new(&founder, 100);
        let action = NetworkAction::CreateCentre {
            centre_id: "dc-a".into(),
        };
        let message = control.signing_message(&action, 101).unwrap();
        let signature = founder.sign(&message);
        assert_eq!(
            control.apply_signed(&founder, &action, 101, 101, &signature),
            Ok(1)
        );
        let backwards = NetworkAction::CreateCentre {
            centre_id: "dc-b".into(),
        };
        let message = control.signing_message(&backwards, 100).unwrap();
        let signature = founder.sign(&message);
        assert_eq!(
            control.apply_signed(&founder, &backwards, 100, 101, &signature),
            Err(NetworkControlError::TimestampWentBackwards)
        );
        let future = NetworkAction::CreateCentre {
            centre_id: "dc-c".into(),
        };
        let message = control.signing_message(&future, 10_000).unwrap();
        let signature = founder.sign(&message);
        assert_eq!(
            control.apply_signed(&founder, &future, 10_000, 101, &signature),
            Err(NetworkControlError::TimestampInFuture)
        );
        assert_eq!(control.sequence(), 1);
    }

    #[test]
    fn corrupt_existing_snapshot_is_not_reinitialized() {
        let founder = identity("founder");
        let path = temporary_path("corrupt");
        fs::write(&path, b"not-json").unwrap();
        assert!(load_or_create(&path, &founder, 100).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"not-json");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn signed_health_and_metrics_updates_are_bounded() {
        let founder = identity("founder");
        let mut control = NetworkControlPlane::new(&founder, 100);
        apply(
            &mut control,
            &founder,
            NetworkAction::CreateCentre {
                centre_id: "dc-a".into(),
            },
            101,
        )
        .unwrap();
        apply(
            &mut control,
            &founder,
            NetworkAction::AddNode {
                centre_id: "dc-a".into(),
                node_id: "node-a".into(),
            },
            102,
        )
        .unwrap();
        apply(
            &mut control,
            &founder,
            NetworkAction::SetNodeHealth {
                node_id: "node-a".into(),
                alive: false,
            },
            103,
        )
        .unwrap();

        let invalid_metrics = PeerMetrics {
            load_percent: 101,
            reliability_percent: 80,
            last_seen_unix: 104,
            ..PeerMetrics::default()
        };
        let invalid_action = NetworkAction::UpdateNodeMetrics {
            node_id: "node-a".into(),
            metrics: invalid_metrics,
        };
        let message = control.signing_message(&invalid_action, 104).unwrap();
        let signature = founder.sign(&message);
        assert_eq!(
            control.apply_signed(&founder, &invalid_action, 104, 104, &signature),
            Err(NetworkControlError::InvalidMetrics)
        );
        assert_eq!(control.sequence(), 3);

        let metrics = PeerMetrics {
            rtt_ms: 12,
            throughput_kbps: 50_000,
            load_percent: 75,
            reliability_percent: 80,
            last_seen_unix: 104,
        };
        apply(
            &mut control,
            &founder,
            NetworkAction::UpdateNodeMetrics {
                node_id: "node-a".into(),
                metrics: metrics.clone(),
            },
            104,
        )
        .unwrap();
        let node = &control.topology().centres["dc-a"].nodes["node-a"];
        assert!(!node.alive);
        assert_eq!(node.metrics, metrics);
    }

    #[test]
    fn relay_failover_and_dependency_safe_removal_are_transactional() {
        let founder = identity("founder");
        let mut control = NetworkControlPlane::new(&founder, 100);
        for (index, centre) in ["dc-a", "dc-b", "dc-c"].iter().enumerate() {
            apply(
                &mut control,
                &founder,
                NetworkAction::CreateCentre {
                    centre_id: (*centre).into(),
                },
                101 + index as u64,
            )
            .unwrap();
        }
        for (index, (centre, node)) in [
            ("dc-a", "a1"),
            ("dc-a", "a2"),
            ("dc-b", "b1"),
            ("dc-b", "b2"),
            ("dc-c", "c1"),
        ]
        .iter()
        .enumerate()
        {
            apply(
                &mut control,
                &founder,
                NetworkAction::AddNode {
                    centre_id: (*centre).into(),
                    node_id: (*node).into(),
                },
                104 + index as u64,
            )
            .unwrap();
        }
        apply(
            &mut control,
            &founder,
            NetworkAction::ConnectCentres {
                left: "dc-a".into(),
                right: "dc-b".into(),
                mode: CentreLinkMode::Relay {
                    relay_node: "a1".into(),
                },
            },
            109,
        )
        .unwrap();
        apply(
            &mut control,
            &founder,
            NetworkAction::FailoverRelay {
                left: "dc-a".into(),
                right: "dc-b".into(),
            },
            110,
        )
        .unwrap();
        apply(
            &mut control,
            &founder,
            NetworkAction::RemoveNode {
                node_id: "a1".into(),
            },
            111,
        )
        .unwrap();

        apply(
            &mut control,
            &founder,
            NetworkAction::CreateDataGroup {
                group_id: "dg-a".into(),
                centre_ids: vec!["dc-a".into(), "dc-b".into(), "dc-c".into()],
            },
            112,
        )
        .unwrap();
        let remove_referenced = NetworkAction::RemoveCentre {
            centre_id: "dc-a".into(),
        };
        let message = control.signing_message(&remove_referenced, 113).unwrap();
        let signature = founder.sign(&message);
        assert!(matches!(
            control.apply_signed(&founder, &remove_referenced, 113, 113, &signature),
            Err(NetworkControlError::Topology(_))
        ));
        assert_eq!(control.sequence(), 12);
        assert!(control.topology().centres.contains_key("dc-a"));

        apply(
            &mut control,
            &founder,
            NetworkAction::RemoveDataGroup {
                group_id: "dg-a".into(),
            },
            113,
        )
        .unwrap();
        apply(
            &mut control,
            &founder,
            NetworkAction::RemoveCentre {
                centre_id: "dc-a".into(),
            },
            114,
        )
        .unwrap();
        control.topology().validate().unwrap();
        assert!(!control.topology().centres.contains_key("dc-a"));
    }

    #[test]
    fn load_or_create_initializes_only_absent_state_and_reloads_commits() {
        let founder = identity("founder");
        let path = temporary_path("load-or-create");
        let mut control = load_or_create(&path, &founder, 100).unwrap();
        assert_eq!(control.sequence(), 0);

        let action = NetworkAction::CreateCentre {
            centre_id: "dc-a".into(),
        };
        let message = control.signing_message(&action, 101).unwrap();
        let signature = founder.sign(&message);
        control
            .apply_and_save(&path, &founder, &action, 101, 101, &signature)
            .unwrap();

        let restored = load_or_create(&path, &founder, 102).unwrap();
        assert_eq!(restored, control);
        assert_eq!(restored.sequence(), 1);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn sequence_exhaustion_is_rejected_before_signing_or_mutation() {
        let founder = identity("founder");
        let mut control = NetworkControlPlane::new(&founder, 100);
        control.sequence = u64::MAX;
        let action = NetworkAction::CreateCentre {
            centre_id: "dc-a".into(),
        };
        assert_eq!(
            control.signing_message(&action, 101),
            Err(NetworkControlError::SequenceExhausted)
        );
        assert!(control.topology().centres.is_empty());
    }
    #[test]
    fn admitted_node_lookup_requires_live_matching_proof_backed_record() {
        let founder = identity("admission-lookup-founder");
        let joining_node = identity("admission-lookup-node");
        let mut control = NetworkControlPlane::new(&founder, 100);
        apply(
            &mut control,
            &founder,
            NetworkAction::CreateCentre {
                centre_id: "dc-admission-lookup".into(),
            },
            101,
        )
        .unwrap();

        let at_unix = 102;
        let proof_message = node_admission_message(
            control.founder_awe_id(),
            "dc-admission-lookup",
            &joining_node.public.public_key,
            control.next_sequence().unwrap(),
            at_unix,
        )
        .unwrap();
        let admission = NetworkAction::AdmitNode {
            centre_id: "dc-admission-lookup".into(),
            public_key: joining_node.public.public_key,
            proof_signature: joining_node.sign(&proof_message),
        };
        apply(&mut control, &founder, admission, at_unix).unwrap();

        let node_id = joining_node.public.awe_id.to_hex();
        assert!(control.is_node_admitted(&node_id, &joining_node.public.public_key));
        assert!(!control.is_node_admitted(&node_id, &[0u8; 32]));
        assert!(!control.is_node_admitted("non-canonical-id", &joining_node.public.public_key));

        apply(
            &mut control,
            &founder,
            NetworkAction::SetNodeHealth {
                node_id: node_id.clone(),
                alive: false,
            },
            103,
        )
        .unwrap();
        // Operational health is not membership revocation: an unhealthy
        // admitted node must be able to reconnect and recover.
        assert!(control.is_node_admitted(&node_id, &joining_node.public.public_key));

        apply(
            &mut control,
            &founder,
            NetworkAction::SetNodeHealth {
                node_id: node_id.clone(),
                alive: true,
            },
            104,
        )
        .unwrap();
        assert!(control.is_node_admitted(&node_id, &joining_node.public.public_key));

        apply(
            &mut control,
            &founder,
            NetworkAction::RemoveNode {
                node_id: node_id.clone(),
            },
            105,
        )
        .unwrap();
        assert!(!control.is_node_admitted(&node_id, &joining_node.public.public_key));

        // A legacy AddNode record with the same ID is not proof of possession
        // and must never satisfy transport admission.
        apply(
            &mut control,
            &founder,
            NetworkAction::AddNode {
                centre_id: "dc-admission-lookup".into(),
                node_id: node_id.clone(),
            },
            106,
        )
        .unwrap();
        assert!(!control.is_node_admitted(&node_id, &joining_node.public.public_key));
    }
    #[test]
    fn follower_checkpoint_survives_restart_and_rejects_signed_rollback_and_equivocation() {
        let founder = identity("checkpoint-founder");
        let follower = identity("checkpoint-follower");
        let other_follower = identity("checkpoint-other-follower");
        let snapshot_path = temporary_path("checkpoint-snapshot");
        let checkpoint_path = temporary_path("checkpoint-state");

        let initial = NetworkControlPlane::new(&founder, 100);
        save(&snapshot_path, &initial, &founder).unwrap();
        let restored = load_verified_with_checkpoint(
            &snapshot_path,
            &checkpoint_path,
            &founder.public.public_key,
            &follower,
            true,
        )
        .unwrap();
        assert_eq!(restored.sequence(), 0);
        assert!(checkpoint_path.exists());

        let mut updated = restored;
        apply(
            &mut updated,
            &founder,
            NetworkAction::CreateCentre {
                centre_id: "dc-checkpoint-current".into(),
            },
            101,
        )
        .unwrap();
        save(&snapshot_path, &updated, &founder).unwrap();

        let restarted = load_verified_with_checkpoint(
            &snapshot_path,
            &checkpoint_path,
            &founder.public.public_key,
            &follower,
            false,
        )
        .unwrap();
        assert_eq!(restarted.sequence(), 1);

        // A previously valid Founder snapshot cannot roll a restarted
        // follower back after its local checkpoint has advanced.
        save(&snapshot_path, &initial, &founder).unwrap();
        assert!(matches!(
            load_verified_with_checkpoint(
                &snapshot_path,
                &checkpoint_path,
                &founder.public.public_key,
                &follower,
                false,
            ),
            Err(NetworkControlError::StaleSnapshot)
        ));

        // A different, valid Founder snapshot at the same sequence is
        // rejected as equivocation, even after the process has restarted.
        let mut conflicting = NetworkControlPlane::new(&founder, 100);
        apply(
            &mut conflicting,
            &founder,
            NetworkAction::CreateCentre {
                centre_id: "dc-checkpoint-conflict".into(),
            },
            101,
        )
        .unwrap();
        save(&snapshot_path, &conflicting, &founder).unwrap();
        assert!(matches!(
            load_verified_with_checkpoint(
                &snapshot_path,
                &checkpoint_path,
                &founder.public.public_key,
                &follower,
                false,
            ),
            Err(NetworkControlError::SnapshotEquivocation)
        ));

        assert!(matches!(
            load_verified_with_checkpoint(
                &snapshot_path,
                &checkpoint_path,
                &founder.public.public_key,
                &other_follower,
                false,
            ),
            Err(NetworkControlError::InvalidCheckpoint)
        ));

        fs::remove_file(&checkpoint_path).unwrap();
        assert!(matches!(
            load_verified_with_checkpoint(
                &snapshot_path,
                &checkpoint_path,
                &founder.public.public_key,
                &follower,
                false,
            ),
            Err(NetworkControlError::CheckpointMissing)
        ));

        fs::write(&checkpoint_path, b"{corrupt").unwrap();
        assert!(load_verified_with_checkpoint(
            &snapshot_path,
            &checkpoint_path,
            &founder.public.public_key,
            &follower,
            false,
        )
        .is_err());

        let _ = fs::remove_file(snapshot_path);
        let _ = fs::remove_file(checkpoint_path);
    }

    #[test]
    fn nonzero_legacy_follower_requires_explicit_checkpoint_bootstrap() {
        let founder = identity("checkpoint-legacy-founder");
        let follower = identity("checkpoint-legacy-follower");
        let snapshot_path = temporary_path("checkpoint-legacy-snapshot");
        let checkpoint_path = temporary_path("checkpoint-legacy-state");

        let mut existing = NetworkControlPlane::new(&founder, 100);
        apply(
            &mut existing,
            &founder,
            NetworkAction::CreateCentre {
                centre_id: "dc-existing".into(),
            },
            101,
        )
        .unwrap();
        save(&snapshot_path, &existing, &founder).unwrap();

        assert!(matches!(
            load_verified_with_checkpoint(
                &snapshot_path,
                &checkpoint_path,
                &founder.public.public_key,
                &follower,
                false,
            ),
            Err(NetworkControlError::CheckpointMissing)
        ));
        assert_eq!(
            load_verified_with_checkpoint(
                &snapshot_path,
                &checkpoint_path,
                &founder.public.public_key,
                &follower,
                true,
            )
            .unwrap()
            .sequence(),
            1
        );
        assert!(checkpoint_path.exists());

        let _ = fs::remove_file(snapshot_path);
        let _ = fs::remove_file(checkpoint_path);
    }


}
