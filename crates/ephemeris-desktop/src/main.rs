// No console window behind the app on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![forbid(unsafe_code)]

fn main() {
    ephemeris_desktop::configure(tauri::Builder::default())
        .run(tauri::generate_context!())
        .expect("EPHEMERIS failed to start");
}
