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
- Serde `deny_unknown_fields` is NOT used (staying forward/backward compatible
  with hand-edited files).
- JSON layout stays flat-compatible with today's file (same keys in the same
  places) — an existing `config.json` must parse unchanged.

### 2. Single resolution + accessor (`config` module)

One function, one derivation rule, used by every consumer:

```rust
resolve_brain_paths() -> BrainPaths      // unchanged semantics: CURATED_BRAIN_CONFIG > CURATED_BRAIN_DB-parent > CURATED_BRAIN_DIR > ~/.brain
BrainConfig::load(brain_dir) -> Result<BrainConfig>       // hard error only on malformed JSON
BrainConfig::load_lenient() -> BrainConfig                // never fails; bad fields → defaults + diagnostics
```

- All five consumers in the Problem table route through this accessor. The
  desktop entrypoint's hardcoded `default_config_path()` call, `AppDb::open`'s
  db-parent guess, and the pipeline's db-parent guess are replaced by
  `resolve_brain_paths()`.
- Backward compat: `VaultConfig` stays as a thin façade over the config module
  (same public methods) so existing call sites and tests compile; new code uses
  `BrainConfig` directly.
- Atomic writes (`tmp` + rename), already used by `inference/write_config`, are
  the single write path for all sections.

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

### 4. Headless onboarding (`--onboard`)

New subcommand routing in `main.rs` (before GUI/MCP dispatch):

```
curated-thoughts --onboard [--vault <path>] [--force]
```

Interactive prompts (stdin/stdout, no TUI):
- Vault path (default `~/Curated-Thoughts`, create if missing, sets `vault_path` + creates layout)
- Embedding profile (default local/fastembed; Ollama/OpenAI-compatible external option)
- Generation provider (skip / sidecar-model / external URL+key)

Runs the same layout-creation code the desktop entrypoint runs today
(`IMMUTABLE_DIR`, `wiki`, agents deposit dir, `.brain/converted`), writes the
unified config atomically, and prints the agent-client snippet
(`command/args/env`) for this brain dir.

`--force` overwrites existing config; otherwise existing config is merged/
preserved and a warning printed.

`--doctor` diagnoses the current binding (config path in use, parse status,
generation/embedding block completeness, vault existence, db existence) —
actionable text, exit codes 0/1. `doctor` is the loud surface for the
silent-failure class above.

### 5. Settings UX (minimal)

`AgentIntegrationPanel` gains a status line: "Sidecar bound to: <brain dir> (via
CURATED_BRAIN_DIR / default)". No snippet changes. Power-user multi-instance
binding remains: copy snippet per instance.

### 6. Tests

- Unit: leniency matrix (legacy embed variants, missing blocks, malformed JSON
  fatal, per-field drop-to-default), resolution-precedence tests (env matrix).
- Integration: entrypoint/pipeline/AppDb all observe `CURATED_BRAIN_DB`+
  `CURATED_BRAIN_CONFIG` split (fixture configs in temp dirs); round-trip of a
  real config.json from `~/.brain` and `~/.brain-equational-wiki` through
  `BrainConfig` without lossy field changes.
- CLI: `--onboard` on a temp HOME (creates layout + config, snippet prints),
  `--doctor` on healthy/missing/malformed fixtures, exit codes.
- CI: extend the existing sidecar smoke test (`scripts/smoke_test_mcp_sidecar.sh`)
  with an `--onboard`-then-`--mcp` boot sequence.

### 7. Rollout

- Land behind no feature flag; access-layer refactor + new subcommands are
  additive. The `tools/` crate `curated-thoughts-mcp` dev binary keeps working
  (it links `tauri_app_lib::retrieval`, which now serves the unified accessor).
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

## Open questions

1. Should `--onboard` also create/verify `brain.db` (schema migration) or leave
   DB creation to first app/`ct` run? Leaning: verify-only, warn if missing.
2. Should `ct` (tools crate) get `onboard`/`doctor` subcommands mirroring the
   shipped binary? Leans no — ship on the bundled binary only, keep tools dev-only.
3. Embedding "External" provider in headless onboarding: prompt for base_url +
   model only, or also API key? Leans base_url+model, key via env (avoids keys in
   shell history).
