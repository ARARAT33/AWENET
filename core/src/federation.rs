use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

pub const FORMAT_VERSION: u16 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AweNodeConfig {
    pub format: String,
    pub version: u16,
    pub node_id: String,
    pub data_centre_id: String,
    pub data_centre_name: String,
    pub issued_at_unix: u64,
    pub endpoint: String,
    pub bootstrap_endpoints: Vec<String>,
    pub join_token: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DataCentreConfig {
    pub format: String,
    pub version: u16,
    pub data_centre_id: String,
    pub data_centre_name: String,
    pub owner_node_id: String,
    pub issued_at_unix: u64,
    pub endpoints: Vec<String>,
    pub member_node_ids: Vec<String>,
    pub peer_data_centres: Vec<String>,
    pub join_token: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DataGroupConfig {
    pub format: String,
    pub version: u16,
    pub data_group_id: String,
    pub data_group_name: String,
    pub issued_at_unix: u64,
    pub owner_data_centre_id: String,
    pub data_centre_ids: Vec<String>,
    pub peer_data_groups: Vec<String>,
    pub join_token: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AweNetConfig {
    pub format: String,
    pub version: u16,
    pub local_node_id: String,
    pub local_data_centre_id: Option<String>,
    pub local_data_group_id: Option<String>,
    pub joined_data_centres: Vec<String>,
    pub joined_data_groups: Vec<String>,
    pub bootstrap_endpoints: Vec<String>,
}

fn token(prefix: &str, seed: &str) -> String {
    let mut bytes = Vec::with_capacity(prefix.len() + seed.len() + 8);
    bytes.extend_from_slice(prefix.as_bytes());
    bytes.extend_from_slice(seed.as_bytes());
    bytes.extend_from_slice(&std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default().as_nanos().to_be_bytes());
    format!("{}-{}", prefix, hex::encode(blake3::hash(&bytes).as_bytes())[..32].to_string())
}

fn id(prefix: &str, seed: &str) -> String {
    let digest = blake3::hash(format!("AWE/{prefix}/v1/{seed}").as_bytes());
    format!("{prefix}-{}", &hex::encode(digest.as_bytes())[..24])
}

pub fn generate_awenode(node_id: &str, name: &str, endpoint: &str, bootstrap: Vec<String>, now: u64) -> AweNodeConfig {
    generate_awenode_for_dc(node_id, &id("dc", &format!("{node_id}:{name}")), name, endpoint, bootstrap, now)
}

pub fn generate_awenode_for_dc(node_id: &str, data_centre_id: &str, name: &str, endpoint: &str, bootstrap: Vec<String>, now: u64) -> AweNodeConfig {
    AweNodeConfig {
        format: "awenode".into(),
        version: FORMAT_VERSION,
        node_id: node_id.into(),
        data_centre_id: data_centre_id.into(),
        data_centre_name: name.into(),
        issued_at_unix: now,
        endpoint: endpoint.into(),
        bootstrap_endpoints: bootstrap,
        join_token: token("node", node_id),
    }
}

pub fn generate_awedc(owner_node_id: &str, dc_id: &str, name: &str, endpoints: Vec<String>, members: Vec<String>, now: u64) -> DataCentreConfig {
    DataCentreConfig {
        format: "awedc".into(),
        version: FORMAT_VERSION,
        data_centre_id: dc_id.into(),
        data_centre_name: name.into(),
        owner_node_id: owner_node_id.into(),
        issued_at_unix: now,
        endpoints,
        member_node_ids: members,
        peer_data_centres: Vec::new(),
        join_token: token("dc", dc_id),
    }
}

pub fn generate_dgc(owner_dc_id: &str, name: &str, data_centres: Vec<String>, now: u64) -> DataGroupConfig {
    let mut sorted = data_centres;
    sorted.sort();
    sorted.dedup();
    DataGroupConfig {
        format: "dgc".into(),
        version: FORMAT_VERSION,
        data_group_id: id("dg", &format!("{owner_dc_id}:{name}:{}", sorted.join(","))),
        data_group_name: name.into(),
        issued_at_unix: now,
        owner_data_centre_id: owner_dc_id.into(),
        data_centre_ids: sorted,
        peer_data_groups: Vec::new(),
        join_token: token("dg", owner_dc_id),
    }
}

pub fn save_json<T: Serialize>(value: &T, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    let data = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    fs::write(path, data).map_err(|e| e.to_string())
}

pub fn load_awenode(path: &Path) -> Result<AweNodeConfig, String> {
    let v: AweNodeConfig = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    validate_awenode(&v)?;
    Ok(v)
}

pub fn load_awedc(path: &Path) -> Result<DataCentreConfig, String> {
    let v: DataCentreConfig = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    validate_awedc(&v)?;
    Ok(v)
}

pub fn load_dgc(path: &Path) -> Result<DataGroupConfig, String> {
    let v: DataGroupConfig = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    validate_dgc(&v)?;
    Ok(v)
}

pub fn validate_awenode(v: &AweNodeConfig) -> Result<(), String> {
    if v.format != "awenode" || v.version != FORMAT_VERSION { return Err("invalid .awenode format/version".into()); }
    if v.node_id.is_empty() || v.data_centre_id.is_empty() || v.endpoint.is_empty() || v.join_token.is_empty() { return Err("incomplete .awenode configuration".into()); }
    Ok(())
}

pub fn validate_awedc(v: &DataCentreConfig) -> Result<(), String> {
    if v.format != "awedc" || v.version != FORMAT_VERSION { return Err("invalid .awedc format/version".into()); }
    if v.data_centre_id.is_empty() || v.owner_node_id.is_empty() || v.endpoints.is_empty() || v.join_token.is_empty() { return Err("incomplete .awedc configuration".into()); }
    if v.member_node_ids.is_empty() { return Err("data centre must contain at least one node".into()); }
    Ok(())
}

pub fn validate_dgc(v: &DataGroupConfig) -> Result<(), String> {
    if v.format != "dgc" || v.version != FORMAT_VERSION { return Err("invalid .dgc format/version".into()); }
    if v.data_group_id.is_empty() || v.owner_data_centre_id.is_empty() || v.join_token.is_empty() { return Err("incomplete .dgc configuration".into()); }
    if v.data_centre_ids.len() < 2 { return Err("data group requires at least two data centres".into()); }
    Ok(())
}

pub fn merge_awedc(local: &mut DataCentreConfig, incoming: &DataCentreConfig) -> Result<(), String> {
    validate_awedc(incoming)?;
    if local.data_centre_id == incoming.data_centre_id { return Err("cannot merge a data centre with itself".into()); }
    for endpoint in &incoming.endpoints { if !local.endpoints.contains(endpoint) { local.endpoints.push(endpoint.clone()); } }
    if !local.peer_data_centres.contains(&incoming.data_centre_id) { local.peer_data_centres.push(incoming.data_centre_id.clone()); }
    Ok(())
}

pub fn merge_dgc(local: &mut DataGroupConfig, incoming: &DataGroupConfig) -> Result<(), String> {
    validate_dgc(incoming)?;
    if local.data_group_id == incoming.data_group_id { return Err("cannot merge a data group with itself".into()); }
    for dc in &incoming.data_centre_ids { if !local.data_centre_ids.contains(dc) { local.data_centre_ids.push(dc.clone()); } }
    for dg in &incoming.peer_data_groups { if !local.peer_data_groups.contains(dg) { local.peer_data_groups.push(dg.clone()); } }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats_validate() {
        let n = generate_awenode("node-1", "DC One", "127.0.0.1:41000", vec![], 1);
        validate_awenode(&n).unwrap();
        let dc = generate_awedc("node-1", &n.data_centre_id, "DC One", vec![n.endpoint.clone()], vec![n.node_id.clone()], 1);
        validate_awedc(&dc).unwrap();
        let dg = generate_dgc(&dc.data_centre_id, "Group", vec![dc.data_centre_id.clone(), "dc-2".into()], 1);
        validate_dgc(&dg).unwrap();
    }
}
