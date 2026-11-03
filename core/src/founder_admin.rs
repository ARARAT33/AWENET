//! Founder-controlled Neter appointments and capability-scoped delegation.
//!
//! Neters are task administrators, never peers of the founder. Only the
//! founder key pinned in genesis can appoint, suspend, remove, or change
//! compensation. Delegation is available only to a Neter explicitly granted
//! that capability by the founder; delegated Neters cannot pass that right on.

use crate::{
    crypto::hash,
    founder::FounderGenesis,
};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use thiserror::Error;

pub const MAX_NETERS: usize = 1024;
pub const MAX_NETER_SCOPES: usize = 64;
pub const MAX_NETER_AUDIT: usize = 4096;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum NeterStatus {
    Active,
    Suspended,
    Removed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NeterRecord {
    pub neter_id: [u8; 32],
    pub public_key: [u8; 32],
    pub display_name: String,
    pub scopes: Vec<String>,
    pub can_appoint_neter: bool,
    pub compensation_atoms_per_task: u128,
    pub status: NeterStatus,
    pub appointed_by: [u8; 32],
    pub created_tick: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NeterApplication {
    pub application_id: [u8; 32],
    pub public_key: [u8; 32],
    pub display_name: String,
    pub requested_scopes: Vec<String>,
    pub message: String,
    pub submitted_tick: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NeterAuditRecord {
    pub sequence: u64,
    pub tick: u64,
    pub action: String,
    pub actor_id: [u8; 32],
    pub target_id: [u8; 32],
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum NeterError {
    #[error("founder genesis is invalid")]
    InvalidGenesis,
    #[error("founder signature is invalid")]
    InvalidFounderSignature,
    #[error("Neter signature is invalid")]
    InvalidNeterSignature,
    #[error("Neter registry is full")]
    RegistryFull,
    #[error("Neter already exists")]
    AlreadyExists,
    #[error("Neter does not exist")]
    UnknownNeter,
    #[error("Neter is suspended or removed")]
    InactiveNeter,
    #[error("Neter is not authorized to appoint another Neter")]
    DelegationNotGranted,
    #[error("delegated scopes exceed the appointing Neter's scopes")]
    ScopeEscalation,
    #[error("invalid or duplicate capability scope")]
    InvalidScopes,
    #[error("application is unknown")]
    UnknownApplication,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NeterRegistry {
    founder_public_key: [u8; 32],
    neters: BTreeMap<String, NeterRecord>,
    applications: BTreeMap<String, NeterApplication>,
    audit: VecDeque<NeterAuditRecord>,
    next_audit_sequence: u64,
}

#[derive(Serialize)]
struct AppointmentStatement<'a> {
    domain: &'static str,
    candidate_public_key: [u8; 32],
    display_name: &'a str,
    scopes: &'a [String],
    can_appoint_neter: bool,
    compensation_atoms_per_task: u128,
    appointed_by: [u8; 32],
    created_tick: u64,
}

#[derive(Serialize)]
struct NeterStatusStatement {
    domain: &'static str,
    neter_id: [u8; 32],
    status: NeterStatus,
    tick: u64,
}

#[derive(Serialize)]
struct CompensationStatement {
    domain: &'static str,
    neter_id: [u8; 32],
    compensation_atoms_per_task: u128,
    tick: u64,
}

#[derive(Serialize)]
struct ApplicationApprovalStatement<'a> {
    domain: &'static str,
    application_id: [u8; 32],
    candidate_public_key: [u8; 32],
    approved_scopes: &'a [String],
    can_appoint_neter: bool,
    compensation_atoms_per_task: u128,
    tick: u64,
}

pub fn neter_id(public_key: &[u8; 32]) -> [u8; 32] {
    hash(b"AWENET/NETER-ID/v1", public_key)
}

pub fn application_proof_message(
    public_key: [u8; 32],
    display_name: &str,
    requested_scopes: &[String],
    message: &str,
    submitted_tick: u64,
) -> Vec<u8> {
    #[derive(Serialize)]
    struct Statement<'a> {
        domain: &'static str,
        public_key: [u8; 32],
        display_name: &'a str,
        requested_scopes: &'a [String],
        message: &'a str,
        submitted_tick: u64,
    }
    serde_json::to_vec(&Statement {
        domain: "AWENET/NETER-APPLICATION-PROOF/v1",
        public_key,
        display_name,
        requested_scopes,
        message,
        submitted_tick,
    })
    .expect("Neter application proof serialization is infallible")
}

pub fn appointment_message(
    candidate_public_key: [u8; 32],
    display_name: &str,
    scopes: &[String],
    can_appoint_neter: bool,
    compensation_atoms_per_task: u128,
    appointed_by: [u8; 32],
    created_tick: u64,
) -> Vec<u8> {
    serde_json::to_vec(&AppointmentStatement {
        domain: "AWENET/NETER-APPOINTMENT/v1",
        candidate_public_key,
        display_name,
        scopes,
        can_appoint_neter,
        compensation_atoms_per_task,
        appointed_by,
        created_tick,
    })
    .expect("Neter appointment serialization is infallible")
}

impl NeterRegistry {
    pub fn new(genesis: &FounderGenesis) -> Result<Self, NeterError> {
        genesis.verify().map_err(|_| NeterError::InvalidGenesis)?;
        Ok(Self {
            founder_public_key: genesis.payload.founder_public_key,
            neters: BTreeMap::new(),
            applications: BTreeMap::new(),
            audit: VecDeque::new(),
            next_audit_sequence: 1,
        })
    }

    pub fn get(&self, id: &[u8; 32]) -> Option<&NeterRecord> {
        self.neters.get(&hex::encode(id))
    }

    pub fn active(&self) -> Vec<&NeterRecord> {
        self.neters
            .values()
            .filter(|record| record.status == NeterStatus::Active)
            .collect()
    }

    pub fn applications(&self) -> impl Iterator<Item = &NeterApplication> {
        self.applications.values()
    }

    pub fn audit(&self) -> impl Iterator<Item = &NeterAuditRecord> {
        self.audit.iter()
    }

    pub fn submit_application(
        &mut self,
        public_key: [u8; 32],
        display_name: &str,
        requested_scopes: Vec<String>,
        message: &str,
        tick: u64,
        proof_signature: [u8; 64],
    ) -> Result<NeterApplication, NeterError> {
        validate_scopes(&requested_scopes)?;
        let proof = application_proof_message(
            public_key, display_name, &requested_scopes, message, tick,
        );
        let applicant_key = VerifyingKey::from_bytes(&public_key)
            .map_err(|_| NeterError::InvalidNeterSignature)?;
        applicant_key
            .verify(&proof, &Signature::from_bytes(&proof_signature))
            .map_err(|_| NeterError::InvalidNeterSignature)?;
        if self.applications.len() >= MAX_NETERS {
            return Err(NeterError::RegistryFull);
        }
        let id = neter_id(&public_key);
        if self.neters.contains_key(&hex::encode(id)) {
            return Err(NeterError::AlreadyExists);
        }
        let application_id = hash(
            b"AWENET/NETER-APPLICATION/v1",
            &[id.as_slice(), &tick.to_be_bytes(), message.as_bytes()].concat(),
        );
        let application = NeterApplication {
            application_id,
            public_key,
            display_name: display_name.chars().take(80).collect(),
            requested_scopes,
            message: message.chars().take(1000).collect(),
            submitted_tick: tick,
        };
        self.applications
            .insert(hex::encode(application_id), application.clone());
        Ok(application)
    }

    /// Founder-approved application. The founder signs the exact application,
    /// approved scopes, delegation grant, compensation and tick.
    pub fn approve_application(
        &mut self,
        genesis: &FounderGenesis,
        application_id: [u8; 32],
        approved_scopes: Vec<String>,
        can_appoint_neter: bool,
        compensation_atoms_per_task: u128,
        tick: u64,
        founder_signature: [u8; 64],
    ) -> Result<NeterRecord, NeterError> {
        self.verify_founder(genesis)?;
        validate_scopes(&approved_scopes)?;
        let app_key = hex::encode(application_id);
        let application = self
            .applications
            .get(&app_key)
            .ok_or(NeterError::UnknownApplication)?;
        if approved_scopes
            .iter()
            .any(|scope| !application.requested_scopes.contains(scope))
        {
            return Err(NeterError::ScopeEscalation);
        }
        let founder_id = neter_id(&self.founder_public_key);
        let statement = ApplicationApprovalStatement {
            domain: "AWENET/NETER-APPLICATION-APPROVAL/v1",
            application_id,
            candidate_public_key: application.public_key,
            approved_scopes: &approved_scopes,
            can_appoint_neter,
            compensation_atoms_per_task,
            tick,
        };
        let bytes = serde_json::to_vec(&statement).map_err(|_| NeterError::InvalidFounderSignature)?;
        self.verify_founder_signature(&bytes, &founder_signature)?;
        let application = self
            .applications
            .remove(&app_key)
            .ok_or(NeterError::UnknownApplication)?;
        self.insert_record(
            application.public_key,
            application.display_name,
            approved_scopes,
            can_appoint_neter,
            compensation_atoms_per_task,
            founder_id,
            tick,
        )
    }

    /// Direct founder appointment, without requiring a prior application.
    pub fn appoint_by_founder(
        &mut self,
        genesis: &FounderGenesis,
        public_key: [u8; 32],
        display_name: &str,
        scopes: Vec<String>,
        can_appoint_neter: bool,
        compensation_atoms_per_task: u128,
        tick: u64,
        signature: [u8; 64],
    ) -> Result<NeterRecord, NeterError> {
        self.verify_founder(genesis)?;
        validate_scopes(&scopes)?;
        let founder_id = neter_id(&self.founder_public_key);
        let message = appointment_message(
            public_key,
            display_name,
            &scopes,
            can_appoint_neter,
            compensation_atoms_per_task,
            founder_id,
            tick,
        );
        self.verify_founder_signature(&message, &signature)?;
        self.insert_record(
            public_key,
            display_name.chars().take(80).collect(),
            scopes,
            can_appoint_neter,
            compensation_atoms_per_task,
            founder_id,
            tick,
        )
    }

    /// A Neter with explicit delegation may appoint a child only inside its
    /// own scopes. The child never receives delegation authority from a Neter.
    pub fn appoint_by_neter(
        &mut self,
        parent_id: [u8; 32],
        public_key: [u8; 32],
        display_name: &str,
        scopes: Vec<String>,
        compensation_atoms_per_task: u128,
        tick: u64,
        signature: [u8; 64],
    ) -> Result<NeterRecord, NeterError> {
        validate_scopes(&scopes)?;
        let parent = self
            .get(&parent_id)
            .filter(|record| record.status == NeterStatus::Active)
            .ok_or(NeterError::InactiveNeter)?;
        if !parent.can_appoint_neter {
            return Err(NeterError::DelegationNotGranted);
        }
        if scopes.iter().any(|scope| !parent.scopes.contains(scope)) {
            return Err(NeterError::ScopeEscalation);
        }
        let parent_key = VerifyingKey::from_bytes(&parent.public_key)
            .map_err(|_| NeterError::InvalidNeterSignature)?;
        let message = appointment_message(
            public_key,
            display_name,
            &scopes,
            false,
            compensation_atoms_per_task,
            parent_id,
            tick,
        );
        parent_key
            .verify(&message, &Signature::from_bytes(&signature))
            .map_err(|_| NeterError::InvalidNeterSignature)?;
        self.insert_record(
            public_key,
            display_name.chars().take(80).collect(),
            scopes,
            false,
            compensation_atoms_per_task,
            parent_id,
            tick,
        )
    }

    pub fn set_status_by_founder(
        &mut self,
        genesis: &FounderGenesis,
        target_id: [u8; 32],
        status: NeterStatus,
        tick: u64,
        signature: [u8; 64],
    ) -> Result<(), NeterError> {
        self.verify_founder(genesis)?;
        let message = serde_json::to_vec(&NeterStatusStatement {
            domain: "AWENET/NETER-STATUS/v1",
            neter_id: target_id,
            status: status.clone(),
            tick,
        })
        .map_err(|_| NeterError::InvalidFounderSignature)?;
        self.verify_founder_signature(&message, &signature)?;
        let record = self
            .neters
            .get_mut(&hex::encode(target_id))
            .ok_or(NeterError::UnknownNeter)?;
        record.status = status.clone();
        self.push_audit(
            tick,
            match status {
                NeterStatus::Active => "neter_reinstated",
                NeterStatus::Suspended => "neter_suspended",
                NeterStatus::Removed => "neter_removed",
            },
            neter_id(&self.founder_public_key),
            target_id,
        );
        Ok(())
    }

    pub fn set_compensation_by_founder(
        &mut self,
        genesis: &FounderGenesis,
        target_id: [u8; 32],
        compensation_atoms_per_task: u128,
        tick: u64,
        signature: [u8; 64],
    ) -> Result<(), NeterError> {
        self.verify_founder(genesis)?;
        let message = serde_json::to_vec(&CompensationStatement {
            domain: "AWENET/NETER-COMPENSATION/v1",
            neter_id: target_id,
            compensation_atoms_per_task,
            tick,
        })
        .map_err(|_| NeterError::InvalidFounderSignature)?;
        self.verify_founder_signature(&message, &signature)?;
        let record = self
            .neters
            .get_mut(&hex::encode(target_id))
            .ok_or(NeterError::UnknownNeter)?;
        record.compensation_atoms_per_task = compensation_atoms_per_task;
        self.push_audit(
            tick,
            "neter_compensation_changed",
            neter_id(&self.founder_public_key),
            target_id,
        );
        Ok(())
    }

    fn insert_record(
        &mut self,
        public_key: [u8; 32],
        display_name: String,
        scopes: Vec<String>,
        can_appoint_neter: bool,
        compensation_atoms_per_task: u128,
        appointed_by: [u8; 32],
        tick: u64,
    ) -> Result<NeterRecord, NeterError> {
        if self.neters.len() >= MAX_NETERS {
            return Err(NeterError::RegistryFull);
        }
        let id = neter_id(&public_key);
        if self.neters.contains_key(&hex::encode(id)) {
            return Err(NeterError::AlreadyExists);
        }
        let record = NeterRecord {
            neter_id: id,
            public_key,
            display_name,
            scopes,
            can_appoint_neter,
            compensation_atoms_per_task,
            status: NeterStatus::Active,
            appointed_by,
            created_tick: tick,
        };
        self.neters.insert(hex::encode(id), record.clone());
        self.push_audit(tick, "neter_appointed", appointed_by, id);
        Ok(record)
    }

    fn verify_founder(&self, genesis: &FounderGenesis) -> Result<(), NeterError> {
        genesis.verify().map_err(|_| NeterError::InvalidGenesis)?;
        if genesis.payload.founder_public_key != self.founder_public_key {
            return Err(NeterError::InvalidGenesis);
        }
        Ok(())
    }

    fn verify_founder_signature(
        &self,
        message: &[u8],
        signature: &[u8; 64],
    ) -> Result<(), NeterError> {
        let key = VerifyingKey::from_bytes(&self.founder_public_key)
            .map_err(|_| NeterError::InvalidFounderSignature)?;
        key.verify(message, &Signature::from_bytes(signature))
            .map_err(|_| NeterError::InvalidFounderSignature)
    }

    fn push_audit(&mut self, tick: u64, action: &str, actor: [u8; 32], target: [u8; 32]) {
        if self.audit.len() == MAX_NETER_AUDIT {
            self.audit.pop_front();
        }
        self.audit.push_back(NeterAuditRecord {
            sequence: self.next_audit_sequence,
            tick,
            action: action.into(),
            actor_id: actor,
            target_id: target,
        });
        self.next_audit_sequence = self.next_audit_sequence.saturating_add(1);
    }
}

fn validate_scopes(scopes: &[String]) -> Result<(), NeterError> {
    if scopes.is_empty()
        || scopes.len() > MAX_NETER_SCOPES
        || scopes.iter().any(|scope| {
            scope.is_empty()
                || scope.len() > 64
                || !scope
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-' | '.'))
        })
    {
        return Err(NeterError::InvalidScopes);
    }
    let mut unique = scopes.to_vec();
    unique.sort();
    unique.dedup();
    if unique.len() != scopes.len() {
        return Err(NeterError::InvalidScopes);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{Identity, Username};
    use rand::{rngs::OsRng, RngCore};
    use std::{fs, path::PathBuf};

    fn setup() -> (Identity, FounderGenesis, NeterRegistry, PathBuf) {
        let founder = Identity::generate(Username::new("AWENeter").unwrap());
        let secret = vec![0xC9; 48];
        let digest = crate::founder::bootstrap_digest(&secret);
        let mut random = [0u8; 8];
        OsRng.fill_bytes(&mut random);
        let path = std::env::temp_dir().join(format!("awenet-neter-{}.json", hex::encode(random)));
        let genesis = FounderGenesis::initialize_once(
            &path, &founder, &secret, digest, crate::founder::FounderRules::default(),
        ).unwrap();
        let registry = NeterRegistry::new(&genesis).unwrap();
        (founder, genesis, registry, path)
    }

    #[test]
    fn founder_can_appoint_and_change_neter_status_and_pay() {
        let (founder, genesis, mut registry, path) = setup();
        let neter = Identity::generate(Username::new("moderator").unwrap());
        let scopes = vec!["awenetwork:tasks".into(), "awemessenger:moderation".into()];
        let founder_id = neter_id(&founder.public.public_key);
        let signature = founder.sign(&appointment_message(
            neter.public.public_key, "Moderator", &scopes, true, 500, founder_id, 10,
        ));
        let record = registry.appoint_by_founder(
            &genesis, neter.public.public_key, "Moderator", scopes, true, 500, 10, signature,
        ).unwrap();
        assert_eq!(registry.active().len(), 1);

        let compensation = serde_json::to_vec(&CompensationStatement {
            domain: "AWENET/NETER-COMPENSATION/v1",
            neter_id: record.neter_id, compensation_atoms_per_task: 750, tick: 11,
        }).unwrap();
        registry.set_compensation_by_founder(
            &genesis, record.neter_id, 750, 11, founder.sign(&compensation),
        ).unwrap();
        assert_eq!(registry.get(&record.neter_id).unwrap().compensation_atoms_per_task, 750);

        let status = serde_json::to_vec(&NeterStatusStatement {
            domain: "AWENET/NETER-STATUS/v1", neter_id: record.neter_id,
            status: NeterStatus::Suspended, tick: 12,
        }).unwrap();
        registry.set_status_by_founder(
            &genesis, record.neter_id, NeterStatus::Suspended, 12, founder.sign(&status),
        ).unwrap();
        assert!(registry.active().is_empty());
        assert_eq!(registry.audit().count(), 3);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn application_requires_applicant_proof_and_founder_approval() {
        let (founder, genesis, mut registry, path) = setup();
        let applicant = Identity::generate(Username::new("applicant").unwrap());
        let scopes = vec!["awenet:tasks".into()];
        let display = "Task reviewer";
        let message = "I can review AWENET tasks";
        let proof = application_proof_message(
            applicant.public.public_key, display, &scopes, message, 4,
        );
        let app = registry.submit_application(
            applicant.public.public_key,
            display,
            scopes.clone(),
            message,
            4,
            applicant.sign(&proof),
        ).unwrap();
        let approval = serde_json::to_vec(&ApplicationApprovalStatement {
            domain: "AWENET/NETER-APPLICATION-APPROVAL/v1",
            application_id: app.application_id,
            candidate_public_key: applicant.public.public_key,
            approved_scopes: &scopes,
            can_appoint_neter: false,
            compensation_atoms_per_task: 25,
            tick: 5,
        }).unwrap();
        let record = registry.approve_application(
            &genesis,
            app.application_id,
            scopes,
            false,
            25,
            5,
            founder.sign(&approval),
        ).unwrap();
        assert_eq!(registry.get(&record.neter_id).unwrap().status, NeterStatus::Active);
        assert_eq!(registry.applications().count(), 0);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn delegation_requires_founder_grant_and_cannot_escalate_scopes() {
        let (founder, genesis, mut registry, path) = setup();
        let parent = Identity::generate(Username::new("parent").unwrap());
        let scopes = vec!["awenetwork:tasks".into()];
        let founder_id = neter_id(&founder.public.public_key);
        let sig = founder.sign(&appointment_message(
            parent.public.public_key, "Parent", &scopes, false, 0, founder_id, 1,
        ));
        let parent_record = registry.appoint_by_founder(
            &genesis, parent.public.public_key, "Parent", scopes.clone(), false, 0, 1, sig,
        ).unwrap();
        let child = Identity::generate(Username::new("child").unwrap());
        let child_scopes = vec!["awenetwork:tasks".into()];
        let child_message = appointment_message(
            child.public.public_key, "Child", &child_scopes, false, 0, parent_record.neter_id, 2,
        );
        assert_eq!(
            registry.appoint_by_neter(
                parent_record.neter_id, child.public.public_key, "Child", child_scopes.clone(),
                0, 2, parent.sign(&child_message),
            ),
            Err(NeterError::DelegationNotGranted)
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn neter_cannot_delegate_outside_its_scope() {
        let (founder, genesis, mut registry, path) = setup();
        let parent = Identity::generate(Username::new("parent").unwrap());
        let scopes = vec!["awenetwork:tasks".into()];
        let founder_id = neter_id(&founder.public.public_key);
        let sig = founder.sign(&appointment_message(
            parent.public.public_key, "Parent", &scopes, true, 0, founder_id, 1,
        ));
        let parent_record = registry.appoint_by_founder(
            &genesis, parent.public.public_key, "Parent", scopes, true, 0, 1, sig,
        ).unwrap();
        let child = Identity::generate(Username::new("child").unwrap());
        let child_scopes = vec!["awemessenger:admin".into()];
        let msg = appointment_message(
            child.public.public_key, "Child", &child_scopes, false, 0, parent_record.neter_id, 2,
        );
        assert_eq!(
            registry.appoint_by_neter(
                parent_record.neter_id, child.public.public_key, "Child", child_scopes, 0, 2,
                parent.sign(&msg),
            ),
            Err(NeterError::ScopeEscalation)
        );
        fs::remove_file(path).unwrap();
    }
}
