mod commands;
mod detectors;
mod documents;
mod humanizer;
mod settings;
mod text;

use std::sync::Mutex;

use settings::Settings;
use tauri::Manager;

/// Shared state for the whole app: persisted settings plus one HTTP client.
pub struct AppState {
    pub settings: Mutex<Settings>,
    pub http: reqwest::Client,
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let settings = Settings::load(app.handle());
            let http = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(900))
                .user_agent(format!("Humanizer/{}", app.package_info().version))
                .build()
                .expect("http client");
            app.manage(AppState {
                settings: Mutex::new(settings),
                http,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::test_connection,
            commands::humanize_text,
            commands::detect_text,
            commands::inspect_document,
            commands::extract_document_text,
            commands::humanize_document,
            commands::save_text_as,
            commands::app_info,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Humanizer");
}
