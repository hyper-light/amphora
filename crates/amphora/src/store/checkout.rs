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

/// Packed layout of `Slot::wait_flags` (mutated only under `Slot::queue`, read lock-free on the
/// release fast path):
/// - bit 0 (`WOKEN_BIT`): the front waiter has been unparked for a barging wakeup and has not yet
///   claimed a slot or re-parked, suppressing redundant wakeups while barging is active.
/// - bit 1 (`HANDOFF_BIT`): the front waiter woke once and lost to a barger (observed starvation,
///   no arbitrary timer), requesting direct FIFO handoff on the next release.
/// - bits `2..`: number of live waiters queued on this slot.
const WOKEN_BIT: usize = 1;
const HANDOFF_BIT: usize = 2;
const WAIT_SHIFT: u32 = 2;
const WAIT_INC: usize = 1 << WAIT_SHIFT;

/// A slot, alone on its cache line pair (the adjacent-line prefetcher pulls lines in pairs, so
/// 128 bytes, as crossbeam's `CachePadded` aligns on x86_64 and aarch64). Each slot owns its own
/// state, packed local waiter flags/count, reader, and FIFO queue so threads on disjoint slots
/// share no locks and no writable cache lines under steady-state concurrency.
#[repr(align(128))]
struct Slot<R> {
    state: AtomicU8,
    /// Packed `WOKEN_BIT | HANDOFF_BIT | (local_waiting << WAIT_SHIFT)` (mutated under `queue`).
    wait_flags: AtomicUsize,
    reader: Mutex<Option<R>>,
    queue: Mutex<SlotQueue>,
}

/// Cache-line-isolated aggregate counts:
/// - `.0`: number of slots that currently have at least one queued read waiter (updated only on
///   a slot's `0 -> 1` and `1 -> 0` `local_waiting` transitions, so steady-state contended readers
///   never write this line).
/// - `.1`: active `take_all()` drainers.
#[repr(align(128))]
struct PaddedCount(AtomicUsize, AtomicUsize);

struct WaiterSlot {
    thread: Option<Thread>,
    assigned: Option<usize>,
    prev: Option<usize>,
    next: Option<usize>,
    queued: bool,
    was_woken: bool,
}

#[derive(Default)]
struct SlotQueue {
    /// Head and tail of the intrusive FIFO doubly-linked list inside `entries` so any waiter
    /// timing out at any position unlinks and recycles its slot in O(1) without growing the queue.
    head: Option<usize>,
    tail: Option<usize>,
    /// Reusable waiter slots (bounded by peak concurrent live waiters on this slot).
    entries: Vec<WaiterSlot>,
    /// Free indices in `entries`.
    free: Vec<usize>,
    /// Threads currently inside `take_all()` waiting for this slot to become free.
    drain_waiters: Vec<Thread>,
}

impl SlotQueue {
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
                    entry.was_woken = false;
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
                    was_woken: false,
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
            entry.was_woken = false;
            self.free.push(id);
        }
    }
}

/// The slots and their per-slot eventual-fairness FIFO queues.
pub(super) struct Slots<R> {
    slots: Box<[Slot<R>]>,
    /// `.0`: slots with queued read waiters; `.1`: active `take_all()` drainers.
    waiting: PaddedCount,
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
                    wait_flags: AtomicUsize::new(0),
                    reader: Mutex::new(None),
                    queue: Mutex::new(SlotQueue::default()),
                })
                .collect(),
            waiting: PaddedCount(AtomicUsize::new(0), AtomicUsize::new(0)),
        }
    }

    fn lock_reader(slot: &Slot<R>) -> MutexGuard<'_, Option<R>> {
        slot.reader.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_slot_queue(slot: &Slot<R>) -> MutexGuard<'_, SlotQueue> {
        slot.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[inline]
    fn inc_local_waiting(&self, slot: &Slot<R>) {
        let prev = slot.wait_flags.fetch_add(WAIT_INC, Ordering::SeqCst);
        if (prev >> WAIT_SHIFT) == 0 {
            self.waiting.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[inline]
    fn dec_local_waiting(&self, slot: &Slot<R>) {
        let mut emptied = false;
        let _ = slot
            .wait_flags
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |prev| {
                let count = prev >> WAIT_SHIFT;
                if count <= 1 {
                    emptied = count == 1;
                    Some(0)
                } else {
                    emptied = false;
                    Some(prev.saturating_sub(WAIT_INC))
                }
            });
        if emptied {
            self.waiting.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[inline]
    fn clear_front_bits(slot: &Slot<R>) {
        slot.wait_flags
            .fetch_and(!(WOKEN_BIT | HANDOFF_BIT), Ordering::SeqCst);
    }

    #[inline]
    fn wake_front_barging(slot: &Slot<R>, queue: &mut SlotQueue) -> Option<Thread> {
        if (slot.wait_flags.load(Ordering::Relaxed) & WOKEN_BIT) != 0 {
            return None;
        }
        let front_id = queue.head?;
        slot.wait_flags.fetch_or(WOKEN_BIT, Ordering::SeqCst);
        let entry = queue.entries.get_mut(front_id)?;
        entry.was_woken = true;
        entry.thread.clone()
    }

    #[inline]
    fn home_index(&self) -> usize {
        let len = self.slots.len();
        thread_number().checked_rem(len).unwrap_or(0)
    }

    /// Marks `slot` free and wakes any `take_all()` drainers registered on it.
    ///
    /// Every transition back to `SLOT_FREE` must go through this helper so a concurrent `take_all()`
    /// that registered and parked on `slot` can never miss its wakeup.
    #[inline]
    fn free_and_wake_drainers(&self, slot: &Slot<R>) -> bool {
        slot.state.store(SLOT_FREE, Ordering::Release);
        fence(Ordering::SeqCst);
        if self.waiting.1.load(Ordering::Acquire) > 0 {
            let drainers = {
                let mut queue = Self::lock_slot_queue(slot);
                std::mem::take(&mut queue.drain_waiters)
            };
            for thread in drainers {
                thread.unpark();
            }
            true
        } else {
            false
        }
    }

    /// Claims one free slot via atomic CAS, probing `first` first without writing cache lines of
    /// already-held slots. When `respect_fairness` is true, avoids barging past a slot whose front
    /// waiter already observed starvation (`HANDOFF_BIT`).
    fn try_claim_free_from(&self, first: usize, respect_fairness: bool) -> Option<usize> {
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
            if respect_fairness {
                let flags = slot.wait_flags.load(Ordering::Relaxed);
                if (flags & HANDOFF_BIT) != 0 && (flags >> WAIT_SHIFT) > 0 {
                    continue;
                }
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

    fn wrap_claimed(&self, idx: usize) -> Option<Taken<'_, R>> {
        let slot = self.slots.get(idx)?;
        LAST_SLOT.with(|last| last.set(idx));
        Some(Taken {
            idx,
            slot: Some(Self::lock_reader(slot)),
            slots: self,
        })
    }

    /// Hands off held slot `assigned_idx` directly to the front live FIFO waiter queued on
    /// `owner_slot`, clearing `WOKEN_BIT | HANDOFF_BIT` so the next waiter starts cleanly.
    fn handoff_from_slot(
        &self,
        owner_slot: &Slot<R>,
        queue: &mut SlotQueue,
        assigned_idx: usize,
    ) -> Option<Thread> {
        let front_id = queue.pop_front()?;
        self.dec_local_waiting(owner_slot);
        Self::clear_front_bits(owner_slot);
        if let Some(entry) = queue.entries.get_mut(front_id) {
            entry.assigned = Some(assigned_idx);
            entry.thread.take()
        } else {
            None
        }
    }

    /// Attempts to hand off held slot `idx` directly to another slot's waiter (work-stealing) when
    /// `idx` itself has no queued waiters.
    fn try_steal_handoff(&self, idx: usize) -> Option<Thread> {
        if self.waiting.0.load(Ordering::Acquire) == 0 || self.waiting.1.load(Ordering::Acquire) > 0
        {
            return None;
        }
        let len = self.slots.len();
        for offset in 1..len {
            let other_idx = idx.checked_add(offset).map_or(0, |sum| {
                if sum < len {
                    sum
                } else {
                    sum.saturating_sub(len)
                }
            });
            let Some(other) = self.slots.get(other_idx) else {
                continue;
            };
            if (other.wait_flags.load(Ordering::Acquire) >> WAIT_SHIFT) == 0 {
                continue;
            }
            let (wake, wake_next) = {
                let mut queue = Self::lock_slot_queue(other);
                if self.waiting.1.load(Ordering::Acquire) == 0 {
                    let handed = self.handoff_from_slot(other, &mut queue, idx);
                    let next =
                        if handed.is_some() && other.state.load(Ordering::Acquire) == SLOT_FREE {
                            Self::wake_front_barging(other, &mut queue)
                        } else {
                            None
                        };
                    (handed, next)
                } else {
                    (None, None)
                }
            };
            if let Some(next_thread) = wake_next {
                next_thread.unpark();
            }
            if wake.is_some() {
                return wake;
            }
        }
        None
    }

    /// Attempts to claim currently-free slot `idx` and hand it directly to another slot's queued
    /// waiter. Never transitions `slot` to `SLOT_HELD` unless a live waiter is confirmed under the
    /// target queue lock, so a failed steal never leaves `slot` temporarily held and can never
    /// cause a missed wakeup for a concurrently arriving local waiter or drainer.
    fn try_steal_from_free(&self, idx: usize, slot: &Slot<R>) -> Option<Thread> {
        if self.waiting.0.load(Ordering::Acquire) == 0 || self.waiting.1.load(Ordering::Acquire) > 0
        {
            return None;
        }
        let len = self.slots.len();
        for offset in 1..len {
            if slot.state.load(Ordering::Acquire) != SLOT_FREE
                || self.waiting.1.load(Ordering::Acquire) > 0
            {
                return None;
            }
            let other_idx = idx.checked_add(offset).map_or(0, |sum| {
                if sum < len {
                    sum
                } else {
                    sum.saturating_sub(len)
                }
            });
            let Some(other) = self.slots.get(other_idx) else {
                continue;
            };
            if (other.wait_flags.load(Ordering::Acquire) >> WAIT_SHIFT) == 0 {
                continue;
            }
            let (wake, wake_next, still_free) = {
                let mut queue = Self::lock_slot_queue(other);
                if self.waiting.1.load(Ordering::Acquire) == 0 && queue.head.is_some() {
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
                        let handed = self.handoff_from_slot(other, &mut queue, idx);
                        let next = if handed.is_some()
                            && other.state.load(Ordering::Acquire) == SLOT_FREE
                        {
                            Self::wake_front_barging(other, &mut queue)
                        } else {
                            None
                        };
                        (handed, next, true)
                    } else {
                        (None, None, false)
                    }
                } else {
                    (None, None, true)
                }
            };
            if let Some(next_thread) = wake_next {
                next_thread.unpark();
            }
            if wake.is_some() {
                return wake;
            }
            if !still_free {
                return None;
            }
        }
        None
    }

    /// Wakes a local waiter after `slot` has been stored `SLOT_FREE` (with `fence(SeqCst)`
    /// already executed):
    /// - When no local waiter is queued, attempts to steal `slot` for another slot's waiter without
    ///   temporarily marking `slot` held on failure, then re-checks for a newly arrived local
    ///   waiter.
    /// - When a local waiter is queued and `slot` is still `SLOT_FREE`, either hands `slot` off
    ///   directly (`HANDOFF_BIT`) or wakes the front waiter (`WOKEN_BIT`).
    /// - When a local waiter is queued and a barger already stole `slot` before we could wake the
    ///   queue head, sets `HANDOFF_BIT` with a Dekker re-check of `SLOT_FREE` so continuous
    ///   barging can never prevent the queue head from receiving a slot.
    fn wake_local_after_free(&self, idx: usize, slot: &Slot<R>) {
        let mut flags = slot.wait_flags.load(Ordering::Acquire);
        if (flags >> WAIT_SHIFT) == 0 {
            if self.waiting.0.load(Ordering::Acquire) > 0
                && self.waiting.1.load(Ordering::Acquire) == 0
                && let Some(thread) = self.try_steal_from_free(idx, slot)
            {
                thread.unpark();
                return;
            }
            flags = slot.wait_flags.load(Ordering::Acquire);
            if (flags >> WAIT_SHIFT) == 0 {
                return;
            }
        }
        if slot.state.load(Ordering::Acquire) != SLOT_FREE {
            // A barger already claimed the slot we just freed while a local waiter was queued:
            // arm direct FIFO handoff lock-free (only while waiters remain), then Dekker re-check
            // in case the barger already released before seeing `HANDOFF_BIT`. No queue lock is
            // taken on this common barged path.
            if (flags & HANDOFF_BIT) == 0 {
                let _ = slot
                    .wait_flags
                    .try_update(Ordering::SeqCst, Ordering::SeqCst, |f| {
                        if (f >> WAIT_SHIFT) > 0 && (f & HANDOFF_BIT) == 0 {
                            Some(f | HANDOFF_BIT)
                        } else {
                            None
                        }
                    });
            }
            if self.waiting.1.load(Ordering::SeqCst) > 0
                || slot.state.load(Ordering::SeqCst) != SLOT_FREE
            {
                return;
            }
            flags = slot.wait_flags.load(Ordering::Acquire);
            if (flags >> WAIT_SHIFT) == 0 {
                return;
            }
        }
        if (flags & HANDOFF_BIT) == 0 && (flags & WOKEN_BIT) != 0 {
            return;
        }
        let wake = {
            let mut queue = Self::lock_slot_queue(slot);
            if self.waiting.1.load(Ordering::Acquire) > 0 || queue.head.is_none() {
                None
            } else if slot.state.load(Ordering::Acquire) == SLOT_FREE {
                let cur = slot.wait_flags.load(Ordering::Relaxed);
                if (cur & HANDOFF_BIT) != 0 {
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
                        self.handoff_from_slot(slot, &mut queue, idx)
                    } else {
                        None
                    }
                } else {
                    Self::wake_front_barging(slot, &mut queue)
                }
            } else {
                // Another barger arrived while we waited for the queue lock: arm `HANDOFF_BIT`
                // and Dekker re-check `SLOT_FREE`.
                slot.wait_flags.fetch_or(HANDOFF_BIT, Ordering::SeqCst);
                if self.waiting.1.load(Ordering::SeqCst) == 0
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
                    self.handoff_from_slot(slot, &mut queue, idx)
                } else {
                    None
                }
            }
        };
        if let Some(thread) = wake {
            thread.unpark();
        }
    }

    /// A slot: a free one at once when uncontended or not starving, or woken/handed off in FIFO
    /// order on this thread's home slot before `until` (None past it).
    pub(super) fn take(&self, until: Instant) -> Option<Taken<'_, R>> {
        let home_idx = self.home_index();
        let start_idx = LAST_SLOT.with(|last| {
            let v = last.get();
            if v < self.slots.len() { v } else { home_idx }
        });
        if self.waiting.1.load(Ordering::Acquire) == 0
            && let Some(idx) = self.try_claim_free_from(start_idx, true)
        {
            return self.wrap_claimed(idx);
        }
        if until.saturating_duration_since(Instant::now()).is_zero() {
            if self.waiting.1.load(Ordering::Acquire) == 0
                && let Some(idx) = self.try_claim_free_from(start_idx, true)
            {
                return self.wrap_claimed(idx);
            }
            return None;
        }

        let home = self.slots.get(home_idx)?;
        let waiter_id = {
            let mut queue = Self::lock_slot_queue(home);
            self.inc_local_waiting(home);
            let front_empty = queue.head.is_none();
            if self.waiting.1.load(Ordering::SeqCst) == 0
                && front_empty
                && let Some(idx) = self.try_claim_free_from(home_idx, false)
            {
                self.dec_local_waiting(home);
                drop(queue);
                return self.wrap_claimed(idx);
            }
            queue.push_back(std::thread::current())
        };

        loop {
            let left = until.saturating_duration_since(Instant::now());
            if !left.is_zero() {
                std::thread::park_timeout(left);
            }
            let expired = until.saturating_duration_since(Instant::now()).is_zero();
            let mut wake_next_front = None;
            let mut queue = Self::lock_slot_queue(home);
            let assigned = queue
                .entries
                .get_mut(waiter_id)
                .and_then(|entry| entry.assigned.take());
            if let Some(idx) = assigned {
                queue.recycle(waiter_id);
                if self.waiting.1.load(Ordering::Acquire) == 0
                    && home.state.load(Ordering::Acquire) == SLOT_FREE
                {
                    wake_next_front = Self::wake_front_barging(home, &mut queue);
                }
                drop(queue);
                if let Some(thread) = wake_next_front {
                    thread.unpark();
                }
                if expired {
                    self.release_slot(idx);
                    return None;
                }
                return self.wrap_claimed(idx);
            }
            let is_front = queue.head == Some(waiter_id);
            if !expired
                && self.waiting.1.load(Ordering::Acquire) == 0
                && let Some(idx) = self.try_claim_free_from(home_idx, !is_front)
            {
                if queue.unlink(waiter_id) {
                    self.dec_local_waiting(home);
                }
                queue.recycle(waiter_id);
                if is_front {
                    Self::clear_front_bits(home);
                    if home.state.load(Ordering::Acquire) == SLOT_FREE {
                        wake_next_front = Self::wake_front_barging(home, &mut queue);
                    }
                }
                drop(queue);
                if let Some(thread) = wake_next_front {
                    thread.unpark();
                }
                return self.wrap_claimed(idx);
            }
            if expired {
                if queue.unlink(waiter_id) {
                    self.dec_local_waiting(home);
                }
                queue.recycle(waiter_id);
                if is_front {
                    Self::clear_front_bits(home);
                    if self.waiting.1.load(Ordering::Acquire) == 0
                        && home.state.load(Ordering::Acquire) == SLOT_FREE
                    {
                        wake_next_front = Self::wake_front_barging(home, &mut queue);
                    }
                }
                drop(queue);
                if let Some(thread) = wake_next_front {
                    thread.unpark();
                }
                return None;
            }
            if is_front {
                let was_woken = queue
                    .entries
                    .get(waiter_id)
                    .is_some_and(|entry| entry.was_woken);
                if was_woken && self.waiting.1.load(Ordering::Acquire) == 0 {
                    // We woke for a free slot and lost it to a barger: request direct FIFO handoff
                    // on the next release so no waiter can starve, without any arbitrary timer.
                    let _ = home
                        .wait_flags
                        .try_update(Ordering::SeqCst, Ordering::SeqCst, |f| {
                            Some((f & !WOKEN_BIT) | HANDOFF_BIT)
                        });
                } else {
                    home.wait_flags.fetch_and(!WOKEN_BIT, Ordering::SeqCst);
                }
                // Dekker re-check after clearing WOKEN_BIT (and possibly setting HANDOFF_BIT).
                if self.waiting.1.load(Ordering::SeqCst) == 0
                    && let Some(idx) = self.try_claim_free_from(home_idx, false)
                {
                    if queue.unlink(waiter_id) {
                        self.dec_local_waiting(home);
                    }
                    queue.recycle(waiter_id);
                    Self::clear_front_bits(home);
                    if home.state.load(Ordering::Acquire) == SLOT_FREE {
                        wake_next_front = Self::wake_front_barging(home, &mut queue);
                    }
                    drop(queue);
                    if let Some(thread) = wake_next_front {
                        thread.unpark();
                    }
                    return self.wrap_claimed(idx);
                }
            }
            if let Some(entry) = queue.entries.get_mut(waiter_id)
                && entry.thread.is_none()
            {
                entry.thread = Some(std::thread::current());
            }
        }
    }

    /// Releases held slot `idx`. When the front waiter observed starvation (`HANDOFF_BIT`), hands
    /// `idx` directly in FIFO order; otherwise releases to `SLOT_FREE` and immediately wakes one
    /// local waiter (if not already woken) or steals for another slot's waiter.
    fn release_slot(&self, idx: usize) {
        let Some(slot) = self.slots.get(idx) else {
            return;
        };
        let draining = self.waiting.1.load(Ordering::Acquire) > 0;
        let flags = slot.wait_flags.load(Ordering::Acquire);

        if !draining {
            if (flags & HANDOFF_BIT) != 0 && (flags >> WAIT_SHIFT) > 0 {
                let wake = {
                    let mut queue = Self::lock_slot_queue(slot);
                    if self.waiting.1.load(Ordering::Acquire) == 0 {
                        self.handoff_from_slot(slot, &mut queue, idx)
                    } else {
                        None
                    }
                };
                if let Some(thread) = wake {
                    thread.unpark();
                    return;
                }
            } else if (flags >> WAIT_SHIFT) == 0
                && self.waiting.0.load(Ordering::Acquire) > 0
                && let Some(thread) = self.try_steal_handoff(idx)
            {
                thread.unpark();
                return;
            }
        }

        if self.free_and_wake_drainers(slot) {
            return;
        }

        self.wake_local_after_free(idx, slot);
    }

    /// Every slot, each once no read holds it: no read is under way while they are held, and the
    /// reads that came meanwhile are handed slots in FIFO order when they are given back.
    pub(super) fn take_all(&self) -> All<'_, R> {
        self.waiting.1.fetch_add(1, Ordering::SeqCst);
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
                    break;
                }
                {
                    let mut queue = Self::lock_slot_queue(slot);
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
                            queue.drain_waiters.retain(|t| t.id() != me_id);
                        }
                        break;
                    }
                    if !queue.drain_waiters.iter().any(|t| t.id() == me_id) {
                        queue.drain_waiters.push(me.clone());
                    }
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
        let indices: Vec<usize> = self.held.drain(..).map(|(idx, _)| idx).collect();
        let _ = self.slots.waiting.1.fetch_sub(1, Ordering::SeqCst);
        let mut to_wake = Vec::new();
        for idx in indices {
            if self.slots.waiting.1.load(Ordering::Acquire) == 0 {
                let handed = if let Some(home) = self.slots.slots.get(idx)
                    && (home.wait_flags.load(Ordering::Acquire) >> WAIT_SHIFT) > 0
                {
                    let mut queue = Slots::<R>::lock_slot_queue(home);
                    if self.slots.waiting.1.load(Ordering::Acquire) == 0 {
                        self.slots.handoff_from_slot(home, &mut queue, idx)
                    } else {
                        None
                    }
                } else {
                    self.slots.try_steal_handoff(idx)
                };
                if let Some(thread) = handed {
                    to_wake.push(thread);
                    continue;
                }
            }
            if let Some(slot) = self.slots.slots.get(idx)
                && !self.slots.free_and_wake_drainers(slot)
            {
                self.slots.wake_local_after_free(idx, slot);
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
                while (slots.slots[0].wait_flags.load(Ordering::Acquire) >> super::WAIT_SHIFT) < id
                {
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

    /// P1 regression: racing `take_all()` against returning readers attempting cross-slot steal
    /// handoffs must never leave the drainer parked on a free slot.
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
            while (slots.slots[0].wait_flags.load(Ordering::Acquire) >> super::WAIT_SHIFT) < 1 {
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
                let queue = super::Slots::lock_slot_queue(&slots.slots[0]);
                assert_eq!(
                    slots.slots[0].wait_flags.load(Ordering::Acquire) >> super::WAIT_SHIFT,
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

        // Run several rounds and check the minimum wake latency so isolated OS scheduling jitter
        // does not flake, while a 3 ms timer-based wake stalls every single round.
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
                while (slots.slots[0].wait_flags.load(Ordering::Acquire) >> super::WAIT_SHIFT) < 1 {
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

    /// Review blocker 3b & fairness regression: adaptive observed-starvation handoff must ensure
    /// queued waiters make progress even under continuous barging, without arbitrary timer constants.
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
    /// without claiming a slot or running its callback past the deadline (without hanging the test
    /// if the waiter expires before the coordinator observes its queue count), and on a multi-slot
    /// pool releasing a slot with no local waiter wakes a waiter queued on another slot while a
    /// local waiter also wakes on a free slot.
    #[test]
    fn review_expired_waiter_never_runs_past_deadline_and_local_waiter_wakes_on_free_slot() {
        // Part 1: expired waiter never runs its callback even when a slot is held across expiry.
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
                && (slots_one.slots[0].wait_flags.load(Ordering::Acquire) >> super::WAIT_SHIFT) < 1
            {
                std::thread::yield_now();
            }
            // Wait until the waiter finishes timing out while the holder still owns the slot.
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

        // Part 2: two-slot cross-slot handoff and local-slot wakeup.
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

            // Queue two waiters on separate threads (likely spanning both slots, or both on one
            // slot so releasing the other exercises cross-slot handoff).
            let mut waiters = Vec::new();
            for i in 0..2 {
                let rw = readers_two.clone();
                waiters.push(s.spawn(move || {
                    rw.with(Instant::now() + Duration::from_secs(2), |_| Ok(i))
                        .unwrap()
                        .unwrap()
                }));
            }
            while slots_two.waiting.0.load(Ordering::Acquire) == 0 {
                std::thread::yield_now();
            }
            // Releasing just one holder must promptly wake a waiter (locally or cross-slot).
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
    /// append duplicate drainer registrations onto `drain_waiters`.
    #[test]
    fn p2_spurious_unparks_do_not_duplicate_drainer_registration() {
        let (readers, _, _) = readers(1);
        let slots = readers.slots.as_ref().unwrap();
        let (held_tx, held_rx) = mpsc::channel();
        let (rel_tx, rel_rx) = mpsc::channel();
        let (drainer_tid_tx, drainer_tid_rx) = mpsc::channel();

        std::thread::scope(|s| {
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

            while super::Slots::lock_slot_queue(&slots.slots[0])
                .drain_waiters
                .is_empty()
            {
                std::thread::yield_now();
            }
            for _ in 0..50 {
                drainer_thread.unpark();
                std::thread::yield_now();
            }
            let registered = super::Slots::lock_slot_queue(&slots.slots[0])
                .drain_waiters
                .len();
            assert_eq!(
                registered, 1,
                "spurious unparks must not duplicate drainer registrations; got {registered}"
            );

            rel_tx.send(()).unwrap();
            holder.join().unwrap();
            drainer.join().unwrap();
        });
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
