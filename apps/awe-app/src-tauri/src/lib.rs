use std::{fs, process::Command};
#[cfg(not(target_os = "android"))]
use tauri::Manager;

#[cfg(not(target_os = "android"))]
fn start_node(app: &tauri::AppHandle) {
    if let Ok(dir) = app.path().resource_dir() {
        let bin_dir = dir.join("binaries");
        if let Ok(entries) = fs::read_dir(&bin_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                let name = p.file_name().and_then(|x| x.to_str()).unwrap_or("");
                if name.starts_with("awe-node") {
                    let _ = Command::new(&p).spawn();
                    break;
                }
            }
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(not(target_os = "android"))]
    let builder = builder.setup(|app| {
        start_node(app.handle());
        Ok(())
    });
    builder
        .run(tauri::generate_context!())
        .expect("error while running AWEp2P");
}
