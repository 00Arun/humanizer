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

echo "==> Building full DMG (app + model)"
rm -f src-tauri/target/release/bundle/dmg/Humanizer_*.dmg
npm run tauri build -- --bundles dmg --config src-tauri/tauri.full.conf.json

if ! strings src-tauri/target/release/humanizer | grep -q "kernel void kernel_"; then
  echo "Built binary does not contain the offline AI engine. Aborting." >&2
  exit 1
fi

OUT="releases-full/Humanizer_${VERSION}_aarch64_full.dmg"
mv src-tauri/target/release/bundle/dmg/Humanizer_"$VERSION"_aarch64.dmg "$OUT"
ls -lh "$OUT"
echo
echo "Done: $ROOT/$OUT"
echo "Host this file yourself (website / Drive); it is too large for GitHub."
