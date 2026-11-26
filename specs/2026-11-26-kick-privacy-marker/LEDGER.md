# LEDGER — kick-ledger privacy marker (#362)

## Working notes

- Fresh worktree /tmp/ct-362 (main checkout untouched). Sidecar stub created
  (untracked, never committed).
- Repo is not a cargo workspace: run cargo from src-tauri/ directly.
- No cargo fmt write-mode.

## Findings

- `set_kick_state` already takes `error: Option<&str>` — only
  `audit_and_seed_kick`'s call site hardcodes None.
- Existing test pattern `with_external_provider` in
  synthesis.rs completer_privacy_tests gives the config.json shape for
  external+strict test setups.
