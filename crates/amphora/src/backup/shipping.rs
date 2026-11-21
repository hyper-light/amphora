//! The thread that backs a [`crate::database::Database`]'s local writer up (CP-T1.12;
//! docs/design/backup.md, "The database's shipping thread").
//!
//! It wakes on each commit the store makes, never on a timer: a round ships what became durable
//! while the last ran, then copies one chunk of a base while one is due, so its cadence follows
//! the log's rate and the store's put time. It ships through a reader on the writer's engine, so
//! the writer never waits on the store. When the database ends it ships once more and ends.

use std::sync::mpsc::{self, Receiver, TryRecvError, TrySendError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use amphora_api::blobs::Blobs;

use super::{BaseCopy, Clock, Hooks, Shipped, Shipper};
use crate::db::Db;
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
    /// The memory a base's chunk may take ([`Shipper::begin_base`]).
    pub memory: usize,
    /// How long one request to the store may take: past it the round is given up and tried again
    /// at the next commit. The caller states it, as the store's own answer time is the caller's
    /// to know (Needs Ada's word: no measured default yet).
    pub patience: Duration,
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
}

/// A handle on a database's shipping thread: what it has done, and waits on it.
#[derive(Clone)]
pub struct Shipping {
    status: Arc<(Mutex<Status>, Condvar)>,
}

impl Shipping {
    fn locked(&self) -> anyhow::Result<MutexGuard<'_, Status>> {
        self.status
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("the backup's status lock was poisoned"))
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

/// A backup made ready on the writer as it opens, its thread not started yet.
pub(crate) struct Prepared {
    archive: Archive,
    shipper: Shipper,
    reader: Db,
}

impl Prepared {
    /// Backs `db`, the writer, up to `archive`: its commits stamped from now on, its log started
    /// again only once shipped ([`Db::back_up`]), and a shipper carrying on from what the archive
    /// holds, on a reader of the same engine.
    pub(crate) fn on(db: &mut Db, archive: Archive, until: Instant) -> anyhow::Result<Self> {
        let hooks = Hooks::new(Arc::clone(&archive.clock), db.log_kept_frames());
        db.back_up(Arc::clone(&hooks));
        let reader = db.reader()?;
        // The shipping reader may be the engine's last connection to close: its close must not
        // empty a log the backup hasn't shipped either.
        reader.hold_log();
        let shipper = Shipper::open(
            &archive.prefix,
            hooks,
            archive.blobs.as_ref(),
            &reader,
            until,
        )?;
        Ok(Self {
            archive,
            shipper,
            reader,
        })
    }

    /// Starts the thread, woken by each commit `on_commit` reports. It ends once the store drops
    /// its watchers, as the database ends.
    pub(crate) fn start(self, on_commit: impl FnOnce(Watcher)) -> anyhow::Result<Shipping> {
        // One wake pending at most: a commit while one waits is shipped by the same round.
        let (wake, woken) = mpsc::sync_channel::<()>(1);
        // The first round runs at once: what the database committed as it opened is shipped, and
        // its first base begins, without waiting for a commit.
        wake.try_send(())?;
        on_commit(Box::new(move || {
            !matches!(wake.try_send(()), Err(TrySendError::Disconnected(())))
        }));
        let shipping = Shipping {
            status: Arc::default(),
        };
        let told = shipping.clone();
        std::thread::Builder::new()
            .name("amphora-backup".into())
            .spawn(move || self.run(&woken, &told))?;
        Ok(shipping)
    }

    fn run(mut self, woken: &Receiver<()>, shipping: &Shipping) {
        let mut copy: Option<BaseCopy> = None;
        loop {
            match self.round(&mut copy) {
                Ok((shipped, base_ended)) => shipping.record(|s| {
                    s.rounds = s.rounds.saturating_add(1);
                    s.shipped.segments = s.shipped.segments.saturating_add(shipped.segments);
                    s.shipped.bytes = s.shipped.bytes.saturating_add(shipped.bytes);
                    if base_ended {
                        s.bases = s.bases.saturating_add(1);
                    }
                }),
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
        // The database ended: what it committed last is shipped before the thread lets go of it.
        let last = self.ship();
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
        let shipped = self.ship()?;
        let blobs = self.archive.blobs.as_ref();
        let until = self.until();
        match copy.as_mut() {
            None => {
                // A base begins in a generation the shipper has shipped: after a round that
                // shipped, or once there's nothing left to (`begin_base` refuses otherwise).
                if self.shipper.base_due(None) && self.shipper.ships_current(&self.reader) {
                    *copy = Some(self.shipper.begin_base(
                        &self.reader,
                        blobs,
                        self.archive.memory,
                    )?);
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
