use std::sync::{
    atomic::{AtomicI64, AtomicU64, AtomicU8, Ordering},
    Mutex,
};
#[allow(unused_imports)]
use std::sync::MutexGuard;

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
}

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
    pub fn enter(&self, stage: Stage, subject: Option<&str>) {
        if let Ok(mut g) = self.subject.lock() {
            *g = subject.map(|s| s.to_string());
        }
        self.stage_started_ms.store(now_ms(), Ordering::SeqCst);
        self.stage.store(stage as u8, Ordering::SeqCst);
        self.seq.fetch_add(1, Ordering::SeqCst);
    }

    /// Read current state without ever blocking on `subject`.
    pub fn snapshot(&self) -> HeartbeatSnapshot {
        let subject = match self.subject.try_lock() {
            Ok(g) => g.clone().unwrap_or_else(|| "unknown".to_string()),
            Err(_) => "unknown".to_string(),
        };
        HeartbeatSnapshot {
            epoch: self.epoch.load(Ordering::SeqCst),
            seq: self.seq.load(Ordering::SeqCst),
            stage: Stage::from_u8(self.stage.load(Ordering::SeqCst)),
            subject,
            stage_started_ms: self.stage_started_ms.load(Ordering::SeqCst),
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
}
