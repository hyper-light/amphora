//! Lane perf-A7: a dropped `Calling` gives back its charge. Alone in its binary: it reads the
//! process's memory budget, which every test in a binary shares.
#![allow(
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros
)]

use std::time::{Duration, Instant};

use amphora::database::{AnswerCharge, Database};
use amphora::memory::{Use, process};

fn until() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

#[test]
fn a7_a_dropped_calling_gives_back_its_charge() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("dropped.db"), until()).unwrap();
    db.call(until(), |c| {
        c.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY)")?;
        Ok(())
    })
    .unwrap();
    let before = process().held(Use::Queues);
    for charge in [
        AnswerCharge::Flat,
        AnswerCharge::Declared(1 << 20),
        AnswerCharge::Measured,
    ] {
        let wait_here = std::sync::mpsc::channel::<()>();
        let held = db
            .call_nowait(until(), charge, move |_| {
                let _ = wait_here.1.recv();
                Ok(vec![7u64; 1 << 17])
            })
            .unwrap();
        assert!(
            process().held(Use::Queues) > before,
            "{charge:?} charged nothing"
        );
        drop(held);
        drop(wait_here.0);
    }
    // The writer runs calls in order and answers them in order: once a later call has its
    // answer, the dropped ones have theirs, and their charges are back.
    db.call(until(), |_| Ok(())).unwrap();
    assert_eq!(process().held(Use::Queues), before);
}
