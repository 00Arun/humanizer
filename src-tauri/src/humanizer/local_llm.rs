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
use once_cell::sync::{Lazy, OnceCell};
use regex::Regex;
use serde::Serialize;

/// One downloadable model tier.
pub struct ModelSpec {
    pub tier: &'static str,
    pub label: &'static str,
    pub file: &'static str,
    pub url: &'static str,
    pub size_bytes: u64,
    /// Total system RAM needed to run it comfortably.
    pub min_ram_gb: u32,
    pub notes: &'static str,
    /// Qwen3 hybrid models think by default; we switch that off per request.
    pub no_think: bool,
}

/// Strongest first. Auto mode picks the strongest tier the machine's RAM
/// allows. Sizes are the GGUF files on Hugging Face (public, ungated).
pub const CATALOG: &[ModelSpec] = &[
    ModelSpec {
        tier: "max",
        label: "Gemma 4 31B (maximum quality)",
        file: "gemma-4-31B-it-Q4_K_M.gguf",
        url: "https://huggingface.co/unsloth/gemma-4-31B-it-GGUF/resolve/main/gemma-4-31B-it-Q4_K_M.gguf",
        size_bytes: 18_320_000_000,
        min_ram_gb: 32,
        notes: "Google's largest Gemma 4. Needs a 32 GB machine; slow on CPU-only PCs.",
        no_think: false,
    },
    ModelSpec {
        tier: "large",
        label: "Gemma 4 26B-A4B (large, fast)",
        file: "gemma-4-26B-A4B-it-UD-Q4_K_M.gguf",
        url: "https://huggingface.co/unsloth/gemma-4-26B-A4B-it-GGUF/resolve/main/gemma-4-26B-A4B-it-UD-Q4_K_M.gguf",
        size_bytes: 16_950_000_000,
        min_ram_gb: 32,
        notes: "Mixture-of-experts: 26B knowledge, 4B active, so it runs as fast as a small model. Needs 32 GB RAM for the file.",
        no_think: false,
    },
    ModelSpec {
        tier: "best",
        label: "Gemma 4 12B (best for 16 GB)",
        file: "gemma-4-12b-it-Q4_K_M.gguf",
        url: "https://huggingface.co/unsloth/gemma-4-12b-it-GGUF/resolve/main/gemma-4-12b-it-Q4_K_M.gguf",
        size_bytes: 7_120_000_000,
        min_ram_gb: 16,
        notes: "Recommended on 16 GB machines. ~15 tokens/s on Apple Silicon.",
        no_think: false,
    },
    ModelSpec {
        tier: "balanced",
        label: "Gemma 4 E4B (balanced)",
        file: "gemma-4-E4B_q4_0-it.gguf",
        url: "https://huggingface.co/google/gemma-4-E4B-it-qat-q4_0-gguf/resolve/main/gemma-4-E4B_q4_0-it.gguf",
        size_bytes: 5_150_000_000,
        min_ram_gb: 12,
        notes: "Google's quantisation-aware 4B-effective model. Good quality at half the size.",
        no_think: false,
    },
    ModelSpec {
        tier: "light",
        label: "Qwen3 4B (light)",
        file: "Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
        url: "https://huggingface.co/unsloth/Qwen3-4B-Instruct-2507-GGUF/resolve/main/Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
        size_bytes: 2_497_281_120,
        min_ram_gb: 8,
        notes: "Runs on 8 GB machines and older laptops. Plainer output, very fast.",
        no_think: false,
    },
    ModelSpec {
        tier: "classic",
        label: "Gemma 3 12B (previous generation)",
        file: "gemma-3-12b-it-qat-Q4_K_M.gguf",
        url: "https://huggingface.co/unsloth/gemma-3-12b-it-qat-GGUF/resolve/main/gemma-3-12b-it-qat-Q4_K_M.gguf",
        size_bytes: 7_300_000_000,
        min_ram_gb: 16,
        notes: "Kept for people who already downloaded it. Gemma 4 12B is the better pick at the same size.",
        no_think: false,
    },
];

/// Tiers offered for download in the UI (everything except the legacy one,
/// which only shows when it is already on disk).
pub fn is_legacy(tier: &str) -> bool {
    tier == "classic"
}

/// The light tier; also the file the test-suite and scripts refer to.
pub const DEFAULT_MODEL_FILE: &str = "Qwen3-4B-Instruct-2507-Q4_K_M.gguf";

pub fn spec(tier: &str) -> Option<&'static ModelSpec> {
    CATALOG.iter().find(|m| m.tier == tier)
}

pub fn spec_for_file(path: &Path) -> Option<&'static ModelSpec> {
    let name = path.file_name()?.to_str()?;
    CATALOG.iter().find(|m| m.file == name)
}

/// Strongest tier the machine can hold.
pub fn recommended_tier(total_ram_gb: u32) -> &'static str {
    // On 32 GB machines prefer the fast MoE over the dense 31B: near-equal
    // quality for rewriting at several times the speed.
    if total_ram_gb >= 32 {
        return "large";
    }
    CATALOG.iter().filter(|m| !is_legacy(m.tier)).find(|m| total_ram_gb >= m.min_ram_gb).map(|m| m.tier).unwrap_or("light")
}

pub fn total_ram_gb() -> u32 {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    ((sys.total_memory() as f64) / 1_073_741_824.0).round() as u32
}

static THINK_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<think>.*?</think>\s*").unwrap());

static BACKEND: OnceCell<LlamaBackend> = OnceCell::new();

fn backend() -> Result<&'static LlamaBackend, String> {
    BACKEND.get_or_try_init(|| {
        let mut b = LlamaBackend::init().map_err(|e| format!("llama.cpp init failed: {e}"))?;
        b.void_logs();
        Ok(b)
    })
}

/// Prompt format used when llama.cpp's built-in template renderer cannot
/// handle a model's (often tool-calling, Jinja-heavy) chat template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Gemma4,
    Gemma,
    ChatMl,
}

pub struct LocalLlm {
    model: LlamaModel,
    pub path: PathBuf,
    pub name: String,
    pub architecture: String,
    threads: i32,
    n_ctx: u32,
    no_think: bool,
    family: Family,
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
        // Bigger models get a smaller context so the KV cache fits next to
        // the weights on 16 GB machines; paragraphs never need more.
        let file_gb = std::fs::metadata(path).map(|m| m.len() as f64 / 1e9).unwrap_or(0.0);
        let cap = if file_gb > 6.0 { 2048 } else { 4096 };
        let n_ctx = model.n_ctx_train().min(cap).max(1024);
        let name = spec_for_file(path)
            .map(|s| s.label.to_string())
            .unwrap_or_else(|| path.file_stem().and_then(|s| s.to_str()).unwrap_or("local model").to_string());
        // Any Qwen3 hybrid model (not the "-2507" instruct releases) thinks unless told not to.
        let fname = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let no_think = spec_for_file(path).map(|s| s.no_think).unwrap_or(fname.contains("Qwen3") && !fname.contains("2507"));
        let architecture = model.meta_val_str("general.architecture").unwrap_or_default();
        let family = match architecture.as_str() {
            "gemma4" => Family::Gemma4,
            a if a.starts_with("gemma") => Family::Gemma,
            _ => Family::ChatMl,
        };
        Ok(Self { model, path: path.to_path_buf(), name, architecture, threads, n_ctx, no_think, family })
    }

    /// Renders the conversation by hand for models whose template llama.cpp
    /// cannot apply. Formats follow the models' own chat templates.
    fn render_fallback(&self, system: &str, turns: &[(String, String)]) -> String {
        let mut p = String::new();
        match self.family {
            Family::Gemma4 => {
                p.push_str("<|turn>system\n");
                p.push_str(system.trim());
                p.push_str("<turn|>\n");
                for (role, content) in turns {
                    let r = if role == "assistant" { "model" } else { role.as_str() };
                    p.push_str(&format!("<|turn>{r}\n{}<turn|>\n", content.trim()));
                }
                // Generation prompt with thinking disabled: an empty thought channel.
                p.push_str("<|turn>model\n<|channel>thought\n<channel|>");
            }
            Family::Gemma => {
                // Gemma 2/3 have no system role; it is folded into the first user turn.
                let mut first = true;
                for (role, content) in turns {
                    let r = if role == "assistant" { "model" } else { role.as_str() };
                    let body = if first && r == "user" { format!("{}\n\n{}", system.trim(), content.trim()) } else { content.trim().to_string() };
                    first = false;
                    p.push_str(&format!("<start_of_turn>{r}\n{body}<end_of_turn>\n"));
                }
                p.push_str("<start_of_turn>model\n");
            }
            Family::ChatMl => {
                p.push_str(&format!("<|im_start|>system\n{}<|im_end|>\n", system.trim()));
                for (role, content) in turns {
                    p.push_str(&format!("<|im_start|>{role}\n{}<|im_end|>\n", content.trim()));
                }
                p.push_str("<|im_start|>assistant\n");
            }
        }
        p
    }

    pub fn accelerator() -> &'static str {
        if cfg!(target_os = "macos") { "Metal GPU" } else { "CPU" }
    }

    /// One chat turn; returns the assistant text. Synchronous and CPU/GPU
    /// bound, so callers run it inside `spawn_blocking`.
    pub fn generate(&self, system: &str, user: &str, max_new: usize, temperature: f32, seed: u32) -> Result<String, String> {
        let turns = [("user".to_string(), user.to_string())];
        self.generate_chat(system, &turns, max_new, temperature, seed, &mut |_| true)
    }

    /// Multi-turn generation with streaming. `turns` alternate user/assistant,
    /// ending with the user's latest message. `on_token` receives each piece
    /// of text as it is produced and returns false to stop early.
    pub fn generate_chat(
        &self,
        system: &str,
        turns: &[(String, String)],
        max_new: usize,
        temperature: f32,
        seed: u32,
        on_token: &mut dyn FnMut(&str) -> bool,
    ) -> Result<String, String> {
        let tmpl = self.model.chat_template(None).ok();
        let mut chat = Vec::with_capacity(turns.len() + 1);
        chat.push(LlamaChatMessage::new("system".into(), system.into()).map_err(|e| e.to_string())?);
        let last = turns.len().saturating_sub(1);
        for (i, (role, content)) in turns.iter().enumerate() {
            let content = if self.no_think && i == last && role == "user" { format!("{content}\n/no_think") } else { content.clone() };
            chat.push(LlamaChatMessage::new(role.clone(), content).map_err(|e| e.to_string())?);
        }
        let prompt = match tmpl.as_ref().ok_or(()).and_then(|t| self.model.apply_chat_template(t, &chat, true).map_err(|_| ())) {
            Ok(p) => p,
            // Newer templates (Gemma 4's tool-calling Jinja, for one) are
            // beyond llama.cpp's built-in renderer; use our own.
            Err(_) => self.render_fallback(system, turns),
        };

        let vocab = self.model.vocab();
        let mut tokens = vocab.tokenize(prompt.as_bytes(), false, true);
        if vocab.should_add_bos() && tokens.first() != Some(&vocab.bos()) {
            tokens.insert(0, vocab.bos());
        }
        let n_ctx = self.n_ctx as usize;
        if tokens.len() + max_new + 8 > n_ctx {
            return Err(format!("Passage too long for the local model ({} tokens; limit ~{}).", tokens.len(), n_ctx - max_new - 8));
        }

        // Fit the context to this request; retry once smaller if memory is tight.
        let needed = (tokens.len() + max_new + 16) as u32;
        let mut ctx_size = needed.max(1024).min(self.n_ctx);
        let n_batch = tokens.len().max(512) as u32;
        let mut attempt = 0;
        let (mut ctx, mut batch) = loop {
            let ctx_params = LlamaContextParams::default()
                .with_n_ctx(NonZeroU32::new(ctx_size))
                .with_n_batch(n_batch)
                .with_n_threads(self.threads)
                .with_n_threads_batch(self.threads);
            let created = self.model.new_context(backend()?, ctx_params).map_err(|e| format!("context: {e}"));
            let decoded = created.and_then(|mut ctx| {
                let mut batch = LlamaBatch::new(n_batch as usize, 1);
                let last = tokens.len() - 1;
                for (i, t) in tokens.iter().enumerate() {
                    batch.add(*t, i as i32, &[0], i == last).map_err(|e| e.to_string())?;
                }
                ctx.decode(&mut batch).map_err(|e| format!("decode: {e}"))?;
                Ok((ctx, batch))
            });
            match decoded {
                Ok(pair) => break pair,
                Err(e) if attempt == 0 && ctx_size > needed => {
                    // Allocation failures show up as decode errors; shrink and retry.
                    attempt += 1;
                    ctx_size = needed;
                    let _ = e;
                }
                Err(e) if e.contains("-3") || e.contains("alloc") || e.contains("memory") => {
                    return Err(format!("Not enough free memory to run {} right now. Close other heavy apps, or pick a smaller model tier in Settings. ({e})", self.name));
                }
                Err(e) => return Err(e),
            }
        };

        let mut sampler = LlamaSampler::chain_simple([
            LlamaSampler::penalties(self.model.n_vocab(), 64, 1.08, 0.0, 0.0),
            LlamaSampler::top_k(40),
            LlamaSampler::top_p(0.92, 1),
            LlamaSampler::min_p(0.05, 1),
            LlamaSampler::temp(temperature),
            LlamaSampler::dist(seed),
        ]);

        let mut out: Vec<u8> = Vec::with_capacity(max_new * 4);
        // Bytes not yet flushed to the callback (a UTF-8 char may span tokens).
        let mut pending: Vec<u8> = Vec::new();
        let mut in_think = false;
        let mut pos = tokens.len() as i32;
        for _ in 0..max_new {
            let token = sampler.sample(&ctx, batch.n_tokens() - 1);
            if vocab.is_eog(token) {
                break;
            }
            let piece = vocab.token_to_piece(token, true, None);
            out.extend(&piece);
            pending.extend(&piece);
            if let Ok(s) = std::str::from_utf8(&pending) {
                // Hide hidden-reasoning blocks from the stream.
                if s.contains("<think>") {
                    in_think = true;
                }
                if in_think {
                    if s.contains("</think>") {
                        in_think = false;
                    }
                    pending.clear();
                } else if !s.is_empty() {
                    let keep_going = on_token(s);
                    pending.clear();
                    if !keep_going {
                        break;
                    }
                }
            }
            batch.clear();
            batch.add(token, pos, &[0], true).map_err(|e| e.to_string())?;
            pos += 1;
            ctx.decode(&mut batch).map_err(|e| format!("decode: {e}"))?;
        }
        let text = String::from_utf8_lossy(&out);
        Ok(THINK_RE.replace_all(&text, "").trim().to_string())
    }
}

// ---------------------------------------------------------------------------
// Model store / download
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct TierStatus {
    pub tier: String,
    pub label: String,
    pub file: String,
    pub size_bytes: u64,
    pub min_ram_gb: u32,
    pub notes: String,
    pub installed: bool,
    pub path: String,
    pub fits: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelStatus {
    /// custom | bundled | downloaded | none
    pub source: String,
    /// The model that will actually be used (or downloaded) right now.
    pub active_tier: String,
    pub installed: bool,
    pub path: String,
    pub name: String,
    pub size_bytes: u64,
    pub loaded: bool,
    pub downloading_tier: Option<String>,
    pub accelerator: String,
    pub total_ram_gb: u32,
    pub recommended_tier: String,
    pub tiers: Vec<TierStatus>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    pub tier: String,
    pub downloaded: u64,
    pub total: u64,
    pub done: bool,
    pub error: Option<String>,
}

pub struct Downloader {
    pub active: AtomicBool,
    pub cancel: AtomicBool,
    pub tier: std::sync::Mutex<String>,
}

impl Default for Downloader {
    fn default() -> Self {
        Self { active: AtomicBool::new(false), cancel: AtomicBool::new(false), tier: std::sync::Mutex::new(String::new()) }
    }
}

/// Streams the GGUF into `<dest>.part` (resuming if a partial file exists),
/// then renames it into place. `report` receives progress updates.
pub async fn download(
    http: reqwest::Client,
    tier: &str,
    url: &str,
    dest: &Path,
    dl: Arc<Downloader>,
    report: impl Fn(DownloadProgress) + Send + Sync + 'static,
) -> Result<(), String> {
    if dl.active.swap(true, Ordering::SeqCst) {
        return Err("A download is already running.".into());
    }
    dl.cancel.store(false, Ordering::SeqCst);
    *dl.tier.lock().unwrap() = tier.to_string();
    let result = download_inner(http, tier, url, dest, &dl, &report).await;
    dl.active.store(false, Ordering::SeqCst);
    dl.tier.lock().unwrap().clear();
    match &result {
        Ok(()) => report(DownloadProgress { tier: tier.into(), downloaded: 0, total: 0, done: true, error: None }),
        Err(e) => report(DownloadProgress { tier: tier.into(), downloaded: 0, total: 0, done: true, error: Some(e.clone()) }),
    }
    result
}

async fn download_inner(
    http: reqwest::Client,
    tier: &str,
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
            report(DownloadProgress { tier: tier.into(), downloaded, total, done: false, error: None });
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
