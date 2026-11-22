//! Continuous restore drills (docs/design/backup.md, "Continuous restore drills"; T-T1.12.C4 to
//! C7): whether the shipping thread checks its archive ([`CheckPolicy`]), whether the disk holds a
//! check ([`admit`]), how a check yields to the foreground step by step ([`Pacer`]), the seconds it
//! drills ([`Drills`]) and what it measured ([`DrillMeter`]).
//!
//! The pure parts take what they decide from as arguments (the free bytes, the quiet time, a step's
//! cost, a random number), so each is tested without a disk, a clock or a seed.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::RowDigest;

/// Whether and where the database's backup restores and checks its own archive (T-T1.12.C4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckPolicy {
    /// No checks.
    Off,
    /// The default (Ada's earlier word, kept by the measurements in docs/design/backup.md,
    /// "Paced checks"): checks whenever one is due, the newest quiet second each time, unpaced,
    /// into the caller's scratch path, else a file beside the database (`<database>.check`, on the
    /// same disk): what lane pitr-8 built, the control.
    Scratch(Option<PathBuf>),
    /// Checks into a file beside the database, each at a second drawn at random from those the
    /// thread holds the source's rows for, paced step by step so they yield to the foreground
    /// ([`Pacer`], T-T1.12.C7).
    Continuous,
}

impl Default for CheckPolicy {
    fn default() -> Self {
        Self::Scratch(None)
    }
}

/// Why a due check waits ([`admit`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deferred {
    /// The scratch disk can't hold the restore and still leave the source the log it may grow by.
    Disk,
}

impl Deferred {
    /// The reason as `amphora_backup_checks_deferred_total{reason}` names it.
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            Self::Disk => "disk",
        }
    }
}

/// What [`admit`] decides from, all measured where the database runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// Bytes free on the scratch file's disk.
    pub free: u64,
    /// Bytes the restore writes: the source's file and log as they stand.
    pub restore: u64,
    /// Bytes the source's log may grow to before it starts again ([`crate::db::Db`]'s kept frames).
    pub log_headroom: u64,
}

/// Whether a due check may begin now (T-T1.12.C5): the restore fits on the scratch disk with the
/// source's log still able to grow to its most. How it shares the machine with the foreground once
/// begun is the [`Pacer`]'s, step by step.
pub fn admit(budget: &Budget) -> Result<(), Deferred> {
    let fits = budget
        .restore
        .checked_add(budget.log_headroom)
        .is_some_and(|needed| needed <= budget.free);
    if fits { Ok(()) } else { Err(Deferred::Disk) }
}

/// What the foreground does, as a check's [`Pacer`] reads it (T-T1.12.C7): when the store's last
/// commit was, and whether the database ended. The store's commit watcher writes it.
#[derive(Debug)]
pub struct Foreground {
    base: Instant,
    /// Nanoseconds from `base` to the last commit.
    last_ns: AtomicU64,
    ended: Mutex<bool>,
    woken: Condvar,
}

impl Default for Foreground {
    fn default() -> Self {
        Self {
            base: Instant::now(),
            last_ns: AtomicU64::new(0),
            ended: Mutex::new(false),
            woken: Condvar::new(),
        }
    }
}

/// A check stopped because its database ended: not a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stopped;

impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the database ended during a check")
    }
}

impl std::error::Error for Stopped {}

impl Foreground {
    fn now_ns(&self) -> u64 {
        u64::try_from(self.base.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }

    /// The store made a commit.
    pub fn committed(&self) {
        self.last_ns.store(self.now_ns(), Ordering::Relaxed);
    }

    /// Nanoseconds since the last commit (since this began, before the first).
    fn quiet_ns(&self) -> u64 {
        self.now_ns()
            .saturating_sub(self.last_ns.load(Ordering::Relaxed))
    }

    /// The database ended: a check under way stops at its next step.
    pub fn end(&self) {
        if let Ok(mut ended) = self.ended.lock() {
            *ended = true;
        }
        self.woken.notify_all();
    }

    fn has_ended(&self) -> bool {
        self.ended.lock().map_or(true, |e| *e)
    }

    /// Waits `ns`, less if the database ends first: whether it ended.
    fn sleep(&self, ns: u64) -> bool {
        let Ok(mut ended) = self.ended.lock() else {
            return true;
        };
        let deadline = Instant::now().checked_add(Duration::from_nanos(ns));
        loop {
            if *ended {
                return true;
            }
            let left = match deadline {
                Some(d) => d.saturating_duration_since(Instant::now()),
                None => Duration::MAX,
            };
            if left.is_zero() {
                return false;
            }
            match self.woken.wait_timeout(ended, left) {
                Ok((guard, _)) => ended = guard,
                Err(_) => return true,
            }
        }
    }
}

/// How long a check's next step waits for the foreground to go quiet (T-T1.12.C7): until no commit
/// was made for as long as the check's last step took, so a step begins only in a quiet stretch at
/// least its own length, and a commit arriving pauses the steps after it. None before a step was
/// measured.
#[must_use]
pub fn quiet_wait(quiet_ns: u64, last_step_ns: Option<u64>) -> u64 {
    last_step_ns.map_or(0, |step| step.saturating_sub(quiet_ns))
}

/// Whether a step of `took_ns` over `bytes` went faster per byte than `best` (its nanoseconds and
/// bytes).
fn faster(took_ns: u64, bytes: u64, best: (u64, u64)) -> bool {
    u128::from(took_ns).saturating_mul(u128::from(best.1))
        < u128::from(best.0).saturating_mul(u128::from(bytes))
}

/// How long a check rests after a step so its share of the device is no more than the room the
/// device has (T-T1.12.C7). The step measures the room itself, as LEDBAT (RFC 6817, "Low Extra
/// Delay Background Transport") and TCP Vegas measure a path's queue by their own delay against the
/// least they saw: the fastest a step of its kind went per byte on this machine, over how fast this
/// one went, is the share of the device the check had (1 with the device to itself); resting
/// `took × (1 − room) / room` holds the check to that share, leaving the rest to the foreground.
#[must_use]
pub fn rest_ns(took_ns: u64, bytes: u64, best: (u64, u64)) -> u64 {
    let (best_ns, best_bytes) = best;
    let observed = u128::from(took_ns).saturating_mul(u128::from(best_bytes));
    let fastest = u128::from(bytes).saturating_mul(u128::from(best_ns));
    let excess = observed.saturating_sub(fastest);
    u128::from(took_ns)
        .saturating_mul(excess)
        .checked_div(fastest)
        .map_or(0, |ns| u64::try_from(ns).unwrap_or(u64::MAX))
}

/// What a restore or a digest does between its steps (T-T1.12.C7): a base's chunk, a log segment, a
/// replay, a chunk's worth of rows read.
pub trait Pace {
    /// Before a step: waits while the foreground keeps the machine busy. Refused ([`Stopped`]) once
    /// the database ended: the check stops there.
    fn ready(&mut self) -> anyhow::Result<()>;
    /// After a step that moved `bytes`, written to `file` if it wrote. Paced, the file is made
    /// durable here, so the step's cost is the device's and its writes never pile up past a step.
    fn done(&mut self, file: Option<&std::fs::File>, bytes: u64) -> anyhow::Result<()>;
}

/// No pacing: a restore the caller asked for, as fast as it goes, and the control's checks.
#[derive(Debug, Clone, Copy, Default)]
pub struct Unpaced;

impl Pace for Unpaced {
    fn ready(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn done(&mut self, _: Option<&std::fs::File>, _: u64) -> anyhow::Result<()> {
        Ok(())
    }
}

/// A continuous check's pace (T-T1.12.C7): each step begins only once the foreground has been quiet
/// as long as the last step took ([`quiet_wait`]), and rests after it by the device's measured room
/// ([`rest_ns`]), so a check takes only what the foreground leaves and pauses when it comes back.
#[derive(Debug)]
pub struct Pacer {
    foreground: Arc<Foreground>,
    meter: Arc<DrillMeter>,
    began: Option<Instant>,
    last_step_ns: Option<u64>,
    /// The fastest step per byte seen, writing and reading: the device's room when the check has it
    /// to itself.
    best_write: Option<(u64, u64)>,
    best_read: Option<(u64, u64)>,
}

impl Pacer {
    #[must_use]
    pub fn new(foreground: Arc<Foreground>, meter: Arc<DrillMeter>) -> Self {
        Self {
            foreground,
            meter,
            began: None,
            last_step_ns: None,
            best_write: None,
            best_read: None,
        }
    }
}

impl Pace for Pacer {
    fn ready(&mut self) -> anyhow::Result<()> {
        let mut paused = false;
        loop {
            if self.foreground.has_ended() {
                return Err(Stopped.into());
            }
            let wait = quiet_wait(self.foreground.quiet_ns(), self.last_step_ns);
            if wait == 0 {
                break;
            }
            if !paused {
                paused = true;
                self.meter.paused.fetch_add(1, Ordering::Relaxed);
            }
            self.meter.waited_ns.fetch_add(wait, Ordering::Relaxed);
            if self.foreground.sleep(wait) {
                return Err(Stopped.into());
            }
        }
        self.began = Some(Instant::now());
        Ok(())
    }

    fn done(&mut self, file: Option<&std::fs::File>, bytes: u64) -> anyhow::Result<()> {
        if let Some(file) = file {
            file.sync_data()?;
        }
        let took = self.began.take().map_or(0, |b| {
            u64::try_from(b.elapsed().as_nanos()).unwrap_or(u64::MAX)
        });
        self.last_step_ns = Some(took);
        if bytes == 0 {
            return Ok(());
        }
        let best = if file.is_some() {
            &mut self.best_write
        } else {
            &mut self.best_read
        };
        let fastest = match *best {
            Some(b) if !faster(took, bytes, b) => b,
            _ => {
                *best = Some((took, bytes));
                (took, bytes)
            }
        };
        let rest = rest_ns(took, bytes, fastest);
        if rest > 0 {
            self.meter.rested_ns.fetch_add(rest, Ordering::Relaxed);
            if self.foreground.sleep(rest) {
                return Err(Stopped.into());
            }
        }
        Ok(())
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

/// What the drills measured: deferrals by reason, drills of a second older than the newest
/// snapshot, the pace's pauses and waits for a quiet foreground and its rests for the device's
/// room, the checks' cost, each check's cost in power-of-two buckets for its quantiles.
#[derive(Debug)]
pub struct DrillMeter {
    pub deferred_disk: AtomicU64,
    pub older: AtomicU64,
    /// Steps that waited for the foreground to go quiet, and the nanoseconds waited.
    pub paused: AtomicU64,
    pub waited_ns: AtomicU64,
    /// Nanoseconds rested after steps to keep within the device's room.
    pub rested_ns: AtomicU64,
    pub cost_ns: AtomicU64,
    /// Checks whose cost in nanoseconds had `i` significant bits, at `i` (0 to 64).
    buckets: [AtomicU64; 65],
}

/// `amphora_backup_checks_deferred_total`: due checks held back, by reason.
pub const DEFER_METRIC: (&str, &str, &str) = (
    "amphora_backup_checks_deferred_total",
    "counter",
    "Due restore-and-checks held back, by reason (disk).",
);

/// `amphora_backup_check_seconds`: what each check took, its digest and its restore-and-check,
/// its pauses and rests included.
pub const COST_METRIC: (&str, &str, &str) = (
    "amphora_backup_check_seconds",
    "histogram",
    "Time a restore-and-check took, the source's digest and the pace's waits included.",
);

impl Default for DrillMeter {
    fn default() -> Self {
        Self {
            deferred_disk: AtomicU64::new(0),
            older: AtomicU64::new(0),
            paused: AtomicU64::new(0),
            waited_ns: AtomicU64::new(0),
            rested_ns: AtomicU64::new(0),
            cost_ns: AtomicU64::new(0),
            buckets: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

impl DrillMeter {
    pub fn defer(&self, why: Deferred) {
        let counter = match why {
            Deferred::Disk => &self.deferred_disk,
        };
        counter.fetch_add(1, Ordering::Relaxed);
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
            free: 1_000,
            restore: 400,
            log_headroom: 100,
        }
    }

    /// T-T1.12.C5: a check begins only while the disk holds the restore and the source's log.
    #[test]
    fn t_t1_12_c5_a_check_begins_only_while_the_disk_holds_it() {
        assert_eq!(admit(&budget()), Ok(()));
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

    /// T-T1.12.C7: a step begins only once the foreground was quiet as long as the last step took;
    /// the first at once.
    #[test]
    fn t_t1_12_c7_a_step_waits_for_a_quiet_stretch_its_own_length() {
        assert_eq!(quiet_wait(0, None), 0);
        assert_eq!(quiet_wait(10, Some(50)), 40);
        assert_eq!(quiet_wait(50, Some(50)), 0);
        assert_eq!(quiet_wait(u64::MAX, Some(50)), 0);
    }

    /// T-T1.12.C7: a step as fast per byte as the fastest rests none; one twice as slow (half the
    /// device's room) rests as long as it took, holding the check to half; ten times as slow, nine
    /// times as long. Nothing measured rests none.
    #[test]
    fn t_t1_12_c7_rests_hold_a_check_to_the_devices_room() {
        assert_eq!(rest_ns(100, 1_000, (100, 1_000)), 0);
        assert_eq!(rest_ns(50, 1_000, (100, 1_000)), 0);
        assert_eq!(rest_ns(200, 1_000, (100, 1_000)), 200);
        assert_eq!(rest_ns(100, 500, (100, 1_000)), 100);
        assert_eq!(rest_ns(1_000, 1_000, (100, 1_000)), 9_000);
        assert_eq!(rest_ns(100, 0, (100, 1_000)), 0);
        assert_eq!(rest_ns(100, 1_000, (0, 0)), 0);
        assert_eq!(rest_ns(u64::MAX, 1, (1, u64::MAX)), u64::MAX);
        assert!(faster(1, 10, (2, 10)));
        assert!(!faster(2, 10, (2, 10)));
    }

    /// T-T1.12.C7: a commit just made pauses the next step (counted); once the database ends, a
    /// step is refused with [`Stopped`], not failed.
    #[test]
    fn t_t1_12_c7_the_pacer_pauses_for_a_commit_and_stops_at_the_end() {
        let foreground = Arc::new(Foreground::default());
        let meter = Arc::new(DrillMeter::default());
        let mut pacer = Pacer::new(Arc::clone(&foreground), Arc::clone(&meter));
        pacer.ready().unwrap();
        pacer.done(None, 0).unwrap();
        assert_eq!(
            meter.paused.load(Ordering::Relaxed),
            0,
            "the first step waits for nothing"
        );
        // The last step's cost: a millisecond.
        pacer.last_step_ns = Some(1_000_000);
        foreground.committed();
        pacer.ready().unwrap();
        assert_eq!(meter.paused.load(Ordering::Relaxed), 1);
        assert!(meter.waited_ns.load(Ordering::Relaxed) > 0);
        pacer.done(None, 4096).unwrap();
        assert_eq!(pacer.best_read.map(|b| b.1), Some(4096));
        foreground.end();
        let refused = pacer.ready().unwrap_err();
        assert_eq!(refused.downcast_ref::<Stopped>(), Some(&Stopped));
        assert!(Unpaced.ready().is_ok());
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
