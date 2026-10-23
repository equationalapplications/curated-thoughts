# INTENT.md status-line compliance + AGENTS.md spec-alignment rule

**Date:** 2026-10-23
**Status:** Implemented in this PR (docs-only)
**Branch:** `docs/intent-compliance`

## Problem

INTENT.md's legend (lines 9-17) requires every rule's Status line to use
exactly one of: Built / Partly built / Planned / Open decision. Two rules
drift from the legend:

- Machine sync (line 154 on `main`): "Mostly planned" — not a legend
  value. The outbox exists, so "Partly built" is the accurate value; the
  status body already states what is missing.
- Fast model judges (line 168 on `main`): "Partly built, used
  differently" — the trailing qualifier is not part of the legend; the
  status body already describes how Jev is used differently, so plain
  "Partly built" suffices.

Second delta: AGENTS.md has no rule tying specs to INTENT.md, even though
INTENT.md declares itself the winner on intent.

## Proposed change

1. Rewrite the two Status lines to legend values (status wording only;
   rule meaning unchanged).
2. Add a short "Spec alignment" section to AGENTS.md: every spec is
   checked against INTENT.md; a spec that contradicts INTENT.md is wrong
   until INTENT.md is amended — INTENT wins on intent, specs win on
   detail.

## Plan

1. Patch the two `INTENT.md` Status lines (wording only).
2. Append the "Spec alignment" section to `AGENTS.md`.
3. Docs-only change — nothing to test beyond a clean diff; CI green
   before merge.

## Files touched

- `INTENT.md` (two Status lines)
- `AGENTS.md` (new section)
- this spec
