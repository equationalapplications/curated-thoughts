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

    /// Drop claims for paths that are no longer sweepable.
    ///
    /// A claim is only meant to cover the window between `try_send` and the
    /// worker finishing the job. Nothing signals completion back to the
    /// supervisor, so completion is inferred from the document leaving the
    /// pending set: once the row is `indexed`/`error` the claim is stale.
    /// Without this the set grows to channel capacity and never shrinks, and
    /// — worse — a path swept once is skipped by every later sweep, which
    /// disables the backstop this module exists to provide
    /// (CodeRabbit review PRRT_kwDOSVmXas6d3ZZQ).
    pub fn retain_sweepable(&mut self, sweepable: &HashSet<String>) {
        self.paths.retain(|p| sweepable.contains(p));
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.paths.len()
    }
}

/// Status marking a row that was deferred by a full channel and must be
/// re-enqueued as a *forced* rechunk rather than a plain ingest.
pub const STATUS_PENDING_REINDEX: &str = "pending_reindex";

/// Every sweepable path with its status. The status is load-bearing:
/// `pending_reindex` rows were staged by `queue_full_reindex` /
/// `run_wiki_reembed`, whose whole point is a forced rechunk. Re-enqueueing
/// them as a plain ingest would let `ingest_file`'s unchanged-hash check
/// short-circuit and silently drop the chunk-strategy or embedding-model
/// upgrade (CodeRabbit review PRRT_kwDOSVmXas6d3ZZM).
pub fn list_sweepable_pending(conn: &Connection, limit: usize) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT path, status FROM documents
          WHERE status IN ('pending', 'pending_reindex') AND quarantined_at IS NULL
          ORDER BY id
          LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// The full sweepable set, used to expire stale claims. Unbounded on purpose:
/// `retain_sweepable` must see every sweepable path or it would drop live
/// claims for rows merely beyond the batch limit.
pub fn sweepable_path_set(conn: &Connection) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(
        "SELECT path FROM documents
          WHERE status IN ('pending', 'pending_reindex') AND quarantined_at IS NULL",
    )?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut out = HashSet::new();
    for row in rows {
        out.insert(row?);
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
    // Expire claims for rows that have left the pending set — that is the
    // only completion signal the supervisor gets (spec §5).
    claims.retain_sweepable(&sweepable_path_set(conn)?);

    let paths = list_sweepable_pending(conn, limit)?;
    let mut queued = 0usize;
    for (path, status) in paths {
        if claims.contains(&path) {
            continue;
        }
        // Preserve the deferred-reindex intent: those rows need force=true.
        let job = if status == STATUS_PENDING_REINDEX {
            PipelineJob::rechunk_for_reembed(path.clone())
        } else {
            PipelineJob::ingest_counted(path.clone())
        };
        match tx.try_send(job) {
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
    fn pending_reindex_rows_are_swept_as_a_forced_rechunk() {
        // The whole point of a deferred reindex is force=true. Sweeping it as
        // a plain ingest lets the unchanged-hash check short-circuit and the
        // rechunk is silently dropped.
        let conn = open_in_memory().unwrap();
        seed(&conn, "/plain.md", "pending");
        seed(&conn, "/reindex.md", "pending_reindex");

        let (tx, rx) = sync_channel(16);
        let mut claims = InFlightClaims::new();
        assert_eq!(sweep(&conn, &tx, &mut claims, 100).unwrap(), 2);
        drop(tx);

        let got: Vec<(String, bool)> = rx
            .iter()
            .map(|j| match j {
                PipelineJob::Ingest { path, force, .. } => (path, force),
                PipelineJob::Delete(path) => (path, false),
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("/plain.md".to_string(), false),
                ("/reindex.md".to_string(), true),
            ]
        );
    }

    #[test]
    fn claims_expire_once_the_document_leaves_the_pending_set() {
        // Nothing signals job completion back to the supervisor, so the sweep
        // infers it from the row leaving the pending set. Without this the
        // claim set grows without bound and a path swept once is skipped
        // forever, disabling the backstop.
        let conn = open_in_memory().unwrap();
        seed(&conn, "/done.md", "pending");

        let (tx, _rx) = sync_channel(16);
        let mut claims = InFlightClaims::new();
        assert_eq!(sweep(&conn, &tx, &mut claims, 100).unwrap(), 1);
        assert_eq!(claims.len(), 1);

        // The worker finishes: the row is no longer sweepable.
        conn.execute(
            "UPDATE documents SET status = 'indexed' WHERE path = '/done.md'",
            [],
        )
        .unwrap();

        assert_eq!(sweep(&conn, &tx, &mut claims, 100).unwrap(), 0);
        assert_eq!(claims.len(), 0, "completed path must not stay claimed");
    }

    #[test]
    fn an_expired_claim_lets_a_requeued_path_be_swept_again() {
        // The regression that matters: a path swept once, completed, then made
        // pending again by the watcher must still be rescued by the sweep.
        let conn = open_in_memory().unwrap();
        seed(&conn, "/again.md", "pending");

        let (tx, _rx) = sync_channel(16);
        let mut claims = InFlightClaims::new();
        assert_eq!(sweep(&conn, &tx, &mut claims, 100).unwrap(), 1);

        conn.execute(
            "UPDATE documents SET status = 'indexed' WHERE path = '/again.md'",
            [],
        )
        .unwrap();
        assert_eq!(sweep(&conn, &tx, &mut claims, 100).unwrap(), 0);

        conn.execute(
            "UPDATE documents SET status = 'pending' WHERE path = '/again.md'",
            [],
        )
        .unwrap();
        assert_eq!(
            sweep(&conn, &tx, &mut claims, 100).unwrap(),
            1,
            "a re-pending path must be swept again"
        );
    }

    #[test]
    fn a_long_running_in_flight_path_keeps_its_claim() {
        // The claim must survive while the row is still pending, or a slow
        // Extracting/Embedding job gets re-enqueued every pass (spec §5).
        let conn = open_in_memory().unwrap();
        seed(&conn, "/slow.md", "pending");

        let (tx, _rx) = sync_channel(16);
        let mut claims = InFlightClaims::new();
        assert_eq!(sweep(&conn, &tx, &mut claims, 100).unwrap(), 1);

        for _ in 0..3 {
            assert_eq!(sweep(&conn, &tx, &mut claims, 100).unwrap(), 0);
        }
        assert_eq!(claims.len(), 1, "still in flight, still claimed");
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
