use std::{fs, process::Command};
#[cfg(not(target_os = "android"))]
use tauri::Manager;

#[cfg(not(target_os = "android"))]
fn start_node(app: &tauri::AppHandle) {
    let resource_dir = match app.path().resource_dir() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("AWENET node was not started: cannot locate app resources: {error}");
            return;
        }
    };
    let bin_dir = resource_dir.join("binaries");
    let entries = match fs::read_dir(&bin_dir) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!(
                "AWENET node was not started: bundled sidecar directory {} is unavailable: {error}",
                bin_dir.display()
            );
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|x| x.to_str()).unwrap_or("");
        if name.starts_with("awe-node") && path.is_file() {
            match Command::new(&path).env("AWE_NO_NATIVE_UI", "1").spawn() {
                Ok(_child) => {
                    eprintln!("AWENET backend sidecar started from {}", path.display());
                    return;
                }
                Err(error) => eprintln!(
                    "Failed to start AWENET backend sidecar {}: {error}",
                    path.display()
                ),
            }
        }
    }
    eprintln!(
        "AWENET node was not started: no bundled awe-node sidecar found in {}",
        bin_dir.display()
    );
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
        .expect("error while running AWENET");
}
