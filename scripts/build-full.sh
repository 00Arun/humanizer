#!/usr/bin/env bash
# Builds the "full" macOS installer with the offline AI model baked in
# (~7.5 GB, Gemma 3 12B). This file is too large for GitHub (100 MB repo / 2 GB release
# limits), so it is written to releases-full/ (git-ignored) for you to host
# on your own website, Google Drive, or hand out on a USB stick.
#
#   ./scripts/build-full.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"

MODEL_FILE="gemma-3-12b-it-qat-Q4_K_M.gguf"
MODEL_URL="https://huggingface.co/unsloth/gemma-3-12b-it-qat-GGUF/resolve/main/$MODEL_FILE"
APP_MODEL="$HOME/Library/Application Support/com.arunaistudio.humanizer/models/$MODEL_FILE"
STAGE="src-tauri/models/$MODEL_FILE"
VERSION=$(python3 -c "import json;print(json.load(open('src-tauri/tauri.conf.json'))['version'])")

mkdir -p src-tauri/models releases-full
if [[ ! -f "$STAGE" ]]; then
  if [[ -f "$APP_MODEL" ]]; then
    echo "==> Staging model from app data folder"
    cp "$APP_MODEL" "$STAGE"
  else
    echo "==> Downloading model (7.3 GB)"
    curl -L -C - -o "$STAGE.part" "$MODEL_URL" && mv "$STAGE.part" "$STAGE"
  fi
fi

echo "==> Building app bundle with the model inside"
rm -rf src-tauri/target/release/bundle/macos/Humanizer.app
npm run tauri build -- --bundles app --config src-tauri/tauri.full.conf.json

APP="src-tauri/target/release/bundle/macos/Humanizer.app"
# (grep -c, not -q: -q closes the pipe early and pipefail would flag a pass as a failure)
if [[ "$(strings "$APP/Contents/MacOS/humanizer" | grep -cE "llama_model_load|ggml_metal")" -eq 0 ]]; then
  echo "Built binary does not contain the offline AI engine. Aborting." >&2
  exit 1
fi
if [[ ! -f "$APP/Contents/Resources/models/$MODEL_FILE" ]]; then
  echo "Model was not bundled into the app resources. Aborting." >&2
  exit 1
fi

# Tauri's create-dmg step unmounts a staging image that Spotlight is still
# indexing and fails with "Resource busy" on multi-GB images, so build the
# DMG directly: app + Applications shortcut, compressed.
echo "==> Creating DMG (this compresses ~7.5 GB; several minutes)"
OUT="releases-full/Humanizer_${VERSION}_aarch64_full.dmg"
STAGING="$(mktemp -d)/Humanizer"
mkdir -p "$STAGING"
cp -R "$APP" "$STAGING/"
ln -s /Applications "$STAGING/Applications"
rm -f "$OUT"
hdiutil create -volname "Humanizer" -srcfolder "$STAGING" -ov -format UDZO -imagekey zlib-level=6 "$OUT"
rm -rf "$(dirname "$STAGING")"
ls -lh "$OUT"
echo
echo "Done: $ROOT/$OUT"
echo "Publish it with ./scripts/publish-full.sh (Hugging Face); it is too large for GitHub."
