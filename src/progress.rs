//! Lightweight, thread-safe task progress for batch evaluation.
//!
//! Progress is observational only: it never changes scoring, never writes to
//! stdout, and reduces to atomically counted totals under concurrency. Events
//! go through `tracing` so operators can watch a run on stderr without
//! corrupting the machine-readable report.

use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

/// An immutable view of progress at a point in time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProgressCounts {
    pub total: usize,
    pub started: usize,
    pub completed: usize,
    pub failed: usize,
}

impl ProgressCounts {
    /// Cases that have finished but whose run failed.
    pub fn remaining(&self) -> usize {
        self.total.saturating_sub(self.completed)
    }

    pub fn succeeded(&self) -> usize {
        self.completed.saturating_sub(self.failed)
    }
}

/// Shared, lock-free counters for one batch run.
#[derive(Debug)]
pub struct Progress {
    total: usize,
    started: AtomicUsize,
    completed: AtomicUsize,
    failed: AtomicUsize,
    started_at: Instant,
}

impl Progress {
    /// Create a tracker for `total` cases and share it across workers.
    pub fn new(total: usize) -> Arc<Self> {
        Arc::new(Self {
            total,
            started: AtomicUsize::new(0),
            completed: AtomicUsize::new(0),
            failed: AtomicUsize::new(0),
            started_at: Instant::now(),
        })
    }

    /// Record that a case has begun.
    pub fn begin(&self, case_id: &str) {
        let started = self.started.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::info!(case_id, started, total = self.total, "case started");
    }

    /// Record that a case finished, marking it failed when appropriate.
    pub fn finish(&self, case_id: &str, outcome: &str, failed: bool, elapsed_ms: u64) {
        let completed = self.completed.fetch_add(1, Ordering::Relaxed) + 1;
        if failed {
            self.failed.fetch_add(1, Ordering::Relaxed);
        }
        tracing::info!(
            case_id,
            outcome,
            elapsed_ms,
            completed,
            total = self.total,
            "case finished"
        );
    }

    /// Snapshot the counters.
    pub fn counts(&self) -> ProgressCounts {
        ProgressCounts {
            total: self.total,
            started: self.started.load(Ordering::Relaxed),
            completed: self.completed.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
        }
    }

    /// Wall-clock time since the tracker was created.
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Emit a final aggregate progress line.
    pub fn summary(&self) {
        let counts = self.counts();
        tracing::info!(
            total = counts.total,
            completed = counts.completed,
            failed = counts.failed,
            succeeded = counts.succeeded(),
            elapsed_ms = self.elapsed().as_millis() as u64,
            "evaluation progress complete"
        );
    }
}
