//! AWENET open resource and compatibility contract.
use crate::identity::AweId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const AWENET_PROTOCOL: &str = "AWENET/1";
pub const AWENET_PROTOCOL_MAJOR: u16 = 1;
pub const MIN_REPLICATION: u8 = 3;
pub const MAX_ALIAS_LEN: usize = 128;
pub const MAX_CAPABILITIES: usize = 64;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceOffer {
    pub storage_bytes: u64,
    pub bandwidth_bytes_per_day: u64,
    pub compute_units_per_day: u64,
    pub relay_bytes_per_day: u64,
    pub online_hours_per_day: u16,
}
impl ResourceOffer {
    pub fn validate(&self) -> Result<(), String> {
        if self.online_hours_per_day > 24 { return Err("online_hours_per_day must be <= 24".into()); }
        if self.storage_bytes == 0 && self.bandwidth_bytes_per_day == 0 && self.compute_units_per_day == 0
            && self.relay_bytes_per_day == 0 && self.online_hours_per_day == 0 {
            return Err("a node must offer at least one resource".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum Capability {
    Routing, Discovery, Storage, Relay, Compute, Browser, Messaging, Calls, Sites, Apps, Store,
}
impl Capability {
    pub fn wire_name(&self) -> &'static str {
        match self {
            Self::Routing=>"routing", Self::Discovery=>"discovery", Self::Storage=>"storage",
            Self::Relay=>"relay", Self::Compute=>"compute", Self::Browser=>"browser",
            Self::Messaging=>"messaging", Self::Calls=>"calls", Self::Sites=>"sites",
            Self::Apps=>"apps", Self::Store=>"store",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeAdvertisement {
    pub protocol: String,
    pub awe_id: AweId,
    pub capabilities: BTreeSet<Capability>,
    pub offer: ResourceOffer,
    pub software: String,
    pub software_version: String,
}
impl NodeAdvertisement {
    pub fn validate(&self) -> Result<(), String> {
        if self.protocol != AWENET_PROTOCOL { return Err("unsupported AWENET protocol".into()); }
        if self.capabilities.len() > MAX_CAPABILITIES { return Err("too many advertised capabilities".into()); }
        self.offer.validate()
    }
    pub fn compatible_with(&self, required: &BTreeSet<Capability>) -> bool {
        required.is_subset(&self.capabilities)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NameBinding {
    pub name: String,
    pub resource_id: [u8; 32],
    pub owner: AweId,
    pub revision: u64,
    pub secret: bool,
}
impl NameBinding {
    pub fn validate(&self) -> Result<(), String> {
        let n = self.name.trim();
        if n.is_empty() || n.len() > MAX_ALIAS_LEN || n.chars().any(|c| c.is_control()) {
            return Err("invalid AWENET resource name".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceDescriptor {
    pub resource_id: [u8; 32],
    pub owner: AweId,
    pub name: Option<String>,
    pub size_bytes: u64,
    pub mime: Option<String>,
    pub replicas: u8,
    pub access_commitment: Option<[u8; 32]>,
}
impl ResourceDescriptor {
    pub fn is_secret(&self) -> bool { self.access_commitment.is_some() }
    pub fn validate(&self) -> Result<(), String> {
        if self.replicas < MIN_REPLICATION { return Err(format!("resource requires at least {MIN_REPLICATION} replicas")); }
        if let Some(n) = &self.name {
            if n.trim().is_empty() || n.len() > MAX_ALIAS_LEN { return Err("invalid resource name".into()); }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContributionReceipt {
    pub node: AweId,
    pub storage_byte_hours: u128,
    pub relay_bytes: u64,
    pub bandwidth_bytes: u64,
    pub compute_units: u64,
    pub uptime_minutes: u64,
    pub period_start_unix: u64,
    pub period_end_unix: u64,
}
impl ContributionReceipt {
    pub fn score(&self) -> u128 {
        self.storage_byte_hours
            .saturating_add(self.relay_bytes as u128)
            .saturating_add(self.bandwidth_bytes as u128)
            .saturating_add((self.compute_units as u128).saturating_mul(4))
            .saturating_add((self.uptime_minutes as u128).saturating_mul(1024))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContributionPolicy {
    pub free_priority: u8,
    pub max_priority: u8,
}
impl Default for ContributionPolicy {
    fn default() -> Self { Self { free_priority: 1, max_priority: 100 } }
}
impl ContributionPolicy {
    pub fn priority(&self, score: u128) -> u8 {
        if score == 0 { return self.free_priority; }
        self.free_priority.max(score.saturating_div(1024).min(self.max_priority as u128) as u8)
            .min(self.max_priority)
    }
}

#[derive(Clone, Debug, Default)]
pub struct ResourceDirectory {
    pub nodes: BTreeMap<AweId, NodeAdvertisement>,
    pub names: BTreeMap<String, NameBinding>,
    pub resources: BTreeMap<[u8; 32], ResourceDescriptor>,
}
impl ResourceDirectory {
    pub fn upsert_node(&mut self, node: NodeAdvertisement) -> Result<(), String> {
        node.validate()?; self.nodes.insert(node.awe_id.clone(), node); Ok(())
    }
    pub fn bind_name(&mut self, binding: NameBinding) -> Result<(), String> {
        binding.validate()?;
        if let Some(old) = self.names.get(&binding.name) {
            if old.owner != binding.owner { return Err("resource name is already owned".into()); }
            if binding.revision <= old.revision { return Err("name binding revision must increase".into()); }
        }
        self.names.insert(binding.name.clone(), binding); Ok(())
    }
    pub fn publish_resource(&mut self, resource: ResourceDescriptor) -> Result<(), String> {
        resource.validate()?; self.resources.insert(resource.resource_id, resource); Ok(())
    }
    pub fn resolve_name(&self, name: &str) -> Option<&NameBinding> { self.names.get(name) }
    pub fn nodes_for(&self, required: &BTreeSet<Capability>) -> Vec<&NodeAdvertisement> {
        self.nodes.values().filter(|n| n.compatible_with(required)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{Identity, Username};
    fn id() -> AweId { Identity::generate(Username::new("test").unwrap()).public.awe_id }
    #[test]
    fn rename_keeps_resource_id() {
        let owner=id(); let rid=[7u8;32]; let mut d=ResourceDirectory::default();
        d.bind_name(NameBinding{name:"old".into(),resource_id:rid,owner:owner.clone(),revision:1,secret:false}).unwrap();
        d.bind_name(NameBinding{name:"new".into(),resource_id:rid,owner,revision:2,secret:false}).unwrap();
        assert_eq!(d.resolve_name("new").unwrap().resource_id,rid);
    }
    #[test]
    fn foreign_owner_cannot_take_name() {
        let mut d=ResourceDirectory::default();
        d.bind_name(NameBinding{name:"site".into(),resource_id:[1;32],owner:id(),revision:1,secret:false}).unwrap();
        assert!(d.bind_name(NameBinding{name:"site".into(),resource_id:[2;32],owner:id(),revision:2,secret:false}).is_err());
    }
    #[test]
    fn capability_matching_works() {
        let mut caps=BTreeSet::new(); caps.insert(Capability::Storage);
        let n=NodeAdvertisement{protocol:AWENET_PROTOCOL.into(),awe_id:id(),capabilities:caps,offer:ResourceOffer{storage_bytes:1,..Default::default()},software:"test".into(),software_version:"1".into()};
        n.validate().unwrap(); let mut required=BTreeSet::new(); required.insert(Capability::Storage);
        assert!(n.compatible_with(&required));
    }
}
