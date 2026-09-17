//! Eavery's desktop shell.
//!
//! The frontend is a renderer: it calls commands and listens to one event
//! (`docs/plan/03-architecture.md` §7). Everything it shows comes from
//! `eavery-core`, which is where the decisions and the history live.
//!
//! M3-T01 is the window and nothing else. The application state and the
//! commands arrive with M3-T03 and M3-T04.
#![deny(unsafe_code)]

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
