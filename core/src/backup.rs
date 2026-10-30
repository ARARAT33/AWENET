//! Deterministic backup manifests and restore planning.
//!
//! Backup items are content-addressed with SHA-256 digests so a manifest is
//! a stable, replayable description of node state at a point in time.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

pub const MAX_ITEMS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Identity,
    Storage,
    Names,
    Store,
    Messenger,
    Settings,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupItem {
    pub id: String,
    pub kind: ItemKind,
    pub bytes_len: u64,
    pub digest: String,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum BackupError {
    #[error("backup item id is empty")]
    EmptyId,
    #[error("backup manifest is full")]
    ManifestFull,
    #[error("duplicate backup item: {0}")]
    Duplicate(String),
}

/// Content-addressed description of node state at a point in time.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BackupManifest {
    pub created_at_tick: u64,
    items: BTreeMap<String, BackupItem>,
}

impl BackupManifest {
    pub fn new(created_at_tick: u64) -> Self {
        BackupManifest {
            created_at_tick,
            items: BTreeMap::new(),
        }
    }

    /// Adds a content-addressed item and returns its SHA-256 hex digest.
    pub fn add_item(
        &mut self,
        id: &str,
        kind: ItemKind,
        bytes: &[u8],
    ) -> Result<String, BackupError> {
        if id.is_empty() {
            return Err(BackupError::EmptyId);
        }
        if self.items.len() >= MAX_ITEMS {
            return Err(BackupError::ManifestFull);
        }
        if self.items.contains_key(id) {
            return Err(BackupError::Duplicate(id.to_string()));
        }
        let digest = hex::encode(Sha256::digest(bytes));
        self.items.insert(
            id.to_string(),
            BackupItem {
                id: id.to_string(),
                kind,
                bytes_len: bytes.len() as u64,
                digest: digest.clone(),
            },
        );
        Ok(digest)
    }

    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    pub fn total_bytes(&self) -> u64 {
        self.items.values().map(|item| item.bytes_len).sum()
    }

    pub fn kinds(&self) -> BTreeSet<ItemKind> {
        self.items.values().map(|item| item.kind).collect()
    }

    pub fn items(&self) -> impl Iterator<Item = &BackupItem> {
        self.items.values()
    }

    /// Compares the manifest with locally available digests and returns a
    /// restore plan: items that can be restored now and items still missing.
    pub fn plan_restore(&self, local_digests: &BTreeMap<String, String>) -> RestorePlan {
        let mut plan = RestorePlan::default();
        for item in self.items.values() {
            let present = local_digests
                .get(&item.id)
                .is_some_and(|digest| digest == &item.digest);
            if present {
                plan.ready.push(item.id.clone());
            } else {
                plan.missing.push(item.id.clone());
            }
        }
        plan
    }
}

/// Restore plan built by comparing a manifest with local digests.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestorePlan {
    pub ready: Vec<String>,
    pub missing: Vec<String>,
}

impl RestorePlan {
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty()
    }

    pub fn progress_percent(&self) -> usize {
        let total = self.ready.len() + self.missing.len();
        if total == 0 {
            return 100;
        }
        (self.ready.len() * 100) / total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_items_with_stable_digests() {
        let mut manifest = BackupManifest::new(10);
        let d1 = manifest
            .add_item("identity", ItemKind::Identity, b"key")
            .unwrap();
        assert_eq!(d1.len(), 64);
        let duplicate = manifest
            .add_item("identity", ItemKind::Identity, b"key")
            .unwrap_err();
        assert_eq!(duplicate, BackupError::Duplicate("identity".into()));
        let mut other = BackupManifest::new(11);
        assert_eq!(
            other.add_item("identity", ItemKind::Identity, b"key").unwrap(),
            d1
        );
        assert_eq!(manifest.item_count(), 1);
        assert_eq!(manifest.total_bytes(), 3);
        assert_eq!(manifest.kinds(), BTreeSet::from([ItemKind::Identity]));
    }

    #[test]
    fn validates_ids_and_capacity() {
        let mut manifest = BackupManifest::new(0);
        assert_eq!(
            manifest.add_item("", ItemKind::Settings, b"x"),
            Err(BackupError::EmptyId)
        );
        for i in 0..MAX_ITEMS {
            let id = format!("item-{i}");
            manifest.add_item(&id, ItemKind::Storage, b"x").unwrap();
        }
        assert_eq!(
            manifest.add_item("overflow", ItemKind::Storage, b"x"),
            Err(BackupError::ManifestFull)
        );
    }

    #[test]
    fn plans_restore() {
        let mut manifest = BackupManifest::new(0);
        let digest = manifest
            .add_item("identity", ItemKind::Identity, b"key")
            .unwrap();
        let mut local = BTreeMap::new();
        local.insert("identity".to_string(), digest);
        let plan = manifest.plan_restore(&local);
        assert!(plan.is_complete());
        assert_eq!(plan.progress_percent(), 100);
        local.insert("identity".to_string(), "mismatch".to_string());
        let plan = manifest.plan_restore(&local);
        assert!(!plan.is_complete());
        assert_eq!(plan.progress_percent(), 0);
        assert_eq!(plan.missing, vec!["identity".to_string()]);
    }
}
