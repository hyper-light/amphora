//! Continuous restore drills (docs/design/backup.md, "Continuous restore drills"; T-T1.12.C4 to
//! C6): whether the shipping thread checks its archive ([`CheckPolicy`]), the budget a check runs
//! within ([`admit`]), the seconds it drills ([`Drills`]) and what it measured ([`DrillMeter`]).
//!
//! The pure parts take what they decide from as arguments (the free bytes, the idle time, a random
//! number), so each is tested without a disk, a clock or a seed.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::RowDigest;

/// Whether and where the shipping thread restores and checks its own archive (T-T1.12.C4).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum CheckPolicy {
    /// No checks.
    Off,
    /// Checks into the caller's scratch path whenever one is due, the newest quiet second each
    /// time, with no budget: what lane pitr-8 built, kept as the control.
    Scratch(PathBuf),
    /// The default: checks into a file beside the database (`<database>.check`, on the same disk),
    /// each at a second drawn at random from those the thread holds the source's rows for, and
    /// only within the budget [`admit`] derives from the machine.
    #[default]
    Continuous,
}

/// Why a due check waits ([`admit`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deferred {
    /// The thread hasn't been idle as long as the last check took: the foreground keeps it busy.
    Busy,
    /// The scratch disk can't hold the restore and still leave the source the log it may grow by.
    Disk,
}

impl Deferred {
    /// The reason as `amphora_backup_checks_deferred_total{reason}` names it.
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::Disk => "disk",
        }
    }
}

/// What [`admit`] decides from, all measured where the database runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// Nanoseconds the shipping thread waited for a commit since the last check ended.
    pub idle_ns: u64,
    /// Nanoseconds the last check took (its digest and its restore-and-check); none before the
    /// first.
    pub last_cost_ns: Option<u64>,
    /// Bytes free on the scratch file's disk.
    pub free: u64,
    /// Bytes the restore writes: the source's file and log as they stand.
    pub restore: u64,
    /// Bytes the source's log may grow to before it starts again ([`crate::db::Db`]'s kept frames).
    pub log_headroom: u64,
}

/// Whether a due check may run now (T-T1.12.C5). The thread spends on checks only time it would
/// have spent waiting: a check runs once the thread has been idle, since the last check ended, at
/// least as long as that check took, so checks never take more of the thread than the foreground
/// leaves it, and none while the foreground keeps it busy. The first runs at once (nothing measured
/// yet). The restore fits on the scratch disk with the source's log still able to grow to its most.
pub fn admit(budget: &Budget) -> Result<(), Deferred> {
    let fits = budget
        .restore
        .checked_add(budget.log_headroom)
        .is_some_and(|needed| needed <= budget.free);
    if !fits {
        return Err(Deferred::Disk);
    }
    match budget.last_cost_ns {
        Some(cost) if budget.idle_ns < cost => Err(Deferred::Busy),
        _ => Ok(()),
    }
}

/// The seconds the thread holds the source's rows for, a uniform sample of every quiet snapshot it
/// took (Vitter's Algorithm R, "Random sampling with a reservoir", ACM TOMS 11(1), 1985), as many
/// as the memory a base's chunk may take holds (`Archive::memory`), at least one.
#[derive(Debug, Default)]
pub struct Drills {
    held: Vec<(u64, RowDigest)>,
    seen: u64,
}

/// The bytes a digest takes held: each table's name, count and hash, and the second it is for.
fn digest_bytes(rows: &RowDigest) -> usize {
    let entry = std::mem::size_of::<(String, u64, [u8; 32])>();
    rows.0
        .iter()
        .map(|(name, _, _)| name.len().saturating_add(entry))
        .fold(
            std::mem::size_of::<(u64, RowDigest)>(),
            usize::saturating_add,
        )
}

impl Drills {
    /// Offers one more snapshot: kept while the memory holds it, else in place of a held one with
    /// the chance Algorithm R gives it. `random` is a uniform draw.
    pub fn offer(&mut self, second: u64, rows: RowDigest, memory: usize, random: u64) {
        self.seen = self.seen.saturating_add(1);
        let room = memory.checked_div(digest_bytes(&rows)).unwrap_or(0).max(1);
        if self.held.len() < room {
            self.held.push((second, rows));
            return;
        }
        self.held.truncate(room);
        // Kept with probability room / seen, in a slot drawn uniformly.
        let slot = random.checked_rem(self.seen).unwrap_or(0);
        if let Some(held) = usize::try_from(slot)
            .ok()
            .and_then(|s| self.held.get_mut(s))
        {
            *held = (second, rows);
        }
    }

    /// One held second drawn uniformly by `random`, with its rows.
    #[must_use]
    pub fn pick(&self, random: u64) -> Option<&(u64, RowDigest)> {
        let len = u64::try_from(self.held.len()).ok()?;
        if len == 0 {
            return None;
        }
        usize::try_from(random.checked_rem(len).unwrap_or(0))
            .ok()
            .and_then(|i| self.held.get(i))
    }

    /// Drops a second the archive no longer holds (expired by its owner: [`super::Check::Expired`]).
    pub fn drop_second(&mut self, second: u64) {
        self.held.retain(|(s, _)| *s != second);
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.held.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }
}

/// SplitMix64 (Steele, Lea and Flood, "Fast splittable pseudorandom number generators", OOPSLA
/// 2014): the next draw from `state`, seeded once from the system's entropy.
pub fn next_random(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// What the drills measured: deferrals by reason, drills of a second older than the newest snapshot, the thread's idle time and the checks' cost, each check's cost
/// in power-of-two buckets for its quantiles.
#[derive(Debug)]
pub struct DrillMeter {
    pub deferred_busy: AtomicU64,
    pub deferred_disk: AtomicU64,
    pub older: AtomicU64,
    pub idle_ns: AtomicU64,
    pub cost_ns: AtomicU64,
    /// Checks whose cost in nanoseconds had `i` significant bits, at `i` (0 to 64).
    buckets: [AtomicU64; 65],
}

/// `amphora_backup_checks_deferred_total`: due checks the budget held back, by reason.
pub const DEFER_METRIC: (&str, &str, &str) = (
    "amphora_backup_checks_deferred_total",
    "counter",
    "Due restore-and-checks the budget held back, by reason (busy, disk).",
);

/// `amphora_backup_check_seconds`: what each check took, its digest and its restore-and-check.
pub const COST_METRIC: (&str, &str, &str) = (
    "amphora_backup_check_seconds",
    "histogram",
    "Time a restore-and-check took, the source's digest included.",
);

impl Default for DrillMeter {
    fn default() -> Self {
        Self {
            deferred_busy: AtomicU64::new(0),
            deferred_disk: AtomicU64::new(0),
            older: AtomicU64::new(0),
            idle_ns: AtomicU64::new(0),
            cost_ns: AtomicU64::new(0),
            buckets: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

impl DrillMeter {
    pub fn defer(&self, why: Deferred) {
        let counter = match why {
            Deferred::Busy => &self.deferred_busy,
            Deferred::Disk => &self.deferred_disk,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn idle(&self, ns: u64) {
        self.idle_ns.fetch_add(ns, Ordering::Relaxed);
    }

    pub fn cost(&self, ns: u64) {
        self.cost_ns.fetch_add(ns, Ordering::Relaxed);
        let bits = usize::try_from(u64::BITS.saturating_sub(ns.leading_zeros())).unwrap_or(64);
        if let Some(bucket) = self.buckets.get(bits) {
            bucket.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The checks counted in the cost buckets.
    pub fn checks(&self) -> u64 {
        self.buckets
            .iter()
            .map(|b| b.load(Ordering::Relaxed))
            .fold(0, u64::saturating_add)
    }

    /// The cost at quantile `q` (0 to 1) as its bucket's upper bound in nanoseconds, so within a
    /// factor of two above; none before a check.
    #[must_use]
    pub fn cost_quantile(&self, q: f64) -> Option<u64> {
        let total = self.checks();
        if total == 0 {
            return None;
        }
        // The rank of the quantile among `total` checks, 1-based (nearest rank).
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let rank = ((q.clamp(0.0, 1.0) * total as f64).ceil() as u64).max(1);
        let mut seen = 0u64;
        for (bits, bucket) in self.buckets.iter().enumerate() {
            seen = seen.saturating_add(bucket.load(Ordering::Relaxed));
            if seen >= rank {
                let bits = u32::try_from(bits).unwrap_or(u64::BITS);
                return Some(
                    1u64.checked_shl(bits)
                        .map_or(u64::MAX, |b| b.saturating_sub(1)),
                );
            }
        }
        Some(u64::MAX)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn budget() -> Budget {
        Budget {
            idle_ns: 0,
            last_cost_ns: None,
            free: 1_000,
            restore: 400,
            log_headroom: 100,
        }
    }

    /// T-T1.12.C5: the first check runs at once; later ones once the thread was idle as long as the
    /// last took; none while the disk can't hold the restore and the source's log.
    #[test]
    fn t_t1_12_c5_a_check_runs_within_the_machines_budget() {
        assert_eq!(admit(&budget()), Ok(()));
        let busy = Budget {
            idle_ns: 49,
            last_cost_ns: Some(50),
            ..budget()
        };
        assert_eq!(admit(&busy), Err(Deferred::Busy));
        assert_eq!(
            admit(&Budget {
                idle_ns: 50,
                ..busy
            }),
            Ok(())
        );
        let full = Budget {
            free: 499,
            ..budget()
        };
        assert_eq!(admit(&full), Err(Deferred::Disk));
        assert_eq!(admit(&Budget { free: 500, ..full }), Ok(()));
        let overflow = Budget {
            restore: u64::MAX,
            ..budget()
        };
        assert_eq!(admit(&overflow), Err(Deferred::Disk));
    }

    fn rows(n: u64) -> RowDigest {
        RowDigest(vec![("t".into(), n, [0; 32])])
    }

    /// T-T1.12.C6: the drills hold as many seconds as the memory does, a uniform sample of all
    /// offered; a pick is uniform over those held; a dropped second is never picked again.
    #[test]
    fn t_t1_12_c6_drills_sample_every_second_uniformly() {
        let one = digest_bytes(&rows(0));
        let mut drills = Drills::default();
        let mut state = 7;
        let mut counts = vec![0u64; 100];
        // 4 slots out of 100 seconds offered, 20,000 times over: each second kept about 4% of the
        // time.
        for _ in 0..20_000 {
            drills = Drills::default();
            for s in 0..100 {
                drills.offer(s, rows(s), one * 4, next_random(&mut state));
            }
            assert_eq!(drills.len(), 4);
            for (s, _) in &drills.held {
                counts[usize::try_from(*s).unwrap()] += 1;
            }
        }
        // Each second's count is Binomial(20,000, 0.04): mean 800, sd 27.7; 6 sd either side.
        for (s, n) in counts.iter().enumerate() {
            assert!((633..=967).contains(n), "second {s} kept {n} times");
        }
        let mut picked = [0u64; 4];
        for _ in 0..4_000 {
            let (s, _) = drills.pick(next_random(&mut state)).unwrap();
            let i = drills.held.iter().position(|(h, _)| h == s).unwrap();
            picked[i] += 1;
        }
        assert!(picked.iter().all(|&n| n > 0), "{picked:?}");
        let (gone, _) = drills.held[0].clone();
        drills.drop_second(gone);
        assert_eq!(drills.len(), 3);
        for _ in 0..100 {
            assert_ne!(drills.pick(next_random(&mut state)).unwrap().0, gone);
        }
        let mut tiny = Drills::default();
        tiny.offer(1, rows(1), 0, 0);
        assert_eq!(tiny.len(), 1, "at least one is held");
        assert!(Drills::default().pick(0).is_none());
    }

    /// T-T1.12.C5: the cost quantiles come from power-of-two buckets, each an upper bound within a
    /// factor of two.
    #[test]
    fn t_t1_12_c5_cost_quantiles() {
        let meter = DrillMeter::default();
        assert_eq!(meter.cost_quantile(0.5), None);
        for ns in [1, 3, 3, 100, 1_000_000] {
            meter.cost(ns);
        }
        assert_eq!(meter.checks(), 5);
        assert_eq!(meter.cost_quantile(0.5), Some(3));
        assert_eq!(meter.cost_quantile(0.8), Some(127));
        assert_eq!(meter.cost_quantile(0.999), Some((1 << 20) - 1));
        assert_eq!(meter.cost_ns.load(Ordering::Relaxed), 1_000_107);
    }
}
