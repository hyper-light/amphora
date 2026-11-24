//! Amphora (G3 step 6, prototype F): a read takes its reader without a lock or a mutex every reader
//! shares (docs/design/store.md decision 43; docs/research/read-path.md, the 32-client profile).
//!
//! The pool (`Readers::with`'s other way) takes the pool's mutex to take a reader and again to give
//! it back, wakes the waiters, and holds the read side of one `RwLock` for the read: three writes
//! per read to lines every reader writes, which move between their cores at 32 clients (decision
//! 22's effect, LeanStore's rule: "programs that frequently write to memory locations accessed by
//! multiple threads do not scale"). Here each reader lives in a slot of its own, on a line of its
//! own; a thread takes the slot its number names, so at most as many threads as slots each write
//! only their own slot's lock. A thread whose slot is busy tries the others; when all are, it waits
//! on the pool's condition variable, and a read that ends wakes it only when someone waits (a read
//! of a counter only waiters write).
//!
//! Holding a slot is what holding the gate's read side was: the writer's restart of the log and
//! the readers' release take every slot, so no read is under way meanwhile.

use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering, fence};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread::Thread;
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
    /// `AMPHORA_READ_CHECKOUT` (read once), else `Slots`.
    pub fn process_default() -> Self {
        static FROM_ENV: std::sync::OnceLock<ReadCheckout> = std::sync::OnceLock::new();
        match PROCESS_DEFAULT.load(Ordering::Relaxed) {
            UNSET => {
                *FROM_ENV.get_or_init(|| match std::env::var("AMPHORA_READ_CHECKOUT").as_deref() {
                    Ok("pool") => Self::Pool,
                    _ => Self::Slots,
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
/// 128 bytes, as crossbeam's `CachePadded` aligns on x86_64 and aarch64). `state` guards ownership
/// so a finishing reader can hand a slot directly to the next FIFO waiter without releasing it to
/// barging callers.
#[repr(align(128))]
struct Slot<R> {
    state: AtomicU8,
    reader: Mutex<Option<R>>,
}

/// Cache-line-isolated waiter and drainer counts so fast-path uncontended reads read a clean
/// shared line that does not false-share with `slots` or `queue`.
#[repr(align(128))]
struct PaddedCount(AtomicUsize, AtomicUsize);

struct WaiterSlot {
    thread: Option<Thread>,
    assigned: Option<usize>,
    cancelled: bool,
}

#[derive(Default)]
struct WaitQueue {
    /// Strict FIFO queue of waiter indices into `entries`.
    fifo: VecDeque<usize>,
    /// Reusable waiter slots (grows lazily to peak concurrent waiters, never reallocates after).
    entries: Vec<WaiterSlot>,
    /// Free indices in `entries`.
    free: Vec<usize>,
    /// Threads currently inside `take_all()` waiting for slots to drain.
    drain_waiters: Vec<Thread>,
}

/// The slots and the fair FIFO queue used only when all slots are busy or draining.
pub(super) struct Slots<R> {
    slots: Box<[Slot<R>]>,
    /// `.0`: read threads waiting for a slot; `.1`: active `take_all()` drainers.
    waiting: PaddedCount,
    queue: Mutex<WaitQueue>,
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
        // Unlock the reader mutex first while `state == SLOT_HELD`, then hand off directly to a
        // queued waiter or mark `SLOT_FREE`.
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
            waiting: PaddedCount(AtomicUsize::new(0), AtomicUsize::new(0)),
            queue: Mutex::new(WaitQueue::default()),
        }
    }

    fn lock_reader(slot: &Slot<R>) -> MutexGuard<'_, Option<R>> {
        slot.reader.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_queue(&self) -> MutexGuard<'_, WaitQueue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[inline]
    fn has_waiters_or_drainers(&self) -> bool {
        self.waiting.0.load(Ordering::Acquire) > 0 || self.waiting.1.load(Ordering::Acquire) > 0
    }

    /// Claims one free slot via atomic state transition, probing this thread's home slot first
    /// without writing cache lines of already-held slots.
    fn try_claim_free(&self) -> Option<usize> {
        let len = self.slots.len();
        let first = thread_number().checked_rem(len).unwrap_or(0);
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
            if slot.state.load(Ordering::Relaxed) == SLOT_FREE
                && slot
                    .state
                    .compare_exchange(SLOT_FREE, SLOT_HELD, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
            {
                return Some(idx);
            }
        }
        None
    }

    fn wrap_claimed(&self, idx: usize) -> Option<Taken<'_, R>> {
        let slot = self.slots.get(idx)?;
        Some(Taken {
            idx,
            slot: Some(Self::lock_reader(slot)),
            slots: self,
        })
    }

    /// Hands off held slot `idx` to the next live FIFO waiter under `queue`.
    fn handoff_under_lock(queue: &mut WaitQueue, idx: usize) -> Option<Thread> {
        while let Some(waiter_id) = queue.fifo.pop_front() {
            let Some(entry) = queue.entries.get_mut(waiter_id) else {
                continue;
            };
            if entry.cancelled {
                entry.cancelled = false;
                entry.thread = None;
                entry.assigned = None;
                queue.free.push(waiter_id);
                continue;
            }
            entry.assigned = Some(idx);
            return entry.thread.take();
        }
        None
    }

    /// A slot: a free one at once when uncontended, or handed off in FIFO order before `until`
    /// (None past it).
    pub(super) fn take(&self, until: Instant) -> Option<Taken<'_, R>> {
        if !self.has_waiters_or_drainers()
            && let Some(idx) = self.try_claim_free()
        {
            return self.wrap_claimed(idx);
        }
        if until.saturating_duration_since(Instant::now()).is_zero() {
            if !self.has_waiters_or_drainers()
                && let Some(idx) = self.try_claim_free()
            {
                return self.wrap_claimed(idx);
            }
            return None;
        }

        self.waiting.0.fetch_add(1, Ordering::SeqCst);
        let waiter_id = {
            let mut queue = self.lock_queue();
            if self.waiting.1.load(Ordering::SeqCst) == 0
                && queue.fifo.is_empty()
                && let Some(idx) = self.try_claim_free()
            {
                drop(queue);
                self.waiting.0.fetch_sub(1, Ordering::SeqCst);
                return self.wrap_claimed(idx);
            }
            let current = std::thread::current();
            let id = match queue.free.pop() {
                Some(id) => {
                    if let Some(entry) = queue.entries.get_mut(id) {
                        entry.thread = Some(current);
                        entry.assigned = None;
                        entry.cancelled = false;
                    }
                    id
                }
                None => {
                    let id = queue.entries.len();
                    queue.entries.push(WaiterSlot {
                        thread: Some(current),
                        assigned: None,
                        cancelled: false,
                    });
                    id
                }
            };
            queue.fifo.push_back(id);
            id
        };

        loop {
            let left = until.saturating_duration_since(Instant::now());
            if !left.is_zero() {
                std::thread::park_timeout(left);
            }
            let mut queue = self.lock_queue();
            if let Some(entry) = queue.entries.get_mut(waiter_id) {
                if let Some(idx) = entry.assigned.take() {
                    entry.thread = None;
                    entry.cancelled = false;
                    queue.free.push(waiter_id);
                    drop(queue);
                    self.waiting.0.fetch_sub(1, Ordering::SeqCst);
                    return self.wrap_claimed(idx);
                }
                if until.saturating_duration_since(Instant::now()).is_zero() {
                    entry.cancelled = true;
                    entry.thread = None;
                    drop(queue);
                    self.waiting.0.fetch_sub(1, Ordering::SeqCst);
                    return None;
                }
                if entry.thread.is_none() {
                    entry.thread = Some(std::thread::current());
                }
            } else {
                drop(queue);
                self.waiting.0.fetch_sub(1, Ordering::SeqCst);
                return None;
            }
        }
    }

    /// Hands off held slot `idx` directly to the front FIFO waiter when readers are waiting and no
    /// `take_all()` is draining; otherwise marks `idx` free and resolves any concurrent waiter or
    /// drainer under the Dekker `SeqCst` fence.
    fn release_slot(&self, idx: usize) {
        let Some(slot) = self.slots.get(idx) else {
            return;
        };
        if self.waiting.0.load(Ordering::Acquire) > 0 && self.waiting.1.load(Ordering::Acquire) == 0
        {
            let wake = {
                let mut queue = self.lock_queue();
                if self.waiting.1.load(Ordering::Acquire) == 0 {
                    Self::handoff_under_lock(&mut queue, idx)
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

        if self.has_waiters_or_drainers() {
            let mut wake_read = None;
            let mut wake_drainers = Vec::new();
            {
                let mut queue = self.lock_queue();
                if self.waiting.1.load(Ordering::Acquire) > 0 {
                    wake_drainers = std::mem::take(&mut queue.drain_waiters);
                } else if !queue.fifo.is_empty()
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
                    wake_read = Self::handoff_under_lock(&mut queue, idx);
                    if wake_read.is_none() {
                        slot.state.store(SLOT_FREE, Ordering::Release);
                    }
                }
            }
            if let Some(thread) = wake_read {
                thread.unpark();
            }
            for thread in wake_drainers {
                thread.unpark();
            }
        }
    }

    /// Every slot, each once no read holds it: no read is under way while they are held, and the
    /// reads that came meanwhile are handed slots in FIFO order when they are given back.
    pub(super) fn take_all(&self) -> All<'_, R> {
        self.waiting.1.fetch_add(1, Ordering::SeqCst);
        let mut held = Vec::with_capacity(self.slots.len());
        for (idx, slot) in self.slots.iter().enumerate() {
            loop {
                if slot
                    .state
                    .compare_exchange(SLOT_FREE, SLOT_HELD, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    break;
                }
                {
                    let mut queue = self.lock_queue();
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
                        break;
                    }
                    queue.drain_waiters.push(std::thread::current());
                }
                std::thread::park();
            }
            held.push((idx, Self::lock_reader(slot)));
        }
        All { held, slots: self }
    }
}

/// Every slot held (`Slots::take_all`, by the log's restart and the readers' release). A read that
/// found them all held queues in FIFO order, and giving them back hands slots to waiting reads.
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
        // Unlock every reader mutex first while all slots stay `SLOT_HELD`.
        let indices: Vec<usize> = self.held.drain(..).map(|(idx, _)| idx).collect();
        let mut to_wake = Vec::new();
        {
            let mut queue = self.slots.lock_queue();
            let remaining_drainers = self.slots.waiting.1.fetch_sub(1, Ordering::SeqCst);
            let other_draining = remaining_drainers > 1;
            for idx in indices {
                if !other_draining
                    && let Some(thread) = Slots::<R>::handoff_under_lock(&mut queue, idx)
                {
                    to_wake.push(thread);
                    continue;
                }
                if let Some(slot) = self.slots.slots.get(idx) {
                    slot.state.store(SLOT_FREE, Ordering::Release);
                }
            }
            if other_draining {
                to_wake.append(&mut queue.drain_waiters);
            }
        }
        for thread in to_wake {
            thread.unpark();
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
            .waiting
            .0
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
            while slots.waiting.0.load(Ordering::SeqCst) == 0 {
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
                while slots.lock_queue().fifo.len() < id {
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
}
