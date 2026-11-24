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
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering, fence};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread::Thread;
use std::time::{Duration, Instant};

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

/// Eventual-fairness handoff threshold (`0.5 ms`, matching Rust's `parking_lot_core`
/// `DEFAULT_FAIR_TIMEOUT` and Go `sync.Mutex`'s starvation mode): allows fast barging while no
/// waiter has waited past `0.5 ms` (avoiding OS wakeup idle bubbles on microsecond queries), then
/// switches to direct FIFO handoff so no waiter ever starves.
const FAIR_HANDOFF_NS: u64 = 500_000;

fn monotonic_ns() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let elapsed = EPOCH.get_or_init(Instant::now).elapsed().as_nanos();
    u64::try_from(elapsed).unwrap_or(u64::MAX).max(1)
}

/// A slot, alone on its cache line pair (the adjacent-line prefetcher pulls lines in pairs, so
/// 128 bytes, as crossbeam's `CachePadded` aligns on x86_64 and aarch64). Each slot owns its own
/// state, local waiter counter, eventual-fairness timestamp, reader, and FIFO queue so threads on
/// disjoint slots share no locks and no writable cache lines under steady-state concurrency.
#[repr(align(128))]
struct Slot<R> {
    state: AtomicU8,
    /// Live waiters queued on this slot alone (mutated only under `self.queue`).
    local_waiting: AtomicUsize,
    /// `monotonic_ns()` deadline when this slot next forces a direct FIFO handoff (`0` when empty).
    /// Advanced by [`FAIR_HANDOFF_NS`] on each handoff (`parking_lot_core` eventual-fairness),
    /// giving each handed-off thread one full fair slice before the next kernel wakeup.
    next_handoff_ns: AtomicU64,
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
    seq: u64,
    queued: bool,
}

#[derive(Default)]
struct SlotQueue {
    /// Head and tail of the intrusive FIFO doubly-linked list inside `entries` so any waiter
    /// timing out at any position unlinks and recycles its slot in O(1) without growing the queue.
    head: Option<usize>,
    tail: Option<usize>,
    live_len: usize,
    next_seq: u64,
    /// Reusable waiter slots (bounded by peak concurrent live waiters on this slot).
    entries: Vec<WaiterSlot>,
    /// Free indices in `entries`.
    free: Vec<usize>,
    /// Threads currently inside `take_all()` waiting for this slot to become free.
    drain_waiters: Vec<Thread>,
}

impl SlotQueue {
    fn push_back(&mut self, thread: Thread) -> (usize, usize) {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        let prev = self.tail;
        let id = match self.free.pop() {
            Some(id) => {
                if let Some(entry) = self.entries.get_mut(id) {
                    entry.thread = Some(thread);
                    entry.assigned = None;
                    entry.prev = prev;
                    entry.next = None;
                    entry.seq = seq;
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
                    seq,
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
        let rank = self.live_len;
        self.live_len = self.live_len.saturating_add(1);
        (id, rank)
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
        self.live_len = self.live_len.saturating_sub(1);
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
        self.live_len = self.live_len.saturating_sub(1);
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

    fn rank_of(&self, id: usize) -> usize {
        if self.head == Some(id) {
            return 0;
        }
        let Some(head_seq) = self.head.and_then(|h| self.entries.get(h)).map(|e| e.seq) else {
            return 0;
        };
        let Some(my_seq) = self.entries.get(id).map(|e| e.seq) else {
            return 0;
        };
        let delta = usize::try_from(my_seq.wrapping_sub(head_seq)).unwrap_or(usize::MAX);
        delta.min(self.live_len.saturating_sub(1))
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
                    local_waiting: AtomicUsize::new(0),
                    next_handoff_ns: AtomicU64::new(0),
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
        if slot.local_waiting.fetch_add(1, Ordering::SeqCst) == 0 {
            self.waiting.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[inline]
    fn dec_local_waiting(&self, slot: &Slot<R>) {
        if slot.local_waiting.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.waiting.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[inline]
    fn home_index(&self) -> usize {
        let len = self.slots.len();
        thread_number().checked_rem(len).unwrap_or(0)
    }

    /// Returns the front live waiter id and clears `slot.next_handoff_ns` when the queue is empty.
    fn front_live(slot: &Slot<R>, queue: &SlotQueue) -> Option<usize> {
        if let Some(front_id) = queue.head {
            Some(front_id)
        } else {
            slot.next_handoff_ns.store(0, Ordering::Relaxed);
            None
        }
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
    /// already-held slots. When `respect_fairness` is true, avoids barging past a slot whose next
    /// eventual-fairness handoff deadline has been reached.
    fn try_claim_free_from(&self, first: usize, respect_fairness: bool) -> Option<usize> {
        let len = self.slots.len();
        let mut now_cached = 0u64;
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
                let deadline = slot.next_handoff_ns.load(Ordering::Relaxed);
                if deadline != 0 {
                    if now_cached == 0 {
                        now_cached = monotonic_ns();
                    }
                    if now_cached >= deadline {
                        continue;
                    }
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
    /// `owner_slot`, advancing `owner_slot.next_handoff_ns` by [`FAIR_HANDOFF_NS`] if more
    /// waiters remain so the handed-off thread gets one full fair slice.
    fn handoff_from_slot(
        &self,
        owner_slot: &Slot<R>,
        queue: &mut SlotQueue,
        assigned_idx: usize,
        now_ns: u64,
    ) -> Option<Thread> {
        let front_id = queue.pop_front()?;
        self.dec_local_waiting(owner_slot);
        let thread = if let Some(entry) = queue.entries.get_mut(front_id) {
            entry.assigned = Some(assigned_idx);
            entry.thread.take()
        } else {
            None
        };
        if Self::front_live(owner_slot, queue).is_some() {
            owner_slot
                .next_handoff_ns
                .store(now_ns.saturating_add(FAIR_HANDOFF_NS), Ordering::Relaxed);
        }
        thread
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
            if other.local_waiting.load(Ordering::Acquire) == 0 {
                continue;
            }
            let (wake, wake_next) = {
                let mut queue = Self::lock_slot_queue(other);
                if self.waiting.1.load(Ordering::Acquire) == 0 {
                    let handed = self.handoff_from_slot(other, &mut queue, idx, monotonic_ns());
                    let next =
                        if handed.is_some() && other.state.load(Ordering::Acquire) == SLOT_FREE {
                            Self::front_live(other, &queue).and_then(|front_id| {
                                queue
                                    .entries
                                    .get(front_id)
                                    .and_then(|entry| entry.thread.clone())
                            })
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

    /// A slot: a free one at once when uncontended or within the eventual-fairness interval, or
    /// handed off in FIFO order on this thread's home slot before `until` (None past it).
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
        let (waiter_id, mut rank) = {
            let mut queue = Self::lock_slot_queue(home);
            self.inc_local_waiting(home);
            let front_empty = Self::front_live(home, &queue).is_none();
            if self.waiting.1.load(Ordering::SeqCst) == 0
                && front_empty
                && let Some(idx) = self.try_claim_free_from(home_idx, false)
            {
                self.dec_local_waiting(home);
                drop(queue);
                return self.wrap_claimed(idx);
            }
            let (id, rank) = queue.push_back(std::thread::current());
            if front_empty {
                home.next_handoff_ns.store(
                    monotonic_ns().saturating_add(FAIR_HANDOFF_NS),
                    Ordering::Relaxed,
                );
            }
            (id, rank)
        };

        loop {
            let left = until.saturating_duration_since(Instant::now());
            if !left.is_zero() {
                // Safety-net timeout scaled by queue rank (`(rank + 2) * FAIR_HANDOFF_NS`): in
                // steady state, direct FIFO handoffs always beat this timeout (zero spurious
                // wakeups or timer storms at 1k–10k waiters), while if a holder goes idle during
                // its fair slice the front waiter still wakes promptly in FIFO order.
                let slices = u64::try_from(rank.saturating_add(2)).unwrap_or(u64::MAX);
                let probe = Duration::from_nanos(FAIR_HANDOFF_NS.saturating_mul(slices));
                std::thread::park_timeout(left.min(probe));
            }
            let mut wake_next_front = None;
            let mut queue = Self::lock_slot_queue(home);
            let assigned = queue
                .entries
                .get_mut(waiter_id)
                .and_then(|entry| entry.assigned.take());
            if let Some(idx) = assigned {
                queue.recycle(waiter_id);
                drop(queue);
                return self.wrap_claimed(idx);
            }
            let front = Self::front_live(home, &queue);
            let is_front = front == Some(waiter_id);
            // If we are at the head of this slot's queue and a slot is free, claim it cleanly.
            if self.waiting.1.load(Ordering::Acquire) == 0
                && is_front
                && let Some(idx) = self.try_claim_free_from(home_idx, false)
            {
                let _ = queue.pop_front();
                self.dec_local_waiting(home);
                queue.recycle(waiter_id);
                if Self::front_live(home, &queue).is_some() {
                    home.next_handoff_ns.store(
                        monotonic_ns().saturating_add(FAIR_HANDOFF_NS),
                        Ordering::Relaxed,
                    );
                }
                drop(queue);
                return self.wrap_claimed(idx);
            }
            if until.saturating_duration_since(Instant::now()).is_zero() {
                if queue.unlink(waiter_id) {
                    self.dec_local_waiting(home);
                }
                queue.recycle(waiter_id);
                if let Some(new_front) = Self::front_live(home, &queue)
                    && is_front
                {
                    wake_next_front = queue
                        .entries
                        .get(new_front)
                        .and_then(|entry| entry.thread.clone());
                }
                drop(queue);
                if let Some(thread) = wake_next_front {
                    thread.unpark();
                }
                return None;
            }
            rank = queue.rank_of(waiter_id);
            if let Some(entry) = queue.entries.get_mut(waiter_id)
                && entry.thread.is_none()
            {
                entry.thread = Some(std::thread::current());
            }
        }
    }

    /// Releases held slot `idx`. When a waiter on this slot has reached [`FAIR_HANDOFF_NS`], hands
    /// `idx` directly to the oldest FIFO waiter so no barger can starve it; otherwise releases to
    /// `SLOT_FREE` with zero locks and zero kernel wakeups.
    fn release_slot(&self, idx: usize) {
        let Some(slot) = self.slots.get(idx) else {
            return;
        };
        let draining = self.waiting.1.load(Ordering::Acquire) > 0;
        let local_waiting = slot.local_waiting.load(Ordering::Acquire);

        if !draining {
            if local_waiting > 0 {
                let deadline = slot.next_handoff_ns.load(Ordering::Relaxed);
                let now = if deadline != 0 { monotonic_ns() } else { 0 };
                if deadline != 0 && now >= deadline {
                    let wake = {
                        let mut queue = Self::lock_slot_queue(slot);
                        if self.waiting.1.load(Ordering::Acquire) == 0 {
                            self.handoff_from_slot(slot, &mut queue, idx, now)
                        } else {
                            None
                        }
                    };
                    if let Some(thread) = wake {
                        thread.unpark();
                        return;
                    }
                }
            } else if self.waiting.0.load(Ordering::Relaxed) > 0
                && let Some(thread) = self.try_steal_handoff(idx)
            {
                thread.unpark();
                return;
            }
        }

        if self.free_and_wake_drainers(slot) {
            return;
        }

        if self.waiting.1.load(Ordering::Acquire) == 0
            && slot.local_waiting.load(Ordering::Acquire) == 0
            && self.waiting.0.load(Ordering::Acquire) > 0
            && slot
                .state
                .compare_exchange(SLOT_FREE, SLOT_HELD, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        {
            if let Some(thread) = self.try_steal_handoff(idx) {
                thread.unpark();
            } else {
                let _ = self.free_and_wake_drainers(slot);
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
        let indices: Vec<usize> = self.held.drain(..).map(|(idx, _)| idx).collect();
        let _ = self.slots.waiting.1.fetch_sub(1, Ordering::SeqCst);
        let mut to_wake = Vec::new();
        for idx in indices {
            if self.slots.waiting.1.load(Ordering::Acquire) == 0 {
                let handed = if let Some(home) = self.slots.slots.get(idx)
                    && home.local_waiting.load(Ordering::Acquire) > 0
                {
                    let mut queue = Slots::<R>::lock_slot_queue(home);
                    if self.slots.waiting.1.load(Ordering::Acquire) == 0 {
                        self.slots
                            .handoff_from_slot(home, &mut queue, idx, monotonic_ns())
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
            if let Some(slot) = self.slots.slots.get(idx) {
                let _ = self.slots.free_and_wake_drainers(slot);
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
                while super::Slots::lock_slot_queue(&slots.slots[0]).live_len < id {
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
            while super::Slots::lock_slot_queue(&slots.slots[0]).live_len < 1 {
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
                assert_eq!(queue.live_len, 1, "only the head waiter may remain queued");
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
}
