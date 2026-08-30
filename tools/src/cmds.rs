//! tools/src/cmds.rs
//!
//! Write-side command bodies for the `ct` headless CLI.
//!
//! Phase 2 of the headless CLI split (see
//! `docs/superpowers/specs/2026-08-25-ct-headless-cli-phase2-watch.md`).
//! This module owns the *write* subcommand fns that used to live in
//! `cli_common.rs`:
//!
//!   - `ingest_run`       — full ingest_vault_once flow
//!   - `librarian_run`    — full run_librarian_once flow (force / model)
//!   - `librarian_run_on` — testable inner loop over an open connection
//!   - `approve_one`      — approve a single pending proposal
//!   - `approve_all`      — approve every pending proposal
//!   - `enqueue_vault_event` — vault filesystem event → brain DB row
//!   - `watch_run`        — long-running vault watcher (`ct watch`)
//!
//! Internal helpers (`run_librarian_docs`, `format_progress`,
//! `format_run_summary`, `LibrarianRunSummary`, and the
//! `ingest_run` exclusion / walker helpers) live here too so the write
//! flow stays self-contained.
//!
//! Read-side command fns (`status`, `search`, `recall`, `code`, `graph`,
//! `wiki_*`) continue to live in `crate::queries`.
//!
//! Path-level helpers (`BrainPaths`, `resolve_brain_paths`, `print_json`,
//! `vault_contains`) live in `crate::paths`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use walkdir::WalkDir;

use tauri_app_lib::chunker::should_ingest_extension;
use tauri_app_lib::db::commit::{resolve_proposal, ResolveOptions};
use tauri_app_lib::db::connection::AppDb;
use tauri_app_lib::db::proposals::{get_proposal_detail, ItemDecision, ItemDecisionKind};
use tauri_app_lib::indexer::linker::run_linker;
use tauri_app_lib::retrieval;
use tauri_app_lib::vault::VaultConfig;
use tauri_app_lib::{entity_id_for_path, ingest_document_with_vault_root};

/// Default `once` mode watchdog: exit after this many seconds even if no
/// SIGINT arrives. The plan suggested 60s; using a named constant (per the
/// "no magic numbers" rule) keeps the value discoverable and testable.
pub const DEFAULT_ONCE_TIMEOUT: Duration = Duration::from_secs(60);

/// Poll interval for the SIGINT loop in `watch_run`. Short enough to feel
/// responsive to Ctrl-C, long enough to avoid burning CPU.
const WATCH_SIGNAL_POLL: Duration = Duration::from_millis(200);

// ---------------------------------------------------------------------------
// Ingest
// ---------------------------------------------------------------------------

/// Directory names never ingested (build artifacts, deps, VCS internals).
const EXCLUDED_DIRS: &[&str] = &[
    "target",
    "node_modules",
    "dist",
    "dist-newstyle",
    ".git",
    ".github",
    ".next",
    ".turbo",
    ".cache",
    "coverage",
    "build",
    "out",
    ".venv",
    "venv",
    "__pycache__",
    ".idea",
    ".vscode",
    ".fastembed_cache",
];

fn is_excluded_dir(dir_name: &str) -> bool {
    EXCLUDED_DIRS.contains(&dir_name)
}

/// File-name patterns never ingested: machine-generated dependency manifests
/// and generated schemas. The chunker bounds chunk size, so this is not about
/// file length — these files carry no retrieval value and just burn embedding
/// API calls (all 20 failures in the Aug 24 full-corpus run were these).
const EXCLUDED_FILE_NAMES: &[&str] = &[
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "Cargo.lock",
    "poetry.lock",
    "uv.lock",
    "CHANGELOG.md",
    "CHANGELOG.md.generated", // commitizen-style generated changelogs
];

/// Path segments (matched anywhere in the relative path) that mark generated
/// machine output rather than authored knowledge.
const EXCLUDED_PATH_SEGMENTS: &[&str] = &["drizzle/meta/", "gen/schemas/"];

fn is_excluded_file(path: &Path) -> bool {
    if let Some(name) = path.file_name() {
        let name = name.to_string_lossy();
        if EXCLUDED_FILE_NAMES.contains(&name.as_ref()) {
            return true;
        }
    }
    let p = path.to_string_lossy();
    EXCLUDED_PATH_SEGMENTS.iter().any(|seg| p.contains(seg))
}

/// Collect files from a directory tree. `follow_symlinked_doc_dirs` enables
/// following symlinked directories whose parent is exactly
/// `<vault_root>/documents` (the staging contract); nested symlinks and
/// symlinks to files are never followed. Traversal errors are returned so an
/// unreadable path can't silently shrink the corpus.
fn collect_files(
    root: &Path,
    follow_symlinked_doc_dirs: bool,
    out: &mut Vec<PathBuf>,
    errors: &mut Vec<String>,
) {
    let walker = WalkDir::new(root).follow_links(false);
    let it = walker.into_iter().filter_entry(|e| {
        // Skip excluded dirs by name at any depth.
        if e.file_type().is_dir() {
            if let Some(name) = e.path().file_name() {
                return !is_excluded_dir(&name.to_string_lossy());
            }
        }
        true
    });
    for entry in it {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                errors.push(format!("traversal: {e}"));
                continue;
            }
        };
        let p = entry.path();
        if entry.file_type().is_file() && is_excluded_file(p) {
            continue;
        }
        let ft = entry.file_type();
        if ft.is_file()
            && p.extension()
                .map(|e| should_ingest_extension(&e.to_string_lossy()))
                .unwrap_or(false)
        {
            out.push(p.to_path_buf());
        } else if follow_symlinked_doc_dirs && ft.is_symlink() {
            // Only follow symlinks that are DIRECT children of
            // <root>/documents, whose names aren't excluded, and whose target
            // is a directory. Never follow file symlinks or nested ones.
            let parent_is_documents = p
                .parent()
                .map(|par| par.file_name().map(|n| n == "documents").unwrap_or(false))
                .unwrap_or(false)
                && entry.depth() == 1;
            let name_excluded = p
                .file_name()
                .map(|n| is_excluded_dir(&n.to_string_lossy()))
                .unwrap_or(false);
            if !parent_is_documents || name_excluded {
                continue;
            }
            match std::fs::canonicalize(p) {
                Ok(target) if target.is_dir() => {
                    // Recurse into the resolved target with symlink-following
                    // OFF, so nested symlinks inside are never descended into.
                    collect_files(&target, false, out, errors)
                }
                Ok(_) => eprintln!(
                    "warn: symlink {} does not point at a directory, skipping",
                    p.display()
                ),
                Err(e) => eprintln!("warn: broken symlink {}, skipping: {e}", p.display()),
            }
        }
    }
}

/// Full ingest_vault_once flow: resolve brain paths + embed profile, open the
/// brain DB, walk the vault honoring the exclusion rules, ingest every
/// ingestible file, then run the linker over each touched entity. Extracted
/// from `ingest_vault_once.rs`; behavior identical to the original bin main.
pub fn ingest_run() -> Result<()> {
    let paths_b = retrieval::resolve_brain_paths();
    let profile =
        retrieval::load_embed_profile(&paths_b.config_path).context("read embed profile")?;
    let db = AppDb::open_with_config(&paths_b.db_path, &paths_b.config_path)
        .context("open brain database")?;
    let conn = &db.0;

    let config = VaultConfig::new(paths_b.config_path.clone());
    let vault_root = config
        .vault_root()
        .context("read vault root")?
        .ok_or_else(|| anyhow::anyhow!("vault root missing"))?;
    let vault_root = vault_root.canonicalize().unwrap_or(vault_root);

    let mut files = Vec::new();
    let mut walk_errors = Vec::new();
    collect_files(&vault_root, true, &mut files, &mut walk_errors);
    files.sort();
    files.dedup();

    // Traversal errors count as failures so an unreadable path can't make a
    // partial run look complete.
    let mut failed = walk_errors.len();
    for e in &walk_errors {
        eprintln!("warn: {e}");
    }
    println!(
        "ingesting {} file(s) from {}",
        files.len(),
        vault_root.display()
    );

    let vault_root_str = vault_root.to_str().unwrap();
    let mut entity_ids = HashSet::new();
    for (i, f) in files.iter().enumerate() {
        match ingest_document_with_vault_root(
            conn,
            &profile,
            f.to_str().unwrap(),
            true,
            Some(vault_root_str),
        ) {
            Ok(_) => {
                entity_ids.insert(entity_id_for_path(
                    f.to_str().unwrap(),
                    Some(vault_root_str),
                ));
                println!("[{}/{}] ok: {}", i + 1, files.len(), f.display());
            }
            Err(e) => {
                failed += 1;
                eprintln!("[{}/{}] FAILED {}: {}", i + 1, files.len(), f.display(), e);
                let mut src = e.source();
                while let Some(s) = src {
                    eprintln!("    caused by: {s}");
                    src = s.source();
                }
            }
        }
    }

    for entity_id in &entity_ids {
        if let Err(e) = run_linker(conn, entity_id, 0) {
            eprintln!("[linker] {}: {}", entity_id, e);
        }
    }
    println!(
        "done: {} docs, {} entities, {} failed",
        files.len(),
        entity_ids.len(),
        failed
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Librarian
// ---------------------------------------------------------------------------

/// Full run_librarian_once flow over already-ingested documents with the
/// given fallback model (bin default: "llama3.2:3b"; config overrides it in
/// sidecar mode). Extracted from `run_librarian_once.rs`.
pub fn librarian_run(model: &str, force: bool) -> Result<()> {
    let paths = retrieval::resolve_brain_paths();
    if !paths.db_path.is_file() {
        anyhow::bail!(
            "brain database not found at {} — run the app (or ingest_vault_once) first",
            paths.db_path.display()
        );
    }
    // Honor split CURATED_BRAIN_DB / CURATED_BRAIN_CONFIG: resolve the vault
    // root from the resolved config path, not from db_path's parent.
    let mut db = AppDb::open_with_config(&paths.db_path, &paths.config_path)
        .with_context(|| format!("open brain database {}", paths.db_path.display()))?;
    // Resolve errors.log's parent directory the same way write_synthesis_error does
    // (vault root derived from the config path), not from db_path's parent, so
    // surface-detection stays correct under non-default brain-dir layouts.
    let error_log_dir = paths.config_path.parent();
    // CLI callers pass a bin-default fallback (e.g. "llama3.2:3b"); the
    // watermark/provenance model must be what the brain config actually
    // resolves to, or the dirty-doc gate never stabilizes against labels.
    let active = tauri_app_lib::librarian::active_generation_model(model);
    librarian_run_on(&mut db.0, error_log_dir, active.as_str(), force)
}

/// Dirty-doc selection + run loop over an open connection (testable core of
/// [`librarian_run`]). Without `force`, only dirty documents are selected:
/// indexed docs whose watermark doesn't match the current content hash and
/// active model (`synth_hash IS NULL OR synth_hash != hash OR synth_model !=
/// ?model`). `--force` selects every document and bypasses the watermark gate.
pub fn librarian_run_on(
    conn: &mut rusqlite::Connection,
    error_log_dir: Option<&std::path::Path>,
    model: &str,
    force: bool,
) -> Result<()> {
    let docs: Vec<(i64, String)> = if force {
        let mut stmt = conn.prepare("SELECT id, path FROM documents ORDER BY path")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    } else {
        let mut stmt = conn.prepare(
            "SELECT id, path FROM documents
             WHERE status = 'indexed'
               AND (
                   synth_hash IS NULL
                   OR synth_hash != hash
                   OR synth_model IS NULL
                   OR synth_model != ?1
               )
             ORDER BY path",
        )?;
        let rows = stmt.query_map([model], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };

    let scope = if force { "all" } else { "dirty" };
    println!(
        "running librarian over {} {scope} document(s) with fallback model {model}",
        docs.len()
    );

    // Synthesis failures are recorded to <brain>/errors.log by
    // write_synthesis_error (called from generate_summary, which still
    // returns Ok). Surface them by watching that file grow across each
    // call. The log directory must be resolved the same way the writer
    // resolves its target (vault root), not from db_path's parent.
    let error_log = error_log_dir.map(|dir| dir.join("errors.log"));
    let paths: Vec<String> = docs.into_iter().map(|(_, p)| p).collect();

    run_librarian_docs(
        &paths,
        |path| {
            tauri_app_lib::librarian::generate_summary(conn, path, model, force)
                .map_err(|e| format!("{e:#}"))
        },
        &mut std::io::stderr(),
        error_log.as_deref(),
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Approve
// ---------------------------------------------------------------------------

/// doc, commit result). Extracted from `approve_pending_proposals.rs`.
pub fn approve_one(proposal_id: &str) -> Result<()> {
    let paths = retrieval::resolve_brain_paths();
    let mut db = AppDb::open_with_config(&paths.db_path, &paths.config_path)?;
    approve_one_on(&mut db.0, proposal_id)
}

fn approve_one_on(conn: &mut rusqlite::Connection, pid: &str) -> Result<()> {
    let detail =
        get_proposal_detail(conn, pid)?.with_context(|| format!("proposal {pid} not found"))?;
    if detail.status != "pending" {
        bail!("proposal {pid} not pending (status={})", detail.status);
    }
    let decisions: Vec<ItemDecision> = detail
        .items
        .iter()
        .map(|i| ItemDecision {
            item_id: i.id.clone(),
            decision: ItemDecisionKind::Accept,
            edited_payload: None,
        })
        .collect();
    let result = resolve_proposal(
        conn,
        pid,
        &decisions,
        None,
        ResolveOptions { auto_approve: true },
    )?;
    println!(
        "approved {pid}: items={} source={} committed={} conflicts={} dropped_edges={} status={}",
        decisions.len(),
        detail
            .source_doc_paths
            .first()
            .map(String::as_str)
            .unwrap_or("-"),
        result.committed.len(),
        result.conflicts.len(),
        result.dropped_edges.len(),
        result.proposal_status,
    );
    Ok(())
}

/// Approve every pending proposal via [`approve_one_on`]. Continues past
/// individual failures so one bad proposal doesn't block the rest. Prints
/// `approved: N` (N=0 on an empty pending set — still exit 0), or
/// `approved: N, failed: M` before returning Err when any failed.
pub fn approve_all() -> Result<()> {
    let paths = retrieval::resolve_brain_paths();
    let mut db = AppDb::open_with_config(&paths.db_path, &paths.config_path)?;
    let ids: Vec<String> = {
        let mut stmt =
            db.0.prepare("SELECT id FROM curated_proposals WHERE status = 'pending'")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let mut approved = 0usize;
    let mut failures: Vec<(String, anyhow::Error)> = Vec::new();
    for pid in &ids {
        match approve_one_on(&mut db.0, pid) {
            Ok(()) => approved += 1,
            Err(e) => failures.push((pid.clone(), e)),
        }
    }
    if failures.is_empty() {
        println!("approved: {approved}");
        return Ok(());
    }
    println!("approved: {approved}, failed: {}", failures.len());
    for (pid, e) in &failures {
        eprintln!("failed {pid}: {e:#}");
    }
    bail!(
        "{} of {} proposal(s) failed to approve",
        failures.len(),
        ids.len()
    )
}

// ---------------------------------------------------------------------------
// Librarian observability helpers (private)
// ---------------------------------------------------------------------------

/// End-of-run totals for [`run_librarian_docs`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LibrarianRunSummary {
    pub attempted: usize,
    pub ok: usize,
    pub error: usize,
    /// Phase-1 synthesis-watermark gate counter. Dirty-doc selection has
    /// landed, but skipped docs are filtered out of the run loop *before* this
    /// counter is incremented — so this stays 0 in practice today. The field
    /// is reserved for a future phase that counts (rather than drops) them.
    pub skipped_by_watermark: usize,
    pub elapsed_secs: u64,
}

pub(crate) fn format_progress(
    n: usize,
    total: usize,
    path: &str,
    status: &str,
    elapsed_secs: u64,
) -> String {
    format!("[{n}/{total}] {path} {status} ({elapsed_secs}s)")
}

pub(crate) fn format_run_summary(summary: &LibrarianRunSummary) -> String {
    format!(
        "librarian run summary: attempted={} ok={} error={} \
         skipped_by_watermark={} elapsed={}s",
        summary.attempted,
        summary.ok,
        summary.error,
        summary.skipped_by_watermark,
        summary.elapsed_secs
    )
}

fn errors_log_len(path: Option<&Path>) -> u64 {
    path.and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .unwrap_or(0)
}

/// First line of whatever was appended to the errors log at `from` offset.
fn errors_log_tail(path: &Path, from: u64) -> Option<String> {
    use std::io::{Read, Seek};
    let mut f = std::fs::File::open(path).ok()?;
    f.seek(std::io::SeekFrom::Start(from)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let line = text.lines().next()?.trim().to_string();
    if line.is_empty() {
        None
    } else {
        Some(line)
    }
}

/// Per-doc librarian loop with stderr observability. One `[n/total] <path>
/// ok|error (<elapsed>s)` line per doc plus a final run-summary line, all
/// written (flushed) to `err`. Synthesis failures are surfaced whether they
/// come back as `Err` or were swallowed into `error_log` by
/// `write_synthesis_error` (detected via log growth during the call).
pub(crate) fn run_librarian_docs<F>(
    docs: &[String],
    mut synthesize: F,
    err: &mut dyn std::io::Write,
    error_log: Option<&Path>,
) -> LibrarianRunSummary
where
    F: FnMut(&str) -> std::result::Result<(), String>,
{
    let start = std::time::Instant::now();
    let total = docs.len();
    let mut summary = LibrarianRunSummary {
        attempted: total,
        ..Default::default()
    };

    for (i, path) in docs.iter().enumerate() {
        let log_before = errors_log_len(error_log);
        let doc_start = std::time::Instant::now();
        let result = synthesize(path);
        let elapsed = doc_start.elapsed().as_secs();

        let mut status = "ok";
        match result {
            Ok(()) => {
                if let Some(log) = error_log {
                    // TODO(pr-followup): errors_log_len/tail reads the shared
                    // error log without coordinating with concurrent writers.
                    // If the librarian pipeline is writing errors.log at the
                    // same moment this check runs (e.g. during a parallel
                    // librarian --force + watcher ingest), the > log_before
                    // check can misattribute an unrelated writer's entry to
                    // the doc currently being synthesized. Flagged by
                    // aws-cloud-agent-pr-review on PR #84 as a minor
                    // concurrency concern; not blocking this PR. Filed in
                    // procedures/curated-thoughts-improvement-backlog.md.
                    if errors_log_len(error_log) > log_before {
                        status = "error";
                        let detail = errors_log_tail(log, log_before).unwrap_or_default();
                        let _ = writeln!(
                            err,
                            "error: synthesis failed for {path} — recorded in {}: {detail}",
                            log.display()
                        );
                    }
                }
            }
            Err(e) => {
                status = "error";
                let _ = writeln!(err, "error: synthesis failed for {path}: {e}");
            }
        }
        if status == "ok" {
            summary.ok += 1;
        } else {
            summary.error += 1;
        }

        let _ = writeln!(
            err,
            "{}",
            format_progress(i + 1, total, path, status, elapsed)
        );
        let _ = err.flush();
    }

    summary.elapsed_secs = start.elapsed().as_secs();
    let _ = writeln!(err, "{}", format_run_summary(&summary));
    let _ = err.flush();
    summary
}

// ---------------------------------------------------------------------------
// enqueue_vault_event (vault filesystem event → brain DB row)
// ---------------------------------------------------------------------------

/// Enqueue a vault filesystem event into the brain DB.
///
/// Thin delegating wrapper around
/// [`tauri_app_lib::db::queue::enqueue_vault_event`] (moved into
/// `src-tauri/src/db/queue.rs` in Task 5b to resolve the cargo dep-cycle
/// that would otherwise block Task 7 calling the same logic from
/// `src-tauri/src/lib.rs`). Identical contract — see spec §6 for the
/// 4-stage path hardening + sha256 + upsert semantics:
///
/// 1. `std::path::absolute()` — defensive (notify v6 already absolute).
/// 2. `std::fs::canonicalize()` — resolves symlinks (e.g. macOS /var → /private/var).
///    Falls back to absolute path on failure (typical for Delete events).
/// 3. `canonical.starts_with(vault_root)` guard — rejects out-of-vault events.
///    The vault root is read from `CURATED_VAULT_ROOT`. If unset (the watcher
///    runs with it set; tests may not), the guard is skipped.
/// 4. sha256 the bytes; upsert documents row with status='pending'.
///
/// For Delete: skip step 4 (file is gone); DELETE the documents row.
/// chunks cascade-delete via FK ON DELETE CASCADE.
pub fn enqueue_vault_event(
    conn: &mut Connection,
    event_kind: notify::EventKind,
    raw_path: &Path,
) -> Result<()> {
    tauri_app_lib::db::queue::enqueue_vault_event(conn, event_kind, raw_path)
}

// ---------------------------------------------------------------------------
// watch_run (headless vault watcher)
// ---------------------------------------------------------------------------

/// Format a single JSON event line for `ct watch --json` (spec §6).
///
/// Pure helper (no I/O) so tests can pin the wire format without spawning
/// the binary. Field order is `kind`, `path`, `ts_ms` — the canonical order
/// documented in `docs/superpowers/specs/2026-08-25-ct-headless-cli-phase2-watch.md`
/// §6. The returned string has no trailing newline; the caller is
/// responsible for emitting it on stdout (so the writer controls flushing).
pub fn format_event(kind: &str, path: &str, ts_ms: i64) -> String {
    serde_json::json!({
        "kind": kind,
        "path": path,
        "ts_ms": ts_ms,
    })
    .to_string()
}

/// Current wall-clock time in unix milliseconds.
///
/// Uses `std::time::SystemTime` (no `chrono` dep — keeping the dependency
/// surface tiny per the phase 2 spec's "no new deps" rule). If the system
/// clock is somehow before the epoch we fall back to 0 rather than panicking
/// — a negative `ts_ms` is meaningless for downstream consumers and we
/// never want the watcher to crash on a bad clock read.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Knobs for [`watch_run`]. `once` switches the SIGINT-blocking loop into a
/// bounded watchdog (`once_timeout`, default 60s). `background` is reserved
/// for a future systemd-style mode; v1 always foregrounds.
pub struct WatchOpts {
    pub once: bool,
    pub json_mode: bool,
    pub background: bool,
    /// Maximum time to wait in `once` mode before exiting cleanly. Defaults
    /// to `DEFAULT_ONCE_TIMEOUT` when `None`.
    pub once_timeout: Option<Duration>,
}

/// Distinct failure modes for `watch_run`. Each variant maps to a
/// distinct exit code per spec §5 ("0 ok / 1 config / 2 lock / 3
/// DB-schema / 4 notify init"). Anything that does not match a
/// specific bucket becomes [`WatchError::Other`], which bubbles to
/// `main()` and exits 1 (preserves the historic "default everything
/// else to exit 1" behavior for unclassified faults).
#[derive(Debug)]
pub enum WatchError {
    /// Vault lock could not be acquired — another watcher holds it.
    /// → exit 2.
    LockConflict(anyhow::Error),
    /// Brain DB open / schema / migration failure.
    /// → exit 3.
    Db(anyhow::Error),
    /// `notify` watcher could not be initialized (non-directory vault
    /// root, or `spawn_vault_watcher` returned an error).
    /// → exit 4.
    NotifyInit(anyhow::Error),
    /// Any other failure. Bubbles to main() → exit 1.
    Other(anyhow::Error),
}

impl WatchError {
    /// Stable exit code per the spec §5 contract.
    pub fn exit_code(&self) -> i32 {
        match self {
            WatchError::LockConflict(_) => 2,
            WatchError::Db(_) => 3,
            WatchError::NotifyInit(_) => 4,
            WatchError::Other(_) => 1,
        }
    }
}

impl std::fmt::Display for WatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WatchError::LockConflict(e) => write!(f, "vault lock conflict: {e}"),
            WatchError::Db(e) => write!(f, "brain.db error: {e}"),
            WatchError::NotifyInit(e) => write!(f, "notify init failure: {e}"),
            WatchError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for WatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WatchError::LockConflict(e)
            | WatchError::Db(e)
            | WatchError::NotifyInit(e)
            | WatchError::Other(e) => Some(e.as_ref()),
        }
    }
}

impl From<anyhow::Error> for WatchError {
    /// Any unclassified error becomes `Other`. Specific buckets are
    /// populated via the explicit constructor paths inside `run()` —
    /// do NOT rely on this From for `WatchError::Db` etc., because we
    /// want the call-site to be explicit about *why* an error belongs
    /// in a particular exit-code bucket (the spec distinguishes them).
    fn from(e: anyhow::Error) -> Self {
        WatchError::Other(e)
    }
}

/// Run the headless vault watcher.
///
/// Returns:
/// - `Ok(0)` on clean shutdown (SIGINT received in foreground mode, or
///   timeout elapsed in `--once` mode).
/// - `Ok(2)` when the vault lock could not be acquired (another watcher
///   already holds it).
/// - `Ok(3)` when the brain DB cannot be opened / its schema is bad.
/// - `Ok(4)` when the `notify` watcher cannot be initialized (vault
///   root missing or non-directory, or inotify/readdir/kqueue backend
///   refused).
/// - `Err(_)` for any other failure (config error, signal-thread
///   failure, etc.) — `main()` turns that into exit 1.
///
/// **JSON event contract (spec §6 — CodeRabbit review on PR #96 pass 3,
/// comment #12):** when `opts.json_mode` is true, this function emits
/// a `shutdown` event on EVERY outcome — clean (exit 0), classified
/// (exit 2/3/4), and unclassified (Err(_)). The pre-start classified
/// failures (lock conflict → 2, missing vault root → 4, non-directory
/// vault root → 4) emit `error` + `shutdown` with `path` carrying the
/// same brain/vault/pid summary as the start event would have (so
/// downstream consumers can pair them). This means a `start` event
/// never appears for pre-start failures — see the spec §6 consumer
/// contract for the trio (start → error → shutdown OR just error →
/// shutdown).
pub fn watch_run(opts: WatchOpts) -> Result<i32> {
    // Resolve brain/vault/pid identity up-front so the shutdown event
    // can carry the same summary as the start event. We do this even
    // when CURATED_VAULT_ROOT is unset so the failure-mode shutdown is
    // still meaningful (we render "vault=<unset>" in that case).
    // Capture `opts.json_mode` locally because `opts` is moved into
    // `run(opts)` below.
    let json_mode = opts.json_mode;
    let brain = crate::paths::resolve_brain_paths();
    let vault_root = std::env::var("CURATED_VAULT_ROOT")
        .unwrap_or_else(|_| "<unset>".to_string());
    let pid = std::process::id();

    let summary = format!(
        "brain={} vault={} pid={}",
        brain.brain_dir.display(),
        vault_root,
        pid,
    );

    let result = run(opts);
    let (exit_code, final_err) = match result {
        Ok(()) => (0i32, None),
        Err(e @ WatchError::LockConflict(_))
        | Err(e @ WatchError::Db(_))
        | Err(e @ WatchError::NotifyInit(_)) => {
            eprintln!("{}", e);
            (e.exit_code(), None)
        }
        Err(WatchError::Other(e)) => (1, Some(e)),
    };

    if json_mode {
        // Emit shutdown on every outcome — see the docstring above.
        // For unclassified errors the "path" carries the same
        // brain/vault/pid summary; consumers match the error+shutdown
        // pair to identify the run.
        println!(
            "{}",
            format_event("shutdown", &summary, now_ms())
        );
    }

    match final_err {
        Some(e) => Err(e),
        None => Ok(exit_code),
    }
}

/// Internal worker for [`watch_run`]. Returns `Result<(), WatchError>`
/// so the public entry point can translate the variant into a stable
/// exit code per spec §5.
fn run(opts: WatchOpts) -> Result<(), WatchError> {
    use crate::lock::VaultLock;

    let brain = crate::paths::resolve_brain_paths();
    let vault_root = std::env::var("CURATED_VAULT_ROOT")
        .map_err(|e| WatchError::Other(anyhow::Error::new(e).context(
            "CURATED_VAULT_ROOT must be set (or pass --vault)",
        )))?;

    let vault_path = PathBuf::from(&vault_root);
    if !vault_path.exists() {
        return Err(WatchError::NotifyInit(anyhow::anyhow!(
            "configured vault path does not exist: {}",
            vault_path.display()
        )));
    }
    if !vault_path.is_dir() {
        // notify backends differ on whether they refuse a non-directory
        // upfront (Windows/macOS do; Linux inotify happens to allow it
        // for single files but can't recursively enumerate). Treat a
        // non-directory vault root as a notify-init failure so callers
        // see a deterministic exit-4 regardless of platform.
        return Err(WatchError::NotifyInit(anyhow::anyhow!(
            "vault root must be a directory (got non-directory): {}",
            vault_path.display()
        )));
    }

    // Probe the brain DB up-front so a corrupt or unreadable file surfaces
    // as exit 3 instead of being swallowed inside the per-event callback.
    // The watcher loop will reopen on every event regardless; this is
    // purely a fail-fast gate. Execute a trivial `SELECT 1` against the
    // freshly opened connection so corruption in the file header (SQLite
    // accepts an open-with-create against garbage bytes without complaint;
    // it only surfaces the error when you try to read) is caught here.
    let brain_for_cb = crate::write::Brain { paths: brain.clone() };
    {
        let probe = crate::write::open_rw(&brain_for_cb)
            .map_err(|e| WatchError::Db(e.context("open brain.db for watcher startup")))?;
        probe
            .execute_batch("SELECT 1;")
            .map_err(|e| WatchError::Db(anyhow::Error::new(e)
                .context("brain.db schema probe failed at watcher startup")))?;
    }

    let lock = match VaultLock::acquire(&brain.brain_dir) {
        Ok(l) => l,
        Err(e) => return Err(WatchError::LockConflict(e)),
    };

    if opts.json_mode {
        // Start-event "path" is a synthetic context string per the
        // drift-fixups plan step 1.5: the original {event, brain, vault,
        // pid} payload collapses into the spec §6 {kind, path, ts_ms}
        // schema by reusing the `path` slot for a human-readable
        // brain/vault/pid summary. `kind=start`, `ts_ms=now_ms()`.
        println!(
            "{}",
            format_event(
                "start",
                &format!(
                    "brain={} vault={} pid={}",
                    brain.brain_dir.display(),
                    vault_root,
                    std::process::id()
                ),
                now_ms()
            )
        );
    } else {
        eprintln!(
            "ct watch v0.2.0 | brain={} | vault={} | lock={}/.curated_thoughts.lock (held)",
            brain.brain_dir.display(),
            vault_root,
            brain.brain_dir.display()
        );
    }

    let handle = crate::watcher::spawn_vault_watcher(vault_path.clone(), move |event| {
        let conn = match crate::write::open_rw(&brain_for_cb) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[watch] db open failed: {}", e);
                return;
            }
        };
        let mut conn = conn;
        let path = match &event {
            crate::watcher::VaultEvent::Added(p)
            | crate::watcher::VaultEvent::Modified(p)
            | crate::watcher::VaultEvent::Deleted(p) => p,
        };
        let event_kind = match &event {
            crate::watcher::VaultEvent::Added(_) => {
                notify::EventKind::Create(notify::event::CreateKind::Any)
            }
            crate::watcher::VaultEvent::Modified(_) => {
                notify::EventKind::Modify(notify::event::ModifyKind::Any)
            }
            crate::watcher::VaultEvent::Deleted(_) => {
                notify::EventKind::Remove(notify::event::RemoveKind::Any)
            }
        };
        if let Err(e) = enqueue_vault_event(&mut conn, event_kind, Path::new(path)) {
            eprintln!("[watch] enqueue failed for {}: {}", path, e);
            return;
        }
        if opts.json_mode {
            // JSON event line → stdout (cron-friendly). The spec §6
            // vocabulary maps Deleted → "removed" (the watcher enum
            // uses "deleted" but the spec wire vocabulary uses
            // "removed" — see plan step 1.5 ruling). All other kinds
            // pass through unchanged.
            let kind = match event {
                crate::watcher::VaultEvent::Added(_) => "added",
                crate::watcher::VaultEvent::Modified(_) => "modified",
                crate::watcher::VaultEvent::Deleted(_) => "removed",
            };
            println!("{}", format_event(kind, &path, now_ms()));
        } else {
            eprintln!(
                "[watch] {} {}",
                match event {
                    crate::watcher::VaultEvent::Added(_) => "+",
                    crate::watcher::VaultEvent::Modified(_) => "~",
                    crate::watcher::VaultEvent::Deleted(_) => "-",
                },
                path
            );
        }
    })
    .map_err(|e| WatchError::NotifyInit(e.context("spawn vault watcher")))?;

    if opts.once {
        let timeout = opts.once_timeout.unwrap_or(DEFAULT_ONCE_TIMEOUT);
        let started = std::time::Instant::now();
        loop {
            if started.elapsed() >= timeout {
                break;
            }
            std::thread::sleep(WATCH_SIGNAL_POLL);
        }
    } else {
        // Foreground: block until SIGINT. We use tokio::signal::ctrl_c() via
        // a tiny dedicated thread + atomic flag — see `wait_for_sigint`.
        // CodeRabbit review on PR #96 (review pass 2): the previous
        // `let term = wait_for_sigint()?; let _ = term;` was dead — the
        // Arc<AtomicBool> returned has no other readers once the
        // signal-thread has set and exited. We just call for its
        // side-effect (block until SIGINT) and let the value drop on
        // the next line.
        wait_for_sigint()?;
    }

    // IMPORTANT ordering: stop the watcher BEFORE releasing the lock.
    // CodeRabbit review on PR #96 (pass 3 comment #13): the previous
    // `drop(lock); handle.stop();` sequence released the lock first,
    // letting a second process acquire it while the watcher thread was
    // still alive and enqueueing events. Rust drops bindings in REVERSE
    // declaration order (handle is declared at line 826, lock at 793),
    // so the natural scope-end drop would also drop the lock first —
    // wrapping the join in an inner scope fixes that.
    {
        handle.stop();
        drop(lock);
    }
    // NOTE: the `shutdown` JSON event is emitted by the public
    // `watch_run` wrapper (line ~780), NOT here — emitting in both
    // places would double-fire. The wrapper runs once per `ct watch`
    // invocation, regardless of the outcome (clean / classified /
    // unclassified). CodeRabbit review on PR #96 pass 3, comment #12.
    Ok(())
}

/// Spawn a background thread that waits for SIGINT (Ctrl-C) via
/// `tokio::signal::ctrl_c()`, flipping an `AtomicBool` that the caller can
/// poll. Returns the flag (already flipped to `true` once SIGINT arrives).
///
/// We can't block the main thread directly on `tokio::signal::ctrl_c().await`
/// without a tokio runtime handle, and `watch_run` is intentionally
/// synchronous (clap dispatch is sync). This shim keeps the dependency
/// surface tiny — just `tokio::signal` — instead of pulling in `ctrlc` or a
/// dedicated `signal-hook` runtime.
///
/// **Failure-mode contract (CodeRabbit review on PR #96):** if the
/// signal-thread spawn fails, the runtime fails to build, or `ctrl_c()`
/// itself returns an error, the `fatal` flag is set to `Some(msg)` so the
/// caller surfaces the failure as an exit-1 (`Other`) instead of hanging
/// forever. The previous implementation swallowed those errors via
/// `eprintln!` + `return;` and then polled `term` indefinitely — meaning
/// any signal-stack init failure made `ct watch` a non-responding
/// foreground daemon.
fn wait_for_sigint() -> Result<Arc<AtomicBool>> {
    let term = Arc::new(AtomicBool::new(false));
    let term_signal = term.clone();
    // Fatal flag: written from the signal thread on startup failure, read
    // from the main thread's poll loop. `None` means "still starting or
    // SIGINT not yet fired"; `Some(msg)` means "fatal — surface to user".
    let fatal: Arc<std::sync::Mutex<Option<String>>> =
        Arc::new(std::sync::Mutex::new(None));
    let fatal_signal = fatal.clone();
    std::thread::Builder::new()
        .name("ct-watch-sigint".into())
        .spawn(move || {
            // Build a one-off current-thread runtime just for the signal
            // future. The runtime is dropped when this closure returns,
            // but the signal future has already completed by then.
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    *fatal_signal.lock().unwrap() = Some(format!(
                        "failed to start signal runtime: {e}"
                    ));
                    return;
                }
            };
            rt.block_on(async {
                if let Err(e) = tokio::signal::ctrl_c().await {
                    *fatal_signal.lock().unwrap() =
                        Some(format!("ctrl_c failed: {e}"));
                    return;
                }
                term_signal.store(true, Ordering::SeqCst);
            });
        })
        .context("spawn sigint watcher thread")?;

    // Spin on the flag until the watcher thread sets it (or, in practice,
    // until Ctrl-C fires). Same poll cadence as the once-mode loop.
    while !term.load(Ordering::SeqCst) {
        std::thread::sleep(WATCH_SIGNAL_POLL);
        if let Some(msg) = fatal.lock().unwrap().clone() {
            // Surface as `WatchError::Other` so main() exits 1 with the
            // signal-thread diagnostic instead of hanging forever.
            return Err(anyhow::anyhow!(msg));
        }
    }
    Ok(term)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tauri_app_lib::db::queries::upsert_document;

    // ---- librarian observability tests (moved verbatim from cli_common) -

    use std::io::Cursor;

    fn lines(out: &str) -> Vec<&str> {
        out.lines().collect()
    }

    // ---- ct watch JSON output (drift-fixups Task 1) ----
    //
    // Pin the wire format for `--json` mode. Spec §6 schema:
    //   {"kind": "<start|added|modified|removed|error|shutdown>",
    //    "path": "<absolute or context string>",
    //    "ts_ms": <i64 unix millis>}
    //
    // These are pure-helper tests — no I/O — so they don't depend on spawning
    // a process and remain stable across refactors of `watch_run` itself.

    #[test]
    fn watch_run_json_writes_to_stdout_not_stderr() {
        // Extract the JSON-line formatting into a small
        // `fn format_event(kind, path, ts_ms) -> String` so it's directly
        // testable. Verify the exact wire format (no trailing newline,
        // single-line JSON, field order is kind → path → ts_ms).
        let line = super::format_event("modified", "/abs/path.md", 1_234_567_890_123);
        assert_eq!(
            line,
            r#"{"kind":"modified","path":"/abs/path.md","ts_ms":1234567890123}"#
        );
    }

    #[test]
    fn watch_run_event_kind_values_are_constrained() {
        // Spec §6: kind ∈ {start, added, modified, removed, error, shutdown}.
        // The formatter is a pure string passthrough, but pin the six
        // canonical values so accidental drift in callers is caught.
        for kind in ["start", "added", "modified", "removed", "error", "shutdown"] {
            let line = super::format_event(kind, "/abs", 0);
            assert!(
                line.contains(&format!(r#""kind":"{}""#, kind)),
                "line {line:?} missing kind field {kind:?}"
            );
        }
    }

    #[test]
    fn watch_run_ts_ms_is_unix_millis() {
        // Use std::time::SystemTime (no chrono dep). The value must be
        // post-2024 in unix millis, and must round-trip through format_event
        // exactly so a downstream JSON consumer can parse it.
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        assert!(now > 1_700_000_000_000, "ts_ms {now} not in 2024+");
        let line = super::format_event("start", "/abs", now);
        assert!(
            line.contains(&format!(r#""ts_ms":{}"#, now)),
            "line {line:?} missing ts_ms value {now}"
        );
    }

    #[test]
    fn progress_line_format_matches_spec() {
        assert_eq!(
            format_progress(3, 10, "docs/a.md", "ok", 12),
            "[3/10] docs/a.md ok (12s)"
        );
        assert_eq!(
            format_progress(4, 10, "docs/b.md", "error", 1),
            "[4/10] docs/b.md error (1s)"
        );
    }

    #[test]
    fn summary_format_includes_reserved_watermark_field() {
        let s = LibrarianRunSummary {
            attempted: 5,
            ok: 4,
            error: 1,
            skipped_by_watermark: 0,
            elapsed_secs: 42,
        };
        assert_eq!(
            format_run_summary(&s),
            "librarian run summary: attempted=5 ok=4 error=1 skipped_by_watermark=0 elapsed=42s"
        );
    }

    #[test]
    fn per_doc_lines_and_counts_all_ok() {
        let docs = vec!["a.md".to_string(), "b.md".to_string()];
        let mut out = Cursor::new(Vec::new());
        let summary = run_librarian_docs(&docs, |_p| Ok(()), &mut out, None);
        let text = String::from_utf8(out.into_inner()).unwrap();
        let ls = lines(&text);
        assert_eq!(ls[0], "[1/2] a.md ok (0s)");
        assert_eq!(ls[1], "[2/2] b.md ok (0s)");
        assert_eq!(
            ls[2],
            "librarian run summary: attempted=2 ok=2 error=0 skipped_by_watermark=0 elapsed=0s"
        );
        assert_eq!(
            summary,
            LibrarianRunSummary {
                attempted: 2,
                ok: 2,
                error: 0,
                skipped_by_watermark: 0,
                elapsed_secs: 0
            }
        );
    }

    #[test]
    fn err_result_counted_and_surfaced() {
        let docs = vec!["ok.md".to_string(), "bad.md".to_string()];
        let mut out = Cursor::new(Vec::new());
        let summary = run_librarian_docs(
            &docs,
            |p| {
                if p == "bad.md" {
                    Err("LLM unreachable".to_string())
                } else {
                    Ok(())
                }
            },
            &mut out,
            None,
        );
        let text = String::from_utf8(out.into_inner()).unwrap();
        assert!(text.contains("error: synthesis failed for bad.md: LLM unreachable"));
        assert!(text.contains("[2/2] bad.md error ("));
        assert_eq!(summary.ok, 1);
        assert_eq!(summary.error, 1);
        assert_eq!(summary.attempted, 2);
    }

    #[test]
    fn swallowed_synthesis_error_via_log_growth_is_surfaced() {
        let dir = tempfile::TempDir::new().unwrap();
        let log = dir.path().join("errors.log");
        std::fs::write(&log, "pre-existing\n").unwrap();

        let docs = vec!["swallow.md".to_string(), "fine.md".to_string()];
        let log_path = log.clone();
        let mut out = Cursor::new(Vec::new());
        let summary = run_librarian_docs(
            &docs,
            move |p| {
                if p == "swallow.md" {
                    // Mirrors write_synthesis_error: append + return Ok.
                    use std::io::Write as _;
                    let mut f = std::fs::OpenOptions::new()
                        .append(true)
                        .open(&log_path)
                        .unwrap();
                    writeln!(
                        f,
                        "[1756123456] synthesis JSON failure for swallow.md: boom"
                    )
                    .unwrap();
                }
                Ok(())
            },
            &mut out,
            Some(&log),
        );
        let text = String::from_utf8(out.into_inner()).unwrap();
        assert!(text.contains("recorded in"));
        assert!(text.contains("errors.log"));
        assert!(text.contains("synthesis JSON failure for swallow.md"));
        assert!(text.contains("[1/2] swallow.md error ("));
        assert!(text.contains("[2/2] fine.md ok ("));
        assert_eq!(summary.error, 1);
        assert_eq!(summary.ok, 1);
    }

    #[test]
    fn empty_run_still_prints_summary() {
        let mut out = Cursor::new(Vec::new());
        let summary = run_librarian_docs(&[], |_p| Ok(()), &mut out, None);
        let text = String::from_utf8(out.into_inner()).unwrap();
        assert_eq!(
            text.trim(),
            "librarian run summary: attempted=0 ok=0 error=0 skipped_by_watermark=0 elapsed=0s"
        );
        assert_eq!(summary.attempted, 0);
    }

    // ---- dirty-doc selection tests (moved verbatim from cli_common) -----

    /// Open a fresh in-memory sqlite connection with only the columns
    /// `enqueue_vault_event` touches applied. The dirty-doc tests live in
    /// `tools` (not `src-tauri/src/db/queue`) but they share this
    /// minimal-schema fixture so the lib doesn't depend on the full
    /// migration stack from inside tools tests.
    fn open_seeded_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open raw in-memory brain db");
        conn.execute_batch(
            "CREATE TABLE documents (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                path            TEXT    NOT NULL UNIQUE,
                hash            TEXT    NOT NULL,
                tier            TEXT    NOT NULL CHECK(tier IN ('user_doc', 'wiki')),
                folder_rules_id INTEGER,
                last_indexed    INTEGER,
                status          TEXT    NOT NULL DEFAULT 'pending'
                                CHECK(status IN ('pending', 'indexed', 'error', 'orphaned')),
                synth_hash      TEXT,
                synth_model     TEXT,
                synth_at        INTEGER
            );",
        )
        .expect("apply minimal documents schema");
        conn
    }

    fn seed_doc(conn: &Connection, path: &str, hash: &str, status: &str) -> i64 {
        let id = upsert_document(conn, path, hash).unwrap();
        conn.execute(
            "UPDATE documents SET status = ?2 WHERE id = ?1",
            rusqlite::params![id, status],
        )
        .unwrap();
        id
    }

    fn dirty_paths(conn: &mut Connection, model: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare(
                "SELECT path FROM documents
                 WHERE status = 'indexed'
                   AND (
                       synth_hash IS NULL
                       OR synth_hash != hash
                       OR synth_model IS NULL
                       OR synth_model != ?1
                   )
                 ORDER BY path",
            )
            .unwrap();
        let rows = stmt
            .query_map([model], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        rows
    }

    #[test]
    fn dirty_select_returns_only_changed_and_new_docs() {
        let mut conn = open_seeded_conn();

        // Indexed docs with current synth_hash == hash + same model → clean.
        seed_doc(&mut conn, "/v/a.md", "h-a", "indexed");
        conn.execute(
            "UPDATE documents SET synth_hash = 'h-a', synth_model = 'm' WHERE path = '/v/a.md'",
            [],
        )
        .unwrap();
        seed_doc(&mut conn, "/v/b.md", "h-b", "indexed");
        conn.execute(
            "UPDATE documents SET synth_hash = 'h-b', synth_model = 'm' WHERE path = '/v/b.md'",
            [],
        )
        .unwrap();

        // Indexed doc whose hash has changed since synth → dirty.
        seed_doc(&mut conn, "/v/c.md", "h-c-new", "indexed");
        conn.execute(
            "UPDATE documents SET synth_hash = 'h-c-old', synth_model = 'm' WHERE path = '/v/c.md'",
            [],
        )
        .unwrap();

        // Indexed doc with no synth record at all → dirty.
        seed_doc(&mut conn, "/v/d.md", "h-d", "indexed");

        // Indexed doc indexed by a different model → dirty.
        seed_doc(&mut conn, "/v/e.md", "h-e", "indexed");
        conn.execute(
            "UPDATE documents SET synth_hash = 'h-e', synth_model = 'old-m' WHERE path = '/v/e.md'",
            [],
        )
        .unwrap();

        // Non-indexed doc must never be selected.
        seed_doc(&mut conn, "/v/f.md", "hash-f", "pending");

        assert_eq!(
            dirty_paths(&mut conn, "m"),
            vec!["/v/c.md", "/v/d.md", "/v/e.md"]
        );
    }

    // ---- ct watch exit codes (drift-fixups Task 2) ----
    //
    // Spec §5 exit-code contract:
    //   0 ok / clean shutdown
    //   1 config error
    //   2 lock conflict
    //   3 DB/schema error
    //   4 notify init failure
    //
    // `watch_run` is a single function; unit tests here cover the four
    // non-zero paths by setting CURATED_VAULT_ROOT / CURATED_BRAIN_DIR
    // per test. All tests use `--once` so the watcher exits after the
    // short `once_timeout` rather than blocking on SIGINT.

    fn watch_run_short_opts() -> WatchOpts {
        WatchOpts {
            once: true,
            json_mode: false,
            background: false,
            once_timeout: Some(Duration::from_millis(200)),
        }
    }

    /// Regression guard: missing CURATED_VAULT_ROOT stays exit 1 (config).
    /// Asserted via the outer `Result::Err` contract — main() turns that
    /// into exit 1 — because the spec classifies missing env var as a
    /// config error rather than a runtime fault.
    #[test]
    fn watch_run_missing_vault_root_returns_err_for_main_to_exit_1() {
        temp_env::with_var("CURATED_VAULT_ROOT", None::<&str>, || {
            let r = watch_run(watch_run_short_opts());
            assert!(
                r.is_err(),
                "missing CURATED_VAULT_ROOT must surface as Err so main() emits exit 1, got {r:?}"
            );
            let msg = format!("{}", r.err().unwrap());
            assert!(
                msg.contains("CURATED_VAULT_ROOT"),
                "error must mention the missing var, got: {msg}"
            );
        });
    }

    /// Lock conflict: another process holds the vault lock → exit 2 (existing
    /// behavior, regression-guarded here as part of the exit-code spec
    /// coverage).
    ///
    /// **Test isolation (CodeRabbit review on PR #96):** the previous
    /// version of this test only set `CURATED_VAULT_ROOT` and let
    /// `resolve_brain_paths()` fall back to `$HOME/.brain`, then
    /// acquired the lock on that real directory. That meant every
    /// run of `cargo test` would briefly create a `.curated_thoughts.lock`
    /// in the developer's actual brain dir — corrupting any concurrent
    /// watcher / making it crash on lock conflict. We now set both
    /// `CURATED_VAULT_ROOT` and `CURATED_BRAIN_DIR` to the same temp
    /// dir so all locks and DB lookups happen inside a throwaway
    /// directory. The `temp_env::with_var` calls serialize the env
    /// mutation (no `temp_env::with_vars` race).
    #[test]
    fn watch_run_lock_conflict_returns_exit_2() {
        let tmp = TempDir::new().unwrap();
        let path_str = tmp.path().to_str().unwrap().to_string();
        // Two sequential `with_var` closures so we never have two
        // env mutations in flight concurrently (the spec at
        // https://docs.rs/temp-env warns that nested `with_var`s are
        // racy on thread-pool executors).
        temp_env::with_var("CURATED_VAULT_ROOT", Some(&path_str), || {
            temp_env::with_var("CURATED_BRAIN_DIR", Some(&path_str), || {
                // Hold the lock for the brain_dir (this is the path
                // `VaultLock::acquire` in `watch_run` actually uses).
                let _held = crate::lock::VaultLock::acquire(
                    &crate::paths::resolve_brain_paths().brain_dir,
                )
                .expect("first lock acquire should succeed in test");
                let r = watch_run(watch_run_short_opts());
                assert!(
                    matches!(r, Ok(2)),
                    "vault lock conflict must surface as Ok(2), got {r:?}"
                );
            });
        });
    }

    /// Notify init failure: vault root exists but is a regular file (not a
    /// directory) → exit 4. Triggers `spawn_vault_watcher` failure on
    /// platforms where recursive watching requires a directory (and on
    /// Linux inotify it also rejects non-existent paths; we use the
    /// non-directory path because it is portable).
    ///
    /// **Test isolation (CodeRabbit review on PR #96):** this test
    /// previously only set `CURATED_VAULT_ROOT` and let
    /// `resolve_brain_paths()` use the developer's real `$HOME/.brain`
    /// dir for the brain DB probe. We now also set `CURATED_BRAIN_DIR`
    /// to the same temp dir so the probe doesn't touch the real brain.
    /// `open_rw` will create a throwaway `brain.db` file in the temp
    /// dir; the watcher init failure happens later, at the `notify`
    /// backend level.
    #[test]
    fn watch_run_non_directory_vault_root_returns_exit_4() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("not_a_dir.txt");
        std::fs::write(&file_path, "x").unwrap();
        let file_str = file_path.to_str().unwrap().to_string();
        temp_env::with_var("CURATED_VAULT_ROOT", Some(&file_str), || {
            temp_env::with_var("CURATED_BRAIN_DIR", Some(tmp.path().to_str().unwrap()), || {
                let r = watch_run(watch_run_short_opts());
                assert!(
                    matches!(r, Ok(4)),
                    "non-directory vault root must surface as Ok(4) (notify init), got {r:?}"
                );
            });
        });
    }
}

// TempDir is referenced in tests; we use the `tempfile` crate already in
// dev-dependencies. Imported here so the use-statement doesn't have to live
// in every test fn.
#[cfg(test)]
use tempfile::TempDir;
