#!/usr/bin/env bash
# build-local-bundle.sh — build a local bundle with a REAL MCP sidecar.
# Usage: build-local-bundle.sh [bundles]   (default: deb on Linux, app on macOS)
# Runs CI's sidecar recipe for the HOST triple, then bundles.
# Refuses Windows and universal targets (CI-only; spec §3).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# --- tool checks (spec §3) ---
for tool in cargo rustc node jq pnpm; do
  command -v "$tool" >/dev/null || { echo "ERROR: $tool not found in PATH" >&2; exit 1; }
done
python3 --version >/dev/null 2>&1 || { echo "ERROR: python3 not found (needed by the smoke test)" >&2; exit 1; }

TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
case "$TRIPLE" in
  *windows*) echo "ERROR: Windows bundles are CI-only (no bash recipe). Use the release workflow." >&2; exit 1 ;;
  *darwin*)
    case "$TRIPLE" in
      *universal*) echo "ERROR: universal macOS bundles are CI-only. Build for your host triple." >&2; exit 1 ;;
    esac
    BUNDLES="${1:-app}" ;;
  *) BUNDLES="${1:-deb}" ;;
esac

DEST="src-tauri/binaries/curated-thoughts-mcp-$TRIPLE"
mkdir -p src-tauri/binaries
[ -e "$DEST" ] || touch "$DEST"   # placeholder satisfies tauri-build; never truncate a real binary
# (No universal branch: rustc -vV never reports a universal host triple, so
#  this recipe physically cannot produce one — Opus plan-review m4.)

echo "== Building MCP sidecar (--features mcp-server) for $TRIPLE =="
cargo build --release --manifest-path src-tauri/Cargo.toml --features mcp-server --bin curated-thoughts

TARGET_DIR="$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)"
cp "$TARGET_DIR/release/curated-thoughts" "$DEST"
chmod +x "$DEST"

echo "== Verifying + smoke-testing the sidecar =="
node scripts/verify-sidecar.mjs "$DEST"
tools/smoke_test_mcp_sidecar.sh "$DEST"

echo "== Bundling ($BUNDLES) — the hook re-verifies independently =="
pnpm tauri build --bundles "$BUNDLES"

echo "== Done. Bundle sidecar verified; hook guard passed. =="
