//! Product-level health scoring for AWEp2P.
//!
//! `diagnostics` tracks a single node. This module aggregates the whole
//! product view -- peer coverage, replica durability, transfer success and
//! DHT responsiveness -- into one 0-100 score, a stable health band, and a
//! concrete recommendation. It also keeps a rolling history so operators can
//! see whether the network is trending up or down instead of guessing from a
//! single snapshot.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// Weight of each factor in the final score. Must sum to 100.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthWeights {
    pub replica_durability: u32,
    pub peer_coverage: u32,
    pub transfer_success: u32,
    pub dht_responsiveness: u32,
}

impl HealthWeights {
    pub fn is_valid(&self) -> bool {
        self.replica_durability
            .saturating_add(self.peer_coverage)
            .saturating_add(self.transfer_success)
            .saturating_add(self.dht_responsiveness)
            == 100
    }
}

impl Default for HealthWeights {
    fn default() -> Self {
        Self {
            replica_durability: 40,
            peer_coverage: 25,
            transfer_success: 20,
            dht_responsiveness: 15,
        }
    }
}

/// Coarse, human-readable band derived from the numeric score.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum HealthBand {
    /// Score 0-39: act now, the product is failing its core guarantees.
    Critical,
    /// Score 40-69: degraded, monitor closely.
    AtRisk,
    /// Score 70-89: healthy with room to improve.
    Stable,
    /// Score 90-100: fully healthy.
    Excellent,
}

impl HealthBand {
    pub fn for_score(score: u8) -> Self {
        match score {
            0..=39 => Self::Critical,
            40..=69 => Self::AtRisk,
            70..=89 => Self::Stable,
            _ => Self::Excellent,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::AtRisk => "at-risk",
            Self::Stable => "stable",
            Self::Excellent => "excellent",
        }
    }
}

/// Raw inputs from the product layer. All rates are percentages 0-100.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthInputs {
    /// Healthy replicas as a percentage of the replication target.
    pub replica_health_pct: u8,
    /// Healthy peers as a percentage of all known peers.
    pub peer_health_pct: u8,
    /// Successful transfers as a percentage of all finished transfers.
    pub transfer_success_pct: u8,
    pub dht_responsiveness_pct: u8,
}

impl HealthInputs {
    pub fn is_valid(&self) -> bool {
        self.replica_health_pct <= 100
            && self.peer_health_pct <= 100
            && self.transfer_success_pct <= 100
            && self.dht_responsiveness_pct <= 100
    }
}

/// One scored snapshot of the network.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthReport {
    pub score: u8,
    pub band: HealthBand,
    /// Factor name -> factor percentage (0-100) used in the score.
    pub factors: BTreeMap<String, u8>,
    /// Weighted contribution of each factor, in score points.
    pub contributions: BTreeMap<String, u32>,
    pub recommendation: String,
}

/// Rolling health history with a bounded sample window.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HealthHistory {
    max_samples: usize,
    samples: VecDeque<(u64, u8)>,
}

impl HealthHistory {
    pub fn new(max_samples: usize) -> Self {
        Self {
            max_samples: max_samples.max(1),
            samples: VecDeque::new(),
        }
    }

    /// Record a score at a unix timestamp. Newest sample is kept last.
    pub fn record(&mut self, timestamp_unix: u64, score: u8) {
        if let Some(&(last_timestamp, _)) = self.samples.back() {
            if timestamp_unix < last_timestamp {
                // Never let a clock jump backwards corrupt the trend.
                return;
            }
        }
        let max_samples = self.max_samples.max(1);
        while self.samples.len() >= max_samples {
            self.samples.pop_front();
        }
        self.samples.push_back((timestamp_unix, score));
    }

    pub fn latest(&self) -> Option<u8> {
        self.samples.back().map(|&(_, score)| score)
    }

    pub fn average(&self) -> Option<u8> {
        if self.samples.is_empty() {
            return None;
        }
        let sum: u32 = self.samples.iter().map(|&(_, s)| u32::from(s)).sum();
        Some((sum / self.samples.len() as u32) as u8)
    }

    /// Signed change between the oldest and newest samples.
    pub fn delta(&self) -> Option<i16> {
        match (self.samples.front(), self.samples.back()) {
            (Some(&(_, first)), Some(&(_, last))) => Some(i16::from(last) - i16::from(first)),
            _ => None,
        }
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
}

impl Default for HealthHistory {
    fn default() -> Self {
        Self::new(60)
    }
}

/// Compute the weighted product score and its recommendation.
///
/// Returns `None` when the weights are invalid so a misconfiguration can
/// never silently produce a nonsense score.
pub fn compute_report(inputs: HealthInputs, weights: HealthWeights) -> Option<HealthReport> {
    if !weights.is_valid() || !inputs.is_valid() {
        return None;
    }

    let factors: [(&str, u8, u32); 4] = [
        (
            "replica_durability",
            inputs.replica_health_pct,
            weights.replica_durability,
        ),
        (
            "peer_coverage",
            inputs.peer_health_pct,
            weights.peer_coverage,
        ),
        (
            "transfer_success",
            inputs.transfer_success_pct,
            weights.transfer_success,
        ),
        (
            "dht_responsiveness",
            inputs.dht_responsiveness_pct,
            weights.dht_responsiveness,
        ),
    ];

    let mut score: u32 = 0;
    let mut factor_map = BTreeMap::new();
    let mut contributions = BTreeMap::new();
    let mut weakest = factors[0];

    for (name, pct, weight) in factors {
        // Points contributed by this factor, rounded down.
        let points = u32::from(pct) * weight / 100;
        score += points;
        factor_map.insert(name.to_owned(), pct);
        contributions.insert(name.to_owned(), points);
        if pct < weakest.1 {
            weakest = (name, pct, weight);
        }
    }

    let score = score.min(100) as u8;
    let band = HealthBand::for_score(score);

    Some(HealthReport {
        score,
        band,
        factors: factor_map,
        contributions,
        recommendation: recommendation_for(weakest.0, weakest.1, band),
    })
}

fn recommendation_for(weakest_factor: &str, weakest_pct: u8, band: HealthBand) -> String {
    match (band, weakest_factor) {
        (HealthBand::Critical, "replica_durability") => format!(
            "Replica durability is at {weakest_pct}%: pause new uploads and trigger repair until replicas return to target."
        ),
        (HealthBand::Critical, "peer_coverage") => format!(
            "Only {weakest_pct}% of known peers are healthy: check node reachability and restart quarantined nodes."
        ),
        (HealthBand::Critical, "transfer_success") => format!(
            "Transfer success dropped to {weakest_pct}%: investigate transport failures and hostile-traffic limits before continuing."
        ),
        (HealthBand::Critical, _) => format!(
            "DHT responsiveness is {weakest_pct}%: verify discovery and routing paths immediately."
        ),
        (HealthBand::AtRisk, _) => format!(
            "Health is degraded: weakest factor is {weakest_factor} at {weakest_pct}%."
        ),
        _ => format!("Healthy; keep monitoring {weakest_factor} ({weakest_pct}%)."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn perfect() -> HealthInputs {
        HealthInputs {
            replica_health_pct: 100,
            peer_health_pct: 100,
            transfer_success_pct: 100,
            dht_responsiveness_pct: 100,
        }
    }

    #[test]
    fn perfect_inputs_score_one_hundred() {
        let report = compute_report(perfect(), HealthWeights::default()).unwrap();
        assert_eq!(report.score, 100);
        assert_eq!(report.band, HealthBand::Excellent);
    }

    #[test]
    fn zero_inputs_score_zero_and_are_critical() {
        let report = compute_report(HealthInputs::default(), HealthWeights::default()).unwrap();
        assert_eq!(report.score, 0);
        assert_eq!(report.band, HealthBand::Critical);
        assert!(!report.recommendation.is_empty());
    }

    #[test]
    fn default_weights_are_valid_and_weighted() {
        let w = HealthWeights::default();
        assert!(w.is_valid());
        let report = compute_report(
            HealthInputs {
                replica_health_pct: 50,
                peer_health_pct: 100,
                transfer_success_pct: 100,
                dht_responsiveness_pct: 100,
            },
            w,
        )
        .unwrap();
        // 50% of the 40-point replica factor = 20 points lost.
        assert_eq!(report.score, 80);
        assert_eq!(report.band, HealthBand::Stable);
    }

    #[test]
    fn invalid_weights_are_rejected() {
        let w = HealthWeights {
            replica_durability: 50,
            peer_coverage: 25,
            transfer_success: 20,
            dht_responsiveness: 15,
        };
        assert!(!w.is_valid());
        assert!(compute_report(perfect(), w).is_none());
    }

    #[test]
    fn out_of_range_inputs_are_rejected() {
        let inputs = HealthInputs {
            replica_health_pct: 101,
            ..perfect()
        };
        assert!(!inputs.is_valid());
        assert!(compute_report(inputs, HealthWeights::default()).is_none());
    }

    #[test]
    fn bands_cover_the_full_range() {
        assert_eq!(HealthBand::for_score(0), HealthBand::Critical);
        assert_eq!(HealthBand::for_score(39), HealthBand::Critical);
        assert_eq!(HealthBand::for_score(40), HealthBand::AtRisk);
        assert_eq!(HealthBand::for_score(69), HealthBand::AtRisk);
        assert_eq!(HealthBand::for_score(70), HealthBand::Stable);
        assert_eq!(HealthBand::for_score(89), HealthBand::Stable);
        assert_eq!(HealthBand::for_score(90), HealthBand::Excellent);
        assert_eq!(HealthBand::for_score(100), HealthBand::Excellent);
    }

    #[test]
    fn recommendation_targets_the_weakest_factor() {
        let report = compute_report(
            HealthInputs {
                replica_health_pct: 100,
                peer_health_pct: 10,
                transfer_success_pct: 100,
                dht_responsiveness_pct: 100,
            },
            HealthWeights::default(),
        )
        .unwrap();
        assert_eq!(report.band, HealthBand::Stable);
        assert!(report.recommendation.contains("peer_coverage"));
    }

    #[test]
    fn history_is_bounded_and_monotonic() {
        let mut h = HealthHistory::new(3);
        for i in 0..5u64 {
            h.record(i, (i * 10) as u8);
        }
        assert_eq!(h.len(), 3);
        assert_eq!(h.latest(), Some(40));
        // Oldest kept sample is 20, newest is 40.
        assert_eq!(h.delta(), Some(20));
        assert_eq!(h.average(), Some(30));

        // Out-of-order timestamps are ignored.
        h.record(0, 99);
        assert_eq!(h.latest(), Some(40));
    }

    #[test]
    fn history_compares_timestamps_not_health_scores() {
        let mut history = HealthHistory::new(4);
        history.record(40, 90);
        // A newer timestamp must be accepted even when the score is lower
        // than the previous score.
        history.record(41, 20);
        assert_eq!(history.len(), 2);
        assert_eq!(history.latest(), Some(20));

        // An actually older timestamp must still be rejected.
        history.record(39, 100);
        assert_eq!(history.len(), 2);
        assert_eq!(history.latest(), Some(20));
    }

    #[test]
    fn empty_history_has_no_trend() {
        let h = HealthHistory::default();
        assert!(h.is_empty());
        assert_eq!(h.latest(), None);
        assert_eq!(h.average(), None);
        assert_eq!(h.delta(), None);
    }

    #[test]
    fn score_never_exceeds_one_hundred() {
        // Even with a hostile weight table, saturating math caps the score.
        let w = HealthWeights {
            replica_durability: 97,
            peer_coverage: 1,
            transfer_success: 1,
            dht_responsiveness: 1,
        };
        let report = compute_report(perfect(), w).unwrap();
        assert!(report.score <= 100);
    }
}
