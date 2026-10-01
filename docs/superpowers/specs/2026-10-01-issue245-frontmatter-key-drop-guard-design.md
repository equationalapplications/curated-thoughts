# vault_write_note: frontmatter key-drop guard on If-Match edits (issue #245)

**Date:** 2026-10-01
**Status:** Proposed (spec review loop)
**Branch:** `feat/issue-245-frontmatter-key-drop-guard`
**Priority:** High — silent data loss on the `vault_write_note` edit path (sibling of #240's body-truncation clobber, frontmatter axis).

Investigation (guard signature audit, call-site analysis, failure simulation):
`2026-10-01-issue245-frontmatter-key-drop-guard-investigation.md` (same directory).

## Problem

`check_round_trip` (`src-tauri/src/okf/write.rs:606`) compares rendered-vs-INCOMING only; the
existing file's frontmatter keys are never consulted. An If-Match edit whose payload silently
drops keys (`tags`, `supersedes`, `entity_type`, …) passes all current gates (typed parse, If-Match
staleness, exact key-set round-trip, size-drop — body bytes unchanged) and replaces the note with a
key-reduced version. Same failure class as #240 (mangled agent payload), not caught by #240's guard.

## Approach (all in `src-tauri`; follows the #240 guard pattern exactly)

1. **D1 — existing-side key-set extraction.** New helper
   `existing_frontmatter_keys(content: &str) -> Option<BTreeSet<String>>` next to
   `collect_frontmatter_fence` (write.rs:146): take the SAME fence view the token reader uses
   (64-line cap, exact `---`, CRLF via `lines()`), parse with `serde_yaml`, collect string keys
   into a `BTreeSet`. Pin: returns `None` when the fence view is absent/invalid — the caller then
   SKIPS the guard (a fence-less existing file is already refused downstream by frontmatter
   validation; the guard never invents its own refusal for an unparsable existing file, mirroring
   how `read_existing_token` failures map to `existing_unparsable` upstream).

   Note: `collect_frontmatter_fence` returns only the inner text with no offset and rebuilds via
   `lines()` (drops `\r`) — fine for key-set purposes (keys are `\r`-insensitive after YAML
   parse), so NO new split helper is needed (unlike #240 D6, which needed byte offsets).

2. **D2 — the guard.** In `write_note_core`, after `enforce_staleness` (write.rs:523) and only on
   the EDIT path (`existing.is_some()`):
   `enforce_key_preservation(existing.as_deref(), &frontmatter, allow_key_drop)?`
   - Existing keys = `existing_frontmatter_keys(existing)`.
   - Incoming keys = the key-set implied by the INCOMING struct (`OkfFrontmatter`): `KNOWN_KEYS`
     minus the `Option` fields currently `None`/empty — the same normalization `check_round_trip`
     already performs at :657–665 (extract that normalization into a shared helper so the two
     computations cannot drift).
   - `dropped = existing_keys − incoming_keys`. If `!dropped.is_empty() && !allow_key_drop`:
     refuse.

3. **D3 — scope: ALL keys, not a reserved subset.** Every key present in the existing fence is
   load-bearing (Kurt's vault ontology depends on `tags`/`supersedes`/`entity_type` visible in the
   wiki; `created_at`/`title` loss is equally silent damage). Pin: no per-key carve-outs. The only
   exemption is the token itself: `updated_at` is EXCLUDED from the comparison on both sides (it
   is rotated by the write path on every successful write — the incoming value always differs; a
   non-exempt `updated_at` would refuse every legitimate edit).

4. **D4 — intentional removal requires the same explicit confirm as #240.** New param
   `allow_key_drop: bool` (serde-default false), mirroring `allow_shrink`:
   - MCP: `VaultWriteNoteParams.allow_key_drop` (schemars auto-derives the schema), threaded
     through `tool_dispatch.rs` exactly like `allow_shrink` (:292/:304/:1147/:1525).
   - Tauri: `lib.rs` command takes `allow_key_drop: Option<bool>` + `.unwrap_or(false)` — same
     rationale as #240 D5 (Tauri command params do not honor `#[serde(default)]`).
   - Removal is still possible: retiring a `supersedes` pointer deliberately is a
     `allow_key_drop: true` edit. The refusals never name the bypass flag in their Display (same
     #240 rule: a compacted agent reading the flag in the error bypasses in one retry).

5. **D5 — error variant + pinned Display.**
   `WriteNoteError::KeyDropRefused { keys: Vec<String> }` (sorted, so the message is
   deterministic), Display PINNED to
   `key_drop_refused:{keys}: re-send the complete frontmatter or pass an explicit key-drop confirmation`
   — no flag name in the text. Registered in the `write.rs` module-header refusal-contract list
   (#240 item 7 convention).

6. **D6 — check order.** After `enforce_staleness`, BEFORE token rotation/render. Pin: key-drop
   refusal outranks a same-payload shrink refusal (the frontmatter damage is the more specific
   diagnosis; both are refusals of the same payload, and order only matters for which message the
   agent sees first). The existing order marker/shrink (post-render, pre-write) is untouched.

## Error handling

One new `WriteNoteError` variant (D5), following the module's Display-string contract. MCP callers
see the Display via `anyhow!("{}", e)` (`tool_dispatch.rs:304`).

## Testing

(`cargo test -p curated-thoughts okf::write`; existing #240 test suite as the pattern.)

- Incident replay: existing note with `tags` + `supersedes`; If-Match edit payload omitting both →
  refused, Display `starts_with("key_drop_refused:")`, names both keys sorted, no flag name in text.
- `allow_key_drop: true` permits the same edit; omitted/false behaves identically to today.
- Boundary: ADDING keys never refuses; dropping zero keys never refuses; drop of exactly one key
  names exactly that key.
- `updated_at` exemption: every legitimate edit (token rotated) passes — regression-proofed by the
  whole existing suite passing unchanged.
- CRLF note: keys extracted correctly through `lines()` fence view (drop-`tags` edit on a CRLF
  fixture refused).
- Fence-less existing file: guard skipped, downstream validation still refuses on its own terms;
  no panic.
- Unknown-key case: existing file with a key outside `KNOWN_KEYS` (hand-edited note) — dropped
  unknown keys ARE refused (the key exists in the file; the incoming typed struct cannot
  re-express it, so losing it silently is exactly the hazard). Pin: comparison runs on the RAW
  existing key-set vs incoming known-key-set, not two `KNOWN_KEYS` projections.
- Audit existing tests for edits that legally drop keys (e.g. fixtures writing a note then
  re-writing with fewer keys) — route intentional ones through `allow_key_drop: true`.
- MCP param plumbing: `allow_key_drop` omitted ⇒ false; true parses (mirror
  `params_allow_shrink_omitted_defaults_false_and_true_parses`).

## Out of scope / open questions

- **Value-axis mutations are NOT covered:** the guard compares KEY SETS only. A key that is
  present but emptied (e.g. an edit sending `created_at: ""` or a blanked `title`) is a value-axis
  hazard with its own failure signature — deliberately not smuggled into this design.
- Other write tools (okf deposit tools, librarian) — issue scope is `vault_write_note` only.
- Frontend UX for the refusal — MCP errors surface as-is; no panel change.
- Restore-from-`allow_key_drop:true` accidents — same stance as `allow_shrink`: git history is the
  recovery path; the flag is deliberate, logged in the caller's payload.

Fixes #245.
