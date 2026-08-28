# Version-aware external endpoint URL resolution (Z.AI GLM Coding Plan support)

**Date:** 2026-08-28
**Status:** DRAFT — awaiting review round 1
**Author:** Tessera (Hermes Agent) at Kurt's direction
**Trigger request:** Kurt, Aug 28 2026 — use `https://api.z.ai/api/coding/paas/v4` as the Curated Thoughts LLM provider endpoint

## Problem

Curated Thoughts' external generation provider assumes every OpenAI-compatible
gateway exposes the OpenAI `/v1` path layout. `HttpLlmCompleter` and
`GenerationProvider::route_info()` hard-append `/v1/chat/completions` to the
configured `external_url` (stripping an existing trailing `/v1` first):

- `src-tauri/src/inference/mod.rs` — `route_info()` (~L58–66), used by the
  `generate_text` Tauri command
- `src-tauri/src/librarian/synthesis.rs` — `build_llm_completer()` (~L221–238),
  used by librarian synthesis; the Sidecar arm builds
  `http://127.0.0.1:<port>/v1/chat/completions` and is out of scope here

Z.AI's GLM Coding Plan endpoint is versioned under `/api/coding/paas/v4` and
has **no `/v1` variant** (all `/v1` permutations return HTTP 404). Consequence:
the Kurt-approved provider (Z.AI `glm-5.3-flash`, the vendor approved for CT
librarian work) is unreachable from CT: config can name it, but every request
404s.

**Live verification (2026-08-28, this machine, the configured GLM key):**

| Probe | Result |
|---|---|
| `POST https://api.z.ai/api/coding/paas/v4/chat/completions`, model `glm-5.3-flash` | **200**, standard OpenAI shape, `choices[0].message.content` = `"ALIVE"` |
| `POST https://api.z.ai/api/coding/paas/v1/chat/completions` (what CT builds today) | **404** `{"status":404,"error":"Not Found","path":"/v1/chat/completions"}` |
| Response parse by CT's current extraction (`choices[0].message.content`) | Compatible — GLM 5.3 returns reasoning in a separate `reasoning_content` field; `content` carries the final answer |
| `max_tokens: 16` (low cap) | `content` empty — 45 reasoning tokens consumed the budget. CT sets no cap; non-issue for the fix, but a known pitfall |

Retirement of the previous provider makes this urgent rather than merely nice:
OpenRouter's `stealth/ox-alpha` (the current live config's model) began
returning 404 on ~2026-08-28, so CT generation is effectively broken today.

## Goal

CT can use any OpenAI-compatible gateway whose chat-completions path is
versioned differently from `/v1` — concretely, Z.AI's
`https://api.z.ai/api/coding/paas/v4` — by **config alone**, with zero
regression risk for existing `/v1`-style providers (OpenRouter, Ollama,
LM Studio, llama.cpp server, anygrasp…).

**Chosen approach (Kurt, Aug 28 2026, from a 3-option brainstorm):**
version-aware append — if the configured base already ends in a version
segment (`/v<digits>`), append only `/chat/completions`; otherwise append
`/v1/chat/completions` exactly as today.

Rejected alternatives (for the record):

- *Explicit config field* (e.g. `endpoint_path`): most explicit, but touches
  config schema, serde defaults, and both GUI screens for the same outcome.
- *Verbatim pass-through* (base ending in `/chat/completions` used as-is):
  smallest diff, but silently redefines `external_url` from "base URL" to
  "full endpoint", inviting misconfiguration.

## Non-goals

- No changes to the Sidecar URL logic (`http://127.0.0.1:<port>/v1/...` is
  llama-server's fixed contract).
- No config schema/migration changes; `external_url` keeps its meaning
  ("base URL, version-segment-aware").
- No GUI changes; the existing placeholder (`http://localhost:11434/v1`)
  remains a valid example.
- No provider allowlist; this is generic URL resolution, not vendor coupling.

## Proposed change

### 1. Shared resolution helper

New pure function in `src-tauri/src/inference/config.rs` (single source of
truth for both call sites):

```rust
/// Resolve the chat-completions endpoint for a configured external base URL.
///
/// If the base already ends in a version segment (`/v<digits>`), treat it as
/// the full path prefix and append only `/chat/completions`; otherwise fall
/// back to the historical `/v1/chat/completions` layout.
///
/// Version detection is STRICTLY `/v<digits>`: qualified versions like
/// `/v1.1` or `/v2_beta` deliberately do NOT match and take the legacy
/// `/v1/chat/completions` fallback. No known OpenAI-compatible gateway uses
/// minor/qualified version paths (industry standard is `/v1`, plus Z.AI's
/// `/v4`); if one ever appears, extend the matcher here rather than
/// special-casing call sites.
pub fn resolve_chat_completions_url(base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    let base = base.strip_suffix("/v1").unwrap_or(base);
    let versioned = base
        .rsplit_once("/v")
        .filter(|(_, tail)| !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()))
        .is_some();
    if versioned {
        format!("{base}/chat/completions")
    } else {
        format!("{base}/v1/chat/completions")
    }
}
```

Behavior contract:

| Configured `external_url` | Resolved endpoint |
|---|---|
| `https://api.z.ai/api/coding/paas/v4` | `https://api.z.ai/api/coding/paas/v4/chat/completions` |
| `https://openrouter.ai/api/v1` | `https://openrouter.ai/api/v1/chat/completions` (unchanged) |
| `https://openrouter.ai/api/v1/` (trailing slash) | `https://openrouter.ai/api/v1/chat/completions` (unchanged) |
| `http://localhost:11434` (no version) | `http://localhost:11434/v1/chat/completions` (unchanged) |
| `http://localhost:11434/v1` | `http://localhost:11434/v1/chat/completions` (unchanged) |
| `https://example.com/api/v2` (hypothetical) | `https://example.com/api/v2/chat/completions` |
| `https://example.com/v10` (multi-digit) | `https://example.com/v10/chat/completions` |

### 2. Call-site updates

Both external-provider URL constructions delegate to the helper; the strip-
suffix logic is deleted from both (it lives in the helper now):

- `inference/mod.rs` `route_info()` — `GenerationProvider::External` arm
- `librarian/synthesis.rs` `build_llm_completer()` — `External` arm

The Sidecar arms in both files keep their explicit
`/v1/chat/completions` strings.

### 3. Tests

**`inference/config.rs` `#[cfg(test)]`** — table-driven unit tests for the
helper covering every row of the behavior contract above, plus edge cases:

- multi-digit versions (`/v10`, `/v99`)
- single-digit boundary: `/v9` versioned; `/v` or `/vX` NOT versioned
- qualified versions do NOT match (Kurt, review round 1): `/v1.1`,
  `/v2_beta` → legacy `/v1/chat/completions` append — deliberate fallback,
  documented in the function's doc-comment
- path containing a version segment that is NOT trailing (e.g.
  `https://host/v1/models`): resolves to `https://host/v1/models/v1/chat/completions`
  — today's behavior, deliberately preserved (see Open questions Q3)
- whitespace-only / empty string → falls through to
  `/v1/chat/completions` (matches today's behavior for degenerate input;
  callers already reject empty URLs before resolution)

**`inference/mod.rs` tests** — keep the existing three `route_info()` tests
passing unchanged (they assert exactly the contract table's Ollama rows); add
one test: base `…/paas/v4` → `…/paas/v4/chat/completions`.

**`librarian/synthesis.rs`** — no new test required by this spec (the
`build_llm_completer` External arm has no existing direct test and constructing
one requires a brain-dir fixture; the helper's unit tests plus the
`route_info()` test cover the resolution logic it delegates to). **(see Open
questions Q4)**

## Compatibility & risk

- Every existing config resolves identically: `/v1`-suffixed and bare-host
  bases take the same path they always did. The only behavior change is for
  bases ending in a non-`v1` version segment — which today can only produce
  broken URLs (`https://host/api/paas/v4/v1/chat/completions`), so nothing
  that works today can break.
- Worst-case false positive: a provider whose real path is
  `/something/v3` where `/something/v3/v1/chat/completions` is correct. No
  known OpenAI-compatible gateway has this shape; the failure mode is a 404
  visible in the librarian error log, recoverable by config tweak.
- No migration, no serde changes, no GUI changes, no dependency changes.

## Rollout

1. Spec review rounds (this document) → Kurt approves.
2. Implementation plan at `docs/superpowers/plans/` (gitignored working doc).
3. Single PR: helper + two call sites + tests (est. <60 LOC excluding tests).
   CI must pass the full suite (`cargo test … --features test-utils,mcp-server
   -- --test-threads=1`) plus frontend checks.
4. Kurt merges (regular merge commit, no squash).
5. Post-merge: install new binary (Kurt prefers CI-built artifacts);
   update `~/.brain-equational-wiki/config.json` generation block to
   `external_url: https://api.z.ai/api/coding/paas/v4`,
   `model_name: glm-5.3-flash`, key from Hermes `GLM_API_KEY` (copy
   programmatically, never echo). Mirror the same generation block into
   `~/.brain/config.json` (both files need complete sections or
   `read_config` silently fails).
6. End-to-end verify: librarian synthesis over the live vault produces real
   model output; no 404s in the error log.

Steps 5–6 touch production config and happen only after the merged build is
installed, at Kurt's go.

## Open questions

- **Q1 — helper name:** `resolve_chat_completions_url` vs
  `chat_completions_endpoint`. Leaning toward the first (states what it
  returns).
- **Q2 — helper location:** `inference/config.rs` (config semantics) vs a
  new `inference/url.rs`. Leaning toward `config.rs` — it already holds
  `GenerationConfig`; no new module for one function.
- **Q3 — `/v1/models`-style bases:** a base ending in a non-version path
  segment after a version segment (e.g. `https://host/v1/models`) currently
  resolves to `https://host/v1/models/v1/chat/completions` (today's behavior,
  preserved by the helper). Keeping today's behavior for these degenerate
  configs seems right; flagging in case Kurt wants them rejected instead.
- **Q4 — synthesis test:** add a brain-dir-fixture test for the
  `build_llm_completer` External arm (stronger, more setup) or rely on the
  helper unit tests + `route_info()` test (weaker coupling, zero new
  fixtures)? Leaning toward rely-on-helper + route_info, per spec above.
