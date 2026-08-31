//! Ingest watchdog. Spec: docs/superpowers/specs/2026-08-31-ingest-drain-stall-watchdog-design.md
pub mod diagnostics;
pub mod heartbeat;
pub mod budgets;
pub mod recovery;
pub mod sweep;

use std::time::{Duration, Instant};

use self::budgets::budget_for;
use self::heartbeat::{HeartbeatSnapshot, Stage};
use crate::embedder::EmbedProfile;

/// How long the queue may sit non-empty and idle before it counts as a drain
/// stall. Must exceed the longest legitimate single-document time
/// (Extracting 300s + Embedding up to 660s ≈ 16 min); the `Idle` requirement
/// means a long-running document cannot trip it regardless (spec §2.3).
pub const DRAIN_STALL_WINDOW: Duration = Duration::from_secs(900);

/// How often the supervisor wakes.
pub const TICK: Duration = Duration::from_secs(5);

/// Wait for a worker thread, giving up after `timeout`. `std::thread` has no
/// timed join, so the thread signals completion over a channel and the caller
/// abandons it on timeout — which is exactly what `switch_vault` needs so it
/// stops inheriting a wedge (spec §6).
pub fn join_with_timeout(
    join: std::thread::JoinHandle<()>,
    timeout: Duration,
) -> bool {
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        let _ = join.join();
        let _ = done_tx.send(());
    });
    done_rx.recv_timeout(timeout).is_ok()
}

/// Returns `Some(stalled_ms)` when the current stage has exceeded its budget.
pub fn evaluate_stage_stall(
    snapshot: &HeartbeatSnapshot,
    profile: &EmbedProfile,
    gen_timeout_secs: u64,
    now_ms: i64,
) -> Option<i64> {
    let budget = budget_for(snapshot.stage, profile, gen_timeout_secs)?;
    let elapsed = now_ms.saturating_sub(snapshot.stage_started_ms);
    if elapsed > budget.as_millis() as i64 {
        Some(elapsed)
    } else {
        None
    }
}

/// Detects the failure class the stage heartbeat cannot see: work is queued
/// but nothing consumes it, so the worker is legitimately `Idle` forever
/// (spec §1.4 class 1, §2.3).
#[derive(Debug, Default)]
pub struct DrainTracker {
    since: Option<Instant>,
    last_pending: Option<i64>,
    last_completed: Option<i64>,
}

impl DrainTracker {
    pub fn new() -> Self {
        DrainTracker {
            since: None,
            last_pending: None,
            last_completed: None,
        }
    }

    /// Feed one observation. Returns true when a drain stall has persisted
    /// for the full window.
    ///
    /// `completed` is the count of documents in a terminal state
    /// (`indexed` or `error`) — any increase means the system is making
    /// progress, so the window resets.
    pub fn observe(
        &mut self,
        pending: i64,
        completed: i64,
        stage: Stage,
        now: Instant,
    ) -> bool {
        let progressing = self.last_completed.map_or(false, |last| completed != last);
        let quiet = pending > 0 && stage == Stage::Idle && !progressing;

        self.last_pending = Some(pending);
        self.last_completed = Some(completed);

        if !quiet {
            self.since = None;
            return false;
        }

        match self.since {
            None => {
                self.since = Some(now);
                false
            }
            Some(start) => now.duration_since(start) >= DRAIN_STALL_WINDOW,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::heartbeat::{HeartbeatSnapshot, Stage};
    use super::*;
    use crate::embedder::EmbedProfile;
    use std::time::{Duration, Instant};

    fn ollama() -> EmbedProfile {
        EmbedProfile::Local {
            model: "nomic-embed-text".to_string(),
        }
    }

    fn snap(stage: Stage, started_ms: i64) -> HeartbeatSnapshot {
        HeartbeatSnapshot {
            epoch: 0,
            seq: 1,
            stage,
            subject: "/a.md".to_string(),
            stage_started_ms: started_ms,
        }
    }

    #[test]
    fn idle_never_trips_a_stage_stall_however_long() {
        let s = snap(Stage::Idle, 0);
        assert!(evaluate_stage_stall(&s, &ollama(), 600, 999_999_999).is_none());
    }

    #[test]
    fn embedding_over_the_active_profile_budget_trips() {
        // Ollama budget is 600 + 60 = 660s.
        let s = snap(Stage::Embedding, 0);
        assert!(evaluate_stage_stall(&s, &ollama(), 600, 659_000).is_none());
        let tripped = evaluate_stage_stall(&s, &ollama(), 600, 661_000).unwrap();
        assert_eq!(tripped, 661_000);
    }

    #[test]
    fn drain_tracker_trips_when_pending_stays_and_nothing_completes() {
        let mut t = DrainTracker::new();
        let t0 = Instant::now();
        assert!(!t.observe(83, 10, Stage::Idle, t0));
        // Same pending, same completion count, still idle, past the window.
        assert!(t.observe(83, 10, Stage::Idle, t0 + DRAIN_STALL_WINDOW + Duration::from_secs(1)));
    }

    #[test]
    fn drain_tracker_does_not_trip_while_completions_advance() {
        let mut t = DrainTracker::new();
        let t0 = Instant::now();
        assert!(!t.observe(83, 10, Stage::Idle, t0));
        // A completion landed — progress is being made.
        assert!(!t.observe(82, 11, Stage::Idle, t0 + DRAIN_STALL_WINDOW + Duration::from_secs(1)));
    }

    #[test]
    fn drain_tracker_does_not_trip_with_an_empty_queue() {
        let mut t = DrainTracker::new();
        let t0 = Instant::now();
        assert!(!t.observe(0, 10, Stage::Idle, t0));
        assert!(!t.observe(0, 10, Stage::Idle, t0 + DRAIN_STALL_WINDOW + Duration::from_secs(1)));
    }

    #[test]
    fn drain_tracker_does_not_trip_while_a_document_is_in_flight() {
        // A long Embedding is not a drain stall regardless of duration — the
        // stage-stall path owns that case (spec §2.3).
        let mut t = DrainTracker::new();
        let t0 = Instant::now();
        assert!(!t.observe(83, 10, Stage::Embedding, t0));
        assert!(!t.observe(
            83,
            10,
            Stage::Embedding,
            t0 + DRAIN_STALL_WINDOW + Duration::from_secs(1)
        ));
    }

    #[test]
    fn join_with_timeout_returns_false_for_a_wedged_thread() {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            // Blocks until the test drops `tx` — stands in for a wedge.
            let _ = rx.recv();
        });

        let finished = join_with_timeout(handle, Duration::from_millis(200));
        assert!(!finished, "a wedged thread must not be waited on forever");

        drop(tx); // let the stand-in thread exit
    }

    #[test]
    fn join_with_timeout_returns_true_for_a_thread_that_finishes() {
        let handle = std::thread::spawn(|| {});
        assert!(join_with_timeout(handle, Duration::from_secs(5)));
    }
}
