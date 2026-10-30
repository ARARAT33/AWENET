//! Threshold alert rules evaluated over product gauges.
//!
//! Deterministic and replay-friendly: the engine holds no wall clock and no
//! threads, so alert transitions are reproducible from the same inputs.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_RULES: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThresholdOp {
    Greater,
    Less,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlertRule {
    pub name: String,
    pub metric: String,
    pub op: ThresholdOp,
    pub threshold: f64,
    pub severity: AlertSeverity,
    pub message: String,
}

impl AlertRule {
    /// Returns true when the value crosses the rule threshold.
    pub fn evaluate(&self, value: f64) -> bool {
        match self.op {
            ThresholdOp::Greater => value > self.threshold,
            ThresholdOp::Less => value < self.threshold,
        }
    }

    fn validate(&self) -> Result<(), AlertError> {
        if self.name.is_empty() {
            return Err(AlertError::EmptyName);
        }
        if self.metric.is_empty() {
            return Err(AlertError::EmptyMetric);
        }
        if !self.threshold.is_finite() {
            return Err(AlertError::InvalidThreshold);
        }
        Ok(())
    }
}

/// A transitioned alert, produced when a rule starts or stops firing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Alert {
    pub rule: String,
    pub severity: AlertSeverity,
    pub message: String,
    pub firing: bool,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum AlertError {
    #[error("rule name is empty")]
    EmptyName,
    #[error("rule metric is empty")]
    EmptyMetric,
    #[error("threshold must be a finite number")]
    InvalidThreshold,
    #[error("alert engine is full")]
    TooManyRules,
    #[error("duplicate rule: {0}")]
    Duplicate(String),
}

/// Threshold alert engine with explicit firing-state transitions.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AlertEngine {
    rules: Vec<AlertRule>,
    active: BTreeMap<String, AlertSeverity>,
}

impl AlertEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_rule(&mut self, rule: AlertRule) -> Result<(), AlertError> {
        rule.validate()?;
        if self.rules.len() >= MAX_RULES {
            return Err(AlertError::TooManyRules);
        }
        if self.rules.iter().any(|r| r.name == rule.name) {
            return Err(AlertError::Duplicate(rule.name));
        }
        self.rules.push(rule);
        Ok(())
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// Evaluates every rule against the current gauge values and returns the
    /// alerts whose firing state changed since the previous evaluation.
    /// Rules whose metric is missing keep their previous state untouched.
    pub fn evaluate(&mut self, gauges: &BTreeMap<String, f64>) -> Vec<Alert> {
        let mut transitions = Vec::new();
        let mut next = BTreeMap::new();
        for rule in &self.rules {
            let Some(value) = gauges.get(&rule.metric) else {
                if let Some(severity) = self.active.get(&rule.name) {
                    next.insert(rule.name.clone(), *severity);
                }
                continue;
            };
            let firing = rule.evaluate(*value);
            if firing {
                next.insert(rule.name.clone(), rule.severity);
            }
            let was_firing = self.active.contains_key(&rule.name);
            if firing != was_firing {
                transitions.push(Alert {
                    rule: rule.name.clone(),
                    severity: rule.severity,
                    message: rule.message.clone(),
                    firing,
                });
            }
        }
        self.active = next;
        transitions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(name: &str, metric: &str, op: ThresholdOp, threshold: f64) -> AlertRule {
        AlertRule {
            name: name.into(),
            metric: metric.into(),
            op,
            threshold,
            severity: AlertSeverity::Warning,
            message: "check".into(),
        }
    }

    #[test]
    fn validates_rules() {
        let mut engine = AlertEngine::new();
        let good = rule("a", "cpu", ThresholdOp::Greater, 80.0);
        engine.add_rule(good.clone()).unwrap();
        assert_eq!(
            engine.add_rule(good.clone()),
            Err(AlertError::Duplicate("a".into()))
        );
        let mut empty_name = rule("b", "cpu", ThresholdOp::Greater, 80.0);
        empty_name.name = String::new();
        assert_eq!(engine.add_rule(empty_name), Err(AlertError::EmptyName));
        let mut empty_metric = rule("c", "", ThresholdOp::Greater, 80.0);
        empty_metric.metric = String::new();
        assert_eq!(engine.add_rule(empty_metric), Err(AlertError::EmptyMetric));
        let bad_threshold = rule("d", "cpu", ThresholdOp::Greater, f64::NAN);
        assert_eq!(
            engine.add_rule(bad_threshold),
            Err(AlertError::InvalidThreshold)
        );
        assert_eq!(engine.rule_count(), 1);
    }

    #[test]
    fn fires_on_threshold_cross() {
        let mut engine = AlertEngine::new();
        engine
            .add_rule(rule("high_cpu", "cpu", ThresholdOp::Greater, 80.0))
            .unwrap();
        let mut gauges = BTreeMap::new();
        gauges.insert("cpu".to_string(), 50.0);
        assert!(engine.evaluate(&gauges).is_empty());
        gauges.insert("cpu".to_string(), 90.0);
        let alerts = engine.evaluate(&gauges);
        assert_eq!(alerts.len(), 1);
        assert!(alerts[0].firing);
        assert_eq!(engine.active_count(), 1);
        gauges.insert("cpu".to_string(), 40.0);
        let alerts = engine.evaluate(&gauges);
        assert_eq!(alerts.len(), 1);
        assert!(!alerts[0].firing);
        assert_eq!(engine.active_count(), 0);
    }

    #[test]
    fn less_op_and_missing_metric_keep_state() {
        let mut engine = AlertEngine::new();
        engine
            .add_rule(rule("low_disk", "disk_pct", ThresholdOp::Less, 10.0))
            .unwrap();
        let mut gauges = BTreeMap::new();
        gauges.insert("disk_pct".to_string(), 5.0);
        assert_eq!(engine.evaluate(&gauges).len(), 1);
        let mut without = BTreeMap::new();
        without.insert("other".to_string(), 1.0);
        assert!(engine.evaluate(&without).is_empty());
        assert_eq!(engine.active_count(), 1);
    }

    #[test]
    fn serializes_to_json() {
        let mut engine = AlertEngine::new();
        engine
            .add_rule(rule("r", "m", ThresholdOp::Greater, 1.0))
            .unwrap();
        let json = serde_json::to_string(&engine).unwrap();
        let back: AlertEngine = serde_json::from_str(&json).unwrap();
        assert_eq!(back.rule_count(), 1);
    }
}
