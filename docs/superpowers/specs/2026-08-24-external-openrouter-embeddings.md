# Spec: External (OpenRouter) embedding profile — recover + harden

**Date:** 2026-08-24
**Repo:** curated-thoughts
**Branch:** `feat/external-embeddings-v2`
**Type:** feature (recovered from lost work) + resilience fixes
**Provenance:** `EmbedProfile::External` was implemented Aug 22 but never committed; recovered from `stash@{1}` ("wip external embeddings") onto this branch. All 439 lib tests green locally.

## Problem

1. Vault ingest silently falls back to local Ollama embeddings because external embedding was never a supported variant (`EmbedProfile::Cloud`/`External` returned "cloud embed not implemented"). Kurt's intent: **OpenRouter-hosted embeddings only** for this vault.
2. Secrets and URLs written into `config.json` on this machine get **redacted and corrupted** (Hermes secret-redaction rewrites keys and IP literals). This has destroyed the OpenRouter key in config **three times**, and earlier corrupted an endpoint URL.
3. Config drift between UI writes and CLI tools has repeatedly reverted `embed_profile` between `local` and `external`.

## Root cause

- Feature gap: no code path for external embeddings (evidence: `embed_batch()` match arm).
- Fragility: any secret material stored in JSON config files on this machine is one redaction pass away from corruption. Config must hold **no secrets and no literal endpoints that matter**.

## Proposed change

Recovered implementation (from stash), plus hardening:

1. **`EmbedProfile::External { profile: ExternalEmbedProfile }`** — OpenAI-compatible `POST {base}/v1/embeddings`, batched ≤64 inputs, index-aware response parsing, dimension-consistent output. *(recovered)*
2. **Key resolution order** *(recovered)*: profile field → `EMBED_API_KEY` env → provider default env (`OPENROUTER_API_KEY` for openrouter bases, else `OPENAI_API_KEY`) → clear error naming which to set. **Config file needs no key at all.**
3. **NEW: optional `base_url`.** When omitted (and model doesn't imply otherwise), defaults to `https://openrouter.ai/api/v1`. So the minimal config entry contains only `{"type": "external", "model": "openai/text-embedding-3-small"}` — nothing that redaction can corrupt.
4. **NEW: `.no_proxy()` parity** on the external embedder's reqwest client (matches ollama.rs fix from PR #64/#70 lineage; avoids system-proxy resolution failures machine-wide).
5. **Dispatch wiring** in `embed_batch()` (`EmbedProfile::External => profile.embed(texts)`); `OllamaEmbedder::from_profile` rejects non-local profiles with a clear message. *(partially recovered, completed)*
6. **Tests**: legacy-config poison test updated (unknown `type` drops profile, keeps vault path); new round-trip test proving `external` survives config read/write; stash's own tests for URL building/key resolution/batching retained. *(done, suite green 439/0)*

## Explicit non-goals

- **No change to defaults.** Missing/unset `embed_profile` still means local Ollama (`nomic-embed-text` @ localhost:11434). Local remains fully supported.
- No config-file migration or UI changes in this PR.
- Do NOT write the API key or full endpoint into any config file — ever. Env vars only.

## Files touched

- `src-tauri/src/embedder/mod.rs` (recovered ExternalEmbedProfile + dispatch)
- `src-tauri/src/embedder/ollama.rs` (exhaustive match fix)
- `src-tauri/src/vault/config.rs` (test updates)

## Test plan

- Full suite: `cargo test --lib --features tauri/test` → target 0 failures (currently 439 passed).
- Post-merge live verification (out of band): with `OPENROUTER_API_KEY` set in env, switch `embed_profile` to external, clear 3 stale local vectors from brain.db, re-run `ingest_vault_once`, assert all embeddings are 1536-dim and zero local-Ollama connections occur during ingest.

## Operational instructions for Kurt (after merge)

Set the key once in your shell profile:

```bash
# ~/.bashrc (or ~/.profile)
export OPENROUTER_API_KEY="sk-or-..."   # paste your real key here yourself
```

Then `source ~/.bashrc`. Any terminal you launch ingest/MCP from inherits it. Never paste the key into chat, config files, or commands — I will never ask you to.
