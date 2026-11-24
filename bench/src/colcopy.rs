//! Column copies within a fixed memory limit against DuckDB (kiln goal c436eeb5, lane colcopy-5,
//! AC-CB.4.2): the scenarios of the rebuy policies' A/B (`columns_ab`, the definitions the
//! engine's A/B test runs too, T-CB.4.1; lane colcopy-7) run by one driver over Amphora under
//! each policy and over DuckDB, both in this process through their Rust libraries
//! (`amphora-engine` and the `duckdb` crate, bundled), in memory, the same statements on the same
//! rows, timed the same way (each statement's prepare, run and every row read, by one `Instant`),
//! under the same memory cap (docs/design/columnar.md, decision 2).
//!
//! The cap, statement by statement: what the engine holds for the table's rows now and the
//! scenario's free bytes on top. Amphora's rows and its copies take from one memory source; the
//! driver's other use takes the rest of it, so the copies have the free bytes in all (the A/B's
//! `press`). DuckDB's table is its columns: `SET memory_limit` to the bytes `duckdb_memory()`
//! counts before the statement and the same free bytes, spilling to `--dir` past it. The free
//! bytes are shares of what one table's copy needs on Amphora at that scale (the scenario's
//! definition), so both engines get the same room for the same analytic work.
//!
//! One JSON line per engine, policy and scenario: latency p50, p99, p99.9 (ns), statements a
//! second, the engine's bytes at most (sampled after each statement), its analytic bytes at most
//! (Amphora: the copies' granted bytes; DuckDB: the same total, its table being its columns), the
//! cap at most, statements the engine refused, DuckDB's spill at most, whether every answer
//! equalled Amphora's row loop's, and the client and platform that produced it. Amphora's lines
//! add what its copies did and why: kept, let go, bought again, resumed, still waiting at the
//! end, the last one let go and the memory refusal behind it, and the copies' trace events by
//! reason (`columns_ab::Traces`). A write the cap refuses is counted (`writes_refused`) and left
//! out of the reference too, so the answers still compare.
//!
//! `--ladder` runs the concurrency ladder instead (lane colcopy-8, AC-CB.4.5): each scenario on
//! each engine at 1, 8, 32, 128, 512, 2048 and 4096 clients at once (`--clients` for fewer; a
//! tiny instance may stop at 128), each client its own connection and thread, all through the
//! same driver and timed the same way; one JSON line a step with its p50, p99 and p99.9, and what
//! says whether the client bounds it: its threads and connections, the process's peak resident
//! bytes in the step (Linux), the cores, and the client's own share of the clients' time. A step
//! runs `clients` times the scenario's statements.
//!
//! Only statements a copy could answer are what the A/B and the comparison weigh (lane colcopy-9,
//! AC-CB.4.6): a statement no copy can answer (the scan hook's `Skip`: its rows, a filter only its
//! loop states, a join, a transaction) reads the rows under either policy. Before the scenarios,
//! each of the A/B's queries is run once on a probe table and Amphora's copy counts say whether
//! it reaches the copies; every record's percentiles (Amphora's and DuckDB's, one client or a
//! ladder's step) count only the statements that do, `all_p50_ns` and the rest count every one,
//! and `skipped_queries` names the queries left out and why. An Amphora record's `probe_agrees`
//! says whether its own counts left out the same statements.
//!
//! usage: amphora-bench --colcopy --dir DIR [--scale K] [--engines rent,room,duckdb]
//!        [--duckdb-threads N] [--ladder [--clients 1,8,32,...]] [--out FILE]
//!
//! `--duckdb-threads` defaults to 1, the threads one Amphora statement runs on.

use std::sync::Arc;
use std::time::Instant;

use amphora_engine::storage::versioned::columns_ab::{
    Handoffs, QUERIES, Scenario, Traces, scenarios, schema,
};
use amphora_engine::storage::versioned::columns_sql::{Dropped, Rebuy};
use amphora_engine::storage::versioned::memory::{Capped, Source};
use amphora_engine::storage::versioned::switch::Storage;
use amphora_engine::{ConnectOpts, Database, MemoryIO, Numeric, SqliteDialect, StepResult, Value};

fn e(err: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("{err}")
}

/// The A/B's rows, in each engine's spelling of a series and a text concatenation.
fn fill(duck: bool, i: usize, from: i64, to: i64) -> String {
    let (series, key) = if duck {
        (
            format!("generate_series({from}, {to}) AS g(value)"),
            "'k' || CAST(value % 11 AS VARCHAR)",
        )
    } else {
        (
            format!("generate_series({from}, {to})"),
            "'k' || (value % 11)",
        )
    };
    format!(
        "INSERT INTO t{i} SELECT value, value % 7, (value * 37) % 1000, CASE WHEN value % 5 = 0 THEN NULL ELSE {key} END FROM {series}"
    )
}

/// A statement's rows as text, sorted: GROUP BY's order is neither engine's promise, and a sum is
/// a HUGEINT on DuckDB and an INTEGER here.
type Answer = Vec<Vec<String>>;

fn amphora_text(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Numeric(Numeric::Integer(i)) => i.to_string(),
        Value::Numeric(Numeric::Float(f)) => f64::from(*f).to_string(),
        other => other.to_text().unwrap_or_default().to_string(),
    }
}

/// One Amphora database on the versioned storage, in memory.
struct Amphora {
    io: Arc<dyn amphora_engine::IO>,
    db: Arc<Database>,
    conn: Arc<amphora_engine::Connection>,
    storage: Arc<Storage>,
    /// Its tables are declared `COLUMNAR`, so they keep column copies (decision 1, item 1).
    columnar: bool,
}

impl Amphora {
    fn open(room: Option<Arc<dyn Source>>, copies: Option<Rebuy>) -> anyhow::Result<Self> {
        let io: Arc<dyn amphora_engine::IO> = Arc::new(MemoryIO::new());
        let db = Database::open_file(io.clone(), ":memory:", Arc::new(SqliteDialect)).map_err(e)?;
        let opts = ConnectOpts::new().with_versioned_storage();
        let opts = match room {
            Some(room) => opts.with_memory(room),
            None => opts,
        };
        let conn = db.connect(opts).map_err(e)?;
        let storage = conn
            .versioned_storage()
            .ok_or_else(|| anyhow::anyhow!("no versioned storage"))?;
        if let Some(rebuy) = copies {
            storage.set_copy_rebuy(rebuy);
        }
        Ok(Self {
            io,
            db,
            conn,
            storage,
            columnar: copies.is_some(),
        })
    }

    fn exec(&self, sql: &str) -> anyhow::Result<()> {
        self.conn.execute(sql).map_err(e)
    }

    /// Runs `sql`, a CREATE TABLE, declared `COLUMNAR` when this database keeps copies.
    fn create(&self, sql: &str) -> anyhow::Result<()> {
        if self.columnar {
            self.exec(&format!("{sql} COLUMNAR"))
        } else {
            self.exec(sql)
        }
    }

    fn rows(&self, sql: &str) -> anyhow::Result<Answer> {
        let mut stmt = self.conn.prepare(sql).map_err(e)?;
        let mut out = Vec::new();
        loop {
            match stmt.step().map_err(e)? {
                StepResult::Row => {
                    if let Some(r) = stmt.row() {
                        out.push(r.get_values().map(amphora_text).collect());
                    }
                }
                StepResult::Done => break,
                // A sleep is stepped as I/O is, as its doc allows a caller that keeps no time.
                StepResult::IO | StepResult::Yield | StepResult::Sleep { .. } => {
                    self.io.step().map_err(e)?
                }
                other => anyhow::bail!("step: {other:?}"),
            }
        }
        out.sort_unstable();
        Ok(out)
    }

    /// Leave this round's room for the copies, accounting for bytes already granted.
    fn press(&self, room: &Capped, other: &mut u64, free: u64) -> anyhow::Result<()> {
        let want = free.saturating_sub(self.storage.copy_meter().held);
        let now = room.room().map_err(e)?;
        if now > want {
            room.take("the driver's other use", now - want).map_err(e)?;
            *other = other.saturating_add(now - want);
        } else {
            let back = (want - now).min(*other);
            room.give(back);
            *other = other.saturating_sub(back);
        }
        Ok(())
    }

    /// The copies' resident and granted bytes.
    fn copies(&self) -> (u64, u64) {
        self.storage.copy_held()
    }
}

/// The bytes DuckDB's buffer manager holds now, every tag.
const MEMORY_SQL: &str = "SELECT coalesce(sum(memory_usage_bytes), 0)::BIGINT FROM duckdb_memory()";

/// One in-memory DuckDB, spilling to `dir`.
struct Duck {
    conn: duckdb::Connection,
}

impl Duck {
    fn open(dir: &std::path::Path, threads: usize) -> anyhow::Result<Self> {
        let conn = duckdb::Connection::open_in_memory()?;
        let spill = dir
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("a --dir that isn't UTF-8"))?
            .replace('\'', "''");
        conn.execute_batch(&format!(
            "SET threads = {threads}; SET temp_directory = '{spill}'"
        ))?;
        Ok(Self { conn })
    }

    fn rows(&self, sql: &str) -> anyhow::Result<Answer> {
        Self::rows_on(&self.conn, sql)
    }

    /// [`Self::rows`] on any connection of the database (a ladder's client's).
    fn rows_on(conn: &duckdb::Connection, sql: &str) -> anyhow::Result<Answer> {
        use duckdb::types::ValueRef;
        let mut stmt = conn.prepare(sql)?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            let mut row = Vec::new();
            for i in 0..r.as_ref().column_count() {
                row.push(match r.get_ref(i)? {
                    ValueRef::Null => "NULL".into(),
                    ValueRef::Int(n) => n.to_string(),
                    ValueRef::BigInt(n) => n.to_string(),
                    ValueRef::HugeInt(n) => n.to_string(),
                    ValueRef::Double(f) => f.to_string(),
                    ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
                    other => anyhow::bail!(
                        "a {} value, which the driver has no text for",
                        other.data_type()
                    ),
                });
            }
            out.push(row);
        }
        out.sort_unstable();
        Ok(out)
    }

    fn one(&self, sql: &str) -> anyhow::Result<u64> {
        Self::one_on(&self.conn, sql)
    }

    fn one_on(conn: &duckdb::Connection, sql: &str) -> anyhow::Result<u64> {
        let n: i64 = conn.query_row(sql, [], |r| r.get(0))?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// The bytes DuckDB's buffer manager holds now, every tag.
    fn memory(&self) -> anyhow::Result<u64> {
        self.one(MEMORY_SQL)
    }

    /// The bytes it has spilled to its temporary files now.
    fn spilled(&self) -> anyhow::Result<u64> {
        self.one("SELECT coalesce(sum(size), 0)::BIGINT FROM duckdb_temporary_files()")
    }
}

/// The bytes granted the copy of one table of `rows` rows, with room to spare: the A/B's `need`.
fn need(rows: i64) -> anyhow::Result<u64> {
    let a = Amphora::open(None, Some(Rebuy::Rent))?;
    a.create(&schema(0))?;
    a.exec(&fill(false, 0, 1, rows))?;
    let q = amphora_engine::storage::versioned::columns_ab::QUERIES
        .get(2)
        .copied()
        .unwrap_or_default()
        .replace("FROM t", "FROM t0");
    for _ in 0..3 {
        a.rows(&q)?;
    }
    Ok(a.copies().1)
}

/// Whether each of the A/B's queries (`QUERIES`' places) reaches the copies: None if it does, else
/// why not (Amphora's `Skip` reason, or "not_handed" when the scan hook never hands it over). Each
/// runs once on a table of `rows` rows, copies on, no cap: a skip is the statement's shape and
/// its transaction, never the copies' state (lane colcopy-9, AC-CB.4.6).
fn reach(rows: i64) -> anyhow::Result<Vec<Option<&'static str>>> {
    let a = Amphora::open(None, Some(Rebuy::Rent))?;
    a.create(&schema(0))?;
    a.exec(&fill(false, 0, 1, rows))?;
    let mut out = Vec::with_capacity(QUERIES.len());
    for q in QUERIES {
        let before = a.storage.copy_counts();
        a.rows(&q.replace("FROM t", "FROM t0"))?;
        let after = a.storage.copy_counts();
        out.push(if after.reached_since(&before, 1) {
            None
        } else if after.handed == before.handed {
            Some("not_handed")
        } else if after.skipped_defaults > before.skipped_defaults {
            Some("defaults")
        } else if after.skipped_rows > before.skipped_rows {
            Some("rows")
        } else if after.skipped_body > before.skipped_body {
            Some("body")
        } else if after.skipped_join > before.skipped_join {
            Some("join")
        } else {
            Some("transaction")
        });
    }
    Ok(out)
}

/// Whether scenario statement `k` reaches the copies, by [`reach`].
fn reaches(reach: &[Option<&'static str>], k: usize) -> bool {
    reach
        .get(k % QUERIES.len().max(1))
        .is_some_and(Option::is_none)
}

/// The queries `reach` leaves out, by place, with why.
fn skipped_queries(reach: &[Option<&'static str>]) -> serde_json::Value {
    serde_json::Value::Object(
        reach
            .iter()
            .enumerate()
            .filter_map(|(i, why)| why.map(|w| (i.to_string(), serde_json::json!(w))))
            .collect(),
    )
}

fn at(sorted: &[u64], p: f64) -> u64 {
    let n = sorted.len();
    let i = ((p * n as f64).ceil() as usize).clamp(1, n.max(1)) - 1;
    sorted.get(i).copied().unwrap_or(0)
}

#[derive(Default)]
struct Tally {
    /// The statements a copy could answer ([`reach`]), and every statement.
    latencies: Vec<u64>,
    all: Vec<u64>,
    engine_peak: u64,
    analytic_peak: u64,
    cap_peak: u64,
    spill_peak: u64,
    refused: u64,
    /// The first refusal's text.
    refusal: Option<String>,
    /// Writes the cap refused, and the first one's text.
    writes_refused: u64,
    write_refusal: Option<String>,
    equal: bool,
}

/// Which engine a run is on.
#[derive(Clone, Copy)]
enum Engine {
    Amphora(Rebuy),
    Duck,
}

struct Context<'a> {
    dir: &'a std::path::Path,
    threads: usize,
    client: &'a serde_json::Value,
    platform: &'a serde_json::Value,
    /// Whether each query reaches the copies ([`reach`]).
    reach: &'a [Option<&'static str>],
}

fn run(
    cx: &Context<'_>,
    s: &Scenario,
    engine: Engine,
    need: u64,
) -> anyhow::Result<serde_json::Value> {
    // The reference: Amphora's row loop, copies off, with no cap.
    let reference = Amphora::open(None, None)?;
    let room = Arc::new(Capped::new(u64::MAX / 2));
    let shared: Arc<dyn Source> = room.clone();
    let (amphora, duck) = match engine {
        Engine::Amphora(rebuy) => (Some(Amphora::open(Some(shared), Some(rebuy))?), None),
        Engine::Duck => (None, Some(Duck::open(cx.dir, cx.threads)?)),
    };
    for i in 0..s.tables {
        reference.create(&schema(i))?;
        reference.exec(&fill(false, i, 1, s.rows))?;
        if let Some(a) = &amphora {
            a.create(&schema(i))?;
            a.exec(&fill(false, i, 1, s.rows))?;
        }
        if let Some(d) = &duck {
            d.conn.execute_batch(&schema(i))?;
            d.conn.execute_batch(&fill(true, i, 1, s.rows))?;
        }
    }
    let mut handoffs = Handoffs::default();
    let mut t = Tally {
        equal: true,
        ..Tally::default()
    };
    let mut other = 0u64;
    let mut next = vec![s.rows + 1; s.tables];
    let began = Instant::now();
    for k in 0..s.statements {
        let i = s.table(k);
        if s.grow > 0 {
            let from = next.get(i).copied().unwrap_or(s.rows + 1);
            let to = from + s.grow - 1;
            // The row growth has room on both engines. Amphora publishes its changes to the
            // copies at COMMIT, where the copies must already face this round's limit.
            let wrote = match (&amphora, &duck) {
                (Some(a), _) => {
                    room.give(other);
                    other = 0;
                    a.exec("BEGIN")?;
                    let written = a.exec(&fill(false, i, from, to)).and_then(|()| {
                        if let Some(free) = s.free_at(k, need) {
                            a.press(&room, &mut other, free)?;
                        }
                        a.exec("COMMIT")
                    });
                    if written.is_err() && !a.conn.get_auto_commit() {
                        a.exec("ROLLBACK")?;
                    }
                    written
                }
                (_, Some(d)) => d
                    .conn
                    .execute_batch(&format!("RESET memory_limit; {}", fill(true, i, from, to)))
                    .map_err(e),
                _ => Ok(()),
            };
            match wrote {
                Ok(()) => {
                    reference.exec(&fill(false, i, from, to))?;
                    if let Some(n) = next.get_mut(i) {
                        *n = from + s.grow;
                    }
                }
                Err(err) => {
                    t.writes_refused += 1;
                    t.write_refusal.get_or_insert_with(|| err.to_string());
                }
            }
        }
        let free = s.free_at(k, need);
        // The cap: the engine's bytes for its rows now, and the free bytes on top.
        if let (Some(a), Some(f)) = (&amphora, free) {
            a.press(&room, &mut other, f)?;
            let rows_bytes = room
                .held()
                .saturating_sub(other)
                .saturating_sub(a.storage.copy_meter().held);
            t.cap_peak = t.cap_peak.max(rows_bytes.saturating_add(f));
        }
        if let (Some(d), Some(f)) = (&duck, free) {
            let cap = d.memory()?.saturating_add(f);
            d.conn
                .execute_batch(&format!("SET memory_limit = '{cap}B'"))?;
            t.cap_peak = t.cap_peak.max(cap);
        }
        let q = s.query(k);
        let before = amphora.as_ref().map(|a| a.storage.copy_counts());
        let clock = Instant::now();
        let got = match (&amphora, &duck) {
            (Some(a), _) => a.rows(&q),
            (_, Some(d)) => d.rows(&q),
            _ => anyhow::bail!("no engine"),
        };
        let took = u64::try_from(clock.elapsed().as_nanos()).unwrap_or(u64::MAX);
        if let (Some(a), Some(before)) = (&amphora, before) {
            handoffs.record(&before, &a.storage.copy_counts());
        }
        match got {
            Ok(got) => {
                t.all.push(took);
                if reaches(cx.reach, k) {
                    t.latencies.push(took);
                }
                t.equal &= got == reference.rows(&q)?;
            }
            // Refused under the cap (DuckDB's out-of-memory): counted, not timed.
            Err(err) => {
                t.refused += 1;
                t.refusal.get_or_insert_with(|| err.to_string());
            }
        }
        if let Some(a) = &amphora {
            let (_, granted) = a.copies();
            t.analytic_peak = t.analytic_peak.max(granted);
            t.engine_peak = t.engine_peak.max(room.held().saturating_sub(other));
        }
        if let Some(d) = &duck {
            let held = d.memory()?;
            t.analytic_peak = t.analytic_peak.max(held);
            t.engine_peak = t.engine_peak.max(held);
            t.spill_peak = t.spill_peak.max(d.spilled()?);
        }
    }
    let elapsed = began.elapsed().as_secs_f64();
    room.give(other);
    t.latencies.sort_unstable();
    t.all.sort_unstable();
    let left_out = (0..s.statements).filter(|k| !reaches(cx.reach, *k)).count() as u64;
    // What Amphora's copies did and why (DuckDB has no copies: null).
    let copies = amphora.as_ref().map(|a| {
        let c = a.storage.copy_counts();
        // The statements its own counts left out: skipped, or never handed over.
        let skipped = handoffs.skipped.values().copied().fold(0u64, u64::saturating_add);
        let own = skipped.saturating_add((s.statements as u64).saturating_sub(handoffs.handed));
        let waiting = a.storage.copy_waiting();
        let last = a.storage.copy_dropped().map(
            |Dropped::Memory {
                 tree,
                 asked,
                 held,
                 limit,
             }| serde_json::json!({"why": "memory source refused", "tree": tree, "asked": asked, "held": held, "limit": limit}),
        );
        serde_json::json!({
            "kept": c.kept,
            "dropped": c.dropped,
            "rebuys": c.rebuys,
            "resumed": c.resumed,
            "build_rows": c.build_rows,
            "rebuy_rows": c.rebuy_rows,
            "copy_statements": c.copy,
            "absent_statements": c.absent,
            "waiting_end": waiting.len(),
            "waiting_bytes_end": waiting.iter().fold(0u64, |b, w| b.saturating_add(w.bytes)),
            "longest_wait_end_ns": waiting.iter().map(|w| w.nanos).max().unwrap_or(0),
            "last_dropped": last,
            "handed": handoffs.handed,
            "skipped": handoffs.skipped,
            "unserved": c.unserved,
            "probe_agrees": own == left_out,
        })
    });
    let (engine_name, policy, version) = match engine {
        Engine::Amphora(Rebuy::Rent) => (
            "amphora",
            Some("A_rent"),
            env!("CARGO_PKG_VERSION").to_string(),
        ),
        Engine::Amphora(Rebuy::Room) => (
            "amphora",
            Some("B_room"),
            env!("CARGO_PKG_VERSION").to_string(),
        ),
        Engine::Duck => {
            let d = duck.as_ref().ok_or_else(|| anyhow::anyhow!("no DuckDB"))?;
            (
                "duckdb",
                None,
                d.conn
                    .query_row("SELECT version()", [], |r| r.get::<_, String>(0))?,
            )
        }
    };
    Ok(serde_json::json!({
        "harness": "colcopy-duckdb",
        "engine": engine_name,
        "version": version,
        "policy": policy,
        "scenario": s.name,
        "forces": s.forces,
        "tables": s.tables,
        "rows": s.rows,
        "statements": s.statements,
        "need_bytes": need,
        "free_bytes": s.free(need),
        "p50_ns": at(&t.latencies, 0.5),
        "p99_ns": at(&t.latencies, 0.99),
        "p999_ns": at(&t.latencies, 0.999),
        "reached_statements": t.latencies.len(),
        "skipped_queries": skipped_queries(cx.reach),
        "all_p50_ns": at(&t.all, 0.5),
        "all_p99_ns": at(&t.all, 0.99),
        "all_p999_ns": at(&t.all, 0.999),
        "statements_per_s": s.statements as f64 / elapsed.max(f64::MIN_POSITIVE),
        "engine_bytes_peak": t.engine_peak,
        "analytic_bytes_peak": t.analytic_peak,
        "cap_bytes_peak": t.cap_peak,
        "spill_bytes_peak": t.spill_peak,
        "refused": t.refused,
        "refusal": t.refusal,
        "writes_refused": t.writes_refused,
        "write_refusal": t.write_refusal,
        "copies": copies,
        "equal": t.equal,
        "threads": match engine { Engine::Duck => cx.threads, Engine::Amphora(_) => 1 },
        "client": cx.client,
        "platform": cx.platform,
    }))
}

/// The concurrency ladder (Ada's, 2026-11-21): the clients each step runs at once.
const LADDER: [usize; 7] = [1, 8, 32, 128, 512, 2048, 4096];

/// One client's connection, on either engine.
enum Client {
    Amphora(Arc<amphora_engine::Connection>),
    Duck(duckdb::Connection),
}

/// What a client's thread opens its connection from: Amphora's database (the connection is made
/// on the client's own thread, as the engine's many-writer tests make theirs) or a DuckDB
/// connection cloned from the database's first.
enum Opener<'a> {
    Amphora(&'a Arc<Database>),
    Duck(duckdb::Connection),
}

impl Opener<'_> {
    fn open(self) -> anyhow::Result<Client> {
        Ok(match self {
            Opener::Amphora(db) => Client::Amphora(
                db.connect(ConnectOpts::new().with_versioned_storage())
                    .map_err(e)?,
            ),
            Opener::Duck(conn) => Client::Duck(conn),
        })
    }
}

/// The clients' start: every client waits here until all are made, so a step measures them at
/// once, not their ramp; called off when one can't be made, so none waits for ever.
struct Gate {
    open: std::sync::Mutex<Option<bool>>,
    turned: std::sync::Condvar,
}

impl Gate {
    fn set(&self, go: bool) {
        if let Ok(mut g) = self.open.lock() {
            *g = Some(go);
        }
        self.turned.notify_all();
    }

    /// Whether the step goes ahead.
    fn wait(&self) -> bool {
        let Ok(mut g) = self.open.lock() else {
            return false;
        };
        loop {
            let state = *g;
            match state {
                Some(go) => return go,
                None => match self.turned.wait(g) {
                    Ok(next) => g = next,
                    Err(_) => return false,
                },
            }
        }
    }
}

impl Client {
    /// A statement's rows as text, sorted, read as [`Amphora::rows`] and [`Duck::rows`] read
    /// them (the I/O and a sleep stepped alike, inside the timed region).
    fn rows(&self, sql: &str) -> anyhow::Result<Answer> {
        match self {
            Client::Amphora(conn) => {
                let mut stmt = conn.prepare(sql).map_err(e)?;
                let mut out = Vec::new();
                stmt.run_with_row_callback(|r| {
                    out.push(r.get_values().map(amphora_text).collect());
                    Ok(())
                })
                .map_err(e)?;
                out.sort_unstable();
                Ok(out)
            }
            Client::Duck(conn) => Duck::rows_on(conn, sql),
        }
    }
}

/// What a client brings back from a step.
#[derive(Default)]
struct Taken {
    /// The statements a copy could answer ([`reach`]), and every statement.
    latencies: Vec<u64>,
    all: Vec<u64>,
    refused: u64,
    refusal: Option<String>,
    unequal: u64,
    /// The client's own time outside the timed statements (comparing answers, counting,
    /// pressing): with the statements' time, whether the client is what bounds the step.
    client_ns: u64,
    statement_ns: u64,
}

/// The room, set by whichever client first finishes a statement that moves the scenario to a
/// room it hadn't: Amphora's other use (the A/B's `press`) or DuckDB's limit. An event (a
/// statement's end), never a timer.
struct Room {
    capped: Arc<Capped>,
    /// Amphora's other use's bytes, and the free bytes last set (`u64::MAX`: none yet).
    other: std::sync::Mutex<(u64, u64)>,
    last: std::sync::atomic::AtomicU64,
}

impl Room {
    /// Sets the room for `free` bytes if it isn't set for them yet: Amphora's through its
    /// `storage`'s copies, DuckDB's through `duck`, a connection of its database.
    fn set(
        &self,
        duck: Option<&duckdb::Connection>,
        storage: Option<&Storage>,
        free: Option<u64>,
    ) -> anyhow::Result<()> {
        use std::sync::atomic::Ordering;
        let Some(f) = free else { return Ok(()) };
        if self.last.load(Ordering::Acquire) == f {
            return Ok(());
        }
        let mut g = self
            .other
            .lock()
            .map_err(|_| anyhow::anyhow!("a client panicked"))?;
        if g.1 == f {
            return Ok(());
        }
        match (duck, storage) {
            (None, Some(st)) => {
                let want = f.saturating_sub(st.copy_meter().held);
                let now = self.capped.room().map_err(e)?;
                if now > want {
                    self.capped
                        .take("the driver's other use", now - want)
                        .map_err(e)?;
                    g.0 = g.0.saturating_add(now - want);
                } else {
                    let back = (want - now).min(g.0);
                    self.capped.give(back);
                    g.0 = g.0.saturating_sub(back);
                }
            }
            (Some(conn), _) => {
                let held = Duck::one_on(conn, MEMORY_SQL)?;
                conn.execute_batch(&format!("SET memory_limit = '{}B'", held.saturating_add(f)))?;
            }
            (None, None) => anyhow::bail!("no engine to set the room of"),
        }
        g.1 = f;
        self.last.store(f, Ordering::Release);
        Ok(())
    }
}

/// The bytes the process has held at most since the last [`rss_reset`]: Linux's `VmHWM`
/// (proc(5)); None elsewhere.
fn rss_peak() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib.saturating_mul(1024))
}

/// Resets Linux's `VmHWM` to the bytes resident now (proc(5), `clear_refs` value 5), so each
/// step's peak is its own.
fn rss_reset() {
    let _ = std::fs::write("/proc/self/clear_refs", "5");
}

/// One step of the ladder (lane colcopy-8, AC-CB.4.5): `clients` clients at once, each its own
/// connection and thread, started together, each running the scenario's statements from its
/// own place in them, through the one driver for every engine. The scenario's room moves with
/// the step's progress, one scenario statement for every `clients` statements finished; the
/// tables don't grow here (concurrent writers would give each statement its own snapshot, with
/// no one reference to compare it to), so every answer is checked against Amphora's row loop.
fn ladder_step(
    cx: &Context<'_>,
    s: &Scenario,
    engine: Engine,
    need: u64,
    clients: usize,
) -> anyhow::Result<serde_json::Value> {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    let reference = Amphora::open(None, None)?;
    let capped = Arc::new(Capped::new(u64::MAX / 2));
    let shared: Arc<dyn Source> = capped.clone();
    let (amphora, duck) = match engine {
        Engine::Amphora(rebuy) => (Some(Amphora::open(Some(shared), Some(rebuy))?), None),
        Engine::Duck => (None, Some(Duck::open(cx.dir, cx.threads)?)),
    };
    for i in 0..s.tables {
        reference.create(&schema(i))?;
        reference.exec(&fill(false, i, 1, s.rows))?;
        if let Some(a) = &amphora {
            a.create(&schema(i))?;
            a.exec(&fill(false, i, 1, s.rows))?;
        }
        if let Some(d) = &duck {
            d.conn.execute_batch(&schema(i))?;
            d.conn.execute_batch(&fill(true, i, 1, s.rows))?;
        }
    }
    // Every statement's answer, from the row loop, before the step.
    let mut answers: HashMap<String, Answer> = HashMap::new();
    for k in 0..s.statements {
        if let std::collections::hash_map::Entry::Vacant(slot) = answers.entry(s.query(k)) {
            let a = reference.rows(slot.key())?;
            slot.insert(a);
        }
    }
    let mut openers = Vec::with_capacity(clients);
    for _ in 0..clients {
        openers.push(match (&amphora, &duck) {
            (Some(a), _) => Opener::Amphora(&a.db),
            (_, Some(d)) => Opener::Duck(d.conn.try_clone()?),
            _ => anyhow::bail!("no engine"),
        });
    }
    let room = Room {
        capped: capped.clone(),
        other: std::sync::Mutex::new((0, u64::MAX)),
        last: AtomicU64::new(u64::MAX),
    };
    let storage = amphora.as_ref().map(|a| a.storage.clone());
    room.set(
        duck.as_ref().map(|d| &d.conn),
        storage.as_deref(),
        s.free_at(0, need),
    )?;
    let done = AtomicUsize::new(0);
    let gate = Gate {
        open: std::sync::Mutex::new(None),
        turned: std::sync::Condvar::new(),
    };
    rss_reset();
    let mut began = None::<Instant>;
    let taken = std::thread::scope(|sc| -> anyhow::Result<Vec<Taken>> {
        let mut handles = Vec::with_capacity(clients);
        for (c, opener) in openers.into_iter().enumerate() {
            let (room, storage, done, gate, answers, reach) =
                (&room, storage.as_deref(), &done, &gate, &answers, cx.reach);
            let spawned = std::thread::Builder::new()
                .name(format!("colcopy-client-{c}"))
                .spawn_scoped(sc, move || -> anyhow::Result<Taken> {
                    let mut t = Taken {
                        latencies: Vec::with_capacity(s.statements),
                        ..Taken::default()
                    };
                    let opened = opener.open();
                    if !gate.wait() {
                        return Ok(t);
                    }
                    let client = opened?;
                    let duck = match &client {
                        Client::Duck(conn) => Some(conn),
                        Client::Amphora(_) => None,
                    };
                    for j in 0..s.statements {
                        let k = (c + j) % s.statements.max(1);
                        let q = s.query(k);
                        let clock = Instant::now();
                        let got = client.rows(&q);
                        let took = u64::try_from(clock.elapsed().as_nanos()).unwrap_or(u64::MAX);
                        let own = Instant::now();
                        match got {
                            Ok(got) => {
                                t.all.push(took);
                                if reaches(reach, k) {
                                    t.latencies.push(took);
                                }
                                t.statement_ns = t.statement_ns.saturating_add(took);
                                if answers.get(&q) != Some(&got) {
                                    t.unequal += 1;
                                }
                            }
                            Err(err) => {
                                t.refused += 1;
                                t.refusal.get_or_insert_with(|| err.to_string());
                            }
                        }
                        let d = done.fetch_add(1, Ordering::AcqRel).saturating_add(1);
                        room.set(duck, storage, s.free_at(d / clients.max(1), need))?;
                        t.client_ns = t.client_ns.saturating_add(
                            u64::try_from(own.elapsed().as_nanos()).unwrap_or(u64::MAX),
                        );
                    }
                    Ok(t)
                });
            match spawned {
                Ok(h) => handles.push(h),
                Err(err) => {
                    gate.set(false);
                    return Err(anyhow::anyhow!("client {c} of {clients}: {err}"));
                }
            }
        }
        began = Some(Instant::now());
        gate.set(true);
        let mut out = Vec::with_capacity(handles.len());
        for h in handles {
            out.push(
                h.join()
                    .map_err(|_| anyhow::anyhow!("a client panicked"))??,
            );
        }
        Ok(out)
    })?;
    let elapsed = began.map_or(0.0, |b| b.elapsed().as_secs_f64());
    let rss = rss_peak();
    let mut latencies = Vec::with_capacity(clients.saturating_mul(s.statements));
    let mut all = Vec::with_capacity(clients.saturating_mul(s.statements));
    let (mut refused, mut refusal, mut unequal, mut client_ns, mut statement_ns) =
        (0u64, None::<String>, 0u64, 0u64, 0u64);
    for t in taken {
        latencies.extend(t.latencies);
        all.extend(t.all);
        refused += t.refused;
        unequal += t.unequal;
        client_ns = client_ns.saturating_add(t.client_ns);
        statement_ns = statement_ns.saturating_add(t.statement_ns);
        if refusal.is_none() {
            refusal = t.refusal;
        }
    }
    latencies.sort_unstable();
    all.sort_unstable();
    let analytic = match (&amphora, &duck) {
        (Some(a), _) => a.copies().1,
        (_, Some(d)) => d.memory()?,
        _ => 0,
    };
    if let Ok(g) = room.other.lock() {
        capped.give(g.0);
    }
    let total = clients.saturating_mul(s.statements);
    let (engine_name, policy) = match engine {
        Engine::Amphora(Rebuy::Rent) => ("amphora", Some("A_rent")),
        Engine::Amphora(Rebuy::Room) => ("amphora", Some("B_room")),
        Engine::Duck => ("duckdb", None),
    };
    Ok(serde_json::json!({
        "harness": "colcopy-ladder",
        "engine": engine_name,
        "policy": policy,
        "scenario": s.name,
        "forces": s.forces,
        "tables": s.tables,
        "rows": s.rows,
        "grow": 0,
        "clients": clients,
        "connections": clients,
        "client_threads": clients,
        "cores": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
        "statements": total,
        "need_bytes": need,
        "free_bytes": s.free(need),
        "p50_ns": at(&latencies, 0.5),
        "p99_ns": at(&latencies, 0.99),
        "p999_ns": at(&latencies, 0.999),
        "reached_statements": latencies.len(),
        "skipped_queries": skipped_queries(cx.reach),
        "all_p50_ns": at(&all, 0.5),
        "all_p99_ns": at(&all, 0.99),
        "all_p999_ns": at(&all, 0.999),
        "statements_per_s": total as f64 / elapsed.max(f64::MIN_POSITIVE),
        "analytic_bytes_end": analytic,
        "rss_peak_bytes": rss,
        "client_share": client_ns as f64 / (client_ns.saturating_add(statement_ns)).max(1) as f64,
        "refused": refused,
        "refusal": refusal,
        "unequal": unequal,
        "equal": unequal == 0,
        "threads": match engine { Engine::Duck => cx.threads, Engine::Amphora(_) => 1 },
        "client": cx.client,
        "platform": cx.platform,
    }))
}

fn host() -> String {
    ["/proc/sys/kernel/hostname", "/etc/hostname"]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .map(|h| h.trim().to_string())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .unwrap_or_else(|| "unknown".into())
}

pub fn main() -> anyhow::Result<()> {
    let (mut dir, mut scale, mut threads, mut out) = (
        std::path::PathBuf::new(),
        1i64,
        1usize,
        None::<std::path::PathBuf>,
    );
    let mut engines = vec!["rent".to_string(), "room".into(), "duckdb".into()];
    let (mut ladder, mut clients) = (false, LADDER.to_vec());
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut v = || {
            args.next()
                .ok_or_else(|| anyhow::anyhow!("{a} takes a value"))
        };
        match a.as_str() {
            "--colcopy" => {}
            "--dir" => dir = v()?.into(),
            "--scale" => scale = v()?.parse::<i64>()?.max(1),
            "--engines" => engines = v()?.split(',').map(str::to_owned).collect(),
            "--duckdb-threads" => threads = v()?.parse::<usize>()?.max(1),
            "--out" => out = Some(v()?.into()),
            "--ladder" => ladder = true,
            "--clients" => {
                clients = v()?
                    .split(',')
                    .map(|n| n.parse::<usize>().map(|n| n.max(1)))
                    .collect::<Result<_, _>>()?
            }
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    anyhow::ensure!(
        !dir.as_os_str().is_empty(),
        "--dir is required (DuckDB spills there)"
    );
    std::fs::create_dir_all(&dir)?;
    let engines = engines
        .iter()
        .map(|n| match n.as_str() {
            "rent" => Ok(Engine::Amphora(Rebuy::Rent)),
            "room" => Ok(Engine::Amphora(Rebuy::Room)),
            "duckdb" => Ok(Engine::Duck),
            other => Err(anyhow::anyhow!(
                "unknown engine {other}: rent, room or duckdb"
            )),
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let client = serde_json::json!({
        "driver": "amphora-bench --colcopy",
        "mode": "in-process, both engines through their Rust libraries, in memory",
        "timing": "Instant around prepare, run and every row read",
    });
    let platform = serde_json::json!({ "host": host(), "os": std::env::consts::OS, "arch": std::env::consts::ARCH });
    let all = scenarios(scale)
        .ok_or_else(|| anyhow::anyhow!("colcopy: a scenario's size at scale {scale} passes i64"))?;
    let probe = all.iter().map(|s| s.rows).min().unwrap_or(1);
    let reach = reach(probe)?;
    eprintln!("queries no copy can answer: {}", skipped_queries(&reach));
    let cx = Context {
        dir: &dir,
        threads,
        client: &client,
        platform: &platform,
        reach: &reach,
    };
    let mut lines = Vec::new();
    for s in all {
        let need = need(s.rows)?;
        for &engine in &engines {
            if ladder {
                for &n in &clients {
                    let line = ladder_step(&cx, &s, engine, need, n)?.to_string();
                    println!("{line}");
                    lines.push(line);
                }
                continue;
            }
            let traces = Traces::default();
            let mut line = traces.during(|| run(&cx, &s, engine, need))?;
            if let (Engine::Amphora(_), Some(o)) = (engine, line.as_object_mut()) {
                o.insert("traces".into(), serde_json::json!(traces.counts()));
                o.insert("traces_compiled".into(), Traces::compiled().into());
            }
            let line = line.to_string();
            println!("{line}");
            lines.push(line);
        }
    }
    if let Some(path) = out {
        let mut text = lines.join("\n");
        text.push('\n');
        std::fs::write(path, text)?;
    }
    Ok(())
}
