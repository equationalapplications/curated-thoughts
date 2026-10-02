# vault_write_note: frontmatter key-drop guard on If-Match edits (issue #245)

**Date:** 2026-10-01
**Status:** Implemented on `feat/issue-245-frontmatter-key-drop-guard` — plan `docs/superpowers/plans/2026-10-02-issue245-frontmatter-key-drop-guard.md`. Review converged before implementation (Opus design-c3 APPROVE WITH NITS; CodeRabbit, Claude review and `/code-review high` 2026-10-02 applied). See [Revision history](#revision-history).
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

**`KNOWN_KEYS` is hoisted (review 2026-10-02).** Today it is a function-local `const` inside
`check_round_trip` (`write.rs:644`). This change moves it to module scope in `write.rs` —
still pre-sorted ascending, keeping its "never reshuffle without keeping it sorted" comment —
so `check_round_trip`, the new incoming-key normalization helper (D2) and
`enforce_key_preservation` all read the ONE list. A second copy of the list is forbidden: it
is exactly the drift D2's shared helper exists to prevent.

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
     parses but is not a mapping → fall through to the line scan. Non-string keys (`1: x`) count
     as PRESENT and land in the unrepresentable partition (Debug form in the refusal) —
     `check_round_trip` already refuses non-string keys in the rendered output (`write.rs:621`/
     `:634`), so silently dropping them on the
     existing side would leave that one key class unguarded (Opus design-c3 N3). Implementation
     pin (review 2026-10-02): the helper must NOT copy `check_round_trip`'s key loop
     (`write.rs:631-637`), which returns `Err` on a non-string key — on the EXISTING side a
     non-string key is data to protect, not injection to reject, so it is collected (Debug form)
     and the function never errors on key type.
   - strict parse fails BUT the fence exists (damaged YAML that nevertheless passed staleness via
     `read_existing_token`'s tolerant line-scan fallback, write.rs:95-129) → collect keys with a
     column-0 `^key:` line scan over the SAME fence buffer, mirroring the token reader's fallback
     philosophy: damaged notes stay guarded, not skipped. Pins: (a) this is an approximation (a
     column-0 line inside a malformed construct could over-count); acceptable because false
     "present" keys only make the guard stricter, and the alternative — skipping the guard on
     exactly the notes most likely to be hand-edited — is the #245 hazard itself; (b) the line
     scan applies the SAME null/empty normalization as tier 2 (Opus design-c2 finding 2,
     wording per design-c3 N1), INCLUDING its D2 restriction to the KNOWN optional fields
     (CodeRabbit follow-up 2026-10-02): only a `tags:` or `supersedes:` line can count as
     ABSENT. Every other key — unknown/legacy keys above all — counts as PRESENT whenever its
     column-0 `^key:` line exists, whatever its value (so a damaged-YAML note's `aliases: []`
     still refuses as `KeyDropUnrepresentable`). For `tags`/`supersedes`, the line counts as
     ABSENT only when its inline value is one of the D2 absent forms — nothing after the colon,
     `null`, `~`, `[]` (`tags`), or `""` (`supersedes`) — AND the next line in the fence is not
     a continuation (any indented line, or a `- ` line). Any non-empty inline value (`tags: foo`)
     counts as PRESENT. The continuation guard is what keeps a block-sequence `tags:` list
     PRESENT; applying the empty-value check without it would silently drop block-valued keys —
     the #245 hazard again. (c) Trailing comments are NOT stripped: the inline value is
     compared verbatim (after trimming whitespace), so `tags: [] # none` counts PRESENT in
     tier 3 even though the strict tier reads it as empty/absent. The tiers disagree only in
     the stricter direction (a false "present" refuses, never silently drops), and the
     comment-stripping alternative would need a YAML-aware scanner to avoid cutting `#` inside
     quoted values — accepted, not fixed (review 2026-10-02). (d) Key-extraction rule, pinned
     (review 2026-10-02 — a placeholder `^key:` invites an identifier-class regex that would
     miss `1: x`, the exact class D1 tier 2 promises to guard): a fence line yields a key iff
     it is non-empty, starts at column 0 with a character other than whitespace, `#` or `-`,
     and contains `:`. The key is the text before the FIRST `:`, trimmed, with one matching
     pair of surrounding `"`/`'` quotes stripped. NO character-class restriction — `1`,
     `my-key`, `some key` are all keys. Over-matching (e.g. a column-0 `:` inside a damaged
     construct) errs in the stricter direction, same as (a).

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
     drift). Existing-side normalization mirror (Opus c1 m3), RESTRICTED per CodeRabbit PR-
     comment finding 2026-10-02 to KNOWN optional fields only. The exhaustive ABSENT list:
     `tags` null / `~` / `[]` (deserializes to `None` or `Some(vec![])`, both of which the
     renderer omits — `check_round_trip` :661); `supersedes` null / `~` (deserializes to
     `None`, omitted via `skip_serializing_if`). `supersedes: ""` also counts ABSENT, but NOT
     because the renderer omits it (it would render `Some("")`): an empty pointer can never be
     re-sent — `under_deposit("")` refuses it upstream (`write.rs:447`) — so counting it
     present would make the note's next edit refuse with `KeyDropRefused` whose advice
     ("re-send the complete frontmatter") cannot be followed: the ONLY way through would be a
     forced `allow_key_drop: true` edit for a value that carries no information. (Not a
     permanent wedge — the flagged edit removes the key — but a refusal with unfollowable
     advice on a meaningless value; review 2026-10-02 corrected the earlier "wedge every edit"
     overstatement.) **Any other value form counts PRESENT**
     — including non-list `tags` (`tags: ""`, `tags: foo`, `tags: {}`) and non-empty
     `supersedes`. Such a hand-edited note still passes staleness (the typed parse fails, the
     token reader's line-scan fallback succeeds), and it does not wedge: the caller keeps the
     key by sending non-empty `tags`, or drops it with the explicit confirmation (review
     2026-10-02). Unknown keys
     (non-`KNOWN_KEYS`) count as PRESENT regardless of value, so a hand-written `aliases: []`
     cannot be silently stripped on a subsequent edit — it must surface as
     `KeyDropUnrepresentable` rather than vanish. A hand-written `tags: []` or
     `supersedes: null` still does not wedge every later edit into a false drop.
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
     locals, never literals — a swap then reads wrong in review. (Implementation: default-
     valued sites pass module-level named consts `NO_SHRINK`/`NO_KEY_DROP` instead of two
     `let` lines at each of ~80 sites — same swap-visibility; `true` sites bind locals.)
     This covers BOTH the unit
     tests in `okf/write.rs` AND the integration binary `src-tauri/tests/mcp_write_integration.rs`
     (direct `write_note(..., None, false)` calls at :836, :868, :883, :907 today — every one
     breaks on the signature change; review 2026-10-02).
   - MCP tool description (`src-tauri/src/mcp_server.rs:127`, review 2026-10-02): the
     `vault_write_note` description enumerates every refusal and its remedy
     (`existing_unparsable`, `shrink_refused`, `compaction_marker`) — it is how agents learn
     the contract. Extend it with both new refusals: `key_drop_refused:{keys}` means the
     payload omitted frontmatter keys the note has — re-read the note and resend the complete
     frontmatter; `key_drop_refused:unrepresentable:{keys}` means the note carries keys this
     tool cannot write — the note must be migrated outside the tool. Same rule as the
     Displays: the description NEVER names `allow_key_drop` (the schemars param schema is
     the only place the flag appears).
   - Removal is still possible: retiring a `supersedes` pointer deliberately is an
     `allow_key_drop: true` edit. Edge case (Opus c1 m5): re-sending `supersedes` to KEEP it
     triggers the `supersedes_not_found` check when the target deposit has since been removed —
     in that situation the only edit path is `allow_key_drop: true` (dropping the stale pointer),
     which is the intended outcome. Sibling case (review 2026-10-02): a NON-deposit note
     (`records/`, `wiki/`) carrying a hand-added `supersedes:` can never re-send it either —
     `write_note` refuses `supersedes` on a non-deposit path ("supersedes is deposit-only")
     before the guard runs. Its `KeyDropRefused` advice ("re-send the complete frontmatter")
     therefore cannot succeed; as with m5, the only edit path is `allow_key_drop: true`.
     Accepted as documented (rare: hand-edited, outside the deposit flow) rather than
     re-partitioning `supersedes` by path.
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

6. **D6 — check order.** Pinned order for every stage from staleness onward (Opus design-c2
   nit 2 / design-c3 N2 — the pre-staleness stages — struct validation, root allow-list,
   supersedes check, path resolution — and the create-only `created_at` check all run earlier or
   don't interact):
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

Verification must match CI's real gates (review 2026-10-02): `cargo test -p curated-thoughts
--features test-utils,mcp-server` — the full lib suite AND the integration binaries
(`tests/mcp_write_integration.rs` calls `write_note` directly; `okf::write` alone neither
builds nor runs it). Iterate with `cargo test -p curated-thoughts --features
test-utils,mcp-server okf::write`, but the full run is the gate. Existing #240 test suite as
the pattern.

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
- Nested block mapping in tier 3 (Opus design-c3 N1): damaged-YAML note with `source:` followed
  by an indented `url:` line → any edit dropping `source` refuses with `KeyDropUnrepresentable`
  (unknown keys always count PRESENT in tier 3 — the continuation guard is not what saves it).
- Unknown empty-valued key, strict tier (CodeRabbit 2026-10-02): well-formed note with
  `aliases: []` → any edit refuses with `KeyDropUnrepresentable` naming `aliases` (the D2
  normalization must not apply to unknown keys).
- Unknown empty-valued key, line-scan tier (CodeRabbit follow-up 2026-10-02): same fixture with
  damaged YAML (strict parse fails, staleness passes) → still refuses with
  `KeyDropUnrepresentable` naming `aliases`.
- Known-field absent forms: existing `tags: null`, `tags: ~`, `supersedes: ~`, and
  `supersedes: ""` each count absent — an edit omitting the key succeeds. Tier-3 non-empty
  scalar: damaged-YAML `tags: foo` counts PRESENT — an edit omitting `tags` refuses.
- Non-list `tags` (review 2026-10-02): damaged-YAML note with `tags: ""` → edit omitting
  `tags` refuses with `KeyDropRefused`; the same edit sending non-empty `tags` succeeds.
- Tier-3 trailing comment (review 2026-10-02): damaged-YAML `tags: [] # none` counts PRESENT
  — an edit omitting `tags` refuses (pins the accepted stricter-direction divergence).
- Non-string keys, strict tier (review 2026-10-02): well-formed note with `1: x` → any edit
  refuses with `KeyDropUnrepresentable` (helper returns the key set, does NOT error — pins the
  "don't copy `check_round_trip`'s reject loop" rule in D1).
- Non-string / non-identifier keys, line-scan tier (review 2026-10-02): damaged-YAML note with
  `1: x` and `my-key: y` → any edit refuses with `KeyDropUnrepresentable` naming both (pins the
  D1 (d) extraction rule against an identifier-class regex). Plus a unit test on the tier-3
  extractor: `"quoted": v` yields `quoted`; `# comment`, `- item`, indented lines and blank
  lines yield nothing.
- MCP description (review 2026-10-02): assert the `vault_write_note` tool description
  mentions `key_drop_refused` and does NOT contain `allow_key_drop` — via the server's
  registered tool list if it is reachable from a test, otherwise a source-text assertion on
  `mcp_server.rs` (`include_str!`).
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

## Revision history

- Opus design-c1 — REQUEST CHANGES (M1–M3, m1–m6, all real) — applied.
- Opus design-c2 — REQUEST CHANGES (MAJOR 1 + 2 minors + 4 nits) — applied.
- Opus design-c3 — APPROVE WITH NITS (N1–N3) — applied.
- GLM self-review — applied.
- CodeRabbit PR comments 2026-10-02 — M1 existing-side null/empty normalization restricted
  to KNOWN optional fields; M2 size-guard scope is body-only. Follow-up: tier-3 line scan
  inherits the known-field restriction, "bare scalar" wording fixed, full absent-form list
  pinned — applied.
- Claude review 2026-10-02 — MCP tool description update (D4); integration-test call sites +
  CI-feature test command (D4/Testing); `KNOWN_KEYS` hoisted to module scope (Problem);
  non-string-key reference fixed to `check_round_trip` :621/:634 (D1); non-deposit
  `supersedes` documented (D4); tier-3 trailing-comment divergence pinned (D1); non-list
  `tags` value forms count PRESENT (D2) — applied.
- `/code-review high` 2026-10-02 — tier-3 key-extraction rule pinned with no character-class
  restriction (D1 (d)); existing-side helper must not copy `check_round_trip`'s non-string-key
  reject (D1); `supersedes: ""` rationale corrected from "wedge every edit" to
  "unfollowable refusal advice" (D2); non-string-key tests for both tiers (Testing) — applied.

Fixes #245.
