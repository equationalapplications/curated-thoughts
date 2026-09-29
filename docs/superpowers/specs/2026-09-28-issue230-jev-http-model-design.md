# Jev HTTP classifier: send the required `model` field (issue #230)

**Date:** 2026-09-28
**Status:** Implemented 2026-09-28 (PR #246)
**Branch:** fix/issue-230-jev-http-model
**Priority:** High (bug — the documented TypeSafe-hosted path cannot work at all)

## Problem

The `jev_http` classifier provider cannot talk to TypeSafe's hosted API: its
request body omits the required top-level `model` field, so every classify
call returns HTTP 422. Evidence: the issue's live curl (422 without / 200 with
`"model": "jev-latest"`; the 200 body already parses via the existing
`from_jev_response()`), corroborated locally by the HTTP mock echoing
`"model": "jev-1.13.0"` (`classifier.rs:662`). Code: `request_body`
(`src-tauri/src/inference/classifier.rs:299-305`) only wraps the Cloudflare
arm in an envelope; the JevHttp arm forwards the inner body bare. A unit test
even pins the omission as desired behavior (`classifier.rs:562-564`).

Full investigation (verified current state, root cause, Opus verdict history):
`2026-09-28-issue230-jev-http-model-investigation.md` (same directory).

## Approach

1. `JEV_HTTP_MODEL: &str = "jev-latest"` const next to `JEV_MODEL`, both
   doc-commented to disambiguate (Cloudflare envelope id `typesafe/jev` vs
   TypeSafe-hosted default `jev-latest`).
2. `#[serde(default)] pub model: Option<String>` on `ClassifierConfig`
   (backward compatible; `api_key` is `#[serde(skip)]`, rest default).
3. `request_body` JevHttp arm (compiling form from the investigation):
   `let mut inner`, then `cfg.model.as_deref().map(str::trim).filter(|m|
   !m.is_empty()).unwrap_or(JEV_HTTP_MODEL)` injected as `inner["model"]`.
   Blank/whitespace → default (never `"model": ""`).
4. Config merge: `set_classifier_config` (`classifier.rs:478-498`) merges
   `model` from stored config when incoming is `None` — panel saves must not
   wipe a hand-pinned model. **Placement (Opus spec M2):** the stored-config
   read moves OUT of the `api_key.is_none()` block — a single
   `merge_stored(incoming, stored)` pass applied unconditionally (api_key
   merged only on `None`, model merged only on `None`), or a save carrying a
   new key + `model: null` still wipes the pin. **Unpin rule (Opus spec c2
   Critical — supersedes any panel-side normalization):** the panel sends
   `model.trim()` AS TYPED — empty string = explicit unpin, null/omitted =
   untouched; the BLANK→default normalization happens BACKEND-SIDE only,
   AFTER the merge (merge `Some("")` past the stored pin, then map blank to
   `None` before persist — `request_body`'s blank-fallback then yields
   `jev-latest`). Panel-side blank→null normalization would re-merge the
   stored pin and make the pin impossible to remove from the UI. **Provider
   rule (Opus spec M1):** non-jev_http providers ALWAYS send `model: null`
   (never `undefined`) so the payload shape is deterministic; the existing
   exact-match panel assertion at `ClassifierPanel.test.tsx:37-44` is UPDATED
   in the same commit (c2 Major: `toHaveBeenCalledWith` distinguishes
   `null` from missing). **Normalization home (Opus c3 nit ①):** the
   blank→`None` normalization lives INSIDE `merge_stored` (the pure helper
   both the command and the test call) — not duplicated in the command body —
   so the OQ2 test blind spot cannot reopen. **Load race (Opus c3 nit ②):**
   the panel's save button is disabled until the load effect has resolved
   (guarded by the same `status !== 'idle'` gate the panel already uses), so
   a save can never fire with unpinned state and overwrite a stored pin.
5. ClassifierPanel: `model` field shown only for `jev_http`; load effect
   hydrates `setModel(cfg.model ?? '')`; payload type in `src/lib/tauri.ts`
   gains `model?: string | null` (m3).

**Rejected alternatives:** Cloudflare-only fix (leaves the documented hosted
path broken); making the panel field write-only-null with no merge (wipes
pins on every save — Opus c1 M3); naive `cfg.model.unwrap_or(...)` (does not
compile: E0507/E0308 — Opus c1 M1).

## Error handling

No new error paths: the field defaults, never fails validation. Backend
blank-fallback means no invalid states reach the HTTP layer.

## Testing

- Flip the :562-564 assertion; `jev_http_body_names_model` (default
  `jev-latest`); pinned `cfg.model` case; `Some("")` → `jev-latest` fallback.
- HTTP seam: add `"model": "jev-latest"` to the `PartialJson` matcher in
  `round_trips_over_http_with_bearer_key` (:657-659).
- Rust merge test mirroring `set_classifier_config_merges_existing_key_when_payload_says_null`
  (:812) — prefer extracting a pure `merge_stored(incoming, stored)` helper
  used by both command and test (the mirror-only test cannot catch missing
  wiring; see investigation OQ2).
- Panel tests: clear→`""` sends `model: ""` and the SAVED CONFIG shows the
  pin removed (backend-verifiable via `read_classifier_config`; the panel
  assertion alone is only wire-level — Opus c3 nit ③); hydration on load;
  non-jev_http providers send `model: null` — the
  `:37-44` exact-match assertion is UPDATED accordingly (null ≠ missing).
- Rust: `set_classifier_config` merge tests — `Some("")` unpin persists
  `None` (blank normalization backend-side, after merge); `Some("  ")`
  whitespace-only also normalizes to `None` (trim before the blank check);
  `None` keeps the stored pin.
- `cargo test -p curated-thoughts classifier` + `pnpm test` (ClassifierPanel).

## Out of scope / open questions

- Live end-to-end verification against api.typesafe.ai needs a key — lands on
  Kurt after merge (setup notes in the investigation doc).
- Third-party jev_http servers rejecting unknown fields: accepted risk
  (investigation "What was NOT checked"); `model` is the documented TypeSafe
  contract.
- Bad pins (m5): a hand-typed model name that the API rejects yields 422 →
  classify is skipped for that pass; acceptable (same failure mode as a bad
  endpoint URL), no extra validation in v1.
- `_` match arm also covers `Unconfigured` (m4) — unreachable via
  `is_available` (see investigation); no code change.

Fixes #230.
