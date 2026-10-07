# Wisdom-gate instruction prefix + scheme cutover (issue #265 follow-up) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **Rust tasks run SERIAL — never parallel cargo waves (flow rule).**

**Goal:** Ship both-side Qwen3 instruction conditioning for the wisdom gate and
`wiki_search`/`wiki_context` (cell E, floor 0.64) behind a scheme cutover that makes
mixed-scheme scoring impossible by construction.

**Architecture:** A WRITE scheme (deploy-time constant: doc prefix, row stamp, parity
text) and a READ scheme tuple (meta-driven: floor, query prefixes, both SELECT
filters) replace the raw-only embedding scheme. `llm_wiki_entries` gains
`embed_scheme TEXT NOT NULL DEFAULT 'raw'`; `meta` key `wisdom_active_scheme`
(`raw` | `instr1`) drives the read side; the sweep gains a scheme-filtered re-embed
mode; an idempotent admin command cuts over after the raw count reaches zero.

**Tech Stack:** Rust (stable), rusqlite, serde/serde_json, clap 4 derive, flate2,
sha2 (existing workspace deps). Tests: `cargo test` (non-gated; the new scheme tests
MUST NOT be feature-gated), `--features slow-tests` only for the existing bench.

**Spec:** [`docs/superpowers/specs/2026-10-07-issue265-wisdom-instruction-prefix-design.md`](../specs/2026-10-07-issue265-wisdom-instruction-prefix-design.md)
(rev 2, CONVERGED) + [`...-investigation.md`](../specs/2026-10-07-issue265-wisdom-instruction-prefix-investigation.md).
The plan argues from the spec; executors read both. Global constraints inherit the
#265 plan (read-only gate, exit codes, `Refs #26x` footers, fmt+clippy before every
commit, never touch the live brain).

## Global Constraints (this PR)

- The prefix string is BYTE-EXACT everywhere:
  `"Instruct: Given a web search query, retrieve relevant passages that answer the query\nQuery:"`
  (no space after `Query:`, direct concatenation with the text). One constant in one
  scheme module; test (a) hard-codes it independently.
- Floor key `external:qwen/qwen3-embedding-4b:instr1` → `0.64` in
  `WISDOM_GATE_FLOORS`; the raw key stays. Unknown read scheme or unregistered floor
  → hard error, fail-closed (never fall back to another scheme's floor).
- `embed_scheme TEXT NOT NULL DEFAULT 'raw'` with V+1 backfill — the NULL class must
  never exist; all filters two-valued SQL.
- Rust tasks SERIAL. `cargo fmt --all` + `cargo clippy --workspace --all-targets
  -- -D warnings` before every commit. Conventional commits, `Refs #265`,
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Never touch the live brain; scratch brains only. Real embedder only (no
  `CURATED_EMBED_STUB` in calibration paths).

## File Structure

| File | Responsibility |
|---|---|
| `src-tauri/src/embed_scheme.rs` (new) | prefix constant, WRITE scheme const, read-scheme resolution from meta (fail-closed), floor-key derivation |
| `src-tauri/src/wisdom_match.rs` (modify) | new floor key + gate SELECT filter from read scheme |
| `src-tauri/src/embed_sweep.rs` (modify) | scheme-filtered sweep mode; stamp on write |
| `src-tauri/src/db/commit.rs` (modify) | parity uses WRITE-scheme text function; stamp on write |
| `src-tauri/src/wiki_graph.rs` (modify) | scheme filter in scoring query + query prefix |
| `tools/src/queries.rs` (modify) | gate query prefix from READ scheme |
| `tools/src/bin/ct.rs` (modify) | `ct wisdom scheme` (status/activate) |
| `src-tauri/migrations/` (new V+1) | `embed_scheme` column + backfill + meta key |
| `tools/src/bin/calibrate_wisdom_gate.rs` (modify) | stamp snapshot with scheme; already has prefix flags |
| `src-tauri/tests/wisdom_gate_bench.rs` (modify) | replay both schemes |
| `src-tauri/tests/scheme_tests.rs` (new, non-gated) | tests (a)–(e) from the spec |

## Tasks (SERIAL)

- [x] **Task 1 — Migration + scheme module + floor keys.** (817ce8a) V+1: add
  `embed_scheme TEXT NOT NULL DEFAULT 'raw'` (SQLite ADD COLUMN with NOT NULL
  DEFAULT backfills existing rows — the migration file must still contain the
  explicit backfill UPDATE the spec pinned, not rely on engine semantics), set meta
  `wisdom_active_scheme='raw'`. New `embed_scheme.rs`: the byte-exact prefix
  constant, `WRITE_SCHEME` const (`instr1`), `read_scheme(conn)` resolving from meta
  with hard-error on unknown, `floor_key_for(model, scheme)`. **Add the
  `external:qwen/qwen3-embedding-4b:instr1 → 0.64` entry to `WISDOM_GATE_FLOORS`
  HERE (plan-review F1: test (d) in Task 3 needs it; the raw key stays).** Unit
  tests for resolution + fail-closed paths + both floor keys.
- [x] **Task 2 — Write path.** (526ff4d) `embed_text_for_entry` applies the doc prefix per the
  WRITE scheme; all blob-writing paths stamp `embed_scheme` from the WRITE constant
  in the same statement; parity
  (`commit.rs:1534-1538, 1721-1725`) compares against the same WRITE-scheme text
  function. Test (c) lands here, plus the writer-inventory test (F4): grep-test
  asserting every UPDATE/INSERT site touching `embedding_blob` also writes
  `embed_scheme` from the WRITE constant. **Writer checklist (from the caller
  trace, scratch/step0-callers.md): embed_sweep.rs (sweep), db/commit.rs:1534/1721,
  db/wisdom.rs:66-79, entities_api.rs:83/104, schema_guard.rs:34 — all five plus
  the graph_reanchor migration bin's write path must stamp.** **GUARDRAIL (F5):
  committed fixtures and
  snapshots are NOT touched in this task — regeneration is Task 5's alone; if a
  raw-text fixture test goes red here, the sanctioned move is to mark it
  scheme-dependent and fix it in Task 5, never to regenerate or hand-edit fixtures.**
- [ ] **Task 3 — Read path.** Gate: SELECT filter + query prefix at
  `queries.rs:724` + floor via `floor_key_for`, all from the READ scheme.
  `wiki_graph`: same filter + query prefix at its scoring query. Test (b)
  (dual-stamp DB, both readers, both schemes) and test (d) (coupling: raw→
  0.70/raw rows/unprefixed; instr1→0.64/instr1 rows/prefixed; unknown→hard error)
  land here. `gate_model_key` gains the scheme suffix per spec.
  **GUARDRAIL (F5): tests (b)/(d) are STRUCTURAL — synthetic vectors in a scratch
  DB; assert which rows are candidates and which floor/filter/prefix mode was
  selected. No network, no real embedder (that would require feature-gating, which
  is forbidden for these tests).**
- [ ] **Task 4 — Sweep + admin command.** Scheme-filtered sweep mode: re-embed rows
  WHERE `embedding_blob IS NOT NULL AND embed_scheme != 'instr1'`, stamping as it
  goes (idempotent resume; stamping re-affirmed per F1c). `ct wisdom scheme`
  subcommand: `status` (counts per scheme, active read scheme) and
  `activate instr1` (refuses unless raw non-null count = 0, prints count otherwise;
  atomic meta flip; idempotent). Integration tests over a scratch DB.
- [ ] **Task 5 — Snapshot + bench (regeneration).** Re-run the calibrator with the
  committed prefix flags on the committed fixtures; commit the new `expected.json` +
  `vectors.json.gz` together, stamped with the scheme (the ONLY task allowed to
  regenerate fixtures — per the Task 2 guardrail). Extend `wisdom_gate_bench.rs` to
  replay both schemes. Run the bench locally (`--features slow-tests`), paste output
  in the PR.
- [ ] **Task 6 — Tests (a) + (e) + log line.** Byte-exact literal test (a);
  reader-inventory grep test (e); the gate-decision log line per spec blocker 3 —
  format pinned for machine parsing (F5): one line, tab-separated fields in order
  `ts<TAB>scheme<TAB>open|closed<TAB>n_results` (the tripwire cron parses this).
- [ ] **Task 7 — Full check + docs.** fmt, clippy -D warnings, full non-gated test
  suite, bench replay, CHANGELOG untouched (release automation owns it).
- [ ] **Task 8 — Pre-merge gates (BLOCKS Steps 7–9; not post-merge — plan-review F2/F3).**
  1. **Live paired calibration (spec blocker 1):** scratch-brain copy; 150 probes
     sampled from recent session logs; GLM-5.3 pre-labels; **Kurt validates ≥ 30
     (PAUSE for Kurt here — user participation is part of the gate)**; run cell A
     (raw, 0.70) vs cell E (0.64) on the same labelled probes. Pass = E hit@2 ≥ A
     hit@2 paired AND E FP ≤ 0.05 (E string only). Fail → no merge, back to spec.
  2. **Canary freeze (spec blocker 4):** select the 32 canary texts, embed frozen
     vectors under the instr1 text function, commit them with the fixtures
     (same commit as Task 5's snapshot or an immediate follow-up); weekly cron
     spec handed to Tessera ops.
  3. Tripwire cron spec (formula, baseline transition, roles) written into the PR
     description for Tessera ops.

## Post-merge (owned by Tessera + Kurt)

1. Cutover on the live brain (`ct wisdom scheme activate instr1`), latency
   re-measure (p95 ≤ 1.5 s recipe).
2. Tripwire cron + weekly canary cron go live (specs from Task 8).
