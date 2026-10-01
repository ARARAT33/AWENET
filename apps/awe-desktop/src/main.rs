use awep2p_core::diagnostics::{HealthStatus, NodeDiagnostics};
use eframe::egui;

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Overview,
    Network,
    Storage,
    Security,
}

struct AweDesktop {
    view: View,
    diagnostics: NodeDiagnostics,
    running: bool,
    peer_input: String,
    peers: Vec<String>,
    message: String,
}

impl Default for AweDesktop {
    fn default() -> Self {
        Self {
            view: View::Overview,
            diagnostics: NodeDiagnostics::new(),
            running: true,
            peer_input: String::new(),
            peers: Vec::new(),
            message: "Native desktop application initialized.".into(),
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
        self.diagnostics.update_metrics(self.diagnostics.metrics().clone());

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
                ui.add_space(16.0);
                if ui
                    .button(if self.running { "Stop Node" } else { "Start Node" })
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
                }
                View::Security => {
                    ui.heading("Security");
                    ui.label("Identity, encrypted transport and secret handling are provided by awep2p-core.");
                    ui.label("Private credentials are not displayed by the desktop UI.");
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
