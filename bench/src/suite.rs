//! The comprehensive suite (`--suite`): every kind of SQL statement a database is judged on, each
//! timed on its own, on the same typed data in every database, so what Amphora does badly shows as
//! plainly as what it does well.
//!
//! - **Data**: `items` (`--items` rows: integers, a real, a boolean, a timestamp in µs, short and
//!   long text, a blob, a JSON document, a text column NULL in half the rows), `owners` (one for 50
//!   items) and `regions` (20), every value a function of its row's id, the same in every database.
//!   Indexes are made after the load, then each database's planner statistics, then each
//!   database's checkpoint (`settle_s`: what the load left in its log goes into its files first).
//! - **Statements** ([`OPS`]): point lookups by primary key, secondary index and text; `IN`; ranges
//!   by key and by secondary index; filters on several columns, on NULL, by `LIKE` prefix and
//!   substring, and with no index (whole scans); sorts by index and by sorting, deep `OFFSET`;
//!   `COUNT`/`SUM`/`AVG`/`MIN`/`MAX`, `GROUP BY` with `HAVING`, `DISTINCT`, `COUNT(DISTINCT)`; joins of
//!   two and three tables, with grouping, `LEFT JOIN` anti-joins, `IN` and `EXISTS` subqueries; CTEs,
//!   `UNION ALL`, `CASE`, window functions, string, date and JSON functions; reads of a blob and of
//!   long text; whole-table aggregates. Writes: one row and a hundred, updates of a plain column, an
//!   indexed one, long text, JSON and a range, an upsert, deletes by key and by range (each row put
//!   back, so the table keeps its size), and a transaction that reads and writes.
//! - **Runs**: each statement for `--seconds` after `--warmup`, at each of `--clients` (closed loop:
//!   each client runs it again as soon as it is answered; writes answered once durable). A run
//!   counts in the window if it is answered within it, whenever it began. A statement
//!   a database's dialect has no form for is recorded unsupported; one it refuses, failed, with the
//!   first error: both are findings.
//!
//! One JSON line: each statement's throughput, latency percentiles, rows and errors at each client
//! count, its longest run (`longest_ms`, warmup included: a statement that answers nothing within
//! the window says how long it took), the process's user and system CPU seconds in the window
//! (`cpu_user_s`, `cpu_sys_s`), and the memory the process held (servers' is added by `run.sh`).
//!
//! Profiling: `--only name$` runs the one statement it names, `--reuse` runs on the database a run
//! left in `--dir` instead of loading again, and AMPHORA_PERF_CTL turns perf's counting on and off
//! around each measured window (`perf_ctl`), so a profile holds that window alone.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::suite_db::{self, Target};
use crate::{summary, Hist, Rng, WORDS};

/// A statement's parameter, whatever the database: each driver binds it in its own type.
#[derive(Clone, Debug)]
pub enum P {
    I(i64),
    F(f64),
    T(String),
    B(Vec<u8>),
    Z(bool),
    N,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dialect {
    /// Amphora, Turso and SQLite.
    Sqlite,
    /// PostgreSQL, OrioleDB and CedarDB.
    Postgres,
    MySql,
    ClickHouse,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Read,
    Write,
}

/// The data's size, and the id the next inserted row takes.
pub struct Shape {
    pub items: i64,
    pub owners: i64,
    pub regions: i64,
    pub next: AtomicI64,
}

impl Shape {
    fn new(items: i64) -> Self {
        Self { items, owners: (items / 50).max(1), regions: 20, next: AtomicI64::new(items + 1) }
    }

    /// An id the load wrote.
    fn id(&self, rng: &mut Rng) -> i64 {
        1 + (rng.next() % (self.items.max(1) as u64)) as i64
    }

    /// The first id of a range of `len` ids the load wrote.
    fn range(&self, rng: &mut Rng, len: i64) -> i64 {
        1 + (rng.next() % ((self.items - len).max(1) as u64)) as i64
    }

    /// The ids client `c` of `n` writes: a slice of its own, so no two clients write a row at once
    /// (contention is its own question, not this suite's).
    fn slice(&self, c: usize, n: usize) -> (i64, i64) {
        let n = n.max(1) as i64;
        let per = (self.items / n).max(1);
        let first = 1 + per * (c as i64 % n);
        (first, first + per - 1)
    }

    /// An id of client `c`'s slice, and the first of `len` there.
    fn own(&self, rng: &mut Rng, c: (usize, usize)) -> i64 {
        let (first, last) = self.slice(c.0, c.1);
        first + (rng.next() % ((last - first + 1).max(1) as u64)) as i64
    }

    fn own_range(&self, rng: &mut Rng, c: (usize, usize), len: i64) -> i64 {
        let (first, last) = self.slice(c.0, c.1);
        first + (rng.next() % ((last - first + 1 - len).max(1) as u64)) as i64
    }
}

/// splitmix64's finalizer: a row's values from its id, the same everywhere.
pub fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

const BASE_US: i64 = 1_700_000_000_000_000;

fn k_of(id: i64) -> i64 {
    (mix(id as u64) >> 1) as i64
}

fn name_of(id: i64) -> String {
    format!("{}-{:06x}", WORDS[(mix(id as u64 ^ 0x6666) % WORDS.len() as u64) as usize], mix(id as u64 ^ 0x7777) & 0xff_ffff)
}

/// A row of `items`, its columns in `ITEM_COLUMNS`' order.
pub fn item_params(id: i64, owners: i64) -> Vec<P> {
    let m = |salt: u64| mix(id as u64 ^ salt);
    let mut body = String::with_capacity(420);
    let mut w = m(0xaaaa);
    // One row in a thousand says the rare word a substring search looks for.
    if m(0xbbbb) % 1000 == 0 {
        body.push_str("zephyr ");
    }
    while body.len() < 400 {
        w = mix(w);
        body.push_str(WORDS[(w % WORDS.len() as u64) as usize]);
        body.push(' ');
    }
    let mut data = Vec::with_capacity(64);
    let mut d = m(0xcccc);
    while data.len() < 64 {
        d = mix(d);
        data.extend_from_slice(&d.to_le_bytes());
    }
    let qty = (m(0x4444) % 100) as i64;
    let doc = format!(r#"{{"a":{},"b":"{}","c":[{},{},{}]}}"#, qty, WORDS[(m(0xdddd) % WORDS.len() as u64) as usize], m(1) % 10, m(2) % 10, m(3) % 10);
    vec![
        P::I(id),
        P::I(k_of(id)),
        P::I((m(0x1111) % 100) as i64),
        P::I((m(0x2222) % owners.max(1) as u64) as i64 + 1),
        P::F((m(0x3333) % 1_000_000) as f64 / 100.0),
        P::I(qty),
        P::Z(m(0x5555) & 1 == 1),
        P::I(BASE_US + id * 1_000_000),
        P::T(name_of(id)),
        P::T(body),
        P::B(data),
        P::T(doc),
        if m(0x8888) & 1 == 0 { P::N } else { P::T(WORDS[(m(0x9999) % WORDS.len() as u64) as usize].to_owned()) },
    ]
}

const ITEM_COLUMNS: &str = "id, k, grp, owner, price, qty, flag, created, name, body, data, doc, note";

fn owner_params(id: i64, regions: i64) -> Vec<P> {
    vec![P::I(id), P::T(format!("owner-{id}")), P::I((mix(id as u64 ^ 0x1234) % regions as u64) as i64 + 1), P::F((mix(id as u64 ^ 0x4321) % 10_000) as f64 / 100.0)]
}

fn region_params(id: i64) -> Vec<P> {
    vec![P::I(id), P::T(format!("region-{id}"))]
}

/// The tables, in `d`'s dialect.
pub fn tables(d: Dialect, cedar: bool) -> Vec<String> {
    let c = if cedar { "" } else { " COLLATE \"C\"" };
    match d {
        Dialect::Sqlite => vec![
            "CREATE TABLE regions (id INTEGER PRIMARY KEY, name TEXT NOT NULL)".into(),
            "CREATE TABLE owners (id INTEGER PRIMARY KEY, name TEXT NOT NULL, region INTEGER NOT NULL, score REAL NOT NULL)".into(),
            "CREATE TABLE items (id INTEGER PRIMARY KEY, k INTEGER NOT NULL, grp INTEGER NOT NULL, owner INTEGER NOT NULL, price REAL NOT NULL, qty INTEGER NOT NULL, flag INTEGER NOT NULL, created INTEGER NOT NULL, name TEXT NOT NULL, body TEXT NOT NULL, data BLOB NOT NULL, doc TEXT NOT NULL, note TEXT)".into(),
        ],
        Dialect::Postgres => vec![
            format!("CREATE TABLE regions (id BIGINT PRIMARY KEY, name TEXT{c} NOT NULL)"),
            format!("CREATE TABLE owners (id BIGINT PRIMARY KEY, name TEXT{c} NOT NULL, region BIGINT NOT NULL, score DOUBLE PRECISION NOT NULL)"),
            format!("CREATE TABLE items (id BIGINT PRIMARY KEY, k BIGINT NOT NULL, grp BIGINT NOT NULL, owner BIGINT NOT NULL, price DOUBLE PRECISION NOT NULL, qty BIGINT NOT NULL, flag BOOLEAN NOT NULL, created BIGINT NOT NULL, name TEXT{c} NOT NULL, body TEXT{c} NOT NULL, data BYTEA NOT NULL, doc JSONB NOT NULL, note TEXT{c})"),
        ],
        Dialect::MySql => {
            let tail = " ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_bin";
            vec![
                format!("CREATE TABLE regions (id BIGINT PRIMARY KEY, name VARCHAR(64) NOT NULL){tail}"),
                format!("CREATE TABLE owners (id BIGINT PRIMARY KEY, name VARCHAR(64) NOT NULL, region BIGINT NOT NULL, score DOUBLE NOT NULL){tail}"),
                format!("CREATE TABLE items (id BIGINT PRIMARY KEY, k BIGINT NOT NULL, grp BIGINT NOT NULL, owner BIGINT NOT NULL, price DOUBLE NOT NULL, qty BIGINT NOT NULL, flag BOOLEAN NOT NULL, created BIGINT NOT NULL, name VARCHAR(64) NOT NULL, body TEXT NOT NULL, data VARBINARY(64) NOT NULL, doc JSON NOT NULL, note VARCHAR(64)){tail}"),
            ]
        }
        Dialect::ClickHouse => vec![
            "CREATE TABLE regions (id Int64, name String) ENGINE = MergeTree ORDER BY id".into(),
            "CREATE TABLE owners (id Int64, name String, region Int64, score Float64) ENGINE = MergeTree ORDER BY id".into(),
            "CREATE TABLE items (id Int64, k Int64, grp Int64, owner Int64, price Float64, qty Int64, flag Bool, created Int64, name String, body String, data String, doc String, note Nullable(String)) ENGINE = MergeTree ORDER BY id".into(),
        ],
    }
}

/// The secondary indexes, made after the load, then the planner's statistics. ClickHouse has no
/// secondary B-trees: its tables are ordered by id alone.
pub fn indexes(d: Dialect) -> Vec<String> {
    let mut s: Vec<String> = match d {
        Dialect::ClickHouse => return vec![],
        Dialect::MySql => vec![
            "ALTER TABLE items ADD INDEX items_k (k), ADD INDEX items_name (name), ADD INDEX items_grp_created (grp, created), ADD INDEX items_owner (owner)".into(),
            "ALTER TABLE owners ADD INDEX owners_region (region)".into(),
        ],
        _ => vec![
            "CREATE INDEX items_k ON items (k)".into(),
            "CREATE INDEX items_name ON items (name)".into(),
            "CREATE INDEX items_grp_created ON items (grp, created)".into(),
            "CREATE INDEX items_owner ON items (owner)".into(),
            "CREATE INDEX owners_region ON owners (region)".into(),
        ],
    };
    s.extend(match d {
        Dialect::Sqlite => vec!["ANALYZE".to_owned()],
        Dialect::Postgres => vec!["VACUUM ANALYZE".to_owned()],
        Dialect::MySql => vec!["ANALYZE TABLE items, owners, regions".to_owned()],
        Dialect::ClickHouse => vec![],
    });
    s
}

/// A row's placeholders in `items`' column order: PostgreSQL takes the document as text, cast.
fn item_values(d: Dialect) -> &'static str {
    match d {
        Dialect::Postgres => "(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?::text::jsonb, ?)",
        _ => "(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    }
}

fn insert_items(d: Dialect, rows: usize) -> String {
    format!("INSERT INTO items ({ITEM_COLUMNS}) VALUES {}", vec![item_values(d); rows].join(", "))
}

/// The load, a batch of rows a statement and many to a transaction, each transaction handed to
/// `each` as it is made (so the harness never holds the whole load): (statement, its parameters).
pub fn load(d: Dialect, shape: &Shape, batch: usize, mut each: impl FnMut(Vec<(String, Vec<P>)>) -> anyhow::Result<()>) -> anyhow::Result<()> {
    let mut txn = Vec::new();
    let regions: Vec<P> = (1..=shape.regions).flat_map(region_params).collect();
    txn.push((format!("INSERT INTO regions (id, name) VALUES {}", vec!["(?, ?)"; shape.regions as usize].join(", ")), regions));
    let mut id = 1;
    while id <= shape.owners {
        let last = (id + batch as i64 - 1).min(shape.owners);
        let params: Vec<P> = (id..=last).flat_map(|i| owner_params(i, shape.regions)).collect();
        txn.push((format!("INSERT INTO owners (id, name, region, score) VALUES {}", vec!["(?, ?, ?, ?)"; (last - id + 1) as usize].join(", ")), params));
        id = last + 1;
    }
    each(std::mem::take(&mut txn))?;
    let mut id = 1;
    while id <= shape.items {
        let last = (id + batch as i64 - 1).min(shape.items);
        let params: Vec<P> = (id..=last).flat_map(|i| item_params(i, shape.owners)).collect();
        txn.push((insert_items(d, (last - id + 1) as usize), params));
        if txn.len() >= 50 {
            each(std::mem::take(&mut txn))?;
        }
        id = last + 1;
    }
    if !txn.is_empty() {
        each(txn)?;
    }
    Ok(())
}

/// Forgets the process's memory high-water marks (Linux's `clear_refs` 5 resets VmHWM; the heap's
/// peak starts again from what it holds now): what the runs hold, not the load the harness made.
fn forget_peaks() {
    let _ = std::fs::write("/proc/self/clear_refs", "5");
    crate::HEAP_PEAK.store(crate::HEAP.load(Ordering::Relaxed), Ordering::Relaxed);
}

/// A statement of the suite: its name, whether it writes, and what it exercises.
pub struct Op {
    pub name: &'static str,
    pub kind: Kind,
    pub what: &'static str,
}

const fn r(name: &'static str, what: &'static str) -> Op {
    Op { name, kind: Kind::Read, what }
}

const fn w(name: &'static str, what: &'static str) -> Op {
    Op { name, kind: Kind::Write, what }
}

/// Every statement of the suite, reads first.
pub const OPS: &[Op] = &[
    r("point_pk", "a row by primary key, every column"),
    r("point_pk_one_column", "one column of a row by primary key"),
    r("point_secondary", "rows by a secondary integer index"),
    r("point_text", "rows by an indexed text column"),
    r("in_list_10", "ten rows by primary key in an IN list"),
    r("range_pk_100", "a hundred rows by a primary-key range"),
    r("range_secondary_100", "about a hundred rows by a secondary-index range"),
    r("filter_multi_column", "an indexed equality, a range and a boolean, LIMIT"),
    r("filter_no_index", "COUNT(*) of a filter no index serves: a whole scan"),
    r("filter_is_null", "an indexed equality and IS NULL, LIMIT"),
    r("like_prefix", "a case-sensitive prefix match on an indexed text column: LIKE, GLOB where LIKE ignores case"),
    r("like_substring", "LIKE '%word%' on long text: a whole scan"),
    r("order_by_index", "ORDER BY an index's second column, DESC, LIMIT 20"),
    r("order_by_sort", "ORDER BY an unindexed column, LIMIT 20: a sort"),
    r("offset_deep", "LIMIT 20 OFFSET up to the table's size"),
    r("count_range", "COUNT(*) over a range of a timestamp: 10,000 rows"),
    r("aggregates_range", "SUM, AVG, MIN, MAX over 1,000 rows"),
    r("group_by_range", "GROUP BY over 10,000 rows"),
    r("group_by_having", "GROUP BY, HAVING, ORDER BY the count, LIMIT"),
    r("distinct_range", "DISTINCT over 10,000 rows"),
    r("count_distinct", "COUNT(DISTINCT) within an indexed equality"),
    r("join_pk", "20 rows joined to another table by its key"),
    r("join_group", "a join filtered on the other table, grouped and ordered"),
    r("join_three", "three tables joined over 1,000 rows, grouped"),
    r("left_join_anti", "LEFT JOIN ... IS NULL: rows with no match"),
    r("subquery_in", "IN (SELECT ...) beside an indexed equality"),
    r("exists_subquery", "EXISTS (SELECT ...) correlated"),
    r("group_correlated", "a correlated subquery in a grouped query's output"),
    r("cte", "WITH ... a grouped CTE joined and ordered"),
    r("union_all", "UNION ALL of two index lookups"),
    r("case_aggregate", "SUM(CASE ...) over 1,000 rows"),
    r("window_row_number", "ROW_NUMBER() OVER (PARTITION BY ... ORDER BY ...) over 1,000 rows"),
    r("string_functions", "UPPER, LENGTH, SUBSTR over 100 rows"),
    r("date_bucket", "rows counted by month from a timestamp, 10,000 rows"),
    r("json_field", "a JSON document's field, by primary key"),
    r("json_filter", "rows filtered on a JSON document's field"),
    r("blob_read", "a 64-byte blob by primary key"),
    r("text_read", "400 bytes of text by primary key"),
    r("table_group_by", "GROUP BY over the whole table"),
    r("table_count", "COUNT(*) of the whole table"),
    w("insert_one", "one row, every type"),
    w("insert_batch_100", "a hundred rows in one statement"),
    w("update_plain", "a real and an integer of a row by key"),
    w("update_indexed", "an indexed column of a row by key"),
    w("update_text", "400 bytes of text of a row by key"),
    w("update_json", "a JSON document of a row by key"),
    w("update_range", "a boolean of ten rows by a key range"),
    w("upsert", "INSERT ... ON CONFLICT DO UPDATE on an existing key"),
    w("delete_insert", "a row deleted by key and put back, one transaction"),
    w("delete_range_insert", "ten rows deleted by range and put back, one transaction"),
    w("txn_read_write", "a transaction that reads two rows, updates them and inserts one"),
];

/// `sql` (SQLite's `?` placeholders) in `d`'s: PostgreSQL's `$n`.
pub fn placeholders(d: Dialect, sql: &str) -> String {
    if d != Dialect::Postgres {
        return sql.to_owned();
    }
    let mut out = String::with_capacity(sql.len() + 16);
    let (mut n, mut quoted) = (0, false);
    for ch in sql.chars() {
        match ch {
            '\'' => {
                quoted = !quoted;
                out.push(ch);
            }
            '?' if !quoted => {
                n += 1;
                out.push_str(&format!("${n}"));
            }
            _ => out.push(ch),
        }
    }
    out
}

/// A JSON document's field as an integer and as text, in `d`'s dialect.
fn json_int(d: Dialect, field: &str) -> String {
    match d {
        Dialect::Sqlite => format!("json_extract(doc, '$.{field}')"),
        Dialect::Postgres => format!("(doc->>'{field}')::bigint"),
        Dialect::MySql => format!("CAST(JSON_EXTRACT(doc, '$.{field}') AS SIGNED)"),
        Dialect::ClickHouse => format!("JSONExtractInt(doc, '{field}')"),
    }
}

fn json_text(d: Dialect, field: &str) -> String {
    match d {
        Dialect::Sqlite => format!("json_extract(doc, '$.{field}')"),
        Dialect::Postgres => format!("doc->>'{field}'"),
        Dialect::MySql => format!("JSON_UNQUOTE(JSON_EXTRACT(doc, '$.{field}'))"),
        Dialect::ClickHouse => format!("JSONExtractString(doc, '{field}')"),
    }
}

/// The statements of `op`, in `d`'s dialect (None: the dialect has no form for it).
pub fn statements(op: &str, d: Dialect) -> Option<Vec<String>> {
    use Dialect::*;
    let ch = d == ClickHouse;
    let substr = if ch { "substring" } else { "SUBSTR" };
    let month = match d {
        Sqlite => "strftime('%Y-%m', created / 1000000, 'unixepoch')",
        Postgres => "to_char(to_timestamp(created / 1000000.0), 'YYYY-MM')",
        MySql => "DATE_FORMAT(FROM_UNIXTIME(created DIV 1000000), '%Y-%m')",
        ClickHouse => "formatDateTime(toDateTime(intDiv(created, 1000000)), '%Y-%m')",
    };
    let one = |s: String| Some(vec![s]);
    let s = match op {
        "point_pk" => format!("SELECT {ITEM_COLUMNS} FROM items WHERE id = ?"),
        "point_pk_one_column" => "SELECT price FROM items WHERE id = ?".into(),
        "point_secondary" => "SELECT id, price, name FROM items WHERE k = ?".into(),
        "point_text" => "SELECT id, price FROM items WHERE name = ?".into(),
        "in_list_10" => "SELECT id, price FROM items WHERE id IN (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)".into(),
        "range_pk_100" => "SELECT id, price, qty FROM items WHERE id BETWEEN ? AND ?".into(),
        "range_secondary_100" => "SELECT id, k FROM items WHERE k BETWEEN ? AND ?".into(),
        "filter_multi_column" => "SELECT id, price FROM items WHERE grp = ? AND price > ? AND flag LIMIT 100".into(),
        "filter_no_index" => "SELECT COUNT(*) FROM items WHERE qty = ? AND price < ?".into(),
        "filter_is_null" => "SELECT id FROM items WHERE grp = ? AND note IS NULL LIMIT 100".into(),
        // LIKE ignores case in SQLite's dialect, the others' (binary collations) heed it: GLOB is
        // the same match there.
        "like_prefix" if d == Dialect::Sqlite => "SELECT id, name FROM items WHERE name GLOB ? LIMIT 50".into(),
        "like_prefix" => "SELECT id, name FROM items WHERE name LIKE ? LIMIT 50".into(),
        "like_substring" => "SELECT id FROM items WHERE body LIKE ? LIMIT 20".into(),
        "order_by_index" => "SELECT id, price, created FROM items WHERE grp = ? ORDER BY created DESC LIMIT 20".into(),
        "order_by_sort" => "SELECT id, price FROM items WHERE grp = ? ORDER BY price DESC LIMIT 20".into(),
        "offset_deep" => "SELECT id, name FROM items ORDER BY id LIMIT 20 OFFSET ?".into(),
        "count_range" => "SELECT COUNT(*) FROM items WHERE created BETWEEN ? AND ?".into(),
        "aggregates_range" => "SELECT SUM(price), AVG(qty), MIN(created), MAX(created) FROM items WHERE id BETWEEN ? AND ?".into(),
        "group_by_range" => "SELECT grp, COUNT(*), SUM(price) FROM items WHERE id BETWEEN ? AND ? GROUP BY grp".into(),
        "group_by_having" => "SELECT owner, COUNT(*) FROM items WHERE grp = ? GROUP BY owner HAVING COUNT(*) > 1 ORDER BY COUNT(*) DESC LIMIT 10".into(),
        "distinct_range" => "SELECT DISTINCT grp FROM items WHERE id BETWEEN ? AND ? ORDER BY grp".into(),
        "count_distinct" => "SELECT COUNT(DISTINCT owner) FROM items WHERE grp = ?".into(),
        "join_pk" => "SELECT i.id, i.price, o.name FROM items i JOIN owners o ON o.id = i.owner WHERE i.id BETWEEN ? AND ?".into(),
        "join_group" => "SELECT o.name, COUNT(*), SUM(i.price) FROM owners o JOIN items i ON i.owner = o.id WHERE o.region = ? GROUP BY o.name ORDER BY COUNT(*) DESC LIMIT 10".into(),
        "join_three" => "SELECT r.name, COUNT(*), AVG(i.price) FROM items i JOIN owners o ON o.id = i.owner JOIN regions r ON r.id = o.region WHERE i.id BETWEEN ? AND ? GROUP BY r.name".into(),
        "left_join_anti" => "SELECT o.id FROM owners o LEFT JOIN items i ON i.owner = o.id AND i.grp = ? WHERE o.region = ? AND i.id IS NULL LIMIT 20".into(),
        "subquery_in" => "SELECT id, price FROM items WHERE grp = ? AND owner IN (SELECT id FROM owners WHERE region = ?) LIMIT 50".into(),
        "exists_subquery" => "SELECT o.id, o.name FROM owners o WHERE o.region = ? AND EXISTS (SELECT 1 FROM items i WHERE i.owner = o.id AND i.qty > ?) LIMIT 20".into(),
        "group_correlated" if !ch => "SELECT o.region, count(*), (SELECT count(*) FROM items i WHERE i.grp = o.region) FROM owners o WHERE o.region = ? GROUP BY o.region".into(),
        "cte" => "WITH t AS (SELECT owner, SUM(price) AS s FROM items WHERE grp = ? GROUP BY owner) SELECT o.name, t.s FROM t JOIN owners o ON o.id = t.owner ORDER BY t.s DESC LIMIT 5".into(),
        "union_all" => "SELECT id FROM items WHERE k = ? UNION ALL SELECT id FROM items WHERE name = ?".into(),
        "case_aggregate" => "SELECT SUM(CASE WHEN flag THEN price ELSE 0 END), SUM(CASE WHEN qty > 50 THEN 1 ELSE 0 END) FROM items WHERE id BETWEEN ? AND ?".into(),
        "window_row_number" => "SELECT id, price, ROW_NUMBER() OVER (PARTITION BY grp ORDER BY price DESC) FROM items WHERE id BETWEEN ? AND ?".into(),
        "string_functions" => format!("SELECT UPPER(name), LENGTH(body), {substr}(name, 1, 3) FROM items WHERE id BETWEEN ? AND ?"),
        "date_bucket" => format!("SELECT {month} AS m, COUNT(*) FROM items WHERE id BETWEEN ? AND ? GROUP BY m ORDER BY m"),
        "json_field" => format!("SELECT {} FROM items WHERE id = ?", json_text(d, "b")),
        "json_filter" => format!("SELECT id FROM items WHERE grp = ? AND {} = ? LIMIT 20", json_int(d, "a")),
        "blob_read" => "SELECT data FROM items WHERE id = ?".into(),
        "text_read" => "SELECT body FROM items WHERE id = ?".into(),
        "table_group_by" => "SELECT grp, COUNT(*), AVG(price), MAX(qty) FROM items GROUP BY grp".into(),
        "table_count" => "SELECT COUNT(*) FROM items".into(),
        "insert_one" => insert_items(d, 1),
        "insert_batch_100" => insert_items(d, 100),
        "update_plain" if !ch => "UPDATE items SET price = ?, qty = qty + 1 WHERE id = ?".into(),
        "update_indexed" if !ch => "UPDATE items SET k = ? WHERE id = ?".into(),
        "update_text" if !ch => "UPDATE items SET body = ? WHERE id = ?".into(),
        "update_json" if !ch => match d {
            Postgres => "UPDATE items SET doc = ?::text::jsonb WHERE id = ?".into(),
            _ => "UPDATE items SET doc = ? WHERE id = ?".into(),
        },
        "update_range" if !ch => "UPDATE items SET flag = NOT flag WHERE id BETWEEN ? AND ?".into(),
        "upsert" => match d {
            Sqlite | Postgres => format!("{} ON CONFLICT (id) DO UPDATE SET price = excluded.price, qty = excluded.qty", insert_items(d, 1)),
            MySql => format!("{} AS new ON DUPLICATE KEY UPDATE price = new.price, qty = new.qty", insert_items(d, 1)),
            ClickHouse => return None,
        },
        "delete_insert" if !ch => return Some(vec!["DELETE FROM items WHERE id = ?".into(), insert_items(d, 1)]),
        "delete_range_insert" if !ch => return Some(vec!["DELETE FROM items WHERE id BETWEEN ? AND ?".into(), insert_items(d, 10)]),
        "txn_read_write" if !ch => {
            return Some(vec![
                "SELECT qty FROM items WHERE id = ?".into(),
                "UPDATE items SET qty = qty + 1 WHERE id = ?".into(),
                "SELECT price FROM items WHERE id = ?".into(),
                "UPDATE items SET price = price + 0.01 WHERE id = ?".into(),
                insert_items(d, 1),
            ])
        }
        _ => return None,
    };
    one(s)
}

/// The parameters of each of `op`'s statements, drawn for one run of it by client `c.0` of `c.1`:
/// reads anywhere in the table, writes in the client's own slice of it.
pub fn params(op: &str, rng: &mut Rng, shape: &Shape, c: (usize, usize), d: Dialect) -> Vec<Vec<P>> {
    let write = OPS.iter().any(|o| o.name == op && o.kind == Kind::Write);
    let id = if write { shape.own(rng, c) } else { shape.id(rng) };
    let grp = || P::I((mix(id as u64 ^ 0x1111) % 100) as i64);
    let region = (rng.next() % shape.regions as u64) as i64 + 1;
    let word = WORDS[(rng.next() % WORDS.len() as u64) as usize];
    // A secondary range wide enough for about a hundred of the table's keys.
    let k_span = (((i64::MAX as f64) / shape.items.max(1) as f64) * 100.0) as i64;
    let new_id = || shape.next.fetch_add(1, Ordering::Relaxed);
    let one = |p: Vec<P>| vec![p];
    match op {
        "point_pk" | "point_pk_one_column" | "json_field" | "blob_read" | "text_read" => one(vec![P::I(id)]),
        "point_secondary" => one(vec![P::I(k_of(id))]),
        "point_text" => one(vec![P::T(name_of(id))]),
        "in_list_10" => one((0..10).map(|_| P::I(shape.id(rng))).collect()),
        "range_pk_100" => {
            let a = shape.range(rng, 100);
            one(vec![P::I(a), P::I(a + 99)])
        }
        "range_secondary_100" => {
            let a = k_of(id);
            one(vec![P::I(a), P::I(a.saturating_add(k_span))])
        }
        "filter_multi_column" => one(vec![grp(), P::F((rng.next() % 10_000) as f64)]),
        "filter_no_index" => one(vec![P::I((rng.next() % 100) as i64), P::F((rng.next() % 1000) as f64)]),
        "filter_is_null" | "order_by_index" | "order_by_sort" | "count_distinct" | "cte" | "group_by_having" => one(vec![grp()]),
        "like_prefix" => {
            let wildcard = if d == Dialect::Sqlite { '*' } else { '%' };
            one(vec![P::T(format!("{word}-{:x}{wildcard}", rng.next() % 16))])
        }
        "like_substring" => one(vec![P::T("%zephyr%".into())]),
        "offset_deep" => one(vec![P::I(shape.range(rng, 20) - 1)]),
        "count_range" => {
            let a = shape.range(rng, 10_000);
            one(vec![P::I(BASE_US + a * 1_000_000), P::I(BASE_US + (a + 9_999) * 1_000_000)])
        }
        "aggregates_range" | "join_three" | "case_aggregate" | "window_row_number" => {
            let a = shape.range(rng, 1000);
            one(vec![P::I(a), P::I(a + 999)])
        }
        "group_by_range" | "distinct_range" | "date_bucket" => {
            let a = shape.range(rng, 10_000);
            one(vec![P::I(a), P::I(a + 9_999)])
        }
        "join_pk" => {
            let a = shape.range(rng, 20);
            one(vec![P::I(a), P::I(a + 19)])
        }
        "string_functions" => {
            let a = shape.range(rng, 100);
            one(vec![P::I(a), P::I(a + 99)])
        }
        "join_group" => one(vec![P::I(region)]),
        "left_join_anti" | "subquery_in" => one(vec![grp(), P::I(region)]),
        "exists_subquery" => one(vec![P::I(region), P::I(90 + (rng.next() % 10) as i64)]),
        "group_correlated" => one(vec![P::I(region)]),
        "union_all" => one(vec![P::I(k_of(id)), P::T(name_of(shape.id(rng)))]),
        "json_filter" => one(vec![grp(), P::I((rng.next() % 100) as i64)]),
        "table_group_by" | "table_count" => one(vec![]),
        "insert_one" => one(item_params(new_id(), shape.owners)),
        "insert_batch_100" => {
            let first = shape.next.fetch_add(100, Ordering::Relaxed);
            one((first..first + 100).flat_map(|i| item_params(i, shape.owners)).collect())
        }
        "update_plain" => one(vec![P::F((rng.next() % 1_000_000) as f64 / 100.0), P::I(id)]),
        "update_indexed" => one(vec![P::I((rng.next() >> 1) as i64), P::I(id)]),
        "update_text" => {
            let mut p = item_params(id, shape.owners);
            let body = std::mem::replace(&mut p[9], P::N);
            one(vec![match body {
                P::T(mut b) => {
                    b.insert_str(0, word);
                    b.truncate(400);
                    P::T(b)
                }
                other => other,
            }, P::I(id)])
        }
        "update_json" => one(vec![P::T(format!(r#"{{"a":{},"b":"{word}","c":[{}]}}"#, rng.next() % 100, rng.next() % 10)), P::I(id)]),
        "update_range" => {
            let a = shape.own_range(rng, c, 10);
            one(vec![P::I(a), P::I(a + 9)])
        }
        "upsert" => {
            let mut p = item_params(id, shape.owners);
            p[4] = P::F((rng.next() % 1_000_000) as f64 / 100.0);
            one(p)
        }
        "delete_insert" => vec![vec![P::I(id)], item_params(id, shape.owners)],
        "delete_range_insert" => {
            let a = shape.own_range(rng, c, 10);
            vec![vec![P::I(a), P::I(a + 9)], (a..a + 10).flat_map(|i| item_params(i, shape.owners)).collect()]
        }
        "txn_read_write" => {
            let b = shape.own(rng, c);
            vec![vec![P::I(id)], vec![P::I(id)], vec![P::I(b)], vec![P::I(b)], item_params(new_id(), shape.owners)]
        }
        _ => vec![],
    }
}

struct Opts {
    target: String,
    dir: String,
    dsn: Option<String>,
    items: i64,
    clients: Vec<usize>,
    seconds: f64,
    warmup: f64,
    only: Option<String>,
    /// Each statement's plan (its database's EXPLAIN) instead of timing it.
    explain: bool,
    /// The database a run left in `dir`, as that run loaded, indexed and settled it, instead of a
    /// new one: a statement profiled again without loading ten million rows again. A diagnosis,
    /// never a record (its runs follow whatever the last run's writes left).
    reuse: bool,
}

fn opts() -> anyhow::Result<Opts> {
    let mut o = Opts { target: String::new(), dir: "/tmp/amphora-suite".into(), dsn: None, items: 200_000, clients: vec![1, 32], seconds: 5.0, warmup: 1.0, only: None, explain: false, reuse: false };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut v = || args.next().ok_or_else(|| anyhow::anyhow!("{a} takes a value"));
        match a.as_str() {
            "--suite" => {}
            "--target" => o.target = v()?,
            "--dir" => o.dir = v()?,
            "--dsn" => o.dsn = Some(v()?),
            "--items" => o.items = v()?.parse()?,
            "--clients" => o.clients = v()?.split(',').map(str::parse).collect::<Result<_, _>>()?,
            "--seconds" => o.seconds = v()?.parse()?,
            "--warmup" => o.warmup = v()?.parse()?,
            "--only" => o.only = Some(v()?),
            "--explain" => o.explain = true,
            "--reuse" => o.reuse = true,
            other => anyhow::bail!("unknown argument {other} (the suite takes --target, --dir, --dsn, --items, --clients, --seconds, --warmup, --only, --explain, --reuse)"),
        }
    }
    Ok(o)
}

/// One statement at one client count: what it did.
struct Outcome {
    hist: Hist,
    rows: u64,
    errors: u64,
    error: Option<String>,
    /// Runs a database asked to be tried again (a conflict, a deadlock, a busy database), tried
    /// again as an application would: their time counts in the run's latency.
    retries: u64,
    /// The longest run, in µs, whenever it began and ended (warmup included): a statement that
    /// answers nothing within the window says how long it took.
    longest_us: u64,
    /// This process's user and system CPU seconds in the measured window ([`cpu_times`]): an
    /// embedded database's work and its clients', a server's clients' alone.
    cpu: Option<(f64, f64)>,
}

/// Whether a database's refusal asks to be tried again (serialization failures, deadlocks, write
/// conflicts, a locked database).
fn retryable(e: &anyhow::Error) -> bool {
    let s = format!("{e:#}").to_lowercase();
    ["could not serialize", "deadlock", "conflict with concurrent", "lock wait timeout", "database is locked", "busy", "try restarting transaction", "concurrently modified"].iter().any(|m| s.contains(m))
}

/// This process's user and system CPU seconds, every thread's, ended ones too: Linux's
/// /proc/self/stat (proc(5): fields 14 and 15, in clock ticks, 100 a second on every Linux). None
/// where there is no /proc.
fn cpu_times() -> Option<(f64, f64)> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // The command's name (field 2) is in parentheses and may hold spaces: count from after it.
    let mut fields = stat.get(stat.rfind(')')? + 1..)?.split_whitespace();
    let user: f64 = fields.nth(11)?.parse().ok()?;
    let system: f64 = fields.next()?.parse().ok()?;
    Some((user / 100.0, system / 100.0))
}

/// AMPHORA_PERF_CTL=<control fifo>,<ack fifo>: perf (`perf stat|record --control
/// fifo:<control>,<ack> --delay=-1`, started first: perf opens both ends, so neither open here
/// blocks) counts each statement's measured window and nothing else: not the load, the warmup or
/// another statement. Each command waits for perf's ack before the window goes on.
fn perf_ctl(cmd: &str) {
    use std::io::{Read, Write};
    let Some(spec) = std::env::var("AMPHORA_PERF_CTL").ok() else { return };
    let Some((ctl, ack)) = spec.split_once(',') else {
        eprintln!("AMPHORA_PERF_CTL takes <control fifo>,<ack fifo>");
        return;
    };
    let fifo = |path: &str| std::fs::OpenOptions::new().read(true).write(true).open(path);
    let done = fifo(ctl).and_then(|mut f| f.write_all(format!("{cmd}\n").as_bytes())).and_then(|()| fifo(ack)).and_then(|mut f| f.read_exact(&mut [0u8; 4]));
    if let Err(e) = done {
        eprintln!("perf {cmd}: {e}");
    }
}

/// Runs `op` from `clients` clients for the warmup and then `seconds`, each client on a session of
/// its own, closed loop.
fn run_op(target: &Arc<dyn Target>, op: &Op, stmts: &Arc<Vec<String>>, shape: &Arc<Shape>, clients: usize, warmup: f64, seconds: f64) -> Outcome {
    let stop = Arc::new(AtomicBool::new(false));
    let began = Instant::now();
    let count_from = began + Duration::from_secs_f64(warmup);
    let until = count_from + Duration::from_secs_f64(seconds);
    let total = Arc::new(Mutex::new(Outcome { hist: Hist::new(), rows: 0, errors: 0, error: None, retries: 0, longest_us: 0, cpu: None }));
    // Each statement's clients draw from streams of their own: mix is a bijection, so no two
    // clients' seeds are equal (adding the client's number and then XORing it in made clients 0, 2,
    // 4, ... draw the same parameters, and statements whose names were as long drew the same).
    let base = op.name.bytes().fold(0x2545_f491_4f6c_dd1d_u64, |h, b| mix(h ^ u64::from(b)));
    let d = target.dialect();
    let cpu = std::thread::scope(|s| {
        // The measured window's own thread: the process's CPU time in it, and perf on and off
        // around it (perf_ctl).
        let window = s.spawn(move || {
            std::thread::sleep(count_from.saturating_duration_since(Instant::now()));
            let before = cpu_times();
            // An empty window turns nothing on, so perf counts nothing for it.
            let counted = until > count_from;
            if counted {
                perf_ctl("enable");
            }
            std::thread::sleep(until.saturating_duration_since(Instant::now()));
            if counted {
                perf_ctl("disable");
            }
            before.zip(cpu_times()).map(|((u0, s0), (u1, s1))| (u1 - u0, s1 - s0))
        });
        for c in 0..clients {
            let (target, stmts, shape, stop, total) = (target.clone(), stmts.clone(), shape.clone(), stop.clone(), total.clone());
            let mut rng = Rng(mix(base ^ mix(c as u64 + 1)) | 1);
            s.spawn(move || {
                let mut mine = Outcome { hist: Hist::new(), rows: 0, errors: 0, error: None, retries: 0, longest_us: 0, cpu: None };
                match target.session() {
                    Err(e) => {
                        mine.errors += 1;
                        mine.error = Some(format!("session: {e:#}"));
                    }
                    Ok(mut session) => {
                        let mut failing = 0u32;
                        while !stop.load(Ordering::Relaxed) {
                            let now = Instant::now();
                            if now >= until {
                                stop.store(true, Ordering::Relaxed);
                                break;
                            }
                            let p = params(op.name, &mut rng, &shape, (c, clients), d);
                            let t = Instant::now();
                            let mut got = session.run(op.kind, &stmts, &p);
                            for _ in 0..20 {
                                match &got {
                                    Err(e) if retryable(e) => {
                                        mine.retries += 1;
                                        got = session.run(op.kind, &stmts, &p);
                                    }
                                    _ => break,
                                }
                            }
                            let ended = Instant::now();
                            let us = u64::try_from(ended.duration_since(t).as_micros()).unwrap_or(u64::MAX);
                            mine.longest_us = mine.longest_us.max(us);
                            match got {
                                Ok(rows) => {
                                    failing = 0;
                                    // A run counts where it ended: answered within the window,
                                    // whenever it began. Counting runs begun in the window instead
                                    // dropped every run a slow one ahead of it pushed past the warmup,
                                    // and counted runs answered long after the window closed.
                                    if ended >= count_from && ended <= until {
                                        mine.hist.record(us);
                                        mine.rows += rows as u64;
                                    }
                                }
                                Err(e) => {
                                    mine.errors += 1;
                                    if mine.error.is_none() {
                                        mine.error = Some(format!("{e:#}").chars().take(300).collect());
                                    }
                                    // A statement refused again and again isn't run for the rest of
                                    // its time: its first error is the finding.
                                    failing += 1;
                                    if failing >= 20 {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                if let Ok(mut t) = total.lock() {
                    t.hist.merge(&mine.hist);
                    t.rows += mine.rows;
                    t.errors += mine.errors;
                    t.retries += mine.retries;
                    t.longest_us = t.longest_us.max(mine.longest_us);
                    if t.error.is_none() {
                        t.error = mine.error;
                    }
                }
            });
        }
        window.join().ok().flatten()
    });
    let lost = || Outcome { hist: Hist::new(), rows: 0, errors: 0, error: Some("lost the run's outcome".into()), retries: 0, longest_us: 0, cpu: None };
    let mut out = Arc::try_unwrap(total).ok().and_then(|m| m.into_inner().ok()).unwrap_or_else(lost);
    out.cpu = cpu;
    out
}

/// Whether `--only` takes statement `name`: every statement without it; with it, those its
/// comma-separated names begin, or with a trailing `$` the one it names (`point_pk$`, not
/// `point_pk_one_column`).
fn selected(only: Option<&str>, name: &str) -> bool {
    only.is_none_or(|only| only.split(',').any(|n| n.strip_suffix('$').map_or(name.starts_with(n), |exact| name == exact)))
}

/// Makes the suite's tables, loads them, builds their indexes and settles the database: each
/// step's seconds.
fn load_all(target: &Arc<dyn Target>, d: Dialect, shape: &Shape, cedardb: bool) -> anyhow::Result<(f64, f64, f64)> {
    let began = Instant::now();
    target.setup(&tables(d, cedardb).into_iter().map(|s| (s, vec![])).collect::<Vec<_>>())?;
    // A hundred rows a statement keeps every placeholder-bound engine under its parameter limit
    // (SQLite 32,766, PostgreSQL's and MySQL's 65,535). ClickHouse makes each INSERT a part it then
    // merges, and asks for bulk inserts of at least 1,000 rows, better 10,000 to 100,000 (its
    // documentation, "Bulk inserts"): at ten million rows, a hundred a statement is 100,000 parts.
    let batch = if d == Dialect::ClickHouse { 20_000 } else { 100 };
    load(d, shape, batch, |txn| target.setup(&txn))?;
    let loaded_s = began.elapsed().as_secs_f64();
    for s in indexes(d) {
        // A statement a database refuses here (its statistics, say) is said, not fatal.
        if let Err(e) = target.setup(&[(s.clone(), vec![])]) {
            eprintln!("setup: {s}: {e:#}");
        }
    }
    let indexed_s = began.elapsed().as_secs_f64() - loaded_s;
    let settling = Instant::now();
    // A database that refuses its checkpoint is said, not stopped: its runs read what it left.
    if let Err(e) = target.settle() {
        eprintln!("settle: {e:#}");
    }
    Ok((loaded_s, indexed_s, settling.elapsed().as_secs_f64()))
}

pub fn main() -> anyhow::Result<()> {
    let o = opts()?;
    let shape = Arc::new(Shape::new(o.items));
    let readers = o.clients.iter().copied().max().unwrap_or(1);
    let target: Arc<dyn Target> = suite_db::open(&o.target, &o.dir, o.dsn.as_deref(), readers, o.reuse)?;
    let d = target.dialect();

    // --reuse: the database a run left, as that run loaded, indexed and settled it.
    let (loaded_s, indexed_s, settle_s) = if o.reuse { (0.0, 0.0, 0.0) } else { load_all(&target, d, &shape, o.target == "cedardb")? };
    target.ready()?;
    let rss_after_load_mb = crate::rss_mb();
    forget_peaks();

    if o.explain {
        let mut session = target.session()?;
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut plans = serde_json::Map::new();
        for op in OPS {
            if !selected(o.only.as_deref(), op.name) {
                continue;
            }
            let Some(stmts) = statements(op.name, d) else { continue };
            let p = params(op.name, &mut rng, &shape, (0, 1), d);
            let (Some(sql), Some(first)) = (stmts.first(), p.first()) else { continue };
            let plan = match session.explain(&placeholders(d, sql), first) {
                Ok(lines) => serde_json::json!(lines),
                Err(e) => serde_json::json!(format!("error: {e:#}")),
            };
            plans.insert(op.name.to_owned(), plan);
        }
        println!("{}", serde_json::json!({ "suite": 1, "explain": true, "target": o.target, "items": shape.items, "plans": plans }));
        return Ok(());
    }

    let mut results = Vec::new();
    for op in OPS {
        if !selected(o.only.as_deref(), op.name) {
            continue;
        }
        let Some(stmts) = statements(op.name, d) else {
            for &clients in &o.clients {
                results.push(serde_json::json!({ "op": op.name, "kind": format!("{:?}", op.kind), "what": op.what, "clients": clients, "unsupported": true }));
            }
            continue;
        };
        let stmts = Arc::new(stmts.iter().map(|s| placeholders(d, s)).collect::<Vec<_>>());
        for &clients in &o.clients {
            eprintln!("== {} {} clients={clients}", o.target, op.name);
            // AMPHORA_PROFILE=<prefix>: each statement's run sampled 997 times a second, written as
            // <prefix>-<target>-<statement>-<clients>.{svg,txt,folded} (main.rs, write_profile).
            let profile = std::env::var("AMPHORA_PROFILE").ok();
            let profiler = match &profile {
                Some(_) => Some(pprof::ProfilerGuardBuilder::default().frequency(997).blocklist(&["libc", "libgcc", "pthread", "vdso"]).build()?),
                None => None,
            };
            let out = run_op(&target, op, &stmts, &shape, clients, o.warmup, o.seconds);
            if let (Some(prefix), Some(guard)) = (&profile, profiler) {
                crate::write_profile(&format!("{prefix}-{}-{}-{clients}", o.target, op.name), &guard)?;
            }
            let n = out.hist.n;
            results.push(serde_json::json!({
                "op": op.name,
                "kind": format!("{:?}", op.kind),
                "what": op.what,
                "clients": clients,
                "ops_per_s": n as f64 / o.seconds,
                "rows_per_op": if n == 0 { 0.0 } else { out.rows as f64 / n as f64 },
                "ms": summary(out.hist),
                "errors": out.errors,
                "retries": out.retries,
                "error": out.error,
                "longest_ms": out.longest_us as f64 / 1000.0,
                "cpu_user_s": out.cpu.map(|c| c.0),
                "cpu_sys_s": out.cpu.map(|c| c.1),
            }));
        }
    }
    let report = serde_json::json!({
        "suite": 1,
        "target": o.target,
        "items": shape.items,
        "owners": shape.owners,
        "regions": shape.regions,
        "clients": o.clients,
        "seconds": o.seconds,
        "warmup": o.warmup,
        "load_s": loaded_s,
        "index_s": indexed_s,
        "settle_s": settle_s,
        "reused": o.reuse,
        "results": results,
        "rss_after_load_mb": rss_after_load_mb,
        "rss_max_mb": crate::rss_max_mb(),
        "heap_peak_mb": crate::heap_mb(&crate::HEAP_PEAK),
        "heap_end_mb": crate::heap_mb(&crate::HEAP),
        "stats": target.stats(),
    });
    println!("{report}");
    Ok(())
}
