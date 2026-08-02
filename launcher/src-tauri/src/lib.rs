mod permissions;
mod supervisor;

use permissions::{capture_permission_status, request_capture_permission};
use supervisor::{
    Supervisor, default_output_directory, enumerate_windows, list_agents, start_capture,
    stop_capture,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Supervisor::default())
        .invoke_handler(tauri::generate_handler![
            enumerate_windows,
            list_agents,
            default_output_directory,
            start_capture,
            stop_capture,
            capture_permission_status,
            request_capture_permission
        ])
        .run(tauri::generate_context!())
        .expect("error while running OmniInlet");
}
