//! Stores on Turso (`vendor/turso`): a database opened the way Amphora's pipeline wants it
//! ([`Db::open`]), its readers ([`Db::reader`]), and what the store's flusher, checkpointer and
//! writer do with it ([`Db::flusher`], [`Db::checkpoint`], [`Db::restart_log`]). An engine on Turso
//! keeps a [`Db`] for its writer and gives readers their own.

use std::collections::HashMap;
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use turso_core::{
    CheckpointMode, ConnectOpts, Connection, Database, DatabaseOpts, IO, LimboError, OpenOptions,
    PlatformIO, Row, SqliteDialect, Statement, StepResult, Value,
};

use crate::store::{Flush, Opener};

/// How long opening waits for another process's hold on the file to go.
const LOCK_WAIT: Duration = Duration::from_secs(15);
/// A statement that finds the database busy (another connection writing, a checkpoint) tries again
/// after this, this many times.
const BUSY_WAIT: Duration = Duration::from_millis(1);
const BUSY_TRIES: u32 = 10_000;
/// Pages each connection keeps in memory, by what it does (Turso keeps 2,000, 8 MiB, on every
/// connection; a store has a writer, readers and a checkpointer, and a reader's miss is a read of the
/// file through the system's page cache): the writer keeps the inner pages of its indexes, a reader
/// what one read touches, the checkpointer next to nothing.
pub const WRITER_PAGES: i64 = 1024;
pub const READER_PAGES: i64 = 128;

/// The log's frames past which the checkpointer asks the writer to start it again: 64 MiB of
/// 4 KiB pages (measured on fedora, 2026-10-20: under ~90 MB of log at 5,000 writes a second).
pub const LOG_FRAMES: u64 = 16_384;

fn fail(e: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

/// Parks this thread for `d` (a park ends early for no reason sometimes: the rest is parked again).
fn pause(d: Duration) {
    let until = Instant::now().checked_add(d).unwrap_or_else(Instant::now);
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        std::thread::park_timeout(left);
    }
}

/// A connection on a Turso database, with the statements it prepared kept.
pub struct Db {
    io: Arc<dyn IO>,
    db: Arc<Database>,
    conn: Arc<Connection>,
    path: PathBuf,
    statements: HashMap<&'static str, Statement>,
}

impl Db {
    /// The database at `path`, made as needed, for a store's writer: a commit writes the log and
    /// doesn't flush it (the store's flusher does, [`Db::flusher`]), and no checkpoint runs in a
    /// commit (the store's checkpointer copies the log, [`Db::checkpoint`]). A checkpoint still
    /// flushes the log before it copies it and the file after.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let name = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("the path isn't UTF-8: {}", path.display()))?;
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().map_err(fail)?);
        let until = Instant::now()
            .checked_add(LOCK_WAIT)
            .unwrap_or_else(Instant::now);
        let db = loop {
            match Database::open(
                io.clone(),
                name,
                OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new()),
            ) {
                Err(LimboError::LockingError(_)) if Instant::now() < until => {
                    pause(Duration::from_millis(100));
                }
                opened => break opened.map_err(fail)?,
            }
        };
        let conn = db.connect(ConnectOpts::new()).map_err(fail)?;
        let d = Self {
            io,
            db,
            conn,
            path: path.to_path_buf(),
            statements: HashMap::new(),
        };
        d.exec("PRAGMA synchronous = NORMAL")?;
        d.exec("PRAGMA wal_autocheckpoint = 0")?;
        d.exec(&format!("PRAGMA cache_size = {WRITER_PAGES}"))?;
        Ok(d)
    }

    /// Keeps `pages` of the database in this connection's memory.
    pub fn keep_pages(&self, pages: i64) -> anyhow::Result<()> {
        self.exec(&format!("PRAGMA cache_size = {pages}"))
    }

    /// Another connection on the same database, for reading or checkpointing: each read sees what
    /// was committed before it; its checkpoints flush.
    pub fn reader(&self) -> anyhow::Result<Self> {
        let r = Self {
            io: self.io.clone(),
            db: self.db.clone(),
            conn: self.db.connect(ConnectOpts::new()).map_err(fail)?,
            path: self.path.clone(),
            statements: HashMap::new(),
        };
        r.keep_pages(READER_PAGES)?;
        Ok(r)
    }

    /// Opens readers on this database from any thread.
    #[must_use]
    pub fn opener(&self) -> Opener<Self> {
        let (io, db, path) = (self.io.clone(), self.db.clone(), self.path.clone());
        Arc::new(move || {
            let r = Self {
                io: io.clone(),
                db: db.clone(),
                conn: db.connect(ConnectOpts::new()).map_err(fail)?,
                path: path.clone(),
                statements: HashMap::new(),
            };
            r.keep_pages(READER_PAGES)?;
            Ok(r)
        })
    }

    /// Flushes the log: what was committed before the flush began is durable once it ends.
    pub fn flusher(&self) -> Flush {
        let mut wal = self.path.clone().into_os_string();
        wal.push("-wal");
        let wal = PathBuf::from(wal);
        Box::new(move || {
            // Opened each time: a flush is a disk's round trip, an open a few microseconds, and the
            // log may have been made again. No log: everything is in the file, which the checkpoint
            // that emptied the log flushed.
            match std::fs::OpenOptions::new().write(true).open(&wal) {
                Ok(f) => f.sync_data(),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            }
        })
    }

    /// Copies what the log holds into the file without waiting for anyone: whether the log holds
    /// more than [`LOG_FRAMES`], for the writer to start it again ([`Db::restart_log`]).
    pub fn checkpoint(&mut self) -> anyhow::Result<bool> {
        self.checkpoint_past(LOG_FRAMES)
    }

    /// [`Db::checkpoint`], the log's bound `frames`.
    pub fn checkpoint_past(&mut self, frames: u64) -> anyhow::Result<bool> {
        match self.conn.checkpoint(CheckpointMode::Passive {
            upper_bound_inclusive: None,
        }) {
            Ok(copied) => Ok(copied.wal_max_frame > frames),
            Err(LimboError::Busy) => Ok(false),
            Err(e) => Err(fail(e)),
        }
    }

    /// With no reader reading (the store holds them back), copies what remains, flushes the file,
    /// and starts the log from its beginning, its file kept: whether it did.
    pub fn restart_log(&mut self) -> anyhow::Result<bool> {
        match self.conn.checkpoint(CheckpointMode::Restart) {
            Ok(_) => Ok(true),
            Err(LimboError::Busy) => Ok(false),
            Err(e) => Err(fail(e)),
        }
    }

    /// The file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Statements that answer nothing, one after another.
    pub fn exec(&self, sql: &str) -> anyhow::Result<()> {
        for one in sql.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            self.conn.execute(one).map_err(|e| {
                anyhow::anyhow!("{e} (in: {})", one.lines().next().unwrap_or_default())
            })?;
        }
        Ok(())
    }

    /// Runs `sql` with `params`, its statement prepared the first time and kept, handing each row
    /// to `each` until it says stop (false).
    pub fn query(
        &mut self,
        sql: &'static str,
        params: &[Value],
        each: impl FnMut(&Row) -> anyhow::Result<bool>,
    ) -> anyhow::Result<()> {
        let stmt = match self.statements.entry(sql) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(self.conn.prepare(sql).map_err(fail)?)
            }
        };
        step(self.io.as_ref(), stmt, params, each)
    }

    /// `f` in one transaction: committed if it returns Ok, rolled back if not.
    pub fn transaction<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.exec("BEGIN")?;
        match f(self) {
            Ok(v) => {
                self.exec("COMMIT")?;
                Ok(v)
            }
            Err(e) => {
                let _ = self.exec("ROLLBACK");
                Err(e)
            }
        }
    }
}

/// Runs `stmt` with `params` to its end, handing each row to `each` until it says stop (false).
pub fn step(
    io: &dyn IO,
    stmt: &mut Statement,
    params: &[Value],
    mut each: impl FnMut(&Row) -> anyhow::Result<bool>,
) -> anyhow::Result<()> {
    stmt.reset().map_err(fail)?;
    for (i, p) in params.iter().enumerate() {
        let at = NonZero::new(i.saturating_add(1)).ok_or_else(|| anyhow::anyhow!("parameter 0"))?;
        stmt.bind_at(at, p.clone()).map_err(fail)?;
    }
    let mut busy = 0u32;
    loop {
        match stmt.step().map_err(fail)? {
            StepResult::Row => {
                if let Some(row) = stmt.row()
                    && !each(row)?
                {
                    return stmt.reset().map_err(fail);
                }
            }
            StepResult::Done => return Ok(()),
            StepResult::IO | StepResult::Yield => io.step().map_err(fail)?,
            StepResult::Sleep { duration } => {
                pause(duration);
                io.step().map_err(fail)?;
            }
            StepResult::Busy => {
                busy = busy.saturating_add(1);
                if busy > BUSY_TRIES {
                    anyhow::bail!("the database stayed busy");
                }
                pause(BUSY_WAIT);
            }
            StepResult::Interrupt => anyhow::bail!("a statement was interrupted"),
        }
    }
}
