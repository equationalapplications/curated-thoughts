//! Restore-path replica sync (issue #213 spec D3).
//!
//! A vault switch that restores a backup replaces `brain.db` wholesale. The
//! outbox lives inside that file, so without extra work the replica sees
//! nothing: the outgoing vault's records are never deleted from it, the
//! restored vault's are never re-inserted, and any of the outgoing vault's
//! events still undrained at copy time die with the old file.
//!
//! The sequence is **capture → stage → mark → preserve → install → publish →
//! sync**:
//!
//! 1. `capture_to_sidecar` writes the replica's outstanding obligations to an
//!    unpublished sidecar and fsyncs it. Holding the capture only in memory
//!    means a crash (or a failed sync) permanently destroys those obligations
//!    — the rows no longer exist anywhere and the divergence is silent.
//! 2. `stage_backup` copies the backup to a temporary file beside the
//!    database, so the database is never truncated mid-copy.
//! 3. `write_install_marker` records — durably, and before anything
//!    destructive happens — that an install is in flight. Crash recovery
//!    pairs the marker with the staged file to tell "committed" from "never
//!    happened": the install rename consumes the staged file, so
//!    marker-present + staged-absent is the one combination that can only
//!    arise after the rename returned.
//! 4. `preserve_outgoing_wal` copies the outgoing `-wal` to a rollback-only
//!    name before the sidecars are removed, so a failed or interrupted
//!    install can put the outgoing database back whole.
//! 5. `commit_staged_backup` installs the staged copy with one atomic
//!    rename — the point of no return — and `sync_install_dir` makes the new
//!    directory entry durable afterwards.
//! 6. `publish_sidecar` renames the capture into the path crash recovery
//!    watches — only now, because a capture consumed before the install would
//!    be replayed against the *outgoing* database.
//! 7. `sync` (or `run_pending`, at startup, after a crash) pushes the
//!    obligations onto the restored file's outbox. A crash anywhere in
//!    between is untangled by `reconcile_interrupted_install`, which runs
//!    before the brain is opened at startup.

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// One undrained `llm_wiki_outbox` row, preserved verbatim.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapturedOutboxRow {
    pub id: String,
    pub entity_id: String,
    pub table_name: String,
    pub record_id: String,
    pub operation: String,
    pub payload: String,
    pub created_at: i64,
}

/// What the replica still owes for the vault being switched away from.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CaptureState {
    /// `(id, entity_id)` for every `llm_wiki_entries` row, archived included.
    pub entries: Vec<(String, String)>,
    /// `(id, entity_id)` for every `llm_wiki_tasks` row, archived included.
    pub tasks: Vec<(String, String)>,
    /// Every row still in `llm_wiki_outbox`. The worker is stopped before the
    /// switch closure and drained rows are deleted on ack, so anything still
    /// here is undrained by construction.
    pub outbox: Vec<CapturedOutboxRow>,
}

/// Where the capture lives between the copy and the sync.
///
/// Its mere presence is the signal that a sync is owed, so it is only ever
/// created by an atomic rename (see `publish_sidecar`) and only ever renamed
/// into place *after* the backup is installed (see `commit_staged_backup`).
pub fn sidecar_path(db_path: &Path) -> PathBuf {
    sibling(db_path, ".sync-capture")
}

/// The unpublished capture, written before the install and renamed onto
/// `sidecar_path` once the install has committed.
pub fn pending_sidecar_path(db_path: &Path) -> PathBuf {
    sibling(db_path, ".sync-capture.pending")
}

/// The incoming database, staged beside `db_path` so the install is a rename.
pub fn staged_backup_path(db_path: &Path) -> PathBuf {
    sibling(db_path, ".restore-incoming")
}

/// The install marker: written after the backup is staged and before anything
/// destructive happens, removed once the switch settles. Its presence next to
/// an ABSENT staged file is the only combination that means the install
/// committed — the install rename consumes the staged file, so every earlier
/// crash point still has it.
pub fn install_marker_path(db_path: &Path) -> PathBuf {
    sibling(db_path, ".restore-installing")
}

/// The outgoing database's `-wal`, copied aside under a rollback-only name
/// before the sidecars are removed.
fn outgoing_wal_path(db_path: &Path) -> PathBuf {
    sibling(db_path, "-wal.outgoing")
}

fn sibling(db_path: &Path, suffix: &str) -> PathBuf {
    let mut name = db_path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    db_path.with_file_name(name)
}

/// fsync the directory holding `path`, so a rename into it survives a crash.
///
/// A rename is atomic with respect to readers the instant it returns, but the
/// directory entry itself is not durable until its directory is synced.
/// Failures propagate — reporting a durable rename that was not one would be
/// worse than failing. The one exception is platforms without directory-fsync
/// support, where this is a documented no-op.
fn fsync_parent_dir(path: &Path) -> Result<()> {
    let Some(dir) = path.parent() else {
        return Ok(());
    };
    #[cfg(windows)]
    {
        // `File::open` on a directory needs backup semantics on Windows and
        // the platform has no directory fsync anyway. The rename that
        // precedes this call is still atomic there; only its durability
        // across a power cut is weaker.
        let _ = dir;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let f =
            std::fs::File::open(dir).with_context(|| format!("open {} to fsync", dir.display()))?;
        f.sync_all()
            .with_context(|| format!("fsync {}", dir.display()))
    }
}

/// Rename `from` onto `to` and make the new directory entry durable.
///
/// A directory-fsync failure is always *post*-rename — `to` already holds the
/// content — so callers must read an error here as "done, but not yet known
/// durable", never as "nothing happened".
fn rename_durably(from: &Path, to: &Path) -> Result<()> {
    std::fs::rename(from, to)
        .with_context(|| format!("rename {} -> {}", from.display(), to.display()))?;
    fsync_parent_dir(to)
}

fn pairs(conn: &Connection, table: &str) -> Result<Vec<(String, String)>> {
    // No `deleted_at IS NULL` filter: an archived record's Insert may already
    // have drained, so the replica still needs its Delete.
    let mut stmt = conn.prepare(&format!("SELECT id, entity_id FROM {table}"))?;
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Read everything the sync will need. Caller supplies the connection so the
/// timing contract (see `capture_to_sidecar`) stays visible at the call site.
pub fn capture(conn: &Connection) -> Result<CaptureState> {
    let entries = pairs(conn, "llm_wiki_entries").context("capture entries")?;
    let tasks = pairs(conn, "llm_wiki_tasks").context("capture tasks")?;

    let mut stmt = conn.prepare(
        "SELECT id, entity_id, table_name, record_id, operation, payload, created_at
           FROM llm_wiki_outbox ORDER BY created_at ASC, rowid ASC",
    )?;
    let outbox = stmt
        .query_map([], |r| {
            Ok(CapturedOutboxRow {
                id: r.get(0)?,
                entity_id: r.get(1)?,
                table_name: r.get(2)?,
                record_id: r.get(3)?,
                operation: r.get(4)?,
                payload: r.get(5)?,
                created_at: r.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("capture undrained outbox rows")?;

    Ok(CaptureState {
        entries,
        tasks,
        outbox,
    })
}

/// Capture the live database and fsync it to the *pending* sidecar.
///
/// **Timing is load-bearing.** Call this after `release_global_db_lock` has
/// swapped in the stub and **before** `remove_sqlite_sidecars` deletes the
/// `-wal`. "Any time before the copy" is not good enough: a concurrent
/// process — the headless `--mcp` server holds its own brain.db connection —
/// prevents the implicit WAL checkpoint on close, so rows written since the
/// last checkpoint exist only in the `-wal` that sidecar removal is about to
/// delete. A capture after that point reads a stale main file and silently
/// misses records.
///
/// The capture lands on `pending_sidecar_path`, invisible to `read_sidecar`,
/// and is published only once the backup is installed. A capture published
/// before the install would be consumed by startup recovery against the
/// *outgoing* database — pushing Deletes for records that are still live in
/// the vault the user is still in, and dropping them from the replica — and
/// the obligations a retried restore needs would be gone.
///
/// A torn pending file can never be published: the publish happens later in
/// the same run and only if this function returned `Ok`, and a crash before
/// that point is untangled at startup by `reconcile_interrupted_install`,
/// which discards a pending capture whose install never committed.
pub fn capture_to_sidecar(db_path: &Path) -> Result<()> {
    let conn = Connection::open(db_path)
        .with_context(|| format!("open {} for restore-sync capture", db_path.display()))?;
    let state = capture(&conn)?;
    drop(conn);

    let path = pending_sidecar_path(db_path);
    let json = serde_json::to_vec(&state).context("serialize restore-sync capture")?;
    let mut file =
        std::fs::File::create(&path).with_context(|| format!("create {}", path.display()))?;
    file.write_all(&json)?;
    file.sync_all().context("fsync restore-sync capture")?;
    Ok(())
}

/// Make the pending capture visible to `read_sidecar`, atomically.
///
/// Call this **after** `commit_staged_backup` and before the sync. Until it
/// runs there is nothing for crash recovery to find, which is the point: the
/// sidecar's presence means "a backup is installed and its sync is owed".
///
/// Idempotent: no pending capture means nothing is owed — it was either
/// published already or published and then synced — so an error path can retry
/// this without inventing a failure. That covers the one failure mode this can
/// report after the fact: a directory-fsync error post-rename, where the
/// capture is already visible at the final path and only its durability is in
/// doubt.
pub fn publish_sidecar(db_path: &Path) -> Result<()> {
    let pending = pending_sidecar_path(db_path);
    if !pending.exists() {
        return Ok(());
    }
    rename_durably(&pending, &sidecar_path(db_path))
}

/// Drop an unpublished capture. Best effort; a stray pending file is inert
/// and the next restore overwrites it.
pub fn discard_pending_sidecar(db_path: &Path) {
    let _ = std::fs::remove_file(pending_sidecar_path(db_path));
}

/// Copy `backup_path` to a temporary file beside `db_path` and fsync it.
///
/// Copying straight onto `db_path` truncates the live database first, so an
/// interrupted or short copy leaves a torn file that the reopen (and the
/// failed-switch recovery, which opens the same path) cannot use. Staging
/// makes the install a rename: `db_path` is either the old database or the
/// whole backup, never a partial one.
pub fn stage_backup(backup_path: &Path, db_path: &Path) -> Result<PathBuf> {
    let staged = staged_backup_path(db_path);
    std::fs::copy(backup_path, &staged)
        .with_context(|| format!("copy {} -> {}", backup_path.display(), staged.display()))?;
    // The fsync handle must carry WRITE access: on Windows `sync_all` routes
    // to `FlushFileBuffers`, which requires GENERIC_WRITE and fails with
    // ERROR_ACCESS_DENIED on a read-only handle.
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&staged)
        .with_context(|| format!("reopen {} to fsync", staged.display()))?;
    file.sync_all()
        .with_context(|| format!("fsync {}", staged.display()))?;
    Ok(staged)
}

/// Delete the stated-ledger rederive stamp from the STAGED backup, before the
/// install rename. This makes the restore's rederive structural (§2-A, T4 M2):
/// whichever open comes next — the happy-path reopen, a failed-step recovery
/// reopen (`restore_installed` is already true past the rename), or the
/// startup reconciliation after a crash mid-install — finds no stamp and
/// rederives from the root the config names. Callers run this BEFORE
/// [`commit_staged_backup`], so a failed clear fails the switch while the
/// outgoing database is still in place. "No such table" is success: a
/// database without the meta table has no stamp either. The delete is
/// checkpointed and fsynced so the rename cannot publish a half-journaled
/// file.
pub fn clear_staged_rederive_stamp(staged: &Path) -> Result<()> {
    let conn = rusqlite::Connection::open(staged)
        .with_context(|| format!("open staged backup {} for the stamp clear", staged.display()))?;
    let cleared = conn.execute(
        "DELETE FROM llm_wiki_meta WHERE key = 'stated_ledger_rederived'",
        [],
    );
    match cleared {
        Ok(_) => {}
        Err(e) => {
            if e.to_string().contains("no such table") {
                // No meta table ⇒ no stamp to clear; the post-install open
                // rederive is already unconditional for this database.
            } else {
                return Err(e).with_context(|| {
                    format!("clear stated_ledger_rederived from {}", staged.display())
                });
            }
        }
    }
    // Fold any WAL/journal content into the main file and fsync it: the
    // install is a rename, so the staged main file must stand alone.
    let _ = conn.execute("PRAGMA wal_checkpoint(TRUNCATE)", []);
    drop(conn);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(staged)
        .with_context(|| format!("reopen {} to fsync after the stamp clear", staged.display()))?;
    file.sync_all()
        .with_context(|| format!("fsync {} after the stamp clear", staged.display()))?;
    Ok(())
}

/// Record, durably and before anything destructive happens, that an install
/// is in flight. See [`install_marker_path`] for how recovery reads it.
pub fn write_install_marker(db_path: &Path, destination_root: &str) -> Result<()> {
    let marker = install_marker_path(db_path);
    std::fs::write(&marker, destination_root.as_bytes())
        .with_context(|| format!("write {}", marker.display()))?;
    let f = std::fs::File::open(&marker)
        .with_context(|| format!("reopen {} to fsync", marker.display()))?;
    f.sync_all()
        .with_context(|| format!("fsync {}", marker.display()))?;
    fsync_parent_dir(&marker)
}

/// The DESTINATION vault root recorded in the install marker, if any.
/// ONLY a zero-byte marker is the legacy empty case → `Ok(None)`; a nonempty
/// marker's contents are returned EXACTLY — no trimming — because a valid
/// destination can end with whitespace and recovery must align the config to
/// the root the restore actually targeted. Read/decode errors propagate
/// (`Err`) so reconciliation can retain the marker and stop startup instead
/// of silently opening under the outgoing config (T4-follow-up round 3).
pub fn read_install_marker_destination(db_path: &Path) -> Result<Option<String>> {
    let bytes = std::fs::read(install_marker_path(db_path))
        .with_context(|| format!("read {}", install_marker_path(db_path).display()))?;
    if bytes.is_empty() {
        return Ok(None);
    }
    String::from_utf8(bytes)
        .map(Some)
        .with_context(|| "install marker destination is not valid UTF-8")
}

/// Remove the install marker. Best effort; a stray marker with no staged file
/// and no pending capture is inert — recovery reads it as "committed" and
/// finds nothing left to do.
pub fn clear_install_marker(db_path: &Path) {
    let _ = std::fs::remove_file(install_marker_path(db_path));
}

/// Copy the outgoing `-wal` to a rollback-only name before the sidecars are
/// removed.
///
/// The restore path deletes `-wal`/`-shm` because they belong to the outgoing
/// file and would be misread as the replacement's. But a concurrent `--mcp`
/// connection blocks the implicit checkpoint on close — the same reason the
/// capture runs before removal — so committed rows can live ONLY in that
/// `-wal`. Deleting the only copy would make a failed install unrecoverable:
/// the old main file would reopen without them. The `-shm` needs no copy; it
/// is a rebuildable index.
pub fn preserve_outgoing_wal(db_path: &Path) -> Result<()> {
    let wal = sibling(db_path, "-wal");
    if !wal.exists() {
        return Ok(());
    }
    let kept = outgoing_wal_path(db_path);
    std::fs::copy(&wal, &kept)
        .with_context(|| format!("copy {} -> {}", wal.display(), kept.display()))?;
    let f = std::fs::File::open(&kept)
        .with_context(|| format!("reopen {} to fsync", kept.display()))?;
    f.sync_all()
        .with_context(|| format!("fsync {}", kept.display()))?;
    fsync_parent_dir(&kept)
}

/// Put a preserved `-wal` back after an install that did NOT commit: the
/// database at `db_path` is still the outgoing one, and its committed rows may
/// exist only in the preserved copy. If a `-wal` is already back at the
/// original name the copy is redundant and dropped instead.
pub fn restore_outgoing_wal(db_path: &Path) {
    let kept = outgoing_wal_path(db_path);
    if !kept.exists() {
        return;
    }
    let wal = sibling(db_path, "-wal");
    if !wal.exists() && std::fs::rename(&kept, &wal).is_ok() {
        return;
    }
    let _ = std::fs::remove_file(&kept);
}

/// Drop the preserved `-wal`: the install committed, so the outgoing journal
/// describes a database that no longer exists at `db_path`.
pub fn discard_outgoing_wal(db_path: &Path) {
    let _ = std::fs::remove_file(outgoing_wal_path(db_path));
}

/// Install a staged backup over `db_path` with a single atomic rename.
///
/// The rename is the point of no return and the **only** thing this does: the
/// moment it returns `Ok`, the file at `db_path` is the replacement, even if
/// the directory fsync that follows ([`sync_install_dir`]) then fails. That is
/// why the fsync is a separate call — the caller must be able to record
/// "installed" between the two, without a fsync failure making the error path
/// forget the database was already replaced.
///
/// The caller must have removed the outgoing database's `-wal`/`-shm` first:
/// they belong to the old file and would be misread as this one's.
pub fn commit_staged_backup(staged: &Path, db_path: &Path) -> Result<()> {
    std::fs::rename(staged, db_path)
        .with_context(|| format!("rename {} -> {}", staged.display(), db_path.display()))
}

/// Make the install's new directory entry durable. Separate from
/// [`commit_staged_backup`] by design (see its doc): the rename already
/// happened, so a failure here must not be mistaken for "the old database is
/// still in place".
pub fn sync_install_dir(db_path: &Path) -> Result<()> {
    fsync_parent_dir(db_path)
}

/// The pending capture, or `None` when there is nothing to finish.
///
/// Only the published sidecar is read — never `pending_sidecar_path` — so a
/// capture is consumed only after the backup it belongs to is installed.
///
/// A sidecar that will not parse is treated as absent and removed. Publishing
/// is a rename of an already-fsynced file, so this should be unreachable; it
/// stays because a half-written capture is not actionable either way.
pub fn read_sidecar(db_path: &Path) -> Result<Option<CaptureState>> {
    let path = sidecar_path(db_path);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    match serde_json::from_slice::<CaptureState>(&bytes) {
        Ok(state) => Ok(Some(state)),
        Err(e) => {
            eprintln!(
                "[restore_sync] discarding unparseable capture at {}: {e}",
                path.display()
            );
            let _ = std::fs::remove_file(&path);
            Ok(None)
        }
    }
}

/// Push the captured obligations onto the restored file's outbox, in order:
/// re-pushed undrained rows, then Deletes for the captured records, then
/// Inserts re-asserting everything the restored file now holds.
///
/// Runs in ONE transaction on the reopened database, before the outbox worker
/// restarts. Runs whether or not a replica is configured — the rows sit in the
/// outbox and drain only if a worker is running.
pub fn sync(conn: &Connection, state: &CaptureState, now_ms: i64) -> Result<()> {
    let tx = conn.unchecked_transaction()?;

    // 1. Re-push undrained rows verbatim, preserving original created_at so
    //    intra-vault event order survives.
    //
    //    `created_at` alone does not pin the order. The drain reads
    //    `ORDER BY created_at ASC, rowid ASC` (`outbox/mod.rs:89`), so rows
    //    sharing a millisecond drain in insertion order — and an Insert that
    //    drained after its record's Delete would resurrect a hard-deleted
    //    record on the replica. `capture` therefore reads in exactly that
    //    order, `CaptureState.outbox` is an ordered list, and this loop
    //    reinserts in list order, so the new rowids reproduce the drain order
    //    the outgoing vault had.
    //
    //    `INSERT OR IGNORE`, not a plain INSERT: `llm_wiki_outbox.id` is
    //    TEXT PRIMARY KEY, and the restored backup can already contain these
    //    exact rows (the outbox is never truncated across a no-restore switch
    //    and backups snapshot the whole file). A collision would fail this
    //    transaction, retain the sidecar, and leave startup recovery looping
    //    on the same statement — a switch that never converges. Skipping is
    //    lossless: a matching id IS the same event, already queued.
    for row in &state.outbox {
        tx.execute(
            "INSERT OR IGNORE INTO llm_wiki_outbox
                 (id, entity_id, table_name, record_id, operation, payload, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                row.id,
                row.entity_id,
                row.table_name,
                row.record_id,
                row.operation,
                row.payload,
                row.created_at,
            ],
        )?;
    }

    // 2. Deletes for every captured record — the outgoing vault's knowledge
    //    must stop being served by the replica.
    for (table_name, records) in [("entries", &state.entries), ("tasks", &state.tasks)] {
        for (id, entity_id) in records.iter() {
            crate::db::outbox_format::push_outbox_row(
                &tx,
                &crate::db::outbox_format::OutboxPushParams {
                    entity_id: entity_id.clone(),
                    table_name: table_name.to_string(),
                    record_id: id.clone(),
                    operation: crate::db::outbox_format::OutboxOperation::Delete,
                    payload: serde_json::json!({ "id": id }),
                },
                Some(now_ms),
            )?;
        }
    }

    // 3. Re-assert every record the restored file holds. Full payloads via the
    //    existing builders, so the rows are byte-identical to what a normal
    //    write would have produced.
    reassert_entries(&tx, now_ms)?;
    reassert_tasks(&tx, now_ms)?;

    tx.commit()?;
    Ok(())
}

/// Finish a restore's replica sync if one is pending.
///
/// Returns `Ok(false)` when there is no sidecar — the overwhelmingly common
/// case, so this is cheap to call unconditionally at startup. On success the
/// sidecar is deleted; on failure it is **kept**, so the next startup retries
/// rather than silently dropping the outgoing vault's replica obligations.
pub fn run_pending(conn: &Connection, db_path: &Path, now_ms: i64) -> Result<bool> {
    let Some(state) = read_sidecar(db_path)? else {
        return Ok(false);
    };
    sync(conn, &state, now_ms).context("restore-sync after backup restore")?;
    let path = sidecar_path(db_path);
    if let Err(e) = std::fs::remove_file(&path) {
        // The sync committed; a leftover sidecar only costs one redundant
        // (idempotent) replay next start.
        eprintln!(
            "[restore_sync] sync committed but sidecar {} could not be removed: {e}",
            path.display()
        );
    }
    Ok(true)
}

/// Resolve a vault switch that died mid-install, BEFORE anything opens the
/// brain.
///
/// The marker plus the staged file reconstruct which side of the commit point
/// the crash landed on, because the install rename consumes the staged file:
///
/// | marker | staged  | meaning                                             |
/// |--------|---------|-----------------------------------------------------|
/// | yes    | absent  | the rename returned — **committed**. Publish the    |
/// |        |         | pending capture so `run_pending` (which the caller   |
/// |        |         | runs next) finishes the sync, and drop the preserved |
/// |        |         | outgoing `-wal`.                                    |
/// | yes    | present | the rename never ran — **nothing was replaced**.    |
/// |        |         | Put the preserved `-wal` back, and drop the capture  |
/// |        |         | and staged copy: they belong to a restore that never |
/// |        |         | happened.                                           |
/// | no     | —       | the crash predates the marker, so nothing destructive|
/// |        |         | had begun; clear any strays.                        |
///
/// Idempotent, and cheap in the common case: one `stat` that finds no marker.
/// Resolve which side of the install commit point a crash landed on, and
/// align the persisted config to the recorded destination when the install
/// committed. Returns `Err` — with the marker RETAINED on disk — when the
/// committed branch's config alignment fails: the caller (startup) must then
/// refuse the rooted open, because an unstamped installed brain opened under
/// the outgoing config would rederive — and stamp complete — the wrong root.
/// The next start retries the alignment; the marker is removed only once the
/// config is persisted (or the install never committed, or the marker carries
/// no destination and there is nothing to align to).
pub fn reconcile_interrupted_install(db_path: &Path, config_path: &Path) -> Result<()> {
    let marker = install_marker_path(db_path);
    if !marker.exists() {
        // A crash this early left at most stray artifacts; the outgoing
        // database is untouched.
        discard_pending_sidecar(db_path);
        let _ = std::fs::remove_file(staged_backup_path(db_path));
        return Ok(());
    }
    if !staged_backup_path(db_path).exists() {
        // Committed: the rename consumed the staged file.
        discard_outgoing_wal(db_path);
        if let Err(e) = publish_sidecar(db_path) {
            eprintln!(
                "[restore_sync] install committed but its capture could not be \
                 published ({e}); the replica may diverge for the outgoing vault"
            );
        }
        // The installed database is the DESTINATION vault's, but the
        // persisted config still names the outgoing vault until
        // `set_vault_path` runs — and it never runs if the process exited
        // inside the post-rename window. The marker records that destination:
        // align the config HERE, before the startup's first rooted open, so
        // the open-time rederive (the stamp was cleared from the staged
        // backup) runs from the root the restore targeted instead of
        // re-deriving — and stamping complete — the outgoing root over the
        // new vault's keys. Read BEFORE the marker is removed, below. An
        // empty (pre-recording) marker leaves the config untouched.
        let dest = read_install_marker_destination(db_path)?;
        if let Some(dest) = dest {
            if let Err(e) = crate::vault::config::VaultConfig::new(config_path.to_path_buf())
                .set_vault_path(&dest)
            {
                // Keep the marker: the destination is still recoverable for
                // the next start, which must retry the alignment instead of
                // opening the vault under the outgoing config.
                return Err(anyhow::anyhow!(
                    "install committed mid-switch but aligning the config to the \
                     recorded destination {dest} failed: {e}; the install marker is \
                     retained and startup is refused"
                ));
            }
            eprintln!(
                "[restore_sync] install committed mid-switch: config aligned to the recorded destination {dest}"
            );
        }
    } else {
        restore_outgoing_wal(db_path);
        discard_pending_sidecar(db_path);
        // Remove the MARKER first (propagating its error), THEN the staged
        // backup (best-effort): a crash or failure between the staged
        // removal and the marker removal would leave marker-without-staged,
        // which the committed branch above misreads as "install committed" —
        // aligning the config to the recorded destination while the OUTGOING
        // database is still at db_path. Marker-first means any leftover
        // staged file lands in the no-marker branch, which discards it
        // without touching the config (T4-follow-up round 3, Major 3).
        std::fs::remove_file(&marker)
            .with_context(|| format!("remove install marker {}", marker.display()))?;
        let _ = std::fs::remove_file(staged_backup_path(db_path));
    }
    let _ = std::fs::remove_file(&marker);
    Ok(())
}

fn reassert_entries(conn: &Connection, now_ms: i64) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT id, entity_id, title, body, tags, confidence, source_type, source_hash,
                source_ref, okf_type, okf_sources, okf_verified, okf_usage_window,
                created_at, updated_at, deleted_at, lifecycle_status, stale_after,
                generated_by, last_verified_at, last_verified_by, area
           FROM llm_wiki_entries",
    )?;
    struct Row {
        id: String,
        entity_id: String,
        title: String,
        body: String,
        tags: String,
        confidence: String,
        source_type: String,
        source_hash: Option<String>,
        source_ref: Option<String>,
        okf_type: Option<String>,
        okf_sources: Option<String>,
        okf_verified: Option<String>,
        okf_usage_window: Option<String>,
        created_at: i64,
        updated_at: i64,
        deleted_at: Option<i64>,
        lifecycle_status: Option<String>,
        stale_after: Option<i64>,
        generated_by: Option<String>,
        last_verified_at: Option<i64>,
        last_verified_by: Option<String>,
        area: Option<String>,
    }
    let rows = stmt
        .query_map([], |r| {
            Ok(Row {
                id: r.get(0)?,
                entity_id: r.get(1)?,
                title: r.get(2)?,
                body: r.get(3)?,
                tags: r.get(4)?,
                confidence: r.get(5)?,
                source_type: r.get(6)?,
                source_hash: r.get(7)?,
                source_ref: r.get(8)?,
                okf_type: r.get(9)?,
                okf_sources: r.get(10)?,
                okf_verified: r.get(11)?,
                okf_usage_window: r.get(12)?,
                created_at: r.get(13)?,
                updated_at: r.get(14)?,
                deleted_at: r.get(15)?,
                lifecycle_status: r.get(16)?,
                stale_after: r.get(17)?,
                generated_by: r.get(18)?,
                last_verified_at: r.get(19)?,
                last_verified_by: r.get(20)?,
                area: r.get(21)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);

    for row in rows {
        let tags: Vec<String> = serde_json::from_str(&row.tags).unwrap_or_default();
        let payload = crate::db::commit::wiki_fact_outbox_payload(
            &row.id,
            &row.entity_id,
            &row.title,
            &row.body,
            &tags,
            &row.confidence,
            &row.source_type,
            row.source_hash.as_deref(),
            row.source_ref.as_deref().unwrap_or(""),
            row.okf_type.as_deref(),
            row.okf_sources.as_deref(),
            row.okf_verified.as_deref(),
            row.okf_usage_window.as_deref(),
            row.created_at,
            row.updated_at,
            row.deleted_at,
            row.lifecycle_status.as_deref(),
            row.stale_after,
            row.generated_by.as_deref(),
            row.last_verified_at,
            row.last_verified_by.as_deref(),
            row.area.as_deref(),
        );
        crate::db::commit::push_entries_outbox(
            conn,
            &row.entity_id,
            &row.id,
            crate::db::outbox_format::OutboxOperation::Insert,
            payload,
            now_ms,
        )?;
    }
    Ok(())
}

fn reassert_tasks(conn: &Connection, now_ms: i64) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT id, entity_id, description, status, priority, created_at, updated_at,
                resolved_at, deleted_at, okf_type, okf_sources, okf_verified,
                okf_usage_window, lifecycle_status, stale_after, generated_by,
                last_verified_at, last_verified_by
           FROM llm_wiki_tasks",
    )?;
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        String,
        String,
        String,
        String,
        i64,
        i64,
        i64,
        Option<i64>,
        Option<i64>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<String>,
        Option<i64>,
        Option<String>,
    )> = stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
                r.get(9)?,
                r.get(10)?,
                r.get(11)?,
                r.get(12)?,
                r.get(13)?,
                r.get(14)?,
                r.get(15)?,
                r.get(16)?,
                r.get(17)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);

    for t in rows {
        let payload = crate::db::commit::wiki_task_outbox_payload(
            &t.0,
            &t.1,
            &t.2,
            &t.3,
            t.4,
            t.5,
            t.6,
            t.7,
            t.8,
            t.9.as_deref(),
            t.10.as_deref(),
            t.11.as_deref(),
            t.12.as_deref(),
            t.13.as_deref(),
            t.14,
            t.15.as_deref(),
            t.16,
            t.17.as_deref(),
        );
        crate::db::commit::push_tasks_outbox(
            conn,
            &t.1,
            &t.0,
            crate::db::outbox_format::OutboxOperation::Insert,
            payload,
            now_ms,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;

    fn seed(conn: &Connection) {
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, created_at, updated_at, deleted_at)
             VALUES ('fact_1', 'ent_a', 'T', 'B', 1, 1, NULL),
                    ('fact_archived', 'ent_a', 'T', 'B', 1, 1, 99)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_wiki_tasks (id, entity_id, description, created_at, updated_at)
             VALUES ('task_1', 'ent_b', 'do it', 1, 1)",
            [],
        )
        .unwrap();
        // An undrained Delete for a record that no longer has a table row —
        // a wiki_forget run while the replica was unreachable. This is the
        // row that only the outbox capture can preserve.
        conn.execute(
            "INSERT INTO llm_wiki_outbox
                 (id, entity_id, table_name, record_id, operation, payload, created_at)
             VALUES ('out_forgotten00000000000001', 'ent_a', 'entries', 'fact_gone',
                     'DELETE', '{\"id\":\"fact_gone\"}', 50)",
            [],
        )
        .unwrap();
    }

    #[test]
    fn capture_takes_every_record_pair_including_archived() {
        let conn = open_in_memory().unwrap();
        seed(&conn);

        let state = capture(&conn).unwrap();

        let mut entries: Vec<&str> = state.entries.iter().map(|(id, _)| id.as_str()).collect();
        entries.sort();
        assert_eq!(
            entries,
            ["fact_1", "fact_archived"],
            "archived rows need Deletes too"
        );
        assert_eq!(state.entries[0].1, "ent_a", "entity_id must ride along");
        assert_eq!(
            state.tasks,
            vec![("task_1".to_string(), "ent_b".to_string())]
        );
    }

    #[test]
    fn capture_takes_undrained_outbox_rows_verbatim() {
        let conn = open_in_memory().unwrap();
        seed(&conn);

        let state = capture(&conn).unwrap();

        assert_eq!(state.outbox.len(), 1);
        let row = &state.outbox[0];
        assert_eq!(row.id, "out_forgotten00000000000001");
        assert_eq!(row.record_id, "fact_gone");
        assert_eq!(row.operation, "DELETE");
        assert_eq!(row.created_at, 50, "original created_at must be preserved");
    }

    #[test]
    fn sidecar_round_trips_and_reports_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("brain.db");

        assert!(
            read_sidecar(&db_path).unwrap().is_none(),
            "no sidecar yet means no pending sync"
        );

        let conn = crate::db::connection::open_app_db(&db_path, None).unwrap();
        seed(&conn);
        drop(conn);

        capture_to_sidecar(&db_path).unwrap();
        assert!(
            pending_sidecar_path(&db_path).exists(),
            "the capture lands on the pending path first"
        );
        assert!(
            read_sidecar(&db_path).unwrap().is_none(),
            "an unpublished capture must stay invisible to recovery"
        );

        publish_sidecar(&db_path).unwrap();
        assert!(sidecar_path(&db_path).exists());
        assert!(
            !pending_sidecar_path(&db_path).exists(),
            "publishing moves the file, it does not copy it"
        );
        publish_sidecar(&db_path).unwrap();
        assert!(
            sidecar_path(&db_path).exists(),
            "publishing twice is a no-op, not a failure — the switch's error \
             path retries it"
        );

        let restored = read_sidecar(&db_path).unwrap().expect("sidecar must parse");
        assert_eq!(restored.entries.len(), 2);
        assert_eq!(restored.tasks.len(), 1);
        assert_eq!(restored.outbox.len(), 1);
    }

    fn outbox_rows(conn: &Connection) -> Vec<(String, String, String, String, i64)> {
        let mut stmt = conn
            .prepare(
                "SELECT id, table_name, record_id, operation, created_at
                   FROM llm_wiki_outbox ORDER BY created_at ASC, rowid ASC",
            )
            .unwrap();
        stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
    }

    #[test]
    fn sync_orders_stale_then_repushed_then_deletes_then_inserts() {
        // The live (outgoing) vault.
        let live = open_in_memory().unwrap();
        seed(&live);
        let state = capture(&live).unwrap();

        // The "restored" file: different records, plus a stale pre-backup event.
        let restored = open_in_memory().unwrap();
        restored
            .execute(
                "INSERT INTO llm_wiki_entries (id, entity_id, title, body, created_at, updated_at)
                 VALUES ('fact_restored', 'ent_r', 'T', 'B', 1, 1)",
                [],
            )
            .unwrap();
        restored
            .execute(
                "INSERT INTO llm_wiki_outbox
                     (id, entity_id, table_name, record_id, operation, payload, created_at)
                 VALUES ('out_stale000000000000000001', 'ent_r', 'entries', 'fact_restored',
                         'INSERT', '{\"id\":\"fact_restored\"}', 10)",
                [],
            )
            .unwrap();

        sync(&restored, &state, 1000).unwrap();

        let rows = outbox_rows(&restored);
        assert_eq!(
            rows[0].0, "out_stale000000000000000001",
            "stale event first"
        );
        assert_eq!(rows[1].0, "out_forgotten00000000000001", "re-pushed next");
        assert_eq!(rows[1].4, 50, "re-push preserves original created_at");

        let tail: Vec<_> = rows[2..].iter().collect();
        assert!(
            tail.iter().all(|r| r.4 == 1000),
            "sync rows carry the switch's now_ms"
        );
        let deletes: Vec<&str> = tail
            .iter()
            .filter(|r| r.3 == "DELETE")
            .map(|r| r.2.as_str())
            .collect();
        assert_eq!(
            deletes.len(),
            3,
            "2 captured entries + 1 captured task get Deletes"
        );
        assert!(deletes.contains(&"fact_archived"));

        let inserts: Vec<&str> = tail
            .iter()
            .filter(|r| r.3 == "INSERT")
            .map(|r| r.2.as_str())
            .collect();
        assert_eq!(
            inserts,
            vec!["fact_restored"],
            "every record in the restored file is re-asserted"
        );
    }

    /// Review round 1 M2: a reasserted payload must carry the row's seeded
    /// area — the reassert SELECT reads `area` and forwards it into
    /// `wiki_fact_outbox_payload`; a dropped column would silently un-scope
    /// every fact on restore.
    #[test]
    fn reasserted_payload_carries_seeded_area() {
        let live = open_in_memory().unwrap();
        seed(&live);
        let state = capture(&live).unwrap();

        let restored = open_in_memory().unwrap();
        restored
            .execute(
                "INSERT INTO llm_wiki_entries (id, entity_id, title, body, created_at, updated_at, area)
                 VALUES ('fact_scoped', 'ent_r', 'T', 'B', 1, 1, 'deploy')",
                [],
            )
            .unwrap();

        sync(&restored, &state, 1000).unwrap();

        let raw: String = restored
            .query_row(
                "SELECT payload FROM llm_wiki_outbox
                 WHERE table_name = 'entries' AND record_id = 'fact_scoped'
                   AND operation = 'INSERT'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            payload["area"],
            serde_json::json!("deploy"),
            "reasserted payload must carry the seeded area, got: {payload}"
        );
    }

    #[test]
    fn sync_skips_repushed_ids_the_restored_file_already_has() {
        // THE WEDGE CASE. llm_wiki_outbox.id is TEXT PRIMARY KEY. The outbox
        // survives no-restore switches and backup_vault_db snapshots the whole
        // file, so a backup taken while the replica was unreachable carries
        // undrained rows that are STILL LIVE when that backup is restored.
        // Without the dedupe this INSERT violates the primary key, the sync
        // fails, the sidecar is retained, and startup recovery re-runs the
        // identical failing statement forever.
        let live = open_in_memory().unwrap();
        seed(&live);
        let state = capture(&live).unwrap();

        let restored = open_in_memory().unwrap();
        restored
            .execute(
                "INSERT INTO llm_wiki_outbox
                     (id, entity_id, table_name, record_id, operation, payload, created_at)
                 VALUES ('out_forgotten00000000000001', 'ent_a', 'entries', 'fact_gone',
                         'DELETE', '{\"id\":\"fact_gone\"}', 50)",
                [],
            )
            .unwrap();

        sync(&restored, &state, 1000).expect("sync must not fail on a colliding id");

        let collisions: i64 = restored
            .query_row(
                "SELECT count(*) FROM llm_wiki_outbox WHERE id = 'out_forgotten00000000000001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(collisions, 1, "the restored file's copy is kept, once");
    }

    #[test]
    fn sync_is_idempotent() {
        let live = open_in_memory().unwrap();
        seed(&live);
        let state = capture(&live).unwrap();

        let restored = open_in_memory().unwrap();
        sync(&restored, &state, 1000).unwrap();
        let first = outbox_rows(&restored).len();
        sync(&restored, &state, 1000).unwrap();

        // Re-pushed rows dedupe by id; Deletes get fresh ids, so a second run
        // adds duplicates — harmless on an append-only replica (a Delete of an
        // already-deleted record is a no-op) but worth pinning as intentional.
        let second = outbox_rows(&restored).len();
        assert!(second >= first, "sync never loses rows");
        let repushed: i64 = restored
            .query_row(
                "SELECT count(*) FROM llm_wiki_outbox WHERE id = 'out_forgotten00000000000001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(repushed, 1, "verbatim re-pushes never duplicate");
    }

    #[test]
    fn run_pending_finishes_an_interrupted_switch_then_clears_the_sidecar() {
        // The crash: capture written, copy done, sync never ran.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("brain.db");

        let live = crate::db::connection::open_app_db(&db_path, None).unwrap();
        seed(&live);
        drop(live);
        capture_to_sidecar(&db_path).unwrap();

        // Simulate the install: a different database now sits at db_path, and
        // the capture is published only once it does.
        std::fs::remove_file(&db_path).unwrap();
        let restored = crate::db::connection::open_app_db(&db_path, None).unwrap();
        publish_sidecar(&db_path).unwrap();

        let did = run_pending(&restored, &db_path, 1000).unwrap();
        assert!(did, "a present sidecar means there is work to finish");
        assert!(
            !sidecar_path(&db_path).exists(),
            "sidecar is deleted once the sync commits"
        );
        assert!(
            outbox_rows(&restored).iter().any(|r| r.2 == "fact_gone"),
            "the forgotten record's Delete survived the crash"
        );

        let again = run_pending(&restored, &db_path, 1000).unwrap();
        assert!(!again, "second run is a no-op — nothing pending");
    }

    #[test]
    fn staged_install_replaces_the_database_atomically() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("brain.db");
        let backup_path = tmp.path().join("brain.db.bak");

        // A live database with one marker row, and a backup with a different one.
        let live = crate::db::connection::open_app_db(&db_path, None).unwrap();
        seed(&live);
        drop(live);
        let backup = crate::db::connection::open_app_db(&backup_path, None).unwrap();
        backup
            .execute(
                "INSERT INTO llm_wiki_entries (id, entity_id, title, body, created_at, updated_at)
                 VALUES ('fact_from_backup', 'ent_b', 'T', 'B', 1, 1)",
                [],
            )
            .unwrap();
        drop(backup);

        let staged = stage_backup(&backup_path, &db_path).unwrap();
        assert_eq!(staged, staged_backup_path(&db_path));
        assert!(
            db_path.exists(),
            "staging must not touch the live database — a failure here has to \
             leave a usable file behind for the failed-switch recovery"
        );

        commit_staged_backup(&staged, &db_path).unwrap();
        assert!(
            !staged.exists(),
            "installing moves the staged file, it does not copy it"
        );

        let installed = crate::db::connection::open_app_db(&db_path, None).unwrap();
        let ids: Vec<String> = installed
            .prepare("SELECT id FROM llm_wiki_entries ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(ids, ["fact_from_backup"], "the backup is what landed");
    }

    /// Restore-branch half of spec R2.9.2 (the clear-branch half lives in
    /// queries.rs): the restore branch NEVER calls `clear_vault_tables` — it
    /// reinstalls the backup FILE via `stage_backup`, so the vault-keyed
    /// behavior comes from the backup's own rows. The backup carries
    /// `llm_wiki_entity_manifests` rows (tier vocabulary AND any entity-level
    /// overrides of the vault being restored); the install must land exactly
    /// those, not the outgoing vault's. This pins the mechanism every later
    /// `--entity` test relies on: entity-level rows are vault-scoped by
    /// construction because the whole file swaps.
    #[test]
    fn stage_backup_install_carries_the_backup_vaults_manifest_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("brain.db");
        let backup_path = tmp.path().join("brain.db.bak");

        // Outgoing vault: an entity-level manifest override its owner set here.
        let live = crate::db::connection::open_app_db(&db_path, None).unwrap();
        live.execute(
            "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, updated_at)
             VALUES ('ent_live_only', 'strict', 1)",
            [],
        )
        .unwrap();
        drop(live);

        // Backup vault (the one being restored): the tier vocabulary plus a
        // DIFFERENT entity-level override, plus a `tier_working::` partition
        // row (vault-root-keyed leg of the tier vocabulary).
        let backup = crate::db::connection::open_app_db(&backup_path, None).unwrap();
        backup
            .execute(
                "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, updated_at)
                 VALUES ('tier_fact', 'strict', 1)",
                [],
            )
            .unwrap();
        backup
            .execute(
                "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, updated_at)
                 VALUES ('tier_working::0123456789abcdef', 'strict', 1)",
                [],
            )
            .unwrap();
        backup
            .execute(
                "INSERT INTO llm_wiki_entity_manifests (entity_id, mode, updated_at)
                 VALUES ('ent_backup_override', 'off', 1)",
                [],
            )
            .unwrap();
        drop(backup);

        let staged = stage_backup(&backup_path, &db_path).unwrap();
        commit_staged_backup(&staged, &db_path).unwrap();

        let installed = crate::db::connection::open_app_db(&db_path, None).unwrap();
        let manifest_ids: Vec<String> = installed
            .prepare("SELECT entity_id FROM llm_wiki_entity_manifests ORDER BY entity_id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            manifest_ids,
            [
                "ent_backup_override".to_string(),
                "tier_fact".to_string(),
                "tier_working::0123456789abcdef".to_string(),
            ],
            "the restore reinstalls the BACKUP's manifest rows — tier vocabulary \
             intact (both the tier_fact seed and the tier_working:: partition), \
             the backup's entity-level override present, the outgoing vault's gone"
        );
    }

    #[test]
    fn capture_and_repush_preserve_drain_order_within_one_millisecond() {
        // The wedge: an Insert and a Delete for the same record share a
        // millisecond. `created_at` cannot order them, so only insertion order
        // keeps the Delete behind the Insert — reverse them and the drain
        // resurrects a hard-deleted record on the replica.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("brain.db");
        let live = crate::db::connection::open_app_db(&db_path, None).unwrap();
        for (id, op) in [
            ("out_samems0000000000000001", "INSERT"),
            ("out_samems0000000000000002", "DELETE"),
        ] {
            live.execute(
                "INSERT INTO llm_wiki_outbox
                     (id, entity_id, table_name, record_id, operation, payload, created_at)
                 VALUES (?1, 'ent_a', 'entries', 'fact_same_ms', ?2, '{}', 70)",
                rusqlite::params![id, op],
            )
            .unwrap();
        }

        let state = capture(&live).unwrap();
        drop(live);
        assert_eq!(
            state
                .outbox
                .iter()
                .map(|r| r.operation.as_str())
                .collect::<Vec<_>>(),
            ["INSERT", "DELETE"],
            "capture reads in the drain's own `created_at ASC, rowid ASC` order"
        );

        std::fs::remove_file(&db_path).unwrap();
        let restored = crate::db::connection::open_app_db(&db_path, None).unwrap();
        sync(&restored, &state, 1000).unwrap();

        let drained: Vec<String> = outbox_rows(&restored)
            .into_iter()
            .filter(|r| r.2 == "fact_same_ms")
            .map(|r| r.3)
            .collect();
        assert_eq!(
            drained,
            ["INSERT", "DELETE"],
            "the re-push must reproduce the drain order, not just the timestamps"
        );
    }

    #[test]
    fn run_pending_keeps_the_sidecar_when_the_sync_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("brain.db");

        let live = crate::db::connection::open_app_db(&db_path, None).unwrap();
        seed(&live);
        drop(live);
        capture_to_sidecar(&db_path).unwrap();
        publish_sidecar(&db_path).unwrap();

        // A database with no outbox table: the sync cannot commit.
        let broken = Connection::open_in_memory().unwrap();

        assert!(run_pending(&broken, &db_path, 1000).is_err());
        assert!(
            sidecar_path(&db_path).exists(),
            "a failed sync must NOT drop the capture — recovery retries it"
        );
    }

    #[test]
    fn reconcile_publishes_a_pending_capture_whose_install_committed() {
        // The crash: marker written, staged file consumed by the install
        // rename, process died before the publish. Without reconciliation
        // startup would ignore the pending capture and the next restore
        // would overwrite it — the outgoing vault's replica obligations
        // lost forever.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("brain.db");

        let live = crate::db::connection::open_app_db(&db_path, None).unwrap();
        seed(&live);
        drop(live);
        capture_to_sidecar(&db_path).unwrap();
        assert!(pending_sidecar_path(&db_path).exists());

        // Simulate the committed install's leftovers: no staged file (the
        // rename consumed it), a marker, and a preserved outgoing -wal.
        std::fs::write(install_marker_path(&db_path), b"").unwrap();
        std::fs::write(outgoing_wal_path(&db_path), b"stale wal").unwrap();

        let config_path = tmp.path().join("config.json");
        reconcile_interrupted_install(&db_path, &config_path)
            .expect("an empty marker has no destination to align; reconcile must succeed");

        assert!(
            sidecar_path(&db_path).exists(),
            "the owed capture must become visible to run_pending"
        );
        assert!(!pending_sidecar_path(&db_path).exists());
        assert!(
            !outgoing_wal_path(&db_path).exists(),
            "the outgoing -wal describes a database that is no longer at db_path"
        );
        assert!(!install_marker_path(&db_path).exists());
    }

    #[test]
    fn reconcile_rolls_back_an_install_that_never_committed() {
        // The crash: marker written, -wal preserved, sidecars removed, staged
        // copy on disk — but the install rename never ran, so brain.db is
        // still the outgoing database and its committed rows may live only
        // in the preserved -wal.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("brain.db");

        let live = crate::db::connection::open_app_db(&db_path, None).unwrap();
        seed(&live);
        drop(live);
        capture_to_sidecar(&db_path).unwrap();

        let wal = sibling(&db_path, "-wal");
        std::fs::write(&wal, b"journal with committed rows").unwrap();
        std::fs::write(staged_backup_path(&db_path), b"staged").unwrap();
        write_install_marker(&db_path, "").unwrap();
        preserve_outgoing_wal(&db_path).unwrap();
        assert!(outgoing_wal_path(&db_path).exists());
        std::fs::remove_file(&wal).unwrap(); // sidecar removal

        let config_path = tmp.path().join("config.json");
        reconcile_interrupted_install(&db_path, &config_path)
            .expect("a never-committed install has nothing to align; reconcile must succeed");

        assert_eq!(
            std::fs::read(&wal).unwrap(),
            b"journal with committed rows",
            "the outgoing database gets its journal back whole"
        );
        assert!(!outgoing_wal_path(&db_path).exists());
        assert!(
            !pending_sidecar_path(&db_path).exists(),
            "the capture belongs to a restore that never happened"
        );
        assert!(!sidecar_path(&db_path).exists());
        assert!(!staged_backup_path(&db_path).exists());
        assert!(!install_marker_path(&db_path).exists());
    }

    #[test]
    fn reconcile_committed_install_aligns_the_config_to_the_destination_root() {
        // The crash window the T4-follow-up Major covers: the install rename
        // ran (staged file consumed, marker present) but the process exited
        // BEFORE `set_vault_path` — the persisted config still names the
        // outgoing vault, while the installed database (whose rederive stamp
        // was cleared pre-rename) belongs to the destination. Recovery must
        // align the config to the DESTINATION the marker recorded, before the
        // startup's first rooted open rederives from the wrong root and
        // stamps that result complete.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("brain.db");
        let config_path = tmp.path().join("config.json");
        crate::vault::config::VaultConfig::new(config_path.clone())
            .set_vault_path(tmp.path().join("vault-a").to_str().unwrap())
            .unwrap();

        // Committed-install leftovers: no staged file (the rename consumed
        // it) and a marker that records the destination root.
        std::fs::write(install_marker_path(&db_path), b"vault-b").unwrap();

        reconcile_interrupted_install(&db_path, &config_path)
            .expect("the committed install's alignment must succeed here");

        let named = crate::vault::config::VaultConfig::new(config_path)
            .get_vault_path()
            .unwrap();
        assert_eq!(
            named.as_deref(),
            Some("vault-b"),
            "recovery left the config on the outgoing vault — the unstamped \
             installed brain would rederive (and stamp) the wrong root"
        );
    }

    #[test]
    fn reconcile_keeps_the_marker_when_the_marker_read_fails() {
        // T4-follow-up round 3, Major 1: the marker here is a DIRECTORY, so
        // reading it FAILS — which must be distinguished from "no marker". A
        // read failure means recovery cannot know the destination; reconcile
        // must retain the marker and propagate, so startup stops instead of
        // opening under the outgoing config and stamping the wrong root.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("brain.db");
        let config_path = tmp.path().join("config.json");
        crate::vault::config::VaultConfig::new(config_path.clone())
            .set_vault_path(tmp.path().join("vault-a").to_str().unwrap())
            .unwrap();
        // The install committed (no staged file) and its marker is unreadable.
        std::fs::create_dir_all(install_marker_path(&db_path)).unwrap();

        let outcome = reconcile_interrupted_install(&db_path, &config_path);

        assert!(
            outcome.is_err(),
            "a marker read failure must be propagated, not treated as no-destination"
        );
        assert!(
            install_marker_path(&db_path).exists(),
            "the marker must be retained for the next start's retry"
        );
    }

    #[test]
    fn reconcile_preserves_the_destination_exactly_no_trim() {
        // T4-follow-up round 3, Major 2: a valid Unix destination can end
        // with whitespace — `switch_vault` records it unchanged, and recovery
        // must align the config to it EXACTLY. Trimming would persist a
        // different root than the restore targeted.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("brain.db");
        let config_path = tmp.path().join("config.json");
        crate::vault::config::VaultConfig::new(config_path.clone())
            .set_vault_path(tmp.path().join("vault-a").to_str().unwrap())
            .unwrap();
        // Committed-install leftovers with a trailing-space destination.
        std::fs::write(install_marker_path(&db_path), b"/tmp/vault-b ").unwrap();

        let _ = reconcile_interrupted_install(&db_path, &config_path);

        let named = crate::vault::config::VaultConfig::new(config_path)
            .get_vault_path()
            .unwrap();
        assert_eq!(
            named.as_deref(),
            Some("/tmp/vault-b "),
            "the destination root must be preserved byte-exact"
        );
    }

    #[test]
    fn reconcile_removes_the_marker_before_the_staged_backup_on_rollback() {
        // T4-follow-up round 3, Major 3: a crash (or removal failure) between
        // "staged removed" and "marker removed" leaves marker-without-staged,
        // which the committed branch misreads as a committed install —
        // aligning the config to the recorded destination while the OUTGOING
        // database is still at db_path. The marker must go FIRST: any
        // leftover staged file then lands in the no-marker branch, which
        // discards it without touching the config.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("brain.db");
        let config_path = tmp.path().join("config.json");
        crate::vault::config::VaultConfig::new(config_path.clone())
            .set_vault_path(tmp.path().join("vault-a").to_str().unwrap())
            .unwrap();
        std::fs::write(staged_backup_path(&db_path), b"staged").unwrap();
        // A DIRECTORY as the marker: `remove_file` on it fails, simulating
        // an unremovable marker.
        std::fs::create_dir_all(install_marker_path(&db_path)).unwrap();

        let outcome = reconcile_interrupted_install(&db_path, &config_path);

        assert!(
            outcome.is_err(),
            "a marker-removal failure during rollback must be propagated"
        );
        assert!(
            staged_backup_path(&db_path).exists(),
            "the staged backup must be retained when the marker cannot be \
             removed — its absence would be misread as a committed install"
        );
        let named = crate::vault::config::VaultConfig::new(config_path)
            .get_vault_path()
            .unwrap();
        assert!(
            !named.as_deref().unwrap_or("").contains("vault-b"),
            "a rolled-back install must not move the config (names {named:?})"
        );
    }

    #[test]
    fn reconcile_keeps_the_marker_when_the_config_alignment_fails() {
        // T4-follow-up round 2: an alignment failure must be PROPAGATED and
        // the marker RETAINED — the destination is still recoverable for the
        // next start, which must retry the alignment instead of opening the
        // vault under the outgoing config (an unstamped installed brain
        // opened under the wrong root would rederive — and stamp complete —
        // the wrong root). The config path here is a DIRECTORY, so every
        // config write fails.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("brain.db");
        let config_dir = tmp.path().join("config.json");
        std::fs::create_dir_all(&config_dir).unwrap();

        // Committed-install leftovers: no staged file, marker records the
        // destination root.
        std::fs::write(install_marker_path(&db_path), b"vault-b").unwrap();

        let r = reconcile_interrupted_install(&db_path, &config_dir);

        assert!(r.is_err(), "an alignment failure must be propagated");
        assert!(
            install_marker_path(&db_path).exists(),
            "the marker must survive a failed alignment — it is the only record \
             of the destination root"
        );
    }

    #[test]
    fn reconcile_never_committed_leaves_the_outgoing_config_alone() {
        // The "never happened" side: staged file present ⇒ the install rename
        // did NOT run, so the config must keep naming the outgoing vault even
        // though a destination was recorded.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("brain.db");
        let config_path = tmp.path().join("config.json");
        crate::vault::config::VaultConfig::new(config_path.clone())
            .set_vault_path(tmp.path().join("vault-a").to_str().unwrap())
            .unwrap();
        std::fs::write(staged_backup_path(&db_path), b"staged").unwrap();
        write_install_marker(&db_path, "vault-b").unwrap();

        reconcile_interrupted_install(&db_path, &config_path)
            .expect("the committed install's alignment must succeed here");

        let named = crate::vault::config::VaultConfig::new(config_path)
            .get_vault_path()
            .unwrap();
        assert!(
            !named.as_deref().unwrap_or("").contains("vault-b"),
            "a never-committed install must not move the config (names {named:?})"
        );
    }

    #[test]
    fn reconcile_clears_strays_when_the_crash_predates_the_marker() {
        // The crash: capture written, staged copy half-written, marker never
        // reached the disk. Nothing was replaced; nothing is owed.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("brain.db");

        std::fs::write(pending_sidecar_path(&db_path), b"{}").unwrap();
        std::fs::write(staged_backup_path(&db_path), b"partial").unwrap();

        let config_path = tmp.path().join("config.json");
        reconcile_interrupted_install(&db_path, &config_path)
            .expect("no marker: nothing to align; reconcile must succeed");

        assert!(!pending_sidecar_path(&db_path).exists());
        assert!(!staged_backup_path(&db_path).exists());
        assert!(!sidecar_path(&db_path).exists());
        assert!(!install_marker_path(&db_path).exists());
    }

    #[test]
    fn preserved_outgoing_wal_restores_only_when_the_original_is_gone() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("brain.db");
        let wal = sibling(&db_path, "-wal");

        // No -wal to preserve: a no-op, not an error.
        preserve_outgoing_wal(&db_path).unwrap();
        assert!(!outgoing_wal_path(&db_path).exists());

        std::fs::write(&wal, b"w").unwrap();
        preserve_outgoing_wal(&db_path).unwrap();
        assert_eq!(std::fs::read(outgoing_wal_path(&db_path)).unwrap(), b"w");

        // Original gone -> the copy goes back under the real name.
        std::fs::remove_file(&wal).unwrap();
        restore_outgoing_wal(&db_path);
        assert!(wal.exists());
        assert!(!outgoing_wal_path(&db_path).exists());

        // Original present again -> the copy is redundant and dropped.
        std::fs::write(outgoing_wal_path(&db_path), b"w").unwrap();
        restore_outgoing_wal(&db_path);
        assert!(wal.exists());
        assert!(!outgoing_wal_path(&db_path).exists());
    }
}
