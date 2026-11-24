//! AUDIT 2026-11-23 §1 (lane perf-A3): allocations per statement on a warm cache, counted by a
//! counting allocator around the system's. Only the thread that turns counting on is counted, so
//! the other tests of this binary run as before. `--nocapture` prints the counts.
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::path::Path;
use std::sync::Arc;

use amphora_engine::Value;

struct Counting;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<u64> = const { Cell::new(0) };
}

// SAFETY: every call forwards to `System` with the caller's own arguments; the counter touches
// only this thread's locals, through `try_with`, so it neither allocates nor panics.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = COUNTING.try_with(|on| {
            if on.get() {
                let _ = COUNT.try_with(|n| n.set(n.get() + 1));
            }
        });
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract, passed on unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from `System.alloc` above with this `layout`.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// Allocations `work` made on this thread, per run of `runs`.
fn per_run(runs: u64, mut work: impl FnMut()) -> u64 {
    COUNT.with(|n| n.set(0));
    COUNTING.with(|on| on.set(true));
    for _ in 0..runs {
        work();
    }
    COUNTING.with(|on| on.set(false));
    COUNT.with(Cell::get) / runs
}

fn db() -> crate::db::Db {
    let db = crate::db::Db::open_with(
        Path::new("allocs-test.db"),
        Arc::new(amphora_engine::MemoryIO::new()),
    )
    .unwrap();
    db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, v INTEGER NOT NULL)")
        .unwrap();
    db.exec("INSERT INTO t VALUES (1, 10)").unwrap();
    db
}

/// Do: run a point read again and again on a warm statement cache. Expect: each run allocates
/// what the engine's own row needs and no more: no waker, no copy of the statement's text (on
/// main before this change: one `Arc<Unpark>` and one `Arc<str>` a statement).
#[test]
fn a_warm_point_read_allocates_neither_a_waker_nor_its_text() {
    let db = db();
    let read = || {
        let mut v = 0;
        db.query(
            "SELECT v FROM t WHERE id = ?1",
            [Value::from_i64(1)],
            |row| {
                v = row.get::<i64>(0).unwrap();
                Ok(true)
            },
        )
        .unwrap();
        assert_eq!(v, 10, "the read answers the row");
    };
    read();
    let allocations = per_run(1_000, read);
    println!("allocations per warm point read: {allocations}");
    // Measured on adacachy2: 3 on main 2d939193 (the engine's 1, an `Arc<Unpark>`, an `Arc<str>`).
    assert!(
        allocations <= 1,
        "a warm point read allocates no waker and no text: {allocations}"
    );
}

/// Do: run a transaction of one update again and again. Expect: BEGIN, the update and COMMIT
/// each allocate no waker and no copy of their text (on main before this change: one of each a
/// statement).
#[test]
fn a_warm_transaction_allocates_neither_a_waker_nor_its_text() {
    let mut db = db();
    let mut write = || {
        db.transaction(|db| db.query("UPDATE t SET v = v + 1 WHERE id = 1", [], |_| Ok(true)))
            .unwrap();
    };
    write();
    let allocations = per_run(1_000, &mut write);
    println!("allocations per warm one-update transaction: {allocations}");
    // Measured on adacachy2: 57 on main 2d939193, two of them for each of its three statements.
    assert!(
        allocations <= 51,
        "no statement of the three allocates a waker or its text: {allocations}"
    );
}
