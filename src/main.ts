import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open, save } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";

// ---------------------------------------------------------------------------
// Types mirrored from the Rust side
// ---------------------------------------------------------------------------
interface Settings {
  anthropic_api_key: string;
  model: string;
  effort: string;
  default_style: string;
  default_intensity: string;
  ai_judge_enabled: boolean;
  engine_mode: string;
  local_model_path: string;
  auto_download_model: boolean;
  local_model_tier: string;
}
interface TierStatus {
  tier: string; label: string; file: string; size_bytes: number; min_ram_gb: number; notes: string;
  installed: boolean; path: string; fits: boolean;
}
interface ModelStatus {
  source: string; active_tier: string; installed: boolean; path: string; name: string; size_bytes: number;
  loaded: boolean; downloading_tier: string | null; accelerator: string; total_ram_gb: number;
  recommended_tier: string; tiers: TierStatus[];
}
interface DownloadProgress { tier: string; downloaded: number; total: number; done: boolean; error: string | null }
interface Signal { label: string; value: string; note: string }
interface Flagged { sentence: string; score: number; reason: string }
interface EngineReport {
  id: string; name: string; description: string; ai_score: number; weight: number;
  signals: Signal[]; flagged: Flagged[]; error: string | null;
}
interface DetectionReport {
  overall: number; verdict: string; confidence: string; word_count: number;
  sentence_count: number; note: string; engines: EngineReport[];
}
interface HumanizeTextResult {
  text: string; engine: string; passes: number; warnings: string[];
  before: DetectionReport; after: DetectionReport;
}
interface DocInfo {
  path: string; name: string; kind: "docx" | "txt" | "md" | "pdf"; words: number;
  paragraphs: number; editable_paragraphs: number; preview: string; score: number;
}
interface DocResult {
  out_path: string; changed: number; total: number; engine: string; passes: number;
  warnings: string[]; before_score: number; after_score: number;
}
interface AppInfo { name: string; version: string; developer: string; company: string; website: string; downloads: string; os: string; arch: string }
interface Progress { id: string; done: number; total: number; stage: string }

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------
const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const words = (s: string) => (s.match(/[A-Za-zऀ-ॿ][\w'’-]*/g) || []).length;
const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c] as string));
const band = (v: number) => (v < 40 ? "low" : v < 60 ? "mid" : "high");
const bandColor = (v: number) => (v < 40 ? "var(--ok)" : v < 60 ? "var(--warn)" : "var(--bad)");

function toast(msg: string, kind: "" | "ok" | "err" = "", ms = 4200): HTMLElement {
  const el = document.createElement("div");
  el.className = `toast ${kind}`;
  el.textContent = msg;
  $("toasts").appendChild(el);
  if (ms > 0) setTimeout(() => el.remove(), ms);
  return el;
}
const errMsg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e));

// ---------------------------------------------------------------------------
// Navigation
// ---------------------------------------------------------------------------
document.querySelectorAll<HTMLButtonElement>(".nav-item").forEach((b) =>
  b.addEventListener("click", () => {
    document.querySelectorAll(".nav-item").forEach((x) => x.classList.remove("active"));
    document.querySelectorAll(".view").forEach((x) => x.classList.remove("active"));
    b.classList.add("active");
    $(`view-${b.dataset.view}`).classList.add("active");
  })
);

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------
let settings: Settings = {
  anthropic_api_key: "",
  model: "claude-fable-5-1",
  effort: "high",
  default_style: "natural",
  default_intensity: "balanced",
  ai_judge_enabled: true,
  engine_mode: "auto",
  local_model_path: "",
  auto_download_model: true,
  local_model_tier: "auto",
};
let modelStatus: ModelStatus | null = null;
let downloadPct: number | null = null;

async function loadSettings() {
  settings = await invoke<Settings>("get_settings");
  $<HTMLInputElement>("api-key").value = settings.anthropic_api_key;
  $<HTMLSelectElement>("model").value = settings.model;
  $<HTMLSelectElement>("effort").value = settings.effort;
  $<HTMLInputElement>("ai-judge").checked = settings.ai_judge_enabled;
  $<HTMLSelectElement>("engine-mode").value = settings.engine_mode;
  $<HTMLInputElement>("auto-download").checked = settings.auto_download_model;
  $<HTMLSelectElement>("style").value = settings.default_style;
  $<HTMLSelectElement>("intensity").value = settings.default_intensity;
  $<HTMLSelectElement>("doc-style").value = settings.default_style;
  $<HTMLSelectElement>("doc-intensity").value = settings.default_intensity;
  $<HTMLInputElement>("use-judge").checked = settings.ai_judge_enabled;
  refreshEnginePill();
}

function activeEngineLabel(): string {
  const hasKey = !!settings.anthropic_api_key;
  const hasLocal = !!modelStatus?.installed;
  const dl = modelStatus?.downloading_tier ? `downloading model${downloadPct !== null ? ` ${downloadPct}%` : "…"}` : "rules (no AI yet)";
  switch (settings.engine_mode) {
    case "api": return hasKey ? settings.model : "rules (no API key set)";
    case "local": return hasLocal ? `Local AI · ${modelStatus!.name}` : dl;
    case "rules": return "rules only";
    default:
      if (hasKey) return settings.model;
      if (hasLocal) return `Local AI · ${modelStatus!.name}`;
      return dl;
  }
}

function refreshEnginePill() {
  const pill = $("engine-pill");
  pill.textContent = `Engine: ${activeEngineLabel()}`;
  pill.title = modelStatus?.installed || settings.anthropic_api_key ? "" : "Download the offline model or add an API key in Settings";
}

// ---------------------------------------------------------------------------
// Offline model
// ---------------------------------------------------------------------------
const fmtGB = (b: number) => `${(b / 1e9).toFixed(2)} GB`;

async function refreshModelStatus() {
  try {
    modelStatus = await invoke<ModelStatus>("local_model_status");
  } catch (e) {
    $("model-status").textContent = errMsg(e);
    return;
  }
  const m = modelStatus;
  const st = $("model-status");
  const dlTier = m.downloading_tier;
  if (m.installed) {
    st.innerHTML = `<b style="color:var(--ok)">Active:</b> ${esc(m.name)} · ${fmtGB(m.size_bytes)} · runs on ${esc(m.accelerator)}${m.loaded ? " · loaded" : ""} · this computer has ${m.total_ram_gb} GB RAM<br><span class="hint">${esc(m.path)}${m.source === "bundled" ? " (bundled with the app)" : ""}</span>`;
  } else if (dlTier) {
    st.textContent = `Downloading the ${dlTier} model… you can keep using the app; rules run until it arrives.`;
  } else {
    st.innerHTML = `<b>No model yet.</b> This computer has ${m.total_ram_gb} GB RAM → recommended tier: <b>${esc(m.recommended_tier)}</b>.`;
  }
  $("model-progress").hidden = !dlTier;
  $("btn-model-test").hidden = !m.installed;
  $("btn-model-custom").hidden = !!dlTier;
  renderSetupBar();

  const chosen = settings.local_model_tier;
  $("model-tiers").innerHTML = m.tiers
    .map((t) => {
      const isActive = m.installed && m.source !== "custom" && t.tier === m.active_tier;
      const badges = [
        t.tier === m.recommended_tier ? `<span class="badge rec">Recommended for this computer</span>` : "",
        t.installed ? `<span class="badge ok">Installed</span>` : "",
        !t.fits ? `<span class="badge">Needs ${t.min_ram_gb} GB RAM</span>` : "",
        chosen === t.tier ? `<span class="badge">Selected</span>` : "",
      ].join(" ");
      let actions = "";
      if (dlTier === t.tier) actions = `<span class="hint">Downloading…</span>`;
      else if (t.installed) {
        actions = `${isActive ? "" : `<button class="btn sm" data-use="${t.tier}">Use</button>`}<button class="btn ghost sm danger" data-remove-tier="${t.tier}">Remove</button>`;
      } else if (!dlTier) {
        actions = `<button class="btn sm ${t.tier === m.recommended_tier ? "primary" : ""}" data-download="${t.tier}">Download ${fmtGB(t.size_bytes)}</button>`;
      }
      return `<div class="tier ${isActive ? "active" : ""} ${t.fits ? "" : "nofit"}">
        <div><div class="tier-name">${esc(t.label)} ${badges}</div><div class="tier-meta">${fmtGB(t.size_bytes)} · needs ${t.min_ram_gb} GB RAM · ${esc(t.notes)}</div></div>
        <div class="tier-actions">${actions}</div>
      </div>`;
    })
    .join("");
  $("model-tiers").querySelectorAll<HTMLButtonElement>("[data-download]").forEach((b) =>
    b.addEventListener("click", () => startDownload(b.dataset.download!))
  );
  $("model-tiers").querySelectorAll<HTMLButtonElement>("[data-use]").forEach((b) =>
    b.addEventListener("click", async () => {
      try {
        modelStatus = await invoke<ModelStatus>("select_model_tier", { tier: b.dataset.use });
        settings.local_model_tier = b.dataset.use!;
        settings.local_model_path = "";
        toast(`Now using ${modelStatus.name}`, "ok");
      } catch (e) {
        toast(errMsg(e), "err");
      }
      refreshModelStatus();
    })
  );
  $("model-tiers").querySelectorAll<HTMLButtonElement>("[data-remove-tier]").forEach((b) =>
    b.addEventListener("click", async () => {
      if (!confirm("Remove this model file? You can download it again later.")) return;
      try {
        await invoke("delete_local_model", { tier: b.dataset.removeTier });
        toast("Model removed", "ok");
      } catch (e) {
        toast(errMsg(e), "err");
      }
      refreshModelStatus();
    })
  );
  refreshEnginePill();
}

async function startDownload(tier: string | null) {
  try {
    const t = await invoke<string>("download_local_model", { tier });
    if (settings.local_model_tier !== "auto") settings.local_model_tier = t;
    settings.local_model_path = "";
    $("model-progress").hidden = false;
    $("model-progress-fill").classList.add("indeterminate");
    $("model-progress-label").textContent = `Starting ${t} model download…`;
    await refreshModelStatus();
  } catch (e) {
    toast(errMsg(e), "err", 6000);
  }
}

/// Whether the app wants a local model and has none yet.
function needsModel(): boolean {
  if (!modelStatus || modelStatus.installed) return false;
  if (settings.engine_mode === "rules" || settings.engine_mode === "api") return false;
  if (settings.engine_mode === "auto" && settings.anthropic_api_key) return false;
  return true;
}

/// The setup bar at the top: visible while a model is downloading, or when
/// one is needed and the download is paused.
function renderSetupBar() {
  const bar = $("setup-bar");
  if (!modelStatus || (!modelStatus.downloading_tier && !needsModel())) {
    bar.hidden = true;
    return;
  }
  bar.hidden = false;
  const dl = modelStatus.downloading_tier;
  const tier = modelStatus.tiers.find((t) => t.tier === (dl || modelStatus!.recommended_tier));
  const name = tier ? tier.label : "offline AI model";
  if (dl) {
    $("setup-title").textContent = `Setting up ${name} (${tier ? fmtGB(tier.size_bytes) : ""})…`;
    $("setup-sub").textContent = "You can start humanizing right away; the app switches to the AI model automatically when it is ready. Closing the app pauses the download; it resumes next launch.";
    $("setup-pause").hidden = false;
    $("setup-resume").hidden = true;
  } else {
    $("setup-title").textContent = `Offline AI model not installed yet (${name}${tier ? `, ${fmtGB(tier.size_bytes)}` : ""})`;
    $("setup-sub").textContent = "Rewrites use the rule engine until the model is downloaded.";
    $("setup-pause").hidden = true;
    $("setup-resume").hidden = false;
    $("setup-pct").textContent = "";
    $("setup-fill").classList.add("indeterminate");
    $("setup-fill").style.width = "";
  }
}
$("setup-pause").addEventListener("click", () => invoke("cancel_model_download"));
$("setup-resume").addEventListener("click", () => startDownload(null));

/// First launch: fetch the recommended model without asking, unless the
/// user turned that off, already has an API key, or chose rules only.
/// Re-running on every launch is what makes an interrupted download resume.
async function maybeAutoDownload() {
  if (!needsModel() || modelStatus!.downloading_tier || !settings.auto_download_model) return;
  await startDownload(null);
}

$("btn-model-cancel").addEventListener("click", () => invoke("cancel_model_download"));
$("btn-model-custom").addEventListener("click", async () => {
  const picked = await open({ multiple: false, filters: [{ name: "GGUF model", extensions: ["gguf"] }] });
  if (!picked) return;
  try {
    modelStatus = await invoke<ModelStatus>("use_custom_model", { path: picked });
    settings.local_model_path = picked as string;
    toast(`Using ${modelStatus.name}`, "ok");
  } catch (e) {
    toast(errMsg(e), "err", 6000);
  }
  refreshModelStatus();
});
$("btn-model-url").addEventListener("click", async () => {
  const url = $<HTMLInputElement>("model-url").value.trim();
  if (!url) return toast("Paste a .gguf link first.", "err");
  try {
    const name = await invoke<string>("download_custom_model", { url });
    settings.local_model_path = "";
    toast(`Downloading ${name}… it becomes the active model when done.`, "", 6000);
    $<HTMLInputElement>("model-url").value = "";
    $("model-progress").hidden = false;
    $("model-progress-fill").classList.add("indeterminate");
    $("model-progress-label").textContent = `Starting ${name}…`;
    await refreshModelStatus();
  } catch (e) {
    toast(errMsg(e), "err", 8000);
  }
});

$("btn-model-test").addEventListener("click", async () => {
  const out = $("model-test-result");
  out.className = "hint";
  out.textContent = "Loading model and generating a sample… (first load takes a few seconds)";
  try {
    out.textContent = await invoke<string>("test_local_model");
    out.classList.add("ok");
  } catch (e) {
    out.textContent = errMsg(e);
    out.classList.add("bad");
  }
  refreshModelStatus();
});

listen<DownloadProgress>("model-download", (ev) => {
  const p = ev.payload;
  const fill = $("model-progress-fill");
  const label = $("model-progress-label");
  if (p.done) {
    downloadPct = null;
    if (p.error) {
      toast(p.error, "err", 7000);
    } else {
      toast("Offline AI model installed. Humanizer now rewrites without any API.", "ok", 6000);
    }
    refreshModelStatus();
    return;
  }
  $("model-progress").hidden = false;
  fill.classList.remove("indeterminate");
  const pct = p.total ? Math.round((p.downloaded / p.total) * 100) : 0;
  downloadPct = p.total ? pct : null;
  fill.style.width = `${pct}%`;
  label.textContent = p.total ? `${p.tier} model · ${fmtGB(p.downloaded)} / ${fmtGB(p.total)} · ${pct}%` : fmtGB(p.downloaded);
  // Mirror onto the first-launch setup bar.
  const sf = $("setup-fill");
  sf.classList.remove("indeterminate");
  sf.style.width = `${pct}%`;
  $("setup-pct").textContent = p.total ? `${fmtGB(p.downloaded)} / ${fmtGB(p.total)} · ${pct}%` : fmtGB(p.downloaded);
  if (!modelStatus?.downloading_tier) refreshModelStatus();
  else refreshEnginePill();
});

$("btn-save-settings").addEventListener("click", async () => {
  const next: Settings = {
    ...settings,
    anthropic_api_key: $<HTMLInputElement>("api-key").value.trim(),
    model: $<HTMLSelectElement>("model").value,
    effort: $<HTMLSelectElement>("effort").value,
    ai_judge_enabled: $<HTMLInputElement>("ai-judge").checked,
    engine_mode: $<HTMLSelectElement>("engine-mode").value,
    auto_download_model: $<HTMLInputElement>("auto-download").checked,
    default_style: $<HTMLSelectElement>("style").value,
    default_intensity: $<HTMLSelectElement>("intensity").value,
  };
  try {
    settings = await invoke<Settings>("save_settings", { settings: next });
    refreshEnginePill();
    $("settings-saved").textContent = "Saved.";
    setTimeout(() => ($("settings-saved").textContent = ""), 2000);
  } catch (e) {
    toast(errMsg(e), "err");
  }
});

$("btn-show-key").addEventListener("click", () => {
  const i = $<HTMLInputElement>("api-key");
  i.type = i.type === "password" ? "text" : "password";
  $("btn-show-key").textContent = i.type === "password" ? "Show" : "Hide";
});

$("btn-test").addEventListener("click", async () => {
  const out = $("test-result");
  out.className = "hint";
  out.textContent = "Testing…";
  try {
    // Save first so the test uses the key in the box.
    $("btn-save-settings").click();
    await new Promise((r) => setTimeout(r, 150));
    out.textContent = await invoke<string>("test_connection");
    out.classList.add("ok");
  } catch (e) {
    out.textContent = errMsg(e);
    out.classList.add("bad");
  }
});

// Remember style/intensity choices as defaults silently.
for (const id of ["style", "intensity"]) {
  $<HTMLSelectElement>(id).addEventListener("change", () => {
    $<HTMLSelectElement>(`doc-${id}`).value = $<HTMLSelectElement>(id).value;
  });
}

// ---------------------------------------------------------------------------
// Humanize text
// ---------------------------------------------------------------------------
const input = $<HTMLTextAreaElement>("input");
const output = $<HTMLTextAreaElement>("output");
let lastAfter: DetectionReport | null = null;

function updateCounts() {
  $("in-count").textContent = `${words(input.value)} words`;
  $("out-count").textContent = `${words(output.value)} words`;
  const has = output.value.trim().length > 0;
  for (const id of ["btn-copy", "btn-save-text", "btn-check-out"]) $<HTMLButtonElement>(id).disabled = !has;
}
input.addEventListener("input", updateCounts);
output.addEventListener("input", updateCounts);

$("btn-clear").addEventListener("click", () => {
  input.value = "";
  output.value = "";
  lastAfter = null;
  $("detect-panel").hidden = true;
  $("verdict").hidden = true;
  $("vstatus").hidden = true;
  $("details-wrap").hidden = true;
  $("warnings").hidden = true;
  updateCounts();
});
$("btn-paste").addEventListener("click", async () => {
  try {
    input.value = await navigator.clipboard.readText();
    updateCounts();
  } catch {
    toast("Clipboard access was denied. Use Cmd/Ctrl+V instead.", "err");
  }
});
$("btn-copy").addEventListener("click", async () => {
  await navigator.clipboard.writeText(output.value);
  toast("Copied to clipboard", "ok", 1600);
});
$("btn-import").addEventListener("click", async () => {
  const picked = await open({ multiple: false, filters: [{ name: "Documents", extensions: ["docx", "pdf", "md", "txt"] }] });
  if (!picked) return;
  try {
    input.value = await invoke<string>("extract_document_text", { path: picked });
    updateCounts();
  } catch (e) {
    toast(errMsg(e), "err");
  }
});
$("btn-save-text").addEventListener("click", async () => {
  const outPath = await save({ defaultPath: "humanized.docx", filters: [{ name: "Word document", extensions: ["docx"] }, { name: "Text", extensions: ["txt", "md"] }] });
  if (!outPath) return;
  try {
    await invoke("save_text_as", { textIn: output.value, outPath });
    toast(`Saved ${outPath.split(/[\\/]/).pop()}`, "ok");
  } catch (e) {
    toast(errMsg(e), "err");
  }
});

function setTextProgress(p: Progress | null) {
  const wrap = $("text-progress");
  if (!p) {
    wrap.hidden = true;
    return;
  }
  wrap.hidden = false;
  const fill = $("text-progress-fill");
  if (p.total <= 1 && p.done === 0) {
    fill.classList.add("indeterminate");
  } else {
    fill.classList.remove("indeterminate");
    fill.style.width = `${Math.round((p.done / Math.max(p.total, 1)) * 100)}%`;
  }
  $("text-progress-label").textContent = p.total > 1 ? `${p.stage} · ${p.done}/${p.total} sections` : p.stage;
}

$("btn-humanize").addEventListener("click", async () => {
  const text = input.value;
  if (words(text) < 3) return toast("Paste some text first.", "err");
  const btn = $<HTMLButtonElement>("btn-humanize");
  btn.disabled = true;
  setTextProgress({ id: "text", done: 0, total: 1, stage: "Starting…" });
  $("warnings").hidden = true;
  try {
    const r = await invoke<HumanizeTextResult>("humanize_text", {
      textIn: text,
      style: $<HTMLSelectElement>("style").value,
      intensity: $<HTMLSelectElement>("intensity").value,
    });
    output.value = r.text;
    lastAfter = r.after;
    updateCounts();
    const hb = humanPct(r.before.overall);
    const ha = humanPct(r.after.overall);
    const unchanged = r.text.trim() === text.trim();
    let status = unchanged
      ? `<b>No change this time.</b> Nothing read more human than what you gave. Click Humanize for a fresh attempt, or try Strong.`
      : ha >= 75
        ? `<b>Done.</b> ${r.passes} pass${r.passes > 1 ? "es" : ""} with ${esc(r.engine)}. Copy it, or click Humanize again for another variation.`
        : `<b>Improved ${hb}% → ${ha}% human</b> in ${r.passes} pass${r.passes > 1 ? "es" : ""}. Click Humanize again to push further, or switch Intensity to Strong.`;
    showVerdict(r.before.overall, r.after.overall, status);
    $("warnings").innerHTML = r.warnings.map((w) => `<div>ⓘ ${esc(w)}</div>`).join("");
    $("warnings").hidden = r.warnings.length === 0;
    renderDetection(r.after, r.text, `Result · ${r.engine} · ${r.passes} pass${r.passes > 1 ? "es" : ""}`);
  } catch (e) {
    toast(errMsg(e), "err", 7000);
  } finally {
    btn.disabled = false;
    setTextProgress(null);
  }
});

/// Human score on a 0-100 "% human" scale, from our machine-likeness.
const humanPct = (machine: number) => Math.max(0, Math.min(100, Math.round(100 - machine)));
function verdictFor(h: number): { cls: "good" | "mid" | "bad"; label: string; color: string } {
  if (h >= 75) return { cls: "good", label: "Looks Human", color: "var(--ok)" };
  if (h >= 60) return { cls: "good", label: "Mostly Human", color: "var(--ok)" };
  if (h >= 40) return { cls: "mid", label: "Mixed", color: "var(--warn)" };
  return { cls: "bad", label: "Looks AI", color: "var(--bad)" };
}
function paintCard(which: "before" | "after", machine: number, sub: string) {
  const h = humanPct(machine);
  const v = verdictFor(h);
  const card = $(`vcard-${which}`);
  card.className = `vcard ${v.cls}`;
  card.style.setProperty("--p", `${h}`);
  card.style.setProperty("--c", v.color);
  $(`ring-val-${which}`).textContent = `${h}%`;
  $(`vbadge-${which}`).textContent = `✓ ${v.label}`.replace("✓ Looks AI", "⚠ Looks AI").replace("✓ Mixed", "~ Mixed");
  $(`vsub-${which}`).textContent = sub;
}
function showVerdict(before: number | null, after: number | null, status: string) {
  $("verdict").hidden = false;
  const two = before !== null && after !== null;
  $("vcard-before").hidden = before === null;
  $("varrow").hidden = !two;
  $("vcard-after").hidden = after === null;
  if (before !== null) paintCard("before", before, "Original text");
  if (after !== null) paintCard("after", after, "After Humanizer");
  const st = $("vstatus");
  st.innerHTML = status;
  st.hidden = !status;
  $("details-wrap").hidden = false;
}

async function runDetection(text: string, title: string) {
  if (words(text) < 3) return toast("Nothing to check.", "err");
  const useJudge = $<HTMLInputElement>("use-judge").checked;
  const panel = $("detect-panel");
  panel.hidden = false;
  $("details-wrap").hidden = false;
  $("detect-title").textContent = title;
  $("detect-verdict").textContent = useJudge ? "Running 5 engines + AI Judge…" : "Running 5 engines…";
  $("engines").innerHTML = "";
  try {
    const r = await invoke<DetectionReport>("detect_text", { textIn: text, withJudge: useJudge });
    const isOriginal = title === "Original text";
    const h = humanPct(r.overall);
    const v = verdictFor(h);
    showVerdict(isOriginal ? r.overall : null, isOriginal ? null : r.overall, `<b>${v.label}</b> · ${h}% human · ${r.engines.filter((e) => !e.error).length} engines agree on this reading.`);
    renderDetection(r, text, title);
  } catch (e) {
    toast(errMsg(e), "err");
  }
}
$("btn-check-in").addEventListener("click", () => runDetection(input.value, "Original text"));
$("btn-check-out").addEventListener("click", () => runDetection(output.value, "Humanized result"));

function renderDetection(r: DetectionReport, text: string, title: string) {
  const panel = $("detect-panel");
  panel.hidden = false;
  $("details-wrap").hidden = false;
  $("detect-title").textContent = title;
  const g = $("gauge");
  g.style.setProperty("--p", `${r.overall}`);
  g.style.setProperty("--c", bandColor(r.overall));
  $("gauge-val").textContent = `${Math.round(r.overall)}`;
  $("detect-verdict").textContent = r.verdict;
  $("detect-verdict").style.color = bandColor(r.overall);
  $("detect-meta").textContent = `${r.word_count} words · ${r.sentence_count} sentences · confidence ${r.confidence} · ${r.engines.filter((e) => !e.error).length} engines`;
  $("detect-note").textContent = r.note;

  $("engines").innerHTML = r.engines
    .map((e) => {
      const sc = Math.round(e.ai_score);
      const sig = e.signals.map((s) => `<span class="k" title="${esc(s.note)}">${esc(s.label)}</span><span class="v">${esc(s.value)}</span>`).join("");
      const flags = e.flagged.length
        ? `<details><summary>${e.flagged.length} flagged sentence${e.flagged.length > 1 ? "s" : ""}</summary>${e.flagged
            .map((f) => `<div class="flag">${esc(f.sentence)}<span class="why">${esc(f.reason)}</span></div>`)
            .join("")}</details>`
        : "";
      return `<div class="engine">
        <div class="engine-top"><span class="engine-name">${esc(e.name)}</span><span class="engine-score" style="color:${e.error ? "var(--fg-muted)" : bandColor(sc)}">${e.error ? "n/a" : sc}</span></div>
        <div class="engine-bar"><i style="width:${e.error ? 0 : sc}%;background:${bandColor(sc)}"></i></div>
        ${e.error ? `<div class="engine-err">${esc(e.error)}</div>` : ""}
        <div class="engine-desc">${esc(e.description)}</div>
        ${sig ? `<div class="signals">${sig}</div>` : ""}
        ${flags}
      </div>`;
    })
    .join("");

  // Highlight flagged sentences in context.
  const flagged = new Map<string, string>();
  for (const e of r.engines) for (const f of e.flagged) {
    const key = f.sentence.trim();
    flagged.set(key, (flagged.get(key) ? flagged.get(key) + " · " : "") + `${e.name}: ${f.reason}`);
  }
  const wrap = $("highlight-wrap");
  if (flagged.size === 0) {
    wrap.hidden = true;
    return;
  }
  let html = esc(text);
  // Longest first so nested matches don't break earlier marks.
  for (const [s, why] of [...flagged.entries()].sort((a, b) => b[0].length - a[0].length)) {
    const needle = esc(s);
    if (!needle) continue;
    html = html.split(needle).join(`<mark title="${esc(why)}">${needle}</mark>`);
  }
  $("highlight").innerHTML = html;
  wrap.hidden = false;
}

// ---------------------------------------------------------------------------
// Documents
// ---------------------------------------------------------------------------
interface DocRow { info: DocInfo; status: "ready" | "working" | "done" | "error"; progress?: Progress; result?: DocResult; error?: string }
const docs: DocRow[] = [];

async function addPaths(paths: string[]) {
  for (const p of paths) {
    if (docs.some((d) => d.info.path === p)) continue;
    if (!/\.(docx|pdf|md|markdown|txt)$/i.test(p)) {
      toast(`Skipped ${p.split(/[\\/]/).pop()} (unsupported type)`, "err");
      continue;
    }
    try {
      const info = await invoke<DocInfo>("inspect_document", { path: p });
      docs.push({ info, status: "ready" });
    } catch (e) {
      toast(`${p.split(/[\\/]/).pop()}: ${errMsg(e)}`, "err", 6000);
    }
  }
  renderDocs();
}

function renderDocs() {
  const list = $("doc-list");
  list.innerHTML = docs
    .map((d, i) => {
      const k = d.info.kind;
      let right = "";
      if (d.status === "ready") right = `<span class="doc-status">Machine-likeness now: <b style="color:${bandColor(d.info.score)}">${Math.round(d.info.score)}</b></span>`;
      if (d.status === "working") {
        const p = d.progress;
        const pct = p && p.total > 1 ? Math.round((p.done / p.total) * 100) : 0;
        right = `<span class="doc-status">${p ? esc(p.stage) : "Starting…"}${p && p.total > 1 ? ` · ${p.done}/${p.total}` : ""}</span><div class="bar"><div class="fill ${pct ? "" : "indeterminate"}" style="width:${pct}%"></div></div>`;
      }
      if (d.status === "done" && d.result) {
        const r = d.result;
        right = `<span class="doc-status">${Math.round(r.before_score)} → <b style="color:${bandColor(r.after_score)}">${Math.round(r.after_score)}</b> · ${r.changed}/${r.total} paragraphs</span><div class="doc-actions"><button class="btn sm" data-reveal="${i}">Show file</button></div>`;
      }
      if (d.status === "error") right = `<span class="doc-status" style="color:var(--bad)">${esc(d.error || "Failed")}</span>`;
      return `<div class="doc">
        <div class="doc-ico ${k}">${k.toUpperCase()}</div>
        <div>
          <div class="doc-name">${esc(d.info.name)}</div>
          <div class="doc-meta">${d.info.words.toLocaleString()} words · ${d.info.editable_paragraphs} of ${d.info.paragraphs} paragraphs will be rewritten${d.status === "done" && d.result ? ` · saved as ${esc(d.result.out_path.split(/[\\/]/).pop() || "")}` : ""}</div>
          <div class="doc-preview">${esc(d.info.preview)}</div>
        </div>
        <div class="doc-right">${right}${d.status === "ready" ? `<button class="btn ghost sm" data-remove="${i}">Remove</button>` : ""}</div>
      </div>`;
    })
    .join("");
  list.querySelectorAll<HTMLButtonElement>("[data-remove]").forEach((b) =>
    b.addEventListener("click", () => {
      docs.splice(Number(b.dataset.remove), 1);
      renderDocs();
    })
  );
  list.querySelectorAll<HTMLButtonElement>("[data-reveal]").forEach((b) =>
    b.addEventListener("click", async () => {
      const d = docs[Number(b.dataset.reveal)];
      if (d.result) {
        try {
          const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
          await revealItemInDir(d.result.out_path);
        } catch (e) {
          toast(errMsg(e), "err");
        }
      }
    })
  );
  const anyReady = docs.some((d) => d.status === "ready");
  $<HTMLButtonElement>("btn-humanize-docs").disabled = !anyReady;
  $<HTMLButtonElement>("btn-clear-docs").disabled = docs.length === 0;
}

$("btn-choose").addEventListener("click", async () => {
  const picked = await open({ multiple: true, filters: [{ name: "Documents", extensions: ["docx", "pdf", "md", "txt"] }] });
  if (!picked) return;
  await addPaths(Array.isArray(picked) ? picked : [picked]);
});
$("btn-clear-docs").addEventListener("click", () => {
  docs.splice(0, docs.length);
  renderDocs();
});

$("btn-humanize-docs").addEventListener("click", async () => {
  const style = $<HTMLSelectElement>("doc-style").value;
  const intensity = $<HTMLSelectElement>("doc-intensity").value;
  const btn = $<HTMLButtonElement>("btn-humanize-docs");
  btn.disabled = true;
  for (const d of docs) {
    if (d.status !== "ready") continue;
    const src = d.info.path;
    const dot = src.lastIndexOf(".");
    const ext = d.info.kind === "pdf" ? "docx" : src.slice(dot + 1);
    const suggested = `${src.slice(0, dot)}.humanized.${ext}`;
    const outPath = await save({
      title: `Save humanized ${d.info.name}`,
      defaultPath: suggested,
      filters: d.info.kind === "pdf" || d.info.kind === "docx"
        ? [{ name: "Word document", extensions: ["docx"] }, { name: "Text", extensions: ["txt"] }]
        : [{ name: "Same format", extensions: [ext] }, { name: "Word document", extensions: ["docx"] }],
    });
    if (!outPath) continue;
    d.status = "working";
    d.progress = undefined;
    renderDocs();
    try {
      d.result = await invoke<DocResult>("humanize_document", { path: src, outPath, style, intensity });
      d.status = "done";
      if (d.result.warnings.length) toast(d.result.warnings[0], "", 6000);
    } catch (e) {
      d.status = "error";
      d.error = errMsg(e);
    }
    renderDocs();
  }
  renderDocs();
});

// Drag & drop from Finder / Explorer (native paths).
getCurrentWebview().onDragDropEvent((ev) => {
  const dz = $("dropzone");
  if (ev.payload.type === "over" || ev.payload.type === "enter") dz.classList.add("over");
  else dz.classList.remove("over");
  if (ev.payload.type === "drop") {
    const onDocs = $("view-documents").classList.contains("active");
    const paths = ev.payload.paths;
    if (onDocs || paths.length > 1) {
      document.querySelector<HTMLButtonElement>('[data-view="documents"]')!.click();
      addPaths(paths);
    } else {
      invoke<string>("extract_document_text", { path: paths[0] })
        .then((t) => { input.value = t; updateCounts(); })
        .catch((e) => toast(errMsg(e), "err"));
    }
  }
});

// Progress events from Rust.
listen<Progress>("humanize-progress", (ev) => {
  const p = ev.payload;
  if (p.id === "text") return setTextProgress(p);
  const d = docs.find((x) => x.info.path === p.id);
  if (d && d.status === "working") {
    d.progress = p;
    renderDocs();
  }
});

// ---------------------------------------------------------------------------
// Chat
// ---------------------------------------------------------------------------
interface ChatMsg { role: "user" | "assistant"; content: string }
interface ChatToken { id: string; text: string; done: boolean; error: string | null; engine: string }
const chatHistory: ChatMsg[] = [];
let chatBusy = false;
let chatReplyId = "";
let chatReplyEl: HTMLElement | null = null;

function chatAppend(role: ChatMsg["role"], content: string, meta = ""): HTMLElement {
  $("chat-empty").hidden = true;
  const el = document.createElement("div");
  el.className = `msg ${role}`;
  el.textContent = content;
  if (meta) {
    const m = document.createElement("span");
    m.className = "meta";
    m.textContent = meta;
    el.appendChild(m);
  }
  $("chat-log").appendChild(el);
  $("chat-log").scrollTop = $("chat-log").scrollHeight;
  return el;
}

async function chatSend() {
  const input = $<HTMLTextAreaElement>("chat-input");
  const text = input.value.trim();
  if (!text || chatBusy) return;
  input.value = "";
  chatHistory.push({ role: "user", content: text });
  chatAppend("user", text);
  chatBusy = true;
  $<HTMLButtonElement>("btn-chat-send").disabled = true;
  $("chat-status").textContent = "Thinking…";
  chatReplyId = `r${Date.now()}`;
  chatReplyEl = chatAppend("assistant", "");
  chatReplyEl.classList.add("streaming");
  try {
    const full = await invoke<string>("chat_send", { id: chatReplyId, turns: chatHistory });
    chatHistory.push({ role: "assistant", content: full });
    if (chatReplyEl) {
      chatReplyEl.classList.remove("streaming");
      // Streaming may have shown partial text; settle on the final answer.
      const meta = chatReplyEl.querySelector(".meta");
      chatReplyEl.textContent = full;
      if (meta) chatReplyEl.appendChild(meta);
    }
  } catch (e) {
    chatReplyEl?.remove();
    chatHistory.pop();
    toast(errMsg(e), "err", 7000);
  } finally {
    chatBusy = false;
    $<HTMLButtonElement>("btn-chat-send").disabled = false;
    $("chat-status").textContent = "";
    $("chat-log").scrollTop = $("chat-log").scrollHeight;
    input.focus();
  }
}
$("btn-chat-send").addEventListener("click", chatSend);
$<HTMLTextAreaElement>("chat-input").addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    chatSend();
  }
});
$("btn-chat-clear").addEventListener("click", () => {
  if (chatBusy) return;
  chatHistory.splice(0, chatHistory.length);
  $("chat-log").querySelectorAll(".msg").forEach((m) => m.remove());
  $("chat-empty").hidden = false;
});
listen<ChatToken>("chat-token", (ev) => {
  const p = ev.payload;
  if (p.id !== chatReplyId || !chatReplyEl) return;
  if (p.error) return;
  if (!p.done) {
    chatReplyEl.textContent += p.text;
    $("chat-log").scrollTop = $("chat-log").scrollHeight;
  } else {
    if (p.text) chatReplyEl.textContent = p.text; // non-streaming engines deliver the whole reply at the end
    const meta = document.createElement("span");
    meta.className = "meta";
    meta.textContent = p.engine;
    chatReplyEl.appendChild(meta);
  }
  $("chat-status").textContent = p.done ? "" : `Replying with ${p.engine}…`;
});

// ---------------------------------------------------------------------------
// About + downloads
// ---------------------------------------------------------------------------
let downloadsUrl = "https://github.com/00Arun/humanizer/tree/main/releases";
async function loadAbout() {
  const info = await invoke<AppInfo>("app_info");
  downloadsUrl = info.downloads;
  $("about-version").textContent = `v${info.version}`;
  $("sidebar-version").textContent = `v${info.version}`;
  $("about-platform").textContent = `${info.os} · ${info.arch}`;
  document.title = `${info.name}`;
}
$("link-website").addEventListener("click", (e) => {
  e.preventDefault();
  openUrl("https://arun-pandey.com.np/");
});
for (const id of ["btn-downloads", "btn-about-downloads"]) {
  $(id).addEventListener("click", () => openUrl(downloadsUrl).catch((e) => toast(errMsg(e), "err")));
}

// ---------------------------------------------------------------------------
// Boot
// ---------------------------------------------------------------------------
(async () => {
  try {
    await loadSettings();
    await loadAbout();
  } catch (e) {
    toast(errMsg(e), "err");
  }
  updateCounts();
  renderDocs();
  await refreshModelStatus();
  maybeAutoDownload();
})();
