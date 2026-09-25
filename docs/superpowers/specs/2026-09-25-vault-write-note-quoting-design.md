# vault_write_note frontmatter quoting: fix false "Stale update" lockouts (issue #231)

**Date:** 2026-09-25
**Status:** Draft
**Branch:** fix/issue-231-yaml-quoting
**Priority:** High — recurring agent-facing write path; third observed occurrence of the same class (2026-08-29, 2026-09-02, 2026-09-25)

## Problem

`vault_write_note` If-Match edits fail with a false `Stale update: file was
modified since updated_at=` (empty token) for any note whose rendered
frontmatter is not strict YAML. Issue #231 reported a "title ≥ 66 chars"
boundary; investigation (2026-09-25, converged through 8 Opus review rounds)
proved length is irrelevant. The real defect, at three layers:

1. **Render side.** `render_frontmatter` writes `title` as an unquoted YAML
   plain scalar (`src-tauri/src/okf/mod.rs:166`), wraps each tag in a bare `"`
   with no escaping (`mod.rs:172`), and writes `supersedes` raw (`mod.rs:183`).
   Any title needing quoting — `: ` anywhere, a trailing `:`, leading
   indicators (`- `, `? `, `: `, `*foo`, `[`, `{`, `|`, `>`, `%`, `@`, `'`,
   `"`), raw control characters, embedded newlines — produces a fence the
   strict parser cannot read. Titles with ` #` or leading `&x`/`#foo`/`!foo`
   parse but come back wrong or empty.
2. **Fence side.** On edit, `extract_updated_at` (`write.rs:55-69`) runs the
   strict parse and `.ok()?` turns a parse failure into `None`;
   `enforce_staleness` (`write.rs:79-98`) then reports `StaleUpdate` with an
   empty token. Create succeeds (`write.rs:83-85` skips the check), so the
   lockout only appears on the SECOND edit.
3. **Recovery side.** No MCP tool can read, delete, or force-overwrite a vault
   note (`mcp_server.rs` registers 16 tools, none path-reads notes), so once
   the fence is broken the note is permanently un-editable for MCP clients.
   The strict `serde_yaml` parser touches vault notes ONLY at this fence
   (`read_document` at `lib.rs:3441` returns raw text; the ingest pipeline
   does no frontmatter parsing; the lenient `frontmatter.rs:445` parser is
   bundle-import-only).

Live reproduction against the installed v2.17.0 sidecar (2026-09-25): a
23-char colon title fails; 66- and 80-char colon-free titles edit fine; the
reported 66-char boundary was an artifact of the reporter's test titles.
Full evidence, adversarial-value catalog, and probe transcripts:
investigation doc (review rounds 1-8), `issue231-investigation.md` artifacts.

## Approach

Quote at render time via a write-path-local wrapper (NOT the shared
`needs_quoting`/`quote_string` — bundle export byte parity depends on them),
tolerantly recover the If-Match token from pre-existing broken notes, and
guard every write with a strict round-trip check so no future escaping gap
can land on disk. Rejected alternatives:

- **Serialize the whole block with `serde_yaml`** — emits `okf_version: '0.1'`
  and block-style tags, breaking pinned formats (`mod.rs:388,392`,
  `write.rs:543`).
- **Extend the shared `needs_quoting`** — its comment pins it to the TS
  reference; bundle export parity would break. (Note for a FOLLOW-UP: bundle
  export has the same timestamp-shape quoting flaw through
  `serialize_scalar_string`; needs TS-parity confirmation before touching.)
- **Fix render only** — existing broken notes stay locked forever (mandatory
  back-compat read, review round 2 MAJOR-1).

Design decisions (Kurt-approved 2026-09-25, all four "1"): token in
`WriteNoteResult` (Q1=1); error-contract amendment bundled in this PR (Q2=1);
repair scan included (Q3=1); full escape coverage with a same-release
`core-okf` TS unescape-table extension (Q4=1).

## Design

### 1. Write-path quoting (`okf/mod.rs`, `okf/write.rs`)

- Add `note_needs_quoting(value) -> bool` local to the write path: a copy of
  `frontmatter::needs_quoting` (`frontmatter.rs:69-116`) **with the
  `is_iso8601_timestamp` early `return false` at :76 REMOVED** (the shape-only
  timestamp check lets `2026-09-25T14:00:00Z: deploy retro` slip through
  unquoted with `: ` inside), OR-composed with: (a) any character in the
  escape set (see §2) present — forces quoting even when nothing else
  triggers (`Plan\u{2028}B`); (b) lone leading indicator (`-`, `?`, `:`
  followed by space or end).
- `quote_for_note(value) -> String`: when `note_needs_quoting` fires, wrap in
  `"` and escape with a write-local escaper (superset of
  `frontmatter.rs:118-131`): `\\ \" \n \r \t` plus C0 controls, DEL (U+007F),
  C1 controls (U+0080–U+009F, NEL U+0085 as `\N`), LS/PS (U+2028/U+2029 as
  `\L`/`\P`), U+FFFE/U+FFFF (`\uFFFE`), `\xNN`, `\uXXXX` — the full YAML
  double-quote escape set. Make `quote_string`-class helpers `pub(crate)` as
  needed.
- Application: `title` and `supersedes` go through
  `note_needs_quoting`→`quote_for_note` (conditional — a normal deposit path
  stays unquoted, preserving `write.rs:910/945` pins). **Tags are ALWAYS
  quoted with `quote_for_note` escaping, unconditionally** (flow context:
  `,` `]` are mid-value indicators; clean tags keep their existing quoted
  shape, preserving `mod.rs:392`). `okf_version`, `profile`, `entity_type`,
  `created_at`, `updated_at` stay unquoted (constants/enum/RFC 3339 — safe
  as plain scalars; keeps `okf_version: 0.1` pin).
- Byte impact (pinned by tests): clean notes unchanged EXCEPT titles
  containing `:`/`#` anywhere (`https://example.com`, `C# tips`) and
  reserved/number/timestamp-like titles gain quotes — parse-equivalent, but
  bytes/sha256 change.

### 2. Pre-write round-trip guard (`okf/write.rs`, in `write_note`)

Before `safe_write_bytes`: strip the fences from the rendered document,
strict-parse it, and require (a) the resulting struct equals the effective
frontmatter after normalizing `Some(vec![])` tags → `None` (render drops
empty lists, `mod.rs:168-169`; derived `PartialEq` would otherwise reject
every `"tags": []` write); AND (b) parsing into `serde_yaml::Mapping` yields
exactly the rendered key set (`OkfFrontmatter` has no
`deny_unknown_fields`; the struct compare alone cannot see an injected
`x\nstatus: approved` unknown key). Mismatch → `InvalidFrontmatter`. This
closes every current and future render gap; the `\n` escaping in §1 remains
the primary defense (a pathological same-key split is the stated residual).

### 3. Tolerant If-Match token read, MANDATORY (`okf/write.rs`)

`extract_updated_at` becomes: strict parse FIRST; only on failure, a
tolerant line-scan with all of: same fence collection as `write.rs:57-62`
including the `lines.take(64)` cap (:61); key `updated_at:` at column 0;
exactly one occurrence (duplicates ⇒ unparsable — unambiguous choice, not an
access control); quotes stripped if quoted; value must parse as RFC 3339.
One shared function serves `enforce_staleness` AND `prev_token`
(`write.rs:267`) so the strictly-newer token floor survives healing writes.
This makes every existing colon-titled note editable again on first edit;
the edit then rewrites the fence quoted, permanently healing it.

### 4. Error contract + token in result (`okf/mod.rs`, `write.rs`, `tool_dispatch.rs`, `mcp_server.rs`)

- `enforce_staleness` distinguishes the cases: genuinely stale →
  `StaleUpdate{current}` (unchanged); existing fence unparsable →
  `InvalidFrontmatter("existing_unparsable:parse")`; no fence →
  `…:no_fence`; fence without token → `…:no_token`. Notes with no fence or
  no token stay PERMANENTLY REFUSED over MCP (manual fix required) — the
  conservative If-Match-safe rule.
- `WriteNoteResult` gains `updated_at: String` (the fresh If-Match token) —
  additive; removes the out-of-band file scrape this incident depended on.
- Update the pinned error-shape docs (`write.rs:16-18`), the
  `vault_write_note` MCP tool description (`mcp_server.rs:128`, currently
  promises `stale_update:{current}` vs actual text at `mod.rs:80`), the
  spec-v2 doc, and the client note in the PR body.

### 5. Repair scan (Kurt Q3=1: include)

One-time, REPORT-only (never silently heals): scan `wiki/` and
`immutable-source-files/agents/` for notes whose fence fails the strict
parse, listing path + failure class. Exposed as a Tauri command + log line
on startup after this version's first run. No auto-rewrites.

### 6. `core-okf` frontend parity (Kurt Q4=1: full coverage)

`@equationalapplications/core-okf` (source: `expo-llm-wiki`
`packages/okf`, consumed at 7.7.4) `unescapeFrontmatterString`
(`dist/index.mjs:112-150`) unescapes only `\\ \" \n \r \t`. Extend the TS
unescaper (and its Rust mirror's expectations) to handle the full escape set
emitted by §1 (`\N`, `\L`, `\P`, `\xNN`, `\uXXXX`), with TS display tests
for a quoted colon title and each escape. CT bumps the pinned version in the
same release train. (Quoted colon titles already display correctly through
the current table — the extension is for the new escapes only.)

### 7. Tests (fix §6 of the converged investigation)

- **Second-edit acceptance:** for EVERY adversarial value class — create
  → scrape token → edit with token → success. Legacy-format fixture
  (unquoted colon title) must become editable after the fix.
- Adversarial set (each probe-verified behavior pinned via Rust unit tests
  against the strict parser, fences stripped first — serde_yaml rejects
  multi-document input; cf. `extract_fm` `write.rs:1014-1025`): colon titles
  (`Deploy: retro`, timestamp-prefixed `2026-09-25T14:00:00Z: deploy retro`
  — must round-trip quoted), `*foo` (lockout), bare `&x` (must NOT come back
  `""`), `#foo`/`!foo` (must NOT come back `""`), partly-quoted
  `'Hello' world` / `"Hello" world` (lockout), BOM `Plan\u{FEFF}B` (parses
  fine — pin that no escape is needed), control chars
  (`Plan\u{2028}B` proves force-quoting; `\N`/`\xNN`/`\uFFFE` round-trip),
  injection titles (`x\nsupersedes: …`, `x\ntags: [a]`,
  `x\nstatus: approved` — must NOT bypass validation), tags (`say "hi"`,
  `C:\p`, `a", "b` live; `a,b`, `x]` regression; `Some(vec![])` byte-pin),
  supersedes values.
- Differential: `tolerant(x) == strict(x)` whenever strict parses.
- Byte pins: `mod.rs:376-385` fixture byte-exact; `mod.rs:392` clean tags;
  `write.rs:543`/`910`/`945`; new-quote pins `C# tips`, `https://x`.
- Unit pin: `note_needs_quoting("2026-09-25T14:00:00Z: x") == true`.

## Out of scope

- Bundle-export `serialize_scalar_string` timestamp quoting flaw (same class;
  needs TS-parity confirmation; separate PR).
- Adoption path for hand-written no-fence notes (stay refused over MCP).
- Any change to the shared `needs_quoting`/`quote_string`/`serialize_*`
  helpers.
- **CANCELLED: the `curated-thoughts-integrations` "safe title convention
  (`title` ≤ 65 chars)" docs note** (proposed in the #231 follow-up comment of
  2026-09-25T14:06Z, per external review). Length was proven irrelevant — the
  boundary was an artifact of the reporter's colon-bearing test titles. After
  this fix, quoting makes any title length parse correctly; documenting a
  65-char limit would enshrine a false constraint. The integrations' real
  guidance, if any, is to quote nothing and let the write path handle it —
  which needs no docs change.

## Open questions

None — all four resolved by Kurt 2026-09-25 (Q1–Q4, all "1").

## References

- Issue #231; investigation doc + 8 Opus review rounds (verdict: APPROVE
  WITH NITS, 2026-09-25); live serde_yaml 0.9.34 / libyaml probe transcripts.
- Delivery flow: `tessera-delivery-flow-superpowers.md` (Step 0 = this
  investigation).
