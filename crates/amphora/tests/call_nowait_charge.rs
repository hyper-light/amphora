//! Lane perf-A7: a dropped `Calling` gives back its charge. Alone in its binary: it reads the
//! process's memory budget, which every test in a binary shares.
#![allow(
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros
)]

use std::time::{Duration, Instant};

use amphora::database::{AnswerMax, Database};
use amphora::memory::{Use, process};

fn until() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

#[test]
fn a7_a_dropped_calling_gives_back_its_charge() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("dropped.db"), until()).unwrap();
    db.call(until(), AnswerMax::fixed(), |c| {
        c.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY)")?;
        Ok(())
    })
    .unwrap();
    const LEN: usize = 1 << 17;
    let answer = AnswerMax::bytes(size_of::<Vec<u64>>() + LEN * size_of::<u64>());
    let before = process().held(Use::Queues);
    let wait_here = std::sync::mpsc::channel::<()>();
    let held = db
        .call_nowait(until(), answer, move |_| {
            let _ = wait_here.1.recv();
            Ok(vec![7u64; LEN])
        })
        .unwrap();
    assert!(
        process().held(Use::Queues) >= before + u64::try_from(answer.get()).unwrap(),
        "{answer:?} not charged"
    );
    drop(held);
    drop(wait_here.0);
    // The writer runs calls in order and answers them in order: once a later call has its
    // answer, the dropped ones have theirs, and their charges are back.
    db.call(until(), AnswerMax::fixed(), |_| Ok(())).unwrap();
    assert_eq!(process().held(Use::Queues), before);
}
