#!/usr/bin/env bash
# Publishes the full offline installer (app + model, ~7.5 GB) to a Hugging
# Face Hub repository. HF stores large files with Git LFS, allows up to
# 50 GB per file and has no practical download limit, which GitHub does not
# (2 GB per file, 1 GB/month LFS bandwidth on the free plan).
#
# One-time setup:
#   pip install -U huggingface_hub           # provides the `hf` CLI
#   hf auth login                            # paste a token with "write" access
#   create the repo once: hf repo create humanizer-releases --type model
#
#   ./scripts/publish-full.sh [owner/repo]   # default: $HF_REPO or 00Arun/humanizer-releases
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
REPO="${1:-${HF_REPO:-00Arun/humanizer-releases}}"
VERSION=$(python3 -c "import json;print(json.load(open('src-tauri/tauri.conf.json'))['version'])")
FULL="releases-full/Humanizer_${VERSION}_aarch64_full.dmg"

if ! command -v hf >/dev/null 2>&1; then
  echo "hf CLI not found. Run: pip install -U huggingface_hub && hf auth login" >&2
  exit 1
fi
if [[ ! -f "$FULL" ]]; then
  echo "Missing $FULL. Build it first with ./scripts/build-full.sh" >&2
  exit 1
fi

cat > releases-full/README.md <<EOF
# Humanizer — full offline installer

**Humanizer v${VERSION}** by Arun Pandey · Arun AI Studio · https://arun-pandey.com.np/

This DMG contains the app **and** the Gemma 3 12B language model, so it works with no internet connection and no API key. Apple Silicon Macs with 16 GB RAM or more.

- \`Humanizer_${VERSION}_aarch64_full.dmg\` — open, drag Humanizer to Applications, done.

Smaller installers that download the model on first launch: https://github.com/00Arun/humanizer/tree/main/releases
EOF

echo "==> Uploading $FULL to https://huggingface.co/$REPO"
hf upload "$REPO" "$FULL" "Humanizer_${VERSION}_aarch64_full.dmg" --repo-type model --commit-message "Humanizer v${VERSION} full installer"
hf upload "$REPO" releases-full/README.md README.md --repo-type model --commit-message "Update README"
echo
echo "Done. Download page: https://huggingface.co/$REPO/tree/main"
