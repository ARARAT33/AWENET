use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fs, path::Path};

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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CentreGroupConfig {
    pub format: String,
    pub version: u16,
    pub centre_group_id: String,
    pub centre_group_name: String,
    pub issued_at_unix: u64,
    pub owner_data_group_id: String,
    pub data_group_ids: Vec<String>,
    pub peer_centre_groups: Vec<String>,
    pub join_token: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AweNetConfig {
    pub format: String,
    pub version: u16,
    pub local_node_id: String,
    pub local_data_centre_id: Option<String>,
    pub local_data_group_id: Option<String>,
    #[serde(default)]
    pub local_centre_group_id: Option<String>,
    pub joined_data_centres: Vec<String>,
    pub joined_data_groups: Vec<String>,
    #[serde(default)]
    pub joined_centre_groups: Vec<String>,
    pub bootstrap_endpoints: Vec<String>,
}

fn token(prefix: &str, _seed: &str) -> String {
    // Join credentials must be unpredictable. Hashing a node/group ID with a
    // timestamp is not a secret and can make invitations guessable.
    let mut secret = [0u8; 32];
    OsRng.fill_bytes(&mut secret);
    format!("{prefix}-{}", hex::encode(secret))
}

fn id(prefix: &str, seed: &str) -> String {
    let digest = blake3::hash(format!("AWE/{prefix}/v1/{seed}").as_bytes());
    format!("{prefix}-{}", &hex::encode(digest.as_bytes())[..24])
}

pub fn generate_awenode(
    node_id: &str,
    name: &str,
    endpoint: &str,
    bootstrap: Vec<String>,
    now: u64,
) -> AweNodeConfig {
    generate_awenode_for_dc(
        node_id,
        &id("dc", &format!("{node_id}:{name}")),
        name,
        endpoint,
        bootstrap,
        now,
    )
}

pub fn generate_awenode_for_dc(
    node_id: &str,
    data_centre_id: &str,
    name: &str,
    endpoint: &str,
    bootstrap: Vec<String>,
    now: u64,
) -> AweNodeConfig {
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

pub fn generate_awedc(
    owner_node_id: &str,
    dc_id: &str,
    name: &str,
    endpoints: Vec<String>,
    members: Vec<String>,
    now: u64,
) -> DataCentreConfig {
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

pub fn generate_dgc(
    owner_dc_id: &str,
    name: &str,
    data_centres: Vec<String>,
    now: u64,
) -> DataGroupConfig {
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

/// Create a Centre Group from at least two Data Groups. The owner must be a member.
pub fn generate_cgc(
    owner_dg_id: &str,
    name: &str,
    data_groups: Vec<String>,
    now: u64,
) -> CentreGroupConfig {
    let mut sorted = data_groups;
    sorted.sort();
    sorted.dedup();
    CentreGroupConfig {
        format: "cgc".into(),
        version: FORMAT_VERSION,
        centre_group_id: id("cg", &format!("{owner_dg_id}:{name}:{}", sorted.join(","))),
        centre_group_name: name.into(),
        issued_at_unix: now,
        owner_data_group_id: owner_dg_id.into(),
        data_group_ids: sorted,
        peer_centre_groups: Vec::new(),
        join_token: token("cg", owner_dg_id),
    }
}

pub fn save_json<T: Serialize>(value: &T, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let data = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    fs::write(path, data).map_err(|e| e.to_string())
}

pub fn load_awenode(path: &Path) -> Result<AweNodeConfig, String> {
    let v: AweNodeConfig = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    validate_awenode(&v)?;
    Ok(v)
}

pub fn load_awedc(path: &Path) -> Result<DataCentreConfig, String> {
    let v: DataCentreConfig = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    validate_awedc(&v)?;
    Ok(v)
}

pub fn load_dgc(path: &Path) -> Result<DataGroupConfig, String> {
    let v: DataGroupConfig = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    validate_dgc(&v)?;
    Ok(v)
}

pub fn load_cgc(path: &Path) -> Result<CentreGroupConfig, String> {
    let v: CentreGroupConfig = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    validate_cgc(&v)?;
    Ok(v)
}

pub fn validate_awenode(v: &AweNodeConfig) -> Result<(), String> {
    if v.format != "awenode" || v.version != FORMAT_VERSION {
        return Err("invalid .awenode format/version".into());
    }
    if v.node_id.trim().is_empty()
        || v.data_centre_id.trim().is_empty()
        || v.data_centre_name.trim().is_empty()
        || v.endpoint.trim().is_empty()
        || v.join_token.trim().is_empty()
        || v.bootstrap_endpoints
            .iter()
            .any(|endpoint| endpoint.trim().is_empty())
    {
        return Err("incomplete .awenode configuration".into());
    }
    Ok(())
}

pub fn validate_awedc(v: &DataCentreConfig) -> Result<(), String> {
    if v.format != "awedc" || v.version != FORMAT_VERSION {
        return Err("invalid .awedc format/version".into());
    }
    if v.data_centre_id.is_empty()
        || v.owner_node_id.is_empty()
        || v.endpoints.is_empty()
        || v.join_token.is_empty()
    {
        return Err("incomplete .awedc configuration".into());
    }
    if v.data_centre_name.trim().is_empty() || v.member_node_ids.is_empty() {
        return Err("data centre must have a name and at least one node".into());
    }
    if v.endpoints
        .iter()
        .any(|endpoint| endpoint.trim().is_empty())
    {
        return Err("data centre endpoints must not be empty".into());
    }
    let members: BTreeSet<_> = v.member_node_ids.iter().collect();
    if members.len() != v.member_node_ids.len() {
        return Err("data centre contains duplicate node IDs".into());
    }
    if !members.contains(&&v.owner_node_id) {
        return Err("data centre owner must be one of its member nodes".into());
    }
    if v.member_node_ids
        .iter()
        .any(|member| member.trim().is_empty())
    {
        return Err("data centre member IDs must not be empty".into());
    }
    let endpoints: BTreeSet<_> = v.endpoints.iter().collect();
    if endpoints.len() != v.endpoints.len() {
        return Err("data centre contains duplicate endpoints".into());
    }
    let peers: BTreeSet<_> = v.peer_data_centres.iter().collect();
    if peers.len() != v.peer_data_centres.len() || peers.contains(&&v.data_centre_id) {
        return Err("data centre contains duplicate or self-referencing peers".into());
    }
    Ok(())
}

pub fn validate_dgc(v: &DataGroupConfig) -> Result<(), String> {
    if v.format != "dgc" || v.version != FORMAT_VERSION {
        return Err("invalid .dgc format/version".into());
    }
    if v.data_group_id.is_empty() || v.owner_data_centre_id.is_empty() || v.join_token.is_empty() {
        return Err("incomplete .dgc configuration".into());
    }
    if v.data_group_name.trim().is_empty() || v.data_centre_ids.len() < 2 {
        return Err("data group requires a name and at least two data centres".into());
    }
    let centres: BTreeSet<_> = v.data_centre_ids.iter().collect();
    if centres.len() != v.data_centre_ids.len() {
        return Err("data group contains duplicate data-centre IDs".into());
    }
    if !centres.contains(&&v.owner_data_centre_id) {
        return Err("data group owner must be one of its member data centres".into());
    }
    if v.data_centre_ids.iter().any(|id| id.trim().is_empty()) {
        return Err("data group member IDs must not be empty".into());
    }
    let peers: BTreeSet<_> = v.peer_data_groups.iter().collect();
    if peers.len() != v.peer_data_groups.len() || peers.contains(&&v.data_group_id) {
        return Err("data group contains duplicate or self-referencing peers".into());
    }
    Ok(())
}

pub fn validate_cgc(v: &CentreGroupConfig) -> Result<(), String> {
    if v.format != "cgc" || v.version != FORMAT_VERSION {
        return Err("invalid .cgc format/version".into());
    }
    if v.centre_group_id.is_empty()
        || v.centre_group_name.trim().is_empty()
        || v.owner_data_group_id.is_empty()
        || v.join_token.is_empty()
    {
        return Err("incomplete .cgc configuration".into());
    }
    if v.data_group_ids.len() < 2 {
        return Err("centre group requires at least two data groups".into());
    }
    let groups: BTreeSet<_> = v.data_group_ids.iter().collect();
    if groups.len() != v.data_group_ids.len() {
        return Err("centre group contains duplicate data-group IDs".into());
    }
    if !groups.contains(&&v.owner_data_group_id) {
        return Err("centre group owner must be one of its member data groups".into());
    }
    if v.data_group_ids.iter().any(|id| id.trim().is_empty()) {
        return Err("centre group member IDs must not be empty".into());
    }
    let peers: BTreeSet<_> = v.peer_centre_groups.iter().collect();
    if peers.len() != v.peer_centre_groups.len() || peers.contains(&&v.centre_group_id) {
        return Err("centre group contains duplicate or self-referencing peers".into());
    }
    Ok(())
}

pub fn merge_awedc(
    local: &mut DataCentreConfig,
    incoming: &DataCentreConfig,
) -> Result<(), String> {
    validate_awedc(local)?;
    validate_awedc(incoming)?;
    if local.data_centre_id == incoming.data_centre_id {
        return Err("cannot merge a data centre with itself".into());
    }
    for endpoint in &incoming.endpoints {
        if !local.endpoints.contains(endpoint) {
            local.endpoints.push(endpoint.clone());
        }
    }
    if !local.peer_data_centres.contains(&incoming.data_centre_id) {
        local
            .peer_data_centres
            .push(incoming.data_centre_id.clone());
    }
    Ok(())
}

pub fn merge_dgc(local: &mut DataGroupConfig, incoming: &DataGroupConfig) -> Result<(), String> {
    validate_dgc(local)?;
    validate_dgc(incoming)?;
    if local.data_group_id == incoming.data_group_id {
        return Err("cannot merge a data group with itself".into());
    }
    for dc in &incoming.data_centre_ids {
        if !local.data_centre_ids.contains(dc) {
            local.data_centre_ids.push(dc.clone());
        }
    }
    for dg in &incoming.peer_data_groups {
        if !local.peer_data_groups.contains(dg) {
            local.peer_data_groups.push(dg.clone());
        }
    }
    Ok(())
}

pub fn merge_cgc(
    local: &mut CentreGroupConfig,
    incoming: &CentreGroupConfig,
) -> Result<(), String> {
    validate_cgc(local)?;
    validate_cgc(incoming)?;
    if local.centre_group_id == incoming.centre_group_id {
        return Err("cannot merge a centre group with itself".into());
    }
    if !local.peer_centre_groups.contains(&incoming.centre_group_id) {
        local
            .peer_centre_groups
            .push(incoming.centre_group_id.clone());
    }
    for peer in &incoming.peer_centre_groups {
        if peer != &local.centre_group_id && !local.peer_centre_groups.contains(peer) {
            local.peer_centre_groups.push(peer.clone());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn join_tokens_are_unpredictable_and_unique() {
        let a = generate_awenode("node-1", "DC One", "127.0.0.1:41000", vec![], 1);
        let b = generate_awenode("node-1", "DC One", "127.0.0.1:41000", vec![], 1);
        assert_ne!(a.join_token, b.join_token);
        assert!(a.join_token.starts_with("node-"));
        assert_eq!(a.join_token.len(), "node-".len() + 64);
    }

    #[test]
    fn rejects_data_centre_without_owner_membership() {
        let dc = generate_awedc(
            "node-owner",
            "dc-1",
            "DC One",
            vec!["127.0.0.1:41000".into()],
            vec!["node-other".into()],
            1,
        );
        assert!(validate_awedc(&dc).is_err());
    }

    #[test]
    fn rejects_duplicate_data_centre_members() {
        let mut dc = generate_awedc(
            "node-1",
            "dc-1",
            "DC One",
            vec!["127.0.0.1:41000".into()],
            vec!["node-1".into(), "node-1".into()],
            1,
        );
        assert!(validate_awedc(&dc).is_err());
        dc.member_node_ids = vec!["node-1".into()];
        assert!(validate_awedc(&dc).is_ok());
    }

    #[test]
    fn rejects_data_group_without_owner_membership() {
        let mut dg = generate_dgc("dc-owner", "Group", vec!["dc-a".into(), "dc-b".into()], 1);
        assert!(validate_dgc(&dg).is_err());
        dg.data_centre_ids.push("dc-owner".into());
        assert!(validate_dgc(&dg).is_ok());
    }

    #[test]
    fn centre_group_requires_owner_membership_and_unique_members() {
        let mut cg = generate_cgc(
            "dg-owner",
            "Core Group",
            vec!["dg-owner".into(), "dg-peer".into()],
            7,
        );
        assert!(validate_cgc(&cg).is_ok());
        cg.owner_data_group_id = "dg-missing".into();
        assert!(validate_cgc(&cg).is_err());
        cg.owner_data_group_id = "dg-owner".into();
        cg.data_group_ids.push("dg-owner".into());
        assert!(validate_cgc(&cg).is_err());
    }

    #[test]
    fn centre_group_merge_does_not_add_self_reference() {
        let mut a = generate_cgc("dg-a", "Group A", vec!["dg-a".into(), "dg-b".into()], 1);
        let b = generate_cgc("dg-b", "Group B", vec!["dg-b".into(), "dg-c".into()], 2);
        merge_cgc(&mut a, &b).unwrap();
        assert!(a.peer_centre_groups.contains(&b.centre_group_id));
        assert!(!a.peer_centre_groups.contains(&a.centre_group_id));
        assert!(validate_cgc(&a).is_ok());
    }

    #[test]
    fn legacy_awenet_state_defaults_centre_group_fields() {
        let legacy = serde_json::json!({
            "format": "awenet",
            "version": FORMAT_VERSION,
            "local_node_id": "node-legacy",
            "local_data_centre_id": null,
            "local_data_group_id": null,
            "joined_data_centres": [],
            "joined_data_groups": [],
            "bootstrap_endpoints": []
        });
        let state: AweNetConfig = serde_json::from_value(legacy).unwrap();
        assert_eq!(state.local_centre_group_id, None);
        assert!(state.joined_centre_groups.is_empty());
    }

    #[test]
    fn formats_validate() {
        let n = generate_awenode("node-1", "DC One", "127.0.0.1:41000", vec![], 1);
        validate_awenode(&n).unwrap();
        let dc = generate_awedc(
            "node-1",
            &n.data_centre_id,
            "DC One",
            vec![n.endpoint.clone()],
            vec![n.node_id.clone()],
            1,
        );
        validate_awedc(&dc).unwrap();
        let dg = generate_dgc(
            &dc.data_centre_id,
            "Group",
            vec![dc.data_centre_id.clone(), "dc-2".into()],
            1,
        );
        validate_dgc(&dg).unwrap();
    }
}
