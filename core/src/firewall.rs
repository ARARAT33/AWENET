//! Prefix-based peer firewall with first-match semantics.
//!
//! Deterministic and replay-friendly: pure data, no wall clock and no
//! threads. The first rule whose pattern is a prefix of the peer id decides;
//! when no rule matches, the configured default action applies.

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_RULES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FirewallAction {
    Allow,
    Deny,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FirewallDecision {
    Allowed,
    Denied,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FirewallRule {
    /// Peer-id prefix this rule applies to.
    pub pattern: String,
    pub action: FirewallAction,
    /// Human-readable reason recorded with the decision.
    pub reason: String,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum FirewallError {
    #[error("rule pattern is empty")]
    EmptyPattern,
    #[error("firewall is full")]
    TooManyRules,
    #[error("duplicate rule: {0}")]
    Duplicate(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Firewall {
    default_action: FirewallAction,
    rules: Vec<FirewallRule>,
}

impl Firewall {
    pub fn new(default_action: FirewallAction) -> Self {
        Self {
            default_action,
            rules: Vec::new(),
        }
    }

    pub fn default_action(&self) -> FirewallAction {
        self.default_action
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    pub fn add_rule(&mut self, rule: FirewallRule) -> Result<(), FirewallError> {
        if rule.pattern.is_empty() {
            return Err(FirewallError::EmptyPattern);
        }
        if self.rules.len() >= MAX_RULES {
            return Err(FirewallError::TooManyRules);
        }
        if self.rules.iter().any(|r| r.pattern == rule.pattern) {
            return Err(FirewallError::Duplicate(rule.pattern));
        }
        self.rules.push(rule);
        Ok(())
    }

    /// Returns the decision and the reason of the rule that produced it.
    pub fn check_with_reason(&self, peer_id: &str) -> (FirewallDecision, Option<String>) {
        for rule in &self.rules {
            if peer_id.starts_with(&rule.pattern) {
                let decision = match rule.action {
                    FirewallAction::Allow => FirewallDecision::Allowed,
                    FirewallAction::Deny => FirewallDecision::Denied,
                };
                return (decision, Some(rule.reason.clone()));
            }
        }
        let decision = match self.default_action {
            FirewallAction::Allow => FirewallDecision::Allowed,
            FirewallAction::Deny => FirewallDecision::Denied,
        };
        (decision, None)
    }

    pub fn check(&self, peer_id: &str) -> FirewallDecision {
        self.check_with_reason(peer_id).0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(pattern: &str, action: FirewallAction, reason: &str) -> FirewallRule {
        FirewallRule {
            pattern: pattern.into(),
            action,
            reason: reason.into(),
        }
    }

    #[test]
    fn validates_rules() {
        let mut fw = Firewall::new(FirewallAction::Allow);
        fw.add_rule(rule("awehost", FirewallAction::Deny, "blocked prefix")).unwrap();
        let dup = fw.add_rule(rule("awehost", FirewallAction::Allow, "dup"));
        assert_eq!(dup, Err(FirewallError::Duplicate("awehost".into())));
        assert_eq!(fw.add_rule(rule("", FirewallAction::Deny, "x")), Err(FirewallError::EmptyPattern));
        assert_eq!(fw.rule_count(), 1);
    }

    #[test]
    fn first_match_wins() {
        let mut fw = Firewall::new(FirewallAction::Allow);
        fw.add_rule(rule("awe", FirewallAction::Deny, "wide deny")).unwrap();
        fw.add_rule(rule("awe-friend", FirewallAction::Allow, "friend exception")).unwrap();
        assert_eq!(fw.check("awe-friend-123"), FirewallDecision::Denied);
        assert_eq!(fw.check("awe-evil"), FirewallDecision::Denied);
        assert_eq!(fw.check("other"), FirewallDecision::Allowed);
    }

    #[test]
    fn default_deny_falls_back() {
        let fw = Firewall::new(FirewallAction::Deny);
        assert_eq!(fw.check("stranger"), FirewallDecision::Denied);
        let (decision, reason) = fw.check_with_reason("stranger");
        assert_eq!(decision, FirewallDecision::Denied);
        assert!(reason.is_none());
    }

    #[test]
    fn reasons_travel_with_decisions() {
        let mut fw = Firewall::new(FirewallAction::Allow);
        fw.add_rule(rule("bad", FirewallAction::Deny, "hostile peer")).unwrap();
        let (decision, reason) = fw.check_with_reason("bad-peer");
        assert_eq!(decision, FirewallDecision::Denied);
        assert_eq!(reason.as_deref(), Some("hostile peer"));
    }

    #[test]
    fn serializes_to_json() {
        let mut fw = Firewall::new(FirewallAction::Deny);
        fw.add_rule(rule("ok", FirewallAction::Allow, "trusted")).unwrap();
        let json = serde_json::to_string(&fw).unwrap();
        let back: Firewall = serde_json::from_str(&json).unwrap();
        assert_eq!(back.rule_count(), 1);
        assert_eq!(back.default_action(), FirewallAction::Deny);
    }
}
