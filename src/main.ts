import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open, save } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { relaunch } from "@tauri-apps/plugin-process";
import { check } from "@tauri-apps/plugin-updater";

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
  auto_update: boolean;
}
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
interface AppInfo { name: string; version: string; developer: string; company: string; website: string; os: string; arch: string }
interface Progress { id: string; done: number; total: number; stage: string }

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------
const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const words = (s: string) => (s.match(/[A-Za-zऀ-ॿ][\w'’-]*/g) || []).length;
const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c] as string));
const band = (v: number) => (v < 30 ? "low" : v < 60 ? "mid" : "high");
const bandColor = (v: number) => (v < 30 ? "var(--ok)" : v < 60 ? "var(--warn)" : "var(--bad)");

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
  auto_update: true,
};

async function loadSettings() {
  settings = await invoke<Settings>("get_settings");
  $<HTMLInputElement>("api-key").value = settings.anthropic_api_key;
  $<HTMLSelectElement>("model").value = settings.model;
  $<HTMLSelectElement>("effort").value = settings.effort;
  $<HTMLInputElement>("ai-judge").checked = settings.ai_judge_enabled;
  $<HTMLInputElement>("auto-update").checked = settings.auto_update;
  $<HTMLSelectElement>("style").value = settings.default_style;
  $<HTMLSelectElement>("intensity").value = settings.default_intensity;
  $<HTMLSelectElement>("doc-style").value = settings.default_style;
  $<HTMLSelectElement>("doc-intensity").value = settings.default_intensity;
  $<HTMLInputElement>("use-judge").checked = settings.ai_judge_enabled;
  refreshEnginePill();
}

function refreshEnginePill() {
  const pill = $("engine-pill");
  pill.textContent = settings.anthropic_api_key ? `Engine: ${settings.model}` : "Engine: offline rules (no API key)";
  pill.title = settings.anthropic_api_key ? "" : "Add an Anthropic API key in Settings for the full rewrite engine";
}

$("btn-save-settings").addEventListener("click", async () => {
  const next: Settings = {
    ...settings,
    anthropic_api_key: $<HTMLInputElement>("api-key").value.trim(),
    model: $<HTMLSelectElement>("model").value,
    effort: $<HTMLSelectElement>("effort").value,
    ai_judge_enabled: $<HTMLInputElement>("ai-judge").checked,
    auto_update: $<HTMLInputElement>("auto-update").checked,
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
  $("score-compare").hidden = true;
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
    showCompare(r.before.overall, r.after.overall);
    if (r.warnings.length) {
      $("warnings").innerHTML = r.warnings.map((w) => `<div>⚠ ${esc(w)}</div>`).join("");
      $("warnings").hidden = false;
    }
    renderDetection(r.after, r.text, `Result · ${r.engine} · ${r.passes} pass${r.passes > 1 ? "es" : ""}`);
    toast(`Done: ${Math.round(r.before.overall)} → ${Math.round(r.after.overall)} machine-likeness`, "ok");
  } catch (e) {
    toast(errMsg(e), "err", 7000);
  } finally {
    btn.disabled = false;
    setTextProgress(null);
  }
});

function showCompare(before: number, after: number) {
  const wrap = $("score-compare");
  wrap.hidden = false;
  const b = $("chip-before");
  const a = $("chip-after");
  b.textContent = `${Math.round(before)}`;
  a.textContent = `${Math.round(after)}`;
  b.className = `chip c-${band(before)}`;
  a.className = `chip c-${band(after)}`;
}

async function runDetection(text: string, title: string) {
  if (words(text) < 3) return toast("Nothing to check.", "err");
  const useJudge = $<HTMLInputElement>("use-judge").checked;
  const panel = $("detect-panel");
  panel.hidden = false;
  $("detect-title").textContent = title;
  $("detect-verdict").textContent = useJudge && settings.anthropic_api_key ? "Running 5 engines + AI Judge…" : "Running 5 engines…";
  $("engines").innerHTML = "";
  try {
    const r = await invoke<DetectionReport>("detect_text", { textIn: text, withJudge: useJudge });
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
// About + updater
// ---------------------------------------------------------------------------
async function loadAbout() {
  const info = await invoke<AppInfo>("app_info");
  $("about-version").textContent = `v${info.version}`;
  $("sidebar-version").textContent = `v${info.version}`;
  $("about-platform").textContent = `${info.os} · ${info.arch}`;
  document.title = `${info.name}`;
}
$("link-website").addEventListener("click", (e) => {
  e.preventDefault();
  openUrl("https://arun-pandey.com.np/");
});

let updating = false;
async function checkForUpdates(silent: boolean, statusEl?: HTMLElement) {
  if (updating) return;
  const set = (t: string) => { if (statusEl) statusEl.textContent = t; };
  set("Checking…");
  try {
    const update = await check();
    if (!update) {
      set("You're on the latest version.");
      if (!silent) toast("You're on the latest version.", "ok");
      return;
    }
    updating = true;
    set(`Downloading v${update.version}…`);
    const t = toast(`Updating to v${update.version}…`, "", 0);
    const bar = document.createElement("div");
    bar.className = "bar";
    bar.innerHTML = `<div class="fill indeterminate"></div>`;
    t.appendChild(bar);
    let total = 0, got = 0;
    await update.downloadAndInstall((ev) => {
      if (ev.event === "Started") total = ev.data.contentLength || 0;
      if (ev.event === "Progress") {
        got += ev.data.chunkLength;
        if (total) {
          const f = bar.firstElementChild as HTMLElement;
          f.classList.remove("indeterminate");
          f.style.width = `${Math.round((got / total) * 100)}%`;
        }
      }
      if (ev.event === "Finished") set("Installing…");
    });
    t.textContent = `Update v${update.version} installed. Restarting…`;
    set("Installed. Restarting…");
    setTimeout(() => relaunch(), 1500);
  } catch (e) {
    updating = false;
    const m = errMsg(e);
    set(`Update check failed: ${m}`);
    if (!silent) toast(`Update check failed: ${m}`, "err", 6000);
  }
}
$("btn-check-update").addEventListener("click", () => checkForUpdates(false, $("update-status")));
$("btn-about-update").addEventListener("click", () => checkForUpdates(false, $("about-update-status")));

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
  if (settings.auto_update) setTimeout(() => checkForUpdates(true), 2500);
})();
