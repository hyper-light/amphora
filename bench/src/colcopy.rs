//! Column copies within a fixed memory limit against DuckDB (kiln goal c436eeb5, lane colcopy-5,
//! AC-CB.4.2): the six scenarios of the rebuy policies' A/B (`columns_sql::tests::ab`, T-CB.4.1)
//! run by one driver over Amphora under each policy and over DuckDB, both in this process through
//! their Rust libraries (`amphora-engine` and the `duckdb` crate, bundled), in memory, the same
//! statements on the same rows, timed the same way (each statement's prepare, run and every row
//! read, by one `Instant`), under the same memory cap (docs/design/columnar.md, decision 2).
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
//! equalled Amphora's row loop's, and the client and platform that produced it.
//!
//! usage: amphora-bench --colcopy --dir DIR [--scale K] [--engines rent,room,duckdb]
//!        [--duckdb-threads N] [--out FILE]
//!
//! `--duckdb-threads` defaults to 1, the threads one Amphora statement runs on.

use std::sync::Arc;
use std::time::Instant;

use amphora_engine::storage::versioned::columns_sql::Rebuy;
use amphora_engine::storage::versioned::memory::{Capped, Source};
use amphora_engine::storage::versioned::switch::Storage;
use amphora_engine::{ConnectOpts, Database, MemoryIO, Numeric, SqliteDialect, StepResult, Value};

fn e(err: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("{err}")
}

/// The A/B's analytic statements (`columns_sql::tests::QUERIES`), on table `t`.
const QUERIES: [&str; 7] = [
    "SELECT count(*), sum(v), min(v), max(v) FROM t",
    "SELECT count(s), min(s), max(s) FROM t WHERE v >= 500",
    "SELECT g, count(*), sum(v) FROM t GROUP BY g",
    "SELECT s, count(*), max(id) FROM t WHERE g <> 3 GROUP BY s",
    "SELECT g, s, sum(v) FROM t WHERE s IS NOT NULL GROUP BY g, s",
    "SELECT count(*) FROM t WHERE s IS NULL",
    "SELECT min(id), max(id), sum(id) FROM t WHERE v < 100",
];

/// How a scenario presses the room, as the A/B defines it.
#[derive(Clone, Copy)]
enum Press {
    Steady { copies: f64 },
    Bursty { piece: f64 },
    Grows,
    None,
}

struct Scenario {
    name: &'static str,
    tables: usize,
    rows: i64,
    statements: usize,
    press: Press,
    grow: i64,
}

/// The A/B's six scenarios at scale `k` (`columns_sql::tests::ab::scenarios`).
fn scenarios(k: i64) -> Vec<Scenario> {
    let n = |s: i64| usize::try_from(s).unwrap_or(1);
    let s = |name, tables, rows, statements, press, grow| Scenario {
        name,
        tables,
        rows,
        statements: n(statements),
        press,
        grow,
    };
    vec![
        s(
            "laptop_cap_one_big_table",
            1,
            2000 * k,
            12 * k,
            Press::Steady { copies: 0.5 },
            0,
        ),
        s(
            "many_tables_small_cap",
            6,
            300 * k,
            24 * k,
            Press::Steady { copies: 2.0 },
            0,
        ),
        s(
            "steady_pressure",
            2,
            500 * k,
            16 * k,
            Press::Steady { copies: 1.5 },
            0,
        ),
        s(
            "bursty_pressure",
            2,
            500 * k,
            16 * k,
            Press::Bursty { piece: 0.125 },
            0,
        ),
        s(
            "growing_while_waiting",
            1,
            500 * k,
            16 * k,
            Press::Grows,
            20 * k,
        ),
        s("huge_cap_many_tables", 12, 300 * k, 36 * k, Press::None, 0),
    ]
}

fn schema(i: usize) -> String {
    format!("CREATE TABLE t{i} (id INTEGER PRIMARY KEY, g INTEGER, v INTEGER, s TEXT)")
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
    _db: Arc<Database>,
    conn: Arc<amphora_engine::Connection>,
    storage: Arc<Storage>,
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
        storage.set_copies_on(copies.is_some());
        if let Some(rebuy) = copies {
            storage.set_copy_rebuy(rebuy);
        }
        Ok(Self {
            io,
            _db: db,
            conn,
            storage,
        })
    }

    fn exec(&self, sql: &str) -> anyhow::Result<()> {
        self.conn.execute(sql).map_err(e)
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
                StepResult::IO | StepResult::Yield => self.io.step().map_err(e)?,
                other => anyhow::bail!("step: {other:?}"),
            }
        }
        out.sort_unstable();
        Ok(out)
    }

    /// The copies' resident and granted bytes.
    fn copies(&self) -> (u64, u64) {
        self.storage.copy_held()
    }
}

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
        use duckdb::types::ValueRef;
        let mut stmt = self.conn.prepare(sql)?;
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
        let n: i64 = self.conn.query_row(sql, [], |r| r.get(0))?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// The bytes DuckDB's buffer manager holds now, every tag.
    fn memory(&self) -> anyhow::Result<u64> {
        self.one("SELECT coalesce(sum(memory_usage_bytes), 0)::BIGINT FROM duckdb_memory()")
    }

    /// The bytes it has spilled to its temporary files now.
    fn spilled(&self) -> anyhow::Result<u64> {
        self.one("SELECT coalesce(sum(size), 0)::BIGINT FROM duckdb_temporary_files()")
    }
}

/// The bytes granted the copy of one table of `rows` rows, with room to spare: the A/B's `need`.
fn need(rows: i64) -> anyhow::Result<u64> {
    let a = Amphora::open(None, Some(Rebuy::Rent))?;
    a.exec(&schema(0))?;
    a.exec(&fill(false, 0, 1, rows))?;
    let q = QUERIES[2].replace("FROM t", "FROM t0");
    for _ in 0..3 {
        a.rows(&q)?;
    }
    Ok(a.copies().1)
}

/// The free bytes the scenario leaves the analytic work at statement `k` (None: no cap).
fn free_at(s: &Scenario, k: usize, need: u64) -> Option<u64> {
    let share = |f: f64| (need as f64 * f) as u64;
    match s.press {
        Press::Steady { copies } => Some(share(copies)),
        Press::Bursty { piece } if k % 2 == 0 => Some(share(0.5).saturating_add(share(piece))),
        Press::Bursty { .. } => Some(share(0.5)),
        Press::Grows if k >= s.statements / 2 => Some(share(0.5).saturating_add(share(1.0))),
        Press::Grows => Some(share(0.5)),
        Press::None => None,
    }
}

fn at(sorted: &[u64], p: f64) -> u64 {
    let n = sorted.len();
    let i = ((p * n as f64).ceil() as usize).clamp(1, n.max(1)) - 1;
    sorted.get(i).copied().unwrap_or(0)
}

#[derive(Default)]
struct Tally {
    latencies: Vec<u64>,
    engine_peak: u64,
    analytic_peak: u64,
    cap_peak: u64,
    spill_peak: u64,
    refused: u64,
    /// The first refusal's text.
    refusal: Option<String>,
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
        reference.exec(&schema(i))?;
        reference.exec(&fill(false, i, 1, s.rows))?;
        if let Some(a) = &amphora {
            a.exec(&schema(i))?;
            a.exec(&fill(false, i, 1, s.rows))?;
        }
        if let Some(d) = &duck {
            d.conn.execute_batch(&schema(i))?;
            d.conn.execute_batch(&fill(true, i, 1, s.rows))?;
        }
    }
    let mut t = Tally {
        equal: true,
        ..Tally::default()
    };
    let mut other = 0u64;
    let mut next = vec![s.rows + 1; s.tables];
    let began = Instant::now();
    for k in 0..s.statements {
        let i = k % s.tables;
        if s.grow > 0 {
            let from = next.get(i).copied().unwrap_or(s.rows + 1);
            reference.exec(&fill(false, i, from, from + s.grow - 1))?;
            if let Some(a) = &amphora {
                a.exec(&fill(false, i, from, from + s.grow - 1))?;
            }
            if let Some(d) = &duck {
                d.conn
                    .execute_batch(&fill(true, i, from, from + s.grow - 1))?;
            }
            if let Some(n) = next.get_mut(i) {
                *n = from + s.grow;
            }
        }
        let free = free_at(s, k, need);
        // The cap: the engine's bytes for its rows now, and the free bytes on top.
        if let (Some(a), Some(f)) = (&amphora, free) {
            let want = f.saturating_sub(a.storage.copy_meter().held);
            let now = room.room().map_err(e)?;
            if now > want {
                room.take("the driver's other use", now - want).map_err(e)?;
                other = other.saturating_add(now - want);
            } else {
                let back = (want - now).min(other);
                room.give(back);
                other = other.saturating_sub(back);
            }
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
        let q = QUERIES
            .get(k % QUERIES.len())
            .copied()
            .unwrap_or(QUERIES[0])
            .replace("FROM t", &format!("FROM t{i}"));
        let clock = Instant::now();
        let got = match (&amphora, &duck) {
            (Some(a), _) => a.rows(&q),
            (_, Some(d)) => d.rows(&q),
            _ => anyhow::bail!("no engine"),
        };
        let took = u64::try_from(clock.elapsed().as_nanos()).unwrap_or(u64::MAX);
        match got {
            Ok(got) => {
                t.latencies.push(took);
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
        "tables": s.tables,
        "rows": s.rows,
        "statements": s.statements,
        "need_bytes": need,
        "free_bytes": free_at(s, 0, need),
        "p50_ns": at(&t.latencies, 0.5),
        "p99_ns": at(&t.latencies, 0.99),
        "p999_ns": at(&t.latencies, 0.999),
        "statements_per_s": s.statements as f64 / elapsed.max(f64::MIN_POSITIVE),
        "engine_bytes_peak": t.engine_peak,
        "analytic_bytes_peak": t.analytic_peak,
        "cap_bytes_peak": t.cap_peak,
        "spill_bytes_peak": t.spill_peak,
        "refused": t.refused,
        "refusal": t.refusal,
        "equal": t.equal,
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
    let cx = Context {
        dir: &dir,
        threads,
        client: &client,
        platform: &platform,
    };
    let mut lines = Vec::new();
    for s in scenarios(scale) {
        let need = need(s.rows)?;
        for &engine in &engines {
            let line = run(&cx, &s, engine, need)?.to_string();
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
