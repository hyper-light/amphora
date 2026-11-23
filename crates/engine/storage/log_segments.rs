//! The log's two segments (docs/design/store.md, decision 45): `<db>-wal` and `<db>-wal2`, which
//! one the writer appends to, which one is closing, and when the closing one may be written over.
//! Pure: the log keeps one [`Segments`] under its shared state's lock and makes the I/O a step
//! names (a switch's header and its sync before any frame; nothing at a reset).

use std::iter;

/// [`Segments::pack`]'s word: bits 0-29 count the current segment's reads and bits 30-59 the
/// closing one's; bit 60 says the closing segment is shipped, 61 that it is copied, 62 that the
/// other segment is closing rather than free, and 63 that the second segment is current. The
/// word is 64 bits, the widest every platform swaps without a lock; the four flags take four, and
/// the two counts take half each of the 60 left.
const READS: u64 = 0x3FFF_FFFF;
const CLOSING_READS_SHIFT: u32 = 30;
const SHIPPED: u64 = 0x1000_0000_0000_0000;
const COPIED: u64 = 0x2000_0000_0000_0000;
const CLOSING: u64 = 0x4000_0000_0000_0000;
const SECOND_CURRENT: u64 = 0x8000_0000_0000_0000;

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
#[derive(Debug, PartialEq, Eq)]
#[must_use = "a read's span is given back at its end, or the segment it holds is never reset"]
pub struct ReadSpan {
    current: SegmentId,
    closing: Option<SegmentId>,
}

impl ReadSpan {
    /// The segment current when the read began: its read marks guard the read.
    pub const fn current(&self) -> SegmentId {
        self.current
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
        }
    }

    /// The read that began with `span` is done: the segment this resets, if the read was the last
    /// thing it waited for. A segment a read counts on is never reset under it, so each segment in
    /// `span` is still current or closing.
    pub fn end_read(&mut self, span: ReadSpan) -> Option<SegmentId> {
        self.release(&span)
    }

    /// [`Self::end_read`] for a state kept in the shared memory, which another process can change
    /// between a load and the swap that stores the result: the caller applies the same span to
    /// what it loads again until its swap holds.
    pub fn release(&mut self, span: &ReadSpan) -> Option<SegmentId> {
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
        if let Some(closing) = &mut self.other {
            closing.copied = true;
        }
        self.reset_if_done()
    }

    /// The backup shipped the closing segment: the segment this resets, if that was the last thing
    /// it waited for.
    pub fn shipped(&mut self) -> Option<SegmentId> {
        if let Some(closing) = &mut self.other {
            closing.shipped = true;
        }
        self.reset_if_done()
    }

    /// This state with the read counts of `counted`: what a process recovered at open, published
    /// over the shared memory's word without losing a read another connection counted there.
    pub fn with_reads_of(self, counted: Self) -> Self {
        Self {
            current: self.current,
            reads: counted.reads,
            other: self.other.map(|closing| Closing {
                reads: counted.other.map_or(0, |counted| counted.reads),
                ..closing
            }),
        }
    }

    /// This state as one word of the multi-process shared memory, so that every process's reads
    /// count where every process's checkpointer sees them, and a read's begin and its end each
    /// change it with one swap. None when a count is past what its bits hold: the read that would
    /// take it there is refused.
    pub fn pack(&self) -> Option<u64> {
        let mut word = self.reads;
        if word > READS {
            return None;
        }
        if self.current == SegmentId::Second {
            word |= SECOND_CURRENT;
        }
        if let Some(closing) = self.other {
            if closing.reads > READS {
                return None;
            }
            word |= CLOSING | closing.reads.wrapping_shl(CLOSING_READS_SHIFT);
            if closing.copied {
                word |= COPIED;
            }
            if closing.shipped {
                word |= SHIPPED;
            }
        }
        Some(word)
    }

    /// The state [`Self::pack`] stored in `word`.
    pub fn unpack(word: u64) -> Self {
        let current = if word & SECOND_CURRENT == 0 {
            SegmentId::First
        } else {
            SegmentId::Second
        };
        Self {
            current,
            reads: word & READS,
            other: (word & CLOSING != 0).then(|| Closing {
                reads: word.wrapping_shr(CLOSING_READS_SHIFT) & READS,
                copied: word & COPIED != 0,
                shipped: word & SHIPPED != 0,
            }),
        }
    }

    fn reset_if_done(&mut self) -> Option<SegmentId> {
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

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Do: pack each kind of state into the shared memory's word, counts at their widest too, and
    /// one count past it. Expect: each unpacks to itself, and a count past its bits is refused.
    #[test]
    fn d45_the_shared_word_holds_every_state_and_refuses_a_count_past_its_bits() {
        let mut log = Segments::new(SegmentId::Second);
        let mut states = vec![log];
        let read = log.begin_read();
        states.push(log);
        assert_eq!(log.switch(true), Some(SegmentId::First));
        states.push(log);
        assert_eq!(log.copied(), None);
        states.push(log);
        let widest = Segments {
            current: SegmentId::First,
            reads: READS,
            other: Some(Closing {
                reads: READS,
                copied: false,
                shipped: true,
            }),
        };
        states.push(widest);
        for state in states {
            let word = state.pack().expect("fits");
            assert_eq!(Segments::unpack(word), state);
        }
        let past = Segments {
            reads: READS + 1,
            ..widest
        };
        assert_eq!(past.pack(), None);
        let mut closing_past = widest;
        closing_past.reads = 0;
        if let Some(closing) = &mut closing_past.other {
            closing.reads = READS + 1;
        }
        assert_eq!(closing_past.pack(), None);
        assert_eq!(log.end_read(read), None);
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
