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
    pub fn open(dir: &Path, cache_kib: i64) -> anyhow::Result<Self> {
        use amphora_engine::{ConnectOpts, Database, DatabaseOpts, OpenOptions, PlatformIO, SqliteDialect};
        std::fs::create_dir_all(dir)?;
        let io: Arc<dyn amphora_engine::IO> = Arc::new(PlatformIO::new().map_err(|e| anyhow::anyhow!("{e}"))?);
        let path = dir.join("bench.db");
        let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
        let db = Database::open(io.clone(), path.to_str().unwrap_or_default(), options).map_err(|e| anyhow::anyhow!("{e}"))?;
        let conn = db.connect(ConnectOpts::new()).map_err(|e| anyhow::anyhow!("{e}"))?;
        let counted = db.connect(ConnectOpts::new()).map_err(|e| anyhow::anyhow!("{e}"))?;
        counted.set_row_counters(true);
        let mut a = Self { io, _db: db, conn, counted, cache_kib };
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
        let mut stmt = match conn.prepare(sql) {
            Ok(stmt) => stmt,
            Err(e) => return (Outcome::Failed(e.to_string()), Vec::new()),
        };
        let prepared = Instant::now();
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
            Ok(true) => Outcome::Done(Run { rows, first, prepare_ms: ms(prepared - began), exec_ms: ms(done - prepared) }),
            Ok(false) => Outcome::TimedOut,
            Err(e) => Outcome::Failed(e.to_string()),
        };
        (outcome, counts)
    }

    /// One run on the counting connection: how it ended and each counter's estimate and rows.
    pub fn counted(&mut self, sql: &str, deadline: Duration) -> (Outcome, Vec<Count>) {
        self.run_on(true, sql, deadline, |_| ())
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
        "amphora".into()
    }

    fn dialect(&self) -> Dialect {
        Dialect::Lite
    }

    fn about(&mut self) -> anyhow::Result<serde_json::Value> {
        Ok(serde_json::json!({ "engine": "amphora", "cache_kib": self.cache_kib, "threads": 1 }))
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
        conn.execute_batch(&format!("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA cache_size = -{cache_kib}"))?;
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
        Ok(serde_json::json!({ "engine": "sqlite", "version": version, "cache_kib": self.cache_kib, "threads": 1 }))
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
            Ok(Run { rows: n, first, prepare_ms: ms(prepared - began), exec_ms: ms(prepared.elapsed()) })
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
            Ok(Run { rows: n, first, prepare_ms: ms(prepared - began), exec_ms: ms(prepared.elapsed()) })
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
        let mut amphora = Amphora { io, _db: db, conn, counted, cache_kib: 0 };
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
                Outcome::Done(Run { rows: n, first, prepare_ms: 0.0, exec_ms: ms(took) })
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
