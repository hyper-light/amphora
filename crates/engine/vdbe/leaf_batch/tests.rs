//! QT.7 (query speed step 7): the batched scan against the row-at-a-time scan, switch on and off,
//! on random tables and predicates: every type, NULLs, affinities, a collation, overflow rows,
//! deleted rows, columns added after rows were written, ascending and descending scans, and
//! batch budgets down to below one row.

use std::cell::Cell;

use crate::io::MemoryIO;
use crate::sync::Arc;
use crate::{Connection, Database, SqliteDialect};

thread_local! {
    static BUDGET: Cell<Option<usize>> = const { Cell::new(None) };
    static DECODED: Cell<usize> = const { Cell::new(0) };
}

/// The batch budget a test set, instead of the measured one.
pub(super) fn budget_override() -> Option<usize> {
    BUDGET.with(Cell::get)
}

pub(super) fn note_decoded(rows: usize) {
    DECODED.with(|decoded| decoded.set(decoded.get() + rows));
}

fn decoded() -> usize {
    DECODED.with(Cell::get)
}

/// A seeded xorshift: the same tables and predicates on every run of a seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn open() -> Arc<Connection> {
    let io = Arc::new(MemoryIO::new());
    let db = Database::open_file(io, ":memory:", Arc::new(SqliteDialect)).unwrap();
    db.connect(crate::ConnectOpts::new()).unwrap()
}

/// A statement's rows, or the error it ended with (`sum` overflowing, say): both must match.
fn rows(conn: &Arc<Connection>, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    match stmt.run_collect_rows() {
        Ok(rows) => rows.into_iter().map(|row| format!("{row:?}")).collect(),
        Err(e) => vec![format!("error: {e}")],
    }
}

/// A value of any storage class, in SQL.
fn literal(rng: &mut Rng) -> String {
    match rng.below(8) {
        0 => "NULL".into(),
        1 => format!("{}", rng.next() as i64),
        2 => format!("{}", rng.below(50) as i64 - 25),
        3 => format!("{:?}", (rng.below(20_000) as f64 - 10_000.0) / 7.0),
        4 => format!(
            "'{}'",
            ["a", "B", "ab", "Ab", "10", "2.5", "x y", ""][rng.below(8) as usize]
        ),
        5 => format!("X'{:02x}{:02x}'", rng.below(256), rng.below(256)),
        // An overflow row: its record spills past the page.
        6 if rng.below(8) == 0 => format!("X'{}'", "ab".repeat(3_000 + rng.below(3_000) as usize)),
        _ => format!("{}", rng.below(100)),
    }
}

fn build(conn: &Arc<Connection>, rng: &mut Rng) {
    conn.execute("CREATE TABLE t(a INTEGER, b REAL, c TEXT COLLATE NOCASE, d BLOB, e NUMERIC, f)")
        .unwrap();
    let n = 200 + rng.below(1_500);
    for _ in 0..n {
        let values: Vec<String> = (0..6).map(|_| literal(rng)).collect();
        conn.execute(format!("INSERT INTO t VALUES ({})", values.join(",")))
            .unwrap();
    }
    conn.execute(format!(
        "DELETE FROM t WHERE rowid % {} = 0",
        3 + rng.below(5)
    ))
    .unwrap();
    // Rows written before the column was added read its default.
    conn.execute("ALTER TABLE t ADD COLUMN g TEXT DEFAULT 'dflt'")
        .unwrap();
    for _ in 0..rng.below(50) {
        let values: Vec<String> = (0..7).map(|_| literal(rng)).collect();
        conn.execute(format!("INSERT INTO t VALUES ({})", values.join(",")))
            .unwrap();
    }
}

fn predicate(rng: &mut Rng) -> String {
    let column = ["a", "b", "c", "d", "e", "f", "g"][rng.below(7) as usize];
    match rng.below(5) {
        0 => format!("{column} IS NULL"),
        1 => format!("{column} IS NOT NULL"),
        2 => format!("{column} > {}", literal(rng)),
        3 => format!("{column} = {}", literal(rng)),
        _ => format!("{column} <= {} OR a < {}", literal(rng), rng.below(60)),
    }
}

fn queries(rng: &mut Rng) -> Vec<String> {
    let mut out = vec![
        "SELECT * FROM t".to_string(),
        "SELECT rowid, * FROM t ORDER BY rowid DESC".to_string(),
        "SELECT count(*) FROM t".to_string(),
        "SELECT count(*), count(a), count(g), sum(a), total(b), min(c), max(c), min(d), max(e), avg(e), group_concat(g) FROM t".to_string(),
        "SELECT c, count(*), sum(e) FROM t GROUP BY c ORDER BY c".to_string(),
        "SELECT length(d), typeof(f), g FROM t".to_string(),
    ];
    for _ in 0..12 {
        let p = predicate(rng);
        out.push(format!("SELECT * FROM t WHERE {p}"));
        out.push(format!(
            "SELECT count(*), sum(a), min(b), max(f), total(e) FROM t WHERE {p}"
        ));
        out.push(format!("SELECT a, c FROM t WHERE {p} ORDER BY rowid DESC"));
    }
    out
}

/// One seed: every query's answer with the switch on equals its answer with it off.
fn differential(seed: u64, budget: Option<usize>) -> usize {
    BUDGET.with(|b| b.set(budget));
    DECODED.with(|d| d.set(0));
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let conn = open();
    build(&conn, &mut rng);
    for sql in queries(&mut rng) {
        conn.set_vectorized_scans(false);
        let row_at_a_time = rows(&conn, &sql);
        conn.set_vectorized_scans(true);
        let batched = rows(&conn, &sql);
        assert_eq!(
            batched, row_at_a_time,
            "seed {seed}, budget {budget:?}: {sql}"
        );
    }
    conn.set_vectorized_scans(false);
    BUDGET.with(|b| b.set(None));
    decoded()
}

/// T-QT.7.1: batched against row-at-a-time on random tables and predicates (NULLs, affinities,
/// a collation, overflow pages, deleted rows, added columns): identical results, and the batches
/// were used (non-vacuity).
#[test]
fn qt7_batched_scans_answer_as_row_at_a_time_scans() {
    // A budget of a whole page's rows and more: one window per leaf.
    for seed in 1..=6 {
        let decoded = differential(seed, Some(1 << 20));
        assert!(decoded > 0, "seed {seed}: no row was read from a batch");
    }
    // The budget as it runs: the measured L2 cache and the statement's memory budget. Where the
    // cache size can't be read the scan reads row-at-a-time, with the same answers.
    let decoded = differential(11, None);
    if cfg!(any(target_os = "linux", target_os = "macos")) {
        assert!(decoded > 0, "the measured budget batched no row");
    } else {
        assert_eq!(decoded, 0, "batched without a measured cache size");
    }
}

/// T-QT.7.2: a batch budget smaller than one page's cells: the scan decodes several windows a
/// leaf, and below one row's charge falls back to row-at-a-time, by its stated rule, with
/// identical results.
#[test]
fn qt7_budget_below_a_page_falls_back_with_identical_results() {
    for seed in 7..=10 {
        // A few rows' worth: several windows on every leaf.
        let decoded = differential(seed, Some(600));
        assert!(decoded > 0, "seed {seed}: no row was read from a batch");
        // Under one row's charge: nothing is batched, every row is read as before.
        let decoded = differential(seed, Some(1));
        assert_eq!(decoded, 0, "seed {seed}: a row was batched past its budget");
    }
}

/// T-QT.7.3: a program that writes keeps the row-at-a-time read, and a statement prepared with
/// the switch off is prepared again when it turns on.
#[test]
fn qt7_writes_read_row_at_a_time_and_the_switch_reprepares() {
    BUDGET.with(|b| b.set(Some(1 << 20)));
    let conn = open();
    conn.execute("CREATE TABLE t(a, b)").unwrap();
    for i in 0..500 {
        conn.execute(format!("INSERT INTO t VALUES ({i}, 'v{i}')"))
            .unwrap();
    }
    conn.set_vectorized_scans(true);
    DECODED.with(|d| d.set(0));
    conn.execute("INSERT INTO t SELECT a + 1000, b FROM t")
        .unwrap();
    assert_eq!(decoded(), 0, "a writing program read by batches");
    assert_eq!(
        rows(&conn, "SELECT count(*), sum(a) FROM t"),
        rows(&conn, "SELECT count(a), sum(a) FROM t")
    );
    assert!(decoded() > 0, "the read-only scan didn't use batches");
    BUDGET.with(|b| b.set(None));
}

/// The sizes sysfs writes for a cache parse to bytes; anything else is refused.
#[test]
fn qt7_cache_sizes_parse() {
    assert_eq!(super::cache::parse_size("2048K\n"), Some(2048 * 1024));
    assert_eq!(super::cache::parse_size("1M"), Some(1024 * 1024));
    assert_eq!(super::cache::parse_size("524288"), Some(524_288));
    assert_eq!(super::cache::parse_size("0K"), None);
    assert_eq!(super::cache::parse_size("big"), None);
}
