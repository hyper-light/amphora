//! The engines the planner bench runs the benchmarks' queries on, each behind [Target]: Amphora
//! (its counters read back beside its estimates), SQLite, DuckDB and PostgreSQL, the same
//! schema, rows, indexes and statistics in each, every query under a deadline.

use std::io::{BufRead, BufReader, Write};
use std::num::NonZero;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::answers::Answer;
use super::data::{Format, Records};

/// How a query ended.
pub enum Outcome {
    Done(Run),
    /// It ran past its deadline and was stopped.
    TimedOut,
    /// The engine refused it: its words.
    Failed(String),
}

/// One run of a query.
pub struct Run {
    pub rows: u64,
    /// The first row, its columns as text joined by ` | `.
    pub first: Option<String>,
    /// Parsing and planning (preparing the statement).
    pub prepare_ms: f64,
    /// Running the prepared statement to its last row.
    pub exec_ms: f64,
    /// Amphora's join search within `prepare_ms` (`amphora_engine::join_search_ns`); none for
    /// the other engines, or where the run's planning is elsewhere.
    pub search_ms: Option<f64>,
}

/// Which column types the schema gets: SQLite's affinities (Amphora, SQLite) or the SQL types
/// TPC-H names (DuckDB, PostgreSQL).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Lite,
    Full,
}

/// A query or schema in the bench's portable text, written for `dialect`: `{DEC}` and `{DATE}`
/// TPC-H's decimal and date, `YEAR(x)` a date's year.
pub fn for_dialect(sql: &str, dialect: Dialect) -> String {
    let (dec, date) = match dialect {
        Dialect::Lite => ("REAL", "TEXT"),
        Dialect::Full => ("DECIMAL(15,2)", "DATE"),
    };
    let mut out = sql.replace("{DEC}", dec).replace("{DATE}", date);
    while let Some(at) = out.find("YEAR(") {
        let Some(close) = out[at..].find(')').map(|c| at + c) else { break };
        let column = out[at + 5..close].to_owned();
        let year = match dialect {
            Dialect::Lite => format!("CAST(substr({column}, 1, 4) AS INTEGER)"),
            Dialect::Full => format!("extract(year from {column})"),
        };
        out.replace_range(at..=close, &year);
    }
    out
}

pub trait Target {
    fn name(&self) -> String;
    fn dialect(&self) -> Dialect;
    /// The engine's version and the settings the runs depend on.
    fn about(&mut self) -> anyhow::Result<serde_json::Value>;
    /// Statements that return no rows, `;`-separated.
    fn exec(&mut self, sql: &str) -> anyhow::Result<()>;
    /// `path`'s rows into `table`: how many.
    fn load(&mut self, table: &str, path: &Path, format: Format) -> anyhow::Result<u64>;
    /// Statistics for the planner.
    fn analyze(&mut self) -> anyhow::Result<()>;
    /// Whether `table` holds rows already (a database loaded before).
    fn loaded(&mut self, table: &str) -> anyhow::Result<bool>;
    /// One timed run of `sql`. `answer`, the first timed run's under `--answers`, gets each row as
    /// the engine returns it; without one nothing is added to what is timed, the row callback a
    /// no-op the compiler inlines away.
    fn run(&mut self, sql: &str, deadline: Duration, answer: Option<&mut Answer>) -> Outcome;
    /// Amphora, whose runs are counted too.
    fn amphora(&mut self) -> Option<&mut Amphora> {
        None
    }
    /// PL.13 (lane replan-9, a cold run): how long ago the engine's server started, in seconds;
    /// None for an engine embedded in this process, which starts with it.
    fn server_started_s(&mut self) -> Option<f64> {
        None
    }
    /// The scaling ladder (`--cores`, QT.6 lane qs6-12): lets the engine's next runs use at most
    /// `cores` cores, with its parallel execution on. False for an engine that runs on one core
    /// whatever it is told (SQLite) or a target that states its own (`duckdb`, one thread).
    fn set_cores(&mut self, _cores: usize) -> anyhow::Result<bool> {
        Ok(false)
    }
}

fn first_row(cells: impl Iterator<Item = String>) -> String {
    cells.collect::<Vec<_>>().join(" | ")
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Rows committed a transaction while loading.
const LOAD_BATCH: u64 = 100_000;

/// The declared types of `table`'s columns, as SQLite's `PRAGMA table_info` gives them.
fn kinds(types: &[String]) -> Vec<Kind> {
    types
        .iter()
        .map(|t| {
            let t = t.to_ascii_uppercase();
            if t.contains("INT") {
                Kind::Int
            } else if t.contains("REAL") || t.contains("DEC") || t.contains("FLOA") || t.contains("DOUB") {
                Kind::Real
            } else {
                Kind::Text
            }
        })
        .collect()
}

#[derive(Clone, Copy)]
enum Kind {
    Int,
    Real,
    Text,
}

// ---- Amphora ----

pub struct Amphora {
    io: Arc<dyn amphora_engine::IO>,
    _db: Arc<amphora_engine::Database>,
    conn: Arc<amphora_engine::Connection>,
    /// A connection whose statements count their rows (`Connection::set_row_counters`).
    counted: Arc<amphora_engine::Connection>,
    cache_kib: i64,
    /// QT.7: its scans decode each leaf once into column batches (`Connection::set_vectorized_scans`).
    vectorized: bool,
    /// PL.13 (lane replan-6): adaptive joins and re-planning on (`Connection::set_adaptive_joins`).
    adaptive: bool,
}

/// One table access's or join's estimate and count, as the record keeps it.
pub struct Count {
    pub plan: usize,
    pub kind: &'static str,
    pub table: Option<String>,
    pub position: Option<usize>,
    pub est_input: Option<f64>,
    pub est_output: Option<f64>,
    pub rows: u64,
}

impl Amphora {
    pub fn open(dir: &Path, cache_kib: i64, vectorized: bool, adaptive: bool) -> anyhow::Result<Self> {
        use amphora_engine::{ConnectOpts, Database, DatabaseOpts, OpenOptions, PlatformIO, SqliteDialect};
        std::fs::create_dir_all(dir)?;
        let io: Arc<dyn amphora_engine::IO> = Arc::new(PlatformIO::new().map_err(|e| anyhow::anyhow!("{e}"))?);
        let path = dir.join("bench.db");
        let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
        let db = Database::open(io.clone(), path.to_str().unwrap_or_default(), options).map_err(|e| anyhow::anyhow!("{e}"))?;
        let conn = db.connect(ConnectOpts::new()).map_err(|e| anyhow::anyhow!("{e}"))?;
        let counted = db.connect(ConnectOpts::new()).map_err(|e| anyhow::anyhow!("{e}"))?;
        counted.set_row_counters(true);
        conn.set_vectorized_scans(vectorized);
        counted.set_vectorized_scans(vectorized);
        conn.set_adaptive_joins(adaptive);
        counted.set_adaptive_joins(adaptive);
        // Lane G8: AMPHORA_REAL_SUMS=exact runs the SUM-over-REAL A/B's arm (b); unset or `serial`
        // is today's. One binary for both arms, so both plan from one calibration (RULES 5e).
        let sums = match std::env::var("AMPHORA_REAL_SUMS").as_deref() {
            Err(_) | Ok("serial") => amphora_engine::RealSums::Serial,
            Ok("exact") => amphora_engine::RealSums::Exact,
            Ok("roworder") => amphora_engine::RealSums::RowOrder,
            Ok(other) => anyhow::bail!("AMPHORA_REAL_SUMS takes serial, exact or roworder, not {other}"),
        };
        conn.set_real_sums(sums);
        counted.set_real_sums(sums);
        let mut a = Self { io, _db: db, conn, counted, cache_kib, vectorized, adaptive };
        for c in [a.conn.clone(), a.counted.clone()] {
            a.exec_on(&c, &format!("PRAGMA cache_size = -{cache_kib}"))?;
        }
        Ok(a)
    }

    fn exec_on(&mut self, conn: &Arc<amphora_engine::Connection>, sql: &str) -> anyhow::Result<()> {
        for one in sql.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            let mut stmt = conn.prepare(one).map_err(|e| anyhow::anyhow!("{e} (in: {one})"))?;
            self.drive(&mut stmt, None, |_| ())?;
        }
        Ok(())
    }

    /// Steps `stmt` to its end, each row to `each`; Ok(false) if `deadline` stopped it.
    fn drive(
        &self,
        stmt: &mut amphora_engine::Statement,
        deadline: Option<Instant>,
        mut each: impl FnMut(&amphora_engine::Row),
    ) -> anyhow::Result<bool> {
        use amphora_engine::StepResult;
        loop {
            let step = stmt.step();
            let late = deadline.is_some_and(|d| Instant::now() >= d);
            match step {
                Ok(StepResult::Row) => {
                    if let Some(row) = stmt.row() {
                        each(row);
                    }
                }
                Ok(StepResult::Done) => return Ok(true),
                Ok(StepResult::IO | StepResult::Yield) => self.io.step().map_err(|e| anyhow::anyhow!("{e}"))?,
                Ok(StepResult::Sleep { duration }) => {
                    std::thread::sleep(duration);
                    self.io.step().map_err(|e| anyhow::anyhow!("{e}"))?;
                }
                Ok(StepResult::Interrupt) if late => return Ok(false),
                Ok(StepResult::Interrupt) => anyhow::bail!("interrupted"),
                Ok(StepResult::Busy) => anyhow::bail!("busy"),
                Err(_) if late => return Ok(false),
                Err(e) => anyhow::bail!("{e}"),
            }
        }
    }

    /// One run, each row to `each` as well.
    fn run_on(&mut self, counted: bool, sql: &str, deadline: Duration, mut each: impl FnMut(&amphora_engine::Row)) -> (Outcome, Vec<Count>) {
        let conn = if counted { self.counted.clone() } else { self.conn.clone() };
        conn.set_query_timeout(deadline);
        let began = Instant::now();
        let searched = amphora_engine::join_search_ns();
        let mut stmt = match conn.prepare(sql) {
            Ok(stmt) => stmt,
            Err(e) => return (Outcome::Failed(e.to_string()), Vec::new()),
        };
        let prepared = Instant::now();
        let search_ms = amphora_engine::join_search_ns().saturating_sub(searched) as f64 / 1e6;
        let (mut rows, mut first) = (0u64, None);
        let ended = self.drive(&mut stmt, Some(began + deadline), |row| {
            if rows == 0 {
                first = Some(first_row(row.get_values().map(|v| v.to_string())));
            }
            rows += 1;
            each(row);
        });
        let done = Instant::now();
        conn.set_query_timeout(Duration::ZERO);
        let counts = stmt
            .row_counts()
            .into_iter()
            .map(|c| Count {
                plan: c.site.plan,
                kind: match c.site.kind {
                    amphora_engine::RowCountKind::Loops => "loops",
                    amphora_engine::RowCountKind::Prefix => "prefix",
                    amphora_engine::RowCountKind::HashBuild => "hash_build",
                },
                table: c.site.table,
                position: c.site.estimate.map(|e| e.position),
                est_input: c.site.estimate.map(|e| e.input_rows),
                est_output: c.site.estimate.map(|e| e.output_rows),
                rows: c.rows,
            })
            .collect();
        let outcome = match ended {
            Ok(true) => Outcome::Done(Run { rows, first, prepare_ms: ms(prepared - began), exec_ms: ms(done - prepared), search_ms: Some(search_ms) }),
            Ok(false) => Outcome::TimedOut,
            Err(e) => Outcome::Failed(e.to_string()),
        };
        (outcome, counts)
    }

    /// One run on the counting connection: how it ended and each counter's estimate and rows.
    pub fn counted(&mut self, sql: &str, deadline: Duration) -> (Outcome, Vec<Count>) {
        self.run_on(true, sql, deadline, |_| ())
    }

    /// PL.12: `sql` prepared once on the timed connection, to run again and again with
    /// [Amphora::run_prepared].
    pub fn prepare(&mut self, sql: &str) -> anyhow::Result<amphora_engine::Statement> {
        self.conn.prepare(sql).map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// PL.12: one run of `stmt`, prepared once, with `values` bound and the connection's plan
    /// cache mode `mode`, to its last row, then reset for its next run. `prepare_ms` is the
    /// binding: a run's planning, where it makes a custom plan, is in its `exec_ms`.
    pub fn run_prepared(&mut self, stmt: &mut amphora_engine::Statement, values: &[amphora_engine::Value], mode: amphora_engine::PlanCacheMode, deadline: Duration) -> Outcome {
        let conn = self.conn.clone();
        conn.set_plan_cache_mode(mode);
        conn.set_query_timeout(deadline);
        let began = Instant::now();
        for (i, value) in values.iter().enumerate() {
            let bound = NonZero::new(i + 1).ok_or_else(|| anyhow::anyhow!("parameter 0")).and_then(|n| stmt.bind_at(n, value.clone()).map_err(|e| anyhow::anyhow!("{e}")));
            if let Err(e) = bound {
                conn.set_query_timeout(Duration::ZERO);
                return Outcome::Failed(e.to_string());
            }
        }
        let bound = Instant::now();
        let (mut rows, mut first) = (0u64, None);
        let ended = self.drive(stmt, Some(began + deadline), |row| {
            if rows == 0 {
                first = Some(first_row(row.get_values().map(|v| v.to_string())));
            }
            rows += 1;
        });
        let done = Instant::now();
        conn.set_query_timeout(Duration::ZERO);
        let reset = stmt.reset().map_err(|e| anyhow::anyhow!("{e}"));
        match (ended, reset) {
            (Ok(true), Ok(())) => Outcome::Done(Run { rows, first, prepare_ms: ms(bound - began), exec_ms: ms(done - bound), search_ms: None }),
            (Ok(false), _) => Outcome::TimedOut,
            (Err(e), _) | (_, Err(e)) => Outcome::Failed(e.to_string()),
        }
    }

    /// EXPLAIN QUERY PLAN FORMAT=JSON's plan of `sql`.
    pub fn plan_json(&mut self, sql: &str) -> anyhow::Result<String> {
        let mut stmt = self.conn.prepare(format!("EXPLAIN QUERY PLAN FORMAT=JSON {sql}")).map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut out = String::new();
        self.drive(&mut stmt, None, |row| {
            for v in row.get_values() {
                out.push_str(&v.to_string());
            }
        })?;
        Ok(out)
    }
}

impl Target for Amphora {
    fn name(&self) -> String {
        match (self.vectorized, self.adaptive) {
            (true, _) => "amphora-vectorized".into(),
            (false, true) => "amphora-adaptive".into(),
            (false, false) => "amphora".into(),
        }
    }

    fn dialect(&self) -> Dialect {
        Dialect::Lite
    }

    fn about(&mut self) -> anyhow::Result<serde_json::Value> {
        Ok(serde_json::json!({ "engine": "amphora", "cache_kib": self.cache_kib, "threads": 1, "vectorized_scans": self.vectorized, "adaptive_joins": self.adaptive }))
    }

    fn exec(&mut self, sql: &str) -> anyhow::Result<()> {
        let conn = self.conn.clone();
        self.exec_on(&conn, sql)
    }

    fn load(&mut self, table: &str, path: &Path, format: Format) -> anyhow::Result<u64> {
        use amphora_engine::Value;
        let conn = self.conn.clone();
        let mut types = Vec::new();
        let mut info = conn.prepare(format!("PRAGMA table_info({table})")).map_err(|e| anyhow::anyhow!("{e}"))?;
        self.drive(&mut info, None, |row| types.push(row.get_values().nth(2).map(|v| v.to_string()).unwrap_or_default()))?;
        let kinds = kinds(&types);
        let marks = vec!["?"; kinds.len()].join(", ");
        let mut records = Records::new(BufReader::with_capacity(1 << 20, std::fs::File::open(path)?), format);
        self.exec("BEGIN")?;
        let mut insert = conn.prepare(format!("INSERT INTO {table} VALUES ({marks})")).map_err(|e| anyhow::anyhow!("{e}"))?;
        while records.next()? {
            anyhow::ensure!(records.fields.len() == kinds.len(), "{}: record {} has {} fields, {table} {} columns", path.display(), records.records, records.fields.len(), kinds.len());
            insert.reset().map_err(|e| anyhow::anyhow!("{e}"))?;
            for (i, (field, kind)) in records.fields.iter_mut().zip(&kinds).enumerate() {
                let value = match (field.take(), kind) {
                    (None, _) => Value::Null,
                    (Some(f), Kind::Int) => f.parse::<i64>().map(Value::from_i64).unwrap_or_else(|_| Value::from_text(f)),
                    (Some(f), Kind::Real) => f.parse::<f64>().map(Value::from_f64).unwrap_or_else(|_| Value::from_text(f)),
                    (Some(f), Kind::Text) => Value::from_text(f),
                };
                insert.bind_at(NonZero::new(i + 1).ok_or_else(|| anyhow::anyhow!("parameter 0"))?, value).map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            self.drive(&mut insert, None, |_| ())?;
            if records.records % LOAD_BATCH == 0 {
                self.exec("COMMIT")?;
                self.exec("BEGIN")?;
            }
        }
        drop(insert);
        self.exec("COMMIT")?;
        Ok(records.records)
    }

    fn analyze(&mut self) -> anyhow::Result<()> {
        self.exec("ANALYZE")?;
        self.exec("PRAGMA wal_checkpoint(TRUNCATE)")
    }

    fn loaded(&mut self, table: &str) -> anyhow::Result<bool> {
        let conn = self.conn.clone();
        let mut found = false;
        let Ok(mut stmt) = conn.prepare(format!("SELECT 1 FROM {table} LIMIT 1")) else { return Ok(false) };
        if self.drive(&mut stmt, None, |_| found = true).is_err() {
            return Ok(false);
        }
        Ok(found)
    }

    fn run(&mut self, sql: &str, deadline: Duration, answer: Option<&mut Answer>) -> Outcome {
        match answer {
            Some(answer) => self.run_on(false, sql, deadline, |row| answer.amphora(row)).0,
            None => self.run_on(false, sql, deadline, |_| ()).0,
        }
    }

    fn amphora(&mut self) -> Option<&mut Amphora> {
        Some(self)
    }

    fn set_cores(&mut self, cores: usize) -> anyhow::Result<bool> {
        // Morsel-driven parallel execution, a scan's LIMIT stopping its workers early too, at most
        // `cores` workers (the cost model picks how many up to them).
        self.conn.set_parallel_mode(amphora_engine::morsel::ParallelMode::On);
        self.conn.set_parallel_limit(true);
        self.conn.set_parallel_cores(Some(std::num::NonZeroUsize::new(cores).ok_or_else(|| anyhow::anyhow!("--cores are at least 1"))?));
        Ok(true)
    }
}

// ---- SQLite ----

pub struct Sqlite {
    conn: rusqlite::Connection,
    cache_kib: i64,
}

impl Sqlite {
    pub fn open(dir: &Path, cache_kib: i64) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let conn = rusqlite::Connection::open(dir.join("bench.db"))?;
        // A read-heavy SQLite in production maps its file and keeps temporaries in memory. The
        // mmap request is the largest an i64 holds, so SQLite clamps it to its own compiled
        // ceiling (SQLITE_MAX_MMAP_SIZE); `about` records the size it took (lane M2, 2026-11-23).
        conn.execute_batch(&format!("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA cache_size = -{cache_kib}; PRAGMA mmap_size = {}; PRAGMA temp_store = MEMORY", i64::MAX))?;
        Ok(Self { conn, cache_kib })
    }
}

impl Target for Sqlite {
    fn name(&self) -> String {
        "sqlite".into()
    }

    fn dialect(&self) -> Dialect {
        Dialect::Lite
    }

    fn about(&mut self) -> anyhow::Result<serde_json::Value> {
        let version: String = self.conn.query_row("SELECT sqlite_version()", [], |r| r.get(0))?;
        let mmap_size: i64 = self.conn.query_row("PRAGMA mmap_size", [], |r| r.get(0))?;
        let temp_store: i64 = self.conn.query_row("PRAGMA temp_store", [], |r| r.get(0))?;
        Ok(serde_json::json!({ "engine": "sqlite", "version": version, "cache_kib": self.cache_kib, "threads": 1, "mmap_size": mmap_size, "temp_store": temp_store }))
    }

    fn exec(&mut self, sql: &str) -> anyhow::Result<()> {
        Ok(self.conn.execute_batch(sql)?)
    }

    fn load(&mut self, table: &str, path: &Path, format: Format) -> anyhow::Result<u64> {
        use rusqlite::types::Value;
        let mut types = Vec::new();
        {
            let mut info = self.conn.prepare(&format!("PRAGMA table_info({table})"))?;
            let mut rows = info.query([])?;
            while let Some(r) = rows.next()? {
                types.push(r.get::<_, String>(2)?);
            }
        }
        let kinds = kinds(&types);
        let marks = vec!["?"; kinds.len()].join(", ");
        let mut records = Records::new(BufReader::with_capacity(1 << 20, std::fs::File::open(path)?), format);
        self.conn.execute_batch("BEGIN")?;
        {
            let mut insert = self.conn.prepare(&format!("INSERT INTO {table} VALUES ({marks})"))?;
            while records.next()? {
                anyhow::ensure!(records.fields.len() == kinds.len(), "{}: record {} has {} fields", path.display(), records.records, records.fields.len());
                let values: Vec<Value> = records
                    .fields
                    .iter_mut()
                    .zip(&kinds)
                    .map(|(field, kind)| match (field.take(), kind) {
                        (None, _) => Value::Null,
                        (Some(f), Kind::Int) => f.parse().map(Value::Integer).unwrap_or(Value::Text(f)),
                        (Some(f), Kind::Real) => f.parse().map(Value::Real).unwrap_or(Value::Text(f)),
                        (Some(f), Kind::Text) => Value::Text(f),
                    })
                    .collect();
                insert.execute(rusqlite::params_from_iter(values))?;
                if records.records % LOAD_BATCH == 0 {
                    self.conn.execute_batch("COMMIT; BEGIN")?;
                }
            }
        }
        self.conn.execute_batch("COMMIT")?;
        Ok(records.records)
    }

    fn analyze(&mut self) -> anyhow::Result<()> {
        Ok(self.conn.execute_batch("ANALYZE; PRAGMA wal_checkpoint(TRUNCATE)")?)
    }

    fn loaded(&mut self, table: &str) -> anyhow::Result<bool> {
        Ok(self.conn.query_row(&format!("SELECT 1 FROM {table} LIMIT 1"), [], |_| Ok(())).is_ok())
    }

    fn run(&mut self, sql: &str, deadline: Duration, answer: Option<&mut Answer>) -> Outcome {
        match answer {
            Some(answer) => self.timed(sql, deadline, |row| answer.sqlite(row)),
            None => self.timed(sql, deadline, |_| ()),
        }
    }
}

impl Sqlite {
    /// One timed run, each row to `each` as well.
    fn timed(&mut self, sql: &str, deadline: Duration, mut each: impl FnMut(&rusqlite::Row<'_>)) -> Outcome {
        let handle = self.conn.get_interrupt_handle();
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let watchdog = std::thread::spawn(move || {
            if stopped.recv_timeout(deadline) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
                handle.interrupt();
            }
        });
        let began = Instant::now();
        let until = began + deadline;
        let outcome = (|| -> rusqlite::Result<Run> {
            let mut stmt = self.conn.prepare(sql)?;
            let prepared = Instant::now();
            let columns = stmt.column_count();
            let mut rows = stmt.query([])?;
            let (mut n, mut first) = (0u64, None);
            while let Some(r) = rows.next()? {
                if n == 0 {
                    first = Some(first_row((0..columns).map(|i| r.get_ref(i).map(|v| match v {
                        rusqlite::types::ValueRef::Null => "NULL".to_owned(),
                        rusqlite::types::ValueRef::Integer(i) => i.to_string(),
                        rusqlite::types::ValueRef::Real(f) => f.to_string(),
                        rusqlite::types::ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
                        rusqlite::types::ValueRef::Blob(b) => format!("blob {}", b.len()),
                    }).unwrap_or_default())));
                }
                n += 1;
                each(r);
            }
            Ok(Run { rows: n, first, prepare_ms: ms(prepared - began), exec_ms: ms(prepared.elapsed()), search_ms: None })
        })();
        let _ = stop.send(());
        let _ = watchdog.join();
        match outcome {
            Ok(run) => Outcome::Done(run),
            Err(_) if Instant::now() >= until => Outcome::TimedOut,
            Err(e) => Outcome::Failed(e.to_string()),
        }
    }
}

// ---- DuckDB ----

pub struct Duck {
    conn: duckdb::Connection,
    threads: Option<usize>,
}

impl Duck {
    /// `threads`: `None` for DuckDB's own choice (every core).
    pub fn open(dir: &Path, threads: Option<usize>) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let conn = duckdb::Connection::open(dir.join("bench.duckdb"))?;
        if let Some(t) = threads {
            conn.execute_batch(&format!("SET threads = {t}"))?;
        }
        Ok(Self { conn, threads })
    }
}

impl Target for Duck {
    fn name(&self) -> String {
        match self.threads {
            Some(_) => "duckdb".into(),
            None => "duckdb-mt".into(),
        }
    }

    fn dialect(&self) -> Dialect {
        Dialect::Full
    }

    fn about(&mut self) -> anyhow::Result<serde_json::Value> {
        let version: String = self.conn.query_row("SELECT version()", [], |r| r.get(0))?;
        let threads: String = self.conn.query_row("SELECT current_setting('threads')::VARCHAR", [], |r| r.get(0))?;
        let memory: String = self.conn.query_row("SELECT current_setting('memory_limit')::VARCHAR", [], |r| r.get(0))?;
        Ok(serde_json::json!({ "engine": "duckdb", "version": version, "threads": threads, "memory_limit": memory }))
    }

    fn exec(&mut self, sql: &str) -> anyhow::Result<()> {
        Ok(self.conn.execute_batch(sql)?)
    }

    fn load(&mut self, table: &str, path: &Path, format: Format) -> anyhow::Result<u64> {
        let file = path.to_str().ok_or_else(|| anyhow::anyhow!("a path that isn't UTF-8"))?.replace('\'', "''");
        let options = match format {
            Format::Csv => "FORMAT csv, HEADER false, QUOTE '\"', ESCAPE '\\'",
            Format::Pipe => "FORMAT csv, HEADER false, DELIMITER '|', QUOTE ''",
        };
        let n = self.conn.execute(&format!("COPY {table} FROM '{file}' ({options})"), [])?;
        Ok(n as u64)
    }

    fn analyze(&mut self) -> anyhow::Result<()> {
        Ok(self.conn.execute_batch("ANALYZE; CHECKPOINT")?)
    }

    fn loaded(&mut self, table: &str) -> anyhow::Result<bool> {
        Ok(self.conn.query_row(&format!("SELECT 1 FROM {table} LIMIT 1"), [], |_| Ok(())).is_ok())
    }

    fn run(&mut self, sql: &str, deadline: Duration, answer: Option<&mut Answer>) -> Outcome {
        match answer {
            Some(answer) => self.timed(sql, deadline, |row| answer.duckdb(row)),
            None => self.timed(sql, deadline, |_| ()),
        }
    }

    fn set_cores(&mut self, cores: usize) -> anyhow::Result<bool> {
        // duckdb-mt only: `duckdb` is the one-thread baseline.
        if self.threads.is_some() {
            return Ok(false);
        }
        self.conn.execute_batch(&format!("SET threads = {cores}"))?;
        Ok(true)
    }
}

impl Duck {
    /// One timed run, each row to `each` as well.
    fn timed(&mut self, sql: &str, deadline: Duration, mut each: impl FnMut(&duckdb::Row<'_>)) -> Outcome {
        let sql = &quote_at(sql);
        let handle = self.conn.interrupt_handle();
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let watchdog = std::thread::spawn(move || {
            if stopped.recv_timeout(deadline) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
                handle.interrupt();
            }
        });
        let began = Instant::now();
        let outcome = (|| -> duckdb::Result<Run> {
            let mut stmt = self.conn.prepare(sql)?;
            let prepared = Instant::now();
            let mut rows = stmt.query([])?;
            let (mut n, mut first) = (0u64, None);
            while let Some(r) = rows.next()? {
                if n == 0 {
                    let columns = r.as_ref().column_count();
                    first = Some(first_row((0..columns).map(|i| r.get_ref(i).map(|v| format!("{:?}", v.to_owned())).unwrap_or_default())));
                }
                n += 1;
                each(r);
            }
            Ok(Run { rows: n, first, prepare_ms: ms(prepared - began), exec_ms: ms(prepared.elapsed()), search_ms: None })
        })();
        let late = began.elapsed() >= deadline;
        let _ = stop.send(());
        let _ = watchdog.join();
        match outcome {
            Ok(run) => Outcome::Done(run),
            Err(_) if late => Outcome::TimedOut,
            Err(e) => Outcome::Failed(e.to_string()),
        }
    }
}

/// DuckDB's parser keeps `at` for itself (`AT TIME ZONE`), which JOB's 15a to 15d name aka_title
/// by: the name quoted wherever it stands alone outside a string, so DuckDB runs the query it refuses as written
/// (its refusal is recorded too, from the run before this rewrite).
fn quote_at(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let bytes = sql.as_bytes();
    let word = |b: Option<&u8>| b.is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'"' || *b == b'\'');
    let mut i = 0;
    let mut literal = false;
    while i < bytes.len() {
        let here = &sql[i..];
        // A string literal is copied as it is (`''` within it closes and opens it again).
        if bytes.get(i) == Some(&b'\'') {
            literal = !literal;
        }
        if !literal && here.len() >= 2 && here[..2].eq_ignore_ascii_case("at") && !word(i.checked_sub(1).and_then(|p| bytes.get(p))) && !word(bytes.get(i + 2)) {
            out.push_str("\"at\"");
            i += 2;
            continue;
        }
        let c = here.chars().next().unwrap_or_default();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Do: quote JOB 15a's alias for DuckDB. Expect: `at` alone quoted, words holding it kept.
    #[test]
    fn quote_at_quotes_the_alias_alone() {
        assert_eq!(
            quote_at("SELECT MIN(at.title) FROM aka_title AS at WHERE at.movie_id = t.id AND t.note LIKE '%at%' AND rating > 2"),
            "SELECT MIN(\"at\".title) FROM aka_title AS \"at\" WHERE \"at\".movie_id = t.id AND t.note LIKE '%at%' AND rating > 2"
        );
    }

    /// An answer as a pass under `--answers` makes one.
    fn writer() -> Answer {
        use crate::planner::answers::{MAX_BYTES, MAX_ROWS};
        Answer::new(MAX_ROWS, MAX_BYTES)
    }

    /// A row of each kind of value TPC-H and JOB give, in the dialect Amphora and SQLite share, and
    /// the answer it is: each REAL its double's shortest text, as the writer writes it.
    const LITE_ROW: &str = "SELECT -2, 0.1 + 0.2, 323.0, 'a' || char(9) || 'b\\c', NULL, ''";
    const LITE_ANSWER: &[u8] = b"-2\t0.30000000000000004\t323.0\ta\\tb\\\\c\t\\N\t\n";

    /// Do: run a query on Amphora writing its answer (kiln plan 01a188991617756c84d73f0e9e100330,
    /// step 1 (f)). Expect: each value as the writer writes it, the run's rows counted as without.
    #[test]
    fn amphora_writes_its_answer() {
        use amphora_engine::{ConnectOpts, Database, DatabaseOpts, MemoryIO, OpenOptions, SqliteDialect};
        let io: Arc<dyn amphora_engine::IO> = Arc::new(MemoryIO::new());
        let db = Database::open(io.clone(), &format!("planner-answers-{}.db", std::process::id()), OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new())).unwrap();
        let (conn, counted) = (db.connect(ConnectOpts::new()).unwrap(), db.connect(ConnectOpts::new()).unwrap());
        let mut amphora = Amphora { io, _db: db, conn, counted, cache_kib: 0, vectorized: false, adaptive: false };
        let mut answer = writer();
        assert!(matches!(amphora.run(LITE_ROW, Duration::from_secs(60), Some(&mut answer)), Outcome::Done(Run { rows: 1, .. })));
        assert_eq!(answer.finish(), LITE_ANSWER);
    }

    /// Do: run the same query on SQLite writing its answer. Expect: Amphora's answer.
    #[test]
    fn sqlite_writes_its_answer() {
        let mut sqlite = Sqlite { conn: rusqlite::Connection::open_in_memory().unwrap(), cache_kib: 0 };
        let mut answer = writer();
        assert!(matches!(sqlite.run(LITE_ROW, Duration::from_secs(60), Some(&mut answer)), Outcome::Done(Run { rows: 1, .. })));
        assert_eq!(answer.finish(), LITE_ANSWER);
    }

    /// Do: run on DuckDB a query giving the kinds TPC-H gives there (DECIMAL, DATE, a sum's
    /// HUGEINT, DOUBLE, text, NULL), then a TIMESTAMP. Expect: the decimal's digits, the date as
    /// Amphora and SQLite keep it, the rest as their answers have them; the TIMESTAMP refused by its
    /// type's name.
    #[test]
    fn duckdb_writes_its_answer() {
        let mut duck = Duck { conn: duckdb::Connection::open_in_memory().unwrap(), threads: Some(1) };
        let mut answer = writer();
        let sql = "SELECT CAST(406181.0111 AS DECIMAL(38, 4)), DATE '1995-03-05', SUM(x), CAST(0.1 AS DOUBLE) + CAST(0.2 AS DOUBLE), 'a' || chr(9) || 'b', CAST(NULL AS INTEGER) FROM (VALUES (1), (2)) AS t(x)";
        assert!(matches!(duck.run(sql, Duration::from_secs(60), Some(&mut answer)), Outcome::Done(Run { rows: 1, .. })));
        assert_eq!(answer.finish(), b"406181.0111\t1995-03-05\t3\t0.30000000000000004\ta\\tb\t\\N\n");
        let mut answer = writer();
        assert!(matches!(duck.run("SELECT TIMESTAMP '1995-03-05 10:00:00'", Duration::from_secs(60), Some(&mut answer)), Outcome::Done(_)));
        assert_eq!(answer.finish(), b"\\!unwritten_value a Timestamp value, which the answer writer has no text for\n");
    }
}

// ---- PostgreSQL ----

pub struct Postgres {
    client: postgres::Client,
}

impl Postgres {
    pub fn open(dsn: &str) -> anyhow::Result<Self> {
        let mut client = postgres::Client::connect(dsn, postgres::NoTls)?;
        // One worker a query, as the embedded engines run: class for class (CLAUDE.md §7).
        client.batch_execute("SET max_parallel_workers_per_gather = 0")?;
        Ok(Self { client })
    }
}

impl Target for Postgres {
    fn name(&self) -> String {
        "postgres".into()
    }

    fn dialect(&self) -> Dialect {
        Dialect::Full
    }

    fn about(&mut self) -> anyhow::Result<serde_json::Value> {
        let row = self.client.query_one(
            "SELECT version(), current_setting('shared_buffers'), current_setting('work_mem'), current_setting('effective_cache_size'), current_setting('max_parallel_workers_per_gather')",
            &[],
        )?;
        Ok(serde_json::json!({
            "engine": "postgres", "version": row.get::<_, String>(0), "shared_buffers": row.get::<_, String>(1),
            "work_mem": row.get::<_, String>(2), "effective_cache_size": row.get::<_, String>(3),
            "max_parallel_workers_per_gather": row.get::<_, String>(4), "threads": 1,
        }))
    }

    fn exec(&mut self, sql: &str) -> anyhow::Result<()> {
        Ok(self.client.batch_execute(sql)?)
    }

    fn load(&mut self, table: &str, path: &Path, format: Format) -> anyhow::Result<u64> {
        let options = match format {
            Format::Csv => "FORMAT csv, ESCAPE '\\'",
            Format::Pipe => "FORMAT csv, DELIMITER '|', QUOTE e'\\x01'",
        };
        let mut writer = self.client.copy_in(&format!("COPY {table} FROM STDIN ({options})"))?;
        let mut input = BufReader::with_capacity(1 << 20, std::fs::File::open(path)?);
        loop {
            let buf = input.fill_buf()?;
            if buf.is_empty() {
                break;
            }
            writer.write_all(buf)?;
            let n = buf.len();
            input.consume(n);
        }
        Ok(writer.finish()?)
    }

    fn analyze(&mut self) -> anyhow::Result<()> {
        // Each alone: a batch of statements is one transaction, which VACUUM refuses.
        self.client.batch_execute("VACUUM ANALYZE")?;
        Ok(self.client.batch_execute("CHECKPOINT")?)
    }

    fn loaded(&mut self, table: &str) -> anyhow::Result<bool> {
        Ok(self.client.query_opt(&format!("SELECT 1 FROM {table} LIMIT 1"), &[]).ok().flatten().is_some())
    }

    fn server_started_s(&mut self) -> Option<f64> {
        self.client.query_one("SELECT extract(epoch FROM now() - pg_postmaster_start_time())::float8", &[]).ok().map(|row| row.get::<_, f64>(0))
    }

    fn run(&mut self, sql: &str, deadline: Duration, mut answer: Option<&mut Answer>) -> Outcome {
        if let Err(e) = self.client.batch_execute(&format!("SET statement_timeout = {}", deadline.as_millis())) {
            return Outcome::Failed(e.to_string());
        }
        let began = Instant::now();
        let outcome = self.client.simple_query(sql);
        let took = began.elapsed();
        let _ = self.client.batch_execute("SET statement_timeout = 0");
        match outcome {
            Ok(messages) => {
                let (mut n, mut first) = (0u64, None);
                for m in messages {
                    if let postgres::SimpleQueryMessage::Row(r) = m {
                        if n == 0 {
                            first = Some(first_row((0..r.len()).map(|i| r.get(i).unwrap_or("NULL").to_owned())));
                        }
                        n += 1;
                        // Every row came before `took` was read: the answer costs the run nothing.
                        if let Some(answer) = answer.as_deref_mut() {
                            answer.postgres(&r);
                        }
                    }
                }
                // The simple protocol plans and runs in one message: its time is all run.
                Outcome::Done(Run { rows: n, first, prepare_ms: 0.0, exec_ms: ms(took), search_ms: None })
            }
            Err(e) if e.code() == Some(&postgres::error::SqlState::QUERY_CANCELED) => Outcome::TimedOut,
            // The client's own words are "db error" for whatever the server said: its message too.
            Err(e) => Outcome::Failed(match e.as_db_error() {
                Some(db) => format!("{e}: {} ({})", db.message(), db.code().code()),
                None => format!("{e:?}"),
            }),
        }
    }
}

/// ClickHouse over its HTTP interface (`bench/servers/clickhouse.sh`), for the scaling ladder (QT.6
/// lane qs6-12, `clickhouse-mt`): the benchmark's tables as MergeTree tables in a database of its
/// own, each query run with `max_threads` the ladder's cores (none given: its own choice, every
/// core) and `max_execution_time` its deadline, its rows read back as TSV. A server is reached
/// only over the network, so its client is this process's HTTP client on the same machine.
pub struct ClickHouse {
    agent: ureq::Agent,
    url: String,
    database: String,
    threads: Option<usize>,
}

impl ClickHouse {
    pub fn open(url: &str, database: &str) -> anyhow::Result<Self> {
        let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
        let ch = Self { agent, url: format!("{}/", url.trim_end_matches('/')), database: database.to_owned(), threads: None };
        ch.post_text(&[], &format!("CREATE DATABASE IF NOT EXISTS {database}"))?;
        Ok(ch)
    }

    /// Posts `body` (the query itself when `query` is none) with `settings` in the URL, in the
    /// bench's database but for the statement that makes it; the response's body once it
    /// succeeded.
    fn post(&self, query: Option<&str>, in_database: bool, settings: &[(&str, String)], body: ureq::SendBody<'_>) -> anyhow::Result<String> {
        let mut request = self.agent.post(&self.url);
        if let Some(q) = query {
            request = request.query("query", q);
        }
        if in_database {
            request = request.query("database", self.database.as_str());
        }
        for (k, v) in settings {
            request = request.query(*k, v.as_str());
        }
        let mut response = request.send(body)?;
        let status = response.status();
        let text = response.body_mut().with_config().limit(u64::MAX).read_to_string()?;
        if !status.is_success() {
            anyhow::bail!("ClickHouse said {status}: {}", text.trim());
        }
        Ok(text)
    }

    /// Posts the statement `sql` as the body.
    fn post_text(&self, settings: &[(&str, String)], sql: &str) -> anyhow::Result<String> {
        let mut bytes = sql.as_bytes();
        self.post(None, !sql.starts_with("CREATE DATABASE"), settings, ureq::SendBody::from_reader(&mut bytes))
    }
}

/// A CREATE TABLE in ClickHouse's words: each column's type its nearest (INTEGER, BIGINT and
/// SMALLINT as Int64, DECIMAL as Decimal(15,2), DATE as Date, REAL and DOUBLE as Float64, every
/// character type as String), Nullable unless NOT NULL; a MergeTree table in the order its rows
/// came. Its keys and constraints are left out: MergeTree enforces none of them.
fn clickhouse_table(stmt: &str) -> Option<String> {
    let rest = stmt.trim().strip_prefix("CREATE TABLE ")?;
    let open = rest.find('(')?;
    let close = rest.rfind(')')?;
    let name = rest.get(..open)?.trim();
    let body = rest.get(open + 1..close)?;
    // The columns, split at the commas outside parentheses (DECIMAL(15,2) holds one).
    let (mut parts, mut depth, mut start) = (Vec::new(), 0i32, 0usize);
    for (at, c) in body.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(body.get(start..at)?);
                start = at + 1;
            }
            _ => {}
        }
    }
    parts.push(body.get(start..)?);
    let mut columns = Vec::new();
    for part in parts {
        let words: Vec<&str> = part.split_whitespace().collect();
        let (Some(column), Some(kind)) = (words.first(), words.get(1)) else { continue };
        let upper = column.to_ascii_uppercase();
        if matches!(upper.as_str(), "PRIMARY" | "FOREIGN" | "UNIQUE" | "CONSTRAINT" | "CHECK") {
            continue;
        }
        let kind = kind.to_ascii_lowercase();
        let ty = if kind.starts_with("int") || kind.starts_with("bigint") || kind.starts_with("smallint") {
            "Int64"
        } else if kind.starts_with("decimal") || kind.starts_with("numeric") {
            "Decimal(15,2)"
        } else if kind == "date" {
            "Date"
        } else if kind.starts_with("real") || kind.starts_with("double") || kind.starts_with("float") {
            "Float64"
        } else {
            "String"
        };
        let not_null = part.to_ascii_uppercase().contains("NOT NULL");
        columns.push(if not_null { format!("{column} {ty}") } else { format!("{column} Nullable({ty})") });
    }
    Some(format!("CREATE TABLE {name} ({}) ENGINE = MergeTree ORDER BY tuple()", columns.join(", ")))
}

impl Target for ClickHouse {
    fn name(&self) -> String {
        "clickhouse-mt".into()
    }

    fn dialect(&self) -> Dialect {
        Dialect::Full
    }

    fn about(&mut self) -> anyhow::Result<serde_json::Value> {
        let version = self.post_text(&[], "SELECT version() FORMAT TSV")?;
        Ok(serde_json::json!({ "engine": "clickhouse", "version": version.trim(), "client": "HTTP, ureq, one connection", "threads": self.threads }))
    }

    fn exec(&mut self, sql: &str) -> anyhow::Result<()> {
        for stmt in sql.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            let upper = stmt.to_ascii_uppercase();
            // MergeTree keeps no secondary index of this kind, and takes no statistics this way.
            if upper.starts_with("CREATE INDEX") || upper.starts_with("CREATE UNIQUE INDEX") || upper.starts_with("ANALYZE") || upper.starts_with("CHECKPOINT") {
                continue;
            }
            let stmt = if upper.starts_with("CREATE TABLE") { clickhouse_table(stmt).ok_or_else(|| anyhow::anyhow!("a CREATE TABLE ClickHouse's words can't say: {stmt}"))? } else { stmt.to_owned() };
            self.post_text(&[], &stmt)?;
        }
        Ok(())
    }

    fn load(&mut self, table: &str, path: &Path, format: Format) -> anyhow::Result<u64> {
        // ClickHouse's CSV reader takes no `\` escapes, and JOB's files escape with them (lane
        // qs6-13): each record is read by the reader Amphora's and SQLite's loads use, and sent
        // as ClickHouse's TabSeparated, whose escapes are exact.
        let file = std::fs::File::open(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        let mut rows = TabSeparated::new(Records::new(BufReader::with_capacity(1 << 20, file), format));
        let sent = self.post(Some(&format!("INSERT INTO {table} FORMAT TabSeparated")), true, &[], ureq::SendBody::from_reader(&mut rows));
        if let Some(e) = rows.failed.take() {
            return Err(e.context(format!("{}", path.display())));
        }
        sent?;
        let count = self.post_text(&[], &format!("SELECT count() FROM {table} FORMAT TSV"))?;
        Ok(count.trim().parse()?)
    }

    fn analyze(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn loaded(&mut self, table: &str) -> anyhow::Result<bool> {
        Ok(self.post_text(&[], &format!("SELECT 1 FROM {table} LIMIT 1 FORMAT TSV")).is_ok_and(|out| !out.trim().is_empty()))
    }

    fn run(&mut self, sql: &str, deadline: Duration, _answer: Option<&mut Answer>) -> Outcome {
        let mut settings = vec![("max_execution_time", deadline.as_secs().max(1).to_string())];
        if let Some(threads) = self.threads {
            settings.push(("max_threads", threads.to_string()));
        }
        let began = Instant::now();
        match self.post_text(&settings, &format!("{sql}\nFORMAT TSV")) {
            Ok(out) => {
                let exec_ms = ms(began.elapsed());
                let mut lines = out.lines();
                let first = lines.next().map(|line| first_row(line.split('\t').map(str::to_owned)));
                let rows = u64::from(first.is_some()) + lines.count() as u64;
                Outcome::Done(Run { rows, first, prepare_ms: 0.0, exec_ms, search_ms: None })
            }
            Err(e) if e.to_string().contains("TIMEOUT_EXCEEDED") => Outcome::TimedOut,
            Err(e) => Outcome::Failed(e.to_string()),
        }
    }

    fn set_cores(&mut self, cores: usize) -> anyhow::Result<bool> {
        self.threads = Some(cores);
        Ok(true)
    }
}

/// A table's records as ClickHouse's TabSeparated rows, streamed: a tab between fields, a newline
/// after each record, NULL as `\N`, and in a value `\`, tab, newline, carriage return and NUL
/// escaped with a backslash (ClickHouse's documented TabSeparated escapes, "Formats for Input
/// and Output Data", TabSeparated). A read error stops the stream at a record's end and is kept
/// in `failed`, since `Read` can't carry its words.
struct TabSeparated<R> {
    records: Records<R>,
    row: Vec<u8>,
    at: usize,
    failed: Option<anyhow::Error>,
}

impl<R: BufRead> TabSeparated<R> {
    fn new(records: Records<R>) -> Self {
        Self { records, row: Vec::new(), at: 0, failed: None }
    }

    /// The next record into `row`; false at the end of the file or on an error.
    fn fill(&mut self) -> bool {
        self.row.clear();
        self.at = 0;
        match self.records.next() {
            Ok(true) => {}
            Ok(false) => return false,
            Err(e) => {
                self.failed = Some(e);
                return false;
            }
        }
        for (i, field) in self.records.fields.iter().enumerate() {
            if i > 0 {
                self.row.push(b'\t');
            }
            match field {
                None => self.row.extend_from_slice(b"\\N"),
                Some(text) => {
                    for &byte in text.as_bytes() {
                        match byte {
                            b'\\' => self.row.extend_from_slice(b"\\\\"),
                            b'\t' => self.row.extend_from_slice(b"\\t"),
                            b'\n' => self.row.extend_from_slice(b"\\n"),
                            b'\r' => self.row.extend_from_slice(b"\\r"),
                            0 => self.row.extend_from_slice(b"\\0"),
                            other => self.row.push(other),
                        }
                    }
                }
            }
        }
        self.row.push(b'\n');
        true
    }
}

impl<R: BufRead> std::io::Read for TabSeparated<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.at >= self.row.len() && !self.fill() {
            return Ok(0);
        }
        let rest = self.row.get(self.at..).unwrap_or_default();
        let n = rest.len().min(buf.len());
        buf.get_mut(..n).unwrap_or_default().copy_from_slice(rest.get(..n).unwrap_or_default());
        self.at += n;
        Ok(n)
    }
}

#[cfg(test)]
mod clickhouse_tests {
    use super::super::data::{Format, Records};
    use super::{clickhouse_table, TabSeparated};
    use std::io::Read;

    /// Do: send JOB rows as COPY wrote them (a NULL, an empty quoted string, a quote and a
    /// backslash escaped with `\`, a tab and a newline inside quotes, a value ending in a
    /// backslash) as ClickHouse's TabSeparated. Expect: each value exact, NULL as `\N`, the
    /// backslash, tab and newline escaped, one line per record.
    #[test]
    fn job_csv_with_backslashes_goes_to_clickhouse_as_tab_separated() {
        let csv = "1,,\"\",\"a \\\"b\\\" \\\\ c\"\n2,\"tab\there\nnext\",\"ends \\\\\",x\n";
        let mut out = String::new();
        TabSeparated::new(Records::new(csv.as_bytes(), Format::Csv)).read_to_string(&mut out).unwrap();
        assert_eq!(out, "1\t\\N\t\ta \"b\" \\\\ c\n2\ttab\\there\\nnext\tends \\\\\tx\n");
    }

    /// Do: a record the reader refuses (the file ends inside quotes). Expect: the stream ends
    /// and keeps the reader's error.
    #[test]
    fn a_refused_record_stops_the_tab_separated_stream_with_its_error() {
        let mut rows = TabSeparated::new(Records::new("1,\"open\n".as_bytes(), Format::Csv));
        let mut out = String::new();
        rows.read_to_string(&mut out).unwrap();
        assert!(out.is_empty());
        assert!(rows.failed.is_some());
    }

    /// Do: translate TPC-H's partsupp (its decimal written for the full dialect, a two-column
    /// key) and JOB's aka_name. Expect: each column's nearest ClickHouse type, Nullable unless NOT
    /// NULL, the key constraint left out, a MergeTree table.
    #[test]
    fn create_tables_in_clickhouses_words() {
        assert_eq!(
            clickhouse_table("CREATE TABLE partsupp (ps_partkey INTEGER NOT NULL, ps_suppkey INTEGER NOT NULL, ps_availqty INTEGER NOT NULL, ps_supplycost DECIMAL(15,2) NOT NULL, ps_comment VARCHAR(199) NOT NULL, PRIMARY KEY (ps_partkey, ps_suppkey))").as_deref(),
            Some("CREATE TABLE partsupp (ps_partkey Int64, ps_suppkey Int64, ps_availqty Int64, ps_supplycost Decimal(15,2), ps_comment String) ENGINE = MergeTree ORDER BY tuple()")
        );
        assert_eq!(
            clickhouse_table("CREATE TABLE aka_name (\n    id integer NOT NULL PRIMARY KEY,\n    person_id integer NOT NULL,\n    name text,\n    md5sum character varying(65)\n)").as_deref(),
            Some("CREATE TABLE aka_name (id Int64, person_id Int64, name Nullable(String), md5sum Nullable(String)) ENGINE = MergeTree ORDER BY tuple()")
        );
        assert_eq!(clickhouse_table("CREATE INDEX x ON t (a)"), None);
    }
}
