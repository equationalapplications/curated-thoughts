# ct release asset (#244) + drift sweep (#241) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the `ct` headless CLI as a per-platform release asset in the Build workflow (closing the stale-local-CLI trap behind the Sep 27 incident), add deletion-is-self-cleaning docs, and add a read-only `ct drift` safety net driven by a classify/apply split of `reconcile_vault` — with the tauri-action tag gate that makes the whole thing CI-verifiable.

**Architecture:** `build.yml` gains a "Build ct CLI" step per matrix leg (version from the stamped `tauri.conf.json`, `gh release upload` on tags only) and a `github.ref_type == 'tag'` gate on tauri-action (BLOCKER fix: today a dispatch run uploads installers onto the LIVE release). `reconcile_vault` splits into `classify_vault` (pure plan) + `reconcile_vault` (classify + apply); a shared walk helper feeds both `ct ingest` and the new `ct drift` subcommand, so drift can never disagree with what ingest would repair. Drift reports the FULL plan (gone + repointed + excluded-deletes), treats ambiguous rows as warnings (exit 0), and exits 3 on any pending repair, 4 on the unmounted-vault guard.

**Tech Stack:** GitHub Actions (bash, `pwsh Compress-Archive` on Windows), Rust (`src-tauri` reconcile + `tools` crate CLI), no frontend changes.

**Spec:** `docs/superpowers/specs/2026-09-28-issue244-241-ct-release-and-drift-design.md` (companion investigation: `2026-09-28-issue244-241-ct-release-and-drift-investigation.md`)

## Global Constraints

- Version derivation: ALWAYS from the stamped `src-tauri/tauri.conf.json` (jq). On tag refs additionally assert it equals `${GITHUB_REF_NAME#v}`; never use `GITHUB_REF_NAME` raw (it is the branch name on dispatch runs; `/` corrupts filenames).
- tauri-action step gains `if: github.ref_type == 'tag'` (BLOCKER fix); the `ct` upload step also runs only on tags and needs explicit `env: GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}` (checkout has `persist-credentials: false`).
- Upload glob is `ct_*.*` (Windows leg ships `.zip` via `pwsh Compress-Archive`; Git Bash has no `zip`).
- `ct drift` is READ-ONLY: `open_ro` only, never repairs (repair = `ct ingest --yes`; the desktop startup path is NOT an equivalent repair — it deletes where ingest repoints).
- Drift consumes the IDENTICAL file list `ct ingest` passes to reconcile (shared walk helper: symlink-trust re-walk, sorted, deduped by `virtual_path`).
- Exit codes: 0 clean (ambiguous warnings don't fail), 3 pending plan (gone/repoint/excluded-delete), 4 classify guard trip (empty walk = unmounted vault). Exit 2 stays `EXIT_NO_RESULTS`.
- `classify_vault` must never write; `reconcile_vault` = classify + apply, behavior byte-identical to today (existing reconcile tests stay green).
- Run Rust tests: `cargo test -p curated-thoughts reconcile && cargo test -p curated-thoughts-tools`; workflow YAML validated with `python3 -c "import yaml,glob; [yaml.safe_load(open(f)) for f in glob.glob('.github/workflows/*.yml')]"`.
- Conventional commits; all work on `feat/issue-244-ct-release-and-241-drift`, one PR (#249).

---

### Task 1: Split `reconcile_vault` into `classify_vault` + apply (behavior-identical)

**Files:**
- Modify: `src-tauri/src/reconcile.rs` (`ReconcileOutcome` :28-37, `reconcile_vault` :50+, new `ClassifiedOutcome` + `classify_vault`)
- Test: same file (`mod tests`, `vanished_file_is_deleted_and_chunks_cascade` :348-363 and neighbors must stay green)

**Interfaces:**
- Produces: `pub struct ClassifiedOutcome { pub plan: ReconcileOutcome, pub empty_walk: bool }` and `pub fn classify_vault(conn: &Connection, walked: &[WalkedFile], vault_root: &Path) -> Result<ClassifiedOutcome>` (pure: SELECTs only). `reconcile_vault` becomes `classify_vault` + `apply_outcome(conn, &plan, empty_walk, vault_root)`. Task 3's `ct drift` calls `classify_vault` + the shared walk helper only.

- [ ] **Step 1: Write a classification-only failing test** (in `reconcile.rs` `mod tests`, beside :348)

```rust
    #[test]
    fn classify_reports_plan_without_writing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = crate::db::connection::open_in_memory().unwrap();
        let survivor = walked(tmp.path(), "kept.md", b"# kept");
        let gone_path = s(&tmp.path().join("gone.md"));
        seed_doc(&conn, &gone_path, &hash_of(b"# gone"), "user_doc", 5);

        let out = classify_vault(&conn, &[survivor], tmp.path()).unwrap();

        assert_eq!(out.plan.deleted, vec![gone_path]);
        assert!(!out.empty_walk);
        // NOTHING was applied: the row and its chunks are untouched.
        assert_eq!(
            doc_count(&conn),
            1,
            "classify_vault must not delete rows"
        );
    }

    #[test]
    fn classify_flags_empty_walk() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = crate::db::connection::open_in_memory().unwrap();
        let out = classify_vault(&conn, &[], tmp.path()).unwrap();
        assert!(out.empty_walk);
    }
```

(Add a `doc_count(&conn) -> i64` test helper if one does not exist: `SELECT COUNT(*) FROM documents WHERE tier='user_doc'`.)

- [ ] **Step 2: Run to verify RED**

Run: `cargo test -p curated-thoughts reconcile`
Expected: compile failure (`classify_vault` not found).

- [ ] **Step 3: Refactor.** Rename the existing body's outcome to the classification phase and extract the apply arms:

```rust
/// What a reconciliation pass WOULD change, computed without writing.
/// `ct drift` reports this; `reconcile_vault` applies it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ClassifiedOutcome {
    pub plan: ReconcileOutcome,
    /// The empty-walk guard tripped (misconfigured/unmounted vault). The
    /// plan in this state only contains the narrow `.brain` purge produced
    /// by `purge_brain_rows` classification — never a full-index delete.
    pub empty_walk: bool,
}
```

Move the existing `reconcile_vault` body into `classify_vault` with two changes: (a) each mutation arm (repoint UPDATE, delete + chunk cascade, the empty-walk `purge_brain_rows` call) is replaced by plan-recording (the arms already build `outcome` vectors — route the empty-walk branch to a classify-shaped variant of `purge_brain_rows` that computes the same narrow list WITHOUT executing DELETEs; keep `purge_brain_rows` itself for the apply path), and (b) return `ClassifiedOutcome`. Then:

```rust
/// Diff `documents` against `walked` and apply renames and deletions.
/// (Unchanged public behavior: classify + apply, byte-identical outcomes.)
pub fn reconcile_vault(
    conn: &Connection,
    walked: &[WalkedFile],
    vault_root: &Path,
) -> Result<ReconcileOutcome> {
    let classified = classify_vault(conn, walked, vault_root)?;
    apply_outcome(conn, classified, vault_root)
}

fn apply_outcome(
    conn: &Connection,
    classified: ClassifiedOutcome,
    vault_root: &Path,
) -> Result<ReconcileOutcome> {
    if classified.empty_walk {
        eprintln!("[reconcile] walk returned no files; skipping reconciliation");
        return purge_brain_rows(conn, vault_root);
    }
    apply_plan(conn, classified.plan, vault_root)
}
```

Extract the mutation arms of the old body into `apply_plan(conn, plan: ReconcileOutcome, vault_root: &Path) -> Result<ReconcileOutcome>` (the rename-detection/hash-match arms currently do detection-then-mutation interleaved; restructure so ALL detection happens in `classify_vault` and `apply_plan` only executes the recorded actions — the interleaved hash computation reads the DB but writes nothing, so it belongs in classify). Every intermediate step must keep `cargo test -p curated-thoughts reconcile` green; commit at each green point if the restructure takes more than one pass.

- [ ] **Step 4: Run to verify GREEN — all existing reconcile tests**

Run: `cargo test -p curated-thoughts reconcile`
Expected: ALL PASS unchanged (`vanished_file_is_deleted_and_chunks_cascade` etc.) + the two new classify tests.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/reconcile.rs
git commit -m "refactor(reconcile): split classify_vault from apply so drift can share the plan (#241)"
```

### Task 2: Shared walk helper in `tools` (identical list for ingest + drift)

**Files:**
- Modify: `tools/src/cmds.rs` (extract from the :120-183 region: walk + trust re-walk + pending/denied/errors surfacing + sort/dedup)
- Create: `tools/src/walk_list.rs` (or add to an existing shared module — keep it one function)

**Interfaces:**
- Produces: `pub fn build_ingest_file_list(vault_root: &Path, brain_cfg: &BrainConfig) -> Result<Vec<WalkedFile>>` — trust-link re-walk included, `denied`/`pending`/`errors` surfaced to stderr exactly as today, sort+dedup by `virtual_path`. `ct ingest` (Task 2 refactor) and `ct drift` (Task 3) both call it.

- [ ] **Step 1: Extract without behavior change.** Move the walk assembly from the `ingest` command path into:

```rust
/// The EXACT file list `ct ingest` feeds to reconcile + ingest: walk with
/// symlink-trust re-walk, surface denied/pending/errors, sort+dedup by
/// virtual_path. `ct drift` shares this so its view can never diverge from
/// what ingest would actually reconcile (spec M4).
pub fn build_ingest_file_list(vault_root: &Path, trust_links: bool) -> anyhow::Result<Vec<tauri_app_lib::walk_vault::WalkedFile>> {
    let brain_cfg = /* read brain config as cmds.rs does today */;
    let mut outcome = walk_vault(
        vault_root,
        &brain_cfg.trusted_links,
        dirs::home_dir().as_deref(),
    );
    if trust_links {
        // (port the :155-160 re-walk block verbatim)
    }
    for d in &outcome.denied { /* verbatim stderr surfacing from :165-170 */ }
    for p in &outcome.pending { /* verbatim from :171-176 */ }
    for e in &outcome.errors { /* verbatim from :177-179 */ }
    let mut files = outcome.files;
    files.sort_by(|a, b| a.virtual_path.cmp(&b.virtual_path));
    files.dedup_by(|a, b| a.virtual_path == b.virtual_path);
    Ok(files)
}
```

(Verbatim-port the referenced blocks from `cmds.rs:120-183` — the [similar-to] notes above are extraction pointers for ONE refactor commit, not license to change logic. The helper takes `trust_links: bool` to mirror the existing `--trust-new-links` flag plumbing.)

Refactor `ingest` in `cmds.rs` to call the helper. Diff check: `git diff` must show ONLY the moved code (plus the new fn).

- [ ] **Step 2: Run**

Run: `cargo test -p curated-thoughts-tools && cargo clippy -p curated-thoughts-tools --all-targets -- -D warnings`
Expected: ALL PASS, no new warnings.

- [ ] **Step 3: Commit**

```bash
git add tools/src/cmds.rs tools/src/walk_list.rs tools/src/lib.rs
git commit -m "refactor(tools): shared ingest walk helper so drift sees the identical file list (#241)"
```

### Task 3: `ct drift` subcommand

**Files:**
- Modify: `tools/src/bin/ct.rs` (`Cmd` enum ~:14-140, `run()` dispatch :271+)
- Create: `tools/src/drift.rs` (read-only handler; module contract like `queries.rs:1-20`)
- Modify: `tools/src/lib.rs` (module registration)
- Test: `tools/tests/drift.rs` (integration style) or `mod tests` in `drift.rs`

**Interfaces:**
- Consumes: `classify_vault` (Task 1), `build_ingest_file_list` (Task 2), `open_ro`/`resolve` (`tools::write`), `EXIT_NO_RESULTS` convention.
- Produces: `Cmd::Drift { json: bool }`; exit contract 0/3/4 per Global Constraints. JSON output shape:

```json
{ "empty_walk": false, "gone": ["wiki/old.md"], "repointed": [{"from": "a.md", "to": "b.md"}], "excluded_deletes": [".brain/errors.log"], "ambiguous_warnings": ["x.md"] }
```

- [ ] **Step 1: Write failing tests** (fixture style of reconcile tests: tempdir + seeded sqlite; the tools crate can reach `tauri_app_lib::db::connection::open_in_memory` and `tauri_app_lib::reconcile::seed`-equivalent helpers — if seeding helpers are `pub(crate)`, replicate the minimal `INSERT INTO documents` inline)

```rust
    // Human + JSON output content assertions live on the CLASSIFY result, so
    // the tests seed a brain.db via the lib helpers, run the handler fn, and
    // assert on the returned summary struct + exit code.
    #[test]
    fn drift_clean_vault_exits_0() { /* seed: every row has its file; classify → empty plan; assert Ok(0) */ }

    #[test]
    fn drift_pending_gone_exits_3() { /* seed one row whose file is gone; assert Ok(3) + report lists it under "gone" */ }

    #[test]
    drift_pending_repoint_exits_3() { /* seed a row whose content moved (same hash, new path); assert Ok(3) + repoint in report */ }

    #[test]
    fn drift_ambiguous_only_exits_0_with_warning() {
        // Seed a vanished row whose hash matches TWO new files (ambiguous arm).
        // Assert Ok(0) and the path listed under ambiguous_warnings.
    }

    #[test]
    fn drift_empty_walk_exits_4() { /* walk list is empty (empty vault dir); assert Ok(4) */ }
```

- [ ] **Step 2: Run to verify RED**

Run: `cargo test -p curated-thoughts-tools drift`
Expected: compile failure (`Drift` variant not found).

- [ ] **Step 3: Implement the handler** (`tools/src/drift.rs`):

```rust
//! `ct drift` — read-only drift report (issue #241).
//!
//! Reports what reconcile WOULD do on the next `ct ingest --yes`:
//! gone files, offline moves, excluded-dir deletes — plus ambiguous rows
//! as WARNINGS (ingest never clears them; failing on them would mean a
//! permanent nonzero exit after a "successful" repair).
//! Never writes: read-only connection, classify only.

pub struct DriftReport {
    pub empty_walk: bool,
    pub gone: Vec<String>,
    pub repointed: Vec<(String, String)>,
    pub excluded_deletes: Vec<String>,
    pub ambiguous_warnings: Vec<String>,
}

pub fn drift_cmd(json: bool) -> anyhow::Result<i32> {
    let brain = crate::write::resolve()?;
    let conn = crate::write::open_ro(&brain)?;
    let vault_root = /* resolve the configured vault root the same way cmds.rs ingest does (brain config) */;
    let files = crate::walk_list::build_ingest_file_list(&vault_root, /* trust_links: false for drift — see note */ false)?;

    let classified = tauri_app_lib::reconcile::classify_vault(&conn, &files, &vault_root)?;

    if classified.empty_walk {
        if json { println!(r#"{{"empty_walk": true}}"#); }
        else { eprintln!("drift: vault walk returned no files — vault missing or unmounted; NOT reporting drift"); }
        return Ok(4);
    }

    let report = DriftReport {
        empty_walk: false,
        gone: classified.plan.deleted.clone(),
        repointed: classified.plan.repointed.clone(),
        excluded_deletes: classified.plan.deleted.iter().filter(|p| is_excluded(p, &vault_root)).cloned().collect(),
        ambiguous_warnings: classified.plan.ambiguous.clone(),
    };
    // NOTE: reconcile's `deleted` vector mixes plain-gone and
    // excluded-dir deletes; if classify exposes the partition
    // (Task 1 restructure), use it directly instead of re-filtering here.

    let pending = !report.gone.is_empty() || !report.repointed.is_empty() || !report.excluded_deletes.is_empty();
    if json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        for p in &report.gone { println!("drift: gone {p}"); }
        for (o, n) in &report.repointed { println!("drift: moved {o} -> {n}"); }
        for p in &report.excluded_deletes { println!("drift: excluded-delete {p}"); }
        for p in &report.ambiguous_warnings { eprintln!("warning: ambiguous (left alone by repair): {p}"); }
        if pending {
            eprintln!("repair with: ct ingest --yes");
        }
    }
    Ok(if pending { 3 } else { 0 })
}
```

(Resolve the two inline `/* ... */` notes against `cmds.rs`'s actual vault-root resolution and `trust_links` plumbing — copy the exact expressions; drift uses the same defaults as ingest's non-flag path. If Task 1's classify kept the excluded/gone partition internal, extend `ClassifiedOutcome` with the partition fields rather than re-filtering here.)

Dispatch in `ct.rs`:

```rust
        Cmd::Drift { json } => crate::drift::drift_cmd(json),
```

with the enum variant (place before `Heal`):

```rust
    /// Report what reconcile would repair (read-only; no writes).
    Drift {
        #[arg(long)]
        json: bool,
    },
```

- [ ] **Step 4: Run to verify GREEN**

Run: `cargo test -p curated-thoughts-tools drift && cargo test -p curated-thoughts-tools`
Expected: ALL PASS.

- [ ] **Step 5: Commit**

```bash
git add tools/src/bin/ct.rs tools/src/drift.rs tools/src/lib.rs tools/tests/drift.rs
git commit -m "feat(tools): ct drift — read-only reconcile dry-run report (#241)"
```

### Task 4: README self-cleaning docs

**Files:**
- Modify: `README.md` (`ct` section, the block around `ct ingest --yes` / `ct status`, ~:226-246)

- [ ] **Step 1: Add the deletion-hygiene note.** After the `ct ingest` row in the CLI table/paragraph list, insert:

```markdown
Deleting a superseded note from the vault is safe and recommended: the
watcher, the next app startup, and `ct ingest` all remove its document row,
chunks, and embeddings automatically. Note lineage lives in git history (and
your session records), not in the retrieval DB. Run `ct drift` for a
read-only check that the database matches the vault; repair any reported
drift with `ct ingest --yes`.
```

- [ ] **Step 2: Verify rendered output**

Run: `grep -A6 "Deleting a superseded note" README.md`
Expected: the block reads correctly in place.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs(readme): vault deletion is self-cleaning; ct drift check / ct ingest repair (#241)"
```

### Task 5: Build workflow — ct asset + tauri-action tag gate

**Files:**
- Modify: `.github/workflows/build.yml`

**Interfaces:**
- Consumes: existing steps "Build MCP sidecar" (:75-100), "Build MCP sidecar (macOS universal)" (:121-127), tauri-action (:147-157).
- Produces: "Build ct CLI" steps (linux/windows/macOS), "Upload ct release asset" step, tauri-action `if:` gate.

- [ ] **Step 1: Add version derivation + ct build steps.** Insert after the "Build MCP sidecar (macOS universal)" step and before "Install dependencies" (exact YAML; follows the sidecar steps' pattern):

```yaml
      - name: Derive version
        id: ctver
        run: |
          VERSION="$(jq -r .version src-tauri/tauri.conf.json)"
          echo "version=${VERSION}" >> "$GITHUB_OUTPUT"
          if [ "${{ github.ref_type }}" = "tag" ]; then
            TAG_VERSION="${GITHUB_REF_NAME#v}"
            if [ "${VERSION}" != "${TAG_VERSION}" ]; then
              echo "::error::tag ${GITHUB_REF_NAME} but tauri.conf.json says ${VERSION}" >&2
              exit 1
            fi
          fi

      - name: Build ct CLI
        if: matrix.platform != 'macos-latest'
        run: |
          set -euo pipefail
          cargo build --release -p curated-thoughts-tools --bin ct
          case "${{ matrix.platform }}" in
            ubuntu-22.04) OS=linux; ARCH=amd64; EXT=tar.gz ;;
            windows-latest) OS=windows; ARCH=amd64; EXT=zip ;;
            *) echo "Unsupported platform: ${{ matrix.platform }}" >&2; exit 1 ;;
          esac
          ASSET="ct_${{ steps.ctver.outputs.version }}_${OS}_${ARCH}.${EXT}"
          echo "asset=${ASSET}" >> "$GITHUB_OUTPUT"
          if [ "${EXT}" = "zip" ]; then
            pwsh -NoProfile -Command "Compress-Archive -Path target/release/ct.exe -DestinationPath '${ASSET}'"
          else
            tar czf "${ASSET}" -C target/release ct
          fi
        shell: bash

      - name: Build ct CLI (macOS universal)
        if: matrix.platform == 'macos-latest'
        run: |
          set -euo pipefail
          cargo build --release -p curated-thoughts-tools --bin ct --target aarch64-apple-darwin
          cargo build --release -p curated-thoughts-tools --bin ct --target x86_64-apple-darwin
          lipo -create \
            target/aarch64-apple-darwin/release/ct \
            target/x86_64-apple-darwin/release/ct \
            -output target/ct-universal
          lipo -info target/ct-universal
          ASSET="ct_${{ steps.ctver.outputs.version }}_macos_universal.tar.gz"
          echo "asset=${ASSET}" >> "$GITHUB_OUTPUT"
          tar czf "${ASSET}" -C target ct-universal
        shell: bash

      - name: Smoke test ct CLI
        run: |
          BIN=./target/release/ct
          if [ "${{ matrix.platform }}" = "macos-latest" ]; then BIN=./target/ct-universal; fi
          "${BIN}" --help > /dev/null
        shell: bash
```

- [ ] **Step 2: Tag-gate tauri-action + add the upload step.** tauri-action (:147-157) gains `if: github.ref_type == 'tag'`. After it, add:

```yaml
      - name: Upload ct release asset
        if: github.ref_type == 'tag'
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          gh release upload "$GITHUB_REF_NAME" ct_*.* --clobber
```

(Also add a step-level `if: startsWith(github.ref, 'refs/tags/')` equivalent only if the tag-type check proves insufficient for `workflow_dispatch` from a tag ref — `ref_type` is the authoritative signal.)

- [ ] **Step 3: Validate the YAML**

Run: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/build.yml'))"`
Expected: no exception.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/build.yml
git commit -m "ci(build): ship ct CLI as a release asset; tag-gate tauri-action (#244)"
```

### Task 6: Spec flip + final verification + push

**Files:**
- Modify: `docs/superpowers/specs/2026-09-28-issue244-241-ct-release-and-drift-design.md` (status)

- [ ] **Step 1: Flip the spec status** to `**Status:** Implemented 2026-09-28 (PR #249)`.

- [ ] **Step 2: Full local verification (CI parity)**

Run: `cargo test -p curated-thoughts && cargo test -p curated-thoughts-tools && cargo clippy -p curated-thoughts -p curated-thoughts-tools --all-targets -- -D warnings && python3 -c "import yaml,glob; [yaml.safe_load(open(f)) for f in glob.glob('.github/workflows/*.yml')]" && pnpm test`
Expected: all green.

- [ ] **Step 3: Commit + push.** CI verification of the workflow steps happens via `workflow_dispatch` on this PR branch after push (build + package steps run; tauri-action gate keeps the release untouched — that is the BLOCKER fix being exercised). The upload path itself is proven on the next real release tag.

```bash
git add docs/superpowers/specs/2026-09-28-issue244-241-ct-release-and-drift-design.md
git commit -m "docs(spec): mark #244/#241 design implemented (PR #249)"
git push origin feat/issue-244-ct-release-and-241-drift
```
