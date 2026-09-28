# jev_http `model` field (issue #230) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the `jev_http` classifier provider send the TypeSafe-required `model` field (default `jev-latest`, user-pinnable), with merge-safe config persistence so panel saves never wipe a hand-pinned model and clearing the field unpins it.

**Architecture:** Backend-only envelope change in `request_body` + a new optional `model` field on `ClassifierConfig`; persistence goes through a new pure `merge_stored(incoming, stored)` helper called by BOTH the Tauri command and its test (closing the mirror-only test blind spot); the panel gains a `model` input shown only for `jev_http`, sends `model.trim()` as typed (empty string = explicit unpin), and normalizes nothing itself — blank→`None` happens inside `merge_stored`.

**Tech Stack:** Rust (src-tauri, serde_json, mockito for the HTTP-seam test), React/TS (ClassifierPanel + vitest).

**Spec:** `docs/superpowers/specs/2026-09-28-issue230-jev-http-model-design.md` (companion investigation: `2026-09-28-issue230-jev-http-model-investigation.md`)

## Global Constraints

- `JEV_HTTP_MODEL: &str = "jev-latest"` — doc-commented to distinguish from `JEV_MODEL` (`typesafe/jev`, Cloudflare envelope id).
- Blank/whitespace model NEVER reaches the wire as `"model": ""` — always `jev-latest`.
- Panel sends `model.trim()` AS TYPED for `jev_http` (`""` = explicit unpin); `null` for non-jev_http providers (never `undefined`).
- `null`/omitted `model` = untouched (merge stored); normalization to `None` happens backend-side AFTER the merge, inside `merge_stored`.
- `set_classifier_config` merge is unconditional (read stored once, apply per-field `None` merges) — the stored-read must NOT sit inside the `api_key.is_none()` block.
- Existing exact-match panel assertion (`ClassifierPanel.test.tsx:37-44` region) is UPDATED in this PR (`model: null` now present in payloads; `toHaveBeenCalledWith` distinguishes `null` from missing).
- `#[serde(default)]` on the new field — existing configs deserialize unchanged.
- Run Rust tests via `cargo test -p curated-thoughts classifier`; frontend via `pnpm test` (vitest run).
- Conventional commits; all work lands on this branch (`fix/issue-230-jev-http-model`), one PR (#246).

---

### Task 1: `model` field on `ClassifierConfig` + `request_body` JevHttp envelope

**Files:**
- Modify: `src-tauri/src/inference/classifier.rs` (consts ~:18-23, struct :25-56, `request_body` :299-305, tests :548-565, matcher :643-645)

**Interfaces:**
- Consumes: existing `ClassifierConfig`, `request_body(&ClassifierConfig, &ClassifyRequest) -> Value`, test helpers `jev_cfg(url)` / `choice_req()`.
- Produces: `pub const JEV_HTTP_MODEL: &str = "jev-latest"`; `ClassifierConfig.model: Option<String>` (serde default); `request_body` JevHttp arm injects `inner["model"]`.

- [ ] **Step 1: Write the failing tests** (in `mod tests`, after `cloudflare_body_wraps_input_and_names_model`)

```rust
    #[test]
    fn jev_http_body_names_model_default() {
        let body = request_body(&jev_cfg("https://x"), &choice_req());
        assert_eq!(body["model"], JEV_HTTP_MODEL);
        assert_eq!(body["state"], "Alice is a person.");
    }

    #[test]
    fn jev_http_body_uses_pinned_model() {
        let cfg = ClassifierConfig {
            model: Some("jev-1.13".into()),
            ..jev_cfg("https://x")
        };
        assert_eq!(request_body(&cfg, &choice_req())["model"], "jev-1.13");
    }

    #[test]
    fn jev_http_blank_model_falls_back_to_default() {
        for blank in [Some(String::new()), Some("   ".into())] {
            let cfg = ClassifierConfig { model: blank, ..jev_cfg("https://x") };
            assert_eq!(request_body(&cfg, &choice_req())["model"], JEV_HTTP_MODEL);
        }
    }
```

- [ ] **Step 2: Flip the contradicting assertion** in `cloudflare_body_wraps_input_and_names_model` (:562-564) — replace

```rust
        assert!(request_body(&jev_cfg("https://x"), &choice_req())
            .get("model")
            .is_none());
```

with

```rust
        assert_eq!(
            request_body(&jev_cfg("https://x"), &choice_req())["model"],
            JEV_HTTP_MODEL
        );
```

- [ ] **Step 3: Run to verify RED**

Run: `cargo test -p curated-thoughts classifier`
Expected: the three new tests FAIL to compile or assert (`model` field doesn't exist; body has no `model`).

- [ ] **Step 4: Minimal implementation**

Next to `JEV_MODEL` (:22):

```rust
/// Model id for the Cloudflare envelope (`typesafe/jev`). Distinct from
/// [`JEV_HTTP_MODEL`], which is the TypeSafe-hosted default for the
/// `jev_http` provider's flat envelope.
pub const JEV_MODEL: &str = "typesafe/jev";
/// Default `model` for the `jev_http` (TypeSafe-hosted) request body. The
/// API rejects requests without it (HTTP 422); users may pin an exact
/// version via `ClassifierConfig::model` for reproducible typing.
pub const JEV_HTTP_MODEL: &str = "jev-latest";
```

Struct field (after `timeout_secs`, keep `#[serde(default)]`):

```rust
    /// Optional model pin for `jev_http` (e.g. `jev-1.13`). `None` or blank
    /// falls back to [`JEV_HTTP_MODEL`]. Ignored by other providers.
    #[serde(default)]
    pub model: Option<String>,
```

`request_body` (:299-305) becomes:

```rust
pub fn request_body(cfg: &ClassifierConfig, req: &ClassifyRequest) -> Value {
    let mut inner = json!({ "state": req.state, "questions": to_jev_questions(&req.questions) });
    match cfg.provider {
        ClassifierProviderKind::CloudflareJev => json!({ "model": JEV_MODEL, "input": inner }),
        _ => {
            let model = cfg
                .model
                .as_deref()
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .unwrap_or(JEV_HTTP_MODEL);
            inner["model"] = json!(model);
            inner
        }
    }
}
```

- [ ] **Step 5: Strengthen the HTTP-seam matcher** — in `round_trips_over_http_with_bearer_key` (:643-645) change

```rust
            .match_body(mockito::Matcher::PartialJson(
                json!({ "state": "Alice is a person." }),
            ))
```

to

```rust
            .match_body(mockito::Matcher::PartialJson(json!({
                "state": "Alice is a person.",
                "model": JEV_HTTP_MODEL,
            })))
```

- [ ] **Step 6: Run to verify GREEN**

Run: `cargo test -p curated-thoughts classifier`
Expected: ALL PASS (including the flipped Cloudflare assertion).

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/inference/classifier.rs
git commit -m "fix(inference): jev_http request body names the required model field (#230)"
```

### Task 2: `merge_stored` helper + unconditional merge in `set_classifier_config`

**Files:**
- Modify: `src-tauri/src/inference/classifier.rs` (`set_classifier_config` :585-606; new helper + tests in `mod tests`)

**Interfaces:**
- Consumes: `ClassifierConfig` (Task 1), `read_classifier_config`.
- Produces: `pub(crate) fn merge_stored(incoming: ClassifierConfig, stored: Option<ClassifierConfig>) -> ClassifierConfig` — PURE (no I/O), testable; semantics: for `api_key` and `model`, a `None` in `incoming` takes the stored value; blank/whitespace `model` (after merge) normalizes to `None`; every other field passes `incoming` through untouched.

- [ ] **Step 1: Write the failing tests** (in `mod tests`, near the :807 merge test)

```rust
    fn stored_cfg() -> ClassifierConfig {
        ClassifierConfig {
            provider: ClassifierProviderKind::JevHttp,
            url: Some("https://x".into()),
            model: Some("jev-1.13".into()),
            ..Default::default()
        }
    }

    #[test]
    fn merge_stored_none_model_keeps_stored_pin() {
        let merged = merge_stored(
            ClassifierConfig { model: None, ..stored_cfg() },
            Some(stored_cfg()),
        );
        assert_eq!(merged.model.as_deref(), Some("jev-1.13"));
    }

    #[test]
    fn merge_stored_blank_model_unpins_after_merge() {
        let merged = merge_stored(
            ClassifierConfig { model: Some("".into()), ..stored_cfg() },
            Some(stored_cfg()),
        );
        assert_eq!(merged.model, None);
        let merged = merge_stored(
            ClassifierConfig { model: Some("  ".into()), ..stored_cfg() },
            Some(stored_cfg()),
        );
        assert_eq!(merged.model, None);
    }

    #[test]
    fn merge_stored_explicit_pin_replaces_stored() {
        let merged = merge_stored(
            ClassifierConfig { model: Some("jev-1.12".into()), ..stored_cfg() },
            Some(stored_cfg()),
        );
        assert_eq!(merged.model.as_deref(), Some("jev-1.12"));
    }

    #[test]
    fn merge_stored_without_stored_keeps_incoming() {
        let incoming = ClassifierConfig { model: Some("jev-1.12".into()), ..stored_cfg() };
        let merged = merge_stored(incoming.clone(), None);
        assert_eq!(merged, incoming);
    }

    #[test]
    fn merge_stored_null_api_key_still_takes_stored_key() {
        let mut stored = stored_cfg();
        stored.api_key = Some("tok".into());
        let merged = merge_stored(
            ClassifierConfig { api_key: None, model: None, ..stored_cfg() },
            Some(stored),
        );
        assert_eq!(merged.api_key.as_deref(), Some("tok"));
        assert_eq!(merged.model.as_deref(), Some("jev-1.13"));
    }
```

- [ ] **Step 2: Run to verify RED**

Run: `cargo test -p curated-thoughts classifier`
Expected: compile failure (`merge_stored` not found).

- [ ] **Step 3: Implement `merge_stored`** (place above `set_classifier_config`; NOT inside `mod tests` — the command calls it)

```rust
/// Merge an incoming save payload with the stored config. `None` in
/// `incoming` means "untouched — keep stored" for `api_key` and `model`;
/// a blank/whitespace `model` normalizes to `None` AFTER the merge so an
/// explicit unpin (`""` from the panel) replaces a stored pin but never
/// persists blank. All other fields pass `incoming` through untouched.
/// Pure helper shared by `set_classifier_config` and its tests — the
/// merge logic exists exactly once.
pub(crate) fn merge_stored(
    mut incoming: ClassifierConfig,
    stored: Option<ClassifierConfig>,
) -> ClassifierConfig {
    if let Some(stored) = stored {
        if incoming.api_key.is_none() {
            incoming.api_key = stored.api_key;
        }
        if incoming.model.is_none() {
            incoming.model = stored.model;
        }
    }
    if let Some(m) = incoming.model.as_deref().map(str::trim) {
        if m.is_empty() {
            incoming.model = None;
        }
    }
    incoming
}
```

- [ ] **Step 4: Rewire `set_classifier_config`** (:585-606) — the stored read moves OUT of the `api_key.is_none()` block:

```rust
#[tauri::command]
pub fn set_classifier_config(
    config: ClassifierConfig,
    app: tauri::AppHandle,
) -> Result<(), String> {
    use tauri::Emitter;
    let store = super::classifier_secrets::KeyringClassifierSecretStore;
    let (_, paths) = current_brain();
    // Unconditional merge: `None` fields (api_key, model) take the stored
    // value so saving one field never wipes another; blank `model` unpins
    // (normalization happens inside merge_stored, after the merge).
    let stored = read_classifier_config(&paths, &store).ok();
    let merged = merge_stored(config, stored);
    write_classifier_config(&paths, &merged, &store).map_err(|e| e.to_string())?;
    let _ = app.emit("classifier-config-changed", ());
    Ok(())
}
```

- [ ] **Step 5: Convert the existing mirror-test** `set_classifier_config_merges_existing_key_when_payload_says_null` (:708-711) — replace the inline merge lines

```rust
        // Mirror the Tauri-command merge.
        if payload.api_key.is_none() {
            payload.api_key = read_classifier_config(&paths, &store).unwrap().api_key;
        }
```

with

```rust
        // Same helper the Tauri command calls — no mirror to drift.
        let stored = read_classifier_config(&paths, &store).ok();
        let payload = merge_stored(payload, stored);
```

- [ ] **Step 6: Run to verify GREEN**

Run: `cargo test -p curated-thoughts classifier`
Expected: ALL PASS (new merge tests + converted mirror test + Task 1 tests).

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/inference/classifier.rs
git commit -m "feat(inference): merge_stored helper for classifier config saves (#230)"
```

### Task 3: Panel `model` field + payload type + test updates

**Files:**
- Modify: `src/lib/tauri.ts:598-622` (`ClassifierConfig` interface)
- Modify: `src/components/settings/ClassifierPanel.tsx` (state block :30-40, load effect :46-69, `persistConfig` :80-100, JSX :133-144 region)
- Modify: `src/components/settings/__tests__/ClassifierPanel.test.tsx` (:31-45 exact-match test + new tests)

**Interfaces:**
- Consumes: backend Task 1-2 (`model` accepted, blank = unpin, null = untouched).
- Produces: `ClassifierConfig.model?: string | null` TS field; panel payload always carries `model` (`string | null`, never `undefined`).

- [ ] **Step 1: Update the TS type** — in `ClassifierConfig` (tauri.ts:364-376) add after `timeout_secs`:

```ts
  /** Model pin for jev_http (e.g. "jev-1.13"). Send a non-empty string to
   * pin, "" to unpin, null/omit to leave the stored pin untouched. Other
   * providers always send null. */
  model?: string | null;
```

- [ ] **Step 2: Update the exact-match test FIRST (RED)** — in `ClassifierPanel.test.tsx` the `:31-45` test's expected payload gains `model: null`:

```tsx
    expect(setClassifierConfig).toHaveBeenCalledWith({
      provider: 'cloudflare_jev',
      url: null,
      account_id: 'abc123',
      api_key: 'tok',
      min_confidence: 0.5,
      timeout_secs: null,
      model: null,
    });
```

Run: `pnpm test -- ClassifierPanel`
Expected: FAIL (panel doesn't send `model` yet).

- [ ] **Step 3: Panel state + load hydration** — state block (:32-40) gains:

```tsx
  const [model, setModel] = useState('');
```

Load effect `.then` block (:52-58) gains (after `setTimeoutSecs(...)`):

```tsx
        setModel(cfg.model ?? '');
```

- [ ] **Step 4: persistConfig sends the field** — the `setClassifierConfig({...})` payload (:84-91) gains one line (after `timeout_secs`):

```tsx
        model: provider === 'jev_http' ? model.trim() : null,
```

(Note: `""` travels as typed — that is the unpin signal; the backend normalizes. No panel-side blank→null normalization: it would re-merge the stored pin and make unpinning impossible.)

- [ ] **Step 5: JSX field** — inside the `provider === 'jev_http'` fragment (:133-138), after the Endpoint URL div, add:

```tsx
          <div>
            <label htmlFor="classifier-model">Model (optional)</label>
            <input
              id="classifier-model"
              type="text"
              value={model}
              placeholder="jev-latest"
              disabled={disableControls}
              onChange={(e) => setModel(e.target.value)}
            />
            <p className="settings-form__hint">
              Pin an exact model (e.g. jev-1.13) for reproducible typing; blank
              uses the default (jev-latest).
            </p>
          </div>
```

- [ ] **Step 6: New panel tests** (in `ClassifierPanel.test.tsx`, after the clear-token test):

```tsx
  it('sends model as typed for jev_http (blank = explicit unpin)', async () => {
    getClassifierConfig.mockResolvedValue({
      provider: 'jev_http', url: 'https://x', model: 'jev-1.13', has_api_key: true,
    });
    render(<ClassifierPanel />);
    const modelInput = await screen.findByLabelText('Model (optional)');
    expect(modelInput).toHaveValue('jev-1.13');
    await userEvent.clear(modelInput);
    await userEvent.click(screen.getByRole('button', { name: 'Save classifier' }));
    await waitFor(() =>
      expect(setClassifierConfig).toHaveBeenCalledWith(
        expect.objectContaining({ model: '' }),
      ),
    );
  });

  it('sends model: null for non-jev_http providers', async () => {
    render(<ClassifierPanel />);
    await userEvent.selectOptions(await screen.findByLabelText('Classifier provider'), 'cloudflare_jev');
    await userEvent.type(screen.getByLabelText('Cloudflare account ID'), 'abc123');
    await userEvent.type(screen.getByLabelText('API token'), 'tok');
    await userEvent.click(screen.getByRole('button', { name: 'Save classifier' }));
    expect(setClassifierConfig).toHaveBeenCalledWith(
      expect.objectContaining({ model: null }),
    );
  });
```

- [ ] **Step 7: Run to verify GREEN (full suites)**

Run: `pnpm test -- ClassifierPanel && cargo test -p curated-thoughts classifier`
Expected: ALL PASS.

- [ ] **Step 8: Commit**

```bash
git add src/lib/tauri.ts src/components/settings/ClassifierPanel.tsx src/components/settings/__tests__/ClassifierPanel.test.tsx
git commit -m "feat(ui): classifier model pin field for jev_http (issue #230)"
```

### Task 4: Spec flip + final verification

**Files:**
- Modify: `docs/superpowers/specs/2026-09-28-issue230-jev-http-model-design.md` (header)

- [ ] **Step 1: Flip the spec status line** to `**Status:** Implemented 2026-09-28 (PR #246)`.

- [ ] **Step 2: Full local verification (CI parity)**

Run: `cargo test -p curated-thoughts classifier && pnpm test && pnpm run build`
Expected: all green.

- [ ] **Step 3: Commit + push**

```bash
git add docs/superpowers/specs/2026-09-28-issue230-jev-http-model-design.md
git commit -m "docs(spec): mark #230 design implemented (PR #246)"
git push origin fix/issue-230-jev-http-model
```
