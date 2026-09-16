//! Restore-path replica sync (issue #213 spec D3).
//!
//! A vault switch that restores a backup replaces `brain.db` wholesale with
//! `std::fs::copy`. The outbox lives inside that file, so without extra work
//! the replica sees nothing: the outgoing vault's records are never deleted
//! from it, the restored vault's are never re-inserted, and any of the
//! outgoing vault's events still undrained at copy time die with the old file.
//!
//! The sequence is capture → copy → sync. The capture is persisted to a
//! sidecar file BEFORE the copy, because holding it only in memory means a
//! crash (or a failed sync) permanently destroys the outgoing vault's replica
//! obligations — its rows no longer exist anywhere and the divergence is
//! silent.

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
pub fn sidecar_path(db_path: &Path) -> PathBuf {
    let mut name = db_path.file_name().unwrap_or_default().to_os_string();
    name.push(".sync-capture");
    db_path.with_file_name(name)
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

/// Capture the live database and fsync it to the sidecar.
///
/// **Timing is load-bearing.** Call this after `release_global_db_lock` has
/// swapped in the stub and **before** `remove_sqlite_sidecars` deletes the
/// `-wal`. "Any time before the copy" is not good enough: a concurrent
/// process — the headless `--mcp` server holds its own brain.db connection —
/// prevents the implicit WAL checkpoint on close, so rows written since the
/// last checkpoint exist only in the `-wal` that sidecar removal is about to
/// delete. A capture after that point reads a stale main file and silently
/// misses records.
pub fn capture_to_sidecar(db_path: &Path) -> Result<()> {
    let conn = Connection::open(db_path)
        .with_context(|| format!("open {} for restore-sync capture", db_path.display()))?;
    let state = capture(&conn)?;
    drop(conn);

    let path = sidecar_path(db_path);
    let json = serde_json::to_vec(&state).context("serialize restore-sync capture")?;
    let mut file =
        std::fs::File::create(&path).with_context(|| format!("create {}", path.display()))?;
    file.write_all(&json)?;
    file.sync_all().context("fsync restore-sync capture")?;
    Ok(())
}

/// The pending capture, or `None` when there is nothing to finish.
///
/// A sidecar that will not parse is treated as absent and removed: it can only
/// come from a crash mid-write, and a half-written capture is not actionable.
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
        assert!(sidecar_path(&db_path).exists());

        let restored = read_sidecar(&db_path).unwrap().expect("sidecar must parse");
        assert_eq!(restored.entries.len(), 2);
        assert_eq!(restored.tasks.len(), 1);
        assert_eq!(restored.outbox.len(), 1);
    }
}
