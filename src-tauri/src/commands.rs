//! Tauri commands: the bridge between the UI and the engines.

use std::{path::PathBuf, sync::Arc};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{
    detectors::{self, judge, DetectionReport},
    documents,
    humanizer::{
        self,
        claude::ClaudeClient,
        local_llm::{self, DownloadProgress, LocalLlm, ModelStatus},
        Engine, HumanizeOptions, ProgressFn,
    },
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

fn settings_of(state: &AppState) -> Settings {
    state.settings.lock().unwrap().clone()
}

fn api_client(state: &AppState, s: &Settings) -> Option<Arc<ClaudeClient>> {
    if !s.has_api_key() {
        return None;
    }
    Some(Arc::new(ClaudeClient { http: state.http.clone(), api_key: s.anthropic_api_key.clone(), model: s.model.clone(), effort: s.effort.clone() }))
}

fn models_dir(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir().unwrap_or_else(|_| std::env::temp_dir()).join("models")
}

/// Where a tier's download goes.
fn download_target(app: &AppHandle, tier: &str) -> PathBuf {
    let file = local_llm::spec(tier).map(|m| m.file).unwrap_or(local_llm::DEFAULT_MODEL_FILE);
    models_dir(app).join(file)
}

/// A model shipped inside the installer ("full" build), strongest first.
fn bundled_model(app: &AppHandle) -> Option<PathBuf> {
    let dir = app.path().resource_dir().ok()?.join("models");
    local_llm::CATALOG.iter().map(|m| dir.join(m.file)).find(|p| p.is_file())
}

/// The tier that will be used: the chosen one, or in auto mode the strongest
/// installed tier, or (nothing installed) the strongest the RAM allows.
fn active_tier(app: &AppHandle, s: &Settings) -> &'static str {
    if s.local_model_tier != "auto" {
        if let Some(m) = local_llm::spec(&s.local_model_tier) {
            return m.tier;
        }
    }
    if let Some(m) = local_llm::CATALOG.iter().find(|m| download_target(app, m.tier).is_file()) {
        return m.tier;
    }
    local_llm::recommended_tier(local_llm::total_ram_gb())
}

/// Where the GGUF lives, in priority order: custom path from Settings,
/// a model bundled with the app, then the active tier's downloaded copy.
/// When none exists yet, returns the download target so callers can create it.
fn model_path(app: &AppHandle, s: &Settings) -> PathBuf {
    if !s.local_model_path.is_empty() {
        return PathBuf::from(&s.local_model_path);
    }
    if let Some(b) = bundled_model(app) {
        return b;
    }
    download_target(app, active_tier(app, s))
}

fn model_source(app: &AppHandle, s: &Settings, path: &std::path::Path) -> &'static str {
    if !s.local_model_path.is_empty() {
        "custom"
    } else if bundled_model(app).as_deref() == Some(path) {
        "bundled"
    } else if path.is_file() {
        "downloaded"
    } else {
        "none"
    }
}

/// Loads (or reuses) the offline model. Returns None when no file exists.
async fn local_engine(app: &AppHandle, state: &AppState, s: &Settings) -> Result<Option<Arc<LocalLlm>>, String> {
    let path = model_path(app, s);
    if !path.is_file() {
        return Ok(None);
    }
    let mut slot = state.local_llm.lock().await;
    if let Some(l) = slot.as_ref() {
        if l.path == path {
            return Ok(Some(l.clone()));
        }
    }
    let p = path.clone();
    let loaded = tokio::task::spawn_blocking(move || LocalLlm::load(&p)).await.map_err(|e| e.to_string())??;
    let arc = Arc::new(loaded);
    *slot = Some(arc.clone());
    Ok(Some(arc))
}

/// Picks the engine according to `engine_mode` and what is available.
async fn resolve_engine(app: &AppHandle, state: &AppState) -> Result<Engine, String> {
    let s = settings_of(state);
    match s.engine_mode.as_str() {
        "api" => Ok(api_client(state, &s).map(Engine::Api).unwrap_or(Engine::Rules)),
        "rules" => Ok(Engine::Rules),
        "local" => Ok(local_engine(app, state, &s).await?.map(Engine::Local).unwrap_or(Engine::Rules)),
        _ => {
            if let Some(c) = api_client(state, &s) {
                return Ok(Engine::Api(c));
            }
            Ok(local_engine(app, state, &s).await?.map(Engine::Local).unwrap_or(Engine::Rules))
        }
    }
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    settings_of(&state)
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
    let s = settings_of(&state);
    let client = api_client(&state, &s).ok_or("Enter an Anthropic API key first.")?;
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
    let progress = progress_fn(&app, "text");
    progress(0, 1, "Loading engine");
    let engine = resolve_engine(&app, &state).await?;
    let opts = HumanizeOptions { style, intensity, seed: None };
    let paras = text::paragraphs(&text_in);
    let before = detectors::detect_local(&text_in);
    let outcome = humanizer::humanize_paragraphs(engine, paras, &opts, progress).await;
    let out = outcome.paragraphs.join("\n\n");
    let after = detectors::detect_local(&out);
    Ok(HumanizeTextResult { text: out, engine: outcome.engine, passes: outcome.passes, warnings: outcome.warnings, before, after })
}

#[tauri::command]
pub async fn detect_text(app: AppHandle, state: State<'_, AppState>, text_in: String, with_judge: bool) -> Result<DetectionReport, String> {
    if text::word_count(&text_in) < 3 {
        return Err("Paste some text first.".into());
    }
    let mut report = detectors::detect_local(&text_in);
    let s = settings_of(&state);
    if with_judge && s.ai_judge_enabled {
        let engine = match resolve_engine(&app, &state).await? {
            Engine::Api(client) => judge::judge(&client, &text_in).await,
            Engine::Local(llm) => judge::judge_local(llm, text_in.clone()).await,
            Engine::Rules => judge::error_report("No AI engine available: add an API key or download the offline model in Settings.".into()),
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
    let progress = progress_fn(&app, &path);
    progress(0, 1, "Loading engine");
    let engine = resolve_engine(&app, &state).await?;
    let opts = HumanizeOptions { style, intensity, seed: None };

    let inputs: Vec<String> = doc.paragraphs.iter().map(|p| if p.editable { p.text.clone() } else { String::new() }).collect();
    let before = detectors::detect_local(&doc.plain_text()).overall;
    let outcome = humanizer::humanize_paragraphs(engine, inputs, &opts, progress).await;

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

// ---------------------------------------------------------------------------
// Offline model management
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn local_model_status(app: AppHandle, state: State<'_, AppState>) -> Result<ModelStatus, String> {
    let s = settings_of(&state);
    let path = model_path(&app, &s);
    let meta = std::fs::metadata(&path).ok();
    let loaded = state.local_llm.lock().await.as_ref().map_or(false, |l| l.path == path);
    let ram = local_llm::total_ram_gb();
    let downloading_tier = {
        let t = state.downloader.tier.lock().unwrap();
        if state.downloader.active.load(std::sync::atomic::Ordering::SeqCst) && !t.is_empty() { Some(t.clone()) } else { None }
    };
    let tiers = local_llm::CATALOG
        .iter()
        .map(|m| {
            let p = download_target(&app, m.tier);
            local_llm::TierStatus {
                tier: m.tier.into(),
                label: m.label.into(),
                file: m.file.into(),
                size_bytes: m.size_bytes,
                min_ram_gb: m.min_ram_gb,
                notes: m.notes.into(),
                installed: p.is_file(),
                path: p.to_string_lossy().to_string(),
                fits: ram >= m.min_ram_gb,
            }
        })
        .collect();
    Ok(ModelStatus {
        source: model_source(&app, &s, &path).to_string(),
        active_tier: local_llm::spec_for_file(&path).map(|m| m.tier.to_string()).unwrap_or_else(|| "custom".into()),
        installed: meta.as_ref().map_or(false, |m| m.is_file()),
        name: local_llm::spec_for_file(&path)
            .map(|m| m.label.to_string())
            .unwrap_or_else(|| path.file_stem().and_then(|n| n.to_str()).unwrap_or("").to_string()),
        path: path.to_string_lossy().to_string(),
        size_bytes: meta.map_or(0, |m| m.len()),
        loaded,
        downloading_tier,
        accelerator: LocalLlm::accelerator().to_string(),
        total_ram_gb: ram,
        recommended_tier: local_llm::recommended_tier(ram).to_string(),
        tiers,
    })
}

/// Starts a tier download in the background (default: the recommended tier);
/// progress arrives as `model-download` events. Returns immediately.
#[tauri::command]
pub async fn download_local_model(app: AppHandle, state: State<'_, AppState>, tier: Option<String>) -> Result<String, String> {
    let mut s = settings_of(&state);
    let tier = tier.unwrap_or_else(|| local_llm::recommended_tier(local_llm::total_ram_gb()).to_string());
    let spec = local_llm::spec(&tier).ok_or("Unknown model tier.")?;
    // A download switches the engine to that tier and drops any custom path.
    let mut changed = false;
    if !s.local_model_path.is_empty() {
        s.local_model_path.clear();
        changed = true;
    }
    if s.local_model_tier != "auto" && s.local_model_tier != tier {
        s.local_model_tier = tier.clone();
        changed = true;
    }
    if changed {
        s.save(&app)?;
        *state.settings.lock().unwrap() = s.clone();
    }
    let dest = download_target(&app, &tier);
    if dest.is_file() {
        return Err("That model is already installed.".into());
    }
    let http = state.http.clone();
    let dl = state.downloader.clone();
    let app2 = app.clone();
    let url = spec.url;
    let tier2 = tier.clone();
    tauri::async_runtime::spawn(async move {
        let emitter = app2.clone();
        let _ = local_llm::download(http, &tier2, url, &dest, dl, move |p: DownloadProgress| {
            let _ = emitter.emit("model-download", p);
        })
        .await;
    });
    Ok(tier)
}

/// Switches which installed tier the engine uses ("auto" lets the app pick).
#[tauri::command]
pub async fn select_model_tier(app: AppHandle, state: State<'_, AppState>, tier: String) -> Result<ModelStatus, String> {
    let mut s = settings_of(&state);
    s.local_model_tier = tier;
    s.local_model_path.clear();
    s.normalize();
    s.save(&app)?;
    *state.settings.lock().unwrap() = s;
    *state.local_llm.lock().await = None;
    local_model_status(app, state).await
}

#[tauri::command]
pub fn cancel_model_download(state: State<'_, AppState>) {
    state.downloader.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
}

#[tauri::command]
pub async fn delete_local_model(app: AppHandle, state: State<'_, AppState>, tier: Option<String>) -> Result<(), String> {
    let s = settings_of(&state);
    let path = match tier {
        Some(t) => download_target(&app, &t),
        None => model_path(&app, &s),
    };
    if model_source(&app, &s, &path) == "bundled" {
        return Err("This model is part of the app bundle and cannot be removed separately.".into());
    }
    let mut slot = state.local_llm.lock().await;
    if slot.as_ref().map_or(false, |l| l.path == path) {
        *slot = None;
    }
    drop(slot);
    if path.is_file() {
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    let part = path.with_extension("gguf.part");
    if part.is_file() {
        let _ = std::fs::remove_file(part);
    }
    Ok(())
}

/// Points the local engine at a GGUF the user already has.
#[tauri::command]
pub async fn use_custom_model(app: AppHandle, state: State<'_, AppState>, path: String) -> Result<ModelStatus, String> {
    if !path.to_lowercase().ends_with(".gguf") || !PathBuf::from(&path).is_file() {
        return Err("Pick a .gguf model file.".into());
    }
    let mut s = settings_of(&state);
    s.local_model_path = path;
    s.save(&app)?;
    *state.settings.lock().unwrap() = s;
    *state.local_llm.lock().await = None;
    local_model_status(app, state).await
}

/// Loads the model and times a short generation.
#[tauri::command]
pub async fn test_local_model(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let s = settings_of(&state);
    let t0 = std::time::Instant::now();
    let llm = local_engine(&app, &state, &s).await?.ok_or("No offline model installed yet.")?;
    let load_ms = t0.elapsed().as_millis();
    let t1 = std::time::Instant::now();
    let l = llm.clone();
    let out = tokio::task::spawn_blocking(move || {
        l.generate("Rewrite the user's sentence in plain, natural English. Output only the sentence.", "It is important to note that the utilization of this tool significantly enhances productivity.", 60, 0.7, 42)
    })
    .await
    .map_err(|e| e.to_string())??;
    let gen_ms = t1.elapsed().as_millis().max(1);
    let toks = text::word_count(&out).max(1) as f64 * 1.3;
    Ok(format!("{} ready on {} (load {:.1}s, ~{:.0} tokens/s). Sample: “{}”", llm.name, LocalLlm::accelerator(), load_ms as f64 / 1000.0, toks / (gen_ms as f64 / 1000.0), out))
}

// ---------------------------------------------------------------------------
// Chat
// ---------------------------------------------------------------------------

const CHAT_SYSTEM: &str = "You are Humanizer's assistant, built into a desktop app by Arun AI Studio. Be helpful, direct and natural. Answer in the language the user writes in (Nepali, English or mixed). Keep answers as short as the question allows; use plain formatting. If asked to rewrite or humanize text, do it in a natural human voice without stock phrases.";

#[derive(serde::Deserialize)]
pub struct ChatTurn {
    pub role: String,
    pub content: String,
}

#[derive(Clone, Serialize)]
struct ChatToken {
    id: String,
    text: String,
    done: bool,
    error: Option<String>,
    engine: String,
}

/// Streams the assistant reply for a conversation as `chat-token` events
/// (`id` identifies the reply) and returns the full text at the end.
#[tauri::command]
pub async fn chat_send(app: AppHandle, state: State<'_, AppState>, id: String, turns: Vec<ChatTurn>) -> Result<String, String> {
    let mut history: Vec<(String, String)> = turns
        .into_iter()
        .filter(|t| (t.role == "user" || t.role == "assistant") && !t.content.trim().is_empty())
        .map(|t| (t.role, t.content))
        .collect();
    if history.last().map_or(true, |(r, _)| r != "user") {
        return Err("The last message must be from you.".into());
    }
    // Keep the context bounded for the local model.
    while history.iter().map(|(_, c)| text::word_count(c)).sum::<usize>() > 1800 && history.len() > 2 {
        history.drain(0..2);
    }

    let emit = {
        let app = app.clone();
        let id = id.clone();
        move |text: &str, done: bool, error: Option<String>, engine: &str| {
            let _ = app.emit("chat-token", ChatToken { id: id.clone(), text: text.to_string(), done, error, engine: engine.to_string() });
        }
    };

    match resolve_engine(&app, &state).await? {
        Engine::Api(client) => {
            let reply = client.chat(CHAT_SYSTEM, &history, 4000, Some("medium")).await;
            match reply {
                Ok(t) => {
                    emit(&t, true, None, &client.model);
                    Ok(t)
                }
                Err(e) => {
                    emit("", true, Some(e.clone()), &client.model);
                    Err(e)
                }
            }
        }
        Engine::Local(llm) => {
            let name = llm.name.clone();
            let emit2 = emit.clone();
            let n2 = name.clone();
            let res = tokio::task::spawn_blocking(move || {
                let mut cb = |piece: &str| {
                    emit2(piece, false, None, &n2);
                    true
                };
                llm.generate_chat(CHAT_SYSTEM, &history, 1200, 0.7, 11, &mut cb)
            })
            .await
            .map_err(|e| e.to_string())?;
            match res {
                Ok(t) => {
                    emit("", true, None, &name);
                    Ok(t)
                }
                Err(e) => {
                    emit("", true, Some(e.clone()), &name);
                    Err(e)
                }
            }
        }
        Engine::Rules => Err("Chat needs an AI engine: wait for the offline model to finish downloading, or add an API key in Settings.".into()),
    }
}
