# vault_write_note: frontmatter key-drop guard on If-Match edits (issue #245)

**Date:** 2026-10-01
**Status:** Proposed — review cycle 2 adjudicated: Opus design-c1 REQUEST CHANGES (M1–M3, m1–m6 all REAL, applied); Opus design-c2 REQUEST CHANGES (MAJOR 1 + findings 2–3 + nits 1–4, all applied). Cycle 3 (delta) = final for a doc per the hard cap.
**Branch:** `feat/issue-245-frontmatter-key-drop-guard`
**Priority:** High — silent data loss on the `vault_write_note` edit path (sibling of #240's body-truncation clobber, frontmatter axis).

Investigation (guard signature audit, call-site analysis, failure simulation):
`2026-10-01-issue245-frontmatter-key-drop-guard-investigation.md` (same directory).

## Problem

`check_round_trip` (`src-tauri/src/okf/write.rs:606`) compares rendered-vs-INCOMING only; the
existing file's frontmatter keys are never consulted. An If-Match edit whose payload silently
drops keys passes all current gates (typed parse, If-Match staleness, exact key-set round-trip,
size-drop — body bytes unchanged) and replaces the note with a key-reduced version. Same failure
class as #240 (mangled agent payload), not caught by #240's guard.

**What can actually be dropped (Opus c1 M1):** `OkfFrontmatter` (`okf/mod.rs:35-47`) declares
`okf_version`, `profile`, `title`, `entity_type`, `created_at` as REQUIRED — a payload omitting
one fails struct parse before `write_note` ever runs. The droppable keys are exactly:
- `tags` and `supersedes` (the `Option` fields), and
- any key outside `KNOWN_KEYS` (hand-added/legacy keys the typed struct cannot re-express —
  silently stripped today).

## Approach (all in `src-tauri`; follows the #240 guard pattern exactly)

1. **D1 — existing-side key-set extraction.** New helper
   `existing_frontmatter_keys(content: &str) -> Option<BTreeSet<String>>` next to
   `collect_frontmatter_fence` (write.rs:160): take the SAME fence view the token reader uses
   (64-line cap, exact `---`, CRLF via `lines()`), parse with `serde_yaml`, collect string keys
   into a `BTreeSet`. Three-tier behavior (Opus c1 M3):
   - fence absent → `None` (guard skipped). Unreachable in practice: a fence-less existing file
     is refused UPSTREAM by `enforce_staleness` (`existing_unparsable:no_fence` via
     `read_existing_token`), so the guard never sees one — the earlier draft's "downstream by
     frontmatter validation" was wrong.
   - strict `serde_yaml` parse succeeds → full key set. Pin (Opus design-c2 finding 3): the strict
     parse targets `serde_yaml::Value` and reads its MAPPING KEYS — NOT `parse_frontmatter` into
     `OkfFrontmatter` (the token reader's strict parse), which silently drops unknown keys and
     would make `KeyDropUnrepresentable` unable to fire for well-formed legacy notes. If the YAML
     parses but is not a mapping → fall through to the line scan.
   - strict parse fails BUT the fence exists (damaged YAML that nevertheless passed staleness via
     `read_existing_token`'s tolerant line-scan fallback, write.rs:95-129) → collect keys with a
     column-0 `^key:` line scan over the SAME fence buffer, mirroring the token reader's fallback
     philosophy: damaged notes stay guarded, not skipped. Pins: (a) this is an approximation (a
     column-0 line inside a malformed construct could over-count); acceptable because false
     "present" keys only make the guard stricter, and the alternative — skipping the guard on
     exactly the notes most likely to be hand-edited — is the #245 hazard itself; (b) the line
     scan applies the SAME null/empty normalization as tier 2 (Opus design-c2 finding 2): an
     inline value of `[]`, `null`, `~`, or nothing-before-EOL counts as ABSENT; block-sequence
     values (next-line `- …`) count as present — a damaged-YAML `tags: []` must not wedge every
     later edit into a false drop that teaches the bypass.

   Note: `collect_frontmatter_fence` returns only the inner text with no offset and rebuilds via
   `lines()` (drops `\r`) — fine for key-set purposes (keys are `\r`-insensitive after YAML
   parse), so NO new split helper is needed (unlike #240 D6, which needed byte offsets).

2. **D2 — the guard.** In `write_note` (`src-tauri/src/okf/write.rs:416`; the earlier draft said
   `write_note_core` — Opus c1 m1), AFTER `enforce_compaction_markers` and BEFORE
   `enforce_size_drop` (order pin in D6), only on the EDIT path (`existing.is_some()`):
   `enforce_key_preservation(existing.as_deref(), &frontmatter, allow_key_drop)?`
   - Existing keys = `existing_frontmatter_keys(existing)`.
   - Incoming keys = the key-set implied by the INCOMING struct: `KNOWN_KEYS` minus the `Option`
     fields currently `None`/empty — the same normalization `check_round_trip` already performs
     at :657–665 (extract that normalization into a shared helper so the two computations cannot
     drift). Existing-side normalization mirror (Opus c1 m3): an existing key whose value is
     null or an empty sequence counts as ABSENT (matching how the renderer omits
     empty-`tags`/`None` fields) — a hand-written `tags: []` or `supersedes: null` must not
     wedge every later edit into a false drop.
   - Partition `dropped = existing_keys − incoming_keys` into KNOWN dropped keys and
     UNKNOWN (non-`KNOWN_KEYS`) dropped keys. Refuse if `!dropped.is_empty() && !allow_key_drop`.

3. **D3 — scope: ALL comparable keys, no carve-outs (pin protects the future).** Given M1, the
   only keys an incoming payload can drop today are `tags`, `supersedes`, and unknown keys — but
   the guard compares the FULL raw existing key-set precisely so that any `Option` field added to
   `OkfFrontmatter` later is automatically covered. `updated_at` is EXEMPT from the comparison on
   both sides (rotated by the write path on every successful write — the incoming value always
   differs; a non-exempt `updated_at` would refuse every legitimate edit).

4. **D4 — intentional removal requires the same explicit confirm as #240.** New param
   `allow_key_drop: bool` (serde-default false), mirroring `allow_shrink`:
   - MCP: `VaultWriteNoteParams.allow_key_drop` (schemars auto-derives the schema), threaded
     through `tool_dispatch.rs` exactly like `allow_shrink` (:292/:304/:1147/:1525).
   - Tauri: `lib.rs` command takes `allow_key_drop: Option<bool>` + `.unwrap_or(false)` — same
     rationale as #240 D5 (Tauri command params do not honor `#[serde(default)]`).
   - Signature note (Opus c1 m6): `write_note` will take two adjacent bare `bool`s
     (`allow_shrink`, `allow_key_drop`) — easy to swap silently. Keep bare bools (a params struct
     would rewrite every #240-era call site — scope creep), but pin: ALL test call sites bind
     named locals first (`let allow_shrink = false; let allow_key_drop = true;`) and pass the
     locals, never literals — a swap then reads wrong in review.
   - Removal is still possible: retiring a `supersedes` pointer deliberately is an
     `allow_key_drop: true` edit. Edge case (Opus c1 m5): re-sending `supersedes` to KEEP it
     triggers the `supersedes_not_found` check when the target deposit has since been removed —
     in that situation the only edit path is `allow_key_drop: true` (dropping the stale pointer),
     which is the intended outcome.
   - The refusals never name the bypass flag in their Display (same #240 rule: a compacted agent
     reading the flag in the error bypasses in one retry).

5. **D5 — error variants + pinned Displays (Opus c1 M2).** TWO variants, so "you dropped it" is
   distinguishable from "the tool cannot keep it":
   - `KeyDropRefused { keys: Vec<String> }` for KNOWN dropped keys, Display PINNED to
     `key_drop_refused:{keys}: re-send the complete frontmatter or pass an explicit key-drop confirmation`.
   - `KeyDropUnrepresentable { keys: Vec<String> }` for UNKNOWN dropped keys (sorted, both
     deterministic), Display PINNED to
     `key_drop_refused:unrepresentable:{keys}: this note carries keys the writer cannot re-emit; migrate the note to the OKF schema outside this tool, or pass an explicit key-drop confirmation`
     — because `check_round_trip` refuses any rendered key outside `KNOWN_KEYS` (:643-675), an
     edit of such a note can NEVER be fixed by "re-sending the complete frontmatter"; the advice
     must name the real way out (migration happens outside `vault_write_note` — Opus design-c2
     nit 4).
   - **Precedence pin (Opus design-c2 MAJOR 1):** when BOTH partitions are non-empty (a legacy
     note with `type:` + `tags`, mangled payload drops `tags`), `KeyDropRefused` (KNOWN) is
     reported FIRST — the agent is then directed to restore its known keys before it ever sees
     the unrepresentable refusal, and by the time the flag is justified only the genuinely
     unrepresentable keys remain dropped. State plainly: **`allow_key_drop: true` bypasses BOTH
     partitions**, so this precedence is the only protection keeping the known keys from being
     silently lost behind an unrepresentable-key message.
   - **Measured known limitation (vault scan 2026-10-01; methodology Opus design-c2 nit 3):**
     fence-regex scan of all 477 live `.md` notes (`.git`/`.brain` excluded), column-0 key lines
     inside frontmatter fences diffed against `KNOWN_KEYS`: 17 notes carry non-schema keys
     (mostly legacy `type:`, `okf_status:`, `id:`, `confidence:`, `source:`, `category:` —
     concentrated in `records/` handoffs and `immutable-source-files/agents/memories/`). All 17
     were cross-checked writable (`NOTE_WRITABLE_SUBDIRS` = `wiki`, `immutable-source-files/
     agents` deposit, `records`), so the affected count is exact, not overstated. Today those
     keys are stripped SILENTLY by any edit; after this guard every edit of those 17 notes
     refuses until the note is migrated outside the tool or the caller confirms the drop. This
     is the intended direction (loud over silent), and the count is the migration backlog.
   - Registered in the `write.rs` module-header refusal-contract list (#240 item 7 convention).

6. **D6 — check order.** Full pinned order (Opus design-c2 nit 2 — every stage listed):
   `enforce_staleness` → token rotation → `render_document` → `check_round_trip` →
   `enforce_compaction_markers` → `enforce_key_preservation` → `enforce_size_drop` →
   `safe_write_bytes`. Pin: the MARKER check outranks key-drop (a compaction artifact is the root
   cause and its check has no bypass — Opus c1 m4); key-drop outranks shrink (the frontmatter
   damage is the more specific diagnosis), and KNOWN keys outrank UNKNOWN within the key-drop
   refusal itself (D5 precedence pin). The key-drop check needs no render products, so placing it
   after render costs nothing and keeps the ordering statement single.

## Error handling

Two new `WriteNoteError` variants (D5), following the module's Display-string contract. MCP
callers see the Display via `anyhow!("{}", e)` (`tool_dispatch.rs:304`).

## Testing

(`cargo test -p curated-thoughts okf::write`; existing #240 test suite as the pattern.)

- Incident replay: existing note with `tags` + `supersedes`; If-Match edit payload omitting both →
  refused with `KeyDropRefused`, Display `starts_with("key_drop_refused:")`, names both keys
  sorted, no flag name in either Display's text.
- Required-field non-case (Opus c1 M1): a payload omitting `entity_type` (or `title`/
  `created_at`) never reaches the guard — it fails struct parse; test asserts the PARSE error
  (guards implementers from writing tests that "fail at parsing and look like guard bugs").
- UNKNOWN-key drop: existing note with a hand-added `aliases:` key; any edit → refused with
  `KeyDropUnrepresentable`, Display contains `unrepresentable` and names `aliases`.
- MIXED drop (Opus design-c2 MAJOR 1): legacy note with `type:` + `tags`; mangled edit drops
  `tags` → refuses with `KeyDropRefused` naming ONLY `tags` (KNOWN partition reported first);
  `type:` must not appear in the refusal's key list.
- Damaged-YAML null/empty normalization (Opus design-c2 finding 2): fence present, strict parse
  fails, existing contains `tags: []` (inline empty) → edit omitting `tags` succeeds (line-scan
  tier counts it absent); same fixture with a block-sequence `tags:` list → edit refuses.
- `allow_key_drop: true` permits both refusal cases; omitted/false behaves identically to today.
- Boundary: ADDING keys never refuses; dropping zero keys never refuses; drop of exactly one key
  names exactly that key.
- Existing-side normalization (Opus c1 m3): existing `tags: []` / `supersedes: null` count as
  absent — an edit re-writing the note without them succeeds (no false drop).
- `updated_at` exemption: every legitimate edit (token rotated) passes — regression-proofed by the
  whole existing suite passing unchanged.
- CRLF note: keys extracted correctly through `lines()` fence view (drop-`tags` edit on a CRLF
  fixture refused).
- Damaged-YAML note (Opus c1 M3): fence present, strict parse fails, token fallback passes
  staleness → column-0 line-scan key-set still catches a drop-`tags` edit (guard NOT skipped).
- Fence-less existing file: unreachable post-staleness (`existing_unparsable:no_fence` upstream);
  test documents the skip branch only as defense-in-depth (no panic if hit).
- Audit existing tests for edits that legally drop keys (e.g. fixtures writing a note then
  re-writing with fewer keys) — route intentional ones through `allow_key_drop: true`, bound to
  named locals per D4.
- MCP param plumbing: `allow_key_drop` omitted ⇒ false; true parses (mirror
  `params_allow_shrink_omitted_defaults_false_and_true_parses` at write.rs:2681).

## Out of scope / open questions

- Value-axis mutations are NOT covered: the guard compares KEY SETS only. A key that is
  present but carries a wrong/emptied VALUE (e.g. `tags` replaced by an unrelated set, or
  `entity_type` changed) passes the guard — `validate_frontmatter` catches only structural
  invalidity (empty `created_at`/blank `title` are refused there, `okf/mod.rs:156-162`), not
  value swaps. Value-axis protection is its own failure signature — deliberately not smuggled
  into this design. (Correction from the earlier draft: the original "`created_at: ""`" example
  mislabeled the refuser as struct parse — it is `validate_frontmatter`. Opus c1 m2 / design-c2
  nit 1.)
- Other write tools (okf deposit tools, librarian) — issue scope is `vault_write_note` only.
- Frontend UX for the refusal — MCP errors surface as-is; no panel change.
- Migration of the 17 measured legacy-key notes (D5) — separate housekeeping pass, not this PR.
- Restore-from-`allow_key_drop:true` accidents — same stance as `allow_shrink`: git history is the
  recovery path; the flag is deliberate, logged in the caller's payload.

Fixes #245.
