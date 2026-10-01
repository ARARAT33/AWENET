use crate::identity::Identity;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerAnnouncement {
    pub node_id: String,
    pub addresses: Vec<String>,
    pub public_key: [u8; 32],
    pub expires_at_unix: u64,
    pub sequence: u64,
    pub signature: [u8; 64],
}

impl PeerAnnouncement {
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"AWE/DISCOVERY/v1");
        out.extend_from_slice(self.node_id.as_bytes());
        out.extend_from_slice(&[0]);
        for a in &self.addresses { out.extend_from_slice(a.as_bytes()); out.extend_from_slice(&[0]); }
        out.extend_from_slice(&self.public_key);
        out.extend_from_slice(&self.expires_at_unix.to_be_bytes());
        out.extend_from_slice(&self.sequence.to_be_bytes());
        out
    }
    pub fn verify_signature(&self) -> bool { Identity::verify(&self.public_key, &self.signing_bytes(), &self.signature) }

    pub fn validate(&self, now_unix: u64, max_lifetime: u64) -> Result<(), String> {
        if self.node_id.is_empty() || self.node_id.len() > 256 { return Err("invalid node id".into()); }
        if !self.verify_signature() { return Err("invalid peer announcement signature".into()); }
        if self.addresses.len() > 16 { return Err("too many addresses".into()); }
        if self.expires_at_unix <= now_unix || self.expires_at_unix.saturating_sub(now_unix) > max_lifetime {
            return Err("peer announcement expired or lifetime too long".into());
        }
        if self.addresses.iter().any(|a| a.len() > 256 || a.is_empty()) { return Err("invalid peer address".into()); }
        Ok(())
    }
}

#[derive(Default, Debug)]
pub struct PeerDirectory { records: HashMap<String, PeerAnnouncement>, max_records: usize }

impl PeerDirectory {
    pub fn with_capacity(max_records: usize) -> Self { Self { records: HashMap::new(), max_records: max_records.max(1) } }

    pub fn upsert(&mut self, record: PeerAnnouncement, now_unix: u64, max_lifetime: u64) -> Result<bool, String> {
        record.validate(now_unix, max_lifetime)?;
        let accept = self.records.get(&record.node_id).map(|old| record.sequence > old.sequence).unwrap_or(true);
        if accept {
            if !self.records.contains_key(&record.node_id) && self.records.len() >= self.max_records {
                if let Some(oldest) = self.records.iter().min_by_key(|(_, r)| (r.sequence, r.expires_at_unix)).map(|(id, _)| id.clone()) { self.records.remove(&oldest); }
            }
            self.records.insert(record.node_id.clone(), record);
        }
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
        let mut d=PeerDirectory::with_capacity(8);
        let id=Identity::generate(crate::identity::Username::new("test").unwrap());
        let mut base=PeerAnnouncement{node_id:"n".into(),addresses:vec!["127.0.0.1:1".into()],public_key:id.public.public_key,expires_at_unix:100,sequence:2,signature:[0;64]};
        base.signature=id.sign(&base.signing_bytes());
        assert!(d.upsert(base.clone(),10,1000).unwrap());
        let mut old=base; old.sequence=1;
        assert!(!d.upsert(old,10,1000).unwrap());
    }
}
