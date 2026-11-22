//! Row skipping against DuckDB (kiln goal 623adc1c, lane skip-7; docs/design/executor.md decision
//! 11): the scenarios that force each skipping kind's failure mode, run by one driver over Amphora
//! under each `PRAGMA skipping` setting and over DuckDB, both in this process through their Rust
//! libraries (`amphora-engine` and the `duckdb` crate, bundled), in memory, the same prepared
//! statements on the same rows with the same bound values, timed the same way (one `Instant`
//! around each statement's bind, run and every row read).
//!
//! The table: `t(id INTEGER PRIMARY KEY, a INTEGER, b INTEGER, pad TEXT)`, `--rows` rows. A
//! scenario is a column shape, a filter width and the writes between filters:
//!
//! - shape `ordered`: `a` follows `id` (time-ordered data, zone maps' ground); `scattered`: `a` is
//!   `id` times a prime modulo the rows (every leaf spans every value: zone maps' failure mode);
//! - width `narrow`: a thousandth of the values; `wide`: a quarter (cracking hands back as many
//!   rowids as match: its failure mode with write churn);
//! - writes: each of `--writes` updates between filters, each setting one row's `a` to a value
//!   of the scenario's shape. Lane skip-9 (T-SKU.11): the default is a write-rate ladder, 0 and
//!   then 1, 8, 32, 128, 512, 2048, 4096 writes a filter (the brief's ladder), so the writes that
//!   land during a dropped kind's build climb from none to many times the table's rows.
//!
//! Lane skip-9 (T-SKU.11): `capped` runs under each build mode for a dropped kind, as
//! `capped:inline` (skip-7's, on the scan's path: the control), `capped:void` (skip-8's, voided by
//! a write during it) and `capped:replay` (the writes during it applied before it installs, up to
//! the rows the summary holds). Each line reports the summary's build counts: builds installed
//! and voided, writes replayed, replay records that reached their bound, the largest record, and
//! rows read for builds on a scan's path.
//!
//! The scenario's numbers are its definition (what is being forced), not tuned engine settings.
//! Bounds and writes come from one seeded generator per scenario (splitmix64, Steele, Lea and Flood,
//! OOPSLA 2014), so every engine sees the same statements. Answers are checked against the first
//! engine run on the scenario.
//!
//! One JSON line per engine, setting and scenario: filter latency p50, p99, p99.9 (ns), write
//! latency p50, p99, p99.9 (ns), filters a second, rows read (Amphora's statement counter; DuckDB
//! reports none), whether every answer equalled the first engine's, and the client and platform
//! that produced it.
//!
//! ClickHouse is not in this driver: it has no in-process library here, and a number from another
//! client (its HTTP server against an in-process Amphora) would be experiment error (the fair
//! comparison rule). Its comparison needs both engines behind the same server client.
//!
//! usage: amphora-bench --skipping [--rows N] [--filters F] [--writes 0,1,8,32,128,512,2048,4096]
//!        [--engines off,zone_maps,cracking,measured,capped:inline,capped:void,capped:replay,duckdb]
//!        [--duckdb-threads N] [--seed S] [--out FILE]
//!
//! `--duckdb-threads` defaults to 1, the threads one Amphora statement runs on.

use std::num::NonZero;
use std::sync::Arc;
use std::time::Instant;

use amphora_engine::{ConnectOpts, Database, MemoryIO, Numeric, SqliteDialect, StepResult, Value};

fn e(err: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("{err}")
}

/// The filter every engine runs, prepared once and bound before each run.
const FILTER: &str = "SELECT count(*), sum(b) FROM t WHERE a BETWEEN ?1 AND ?2";
/// The write between filters, prepared once.
const WRITE: &str = "UPDATE t SET a = ?1 WHERE id = ?2";

/// splitmix64: a seeded stream, the same for every engine.
struct Stream(u64);

impl Stream {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0..n` (n > 0).
    fn below(&mut self, n: i64) -> i64 {
        let n = u64::try_from(n.max(1)).unwrap_or(1);
        i64::try_from(self.next() % n).unwrap_or(0)
    }
}

#[derive(Clone, Copy)]
enum Shape {
    Ordered,
    Scattered,
}

impl Shape {
    fn name(self) -> &'static str {
        match self {
            Shape::Ordered => "ordered",
            Shape::Scattered => "scattered",
        }
    }

    /// Row `id`'s value of `a` in a table of `rows` rows. 7919 is the 1000th prime: coprime to
    /// any row count it doesn't divide, so the scattered column is a permutation of the ids.
    fn value(self, id: i64, rows: i64, s: &mut Stream) -> i64 {
        match self {
            Shape::Ordered => id.saturating_add(s.below(17)).saturating_sub(8),
            Shape::Scattered => id.wrapping_mul(7919).rem_euclid(rows.max(1)),
        }
    }
}

struct Scenario {
    shape: Shape,
    wide: bool,
    writes: usize,
}

impl Scenario {
    fn name(&self) -> String {
        format!(
            "{}-{}-w{}",
            self.shape.name(),
            if self.wide { "wide" } else { "narrow" },
            self.writes
        )
    }

    fn width(&self, rows: i64) -> i64 {
        if self.wide {
            rows / 4
        } else {
            (rows / 1000).max(1)
        }
    }
}

fn pct(sorted: &[u64], p: f64) -> u64 {
    let n = sorted.len();
    let i = ((p * n as f64).ceil() as usize).clamp(1, n.max(1)) - 1;
    sorted.get(i).copied().unwrap_or(0)
}

fn int(v: i64) -> Value {
    Value::Numeric(Numeric::Integer(v))
}

/// One in-memory Amphora database on the pager's B-tree (skipping summarizes its leaves).
struct Amphora {
    io: Arc<dyn amphora_engine::IO>,
    _db: Arc<Database>,
    conn: Arc<amphora_engine::Connection>,
}

impl Amphora {
    fn open() -> anyhow::Result<Self> {
        let io: Arc<dyn amphora_engine::IO> = Arc::new(MemoryIO::new());
        let db = Database::open_file(io.clone(), ":memory:", Arc::new(SqliteDialect)).map_err(e)?;
        let conn = db.connect(ConnectOpts::new()).map_err(e)?;
        conn.set_vectorized_scans(true);
        Ok(Self { io, _db: db, conn })
    }

    /// Runs `stmt` bound to `values`; its rows' integers, and the rows its scan read.
    fn run(
        &self,
        stmt: &mut amphora_engine::Statement,
        values: &[i64],
    ) -> anyhow::Result<(Vec<Option<i64>>, u64)> {
        stmt.reset().map_err(e)?;
        stmt.reset_metrics();
        for (i, v) in values.iter().enumerate() {
            let index = NonZero::new(i + 1).ok_or_else(|| anyhow::anyhow!("parameter 0"))?;
            stmt.bind_at(index, int(*v)).map_err(e)?;
        }
        let mut out = Vec::new();
        loop {
            match stmt.step().map_err(e)? {
                StepResult::Row => {
                    if let Some(r) = stmt.row() {
                        out.extend(r.get_values().map(|v| match v {
                            Value::Numeric(Numeric::Integer(i)) => Some(*i),
                            _ => None,
                        }));
                    }
                }
                StepResult::Done => break,
                StepResult::IO | StepResult::Yield => self.io.step().map_err(e)?,
                other => anyhow::bail!("step: {other:?}"),
            }
        }
        Ok((out, stmt.metrics().rows_read))
    }
}

/// What a run measured.
#[derive(Default)]
struct Tally {
    filters: Vec<u64>,
    writes: Vec<u64>,
    rows_read: Option<u64>,
    answers: Vec<Vec<Option<i64>>>,
    seconds: f64,
    /// Lane skip-9 (T-SKU.11): the summary's build counts at the run's end (Amphora only).
    builds: Option<amphora_engine::skipping::SkipCounts>,
}

fn fill_sql(shape: Shape, rows: i64, s: &mut Stream) -> Vec<String> {
    (1..=rows)
        .map(|id| {
            let a = shape.value(id, rows, s);
            format!(
                "INSERT INTO t VALUES ({id}, {a}, {}, '{}')",
                id % 7,
                "x".repeat(40)
            )
        })
        .collect()
}

const SCHEMA: &str = "CREATE TABLE t (id INTEGER PRIMARY KEY, a INTEGER, b INTEGER, pad TEXT)";

/// The scenario's stream of (writes, bounds) for each filter.
fn plan(
    sc: &Scenario,
    rows: i64,
    filters: usize,
    seed: u64,
) -> (Vec<String>, Vec<(Vec<(i64, i64)>, (i64, i64))>) {
    let mut s = Stream(seed);
    let fill = fill_sql(sc.shape, rows, &mut s);
    let width = sc.width(rows);
    let steps = (0..filters)
        .map(|_| {
            let writes = (0..sc.writes)
                .map(|_| {
                    let id = s.below(rows).saturating_add(1);
                    (sc.shape.value(id, rows, &mut s), id)
                })
                .collect();
            let lo = s.below(rows);
            (writes, (lo, lo.saturating_add(width)))
        })
        .collect();
    (fill, steps)
}

fn run_amphora(
    setting: &str,
    fill: &[String],
    steps: &[(Vec<(i64, i64)>, (i64, i64))],
) -> anyhow::Result<Tally> {
    let db = Amphora::open()?;
    db.conn.execute(SCHEMA).map_err(e)?;
    db.conn.execute("BEGIN").map_err(e)?;
    for sql in fill {
        db.conn.execute(sql).map_err(e)?;
    }
    db.conn.execute("COMMIT").map_err(e)?;
    if setting != "off" {
        // `capped:void` is the setting `capped void`.
        let setting = setting.replace(':', " ");
        db.conn
            .execute(format!("PRAGMA skipping = 't.a {setting}'"))
            .map_err(e)?;
    }
    let mut filter = db.conn.prepare(FILTER).map_err(e)?;
    let mut write = db.conn.prepare(WRITE).map_err(e)?;
    let mut t = Tally {
        rows_read: Some(0),
        ..Tally::default()
    };
    let began = Instant::now();
    for (writes, (lo, hi)) in steps {
        for (a, id) in writes {
            let clock = Instant::now();
            db.run(&mut write, &[*a, *id])?;
            t.writes
                .push(u64::try_from(clock.elapsed().as_nanos()).unwrap_or(u64::MAX));
        }
        let clock = Instant::now();
        let (answer, read) = db.run(&mut filter, &[*lo, *hi])?;
        t.filters
            .push(u64::try_from(clock.elapsed().as_nanos()).unwrap_or(u64::MAX));
        t.rows_read = t.rows_read.map(|r| r.saturating_add(read));
        t.answers.push(answer);
    }
    t.seconds = began.elapsed().as_secs_f64();
    t.builds = amphora_engine::skipping::skipping_counts(&db.conn, "t");
    Ok(t)
}

/// A DuckDB integer as the driver compares it (its `sum` of integers is a HUGEINT).
fn duck_int(v: duckdb::types::ValueRef<'_>) -> anyhow::Result<Option<i64>> {
    use duckdb::types::ValueRef;
    Ok(match v {
        ValueRef::Null => None,
        ValueRef::Int(n) => Some(i64::from(n)),
        ValueRef::BigInt(n) => Some(n),
        ValueRef::HugeInt(n) => Some(i64::try_from(n)?),
        other => anyhow::bail!(
            "a {} value, which the driver doesn't compare",
            other.data_type()
        ),
    })
}

fn run_duck(
    threads: usize,
    fill: &[String],
    steps: &[(Vec<(i64, i64)>, (i64, i64))],
) -> anyhow::Result<Tally> {
    let conn = duckdb::Connection::open_in_memory()?;
    conn.execute_batch(&format!("SET threads = {threads}"))?;
    conn.execute_batch(SCHEMA)?;
    conn.execute_batch("BEGIN")?;
    for sql in fill {
        conn.execute_batch(sql)?;
    }
    conn.execute_batch("COMMIT")?;
    let mut filter = conn.prepare(FILTER)?;
    let mut write = conn.prepare(WRITE)?;
    let mut t = Tally::default();
    let began = Instant::now();
    for (writes, (lo, hi)) in steps {
        for (a, id) in writes {
            let clock = Instant::now();
            write.execute(duckdb::params![*a, *id])?;
            t.writes
                .push(u64::try_from(clock.elapsed().as_nanos()).unwrap_or(u64::MAX));
        }
        let clock = Instant::now();
        let mut answer = Vec::new();
        let mut found = filter.query(duckdb::params![*lo, *hi])?;
        while let Some(r) = found.next()? {
            for i in 0..r.as_ref().column_count() {
                answer.push(duck_int(r.get_ref(i)?)?);
            }
        }
        drop(found);
        t.filters
            .push(u64::try_from(clock.elapsed().as_nanos()).unwrap_or(u64::MAX));
        t.answers.push(answer);
    }
    t.seconds = began.elapsed().as_secs_f64();
    Ok(t)
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
    let (mut rows, mut filters, mut threads, mut seed, mut out) = (
        100_000i64,
        400usize,
        1usize,
        1u64,
        None::<std::path::PathBuf>,
    );
    let mut writes = vec![0usize, 1, 8, 32, 128, 512, 2048, 4096];
    let known = [
        "off",
        "zone_maps",
        "cracking",
        "measured",
        "capped",
        "capped:inline",
        "capped:void",
        "capped:replay",
        "duckdb",
    ];
    let mut engines: Vec<String> = known
        .iter()
        .filter(|engine| **engine != "capped")
        .map(|s| s.to_string())
        .collect();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut v = || {
            args.next()
                .ok_or_else(|| anyhow::anyhow!("{a} takes a value"))
        };
        match a.as_str() {
            "--skipping" => {}
            "--rows" => rows = v()?.parse::<i64>()?.max(1),
            "--filters" => filters = v()?.parse::<usize>()?.max(1),
            "--writes" => writes = v()?.split(',').map(str::parse).collect::<Result<_, _>>()?,
            "--engines" => engines = v()?.split(',').map(str::to_owned).collect(),
            "--duckdb-threads" => threads = v()?.parse::<usize>()?.max(1),
            "--seed" => seed = v()?.parse()?,
            "--out" => out = Some(v()?.into()),
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    for engine in &engines {
        anyhow::ensure!(
            known.contains(&engine.as_str()),
            "unknown engine {engine}: one of {}",
            known.join(", ")
        );
    }
    let client = serde_json::json!({
        "driver": "amphora-bench --skipping",
        "mode": "in-process, both engines through their Rust libraries, in memory, one connection",
        "timing": "Instant around each statement's bind, run and every row read",
    });
    let platform = serde_json::json!({ "host": host(), "os": std::env::consts::OS, "arch": std::env::consts::ARCH });
    let mut lines = Vec::new();
    for shape in [Shape::Ordered, Shape::Scattered] {
        for wide in [false, true] {
            for &w in &writes {
                let sc = Scenario {
                    shape,
                    wide,
                    writes: w,
                };
                let (fill, steps) = plan(&sc, rows, filters, seed);
                let mut first: Option<Vec<Vec<Option<i64>>>> = None;
                for engine in &engines {
                    let mut t = match engine.as_str() {
                        "duckdb" => run_duck(threads, &fill, &steps)?,
                        setting => run_amphora(setting, &fill, &steps)?,
                    };
                    let equal = match &first {
                        Some(answers) => *answers == t.answers,
                        None => {
                            first = Some(std::mem::take(&mut t.answers));
                            true
                        }
                    };
                    t.filters.sort_unstable();
                    t.writes.sort_unstable();
                    let name = if engine == "duckdb" {
                        "duckdb"
                    } else {
                        "amphora"
                    };
                    let rate = if t.seconds > 0.0 {
                        filters as f64 / t.seconds
                    } else {
                        0.0
                    };
                    let line = serde_json::json!({
                        "bench": "skipping",
                        "scenario": sc.name(),
                        "engine": name,
                        "setting": engine,
                        "rows": rows,
                        "filters": filters,
                        "filter_p50_ns": pct(&t.filters, 0.50),
                        "filter_p99_ns": pct(&t.filters, 0.99),
                        "filter_p999_ns": pct(&t.filters, 0.999),
                        "write_p50_ns": pct(&t.writes, 0.50),
                        "write_p99_ns": pct(&t.writes, 0.99),
                        "write_p999_ns": pct(&t.writes, 0.999),
                        "filters_per_s": rate,
                        "rows_read": t.rows_read,
                        "builds": t.builds.map(|c| serde_json::json!({
                            "installed": c.builds_installed,
                            "voided": c.builds_invalidated,
                            "replayed_writes": c.replayed_writes,
                            "replay_overflows": c.replay_overflows,
                            "replay_peak": c.replay_peak,
                            "inline_rows_read": c.inline_rows_read,
                        })),
                        "equal": equal,
                        "client": client,
                        "platform": platform,
                    })
                    .to_string();
                    println!("{line}");
                    lines.push(line);
                }
            }
        }
    }
    if let Some(path) = out {
        let mut text = lines.join("\n");
        text.push('\n');
        std::fs::write(path, text)?;
    }
    Ok(())
}
