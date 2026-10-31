//! The offline AI engine: a small open-weight instruct model run in-process
//! through llama.cpp (Metal on Apple Silicon, multi-threaded CPU elsewhere),
//! plus the download manager that fetches the GGUF file once.

use std::{
    fs,
    io::Write,
    num::NonZeroU32,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use futures::StreamExt;
use llama_cpp_2::{
    context::params::LlamaContextParams,
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{params::LlamaModelParams, LlamaChatMessage, LlamaModel},
    sampling::LlamaSampler,
};
use once_cell::sync::OnceCell;
use serde::Serialize;

/// The model shipped by default: Qwen3-4B-Instruct (2507), 4-bit GGUF.
/// Apache-2.0, strong at rewriting for its size, ~2.5 GB on disk.
pub const DEFAULT_MODEL_FILE: &str = "Qwen3-4B-Instruct-2507-Q4_K_M.gguf";
pub const DEFAULT_MODEL_URL: &str =
    "https://huggingface.co/unsloth/Qwen3-4B-Instruct-2507-GGUF/resolve/main/Qwen3-4B-Instruct-2507-Q4_K_M.gguf";
pub const DEFAULT_MODEL_LABEL: &str = "Qwen3 4B Instruct (Q4_K_M, 2.5 GB)";

static BACKEND: OnceCell<LlamaBackend> = OnceCell::new();

fn backend() -> Result<&'static LlamaBackend, String> {
    BACKEND.get_or_try_init(|| {
        let mut b = LlamaBackend::init().map_err(|e| format!("llama.cpp init failed: {e}"))?;
        b.void_logs();
        Ok(b)
    })
}

pub struct LocalLlm {
    model: LlamaModel,
    pub path: PathBuf,
    pub name: String,
    threads: i32,
    n_ctx: u32,
}

impl LocalLlm {
    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.is_file() {
            return Err("Local model file not found. Download it in Settings.".into());
        }
        let backend = backend()?;
        let gpu_layers: u32 = if cfg!(target_os = "macos") { 999 } else { 0 };
        let params = LlamaModelParams::default().with_n_gpu_layers(gpu_layers);
        let model = LlamaModel::load_from_file(backend, path, &params).map_err(|e| format!("Could not load model: {e}"))?;
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 12) as i32;
        let n_ctx = model.n_ctx_train().min(4096).max(1024);
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("local model").to_string();
        Ok(Self { model, path: path.to_path_buf(), name, threads, n_ctx })
    }

    pub fn accelerator() -> &'static str {
        if cfg!(target_os = "macos") { "Metal GPU" } else { "CPU" }
    }

    /// One chat turn; returns the assistant text. Synchronous and CPU/GPU
    /// bound, so callers run it inside `spawn_blocking`.
    pub fn generate(&self, system: &str, user: &str, max_new: usize, temperature: f32, seed: u32) -> Result<String, String> {
        let tmpl = self.model.chat_template(None).map_err(|e| format!("chat template: {e}"))?;
        let chat = [
            LlamaChatMessage::new("system".into(), system.into()).map_err(|e| e.to_string())?,
            LlamaChatMessage::new("user".into(), user.into()).map_err(|e| e.to_string())?,
        ];
        let prompt = self.model.apply_chat_template(&tmpl, &chat, true).map_err(|e| format!("apply template: {e}"))?;

        let vocab = self.model.vocab();
        let mut tokens = vocab.tokenize(prompt.as_bytes(), false, true);
        if vocab.should_add_bos() && tokens.first() != Some(&vocab.bos()) {
            tokens.insert(0, vocab.bos());
        }
        let n_ctx = self.n_ctx as usize;
        if tokens.len() + max_new + 8 > n_ctx {
            return Err(format!("Passage too long for the local model ({} tokens; limit ~{}).", tokens.len(), n_ctx - max_new - 8));
        }

        let n_batch = tokens.len().max(512) as u32;
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(self.n_ctx))
            .with_n_batch(n_batch)
            .with_n_threads(self.threads)
            .with_n_threads_batch(self.threads);
        let mut ctx = self.model.new_context(backend()?, ctx_params).map_err(|e| format!("context: {e}"))?;

        let mut batch = LlamaBatch::new(n_batch as usize, 1);
        let last = tokens.len() - 1;
        for (i, t) in tokens.iter().enumerate() {
            batch.add(*t, i as i32, &[0], i == last).map_err(|e| e.to_string())?;
        }
        ctx.decode(&mut batch).map_err(|e| format!("decode: {e}"))?;

        let mut sampler = LlamaSampler::chain_simple([
            LlamaSampler::penalties(self.model.n_vocab(), 64, 1.08, 0.0, 0.0),
            LlamaSampler::top_k(40),
            LlamaSampler::top_p(0.92, 1),
            LlamaSampler::min_p(0.05, 1),
            LlamaSampler::temp(temperature),
            LlamaSampler::dist(seed),
        ]);

        let mut out: Vec<u8> = Vec::with_capacity(max_new * 4);
        let mut pos = tokens.len() as i32;
        for _ in 0..max_new {
            let token = sampler.sample(&ctx, batch.n_tokens() - 1);
            if vocab.is_eog(token) {
                break;
            }
            out.extend(vocab.token_to_piece(token, true, None));
            batch.clear();
            batch.add(token, pos, &[0], true).map_err(|e| e.to_string())?;
            pos += 1;
            ctx.decode(&mut batch).map_err(|e| format!("decode: {e}"))?;
        }
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    }
}

// ---------------------------------------------------------------------------
// Model store / download
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ModelStatus {
    pub installed: bool,
    pub path: String,
    pub name: String,
    pub size_bytes: u64,
    pub loaded: bool,
    pub downloading: bool,
    pub accelerator: String,
    pub default_label: String,
    pub default_size_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: u64,
    pub done: bool,
    pub error: Option<String>,
}

pub struct Downloader {
    pub active: AtomicBool,
    pub cancel: AtomicBool,
}

impl Default for Downloader {
    fn default() -> Self {
        Self { active: AtomicBool::new(false), cancel: AtomicBool::new(false) }
    }
}

/// Streams the GGUF into `<dest>.part` (resuming if a partial file exists),
/// then renames it into place. `report` receives progress updates.
pub async fn download(
    http: reqwest::Client,
    url: &str,
    dest: &Path,
    dl: Arc<Downloader>,
    report: impl Fn(DownloadProgress) + Send + Sync + 'static,
) -> Result<(), String> {
    if dl.active.swap(true, Ordering::SeqCst) {
        return Err("A download is already running.".into());
    }
    dl.cancel.store(false, Ordering::SeqCst);
    let result = download_inner(http, url, dest, &dl, &report).await;
    dl.active.store(false, Ordering::SeqCst);
    match &result {
        Ok(()) => report(DownloadProgress { downloaded: 0, total: 0, done: true, error: None }),
        Err(e) => report(DownloadProgress { downloaded: 0, total: 0, done: true, error: Some(e.clone()) }),
    }
    result
}

async fn download_inner(
    http: reqwest::Client,
    url: &str,
    dest: &Path,
    dl: &Downloader,
    report: &(impl Fn(DownloadProgress) + Send + Sync),
) -> Result<(), String> {
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let part = dest.with_extension("gguf.part");
    let mut have: u64 = part.metadata().map(|m| m.len()).unwrap_or(0);

    let mut req = http.get(url);
    if have > 0 {
        req = req.header("Range", format!("bytes={have}-"));
    }
    let resp = req.send().await.map_err(|e| format!("Download failed: {e}"))?;
    let status = resp.status();
    if !(status.is_success() || status.as_u16() == 206) {
        return Err(format!("Download failed: HTTP {status}"));
    }
    let resumed = status.as_u16() == 206;
    if !resumed {
        have = 0;
    }
    let total = resp.content_length().unwrap_or(0) + have;

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(resumed)
        .write(true)
        .truncate(!resumed)
        .open(&part)
        .map_err(|e| e.to_string())?;

    let mut downloaded = have;
    let mut last_report = std::time::Instant::now();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        if dl.cancel.load(Ordering::SeqCst) {
            return Err("Download cancelled.".into());
        }
        let chunk = chunk.map_err(|e| format!("Download interrupted: {e}"))?;
        file.write_all(&chunk).map_err(|e| e.to_string())?;
        downloaded += chunk.len() as u64;
        if last_report.elapsed().as_millis() > 200 {
            report(DownloadProgress { downloaded, total, done: false, error: None });
            last_report = std::time::Instant::now();
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);
    if total > 0 && downloaded < total {
        return Err("Download ended early; run it again to resume.".into());
    }
    fs::rename(&part, dest).map_err(|e| e.to_string())?;
    Ok(())
}
