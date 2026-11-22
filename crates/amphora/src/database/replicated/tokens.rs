//! Session tokens: a session reads its own writes on any replica (replication.md, "Decision (lane
//! ryw-1): read-your-writes by session tokens"; kiln goal 8119f25d, AC-T1.10.R1 to R4).
//!
//! A write's acknowledgement gives its session a [`Token`]: the point of its own entry in the
//! group's committed log, so a session waits for its own writes and no one else's. A read that carries one is served by any replica whose applied
//! point is at or past it; it waits on that replica alone, only while the replica is behind, and
//! only up to the replica's measured apply lag, past which it is redirected to the leader's read.
//! A read with no token is a local read, which never waits for the log.

use super::*;

/// Where a session's writes stand: what a read must see to read them.
///
/// `micros` is the group's time (replication.md, "Follower reads at a closed time"): microseconds
/// since the Unix epoch, rising strictly along the committed log, the same on every member. It is
/// the domain of the cross-group coordinator's timestamps, so a single-group commit and a
/// cross-group one compare in one order. `index` is the commit index in this database's group
/// (0 for a token from another group's time alone). Within one group the two rise together, so
/// the derived order (time first) is the order of the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub struct Token {
    pub micros: u64,
    pub index: u64,
}

impl Token {
    /// A token at a time alone: a cross-group commit's timestamp, or a time a session has seen.
    #[must_use]
    pub fn at(micros: u64) -> Self {
        Self { micros, index: 0 }
    }
    /// The later of two tokens a session holds, field by field: what reads both.
    #[must_use]
    pub fn after(self, other: Self) -> Self {
        Self {
            micros: self.micros.max(other.micros),
            index: self.index.max(other.index),
        }
    }
    /// Whether a replica at `point` has applied everything this token names.
    fn served_by(self, point: &Closed) -> bool {
        point.applied >= self.index && point.time.unwrap_or(0) >= self.micros
    }
}

/// The read a session asks for (the design switch, kiln goal 8119f25d; Ada's order 2026-11-22).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReadMode {
    /// The control, today's read: every read waits for the leader's latest commit (its
    /// ReadIndex), token or not.
    #[default]
    Latest,
    /// Session tokens: a read with a token waits only for its token, on the replica it asked;
    /// one without a token never waits.
    Session,
}

/// What a token read on one replica came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Served<R> {
    /// Answered here.
    Here(R),
    /// This replica stayed behind the token past its measured apply lag, or has measured none:
    /// the read goes to the leader.
    Redirect,
}

/// A replica's token reads, counted (`SessionCounters`).
#[derive(Default)]
pub(super) struct SessionMeter {
    at_once: AtomicU64,
    waited: AtomicU64,
    wait_nanos: AtomicU64,
    redirects: AtomicU64,
}

/// A replica's token reads: those served at once, those that waited and the nanoseconds they
/// waited in all, those redirected; and the apply lag it measured (the p99 of its closed time's
/// distance behind its clock, microseconds), which bounds a wait.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionCounters {
    pub at_once: u64,
    pub waited: u64,
    pub wait_nanos: u64,
    pub redirects: u64,
    pub apply_lag_micros: Option<u64>,
}

impl Client {
    /// This replica's closed time in microseconds (0 before the group's first stamped entry): the
    /// replicated group's clock on the one cross-group clock ([`crate::Database::cross_clock`]).
    pub(in crate::database) fn closed_micros(&self) -> Result<u64, Refusal> {
        Ok(locked(&self.handle.shared.closed)?.time.unwrap_or(0))
    }
    /// `read` on a local reader once this replica has applied `token`; `read` answers `Err` with
    /// the time it found if its own transaction's state is earlier than the token's time, and the
    /// next try waits for a later one. Redirected once the replica stays behind past its
    /// measured apply lag from the read's start, or at once if it has measured none while behind.
    /// Refused Timeout at `until`, and Fenced once the owner ended or is fenced.
    pub(in crate::database) fn read_session<R>(
        &self,
        token: Token,
        until: Instant,
        mut read: impl FnMut(&mut Db) -> anyhow::Result<Result<R, Option<u64>>>,
    ) -> Result<Served<R>, Refusal> {
        let sessions = &self.handle.shared.sessions;
        let began = Instant::now();
        let mut seen = None;
        let mut waited = false;
        loop {
            match self.wait_token(token, seen, began, until)? {
                Wait::Redirect => {
                    sessions.redirects.fetch_add(1, Ordering::Relaxed);
                    return Ok(Served::Redirect);
                }
                Wait::Waited => waited = true,
                Wait::Ready => {}
            }
            match self.read_local(until, &mut read)? {
                Ok(answer) => {
                    if waited {
                        let nanos = u64::try_from(began.elapsed().as_nanos()).unwrap_or(u64::MAX);
                        sessions.waited.fetch_add(1, Ordering::Relaxed);
                        sessions.wait_nanos.fetch_add(nanos, Ordering::Relaxed);
                    } else {
                        sessions.at_once.fetch_add(1, Ordering::Relaxed);
                    }
                    return Ok(Served::Here(answer));
                }
                Err(found) => seen = found.or(Some(0)),
            }
        }
    }
    /// Waits, on the owner's publication (never a timer), until this replica has applied `token`
    /// and, after a try that found an earlier state, a later time than `seen`.
    fn wait_token(
        &self,
        token: Token,
        seen: Option<u64>,
        began: Instant,
        until: Instant,
    ) -> Result<Wait, Refusal> {
        let shared = &self.handle.shared;
        let mut point = locked(&shared.closed)?;
        let mut waited = false;
        loop {
            if locked(&shared.local.open)?.is_none() {
                shared.meter.local_refused(LocalRefusal::Fenced);
                return Err(fenced("replica owner ended"));
            }
            match &*locked(&shared.status)? {
                Err(ended) => {
                    shared.meter.local_refused(LocalRefusal::Fenced);
                    return Err(ended.clone());
                }
                Ok(status) => {
                    if let Some(reason) = &status.fenced {
                        shared.meter.local_refused(LocalRefusal::Fenced);
                        return Err(fenced(reason));
                    }
                }
            }
            let later = seen.is_none_or(|seen| point.time.is_some_and(|time| time > seen));
            if token.served_by(&point) && later {
                return Ok(if waited { Wait::Waited } else { Wait::Ready });
            }
            // The bound is this replica's own measured apply lag: a replica further behind than
            // that is slower than it has been 99 times in 100, and the leader answers sooner.
            let Some(lag) = point.lag else {
                return Ok(Wait::Redirect);
            };
            let bound = began
                .checked_add(Duration::from_micros(lag))
                .unwrap_or(until);
            let now = Instant::now();
            if now >= until {
                shared.meter.local_refused(LocalRefusal::Timeout);
                return Err(Refusal::Timeout);
            }
            if now >= bound {
                return Ok(Wait::Redirect);
            }
            waited = true;
            point = shared
                .moved
                .wait_timeout(point, bound.min(until).saturating_duration_since(now))
                .map_err(|_| unavailable("the closed time's lock was poisoned"))?
                .0;
        }
    }
    /// This replica's token reads, counted, and its measured apply lag.
    pub(in crate::database) fn session_counters(&self) -> Result<SessionCounters, Refusal> {
        let sessions = &self.handle.shared.sessions;
        Ok(SessionCounters {
            at_once: sessions.at_once.load(Ordering::Relaxed),
            waited: sessions.waited.load(Ordering::Relaxed),
            wait_nanos: sessions.wait_nanos.load(Ordering::Relaxed),
            redirects: sessions.redirects.load(Ordering::Relaxed),
            apply_lag_micros: locked(&self.handle.shared.closed)?.lag,
        })
    }
}

/// How a token's wait ended.
enum Wait {
    Ready,
    Waited,
    Redirect,
}
