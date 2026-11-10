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
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::Read,
    path::Path,
};
use thiserror::Error;

pub const NETWORK_CONTROL_VERSION: u16 = 1;
pub const MAX_NETWORK_CONTROL_FILE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_NETWORK_ACTION_BYTES: usize = 64 * 1024;
pub const MAX_NETWORK_ID_BYTES: usize = 128;
pub const MAX_CLOCK_SKEW_SECS: u64 = 300;

const MUTATION_CONTEXT: &[u8] = b"AWENET-NETWORK-CONTROL-MUTATION/v1\0";
const STORE_CONTEXT: &[u8] = b"AWENET-NETWORK-CONTROL-STORE/v1\0";

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

impl NetworkControlPlane {
    pub fn new(founder: &Identity, at_unix: u64) -> Self {
        Self {
            version: NETWORK_CONTROL_VERSION,
            founder_awe_id: founder.public.awe_id.to_hex(),
            founder_public_key: founder.public.public_key,
            sequence: 0,
            updated_at_unix: at_unix,
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

    pub fn updated_at_unix(&self) -> u64 {
        self.updated_at_unix
    }

    pub fn topology(&self) -> &AweNet {
        &self.topology
    }

    /// Validates the complete restored state before it is trusted or served.
    pub fn validate(&self) -> Result<(), NetworkControlError> {
        if self.version != NETWORK_CONTROL_VERSION
            || self.sequence == u64::MAX && self.updated_at_unix == 0
            || self.founder_awe_id != AweId::from_public_key(&self.founder_public_key).to_hex()
        {
            return Err(NetworkControlError::InvalidState);
        }
        self.topology
            .validate()
            .map_err(NetworkControlError::Topology)
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
        let payload_len = u32::try_from(payload.len())
            .map_err(|_| NetworkControlError::ActionTooLarge)?;
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
        let sequence =
            candidate.apply_signed(founder, action, at_unix, now_unix, signature)?;
        save(path, &candidate, founder)?;
        *self = candidate;
        Ok(sequence)
    }
}

fn validate_id(value: &str) -> Result<(), NetworkControlError> {
    if value.is_empty()
        || value.len() > MAX_NETWORK_ID_BYTES
        || value.chars().any(|character| character.is_control() || character.is_whitespace())
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

/// Loads only a founder-authenticated snapshot whose complete topology passes
/// validation. A corrupt or differently-owned file is never replaced.
pub fn load(
    path: &Path,
    founder: &Identity,
) -> Result<NetworkControlPlane, NetworkControlError> {
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
    if envelope.founder_awe_id != founder.public.awe_id.to_hex()
        || envelope.founder_public_key != founder.public.public_key
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
        &founder.public.public_key,
        &signing_bytes,
        &envelope.signature,
    ) {
        return Err(NetworkControlError::InvalidSignature);
    }
    let control: NetworkControlPlane = serde_json::from_str(&envelope.snapshot)
        .map_err(|error| NetworkControlError::Serialization(error.to_string()))?;
    control.validate()?;
    if control.founder_awe_id != envelope.founder_awe_id
        || control.founder_public_key != envelope.founder_public_key
    {
        return Err(NetworkControlError::WrongFounder);
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
            centre_ids: vec!["dc-missing-a".into(), "dc-missing-b".into(), "dc-missing-c".into()],
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
        assert_eq!(load(&path, &stranger), Err(NetworkControlError::WrongFounder));

        let mut envelope: SignedNetworkControlFile =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        envelope.snapshot = envelope.snapshot.replace("\"sequence\":1", "\"sequence\":9");
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
}
