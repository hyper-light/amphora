//! The log's two segments (docs/design/store.md, decision 45): `<db>-wal` and `<db>-wal2`, which
//! one the writer appends to, which one is closing, and when the closing one may be written over.
//! Pure: the log keeps one [`Segments`] in a [`SegmentsCell`], each change one of its steps, and
//! makes the I/O a step names (a switch's header and its sync before any frame; nothing at a reset).

use crate::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use crate::sync::Mutex;
use crossbeam_utils::CachePadded;
use std::iter;
use std::sync::OnceLock;

/// Power-of-two per-core shard count derived directly from hardware parallelism (`available_parallelism`),
/// used by `SegmentsCell` and `ShardedRwLock` so concurrent readers touch disjoint cache lines.
pub(crate) fn shard_count() -> usize {
    static COUNT: OnceLock<usize> = OnceLock::new();
    *COUNT.get_or_init(|| {
        std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(1)
            .next_power_of_two()
    })
}

static NEXT_THREAD_SHARD: AtomicUsize = AtomicUsize::new(0);

crate::thread::thread_local! {
    static THREAD_SHARD: usize = NEXT_THREAD_SHARD.fetch_add(1, Ordering::Relaxed);
}

/// Stable shard index for the current thread (no syscalls, one TLS load).
#[inline]
pub(crate) fn thread_shard() -> usize {
    THREAD_SHARD.try_with(|&id| id).unwrap_or(0)
}

/// [`Segments::pack`]'s word: bit 60 says the closing segment is shipped, 61 that it is copied,
/// 62 that the other segment is closing rather than free, and 63 that the second segment is
/// current. Reads are not in it: each reader slot counts its own ([`SlotReads`]).
const SHIPPED: u64 = 0x1000_0000_0000_0000;
const COPIED: u64 = 0x2000_0000_0000_0000;
const CLOSING: u64 = 0x4000_0000_0000_0000;
const SECOND_CURRENT: u64 = 0x8000_0000_0000_0000;
/// The multi-process shared word's bits 0-59 ([`Segments::shared_word`]): how many times its
/// segments and flags changed.
const CHANGES: u64 = 0x0FFF_FFFF_FFFF_FFFF;
/// [`SegmentsCell`]'s word holds the flags in bits 60-63, `SATURATED` in bit 59 (set only when
/// total reads reach the 30-bit cap), and a monotonic transition version in bits 0-58 so lock-free
/// per-shard readers detect any concurrent segment switch/copy/reset.
const READS: u64 = 0x3FFF_FFFF;
const SATURATED: u64 = 0x0800_0000_0000_0000;
const VERSION_MASK: u64 = 0x07FF_FFFF_FFFF_FFFF;

/// One of the log's two files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SegmentId {
    /// `<db>-wal`.
    First,
    /// `<db>-wal2`.
    Second,
}

impl SegmentId {
    pub const fn other(self) -> Self {
        match self {
            Self::First => Self::Second,
            Self::Second => Self::First,
        }
    }
}

/// What one segment holds now ([`Segments::state`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Segment {
    /// The writer appends here; `reads` reads under way count on it.
    Current { reads: u64 },
    /// Left at a switch: every frame in it was committed before any frame of the current one.
    Closing(Closing),
    /// Nothing in it is needed: the next switch writes its header over whatever it holds.
    Free,
}

/// A closing segment's three conditions for its reset: no read counts on it, it is copied whole
/// into a file since flushed, and a backup (if any) has shipped it. The last to come resets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Closing {
    pub reads: u64,
    pub copied: bool,
    pub shipped: bool,
}

/// The segments one read counts on, given back whole at its end ([`Segments::end_read`]).
#[derive(Debug)]
#[must_use = "a read's span is given back at its end, or the segment it holds is never reset"]
pub struct ReadSpan {
    current: SegmentId,
    closing: Option<SegmentId>,
    /// Shard in [`SegmentsCell`] incremented when this read began, so ending the read on another
    /// thread decrements the exact same shard.
    shard: u16,
}

impl PartialEq for ReadSpan {
    fn eq(&self, other: &Self) -> bool {
        self.current == other.current && self.closing == other.closing
    }
}

impl Eq for ReadSpan {}

impl ReadSpan {
    /// Sentinel value for an absent [`ReadSpan`] in packed atomic storage.
    pub(crate) const PACKED_NONE: u32 = u32::MAX;

    /// Pack into a `u32` for lock-free atomic storage on `WalFile`.
    pub(crate) fn pack_u32(self) -> u32 {
        let cur_bit: u32 = match self.current {
            SegmentId::First => 0,
            SegmentId::Second => 1,
        };
        let closing_bits: u32 = match self.closing {
            None => 0,
            Some(SegmentId::First) => 0b10,
            Some(SegmentId::Second) => 0b110,
        };
        cur_bit | closing_bits | (u32::from(self.shard) << 16)
    }

    /// Unpack a value stored with [`Self::pack_u32`].
    pub(crate) fn unpack_u32(packed: u32) -> Option<Self> {
        if packed == Self::PACKED_NONE {
            return None;
        }
        let current = if (packed & 1) == 0 {
            SegmentId::First
        } else {
            SegmentId::Second
        };
        let closing = if (packed & 0b10) == 0 {
            None
        } else if (packed & 0b100) == 0 {
            Some(SegmentId::First)
        } else {
            Some(SegmentId::Second)
        };
        let shard = u16::try_from(packed >> 16).unwrap_or(0);
        Some(Self {
            current,
            closing,
            shard,
        })
    }

    /// The segment current when the read began: its read marks guard the read.
    pub const fn current(&self) -> SegmentId {
        self.current
    }

    /// The per-core shard associated with this read transaction.
    pub(crate) const fn shard(&self) -> usize {
        self.shard as usize
    }

    /// Assigns the per-core shard for a span created outside [`SegmentsCell`] (e.g. shared memory).
    #[cfg(host_shared_wal)]
    pub(crate) fn set_shard(&mut self, shard: usize) {
        self.shard = u16::try_from(shard).unwrap_or(0);
    }

    /// The closing segment the read counts on, if any: its uncopied frames are in no file, so
    /// such a read never takes read mark 0 (decision 45's rule 2).
    pub const fn closing(&self) -> Option<SegmentId> {
        self.closing
    }

    /// Where the read looks a page up, in order, before the file.
    pub fn lookup_order(&self) -> impl Iterator<Item = SegmentId> {
        iter::once(self.current).chain(self.closing)
    }
}

/// The log's two segments: always one current, the other free or closing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segments {
    current: SegmentId,
    reads: u64,
    other: Option<Closing>,
}

impl Segments {
    /// A log appended to at `current`, its other segment free: a new database's, or one recovered
    /// with a single valid segment.
    pub const fn new(current: SegmentId) -> Self {
        Self {
            current,
            reads: 0,
            other: None,
        }
    }

    /// The log recovery finds, from the checkpoint sequence in each segment's header (None where
    /// the file is absent or its header's checksum fails). One valid segment is current and the
    /// other free. Of two, the one whose sequence is one past the other's is current (a switch
    /// writes the other's plus one, and the sequence is a u32 that wraps, so "the higher" is
    /// wrong at the wrap), and the other closes with nothing of it copied since the crash;
    /// `shipped` says no backup still needs its frames. None for two valid segments neither of
    /// which follows the other: no switch leaves such a pair.
    pub fn recover(first: Option<u32>, second: Option<u32>, shipped: bool) -> Option<Self> {
        let current = match (first, second) {
            (_, None) => return Some(Self::new(SegmentId::First)),
            (None, Some(_)) => return Some(Self::new(SegmentId::Second)),
            (Some(first), Some(second)) if second == first.wrapping_add(1) => SegmentId::Second,
            (Some(first), Some(second)) if first == second.wrapping_add(1) => SegmentId::First,
            (Some(_), Some(_)) => return None,
        };
        Some(Self {
            current,
            reads: 0,
            other: Some(Closing {
                reads: 0,
                copied: false,
                shipped,
            }),
        })
    }

    pub const fn current(&self) -> SegmentId {
        self.current
    }

    /// The closing segment, if one is.
    pub fn closing(&self) -> Option<SegmentId> {
        self.other.map(|_| self.current.other())
    }

    /// The segment a checkpoint pass copies (decision 45's rule 1): the closing one while there is
    /// one, since every frame in it is older than the current one's, and the current one only once
    /// the closing one is reset.
    pub fn to_copy(&self) -> SegmentId {
        self.closing().unwrap_or(self.current)
    }

    pub fn state(&self, segment: SegmentId) -> Segment {
        if segment == self.current {
            return Segment::Current { reads: self.reads };
        }
        self.other.map_or(Segment::Free, Segment::Closing)
    }

    /// A read begins. It counts on the current segment, and on the closing one only while that one
    /// isn't copied: once it is, its frames are all in the file, and the read takes them from there.
    /// So reads that overlap with no gap still let a copied segment's count fall to zero.
    pub fn begin_read(&mut self) -> ReadSpan {
        self.reads = self.reads.saturating_add(1);
        let closing = match &mut self.other {
            Some(closing) if !closing.copied => {
                closing.reads = closing.reads.saturating_add(1);
                Some(self.current.other())
            }
            _ => None,
        };
        ReadSpan {
            current: self.current,
            closing,
            shard: 0,
        }
    }

    /// The read that began with `span` is done: the segment this resets, if the read was the last
    /// thing it waited for. A segment a read counts on is never reset under it, so each segment in
    /// `span` is still current or closing.
    pub fn end_read(&mut self, span: ReadSpan) -> Option<SegmentId> {
        for segment in span.lookup_order() {
            if segment == self.current {
                self.reads = self.reads.saturating_sub(1);
            } else if let Some(closing) = &mut self.other {
                closing.reads = closing.reads.saturating_sub(1);
            }
        }
        self.reset_if_done()
    }

    /// The writer's switch, where a restart started the log again: the current segment closes and
    /// the free one becomes current, its header written and synced before any frame. None while the
    /// other segment is still closing: the writer appends on and asks again at its next turn after
    /// the reset. Either way it waits for nothing.
    pub fn switch(&mut self, backed_up: bool) -> Option<SegmentId> {
        if self.other.is_some() {
            return None;
        }
        self.other = Some(Closing {
            reads: self.reads,
            copied: false,
            shipped: !backed_up,
        });
        self.current = self.current.other();
        self.reads = 0;
        Some(self.current)
    }

    /// The checkpointer copied the closing segment's last frame and flushed the file: the segment
    /// this resets, if that was the last thing it waited for.
    pub fn copied(&mut self) -> Option<SegmentId> {
        self.mark_copied();
        self.reset_if_done()
    }

    /// [`Self::copied`]'s flag without its reset: the multi-process shared memory publishes the
    /// flag before it counts every process's reads, then asks [`Self::reset_if_done`].
    pub fn mark_copied(&mut self) {
        if let Some(closing) = &mut self.other {
            closing.copied = true;
        }
    }

    /// The backup shipped the closing segment: the segment this resets, if that was the last thing
    /// it waited for.
    pub fn shipped(&mut self) -> Option<SegmentId> {
        if let Some(closing) = &mut self.other {
            closing.shipped = true;
        }
        self.reset_if_done()
    }

    /// This state with `reads` on each segment: what the multi-process shared memory's reader
    /// slots count ([`SlotReads`]).
    pub fn with_reads(self, reads: SlotReads) -> Self {
        Self {
            current: self.current,
            reads: reads.on(self.current),
            other: self.other.map(|closing| Closing {
                reads: reads.on(self.current.other()),
                ..closing
            }),
        }
    }

    /// This state's segments and flags as one word of the multi-process shared memory, its read
    /// counts left out: every process's reads are in the reader slots ([`SlotReads`]).
    pub fn pack(&self) -> u64 {
        let mut word = 0;
        if self.current == SegmentId::Second {
            word |= SECOND_CURRENT;
        }
        if let Some(closing) = self.other {
            word |= CLOSING;
            if closing.copied {
                word |= COPIED;
            }
            if closing.shipped {
                word |= SHIPPED;
            }
        }
        word
    }

    /// The multi-process shared word that follows `word` with this state: [`Self::pack`]'s flags,
    /// and in bits 0-59 a count raised by each change of them (it wraps at 60 bits). A read checks
    /// the whole word once it is counted, so flags brought back by later changes (a reset and a
    /// switch, twice) can't pass for no change (decision 45, lane B15's note).
    pub fn shared_word(&self, word: u64) -> u64 {
        let flags = self.pack();
        if flags == word & !CHANGES {
            return word;
        }
        flags | (word.wrapping_add(1) & CHANGES)
    }

    /// The state [`Self::pack`] stored in `word`, with no read counted.
    pub fn unpack(word: u64) -> Self {
        let current = if word & SECOND_CURRENT == 0 {
            SegmentId::First
        } else {
            SegmentId::Second
        };
        let other = if word & CLOSING == 0 {
            None
        } else {
            Some(Closing {
                reads: 0,
                copied: word & COPIED != 0,
                shipped: word & SHIPPED != 0,
            })
        };
        Self {
            current,
            reads: 0,
            other,
        }
    }

    /// Free the closing segment if no read counts on it, it is copied and it is shipped: the
    /// segment this resets.
    pub fn reset_if_done(&mut self) -> Option<SegmentId> {
        match self.other {
            Some(Closing {
                reads: 0,
                copied: true,
                shipped: true,
            }) => {
                self.other = None;
                Some(self.current.other())
            }
            _ => None,
        }
    }
}

/// One process's [`Segments`], with read counts striped across per-core cache-padded shards so
/// concurrent readers never contend on a single global CAS word. Rare segment state transitions
/// (switch, copy, ship, reset) bump a monotonic version in `state` and serialize under `transition`,
/// using SeqCst Dekker ordering against per-shard counters so no read or closing-segment reset is
/// ever missed.
#[derive(Debug)]
pub struct SegmentsCell {
    state: CachePadded<AtomicU64>,
    shards: Box<[CachePadded<AtomicU64>]>,
    mask: usize,
    transition: Mutex<()>,
}

impl SegmentsCell {
    const READY_TO_RESET: u64 = CLOSING | COPIED | SHIPPED;

    /// A cell holding `segments`' segments and flags, with any initial reads placed on shard 0.
    pub fn new(segments: Segments) -> Self {
        let count = shard_count().max(1);
        let shards: Box<[CachePadded<AtomicU64>]> = (0..count)
            .map(|_| CachePadded::new(AtomicU64::new(0)))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let (first_reads, second_reads) = Self::reads_by_segment(&segments);
        if let Some(slot0) = shards.first() {
            let packed = (first_reads & SlotReads::COUNT)
                | (second_reads & SlotReads::COUNT).wrapping_shl(SlotReads::SECOND_SHIFT);
            slot0.store(packed, Ordering::Release);
        }
        let saturated = first_reads >= READS || second_reads >= READS;
        let word = segments.pack() | if saturated { SATURATED } else { 0 };
        Self {
            state: CachePadded::new(AtomicU64::new(word)),
            shards,
            mask: count.wrapping_sub(1),
            transition: Mutex::new(()),
        }
    }

    const fn reads_by_segment(segments: &Segments) -> (u64, u64) {
        let closing = match segments.other {
            Some(c) => c.reads,
            None => 0,
        };
        match segments.current {
            SegmentId::First => (segments.reads, closing),
            SegmentId::Second => (closing, segments.reads),
        }
    }

    #[inline(always)]
    const fn span_delta(current: SegmentId, closing: Option<SegmentId>) -> u64 {
        let mut d = SlotReads::one(current);
        if let Some(c) = closing {
            d |= SlotReads::one(c);
        }
        d
    }

    fn sum_reads(&self) -> (u64, u64) {
        let mut first = 0u64;
        let mut second = 0u64;
        for shard in self.shards.iter() {
            let word = shard.load(Ordering::SeqCst);
            let sr = SlotReads(word);
            first = first.saturating_add(sr.on(SegmentId::First));
            second = second.saturating_add(sr.on(SegmentId::Second));
        }
        (first, second)
    }

    fn assemble(word: u64, first: u64, second: u64) -> Segments {
        let mut segments = Segments::unpack(word);
        let (cur, oth) = match segments.current {
            SegmentId::First => (first, second),
            SegmentId::Second => (second, first),
        };
        segments.reads = cur;
        if let Some(closing) = &mut segments.other {
            closing.reads = oth;
        }
        segments
    }

    /// The state now.
    pub fn load(&self) -> Segments {
        let word = self.state.load(Ordering::SeqCst);
        let (first, second) = self.sum_reads();
        Self::assemble(word, first, second)
    }

    /// Applies `step` to the state as one change, and gives back what it returned. None, with
    /// nothing changed, when the step would count more reads on a segment than its 30 bits hold.
    pub fn update<T>(&self, mut step: impl FnMut(&mut Segments) -> T) -> Option<T> {
        let _guard = self.transition.lock();
        let word = self.state.load(Ordering::SeqCst);
        let (before_first, before_second) = self.sum_reads();
        let mut segments = Self::assemble(word, before_first, before_second);
        let out = step(&mut segments);
        let closing_reads = segments.other.map_or(0, |c| c.reads);
        if segments.reads > READS || closing_reads > READS {
            return None;
        }
        let (after_first, after_second) = Self::reads_by_segment(&segments);
        if after_first != before_first || after_second != before_second {
            if let Some(slot0) = self.shards.first() {
                // Synthetic read-count adjustment (used by tests): apply delta on shard 0.
                let mut cur = slot0.load(Ordering::SeqCst);
                loop {
                    let sr = SlotReads(cur);
                    let f = if after_first >= before_first {
                        sr.on(SegmentId::First)
                            .saturating_add(after_first.saturating_sub(before_first))
                    } else {
                        sr.on(SegmentId::First)
                            .saturating_sub(before_first.saturating_sub(after_first))
                    };
                    let s = if after_second >= before_second {
                        sr.on(SegmentId::Second)
                            .saturating_add(after_second.saturating_sub(before_second))
                    } else {
                        sr.on(SegmentId::Second)
                            .saturating_sub(before_second.saturating_sub(after_second))
                    };
                    let next = (f & SlotReads::COUNT)
                        | (s & SlotReads::COUNT).wrapping_shl(SlotReads::SECOND_SHIFT);
                    match slot0.compare_exchange_weak(
                        cur,
                        next,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    ) {
                        Ok(_) => break,
                        Err(seen) => cur = seen,
                    }
                }
            }
        }
        let saturated = after_first >= READS || after_second >= READS;
        let next_ver = word.wrapping_add(1) & VERSION_MASK;
        let next_word = segments.pack() | if saturated { SATURATED } else { 0 } | next_ver;
        self.state.store(next_word, Ordering::SeqCst);

        // Dekker re-check: if a concurrent reader finished between our first sum and publishing
        // COPIED|SHIPPED, free the closing segment now.
        if (next_word & Self::READY_TO_RESET) == Self::READY_TO_RESET {
            let closing_id = Segments::unpack(next_word).current().other();
            let (f, s) = self.sum_reads();
            let rem = match closing_id {
                SegmentId::First => f,
                SegmentId::Second => s,
            };
            if rem == 0 {
                let cleared = (next_word & !(CLOSING | COPIED | SHIPPED)) & !VERSION_MASK
                    | (next_word.wrapping_add(1) & VERSION_MASK);
                self.state.store(cleared, Ordering::SeqCst);
            }
        }
        Some(out)
    }

    #[cold]
    #[inline(never)]
    fn begin_read_saturated(&self, shard_idx: usize) -> Option<ReadSpan> {
        let _guard = self.transition.lock();
        let word = self.state.load(Ordering::SeqCst);
        let (first, second) = self.sum_reads();
        let current = if (word & SECOND_CURRENT) == 0 {
            SegmentId::First
        } else {
            SegmentId::Second
        };
        let closing = ((word & CLOSING) != 0 && (word & COPIED) == 0).then(|| current.other());
        let check_segment = |seg: SegmentId| match seg {
            SegmentId::First => first < READS,
            SegmentId::Second => second < READS,
        };
        if !check_segment(current) || closing.is_some_and(|c| !check_segment(c)) {
            return None;
        }
        let delta = Self::span_delta(current, closing);
        let shard = self.shards.get(shard_idx)?;
        shard.fetch_add(delta, Ordering::SeqCst);
        let new_first = first.saturating_add(u64::from(
            current == SegmentId::First || closing == Some(SegmentId::First),
        ));
        let new_second = second.saturating_add(u64::from(
            current == SegmentId::Second || closing == Some(SegmentId::Second),
        ));
        if new_first < READS && new_second < READS && (word & SATURATED) != 0 {
            let next_word = (word & !SATURATED & !VERSION_MASK)
                | (word.wrapping_add(1) & VERSION_MASK);
            self.state.store(next_word, Ordering::SeqCst);
        }
        Some(ReadSpan {
            current,
            closing,
            shard: u16::try_from(shard_idx).unwrap_or(0),
        })
    }

    #[cold]
    #[inline(never)]
    fn try_reset_closing(&self) -> Option<SegmentId> {
        let _guard = self.transition.lock();
        let word = self.state.load(Ordering::SeqCst);
        let (first, second) = self.sum_reads();
        if (word & SATURATED) != 0 && first < READS && second < READS {
            let cleared = (word & !SATURATED & !VERSION_MASK)
                | (word.wrapping_add(1) & VERSION_MASK);
            self.state.store(cleared, Ordering::SeqCst);
        }
        let word = self.state.load(Ordering::SeqCst);
        if (word & Self::READY_TO_RESET) != Self::READY_TO_RESET {
            return None;
        }
        let closing_id = Segments::unpack(word).current().other();
        let rem = match closing_id {
            SegmentId::First => first,
            SegmentId::Second => second,
        };
        if rem != 0 {
            return None;
        }
        let cleared = (word & !(CLOSING | COPIED | SHIPPED) & !VERSION_MASK)
            | (word.wrapping_add(1) & VERSION_MASK);
        self.state.store(cleared, Ordering::SeqCst);
        Some(closing_id)
    }

    /// [`Segments::begin_read`]. None when a segment already counts as many reads as its bits
    /// hold: the read is refused and tried again.
    #[inline]
    pub fn begin_read(&self) -> Option<ReadSpan> {
        let shard_idx = thread_shard() & self.mask;
        let shard = self.shards.get(shard_idx)?;
        loop {
            let w1 = self.state.load(Ordering::Acquire);
            if (w1 & SATURATED) != 0 {
                return self.begin_read_saturated(shard_idx);
            }
            let current = if (w1 & SECOND_CURRENT) == 0 {
                SegmentId::First
            } else {
                SegmentId::Second
            };
            let closing = ((w1 & CLOSING) != 0 && (w1 & COPIED) == 0).then(|| current.other());
            let delta = Self::span_delta(current, closing);
            let prev = SlotReads(shard.fetch_add(delta, Ordering::SeqCst));
            let overflow = (current == SegmentId::First && prev.on(SegmentId::First) >= READS)
                || (current == SegmentId::Second && prev.on(SegmentId::Second) >= READS)
                || (closing == Some(SegmentId::First) && prev.on(SegmentId::First) >= READS)
                || (closing == Some(SegmentId::Second) && prev.on(SegmentId::Second) >= READS);
            if overflow {
                shard.fetch_sub(delta, Ordering::SeqCst);
                return self.begin_read_saturated(shard_idx);
            }
            let w2 = self.state.load(Ordering::SeqCst);
            if w1 == w2 {
                return Some(ReadSpan {
                    current,
                    closing,
                    shard: u16::try_from(shard_idx).unwrap_or(0),
                });
            }
            shard.fetch_sub(delta, Ordering::SeqCst);
            if (w2 & Self::READY_TO_RESET) == Self::READY_TO_RESET {
                let _ = self.try_reset_closing();
            }
        }
    }

    /// [`Segments::end_read`]: the segment this resets, if the read was the last thing it waited
    /// for. Counts only fall here, so the change always fits the word.
    #[inline]
    pub fn end_read(&self, span: ReadSpan) -> Option<SegmentId> {
        let shard_idx = usize::from(span.shard) & self.mask;
        let delta = Self::span_delta(span.current, span.closing);
        if let Some(shard) = self.shards.get(shard_idx) {
            shard.fetch_sub(delta, Ordering::SeqCst);
        }
        let word = self.state.load(Ordering::SeqCst);
        if (word & Self::READY_TO_RESET) != Self::READY_TO_RESET && (word & SATURATED) == 0 {
            return None;
        }
        self.try_reset_closing()
    }
}

/// The reads that count on each segment: one reader slot's in the multi-process shared memory, or
/// the sum of every live slot's. Bits 0-31 count the reads on the first segment and bits 32-63 on
/// the second. One slot serves every read of a snapshot in its process, so it counts rather than
/// flags them; a slot reclaimed from a dead process is zeroed, so its reads stop counting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SlotReads(pub u64);

impl SlotReads {
    const COUNT: u64 = 0xFFFF_FFFF;
    const SECOND_SHIFT: u32 = 32;

    pub const fn on(self, segment: SegmentId) -> u64 {
        match segment {
            SegmentId::First => self.0 & Self::COUNT,
            SegmentId::Second => self.0.wrapping_shr(Self::SECOND_SHIFT),
        }
    }

    const fn one(segment: SegmentId) -> u64 {
        match segment {
            SegmentId::First => 1,
            SegmentId::Second => 1u64.wrapping_shl(Self::SECOND_SHIFT),
        }
    }

    /// These reads and the one begun with `span`. None when a count is at the most its bits hold:
    /// the read is refused and tried again.
    pub fn with(self, span: &ReadSpan) -> Option<Self> {
        span.lookup_order().try_fold(self, |reads, segment| {
            (reads.on(segment) < Self::COUNT)
                .then(|| Self(reads.0.wrapping_add(Self::one(segment))))
        })
    }

    /// These reads without the one begun with `span`, which [`Self::with`] counted.
    pub fn without(self, span: &ReadSpan) -> Self {
        span.lookup_order().fold(self, |reads, segment| {
            if reads.on(segment) == 0 {
                reads
            } else {
                Self(reads.0.wrapping_sub(Self::one(segment)))
            }
        })
    }

    /// These reads and `other`'s, each segment's count stopping at the most its bits hold.
    pub fn plus(self, other: Self) -> Self {
        let first = self
            .on(SegmentId::First)
            .saturating_add(other.on(SegmentId::First));
        let second = self
            .on(SegmentId::Second)
            .saturating_add(other.on(SegmentId::Second));
        Self(first.min(Self::COUNT) | second.min(Self::COUNT).wrapping_shl(Self::SECOND_SHIFT))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Do (decision 45, lane Q3): the same 20,000 steps (reads begun and ended, switches, copies,
    /// shipments, in a seeded random order) on a plain state and on a cell. Expect: after each
    /// step both hold the same state and the step answered the same.
    #[test]
    fn q3_the_cell_steps_as_the_state_machine_does() {
        let mut plain = Segments::new(SegmentId::First);
        let cell = SegmentsCell::new(plain);
        let (mut plain_spans, mut cell_spans) = (Vec::new(), Vec::new());
        let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..20_000 {
            match next() % 6 {
                0 | 1 => {
                    plain_spans.push(plain.begin_read());
                    cell_spans.push(cell.begin_read().unwrap());
                }
                2 if !plain_spans.is_empty() => {
                    let at = (next() % plain_spans.len() as u64) as usize;
                    let reset = plain.end_read(plain_spans.swap_remove(at));
                    assert_eq!(cell.end_read(cell_spans.swap_remove(at)), reset);
                }
                3 => {
                    let backed_up = next() % 2 == 0;
                    let switched = plain.switch(backed_up);
                    assert_eq!(cell.update(|s| s.switch(backed_up)), Some(switched));
                }
                4 => assert_eq!(cell.update(Segments::copied), Some(plain.copied())),
                _ => assert_eq!(cell.update(Segments::shipped), Some(plain.shipped())),
            }
            assert_eq!(cell.load(), plain);
            assert_eq!(cell_spans, plain_spans);
        }
    }

    /// Do (decision 45, lane Q3): eight threads each begin and end 20,000 reads on one cell while
    /// the writer switches, and the closing segment is copied and shipped. Expect: once every read
    /// has ended no read is counted, and the closing segment is free again.
    #[test]
    fn q3_reads_on_many_threads_leave_no_count_and_free_the_closing_segment() {
        let cell = SegmentsCell::new(Segments::new(SegmentId::First));
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..20_000 {
                        let span = cell.begin_read().unwrap();
                        cell.end_read(span);
                    }
                });
            }
            assert_eq!(
                cell.update(|s| s.switch(false)),
                Some(Some(SegmentId::Second))
            );
            cell.update(Segments::copied).unwrap();
            cell.update(Segments::shipped).unwrap();
        });
        assert_eq!(cell.load(), Segments::new(SegmentId::Second));
    }

    /// Do (lane Q3): begin a read on a cell whose current segment counts as many reads as its 30
    /// bits hold. Expect: the read is refused and the state is unchanged.
    #[test]
    fn q3_a_read_past_what_the_word_counts_is_refused() {
        let cell = SegmentsCell::new(Segments::new(SegmentId::First));
        cell.update(|s| s.reads = READS).unwrap();
        let before = cell.load();
        assert_eq!(
            before.state(SegmentId::First),
            Segment::Current { reads: READS }
        );
        assert_eq!(cell.begin_read(), None);
        assert_eq!(cell.load(), before);
    }

    /// Do: switch, then ask for a second switch before the closing segment is reset, and again
    /// after. Expect: the second ask is refused and changes nothing; the third switches back.
    #[test]
    fn d45_the_writer_switches_only_into_a_free_segment() {
        let mut log = Segments::new(SegmentId::First);
        assert_eq!(log.switch(false), Some(SegmentId::Second));
        assert_eq!(log.closing(), Some(SegmentId::First));
        let before = log;
        assert_eq!(log.switch(false), None);
        assert_eq!(log, before);
        assert_eq!(log.copied(), Some(SegmentId::First));
        assert_eq!(log.state(SegmentId::First), Segment::Free);
        assert_eq!(log.switch(false), Some(SegmentId::First));
    }

    /// Do: switch, copy the closing segment while a read counts on it, then end the read. Expect:
    /// a checkpoint copies the current segment before any switch and after the reset, and the
    /// closing one from the switch to its reset, copied or not (decision 45's rule 1).
    #[test]
    fn d45_a_checkpoint_copies_the_closing_segment_until_its_reset() {
        let mut log = Segments::new(SegmentId::First);
        assert_eq!(log.to_copy(), SegmentId::First);
        let read = log.begin_read();
        assert_eq!(log.switch(false), Some(SegmentId::Second));
        assert_eq!(log.to_copy(), SegmentId::First);
        assert_eq!(log.copied(), None);
        assert_eq!(log.to_copy(), SegmentId::First, "copied, not yet reset");
        assert_eq!(log.end_read(read), Some(SegmentId::First));
        assert_eq!(log.to_copy(), SegmentId::Second);
    }

    /// Do: a read begins, the writer switches, the closing segment is copied, then the read ends.
    /// Expect: the copy doesn't reset the segment under the read; the read's end does, with no
    /// other event between.
    #[test]
    fn d45_the_closing_segment_is_reset_when_its_last_read_leaves() {
        let mut log = Segments::new(SegmentId::First);
        let read = log.begin_read();
        assert_eq!(log.switch(false), Some(SegmentId::Second));
        assert_eq!(log.copied(), None);
        assert_eq!(
            log.state(SegmentId::First),
            Segment::Closing(Closing {
                reads: 1,
                copied: true,
                shipped: true
            })
        );
        assert_eq!(log.end_read(read), Some(SegmentId::First));
        assert_eq!(log.state(SegmentId::First), Segment::Free);
    }

    /// Do: for every order of the reset's three events (the last read's end, the copy, the ship),
    /// make them one at a time on a backed-up log. Expect: the reset comes with the last event,
    /// and with no earlier one.
    #[test]
    fn d45_the_reset_comes_with_the_last_of_its_three_events() {
        #[derive(Clone, Copy, Debug)]
        enum Event {
            ReadEnds,
            Copied,
            Shipped,
        }
        use Event::*;
        let orders = [
            [ReadEnds, Copied, Shipped],
            [ReadEnds, Shipped, Copied],
            [Copied, ReadEnds, Shipped],
            [Copied, Shipped, ReadEnds],
            [Shipped, ReadEnds, Copied],
            [Shipped, Copied, ReadEnds],
        ];
        for order in orders {
            let mut log = Segments::new(SegmentId::Second);
            let mut read = Some(log.begin_read());
            assert_eq!(log.switch(true), Some(SegmentId::First), "{order:?}");
            for (step, event) in order.into_iter().enumerate() {
                let reset = match event {
                    ReadEnds => read.take().and_then(|r| log.end_read(r)),
                    Copied => log.copied(),
                    Shipped => log.shipped(),
                };
                let last = step + 1 == order.len();
                assert_eq!(
                    reset,
                    last.then_some(SegmentId::Second),
                    "{order:?} at {event:?}"
                );
                assert_eq!(log.closing().is_none(), last, "{order:?} at {event:?}");
            }
        }
    }

    /// Do: keep a read always under way (each begins before the one before it ends) across a
    /// switch and the closing segment's copy. Expect: reads begun after the copy don't count on
    /// the closing segment, so it is reset when the last read begun before the copy ends, and the
    /// writer switches again while reads still overlap.
    #[test]
    fn d45_reads_overlapping_with_no_gap_let_a_copied_segment_go() {
        let mut log = Segments::new(SegmentId::First);
        let mut under_way = log.begin_read();
        assert_eq!(log.switch(false), Some(SegmentId::Second));
        let before_copy = log.begin_read();
        assert_eq!(log.end_read(under_way), None);
        under_way = before_copy;
        assert_eq!(log.copied(), None);
        let mut reset = None;
        for _ in 0..4 {
            let next = log.begin_read();
            assert_eq!(next.lookup_order().collect::<Vec<_>>(), [SegmentId::Second]);
            let ended = log.end_read(under_way);
            reset = reset.or(ended);
            under_way = next;
        }
        assert_eq!(reset, Some(SegmentId::First));
        assert_eq!(log.switch(false), Some(SegmentId::First));
        assert_eq!(log.end_read(under_way), None);
        assert_eq!(
            log.state(SegmentId::Second),
            Segment::Closing(Closing {
                reads: 0,
                copied: false,
                shipped: true
            })
        );
    }

    /// Do: a read begun before a switch, one after it, and one after the closing segment's copy.
    /// Expect: the first looks in the segment it began on (now closing) alone; the second in the
    /// current segment, then the closing one; the third in the current segment alone.
    #[test]
    fn d45_a_read_looks_in_the_current_segment_then_a_closing_one_it_counts_on() {
        let mut log = Segments::new(SegmentId::First);
        let before = log.begin_read();
        assert_eq!(log.switch(false), Some(SegmentId::Second));
        let after_switch = log.begin_read();
        assert_eq!(log.copied(), None);
        let after_copy = log.begin_read();
        let order = |span: &ReadSpan| span.lookup_order().collect::<Vec<_>>();
        assert_eq!(order(&before), [SegmentId::First]);
        assert_eq!(order(&after_switch), [SegmentId::Second, SegmentId::First]);
        assert_eq!(order(&after_copy), [SegmentId::Second]);
        assert_eq!(log.end_read(after_copy), None);
        assert_eq!(log.end_read(before), None);
        assert_eq!(log.end_read(after_switch), Some(SegmentId::First));
    }

    /// Do: recover a log with two valid segments, backed up and not. Expect: the closing one waits
    /// for its copy (and, backed up, its ship) before it is reset; no read is counted on either.
    #[test]
    fn d45_a_recovered_closing_segment_waits_for_its_copy() {
        let mut log = Segments::recover(Some(4), Some(5), true).expect("a switch's pair");
        assert_eq!(log.switch(false), None);
        assert_eq!(log.copied(), Some(SegmentId::First));
        let mut backed_up = Segments::recover(Some(6), Some(5), false).expect("a switch's pair");
        assert_eq!(backed_up.copied(), None);
        assert_eq!(backed_up.shipped(), Some(SegmentId::Second));
        assert_eq!(backed_up.switch(true), Some(SegmentId::Second));
    }

    /// Do: pack each kind of state into the shared memory's word, with reads under way. Expect:
    /// each unpacks to itself with no read counted, and the reads the slots count fill it back.
    /// Do: from a switch, reset and switch twice more, so the flags are as they were, and write
    /// the shared word at each step. Expect: the word differs from the one a read took its span
    /// from at the start, though it unpacks to the same state; a step that changes nothing leaves
    /// the word as it was (decision 45, lane B15's note).
    #[test]
    fn d45_two_switches_never_bring_the_shared_word_back() {
        let mut log = Segments::new(SegmentId::First);
        assert_eq!(log.switch(false), Some(SegmentId::Second));
        let spanned = log.shared_word(0);
        let mut word = spanned;
        for _ in 0..2 {
            assert!(log.copied().is_some());
            word = log.shared_word(word);
            assert!(log.switch(false).is_some());
            word = log.shared_word(word);
        }
        assert_eq!(Segments::unpack(word), Segments::unpack(spanned));
        assert_ne!(word, spanned, "a read's re-check sees the four changes");
        assert_eq!(log.shared_word(word), word);
        let at_most = log.pack() | CHANGES;
        assert!(log.copied().is_some());
        assert_eq!(
            log.shared_word(at_most),
            log.pack(),
            "the count wraps, the flags kept"
        );
    }

    #[test]
    fn d45_the_shared_word_holds_every_state() {
        let mut log = Segments::new(SegmentId::Second);
        let mut states = vec![log];
        let read = log.begin_read();
        states.push(log);
        assert_eq!(log.switch(true), Some(SegmentId::First));
        states.push(log);
        assert_eq!(log.copied(), None);
        states.push(log);
        assert_eq!(log.shipped(), None);
        states.push(log);
        for state in states {
            let unpacked = Segments::unpack(state.pack());
            assert_eq!(unpacked.current(), state.current());
            assert_eq!(unpacked.closing(), state.closing());
            let counted = SlotReads::default()
                .with(&ReadSpan {
                    current: state.current(),
                    closing: state.closing(),
                    shard: 0,
                })
                .expect("one read fits");
            let filled = unpacked.with_reads(counted);
            assert!(matches!(
                filled.state(filled.current()),
                Segment::Current { reads: 1 }
            ));
            if let Some(closing) = filled.closing() {
                let Segment::Closing(closing) = filled.state(closing) else {
                    panic!("closing");
                };
                assert_eq!(closing.reads, 1);
            }
        }
        assert_eq!(log.end_read(read), Some(SegmentId::Second));
    }

    /// Do: count reads on a slot, on one segment and on both, to the most a count holds, and take
    /// them off. Expect: each segment counts its own; the read past the most is refused and changes
    /// nothing; taking a read off a segment with none leaves the other's count whole.
    #[test]
    fn d45_a_slot_counts_each_segments_reads_and_refuses_one_past_its_bits() {
        use SegmentId::{First, Second};
        let both = ReadSpan {
            current: Second,
            closing: Some(First),
            shard: 0,
        };
        let first_only = ReadSpan {
            current: First,
            closing: None,
            shard: 0,
        };
        let reads = SlotReads::default().with(&both).expect("fits");
        let reads = reads.with(&first_only).expect("fits");
        assert_eq!((reads.on(First), reads.on(Second)), (2, 1));
        let full = SlotReads(SlotReads::COUNT);
        assert_eq!(full.with(&first_only), None);
        assert_eq!(full.with(&both), None);
        let second_only = ReadSpan {
            current: Second,
            closing: None,
            shard: 0,
        };
        let one_second = full.with(&second_only).expect("the second's count is free");
        assert_eq!(
            (one_second.on(First), one_second.on(Second)),
            (SlotReads::COUNT, 1)
        );
        assert_eq!(SlotReads::default().without(&both), SlotReads::default());
        assert_eq!(
            reads.without(&both).without(&first_only),
            SlotReads::default()
        );
        assert_eq!(
            full.plus(SlotReads(1)).on(First),
            SlotReads::COUNT,
            "a sum stops at the most a count holds"
        );
    }

    /// Do: recover from each pair of headers a switch or a crash in one can leave, at the
    /// sequence's wrap too, and from pairs no switch writes. Expect: the segment one past the
    /// other is current and the other closing; a lone valid segment is current and the other
    /// free; a pair neither of which follows the other is refused.
    #[test]
    fn d45_recovery_takes_the_segment_one_past_the_other_as_current() {
        use SegmentId::{First, Second};
        let found = |first: Option<u32>, second: Option<u32>| {
            Segments::recover(first, second, true).map(|log| (log.current(), log.closing()))
        };
        assert_eq!(found(None, None), Some((First, None)));
        assert_eq!(found(Some(7), None), Some((First, None)));
        assert_eq!(found(None, Some(7)), Some((Second, None)));
        assert_eq!(found(Some(7), Some(8)), Some((Second, Some(First))));
        assert_eq!(found(Some(8), Some(7)), Some((First, Some(Second))));
        assert_eq!(found(Some(u32::MAX), Some(0)), Some((Second, Some(First))));
        assert_eq!(found(Some(0), Some(u32::MAX)), Some((First, Some(Second))));
        assert_eq!(found(Some(7), Some(7)), None);
        assert_eq!(found(Some(7), Some(9)), None);
    }
}
