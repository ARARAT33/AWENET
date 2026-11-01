//! Deterministic state synchronization with conflict resolution.
//!
//! A small, explicit last-writer-wins merge model for replicated state.
//! Every change carries a version and a writer id; higher versions win,
//! and equal versions are broken by the lower writer id, so two nodes that
//! see the same set of changes always converge to the same state without
//! a coordinator, a wall clock or randomness.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_KEYS: usize = 65_536;

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum SyncError {
    #[error("sync store is full (max {MAX_KEYS})")]
    StoreFull,
    #[error("empty keys or writer ids are not allowed")]
    EmptyKey,
}

/// A replicated change to one key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change<V> {
    pub key: String,
    pub value: V,
    pub version: u64,
    pub writer: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// The change advanced the key and is now the local state.
    Applied,
    /// The change lost the merge and was ignored.
    IgnoredStale,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry<V> {
    pub value: V,
    pub version: u64,
    pub writer: String,
}

/// Last-writer-wins replica of a bounded key/value state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StateStore<V: Clone> {
    entries: BTreeMap<String, Entry<V>>,
}

impl<V: Clone> StateStore<V> {
    pub fn new() -> Self {
        StateStore {
            entries: BTreeMap::new(),
        }
    }

    /// Merges one change into the store.
    ///
    /// Higher versions win; on equal versions the lower writer id wins.
    /// The outcome is the same no matter which replica applies the changes
    /// first, so convergence is guaranteed without a coordinator.
    pub fn apply(&mut self, change: Change<V>) -> Result<ApplyOutcome, SyncError> {
        if change.key.is_empty() || change.writer.is_empty() {
            return Err(SyncError::EmptyKey);
        }
        match self.entries.get(&change.key) {
            Some(existing)
                if !loses_to(existing, &change) =>
            {
                return Ok(ApplyOutcome::IgnoredStale);
            }
            None if self.entries.len() >= MAX_KEYS => return Err(SyncError::StoreFull),
            _ => {}
        }
        self.entries.insert(
            change.key,
            Entry {
                value: change.value,
                version: change.version,
                writer: change.writer,
            },
        );
        Ok(ApplyOutcome::Applied)
    }

    pub fn get(&self, key: &str) -> Option<&Entry<V>> {
        self.entries.get(key)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn keys(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    /// Outgoing changes for a peer that already has `known_versions`.
    /// Keys missing from the peer's map are always included.
    pub fn diff(&self, known_versions: &BTreeMap<String, u64>) -> Vec<Change<V>> {
        self.entries
            .iter()
            .filter(|(key, entry)| known_versions.get(*key) != Some(&entry.version))
            .map(|(key, entry)| Change {
                key: key.clone(),
                value: entry.value.clone(),
                version: entry.version,
                writer: entry.writer.clone(),
            })
            .collect()
    }
}

/// True when `existing` must be replaced by `change`.
fn loses_to<V>(existing: &Entry<V>, change: &Change<V>) -> bool {
    if change.version != existing.version {
        return change.version > existing.version;
    }
    change.writer < existing.writer
}

impl<V: Clone> Default for StateStore<V> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(key: &str, value: u32, version: u64, writer: &str) -> Change<u32> {
        Change {
            key: key.to_string(),
            value,
            version,
            writer: writer.to_string(),
        }
    }

    #[test]
    fn applies_changes_in_order() {
        let mut store = StateStore::new();
        assert_eq!(store.apply(change("k", 1, 1, "a")).unwrap(), ApplyOutcome::Applied);
        assert_eq!(store.apply(change("k", 2, 2, "a")).unwrap(), ApplyOutcome::Applied);
        assert_eq!(store.apply(change("k", 1, 1, "b")).unwrap(), ApplyOutcome::IgnoredStale);
        assert_eq!(store.get("k").unwrap().value, 2);
        assert_eq!(store.get("k").unwrap().version, 2);
    }

    #[test]
    fn equal_versions_break_by_writer_id() {
        let mut store = StateStore::new();
        store.apply(change("k", 5, 3, "node-b")).unwrap();
        // Lower writer id wins the tie, regardless of arrival order.
        assert_eq!(store.apply(change("k", 7, 3, "node-a")).unwrap(), ApplyOutcome::Applied);
        assert_eq!(store.get("k").unwrap().value, 7);
        assert_eq!(store.get("k").unwrap().writer, "node-a");
        // A higher writer id loses the same tie.
        let mut other = StateStore::new();
        other.apply(change("k", 7, 3, "node-a")).unwrap();
        assert_eq!(
            other.apply(change("k", 9, 3, "node-b")).unwrap(),
            ApplyOutcome::IgnoredStale
        );
        assert_eq!(other.get("k").unwrap().value, 7);
    }

    #[test]
    fn replicas_converge_from_any_order() {
        let changes = vec![
            change("a", 1, 1, "node-a"),
            change("a", 2, 2, "node-b"),
            change("b", 3, 5, "node-a"),
            change("b", 4, 5, "node-a"),
        ];
        let mut forward = StateStore::new();
        let mut backward = StateStore::new();
        for c in &changes {
            forward.apply(c.clone()).unwrap();
        }
        for c in changes.iter().rev() {
            backward.apply(c.clone()).unwrap();
        }
        assert_eq!(forward.get("a").unwrap().value, backward.get("a").unwrap().value);
        assert_eq!(forward.get("a").unwrap().version, 2);
        assert_eq!(forward.get("b").unwrap().value, backward.get("b").unwrap().value);
        assert_eq!(forward.get("b").unwrap().version, 5);
    }

    #[test]
    fn diff_returns_only_newer_keys() {
        let mut store = StateStore::new();
        store.apply(change("a", 1, 1, "n")).unwrap();
        store.apply(change("b", 2, 2, "n")).unwrap();
        let mut known = std::collections::BTreeMap::new();
        known.insert("a".to_string(), 1);
        let diff = store.diff(&known);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff[0].key, "b");
    }

    #[test]
    fn validates_keys_and_bounds() {
        let mut store = StateStore::new();
        assert_eq!(
            store.apply(change("", 1, 1, "a")).unwrap_err(),
            SyncError::EmptyKey
        );
        assert_eq!(
            store.apply(change("k", 1, 1, "")).unwrap_err(),
            SyncError::EmptyKey
        );
    }

    #[test]
    fn store_is_bounded() {
        let mut store = StateStore::new();
        for i in 0..MAX_KEYS {
            store.apply(change(&format!("k{i}"), 1, 1, "a")).unwrap();
        }
        assert!(matches!(
            store.apply(change("overflow", 1, 1, "a")),
            Err(SyncError::StoreFull)
        ));
    }

    #[test]
    fn serializes_to_json() {
        let mut store = StateStore::new();
        store.apply(change("k", 42, 7, "node-a")).unwrap();
        let json = serde_json::to_string(&store).unwrap();
        let back: StateStore<u32> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.get("k").unwrap().value, 42);
        assert_eq!(back.get("k").unwrap().writer, "node-a");
        assert_eq!(back.len(), 1);
    }
}
