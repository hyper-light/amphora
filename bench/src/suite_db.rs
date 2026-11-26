//! The suite's databases (`crate::suite`), each as it ships or as Amphora runs: each opens fresh,
//! takes the load in transactions, and gives each client a session of its own that runs a
//! statement's parts and takes every column of every row it answers. A write is acknowledged as each
//! database acknowledges a durable commit:
//! - **amphora**: reads on the store's readers, on what is committed; writes handed to its writer
//!   (one group: the suite's data isn't split by key), committed with whatever else came, and
//!   answered once its flusher made them durable. AMPHORA_PACK_LOG=1 packs its log;
//!   AMPHORA_PAGE_LOG=1 logs a page a frame (the log before decision 37, for comparing against the
//!   mixed log Amphora ships in one binary); AMPHORA_READER_PAGES sets each reader's page budget, AMPHORA_WRITER_PAGES its writer's;
//!   AMPHORA_COMMIT_ROWS the statements a commit takes at most; AMPHORA_SHARED_PAGES=<pages>
//!   shares pages among its connections;
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
    /// The load and the indexes are in: everything they wrote into the database's own files, as its
    /// checkpoint puts it, so no run reads against what the load left to copy (SQLite checkpoints
    /// as it loads; Amphora's loader leaves the whole load in its log for the store's checkpointer).
    /// Timed as `settle_s`. A database with no such statement does nothing here.
    fn settle(&self) -> anyhow::Result<()> {
        Ok(())
    }
    /// The load and the indexes are in: the runs begin.
    fn ready(&self) -> anyhow::Result<()> {
        Ok(())
    }
    /// A client's session.
    fn session(&self) -> anyhow::Result<Box<dyn Session>>;
    /// Maximum concurrent sessions this target's internal pool supports, if bounded.
    fn max_sessions(&self) -> Option<usize> {
        None
    }
    /// Optional weak probe to the underlying engine instance, returning `true` once torn down.
    fn instance_dropped_probe(&self) -> Option<Box<dyn Fn() -> bool + Send + Sync>> {
        None
    }
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
    /// A read's rows with every value as [`Cell::text`] writes it, untimed: the answer check
    /// (`bench/suite_answers.py`) holds each database's to another's.
    fn answer(
        &mut self,
        stmts: &[String],
        params: &[Vec<P>],
    ) -> anyhow::Result<Vec<Vec<Option<String>>>> {
        let _ = (stmts, params);
        anyhow::bail!("no answer check here")
    }
    /// Runs a self-cleaning DDL workload in a transaction and returns every row produced by its
    /// verification queries (e.g. `ddl_analyze_stats`'s `SELECT COUNT(*) FROM regions`).
    fn answer_ddl(
        &mut self,
        stmts: &[String],
        params: &[Vec<P>],
    ) -> anyhow::Result<Vec<Vec<Option<String>>>> {
        self.answer(stmts, params)
    }
    /// The plan the database makes for `sql` with `params` (its EXPLAIN), a line a step.
    fn explain(&mut self, sql: &str, params: &[P]) -> anyhow::Result<Vec<String>> {
        let _ = (sql, params);
        anyhow::bail!("no EXPLAIN here")
    }
}

/// A value of a read's answer, whatever the database returned it as.
pub enum Cell<'a> {
    Null,
    Int(i64),
    Real(f64),
    Text(&'a str),
    Blob(&'a [u8]),
}

impl Cell<'_> {
    /// The value as the answer check compares it: an integer in decimal; a real as Rust writes an
    /// f64 (the shortest text that reads back as the same f64), `.0` added to a whole one so it
    /// stays apart from an integer; text as it is; a blob as `x'..'` in hex; NULL as none.
    pub fn text(&self) -> Option<String> {
        Some(match self {
            Cell::Null => return None,
            Cell::Int(i) => i.to_string(),
            Cell::Real(f) => {
                let s = f.to_string();
                if s.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
                    format!("{s}.0")
                } else {
                    s
                }
            }
            Cell::Text(t) => (*t).to_owned(),
            Cell::Blob(b) => format!(
                "x'{}'",
                b.iter().map(|x| format!("{x:02x}")).collect::<String>()
            ),
        })
    }
}

/// `reuse`: the file databases (Amphora, Turso, SQLite) open the database a run left in `dir`
/// instead of a new one (`--reuse`); the servers keep their data where they keep it.
pub fn open(
    target: &str,
    dir: &str,
    dsn: Option<&str>,
    clients: usize,
    reuse: bool,
) -> anyhow::Result<Arc<dyn Target>> {
    open_with_pg_schema(target, dir, dsn, clients, reuse, None)
}

/// Opens a target for `--gauntlet`. On PostgreSQL-protocol servers, isolates tables inside
/// `pg_schema` (one schema per shard, with `public` excluded from `search_path`) so gauntlet runs
/// never touch tables in `public` or collide across parallel shard loaders.
pub fn open_gauntlet(
    target: &str,
    dir: &str,
    dsn: Option<&str>,
    clients: usize,
    reuse: bool,
    pg_schema: &str,
) -> anyhow::Result<Arc<dyn Target>> {
    open_with_pg_schema(target, dir, dsn, clients, reuse, Some(pg_schema))
}

fn open_with_pg_schema(
    target: &str,
    dir: &str,
    dsn: Option<&str>,
    clients: usize,
    reuse: bool,
    pg_schema: Option<&str>,
) -> anyhow::Result<Arc<dyn Target>> {
    let dsn = |local: &str| dsn.unwrap_or(local).to_owned();
    Ok(match target {
        "amphora" => Arc::new(Amphora::open(dir, clients, reuse)?),
        "amphora-pg" => Arc::new(Amphora::open_postgres(dir, clients, reuse)?),
        "turso" => Arc::new(Turso::open(dir, reuse)?),
        "mvcc" => Arc::new(mvcc_target::Mvcc::open(dir, reuse)?),
        "sqlite" => Arc::new(Sqlite::open(dir, reuse)?),
        "postgres" => Arc::new(Pg::open(
            &dsn("postgres://postgres@127.0.0.1:55432/postgres"),
            PgKind::Postgres,
            pg_schema,
        )?),
        "orioledb" => Arc::new(Pg::open(
            &dsn("postgres://postgres@127.0.0.1:55433/postgres"),
            PgKind::Orioledb,
            pg_schema,
        )?),
        "cedardb" => Arc::new(Pg::open(
            &dsn("postgres://postgres:amphorabenchlocalonly2026@127.0.0.1:55434/postgres"),
            PgKind::Cedardb,
            pg_schema,
        )?),
        "mysql" => Arc::new(MySql::open(
            &dsn("mysql://root@127.0.0.1:33306/convo"),
            clients + 4,
        )?),
        "clickhouse" => Arc::new(ClickHouse::open(
            &dsn("http://127.0.0.1:58123"),
            clients + 4,
        )?),
        t => anyhow::bail!(
            "the suite has no target {t} (amphora, amphora-pg, mvcc, turso, sqlite, postgres, orioledb, cedardb, mysql, clickhouse)"
        ),
    })
}

/// Exact database and engine sidecar filenames owned by a suite/gauntlet fixture in `--dir`.
const SUITE_DB_FILES: &[&str] = &[
    "suite.db",
    "suite.db-wal",
    "suite.db-wal2",
    "suite.db-shm",
    "suite.db-tshm",
    "suite.db-journal",
    "suite.db-log",
    "suite.retired",
];

fn fresh(dir: &str) -> anyhow::Result<PathBuf> {
    let dir = Path::new(dir);
    std::fs::create_dir_all(dir)?;
    // Remove ONLY this fixture's exact database and sidecar files; never delete directories or
    // unrelated files (such as `suite.db.backup/` or `suite.db.notes`) that may live in `--dir`.
    for name in SUITE_DB_FILES {
        let path = dir.join(name);
        if let Ok(meta) = std::fs::symlink_metadata(&path)
            && !meta.is_dir()
        {
            let _ = std::fs::remove_file(&path);
        }
    }
    Ok(dir.join("suite.db"))
}

/// The suite's database in `dir`: a new one, or with `reuse` the one a run left there.
fn suite_db(dir: &str, reuse: bool) -> anyhow::Result<PathBuf> {
    if !reuse {
        return fresh(dir);
    }
    let path = Path::new(dir).join("suite.db");
    anyhow::ensure!(path.exists(), "--reuse: no database at {}", path.display());
    Ok(path)
}

/// How long a statement waits for another writer before it fails: each engine's deadline and the
/// busy timeout of SQLite and Turso, whose own busy handlers do the waiting.
const WAIT: Duration = Duration::from_secs(120);

// ---------------------------------------------------------------------------------------------
// Amphora

mod amphora_target {
    use super::*;
    use amphora::db::Db;
    use amphora::groups::Groups;
    use amphora::store::{Checkpointed, Config, Engine, Flush, Opener};
    use amphora_engine::Value;

    type AnswerRows = Vec<Vec<Option<String>>>;
    type AnswerSlot = Arc<Mutex<Option<AnswerRows>>>;

    /// A write: its statements and their parameters, applied in the writer's transaction.
    pub struct SqlRow {
        stmts: Arc<Vec<&'static str>>,
        params: Vec<Vec<P>>,
        rows_read: Option<Arc<std::sync::atomic::AtomicUsize>>,
        answer_rows: Option<AnswerSlot>,
    }

    const HOLDING_TABLE: &str = "CREATE TABLE IF NOT EXISTS __suite_holding(one INTEGER PRIMARY KEY, holding BLOB NOT NULL)";
    const HOLDING_TABLE_PG: &str = "CREATE TABLE IF NOT EXISTS __suite_holding(one INTEGER PRIMARY KEY, holding BYTEA NOT NULL)";

    pub struct SqlEngine {
        db: Db,
        dialect: Dialect,
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

    fn cell(v: &Value) -> Option<String> {
        match v {
            Value::Null => Cell::Null,
            Value::Numeric(amphora_engine::Numeric::Integer(i)) => Cell::Int(*i),
            Value::Numeric(amphora_engine::Numeric::Float(f)) => Cell::Real(f64::from(*f)),
            Value::Text(t) => Cell::Text(t.as_str()),
            Value::Blob(b) => Cell::Blob(b),
        }
        .text()
    }

    /// Every row of each statement, every value as the answer check writes it.
    fn answer_all(
        db: &mut Db,
        stmts: &[&'static str],
        params: &[Vec<P>],
    ) -> anyhow::Result<Vec<Vec<Option<String>>>> {
        let mut rows = Vec::new();
        for (sql, ps) in stmts.iter().zip(params) {
            db.query(sql, ps.iter().map(value), |r| {
                rows.push(r.get_values().map(cell).collect());
                Ok(true)
            })?;
        }
        Ok(rows)
    }

    /// The holding a bench engine's group keeps with its rows, as Amphora's own engine keeps it
    /// (CP-T1.9, docs/design/reshard.md, "A group's holding"): `Groups::start` asks every engine
    /// for it, and a run stops at open without it.
    pub fn holding(db: &mut Db) -> anyhow::Result<Option<Vec<u8>>> {
        holding_sql(db, HOLDING_TABLE)
    }

    fn holding_sql(db: &mut Db, create_table: &str) -> anyhow::Result<Option<Vec<u8>>> {
        db.exec(create_table)?;
        let mut kept = None;
        db.query(
            "SELECT holding FROM __suite_holding WHERE one = 0",
            [],
            |r| {
                kept = r.get_value(0).to_blob().map(<[u8]>::to_vec);
                Ok(false)
            },
        )?;
        Ok(kept)
    }

    /// Keeps `holding` in place of the one kept before, durable before it returns.
    pub fn keep_holding(db: &mut Db, holding: &[u8]) -> anyhow::Result<()> {
        keep_holding_sql(db, HOLDING_TABLE, holding)
    }

    fn keep_holding_sql(db: &mut Db, create_table: &str, holding: &[u8]) -> anyhow::Result<()> {
        let bytes = Value::from_slice(holding).map_err(|e| anyhow::anyhow!("{e}"))?;
        db.transaction(|db| {
            db.exec(create_table)?;
            db.query("INSERT INTO __suite_holding(one, holding) VALUES (0, ?1) ON CONFLICT(one) DO UPDATE SET holding = excluded.holding", [bytes], |_| Ok(true))
        })?;
        (db.flusher())()?;
        Ok(())
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

        /// Its parameters: its statements are shared by every row of its kind.
        fn row_bytes(row: &SqlRow) -> usize {
            let list = std::mem::size_of::<SqlRow>().saturating_add(
                row.params
                    .capacity()
                    .saturating_mul(std::mem::size_of::<Vec<P>>()),
            );
            row.params.iter().fold(list, |n, ps| {
                ps.iter().fold(
                    n.saturating_add(ps.capacity().saturating_mul(std::mem::size_of::<P>())),
                    |n, p| {
                        n.saturating_add(match p {
                            P::T(s) => s.capacity(),
                            P::B(b) => b.capacity(),
                            P::I(_) | P::F(_) | P::Z(_) | P::N => 0,
                        })
                    },
                )
            })
        }

        fn apply(&mut self, rows: &[SqlRow]) -> anyhow::Result<usize> {
            self.db.transaction(|db| {
                for row in rows {
                    if let Some(slot) = &row.answer_rows {
                        let out = answer_all(db, &row.stmts, &row.params)?;
                        if let Ok(mut guard) = slot.lock() {
                            *guard = Some(out);
                        }
                    } else {
                        let n = read_all(db, &row.stmts, &row.params)?;
                        if let Some(counter) = &row.rows_read {
                            counter.store(n, std::sync::atomic::Ordering::Release);
                        }
                    }
                }
                Ok(rows.len())
            })
        }

        // As many statements to a commit as the writer's cache holds the pages of (store.md, decision 23).
        fn apply_batches(
            &mut self,
            rows: &[SqlRow],
            ends: &[usize],
        ) -> anyhow::Result<(usize, usize)> {
            let mut start = 0;
            let batches = ends.iter().map(|&end| {
                let batch = rows.get(start..end).unwrap_or_default();
                start = end;
                batch
            });
            let mut written = 0;
            let committed = self.db.transaction_while_room(batches, |db, batch| {
                for row in batch {
                    if let Some(slot) = &row.answer_rows {
                        let out = answer_all(db, &row.stmts, &row.params)?;
                        if let Ok(mut guard) = slot.lock() {
                            *guard = Some(out);
                        }
                    } else {
                        let n = read_all(db, &row.stmts, &row.params)?;
                        if let Some(counter) = &row.rows_read {
                            counter.store(n, std::sync::atomic::Ordering::Release);
                        }
                    }
                }
                written += batch.len();
                Ok(())
            })?;
            Ok((committed, written))
        }

        fn flusher(&self) -> Option<Flush> {
            Some(self.db.flusher())
        }

        fn holding(&mut self) -> anyhow::Result<Option<Vec<u8>>> {
            let table = if self.dialect == Dialect::AmphoraPostgres {
                HOLDING_TABLE_PG
            } else {
                HOLDING_TABLE
            };
            holding_sql(&mut self.db, table)
        }

        fn keep_holding(&mut self, holding: &[u8]) -> anyhow::Result<()> {
            let table = if self.dialect == Dialect::AmphoraPostgres {
                HOLDING_TABLE_PG
            } else {
                HOLDING_TABLE
            };
            keep_holding_sql(&mut self.db, table, holding)
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

        // As Amphora's own engine reports it (database.rs): the checkpointer then leaves a log
        // within its bound uncopied (docs/design/store.md, decision 35). Without it the store can't
        // tell and copies the log into the file every period, with a flush of the file each time.
        fn uncopied(reader: &mut Db) -> anyhow::Result<Option<Checkpointed>> {
            Ok(Some(reader.uncopied()))
        }

        fn restart_log(&mut self) -> anyhow::Result<bool> {
            self.db.restart_log()
        }
    }

    pub struct Amphora {
        dialect: Dialect,
        loader: Mutex<Option<Db>>,
        groups: OnceLock<Arc<Groups<SqlEngine>>>,
        /// Statements made `'static` once each (the engine's statement cache keeps them by
        /// address).
        sql: Arc<Mutex<HashMap<String, &'static str>>>,
        readers: usize,
        reader_pages: Option<i64>,
        commit_rows: Option<usize>,
        /// Where its groups keep the groups retired (CP-T1.9).
        retired: PathBuf,
        /// The pages its connections share, if they do: their counts go into the run's stats.
        pool: Option<Arc<amphora::db::SharedPages>>,
        /// Weak handle to the underlying engine `Database` so cold-reopen oracles can verify full
        /// teardown before reopening from disk.
        db_weak: std::sync::Weak<amphora_engine::Database>,
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
        pub fn open(dir: &str, clients: usize, reuse: bool) -> anyhow::Result<Self> {
            Self::open_dialect(dir, clients, reuse, Dialect::Sqlite)
        }

        pub fn open_postgres(dir: &str, clients: usize, reuse: bool) -> anyhow::Result<Self> {
            Self::open_dialect(dir, clients, reuse, Dialect::AmphoraPostgres)
        }

        fn open_dialect(
            dir: &str,
            clients: usize,
            reuse: bool,
            dialect: Dialect,
        ) -> anyhow::Result<Self> {
            let path = suite_db(dir, reuse)?;
            let sharing = amphora_sharing()?;
            let pool = match &sharing {
                amphora::db::Sharing::Process => Some(amphora::db::process_pages()),
                amphora::db::Sharing::Pool(pool) => Some(pool.clone()),
                amphora::db::Sharing::Off => None,
            };
            let db = if dialect == Dialect::AmphoraPostgres {
                Db::open_postgres_sharing(&path, &sharing)?
            } else {
                Db::open_sharing(&path, &sharing)?
            };
            if std::env::var("AMPHORA_PACK_LOG").ok().as_deref() == Some("1") {
                db.log_format(amphora::db::LogFormat::Packed)?;
            }
            if std::env::var("AMPHORA_PAGE_LOG").ok().as_deref() == Some("1") {
                db.log_format(amphora::db::LogFormat::Pages)?;
            }
            if let Some(pages) = std::env::var("AMPHORA_WRITER_PAGES")
                .ok()
                .and_then(|v| v.parse().ok())
            {
                db.keep_pages(pages)?;
            }
            let reader_pages = std::env::var("AMPHORA_READER_PAGES")
                .ok()
                .and_then(|v| v.parse().ok());
            let commit_rows = std::env::var("AMPHORA_COMMIT_ROWS")
                .ok()
                .and_then(|v| v.parse().ok());
            let retired = path.with_file_name("suite.retired");
            let db_weak = db.database_weak();
            Ok(Self {
                dialect,
                loader: Mutex::new(Some(db)),
                groups: OnceLock::new(),
                sql: Arc::new(Mutex::new(HashMap::new())),
                readers: clients.max(1),
                reader_pages,
                commit_rows,
                retired,
                pool,
                db_weak,
            })
        }
    }

    impl Target for Amphora {
        fn dialect(&self) -> Dialect {
            self.dialect
        }

        fn setup(&self, stmts: &[(String, Vec<P>)]) -> anyhow::Result<()> {
            let mut loader = self
                .loader
                .lock()
                .map_err(|_| anyhow::anyhow!("poisoned"))?;
            let db = loader
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("the store has started"))?;
            let leaked: Vec<&'static str> = stmts
                .iter()
                .map(|(s, _)| leak(&self.sql, s))
                .collect::<anyhow::Result<_>>()?;
            db.transaction(|db| {
                for (sql, (_, ps)) in leaked.iter().zip(stmts) {
                    db.query(sql, ps.iter().map(value), |_| Ok(true))?;
                }
                Ok(())
            })?;
            db.flusher()()?;
            Ok(())
        }

        fn settle(&self) -> anyhow::Result<()> {
            let loader = self
                .loader
                .lock()
                .map_err(|_| anyhow::anyhow!("poisoned"))?;
            let db = loader
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("the store has started"))?;
            // No reader exists yet, so nothing holds the log: the checkpoint copies all of it and
            // cuts the file, as SQLite's and Turso's settles do. A restart alone kept the load's 7.5
            // GB of log, whose pages then crowded the database out of the unit's OS cache.
            db.exec("PRAGMA wal_checkpoint(TRUNCATE)")
        }

        fn ready(&self) -> anyhow::Result<()> {
            let db = self
                .loader
                .lock()
                .map_err(|_| anyhow::anyhow!("poisoned"))?
                .take()
                .ok_or_else(|| anyhow::anyhow!("started twice"))?;
            let mut slot = Some(SqlEngine {
                db,
                dialect: self.dialect,
                reader_pages: self.reader_pages,
            });
            // As many readers as clients: a read never waits for another's reader.
            let defaults = Config::default();
            let config = Config {
                readers: self.readers,
                commit_rows: self.commit_rows.unwrap_or(defaults.commit_rows),
                ..defaults
            };
            let groups = Groups::start(1, config, &self.retired, |_| {
                slot.take().ok_or_else(|| anyhow::anyhow!("one group"))
            })?;
            self.groups
                .set(Arc::new(groups))
                .map_err(|_| anyhow::anyhow!("started twice"))?;
            Ok(())
        }

        fn session(&self) -> anyhow::Result<Box<dyn Session>> {
            let groups = self
                .groups
                .get()
                .ok_or_else(|| anyhow::anyhow!("not started"))?
                .clone();
            Ok(Box::new(AmphoraSession {
                groups,
                sql: self.sql.clone(),
                known: HashMap::new(),
            }))
        }

        fn instance_dropped_probe(&self) -> Option<Box<dyn Fn() -> bool + Send + Sync>> {
            let weak = self.db_weak.clone();
            Some(Box::new(move || weak.upgrade().is_none()))
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
                "pool": self.pool.as_ref().map(|p| {
                    let n = p.counts();
                    serde_json::json!({ "hits": n.hits, "misses": n.misses, "added": n.added, "held": p.len(), "capacity": p.capacity() })
                }),
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
        fn run(
            &mut self,
            kind: Kind,
            stmts: &[String],
            params: &[Vec<P>],
        ) -> anyhow::Result<usize> {
            let at = stmts.as_ptr() as usize;
            let leaked = match self.known.get(&at) {
                Some(l)
                    if l.len() == stmts.len()
                        && l.iter().zip(stmts).all(|(a, b)| *a == b.as_str()) =>
                {
                    l.clone()
                }
                _ => {
                    let l = Arc::new(
                        stmts
                            .iter()
                            .map(|s| leak(&self.sql, s))
                            .collect::<anyhow::Result<Vec<_>>>()?,
                    );
                    self.known.insert(at, l.clone());
                    l
                }
            };
            let until = Instant::now() + WAIT;
            match kind {
                Kind::Read => {
                    let (s1, p1) = (leaked.clone(), params.to_vec());
                    let (s2, p2) = (leaked, params.to_vec());
                    self.groups.read_committed(
                        "",
                        until,
                        move |db| read_all(db, &s1, &p1),
                        move |e: &mut SqlEngine| read_all(&mut e.db, &s2, &p2),
                    )
                }
                Kind::Write => {
                    let ticket = self.groups.put_wait(
                        vec![SqlRow {
                            stmts: leaked,
                            params: params.to_vec(),
                            rows_read: None,
                            answer_rows: None,
                        }],
                        until,
                    )?;
                    self.groups.durable(&ticket, until)?;
                    Ok(0)
                }
                Kind::Ddl => {
                    let rows_read = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                    let ticket = self.groups.put_wait(
                        vec![SqlRow {
                            stmts: leaked,
                            params: params.to_vec(),
                            rows_read: Some(Arc::clone(&rows_read)),
                            answer_rows: None,
                        }],
                        until,
                    )?;
                    self.groups.durable(&ticket, until)?;
                    Ok(rows_read.load(std::sync::atomic::Ordering::Acquire))
                }
            }
        }

        fn answer(
            &mut self,
            stmts: &[String],
            params: &[Vec<P>],
        ) -> anyhow::Result<Vec<Vec<Option<String>>>> {
            let leaked = Arc::new(
                stmts
                    .iter()
                    .map(|s| leak(&self.sql, s))
                    .collect::<anyhow::Result<Vec<_>>>()?,
            );
            let (s1, p1) = (leaked.clone(), params.to_vec());
            let (s2, p2) = (leaked, params.to_vec());
            self.groups.read_committed(
                "",
                Instant::now() + WAIT,
                move |db| answer_all(db, &s1, &p1),
                move |e: &mut SqlEngine| answer_all(&mut e.db, &s2, &p2),
            )
        }

        fn answer_ddl(
            &mut self,
            stmts: &[String],
            params: &[Vec<P>],
        ) -> anyhow::Result<Vec<Vec<Option<String>>>> {
            let leaked = Arc::new(
                stmts
                    .iter()
                    .map(|s| leak(&self.sql, s))
                    .collect::<anyhow::Result<Vec<_>>>()?,
            );
            let until = Instant::now() + WAIT;
            let slot = Arc::new(Mutex::new(None));
            let ticket = self.groups.put_wait(
                vec![SqlRow {
                    stmts: leaked,
                    params: params.to_vec(),
                    rows_read: None,
                    answer_rows: Some(Arc::clone(&slot)),
                }],
                until,
            )?;
            self.groups.durable(&ticket, until)?;
            slot.lock()
                .map_err(|_| anyhow::anyhow!("answer slot poisoned"))?
                .take()
                .ok_or_else(|| anyhow::anyhow!("DDL answer was not recorded"))
        }

        fn explain(&mut self, sql: &str, params: &[P]) -> anyhow::Result<Vec<String>> {
            // AMPHORA_EQP_JSON=1: the plan with the optimizer's estimates (EXPLAIN QUERY PLAN FORMAT=JSON).
            let format = if std::env::var("AMPHORA_EQP_JSON").ok().as_deref() == Some("1") {
                "FORMAT=JSON "
            } else {
                ""
            };
            let plan = leak(&self.sql, &format!("EXPLAIN QUERY PLAN {format}{sql}"))?;
            let p = params.to_vec();
            let lines = move |db: &mut Db| -> anyhow::Result<Vec<String>> {
                let mut out = Vec::new();
                db.query(plan, p.iter().map(value), |r| {
                    out.push(
                        r.get_values()
                            .map(|v| v.to_string())
                            .collect::<Vec<_>>()
                            .join(" | "),
                    );
                    Ok(true)
                })?;
                Ok(out)
            };
            let on_writer = lines.clone();
            self.groups.read_committed(
                "",
                Instant::now() + WAIT,
                lines,
                move |e: &mut SqlEngine| on_writer(&mut e.db),
            )
        }
    }
}

pub use amphora_target::{
    holding as amphora_holding, keep_holding as amphora_keep_holding, value as amphora_value,
};

/// How the bench's Amphora databases share the pages their connections read (docs/design/store.md,
/// decisions 20 and 26): `AMPHORA_SHARED_PAGES` unset or 0, none, as Amphora ships; `process`, the
/// process's pool (a quarter of the memory it may use); a number, a pool of that many pages. A
/// packed log (`AMPHORA_PACK_LOG=1`) shares none.
pub fn amphora_sharing() -> anyhow::Result<amphora::db::Sharing> {
    use amphora::db::{SharedPages, Sharing};
    if std::env::var("AMPHORA_PACK_LOG").ok().as_deref() == Some("1") {
        return Ok(Sharing::Off);
    }
    Ok(
        match std::env::var("AMPHORA_SHARED_PAGES").ok().as_deref() {
            Some("0") => Sharing::Off,
            None | Some("process") => Sharing::Process,
            Some(pages) => {
                let pages = pages.parse::<usize>().map_err(|e| {
                    anyhow::anyhow!(
                        "AMPHORA_SHARED_PAGES={pages}: {e} (a number of pages, 0 or process)"
                    )
                })?;
                Sharing::Pool(Arc::new(SharedPages::new(pages)))
            }
        },
    )
}
use amphora_target::Amphora;

// ---------------------------------------------------------------------------------------------
// Amphora's engine in MVCC (`PRAGMA journal_mode = 'mvcc'`): a connection a client, every write in
// `BEGIN CONCURRENT`, tried again when it conflicts with another, every commit flushed
// (`synchronous = FULL`). Measures the engine's concurrent writers on their own, without the store.

mod mvcc_target {
    use super::*;
    use amphora_engine::{
        ConnectOpts, Connection, Database, DatabaseOpts, IO, LimboError, OpenOptions, PlatformIO,
        SqliteDialect, Statement, StepResult, Value,
    };

    pub struct Mvcc {
        io: Arc<dyn IO>,
        db: Arc<Database>,
        /// The checkpointing thread's stop (AMPHORA_MVCC_CHECKPOINT_MS), set when the target goes.
        checkpointer: Option<Arc<crate::Stop>>,
    }

    impl Drop for Mvcc {
        fn drop(&mut self) {
            if let Some(stop) = &self.checkpointer {
                stop.stop();
            }
        }
    }

    impl Mvcc {
        pub fn open(dir: &str, reuse: bool) -> anyhow::Result<Self> {
            let path = suite_db(dir, reuse)?;
            let name = path
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("a path that isn't UTF-8"))?;
            let io: Arc<dyn IO> = Arc::new(PlatformIO::new().map_err(|e| anyhow::anyhow!("{e}"))?);
            let db = Database::open(
                io.clone(),
                name,
                OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new()),
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            // AMPHORA_MVCC_CHECKPOINT_MS: commits never checkpoint; a thread of its own does, this
            // often (unset: each commit past the log's threshold checkpoints, as the engine ships).
            let background = std::env::var("AMPHORA_MVCC_CHECKPOINT_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .map(Duration::from_millis);
            let mut m = Self {
                io,
                db,
                checkpointer: None,
            };
            let mut s = m.connect()?;
            let mut rows = 0;
            if let Err(Retry) = s.step("PRAGMA journal_mode = 'mvcc'", &[], &mut rows)? {
                anyhow::bail!("couldn't turn MVCC on");
            }
            if let Some(every) = background {
                if let Err(Retry) =
                    s.step("PRAGMA mvcc_checkpoint_threshold = -1", &[], &mut rows)?
                {
                    anyhow::bail!("couldn't turn inline checkpoints off");
                }
                let mut c = m.connect()?;
                let stop = Arc::new(crate::Stop::default());
                m.checkpointer = Some(stop.clone());
                // Every `every`, until the target goes: the period is the setting's, a timed wait on
                // the stop.
                std::thread::Builder::new()
                    .name("mvcc-checkpoint".into())
                    .spawn(move || {
                        while !stop.wait_until(Instant::now() + every) {
                            let mut rows = 0;
                            if let Err(e) =
                                c.step("PRAGMA wal_checkpoint(TRUNCATE)", &[], &mut rows)
                            {
                                eprintln!("mvcc checkpoint: {e:#}");
                            }
                        }
                    })?;
            }
            Ok(m)
        }

        fn connect(&self) -> anyhow::Result<MvccSession> {
            let conn = self
                .db
                .connect(ConnectOpts::new())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let mut s = MvccSession {
                io: self.io.clone(),
                conn,
                stmts: HashMap::new(),
            };
            let mut rows = 0;
            if let Err(Retry) = s.step("PRAGMA synchronous = FULL", &[], &mut rows)? {
                anyhow::bail!("couldn't set synchronous");
            }
            Ok(s)
        }
    }

    impl Target for Mvcc {
        fn dialect(&self) -> Dialect {
            Dialect::Sqlite
        }

        fn setup(&self, stmts: &[(String, Vec<P>)]) -> anyhow::Result<()> {
            let mut s = self.connect()?;
            let sql: Vec<String> = stmts.iter().map(|(s, _)| s.clone()).collect();
            let params: Vec<Vec<P>> = stmts.iter().map(|(_, p)| p.clone()).collect();
            s.transaction("BEGIN", &sql, &params)
        }

        fn session(&self) -> anyhow::Result<Box<dyn Session>> {
            Ok(Box::new(self.connect()?))
        }
    }

    pub struct MvccSession {
        io: Arc<dyn IO>,
        conn: Arc<Connection>,
        stmts: HashMap<String, Statement>,
    }

    /// A statement that met another writer (a conflict, a busy database): the transaction goes again.
    struct Retry;

    impl MvccSession {
        fn step(
            &mut self,
            sql: &str,
            params: &[P],
            rows: &mut usize,
        ) -> anyhow::Result<Result<(), Retry>> {
            if !self.stmts.contains_key(sql) {
                let stmt = self.conn.prepare(sql).map_err(|e| anyhow::anyhow!("{e}"))?;
                self.stmts.insert(sql.to_owned(), stmt);
            }
            let io = self.io.clone();
            let stmt = self
                .stmts
                .get_mut(sql)
                .ok_or_else(|| anyhow::anyhow!("lost a statement"))?;
            stmt.reset().map_err(|e| anyhow::anyhow!("{e}"))?;
            for (i, p) in params.iter().enumerate() {
                stmt.bind_at(
                    NonZero::new(i + 1).ok_or_else(|| anyhow::anyhow!("parameter 0"))?,
                    value(p),
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            let waker = crate::unpark_waker();
            loop {
                match stmt.step_with_waker(&waker) {
                    Ok(StepResult::Row) => {
                        if let Some(r) = stmt.row() {
                            for v in r.get_values() {
                                std::hint::black_box(v);
                            }
                        }
                        *rows += 1;
                    }
                    Ok(StepResult::Done) => return Ok(Ok(())),
                    Ok(StepResult::IO | StepResult::Yield) => {
                        io.step().map_err(|e| anyhow::anyhow!("{e}"))?
                    }
                    Ok(StepResult::Sleep { duration }) => {
                        // Busy: until the engine wakes this thread (the lock released) or its deadline; a wake for
                        // no reason only steps again.
                        std::thread::park_timeout(duration);
                        io.step().map_err(|e| anyhow::anyhow!("{e}"))?;
                    }
                    Ok(StepResult::Busy) => return Ok(Err(Retry)),
                    Err(
                        LimboError::Busy
                        | LimboError::BusySnapshot
                        | LimboError::WriteWriteConflict
                        | LimboError::CommitDependencyAborted
                        | LimboError::TxTerminated,
                    ) => {
                        return Ok(Err(Retry));
                    }
                    Ok(StepResult::Interrupt) => anyhow::bail!("interrupted"),
                    Err(e) => return Err(anyhow::anyhow!("{e}")),
                }
            }
        }

        /// `stmts` in one transaction begun by `begin`, tried again while it meets another writer.
        fn transaction(
            &mut self,
            begin: &str,
            stmts: &[String],
            params: &[Vec<P>],
        ) -> anyhow::Result<()> {
            let began = Instant::now();
            loop {
                let mut rows = 0;
                let attempt = (|| -> anyhow::Result<Result<(), Retry>> {
                    if let Err(r) = self.step(begin, &[], &mut rows)? {
                        return Ok(Err(r));
                    }
                    for (s, p) in stmts.iter().zip(params) {
                        if let Err(r) = self.step(s, p, &mut rows)? {
                            return Ok(Err(r));
                        }
                    }
                    self.step("COMMIT", &[], &mut rows)
                })();
                match attempt {
                    Ok(Ok(())) => return Ok(()),
                    Ok(Err(Retry)) => {
                        if !self.conn.get_auto_commit() {
                            let _ = self.step("ROLLBACK", &[], &mut rows);
                        }
                        if began.elapsed() > WAIT {
                            anyhow::bail!("the transaction kept meeting other writers");
                        }
                    }
                    Err(e) => {
                        if !self.conn.get_auto_commit() {
                            let _ = self.step("ROLLBACK", &[], &mut rows);
                        }
                        return Err(e);
                    }
                }
            }
        }
    }

    fn value(p: &P) -> Value {
        match p {
            P::I(i) => Value::from_i64(*i),
            P::F(f) => Value::from_f64(*f),
            P::T(s) => Value::from_text(s.clone()),
            P::B(b) => Value::from_slice(b).unwrap_or(Value::Null),
            P::Z(z) => Value::from_i64(i64::from(*z)),
            P::N => Value::Null,
        }
    }

    fn cell(v: &Value) -> Option<String> {
        match v {
            Value::Null => Cell::Null,
            Value::Numeric(amphora_engine::Numeric::Integer(i)) => Cell::Int(*i),
            Value::Numeric(amphora_engine::Numeric::Float(f)) => Cell::Real(f64::from(*f)),
            Value::Text(t) => Cell::Text(t.as_str()),
            Value::Blob(b) => Cell::Blob(b),
        }
        .text()
    }

    impl MvccSession {
        fn step_collect(
            &mut self,
            sql: &str,
            params: &[P],
            out: &mut Vec<Vec<Option<String>>>,
        ) -> anyhow::Result<Result<(), Retry>> {
            if !self.stmts.contains_key(sql) {
                let stmt = self.conn.prepare(sql).map_err(|e| anyhow::anyhow!("{e}"))?;
                self.stmts.insert(sql.to_owned(), stmt);
            }
            let io = self.io.clone();
            let stmt = self
                .stmts
                .get_mut(sql)
                .ok_or_else(|| anyhow::anyhow!("lost a statement"))?;
            stmt.reset().map_err(|e| anyhow::anyhow!("{e}"))?;
            for (i, p) in params.iter().enumerate() {
                stmt.bind_at(
                    NonZero::new(i + 1).ok_or_else(|| anyhow::anyhow!("parameter 0"))?,
                    value(p),
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            let waker = crate::unpark_waker();
            loop {
                match stmt.step_with_waker(&waker) {
                    Ok(StepResult::Row) => {
                        if let Some(r) = stmt.row() {
                            out.push(r.get_values().map(cell).collect());
                        }
                    }
                    Ok(StepResult::Done) => return Ok(Ok(())),
                    Ok(StepResult::IO | StepResult::Yield) => {
                        io.step().map_err(|e| anyhow::anyhow!("{e}"))?
                    }
                    Ok(StepResult::Sleep { duration }) => {
                        std::thread::park_timeout(duration);
                        io.step().map_err(|e| anyhow::anyhow!("{e}"))?;
                    }
                    Ok(StepResult::Busy) => return Ok(Err(Retry)),
                    Err(
                        LimboError::Busy
                        | LimboError::BusySnapshot
                        | LimboError::WriteWriteConflict
                        | LimboError::CommitDependencyAborted
                        | LimboError::TxTerminated,
                    ) => {
                        return Ok(Err(Retry));
                    }
                    Ok(StepResult::Interrupt) => anyhow::bail!("interrupted"),
                    Err(e) => return Err(anyhow::anyhow!("{e}")),
                }
            }
        }
    }

    impl Session for MvccSession {
        fn run(
            &mut self,
            kind: Kind,
            stmts: &[String],
            params: &[Vec<P>],
        ) -> anyhow::Result<usize> {
            match kind {
                Kind::Write => {
                    self.transaction("BEGIN CONCURRENT", stmts, params)?;
                    Ok(0)
                }
                Kind::Ddl => {
                    let out = self.answer_ddl(stmts, params)?;
                    Ok(out.len())
                }
                _ => {
                    let began = Instant::now();
                    'again: loop {
                        let mut rows = 0;
                        for (s, p) in stmts.iter().zip(params) {
                            if let Err(Retry) = self.step(s, p, &mut rows)? {
                                if began.elapsed() > WAIT {
                                    anyhow::bail!("the database stayed busy");
                                }
                                continue 'again;
                            }
                        }
                        return Ok(rows);
                    }
                }
            }
        }

        fn answer_ddl(
            &mut self,
            stmts: &[String],
            params: &[Vec<P>],
        ) -> anyhow::Result<Vec<Vec<Option<String>>>> {
            let began = Instant::now();
            loop {
                let mut rows = 0;
                let mut out = Vec::new();
                let attempt = (|| -> anyhow::Result<Result<(), Retry>> {
                    if let Err(r) = self.step("BEGIN CONCURRENT", &[], &mut rows)? {
                        return Ok(Err(r));
                    }
                    for (s, p) in stmts.iter().zip(params) {
                        let mut stmt = self.conn.prepare(s).map_err(|e| anyhow::anyhow!("{e}"))?;
                        for (i, param) in p.iter().enumerate() {
                            stmt.bind_at(
                                NonZero::new(i + 1)
                                    .ok_or_else(|| anyhow::anyhow!("parameter 0"))?,
                                value(param),
                            )
                            .map_err(|e| anyhow::anyhow!("{e}"))?;
                        }
                        let waker = crate::unpark_waker();
                        loop {
                            match stmt.step_with_waker(&waker) {
                                Ok(StepResult::Row) => {
                                    if let Some(r) = stmt.row() {
                                        out.push(r.get_values().map(cell).collect());
                                    }
                                }
                                Ok(StepResult::Done) => break,
                                Ok(StepResult::IO | StepResult::Yield) => {
                                    self.io.step().map_err(|e| anyhow::anyhow!("{e}"))?;
                                }
                                Ok(StepResult::Sleep { duration }) => {
                                    std::thread::park_timeout(duration);
                                    self.io.step().map_err(|e| anyhow::anyhow!("{e}"))?;
                                }
                                Ok(StepResult::Busy) => return Ok(Err(Retry)),
                                Err(
                                    LimboError::Busy
                                    | LimboError::BusySnapshot
                                    | LimboError::WriteWriteConflict
                                    | LimboError::CommitDependencyAborted
                                    | LimboError::TxTerminated,
                                ) => return Ok(Err(Retry)),
                                Ok(StepResult::Interrupt) => anyhow::bail!("interrupted"),
                                Err(e) => return Err(anyhow::anyhow!("{e}")),
                            }
                        }
                    }
                    self.step("COMMIT", &[], &mut rows)
                })();
                match attempt {
                    Ok(Ok(())) => return Ok(out),
                    Ok(Err(Retry)) => {
                        if !self.conn.get_auto_commit() {
                            let _ = self.step("ROLLBACK", &[], &mut rows);
                        }
                        if began.elapsed() > WAIT {
                            anyhow::bail!("the transaction kept meeting other writers");
                        }
                    }
                    Err(e) => {
                        if !self.conn.get_auto_commit() {
                            let _ = self.step("ROLLBACK", &[], &mut rows);
                        }
                        return Err(e);
                    }
                }
            }
        }

        fn answer(
            &mut self,
            stmts: &[String],
            params: &[Vec<P>],
        ) -> anyhow::Result<Vec<Vec<Option<String>>>> {
            let began = Instant::now();
            'again: loop {
                let mut out = Vec::new();
                for (s, p) in stmts.iter().zip(params) {
                    if let Err(Retry) = self.step_collect(s, p, &mut out)? {
                        if began.elapsed() > WAIT {
                            anyhow::bail!("the database stayed busy");
                        }
                        continue 'again;
                    }
                }
                return Ok(out);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Turso (upstream)

struct Turso {
    io: Arc<dyn turso_core::IO>,
    db: Arc<turso_core::Database>,
}

impl Turso {
    fn open(dir: &str, reuse: bool) -> anyhow::Result<Self> {
        let path = suite_db(dir, reuse)?;
        let name = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("a path that isn't UTF-8"))?;
        let io: Arc<dyn turso_core::IO> =
            Arc::new(turso_core::PlatformIO::new().map_err(|e| anyhow::anyhow!("{e}"))?);
        let db = turso_core::Database::open(
            io.clone(),
            name,
            turso_core::OpenOptions::new(Arc::new(turso_core::SqliteDialect))
                .db_opts(turso_core::DatabaseOpts::new()),
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(Self { io, db })
    }

    fn connect(&self) -> anyhow::Result<TursoSession> {
        let conn = self
            .db
            .connect(turso_core::ConnectOpts::new())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        conn.set_busy_timeout(WAIT);
        Ok(TursoSession {
            io: self.io.clone(),
            conn,
            stmts: HashMap::new(),
        })
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

    fn settle(&self) -> anyhow::Result<()> {
        let mut s = self.connect()?;
        let mut rows = 0;
        s.step("PRAGMA wal_checkpoint(TRUNCATE)", &[], &mut rows)
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
    fn step(&mut self, sql: &str, params: &[P], rows: &mut usize) -> anyhow::Result<()> {
        if !self.stmts.contains_key(sql) {
            let stmt = self.conn.prepare(sql).map_err(|e| anyhow::anyhow!("{e}"))?;
            self.stmts.insert(sql.to_owned(), stmt);
        }
        let io = self.io.clone();
        let stmt = self
            .stmts
            .get_mut(sql)
            .ok_or_else(|| anyhow::anyhow!("lost a statement"))?;
        stmt.reset().map_err(|e| anyhow::anyhow!("{e}"))?;
        for (i, p) in params.iter().enumerate() {
            stmt.bind_at(
                NonZero::new(i + 1).ok_or_else(|| anyhow::anyhow!("parameter 0"))?,
                turso_value(p),
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;
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
                Ok(turso_core::StepResult::Done) => return Ok(()),
                // Sleep is the busy handler's backoff: Turso's own SDK steps again through it
                // (sdk-kit's `step_inner`), so its handler does the waiting.
                Ok(
                    turso_core::StepResult::IO
                    | turso_core::StepResult::Yield
                    | turso_core::StepResult::Sleep { .. },
                ) => io.step().map_err(|e| anyhow::anyhow!("{e}"))?,
                Ok(turso_core::StepResult::Busy) => anyhow::bail!("busy past the busy timeout"),
                Ok(turso_core::StepResult::Interrupt) => anyhow::bail!("interrupted"),
                Err(e) => return Err(anyhow::anyhow!("{e}")),
            }
        }
    }

    /// `stmts` in one transaction.
    fn transaction(&mut self, stmts: &[String], params: &[Vec<P>]) -> anyhow::Result<()> {
        let mut rows = 0;
        let done = (|| -> anyhow::Result<()> {
            self.step("BEGIN IMMEDIATE", &[], &mut rows)?;
            for (s, p) in stmts.iter().zip(params) {
                self.step(s, p, &mut rows)?;
            }
            self.step("COMMIT", &[], &mut rows)
        })();
        if done.is_err() {
            let _ = self.step("ROLLBACK", &[], &mut rows);
        }
        done
    }
}

fn turso_cell(v: &turso_core::Value) -> Option<String> {
    match v {
        turso_core::Value::Null => Cell::Null,
        turso_core::Value::Numeric(turso_core::Numeric::Integer(i)) => Cell::Int(*i),
        turso_core::Value::Numeric(turso_core::Numeric::Float(f)) => Cell::Real(f64::from(*f)),
        turso_core::Value::Text(t) => Cell::Text(t.as_str()),
        turso_core::Value::Blob(b) => Cell::Blob(b),
    }
    .text()
}

impl Session for TursoSession {
    fn answer(
        &mut self,
        stmts: &[String],
        params: &[Vec<P>],
    ) -> anyhow::Result<Vec<Vec<Option<String>>>> {
        let mut out = Vec::new();
        for (sql, ps) in stmts.iter().zip(params) {
            let mut stmt = self.conn.prepare(sql).map_err(|e| anyhow::anyhow!("{e}"))?;
            for (i, p) in ps.iter().enumerate() {
                stmt.bind_at(
                    NonZero::new(i + 1).ok_or_else(|| anyhow::anyhow!("parameter 0"))?,
                    turso_value(p),
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            loop {
                match stmt.step().map_err(|e| anyhow::anyhow!("{e}"))? {
                    turso_core::StepResult::Row => {
                        if let Some(r) = stmt.row() {
                            out.push(r.get_values().map(turso_cell).collect());
                        }
                    }
                    turso_core::StepResult::Done => break,
                    turso_core::StepResult::IO | turso_core::StepResult::Yield => {
                        self.io.step().map_err(|e| anyhow::anyhow!("{e}"))?
                    }
                    other => anyhow::bail!("the answer stopped: {other:?}"),
                }
            }
        }
        Ok(out)
    }

    fn explain(&mut self, sql: &str, params: &[P]) -> anyhow::Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare(format!("EXPLAIN QUERY PLAN {sql}"))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        for (i, p) in params.iter().enumerate() {
            stmt.bind_at(
                NonZero::new(i + 1).ok_or_else(|| anyhow::anyhow!("parameter 0"))?,
                turso_value(p),
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        let mut out = Vec::new();
        loop {
            match stmt.step().map_err(|e| anyhow::anyhow!("{e}"))? {
                turso_core::StepResult::Row => {
                    if let Some(r) = stmt.row() {
                        out.push(
                            r.get_values()
                                .map(|v| v.to_string())
                                .collect::<Vec<_>>()
                                .join(" | "),
                        );
                    }
                }
                turso_core::StepResult::Done => return Ok(out),
                turso_core::StepResult::IO | turso_core::StepResult::Yield => {
                    self.io.step().map_err(|e| anyhow::anyhow!("{e}"))?
                }
                other => anyhow::bail!("EXPLAIN stopped: {other:?}"),
            }
        }
    }

    fn answer_ddl(
        &mut self,
        stmts: &[String],
        params: &[Vec<P>],
    ) -> anyhow::Result<Vec<Vec<Option<String>>>> {
        let mut dummy = 0;
        let done = (|| -> anyhow::Result<Vec<Vec<Option<String>>>> {
            self.step("BEGIN IMMEDIATE", &[], &mut dummy)?;
            let out = self.answer(stmts, params)?;
            self.step("COMMIT", &[], &mut dummy)?;
            Ok(out)
        })();
        if done.is_err() {
            let _ = self.step("ROLLBACK", &[], &mut dummy);
        }
        done
    }

    fn run(&mut self, kind: Kind, stmts: &[String], params: &[Vec<P>]) -> anyhow::Result<usize> {
        match (kind, stmts) {
            (Kind::Write, [_, _, ..]) => self.transaction(stmts, params).map(|()| 0),
            (Kind::Ddl, _) => self.answer_ddl(stmts, params).map(|rows| rows.len()),
            _ => {
                let mut rows = 0;
                for (s, p) in stmts.iter().zip(params) {
                    self.step(s, p, &mut rows)?;
                }
                Ok(rows)
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
    fn open(dir: &str, reuse: bool) -> anyhow::Result<Self> {
        Ok(Self {
            path: suite_db(dir, reuse)?,
        })
    }

    fn connect(&self) -> anyhow::Result<rusqlite::Connection> {
        let c = rusqlite::Connection::open(&self.path)?;
        c.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;")?;
        c.busy_timeout(WAIT)?;
        Ok(c)
    }
}

pub fn sqlite_value(p: &P) -> rusqlite::types::Value {
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

    fn settle(&self) -> anyhow::Result<()> {
        let c = self.connect()?;
        sqlite_rows(&c, "PRAGMA wal_checkpoint(TRUNCATE)", &[])?;
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
    fn answer(
        &mut self,
        stmts: &[String],
        params: &[Vec<P>],
    ) -> anyhow::Result<Vec<Vec<Option<String>>>> {
        use rusqlite::types::ValueRef;
        let mut out = Vec::new();
        for (sql, ps) in stmts.iter().zip(params) {
            let mut stmt = self.c.prepare_cached(sql)?;
            let columns = stmt.column_count();
            let mut rows = stmt.query(rusqlite::params_from_iter(ps.iter().map(sqlite_value)))?;
            while let Some(r) = rows.next()? {
                let mut row = Vec::with_capacity(columns);
                for col in 0..columns {
                    row.push(
                        match r.get_ref(col)? {
                            ValueRef::Null => Cell::Null,
                            ValueRef::Integer(i) => Cell::Int(i),
                            ValueRef::Real(f) => Cell::Real(f),
                            ValueRef::Text(t) => Cell::Text(std::str::from_utf8(t)?),
                            ValueRef::Blob(b) => Cell::Blob(b),
                        }
                        .text(),
                    );
                }
                out.push(row);
            }
        }
        Ok(out)
    }

    fn answer_ddl(
        &mut self,
        stmts: &[String],
        params: &[Vec<P>],
    ) -> anyhow::Result<Vec<Vec<Option<String>>>> {
        use rusqlite::types::ValueRef;
        let mut out = Vec::new();
        let tx = self
            .c
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for (sql, ps) in stmts.iter().zip(params) {
            let mut stmt = tx.prepare(sql)?;
            let columns = stmt.column_count();
            let mut rows = stmt.query(rusqlite::params_from_iter(ps.iter().map(sqlite_value)))?;
            while let Some(r) = rows.next()? {
                let mut row = Vec::with_capacity(columns);
                for col in 0..columns {
                    row.push(
                        match r.get_ref(col)? {
                            ValueRef::Null => Cell::Null,
                            ValueRef::Integer(i) => Cell::Int(i),
                            ValueRef::Real(f) => Cell::Real(f),
                            ValueRef::Text(t) => Cell::Text(std::str::from_utf8(t)?),
                            ValueRef::Blob(b) => Cell::Blob(b),
                        }
                        .text(),
                    );
                }
                out.push(row);
            }
        }
        tx.commit()?;
        Ok(out)
    }

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
        if kind == Kind::Ddl {
            let mut n = 0usize;
            if stmts.len() > 1 {
                let tx = self
                    .c
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                for (s, p) in stmts.iter().zip(params) {
                    let mut stmt = tx.prepare(s)?;
                    let columns = stmt.column_count();
                    let mut rows =
                        stmt.query(rusqlite::params_from_iter(p.iter().map(sqlite_value)))?;
                    while let Some(r) = rows.next()? {
                        for i in 0..columns {
                            std::hint::black_box(r.get_ref(i)?);
                        }
                        n += 1;
                    }
                }
                tx.commit()?;
            } else {
                for (s, p) in stmts.iter().zip(params) {
                    let mut stmt = self.c.prepare(s)?;
                    let columns = stmt.column_count();
                    let mut rows =
                        stmt.query(rusqlite::params_from_iter(p.iter().map(sqlite_value)))?;
                    while let Some(r) = rows.next()? {
                        for i in 0..columns {
                            std::hint::black_box(r.get_ref(i)?);
                        }
                        n += 1;
                    }
                }
            }
            return Ok(n);
        }
        if kind == Kind::Write && stmts.len() > 1 {
            let tx = self
                .c
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
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
    schema: Option<String>,
}

/// A column's bytes as the server sent them: every column taken, whatever its type.
struct Raw(usize);

impl<'a> postgres::types::FromSql<'a> for Raw {
    fn from_sql(
        _: &postgres::types::Type,
        raw: &'a [u8],
    ) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(Raw(raw.len()))
    }

    fn accepts(_: &postgres::types::Type) -> bool {
        true
    }
}

/// A column's value decoded by its type for the answer check: a boolean as SQLite keeps one (1 or
/// 0), NUMERIC in decimal, JSONB as its text.
struct PgCell(Option<String>);

impl<'a> postgres::types::FromSql<'a> for PgCell {
    fn from_sql(
        ty: &postgres::types::Type,
        raw: &'a [u8],
    ) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        use postgres::types::Type;
        let cell = match *ty {
            Type::BOOL => Cell::Int(i64::from(<bool as postgres::types::FromSql>::from_sql(
                ty, raw,
            )?)),
            Type::INT2 => Cell::Int(i64::from(<i16 as postgres::types::FromSql>::from_sql(
                ty, raw,
            )?)),
            Type::INT4 => Cell::Int(i64::from(<i32 as postgres::types::FromSql>::from_sql(
                ty, raw,
            )?)),
            Type::INT8 => Cell::Int(<i64 as postgres::types::FromSql>::from_sql(ty, raw)?),
            Type::FLOAT4 => Cell::Real(f64::from(<f32 as postgres::types::FromSql>::from_sql(
                ty, raw,
            )?)),
            Type::FLOAT8 => Cell::Real(<f64 as postgres::types::FromSql>::from_sql(ty, raw)?),
            Type::NUMERIC => {
                return Ok(PgCell(Some(
                    numeric_text(raw).ok_or("a NUMERIC cut short")?,
                )));
            }
            Type::BYTEA => Cell::Blob(raw),
            // JSONB's binary form is a version byte, then the document's text.
            Type::JSONB => Cell::Text(std::str::from_utf8(raw.get(1..).unwrap_or_default())?),
            _ => Cell::Text(std::str::from_utf8(raw)?),
        };
        Ok(PgCell(cell.text()))
    }

    fn from_sql_null(
        _: &postgres::types::Type,
    ) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(PgCell(None))
    }

    fn accepts(_: &postgres::types::Type) -> bool {
        true
    }
}

/// NUMERIC's binary form (PostgreSQL's numeric.c, `numeric_send`) in decimal: digit count, weight
/// of the first base-10000 digit, sign, display scale, then the digits.
fn numeric_text(raw: &[u8]) -> Option<String> {
    let u = |i: usize| {
        raw.get(i..i + 2)
            .map(|b| b.iter().fold(0u16, |n, &x| (n << 8) | u16::from(x)))
    };
    let (n, weight, sign, scale) = (
        usize::from(u(0)?),
        u(2)?.cast_signed(),
        u(4)?,
        usize::from(u(6)?),
    );
    match sign {
        0xC000 => return Some("NaN".into()),
        0xD000 => return Some("Infinity".into()),
        0xF000 => return Some("-Infinity".into()),
        _ => {}
    }
    let digits: Vec<u16> = (0..n).map(|i| u(8 + 2 * i)).collect::<Option<_>>()?;
    // The digit at exponent `e` (base 10000) sits at index weight - e.
    let at = |e: i16| {
        usize::try_from(weight.checked_sub(e)?)
            .ok()
            .and_then(|i| digits.get(i).copied())
            .or(Some(0))
    };
    let mut s = String::new();
    if sign == 0x4000 {
        s.push('-');
    }
    if weight < 0 {
        s.push('0');
    }
    for e in (0..=weight).rev() {
        let d = at(e)?;
        if e == weight {
            s.push_str(&d.to_string());
        } else {
            s.push_str(&format!("{d:04}"));
        }
    }
    if scale > 0 {
        let mut frac = String::new();
        let mut e: i16 = -1;
        while frac.len() < scale {
            frac.push_str(&format!("{:04}", at(e)?));
            e = e.checked_sub(1)?;
        }
        frac.truncate(scale);
        s.push('.');
        s.push_str(&frac);
    }
    Some(s)
}

/// Type-agnostic SQL `NULL` parameter for `rust-postgres`, accepting any target column type
/// (`BIGINT`, `DOUBLE PRECISION`, `TEXT`, `BYTEA`, `JSONB`, `BOOLEAN`, ...).
#[derive(Debug)]
struct PgNull;

impl postgres::types::ToSql for PgNull {
    fn to_sql(
        &self,
        _ty: &postgres::types::Type,
        _out: &mut postgres::types::private::BytesMut,
    ) -> Result<postgres::types::IsNull, Box<dyn std::error::Error + Sync + Send>> {
        Ok(postgres::types::IsNull::Yes)
    }

    fn accepts(_ty: &postgres::types::Type) -> bool {
        true
    }

    postgres::types::to_sql_checked!();
}

/// Integer parameter for `rust-postgres` accepting `INT2`, `INT4`, and `INT8` columns/literals
/// (so `SELECT 1` in recursive CTEs and `INTEGER PRIMARY KEY` scratch DDL tables serialize cleanly
/// alongside `BIGINT` columns).
#[derive(Debug)]
struct PgInt(i64);

impl postgres::types::ToSql for PgInt {
    fn to_sql(
        &self,
        ty: &postgres::types::Type,
        out: &mut postgres::types::private::BytesMut,
    ) -> Result<postgres::types::IsNull, Box<dyn std::error::Error + Sync + Send>> {
        use postgres::types::Type;
        match *ty {
            Type::INT2 => i16::try_from(self.0)?.to_sql(ty, out),
            Type::INT4 => i32::try_from(self.0)?.to_sql(ty, out),
            Type::INT8 => self.0.to_sql(ty, out),
            _ => Err(format!("cannot convert PgInt to Postgres type {ty}").into()),
        }
    }

    fn accepts(ty: &postgres::types::Type) -> bool {
        matches!(
            *ty,
            postgres::types::Type::INT2 | postgres::types::Type::INT4 | postgres::types::Type::INT8
        )
    }

    postgres::types::to_sql_checked!();
}

fn pg_params(params: &[P]) -> Vec<Box<dyn postgres::types::ToSql + Sync>> {
    params
        .iter()
        .map(|p| -> Box<dyn postgres::types::ToSql + Sync> {
            match p {
                P::I(i) => Box::new(PgInt(*i)),
                P::F(f) => Box::new(*f),
                P::T(s) => Box::new(s.clone()),
                P::B(b) => Box::new(b.clone()),
                P::Z(z) => Box::new(*z),
                P::N => Box::new(PgNull),
            }
        })
        .collect()
}

fn pg_search_path_sql(schema: &str) -> String {
    // Exclude `public` so `--reuse` reads and unqualified writes can never silently hit or mutate
    // user tables in `public`.
    format!("SET search_path TO {schema}")
}

impl Pg {
    fn open(dsn: &str, kind: PgKind, schema: Option<&str>) -> anyhow::Result<Self> {
        let pg = Self {
            dsn: dsn.to_owned(),
            kind,
            schema: schema.map(str::to_owned),
        };
        let mut c = pg.client()?;
        if kind == PgKind::Orioledb {
            c.batch_execute("CREATE EXTENSION IF NOT EXISTS orioledb")?;
        }
        Ok(pg)
    }

    fn client(&self) -> anyhow::Result<postgres::Client> {
        let mut c = postgres::Client::connect(&self.dsn, postgres::NoTls)?;
        if self.kind == PgKind::Cedardb {
            c.batch_execute("SET async_commit = off")?;
        }
        if let Some(schema) = &self.schema {
            c.batch_execute(&pg_search_path_sql(schema))?;
        }
        Ok(c)
    }
}

fn pg_runs_outside_tx(sql: &str, params: &[P]) -> bool {
    params.is_empty() && sql.trim_start().to_ascii_uppercase().starts_with("VACUUM")
}

impl Target for Pg {
    fn dialect(&self) -> Dialect {
        Dialect::Postgres
    }

    fn setup(&self, stmts: &[(String, Vec<P>)]) -> anyhow::Result<()> {
        let mut c = self.client()?;
        let mut idx = 0usize;
        while idx < stmts.len() {
            if pg_runs_outside_tx(&stmts[idx].0, &stmts[idx].1) {
                c.batch_execute(&stmts[idx].0)?;
                idx += 1;
                continue;
            }
            let mut tx = c.transaction()?;
            while idx < stmts.len() && !pg_runs_outside_tx(&stmts[idx].0, &stmts[idx].1) {
                let (s, p) = &stmts[idx];
                let s = if self.kind == PgKind::Orioledb && s.starts_with("CREATE TABLE") {
                    format!("{s} USING orioledb")
                } else {
                    s.clone()
                };
                let s = crate::suite::placeholders(Dialect::Postgres, &s);
                let owned = pg_params(p);
                let refs: Vec<&(dyn postgres::types::ToSql + Sync)> =
                    owned.iter().map(|b| b.as_ref()).collect();
                tx.execute(s.as_str(), &refs).map_err(|e| {
                    anyhow::anyhow!("{e} (in: {})", s.chars().take(200).collect::<String>())
                })?;
                idx += 1;
            }
            tx.commit()?;
        }
        Ok(())
    }

    fn settle(&self) -> anyhow::Result<()> {
        self.client()?.batch_execute("CHECKPOINT")?;
        Ok(())
    }

    fn session(&self) -> anyhow::Result<Box<dyn Session>> {
        Ok(Box::new(PgSession {
            c: self.client()?,
            stmts: HashMap::new(),
        }))
    }
}

struct PgSession {
    c: postgres::Client,
    stmts: HashMap<String, postgres::Statement>,
}

impl PgSession {
    fn statement(&mut self, sql: &str) -> anyhow::Result<postgres::Statement> {
        let translated = crate::suite::placeholders(Dialect::Postgres, sql);
        if let Some(s) = self.stmts.get(&translated) {
            return Ok(s.clone());
        }
        let s = self.c.prepare(&translated)?;
        self.stmts.insert(translated, s.clone());
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

fn pg_is_savepoint_control(sql: &str) -> bool {
    let upper = sql.trim_start().to_ascii_uppercase();
    upper.starts_with("SAVEPOINT ")
        || upper.starts_with("ROLLBACK TO ")
        || upper.starts_with("RELEASE ")
}

impl Session for PgSession {
    fn answer(
        &mut self,
        stmts: &[String],
        params: &[Vec<P>],
    ) -> anyhow::Result<Vec<Vec<Option<String>>>> {
        let mut out = Vec::new();
        for (sql, ps) in stmts.iter().zip(params) {
            let s = self.statement(sql)?;
            let owned = pg_params(ps);
            let refs: Vec<&(dyn postgres::types::ToSql + Sync)> =
                owned.iter().map(|b| b.as_ref()).collect();
            for r in self.c.query(&s, &refs)? {
                out.push(
                    (0..r.len())
                        .map(|i| r.try_get::<_, PgCell>(i).map(|c| c.0))
                        .collect::<Result<_, _>>()?,
                );
            }
        }
        Ok(out)
    }

    fn answer_ddl(
        &mut self,
        stmts: &[String],
        params: &[Vec<P>],
    ) -> anyhow::Result<Vec<Vec<Option<String>>>> {
        let mut out = Vec::new();
        let mut tx = self.c.transaction()?;
        for (sql, ps) in stmts.iter().zip(params) {
            if ps.is_empty() && pg_is_savepoint_control(sql) {
                tx.batch_execute(sql)?;
                continue;
            }
            let translated = crate::suite::placeholders(Dialect::Postgres, sql);
            let owned = pg_params(ps);
            let refs: Vec<&(dyn postgres::types::ToSql + Sync)> =
                owned.iter().map(|b| b.as_ref()).collect();
            for r in tx.query(translated.as_str(), &refs)? {
                out.push(
                    (0..r.len())
                        .map(|i| r.try_get::<_, PgCell>(i).map(|c| c.0))
                        .collect::<Result<_, _>>()?,
                );
            }
        }
        tx.commit()?;
        Ok(out)
    }

    fn explain(&mut self, sql: &str, params: &[P]) -> anyhow::Result<Vec<String>> {
        let translated = crate::suite::placeholders(Dialect::Postgres, sql);
        let owned = pg_params(params);
        let refs: Vec<&(dyn postgres::types::ToSql + Sync)> =
            owned.iter().map(|b| b.as_ref()).collect();
        let rows = self
            .c
            .query(format!("EXPLAIN {translated}").as_str(), &refs)?;
        rows.iter()
            .map(|r| r.try_get::<_, String>(0).map_err(Into::into))
            .collect()
    }

    fn run(&mut self, kind: Kind, stmts: &[String], params: &[Vec<P>]) -> anyhow::Result<usize> {
        if kind == Kind::Ddl {
            let mut n = 0usize;
            let mut tx = self.c.transaction()?;
            for (s, p) in stmts.iter().zip(params) {
                if p.is_empty() && pg_is_savepoint_control(s) {
                    tx.batch_execute(s)?;
                    continue;
                }
                let translated = crate::suite::placeholders(Dialect::Postgres, s);
                let owned = pg_params(p);
                let refs: Vec<&(dyn postgres::types::ToSql + Sync)> =
                    owned.iter().map(|b| b.as_ref()).collect();
                n += pg_rows(&tx.query(translated.as_str(), &refs)?)?;
            }
            tx.commit()?;
            return Ok(n);
        }
        if kind == Kind::Write && stmts.len() > 1 {
            let prepared: Vec<Option<postgres::Statement>> = stmts
                .iter()
                .map(|s| {
                    if pg_is_savepoint_control(s) {
                        Ok(None)
                    } else {
                        self.statement(s).map(Some)
                    }
                })
                .collect::<anyhow::Result<_>>()?;
            let mut tx = self.c.transaction()?;
            for ((raw, stmt), p) in stmts.iter().zip(&prepared).zip(params) {
                match stmt {
                    Some(s) => {
                        let owned = pg_params(p);
                        let refs: Vec<&(dyn postgres::types::ToSql + Sync)> =
                            owned.iter().map(|b| b.as_ref()).collect();
                        pg_rows(&tx.query(s, &refs)?)?;
                    }
                    None => {
                        tx.batch_execute(raw)?;
                    }
                }
            }
            tx.commit()?;
            return Ok(0);
        }
        let prepared: Vec<postgres::Statement> = stmts
            .iter()
            .map(|s| self.statement(s))
            .collect::<anyhow::Result<_>>()?;
        let mut n = 0;
        for (s, p) in prepared.iter().zip(params) {
            let owned = pg_params(p);
            let refs: Vec<&(dyn postgres::types::ToSql + Sync)> =
                owned.iter().map(|b| b.as_ref()).collect();
            n += pg_rows(&self.c.query(s, &refs)?)?;
        }
        Ok(n)
    }
}

// ---------------------------------------------------------------------------------------------
// MySQL

struct MySql {
    pool: mysql::Pool,
    max_connections: usize,
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
fn mysql_rows(
    q: &mut impl mysql::prelude::Queryable,
    sql: &str,
    params: &[P],
) -> anyhow::Result<usize> {
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
        let max_connections = connections.max(2);
        let constraints = mysql::PoolConstraints::new(1, max_connections)
            .ok_or_else(|| anyhow::anyhow!("bad pool size"))?;
        let opts = mysql::OptsBuilder::from_opts(mysql::Opts::from_url(dsn)?)
            .pool_opts(mysql::PoolOpts::default().with_constraints(constraints));
        let pool = mysql::Pool::new(opts)?;
        let _ = pool.get_conn()?;
        Ok(Self {
            pool,
            max_connections,
        })
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
            mysql_rows(&mut tx, s, p).map_err(|e| {
                anyhow::anyhow!("{e} (in: {})", s.chars().take(200).collect::<String>())
            })?;
        }
        tx.commit()?;
        Ok(())
    }

    fn session(&self) -> anyhow::Result<Box<dyn Session>> {
        Ok(Box::new(MySqlSession {
            c: self.pool.get_conn()?,
        }))
    }

    fn max_sessions(&self) -> Option<usize> {
        Some(self.max_connections)
    }
}

struct MySqlSession {
    c: mysql::PooledConn,
}

impl Session for MySqlSession {
    fn explain(&mut self, sql: &str, params: &[P]) -> anyhow::Result<Vec<String>> {
        use mysql::prelude::Queryable;
        let rows: Vec<String> = self
            .c
            .exec(format!("EXPLAIN FORMAT=TREE {sql}"), mysql_params(params))?;
        Ok(rows
            .iter()
            .flat_map(|r| r.lines().map(str::to_owned).collect::<Vec<_>>())
            .collect())
    }

    fn run(&mut self, kind: Kind, stmts: &[String], params: &[Vec<P>]) -> anyhow::Result<usize> {
        if matches!(kind, Kind::Write | Kind::Ddl) && stmts.len() > 1 {
            let mut n = 0;
            let mut tx = self.c.start_transaction(mysql::TxOpts::default())?;
            for (s, p) in stmts.iter().zip(params) {
                n += mysql_rows(&mut tx, s, p)?;
            }
            tx.commit()?;
            return Ok(if kind == Kind::Ddl { n } else { 0 });
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
        let ch = Self {
            agent,
            url: format!("{}/", url.trim_end_matches('/')),
        };
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
        anyhow::bail!(
            "ClickHouse said {status}: {}",
            text.trim().chars().take(300).collect::<String>()
        );
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
        Ok(Box::new(ClickHouseSession {
            agent: self.agent.clone(),
            url: self.url.clone(),
        }))
    }
}

struct ClickHouseSession {
    agent: ureq::Agent,
    url: String,
}

impl Session for ClickHouseSession {
    fn explain(&mut self, sql: &str, params: &[P]) -> anyhow::Result<Vec<String>> {
        let out = post(
            &self.agent,
            &self.url,
            &format!("EXPLAIN indexes = 1 {} FORMAT TSV", inline(sql, params)),
        )?;
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

#[cfg(test)]
mod tests {
    use super::{Cell, numeric_text};

    #[test]
    fn answer_values_keep_reals_apart_from_integers() {
        assert_eq!(Cell::Real(50.0).text().as_deref(), Some("50.0"));
        assert_eq!(Cell::Real(-3.0).text().as_deref(), Some("-3.0"));
        assert_eq!(Cell::Real(49.5).text().as_deref(), Some("49.5"));
        assert_eq!(Cell::Int(50).text().as_deref(), Some("50"));
        assert_eq!(Cell::Text("a b").text().as_deref(), Some("a b"));
        assert_eq!(Cell::Blob(&[0, 255]).text().as_deref(), Some("x'00ff'"));
        assert_eq!(Cell::Null.text(), None);
    }

    #[test]
    fn numeric_binary_reads_as_its_decimal() {
        // 12345.6780: 3 digits, weight 1, positive, scale 4; digits 1, 2345, 6780.
        let raw = [0, 3, 0, 1, 0, 0, 0, 4, 0, 1, 0x09, 0x29, 0x1a, 0x7c];
        assert_eq!(numeric_text(&raw).as_deref(), Some("12345.6780"));
        // -0.00001234: 1 digit, weight -2, negative, scale 8; digit 1234.
        let raw = [0, 1, 0xff, 0xfe, 0x40, 0, 0, 8, 0x04, 0xd2];
        assert_eq!(numeric_text(&raw).as_deref(), Some("-0.00001234"));
        // 10000: 1 digit, weight 1, scale 0: the trailing zero digit isn't sent.
        let raw = [0, 1, 0, 1, 0, 0, 0, 0, 0, 1];
        assert_eq!(numeric_text(&raw).as_deref(), Some("10000"));
        // 0 at scale 2: no digits.
        let raw = [0, 0, 0, 0, 0, 0, 0, 2];
        assert_eq!(numeric_text(&raw).as_deref(), Some("0.00"));
        assert_eq!(numeric_text(&[0, 1, 0, 0]), None);
    }

    #[test]
    fn review_p1_fresh_preserves_unrelated_files_in_dir() {
        let dir = tempfile::tempdir().unwrap();
        let sentinel = dir.path().join("keep_me.txt");
        let backup_dir = dir.path().join("suite.db.backup");
        let backup_file = backup_dir.join("snapshot.bin");
        let notes = dir.path().join("suite.db.notes");
        let old_db = dir.path().join("suite.db");
        let old_wal = dir.path().join("suite.db-wal");
        let old_wal2 = dir.path().join("suite.db-wal2");
        let old_log = dir.path().join("suite.db-log");
        let old_retired = dir.path().join("suite.retired");
        std::fs::create_dir_all(&backup_dir).unwrap();
        std::fs::write(&backup_file, b"keep backup").unwrap();
        std::fs::write(&notes, b"keep notes").unwrap();
        std::fs::write(&sentinel, b"do not delete").unwrap();
        std::fs::write(&old_db, b"old db").unwrap();
        std::fs::write(&old_wal, b"old wal").unwrap();
        std::fs::write(&old_wal2, b"old wal2").unwrap();
        std::fs::write(&old_log, b"old log").unwrap();
        std::fs::write(&old_retired, b"old retired").unwrap();

        let path = super::fresh(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(path, old_db);
        assert!(
            sentinel.exists(),
            "fresh() must never delete unrelated files"
        );
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"do not delete");
        assert!(
            backup_file.exists(),
            "fresh() must never delete suite.db.backup/ directory"
        );
        assert_eq!(std::fs::read(&backup_file).unwrap(), b"keep backup");
        assert!(
            notes.exists(),
            "fresh() must never delete suite.db.notes file"
        );
        assert_eq!(std::fs::read(&notes).unwrap(), b"keep notes");
        assert!(!old_db.exists());
        assert!(!old_wal.exists());
        assert!(!old_wal2.exists());
        assert!(!old_log.exists());
        assert!(!old_retired.exists());
    }

    #[test]
    fn review_p1_mvcc_reuse_preserves_loaded_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let dir_str = dir.path().to_str().unwrap();
        {
            let target = super::open("mvcc", dir_str, None, 1, false).unwrap();
            let mut stmts = vec![(
                "CREATE TABLE tool_calls (id INTEGER PRIMARY KEY, name TEXT NOT NULL)".into(),
                vec![],
            )];
            for i in 1..=50i64 {
                stmts.push((
                    "INSERT INTO tool_calls (id, name) VALUES (?, ?)".into(),
                    vec![super::P::I(i), super::P::T(format!("call-{i}"))],
                ));
            }
            target.setup(&stmts).unwrap();
            target.settle().unwrap();
            target.ready().unwrap();
        }
        let reused = super::open("mvcc", dir_str, None, 1, true).unwrap();
        reused.ready().unwrap();
        let mut sess = reused.session().unwrap();
        let rows = sess
            .answer(&["SELECT COUNT(*) FROM tool_calls".into()], &[vec![]])
            .unwrap();
        assert_eq!(rows, vec![vec![Some("50".to_owned())]]);
    }

    #[test]
    fn review_p1_pg_search_path_excludes_public() {
        let sql = super::pg_search_path_sql("amphora_gauntlet_s001");
        assert_eq!(sql, "SET search_path TO amphora_gauntlet_s001");
        assert!(
            !sql.contains("public"),
            "gauntlet search_path must not fall through to public: {sql}"
        );
    }

    #[test]
    fn review_p1_pg_null_accepts_all_postgres_column_types() {
        use postgres::types::{ToSql, Type};
        for ty in [
            Type::INT8,
            Type::INT4,
            Type::FLOAT8,
            Type::TEXT,
            Type::BYTEA,
            Type::JSONB,
            Type::BOOL,
            Type::NUMERIC,
        ] {
            assert!(
                <super::PgNull as ToSql>::accepts(&ty),
                "PgNull must accept Postgres type {ty:?}"
            );
        }
    }

    /// Review Round 9 Standards P2 regression: on a 20-item table with 4 clients,
    /// `delete_range_insert` and `delete_filtered_reinsert` must delete and reinsert the same 5
    /// owned IDs per transaction without padding with fresh IDs or growing the table.
    #[test]
    fn review_r9_delete_reinsert_does_not_grow_small_table() {
        let dir = tempfile::tempdir().unwrap();
        let shape = crate::suite::Shape::new(20);
        let clients = 4usize;
        assert_eq!(crate::suite::delete_reinsert_batch(shape.items, clients), 5);

        let target =
            super::open("sqlite", dir.path().to_str().unwrap(), None, clients, false).unwrap();
        let d = target.dialect();
        target
            .setup(
                &crate::suite::tables(d, false)
                    .into_iter()
                    .map(|s| (s, vec![]))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        crate::suite::load(d, &shape, 100, |txn| target.setup(&txn)).unwrap();
        target.ready().unwrap();

        let mut session = target.session().unwrap();
        for op in ["delete_range_insert", "delete_filtered_reinsert"] {
            let stmts: Vec<String> = crate::suite::statements_for(op, d, &shape, clients)
                .unwrap()
                .into_iter()
                .map(|s| crate::suite::placeholders(d, &s))
                .collect();
            for c in 0..clients {
                let mut rng = crate::Rng(0x1234_5678 + c as u64);
                for _ in 0..5 {
                    let p = crate::suite::params(op, &mut rng, &shape, (c, clients), d);
                    session.run(crate::suite::Kind::Write, &stmts, &p).unwrap();
                }
            }
            let count = session
                .answer(&["SELECT COUNT(*) FROM items".into()], &[vec![]])
                .unwrap();
            assert_eq!(
                count,
                vec![vec![Some("20".to_owned())]],
                "{op} grew the table instead of keeping 20 items"
            );
        }
    }

    /// Review Round 9 Standards P2 regression: `ddl_analyze_stats` must return the verified
    /// region count in `answer_ddl` and count the returned row in `run()` on Amphora (SQLite & PG)
    /// and stock SQLite.
    #[test]
    fn review_r9_ddl_analyze_stats_returns_and_verifies_region_count() {
        let shape = crate::suite::Shape::new(40);
        for target_name in ["amphora", "amphora-pg", "sqlite"] {
            let dir = tempfile::tempdir().unwrap();
            let target =
                super::open(target_name, dir.path().to_str().unwrap(), None, 1, false).unwrap();
            let d = target.dialect();
            target
                .setup(
                    &crate::suite::tables(d, false)
                        .into_iter()
                        .map(|s| (s, vec![]))
                        .collect::<Vec<_>>(),
                )
                .unwrap();
            crate::suite::load(d, &shape, 100, |txn| target.setup(&txn)).unwrap();
            target.ready().unwrap();

            let mut session = target.session().unwrap();
            let stmts: Vec<String> =
                crate::suite::statements_for("ddl_analyze_stats", d, &shape, 1)
                    .unwrap()
                    .into_iter()
                    .map(|s| crate::suite::placeholders(d, &s))
                    .collect();
            let p = vec![vec![], vec![]];
            let ans = session.answer_ddl(&stmts, &p).unwrap();
            assert_eq!(
                ans,
                vec![vec![Some(shape.regions.to_string())]],
                "{target_name}: ddl_analyze_stats answer_ddl must return exact regions count"
            );
            let rows = session.run(crate::suite::Kind::Ddl, &stmts, &p).unwrap();
            assert_eq!(
                rows, 1,
                "{target_name}: ddl_analyze_stats run() must not discard the returned count row"
            );
        }
    }

    /// Regression coverage test: every Round 1-9 regression workload added to `suite::OPS`
    /// executes without error across `amphora`, `amphora-pg`, and `sqlite`, and all shared
    /// read/DDL answers match stock SQLite exactly.
    #[test]
    fn review_r9_all_regression_suite_ops_match_sqlite() {
        let shape = crate::suite::Shape::new(250);
        let regression_ops = [
            "group_min_bare",
            "group_max_bare",
            "group_pk_functional_dep",
            "count_expr_overflow_null",
            "count_expr_nullable_mix",
            "group_by_wide_accum",
            "order_by_tied_limit_offset",
            "order_by_wide_large_limit",
            "order_by_full_wide_sort",
            "agg_ordered_set_stats",
            "hash_join_nullable_key",
            "distinct_multi_col_nulls",
            "catalog_public_relations",
            "write_empty_noop",
            "write_savepoint_rollback_commit",
            "ddl_view_pg_class_literal",
            "ddl_temp_tables_multi",
            "ddl_drop_recreate_table",
            "ddl_savepoint_rollback",
        ];

        let mut answers: std::collections::HashMap<(&str, &str), Vec<Vec<Option<String>>>> =
            std::collections::HashMap::new();
        for target_name in ["sqlite", "amphora", "amphora-pg"] {
            let dir = tempfile::tempdir().unwrap();
            let target =
                super::open(target_name, dir.path().to_str().unwrap(), None, 1, false).unwrap();
            let d = target.dialect();
            target
                .setup(
                    &crate::suite::tables(d, false)
                        .into_iter()
                        .map(|s| (s, vec![]))
                        .collect::<Vec<_>>(),
                )
                .unwrap();
            crate::suite::load(d, &shape, 100, |txn| target.setup(&txn)).unwrap();
            for idx in crate::suite::indexes(d) {
                target.setup(&[(idx, vec![])]).unwrap();
            }
            target.ready().unwrap();
            let mut session = target.session().unwrap();

            for op_name in regression_ops {
                let op = crate::suite::OPS
                    .iter()
                    .find(|o| o.name == op_name)
                    .unwrap_or_else(|| panic!("missing op {op_name}"));
                let Some(stmts) = crate::suite::statements_for(op_name, d, &shape, 1) else {
                    continue;
                };
                let stmts: Vec<String> = stmts
                    .into_iter()
                    .map(|s| crate::suite::placeholders(d, &s))
                    .collect();
                let mut rng = crate::Rng(0xCAFE_BABE_1234_5678);
                let p = crate::suite::params(op_name, &mut rng, &shape, (0, 1), d);
                match op.kind {
                    crate::suite::Kind::Read => {
                        let ans = session.answer(&stmts, &p).unwrap_or_else(|e| {
                            panic!("{target_name} read {op_name} failed: {e:#}")
                        });
                        assert!(
                            !ans.is_empty(),
                            "{target_name} read {op_name} must return at least 1 row"
                        );
                        answers.insert((target_name, op_name), ans);
                    }
                    crate::suite::Kind::Ddl => {
                        let ans = session.answer_ddl(&stmts, &p).unwrap_or_else(|e| {
                            panic!("{target_name} ddl {op_name} failed: {e:#}")
                        });
                        assert!(
                            !ans.is_empty(),
                            "{target_name} ddl {op_name} must return verification rows"
                        );
                        // Run twice to ensure DDL cleanup and statement invalidation hold across iterations.
                        let _ = session
                            .run(crate::suite::Kind::Ddl, &stmts, &p)
                            .unwrap_or_else(|e| {
                                panic!("{target_name} ddl second run {op_name} failed: {e:#}")
                            });
                        answers.insert((target_name, op_name), ans);
                    }
                    crate::suite::Kind::Write => {
                        session
                            .run(crate::suite::Kind::Write, &stmts, &p)
                            .unwrap_or_else(|e| {
                                panic!("{target_name} write {op_name} failed: {e:#}")
                            });
                    }
                }
            }
        }

        // Verify exact answer parity against stock SQLite for every shared op.
        for op_name in regression_ops {
            if matches!(
                op_name,
                "agg_ordered_set_stats" | "catalog_public_relations"
            ) {
                continue;
            }
            if let Some(ref_ans) = answers.get(&("sqlite", op_name)) {
                if let Some(amp_ans) = answers.get(&("amphora", op_name)) {
                    assert_eq!(
                        amp_ans, ref_ans,
                        "amphora diverged from sqlite on regression op {op_name}"
                    );
                }
                if let Some(pg_ans) = answers.get(&("amphora-pg", op_name)) {
                    assert_eq!(
                        pg_ans, ref_ans,
                        "amphora-pg diverged from sqlite on regression op {op_name}"
                    );
                }
            }
        }
        assert_eq!(
            answers.get(&("amphora", "count_expr_overflow_null")),
            Some(&vec![vec![Some("0".to_owned())]]),
        );
        assert_eq!(
            answers.get(&("amphora-pg", "count_expr_overflow_null")),
            Some(&vec![vec![Some("0".to_owned())]]),
        );
        assert_eq!(
            answers.get(&("amphora-pg", "catalog_public_relations")),
            Some(&vec![
                vec![Some("items".to_owned()), Some("r".to_owned())],
                vec![Some("owners".to_owned()), Some("r".to_owned())],
                vec![Some("regions".to_owned()), Some("r".to_owned())],
            ]),
        );
    }

    #[test]
    fn review_r9_pg_int_accepts_int2_int4_int8() {
        use super::PgInt;
        use postgres::types::{ToSql, Type};
        for ty in [Type::INT2, Type::INT4, Type::INT8] {
            assert!(
                <PgInt as ToSql>::accepts(&ty),
                "PgInt must accept Postgres integer type {ty:?}"
            );
            let mut buf = postgres::types::private::BytesMut::new();
            PgInt(42).to_sql(&ty, &mut buf).unwrap();
            assert!(!buf.is_empty());
        }
        assert!(!<PgInt as ToSql>::accepts(&Type::TEXT));
    }
}
