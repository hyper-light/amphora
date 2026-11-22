//! The suite's tables (CP-T1.5, AC-T1.5.3: the suite past 200 cases): each case runs one row of a
//! table of inputs (a key's bytes, a value, a family's name, a page's size, a revision) through one
//! clause of the contract, so every input the contract names an edge for is asked of every
//! implementation, and the seeded comparison (crates/amphora/tests/records_conformance.rs) asks
//! the rest. The case's id names the table and the row (`AMPHORA-RECORDS-KEY-7`); [`cases`] lists
//! them, a row each.

use std::time::Instant;

use super::{Case, F, Failure, ensure, fine, keys, put, refused, same, tx};
use crate::{
    Change, Cond, Consistency, KEY_MAX, Records, Refusal, Rev, Row, SCAN_MAX, Scan, TX_MAX, Tx,
    VALUE_MAX, Write,
};

/// A key's bytes, by row: the bytes at each end, a key that is another's prefix, the longest.
pub(super) fn key_of(n: usize) -> Vec<u8> {
    match n {
        0 => vec![0],
        1 => vec![0xff],
        2 => vec![0, 0],
        3 => b"a".to_vec(),
        4 => b"a\0".to_vec(),
        5 => b"a\xff".to_vec(),
        6 => vec![b'k'; KEY_MAX],
        7 => vec![0xff; KEY_MAX],
        8 => "\u{e9}".as_bytes().to_vec(),
        9 => b"/".to_vec(),
        10 => b"a/b/c".to_vec(),
        11 => (1..=255u8).collect(),
        12 => vec![0x7f],
        13 => vec![0x80],
        14 => b" ".to_vec(),
        15 => b"%".to_vec(),
        16 => vec![0, 0xff],
        17 => vec![0xff, 0],
        18 => b"goal/0d717247903180688a9a38422b14d923".to_vec(),
        _ => u64::MAX.to_be_bytes().to_vec(),
    }
}

/// A value's bytes, by row: empty, one byte at each end, the largest, every byte.
pub(super) fn value_of(n: usize) -> Vec<u8> {
    match n {
        0 => Vec::new(),
        1 => vec![0],
        2 => vec![0xff],
        3 => vec![b'v'; VALUE_MAX],
        4 => (0..=255u8).collect(),
        5 => "\u{e9}t\u{e9}".as_bytes().to_vec(),
        6 => vec![0; 4096],
        _ => vec![0xff; VALUE_MAX.saturating_sub(1)],
    }
}

/// A family name the contract refuses, by row.
pub(super) fn bad_family_of(n: usize) -> String {
    match n {
        0 => String::new(),
        1 => "a".repeat(crate::FAMILY_MAX.saturating_add(1)),
        2 => "A".into(),
        3 => "a b".into(),
        4 => "a/b".into(),
        5 => "\u{e9}".into(),
        6 => "a\0".into(),
        7 => "a:b".into(),
        _ => "a$".into(),
    }
}

/// A family name the contract takes, by row: the longest, one byte of each kind, one sharing the
/// suite's family as its prefix.
pub(super) fn good_family_of(n: usize) -> String {
    match n {
        0 => "a".repeat(crate::FAMILY_MAX),
        1 => "a".into(),
        2 => "0".into(),
        3 => "_".into(),
        4 => "-".into(),
        5 => ".".into(),
        6 => "a.b-c_d9".into(),
        _ => format!("{F}.x"),
    }
}

/// A key the contract refuses, by row.
fn bad_key_of(n: usize) -> Vec<u8> {
    match n {
        0 => Vec::new(),
        _ => vec![b'k'; KEY_MAX.saturating_add(1)],
    }
}

fn put_in(family: &str, key: &[u8], value: &[u8]) -> Write {
    Write::Put {
        family: family.into(),
        key: key.to_vec(),
        value: value.to_vec(),
    }
}

fn delete_in(family: &str, key: &[u8]) -> Write {
    Write::Delete {
        family: family.into(),
        key: key.to_vec(),
    }
}

fn get(
    r: &dyn Records,
    family: &str,
    key: &[u8],
    at: Consistency,
    until: Instant,
) -> Result<Option<Row>, Failure> {
    fine("get", r.get(family, key, at, until))
}

fn all_changes(
    r: &dyn Records,
    family: &str,
    after: Rev,
    until: Instant,
) -> Result<Vec<Change>, Failure> {
    Ok(fine("changes", r.changes(family, after, SCAN_MAX, until))?.changes)
}

/// A refusal of the kinds `ok` names, else a failure naming what it was.
fn refused_as<T: std::fmt::Debug>(
    what: &str,
    v: Result<T, Refusal>,
    ok: fn(&Refusal) -> bool,
) -> Result<Refusal, Failure> {
    let e = refused(what, v)?;
    ensure(ok(&e), || format!("{what}: refused as {e:?}"))?;
    Ok(e)
}

fn invalid_or_large(e: &Refusal) -> bool {
    matches!(e, Refusal::Invalid { .. } | Refusal::TooLarge { .. })
}

/// KEY: a key of these bytes is read back by get, found first under its own prefix (before a key
/// it is a prefix of), carried by the feed, and gone after its delete.
pub(super) fn key_round_trip<const K: usize>(
    r: &dyn Records,
    until: Instant,
) -> Result<(), Failure> {
    let key = key_of(K);
    let from = fine("head", r.head(until))?;
    let mut longer = key.clone();
    longer.push(0);
    let mut writes = vec![put_in(F, &key, b"v")];
    if longer.len() <= KEY_MAX {
        writes.push(put_in(F, &longer, b"w"));
    }
    let rev = fine("commit", r.commit(tx(writes.clone()), until))?;
    same(
        "get",
        get(r, F, &key, Consistency::Linearizable, until)?,
        Some(Row {
            key: key.clone(),
            value: b"v".to_vec(),
            rev,
        }),
    )?;
    let page = fine(
        "scan under the key",
        r.scan(
            F,
            Scan {
                prefix: key.clone(),
                after: None,
                limit: 2,
                at: Consistency::Linearizable,
            },
            until,
        ),
    )?;
    same(
        "scan under the key",
        page.rows.iter().map(|x| x.key.clone()).collect::<Vec<_>>(),
        writes
            .iter()
            .map(|w| match w {
                Write::Put { key, .. } | Write::Delete { key, .. } => key.clone(),
            })
            .collect::<Vec<_>>(),
    )?;
    let fed = all_changes(r, F, from, until)?;
    same(
        "the feed's first change",
        fed.first().cloned(),
        Some(Change {
            rev,
            key: key.clone(),
            value: Some(b"v".to_vec()),
        }),
    )?;
    let gone = fine("delete", r.commit(tx(vec![delete_in(F, &key)]), until))?;
    same(
        "get after the delete",
        get(r, F, &key, Consistency::Linearizable, until)?,
        None,
    )?;
    same(
        "the feed after the delete",
        all_changes(r, F, rev, until)?,
        vec![Change {
            rev: gone,
            key,
            value: None,
        }],
    )
}

/// VALUE: a value of these bytes is read back whole by get, by a scan, by a snapshot after it is
/// overwritten, and by the feed.
pub(super) fn value_round_trip<const V: usize>(
    r: &dyn Records,
    until: Instant,
) -> Result<(), Failure> {
    let value = value_of(V);
    let from = fine("head", r.head(until))?;
    let rev = fine("commit", r.commit(tx(vec![put_in(F, b"k", &value)]), until))?;
    same(
        "get",
        get(r, F, b"k", Consistency::Linearizable, until)?.map(|x| x.value),
        Some(value.clone()),
    )?;
    let page = fine(
        "scan",
        r.scan(
            F,
            Scan {
                prefix: Vec::new(),
                after: None,
                limit: 1,
                at: Consistency::Linearizable,
            },
            until,
        ),
    )?;
    same(
        "scan",
        page.rows.first().map(|x| x.value.clone()),
        Some(value.clone()),
    )?;
    fine("overwrite", r.commit(tx(vec![put("k", "x")]), until))?;
    same(
        "a snapshot from before the overwrite",
        get(r, F, b"k", Consistency::Snapshot(rev), until)?.map(|x| x.value),
        Some(value.clone()),
    )?;
    same(
        "the feed",
        fine("changes", r.changes(F, from, 1, until))?.changes,
        vec![Change {
            rev,
            key: b"k".to_vec(),
            value: Some(value),
        }],
    )
}

/// BAD-FAMILY: each request naming a family the contract refuses is refused, typed, before it
/// waits or writes: the head unmoved and the rows as they were.
pub(super) fn bad_family<const N: usize, const OP: usize>(
    r: &dyn Records,
    until: Instant,
) -> Result<(), Failure> {
    let bad = bad_family_of(N);
    let head = fine("a row first", r.commit(tx(vec![put("k", "v")]), until))?;
    let good = put("ok", "v");
    let scan = Scan {
        prefix: Vec::new(),
        after: None,
        limit: 1,
        at: Consistency::Linearizable,
    };
    let what = [
        "get",
        "scan",
        "a put",
        "a delete",
        "an absence condition",
        "a revision condition",
        "changes",
        "wait",
        "changed",
    ];
    let name = what.get(OP).copied().unwrap_or("?");
    match OP {
        0 => refused_as(
            name,
            r.get(&bad, b"k", Consistency::Linearizable, until),
            invalid_or_large,
        )
        .map(drop)?,
        1 => refused_as(name, r.scan(&bad, scan, until), invalid_or_large).map(drop)?,
        2 => refused_as(
            name,
            r.commit(tx(vec![good, put_in(&bad, b"k", b"v")]), until),
            invalid_or_large,
        )
        .map(drop)?,
        3 => refused_as(
            name,
            r.commit(tx(vec![good, delete_in(&bad, b"k")]), until),
            invalid_or_large,
        )
        .map(drop)?,
        4 => refused_as(
            name,
            r.commit(
                Tx {
                    conds: vec![Cond::Absent {
                        family: bad,
                        key: b"k".to_vec(),
                    }],
                    writes: vec![good],
                },
                until,
            ),
            invalid_or_large,
        )
        .map(drop)?,
        5 => refused_as(
            name,
            r.commit(
                Tx {
                    conds: vec![Cond::At {
                        family: bad,
                        key: b"k".to_vec(),
                        rev: head,
                    }],
                    writes: vec![good],
                },
                until,
            ),
            invalid_or_large,
        )
        .map(drop)?,
        6 => refused_as(name, r.changes(&bad, 0, 10, until), invalid_or_large).map(drop)?,
        7 => refused_as(name, r.wait(&bad, head, until), invalid_or_large).map(drop)?,
        _ => refused_as(name, r.changed(&bad, 0, until), invalid_or_large).map(drop)?,
    }
    same("the head after", fine("head", r.head(until))?, head)?;
    same(
        "a write beside it",
        get(r, F, b"ok", Consistency::Linearizable, until)?,
        None,
    )
}

/// GOOD-FAMILY: a family of this name keeps its own rows and feed, apart from the suite's.
pub(super) fn good_family<const N: usize>(r: &dyn Records, until: Instant) -> Result<(), Failure> {
    let g = good_family_of(N);
    let from = fine("head", r.head(until))?;
    let rev = fine(
        "commit",
        r.commit(
            tx(vec![
                put_in(&g, b"k", b"g"),
                put_in(F, b"k", b"f"),
                put_in(F, b"only", b"f"),
            ]),
            until,
        ),
    )?;
    same(
        "its row",
        get(r, &g, b"k", Consistency::Linearizable, until)?.map(|x| x.value),
        Some(b"g".to_vec()),
    )?;
    same(
        "the suite's row",
        get(r, F, b"k", Consistency::Linearizable, until)?.map(|x| x.value),
        Some(b"f".to_vec()),
    )?;
    let page = fine(
        "its scan",
        r.scan(
            &g,
            Scan {
                prefix: Vec::new(),
                after: None,
                limit: 10,
                at: Consistency::Linearizable,
            },
            until,
        ),
    )?;
    same("its scan", keys(&page), vec!["k".to_string()])?;
    same(
        "its feed",
        all_changes(r, &g, from, until)?,
        vec![Change {
            rev,
            key: b"k".to_vec(),
            value: Some(b"g".to_vec()),
        }],
    )
}

/// BAD-KEY: a key the contract refuses (empty, or past KEY_MAX) is refused, typed, in a get, a
/// write and a condition, and nothing is written.
pub(super) fn bad_key<const N: usize, const OP: usize>(
    r: &dyn Records,
    until: Instant,
) -> Result<(), Failure> {
    let bad = bad_key_of(N);
    let head = fine("a row first", r.commit(tx(vec![put("k", "v")]), until))?;
    let good = put("ok", "v");
    match OP {
        0 => refused_as(
            "get",
            r.get(F, &bad, Consistency::Linearizable, until),
            invalid_or_large,
        )
        .map(drop)?,
        1 => refused_as(
            "a put",
            r.commit(tx(vec![good, put_in(F, &bad, b"v")]), until),
            invalid_or_large,
        )
        .map(drop)?,
        2 => refused_as(
            "a delete",
            r.commit(tx(vec![good, delete_in(F, &bad)]), until),
            invalid_or_large,
        )
        .map(drop)?,
        3 => refused_as(
            "an absence condition",
            r.commit(
                Tx {
                    conds: vec![Cond::Absent {
                        family: F.into(),
                        key: bad,
                    }],
                    writes: vec![good],
                },
                until,
            ),
            invalid_or_large,
        )
        .map(drop)?,
        _ => refused_as(
            "a revision condition",
            r.commit(
                Tx {
                    conds: vec![Cond::At {
                        family: F.into(),
                        key: bad,
                        rev: head,
                    }],
                    writes: vec![good],
                },
                until,
            ),
            invalid_or_large,
        )
        .map(drop)?,
    }
    same("the head after", fine("head", r.head(until))?, head)?;
    same(
        "a write beside it",
        get(r, F, b"ok", Consistency::Linearizable, until)?,
        None,
    )
}

/// Walks a scan page by page, `limit` at a time, from `at`: the keys, and whether every page but
/// the last said more follow and the last said none.
fn walk(
    r: &dyn Records,
    prefix: &[u8],
    limit: usize,
    at: Consistency,
    until: Instant,
) -> Result<Vec<(String, Vec<u8>)>, Failure> {
    let mut out = Vec::new();
    let mut after = None;
    for _ in 0..=64 {
        let page = fine(
            "a page",
            r.scan(
                F,
                Scan {
                    prefix: prefix.to_vec(),
                    after: after.clone(),
                    limit,
                    at,
                },
                until,
            ),
        )?;
        ensure(page.rows.len() <= limit, || {
            format!("a page of {} rows at a limit of {limit}", page.rows.len())
        })?;
        out.extend(page.rows.iter().map(|x| {
            (
                String::from_utf8_lossy(&x.key).into_owned(),
                x.value.clone(),
            )
        }));
        let rest = fine(
            "the rest",
            r.scan(
                F,
                Scan {
                    prefix: prefix.to_vec(),
                    after: page.rows.last().map(|x| x.key.clone()).or(after.clone()),
                    limit,
                    at,
                },
                until,
            ),
        )?;
        same("whether more follow", page.more, !rest.rows.is_empty())?;
        if !page.more {
            return Ok(out);
        }
        after = page.rows.last().map(|x| x.key.clone());
    }
    Err("the walk didn't end in 64 pages".into())
}

/// SCAN-PAGE: walked a page of this size at a time, a scan gives every row under its prefix once,
/// in key order, each page saying truly whether more follow.
pub(super) fn scan_pages<const L: usize>(r: &dyn Records, until: Instant) -> Result<(), Failure> {
    let mut writes: Vec<Write> = (0..10).map(|i| put(&format!("p{i:02}"), "v")).collect();
    writes.push(put("q0", "v"));
    writes.push(put("o9", "v"));
    fine("commit", r.commit(tx(writes), until))?;
    let got = walk(r, b"p", L, Consistency::Linearizable, until)?;
    same(
        "the keys",
        got.into_iter().map(|(k, _)| k).collect::<Vec<_>>(),
        (0..10).map(|i| format!("p{i:02}")).collect::<Vec<_>>(),
    )
}

/// SNAPSHOT-PAGE: walked at a snapshot, a page of this size at a time, a scan gives the rows as
/// they were then, whatever was written, deleted or overwritten since.
pub(super) fn snapshot_pages<const L: usize>(
    r: &dyn Records,
    until: Instant,
) -> Result<(), Failure> {
    let at = fine(
        "commit",
        r.commit(
            tx((0..6).map(|i| put(&format!("s{i}"), "1")).collect()),
            until,
        ),
    )?;
    fine(
        "later",
        r.commit(
            tx(vec![
                Write::Delete {
                    family: F.into(),
                    key: b"s1".to_vec(),
                },
                Write::Delete {
                    family: F.into(),
                    key: b"s4".to_vec(),
                },
                put("s2", "2"),
                put("s10", "2"),
                put("s6", "2"),
            ]),
            until,
        ),
    )?;
    let got = walk(r, b"s", L, Consistency::Snapshot(at), until)?;
    same(
        "the rows then",
        got,
        (0..6)
            .map(|i| (format!("s{i}"), b"1".to_vec()))
            .collect::<Vec<_>>(),
    )
}

/// FEED-PAGE: read a page of this size at a time, the feed gives every change once, in commit
/// order, each page ending between commits.
pub(super) fn feed_pages<const L: usize>(r: &dyn Records, until: Instant) -> Result<(), Failure> {
    let from = fine("head", r.head(until))?;
    let mut whole = Vec::new();
    for (c, n) in [1usize, 2, 1, 3, 1].into_iter().enumerate() {
        let writes: Vec<Write> = (0..n).map(|i| put(&format!("c{c}w{i}"), "v")).collect();
        let rev = fine("commit", r.commit(tx(writes), until))?;
        whole.extend((0..n).map(|i| Change {
            rev,
            key: format!("c{c}w{i}").into_bytes(),
            value: Some(b"v".to_vec()),
        }));
    }
    let mut got: Vec<Change> = Vec::new();
    let mut after = from;
    for _ in 0..=whole.len() {
        let page = fine("a page", r.changes(F, after, L, until))?.changes;
        ensure(page.len() <= L, || {
            format!("a page of {} changes at a limit of {L}", page.len())
        })?;
        let Some(last) = page.last().map(|c| c.rev) else {
            break;
        };
        ensure(
            got.last()
                .is_none_or(|c| c.rev < page.first().map_or(last, |c| c.rev)),
            || "a page began inside the commit the page before ended in".into(),
        )?;
        got.extend(page);
        after = last;
    }
    same("the changes", got, whole)
}

/// LIMIT-1: a scan's limit past SCAN_MAX is served as SCAN_MAX.
pub(super) fn scan_limit_capped(r: &dyn Records, until: Instant) -> Result<(), Failure> {
    let rows = SCAN_MAX.saturating_add(1);
    let first: Vec<Write> = (0..TX_MAX.min(rows))
        .map(|i| put(&format!("{i:06}"), ""))
        .collect();
    fine("commit", r.commit(tx(first), until))?;
    let rest: Vec<Write> = (TX_MAX.min(rows)..rows)
        .map(|i| put(&format!("{i:06}"), ""))
        .collect();
    if !rest.is_empty() {
        fine("commit", r.commit(tx(rest), until))?;
    }
    let page = fine(
        "scan",
        r.scan(
            F,
            Scan {
                prefix: Vec::new(),
                after: None,
                limit: usize::MAX,
                at: Consistency::Linearizable,
            },
            until,
        ),
    )?;
    same(
        "rows and more",
        (page.rows.len(), page.more),
        (SCAN_MAX, true),
    )
}

/// LIMIT-2: a read of the feed with a limit of 0 is refused Invalid.
pub(super) fn changes_limit_zero(r: &dyn Records, until: Instant) -> Result<(), Failure> {
    fine("commit", r.commit(tx(vec![put("k", "v")]), until))?;
    refused_as("changes, limit 0", r.changes(F, 0, 0, until), |e| {
        matches!(e, Refusal::Invalid { .. })
    })
    .map(drop)
}

/// LIMIT-3: a read of the feed with a limit past SCAN_MAX is served as SCAN_MAX, whole commits.
pub(super) fn changes_limit_capped(r: &dyn Records, until: Instant) -> Result<(), Failure> {
    let from = fine("head", r.head(until))?;
    let big = fine(
        "commit",
        r.commit(
            tx((0..SCAN_MAX.min(TX_MAX))
                .map(|i| put(&format!("{i:06}"), ""))
                .collect()),
            until,
        ),
    )?;
    fine("commit", r.commit(tx(vec![put("z", "")]), until))?;
    let page = fine("changes", r.changes(F, from, usize::MAX, until))?;
    same(
        "the page",
        (page.changes.len(), page.changes.last().map(|c| c.rev)),
        (SCAN_MAX.min(TX_MAX), Some(big)),
    )
}

/// TX: a transaction at and past each bound, and writes that meet in one commit.
pub(super) fn transactions<const T: usize>(r: &dyn Records, until: Instant) -> Result<(), Failure> {
    let head = fine("a row first", r.commit(tx(vec![put("k", "v")]), until))?;
    let many = |n: usize| -> Vec<Write> { (0..n).map(|i| put(&format!("w{i:05}"), "v")).collect() };
    let absent = |n: usize| -> Vec<Cond> {
        (0..n)
            .map(|i| Cond::Absent {
                family: F.into(),
                key: format!("c{i:05}").into_bytes(),
            })
            .collect()
    };
    let large = |e: &Refusal| matches!(e, Refusal::TooLarge { .. });
    let unmoved = |r: &dyn Records| -> Result<(), Failure> {
        same("the head after", fine("head", r.head(until))?, head)
    };
    match T {
        0 => {
            let rev = fine("TX_MAX writes", r.commit(tx(many(TX_MAX)), until))?;
            let page = fine(
                "scan",
                r.scan(
                    F,
                    Scan {
                        prefix: b"w".to_vec(),
                        after: None,
                        limit: SCAN_MAX,
                        at: Consistency::Linearizable,
                    },
                    until,
                ),
            )?;
            ensure(page.rows.iter().all(|x| x.rev == rev), || {
                "TX_MAX writes at more than one revision".into()
            })?;
            same("rows", page.rows.len(), TX_MAX.min(SCAN_MAX))
        }
        1 => {
            refused_as(
                "TX_MAX + 1 writes",
                r.commit(tx(many(TX_MAX.saturating_add(1))), until),
                large,
            )?;
            unmoved(r)
        }
        2 => fine(
            "TX_MAX conditions",
            r.commit(
                Tx {
                    conds: absent(TX_MAX),
                    writes: many(1),
                },
                until,
            ),
        )
        .map(drop),
        3 => {
            refused_as(
                "TX_MAX + 1 conditions",
                r.commit(
                    Tx {
                        conds: absent(TX_MAX.saturating_add(1)),
                        writes: many(1),
                    },
                    until,
                ),
                large,
            )?;
            unmoved(r)
        }
        4 => {
            refused_as(
                "a value past VALUE_MAX",
                r.commit(
                    tx(vec![
                        put("ok", "v"),
                        put_in(F, b"big", &vec![0; VALUE_MAX.saturating_add(1)]),
                    ]),
                    until,
                ),
                large,
            )?;
            unmoved(r)?;
            same(
                "a write beside it",
                get(r, F, b"ok", Consistency::Linearizable, until)?,
                None,
            )
        }
        5 => {
            let rev = fine(
                "a put then a delete",
                r.commit(tx(vec![put("pd", "v"), delete_in(F, b"pd")]), until),
            )?;
            same(
                "the row",
                get(r, F, b"pd", Consistency::Linearizable, until)?,
                None,
            )?;
            same(
                "the feed",
                all_changes(r, F, head, until)?,
                vec![Change {
                    rev,
                    key: b"pd".to_vec(),
                    value: None,
                }],
            )
        }
        6 => {
            let rev = fine(
                "a delete then a put",
                r.commit(tx(vec![delete_in(F, b"k"), put("k", "w")]), until),
            )?;
            same(
                "the row",
                get(r, F, b"k", Consistency::Linearizable, until)?.map(|x| (x.value, x.rev)),
                Some((b"w".to_vec(), rev)),
            )?;
            same(
                "the feed",
                all_changes(r, F, head, until)?,
                vec![Change {
                    rev,
                    key: b"k".to_vec(),
                    value: Some(b"w".to_vec()),
                }],
            )
        }
        _ => {
            let other = "conformance-other";
            let rev = fine(
                "writes in two families",
                r.commit(tx(vec![put("a", "1"), put_in(other, b"a", b"2")]), until),
            )?;
            same(
                "this family's feed",
                all_changes(r, F, head, until)?,
                vec![Change {
                    rev,
                    key: b"a".to_vec(),
                    value: Some(b"1".to_vec()),
                }],
            )?;
            same(
                "the other's feed",
                all_changes(r, other, head, until)?,
                vec![Change {
                    rev,
                    key: b"a".to_vec(),
                    value: Some(b"2".to_vec()),
                }],
            )
        }
    }
}

/// COND: conditions on rows absent, deleted, elsewhere, and judged before the writes.
pub(super) fn conditions<const C: usize>(r: &dyn Records, until: Instant) -> Result<(), Failure> {
    let at = |key: &str, rev: Rev| Cond::At {
        family: F.into(),
        key: key.as_bytes().to_vec(),
        rev,
    };
    let absent = |key: &str| Cond::Absent {
        family: F.into(),
        key: key.as_bytes().to_vec(),
    };
    let conflict =
        |what: &str, v: Result<Rev, Refusal>, key: &str, now: Option<Rev>| -> Result<(), Failure> {
            let e = refused(what, v)?;
            same(
                what,
                e,
                Refusal::Conflict {
                    family: F.into(),
                    key: key.as_bytes().to_vec(),
                    at: now,
                },
            )
        };
    let r1 = fine(
        "a and b",
        r.commit(tx(vec![put("a", "1"), put("b", "1")]), until),
    )?;
    let head = |r: &dyn Records| fine("head", r.head(until));
    match C {
        0 => conflict(
            "at revision 0, a row absent",
            r.commit(
                Tx {
                    conds: vec![at("x", 0)],
                    writes: vec![put("x", "1")],
                },
                until,
            ),
            "x",
            None,
        ),
        1 => {
            fine("delete a", r.commit(tx(vec![delete_in(F, b"a")]), until))?;
            fine(
                "absent, a deleted row",
                r.commit(
                    Tx {
                        conds: vec![absent("a")],
                        writes: vec![put("a", "2")],
                    },
                    until,
                ),
            )
            .map(drop)
        }
        2 => {
            fine("delete a", r.commit(tx(vec![delete_in(F, b"a")]), until))?;
            conflict(
                "at a deleted row's last revision",
                r.commit(
                    Tx {
                        conds: vec![at("a", r1)],
                        writes: vec![put("a", "2")],
                    },
                    until,
                ),
                "a",
                None,
            )
        }
        3 => conflict(
            "at a revision past the head",
            r.commit(
                Tx {
                    conds: vec![at("a", r1.saturating_add(5))],
                    writes: vec![put("a", "2")],
                },
                until,
            ),
            "a",
            Some(r1),
        ),
        4 => {
            let other = "conformance-other";
            let rev = fine(
                "a condition here, a write there",
                r.commit(
                    Tx {
                        conds: vec![at("a", r1)],
                        writes: vec![put_in(other, b"a", b"2")],
                    },
                    until,
                ),
            )?;
            same(
                "the write",
                get(r, other, b"a", Consistency::Linearizable, until)?.map(|x| x.rev),
                Some(rev),
            )
        }
        5..=7 => {
            let mut conds = vec![at("a", r1), at("b", r1), absent("c")];
            let failing = ["a", "b", "c"]
                .get(C.saturating_sub(5))
                .copied()
                .unwrap_or("a");
            if let Some(slot) = conds.get_mut(C.saturating_sub(5)) {
                *slot = if failing == "c" {
                    at("c", r1)
                } else {
                    absent(failing)
                };
            }
            conflict(
                "the failing condition named",
                r.commit(
                    Tx {
                        conds,
                        writes: vec![put("d", "1")],
                    },
                    until,
                ),
                failing,
                if failing == "c" { None } else { Some(r1) },
            )?;
            same(
                "nothing written",
                get(r, F, b"d", Consistency::Linearizable, until)?,
                None,
            )
        }
        8 => same(
            "conditions alone, holding",
            fine(
                "commit",
                r.commit(
                    Tx {
                        conds: vec![at("a", r1), absent("x")],
                        writes: Vec::new(),
                    },
                    until,
                ),
            )?,
            head(r)?,
        ),
        9 => conflict(
            "conditions alone, failing",
            r.commit(
                Tx {
                    conds: vec![absent("a")],
                    writes: Vec::new(),
                },
                until,
            ),
            "a",
            Some(r1),
        ),
        10 => {
            let other = "conformance-other";
            fine(
                "absent here, present there",
                r.commit(
                    Tx {
                        conds: vec![Cond::Absent {
                            family: other.into(),
                            key: b"a".to_vec(),
                        }],
                        writes: vec![put_in(other, b"a", b"1")],
                    },
                    until,
                ),
            )
            .map(drop)
        }
        11 => {
            fine(
                "b at its commit, which wrote a too",
                r.commit(
                    Tx {
                        conds: vec![at("b", r1)],
                        writes: vec![put("b", "2")],
                    },
                    until,
                ),
            )?;
            conflict(
                "b at the old revision, a still at it",
                r.commit(
                    Tx {
                        conds: vec![at("b", r1), at("a", r1)],
                        writes: vec![put("e", "1")],
                    },
                    until,
                ),
                "b",
                head(r).ok(),
            )
        }
        _ => {
            fine(
                "absent x, writing x",
                r.commit(
                    Tx {
                        conds: vec![absent("x")],
                        writes: vec![put("x", "1")],
                    },
                    until,
                ),
            )?;
            let x = get(r, F, b"x", Consistency::Linearizable, until)?.map(|x| x.rev);
            conflict(
                "the same again",
                r.commit(
                    Tx {
                        conds: vec![absent("x")],
                        writes: vec![put("x", "2")],
                    },
                    until,
                ),
                "x",
                x,
            )
        }
    }
}

/// SNAPSHOT: at each revision of a history, get and scan see the rows as they were just after it.
pub(super) fn snapshots<const S: usize>(r: &dyn Records, until: Instant) -> Result<(), Failure> {
    let base = fine("head", r.head(until))?;
    let steps: [Vec<Write>; 5] = [
        vec![put("a", "1")],
        vec![put("b", "1")],
        vec![put("a", "2")],
        vec![delete_in(F, b"b")],
        vec![put("c", "1"), put("a", "3")],
    ];
    let then: [&[(&str, &str)]; 6] = [
        &[],
        &[("a", "1")],
        &[("a", "1"), ("b", "1")],
        &[("a", "2"), ("b", "1")],
        &[("a", "2")],
        &[("a", "3"), ("c", "1")],
    ];
    let mut revs = vec![base];
    for w in steps {
        revs.push(fine("commit", r.commit(tx(w), until))?);
    }
    let rev = revs.get(S).copied().ok_or("no such step")?;
    let want = then.get(S).copied().ok_or("no such step")?;
    let at = Consistency::Snapshot(rev);
    let page = fine(
        "scan",
        r.scan(
            F,
            Scan {
                prefix: Vec::new(),
                after: None,
                limit: 10,
                at,
            },
            until,
        ),
    )?;
    same(
        "the rows then",
        page.rows
            .iter()
            .map(|x| {
                (
                    String::from_utf8_lossy(&x.key).into_owned(),
                    String::from_utf8_lossy(&x.value).into_owned(),
                )
            })
            .collect::<Vec<_>>(),
        want.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<Vec<_>>(),
    )?;
    for k in ["a", "b", "c"] {
        let row = get(r, F, k.as_bytes(), at, until)?
            .map(|x| String::from_utf8_lossy(&x.value).into_owned());
        same(
            &format!("{k} then"),
            row,
            want.iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| v.to_string()),
        )?;
    }
    Ok(())
}

/// AHEAD: a snapshot this far past the head is refused Ahead, naming it and the head, in a get
/// (`G` 0) and a scan (1).
pub(super) fn ahead<const D: u64, const G: usize>(
    r: &dyn Records,
    until: Instant,
) -> Result<(), Failure> {
    let head = fine("commit", r.commit(tx(vec![put("a", "1")]), until))?;
    let asked = head.saturating_add(D);
    let at = Consistency::Snapshot(asked);
    let e = if G == 0 {
        refused("get", r.get(F, b"a", at, until))?
    } else {
        refused(
            "scan",
            r.scan(
                F,
                Scan {
                    prefix: Vec::new(),
                    after: None,
                    limit: 1,
                    at,
                },
                until,
            ),
        )?
    };
    same("the refusal", e, Refusal::Ahead { asked, head })
}

/// WAIT: what `changed` and `wait` answer from a family's feed.
pub(super) fn waits<const W: usize>(r: &dyn Records, until: Instant) -> Result<(), Failure> {
    let first = fine("commit", r.commit(tx(vec![put("a", "1")]), until))?;
    match W {
        0 => same(
            "changed after the head",
            fine("changed", r.changed(F, first, until))?,
            None,
        ),
        1 => {
            fine(
                "another family",
                r.commit(tx(vec![put_in("conformance-other", b"a", b"1")]), until),
            )?;
            same(
                "changed after another family's change",
                fine("changed", r.changed(F, first, until))?,
                None,
            )
        }
        2 => {
            let newest = fine("commit", r.commit(tx(vec![put("b", "1")]), until))?;
            same(
                "changed: the newest",
                fine("changed", r.changed(F, 0, until))?,
                Some(newest),
            )
        }
        3 => {
            let e = refused("a wait past its deadline", r.wait(F, first, Instant::now()))?;
            same("the refusal", e, Refusal::Timeout)
        }
        _ => {
            let gone = fine("a delete", r.commit(tx(vec![delete_in(F, b"a")]), until))?;
            same(
                "wait for a delete",
                fine("wait", r.wait(F, first, until))?,
                gone,
            )
        }
    }
}

/// The tables' cases, a row each.
pub(super) fn cases() -> Vec<Case> {
    vec![
        Case {
            id: "AMPHORA-RECORDS-KEY-1",
            says: "a key of the byte 0 is read back by get, found first under its own prefix, carried by the feed, and gone after its delete",
            check: key_round_trip::<0>,
        },
        Case {
            id: "AMPHORA-RECORDS-KEY-2",
            says: "a key of the byte 0xff is read back by get, found first under its own prefix, carried by the feed, and gone after its delete",
            check: key_round_trip::<1>,
        },
        Case {
            id: "AMPHORA-RECORDS-KEY-3",
            says: "a key of two 0 bytes is read back by get, found first under its own prefix, carried by the feed, and gone after its delete",
            check: key_round_trip::<2>,
        },
        Case {
            id: "AMPHORA-RECORDS-KEY-4",
            says: "a key of a is read back by get, found first under its own prefix, carried by the feed, and gone after its delete",
            check: key_round_trip::<3>,
        },
        Case {
            id: "AMPHORA-RECORDS-KEY-5",
            says: "a key of a then 0 is read back by get, found first under its own prefix, carried by the feed, and gone after its delete",
            check: key_round_trip::<4>,
        },
        Case {
            id: "AMPHORA-RECORDS-KEY-6",
            says: "a key of a then 0xff is read back by get, found first under its own prefix, carried by the feed, and gone after its delete",
            check: key_round_trip::<5>,
        },
        Case {
            id: "AMPHORA-RECORDS-KEY-7",
            says: "a key of KEY_MAX bytes is read back by get, found first under its own prefix, carried by the feed, and gone after its delete",
            check: key_round_trip::<6>,
        },
        Case {
            id: "AMPHORA-RECORDS-KEY-8",
            says: "a key of KEY_MAX bytes of 0xff is read back by get, found first under its own prefix, carried by the feed, and gone after its delete",
            check: key_round_trip::<7>,
        },
        Case {
            id: "AMPHORA-RECORDS-KEY-9",
            says: "a key of a two-byte UTF-8 letter is read back by get, found first under its own prefix, carried by the feed, and gone after its delete",
            check: key_round_trip::<8>,
        },
        Case {
            id: "AMPHORA-RECORDS-KEY-10",
            says: "a key of / is read back by get, found first under its own prefix, carried by the feed, and gone after its delete",
            check: key_round_trip::<9>,
        },
        Case {
            id: "AMPHORA-RECORDS-KEY-11",
            says: "a key of a/b/c is read back by get, found first under its own prefix, carried by the feed, and gone after its delete",
            check: key_round_trip::<10>,
        },
        Case {
            id: "AMPHORA-RECORDS-KEY-12",
            says: "a key of every byte 1 to 255 is read back by get, found first under its own prefix, carried by the feed, and gone after its delete",
            check: key_round_trip::<11>,
        },
        Case {
            id: "AMPHORA-RECORDS-KEY-13",
            says: "a key of 0x7f is read back by get, found first under its own prefix, carried by the feed, and gone after its delete",
            check: key_round_trip::<12>,
        },
        Case {
            id: "AMPHORA-RECORDS-KEY-14",
            says: "a key of 0x80 is read back by get, found first under its own prefix, carried by the feed, and gone after its delete",
            check: key_round_trip::<13>,
        },
        Case {
            id: "AMPHORA-RECORDS-KEY-15",
            says: "a key of a space is read back by get, found first under its own prefix, carried by the feed, and gone after its delete",
            check: key_round_trip::<14>,
        },
        Case {
            id: "AMPHORA-RECORDS-KEY-16",
            says: "a key of % is read back by get, found first under its own prefix, carried by the feed, and gone after its delete",
            check: key_round_trip::<15>,
        },
        Case {
            id: "AMPHORA-RECORDS-KEY-17",
            says: "a key of 0 then 0xff is read back by get, found first under its own prefix, carried by the feed, and gone after its delete",
            check: key_round_trip::<16>,
        },
        Case {
            id: "AMPHORA-RECORDS-KEY-18",
            says: "a key of 0xff then 0 is read back by get, found first under its own prefix, carried by the feed, and gone after its delete",
            check: key_round_trip::<17>,
        },
        Case {
            id: "AMPHORA-RECORDS-KEY-19",
            says: "a key of a goal's id is read back by get, found first under its own prefix, carried by the feed, and gone after its delete",
            check: key_round_trip::<18>,
        },
        Case {
            id: "AMPHORA-RECORDS-KEY-20",
            says: "a key of u64::MAX, big-endian is read back by get, found first under its own prefix, carried by the feed, and gone after its delete",
            check: key_round_trip::<19>,
        },
        Case {
            id: "AMPHORA-RECORDS-VALUE-1",
            says: "a value empty is read back whole by get, a scan, a snapshot after it is overwritten, and the feed",
            check: value_round_trip::<0>,
        },
        Case {
            id: "AMPHORA-RECORDS-VALUE-2",
            says: "a value the byte 0 is read back whole by get, a scan, a snapshot after it is overwritten, and the feed",
            check: value_round_trip::<1>,
        },
        Case {
            id: "AMPHORA-RECORDS-VALUE-3",
            says: "a value the byte 0xff is read back whole by get, a scan, a snapshot after it is overwritten, and the feed",
            check: value_round_trip::<2>,
        },
        Case {
            id: "AMPHORA-RECORDS-VALUE-4",
            says: "a value VALUE_MAX bytes is read back whole by get, a scan, a snapshot after it is overwritten, and the feed",
            check: value_round_trip::<3>,
        },
        Case {
            id: "AMPHORA-RECORDS-VALUE-5",
            says: "a value every byte is read back whole by get, a scan, a snapshot after it is overwritten, and the feed",
            check: value_round_trip::<4>,
        },
        Case {
            id: "AMPHORA-RECORDS-VALUE-6",
            says: "a value UTF-8 is read back whole by get, a scan, a snapshot after it is overwritten, and the feed",
            check: value_round_trip::<5>,
        },
        Case {
            id: "AMPHORA-RECORDS-VALUE-7",
            says: "a value 4096 zero bytes is read back whole by get, a scan, a snapshot after it is overwritten, and the feed",
            check: value_round_trip::<6>,
        },
        Case {
            id: "AMPHORA-RECORDS-VALUE-8",
            says: "a value VALUE_MAX - 1 bytes of 0xff is read back whole by get, a scan, a snapshot after it is overwritten, and the feed",
            check: value_round_trip::<7>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-1-GET",
            says: "a get naming a family empty is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<0, 0>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-1-SCAN",
            says: "a scan naming a family empty is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<0, 1>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-1-PUT",
            says: "a put naming a family empty is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<0, 2>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-1-DELETE",
            says: "a delete naming a family empty is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<0, 3>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-1-ABSENCE-CONDITION",
            says: "a absence condition naming a family empty is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<0, 4>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-1-REVISION-CONDITION",
            says: "a revision condition naming a family empty is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<0, 5>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-1-CHANGES",
            says: "a changes naming a family empty is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<0, 6>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-1-WAIT",
            says: "a wait naming a family empty is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<0, 7>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-1-CHANGED",
            says: "a changed naming a family empty is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<0, 8>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-2-GET",
            says: "a get naming a family FAMILY_MAX + 1 bytes long is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<1, 0>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-2-SCAN",
            says: "a scan naming a family FAMILY_MAX + 1 bytes long is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<1, 1>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-2-PUT",
            says: "a put naming a family FAMILY_MAX + 1 bytes long is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<1, 2>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-2-DELETE",
            says: "a delete naming a family FAMILY_MAX + 1 bytes long is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<1, 3>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-2-ABSENCE-CONDITION",
            says: "a absence condition naming a family FAMILY_MAX + 1 bytes long is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<1, 4>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-2-REVISION-CONDITION",
            says: "a revision condition naming a family FAMILY_MAX + 1 bytes long is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<1, 5>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-2-CHANGES",
            says: "a changes naming a family FAMILY_MAX + 1 bytes long is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<1, 6>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-2-WAIT",
            says: "a wait naming a family FAMILY_MAX + 1 bytes long is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<1, 7>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-2-CHANGED",
            says: "a changed naming a family FAMILY_MAX + 1 bytes long is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<1, 8>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-3-GET",
            says: "a get naming a family with a capital letter is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<2, 0>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-3-SCAN",
            says: "a scan naming a family with a capital letter is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<2, 1>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-3-PUT",
            says: "a put naming a family with a capital letter is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<2, 2>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-3-DELETE",
            says: "a delete naming a family with a capital letter is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<2, 3>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-3-ABSENCE-CONDITION",
            says: "a absence condition naming a family with a capital letter is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<2, 4>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-3-REVISION-CONDITION",
            says: "a revision condition naming a family with a capital letter is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<2, 5>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-3-CHANGES",
            says: "a changes naming a family with a capital letter is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<2, 6>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-3-WAIT",
            says: "a wait naming a family with a capital letter is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<2, 7>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-3-CHANGED",
            says: "a changed naming a family with a capital letter is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<2, 8>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-4-GET",
            says: "a get naming a family with a space is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<3, 0>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-4-SCAN",
            says: "a scan naming a family with a space is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<3, 1>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-4-PUT",
            says: "a put naming a family with a space is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<3, 2>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-4-DELETE",
            says: "a delete naming a family with a space is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<3, 3>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-4-ABSENCE-CONDITION",
            says: "a absence condition naming a family with a space is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<3, 4>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-4-REVISION-CONDITION",
            says: "a revision condition naming a family with a space is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<3, 5>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-4-CHANGES",
            says: "a changes naming a family with a space is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<3, 6>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-4-WAIT",
            says: "a wait naming a family with a space is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<3, 7>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-4-CHANGED",
            says: "a changed naming a family with a space is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<3, 8>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-5-GET",
            says: "a get naming a family with a slash is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<4, 0>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-5-SCAN",
            says: "a scan naming a family with a slash is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<4, 1>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-5-PUT",
            says: "a put naming a family with a slash is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<4, 2>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-5-DELETE",
            says: "a delete naming a family with a slash is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<4, 3>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-5-ABSENCE-CONDITION",
            says: "a absence condition naming a family with a slash is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<4, 4>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-5-REVISION-CONDITION",
            says: "a revision condition naming a family with a slash is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<4, 5>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-5-CHANGES",
            says: "a changes naming a family with a slash is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<4, 6>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-5-WAIT",
            says: "a wait naming a family with a slash is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<4, 7>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-5-CHANGED",
            says: "a changed naming a family with a slash is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<4, 8>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-6-GET",
            says: "a get naming a family with a non-ASCII letter is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<5, 0>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-6-SCAN",
            says: "a scan naming a family with a non-ASCII letter is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<5, 1>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-6-PUT",
            says: "a put naming a family with a non-ASCII letter is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<5, 2>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-6-DELETE",
            says: "a delete naming a family with a non-ASCII letter is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<5, 3>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-6-ABSENCE-CONDITION",
            says: "a absence condition naming a family with a non-ASCII letter is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<5, 4>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-6-REVISION-CONDITION",
            says: "a revision condition naming a family with a non-ASCII letter is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<5, 5>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-6-CHANGES",
            says: "a changes naming a family with a non-ASCII letter is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<5, 6>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-6-WAIT",
            says: "a wait naming a family with a non-ASCII letter is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<5, 7>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-6-CHANGED",
            says: "a changed naming a family with a non-ASCII letter is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<5, 8>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-7-GET",
            says: "a get naming a family with a 0 byte is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<6, 0>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-7-SCAN",
            says: "a scan naming a family with a 0 byte is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<6, 1>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-7-PUT",
            says: "a put naming a family with a 0 byte is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<6, 2>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-7-DELETE",
            says: "a delete naming a family with a 0 byte is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<6, 3>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-7-ABSENCE-CONDITION",
            says: "a absence condition naming a family with a 0 byte is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<6, 4>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-7-REVISION-CONDITION",
            says: "a revision condition naming a family with a 0 byte is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<6, 5>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-7-CHANGES",
            says: "a changes naming a family with a 0 byte is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<6, 6>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-7-WAIT",
            says: "a wait naming a family with a 0 byte is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<6, 7>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-7-CHANGED",
            says: "a changed naming a family with a 0 byte is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<6, 8>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-8-GET",
            says: "a get naming a family with a colon is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<7, 0>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-8-SCAN",
            says: "a scan naming a family with a colon is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<7, 1>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-8-PUT",
            says: "a put naming a family with a colon is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<7, 2>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-8-DELETE",
            says: "a delete naming a family with a colon is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<7, 3>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-8-ABSENCE-CONDITION",
            says: "a absence condition naming a family with a colon is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<7, 4>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-8-REVISION-CONDITION",
            says: "a revision condition naming a family with a colon is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<7, 5>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-8-CHANGES",
            says: "a changes naming a family with a colon is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<7, 6>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-8-WAIT",
            says: "a wait naming a family with a colon is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<7, 7>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-8-CHANGED",
            says: "a changed naming a family with a colon is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<7, 8>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-9-GET",
            says: "a get naming a family with a dollar sign is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<8, 0>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-9-SCAN",
            says: "a scan naming a family with a dollar sign is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<8, 1>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-9-PUT",
            says: "a put naming a family with a dollar sign is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<8, 2>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-9-DELETE",
            says: "a delete naming a family with a dollar sign is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<8, 3>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-9-ABSENCE-CONDITION",
            says: "a absence condition naming a family with a dollar sign is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<8, 4>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-9-REVISION-CONDITION",
            says: "a revision condition naming a family with a dollar sign is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<8, 5>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-9-CHANGES",
            says: "a changes naming a family with a dollar sign is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<8, 6>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-9-WAIT",
            says: "a wait naming a family with a dollar sign is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<8, 7>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-FAMILY-9-CHANGED",
            says: "a changed naming a family with a dollar sign is refused, typed, before it waits or writes: the head unmoved",
            check: bad_family::<8, 8>,
        },
        Case {
            id: "AMPHORA-RECORDS-GOOD-FAMILY-1",
            says: "a family named FAMILY_MAX bytes long keeps its own rows and feed, apart from another family's",
            check: good_family::<0>,
        },
        Case {
            id: "AMPHORA-RECORDS-GOOD-FAMILY-2",
            says: "a family named a keeps its own rows and feed, apart from another family's",
            check: good_family::<1>,
        },
        Case {
            id: "AMPHORA-RECORDS-GOOD-FAMILY-3",
            says: "a family named 0 keeps its own rows and feed, apart from another family's",
            check: good_family::<2>,
        },
        Case {
            id: "AMPHORA-RECORDS-GOOD-FAMILY-4",
            says: "a family named _ keeps its own rows and feed, apart from another family's",
            check: good_family::<3>,
        },
        Case {
            id: "AMPHORA-RECORDS-GOOD-FAMILY-5",
            says: "a family named - keeps its own rows and feed, apart from another family's",
            check: good_family::<4>,
        },
        Case {
            id: "AMPHORA-RECORDS-GOOD-FAMILY-6",
            says: "a family named . keeps its own rows and feed, apart from another family's",
            check: good_family::<5>,
        },
        Case {
            id: "AMPHORA-RECORDS-GOOD-FAMILY-7",
            says: "a family named a.b-c_d9 keeps its own rows and feed, apart from another family's",
            check: good_family::<6>,
        },
        Case {
            id: "AMPHORA-RECORDS-GOOD-FAMILY-8",
            says: "a family named the suite's family then .x keeps its own rows and feed, apart from another family's",
            check: good_family::<7>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-KEY-1-GET",
            says: "a get on a key empty is refused, typed, and writes nothing",
            check: bad_key::<0, 0>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-KEY-1-PUT",
            says: "a put on a key empty is refused, typed, and writes nothing",
            check: bad_key::<0, 1>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-KEY-1-DELETE",
            says: "a delete on a key empty is refused, typed, and writes nothing",
            check: bad_key::<0, 2>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-KEY-1-ABSENCE-CONDITION",
            says: "a absence condition on a key empty is refused, typed, and writes nothing",
            check: bad_key::<0, 3>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-KEY-1-REVISION-CONDITION",
            says: "a revision condition on a key empty is refused, typed, and writes nothing",
            check: bad_key::<0, 4>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-KEY-2-GET",
            says: "a get on a key KEY_MAX + 1 bytes long is refused, typed, and writes nothing",
            check: bad_key::<1, 0>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-KEY-2-PUT",
            says: "a put on a key KEY_MAX + 1 bytes long is refused, typed, and writes nothing",
            check: bad_key::<1, 1>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-KEY-2-DELETE",
            says: "a delete on a key KEY_MAX + 1 bytes long is refused, typed, and writes nothing",
            check: bad_key::<1, 2>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-KEY-2-ABSENCE-CONDITION",
            says: "a absence condition on a key KEY_MAX + 1 bytes long is refused, typed, and writes nothing",
            check: bad_key::<1, 3>,
        },
        Case {
            id: "AMPHORA-RECORDS-BAD-KEY-2-REVISION-CONDITION",
            says: "a revision condition on a key KEY_MAX + 1 bytes long is refused, typed, and writes nothing",
            check: bad_key::<1, 4>,
        },
        Case {
            id: "AMPHORA-RECORDS-SCAN-PAGE-1",
            says: "walked 1 at a time, a scan gives every row under its prefix once, in key order, each page saying truly whether more follow",
            check: scan_pages::<1>,
        },
        Case {
            id: "AMPHORA-RECORDS-SCAN-PAGE-2",
            says: "walked 2 at a time, a scan gives every row under its prefix once, in key order, each page saying truly whether more follow",
            check: scan_pages::<2>,
        },
        Case {
            id: "AMPHORA-RECORDS-SCAN-PAGE-3",
            says: "walked 3 at a time, a scan gives every row under its prefix once, in key order, each page saying truly whether more follow",
            check: scan_pages::<3>,
        },
        Case {
            id: "AMPHORA-RECORDS-SCAN-PAGE-4",
            says: "walked 4 at a time, a scan gives every row under its prefix once, in key order, each page saying truly whether more follow",
            check: scan_pages::<4>,
        },
        Case {
            id: "AMPHORA-RECORDS-SCAN-PAGE-5",
            says: "walked 5 at a time, a scan gives every row under its prefix once, in key order, each page saying truly whether more follow",
            check: scan_pages::<5>,
        },
        Case {
            id: "AMPHORA-RECORDS-SCAN-PAGE-6",
            says: "walked 6 at a time, a scan gives every row under its prefix once, in key order, each page saying truly whether more follow",
            check: scan_pages::<6>,
        },
        Case {
            id: "AMPHORA-RECORDS-SCAN-PAGE-7",
            says: "walked 7 at a time, a scan gives every row under its prefix once, in key order, each page saying truly whether more follow",
            check: scan_pages::<7>,
        },
        Case {
            id: "AMPHORA-RECORDS-SCAN-PAGE-8",
            says: "walked 8 at a time, a scan gives every row under its prefix once, in key order, each page saying truly whether more follow",
            check: scan_pages::<8>,
        },
        Case {
            id: "AMPHORA-RECORDS-SCAN-PAGE-9",
            says: "walked 9 at a time, a scan gives every row under its prefix once, in key order, each page saying truly whether more follow",
            check: scan_pages::<9>,
        },
        Case {
            id: "AMPHORA-RECORDS-SCAN-PAGE-10",
            says: "walked 10 at a time, a scan gives every row under its prefix once, in key order, each page saying truly whether more follow",
            check: scan_pages::<10>,
        },
        Case {
            id: "AMPHORA-RECORDS-SCAN-PAGE-11",
            says: "walked 11 at a time, a scan gives every row under its prefix once, in key order, each page saying truly whether more follow",
            check: scan_pages::<11>,
        },
        Case {
            id: "AMPHORA-RECORDS-SCAN-PAGE-12",
            says: "walked 12 at a time, a scan gives every row under its prefix once, in key order, each page saying truly whether more follow",
            check: scan_pages::<12>,
        },
        Case {
            id: "AMPHORA-RECORDS-SNAPSHOT-PAGE-1",
            says: "walked 1 at a time at a snapshot, a scan gives the rows as they were then, whatever changed since",
            check: snapshot_pages::<1>,
        },
        Case {
            id: "AMPHORA-RECORDS-SNAPSHOT-PAGE-2",
            says: "walked 2 at a time at a snapshot, a scan gives the rows as they were then, whatever changed since",
            check: snapshot_pages::<2>,
        },
        Case {
            id: "AMPHORA-RECORDS-SNAPSHOT-PAGE-3",
            says: "walked 3 at a time at a snapshot, a scan gives the rows as they were then, whatever changed since",
            check: snapshot_pages::<3>,
        },
        Case {
            id: "AMPHORA-RECORDS-SNAPSHOT-PAGE-4",
            says: "walked 4 at a time at a snapshot, a scan gives the rows as they were then, whatever changed since",
            check: snapshot_pages::<4>,
        },
        Case {
            id: "AMPHORA-RECORDS-SNAPSHOT-PAGE-5",
            says: "walked 5 at a time at a snapshot, a scan gives the rows as they were then, whatever changed since",
            check: snapshot_pages::<5>,
        },
        Case {
            id: "AMPHORA-RECORDS-SNAPSHOT-PAGE-6",
            says: "walked 6 at a time at a snapshot, a scan gives the rows as they were then, whatever changed since",
            check: snapshot_pages::<6>,
        },
        Case {
            id: "AMPHORA-RECORDS-FEED-PAGE-3",
            says: "read 3 at a time, the feed gives every change once, in commit order, each page ending between commits",
            check: feed_pages::<3>,
        },
        Case {
            id: "AMPHORA-RECORDS-FEED-PAGE-4",
            says: "read 4 at a time, the feed gives every change once, in commit order, each page ending between commits",
            check: feed_pages::<4>,
        },
        Case {
            id: "AMPHORA-RECORDS-FEED-PAGE-5",
            says: "read 5 at a time, the feed gives every change once, in commit order, each page ending between commits",
            check: feed_pages::<5>,
        },
        Case {
            id: "AMPHORA-RECORDS-FEED-PAGE-6",
            says: "read 6 at a time, the feed gives every change once, in commit order, each page ending between commits",
            check: feed_pages::<6>,
        },
        Case {
            id: "AMPHORA-RECORDS-FEED-PAGE-7",
            says: "read 7 at a time, the feed gives every change once, in commit order, each page ending between commits",
            check: feed_pages::<7>,
        },
        Case {
            id: "AMPHORA-RECORDS-FEED-PAGE-8",
            says: "read 8 at a time, the feed gives every change once, in commit order, each page ending between commits",
            check: feed_pages::<8>,
        },
        Case {
            id: "AMPHORA-RECORDS-LIMIT-1",
            says: "a scan's limit past SCAN_MAX is served as SCAN_MAX",
            check: scan_limit_capped,
        },
        Case {
            id: "AMPHORA-RECORDS-LIMIT-2",
            says: "a read of the feed with a limit of 0 is refused Invalid",
            check: changes_limit_zero,
        },
        Case {
            id: "AMPHORA-RECORDS-LIMIT-3",
            says: "a read of the feed with a limit past SCAN_MAX is served as SCAN_MAX, in whole commits",
            check: changes_limit_capped,
        },
        Case {
            id: "AMPHORA-RECORDS-TX-1",
            says: "TX_MAX writes commit at one revision",
            check: transactions::<0>,
        },
        Case {
            id: "AMPHORA-RECORDS-TX-2",
            says: "TX_MAX + 1 writes are refused TooLarge, the head unmoved",
            check: transactions::<1>,
        },
        Case {
            id: "AMPHORA-RECORDS-TX-3",
            says: "TX_MAX conditions commit",
            check: transactions::<2>,
        },
        Case {
            id: "AMPHORA-RECORDS-TX-4",
            says: "TX_MAX + 1 conditions are refused TooLarge, the head unmoved",
            check: transactions::<3>,
        },
        Case {
            id: "AMPHORA-RECORDS-TX-5",
            says: "a value past VALUE_MAX is refused TooLarge and the writes beside it don't apply",
            check: transactions::<4>,
        },
        Case {
            id: "AMPHORA-RECORDS-TX-6",
            says: "a put then a delete of one key in one commit leaves it absent, one change in the feed",
            check: transactions::<5>,
        },
        Case {
            id: "AMPHORA-RECORDS-TX-7",
            says: "a delete then a put of one key in one commit leaves the put, one change in the feed",
            check: transactions::<6>,
        },
        Case {
            id: "AMPHORA-RECORDS-TX-8",
            says: "writes to two families in one commit share its revision, each in its own family's feed",
            check: transactions::<7>,
        },
        Case {
            id: "AMPHORA-RECORDS-COND-1",
            says: "a revision condition on a row never written is refused Conflict, naming none",
            check: conditions::<0>,
        },
        Case {
            id: "AMPHORA-RECORDS-COND-2",
            says: "an absence condition holds on a deleted row",
            check: conditions::<1>,
        },
        Case {
            id: "AMPHORA-RECORDS-COND-3",
            says: "a revision condition at a deleted row's last revision is refused Conflict, naming none",
            check: conditions::<2>,
        },
        Case {
            id: "AMPHORA-RECORDS-COND-4",
            says: "a revision condition past the head is refused Conflict, naming the row's revision",
            check: conditions::<3>,
        },
        Case {
            id: "AMPHORA-RECORDS-COND-5",
            says: "a condition in one family and a write in another commit together",
            check: conditions::<4>,
        },
        Case {
            id: "AMPHORA-RECORDS-COND-6",
            says: "of three conditions, a failing first is named in the Conflict and nothing is written",
            check: conditions::<5>,
        },
        Case {
            id: "AMPHORA-RECORDS-COND-7",
            says: "of three conditions, a failing second is named in the Conflict and nothing is written",
            check: conditions::<6>,
        },
        Case {
            id: "AMPHORA-RECORDS-COND-8",
            says: "of three conditions, a failing third is named in the Conflict and nothing is written",
            check: conditions::<7>,
        },
        Case {
            id: "AMPHORA-RECORDS-COND-9",
            says: "conditions alone, holding, commit nothing and return the head",
            check: conditions::<8>,
        },
        Case {
            id: "AMPHORA-RECORDS-COND-10",
            says: "conditions alone, failing, are refused Conflict",
            check: conditions::<9>,
        },
        Case {
            id: "AMPHORA-RECORDS-COND-11",
            says: "an absence condition holds in a family where the key is absent though another family has it",
            check: conditions::<10>,
        },
        Case {
            id: "AMPHORA-RECORDS-COND-12",
            says: "a revision condition fails once its row is rewritten, though another row written in the same commit is still at it",
            check: conditions::<11>,
        },
        Case {
            id: "AMPHORA-RECORDS-COND-13",
            says: "conditions are judged before the transaction's own writes",
            check: conditions::<12>,
        },
        Case {
            id: "AMPHORA-RECORDS-SNAPSHOT-0",
            says: "a snapshot after step 0 of a history of puts, overwrites and deletes sees, by get and by scan, the rows as they were then",
            check: snapshots::<0>,
        },
        Case {
            id: "AMPHORA-RECORDS-SNAPSHOT-1",
            says: "a snapshot after step 1 of a history of puts, overwrites and deletes sees, by get and by scan, the rows as they were then",
            check: snapshots::<1>,
        },
        Case {
            id: "AMPHORA-RECORDS-SNAPSHOT-2",
            says: "a snapshot after step 2 of a history of puts, overwrites and deletes sees, by get and by scan, the rows as they were then",
            check: snapshots::<2>,
        },
        Case {
            id: "AMPHORA-RECORDS-SNAPSHOT-3",
            says: "a snapshot after step 3 of a history of puts, overwrites and deletes sees, by get and by scan, the rows as they were then",
            check: snapshots::<3>,
        },
        Case {
            id: "AMPHORA-RECORDS-SNAPSHOT-4",
            says: "a snapshot after step 4 of a history of puts, overwrites and deletes sees, by get and by scan, the rows as they were then",
            check: snapshots::<4>,
        },
        Case {
            id: "AMPHORA-RECORDS-SNAPSHOT-5",
            says: "a snapshot after step 5 of a history of puts, overwrites and deletes sees, by get and by scan, the rows as they were then",
            check: snapshots::<5>,
        },
        Case {
            id: "AMPHORA-RECORDS-AHEAD-1-GET",
            says: "a get at a snapshot 1 past the head is refused Ahead, naming it and the head",
            check: ahead::<1, 0>,
        },
        Case {
            id: "AMPHORA-RECORDS-AHEAD-1-SCAN",
            says: "a scan at a snapshot 1 past the head is refused Ahead, naming it and the head",
            check: ahead::<1, 1>,
        },
        Case {
            id: "AMPHORA-RECORDS-AHEAD-2-GET",
            says: "a get at a snapshot 2 past the head is refused Ahead, naming it and the head",
            check: ahead::<2, 0>,
        },
        Case {
            id: "AMPHORA-RECORDS-AHEAD-2-SCAN",
            says: "a scan at a snapshot 2 past the head is refused Ahead, naming it and the head",
            check: ahead::<2, 1>,
        },
        Case {
            id: "AMPHORA-RECORDS-AHEAD-3-GET",
            says: "a get at a snapshot 3 past the head is refused Ahead, naming it and the head",
            check: ahead::<3, 0>,
        },
        Case {
            id: "AMPHORA-RECORDS-AHEAD-3-SCAN",
            says: "a scan at a snapshot 3 past the head is refused Ahead, naming it and the head",
            check: ahead::<3, 1>,
        },
        Case {
            id: "AMPHORA-RECORDS-WAIT-1",
            says: "changed after the head is none",
            check: waits::<0>,
        },
        Case {
            id: "AMPHORA-RECORDS-WAIT-2",
            says: "a change to another family doesn't answer changed",
            check: waits::<1>,
        },
        Case {
            id: "AMPHORA-RECORDS-WAIT-3",
            says: "changed gives the newest revision of a change after its revision",
            check: waits::<2>,
        },
        Case {
            id: "AMPHORA-RECORDS-WAIT-4",
            says: "a wait with no change by its deadline is refused Timeout",
            check: waits::<3>,
        },
        Case {
            id: "AMPHORA-RECORDS-WAIT-5",
            says: "a delete answers a wait",
            check: waits::<4>,
        },
    ]
}
