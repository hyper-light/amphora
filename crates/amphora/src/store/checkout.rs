//! Amphora (G3 step 6, prototype F): a read takes its reader without a lock or a mutex every reader
//! shares (docs/design/store.md decision 43; docs/research/read-path.md, the 32-client profile).
//!
//! The pool (`Readers::with`'s other way) takes the pool's mutex to take a reader and again to give
//! it back, wakes the waiters, and holds the read side of one `RwLock` for the read: three writes
//! per read to lines every reader writes, which move between their cores at 32 clients (decision
//! 22's effect, LeanStore's rule: "programs that frequently write to memory locations accessed by
//! multiple threads do not scale"). Here each reader lives in a slot of its own, on a line of its
//! own; a thread takes the slot its number names, so at most as many threads as slots each write
//! only their own slot's atomic state. When all slots are busy, waiters join a single global FIFO
//! wait queue so a long read on one slot never blocks short reads from receiving any other slot
//! that frees up; an uncontended read or release never touches the wait queue lock.
//!
//! Holding a slot is what holding the gate's read side was: the writer's restart of the log and
//! the readers' release take every slot, so no read is under way meanwhile.

use std::cell::Cell;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering, fence};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread::{Thread, ThreadId};
use std::time::Instant;

/// How a read takes its reader (the prototype's switch; `AMPHORA_READ_CHECKOUT` sets it for a
/// process: `pool` or `slots`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ReadCheckout {
    /// Today's: one pool behind a mutex, and a shared gate held through each read.
    Pool = 0,
    /// A slot a reader, on a line of its own, taken by the thread whose number names it.
    Slots = 1,
}

/// `set_process_default`'s choice; `UNSET` until it is called.
static PROCESS_DEFAULT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(UNSET);
const UNSET: u8 = u8::MAX;

impl ReadCheckout {
    /// The way stores started from now on take: `set_process_default`'s if called, else
    /// `AMPHORA_READ_CHECKOUT` (read once), else `Pool`.
    pub fn process_default() -> Self {
        static FROM_ENV: std::sync::OnceLock<ReadCheckout> = std::sync::OnceLock::new();
        match PROCESS_DEFAULT.load(Ordering::Relaxed) {
            UNSET => {
                *FROM_ENV.get_or_init(|| match std::env::var("AMPHORA_READ_CHECKOUT").as_deref() {
                    Ok("slots") => Self::Slots,
                    _ => Self::Pool,
                })
            }
            1 => Self::Slots,
            _ => Self::Pool,
        }
    }

    /// Sets the way stores started from now on in this process take (a bench's or a test's switch).
    pub fn set_process_default(mode: Self) {
        PROCESS_DEFAULT.store(mode as u8, Ordering::Relaxed);
    }
}

const SLOT_FREE: u8 = 0;
const SLOT_HELD: u8 = 1;

/// A slot, alone on its cache line pair (the adjacent-line prefetcher pulls lines in pairs, so
/// 128 bytes, as crossbeam's `CachePadded` aligns on x86_64 and aarch64). Each slot owns its own
/// atomic state and reader mutex so threads on disjoint slots share no locks and no writable cache
/// lines while uncontended.
#[repr(align(128))]
struct Slot<R> {
    state: AtomicU8,
    reader: Mutex<Option<R>>,
}

/// Cache-line-isolated wait coordination shared only when all slots are busy or `take_all()` is
/// draining. Uncontended reads and releases only perform shared-clean loads of `waiting` and
/// `draining` without writing this cache line or taking `waiters`.
#[repr(align(128))]
struct WaitState {
    /// Number of live read waiters queued in `waiters` (plus transient +1 during enqueue Dekker
    /// re-check).
    waiting: AtomicUsize,
    /// Number of active `take_all()` drainers.
    draining: AtomicUsize,
    /// Global FIFO wait queue across all slots, plus `take_all()` drainer registrations.
    waiters: Mutex<WaitQueue>,
}

struct WaiterSlot {
    thread: Option<Thread>,
    assigned: Option<usize>,
    prev: Option<usize>,
    next: Option<usize>,
    queued: bool,
}

#[derive(Default)]
struct WaitQueue {
    /// Head and tail of the intrusive FIFO doubly-linked list inside `entries` so any waiter
    /// timing out at any position unlinks and recycles its slot in O(1) without growing the queue.
    head: Option<usize>,
    tail: Option<usize>,
    /// Reusable waiter slots (bounded by peak concurrent live waiters).
    entries: Vec<WaiterSlot>,
    /// Free indices in `entries`.
    free: Vec<usize>,
    /// Threads currently inside `take_all()` waiting for a specific slot index to become free,
    /// deduplicated by `(slot_idx, ThreadId)`.
    drain_waiters: Vec<(usize, Thread)>,
}

impl WaitQueue {
    fn push_back(&mut self, thread: Thread) -> usize {
        let prev = self.tail;
        let id = match self.free.pop() {
            Some(id) => {
                if let Some(entry) = self.entries.get_mut(id) {
                    entry.thread = Some(thread);
                    entry.assigned = None;
                    entry.prev = prev;
                    entry.next = None;
                    entry.queued = true;
                }
                id
            }
            None => {
                let id = self.entries.len();
                self.entries.push(WaiterSlot {
                    thread: Some(thread),
                    assigned: None,
                    prev,
                    next: None,
                    queued: true,
                });
                id
            }
        };
        if let Some(prev_id) = prev {
            if let Some(prev_entry) = self.entries.get_mut(prev_id) {
                prev_entry.next = Some(id);
            }
        } else {
            self.head = Some(id);
        }
        self.tail = Some(id);
        id
    }

    fn pop_front(&mut self) -> Option<usize> {
        let front_id = self.head?;
        let next = self.entries.get(front_id).and_then(|e| e.next);
        self.head = next;
        if let Some(next_id) = next {
            if let Some(next_entry) = self.entries.get_mut(next_id) {
                next_entry.prev = None;
            }
        } else {
            self.tail = None;
        }
        if let Some(entry) = self.entries.get_mut(front_id) {
            entry.prev = None;
            entry.next = None;
            entry.queued = false;
        }
        Some(front_id)
    }

    fn unlink(&mut self, id: usize) -> bool {
        let Some((prev, next, queued)) = self
            .entries
            .get(id)
            .map(|entry| (entry.prev, entry.next, entry.queued))
        else {
            return false;
        };
        if !queued {
            return false;
        }
        if let Some(entry) = self.entries.get_mut(id) {
            entry.prev = None;
            entry.next = None;
            entry.queued = false;
        }
        if let Some(prev_id) = prev {
            if let Some(prev_entry) = self.entries.get_mut(prev_id) {
                prev_entry.next = next;
            }
        } else {
            self.head = next;
        }
        if let Some(next_id) = next {
            if let Some(next_entry) = self.entries.get_mut(next_id) {
                next_entry.prev = prev;
            }
        } else {
            self.tail = prev;
        }
        true
    }

    fn recycle(&mut self, id: usize) {
        if let Some(entry) = self.entries.get_mut(id) {
            entry.thread = None;
            entry.assigned = None;
            entry.prev = None;
            entry.next = None;
            entry.queued = false;
            self.free.push(id);
        }
    }

    fn register_drainer(&mut self, slot_idx: usize, thread: &Thread, tid: ThreadId) {
        if !self
            .drain_waiters
            .iter()
            .any(|(idx, t)| *idx == slot_idx && t.id() == tid)
        {
            self.drain_waiters.push((slot_idx, thread.clone()));
        }
    }

    fn unregister_drainer(&mut self, slot_idx: usize, tid: ThreadId) {
        self.drain_waiters
            .retain(|(idx, t)| !(*idx == slot_idx && t.id() == tid));
    }

    fn take_drainers_for(&mut self, slot_idx: usize) -> Vec<Thread> {
        let mut wake = Vec::new();
        let mut keep = Vec::with_capacity(self.drain_waiters.len());
        for (idx, thread) in self.drain_waiters.drain(..) {
            if idx == slot_idx {
                wake.push(thread);
            } else {
                keep.push((idx, thread));
            }
        }
        self.drain_waiters = keep;
        wake
    }
}

/// The slots and their shared global FIFO wait queue.
pub(super) struct Slots<R> {
    slots: Box<[Slot<R>]>,
    wait: WaitState,
}

/// A slot taken: its index, its reader (None until one is opened in it), and the slots, to hand
/// it off or release it when dropped.
pub(super) struct Taken<'a, R> {
    idx: usize,
    slot: Option<MutexGuard<'a, Option<R>>>,
    slots: &'a Slots<R>,
}

impl<R> Taken<'_, R> {
    /// The slot's reader: None until one is opened in it.
    pub(super) fn reader(&mut self) -> Option<&mut Option<R>> {
        self.slot.as_deref_mut()
    }
}

impl<R> Drop for Taken<'_, R> {
    fn drop(&mut self) {
        // Unlock the reader mutex first while `state == SLOT_HELD`, then hand off directly to the
        // globally oldest queued waiter or mark `SLOT_FREE`.
        drop(self.slot.take());
        self.slots.release_slot(self.idx);
    }
}

/// Each thread's number, taken once from a counter (a write to a shared line once per thread,
/// never per read).
fn thread_number() -> usize {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    thread_local! {
        static NUMBER: Cell<Option<usize>> = const { Cell::new(None) };
    }
    NUMBER.with(|n| match n.get() {
        Some(v) => v,
        None => {
            let v = NEXT.fetch_add(1, Ordering::Relaxed);
            n.set(Some(v));
            v
        }
    })
}

thread_local! {
    static LAST_SLOT: Cell<usize> = const { Cell::new(usize::MAX) };
}

impl<R> Slots<R> {
    /// `most` slots, at least one.
    pub(super) fn new(most: usize) -> Self {
        Self {
            slots: (0..most.max(1))
                .map(|_| Slot {
                    state: AtomicU8::new(SLOT_FREE),
                    reader: Mutex::new(None),
                })
                .collect(),
            wait: WaitState {
                waiting: AtomicUsize::new(0),
                draining: AtomicUsize::new(0),
                waiters: Mutex::new(WaitQueue::default()),
            },
        }
    }

    fn lock_reader(slot: &Slot<R>) -> MutexGuard<'_, Option<R>> {
        slot.reader.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_waiters(&self) -> MutexGuard<'_, WaitQueue> {
        self.wait
            .waiters
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    #[inline]
    fn home_index(&self) -> usize {
        let len = self.slots.len();
        thread_number().checked_rem(len).unwrap_or(0)
    }

    /// Claims one free slot via atomic CAS without writing cache lines of already-held slots.
    fn try_claim_any_free(&self, first: usize) -> Option<usize> {
        let len = self.slots.len();
        for offset in 0..len {
            let idx = first.checked_add(offset).map_or(0, |sum| {
                if sum < len {
                    sum
                } else {
                    sum.saturating_sub(len)
                }
            });
            let Some(slot) = self.slots.get(idx) else {
                continue;
            };
            if slot.state.load(Ordering::Relaxed) != SLOT_FREE {
                continue;
            }
            if slot
                .state
                .compare_exchange(SLOT_FREE, SLOT_HELD, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return Some(idx);
            }
        }
        None
    }

    /// Claims one free slot via atomic CAS before `until`.
    fn try_claim_free_before(&self, first: usize, until: Instant) -> Option<usize> {
        let len = self.slots.len();
        for offset in 0..len {
            let idx = first.checked_add(offset).map_or(0, |sum| {
                if sum < len {
                    sum
                } else {
                    sum.saturating_sub(len)
                }
            });
            let Some(slot) = self.slots.get(idx) else {
                continue;
            };
            if slot.state.load(Ordering::Relaxed) != SLOT_FREE {
                continue;
            }
            if until.saturating_duration_since(Instant::now()).is_zero() {
                return None;
            }
            if slot
                .state
                .compare_exchange(SLOT_FREE, SLOT_HELD, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return Some(idx);
            }
        }
        None
    }

    /// Confirms the deadline has not passed AFTER claiming `idx` (with no queue locks held).
    /// Because `release_slot` happens-before our `Acquire` CAS which happens-before this clock
    /// read, any slot first released after `until` is guaranteed to be rejected and released here.
    #[inline]
    fn finish_claimed_before(&self, idx: usize, until: Instant) -> Option<Taken<'_, R>> {
        if until.saturating_duration_since(Instant::now()).is_zero() {
            self.release_slot(idx);
            return None;
        }
        self.wrap_claimed(idx)
    }

    fn wrap_claimed(&self, idx: usize) -> Option<Taken<'_, R>> {
        let slot = self.slots.get(idx)?;
        LAST_SLOT.with(|last| last.set(idx));
        Some(Taken {
            idx,
            slot: Some(Self::lock_reader(slot)),
            slots: self,
        })
    }

    /// Hands off held slot `assigned_idx` directly to the globally oldest live FIFO waiter.
    fn handoff_front(&self, queue: &mut WaitQueue, assigned_idx: usize) -> Option<Thread> {
        let front_id = queue.pop_front()?;
        self.wait.waiting.fetch_sub(1, Ordering::SeqCst);
        if let Some(entry) = queue.entries.get_mut(front_id) {
            entry.assigned = Some(assigned_idx);
            entry.thread.take()
        } else {
            None
        }
    }

    /// A slot: a free one at once when uncontended (zero shared locks or shared cache-line
    /// writes), or handed off in global FIFO order as soon as ANY slot frees before `until`
    /// (None past it).
    pub(super) fn take(&self, until: Instant) -> Option<Taken<'_, R>> {
        if until.saturating_duration_since(Instant::now()).is_zero() {
            return None;
        }
        let home_idx = self.home_index();
        let start_idx = LAST_SLOT.with(|last| {
            let v = last.get();
            if v < self.slots.len() { v } else { home_idx }
        });
        if self.wait.draining.load(Ordering::Acquire) == 0
            && self.wait.waiting.load(Ordering::Acquire) == 0
            && let Some(idx) = self.try_claim_free_before(start_idx, until)
        {
            return self.finish_claimed_before(idx, until);
        }
        if until.saturating_duration_since(Instant::now()).is_zero() {
            return None;
        }

        let mut to_wake = Vec::new();
        let waiter_id = {
            let mut queue = self.lock_waiters();
            if until.saturating_duration_since(Instant::now()).is_zero() {
                return None;
            }
            // Publish our intent to wait (SeqCst) BEFORE scanning slots so any concurrent
            // `release_slot` either frees its slot before this scan sees it, or observes
            // `waiting > 0` after its SeqCst fence and hands the slot off under this lock.
            self.wait.waiting.fetch_add(1, Ordering::SeqCst);
            while self.wait.draining.load(Ordering::SeqCst) == 0 && queue.head.is_some() {
                let Some(idx) = self.try_claim_any_free(start_idx) else {
                    break;
                };
                if let Some(thread) = self.handoff_front(&mut queue, idx) {
                    to_wake.push(thread);
                } else {
                    drop(queue);
                    for thread in to_wake {
                        thread.unpark();
                    }
                    return self.finish_claimed_before(idx, until);
                }
            }
            if self.wait.draining.load(Ordering::SeqCst) == 0
                && queue.head.is_none()
                && let Some(idx) = self.try_claim_free_before(start_idx, until)
            {
                self.wait.waiting.fetch_sub(1, Ordering::SeqCst);
                drop(queue);
                for thread in to_wake {
                    thread.unpark();
                }
                return self.finish_claimed_before(idx, until);
            }
            queue.push_back(std::thread::current())
        };
        for thread in to_wake {
            thread.unpark();
        }

        loop {
            let left = until.saturating_duration_since(Instant::now());
            if !left.is_zero() {
                std::thread::park_timeout(left);
            }
            let mut queue = self.lock_waiters();
            // Sample expiry AFTER acquiring the queue lock so we never accept a slot that became
            // available after our deadline while we waited for the mutex.
            let expired = until.saturating_duration_since(Instant::now()).is_zero();
            let assigned = queue
                .entries
                .get_mut(waiter_id)
                .and_then(|entry| entry.assigned.take());
            if let Some(idx) = assigned {
                queue.recycle(waiter_id);
                drop(queue);
                if expired {
                    self.release_slot(idx);
                    return None;
                }
                return self.finish_claimed_before(idx, until);
            }
            if expired {
                if queue.unlink(waiter_id) {
                    self.wait.waiting.fetch_sub(1, Ordering::SeqCst);
                }
                queue.recycle(waiter_id);
                return None;
            }
            if queue.head == Some(waiter_id)
                && self.wait.draining.load(Ordering::SeqCst) == 0
                && let Some(idx) = self.try_claim_free_before(start_idx, until)
            {
                if queue.unlink(waiter_id) {
                    self.wait.waiting.fetch_sub(1, Ordering::SeqCst);
                }
                queue.recycle(waiter_id);
                drop(queue);
                return self.finish_claimed_before(idx, until);
            }
            if let Some(entry) = queue.entries.get_mut(waiter_id)
                && entry.thread.is_none()
            {
                entry.thread = Some(std::thread::current());
            }
        }
    }

    /// Releases held slot `idx`. When waiters are queued and no `take_all()` drainer is active,
    /// hands `idx` directly to the globally oldest FIFO waiter; otherwise marks `SLOT_FREE` and
    /// performs a Dekker re-check for racing drainers or newly queued waiters.
    fn release_slot(&self, idx: usize) {
        let Some(slot) = self.slots.get(idx) else {
            return;
        };
        if self.wait.draining.load(Ordering::Acquire) == 0
            && self.wait.waiting.load(Ordering::Acquire) > 0
        {
            let wake = {
                let mut queue = self.lock_waiters();
                if self.wait.draining.load(Ordering::Acquire) == 0 {
                    self.handoff_front(&mut queue, idx)
                } else {
                    None
                }
            };
            if let Some(thread) = wake {
                thread.unpark();
                return;
            }
        }

        slot.state.store(SLOT_FREE, Ordering::Release);
        fence(Ordering::SeqCst);

        if self.wait.draining.load(Ordering::Acquire) > 0 {
            let drainers = {
                let mut queue = self.lock_waiters();
                queue.take_drainers_for(idx)
            };
            for thread in drainers {
                thread.unpark();
            }
            return;
        }

        if self.wait.waiting.load(Ordering::Acquire) > 0 {
            let wake = {
                let mut queue = self.lock_waiters();
                if self.wait.draining.load(Ordering::Acquire) == 0
                    && queue.head.is_some()
                    && slot
                        .state
                        .compare_exchange(
                            SLOT_FREE,
                            SLOT_HELD,
                            Ordering::Acquire,
                            Ordering::Relaxed,
                        )
                        .is_ok()
                {
                    self.handoff_front(&mut queue, idx)
                } else {
                    None
                }
            };
            if let Some(thread) = wake {
                thread.unpark();
            }
        }
    }

    /// Every slot, each once no read holds it: no read is under way while they are held, and the
    /// reads that came meanwhile are handed slots in global FIFO order when they are given back.
    pub(super) fn take_all(&self) -> All<'_, R> {
        self.wait.draining.fetch_add(1, Ordering::SeqCst);
        let me = std::thread::current();
        let me_id = me.id();
        let mut held = Vec::with_capacity(self.slots.len());
        for (idx, slot) in self.slots.iter().enumerate() {
            let mut registered = false;
            loop {
                if slot
                    .state
                    .compare_exchange(SLOT_FREE, SLOT_HELD, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    if registered {
                        self.lock_waiters().unregister_drainer(idx, me_id);
                    }
                    break;
                }
                {
                    let mut queue = self.lock_waiters();
                    if slot
                        .state
                        .compare_exchange(
                            SLOT_FREE,
                            SLOT_HELD,
                            Ordering::Acquire,
                            Ordering::Relaxed,
                        )
                        .is_ok()
                    {
                        if registered {
                            queue.unregister_drainer(idx, me_id);
                        }
                        break;
                    }
                    queue.register_drainer(idx, &me, me_id);
                    registered = true;
                }
                std::thread::park();
            }
            held.push((idx, Self::lock_reader(slot)));
        }
        All { held, slots: self }
    }
}

/// Every slot held (`Slots::take_all`, by the log's restart and the readers' release). A read that
/// found them all held queues in global FIFO order, and giving them back hands slots to waiting
/// reads.
pub(super) struct All<'a, R> {
    held: Vec<(usize, MutexGuard<'a, Option<R>>)>,
    slots: &'a Slots<R>,
}

impl<R> All<'_, R> {
    /// Each slot's reader (None where none was opened).
    pub(super) fn readers(&mut self) -> impl Iterator<Item = &mut Option<R>> {
        self.held.iter_mut().map(|(_, slot)| &mut **slot)
    }
}

impl<R> Drop for All<'_, R> {
    fn drop(&mut self) {
        let indices: Vec<usize> = self.held.drain(..).map(|(idx, _)| idx).collect();
        let _ = self.slots.wait.draining.fetch_sub(1, Ordering::SeqCst);
        for idx in indices {
            self.slots.release_slot(idx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Readers, Refused, locked};
    use super::ReadCheckout;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::{Duration, Instant};

    /// A reader: its number, and a flag set while a read uses it.
    struct Reader {
        busy: Arc<AtomicBool>,
        dropped: Arc<AtomicUsize>,
    }

    impl Drop for Reader {
        fn drop(&mut self) {
            self.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn readers(most: usize) -> (Arc<Readers<Reader>>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let readers = Arc::new(Readers::with_checkout(
            most,
            Arc::default(),
            ReadCheckout::Slots,
        ));
        let (opened, dropped) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (o, d) = (opened.clone(), dropped.clone());
        *locked(&readers.open) = Some(Arc::new(move || {
            o.fetch_add(1, Ordering::SeqCst);
            Ok(Reader {
                busy: Arc::new(AtomicBool::new(false)),
                dropped: d.clone(),
            })
        }));
        (readers, opened, dropped)
    }

    fn far() -> Instant {
        Instant::now() + Duration::from_secs(600)
    }

    fn park_for(duration: Duration) {
        let target = Instant::now() + duration;
        while Instant::now() < target {
            let remaining = target.saturating_duration_since(Instant::now());
            if !remaining.is_zero() {
                std::thread::park_timeout(remaining);
            }
        }
    }

    /// G3 step 6 (prototype F, T-G3.6.3). Do: 16 threads read 500 times each through 4 slots. Expect: every
    /// read answered, at most 4 readers opened, and no reader used by two reads at once.
    #[test]
    fn g3_s6_slot_readers_serve_many_threads_one_read_a_reader_at_a_time() {
        let (readers, opened, _) = readers(4);
        let threads: Vec<_> = (0..16)
            .map(|_| {
                let readers = readers.clone();
                std::thread::spawn(move || {
                    for _ in 0..500 {
                        let answer = readers.with(far(), |r| {
                            let was = r.busy.swap(true, Ordering::SeqCst);
                            std::hint::spin_loop();
                            r.busy.store(false, Ordering::SeqCst);
                            Ok(was)
                        });
                        assert!(
                            !answer.unwrap().unwrap(),
                            "a reader used by two reads at once"
                        );
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert!(opened.load(Ordering::SeqCst) <= 4);
    }

    /// T-G3.6.3. Do: hold the one slot in a read; read from another thread with no time left, then with
    /// time; end the first read. Expect: the first try refused as a timeout; the second answered
    /// once the slot is given back (a waiter is woken, never left to its deadline).
    #[test]
    fn g3_s6_a_read_waits_for_a_slot_and_is_woken_when_one_is_given_back() {
        let (readers, _, _) = readers(1);
        let (hold_tx, hold_rx) = mpsc::channel::<()>();
        let (held_tx, held_rx) = mpsc::channel::<()>();
        let holder = {
            let readers = readers.clone();
            std::thread::spawn(move || {
                readers.with(far(), |_| {
                    held_tx.send(()).unwrap();
                    hold_rx.recv().unwrap();
                    Ok(())
                })
            })
        };
        held_rx.recv().unwrap();
        let refused = readers.with(Instant::now(), |_| Ok(()));
        assert!(matches!(
            refused.unwrap().unwrap_err().downcast_ref::<Refused>(),
            Some(Refused::Timeout)
        ));
        let waiter = {
            let readers = readers.clone();
            std::thread::spawn(move || readers.with(far(), |_| Ok(7)))
        };
        // The waiter counts itself before it waits; let the holder go once it has.
        while readers
            .slots
            .as_ref()
            .unwrap()
            .wait
            .waiting
            .load(Ordering::SeqCst)
            == 0
        {
            std::thread::yield_now();
        }
        hold_tx.send(()).unwrap();
        holder.join().unwrap().unwrap().unwrap();
        assert_eq!(waiter.join().unwrap().unwrap().unwrap(), 7);
    }

    /// T-G3.6.3. Do: hold every slot (as the log's restart does), read from another thread, then
    /// give the slots back. Expect: the read answered once they are given back (the restart's end
    /// wakes a waiting read; none is left to its deadline).
    #[test]
    fn g3_s6_a_read_held_back_by_a_restart_is_woken_when_it_ends() {
        let (readers, _, _) = readers(2);
        let slots = readers.slots.as_ref().unwrap();
        std::thread::scope(|s| {
            let all = slots.take_all();
            let waiter =
                s.spawn(|| readers.with(Instant::now() + Duration::from_secs(60), |_| Ok(7)));
            while slots.wait.waiting.load(Ordering::SeqCst) == 0 {
                std::thread::yield_now();
            }
            drop(all);
            assert_eq!(waiter.join().unwrap().unwrap().unwrap(), 7);
        });
    }

    /// T-G3.6.3. Do: read on 3 slots, then release the readers, then read again. Expect: every reader
    /// opened is let go by the release, and a read after it finds no reader (the writer reads).
    #[test]
    fn g3_s6_release_lets_every_slot_reader_go() {
        let (readers, opened, dropped) = readers(3);
        std::thread::scope(|s| {
            for _ in 0..3 {
                s.spawn(|| {
                    for _ in 0..50 {
                        readers.with(far(), |_| Ok(())).unwrap().unwrap();
                    }
                });
            }
        });
        readers.release();
        assert_eq!(
            dropped.load(Ordering::SeqCst),
            opened.load(Ordering::SeqCst)
        );
        assert!(readers.with(far(), |_| Ok(())).is_none());
    }

    /// Behavioral fairness: under oversubscription on 1 slot, queued waiters are handed the slot
    /// in FIFO order rather than starved by a caller looping on the slot.
    #[test]
    fn queued_waiters_are_served_in_fifo_order_without_starvation() {
        let (readers, _, _) = readers(1);
        let slots = readers.slots.as_ref().unwrap();
        let (hold_tx, hold_rx) = mpsc::channel::<()>();
        let (held_tx, held_rx) = mpsc::channel::<()>();
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));

        std::thread::scope(|s| {
            let r0 = readers.clone();
            let holder = s.spawn(move || {
                r0.with(far(), |_| {
                    held_tx.send(()).unwrap();
                    hold_rx.recv().unwrap();
                    Ok(())
                })
                .unwrap()
                .unwrap();
                // Immediately try to re-acquire; FIFO handoff must serve already-queued waiters first.
                r0.with(far(), |_| Ok(())).unwrap().unwrap();
            });
            held_rx.recv().unwrap();

            let mut waiters = Vec::new();
            for id in 1..=4usize {
                let r = readers.clone();
                let order = order.clone();
                waiters.push(s.spawn(move || {
                    r.with(far(), |_| {
                        order.lock().unwrap().push(id);
                        Ok(())
                    })
                    .unwrap()
                    .unwrap();
                }));
                while slots.wait.waiting.load(Ordering::Acquire) < id {
                    std::thread::yield_now();
                }
            }
            hold_tx.send(()).unwrap();
            holder.join().unwrap();
            for w in waiters {
                w.join().unwrap();
            }
        });

        assert_eq!(*order.lock().unwrap(), vec![1, 2, 3, 4]);
    }

    /// Blocker 4 regression: with 4 slots and 64 concurrent clients where 1 client holds a slot
    /// for a long read (350 ms), none of the other 63 short readers may get stuck behind that
    /// slot; all 63 complete via the remaining 3 slots with zero refusals under a 250 ms deadline.
    #[test]
    fn blocker_short_reads_never_stall_behind_one_long_read() {
        let (readers, _, _) = readers(4);
        let (long_held_tx, long_held_rx) = mpsc::channel::<()>();
        let (long_done_tx, long_done_rx) = mpsc::channel::<()>();

        let refused = AtomicUsize::new(0);
        let max_wait = std::sync::Mutex::new(Duration::ZERO);

        std::thread::scope(|s| {
            let r_long = readers.clone();
            let long_reader = s.spawn(move || {
                r_long
                    .with(far(), |_| {
                        long_held_tx.send(()).unwrap();
                        let _ = long_done_rx.recv_timeout(Duration::from_millis(350));
                        Ok(())
                    })
                    .unwrap()
                    .unwrap();
            });
            long_held_rx.recv().unwrap();

            let short_handles: Vec<_> = (0..63)
                .map(|_| {
                    let r = readers.clone();
                    let refused = &refused;
                    let max_wait = &max_wait;
                    s.spawn(move || {
                        for _ in 0..8 {
                            let start = Instant::now();
                            let res = r.with(start + Duration::from_millis(250), |_| {
                                park_for(Duration::from_micros(200));
                                Ok(())
                            });
                            let elapsed = start.elapsed();
                            let mut best = max_wait.lock().unwrap();
                            if elapsed > *best {
                                *best = elapsed;
                            }
                            drop(best);
                            if !matches!(res, Some(Ok(()))) {
                                refused.fetch_add(1, Ordering::SeqCst);
                            }
                        }
                    })
                })
                .collect();

            for handle in short_handles {
                handle.join().unwrap();
            }
            let _ = long_done_tx.send(());
            long_reader.join().unwrap();
        });

        let refusals = refused.load(Ordering::SeqCst);
        let worst = *max_wait.lock().unwrap();
        assert_eq!(
            refusals, 0,
            "short readers stalled behind long reader on another slot: {refusals} refused, worst wait {worst:?}"
        );
    }

    /// P1 regression: racing `take_all()` against returning readers must never leave the drainer
    /// parked on a free slot.
    #[test]
    fn p1_take_all_never_parks_forever_when_racing_release_steal_backout() {
        let (readers, _, _) = readers(2);
        let slots = readers.slots.as_ref().unwrap();
        let stop = AtomicBool::new(false);
        std::thread::scope(|s| {
            for _ in 0..6 {
                let readers = readers.clone();
                let stop = &stop;
                s.spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        let _ = readers.with(Instant::now() + Duration::from_millis(5), |_| Ok(()));
                    }
                });
            }
            for _ in 0..250 {
                let all = slots.take_all();
                drop(all);
            }
            stop.store(true, Ordering::Relaxed);
        });
    }

    /// P2 regression: repeated reader timeouts behind a long-lived head waiter must unlink and
    /// recycle their queue entries immediately rather than growing the queue indefinitely.
    #[test]
    fn p2_reader_timeouts_behind_head_do_not_grow_queue() {
        let (readers, _, _) = readers(1);
        let slots = readers.slots.as_ref().unwrap();
        let (hold_tx, hold_rx) = mpsc::channel::<()>();
        let (held_tx, held_rx) = mpsc::channel::<()>();

        std::thread::scope(|s| {
            let r_hold = readers.clone();
            let holder = s.spawn(move || {
                r_hold
                    .with(far(), |_| {
                        held_tx.send(()).unwrap();
                        hold_rx.recv().unwrap();
                        Ok(())
                    })
                    .unwrap()
                    .unwrap();
            });
            held_rx.recv().unwrap();

            let r_head = readers.clone();
            let head_waiter = s.spawn(move || r_head.with(far(), |_| Ok(99)).unwrap().unwrap());
            while slots.wait.waiting.load(Ordering::Acquire) < 1 {
                std::thread::yield_now();
            }

            // Time out 200 waiters behind the long-lived head waiter.
            for _ in 0..200 {
                let res = readers.with(Instant::now() + Duration::from_micros(50), |_| Ok(()));
                assert!(matches!(
                    res.unwrap().unwrap_err().downcast_ref::<Refused>(),
                    Some(Refused::Timeout)
                ));
            }

            {
                let queue = slots.lock_waiters();
                assert_eq!(
                    slots.wait.waiting.load(Ordering::Acquire),
                    1,
                    "only the head waiter may remain queued"
                );
                assert!(
                    queue.entries.len() <= 2,
                    "cancelled behind-head waiters must be recycled, got {} entries",
                    queue.entries.len()
                );
            }

            hold_tx.send(()).unwrap();
            holder.join().unwrap();
            assert_eq!(head_waiter.join().unwrap(), 99);
        });
    }

    /// Review blocker 3a regression: releasing a slot when a single waiter is queued must wake the
    /// waiter directly via unpark instead of leaving it parked on a millisecond timer stall.
    #[test]
    fn review_freed_slot_wakes_waiter_immediately_without_millisecond_stall() {
        let (readers, _, _) = readers(1);
        let slots = readers.slots.as_ref().unwrap();
        let mut min_latency = Duration::MAX;
        let mut max_latency = Duration::ZERO;

        for _ in 0..8 {
            let (hold_tx, hold_rx) = mpsc::channel::<Instant>();
            let (held_tx, held_rx) = mpsc::channel::<()>();

            let wake_latency = std::thread::scope(|s| {
                let r_hold = readers.clone();
                let holder = s.spawn(move || {
                    r_hold
                        .with(far(), |_| {
                            held_tx.send(()).unwrap();
                            let t = hold_rx.recv().unwrap();
                            Ok(t)
                        })
                        .unwrap()
                        .unwrap()
                });
                held_rx.recv().unwrap();

                let r_wait = readers.clone();
                let waiter =
                    s.spawn(move || r_wait.with(far(), |_| Ok(Instant::now())).unwrap().unwrap());
                while slots.wait.waiting.load(Ordering::Acquire) < 1 {
                    std::thread::yield_now();
                }
                let release_start = Instant::now();
                hold_tx.send(release_start).unwrap();
                let _ = holder.join().unwrap();
                let acquired_at = waiter.join().unwrap();
                acquired_at.saturating_duration_since(release_start)
            });
            min_latency = min_latency.min(wake_latency);
            max_latency = max_latency.max(wake_latency);
        }

        assert!(
            min_latency < Duration::from_millis(2) && max_latency < Duration::from_secs(1),
            "freed slot must wake parked waiter directly; min={min_latency:?} max={max_latency:?}"
        );
    }

    /// Review blocker 3b & fairness regression: direct FIFO handoff must ensure queued waiters
    /// make progress even under continuous barging, without arbitrary timer constants.
    #[test]
    fn review_bargers_cannot_starve_queued_waiter_under_adaptive_handoff() {
        struct StopOnDrop<'a>(&'a AtomicBool);
        impl Drop for StopOnDrop<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }

        let (readers, _, _) = readers(1);
        let stop = AtomicBool::new(false);
        std::thread::scope(|s| {
            for _ in 0..4 {
                let r = readers.clone();
                let stop = &stop;
                s.spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        let _ = r.with(far(), |_| Ok(()));
                    }
                });
            }
            let _guard = StopOnDrop(&stop);
            for _ in 0..50 {
                let ok = readers
                    .with(Instant::now() + Duration::from_secs(2), |_| Ok(7))
                    .unwrap()
                    .unwrap();
                assert_eq!(ok, 7);
            }
        });
    }

    /// Review P1/P2 regression: a waiter that wakes at its expired deadline must return `Timeout`
    /// without claiming a slot or running its callback past the deadline, and releasing one of two
    /// slots wakes a waiting reader immediately.
    #[test]
    fn review_expired_waiter_never_runs_past_deadline_and_local_waiter_wakes_on_free_slot() {
        let (readers_one, _, _) = readers(1);
        let slots_one = readers_one.slots.as_ref().unwrap();
        let (held_tx, held_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let ran = AtomicBool::new(false);
        let waiter_done = AtomicBool::new(false);

        std::thread::scope(|s| {
            let r_hold = readers_one.clone();
            let holder = s.spawn(move || {
                r_hold
                    .with(far(), |_| {
                        held_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                        Ok(())
                    })
                    .unwrap()
                    .unwrap();
            });
            held_rx.recv().unwrap();

            let r_expired = readers_one.clone();
            let ran_ref = &ran;
            let done_ref = &waiter_done;
            let expired_waiter = s.spawn(move || {
                let res = r_expired.with(Instant::now() + Duration::from_millis(25), |_| {
                    ran_ref.store(true, Ordering::SeqCst);
                    Ok(())
                });
                done_ref.store(true, Ordering::Release);
                res
            });
            while !waiter_done.load(Ordering::Acquire)
                && slots_one.wait.waiting.load(Ordering::Acquire) < 1
            {
                std::thread::yield_now();
            }
            while !waiter_done.load(Ordering::Acquire) {
                std::thread::park_timeout(Duration::from_millis(5));
            }
            release_tx.send(()).unwrap();
            let _ = holder.join();
            let res = expired_waiter.join().unwrap();
            assert!(
                matches!(
                    res,
                    Some(Err(ref err)) if err.downcast_ref::<Refused>() == Some(&Refused::Timeout)
                ),
                "expired waiter must time out instead of claiming slot past deadline: {res:?}"
            );
            assert!(
                !ran.load(Ordering::SeqCst),
                "expired waiter callback must not run after deadline"
            );
        });

        let (readers_two, _, _) = readers(2);
        let slots_two = readers_two.slots.as_ref().unwrap();
        let (h0_held_tx, h0_held_rx) = mpsc::channel();
        let (h1_held_tx, h1_held_rx) = mpsc::channel();
        let (h0_rel_tx, h0_rel_rx) = mpsc::channel();
        let (h1_rel_tx, h1_rel_rx) = mpsc::channel();

        std::thread::scope(|s| {
            let r0 = readers_two.clone();
            let holder0 = s.spawn(move || {
                r0.with(far(), |_| {
                    h0_held_tx.send(()).unwrap();
                    h0_rel_rx.recv().unwrap();
                    Ok(())
                })
                .unwrap()
                .unwrap();
            });
            h0_held_rx.recv().unwrap();

            let r1 = readers_two.clone();
            let holder1 = s.spawn(move || {
                r1.with(far(), |_| {
                    h1_held_tx.send(()).unwrap();
                    h1_rel_rx.recv().unwrap();
                    Ok(())
                })
                .unwrap()
                .unwrap();
            });
            h1_held_rx.recv().unwrap();

            let mut waiters = Vec::new();
            for i in 0..2 {
                let rw = readers_two.clone();
                waiters.push(s.spawn(move || {
                    rw.with(Instant::now() + Duration::from_secs(2), |_| Ok(i))
                        .unwrap()
                        .unwrap()
                }));
            }
            while slots_two.wait.waiting.load(Ordering::Acquire) < 2 {
                std::thread::yield_now();
            }
            h0_rel_tx.send(()).unwrap();
            holder0.join().unwrap();
            h1_rel_tx.send(()).unwrap();
            holder1.join().unwrap();
            let mut results: Vec<usize> = waiters.into_iter().map(|h| h.join().unwrap()).collect();
            results.sort_unstable();
            assert_eq!(results, vec![0, 1]);
        });
    }

    /// Review P2 regression: spurious unparks while `take_all()` waits on a held slot must not
    /// append duplicate drainer registrations onto `drain_waiters`, and the test releases its
    /// holder before asserting so any failure cannot hang the thread scope.
    #[test]
    fn p2_spurious_unparks_do_not_duplicate_drainer_registration() {
        let (readers, _, _) = readers(1);
        let slots = readers.slots.as_ref().unwrap();
        let (held_tx, held_rx) = mpsc::channel();
        let (rel_tx, rel_rx) = mpsc::channel();
        let (drainer_tid_tx, drainer_tid_rx) = mpsc::channel();

        let registered = std::thread::scope(|s| {
            let r_hold = readers.clone();
            let holder = s.spawn(move || {
                r_hold
                    .with(far(), |_| {
                        held_tx.send(()).unwrap();
                        rel_rx.recv().unwrap();
                        Ok(())
                    })
                    .unwrap()
                    .unwrap();
            });
            held_rx.recv().unwrap();

            let drainer = s.spawn(move || {
                drainer_tid_tx.send(std::thread::current()).unwrap();
                let all = slots.take_all();
                drop(all);
            });
            let drainer_thread = drainer_tid_rx.recv().unwrap();

            while slots.lock_waiters().drain_waiters.is_empty() {
                std::thread::yield_now();
            }
            for _ in 0..50 {
                drainer_thread.unpark();
                std::thread::yield_now();
            }
            let count = slots.lock_waiters().drain_waiters.len();

            rel_tx.send(()).unwrap();
            holder.join().unwrap();
            drainer.join().unwrap();
            count
        });
        assert_eq!(
            registered, 1,
            "spurious unparks must not duplicate drainer registrations; got {registered}"
        );
    }

    /// Spec P1-2 regression: an already-expired deadline must return `Timeout` immediately even on
    /// a warmed, idle store without running the callback.
    #[test]
    fn p1_expired_deadline_never_claims_idle_slot() {
        let (readers, _, _) = readers(2);
        assert_eq!(readers.with(far(), |_| Ok(1)).unwrap().unwrap(), 1);
        let ran = AtomicBool::new(false);
        let past = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        let res = readers.with(past, |_| {
            ran.store(true, Ordering::SeqCst);
            Ok(17)
        });
        assert!(
            matches!(
                res,
                Some(Err(ref err)) if err.downcast_ref::<Refused>() == Some(&Refused::Timeout)
            ),
            "already-expired deadline on idle store must return Timeout: {res:?}"
        );
        assert!(
            !ran.load(Ordering::SeqCst),
            "expired deadline must not execute callback"
        );
    }

    /// Spec P1-3 & Standards P2-6 regression: if a front waiter is handed a freed slot after its
    /// deadline expires, it must reject the slot without running its callback and immediately hand
    /// the slot onward to the next queued waiter while the other slot remains pinned. The deadline
    /// is computed inside the waiter right before entering checkout and the coordinator observes
    /// completion as well as registration so scheduler delay can never hang the test.
    #[test]
    fn p1_expired_front_waiter_hands_slot_to_next_waiter_while_other_slot_held() {
        let (readers, _, _) = readers(2);
        let slots = readers.slots.as_ref().unwrap();
        let (h0_held_tx, h0_held_rx) = mpsc::channel();
        let (h1_held_tx, h1_held_rx) = mpsc::channel();
        let (h0_rel_tx, h0_rel_rx) = mpsc::channel();
        let (h1_rel_tx, h1_rel_rx) = mpsc::channel();

        let (w_live_res, w_exp_res, w_exp_ran) = std::thread::scope(|s| {
            let r0 = readers.clone();
            let holder0 = s.spawn(move || {
                r0.with(far(), |_| {
                    h0_held_tx.send(()).unwrap();
                    h0_rel_rx.recv().unwrap();
                    Ok(())
                })
                .unwrap()
                .unwrap();
            });
            h0_held_rx.recv().unwrap();

            let r1 = readers.clone();
            let holder1 = s.spawn(move || {
                r1.with(far(), |_| {
                    h1_held_tx.send(()).unwrap();
                    h1_rel_rx.recv().unwrap();
                    Ok(())
                })
                .unwrap()
                .unwrap();
            });
            h1_held_rx.recv().unwrap();

            let exp_ran = Arc::new(AtomicBool::new(false));
            let exp_done = Arc::new(AtomicBool::new(false));
            let (exp_deadline_tx, exp_deadline_rx) = mpsc::channel();
            let r_exp = readers.clone();
            let ran_flag = exp_ran.clone();
            let done_flag = exp_done.clone();
            let waiter_exp = s.spawn(move || {
                // Compute deadline right before entering checkout so thread spawn jitter never
                // expires it before queuing.
                let deadline = Instant::now() + Duration::from_millis(80);
                exp_deadline_tx.send(deadline).unwrap();
                let res = r_exp.with(deadline, |_| {
                    ran_flag.store(true, Ordering::SeqCst);
                    Ok(99)
                });
                done_flag.store(true, Ordering::Release);
                res
            });
            let exp_deadline = exp_deadline_rx.recv().unwrap();
            while !exp_done.load(Ordering::Acquire)
                && slots.wait.waiting.load(Ordering::Acquire) < 1
            {
                std::thread::yield_now();
            }

            let r_live = readers.clone();
            let waiter_live =
                s.spawn(move || r_live.with(Instant::now() + Duration::from_secs(5), |_| Ok(42)));
            while !exp_done.load(Ordering::Acquire)
                && slots.wait.waiting.load(Ordering::Acquire) < 2
            {
                std::thread::yield_now();
            }

            // Hold the waiters mutex past `exp_deadline` so `waiter_exp` stays queued as the front
            // waiter when it expires, then release `holder0` (which will hand slot 0 to
            // `waiter_exp` as soon as we drop the mutex, or directly to `waiter_live` if
            // `waiter_exp` already unlinked itself). Either way, `waiter_live` MUST complete while
            // `holder1` stays pinned.
            let queue_guard = slots.lock_waiters();
            let past_deadline = exp_deadline + Duration::from_millis(10);
            while Instant::now() < past_deadline {
                let rem = past_deadline.saturating_duration_since(Instant::now());
                if !rem.is_zero() {
                    std::thread::park_timeout(rem);
                }
            }
            drop(queue_guard);

            h0_rel_tx.send(()).unwrap();
            holder0.join().unwrap();

            // Waiter 2 must finish while holder 1 is STILL holding its slot!
            let live_res = waiter_live.join().unwrap();
            let exp_res = waiter_exp.join().unwrap();

            h1_rel_tx.send(()).unwrap();
            holder1.join().unwrap();
            (live_res, exp_res, exp_ran.load(Ordering::SeqCst))
        });

        assert!(!w_exp_ran, "expired waiter must not execute its callback");
        assert!(
            matches!(
                w_exp_res.unwrap().unwrap_err().downcast_ref::<Refused>(),
                Some(Refused::Timeout)
            ),
            "expired waiter must return Refused::Timeout"
        );
        assert_eq!(
            w_live_res.unwrap().unwrap(),
            42,
            "live waiter must receive the freed slot while the other slot stays pinned"
        );
    }

    /// Standards P1-2 regression: a slot first released after a reader's deadline must never be
    /// claimed and returned to that reader, and must remain free for subsequent valid readers.
    #[test]
    fn p1_slot_released_after_deadline_is_never_claimed() {
        let (readers, _, _) = readers(2);
        let slots = readers.slots.as_ref().unwrap();
        let past = Instant::now()
            .checked_sub(Duration::from_millis(1))
            .unwrap_or_else(Instant::now);
        assert!(slots.try_claim_free_before(0, past).is_none());
        let claimed = slots
            .try_claim_free_before(0, far())
            .expect("slot 0 is free");
        assert!(
            slots.finish_claimed_before(claimed, past).is_none(),
            "post-claim expiry check must reject and release the slot"
        );
        assert_eq!(
            slots.slots[claimed].state.load(Ordering::Acquire),
            super::SLOT_FREE,
            "rejected slot must be returned to SLOT_FREE"
        );
        assert_eq!(readers.with(far(), |_| Ok(7)).unwrap().unwrap(), 7);
    }

    /// Review smaller finding: explicit `Config.readers` must not be silently capped at the core
    /// count when a caller asks for more readers.
    #[test]
    fn review_explicit_readers_count_is_not_silently_capped_at_cores() {
        let requested = super::super::machine_readers().saturating_add(16);
        let readers: super::super::Readers<Reader> =
            super::super::Readers::with_checkout(requested, Arc::default(), ReadCheckout::Slots);
        assert_eq!(readers.most, requested);
        assert_eq!(readers.slots.as_ref().unwrap().slots.len(), requested);
    }
}
