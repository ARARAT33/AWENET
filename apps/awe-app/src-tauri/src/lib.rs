#[cfg(not(target_os = "android"))]
use std::{
    fs,
    process::{Child, Command},
    sync::Mutex,
};
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
#[cfg(not(target_os = "android"))]
use tauri::Manager;

#[cfg(not(target_os = "android"))]
struct NodeProcess(Mutex<Child>);

#[cfg(not(target_os = "android"))]
fn start_node(app: &tauri::AppHandle) -> std::io::Result<Option<Child>> {
    let Ok(dir) = app.path().resource_dir() else {
        return Ok(None);
    };
    let bin_dir = dir.join("binaries");
    let Ok(entries) = fs::read_dir(&bin_dir) else {
        return Ok(None);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|value| value.to_str()).unwrap_or("");
        if !name.starts_with("awe-node") {
            continue;
        }
        let mut command = Command::new(&path);
        // The Tauri window is the only UI. The sidecar runs as a background
        // node and must not launch the separate egui desktop window.
        command.env("AWE_NO_NATIVE_UI", "1");
        #[cfg(target_os = "windows")]
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        return command.spawn().map(Some);
    }
    Ok(None)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(not(target_os = "android"))]
    let builder = builder.setup(|app| {
        match start_node(app.handle()) {
            Ok(Some(child)) => {
                app.manage(NodeProcess(Mutex::new(child)));
            }
            Ok(None) => {
                // Development can run against an externally started node, but a
                // packaged desktop app must include its bundled runtime.
                #[cfg(not(debug_assertions))]
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "bundled awe-node executable was not found",
                )
                .into());
            }
            Err(error) => return Err(error.into()),
        }
        Ok(())
    });
    builder
        .build(tauri::generate_context!())
        .expect("error while building AWEp2P")
        .run(|app_handle, event| {
            #[cfg(not(target_os = "android"))]
            if let tauri::RunEvent::Exit = event {
                if let Some(process) = app_handle.try_state::<NodeProcess>() {
                    if let Ok(mut child) = process.0.lock() {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                }
            }
        });
}
