# vault_write_note: size-drop guard + compaction-marker reject (issue #240)

**Date:** 2026-09-28
**Status:** Draft
**Branch:** feat/issue-240-shrink-guard
**Priority:** High (real production incident 2026-09-26; write-path hardening)

## Problem

`vault_write_note` validates *conflict* (If-Match staleness) and *shape*
(frontmatter round-trip) but never *magnitude*. On 2026-09-26 a mangled
agent payload (truncated upstream in context compaction — valid OKF,
parseable frontmatter, matching If-Match token) replaced a 12,860-byte
procedure note with 505 bytes, silently; damage went unnoticed ~20 hours and
required git-history restoration (see the incident note in the delivery-flow
procedure). All existing gates passed.

Full investigation (write-path map, Opus verdict history across 3 cycles):
`2026-09-28-issue240-shrink-guard-investigation.md` (same directory).

## Approach

All in-repo — the OKF write path has NO core-okf dependency (grep-verified);
core is `write_note` (`src-tauri/src/okf/write.rs:297`), fed by the MCP
dispatch adapter (`tool_dispatch.rs:287`, params at :1135-1141) and the
Tauri command (`lib.rs:877`). No internal repair pass writes through
`write_note`; the frontend never calls the tool.

1. **D1 — size-drop guard** on the If-Match edit path, after
   `enforce_staleness` (:403), before token rotation; the exact trigger,
   floor, boundary, error shape, and measurement basis are pinned in D3/D4.
2. **D2 — compaction-marker reject:** creates reject ALL markers
   (`[SKILL_PRUNED]`, `HERMES-CONTEXT-COMPRESSION`, const list); edits reject
   only NEWLY-INTRODUCED markers (a note quoting a marker — e.g. the
   incident note itself — stays appendable). Refusals teach re-read/rephrase
   and never mention `allow_shrink` (a compacted agent reading that flag
   bypasses in one retry).
3. **D3 — guard shape:** integer math, REFUSED iff `new * 2 < existing`
   (`==` allowed; odd-size boundary example 1025→512); applies only when the
   existing body ≥ `MIN_GUARDED_BODY_BYTES` (~1 KiB — small-note rewrites
   stay free); `allow_shrink: true` (serde-default false) overrides on
   edits. Error: `ShrinkRefused { existing_bytes, new_bytes }`, Display
   PINNED to
   `shrink_refused:{existing_bytes}:{new_bytes}: re-read the note and resend the full body`.
4. **D4 — byte basis:** new body (normalized, exactly what `render_document`
   appends) vs existing body (via `collect_frontmatter_fence`, :146) —
   body-to-body, frontmatter excluded from both sides.
5. **D5 — plumbing:** `allow_shrink` on `VaultWriteNoteParams` (schemars
   auto-derives the MCP schema), passed through both adapters; both
   production callers default to `false`.
6. Extend the refusal-contract list in the `write.rs` module header (:16-18).

**Rejected alternatives:** float ratio const (superseded by integer math);
full-content comparison basis (frontmatter size noise can flip borderline
cases — Opus c2 finding 1); unconditional marker reject on edits (locks the
incident note itself — Opus c1 M2); advertising `allow_shrink` in error text
(bypass invite — Opus c1 M4).

**Explicitly out of scope:** frontmatter key-drop on edits — real gap,
`check_round_trip` never sees the existing file's keys — tracked as #245,
not smuggled in here (semantic change deserves its own design).

## Error handling

Two new `WriteNoteError` refusal variants (shrink, marker), following the
module's `{detail}:{value}` Display convention. MCP callers see the Display
string via `anyhow!("{}", e)` (`tool_dispatch.rs:304`).

## Testing

(`cargo test -p curated-thoughts okf::write`; 56 existing tests, helpers
`vault()`/`fm()`.)

- Incident replay: synthetic 12,860→505 BODY-size edit rejected (numbers are
  body sizes per D4, not whole-file sizes); `starts_with` prefix assertion +
  re-read instruction present.
- Boundary: `new * 2 == existing` allowed; `new * 2 < existing` refused
  (incl. odd pair 1025→512); existing < floor → any shrink free.
- Markers: create rejected; newly-introduced-on-edit rejected;
  already-present marker + append succeeds.
- `allow_shrink: true` permits a major shrink.
- Neither refusal's Display contains `allow_shrink`.
- Audit existing tests for shrinking fixture edits (long → `"x\n"`) — route
  intentional ones through `allow_shrink: true`.

## Out of scope / open questions

- #245 (frontmatter key-drop guard) — follow-up, filed.
- Other MCP write tools — issue scope is `vault_write_note` only.
- Frontend UX for the refusal — MCP errors surface as-is; no panel change.

Fixes #240.
