#!/usr/bin/env bash
# Bumps the app version in all three manifests (kept identical).
#   ./scripts/bump-version.sh            # patch: 0.2.0 -> 0.2.1
#   ./scripts/bump-version.sh minor      # 0.2.1 -> 0.3.0
#   ./scripts/bump-version.sh 1.0.0      # explicit
set -euo pipefail
cd "$(dirname "$0")/.."
CUR=$(python3 -c "import json;print(json.load(open('src-tauri/tauri.conf.json'))['version'])")
IFS=. read -r MA MI PA <<<"$CUR"
case "${1:-patch}" in
  patch) NEW="$MA.$MI.$((PA+1))" ;;
  minor) NEW="$MA.$((MI+1)).0" ;;
  major) NEW="$((MA+1)).0.0" ;;
  *) NEW="$1" ;;
esac
[[ "$NEW" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "bad version: $NEW" >&2; exit 1; }
sed -i '' "s/\"version\": \"$CUR\"/\"version\": \"$NEW\"/" package.json src-tauri/tauri.conf.json
python3 - "$CUR" "$NEW" <<'PY'
import sys,re
p='src-tauri/Cargo.toml'; s=open(p).read()
s=re.sub(r'^version = "[0-9.]+"', 'version = "'+sys.argv[2]+'"', s, count=1, flags=re.M)
open(p,'w').write(s)
PY
echo "$NEW" > releases/VERSION
echo "$CUR -> $NEW"
