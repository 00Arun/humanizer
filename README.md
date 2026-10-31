# Humanizer

Desktop app (macOS + Windows) that rewrites AI-assisted or over-polished text so it reads like a person wrote it, then checks the result with a built-in suite of AI-text detectors.

**Developer:** Arun Pandey · **Company:** Arun AI Studio · **Website:** https://arun-pandey.com.np/

Built in Rust with [Tauri 2](https://v2.tauri.app). No third-party humanizer or detector APIs: the engines are our own.

## What it does

| Feature | How |
|---|---|
| Humanize pasted text | Paste → pick a register (Natural / Academic / Professional / Casual / Creative) and intensity (Light / Balanced / Strong) → Humanize → Copy or Save as .docx/.txt |
| Humanize files | Drop `.docx`, `.pdf`, `.md`, `.txt`. Word files are edited **surgically**: paragraph styles, headings, numbering, tables, images, headers/footers stay intact; only the prose runs are replaced. PDFs are saved as `.docx`. |
| Detection suite | 5 offline statistical engines + an optional model-based **AI Judge**, combined into one 0-100 machine-likeness score with per-sentence flags and in-context highlights |
| Rewrite engines | **Offline AI** – Qwen3-4B-Instruct (GGUF, Apache-2.0) runs in-process via llama.cpp (Metal on Apple Silicon, multi-threaded CPU on Windows); one-time 2.5 GB download from Settings, no key, no internet afterwards. **Claude API** – Fable 5.1 (default; Opus 5.5 / Sonnet 5.5 selectable) with your own key, paragraph-chunked and run in parallel. **Rules** – the deterministic engine that always runs as a polish pass and as the last resort. Engine choice is automatic (API key → offline model → rules) or forced in Settings. Strong mode adds a second pass guided by our own detector's flags on every engine. |
| Updates | The latest installers always sit in [`releases/`](releases/). Run the new installer over the old version: NSIS and the macOS DMG upgrade in place. *Settings → Get latest version* opens that folder. |
| Installers | macOS `.dmg` (custom background), Windows NSIS `-setup.exe` (branded header/sidebar, per-user install, license page) |

### Detection engines

1. **Rhythm & Burstiness** – coefficient of variation of sentence length, share of short/long sentences, runs of same-length sentences.
2. **Lexical Predictability** – perplexity proxy from a 47k-word Zipf frequency table (OpenSubtitles 2018 list, CC-BY-SA 4.0): spread of word frequencies, "polished" mid-frequency vocabulary, rare-word rate, windowed type-token ratio.
3. **Stylometric Fingerprint** – average word length, contraction rate, sentence-opener diversity, transition-word openers, punctuation variety, first-person presence.
4. **Machine Idioms** – weighted lexicon of 150+ over-used words/phrases ("delve", "tapestry", "it's important to note", "not only … but also"), em-dash density.
5. **Structural Templates** – tricolons, wrap-up sentences closing paragraphs, colon set-ups, rhetorical-question transitions, paragraph-length uniformity.
6. **AI Judge** (optional, uses the API) – the configured model returns a calibrated 0-100 estimate, reasons, and flagged sentences.

Scores are heuristic. They are tuned so a typical model-written passage scores > 60 and typical human prose < 40 (see `src-tauri/src/detectors/tests.rs`), but no detector, ours or anyone's, is proof of authorship.

## Project layout

```
index.html, src/            Frontend (Vite + TypeScript, no framework)
src-tauri/
  src/lib.rs                Tauri setup, plugins, command registry
  src/commands.rs           IPC commands (humanize_text, detect_text, humanize_document, …)
  src/humanizer/mod.rs      Pipeline: chunking, model pass, detector-guided refine, polish
  src/humanizer/claude.rs   Raw-HTTP Claude Messages client (fallbacks: "default", effort)
  src/humanizer/local.rs    Offline rule engine
  src/detectors/            Five engines + AI Judge + ensemble
  src/documents/mod.rs      docx surgical edit, new docx writer, pdf/md/txt
  src/text.rs               Tokeniser, sentence splitter, lexicon, word-frequency table
  data/wordfreq.txt         Zipf table (generated)
  installer/                Icon source, DMG background, NSIS bitmaps
  tauri.conf.json           Bundle, updater, window config
.github/workflows/release.yml   CI: builds mac (arm64 + x64) and Windows, publishes release + latest.json
```

## Develop

Prerequisites: Rust (stable ≥ 1.90), Node 20+, and on Windows the WebView2 runtime (preinstalled on Win 10/11) and the MSVC build tools.

```bash
npm install
npm run tauri dev        # hot-reloading dev app
cd src-tauri && cargo test --lib   # engine tests
npm run tauri build      # installers in src-tauri/target/release/bundle/
```

## Releasing a new version

Installers under `releases/` are stored with **Git LFS** (see `.gitattributes`), so each release replaces an LFS object instead of growing the repo. Clone with `git lfs install && git clone …` to get the binaries rather than pointer files.

Only the **latest** installers are kept, in `releases/`:

| File | Built by |
|---|---|
| `Humanizer_x.y.z_aarch64.dmg` (Apple Silicon) | your Mac, via the script below |
| `Humanizer_x.y.z_x64.dmg` (Intel Mac) | GitHub Actions, after the tag is pushed |
| `Humanizer_x.y.z_x64-setup.exe` (Windows) | GitHub Actions, after the tag is pushed |

```bash
./scripts/release.sh 0.1.1
```

The script bumps the version in the three manifests, builds the DMG, replaces the old file in `releases/`, commits, tags `v0.1.1` and pushes. The workflow then builds the Windows and Intel-Mac installers on GitHub's runners and commits them into `releases/` on `main` (~10 min). No secrets or signing keys are involved.

Users install by downloading the file for their platform and running it; an existing installation is upgraded in place. Unsigned builds show a one-time Gatekeeper (right-click → Open) or SmartScreen (*More info → Run anyway*) prompt.

## Settings & privacy

- The API key is saved in the OS app-config folder (`~/Library/Application Support/com.arunaistudio.humanizer/settings.json` on macOS, `%APPDATA%\com.arunaistudio.humanizer\settings.json` on Windows), file mode 600 on macOS.
- Text leaves the machine only when it is sent to the Anthropic API for rewriting or the AI Judge. The five statistical detectors and the offline rule engine never touch the network.

## License

See `LICENSE`. © 2026 Arun Pandey, Arun AI Studio.

## Offline AI model

Humanizer ships small and fetches its language model on first launch (no click needed; turn off in Settings). Three tiers; the app recommends the strongest your RAM allows and you can switch any time:

| Tier | Model | Size | RAM |
|---|---|---|---|
| best | Gemma 3 12B (QAT, Q4_K_M) | 7.3 GB | 16 GB |
| balanced | Qwen3 8B (Q4_K_M, thinking off) | 5.0 GB | 12 GB |
| light | Qwen3 4B Instruct 2507 (Q4_K_M) | 2.5 GB | 8 GB |

Files live in the app data folder (`~/Library/Application Support/com.arunaistudio.humanizer/models/` on macOS, `%APPDATA%\com.arunaistudio.humanizer\models\` on Windows); downloads resume if interrupted. Any other `.gguf` instruct model works via *Use my own .gguf…*. Every local rewrite is gated by the detector suite: 1–3 candidates are drawn (Light/Balanced/Strong), the most human-scoring one wins, and the original is kept if none beats it.

**Full offline installer.** `./scripts/build-full.sh` produces a macOS DMG with the best model baked in (~7.5 GB) in `releases-full/`, and `./scripts/publish-full.sh` uploads it to a Hugging Face Hub repo (LFS-backed, 50 GB per file, no download caps). It cannot go on GitHub: even with LFS, GitHub stops at 2 GB per file and 1 GB/month of free LFS bandwidth. Building from source needs `cmake` (`brew install cmake`; present on GitHub runners and in Visual Studio Build Tools).
