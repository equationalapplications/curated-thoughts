#!/usr/bin/env bash
# install-ct.sh — installer for a freshly built Curated Thoughts .deb.
# Usage: install-ct.sh [path/to/Curated_Thoughts_VERSION_amd64.deb]
#   (no arg = newest .deb in src-tauri/target/release/bundle/deb/)
set -euo pipefail

# NOTE: absolute path — when copied to ~/kv-script.sh, relative resolution would break.
REPO_ROOT="/home/kv-thinkpad-t420-ubuntu/code/github/equationalapplications/curated-thoughts"
DEB_DIRS=("$REPO_ROOT/target/release/bundle/deb" "$REPO_ROOT/src-tauri/target/release/bundle/deb")
DEB="${1:-}"
if [[ -z "$DEB" ]]; then
  for dir in "${DEB_DIRS[@]}"; do
    DEB=$(ls -t "$dir"/*.deb 2>/dev/null | head -1 || true)
    [[ -n "$DEB" ]] && break
  done
  if [[ -z "$DEB" ]]; then
    echo "ERROR: no .deb found under ${DEB_DIRS[*]} — run scripts/build-local-bundle.sh deb first" >&2
    exit 1
  fi
fi
[[ -f "$DEB" ]] || { echo "ERROR: $DEB not found" >&2; exit 1; }

# Sidecar gate: refuse a .deb whose packaged sidecar is a placeholder/broken
# (spec §4; reuses the repo verifier so the rules cannot drift).
TMP_EXTRACT="$(mktemp -d)"
trap 'rm -rf "$TMP_EXTRACT"' EXIT
dpkg-deb -x "$DEB" "$TMP_EXTRACT"
SIDECAR="$TMP_EXTRACT/usr/bin/curated-thoughts-mcp"
if [[ ! -e "$SIDECAR" ]]; then
  echo "ERROR: refusing to install: no sidecar in $DEB" >&2
  exit 1
fi
command -v node >/dev/null 2>&1 || { echo "ERROR: node is required for the sidecar gate but was not found in PATH" >&2; exit 1; }
node "$REPO_ROOT/scripts/verify-sidecar.mjs" "$SIDECAR" || {
  echo "ERROR: refusing to install: broken sidecar in $DEB" >&2
  exit 1
}

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
