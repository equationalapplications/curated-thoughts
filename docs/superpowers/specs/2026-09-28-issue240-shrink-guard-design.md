# vault_write_note: size-drop guard + compaction-marker reject (issue #240)

**Date:** 2026-09-28
**Status:** Implemented 2026-09-28 (PR #248)
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

1. **D1 — size-drop guard** on the If-Match edit path; the exact trigger,
   floor, boundary, error shape, and measurement basis are pinned in D3/D4.
   **Placement (Opus spec c2 N1):** BOTH checks (shrink + marker) run after
   `render_document` (write.rs `:432`) and before `safe_write_bytes`
   (`:435`) — NOT immediately after `enforce_staleness` — because the
   measurement basis is the rendered body, which only exists after rendering
   (raw `body.len()` disagrees with the rendered length whenever a trailing
   newline gets appended: 512→513 bytes flips the pinned 1025 boundary
   pair).
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
4. **D4 — byte basis:** new body = rendered document length minus the split
   helper's offset (`doc.len() - offset`; `render_document` ensures the
   rendered body ends with at least one trailing `\n` — added only if
   missing; trailing newlines are never collapsed — the measurement basis is
   the RENDERED document) vs existing body =
   `content.len() - offset` — both measured via the new
   `split_frontmatter_fence` helper (item 6), never via
   `collect_frontmatter_fence` (mis-measures CRLF, see item 6).
   Body-to-body, frontmatter excluded from both sides.
5. **D5 — plumbing:** `allow_shrink` on `VaultWriteNoteParams` (schemars
   auto-derives the MCP schema), passed through both adapters; both
   production callers default to `false`. **Tauri command shape (Opus spec
   M1):** `lib.rs:877` takes `allow_shrink: Option<bool>` +
   `.unwrap_or(false)` — Tauri command params do NOT honor
   `#[serde(default)]`, so a plain `bool` would make `allowShrink` a
   required invoke key and break every existing invoke (incl.
   `tests/mcp_write_integration.rs:80-87`).
6. **CRLF-safe body split (Opus spec M2):** a new
   `split_frontmatter_fence(content) -> Option<(String, usize)>` helper
   sharing the fence logic — `collect_frontmatter_fence` (:146) returns only
   inner text with no offset and rebuilds via `lines()` (drops `\r`), so
   `content.len() - inner.len()` mis-measures every CRLF note. D4's byte
   basis uses the helper's offset; CRLF boundary test required.
7. Extend the refusal-contract list in the `write.rs` module header (:16-18).

**Pins (Opus spec minors + c2):** new-body bytes = rendered document length
minus the split helper's frontmatter offset (`doc.len() - offset`; the
rendered body ends with at least one trailing `\n` — added only if missing,
trailing newlines never collapsed — and "normalized" in earlier drafts meant
exactly this measured form);
marker error variant `CompactionMarkerRejected { marker: String }` with
Display `compaction_marker:{marker}: rephrase and resend without compaction
artifacts` (same no-`allow_shrink` rule as the shrink refusal); "newly
introduced" = the marker string is absent from the existing content —
FRONTMATTER INCLUDED (Opus spec c2 N2: scanning only the existing body would
lock any note whose title/description quotes a marker, since every
legitimate edit re-sends that frontmatter; presence, not count; the check
covers the incoming payload's body AND frontmatter symmetrically;
`allow_shrink` does NOT bypass the marker check); `MIN_GUARDED_BODY_BYTES =
1024`; check ORDER pinned: marker check first, then shrink check (both after
`render_document`, before `safe_write_bytes`); the module-header convention
claim is corrected to "Display strings are the contract; see each variant's
`#[error]`" (the literal `{detail}:{value}` shape doesn't hold for
`StaleUpdate`).

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
module's Display-string contract (see Pins for the exact strings). MCP
callers see the Display string via `anyhow!("{}", e)` (`tool_dispatch.rs:304`).

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
- `allow_shrink: true` permits a major shrink; `allow_shrink: false`/omitted
  (Tauri `Option<bool>` unwrap path) behaves identically to today.
- CRLF note: `content.len() - offset` basis measured via the new split
  helper matches the body length on CRLF files.
- c2-behaviour regression tests (Opus c3): marker quoted in EXISTING
  FRONTMATTER (title/description) + body re-sends it → edit succeeds
  (N2 lock-out); check ORDER — a payload that is both marker-tainted and
  shrunk refuses with `compaction_marker` first; rendered-length basis —
  body not ending in `\n` measures +1 after render and the boundary pair
  (1025/512) refuses exactly as pinned.
- `split_frontmatter_fence` returning `None` (existing file without a
  fence): pin = treat the whole existing content as body for measurement
  (`offset = 0`) and let frontmatter validation produce its own refusal
  downstream; test: fence-less existing file edits measure correctly and do
  not panic.
- Neither refusal's Display contains `allow_shrink`.
- Audit existing tests for shrinking fixture edits (long → `"x\n"`) — route
  intentional ones through `allow_shrink: true`.

## Out of scope / open questions

- #245 (frontmatter key-drop guard) — follow-up, filed.
- Other MCP write tools — issue scope is `vault_write_note` only.
- Frontend UX for the refusal — MCP errors surface as-is; no panel change.

Fixes #240.
