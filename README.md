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
| Rewrite engine | Claude **Fable 5.1** (default; Opus 5.5 / Sonnet 5.5 selectable) via your own Anthropic API key, with our prompt, paragraph-chunked and run in parallel. Strong mode runs a second pass guided by our own detector's flags. |
| Offline fallback | With no API key, a rule engine still removes machine idioms, adds contractions, softens em-dashes and varies sentence rhythm |
| Auto-update | Checks GitHub Releases on launch, downloads, verifies the minisign signature, installs and relaunches |
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

## Release & auto-update

Installed copies poll `https://github.com/00Arun/humanizer-releases/releases/latest/download/latest.json` on launch. The code repo (`00Arun/humanizer`) is private; `humanizer-releases` is a public repo that holds only the installers, signatures and `latest.json`, mirrored there by CI.

One-time setup:

1. Code lives in the private repo `00Arun/humanizer`. Create a second, PUBLIC, empty repo `00Arun/humanizer-releases` (binaries only), and a fine-grained PAT with *Contents: read and write* on it → secret `RELEASES_REPO_TOKEN`.
2. In **Settings → Secrets and variables → Actions** add:
   - `TAURI_SIGNING_PRIVATE_KEY` = contents of `~/.tauri/humanizer.key` (generated on this machine; **back it up** — without it installed copies can never update again)
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` = empty string (the key has no password)
   - Optional Apple notarisation secrets (`APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`) to remove the macOS Gatekeeper warning. Windows code-signing is optional too; unsigned installers show a SmartScreen prompt the first few hundred times.

Each release:

```bash
# bump the version in package.json, src-tauri/tauri.conf.json and src-tauri/Cargo.toml (keep them identical)
git commit -am "v0.1.1"
git tag v0.1.1 && git push && git push --tags
```

CI builds the three installers, signs the updater artifacts, creates the GitHub Release and uploads `latest.json`. Every installed copy picks the update up on its next launch (or via *Check for updates* in Settings / About).

## Settings & privacy

- The API key is saved in the OS app-config folder (`~/Library/Application Support/com.arunaistudio.humanizer/settings.json` on macOS, `%APPDATA%\com.arunaistudio.humanizer\settings.json` on Windows), file mode 600 on macOS.
- Text leaves the machine only when it is sent to the Anthropic API for rewriting or the AI Judge. The five statistical detectors and the offline rule engine never touch the network.

## License

See `LICENSE`. © 2026 Arun Pandey, Arun AI Studio.
