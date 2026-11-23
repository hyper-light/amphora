//! Continuous restore drills (docs/design/backup.md, "Continuous restore drills"; T-T1.12.C4 to
//! C7): whether the shipping thread checks its archive ([`CheckPolicy`]), whether the disk holds a
//! check ([`admit`]), how a check yields to the foreground step by step ([`Pacer`], [`Yield`]), the
//! seconds it drills ([`Drills`]) and what it measured ([`DrillMeter`]).
//!
//! The pure parts take what they decide from as arguments (the free bytes, a step's cost, the time
//! a check has run, a random number), so each is tested without a disk, a clock or a seed.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    /// fully ([`Yield::Fully`], T-T1.12.C7).
    Continuous,
    /// [`CheckPolicy::Continuous`], each check given a minimum share of the machine so the
    /// foreground can't starve it ([`Yield::Share`], T-T1.12.C8): the archive's retention period,
    /// the one its owner expires it by ([`super::expire`]), a check's second lost past it.
    Shared(Duration),
}

impl CheckPolicy {
    /// How a check under this policy yields to the foreground: none when unpaced or off.
    #[must_use]
    pub fn pace(&self) -> Option<Yield> {
        match self {
            Self::Off | Self::Scratch(_) => None,
            Self::Continuous => Some(Yield::Fully),
            Self::Shared(retention) => Some(Yield::Share {
                retention_ns: u64::try_from(retention.as_nanos()).unwrap_or(u64::MAX),
            }),
        }
    }
}

/// How a paced check shares the machine with the foreground (T-T1.12.C8): the two candidates the
/// A/B runs, behind [`CheckPolicy`], the control ([`CheckPolicy::Scratch`]) unpaced beside them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Yield {
    /// A step begins only while the database runs no call: the call that ends last wakes it. A
    /// foreground that always has a call in flight starves the check.
    Fully,
    /// As [`Yield::Fully`], but a step also begins with calls in flight while the check is behind
    /// its minimum share ([`behind`]): its work over the retention period, so a check finishes
    /// within the period its second is kept for, whatever the load.
    Share { retention_ns: u64 },
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

/// What the foreground does, as a check's [`Pacer`] reads it (T-T1.12.C7, C8): the calls the
/// database runs now, each counted from its submission to its answer ([`Foreground::call`]), and
/// whether the database ended. The last call's end, each commit and the database's end wake a
/// waiting check: it waits on events, never on a timer.
#[derive(Debug, Default)]
pub struct Foreground {
    in_flight: AtomicU64,
    /// The check thread waits for an event: whoever makes one tells it.
    waiting: AtomicBool,
    ended: Mutex<bool>,
    woken: Condvar,
}

/// A call in flight, counted until it drops ([`Foreground::call`]).
#[derive(Debug)]
pub struct InFlight(Arc<Foreground>);

impl Drop for InFlight {
    fn drop(&mut self) {
        // Only the last call's end wakes a check: the callers never meet on the check's lock while
        // the foreground stays busy; the store's commits, from its one thread, wake it meanwhile.
        if self.0.in_flight.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.wake();
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
    /// A call begins: counted in flight until what this returns drops.
    #[must_use]
    pub fn call(self: &Arc<Self>) -> InFlight {
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        InFlight(Arc::clone(self))
    }

    /// The store made a commit: a waiting check looks again.
    pub fn committed(&self) {
        self.wake();
    }

    /// Tells a waiting check of an event. The event's change is made before `waiting` is read and
    /// the waiter sets `waiting` before it reads the change, both sequentially consistent, so
    /// either the waiter sees the change or this sees the waiter; the lock taken to notify is the
    /// one it waits under, so the notice can't fall between its look and its wait.
    fn wake(&self) {
        if self.waiting.load(Ordering::SeqCst) {
            let _held = self.ended.lock();
            self.woken.notify_all();
        }
    }

    /// The database ended: a check under way stops at its next step.
    pub fn end(&self) {
        if let Ok(mut ended) = self.ended.lock() {
            *ended = true;
        }
        self.woken.notify_all();
    }

    /// Waits, event by event, while `blocked(busy)` holds, `busy` being whether a call is in
    /// flight: whether it waited at all. Refused [`Stopped`] once the database ended.
    fn wait_while(&self, mut blocked: impl FnMut(bool) -> bool) -> Result<bool, Stopped> {
        let mut ended = self.ended.lock().map_err(|_| Stopped)?;
        let mut waited = false;
        let out = loop {
            if *ended {
                break Err(Stopped);
            }
            self.waiting.store(true, Ordering::SeqCst);
            if !blocked(self.in_flight.load(Ordering::SeqCst) > 0) {
                break Ok(waited);
            }
            waited = true;
            match self.woken.wait(ended) {
                Ok(guard) => ended = guard,
                Err(_) => break Err(Stopped),
            }
        };
        self.waiting.store(false, Ordering::SeqCst);
        out
    }
}

/// Whether a step of `took_ns` over `bytes` went faster per byte than `best` (its nanoseconds and
/// bytes).
fn faster(took_ns: u64, bytes: u64, best: (u64, u64)) -> bool {
    u128::from(took_ns).saturating_mul(u128::from(best.1))
        < u128::from(best.0).saturating_mul(u128::from(bytes))
}

/// What a step cost the device (T-T1.12.C8): the time its bytes take at the fastest a step of its
/// kind went per byte (`reference`, its nanoseconds and bytes), never more than it took; what it
/// took when it moved no bytes or nothing is measured. A step slowed by the foreground counts what
/// it would take alone, as LEDBAT (RFC 6817, "Low Extra Delay Background Transport") and TCP Vegas
/// take the least delay they saw as the path's own.
#[must_use]
pub fn device_ns(took_ns: u64, bytes: u64, reference: Option<(u64, u64)>) -> u64 {
    match reference {
        Some((ns, of)) if bytes > 0 && of > 0 => u128::from(bytes)
            .saturating_mul(u128::from(ns))
            .checked_div(u128::from(of))
            .map_or(took_ns, |alone| {
                u64::try_from(alone).unwrap_or(u64::MAX).min(took_ns)
            }),
        _ => took_ns,
    }
}

/// Whether a check is behind its minimum share (T-T1.12.C8): after `elapsed_ns` of its `retention_ns`,
/// it owes `whole_ns × elapsed / retention` of work, `whole_ns` being the last whole check's work,
/// and has done `work_ns`. Before a whole check was measured there is no work to divide, and a check
/// is behind once it has run the whole period, its second about to be lost.
#[must_use]
pub fn behind(elapsed_ns: u64, retention_ns: u64, work_ns: u64, whole_ns: Option<u64>) -> bool {
    match whole_ns {
        Some(whole) => {
            u128::from(work_ns).saturating_mul(u128::from(retention_ns))
                < u128::from(whole).saturating_mul(u128::from(elapsed_ns))
        }
        None => elapsed_ns >= retention_ns,
    }
}

/// The fastest step per byte of one kind (writing or reading): the device's own speed, when a
/// check has it to itself. It decays: it holds the check under way's fastest and the last whole
/// check's, so a device that slowed for good (worn, throttled, moved) is read as it is now after
/// one check, as LEDBAT keeps its base delay as minima over recent intervals (RFC 6817's
/// BASE_HISTORY), the interval here a check: the event a reference changes on, not a clock's.
#[derive(Debug, Clone, Copy, Default)]
struct Fastest {
    now: Option<(u64, u64)>,
    before: Option<(u64, u64)>,
}

impl Fastest {
    fn reference(&self) -> Option<(u64, u64)> {
        match (self.now, self.before) {
            (Some(a), Some(b)) => Some(if faster(a.0, a.1, b) { a } else { b }),
            (a, b) => a.or(b),
        }
    }

    fn observe(&mut self, took_ns: u64, bytes: u64) {
        if bytes > 0 && self.now.is_none_or(|best| faster(took_ns, bytes, best)) {
            self.now = Some((took_ns, bytes));
        }
    }

    /// A check ended: its fastest becomes the one before, if it measured any.
    fn age(&mut self) {
        if self.now.is_some() {
            self.before = self.now.take();
        }
    }
}

/// What a restore or a digest does between its steps (T-T1.12.C7): a base's chunk, a log segment, a
/// replay's part, a chunk's worth of rows read, each wait of the integrity check on the device.
pub trait Pace {
    /// Before a step: waits while the foreground keeps the machine busy. Refused ([`Stopped`]) once
    /// the database ended: the check stops there.
    fn ready(&mut self) -> anyhow::Result<()>;
    /// After a step that moved `bytes`, written to `file` if it wrote. Paced, the file is made
    /// durable here, so the step's cost is the device's and its writes never pile up past a step.
    fn done(&mut self, file: Option<&std::fs::File>, bytes: u64) -> anyhow::Result<()>;
    /// Whether steps are paced: an unpaced restore replays its log in one step, as before.
    fn paced(&self) -> bool {
        true
    }
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

    fn paced(&self) -> bool {
        false
    }
}

/// A continuous check's pace (T-T1.12.C7, C8): each step begins only while the database runs no
/// call, woken by the call that ends last; under [`Yield::Share`] also while the check is behind
/// its minimum share, woken by each commit to look again. No timer: every wait ends on an event.
#[derive(Debug)]
pub struct Pacer {
    foreground: Arc<Foreground>,
    meter: Arc<DrillMeter>,
    policy: Yield,
    /// When the check under way took its first step; none between checks.
    check_began: Option<Instant>,
    step_began: Option<Instant>,
    /// The check under way's work so far and the last whole check's, in the device's nanoseconds
    /// ([`device_ns`]).
    work_ns: u64,
    whole_ns: Option<u64>,
    write: Fastest,
    read: Fastest,
}

impl Pacer {
    #[must_use]
    pub fn new(foreground: Arc<Foreground>, meter: Arc<DrillMeter>, policy: Yield) -> Self {
        Self {
            foreground,
            meter,
            policy,
            check_began: None,
            step_began: None,
            work_ns: 0,
            whole_ns: None,
            write: Fastest::default(),
            read: Fastest::default(),
        }
    }

    /// A check ended: whole, its work is the next check's measure and the fastest steps age
    /// ([`Fastest::age`]); stopped short, it's measured again by the next.
    pub fn end_check(&mut self, whole: bool) {
        if whole {
            self.whole_ns = Some(self.work_ns);
            self.write.age();
            self.read.age();
        }
        self.work_ns = 0;
        self.check_began = None;
        self.step_began = None;
    }
}

fn since_ns(at: Instant) -> u64 {
    u64::try_from(at.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

impl Pace for Pacer {
    fn ready(&mut self) -> anyhow::Result<()> {
        let began = *self.check_began.get_or_insert_with(Instant::now);
        let (policy, work, whole) = (self.policy, self.work_ns, self.whole_ns);
        let mut forced = false;
        let asked = Instant::now();
        let waited = self.foreground.wait_while(|busy| {
            forced = false;
            if !busy {
                return false;
            }
            match policy {
                Yield::Fully => true,
                Yield::Share { retention_ns } => {
                    forced = behind(since_ns(began), retention_ns, work, whole);
                    !forced
                }
            }
        })?;
        if waited {
            self.meter.paused.fetch_add(1, Ordering::Relaxed);
            self.meter
                .waited_ns
                .fetch_add(since_ns(asked), Ordering::Relaxed);
        }
        if forced {
            self.meter.forced.fetch_add(1, Ordering::Relaxed);
        }
        self.step_began = Some(Instant::now());
        Ok(())
    }

    fn done(&mut self, file: Option<&std::fs::File>, bytes: u64) -> anyhow::Result<()> {
        if let Some(file) = file {
            file.sync_data()?;
        }
        let took = self.step_began.take().map_or(0, since_ns);
        let fastest = if file.is_some() {
            &mut self.write
        } else {
            &mut self.read
        };
        fastest.observe(took, bytes);
        self.work_ns = self
            .work_ns
            .saturating_add(device_ns(took, bytes, fastest.reference()));
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
/// snapshot, the pace's pauses and waits for the foreground and the steps its minimum share began
/// with calls in flight, the checks' cost, each check's cost in power-of-two buckets for its
/// quantiles.
#[derive(Debug)]
pub struct DrillMeter {
    pub deferred_disk: AtomicU64,
    pub older: AtomicU64,
    /// Steps that waited for the foreground, and the nanoseconds waited.
    pub paused: AtomicU64,
    pub waited_ns: AtomicU64,
    /// Steps begun with calls in flight, the check behind its minimum share ([`Yield::Share`]).
    pub forced: AtomicU64,
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
/// its pauses included.
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
            forced: AtomicU64::new(0),
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

    /// The cost at quantile `per / of` (`per` above `of` counts as 1) as its bucket's upper bound
    /// in nanoseconds, so within a factor of two above; none before a check or for `of` of 0.
    #[must_use]
    pub fn cost_quantile(&self, per: u64, of: u64) -> Option<u64> {
        let total = u128::from(self.checks());
        if total == 0 || of == 0 {
            return None;
        }
        // The rank of the quantile among `total` checks, 1-based (nearest rank): the ceiling of
        // total * per / of, exact in u128 (a u64 product never overflows it).
        let (per, of) = (u128::from(per.min(of)), u128::from(of));
        let rank = total
            .checked_mul(per)?
            .checked_add(of.checked_sub(1)?)?
            .checked_div(of)?
            .max(1);
        let mut seen = 0u128;
        for (bits, bucket) in self.buckets.iter().enumerate() {
            seen = seen.saturating_add(u128::from(bucket.load(Ordering::Relaxed)));
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

    /// T-T1.12.C8: a step costs the device what its bytes take at the fastest a step of its kind
    /// went, never more than it took; what it took with no bytes or nothing measured.
    #[test]
    fn t_t1_12_c8_a_step_costs_what_it_would_alone() {
        assert_eq!(device_ns(100, 1_000, Some((100, 1_000))), 100);
        assert_eq!(device_ns(200, 1_000, Some((100, 1_000))), 100);
        assert_eq!(device_ns(1_000, 500, Some((100, 1_000))), 50);
        assert_eq!(device_ns(50, 1_000, Some((100, 1_000))), 50);
        assert_eq!(device_ns(100, 0, Some((100, 1_000))), 100);
        assert_eq!(device_ns(100, 1_000, None), 100);
        assert_eq!(device_ns(100, 1_000, Some((0, 0))), 100);
        assert_eq!(device_ns(u64::MAX, u64::MAX, Some((u64::MAX, 1))), u64::MAX);
        assert!(faster(1, 10, (2, 10)));
        assert!(!faster(2, 10, (2, 10)));
    }

    /// T-T1.12.C8: a check is behind its minimum share once its work falls under the last whole
    /// check's work times the share of the retention period gone; before any whole check, once
    /// the period is gone.
    #[test]
    fn t_t1_12_c8_the_minimum_share_is_a_checks_work_over_the_retention() {
        // A whole check of 100 over a period of 1,000: half the period gone owes 50.
        assert!(behind(500, 1_000, 49, Some(100)));
        assert!(!behind(500, 1_000, 50, Some(100)));
        assert!(!behind(0, 1_000, 0, Some(100)));
        assert!(behind(2_000, 1_000, 199, Some(100)));
        assert!(!behind(500, 1_000, 0, Some(0)));
        assert!(!behind(999, 1_000, 0, None));
        assert!(behind(1_000, 1_000, 0, None));
        assert!(!behind(u64::MAX, u64::MAX, u64::MAX, Some(u64::MAX)));
    }

    /// T-T1.12.C8: the fastest-step reference decays: a check's fastest holds through the next
    /// check, then gives way to that check's own, slower or not; a check that measured nothing
    /// keeps the one before.
    #[test]
    fn t_t1_12_c8_the_fastest_step_reference_decays() {
        let mut fastest = Fastest::default();
        assert_eq!(fastest.reference(), None);
        fastest.observe(100, 1_000);
        fastest.observe(150, 1_000);
        assert_eq!(fastest.reference(), Some((100, 1_000)));
        fastest.age();
        fastest.observe(300, 1_000);
        assert_eq!(
            fastest.reference(),
            Some((100, 1_000)),
            "held through the next check"
        );
        fastest.age();
        assert_eq!(
            fastest.reference(),
            Some((300, 1_000)),
            "then the slower device's own"
        );
        fastest.age();
        assert_eq!(
            fastest.reference(),
            Some((300, 1_000)),
            "kept by a check that measured none"
        );
        fastest.observe(1, 0);
        assert_eq!(fastest.now, None, "a step of no bytes measures nothing");
    }

    /// T-T1.12.C8: yielding fully, a step never begins while a call is in flight: it begins once the
    /// last call ends, woken by that end, the wait counted; with nothing in flight it begins at once.
    #[test]
    fn t_t1_12_c8_yielding_fully_waits_for_the_last_call_to_end() {
        let foreground = Arc::new(Foreground::default());
        let meter = Arc::new(DrillMeter::default());
        let mut pacer = Pacer::new(Arc::clone(&foreground), Arc::clone(&meter), Yield::Fully);
        pacer.ready().unwrap();
        pacer.done(None, 4096).unwrap();
        assert_eq!(meter.paused.load(Ordering::Relaxed), 0);
        let first = foreground.call();
        let second = foreground.call();
        let ended = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&ended);
        let step = std::thread::spawn(move || {
            pacer.ready().unwrap();
            assert!(
                seen.load(Ordering::SeqCst),
                "a step began with a call in flight"
            );
            pacer
        });
        drop(first);
        foreground.committed();
        ended.store(true, Ordering::SeqCst);
        drop(second);
        let pacer = step.join().unwrap();
        assert_eq!(meter.forced.load(Ordering::Relaxed), 0);
        assert!(pacer.step_began.is_some());
    }

    /// T-T1.12.C8: under a minimum share, a check behind it takes its step with calls in flight,
    /// counted; once the database ends a waiting step is refused [`Stopped`], not failed.
    #[test]
    fn t_t1_12_c8_a_share_steps_through_load_and_the_end_stops_it() {
        let foreground = Arc::new(Foreground::default());
        let meter = Arc::new(DrillMeter::default());
        // A period of none: any check is behind.
        let mut shared = Pacer::new(
            Arc::clone(&foreground),
            Arc::clone(&meter),
            Yield::Share { retention_ns: 0 },
        );
        let held = foreground.call();
        shared.ready().unwrap();
        shared.done(None, 0).unwrap();
        assert_eq!(meter.forced.load(Ordering::Relaxed), 1);
        shared.end_check(true);
        assert!(shared.whole_ns.is_some(), "a whole check measures the next");
        let mut fully = Pacer::new(Arc::clone(&foreground), Arc::clone(&meter), Yield::Fully);
        let waiting = std::thread::spawn(move || fully.ready().unwrap_err());
        foreground.end();
        let refused = waiting.join().unwrap();
        assert_eq!(refused.downcast_ref::<Stopped>(), Some(&Stopped));
        drop(held);
        assert!(Unpaced.ready().is_ok());
        assert!(!Unpaced.paced());
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
        assert_eq!(meter.cost_quantile(1, 2), None);
        for ns in [1, 3, 3, 100, 1_000_000] {
            meter.cost(ns);
        }
        assert_eq!(meter.checks(), 5);
        assert_eq!(meter.cost_quantile(1, 2), Some(3));
        assert_eq!(meter.cost_quantile(4, 5), Some(127));
        assert_eq!(meter.cost_quantile(999, 1000), Some((1 << 20) - 1));
        assert_eq!(meter.cost_quantile(0, 1), Some(1));
        assert_eq!(meter.cost_quantile(3, 1), Some((1 << 20) - 1));
        assert_eq!(meter.cost_quantile(1, 0), None);
        assert_eq!(meter.cost_ns.load(Ordering::Relaxed), 1_000_107);
    }
}
