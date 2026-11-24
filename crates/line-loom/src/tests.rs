use super::*;
use line::{Reservation, Room, alone};
use loom::thread;
use std::sync::atomic::Ordering;

/// A deadline no test's clock reaches, and one a test's clock passes.
const FAR: Instant = Instant(u64::MAX);
const SOON: Instant = Instant(1);
/// The bytes of a call in these queues, and of the record that finds a leader with its envelope.
const CALL: usize = 10;
const RECORD: usize = RECORD_BYTES + ENVELOPE_BYTES;

/// `model` on every interleaving: all of them (none), or those of at most `preemptions` of one
/// thread by another where all of them are too many to run.
fn every(preemptions: Option<usize>, model: impl Fn() + Send + Sync + 'static) {
    let mut builder = loom::model::Builder::new();
    builder.preemption_bound = preemptions;
    builder.check(model);
}
/// A queue of a hundred bytes, fifty of them the calls', that holds `commands` inputs.
fn queue(commands: usize) -> Arc<Room> {
    Room::new(100, 50, commands)
}
/// Until `calls` wait in line.
fn in_line(room: &Arc<Room>, calls: usize) {
    while room.line.waiting.load(Ordering::SeqCst) < calls {
        thread::yield_now();
    }
}
/// A call of `CALL` bytes on a thread, waiting for its place until `until`, handed to `then`.
fn call<T: 'static>(
    room: &Arc<Room>,
    until: Instant,
    then: impl FnOnce(Result<Reservation, Refusal>) -> T + 'static,
) -> thread::JoinHandle<T> {
    let room = Arc::clone(room);
    thread::spawn(move || then(Room::place_by(&room, CALL, until)))
}
/// The queue holds nothing and nothing waits for it.
fn holds_nothing(room: &Arc<Room>) {
    for (what, held) in [
        ("bytes", &room.admitted),
        ("calls' bytes", &room.called),
        ("bytes charged", &room.charged),
        ("inputs", &room.commands),
        ("calls in line", &room.line.waiting),
    ] {
        assert_eq!(held.load(Ordering::SeqCst), 0, "{what}");
    }
}

/// T-QB.1.19: no wake is lost and nothing is left held. Do: while the one place calls may take is
/// held, a call comes as it is given back. Expect: the call admitted on every interleaving.
#[test]
fn t_qb_1_19_a_place_given_back_reaches_the_call_that_waits_for_it() {
    every(None, || {
        let room = queue(2);
        let held = Room::place(&room, CALL, true).unwrap();
        let waited = call(&room, FAR, |place| place.map(drop));
        drop(held);
        assert_eq!(waited.join().unwrap(), Ok(()));
        holds_nothing(&room);
    });
}

/// T-QB.1.19: bytes given back as the owner's hold ends reach the call that waits for them, not the
/// place alone its taking gave back. Do: a call holds the calls' bytes; the owner takes it from the
/// channel, then is done with it; another call comes. Expect: the other admitted.
#[test]
fn t_qb_1_19_bytes_given_back_reach_the_call_that_waits_for_them() {
    every(None, || {
        let room = Room::new(100, CALL + CALL / 2, 3);
        let mut held = Room::place(&room, CALL, true).unwrap();
        let waited = call(&room, FAR, |place| place.map(drop));
        held.taken();
        drop(held);
        assert_eq!(waited.join().unwrap(), Ok(()));
        holds_nothing(&room);
    });
}

/// T-QB.1.19: the bounds learned again reach the call they make room for. Do: with the one place
/// taken and kept, a call comes as the owner learns a bound of one input more. Expect: the call
/// admitted, the kept place still held.
#[test]
fn t_qb_1_19_bounds_learned_again_reach_the_call_they_make_room_for() {
    every(None, || {
        let room = queue(2);
        let held = Room::place(&room, CALL, true).unwrap();
        let waited = call(&room, FAR, |place| place.map(drop));
        room.command_bound.store(3, Ordering::SeqCst);
        room.line.room_came();
        assert_eq!(waited.join().unwrap(), Ok(()));
        drop(held);
        holds_nothing(&room);
    });
}

/// T-QB.1.19: first come, first served, one place for one call at a time. Do: with the one place
/// held, a call stands in line, then another behind it; the place is given back. Expect: the first
/// admitted first, never both at once, and none left.
#[test]
fn t_qb_1_19_the_first_in_line_is_the_first_admitted_and_no_place_is_taken_twice() {
    every(Some(3), || {
        let room = queue(2);
        let held = Room::place(&room, CALL, true).unwrap();
        let (order, holding) = (
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(loom::sync::atomic::AtomicUsize::new(0)),
        );
        let admit = |id: usize| {
            let (order, holding) = (Arc::clone(&order), Arc::clone(&holding));
            move |place: Result<Reservation, Refusal>| {
                let place = place.unwrap();
                assert_eq!(holding.fetch_add(1, Ordering::SeqCst), 0, "two hold one place");
                order.lock().unwrap().push(id);
                holding.fetch_sub(1, Ordering::SeqCst);
                drop(place);
            }
        };
        let first = call(&room, FAR, admit(1));
        in_line(&room, 1);
        let second = call(&room, FAR, admit(2));
        in_line(&room, 2);
        drop(held);
        first.join().unwrap();
        second.join().unwrap();
        assert_eq!(*order.lock().unwrap(), vec![1, 2]);
        holds_nothing(&room);
    });
}

/// T-QB.1.19: two places given back reach two calls in line, though one event may ring the first
/// alone. Do: two calls wait behind two held places; both are given back at once. Expect: both
/// admitted.
#[test]
fn t_qb_1_19_two_places_given_back_at_once_reach_both_calls_in_line() {
    every(Some(3), || {
        let room = queue(3);
        let held = [
            Room::place(&room, CALL, true).unwrap(),
            Room::place(&room, CALL, true).unwrap(),
        ];
        let waited = [
            call(&room, FAR, |place| place.map(drop)),
            call(&room, FAR, |place| place.map(drop)),
        ];
        drop(held);
        for waited in waited {
            assert_eq!(waited.join().unwrap(), Ok(()));
        }
        holds_nothing(&room);
    });
}

/// T-QB.1.19: a deadline always ends the wait, and with `Full`, holding nothing. Do: with the one
/// place held throughout, a call whose deadline passes as it comes, or while it waits. Expect: `Full`
/// at the inputs' bound.
#[test]
fn t_qb_1_19_a_deadline_ends_the_wait_with_full() {
    every(None, || {
        let room = queue(2);
        let held = Room::place(&room, CALL, true).unwrap();
        let waited = call(&room, SOON, |place| place.map(drop));
        CLOCK.pass(1);
        assert_eq!(waited.join().unwrap(), Err(Refusal::Full { limit: 2 }));
        drop(held);
        holds_nothing(&room);
    });
}

/// T-QB.1.19: a deadline that passes as the room comes answers its call one way, and the call behind
/// it has the room either way. Do: a call whose deadline passes, and one behind it, as the place is
/// given back. Expect: the first admitted or `Full`, the second admitted.
#[test]
fn t_qb_1_19_a_deadline_beside_a_give_back_leaves_the_next_call_its_room() {
    every(Some(2), || {
        let room = queue(2);
        let held = Room::place(&room, CALL, true).unwrap();
        let first = call(&room, SOON, |place| place.map(drop));
        in_line(&room, 1);
        let second = call(&room, FAR, |place| place.map(drop));
        let timer = thread::spawn(|| CLOCK.pass(1));
        drop(held);
        let answered = first.join().unwrap();
        assert!(
            matches!(answered, Ok(()) | Err(Refusal::Full { .. })),
            "{answered:?}"
        );
        assert_eq!(second.join().unwrap(), Ok(()));
        timer.join().unwrap();
        holds_nothing(&room);
    });
}

/// T-QB.1.19: an owner's end wakes every call in line, with its reason, and each that joins after.
/// Do: two calls wait, or come, as the owner ends; then one more. Expect: each refused with the
/// owner's reason or, where the place was free, admitted; none waits on.
#[test]
fn t_qb_1_19_the_owners_end_wakes_every_call_in_line() {
    every(Some(3), || {
        let room = queue(2);
        let held = Room::place(&room, CALL, true).unwrap();
        let waited = [
            call(&room, FAR, |place| place.map(drop)),
            call(&room, FAR, |place| place.map(drop)),
        ];
        let reason = Refusal::Fenced("ended".into());
        room.line.end(&reason);
        for waited in waited {
            assert_eq!(waited.join().unwrap(), Err(reason.clone()));
        }
        assert_eq!(
            Room::place_by(&room, CALL, FAR).map(drop),
            Err(reason.clone())
        );
        drop(held);
        holds_nothing(&room);
    });
}

/// T-QB.1.19: a wire record finds its place and its bytes whatever the calls do, so none of them
/// starves it. Do: calls fill the queue's places and bytes but a wire record's, one of them in line;
/// a record comes as a place is given back. Expect: the record admitted at once, always; the calls
/// after it.
#[test]
fn t_qb_1_19_a_record_is_admitted_at_once_whatever_the_calls_do() {
    every(Some(3), || {
        // Fifty bytes are the calls', and two inputs, of which a call takes one.
        let room = Room::new(50 + RECORD, 50, 2);
        let held = Room::place(&room, 50, true).unwrap();
        let waited = call(&room, FAR, |place| place.map(drop));
        let record = {
            let room = Arc::clone(&room);
            thread::spawn(move || Room::place(&room, RECORD, false).map(drop))
        };
        drop(held);
        assert_eq!(record.join().unwrap(), Ok(()));
        assert_eq!(waited.join().unwrap(), Ok(()));
        holds_nothing(&room);
    });
}

/// T-QB.1.19 (6c9ca0c0's hole): the largest call a queue holds alone, held or coming, never refuses
/// the record that finds a leader, and waits for a call held before it. Do: a small call held; the
/// largest comes, and a record, as the small one is given back. Expect: the record admitted at once
/// on every interleaving, the call admitted, and nothing left.
#[test]
fn t_qb_1_19_the_largest_call_never_refuses_the_record_that_finds_a_leader() {
    every(Some(3), || {
        let queued = 100;
        let room = Room::new(queued, 50, 3);
        let held = Room::place(&room, 5, true).unwrap();
        let largest = {
            let room = Arc::clone(&room);
            thread::spawn(move || Room::place_by(&room, alone(queued), FAR).map(drop))
        };
        let record = {
            let room = Arc::clone(&room);
            thread::spawn(move || Room::place(&room, RECORD, false).map(drop))
        };
        drop(held);
        assert_eq!(record.join().unwrap(), Ok(()));
        assert_eq!(largest.join().unwrap(), Ok(()));
        holds_nothing(&room);
    });
}

/// T-QB.1.19: a place granted while the budget's room went is given back, and the call waits for the
/// budget again, admitted only with its charge. Do: the one place held, a write charged five bytes of
/// a budget of five; another holder takes the five and lets them go, as the place is given back.
/// Expect: the write admitted, both its charge and its place held, and nothing after.
#[test]
fn t_qb_1_19_a_granted_place_whose_budget_room_went_waits_for_it_again() {
    every(Some(3), || {
        let budget = memory::Budget::of(5);
        let room = queue(2);
        let held = Room::place(&room, 5, true).unwrap();
        let write = {
            let (room, budget) = (Arc::clone(&room), Arc::clone(&budget));
            thread::spawn(move || {
                let place = Room::submit_by(&room, 5, FAR).unwrap();
                assert_eq!(room.charged.load(Ordering::SeqCst), 5, "admitted uncharged");
                assert_eq!(budget.held(), 5, "admitted without its charge");
                drop(place);
            })
        };
        let other = {
            let budget = Arc::clone(&budget);
            thread::spawn(move || drop(budget.take(memory::Use::Queues, 5)))
        };
        drop(held);
        write.join().unwrap();
        other.join().unwrap();
        holds_nothing(&room);
        assert_eq!(budget.held(), 0);
    });
}

/// T-QB.1.19: the same, the room never given back: `Full` at the deadline, the place left to the call
/// in line behind. Do: the one place held, a write of five bytes; another holder takes the budget's
/// five for good; the place is given back, then the write's deadline passes. Expect: the write
/// admitted before the other took it, or `Full`; its place never held without its charge; a call
/// behind it admitted.
#[test]
fn t_qb_1_19_a_granted_place_whose_budget_room_never_comes_is_full_at_its_deadline() {
    every(Some(2), || {
        let budget = memory::Budget::of(5);
        let room = queue(2);
        let held = Room::place(&room, 5, true).unwrap();
        let write = {
            let room = Arc::clone(&room);
            thread::spawn(move || Room::submit_by(&room, 5, SOON).map(drop))
        };
        let other = {
            let budget = Arc::clone(&budget);
            thread::spawn(move || budget.take(memory::Use::Queues, 5).ok())
        };
        drop(held);
        CLOCK.pass(1);
        let answered = write.join().unwrap();
        assert!(
            matches!(answered, Ok(()) | Err(Refusal::Full { .. })),
            "{answered:?}"
        );
        assert_eq!(Room::place_by(&room, CALL, FAR).map(drop), Ok(()));
        drop(other.join().unwrap());
        holds_nothing(&room);
        assert_eq!(budget.held(), 0);
    });
}

/// T-QB.1.19: writes that need both the budget's only room and the queue's only place never hold one
/// and wait for the other for good. Do: two writes at once, each needing all of both. Expect: both
/// admitted, one after the other.
#[test]
fn t_qb_1_19_two_writes_that_need_all_the_room_are_both_admitted() {
    every(Some(3), || {
        let budget = memory::Budget::of(5);
        let room = queue(2);
        let write = || {
            let room = Arc::clone(&room);
            thread::spawn(move || Room::submit_by(&room, 5, FAR).map(drop))
        };
        let writes = [write(), write()];
        for write in writes {
            assert_eq!(write.join().unwrap(), Ok(()));
        }
        holds_nothing(&room);
        assert_eq!(budget.held(), 0);
    });
}
