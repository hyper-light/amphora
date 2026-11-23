//! The replicated owner's queue bounds, learned from how fast each queue drains where it runs
//! (replication.md, "Decision (lane constants): the queue bounds by drain rate"; QB.1). A queue
//! holds what its reader takes in an election timeout: what waits longer than that is older than
//! the time in which the group elects another leader, so holding more serves no one. The outgoing
//! queue's reader is the host (`ReplicaHandle::drain`), the command queue's the owner's intake.
//!
//! The rate is a decaying ratio of sums, bytes and messages over the nanoseconds the queue held
//! work, each sum kept at seven eighths of itself before a sample is added: RFC 6298 §2's gain of
//! 1/8 (Jacobson, "Congestion Avoidance and Control", SIGCOMM 1988), as the compaction's costs are
//! (CT.1), the eighth taken rounded up so that a sum no sample feeds decays to nothing. The sums
//! are 64.64 fixed point, as many bits below the point as a sample has above it (u64), so the
//! eighth rounded up is 2^-64 at most (lane constants-K4): rounded up in whole units, a sum fed one
//! message an interval settled at one, not eight, and learned an eighth of the reader's rate. An
//! interval counts when the queue held work at its start or its reader took some in it: one with
//! nothing to read and nothing read tells nothing of how fast it reads. The election timeout is
//! learned too: `election_tick` periods of the owner's measured pace (TP.1). Each bound is the lesser of the rate times that
//! timeout and the memory budget's room for queues (MB.1), no less than the least a queue can
//! carry and still work (the outgoing queue's: a record to each peer, one broadcast); before
//! anything is measured, the room.
//!
//! A pure core: the owner tells it what was drained and reads its own clock; nothing here reads a
//! clock, the disk or the memory.

use std::time::Duration;

/// A queue's bound: its bytes and its messages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Bound {
    pub(super) bytes: u64,
    pub(super) messages: u64,
}

/// What a queue's reader drained, and the rate learned from it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Drain {
    bytes: u128,
    messages: u128,
    nanos: u128,
    /// Bytes and messages drained since the last sample, not yet weighed.
    pending: (u64, u64),
    /// The clock reading the interval under way began at, and whether the queue held work then.
    since: Option<(u64, bool)>,
}

impl Drain {
    /// The reader took `messages` holding `bytes`.
    pub(super) fn drained(&mut self, bytes: u64, messages: u64) {
        self.pending = (
            self.pending.0.saturating_add(bytes),
            self.pending.1.saturating_add(messages),
        );
    }

    /// The owner's clock reads `now`, the queue holding work (`busy`) or not: the interval since
    /// the last sample is weighed if the queue held work at its start or some was drained in it,
    /// and a new one begins.
    pub(super) fn sample(&mut self, now: u64, busy: bool) {
        match self.since {
            Some((at, held)) if now > at => {
                if held || self.pending.1 > 0 {
                    self.weigh(now.saturating_sub(at));
                }
                self.pending = (0, 0);
                self.since = Some((now, busy));
            }
            // No time passed on the clock: below its resolution, the interval goes on.
            Some((at, held)) => self.since = Some((at, held || busy)),
            None => {
                self.pending = (0, 0);
                self.since = Some((now, busy));
            }
        }
    }

    /// Adds an interval of `nanos` and what was drained in it to the sums.
    fn weigh(&mut self, nanos: u64) {
        // RFC 6298 §2's gain: the old sums keep seven eighths, the eighth rounded up.
        let add = |sum: u128, sample: u64| {
            sum.saturating_sub(sum.div_ceil(8))
                .saturating_add(u128::from(sample).saturating_mul(ONE))
        };
        self.nanos = add(self.nanos, nanos);
        self.bytes = add(self.bytes, self.pending.0);
        self.messages = add(self.messages, self.pending.1);
    }

    /// The bound in force: what the reader drains in `timeout` at its measured rate, no less than
    /// `least` (one record), its bytes no more than `room` (the memory budget's room for queues).
    pub(super) fn bound(&self, timeout: Duration, least: Bound, room: u64) -> Bound {
        // Nothing measured yet: the memory budget's room, the only bound known before the reader
        // has drained (QB.1). One record would fence a host whose first burst outruns it.
        if self.nanos == 0 {
            return Bound {
                bytes: room,
                messages: u64::MAX,
            };
        }
        let within = |sum: u128| scaled(sum, timeout.as_nanos(), self.nanos);
        Bound {
            bytes: within(self.bytes).max(least.bytes).min(room),
            messages: within(self.messages).max(least.messages),
        }
    }
}

impl Drain {
    /// The bytes the reader drains in `over` at its measured rate: what its path carries in that
    /// time. `None` before anything is measured.
    pub(super) fn carried(&self, over: Duration) -> Option<u64> {
        if self.nanos == 0 {
            return None;
        }
        Some(scaled(self.bytes, over.as_nanos(), self.nanos))
    }
}

/// One in the sums' 64.64 fixed point.
const ONE: u128 = 1 << u64::BITS;

/// `sum` × `by` / `over`, rounded down, a u64's most past it; `sum` and `over` share their scale.
/// As `sum` = whole × `over` + part with part under `over`, the product is whole × `by` plus part ×
/// `by` / `over`, under `by`; when part × `by` passes a u128, part and `over` drop the same low
/// bits first, below the sums' own 2^-64.
fn scaled(sum: u128, by: u128, over: u128) -> u64 {
    let (Some(whole), Some(part)) = (sum.checked_div(over), sum.checked_rem(over)) else {
        return u64::MAX;
    };
    let width = |n: u128| u128::BITS.saturating_sub(n.leading_zeros());
    let drop = width(part)
        .saturating_add(width(by))
        .saturating_sub(u128::BITS);
    let rest = part
        .checked_shr(drop)
        .unwrap_or(0)
        .saturating_mul(by)
        .checked_div(over.checked_shr(drop).unwrap_or(0))
        .unwrap_or(0);
    u64::try_from(whole.saturating_mul(by).saturating_add(rest)).unwrap_or(u64::MAX)
}

/// The window a leader keeps in flight to each member (lane constants-21): two candidates behind
/// one switch, decided by the A/B scenario (`window_slow_flooded_fast_starved`), the picked one
/// its control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Window {
    /// The control: four messages and 64 MiB to every member, picked. Only the A/B's tests open
    /// owners with it (`opened_window`); production opens with the default.
    #[cfg_attr(not(test), allow(dead_code))]
    Picked,
    /// hyper-raft's own rule (mantle note 32 R16): no count of its own, and twice what the path
    /// carries in a round trip (`hyper_timing::inflight_window`), the path's drain over its round
    /// trip as `pace` measures it, at least a record's own bytes; before a path is measured, the
    /// memory room, as the queues' bounds start.
    Carried,
}

impl Window {
    /// The window an owner opens with.
    pub(super) const DEFAULT: Self = Self::Carried;
}

/// What a replicated owner does when its outgoing queue is past its learned bound (QB.2): three
/// candidates behind one switch, decided by the A/B scenario
/// (`qb2_many_slow_one_stalled_for_good_by_egress`), the fence its control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Egress {
    /// QB.1's rule: the owner fences once the queue has stayed past its bound an election timeout.
    Fence,
    /// The oldest records past the bound are dropped, counted, and their members reported
    /// unreachable: etcd's transport drops a message whose peer's buffer is full and reports the
    /// peer unreachable (etcd server/etcdserver/api/rafthttp/peer.go, `send`; raft's
    /// `ReportUnreachable`). Once the host's drain leaves room for a record to each peer, a
    /// leader beats, and each member's answer brings what it lacks: the drain prompts the resend,
    /// not the next heartbeat tick (`Actor::resend`).
    Shed,
    /// The owner takes no input and makes no raft step while the queue is at its bound.
    Pause,
}

impl Egress {
    /// The candidate in force unless a test switches it.
    pub(super) const DEFAULT: Self = Self::Shed;

    pub(super) fn code(self) -> u8 {
        match self {
            Self::Fence => 0,
            Self::Shed => 1,
            Self::Pause => 2,
        }
    }

    /// The candidate a code names; an unknown code is the default.
    pub(super) fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Fence,
            1 => Self::Shed,
            2 => Self::Pause,
            _ => Self::DEFAULT,
        }
    }
}

/// Whether a queue holding `bytes` in `messages` is past `bound`.
pub(super) fn past(bound: Bound, bytes: u64, messages: u64) -> bool {
    bytes > bound.bytes || messages > bound.messages
}

/// The learned election timeout: `election_tick` periods of the owner's pace.
pub(super) fn election_timeout(election_tick: usize, period: Duration) -> Duration {
    period.saturating_mul(u32::try_from(election_tick).unwrap_or(u32::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEAST: Bound = Bound {
        bytes: 100,
        messages: 1,
    };
    const MS: u64 = 1_000_000;

    /// T-QB.2.1: each candidate's code names it again, and a queue is past its bound once either
    /// its bytes or its messages are.
    #[test]
    fn qb2_egress_codes_and_past() {
        for egress in [Egress::Fence, Egress::Shed, Egress::Pause] {
            assert_eq!(Egress::from_code(egress.code()), egress);
        }
        assert_eq!(Egress::from_code(u8::MAX), Egress::DEFAULT);
        assert!(!past(LEAST, 100, 1));
        assert!(past(LEAST, 101, 1));
        assert!(past(LEAST, 1, 2));
    }

    /// T-QB.1.1: nothing measured, then an interval with nothing queued and nothing read: passes
    /// when the bound is one record (`least`), the room no matter how large.
    #[test]
    fn drain_nothing_measured_bounds_the_room() {
        let mut drain = Drain::default();
        let timeout = Duration::from_secs(1);
        let room = Bound {
            bytes: 4000,
            messages: u64::MAX,
        };
        assert_eq!(drain.bound(timeout, LEAST, 4000), room);
        drain.sample(0, false);
        drain.sample(10 * MS, false);
        assert_eq!(
            drain.bound(timeout, LEAST, 4000),
            room,
            "an idle start tells nothing"
        );
    }

    /// T-QB.1.2: a reader that drains 1000 bytes in 10 messages every 10 ms while the queue holds
    /// work, under a 100 ms election timeout: passes when the bound is ten intervals' worth, then
    /// the budget's room when that is less, and messages beyond the room's reach.
    #[test]
    fn drain_bound_is_rate_times_election_timeout_within_room() {
        let mut drain = Drain::default();
        drain.sample(0, true);
        for step in 1..=40u64 {
            drain.drained(1000, 10);
            drain.sample(step * 10 * MS, true);
        }
        let timeout = election_timeout(10, Duration::from_millis(10));
        assert_eq!(timeout, Duration::from_millis(100));
        let bound = drain.bound(timeout, LEAST, u64::MAX);
        assert!((9_000..=11_000).contains(&bound.bytes), "{bound:?}");
        assert!((90..=110).contains(&bound.messages), "{bound:?}");
        assert_eq!(drain.bound(timeout, LEAST, 4000).bytes, 4000);
    }

    /// T-QB.1.3: the reader slows to a tenth: passes when the bound follows it down as the old
    /// samples decay, never below one record; and an interval with no time on the clock adds none.
    #[test]
    fn drain_bound_follows_the_reader_and_ignores_no_time() {
        let mut drain = Drain::default();
        drain.sample(0, true);
        let mut now = 0;
        for _ in 0..40 {
            now += 10 * MS;
            drain.drained(1000, 10);
            drain.sample(now, true);
        }
        let timeout = Duration::from_millis(100);
        let fast = drain.bound(timeout, LEAST, u64::MAX);
        for _ in 0..80 {
            now += 10 * MS;
            drain.drained(100, 1);
            drain.sample(now, true);
        }
        let slow = drain.bound(timeout, LEAST, u64::MAX);
        assert!(slow.bytes < fast.bytes / 5, "{slow:?} after {fast:?}");
        assert!(slow.bytes >= 900 && slow.messages >= 9, "{slow:?}");
        let before = drain;
        drain.drained(5, 1);
        drain.sample(now, true);
        assert_eq!(
            drain.bound(timeout, LEAST, u64::MAX),
            before.bound(timeout, LEAST, u64::MAX)
        );
        // A reader that stops while work waits: the bound falls to one record.
        for _ in 0..400 {
            now += 10 * MS;
            drain.sample(now, true);
        }
        assert_eq!(drain.bound(timeout, LEAST, u64::MAX), LEAST);
    }

    /// Lane constants-K4 (K3's finding): a reader that takes one message of 99 bytes in each 10 ms
    /// it holds work, under a 105 ms election timeout. Passes when the bound is what it takes in
    /// the timeout, 10.5 messages and 1039.5 bytes rounded down, from the first interval on and
    /// 400 intervals later. With the eighth taken in whole messages, the sum of one message an
    /// interval settled at one, not eight, and the bound was one message: an eighth of the rate.
    #[test]
    fn a_reader_taking_one_message_an_interval_learns_its_rate() {
        let mut drain = Drain::default();
        let timeout = Duration::from_millis(105);
        let rate = Bound {
            bytes: 1039,
            messages: 10,
        };
        drain.sample(0, true);
        for step in 1..=400u64 {
            drain.drained(99, 1);
            drain.sample(step * 10 * MS, true);
            if step == 1 || step == 400 {
                assert_eq!(drain.bound(timeout, LEAST, u64::MAX), rate, "{step}");
            }
        }
    }
}
