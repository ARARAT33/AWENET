//! Lightweight in-process metrics registry.
//! Counters, gauges and bounded latency samples for the product health
//! pipeline. Deterministic and replay-friendly: no wall clock, no threads.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_SAMPLES_PER_METRIC: usize = 64;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    counters: BTreeMap<String, u64>,
    gauges: BTreeMap<String, f64>,
    samples: BTreeMap<String, Vec<u64>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MetricSummary {
    pub name: String,
    pub counter: Option<u64>,
    pub gauge: Option<f64>,
    pub sample_count: usize,
    pub mean: Option<f64>,
    pub p95: Option<u64>,
}

impl Metrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn incr(&mut self, name: &str) {
        self.add(name, 1);
    }

    pub fn add(&mut self, name: &str, delta: u64) {
        *self.counters.entry(name.to_string()).or_insert(0) += delta;
    }

    pub fn counter(&self, name: &str) -> u64 {
        self.counters.get(name).copied().unwrap_or(0)
    }

    pub fn set_gauge(&mut self, name: &str, value: f64) {
        self.gauges.insert(name.to_string(), value);
    }

    pub fn gauge(&self, name: &str) -> Option<f64> {
        self.gauges.get(name).copied()
    }

    /// Records a latency or size sample in microseconds, keeping at most
    /// MAX_SAMPLES_PER_METRIC of the most recent values per metric.
    pub fn record_sample(&mut self, name: &str, micros: u64) {
        let entry = self.samples.entry(name.to_string()).or_default();
        if entry.len() == MAX_SAMPLES_PER_METRIC {
            entry.remove(0);
        }
        entry.push(micros);
    }

    pub fn samples(&self, name: &str) -> &[u64] {
        self.samples.get(name).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn mean(&self, name: &str) -> Option<f64> {
        let s = self.samples(name);
        if s.is_empty() {
            return None;
        }
        let total: u64 = s.iter().sum();
        Some(total as f64 / s.len() as f64)
    }

    /// 95th percentile of the recorded samples, nearest-rank method.
    pub fn p95(&self, name: &str) -> Option<u64> {
        let s = self.samples(name);
        if s.is_empty() {
            return None;
        }
        let mut sorted: Vec<u64> = s.to_vec();
        sorted.sort_unstable();
        let rank = (sorted.len() - sorted.len() / 20).clamp(1, sorted.len());
        Some(sorted[rank - 1])
    }

    pub fn summary(&self) -> Vec<MetricSummary> {
        let mut names: Vec<&String> = self.counters.keys().collect();
        names.extend(self.gauges.keys());
        names.extend(self.samples.keys());
        names.sort();
        names.dedup();
        names
            .into_iter()
            .map(|name| MetricSummary {
                name: name.clone(),
                counter: self.counters.get(name).copied(),
                gauge: self.gauges.get(name).copied(),
                sample_count: self.samples(name).len(),
                mean: self.mean(name),
                p95: self.p95(name),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_and_gauges() {
        let mut m = Metrics::new();
        m.incr("transfers.total");
        m.incr("transfers.total");
        m.add("transfers.total", 3);
        m.set_gauge("peers.online", 7.0);
        assert_eq!(m.counter("transfers.total"), 5);
        assert_eq!(m.gauge("peers.online"), Some(7.0));
        assert_eq!(m.counter("missing"), 0);
        assert_eq!(m.gauge("missing"), None);
    }

    #[test]
    fn samples_stay_bounded() {
        let mut m = Metrics::new();
        for i in 0..(MAX_SAMPLES_PER_METRIC + 20) as u64 {
            m.record_sample("op.latency", i);
        }
        assert_eq!(m.samples("op.latency").len(), MAX_SAMPLES_PER_METRIC);
        assert_eq!(m.p95("op.latency"), Some(80));
        assert!(m.mean("op.latency").is_some());
        assert_eq!(m.p95("missing"), None);
    }

    #[test]
    fn summary_merges_names() {
        let mut m = Metrics::new();
        m.incr("a");
        m.set_gauge("b", 1.0);
        m.record_sample("c", 5);
        let s = m.summary();
        let names: Vec<&str> = s.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
        assert_eq!(s[0].counter, Some(1));
    }
}
