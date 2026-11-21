//! The process's descriptor budget (docs/design/local-owner-transport.md, "One descriptor budget"):
//! the file descriptors this process may still open, read once from the system's own limit, that
//! what Amphora holds open charges as it opens it. Connections accepted by the local owners charge
//! one each (their socket), so no count of connections is picked: the system's limit is the bound.
//!
//! [`Descriptors`] is the pure count; [`process`] reads the system once.

use amphora_api::Refusal;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

/// A count of descriptors that may be held at once, and those held now.
pub(crate) struct Descriptors {
    bound: usize,
    held: AtomicUsize,
}

/// Descriptors held against a budget, given back when this drops.
pub(crate) struct Held {
    budget: Arc<Descriptors>,
    count: usize,
}

impl Descriptors {
    pub(crate) fn new(bound: usize) -> Arc<Self> {
        Arc::new(Self {
            bound,
            held: AtomicUsize::new(0),
        })
    }

    /// The most descriptors that may be held at once.
    pub(crate) fn bound(&self) -> usize {
        self.bound
    }

    /// The descriptors held now.
    #[cfg(test)]
    pub(crate) fn held(&self) -> usize {
        self.held.load(Ordering::Acquire)
    }

    /// `count` more held, refused typed (`what`, past the bound) when they would pass it.
    pub(crate) fn take(self: &Arc<Self>, what: &str, count: usize) -> Result<Held, Refusal> {
        let mut past = 0;
        self.held
            .try_update(Ordering::AcqRel, Ordering::Acquire, |held| {
                let after = held.checked_add(count);
                past = after.unwrap_or(usize::MAX);
                after.filter(|after| *after <= self.bound)
            })
            .map_err(|_| Refusal::TooLarge {
                what: what.into(),
                size: past,
                limit: self.bound,
            })?;
        Ok(Held {
            budget: Arc::clone(self),
            count,
        })
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        // Never below none: each give-back follows its own take.
        let _given = self
            .budget
            .held
            .try_update(Ordering::AcqRel, Ordering::Acquire, |held| {
                Some(held.saturating_sub(self.count))
            });
    }
}

/// The process's budget: the system's limit less the descriptors open when it was first asked
/// for ([`room`]); a test's own, where it set one for what it opens on its thread.
pub(crate) fn process() -> Result<Arc<Descriptors>, Refusal> {
    #[cfg(test)]
    if let Some(budget) = tests::BUDGET.with(|budget| budget.borrow().clone()) {
        return Ok(budget);
    }
    static PROCESS: OnceLock<Result<Arc<Descriptors>, String>> = OnceLock::new();
    PROCESS
        .get_or_init(|| room().map(Descriptors::new))
        .clone()
        .map_err(Refusal::Unavailable)
}

/// The descriptors this process may still open: the system's limit (`limit`) less those open now
/// (`open`). Descriptors opened later by code that charges no budget (the host's own) come out of
/// the same limit: past it the system refuses the open (EMFILE), an error returned typed where it
/// happens.
fn room() -> Result<usize, String> {
    Ok(limit().saturating_sub(open()?))
}

/// The soft `RLIMIT_NOFILE` (POSIX `getrlimit`): the most descriptors the system lets this process
/// hold. With none set (`RLIM_INFINITY`) nothing here bounds them, and the kernel's own ceiling
/// (Linux's `fs.nr_open`) refuses past it.
#[cfg(unix)]
fn limit() -> usize {
    rustix::process::getrlimit(rustix::process::Resource::Nofile)
        .current
        .map_or(usize::MAX, |soft| {
            usize::try_from(soft).unwrap_or(usize::MAX)
        })
}

/// Windows has no descriptor limit a process can set: its handle table holds at most 2^24
/// handles a process (Mark Russinovich, "Pushing the Limits of Windows: Handles", Microsoft,
/// 2009, the handle table's three levels), and Winsock's sockets are handles out of the same table
/// (`WSADATA::iMaxSockets` is ignored since Winsock 2, Microsoft's WSAStartup documentation).
#[cfg(windows)]
fn limit() -> usize {
    1 << 24
}

/// The descriptors this process holds now: the entries of `/dev/fd` (Linux's link to
/// `/proc/self/fd`, macOS's fdesc), the one this count opens among them, so it counts one more
/// than it leaves.
#[cfg(unix)]
fn open() -> Result<usize, String> {
    let entries = std::fs::read_dir("/dev/fd")
        .map_err(|error| format!("this process's open descriptors (/dev/fd): {error}"))?;
    Ok(entries.count())
}

/// Windows says a process's open handles only through `GetProcessHandleCount`, which this crate
/// can't call without `unsafe` (CLAUDE.md section 12): none counted, so a handle the process holds
/// already comes out of the same table, and the system refuses past it.
#[cfg(windows)]
fn open() -> Result<usize, String> {
    Ok(0)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    std::thread_local! {
        /// A test's own budget, for what it opens on its thread.
        pub(crate) static BUDGET: std::cell::RefCell<Option<Arc<Descriptors>>> =
            const { std::cell::RefCell::new(None) };
    }

    /// Sets this thread's budget to one of `bound` descriptors, for what the test opens on it.
    pub(crate) fn budget(bound: usize) -> Arc<Descriptors> {
        let budget = Descriptors::new(bound);
        BUDGET.with(|set| *set.borrow_mut() = Some(Arc::clone(&budget)));
        budget
    }

    /// AC-FD.1.1, T-FD.1.1 (local-owner-transport.md, "One descriptor budget"): a budget holds what it was
    /// given and no more. Do: take a budget of 3 one, then two; ask one more; give the two back
    /// and ask two. Expect: the first three held; the fourth refused, naming what it was for, 4
    /// past 3; once given back, two more held.
    #[test]
    fn a_budget_holds_its_bound_and_takes_back_what_is_given() {
        let budget = Descriptors::new(3);
        let one = budget.take("a connection", 1).unwrap();
        let two = budget.take("a connection", 2).unwrap();
        let refused = budget.take("a connection", 1).err().unwrap();
        assert_eq!(
            refused,
            Refusal::TooLarge {
                what: "a connection".into(),
                size: 4,
                limit: 3
            }
        );
        drop(two);
        let _again = budget.take("a connection", 2).unwrap();
        drop(one);
        assert!(budget.take("a connection", 2).is_err());
    }

    /// AC-FD.1.1, T-FD.1.2 (local-owner-transport.md, "One descriptor budget"): the process's budget is the
    /// system's limit less what the process holds. Do: read the budget's room on Unix. Expect: some
    /// room, under the soft RLIMIT_NOFILE by stdin, stdout and stderr at least.
    #[cfg(unix)]
    #[test]
    fn the_process_budget_is_the_systems_limit_less_what_is_open() {
        let (soft, room) = (limit(), room().unwrap());
        assert!(
            room > 0 && soft.saturating_sub(room) >= 3,
            "room {room} of {soft}"
        );
    }
}
