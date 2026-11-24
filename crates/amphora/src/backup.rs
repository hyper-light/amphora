//! Backups and point-in-time recovery to Mantle (CP-T1.12, AC-T1.12.3, AC-T1.12.4;
//! docs/design/backup.md). A database's log is shipped to an object store ([`amphora_api::blobs`],
//! Mantle behind it) one second of commits to an object, as the second ends; a base is a copy of
//! its file, made consistent by the log shipped beside it, as PostgreSQL's base backups are; a
//! restore writes a base and replays the log after it, through the engine's own recovery, up to the
//! last commit of the second asked for.
//!
//! - [`Hooks`]: what a database's writer tells the backup: when each commit was made ([`Marks`])
//!   and whether its log may start again ([`Fence`]): a log only starts again once what it holds is
//!   shipped, until it reaches the most it keeps.
//! - [`plan`]: the pure core: the archive's keys, which objects restore a second, which expire.
//! - [`Shipper`] and [`BaseCopy`]: the shell that ships the log and copies bases.
//! - [`Shipping`]: the thread that ships a database's log and copies its bases as it commits
//!   ([`crate::database::Database::open_backed_up`]).
//! - [`restore()`]: the shell that restores a second.
//! - [`restore_and_check`]: restores the newest whole second into a scratch file and checks it.
//! - [`CheckPolicy`]: whether the shipping thread checks its archive: by default continuously, at
//!   seconds drawn at random ([`Drills`]), within a budget derived from the machine ([`admit`]).
//! - [`restore_cut`]: several groups restored to one second, cross-group transactions finished.
//! - [`FolderBlobs`]: the store Amphora owns: a folder, local or a peer's over a mount.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

mod check;
mod cut;
mod drill;
mod folder;
mod object;
pub mod plan;
mod restore;
mod ship;
mod shipping;

pub use check::{
    CHECK_METRIC, Check, CheckCounters, CheckFailure, RowDigest, check_due, digest, digest_paced,
    newest_second, restore_and_check, restore_and_check_at,
};
pub use cut::restore_cut;
pub use drill::{
    Budget, COST_METRIC, CheckPolicy, DEFER_METRIC, Deferred, DrillMeter, Drills, Foreground,
    InFlight, Pace, Pacer, Stopped, Unpaced, Yield, admit, behind, device_ns, next_random,
};
pub use folder::FolderBlobs;
pub use restore::{Restored, expire, restore, restore_paced, restore_within};
pub use ship::{BaseCopy, ChunkGrowth, Shipped, Shipper};
pub(crate) use shipping::Prepared;
pub use shipping::{Archive, Shipping, Status, system_clock};

/// Nanoseconds since the Unix epoch, from the clock the caller gives (the system's in production, a
/// simulated one in tests).
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// The nanoseconds of a second: what "any second" is counted in (AC-T1.12.1).
pub const SECOND_NS: u64 = 1_000_000_000;

/// The latest stamp in microseconds `state` has made or seen: its own, or a nanosecond stamp read
/// up to the microsecond it ends.
fn latest_micros(state: &MarkState) -> u64 {
    state.micros.max(state.last.div_ceil(MICRO_NS))
}

/// The nanoseconds of a microsecond: the cross-group clock's unit, as [`Marks::next_micros`] reads
/// this one (a unit, not a tuned number).
pub const MICRO_NS: u64 = 1_000;

/// The second a stamp falls in, counted as restore targets are: a commit stamped in
/// `(s - 1, s]` seconds is in second `s`, so restoring to `s` keeps it.
#[must_use]
pub fn second_of(ns: u64) -> u64 {
    ns.div_ceil(SECOND_NS)
}

/// The last commit of a second the writer made, where its log held it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    /// The log's generation (its header's checkpoint sequence).
    pub seq: u32,
    /// The commit's last frame.
    pub frame: u64,
    /// When it was committed.
    pub ns: u64,
    /// Set when marks were merged to stay within their bound: the commits up to `frame` were made
    /// after this and at or before `ns`, but which of them by which second isn't known.
    pub unknown_from: Option<u64>,
}

#[derive(Default)]
struct MarkState {
    marks: VecDeque<Mark>,
    /// The latest stamp or drain: the next is later (a clock that steps back is held at it).
    last: u64,
    /// The latest stamp in microseconds made or seen ([`Marks::next_micros`]): the next is later.
    micros: u64,
    held_back: u64,
    merged: u64,
    /// The newest mark drained: a statement that commits nothing after it makes none.
    drained: Option<(u32, u64)>,
    /// The newest commit stamped: its generation, last frame and stamp.
    stamped: Option<(u32, u64, u64)>,
    /// The second a quiet snapshot holds ([`Marks::quiet_second`]), and whether a commit was
    /// stamped in it since.
    watch: Option<(u64, bool)>,
}

/// When each commit was made: the last commit of each second, in commit order, until the shipper
/// takes them. One mark a second, at most `capacity`; past it the two oldest merge, the second
/// between them then refused as unknown rather than restored wrong (CLAUDE.md §2, §3).
pub struct Marks {
    state: Mutex<MarkState>,
    capacity: usize,
    clock: Clock,
}

/// What the shipper takes from [`Marks`]: the marks, and the time every commit stamped at or
/// before is among them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drained {
    pub marks: Vec<Mark>,
    pub through: u64,
}

impl Marks {
    fn state(&self) -> anyhow::Result<MutexGuard<'_, MarkState>> {
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("the backup's marks lock was poisoned"))
    }

    /// A stamp later than every one before: a clock that steps back is held at the last.
    fn stamp(&self, state: &mut MarkState) -> u64 {
        let now = (self.clock)();
        let next = state.last.saturating_add(1);
        let ns = if now < next {
            state.held_back = state.held_back.saturating_add(1);
            next
        } else {
            now
        };
        state.last = ns;
        ns
    }

    /// A commit ended at `frame` of generation `seq`: stamped now, under the lock the shipper
    /// drains by, so a drain at `t` has every commit stamped at or before `t`.
    pub fn committed(&self, seq: u32, frame: u64) -> anyhow::Result<()> {
        let mut state = self.state()?;
        // A statement that committed nothing (a read, a BEGIN) leaves the log where it was.
        if state
            .marks
            .back()
            .is_some_and(|last| last.seq == seq && last.frame >= frame)
            || state.drained.is_some_and(|(s, f)| s == seq && f >= frame)
        {
            return Ok(());
        }
        let ns = self.stamp(&mut state);
        self.keep_commit(&mut state, seq, frame, ns);
        Ok(())
    }

    /// Publish a lock step and its reply stamp together (T-T1.9.R28/R30). The step has
    /// finished taking its locks. A reader that sampled the clock before us has a smaller
    /// stamp. A reader sampling it after us can already see the lock in the committed WAL.
    pub(crate) fn publish_lock(
        &self,
        commit: impl FnOnce(u64) -> anyhow::Result<(u32, u64)>,
    ) -> anyhow::Result<()> {
        let mut state = self.state()?;
        let at = self.next_locked(&mut state);
        let (seq, frame) = commit(at)?;
        let ns = state.last;
        self.keep_commit(&mut state, seq, frame, ns);
        Ok(())
    }

    fn keep_commit(&self, state: &mut MarkState, seq: u32, frame: u64, ns: u64) {
        if let Some((second, moved)) = state.watch.as_mut()
            && *second == second_of(ns)
        {
            *moved = true;
        }
        if let Some(last) = state.marks.back_mut()
            && last.seq == seq
            && second_of(last.ns) == second_of(ns)
        {
            last.frame = last.frame.max(frame);
            last.ns = ns;
            let stamped = (seq, last.frame, ns);
            state.stamped = Some(stamped);
            return;
        }
        state.stamped = Some((seq, frame, ns));
        if state.marks.len() >= self.capacity
            && let Some(oldest) = state.marks.pop_front()
            && let Some(next) = state.marks.front_mut()
        {
            // Merging marks of two generations would claim a frame of one for the other: the
            // older is dropped then, its second unknown as a merged one's is.
            let from = oldest.unknown_from.unwrap_or(oldest.ns);
            next.unknown_from = Some(next.unknown_from.map_or(from, |f| f.min(from)));
            state.merged = state.merged.saturating_add(1);
        }
        state.marks.push_back(Mark {
            seq,
            frame,
            ns,
            unknown_from: None,
        });
    }

    /// Takes every mark made so far, and the time through which they are all of them.
    pub fn drain(&self) -> anyhow::Result<Drained> {
        let mut state = self.state()?;
        let through = self.stamp(&mut state);
        if let Some(last) = state.marks.back() {
            state.drained = Some((last.seq, last.frame));
        }
        Ok(Drained {
            marks: state.marks.drain(..).collect(),
            through,
        })
    }

    /// Takes in a stamp another group made before a step it sent here (T-T1.12.X2): every commit
    /// stamped here from now on is later, a hybrid logical clock's receive rule (Kulkarni et al.,
    /// "Logical Physical Clocks", OPODIS 2014): a stamp is the larger of the clock's reading and
    /// one past the latest stamp seen, so a commit made after another, by any path of steps, is
    /// stamped after it whatever the two clocks read.
    pub fn observe(&self, ns: u64) -> anyhow::Result<()> {
        let mut state = self.state()?;
        state.last = state.last.max(ns);
        Ok(())
    }

    /// A stamp later than every one made or seen here, under the lock commits are stamped by: every
    /// stamp after it is later still. On the writer, a Records revision's place in the order across
    /// groups as its commit writes it, and the time through which a group has made every revision
    /// it will (lane split-merge-18, T-T1.9.R20); the commit's own mark, made as it ends, is later.
    pub fn next(&self) -> anyhow::Result<u64> {
        let mut state = self.state()?;
        Ok(self.stamp(&mut state))
    }

    /// The latest stamp made or seen here: every commit stamped so far is at or before it.
    pub fn latest(&self) -> anyhow::Result<u64> {
        Ok(self.state()?.last)
    }

    /// [`Marks::next`] on the clock reads across groups and their commits are stamped by (lane
    /// split-merge-21, T-T1.9.R28; Ada's decision of 2026-11-22): the cross-group hybrid logical
    /// clock, in microseconds since the Unix epoch, the unit of the session tokens' times
    /// (`Token::micros`) and of replicated groups' closed times. It is this clock read
    /// a thousandth as finely, each stamp the clock's reading or one past the last microsecond
    /// stamp made or seen, whichever is later: a hybrid logical clock in microseconds, strictly
    /// rising here, kept beside the nanoseconds so a stamp spends none of the backup marks' (their
    /// held-back count stays the clock's own steps back).
    pub fn next_micros(&self) -> anyhow::Result<u64> {
        let mut state = self.state()?;
        Ok(self.next_locked(&mut state))
    }

    fn next_locked(&self, state: &mut MarkState) -> u64 {
        let before = latest_micros(state);
        let micros = self
            .stamp(state)
            .div_ceil(MICRO_NS)
            .max(before.saturating_add(1));
        state.micros = micros;
        micros
    }

    /// The latest stamp in microseconds made or seen here, the backup's nanosecond stamps among
    /// them (so a step's carried stamp covers its commit, T-T1.12.X2): every one
    /// [`Marks::next_micros`] makes from now on is later.
    pub fn latest_micros(&self) -> anyhow::Result<u64> {
        Ok(latest_micros(&*self.state()?))
    }

    /// [`Marks::observe`] of a stamp in microseconds: every stamp here from now on, in either
    /// unit, is past the whole of that microsecond, so [`Marks::next_micros`] answers later.
    pub fn observe_micros(&self, micros: u64) -> anyhow::Result<()> {
        let mut state = self.state()?;
        state.micros = state.micros.max(micros);
        state.last = state.last.max(micros.saturating_mul(MICRO_NS));
        Ok(())
    }

    /// Lines a reader's snapshot up with the commits' stamps (T-T1.12.C3): under the lock commits
    /// are stamped by, `snapshot` begins a read and says where the log ends. When that is the
    /// newest commit stamped, no commit lies between the snapshot and its stamp, so the snapshot
    /// holds every commit stamped at or before it and none after: the rows of its second, once no
    /// commit is stamped later in that second ([`Marks::settled`]). Which second, if it does: none
    /// when the log moved past the newest stamp (a commit not stamped yet, a log started again).
    pub fn quiet_second(
        &self,
        snapshot: impl FnOnce() -> anyhow::Result<(u32, u64)>,
    ) -> anyhow::Result<Option<u64>> {
        let mut state = self.state()?;
        let Some((seq, frame, ns)) = state.stamped else {
            return Ok(None);
        };
        if snapshot()? != (seq, frame) {
            return Ok(None);
        }
        let second = second_of(ns);
        state.watch = Some((second, false));
        Ok(Some(second))
    }

    /// Whether the snapshot [`Marks::quiet_second`] last lined up holds the rows of `second`: yes
    /// once the clock is past the second's end with no commit stamped in it since (every later
    /// commit is stamped later still), no once one was (or a later snapshot took its place), not
    /// yet while the second runs.
    pub fn settled(&self, second: u64) -> anyhow::Result<Option<bool>> {
        let mut state = self.state()?;
        match state.watch {
            Some((s, false)) if s == second => {
                let now = self.stamp(&mut state);
                Ok(if now / SECOND_NS >= second {
                    Some(true)
                } else {
                    None
                })
            }
            _ => Ok(Some(false)),
        }
    }

    /// How often the clock stepped back and was held, and how many marks were merged.
    pub fn counts(&self) -> anyhow::Result<(u64, u64)> {
        let state = self.state()?;
        Ok((state.held_back, state.merged))
    }
}

/// The log's restarts since the last that ended a generation with frames in it (an empty one ends
/// no frames, so is never forced). The engine's restart raises the header's checkpoint sequence by
/// one (`restart_wal_header`, as SQLite's `walRestartHdr`), so the generations they ended run on
/// from `seq` one by one, and a log restarted twice with no write between keeps its link whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ended {
    /// The generation that ended with frames in it, its last frame, and whether it was started
    /// again before all of it was shipped.
    seq: u32,
    frame: u64,
    forced: bool,
    /// How many empty generations ended after it, one after another.
    empties: u32,
}

impl Ended {
    /// The generation the latest restart began.
    fn next(&self) -> u32 {
        self.seq.wrapping_add(self.empties).wrapping_add(1)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct FenceState {
    /// The last frame shipped, of generation `.0`.
    shipped: Option<(u32, u64)>,
    ended: Option<Ended>,
    /// The restart the writer is making: what `ended` is once it is made.
    ending: Option<Ended>,
}

/// What a log restart asks of the backup: whether all the log holds is shipped.
#[derive(Default)]
pub struct Fence {
    state: Mutex<FenceState>,
    /// A test's round, run once within the writer's restart, its switch just made.
    #[cfg(test)]
    during_switch: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Fence {
    fn state(&self) -> anyhow::Result<MutexGuard<'_, FenceState>> {
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("the backup's fence lock was poisoned"))
    }

    pub(crate) fn shipped(&self, seq: u32, frame: u64) -> anyhow::Result<()> {
        self.state()?.shipped = Some((seq, frame));
        Ok(())
    }

    /// Whether the log, at `frame` of generation `seq`, is shipped whole.
    pub(crate) fn holds(&self, seq: u32, frame: u64) -> anyhow::Result<bool> {
        Ok(frame == 0
            || self
                .state()?
                .shipped
                .is_some_and(|(s, f)| s == seq && f >= frame))
    }

    /// The writer's restart of the log, by `switch` (whether it made it), from the end of
    /// generation `seq` at `frame`. The fence is told how the generation ends before `switch` is
    /// made: a round may see the next one the moment it is, before `switch` even returns, and asks
    /// the fence how this one ended ([`Fence::ended_whole`]).
    pub(crate) fn restart(
        &self,
        seq: u32,
        frame: u64,
        forced: bool,
        switch: impl FnOnce() -> anyhow::Result<bool>,
    ) -> anyhow::Result<bool> {
        {
            let mut state = self.state()?;
            state.ending = match state.ended {
                Some(mut e) if frame == 0 && e.next() == seq => {
                    e.empties = e.empties.wrapping_add(1);
                    Some(e)
                }
                _ => Some(Ended {
                    seq,
                    frame,
                    forced,
                    empties: 0,
                }),
            };
        }
        let switched = switch();
        #[cfg(test)]
        if matches!(switched, Ok(true)) {
            self.at_switch();
        }
        let mut state = self.state()?;
        let ending = state.ending.take();
        if matches!(switched, Ok(true)) {
            state.ended = ending;
        }
        switched
    }

    /// The writer switched and its restart hasn't ended: runs what a test put there.
    #[cfg(test)]
    fn at_switch(&self) {
        let during = self.during_switch.lock().unwrap().take();
        if let Some(during) = during {
            during();
        }
    }

    /// Whether the log ran unbroken from frame `frame` of generation `old`, as it ended, to the start
    /// of generation `new`: every restart between them was through [`crate::db::Db::restart_log`]
    /// and none of them forced (T-T1.12.D1).
    pub(crate) fn ended_whole(&self, old: u32, frame: u64, new: u32) -> anyhow::Result<bool> {
        let state = *self.state()?;
        // A log at the generation the restart under way begins says it was made; a log at the one
        // it ends was begun by the restart before.
        let told = [state.ending, state.ended];
        let Some(e) = told.into_iter().flatten().find(|e| e.next() == new) else {
            return Ok(false);
        };
        if old == e.seq {
            return Ok(frame == e.frame && !e.forced);
        }
        // One of the empty generations after it: it ended at frame 0.
        let after = old.wrapping_sub(e.seq);
        Ok(frame == 0 && (1..=e.empties).contains(&after))
    }
}

/// What a database's writer tells its backup ([`crate::db::Db::back_up`]).
pub struct Hooks {
    pub marks: Marks,
    pub fence: Fence,
}

impl Hooks {
    /// Hooks stamping commits by `clock`, keeping at most `capacity` marks between drains: the
    /// frames a log holds at most between restarts (decision 31's [`crate::db::Db::keep_log`]),
    /// since a mark is a commit and a commit at least a frame.
    #[must_use]
    pub fn new(clock: Clock, capacity: u64) -> Arc<Self> {
        Arc::new(Self {
            marks: Marks {
                state: Mutex::new(MarkState::default()),
                capacity: usize::try_from(capacity).unwrap_or(usize::MAX).max(2),
                clock,
            },
            fence: Fence::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn clock() -> (Arc<AtomicU64>, Clock) {
        // Just past a second's start: a stamp at exactly 10 s is in second 10 ((9, 10] s) and one
        // half a second later in second 11, so the tests begin inside a second, as most stamps are.
        let now = Arc::new(AtomicU64::new(10 * SECOND_NS + 1));
        let read = Arc::clone(&now);
        (now, Arc::new(move || read.load(Ordering::SeqCst)))
    }

    /// T-T1.9.R28 (lane split-merge-21): the cross-group clock in microseconds. Do: a clock that
    /// stands still; two stamps; a stamp a second ahead taken in; then the clock read in both units.
    /// Expect: the stamps rise though the clock stood; the next after the one taken in is past it;
    /// the backup's own nanosecond stamps are past it too.
    #[test]
    fn t_t1_9_r28_the_cross_group_clock_rises_in_microseconds() {
        let (_now, clock) = clock();
        let hooks = Hooks::new(clock, 100);
        let first = hooks.marks.next_micros().unwrap();
        let second = hooks.marks.next_micros().unwrap();
        assert_eq!(first, 10 * SECOND_NS / MICRO_NS + 1);
        assert!(second > first, "{second} after {first}");
        let ahead = first + SECOND_NS / MICRO_NS;
        hooks.marks.observe_micros(ahead).unwrap();
        let third = hooks.marks.next_micros().unwrap();
        assert!(third > ahead, "{third} after {ahead}");
        assert!(hooks.marks.latest_micros().unwrap() >= third);
        assert!(hooks.marks.next().unwrap() > ahead * MICRO_NS);
    }

    /// T-T1.12.P1: commits within a second keep one mark, its last; a new second a new mark.
    #[test]
    fn t_t1_12_p1_one_mark_a_second() {
        let (now, clock) = clock();
        let hooks = Hooks::new(clock, 100);
        hooks.marks.committed(1, 3).unwrap();
        now.fetch_add(SECOND_NS / 2, Ordering::SeqCst);
        hooks.marks.committed(1, 5).unwrap();
        now.fetch_add(SECOND_NS, Ordering::SeqCst);
        hooks.marks.committed(1, 9).unwrap();
        let drained = hooks.marks.drain().unwrap();
        assert_eq!(
            drained.marks.iter().map(|m| m.frame).collect::<Vec<_>>(),
            vec![5, 9]
        );
        assert!(drained.through > drained.marks[1].ns);
    }

    /// T-T1.12.P2: a clock that steps back holds stamps at the last, each later than the one
    /// before, and is counted.
    #[test]
    fn t_t1_12_p2_clock_held_back() {
        let (now, clock) = clock();
        let hooks = Hooks::new(clock, 100);
        hooks.marks.committed(1, 1).unwrap();
        now.fetch_sub(5 * SECOND_NS, Ordering::SeqCst);
        hooks.marks.committed(1, 2).unwrap();
        let drained = hooks.marks.drain().unwrap();
        assert_eq!(drained.marks.len(), 1);
        assert_eq!(drained.marks[0].frame, 2);
        assert_eq!(hooks.marks.counts().unwrap().0, 2);
    }

    /// T-T1.12.P3: past its bound the two oldest marks merge and their seconds are unknown.
    #[test]
    fn t_t1_12_p3_marks_merge_at_their_bound() {
        let (now, clock) = clock();
        let hooks = Hooks::new(clock, 2);
        for frame in 1..=3 {
            hooks.marks.committed(1, frame).unwrap();
            now.fetch_add(SECOND_NS, Ordering::SeqCst);
        }
        let drained = hooks.marks.drain().unwrap();
        assert_eq!(drained.marks.len(), 2);
        assert_eq!(drained.marks[0].frame, 2);
        assert_eq!(drained.marks[0].unknown_from, Some(10 * SECOND_NS + 1));
        assert_eq!(hooks.marks.counts().unwrap().1, 1);
    }

    /// T-T1.12.C3: a snapshot lines up with the stamps only when the log ends at the newest commit
    /// stamped; its second settles once the clock passes the second's end with no commit in it
    /// since, and never once one is stamped in it.
    #[test]
    fn t_t1_12_c3_quiet_second_lines_a_snapshot_up() {
        let (now, clock) = clock();
        let hooks = Hooks::new(clock, 100);
        assert_eq!(
            hooks.marks.quiet_second(|| Ok((1, 0))).unwrap(),
            None,
            "nothing stamped"
        );
        hooks.marks.committed(1, 4).unwrap();
        assert_eq!(
            hooks.marks.quiet_second(|| Ok((1, 5))).unwrap(),
            None,
            "a commit not stamped yet"
        );
        assert_eq!(
            hooks.marks.quiet_second(|| Ok((2, 0))).unwrap(),
            None,
            "the log started again"
        );
        assert_eq!(hooks.marks.quiet_second(|| Ok((1, 4))).unwrap(), Some(11));
        assert_eq!(
            hooks.marks.settled(11).unwrap(),
            None,
            "second 11 still runs"
        );
        assert_eq!(
            hooks.marks.settled(10).unwrap(),
            Some(false),
            "not the second lined up"
        );
        now.fetch_add(SECOND_NS, Ordering::SeqCst);
        assert_eq!(hooks.marks.settled(11).unwrap(), Some(true));
        hooks.marks.committed(1, 6).unwrap();
        assert_eq!(
            hooks.marks.settled(11).unwrap(),
            Some(true),
            "a commit in a later second"
        );

        // A commit stamped within the second lined up: the snapshot isn't that second's rows.
        assert_eq!(hooks.marks.quiet_second(|| Ok((1, 6))).unwrap(), Some(12));
        hooks.marks.committed(1, 8).unwrap();
        now.fetch_add(SECOND_NS, Ordering::SeqCst);
        assert_eq!(hooks.marks.settled(12).unwrap(), Some(false));
    }

    /// T-T1.12.X2: a stamp observed from another group makes every later commit here later than
    /// it, though this group's clock reads earlier.
    #[test]
    fn t_t1_12_x2_observed_stamps_order_later_commits() {
        let (_, clock) = clock();
        let hooks = Hooks::new(clock, 100);
        hooks.marks.committed(1, 1).unwrap();
        let ahead = 15 * SECOND_NS;
        hooks.marks.observe(ahead).unwrap();
        hooks.marks.committed(1, 2).unwrap();
        let drained = hooks.marks.drain().unwrap();
        assert!(drained.marks.last().unwrap().ns > ahead);
        assert!(hooks.marks.latest().unwrap() >= drained.through);
    }

    /// Decision 45 (lane B24): the fence is told how a generation ends before the switch that ends
    /// it. Do: ship generation 4 whole and restart; ask within the switch; then, the shipper not yet
    /// in 5, restart 5 unshipped (forced), asking within that switch, and make it not. Expect: 4
    /// reaches 5 whole within its own switch, and still within and after the next one's.
    #[test]
    fn d45_the_fence_is_told_before_the_switch() {
        let fence = Fence::default();
        fence.shipped(4, 9).unwrap();
        let mut whole = Vec::new();
        let ask = |whole: &mut Vec<bool>| whole.push(fence.ended_whole(4, 9, 5).unwrap());
        fence
            .restart(4, 9, false, || {
                ask(&mut whole);
                Ok(true)
            })
            .unwrap();
        let made = fence
            .restart(5, 3, true, || {
                ask(&mut whole);
                Ok(false)
            })
            .unwrap();
        assert!(!made);
        ask(&mut whole);
        assert_eq!(whole, [true, true, true]);
        assert!(
            !fence.ended_whole(5, 3, 6).unwrap(),
            "a restart not made ended nothing"
        );
    }

    /// Decision 45, AC-T1.12.4 (lane B25): a generation ended by force is whole once all of it is
    /// shipped, and no sooner. Do: start 6 again at frame 5 with 3 shipped; ship it to 4, then 5. Expect:
    /// 6 reaches 7 whole only at 5, asked from 5.
    #[test]
    fn d45_a_generation_ended_by_force_is_whole_once_shipped() {
        let fence = Fence::default();
        fence.shipped(6, 3).unwrap();
        fence.restart(6, 5, true, || Ok(true)).unwrap();
        for frame in 3..=5 {
            fence.shipped(6, frame).unwrap();
            let whole = fence.ended_whole(6, frame, 7).unwrap();
            assert_eq!(whole, frame == 5, "shipped through {frame} of 5");
        }
        assert!(!fence.ended_whole(6, 4, 7).unwrap(), "it ended at 5");
    }

    /// T-T1.12.P11: a log started again twice with no write between keeps its link whole; a
    /// restart forced before its frames were shipped, or one the fence wasn't told of, breaks it.
    #[test]
    fn t_t1_12_p11_fence_follows_restarts_one_by_one() {
        let fence = Fence::default();
        assert!(!fence.ended_whole(4, 9, 5).unwrap(), "no restart seen");
        fence.shipped(4, 9).unwrap();
        fence.restart(4, 9, false, || Ok(true)).unwrap();
        assert!(fence.ended_whole(4, 9, 5).unwrap());
        assert!(
            !fence.ended_whole(4, 8, 5).unwrap(),
            "ended past what was shipped"
        );
        // Generation 5 holds nothing and starts again: 4 still reaches 6 whole, and so does 5.
        fence.restart(5, 0, false, || Ok(true)).unwrap();
        assert!(fence.ended_whole(4, 9, 6).unwrap());
        assert!(fence.ended_whole(5, 0, 6).unwrap());
        assert!(!fence.ended_whole(4, 9, 5).unwrap(), "the log is past 5");
        assert!(!fence.ended_whole(4, 9, 7).unwrap(), "6 ended untold");
        // A forced restart breaks the link from it on.
        fence.restart(6, 3, true, || Ok(true)).unwrap();
        assert!(!fence.ended_whole(6, 3, 7).unwrap());
        assert!(!fence.ended_whole(4, 9, 7).unwrap());
        // An empty generation that doesn't follow on starts afresh.
        fence.restart(9, 0, false, || Ok(true)).unwrap();
        assert!(fence.ended_whole(9, 0, 10).unwrap());
        assert!(!fence.ended_whole(6, 3, 10).unwrap());
    }
}
