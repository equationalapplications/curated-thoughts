// Implementation is added in Step 3 after the test verifies compilation failure.

use std::collections::HashSet;
use std::sync::mpsc::{SyncSender, TrySendError};
use std::time::Duration;

use anyhow::Result;
use rusqlite::Connection;

use crate::pipeline::PipelineJob;

#[allow(dead_code)]
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// Paths whose `PipelineJob::ingest_counted` is currently in the channel or
/// being processed. The sweep skips these so a long `Extracting` /
/// `Embedding` job is not re-enqueued by the next 60s pass (spec §5).
#[derive(Debug, Default)]
pub struct InFlightClaims {
    paths: HashSet<String>,
}

impl InFlightClaims {
    pub fn new() -> Self {
        InFlightClaims {
            paths: HashSet::new(),
        }
    }

    /// Returns true when `path` is currently claimed.
    pub fn contains(&self, path: &str) -> bool {
        self.paths.contains(path)
    }

    /// Mark `path` as in-flight. Called after `try_send` succeeds.
    pub fn claim(&mut self, path: String) {
        self.paths.insert(path);
    }

    /// Drop the claim because `try_send` returned `QueueFull`: the path stays
    /// `pending` and will be picked up on the next sweep.
    pub fn release(&mut self, path: &str) {
        self.paths.remove(path);
    }

    /// Clear all claims. Called when the worker is respawned, since the
    /// abandoned in-flight set is gone with the abandoned worker (spec §5).
    pub fn clear(&mut self) {
        self.paths.clear();
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.paths.len()
    }
}

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

/// Sweep one batch. Skips paths already in `claims`, claims successful enqueues,
/// and releases the claim on `QueueFull` so the path stays pending for the next
/// pass (spec §5).
pub fn sweep(
    conn: &Connection,
    tx: &SyncSender<PipelineJob>,
    claims: &mut InFlightClaims,
    limit: usize,
) -> Result<usize> {
    let paths = list_sweepable_pending(conn, limit)?;
    let mut queued = 0usize;
    for path in paths {
        if claims.contains(&path) {
            continue;
        }
        match tx.try_send(PipelineJob::ingest_counted(path.clone())) {
            Ok(()) => {
                claims.claim(path);
                queued += 1;
            }
            Err(TrySendError::Full(_)) => {
                claims.release(&path);
                break;
            }
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
        let mut claims = InFlightClaims::new();
        let n = sweep(&conn, &tx, &mut claims, 100).unwrap();

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
        assert_eq!(claims.len(), 2, "claimed paths must be tracked");
    }

    #[test]
    fn sweep_skips_quarantined_paths() {
        let conn = open_in_memory().unwrap();
        seed(&conn, "/poison.md", "pending");
        seed(&conn, "/ok.md", "pending");
        quarantine(&conn, "/poison.md").unwrap();

        let (tx, rx) = sync_channel(16);
        let mut claims = InFlightClaims::new();
        let n = sweep(&conn, &tx, &mut claims, 100).unwrap();

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
        let mut claims = InFlightClaims::new();
        let n = sweep(&conn, &tx, &mut claims, 100).unwrap();
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

    #[test]
    fn sweep_skips_paths_already_in_flight() {
        // Spec §5: a long Extracting/Embedding must not be re-enqueued by the
        // next 60s sweep. Claiming a path keeps subsequent sweeps from
        // selecting it.
        let conn = open_in_memory().unwrap();
        seed(&conn, "/long.md", "pending");
        seed(&conn, "/new.md", "pending");

        let (tx, _rx) = sync_channel(16);
        let mut claims = InFlightClaims::new();

        // First sweep claims both rows.
        let n1 = sweep(&conn, &tx, &mut claims, 100).unwrap();
        assert_eq!(n1, 2);

        // Second sweep finds no unclaimed rows and queues nothing.
        let n2 = sweep(&conn, &tx, &mut claims, 100).unwrap();
        assert_eq!(n2, 0, "claimed paths must be skipped");
    }

    #[test]
    fn queue_full_releases_the_claim_so_next_pass_can_retry() {
        // Spec §5: try_send returning Full must release the claim so the path
        // stays pending and a later sweep can pick it up.
        let conn = open_in_memory().unwrap();
        seed(&conn, "/retry.md", "pending");

        let (tx, _rx) = sync_channel(0);
        let mut claims = InFlightClaims::new();
        let n = sweep(&conn, &tx, &mut claims, 100).unwrap();
        assert_eq!(n, 0, "capacity 0 cannot accept any enqueue");
        assert_eq!(claims.len(), 0, "Full path stays pending");
    }

    #[test]
    fn claims_clear_on_respawn() {
        // Spec §5: respawning the worker abandons the in-flight set, so the
        // supervisor must clear the claims to avoid losing rows permanently.
        let conn = open_in_memory().unwrap();
        seed(&conn, "/x.md", "pending");

        let (tx, _rx) = sync_channel(16);
        let mut claims = InFlightClaims::new();
        sweep(&conn, &tx, &mut claims, 100).unwrap();
        assert!(claims.len() > 0);

        claims.clear();
        assert_eq!(claims.len(), 0);

        // After clear, the sweep picks the row up again.
        let n = sweep(&conn, &tx, &mut claims, 100).unwrap();
        assert_eq!(n, 1);
    }
}
