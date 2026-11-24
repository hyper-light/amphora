use crate::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use crate::sync::Mutex;
use crate::MonotonicInstant;
use crossbeam_utils::CachePadded;
use std::sync::Arc;
use std::task::{Wake, Waker};
use std::time::Duration;

/// Amphora (lane constants-22, goal "no picked constants"): how a statement that finds the database
/// busy waits under a timeout handler ([`BusyHandler::Timeout`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BusyWake {
    /// Until a lock it could be waiting on is released ([`LockRelease`]), with the busy timeout as
    /// its deadline only. A statement stepped without a waker, or on a log whose holders may be in
    /// another process (which no event here sees), waits on SQLite's schedule instead.
    #[default]
    Release,
    /// SQLite's schedule (`sqliteDefaultBusyCallback`: 1, 2, 5 ... 100 ms): the control.
    Schedule,
}

/// Amphora (lane constants-22): the event a busy statement waits on: a lock of the log's shared
/// state (the write lock, a read mark, the checkpoint lock, the vacuum lock) released, or a commit
/// published. Each release counts once (`generation`); a statement reads the count before it tries
/// its locks, so a release between its failed try and its wait is never missed: [`Self::wait`]
/// sees the count moved and the statement tries again at once. The wakers held are those of the
/// statements busy on this log now, at most one each (a waker already held isn't held twice).
#[derive(Debug)]
pub struct LockRelease {
    generation: AtomicU64,
    /// Packed waiter state kept on its own cache line so uncontended shared-read unlocks
    /// (`notify_shared`) stay read-only and never dirty `generation` across cores:
    /// - bit 0 ([`Self::ARMED_BIT`]): at least one waker is registered in `wakers`
    /// - bits 1..32 (step [`Self::INTEREST_INC`]): active [`SharedInterestGuard`] count
    ///
    /// Checking `wait_state != 0` in `notify_shared()` wakes both crate-internal retrying
    /// statements and direct public [`Self::wait`] callers without an extra cache-line read.
    wait_state: CachePadded<AtomicU32>,
    wakers: Mutex<Vec<Waker>>,
}

impl Default for LockRelease {
    fn default() -> Self {
        Self {
            generation: AtomicU64::new(0),
            wait_state: CachePadded::new(AtomicU32::new(0)),
            wakers: Mutex::new(Vec::new()),
        }
    }
}

/// RAII registration keeping [`LockRelease::notify_shared`] active while a busy statement retries
/// and waits for a lock release.
#[derive(Debug)]
pub struct SharedInterestGuard {
    release: Arc<LockRelease>,
}

impl Drop for SharedInterestGuard {
    fn drop(&mut self) {
        self.release
            .wait_state
            .fetch_sub(LockRelease::INTEREST_INC, Ordering::Release);
    }
}

impl LockRelease {
    const ARMED_BIT: u32 = 1;
    const INTEREST_INC: u32 = 2;

    /// The releases so far.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Holds `waker` for the next release after `seen` by anyone but this thread's own `step`:
    /// true. False when one came already: try again now.
    pub fn wait(&self, seen: u64, step: &OwnReleases, waker: &Waker) -> bool {
        let own = step.count(self);
        let mut wakers = self.wakers.lock();
        self.wait_state.fetch_or(Self::ARMED_BIT, Ordering::SeqCst);
        if self.generation.load(Ordering::SeqCst) != seen.wrapping_add(own) {
            if wakers.is_empty() {
                self.wait_state
                    .fetch_and(!Self::ARMED_BIT, Ordering::SeqCst);
            }
            return false;
        }
        if !wakers.iter().any(|held| held.will_wake(waker)) {
            wakers.push(waker.clone());
        }
        true
    }

    /// Each statement owns its registration even when two statements share a task (T-QB.1.13).
    pub(crate) fn subscribe(
        self: &Arc<Self>,
        seen: u64,
        step: &OwnReleases,
        waker: &Waker,
    ) -> Option<ReleaseWait> {
        let waker = Waker::from(Arc::new(StatementWake(waker.clone())));
        self.wait(seen, step, &waker).then(|| ReleaseWait {
            release: Arc::clone(self),
            waker,
        })
    }

    /// Starts counting the releases this thread makes of this log's locks, for a step that will
    /// [`Self::wait`] on the others'.
    pub fn own_releases(&self) -> OwnReleases {
        let key = self.key();
        let inert = OwnReleases {
            base: 0,
            restore: None,
        };
        OWN.try_with(|own| {
            let (held, made) = own.get();
            if held == key {
                OwnReleases {
                    base: made,
                    restore: None,
                }
            } else {
                own.set((key, 0));
                OwnReleases {
                    base: 0,
                    restore: Some((held, made)),
                }
            }
        })
        .unwrap_or(inert)
    }

    fn key(&self) -> usize {
        std::ptr::from_ref(self) as usize
    }

    /// Registers interest in shared-read lock releases (`notify_shared`) with `SeqCst` visibility
    /// so any reader that unlocks after a subsequent failed exclusive lock attempt observes the
    /// interest and calls `notify()`.
    pub fn interest(self: &Arc<Self>) -> SharedInterestGuard {
        self.wait_state
            .fetch_add(Self::INTEREST_INC, Ordering::SeqCst);
        SharedInterestGuard {
            release: Arc::clone(self),
        }
    }

    /// A shared-read lock was released: wakes waiters whenever any waker is armed or any
    /// contending caller has registered interest via [`Self::interest`]. With nobody waiting, this
    /// performs a single read of an uncontended cache line and avoids dirtying `generation`.
    #[inline]
    pub fn notify_shared(&self) {
        if self.wait_state.load(Ordering::SeqCst) != 0 {
            self.notify();
        }
    }

    /// A lock was released: wakes every statement waiting.
    pub fn notify(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let key = self.key();
        let _ = OWN.try_with(|own| {
            let (held, made) = own.get();
            if held == key {
                own.set((held, made.wrapping_add(1)));
            }
        });
        if (self.wait_state.load(Ordering::SeqCst) & Self::ARMED_BIT) == 0 {
            return;
        }
        let woken = {
            let mut wakers = self.wakers.lock();
            self.wait_state
                .fetch_and(!Self::ARMED_BIT, Ordering::SeqCst);
            std::mem::take(&mut *wakers)
        };
        for waker in woken {
            waker.wake();
        }
    }
}

struct StatementWake(Waker);

impl Wake for StatementWake {
    fn wake(self: Arc<Self>) {
        self.0.wake_by_ref();
    }
}

/// Dropping a pending step relinquishes its task, even if the lock holder never releases
/// (T-QB.1.13).
#[derive(Debug)]
pub(crate) struct ReleaseWait {
    release: Arc<LockRelease>,
    waker: Waker,
}

impl Drop for ReleaseWait {
    fn drop(&mut self) {
        let mut wakers = self.release.wakers.lock();
        wakers.retain(|held| !held.will_wake(&self.waker));
        if wakers.is_empty() {
            self.release
                .wait_state
                .fetch_and(!LockRelease::ARMED_BIT, Ordering::SeqCst);
        }
    }
}

crate::thread::thread_local! {
    /// The log whose releases this thread is counting (its [`LockRelease`]'s address, 0: none),
    /// and how many it has made since it began counting.
    static OWN: std::cell::Cell<(usize, u64)> = const { std::cell::Cell::new((0, 0)) };
}

/// Amphora (lane perf-A5, fixing lane constants-22's wait): the releases one step makes itself of
/// the log it may wait on. A write busy on another connection's write lock ends the read
/// transaction its failed upgrade began, and that release moves the count it read before trying;
/// counted as anyone's, its wait found a release every time and tried again at once, a spin
/// for as long as the other held the lock. Its own releases never free what it waits for, so the
/// wait counts only the others'. A step inside a step on the same log shares the outer's count;
/// one on another log counts its own and gives the outer's back when it ends.
pub struct OwnReleases {
    base: u64,
    restore: Option<(usize, u64)>,
}

impl OwnReleases {
    /// This thread's releases of `release` since [`LockRelease::own_releases`].
    fn count(&self, release: &LockRelease) -> u64 {
        OWN.try_with(|own| {
            let (held, made) = own.get();
            if held == release.key() {
                made.wrapping_sub(self.base)
            } else {
                0
            }
        })
        .unwrap_or(0)
    }
}

impl Drop for OwnReleases {
    fn drop(&mut self) {
        if let Some(outer) = self.restore {
            let _ = OWN.try_with(|own| own.set(outer));
        }
    }
}

/// Type alias for busy handler callback function.
///
/// The callback receives:
/// - `count`: The number of times the busy handler has been invoked for the same locking event
///
/// Returns:
/// - `0` to stop retrying and return SQLITE_BUSY to the application.
/// - Non-zero to retry the database access.
///
/// # Safety Notes (per SQLite spec)
/// - The callback MUST NOT modify the database connection that invoked it.
/// - The callback MUST NOT close the connection or any prepared statement.
/// - The callback is NOT reentrant.
pub type BusyHandlerCallback = Box<dyn Fn(i32) -> i32 + Send + Sync>;

#[derive(Default)]
/// Represents the busy handler configuration for a connection.
pub enum BusyHandler {
    #[default]
    /// No busy handler: return SQLITE_BUSY immediately on lock contention.
    None,
    /// Default timeout-based handler (implements sqliteDefaultBusyCallback)
    /// The duration is the maximum total time to wait before giving up
    Timeout(Duration),
    /// Custom user-defined callback handler
    Custom { callback: BusyHandlerCallback },
}

impl std::fmt::Debug for BusyHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BusyHandler::None => write!(f, "BusyHandler::None"),
            BusyHandler::Timeout(d) => write!(f, "BusyHandler::Timeout({d:?}"),
            BusyHandler::Custom { .. } => write!(f, "BusyHandler::Custom"),
        }
    }
}

/// Tracks the state of busy handler invocations for a statement.
///
/// This implements a yield-based busy handling mechanism that integrates with
/// the async event loop. Instead of blocking with `thread::sleep`, the statement
/// yields back to the caller with `StepResult::Sleep { duration }`. When `step()`
/// is called again after the timeout has passed, it retries the operation.
///
/// Uses increasing delays. After 12 iterations, continues with 100ms delays until max duration is reached.
#[derive(Debug)]
pub struct BusyHandlerState {
    /// Number of times the busy handler has been invoked for this locking event
    invocation_count: i32,
    /// For timeout-based handlers: the next timeout instant to wait until
    timeout: MonotonicInstant,
    /// For timeout-based handlers: the current iteration index into DELAYS
    iteration: usize,
    /// Amphora (lane constants-22): under [`BusyWake::Release`], when the first busy round came:
    /// the busy timeout counts from it.
    first: Option<MonotonicInstant>,
}

impl BusyHandlerState {
    /// Delay schedule for timeout-based busy handler (sqliteDefaultBusyCallback)
    const DELAYS: [Duration; 12] = [
        Duration::from_millis(1),
        Duration::from_millis(2),
        Duration::from_millis(5),
        Duration::from_millis(10),
        Duration::from_millis(15),
        Duration::from_millis(20),
        Duration::from_millis(25),
        Duration::from_millis(25),
        Duration::from_millis(25),
        Duration::from_millis(50),
        Duration::from_millis(50),
        Duration::from_millis(100),
    ];

    /// Cumulative totals for each iteration (for calculating remaining time)
    const TOTALS: [Duration; 12] = [
        Duration::from_millis(0),
        Duration::from_millis(1),
        Duration::from_millis(3),
        Duration::from_millis(8),
        Duration::from_millis(18),
        Duration::from_millis(33),
        Duration::from_millis(53),
        Duration::from_millis(78),
        Duration::from_millis(103),
        Duration::from_millis(128),
        Duration::from_millis(178),
        Duration::from_millis(228),
    ];

    /// Create a new busy handler state
    pub fn new(now: MonotonicInstant) -> Self {
        Self {
            invocation_count: 0,
            timeout: now,
            iteration: 0,
            first: None,
        }
    }

    /// Reset the state for a new locking event
    pub fn reset(&mut self, now: MonotonicInstant) {
        self.invocation_count = 0;
        self.timeout = now;
        self.iteration = 0;
        self.first = None;
    }

    /// Amphora (lane constants-22): under [`BusyWake::Release`], how long the statement may still
    /// wait for a release (about `Duration::MAX`: no deadline), or None when `max_duration`, counted from
    /// the first busy round, has passed. The statement steps again when woken, never held back by a
    /// delay of the schedule.
    pub fn until_released(
        &mut self,
        max_duration: Duration,
        now: MonotonicInstant,
    ) -> Option<Duration> {
        let first = *self.first.get_or_insert(now);
        self.invocation_count = self.invocation_count.saturating_add(1);
        self.timeout = now;
        let left = max_duration.saturating_sub(now.duration_since(first));
        if left.is_zero() {
            None
        } else {
            Some(left)
        }
    }

    /// Get the current timeout instant
    pub fn timeout(&self) -> MonotonicInstant {
        self.timeout
    }

    /// Invoke the busy handler and determine whether to retry.
    ///
    /// Returns `true` if the operation should be retried, `false` if SQLITE_BUSY
    /// should be returned to the application.
    ///
    /// For timeout-based handlers, this also updates the internal timeout instant.
    /// For custom handlers, this invokes the callback and respects its return value.
    pub fn invoke(&mut self, handler: &BusyHandler, now: MonotonicInstant) -> bool {
        match handler {
            BusyHandler::None => {
                // No handler: return BUSY immediately
                false
            }
            BusyHandler::Timeout(max_duration) => self.invoke_timeout_handler(*max_duration, now),
            BusyHandler::Custom { callback } => {
                let result = callback(self.invocation_count);
                self.invocation_count += 1;
                if result != 0 {
                    // Retry with a small delay
                    self.timeout = now + Duration::from_millis(1);
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Implements sqliteDefaultBusyCallback logic for timeout-based handling.
    ///
    /// This uses an exponentially increasing delay schedule, capped at 100ms per iteration.
    fn invoke_timeout_handler(&mut self, max_duration: Duration, now: MonotonicInstant) -> bool {
        let idx = self.iteration.min(11);
        let mut delay = Self::DELAYS[idx];
        let mut prior = Self::TOTALS[idx];

        // After 12 iterations, each additional iteration adds 100ms
        if self.iteration >= 12 {
            prior += delay * (self.iteration as u32 - 11);
        }

        // Check if we've exceeded or would exceed the max duration
        if prior + delay > max_duration {
            delay = max_duration.saturating_sub(prior);
            if delay.is_zero() {
                return false;
            }
        }

        self.iteration = self.iteration.saturating_add(1);
        self.invocation_count += 1;
        self.timeout = now + delay;
        true
    }

    /// Get the delay duration that should be waited before the next retry.
    ///
    /// This returns the duration between `now` and the timeout instant.
    /// Returns `Duration::ZERO` if the timeout has already passed.
    pub fn get_delay(&self, now: MonotonicInstant) -> Duration {
        if now >= self.timeout {
            Duration::ZERO
        } else {
            self.timeout.duration_since(now)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Counts the times it is woken.
    struct Count(std::sync::atomic::AtomicUsize);
    impl std::task::Wake for Count {
        fn wake(self: std::sync::Arc<Self>) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// T-QB.1.13 (lane constants-22): a release after the count was read is never missed (the
    /// wait says try again), a waiting waker is woken once by the next release and then let go,
    /// and one waker waiting twice is held once.
    #[test]
    fn constants_22_lock_release_wakes_its_waiters_once_and_misses_none() {
        let release = LockRelease::default();
        let count = std::sync::Arc::new(Count(std::sync::atomic::AtomicUsize::new(0)));
        let waker = Waker::from(std::sync::Arc::clone(&count));
        let seen = release.generation();
        release.notify();
        let step = release.own_releases();
        assert!(
            !release.wait(seen, &step, &waker),
            "a release since `seen`: try again now"
        );
        let seen = release.generation();
        assert!(release.wait(seen, &step, &waker));
        assert!(release.wait(seen, &step, &waker));
        assert_eq!(release.wakers.lock().len(), 1, "one waker held once");
        release.notify();
        assert_eq!(count.0.load(std::sync::atomic::Ordering::SeqCst), 1);
        release.notify();
        assert_eq!(
            count.0.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "woken wakers are let go"
        );
    }

    #[test]
    fn t_qb_1_13_a_cancelled_busy_wait_relinquishes_its_task() {
        let release = std::sync::Arc::new(LockRelease::default());
        let count = std::sync::Arc::new(Count(std::sync::atomic::AtomicUsize::new(0)));
        let task = std::sync::Arc::downgrade(&count);
        let waker = Waker::from(count);
        let step = release.own_releases();
        let seen = release.generation();
        let first = release.subscribe(seen, &step, &waker).unwrap();
        let second = release.subscribe(seen, &step, &waker).unwrap();
        drop(first);
        assert_eq!(
            release.wakers.lock().len(),
            1,
            "the other statement still waits"
        );
        std::thread::scope(|s| {
            s.spawn(|| release.notify());
        });
        assert_eq!(task.upgrade().unwrap().0.load(Ordering::SeqCst), 1);
        drop(second);
        let pending = release
            .subscribe(release.generation(), &step, &waker)
            .unwrap();
        drop(waker);
        drop(pending);
        assert!(
            task.upgrade().is_none(),
            "the abandoned wait kept its task alive"
        );
    }

    /// Lane perf-A5 (the db test constants_22 spun forever on this): a busy write's own releases
    /// (the read transaction its failed upgrade ends) never end its wait, and another thread's
    /// release still does. A step on another log inside it gives the outer's count back.
    #[test]
    fn a5_a_steps_own_releases_never_end_its_wait() {
        let release = LockRelease::default();
        let waker = Waker::from(std::sync::Arc::new(Count(
            std::sync::atomic::AtomicUsize::new(0),
        )));
        let step = release.own_releases();
        let seen = release.generation();
        release.notify();
        assert!(
            release.wait(seen, &step, &waker),
            "its own release: it waits for another's"
        );
        let other = LockRelease::default();
        let inner = other.own_releases();
        other.notify();
        drop(inner);
        release.notify();
        assert!(
            release.wait(seen, &step, &waker),
            "its own releases still counted after a step on another log"
        );
        std::thread::scope(|s| {
            s.spawn(|| release.notify());
        });
        assert!(
            !release.wait(seen, &step, &waker),
            "another thread's release since `seen`: try again now"
        );
    }

    /// Lane constants-22: under the release wake the busy timeout is a deadline only, counted from
    /// the first busy round: no delay of the schedule, and None once it has passed.
    #[test]
    fn constants_22_until_released_is_the_deadline_left() {
        let now = test_instant();
        let mut state = BusyHandlerState::new(now);
        let max = Duration::from_millis(100);
        assert_eq!(state.until_released(max, now), Some(max));
        assert_eq!(state.timeout(), now, "no delay before the next try");
        let later = now + Duration::from_millis(40);
        assert_eq!(
            state.until_released(max, later),
            Some(Duration::from_millis(60))
        );
        assert_eq!(state.until_released(max, now + max), None);
        assert!(state
            .until_released(Duration::MAX, now)
            .is_some_and(|left| left > max));
    }

    fn test_instant() -> MonotonicInstant {
        MonotonicInstant::now()
    }

    #[test]
    fn test_busy_handler_timeout_basic() {
        let handler = BusyHandler::Timeout(Duration::from_millis(100));
        let now = test_instant();
        let mut state = BusyHandlerState::new(now);

        // First invocation should return true (retry)
        assert!(state.invoke(&handler, now));
        // Timeout should be set to 1ms from now
        assert_eq!(state.timeout(), now + Duration::from_millis(1));
    }

    #[test]
    fn test_busy_handler_timeout_exhausted() {
        let handler = BusyHandler::Timeout(Duration::from_millis(0));
        let now = test_instant();
        let mut state = BusyHandlerState::new(now);

        // Zero timeout should return false immediately
        assert!(!state.invoke(&handler, now));
    }

    #[test]
    fn test_busy_handler_custom_callback() {
        // Callback that retries 3 times then gives up
        let callback: BusyHandlerCallback = Box::new(|count| if count < 3 { 1 } else { 0 });
        let handler = BusyHandler::Custom { callback };
        let now = test_instant();
        let mut state = BusyHandlerState::new(now);

        // First 3 invocations should retry
        assert!(state.invoke(&handler, now));
        assert!(state.invoke(&handler, now));
        assert!(state.invoke(&handler, now));
        // 4th invocation should return false
        assert!(!state.invoke(&handler, now));
    }

    #[test]
    fn test_busy_handler_none_returns_false_immediately() {
        let handler = BusyHandler::None;
        let now = test_instant();
        let mut state = BusyHandlerState::new(now);

        // None handler should always return false (don't retry)
        assert!(!state.invoke(&handler, now));
        // Even on subsequent invocations
        assert!(!state.invoke(&handler, now));
    }

    #[test]
    fn test_custom_callback_receives_correct_count() {
        use std::sync::{Arc, Mutex};

        // Track the counts passed to callback (using Arc+Mutex for Send+Sync)
        let counts = Arc::new(Mutex::new(Vec::new()));
        let counts_clone = counts.clone();

        let callback: BusyHandlerCallback = Box::new(move |count| {
            counts_clone.lock().unwrap().push(count);
            if count < 5 {
                1
            } else {
                0
            }
        });

        let handler = BusyHandler::Custom { callback };
        let now = test_instant();
        let mut state = BusyHandlerState::new(now);

        // Invoke 6 times
        for _ in 0..6 {
            state.invoke(&handler, now);
        }

        // Verify counts were 0, 1, 2, 3, 4, 5
        assert_eq!(*counts.lock().unwrap(), vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_custom_callback_always_retry() {
        // Callback that always retries
        let callback: BusyHandlerCallback = Box::new(|_| 1);
        let handler = BusyHandler::Custom { callback };
        let now = test_instant();
        let mut state = BusyHandlerState::new(now);

        // Should always return true
        for _ in 0..100 {
            assert!(state.invoke(&handler, now));
        }
    }

    #[test]
    fn test_custom_callback_never_retry() {
        // Callback that never retries
        let callback: BusyHandlerCallback = Box::new(|_| 0);
        let handler = BusyHandler::Custom { callback };
        let now = test_instant();
        let mut state = BusyHandlerState::new(now);

        // First invocation should return false
        assert!(!state.invoke(&handler, now));
    }

    #[test]
    fn test_custom_callback_sets_timeout() {
        let callback: BusyHandlerCallback = Box::new(|_| 1);
        let handler = BusyHandler::Custom { callback };
        let now = test_instant();
        let mut state = BusyHandlerState::new(now);

        assert!(state.invoke(&handler, now));
        // Custom callback sets 1ms timeout
        assert_eq!(state.timeout(), now + Duration::from_millis(1));
    }

    #[test]
    fn test_timeout_delay_schedule() {
        let handler = BusyHandler::Timeout(Duration::from_secs(10)); // Long timeout
        let now = test_instant();
        let mut state = BusyHandlerState::new(now);

        // Expected delays per iteration: 1, 2, 5, 10, 15, 20, 25, 25, 25, 50, 50, 100ms
        // The timeout is set to `now + delay` each time, so we check individual delays
        let expected_delays_ms: [u64; 12] = [1, 2, 5, 10, 15, 20, 25, 25, 25, 50, 50, 100];

        for (i, expected_ms) in expected_delays_ms.iter().enumerate() {
            assert!(state.invoke(&handler, now), "iteration {i} should retry");
            let timeout = state.timeout();
            assert_eq!(
                timeout,
                now + Duration::from_millis(*expected_ms),
                "iteration {i} should have delay of {expected_ms}ms"
            );
        }
    }

    #[test]
    fn test_timeout_caps_at_max_duration() {
        // 5ms timeout - should only allow a few iterations
        let handler = BusyHandler::Timeout(Duration::from_millis(5));
        let now = test_instant();
        let mut state = BusyHandlerState::new(now);

        // First iteration: 1ms delay (total: 1ms)
        assert!(state.invoke(&handler, now));
        // Second iteration: 2ms delay (total: 3ms)
        assert!(state.invoke(&handler, now));
        // Third iteration: would be 5ms but only 2ms left (total would be 8ms > 5ms)
        // So delay is capped to 2ms
        assert!(state.invoke(&handler, now));
        // Fourth iteration: no time left
        assert!(!state.invoke(&handler, now));
    }

    #[test]
    fn test_state_reset() {
        let handler = BusyHandler::Timeout(Duration::from_millis(100));
        let now = test_instant();
        let mut state = BusyHandlerState::new(now);

        // Invoke a few times
        state.invoke(&handler, now);
        state.invoke(&handler, now);
        state.invoke(&handler, now);

        // Reset
        let later = MonotonicInstant::now();
        state.reset(later);

        // Should be back to initial state
        assert_eq!(state.timeout(), later);
        assert!(state.invoke(&handler, later));
        // First delay after reset should be 1ms
        assert_eq!(state.timeout(), later + Duration::from_millis(1));
    }

    #[test]
    fn test_get_delay_when_timeout_passed() {
        let now = MonotonicInstant::now();
        let state = BusyHandlerState::new(now);

        // Timeout is at `now`, so any time >= now should return zero delay
        assert_eq!(state.get_delay(now), Duration::ZERO);

        // A later time should also return zero
        std::thread::sleep(Duration::from_micros(10));
        let later = MonotonicInstant::now();
        assert_eq!(state.get_delay(later), Duration::ZERO);
    }

    #[test]
    fn test_get_delay_calculates_remaining_time() {
        let now = MonotonicInstant::now();
        let mut state = BusyHandlerState::new(now);

        let handler = BusyHandler::Timeout(Duration::from_millis(100));
        state.invoke(&handler, now); // Sets timeout to now + 1ms

        // Check delay from `now` - should be 1ms
        let delay = state.get_delay(now);
        assert_eq!(delay, Duration::from_millis(1));
    }

    /// P2 regression: a direct public `LockRelease::wait` caller that did not acquire a
    /// `SharedInterestGuard` must still be woken when the last reader calls `notify_shared()`.
    #[test]
    fn p2_public_wait_is_woken_by_notify_shared_without_interest_guard() {
        let release = LockRelease::default();
        let count = std::sync::Arc::new(Count(std::sync::atomic::AtomicUsize::new(0)));
        let waker = Waker::from(std::sync::Arc::clone(&count));
        let step = release.own_releases();
        let seen = release.generation();
        assert!(release.wait(seen, &step, &waker));
        std::thread::scope(|s| {
            s.spawn(|| release.notify_shared());
        });
        assert_eq!(
            count.0.load(Ordering::SeqCst),
            1,
            "public waiter must be woken by notify_shared()"
        );
    }
}
