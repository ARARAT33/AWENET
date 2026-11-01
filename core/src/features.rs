//! Deterministic feature flag registry.
//!
//! Flag rollout is bucketed by a stable hash of the subject key, so the
//! same subject always resolves the same way for the same flag state, and
//! results are reproducible across nodes and replays. No wall clock, no
//! threads, no randomness.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_FLAGS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum FeatureError {
    #[error("flag registry is full (max {MAX_FLAGS})")]
    RegistryFull,
    #[error("unknown flag '{0}'")]
    UnknownFlag(String),
    #[error("rollout percent must be within 0..=100, got {0}")]
    InvalidRollout(u8),
    #[error("empty flag names are not allowed")]
    EmptyName,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flag {
    /// Global kill switch: a disabled flag never evaluates true.
    pub enabled: bool,
    /// Percent of hash buckets that see the feature (0..=100).
    pub rollout_percent: u8,
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FeatureFlags {
    flags: BTreeMap<String, Flag>,
}

/// FNV-1a hash, used only for stable deterministic bucketing.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

impl FeatureFlags {
    pub fn new() -> Self {
        FeatureFlags {
            flags: BTreeMap::new(),
        }
    }

    /// Registers or updates a flag with a rollout percentage.
    pub fn register(
        &mut self,
        name: &str,
        enabled: bool,
        rollout_percent: u8,
        description: &str,
    ) -> Result<(), FeatureError> {
        if name.is_empty() {
            return Err(FeatureError::EmptyName);
        }
        if rollout_percent > 100 {
            return Err(FeatureError::InvalidRollout(rollout_percent));
        }
        if !self.flags.contains_key(name) && self.flags.len() >= MAX_FLAGS {
            return Err(FeatureError::RegistryFull);
        }
        self.flags.insert(
            name.to_string(),
            Flag {
                enabled,
                rollout_percent,
                description: description.to_string(),
            },
        );
        Ok(())
    }

    pub fn set_enabled(&mut self, name: &str, enabled: bool) -> Result<(), FeatureError> {
        let flag = self
            .flags
            .get_mut(name)
            .ok_or_else(|| FeatureError::UnknownFlag(name.to_string()))?;
        flag.enabled = enabled;
        Ok(())
    }

    pub fn set_rollout(&mut self, name: &str, percent: u8) -> Result<(), FeatureError> {
        if percent > 100 {
            return Err(FeatureError::InvalidRollout(percent));
        }
        let flag = self
            .flags
            .get_mut(name)
            .ok_or_else(|| FeatureError::UnknownFlag(name.to_string()))?;
        flag.rollout_percent = percent;
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Flag> {
        self.flags.get(name)
    }

    pub fn len(&self) -> usize {
        self.flags.len()
    }

    pub fn is_empty(&self) -> bool {
        self.flags.is_empty()
    }

    pub fn names(&self) -> Vec<String> {
        self.flags.keys().cloned().collect()
    }

    /// Evaluates a flag for a subject key. Deterministic: the same
    /// (flag state, subject) pair always yields the same answer.
    pub fn is_enabled(&self, name: &str, subject: &str) -> Result<bool, FeatureError> {
        let flag = self
            .flags
            .get(name)
            .ok_or_else(|| FeatureError::UnknownFlag(name.to_string()))?;
        if !flag.enabled || flag.rollout_percent == 0 {
            return Ok(false);
        }
        if flag.rollout_percent == 100 {
            return Ok(true);
        }
        let bucket = (fnv1a(name.as_bytes()) ^ fnv1a(subject.as_bytes())) % 100;
        Ok(bucket < u64::from(flag.rollout_percent))
    }
}

impl Default for FeatureFlags {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_and_evaluates() {
        let mut flags = FeatureFlags::new();
        flags
            .register("messenger.calls", true, 100, "Calls")
            .unwrap();
        assert!(flags.is_enabled("messenger.calls", "anyone").unwrap());
        assert_eq!(flags.len(), 1);
    }

    #[test]
    fn validates_input() {
        let mut flags = FeatureFlags::new();
        assert_eq!(
            flags.register("", true, 100, "").unwrap_err(),
            FeatureError::EmptyName
        );
        assert_eq!(
            flags.register("x", true, 101, "").unwrap_err(),
            FeatureError::InvalidRollout(101)
        );
        flags.register("x", true, 100, "").unwrap();
        assert_eq!(
            flags.is_enabled("missing", "s").unwrap_err(),
            FeatureError::UnknownFlag("missing".to_string())
        );
    }

    #[test]
    fn kill_switch_overrides_rollout() {
        let mut flags = FeatureFlags::new();
        flags.register("f", false, 100, "").unwrap();
        assert!(!flags.is_enabled("f", "subject").unwrap());
        flags.set_enabled("f", true).unwrap();
        assert!(flags.is_enabled("f", "subject").unwrap());
    }

    #[test]
    fn zero_rollout_disables_everyone() {
        let mut flags = FeatureFlags::new();
        flags.register("f", true, 0, "").unwrap();
        for i in 0..50 {
            assert!(!flags.is_enabled("f", &format!("s{i}")).unwrap());
        }
    }

    #[test]
    fn evaluation_is_deterministic_and_stable() {
        let mut flags = FeatureFlags::new();
        flags.register("f", true, 50, "").unwrap();
        // The same subject resolves the same way on every call.
        for subject in ["a", "b", "c"] {
            let first = flags.is_enabled("f", subject).unwrap();
            for _ in 0..5 {
                assert_eq!(flags.is_enabled("f", subject).unwrap(), first);
            }
        }
        // A 50% rollout reaches a meaningful share of subjects, not all.
        let enabled: usize = (0..200)
            .filter(|i| flags.is_enabled("f", &format!("s{i}")).unwrap())
            .count();
        assert!(enabled > 0 && enabled < 200, "enabled = {enabled}");
        // Widening the rollout never removes a subject from the enabled set.
        flags.set_rollout("f", 60).unwrap();
        let wider: usize = (0..200)
            .filter(|i| flags.is_enabled("f", &format!("s{i}")).unwrap())
            .count();
        assert!(wider >= enabled);
    }

    #[test]
    fn registry_is_bounded() {
        let mut flags = FeatureFlags::new();
        for i in 0..MAX_FLAGS {
            flags.register(&format!("flag{i}"), true, 100, "").unwrap();
        }
        assert!(matches!(
            flags.register("overflow", true, 100, ""),
            Err(FeatureError::RegistryFull)
        ));
        // Updating an existing flag is always allowed.
        flags.register("flag0", false, 0, "").unwrap();
    }

    #[test]
    fn serializes_to_json() {
        let mut flags = FeatureFlags::new();
        flags.register("f", true, 50, "desc").unwrap();
        let json = serde_json::to_string(&flags).unwrap();
        let back: FeatureFlags = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back.get("f").unwrap().description, "desc");
        let a = flags.is_enabled("f", "subject").unwrap();
        assert_eq!(back.is_enabled("f", "subject").unwrap(), a);
    }
}
