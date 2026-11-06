//! What a store is doing, counted as it goes ([`Meter`]): cheap to keep, read at any time without
//! asking its writer ([`Counters`]).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// How long reads waited for their answer, counted by power of two of milliseconds: under 1 ms,
/// under 2, under 4, ... and the last bucket for the rest (past 16 s).
pub const WAIT_BUCKETS: usize = 16;
const LAST_BUCKET: u32 = 15;

/// The bucket a wait of `d` counts in.
#[must_use]
pub fn wait_bucket(d: Duration) -> usize {
    let ms = u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
    let bucket = u64::BITS
        .saturating_sub(ms.leading_zeros())
        .min(LAST_BUCKET);
    usize::try_from(bucket).unwrap_or(WAIT_BUCKETS.saturating_sub(1))
}

/// About the `q`th quantile (0 to 1) of the waits counted in `buckets`, in ms: the top of the
/// bucket it falls in (None: none counted).
#[must_use]
pub fn wait_quantile(buckets: &[u64], q: f64) -> Option<u64> {
    let total = buckets.iter().fold(0u64, |a, n| a.saturating_add(*n));
    if total == 0 {
        return None;
    }
    let want = (total as f64 * q).max(1.0);
    let top = |i: usize| {
        1u64.checked_shl(u32::try_from(i).unwrap_or(LAST_BUCKET))
            .unwrap_or(u64::MAX)
    };
    let mut seen = 0u64;
    for (i, n) in buckets.iter().enumerate() {
        seen = seen.saturating_add(*n);
        if seen as f64 >= want {
            return Some(top(i));
        }
    }
    Some(top(buckets.len().saturating_sub(1)))
}

/// What the store keeps the longest of ([`Counters::longest_us`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Longest {
    Commit,
    Flush,
    Checkpoint,
    RestartWait,
    Restart,
}

const LONGEST: usize = 5;

impl Longest {
    /// Their order in [`Counters::longest_us`].
    pub const ALL: [Self; LONGEST] = [
        Self::Commit,
        Self::Flush,
        Self::Checkpoint,
        Self::RestartWait,
        Self::Restart,
    ];

    fn index(self) -> usize {
        match self {
            Self::Commit => 0,
            Self::Flush => 1,
            Self::Checkpoint => 2,
            Self::RestartWait => 3,
            Self::Restart => 4,
        }
    }
}

/// A store's counters, bumped by its writer, its flusher and its readers as they go.
#[derive(Default)]
pub struct Meter {
    commits: AtomicU64,
    rows: AtomicU64,
    /// Jobs run on the writer.
    jobs: AtomicU64,
    /// How long the writer was held (its commits, its jobs, its upkeep), and of that its commits,
    /// in µs.
    busy_us: AtomicU64,
    apply_us: AtomicU64,
    /// Flushes of what was committed, and how long they took, in µs.
    flushes: AtomicU64,
    flush_us: AtomicU64,
    /// Batches refused because the writer's queue was full.
    dropped: AtomicU64,
    /// Batches whose commit failed, each refused to its sender.
    failed: AtomicU64,
    /// Passive checkpoints, and how long they took, in µs.
    checkpoints: AtomicU64,
    checkpoint_us: AtomicU64,
    /// The log started again by the writer, how long it waited for the readers and the
    /// checkpointer to let go, and how long the restart took once they had, in µs.
    restarts: AtomicU64,
    restart_wait_us: AtomicU64,
    restart_us: AtomicU64,
    /// The longest of each, in µs: a stall shows here first.
    longest: [AtomicU64; LONGEST],
    /// Reads that got no answer in time.
    timeouts: AtomicU64,
    waits: [AtomicU64; WAIT_BUCKETS],
}

/// What a store's counters say now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counters {
    pub commits: u64,
    pub rows: u64,
    pub jobs: u64,
    pub busy_ms: u64,
    pub apply_ms: u64,
    pub flushes: u64,
    pub flush_ms: u64,
    pub dropped: u64,
    pub failed: u64,
    pub checkpoints: u64,
    pub checkpoint_ms: u64,
    pub restarts: u64,
    pub restart_wait_ms: u64,
    pub restart_ms: u64,
    /// The longest commit, flush, checkpoint, wait to restart and restart, in µs, in the order of
    /// [`Longest::ALL`].
    pub longest_us: Vec<u64>,
    pub timeouts: u64,
    /// Reads' waits ([`WAIT_BUCKETS`]).
    pub waits: Vec<u64>,
}

fn micros(d: Duration) -> u64 {
    u64::try_from(d.as_micros()).unwrap_or(u64::MAX)
}

impl Meter {
    pub fn committed(&self, rows: usize, took: Duration) {
        self.commits.fetch_add(1, Ordering::Relaxed);
        self.rows
            .fetch_add(u64::try_from(rows).unwrap_or(u64::MAX), Ordering::Relaxed);
        self.apply_us.fetch_add(micros(took), Ordering::Relaxed);
        self.longest(Longest::Commit, took);
    }

    fn longest(&self, of: Longest, took: Duration) {
        if let Some(l) = self.longest.get(of.index()) {
            l.fetch_max(micros(took), Ordering::Relaxed);
        }
    }

    pub fn checkpointed(&self, took: Duration) {
        self.checkpoints.fetch_add(1, Ordering::Relaxed);
        self.checkpoint_us
            .fetch_add(micros(took), Ordering::Relaxed);
        self.longest(Longest::Checkpoint, took);
    }

    /// The log started again: `waited` for the readers and the checkpointer, then `took`.
    pub fn restarted(&self, waited: Duration, took: Duration) {
        self.restarts.fetch_add(1, Ordering::Relaxed);
        self.restart_wait_us
            .fetch_add(micros(waited), Ordering::Relaxed);
        self.restart_us.fetch_add(micros(took), Ordering::Relaxed);
        self.longest(Longest::RestartWait, waited);
        self.longest(Longest::Restart, took);
    }

    pub fn ran(&self) {
        self.jobs.fetch_add(1, Ordering::Relaxed);
    }

    pub fn held(&self, d: Duration) {
        self.busy_us.fetch_add(micros(d), Ordering::Relaxed);
    }

    pub fn flushed(&self, d: Duration) {
        self.flushes.fetch_add(1, Ordering::Relaxed);
        self.flush_us.fetch_add(micros(d), Ordering::Relaxed);
        self.longest(Longest::Flush, d);
    }

    /// A batch dropped: how many have been, this one too.
    pub fn dropped(&self) -> u64 {
        self.dropped
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1)
    }

    /// A batch whose commit failed: how many have, this one too.
    pub fn failed(&self) -> u64 {
        self.failed
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1)
    }

    pub fn waited(&self, d: Duration) {
        if let Some(w) = self.waits.get(wait_bucket(d)) {
            w.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn timed_out(&self) {
        self.timeouts.fetch_add(1, Ordering::Relaxed);
    }

    #[must_use]
    pub fn counters(&self) -> Counters {
        Counters {
            commits: self.commits.load(Ordering::Relaxed),
            rows: self.rows.load(Ordering::Relaxed),
            jobs: self.jobs.load(Ordering::Relaxed),
            busy_ms: self.busy_us.load(Ordering::Relaxed) / 1000,
            apply_ms: self.apply_us.load(Ordering::Relaxed) / 1000,
            flushes: self.flushes.load(Ordering::Relaxed),
            flush_ms: self.flush_us.load(Ordering::Relaxed) / 1000,
            dropped: self.dropped.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            checkpoints: self.checkpoints.load(Ordering::Relaxed),
            checkpoint_ms: self.checkpoint_us.load(Ordering::Relaxed) / 1000,
            restarts: self.restarts.load(Ordering::Relaxed),
            restart_wait_ms: self.restart_wait_us.load(Ordering::Relaxed) / 1000,
            restart_ms: self.restart_us.load(Ordering::Relaxed) / 1000,
            longest_us: self
                .longest
                .iter()
                .map(|l| l.load(Ordering::Relaxed))
                .collect(),
            timeouts: self.timeouts.load(Ordering::Relaxed),
            waits: self
                .waits
                .iter()
                .map(|w| w.load(Ordering::Relaxed))
                .collect(),
        }
    }
}

/// How long the replicated class's flushes took, by power of two: bucket `i` holds those of at
/// most 62.5 µs · 2^i, from 62.5 µs to 16.384 s, and the last the rest. The measured flushes run
/// from 3.9 ms (p50, a calm Mac) to 5.6 s (max, loaded btrfs; replication.md, "Fast
/// acknowledgement"); the bounds below a millisecond are for a quiet fsync on NVMe, kiln's
/// production disk, as the Kiln Metrics agent asked (replication.md, "Metrics").
pub const FLUSH_BUCKETS: usize = 20;
const LAST_FLUSH: u32 = 19;
const FIRST_FLUSH_BOUND_NS: u64 = 62_500;

/// The bucket a flush of `d` counts in.
#[must_use]
pub fn flush_bucket(d: Duration) -> usize {
    let ns = u64::try_from(d.as_nanos()).unwrap_or(u64::MAX);
    let bucket = (0..LAST_FLUSH)
        .find(|i| {
            FIRST_FLUSH_BOUND_NS
                .checked_shl(*i)
                .is_some_and(|bound| ns <= bound)
        })
        .unwrap_or(LAST_FLUSH);
    usize::try_from(bucket).unwrap_or(FLUSH_BUCKETS.saturating_sub(1))
}

/// The replicated class's metrics, as their host exports them: name, type and help, in the
/// order of [`ReplicaCounters`]' fields (docs/design/replication.md, "Metrics").
pub const REPLICA_METRICS: [(&str, &str, &str); 3] = [
    (
        "amphora_replica_flush_backlog_entries",
        "gauge",
        "Entries this member applied that its database's log hasn't flushed yet.",
    ),
    (
        "amphora_replica_flush_seconds",
        "histogram",
        "Each flush of the database's log the replicated class's flushing thread ran.",
    ),
    (
        "amphora_replica_commits_total",
        "counter",
        "Entries this member committed as leader, by the quorum that committed each first (quorum: held, classic); leader only, so a follower's stays at 0.",
    ),
];

/// A replica owner's counters for the replicated class, bumped by its owner and its flushing
/// thread, read by its host at any time without asking the owner.
#[derive(Default)]
pub struct ReplicaMeter {
    backlog: AtomicU64,
    flushes: [AtomicU64; FLUSH_BUCKETS],
    flush_us: AtomicU64,
    held: AtomicU64,
    classic: AtomicU64,
}

/// What a replica owner's counters say now ([`REPLICA_METRICS`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplicaCounters {
    /// Entries applied and not yet flushed: 0 outside the replicated class, which flushes each
    /// entry as it applies it.
    pub flush_backlog: u64,
    /// The flushing thread's flushes, by [`flush_bucket`], and how long they took in all.
    pub flushes: [u64; FLUSH_BUCKETS],
    pub flush_time: Duration,
    /// Entries committed as leader by a held quorum before a classic one held them, and by a
    /// classic quorum.
    pub commits_held: u64,
    pub commits_classic: u64,
}

impl ReplicaMeter {
    pub fn flushed(&self, took: Duration) {
        if let Some(b) = self.flushes.get(flush_bucket(took)) {
            b.fetch_add(1, Ordering::Relaxed);
        }
        self.flush_us.fetch_add(micros(took), Ordering::Relaxed);
    }

    /// The owner's state after a step: its backlog, and its core's commits by quorum.
    pub fn stepped(&self, backlog: u64, held: u64, classic: u64) {
        self.backlog.store(backlog, Ordering::Relaxed);
        self.held.store(held, Ordering::Relaxed);
        self.classic.store(classic, Ordering::Relaxed);
    }

    #[must_use]
    pub fn counters(&self) -> ReplicaCounters {
        ReplicaCounters {
            flush_backlog: self.backlog.load(Ordering::Relaxed),
            flushes: std::array::from_fn(|i| {
                self.flushes.get(i).map_or(0, |b| b.load(Ordering::Relaxed))
            }),
            flush_time: Duration::from_micros(self.flush_us.load(Ordering::Relaxed)),
            commits_held: self.held.load(Ordering::Relaxed),
            commits_classic: self.classic.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Waits count by power of two of milliseconds, and their quantiles read back as the top of the
    /// bucket they fall in (CP-T1.1: the store's waits, which its dashboard charts).
    #[test]
    fn waits_count_by_power_of_two_and_quantiles_read_back() {
        assert_eq!(wait_bucket(Duration::from_micros(300)), 0);
        assert_eq!(wait_bucket(Duration::from_millis(3)), 2);
        assert_eq!(wait_bucket(Duration::from_secs(600)), WAIT_BUCKETS - 1);
        assert_eq!(wait_quantile(&[0; WAIT_BUCKETS], 0.5), None);
        let mut b = [0u64; WAIT_BUCKETS];
        b[0] = 98;
        b[4] = 2;
        assert_eq!(wait_quantile(&b, 0.5), Some(1));
        assert_eq!(wait_quantile(&b, 0.99), Some(16));
    }

    /// Flushes count by power of two from 62.5 µs, each in the first bucket whose bound holds it
    /// (CP-T1.8: the replicated class's flush seconds, which its dashboard charts).
    #[test]
    fn flushes_count_in_the_first_bucket_whose_bound_holds_them() {
        assert_eq!(flush_bucket(Duration::from_micros(50)), 0);
        assert_eq!(flush_bucket(Duration::from_nanos(62_500)), 0);
        assert_eq!(flush_bucket(Duration::from_micros(400)), 3);
        assert_eq!(flush_bucket(Duration::from_millis(1)), 4);
        assert_eq!(flush_bucket(Duration::from_micros(1_001)), 5);
        assert_eq!(flush_bucket(Duration::from_micros(3_928)), 6);
        assert_eq!(flush_bucket(Duration::from_millis(16_384)), 18);
        assert_eq!(flush_bucket(Duration::from_secs(600)), FLUSH_BUCKETS - 1);
    }

    /// CP-T1.8, doc truth (CLAUDE.md sections 6 and 10): the replicated class's metrics are the
    /// ones its design names. Do: read docs/design/replication.md. Expect: every
    /// `amphora_replica_` name there (a histogram's `_bucket` series as the histogram) is in
    /// [`REPLICA_METRICS`], and every name there is in the design.
    #[test]
    fn the_replica_metrics_are_the_ones_the_design_names() {
        let design = include_str!("../../../docs/design/replication.md");
        let named: std::collections::BTreeSet<&str> = design
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .filter(|word| word.starts_with("amphora_replica_"))
            .map(|word| word.strip_suffix("_bucket").unwrap_or(word))
            .collect();
        let code = REPLICA_METRICS.iter().map(|(name, _, _)| *name).collect();
        assert_eq!(named, code);
    }
}
