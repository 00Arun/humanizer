mod commands;
mod detectors;
mod documents;
mod humanizer;
mod settings;
mod text;

use std::sync::{Arc, Mutex};

use humanizer::local_llm::{Downloader, LocalLlm};
use settings::Settings;
use tauri::Manager;

/// Shared state for the whole app: persisted settings, one HTTP client, the
/// lazily loaded offline model and the model downloader.
pub struct AppState {
    pub settings: Mutex<Settings>,
    pub http: reqwest::Client,
    pub local_llm: tokio::sync::Mutex<Option<Arc<LocalLlm>>>,
    pub downloader: Arc<Downloader>,
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
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
                local_llm: tokio::sync::Mutex::new(None),
                downloader: Arc::new(Downloader::default()),
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
            commands::local_model_status,
            commands::download_local_model,
            commands::cancel_model_download,
            commands::delete_local_model,
            commands::select_model_tier,
            commands::use_custom_model,
            commands::test_local_model,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Humanizer");
}
