//! Live peer-to-peer transport for the AWENET usage verification economy.
//!
//! `verifier_net.rs` defines the multi-node verification contract: a
//! founder-gated verifier registry, per-period attestation windows and
//! the acceptance rules for signed usage reports. Until now that state
//! had no network life: nothing told the peers of a contributing node
//! that a window was open, and nothing carried a verifier's signed
//! report from the verifier node to the contributing node.
//!
//! This module is the wire between live nodes:
//!
//! - `WindowAnnounce`: a contributing node announces one open
//!   attestation window to its peers. The announcement is only a
//!   well-formed re-statement of a window the node already opened; a
//!   peer accepts it only when the sender really is the announced node,
//!   the period is well formed and the report deadline has not passed.
//! - `ReportDelivery`: a verifier node delivers one signed
//!   `SignedUsageReport` for a window it observed, built strictly from
//!   the bytes and contact time it actually measured.
//! - `VerifierSide`: the verifier-side observation state. It tracks the
//!   announced windows in bounded memory, accumulates the bytes this
//!   node exchanged with each observed node, and builds exactly one
//!   report per window once the settlement period has ended. A report
//!   is attempted once: a verifier never re-reports the same window, so
//!   the contributing node's duplicate rule is never exercised by
//!   transport retries.
//!
//! Nothing here mints ONECOIN, changes reward rates, enrolls verifiers
//! or weakens the founder's exclusive rights. Every acceptance decision
//! still runs inside `VerifierNet`; this module only moves honest
//! messages between nodes.

use crate::identity::{AweId, Identity};
use crate::resource_meter::SignedUsageReport;
use crate::verifier_net::{
    AttestationWindow, PeerObservation, VerifierNet, MAX_WINDOW_PERIOD_SECS, REPORT_GRACE_SECS,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

/// Stream 202 on the authenticated node transport: storage uses 200,
/// ONECOIN transfers use 201 and verifier reports travel here.
pub const VERIFIER_TRANSPORT_STREAM: u32 = 202;
pub const VERIFIER_TRANSPORT_VERSION: u16 = 1;
/// The transport carries JSON messages bounded by the node policy size.
pub const MAX_TRANSPORT_MESSAGE_BYTES: usize = 64 * 1024;
/// How many announced windows one verifier node tracks at most.
pub const MAX_OBSERVED_WINDOWS: usize = 1_024;
/// Observed windows past their report deadline plus this horizon are
/// pruned from the verifier-side memory.
pub const OBSERVED_RETENTION_SECS: u64 = 30 * 24 * 3_600;

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum VerifierTransportError {
    #[error("the verifier transport message exceeds the maximum size")]
    MessageTooLarge,
    #[error("verifier transport serialization failed: {0}")]
    Serialization(String),
    #[error("the verifier transport message is malformed")]
    Malformed,
    #[error("the attestation window announcement is invalid")]
    InvalidAnnounce,
    #[error("only the announced node itself may announce its window")]
    ForeignAnnounce,
    #[error("the announced attestation window no longer accepts reports")]
    Expired,
    #[error("the observed window registry is full (max {MAX_OBSERVED_WINDOWS} windows)")]
    TooManyObservations,
}

/// The announcement a contributing node sends for one of its open
/// attestation windows. It carries no new authority: every field is a
/// restatement of a window the node opened in its own verifier net, and
/// receivers keep validating reports against their own state.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WindowAnnounce {
    pub version: u16,
    pub node: AweId,
    pub period_start_unix: u64,
    pub period_end_unix: u64,
    pub report_deadline_unix: u64,
}

impl WindowAnnounce {
    pub fn new(window: &AttestationWindow) -> Self {
        Self {
            version: VERIFIER_TRANSPORT_VERSION,
            node: window.node.clone(),
            period_start_unix: window.period_start_unix,
            period_end_unix: window.period_end_unix,
            report_deadline_unix: window.report_deadline_unix,
        }
    }

    /// An announcement is only well formed when its version matches, its
    /// period is bounded and its deadline is exactly the period end plus
    /// the report grace of the verifier net.
    pub fn is_well_formed(&self) -> bool {
        self.version == VERIFIER_TRANSPORT_VERSION
            && self
                .period_end_unix
                .checked_sub(self.period_start_unix)
                .is_some_and(|duration| {
                    duration > 0 && duration <= MAX_WINDOW_PERIOD_SECS
                })
            && self.report_deadline_unix
                == self.period_end_unix.saturating_add(REPORT_GRACE_SECS)
    }
}

/// One message on the verifier transport stream.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum VerifierTransportMessage {
    /// A contributing node announcing one of its open attestation
    /// windows to its peers.
    Announce(WindowAnnounce),
    /// A verifier node delivering one signed usage report to the
    /// contributing node the report is about.
    Report(SignedUsageReport),
}

/// Encodes one transport message. The encoded form is bounded by
/// `MAX_TRANSPORT_MESSAGE_BYTES` so a hostile peer cannot push an
/// unbounded payload through the transport.
pub fn encode(message: &VerifierTransportMessage) -> Result<Vec<u8>, VerifierTransportError> {
    let bytes = serde_json::to_vec(message)
        .map_err(|error| VerifierTransportError::Serialization(error.to_string()))?;
    if bytes.len() > MAX_TRANSPORT_MESSAGE_BYTES {
        return Err(VerifierTransportError::MessageTooLarge);
    }
    Ok(bytes)
}

/// Decodes one transport message. Malformed or oversized input is
/// rejected without touching any verifier state.
pub fn decode(bytes: &[u8]) -> Result<VerifierTransportMessage, VerifierTransportError> {
    if bytes.len() > MAX_TRANSPORT_MESSAGE_BYTES {
        return Err(VerifierTransportError::MessageTooLarge);
    }
    serde_json::from_slice(bytes).map_err(|_| VerifierTransportError::Malformed)
}

/// The open attestation windows of a verifier net, ready to be announced
/// to peers. Windows that are closed or past their report deadline are
/// not announced: peers could not deliver a report for them anymore.
pub fn open_window_announces(net: &VerifierNet, now_unix: u64) -> Vec<WindowAnnounce> {
    net.windows()
        .filter(|window| !window.closed && now_unix <= window.report_deadline_unix)
        .map(WindowAnnounce::new)
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ObservedWindow {
    node: AweId,
    period_start_unix: u64,
    period_end_unix: u64,
    report_deadline_unix: u64,
    first_seen_unix: u64,
    last_seen_unix: u64,
    bytes_exchanged: u64,
    reported: bool,
}

/// The verifier-side observation state of one node: the attestation
/// windows announced by its peers, together with the bytes this node
/// genuinely exchanged with each observed node and the contact time it
/// genuinely observed. The state lives in memory only: announced
/// windows re-announce while they are open, so a restart simply starts
/// observing again. A report is built at most once per window, only
/// after the settlement period ended and only from the observation this
/// node actually made.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VerifierSide {
    observed: BTreeMap<([u8; 32], u64), ObservedWindow>,
}

impl VerifierSide {
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of windows this node currently observes.
    pub fn len(&self) -> usize {
        self.observed.len()
    }

    pub fn is_empty(&self) -> bool {
        self.observed.is_empty()
    }

    /// Records one window announcement. The sender must be the
    /// announced contributing node itself, the announcement must be
    /// well formed, the window must still accept reports, and a window
    /// already under observation is only refreshed when the period
    /// fields match exactly: an announced period is immutable.
    pub fn observe_announce(
        &mut self,
        sender: &[u8; 32],
        announce: &WindowAnnounce,
        now_unix: u64,
    ) -> Result<(), VerifierTransportError> {
        if announce.node.as_bytes() != sender {
            return Err(VerifierTransportError::ForeignAnnounce);
        }
        if !announce.is_well_formed() {
            return Err(VerifierTransportError::InvalidAnnounce);
        }
        if now_unix > announce.report_deadline_unix {
            return Err(VerifierTransportError::Expired);
        }
        let key = (*sender, announce.period_start_unix);
        match self.observed.entry(key) {
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                let window = entry.get_mut();
                if window.period_end_unix != announce.period_end_unix
                    || window.report_deadline_unix != announce.report_deadline_unix
                {
                    return Err(VerifierTransportError::InvalidAnnounce);
                }
                window.last_seen_unix = window.last_seen_unix.max(now_unix);
                Ok(())
            }
            std::collections::btree_map::Entry::Vacant(entry) => {
                if self.observed.len() >= MAX_OBSERVED_WINDOWS {
                    let _ = self.prune(now_unix);
                }
                if self.observed.len() >= MAX_OBSERVED_WINDOWS {
                    return Err(VerifierTransportError::TooManyObservations);
                }
                entry.insert(ObservedWindow {
                    node: announce.node.clone(),
                    period_start_unix: announce.period_start_unix,
                    period_end_unix: announce.period_end_unix,
                    report_deadline_unix: announce.report_deadline_unix,
                    first_seen_unix: now_unix,
                    last_seen_unix: now_unix,
                    bytes_exchanged: 0,
                    reported: false,
                });
                Ok(())
            }
        }
    }

    /// Accumulates bytes exchanged with a peer against every observed
    /// window of that peer, moving the observed contact time forward.
    /// Both directions count; counters only ever grow.
    pub fn record_traffic(&mut self, sender: &[u8; 32], bytes: u64, now_unix: u64) {
        for window in self.observed.values_mut() {
            if window.node.as_bytes() != sender {
                continue;
            }
            window.bytes_exchanged = window.bytes_exchanged.saturating_add(bytes);
            window.last_seen_unix = window.last_seen_unix.max(now_unix);
        }
    }

    /// Builds the signed reports that are due now: one per observed
    /// window whose settlement period has ended, whose report deadline
    /// has not passed and which this node has not reported yet. Each
    /// report is built exactly once and carries only the bytes and
    /// contact time actually observed, capped to the settlement period.
    pub fn due_reports(
        &mut self,
        verifier: &Identity,
        now_unix: u64,
    ) -> Vec<(AweId, SignedUsageReport)> {
        let mut due = Vec::new();
        for window in self.observed.values_mut() {
            if window.reported
                || now_unix < window.period_end_unix
                || now_unix > window.report_deadline_unix
            {
                continue;
            }
            window.reported = true;
            let observation = PeerObservation {
                bytes_exchanged: window.bytes_exchanged,
                observed_available_secs: window
                    .last_seen_unix
                    .saturating_sub(window.first_seen_unix)
                    .min(
                        window
                            .period_end_unix
                            .saturating_sub(window.first_seen_unix),
                    ),
            };
            due.push((
                window.node.clone(),
                observation.into_report(
                    verifier,
                    window.node.clone(),
                    window.period_start_unix,
                    window.period_end_unix,
                ),
            ));
        }
        due
    }

    /// Drops observed windows past their report deadline plus the
    /// retention horizon. Returns how many windows were pruned.
    pub fn prune(&mut self, now_unix: u64) -> usize {
        let expired: Vec<([u8; 32], u64)> = self
            .observed
            .iter()
            .filter(|(_, window)| {
                now_unix
                    > window
                        .report_deadline_unix
                        .saturating_add(OBSERVED_RETENTION_SECS)
            })
            .map(|(key, _)| *key)
            .collect();
        let removed = expired.len();
        for key in expired {
            self.observed.remove(&key);
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Username;
    use crate::resource_economy::MeasuredUsage;
    use crate::verifier_net::{
        CLOSED_WINDOW_RETENTION_SECS, REPORT_GRACE_SECS as GRACE,
    };

    fn identity(name: &str) -> Identity {
        Identity::generate(Username::new(name).unwrap())
    }

    fn announce(node: &AweId, start: u64, end: u64) -> WindowAnnounce {
        WindowAnnounce {
            version: VERIFIER_TRANSPORT_VERSION,
            node: node.clone(),
            period_start_unix: start,
            period_end_unix: end,
            report_deadline_unix: end + GRACE,
        }
    }

    #[test]
    fn messages_roundtrip_and_reject_malformed_input() {
        let node = identity("encode-node");
        let message = VerifierTransportMessage::Announce(announce(
            &node.public.awe_id,
            0,
            3_600,
        ));
        let bytes = encode(&message).unwrap();
        assert_eq!(decode(&bytes).unwrap(), message);
        assert!(matches!(
            decode(b"not verifier transport json"),
            Err(VerifierTransportError::Malformed)
        ));
        let oversized = vec![b'x'; MAX_TRANSPORT_MESSAGE_BYTES + 1];
        assert!(matches!(
            decode(&oversized),
            Err(VerifierTransportError::MessageTooLarge)
        ));
        let huge_report = VerifierTransportMessage::Report(SignedUsageReport::new(
            &node,
            node.public.awe_id.clone(),
            0,
            3_600,
            MeasuredUsage {
                storage_gb_hours: u128::MAX,
                ..MeasuredUsage::default()
            },
        ));
        assert!(encode(&huge_report).is_ok());
    }

    #[test]
    fn announcements_must_be_well_formed() {
        let node = identity("wellformed-node");
        let valid = announce(&node.public.awe_id, 0, 3_600);
        assert!(valid.is_well_formed());
        let mut stale_version = valid.clone();
        stale_version.version = VERIFIER_TRANSPORT_VERSION + 1;
        assert!(!stale_version.is_well_formed());
        let mut empty_period = valid.clone();
        empty_period.period_end_unix = 0;
        assert!(!empty_period.is_well_formed());
        let mut too_long = valid;
        too_long.period_end_unix = MAX_WINDOW_PERIOD_SECS + 2;
        too_long.report_deadline_unix = too_long.period_end_unix + GRACE;
        assert!(!too_long.is_well_formed());
        let mut wrong_deadline = announce(&node.public.awe_id, 0, 3_600);
        wrong_deadline.report_deadline_unix += 1;
        assert!(!wrong_deadline.is_well_formed());
    }

    #[test]
    fn peers_observe_only_their_own_announced_windows() {
        let node = identity("observe-node");
        let other = identity("observe-other");
        let mut side = VerifierSide::new();
        assert_eq!(side.len(), 0);
        let window = announce(&node.public.awe_id, 0, 3_600);
        // a node cannot announce a window for someone else
        assert!(matches!(
            side.observe_announce(other.public.awe_id.as_bytes(), &window, 1),
            Err(VerifierTransportError::ForeignAnnounce)
        ));
        // a malformed announcement is rejected
        let mut malformed = window.clone();
        malformed.report_deadline_unix += 1;
        assert!(matches!(
            side.observe_announce(node.public.awe_id.as_bytes(), &malformed, 1),
            Err(VerifierTransportError::InvalidAnnounce)
        ));
        // an expired window no longer accepts reports
        let expired = announce(&node.public.awe_id, 0, 3_600);
        let past = 3_600 + GRACE + 1;
        assert!(matches!(
            side.observe_announce(node.public.awe_id.as_bytes(), &expired, past),
            Err(VerifierTransportError::Expired)
        ));
        // the honest announcement is observed and idempotent
        side.observe_announce(node.public.awe_id.as_bytes(), &window, 1)
            .unwrap();
        side.observe_announce(node.public.awe_id.as_bytes(), &window, 2)
            .unwrap();
        assert_eq!(side.len(), 1);
        // a re-announced window must not change its immutable period
        let mut moved_period = window.clone();
        moved_period.period_end_unix = 7_200;
        moved_period.report_deadline_unix = 7_200 + GRACE;
        assert!(matches!(
            side.observe_announce(node.public.awe_id.as_bytes(), &moved_period, 3),
            Err(VerifierTransportError::InvalidAnnounce)
        ));
    }

    #[test]
    fn traffic_accumulates_only_for_observed_windows() {
        let node = identity("traffic-node");
        let stranger = identity("traffic-stranger");
        let mut side = VerifierSide::new();
        side.observe_announce(
            node.public.awe_id.as_bytes(),
            &announce(&node.public.awe_id, 0, 3_600),
            10,
        )
        .unwrap();
        side.record_traffic(node.public.awe_id.as_bytes(), 500, 20);
        side.record_traffic(node.public.awe_id.as_bytes(), 250, 30);
        // bytes from a peer without an observed window are simply not
        // attributed to anything
        side.record_traffic(stranger.public.awe_id.as_bytes(), 999, 40);
        let reports = side.due_reports(&stranger, 3_600);
        assert_eq!(reports.len(), 1);
        let (reported_node, report) = &reports[0];
        assert_eq!(*reported_node, node.public.awe_id);
        assert!(report.verify());
        assert_eq!(report.node, node.public.awe_id);
        assert_eq!(report.usage.uptime_hours, 0);
        assert_eq!(report.usage.cpu_core_ticks, 0);
        // 750 bytes observed: far below a gigabyte, so the honest
        // observation reports zero bandwidth
        assert_eq!(report.usage.bandwidth_gb, 0);
    }

    #[test]
    fn reports_are_built_once_after_the_period_and_capped_to_it() {
        let node = identity("due-node");
        let verifier = identity("due-verifier");
        let mut side = VerifierSide::new();
        side.observe_announce(
            node.public.awe_id.as_bytes(),
            &announce(&node.public.awe_id, 1_000, 2_600),
            1_100,
        )
        .unwrap();
        side.record_traffic(node.public.awe_id.as_bytes(), 3_000_000_000, 2_000);
        // contact time also counts, but never beyond the period end
        side.record_traffic(node.public.awe_id.as_bytes(), 3_000_000_000, 9_999);
        // before the period ends nothing is due
        assert!(side.due_reports(&verifier, 2_599).is_empty());
        let reports = side.due_reports(&verifier, 2_600);
        assert_eq!(reports.len(), 1);
        let (_, report) = &reports[0];
        assert!(report.verify());
        assert_eq!(report.period_start_unix, 1_000);
        assert_eq!(report.period_end_unix, 2_600);
        // 6 GB exchanged, availability capped at period end minus first
        // contact: 1_500 seconds, still zero whole hours
        assert_eq!(report.usage.bandwidth_gb, 6);
        assert_eq!(report.usage.uptime_hours, 0);
        // a window is reported exactly once
        assert!(side.due_reports(&verifier, 2_700).is_empty());
        // and never again after the deadline either
        assert!(side.due_reports(&verifier, 2_600 + GRACE + 1).is_empty());
    }

    #[test]
    fn unobserved_and_expired_windows_are_pruned() {
        let node = identity("prune-node");
        let mut side = VerifierSide::new();
        let early = announce(&node.public.awe_id, 0, 3_600);
        side.observe_announce(node.public.awe_id.as_bytes(), &early, 1)
            .unwrap();
        let late = announce(&node.public.awe_id, 10 * 3_600, 11 * 3_600);
        side.observe_announce(node.public.awe_id.as_bytes(), &late, 10 * 3_600)
            .unwrap();
        assert_eq!(side.len(), 2);
        // nothing is pruned before the retention horizon
        let horizon = 11 * 3_600 + GRACE + CLOSED_WINDOW_RETENTION_SECS;
        assert_eq!(side.prune(horizon - 1), 0);
        assert_eq!(side.len(), 2);
        assert_eq!(side.prune(horizon + 1), 2);
        assert!(side.is_empty());
    }

    #[test]
    fn observed_windows_are_bounded() {
        let node = identity("bound-observe-node");
        let mut side = VerifierSide::new();
        for index in 0..MAX_OBSERVED_WINDOWS {
            let start = index as u64 * 3_600;
            side.observe_announce(
                node.public.awe_id.as_bytes(),
                &announce(&node.public.awe_id, start, start + 3_600),
                start,
            )
            .unwrap();
        }
        assert_eq!(side.len(), MAX_OBSERVED_WINDOWS);
        let extra = announce(
            &node.public.awe_id,
            MAX_OBSERVED_WINDOWS as u64 * 3_600,
            MAX_OBSERVED_WINDOWS as u64 * 3_600 + 3_600,
        );
        assert!(matches!(
            side.observe_announce(
                node.public.awe_id.as_bytes(),
                &extra,
                MAX_OBSERVED_WINDOWS as u64 * 3_600
            ),
            Err(VerifierTransportError::TooManyObservations)
        ));
    }

    #[test]
    fn a_live_transport_flow_reaches_verifier_consensus() {
        // The full wire flow: the contributing node opens a window and
        // announces it; two peers observe the announcement, exchange
        // traffic with the node and deliver one signed report each; the
        // contributing node's verifier net accepts both and reaches the
        // consensus the resource economy settles from.
        let founder = identity("flow-founder");
        let contributor = identity("flow-contributor");
        let verifier_a = identity("flow-verifier-a");
        let verifier_b = identity("flow-verifier-b");
        let mut net = VerifierNet::new(&founder, 0);
        net.enroll_verifier_public(
            &founder,
            &verifier_a.public.awe_id,
            &verifier_a.public.public_key,
            1,
        )
        .unwrap();
        net.enroll_verifier_public(
            &founder,
            &verifier_b.public.awe_id,
            &verifier_b.public.public_key,
            2,
        )
        .unwrap();
        let period_start = 10_000;
        let period_end = 13_600;
        net.open_window(
            &contributor.public.awe_id,
            period_start,
            period_end,
            period_start,
        )
        .unwrap();
        let announces = open_window_announces(&net, period_start);
        assert_eq!(announces.len(), 1);
        let announce_bytes = encode(&VerifierTransportMessage::Announce(announces[0].clone()))
            .unwrap();

        let mut side_a = VerifierSide::new();
        let mut side_b = VerifierSide::new();
        for side in [&mut side_a, &mut side_b] {
            let decoded = decode(&announce_bytes).unwrap();
            let VerifierTransportMessage::Announce(announce) = decoded else {
                panic!("expected an announcement on the verifier transport");
            };
            side.observe_announce(contributor.public.awe_id.as_bytes(), &announce, period_start + 1)
                .unwrap();
            side.record_traffic(
                contributor.public.awe_id.as_bytes(),
                2_000_000_000,
                period_start + 2,
            );
        }
        // nothing is due while the period is still running
        assert!(side_a.due_reports(&verifier_a, period_end - 1).is_empty());

        for (side, verifier) in [(&mut side_a, &verifier_a), (&mut side_b, &verifier_b)] {
            for (node, report) in side.due_reports(verifier, period_end) {
                assert_eq!(node, contributor.public.awe_id);
                let bytes =
                    encode(&VerifierTransportMessage::Report(report)).unwrap();
                let decoded = decode(&bytes).unwrap();
                let VerifierTransportMessage::Report(report) = decoded else {
                    panic!("expected a report on the verifier transport");
                };
                net.submit_report(report, period_end).unwrap();
            }
        }
        assert!(net.has_consensus(
            &contributor.public.awe_id,
            period_start,
            period_end
        ));
        let consensus = net
            .consensus_usage(&contributor.public.awe_id, period_start, period_end)
            .unwrap();
        assert_eq!(consensus.bandwidth_gb, 2);
        assert_eq!(consensus.cpu_core_ticks, 0);
        // a replayed report from the same verifier is still a duplicate
        let replay = side_a
            .due_reports(&verifier_a, period_end + 1)
            .into_iter()
            .map(|(_, report)| report)
            .collect::<Vec<_>>();
        assert!(replay.is_empty());
    }
}
