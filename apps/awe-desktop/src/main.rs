#![cfg_attr(windows, windows_subsystem = "windows")]

use awep2p_core::diagnostics::{HealthStatus, NodeDiagnostics};
use awep2p_core::identity::{Identity, Username};
use awep2p_core::permissions::CapabilitySet;
use awep2p_core::sandbox::{SandboxConfig, WasmSandbox};
use awep2p_core::storage::StoragePolicy;
use awep2p_core::store::{AWEPackage, AppCapability, AppKind};
use eframe::egui;
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Overview,
    Network,
    Storage,
    Security,
    AweStore,
    Mlab,
}

struct AweDesktop {
    view: View,
    diagnostics: NodeDiagnostics,
    running: bool,
    peer_input: String,
    peers: Vec<String>,
    message: String,
    shard_count_input: usize,
    shard_status: String,
    // Store & MLAB
    store_apps: Vec<(String, String, Vec<u8>)>,
    ephemeral_output: String,
    mlab_app_id: String,
    mlab_app_name: String,
    mlab_app_version: String,
    mlab_status: String,
    identity: Identity,
}

impl Default for AweDesktop {
    fn default() -> Self {
        let identity = Identity::generate(Username::new("desktop_user").unwrap());
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
            view: View::Overview,
            diagnostics: NodeDiagnostics::new(),
            running: true,
            peer_input: String::new(),
            peers: Vec::new(),
            message: "Native desktop application initialized.".into(),
            shard_count_input: 1000,
            shard_status: "Default 1000 Shards configured across P2P network.".into(),
            store_apps,
            ephemeral_output: "Ready to launch ephemeral WASM module.".into(),
            mlab_app_id: "org.awe.desktop_mod".into(),
            mlab_app_name: "Desktop Module".into(),
            mlab_app_version: "1.0.0".into(),
            mlab_status: "MLAB Module Creator Ready.".into(),
            identity,
        }
    }
}

impl AweDesktop {
    fn status_text(status: HealthStatus) -> &'static str {
        match status {
            HealthStatus::Online => "Online",
            HealthStatus::Degraded => "Degraded",
            HealthStatus::Warning => "Warning",
            HealthStatus::Offline => "Offline",
            HealthStatus::Quarantined => "Quarantined",
        }
    }
}

impl eframe::App for AweDesktop {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let metrics = self.diagnostics.metrics().clone();
        self.diagnostics.update_metrics(metrics);

        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("AWEp2P");
                ui.label("Native Desktop");
                ui.separator();
                ui.label(format!(
                    "Node: {}",
                    if self.running { "RUNNING" } else { "STOPPED" }
                ));
            });
        });

        egui::SidePanel::left("navigation")
            .resizable(false)
            .default_width(180.0)
            .show(ctx, |ui| {
                ui.heading("Control");
                ui.separator();
                ui.selectable_value(&mut self.view, View::Overview, "Overview");
                ui.selectable_value(&mut self.view, View::Network, "Network");
                ui.selectable_value(&mut self.view, View::Storage, "Storage");
                ui.selectable_value(&mut self.view, View::Security, "Security");
                ui.selectable_value(&mut self.view, View::AweStore, "AWEStore");
                ui.selectable_value(&mut self.view, View::Mlab, "MLAB");
                ui.add_space(16.0);
                if ui
                    .button(if self.running {
                        "Stop Node"
                    } else {
                        "Start Node"
                    })
                    .clicked()
                {
                    self.running = !self.running;
                    self.message = if self.running {
                        "Node start requested.".into()
                    } else {
                        "Node stop requested.".into()
                    };
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            match self.view {
                View::Overview => {
                    ui.heading("Node Overview");
                    ui.label(format!(
                        "Health: {}",
                        Self::status_text(self.diagnostics.status())
                    ));
                    ui.separator();
                    let m = self.diagnostics.metrics();
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("Uptime: {} s", m.uptime_secs));
                        ui.label(format!("Latency: {} ms", m.latency_ms));
                        ui.label(format!("Bandwidth: {} KB/s", m.bandwidth_kbps));
                        ui.label(format!("CPU: {}%", m.cpu_usage_pct));
                        ui.label(format!("RAM: {}%", m.ram_usage_pct));
                    });
                    ui.add_space(12.0);
                    ui.label(&self.message);
                    ui.small("This is a native Rust GUI. It does not embed a browser or require a terminal.");
                }
                View::Network => {
                    ui.heading("P2P Network");
                    ui.label("Peer management is native and belongs to the AWEp2P application.");
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.peer_input);
                        if ui.button("Add Peer").clicked() && !self.peer_input.trim().is_empty() {
                            self.peers.push(self.peer_input.trim().to_owned());
                            self.peer_input.clear();
                        }
                    });
                    for peer in &self.peers {
                        ui.label(format!("• {peer}"));
                    }
                }
                View::Storage => {
                    ui.heading("Distributed Storage");
                    let m = self.diagnostics.metrics();
                    ui.label(format!(
                        "Used: {} MB",
                        m.used_storage_bytes / 1024 / 1024
                    ));
                    ui.label(format!(
                        "Available: {} MB",
                        m.available_storage_bytes / 1024 / 1024
                    ));
                    ui.label(format!("Replica health: {}%", m.replica_health_pct));
                    ui.separator();
                    ui.heading("Shard Scaling Configuration");
                    ui.horizontal(|ui| {
                        ui.label("Total Shards (1,000 - 100,000,000):");
                        ui.add(egui::DragValue::new(&mut self.shard_count_input).range(1000..=100_000_000));
                        if ui.button("Apply Shard Config").clicked() {
                            let policy = StoragePolicy::custom_scaled(self.shard_count_input);
                            self.shard_status = format!(
                                "Configured {} Data / {} Parity Shards (3x Replication)",
                                policy.data_shards, policy.parity_shards
                            );
                        }
                    });
                    ui.label(&self.shard_status);
                }
                View::Security => {
                    ui.heading("Security");
                    ui.label("Identity, encrypted transport and secret handling are provided by awep2p-core.");
                    ui.label("Private credentials are not displayed by the desktop UI.");
                }
                View::AweStore => {
                    ui.heading("AWEStore (Zero-Disk Ephemeral Execution)");
                    ui.separator();
                    for (id, name, wasm_bytes) in &self.store_apps {
                        ui.horizontal(|ui| {
                            ui.label(format!("📦 {} ({})", name, id));
                            if ui.button("▶ Run Ephemerally").clicked() {
                                let sandbox = WasmSandbox::new(SandboxConfig::default(), CapabilitySet::default());
                                match sandbox.execute_module(wasm_bytes) {
                                    Ok(res) => {
                                        self.ephemeral_output = format!(
                                            "Ran {} in-memory: {}",
                                            name,
                                            String::from_utf8_lossy(&res)
                                        );
                                    }
                                    Err(e) => {
                                        self.ephemeral_output = format!("Execution error: {}", e);
                                    }
                                }
                            }
                        });
                    }
                    ui.separator();
                    ui.label(format!("Execution Output: {}", self.ephemeral_output));
                }
                View::Mlab => {
                    ui.heading("MLAB (Module Lab)");
                    ui.label("Build, sign & publish WASM modules and extensions.");
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("ID:");
                        ui.text_edit_singleline(&mut self.mlab_app_id);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Name:");
                        ui.text_edit_singleline(&mut self.mlab_app_name);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Version:");
                        ui.text_edit_singleline(&mut self.mlab_app_version);
                    });
                    if ui.button("Publish to AWEStore").clicked() {
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
                                    "Published {} v{} to AWEStore!",
                                    self.mlab_app_name, self.mlab_app_version
                                );
                            }
                            Err(e) => {
                                self.mlab_status = format!("Build error: {}", e);
                            }
                        }
                    }
                    ui.separator();
                    ui.label(&self.mlab_status);
                }
            }
        });
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([800.0, 520.0]),
        ..Default::default()
    };

    eframe::run_native(
        "AWEp2P Native Desktop",
        options,
        Box::new(|_cc| Ok(Box::new(AweDesktop::default()))),
    )
}
