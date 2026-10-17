#!/usr/bin/env bash
# calibrate-two-stage.sh — spec §9 step 3/3a ordered runbook (r22 MAJOR-4):
# bootstrap (verify-scratch stamp + sampled backfill) MUST precede calibration;
# repair only when coverage < 100%; flip only at the end, via Task 15's
# letter-gated machinery. See docs/superpowers/plans/2026-10-15-issue271-two-stage-retrieval.md
# Tasks 12-16 and docs/superpowers/specs/2026-10-09-issue271-two-stage-retrieval-design.md §6/§7/§9.
#
# Usage:
#   scripts/calibrate-two-stage.sh --probes <probes.jsonl> --baseline-key <v1 key> [--out <dir>]
#
# Exit codes: 0 ok · 2 coverage still < 100% after repair · 3 usage · 1 unexpected.
set -euo pipefail

PROBES=""
BASELINE_KEY=""
OUT_DIR="calibration-run"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --probes)       PROBES="${2:?--probes requires a value}"; shift 2 ;;
    --baseline-key) BASELINE_KEY="${2:?--baseline-key requires a value}"; shift 2 ;;
    --out)          OUT_DIR="${2:?--out requires a value}"; shift 2 ;;
    *) echo "unknown argument: $1 (usage: $0 --probes <jsonl> --baseline-key <v1 key> [--out <dir>])" >&2; exit 3 ;;
  esac
done
[[ -n "$PROBES" ]] || { echo "--probes is required" >&2; exit 3; }
[[ -n "$BASELINE_KEY" ]] || { echo "--baseline-key is required" >&2; exit 3; }
[[ -f "$PROBES" ]] || { echo "probes file not found: $PROBES" >&2; exit 3; }

CT() { cargo run --quiet --manifest-path tools/Cargo.toml --bin ct -- "$@"; }
CAL() { cargo run --quiet --manifest-path tools/Cargo.toml --bin calibrate_two_stage -- "$@"; }

# coverage: `<covered>/<candidates>` — the Task 13 output contract (reindex.rs
# print_coverage: "coverage: {cov}/{n}"). Returns 0 iff covered == candidates.
coverage_is_full() {
  local line cov n
  line=$(CT reindex verify-scratch --dry-run 2>/dev/null | grep 'coverage: ' | tail -1)
  cov="${line#coverage: }"; cov="${cov%%/*}"
  n="${line#*/}"
  [[ "$cov" =~ ^[0-9]+$ && "$n" =~ ^[0-9]+$ ]] || { echo "could not parse coverage line: '$line'" >&2; return 1; }
  [[ "$cov" -eq "$n" ]]
}

mkdir -p "$OUT_DIR"

echo "== step 3a/1: verify-scratch on the LIVE brain (stamp + sampled backfill) =="
CT reindex verify-scratch

echo "== step 3a/2: coverage gate (dry-run re-check) =="
if ! coverage_is_full; then
  echo "coverage < 100% — running repair-embed-key --stale, then re-verifying" >&2
  CT reindex repair-embed-key --stale
  CT reindex verify-scratch
  if ! coverage_is_full; then
    echo "coverage still < 100% after repair — ABORTING before calibration." >&2
    echo "Inspect skipped docs above; resolve the underlying files/rows, then re-run this script." >&2
    exit 2
  fi
fi
echo "coverage 100% — cleared for calibration"

echo "== step 3/3: paired calibration run (both arms, one invocation) =="
ARTIFACT="$OUT_DIR/artifact.json"
CAL --probes "$PROBES" --baseline-key "$BASELINE_KEY" --json-out "$ARTIFACT"

echo "== step 3/4: grid pick (tie-break per GLM r1 minor-5) =="
CAL pick --grid "$OUT_DIR"

echo "== step 3/5: letter-gated flip (refuses on anything but a passing letter) =="
CAL flip --artifact "$ARTIFACT"

echo "== done: apply the emitted patch snippet only after human review of the letter =="
