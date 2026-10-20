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
    }

    /// A batch dropped: how many have been, this one too.
    pub fn dropped(&self) -> u64 {
        self.dropped
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
            timeouts: self.timeouts.load(Ordering::Relaxed),
            waits: self
                .waits
                .iter()
                .map(|w| w.load(Ordering::Relaxed))
                .collect(),
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
}
