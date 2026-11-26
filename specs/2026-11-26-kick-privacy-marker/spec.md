# Spec: Record `blocked_by_privacy_mode` in the kick ledger error column (#362)

Status: Reviewed (owner chat ruling 2026-11-26)

## Problem

When a wisdom deposit or supersession kick cannot run, the ledger seeds
`state = 'no_ingest_host'` with `error = NULL`. Two different causes fold
into that one row: a truly unconfigured generation provider, and a
configured External provider blocked by strict privacy mode (INTENT rule
11). The ledger cannot distinguish them, so debugging a `no_ingest_host`
deposit requires re-deriving the cause by hand.

## Evidence

- `llm_generation_configured()`
  (`src-tauri/src/librarian/synthesis.rs:1404`) folds two causes into one
  bool: provider `Unconfigured`, OR provider `External` blocked by the
  strict-privacy gate in `build_llm_completer`'s
  `GenerationProviderKind::External` arm (`synthesis.rs:307-308`, INTENT
  rule 11) via `crate::privacy::allows_external_generation(mode)`.
- Both seed sites (`src-tauri/src/wisdom_deposit.rs` ~1114 and ~1420) call
  `audit_and_seed_kick(...)` with
  `initial_kick_state(generation_configured)`.
- `audit_and_seed_kick` (~767) writes the initial row through
  `set_kick_state(&tx, &kick_rel, initial_state, None)` — the error
  argument is hardcoded `None`.
- `deposit_kick_state` (`src-tauri/src/db/connection.rs:793-798`) already
  has an `error TEXT` column. The `state` CHECK already includes
  `'no_ingest_host'`. No schema change is needed.
- The no-auto-redrive semantics already ship
  (`wisdom_deposit.rs:841-845` doc comment). This change is observability
  only.

## Owner approval (verbatim, counts as spec approval)

Owner design ruling 2026-11-26:

> Decouple observability from re-drive (no automatic re-drive). Never
> auto-redrive Strict-era blocked attempts when privacy mode later
> relaxes. Keep status = 'no_ingest_host' (zero enum/MCP churn) and
> record 'blocked_by_privacy_mode' in the existing error text column.

## Proposed change

1. Add `pub(crate) fn llm_generation_privacy_blocked() -> bool` in
   `src-tauri/src/librarian/synthesis.rs`. It loads `BrainConfig` through
   the same lenient path `build_llm_completer` uses (manual `BrainPaths`,
   `config::BrainConfig::load_lenient`), then matches
   `GenerationProviderKind`:
   - `Unconfigured` or `Sidecar` => `false`
   - `External` => `!allows_external_generation(effective_mode(&config.privacy))`
     (same fail-closed raw mode read as the `External` arm in
     `build_llm_completer`)
   A config load error returns `false` (not privacy-caused).
   Do NOT refactor `build_llm_completer`'s signature; it has other callers.
2. In `src-tauri/src/wisdom_deposit.rs`:
   - `audit_and_seed_kick` gains a `kick_error: Option<&'static str>`
     parameter and passes it to `set_kick_state` instead of the hardcoded
     `None`.
   - Both seed sites (deposit lane ~1114, supersession lane ~1420) compute:
     when `generation_configured` is false AND
     `llm_generation_privacy_blocked()` is true, pass
     `Some("blocked_by_privacy_mode")`; else `None`.
   - Update `initial_kick_state`'s doc comment to mention the
     error-column marker.
3. No behavior change. No automatic re-drive. `state` stays
   `'no_ingest_host'`. Zero enum/MCP/schema churn.

## Files touched

- `src-tauri/src/librarian/synthesis.rs` — new helper + doc comment.
- `src-tauri/src/wisdom_deposit.rs` — new `audit_and_seed_kick` param,
  both seed sites, `initial_kick_state` doc comment, two new tests.

## Rejected alternatives

- Re-drive Strict-era blocked kicks when privacy mode relaxes: rejected by
  the owner ruling (no automatic re-drive).
- New ledger state enum value (e.g. `'blocked_by_privacy_mode'` state):
  rejected — owner ruled for zero enum/MCP churn; the existing `error`
  column carries the cause.
- Refactoring `build_llm_completer` to expose its config load: rejected —
  it has other callers and a signature change would churn them.

## Test plan

Unit tests in `wisdom_deposit.rs`'s `mod tests`, following the existing
kick-seed conventions (`wisdom_deposit.rs:2430`,
`connection.rs:5851`):

1. Strict privacy mode + External provider configured: deposit lands, the
   seeded `deposit_kick_state` row has `state = 'no_ingest_host'` and
   `error = 'blocked_by_privacy_mode'`.
2. Provider unconfigured: the seeded row has `error = NULL` and
   `state = 'no_ingest_host'`.

Run from `src-tauri/` with `PATH=$HOME/.cargo/bin:$PATH` and
`CURATED_BRAIN_DIR=/tmp/ct-canary`: targeted
`cargo test --features test-utils <names>`, then the full lib suite
`cargo test --features test-utils -- --test-threads=1` before pushing.
Aggregate `test result:` lines. No `cargo fmt` write-mode.
