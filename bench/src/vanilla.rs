//! The engines as they ship, on kiln's conversation schema and statements (`crate::schema`), each
//! as durable as it goes by default:
//! - **Turso** (`turso_core` upstream, at 3e09a82, the revision Amphora's engine was taken from,
//!   with none of Amphora's changes): its defaults (every commit
//!   flushed, its own checkpoints), a connection for each thread, a write one transaction, tried
//!   again while another writer holds the lock;
//! - **SQLite** (rusqlite's bundled SQLite): its log (WAL) with every commit flushed
//!   (`synchronous = FULL`), a connection for each thread, the same transactions.

use std::collections::HashSet;
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::schema as sql;
use turso_core::{Connection, ConnectOpts, Database, DatabaseOpts, IO, OpenOptions, PlatformIO, SqliteDialect, StepResult, Value};

use crate::Event;
use crate::pool::Blocking;

fn fresh(dir: &str) -> anyhow::Result<PathBuf> {
    let dir = Path::new(dir);
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir)?;
    Ok(dir.join("bench.db"))
}

/// The schema the store keeps conversations in, statement by statement.
fn schema() -> Vec<String> {
    sql::SCHEMA.iter().map(|s| (*s).to_owned()).collect()
}

/// The words of a search, as the store's LIKE would take them.
fn like(word: &str) -> String {
    format!("%{word}%")
}

const SEARCH: &str = sql::SEARCH;

pub struct Turso {
    io: Arc<dyn IO>,
    db: Arc<Database>,
    pool: Mutex<Vec<Arc<Connection>>>,
    rev: AtomicU64,
    seen: Mutex<HashSet<String>>,
}

/// A statement that found the database locked by another writer: the transaction goes again.
struct Busy;

impl Turso {
    pub fn open(dir: &str) -> anyhow::Result<Self> {
        let path = fresh(dir)?;
        let name = path.to_str().ok_or_else(|| anyhow::anyhow!("a path that isn't UTF-8"))?;
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().map_err(|e| anyhow::anyhow!("{e}"))?);
        let db = Database::open(io.clone(), name, OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new())).map_err(|e| anyhow::anyhow!("{e}"))?;
        let t = Self { io, db, pool: Mutex::new(Vec::new()), rev: AtomicU64::new(0), seen: Mutex::new(HashSet::new()) };
        let c = t.connection()?;
        for s in schema() {
            c.execute(&s).map_err(|e| anyhow::anyhow!("{e} (in: {s})"))?;
        }
        t.give(c);
        Ok(t)
    }

    fn connection(&self) -> anyhow::Result<Arc<Connection>> {
        if let Some(c) = self.pool.lock().map_err(|_| anyhow::anyhow!("poisoned"))?.pop() {
            return Ok(c);
        }
        self.db.connect(ConnectOpts::new()).map_err(|e| anyhow::anyhow!("{e}"))
    }

    fn give(&self, c: Arc<Connection>) {
        if let Ok(mut pool) = self.pool.lock() {
            pool.push(c);
        }
    }

    /// Runs `sql` with `params`, handing each row's columns to `each`.
    fn run(&self, c: &Arc<Connection>, sql: &str, params: &[Value], mut each: impl FnMut(&turso_core::Row)) -> anyhow::Result<Result<(), Busy>> {
        let mut stmt = c.prepare(sql).map_err(|e| anyhow::anyhow!("{e}"))?;
        for (i, p) in params.iter().enumerate() {
            stmt.bind_at(NonZero::new(i + 1).ok_or_else(|| anyhow::anyhow!("parameter 0"))?, p.clone()).map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        loop {
            match stmt.step() {
                Ok(StepResult::Row) => {
                    if let Some(r) = stmt.row() {
                        each(r);
                    }
                }
                Ok(StepResult::Done) => return Ok(Ok(())),
                Ok(StepResult::IO | StepResult::Yield) => self.io.step().map_err(|e| anyhow::anyhow!("{e}"))?,
                Ok(StepResult::Sleep { duration }) => {
                    std::thread::sleep(duration);
                    self.io.step().map_err(|e| anyhow::anyhow!("{e}"))?;
                }
                Ok(StepResult::Busy) | Err(turso_core::LimboError::Busy) => return Ok(Err(Busy)),
                Ok(StepResult::Interrupt) => anyhow::bail!("interrupted"),
                Err(e) => return Err(anyhow::anyhow!("{e}")),
            }
        }
    }

    fn try_write(&self, c: &Arc<Connection>, events: &[Event], new: &[String]) -> anyhow::Result<Result<(), Busy>> {
        if let Err(b) = self.run(c, "BEGIN IMMEDIATE", &[], |_| {})? {
            return Ok(Err(b));
        }
        let t = |s: &str| Value::from_text(s.to_owned());
        let i = |n: i64| Value::from_i64(n);
        for s in new {
            let rev = i(i64::try_from(self.rev.fetch_add(1, Ordering::Relaxed) + 1).unwrap_or(i64::MAX));
            let params = [t(s), t("{}"), Value::Null, Value::Null, Value::Null, i(0), i(0), rev];
            if let Err(b) = self.run(c, sql::SESSION_UPSERT, &params, |_| {})? {
                return Ok(Err(b));
            }
        }
        for e in events {
            let rev = i(i64::try_from(self.rev.fetch_add(1, Ordering::Relaxed) + 1).unwrap_or(i64::MAX));
            let params = [t(&e.session), t(&e.id), i(i64::try_from(e.seq).unwrap_or(i64::MAX)), i(e.at), Value::Null, Value::Null, t(e.kind), Value::Null, Value::Null, Value::Null, t(&e.text), t(&e.body), rev];
            if let Err(b) = self.run(c, sql::EVENT_UPSERT, &params, |_| {})? {
                return Ok(Err(b));
            }
        }
        self.run(c, "COMMIT", &[], |_| {})
    }
}

impl Blocking for Turso {
    fn write(&self, events: &[Event]) -> anyhow::Result<()> {
        let new: Vec<String> = {
            let mut seen = self.seen.lock().map_err(|_| anyhow::anyhow!("poisoned"))?;
            events.iter().filter(|e| seen.insert(e.session.clone())).map(|e| e.session.clone()).collect()
        };
        let c = self.connection()?;
        let done = loop {
            match self.try_write(&c, events, &new) {
                Ok(Ok(())) => break Ok(()),
                Ok(Err(Busy)) => {
                    let _ = c.execute("ROLLBACK");
                    std::thread::sleep(std::time::Duration::from_micros(200));
                }
                Err(e) => {
                    let _ = c.execute("ROLLBACK");
                    break Err(e);
                }
            }
        };
        self.give(c);
        done
    }

    fn history(&self, session: &str, limit: usize) -> anyhow::Result<usize> {
        let c = self.connection()?;
        let params = [Value::from_text(session.to_owned()), Value::from_i64(i64::try_from(limit).unwrap_or(i64::MAX))];
        let done = loop {
            let mut n = 0;
            // Every column taken, as a reader of the page would (the store builds its rows).
            match self.run(&c, sql::EVENTS_LAST, &params, |r| {
                let row: Vec<turso_core::Value> = (0..12).map(|i| r.get_value(i).clone()).collect();
                std::hint::black_box(row);
                n += 1;
            }) {
                Ok(Ok(())) => break Ok(n),
                Ok(Err(Busy)) => std::thread::sleep(std::time::Duration::from_micros(200)),
                Err(e) => break Err(e),
            }
        };
        self.give(c);
        done
    }

    fn search(&self, word: &str, limit: usize) -> anyhow::Result<usize> {
        let c = self.connection()?;
        let mut n = 0;
        let params = [Value::from_text(like(word)), Value::from_i64(i64::try_from(limit).unwrap_or(i64::MAX))];
        let done = match self.run(&c, SEARCH, &params, |_| n += 1) {
            Ok(_) => Ok(n),
            Err(e) => Err(e),
        };
        self.give(c);
        done
    }
}

pub struct Sqlite {
    path: PathBuf,
    pool: Mutex<Vec<rusqlite::Connection>>,
    rev: AtomicU64,
    seen: Mutex<HashSet<String>>,
}

impl Sqlite {
    pub fn open(dir: &str) -> anyhow::Result<Self> {
        let path = fresh(dir)?;
        let s = Self { path, pool: Mutex::new(Vec::new()), rev: AtomicU64::new(0), seen: Mutex::new(HashSet::new()) };
        let c = s.connection()?;
        for stmt in schema() {
            c.execute_batch(&stmt)?;
        }
        s.give(c);
        Ok(s)
    }

    fn connection(&self) -> anyhow::Result<rusqlite::Connection> {
        if let Some(c) = self.pool.lock().map_err(|_| anyhow::anyhow!("poisoned"))?.pop() {
            return Ok(c);
        }
        let c = rusqlite::Connection::open(&self.path)?;
        c.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL; PRAGMA busy_timeout = 60000;")?;
        Ok(c)
    }

    fn give(&self, c: rusqlite::Connection) {
        if let Ok(mut pool) = self.pool.lock() {
            pool.push(c);
        }
    }
}

impl Blocking for Sqlite {
    fn write(&self, events: &[Event]) -> anyhow::Result<()> {
        let new: Vec<String> = {
            let mut seen = self.seen.lock().map_err(|_| anyhow::anyhow!("poisoned"))?;
            events.iter().filter(|e| seen.insert(e.session.clone())).map(|e| e.session.clone()).collect()
        };
        let mut c = self.connection()?;
        let done = (|| -> anyhow::Result<()> {
            let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            for s in &new {
                let rev = self.rev.fetch_add(1, Ordering::Relaxed) + 1;
                tx.prepare_cached(sql::SESSION_UPSERT)?.execute(rusqlite::params![s, "{}", None::<String>, None::<String>, None::<String>, 0i64, 0i64, rev as i64])?;
            }
            for e in events {
                let rev = self.rev.fetch_add(1, Ordering::Relaxed) + 1;
                tx.prepare_cached(sql::EVENT_UPSERT)?.execute(rusqlite::params![
                    e.session,
                    e.id,
                    e.seq as i64,
                    e.at,
                    None::<String>,
                    None::<String>,
                    e.kind,
                    None::<String>,
                    None::<String>,
                    None::<String>,
                    e.text,
                    e.body,
                    rev as i64
                ])?;
            }
            tx.commit()?;
            Ok(())
        })();
        self.give(c);
        done
    }

    fn history(&self, session: &str, limit: usize) -> anyhow::Result<usize> {
        let c = self.connection()?;
        let n = {
            let mut stmt = c.prepare_cached(sql::EVENTS_LAST)?;
            let mut rows = stmt.query(rusqlite::params![session, limit as i64])?;
            let mut n = 0;
            // Every column taken, as a reader of the page would (the store builds its rows).
            while let Some(r) = rows.next()? {
                let row: Vec<rusqlite::types::Value> = (0..12).map(|i| r.get::<_, rusqlite::types::Value>(i)).collect::<Result<_, _>>()?;
                std::hint::black_box(row);
                n += 1;
            }
            n
        };
        self.give(c);
        Ok(n)
    }

    fn search(&self, word: &str, limit: usize) -> anyhow::Result<usize> {
        let c = self.connection()?;
        let n = {
            let mut stmt = c.prepare_cached(SEARCH)?;
            let mut rows = stmt.query(rusqlite::params![like(word), limit as i64])?;
            let mut n = 0;
            while rows.next()?.is_some() {
                n += 1;
            }
            n
        };
        self.give(c);
        Ok(n)
    }
}
