//! Tauri commands: the bridge between the UI and the engines.

use std::{path::PathBuf, sync::Arc};

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::{
    detectors::{self, judge, DetectionReport},
    documents,
    humanizer::{self, claude::ClaudeClient, HumanizeOptions, ProgressFn},
    settings::Settings,
    text, AppState,
};

#[derive(Clone, Serialize)]
struct Progress {
    id: String,
    done: usize,
    total: usize,
    stage: String,
}

fn progress_fn(app: &AppHandle, id: &str) -> ProgressFn {
    let app = app.clone();
    let id = id.to_string();
    Arc::new(move |done, total, stage| {
        let _ = app.emit("humanize-progress", Progress { id: id.clone(), done, total, stage: stage.to_string() });
    })
}

fn client_from(state: &AppState) -> Option<Arc<ClaudeClient>> {
    let s = state.settings.lock().unwrap();
    if !s.has_api_key() {
        return None;
    }
    Some(Arc::new(ClaudeClient { http: state.http.clone(), api_key: s.anthropic_api_key.clone(), model: s.model.clone(), effort: s.effort.clone() }))
}

fn judge_enabled(state: &AppState) -> bool {
    state.settings.lock().unwrap().ai_judge_enabled
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
pub fn save_settings(app: AppHandle, state: State<'_, AppState>, mut settings: Settings) -> Result<Settings, String> {
    settings.normalize();
    settings.save(&app)?;
    *state.settings.lock().unwrap() = settings.clone();
    Ok(settings)
}

#[tauri::command]
pub async fn test_connection(state: State<'_, AppState>) -> Result<String, String> {
    let client = client_from(&state).ok_or("Enter an Anthropic API key first.")?;
    client.ping().await
}

#[derive(Serialize)]
pub struct HumanizeTextResult {
    pub text: String,
    pub engine: String,
    pub passes: u32,
    pub warnings: Vec<String>,
    pub before: DetectionReport,
    pub after: DetectionReport,
}

#[tauri::command]
pub async fn humanize_text(app: AppHandle, state: State<'_, AppState>, text_in: String, style: String, intensity: String) -> Result<HumanizeTextResult, String> {
    if text::word_count(&text_in) < 3 {
        return Err("Paste some text first.".into());
    }
    let client = client_from(&state);
    let opts = HumanizeOptions { style, intensity };
    let paras = text::paragraphs(&text_in);
    let before = detectors::detect_local(&text_in);
    let outcome = humanizer::humanize_paragraphs(client, paras, &opts, progress_fn(&app, "text")).await;
    let out = outcome.paragraphs.join("\n\n");
    let after = detectors::detect_local(&out);
    Ok(HumanizeTextResult { text: out, engine: outcome.engine, passes: outcome.passes, warnings: outcome.warnings, before, after })
}

#[tauri::command]
pub async fn detect_text(state: State<'_, AppState>, text_in: String, with_judge: bool) -> Result<DetectionReport, String> {
    if text::word_count(&text_in) < 3 {
        return Err("Paste some text first.".into());
    }
    let mut report = detectors::detect_local(&text_in);
    if with_judge && judge_enabled(&state) {
        let engine = match client_from(&state) {
            Some(client) => judge::judge(&client, &text_in).await,
            None => judge::error_report("No API key configured; AI Judge skipped.".into()),
        };
        let mut engines = report.engines;
        engines.push(engine);
        report = detectors::combine(&text_in, engines);
    }
    Ok(report)
}

#[derive(Serialize)]
pub struct DocInfo {
    pub path: String,
    pub name: String,
    pub kind: documents::DocKind,
    pub words: usize,
    pub paragraphs: usize,
    pub editable_paragraphs: usize,
    pub preview: String,
    pub score: f64,
}

#[tauri::command]
pub async fn inspect_document(path: String) -> Result<DocInfo, String> {
    let p = PathBuf::from(&path);
    let doc = tokio::task::spawn_blocking(move || documents::load(&p)).await.map_err(|e| e.to_string())??;
    let plain = doc.plain_text();
    let preview: String = plain.chars().take(280).collect();
    let score = detectors::detect_local(&plain).overall;
    Ok(DocInfo {
        name: doc.path.file_name().and_then(|n| n.to_str()).unwrap_or("document").to_string(),
        kind: doc.kind,
        words: doc.word_count(),
        paragraphs: doc.paragraphs.len(),
        editable_paragraphs: doc.paragraphs.iter().filter(|p| p.editable && text::word_count(&p.text) >= 6).count(),
        preview,
        score,
        path,
    })
}

#[tauri::command]
pub async fn extract_document_text(path: String) -> Result<String, String> {
    let p = PathBuf::from(path);
    let doc = tokio::task::spawn_blocking(move || documents::load(&p)).await.map_err(|e| e.to_string())??;
    Ok(doc.plain_text())
}

#[derive(Serialize)]
pub struct DocResult {
    pub out_path: String,
    pub changed: usize,
    pub total: usize,
    pub engine: String,
    pub passes: u32,
    pub warnings: Vec<String>,
    pub before_score: f64,
    pub after_score: f64,
}

#[tauri::command]
pub async fn humanize_document(app: AppHandle, state: State<'_, AppState>, path: String, out_path: String, style: String, intensity: String) -> Result<DocResult, String> {
    let src = PathBuf::from(&path);
    let doc = tokio::task::spawn_blocking(move || documents::load(&src)).await.map_err(|e| e.to_string())??;
    let client = client_from(&state);
    let opts = HumanizeOptions { style, intensity };

    // Only editable paragraphs go through the engine; others pass straight back.
    let inputs: Vec<String> = doc.paragraphs.iter().map(|p| if p.editable { p.text.clone() } else { String::new() }).collect();
    let before = detectors::detect_local(&doc.plain_text()).overall;
    let outcome = humanizer::humanize_paragraphs(client, inputs, &opts, progress_fn(&app, &path)).await;

    let new_texts: Vec<String> = doc
        .paragraphs
        .iter()
        .zip(outcome.paragraphs)
        .map(|(p, t)| if p.editable && !t.trim().is_empty() { t } else { p.text.clone() })
        .collect();
    let changed = doc.paragraphs.iter().zip(&new_texts).filter(|(p, t)| p.text.trim() != t.trim()).count();
    let after_text = doc.paragraphs.iter().zip(&new_texts).map(|(p, t)| format!("{}{}", p.prefix, t)).collect::<Vec<_>>().join("\n\n");
    let after = detectors::detect_local(&after_text).overall;

    let out = PathBuf::from(&out_path);
    let total = doc.paragraphs.len();
    tokio::task::spawn_blocking(move || doc.save_with(&new_texts, &out)).await.map_err(|e| e.to_string())??;

    Ok(DocResult { out_path, changed, total, engine: outcome.engine, passes: outcome.passes, warnings: outcome.warnings, before_score: before, after_score: after })
}

/// Saves text-box output as .docx / .txt / .md.
#[tauri::command]
pub async fn save_text_as(text_in: String, out_path: String) -> Result<(), String> {
    let out = PathBuf::from(&out_path);
    let kind = documents::kind_of(&out)?;
    tokio::task::spawn_blocking(move || match kind {
        documents::DocKind::Docx => documents::write_new_docx(&text::paragraphs(&text_in), &out),
        documents::DocKind::Pdf => Err("Choose .docx, .txt or .md".into()),
        _ => std::fs::write(&out, text_in).map_err(|e| e.to_string()),
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub developer: String,
    pub company: String,
    pub website: String,
    /// Where the latest installers live.
    pub downloads: String,
    pub os: String,
    pub arch: String,
}

#[tauri::command]
pub fn app_info(app: AppHandle) -> AppInfo {
    AppInfo {
        name: app.package_info().name.clone(),
        version: app.package_info().version.to_string(),
        developer: "Arun Pandey".into(),
        company: "Arun AI Studio".into(),
        website: "https://arun-pandey.com.np/".into(),
        downloads: "https://github.com/00Arun/humanizer/tree/main/releases".into(),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
    }
}
