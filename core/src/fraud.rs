//! Fraud and abuse detection for the ONECOIN economy.
//!
//! The detector is advisory by design: it observes the economy's raw
//! signals (resource reports, payouts, submissions, store orders and
//! transfers), flags the patterns the directive calls out — inflated
//! usage reports, double payouts, resubmitted work, self-dealing and
//! wash trading, abnormal transfer velocity — and keeps bounded
//! evidence for the founder's review. It holds no funds, mints no
//! ONECOIN and can neither approve work nor suspend accounts; those
//! powers stay with the founder paths in founder.rs.

use crate::identity::AweId;
use crate::resource_economy::MeasuredUsage;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use thiserror::Error;

pub const MAX_ALERTS: usize = 10_000;
pub const MAX_OBSERVATIONS: usize = 20_000;
pub const MAX_SUBJECTS: usize = 10_000;
pub const MAX_SEEN_KEYS: usize = 100_000;
pub const MAX_DETAIL_CHARS: usize = 256;

pub const DEFAULT_REPORT_TOLERANCE_BPS: u16 = 2_000;
pub const DEFAULT_VELOCITY_WINDOW_SECS: u64 = 3_600;
pub const DEFAULT_MAX_TRANSFERS_PER_WINDOW: usize = 60;
pub const DEFAULT_WASH_WINDOW_SECS: u64 = 3_600;
pub const DEFAULT_WASH_REPEAT_THRESHOLD: usize = 5;

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum FraudError {
    #[error("fraud alert log is full (max {MAX_ALERTS} alerts)")]
    AlertLogFull,
    #[error("fraud alert id space is exhausted")]
    AlertIdExhausted,
    #[error("fraud detector subject capacity is full (max {MAX_SUBJECTS})")]
    SubjectCapacity,
    #[error("fraud alert {0} is unknown")]
    UnknownAlert(u64),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FraudPolicy {
    pub report_tolerance_bps: u16,
    pub velocity_window_secs: u64,
    pub max_transfers_per_window: usize,
    pub wash_window_secs: u64,
    pub wash_repeat_threshold: usize,
}

impl Default for FraudPolicy {
    fn default() -> Self {
        Self {
            report_tolerance_bps: DEFAULT_REPORT_TOLERANCE_BPS,
            velocity_window_secs: DEFAULT_VELOCITY_WINDOW_SECS,
            max_transfers_per_window: DEFAULT_MAX_TRANSFERS_PER_WINDOW,
            wash_window_secs: DEFAULT_WASH_WINDOW_SECS,
            wash_repeat_threshold: DEFAULT_WASH_REPEAT_THRESHOLD,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlertKind {
    InflatedUsageReport,
    DuplicatePayout,
    DuplicateSubmission,
    SelfDealingOrder,
    WashTrading,
    TransferVelocity,
}

impl AlertKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InflatedUsageReport => "inflated_usage_report",
            Self::DuplicatePayout => "duplicate_payout",
            Self::DuplicateSubmission => "duplicate_submission",
            Self::SelfDealingOrder => "self_dealing_order",
            Self::WashTrading => "wash_trading",
            Self::TransferVelocity => "transfer_velocity",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Low,
    Medium,
    High,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Recommendation {
    Review,
    Hold,
    Suspend,
}

impl Recommendation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Review => "review",
            Self::Hold => "hold",
            Self::Suspend => "suspend",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FraudAlert {
    pub id: u64,
    pub kind: AlertKind,
    pub subject: Option<AweId>,
    pub severity: Severity,
    pub recommendation: Recommendation,
    pub detail: String,
    pub evidence_hash: [u8; 32],
    pub observed_at_unix: u64,
    pub acknowledged: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObservationRecord {
    pub at_unix: u64,
    pub subject: Option<AweId>,
    pub description: String,
    pub evidence_hash: [u8; 32],
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FraudDetector {
    policy: FraudPolicy,
    next_alert_id: u64,
    alerts: Vec<FraudAlert>,
    observations: VecDeque<ObservationRecord>,
    seen_payouts: BTreeSet<[u8; 32]>,
    submission_keys: BTreeSet<([u8; 32], [u8; 32])>,
    transfer_times: BTreeMap<String, Vec<u64>>,
    order_history: BTreeMap<(String, String), Vec<(u128, u64)>>,
}

/// A claimed value is inflated when it exceeds the measured value by more
/// than the tolerance; any claim without a measurement behind it counts as
/// inflated as well.
fn usage_claim_exceeds(claimed: u128, measured: u128, tolerance_bps: u128) -> bool {
    if claimed == 0 {
        return false;
    }
    if measured == 0 {
        return true;
    }
    claimed.saturating_mul(10_000) > measured.saturating_mul(10_000 + tolerance_bps)
}

impl FraudDetector {
    pub fn new(policy: FraudPolicy) -> Self {
        Self {
            policy,
            next_alert_id: 1,
            alerts: Vec::new(),
            observations: VecDeque::new(),
            seen_payouts: BTreeSet::new(),
            submission_keys: BTreeSet::new(),
            transfer_times: BTreeMap::new(),
            order_history: BTreeMap::new(),
        }
    }

    pub fn policy(&self) -> &FraudPolicy {
        &self.policy
    }

    pub fn alerts(&self) -> &[FraudAlert] {
        &self.alerts
    }

    pub fn open_alerts(&self) -> Vec<&FraudAlert> {
        self.alerts
            .iter()
            .filter(|alert| !alert.acknowledged)
            .collect()
    }

    pub fn observations(&self) -> &VecDeque<ObservationRecord> {
        &self.observations
    }

    fn record_observation(
        &mut self,
        subject: Option<AweId>,
        at_unix: u64,
        description: &str,
        payload: &[u8],
    ) -> [u8; 32] {
        let evidence_hash = *blake3::hash(payload).as_bytes();
        if self.observations.len() >= MAX_OBSERVATIONS {
            self.observations.pop_front();
        }
        self.observations.push_back(ObservationRecord {
            at_unix,
            subject,
            description: description.to_string(),
            evidence_hash,
        });
        evidence_hash
    }

    fn push_alert(
        &mut self,
        kind: AlertKind,
        subject: Option<AweId>,
        severity: Severity,
        recommendation: Recommendation,
        detail: &str,
        at_unix: u64,
        evidence_hash: [u8; 32],
    ) -> Result<u64, FraudError> {
        if self.alerts.len() >= MAX_ALERTS {
            return Err(FraudError::AlertLogFull);
        }
        let id = self
            .next_alert_id
            .checked_add(1)
            .ok_or(FraudError::AlertIdExhausted)?;
        self.next_alert_id = id;
        let bounded_detail: String = detail.chars().take(MAX_DETAIL_CHARS).collect();
        self.alerts.push(FraudAlert {
            id,
            kind,
            subject,
            severity,
            recommendation,
            detail: bounded_detail,
            evidence_hash,
            observed_at_unix: at_unix,
            acknowledged: false,
        });
        Ok(id)
    }

    /// Compares a node's claimed usage with the network's own measurement
    /// of the same period.
    pub fn observe_resource_report(
        &mut self,
        node: &AweId,
        claimed: &MeasuredUsage,
        measured: &MeasuredUsage,
        at_unix: u64,
    ) -> Result<Option<u64>, FraudError> {
        let tolerance = self.policy.report_tolerance_bps as u128;
        let mut inflated: Vec<&'static str> = Vec::new();
        if usage_claim_exceeds(
            claimed.cpu_core_ticks as u128,
            measured.cpu_core_ticks as u128,
            tolerance,
        ) {
            inflated.push("cpu");
        }
        if usage_claim_exceeds(
            claimed.gpu_unit_ticks as u128,
            measured.gpu_unit_ticks as u128,
            tolerance,
        ) {
            inflated.push("gpu");
        }
        if usage_claim_exceeds(claimed.ram_mb_hours, measured.ram_mb_hours, tolerance) {
            inflated.push("ram");
        }
        if usage_claim_exceeds(
            claimed.storage_gb_hours,
            measured.storage_gb_hours,
            tolerance,
        ) {
            inflated.push("storage");
        }
        if usage_claim_exceeds(
            claimed.bandwidth_gb as u128,
            measured.bandwidth_gb as u128,
            tolerance,
        ) {
            inflated.push("bandwidth");
        }
        if usage_claim_exceeds(
            claimed.uptime_hours as u128,
            measured.uptime_hours as u128,
            tolerance,
        ) {
            inflated.push("uptime");
        }
        let mut payload = Vec::new();
        payload.extend_from_slice(b"AWENET-FRAUD-RESOURCE-REPORT/v1");
        payload.extend_from_slice(node.as_bytes());
        payload.extend_from_slice(&serde_json::to_vec(claimed).unwrap_or_default());
        payload.extend_from_slice(&serde_json::to_vec(measured).unwrap_or_default());
        let evidence_hash =
            self.record_observation(Some(node.clone()), at_unix, "resource report", &payload);
        if inflated.is_empty() {
            return Ok(None);
        }
        let detail = format!(
            "usage claim exceeds the network measurement for: {}",
            inflated.join(", ")
        );
        let id = self.push_alert(
            AlertKind::InflatedUsageReport,
            Some(node.clone()),
            Severity::High,
            Recommendation::Hold,
            &detail,
            at_unix,
            evidence_hash,
        )?;
        Ok(Some(id))
    }

    /// Tracks reward purposes: the same payout id may only ever be
    /// presented once.
    pub fn observe_payout(
        &mut self,
        reward_id: [u8; 32],
        at_unix: u64,
    ) -> Result<Option<u64>, FraudError> {
        let mut payload = Vec::new();
        payload.extend_from_slice(b"AWENET-FRAUD-PAYOUT/v1");
        payload.extend_from_slice(&reward_id);
        let evidence_hash = self.record_observation(None, at_unix, "payout", &payload);
        if self.seen_payouts.contains(&reward_id) {
            let id = self.push_alert(
                AlertKind::DuplicatePayout,
                None,
                Severity::High,
                Recommendation::Suspend,
                "a reward id was presented for payout a second time",
                at_unix,
                evidence_hash,
            )?;
            return Ok(Some(id));
        }
        if self.seen_payouts.len() >= MAX_SEEN_KEYS {
            return Err(FraudError::SubjectCapacity);
        }
        self.seen_payouts.insert(reward_id);
        Ok(None)
    }

    /// Tracks submitted work by (contributor, content hash): the same
    /// contributor cannot submit the same content twice.
    pub fn observe_submission(
        &mut self,
        contributor: &AweId,
        content_hash: [u8; 32],
        at_unix: u64,
    ) -> Result<Option<u64>, FraudError> {
        let key = (*contributor.as_bytes(), content_hash);
        let mut payload = Vec::new();
        payload.extend_from_slice(b"AWENET-FRAUD-SUBMISSION/v1");
        payload.extend_from_slice(&key.0);
        payload.extend_from_slice(&key.1);
        let evidence_hash =
            self.record_observation(Some(contributor.clone()), at_unix, "submission", &payload);
        if self.submission_keys.contains(&key) {
            let id = self.push_alert(
                AlertKind::DuplicateSubmission,
                Some(contributor.clone()),
                Severity::High,
                Recommendation::Hold,
                "the same work content was submitted again by the same contributor",
                at_unix,
                evidence_hash,
            )?;
            return Ok(Some(id));
        }
        if self.submission_keys.len() >= MAX_SEEN_KEYS {
            return Err(FraudError::SubjectCapacity);
        }
        self.submission_keys.insert(key);
        Ok(None)
    }

    /// Tracks store orders for self-dealing and wash trading between the
    /// same buyer and seller.
    pub fn observe_store_order(
        &mut self,
        buyer: &AweId,
        seller: &AweId,
        amount_atoms: u128,
        at_unix: u64,
    ) -> Result<Option<u64>, FraudError> {
        let mut payload = Vec::new();
        payload.extend_from_slice(b"AWENET-FRAUD-STORE-ORDER/v1");
        payload.extend_from_slice(buyer.as_bytes());
        payload.extend_from_slice(seller.as_bytes());
        payload.extend_from_slice(&amount_atoms.to_be_bytes());
        let evidence_hash =
            self.record_observation(Some(buyer.clone()), at_unix, "store order", &payload);
        if buyer == seller {
            let id = self.push_alert(
                AlertKind::SelfDealingOrder,
                Some(buyer.clone()),
                Severity::Medium,
                Recommendation::Review,
                "a store order lists the buyer and the seller as the same account",
                at_unix,
                evidence_hash,
            )?;
            return Ok(Some(id));
        }
        let window = self.policy.wash_window_secs;
        let threshold = self.policy.wash_repeat_threshold;
        let key = (buyer.to_hex(), seller.to_hex());
        let history = match self.order_history.get_mut(&key) {
            Some(history) => history,
            None => {
                if self.order_history.len() >= MAX_SUBJECTS {
                    return Err(FraudError::SubjectCapacity);
                }
                self.order_history.entry(key).or_default()
            }
        };
        history.push((amount_atoms, at_unix));
        let cutoff = at_unix.saturating_sub(window);
        history.retain(|(_, at)| *at >= cutoff);
        let repeats = history
            .iter()
            .filter(|(amount, _)| *amount == amount_atoms)
            .count();
        if repeats == threshold {
            let id = self.push_alert(
                AlertKind::WashTrading,
                Some(buyer.clone()),
                Severity::High,
                Recommendation::Hold,
                "the same buyer-seller pair repeated identical store orders within the wash window",
                at_unix,
                evidence_hash,
            )?;
            return Ok(Some(id));
        }
        Ok(None)
    }

    /// Tracks transfer velocity per sender inside a rolling window.
    pub fn observe_transfer(
        &mut self,
        sender: &AweId,
        at_unix: u64,
    ) -> Result<Option<u64>, FraudError> {
        let mut payload = Vec::new();
        payload.extend_from_slice(b"AWENET-FRAUD-TRANSFER/v1");
        payload.extend_from_slice(sender.as_bytes());
        let evidence_hash =
            self.record_observation(Some(sender.clone()), at_unix, "transfer", &payload);
        let window = self.policy.velocity_window_secs;
        let max_transfers = self.policy.max_transfers_per_window;
        let key = sender.to_hex();
        let times = match self.transfer_times.get_mut(&key) {
            Some(times) => times,
            None => {
                if self.transfer_times.len() >= MAX_SUBJECTS {
                    return Err(FraudError::SubjectCapacity);
                }
                self.transfer_times.entry(key).or_default()
            }
        };
        times.push(at_unix);
        let cutoff = at_unix.saturating_sub(window);
        times.retain(|at| *at >= cutoff);
        if times.len() == max_transfers.saturating_add(1) {
            let id = self.push_alert(
                AlertKind::TransferVelocity,
                Some(sender.clone()),
                Severity::Medium,
                Recommendation::Review,
                "transfer rate exceeded the configured per-window limit",
                at_unix,
                evidence_hash,
            )?;
            return Ok(Some(id));
        }
        Ok(None)
    }

    /// Marks an alert as reviewed. Acknowledging is bookkeeping only: it
    /// changes no wallet, no payout and no account state.
    pub fn acknowledge(&mut self, alert_id: u64) -> Result<(), FraudError> {
        let alert = self
            .alerts
            .iter_mut()
            .find(|alert| alert.id == alert_id)
            .ok_or(FraudError::UnknownAlert(alert_id))?;
        alert.acknowledged = true;
        Ok(())
    }
}

impl Default for FraudDetector {
    fn default() -> Self {
        Self::new(FraudPolicy::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Username;

    fn identity(name: &str) -> AweId {
        crate::identity::Identity::generate(Username::new(name).unwrap())
            .public
            .awe_id
    }

    fn usage(cpu: u64, ram: u128, bandwidth: u64, uptime: u64) -> MeasuredUsage {
        MeasuredUsage {
            cpu_core_ticks: cpu,
            gpu_unit_ticks: 0,
            ram_mb_hours: ram,
            storage_gb_hours: 0,
            bandwidth_gb: bandwidth,
            uptime_hours: uptime,
        }
    }

    #[test]
    fn inflated_usage_reports_are_flagged_within_tolerance() {
        let mut detector = FraudDetector::default();
        let node = identity("fraud-node");
        let measured = usage(100, 0, 10, 1);
        // 25% above the measurement: beyond the default 20% tolerance
        let inflated = usage(125, 0, 10, 1);
        detector
            .observe_resource_report(&node, &inflated, &measured, 10)
            .unwrap()
            .unwrap();
        let alert = detector.alerts().last().unwrap();
        assert_eq!(alert.kind, AlertKind::InflatedUsageReport);
        assert_eq!(alert.severity, Severity::High);
        assert_eq!(alert.recommendation, Recommendation::Hold);
        assert!(alert.detail.contains("cpu"));
        // 10% above stays inside the tolerance
        let borderline = usage(110, 0, 10, 1);
        assert!(detector
            .observe_resource_report(&node, &borderline, &measured, 20)
            .unwrap()
            .is_none());
        // a claim with no measurement behind it is flagged too
        let unverifiable = usage(1, 0, 0, 0);
        assert!(detector
            .observe_resource_report(&node, &unverifiable, &usage(0, 0, 0, 0), 30)
            .unwrap()
            .is_some());
        assert_eq!(detector.observations().len(), 3);
    }

    #[test]
    fn duplicate_payouts_are_detected_once() {
        let mut detector = FraudDetector::default();
        let reward_id = [9u8; 32];
        assert!(detector.observe_payout(reward_id, 1).unwrap().is_none());
        detector.observe_payout(reward_id, 2).unwrap().unwrap();
        let alert = detector.alerts().last().unwrap();
        assert_eq!(alert.kind, AlertKind::DuplicatePayout);
        assert_eq!(alert.recommendation, Recommendation::Suspend);
    }

    #[test]
    fn resubmitted_work_is_flagged_per_contributor() {
        let mut detector = FraudDetector::default();
        let contributor = identity("fraud-contributor");
        let other = identity("fraud-other");
        let content = [3u8; 32];
        assert!(detector
            .observe_submission(&contributor, content, 1)
            .unwrap()
            .is_none());
        detector
            .observe_submission(&contributor, content, 2)
            .unwrap()
            .unwrap();
        assert_eq!(
            detector.alerts().last().unwrap().kind,
            AlertKind::DuplicateSubmission
        );
        // the same content from a different contributor is a new key
        assert!(detector
            .observe_submission(&other, content, 3)
            .unwrap()
            .is_none());
    }

    #[test]
    fn self_dealing_orders_are_flagged() {
        let mut detector = FraudDetector::default();
        let account = identity("fraud-account");
        detector
            .observe_store_order(&account, &account, 1_000, 1)
            .unwrap()
            .unwrap();
        assert_eq!(
            detector.alerts().last().unwrap().kind,
            AlertKind::SelfDealingOrder
        );
    }

    #[test]
    fn repeated_identical_orders_look_like_wash_trading() {
        let mut detector = FraudDetector::default();
        let buyer = identity("wash-buyer");
        let seller = identity("wash-seller");
        for at in 0..(DEFAULT_WASH_REPEAT_THRESHOLD - 1) as u64 {
            assert!(detector
                .observe_store_order(&buyer, &seller, 5_000, at)
                .unwrap()
                .is_none());
        }
        detector
            .observe_store_order(&buyer, &seller, 5_000, 10)
            .unwrap()
            .unwrap();
        assert_eq!(
            detector.alerts().last().unwrap().kind,
            AlertKind::WashTrading
        );
        // a different amount is tracked separately
        assert!(detector
            .observe_store_order(&buyer, &seller, 7_000, 11)
            .unwrap()
            .is_none());
    }

    #[test]
    fn transfer_velocity_alerts_once_per_breach() {
        let mut detector = FraudDetector::default();
        let sender = identity("velocity-sender");
        for at in 0..DEFAULT_MAX_TRANSFERS_PER_WINDOW as u64 {
            assert!(detector.observe_transfer(&sender, at).unwrap().is_none());
        }
        detector
            .observe_transfer(&sender, DEFAULT_MAX_TRANSFERS_PER_WINDOW as u64)
            .unwrap()
            .unwrap();
        assert_eq!(
            detector.alerts().last().unwrap().kind,
            AlertKind::TransferVelocity
        );
        // the next transfer inside the same window does not repeat the alert
        assert!(detector
            .observe_transfer(&sender, DEFAULT_MAX_TRANSFERS_PER_WINDOW as u64 + 1)
            .unwrap()
            .is_none());
        // a fresh window resets the count
        let later = DEFAULT_VELOCITY_WINDOW_SECS + 100;
        assert!(detector.observe_transfer(&sender, later).unwrap().is_none());
    }

    #[test]
    fn acknowledgement_is_bookkeeping_only() {
        let mut detector = FraudDetector::default();
        let account = identity("ack-account");
        detector
            .observe_store_order(&account, &account, 1, 1)
            .unwrap()
            .unwrap();
        let id = detector.alerts().last().unwrap().id;
        assert_eq!(detector.open_alerts().len(), 1);
        detector.acknowledge(id).unwrap();
        assert!(detector.open_alerts().is_empty());
        assert!(detector.acknowledge(id + 1).is_err());
    }

    #[test]
    fn evidence_observations_are_bounded() {
        let policy = FraudPolicy {
            report_tolerance_bps: 0,
            ..FraudPolicy::default()
        };
        let mut detector = FraudDetector::new(policy);
        let node = identity("evidence-node");
        for at in 0..(MAX_OBSERVATIONS as u64) + 10 {
            detector.observe_payout([at as u8; 32], at).unwrap();
        }
        assert_eq!(detector.observations().len(), MAX_OBSERVATIONS);
        let _ = node;
    }
}
