//! Runtime policy enforcement for the autonomous AWENET product.
//!
//! The policy is deliberately local and fail-closed: authenticated transport is
//! necessary but not sufficient for application operations. A node evaluates
//! limits and allowed protocol surfaces before accepting work.

use serde::{Deserialize, Serialize};
use std::{fs, io, path::Path};

pub const POLICY_VERSION: u16 = 1;
pub const MESSENGER_STREAM: u32 = 100;
pub const ONECOIN_TRANSFER_STREAM: u32 = 201;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkPolicy {
    pub format: String,
    pub version: u16,
    pub enabled: bool,
    pub require_authenticated_peers: bool,
    pub max_upload_bytes: u64,
    pub max_message_bytes: usize,
    pub max_shard_bytes: usize,
    pub max_bootstrap_peers: usize,
    pub allowed_streams: Vec<u32>,
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self {
            format: "awenet-policy".into(),
            version: POLICY_VERSION,
            enabled: true,
            require_authenticated_peers: true,
            max_upload_bytes: 4 * 1024 * 1024 * 1024,
            max_message_bytes: 64 * 1024,
            max_shard_bytes: 4 * 1024 * 1024,
            max_bootstrap_peers: 64,
            allowed_streams: vec![MESSENGER_STREAM, crate::data_plane::STORAGE_STREAM, ONECOIN_TRANSFER_STREAM],
        }
    }
}

impl NetworkPolicy {
    pub fn validate(&self) -> Result<(), String> {
        if self.format != "awenet-policy" || self.version != POLICY_VERSION {
            return Err("unsupported AWENET policy format/version".into());
        }
        if self.max_upload_bytes == 0 || self.max_message_bytes == 0 || self.max_shard_bytes == 0 {
            return Err("policy limits must be non-zero".into());
        }
        if self.max_bootstrap_peers == 0 {
            return Err("max_bootstrap_peers must be non-zero".into());
        }
        if self.require_authenticated_peers && self.allowed_streams.is_empty() {
            return Err("authenticated policy must declare allowed streams".into());
        }
        Ok(())
    }

    pub fn allows_stream(&self, stream: u32) -> bool {
        self.enabled && self.allowed_streams.contains(&stream)
    }

    pub fn allows_upload(&self, bytes: usize) -> bool {
        self.enabled && (bytes as u64) <= self.max_upload_bytes
    }

    pub fn allows_message(&self, bytes: usize) -> bool {
        self.enabled && bytes <= self.max_message_bytes
    }

    pub fn allows_shard(&self, bytes: usize) -> bool {
        self.enabled && bytes <= self.max_shard_bytes
    }
}

pub fn load_or_create(path: &Path) -> Result<NetworkPolicy, String> {
    if path.exists() {
        let policy: NetworkPolicy =
            serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        policy.validate()?;
        return Ok(policy);
    }

    let policy = NetworkPolicy::default();
    save(&policy, path)?;
    Ok(policy)
}

pub fn save(policy: &NetworkPolicy, path: &Path) -> Result<(), String> {
    policy.validate()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(policy).map_err(|e| e.to_string())?;
    fs::write(path, bytes).map_err(|e| e.to_string())
}

pub fn load_if_changed(path: &Path, previous: &NetworkPolicy) -> Result<NetworkPolicy, String> {
    if !path.exists() {
        return Ok(previous.clone());
    }
    let candidate: NetworkPolicy =
        serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    candidate.validate()?;
    Ok(candidate)
}

pub fn ensure_parent(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_safe_and_valid() {
        let policy = NetworkPolicy::default();
        policy.validate().unwrap();
        assert!(policy.allows_stream(MESSENGER_STREAM));
        assert!(policy.allows_stream(crate::data_plane::STORAGE_STREAM));
        assert!(policy.allows_stream(ONECOIN_TRANSFER_STREAM));
        assert!(!policy.allows_stream(999_999));
    }

    #[test]
    fn limits_fail_closed() {
        let mut policy = NetworkPolicy::default();
        assert!(policy.allows_message(1024));
        policy.enabled = false;
        assert!(!policy.allows_message(1024));
    }
}
