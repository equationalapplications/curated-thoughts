# Unified Brain Configuration — Design Spec

**Date:** 2026-08-28
**Status:** Approved design (pending implementation plan)
**Repo:** curated-thoughts
**Origin:** Discord thread "Curated Thoughts Configuration" with Kurt, 2026-08-28.

## Problem

The desktop app and the MCP sidecar nominally share `{brain_dir}/config.json`, but
today that file is written and read through **three parallel, hand-rolled access
layers** that disagree about where the file lives and how failures are handled:

| Consumer | Config path derivation | Failure behavior |
|---|---|---|
| Desktop entrypoint (`lib.rs::run`) | Hardcoded `VaultConfig::default_config_path()` (`~/.brain/config.json`) | `.ok().flatten()` — silent |
| `AppDb::open` | `{db parent}/config.json` | `.unwrap_or(None)` — silent |
| Pipeline worker | `{db parent}/config.json` | `.unwrap_or_else(default profile)` — silent |
| MCP sidecar (shipped, `mcp_server.rs`) + `tools` crate | `resolve_brain_paths()` (env → brain dir) | Errors propagate with hints |
| `inference/config.rs::read_config` | `config_path(brain_dir)` (env → brain dir) | `unwrap_or_default()` — **silent** (re-onboards or runs unconfigured) |
| `privacy/mod.rs` | `{brain_dir}/config.json`, third reader/writer | Missing file → default |
| Frontend panels | via `get_provider_config` etc. | — |

Three concrete failure classes this causes:

1. **Split-file drift.** `CURATED_BRAIN_DB`/`CURATED_BRAIN_CONFIG` set → desktop
   entrypoint still reads `~/.brain/config.json` for the v2-migration flag; two
   files can drift while the app runs.
2. **Silent misconfiguration.** `read_config` swallows malformed JSON
   (`serde_json::from_str(...).unwrap_or_default()`), so a single bad field resets
   generation/embedding to `Unconfigured` and the app "just works" with no LLM —
   the "LLM provider not configured" silent failure Kurt hit in Aug 2026.
3. **No shared onboarding.** Onboarding exists only in the desktop UI. The shipped
   sidecar binary is CLI-installable (deb) but has no headless setup: a CLI-only
   user must hand-write `config.json`.

## Non-goals

- No global multi-vault registry or pointer file. Explicitly rejected in design
  discussion: one install = one instance = one config; N installs = N fully
  independent instances (no single-instance lock exists; brain dir = instance
  identity). Env-var binding per sidecar is intentional.
- No live re-pointing of running sidecar processes (agent clients own sidecar
  lifecycles, not the app).
- No new network transports or new MCP tools.
- Vault contents/migration semantics (`migrated_to_v2`) unchanged — only the
  access layer around them.

## Design

### 1. Unified schema (`BrainConfig`)

New module `src-tauri/src/config/mod.rs` defining one typed struct per brain dir:

```rust
pub struct BrainConfig {
    pub vault_path: Option<String>,
    pub embed_profile: Option<EmbedProfile>,
    pub migrated_to_v2: bool,
    pub generation: GenerationConfig,
    pub embedding: EmbeddingConfig,
    privacy: PrivacyConfig,
}
```

- `generation`/`embedding` move (type definitions only) from
  `inference/config.rs` into the config module; `inference` re-exports them for
  API compatibility.
- Leniency policy from `vault/config.rs` carries over, generalized: **unknown or
  per-field-unparseable values are dropped to that field's default, never fatal
  to the whole file**; malformed top-level JSON is a hard error. Exact per-field
  leniency matrix (which fields tolerate what) is an implementation-plan detail,
  but the privacy gotcha from Aug 2026 stands: complete `generation`+`embedding`
  blocks must round-trip, and missing blocks must be loud in `doctor`/onboarding
  output, not silently defaulted.
- **`vault_path` carve-out (final-review M1):** present-but-unparseable
  `vault_path` stays a **hard error**. Today's `vault/config.rs::from_text`
  deliberately refuses to mask it (masking once silently reset users' vault
  path and forced re-onboarding); the blanket leniency rule does not apply to
  this field.
- **`LlmConfig`/`read_config` fate (final-review M3):** `LlmConfig` remains as
  a compatibility struct `{generation, embedding}` constructible from
  `BrainConfig`; `read_config` becomes a deprecated thin delegation to the
  unified load (loud `Result`). New code uses `BrainConfig` directly.
- Serde `deny_unknown_fields` is NOT used (staying forward/backward compatible
  with hand-edited files).
- JSON layout stays flat-compatible with today's file (same keys in the same
  places) — an existing `config.json` must parse unchanged.
- **Unknown-key preservation on write (CodeRabbit #2, PR #120).** Serialization
  must not drop unmodeled keys, including nested ones. The write path is a
  **raw-document merge**: read existing JSON as a `serde_json::Value` tree,
  overlay the modeled sections, write back — same strategy as today's
  `inference::write_config`. A typed deserialize → serialize round-trip is
  forbidden as the write path. Test: round-trip a config containing an unknown
  top-level key and an unknown nested key; both must survive.

### 2. Single resolution + accessor (`config` module)

One function, one derivation rule, used by every consumer:

```rust
resolve_brain_paths() -> BrainPaths      // unchanged semantics: CURATED_BRAIN_CONFIG > CURATED_BRAIN_DB-parent > CURATED_BRAIN_DIR > ~/.brain
BrainConfig::load(paths: &BrainPaths) -> Result<BrainConfig>   // reads paths.config_path exactly — never re-derives
BrainConfig::load_lenient(paths: &BrainPaths) -> LoadReport    // per-field leniency; malformed top-level JSON stays FATAL
```

(Signatures take the resolved `BrainPaths`, not `brain_dir`: joining
`config.json` inside `load` would re-derive the path and break
`CURATED_BRAIN_CONFIG` split launches — final-review C3.)

`load_lenient` returns a **`LoadReport`**, not a bare `BrainConfig` — an opaque
return would hide which fields were silently defaulted, and `--doctor` could not
tell "generation block present" from "filled in by leniency":

```rust
pub struct LoadReport {
    pub config: BrainConfig,
    /// One entry per silently-defaulted field. (A structured enum may replace
    /// the Vec in the plan; the contract is "every defaulting is observable".)
    pub diagnostics: Vec<String>,
    pub generation_missing: bool,
    pub embedding_missing: bool,
}
```

Consumers: `--doctor` renders the report (this is its primary data source);
the pipeline worker logs `diagnostics` and continues with `.config`; desktop
commands surface diagnostics through events/error strings.

`load_lenient` is lenient about *fields*, never about the document: truncated
or invalid top-level JSON is a hard error in every load mode, so the pipeline
cannot continue on garbage defaults — CodeRabbit #1, PR #120.

- All seven consumers in the Problem table route through this accessor —
  including `privacy/mod.rs` (retiring its third reader/writer) and the
  frontend path (`get_provider_config` → unified load). The replacements:
  desktop entrypoint hardcoded `default_config_path()` → `resolve_brain_paths()`;
  `AppDb::open` db-parent guess → `resolve_brain_paths()`; pipeline db-parent
  guess → `resolve_brain_paths()`; `privacy/mod.rs` hand-rolled reader/writer →
  unified accessor + atomic write path; frontend commands → unified load via
  shared command helpers. Each consumer gets a test (or an explicit exclusion
  reason in the plan).
- Backward compat: `VaultConfig` stays as a thin façade over the config module
  (same public methods) so existing call sites and tests compile; new code uses
  `BrainConfig` directly.
- Atomic writes (`tmp` + rename), already used by `inference/write_config`, are
  the single write path for all sections. Section-scoped raw-document merge
  means concurrent writers to *different* sections cannot clobber each other;
  same-section concurrent writes are last-writer-wins (accepted, unchanged from
  today) — final-review M4.
- **`AppDb::open` split-env rule (final-review M5):** `open(path)` derives its
  config from `resolve_brain_paths().config_path` (replacing the db-parent
  guess) and keeps its caller-supplied db path; callers pairing an explicit db
  with a matched config keep using `open_with_config`. The plan audits call
  sites for caller-db-vs-env-config disagreement.

- `read_config`'s `unwrap_or_default()` becomes a loud `Result` at call sites
  that can surface errors (desktop commands), and `load_lenient` + logged
  diagnostics where a default is genuinely acceptable (pipeline worker).

### 3. Sidecar binding (unchanged from today, now guaranteed)

Each sidecar binds to exactly one brain dir: `CURATED_BRAIN_DIR` if set, else
`~/.brain`. The bundled `mcp_server.rs` already routes through
`resolve_brain_paths()` — after this refactor it reads through the unified
accessor. Multiple installs = multiple independent sidecars, each with its own
config. Settings snippet (`AgentIntegrationPanel`) already embeds
`CURATED_BRAIN_DIR` — unchanged.

**Env-matrix contract (CodeRabbit #3, PR #120).** `resolve_brain_paths()`
combines three env vars; every component must consume the *same resolved
`BrainPaths`*, never re-derive. The binding matrix:

| Env set | brain_dir | config_path | db_path |
|---|---|---|---|
| none | `~/.brain` | `{brain_dir}/config.json` | `{brain_dir}/brain.db` |
| `DIR` only | `$DIR` | `{brain_dir}/config.json` | `{brain_dir}/brain.db` |
| `DB` only | `~/.brain` | `{db parent}/config.json` | `$DB` |
| `CONFIG` only | `~/.brain` | `$CONFIG` | `{brain_dir}/brain.db` |
| `DIR`+`DB` | `$DIR` | `{db parent}/config.json` | `$DB` |
| `DIR`+`CONFIG` | `$DIR` | `$CONFIG` | `{brain_dir}/brain.db` |
| `DB`+`CONFIG` | `~/.brain` | `$CONFIG` | `$DB` |
| `DIR`+`DB`+`CONFIG` | `$DIR` | `$CONFIG` | `$DB` |

All eight combinations are legal and must resolve exactly as named — the
resolution-precedence tests in §6 enumerate all eight rows (final-review C2:
the previous six-row table omitted `DIR+DB` and `DIR+CONFIG`).

### 4. Headless onboarding (`--onboard`)

New subcommand routing in `main.rs` (before GUI/MCP dispatch):

```console
curated-thoughts --onboard [--vault <path>] [--force]
```

Interactive prompts (stdin/stdout, no TUI):
- Vault path (default `~/Curated-Thoughts`, create if missing, sets `vault_path` + creates layout)
- Embedding profile (default `Local`/Ollama `nomic-embed-code` — the actual
  `EmbedProfile::default()`; external OpenAI-compatible option. *Not* fastembed:
  that path is bench-fixture-only — final-review G2)
- Generation provider (skip / sidecar-model / external URL+model; if external,
  the API key is prompted **optionally** and stored only in config `generation`
  per existing desktop behavior — or skipped entirely in favor of env, matching
  decision 3's spirit; the plan fixes the exact choice)

Runs the same layout-creation code the desktop entrypoint runs today
(`IMMUTABLE_DIR`, `wiki`, agents deposit dir, `.brain/converted`), writes the
unified config atomically, and prints the agent-client snippet
(`command/args/env`) for this brain dir.

`--force` overwrites existing config; otherwise existing config is merged/
preserved and a warning printed.

`--doctor` diagnoses the current binding (config path in use, parse status,
generation/embedding block completeness, vault existence, db existence) —
actionable text, exit codes: **0** = config parses + required blocks present
(0-byte optional-missing warnings allowed); **1** = config missing, malformed,
or required block absent. Fresh-install "no config" is therefore exit 1 with
a run-`--onboard` instruction (final-review T4). `doctor` is the loud surface
for the silent-failure class above.

### 5. Settings UX (minimal)

`AgentIntegrationPanel` gains a status line: "Sidecar bound to: <brain dir> (via
CURATED_BRAIN_DIR / default)". No snippet changes. Power-user multi-instance
binding remains: copy snippet per instance. Frontend test asserts the status
line renders the resolved brain dir from `get_brain_dir` (final-review T1).

### 6. Tests

- Unit: leniency matrix (legacy embed variants, missing blocks, malformed JSON
  fatal, per-field drop-to-default), resolution-precedence tests (the env
  matrix in §3, **all eight rows** — final-review C2).
- Per-consumer coverage (CodeRabbit #4, PR #120): each Problem-table consumer
  gets a test asserting it routes through the unified accessor — entrypoint,
  `AppDb::open`, pipeline, shipped sidecar (`mcp_server.rs`), tools-crate
  sidecar, `read_config` call sites, `privacy/mod.rs`, frontend
  (`get_provider_config`), plus the Settings status line (§5). (Table row 4
  bundles shipped + tools sidecars; the test list deliberately splits them —
  eight targets, final-review C1.) A consumer with no direct test needs an
  explicit exclusion reason in the plan.
- Pipeline fatal-JSON test (CodeRabbit catch 2, PR #120): feed the pipeline
  worker a truncated/invalid `config.json`; assert a **hard error** is returned
  and the worker does NOT continue with a default profile.
- Integration: entrypoint/pipeline/AppDb all observe `CURATED_BRAIN_DB`+
  `CURATED_BRAIN_CONFIG` split (fixture configs in temp dirs); round-trip of a
  real config.json from `~/.brain` and `~/.brain-equational-wiki` through
  `BrainConfig` without lossy field changes; unknown-key round-trip (§1).
- CLI: `--onboard` on a temp HOME (creates layout + config, snippet prints),
  `--doctor` on healthy/missing/malformed fixtures with the §4 exit-code map
  asserted (0 = parses + blocks present; 1 = missing/malformed/absent block —
  final-review T4/T5). `--onboard` merge-into-existing preserves unknown keys
  and prior sections; `--force` over existing config replaces modeled sections
  wholesale (final-review T3).
- Live-machine round-trip (config.json from `~/.brain` and
  `~/.brain-equational-wiki`) runs **only when those paths exist and the env
  var `CT_LIVE_CONFIG_TESTS=1` is set** — CI uses copied fixtures instead
  (final-review M6).
- CI: extend the existing sidecar smoke test
  (`tools/smoke_test_mcp_sidecar.sh`, invoked from
  `.github/workflows/build.yml` — final-review G1) with an
  `--onboard`-then-`--mcp` boot sequence.

### 7. Rollout

- Land behind no feature flag; access-layer refactor + new subcommands are
  additive. The `tools/` crate `curated-thoughts-mcp` dev binary keeps working
  (it links `tauri_app_lib::retrieval`, which now serves the unified accessor).
- **Behavior change, deliberate (final-review M2):** malformed top-level JSON
  becoming fatal (pipeline/AppDb paths that today silently default) will
  hard-fail installs that currently "just work" while misconfigured. This is
  the point of the refactor — silent re-onboarding is the Aug 2026 incident
  class — but the desktop `run()` startup path must degrade visibly, not
  crash-loop: on malformed config at startup, show an actionable error surface
  (dialog/banner with the `--doctor` remediation) and continue with defaults
  for the session; other consumers hard-fail. The plan specifies the exact
  startup UX.
- **Durability gate (final-review T2):** the `sync_data()`-before-rename
  requirement is verified by (a) a review-gate checklist item on the write-path
  PR and (b) a code-structure test asserting the temp file is synced before
  rename in the unified writer (e.g. a seam function whose order is asserted in
  a unit test).
- Watch items after release: re-check any code paths still calling
  `VaultConfig::default_config_path()` directly (grep should return only the
  config module + tests).

## Decision log

- **2026-08-28 (Discord, this thread):** Kurt approved Option 2 = unified schema
  + single resolution path + headless onboarding on the shipped binary. Global
  config registry rejected ("each instance has one config, each desktop app has
  its own sidecar"). Pointer-file (`active.json`) approach withdrawn by Hermes
  after Kurt's one-instance-per-install model made it harmful (would let
  instance #2 hijack instance #1's env-less sidecars).
- **CLI-only installs:** confirmed possible today via deb + `--mcp` sidecar, but
  no headless onboarding exists. `--onboard` closes that gap.
- **Fallback UI:** plain numbered options in Discord (buttons truncate on mobile
  and rush Kurt; logged preference 2026-08-28).

## Resolved decisions (locked 2026-08-28, Kurt)

1. **`--onboard` and `brain.db`: verify-only.** Warn if missing; the app or
   ingestion engine owns schema creation/migrations. Duplicating migration logic
   in a config tool is a recipe for drift.
2. **`ct` (tools crate) does NOT get `onboard`/`doctor`.** Tools crate stays a
   lightweight dev/headless indexing utility; the shipped bundled binary is the
   canonical user entry point for configuration and diagnostics.
3. **External embedding onboarding prompts base_url + model only; API key via
   env.** Keeps keys out of shell history, logs, and plaintext `config.json`;
   nudges users toward proper credential management (.env, keystores).

## Durability & performance notes (Kurt, 2026-08-28)

- **Atomic write durability.** The unified write path's tmp + rename must also
  `sync_data()` (file contents) on the temp file's handle *before* the rename,
  and prefer `sync_all()` when the directory entry itself must survive an
  immediate hard power loss. Plain `std::fs::write` + `rename` alone does not
  guarantee the data is on disk before the rename is visible.
- **Resolution caching.** `resolve_brain_paths()` stays uncached and
  re-evaluates env vars on every call — by design. It is called at process boot
  and command boundaries (sidecar calls it once at startup; desktop calls it in
  `run()` and behind `get_brain_dir`), not in per-tool-call hot loops, and env
  reads + path joins are microseconds. If a hot loop ever needs it, cache at the
  call site (explicit, testable) rather than baking a cache into the resolver
  (which would freeze env state and break test isolation).
