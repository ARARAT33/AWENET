mod native_ui;
use anyhow::{Context, Result};
use awep2p_core::data_plane::{
    StorageShardAck, StorageShardRequest, StorageShardTransfer, STORAGE_STREAM,
};
use awep2p_core::diagnostics::{NodeDiagnostics, NodeMetrics};
use awep2p_core::federation::{
    self, AweNetConfig, AweNodeConfig, CentreGroupConfig, DataCentreConfig, DataGroupConfig,
};
use awep2p_core::identity::{AweId, AweSecret, Identity, LocalVault, Username};
use awep2p_core::lan_mesh::LanPeerBeacon;
use awep2p_core::messenger::format_uid;
use awep2p_core::network::{format_node_descriptor, Node};
use awep2p_core::onebank::{classify_tier, ResourceContribution};
use awep2p_core::onecoin::{OnecoinLedger, OnecoinTransaction, ATOMS_PER_COIN};
use awep2p_core::onecoin_consensus_runtime::{
    OnecoinConsensusMessage, OnecoinConsensusRuntime, ONECOIN_CONSENSUS_STREAM,
};
use awep2p_core::policy::{self, NetworkPolicy};
use awep2p_core::reputation::NodeReputation;
use awep2p_core::storage::{
    encode_shards, recover_shards, save_manifest, LocalNodeStore, StoragePolicy,
};
use awep2p_core::store::{AppCapability, Store};
use awep2p_core::supervisor::{PeerSupervisor, SupervisorConfig};
use rand::{rngs::OsRng, RngCore};
use std::{
    collections::BTreeMap,
    env, fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const UI_HTML: &str = include_str!("../../awe-desktop/ui/index.html");
const UI_CSS: &str = include_str!("../../awe-desktop/ui/style.css");
const UI_JS: &str = include_str!("../../awe-desktop/ui/app.js");
const ONECOIN_JS: &str = include_str!("../../awe-desktop/ui/onecoin.js");
const ONECOIN_CSS: &str = include_str!("../../awe-desktop/ui/onecoin.css");
const QR_JS: &str = include_str!("../../awe-desktop/ui/vendor/qrcode.js");
const UI_SESSION_BOOTSTRAP: &str = r#"<meta name="awenet-api-token" content="__AWENET_API_TOKEN__">
<script>
(() => {
    const meta = document.querySelector('meta[name="awenet-api-token"]');
    const token = meta ? meta.content : "";
    const originalFetch = window.fetch.bind(window);
    window.fetch = (input, init = {}) => {
        const rawUrl = typeof input === "string" ? input : input.url;
        const target = new URL(rawUrl, window.location.href);
        if (target.origin === window.location.origin && target.pathname.startsWith("/api/")) {
            const sourceHeaders = init.headers || (input instanceof Request ? input.headers : undefined);
            const headers = new Headers(sourceHeaders);
            headers.set("X-AWENET-Session", token);
            init = { ...init, headers };
        }
        return originalFetch(input, init);
    };
})();
</script>"#;
const DEFAULT_UI_ADDR: &str = "127.0.0.1:41800";

type MessengerLog = Arc<Mutex<Vec<serde_json::Value>>>;
const MAX_MESSENGER_LOG_ENTRIES: usize = 5_000;
const MAX_COMMUNITY_TITLE_BYTES: usize = 128;
const MAX_COMMUNITY_MESSAGE_BYTES: usize = 16 * 1024;
const MAX_GROUP_MEMBERS: usize = 512;
const MAX_COMMUNITY_MESSAGES_PER_FEED: usize = 5_000;

fn append_bounded_json_entry(entries: &mut Vec<serde_json::Value>, value: serde_json::Value) {
    if entries.len() >= MAX_COMMUNITY_MESSAGES_PER_FEED {
        let remove_count = (MAX_COMMUNITY_MESSAGES_PER_FEED / 10).max(1);
        entries.drain(..remove_count.min(entries.len()));
    }
    entries.push(value);
}

fn validate_community_title(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err("title is required".into());
    }
    if value.len() > MAX_COMMUNITY_TITLE_BYTES {
        return Err(format!(
            "title must not exceed {MAX_COMMUNITY_TITLE_BYTES} bytes"
        ));
    }
    Ok(())
}

fn validate_community_message(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err("message text is required".into());
    }
    if value.len() > MAX_COMMUNITY_MESSAGE_BYTES {
        return Err(format!(
            "message text must not exceed {MAX_COMMUNITY_MESSAGE_BYTES} bytes"
        ));
    }
    Ok(())
}

fn validate_group_member_ids(members: &[String]) -> Result<(), String> {
    if members.len() > MAX_GROUP_MEMBERS {
        return Err(format!("group cannot exceed {MAX_GROUP_MEMBERS} members"));
    }
    if members
        .iter()
        .any(|member| AweId::from_hex(member).is_err())
    {
        return Err("group member IDs must be 64-character hexadecimal AWEIDs".into());
    }
    Ok(())
}

fn validate_remote_group(
    group: &serde_json::Value,
    authenticated_sender: &str,
) -> Result<(), String> {
    if group.get("owner").and_then(|v| v.as_str()) != Some(authenticated_sender) {
        return Err("group owner does not match authenticated transport identity".into());
    }
    let id = group.get("id").and_then(|v| v.as_str()).unwrap_or("");
    if id.is_empty() || id.len() > 128 {
        return Err("invalid group ID".into());
    }
    validate_community_title(group.get("title").and_then(|v| v.as_str()).unwrap_or(""))?;
    let members = group
        .get("members")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "group members must be an array".to_string())?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "group member ID must be text".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    if members.is_empty() || !members.iter().any(|member| member == authenticated_sender) {
        return Err("group owner must be a group member".into());
    }
    validate_group_member_ids(&members)?;
    if group
        .get("messages")
        .and_then(|v| v.as_array())
        .map(|items| items.len() > MAX_COMMUNITY_MESSAGES_PER_FEED)
        .unwrap_or(true)
    {
        return Err("group message history is missing or exceeds the allowed limit".into());
    }
    Ok(())
}

fn validate_remote_feed_message(
    message: &serde_json::Value,
    authenticated_sender: &str,
) -> Result<(), String> {
    if message.get("sender").and_then(|v| v.as_str()) != Some(authenticated_sender) {
        return Err("message sender does not match authenticated transport identity".into());
    }
    let id = message.get("id").and_then(|v| v.as_str()).unwrap_or("");
    if id.is_empty() || id.len() > 128 {
        return Err("invalid community message ID".into());
    }
    validate_community_message(message.get("text").and_then(|v| v.as_str()).unwrap_or(""))?;
    if message.get("timestamp").and_then(|v| v.as_u64()).is_none() {
        return Err("message timestamp is required".into());
    }
    Ok(())
}

fn validate_remote_channel(
    channel: &serde_json::Value,
    authenticated_sender: &str,
) -> Result<(), String> {
    if channel.get("owner").and_then(|v| v.as_str()) != Some(authenticated_sender) {
        return Err("channel owner does not match authenticated transport identity".into());
    }
    let id = channel.get("id").and_then(|v| v.as_str()).unwrap_or("");
    if id.is_empty() || id.len() > 128 {
        return Err("invalid channel ID".into());
    }
    validate_community_title(channel.get("title").and_then(|v| v.as_str()).unwrap_or(""))?;
    let subscribers = channel
        .get("subscribers")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "channel subscribers must be an array".to_string())?;
    if subscribers.len() > 10_000
        || subscribers.iter().any(|value| {
            value
                .as_str()
                .and_then(|id| AweId::from_hex(id).ok())
                .is_none()
        })
    {
        return Err("invalid or excessive channel subscriber list".into());
    }
    if !subscribers
        .iter()
        .any(|value| value.as_str() == Some(authenticated_sender))
    {
        return Err("channel owner must be subscribed".into());
    }
    if channel
        .get("messages")
        .and_then(|v| v.as_array())
        .map(|items| items.len() > MAX_COMMUNITY_MESSAGES_PER_FEED)
        .unwrap_or(true)
    {
        return Err("channel message history is missing or exceeds the allowed limit".into());
    }
    Ok(())
}

fn append_messenger_entry(log: &mut Vec<serde_json::Value>, item: serde_json::Value) {
    if log.len() >= MAX_MESSENGER_LOG_ENTRIES {
        let remove_count = (MAX_MESSENGER_LOG_ENTRIES / 10).max(1);
        log.drain(..remove_count.min(log.len()));
    }
    log.push(item);
}

fn storage_upload_is_complete(
    stored_local: usize,
    sent_remote: usize,
    required_placements: usize,
    failed_transfers: usize,
) -> bool {
    failed_transfers == 0 && stored_local.checked_add(sent_remote) == Some(required_placements)
}

fn storage_ack_matches(
    ack_sender: &[u8; 32],
    expected_sender: &[u8; 32],
    ack: &StorageShardAck,
    file_id: &[u8; 32],
    shard_index: u16,
    expected_hash: &[u8; 32],
) -> bool {
    ack_sender == expected_sender
        && ack.file_id == *file_id
        && ack.shard_index == shard_index
        && ack.stored_object_id == *expected_hash
}

fn validate_storage_file_id(value: &str) -> Result<[u8; 32], String> {
    let bytes =
        hex::decode(value).map_err(|_| "file_id must be 64 hexadecimal characters".to_string())?;
    let id: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "file_id must be 64 hexadecimal characters".to_string())?;
    if value.len() != 64 {
        return Err("file_id must be 64 hexadecimal characters".into());
    }
    Ok(id)
}
type FederationState = Arc<Mutex<AweNetConfig>>;
type StorageState = Arc<LocalNodeStore>;
type PendingAcks = Arc<Mutex<BTreeMap<[u8; 16], ([u8; 32], StorageShardAck)>>>;
type PolicyState = Arc<Mutex<NetworkPolicy>>;
type CommunityState = Arc<Mutex<serde_json::Value>>;
type ConsensusState = Arc<Mutex<Option<OnecoinConsensusRuntime>>>;
type OnecoinLedgerState = Arc<Mutex<OnecoinLedger>>;
type ContributionState = Arc<Mutex<ResourceContribution>>;
type ConsensusSendResult = (
    String,
    u128,
    Vec<([u8; 32], OnecoinConsensusMessage)>,
    u64,
    bool,
);

#[derive(Clone)]
struct UiState {
    node: Node,
    api_token: String,
    messenger: MessengerLog,
    federation_state: FederationState,
    federation_path: PathBuf,
    storage: StorageState,
    policy_state: PolicyState,
    community: CommunityState,
    consensus_state: ConsensusState,
    onecoin_ledger: OnecoinLedgerState,
    onecoin_path: PathBuf,
    onecoin_offers_path: PathBuf,
    contribution: ContributionState,
    contribution_path: PathBuf,
}

async fn build_onecoin_validators(
    node: &Node,
    configured: &[String],
) -> BTreeMap<String, [u8; 32]> {
    let wanted = configured
        .iter()
        .map(|v| v.to_lowercase())
        .collect::<std::collections::BTreeSet<_>>();
    let mut validators = BTreeMap::new();
    let local_id = node.identity.public.awe_id.to_hex().to_lowercase();
    if wanted.contains(&local_id) {
        validators.insert(local_id, node.identity.public.public_key);
    }
    for peer in node.peers().await {
        let id = hex::encode(peer.awe_id).to_lowercase();
        if wanted.contains(&id) {
            validators.insert(id, peer.public_key);
        }
    }
    validators
}

fn default_vault() -> PathBuf {
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home).join(".awep2p").join("identity.vault");
    }
    if let Some(profile) = env::var_os("USERPROFILE") {
        return PathBuf::from(profile)
            .join(".awep2p")
            .join("identity.vault");
    }
    PathBuf::from("identity.vault")
}

fn usage() -> ! {
    eprintln!("AWENET\n\nUsage:\n  awe-node                 Start the complete local product\n  awe-node app             Start UI + local node\n  awe-node secret <username> [out-file]\n  awe-node init <username> [vault-file]\n  awe-node run <vault-file> <password> <listen-addr> [bootstrap-addr ...]\n  awe-node id <vault-file> <password> <username>\n  awe-node status [vault-file]\n  awe-node diagnostics\n  awe-node mesh <listen-port>\n  awe-node health\n  awe-node probe <address>");
    std::process::exit(2)
}

fn generate_secret_file(username_str: &str, out_path: Option<PathBuf>) -> Result<()> {
    let username = Username::new(username_str).map_err(anyhow::Error::msg)?;
    let identity = Identity::generate(username);
    let secret = AweSecret::generate(&identity);
    let bytes = secret
        .to_bytes()
        .context("failed to serialize .awesecret")?;
    let path = out_path.unwrap_or_else(|| {
        if let Some(home) = env::var_os("HOME") {
            PathBuf::from(home)
                .join(".awep2p")
                .join(format!("{username_str}.awesecret"))
        } else {
            PathBuf::from(format!("{username_str}.awesecret"))
        }
    });
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, bytes)?;
    println!("AWE-ID: {}", secret.awe_id);
    println!(
        "Node Descriptor: {}",
        format_node_descriptor(identity.public.awe_id.as_bytes())
    );
    println!("Saved: {}", path.display());
    Ok(())
}

fn init(username: &str, path: PathBuf) -> Result<()> {
    let identity =
        Identity::generate(Username::new(username.to_owned()).map_err(anyhow::Error::msg)?);
    let password = rpassword::prompt_password("Vault password: ")?;
    if password.is_empty() {
        anyhow::bail!("vault password must not be empty");
    }
    let vault = LocalVault::seal(&identity, &password).map_err(anyhow::Error::msg)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, vault)?;
    println!("AWE-ID: {}", identity.public.awe_id.to_hex());
    println!("Identity vault: {}", path.display());
    Ok(())
}

fn load_identity(path: &PathBuf, password: &str, username: &str) -> Result<Identity> {
    let data = fs::read(path)?;
    LocalVault::open(
        &data,
        Username::new(username.to_owned()).map_err(anyhow::Error::msg)?,
        password,
    )
    .map_err(anyhow::Error::msg)
}

fn is_allowed_ui_origin(origin: &str) -> bool {
    matches!(
        origin,
        "tauri://localhost"
            | "http://tauri.localhost"
            | "https://tauri.localhost"
            | "http://127.0.0.1:41800"
            | "http://localhost:41800"
    )
}

fn is_valid_api_session(expected: &str, supplied: Option<&str>) -> bool {
    supplied == Some(expected)
}

fn sign_federation_invitation(
    kind: &str,
    payload: serde_json::Value,
    identity: &Identity,
) -> Result<String, String> {
    if !matches!(kind, "awenode" | "awedc" | "dgc" | "cgc") {
        return Err("unsupported federation invitation kind".into());
    }
    let owner_node_id = format_uid(identity.public.awe_id.as_bytes());
    if kind == "awenode"
        && payload.get("node_id").and_then(|value| value.as_str()) != Some(owner_node_id.as_str())
    {
        return Err("node invitation identity does not match its signer".into());
    }
    if kind == "awedc"
        && payload
            .get("owner_node_id")
            .and_then(|value| value.as_str())
            != Some(owner_node_id.as_str())
    {
        return Err("data-centre owner does not match its signer".into());
    }
    let unsigned = serde_json::json!({
        "format": "awenet-signed-federation-invitation",
        "version": 1,
        "kind": kind,
        "owner_node_id": owner_node_id,
        "owner_public_key": hex::encode(identity.public.public_key),
        "payload": payload
    });
    let message = serde_json::to_vec(&unsigned).map_err(|error| error.to_string())?;
    let signature = hex::encode(identity.sign(&message));
    let mut envelope = unsigned;
    envelope["signature"] = serde_json::Value::String(signature);
    serde_json::to_string_pretty(&envelope).map_err(|error| error.to_string())
}

fn verify_signed_federation_invitation(
    content: &str,
    expected_kind: &str,
) -> Result<(serde_json::Value, String), String> {
    let envelope: serde_json::Value = serde_json::from_str(content)
        .map_err(|error| format!("invalid invitation JSON: {error}"))?;
    if envelope.get("format").and_then(|value| value.as_str())
        != Some("awenet-signed-federation-invitation")
        || envelope.get("version").and_then(|value| value.as_u64()) != Some(1)
    {
        return Err(
            "unsigned or unsupported federation invitation; regenerate it with this AWENET version"
                .into(),
        );
    }
    if envelope.get("kind").and_then(|value| value.as_str()) != Some(expected_kind) {
        return Err("invitation kind does not match the requested import kind".into());
    }
    let owner_node_id = envelope
        .get("owner_node_id")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "invitation owner identity is missing".to_string())?
        .to_string();
    let public_key_bytes = hex::decode(
        envelope
            .get("owner_public_key")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "invitation owner public key is missing".to_string())?,
    )
    .map_err(|error| format!("invalid owner public key: {error}"))?;
    let public_key: [u8; 32] = public_key_bytes
        .try_into()
        .map_err(|_| "owner public key must be 32 bytes".to_string())?;
    if format_uid(AweId::from_public_key(&public_key).as_bytes()) != owner_node_id {
        return Err("owner identity does not match the public key".into());
    }
    let signature_bytes = hex::decode(
        envelope
            .get("signature")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "invitation signature is missing".to_string())?,
    )
    .map_err(|error| format!("invalid invitation signature: {error}"))?;
    let signature: [u8; 64] = signature_bytes
        .try_into()
        .map_err(|_| "invitation signature must be 64 bytes".to_string())?;
    let mut unsigned = envelope.clone();
    unsigned
        .as_object_mut()
        .ok_or_else(|| "invitation must be a JSON object".to_string())?
        .remove("signature");
    let message = serde_json::to_vec(&unsigned).map_err(|error| error.to_string())?;
    if !Identity::verify(&public_key, &message, &signature) {
        return Err("federation invitation signature verification failed".into());
    }
    let payload = envelope
        .get("payload")
        .cloned()
        .ok_or_else(|| "invitation payload is missing".to_string())?;
    if expected_kind == "awenode"
        && payload.get("node_id").and_then(|value| value.as_str()) != Some(owner_node_id.as_str())
    {
        return Err("node invitation identity does not match its signer".into());
    }
    if expected_kind == "awedc"
        && payload
            .get("owner_node_id")
            .and_then(|value| value.as_str())
            != Some(owner_node_id.as_str())
    {
        return Err("data-centre owner does not match its signer".into());
    }
    Ok((payload, owner_node_id))
}

fn record_bootstrap_endpoint(
    state: &Arc<Mutex<AweNetConfig>>,
    path: &Path,
    endpoint: &str,
    local_node_id: &str,
) -> Result<(), String> {
    let mut current = state
        .lock()
        .map_err(|_| "federation state lock poisoned".to_string())?;
    let mut next = current.clone();
    next.local_node_id = local_node_id.to_string();
    next.format = "awenet".into();
    next.version = federation::FORMAT_VERSION;
    if !next
        .bootstrap_endpoints
        .iter()
        .any(|known| known == endpoint)
    {
        next.bootstrap_endpoints.push(endpoint.to_string());
    }
    federation::save_json(&next, path)?;
    *current = next;
    Ok(())
}

fn record_generated_federation_owner(
    kind: &str,
    payload: &serde_json::Value,
    owner_node_id: &str,
    state: &Arc<Mutex<AweNetConfig>>,
    path: &Path,
) -> Result<(), String> {
    if !matches!(kind, "awedc" | "dgc" | "cgc") {
        return Ok(());
    }
    let mut current = state
        .lock()
        .map_err(|_| "federation state lock poisoned".to_string())?;
    let mut next = current.clone();
    next.local_node_id = owner_node_id.to_string();
    next.format = "awenet".into();
    next.version = federation::FORMAT_VERSION;
    match kind {
        "awedc" => {
            let id = payload
                .get("data_centre_id")
                .and_then(|value| value.as_str())
                .ok_or_else(|| "generated data-centre ID is missing".to_string())?;
            if next
                .data_centre_owners
                .get(id)
                .is_some_and(|owner| owner != owner_node_id)
            {
                return Err("data-centre ID is already bound to a different owner".into());
            }
            next.data_centre_owners
                .insert(id.to_string(), owner_node_id.to_string());
            if !next.joined_data_centres.iter().any(|joined| joined == id) {
                next.joined_data_centres.push(id.to_string());
            }
            if next.local_data_centre_id.is_none() {
                next.local_data_centre_id = Some(id.to_string());
            }
        }
        "dgc" => {
            let id = payload
                .get("data_group_id")
                .and_then(|value| value.as_str())
                .ok_or_else(|| "generated data-group ID is missing".to_string())?;
            if next
                .data_group_owners
                .get(id)
                .is_some_and(|owner| owner != owner_node_id)
            {
                return Err("data-group ID is already bound to a different owner".into());
            }
            next.data_group_owners
                .insert(id.to_string(), owner_node_id.to_string());
            if !next.joined_data_groups.iter().any(|joined| joined == id) {
                next.joined_data_groups.push(id.to_string());
            }
            if next.local_data_group_id.is_none() {
                next.local_data_group_id = Some(id.to_string());
            }
        }
        "cgc" => {
            let id = payload
                .get("centre_group_id")
                .and_then(|value| value.as_str())
                .ok_or_else(|| "generated centre-group ID is missing".to_string())?;
            if !next.joined_centre_groups.iter().any(|joined| joined == id) {
                next.joined_centre_groups.push(id.to_string());
            }
            if next.local_centre_group_id.is_none() {
                next.local_centre_group_id = Some(id.to_string());
            }
        }
        _ => {}
    }
    federation::save_json(&next, path)?;
    *current = next;
    Ok(())
}

fn leave_federation_membership(
    state: &mut AweNetConfig,
    kind: &str,
    id: &str,
    local_node_id: &str,
) -> Result<(), String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("membership ID must not be empty".into());
    }
    if !state.local_node_id.is_empty() && state.local_node_id != local_node_id {
        return Err("federation state belongs to a different local node".into());
    }

    let kind = kind.trim().to_ascii_lowercase();
    let (joined, local_id) = match kind.as_str() {
        "dc" | "awedc" => (
            &mut state.joined_data_centres,
            &mut state.local_data_centre_id,
        ),
        "dg" | "dgc" => (
            &mut state.joined_data_groups,
            &mut state.local_data_group_id,
        ),
        "cg" | "cgc" => (
            &mut state.joined_centre_groups,
            &mut state.local_centre_group_id,
        ),
        _ => return Err("kind must be dc, dg, or cg".into()),
    };
    let is_member = joined.iter().any(|member| member == id) || local_id.as_deref() == Some(id);
    if !is_member {
        return Err("membership is not joined by this node".into());
    }
    joined.retain(|member| member != id);
    if local_id.as_deref() == Some(id) {
        *local_id = None;
    }
    match kind.as_str() {
        "dc" | "awedc" => {
            state.data_centre_owners.remove(id);
            state.data_group_owners.clear();
            state.joined_data_groups.clear();
            state.local_data_group_id = None;
            state.joined_centre_groups.clear();
            state.local_centre_group_id = None;
        }
        "dg" | "dgc" => {
            state.data_group_owners.remove(id);
            state.joined_centre_groups.clear();
            state.local_centre_group_id = None;
        }
        _ => {}
    }
    state.local_node_id = local_node_id.to_string();
    Ok(())
}

fn ui_html_with_session_token(token: &str) -> String {
    let bootstrap = UI_SESSION_BOOTSTRAP.replace("__AWENET_API_TOKEN__", token);
    UI_HTML.replace("</head>", &format!("{bootstrap}\n</head>"))
}

async fn http_response(
    status: &str,
    content_type: &str,
    body: &str,
    origin: Option<&str>,
) -> Vec<u8> {
    let cors = origin
        .filter(|value| is_allowed_ui_origin(value))
        .map(|value| {
            format!(
                "Access-Control-Allow-Origin: {value}\r\nVary: Origin\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: content-type, x-awenet-session\r\n"
            )
        })
        .unwrap_or_default();
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n{cors}Cache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn parse_content_length(headers: &str) -> Result<usize, &'static str> {
    let mut content_length = None;
    for line in headers.lines().skip(1) {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let (name, value) = line.split_once(':').ok_or("malformed HTTP header")?;
        if name.trim().is_empty() {
            return Err("malformed HTTP header name");
        }
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err("Transfer-Encoding is unsupported");
        }
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err("duplicate Content-Length header");
            }
            let value = value.trim();
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err("invalid Content-Length header");
            }
            content_length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| "invalid Content-Length header")?,
            );
        }
    }
    Ok(content_length.unwrap_or(0))
}

async fn read_http_request(stream: &mut tokio::net::TcpStream) -> Result<String> {
    const HEADER_LIMIT: usize = 64 * 1024;
    const BODY_LIMIT: usize = 64 * 1024 * 1024;
    let mut data = Vec::with_capacity(8192);
    let header_end;
    loop {
        let mut chunk = [0u8; 4096];
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            anyhow::bail!("client closed connection before HTTP headers");
        }
        data.extend_from_slice(&chunk[..n]);
        if let Some(pos) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            if pos + 4 > HEADER_LIMIT {
                anyhow::bail!("HTTP headers too large");
            }
            header_end = pos + 4;
            break;
        }
        if data.len() > HEADER_LIMIT {
            anyhow::bail!("HTTP headers too large");
        }
    }

    let headers = String::from_utf8_lossy(&data[..header_end]);
    let content_length = parse_content_length(&headers).map_err(anyhow::Error::msg)?;
    if content_length > BODY_LIMIT {
        anyhow::bail!("HTTP request body too large");
    }
    let target = header_end + content_length;
    if data.len() < target {
        let mut filled = data.len();
        data.resize(target, 0);
        while filled < target {
            let n = stream.read(&mut data[filled..target]).await?;
            if n == 0 {
                anyhow::bail!("client closed connection before request body was complete");
            }
            filled += n;
        }
    } else if data.len() > target {
        data.truncate(target);
    }
    String::from_utf8(data).context("HTTP request must be UTF-8")
}

fn json_u64_field(value: &serde_json::Value, field: &str) -> Result<u64, String> {
    match value.get(field) {
        None => Ok(0),
        Some(raw) => raw
            .as_u64()
            .ok_or_else(|| format!("{field} must be a non-negative integer")),
    }
}

fn json_u32_field(value: &serde_json::Value, field: &str) -> Result<u32, String> {
    u32::try_from(json_u64_field(value, field)?)
        .map_err(|_| format!("{field} exceeds the supported range"))
}

fn json_u16_field(value: &serde_json::Value, field: &str) -> Result<u16, String> {
    u16::try_from(json_u64_field(value, field)?)
        .map_err(|_| format!("{field} exceeds the supported range"))
}

fn onecoin_consensus_configured(config: &str) -> bool {
    config
        .split(',')
        .any(|validator| !validator.trim().is_empty())
}

fn parse_onecoin_atoms(value: &serde_json::Value) -> Result<u128, String> {
    let raw = if let Some(text) = value.as_str() {
        text.trim().to_owned()
    } else if value.is_number() {
        value.to_string()
    } else {
        return Err("amount_coins must be a number or decimal string".into());
    };
    if raw.is_empty() || raw.contains('e') || raw.contains('E') {
        return Err("amount_coins must be a plain decimal value".into());
    }
    let mut parts = raw.split('.');
    let whole = parts.next().unwrap_or("");
    let fraction = parts.next().unwrap_or("");
    if parts.next().is_some() || whole.is_empty() || !whole.chars().all(|c| c.is_ascii_digit()) {
        return Err("amount_coins has invalid decimal syntax".into());
    }
    if fraction.len() > 18 || !fraction.chars().all(|c| c.is_ascii_digit()) {
        return Err("amount_coins supports at most 18 decimal places".into());
    }
    let whole_atoms = whole
        .parse::<u128>()
        .map_err(|_| "amount_coins is too large".to_string())?
        .checked_mul(ATOMS_PER_COIN)
        .ok_or_else(|| "amount_coins is too large".to_string())?;
    let padded = format!("{fraction:0<18}");
    let fraction_atoms = if padded.is_empty() {
        0
    } else {
        padded
            .parse::<u128>()
            .map_err(|_| "amount_coins has invalid precision".to_string())?
    };
    whole_atoms
        .checked_add(fraction_atoms)
        .ok_or_else(|| "amount_coins is too large".to_string())
}

async fn serve_ui(mut stream: tokio::net::TcpStream, state: UiState) -> Result<()> {
    let UiState {
        node,
        api_token,
        messenger,
        federation_state,
        federation_path,
        storage,
        policy_state,
        community,
        consensus_state,
        onecoin_ledger,
        onecoin_path,
        onecoin_offers_path,
        contribution,
        contribution_path,
    } = state;
    // The UI listener is local, but a stalled client must not hold a task and socket forever.
    // Keep enough time for large local uploads while bounding slow-header/body resource use.
    let request_result = tokio::time::timeout(
        Duration::from_secs(30),
        read_http_request(&mut stream),
    )
    .await;
    let request = match request_result {
        Ok(Ok(request)) => request,
        Ok(Err(error)) => {
            let response = http_response(
                "400 Bad Request",
                "application/json; charset=utf-8",
                &serde_json::json!({"status":"error","error":error.to_string()}).to_string(),
                None,
            )
            .await;
            stream.write_all(&response).await?;
            stream.shutdown().await?;
            return Ok(());
        }
        Err(_) => {
            let response = http_response(
                "408 Request Timeout",
                "application/json; charset=utf-8",
                r#"{"status":"error","error":"request timed out while reading headers or body"}"#,
                None,
            )
            .await;
            stream.write_all(&response).await?;
            stream.shutdown().await?;
            return Ok(());
        }
    };
    let request_line = request.lines().next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let path = parts.next().unwrap_or("/").split('?').next().unwrap_or("/");
    let origin = request.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("origin").then(|| value.trim())
    });
    if let Some(origin) = origin {
        if !is_allowed_ui_origin(origin) {
            stream
                .write_all(
                    &http_response(
                        "403 Forbidden",
                        "application/json; charset=utf-8",
                        r#"{"status":"error","error":"origin not allowed"}"#,
                        None,
                    )
                    .await,
                )
                .await?;
            stream.shutdown().await?;
            return Ok(());
        }
    }
    if method.eq_ignore_ascii_case("OPTIONS") {
        stream
            .write_all(
                &http_response("204 No Content", "text/plain; charset=utf-8", "", origin).await,
            )
            .await?;
        stream.shutdown().await?;
        return Ok(());
    }
    if method.eq_ignore_ascii_case("POST") && path.starts_with("/api/") {
        let supplied_token = request.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("x-awenet-session")
                .then(|| value.trim())
        });
        if !is_valid_api_session(&api_token, supplied_token) {
            stream
                .write_all(
                    &http_response(
                        "401 Unauthorized",
                        "application/json; charset=utf-8",
                        r#"{"status":"error","error":"valid AWENET session token required"}"#,
                        origin,
                    )
                    .await,
                )
                .await?;
            stream.shutdown().await?;
            return Ok(());
        }
    }
    // Reject malformed/non-object JSON before route handlers can silently replace it
    // with an empty object via unwrap_or_default(). Form-encoded bootstrap requests are
    // intentionally excluded.
    if method.eq_ignore_ascii_case("POST")
        && path.starts_with("/api/")
        && request
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-type")
                    .then(|| value.trim())
            })
            .is_some_and(|value| value.to_ascii_lowercase().starts_with("application/json"))
    {
        let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
        let valid_object =
            serde_json::from_str::<serde_json::Value>(body).is_ok_and(|value| value.is_object());
        if !valid_object {
            let response = http_response(
                "400 Bad Request",
                "application/json; charset=utf-8",
                r#"{"status":"error","error":"JSON request body must be a valid object"}"#,
                origin,
            )
            .await;
            stream.write_all(&response).await?;
            stream.shutdown().await?;
            return Ok(());
        }
    }
    let (status, mime, body) = match path {
        "/" | "/index.html" => ("200 OK", "text/html; charset=utf-8", ui_html_with_session_token(&api_token)),
        "/style.css" => ("200 OK", "text/css; charset=utf-8", UI_CSS.to_string()),
        "/app.js" => ("200 OK", "application/javascript; charset=utf-8", UI_JS.to_string()),
        "/onecoin.js" => ("200 OK", "application/javascript; charset=utf-8", ONECOIN_JS.to_string()),
        "/onecoin.css" => ("200 OK", "text/css; charset=utf-8", ONECOIN_CSS.to_string()),
        "/vendor/qrcode.js" => ("200 OK", "application/javascript; charset=utf-8", QR_JS.to_string()),
        "/api/onebank/wallet" if method == "GET" => {
            let consensus_snapshot = consensus_state.lock().map(|guard| {
                guard.as_ref().map(|runtime| (
                    runtime.persisted_state().ledger.clone(),
                    runtime.persisted_state().height,
                ))
            });
            let (ledger, finality_mode, consensus_height) = match consensus_snapshot {
                Ok(Some((ledger, height))) => (ledger, "CONSENSUS_FINALIZED", Some(height)),
                Ok(None) => (
                    onecoin_ledger.lock().map(|l| l.clone()).unwrap_or_default(),
                    "LOCAL_LEDGER",
                    None,
                ),
                Err(_) => (OnecoinLedger::default(), "CONSENSUS_UNAVAILABLE", None),
            };
            let id = node.identity.public.awe_id.clone();
            let balance_atoms = ledger.balance_atoms(&id);
            let contribution_snapshot = contribution.lock().map(|r| r.clone()).unwrap_or_default();
            let tier = classify_tier(&contribution_snapshot).wire_name();
            ("200 OK", "application/json; charset=utf-8", serde_json::json!({
                "awe_id": id.to_hex(),
                "balance_atoms": balance_atoms,
                "balance_coins": balance_atoms / ATOMS_PER_COIN,
                "tier": tier,
                "fee_bps": 100,
                "finality_mode": finality_mode,
                "consensus_height": consensus_height,
                "resource_score": contribution_snapshot.score(),
                "resource": contribution_snapshot,
                "resource_verified": false,
                "exchange": "P2P_ONECOIN_ONLY",
                "fiat_settlement": "EXTERNAL_PERSON_TO_PERSON"
            }).to_string())
        },
        "/api/onebank/contribution" if method == "GET" => {
            let r = contribution.lock().map(|v| v.clone()).unwrap_or_default();
            ("200 OK", "application/json; charset=utf-8", serde_json::json!({
                "resource": r,
                "tier": classify_tier(&r).wire_name(),
                "verified": false,
                "reward_status": "requires_signed_usage_receipt"
            }).to_string())
        },
        "/api/onebank/contribution" if method == "POST" => {
            let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
            let parsed: Result<serde_json::Value, String> = serde_json::from_str(body)
                .map_err(|_| "contribution body must be valid JSON".to_string());
            let contribution_result: Result<ResourceContribution, String> = parsed.and_then(|parsed| {
                let resource = ResourceContribution {
                    storage_bytes: json_u64_field(&parsed, "storage_bytes")?,
                    cpu_cores: json_u32_field(&parsed, "cpu_cores")?,
                    ram_bytes: json_u64_field(&parsed, "ram_bytes")?,
                    gpu_units: json_u32_field(&parsed, "gpu_units")?,
                    bandwidth_bytes: json_u64_field(&parsed, "bandwidth_bytes")?,
                    online_hours: json_u16_field(&parsed, "online_hours")?,
                    node_count: json_u32_field(&parsed, "node_count")?,
                    server_count: json_u32_field(&parsed, "server_count")?,
                    uptime_bps: json_u16_field(&parsed, "uptime_bps")?,
                    utilization_bps: json_u16_field(&parsed, "utilization_bps")?,
                };
                resource.validate()?;
                Ok(resource)
            });
            match contribution_result {
                Ok(r) => {
                    if let Ok(mut guard) = contribution.lock() { *guard = r.clone(); }
                    match fs::write(&contribution_path, serde_json::to_vec_pretty(&r).unwrap_or_default()) {
                        Ok(()) => ("200 OK", "application/json; charset=utf-8", serde_json::json!({
                            "status":"accepted_for_verification",
                            "tier": classify_tier(&r).wire_name(),
                            "verified":false,
                            "reward_status":"requires_signed_usage_receipt"
                        }).to_string()),
                        Err(e) => ("500 Internal Server Error", "application/json; charset=utf-8", serde_json::json!({"status":"error","error":e.to_string()}).to_string())
                    }
                },
                Err(error) => ("400 Bad Request", "application/json; charset=utf-8", serde_json::json!({"status":"error","error":error}).to_string())
            }
        },
        "/api/onebank/wallet/send" if method == "POST" => {
            let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
            let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
            let recipient_hex = parsed.get("recipient").and_then(|v| v.as_str()).unwrap_or("");
            let amount_value = parsed.get("amount_coins").cloned().unwrap_or(serde_json::Value::Null);
            let memo = parsed.get("memo").and_then(|v| v.as_str()).map(str::to_owned);
            // Configured consensus must never silently fall back to a separate local ledger while
            // validator discovery/recovery is incomplete. That would create conflicting balances.
            let consensus_configured = onecoin_consensus_configured(
                &env::var("AWE_ONECOIN_VALIDATORS").unwrap_or_default(),
            );
            if consensus_state.lock().map(|guard| guard.is_some()).unwrap_or(true)
                || consensus_configured
            {
                let consensus_result: Result<ConsensusSendResult, String> = (|| {
                    let recipient = AweId::from_hex(recipient_hex)?;
                    if recipient == node.identity.public.awe_id {
                        return Err("cannot transfer ONECOIN to the same wallet".into());
                    }
                    let amount_atoms = parse_onecoin_atoms(&amount_value)?;
                    if amount_atoms == 0 {
                        return Err("amount_coins must be positive".into());
                    }
                    let mut guard = consensus_state.lock().map_err(|_| "ONECOIN consensus lock failed".to_string())?;
                    let runtime = guard.as_mut().ok_or("ONECOIN consensus is unavailable")?;
                    let ledger = &runtime.persisted_state().ledger;
                    let recipient_key = recipient.to_hex();
                    if !ledger.members.contains_key(&recipient_key) {
                        return Err("recipient is not yet a member of finalized ONECOIN state".into());
                    }
                    let sender = node.identity.public.awe_id.clone();
                    if !ledger.members.contains_key(&sender.to_hex()) {
                        return Err("sender is not a member of finalized ONECOIN state".into());
                    }
                    let old_height = runtime.persisted_state().height;
                    let nonce = ledger.nonces.get(&sender.to_hex()).copied().unwrap_or(0);
                    let tx = OnecoinTransaction::new(&node.identity, nonce, &recipient, amount_atoms, memo);
                    let tx_id = hex::encode(tx.id());
                    let fee_atoms = amount_atoms.saturating_mul(100).saturating_div(10_000);
                    let outbound = if runtime.local_is_validator(&node.identity) {
                        runtime.propose(&node.identity, vec![tx], now_unix())?
                    } else {
                        // Wallets may submit signed transactions to configured validators without
                        // being validators themselves. The validator rechecks membership, nonce,
                        // signature, balance, and fee against its own finalized state.
                        let mut submissions = Vec::new();
                        for (validator_id, public_key) in &runtime.validators {
                            if public_key == &node.identity.public.public_key {
                                continue;
                            }
                            let decoded = hex::decode(validator_id)
                                .map_err(|_| "invalid configured validator AWEID".to_string())?;
                            let peer_id: [u8; 32] = decoded
                                .try_into()
                                .map_err(|_| "invalid configured validator AWEID length".to_string())?;
                            submissions.push((
                                peer_id,
                                OnecoinConsensusMessage::TransactionSubmission(tx.clone()),
                            ));
                        }
                        if submissions.is_empty() {
                            return Err("no remote ONECOIN validator is currently available".into());
                        }
                        submissions
                    };
                    let height = runtime.persisted_state().height;
                    Ok((tx_id, fee_atoms, outbound, height, height > old_height))
                })();
                match consensus_result {
                    Ok((tx_id, fee_atoms, outbound, height, finalized)) => {
                        let mut delivered = 0usize;
                        for (peer, message) in outbound {
                            if peer == *node.identity.public.awe_id.as_bytes() { continue; }
                            if let Ok(bytes) = message.encode() {
                                if node.send_to_peer(&peer, ONECOIN_CONSENSUS_STREAM, bytes).await.is_ok() {
                                    delivered += 1;
                                }
                            }
                        }
                        let status = if finalized { "finalized" } else { "submitted_to_consensus" };
                        (if finalized { "200 OK" } else { "202 Accepted" }, "application/json; charset=utf-8", serde_json::json!({
                            "status": status,
                            "tx_id": tx_id,
                            "fee_atoms": fee_atoms,
                            "fee_bps": 100,
                            "finality_mode": "CONSENSUS",
                            "consensus_height": height,
                            "outbound_messages_delivered": delivered,
                            "note": if finalized { "Transaction is in finalized consensus state." } else { "Transaction sent to consensus peers; wallet balance changes only after quorum finality." }
                        }).to_string())
                    },
                    Err(error) => (
                        if error == "ONECOIN consensus is unavailable" {
                            "503 Service Unavailable"
                        } else {
                            "409 Conflict"
                        },
                        "application/json; charset=utf-8",
                        serde_json::json!({"status":"rejected","finality_mode":"CONSENSUS","error":error}).to_string()
                    )
                }
            } else {
            let result: Result<(String, u128, OnecoinTransaction), String> = (|| {
                if recipient_hex.len() != 64 || !recipient_hex.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err("recipient must be a 64-character AWE-ID".into());
                }
                let amount_atoms = parse_onecoin_atoms(&amount_value)?;
                if amount_atoms == 0 {
                    return Err("amount_coins must be positive".into());
                }
                let recipient = AweId::from_hex(recipient_hex)?;
                if recipient == node.identity.public.awe_id {
                    return Err("cannot transfer ONECOIN to the same wallet".into());
                }
                let mut ledger = onecoin_ledger.lock().map_err(|_| "ONECOIN ledger lock failed".to_string())?;
                let sender = node.identity.public.awe_id.clone();
                if ledger.members.is_empty() {
                    ledger.initialize_genesis(std::slice::from_ref(&sender))?;
                }
                ledger.ensure_member(&recipient);
                if ledger.balance_atoms(&sender) < amount_atoms {
                    return Err("insufficient ONECOIN balance".into());
                }
                let nonce = ledger.nonces.get(&sender.to_hex()).copied().unwrap_or(0);
                let tx = OnecoinTransaction::new(&node.identity, nonce, &recipient, amount_atoms, memo);
                let (tx_id, fee_atoms) = ledger.apply_transfer_with_fee(&tx, &node.identity.public.public_key, 100)?;
                fs::write(&onecoin_path, serde_json::to_vec_pretty(&*ledger).map_err(|_| "ONECOIN ledger serialization failed".to_string())?)
                    .map_err(|e| e.to_string())?;
                Ok((hex::encode(tx_id), fee_atoms, tx))
            })();
            match result {
                Ok((tx_id, fee_atoms, tx)) => { let recipient_id = tx.recipient; let delivered = if node.peers().await.into_iter().any(|p| p.awe_id == recipient_id) {
                        match serde_json::to_vec(&tx) {
                            Ok(bytes) => {
                                if node
                                    .send_to_peer_confirmed(
                                        &recipient_id,
                                        policy::ONECOIN_TRANSFER_STREAM,
                                        bytes.clone(),
                                    )
                                    .await
                                    .is_ok()
                                {
                                    true
                                } else {
                                    node.send_to_peer(
                                        &recipient_id,
                                        policy::ONECOIN_TRANSFER_STREAM,
                                        bytes,
                                    )
                                    .await
                                    .is_ok()
                                }
                            },
                            Err(_) => false,
                        }
                    } else {
                        false
                    }; ("200 OK", "application/json; charset=utf-8", serde_json::json!({"status":"accepted","tx_id":tx_id,"fee_atoms":fee_atoms,"fee_bps":100,"recipient_delivered":delivered,"recipient_pending":!delivered}).to_string()) },
                Err(error) => ("400 Bad Request", "application/json; charset=utf-8", serde_json::json!({"status":"rejected","error":error,"finality_mode":"LOCAL_LEDGER"}).to_string())
            }
            }
        },
        "/api/onebank/exchange/offers" if method == "GET" => {
            let offers: Vec<serde_json::Value> = fs::read(&onecoin_offers_path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
            ("200 OK", "application/json; charset=utf-8", serde_json::to_string(&offers).unwrap_or_else(|_| "[]".into()))
        },
        "/api/onebank/exchange/offers" if method == "POST" => {
            let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
            let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
            let mut offer = parsed.get("offer").cloned().unwrap_or(parsed);
            let validation = (|| -> Result<String, String> {
                let obj = offer.as_object().ok_or_else(|| "offer must be a JSON object".to_string())?;
                let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("").trim();
                if id.is_empty() || id.len() > 128 {
                    return Err("offer.id must be 1..128 characters".into());
                }
                let side = obj.get("side").and_then(|v| v.as_str()).unwrap_or("");
                if !matches!(side, "buy" | "sell") {
                    return Err("offer.side must be buy or sell".into());
                }
                let amount = obj.get("amount").and_then(|v| v.as_f64()).unwrap_or(0.0);
                if !amount.is_finite() || amount <= 0.0 {
                    return Err("offer.amount must be a positive finite number".into());
                }
                let price = obj.get("price").and_then(|v| v.as_f64()).unwrap_or(0.0);
                if !price.is_finite() || price <= 0.0 {
                    return Err("offer.price must be a positive finite number".into());
                }
                let currency = obj.get("currency").and_then(|v| v.as_str()).unwrap_or("");
                if !matches!(currency, "USD" | "EUR" | "AMD" | "GBP") {
                    return Err("offer.currency must be USD, EUR, AMD, or GBP".into());
                }
                let rail = obj.get("rail").and_then(|v| v.as_str()).unwrap_or("");
                if !matches!(rail, "ExternalPayment" | "BankTransfer" | "Cash") {
                    return Err("offer.rail must be ExternalPayment, BankTransfer, or Cash".into());
                }
                Ok(id.to_string())
            })();
            match validation {
                Err(error) => ("400 Bad Request", "application/json; charset=utf-8",
                    serde_json::json!({"status":"error","error":error}).to_string()),
                Ok(offer_id) => {
                    let mut offers: Vec<serde_json::Value> = fs::read(&onecoin_offers_path)
                        .ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
                    if offers.len() >= 10_000 {
                        ("507 Insufficient Storage", "application/json; charset=utf-8",
                            serde_json::json!({"status":"error","error":"offer book limit reached"}).to_string())
                    } else if offers.iter().any(|known| {
                        known.get("id").and_then(|v| v.as_str()) == Some(offer_id.as_str())
                    }) {
                        ("409 Conflict", "application/json; charset=utf-8",
                            serde_json::json!({"status":"error","error":"offer ID already exists"}).to_string())
                    } else {
                        if let Some(obj) = offer.as_object_mut() {
                            obj.insert("owner".into(), serde_json::json!(node.identity.public.awe_id.to_hex()));
                            obj.insert("settlement".into(), serde_json::json!("DIRECT_PERSON_TO_PERSON"));
                            obj.insert("coin_transfer".into(), serde_json::json!("AWENET_WALLET"));
                            obj.insert("fiat_transfer".into(), serde_json::json!("OUTSIDE_AWENET"));
                            obj.insert("created_at".into(), serde_json::json!(now_unix()));
                        }
                        offers.push(offer.clone());
                        let response = serde_json::to_vec_pretty(&offers)
                            .map_err(std::io::Error::other)
                            .and_then(|bytes| fs::write(&onecoin_offers_path, bytes));
                        match response {
                            Ok(()) => ("200 OK", "application/json; charset=utf-8",
                                serde_json::json!({
                                    "status":"published",
                                    "offer_id":offer_id,
                                    "offer":offer,
                                    "notice":"ONECOIN transfer is handled by AWENET wallet. Fiat is exchanged directly between people outside AWENET."
                                }).to_string()),
                            Err(error) => ("500 Internal Server Error", "application/json; charset=utf-8",
                                serde_json::json!({"status":"error","error":error.to_string()}).to_string())
                        }
                    }
                }
            }
        },
        "/api/node" => ("200 OK", "application/json; charset=utf-8", serde_json::json!({
            "id": format_uid(node.identity.public.awe_id.as_bytes()),
            "descriptor": node.node_descriptor(),
            "username": node.identity.public.username.as_str(),
            "address": node.listen_addr.to_string(),
            "protocol": 1
        }).to_string()),
        "/api/policy" => {
            let policy = policy_state.lock().map(|p| p.clone()).unwrap_or_default();
            ("200 OK", "application/json; charset=utf-8", serde_json::to_string(&policy).unwrap_or_else(|_| "{}".into()))
        },
        "/api/status" => {
            let peers = node.closest_peers(node.identity.public.awe_id.as_bytes(), 64).await;
            let peer_json = peers.iter().map(|p| serde_json::json!({
                "id": format_uid(&p.awe_id),
                "address": p.addresses.first().map(ToString::to_string).unwrap_or_else(|| "unknown".into()),
                "last_seen": p.last_seen_unix
            })).collect::<Vec<_>>();
            ("200 OK", "application/json; charset=utf-8", serde_json::json!({
                "product": "AWENET", "status": "online",
                "node_id": format_uid(node.identity.public.awe_id.as_bytes()),
                "node_address": node.listen_addr.to_string(),
                "transport": "AWE encrypted TCP", "ui": "connected", "peers": peer_json, "discovered_peers": peer_json.len(), "active_connections": node.active_peer_count().await
            }).to_string())
        },
        "/api/connect" if method == "POST" => {
            let address = request
                .split("address=")
                .nth(1)
                .and_then(|value| value.split_whitespace().next())
                .unwrap_or("");
            let address = address.replace("%3A", ":").replace("%3a", ":");
            match address.parse::<SocketAddr>() {
                Ok(addr) => {
                    let endpoint = addr.to_string();
                    let local_node_id = format_uid(node.identity.public.awe_id.as_bytes());
                    match record_bootstrap_endpoint(
                        &federation_state,
                        &federation_path,
                        &endpoint,
                        &local_node_id,
                    ) {
                        Ok(()) => {
                            let connect_node = node.clone();
                            tokio::spawn(async move {
                                let _ = connect_node.bootstrap(&[addr]).await;
                            });
                            (
                                "202 Accepted",
                                "application/json; charset=utf-8",
                                serde_json::json!({"status":"connecting","address":endpoint}).to_string(),
                            )
                        }
                        Err(error) => (
                            "500 Internal Server Error",
                            "application/json; charset=utf-8",
                            serde_json::json!({"status":"error","error":error}).to_string(),
                        ),
                    }
                }
                Err(_) => (
                    "400 Bad Request",
                    "application/json; charset=utf-8",
                    serde_json::json!({"status":"error","error":"invalid socket address"}).to_string(),
                ),
            }
        },
        "/api/federation" => {
            let state = federation_state.lock().map(|s| s.clone()).unwrap_or_default();
            ("200 OK", "application/json; charset=utf-8", serde_json::json!({
                "format": state.format, "version": state.version, "local_node_id": state.local_node_id,
                "data_centre_id": state.local_data_centre_id, "data_group_id": state.local_data_group_id,
                "centre_group_id": state.local_centre_group_id,
                "joined_data_centres": state.joined_data_centres, "joined_data_groups": state.joined_data_groups,
                "joined_centre_groups": state.joined_centre_groups,
                "data_centre_owners": state.data_centre_owners,
                "data_group_owners": state.data_group_owners,
                "bootstrap_endpoints": state.bootstrap_endpoints
            }).to_string())
        },
        "/api/federation/generate" if method == "POST" => {
            let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
            let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
            let kind = parsed.get("kind").and_then(|value| value.as_str()).unwrap_or("");
            let name = parsed.get("name").and_then(|value| value.as_str()).unwrap_or("AWE");
            let now = now_unix();
            let owner_node_id = format_uid(node.identity.public.awe_id.as_bytes());
            let result: Result<(String, String), String> = match kind {
                "awenode" => {
                    let endpoint = parsed.get("endpoint").and_then(|value| value.as_str())
                        .map(str::to_owned).unwrap_or_else(|| node.listen_addr.to_string());
                    let bootstrap = parsed.get("bootstrap").and_then(|value| value.as_array())
                        .map(|items| items.iter().filter_map(|value| value.as_str().map(str::to_owned)).collect())
                        .unwrap_or_default();
                    let dc_id = parsed.get("data_centre_id").and_then(|value| value.as_str())
                        .map(str::to_owned)
                        .or_else(|| federation_state.lock().ok().and_then(|state| state.local_data_centre_id.clone()))
                        .unwrap_or_else(|| format!("dc-{}", &hex::encode(blake3::hash(format!("AWE/DC/{owner_node_id}").as_bytes()).as_bytes())[..24]));
                    let config = federation::generate_awenode_for_dc(&owner_node_id, &dc_id, name, &endpoint, bootstrap, now);
                    federation::validate_awenode(&config).and_then(|_| {
                        serde_json::to_string_pretty(&config)
                            .map(|content| ("awenode.awenode".into(), content))
                            .map_err(|error| error.to_string())
                    })
                }
                "awedc" => {
                    let dc_id = parsed.get("data_centre_id").and_then(|value| value.as_str()).unwrap_or("");
                    let endpoints = parsed.get("endpoints").and_then(|value| value.as_array())
                        .map(|items| items.iter().filter_map(|value| value.as_str().map(str::to_owned)).collect())
                        .unwrap_or_else(|| vec![node.listen_addr.to_string()]);
                    let members = parsed.get("member_node_ids").and_then(|value| value.as_array())
                        .map(|items| items.iter().filter_map(|value| value.as_str().map(str::to_owned)).collect())
                        .unwrap_or_else(|| vec![owner_node_id.clone()]);
                    if dc_id.is_empty() {
                        Err("data_centre_id is required".into())
                    } else {
                        let config = federation::generate_awedc(&owner_node_id, dc_id, name, endpoints, members, now);
                        federation::validate_awedc(&config).and_then(|_| {
                            serde_json::to_string_pretty(&config)
                                .map(|content| ("data-centre.awedc".into(), content))
                                .map_err(|error| error.to_string())
                        })
                    }
                }
                "dgc" => {
                    let dc_id = parsed.get("owner_data_centre_id").and_then(|value| value.as_str()).unwrap_or("");
                    let centres: Vec<String> = parsed.get("data_centre_ids").and_then(|value| value.as_array())
                        .map(|items| items.iter().filter_map(|value| value.as_str().map(str::to_owned)).collect())
                        .unwrap_or_default();
                    let (owner, all_centres_trusted) = federation_state.lock().ok()
                        .map(|state| (
                            state.data_centre_owners.get(dc_id).cloned(),
                            centres.iter().all(|id| state.data_centre_owners.contains_key(id)),
                        ))
                        .unwrap_or((None, false));
                    if dc_id.is_empty() {
                        Err("owner_data_centre_id is required".into())
                    } else if owner.as_deref() != Some(owner_node_id.as_str()) {
                        Err("only the authenticated owner of the data centre may create a data group".into())
                    } else if !all_centres_trusted {
                        Err("all data centres in a data group must have verified signed invitations".into())
                    } else {
                        let config = federation::generate_dgc(dc_id, name, centres, now);
                        federation::validate_dgc(&config).and_then(|_| {
                            serde_json::to_string_pretty(&config)
                                .map(|content| ("data-group.dgc".into(), content))
                                .map_err(|error| error.to_string())
                        })
                    }
                }
                "cgc" => {
                    let dg_id = parsed.get("owner_data_group_id").and_then(|value| value.as_str()).unwrap_or("");
                    let groups: Vec<String> = parsed.get("data_group_ids").and_then(|value| value.as_array())
                        .map(|items| items.iter().filter_map(|value| value.as_str().map(str::to_owned)).collect())
                        .unwrap_or_default();
                    let (owner, all_groups_trusted) = federation_state.lock().ok()
                        .map(|state| (
                            state.data_group_owners.get(dg_id).cloned(),
                            groups.iter().all(|id| state.data_group_owners.contains_key(id)),
                        ))
                        .unwrap_or((None, false));
                    if dg_id.is_empty() {
                        Err("owner_data_group_id is required".into())
                    } else if owner.as_deref() != Some(owner_node_id.as_str()) {
                        Err("only the authenticated owner of the data group may create a centre group".into())
                    } else if !all_groups_trusted {
                        Err("all data groups in a centre group must have verified signed invitations".into())
                    } else {
                        let config = federation::generate_cgc(dg_id, name, groups, now);
                        federation::validate_cgc(&config).and_then(|_| {
                            serde_json::to_string_pretty(&config)
                                .map(|content| ("centre-group.cgc".into(), content))
                                .map_err(|error| error.to_string())
                        })
                    }
                }
                _ => Err("kind must be awenode, awedc, dgc or cgc".to_string())
            };
            match result {
                Ok((filename, content)) => {
                    match serde_json::from_str::<serde_json::Value>(&content) {
                        Ok(payload) => match sign_federation_invitation(kind, payload.clone(), &node.identity) {
                            Ok(signed_content) => match record_generated_federation_owner(
                                kind, &payload, &owner_node_id, &federation_state, &federation_path,
                            ) {
                                Ok(()) => (
                                    "200 OK",
                                    "application/json; charset=utf-8",
                                    serde_json::json!({"status":"generated","filename":filename,"content":signed_content}).to_string(),
                                ),
                                Err(error) => (
                                    "500 Internal Server Error",
                                    "application/json; charset=utf-8",
                                    serde_json::json!({"status":"error","error":error}).to_string(),
                                ),
                            },
                            Err(error) => (
                                "400 Bad Request",
                                "application/json; charset=utf-8",
                                serde_json::json!({"status":"error","error":error}).to_string(),
                            ),
                        },
                        Err(error) => (
                            "500 Internal Server Error",
                            "application/json; charset=utf-8",
                            serde_json::json!({"status":"error","error":error.to_string()}).to_string(),
                        ),
                    }
                }
                Err(error) => (
                    "400 Bad Request",
                    "application/json; charset=utf-8",
                    serde_json::json!({"status":"error","error":error}).to_string(),
                ),
            }
        },
        "/api/federation/leave" if method == "POST" => {
            let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
            match serde_json::from_str::<serde_json::Value>(body) {
                Err(error) => (
                    "400 Bad Request",
                    "application/json; charset=utf-8",
                    serde_json::json!({"status":"error","error":error.to_string()}).to_string(),
                ),
                Ok(input) => {
                    let kind = input
                        .get("kind")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    let id = input
                        .get("id")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    let local_node_id = format_uid(node.identity.public.awe_id.as_bytes());
                    match federation_state.lock() {
                        Err(_) => (
                            "500 Internal Server Error",
                            "application/json; charset=utf-8",
                            serde_json::json!({
                                "status":"error",
                                "error":"federation state lock poisoned"
                            })
                            .to_string(),
                        ),
                        Ok(mut state) => {
                            let mut next = state.clone();
                            match leave_federation_membership(
                                &mut next,
                                &kind,
                                &id,
                                &local_node_id,
                            ) {
                                Err(error) => (
                                    "400 Bad Request",
                                    "application/json; charset=utf-8",
                                    serde_json::json!({"status":"error","error":error}).to_string(),
                                ),
                                Ok(()) => match federation::save_json(&next, &federation_path) {
                                    Ok(()) => {
                                        *state = next;
                                        (
                                            "200 OK",
                                            "application/json; charset=utf-8",
                                            serde_json::json!({
                                                "status":"left",
                                                "kind":kind,
                                                "id":id
                                            })
                                            .to_string(),
                                        )
                                    }
                                    Err(error) => (
                                        "500 Internal Server Error",
                                        "application/json; charset=utf-8",
                                        serde_json::json!({"status":"error","error":error})
                                            .to_string(),
                                    ),
                                },
                            }
                        }
                    }
                }
            }
        }
        "/api/federation/import" if method == "POST" => {
            let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
            let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
            let kind = parsed.get("kind").and_then(|value| value.as_str()).unwrap_or("");
            let content = parsed.get("content").and_then(|value| value.as_str()).unwrap_or("");
            let local_node_id = format_uid(node.identity.public.awe_id.as_bytes());
            let result: Result<Vec<String>, anyhow::Error> = (|| {
                let (payload, owner_node_id) = verify_signed_federation_invitation(content, kind)
                    .map_err(anyhow::Error::msg)?;
                let payload_content = serde_json::to_string(&payload)?;
                let mut state = federation_state.lock()
                    .map_err(|_| anyhow::anyhow!("federation state lock poisoned"))?;
                let mut next = state.clone();
                let mut new_endpoints = Vec::new();
                match kind {
                    "awenode" => {
                        let config: AweNodeConfig = serde_json::from_str(&payload_content)?;
                        federation::validate_awenode(&config).map_err(anyhow::Error::msg)?;
                        if config.node_id != owner_node_id {
                            return Err(anyhow::anyhow!("node invitation identity does not match signer"));
                        }
                        next.local_node_id = local_node_id.clone();
                        for endpoint in config.bootstrap_endpoints.iter().chain(std::iter::once(&config.endpoint)) {
                            if !next.bootstrap_endpoints.contains(endpoint) {
                                next.bootstrap_endpoints.push(endpoint.clone());
                                new_endpoints.push(endpoint.clone());
                            }
                        }
                        if !next.joined_data_centres.contains(&config.data_centre_id) {
                            next.joined_data_centres.push(config.data_centre_id.clone());
                        }
                    }
                    "awedc" => {
                        let config: DataCentreConfig = serde_json::from_str(&payload_content)?;
                        federation::validate_awedc(&config).map_err(anyhow::Error::msg)?;
                        if config.owner_node_id != owner_node_id {
                            return Err(anyhow::anyhow!("data-centre owner does not match signer"));
                        }
                        if next.data_centre_owners.get(&config.data_centre_id)
                            .is_some_and(|owner| owner != &owner_node_id)
                        {
                            return Err(anyhow::anyhow!("data-centre ID is already bound to a different owner"));
                        }
                        next.local_node_id = local_node_id.clone();
                        next.data_centre_owners.insert(config.data_centre_id.clone(), owner_node_id.clone());
                        if next.local_data_centre_id.is_none() {
                            next.local_data_centre_id = Some(format!(
                                "dc-{}",
                                &hex::encode(blake3::hash(format!("AWE/DC/{}", next.local_node_id).as_bytes()).as_bytes())[..24]
                            ));
                        }
                        for endpoint in &config.endpoints {
                            if !next.bootstrap_endpoints.contains(endpoint) {
                                next.bootstrap_endpoints.push(endpoint.clone());
                                new_endpoints.push(endpoint.clone());
                            }
                        }
                        if !next.joined_data_centres.contains(&config.data_centre_id) {
                            next.joined_data_centres.push(config.data_centre_id.clone());
                        }
                    }
                    "dgc" => {
                        let config: DataGroupConfig = serde_json::from_str(&payload_content)?;
                        federation::validate_dgc(&config).map_err(anyhow::Error::msg)?;
                        if next.data_centre_owners.get(&config.owner_data_centre_id) != Some(&owner_node_id) {
                            return Err(anyhow::anyhow!("data-group invitation was not signed by the trusted data-centre owner"));
                        }
                        if config.data_centre_ids.iter().any(|id| !next.data_centre_owners.contains_key(id)) {
                            return Err(anyhow::anyhow!("data-group references a data centre whose signed invitation has not been imported"));
                        }
                        if next.data_group_owners.get(&config.data_group_id)
                            .is_some_and(|owner| owner != &owner_node_id)
                        {
                            return Err(anyhow::anyhow!("data-group ID is already bound to a different owner"));
                        }
                        next.local_node_id = local_node_id.clone();
                        next.data_group_owners.insert(config.data_group_id.clone(), owner_node_id.clone());
                        next.local_data_group_id = Some(config.data_group_id.clone());
                        for dc in &config.data_centre_ids {
                            if !next.joined_data_centres.contains(dc) {
                                next.joined_data_centres.push(dc.clone());
                            }
                        }
                        if !next.joined_data_groups.contains(&config.data_group_id) {
                            next.joined_data_groups.push(config.data_group_id.clone());
                        }
                    }
                    "cgc" => {
                        let config: CentreGroupConfig = serde_json::from_str(&payload_content)?;
                        federation::validate_cgc(&config).map_err(anyhow::Error::msg)?;
                        if next.data_group_owners.get(&config.owner_data_group_id) != Some(&owner_node_id) {
                            return Err(anyhow::anyhow!("centre-group invitation was not signed by the trusted data-group owner"));
                        }
                        if config.data_group_ids.iter().any(|id| !next.data_group_owners.contains_key(id)) {
                            return Err(anyhow::anyhow!("centre group references a data group whose signed invitation has not been imported"));
                        }
                        next.local_node_id = local_node_id.clone();
                        next.local_centre_group_id = Some(config.centre_group_id.clone());
                        for dg in &config.data_group_ids {
                            if !next.joined_data_groups.contains(dg) {
                                next.joined_data_groups.push(dg.clone());
                            }
                        }
                        if !next.joined_centre_groups.contains(&config.centre_group_id) {
                            next.joined_centre_groups.push(config.centre_group_id.clone());
                        }
                    }
                    _ => return Err(anyhow::anyhow!("kind must be awenode, awedc, dgc or cgc")),
                }
                next.format = "awenet".into();
                next.version = federation::FORMAT_VERSION;
                federation::save_json(&next, &federation_path).map_err(anyhow::Error::msg)?;
                *state = next;
                Ok(new_endpoints)
            })();
            match result {
                Ok(new_endpoints) => {
                    let addresses = new_endpoints.iter()
                        .filter_map(|endpoint| endpoint.parse::<SocketAddr>().ok())
                        .collect::<Vec<_>>();
                    let discovered = if addresses.is_empty() { 0 } else { node.bootstrap(&addresses).await.unwrap_or(0) };
                    ("200 OK", "application/json; charset=utf-8",
                        serde_json::json!({"status":"imported","connected_bootstrap_peers":discovered}).to_string())
                }
                Err(error) => ("400 Bad Request", "application/json; charset=utf-8",
                    serde_json::json!({"status":"error","error":error.to_string()}).to_string())
            }
        },
        "/api/health" => ("200 OK", "application/json; charset=utf-8", serde_json::json!({
            "status":"healthy","core":"ready","network":"listening","ui":"ready","api":"ready"
        }).to_string()),
        "/api/call/signals" => {
            let since = request.lines().next().unwrap_or("").split_whitespace().nth(1)
                .and_then(|p| p.split('?').nth(1))
                .and_then(|q| q.split('&').find_map(|v| v.strip_prefix("since=")))
                .and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
            let local_uid = format_uid(node.identity.public.awe_id.as_bytes());
            let signals = messenger.lock().map(|log| log.iter().filter(|m| {
                m.get("kind").and_then(|v| v.as_str()) == Some("awe.call.v1")
                    && m.get("recipient").and_then(|v| v.as_str()) == Some(local_uid.as_str())
                    && m.get("timestamp").and_then(|v| v.as_u64()).unwrap_or(0) > since
            }).cloned().collect::<Vec<_>>()).unwrap_or_default();
            ("200 OK", "application/json; charset=utf-8", serde_json::json!({"status":"ok","signals":signals}).to_string())
        },
        "/api/call/signal" if method == "POST" => {
            let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
            let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
            let recipient = parsed.get("recipient").and_then(|v| v.as_str()).unwrap_or("").trim();
            let call_id = parsed.get("call_id").and_then(|v| v.as_str()).unwrap_or("").trim();
            let signal_type = parsed.get("signal_type").and_then(|v| v.as_str()).unwrap_or("").trim();
            let data = parsed.get("data").and_then(|v| v.as_str()).unwrap_or("");
            if recipient.is_empty() || call_id.is_empty() || signal_type.is_empty() || data.is_empty() {
                ("400 Bad Request", "application/json; charset=utf-8", serde_json::json!({"status":"error","error":"recipient, call_id, signal_type and data are required"}).to_string())
            } else {
                let peers = node.closest_peers(node.identity.public.awe_id.as_bytes(), 64).await;
                let recipient_id = hex::decode(recipient).ok().and_then(|b| <[u8;32]>::try_from(b).ok())
                    .or_else(|| peers.iter().find(|p| format_uid(&p.awe_id) == recipient).map(|p| p.awe_id));
                match recipient_id {
                    None => ("400 Bad Request", "application/json; charset=utf-8", serde_json::json!({"status":"error","error":"recipient must be a 64-hex AWE ID or discovered UID"}).to_string()),
                    Some(recipient_id) => {
                        let timestamp = now_unix();
                        let digest = blake3::hash(format!("call:{}:{}:{}:{}", call_id, signal_type, timestamp, format_uid(node.identity.public.awe_id.as_bytes())).as_bytes());
                        let id = hex::encode(&digest.as_bytes()[..16]);
                        let envelope = serde_json::json!({
                            "kind":"awe.call.v1","id":id,"call_id":call_id,
                            "sender":format_uid(node.identity.public.awe_id.as_bytes()),
                            "recipient":format_uid(&recipient_id),"signal_type":signal_type,"data":data,"timestamp":timestamp
                        });
                        match serde_json::to_vec(&envelope) {
                            Ok(payload) => match node.send_to_peer(&recipient_id, 100, payload).await {
                                Ok(_) => {
                                    if let Ok(mut log)=messenger.lock(){ append_messenger_entry(&mut log, envelope.clone()); }
                                    ("200 OK","application/json; charset=utf-8",serde_json::json!({"status":"sent","signal":envelope}).to_string())
                                },
                                Err(error)=>("502 Bad Gateway","application/json; charset=utf-8",serde_json::json!({"status":"error","error":error.to_string()}).to_string())
                            },
                            Err(error)=>("500 Internal Server Error","application/json; charset=utf-8",serde_json::json!({"status":"error","error":error.to_string()}).to_string())
                        }
                    }
                }
            }
        },
        "/api/groups" => {
            let groups = community.lock().map(|s| s.get("groups").cloned().unwrap_or_else(|| serde_json::json!([]))).unwrap_or_else(|_| serde_json::json!([]));
            ("200 OK", "application/json; charset=utf-8", serde_json::json!({"status":"ok","groups":groups}).to_string())
        },
        "/api/groups/create" if method == "POST" => {
            let body=request.split("\r\n\r\n").nth(1).unwrap_or("");
            let p:serde_json::Value=serde_json::from_str(body).unwrap_or_default();
            let title=p.get("title").and_then(|v|v.as_str()).unwrap_or("").trim().to_string();
            let mut members=p.get("members").and_then(|v|v.as_array()).cloned().unwrap_or_default().into_iter().filter_map(|v|v.as_str().map(str::to_string)).filter(|v|!v.is_empty()).collect::<Vec<_>>();
            let local=node.identity.public.awe_id.to_hex();
            let title_validation = validate_community_title(&title);
            let final_member_count =
                members.len() + (if members.contains(&local) { 0 } else { 1 });
            let members_validation = if final_member_count > MAX_GROUP_MEMBERS {
                Err(format!("group cannot exceed {MAX_GROUP_MEMBERS} members"))
            } else {
                validate_group_member_ids(&members)
            };
            if let Err(error) = title_validation {
                ("400 Bad Request","application/json; charset=utf-8",serde_json::json!({"status":"error","error":error}).to_string())
            } else if let Err(error) = members_validation {
                ("400 Bad Request","application/json; charset=utf-8",serde_json::json!({"status":"error","error":error}).to_string())
            } else {
                if !members.contains(&local){members.push(local.clone());} members.sort();members.dedup();
                let id=format!("gid-{}",hex::encode(&blake3::hash(format!("{}:{}:{}",local,title,now_unix()).as_bytes()).as_bytes()[..12]));
                let group=serde_json::json!({"id":id,"title":title,"owner":local,"members":members,"created_at":now_unix(),"messages":[]});
                if let Ok(mut s)=community.lock(){if let Some(a)=s.get_mut("groups").and_then(|v|v.as_array_mut()){a.push(group.clone());}}
                let env=serde_json::json!({"kind":"awe.group.v1","event":"upsert","group":group,"sender":local});
                let members=group.get("members").and_then(|v|v.as_array()).cloned().unwrap_or_default(); let mut delivered=0usize;
                if let Ok(payload)=serde_json::to_vec(&env){for peer in node.peers().await{let pid=hex::encode(peer.awe_id);if pid!=local&&members.iter().any(|v|v.as_str()==Some(pid.as_str()))&&node.send_to_peer(&peer.awe_id,100,payload.clone()).await.is_ok(){delivered+=1;}}}
                ("200 OK","application/json; charset=utf-8",serde_json::json!({"status":"created","group":group,"delivered_members":delivered}).to_string())
            }
        },
        "/api/groups/send" if method == "POST" => {
            let body=request.split("\r\n\r\n").nth(1).unwrap_or(""); let p:serde_json::Value=serde_json::from_str(body).unwrap_or_default();
            let gid=p.get("group_id").and_then(|v|v.as_str()).unwrap_or("").trim(); let text=p.get("text").and_then(|v|v.as_str()).unwrap_or("").trim(); let local=node.identity.public.awe_id.to_hex();
            if gid.is_empty() {
                ("400 Bad Request","application/json; charset=utf-8",serde_json::json!({"status":"error","error":"group_id is required"}).to_string())
            } else if let Err(error) = validate_community_message(text) {
                ("400 Bad Request","application/json; charset=utf-8",serde_json::json!({"status":"error","error":error}).to_string())
            } else {
                let group=community.lock().ok().and_then(|s|s.get("groups").and_then(|v|v.as_array()).and_then(|a|a.iter().find(|g|g.get("id").and_then(|v|v.as_str())==Some(gid)).cloned()));
                match group{
                    None=>("404 Not Found","application/json; charset=utf-8",serde_json::json!({"status":"error","error":"group not found"}).to_string()),
                    Some(group) if !group.get("members").and_then(|v|v.as_array()).map(|a|a.iter().any(|v|v.as_str()==Some(local.as_str()))).unwrap_or(false)=>("403 Forbidden","application/json; charset=utf-8",serde_json::json!({"status":"error","error":"not a group member"}).to_string()),
                    Some(group)=>{
                        let timestamp=now_unix(); let message=serde_json::json!({"id":hex::encode(&blake3::hash(format!("group:{}:{}:{}",gid,local,timestamp).as_bytes()).as_bytes()[..16]),"sender":local,"text":text,"timestamp":timestamp});
                        if let Ok(mut st)=community.lock(){if let Some(g)=st.get_mut("groups").and_then(|v|v.as_array_mut()).and_then(|a|a.iter_mut().find(|g|g.get("id").and_then(|v|v.as_str())==Some(gid))){if let Some(a)=g.get_mut("messages").and_then(|v|v.as_array_mut()){append_bounded_json_entry(a, message.clone());}}}
                        let env=serde_json::json!({"kind":"awe.group.v1","event":"message","group_id":gid,"message":message,"sender":local}); let members=group.get("members").and_then(|v|v.as_array()).cloned().unwrap_or_default(); let mut delivered=0usize;
                        if let Ok(payload)=serde_json::to_vec(&env){for peer in node.peers().await{let pid=hex::encode(peer.awe_id);if pid!=local&&members.iter().any(|v|v.as_str()==Some(pid.as_str())){let sent=node.send_to_peer_confirmed(&peer.awe_id,100,payload.clone()).await.is_ok()||node.send_to_peer(&peer.awe_id,100,payload.clone()).await.is_ok();if sent{delivered+=1;}}}}
                        ("200 OK","application/json; charset=utf-8",serde_json::json!({"status":"sent","message":message,"delivered_members":delivered}).to_string())
                    }
                }
            }
        },
        "/api/channels" => {
            let channels=community.lock().map(|s|s.get("channels").cloned().unwrap_or_else(||serde_json::json!([]))).unwrap_or_else(|_|serde_json::json!([]));
            ("200 OK","application/json; charset=utf-8",serde_json::json!({"status":"ok","channels":channels}).to_string())
        },
        "/api/channels/create" if method == "POST" => {
            let body=request.split("\r\n\r\n").nth(1).unwrap_or(""); let p:serde_json::Value=serde_json::from_str(body).unwrap_or_default(); let title=p.get("title").and_then(|v|v.as_str()).unwrap_or("").trim().to_string(); let local=node.identity.public.awe_id.to_hex();
            if let Err(error) = validate_community_title(&title) {
                ("400 Bad Request","application/json; charset=utf-8",serde_json::json!({"status":"error","error":error}).to_string())
            } else {
                let id=format!("cid-{}",hex::encode(&blake3::hash(format!("{}:{}:{}",local,title,now_unix()).as_bytes()).as_bytes()[..12])); let channel=serde_json::json!({"id":id,"title":title,"owner":local,"subscribers":[local],"created_at":now_unix(),"messages":[]});
                if let Ok(mut st)=community.lock(){if let Some(a)=st.get_mut("channels").and_then(|v|v.as_array_mut()){a.push(channel.clone());}}
                let env=serde_json::json!({"kind":"awe.channel.v1","event":"upsert","channel":channel,"sender":local}); let mut delivered=0usize;
                if let Ok(payload)=serde_json::to_vec(&env){for peer in node.peers().await{if node.send_to_peer(&peer.awe_id,100,payload.clone()).await.is_ok(){delivered+=1;}}}
                ("200 OK","application/json; charset=utf-8",serde_json::json!({"status":"created","channel":channel,"delivered_peers":delivered}).to_string())
            }
        },
        "/api/channels/subscribe" if method == "POST" => {
            let body=request.split("\r\n\r\n").nth(1).unwrap_or("");
            let p:serde_json::Value=serde_json::from_str(body).unwrap_or_default();
            let cid=p.get("channel_id").and_then(|v|v.as_str()).unwrap_or("").trim();
            let local=node.identity.public.awe_id.to_hex();
            let channel=community.lock().ok().and_then(|s|s.get("channels").and_then(|v|v.as_array()).and_then(|a|a.iter().find(|c|c.get("id").and_then(|v|v.as_str())==Some(cid)).cloned()));
            match channel {
                None if cid.is_empty()=>("400 Bad Request","application/json; charset=utf-8",serde_json::json!({"status":"error","error":"channel_id is required"}).to_string()),
                None=>("404 Not Found","application/json; charset=utf-8",serde_json::json!({"status":"error","error":"channel not found"}).to_string()),
                Some(channel)=>{
                    let owner=channel.get("owner").and_then(|v|v.as_str()).unwrap_or("").to_string();
                    if let Ok(mut st)=community.lock(){if let Some(ch)=st.get_mut("channels").and_then(|v|v.as_array_mut()).and_then(|a|a.iter_mut().find(|c|c.get("id").and_then(|v|v.as_str())==Some(cid))){if let Some(a)=ch.get_mut("subscribers").and_then(|v|v.as_array_mut()){if !a.iter().any(|v|v.as_str()==Some(local.as_str())){a.push(serde_json::Value::String(local.clone()));}}}}
                    if owner==local {
                        ("200 OK","application/json; charset=utf-8",serde_json::json!({"status":"subscribed","channel_id":cid,"subscriber":local,"owner_notified":true}).to_string())
                    } else {
                        let owner_id=hex::decode(&owner).ok().and_then(|b|<[u8;32]>::try_from(b).ok());
                        let event=serde_json::json!({"kind":"awe.channel.v1","event":"subscribe","channel_id":cid,"subscriber":local});
                        let (sent, delivery_error) = match owner_id {
                            Some(id) => match serde_json::to_vec(&event) {
                                Ok(bytes) => match node.send_to_peer_confirmed(&id, 100, bytes.clone()).await {
                                    Ok(_) => (true, None),
                                    Err(confirmed_error) => match node.send_to_peer(&id, 100, bytes).await {
                                        Ok(_) => (true, None),
                                        Err(fallback_error) => (false, Some(format!(
                                            "confirmed send failed: {confirmed_error}; fallback send failed: {fallback_error}"
                                        ))),
                                    },
                                },
                                Err(error) => (false, Some(error.to_string())),
                            },
                            None => (false, Some("invalid channel owner AWEID".to_string())),
                        };
                        (if sent{"200 OK"}else{"202 Accepted"},"application/json; charset=utf-8",serde_json::json!({"status":"subscribed","channel_id":cid,"subscriber":local,"owner_notified":sent,"pending":!sent,"delivery_error":delivery_error}).to_string())
                    }
                }
            }
        },
        "/api/channels/publish" if method == "POST" => {
            let body=request.split("\r\n\r\n").nth(1).unwrap_or(""); let p:serde_json::Value=serde_json::from_str(body).unwrap_or_default(); let cid=p.get("channel_id").and_then(|v|v.as_str()).unwrap_or("").trim(); let text=p.get("text").and_then(|v|v.as_str()).unwrap_or("").trim(); let local=node.identity.public.awe_id.to_hex();
            let channel=community.lock().ok().and_then(|s|s.get("channels").and_then(|v|v.as_array()).and_then(|a|a.iter().find(|c|c.get("id").and_then(|v|v.as_str())==Some(cid)).cloned()));
            match channel{
                None=>("404 Not Found","application/json; charset=utf-8",serde_json::json!({"status":"error","error":"channel not found"}).to_string()),
                Some(channel) if channel.get("owner").and_then(|v|v.as_str())!=Some(local.as_str())=>("403 Forbidden","application/json; charset=utf-8",serde_json::json!({"status":"error","error":"only channel owner may publish"}).to_string()),
                Some(_channel) if validate_community_message(text).is_err()=>("400 Bad Request","application/json; charset=utf-8",serde_json::json!({"status":"error","error":validate_community_message(text).unwrap_err()}).to_string()),
                Some(_channel)=>{
                    let message=serde_json::json!({"id":hex::encode(&blake3::hash(format!("channel:{}:{}:{}",cid,local,now_unix()).as_bytes()).as_bytes()[..16]),"sender":local,"text":text,"timestamp":now_unix()});
                    if let Ok(mut st)=community.lock(){if let Some(ch)=st.get_mut("channels").and_then(|v|v.as_array_mut()).and_then(|a|a.iter_mut().find(|c|c.get("id").and_then(|v|v.as_str())==Some(cid))){if let Some(a)=ch.get_mut("messages").and_then(|v|v.as_array_mut()){append_bounded_json_entry(a, message.clone());}}}
                    let env=serde_json::json!({"kind":"awe.channel.v1","event":"message","channel_id":cid,"message":message,"sender":local}); let mut delivered=0usize;
                    let active=node.active_peers().await;
                    for peer_id in active{let pid=format_uid(&peer_id);if pid!=local{let send_node=node.clone();let bytes=serde_json::to_vec(&env).unwrap_or_default();let fallback=serde_json::json!({"kind":"awe.messenger.v1","id":message.get("id").cloned().unwrap_or_else(||serde_json::json!("")),"sender":local,"recipient":pid,"text":text,"timestamp":message.get("timestamp").cloned().unwrap_or_else(||serde_json::json!(now_unix())),"channel_id":cid,"channel_message":message.clone(),"silent":true});let fallback_bytes=serde_json::to_vec(&fallback).unwrap_or_default();tokio::spawn(async move{if send_node.send_to_peer(&peer_id,100,bytes).await.is_err(){let _=send_node.send_to_peer_confirmed(&peer_id,100,fallback_bytes.clone()).await;}else{let _=send_node.send_to_peer(&peer_id,100,fallback_bytes).await;}});delivered+=1;}}
                    ("200 OK","application/json; charset=utf-8",serde_json::json!({"status":"published","message":message,"delivered_subscribers":delivered}).to_string())
                }
            }
        },
        "/api/messenger" => {
            ("200 OK", "application/json; charset=utf-8", serde_json::json!({
                "transport":"AWE encrypted TCP",
                "application_transport":"authenticated peer data stream",
                "messages": messenger.lock().map(|x| x.clone()).unwrap_or_default()
            }).to_string())
        },
        "/api/store/catalog" => {
            let root = PathBuf::from(data_dir_for_api()).join("store");
            match Store::open(&root) {
                Ok(_store) => {
                    let mut apps = Vec::new();
                    if let Ok(entries) = fs::read_dir(root.join("packages")) {
                        for entry in entries.flatten() {
                            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) { continue; }
                            let Ok(bytes) = fs::read(entry.path()) else { continue; };
                            let Ok(package) = awep2p_core::store::AWEPackage::from_bytes(&bytes) else { continue; };
                            let package_hash = hex::encode(blake3::hash(&bytes).as_bytes());
                            apps.push(serde_json::json!({
                                "package_hash": package_hash,
                                "manifest": package.manifest.manifest
                            }));
                        }
                    }
                    apps.sort_by(|a,b| a.get("package_hash").and_then(|v| v.as_str()).cmp(&b.get("package_hash").and_then(|v| v.as_str())));
                    let installed = fs::read_dir(root.join("installed")).ok().into_iter().flatten()
                        .filter_map(|e| e.ok()).filter_map(|e| fs::read(e.path()).ok())
                        .filter_map(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                        .collect::<Vec<_>>();
                    ("200 OK", "application/json; charset=utf-8", serde_json::json!({"status":"ok","apps":apps,"installed":installed}).to_string())
                }
                Err(error) => ("500 Internal Server Error", "application/json; charset=utf-8", serde_json::json!({"status":"error","error":error.to_string()}).to_string())
            }
        },
        "/api/store/install" if method == "POST" => {
            let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
            let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
            let hash = parsed.get("package_hash").and_then(|v| v.as_str()).unwrap_or("");
            let granted = parsed.get("granted_permissions").cloned().unwrap_or_else(|| serde_json::json!([]));
            let hash_bytes = hex::decode(hash).ok().and_then(|b| <[u8;32]>::try_from(b).ok());
            let permissions: Result<Vec<AppCapability>, _> = serde_json::from_value(granted);
            match (hash_bytes, permissions) {
                (Some(hash), Ok(permissions)) => {
                    let root = PathBuf::from(data_dir_for_api()).join("store");
                    match Store::open(&root).and_then(|store| store.install(&hash, &permissions)) {
                        Ok(app) => ("200 OK", "application/json; charset=utf-8", serde_json::json!({"status":"installed","app":app}).to_string()),
                        Err(error) => ("400 Bad Request", "application/json; charset=utf-8", serde_json::json!({"status":"error","error":error.to_string()}).to_string())
                    }
                }
                _ => ("400 Bad Request", "application/json; charset=utf-8", serde_json::json!({"status":"error","error":"invalid package hash or permissions"}).to_string())
            }
        },
        "/api/security" => ("200 OK", "application/json; charset=utf-8", serde_json::json!({
            "identity":"ed25519","transport":"x25519 + chacha20-poly1305","replay_protection":"enabled","a2p2_fixed_packet":1280
        }).to_string()),
        "/api/messenger/send" if method == "POST" => {
            let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
            let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
            let recipient = parsed.get("recipient").and_then(|v| v.as_str()).unwrap_or("").trim();
            let text = parsed.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
            if recipient.is_empty() || text.is_empty() {
                ("400 Bad Request", "application/json; charset=utf-8", serde_json::json!({"status":"error","error":"recipient and text are required"}).to_string())
            } else {
                let runtime_policy = policy_state.lock().map(|p| p.clone()).unwrap_or_default();
                if !runtime_policy.allows_message(text.len()) || !runtime_policy.allows_stream(policy::MESSENGER_STREAM) {
                    ("413 Payload Too Large", "application/json; charset=utf-8", serde_json::json!({"status":"rejected","error":"message rejected by local AWENET policy"}).to_string())
                } else {
                let peers = node.closest_peers(node.identity.public.awe_id.as_bytes(), 64).await;
                let recipient_id = if let Some(hex_id) = recipient.strip_prefix("0x").or_else(|| recipient.strip_prefix("0X")) {
                    hex::decode(hex_id).ok().and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
                } else {
                    hex::decode(recipient).ok().and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
                }.or_else(|| {
                    peers.iter().find(|peer| format_uid(&peer.awe_id) == recipient).map(|peer| peer.awe_id)
                });
                match recipient_id {
                    None => ("400 Bad Request", "application/json; charset=utf-8", serde_json::json!({"status":"error","error":"recipient must be a 64-hex AWE ID or a discovered UID"}).to_string()),
                    Some(recipient_id) => {
                        let timestamp = now_unix();
                let digest = blake3::hash(format!("{}:{}:{}:{}", format_uid(node.identity.public.awe_id.as_bytes()), recipient, text, timestamp).as_bytes());
                let message_id = hex::encode(&digest.as_bytes()[..16]);
                let envelope = serde_json::json!({
                    "kind": "awe.messenger.v1",
                    "id": message_id,
                    "sender": format_uid(node.identity.public.awe_id.as_bytes()),
                    "recipient": format_uid(&recipient_id),
                    "text": text,
                    "timestamp": timestamp
                });
                match serde_json::to_vec(&envelope) {
                    Ok(payload) => match node.send_to_peer(&recipient_id, 100, payload).await {
                        Ok(rtt) => {
                            let item = serde_json::json!({
                                "id": message_id,
                                "sender": format_uid(node.identity.public.awe_id.as_bytes()),
                                "recipient": format_uid(&recipient_id),
                                "text": text,
                                "state": "sent",
                                "timestamp": timestamp,
                                "rtt_ms": rtt.as_millis()
                            });
                            if let Ok(mut log) = messenger.lock() {
                                append_messenger_entry(&mut log, item.clone());
                            }
                            ("200 OK", "application/json; charset=utf-8", serde_json::json!({"status":"sent","message":item}).to_string())
                        }
                        Err(error) => {
                            let item = serde_json::json!({
                                "id": message_id,
                                "sender": format_uid(node.identity.public.awe_id.as_bytes()),
                                "recipient": format_uid(&recipient_id),
                                "text": text,
                                "state": "failed",
                                "timestamp": timestamp,
                                "error": error.to_string()
                            });
                            if let Ok(mut log) = messenger.lock() {
                                append_messenger_entry(&mut log, item.clone());
                            }
                            ("502 Bad Gateway", "application/json; charset=utf-8", serde_json::json!({"status":"failed","message":item}).to_string())
                        }
                    },
                    Err(error) => ("500 Internal Server Error", "application/json; charset=utf-8", serde_json::json!({"status":"error","error":error.to_string()}).to_string())
                }
                    }
                }
            }
        }
        },
        "/api/storage/put" if method == "POST" => {
            let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
            let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
            let filename = parsed.get("filename").and_then(|v| v.as_str()).unwrap_or("object.bin").trim();
            let data_hex = parsed.get("data_hex").and_then(|v| v.as_str()).unwrap_or("").trim();

            if filename.is_empty() || data_hex.is_empty() {
                ("400 Bad Request", "application/json; charset=utf-8",
                    serde_json::json!({"status":"error","error":"filename and data_hex are required"}).to_string())
            } else {
                match hex::decode(data_hex) {
                    Err(_) => ("400 Bad Request", "application/json; charset=utf-8",
                        serde_json::json!({"status":"error","error":"data_hex is not valid hexadecimal"}).to_string()),
                    Ok(data) => {
                        let runtime_policy = policy_state.lock().map(|p| p.clone()).unwrap_or_default();
                        if !runtime_policy.allows_upload(data.len()) || !runtime_policy.allows_stream(STORAGE_STREAM) {
                            ("403 Forbidden", "application/json; charset=utf-8", serde_json::json!({"status":"rejected","error":"upload rejected by local AWENET policy"}).to_string())
                        } else {
                        let policy = StoragePolicy::for_file_size(data.len());
                        let file_id = *blake3::hash(&data).as_bytes();
                        let peers = node.closest_peers(node.identity.public.awe_id.as_bytes(), 64).await;
                        let local_id = format_uid(node.identity.public.awe_id.as_bytes());
                        let mut node_ids = vec![local_id.clone()];
                        for peer in &peers {
                            let id = format_uid(&peer.awe_id);
                            if id != local_id && !node_ids.contains(&id) {
                                node_ids.push(id);
                            }
                        }

                        match awep2p_core::replication::build_plan_for_shards(file_id, &node_ids, policy.total_shards()) {
                            Err(error) => ("409 Conflict", "application/json; charset=utf-8",
                                serde_json::json!({"status":"error","error":error,"discovered_nodes":node_ids.len()}).to_string()),
                            Ok(plan) => match encode_shards(&data, &policy) {
                                Err(error) => ("500 Internal Server Error", "application/json; charset=utf-8",
                                    serde_json::json!({"status":"error","error":error.to_string()}).to_string()),
                                Ok(shards) => {
                                    let shard_hashes: Vec<String> = shards.iter()
                                        .map(|shard| hex::encode(blake3::hash(shard).as_bytes()))
                                        .collect();
                                    let mut peer_ids = std::collections::BTreeMap::<String, [u8; 32]>::new();
                                    for peer in peers {
                                        peer_ids.insert(format_uid(&peer.awe_id), peer.awe_id);
                                    }
                                    let mut stored_local = 0usize;
                                    let mut sent_remote = 0usize;
                                    let mut failed = Vec::new();
                                    let transfer_limit = Arc::new(tokio::sync::Semaphore::new(32));
                                    // A peer must have at most one first-connect/upload operation
                                    // in flight. Otherwise concurrent sends can all observe an empty
                                    // active-connection cache, race to create sockets, and replace one
                                    // another's cached connection before ACKs are returned.
                                    let peer_transfer_limits = peer_ids
                                        .values()
                                        .copied()
                                        .map(|peer_id| (peer_id, Arc::new(tokio::sync::Semaphore::new(1))))
                                        .collect::<std::collections::BTreeMap<_, _>>();
                                    let mut remote_jobs: tokio::task::JoinSet<Result<(usize, Option<serde_json::Value>), String>> = tokio::task::JoinSet::new();

                                    for (index, shard) in shards.into_iter().enumerate() {
                                        let placement = &plan.placements[index];
                                        let shard_hash = *blake3::hash(&shard).as_bytes();
                                        for target in &placement.nodes {
                                            if target == &local_id {
                                                match storage.put(&shard) {
                                                    Ok(_) => stored_local += 1,
                                                    Err(error) => failed.push(serde_json::json!({
                                                        "shard": index, "node": target, "error": error.to_string()
                                                    }))
                                                }
                                            } else if let Some(peer_id) = peer_ids.get(target).copied() {
                                                let request_id = blake3::hash(
                                                    format!("upload:{}:{}:{}:{}", hex::encode(file_id), index, target, now_unix()).as_bytes()
                                                ).as_bytes()[..16].try_into().unwrap_or([0u8; 16]);
                                                let transfer = StorageShardTransfer::new(
                                                    request_id,
                                                    *node.identity.public.awe_id.as_bytes(),
                                                    file_id,
                                                    index as u16,
                                                    policy.total_shards() as u16,
                                                    data.len() as u64,
                                                    shard.clone(),
                                                );
                                                let Ok(bytes) = serde_json::to_vec(&transfer) else {
                                                    failed.push(serde_json::json!({
                                                        "shard": index, "node": target, "error": "failed to serialize storage transfer"
                                                    }));
                                                    continue;
                                                };
                                                let task_node = node.clone();
                                                let task_limit = transfer_limit.clone();
                                                let Some(peer_limit) = peer_transfer_limits.get(&peer_id).cloned() else {
                                                    failed.push(serde_json::json!({
                                                        "shard": index,
                                                        "node": target,
                                                        "error": "missing per-peer storage transfer limit"
                                                    }));
                                                    continue;
                                                };
                                                let target_name = target.clone();
                                                remote_jobs.spawn(async move {
                                                    let _permit = task_limit.acquire_owned().await
                                                        .map_err(|_| "storage transfer scheduler closed".to_string())?;
                                                    let _peer_permit = peer_limit.acquire_owned().await
                                                        .map_err(|_| "per-peer storage transfer scheduler closed".to_string())?;
                                                    match task_node.send_to_peer_request(&peer_id, STORAGE_STREAM, bytes).await {
                                                        Ok((_, response)) => {
                                                            let ack = serde_json::from_slice::<StorageShardAck>(&response).ok();
                                                            let ack_received = ack.is_some();
                                                            let ack_identity_matches = ack_received; // response came over this authenticated peer session
                                                            let ack_payload_matches = ack.as_ref().map(|ack| {
                                                                ack.request_id == request_id
                                                                    && ack.file_id == file_id
                                                                    && ack.shard_index == index as u16
                                                                    && ack.stored_object_id == shard_hash
                                                            }).unwrap_or(false);
                                                            let confirmed = ack.as_ref().map(|ack| {
                                                                ack.request_id == request_id && storage_ack_matches(
                                                                    &peer_id, &peer_id, ack, &file_id, index as u16, &shard_hash
                                                                )
                                                            }).unwrap_or(false);
                                                            if confirmed {
                                                                Ok((1usize, None))
                                                            } else {
                                                                Ok((0usize, Some(serde_json::json!({
                                                                    "shard": index,
                                                                    "node": target_name,
                                                                    "error": "remote storage ACK response was invalid",
                                                                    "ack_received": ack_received,
                                                                    "ack_identity_matches": ack_identity_matches,
                                                                    "ack_payload_matches": ack_payload_matches
                                                                }))))
                                                            }
                                                        }
                                                        Err(error) => Ok((0usize, Some(serde_json::json!({
                                                            "shard": index, "node": target_name, "error": error.to_string()
                                                        }))))
                                                    }
                                                });
                                            } else {
                                                failed.push(serde_json::json!({
                                                    "shard": index, "node": target, "error": "peer is no longer known"
                                                }));
                                            }
                                        }
                                    }

                                    while let Some(result) = remote_jobs.join_next().await {
                                        match result {
                                            Ok(Ok((count, error))) => {
                                                sent_remote += count;
                                                if let Some(error) = error {
                                                    failed.push(error);
                                                }
                                            }
                                            Ok(Err(error)) => failed.push(serde_json::json!({
                                                "error": format!("storage transfer task failed: {error}")
                                            })),
                                            Err(error) => failed.push(serde_json::json!({
                                                "error": format!("storage transfer task panicked: {error}")
                                            })),
                                        }
                                    }

                                    let required_placements = plan
                                        .placements
                                        .iter()
                                        .map(|placement| placement.nodes.len())
                                        .sum::<usize>();
                                    let complete = storage_upload_is_complete(
                                        stored_local,
                                        sent_remote,
                                        required_placements,
                                        failed.len(),
                                    );
                                    let manifest_path = PathBuf::from(data_dir_for_api())
                                        .join("storage")
                                        .join("manifests");
                                    let manifest = serde_json::json!({
                                        "version": 1,
                                        "file_id": hex::encode(file_id),
                                        "filename": filename,
                                        "original_size": data.len(),
                                        "shards": policy.total_shards(),
                                        "data_shards": policy.data_shards,
                                        "parity_shards": policy.parity_shards,
                                        "replicas": 3,
                                        "shard_hashes": shard_hashes,
                                        "placements": plan.placements,
                                        "stored_local": stored_local,
                                        "sent_remote": sent_remote
                                    });
                                    let persist_result = if complete {
                                        fs::create_dir_all(&manifest_path).and_then(|()| {
                                            let bytes = serde_json::to_vec_pretty(&manifest)
                                                .map_err(std::io::Error::other)?;
                                            save_manifest(
                                                manifest_path.join(format!("{}.json", hex::encode(file_id))),
                                                &bytes,
                                            )
                                        })
                                    } else {
                                        Ok(())
                                    };

                                    if !complete {
                                        ("503 Service Unavailable", "application/json; charset=utf-8",
                                            serde_json::json!({
                                                "status": "partial",
                                                "error": "not all planned shard replicas were durably acknowledged; manifest was not committed",
                                                "file_id": hex::encode(file_id),
                                                "filename": filename,
                                                "original_size": data.len(),
                                                "required_placements": required_placements,
                                                "stored_local": stored_local,
                                                "sent_remote": sent_remote,
                                                "failures": failed
                                            }).to_string())
                                    } else if let Err(error) = persist_result {
                                        ("500 Internal Server Error", "application/json; charset=utf-8",
                                            serde_json::json!({
                                                "status": "error",
                                                "error": format!("all replicas stored but manifest persistence failed: {error}"),
                                                "file_id": hex::encode(file_id)
                                            }).to_string())
                                    } else {
                                        ("200 OK", "application/json; charset=utf-8",
                                            serde_json::json!({
                                                "status": "stored",
                                                "file_id": hex::encode(file_id),
                                                "filename": filename,
                                                "original_size": data.len(),
                                                "shards": policy.total_shards(),
                                                "replicas": 3,
                                                "stored_local": stored_local,
                                                "sent_remote": sent_remote,
                                                "failures": failed
                                            }).to_string())
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        },
        "/api/storage/push" if method == "POST" => {
            let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
            let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
            let recipient = parsed.get("recipient").and_then(|v| v.as_str()).unwrap_or("").trim();
            let file_id_hex = parsed.get("file_id").and_then(|v| v.as_str()).unwrap_or("").trim();
            let shard_index = parsed.get("shard_index").and_then(|v| v.as_u64()).unwrap_or(u64::MAX);
            let original_size = parsed.get("original_size").and_then(|v| v.as_u64()).unwrap_or(0);
            let payload_hex = parsed.get("payload_hex").and_then(|v| v.as_str()).unwrap_or("").trim();

            let recipient_id = hex::decode(recipient)
                .ok()
                .and_then(|b| <[u8; 32]>::try_from(b).ok())
                .or(None);
            let file_id = hex::decode(file_id_hex)
                .ok()
                .and_then(|b| <[u8; 32]>::try_from(b).ok());
            let payload = hex::decode(payload_hex).ok();

            match (recipient_id, file_id, payload, usize::try_from(shard_index)) {
                (Some(peer_id), Some(file_id), Some(payload), Ok(index)) if index < 1000 => {
                    let request_id = blake3::hash(
                        format!("{}:{}:{}", file_id_hex, shard_index, now_unix()).as_bytes()
                    ).as_bytes()[..16].try_into().unwrap_or([0u8; 16]);
                    let expected_hash = *blake3::hash(&payload).as_bytes();
                    let transfer = StorageShardTransfer::new(
                        request_id,
                        *node.identity.public.awe_id.as_bytes(),
                        file_id,
                        shard_index as u16,
                        1000,
                        original_size,
                        payload,
                    );
                    match serde_json::to_vec(&transfer) {
                        Ok(bytes) => match node.send_to_peer_request(&peer_id, STORAGE_STREAM, bytes).await {
                            Ok((rtt, response)) => match serde_json::from_slice::<StorageShardAck>(&response) {
                                Ok(ack) if ack.request_id == request_id
                                    && storage_ack_matches(&peer_id, &peer_id, &ack, &file_id, index as u16, &expected_hash) =>
                                    ("200 OK", "application/json; charset=utf-8",
                                        serde_json::json!({"status":"stored","stream":STORAGE_STREAM,"shard_index":index,"rtt_ms":rtt.as_millis()}).to_string()),
                                _ => ("502 Bad Gateway", "application/json; charset=utf-8",
                                    serde_json::json!({"status":"error","error":"remote storage ACK was invalid"}).to_string())
                            },
                            Err(error) => ("502 Bad Gateway", "application/json; charset=utf-8",
                                serde_json::json!({"status":"error","error":error.to_string()}).to_string())
                        },
                        Err(error) => ("500 Internal Server Error", "application/json; charset=utf-8",
                            serde_json::json!({"status":"error","error":error.to_string()}).to_string())
                    }
                }
                _ => ("400 Bad Request", "application/json; charset=utf-8",
                    serde_json::json!({"status":"error","error":"recipient, 32-byte file_id, shard_index 0..999 and payload_hex are required"}).to_string())
            }
        },
        "/api/storage/get" => {
            let query = request.lines().next().unwrap_or("");
            let file_id_hex = query.split_whitespace().nth(1)
                .and_then(|path| path.split('?').nth(1))
                .and_then(|q| q.split('&').find_map(|p| p.strip_prefix("file_id=")))
                .unwrap_or("");
            let manifest_path = match validate_storage_file_id(file_id_hex) {
                Ok(_) => PathBuf::from(data_dir_for_api())
                    .join("storage")
                    .join("manifests")
                    .join(format!("{file_id_hex}.json")),
                Err(error) => {
                    let response = ("400 Bad Request", "application/json; charset=utf-8",
                        serde_json::json!({"status":"error","error":error}).to_string());
                    stream.write_all(&http_response(response.0, response.1, &response.2, origin).await).await?;
                    stream.shutdown().await?;
                    return Ok(());
                }
            };
            match fs::read(&manifest_path) {
                Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
                    Ok(manifest) => {
                        let total_shards = manifest
                            .get("shards")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(1000)
                            .clamp(12, 1000) as usize;
                        let policy = StoragePolicy::custom_scaled(total_shards);
                        let mut shards = vec![None; total_shards];
                        let mut present = 0usize;
                        let mut missing = Vec::new();
                        if let Some(placements) = manifest.get("placements").and_then(|v| v.as_array()) {
                            for (index, placement) in placements.iter().enumerate().take(total_shards) {
                                let local = format_uid(node.identity.public.awe_id.as_bytes());
                                let owns = placement.get("nodes").and_then(|v| v.as_array())
                                    .map(|nodes| nodes.iter().any(|n| n.as_str() == Some(&local))).unwrap_or(false);
                                if owns {
                                    if let Ok(hash_bytes) = hex::decode(
                                        manifest.get("shard_hashes").and_then(|v| v.get(index))
                                            .and_then(|v| v.as_str()).unwrap_or("")
                                    ) {
                                        if let Ok(hash) = <[u8;32]>::try_from(hash_bytes) {
                                            if let Ok(data) = storage.get(&hash) {
                                                shards[index] = Some(data);
                                                present += 1;
                                                continue;
                                            }
                                        }
                                    }
                                }
                                missing.push(index);
                            }
                        }
                        if present < policy.data_shards && !missing.is_empty() {
                            let peers = node.peers().await;
                            let peer_ids = peers.into_iter()
                                .map(|p| (format_uid(&p.awe_id), p.awe_id))
                                .collect::<std::collections::BTreeMap<_, _>>();
                            let local_id = format_uid(node.identity.public.awe_id.as_bytes());

                            for index in missing.clone() {
                                if present >= policy.data_shards {
                                    break;
                                }
                                let Some(placement) = manifest.get("placements").and_then(|v| v.get(index)).and_then(|v| v.get("nodes")).and_then(|v| v.as_array()) else {
                                    continue;
                                };
                                let Some(expected_hex) = manifest.get("shard_hashes").and_then(|v| v.get(index)).and_then(|v| v.as_str()) else {
                                    continue;
                                };
                                let Ok(expected_bytes) = hex::decode(expected_hex) else {
                                    continue;
                                };
                                let Ok(expected_hash) = <[u8; 32]>::try_from(expected_bytes) else {
                                    continue;
                                };
                                let original_size = manifest
            .get("original_size")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

                                for target in placement.iter().filter_map(|v| v.as_str()) {
                                    if target == local_id {
                                        continue;
                                    }
                                    let Some(peer_id) = peer_ids.get(target) else {
                                        continue;
                                    };
                                    let request_id = blake3::hash(
                                        format!("download:{}:{}:{}:{}", file_id_hex, index, target, now_unix()).as_bytes()
                                    ).as_bytes()[..16].try_into().unwrap_or([0u8; 16]);
                                    let request = StorageShardRequest::new(
                                        request_id,
                                        *node.identity.public.awe_id.as_bytes(),
                                        <[u8;32]>::try_from(hex::decode(file_id_hex).unwrap_or_default()).unwrap_or([0;32]),
                                        index as u16,
                                        policy.total_shards() as u16,
                                        expected_hash,
                                        original_size,
                                        4 * 1024 * 1024,
                                    );
                                    let Ok(bytes) = serde_json::to_vec(&request) else {
                                        continue;
                                    };
                                    if let Ok((_, response_bytes)) = node
                                        .send_to_peer_request(peer_id, STORAGE_STREAM, bytes)
                                        .await
                                    {
                                        if let Ok(response) =
                                            serde_json::from_slice::<StorageShardTransfer>(&response_bytes)
                                        {
                                            if response.request_id == request_id
                                                && response.sender == *peer_id
                                                && response.file_id == request.file_id
                                                && response.shard_index == index as u16
                                                && response.verify().is_ok()
                                                && response.payload_hash == expected_hash
                                            {
                                                if let Ok(object_id) = storage.put(&response.payload) {
                                                    if object_id == expected_hash {
                                                        shards[index] = Some(response.payload);
                                                        present += 1;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    if shards[index].is_some() {
                                        break;
                                    }
                                }
                            }
                        }

                        if present < policy.data_shards {
                            ("409 Conflict","application/json; charset=utf-8",
                                serde_json::json!({"status":"insufficient_shards","present":present,"required":policy.data_shards,"missing":missing}).to_string())
                        } else {
                        match recover_shards(&mut shards, &policy) {
                            Ok(mut data) => {
                                let original_size = manifest.get("original_size").and_then(|v| v.as_u64()).unwrap_or(data.len() as u64) as usize;
                                data.truncate(original_size);
                                ("200 OK","application/json; charset=utf-8",
                                    serde_json::json!({
                                        "status":"reconstructed",
                                        "file_id":file_id_hex,
                                        "filename":manifest.get("filename").and_then(|v| v.as_str()).unwrap_or("object.bin"),
                                        "size":data.len(),
                                        "data_hex":hex::encode(data)
                                    }).to_string())
                            },
                            Err(error) => ("500 Internal Server Error","application/json; charset=utf-8",
                                serde_json::json!({"status":"error","error":error.to_string()}).to_string())
                        }
                        }
                    },
                    Err(error) => ("500 Internal Server Error","application/json; charset=utf-8",
                        serde_json::json!({"status":"error","error":error.to_string()}).to_string())
                },
                Err(_) => ("404 Not Found","application/json; charset=utf-8",
                    serde_json::json!({"status":"not_found","file_id":file_id_hex}).to_string())
            }
        },
        "/api/storage/health" => {
            let mut online: std::collections::BTreeSet<String> = node
                .closest_peers(node.identity.public.awe_id.as_bytes(), 4096)
                .await
                .into_iter()
                .map(|p| format_uid(&p.awe_id))
                .collect();
            online.insert(format_uid(node.identity.public.awe_id.as_bytes()));
            let manifest_dir = PathBuf::from(data_dir_for_api()).join("storage").join("manifests");
            let mut files = 0usize;
            let mut healthy = 0usize;
            let mut degraded = 0usize;
            if let Ok(entries) = fs::read_dir(&manifest_dir) {
                for entry in entries.flatten() {
                    if let Ok(bytes) = fs::read(entry.path()) {
                        if let Ok(m) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                            files += 1;
                            let mut ok = true;
                            if let Some(placements) = m.get("placements").and_then(|v| v.as_array()) {
                                for p in placements {
                                    let count = p.get("nodes").and_then(|v| v.as_array()).map(|ns|
                                        ns.iter().filter(|n| n.as_str().map(|s| online.contains(s)).unwrap_or(false)).count()
                                    ).unwrap_or(0);
                                    if count < 3 { ok = false; break; }
                                }
                            } else { ok = false; }
                            if ok { healthy += 1; } else { degraded += 1; }
                        }
                    }
                }
            }
            ("200 OK","application/json; charset=utf-8",
                serde_json::json!({"files":files,"healthy_files":healthy,"degraded_files":degraded,"online_nodes":online.len()}).to_string())
        },
        "/api/storage" => {
            let stats = storage.stats().unwrap_or_default();
            let free = awep2p_core::node::get_available_disk_space(PathBuf::from(data_dir_for_api()).as_path()).unwrap_or(0);
            ("200 OK", "application/json; charset=utf-8", serde_json::json!({
                "root": data_dir_for_api(),
                "free_bytes": free,
                "capacity_bytes": stats.capacity,
                "used_bytes": stats.used,
                "objects": stats.objects,
                "healthy_replicas": stats.healthy_replicas,
                "repaired_chunks": stats.repaired_chunks,
                "replication_policy": {"shards": 1000, "replicas": 3}
            }).to_string())
        },
        _ => ("404 Not Found", "text/plain; charset=utf-8", "Not Found".to_string()),
    };
    stream
        .write_all(&http_response(status, mime, &body, origin).await)
        .await?;
    stream.shutdown().await?;
    Ok(())
}

fn data_dir_for_api() -> String {
    if let Some(value) = env::var_os("AWE_DATA_DIR") {
        return PathBuf::from(value).display().to_string();
    }
    if let Some(home) = env::var_os("USERPROFILE").or_else(|| env::var_os("HOME")) {
        PathBuf::from(home).join(".awep2p").display().to_string()
    } else {
        PathBuf::from(".awep2p").display().to_string()
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

async fn autonomous_repair_cycle(
    node: &Node,
    storage: &LocalNodeStore,
    data_dir: &std::path::Path,
    policy: &NetworkPolicy,
) {
    if !policy.enabled || !policy.allows_stream(STORAGE_STREAM) {
        return;
    }

    let manifest_dir = data_dir.join("storage").join("manifests");
    let Ok(entries) = fs::read_dir(&manifest_dir) else {
        return;
    };

    let peers = node.peers().await;
    let local_id = format_uid(node.identity.public.awe_id.as_bytes());
    let mut peer_ids = std::collections::BTreeMap::<String, [u8; 32]>::new();
    for peer in peers {
        peer_ids.insert(format_uid(&peer.awe_id), peer.awe_id);
    }

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(mut manifest) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        let Some(shard_hashes) = manifest
            .get("shard_hashes")
            .and_then(|v| v.as_array())
            .cloned()
        else {
            continue;
        };
        let file_id_hex = manifest
            .get("file_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let Ok(file_id_bytes) = hex::decode(&file_id_hex) else {
            continue;
        };
        let Ok(file_id) = <[u8; 32]>::try_from(file_id_bytes) else {
            continue;
        };
        let original_size = manifest
            .get("original_size")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let total_shards = manifest.get("shards").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let mut manifest_changed = false;
        {
            let Some(placements) = manifest
                .get_mut("placements")
                .and_then(|v| v.as_array_mut())
            else {
                continue;
            };

            for index in 0..total_shards.min(1000) {
                let Some(nodes_value) = placements
                    .get_mut(index)
                    .and_then(|v| v.get_mut("nodes"))
                    .and_then(|v| v.as_array_mut())
                else {
                    continue;
                };
                let current_nodes = nodes_value
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect::<Vec<_>>();
                let available = current_nodes
                    .iter()
                    .filter(|id| *id == &local_id || peer_ids.contains_key(*id))
                    .count();
                if available >= 3 {
                    continue;
                }

                let Some(expected_hex) = shard_hashes.get(index).and_then(|v| v.as_str()) else {
                    continue;
                };
                let Ok(expected_bytes) = hex::decode(expected_hex) else {
                    continue;
                };
                let Ok(expected_hash) = <[u8; 32]>::try_from(expected_bytes) else {
                    continue;
                };

                let mut source_data: Option<Vec<u8>> = None;
                for source in &current_nodes {
                    if source == &local_id {
                        if let Ok(data) = storage.get(&expected_hash) {
                            source_data = Some(data);
                            break;
                        }
                    } else if let Some(source_id) = peer_ids.get(source) {
                        let request_id = blake3::hash(
                            format!(
                                "repair-source:{}:{}:{}:{}",
                                file_id_hex,
                                index,
                                source,
                                now_unix()
                            )
                            .as_bytes(),
                        )
                        .as_bytes()[..16]
                            .try_into()
                            .unwrap_or([0u8; 16]);
                        let request = StorageShardRequest::new(
                            request_id,
                            *node.identity.public.awe_id.as_bytes(),
                            file_id,
                            index as u16,
                            total_shards as u16,
                            expected_hash,
                            original_size,
                            4 * 1024 * 1024,
                        );
                        let Ok(request_bytes) = serde_json::to_vec(&request) else {
                            continue;
                        };
                        if let Ok((_, response_bytes)) = node
                            .send_to_peer_request(source_id, STORAGE_STREAM, request_bytes)
                            .await
                        {
                            if let Ok(response) =
                                serde_json::from_slice::<StorageShardTransfer>(&response_bytes)
                            {
                                if response.request_id == request_id
                                    && response.sender == *source_id
                                    && response.file_id == file_id
                                    && response.shard_index == index as u16
                                    && response.payload_hash == expected_hash
                                    && response.verify().is_ok()
                                {
                                    source_data = Some(response.payload);
                                }
                            }
                        }
                        if source_data.is_some() {
                            break;
                        }
                    }
                }

                let Some(data) = source_data else {
                    continue;
                };
                if !policy.allows_shard(data.len())
                    || *blake3::hash(&data).as_bytes() != expected_hash
                {
                    continue;
                }

                let mut repaired = false;
                let mut candidates = peer_ids
                    .keys()
                    .filter(|id| !current_nodes.contains(id))
                    .cloned()
                    .collect::<Vec<_>>();
                if !current_nodes.contains(&local_id) {
                    candidates.insert(0, local_id.clone());
                }
                candidates.sort();
                candidates.dedup();

                for target in candidates {
                    if current_nodes.contains(&target) {
                        continue;
                    }

                    if target == local_id {
                        if storage.put(&data).is_ok() {
                            nodes_value.push(serde_json::Value::String(target.clone()));
                            repaired = true;
                        }
                    } else if let Some(target_id) = peer_ids.get(&target) {
                        let request_id = blake3::hash(
                            format!(
                                "repair-target:{}:{}:{}:{}",
                                file_id_hex,
                                index,
                                target,
                                now_unix()
                            )
                            .as_bytes(),
                        )
                        .as_bytes()[..16]
                            .try_into()
                            .unwrap_or([0u8; 16]);
                        let transfer = StorageShardTransfer::new(
                            request_id,
                            *node.identity.public.awe_id.as_bytes(),
                            file_id,
                            index as u16,
                            total_shards as u16,
                            original_size,
                            data.clone(),
                        );
                        let Ok(transfer_bytes) = serde_json::to_vec(&transfer) else {
                            continue;
                        };
                        if let Ok((_, response_bytes)) = node
                            .send_to_peer_request(target_id, STORAGE_STREAM, transfer_bytes)
                            .await
                        {
                            if let Ok(ack) =
                                serde_json::from_slice::<StorageShardAck>(&response_bytes)
                            {
                                if ack.request_id == request_id
                                    && storage_ack_matches(
                                        target_id,
                                        target_id,
                                        &ack,
                                        &file_id,
                                        index as u16,
                                        &expected_hash,
                                    )
                                {
                                    nodes_value.push(serde_json::Value::String(target.clone()));
                                    repaired = true;
                                }
                            }
                        }
                    }

                    if repaired {
                        break;
                    }
                }

                if repaired {
                    manifest_changed = true;
                }
            }
        }

        if manifest_changed {
            let _ = fs::write(
                &path,
                serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
            );
        }
    }
}

async fn run_product() -> Result<()> {
    let data_dir = if let Some(value) = env::var_os("AWE_DATA_DIR") {
        PathBuf::from(value)
    } else if let Some(home) = env::var_os("USERPROFILE").or_else(|| env::var_os("HOME")) {
        PathBuf::from(home).join(".awep2p")
    } else {
        PathBuf::from(".awep2p")
    };
    fs::create_dir_all(&data_dir)?;
    let mut api_token_bytes = [0u8; 32];
    OsRng.fill_bytes(&mut api_token_bytes);
    let api_token = hex::encode(api_token_bytes);
    let secret_path = data_dir.join("node.awesecret");
    let identity = if secret_path.exists() {
        AweSecret::from_bytes(&fs::read(&secret_path)?)
            .map_err(anyhow::Error::msg)?
            .authenticate()
            .map_err(anyhow::Error::msg)?
    } else {
        let identity =
            Identity::generate(Username::new("awe-node".to_string()).map_err(anyhow::Error::msg)?);
        let secret = AweSecret::generate(&identity);
        fs::write(&secret_path, secret.to_bytes()?)?;
        identity
    };
    let node_id = format_uid(identity.public.awe_id.as_bytes());
    let listen: SocketAddr = env::var("AWE_LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:41000".into())
        .parse()
        .context("invalid AWE_LISTEN_ADDR")?;
    let node = Node::new(identity, listen);
    let storage_root = data_dir.join("storage");
    fs::create_dir_all(&storage_root)?;
    let storage_quota = awep2p_core::node::get_available_disk_space(&storage_root)
        .context("failed to determine available node storage capacity")?;
    if storage_quota == 0 {
        anyhow::bail!("node storage has no available capacity");
    }
    let storage: StorageState = Arc::new(LocalNodeStore::open(&storage_root, storage_quota)?);
    let messenger_path = data_dir.join("messenger.json");
    let messenger: MessengerLog = Arc::new(Mutex::new(
        fs::read(&messenger_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<serde_json::Value>>(&bytes).ok())
            .unwrap_or_default()
            .into_iter()
            .rev()
            .take(MAX_MESSENGER_LOG_ENTRIES)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect(),
    ));
    let messenger_save = messenger.clone();
    let messenger_save_path = messenger_path.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            if let Ok(log) = messenger_save.lock() {
                if let Ok(bytes) = serde_json::to_vec(&*log) {
                    let _ = fs::write(&messenger_save_path, bytes);
                }
            }
        }
    });
    let community_path = data_dir.join("community.json");
    let community: CommunityState = Arc::new(Mutex::new(
        fs::read(&community_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_else(|| serde_json::json!({"groups":[],"channels":[]})),
    ));
    let community_save = community.clone();
    let community_save_path = community_path.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            if let Ok(state) = community_save.lock() {
                if let Ok(bytes) = serde_json::to_vec_pretty(&*state) {
                    let _ = fs::write(&community_save_path, bytes);
                }
            }
        }
    });
    let pending_acks: PendingAcks = Arc::new(Mutex::new(BTreeMap::new()));
    let federation_path = data_dir.join("awenet.json");
    let policy_path = data_dir.join("policy.json");
    let initial_policy = policy::load_or_create(&policy_path).map_err(anyhow::Error::msg)?;
    let policy_state: PolicyState = Arc::new(Mutex::new(initial_policy));
    let federation_state: FederationState = if federation_path.exists() {
        fs::read(&federation_path)
            .ok()
            .and_then(|b| serde_json::from_slice::<AweNetConfig>(&b).ok())
            .map(|mut s| {
                s.local_node_id = node_id.clone();
                Arc::new(Mutex::new(s))
            })
            .unwrap_or_else(|| {
                Arc::new(Mutex::new(AweNetConfig {
                    format: "awenet".into(),
                    version: federation::FORMAT_VERSION,
                    local_node_id: node_id.clone(),
                    ..Default::default()
                }))
            })
    } else {
        Arc::new(Mutex::new(AweNetConfig {
            format: "awenet".into(),
            version: federation::FORMAT_VERSION,
            local_node_id: node_id.clone(),
            ..Default::default()
        }))
    };
    if let Ok(state) = federation_state.lock() {
        let _ = federation::save_json(&*state, &federation_path);
    }
    let policy_refresh = policy_state.clone();
    let policy_refresh_path = policy_path.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            let current = policy_refresh.lock().map(|p| p.clone()).unwrap_or_default();
            if let Ok(next) = policy::load_if_changed(&policy_refresh_path, &current) {
                if let Ok(mut p) = policy_refresh.lock() {
                    *p = next;
                }
            }
        }
    });
    let startup_bootstrap = federation_state
        .lock()
        .map(|s| s.bootstrap_endpoints.clone())
        .unwrap_or_default()
        .iter()
        .filter_map(|x| x.parse::<SocketAddr>().ok())
        .collect::<Vec<_>>();
    if !startup_bootstrap.is_empty() {
        let _ = node.bootstrap(&startup_bootstrap).await;
        let supervisor = PeerSupervisor::new(
            node.clone(),
            startup_bootstrap.clone(),
            SupervisorConfig::default(),
        )
        .map_err(anyhow::Error::msg)?;
        let _supervisor_task = supervisor.spawn();
    }
    let repair_node = node.clone();
    let repair_storage = storage.clone();
    let repair_policy = policy_state.clone();
    let repair_data_dir = data_dir.clone();
    tokio::spawn(async move {
        loop {
            let policy = repair_policy.lock().map(|p| p.clone()).unwrap_or_default();
            autonomous_repair_cycle(&repair_node, &repair_storage, &repair_data_dir, &policy).await;
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
    });

    let node_for_listener = node.clone();
    tokio::spawn(async move {
        if let Err(e) = node_for_listener.listen().await {
            eprintln!("AWE node stopped: {e}");
        }
    });

    let onecoin_path = data_dir.join("onecoin-ledger.json");
    let onecoin_ledger: OnecoinLedgerState = Arc::new(Mutex::new(
        fs::read(&onecoin_path)
            .ok()
            .and_then(|b| serde_json::from_slice::<OnecoinLedger>(&b).ok())
            .unwrap_or_default(),
    ));
    {
        let mut ledger = onecoin_ledger
            .lock()
            .map_err(|_| anyhow::anyhow!("ONECOIN ledger lock failed"))?;
        if ledger.members.is_empty() {
            ledger
                .initialize_genesis(std::slice::from_ref(&node.identity.public.awe_id))
                .map_err(anyhow::Error::msg)?;
            fs::write(&onecoin_path, serde_json::to_vec_pretty(&*ledger)?)?;
        }
    }
    let onecoin_offers_path = data_dir.join("onecoin-exchange-offers.json");
    let contribution_path = data_dir.join("resource-contribution.json");
    let contribution: ContributionState = Arc::new(Mutex::new(
        fs::read(&contribution_path)
            .ok()
            .and_then(|b| serde_json::from_slice::<ResourceContribution>(&b).ok())
            .unwrap_or_default(),
    ));

    let consensus_state: ConsensusState = {
        let configured = env::var("AWE_ONECOIN_VALIDATORS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if configured.is_empty() {
            Arc::new(Mutex::new(None))
        } else {
            let validators = build_onecoin_validators(&node, &configured).await;
            if validators.len()
                != configured
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
            {
                eprintln!(
                    "ONECOIN consensus waiting: not all configured validators are discovered"
                );
                Arc::new(Mutex::new(None))
            } else {
                let state_path = data_dir.join("onecoin-consensus.json");
                match OnecoinConsensusRuntime::open(&state_path, validators) {
                    Ok(mut runtime) => {
                        if runtime.state.state.ledger.members.is_empty() {
                            let members = runtime
                                .validators
                                .values()
                                .map(|key| AweId::from_public_key(key))
                                .collect::<Vec<_>>();
                            if !members.is_empty() {
                                if let Err(error) =
                                    runtime.state.state.ledger.initialize_genesis(&members)
                                {
                                    eprintln!("ONECOIN genesis initialization failed: {error}");
                                } else if let Err(error) = runtime.state.save() {
                                    eprintln!("ONECOIN genesis persistence failed: {error}");
                                }
                            }
                        }
                        Arc::new(Mutex::new(Some(runtime)))
                    }
                    Err(error) => {
                        eprintln!("ONECOIN consensus disabled: {error}");
                        Arc::new(Mutex::new(None))
                    }
                }
            }
        }
    };

    let dispatcher_node = node.clone();
    let dispatcher_storage = storage.clone();
    let dispatcher_messenger = messenger.clone();
    let dispatcher_acks = pending_acks.clone();
    let dispatcher_policy = policy_state.clone();
    let dispatcher_community = community.clone();
    let dispatcher_consensus = consensus_state.clone();
    let dispatcher_onecoin_ledger = onecoin_ledger.clone();
    let dispatcher_onecoin_path = onecoin_path.clone();
    tokio::spawn(async move {
        loop {
            for mut inbound in dispatcher_node.take_inbox_messages() {
                let sender = inbound.sender;
                let stream = inbound.stream;
                let payload = inbound.payload.clone();
                let runtime_policy = dispatcher_policy
                    .lock()
                    .map(|p| p.clone())
                    .unwrap_or_default();
                if !runtime_policy.allows_stream(stream) {
                    if stream == STORAGE_STREAM || stream == ONECOIN_CONSENSUS_STREAM {
                        eprintln!(
                            "application stream rejected by local policy: stream={}, allowed={:?}",
                            stream, runtime_policy.allowed_streams
                        );
                    }
                    continue;
                }
                if stream == ONECOIN_CONSENSUS_STREAM {
                    let result = {
                        let mut guard = dispatcher_consensus.lock().ok();
                        match guard.as_mut().and_then(|g| g.as_mut()) {
                            Some(runtime) => match OnecoinConsensusMessage::decode(&payload) {
                                Ok(message) => {
                                    runtime.handle(sender, &dispatcher_node.identity, message)
                                }
                                Err(error) => Err(error),
                            },
                            None => Ok(Vec::new()),
                        }
                    };
                    if let Ok(outbound) = result {
                        for (peer, message) in outbound {
                            if let Ok(bytes) = message.encode() {
                                let _ = dispatcher_node
                                    .send_to_peer(&peer, ONECOIN_CONSENSUS_STREAM, bytes)
                                    .await;
                            }
                        }
                    }
                    continue;
                }
                if stream == policy::ONECOIN_TRANSFER_STREAM {
                    // Once consensus is configured, legacy direct transfers must not mutate a
                    // separate non-finalized ledger behind the wallet's finalized balance.
                    if dispatcher_consensus
                        .lock()
                        .map(|state| state.is_some())
                        .unwrap_or(true)
                    {
                        continue;
                    }
                    if let Ok(tx) = serde_json::from_slice::<OnecoinTransaction>(&payload) {
                        if tx.recipient == *dispatcher_node.identity.public.awe_id.as_bytes() {
                            if let Ok(mut ledger) = dispatcher_onecoin_ledger.lock() {
                                let sender_id = AweId::from_public_key(&tx.sender);
                                ledger.ensure_member(&sender_id);
                                ledger.ensure_member(&dispatcher_node.identity.public.awe_id);
                                if ledger
                                    .receive_transfer(
                                        &tx,
                                        &tx.sender,
                                        &dispatcher_node.identity.public.awe_id,
                                    )
                                    .is_ok()
                                {
                                    let _ = fs::write(
                                        &dispatcher_onecoin_path,
                                        serde_json::to_vec_pretty(&*ledger).unwrap_or_default(),
                                    );
                                }
                            }
                        }
                    }
                    continue;
                }
                if stream == 100 {
                    if let Ok(message) = serde_json::from_slice::<serde_json::Value>(&payload) {
                        if message.get("kind").and_then(|v| v.as_str()) == Some("awe.call.v1") {
                            let id = message.get("id").and_then(|v| v.as_str()).unwrap_or("");
                            let recipient = message
                                .get("recipient")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let call_id = message
                                .get("call_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let signal_type = message
                                .get("signal_type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let data = message.get("data").and_then(|v| v.as_str()).unwrap_or("");
                            if !id.is_empty()
                                && !recipient.is_empty()
                                && !call_id.is_empty()
                                && !signal_type.is_empty()
                                && !data.is_empty()
                            {
                                if let Ok(mut log) = dispatcher_messenger.lock() {
                                    if !log.iter().any(|existing| {
                                        existing.get("id").and_then(|v| v.as_str()) == Some(id)
                                    }) {
                                        let mut item = message.clone();
                                        if let Some(obj) = item.as_object_mut() {
                                            obj.insert(
                                                "sender".into(),
                                                serde_json::json!(format_uid(&sender)),
                                            );
                                        }
                                        append_messenger_entry(&mut log, item);
                                    }
                                }
                            }
                            continue;
                        }
                        if message.get("kind").and_then(|v| v.as_str()) == Some("awe.group.v1") {
                            if let Ok(mut state) = dispatcher_community.lock() {
                                let event =
                                    message.get("event").and_then(|v| v.as_str()).unwrap_or("");
                                let sender_uid = hex::encode(sender);
                                if event == "upsert" {
                                    if let Some(group) = message.get("group").cloned() {
                                        let owner = group
                                            .get("owner")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        if owner == sender_uid
                                            && validate_remote_group(&group, &sender_uid).is_ok()
                                        {
                                            let gid = group
                                                .get("id")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("");
                                            if let Some(groups) = state
                                                .get_mut("groups")
                                                .and_then(|v| v.as_array_mut())
                                            {
                                                if let Some(existing) =
                                                    groups.iter_mut().find(|g| {
                                                        g.get("id").and_then(|v| v.as_str())
                                                            == Some(gid)
                                                    })
                                                {
                                                    *existing = group;
                                                } else {
                                                    groups.push(group);
                                                }
                                            }
                                        }
                                    }
                                } else if event == "message" {
                                    let gid = message
                                        .get("group_id")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");
                                    if let Some(g) = state
                                        .get_mut("groups")
                                        .and_then(|v| v.as_array_mut())
                                        .and_then(|a| {
                                            a.iter_mut().find(|g| {
                                                g.get("id").and_then(|v| v.as_str()) == Some(gid)
                                            })
                                        })
                                    {
                                        let member = g
                                            .get("members")
                                            .and_then(|v| v.as_array())
                                            .map(|a| {
                                                a.iter().any(|v| {
                                                    v.as_str() == Some(sender_uid.as_str())
                                                })
                                            })
                                            .unwrap_or(false);
                                        if member {
                                            if let Some(m) = message.get("message").cloned() {
                                                if validate_remote_feed_message(&m, &sender_uid)
                                                    .is_ok()
                                                {
                                                    if let Some(a) = g
                                                        .get_mut("messages")
                                                        .and_then(|v| v.as_array_mut())
                                                    {
                                                        append_bounded_json_entry(a, m);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            continue;
                        }
                        if message.get("kind").and_then(|v| v.as_str()) == Some("awe.channel.v1") {
                            let event = message.get("event").and_then(|v| v.as_str()).unwrap_or("");
                            let sender_uid = hex::encode(sender);
                            let mut sync: Option<(String, Vec<u8>)> = None;
                            if let Ok(mut state) = dispatcher_community.lock() {
                                if event == "upsert" {
                                    if let Some(channel) = message.get("channel").cloned() {
                                        let owner = channel
                                            .get("owner")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        if owner == sender_uid
                                            && validate_remote_channel(&channel, &sender_uid)
                                                .is_ok()
                                        {
                                            let cid = channel
                                                .get("id")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("");
                                            if let Some(chs) = state
                                                .get_mut("channels")
                                                .and_then(|v| v.as_array_mut())
                                            {
                                                if let Some(existing) = chs.iter_mut().find(|c| {
                                                    c.get("id").and_then(|v| v.as_str())
                                                        == Some(cid)
                                                }) {
                                                    *existing = channel;
                                                } else {
                                                    chs.push(channel);
                                                }
                                            }
                                        }
                                    }
                                } else if event == "subscribe" {
                                    let cid = message
                                        .get("channel_id")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");
                                    // The authenticated transport identity is authoritative; never trust a
                                    // payload-supplied subscriber ID for channel membership.
                                    let subscriber = sender_uid.as_str();
                                    if message
                                        .get("subscriber")
                                        .and_then(|v| v.as_str())
                                        .is_some_and(|claimed| claimed != subscriber)
                                    {
                                        continue;
                                    }
                                    let local_uid = hex::encode(
                                        dispatcher_node.identity.public.awe_id.as_bytes(),
                                    );
                                    if let Some(ch) = state
                                        .get_mut("channels")
                                        .and_then(|v| v.as_array_mut())
                                        .and_then(|a| {
                                            a.iter_mut().find(|c| {
                                                c.get("id").and_then(|v| v.as_str()) == Some(cid)
                                            })
                                        })
                                    {
                                        if ch.get("owner").and_then(|v| v.as_str())
                                            == Some(local_uid.as_str())
                                        {
                                            if !ch
                                                .get("subscribers")
                                                .and_then(|v| v.as_array())
                                                .map(|a| {
                                                    a.iter().any(|v| v.as_str() == Some(subscriber))
                                                })
                                                .unwrap_or(false)
                                            {
                                                if let Some(a) = ch
                                                    .get_mut("subscribers")
                                                    .and_then(|v| v.as_array_mut())
                                                {
                                                    a.push(serde_json::Value::String(
                                                        subscriber.to_string(),
                                                    ));
                                                }
                                            }
                                            let env = serde_json::json!({"kind":"awe.channel.v1","event":"upsert","channel":ch.clone(),"sender":local_uid});
                                            if let Ok(bytes) = serde_json::to_vec(&env) {
                                                sync = Some((subscriber.to_string(), bytes));
                                            }
                                        }
                                    }
                                } else if event == "message" {
                                    let cid = message
                                        .get("channel_id")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");
                                    if let Some(ch) = state
                                        .get_mut("channels")
                                        .and_then(|v| v.as_array_mut())
                                        .and_then(|a| {
                                            a.iter_mut().find(|c| {
                                                c.get("id").and_then(|v| v.as_str()) == Some(cid)
                                            })
                                        })
                                    {
                                        let subscriber = ch
                                            .get("subscribers")
                                            .and_then(|v| v.as_array())
                                            .map(|a| {
                                                a.iter().any(|v| {
                                                    v.as_str() == Some(sender_uid.as_str())
                                                })
                                            })
                                            .unwrap_or(false)
                                            || ch.get("owner").and_then(|v| v.as_str())
                                                == Some(sender_uid.as_str());
                                        if subscriber {
                                            if let Some(m) = message.get("message").cloned() {
                                                if validate_remote_feed_message(&m, &sender_uid)
                                                    .is_ok()
                                                {
                                                    if let Some(a) = ch
                                                        .get_mut("messages")
                                                        .and_then(|v| v.as_array_mut())
                                                    {
                                                        append_bounded_json_entry(a, m);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            if let Some((target_uid, bytes)) = sync {
                                let sync_node = dispatcher_node.clone();
                                tokio::spawn(async move {
                                    let peers = sync_node.active_peers().await;
                                    if let Some(target) =
                                        peers.into_iter().find(|p| hex::encode(*p) == target_uid)
                                    {
                                        let _ = sync_node.send_to_peer(&target, 100, bytes).await;
                                    }
                                });
                            }
                            continue;
                        }
                        if message.get("kind").and_then(|v| v.as_str()) == Some("awe.messenger.v1")
                        {
                            let id = message.get("id").and_then(|v| v.as_str()).unwrap_or("");
                            let text_value =
                                message.get("text").and_then(|v| v.as_str()).unwrap_or("");
                            let recipient = message
                                .get("recipient")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            if !id.is_empty() && !text_value.is_empty() && !recipient.is_empty() {
                                if let (Some(cid), Some(channel_message)) = (
                                    message.get("channel_id").and_then(|v| v.as_str()),
                                    message.get("channel_message").cloned(),
                                ) {
                                    if let Ok(mut state) = dispatcher_community.lock() {
                                        if let Some(ch) = state
                                            .get_mut("channels")
                                            .and_then(|v| v.as_array_mut())
                                            .and_then(|a| {
                                                a.iter_mut().find(|c| {
                                                    c.get("id").and_then(|v| v.as_str())
                                                        == Some(cid)
                                                })
                                            })
                                        {
                                            let sender_uid = format_uid(&sender);
                                            let authorized = ch
                                                .get("owner")
                                                .and_then(|v| v.as_str())
                                                == Some(sender_uid.as_str())
                                                || ch
                                                    .get("subscribers")
                                                    .and_then(|v| v.as_array())
                                                    .map(|a| {
                                                        a.iter().any(|v| {
                                                            v.as_str() == Some(sender_uid.as_str())
                                                        })
                                                    })
                                                    .unwrap_or(false);
                                            if authorized {
                                                if let Some(a) = ch
                                                    .get_mut("messages")
                                                    .and_then(|v| v.as_array_mut())
                                                {
                                                    a.push(channel_message);
                                                }
                                            }
                                        }
                                    }
                                }
                                if message.get("silent").and_then(|v| v.as_bool()) != Some(true) {
                                    let item = serde_json::json!({
                                        "id": id,
                                        "sender": format_uid(&sender),
                                        "recipient": recipient,
                                        "text": text_value,
                                        "state": "delivered",
                                        "timestamp": message.get("timestamp").and_then(|v| v.as_u64()).unwrap_or_else(now_unix)
                                    });
                                    if let Ok(mut log) = dispatcher_messenger.lock() {
                                        if !log.iter().any(|existing| {
                                            existing.get("id").and_then(|v| v.as_str()) == Some(id)
                                        }) {
                                            append_messenger_entry(&mut log, item);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    continue;
                }
                if stream != STORAGE_STREAM {
                    continue;
                }
                eprintln!(
                    "storage stream payload received: peer={}, bytes={}",
                    hex::encode(sender),
                    payload.len()
                );
                if let Ok(ack) = serde_json::from_slice::<StorageShardAck>(&payload) {
                    if ack.version == awep2p_core::data_plane::STORAGE_PROTOCOL_VERSION {
                        if let Ok(mut pending) = dispatcher_acks.lock() {
                            pending.insert(ack.request_id, (sender, ack));
                        }
                    }
                    continue;
                }

                if let Ok(request) = serde_json::from_slice::<StorageShardRequest>(&payload) {
                    if request.requester != sender || request.verify().is_err() {
                        continue;
                    }
                    let Ok(data) = dispatcher_storage.get(&request.expected_hash) else {
                        continue;
                    };
                    if data.len() > request.max_bytes as usize
                        || !runtime_policy.allows_shard(data.len())
                    {
                        continue;
                    }
                    let transfer = StorageShardTransfer::new(
                        request.request_id,
                        *dispatcher_node.identity.public.awe_id.as_bytes(),
                        request.file_id,
                        request.shard_index,
                        request.total_shards,
                        request.original_size,
                        data,
                    );
                    if transfer.payload_hash != request.expected_hash {
                        continue;
                    }
                    if let Ok(bytes) = serde_json::to_vec(&transfer) {
                        if inbound.expects_response() {
                            let _ = inbound.respond(bytes);
                        } else {
                            let _ = dispatcher_node
                                .send_to_peer(&sender, STORAGE_STREAM, bytes)
                                .await;
                        }
                    }
                    continue;
                }

                let Ok(transfer) = serde_json::from_slice::<StorageShardTransfer>(&payload) else {
                    continue;
                };
                if transfer.sender != sender {
                    eprintln!(
                        "storage transfer rejected: authenticated sender mismatch (peer={}, claimed={})",
                        hex::encode(sender),
                        hex::encode(transfer.sender)
                    );
                    continue;
                }
                if let Err(error) = transfer.verify() {
                    eprintln!("storage transfer rejected by validation: {error}");
                    continue;
                }
                if !runtime_policy.allows_shard(transfer.payload.len())
                    || transfer.original_size > runtime_policy.max_upload_bytes
                {
                    eprintln!(
                        "storage transfer rejected by local size policy: shard_bytes={}, original_bytes={}",
                        transfer.payload.len(),
                        transfer.original_size
                    );
                    continue;
                }
                let object_id = match dispatcher_storage.put(&transfer.payload) {
                    Ok(object_id) => object_id,
                    Err(error) => {
                        eprintln!("storage shard durable write failed: {error}");
                        continue;
                    }
                };
                let ack = StorageShardAck::new(
                    transfer.request_id,
                    transfer.file_id,
                    transfer.shard_index,
                    object_id,
                );
                match serde_json::to_vec(&ack) {
                    Ok(bytes) => {
                        if inbound.expects_response() {
                            if !inbound.respond(bytes) {
                                eprintln!("storage ACK response channel closed before reply");
                            }
                        } else if let Err(error) = dispatcher_node
                            .send_to_peer(&sender, STORAGE_STREAM, bytes)
                            .await
                        {
                            eprintln!("storage ACK send failed: {error}");
                        }
                    }
                    Err(error) => eprintln!("storage ACK serialization failed: {error}"),
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    });

    let ui_addr: SocketAddr = env::var("AWE_UI_ADDR")
        .unwrap_or_else(|_| DEFAULT_UI_ADDR.into())
        .parse()
        .context("invalid AWE_UI_ADDR")?;
    let listener = tokio::net::TcpListener::bind(ui_addr)
        .await
        .with_context(|| format!("cannot bind AWENET UI to {ui_addr}"))?;
    println!("AWENET is running.");
    println!("Node: {node_id}");
    println!("Node transport: {listen}");
    println!("Native UI: AWENET desktop window");

    let native_ui_addr = ui_addr;
    let api_token_for_server = api_token.clone();
    tokio::spawn(async move {
        loop {
            let (mut stream, peer_addr) = match listener.accept().await {
                Ok(value) => value,
                Err(error) => {
                    eprintln!("UI listener error: {error}");
                    continue;
                }
            };
            if !peer_addr.ip().is_loopback() {
                let response = http_response(
                    "403 Forbidden",
                    "application/json; charset=utf-8",
                    r#"{"status":"error","error":"local UI API accepts loopback clients only"}"#,
                    None,
                )
                .await;
                let _ = stream.write_all(&response).await;
                let _ = stream.shutdown().await;
                continue;
            }
            let api_token = api_token_for_server.clone();
            let api_node = node.clone();
            let api_messenger = messenger.clone();
            let api_federation = federation_state.clone();
            let api_storage = storage.clone();
            let api_federation_path = federation_path.clone();
            let api_policy = policy_state.clone();
            let api_community = community.clone();
            let api_consensus_state = consensus_state.clone();
            let api_onecoin_ledger = onecoin_ledger.clone();
            let api_onecoin_path = onecoin_path.clone();
            let api_onecoin_offers_path = onecoin_offers_path.clone();
            let api_contribution = contribution.clone();
            let api_contribution_path = contribution_path.clone();
            tokio::spawn(async move {
                if let Err(e) = serve_ui(
                    stream,
                    UiState {
                        node: api_node,
                        api_token,
                        messenger: api_messenger,
                        federation_state: api_federation,
                        federation_path: api_federation_path,
                        storage: api_storage,
                        policy_state: api_policy,
                        community: api_community,
                        consensus_state: api_consensus_state,
                        onecoin_ledger: api_onecoin_ledger,
                        onecoin_path: api_onecoin_path,
                        onecoin_offers_path: api_onecoin_offers_path,
                        contribution: api_contribution,
                        contribution_path: api_contribution_path,
                    },
                )
                .await
                {
                    eprintln!("UI request error: {e}");
                }
            });
        }
    });

    if env::var_os("AWE_NO_NATIVE_UI").is_some() {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    }

    native_ui::run(native_ui_addr, api_token).map_err(anyhow::Error::msg)?;
    Ok(())
}

async fn run_node(
    path: PathBuf,
    password: String,
    username: String,
    listen: SocketAddr,
    bootstrap: Vec<SocketAddr>,
) -> Result<()> {
    let identity = load_identity(&path, &password, &username)?;
    let node = Node::new(identity, listen);
    if !bootstrap.is_empty() {
        node.bootstrap(&bootstrap)
            .await
            .context("bootstrap failed")?;
        // Discovery is a continuous control-plane function, not a startup step.
        // Keep multiple seeds and learned peers alive so partitions can heal.
        node.spawn_discovery_loop(bootstrap.clone());
    }
    node.listen().await.map_err(anyhow::Error::msg)
}

fn print_id(path: PathBuf, password: String, username: String) -> Result<()> {
    println!(
        "{}",
        load_identity(&path, &password, &username)?
            .public
            .awe_id
            .to_hex()
    );
    Ok(())
}
fn print_status(path: PathBuf) -> Result<()> {
    println!(
        "{}",
        if path.exists() {
            format!("Vault exists at {}", path.display())
        } else {
            format!("Vault not found at {}", path.display())
        }
    );
    Ok(())
}
fn print_diagnostics() -> Result<()> {
    let mut d = NodeDiagnostics::new();
    d.update_metrics(NodeMetrics::default());
    println!("Status: {:?}", d.status());
    println!("Metrics: {:?}", d.metrics());
    Ok(())
}
fn run_mesh(port: u16) -> Result<()> {
    let addr: SocketAddr = format!("0.0.0.0:{port}").parse()?;
    let beacon = LanPeerBeacon::new([1u8; 32], addr, false);
    let bytes = beacon.encode().map_err(anyhow::Error::msg)?;
    println!("Broadcast beacon bytes: {}", bytes.len());
    println!(
        "Decoded: {:?}",
        LanPeerBeacon::decode(&bytes)
            .map_err(anyhow::Error::msg)?
            .node_id
    );
    Ok(())
}
fn print_health() -> Result<()> {
    println!(
        "Initial reputation score: {}",
        NodeReputation::new([1u8; 32]).score()
    );
    println!("Health: ONLINE");
    Ok(())
}
async fn probe(address: SocketAddr) -> Result<()> {
    let node = Node::new(
        Identity::generate(Username::new("probe-node").map_err(anyhow::Error::msg)?),
        "127.0.0.1:0".parse()?,
    );
    let mut c = node.connect(address).await.map_err(anyhow::Error::msg)?;
    println!("Authenticated peer: {:?}", c.remote_id);
    println!(
        "Heartbeat: {:?}",
        c.ping_roundtrip(1).await.map_err(anyhow::Error::msg)?
    );
    println!(
        "Encrypted data-plane: {:?}",
        c.send_data_roundtrip(7, b"AWEP2P-REAL-DATA-PROBE-v1".to_vec())
            .await
            .map_err(anyhow::Error::msg)?
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        None | Some("app") | Some("gui") => run_product().await,
        Some("secret") => {
            let username = args.next().unwrap_or_else(|| usage());
            generate_secret_file(&username, args.next().map(PathBuf::from))
        }
        Some("init") => {
            let username = args.next().unwrap_or_else(|| usage());
            init(
                &username,
                args.next().map(PathBuf::from).unwrap_or_else(default_vault),
            )
        }
        Some("run") => {
            let path = args.next().map(PathBuf::from).unwrap_or_else(|| usage());
            let password = args.next().unwrap_or_else(|| usage());
            let listen = args
                .next()
                .unwrap_or_else(|| usage())
                .parse()
                .context("invalid listen address")?;
            let username = env::var("AWE_USERNAME").unwrap_or_else(|_| "node".to_string());
            let bootstrap = args
                .map(|x| x.parse().context("invalid bootstrap address"))
                .collect::<Result<Vec<SocketAddr>>>()?;
            run_node(path, password, username, listen, bootstrap).await
        }
        Some("id") => print_id(
            args.next().map(PathBuf::from).unwrap_or_else(|| usage()),
            args.next().unwrap_or_else(|| usage()),
            args.next().unwrap_or_else(|| usage()),
        ),
        Some("status") => {
            print_status(args.next().map(PathBuf::from).unwrap_or_else(default_vault))
        }
        Some("diagnostics") => print_diagnostics(),
        Some("mesh") => run_mesh(args.next().unwrap_or_else(|| "41000".into()).parse()?),
        Some("health") => print_health(),
        Some("probe") => {
            probe(
                args.next()
                    .unwrap_or_else(|| usage())
                    .parse()
                    .context("invalid peer address")?,
            )
            .await
        }
        _ => usage(),
    }
}

#[cfg(test)]
mod phase_two_backend_tests {
    use super::{
        append_bounded_json_entry, append_messenger_entry, format_uid, is_allowed_ui_origin,
        is_valid_api_session, json_u16_field, json_u32_field, leave_federation_membership,
        parse_content_length, parse_onecoin_atoms, sign_federation_invitation, storage_ack_matches,
        storage_upload_is_complete, ui_html_with_session_token, validate_community_message,
        validate_community_title, validate_group_member_ids, validate_remote_channel,
        validate_remote_feed_message, validate_remote_group, validate_storage_file_id,
        verify_signed_federation_invitation, AweNetConfig, StorageShardAck, ATOMS_PER_COIN,
        MAX_COMMUNITY_MESSAGES_PER_FEED, MAX_COMMUNITY_MESSAGE_BYTES, MAX_COMMUNITY_TITLE_BYTES,
        MAX_GROUP_MEMBERS, MAX_MESSENGER_LOG_ENTRIES,
    };
    use serde_json::json;

    #[test]
    fn configured_onecoin_consensus_never_falls_back_to_local_ledger() {
        assert!(!onecoin_consensus_configured(""));
        assert!(!onecoin_consensus_configured(" ,  , "));
        assert!(onecoin_consensus_configured("validator-a"));
        assert!(onecoin_consensus_configured("validator-a, validator-b"));
    }

    #[test]
    fn parses_coin_amounts_using_full_18_decimal_precision() {
        assert_eq!(parse_onecoin_atoms(&json!("1")).unwrap(), ATOMS_PER_COIN);
        assert_eq!(
            parse_onecoin_atoms(&json!("1.5")).unwrap(),
            ATOMS_PER_COIN + ATOMS_PER_COIN / 2
        );
        assert_eq!(
            parse_onecoin_atoms(&json!("0.000000000000000001")).unwrap(),
            1
        );
        assert_eq!(
            parse_onecoin_atoms(&json!("0.00000001")).unwrap(),
            ATOMS_PER_COIN / 100_000_000
        );
    }

    #[test]
    fn rejects_coin_amounts_with_more_than_18_decimals_or_exponent_notation() {
        assert!(parse_onecoin_atoms(&json!("0.0000000000000000001")).is_err());
        assert!(parse_onecoin_atoms(&json!("1e-3")).is_err());
    }

    #[test]
    fn rejects_ambiguous_or_unsupported_http_framing() {
        assert_eq!(
            parse_content_length("POST / HTTP/1.1\r\nContent-Length: 12\r\n\r\n").unwrap(),
            12
        );
        assert!(parse_content_length(
            "POST / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n"
        )
        .is_err());
        assert!(
            parse_content_length("POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n").is_err()
        );
        assert!(parse_content_length("POST / HTTP/1.1\r\nContent-Length: -1\r\n\r\n").is_err());
    }

    #[test]
    fn api_session_token_is_required_and_exact() {
        assert!(!is_valid_api_session("secret", None));
        assert!(!is_valid_api_session("secret", Some("wrong")));
        assert!(!is_valid_api_session("secret", Some("secret ")));
        assert!(is_valid_api_session("secret", Some("secret")));
    }

    #[test]
    fn signed_federation_invitation_rejects_tampering() {
        let identity = awep2p_core::identity::Identity::generate(
            awep2p_core::identity::Username::new("signed-invitation-test").unwrap(),
        );
        let owner_node_id = format_uid(identity.public.awe_id.as_bytes());
        let payload = serde_json::json!({"node_id": owner_node_id});
        let signed = sign_federation_invitation("awenode", payload.clone(), &identity).unwrap();
        let (verified, signer) = verify_signed_federation_invitation(&signed, "awenode").unwrap();
        assert_eq!(verified, payload);
        assert_eq!(signer, owner_node_id);

        let mut tampered: serde_json::Value = serde_json::from_str(&signed).unwrap();
        tampered["payload"]["node_id"] = serde_json::Value::String("attacker-node".into());
        let tampered = serde_json::to_string(&tampered).unwrap();
        assert!(verify_signed_federation_invitation(&tampered, "awenode").is_err());
    }

    #[test]
    fn federation_leave_removes_membership_and_clears_local_owner_id() {
        let mut state = AweNetConfig {
            local_node_id: "node-a".into(),
            local_data_centre_id: Some("dc-a".into()),
            local_data_group_id: Some("dg-a".into()),
            local_centre_group_id: Some("cg-a".into()),
            joined_data_centres: vec!["dc-a".into(), "dc-b".into()],
            joined_data_groups: vec!["dg-a".into()],
            joined_centre_groups: vec!["cg-a".into()],
            ..AweNetConfig::default()
        };
        leave_federation_membership(&mut state, "cg", "cg-a", "node-a").unwrap();
        leave_federation_membership(&mut state, "dg", "dg-a", "node-a").unwrap();
        leave_federation_membership(&mut state, "dc", "dc-a", "node-a").unwrap();
        assert_eq!(state.joined_data_centres, vec!["dc-b"]);
        assert!(state.joined_data_groups.is_empty());
        assert!(state.joined_centre_groups.is_empty());
        assert_eq!(state.local_data_centre_id, None);
        assert_eq!(state.local_data_group_id, None);
        assert_eq!(state.local_centre_group_id, None);
    }

    #[test]
    fn federation_leave_rejects_unknown_membership_and_other_node_state() {
        let mut state = AweNetConfig {
            local_node_id: "node-a".into(),
            joined_data_centres: vec!["dc-a".into()],
            ..AweNetConfig::default()
        };
        assert!(leave_federation_membership(&mut state, "dc", "dc-missing", "node-a").is_err());
        assert!(leave_federation_membership(&mut state, "dc", "dc-a", "node-b").is_err());
        assert_eq!(state.joined_data_centres, vec!["dc-a"]);
    }

    #[test]
    fn ui_html_injects_session_token_and_fetch_header_wrapper() {
        let html = ui_html_with_session_token("test-session-token");
        assert!(html.contains("content=\"test-session-token\""));
        assert!(html.contains("X-AWENET-Session"));
    }

    #[test]
    fn allows_only_known_local_desktop_origins() {
        for origin in [
            "tauri://localhost",
            "http://tauri.localhost",
            "https://tauri.localhost",
            "http://127.0.0.1:41800",
            "http://localhost:41800",
        ] {
            assert!(
                is_allowed_ui_origin(origin),
                "expected allowed origin: {origin}"
            );
        }
    }

    #[test]
    fn rejects_untrusted_web_origins_and_null_origin() {
        for origin in [
            "https://evil.example",
            "http://localhost.evil.example:41800",
            "null",
            "*",
        ] {
            assert!(
                !is_allowed_ui_origin(origin),
                "expected rejected origin: {origin}"
            );
        }
    }

    #[test]
    fn remote_group_requires_authenticated_owner_and_valid_members() {
        let owner = "ab".repeat(32);
        let member = "cd".repeat(32);
        let group = json!({
            "id":"gid-1","title":"AWE group","owner":owner,
            "members":[owner,member],"messages":[]
        });
        assert!(validate_remote_group(&group, &owner).is_ok());
        assert!(validate_remote_group(&group, &member).is_err());
        let mut malformed = group.clone();
        malformed["members"] = json!(["uid-not-an-aweid"]);
        assert!(validate_remote_group(&malformed, &owner).is_err());
    }

    #[test]
    fn community_message_must_match_authenticated_sender_and_limits() {
        let sender = "ab".repeat(32);
        let message = json!({"id":"msg-1","sender":sender,"text":"hello","timestamp":1});
        assert!(validate_remote_feed_message(&message, &sender).is_ok());
        assert!(validate_remote_feed_message(&message, &"cd".repeat(32)).is_err());
        let mut malformed = message.clone();
        malformed["text"] = json!("x".repeat(MAX_COMMUNITY_MESSAGE_BYTES + 1));
        assert!(validate_remote_feed_message(&malformed, &sender).is_err());
    }

    #[test]
    fn remote_channel_requires_authenticated_owner_and_valid_subscribers() {
        let owner = "ab".repeat(32);
        let channel = json!({
            "id":"cid-1","title":"AWE channel","owner":owner,
            "subscribers":[owner],"messages":[]
        });
        assert!(validate_remote_channel(&channel, &owner).is_ok());
        assert!(validate_remote_channel(&channel, &"cd".repeat(32)).is_err());
        let mut malformed = channel.clone();
        malformed["subscribers"] = json!(["uid-not-an-aweid"]);
        assert!(validate_remote_channel(&malformed, &owner).is_err());
    }

    #[test]
    fn community_titles_and_messages_have_explicit_size_limits() {
        assert!(validate_community_title("AWE").is_ok());
        assert!(validate_community_title("  ").is_err());
        assert!(validate_community_title(&"x".repeat(MAX_COMMUNITY_TITLE_BYTES)).is_ok());
        assert!(validate_community_title(&"x".repeat(MAX_COMMUNITY_TITLE_BYTES + 1)).is_err());
        assert!(validate_community_message("hello").is_ok());
        assert!(validate_community_message(" ").is_err());
        assert!(validate_community_message(&"x".repeat(MAX_COMMUNITY_MESSAGE_BYTES)).is_ok());
        assert!(validate_community_message(&"x".repeat(MAX_COMMUNITY_MESSAGE_BYTES + 1)).is_err());
    }

    #[test]
    fn group_members_must_be_bounded_valid_aweids() {
        assert!(validate_group_member_ids(&[]).is_ok());
        assert!(validate_group_member_ids(&["ab".repeat(32)]).is_ok());
        assert!(validate_group_member_ids(&["uid-not-an-aweid".to_string()]).is_err());
        assert!(validate_group_member_ids(&vec!["ab".repeat(32); MAX_GROUP_MEMBERS + 1]).is_err());
    }

    #[test]
    fn community_feed_history_is_bounded() {
        let mut entries = Vec::new();
        for index in 0..=MAX_COMMUNITY_MESSAGES_PER_FEED {
            append_bounded_json_entry(&mut entries, json!({"index": index}));
        }
        assert!(entries.len() <= MAX_COMMUNITY_MESSAGES_PER_FEED);
        assert_eq!(
            entries
                .last()
                .and_then(|entry| entry.get("index"))
                .and_then(|v| v.as_u64()),
            Some(MAX_COMMUNITY_MESSAGES_PER_FEED as u64)
        );
        assert_eq!(
            entries
                .first()
                .and_then(|entry| entry.get("index"))
                .and_then(|v| v.as_u64()),
            Some((MAX_COMMUNITY_MESSAGES_PER_FEED / 10) as u64)
        );
    }

    #[test]
    fn messenger_history_is_bounded_and_keeps_recent_entries() {
        let mut log = Vec::new();
        for index in 0..=MAX_MESSENGER_LOG_ENTRIES {
            append_messenger_entry(&mut log, json!({"index": index}));
        }
        assert!(log.len() <= MAX_MESSENGER_LOG_ENTRIES);
        assert_eq!(
            log.last()
                .and_then(|entry| entry.get("index"))
                .and_then(|v| v.as_u64()),
            Some(MAX_MESSENGER_LOG_ENTRIES as u64)
        );
        assert_eq!(
            log.first()
                .and_then(|entry| entry.get("index"))
                .and_then(|v| v.as_u64()),
            Some((MAX_MESSENGER_LOG_ENTRIES / 10) as u64)
        );
    }

    #[test]
    fn storage_manifest_requires_all_durable_placements() {
        assert!(storage_upload_is_complete(1, 2, 3, 0));
        assert!(!storage_upload_is_complete(1, 1, 3, 0));
        assert!(!storage_upload_is_complete(1, 2, 3, 1));
        assert!(!storage_upload_is_complete(usize::MAX, 1, 3, 0));
    }

    #[test]
    fn storage_ack_must_match_the_authenticated_target() {
        let expected_sender = [7u8; 32];
        let attacker = [8u8; 32];
        let file_id = [9u8; 32];
        let hash = [10u8; 32];
        let ack = StorageShardAck::new([1u8; 16], file_id, 4, hash);
        assert!(storage_ack_matches(
            &expected_sender,
            &expected_sender,
            &ack,
            &file_id,
            4,
            &hash
        ));
        assert!(!storage_ack_matches(
            &attacker,
            &expected_sender,
            &ack,
            &file_id,
            4,
            &hash
        ));
        assert!(!storage_ack_matches(
            &expected_sender,
            &expected_sender,
            &ack,
            &file_id,
            5,
            &hash
        ));
    }

    #[test]
    fn storage_object_ids_must_be_exactly_32_bytes_of_hex() {
        let valid = "ab".repeat(32);
        assert!(validate_storage_file_id(&valid).is_ok());
        assert!(validate_storage_file_id("../etc/passwd").is_err());
        assert!(validate_storage_file_id(&"a".repeat(63)).is_err());
        assert!(validate_storage_file_id(&"g".repeat(64)).is_err());
    }

    #[test]
    fn rejects_overflowing_resource_contribution_fields() {
        assert!(json_u32_field(&json!({"cpu_cores": 4_294_967_296u64}), "cpu_cores").is_err());
        assert!(json_u16_field(&json!({"online_hours": 65_536}), "online_hours").is_err());
        assert!(json_u32_field(&json!({"node_count": -1}), "node_count").is_err());
    }
}
