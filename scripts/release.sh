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
sed -i '' "s/\"version\": \"[0-9.]*\"/\"version\": \"$VERSION\"/" package.json src-tauri/tauri.conf.json
sed -i '' "0,/^version = \"[0-9.]*\"/s//version = \"$VERSION\"/" src-tauri/Cargo.toml
(cd src-tauri && cargo generate-lockfile --offline >/dev/null 2>&1 || cargo update -p humanizer --offline >/dev/null 2>&1 || true)

echo "==> Building macOS installer"
npm run tauri build -- --bundles dmg

echo "==> Updating releases/"
mkdir -p releases
rm -f releases/Humanizer_*_aarch64.dmg releases/Humanizer_*_x64.dmg
cp src-tauri/target/release/bundle/dmg/Humanizer_"$VERSION"_*.dmg releases/
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
