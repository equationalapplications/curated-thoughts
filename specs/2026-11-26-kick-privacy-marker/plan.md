# Plan: kick-ledger privacy marker (#362)

Approval: owner chat ruling 2026-11-26 covers spec + plan (owner pre-approved
the design in the dispatch brief).

## Commit 1 — spec (done)

`docs(spec): kick-ledger privacy marker observability (#362)`

## Commit 2 — implementation

1. `src-tauri/src/librarian/synthesis.rs`: add
   `pub(crate) fn llm_generation_privacy_blocked() -> bool` next to
   `llm_generation_configured`. Same lenient config load as
   `build_llm_completer` (manual `BrainPaths` + `load_lenient`); load error
   => false; `Unconfigured`/`Sidecar` => false; `External` =>
   `!allows_external_generation(effective_mode(...))`.
2. `src-tauri/src/wisdom_deposit.rs`:
   - `audit_and_seed_kick` + `kick_error: Option<&'static str>` param,
     forwarded to `set_kick_state`.
   - Both seed sites compute `kick_error` from
     `!generation_configured && llm_generation_privacy_blocked()`.
   - `initial_kick_state` doc comment mentions the marker.

Tests (per spec test plan): strict+External => error marker + no_ingest_host;
unconfigured => NULL error. Targeted cargo test first, full lib suite single
thread before push.

## Risks

- Double config load at deposit time (helper + completer probe): accepted,
  matches existing probing cost; deposit lane is not hot.
- Test env isolation: follow `with_brain`/`temp_env` discipline; rewrite
  `config.json` inside the redirected env before dispatch.
