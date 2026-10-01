use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerAnnouncement {
    pub node_id: String,
    pub addresses: Vec<String>,
    pub public_key: [u8; 32],
    pub expires_at_unix: u64,
    pub sequence: u64,
}

impl PeerAnnouncement {
    pub fn validate(&self, now_unix: u64, max_lifetime: u64) -> Result<(), String> {
        if self.node_id.is_empty() || self.node_id.len() > 256 { return Err("invalid node id".into()); }
        if self.addresses.len() > 16 { return Err("too many addresses".into()); }
        if self.expires_at_unix <= now_unix || self.expires_at_unix.saturating_sub(now_unix) > max_lifetime {
            return Err("peer announcement expired or lifetime too long".into());
        }
        if self.addresses.iter().any(|a| a.len() > 256 || a.is_empty()) { return Err("invalid peer address".into()); }
        Ok(())
    }
}

#[derive(Default, Debug)]
pub struct PeerDirectory { records: HashMap<String, PeerAnnouncement> }

impl PeerDirectory {
    pub fn upsert(&mut self, record: PeerAnnouncement, now_unix: u64, max_lifetime: u64) -> Result<bool, String> {
        record.validate(now_unix, max_lifetime)?;
        let accept = self.records.get(&record.node_id).map(|old| record.sequence > old.sequence).unwrap_or(true);
        if accept { self.records.insert(record.node_id.clone(), record); }
        Ok(accept)
    }
    pub fn expire(&mut self, now_unix: u64) { self.records.retain(|_, r| r.expires_at_unix > now_unix); }
    pub fn get(&self, node_id: &str, now_unix: u64) -> Option<&PeerAnnouncement> {
        self.records.get(node_id).filter(|r| r.expires_at_unix > now_unix)
    }
    pub fn len(&self) -> usize { self.records.len() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn newest_sequence_wins() {
        let mut d=PeerDirectory::default();
        let base=PeerAnnouncement{node_id:"n".into(),addresses:vec!["127.0.0.1:1".into()],public_key:[1;32],expires_at_unix:100,sequence:2};
        assert!(d.upsert(base.clone(),10,1000).unwrap());
        let mut old=base; old.sequence=1;
        assert!(!d.upsert(old,10,1000).unwrap());
    }
}
