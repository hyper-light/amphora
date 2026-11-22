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
//! The storage is the process's: `AMPHORA_STORAGE=versioned` puts the database on the versioned
//! storage (`storage::versioned::switch`), so each harness runs on either storage unchanged.

use amphora_engine::{ConnectOpts, Connection, Database, DatabaseOpts, IO, LimboError, Numeric, OpenOptions, PlatformIO, SqliteDialect, Statement, StepResult, Value};
use std::collections::HashSet;
use std::io::Write;
use std::num::NonZero;
use std::sync::{Arc, Mutex};

pub fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let dir = flag("--dir").ok_or_else(|| anyhow::anyhow!("--dir <dir>: where the database is"))?;
    let path = std::path::Path::new(&dir).join("powercut.db");
    let name = path.to_str().ok_or_else(|| anyhow::anyhow!("a path that isn't UTF-8: {}", path.display()))?;
    let mode = args.iter().skip_while(|a| *a != "--powercut").nth(1).map(String::as_str);
    match mode {
        Some("write") => {
            // The step's 32 writers (G8 step 5's bar) unless set.
            let writers = flag("--writers").map(|w| w.parse::<u64>()).transpose()?.unwrap_or(32);
            if writers == 0 {
                anyhow::bail!("--writers: at least one");
            }
            write(name, writers)
        }
        Some("check") => {
            let acks = flag("--acks").ok_or_else(|| anyhow::anyhow!("--acks <file>: the acknowledged ids"))?;
            check(name, &acks)
        }
        other => anyhow::bail!("usage: amphora-bench --powercut write|check --dir <dir> [--writers <n>] [--acks <file>] (got {other:?})"),
    }
}

fn e(err: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("{err}")
}

fn open(name: &str) -> anyhow::Result<(Arc<dyn IO>, Arc<Database>)> {
    let io: Arc<dyn IO> = Arc::new(PlatformIO::new().map_err(e)?);
    let db = Database::open(io.clone(), name, OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new())).map_err(e)?;
    Ok((io, db))
}

/// A connection whose commits return once durable.
fn connect(io: &Arc<dyn IO>, db: &Arc<Database>) -> anyhow::Result<Arc<Connection>> {
    let conn = db.connect(ConnectOpts::new()).map_err(e)?;
    exec(io, &conn, "PRAGMA synchronous = FULL").map_err(e)?;
    Ok(conn)
}

/// Runs `stmt` to its end, giving `row` each row's values.
fn run(io: &Arc<dyn IO>, stmt: &mut Statement, mut row: impl FnMut(&[Value])) -> Result<(), LimboError> {
    let step_io = || io.step().map_err(|err| LimboError::InternalError(err.to_string()));
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
    matches!(err, LimboError::Busy | LimboError::BusySnapshot | LimboError::WriteWriteConflict | LimboError::SerializationFailure)
}

fn write(name: &str, writers: u64) -> anyhow::Result<()> {
    let (io, db) = open(name)?;
    let conn = connect(&io, &db)?;
    exec(&io, &conn, "PRAGMA journal_mode = WAL").map_err(e)?;
    exec(&io, &conn, "CREATE TABLE IF NOT EXISTS acked (id INTEGER PRIMARY KEY, writer INTEGER NOT NULL)").map_err(e)?;
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let mut threads = Vec::new();
    for w in 0..writers {
        let (io, db, out) = (io.clone(), db.clone(), out.clone());
        threads.push(std::thread::Builder::new().name(format!("powercut-{w}")).spawn(move || writer(&io, &db, &out, w, writers))?);
    }
    for t in threads {
        t.join().map_err(|_| anyhow::anyhow!("a writer's thread panicked"))??;
    }
    Ok(())
}

/// Writer `w` of `n`: its ids committed one a transaction, each printed once acknowledged.
fn writer(io: &Arc<dyn IO>, db: &Arc<Database>, out: &Mutex<std::io::Stdout>, w: u64, n: u64) -> anyhow::Result<()> {
    let conn = connect(io, db)?;
    let mut insert = conn.prepare("INSERT INTO acked (id, writer) VALUES (?1, ?2)").map_err(e)?;
    let (first, second) = (NonZero::new(1).ok_or_else(|| e("parameter 0"))?, NonZero::new(2).ok_or_else(|| e("parameter 0"))?);
    let writer = Value::from_i64(i64::try_from(w)?);
    let mut id = w;
    loop {
        insert.reset().map_err(e)?;
        insert.bind_at(first, Value::from_i64(i64::try_from(id)?)).map_err(e)?;
        insert.bind_at(second, writer.clone()).map_err(e)?;
        match run(io, &mut insert, |_| {}) {
            Ok(()) => {
                let mut o = out.lock().map_err(|_| e("stdout's lock poisoned"))?;
                // Its stdout closed, the harness has gone: the writer ends.
                if writeln!(o, "{id}").and_then(|()| o.flush()).is_err() {
                    return Ok(());
                }
            }
            Err(err) if refused(&err) => {
                if !conn.get_auto_commit() {
                    exec(io, &conn, "ROLLBACK").map_err(e)?;
                }
                continue;
            }
            Err(err) => anyhow::bail!("writer {w}, id {id}: {err}"),
        }
        id = id.checked_add(n).ok_or_else(|| e("ids past 2^64"))?;
    }
}

fn check(name: &str, acks: &str) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(acks).map_err(|err| anyhow::anyhow!("{acks}: {err}"))?;
    let mut lines: Vec<&str> = text.lines().collect();
    // A last line without its newline was torn as it was written: not acknowledged.
    if !text.is_empty() && !text.ends_with('\n') {
        lines.pop();
    }
    let acked = lines.iter().map(|l| l.trim().parse::<i64>().map_err(|err| anyhow::anyhow!("{acks}: {l:?}: {err}"))).collect::<anyhow::Result<Vec<i64>>>()?;
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
    println!("{}", serde_json::json!({ "acked": acked.len(), "present": present.len(), "lost": lost }));
    if lost > 0 {
        anyhow::bail!("{lost} acknowledged commits lost");
    }
    Ok(())
}
