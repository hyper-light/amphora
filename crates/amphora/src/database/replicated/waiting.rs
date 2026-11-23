//! The submits waiting behind the owner's prepared write (AUDIT 6 item 2): first come, first
//! served, and found by deadline. The owner asks for the soonest deadline and the expired every
//! step; ordered by deadline beside their order of arrival, each costs O(log N) in the waiting,
//! not a scan and a rotation of the whole queue.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

pub(super) struct Waiting<T> {
    /// Each by its place in line: a submit moved back to the front takes a place before the first.
    line: BTreeMap<i64, (Instant, T)>,
    /// The same places by deadline, ties in line order.
    deadlines: BTreeSet<(Instant, i64)>,
}

impl<T> Waiting<T> {
    pub(super) fn new() -> Self {
        Self {
            line: BTreeMap::new(),
            deadlines: BTreeSet::new(),
        }
    }

    pub(super) fn len(&self) -> usize {
        self.line.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.line.is_empty()
    }

    /// `item`, due by `until`, last in line.
    pub(super) fn push_back(&mut self, until: Instant, item: T) {
        let place = match self.line.last_key_value() {
            None => 0,
            Some((last, _)) => match last.checked_add(1) {
                Some(place) => place,
                None => self.renumbered().saturating_add(1),
            },
        };
        self.insert(place, until, item);
    }

    /// `item`, due by `until`, first in line.
    pub(super) fn push_front(&mut self, until: Instant, item: T) {
        let place = match self.line.first_key_value() {
            None => 0,
            Some((first, _)) => match first.checked_sub(1) {
                Some(place) => place,
                None => {
                    // Renumbered from 0, the first stands at 0 and -1 is free.
                    self.renumbered();
                    -1
                }
            },
        };
        self.insert(place, until, item);
    }

    /// The first in line.
    pub(super) fn pop_front(&mut self) -> Option<T> {
        let (place, (until, item)) = self.line.pop_first()?;
        self.deadlines.remove(&(until, place));
        Some(item)
    }

    /// The soonest deadline of those waiting.
    pub(super) fn next_deadline(&self) -> Option<Instant> {
        self.deadlines.first().map(|(until, _)| *until)
    }

    /// Those whose deadline is at or before `at`, taken out of line, soonest first; the rest
    /// keep their order.
    pub(super) fn expire(&mut self, at: Instant) -> Vec<T> {
        let mut expired = Vec::new();
        while let Some(&(until, place)) = self.deadlines.first()
            && until <= at
        {
            self.deadlines.pop_first();
            if let Some((_, item)) = self.line.remove(&place) {
                expired.push(item);
            }
        }
        expired
    }

    fn insert(&mut self, place: i64, until: Instant, item: T) {
        self.deadlines.insert((until, place));
        self.line.insert(place, (until, item));
    }

    /// Every place numbered again from 0 in line order, for a line that drifted to the end of
    /// the numbers (2^63 submits with the line never empty): the last place.
    fn renumbered(&mut self) -> i64 {
        let line = std::mem::take(&mut self.line);
        self.deadlines.clear();
        let mut last = -1;
        for (place, (_, (until, item))) in (0_i64..).zip(line) {
            self.insert(place, until, item);
            last = place;
        }
        last
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::time::Duration;

    /// AUDIT 6 item 2: the waiting line answers as the queue it replaces did. Do: submits with
    /// deadlines out of arrival order, one moved back to the front, then expire at a time between
    /// them. Expect: the expired are those due by then; the rest come out first come, first
    /// served, the moved one first; the soonest deadline is the least of those left.
    #[test]
    fn audit6_waiting_keeps_line_order_and_expires_by_deadline() {
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let mut waiting = Waiting::new();
        waiting.push_back(at(30), "a");
        waiting.push_back(at(10), "b");
        waiting.push_back(at(50), "c");
        waiting.push_back(at(20), "d");
        waiting.push_front(at(40), "front");
        assert_eq!(waiting.len(), 5);
        assert_eq!(waiting.next_deadline(), Some(at(10)));
        assert_eq!(waiting.expire(at(20)), vec!["b", "d"]);
        assert_eq!(waiting.next_deadline(), Some(at(30)));
        assert_eq!(waiting.pop_front(), Some("front"));
        assert_eq!(waiting.pop_front(), Some("a"));
        assert_eq!(waiting.next_deadline(), Some(at(50)));
        assert_eq!(waiting.pop_front(), Some("c"));
        assert!(waiting.is_empty());
        assert_eq!(waiting.next_deadline(), None);
        assert_eq!(waiting.pop_front(), None);
    }

    /// AUDIT 6 item 2: the line holds its order across either end of its numbers. Do: places
    /// at the first and last numbers, push at the front; a place at the last number again, push
    /// at the back. Expect: every item out in line order.
    #[test]
    fn audit6_waiting_renumbers_at_the_end_of_its_numbers() {
        let until = Instant::now();
        let mut waiting = Waiting::new();
        waiting.insert(i64::MIN, until, 1);
        waiting.insert(i64::MAX, until, 2);
        waiting.push_front(until, 0);
        waiting.insert(i64::MAX, until, 3);
        waiting.push_back(until, 4);
        let order: Vec<i32> = std::iter::from_fn(|| waiting.pop_front()).collect();
        assert_eq!(order, vec![0, 1, 2, 3, 4]);
        assert!(waiting.deadlines.is_empty());
    }

    /// AUDIT 6 item 2's A/B: the owner's per-step work on its waiting submits with N waiting and
    /// none due, the old queue's scan and rotation against this line's. Run:
    /// `cargo test --release -p amphora --lib audit6_measure_waiting -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement"]
    fn audit6_measure_waiting() {
        let start = Instant::now();
        let steps: u32 = 20_000;
        for n in [1_u64, 100, 1_000, 10_000] {
            // Deadlines spread over a minute, none due at `start`.
            let until = |i: u64| start + Duration::from_micros(1 + i.wrapping_mul(7919) % 60_000_000);
            let mut queue: VecDeque<(Instant, u64)> = (0..n).map(|i| (until(i), i)).collect();
            let mut line = Waiting::new();
            for i in 0..n {
                line.push_back(until(i), i);
            }
            let began = Instant::now();
            let mut soonest = start;
            for _ in 0..steps {
                soonest = queue.iter().map(|(until, _)| *until).fold(start + Duration::from_secs(120), Instant::min);
                for _ in 0..queue.len() {
                    let Some(entry) = queue.pop_front() else { break };
                    if start >= entry.0 {
                        continue;
                    }
                    queue.push_back(entry);
                }
            }
            let before = began.elapsed() / steps;
            std::hint::black_box(soonest);
            let began = Instant::now();
            let mut soonest = None;
            let mut expired = 0;
            for _ in 0..steps {
                soonest = line.next_deadline();
                expired += line.expire(start).len();
            }
            let after = began.elapsed() / steps;
            std::hint::black_box((soonest, expired));
            println!(
                "audit6 waiting n={n}: before {} ns/step, after {} ns/step",
                before.as_nanos(),
                after.as_nanos()
            );
        }
    }
}
