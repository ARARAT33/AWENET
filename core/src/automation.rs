//! Metric-driven automation rules with cooldowns and dry-run support.
//!
//! Deterministic and replay-friendly: the engine holds no wall clock and no
//! threads. Time is an explicit tick counter supplied by the caller, so runs
//! are reproducible from the same inputs.

use crate::alerts::ThresholdOp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_RULES: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationAction {
    Notify,
    ShedLoad,
    TriggerBackup,
    PurgeCache,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutomationRule {
    pub name: String,
    pub metric: String,
    pub op: ThresholdOp,
    pub threshold: f64,
    pub action: AutomationAction,
    pub cooldown_ticks: u64,
    pub enabled: bool,
}

impl AutomationRule {
    /// Returns true when the value crosses the rule threshold.
    pub fn evaluate(&self, value: f64) -> bool {
        match self.op {
            ThresholdOp::Greater => value > self.threshold,
            ThresholdOp::Less => value < self.threshold,
        }
    }

    fn validate(&self) -> Result<(), AutomationError> {
        if self.name.is_empty() {
            return Err(AutomationError::EmptyName);
        }
        if self.metric.is_empty() {
            return Err(AutomationError::EmptyMetric);
        }
        if !self.threshold.is_finite() {
            return Err(AutomationError::InvalidThreshold);
        }
        Ok(())
    }
}

/// An automation that fired at a given tick.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TriggeredAutomation {
    pub rule: String,
    pub action: AutomationAction,
    pub tick: u64,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum AutomationError {
    #[error("rule name is empty")]
    EmptyName,
    #[error("rule metric is empty")]
    EmptyMetric,
    #[error("threshold must be a finite number")]
    InvalidThreshold,
    #[error("automation engine is full")]
    TooManyRules,
    #[error("duplicate rule: {0}")]
    Duplicate(String),
}

/// Automation engine that maps gauge values to actions with cooldowns.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AutomationEngine {
    rules: Vec<AutomationRule>,
    last_fired: BTreeMap<String, u64>,
}

impl AutomationEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_rule(&mut self, rule: AutomationRule) -> Result<(), AutomationError> {
        rule.validate()?;
        if self.rules.len() >= MAX_RULES {
            return Err(AutomationError::TooManyRules);
        }
        if self.rules.iter().any(|r| r.name == rule.name) {
            return Err(AutomationError::Duplicate(rule.name));
        }
        self.rules.push(rule);
        Ok(())
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    fn fireable(&self, gauges: &BTreeMap<String, f64>, tick: u64) -> Vec<TriggeredAutomation> {
        self.rules
            .iter()
            .filter(|rule| rule.enabled)
            .filter(|rule| {
                let Some(last) = self.last_fired.get(&rule.name) else {
                    return true;
                };
                tick.saturating_sub(*last) >= rule.cooldown_ticks
            })
            .filter_map(|rule| {
                let value = gauges.get(&rule.metric)?;
                rule.evaluate(*value).then(|| TriggeredAutomation {
                    rule: rule.name.clone(),
                    action: rule.action,
                    tick,
                })
            })
            .collect()
    }

    /// Dry-run: returns the automations that would fire at this tick without
    /// recording them, so operators can preview a rule set safely.
    pub fn plan(&self, gauges: &BTreeMap<String, f64>, tick: u64) -> Vec<TriggeredAutomation> {
        self.fireable(gauges, tick)
    }

    /// Evaluates all enabled rules against the gauge values and records the
    /// firing ticks, honouring each rule's cooldown.
    pub fn evaluate(
        &mut self,
        gauges: &BTreeMap<String, f64>,
        tick: u64,
    ) -> Vec<TriggeredAutomation> {
        let fired = self.fireable(gauges, tick);
        for trigger in &fired {
            self.last_fired.insert(trigger.rule.clone(), tick);
        }
        fired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(
        name: &str,
        metric: &str,
        op: ThresholdOp,
        threshold: f64,
        cooldown: u64,
    ) -> AutomationRule {
        AutomationRule {
            name: name.into(),
            metric: metric.into(),
            op,
            threshold,
            action: AutomationAction::ShedLoad,
            cooldown_ticks: cooldown,
            enabled: true,
        }
    }

    fn gauges(metric: &str, value: f64) -> BTreeMap<String, f64> {
        let mut g = BTreeMap::new();
        g.insert(metric.to_string(), value);
        g
    }

    #[test]
    fn validates_rules() {
        let mut engine = AutomationEngine::new();
        engine
            .add_rule(rule("a", "cpu", ThresholdOp::Greater, 80.0, 5))
            .unwrap();
        let dup = engine.add_rule(rule("a", "cpu", ThresholdOp::Greater, 80.0, 5));
        assert_eq!(dup, Err(AutomationError::Duplicate("a".into())));
        let mut empty_name = rule("b", "cpu", ThresholdOp::Greater, 80.0, 5);
        empty_name.name = String::new();
        assert_eq!(engine.add_rule(empty_name), Err(AutomationError::EmptyName));
        let bad = rule("c", "cpu", ThresholdOp::Greater, f64::NAN, 5);
        assert_eq!(engine.add_rule(bad), Err(AutomationError::InvalidThreshold));
        assert_eq!(engine.rule_count(), 1);
    }

    #[test]
    fn fires_once_then_respects_cooldown() {
        let mut engine = AutomationEngine::new();
        engine
            .add_rule(rule("shed", "load", ThresholdOp::Greater, 0.9, 3))
            .unwrap();
        let hot = gauges("load", 0.95);
        assert_eq!(engine.evaluate(&hot, 10).len(), 1);
        assert!(engine.evaluate(&hot, 11).is_empty());
        assert!(engine.evaluate(&hot, 12).is_empty());
        assert_eq!(engine.evaluate(&hot, 13).len(), 1);
    }

    #[test]
    fn plan_is_a_dry_run() {
        let mut engine = AutomationEngine::new();
        engine
            .add_rule(rule("shed", "load", ThresholdOp::Greater, 0.9, 3))
            .unwrap();
        let hot = gauges("load", 0.95);
        assert_eq!(engine.plan(&hot, 1).len(), 1);
        assert_eq!(engine.plan(&hot, 1).len(), 1);
        assert!(engine.evaluate(&hot, 1).is_empty() == false);
    }

    #[test]
    fn disabled_rules_and_missing_metrics_never_fire() {
        let mut engine = AutomationEngine::new();
        let mut off = rule("off", "cpu", ThresholdOp::Greater, 10.0, 0);
        off.enabled = false;
        engine.add_rule(off).unwrap();
        engine
            .add_rule(rule("gone", "mem", ThresholdOp::Greater, 10.0, 0))
            .unwrap();
        let gauges = gauges("cpu", 99.0);
        assert!(engine.evaluate(&gauges, 1).is_empty());
    }

    #[test]
    fn serializes_to_json() {
        let mut engine = AutomationEngine::new();
        engine
            .add_rule(rule("r", "m", ThresholdOp::Less, 1.0, 2))
            .unwrap();
        let json = serde_json::to_string(&engine).unwrap();
        let back: AutomationEngine = serde_json::from_str(&json).unwrap();
        assert_eq!(back.rule_count(), 1);
    }
}
