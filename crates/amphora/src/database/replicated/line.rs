//! The room of a replicated owner's command queue, and the calls that wait for it in line
//! (replication.md, QB.1 and its amendments of lanes perf-A26 and perf-A27). What it shares between
//! threads comes from `super::sync`: `crates/line-loom` compiles this file over loom's, and runs
//! every interleaving of the events that change it (T-QB.1.19).

use super::sync::{Arc, AtomicUsize, Instant, Mutex, fence};
use super::{Bell, ENVELOPE_BYTES, RECORD_BYTES, Refusal, api, locked, unavailable};
use std::sync::atomic::Ordering;

/// The command queue's room: what its inputs hold of it, the bounds they are held to, and the calls
/// that wait for some.
pub(super) struct Room {
    /// The bytes every input in the queue holds, the calls' and the wire records'.
    pub(super) admitted: AtomicUsize,
    /// The inputs in the command queue's channel, not yet taken by the owner: no more than
    /// `command_bound` (QB.1, lane constants-20), as a channel of that many slots held them.
    pub(super) commands: AtomicUsize,
    /// The calls waiting for the command queue's room (lane perf-A26).
    pub(super) line: Line,
    /// The most inputs the command queue holds, as the owner last learned it (`Bounds::commands`).
    pub(super) command_bound: AtomicUsize,
    /// The bytes the application calls waiting in the command queue hold, each its own real or
    /// declared size (`Admission::bytes_for`), held to `applications` apart from the wire records
    /// beside them (QB.1, lane perf-A11). Calls are not counted: the bytes bound them.
    pub(super) called: AtomicUsize,
    /// The bytes of `admitted` that their writes hold on the memory budget themselves
    /// ([`Room::submit_by`], lane perf-A21): the queues' charge each tick takes leaves them out
    /// (`Actor::rebound`), so no byte is charged twice.
    pub(super) charged: AtomicUsize,
    // The command queue's bound in bytes and the calls' share of it, as the owner last learned them
    // (`queue_bounds`; QB.1).
    pub(super) queued: AtomicUsize,
    pub(super) applications: AtomicUsize,
}
impl Room {
    /// The room of a queue that holds nothing, bound to `queued` bytes, `applications` of them the
    /// calls', and `commands` inputs.
    pub(super) fn new(queued: usize, applications: usize, commands: usize) -> Arc<Self> {
        Arc::new(Self {
            admitted: AtomicUsize::new(0),
            commands: AtomicUsize::new(0),
            line: Line::new(),
            command_bound: AtomicUsize::new(commands),
            called: AtomicUsize::new(0),
            charged: AtomicUsize::new(0),
            queued: AtomicUsize::new(queued),
            applications: AtomicUsize::new(applications),
        })
    }

    /// The room a write of `bytes` takes, on the memory budget and in the command queue, waited
    /// for until its deadline on the events that make it, holding none of it meanwhile. Its bytes
    /// are charged to the budget's room for queues as a local writer's admission charges them
    /// (`Admission`, lane perf-A21): past the budget's whole limit refused `TooLarge` at once, with
    /// no room at its deadline `Full`. A write in line holds no charge (lane perf-A26): charges held
    /// by writes the queue has no room for would take the images' and the caches' room.
    pub(super) fn submit_by(
        room: &Arc<Self>,
        bytes: usize,
        until: Instant,
    ) -> Result<Reservation, Refusal> {
        let budget = crate::memory::process();
        let size = u64::try_from(bytes).unwrap_or(u64::MAX);
        loop {
            let charge = budget.take_until(crate::memory::Use::Queues, size, until, || {})?;
            if room.line.waiting.load(Ordering::Acquire) == 0
                && let Ok(place) = Self::place(room, bytes, true)
            {
                return Ok(place.charged(charge, bytes));
            }
            drop(charge);
            let place = Self::place_by(room, bytes, until)?;
            match budget.take(crate::memory::Use::Queues, size) {
                Ok(charge) => return Ok(place.charged(charge, bytes)),
                // The budget's room went meanwhile: it is waited for again, the place given back.
                Err(Refusal::TooLarge { .. }) => drop(place),
                Err(refusal) => return Err(refusal),
            }
        }
    }

    /// A call's place in the command queue, waited for in line (`Line`) until `until`, holding
    /// nothing meanwhile: refused `Full` at the bound it is past once `until` has passed, with the
    /// owner's reason once it ended, and `TooLarge` at once past what one call may hold (`alone`),
    /// which no bytes given back make room for.
    pub(super) fn place_by(
        room: &Arc<Self>,
        bytes: usize,
        until: Instant,
    ) -> Result<Reservation, Refusal> {
        let line = &room.line;
        if line.waiting.load(Ordering::Acquire) == 0
            && let Ok(place) = Self::place(room, bytes, true)
        {
            return Ok(place);
        }
        let turn = line.join()?;
        loop {
            let queued = room.queued.load(Ordering::Acquire);
            api::bound("replica command queue bytes", bytes, alone(queued))?;
            // Looked at before it is taken: a place taken in part and given back would ring this
            // call again, and it would look without end.
            let past = room.past(bytes);
            if turn.first()?
                && past.is_none()
                && let Ok(place) = Self::place(room, bytes, true)
            {
                return Ok(place);
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(past.unwrap_or(Refusal::Full {
                    limit: room.applications.load(Ordering::Acquire),
                }));
            }
            turn.bell.wait(left)?;
        }
    }

    /// The bound of the command queue a call of `bytes` is past now, as the refusal naming it.
    fn past(&self, bytes: usize) -> Option<Refusal> {
        let commands = self.command_bound.load(Ordering::Acquire);
        if self.commands.load(Ordering::Acquire) >= commands.saturating_sub(1) {
            return Some(Refusal::Full { limit: commands });
        }
        let queued = self.queued.load(Ordering::Acquire);
        let applications = self.applications.load(Ordering::Acquire);
        let called = self.called.load(Ordering::Acquire);
        if within(called, bytes, applications, alone(queued)).is_none() {
            return Some(Refusal::Full {
                limit: applications,
            });
        }
        let admitted = self.admitted.load(Ordering::Acquire);
        if admitted.saturating_add(bytes) > queued {
            return Some(Refusal::Full { limit: queued });
        }
        None
    }

    /// A place in the command queue for an input of `bytes`, a call's (`call`) or another's, or the
    /// refusal naming the bound it is past. Calls leave room for a wire record, the one that finds
    /// a leader above all: the last place, and the queue's bytes past the calls' bound (QB.1).
    pub(super) fn place(
        room: &Arc<Self>,
        bytes: usize,
        call: bool,
    ) -> Result<Reservation, Refusal> {
        let limit = room.queued.load(Ordering::Acquire);
        let application_limit = room.applications.load(Ordering::Acquire);
        let command_limit = room.command_bound.load(Ordering::Acquire);
        // Built before any refusal below, so that dropping it gives back what it took.
        let mut reservation = Reservation::none(room);
        // Every input takes a place in the channel, held to the inputs' bound (lane constants-20); a
        // call never the last, which stays a wire record's.
        let places = if call {
            command_limit.saturating_sub(1)
        } else {
            command_limit
        };
        room.commands
            .try_update(Ordering::AcqRel, Ordering::Acquire, |held| {
                held.checked_add(1).filter(|next| *next <= places)
            })
            .map_err(|_| Refusal::Full {
                limit: command_limit,
            })?;
        reservation.slot = true;
        // A call's bytes are held to the calls' bound on their own: the wire records queued beside
        // it are the rest of the queue's, not the calls'.
        if call {
            room.called
                .try_update(Ordering::AcqRel, Ordering::Acquire, |held| {
                    within(held, bytes, application_limit, alone(limit))
                })
                .map_err(|_| Refusal::Full {
                    limit: application_limit,
                })?;
            reservation.called = bytes;
        }
        room.admitted
            .try_update(Ordering::AcqRel, Ordering::Acquire, |charged| {
                charged.checked_add(bytes).filter(|next| *next <= limit)
            })
            .map_err(|charged| match charged.checked_add(bytes) {
                Some(next) => api::bound("replica command queue bytes", next, limit)
                    .err()
                    .unwrap_or(Refusal::Full { limit }),
                None => Refusal::Full { limit },
            })?;
        reservation.bytes = bytes;
        Ok(reservation)
    }
}

/// What an input holds of the queue's room, given back as it drops.
pub(super) struct Reservation {
    room: Arc<Room>,
    /// An input's place in the command queue's channel (`Room::commands`), given back once the
    /// owner takes it or it is dropped untaken (QB.1, lane constants-20).
    slot: bool,
    pub(super) bytes: usize,
    /// An application call's bytes in `called`, given back with it.
    pub(super) called: usize,
    /// A write's bytes held on the memory budget ([`Room::submit_by`], lane perf-A21) and
    /// counted in `charged`: given back with the rest, whether the write applied or was refused.
    charge: Option<crate::memory::Charge>,
}
impl Reservation {
    /// One that holds nothing: of work the owner began itself, which came through no queue.
    pub(super) fn none(room: &Arc<Room>) -> Self {
        Self {
            room: Arc::clone(room),
            slot: false,
            bytes: 0,
            called: 0,
            charge: None,
        }
    }
    /// With the charge its write's `bytes` hold on the memory budget.
    fn charged(mut self, charge: crate::memory::Charge, bytes: usize) -> Self {
        self.room.charged.fetch_add(bytes, Ordering::AcqRel);
        self.charge = Some(charge);
        self
    }
    /// The owner took its input from the channel: its place there is free, its bytes still held.
    pub(super) fn taken(&mut self) {
        if std::mem::take(&mut self.slot) {
            self.room.commands.fetch_sub(1, Ordering::SeqCst);
            self.room.line.room_came();
        }
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.taken();
        self.room.admitted.fetch_sub(self.bytes, Ordering::SeqCst);
        self.room.called.fetch_sub(self.called, Ordering::SeqCst);
        if let Some(charge) = self.charge.take() {
            let bytes = usize::try_from(charge.bytes()).unwrap_or(usize::MAX);
            self.room.charged.fetch_sub(bytes, Ordering::AcqRel);
        }
        self.room.line.room_came();
    }
}
/// The calls waiting for the command queue's room ([`Room::place_by`]; QB.1, lane perf-A26), first
/// come, first served: one entry a waiting caller's thread, gone by that caller's deadline. Only the
/// first looks for room, rung by each event that may make some (a place given back, bytes given back,
/// the bounds learned again), and the next is rung as the first leaves, so an event wakes one thread
/// however many wait.
pub(super) struct Line {
    /// The calls in line: read without the lock by what makes room, so that room made with none
    /// waiting takes no lock.
    pub(super) waiting: AtomicUsize,
    turns: Mutex<Turns>,
}
struct Turns {
    /// Each waiting call's bell by its turn, the first in line first.
    line: std::collections::BTreeMap<u64, Arc<Bell>>,
    next: u64,
    /// The owner ended: its reason, which every call in line is refused with.
    ended: Option<Refusal>,
}
impl Line {
    fn new() -> Self {
        Self {
            waiting: AtomicUsize::new(0),
            turns: Mutex::new(Turns {
                line: std::collections::BTreeMap::new(),
                next: 0,
                ended: None,
            }),
        }
    }
    /// The queue's room moved, by a sequentially consistent change made before this: the first in
    /// line looks again. Pairs with [`Line::join`]'s fence (C++ [atomics.order], the single total
    /// order): either this sees the call that joined, or that call's look sees the room.
    pub(super) fn room_came(&self) {
        if self.waiting.load(Ordering::SeqCst) == 0 {
            return;
        }
        // A poisoned line is heard by its own calls, each of whose looks fails on the poison.
        if let Ok(turns) = self.turns.lock()
            && let Some(first) = turns.line.values().next()
        {
            first.ring();
        }
    }
    /// A call joins the line, last.
    fn join(&self) -> Result<Turn<'_>, Refusal> {
        let bell = Bell::new();
        let mut turns = locked(&self.turns)?;
        let turn = turns.next;
        turns.next = turn
            .checked_add(1)
            .ok_or_else(|| unavailable("the queue's line ran out of turns"))?;
        turns.line.insert(turn, Arc::clone(&bell));
        self.waiting.fetch_add(1, Ordering::SeqCst);
        drop(turns);
        fence(Ordering::SeqCst);
        Ok(Turn {
            line: self,
            turn,
            bell,
        })
    }
    /// The owner ended: every call in line, and each that joins after, is refused with `reason`.
    pub(super) fn end(&self, reason: &Refusal) {
        if let Ok(mut turns) = self.turns.lock() {
            turns.ended.get_or_insert_with(|| reason.clone());
            for bell in turns.line.values() {
                bell.ring();
            }
        }
    }
}
/// A call's turn in the line; it leaves the line as this drops, and the first behind it looks.
struct Turn<'a> {
    line: &'a Line,
    turn: u64,
    bell: Arc<Bell>,
}
impl Turn<'_> {
    /// Whether this call is the first in line, or the owner's reason once it ended.
    fn first(&self) -> Result<bool, Refusal> {
        let turns = locked(&self.line.turns)?;
        match &turns.ended {
            Some(reason) => Err(reason.clone()),
            None => Ok(turns.line.keys().next() == Some(&self.turn)),
        }
    }
}
impl Drop for Turn<'_> {
    fn drop(&mut self) {
        if let Ok(mut turns) = self.line.turns.lock() {
            turns.line.remove(&self.turn);
            self.line.waiting.fetch_sub(1, Ordering::SeqCst);
            if let Some(first) = turns.line.values().next() {
                first.ring();
            }
        }
    }
}
/// What the calls' bytes hold with `bytes` more, within their `limit`; holding none, they take one
/// call of up to `alone`, as a window that holds nothing sends one entry however large (hyper-raft's
/// `Inflights::full`): what the memory budget has room for never waits for room no one can make.
fn within(held: usize, bytes: usize, limit: usize, alone: usize) -> Option<usize> {
    let limit = if held == 0 { limit.max(alone) } else { limit };
    held.checked_add(bytes).filter(|next| *next <= limit)
}
/// The most bytes one call may hold of a command queue of `queued` bytes: all but a record without
/// entries (`RECORD_BYTES`) and its envelope, so that a vote or a heartbeat, the record that finds
/// a leader, enters beside it (QB.1).
pub(super) fn alone(queued: usize) -> usize {
    queued
        .saturating_sub(RECORD_BYTES)
        .saturating_sub(ENVELOPE_BYTES)
}
