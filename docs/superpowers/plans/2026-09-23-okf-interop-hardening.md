# OKF Interop Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the three open CodeRabbit findings on PR #226 (permissions, durability, `$HOME` precedence) and pin zip timestamps for deterministic backups.

**Architecture:** Tasks 1–3 harden the exporter + shared zip writer in place; Task 4 is the shared-writer determinism change that also benefits the GUI; Task 5 is live verification against the real brain. All work happens on the PR branch — the spec already lives at `docs/superpowers/specs/2026-09-23-okf-interop-hardening-design.md` on this same branch.

**Tech Stack:** Rust (workspace: `src-tauri` lib + `tools` bins), `zip` 8.6, `rusqlite`, vitest (frontend unaffected except regression).

**Spec:** `docs/superpowers/specs/2026-09-23-okf-interop-hardening-design.md`

## Global Constraints

- Branch: `fix/okf-dialog-filter-and-exporter` (PR #226). No new branch, no new PR.
- OKF version `0.2`, profile `llm-wiki/2` — do not change bundle format or import semantics.
- `write_bundle_zip` in `src-tauri/src/okf/zip_io.rs` is shared by GUI (`okf_api.rs:66`) and exporter — changes must keep both callers working and compile on Windows (`#[cfg(unix)]` guards for permission/fsync code).
- Repo convention: regular merge commits; clippy is enforced; gitleaks runs on commit (`--no-verify` only where documented).
- TDD: every behavior change lands test-first where testable without a real brain DB.

---

### Task 1: Resolve `$HOME` only when no destination is supplied (R5)

**Files:**
- Modify: `tools/src/bin/export_okf_bundle.rs` (main fn, dest resolution ~lines 48–55)
- Test: live-run matrix (no unit harness for this bin — covered by run steps below)

**Interfaces:**
- Consumes: nothing new.
- Produces: dest resolution order = explicit arg → `$HOME/brain-okf.zip` → error. No signature changes.

- [ ] **Step 1: Reorder resolution so explicit args never touch `dirs::home_dir()`**

Replace the current block:

```rust
let home = dirs::home_dir().context("cannot resolve $HOME; pass an explicit dest")?;
let dest = std::env::args()
    .nth(1)
    .map(PathBuf::from)
    .unwrap_or_else(|| home.join("brain-okf.zip"));
```

with:

```rust
let dest = match std::env::args().nth(1) {
    Some(arg) => {
        if arg == "--help" || arg == "-h" {
            eprintln!("usage: export_okf_bundle [dest.zip]");
            eprintln!();
            eprintln!("Exports the whole brain as an OKF 0.2 bundle.");
            eprintln!("Defaults to $HOME/brain-okf.zip. Honors CURATED_BRAIN_* env vars.");
            std::process::exit(0);
        }
        if arg.starts_with('-') {
            eprintln!("error: unknown flag {arg}");
            eprintln!("usage: export_okf_bundle [dest.zip]");
            std::process::exit(2);
        }
        PathBuf::from(arg)
    }
    None => {
        let home = dirs::home_dir().context("cannot resolve $HOME; pass an explicit dest")?;
        home.join("brain-okf.zip")
    }
};
```

(Note: keep the existing flag-validation semantics — this restructure preserves them and folds `--help` into the same match.)

- [ ] **Step 2: Verify the matrix by live run**

Run: `cargo build --release -p curated-thoughts-tools --bin export_okf_bundle && HOME= /home/kv/code/github/equationalapplications/curated-thoughts/target/release/export_okf_bundle /tmp/okf-t1/a.zip && ls -la /tmp/okf-t1/`

Expected: explicit dest works even with `$HOME` empty (the `HOME=` prefix simulates the headless account). Then run the default path `…/export_okf_bundle` with no args → writes `~/brain-okf.zip`, exit 0. Delete both test artifacts afterwards.

- [ ] **Step 3: Commit**

```bash
git add tools/src/bin/export_okf_bundle.rs
git commit -m "fix(tools): resolve \$HOME only for the default export path (CR 4088815418)"
```

### Task 2: Durability — sync the zip before rename, fsync the parent dir after (R4)

**Files:**
- Modify: `src-tauri/src/okf/zip_io.rs` (`write_bundle_zip`, ~lines 82–95)
- Modify: `tools/src/bin/export_okf_bundle.rs` (after `std::fs::rename`, before digest)
- Test: `src-tauri/src/okf/zip_io.rs` `#[cfg(test)] mod tests` (existing `zip_round_trip` must stay green)

**Interfaces:**
- Consumes: `write_bundle_zip(dest: &Path, files: &[OkfFile]) -> Result<()>` (unchanged signature).
- Produces: `write_bundle_zip` now fsyncs the file before returning. New private helper `sync_parent_dir(path: &Path) -> Result<()>` in the exporter (Unix-only behavior, no-op stub elsewhere) — defined in the exporter, not the lib, because the lib has no std-fs durability dependency today.

- [ ] **Step 1: Extend `write_bundle_zip` to fsync the finished archive**

In `src-tauri/src/okf/zip_io.rs`, change the function body to keep a handle to the file so it can sync before drop:

```rust
pub fn write_bundle_zip(dest: &Path, files: &[OkfFile]) -> Result<()> {
    let file = File::create(dest).with_context(|| format!("creating {}", dest.display()))?;
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for f in files {
        writer.start_file(&f.path, options)?;
        writer.write_all(f.content.as_bytes())?;
    }
    let file = writer
        .finish()
        .with_context(|| format!("finishing {}", dest.display()))?;
    file.sync_all()
        .with_context(|| format!("syncing {}", dest.display()))?;
    Ok(())
}
```

(`zip::ZipWriter::finish()` returns the inner `File` — verify the exact type with `cargo check`; if it returns `()` in zip 8.6, reopen with `File::open(dest)?` + `sync_all()` instead.)

- [ ] **Step 2: Run the existing zip tests**

Run: `cargo test -p curated-thoughts --lib okf::zip_io -- --nocapture`
Expected: `zip_round_trip` (and siblings) PASS.

- [ ] **Step 3: fsync the parent dir in the exporter after rename (Unix)**

In `tools/src/bin/export_okf_bundle.rs`, immediately after the existing `std::fs::rename(...)` and before `sha256_hex`:

```rust
#[cfg(unix)]
{
    if let Some(parent) = dest.parent() {
        let dir = std::fs::File::open(parent)
            .with_context(|| format!("opening {}", parent.display()))?;
        dir.sync_all()
            .with_context(|| format!("syncing {}", parent.display()))?;
    }
}
```

- [ ] **Step 4: Build + live run**

Run: `cargo build --release -p curated-thoughts-tools --bin export_okf_bundle && ./target/release/export_okf_bundle /tmp/okf-t2/b.zip && unzip -t /tmp/okf-t2/b.zip >/dev/null && echo OK`
Expected: exports cleanly, `OK` printed. Remove `/tmp/okf-t2` afterwards.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/okf/zip_io.rs tools/src/bin/export_okf_bundle.rs
git commit -m "fix(okf): fsync bundle before rename and parent dir after (CR 4088815428)"
```

### Task 3: Restrictive permissions on the published backup (R3)

**Files:**
- Modify: `tools/src/bin/export_okf_bundle.rs` (temp-file creation + rename block)
- Test: run-step permission assertion (no cross-platform unit test — the bin has no fixture harness; behavior asserted live in Task 5)

**Interfaces:**
- Consumes: `write_bundle_zip` from Task 2 (fsync inside).
- Produces: published bundle is `0o600` on Unix regardless of umask.

- [ ] **Step 1: Create the temp file at 0o600 before handing it to the writer**

Replace the temp-write + rename block with:

```rust
// Write to a temp sibling, verify, then atomically publish. The temp file
// is created 0o600 (umask-independent) so the rename can never widen the
// permissions of an existing 0o600 backup; the bundle is unredacted.
#[cfg(unix)]
{
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = dest.with_extension("zip.partial");
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .with_context(|| format!("creating {}", tmp.display()))?;
    write_bundle_zip(&tmp, &files).with_context(|| format!("writing {}", tmp.display()))?;
    // ... existing round-trip verify, then rename (unchanged) ...
}
#[cfg(not(unix))]
{
    let tmp = dest.with_extension("zip.partial");
    write_bundle_zip(&tmp, &files).with_context(|| format!("writing {}", tmp.display()))?;
    // ... existing round-trip verify, then rename (unchanged) ...
}
```

To avoid duplicating the verify+rename sequence across the two cfg arms, factor the body into a small closure or keep the cfg only around the OpenOptions pre-create step and let `write_bundle_zip` truncate the already-created file — the writer uses `File::create`, which truncates without changing existing permissions. Simplest correct shape:

```rust
let tmp = dest.with_extension("zip.partial");
#[cfg(unix)]
{
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .with_context(|| format!("creating {}", tmp.display()))?;
}
write_bundle_zip(&tmp, &files).with_context(|| format!("writing {}", tmp.display()))?;
// (existing round-trip verify + rename stay exactly as they are)
```

Use this last shape — single writer call, cfg only around the pre-create.

- [ ] **Step 2: Build + permission check**

Run: `cargo build --release -p curated-thoughts-tools --bin export_okf_bundle && rm -f /tmp/okf-t3/c.zip && umask 022 && ./target/release/export_okf_bundle /tmp/okf-t3/c.zip && stat -c '%a' /tmp/okf-t3/c.zip`
Expected: `600`.

- [ ] **Step 3: Rename-over-widening check**

Run: `chmod 644 /tmp/okf-t3/c.zip && ./target/release/export_okf_bundle /tmp/okf-t3/c.zip && stat -c '%a' /tmp/okf-t3/c.zip`
Expected: `600` (the rename replaced the 644 file and the new inode is 600).

- [ ] **Step 4: Commit**

```bash
git add tools/src/bin/export_okf_bundle.rs
git commit -m "fix(tools): create export temp file 0o600 so rename never widens backup perms (CR 4088815424)"
```

### Task 4: Deterministic zip timestamps in the shared writer (R6)

**Files:**
- Modify: `src-tauri/src/okf/zip_io.rs` (`write_bundle_zip` options block)
- Test: `src-tauri/src/okf/zip_io.rs` tests — new `zip_bytes_deterministic`

**Interfaces:**
- Consumes: `zip::write::SimpleFileOptions` (zip 8.6).
- Produces: byte-identical zips for identical `(path, content)` sequences, regardless of wall-clock. GUI export path inherits determinism (content unchanged).

- [ ] **Step 1: Write the failing determinism test**

Add to `mod tests` in `src-tauri/src/okf/zip_io.rs`:

```rust
#[test]
fn zip_bytes_deterministic() {
    let files = vec![OkfFile {
        path: "index.md".into(),
        content: "---\nokf_version: 0.2\n---\n".into(),
    }];
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.zip");
    let b = dir.path().join("b.zip");
    write_bundle_zip(&a, &files).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    write_bundle_zip(&b, &files).unwrap();
    let ha = sha256_hex_file(&a).unwrap();
    let hb = sha256_hex_file(&b).unwrap();
    assert_eq!(ha, hb, "identical inputs must produce identical zip bytes");
}

// 64KiB-chunked sha256 helper local to the tests module
fn sha256_hex_file(path: &std::path::Path) -> anyhow::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read as _;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p curated-thoughts --lib okf::zip_io::tests::zip_bytes_deterministic`
Expected: FAIL (zip 8.6 defaults `last_modified_time` to now, so the two archives differ).

- [ ] **Step 3: Pin the timestamp in the writer options**

In `write_bundle_zip`, replace the options construction:

```rust
let options = zip::write::SimpleFileOptions::default()
    .compression_method(zip::CompressionMethod::Deflated);
```

with:

```rust
// Fixed mtime (2026-01-01 00:00:00 UTC) so identical content produces
// byte-identical archives — required for nightly-backup change detection.
let fixed = zip::DateTime::from_date_and_time(2026, 1, 1, 0, 0, 0)
    .unwrap_or_default();
let options = zip::write::SimpleFileOptions::default()
    .compression_method(zip::CompressionMethod::Deflated)
    .last_modified_time(fixed);
```

(Verify the exact constructor name against the `zip` 8.6 docs — candidates: `DateTime::from_date_and_time(y, m, d, h, min, s)` or `Default`-then-`last_modified_time`; adjust if the API differs. If `DateTime` validation rejects year 2026, use the earliest valid date instead — any fixed value satisfies the requirement.)

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p curated-thoughts --lib okf::zip_io`
Expected: `zip_bytes_deterministic` PASS and existing `zip_round_trip` PASS.

- [ ] **Step 5: Full frontend + lib regression**

Run: `cargo test -p curated-thoughts --lib okf && pnpm exec vitest run src/__tests__/OkfInteropBar.test.tsx src/__tests__/axe-core.test.ts`
Expected: all PASS (GUI export path is content-identical, only metadata changed).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/okf/zip_io.rs
git commit -m "feat(okf): pin zip entry mtimes for byte-deterministic bundles"
```

### Task 5: Live verification + review closure

**Files:**
- Modify: none (verification only)
- Reference: `~/.hermes/scripts/okf-bundle-sync.sh` (cron contract check)

**Interfaces:**
- Consumes: fully built branch artifacts.
- Produces: evidence for PR close-out.

- [ ] **Step 1: Live exporter run — permissions, determinism, restorability**

Run:
```bash
cd ~/code/github/equationalapplications/curated-thoughts
cargo build --release -p curated-thoughts-tools --bin export_okf_bundle
./target/release/export_okf_bundle /tmp/okf-t5/x.zip && stat -c '%a' /tmp/okf-t5/x.zip
sleep 1
./target/release/export_okf_bundle /tmp/okf-t5/y.zip
sha256sum /tmp/okf-t5/x.zip /tmp/okf-t5/y.zip
unzip -t /tmp/okf-t5/x.zip > /dev/null && echo "integrity OK"
```
Expected: perms `600`; the two digests match only if the vault was untouched in the sleep window — if they differ, that is live drift, not a failure (re-run back-to-back to confirm identical); integrity OK. Clean up `/tmp/okf-t5` afterwards.

- [ ] **Step 2: Redeploy the binary to the cron path**

Run: `cp target/release/export_okf_bundle ~/.hermes/bin/export_okf_bundle && ~/.hermes/bin/export_okf_bundle /tmp/okf-deploy-check.zip && rm /tmp/okf-deploy-check.zip`
Expected: exit 0 — tonight's cron runs the final build.

- [ ] **Step 3: Answer/resolve all three CodeRabbit threads**

Use the GH GraphQL mutation `resolveReviewThread` for thread IDs on comments `4088815418`, `4088815424`, `4088815428` (fetch thread IDs via `gh api graphql` query `repository.pullRequest.reviewThreads`), replying first with a one-line resolution note each (commit hash + what was done). Alternatively reply on each thread and let Kurt mark resolved during merge.

- [ ] **Step 4: Clippy gate**

Run: `cargo clippy -p curated-thoughts-tools --bin export_okf_bundle 2>&1 | grep -c "^warning\|^error"` (expect 0) — repo enforces clippy clean.

- [ ] **Step 5: Push and confirm CI green; report ready-to-merge**

Run: `git push origin fix/okf-dialog-filter-and-exporter && gh pr checks 226 --watch`
Expected: all checks pass (CodeRabbit may re-review; resolve any new actionable findings through the same verify→fix cycle).

No commit for this task (verification-only), unless a finding forces a fix — then a `fix(tools): …` commit per the same pattern.
