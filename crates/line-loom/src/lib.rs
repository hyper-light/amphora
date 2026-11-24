//! The replicated owner's command queue and the calls that wait for its room (T-QB.1.19, lane
//! perf-A27; replication.md, QB.1's amendment of that lane), on every interleaving of the events
//! that move them: a call's arrival, a place or bytes given back, a wire record's arrival, a
//! deadline, the bounds learned again, the owner's end, and the memory budget's room going after a
//! place was granted. And the wait for the memory budget's room (T-MB.1.7, lane perf-A28; store.md),
//! on every interleaving of a holder's wait, bytes given back and an owner's end. The code under test
//! is the owner's own file, `line.rs`, and the budget's, `memory/wait.rs`, compiled here over loom's
//! types; the stand-ins below are what they take from around them.
//!
//! Run: `RUSTFLAGS="--cfg loom" cargo test -p amphora-line-loom --release --locked -- --test-threads=3`
//! (scripts/gates.sh; about three minutes on three cores).
#![cfg(all(loom, test))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros
)]

use amphora_api as api;
use amphora_api::Refusal;
use loom::sync::{Condvar, Mutex, MutexGuard};
use std::time::Duration;

#[path = "../../amphora/src/database/replicated/line.rs"]
#[allow(dead_code)]
mod line;

#[path = "../../amphora/src/memory/wait.rs"]
#[allow(dead_code)]
mod wait;

fn unavailable(e: impl std::fmt::Display) -> Refusal {
    Refusal::Unavailable(e.to_string())
}
fn locked<T>(m: &Mutex<T>) -> Result<MutexGuard<'_, T>, Refusal> {
    m.lock().map_err(|_| unavailable("poisoned"))
}

/// What `line` and `wait` share between threads. Loom takes a sequentially consistent access for an
/// acquire-release one and models only `fence(SeqCst)` as C++ states it (its README, "SeqCst
/// accesses"), so each such access carries the fence before it: the mapping of C++'s onto a machine
/// with none of its own (Power's, "leading sync"), in which no access of `join`'s stands after its
/// own `fence` but by that fence.
mod sync {
    pub(crate) use loom::sync::atomic::fence;
    pub(crate) use loom::sync::{Arc, Condvar, Mutex, MutexGuard};
    use std::sync::atomic::Ordering;

    pub(crate) struct AtomicUsize(loom::sync::atomic::AtomicUsize);
    impl AtomicUsize {
        pub(crate) fn new(value: usize) -> Self {
            Self(loom::sync::atomic::AtomicUsize::new(value))
        }
        fn fenced(order: Ordering) {
            if order == Ordering::SeqCst {
                fence(Ordering::SeqCst);
            }
        }
        pub(crate) fn load(&self, order: Ordering) -> usize {
            Self::fenced(order);
            self.0.load(order)
        }
        pub(crate) fn store(&self, value: usize, order: Ordering) {
            Self::fenced(order);
            self.0.store(value, order);
        }
        pub(crate) fn fetch_add(&self, by: usize, order: Ordering) -> usize {
            Self::fenced(order);
            self.0.fetch_add(by, order)
        }
        pub(crate) fn fetch_sub(&self, by: usize, order: Ordering) -> usize {
            Self::fenced(order);
            self.0.fetch_sub(by, order)
        }
        pub(crate) fn try_update(
            &self,
            set: Ordering,
            fetch: Ordering,
            change: impl FnMut(usize) -> Option<usize>,
        ) -> Result<usize, usize> {
            Self::fenced(set);
            self.0.fetch_update(set, fetch, change)
        }
    }

    pub(crate) struct AtomicU64(loom::sync::atomic::AtomicU64);
    impl AtomicU64 {
        pub(crate) fn new(value: u64) -> Self {
            Self(loom::sync::atomic::AtomicU64::new(value))
        }
        pub(crate) fn load(&self, order: Ordering) -> u64 {
            AtomicUsize::fenced(order);
            self.0.load(order)
        }
        pub(crate) fn fetch_add(&self, by: u64, order: Ordering) -> u64 {
            AtomicUsize::fenced(order);
            self.0.fetch_add(by, order)
        }
        pub(crate) fn fetch_sub(&self, by: u64, order: Ordering) -> u64 {
            AtomicUsize::fenced(order);
            self.0.fetch_sub(by, order)
        }
        pub(crate) fn try_update(
            &self,
            set: Ordering,
            fetch: Ordering,
            change: impl FnMut(u64) -> Option<u64>,
        ) -> Result<u64, u64> {
            AtomicUsize::fenced(set);
            self.0.fetch_update(set, fetch, change)
        }
    }

    /// A moment of the model's clock, in ticks: what a deadline is compared with.
    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
    pub(crate) struct Instant(pub(crate) u64);
    impl Instant {
        pub(crate) fn now() -> Self {
            Self(super::CLOCK.now())
        }
        pub(crate) fn saturating_duration_since(self, earlier: Self) -> std::time::Duration {
            std::time::Duration::from_nanos(self.0.saturating_sub(earlier.0))
        }
    }
}
use sync::{Arc, Instant};

/// The model's clock: it stands still but as a test's thread moves it, and moving it wakes whatever
/// waits with a time, as a timer would (loom's `Condvar::wait_timeout` never times out).
struct Clock {
    now: loom::sync::atomic::AtomicUsize,
    /// The bells that wait with a time, or have.
    waits: Mutex<Vec<Arc<Bell>>>,
    /// The memory budget's waiters, who wait with a time too.
    budget: Mutex<Vec<memory::Budget>>,
}
impl Clock {
    fn now(&self) -> u64 {
        u64::try_from(self.now.load(std::sync::atomic::Ordering::SeqCst)).unwrap()
    }
    fn pass(&self, to: u64) {
        self.now.store(
            usize::try_from(to).unwrap(),
            std::sync::atomic::Ordering::SeqCst,
        );
        let waits: Vec<_> = self.waits.lock().unwrap().clone();
        for bell in waits {
            bell.timer();
        }
        let budgets: Vec<_> = self.budget.lock().unwrap().clone();
        for budget in budgets {
            budget.timer();
        }
    }
}
loom::lazy_static! {
    static ref CLOCK: Clock = Clock {
        now: loom::sync::atomic::AtomicUsize::new(0),
        waits: Mutex::new(Vec::new()),
        budget: Mutex::new(Vec::new()),
    };
    static ref BUDGET: Mutex<Option<memory::Budget>> = Mutex::new(None);
}

/// The owner's bell (`replicated::Bell`): a ring kept until a wait takes it, or the wait's time.
struct Bell {
    rung: Mutex<bool>,
    ringing: Condvar,
}
impl Bell {
    fn new() -> Arc<Self> {
        let bell = Arc::new(Self {
            rung: Mutex::new(false),
            ringing: Condvar::new(),
        });
        CLOCK.waits.lock().unwrap().push(Arc::clone(&bell));
        bell
    }
    fn ring(&self) {
        let mut rung = self.rung.lock().unwrap();
        *rung = true;
        self.ringing.notify_one();
    }
    fn timer(&self) {
        let rung = self.rung.lock().unwrap();
        self.ringing.notify_one();
        drop(rung);
    }
    fn wait(&self, left: Duration) -> Result<bool, Refusal> {
        // The clock stands at 0 until a test moves it to a deadline, so what is left of a time
        // asked at 0 is that deadline.
        let end = u64::try_from(left.as_nanos()).unwrap();
        let mut rung = locked(&self.rung)?;
        while !*rung && CLOCK.now() < end {
            rung = self.ringing.wait(rung).map_err(unavailable)?;
        }
        Ok(std::mem::take(&mut *rung))
    }
}

/// The process's memory budget (`crate::memory`): its limit less what is charged, its waits the
/// budget's own (`wait`).
mod memory {
    use super::sync::{Arc, AtomicU64, Instant};
    pub(crate) use super::wait::Ending;
    use super::wait::{NoRoom, Waiters};
    use super::{CLOCK, Refusal};
    use std::sync::atomic::Ordering;

    #[derive(Clone, Copy)]
    pub(crate) enum Use {
        Queues,
    }
    #[derive(Clone)]
    pub(crate) struct Budget(Arc<Held>);
    struct Held {
        /// The room left.
        left: AtomicU64,
        waiters: Arc<Waiters>,
        limit: u64,
    }
    pub(crate) struct Charge {
        budget: Budget,
        bytes: u64,
    }
    impl Charge {
        pub(crate) fn bytes(&self) -> u64 {
            self.bytes
        }
    }
    impl Drop for Charge {
        fn drop(&mut self) {
            self.budget.0.left.fetch_add(self.bytes, Ordering::SeqCst);
            self.budget.0.waiters.given();
        }
    }
    impl Budget {
        /// The process's budget from here on, of `limit` bytes.
        pub(crate) fn of(limit: u64) -> Self {
            let budget = Self(Arc::new(Held {
                left: AtomicU64::new(limit),
                waiters: Waiters::new(),
                limit,
            }));
            CLOCK.budget.lock().unwrap().push(budget.clone());
            *super::BUDGET.lock().unwrap() = Some(budget.clone());
            budget
        }
        /// A timed wait's time passing: a holder looks again, as the timer's wake would have it.
        pub(crate) fn timer(&self) {
            self.0.waiters.look_again();
        }
        pub(crate) fn held(&self) -> u64 {
            self.0.limit - self.0.left.load(Ordering::SeqCst)
        }
        pub(crate) fn waiting(&self) -> u64 {
            self.0.waiters.holders()
        }
        fn too_large(bytes: u64, limit: u64) -> Refusal {
            Refusal::TooLarge {
                what: "memory for queues".into(),
                size: usize::try_from(bytes).unwrap(),
                limit: usize::try_from(limit).unwrap(),
            }
        }
        fn charged(&self, bytes: u64) -> Result<Charge, u64> {
            self.0
                .left
                .try_update(Ordering::AcqRel, Ordering::Acquire, |left| {
                    left.checked_sub(bytes)
                })
                .map(|_| Charge {
                    budget: self.clone(),
                    bytes,
                })
        }
        pub(crate) fn take(&self, _: Use, bytes: u64) -> Result<Charge, Refusal> {
            self.charged(bytes)
                .map_err(|left| Self::too_large(bytes, left))
        }
        pub(crate) fn take_until(
            &self,
            used: Use,
            bytes: u64,
            until: Instant,
            owner: &Ending,
            waits: impl FnMut(),
        ) -> Result<Charge, Refusal> {
            if let Ok(charge) = self.take(used, bytes) {
                return Ok(charge);
            }
            Waiters::until(
                &self.0.waiters,
                until,
                owner,
                || match self.charged(bytes) {
                    Ok(charge) => Ok(charge),
                    Err(_) if bytes > self.0.limit => {
                        Err(NoRoom::Refused(Self::too_large(bytes, self.0.limit)))
                    }
                    Err(left) => Err(NoRoom::Now(usize::try_from(left).unwrap())),
                },
                waits,
            )
        }
    }
    pub(crate) fn process() -> Budget {
        super::BUDGET.lock().unwrap().clone().unwrap()
    }
}

#[cfg(test)]
mod tests;
