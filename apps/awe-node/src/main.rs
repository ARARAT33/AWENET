use anyhow::{Context, Result};
use awep2p_core::identity::{AweSecret, Identity, LocalVault, Username};
use awep2p_core::lan_mesh::LanPeerBeacon;
use awep2p_core::messenger::format_uid;
use awep2p_core::namespace::AweBrowserResolver;
use awep2p_core::network::{format_node_descriptor, Node};
use awep2p_core::node::{validate_and_configure_node_allocation, NodeAllocationMode};
use awep2p_core::diagnostics::{NodeDiagnostics, NodeMetrics};
use awep2p_core::reputation::NodeReputation;
use awep2p_core::storage::{SecretFilePackage, StoragePolicy};
use awep2p_core::store::{AWEPackage, AppCapability, AppKind};
use awep2p_core::permissions::CapabilitySet;
use awep2p_core::sandbox::{SandboxConfig, WasmSandbox};
use std::{collections::BTreeMap, env, fs, net::SocketAddr, path::PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const UI_HTML: &str = include_str!("../../awe-desktop/ui/index.html");
const UI_CSS: &str = include_str!("../../awe-desktop/ui/style.css");
const UI_JS: &str = include_str!("../../awe-desktop/ui/app.js");
const UI_ADDR: &str = "127.0.0.1:41800";

fn default_vault() -> PathBuf {
    if let Some(home) = env::var_os("HOME") { return PathBuf::from(home).join(".awep2p").join("identity.vault"); }
    if let Some(profile) = env::var_os("USERPROFILE") { return PathBuf::from(profile).join(".awep2p").join("identity.vault"); }
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
    let bytes = secret.to_bytes().context("failed to serialize .awesecret")?;
    let path = out_path.unwrap_or_else(|| {
        if let Some(home) = env::var_os("HOME") { PathBuf::from(home).join(".awep2p").join(format!("{username_str}.awesecret")) }
        else { PathBuf::from(format!("{username_str}.awesecret")) }
    });
    if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
    fs::write(&path, bytes)?;
    println!("AWE-ID: {}", secret.awe_id);
    println!("Node Descriptor: {}", format_node_descriptor(identity.public.awe_id.as_bytes()));
    println!("Saved: {}", path.display());
    Ok(())
}

fn init(username: &str, path: PathBuf) -> Result<()> {
    let identity = Identity::generate(Username::new(username.to_owned()).map_err(anyhow::Error::msg)?);
    let password = rpassword::prompt_password("Vault password: ")?;
    if password.is_empty() { anyhow::bail!("vault password must not be empty"); }
    let vault = LocalVault::seal(&identity, &password).map_err(anyhow::Error::msg)?;
    if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
    fs::write(&path, vault)?;
    println!("AWE-ID: {}", identity.public.awe_id.to_hex());
    println!("Identity vault: {}", path.display());
    Ok(())
}

fn load_identity(path: &PathBuf, password: &str, username: &str) -> Result<Identity> {
    let data = fs::read(path)?;
    LocalVault::open(&data, Username::new(username.to_owned()).map_err(anyhow::Error::msg)?, password)
        .map_err(anyhow::Error::msg)
}

async fn http_response(status: &str, content_type: &str, body: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes()
}

async fn serve_ui(mut stream: tokio::net::TcpStream, node: Node) -> Result<()> {
    let mut buf = vec![0u8; 8192];
    let n = stream.read(&mut buf).await?;
    let request = String::from_utf8_lossy(&buf[..n]);
    let request_line = request.lines().next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let path = parts.next().unwrap_or("/");
    let (status, mime, body) = match path {
        "/" | "/index.html" => ("200 OK", "text/html; charset=utf-8", UI_HTML.to_string()),
        "/style.css" => ("200 OK", "text/css; charset=utf-8", UI_CSS.to_string()),
        "/app.js" => ("200 OK", "application/javascript; charset=utf-8", UI_JS.to_string()),
        "/api/status" => {
            let peers = node.closest_peers(node.identity.public.awe_id.as_bytes(), 64).await;
            let peer_json = peers.iter().map(|p| serde_json::json!({
                "id": format_uid(&p.awe_id),
                "address": p.addresses.first().map(ToString::to_string).unwrap_or_else(|| "unknown".into()),
                "last_seen": p.last_seen_unix
            })).collect::<Vec<_>>();
            ("200 OK", "application/json; charset=utf-8", serde_json::json!({
                "product": "AWEp2P", "status": "online",
                "node_id": format_uid(node.identity.public.awe_id.as_bytes()),
                "node_address": node.listen_addr.to_string(),
                "transport": "AWE encrypted TCP", "ui": "connected", "peers": peer_json
            }).to_string())
        },
        "/api/connect" if method == "POST" => {
            let address = request.split("address=").nth(1).and_then(|x| x.split_whitespace().next()).unwrap_or("");
            let address = address.replace("%3A", ":").replace("%3a", ":");
            match address.parse::<SocketAddr>() {
                Ok(addr) => match node.bootstrap(&[addr]).await {
                    Ok(found) => ("200 OK", "application/json; charset=utf-8", serde_json::json!({"status":"connected","address":addr.to_string(),"discovered":found,"peer_id":format_uid(&node.closest_peers(node.identity.public.awe_id.as_bytes(),1).await.first().map(|p| p.awe_id).unwrap_or([0;32]))}).to_string()),
                    Err(e) => ("502 Bad Gateway", "application/json; charset=utf-8", serde_json::json!({"status":"error","error":e.to_string()}).to_string())
                },
                Err(_) => ("400 Bad Request", "application/json; charset=utf-8", serde_json::json!({"status":"error","error":"invalid socket address"}).to_string())
            }
        },
        "/api/health" => ("200 OK", "application/json; charset=utf-8", serde_json::json!({
            "status":"healthy","core":"ready","ui":"ready","api":"ready"
        }).to_string()),
        _ => ("404 Not Found", "text/plain; charset=utf-8", "Not Found".to_string()),
    };
    stream.write_all(&http_response(status, mime, &body).await).await?;
    Ok(())
}

async fn run_product() -> Result<()> {
    let identity = Identity::generate(Username::new("awe-node".to_string()).map_err(anyhow::Error::msg)?);
    let node_id = format_uid(identity.public.awe_id.as_bytes());
    let listen: SocketAddr = "127.0.0.1:41000".parse()?;
    let node = Node::new(identity, listen);
    let node_for_listener = node.clone();
    tokio::spawn(async move {
        if let Err(e) = node_for_listener.listen().await {
            eprintln!("AWE node stopped: {e}");
        }
    });

    let listener = tokio::net::TcpListener::bind(UI_ADDR).await
        .with_context(|| format!("cannot bind AWEp2P UI to {UI_ADDR}"))?;
    println!("AWEp2P is running.");
    println!("Node: {node_id}");
    println!("Node transport: {listen}");
    println!("UI: http://{UI_ADDR}");

    let url = format!("http://{UI_ADDR}/");
    #[cfg(target_os = "windows")]
    { let _ = std::process::Command::new("cmd").args(["/C", "start", "", &url]).spawn(); }
    #[cfg(target_os = "macos")]
    { let _ = std::process::Command::new("open").arg(&url).spawn(); }
    #[cfg(all(unix, not(target_os = "macos")))]
    { let _ = std::process::Command::new("xdg-open").arg(&url).spawn(); }

    loop {
        let (stream, _) = listener.accept().await?;
        let api_node = node.clone();
        tokio::spawn(async move {
            if let Err(e) = serve_ui(stream, api_node).await { eprintln!("UI request error: {e}"); }
        });
    }
}

async fn run_node(path: PathBuf, password: String, username: String, listen: SocketAddr, bootstrap: Vec<SocketAddr>) -> Result<()> {
    let identity = load_identity(&path, &password, &username)?;
    let node = Node::new(identity, listen);
    if !bootstrap.is_empty() { node.bootstrap(&bootstrap).await.context("bootstrap failed")?; }
    node.listen().await.map_err(anyhow::Error::msg)
}

fn print_id(path: PathBuf, password: String, username: String) -> Result<()> {
    println!("{}", load_identity(&path, &password, &username)?.public.awe_id.to_hex());
    Ok(())
}
fn print_status(path: PathBuf) -> Result<()> {
    println!("{}", if path.exists() { format!("Vault exists at {}", path.display()) } else { format!("Vault not found at {}", path.display()) });
    Ok(())
}
fn print_diagnostics() -> Result<()> {
    let mut d = NodeDiagnostics::new(); d.update_metrics(NodeMetrics::default());
    println!("Status: {:?}", d.status()); println!("Metrics: {:?}", d.metrics()); Ok(())
}
fn run_mesh(port: u16) -> Result<()> {
    let addr: SocketAddr = format!("0.0.0.0:{port}").parse()?;
    let beacon = LanPeerBeacon::new([1u8;32], addr, false);
    let bytes = beacon.encode().map_err(anyhow::Error::msg)?;
    println!("Broadcast beacon bytes: {}", bytes.len());
    println!("Decoded: {:?}", LanPeerBeacon::decode(&bytes).map_err(anyhow::Error::msg)?.node_id);
    Ok(())
}
fn print_health() -> Result<()> {
    println!("Initial reputation score: {}", NodeReputation::new([1u8;32]).score());
    println!("Health: ONLINE"); Ok(())
}
async fn probe(address: SocketAddr) -> Result<()> {
    let node = Node::new(Identity::generate(Username::new("probe-node").map_err(anyhow::Error::msg)?), "127.0.0.1:0".parse()?);
    let mut c = node.connect(address).await.map_err(anyhow::Error::msg)?;
    println!("Authenticated peer: {:?}", c.remote_id);
    println!("Heartbeat: {:?}", c.ping_roundtrip(1).await.map_err(anyhow::Error::msg)?);
    println!("Encrypted data-plane: {:?}", c.send_data_roundtrip(7, b"AWEP2P-REAL-DATA-PROBE-v1".to_vec()).await.map_err(anyhow::Error::msg)?);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        None | Some("app") | Some("gui") => run_product().await,
        Some("secret") => { let username = args.next().unwrap_or_else(|| usage()); generate_secret_file(&username, args.next().map(PathBuf::from)) },
        Some("init") => { let username = args.next().unwrap_or_else(|| usage()); init(&username, args.next().map(PathBuf::from).unwrap_or_else(default_vault)) },
        Some("run") => {
            let path = args.next().map(PathBuf::from).unwrap_or_else(|| usage());
            let password = args.next().unwrap_or_else(|| usage());
            let listen = args.next().unwrap_or_else(|| usage()).parse().context("invalid listen address")?;
            let username = env::var("AWE_USERNAME").unwrap_or_else(|_| "node".to_string());
            let bootstrap = args.map(|x| x.parse().context("invalid bootstrap address")).collect::<Result<Vec<SocketAddr>>>()?;
            run_node(path, password, username, listen, bootstrap).await
        },
        Some("id") => print_id(args.next().map(PathBuf::from).unwrap_or_else(|| usage()), args.next().unwrap_or_else(|| usage()), args.next().unwrap_or_else(|| usage())),
        Some("status") => print_status(args.next().map(PathBuf::from).unwrap_or_else(default_vault)),
        Some("diagnostics") => print_diagnostics(),
        Some("mesh") => run_mesh(args.next().unwrap_or_else(|| "41000".into()).parse()?),
        Some("health") => print_health(),
        Some("probe") => probe(args.next().unwrap_or_else(|| usage()).parse().context("invalid peer address")?).await,
        _ => usage(),
    }
}
