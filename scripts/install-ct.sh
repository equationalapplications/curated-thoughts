#!/usr/bin/env bash
# install-ct.sh — installer for a freshly built Curated Thoughts .deb.
# Usage: install-ct.sh [path/to/Curated_Thoughts_VERSION_amd64.deb]
#   (no arg = newest .deb in src-tauri/target/release/bundle/deb/)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEB_DIRS=("$REPO_ROOT/target/release/bundle/deb" "$REPO_ROOT/src-tauri/target/release/bundle/deb")
DEB="${1:-}"
if [[ -z "$DEB" ]]; then
  for dir in "${DEB_DIRS[@]}"; do
    DEB=$(ls -t "$dir"/*.deb 2>/dev/null | head -1 || true)
    [[ -n "$DEB" ]] && break
  done
  if [[ -z "$DEB" ]]; then
    echo "ERROR: no .deb found under ${DEB_DIRS[*]} — run 'pnpm tauri build --bundles deb' first" >&2
    exit 1
  fi
fi
[[ -f "$DEB" ]] || { echo "ERROR: $DEB not found" >&2; exit 1; }

echo "== Installing $(basename "$DEB") =="
echo "   current: $(dpkg-query -W -f='${Version}' curated-thoughts 2>/dev/null || echo 'not installed')"

# Stop the running app before dpkg replaces the binary.
if pgrep -f '/usr/bin/curated-thoughts' >/dev/null 2>&1; then
  echo "== Stopping running Curated Thoughts =="
  pkill -f '/usr/bin/curated-thoughts' || true
  sleep 2
fi

sudo dpkg -i "$DEB"

echo "== Installed: $(dpkg-query -W -f='${Version}' curated-thoughts) =="

# Sanity: binary present + vault config intact (config-corruption guard, runbook §0.5)
command -v /usr/bin/curated-thoughts >/dev/null || { echo "ERROR: binary missing after install" >&2; exit 1; }
CFG="$HOME/.brain/config.json"
if [[ -f "$CFG" ]]; then
  if grep -q '/tmp/.tmp' "$CFG"; then
    echo "WARN: config.json contains a /tmp/.tmp path — possible corruption (runbook §0.5). Check before launching." >&2
  else
    echo "== config.json sanity: OK =="
  fi
fi

echo "== Done. Launch via the desktop icon or ~/.local/bin/curated-thoughts =="
