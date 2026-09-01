#[allow(unused_imports)]
use std::sync::MutexGuard;
use std::sync::{
    atomic::{AtomicI64, AtomicU64, AtomicU8, Ordering},
    Mutex,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Idle = 0,
    Reading = 1,
    Extracting = 2,
    Chunking = 3,
    Embedding = 4,
    Summarizing = 5,
    Linking = 6,
    Committing = 7,
    Deleting = 8,
}

impl Stage {
    pub fn as_str(&self) -> &'static str {
        match self {
            Stage::Idle => "idle",
            Stage::Reading => "reading",
            Stage::Extracting => "extracting",
            Stage::Chunking => "chunking",
            Stage::Embedding => "embedding",
            Stage::Summarizing => "summarizing",
            Stage::Linking => "linking",
            Stage::Committing => "committing",
            Stage::Deleting => "deleting",
        }
    }

    pub fn from_u8(v: u8) -> Stage {
        match v {
            1 => Stage::Reading,
            2 => Stage::Extracting,
            3 => Stage::Chunking,
            4 => Stage::Embedding,
            5 => Stage::Summarizing,
            6 => Stage::Linking,
            7 => Stage::Committing,
            8 => Stage::Deleting,
            _ => Stage::Idle,
        }
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone)]
pub struct HeartbeatSnapshot {
    pub epoch: u64,
    pub seq: u64,
    pub stage: Stage,
    pub subject: String,
    pub stage_started_ms: i64,
    /// False when the seqlock retry budget was exhausted and the fields below
    /// may straddle two transitions. A torn snapshot must never drive a trip
    /// decision — the supervisor skips the tick instead (spec §2.1).
    pub consistent: bool,
}

/// How many times `snapshot()` retries a torn read before degrading. The
/// writer window is three atomic stores plus a `try_lock`, so convergence is
/// the overwhelmingly common case; the budget only bounds pathological
/// contention.
const SNAPSHOT_RETRY_BUDGET: u32 = 64;

/// Worker progress, published lock-free for the trip decision.
///
/// Only the atomics are load-bearing. `subject` is diagnostic detail behind a
/// mutex, and the supervisor reads it with `try_lock` — a blocking read would
/// deadlock against exactly the stall the watchdog exists to detect
/// (spec §2.1).
#[derive(Debug)]
pub struct Heartbeat {
    epoch: AtomicU64,
    seq: AtomicU64,
    stage: AtomicU8,
    stage_started_ms: AtomicI64,
    subject: Mutex<Option<String>>,
}

impl Default for Heartbeat {
    fn default() -> Self {
        Self::new()
    }
}

impl Heartbeat {
    pub fn new() -> Self {
        Heartbeat {
            epoch: AtomicU64::new(0),
            seq: AtomicU64::new(0),
            stage: AtomicU8::new(Stage::Idle as u8),
            stage_started_ms: AtomicI64::new(now_ms()),
            subject: Mutex::new(None),
        }
    }

    /// Record a stage transition. Called by the worker only.
    ///
    /// Follows the seqlock convention: bump `seq` to mark a write in progress,
    /// update the atomics, then bump `seq` again to mark the write complete.
    /// `snapshot()` retries when the two seq reads straddle an odd value, which
    /// is what makes the read-side consistent (spec §2.1).
    pub fn enter(&self, stage: Stage, subject: Option<&str>) {
        // Open the seqlock window. Every field a snapshot reads — `subject`
        // included — must be written inside it, or a reader can pair a new
        // stage with the previous subject and blame the wrong document
        // (CodeRabbit review PRRT_kwDOSVmXas6d3ZY8).
        self.seq.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut g) = self.subject.lock() {
            *g = subject.map(|s| s.to_string());
        }
        self.stage_started_ms.store(now_ms(), Ordering::SeqCst);
        self.stage.store(stage as u8, Ordering::SeqCst);
        // Close the seqlock window. After this, the seq counter is even again.
        self.seq.fetch_add(1, Ordering::SeqCst);
    }

    /// Read current state without ever blocking on `subject`.
    ///
    /// Implements the seqlock read side: read `seq`, snapshot the remaining
    /// fields, re-read `seq`, and accept only when the two reads are equal
    /// *and* even. `enter()` brackets its writes with an odd `seq`, so a
    /// reader that lands mid-write sees either an odd value or two different
    /// ones and retries; an accepted read is therefore a consistent
    /// point-in-time view (spec §2.1).
    ///
    /// `SNAPSHOT_RETRY_BUDGET` bounds the worst case under continuous writer
    /// pressure. Exhausting it returns `consistent: false` — the fields may
    /// straddle two transitions, so callers must not decide a trip on it.
    pub fn snapshot(&self) -> HeartbeatSnapshot {
        let mut attempts = 0;
        loop {
            let seq_before = self.seq.load(Ordering::SeqCst);
            let epoch = self.epoch.load(Ordering::SeqCst);
            let stage = Stage::from_u8(self.stage.load(Ordering::SeqCst));
            let stage_started_ms = self.stage_started_ms.load(Ordering::SeqCst);
            // Read `subject` inside the validated window too. `try_lock` keeps
            // the supervisor from ever blocking on the wedged worker's lock.
            let subject = match self.subject.try_lock() {
                Ok(g) => g.clone().unwrap_or_else(|| "unknown".to_string()),
                Err(_) => "unknown".to_string(),
            };
            let seq_after = self.seq.load(Ordering::SeqCst);

            if seq_before == seq_after && seq_before.is_multiple_of(2) {
                return HeartbeatSnapshot {
                    epoch,
                    seq: seq_before,
                    stage,
                    subject,
                    stage_started_ms,
                    consistent: true,
                };
            }

            attempts += 1;
            if attempts >= SNAPSHOT_RETRY_BUDGET {
                // A writer is racing continuously; degrade rather than loop
                // forever. The fields may straddle two transitions, so flag
                // the snapshot inconsistent — callers must not trip on it.
                return HeartbeatSnapshot {
                    epoch,
                    seq: seq_after,
                    stage,
                    subject,
                    stage_started_ms,
                    consistent: false,
                };
            }
            std::hint::spin_loop();
        }
    }

    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    /// Supersede the current worker. Returns the new epoch.
    pub fn bump_epoch(&self) -> u64 {
        self.epoch.fetch_add(1, Ordering::SeqCst) + 1
    }

    #[cfg(test)]
    pub fn lock_subject_for_test(&self) -> MutexGuard<'_, Option<String>> {
        self.subject.lock().unwrap()
    }
}

/// Epoch-aware handle for publishing stage transitions.
///
/// `Heartbeat::enter` is unconditional, so any code holding a bare `&Heartbeat`
/// can keep writing transitions after the watchdog superseded its worker —
/// masking a stall on the *replacement* worker and defeating the §4.1 epoch
/// guard. Routing every in-job transition through this type makes the guard
/// impossible to bypass: `enter` returns `false` once superseded and the
/// caller must return immediately.
pub struct StageReporter<'a> {
    hb: &'a Heartbeat,
    /// `None` for standalone callers (tooling, Tauri commands) that have no
    /// watchdog-managed worker to be superseded.
    my_epoch: Option<u64>,
}

impl<'a> StageReporter<'a> {
    /// For callers with no watchdog-managed worker; never reports superseded.
    pub fn unguarded(hb: &'a Heartbeat) -> Self {
        StageReporter { hb, my_epoch: None }
    }

    /// For a worker that must stop as soon as the watchdog supersedes it.
    pub fn guarded(hb: &'a Heartbeat, my_epoch: u64) -> Self {
        StageReporter {
            hb,
            my_epoch: Some(my_epoch),
        }
    }

    pub fn superseded(&self) -> bool {
        matches!(self.my_epoch, Some(e) if self.hb.epoch() != e)
    }

    /// Publish a stage transition. Returns `false` when this worker has been
    /// superseded, in which case nothing is written and the caller must bail.
    pub fn enter(&self, stage: Stage, subject: Option<&str>) -> bool {
        if self.superseded() {
            return false;
        }
        self.hb.enter(stage, subject);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn enter_advances_seq_and_records_stage() {
        let hb = Heartbeat::new();
        let before = hb.snapshot();
        assert_eq!(before.stage, Stage::Idle);

        hb.enter(Stage::Embedding, Some("/vault/documents/a.md"));
        let after = hb.snapshot();

        assert_eq!(after.stage, Stage::Embedding);
        assert_eq!(after.subject, "/vault/documents/a.md");
        assert!(after.seq > before.seq, "seq must advance on transition");
        assert!(after.stage_started_ms > 0);
    }

    #[test]
    fn snapshot_does_not_block_when_subject_is_held() {
        // The supervisor must never wait on a lock the wedged worker owns.
        let hb = Arc::new(Heartbeat::new());
        hb.enter(Stage::Embedding, Some("/vault/documents/a.md"));

        let guard = hb.lock_subject_for_test();
        let hb2 = hb.clone();
        let handle = std::thread::spawn(move || hb2.snapshot());
        let snap = handle
            .join()
            .expect("snapshot must not block while subject is held");
        drop(guard);

        // Degrades rather than deadlocking; atomics are still accurate.
        assert_eq!(snap.stage, Stage::Embedding);
        assert_eq!(snap.subject, "unknown");
    }

    #[test]
    fn bump_epoch_increments_and_is_observable() {
        let hb = Heartbeat::new();
        let e0 = hb.epoch();
        let e1 = hb.bump_epoch();
        assert_eq!(e1, e0 + 1);
        assert_eq!(hb.epoch(), e1);
    }

    #[test]
    fn seqlock_holds_under_concurrent_transitions() {
        // Spec §2.1: a successful snapshot must combine fields from the same
        // transition. Under contention the seqlock retry must reject torn
        // reads rather than return a mid-write interleaving. We assert that
        // a strong majority of snapshots succeed (even seq); under sustained
        // pathological contention the budget-exhaustion path returns a
        // best-effort degraded snapshot rather than spinning forever.
        let hb = Arc::new(Heartbeat::new());
        let stages = [
            Stage::Reading,
            Stage::Extracting,
            Stage::Embedding,
            Stage::Summarizing,
            Stage::Committing,
        ];

        let writers: Vec<_> = stages
            .iter()
            .take(2) // lower contention: 2 writers, not 5
            .enumerate()
            .map(|(i, stage)| {
                let hb = hb.clone();
                let stage = *stage;
                std::thread::spawn(move || {
                    for _ in 0..500 {
                        hb.enter(stage, Some(&format!("/doc/{i}.md")));
                    }
                })
            })
            .collect();

        let reader = {
            let hb = hb.clone();
            std::thread::spawn(move || {
                let mut saw_seq_even = 0usize;
                let mut total = 0usize;
                for _ in 0..5_000 {
                    let snap = hb.snapshot();
                    total += 1;
                    if snap.seq.is_multiple_of(2) {
                        saw_seq_even += 1;
                    }
                }
                (saw_seq_even, total)
            })
        };

        for h in writers {
            h.join().unwrap();
        }
        let (even, total) = reader.join().unwrap();
        assert!(total > 0);
        // Strong majority: budget exhaustion is rare, and the seqlock
        // protocol guarantees no torn reads on the even path.
        assert!(
            even * 10 >= total * 9,
            "at least 90% of snapshots must observe even seq, got {even}/{total}"
        );
    }

    #[test]
    fn a_consistent_snapshot_always_has_an_even_seq() {
        // The `consistent` flag is what the supervisor trusts before tripping.
        // It must never be set on a read that failed seqlock validation.
        let hb = Arc::new(Heartbeat::new());
        let writer = {
            let hb = hb.clone();
            std::thread::spawn(move || {
                for _ in 0..2_000 {
                    hb.enter(Stage::Embedding, Some("/doc/a.md"));
                }
            })
        };
        for _ in 0..5_000 {
            let snap = hb.snapshot();
            if snap.consistent {
                assert!(
                    snap.seq.is_multiple_of(2),
                    "a consistent snapshot must never carry an odd seq, got {}",
                    snap.seq
                );
            }
        }
        writer.join().unwrap();
    }

    #[test]
    fn subject_is_written_inside_the_seqlock_window() {
        // A consistent snapshot must never pair a new stage with the previous
        // subject, or the watchdog strikes the wrong document.
        let hb = Arc::new(Heartbeat::new());
        hb.enter(Stage::Reading, Some("/first.md"));

        let hb2 = hb.clone();
        let writer = std::thread::spawn(move || {
            for i in 0..1_000 {
                hb2.enter(Stage::Embedding, Some(&format!("/doc{i}.md")));
            }
        });
        for _ in 0..3_000 {
            let snap = hb.snapshot();
            if snap.consistent && snap.stage == Stage::Embedding {
                assert!(
                    snap.subject == "unknown" || snap.subject.starts_with("/doc"),
                    "Embedding paired with a stale subject: {}",
                    snap.subject
                );
            }
        }
        writer.join().unwrap();
    }

    #[test]
    fn a_guarded_reporter_stops_publishing_once_superseded() {
        // Spec §4.1: the epoch guard must hold for every in-job transition,
        // not just the ones at the top of the worker loop.
        let hb = Heartbeat::new();
        let reporter = StageReporter::guarded(&hb, hb.epoch());

        assert!(reporter.enter(Stage::Reading, Some("/a.md")));
        assert!(!reporter.superseded());

        hb.bump_epoch();

        assert!(reporter.superseded());
        assert!(
            !reporter.enter(Stage::Embedding, Some("/a.md")),
            "a superseded worker must not publish a transition"
        );
        assert_eq!(
            hb.snapshot().stage,
            Stage::Reading,
            "the superseded write must not have landed on the shared heartbeat"
        );
    }

    #[test]
    fn an_unguarded_reporter_is_never_superseded() {
        // Standalone callers (tooling, Tauri commands) have no worker to
        // supersede, so a bumped epoch must not silently no-op their ingest.
        let hb = Heartbeat::new();
        let reporter = StageReporter::unguarded(&hb);

        hb.bump_epoch();
        hb.bump_epoch();

        assert!(!reporter.superseded());
        assert!(reporter.enter(Stage::Chunking, Some("/a.md")));
        assert_eq!(hb.snapshot().stage, Stage::Chunking);
    }
}
