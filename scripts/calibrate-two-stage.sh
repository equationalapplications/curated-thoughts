#!/usr/bin/env bash
# calibrate-two-stage.sh — spec §9 step 3/3a ordered runbook (r22 MAJOR-4):
# bootstrap (verify-scratch stamp + sampled backfill) MUST precede calibration;
# repair only when coverage < 100%; flip only at the end, via Task 15's
# letter-gated machinery. See docs/superpowers/plans/2026-10-15-issue271-two-stage-retrieval.md
# Tasks 12-16 and docs/superpowers/specs/2026-10-09-issue271-two-stage-retrieval-design.md §6/§7/§9.
#
# Usage:
#   scripts/calibrate-two-stage.sh --probes <probes.jsonl> --baseline-key <v1 key> \
#     --cell <rule>:<floor>:<k> [--cell <rule>:<floor>:<k> ...] [--out <dir>]
#
# pr283-review findings 4+5: the paired-run step now (a) exports CT_BUILD_SHA
# so the artifact carries code provenance and `flip`'s staleness check means
# something (flip_plan REFUSES an unversioned artifact/build), and (b) runs
# ONE paired live run PER GRID CELL (the CLI's live-run path requires
# --rule/--floor/--k; `pick` sweeps per-cell artifacts; `flip` takes the
# WINNING cell's artifact).
#
# Exit codes: 0 ok · 2 coverage still < 100% after repair · 3 usage · 1 unexpected.
set -euo pipefail

PROBES=""
BASELINE_KEY=""
OUT_DIR="calibration-run"
CELLS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --probes)       PROBES="${2:?--probes requires a value}"; shift 2 ;;
    --baseline-key) BASELINE_KEY="${2:?--baseline-key requires a value}"; shift 2 ;;
    --cell)         CELLS+=("${2:?--cell requires a value}"); shift 2 ;;
    --out)          OUT_DIR="${2:?--out requires a value}"; shift 2 ;;
    *) echo "unknown argument: $1 (usage: $0 --probes <jsonl> --baseline-key <v1 key> --cell <rule>:<floor>:<k> [--cell ...] [--out <dir>])" >&2; exit 3 ;;
  esac
done
[[ -n "$PROBES" ]] || { echo "--probes is required" >&2; exit 3; }
[[ -n "$BASELINE_KEY" ]] || { echo "--baseline-key is required" >&2; exit 3; }
[[ ${#CELLS[@]} -gt 0 ]] || { echo "at least one --cell <rule>:<floor>:<k> is required (§9 step 3 grid)" >&2; exit 3; }
[[ -f "$PROBES" ]] || { echo "probes file not found: $PROBES" >&2; exit 3; }
for cell in "${CELLS[@]}"; do
  [[ "$cell" =~ ^(restricted-cosine|chunk-hop):[0-9.]+:[0-9]+$ ]] || {
    echo "bad --cell '$cell' (expected <rule>:<floor>:<k>, rule = restricted-cosine|chunk-hop)" >&2
    exit 3
  }
done

# pr283-review finding 4: the artifact must carry code provenance — flip_plan
# refuses "unversioned" artifacts AND unversioned builds. option_env! is
# compile-time; tools/build.rs pins rerun-if-env-changed so a changed sha
# rebuilds the binary.
CT_BUILD_SHA="$(git rev-parse HEAD)"
export CT_BUILD_SHA
echo "code provenance: CT_BUILD_SHA=${CT_BUILD_SHA}"

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

echo "== step 3/3: paired calibration run, ONE per grid cell (§9 step 3 grid) =="
for cell in "${CELLS[@]}"; do
  rule="${cell%%:*}"
  rest="${cell#*:}"
  floor="${rest%%:*}"
  k="${rest##*:}"
  artifact="$OUT_DIR/cell-${rule}-${floor}-${k}.json"
  echo "-- cell rule=$rule floor=$floor k=$k → $artifact"
  CAL --probes "$PROBES" --baseline-key "$BASELINE_KEY" \
      --rule "$rule" --floor "$floor" --k "$k" \
      --json-out "$artifact"
done

echo "== step 3/4: grid pick over the per-cell artifacts (tie-break per GLM r1 minor-5) =="
CAL pick --grid "$OUT_DIR" --json-out "$OUT_DIR/pick-report.json"

# Identify the WINNING cell's artifact (flip takes ONE artifact): match the
# pick report's winner (rule, floor, k) against each per-cell artifact's
# recorded candidate cell. Floor compared numerically (awk) — "0.35" vs
# "0.350" must match.
winner_rule=$(sed -n 's/^    "rule": "\(.*\)",\?$/\1/p' "$OUT_DIR/pick-report.json" | head -1)
winner_floor=$(sed -n 's/^    "floor": \([0-9.]*\),\?$/\1/p' "$OUT_DIR/pick-report.json" | head -1)
winner_k=$(sed -n 's/^    "k": \([0-9]*\),\?$/\1/p' "$OUT_DIR/pick-report.json" | head -1)
[[ -n "$winner_rule" && -n "$winner_floor" && -n "$winner_k" ]] || {
  echo "could not parse the winner from $OUT_DIR/pick-report.json" >&2
  exit 1
}
WINNING_ARTIFACT=""
for artifact in "$OUT_DIR"/cell-*.json; do
  a_rule=$(sed -n 's/^    "rule": "\(.*\)",\?$/\1/p' "$artifact" | head -1)
  a_k=$(sed -n 's/^    "k": \([0-9]*\),\?$/\1/p' "$artifact" | head -1)
  a_floor=$(sed -n 's/^    "floor": \([0-9.]*\),\?$/\1/p' "$artifact" | head -1)
  [[ "$a_rule" == "$winner_rule" && "$a_k" == "$winner_k" ]] || continue
  if awk -v a="$a_floor" -v b="$winner_floor" 'BEGIN { exit !(a == b) }'; then
    WINNING_ARTIFACT="$artifact"
    break
  fi
done
[[ -n "$WINNING_ARTIFACT" ]] || {
  echo "no per-cell artifact matches the picked winner (rule=$winner_rule floor=$winner_floor k=$winner_k)" >&2
  exit 1
}
echo "winning cell artifact: $WINNING_ARTIFACT"

echo "== step 3/5: letter-gated flip on the WINNING cell (refuses on anything but a passing letter) =="
CAL flip --artifact "$WINNING_ARTIFACT"

echo "== done: apply the emitted patch snippet only after human review of the letter =="
