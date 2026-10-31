#!/usr/bin/env bash
# Local release: bump the version, build the macOS installer, drop it into
# releases/ (replacing the previous one), commit, tag and push.
#
#   ./scripts/release.sh 0.1.1
#
# The Windows installer is built by GitHub Actions when the tag lands and is
# committed into releases/ by the workflow (see .github/workflows/release.yml).
set -euo pipefail

VERSION="${1:-}"
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "usage: $0 <version>   e.g. $0 0.1.1" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"

if [[ -n "$(git status --porcelain)" ]]; then
  echo "Working tree is not clean. Commit or stash first." >&2
  exit 1
fi

echo "==> Setting version $VERSION"
./scripts/bump-version.sh "$VERSION"

echo "==> Building macOS installer"
# Remove the previous DMG first so a failed build can never ship a stale file.
rm -f src-tauri/target/release/bundle/dmg/Humanizer_*.dmg
npm run tauri build -- --bundles dmg
DMG=$(ls src-tauri/target/release/bundle/dmg/Humanizer_"$VERSION"_*.dmg)
# The engine must be inside: llama.cpp leaves its Metal kernels in the binary.
# (grep -c, not -q: -q closes the pipe early and pipefail would flag a pass as a failure)
if [[ "$(strings src-tauri/target/release/humanizer | grep -cE "llama_model_load|ggml_metal")" -eq 0 ]]; then
  echo "Built binary does not contain the offline AI engine. Aborting." >&2
  exit 1
fi

echo "==> Updating releases/"
mkdir -p releases
rm -f releases/Humanizer_*_aarch64.dmg releases/Humanizer_*_x64.dmg
cp "$DMG" releases/
cat > releases/VERSION <<EOF
$VERSION
EOF
ls -la releases/

echo "==> Committing and tagging v$VERSION"
git add -A
git commit -m "Release v$VERSION"
git tag -a "v$VERSION" -m "Humanizer v$VERSION"
git push && git push --tags

echo
echo "Done. GitHub Actions will add the Windows installer to releases/ in ~10 minutes."
echo "Downloads: https://github.com/00Arun/humanizer/tree/main/releases"
