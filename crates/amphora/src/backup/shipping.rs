//! The thread that backs a [`crate::database::Database`]'s local writer up (CP-T1.12;
//! docs/design/backup.md, "The database's shipping thread").
//!
//! It wakes on each commit the store makes, never on a timer: a round ships what became durable
//! while the last ran, then copies one chunk of a base while one is due, so its cadence follows
//! the log's rate and the store's put time. It ships through a reader on the writer's engine, so
//! the writer never waits on the store. When the database ends it ships once more and ends.
//!
//! A thread of its own beside it restores and checks the archive on the archive's cadence
//! ([`super::check_due`]), as [`Archive::check`] says ([`CheckPolicy`]): against the source's rows
//! at a second, digested from a reader whose snapshot lines up with that second's last commit
//! ([`super::Marks::quiet_second`]). The shipping thread tells it of each round and never waits on
//! it. Continuous ([`CheckPolicy::Continuous`], [`CheckPolicy::Shared`]), the second is drawn at
//! random from those it holds the rows of ([`Drills`]), and each step of a check yields to the
//! foreground's calls in flight ([`Pacer`], T-T1.12.C7, C8).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use amphora_api::blobs::Blobs;

use super::check::step_bytes;
use super::drill::{Foreground, Pace, Pacer, Unpaced};
use super::plan::Lacks;
use super::restore::remove;
use super::ship::FRAME_BYTES;
use super::{
    BaseCopy, Budget, Check, CheckCounters, CheckFailure, CheckPolicy, ChunkGrowth, Clock,
    DrillMeter, Drills, Hooks, RowDigest, Shipped, Shipper, Stopped, admit, check_due,
    digest_paced, newest_second, next_random, plan, restore_and_check_at,
};
use crate::db::{Db, PAGE_BYTES, log_paths, restoring_path};
use crate::store::Watcher;

/// Where and how a database is backed up ([`crate::database::Database::open_backed_up`]).
#[derive(Clone)]
pub struct Archive {
    /// The object store: Mantle, through kiln's adapter, in production.
    pub blobs: Arc<dyn Blobs>,
    /// The archive's prefix in the store: one database's.
    pub prefix: String,
    /// When each commit was made ([`system_clock`] in production).
    pub clock: Clock,
    /// The most bytes a base's chunk may hold ([`Shipper::begin_base`]'s `cap`): under it, a
    /// chunk holds what the process's memory budget leaves its stream (T-T1.12.M1).
    pub memory: usize,
    /// How long one request to the store may take: past it the round is given up and tried again
    /// at the next commit. The caller states it, as the store's own answer time is the caller's
    /// to know (Needs Ada's word: no measured default yet).
    pub patience: Duration,
    /// Whether and where the thread restores its archive to check it (T-T1.12.C3, C4): a new file
    /// each check, removed once it passes and kept where it restored and failed, to look into. By default
    /// ([`CheckPolicy::Continuous`]) beside the database, within the machine's budget.
    pub check: CheckPolicy,
    /// Whether a new base's chunks grow with the room or keep the latest base's size
    /// ([`ChunkGrowth`], T-T1.12.M5's A/B): `Keep`, today's behaviour, until the A/B of record
    /// decides.
    pub chunks: ChunkGrowth,
}

/// Nanoseconds since the Unix epoch by the system's clock: 0 before the epoch, the most a `u64`
/// holds past its range (both held back by [`super::Marks`], which never stamps backwards).
#[must_use]
pub fn system_clock() -> Clock {
    Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
    })
}

/// What the shipping thread has done.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// Rounds that ended without failing.
    pub rounds: u64,
    /// All it shipped of the log.
    pub shipped: Shipped,
    /// Bases it ended.
    pub bases: u64,
    /// Rounds that failed, and the latest failure.
    pub failures: u64,
    pub last_failure: Option<String>,
    /// Whether the thread ended (its database did).
    pub ended: bool,
    /// The latest restore-and-check's outcome ([`Archive::check`]).
    pub last_check: Option<Check>,
}

/// A handle on a database's shipping thread: what it has done, and waits on it.
#[derive(Clone)]
pub struct Shipping {
    status: Arc<(Mutex<Status>, Condvar)>,
    counters: Arc<CheckCounters>,
    meter: Arc<DrillMeter>,
}

impl Shipping {
    fn locked(&self) -> anyhow::Result<MutexGuard<'_, Status>> {
        self.status
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("the backup's status lock was poisoned"))
    }

    /// The checks the thread ran, by result: `amphora_backup_checks_total` ([`super::CHECK_METRIC`]).
    #[must_use]
    pub fn checks(&self) -> &CheckCounters {
        &self.counters
    }

    /// What the drills measured: deferrals by reason ([`super::DEFER_METRIC`]), each check's cost
    /// ([`super::COST_METRIC`]), the pace's pauses, waits and forced steps.
    #[must_use]
    pub fn drills(&self) -> &DrillMeter {
        &self.meter
    }

    /// What the thread has done so far.
    pub fn status(&self) -> anyhow::Result<Status> {
        Ok(self.locked()?.clone())
    }

    /// Waits until `done` holds of the thread's status, until `until` at most (refused then).
    pub fn wait(&self, until: Instant, done: impl Fn(&Status) -> bool) -> anyhow::Result<Status> {
        let mut status = self.locked()?;
        loop {
            if done(&status) {
                return Ok(status.clone());
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                anyhow::bail!("the backup's status didn't reach what was waited for in time");
            }
            status = self
                .status
                .1
                .wait_timeout(status, left)
                .map_err(|_| anyhow::anyhow!("the backup's status lock was poisoned"))?
                .0;
        }
    }

    fn record(&self, change: impl FnOnce(&mut Status)) {
        if let Ok(mut status) = self.locked() {
            change(&mut status);
        }
        self.status.1.notify_all();
    }
}

/// A backup made ready on the writer as it opens, its threads not started yet.
pub(crate) struct Prepared {
    archive: Archive,
    shipper: Shipper,
    reader: Db,
    hooks: Arc<Hooks>,
    /// What checks the archive, on a thread of its own ([`Archive::check`]); none when off.
    checker: Option<Checker>,
    /// When the store commits, as the checks' pace reads it.
    foreground: Arc<Foreground>,
    counters: Arc<CheckCounters>,
    meter: Arc<DrillMeter>,
    /// Whether a base that was to begin waits for the log's closing segment to be copied.
    base_waits: bool,
    mends: Arc<Mends>,
}

/// What the check thread asks of the shipping thread: a base for what a check's restore found the
/// archive to lack (the latest ask; an earlier one untaken, the next check finds again), and that
/// thread's wake, held weakly: the thread ends once the store's own is gone.
#[derive(Default)]
struct Mends {
    asked: Mutex<(Option<Lacks>, Weak<SyncSender<()>>)>,
}

impl Mends {
    /// Asks, and wakes the shipping thread: its next round begins the base, with no commit to wait for.
    fn ask(&self, lacks: Lacks) {
        let Ok(mut state) = self.asked.lock() else {
            return;
        };
        let (asked, wake) = &mut *state;
        *asked = Some(lacks);
        // Full, a wake is pending already: the round that takes this.
        if let Some(wake) = wake.upgrade() {
            let _pending = wake.try_send(());
        }
    }

    fn take(&self) -> Option<Lacks> {
        self.asked.lock().ok()?.0.take()
    }

    fn wakes(&self, wake: Weak<SyncSender<()>>) {
        if let Ok(mut state) = self.asked.lock() {
            state.1 = wake;
        }
    }
}

/// What the shipping thread tells the check thread: rounds ended, the latest base, the log shipped
/// since the check thread last looked, and whether the database ended.
#[derive(Default)]
struct Feed {
    state: Mutex<FeedState>,
    woken: Condvar,
}

#[derive(Default)]
struct FeedState {
    rounds: u64,
    latest_base: Option<u64>,
    shipped: u64,
    ended: bool,
}

impl Feed {
    fn round(&self, latest_base: Option<u64>, shipped: u64, ended: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.rounds = state.rounds.saturating_add(1);
            state.latest_base = latest_base;
            state.shipped = state.shipped.saturating_add(shipped);
            state.ended |= ended;
        }
        self.woken.notify_all();
    }

    /// Waits for a round after `seen`: its count, the latest base and the log shipped since the
    /// last look; none once the database ended.
    fn next(&self, seen: u64) -> Option<(u64, Option<u64>, u64)> {
        let mut state = self.state.lock().ok()?;
        loop {
            if state.ended {
                return None;
            }
            if state.rounds != seen {
                let shipped = std::mem::take(&mut state.shipped);
                return Some((state.rounds, state.latest_base, shipped));
            }
            state = self.woken.wait(state).ok()?;
        }
    }
}

/// The check thread's state: a restore-and-check of the archive, its source's snapshot and its
/// pace.
struct Checker {
    blobs: Arc<dyn Blobs>,
    prefix: String,
    patience: Duration,
    memory: usize,
    hooks: Arc<Hooks>,
    /// The reader a check's snapshot of the source is taken on, apart from the shipping reader.
    reader: Db,
    counters: Arc<CheckCounters>,
    /// The latest check's base and the log it replayed, the log shipped since
    /// ([`super::check_due`]), and a quiet snapshot's second, rows and base waiting to be checked.
    last_check: Option<(u64, u64)>,
    log_since_check: u64,
    pending_check: Option<(u64, RowDigest, u64)>,
    /// Where checks restore to ([`Archive::check`]).
    scratch: PathBuf,
    /// The database's file, whose size (with its log's) a restore writes.
    source: PathBuf,
    meter: Arc<DrillMeter>,
    drills: Drills,
    random: u64,
    /// The checks' pace, at drilled seconds ([`CheckPolicy::Continuous`], [`CheckPolicy::Shared`]);
    /// none unpaced, at the newest second ([`CheckPolicy::Scratch`]).
    pacer: Option<Pacer>,
    /// What the pending check's digest cost.
    pending_cost: u64,
    mends: Arc<Mends>,
}

fn elapsed_ns(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

/// `path` with `suffix` after its name.
fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// A file's length; 0 when it isn't there.
fn file_len(path: &Path) -> anyhow::Result<u64> {
    match std::fs::metadata(path) {
        Ok(m) => Ok(m.len()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(e.into()),
    }
}

impl Prepared {
    /// The writer's stamps.
    pub(crate) fn hooks(&self) -> Arc<Hooks> {
        Arc::clone(&self.hooks)
    }

    /// What the checks' pace reads of the foreground: the database counts its calls in it
    /// ([`Foreground::call`], T-T1.12.C8).
    pub(crate) fn foreground(&self) -> Arc<Foreground> {
        Arc::clone(&self.foreground)
    }

    /// Backs `db`, the writer of the database at `path`, up to `archive`: its commits stamped from
    /// now on, its log started again only once shipped ([`Db::back_up`]), and a shipper carrying
    /// on from what the archive holds, on a reader of the same engine.
    pub(crate) fn on(
        db: &mut Db,
        path: &Path,
        archive: Archive,
        until: Instant,
    ) -> anyhow::Result<Self> {
        let hooks = Hooks::new(Arc::clone(&archive.clock), db.log_kept_frames());
        db.back_up(Arc::clone(&hooks));
        let reader = db.reader()?;
        // The shipping reader may be the engine's last connection to close: its close must not
        // empty a log the backup hasn't shipped either.
        reader.hold_log();
        let scratch = match &archive.check {
            CheckPolicy::Off => None,
            CheckPolicy::Scratch(scratch) => {
                Some(scratch.clone().unwrap_or_else(|| suffixed(path, ".check")))
            }
            CheckPolicy::Continuous | CheckPolicy::Shared(_) => Some(suffixed(path, ".check")),
        };
        let foreground = Arc::new(Foreground::default());
        let counters: Arc<CheckCounters> = Arc::default();
        let meter: Arc<DrillMeter> = Arc::default();
        let mends: Arc<Mends> = Arc::default();
        let checker = match scratch {
            Some(scratch) => {
                let reader = db.reader()?;
                reader.hold_log();
                // The drills' draws: seeded once from the system's entropy.
                let mut seed = [0u8; 8];
                getrandom::fill(&mut seed)
                    .map_err(|e| anyhow::anyhow!("the system's entropy: {e}"))?;
                Some(Checker {
                    blobs: Arc::clone(&archive.blobs),
                    prefix: archive.prefix.clone(),
                    patience: archive.patience,
                    memory: archive.memory,
                    hooks: Arc::clone(&hooks),
                    reader,
                    counters: Arc::clone(&counters),
                    last_check: None,
                    log_since_check: 0,
                    pending_check: None,
                    scratch,
                    source: path.to_path_buf(),
                    meter: Arc::clone(&meter),
                    drills: Drills::default(),
                    random: u64::from_le_bytes(seed),
                    pacer: archive.check.pace().map(|policy| {
                        Pacer::new(Arc::clone(&foreground), Arc::clone(&meter), policy)
                    }),
                    pending_cost: 0,
                    mends: Arc::clone(&mends),
                })
            }
            None => None,
        };
        let mut shipper = Shipper::open(
            &archive.prefix,
            Arc::clone(&hooks),
            archive.blobs.as_ref(),
            &reader,
            until,
        )?;
        shipper.set_chunk_growth(archive.chunks);
        Ok(Self {
            archive,
            shipper,
            reader,
            hooks,
            checker,
            foreground,
            counters,
            meter,
            base_waits: false,
            mends,
        })
    }

    /// Starts the threads, the shipping one woken by each commit `on_commit` reports. They end once
    /// the store drops its watchers, as the database ends.
    pub(crate) fn start(mut self, on_commit: impl FnOnce(Watcher)) -> anyhow::Result<Shipping> {
        // One wake pending at most: a commit while one waits is shipped by the same round.
        let (wake, woken) = mpsc::sync_channel::<()>(1);
        // The first round runs at once: what the database committed as it opened is shipped, and
        // its first base begins, without waiting for a commit.
        wake.try_send(())?;
        let wake = Arc::new(wake);
        self.mends.wakes(Arc::downgrade(&wake));
        let foreground = Arc::clone(&self.foreground);
        on_commit(Box::new(move || {
            foreground.committed();
            !matches!(wake.try_send(()), Err(TrySendError::Disconnected(())))
        }));
        let shipping = Shipping {
            status: Arc::default(),
            counters: Arc::clone(&self.counters),
            meter: Arc::clone(&self.meter),
        };
        let feed = Arc::new(Feed::default());
        let checks = match self.checker.take() {
            Some(checker) => {
                let told = shipping.clone();
                let fed = Arc::clone(&feed);
                Some(
                    std::thread::Builder::new()
                        .name("amphora-backup-check".into())
                        .spawn(move || checker.run(&fed, &told))?,
                )
            }
            None => None,
        };
        let told = shipping.clone();
        std::thread::Builder::new()
            .name("amphora-backup".into())
            .spawn(move || self.run(&woken, &told, &feed, checks))?;
        Ok(shipping)
    }

    fn run(
        mut self,
        woken: &Receiver<()>,
        shipping: &Shipping,
        feed: &Feed,
        checks: Option<std::thread::JoinHandle<()>>,
    ) {
        let mut copy: Option<BaseCopy> = None;
        loop {
            match self.round(&mut copy) {
                Ok((shipped, base_ended)) => {
                    shipping.record(|s| {
                        s.rounds = s.rounds.saturating_add(1);
                        s.shipped.segments = s.shipped.segments.saturating_add(shipped.segments);
                        s.shipped.bytes = s.shipped.bytes.saturating_add(shipped.bytes);
                        if base_ended {
                            s.bases = s.bases.saturating_add(1);
                        }
                    });
                    feed.round(self.shipper.latest_base(), shipped.bytes, false);
                }
                Err(e) => {
                    let why = format!("{e:#}");
                    tracing::warn!(target: "amphora.backup", event = "backup.round_failed", prefix = %self.archive.prefix, error = %why, "a backup round failed: tried again at the next commit");
                    // A base under way starts over: its chunks may straddle the failure.
                    copy = None;
                    shipping.record(|s| {
                        s.failures = s.failures.saturating_add(1);
                        s.last_failure = Some(why);
                    });
                }
            }
            // A base under way copies its next chunk at once; otherwise the next commit wakes it.
            let next = if copy.is_some() {
                match woken.try_recv() {
                    Ok(()) | Err(TryRecvError::Empty) => Ok(()),
                    Err(TryRecvError::Disconnected) => Err(()),
                }
            } else {
                woken.recv().map_err(|_| ())
            };
            if next.is_err() {
                break;
            }
        }
        // The database ended: what it committed last is shipped before the thread lets go of it,
        // and a check under way stops at its next step.
        let last = self.ship();
        self.foreground.end();
        feed.round(None, 0, true);
        if let Some(checks) = checks
            && checks.join().is_err()
        {
            tracing::error!(target: "amphora.backup", event = "backup.check_thread_panicked", prefix = %self.archive.prefix);
        }
        shipping.record(|s| {
            match last {
                Ok(shipped) => {
                    s.rounds = s.rounds.saturating_add(1);
                    s.shipped.segments = s.shipped.segments.saturating_add(shipped.segments);
                    s.shipped.bytes = s.shipped.bytes.saturating_add(shipped.bytes);
                }
                Err(e) => {
                    s.failures = s.failures.saturating_add(1);
                    s.last_failure = Some(format!("{e:#}"));
                }
            }
            s.ended = true;
        });
    }

    fn until(&self) -> Instant {
        Instant::now()
            .checked_add(self.archive.patience)
            .unwrap_or_else(Instant::now)
    }

    fn ship(&mut self) -> anyhow::Result<Shipped> {
        let until = self.until();
        self.shipper
            .ship(&self.reader, self.archive.blobs.as_ref(), until)
    }

    /// Ships, then moves a base on by one chunk while one is due: what it shipped, and whether a
    /// base ended.
    fn round(&mut self, copy: &mut Option<BaseCopy>) -> anyhow::Result<(Shipped, bool)> {
        if let Some(lacks) = self.mends.take()
            && self.shipper.mend(&self.reader, lacks)?
        {
            tracing::warn!(target: "amphora.backup", event = "backup.base_begun_again", prefix = %self.archive.prefix, lacks = ?lacks, "a check found the archive short: a new base begins");
            *copy = None;
        }
        let generation = self.shipper.generation();
        let shipped = self.ship()?;
        let blobs = self.archive.blobs.as_ref();
        let until = self.until();
        // A base begun before the log broke restores nothing after it: another begins.
        if copy.as_ref().is_some_and(|c| self.shipper.broke_since(c)) {
            *copy = None;
        }
        match copy.as_mut() {
            None => {
                // A base begins in a generation the shipper has shipped: after a round that
                // shipped, or once there's nothing left to (`begin_base` refuses otherwise); and
                // just after the log starts again, so the next base copies least (T-T1.12.I4).
                let begins = self.base_waits
                    || plan::base_begins_now(
                        self.shipper.base_needed(),
                        self.shipper.generation() != generation,
                        self.shipper.frames_in_generation(),
                        self.reader.log_kept_frames(),
                    );
                if self.shipper.base_due(None) && self.shipper.ships_current(&self.reader) && begins
                {
                    // The file must hold the closing segment first (decision 45): a pass copies
                    // what the reads under way let it, and where one holds it back the base waits
                    // for a later round.
                    if !self.reader.closing_copied() {
                        let _copied = self.reader.checkpoint()?;
                    }
                    self.base_waits = !self.reader.closing_copied();
                    if !self.base_waits {
                        *copy = Some(self.shipper.begin_base(
                            &self.reader,
                            blobs,
                            &crate::memory::process(),
                            self.archive.memory,
                        )?);
                    }
                }
                Ok((shipped, false))
            }
            Some(c) => {
                if self.shipper.copy_base(&self.reader, blobs, c, until)? {
                    return Ok((shipped, false));
                }
                let Some(done) = copy.take() else {
                    return Ok((shipped, false));
                };
                self.shipper.end_base(&self.reader, blobs, done, until)?;
                Ok((shipped, true))
            }
        }
    }
}

impl Checker {
    fn until(&self) -> Instant {
        Instant::now()
            .checked_add(self.patience)
            .unwrap_or_else(Instant::now)
    }

    /// Checks after each round the shipping thread tells of, until the database ends.
    fn run(mut self, feed: &Feed, shipping: &Shipping) {
        let mut seen = 0;
        while let Some((rounds, latest_base, shipped)) = feed.next(seen) {
            seen = rounds;
            self.log_since_check = self.log_since_check.saturating_add(shipped);
            match self.checks(latest_base) {
                Ok(Some(check)) => shipping.record(|s| s.last_check = Some(check)),
                Ok(None) => {}
                Err(e) if e.downcast_ref::<Stopped>().is_some() => break,
                Err(e) => {
                    let why = format!("{e:#}");
                    tracing::warn!(target: "amphora.backup", event = "backup.check_failed_to_run", prefix = %self.prefix, error = %why, "a backup check couldn't run: tried again after the next round");
                    shipping.record(|s| {
                        s.failures = s.failures.saturating_add(1);
                        s.last_failure = Some(why);
                    });
                }
            }
        }
    }

    /// Checks a quiet snapshot's second once it settled and the archive holds it whole, then takes
    /// the next snapshot when a check is due (T-T1.12.C3): what a check found, if one ran.
    fn checks(&mut self, latest_base: Option<u64>) -> anyhow::Result<Option<Check>> {
        let check = self.check_pending(latest_base)?;
        if self.pending_check.is_none()
            && let Some(base) = latest_base
            && check_due(self.last_check, base, self.log_since_check)
            && self.admitted()?
        {
            let began = Instant::now();
            self.pending_check = self
                .quiet_digest()?
                .map(|(second, rows)| (second, rows, base));
            self.pending_cost = elapsed_ns(began);
        }
        Ok(check)
    }

    /// What a check's restore takes of the scratch disk, and what the disk has ([`admit`]): the
    /// database's pages, those only its log holds yet among them, and beside them the generation of
    /// the log being replayed, in either of the source's two segments (decision 45).
    fn budget(&self) -> anyhow::Result<Budget> {
        let disk = match self.scratch.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        let pages = self.reader.pages()?.saturating_mul(PAGE_BYTES);
        let mut restore = file_len(&self.source)?.max(pages);
        let mut log_headroom = 0u64;
        for segment in log_paths(&self.source) {
            restore = restore.saturating_add(file_len(&segment)?);
            log_headroom = log_headroom
                .saturating_add(self.reader.log_kept_frames().saturating_mul(FRAME_BYTES));
        }
        Ok(Budget {
            free: fs4::available_space(disk)?,
            restore,
            log_headroom,
        })
    }

    /// Whether a due check begins now: at once given a scratch path; continuous, once the disk
    /// holds it ([`admit`], T-T1.12.C5), a deferral counted with its reason.
    fn admitted(&self) -> anyhow::Result<bool> {
        if self.pacer.is_none() {
            return Ok(true);
        }
        let budget = self.budget()?;
        match admit(&budget) {
            Ok(()) => Ok(true),
            Err(why) => {
                self.meter.defer(why);
                tracing::debug!(target: "amphora.backup.check", event = "deferred", reason = why.reason(), free = budget.free, restore = budget.restore, log_headroom = budget.log_headroom);
                Ok(false)
            }
        }
    }

    /// The source's rows at a second, from a snapshot lined up with that second's last commit
    /// ([`super::Marks::quiet_second`]); none when the log isn't quiet now.
    fn quiet_digest(&mut self) -> anyhow::Result<Option<(u64, RowDigest)>> {
        let step = step_bytes(self.blobs.as_ref());
        let marks = &self.hooks.marks;
        let mut unpaced = Unpaced;
        let pace: &mut dyn Pace = match self.pacer.as_mut() {
            Some(pacer) => pacer,
            None => &mut unpaced,
        };
        self.reader.query_transaction(|r| {
            let second = marks.quiet_second(|| {
                // A deferred `BEGIN` takes no snapshot: this first read takes it, under the lock
                // commits are stamped by, and the log's end is read after it.
                r.query("SELECT count(*) FROM sqlite_schema", [], |_| Ok(true))?;
                Ok(r.log_end())
            })?;
            match second {
                Some(second) => Ok(Some((second, digest_paced(r, pace, step)?))),
                None => Ok(None),
            }
        })
    }

    fn check_pending(&mut self, latest_base: Option<u64>) -> anyhow::Result<Option<Check>> {
        let Some((second, _, _)) = self.pending_check else {
            return Ok(None);
        };
        match self.hooks.marks.settled(second)? {
            None => return Ok(None),
            Some(false) => {
                self.pending_check = None;
                if let Some(pacer) = self.pacer.as_mut() {
                    pacer.end_check(false);
                }
                return Ok(None);
            }
            Some(true) => {}
        }
        let scratch = self.scratch.clone();
        let until = self.until();
        let blobs = self.blobs.as_ref();
        // The archive holds the second whole once a round shipped past its end: until then the
        // snapshot waits for the next round.
        if newest_second(blobs, &self.prefix, until)?.is_none_or(|newest| newest < second) {
            return Ok(None);
        }
        let Some((newest, rows, base)) = self.pending_check.take() else {
            return Ok(None);
        };
        // Continuous: the snapshot joins the drills, and the check restores one of their seconds
        // drawn at random, so every second the thread ever held is as likely to be drilled
        // (T-T1.12.C6). Given a scratch path: the snapshot's own second.
        let (second, rows) = if self.pacer.is_some() {
            let draw = next_random(&mut self.random);
            self.drills.offer(newest, rows, self.memory, draw);
            let draw = next_random(&mut self.random);
            let Some((second, rows)) = self.drills.pick(draw).cloned() else {
                return Ok(None);
            };
            (second, rows)
        } else {
            (newest, rows)
        };
        if second < newest {
            self.meter
                .older
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        remove_scratch(&scratch)?;
        let began = Instant::now();
        let source = |s: u64| Ok((s == second).then(|| rows.clone()));
        let mut unpaced = Unpaced;
        let pace: &mut dyn Pace = match self.pacer.as_mut() {
            Some(pacer) => pacer,
            None => &mut unpaced,
        };
        let check = restore_and_check_at(
            blobs,
            &self.prefix,
            second,
            &scratch,
            &source,
            &self.counters,
            pace,
            until,
        );
        // A check run to its end measures the next one's minimum share (T-T1.12.C8).
        if let Some(pacer) = self.pacer.as_mut() {
            pacer.end_check(check.is_ok());
        }
        let check = check?;
        if let Check::Failed {
            why: CheckFailure::Lacking { lacks, .. },
            ..
        } = &check
        {
            self.mends.ask(*lacks);
        }
        let cost = self.pending_cost.saturating_add(elapsed_ns(began));
        self.meter.cost(cost);
        self.last_check = Some(match &check {
            Check::Passed { restored, .. } => {
                remove_scratch(&scratch)?;
                (base, restored.log_bytes)
            }
            Check::Expired { second } => {
                self.drills.drop_second(*second);
                (base, self.log_since_check)
            }
            // Not passed: checked again at the next base, or once as much log again has shipped.
            _ => (latest_base.unwrap_or(0), self.log_since_check),
        });
        self.log_since_check = 0;
        Ok(Some(check))
    }
}

/// Removes a check's scratch database and its log, and what a restore to it that ended midway (its
/// process killed) left, if there.
fn remove_scratch(scratch: &Path) -> anyhow::Result<()> {
    remove(scratch)?;
    remove(&suffixed(scratch, "-shm"))?;
    remove(&restoring_path(scratch))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use amphora_api::blobs::SimBlobs;

    use super::*;
    use crate::backup::{SECOND_NS, restore, restore_paced};

    fn until() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    /// The archive a test's database is backed up to, its clock `now`.
    fn archive(blobs: &Arc<SimBlobs>, now: &Arc<AtomicU64>, check: CheckPolicy) -> Archive {
        let read = Arc::clone(now);
        Archive {
            blobs: Arc::clone(blobs) as Arc<dyn Blobs>,
            prefix: "db".into(),
            clock: Arc::new(move || read.load(Ordering::SeqCst)),
            memory: 32 * 1024,
            patience: Duration::from_secs(60),
            chunks: ChunkGrowth::Keep,
            check,
        }
    }

    /// A database at `path` backed up round by round, its table made, with its clock.
    fn backed_up(
        path: &Path,
        blobs: &Arc<SimBlobs>,
        kept: Option<u64>,
        check: CheckPolicy,
    ) -> (Db, Prepared, Arc<AtomicU64>) {
        let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS));
        let mut db = Db::open(path).unwrap();
        if let Some(kept) = kept {
            db.keep_log(kept);
        }
        let archive = archive(blobs, &now, check);
        let prepared = Prepared::on(&mut db, path, archive, until()).unwrap();
        db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")
            .unwrap();
        (db, prepared, now)
    }

    /// Writes `rows` rows from id `from`, one commit a second.
    fn write(db: &Db, now: &AtomicU64, from: i64, rows: i64) {
        for id in from..from + rows {
            now.fetch_add(SECOND_NS, Ordering::SeqCst);
            db.exec(&format!(
                "INSERT INTO t (id, v) VALUES ({id}, '{}')",
                "z".repeat(500)
            ))
            .unwrap();
        }
    }

    fn ids(db: &Db) -> Vec<i64> {
        let mut ids = Vec::new();
        db.query("SELECT id FROM t ORDER BY id", [], |r| {
            ids.push(r.get::<i64>(0).map_err(|e| anyhow::anyhow!("{e}"))?);
            Ok(true)
        })
        .unwrap();
        ids
    }

    /// Decision 45 (lane B22): a due base waits for the log's closing segment to be in the file.
    /// Do: end a first base; write until another is due; open a read and leave it open, write 20
    /// rows and ship them, start the log again (the read keeps those 20 out of the file), write a
    /// row and run a round; end the read, write a row and run rounds until a base ends; restore the
    /// last second. Expect: no base begins while the read is open, one does in the next round after
    /// it, and the restore holds every row.
    #[test]
    fn d45_a_due_base_waits_for_the_closing_segment() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.db");
        let blobs = Arc::new(SimBlobs::new(64 * 1024, 1_000_000, 1 << 30));
        let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS));
        let read = Arc::clone(&now);
        let archive = Archive {
            blobs: Arc::clone(&blobs) as Arc<dyn Blobs>,
            prefix: "db".into(),
            clock: Arc::new(move || read.load(Ordering::SeqCst)),
            memory: 32 * 1024,
            patience: Duration::from_secs(60),
            chunks: ChunkGrowth::Keep,
            check: CheckPolicy::Off,
        };
        let mut db = Db::open(&path).unwrap();
        let mut prepared = Prepared::on(&mut db, &path, archive, until()).unwrap();
        db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")
            .unwrap();
        let put = |db: &mut Db, from: i64, rows: i64| {
            for id in from..from + rows {
                now.fetch_add(SECOND_NS, Ordering::SeqCst);
                db.exec(&format!(
                    "INSERT INTO t (id, v) VALUES ({id}, '{}')",
                    "z".repeat(500)
                ))
                .unwrap();
            }
        };
        let mut copy = None;
        put(&mut db, 0, 10);
        let mut ended = 0;
        while ended == 0 {
            ended += usize::from(prepared.round(&mut copy).unwrap().1);
        }
        let mut rows = 10;
        while !prepared.shipper.base_due(None) {
            put(&mut db, rows, 10);
            rows += 10;
            prepared.round(&mut copy).unwrap();
            assert!(copy.is_none(), "due, it waits for the log to start again");
        }

        let slow = db.reader().unwrap();
        slow.exec("BEGIN").unwrap();
        let held = ids(&slow);
        put(&mut db, rows, 20);
        prepared.round(&mut copy).unwrap();
        assert!(db.restart_log().unwrap(), "shipped, so it starts again");
        put(&mut db, rows + 20, 1);
        prepared.round(&mut copy).unwrap();
        assert!(copy.is_none(), "the read keeps the closing segment back");
        assert_eq!(ids(&slow), held, "the read is under way");
        slow.exec("COMMIT").unwrap();
        put(&mut db, rows + 21, 1);
        prepared.round(&mut copy).unwrap();
        assert!(copy.is_some(), "the base that waited begins");
        while !prepared.round(&mut copy).unwrap().1 {}
        now.fetch_add(SECOND_NS, Ordering::SeqCst);
        prepared.round(&mut copy).unwrap();

        let second = now.load(Ordering::SeqCst) / SECOND_NS - 1;
        let target = dir.path().join("restored.db");
        restore(blobs.as_ref(), "db", second, &target, until()).unwrap();
        assert_eq!(ids(&Db::open(&target).unwrap()), ids(&db));
    }

    /// Decision 45, AC-T1.12.3 (lane B24): a round within the writer's restart, its switch made and
    /// the restart not ended, ends the base under way. Do: begin a first base; write 5 rows and ship
    /// them; start the log again, and within the restart run the round that copies the base's last
    /// chunk, as the shipping thread may; restore the last commit's second. Expect: the base ended,
    /// so the second restores with every row.
    #[test]
    fn d45_a_round_within_the_writers_switch_ends_the_base_under_way() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = Arc::new(SimBlobs::new(64 * 1024, 1_000_000, 1 << 30));
        let (mut db, mut prepared, now) = backed_up(
            &dir.path().join("source.db"),
            &blobs,
            None,
            CheckPolicy::Off,
        );
        write(&db, &now, 0, 10);
        let mut copy = None;
        prepared.round(&mut copy).unwrap();
        assert!(copy.is_some(), "the first base begins");
        write(&db, &now, 10, 5);
        prepared.ship().unwrap();
        let hooks = prepared.hooks();
        let thread = Arc::new(Mutex::new((prepared, copy, Vec::new())));
        let within = Arc::clone(&thread);
        *hooks.fence.during_switch.lock().unwrap() = Some(Box::new(move || {
            let mut thread = within.lock().unwrap();
            let (prepared, copy, rounds) = &mut *thread;
            // As the thread's loop does: a round that fails starts the base over.
            let round = prepared.round(copy);
            if round.is_err() {
                *copy = None;
            }
            rounds.push(round.map(|(_, ended)| ended).map_err(|e| format!("{e:#}")));
        }));
        assert!(db.restart_log().unwrap(), "shipped, so it starts again");
        let (_, copy, rounds) = &*thread.lock().unwrap();
        assert_eq!(rounds.len(), 1, "a round ran within the switch");

        let second = now.load(Ordering::SeqCst) / SECOND_NS;
        let target = dir.path().join("restored.db");
        restore(blobs.as_ref(), "db", second, &target, until()).unwrap_or_else(|e| {
            panic!(
                "second {second} refused: {e:#}; the round within the switch: {rounds:?}; a base \
                 under way after it: {}",
                copy.is_some()
            )
        });
        assert_eq!(ids(&Db::open(&target).unwrap()), ids(&db));
    }

    /// Runs the thread's rounds while a base is under way: whether one ended.
    fn rounds(prepared: &mut Prepared, copy: &mut Option<BaseCopy>) -> bool {
        let mut ended = false;
        loop {
            ended |= prepared.round(copy).unwrap().1;
            if copy.is_none() {
                return ended;
            }
        }
    }

    /// The rows of the last whole second the clock has passed, restored.
    fn restored_now(dir: &Path, blobs: &SimBlobs, now: &AtomicU64) -> anyhow::Result<Vec<i64>> {
        let second = now.load(Ordering::SeqCst) / SECOND_NS - 1;
        let target = dir.join(format!("at-{second}.db"));
        restore(blobs, "db", second, &target, until())?;
        Ok(ids(&Db::open(&target)?))
    }

    /// AC-T1.12.3 (lane B25): a base begun before the log broke mends nothing: no restore takes it.
    /// Do: begin the first base; with its copy under way and no round run, write past what a restart
    /// keeps and start the log again, unshipped (the log breaks); commit, and run the thread's
    /// rounds until the base under way ends; commit again, as wakes the thread, and run its rounds;
    /// restore the last commit's second. Expect: it restores with every row.
    #[test]
    fn a_base_begun_before_the_log_broke_is_begun_again() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = Arc::new(SimBlobs::new(64 * 1024, 1_000_000, 1 << 30));
        let (mut db, mut prepared, now) = backed_up(
            &dir.path().join("source.db"),
            &blobs,
            Some(20),
            CheckPolicy::Off,
        );
        write(&db, &now, 0, 10);
        let mut copy = None;
        prepared.round(&mut copy).unwrap();
        assert!(copy.is_some(), "the first base begins");
        write(&db, &now, 10, 60);
        assert!(
            db.restart_log().unwrap(),
            "past what a restart keeps, it starts again unshipped"
        );
        write(&db, &now, 70, 1);
        assert!(rounds(&mut prepared, &mut copy), "a base ended");
        write(&db, &now, 71, 1);
        rounds(&mut prepared, &mut copy);
        now.fetch_add(SECOND_NS, Ordering::SeqCst);
        prepared.round(&mut copy).unwrap();

        match restored_now(dir.path(), &blobs, &now) {
            Ok(restored) => assert_eq!(restored, ids(&db)),
            Err(e) => panic!(
                "the last commit's second is refused, a base ended and another needed: {}: {e:#}",
                prepared.shipper.base_needed()
            ),
        }
    }

    /// AC-T1.12.3 (lane B25): an archive whose latest base was begun before its log broke, as a
    /// shipper before this lane ended one, has no base a restore takes after the break: a shipper
    /// opened on it begins one. Do: end a base; write past what a restart keeps and start the log
    /// again unshipped; ship the generation after and put there a base begun in the one before and
    /// ended in it; open the backup again, commit, and run the thread's rounds; restore the last
    /// commit's second. Expect: it restores with every row.
    #[test]
    fn a_shipper_opened_on_a_base_begun_before_a_break_begins_another() {
        use crate::backup::object;
        use crate::backup::plan::BaseKey;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.db");
        let blobs = Arc::new(SimBlobs::new(64 * 1024, 1_000_000, 1 << 30));
        let (mut db, mut prepared, now) = backed_up(&path, &blobs, Some(20), CheckPolicy::Off);
        write(&db, &now, 0, 10);
        let mut copy = None;
        assert!(rounds(&mut prepared, &mut copy), "the first base ends");
        write(&db, &now, 10, 60);
        assert!(
            db.restart_log().unwrap(),
            "past what a restart keeps, it starts again unshipped"
        );
        write(&db, &now, 70, 1);
        prepared.ship().unwrap();
        let listed = blobs.list("db/base/", None, 10, until()).unwrap().objects;
        let (key, _) = listed.first().unwrap();
        let first = object::manifest(key, &blobs.get(key, until()).unwrap())
            .unwrap()
            .unwrap();
        let useless = object::Manifest {
            id: first.id + 1,
            end: (2, prepared.shipper.frames_in_generation()),
            from: object::runs(&first.chunk_bases().unwrap()),
            ..first
        };
        let key = BaseKey {
            end: useless.end,
            start_gen: useless.start_gen,
            id: useless.id,
        };
        blobs
            .create(&key.key("db"), &object::encode(&useless).unwrap(), until())
            .unwrap();
        drop(prepared);

        let reopened = archive(&blobs, &now, CheckPolicy::Off);
        let mut prepared = Prepared::on(&mut db, &path, reopened, until()).unwrap();
        now.fetch_add(SECOND_NS, Ordering::SeqCst);
        write(&db, &now, 71, 1);
        rounds(&mut prepared, &mut copy);
        now.fetch_add(SECOND_NS, Ordering::SeqCst);
        prepared.round(&mut copy).unwrap();
        match restored_now(dir.path(), &blobs, &now) {
            Ok(restored) => assert_eq!(restored, ids(&db)),
            Err(e) => panic!(
                "the last commit's second is refused, another base needed: {}: {e:#}",
                prepared.shipper.base_needed()
            ),
        }
    }

    /// AC-T1.12.4 (lane B26): a check that finds the archive lacking gets one base for it, and another
    /// only where that one lacks too and a commit moved the log since: a check that fails again and
    /// again never keeps a base copying for nothing. Do: end a first base of 400 rows; ask for one
    /// for frames of the generation shipped now and run rounds; ask again for the same frames, for
    /// frames of the generation after, which the base begun began in, and for that base's proof; change a
    /// row over two generations, ask for its proof again, and run rounds; restore the last second.
    /// Expect: a base ends at the first ask, in a generation of its own with the log shipped again,
    /// and at the last, keeping no chunk; none in between; the second restores with every row.
    #[test]
    fn a_check_that_finds_the_archive_lacking_gets_one_base_until_a_commit() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = Arc::new(SimBlobs::new(64 * 1024, 1_000_000, 1 << 30));
        let (mut db, mut prepared, now) = backed_up(
            &dir.path().join("source.db"),
            &blobs,
            None,
            CheckPolicy::Off,
        );
        write(&db, &now, 0, 400);
        let mut copy = None;
        assert!(rounds(&mut prepared, &mut copy), "the first base ends");
        let generation = prepared.shipper.generation();
        prepared.mends.ask(Lacks::Frames { generation });
        assert!(rounds(&mut prepared, &mut copy), "a base for the frames");
        assert_eq!(prepared.shipper.generation(), generation + 1);
        let id = prepared.shipper.latest_base().unwrap();
        for lacks in [
            Lacks::Frames { generation },
            Lacks::Frames {
                generation: generation + 1,
            },
            Lacks::Base { id },
        ] {
            prepared.mends.ask(lacks);
            assert!(!rounds(&mut prepared, &mut copy), "{lacks:?}, no commit");
        }
        let change = |db: &Db, v: &str| {
            now.fetch_add(SECOND_NS, Ordering::SeqCst);
            db.exec(&format!("UPDATE t SET v = '{v}' WHERE id = 5"))
                .unwrap();
        };
        change(&db, "a");
        prepared.ship().unwrap();
        assert!(db.restart_log().unwrap(), "shipped, so it starts again");
        change(&db, "b");
        prepared.mends.ask(Lacks::Base { id });
        assert!(rounds(&mut prepared, &mut copy), "a base for the proof");
        let whole = prepared.shipper.latest_base().unwrap();
        assert_ne!(whole, id);
        let listed = blobs.list("db/base/", None, 10, until()).unwrap().objects;
        let (key, _) = listed.last().unwrap();
        let manifest = crate::backup::object::manifest(key, &blobs.get(key, until()).unwrap())
            .unwrap()
            .unwrap();
        assert!(
            manifest.chunks > 4,
            "non-vacuous: {} chunks",
            manifest.chunks
        );
        assert_eq!(
            manifest.from,
            vec![(whole, manifest.chunks)],
            "no chunk of another base's"
        );
        now.fetch_add(SECOND_NS, Ordering::SeqCst);
        prepared.round(&mut copy).unwrap();
        assert_eq!(restored_now(dir.path(), &blobs, &now).unwrap(), ids(&db));
    }

    /// AC-T1.12.4 (lane B26): a check that fails wakes the shipping thread for the base it asks for: no
    /// commit does. Do: end a first base, commit and ship; zero the last frame of the log's
    /// generation in the archive, sealed again; start the threads, whose wakes are the test's to make; once
    /// the check thread took its snapshot, make one, so the round after it checks that second; make no
    /// other.
    /// Expect: the check fails, and a base begun after the short generation ends.
    #[test]
    fn a_check_that_fails_wakes_the_round_that_begins_a_base() {
        use crate::backup::object;
        let dir = tempfile::tempdir().unwrap();
        let blobs = Arc::new(SimBlobs::new(64 * 1024, 1_000_000, 1 << 30));
        let (db, mut prepared, now) = backed_up(
            &dir.path().join("source.db"),
            &blobs,
            None,
            CheckPolicy::Scratch(None),
        );
        write(&db, &now, 0, 10);
        let mut copy = None;
        assert!(rounds(&mut prepared, &mut copy), "the first base ends");
        write(&db, &now, 10, 1);
        prepared.ship().unwrap();
        let generation = prepared.shipper.generation();
        let listed = blobs.list("db/log/", None, 100, until()).unwrap().objects;
        let (key, _) = listed.last().unwrap();
        let sealed = blobs.get(key, until()).unwrap();
        let mut segment: object::Segment =
            postcard::from_bytes(object::opened(key, &sealed).unwrap()).unwrap();
        let tail = segment.frames.len() - usize::try_from(FRAME_BYTES).unwrap();
        segment.frames[tail..].fill(0);
        blobs.delete(key, until()).unwrap();
        blobs
            .create(key, &object::encode(&segment).unwrap(), until())
            .unwrap();
        now.fetch_add(2 * SECOND_NS, Ordering::SeqCst);

        let hooks = prepared.hooks();
        let mut wake = None;
        let shipping = prepared.start(|watcher| wake = Some(watcher)).unwrap();
        let deadline = until();
        while hooks.marks.state().unwrap().watch.is_none() {
            assert!(Instant::now() < deadline, "no snapshot was taken");
            std::thread::yield_now();
        }
        assert!(wake.as_ref().unwrap()(), "the thread runs");
        let failed = |s: &Status| match &s.last_check {
            Some(Check::Failed {
                why: CheckFailure::Lacking { lacks, .. },
                ..
            }) => *lacks == Lacks::Frames { generation },
            _ => false,
        };
        shipping
            .wait(until(), |s| s.bases > 0 && failed(s))
            .unwrap_or_else(|e| panic!("no base ended: {e:#}; {:?}", shipping.status()));
        let bases = blobs.list("db/base/", None, 10, until()).unwrap().objects;
        let (newest, _) = bases.last().unwrap();
        let newest = plan::BaseKey::parse("db", newest).unwrap();
        assert_eq!(newest.start_gen, generation + 1);
        drop(wake);
        shipping.wait(until(), |s| s.ended).unwrap();
    }

    /// The most bytes the files in a restore's folder held together, seen at each of its steps.
    struct Peak {
        dir: PathBuf,
        bytes: u64,
    }

    impl Peak {
        fn see(&mut self) -> anyhow::Result<()> {
            let mut held = 0;
            for entry in std::fs::read_dir(&self.dir)? {
                held += entry?.metadata()?.len();
            }
            self.bytes = self.bytes.max(held);
            Ok(())
        }
    }

    impl Pace for Peak {
        fn ready(&mut self) -> anyhow::Result<()> {
            self.see()
        }
        fn done(&mut self, _: Option<&std::fs::File>, _: u64) -> anyhow::Result<()> {
            self.see()
        }
    }

    /// Decision 45, T-T1.12.C5 (lane B24): a check begins only where the disk holds its restore and
    /// the log's growth, both segments counted. Do: end a first base; start the log again (the
    /// writer switches to `-wal2`) and write 300 rows there, shipped; take the room a check is let
    /// begin with; run its restore, the room it takes measured at each step; then, no round running
    /// (the store answers nothing), let the writer commit and start the log again past its bound in
    /// each segment. Expect: the restore takes no more than the room counted for it, and the log
    /// grows by no more than the room left it: the disk never fills.
    #[test]
    fn d45_a_check_begins_only_where_the_disk_holds_both_of_the_logs_segments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.db");
        let blobs = Arc::new(SimBlobs::new(64 * 1024, 1_000_000, 1 << 30));
        let kept = 800;
        let (mut db, mut prepared, now) =
            backed_up(&path, &blobs, Some(kept), CheckPolicy::Continuous);
        write(&db, &now, 0, 10);
        let mut copy = None;
        while !prepared.round(&mut copy).unwrap().1 {}
        assert!(db.restart_log().unwrap(), "shipped, so it starts again");
        write(&db, &now, 10, 300);
        now.fetch_add(SECOND_NS, Ordering::SeqCst);
        prepared.round(&mut copy).unwrap();
        let logs = || {
            let first = file_len(&suffixed(&path, "-wal")).unwrap();
            let second = file_len(&suffixed(&path, "-wal2")).unwrap();
            (first, second)
        };
        let (first, second) = logs();
        assert!(
            second > 300 * FRAME_BYTES && first < 100 * FRAME_BYTES,
            "non-vacuous: the log is in `-wal2` ({second} bytes; `-wal` {first})"
        );

        let budget = prepared.checker.as_ref().unwrap().budget().unwrap();
        let scratch = dir.path().join("scratch");
        std::fs::create_dir(&scratch).unwrap();
        let mut peak = Peak {
            dir: scratch.clone(),
            bytes: 0,
        };
        let at = now.load(Ordering::SeqCst) / SECOND_NS - 1;
        restore_paced(
            blobs.as_ref(),
            "db",
            at,
            &scratch.join("restored.db"),
            &mut peak,
            until(),
        )
        .unwrap();
        assert_eq!(
            ids(&Db::open(&scratch.join("restored.db")).unwrap()).len(),
            310
        );
        assert!(
            peak.bytes <= budget.restore,
            "the restore took {} bytes of the disk, the check begins where {} are free for it: file \
             {}, `-wal` {first}, `-wal2` {second}",
            peak.bytes,
            budget.restore,
            file_len(&path).unwrap()
        );

        let mut most = 0;
        let mut restarts = 0;
        let mut id = 1000;
        while restarts < 2 {
            write(&db, &now, id, 1);
            id += 1;
            let (first, second) = logs();
            most = most.max(first + second);
            restarts += u64::from(db.restart_log().unwrap());
        }
        let grew = most.saturating_sub(first + second);
        assert!(
            grew <= budget.log_headroom,
            "the log grew by {grew} bytes, the check leaves it {}",
            budget.log_headroom
        );
    }
}
