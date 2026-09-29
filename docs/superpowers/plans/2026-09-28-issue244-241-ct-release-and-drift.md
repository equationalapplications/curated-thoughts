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
- Produces: `pub struct ClassifiedOutcome { pub plan: ReconcileOutcome, pub gone_deletes: Vec<String>, pub excluded_deletes: Vec<String>, pub empty_walk: bool }` and `pub fn classify_vault(conn: &Connection, walked: &[WalkedFile], vault_root: &Path) -> Result<ClassifiedOutcome>` (pure: SELECTs only). The `gone_deletes` / `excluded_deletes` partition is filled from the classify pass's existing `excluded` / `remaining` arms (reconcile.rs:190-192 fills `excluded_deletes`, :215 fills `gone_deletes`), so `plan.deleted == gone_deletes + excluded_deletes` always holds and drift never re-filters. `ReconcileOutcome` itself is UNCHANGED, so `reconcile_vault` output stays byte-identical. `reconcile_vault` becomes `classify_vault` + `apply_outcome(conn, classified, vault_root)`. Task 3's `ct drift` calls `classify_vault` + the shared walk helper only.

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
    /// The subset of `plan.deleted` whose files landed under an excluded
    /// dir (the `excluded` arm at reconcile.rs:190-192).
    pub excluded_deletes: Vec<String>,
    /// The subset of `plan.deleted` whose files plain vanished (the
    /// no-candidate arm at :215). `plan.deleted == gone_deletes +
    /// excluded_deletes` — drift consumes this partition directly and
    /// never re-filters.
    pub gone_deletes: Vec<String>,
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
- Modify: `tools/src/cmds.rs` (extract from the :90-210 region: walk + trust re-walk + pending/denied/errors surfacing + sort/dedup)
- Create: `tools/src/walk_list.rs` (or add to an existing shared module — keep it one function)
- Modify: `tools/src/lib.rs` (module registration — the commit below stages it, so the Files list must include it)

**Interfaces:**
- Produces: `pub fn build_ingest_file_list(paths_b: &BrainPaths, trust_new_links: bool) -> anyhow::Result<(std::path::PathBuf, Vec<tauri_app_lib::walk_vault::WalkedFile>)>` — returns the CANONICALIZED vault root alongside the list, because ingest canonicalizes (`cmds.rs:23`: `vault_root.canonicalize().unwrap_or(vault_root)`) and drift must see the same root or `relativize_to_vault` matching diverges (spec M4). Trust-link re-walk included verbatim (`cmds.rs:108-184`: the `trust_new_links` gate, `classify_link` promotion, ledger persist via `brain_cfg.write(&paths_b)`, second `walk_vault`, the `!outcome.pending.is_empty()` guard — NOT just the final re-walk), `denied`/`pending`/`errors` surfaced to stderr exactly as today, sort+dedup by `virtual_path`. `ct ingest` (Task 2 refactor) and `ct drift` (Task 3) both call it.

- [ ] **Step 1: Extract without behavior change.** Move the walk assembly from the `ingest` command path into:

```rust
/// The EXACT file list `ct ingest` feeds to reconcile + ingest: resolve the
/// vault root from config and canonicalize it (cmds.rs:22-23), walk with the
/// symlink-trust re-walk, surface denied/pending/errors, sort+dedup by
/// virtual_path. Returns the CANONICAL root so drift reconciles against the
/// same root ingest used. `ct drift` shares this so its view can never
/// diverge from what ingest would actually reconcile (spec M4).
pub fn build_ingest_file_list(
    paths_b: &BrainPaths,
    trust_new_links: bool,
) -> anyhow::Result<(std::path::PathBuf, Vec<tauri_app_lib::walk_vault::WalkedFile>)> {
    // Verbatim from cmds.rs ingest (cmds.rs:18-23):
    let config = VaultConfig::new(paths_b.config_path.clone());
    let vault_root = config
        .vault_root()?
        .ok_or_else(|| anyhow::anyhow!("vault root missing"))?;
    let vault_root = vault_root.canonicalize().unwrap_or(vault_root);

    let mut brain_cfg = tauri_app_lib::config::BrainConfig::load(paths_b)
        .context("read trusted_links ledger from config.json")?;
    let mut outcome = walk_vault(
        &vault_root,
        &brain_cfg.trusted_links,
        dirs::home_dir().as_deref(),
    );

    // Port cmds.rs:108-184 WHOLESALE — the `trust_new_links &&
    // !outcome.pending.is_empty()` gate, classify_link promotion, the
    // ledger persist via brain_cfg.write(paths_b), and the second
    // walk_vault. `if trust_new_links` alone is NOT the gate.
    if trust_new_links && !outcome.pending.is_empty() {
        // (verbatim block from cmds.rs:111-144)
    }

    for d in &outcome.denied { /* verbatim stderr surfacing */ }
    for p in &outcome.pending { /* verbatim */ }
    for e in &outcome.errors { /* verbatim */ }
    let mut files = outcome.files;
    files.sort_by(|a, b| a.virtual_path.cmp(&b.virtual_path));
    files.dedup_by(|a, b| a.virtual_path == b.virtual_path);
    Ok((vault_root, files))
}
```

(Verbatim-port the referenced blocks from `cmds.rs:90-210` — the [similar-to] notes above are extraction pointers for ONE refactor commit, not license to change logic. The helper takes `trust_new_links: bool` to mirror the existing `--trust-new-links` flag plumbing.)

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

- [ ] **Step 1: Write failing tests** (target `drift_report`, the pure core — NOT `drift_cmd`, whose brain/HOME resolution makes it untestable as a unit. If end-to-end coverage is wanted later, `temp-env` (already a dev-dependency) is the tool for `drift_cmd`. Fixture style of reconcile tests: tempdir + seeded sqlite. `tauri_app_lib::db::connection::open_in_memory` is `pub` (reconcile.rs:873) and the reconcile `mod tests` helpers (`seed_doc` :274, `walked` :308, `hash_of` :320, `s` :324) are `#[cfg(test)]`-gated — replicate the minimal `INSERT INTO documents` inline or via a small local helper in the drift test module.)

```rust
    // All tests call tools::drift::drift_report(&conn, &files, &vault_root)
    // and assert on (DriftReport, i32). Fixtures use the same
    // open_in_memory + seed-docs + walked() pattern as reconcile's tests.

    #[test]
    fn drift_clean_vault_exits_0() { /* seed: every row has its file; classify → empty plan; assert Ok((report, 0)), all report vecs empty */ }

    #[test]
    fn drift_pending_gone_exits_3() { /* seed one row whose file is gone; assert Ok((report, 3)) + report.gone lists it */ }

    #[test]
    fn drift_pending_repoint_exits_3() { /* seed a row whose content moved (same hash, new path); assert Ok((report, 3)) + report.repointed has {from, to} */ }

    #[test]
    fn drift_excluded_delete_listed_under_excluded_not_gone() {
        // Spec :111 REQUIRED case — the Sep-27 regression class. Seed a
        // `documents` row whose path lives under an excluded dir (e.g.
        // `.brain/errors.log`) and remove the file. Assert Ok((report, 3)),
        // the path appears in report.excluded_deletes AND NOT in
        // report.gone (no double-count), and the JSON serialization of
        // excluded_deletes is a plain string array.
    }

    #[test]
    fn drift_walk_identity_with_ingest() {
        // Spec :114-115 REQUIRED case — drift and ingest must produce the
        // same file list. Build the list via
        // tools::walk_list::build_ingest_file_list(&paths, false), then
        // assert classify_vault(conn, &files, &root) equals what drift_report
        // computed for the same inputs (same root, same list ⇒ same report).
        // Guards the canonicalized-root contract: if drift ever walks with a
        // non-canonical root while ingest canonicalizes, this test fails.
    }

    #[test]
    fn drift_ambiguous_only_exits_0_with_warning() {
        // Seed a vanished row whose hash matches TWO new files (ambiguous arm).
        // Assert Ok((report, 0)) and the path listed under ambiguous_warnings.
    }

    #[test]
    fn drift_empty_walk_exits_4() { /* walk list is empty (empty vault dir); assert Ok((report, 4)), report.empty_walk */ }

    #[test]
    fn drift_report_serializes_documented_shape() {
        // Serialize a populated DriftReport with serde_json and assert:
        // repointed is [{"from": .., "to": ..}] objects (NOT [["a","b"]]),
        // and the empty-walk report serializes as the FULL shape with
        // empty_walk: true and empty vectors — not a different shape.
    }
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

use serde::Serialize;

/// Serializable EXACTLY as documented in the JSON contract above:
/// `repointed` must emit `[{"from": .., "to": ..}]` objects, NOT
/// `Vec<(String, String)>` (which serializes as `[["a","b"]]`).
#[derive(Debug, Serialize)]
pub struct Repoint {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Serialize)]
pub struct DriftReport {
    pub empty_walk: bool,
    pub gone: Vec<String>,
    pub repointed: Vec<Repoint>,
    pub excluded_deletes: Vec<String>,
    pub ambiguous_warnings: Vec<String>,
}

/// Pure core: classify an already-built file list into a report + exit code.
/// Unit tests target THIS function (no brain config, no HOME resolution);
/// `drift_cmd` is a thin I/O wrapper around it.
pub fn drift_report(
    conn: &Connection,
    files: &[tauri_app_lib::walk_vault::WalkedFile],
    vault_root: &Path,
) -> anyhow::Result<(DriftReport, i32)> {
    let classified = tauri_app_lib::reconcile::classify_vault(conn, files, vault_root)?;

    if classified.empty_walk {
        let report = DriftReport {
            empty_walk: true,
            gone: vec![],
            repointed: vec![],
            excluded_deletes: vec![],
            ambiguous_warnings: vec![],
        };
        return Ok((report, 4));
    }

    let report = DriftReport {
        empty_walk: false,
        // Consume the classify partition directly — NO is_excluded
        // re-filter (there is no such helper, and `plan.deleted` holds
        // both categories).
        gone: classified.gone_deletes.clone(),
        excluded_deletes: classified.excluded_deletes.clone(),
        repointed: classified
            .plan
            .repointed
            .iter()
            .map(|(from, to)| Repoint { from: from.clone(), to: to.clone() })
            .collect(),
        ambiguous_warnings: classified.plan.ambiguous.clone(),
    };
    let pending = !report.gone.is_empty()
        || !report.repointed.is_empty()
        || !report.excluded_deletes.is_empty();
    Ok((report, if pending { 3 } else { 0 }))
}

/// I/O wrapper: resolves brain + vault root, builds the walk list, prints.
pub fn drift_cmd(json: bool) -> anyhow::Result<i32> {
    let brain = crate::write::resolve()?;
    let conn = crate::write::open_ro(&brain)?;
    // Resolve the configured vault root exactly as cmds.rs ingest does:
    // VaultConfig::new(paths.config_path).vault_root()? (error if missing),
    // then .canonicalize().unwrap_or(vault_root). Canonicalization is NOT
    // optional — non-canonical roots break relativize_to_vault matching and
    // drift would report false gone/excluded deletes (spec M4).
    let paths = tauri_app_lib::retrieval::resolve_brain_paths();
    let config = tauri_app_lib::vault::VaultConfig::new(paths.config_path);
    let vault_root = config
        .vault_root()?
        .ok_or_else(|| anyhow::anyhow!("vault root missing"))?;
    let vault_root = vault_root.canonicalize().unwrap_or(vault_root);
    // trust_links: false — drift reports what a plain ingest would see;
    // promoting pending links is a `ct trust` decision, not drift's.
    let (_vault_root_from_helper, files) =
        crate::walk_list::build_ingest_file_list(&paths, false)?;
    // `_vault_root_from_helper` equals the canonicalized `vault_root`
    // resolved above (same code path); either may be used for classify.

    let (report, code) = drift_report(&conn, &files, &vault_root)?;

    if report.empty_walk {
        if json { println!(r#"{{"empty_walk": true}}"#); }
        else {
            // Spec :90-92 wording — drift made NO classification; ingest
            // or app startup WOULD purge .brain rows on this walk.
            eprintln!("drift: vault walk returned no files — vault missing or unmounted; no drift classified (ingest would purge .brain rows for this walk)");
        }
        return Ok(4);
    }

    if json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        for p in &report.gone { println!("drift: gone {p}"); }
        for r in &report.repointed { println!("drift: moved {} -> {}", r.from, r.to); }
        for p in &report.excluded_deletes { println!("drift: excluded-delete {p}"); }
        for p in &report.ambiguous_warnings { eprintln!("warning: ambiguous (left alone by repair): {p}"); }
        if code == 3 {
            eprintln!("repair with: ct ingest --yes");
        }
    }
    Ok(code)
}
```

Dispatch in `ct.rs`:

```rust
        Cmd::Drift { json } => curated_thoughts_tools::drift::drift_cmd(json),
```

(NOT `crate::drift::drift_cmd` — `ct.rs` is a separate bin target, so `crate::` resolves inside the bin only and `drift` lives in the library crate. This matches the existing pattern at `ct.rs:3`.)

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
        # bash syntax (jq, $GITHUB_OUTPUT, [ ]) — without this, the Windows
        # leg runs `run:` under pwsh and dies before tauri-action ever fires.
        shell: bash
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
        id: ctbuild
        shell: bash
        run: |
          set -euo pipefail
          cargo build --release -p curated-thoughts-tools --bin ct
          TARGET_DIR="$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)"
          case "${{ matrix.platform }}" in
            ubuntu-22.04) OS=linux; ARCH=amd64; EXT=tar.gz ;;
            windows-latest) OS=windows; ARCH=amd64; EXT=zip ;;
            *) echo "Unsupported platform: ${{ matrix.platform }}" >&2; exit 1 ;;
          esac
          ASSET="ct_${{ steps.ctver.outputs.version }}_${OS}_${ARCH}.${EXT}"
          echo "asset=${ASSET}" >> "$GITHUB_OUTPUT"
          # Spec :38-41 — every archive carries a short README alongside the binary.
          printf 'ct: the Curated Thoughts headless CLI. See the repo README for usage.\n' > README.txt
          if [ "${EXT}" = "zip" ]; then
            pwsh -NoProfile -Command "Compress-Archive -Path '${TARGET_DIR}/release/ct.exe','README.txt' -DestinationPath '${ASSET}'"
          else
            tar czf "${ASSET}" -C "${TARGET_DIR}/release" ct -C "$OLDPWD/." README.txt
          fi

      - name: Build ct CLI (macOS universal)
        if: matrix.platform == 'macos-latest'
        id: ctbuildmac
        shell: bash
        run: |
          set -euo pipefail
          cargo build --release -p curated-thoughts-tools --bin ct --target aarch64-apple-darwin
          cargo build --release -p curated-thoughts-tools --bin ct --target x86_64-apple-darwin
          TARGET_DIR="$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)"
          lipo -create \
            "${TARGET_DIR}/aarch64-apple-darwin/release/ct" \
            "${TARGET_DIR}/x86_64-apple-darwin/release/ct" \
            -output "${TARGET_DIR}/ct-universal"
          lipo -info "${TARGET_DIR}/ct-universal"
          # Stage under the user-facing name so the archive unpacks to `ct`,
          # not `ct-universal` (spec: each archive holds the binary + README).
          cp "${TARGET_DIR}/ct-universal" "${TARGET_DIR}/ct"
          ASSET="ct_${{ steps.ctver.outputs.version }}_macos_universal.tar.gz"
          echo "asset=${ASSET}" >> "$GITHUB_OUTPUT"
          printf 'ct: the Curated Thoughts headless CLI. See the repo README for usage.\n' > "${TARGET_DIR}/README.txt"
          tar czf "${ASSET}" -C "${TARGET_DIR}" ct README.txt

      - name: Smoke test ct CLI
        shell: bash
        run: |
          TARGET_DIR="$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)"
          BIN="${TARGET_DIR}/release/ct"
          if [ "${{ matrix.platform }}" = "macos-latest" ]; then BIN="${TARGET_DIR}/ct-universal"; fi
          "${BIN}" --help > /dev/null
```

- [ ] **Step 2: Tag-gate tauri-action + add the upload step.** tauri-action (:147-157) gains `if: github.ref_type == 'tag'`. After it, add:

```yaml
      - name: Upload ct release asset
        if: github.ref_type == 'tag'
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        # bash syntax ($GITHUB_REF_NAME env-var expansion). pwsh would expand
        # "$GITHUB_REF_NAME" as an undefined PowerShell variable to "" and the
        # glob `ct_*.*` would never expand. Upload the EXACT filename from the
        # build step output instead of a glob so a partial matrix leg cannot
        # silently skip or double-upload.
        shell: bash
        run: |
          if [ -n "${{ steps.ctbuild.outputs.asset }}" ]; then
            gh release upload "$GITHUB_REF_NAME" "${{ steps.ctbuild.outputs.asset }}" --clobber
          else
            gh release upload "$GITHUB_REF_NAME" "${{ steps.ctbuildmac.outputs.asset }}" --clobber
          fi
```

(The empty-output `if/else` picks the asset from whichever build step ran on this matrix leg — the non-macOS `ctbuild` step is skipped on macOS, and vice versa. The upload path itself is proven on the next real release tag; the dispatch run exercises everything up to it.)

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
