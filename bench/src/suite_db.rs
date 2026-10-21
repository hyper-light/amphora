//! The suite's databases (`crate::suite`), each as it ships or as Amphora runs: each opens fresh,
//! takes the load in transactions, and gives each client a session of its own that runs a
//! statement's parts and takes every column of every row it answers. A write is acknowledged as each
//! database acknowledges a durable commit:
//! - **amphora**: reads on the store's readers, on what is committed; writes handed to its writer
//!   (one group: the suite's data isn't split by key), committed with whatever else came, and
//!   answered once its flusher made them durable. AMPHORA_PACK_LOG=1 packs its log;
//!   AMPHORA_READER_PAGES sets each reader's page budget; AMPHORA_SHARED_PAGES=<pages> shares
//!   pages among its connections;
//! - **turso**: upstream Turso at 3e09a82 with its defaults, a connection a client;
//! - **sqlite**: SQLite in WAL with `synchronous = FULL`, a connection a client;
//! - **postgres**, **orioledb**, **cedardb**: a connection a client, as `pg.rs` sets each up;
//! - **mysql**: InnoDB, a pooled connection a client;
//! - **clickhouse**: over HTTP, as it ships; parameters inlined as literals (its HTTP interface's
//!   typed parameters need a type each), no transactions.

use std::collections::HashMap;
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::suite::{Dialect, Kind, P};

/// A database the suite runs against.
pub trait Target: Send + Sync {
    fn dialect(&self) -> Dialect;
    /// `stmts`, each with its parameters, in one transaction, durably: the tables, the load, the
    /// indexes.
    fn setup(&self, stmts: &[(String, Vec<P>)]) -> anyhow::Result<()>;
    /// The load and the indexes are in: the runs begin.
    fn ready(&self) -> anyhow::Result<()> {
        Ok(())
    }
    /// A client's session.
    fn session(&self) -> anyhow::Result<Box<dyn Session>>;
    /// What the database says of itself after the runs.
    fn stats(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
}

/// A client's connection.
pub trait Session: Send {
    /// `stmts` with `params` (a list each): a read's rows taken whole, how many came; a write's
    /// statements in one transaction, returned once durable.
    fn run(&mut self, kind: Kind, stmts: &[String], params: &[Vec<P>]) -> anyhow::Result<usize>;
    /// The plan the database makes for `sql` with `params` (its EXPLAIN), a line a step.
    fn explain(&mut self, sql: &str, params: &[P]) -> anyhow::Result<Vec<String>> {
        let _ = (sql, params);
        anyhow::bail!("no EXPLAIN here")
    }
}

pub fn open(target: &str, dir: &str, dsn: Option<&str>, clients: usize) -> anyhow::Result<Arc<dyn Target>> {
    let dsn = |local: &str| dsn.unwrap_or(local).to_owned();
    Ok(match target {
        "amphora" => Arc::new(Amphora::open(dir, clients)?),
        "turso" => Arc::new(Turso::open(dir)?),
        "sqlite" => Arc::new(Sqlite::open(dir)?),
        "postgres" => Arc::new(Pg::open(&dsn("postgres://postgres@127.0.0.1:55432/postgres"), PgKind::Postgres)?),
        "orioledb" => Arc::new(Pg::open(&dsn("postgres://postgres@127.0.0.1:55433/postgres"), PgKind::Orioledb)?),
        "cedardb" => Arc::new(Pg::open(&dsn("postgres://postgres:amphorabenchlocalonly2026@127.0.0.1:55434/postgres"), PgKind::Cedardb)?),
        "mysql" => Arc::new(MySql::open(&dsn("mysql://root@127.0.0.1:33306/convo"), clients + 4)?),
        "clickhouse" => Arc::new(ClickHouse::open(&dsn("http://127.0.0.1:58123"), clients + 4)?),
        t => anyhow::bail!("the suite has no target {t} (amphora, turso, sqlite, postgres, orioledb, cedardb, mysql, clickhouse)"),
    })
}

fn fresh(dir: &str) -> anyhow::Result<PathBuf> {
    let dir = Path::new(dir);
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir)?;
    Ok(dir.join("suite.db"))
}

const WAIT: Duration = Duration::from_secs(120);

// ---------------------------------------------------------------------------------------------
// Amphora

mod amphora_target {
    use super::*;
    use amphora::db::Db;
    use amphora::groups::Groups;
    use amphora::store::{Checkpointed, Config, Engine, Flush, Opener};
    use amphora_engine::Value;

    /// A write: its statements and their parameters, applied in the writer's transaction.
    pub struct SqlRow {
        stmts: Arc<Vec<&'static str>>,
        params: Vec<Vec<P>>,
    }

    pub struct SqlEngine {
        db: Db,
        reader_pages: Option<i64>,
    }

    pub fn value(p: &P) -> Value {
        match p {
            P::I(i) => Value::from_i64(*i),
            P::F(f) => Value::from_f64(*f),
            P::T(s) => Value::from_text(s.clone()),
            P::B(b) => Value::from_slice(b).unwrap_or(Value::Null),
            P::Z(z) => Value::from_i64(i64::from(*z)),
            P::N => Value::Null,
        }
    }

    /// Every row of each statement, every column taken: how many rows.
    fn read_all(db: &mut Db, stmts: &[&'static str], params: &[Vec<P>]) -> anyhow::Result<usize> {
        let mut n = 0;
        for (sql, ps) in stmts.iter().zip(params) {
            db.query(sql, ps.iter().map(value), |r| {
                for v in r.get_values() {
                    std::hint::black_box(v);
                }
                n += 1;
                Ok(true)
            })?;
        }
        Ok(n)
    }

    impl Engine for SqlEngine {
        type Row = SqlRow;
        type Reader = Db;

        fn key(_row: &SqlRow) -> &str {
            ""
        }

        fn apply(&mut self, rows: &[SqlRow]) -> anyhow::Result<usize> {
            self.db.transaction(|db| {
                for row in rows {
                    read_all(db, &row.stmts, &row.params)?;
                }
                Ok(rows.len())
            })
        }

        fn flusher(&self) -> Option<Flush> {
            Some(self.db.flusher())
        }

        fn opener(&self) -> Option<Opener<Db>> {
            let (open, pages) = (self.db.opener(), self.reader_pages);
            Some(Arc::new(move || {
                let r = open()?;
                if let Some(p) = pages {
                    r.keep_pages(p)?;
                }
                Ok(r)
            }))
        }

        fn checkpoint(reader: &mut Db) -> anyhow::Result<Checkpointed> {
            reader.checkpoint()
        }

        fn restart_log(&mut self) -> anyhow::Result<bool> {
            self.db.restart_log()
        }
    }

    pub struct Amphora {
        loader: Mutex<Option<Db>>,
        groups: OnceLock<Arc<Groups<SqlEngine>>>,
        /// Statements made `'static` once each (the engine's statement cache keeps them by
        /// address).
        sql: Arc<Mutex<HashMap<String, &'static str>>>,
        readers: usize,
        reader_pages: Option<i64>,
    }

    fn leak(cache: &Mutex<HashMap<String, &'static str>>, s: &str) -> anyhow::Result<&'static str> {
        let mut cache = cache.lock().map_err(|_| anyhow::anyhow!("poisoned"))?;
        if let Some(found) = cache.get(s) {
            return Ok(found);
        }
        let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
        cache.insert(s.to_owned(), leaked);
        Ok(leaked)
    }

    impl Amphora {
        pub fn open(dir: &str, clients: usize) -> anyhow::Result<Self> {
            let path = fresh(dir)?;
            let db = Db::open(&path)?;
            if std::env::var("AMPHORA_PACK_LOG").ok().as_deref() == Some("1") {
                db.pack_log(true)?;
            }
            // AMPHORA_SHARED_PAGES: pages the store's connections share at most (decision 20).
            if let Some(pages) = std::env::var("AMPHORA_SHARED_PAGES").ok().and_then(|v| v.parse().ok()) {
                db.share_pages(pages)?;
            }
            let reader_pages = std::env::var("AMPHORA_READER_PAGES").ok().and_then(|v| v.parse().ok());
            Ok(Self { loader: Mutex::new(Some(db)), groups: OnceLock::new(), sql: Arc::new(Mutex::new(HashMap::new())), readers: clients.max(1), reader_pages })
        }
    }

    impl Target for Amphora {
        fn dialect(&self) -> Dialect {
            Dialect::Sqlite
        }

        fn setup(&self, stmts: &[(String, Vec<P>)]) -> anyhow::Result<()> {
            let mut loader = self.loader.lock().map_err(|_| anyhow::anyhow!("poisoned"))?;
            let db = loader.as_mut().ok_or_else(|| anyhow::anyhow!("the store has started"))?;
            let leaked: Vec<&'static str> = stmts.iter().map(|(s, _)| leak(&self.sql, s)).collect::<anyhow::Result<_>>()?;
            db.transaction(|db| {
                for (sql, (_, ps)) in leaked.iter().zip(stmts) {
                    db.query(sql, ps.iter().map(value), |_| Ok(true))?;
                }
                Ok(())
            })?;
            db.flusher()()?;
            Ok(())
        }

        fn ready(&self) -> anyhow::Result<()> {
            let db = self.loader.lock().map_err(|_| anyhow::anyhow!("poisoned"))?.take().ok_or_else(|| anyhow::anyhow!("started twice"))?;
            let mut slot = Some(SqlEngine { db, reader_pages: self.reader_pages });
            // As many readers as clients: a read never waits for another's reader.
            let config = Config { readers: self.readers, ..Config::default() };
            let groups = Groups::start(1, config, |_| slot.take().ok_or_else(|| anyhow::anyhow!("one group")))?;
            self.groups.set(Arc::new(groups)).map_err(|_| anyhow::anyhow!("started twice"))?;
            Ok(())
        }

        fn session(&self) -> anyhow::Result<Box<dyn Session>> {
            let groups = self.groups.get().ok_or_else(|| anyhow::anyhow!("not started"))?.clone();
            Ok(Box::new(AmphoraSession { groups, sql: self.sql.clone(), known: HashMap::new() }))
        }

        fn stats(&self) -> serde_json::Value {
            let Some(g) = self.groups.get() else {
                return serde_json::Value::Null;
            };
            let c = g.counters().into_iter().next().unwrap_or_default();
            serde_json::json!({
                "commits": c.commits, "rows": c.rows, "busy_ms": c.busy_ms, "apply_ms": c.apply_ms,
                "flushes": c.flushes, "flush_ms": c.flush_ms, "failed": c.failed, "timeouts": c.timeouts,
                "checkpoints": c.checkpoints, "checkpoint_ms": c.checkpoint_ms, "restarts": c.restarts,
                "restart_ms": c.restart_ms, "longest_us": c.longest_us,
            })
        }
    }

    struct AmphoraSession {
        groups: Arc<Groups<SqlEngine>>,
        sql: Arc<Mutex<HashMap<String, &'static str>>>,
        /// This client's statements made `'static`, by the address of the run's list.
        known: HashMap<usize, Arc<Vec<&'static str>>>,
    }

    impl Session for AmphoraSession {
        fn run(&mut self, kind: Kind, stmts: &[String], params: &[Vec<P>]) -> anyhow::Result<usize> {
            let at = stmts.as_ptr() as usize;
            let leaked = match self.known.get(&at) {
                Some(l) if l.len() == stmts.len() => l.clone(),
                _ => {
                    let l = Arc::new(stmts.iter().map(|s| leak(&self.sql, s)).collect::<anyhow::Result<Vec<_>>>()?);
                    self.known.insert(at, l.clone());
                    l
                }
            };
            let until = Instant::now() + WAIT;
            match kind {
                Kind::Read => {
                    let (s1, p1) = (leaked.clone(), params.to_vec());
                    let (s2, p2) = (leaked, params.to_vec());
                    self.groups.read_committed("", until, move |db| read_all(db, &s1, &p1), move |e: &mut SqlEngine| read_all(&mut e.db, &s2, &p2))
                }
                Kind::Write => {
                    let ticket = self.groups.put_wait(vec![SqlRow { stmts: leaked, params: params.to_vec() }], until)?;
                    self.groups.durable(&ticket, until)?;
                    Ok(0)
                }
            }
        }

        fn explain(&mut self, sql: &str, params: &[P]) -> anyhow::Result<Vec<String>> {
            // AMPHORA_EQP_JSON=1: the plan with the optimizer's estimates (EXPLAIN QUERY PLAN FORMAT=JSON).
            let format = if std::env::var("AMPHORA_EQP_JSON").ok().as_deref() == Some("1") { "FORMAT=JSON " } else { "" };
            let plan = leak(&self.sql, &format!("EXPLAIN QUERY PLAN {format}{sql}"))?;
            let p = params.to_vec();
            let lines = move |db: &mut Db| -> anyhow::Result<Vec<String>> {
                let mut out = Vec::new();
                db.query(plan, p.iter().map(value), |r| {
                    out.push(r.get_values().map(|v| v.to_string()).collect::<Vec<_>>().join(" | "));
                    Ok(true)
                })?;
                Ok(out)
            };
            let on_writer = lines.clone();
            self.groups.read_committed("", Instant::now() + WAIT, lines, move |e: &mut SqlEngine| on_writer(&mut e.db))
        }
    }
}

use amphora_target::Amphora;

// ---------------------------------------------------------------------------------------------
// Turso (upstream)

struct Turso {
    io: Arc<dyn turso_core::IO>,
    db: Arc<turso_core::Database>,
}

impl Turso {
    fn open(dir: &str) -> anyhow::Result<Self> {
        let path = fresh(dir)?;
        let name = path.to_str().ok_or_else(|| anyhow::anyhow!("a path that isn't UTF-8"))?;
        let io: Arc<dyn turso_core::IO> = Arc::new(turso_core::PlatformIO::new().map_err(|e| anyhow::anyhow!("{e}"))?);
        let db = turso_core::Database::open(io.clone(), name, turso_core::OpenOptions::new(Arc::new(turso_core::SqliteDialect)).db_opts(turso_core::DatabaseOpts::new())).map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(Self { io, db })
    }

    fn connect(&self) -> anyhow::Result<TursoSession> {
        let conn = self.db.connect(turso_core::ConnectOpts::new()).map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(TursoSession { io: self.io.clone(), conn, stmts: HashMap::new() })
    }
}

impl Target for Turso {
    fn dialect(&self) -> Dialect {
        Dialect::Sqlite
    }

    fn setup(&self, stmts: &[(String, Vec<P>)]) -> anyhow::Result<()> {
        let mut s = self.connect()?;
        let sql: Vec<String> = stmts.iter().map(|(s, _)| s.clone()).collect();
        let params: Vec<Vec<P>> = stmts.iter().map(|(_, p)| p.clone()).collect();
        s.transaction(&sql, &params)
    }

    fn session(&self) -> anyhow::Result<Box<dyn Session>> {
        Ok(Box::new(self.connect()?))
    }
}

struct TursoSession {
    io: Arc<dyn turso_core::IO>,
    conn: Arc<turso_core::Connection>,
    stmts: HashMap<String, turso_core::Statement>,
}

/// A statement found the database locked by another writer.
struct Busy;

fn turso_value(p: &P) -> turso_core::Value {
    match p {
        P::I(i) => turso_core::Value::from_i64(*i),
        P::F(f) => turso_core::Value::from_f64(*f),
        P::T(s) => turso_core::Value::from_text(s.clone()),
        P::B(b) => turso_core::Value::from_slice(b).unwrap_or(turso_core::Value::Null),
        P::Z(z) => turso_core::Value::from_i64(i64::from(*z)),
        P::N => turso_core::Value::Null,
    }
}

impl TursoSession {
    fn step(&mut self, sql: &str, params: &[P], rows: &mut usize) -> anyhow::Result<Result<(), Busy>> {
        if !self.stmts.contains_key(sql) {
            let stmt = self.conn.prepare(sql).map_err(|e| anyhow::anyhow!("{e}"))?;
            self.stmts.insert(sql.to_owned(), stmt);
        }
        let io = self.io.clone();
        let stmt = self.stmts.get_mut(sql).ok_or_else(|| anyhow::anyhow!("lost a statement"))?;
        stmt.reset().map_err(|e| anyhow::anyhow!("{e}"))?;
        for (i, p) in params.iter().enumerate() {
            stmt.bind_at(NonZero::new(i + 1).ok_or_else(|| anyhow::anyhow!("parameter 0"))?, turso_value(p)).map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        loop {
            match stmt.step() {
                Ok(turso_core::StepResult::Row) => {
                    if let Some(r) = stmt.row() {
                        for v in r.get_values() {
                            std::hint::black_box(v);
                        }
                    }
                    *rows += 1;
                }
                Ok(turso_core::StepResult::Done) => return Ok(Ok(())),
                Ok(turso_core::StepResult::IO | turso_core::StepResult::Yield) => io.step().map_err(|e| anyhow::anyhow!("{e}"))?,
                Ok(turso_core::StepResult::Sleep { duration }) => {
                    std::thread::sleep(duration);
                    io.step().map_err(|e| anyhow::anyhow!("{e}"))?;
                }
                Ok(turso_core::StepResult::Busy) | Err(turso_core::LimboError::Busy) => return Ok(Err(Busy)),
                Ok(turso_core::StepResult::Interrupt) => anyhow::bail!("interrupted"),
                Err(e) => return Err(anyhow::anyhow!("{e}")),
            }
        }
    }

    /// `stmts` in one transaction, tried again while another writer holds the database.
    fn transaction(&mut self, stmts: &[String], params: &[Vec<P>]) -> anyhow::Result<()> {
        let began = Instant::now();
        loop {
            let mut rows = 0;
            let attempt = (|| -> anyhow::Result<Result<(), Busy>> {
                if let Err(b) = self.step("BEGIN IMMEDIATE", &[], &mut rows)? {
                    return Ok(Err(b));
                }
                for (s, p) in stmts.iter().zip(params) {
                    if let Err(b) = self.step(s, p, &mut rows)? {
                        return Ok(Err(b));
                    }
                }
                self.step("COMMIT", &[], &mut rows)
            })();
            match attempt {
                Ok(Ok(())) => return Ok(()),
                Ok(Err(Busy)) => {
                    let _ = self.step("ROLLBACK", &[], &mut rows);
                    if began.elapsed() > WAIT {
                        anyhow::bail!("the database stayed busy");
                    }
                    std::thread::sleep(Duration::from_micros(200));
                }
                Err(e) => {
                    let _ = self.step("ROLLBACK", &[], &mut rows);
                    return Err(e);
                }
            }
        }
    }
}

impl Session for TursoSession {
    fn explain(&mut self, sql: &str, params: &[P]) -> anyhow::Result<Vec<String>> {
        let mut stmt = self.conn.prepare(format!("EXPLAIN QUERY PLAN {sql}")).map_err(|e| anyhow::anyhow!("{e}"))?;
        for (i, p) in params.iter().enumerate() {
            stmt.bind_at(NonZero::new(i + 1).ok_or_else(|| anyhow::anyhow!("parameter 0"))?, turso_value(p)).map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        let mut out = Vec::new();
        loop {
            match stmt.step().map_err(|e| anyhow::anyhow!("{e}"))? {
                turso_core::StepResult::Row => {
                    if let Some(r) = stmt.row() {
                        out.push(r.get_values().map(|v| v.to_string()).collect::<Vec<_>>().join(" | "));
                    }
                }
                turso_core::StepResult::Done => return Ok(out),
                turso_core::StepResult::IO | turso_core::StepResult::Yield => self.io.step().map_err(|e| anyhow::anyhow!("{e}"))?,
                other => anyhow::bail!("EXPLAIN stopped: {other:?}"),
            }
        }
    }

    fn run(&mut self, kind: Kind, stmts: &[String], params: &[Vec<P>]) -> anyhow::Result<usize> {
        match (kind, stmts) {
            (Kind::Write, [_, _, ..]) => self.transaction(stmts, params).map(|()| 0),
            _ => {
                let began = Instant::now();
                loop {
                    let mut rows = 0;
                    let mut busy = false;
                    for (s, p) in stmts.iter().zip(params) {
                        if self.step(s, p, &mut rows)?.is_err() {
                            busy = true;
                            break;
                        }
                    }
                    if !busy {
                        return Ok(rows);
                    }
                    if began.elapsed() > WAIT {
                        anyhow::bail!("the database stayed busy");
                    }
                    std::thread::sleep(Duration::from_micros(200));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// SQLite

struct Sqlite {
    path: PathBuf,
}

impl Sqlite {
    fn open(dir: &str) -> anyhow::Result<Self> {
        Ok(Self { path: fresh(dir)? })
    }

    fn connect(&self) -> anyhow::Result<rusqlite::Connection> {
        let c = rusqlite::Connection::open(&self.path)?;
        c.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL; PRAGMA busy_timeout = 120000;")?;
        Ok(c)
    }
}

fn sqlite_value(p: &P) -> rusqlite::types::Value {
    use rusqlite::types::Value;
    match p {
        P::I(i) => Value::Integer(*i),
        P::F(f) => Value::Real(*f),
        P::T(s) => Value::Text(s.clone()),
        P::B(b) => Value::Blob(b.clone()),
        P::Z(z) => Value::Integer(i64::from(*z)),
        P::N => Value::Null,
    }
}

/// Every row of `sql`, every column taken: how many.
fn sqlite_rows(c: &rusqlite::Connection, sql: &str, params: &[P]) -> anyhow::Result<usize> {
    let mut stmt = c.prepare_cached(sql)?;
    let columns = stmt.column_count();
    let mut rows = stmt.query(rusqlite::params_from_iter(params.iter().map(sqlite_value)))?;
    let mut n = 0;
    while let Some(r) = rows.next()? {
        for i in 0..columns {
            std::hint::black_box(r.get_ref(i)?);
        }
        n += 1;
    }
    Ok(n)
}

impl Target for Sqlite {
    fn dialect(&self) -> Dialect {
        Dialect::Sqlite
    }

    fn setup(&self, stmts: &[(String, Vec<P>)]) -> anyhow::Result<()> {
        let mut c = self.connect()?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for (s, p) in stmts {
            sqlite_rows(&tx, s, p)?;
        }
        tx.commit()?;
        Ok(())
    }

    fn session(&self) -> anyhow::Result<Box<dyn Session>> {
        Ok(Box::new(SqliteSession { c: self.connect()? }))
    }
}

struct SqliteSession {
    c: rusqlite::Connection,
}

impl Session for SqliteSession {
    fn explain(&mut self, sql: &str, params: &[P]) -> anyhow::Result<Vec<String>> {
        let mut stmt = self.c.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
        let mut rows = stmt.query(rusqlite::params_from_iter(params.iter().map(sqlite_value)))?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(r.get::<_, String>(3)?);
        }
        Ok(out)
    }

    fn run(&mut self, kind: Kind, stmts: &[String], params: &[Vec<P>]) -> anyhow::Result<usize> {
        if kind == Kind::Write && stmts.len() > 1 {
            let tx = self.c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            for (s, p) in stmts.iter().zip(params) {
                sqlite_rows(&tx, s, p)?;
            }
            tx.commit()?;
            return Ok(0);
        }
        let mut n = 0;
        for (s, p) in stmts.iter().zip(params) {
            n += sqlite_rows(&self.c, s, p)?;
        }
        Ok(n)
    }
}

// ---------------------------------------------------------------------------------------------
// PostgreSQL, OrioleDB, CedarDB

#[derive(Clone, Copy, PartialEq, Eq)]
enum PgKind {
    Postgres,
    Orioledb,
    Cedardb,
}

struct Pg {
    dsn: String,
    kind: PgKind,
}

/// A column's bytes as the server sent them: every column taken, whatever its type.
struct Raw(usize);

impl<'a> postgres::types::FromSql<'a> for Raw {
    fn from_sql(_: &postgres::types::Type, raw: &'a [u8]) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(Raw(raw.len()))
    }

    fn accepts(_: &postgres::types::Type) -> bool {
        true
    }
}

fn pg_params(params: &[P]) -> Vec<Box<dyn postgres::types::ToSql + Sync>> {
    params
        .iter()
        .map(|p| -> Box<dyn postgres::types::ToSql + Sync> {
            match p {
                P::I(i) => Box::new(*i),
                P::F(f) => Box::new(*f),
                P::T(s) => Box::new(s.clone()),
                P::B(b) => Box::new(b.clone()),
                P::Z(z) => Box::new(*z),
                P::N => Box::new(None::<String>),
            }
        })
        .collect()
}

impl Pg {
    fn open(dsn: &str, kind: PgKind) -> anyhow::Result<Self> {
        let pg = Self { dsn: dsn.to_owned(), kind };
        let mut c = pg.client()?;
        if kind == PgKind::Orioledb {
            c.batch_execute("CREATE EXTENSION IF NOT EXISTS orioledb")?;
        }
        c.batch_execute("DROP TABLE IF EXISTS items; DROP TABLE IF EXISTS owners; DROP TABLE IF EXISTS regions")?;
        Ok(pg)
    }

    fn client(&self) -> anyhow::Result<postgres::Client> {
        let mut c = postgres::Client::connect(&self.dsn, postgres::NoTls)?;
        if self.kind == PgKind::Cedardb {
            c.batch_execute("SET async_commit = off")?;
        }
        Ok(c)
    }
}

impl Target for Pg {
    fn dialect(&self) -> Dialect {
        Dialect::Postgres
    }

    fn setup(&self, stmts: &[(String, Vec<P>)]) -> anyhow::Result<()> {
        let mut c = self.client()?;
        // VACUUM runs outside a transaction.
        if let [(s, p)] = stmts {
            if p.is_empty() && s.starts_with("VACUUM") {
                c.batch_execute(s)?;
                return Ok(());
            }
        }
        let mut tx = c.transaction()?;
        for (s, p) in stmts {
            let s = if self.kind == PgKind::Orioledb && s.starts_with("CREATE TABLE") { format!("{s} USING orioledb") } else { s.clone() };
            let s = crate::suite::placeholders(Dialect::Postgres, &s);
            let owned = pg_params(p);
            let refs: Vec<&(dyn postgres::types::ToSql + Sync)> = owned.iter().map(|b| b.as_ref()).collect();
            tx.execute(s.as_str(), &refs).map_err(|e| anyhow::anyhow!("{e} (in: {})", s.chars().take(200).collect::<String>()))?;
        }
        tx.commit()?;
        Ok(())
    }

    fn session(&self) -> anyhow::Result<Box<dyn Session>> {
        Ok(Box::new(PgSession { c: self.client()?, stmts: HashMap::new() }))
    }
}

struct PgSession {
    c: postgres::Client,
    stmts: HashMap<String, postgres::Statement>,
}

impl PgSession {
    fn statement(&mut self, sql: &str) -> anyhow::Result<postgres::Statement> {
        if let Some(s) = self.stmts.get(sql) {
            return Ok(s.clone());
        }
        let s = self.c.prepare(sql)?;
        self.stmts.insert(sql.to_owned(), s.clone());
        Ok(s)
    }
}

fn pg_rows(rows: &[postgres::Row]) -> anyhow::Result<usize> {
    for r in rows {
        for i in 0..r.len() {
            let v: Option<Raw> = r.try_get(i)?;
            std::hint::black_box(v.map(|raw| raw.0));
        }
    }
    Ok(rows.len())
}

impl Session for PgSession {
    fn explain(&mut self, sql: &str, params: &[P]) -> anyhow::Result<Vec<String>> {
        let owned = pg_params(params);
        let refs: Vec<&(dyn postgres::types::ToSql + Sync)> = owned.iter().map(|b| b.as_ref()).collect();
        let rows = self.c.query(format!("EXPLAIN {sql}").as_str(), &refs)?;
        rows.iter().map(|r| r.try_get::<_, String>(0).map_err(Into::into)).collect()
    }

    fn run(&mut self, kind: Kind, stmts: &[String], params: &[Vec<P>]) -> anyhow::Result<usize> {
        let prepared: Vec<postgres::Statement> = stmts.iter().map(|s| self.statement(s)).collect::<anyhow::Result<_>>()?;
        if kind == Kind::Write && stmts.len() > 1 {
            let mut tx = self.c.transaction()?;
            for (s, p) in prepared.iter().zip(params) {
                let owned = pg_params(p);
                let refs: Vec<&(dyn postgres::types::ToSql + Sync)> = owned.iter().map(|b| b.as_ref()).collect();
                pg_rows(&tx.query(s, &refs)?)?;
            }
            tx.commit()?;
            return Ok(0);
        }
        let mut n = 0;
        for (s, p) in prepared.iter().zip(params) {
            let owned = pg_params(p);
            let refs: Vec<&(dyn postgres::types::ToSql + Sync)> = owned.iter().map(|b| b.as_ref()).collect();
            n += pg_rows(&self.c.query(s, &refs)?)?;
        }
        Ok(n)
    }
}

// ---------------------------------------------------------------------------------------------
// MySQL

struct MySql {
    pool: mysql::Pool,
}

fn mysql_params(params: &[P]) -> mysql::Params {
    if params.is_empty() {
        return mysql::Params::Empty;
    }
    mysql::Params::Positional(
        params
            .iter()
            .map(|p| match p {
                P::I(i) => mysql::Value::Int(*i),
                P::F(f) => mysql::Value::Double(*f),
                P::T(s) => mysql::Value::Bytes(s.clone().into_bytes()),
                P::B(b) => mysql::Value::Bytes(b.clone()),
                P::Z(z) => mysql::Value::Int(i64::from(*z)),
                P::N => mysql::Value::NULL,
            })
            .collect(),
    )
}

/// Every row of `sql` on `q`, every column taken: how many.
fn mysql_rows(q: &mut impl mysql::prelude::Queryable, sql: &str, params: &[P]) -> anyhow::Result<usize> {
    let mut n = 0;
    let result = q.exec_iter(sql, mysql_params(params))?;
    for row in result {
        let row = row?;
        for i in 0..row.len() {
            std::hint::black_box(row.as_ref(i));
        }
        n += 1;
    }
    Ok(n)
}

impl MySql {
    fn open(dsn: &str, connections: usize) -> anyhow::Result<Self> {
        use mysql::prelude::Queryable;
        let constraints = mysql::PoolConstraints::new(1, connections.max(2)).ok_or_else(|| anyhow::anyhow!("bad pool size"))?;
        let opts = mysql::OptsBuilder::from_opts(mysql::Opts::from_url(dsn)?).pool_opts(mysql::PoolOpts::default().with_constraints(constraints));
        let pool = mysql::Pool::new(opts)?;
        let mut c = pool.get_conn()?;
        for t in ["items", "owners", "regions"] {
            c.query_drop(format!("DROP TABLE IF EXISTS {t}"))?;
        }
        Ok(Self { pool })
    }
}

impl Target for MySql {
    fn dialect(&self) -> Dialect {
        Dialect::MySql
    }

    fn setup(&self, stmts: &[(String, Vec<P>)]) -> anyhow::Result<()> {
        let mut c = self.pool.get_conn()?;
        let mut tx = c.start_transaction(mysql::TxOpts::default())?;
        for (s, p) in stmts {
            mysql_rows(&mut tx, s, p).map_err(|e| anyhow::anyhow!("{e} (in: {})", s.chars().take(200).collect::<String>()))?;
        }
        tx.commit()?;
        Ok(())
    }

    fn session(&self) -> anyhow::Result<Box<dyn Session>> {
        Ok(Box::new(MySqlSession { c: self.pool.get_conn()? }))
    }
}

struct MySqlSession {
    c: mysql::PooledConn,
}

impl Session for MySqlSession {
    fn explain(&mut self, sql: &str, params: &[P]) -> anyhow::Result<Vec<String>> {
        use mysql::prelude::Queryable;
        let rows: Vec<String> = self.c.exec(format!("EXPLAIN FORMAT=TREE {sql}"), mysql_params(params))?;
        Ok(rows.iter().flat_map(|r| r.lines().map(str::to_owned).collect::<Vec<_>>()).collect())
    }

    fn run(&mut self, kind: Kind, stmts: &[String], params: &[Vec<P>]) -> anyhow::Result<usize> {
        if kind == Kind::Write && stmts.len() > 1 {
            let mut tx = self.c.start_transaction(mysql::TxOpts::default())?;
            for (s, p) in stmts.iter().zip(params) {
                mysql_rows(&mut tx, s, p)?;
            }
            tx.commit()?;
            return Ok(0);
        }
        let mut n = 0;
        for (s, p) in stmts.iter().zip(params) {
            n += mysql_rows(&mut self.c, s, p)?;
        }
        Ok(n)
    }
}

// ---------------------------------------------------------------------------------------------
// ClickHouse

struct ClickHouse {
    agent: ureq::Agent,
    url: String,
}

/// `sql` with each `?` outside quotes replaced by its parameter as a literal.
fn inline(sql: &str, params: &[P]) -> String {
    let mut out = String::with_capacity(sql.len() + params.len() * 16);
    let (mut next, mut quoted) = (params.iter(), false);
    for ch in sql.chars() {
        match ch {
            '\'' => {
                quoted = !quoted;
                out.push(ch);
            }
            '?' if !quoted => match next.next() {
                Some(P::I(i)) => out.push_str(&i.to_string()),
                Some(P::F(f)) => out.push_str(&format!("{f:?}")),
                Some(P::T(s)) => {
                    out.push('\'');
                    for c in s.chars() {
                        if c == '\'' || c == '\\' {
                            out.push('\\');
                        }
                        out.push(c);
                    }
                    out.push('\'');
                }
                Some(P::B(b)) => {
                    out.push_str("unhex('");
                    for byte in b {
                        out.push_str(&format!("{byte:02x}"));
                    }
                    out.push_str("')");
                }
                Some(P::Z(z)) => out.push_str(if *z { "true" } else { "false" }),
                Some(P::N) | None => out.push_str("NULL"),
            },
            _ => out.push(ch),
        }
    }
    out
}

impl ClickHouse {
    fn open(url: &str, connections: usize) -> anyhow::Result<Self> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .max_idle_connections(connections)
            .max_idle_connections_per_host(connections)
            .max_idle_age(Duration::from_secs(5))
            .no_delay(true)
            .http_status_as_error(false)
            .build()
            .into();
        let ch = Self { agent, url: format!("{}/", url.trim_end_matches('/')) };
        for t in ["items", "owners", "regions"] {
            ch.post(&format!("DROP TABLE IF EXISTS {t} SYNC"))?;
        }
        Ok(ch)
    }

    fn post(&self, sql: &str) -> anyhow::Result<String> {
        post(&self.agent, &self.url, sql)
    }
}

fn post(agent: &ureq::Agent, url: &str, sql: &str) -> anyhow::Result<String> {
    let mut response = agent.post(url).send(sql.to_owned())?;
    let status = response.status();
    let text = response.body_mut().read_to_string()?;
    if !status.is_success() {
        anyhow::bail!("ClickHouse said {status}: {}", text.trim().chars().take(300).collect::<String>());
    }
    Ok(text)
}

impl Target for ClickHouse {
    fn dialect(&self) -> Dialect {
        Dialect::ClickHouse
    }

    fn setup(&self, stmts: &[(String, Vec<P>)]) -> anyhow::Result<()> {
        for (s, p) in stmts {
            self.post(&inline(s, p))?;
        }
        Ok(())
    }

    fn session(&self) -> anyhow::Result<Box<dyn Session>> {
        Ok(Box::new(ClickHouseSession { agent: self.agent.clone(), url: self.url.clone() }))
    }
}

struct ClickHouseSession {
    agent: ureq::Agent,
    url: String,
}

impl Session for ClickHouseSession {
    fn explain(&mut self, sql: &str, params: &[P]) -> anyhow::Result<Vec<String>> {
        let out = post(&self.agent, &self.url, &format!("EXPLAIN indexes = 1 {} FORMAT TSV", inline(sql, params)))?;
        Ok(out.lines().map(str::to_owned).collect())
    }

    fn run(&mut self, kind: Kind, stmts: &[String], params: &[Vec<P>]) -> anyhow::Result<usize> {
        let mut n = 0;
        for (s, p) in stmts.iter().zip(params) {
            let sql = inline(s, p);
            if kind == Kind::Read || s.starts_with("SELECT") {
                let out = post(&self.agent, &self.url, &format!("{sql} FORMAT TSV"))?;
                for line in out.lines() {
                    std::hint::black_box(line.split('\t').count());
                    n += 1;
                }
            } else {
                post(&self.agent, &self.url, &sql)?;
            }
        }
        Ok(n)
    }
}
