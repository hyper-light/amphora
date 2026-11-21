//! T-G2.7.2: a corpus file run on Amphora through the gate's own code path (amphora-corpus's
//! runner), SQLite's answers in the file.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use amphora_corpus::amphora::{Amphora, AmphoraRender};
use amphora_corpus::run::{Outcome, run_file};

const FILE: &str = "hash-threshold 4

statement ok
CREATE TABLE t(a INTEGER, b TEXT, c REAL)

statement ok
INSERT INTO t VALUES (1, 'one', 1.5), (2, 'two', NULL), (3, NULL, 2.25)

query ITR rowsort
SELECT a, b, c FROM t
----
6 values hashing to 0123456789abcdef0123456789abcdef

query R nosort
SELECT sum(c) FROM t
----
3.750

query T nosort
SELECT 1e15
----
1000000000000000.0

statement error
SELECT * FROM nope

onlyif mysql
query I nosort
SELECT 9
----
9
";

/// Do: run a file whose hashed answer is wrong on purpose, then the same file with it right.
/// Expect: that query, and only it, disagrees; then every record agrees, the MySQL-only one skipped.
#[test]
fn t_g2_7_2_a_file_runs_on_amphora_as_sqlite_runs_it() {
    let right = amphora_corpus::answer::md5_of(
        &[
            "1", "one", "1.500", "2", "two", "NULL", "3", "NULL", "2.250",
        ]
        .map(String::from),
    );
    let wrong_hash = FILE.replace("6 values", "9 values");
    let mut render = AmphoraRender::new().unwrap();
    let mut off = Vec::new();
    let counts = run_file(
        &mut Amphora::in_memory().unwrap(),
        &mut render,
        &wrong_hash,
        None,
        None,
        &mut |r| {
            if r.outcome != Outcome::Agree {
                off.push((r.record.line, r.outcome));
            }
        },
    )
    .unwrap();
    assert_eq!(off.len(), 1, "{off:?}");
    assert_eq!(off[0].0, 9);
    assert_eq!(counts.disagree, 1);
    let fixed = wrong_hash.replace("0123456789abcdef0123456789abcdef", &right);
    let mut off = Vec::new();
    let counts = run_file(
        &mut Amphora::in_memory().unwrap(),
        &mut render,
        &fixed,
        None,
        None,
        &mut |r| {
            if r.outcome != Outcome::Agree {
                off.push((r.record.line, r.outcome));
            }
        },
    )
    .unwrap();
    assert!(off.is_empty(), "{off:?}");
    assert_eq!((counts.agree, counts.skipped, counts.unsettled), (6, 1, 0));
}

/// Do: run a file with a deadline already past. Expect: no record run, each counted unreached.
#[test]
fn records_past_the_deadline_are_unreached_not_failed() {
    let mut render = AmphoraRender::new().unwrap();
    let past = std::time::Instant::now();
    let counts = run_file(
        &mut Amphora::in_memory().unwrap(),
        &mut render,
        FILE,
        None,
        Some(past),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!((counts.agree, counts.disagree, counts.unreached), (0, 0, 6));
}
