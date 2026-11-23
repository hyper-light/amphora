//! Amphora as a corpus engine: its engine crate on a database of its own per file, opened as SQLite
//! opens one (no store settings: the corpus's answers are a default SQLite connection's), and the
//! renderer of REALs, SQLite's text through Amphora's own port of it. The gate's database is in
//! memory; [`Amphora::on_disk`] puts it on a file on the disk under test, with the commit bench's
//! durability (`bench/src/powercut.rs`: WAL, `synchronous = FULL`), so the storages' times are a
//! disk's (T-T1.4.1).

use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, IO, LimboError, MemoryIO, Numeric,
    OpenOptions, PlatformIO, SqliteDialect, Statement, StepResult,
};

use crate::answer::{Render, Value};
use crate::run::{Engine, Stop};

/// Names every in-memory database the process opens apart (the engine keeps databases by name).
static OPENED: AtomicU64 = AtomicU64::new(0);

fn fresh_name() -> String {
    let n = OPENED.fetch_add(1, Ordering::Relaxed);
    format!("corpus-{}-{n}.db", std::process::id())
}

fn fresh(io: &Arc<dyn IO>, name: &str) -> Result<(Arc<Database>, Arc<Connection>), String> {
    let db = Database::open(
        io.clone(),
        name,
        OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new()),
    )
    .map_err(|e| e.to_string())?;
    let conn = db.connect(ConnectOpts::new()).map_err(|e| e.to_string())?;
    Ok((db, conn))
}

fn value(v: &amphora_engine::Value) -> Value {
    use amphora_engine::Value as V;
    match v {
        V::Null => Value::Null,
        V::Numeric(Numeric::Integer(i)) => Value::Integer(*i),
        V::Numeric(Numeric::Float(f)) => Value::Real(f64::from(*f)),
        V::Text(_) => Value::Text(v.to_text().unwrap_or_default().as_bytes().to_vec()),
        V::Blob(_) => Value::Blob(v.to_blob().unwrap_or_default().to_vec()),
    }
}

fn stop(e: LimboError) -> Stop {
    match e {
        LimboError::Interrupt => Stop::PastBudget,
        e => Stop::Refused(e.to_string()),
    }
}

/// Steps `stmt` to its end, keeping at most `keep` rows.
fn drain(io: &dyn IO, stmt: &mut Statement, keep: usize) -> Result<Vec<Vec<Value>>, Stop> {
    let mut rows = Vec::new();
    loop {
        match stmt.step().map_err(stop)? {
            StepResult::Row => {
                if rows.len() < keep
                    && let Some(r) = stmt.row()
                {
                    rows.push(r.get_values().map(value).collect());
                }
            }
            StepResult::Done => return Ok(rows),
            StepResult::Interrupt => return Err(Stop::PastBudget),
            StepResult::IO | StepResult::Yield => io.step().map_err(stop)?,
            other => return Err(Stop::Refused(format!("step: {other:?}"))),
        }
    }
}

/// Amphora on an in-memory database (the gate's engine), or on a file ([`Amphora::on_disk`]).
/// Fields drop in order: the connection and the database close before their files go.
pub struct Amphora {
    io: Arc<dyn IO>,
    conn: Arc<Connection>,
    _db: Arc<Database>,
    _files: Option<Files>,
}

impl Amphora {
    pub fn in_memory() -> Result<Self, String> {
        let io: Arc<dyn IO> = Arc::new(MemoryIO::new());
        let (db, conn) = fresh(&io, &fresh_name())?;
        Ok(Self {
            io,
            conn,
            _db: db,
            _files: None,
        })
    }

    /// A fresh database file in `dir` (the disk under test), with the commit bench's durability:
    /// `PRAGMA journal_mode = WAL` and `PRAGMA synchronous = FULL`, as `amphora-bench --powercut
    /// bench` sets them, so a commit returns once it is on the disk. The storage is the process's
    /// (`AMPHORA_STORAGE`); the versioned one keeps its files in `<file>-versioned`. Its files are
    /// removed when it drops, so a run's disk holds one file's database at a time.
    pub fn on_disk(dir: &Path) -> Result<Self, String> {
        let path = dir.join(fresh_name());
        let name = path
            .to_str()
            .ok_or_else(|| format!("a path that isn't UTF-8: {}", path.display()))?
            .to_owned();
        let files = Files(path);
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().map_err(|e| e.to_string())?);
        let (db, conn) = fresh(&io, &name)?;
        let mut a = Self {
            io,
            conn,
            _db: db,
            _files: Some(files),
        };
        for pragma in ["PRAGMA journal_mode = WAL", "PRAGMA synchronous = FULL"] {
            a.run(pragma, 1, None)
                .map_err(|e| format!("{pragma}: {e:?}"))?;
        }
        Ok(a)
    }
}

/// A database file's path: the file, its log, its shared memory and the versioned storage's
/// directory are removed when it drops (whichever of them exist).
struct Files(PathBuf);

impl Drop for Files {
    fn drop(&mut self) {
        let base = self.0.as_os_str().to_owned();
        for end in ["", "-wal", "-shm"] {
            let mut p = base.clone();
            p.push(end);
            let _ = std::fs::remove_file(&p);
        }
        let mut v = base;
        v.push("-versioned");
        let _ = std::fs::remove_dir_all(&v);
    }
}

impl Engine for Amphora {
    /// The bound is the engine's own query deadline: its VM checks it as it steps and interrupts
    /// the statement (vdbe `maybe_request_interrupt`), so a record that can't finish ends there.
    fn run(
        &mut self,
        sql: &str,
        keep: usize,
        until: Option<Instant>,
    ) -> Result<Vec<Vec<Value>>, Stop> {
        let mut stmt = self.conn.prepare(sql).map_err(stop)?;
        if let Some(u) = until {
            stmt.set_query_timeout_override(Some(Some(
                u.saturating_duration_since(Instant::now()),
            )));
        }
        drain(self.io.as_ref(), &mut stmt, keep)
    }

    /// `PRAGMA wal_checkpoint(TRUNCATE)`: every frame copied and the log cut to nothing, or the
    /// row it answers says busy (its first column non-zero).
    fn settle(&mut self, until: Option<Instant>) -> Result<(), Stop> {
        let rows = self.run("PRAGMA wal_checkpoint(TRUNCATE)", 1, until)?;
        match rows.first().and_then(|r| r.first()) {
            Some(Value::Integer(0)) | None => Ok(()),
            Some(other) => Err(Stop::Refused(format!(
                "the checkpoint answered busy: {other:?}"
            ))),
        }
    }
}

/// REALs rendered by Amphora's port of SQLite's (printf's etFLOAT for R, `%!.17g` for T), each
/// through a statement prepared once.
pub struct AmphoraRender {
    io: Arc<dyn IO>,
    real3: Statement,
    text: Statement,
    _conn: Arc<Connection>,
    _db: Arc<Database>,
}

impl AmphoraRender {
    pub fn new() -> Result<Self, String> {
        let io: Arc<dyn IO> = Arc::new(MemoryIO::new());
        let (db, conn) = fresh(&io, &fresh_name())?;
        let real3 = conn
            .prepare("SELECT printf('%.3f', ?1)")
            .map_err(|e| e.to_string())?;
        let text = conn
            .prepare("SELECT CAST(?1 AS TEXT)")
            .map_err(|e| e.to_string())?;
        Ok(Self {
            io,
            real3,
            text,
            _conn: conn,
            _db: db,
        })
    }

    fn one(io: &dyn IO, stmt: &mut Statement, r: f64) -> Result<String, String> {
        // NaN is no SQLite value (sqlite3_bind_double and every result store it as NULL, SQLite's
        // "Floating Point Numbers", https://www.sqlite.org/floatingpoint.html), so no SQLite answer
        // holds it; a competitor's REAL can (DuckDB's power(-1, 0.5)). It renders as its own text,
        // which no answer of SQLite's is: the answer disagrees and the run goes on.
        if r.is_nan() {
            return Ok("NaN".to_owned());
        }
        stmt.reset().map_err(|e| e.to_string())?;
        let first = NonZero::new(1usize).ok_or("parameter 0")?;
        stmt.bind_at(first, amphora_engine::Value::from_f64(r))
            .map_err(|e| e.to_string())?;
        let rows = drain(io, stmt, 1).map_err(|e| format!("{e:?}"))?;
        match rows.first().and_then(|row| row.first()) {
            Some(Value::Text(t)) => String::from_utf8(t.clone()).map_err(|e| e.to_string()),
            other => Err(format!("rendering {r}: {other:?}")),
        }
    }
}

impl Render for AmphoraRender {
    fn real3(&mut self, r: f64) -> Result<String, String> {
        Self::one(self.io.as_ref(), &mut self.real3, r)
    }

    fn real_text(&mut self, r: f64) -> Result<String, String> {
        Self::one(self.io.as_ref(), &mut self.text, r)
    }
}
