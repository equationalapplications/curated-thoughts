// Implementation is added in Step 3 after the test verifies compilation failure.

use std::sync::mpsc::{SyncSender, TrySendError};
use std::time::Duration;

use anyhow::Result;
use rusqlite::Connection;

use crate::pipeline::PipelineJob;

#[allow(dead_code)]
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

pub fn list_sweepable_pending(conn: &Connection, limit: usize) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT path FROM documents
          WHERE status = 'pending' AND quarantined_at IS NULL
          ORDER BY id
          LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn sweep(
    conn: &Connection,
    tx: &SyncSender<PipelineJob>,
    limit: usize,
) -> Result<usize> {
    let paths = list_sweepable_pending(conn, limit)?;
    let mut queued = 0usize;
    for path in paths {
        match tx.try_send(PipelineJob::ingest_counted(path)) {
            Ok(()) => queued += 1,
            Err(TrySendError::Full(_)) => break,
            Err(TrySendError::Disconnected(_)) => {
                eprintln!("[watchdog] sweep aborted: pipeline channel closed");
                break;
            }
        }
    }
    Ok(queued)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::open_in_memory;
    use crate::pipeline::watchdog::recovery::quarantine;
    use std::sync::mpsc::sync_channel;

    fn seed(conn: &rusqlite::Connection, path: &str, status: &str) {
        conn.execute(
            "INSERT INTO documents (path, hash, status, tier) VALUES (?1, 'h', ?2, 'user_doc')",
            rusqlite::params![path, status],
        )
        .unwrap();
    }

    #[test]
    fn sweep_enqueues_pending_rows() {
        let conn = open_in_memory().unwrap();
        seed(&conn, "/a.md", "pending");
        seed(&conn, "/b.md", "pending");
        seed(&conn, "/c.md", "indexed");

        let (tx, rx) = sync_channel(16);
        let n = sweep(&conn, &tx, 100).unwrap();

        assert_eq!(n, 2, "only pending rows are swept");
        drop(tx);
        let got: Vec<String> = rx
            .iter()
            .map(|j| match j {
                PipelineJob::Ingest { path, .. } => path,
                PipelineJob::Delete(path) => path,
            })
            .collect();
        assert_eq!(got, vec!["/a.md".to_string(), "/b.md".to_string()]);
    }

    #[test]
    fn sweep_skips_quarantined_paths() {
        let conn = open_in_memory().unwrap();
        seed(&conn, "/poison.md", "pending");
        seed(&conn, "/ok.md", "pending");
        quarantine(&conn, "/poison.md").unwrap();

        let (tx, rx) = sync_channel(16);
        let n = sweep(&conn, &tx, 100).unwrap();

        assert_eq!(n, 1);
        drop(tx);
        let got: Vec<String> = rx
            .iter()
            .map(|j| match j {
                PipelineJob::Ingest { path, .. } => path,
                PipelineJob::Delete(path) => path,
            })
            .collect();
        assert_eq!(got, vec!["/ok.md".to_string()]);
    }

    #[test]
    fn sweep_respects_channel_capacity_and_leaves_the_rest_pending() {
        let conn = open_in_memory().unwrap();
        for i in 0..5 {
            seed(&conn, &format!("/f{i}.md"), "pending");
        }

        // Capacity 2: try_send must stop rather than block (spec §5).
        let (tx, rx) = sync_channel(2);
        let n = sweep(&conn, &tx, 100).unwrap();
        assert_eq!(n, 2, "sweep stops at channel capacity");

        // The unswept rows are still pending for the next pass.
        let still: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE status = 'pending'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(still, 5);
        drop(rx);
    }
}
