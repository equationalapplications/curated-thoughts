# Investigation — issue #230: jev_http omits required `model` field (TypeSafe 422)

**Date:** 2026-09-28
**Status:** Investigation (Step 0 of the delivery flow)
**Issue:** #230 (bug, rust)
**Evidence tags:** [V] = controller-verified; [C] = child-reported

## Repro of the reported behavior

The issue's curl repro (exact `jev_http` body shape against
`https://api.typesafe.ai/v1/systemone` with a valid key) returned HTTP 422
`{"detail":[{"type":"missing","loc":["body","model"],"msg":"Field required",…}]}`;
adding `"model": "jev-latest"` returned HTTP 200 with a body that
`from_jev_response()` already parses. The reporter ran this live with a valid
key on 2026-09-28; I have no TypeSafe key on this machine, so the live HTTP
round-trip is [C] (issue-reported, with exact request/response bodies quoted).
The code-side omission is [V] below and is independently sufficient.

## Code-level current state [V]

All verified at repo `eab433f`.

**The bug:** `src-tauri/src/inference/classifier.rs:299-305`:

```rust
pub fn request_body(cfg: &ClassifierConfig, req: &ClassifyRequest) -> Value {
    let inner = json!({ "state": req.state, "questions": to_jev_questions(&req.questions) });
    match cfg.provider {
        ClassifierProviderKind::CloudflareJev => json!({ "model": JEV_MODEL, "input": inner }),
        _ => inner,   // JevHttp: no "model" — the 422
    }
}
```

- `JEV_MODEL` const lives at `classifier.rs:22` (Cloudflare arm only).
- The `_` arm also matches `Unconfigured`, but that cannot reach `request_body`:
  `classify_with` → `is_available` → `endpoint()` (`classifier.rs:239` region)
  bails first. JevHttp is the only arm that matters at runtime.
- `ClassifierConfig` (`classifier.rs:34-56`) is deserialized from the
  `classifier` preserved key of brain `config.json` via `read_classifier_config`
  (`classifier.rs:134`, `serde_json::from_value`). Fields default sensibly
  (`api_key` is `#[serde(skip)]`, the rest `#[serde(default)]`) → adding
  `model: Option<String>` with `#[serde(default)]` is backward compatible with
  existing configs.
- Frontend: `src/components/settings/ClassifierPanel.tsx:66-71` builds the
  config object; tests at `src/components/settings/__tests__/ClassifierPanel.test.tsx`.

**Existing tests [V] — one must CHANGE, the issue does not mention this:**
`mod tests` at `classifier.rs:500`. `cloudflare_body_wraps_input_and_names_model`
(`classifier.rs:548`) ends with an assertion of the OPPOSITE of the fix at
lines 562-564:

```rust
assert!(request_body(&jev_cfg("https://x"), &choice_req()).get("model").is_none());
```

This documents today's omission as desired behavior; it must be flipped/replaced
in the same commit or CI goes red.

**Test infra [V]:** `cargo test -p curated-thoughts classifier` from workspace root.

## Root cause [V for the code omission; C for the server requirement]

`request_body` was written against the Cloudflare Jev contract (model in the
outer envelope, `input` nested) and the JevHttp arm simply forwards the inner
body. That the JevHttp/TypeSafe endpoint requires `model` is evidenced by the
issue's live curl (422 without / 200 with — [C], no key on this machine), with
local corroboration: the existing HTTP mock already echoes
`"model": "jev-1.13.0"` (`classifier.rs:662`), i.e. the response contract
assumed a model all along. The two providers share `to_jev_questions` /
`from_jev_response` but not the envelope, and only the Cloudflare envelope was
implemented.

## Proposed fix direction

- `JEV_HTTP_MODEL: &str = "jev-latest"` const next to `JEV_MODEL`
  (TypeSafe's documented default; quickstart + SDK constants per the issue's
  links). Doc-comment both consts distinguishing them: `JEV_MODEL` is the
  Cloudflare envelope's model id (`typesafe/jev`); `JEV_HTTP_MODEL` is the
  TypeSafe-hosted default (`jev-latest`) — confusable names, so the comments
  are mandatory.
- Optional `#[serde(default)] pub model: Option<String>` on `ClassifierConfig`
  so users can pin (e.g. `jev-1.13`) for reproducible typing.
- `request_body` JevHttp arm — **compiling form** (the naive
  `cfg.model.unwrap_or(...)` both moves out of a borrow (E0507) and type-mismatches
  (E0308), and `inner` is immutably bound):

```rust
let mut inner = json!({ "state": req.state, "questions": to_jev_questions(&req.questions) });
match cfg.provider {
    ClassifierProviderKind::CloudflareJev => json!({ "model": JEV_MODEL, "input": inner }),
    _ => {
        // Blank/whitespace model (cleared panel field) falls back to the
        // default rather than shipping "model": "".
        let model = cfg.model.as_deref().map(str::trim).filter(|m| !m.is_empty())
            .unwrap_or(JEV_HTTP_MODEL);
        inner["model"] = json!(model);
        inner
    }
}
```

- **Panel/config persistence (decision):** add the `model` field to
  ClassifierPanel (shown only for `jev_http`), AND make `set_classifier_config`
  (`classifier.rs:478-498`) merge `model` from the stored config when the
  incoming value is `None` — `persistConfig` (`ClassifierPanel.tsx:65-72`)
  sends the whole config object, so without the merge any panel save would
  silently wipe a hand-pinned model.
- **Wire rule (Opus c2, panel-unpin gap):** the panel must NOT copy the
  `apiKey.trim() || null` idiom (`ClassifierPanel.tsx:84`) for `model` — that
  would make clearing the field send `null`, which the None-merge folds back
  into the stored pin, so the pin could never be removed from the UI. Rule:
  when `provider === 'jev_http'`, the panel sends `model.trim()` as a STRING
  (empty string = explicit unpin → backend blank-fallback → `jev-latest`);
  `null`/omitted = untouched (merge stored). The load effect (~:40-45) must
  hydrate `setModel(cfg.model ?? '')`. Tests: panel clear→`""` saves and the
  pin is gone; Rust `None`-merge test mirroring
  `set_classifier_config_merges_existing_key_when_payload_says_null` (:812).
- Flip the :562-564 assertion; add `jev_http_body_names_model` (default), a
  pinned-`cfg.model` case, and a `Some("")` → `jev-latest` fallback case.
  Strengthen the HTTP round-trip: add `"model": "jev-latest"` to the
  `PartialJson` matcher in `round_trips_over_http_with_bearer_key`
  (`classifier.rs:657-659`) so the envelope change is pinned at the HTTP seam,
  not just the body builder.
- Setup note for Kurt once fixed (from the issue): Settings → Models → Classifier
  → Jev-compatible endpoint, URL `https://api.typesafe.ai/v1/systemone`,
  API token from console.typesafe.ai/keys; privacy mode Ephemeral/Connected
  (Strict blocks the classifier).

## Open questions

- **OQ1 (live verification):** no TypeSafe key here, so end-to-end verification
  (the curl in the issue, then the app) lands on Kurt or a later session with a
  key. Unit tests pin the request shape; the issue already proved the shape
  round-trips with the existing response parser.
- **OQ2 (merge-test blind spot, Opus c3 NIT):** the None-merge test modeled on
  `set_classifier_config_merges_existing_key_when_payload_says_null`
  (`classifier.rs:812`) mirrors the merge in the TEST BODY (:844-847) because
  the command needs an `AppHandle` — it can pass while `set_classifier_config`
  (:489-494) lacks the merge. The plan should extract a pure
  `merge_stored(incoming, stored)` helper called by BOTH the command and the
  test; if that extraction is out of scope, the doc note stands: the test only
  proves the merge shape, not its wiring.
- **OQ3 (payload type, Opus c3 observation):** the panel payload type in
  `src/lib/tauri.ts` needs `model?: string | null`; `tsc` will catch the
  omission if missed.

## What was NOT checked

- Whether third-party "Jev-compatible" servers (the `jev_http` provider accepts
  any endpoint URL, `classifier.rs:4-5`) tolerate an added top-level `model`
  field. Strict servers that reject unknown fields would break; the panel field
  cannot be emptied to escape the default (blank → `jev-latest`), so such a
  server would need an upstream code change. Accepted risk: TypeSafe-hosted is
  the documented target of this provider.
