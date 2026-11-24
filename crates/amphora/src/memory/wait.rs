//! What a holder that finds no room on the memory budget waits on ([`super::Budget::take_until`]; MB.1,
//! docs/design/store.md): bytes given back anywhere in the budget, the end of the owner the room is for
//! (lane perf-A28), or its deadline, whichever is first. What it shares between threads comes from
//! `super::sync`: `crates/line-loom` compiles this file over loom's, and runs every interleaving of a
//! holder's wait, a give-back and an owner's end (T-MB.1.7).

use super::Refusal;
use super::sync::{Arc, AtomicU64, Condvar, Instant, Mutex, fence};
use std::sync::PoisonError;
use std::sync::atomic::Ordering;

/// Why a look for room took none.
pub(super) enum NoRoom {
    /// None now, the room seen: bytes given back may make some.
    Now(usize),
    /// Refused: none of those makes room for it.
    Refused(Refusal),
}

/// The holders waiting for room.
pub(super) struct Waiters {
    /// How many wait now: bytes given back with none waiting take no lock.
    holders: AtomicU64,
    /// How often they were told to look again, counted under this lock: a holder that found no room
    /// waits on it until the count moves or its deadline passes.
    told: Mutex<u64>,
    telling: Condvar,
}

impl Waiters {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            holders: AtomicU64::new(0),
            told: Mutex::new(0),
            telling: Condvar::new(),
        })
    }

    /// The holders waiting now.
    pub(super) fn holders(&self) -> u64 {
        self.holders.load(Ordering::Relaxed)
    }

    /// Bytes were given back, by a sequentially consistent change made before this: the holders
    /// waiting look again. Pairs with [`Waiters::until`]'s fence.
    pub(super) fn given(&self) {
        if self.holders.load(Ordering::SeqCst) > 0 {
            self.look_again();
        }
    }

    /// Every holder waiting looks again.
    pub(super) fn look_again(&self) {
        let mut told = self.told.lock().unwrap_or_else(PoisonError::into_inner);
        *told = told.wrapping_add(1);
        drop(told);
        self.telling.notify_all();
    }

    /// What `look` finds, looked for until `until`: again each time bytes are given back, never by a
    /// timer, `waits` told (under the lock) each time it waits. Refused [`Refusal::Full`], with the
    /// room it last saw, once `until` has passed; with `owner`'s reason, nothing taken, once it has
    /// ended.
    pub(super) fn until<T>(
        waiters: &Arc<Self>,
        until: Instant,
        owner: &Ending,
        mut look: impl FnMut() -> Result<T, NoRoom>,
        mut waits: impl FnMut(),
    ) -> Result<T, Refusal> {
        let _watched = owner.watched_by(waiters)?;
        let mut told = waiters.told.lock().unwrap_or_else(PoisonError::into_inner);
        waiters.holders.fetch_add(1, Ordering::Relaxed);
        // Pairs with the sequentially consistent give-back and read of `holders` of `given`'s caller
        // and `given` (C++ [atomics.order], the single total order; Linux's include/linux/wait.h,
        // `waitqueue_active`): either this look for room sees bytes given back, or their giver sees
        // this holder waiting.
        fence(Ordering::SeqCst);
        let found = 'look: loop {
            let seen = *told;
            // The end is told under this lock after its reason is set, so this look sees the reason
            // or the wait below hears the end.
            if let Some(reason) = owner.reason() {
                break Err(reason);
            }
            let room = match look() {
                Ok(found) => break Ok(found),
                Err(NoRoom::Refused(refusal)) => break Err(refusal),
                Err(NoRoom::Now(room)) => room,
            };
            let refused = Refusal::Full { limit: room };
            let mut left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break Err(refused);
            }
            waits();
            while *told == seen {
                if left.is_zero() {
                    break 'look Err(refused);
                }
                told = waiters
                    .telling
                    .wait_timeout(told, left)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
                left = until.saturating_duration_since(Instant::now());
            }
        };
        waiters.holders.fetch_sub(1, Ordering::Relaxed);
        drop(told);
        found
    }
}

/// An owner's end, once, with its reason, told to those that wait on the budget for the room of the
/// owner's work. A wait with no owner is given one that never ends.
#[derive(Default)]
pub struct Ending {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Why it ended, once it has.
    reason: Option<Refusal>,
    /// The waiters of each wait that the end is to be told to, once for each wait.
    watched: Vec<Arc<Waiters>>,
}

impl Ending {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Why it ended, once it has.
    pub(crate) fn reason(&self) -> Option<Refusal> {
        self.locked().reason.clone()
    }

    /// The owner ended: every wait that is for it, and each that begins after, is refused with
    /// `reason`. The first reason stands.
    pub(crate) fn end(&self, reason: &Refusal) {
        let mut watched = {
            let mut state = self.locked();
            state.reason.get_or_insert_with(|| reason.clone());
            std::mem::take(&mut state.watched)
        };
        // Told once for a budget, however many wait on it.
        watched.dedup_by(|a, b| Arc::ptr_eq(a, b));
        for waiters in watched {
            waiters.look_again();
        }
    }

    /// Told to `waiters` as it ends, until the guard drops; its reason if it has ended.
    fn watched_by(&self, waiters: &Arc<Waiters>) -> Result<Watched<'_>, Refusal> {
        let mut state = self.locked();
        if let Some(reason) = &state.reason {
            return Err(reason.clone());
        }
        state.watched.push(Arc::clone(waiters));
        drop(state);
        Ok(Watched {
            ending: self,
            waiters: Arc::clone(waiters),
        })
    }

    /// The waits told as it ends.
    #[cfg(test)]
    pub(crate) fn waits(&self) -> usize {
        self.locked().watched.len()
    }

    fn locked(&self) -> super::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A wait told of an owner's end while this lives.
struct Watched<'a> {
    ending: &'a Ending,
    waiters: Arc<Waiters>,
}

impl Drop for Watched<'_> {
    fn drop(&mut self) {
        let mut state = self.ending.locked();
        // Gone if the end took the list.
        if let Some(at) = state
            .watched
            .iter()
            .position(|waiters| Arc::ptr_eq(waiters, &self.waiters))
        {
            state.watched.swap_remove(at);
        }
    }
}
