//! T-T1.4.1 (CP-T1.4, AC-T1.4.4; G8 step 4): the database's side of the power-cut harness.
//! `bench/powercut/linux.sh` cuts a dm-flakey device's writes and `bench/powercut/macos.sh` powers a
//! VM off; both run these two modes and count what they report.
//!
//! - `amphora-bench --powercut write --dir <dir> [--writers <n>]`: `n` connections (the step's 32
//!   unless set), each a thread, commit single-row transactions to `<dir>/powercut.db` with
//!   `PRAGMA synchronous = FULL`, ids from each writer's own slice (writer `w` writes `w`, `w + n`,
//!   `w + 2n`, ...), and print each id on stdout, flushed, once its commit has returned: an
//!   acknowledged commit. A refused commit (another writer's conflict, a busy database) is rolled
//!   back and its id written again. It writes until the power goes, it is killed, or its stdout is
//!   closed (the harness gone).
//! - `amphora-bench --powercut check --dir <dir> --acks <file>`: reopens the database (recovery
//!   runs as it opens), reads every id, and counts the ids `<file>` acknowledged (one a line; a last
//!   line without its newline was torn, not acknowledged) that are missing: lost. Prints one JSON
//!   line, `{"acked", "present", "lost"}`, and fails when any is lost.
//!
//! - `amphora-bench --powercut span --dir <dir>`: prints how many acknowledgements a harness draws
//!   its cut point from (1 to that, uniformly): two of the WAL's checkpoint cycles, from the engine's
//!   own `wal_autocheckpoint` read where it runs ([`span`]).
//!
//! - `amphora-bench --powercut bench --engine amphora|sqlite|postgres --commits <n> [--writers <n>]
//!   (--dir <dir> | --dsn <libpq string>)`: the goal's benchmark ("Many clients write through SQL
//!   at once, safely": 20,000 durable commits a second from 32 writers, against SQLite and
//!   PostgreSQL), `n` commits in all from the writers above, each writer running the same loop
//!   ([`writes`]) on every engine: Amphora (the storage the process's, below), SQLite compiled in
//!   (WAL, `synchronous = FULL`) and PostgreSQL (`synchronous_commit = on`). One JSON line: commits
//!   a second and each commit's time from its first try to its acknowledgement at p50, p99 and
//!   p99.9, with the client and the engine's settings; bench/commits.sh runs every system on one
//!   machine and disk and adds the platform.
//!
//! The storage is the process's: `AMPHORA_STORAGE=versioned` puts the database on the versioned
//! storage (`storage::versioned::switch`), so each harness runs on either storage unchanged.

use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, IO, LimboError, Numeric, OpenOptions,
    PlatformIO, SqliteDialect, Statement, StepResult, Value,
};
use std::collections::HashSet;
use std::io::Write;
use std::num::NonZero;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let name = || -> anyhow::Result<String> {
        let dir =
            flag("--dir").ok_or_else(|| anyhow::anyhow!("--dir <dir>: where the database is"))?;
        let path = std::path::Path::new(&dir).join("powercut.db");
        path.to_str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("a path that isn't UTF-8: {}", path.display()))
    };
    // The step's 32 writers (G8 step 5's bar) unless set.
    let writers = || -> anyhow::Result<u64> {
        let writers = flag("--writers")
            .map(|w| w.parse::<u64>())
            .transpose()?
            .unwrap_or(32);
        if writers == 0 {
            anyhow::bail!("--writers: at least one");
        }
        Ok(writers)
    };
    let mode = args
        .iter()
        .skip_while(|a| *a != "--powercut")
        .nth(1)
        .map(String::as_str);
    match mode {
        Some("write") => write(&name()?, writers()?),
        Some("bench") => {
            let commits = flag("--commits")
                .ok_or_else(|| anyhow::anyhow!("--commits <n>: how many commits in all"))?
                .parse::<u64>()?;
            // A fresh database: the bench measures commits into an empty table.
            let fresh = || -> anyhow::Result<String> {
                let name = name()?;
                if std::path::Path::new(&name).exists() {
                    anyhow::bail!(
                        "{name}: a database is already there; the bench starts from none"
                    );
                }
                Ok(name)
            };
            let target = match flag("--engine").as_deref() {
                Some("amphora") => {
                    let (io, db) = open(&fresh()?)?;
                    Target::Amphora { io, db }
                }
                Some("sqlite") => Target::Sqlite { path: fresh()? },
                Some("postgres") => Target::Postgres {
                    dsn: flag("--dsn")
                        .ok_or_else(|| anyhow::anyhow!("--dsn <libpq string>: the server"))?,
                },
                other => anyhow::bail!("--engine amphora|sqlite|postgres (got {other:?})"),
            };
            bench(
                &target,
                writers()?,
                commits,
                args.iter().any(|a| a == "--hold"),
            )
        }
        Some("check") => {
            let acks = flag("--acks")
                .ok_or_else(|| anyhow::anyhow!("--acks <file>: the acknowledged ids"))?;
            check(&name()?, &acks)
        }
        Some("span") => span(&name()?),
        other => anyhow::bail!(
            "usage: amphora-bench --powercut write|check|span|bench --dir <dir> [--writers <n>] [--acks <file>] [--engine amphora|sqlite|postgres --commits <n> --dsn <libpq string> --hold] (got {other:?})"
        ),
    }
}

fn e(err: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("{err}")
}

fn open(name: &str) -> anyhow::Result<(Arc<dyn IO>, Arc<Database>)> {
    let io: Arc<dyn IO> = Arc::new(PlatformIO::new().map_err(e)?);
    let db = Database::open(
        io.clone(),
        name,
        OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new()),
    )
    .map_err(e)?;
    Ok((io, db))
}

/// A connection whose commits return once durable.
fn connect(io: &Arc<dyn IO>, db: &Arc<Database>) -> anyhow::Result<Arc<Connection>> {
    let conn = db.connect(ConnectOpts::new()).map_err(e)?;
    exec(io, &conn, "PRAGMA synchronous = FULL").map_err(e)?;
    Ok(conn)
}

/// Runs `stmt` to its end, giving `row` each row's values.
fn run(
    io: &Arc<dyn IO>,
    stmt: &mut Statement,
    mut row: impl FnMut(&[Value]),
) -> Result<(), LimboError> {
    let step_io = || {
        io.step()
            .map_err(|err| LimboError::InternalError(err.to_string()))
    };
    loop {
        match stmt.step()? {
            StepResult::Row => {
                if let Some(r) = stmt.row() {
                    let values: Vec<Value> = r.get_values().cloned().collect();
                    row(&values);
                }
            }
            StepResult::Done => return Ok(()),
            StepResult::IO | StepResult::Yield => step_io()?,
            StepResult::Sleep { duration } => {
                std::thread::sleep(duration);
                step_io()?;
            }
            StepResult::Busy => return Err(LimboError::Busy),
            StepResult::Interrupt => return Err(LimboError::InternalError("interrupted".into())),
        }
    }
}

fn exec(io: &Arc<dyn IO>, conn: &Arc<Connection>, sql: &str) -> Result<(), LimboError> {
    let mut stmt = conn.prepare(sql)?;
    run(io, &mut stmt, |_| {})
}

/// What refuses a commit under many writers, for the writer to roll back and try again.
fn refused(err: &LimboError) -> bool {
    matches!(
        err,
        LimboError::Busy
            | LimboError::BusySnapshot
            | LimboError::WriteWriteConflict
            | LimboError::SerializationFailure
    )
}

fn write(name: &str, writers: u64) -> anyhow::Result<()> {
    let (io, db) = open(name)?;
    schema(&io, &db)?;
    let target = Target::Amphora { io, db };
    let out = Mutex::new(std::io::stdout());
    std::thread::scope(|s| -> anyhow::Result<()> {
        let threads = (0..writers)
            .map(|w| {
                let (target, out) = (&target, &out);
                std::thread::Builder::new()
                    .name(format!("powercut-{w}"))
                    .spawn_scoped(s, move || -> anyhow::Result<()> {
                        let mut c = target.connect()?;
                        writes(c.as_mut(), w, writers, |id, _| {
                            let mut o = out.lock().map_err(|_| e("stdout's lock poisoned"))?;
                            // Its stdout closed, the harness has gone: the writer ends.
                            Ok(writeln!(o, "{id}").and_then(|()| o.flush()).is_ok())
                        })
                        .map(|_| ())
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        for t in threads {
            t.join()
                .map_err(|_| anyhow::anyhow!("a writer's thread panicked"))??;
        }
        Ok(())
    })
}

/// The table every writer commits to, on the engine's first connection, the journal in WAL.
fn schema(io: &Arc<dyn IO>, db: &Arc<Database>) -> anyhow::Result<()> {
    let conn = connect(io, db)?;
    exec(io, &conn, "PRAGMA journal_mode = WAL").map_err(e)?;
    exec(
        io,
        &conn,
        "CREATE TABLE IF NOT EXISTS acked (id INTEGER PRIMARY KEY, writer INTEGER NOT NULL)",
    )
    .map_err(e)
}

/// One engine's durable single-row commit, the step every writer of the loop ([`writes`]) takes:
/// `Ok(true)` once the commit has returned durable (acknowledged), `Ok(false)` when it was refused
/// (another writer's conflict, a busy database) and rolled back, to be tried again.
trait Commit {
    fn commit(&mut self, id: i64, writer: i64) -> anyhow::Result<bool>;
}

/// Writer `w` of `n`'s loop, the same for every engine: ids from its own slice (`w`, `w + n`, ...),
/// each committed in a transaction of its own and tried again until acknowledged; `ack` gets the
/// id and the time from its first try to its acknowledgement, and says whether to go on. Returns how
/// many tries were refused.
fn writes(
    c: &mut dyn Commit,
    w: u64,
    n: u64,
    mut ack: impl FnMut(u64, std::time::Duration) -> anyhow::Result<bool>,
) -> anyhow::Result<u64> {
    let writer = i64::try_from(w)?;
    let (mut id, mut refused) = (w, 0u64);
    loop {
        let began = Instant::now();
        while !c.commit(i64::try_from(id)?, writer)? {
            refused = refused.saturating_add(1);
        }
        if !ack(id, began.elapsed())? {
            return Ok(refused);
        }
        id = id.checked_add(n).ok_or_else(|| e("ids past 2^64"))?;
    }
}

/// Amphora's commit: the prepared INSERT on a connection whose commits return once durable.
struct AmphoraCommit {
    io: Arc<dyn IO>,
    conn: Arc<Connection>,
    insert: Statement,
    first: NonZero<usize>,
    second: NonZero<usize>,
}

impl Commit for AmphoraCommit {
    fn commit(&mut self, id: i64, writer: i64) -> anyhow::Result<bool> {
        self.insert.reset().map_err(e)?;
        self.insert
            .bind_at(self.first, Value::from_i64(id))
            .map_err(e)?;
        self.insert
            .bind_at(self.second, Value::from_i64(writer))
            .map_err(e)?;
        match run(&self.io, &mut self.insert, |_| {}) {
            Ok(()) => Ok(true),
            Err(err) if refused(&err) => {
                if !self.conn.get_auto_commit() {
                    exec(&self.io, &self.conn, "ROLLBACK").map_err(e)?;
                }
                Ok(false)
            }
            Err(err) => anyhow::bail!("writer {writer}, id {id}: {err}"),
        }
    }
}

/// SQLite's commit (rusqlite, SQLite compiled in): WAL, `synchronous = FULL`, no busy handler, so a
/// busy database is refused at once and tried again by the loop, as Amphora's is (its connections
/// have none by default, `BusyHandler::None`). rusqlite gives every connection it opens a 5 s busy
/// timeout, which [`Target::connect`] takes away: with it, SQLite would wait in its own handler
/// where Amphora is refused and tries again, two loops, not one.
struct SqliteCommit(rusqlite::Connection);

impl Commit for SqliteCommit {
    fn commit(&mut self, id: i64, writer: i64) -> anyhow::Result<bool> {
        let done = self
            .0
            .prepare_cached("INSERT INTO acked (id, writer) VALUES (?1, ?2)")
            .and_then(|mut s| s.execute((id, writer)));
        match done {
            Ok(_) => Ok(true),
            Err(err)
                if matches!(
                    err.sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
                ) =>
            {
                Ok(false)
            }
            Err(err) => anyhow::bail!("writer {writer}, id {id}: {err}"),
        }
    }
}

/// PostgreSQL's commit: the prepared INSERT, each its own transaction, `synchronous_commit = on`.
struct PostgresCommit {
    client: postgres::Client,
    insert: postgres::Statement,
}

impl Commit for PostgresCommit {
    fn commit(&mut self, id: i64, writer: i64) -> anyhow::Result<bool> {
        match self.client.execute(&self.insert, &[&id, &writer]) {
            Ok(_) => Ok(true),
            Err(err)
                if err.code().is_some_and(|c| {
                    *c == postgres::error::SqlState::T_R_SERIALIZATION_FAILURE
                        || *c == postgres::error::SqlState::T_R_DEADLOCK_DETECTED
                }) =>
            {
                Ok(false)
            }
            Err(err) => anyhow::bail!("writer {writer}, id {id}: {err}"),
        }
    }
}

/// The engine a run commits to; each writer's thread makes its own connection from it.
enum Target {
    Amphora { io: Arc<dyn IO>, db: Arc<Database> },
    Sqlite { path: String },
    Postgres { dsn: String },
}

impl Target {
    fn connect(&self) -> anyhow::Result<Box<dyn Commit>> {
        Ok(match self {
            Target::Amphora { io, db } => {
                let conn = connect(io, db)?;
                let insert = conn
                    .prepare("INSERT INTO acked (id, writer) VALUES (?1, ?2)")
                    .map_err(e)?;
                Box::new(AmphoraCommit {
                    io: io.clone(),
                    conn,
                    insert,
                    first: NonZero::new(1).ok_or_else(|| e("parameter 0"))?,
                    second: NonZero::new(2).ok_or_else(|| e("parameter 0"))?,
                })
            }
            Target::Sqlite { path } => {
                let c = rusqlite::Connection::open(path)?;
                c.busy_handler(None)?;
                c.pragma_update(None, "journal_mode", "WAL")?;
                c.pragma_update(None, "synchronous", "FULL")?;
                Box::new(SqliteCommit(c))
            }
            Target::Postgres { dsn } => {
                let mut client = postgres::Client::connect(dsn, postgres::NoTls)?;
                client.batch_execute("SET synchronous_commit = on")?;
                let insert = client.prepare("INSERT INTO acked (id, writer) VALUES ($1, $2)")?;
                Box::new(PostgresCommit { client, insert })
            }
        })
    }

    /// The table, made once before the writers start, and the engine's version and durability as
    /// it reports them, for the record.
    fn prepare(&self) -> anyhow::Result<serde_json::Value> {
        Ok(match self {
            Target::Amphora { io, db } => {
                schema(io, db)?;
                let storage = if amphora_engine::storage::versioned::switch::asked() {
                    "versioned"
                } else {
                    "pager"
                };
                serde_json::json!({ "engine": "amphora", "storage": storage, "synchronous": "FULL", "journal": "WAL", "busy_handler": "none" })
            }
            Target::Sqlite { path } => {
                let c = rusqlite::Connection::open(path)?;
                let mode: String =
                    c.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))?;
                c.pragma_update(None, "synchronous", "FULL")?;
                c.execute_batch("CREATE TABLE IF NOT EXISTS acked (id INTEGER PRIMARY KEY, writer INTEGER NOT NULL)")?;
                serde_json::json!({ "engine": "sqlite", "version": rusqlite::version(), "synchronous": "FULL", "journal": mode, "busy_handler": "none" })
            }
            Target::Postgres { dsn } => {
                let mut c = postgres::Client::connect(dsn, postgres::NoTls)?;
                c.batch_execute("CREATE TABLE IF NOT EXISTS acked (id BIGINT PRIMARY KEY, writer BIGINT NOT NULL)")?;
                let mut show = |what: &str| -> anyhow::Result<String> {
                    Ok(c.query_one(&format!("SHOW {what}"), &[])?
                        .try_get::<_, String>(0)?)
                };
                serde_json::json!({
                    "engine": "postgres",
                    "version": show("server_version")?,
                    "fsync": show("fsync")?,
                    "shared_buffers": show("shared_buffers")?,
                    "work_mem": show("work_mem")?,
                    "max_connections": show("max_connections")?,
                    "synchronous_commit": "on",
                    "wal_sync_method": show("wal_sync_method")?,
                    "full_page_writes": show("full_page_writes")?,
                })
            }
        })
    }
}

/// `--powercut bench`: `commits` durable single-row commits from `writers` writers, each its own
/// thread and connection running [`writes`], the same loop for every engine; prints one JSON line:
/// commits a second and each commit's time from first try to acknowledgement at p50, p99 and p99.9.
/// The clock starts once every writer has connected and stops when the last commit is in. With
/// `hold`, every connection stays open after the last commit while the harness reads what the run
/// holds (bench/commits_fit.py: its open files, a server's memory): `held` on stderr, then it waits
/// for stdin to close before the connections do (T-T1.4.1).
fn bench(target: &Target, writers: u64, commits: u64, hold: bool) -> anyhow::Result<()> {
    let engine = target.prepare()?;
    let all = usize::try_from(writers)?.saturating_add(1);
    let (start, done, release) = (
        std::sync::Barrier::new(all),
        std::sync::Barrier::new(all),
        std::sync::Barrier::new(all),
    );
    let (began, results) = std::thread::scope(|s| -> anyhow::Result<_> {
        let threads = (0..writers)
            .map(|w| {
                let (start, done, release) = (&start, &done, &release);
                // Writer w's share of the commits: an even split, the remainder to the first ones.
                let quota = commits / writers + u64::from(w < commits % writers);
                std::thread::Builder::new()
                    .name(format!("commits-{w}"))
                    .spawn_scoped(s, move || -> anyhow::Result<(Vec<u64>, u64)> {
                        let mut c = target.connect();
                        start.wait();
                        // Every writer reaches both barriers, its connection still open, whatever
                        // its commits came to: none is left waiting on one that ended early.
                        let got = (|| -> anyhow::Result<(Vec<u64>, u64)> {
                            let c = c.as_mut().map_err(e)?;
                            let mut times = Vec::with_capacity(usize::try_from(quota)?);
                            if quota == 0 {
                                return Ok((times, 0));
                            }
                            let refused = writes(c.as_mut(), w, writers, |_, took| {
                                times.push(u64::try_from(took.as_nanos()).unwrap_or(u64::MAX));
                                Ok(u64::try_from(times.len())? < quota)
                            })?;
                            Ok((times, refused))
                        })();
                        done.wait();
                        release.wait();
                        drop(c);
                        got
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        start.wait();
        let began = Instant::now();
        done.wait();
        let took = began.elapsed();
        let held = if hold {
            // Every connection open, the last commit in: the harness reads what the run holds.
            eprintln!("held");
            let mut rest = Vec::new();
            std::io::Read::read_to_end(&mut std::io::stdin(), &mut rest).map(|_| ())
        } else {
            Ok(())
        };
        release.wait();
        held?;
        let mut results = Vec::with_capacity(threads.len());
        for t in threads {
            results.push(
                t.join()
                    .map_err(|_| anyhow::anyhow!("a writer's thread panicked"))??,
            );
        }
        Ok((took, results))
    })?;
    let refused: u64 = results.iter().map(|(_, r)| r).sum();
    let mut times: Vec<u64> = results.into_iter().flat_map(|(t, _)| t).collect();
    times.sort_unstable();
    // Nearest rank: the smallest time at least `num / den` of the commits took no longer than.
    let rank = |num: u64, den: u64| -> Option<u64> {
        let n = u64::try_from(times.len()).ok()?;
        let r = n.checked_mul(num)?.div_ceil(den).max(1);
        times.get(usize::try_from(r - 1).ok()?).copied()
    };
    let us = |ns: Option<u64>| ns.map(|n| n as f64 / 1e3);
    let secs = began.as_secs_f64();
    println!(
        "{}",
        serde_json::json!({
            "engine": engine,
            "client": "amphora-bench --powercut bench: one process, a thread and a connection a writer, the same commit loop and clock for every engine",
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "writers": writers,
            "commits": times.len(),
            "refused": refused,
            "seconds": secs,
            "commits_per_s": times.len() as f64 / secs,
            "p50_us": us(rank(1, 2)),
            "p99_us": us(rank(99, 100)),
            "p999_us": us(rank(999, 1000)),
        })
    );
    Ok(())
}

/// The acknowledgements a cut point is drawn from, so cuts land in every state the durable path
/// goes through: two of the WAL's checkpoint cycles. A commit appends at least one frame, so the WAL
/// is checkpointed within `wal_autocheckpoint` commits (the engine's own setting, read from the
/// database, not chosen here); cuts drawn over two cycles land before the first checkpoint, across
/// it, and in the WAL written again after it. The versioned storage checkpoints only when asked,
/// which no SQL commit does, so its redo log only grows and the pager's cycle is the only one.
fn span(name: &str) -> anyhow::Result<()> {
    let (io, db) = open(name)?;
    let conn = connect(&io, &db)?;
    exec(&io, &conn, "PRAGMA journal_mode = WAL").map_err(e)?;
    let mut frames = None;
    let mut stmt = conn.prepare("PRAGMA wal_autocheckpoint").map_err(e)?;
    run(&io, &mut stmt, |r| {
        if let Some(Value::Numeric(Numeric::Integer(i))) = r.first() {
            frames = Some(*i);
        }
    })
    .map_err(e)?;
    let frames = frames.ok_or_else(|| e("PRAGMA wal_autocheckpoint gave no number"))?;
    if frames <= 0 {
        anyhow::bail!(
            "the WAL is never checkpointed (wal_autocheckpoint = {frames}): no cycle to span"
        );
    }
    println!(
        "{}",
        frames.checked_mul(2).ok_or_else(|| e("a span past 2^63"))?
    );
    Ok(())
}

fn check(name: &str, acks: &str) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(acks).map_err(|err| anyhow::anyhow!("{acks}: {err}"))?;
    let mut lines: Vec<&str> = text.lines().collect();
    // A last line without its newline was torn as it was written: not acknowledged.
    if !text.is_empty() && !text.ends_with('\n') {
        lines.pop();
    }
    let acked = lines
        .iter()
        .map(|l| {
            l.trim()
                .parse::<i64>()
                .map_err(|err| anyhow::anyhow!("{acks}: {l:?}: {err}"))
        })
        .collect::<anyhow::Result<Vec<i64>>>()?;
    let (io, db) = open(name)?;
    let conn = connect(&io, &db)?;
    let mut present = HashSet::new();
    let mut odd = 0u64;
    let mut stmt = conn.prepare("SELECT id FROM acked").map_err(e)?;
    run(&io, &mut stmt, |r| match r.first() {
        Some(Value::Numeric(Numeric::Integer(i))) => {
            present.insert(*i);
        }
        _ => odd += 1,
    })
    .map_err(e)?;
    if odd > 0 {
        anyhow::bail!("{odd} rows whose id isn't an integer");
    }
    let lost = acked.iter().filter(|id| !present.contains(id)).count();
    println!(
        "{}",
        serde_json::json!({ "acked": acked.len(), "present": present.len(), "lost": lost })
    );
    if lost > 0 {
        anyhow::bail!("{lost} acknowledged commits lost");
    }
    Ok(())
}
