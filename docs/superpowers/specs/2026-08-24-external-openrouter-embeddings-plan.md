# Plan: External (OpenRouter) embedding profile

**Spec:** `docs/superpowers/specs/2026-08-24-external-openrouter-embeddings.md`
**Branch:** `feat/external-embeddings-v2` (already created from ff-synced main; stash recovered)

## Commits

1. `feat(embedder): add External profile for OpenAI-compatible embedding endpoints` — mod.rs (ExternalEmbedProfile, dispatch, tests) + ollama.rs exhaustive-match fix + vault/config.rs test updates (legacy poison case now uses unknown type; new external round-trip test).
   - Tests: full suite `cargo test --lib --features tauri/test` (currently 439 passed / 0 failed).

## Risks

- Low: purely additive variant; default/local paths unchanged and covered by existing suite.
- Config-schema addition is backward compatible; unknown types still drop gracefully.

## Verification after merge (out-of-band, with Kurt)

1. Key via env (`OPENROUTER_API_KEY`) or copied programmatically from existing `~/.brain/config.json` generation key (never printed/logged).
2. Flip live `~/.brain-equational-wiki/config.json` embed_profile to external (scripted, values never echoed).
3. Purge 3 stale 768-dim vectors from killed local run.
4. Re-run `ingest_vault_once`; assert 1536-dim throughout, no localhost:11434 traffic during ingest.
5. Then librarian maintenance run (`run_librarian_once`) — needs generation config present in live brain dir too.

## Gates

- Subagent self-review against spec before push (hard rule).
