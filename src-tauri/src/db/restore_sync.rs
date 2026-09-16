//! Restore-path replica sync (issue #213 spec D3).
//!
//! A vault switch that restores a backup replaces `brain.db` wholesale. The
//! outbox lives inside that file, so without extra work the replica sees
//! nothing: the outgoing vault's records are never deleted from it, the
//! restored vault's are never re-inserted, and any of the outgoing vault's
//! events still undrained at copy time die with the old file.
//!
//! The sequence is **capture → stage → install → publish → sync**:
//!
//! 1. `capture_to_sidecar` writes the replica's outstanding obligations to an
//!    unpublished sidecar and fsyncs it. Holding the capture only in memory
//!    means a crash (or a failed sync) permanently destroys those obligations
//!    — the rows no longer exist anywhere and the divergence is silent.
//! 2. `stage_backup` copies the backup to a temporary file beside the
//!    database, so the database is never truncated mid-copy.
//! 3. `commit_staged_backup` installs it with one atomic rename.
//! 4. `publish_sidecar` renames the capture into the path crash recovery
//!    watches — only now, because a capture consumed before the install would
//!    be replayed against the *outgoing* database.
//! 5. `sync` (or `run_pending`, at startup, after a crash) pushes the
//!    obligations onto the restored file's outbox.

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

fn sibling(db_path: &Path, suffix: &str) -> PathBuf {
    let mut name = db_path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    db_path.with_file_name(name)
}

/// fsync the directory holding `path`, so a rename into it survives a crash.
///
/// A rename is atomic with respect to readers the instant it returns, but the
/// directory entry itself is not durable until its directory is synced. Best
/// effort on the open: some platforms refuse `File::open` on a directory, and
/// failing the whole restore over an unsyncable directory would be worse than
/// the (already small) window this closes.
fn fsync_parent_dir(path: &Path) -> Result<()> {
    let Some(dir) = path.parent() else {
        return Ok(());
    };
    match std::fs::File::open(dir) {
        Ok(f) => {
            let _ = f.sync_all();
            Ok(())
        }
        Err(_) => Ok(()),
    }
}

/// Rename `from` onto `to` and make the new directory entry durable.
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
/// the same run and only if this function returned `Ok`, and the next restore
/// truncates the file before publishing it.
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
/// this without inventing a failure.
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
    let file = std::fs::File::open(&staged)
        .with_context(|| format!("reopen {} to fsync", staged.display()))?;
    file.sync_all()
        .with_context(|| format!("fsync {}", staged.display()))?;
    Ok(staged)
}

/// Install a staged backup over `db_path` with a single atomic rename.
///
/// The caller must have removed the outgoing database's `-wal`/`-shm` first:
/// they belong to the old file and would be misread as this one's.
pub fn commit_staged_backup(staged: &Path, db_path: &Path) -> Result<()> {
    rename_durably(staged, db_path)
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

fn reassert_entries(conn: &Connection, now_ms: i64) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT id, entity_id, title, body, tags, confidence, source_type, source_hash,
                source_ref, okf_type, okf_sources, okf_verified, okf_usage_window,
                created_at, updated_at, deleted_at, lifecycle_status, stale_after,
                generated_by, last_verified_at, last_verified_by
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
}
