//! Multi-node usage verification for the AWENET resource economy.
//!
//! The resource economy settles a contribution period only when
//! independent signals agree on the node's usage, and the network-side
//! signal needs at least MIN_INDEPENDENT_VERIFIERS distinct valid
//! verifier signatures with a conservative per-resource minimum
//! consensus. Until now those reports had no network life: a node could
//! collect them in memory, but nothing enrolled verifiers, opened
//! attestation windows or carried a peer's signed report from one node
//! to another.
//!
//! This module is the network half of that contract:
//!
//! - a founder-gated `VerifierRegistry`: only the founder key installed
//!   at genesis can enroll or retire verifier nodes, every certificate is
//!   founder-signed, and any peer can validate it offline;
//! - `AttestationWindow`s: a contributing node opens one bounded window
//!   per settlement period with a report deadline, and reports are
//!   accepted only for open windows, from enrolled verifiers, never from
//!   the contributing node itself and never twice from the same verifier;
//! - `PeerObservation`: what one node can honestly measure about another
//!   node over the network (bytes exchanged with it, availability
//!   observed under direct contact) turned into a signed usage report,
//!   so a remote verifier never invents counters it cannot see.
//!
//! The collected reports build the same `UsageVerification` the
//! resource meter produces, so settlement keeps its existing
//! guarantees: at least two distinct verifier signatures and the
//! per-resource minimum across them. Nothing here mints ONECOIN, changes
//! reward rates or weakens the founder's exclusive rights.

use crate::identity::{AweId, Identity};
use crate::resource_economy::{MeasuredUsage, UsageEvidence};
use crate::resource_meter::{SignedUsageReport, UsageVerification, MAX_VERIFIER_REPORTS};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
};
use thiserror::Error;
use zeroize::Zeroize;

pub const MAX_VERIFIERS: usize = 256;
pub const MAX_OPEN_WINDOWS: usize = 4_096;
/// A settlement period cannot be longer than 30 days.
pub const MAX_WINDOW_PERIOD_SECS: u64 = 30 * 24 * 3_600;
/// Verifiers may still deliver reports this long after the period ends.
pub const REPORT_GRACE_SECS: u64 = 24 * 3_600;
/// Closed windows and their reports are kept this long for audit, then
/// pruned by `VerifierNet::close_expired`.
pub const CLOSED_WINDOW_RETENTION_SECS: u64 = 30 * 24 * 3_600;
pub const VERIFIER_NET_STORE_VERSION: u16 = 1;
pub const MAX_VERIFIER_NET_FILE_BYTES: u64 = 64 * 1024 * 1024;
const STORE_CONTEXT: &[u8] = b"AWENET-VERIFIER-NET-STORE/v1\0";

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum VerifierNetError {
    #[error("only the AWENET founder may manage the verifier registry")]
    NotFounder,
    #[error("verifier registry is full (max {MAX_VERIFIERS} verifiers)")]
    RegistryFull,
    #[error("this verifier is already enrolled")]
    AlreadyEnrolled,
    #[error("this verifier is not enrolled")]
    UnknownVerifier,
    #[error("the settlement period is invalid")]
    InvalidPeriod,
    #[error("the report deadline for this period has already passed")]
    WindowExpired,
    #[error("an attestation window is already open for this node and period")]
    WindowAlreadyOpen,
    #[error("no attestation window is open for this node and period")]
    UnknownWindow,
    #[error("the attestation window is closed to further reports")]
    WindowClosed,
    #[error("attestation window registry is full (max {MAX_OPEN_WINDOWS} windows)")]
    TooManyWindows,
    #[error("reports for this attestation window are full (max {MAX_VERIFIER_REPORTS} reports)")]
    WindowFull,
    #[error("a node cannot verify its own contribution")]
    SelfAttestation,
    #[error("this verifier already reported for this window")]
    DuplicateVerifier,
    #[error("the usage report signature is invalid")]
    InvalidSignature,
    #[error("the verifier registry sequence is exhausted")]
    SequenceExhausted,
    #[error("the verifier registry is inconsistent or contains an invalid certificate")]
    InvalidRegistry,
    #[error("the verifier network snapshot is inconsistent")]
    InvalidState,
    #[error("the verifier network belongs to another founder")]
    WrongFounder,
    #[error("the verifier network store version is unsupported")]
    InvalidStoreVersion,
    #[error("the verifier network store exceeds the maximum size")]
    StoreTooLarge,
    #[error("verifier network serialization failed: {0}")]
    Serialization(String),
    #[error("verifier network I/O failed: {0}")]
    Io(String),
}

/// A founder-signed enrollment certificate for one verifier node. Every
/// peer can validate it offline against the founder public key installed
/// at genesis; the claimed AWE ID must also really derive from the
/// verifier's public key.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerifierCertificate {
    pub verifier: AweId,
    pub public_key: [u8; 32],
    pub enrolled_at_unix: u64,
    pub retired_at_unix: Option<u64>,
    pub sequence: u64,
    #[serde(with = "crate::serde_bytes_64")]
    pub founder_signature: [u8; 64],
}

impl VerifierCertificate {
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut unsigned = self.clone();
        unsigned.founder_signature = [0u8; 64];
        serde_json::to_vec(&unsigned).expect("verifier certificate serialization")
    }

    /// The certificate counts only when the founder signature is valid
    /// and the claimed AWE ID really derives from the public key.
    pub fn is_valid(&self, founder_public_key: &[u8; 32]) -> bool {
        let founder_signed = Identity::verify(
            founder_public_key,
            &self.signing_bytes(),
            &self.founder_signature,
        );
        founder_signed && self.verifier == AweId::from_public_key(&self.public_key)
    }

    pub fn is_active(&self) -> bool {
        self.retired_at_unix.is_none()
    }
}

/// The verifier roster of the resource economy. The founder key is
/// installed once at genesis and is immutable afterwards; there is
/// deliberately no way to replace it through profile fields or API
/// calls, mirroring `FounderAuthority`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerifierRegistry {
    founder_public_key: [u8; 32],
    installed_at_unix: u64,
    next_sequence: u64,
    verifiers: Vec<VerifierCertificate>,
}

impl VerifierRegistry {
    pub fn install(founder: &Identity, at_unix: u64) -> Self {
        Self {
            founder_public_key: founder.public.public_key,
            installed_at_unix: at_unix,
            next_sequence: 1,
            verifiers: Vec::new(),
        }
    }

    pub fn founder_public_key(&self) -> &[u8; 32] {
        &self.founder_public_key
    }

    pub fn installed_at_unix(&self) -> u64 {
        self.installed_at_unix
    }

    pub fn certificate(&self, verifier_public_key: &[u8; 32]) -> Option<&VerifierCertificate> {
        self.verifiers
            .iter()
            .find(|certificate| certificate.public_key == *verifier_public_key)
    }

    /// Returns true only when the stored certificate is founder-signed and
    /// its AWE ID is derived from the certified public key. This remains
    /// fail-closed even when a registry was deserialized from untrusted bytes.
    pub fn is_certified(&self, verifier_public_key: &[u8; 32]) -> bool {
        self.certificate(verifier_public_key)
            .is_some_and(|certificate| certificate.is_valid(&self.founder_public_key))
    }

    pub fn is_active(&self, verifier_public_key: &[u8; 32]) -> bool {
        self.certificate(verifier_public_key)
            .is_some_and(|certificate| {
                certificate.is_active() && certificate.is_valid(&self.founder_public_key)
            })
    }

    pub fn active_verifiers(&self) -> usize {
        self.verifiers
            .iter()
            .filter(|certificate| {
                certificate.is_active() && certificate.is_valid(&self.founder_public_key)
            })
            .count()
    }

    /// Validates a deserialized or persisted registry before it is used.
    /// Every certificate must be founder-signed, unique and below the next
    /// sequence number; retirement cannot predate enrollment.
    pub fn validate(&self) -> Result<(), VerifierNetError> {
        if self.next_sequence == 0 || self.verifiers.len() > MAX_VERIFIERS {
            return Err(VerifierNetError::InvalidRegistry);
        }
        let mut public_keys = BTreeSet::new();
        let mut sequences = BTreeSet::new();
        for certificate in &self.verifiers {
            if certificate.sequence == 0
                || certificate.sequence >= self.next_sequence
                || !public_keys.insert(certificate.public_key)
                || !sequences.insert(certificate.sequence)
                || !certificate.is_valid(&self.founder_public_key)
                || certificate
                    .retired_at_unix
                    .is_some_and(|retired| retired < certificate.enrolled_at_unix)
            {
                return Err(VerifierNetError::InvalidRegistry);
            }
        }
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.verifiers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.verifiers.is_empty()
    }

    /// Enrolls a verifier node. Only the founder key installed at
    /// genesis can do this; the certificate is founder-signed so every
    /// peer can validate it offline. A retired verifier can be
    /// re-enrolled with a fresh sequence.
    pub fn enroll(
        &mut self,
        founder: &Identity,
        verifier: &Identity,
        at_unix: u64,
    ) -> Result<VerifierCertificate, VerifierNetError> {
        if founder.public.public_key != self.founder_public_key {
            return Err(VerifierNetError::NotFounder);
        }
        self.validate()?;
        if at_unix < self.installed_at_unix {
            return Err(VerifierNetError::InvalidState);
        }
        if let Some(existing) = self.certificate(&verifier.public.public_key) {
            if existing.is_active() {
                return Err(VerifierNetError::AlreadyEnrolled);
            }
            if existing
                .retired_at_unix
                .is_some_and(|retired| at_unix < retired)
            {
                return Err(VerifierNetError::InvalidState);
            }
        } else if self.verifiers.len() >= MAX_VERIFIERS {
            return Err(VerifierNetError::RegistryFull);
        }
        let next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(VerifierNetError::SequenceExhausted)?;
        let mut certificate = VerifierCertificate {
            verifier: verifier.public.awe_id.clone(),
            public_key: verifier.public.public_key,
            enrolled_at_unix: at_unix,
            retired_at_unix: None,
            sequence: self.next_sequence,
            founder_signature: [0u8; 64],
        };
        certificate.founder_signature = founder.sign(&certificate.signing_bytes());
        self.next_sequence = next_sequence;
        self.verifiers
            .retain(|existing| existing.public_key != certificate.public_key);
        self.verifiers.push(certificate.clone());
        Ok(certificate)
    }

    /// Retires a verifier: retired verifiers stop being accepted for new
    /// reports. Retiring an already retired verifier returns its existing
    /// certificate unchanged.
    pub fn retire(
        &mut self,
        founder: &Identity,
        verifier_public_key: &[u8; 32],
        at_unix: u64,
    ) -> Result<VerifierCertificate, VerifierNetError> {
        if founder.public.public_key != self.founder_public_key {
            return Err(VerifierNetError::NotFounder);
        }
        self.validate()?;
        let existing = match self.certificate(verifier_public_key) {
            Some(certificate) => certificate.clone(),
            None => return Err(VerifierNetError::UnknownVerifier),
        };
        if !existing.is_active() {
            return Ok(existing);
        }
        if at_unix < existing.enrolled_at_unix {
            return Err(VerifierNetError::InvalidState);
        }
        let next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(VerifierNetError::SequenceExhausted)?;
        let mut certificate = existing;
        certificate.retired_at_unix = Some(at_unix);
        certificate.sequence = self.next_sequence;
        certificate.founder_signature = [0u8; 64];
        certificate.founder_signature = founder.sign(&certificate.signing_bytes());
        self.next_sequence = next_sequence;
        self.verifiers
            .retain(|kept| kept.public_key != *verifier_public_key);
        self.verifiers.push(certificate.clone());
        Ok(certificate)
    }
}

/// One settlement period of one contributing node, open for verifier
/// reports until the deadline (period end plus a bounded grace).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttestationWindow {
    pub node: AweId,
    pub period_start_unix: u64,
    pub period_end_unix: u64,
    pub opened_at_unix: u64,
    pub report_deadline_unix: u64,
    pub closed: bool,
}

fn window_key(node: &AweId, period_start_unix: u64) -> String {
    format!("{}:{}", node.to_hex(), period_start_unix)
}

/// What one node can honestly observe about another node over the
/// network in one settlement period: the bytes the pair exchanged and
/// the time the node answered direct contact. A remote verifier cannot
/// read another host's CPU, GPU, RAM or storage counters, so a report
/// built from an observation carries only the remotely observable
/// resources and leaves the rest at zero; the settlement's per-resource
/// minimum consensus then pays only for what verifiers actually saw.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerObservation {
    pub bytes_exchanged: u64,
    pub observed_available_secs: u64,
}

impl PeerObservation {
    pub fn into_usage(self) -> MeasuredUsage {
        MeasuredUsage {
            cpu_core_ticks: 0,
            gpu_unit_ticks: 0,
            ram_mb_hours: 0,
            storage_gb_hours: 0,
            bandwidth_gb: self.bytes_exchanged / 1_000_000_000,
            uptime_hours: self.observed_available_secs / 3_600,
        }
    }

    pub fn into_report(
        self,
        verifier: &Identity,
        node: AweId,
        period_start_unix: u64,
        period_end_unix: u64,
    ) -> SignedUsageReport {
        SignedUsageReport::new(
            verifier,
            node,
            period_start_unix,
            period_end_unix,
            self.into_usage(),
        )
    }
}

/// The multi-node usage verification state of one AWENET node: the
/// founder-gated verifier registry, the open attestation windows and
/// the verifier reports collected for each window. Reports are accepted
/// only through `submit_report`, which enforces every rule before a
/// report counts towards a settlement consensus.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerifierNet {
    registry: VerifierRegistry,
    windows: BTreeMap<String, AttestationWindow>,
    reports: BTreeMap<String, Vec<SignedUsageReport>>,
}

impl VerifierNet {
    pub fn new(founder: &Identity, at_unix: u64) -> Self {
        Self {
            registry: VerifierRegistry::install(founder, at_unix),
            windows: BTreeMap::new(),
            reports: BTreeMap::new(),
        }
    }

    pub fn registry(&self) -> &VerifierRegistry {
        &self.registry
    }

    pub fn registry_mut(&mut self) -> &mut VerifierRegistry {
        &mut self.registry
    }

    pub fn enroll_verifier(
        &mut self,
        founder: &Identity,
        verifier: &Identity,
        at_unix: u64,
    ) -> Result<VerifierCertificate, VerifierNetError> {
        self.registry.enroll(founder, verifier, at_unix)
    }

    pub fn retire_verifier(
        &mut self,
        founder: &Identity,
        verifier_public_key: &[u8; 32],
        at_unix: u64,
    ) -> Result<VerifierCertificate, VerifierNetError> {
        self.registry.retire(founder, verifier_public_key, at_unix)
    }

    /// Opens the attestation window for one contribution period. The
    /// period must be well formed and not longer than
    /// MAX_WINDOW_PERIOD_SECS, and the report deadline (period end plus
    /// grace) must not already be in the past.
    pub fn open_window(
        &mut self,
        node: &AweId,
        period_start_unix: u64,
        period_end_unix: u64,
        now_unix: u64,
    ) -> Result<AttestationWindow, VerifierNetError> {
        self.validate()?;
        let _duration = period_end_unix
            .checked_sub(period_start_unix)
            .filter(|duration| *duration > 0 && *duration <= MAX_WINDOW_PERIOD_SECS)
            .ok_or(VerifierNetError::InvalidPeriod)?;
        let report_deadline_unix = period_end_unix
            .checked_add(REPORT_GRACE_SECS)
            .ok_or(VerifierNetError::InvalidPeriod)?;
        if now_unix > report_deadline_unix {
            return Err(VerifierNetError::WindowExpired);
        }
        let key = window_key(node, period_start_unix);
        if self.windows.contains_key(&key) {
            return Err(VerifierNetError::WindowAlreadyOpen);
        }
        if self.windows.len() >= MAX_OPEN_WINDOWS {
            return Err(VerifierNetError::TooManyWindows);
        }
        let window = AttestationWindow {
            node: node.clone(),
            period_start_unix,
            period_end_unix,
            opened_at_unix: now_unix,
            report_deadline_unix,
            closed: false,
        };
        self.windows.insert(key, window.clone());
        Ok(window)
    }

    pub fn window(&self, node: &AweId, period_start_unix: u64) -> Option<&AttestationWindow> {
        self.windows.get(&window_key(node, period_start_unix))
    }

    pub fn reports_for(&self, node: &AweId, period_start_unix: u64) -> &[SignedUsageReport] {
        self.reports
            .get(&window_key(node, period_start_unix))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Accepts one signed report from a peer verifier. Every acceptance
    /// rule is enforced here, before the report can ever count towards a
    /// settlement: a valid verifier signature, an open window for this
    /// exact period, an enrolled active verifier, no self-attestation, no
    /// duplicate verifier and bounded storage.
    pub fn submit_report(
        &mut self,
        report: SignedUsageReport,
        now_unix: u64,
    ) -> Result<(), VerifierNetError> {
        self.validate()?;
        if !report.verify() {
            return Err(VerifierNetError::InvalidSignature);
        }
        let key = window_key(&report.node, report.period_start_unix);
        let window = match self.windows.get(&key) {
            Some(window) => window,
            None => return Err(VerifierNetError::UnknownWindow),
        };
        if window.period_end_unix != report.period_end_unix {
            return Err(VerifierNetError::UnknownWindow);
        }
        if window.closed || now_unix > window.report_deadline_unix {
            return Err(VerifierNetError::WindowClosed);
        }
        if !self.registry.is_active(&report.verifier_public_key) {
            return Err(VerifierNetError::UnknownVerifier);
        }
        if AweId::from_public_key(&report.verifier_public_key) == report.node {
            return Err(VerifierNetError::SelfAttestation);
        }
        if let Some(reports) = self.reports.get(&key) {
            if reports.len() >= MAX_VERIFIER_REPORTS {
                return Err(VerifierNetError::WindowFull);
            }
            if reports
                .iter()
                .any(|existing| existing.verifier_public_key == report.verifier_public_key)
            {
                return Err(VerifierNetError::DuplicateVerifier);
            }
        }
        self.reports.entry(key).or_default().push(report);
        Ok(())
    }

    /// The collected reports for one window as the same
    /// `UsageVerification` the resource meter produces, so settlement
    /// code sees one shape regardless of where reports came from.
    pub fn verification(
        &self,
        node: &AweId,
        period_start_unix: u64,
        period_end_unix: u64,
    ) -> UsageVerification {
        let mut verification = UsageVerification::default();
        if self.validate().is_err() {
            return verification;
        }
        let mut seen = BTreeSet::new();
        for report in self.reports_for(node, period_start_unix) {
            if report.node == *node
                && report.period_end_unix == period_end_unix
                && report.verify()
                && self.registry.is_certified(&report.verifier_public_key)
                && AweId::from_public_key(&report.verifier_public_key) != *node
                && seen.insert(report.verifier_public_key)
            {
                verification.add(report.clone());
            }
        }
        verification
    }

    pub fn consensus_usage(
        &self,
        node: &AweId,
        period_start_unix: u64,
        period_end_unix: u64,
    ) -> Option<MeasuredUsage> {
        self.verification(node, period_start_unix, period_end_unix)
            .consensus_usage(node, period_start_unix, period_end_unix)
    }

    /// True when the collected reports for this window reach the
    /// independent verifier consensus settlement requires.
    pub fn has_consensus(
        &self,
        node: &AweId,
        period_start_unix: u64,
        period_end_unix: u64,
    ) -> bool {
        self.consensus_usage(node, period_start_unix, period_end_unix)
            .is_some()
    }

    /// Settlement evidence for one window: the node's own metered
    /// report, the verifier consensus as the network measurement and
    /// the delivered work results.
    pub fn evidence(
        &self,
        node_report: MeasuredUsage,
        node: &AweId,
        period_start_unix: u64,
        period_end_unix: u64,
        delivered_results: Option<MeasuredUsage>,
    ) -> Option<UsageEvidence> {
        self.verification(node, period_start_unix, period_end_unix)
            .evidence(
                node_report,
                node,
                period_start_unix,
                period_end_unix,
                delivered_results,
            )
    }

    /// Validates the complete state before it is persisted, restored or used
    /// to accept a new report. Reports from a verifier that was later retired
    /// remain auditable, but their certificate must still be founder-signed.
    pub fn validate(&self) -> Result<(), VerifierNetError> {
        self.registry.validate()?;
        if self.windows.len() > MAX_OPEN_WINDOWS {
            return Err(VerifierNetError::InvalidState);
        }

        for (key, window) in &self.windows {
            let _duration = window
                .period_end_unix
                .checked_sub(window.period_start_unix)
                .filter(|duration| *duration > 0 && *duration <= MAX_WINDOW_PERIOD_SECS)
                .ok_or(VerifierNetError::InvalidState)?;
            let expected_deadline = window
                .period_end_unix
                .checked_add(REPORT_GRACE_SECS)
                .ok_or(VerifierNetError::InvalidState)?;
            if key != &window_key(&window.node, window.period_start_unix)
                || window.report_deadline_unix != expected_deadline
                || window.opened_at_unix > window.report_deadline_unix
            {
                return Err(VerifierNetError::InvalidState);
            }
        }

        for (key, reports) in &self.reports {
            let window = self
                .windows
                .get(key)
                .ok_or(VerifierNetError::InvalidState)?;
            if reports.len() > MAX_VERIFIER_REPORTS {
                return Err(VerifierNetError::InvalidState);
            }
            let mut seen = BTreeSet::new();
            for report in reports {
                let certificate_valid = self
                    .registry
                    .certificate(&report.verifier_public_key)
                    .is_some_and(|certificate| {
                        certificate.is_valid(self.registry.founder_public_key())
                    });
                if !report.verify()
                    || !certificate_valid
                    || report.node != window.node
                    || report.period_start_unix != window.period_start_unix
                    || report.period_end_unix != window.period_end_unix
                    || AweId::from_public_key(&report.verifier_public_key) == report.node
                    || !seen.insert(report.verifier_public_key)
                {
                    return Err(VerifierNetError::InvalidState);
                }
            }
        }
        Ok(())
    }

    /// Closes windows past their report deadline and prunes closed
    /// windows (with their reports) past the retention horizon.
    /// Returns the number of windows removed.
    pub fn close_expired(&mut self, now_unix: u64) -> usize {
        for window in self.windows.values_mut() {
            if !window.closed && now_unix > window.report_deadline_unix {
                window.closed = true;
            }
        }
        let mut expired = Vec::new();
        for (key, window) in &self.windows {
            let forgotten_at = window
                .report_deadline_unix
                .saturating_add(CLOSED_WINDOW_RETENTION_SECS);
            if window.closed && now_unix > forgotten_at {
                expired.push(key.clone());
            }
        }
        let removed = expired.len();
        for key in expired {
            self.windows.remove(&key);
            self.reports.remove(&key);
        }
        removed
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct SignedVerifierNetFile {
    version: u16,
    net: VerifierNet,
    #[serde(with = "crate::serde_bytes_64")]
    signature: [u8; 64],
}

#[derive(Serialize)]
struct VerifierNetPayload<'a> {
    version: u16,
    net: &'a VerifierNet,
}

fn store_signing_bytes(net: &VerifierNet) -> Result<Vec<u8>, VerifierNetError> {
    let payload = VerifierNetPayload {
        version: VERIFIER_NET_STORE_VERSION,
        net,
    };
    let serialized = serde_json::to_vec(&payload)
        .map_err(|error| VerifierNetError::Serialization(error.to_string()))?;
    let mut bytes = Vec::with_capacity(STORE_CONTEXT.len() + serialized.len());
    bytes.extend_from_slice(STORE_CONTEXT);
    bytes.extend_from_slice(&serialized);
    Ok(bytes)
}

fn atomic_write(path: &Path, encoded: &[u8]) -> Result<(), VerifierNetError> {
    if encoded.len() as u64 > MAX_VERIFIER_NET_FILE_BYTES {
        return Err(VerifierNetError::StoreTooLarge);
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|error| VerifierNetError::Io(error.to_string()))?;
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
            return Err(VerifierNetError::Io(
                "secure random generation failed".into(),
            ));
        }
        let temporary = parent.join(format!(
            ".awenet-verifier-net-{}-{}.tmp",
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
            Err(error) => return Err(VerifierNetError::Io(error.to_string())),
        }
    }
    let Some((temporary, mut file)) = opened else {
        return Err(VerifierNetError::Io(
            "could not allocate a unique temporary verifier-net file".into(),
        ));
    };
    if let Err(error) = file.write_all(encoded).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(&temporary);
        return Err(VerifierNetError::Io(error.to_string()));
    }
    drop(file);
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(VerifierNetError::Io(error.to_string()));
    }
    #[cfg(unix)]
    if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

/// Atomically saves a founder-signed verifier network snapshot. The founder
/// private key is never written to disk; only the public registry, windows
/// and signed reports are included in the snapshot.
pub fn save(path: &Path, net: &VerifierNet, founder: &Identity) -> Result<(), VerifierNetError> {
    if net.registry.founder_public_key != founder.public.public_key {
        return Err(VerifierNetError::WrongFounder);
    }
    net.validate()?;
    let file = SignedVerifierNetFile {
        version: VERIFIER_NET_STORE_VERSION,
        net: net.clone(),
        signature: founder.sign(&store_signing_bytes(net)?),
    };
    let encoded = serde_json::to_vec_pretty(&file)
        .map_err(|error| VerifierNetError::Serialization(error.to_string()))?;
    atomic_write(path, &encoded)
}

/// Loads a snapshot only when its outer founder signature, registry
/// certificates, attestation windows and stored reports all validate.
pub fn load(path: &Path, founder: &Identity) -> Result<VerifierNet, VerifierNetError> {
    let file = fs::File::open(path).map_err(|error| VerifierNetError::Io(error.to_string()))?;
    let mut bytes = Vec::new();
    file.take(MAX_VERIFIER_NET_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| VerifierNetError::Io(error.to_string()))?;
    if bytes.len() as u64 > MAX_VERIFIER_NET_FILE_BYTES {
        return Err(VerifierNetError::StoreTooLarge);
    }
    let signed: SignedVerifierNetFile = serde_json::from_slice(&bytes)
        .map_err(|error| VerifierNetError::Serialization(error.to_string()))?;
    if signed.version != VERIFIER_NET_STORE_VERSION {
        return Err(VerifierNetError::InvalidStoreVersion);
    }
    if signed.net.registry.founder_public_key != founder.public.public_key {
        return Err(VerifierNetError::WrongFounder);
    }
    let message = store_signing_bytes(&signed.net)?;
    if !Identity::verify(&founder.public.public_key, &message, &signed.signature) {
        return Err(VerifierNetError::InvalidSignature);
    }
    signed.net.validate()?;
    Ok(signed.net)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Username;
    use crate::onecoin::{OnecoinLedger, ATOMS_PER_COIN};
    use crate::resource_economy::{ResourceEconomy, ResourceEconomyError, ResourceLimits};

    fn identity(name: &str) -> Identity {
        Identity::generate(Username::new(name).unwrap())
    }

    fn period_usage() -> MeasuredUsage {
        MeasuredUsage {
            cpu_core_ticks: 100,
            gpu_unit_ticks: 0,
            ram_mb_hours: 0,
            storage_gb_hours: 0,
            bandwidth_gb: 5,
            uptime_hours: 2,
        }
    }

    fn limits() -> ResourceLimits {
        ResourceLimits {
            cpu_core_ticks: 100,
            gpu_unit_ticks: 0,
            ram_mb_hours: 1_000,
            storage_gb_hours: 0,
            bandwidth_gb: 10,
            uptime_hours: 24,
        }
    }

    #[test]
    fn only_the_founder_can_enroll_verifiers() {
        let founder = identity("net-founder");
        let imposter = identity("net-imposter");
        let verifier = identity("net-verifier");
        let mut net = VerifierNet::new(&founder, 0);
        assert!(matches!(
            net.enroll_verifier(&imposter, &verifier, 1).unwrap_err(),
            VerifierNetError::NotFounder
        ));
        assert!(!net.registry().is_active(&verifier.public.public_key));
        let certificate = net.enroll_verifier(&founder, &verifier, 2).unwrap();
        assert!(certificate.is_valid(&founder.public.public_key));
        assert!(certificate.is_active());
        assert!(net.registry().is_active(&verifier.public.public_key));
        assert_eq!(net.registry().active_verifiers(), 1);
        assert!(matches!(
            net.enroll_verifier(&founder, &verifier, 3).unwrap_err(),
            VerifierNetError::AlreadyEnrolled
        ));
    }

    #[test]
    fn forged_certificates_do_not_validate() {
        let founder = identity("forge-founder");
        let imposter = identity("forge-imposter");
        let verifier = identity("forge-verifier");
        let mut certificate = VerifierCertificate {
            verifier: verifier.public.awe_id.clone(),
            public_key: verifier.public.public_key,
            enrolled_at_unix: 1,
            retired_at_unix: None,
            sequence: 1,
            founder_signature: [0u8; 64],
        };
        certificate.founder_signature = imposter.sign(&certificate.signing_bytes());
        assert!(!certificate.is_valid(&founder.public.public_key));
        // a certificate whose claimed AWE ID does not derive from the
        // verifier key is invalid even when signed by the founder
        let mut mismatched = certificate;
        mismatched.verifier = founder.public.awe_id.clone();
        mismatched.founder_signature = founder.sign(&mismatched.signing_bytes());
        assert!(!mismatched.is_valid(&founder.public.public_key));
    }

    #[test]
    fn retired_verifiers_are_no_longer_accepted() {
        let founder = identity("retire-founder");
        let node = identity("retire-node");
        let verifier = identity("retire-verifier");
        let mut net = VerifierNet::new(&founder, 0);
        net.enroll_verifier(&founder, &verifier, 1).unwrap();
        let retired = net
            .retire_verifier(&founder, &verifier.public.public_key, 2)
            .unwrap();
        assert!(!retired.is_active());
        assert!(retired.is_valid(&founder.public.public_key));
        assert!(!net.registry().is_active(&verifier.public.public_key));
        // retiring twice is idempotent
        let again = net
            .retire_verifier(&founder, &verifier.public.public_key, 3)
            .unwrap();
        assert_eq!(again, retired);
        // an imposter cannot retire verifiers either
        let imposter = identity("retire-imposter");
        let other = identity("retire-other");
        net.enroll_verifier(&founder, &other, 4).unwrap();
        assert!(matches!(
            net.retire_verifier(&imposter, &other.public.public_key, 5)
                .unwrap_err(),
            VerifierNetError::NotFounder
        ));
        // and the retired verifier's reports are rejected
        net.open_window(&node.public.awe_id, 0, 3_600, 3_600)
            .unwrap();
        let report = SignedUsageReport::new(
            &verifier,
            node.public.awe_id.clone(),
            0,
            3_600,
            period_usage(),
        );
        assert!(matches!(
            net.submit_report(report, 3_600).unwrap_err(),
            VerifierNetError::UnknownVerifier
        ));
    }

    #[test]
    fn windows_validate_periods_and_open_once() {
        let founder = identity("window-founder");
        let node = identity("window-node");
        let mut net = VerifierNet::new(&founder, 0);
        assert!(matches!(
            net.open_window(&node.public.awe_id, 1_000, 1_000, 1_000)
                .unwrap_err(),
            VerifierNetError::InvalidPeriod
        ));
        assert!(matches!(
            net.open_window(&node.public.awe_id, 0, MAX_WINDOW_PERIOD_SECS + 2, 0)
                .unwrap_err(),
            VerifierNetError::InvalidPeriod
        ));
        let deadline_passed = MAX_WINDOW_PERIOD_SECS + REPORT_GRACE_SECS + 1;
        assert!(matches!(
            net.open_window(
                &node.public.awe_id,
                0,
                MAX_WINDOW_PERIOD_SECS,
                deadline_passed
            )
            .unwrap_err(),
            VerifierNetError::WindowExpired
        ));
        let window = net
            .open_window(&node.public.awe_id, 0, 7_200, 7_200)
            .unwrap();
        assert!(!window.closed);
        assert_eq!(window.report_deadline_unix, 7_200 + REPORT_GRACE_SECS);
        assert!(matches!(
            net.open_window(&node.public.awe_id, 0, 7_200, 7_200)
                .unwrap_err(),
            VerifierNetError::WindowAlreadyOpen
        ));
        // a different period for the same node is a different window
        assert!(net
            .open_window(&node.public.awe_id, 7_200, 14_400, 7_200)
            .is_ok());
    }

    #[test]
    fn unenrolled_and_self_reports_are_rejected() {
        let founder = identity("reject-founder");
        let node = identity("reject-node");
        let stranger = identity("reject-stranger");
        let mut net = VerifierNet::new(&founder, 0);
        net.enroll_verifier(&founder, &node, 1).unwrap();
        net.open_window(&node.public.awe_id, 0, 3_600, 3_600)
            .unwrap();
        // a well-signed report from a node that is not enrolled
        let stranger_report = SignedUsageReport::new(
            &stranger,
            node.public.awe_id.clone(),
            0,
            3_600,
            period_usage(),
        );
        assert!(matches!(
            net.submit_report(stranger_report, 3_600).unwrap_err(),
            VerifierNetError::UnknownVerifier
        ));
        // the contributing node is enrolled as a verifier but cannot
        // verify its own contribution
        let self_report =
            SignedUsageReport::new(&node, node.public.awe_id.clone(), 0, 3_600, period_usage());
        assert!(matches!(
            net.submit_report(self_report, 3_600).unwrap_err(),
            VerifierNetError::SelfAttestation
        ));
        // and a report for an unknown window is rejected even from an
        // enrolled verifier
        let verifier = identity("reject-verifier");
        net.enroll_verifier(&founder, &verifier, 2).unwrap();
        let no_window = SignedUsageReport::new(
            &verifier,
            node.public.awe_id.clone(),
            3_600,
            7_200,
            period_usage(),
        );
        assert!(matches!(
            net.submit_report(no_window, 3_600).unwrap_err(),
            VerifierNetError::UnknownWindow
        ));
    }

    #[test]
    fn tampered_reports_are_rejected() {
        let founder = identity("tamper-founder");
        let node = identity("tamper-node");
        let verifier = identity("tamper-verifier");
        let mut net = VerifierNet::new(&founder, 0);
        net.enroll_verifier(&founder, &verifier, 1).unwrap();
        net.open_window(&node.public.awe_id, 0, 3_600, 3_600)
            .unwrap();
        let mut tampered = SignedUsageReport::new(
            &verifier,
            node.public.awe_id.clone(),
            0,
            3_600,
            period_usage(),
        );
        tampered.usage.cpu_core_ticks = 1_000_000;
        assert!(matches!(
            net.submit_report(tampered, 3_600).unwrap_err(),
            VerifierNetError::InvalidSignature
        ));
        assert!(net.reports_for(&node.public.awe_id, 0).is_empty());
    }

    #[test]
    fn duplicate_verifier_reports_are_rejected() {
        let founder = identity("dup-founder");
        let node = identity("dup-node");
        let verifier = identity("dup-verifier");
        let mut net = VerifierNet::new(&founder, 0);
        net.enroll_verifier(&founder, &verifier, 1).unwrap();
        net.open_window(&node.public.awe_id, 0, 7_200, 7_200)
            .unwrap();
        let report = SignedUsageReport::new(
            &verifier,
            node.public.awe_id.clone(),
            0,
            7_200,
            period_usage(),
        );
        net.submit_report(report.clone(), 7_200).unwrap();
        assert!(matches!(
            net.submit_report(report, 7_200).unwrap_err(),
            VerifierNetError::DuplicateVerifier
        ));
        assert!(!net.has_consensus(&node.public.awe_id, 0, 7_200));
        assert!(net.consensus_usage(&node.public.awe_id, 0, 7_200).is_none());
    }

    #[test]
    fn late_reports_are_rejected_and_windows_close() {
        let founder = identity("late-founder");
        let node = identity("late-node");
        let verifier_a = identity("late-verifier-a");
        let verifier_b = identity("late-verifier-b");
        let mut net = VerifierNet::new(&founder, 0);
        net.enroll_verifier(&founder, &verifier_a, 1).unwrap();
        net.enroll_verifier(&founder, &verifier_b, 2).unwrap();
        net.open_window(&node.public.awe_id, 0, 3_600, 3_600)
            .unwrap();
        net.submit_report(
            SignedUsageReport::new(
                &verifier_a,
                node.public.awe_id.clone(),
                0,
                3_600,
                period_usage(),
            ),
            3_600,
        )
        .unwrap();
        let late = 3_600 + REPORT_GRACE_SECS + 1;
        assert!(matches!(
            net.submit_report(
                SignedUsageReport::new(
                    &verifier_b,
                    node.public.awe_id.clone(),
                    0,
                    3_600,
                    period_usage(),
                ),
                late,
            )
            .unwrap_err(),
            VerifierNetError::WindowClosed
        ));
        // closing keeps the collected reports until the retention
        // horizon, so an audit can still verify what was settled
        assert_eq!(net.close_expired(late), 0);
        assert!(net.window(&node.public.awe_id, 0).unwrap().closed);
        assert_eq!(net.reports_for(&node.public.awe_id, 0).len(), 1);
        // past the retention horizon the window and its reports are
        // pruned
        let forgotten = late + CLOSED_WINDOW_RETENTION_SECS + 1;
        assert_eq!(net.close_expired(forgotten), 1);
        assert!(net.window(&node.public.awe_id, 0).is_none());
        assert!(net.reports_for(&node.public.awe_id, 0).is_empty());
        assert!(net.consensus_usage(&node.public.awe_id, 0, 3_600).is_none());
    }

    #[test]
    fn two_verifiers_reach_consensus_and_settle() {
        let founder = identity("settle-founder");
        let node = identity("settle-node");
        let verifier_a = identity("settle-verifier-a");
        let verifier_b = identity("settle-verifier-b");
        let mut net = VerifierNet::new(&founder, 0);
        net.enroll_verifier(&founder, &verifier_a, 1).unwrap();
        net.enroll_verifier(&founder, &verifier_b, 2).unwrap();
        let mut ledger = OnecoinLedger::default();
        ledger
            .initialize_genesis_with_verifiers(
                &[
                    founder.public.awe_id.clone(),
                    node.public.awe_id.clone(),
                    verifier_a.public.awe_id.clone(),
                    verifier_b.public.awe_id.clone(),
                ],
                &[founder.public.public_key],
            )
            .unwrap();
        // one verifier alone is not a settlement consensus
        net.open_window(&node.public.awe_id, 0, 7_200, 7_200)
            .unwrap();
        net.submit_report(
            SignedUsageReport::new(
                &verifier_a,
                node.public.awe_id.clone(),
                0,
                7_200,
                period_usage(),
            ),
            7_200,
        )
        .unwrap();
        assert!(!net.has_consensus(&node.public.awe_id, 0, 7_200));
        let single = net.evidence(
            period_usage(),
            &node.public.awe_id,
            0,
            7_200,
            Some(period_usage()),
        );
        assert!(single.is_none());
        // the second distinct verifier completes the consensus
        net.submit_report(
            SignedUsageReport::new(
                &verifier_b,
                node.public.awe_id.clone(),
                0,
                7_200,
                period_usage(),
            ),
            7_200,
        )
        .unwrap();
        assert!(net.has_consensus(&node.public.awe_id, 0, 7_200));
        let evidence = net
            .evidence(
                period_usage(),
                &node.public.awe_id,
                0,
                7_200,
                Some(period_usage()),
            )
            .unwrap();
        let mut economy = ResourceEconomy::default();
        economy.grant(&node, limits(), 1).unwrap();
        let settlement = economy
            .settle(
                &mut ledger,
                &founder,
                &node.public.awe_id,
                &evidence,
                0,
                7_200,
            )
            .unwrap();
        // cpu 100 * 1000 + bandwidth 5 * 5000 + uptime 2 * 100 = 125_200
        // usage atoms, plus 2 * 50 availability atoms
        assert_eq!(settlement.usage_atoms, 125_200);
        assert_eq!(settlement.availability_atoms, 100);
        assert_eq!(settlement.total_atoms, 125_300);
        assert_eq!(
            ledger.balance_atoms(&node.public.awe_id),
            10 * ATOMS_PER_COIN + 125_300
        );
        // the same period cannot be settled twice
        assert!(matches!(
            economy
                .settle(
                    &mut ledger,
                    &founder,
                    &node.public.awe_id,
                    &evidence,
                    0,
                    7_200
                )
                .unwrap_err(),
            ResourceEconomyError::AlreadySettled
        ));
    }

    #[test]
    fn peer_observations_attest_only_observable_resources() {
        let founder = identity("observe-founder");
        let node = identity("observe-node");
        let verifier_a = identity("observe-verifier-a");
        let verifier_b = identity("observe-verifier-b");
        let mut net = VerifierNet::new(&founder, 0);
        net.enroll_verifier(&founder, &verifier_a, 1).unwrap();
        net.enroll_verifier(&founder, &verifier_b, 2).unwrap();
        net.open_window(&node.public.awe_id, 0, 7_200, 7_200)
            .unwrap();
        let observation = PeerObservation {
            bytes_exchanged: 3_000_000_000,
            observed_available_secs: 7_200,
        };
        net.submit_report(
            observation.into_report(&verifier_a, node.public.awe_id.clone(), 0, 7_200),
            7_200,
        )
        .unwrap();
        net.submit_report(
            observation.into_report(&verifier_b, node.public.awe_id.clone(), 0, 7_200),
            7_200,
        )
        .unwrap();
        let consensus = net.consensus_usage(&node.public.awe_id, 0, 7_200).unwrap();
        // a remote verifier only saw bytes and availability: the CPU
        // claim of the node's own report earns nothing through this
        // window, because the settlement consensus is the per-resource
        // minimum
        assert_eq!(consensus.bandwidth_gb, 3);
        assert_eq!(consensus.uptime_hours, 2);
        assert_eq!(consensus.cpu_core_ticks, 0);
        assert_eq!(consensus.ram_mb_hours, 0);
        assert_eq!(consensus.storage_gb_hours, 0);
        assert_eq!(consensus.gpu_unit_ticks, 0);
    }

    #[test]
    fn windows_bound_their_reports() {
        let founder = identity("bound-founder");
        let node = identity("bound-node");
        let mut net = VerifierNet::new(&founder, 0);
        net.open_window(&node.public.awe_id, 0, 3_600, 3_600)
            .unwrap();
        for index in 0..MAX_VERIFIER_REPORTS {
            let verifier = identity(&format!("bound-verifier-{index}"));
            net.enroll_verifier(&founder, &verifier, 1).unwrap();
            net.submit_report(
                SignedUsageReport::new(
                    &verifier,
                    node.public.awe_id.clone(),
                    0,
                    3_600,
                    period_usage(),
                ),
                3_600,
            )
            .unwrap();
        }
        let extra = identity("bound-verifier-extra");
        net.enroll_verifier(&founder, &extra, 2).unwrap();
        assert!(matches!(
            net.submit_report(
                SignedUsageReport::new(
                    &extra,
                    node.public.awe_id.clone(),
                    0,
                    3_600,
                    period_usage(),
                ),
                3_600,
            )
            .unwrap_err(),
            VerifierNetError::WindowFull
        ));
        assert_eq!(
            net.reports_for(&node.public.awe_id, 0).len(),
            MAX_VERIFIER_REPORTS
        );
    }

    #[test]
    fn registry_is_bounded() {
        let founder = identity("registry-founder");
        let mut net = VerifierNet::new(&founder, 0);
        for index in 0..MAX_VERIFIERS {
            let verifier = identity(&format!("registry-verifier-{index}"));
            net.enroll_verifier(&founder, &verifier, 1).unwrap();
        }
        let extra = identity("registry-verifier-extra");
        assert!(matches!(
            net.enroll_verifier(&founder, &extra, 2).unwrap_err(),
            VerifierNetError::RegistryFull
        ));
        assert_eq!(net.registry().len(), MAX_VERIFIERS);
    }

    #[test]
    fn windows_are_bounded() {
        let founder = identity("bounded-window-founder");
        let node = identity("bounded-window-node");
        let mut net = VerifierNet::new(&founder, 0);
        for index in 0..MAX_OPEN_WINDOWS {
            let start = index as u64 * 3_600;
            net.open_window(&node.public.awe_id, start, start + 3_600, 0)
                .unwrap();
        }
        let start = MAX_OPEN_WINDOWS as u64 * 3_600;
        assert!(matches!(
            net.open_window(&node.public.awe_id, start, start + 3_600, 0)
                .unwrap_err(),
            VerifierNetError::TooManyWindows
        ));
    }

    #[test]
    fn verifier_net_round_trips_through_serde() {
        let founder = identity("serde-founder");
        let node = identity("serde-node");
        let verifier = identity("serde-verifier");
        let mut net = VerifierNet::new(&founder, 0);
        let certificate = net.enroll_verifier(&founder, &verifier, 1).unwrap();
        net.open_window(&node.public.awe_id, 0, 3_600, 3_600)
            .unwrap();
        net.submit_report(
            SignedUsageReport::new(
                &verifier,
                node.public.awe_id.clone(),
                0,
                3_600,
                period_usage(),
            ),
            3_600,
        )
        .unwrap();
        let bytes = serde_json::to_vec(&net).expect("verifier net serialization");
        let restored: VerifierNet =
            serde_json::from_slice(&bytes).expect("verifier net deserialization");
        assert_eq!(restored, net);
        assert!(restored.validate().is_ok());
        // the restored registry still validates its founder-signed
        // certificates and the restored reports still verify
        let restored_certificate = restored
            .registry()
            .certificate(&verifier.public.public_key)
            .unwrap();
        assert_eq!(restored_certificate, &certificate);
        assert!(restored_certificate.is_valid(&founder.public.public_key));
        assert_eq!(restored.reports_for(&node.public.awe_id, 0).len(), 1);
        assert!(restored.reports_for(&node.public.awe_id, 0)[0].verify());
    }

    fn verifier_net_temp_path(label: &str) -> std::path::PathBuf {
        let mut nonce = [0u8; 16];
        let mut rng = OsRng;
        rng.try_fill_bytes(&mut nonce).unwrap();
        let path = std::env::temp_dir().join(format!(
            "awenet-verifier-net-{label}-{}-{}.json",
            std::process::id(),
            hex::encode(nonce)
        ));
        nonce.zeroize();
        path
    }

    #[test]
    fn invalid_deserialized_certificates_cannot_enroll_or_verify() {
        let founder = identity("invalid-registry-founder");
        let verifier = identity("invalid-registry-verifier");
        let imposter = identity("invalid-registry-imposter");
        let node = identity("invalid-registry-node");
        let mut net = VerifierNet::new(&founder, 0);
        net.open_window(&node.public.awe_id, 0, 3_600, 3_600)
            .unwrap();
        let mut forged = VerifierCertificate {
            verifier: verifier.public.awe_id.clone(),
            public_key: verifier.public.public_key,
            enrolled_at_unix: 1,
            retired_at_unix: None,
            sequence: 1,
            founder_signature: [0u8; 64],
        };
        forged.founder_signature = imposter.sign(&forged.signing_bytes());
        net.registry.verifiers.push(forged);
        assert!(!net.registry.is_active(&verifier.public.public_key));
        assert!(matches!(
            net.validate(),
            Err(VerifierNetError::InvalidRegistry)
        ));
        let report = SignedUsageReport::new(
            &verifier,
            node.public.awe_id.clone(),
            0,
            3_600,
            period_usage(),
        );
        assert!(matches!(
            net.submit_report(report, 3_600),
            Err(VerifierNetError::InvalidRegistry)
        ));
        assert!(!net.has_consensus(&node.public.awe_id, 0, 3_600));
    }

    #[test]
    fn registry_sequence_exhaustion_never_reuses_a_sequence_or_mutates_state() {
        let founder = identity("sequence-founder");
        let verifier = identity("sequence-verifier");
        let mut net = VerifierNet::new(&founder, 0);
        net.registry.next_sequence = u64::MAX;
        let before = net.clone();
        assert!(matches!(
            net.enroll_verifier(&founder, &verifier, 1),
            Err(VerifierNetError::SequenceExhausted)
        ));
        assert_eq!(net, before);

        let mut net = VerifierNet::new(&founder, 0);
        net.enroll_verifier(&founder, &verifier, 1).unwrap();
        net.registry.next_sequence = u64::MAX;
        let before = net.clone();
        assert!(matches!(
            net.retire_verifier(&founder, &verifier.public.public_key, 2),
            Err(VerifierNetError::SequenceExhausted)
        ));
        assert_eq!(net, before);
    }

    #[test]
    fn verifier_reenrollment_cannot_move_time_backwards() {
        let founder = identity("reenroll-founder");
        let verifier = identity("reenroll-verifier");
        let mut net = VerifierNet::new(&founder, 0);
        net.enroll_verifier(&founder, &verifier, 10).unwrap();
        net.retire_verifier(&founder, &verifier.public.public_key, 20)
            .unwrap();
        let before = net.clone();
        assert!(matches!(
            net.enroll_verifier(&founder, &verifier, 19),
            Err(VerifierNetError::InvalidState)
        ));
        assert_eq!(net, before);
        assert!(net
            .enroll_verifier(&founder, &verifier, 20)
            .unwrap()
            .is_active());
    }

    #[test]
    fn window_deadline_overflow_is_rejected_without_mutation() {
        let founder = identity("window-overflow-founder");
        let node = identity("window-overflow-node");
        let mut net = VerifierNet::new(&founder, 0);
        let before = net.clone();
        assert!(matches!(
            net.open_window(
                &node.public.awe_id,
                u64::MAX - 10,
                u64::MAX - 1,
                u64::MAX - 1
            ),
            Err(VerifierNetError::InvalidPeriod)
        ));
        assert_eq!(net, before);
    }

    #[test]
    fn signed_verifier_net_store_roundtrips_and_rejects_wrong_founder() {
        let founder = identity("store-founder");
        let node = identity("store-node");
        let verifier_a = identity("store-verifier-a");
        let verifier_b = identity("store-verifier-b");
        let mut net = VerifierNet::new(&founder, 0);
        net.enroll_verifier(&founder, &verifier_a, 1).unwrap();
        net.enroll_verifier(&founder, &verifier_b, 2).unwrap();
        net.open_window(&node.public.awe_id, 0, 3_600, 3_600)
            .unwrap();
        net.submit_report(
            SignedUsageReport::new(
                &verifier_a,
                node.public.awe_id.clone(),
                0,
                3_600,
                period_usage(),
            ),
            3_600,
        )
        .unwrap();

        let path = verifier_net_temp_path("roundtrip");
        save(&path, &net, &founder).unwrap();
        let restored = load(&path, &founder).unwrap();
        assert_eq!(restored, net);
        let wrong_founder = identity("store-wrong-founder");
        assert!(matches!(
            load(&path, &wrong_founder),
            Err(VerifierNetError::WrongFounder)
        ));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn signed_store_rejects_founder_signed_but_internally_forged_registry() {
        let founder = identity("store-forged-founder");
        let verifier = identity("store-forged-verifier");
        let imposter = identity("store-forged-imposter");
        let mut net = VerifierNet::new(&founder, 0);
        let mut certificate = VerifierCertificate {
            verifier: verifier.public.awe_id.clone(),
            public_key: verifier.public.public_key,
            enrolled_at_unix: 1,
            retired_at_unix: None,
            sequence: 1,
            founder_signature: [0u8; 64],
        };
        certificate.founder_signature = imposter.sign(&certificate.signing_bytes());
        net.registry.verifiers.push(certificate);

        let path = verifier_net_temp_path("forged");
        let mut signed = SignedVerifierNetFile {
            version: VERIFIER_NET_STORE_VERSION,
            net: net.clone(),
            signature: [0u8; 64],
        };
        signed.signature = founder.sign(&store_signing_bytes(&signed.net).unwrap());
        std::fs::write(&path, serde_json::to_vec(&signed).unwrap()).unwrap();
        assert!(matches!(
            load(&path, &founder),
            Err(VerifierNetError::InvalidRegistry)
        ));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn failed_verifier_net_save_preserves_previous_snapshot() {
        let founder = identity("store-preserve-founder");
        let verifier = identity("store-preserve-verifier");
        let mut net = VerifierNet::new(&founder, 0);
        net.enroll_verifier(&founder, &verifier, 1).unwrap();
        let path = verifier_net_temp_path("preserve");
        save(&path, &net, &founder).unwrap();
        let before = std::fs::read(&path).unwrap();

        let mut invalid = net.clone();
        invalid.registry.verifiers[0].founder_signature[0] ^= 1;
        assert!(matches!(
            save(&path, &invalid, &founder),
            Err(VerifierNetError::InvalidRegistry)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_file(path).unwrap();
    }
}
