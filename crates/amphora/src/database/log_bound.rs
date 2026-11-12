//! When a replica's owner starts its database's log again, and how far the log may grow
//! (docs/design/replication.md, "A replica's database log bounded by what it measures"). A pure
//! core: the owner tells it each step's entries and time, each tick and each restart, and the
//! flushing thread gives it the filesystem's free bytes. Nothing in it is picked:
//!
//! - a restart holds the owner, and with it every entry the owner applies until its first idle
//!   step after (held). One is due only while the entries held by restarts, the entries the
//!   owner's own long steps already delayed past AC-T1.3.4's p99.9 (over), and the most one
//!   restart held (next, for the restart about to come) stay strictly below the p99.9's tail of
//!   the entries applied, over patience's window of two generations;
//! - the log may hold what its own file and an equal share, among the replica logs on its
//!   filesystem, of what is free there less one snapshot's image, hold: each frame its own bytes
//!   and the page its copy may add to the database's file ([`log_room`]).

use amphora_api::Refusal;
use std::time::Duration;

use super::patience::GENERATION;

/// The quantile the registry holds a commit's latency to, as a fraction: AC-T1.3.4's p99.9, the
/// highest of docs/requirements.toml (AC-T1.4.3 holds the same quantile).
const QUANTILE: (u64, u64) = (999, 1_000);
/// AC-T1.3.4's bound on a commit's p99.9: what an owner's step applies is delayed past it once the
/// step takes longer, a proxy for that entry's commit, whose quorum and network the owner can't
/// see.
const COMMIT_TAIL: Duration = Duration::from_millis(15);
/// A frame of the database's log: a 4 KiB page (the page every replicated delta and image holds,
/// `replication::validate_pages`) and its 24-byte header (sqlite.org/fileformat.html, "WAL File
/// Format").
pub(super) const FRAME_BYTES: u64 = 4096 + 24;

/// What one generation of the window counted, in entries applied.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Window {
    applied: u64,
    /// Applied while a restart held the owner, or before its first idle step after.
    held: u64,
    /// Applied, not held, by a step longer than [`COMMIT_TAIL`].
    over: u64,
    /// The most one restart held that ended its hold in this generation.
    most: u64,
}

/// An owner's count of what its restarts of the database's log held.
#[derive(Debug, Default)]
pub(super) struct Spacing {
    ticks: u64,
    generation: u64,
    current: Window,
    before: Window,
    /// The entries the restart under way has held so far: from its start to the owner's first
    /// idle step after it.
    holding: Option<u64>,
}

fn overflowed(what: &str) -> Refusal {
    Refusal::Unavailable(format!("the log restarts' count of {what} overflowed"))
}

impl Spacing {
    /// One tick of the owner: past a generation, the generation before is forgotten.
    pub(super) fn tick(&mut self) -> Result<(), Refusal> {
        self.ticks = self
            .ticks
            .checked_add(1)
            .ok_or_else(|| overflowed("ticks"))?;
        if self.ticks.saturating_sub(self.generation) >= GENERATION {
            self.generation = self.ticks;
            self.before = std::mem::take(&mut self.current);
        }
        Ok(())
    }

    /// The owner started the log again: what it applies from now to its first idle step is held.
    /// A restart within another's hold goes on counting into it.
    pub(super) fn restarted(&mut self) {
        self.holding = Some(self.holding.unwrap_or(0));
    }

    /// A step of the owner that applied `entries` and took `took` of its clock; `idle` when it
    /// found no input waiting, which ends a restart's hold.
    pub(super) fn stepped(
        &mut self,
        entries: u64,
        took: Duration,
        idle: bool,
    ) -> Result<(), Refusal> {
        let window = &mut self.current;
        window.applied = window
            .applied
            .checked_add(entries)
            .ok_or_else(|| overflowed("entries applied"))?;
        if let Some(held) = self.holding.as_mut() {
            *held = held
                .checked_add(entries)
                .ok_or_else(|| overflowed("entries held"))?;
            window.held = window
                .held
                .checked_add(entries)
                .ok_or_else(|| overflowed("entries held"))?;
        } else if took > COMMIT_TAIL {
            window.over = window
                .over
                .checked_add(entries)
                .ok_or_else(|| overflowed("entries over the tail"))?;
        }
        if idle && let Some(held) = self.holding.take() {
            window.most = window.most.max(held);
        }
        Ok(())
    }

    /// Whether a restart is due by its share: the held, the over and the next restart's worst
    /// guess, against the entries applied over the window, strictly below the quantile's tail.
    pub(super) fn due(&self) -> Result<bool, Refusal> {
        let (quantile, of) = QUANTILE;
        let tail = of
            .checked_sub(quantile)
            .ok_or_else(|| overflowed("the tail"))?;
        let (current, before) = (&self.current, &self.before);
        let sum = |a: u64, b: u64, what| a.checked_add(b).ok_or_else(|| overflowed(what));
        let applied = sum(current.applied, before.applied, "entries applied")?;
        let next = current.most.max(before.most).max(self.holding.unwrap_or(0));
        let spent = sum(
            sum(current.held, before.held, "entries held")?,
            sum(current.over, before.over, "entries over the tail")?,
            "entries spent",
        )?;
        let spent = sum(spent, next, "entries spent")?;
        let spent = spent
            .checked_mul(of)
            .ok_or_else(|| overflowed("entries spent"))?;
        let room = applied
            .checked_mul(tail)
            .ok_or_else(|| overflowed("entries applied"))?;
        Ok(spent < room)
    }
}

/// The frames a replica's database log may hold: the bytes its file already takes (`kept` frames,
/// reused, store.md decision 13) and an equal share among the `logs` replica logs on its
/// filesystem of the `available` bytes there, less one snapshot's image
/// ([`crate::replication::SNAPSHOT_BYTES`], so an install has room), each frame taking its own
/// bytes and the page its copy may add to the database's file. Counted in bytes, a frame the log
/// grows by moves bytes from `available` to its file, so the room holds still for one log and
/// grows by less than half a frame a frame for more, the others' growth seen at the next pass. The
/// process's disk budget, once it exists, passes its room for replica logs as `available`.
pub(super) fn log_room(available: u64, logs: u64, kept: u64) -> u64 {
    let image = u64::try_from(crate::replication::SNAPSHOT_BYTES).unwrap_or(u64::MAX);
    // Saturating, each: nothing free past the image, and a length past u64's bytes the most it can.
    let free = available.saturating_sub(image);
    let share = free.checked_div(logs.max(1)).unwrap_or(0);
    let held = kept.saturating_mul(FRAME_BYTES).saturating_add(share);
    FRAME_BYTES
        .checked_add(crate::db::PAGE_BYTES)
        .and_then(|each| held.checked_div(each))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUICK: Duration = Duration::from_millis(1);

    /// AC-T1.8.D1, T-T1.8.D3: do a step that applied one entry on a new owner; expect a restart
    /// due: the first restart is free, nothing held yet.
    #[test]
    fn the_first_restart_is_free() {
        let mut spacing = Spacing::default();
        assert!(!spacing.due().unwrap(), "due with nothing applied");
        spacing.stepped(1, QUICK, true).unwrap();
        assert!(spacing.due().unwrap());
    }

    /// AC-T1.8.D1, T-T1.8.D3: do a restart that held 5 entries until an idle step; expect the next
    /// due only once a thousand times its held and its worst guess (5 + 5) are below the entries
    /// applied: at 10,001 and not at 10,000.
    #[test]
    fn a_restart_spends_what_it_held_and_spaces_the_next() {
        let mut spacing = Spacing::default();
        spacing.restarted();
        spacing.stepped(5, QUICK, false).unwrap();
        spacing.stepped(0, QUICK, true).unwrap();
        spacing.stepped(9_995, QUICK, true).unwrap();
        assert!(!spacing.due().unwrap(), "due at 10,000 applied");
        spacing.stepped(1, QUICK, true).unwrap();
        assert!(spacing.due().unwrap(), "not due at 10,001 applied");
    }

    /// AC-T1.8.D1, T-T1.8.D3: do a step longer than AC-T1.3.4's 15 ms applying 3 entries; expect
    /// them spent as the base tail (a restart due at 3,001 applied, not 3,000), and a restart's
    /// hold not counted twice: entries a long step applies while held count as held only.
    #[test]
    fn long_steps_spend_the_tail_before_any_restart() {
        let mut spacing = Spacing::default();
        spacing.stepped(3, Duration::from_millis(16), true).unwrap();
        spacing.stepped(2_997, QUICK, true).unwrap();
        assert!(!spacing.due().unwrap(), "due at 3,000 applied with 3 over");
        spacing.stepped(1, QUICK, true).unwrap();
        assert!(spacing.due().unwrap());
        assert_eq!(spacing.current.over, 3);
        spacing.restarted();
        spacing.stepped(4, Duration::from_millis(16), true).unwrap();
        assert_eq!((spacing.current.over, spacing.current.held), (3, 4));
    }

    /// AC-T1.8.D1, T-T1.8.D3 (amphora-owner-t1-log-bound-4a274ec-20261111, point 1): an owner under
    /// steady load, never idle after a restart, still ends the restart's hold once its backlog is
    /// gone. Do: a restart that held 5 entries, then a million steps of 1 entry each, every one
    /// with input waiting. Expect: a restart due again by its share.
    #[test]
    fn a_restart_due_again_by_its_share_on_an_owner_never_idle() {
        let mut spacing = Spacing::default();
        spacing.restarted();
        spacing.stepped(5, QUICK, false).unwrap();
        for _ in 0..1_000_000 {
            spacing.stepped(1, QUICK, false).unwrap();
        }
        assert!(spacing.due().unwrap(), "held for good: {:?}", spacing.current);
    }

    /// AC-T1.8.D1, T-T1.8.D3: do a restart that held 50 entries, then two generations of ticks;
    /// expect it forgotten: a restart due again after one entry.
    #[test]
    fn the_window_forgets_after_two_generations() {
        let mut spacing = Spacing::default();
        spacing.restarted();
        spacing.stepped(50, QUICK, true).unwrap();
        spacing.stepped(1, QUICK, true).unwrap();
        assert!(!spacing.due().unwrap());
        for _ in 0..GENERATION {
            spacing.tick().unwrap();
        }
        assert!(!spacing.due().unwrap(), "forgotten after one generation");
        for _ in 0..GENERATION {
            spacing.tick().unwrap();
        }
        spacing.stepped(1, QUICK, true).unwrap();
        assert!(spacing.due().unwrap());
    }

    /// AC-T1.8.D1, T-T1.8.D3 (the share): do an owner applying entries in steps of 1 to 7, a step in
    /// 20,011 a long one (a base tail well within the p99.9's), each restart made as soon as due
    /// and holding 1 to 9 entries; expect, after
    /// the hold of every restart that held no more than the worst before it, the held and the
    /// base tail strictly below a thousandth of the entries applied over the window, through four
    /// generations; and such restarts made.
    #[test]
    fn restarts_made_when_due_keep_their_share_below_the_tail() {
        let mut spacing = Spacing::default();
        let (mut restarts, mut checked, mut worst, mut seed) = (0u64, 0u64, 0u64, 7u64);
        let mut next = || {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            seed >> 33
        };
        for step in 0..4 * GENERATION * 20 {
            if step % 20 == 0 {
                spacing.tick().unwrap();
            }
            let took = if next() % 20_011 == 0 {
                Duration::from_millis(20)
            } else {
                QUICK
            };
            spacing.stepped(1 + next() % 7, took, true).unwrap();
            if spacing.due().unwrap() {
                spacing.restarted();
                let held = 1 + next() % 9;
                spacing.stepped(held, QUICK, false).unwrap();
                spacing.stepped(0, QUICK, true).unwrap();
                restarts += 1;
                if held > worst {
                    worst = held;
                    continue;
                }
                checked += 1;
                let (c, b) = (&spacing.current, &spacing.before);
                let spent = (c.held + b.held + c.over + b.over) * 1_000;
                assert!(
                    spent < c.applied + b.applied,
                    "restart {restarts}: {spent} against {} applied",
                    c.applied + b.applied
                );
            }
        }
        assert!(checked >= 4, "{checked} of {restarts} restarts checked");
    }

    /// AC-T1.8.D1, T-T1.8.D3: do the disk's share at its edges; expect the log's own file and an
    /// equal share of what is free past one image, each frame its own bytes and a page: a file of
    /// 40 frames (164,800 bytes) alone holds 20; with 100 frames' worth free, 120; with a second
    /// log on the filesystem, 70; nothing free past the image the same as none. A frame grown
    /// from free bytes into the file leaves one log's room where it was.
    #[test]
    fn a_log_takes_its_file_and_an_equal_share_of_what_is_free_past_one_image() {
        let image = u64::try_from(crate::replication::SNAPSHOT_BYTES).unwrap();
        let each = FRAME_BYTES + crate::db::PAGE_BYTES;
        assert_eq!(log_room(image, 1, 40), 20);
        assert_eq!(log_room(image / 2, 1, 40), 20);
        assert_eq!(log_room(image + 100 * each, 1, 40), 120);
        assert_eq!(log_room(image + 100 * each, 2, 40), 70);
        assert_eq!(
            log_room(image + 100 * each - FRAME_BYTES, 1, 41),
            log_room(image + 100 * each, 1, 40)
        );
        assert_eq!(log_room(u64::MAX, 0, u64::MAX), u64::MAX / each);
    }
}
