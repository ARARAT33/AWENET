//! Bounded rolling-window metric insights with anomaly detection.
//!
//! Deterministic and replay-friendly: pure data, no wall clock and no
//! threads. Each named series keeps a bounded window of tick-stamped
//! samples; summaries, trends and anomaly verdicts derive from that window
//! only, so identical inputs always produce identical insights.

use std::collections::{BTreeMap, VecDeque};
use thiserror::Error;

pub const MAX_WINDOW: usize = 4_096;
pub const MAX_SERIES: usize = 256;
pub const MIN_SAMPLES_FOR_ANOMALY: usize = 4;

#[derive(Clone, Debug, PartialEq, Error)]
pub enum InsightsError {
    #[error("sample window must be greater than zero")]
    ZeroWindow,
    #[error("sample window exceeds the maximum of {MAX_WINDOW}")]
    WindowTooLarge,
    #[error("series count exceeds the maximum of {MAX_SERIES}")]
    TooManySeries,
    #[error("series not found: {0}")]
    UnknownSeries(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slope {
    Rising,
    Falling,
    Flat,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub tick: u64,
    pub value: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Summary {
    pub count: usize,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub last: f64,
    pub slope: Slope,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Anomaly {
    pub series: String,
    pub tick: u64,
    pub value: f64,
    pub mean: f64,
    pub deviation: f64,
    pub z: f64,
}

#[derive(Debug)]
struct Series {
    samples: VecDeque<Sample>,
    threshold: f64,
}

#[derive(Debug)]
pub struct Insights {
    window: usize,
    default_threshold: f64,
    series: BTreeMap<String, Series>,
}

impl Insights {
    pub fn new(window: usize, default_threshold: f64) -> Result<Self, InsightsError> {
        if window == 0 {
            return Err(InsightsError::ZeroWindow);
        }
        if window > MAX_WINDOW {
            return Err(InsightsError::WindowTooLarge);
        }
        Ok(Self {
            window,
            default_threshold,
            series: BTreeMap::new(),
        })
    }

    pub fn series_count(&self) -> usize {
        self.series.len()
    }

    /// Pushes a tick-stamped sample and returns an anomaly verdict when the
    /// value deviates from the rolling baseline by more than the series
    /// threshold (in z-scores).
    pub fn push(
        &mut self,
        name: &str,
        tick: u64,
        value: f64,
    ) -> Result<Option<Anomaly>, InsightsError> {
        if !self.series.contains_key(name) {
            if self.series.len() >= MAX_SERIES {
                return Err(InsightsError::TooManySeries);
            }
            self.series.insert(
                name.to_string(),
                Series {
                    samples: VecDeque::new(),
                    threshold: self.default_threshold,
                },
            );
        }
        let series = self.series.get_mut(name).expect("series was just inserted");
        let anomaly = Self::anomaly_of(name, tick, value, series);
        series.samples.push_back(Sample { tick, value });
        if series.samples.len() > self.window {
            series.samples.pop_front();
        }
        Ok(anomaly)
    }

    pub fn set_threshold(&mut self, name: &str, threshold: f64) -> Result<(), InsightsError> {
        match self.series.get_mut(name) {
            Some(series) => {
                series.threshold = threshold;
                Ok(())
            }
            None => Err(InsightsError::UnknownSeries(name.to_string())),
        }
    }

    pub fn summary(&self, name: &str) -> Result<Option<Summary>, InsightsError> {
        let series = match self.series.get(name) {
            Some(series) => series,
            None => return Err(InsightsError::UnknownSeries(name.to_string())),
        };
        if series.samples.is_empty() {
            return Ok(None);
        }
        let values: Vec<f64> = series.samples.iter().map(|s| s.value).collect();
        let count = values.len();
        let min = values.iter().fold(f64::INFINITY, |acc, v| acc.min(*v));
        let max = values.iter().fold(f64::NEG_INFINITY, |acc, v| acc.max(*v));
        let mean = values.iter().sum::<f64>() / count as f64;
        let last = values[count - 1];
        let half = count / 2;
        let first_mean = values[..half].iter().sum::<f64>() / half.max(1) as f64;
        let second_len = (count - half).max(1);
        let second_mean = values[half..].iter().sum::<f64>() / second_len as f64;
        let slope = if second_mean - first_mean > 1e-9 {
            Slope::Rising
        } else if first_mean - second_mean > 1e-9 {
            Slope::Falling
        } else {
            Slope::Flat
        };
        Ok(Some(Summary {
            count,
            min,
            max,
            mean,
            last,
            slope,
        }))
    }

    fn anomaly_of(name: &str, tick: u64, value: f64, series: &Series) -> Option<Anomaly> {
        if series.samples.len() < MIN_SAMPLES_FOR_ANOMALY {
            return None;
        }
        let count = series.samples.len() as f64;
        let mean = series.samples.iter().map(|s| s.value).sum::<f64>() / count;
        let variance = series
            .samples
            .iter()
            .map(|s| (s.value - mean) * (s.value - mean))
            .sum::<f64>()
            / count;
        let stddev = variance.sqrt();
        if stddev <= f64::EPSILON {
            return None;
        }
        let z = ((value - mean) / stddev).abs();
        if z > series.threshold {
            Some(Anomaly {
                series: name.to_string(),
                tick,
                value,
                mean,
                deviation: (value - mean).abs(),
                z,
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_window() {
        assert_eq!(
            Insights::new(0, 3.0).unwrap_err(),
            InsightsError::ZeroWindow
        );
        assert_eq!(
            Insights::new(MAX_WINDOW + 1, 3.0).unwrap_err(),
            InsightsError::WindowTooLarge
        );
        assert!(Insights::new(1, 3.0).is_ok());
    }

    #[test]
    fn detects_spike_anomaly() {
        let mut insights = Insights::new(16, 3.0).unwrap();
        for (tick, value) in [8.0, 9.0, 10.0, 11.0, 12.0, 13.0].into_iter().enumerate() {
            assert_eq!(insights.push("latency", tick as u64, value).unwrap(), None);
        }
        let anomaly = insights.push("latency", 6, 100.0).unwrap();
        assert!(anomaly.is_some());
        let anomaly = anomaly.expect("anomaly verdict");
        assert_eq!(anomaly.series, "latency");
        assert_eq!(anomaly.tick, 6);
        assert_eq!(anomaly.value, 100.0);
        assert!(anomaly.z > 3.0);
    }

    #[test]
    fn constant_baseline_never_flags() {
        let mut insights = Insights::new(16, 3.0).unwrap();
        for tick in 0..10u64 {
            assert_eq!(insights.push("cpu", tick, 42.0).unwrap(), None);
        }
        // Zero stddev: identical spikes cannot be distinguished.
        assert_eq!(insights.push("cpu", 10, 42.0).unwrap(), None);
    }

    #[test]
    fn window_is_bounded() {
        let mut insights = Insights::new(3, 3.0).unwrap();
        for tick in 0..10u64 {
            insights.push("net", tick, tick as f64).unwrap();
        }
        let summary = insights.summary("net").unwrap().expect("summary");
        assert_eq!(summary.count, 3);
        assert_eq!(summary.min, 7.0);
        assert_eq!(summary.max, 9.0);
        assert_eq!(summary.last, 9.0);
        assert_eq!(summary.slope, Slope::Rising);
    }

    #[test]
    fn slope_directions() {
        let mut rising = Insights::new(8, 3.0).unwrap();
        for tick in 0..6u64 {
            rising.push("r", tick, tick as f64).unwrap();
        }
        assert_eq!(
            rising.summary("r").unwrap().expect("summary").slope,
            Slope::Rising
        );

        let mut falling = Insights::new(8, 3.0).unwrap();
        for tick in 0..6u64 {
            falling.push("f", tick, (10 - tick) as f64).unwrap();
        }
        assert_eq!(
            falling.summary("f").unwrap().expect("summary").slope,
            Slope::Falling
        );

        let mut flat = Insights::new(8, 3.0).unwrap();
        for tick in 0..6u64 {
            flat.push("k", tick, 5.0).unwrap();
        }
        assert_eq!(
            flat.summary("k").unwrap().expect("summary").slope,
            Slope::Flat
        );
    }

    #[test]
    fn unknown_series_is_reported() {
        let mut insights = Insights::new(8, 3.0).unwrap();
        assert!(matches!(
            insights.set_threshold("nope", 2.0),
            Err(InsightsError::UnknownSeries(_))
        ));
        assert!(matches!(
            insights.summary("nope"),
            Err(InsightsError::UnknownSeries(_))
        ));
        insights.push("real", 0, 1.0).unwrap();
        assert!(insights.summary("real").unwrap().is_some());
    }
}
