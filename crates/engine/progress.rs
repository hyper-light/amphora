use crate::sync::{
    atomic::{AtomicBool, AtomicU64},
    RwLock,
};
use std::sync::atomic::Ordering;
use std::sync::OnceLock;
use std::time::Instant;

pub(crate) type ProgressHandlerCallback = Box<dyn Fn() -> bool + Send + Sync>;

/// Process-wide monotonic anchor for encoding absolute `Instant` deadlines into an `AtomicU64`
/// (nanoseconds since anchor; `0` = no deadline, `1` = already expired, `>= 2` = live deadline).
static DEADLINE_EPOCH: OnceLock<Instant> = OnceLock::new();
const DEADLINE_NONE: u64 = 0;
const DEADLINE_EXPIRED: u64 = 1;

fn min_nonzero(a: u64, b: u64) -> u64 {
    match (a, b) {
        (0, x) | (x, 0) => x,
        (x, y) => x.min(y),
    }
}

/// Connection-scoped progress callback and lock-free deadline state.
///
/// This models SQLite's `sqlite3_progress_handler()` contract for step-time
/// execution:
/// - one handler per connection
/// - the callback runs approximately every `N` virtual machine instructions
/// - a non-zero callback result interrupts the running operation
///
/// In addition, a monotonic wall-clock deadline can be installed via [`Self::set_deadline`]
/// without heap allocation or acquiring the callback lock.
#[derive(Default)]
pub(crate) struct ProgressHandler {
    callback: RwLock<Option<ProgressHandlerCallback>>,
    has_callback: AtomicBool,
    callback_ops: AtomicU64,
    deadline_nanos: AtomicU64,
    deadline_ops: AtomicU64,
    ops: AtomicU64,
}

impl std::fmt::Debug for ProgressHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgressHandler")
            .field("enabled", &self.is_enabled())
            .field("ops", &self.ops())
            .finish()
    }
}

impl ProgressHandler {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn refresh_ops(&self) {
        let effective = min_nonzero(
            self.callback_ops.load(Ordering::Acquire),
            self.deadline_ops.load(Ordering::Acquire),
        );
        self.ops.store(effective, Ordering::Release);
    }

    /// Install or clear the progress handler.
    ///
    /// SQLite disables the handler when `N < 1` or the callback is null, so we
    /// do the same here by clearing both the callback and opcode interval.
    pub(crate) fn set(&self, ops: u64, callback: Option<ProgressHandlerCallback>) {
        if ops == 0 || callback.is_none() {
            self.has_callback.store(false, Ordering::Release);
            *self.callback.write() = None;
            self.callback_ops.store(0, Ordering::Release);
            self.refresh_ops();
            return;
        }
        *self.callback.write() = callback;
        self.has_callback.store(true, Ordering::Release);
        self.callback_ops.store(ops, Ordering::Release);
        self.refresh_ops();
    }

    /// Install or clear a monotonic wall-clock deadline checked approximately every `ops` VM steps
    /// without allocating a closure or taking the callback lock.
    pub(crate) fn set_deadline(&self, ops: u64, until: Option<Instant>) {
        let Some(until) = until.filter(|_| ops > 0) else {
            self.deadline_nanos.store(DEADLINE_NONE, Ordering::Release);
            self.deadline_ops.store(0, Ordering::Release);
            self.refresh_ops();
            return;
        };
        let epoch = *DEADLINE_EPOCH.get_or_init(Instant::now);
        let encoded = match until.checked_duration_since(epoch) {
            None => DEADLINE_EXPIRED,
            Some(d) if d.is_zero() => DEADLINE_EXPIRED,
            Some(d) => u64::try_from(d.as_nanos()).unwrap_or(u64::MAX).max(2),
        };
        self.deadline_nanos.store(encoded, Ordering::Release);
        self.deadline_ops.store(ops, Ordering::Release);
        self.refresh_ops();
    }

    pub(crate) fn ops(&self) -> u64 {
        self.ops.load(Ordering::Acquire)
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.ops() != 0
    }

    /// Returns true when the callback or deadline requests interruption for the VM steps
    /// executed in `(prev_steps, vm_steps]`.
    ///
    /// The cadence is approximate by design, matching SQLite's documentation:
    /// the callback is consulted only when the VM-step count crosses a
    /// configured multiple of `ops`. Callers that consult this once per step
    /// pass `prev_steps = vm_steps - 1`; callers that batch several steps per
    /// consultation pass the count from the previous consultation.
    pub(crate) fn should_interrupt(&self, prev_steps: u64, vm_steps: u64) -> bool {
        let ops = self.ops.load(Ordering::Acquire);
        if ops == 0 || prev_steps / ops == vm_steps / ops {
            return false;
        }
        let deadline = self.deadline_nanos.load(Ordering::Acquire);
        if deadline != DEADLINE_NONE {
            if deadline == DEADLINE_EXPIRED {
                return true;
            }
            if let Some(epoch) = DEADLINE_EPOCH.get() {
                let now = u64::try_from(epoch.elapsed().as_nanos()).unwrap_or(u64::MAX);
                if now >= deadline {
                    return true;
                }
            }
        }
        if self.has_callback.load(Ordering::Acquire) {
            let callback = self.callback.read();
            if let Some(callback) = callback.as_ref() {
                return callback();
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[test]
    fn disabled_handler_never_interrupts() {
        let handler = ProgressHandler::new();
        assert!(!handler.should_interrupt(0, 1));
        assert!(!handler.is_enabled());
    }

    #[test]
    fn handler_runs_only_on_configured_interval() {
        let calls = Arc::new(AtomicUsize::new(0));
        let handler = ProgressHandler::new();
        let callback_calls = Arc::clone(&calls);
        handler.set(
            3,
            Some(Box::new(move || {
                callback_calls.fetch_add(1, Ordering::SeqCst);
                false
            })),
        );

        assert!(!handler.should_interrupt(0, 1));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(!handler.should_interrupt(1, 2));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(!handler.should_interrupt(2, 3));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!handler.should_interrupt(3, 4));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!handler.should_interrupt(5, 6));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn handler_can_request_interrupt() {
        let handler = ProgressHandler::new();
        handler.set(2, Some(Box::new(|| true)));

        assert!(!handler.should_interrupt(0, 1));
        assert!(handler.should_interrupt(1, 2));
    }

    #[test]
    fn disabling_clears_handler() {
        let calls = Arc::new(AtomicUsize::new(0));
        let handler = ProgressHandler::new();
        let callback_calls = Arc::clone(&calls);
        handler.set(
            1,
            Some(Box::new(move || {
                callback_calls.fetch_add(1, Ordering::SeqCst);
                true
            })),
        );
        assert!(handler.should_interrupt(0, 1));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        handler.set(0, None);
        assert!(!handler.should_interrupt(1, 2));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
