# Investigation — issue #240: vault_write_note size-drop guard

**Date:** 2026-09-28
**Status:** Investigation (Step 0 of the delivery flow)
**Issue:** #240 (enhancement — write-path hardening, real incident 2026-09-26)
**Evidence tags:** [V] = controller-verified (re-read lines / re-ran command); [C] = child-reported

## Repro of the failure class

The incident (2026-09-26, production vault): a `vault_write_note` call intended to
APPEND a lesson to a 226-line / 12,860-byte procedure note arrived as a ~505-byte
fragment — valid OKF, parseable frontmatter, matching If-Match token. Every
validation gate passed and the edit **succeeded silently**, replacing 12,860 bytes
with 505. Damage went unnoticed ~20 hours. This exact file was restored from git
commit 842ffc4 (see the incident note at the end of the delivery-flow procedure).

A live repro of the *guard* is part of the implementation plan (TDD: replay
12,860→505 and assert rejection). Reproducing the original truncation upstream
(agent context compaction) is not possible in-test; the unit under test is the
rejection, not the truncation.

## Code-level current state [V]

All verified at repo `eab433f` (main, == installed v2.20.1).

**Write path (three surfaces, ONE core):**
1. MCP: `src-tauri/src/mcp_server.rs:126-139` → `tool_dispatch::dispatch_tool_call`
2. Dispatch: `src-tauri/src/tool_dispatch.rs:287` (`dispatch_vault_write_note`, thin
   adapter); MCP params struct `VaultWriteNoteParams` at `tool_dispatch.rs:1135-1141`
   (`path`, `frontmatter`, `body`); call site `tool_dispatch.rs:1513`
3. Tauri command: `src-tauri/src/lib.rs:877`, calling core at `lib.rs:887`
4. **Core: `src-tauri/src/okf/write.rs:297` `pub fn write_note()`** — existing content
   read at `write.rs:393-402` (NotFound → None; non-UTF8 → `existing_unparsable:parse`
   refusal; other read errors → `WriteError`), If-Match enforcement via
   `enforce_staleness` at `write.rs:403` (fn at `write.rs:177`), token rotation at
   `write.rs:405-419`, then the write.

**Error variants:** `WriteNoteError` at `src-tauri/src/okf/mod.rs:76-93`:
`PathOutsideVault`, `DisallowedRoot`, `InvalidFrontmatter(String)`, `StaleUpdate`,
`WriteError(String)`. String-shape convention for machine-readable refusals:
`{detail}:{value}` (e.g. `existing_unparsable:parse`, `write_error:disallowed_root:...`).
The contract list in the module header (`write.rs:16-18`) enumerates every refusal
reason and must be extended with any new one.

**Callers [V]:** production callers — the Tauri command
(`lib.rs:887`) and the MCP dispatch adapter (`tool_dispatch.rs:297`). No internal
repair/heal pass writes through `write_note` (the repair scan is read-only; it
reports unparsable notes, never rewrites them). The frontend never calls
`vault_write_note` (verified by Opus c1), so `allow_shrink: false` default locks
out no human screen. TEST callers also exist: `tests/mcp_write_integration.rs`
(:836/:867/:881/:904) and the 56 `write.rs` unit tests — the plan must audit
these for shrinking edits (long body → `"x\n"`) and route them through
`allow_shrink: true` where the shrink is intentional fixture-setup.

**No core-okf dependency [V]:** grep over `Cargo.toml`, `tools/Cargo.toml`,
`Cargo.lock` for `core-okf|core_okf` → zero hits. The entire OKF write path is
in-repo at `src-tauri/src/okf/`. The guard belongs in-repo; no upstream
coordination. (Note: the vault MEMORY RULES mention core-okf 7.1.0 as a pinned
dep of the *wiki packages*; that does not apply to this Rust workspace.)

**Existing tests [V]:** `mod tests` at `write.rs:771` (56 `#[test]` functions,
pure `tempfile`, no Tauri runtime). Relevant patterns: `d2_edit_requires_exact_token` (:1044),
`write_refuses_non_utf8_existing_file_without_clobbering` (:908),
`stale_carries_tolerant_read_current_token` (:971). Helpers `vault()` (:983),
`fm()` (:990). Run: `cargo test -p curated-thoughts okf::write`.

## Root cause [V]

`write_note` validates *conflict* (If-Match staleness) and *shape* (frontmatter,
key-set round-trip) but never *magnitude*. An LLM-driven write whose payload was
mangled upstream (context compaction) is still perfectly valid OKF — so the
existing gates cannot see it. The fix is a cheap backstop on the one dimension
that distinguishes "append a lesson" from "clobber the note": size delta.

## Proposed fix directions

**D1 — size-drop guard on the If-Match edit path (primary, issue's ask).**
After the staleness check (`write.rs:403`), before token rotation: reject an
edit whose new body is drastically smaller than the existing body, unless the
caller explicitly passed `allow_shrink: true`. Creates (no existing file) and
growing edits are unaffected. **The precise trigger, floor, boundary, error
shape, and measurement basis are pinned in D3 and D4 — D1 does not restate
place)** (an earlier draft's float-ratio/full-content-fallback wording here
produced contradictory plan inputs in review; the pins live in exactly one
place).

**D2 — compaction-marker hard reject (complementary, issue's ask; refined per
Opus c1 M2).**
Reject payloads containing context-compaction markers — const list, at minimum
`[SKILL_PRUNED]` and `HERMES-CONTEXT-COMPRESSION` — with its own refusal shape
(e.g. `compaction_marker:{token}`).
- Creates: reject ALL markers unconditionally (no override).
- Edits: reject only markers NOT already present in the existing body. A note
  that legitimately quotes `[SKILL_PRUNED]` (quite possibly the incident note
  itself) must remain appendable — otherwise the guard locks its own incident
  report on day one. A marker newly introduced by the edit is presumptively
  compaction debris.
- Error wording must NOT advertise `allow_shrink` (Opus c1 M4): a compacted
  agent reading "pass allow_shrink: true" would bypass the guard in one retry.
  Word the refusal as an instruction to re-read the note and resend the full
  body; `allow_shrink` is documented only in the tool schema, for deliberate
  full rewrites.

**D3 — size-drop guard shape (per Opus c1 M1/M3):**
- **Minimum-size floor:** the ratio test applies only when the EXISTING body
  is at least `MIN_GUARDED_BODY_BYTES` (named const, ~1 KiB — the incident was
  12,860 bytes). Below the floor, a legitimate full rewrite of a small note
  (80→30 bytes) must stay possible without `allow_shrink`.
- **Integer math:** `new_bytes * 2 < existing_bytes` (no float ratio const).
  Boundary defined on the integer condition itself: the write is REFUSED iff
  `new * 2 < existing`; `new * 2 == existing` is ALLOWED (no "existing/2"
  phrasing — it is ambiguous for odd `existing`: at existing=1025, new=512 is
  refused because 1024 < 1025). Boundary tests use an existing body at or
  above `MIN_GUARDED_BODY_BYTES` so the floor never masks the boundary case,
  with (existing, new) pairs hitting both sides of the condition.
- **Error variant:** `ShrinkRefused { existing_bytes: usize, new_bytes: usize }`
  with the `#[error]` Display PINNED to
  `shrink_refused:{existing_bytes}:{new_bytes}: re-read the note and resend the full body`
  — a machine-readable prefix (matches `shrink_refused:{e}:{n}` via
  `starts_with`) PLUS the fixed instruction suffix. Both requirements are
  satisfiable only in this combined form: MCP errors surface via
  `anyhow!("{}", e)` (`tool_dispatch.rs:304`), so the caller sees exactly this
  Display string; tests assert the prefix with `starts_with` AND that the
  string does not contain `allow_shrink` (an exact-equality test on the bare
  prefix would leave no room for the instruction and make the M4 guard
  vacuous).

**D4 — byte basis (Opus c1 m2, pinned):**
Compare the NEW body against the EXISTING body, both measured as the note body
(frontmatter excluded). The existing body is scannable via
`collect_frontmatter_fence` (`write.rs:146`) — same fence logic the token
reader uses; do not claim the existing file "must be parseable" (the tolerant
path accepts colon titles). The new body is measured normalized — exactly the
bytes `render_document` will append after the frontmatter.

**Frontmatter-drop gap (Opus c1 M1 — NOT covered here, tracked as issue #245):**
OQ3 previously claimed the key-set round-trip guard blocks frontmatter drops.
That is FALSE: `check_round_trip` (`write.rs:433`, fn at :480) compares the
RENDERED document against the INCOMING frontmatter only — it never sees the
existing file's keys. An edit that silently drops `tags`/`supersedes` passes
every current gate. Fixing it is a semantic change (what makes a key-drop
"unintended"?) and is deferred to a follow-up issue rather than smuggled into
this guard. This investigation doc's OQ3 claim stands corrected.

**D5 — plumbing.** `allow_shrink` (serde default false) on `VaultWriteNoteParams`
(`tool_dispatch.rs:1137`; MCP schema auto-derives via schemars), pass-through at
`tool_dispatch.rs:1513` and the Tauri command (`lib.rs:877`), 6th param (or params
struct) on `write_note`. Both callers pass `false` today.

**Tests (TDD):** incident replay (a synthetic 12,860→505 BODY-size edit
rejected — the numbers are body sizes per D4, not the whole-file incident
sizes; `err.to_string()`
`starts_with("shrink_refused:12860:505")` and contains the re-read
instruction); boundary tests (existing ≥ `MIN_GUARDED_BODY_BYTES`; `new * 2 ==
existing` allowed; `new * 2 < existing` — including an odd-`existing` pair
like 1025→512 — refused; existing below the floor → any shrink allowed
without `allow_shrink`); marker rejected on create; marker
NEWLY-introduced on edit rejected; marker ALREADY in existing body + append
succeeds; `allow_shrink: true` permits a major shrink; error text does NOT
contain the string `allow_shrink` for EITHER refusal variant (assert both the
shrink and the marker refusal teach re-read/rephrase, not bypass).

## Open questions

- **OQ1 (threshold):** 50% is the issue's proposal. Rationale: legitimate edits
  (append a lesson, fix a typo) never halve a note; legitimate replacements
  (full rewrite) are rare and can pass `allow_shrink` deliberately. Implementation
  is integer math (`new * 2 < existing`, Opus c1 m3) plus the
  `MIN_GUARDED_BODY_BYTES` floor (D3) — the float-ratio const originally
  proposed here is superseded.
- **OQ2 (marker false positives — resolved by Opus c1 M2):** edits reject only
  NEWLY-INTRODUCED markers (markers already present in the existing body stay
  legal, so notes quoting `[SKILL_PRUNED]` — e.g. the incident note itself —
  remain editable); creates reject all. No override flag for markers: the
  escape is to rephrase, since a newly-introduced marker is presumptively
  compaction debris. See D2.
- **OQ3 (frontmatter shrink — CORRECTED by Opus c1 M1):** the original claim
  that the key-set round-trip guard already refuses frontmatter drops was
  FALSE — `check_round_trip` never compares against the existing file's keys.
  The frontmatter-drop gap is real, is OUT of scope here, and is tracked as
  **issue #245**. See "Frontmatter-drop gap" under the fix directions.

## What was NOT checked

- Whether any OTHER MCP write tools (`vault_write_note` siblings) need the same
  guard (e.g. a bulk-import tool) — issue scope is `vault_write_note` only.
- Frontend UX for the new refusal (the desktop app surfaces MCP errors as-is;
  no panel change proposed).
