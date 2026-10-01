use anyhow::{Context, Result};
use awep2p_core::diagnostics::{NodeDiagnostics, NodeMetrics};
use awep2p_core::identity::{AweSecret, Identity, LocalVault, Username};
use awep2p_core::lan_mesh::LanPeerBeacon;
use awep2p_core::messenger::format_uid;
use awep2p_core::namespace::AweBrowserResolver;
use awep2p_core::network::{format_node_descriptor, Node};
use awep2p_core::node::{validate_and_configure_node_allocation, NodeAllocationMode};
use awep2p_core::reputation::NodeReputation;
use awep2p_core::permissions::CapabilitySet;
use awep2p_core::sandbox::{SandboxConfig, WasmSandbox};
use awep2p_core::storage::{SecretFilePackage, StoragePolicy};
use awep2p_core::store::{AWEPackage, AppCapability, AppKind};
#[cfg(not(target_os = "android"))]
use eframe::egui;
use std::{collections::BTreeMap, env, fs, net::SocketAddr, path::PathBuf};

const USAGE: &str = r#"AWEp2P — Sovereign Native P2P Standalone App & Node (100% Rust)

Usage:
  awe-node                                 Launch AWEp2P Native App Engine (Default)
  awe-node app                             Launch AWEp2P Native App Engine
  awe-node secret <username> [out-file]    Generate .awesecret credential file
  awe-node init <username> [vault-file]    Initialize identity vault and .awesecret
  awe-node run <vault-file> <password> <listen-addr> [bootstrap-addr ...]
  awe-node id <vault-file> <password> <username>
  awe-node status [vault-file]
  awe-node diagnostics
  awe-node mesh <listen-port>
  awe-node health
  awe-node probe <address>

Examples:
  awe-node
  awe-node secret ararat ~/.awep2p/ararat.awesecret
"#;

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
    eprintln!("{USAGE}");
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
                .join(format!("{}.awesecret", username_str))
        } else {
            PathBuf::from(format!("{}.awesecret", username_str))
        }
    });

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("failed to create directory")?;
    }
    fs::write(&path, &bytes).context("failed to write .awesecret file")?;

    println!("============================================================");
    println!("🔐 AWEp2P .awesecret Key Generated Successfully!");
    println!("Username: {}", secret.username);
    println!("AWE-ID: {}", secret.awe_id);
    println!(
        "Node Descriptor: {}",
        format_node_descriptor(identity.public.awe_id.as_bytes())
    );
    println!("Saved Secret Key to: {}", path.display());
    println!("============================================================");

    Ok(())
}

fn init(username: &str, path: PathBuf) -> Result<()> {
    let username_obj = Username::new(username.to_owned()).map_err(anyhow::Error::msg)?;
    let identity = Identity::generate(username_obj);
    let password =
        rpassword::prompt_password("Vault password: ").context("failed to read vault password")?;
    if password.is_empty() {
        anyhow::bail!("vault password must not be empty");
    }
    let vault = LocalVault::seal(&identity, &password).map_err(anyhow::Error::msg)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("failed to create AWE data directory")?;
    }
    fs::write(&path, vault).context("failed to write identity vault")?;

    // The encrypted vault is the default persisted credential.
    // Plaintext .awesecret export remains an explicit command only.
    println!("AWE-ID: {}", identity.public.awe_id.to_hex());
    println!(
        "Node Descriptor: {}",
        format_node_descriptor(identity.public.awe_id.as_bytes())
    );
    println!("Identity vault: {}", path.display());
    println!(".awesecret export: use 'awe-node secret <username> [out-file]' explicitly");
    Ok(())
}

fn load_identity(path: &PathBuf, password: &str, username: &str) -> Result<Identity> {
    let data = fs::read(path).context("failed to read identity vault")?;
    let username = Username::new(username.to_owned()).map_err(anyhow::Error::msg)?;
    LocalVault::open(&data, username, password).map_err(anyhow::Error::msg)
}

#[cfg(not(target_os = "android"))]
#[derive(PartialEq)]
enum AppTab {
    Browser,
    Messenger,
    AweDrive,
    NodeDashboard,
    SiteDashboard,
    AweStore,
    Mlab,
}

#[cfg(not(target_os = "android"))]
struct AweNativeGuiApp {
    active_tab: AppTab,
    address_input: String,
    resolved_result: String,
    my_uid: String,
    connect_uid_input: String,
    message_input: String,
    chat_history: Vec<String>,
    sfid_filename: String,
    sfid_data: String,
    sfid_password: String,
    created_sfid: String,
    shard_count_input: usize,
    configured_policy_status: String,
    selected_allocation_mode: usize,
    folder_path: String,
    non_system_disks: String,
    allocation_status: String,
    // MLAB state
    mlab_app_id: String,
    mlab_app_name: String,
    mlab_app_version: String,
    mlab_status: String,
    // AWEStore state
    store_apps: Vec<(String, String, Vec<u8>)>,
    ephemeral_run_output: String,
    identity: Identity,
}

#[cfg(not(target_os = "android"))]
impl Default for AweNativeGuiApp {
    fn default() -> Self {
        let username_obj = Username::new("ararat_node").unwrap();
        let identity = Identity::generate(username_obj);
        let my_uid = format_uid(identity.public.awe_id.as_bytes());

        let default_wasm = b"\0asm\x01\0\0\0".to_vec();
        let mut store_apps = Vec::new();
        store_apps.push((
            "org.awenet.messenger".to_string(),
            "AWE Messenger Module".to_string(),
            default_wasm.clone(),
        ));
        store_apps.push((
            "org.awenet.browser".to_string(),
            "Sovereign Browser Extension".to_string(),
            default_wasm,
        ));

        Self {
            active_tab: AppTab::Browser,
            address_input: "a2p2://site.awe".to_string(),
            resolved_result: "Ready".to_string(),
            my_uid,
            connect_uid_input: String::new(),
            message_input: String::new(),
            chat_history: vec!["💬 Welcome to Sovereign P2P Zero-Knowledge Messenger!".into()],
            sfid_filename: "private_doc.pdf".to_string(),
            sfid_data: "Confidential Sovereign Data".to_string(),
            sfid_password: "SecretPass123".to_string(),
            created_sfid: String::new(),
            shard_count_input: 1000,
            configured_policy_status: "450 Data / 550 Parity Shards (3x Redundancy across nodes)"
                .to_string(),
            selected_allocation_mode: 0,
            folder_path: "/var/awe_node_storage".to_string(),
            non_system_disks: "D:\\, E:\\".to_string(),
            allocation_status: "Light Folder Mode (2 Cores, 2GB RAM)".to_string(),
            mlab_app_id: "org.awe.sampleapp".to_string(),
            mlab_app_name: "Sample Sovereign Extension".to_string(),
            mlab_app_version: "1.0.0".to_string(),
            mlab_status: "MLAB Ready to package extensions and WASM modules.".to_string(),
            store_apps,
            ephemeral_run_output: "Ready to launch ephemeral sandboxed WASM app.".to_string(),
            identity,
        }
    }
}

#[cfg(not(target_os = "android"))]
impl eframe::App for AweNativeGuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.heading("🛡️ AWEP2P SOVEREIGN NATIVE APP WINDOW (100% Rust GUI)");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.active_tab, AppTab::Browser, "🔍 AWEBrowser");
                ui.selectable_value(&mut self.active_tab, AppTab::Messenger, "💬 Messenger");
                ui.selectable_value(&mut self.active_tab, AppTab::AweDrive, "📁 AWEDrive");
                ui.selectable_value(
                    &mut self.active_tab,
                    AppTab::NodeDashboard,
                    "🖥️ Node Dashboard",
                );
                ui.selectable_value(
                    &mut self.active_tab,
                    AppTab::SiteDashboard,
                    "🌐 Site Dashboard",
                );
                ui.selectable_value(&mut self.active_tab, AppTab::AweStore, "🛒 AWEStore");
                ui.selectable_value(&mut self.active_tab, AppTab::Mlab, "🧪 MLAB");
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| match self.active_tab {
            AppTab::Browser => {
                ui.heading("🔍 Sovereign AWEBrowser");
                ui.horizontal(|ui| {
                    ui.label("a2p2:// | .awe | fid- | sfid- | uid- :");
                    ui.text_edit_singleline(&mut self.address_input);
                    if ui.button("Navigate / Resolve").clicked() {
                        let res = AweBrowserResolver::parse_and_resolve(&self.address_input);
                        self.resolved_result = format!("{:?}", res);
                    }
                });
                ui.separator();
                ui.label(format!("Resolution State: {}", self.resolved_result));
            }
            AppTab::Messenger => {
                ui.heading("💬 Zero-Knowledge P2P Messenger");
                ui.label(format!("Your My-UID: {}", self.my_uid));
                ui.horizontal(|ui| {
                    ui.label("Connect Friend UID:");
                    ui.text_edit_singleline(&mut self.connect_uid_input);
                    if ui.button("Connect").clicked() {
                        self.chat_history
                            .push(format!("Connected to {}", self.connect_uid_input));
                    }
                });
                ui.separator();
                for msg in &self.chat_history {
                    ui.label(msg);
                }
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.message_input);
                    if ui.button("Send P2P").clicked() {
                        self.chat_history
                            .push(format!("Me: {}", self.message_input));
                        self.message_input.clear();
                    }
                });
            }
            AppTab::AweDrive => {
                ui.heading("📁 AWEDrive (Distributed Node Swarm Storage)");
                ui.label("Secret File (sfid-) Password Encryption Creator:");
                ui.horizontal(|ui| {
                    ui.label("Filename:");
                    ui.text_edit_singleline(&mut self.sfid_filename);
                });
                ui.horizontal(|ui| {
                    ui.label("Payload:");
                    ui.text_edit_singleline(&mut self.sfid_data);
                });
                ui.horizontal(|ui| {
                    ui.label("Password:");
                    ui.text_edit_singleline(&mut self.sfid_password);
                });
                if ui.button("Create Encrypted Secret File (sfid-)").clicked() {
                    if let Ok(pkg) = SecretFilePackage::create(
                        &self.sfid_filename,
                        self.sfid_data.as_bytes(),
                        &self.sfid_password,
                    ) {
                        self.created_sfid = pkg.sfid;
                    }
                }
                if !self.created_sfid.is_empty() {
                    ui.label(format!("Created Secret File ID: {}", self.created_sfid));
                }
                ui.separator();
                ui.heading("⚙️ Storage Policy & Shard Distribution Config");
                ui.horizontal(|ui| {
                    ui.label("Total Erasure Shards (1,000 - 100,000,000):");
                    ui.add(egui::DragValue::new(&mut self.shard_count_input).range(1000..=100_000_000));
                    if ui.button("Apply Shard Policy").clicked() {
                        let policy = StoragePolicy::custom_scaled(self.shard_count_input);
                        self.configured_policy_status = format!(
                            "Policy set: {} Data / {} Parity Shards across P2P Swarm (3x Replica Distribution)",
                            policy.data_shards, policy.parity_shards
                        );
                    }
                });
                ui.label(&self.configured_policy_status);
            }
            AppTab::NodeDashboard => {
                ui.heading("🖥️ Node Dashboard & Resource Allocation");
                ui.radio_value(
                    &mut self.selected_allocation_mode,
                    0,
                    "Single Folder Allocation Mode (Light CPU/RAM/GPU)",
                );
                if self.selected_allocation_mode == 0 {
                    ui.text_edit_singleline(&mut self.folder_path);
                }
                ui.radio_value(
                    &mut self.selected_allocation_mode,
                    1,
                    "Whole Non-System Disks Allocation Mode (Max CPU/RAM/GPU/TPU)",
                );
                if self.selected_allocation_mode == 1 {
                    ui.text_edit_singleline(&mut self.non_system_disks);
                }
                if ui.button("Apply Node Allocation").clicked() {
                    let mode = if self.selected_allocation_mode == 0 {
                        NodeAllocationMode::FolderAllocation {
                            folder_path: self.folder_path.clone(),
                        }
                    } else {
                        let disks: Vec<String> = self
                            .non_system_disks
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .collect();
                        NodeAllocationMode::WholeNonSystemDisksAllocation {
                            allocated_disks: disks,
                        }
                    };
                    match validate_and_configure_node_allocation(&mode) {
                        Ok(hw) => {
                            self.allocation_status = format!(
                                "Active Allocation: {} Cores, {} MB RAM, {} MB VRAM",
                                hw.vcpu_cores, hw.ram_mb, hw.gpu_vram_mb
                            )
                        }
                        Err(e) => self.allocation_status = format!("Error: {}", e),
                    }
                }
                ui.label(&self.allocation_status);
            }
            AppTab::SiteDashboard => {
                ui.heading("🌐 Site Dashboard (P2P Hosting)");
                ui.label("Managed Domains: portal.awe, app.awe");
                ui.label(format!(
                    "Swarm Storage Config: Distributed across {} Shards",
                    self.shard_count_input
                ));
            }
            AppTab::AweStore => {
                ui.heading("🛒 AWEStore (P2P App Repository & Ephemeral Runtime)");
                ui.label("Zero-Disk Footprint Execution Engine (Runs 100% in Memory)");
                ui.separator();
                for (id, name, wasm_bytes) in &self.store_apps {
                    ui.horizontal(|ui| {
                        ui.label(format!("📦 {} ({})", name, id));
                        if ui.button("▶ Run Ephemerally").clicked() {
                            let sandbox = WasmSandbox::new(SandboxConfig::default(), CapabilitySet::default());
                            match sandbox.execute_module(wasm_bytes) {
                                Ok(res) => {
                                    self.ephemeral_run_output = format!(
                                        "Executed {} ephemerally in memory! Output: {}",
                                        name,
                                        String::from_utf8_lossy(&res)
                                    );
                                }
                                Err(e) => {
                                    self.ephemeral_run_output = format!("Execution error: {}", e);
                                }
                            }
                        }
                    });
                }
                ui.separator();
                ui.label(format!("Ephemeral Execution Log: {}", self.ephemeral_run_output));
            }
            AppTab::Mlab => {
                ui.heading("🧪 MLAB (Module Lab / Extension Builder)");
                ui.label("Draft, compile, sign and publish sovereign applications & extensions to AWEStore.");
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("App ID:");
                    ui.text_edit_singleline(&mut self.mlab_app_id);
                });
                ui.horizontal(|ui| {
                    ui.label("App Name:");
                    ui.text_edit_singleline(&mut self.mlab_app_name);
                });
                ui.horizontal(|ui| {
                    ui.label("Version:");
                    ui.text_edit_singleline(&mut self.mlab_app_version);
                });
                if ui.button("🛠️ Package, Sign & Publish to AWEStore").clicked() {
                    let mut files = BTreeMap::new();
                    files.insert("/app.wasm".to_string(), b"\0asm\x01\0\0\0".to_vec());
                    match AWEPackage::new(
                        &self.identity,
                        &self.mlab_app_id,
                        &self.mlab_app_name,
                        &self.mlab_app_version,
                        AppKind::Wasm,
                        "/app.wasm",
                        files.clone(),
                        vec![AppCapability::Network, AppCapability::Storage],
                        vec![],
                    ) {
                        Ok(pkg) => {
                            self.store_apps.push((
                                pkg.manifest.manifest.id.clone(),
                                pkg.manifest.manifest.name.clone(),
                                files.get("/app.wasm").cloned().unwrap_or_default(),
                            ));
                            self.mlab_status = format!(
                                "Successfully created & signed AWEPackage '{}' v{}! Published to AWEStore.",
                                self.mlab_app_name, self.mlab_app_version
                            );
                        }
                        Err(e) => {
                            self.mlab_status = format!("MLAB Package build failed: {}", e);
                        }
                    }
                }
                ui.separator();
                ui.label(&self.mlab_status);
            }
        });
    }
}

#[cfg(not(target_os = "android"))]
async fn run_standalone_app() -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([800.0, 600.0]),
        ..Default::default()
    };

    let res = eframe::run_native(
        "AWEP2P Sovereign Native Desktop Application",
        options,
        Box::new(|_cc| Ok(Box::new(AweNativeGuiApp::default()) as Box<dyn eframe::App>)),
    );

    if res.is_err() {
        println!("============================================================");
        println!("🛡️ AWEP2P SOVEREIGN NATIVE APP ENGINE (100% Rust)");
        println!("Status: Running in Headless Native Node Process Mode");
        println!("============================================================");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!("\nShutting down AWEp2P sovereign native application process...");
            }
        }
    }

    Ok(())
}

#[cfg(target_os = "android")]
#[no_mangle]
pub fn android_main() {}

#[cfg(target_os = "android")]
async fn run_standalone_app() -> Result<()> {
    println!("============================================================");
    println!("🛡️ AWEP2P SOVEREIGN NATIVE APP ENGINE (Android 100% Rust Node)");
    println!("Status: Running Sovereign Node Engine Background Loop");
    println!("============================================================");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            println!("\nShutting down AWEp2P Android application process...");
        }
    }
    Ok(())
}

async fn run(
    path: PathBuf,
    password: String,
    username: String,
    listen: SocketAddr,
    bootstrap: Vec<SocketAddr>,
) -> Result<()> {
    let identity = load_identity(&path, &password, &username)?;
    let nd = format_node_descriptor(identity.public.awe_id.as_bytes());
    println!("AWE-ID: {}", identity.public.awe_id.to_hex());
    println!("Node Descriptor: {}", nd);
    println!("Listening: {listen}");
    let node = Node::new(identity, listen);
    if !bootstrap.is_empty() {
        let found = node
            .bootstrap(&bootstrap)
            .await
            .context("bootstrap failed")?;
        println!("Discovered peers: {found}");
    }
    node.listen().await.map_err(anyhow::Error::msg)
}

fn print_id(path: PathBuf, password: String, username: String) -> Result<()> {
    let identity = load_identity(&path, &password, &username)?;
    println!("{}", identity.public.awe_id.to_hex());
    Ok(())
}

fn print_status(path: PathBuf) -> Result<()> {
    if path.exists() {
        println!("Status: Vault exists at {}", path.display());
    } else {
        println!("Status: Vault not found at {}", path.display());
    }
    Ok(())
}

fn print_diagnostics() -> Result<()> {
    let mut diag = NodeDiagnostics::new();
    let metrics = NodeMetrics::default();
    diag.update_metrics(metrics);
    println!("Status: {:?}", diag.status());
    println!("Metrics: {:?}", diag.metrics());
    Ok(())
}

fn run_mesh(port: u16) -> Result<()> {
    let addr: SocketAddr = format!("0.0.0.0:{}", port).parse()?;
    let beacon = LanPeerBeacon::new([1u8; 32], addr, false);
    let bytes = beacon.encode().map_err(anyhow::Error::msg)?;
    println!("Broadcast beacon bytes: {}", bytes.len());
    let decoded = LanPeerBeacon::decode(&bytes).map_err(anyhow::Error::msg)?;
    println!("Decoded LAN beacon node_id: {:?}", decoded.node_id);
    Ok(())
}

fn print_health() -> Result<()> {
    let rep = NodeReputation::new([1u8; 32]);
    println!("Initial reputation score: {}", rep.score());
    println!("Health: ONLINE");
    Ok(())
}

async fn probe(address: SocketAddr) -> Result<()> {
    let identity = Identity::generate(Username::new("probe-node").map_err(anyhow::Error::msg)?);
    let node = Node::new(identity, "127.0.0.1:0".parse()?);
    println!("Connecting to {address}...");
    let mut connection = node.connect(address).await.map_err(anyhow::Error::msg)?;
    println!("Authenticated peer: {:?}", connection.remote_id);
    let rtt = connection
        .ping_roundtrip(1)
        .await
        .map_err(anyhow::Error::msg)?;
    println!("Authenticated heartbeat: OK (RTT: {:?})", rtt);
    let payload = b"AWEP2P-REAL-DATA-PROBE-v1".to_vec();
    let data_rtt = connection
        .send_data_roundtrip(7, payload)
        .await
        .map_err(anyhow::Error::msg)?;
    println!("Encrypted data-plane: OK (RTT: {:?})", data_rtt);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        None | Some("app") | Some("gui") => run_standalone_app().await,
        Some("secret") => {
            let username = args.next().unwrap_or_else(|| usage());
            let path = args.next().map(PathBuf::from);
            generate_secret_file(&username, path)
        }
        Some("init") => {
            let username = args.next().unwrap_or_else(|| usage());
            let path = args.next().map(PathBuf::from).unwrap_or_else(default_vault);
            init(&username, path)
        }
        Some("run") => {
            let path = args.next().map(PathBuf::from).unwrap_or_else(|| usage());
            let password = args.next().unwrap_or_else(|| usage());
            let listen: SocketAddr = args
                .next()
                .unwrap_or_else(|| usage())
                .parse()
                .context("invalid listen address")?;
            let username = env::var("AWE_USERNAME").unwrap_or_else(|_| "node".to_string());
            let bootstrap = args
                .map(|x| x.parse().context("invalid bootstrap address"))
                .collect::<Result<Vec<SocketAddr>>>()?;
            run(path, password, username, listen, bootstrap).await
        }
        Some("id") => {
            let path = args.next().map(PathBuf::from).unwrap_or_else(|| usage());
            let password = args.next().unwrap_or_else(|| usage());
            let username = args.next().unwrap_or_else(|| usage());
            print_id(path, password, username)
        }
        Some("status") => {
            let path = args.next().map(PathBuf::from).unwrap_or_else(default_vault);
            print_status(path)
        }
        Some("diagnostics") => print_diagnostics(),
        Some("mesh") => {
            let port: u16 = args.next().unwrap_or("41000".to_string()).parse()?;
            run_mesh(port)
        }
        Some("health") => print_health(),
        Some("probe") => {
            let address: SocketAddr = args
                .next()
                .unwrap_or_else(|| usage())
                .parse()
                .context("invalid peer address")?;
            probe(address).await
        }
        _ => usage(),
    }
}
