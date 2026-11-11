mod economy_api;
mod native_ui;
use anyhow::{Context, Result};
use awep2p_core::data_plane::{
    StorageShardAck, StorageShardRequest, StorageShardTransfer, STORAGE_STREAM,
};
use awep2p_core::diagnostics::{NodeDiagnostics, NodeMetrics};
use awep2p_core::federation::{
    self, AweNetConfig, AweNodeConfig, DataCentreConfig, DataGroupConfig,
};
use awep2p_core::founder_guard::FounderGuard;
use awep2p_core::identity::{AweId, AweSecret, Identity, LocalVault, Username};
use awep2p_core::lan_mesh::LanPeerBeacon;
use awep2p_core::messenger::format_uid;
use awep2p_core::network::{format_node_descriptor, Node};
use awep2p_core::network_control::{self, NetworkAction, NetworkControlError, NetworkControlPlane};
use awep2p_core::onebank::{classify_tier, ResourceContribution};
use awep2p_core::onecoin::{OnecoinLedger, OnecoinTransaction, ATOMS_PER_COIN};
use awep2p_core::onecoin_consensus_runtime::{
    OnecoinConsensusMessage, OnecoinConsensusRuntime, ONECOIN_CONSENSUS_STREAM,
};
use awep2p_core::passkey_store;
use awep2p_core::policy::{self, NetworkPolicy};
use awep2p_core::reputation::NodeReputation;
use awep2p_core::storage::{encode_shards, recover_shards, LocalNodeStore, StoragePolicy};
use awep2p_core::store::{AppCapability, Store};
use awep2p_core::supervisor::{PeerSupervisor, SupervisorConfig};
use awep2p_core::user_registry::{
    self, UserAccount, UserRegistry, UserRegistryError, UserRole, UserStatus,
};
use awep2p_core::verifier_net::{self as verifier_net_store, VerifierNet};
use awep2p_core::verifier_transport::{
    self as verifier_transport, VerifierSide, VERIFIER_TRANSPORT_STREAM,
};
use std::{
    collections::BTreeMap,
    env, fs,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const UI_HTML: &str = include_str!("../../awe-desktop/ui/index.html");
const UI_CSS: &str = include_str!("../../awe-desktop/ui/style.css");
const UI_JS: &str = include_str!("../../awe-desktop/ui/app.js");
const ONECOIN_JS: &str = include_str!("../../awe-desktop/ui/onecoin.js");
const ONECOIN_CSS: &str = include_str!("../../awe-desktop/ui/onecoin.css");
const ECONOMY_CSS: &str = include_str!("../../awe-desktop/ui/economy.css");
const AWENETWORK_JS: &str = include_str!("../../awe-desktop/ui/awenetwork.js");
const STOREFRONT_JS: &str = include_str!("../../awe-desktop/ui/storefront.js");
const FOUNDER_JS: &str = include_str!("../../awe-desktop/ui/founder.js");
const QR_JS: &str = include_str!("../../awe-desktop/ui/vendor/qrcode.js");
const I18N_JS: &str = include_str!("../../awe-desktop/ui/i18n.js");
const DEFAULT_UI_ADDR: &str = "127.0.0.1:41800";

type MessengerLog = Arc<Mutex<Vec<serde_json::Value>>>;
type FederationState = Arc<Mutex<AweNetConfig>>;
type StorageState = Arc<LocalNodeStore>;
type PendingAcks = Arc<Mutex<BTreeMap<[u8; 16], StorageShardAck>>>;
type PendingShards = Arc<Mutex<BTreeMap<[u8; 16], StorageShardTransfer>>>;
type PolicyState = Arc<Mutex<NetworkPolicy>>;

/// Read runtime policy without silently replacing a poisoned policy with permissive defaults.
/// A poisoned policy lock disables policy-controlled operations until restart/recovery.
fn current_policy(state: &PolicyState) -> NetworkPolicy {
    match state.lock() {
        Ok(policy) => policy.clone(),
        Err(_) => NetworkPolicy {
            enabled: false,
            ..NetworkPolicy::default()
        },
    }
}

type CommunityState = Arc<Mutex<serde_json::Value>>;
type ConsensusState = Arc<Mutex<Option<OnecoinConsensusRuntime>>>;
type OnecoinLedgerState = Arc<Mutex<OnecoinLedger>>;
type ContributionState = Arc<Mutex<ResourceContribution>>;
type FounderGuardState = Arc<Mutex<FounderGuard>>;
type UserRegistryState = Arc<Mutex<UserRegistry>>;
type NetworkControlState = Arc<Mutex<NetworkControlPlane>>;

#[derive(Clone)]
struct UiState {
    node: Node,
    messenger: MessengerLog,
    federation_state: FederationState,
    federation_path: PathBuf,
    storage: StorageState,
    pending_acks: PendingAcks,
    pending_shards: PendingShards,
    policy_state: PolicyState,
    community: CommunityState,
    onecoin_ledger: OnecoinLedgerState,
    onecoin_path: PathBuf,
    onecoin_offers_path: PathBuf,
    contribution: ContributionState,
    contribution_path: PathBuf,
    founder_guard: FounderGuardState,
    user_registry: UserRegistryState,
    user_registry_path: PathBuf,
    network_control: NetworkControlState,
    network_control_path: PathBuf,
    economy: economy_api::EconomyState,
    economy_path: PathBuf,
    verifier_net: economy_api::VerifierNetState,
    verifier_net_path: PathBuf,
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
    eprintln!("AWEp2P\n\nUsage:\n  awe-node                 Start the complete local product\n  awe-node app             Start UI + local node\n  awe-node secret <username> [out-file]\n  awe-node init <username> [vault-file]\n  awe-node run <vault-file> <password> <listen-addr> [bootstrap-addr ...]\n  awe-node id <vault-file> <password> <username>\n  awe-node status [vault-file]\n  awe-node diagnostics\n  awe-node mesh <listen-port>\n  awe-node health\n  awe-node probe <address>");
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

fn http_response(status: &str, content_type: &str, body: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nX-Content-Type-Options: nosniff\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes()
}

async fn write_json_error(
    stream: &mut tokio::net::TcpStream,
    status: &str,
    message: &str,
) -> Result<()> {
    let body = serde_json::json!({"status":"error","error":message}).to_string();
    stream
        .write_all(&http_response(
            status,
            "application/json; charset=utf-8",
            &body,
        ))
        .await?;
    stream.shutdown().await?;
    Ok(())
}

fn is_loopback_host(host: &str) -> bool {
    let host = host.trim();
    let hostname = if host.starts_with('[') {
        let Some(close) = host.find(']') else {
            return false;
        };
        let suffix = &host[close + 1..];
        if !suffix.is_empty() {
            let Some(port) = suffix.strip_prefix(':') else {
                return false;
            };
            if port.parse::<u16>().is_err() {
                return false;
            }
        }
        &host[..=close]
    } else if let Some((hostname, port)) = host.rsplit_once(':') {
        if hostname.contains(':') || port.parse::<u16>().is_err() {
            return false;
        }
        hostname
    } else {
        host
    };

    hostname.eq_ignore_ascii_case("localhost") || hostname == "127.0.0.1" || hostname == "[::1]"
}

/// The local product API is same-origin only. Native eframe requests use the
/// fixed `awenet` Host alias and do not send a browser Origin header.
fn local_request_allowed(request: &str) -> bool {
    let mut host = None;
    let mut origin = None;

    for line in request.lines().skip(1) {
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim();
        if name.eq_ignore_ascii_case("host") {
            if host.replace(value).is_some() {
                return false;
            }
        } else if name.eq_ignore_ascii_case("origin") && origin.replace(value).is_some() {
            return false;
        }
    }

    let Some(host) = host else {
        return false;
    };
    match origin {
        None => host.eq_ignore_ascii_case("awenet") || is_loopback_host(host),
        Some(origin) => {
            if !is_loopback_host(host) {
                return false;
            }
            origin.eq_ignore_ascii_case(&format!("http://{host}"))
        }
    }
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
        if data.len() > HEADER_LIMIT {
            anyhow::bail!("HTTP headers too large");
        }
        if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            header_end = pos + 4;
            break;
        }
    }

    let headers = String::from_utf8_lossy(&data[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);

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

#[allow(clippy::too_many_arguments)]
pub(crate) fn parse_onecoin_atoms(value: &serde_json::Value) -> Result<u128, String> {
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
    if fraction.len() > 8 || !fraction.chars().all(|c| c.is_ascii_digit()) {
        return Err("amount_coins supports at most 8 decimal places".into());
    }
    let whole_atoms = whole
        .parse::<u128>()
        .map_err(|_| "amount_coins is too large".to_string())?
        .checked_mul(ATOMS_PER_COIN)
        .ok_or_else(|| "amount_coins is too large".to_string())?;
    let padded = format!("{fraction:0<8}");
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

fn parse_json_u64(value: &serde_json::Value, field: &str) -> Result<u64, String> {
    value
        .get(field)
        .and_then(|value| value.as_u64())
        .ok_or_else(|| format!("{field} must be a non-negative integer"))
}

fn parse_json_u32(value: &serde_json::Value, field: &str) -> Result<u32, String> {
    u32::try_from(parse_json_u64(value, field)?).map_err(|_| format!("{field} is too large"))
}

fn parse_json_u16(value: &serde_json::Value, field: &str) -> Result<u16, String> {
    u16::try_from(parse_json_u64(value, field)?).map_err(|_| format!("{field} is too large"))
}

fn parse_hex_array<const N: usize>(value: &str, field: &str) -> Result<[u8; N], String> {
    if value.len() != N * 2 {
        return Err(format!(
            "{field} must contain exactly {} hexadecimal characters",
            N * 2
        ));
    }
    let bytes = hex::decode(value).map_err(|_| format!("{field} must be hexadecimal"))?;
    bytes
        .try_into()
        .map_err(|_| format!("{field} must contain exactly {N} bytes"))
}

fn user_account_json(account: &UserAccount) -> serde_json::Value {
    serde_json::json!({
        "awe_id": account.awe_id,
        "username": account.username,
        "public_key": hex::encode(account.public_key),
        "role": account.role.as_str(),
        "status": account.status.as_str(),
        "created_at_unix": account.created_at_unix,
        "updated_at_unix": account.updated_at_unix,
        "sequence": account.sequence
    })
}

fn user_registry_error_status(error: &UserRegistryError) -> &'static str {
    match error {
        UserRegistryError::InvalidSignature
        | UserRegistryError::FounderProtected
        | UserRegistryError::WrongFounder
        | UserRegistryError::InvalidStoreSignature => "403 Forbidden",
        UserRegistryError::UnknownUser => "404 Not Found",
        UserRegistryError::UsernameTaken | UserRegistryError::AlreadyRegistered => "409 Conflict",
        _ => "400 Bad Request",
    }
}

fn network_control_error_status(error: &NetworkControlError) -> &'static str {
    match error {
        NetworkControlError::NotFounder
        | NetworkControlError::InvalidSignature
        | NetworkControlError::WrongFounder => "403 Forbidden",
        NetworkControlError::SequenceExhausted
        | NetworkControlError::TimestampWentBackwards
        | NetworkControlError::Topology(_) => "409 Conflict",
        NetworkControlError::ActionTooLarge | NetworkControlError::StoreTooLarge => {
            "413 Payload Too Large"
        }
        NetworkControlError::TimestampInFuture
        | NetworkControlError::InvalidIdentifier
        | NetworkControlError::InvalidMetrics => "400 Bad Request",
        NetworkControlError::InvalidState
        | NetworkControlError::InvalidStoreVersion
        | NetworkControlError::Serialization(_)
        | NetworkControlError::Io(_) => "500 Internal Server Error",
    }
}

async fn serve_ui(mut stream: tokio::net::TcpStream, state: UiState) -> Result<()> {
    let UiState {
        node,
        messenger,
        federation_state,
        federation_path,
        storage,
        pending_acks,
        pending_shards,
        policy_state,
        community,
        onecoin_ledger,
        onecoin_path,
        onecoin_offers_path,
        contribution,
        contribution_path,
        founder_guard,
        user_registry,
        user_registry_path,
        network_control,
        network_control_path,
        economy,
        economy_path,
        verifier_net,
        verifier_net_path,
    } = state;
    let request = read_http_request(&mut stream).await?;
    if !local_request_allowed(&request) {
        stream
            .write_all(&http_response(
                "403 Forbidden",
                "application/json; charset=utf-8",
                r#"{"status":"error","error":"local_origin_not_allowed"}"#,
            ))
            .await?;
        stream.shutdown().await?;
        return Ok(());
    }
    let request_line = request.lines().next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let target = parts.next().unwrap_or("/");
    let path = target.split('?').next().unwrap_or("/");
    let (status, mime, body) = match path {
        "/" | "/index.html" => ("200 OK", "text/html; charset=utf-8", UI_HTML.to_string()),
        "/style.css" => ("200 OK", "text/css; charset=utf-8", UI_CSS.to_string()),
        "/app.js" => ("200 OK", "application/javascript; charset=utf-8", UI_JS.to_string()),
        "/onecoin.js" => ("200 OK", "application/javascript; charset=utf-8", ONECOIN_JS.to_string()),
        "/onecoin.css" => ("200 OK", "text/css; charset=utf-8", ONECOIN_CSS.to_string()),
        "/economy.css" => ("200 OK", "text/css; charset=utf-8", ECONOMY_CSS.to_string()),
        "/awenetwork.js" => ("200 OK", "application/javascript; charset=utf-8", AWENETWORK_JS.to_string()),
        "/storefront.js" => ("200 OK", "application/javascript; charset=utf-8", STOREFRONT_JS.to_string()),
        "/founder.js" => ("200 OK", "application/javascript; charset=utf-8", FOUNDER_JS.to_string()),
        "/vendor/qrcode.js" => ("200 OK", "application/javascript; charset=utf-8", QR_JS.to_string()),
        "/i18n.js" => ("200 OK", "application/javascript; charset=utf-8", I18N_JS.to_string()),
        "/api/onebank/wallet" if method == "GET" => {
            let ledger = onecoin_ledger.lock().map(|l| l.clone()).unwrap_or_default();
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
            let result: Result<ResourceContribution, String> = (|| {
                let parsed: serde_json::Value = serde_json::from_str(body)
                    .map_err(|error| format!("invalid contribution JSON: {error}"))?;
                Ok(ResourceContribution {
                    storage_bytes: parse_json_u64(&parsed, "storage_bytes")?,
                    cpu_cores: parse_json_u32(&parsed, "cpu_cores")?,
                    ram_bytes: parse_json_u64(&parsed, "ram_bytes")?,
                    gpu_units: parse_json_u32(&parsed, "gpu_units")?,
                    bandwidth_bytes: parse_json_u64(&parsed, "bandwidth_bytes")?,
                    online_hours: parse_json_u16(&parsed, "online_hours")?,
                    node_count: parse_json_u32(&parsed, "node_count")?,
                    server_count: parse_json_u32(&parsed, "server_count")?,
                    uptime_bps: parse_json_u16(&parsed, "uptime_bps")?,
                    utilization_bps: parse_json_u16(&parsed, "utilization_bps")?,
                })
            })();
            match result.and_then(|r| r.validate().map(|()| r)) {
                Ok(r) => {
                    match fs::write(&contribution_path, serde_json::to_vec_pretty(&r).unwrap_or_default()) {
                        Ok(()) => {
                            if let Ok(mut guard) = contribution.lock() { *guard = r.clone(); }
                            ("200 OK", "application/json; charset=utf-8", serde_json::json!({
                                "status":"accepted_for_verification",
                                "tier": classify_tier(&r).wire_name(),
                                "verified":false,
                                "reward_status":"requires_signed_usage_receipt"
                            }).to_string())
                        },
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
                Err(error) => ("400 Bad Request", "application/json; charset=utf-8", serde_json::json!({"status":"rejected","error":error}).to_string())
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
            let mut offer: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
            if !offer.is_object() {
                offer = serde_json::json!({});
            }
            if let Some(obj) = offer.as_object_mut() {
                obj.insert("owner".into(), serde_json::json!(node.identity.public.awe_id.to_hex()));
                obj.insert("settlement".into(), serde_json::json!("DIRECT_PERSON_TO_PERSON"));
                obj.insert("coin_transfer".into(), serde_json::json!("AWENET_WALLET"));
                obj.insert("fiat_transfer".into(), serde_json::json!("OUTSIDE_AWENET"));
                obj.insert("created_at".into(), serde_json::json!(now_unix()));
            }
            let mut offers: Vec<serde_json::Value> = fs::read(&onecoin_offers_path)
                .ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
            offers.push(offer.clone());
            let response = fs::write(&onecoin_offers_path, serde_json::to_vec_pretty(&offers).unwrap_or_default());
            match response {
                Ok(()) => ("200 OK", "application/json; charset=utf-8", serde_json::json!({"status":"published","offer":offer,"notice":"ONECOIN transfer is handled by AWENET wallet. Fiat is exchanged directly between people outside AWENET."}).to_string()),
                Err(error) => ("500 Internal Server Error", "application/json; charset=utf-8", serde_json::json!({"status":"error","error":error.to_string()}).to_string())
            }
        },
        "/api/network/control" | "/api/network/topology" if method == "GET" => {
            match network_control.lock() {
                Ok(control) => match serde_json::to_value(control.topology()) {
                    Ok(topology) => (
                        "200 OK",
                        "application/json; charset=utf-8",
                        serde_json::json!({
                            "status": "ok",
                            "version": network_control::NETWORK_CONTROL_VERSION,
                            "founder_awe_id": control.founder_awe_id(),
                            "sequence": control.sequence(),
                            "next_sequence": control.next_sequence(),
                            "updated_at_unix": control.updated_at_unix(),
                            "server_time_unix": now_unix(),
                            "topology": topology
                        }).to_string(),
                    ),
                    Err(_) => (
                        "500 Internal Server Error",
                        "application/json; charset=utf-8",
                        r#"{"status":"error","error":"network_topology_serialization_failed"}"# .into(),
                    ),
                },
                Err(_) => (
                    "500 Internal Server Error",
                    "application/json; charset=utf-8",
                    r#"{"status":"error","error":"network_control_lock_failed"}"# .into(),
                ),
            }
        },
        "/api/network/control" if method == "POST" => {
            let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
            if body.len() > network_control::MAX_NETWORK_ACTION_BYTES.saturating_add(4096) {
                return write_json_error(&mut stream, "413 Payload Too Large", "network_action_request_too_large").await;
            }
            let parsed: serde_json::Value = match serde_json::from_str(body) {
                Ok(value) => value,
                Err(error) => return write_json_error(&mut stream, "400 Bad Request", &format!("invalid_json: {error}")).await,
            };
            let action_value = match parsed.get("action").cloned() {
                Some(value) => value,
                None => return write_json_error(&mut stream, "400 Bad Request", "action is required").await,
            };
            let action: NetworkAction = match serde_json::from_value(action_value) {
                Ok(value) => value,
                Err(error) => return write_json_error(&mut stream, "400 Bad Request", &format!("invalid_network_action: {error}")).await,
            };
            let at_unix = match parse_json_u64(&parsed, "at_unix") {
                Ok(value) => value,
                Err(error) => return write_json_error(&mut stream, "400 Bad Request", &error).await,
            };
            let signature = match parse_hex_array::<64>(
                parsed.get("signature").and_then(|value| value.as_str()).unwrap_or(""),
                "signature",
            ) {
                Ok(value) => value,
                Err(error) => return write_json_error(&mut stream, "400 Bad Request", &error).await,
            };
            match network_control.lock() {
                Ok(mut current) => match current.apply_and_save(
                    &network_control_path,
                    &node.identity,
                    &action,
                    at_unix,
                    now_unix(),
                    &signature,
                ) {
                    Ok(sequence) => (
                        "200 OK",
                        "application/json; charset=utf-8",
                        serde_json::json!({
                            "status": "applied",
                            "sequence": sequence,
                            "updated_at_unix": current.updated_at_unix()
                        }).to_string(),
                    ),
                    Err(error) => {
                        let status = network_control_error_status(&error);
                        (status, "application/json; charset=utf-8", serde_json::json!({
                            "status": "error",
                            "error": error.to_string(),
                            "sequence": current.sequence()
                        }).to_string())
                    }
                },
                Err(_) => (
                    "500 Internal Server Error",
                    "application/json; charset=utf-8",
                    r#"{"status":"error","error":"network_control_lock_failed"}"# .into(),
                ),
            }
        },
        "/api/users" if method == "GET" => {
            match user_registry.lock() {
                Ok(registry) => {
                    let users = registry.accounts().iter().map(user_account_json).collect::<Vec<_>>();
                    ("200 OK", "application/json; charset=utf-8", serde_json::json!({
                        "status": "ok",
                        "founder_awe_id": registry.founder_awe_id(),
                        "sequence": registry.sequence(),
                        "next_sequence": registry.sequence().checked_add(1),
                        "server_time_unix": now_unix(),
                        "users": users
                    }).to_string())
                }
                Err(_) => ("500 Internal Server Error", "application/json; charset=utf-8",
                    serde_json::json!({"status":"error","error":"user_registry_lock_failed"}).to_string())
            }
        },
        "/api/users/register" if method == "POST" => {
            let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
            let parsed = match serde_json::from_str::<serde_json::Value>(body) {
                Ok(value) => value,
                Err(error) => return write_json_error(&mut stream, "400 Bad Request", &format!("invalid_json: {error}")).await,
            };
            let username = parsed.get("username").and_then(|value| value.as_str()).unwrap_or("");
            let public_key = parsed.get("public_key").and_then(|value| value.as_str()).unwrap_or("");
            let signature = parsed.get("signature").and_then(|value| value.as_str()).unwrap_or("");
            let public_key = match parse_hex_array::<32>(public_key, "public_key") {
                Ok(value) => value,
                Err(error) => return write_json_error(&mut stream, "400 Bad Request", &error).await,
            };
            let signature = match parse_hex_array::<64>(signature, "signature") {
                Ok(value) => value,
                Err(error) => return write_json_error(&mut stream, "400 Bad Request", &error).await,
            };
            match user_registry.lock() {
                Ok(mut current) => {
                    let mut candidate = current.clone();
                    match candidate.register(username, &public_key, &signature, now_unix()) {
                        Ok(account) => {
                            let created = candidate.sequence() != current.sequence();
                            if created {
                                if let Err(error) = user_registry::save(&user_registry_path, &candidate, &node.identity) {
                                    ("500 Internal Server Error", "application/json; charset=utf-8",
                                        serde_json::json!({"status":"error","error":error.to_string()}).to_string())
                                } else {
                                    *current = candidate;
                                    ("201 Created", "application/json; charset=utf-8",
                                        serde_json::json!({"status":"created","user":user_account_json(&account),"sequence":current.sequence()}).to_string())
                                }
                            } else {
                                ("200 OK", "application/json; charset=utf-8",
                                    serde_json::json!({"status":"already_registered","user":user_account_json(&account),"sequence":current.sequence()}).to_string())
                            }
                        }
                        Err(error) => (user_registry_error_status(&error), "application/json; charset=utf-8",
                            serde_json::json!({"status":"error","error":error.to_string()}).to_string())
                    }
                }
                Err(_) => ("500 Internal Server Error", "application/json; charset=utf-8",
                    serde_json::json!({"status":"error","error":"user_registry_lock_failed"}).to_string())
            }
        },
        "/api/admin/users/role" if method == "POST" => {
            let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
            let parsed: serde_json::Value = match serde_json::from_str(body) {
                Ok(value) => value,
                Err(error) => return write_json_error(&mut stream, "400 Bad Request", &format!("invalid_json: {error}")).await,
            };
            let target = parsed.get("awe_id").and_then(|value| value.as_str()).unwrap_or("");
            let at_unix = match parse_json_u64(&parsed, "at_unix") {
                Ok(value) => value,
                Err(error) => return write_json_error(&mut stream, "400 Bad Request", &error).await,
            };
            let role = match parsed.get("role").and_then(|value| value.as_str()) {
                Some("founder") => UserRole::Founder,
                Some("admin") => UserRole::Admin,
                Some("user") => UserRole::User,
                _ => return write_json_error(&mut stream, "400 Bad Request", "role must be founder, admin or user").await,
            };
            let signature = match parse_hex_array::<64>(parsed.get("signature").and_then(|value| value.as_str()).unwrap_or(""), "signature") {
                Ok(value) => value,
                Err(error) => return write_json_error(&mut stream, "400 Bad Request", &error).await,
            };
            match user_registry.lock() {
                Ok(mut current) => {
                    let mut candidate = current.clone();
                    match candidate.set_role(target, role, at_unix, &signature) {
                        Ok(account) => {
                            if candidate.sequence() != current.sequence() {
                                if let Err(error) = user_registry::save(&user_registry_path, &candidate, &node.identity) {
                                    ("500 Internal Server Error", "application/json; charset=utf-8",
                                        serde_json::json!({"status":"error","error":error.to_string()}).to_string())
                                } else {
                                    *current = candidate;
                                    ("200 OK", "application/json; charset=utf-8",
                                        serde_json::json!({"status":"updated","user":user_account_json(&account),"sequence":current.sequence()}).to_string())
                                }
                            } else {
                                ("200 OK", "application/json; charset=utf-8",
                                    serde_json::json!({"status":"unchanged","user":user_account_json(&account),"sequence":current.sequence()}).to_string())
                            }
                        }
                        Err(error) => (user_registry_error_status(&error), "application/json; charset=utf-8",
                            serde_json::json!({"status":"error","error":error.to_string()}).to_string())
                    }
                }
                Err(_) => ("500 Internal Server Error", "application/json; charset=utf-8",
                    serde_json::json!({"status":"error","error":"user_registry_lock_failed"}).to_string())
            }
        },
        "/api/admin/users/status" if method == "POST" => {
            let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
            let parsed: serde_json::Value = match serde_json::from_str(body) {
                Ok(value) => value,
                Err(error) => return write_json_error(&mut stream, "400 Bad Request", &format!("invalid_json: {error}")).await,
            };
            let target = parsed.get("awe_id").and_then(|value| value.as_str()).unwrap_or("");
            let at_unix = match parse_json_u64(&parsed, "at_unix") {
                Ok(value) => value,
                Err(error) => return write_json_error(&mut stream, "400 Bad Request", &error).await,
            };
            let status = match parsed.get("status").and_then(|value| value.as_str()) {
                Some("active") => UserStatus::Active,
                Some("suspended") => UserStatus::Suspended,
                _ => return write_json_error(&mut stream, "400 Bad Request", "status must be active or suspended").await,
            };
            let signature = match parse_hex_array::<64>(parsed.get("signature").and_then(|value| value.as_str()).unwrap_or(""), "signature") {
                Ok(value) => value,
                Err(error) => return write_json_error(&mut stream, "400 Bad Request", &error).await,
            };
            match user_registry.lock() {
                Ok(mut current) => {
                    let mut candidate = current.clone();
                    match candidate.set_status(target, status, at_unix, &signature) {
                        Ok(account) => {
                            if candidate.sequence() != current.sequence() {
                                if let Err(error) = user_registry::save(&user_registry_path, &candidate, &node.identity) {
                                    ("500 Internal Server Error", "application/json; charset=utf-8",
                                        serde_json::json!({"status":"error","error":error.to_string()}).to_string())
                                } else {
                                    *current = candidate;
                                    ("200 OK", "application/json; charset=utf-8",
                                        serde_json::json!({"status":"updated","user":user_account_json(&account),"sequence":current.sequence()}).to_string())
                                }
                            } else {
                                ("200 OK", "application/json; charset=utf-8",
                                    serde_json::json!({"status":"unchanged","user":user_account_json(&account),"sequence":current.sequence()}).to_string())
                            }
                        }
                        Err(error) => (user_registry_error_status(&error), "application/json; charset=utf-8",
                            serde_json::json!({"status":"error","error":error.to_string()}).to_string())
                    }
                }
                Err(_) => ("500 Internal Server Error", "application/json; charset=utf-8",
                    serde_json::json!({"status":"error","error":"user_registry_lock_failed"}).to_string())
            }
        },
        _ if path.starts_with("/api/users/") && method == "GET" => {
            let requested = path.strip_prefix("/api/users/").unwrap_or("");
            match AweId::from_hex(requested) {
                Ok(id) => match user_registry.lock() {
                    Ok(registry) => match registry.get(&id.to_hex()) {
                        Some(account) => ("200 OK", "application/json; charset=utf-8",
                            serde_json::json!({"status":"ok","user":user_account_json(account),"sequence":registry.sequence()}).to_string()),
                        None => ("404 Not Found", "application/json; charset=utf-8",
                            serde_json::json!({"status":"error","error":"user_not_found"}).to_string())
                    },
                    Err(_) => ("500 Internal Server Error", "application/json; charset=utf-8",
                        serde_json::json!({"status":"error","error":"user_registry_lock_failed"}).to_string())
                },
                Err(_) => ("400 Bad Request", "application/json; charset=utf-8",
                    serde_json::json!({"status":"error","error":"invalid_awe_id"}).to_string())
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
            let policy = current_policy(&policy_state);
            ("200 OK", "application/json; charset=utf-8", serde_json::to_string(&policy).unwrap_or_else(|_| "{}".into()))
        },
        "/api/status" => {
            let peers = node.closest_peers(node.identity.public.awe_id.as_bytes(), 64).await;
            let peer_json = peers.iter().map(|p| serde_json::json!({
                "id": format_uid(&p.awe_id),
                "awe_id": hex::encode(p.awe_id),
                "public_key": hex::encode(p.public_key),
                "address": p.addresses.first().map(ToString::to_string).unwrap_or_else(|| "unknown".into()),
                "last_seen": p.last_seen_unix
            })).collect::<Vec<_>>();
            ("200 OK", "application/json; charset=utf-8", serde_json::json!({
                "product": "AWEp2P", "status": "online",
                "node_id": format_uid(node.identity.public.awe_id.as_bytes()),
                "node_address": node.listen_addr.to_string(),
                "transport": "AWE encrypted TCP", "ui": "connected", "peers": peer_json, "discovered_peers": peer_json.len(), "active_connections": node.active_peer_count().await
            }).to_string())
        },
        "/api/security/passkeys" if method == "GET" => {
            let guard = founder_guard.lock().map(|value| value.clone()).unwrap_or_else(|_| {
                FounderGuard::install(&node.identity, 0)
            });
            ("200 OK", "application/json; charset=utf-8", serde_json::json!({
                "storage": "founder_signed_atomic",
                "max_credentials": awep2p_core::founder_guard::MAX_PASSKEYS,
                "enrolled": guard.passkeys().len(),
                "labels": guard.passkeys().iter().map(|key| key.label.clone()).collect::<Vec<_>>(),
                "recovery_codes_remaining": guard.recovery_codes_remaining(),
                "step_up_active": guard.session_active(now_unix())
            }).to_string())
        },
        "/api/connect" if method == "POST" => {
            let address = request.split("address=").nth(1).and_then(|x| x.split_whitespace().next()).unwrap_or("");
            let address = address.replace("%3A", ":").replace("%3a", ":");
            match address.parse::<SocketAddr>() {
                Ok(addr) => {
                    let connect_node = node.clone();
                    tokio::spawn(async move {
                        let _ = connect_node.bootstrap(&[addr]).await;
                    });
                    ("202 Accepted", "application/json; charset=utf-8",
                        serde_json::json!({"status":"connecting","address":addr.to_string()}).to_string())
                },
                Err(_) => ("400 Bad Request", "application/json; charset=utf-8", serde_json::json!({"status":"error","error":"invalid socket address"}).to_string())
            }
        },
        "/api/federation" => {
            let state = federation_state.lock().map(|s| s.clone()).unwrap_or_default();
            ("200 OK", "application/json; charset=utf-8", serde_json::json!({
                "format": state.format, "version": state.version, "local_node_id": state.local_node_id,
                "data_centre_id": state.local_data_centre_id, "data_group_id": state.local_data_group_id,
                "joined_data_centres": state.joined_data_centres, "joined_data_groups": state.joined_data_groups,
                "bootstrap_endpoints": state.bootstrap_endpoints
            }).to_string())
        },
        "/api/federation/generate" if method == "POST" => {
            let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
            let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
            let kind = parsed.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            let name = parsed.get("name").and_then(|v| v.as_str()).unwrap_or("AWE");
            let now = now_unix();
            let result: Result<(String, String), String> = match kind {
                "awenode" => {
                    let endpoint = parsed.get("endpoint").and_then(|v| v.as_str()).map(str::to_owned).unwrap_or_else(|| node.listen_addr.to_string());
                    let bootstrap = parsed.get("bootstrap").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
                    let dc_id = parsed.get("data_centre_id").and_then(|v| v.as_str()).map(str::to_owned).or_else(|| federation_state.lock().ok().and_then(|s| s.local_data_centre_id.clone())).unwrap_or_else(|| format!("dc-{}", &hex::encode(blake3::hash(format!("AWE/DC/{}", format_uid(node.identity.public.awe_id.as_bytes())).as_bytes()).as_bytes())[..24]));
                    federation::generate_awenode_for_dc(
                        &format_uid(node.identity.public.awe_id.as_bytes()),
                        &dc_id,
                        name,
                        &endpoint,
                        bootstrap,
                        now,
                    )
                    .and_then(|cfg| {
                        serde_json::to_string_pretty(&cfg)
                            .map(|s| ("awenode.awenode".into(), s))
                            .map_err(|e| e.to_string())
                    })
                },
                "awedc" => {
                    let dc_id = parsed.get("data_centre_id").and_then(|v| v.as_str()).unwrap_or("");
                    let endpoints = parsed.get("endpoints").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_else(|| vec![node.listen_addr.to_string()]);
                    let members = parsed.get("member_node_ids").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_else(|| vec![format_uid(node.identity.public.awe_id.as_bytes())]);
                    if dc_id.is_empty() { Err("data_centre_id is required".into()) } else {
                        federation::generate_awedc(
                            &format_uid(node.identity.public.awe_id.as_bytes()),
                            dc_id,
                            name,
                            endpoints,
                            members,
                            now,
                        )
                        .and_then(|cfg| {
                            serde_json::to_string_pretty(&cfg)
                                .map(|s| ("data-centre.awedc".into(), s))
                                .map_err(|e| e.to_string())
                        })
                    }
                },
                "dgc" => {
                    let dc_id = parsed.get("owner_data_centre_id").and_then(|v| v.as_str()).unwrap_or("");
                    let centres = parsed.get("data_centre_ids").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
                    if dc_id.is_empty() { Err("owner_data_centre_id is required".into()) } else {
                        federation::generate_dgc(dc_id, name, centres, now).and_then(|cfg| {
                            serde_json::to_string_pretty(&cfg)
                                .map(|s| ("data-group.dgc".into(), s))
                                .map_err(|e| e.to_string())
                        })
                    }
                },
                _ => Err("kind must be awenode, awedc or dgc".to_string())
            };
            match result {
                Ok((filename, content)) => ("200 OK", "application/json; charset=utf-8", serde_json::json!({"status":"generated","filename":filename,"content":content}).to_string()),
                Err(error) => {
                    let status = if error == "secure_random_unavailable" {
                        "500 Internal Server Error"
                    } else {
                        "400 Bad Request"
                    };
                    (
                        status,
                        "application/json; charset=utf-8",
                        serde_json::json!({"status":"error","error":error}).to_string(),
                    )
                }
            }
        },
        "/api/federation/import" if method == "POST" => {
            let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
            let result: Result<Vec<String>, (u16, String)> = (|| {
                let parsed: serde_json::Value = serde_json::from_str(body)
                    .map_err(|error| (400, format!("invalid federation import JSON: {error}")))?;
                let kind = parsed.get("kind").and_then(|v| v.as_str()).unwrap_or("");
                let content = parsed.get("content").and_then(|v| v.as_str()).unwrap_or("");
                if content.is_empty() {
                    return Err((400, "federation invitation content is required".into()));
                }

                // Keep the state lock until the candidate is durably published. This
                // prevents concurrent imports from overwriting one another and ensures
                // the in-memory state is never ahead of the persisted state.
                let mut current = federation_state
                    .lock()
                    .map_err(|_| (500, "federation state lock poisoned".to_string()))?;
                let mut candidate = current.clone();

                match kind {
                    "awenode" => {
                        let cfg: AweNodeConfig = serde_json::from_str(content)
                            .map_err(|error| (400, error.to_string()))?;
                        federation::validate_awenode(&cfg).map_err(|error| (400, error))?;
                        candidate.local_node_id =
                            format_uid(node.identity.public.awe_id.as_bytes());
                        candidate.local_data_centre_id = Some(cfg.data_centre_id.clone());
                        candidate.bootstrap_endpoints = cfg.bootstrap_endpoints.clone();
                        if !candidate.bootstrap_endpoints.contains(&cfg.endpoint) {
                            candidate.bootstrap_endpoints.push(cfg.endpoint);
                        }
                        if !candidate.joined_data_centres.contains(&cfg.data_centre_id) {
                            candidate.joined_data_centres.push(cfg.data_centre_id);
                        }
                    }
                    "awedc" => {
                        let cfg: DataCentreConfig = serde_json::from_str(content)
                            .map_err(|error| (400, error.to_string()))?;
                        federation::validate_awedc(&cfg).map_err(|error| (400, error))?;
                        candidate.local_node_id =
                            format_uid(node.identity.public.awe_id.as_bytes());
                        // A DC invitation adds the remote DC as a peer; it must not
                        // silently replace this node's own data-centre identity.
                        if candidate.local_data_centre_id.is_none() {
                            candidate.local_data_centre_id = Some(format!(
                                "dc-{}",
                                &hex::encode(
                                    blake3::hash(
                                        format!("AWE/DC/{}", candidate.local_node_id).as_bytes()
                                    )
                                    .as_bytes()
                                )[..24]
                            ));
                        }
                        candidate.bootstrap_endpoints.extend(cfg.endpoints);
                        candidate.bootstrap_endpoints.sort();
                        candidate.bootstrap_endpoints.dedup();
                        if !candidate.joined_data_centres.contains(&cfg.data_centre_id) {
                            candidate.joined_data_centres.push(cfg.data_centre_id);
                        }
                    }
                    "dgc" => {
                        let cfg: DataGroupConfig = serde_json::from_str(content)
                            .map_err(|error| (400, error.to_string()))?;
                        federation::validate_dgc(&cfg).map_err(|error| (400, error))?;
                        candidate.local_node_id =
                            format_uid(node.identity.public.awe_id.as_bytes());
                        candidate.local_data_group_id = Some(cfg.data_group_id.clone());
                        for dc in cfg.data_centre_ids {
                            if !candidate.joined_data_centres.contains(&dc) {
                                candidate.joined_data_centres.push(dc);
                            }
                        }
                        if !candidate.joined_data_groups.contains(&cfg.data_group_id) {
                            candidate.joined_data_groups.push(cfg.data_group_id);
                        }
                    }
                    _ => {
                        return Err((
                            400,
                            "kind must be awenode, awedc or dgc".to_string(),
                        ));
                    }
                }

                candidate.format = "awenet".into();
                candidate.version = federation::FORMAT_VERSION;
                let endpoints = candidate.bootstrap_endpoints.clone();
                federation::save_json(&candidate, &federation_path).map_err(|error| {
                    (
                        500,
                        format!("failed to persist federation state: {error}"),
                    )
                })?;
                *current = candidate;
                Ok(endpoints)
            })();

            match result {
                Ok(endpoints) => {
                    let addresses = endpoints
                        .iter()
                        .filter_map(|endpoint| endpoint.parse::<SocketAddr>().ok())
                        .collect::<Vec<_>>();
                    let discovered = if addresses.is_empty() {
                        0
                    } else {
                        node.bootstrap(&addresses).await.unwrap_or(0)
                    };
                    (
                        "200 OK",
                        "application/json; charset=utf-8",
                        serde_json::json!({
                            "status": "imported",
                            "connected_bootstrap_peers": discovered
                        })
                        .to_string(),
                    )
                }
                Err((status, error)) => {
                    let status_text = if status == 500 {
                        "500 Internal Server Error"
                    } else {
                        "400 Bad Request"
                    };
                    (
                        status_text,
                        "application/json; charset=utf-8",
                        serde_json::json!({"status":"error","error":error}).to_string(),
                    )
                }
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
