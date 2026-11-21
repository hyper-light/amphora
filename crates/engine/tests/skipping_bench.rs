//! G5 step 4's harness (AC-G5.4.1: both prototypes' scan and write costs): zone maps against
//! cracking against a plain scan, on filter_no_index-like filters with writes interleaved. The
//! coordinator runs it at the suite's size and records it in bench/; the test run here is a smoke
//! instance (it asserts only that the three agree).
//!
//!   AMPHORA_SKIP_ROWS=10000000 AMPHORA_SKIP_SCANS=200 AMPHORA_SKIP_WRITES=100 \
//!     cargo test -p amphora-engine --release --test skipping_bench -- --ignored --nocapture
//!
//! Two classes over `items`-shaped rows (bench/src/suite.rs): `qty = ?` (filter_no_index's column,
//! uniform over 0..100 and unrelated to the rowid) and `created BETWEEN ? AND ?` (a time-ordered
//! column, which follows the rowid). Each scan is preceded by AMPHORA_SKIP_WRITES writes (a third
//! appends, a third updates, a third deletes). Zone maps are built over leaf runs sized from the
//! rows' encoded bytes against the page (the SQLite file format: 4,096-byte default page, an 8-byte
//! table-leaf header, a 2-byte cell pointer, the payload and rowid varints), not a picked run size.
//!
//! Printed per class and prototype: rows (or entries) examined per scan, nanoseconds per scan and
//! per write, and the summary's size. Times are wall clock on whatever host runs it: read them as
//! the coordinator's recorded run, never as a test's assertion.

use amphora_engine::skipping::{Candidates, CrackerColumn, RowWrite, Skipping, ZoneMapBuilder};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::collections::BTreeMap;
use std::time::Instant;

/// SQLite's file format: the default page size, a table leaf's header, a cell pointer.
const PAGE_BYTES: usize = 4_096;
const LEAF_HEADER_BYTES: usize = 8;
const CELL_POINTER_BYTES: usize = 2;

fn env(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn varint_len(v: u64) -> usize {
    (1..9).find(|n| v < 1u64 << (7 * n)).unwrap_or(9)
}

fn int_bytes(v: i64) -> usize {
    match v {
        0 | 1 => 0,
        -128..=127 => 1,
        -32_768..=32_767 => 2,
        -8_388_608..=8_388_607 => 3,
        -2_147_483_648..=2_147_483_647 => 4,
        -140_737_488_355_328..=140_737_488_355_327 => 6,
        _ => 8,
    }
}

#[derive(Clone, Copy)]
struct Item {
    qty: i64,
    created: i64,
    /// The rest of an items row's encoded payload (its text, blob and other columns).
    rest: usize,
}

fn item(rng: &mut ChaCha8Rng, rowid: i64) -> Item {
    Item {
        qty: rng.random_range(0..100),
        created: rowid * 10 + rng.random_range(0..10),
        rest: rng.random_range(60..200),
    }
}

/// The bytes a row takes in a table leaf: its record (header varints and bodies) and its cell.
fn cell_bytes(rowid: i64, item: &Item) -> usize {
    let body = int_bytes(item.qty) + int_bytes(item.created) + item.rest;
    let header = 3 + 1; // header length varint and the two integers' serial types, one byte each
    let payload = header + body;
    payload + varint_len(payload as u64) + varint_len(rowid as u64) + CELL_POINTER_BYTES
}

type Table = BTreeMap<i64, Item>;

fn build(table: &Table, column: fn(&Item) -> i64, cracking: bool) -> (Skipping, usize) {
    if cracking {
        let cracker = CrackerColumn::new(table.iter().map(|(r, i)| (*r, Some(column(i)))));
        return (Skipping::Cracking(cracker), 0);
    }
    let mut builder = ZoneMapBuilder::new();
    let mut used = LEAF_HEADER_BYTES;
    for (&rowid, item) in table {
        let bytes = cell_bytes(rowid, item);
        if used + bytes > PAGE_BYTES {
            builder.end_run();
            used = LEAF_HEADER_BYTES;
        }
        used += bytes;
        builder.row(rowid, Some(column(item))).unwrap();
    }
    let zones = builder.finish();
    let count = zones.zone_count();
    (Skipping::ZoneMaps(zones), count)
}

fn write(rng: &mut ChaCha8Rng, table: &mut Table, next: &mut i64, column: fn(&Item) -> i64) -> RowWrite {
    match rng.random_range(0..3) {
        0 => {
            *next += 1;
            let it = item(rng, *next);
            table.insert(*next, it);
            RowWrite::Insert { rowid: *next, value: Some(column(&it)) }
        }
        pick => {
            let rowid = loop {
                let r = rng.random_range(1..=*next);
                if table.contains_key(&r) {
                    break r;
                }
            };
            let old = Some(column(&table[&rowid]));
            if pick == 1 {
                let mut it = item(rng, rowid);
                it.created = table[&rowid].created;
                table.insert(rowid, it);
                RowWrite::Update { rowid, old, new: Some(column(&it)) }
            } else {
                table.remove(&rowid);
                RowWrite::Delete { rowid, old }
            }
        }
    }
}

struct Class {
    name: &'static str,
    column: fn(&Item) -> i64,
    filter: fn(&mut ChaCha8Rng, i64) -> (i64, i64),
}

fn run_class(class: &Class, prototype: &str, rows: usize, scans: usize, writes: usize) {
    let mut rng = ChaCha8Rng::seed_from_u64(42);
    let mut table = Table::new();
    for rowid in 1..=rows as i64 {
        let it = item(&mut rng, rowid);
        table.insert(rowid, it);
    }
    let mut next = rows as i64;
    let started = Instant::now();
    let (mut skipping, zones) = match prototype {
        "plain" => (Skipping::Off, 0),
        "zone_maps" => build(&table, class.column, false),
        _ => build(&table, class.column, true),
    };
    let build_ns = started.elapsed().as_nanos();
    let (mut write_ns, mut scan_ns, mut examined, mut matched) = (0u128, 0u128, 0usize, 0usize);
    for _ in 0..scans {
        for _ in 0..writes {
            let w = write(&mut rng, &mut table, &mut next, class.column);
            let t = Instant::now();
            skipping.on_write(w).unwrap();
            write_ns += t.elapsed().as_nanos();
        }
        let (lo, hi) = (class.filter)(&mut rng, next);
        let hit = |i: &Item| (lo..=hi).contains(&(class.column)(i));
        let t = Instant::now();
        let (seen, found) = match skipping.candidates(lo, hi).unwrap() {
            Candidates::All => (table.len(), table.values().filter(|i| hit(i)).count()),
            Candidates::Ranges(ranges) => ranges.iter().fold((0, 0), |(s, f), r| {
                let rows = table.range(r.first..=r.last);
                let (n, m) = rows.fold((0, 0), |(n, m), (_, i)| (n + 1, m + usize::from(hit(i))));
                (s + n, f + m)
            }),
            Candidates::Rowids(rowids) => (rowids.len(), rowids.iter().filter(|r| hit(&table[*r])).count()),
        };
        scan_ns += t.elapsed().as_nanos();
        assert_eq!(found, table.values().filter(|i| hit(i)).count(), "{} {prototype}", class.name);
        examined += seen;
        matched += found;
    }
    let scans_n = scans.max(1) as u128;
    let writes_n = (scans * writes).max(1) as u128;
    println!(
        "{:<16} {:<10} rows {:>9} zones {:>7} build_ns {:>12} rows_examined/scan {:>10} matched/scan {:>9} ns/scan {:>11} ns/write {:>7}",
        class.name,
        prototype,
        rows,
        zones,
        build_ns,
        examined / scans.max(1),
        matched / scans.max(1),
        scan_ns / scans_n,
        write_ns / writes_n,
    );
}

/// Do: run each class under plain scans, zone maps and cracking with writes between scans. Expect:
/// every scan's count equals the plain scan's (the costs are printed for the record).
#[test]
#[ignore = "G5 step 4's harness: the coordinator runs it (see the module's comment)"]
fn g5_s4_skipping_harness() {
    let rows = env("AMPHORA_SKIP_ROWS", 20_000);
    let scans = env("AMPHORA_SKIP_SCANS", 20);
    let writes = env("AMPHORA_SKIP_WRITES", 10);
    let classes = [
        Class {
            name: "qty_eq",
            column: |i| i.qty,
            filter: |rng, _| {
                let q = rng.random_range(0..100);
                (q, q)
            },
        },
        Class {
            name: "created_range",
            column: |i| i.created,
            filter: |rng, next| {
                let lo = rng.random_range(0..next * 10);
                (lo, lo + next / 10)
            },
        },
    ];
    for class in &classes {
        for prototype in ["plain", "zone_maps", "cracking"] {
            run_class(class, prototype, rows, scans, writes);
        }
    }
}
