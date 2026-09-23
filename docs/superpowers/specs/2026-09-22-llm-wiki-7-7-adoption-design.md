# core-llm-wiki 7.7.4 adoption and pattern integration

**Date:** 2026-09-22
**Status:** Implemented on `feat/llm-wiki-7-7-adoption` (rev 2) — pending PR review
**Branch:** `feat/llm-wiki-7-7-adoption` (spec + plan + implementation ride one branch and one PR)
**Upstream:** expo-llm-wiki `v7.7.4` (`4ea9924`); upstream design for grounding/diagnostics/classifier: `docs/superpowers/specs/2026-09-21-grounding-diagnostics-classifier-design.md`

## Problem

Curated Thoughts (CT) pins the `@equationalapplications/*` llm-wiki packages at
`7.1.1`. Upstream `7.2.0`–`7.7.4` adds engine migration V12, typed diagnostics,
draft review, a read-only lint report, and an optional System-One classifier
hook (`LLMProvider.classify`, e.g. TypeSafe's Jev) for ontology backfill.

CT cannot adopt these blindly. Two CT invariants shape the design:

1. **CT runs ingest, librarian synthesis, heal and prune in Rust**, not in the
   TS engine. Engine features that act only on those engine writers do nothing
   in CT.
2. **CT's outbox replicas carry `lifecycle_status`.** Upstream `promoteDraft`
   deliberately writes no outbox row, so calling it from CT would silently
   diverge local and replica state.

The Jev classifier also sends fact text off-device, so it must obey CT's
privacy modes.

## Verified baseline (grepped on `main` @ `8587e09`; plans must re-verify line numbers)

- **Pin sites (three, all must move together):** `package.json` dependencies and
  `pnpm.overrides` (core-llm-wiki, core-okf, react-llm-wiki,
  schema-org-llm-wiki, schema-software-org, all `7.1.1`);
  `src-tauri/tests/engine_source_ref_gate.rs:51-62` (expected engine version);
  `src-tauri/src/db/schema_guard.rs:8` (`PINNED_CORE_LLM_WIKI_VERSION`).
- **Engine surface CT actually calls** (`src/lib/wiki.ts`): `createWiki` +
  `setup()` (engine migrations), `setOntologyManifest(s)`,
  `runOntologyBackfill` (inside `applyOntologyChange`), `read` (only via
  `tieredRead`, which has **no production callers** — tests only),
  `enableOutbox`. `WikiProvider` wraps the app in `src/main.tsx`.
- **Rust-native paths:** `ingest_document_cmd` (`lib.rs` ~3277, `pipeline::`),
  `run_wiki_heal` (`lib.rs` ~2167), `run_wiki_prune` (`lib.rs` ~2206),
  librarian synthesis (`librarian/synthesis.rs`). Search: `useMemoryRead` →
  `searchVault` (Rust), not the engine's `read`.
- **Upstream V12** (`packages/core/src/db/migrations.ts`): creates
  `{prefix}edges_entity_id_idx ON edges(entity_id, id)`, then drops
  `{prefix}edges_entity_idx`; `schema.ts` fresh DDL changes to match. CT
  mirrors the old index at `src-tauri/src/db/okf_ddl.rs:101`;
  `ddl_compat::rust_llm_wiki_ddl_matches_core_llm_wiki_package` fails on drift.
- **Upstream `promoteDraft`** (`WikiMemory.ts` ~933): in one tx,
  `setLifecycleStatus(…,'stable')` + `writeOkfTrust(…, [{ by, at }])`; no outbox
  row, no `updated_at` bump. Trust tier derives from `by` prefix: `human:` →
  `human-reviewed`.
- **CT outbox helper:** `db::commit::push_entries_outbox` (`commit.rs:1303`) +
  `wiki_fact_outbox_payload` (`commit.rs:1210`), used by the existing
  `OutboxOperation::Update` path.
- **Privacy:** `privacy::enforce::allows_external_generation(mode)` — false in
  `Strict`, true in `Ephemeral` and `Connected`.
- **Provider config:** `inference::config::LlmConfig { generation, embedding }`
  in `~/.brain/config.json`; generation is an OpenAI-compatible
  chat-completions route (`inference/mod.rs::generate_text`).
- **Upstream classify contract** (`packages/core/src/types.ts` ~554–620):
  `classify?(ClassifyRequest) → { answers }`; question kinds
  `choice | binary | score`; engine uses it only when
  `ontology.backfillClassifier: 'auto'` (default `'llm'`); one `choice` question
  per untyped fact over manifest node-type slugs; answers below
  `classifyMinConfidence` (default 0.5) stay untyped; **no edges** in classifier
  mode; >255 node types or absent `classify` fall back to the generative path; a
  thrown `classify` counts as `skipped`.

## 1. Version bumps and schema sync [CT-REQ-BUMP-01]

- Move all five packages (dependencies + overrides) from `7.1.1` to `7.7.4`.
- Move `engine_source_ref_gate.rs` and `schema_guard.rs` pins to `7.7.4`.
- `okf_ddl.rs`: replace the edges index with
  `CREATE INDEX IF NOT EXISTS llm_wiki_edges_entity_id_idx ON llm_wiki_edges(entity_id, id);`
  so fresh Rust-created DBs match engine fresh DDL.
- Add an idempotent CT-side step on Rust connection open (the same open path
  that runs `verify_llm_wiki_schema`) that creates `llm_wiki_edges_entity_id_idx`
  **before** dropping `llm_wiki_edges_entity_idx`, so a DB first opened by the
  CLI/MCP binaries — before any TS `setup()` — gets the V12 index shape and is
  never without an `entity_id` index. It must be safe to run after the engine's
  own V12 has already run.
- `ddl_compat` drift test passes against the 7.7.4 package; `tests/okf_migration.rs`
  schema watermark assertion is re-checked and updated only if the engine bump
  moves it.

## 2. Engine diagnostics (`onDiagnostic`) [CT-REQ-DIAG-01]

- `makeWikiOptions` passes `onDiagnostic`, forwarding each diagnostic object
  (`code, severity, operation, trigger, entityId, at, message, detail`) to a new
  Tauri command `record_wiki_diagnostic`.
- Rust logs each diagnostic and keeps in-memory per-severity counts, exposed
  through the existing wiki status payload so `StatusBar` and
  `MaintenanceDashboard` show the error/warning counts. Counts reset on app
  restart and on vault switch. No persisted table in this PR.
- The TS hook never throws: an IPC failure is caught and logged with
  `console.warn`. Diagnostics carry IDs/counts/slugs only, never fact text or
  LLM output (upstream contract); CT forwards them unchanged.

## 3. Drafts [CT-REQ-DRAFT-01]

- **Reads:** `tieredRead` passes `excludeDrafts: false` explicitly (current
  behavior, now explicit). Rust `searchVault` is unchanged; drafts stay visible
  there too. Flipping visibility is a later decision.
- **Drafts panel (MaintenanceDashboard):** lists drafts per seeded tier
  (`tier_fact`, `tier_wisdom`, workspace tier) with the engine's read-only
  `listDrafts(entityId, { limit, cursor })`, paginated by `nextCursor`.
- **Promotion goes through Rust, not `wiki.promoteDraft`:** new command
  `promote_draft_cmd(entry_id, entity_id)`. In one SQLite transaction:
  1. Require the row to exist, be live, belong to `entity_id`, and have
     `lifecycle_status = 'draft'`; otherwise return a structured error
     (`not_found` / `not_draft`) and write nothing.
  2. Set `lifecycle_status = 'stable'`.
  3. Append `{ "by": "human:local", "at": <ISO-8601 now> }` to `okf_verified`
     (JSON array, existing entries preserved). `human:local` is a fixed actor
     string; upstream derives `human-reviewed` from the `human:` prefix.
     Like core's `writeOkfTrust`, also set `last_verified_by = 'human:local'`
     and `last_verified_at = <now, epoch ms>`.
  4. Push an `OutboxOperation::Update` row via `push_entries_outbox` with the
     payload built by `wiki_fact_outbox_payload` from the post-update row, so
     replicas receive the new `lifecycle_status` and `okf_verified`.
  - `updated_at` is left unchanged, matching upstream promote semantics.
- The HVG proposal review queue (spec 2026-09-09) is untouched: it gates
  proposals before commit; this panel promotes already-committed draft entries.

## 4. Lint [CT-REQ-LINT-01]

- **Health report panel (MaintenanceDashboard):** on user request, runs
  `wiki.lint(entityId)` for each seeded tier and shows `danglingEdges`,
  `manifestViolations`, `untypedFacts`, `drafts`, `unverifiedInferred`, and the
  `sample` edge IDs.
- Report only. No repair buttons; `purge_off_manifest_edges_cmd` remains the only
  edge repair path.

## 5. Jev classifier [CT-REQ-CLASS-01]

### 5.1 Config

New optional `classifier` block in `config.json`, beside `generation` and
`embedding` (absent block = `unconfigured`; existing configs load unchanged):

| Field | Type | Notes |
|---|---|---|
| `provider` | `unconfigured` \| `jev_http` \| `cloudflare_jev` | default `unconfigured` |
| `url` | string | required for `jev_http` |
| `account_id` | string | required for `cloudflare_jev`; URL built from it and the Workers AI `typesafe/jev` model route |
| `api_key` | string | bearer token; stored like `generation.api_key` |
| `min_confidence` | number in [0, 1] | maps to `ontology.classifyMinConfidence`; default 0.5 |
| `timeout_secs` | integer | per-request timeout; default 30 |

The exact Workers AI REST path and Jev field names are confirmed against the
provider's current API reference in the implementation plan; the mapping below
follows upstream's documented adapter (core README, "Classifier mode").

### 5.2 `classify` Tauri command

- Signature mirrors `ClassifyRequest` → `ClassifyResponse`.
- **Privacy gate first:** if `!allows_external_generation(mode)` (i.e. `Strict`)
  or the provider is `unconfigured`, return `classifier-not-available` without
  any network call.
- Request mapping: `choice { options }` → Jev `choice` with `criteria` = option →
  option; `score { levels }` → Jev `score` with `criteria` = levels; `binary` →
  Jev `noul`. `instructions` passes through.
- Response mapping: Jev `choice` → `{ kind: 'choice', choice, confidence,
  probabilities }`; Jev `score` → `{ kind: 'score', score, confidence,
  probabilities }` with level-keyed probabilities sorted numerically into an
  array; Jev `noul` → `{ kind: 'binary', probability }`.
- Shape check before returning: every requested key present; numbers finite.
  Range and off-list checks stay in the engine (upstream validates answers as
  untrusted), so CT does not duplicate them. A shape failure or HTTP error is
  returned as an error string; the engine counts a thrown `classify` as
  `skipped` and retries next pass.

### 5.3 Engine wiring

- `makeWikiOptions` adds `llmProvider.classify` (invoking the command) and sets
  `config.ontology.backfillClassifier = 'auto'` and `classifyMinConfidence`
  **only when** the classifier is configured and the current privacy mode allows
  it. Otherwise neither is set, and backfill is exactly today's generative path.
- A new Tauri query `classifier_status` (`{ available: bool }`) drives that
  decision at `createWiki` time. The wiki instance is rebuilt on classifier
  config change or privacy mode change, using the same generation-counter
  pattern as the `outbox-worker-started`/`-stopped` listeners in `setupWiki`.
- **Schema switch stays generative.** `applyOntologyChange` (forward loop and
  rollback loop) calls `runOntologyBackfill(entityId, { classifier: 'llm' })`
  explicitly, so a switch always extracts manifest edges. Rationale (rev 2):
  backfill scans only `okf_type IS NULL` facts and classifier mode proposes no
  edges, so a classifier pass followed by an `'llm'` pass would find nothing
  left to scan and add zero edges.
- **Classifier use site: "Type untyped facts" action.** The engine never
  auto-runs backfill and CT's Rust ingest leaves facts untyped, so the backlog
  grows between schema switches. MaintenanceDashboard gets a "Type untyped
  facts" button that, for each seeded tier, loops
  `runOntologyBackfill(entityId, { classifier: 'auto' })` until `remaining` is 0
  (or a pass makes no progress: `typed === 0 && remaining > 0` stops the loop,
  since low-confidence facts are cooldown-stamped and would otherwise spin).
  With no classifier available, `'auto'` falls back to the generative path, so
  the button works either way. It sits next to the Health report's
  `untypedFacts` count and is disabled while any wiki job is busy.

### 5.4 Settings UI

New Classifier section in Settings: provider picker, fields per provider,
disabled with an explanation in `Strict` mode, and a disclosure that fact
titles and bodies are sent to the configured endpoint during ontology backfill.

## 6. Out of scope

Recorded here so no one assumes the protection exists:

- **`grounding`** — only affects engine ingest/librarian/heal, which CT runs in
  Rust. Follow-up: port the quote-evidence check into the Rust ingest pipeline
  alongside the #186 evidence gate (separate spec).
- **`pendingSources`** — tracks engine-ingest partial rows; CT ingest is Rust.
- **`getInstructions`** — returns engine writer prompts; CT's MCP server is Rust
  and its writers use their own prompts.
- A logprobs-based classifier over the generation route.
- Upstream §7.4 deferred classifier uses (edge-type selection, entailment,
  heal duplicate detection).

## 7. Testing

**Rust** (run with `--features test-utils,mcp-server`, as CI does):

- Jev mapping round-trip for all three kinds, plus malformed response, missing
  key, non-finite number, HTTP error, and timeout.
- `classify` in `Strict` returns `classifier-not-available` with no request made
  (assert against a local mock server).
- `promote_draft_cmd`: writes one `Update` outbox row whose payload carries
  `lifecycle_status: "stable"` and the appended `okf_verified`; `not_found` and
  `not_draft` write nothing (no row change, no outbox row).
- V12 step: on a DB with only `llm_wiki_edges_entity_idx`, afterwards exactly
  `llm_wiki_edges_entity_id_idx` exists; re-running is a no-op.
- `ddl_compat` drift test and `engine_source_ref_gate` pass at 7.7.4.

**TypeScript** (vitest):

- `makeWikiOptions` with classifier unavailable has no `classify` key and no
  `backfillClassifier`; with it available, both are set.
- `onDiagnostic` survives a rejected `invoke`.
- Drafts and Health report panels render counts and pagination, and promote calls
  `promote_draft_cmd`, never `wiki.promoteDraft`.
- `applyOntologyChange` passes `{ classifier: 'llm' }` on every backfill call
  (forward and rollback).
- "Type untyped facts" loops `{ classifier: 'auto' }` per tier until
  `remaining === 0`, and stops on a no-progress pass.

## Risks

- **Jev API drift:** early-access vendor API; the mapping sits in one Rust
  module with its own tests.
- **Backfill cost:** classifier mode is one request per untyped fact (upstream
  §2.2); large vaults mean many requests. Settings disclosure mentions this.
- **Engine writes outside Rust commit discipline:** `runOntologyBackfill` already
  writes through the TS adapter today; this PR adds no new engine write path
  (promotion is Rust-side by design).
