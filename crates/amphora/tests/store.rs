//! The store's pipeline (CP-T1.1, AC-T1.1.1: the conversation store drops no update and serves
//! reads without waiting on writes), on a small engine of keyed events over Turso.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
        clippy::disallowed_macros
    )
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use amphora::db::Db;
use amphora::store::{
    Checkpointed, Config, Engine, Flush, ForReaders, Opener, Refused, Store, Upkept,
};
use amphora_engine::Value;

struct Events {
    db: Db,
    /// The upkeep to answer next, for a test of what readers do around it.
    next: Arc<std::sync::Mutex<Vec<ForReaders>>>,
}

struct Row {
    key: String,
    seq: i64,
    text: String,
}

const INSERT: &str = "INSERT INTO events (key, seq, text) VALUES (?1, ?2, ?3)";
const COUNT: &str = "SELECT count(*) FROM events WHERE key = ?1";

impl Engine for Events {
    type Row = Row;
    type Reader = Db;

    fn key(row: &Row) -> &str {
        &row.key
    }

    fn row_bytes(row: &Row) -> usize {
        std::mem::size_of::<Row>()
            .saturating_add(row.key.capacity())
            .saturating_add(row.text.capacity())
    }

    fn apply(&mut self, rows: &[Row]) -> anyhow::Result<usize> {
        self.db.transaction(|db| insert(db, rows))?;
        Ok(rows.len())
    }

    fn apply_batches(&mut self, rows: &[Row], ends: &[usize]) -> anyhow::Result<(usize, usize)> {
        let mut start = 0;
        let batches = ends.iter().map(|&end| {
            let batch = &rows[start..end];
            start = end;
            batch
        });
        let mut written = 0;
        let committed = self.db.transaction_while_room(batches, |db, batch| {
            insert(db, batch)?;
            written += batch.len();
            Ok(())
        })?;
        Ok((committed, written))
    }

    fn flusher(&self) -> Option<Flush> {
        Some(self.db.flusher())
    }

    fn opener(&self) -> Option<Opener<Db>> {
        Some(self.db.opener())
    }

    fn checkpoint(reader: &mut Db) -> anyhow::Result<Checkpointed> {
        reader.checkpoint()
    }

    // As Amphora's own engine (`database::Local`): the store sees what its log holds uncopied.
    fn uncopied(reader: &mut Db) -> anyhow::Result<Option<Checkpointed>> {
        Ok(Some(reader.uncopied()))
    }

    fn restart_log(&mut self) -> anyhow::Result<bool> {
        self.db.restart_log()
    }

    fn upkeep(&mut self, _quiet: bool) -> anyhow::Result<Upkept> {
        let readers = self.next.lock().unwrap().pop().unwrap_or_default();
        Ok(Upkept {
            again_in: Some(Duration::from_millis(20)),
            readers,
        })
    }
}

fn insert(db: &mut Db, rows: &[Row]) -> anyhow::Result<()> {
    for r in rows {
        let params = [
            Value::from_text(r.key.clone()),
            Value::from_i64(r.seq),
            Value::from_text(r.text.clone()),
        ];
        db.query(INSERT, params, |_| Ok(true))?;
    }
    Ok(())
}

fn count_on(db: &mut Db, key: &str) -> anyhow::Result<i64> {
    let mut n = 0;
    db.query(COUNT, [Value::from_text(key.to_owned())], |r| {
        n = r.get::<i64>(0).map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(true)
    })?;
    Ok(n)
}

struct Lab {
    _dir: tempfile::TempDir,
    path: std::path::PathBuf,
    store: Store<Events>,
    next: Arc<std::sync::Mutex<Vec<ForReaders>>>,
}

fn lab(config: Config, flush: Option<Flush>) -> Lab {
    lab_keeping(config, |_| flush, None)
}

/// A lab whose writer keeps `pages` pages in memory (None: Amphora's default), flushed as `flush`
/// says given the database (None: the engine's own flusher).
fn lab_keeping(
    config: Config,
    flush: impl FnOnce(&Db) -> Option<Flush>,
    pages: Option<i64>,
) -> Lab {
    lab_set(config, flush, |db| {
        if let Some(pages) = pages {
            db.keep_pages(pages).unwrap();
        }
    })
}

/// A lab whose writer is as `set` makes it.
fn lab_set(
    config: Config,
    flush: impl FnOnce(&Db) -> Option<Flush>,
    set: impl FnOnce(&mut Db),
) -> Lab {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.db");
    let mut db = Db::open(&path).unwrap();
    set(&mut db);
    // A row of negative seq can't be written: a batch that holds one fails.
    db.exec("CREATE TABLE events (key TEXT, seq INTEGER CHECK (seq >= 0), text TEXT); CREATE INDEX by_key ON events (key, seq)")
        .unwrap();
    let flush = flush(&db);
    let next = Arc::new(std::sync::Mutex::new(Vec::new()));
    let engine = Events {
        db,
        next: next.clone(),
    };
    let store = Store::start_with(engine, config, flush).unwrap();
    Lab {
        _dir: dir,
        path,
        store,
        next,
    }
}

fn soon() -> Instant {
    Instant::now() + Duration::from_secs(10)
}

fn row(key: &str, seq: i64) -> Row {
    Row {
        key: key.to_owned(),
        seq,
        text: format!("event {seq} of {key}"),
    }
}

fn count(store: &Store<Events>, key: &str) -> i64 {
    let (k, w) = (key.to_owned(), key.to_owned());
    store
        .read(
            Some(key),
            soon(),
            move |r| count_on(r, &k),
            move |e| count_on(&mut e.db, &w),
        )
        .unwrap()
}

/// What is put is read back by its key at once: a read waits for its key's writes handed over
/// before it, and no longer.
fn rows_in(db: &Db) -> i64 {
    let mut n = 0;
    db.query("SELECT count(*) FROM t", [], |r| {
        n = r.get::<i64>(0).map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(true)
    })
    .unwrap();
    n
}

/// Decision 45 (lane B18): one slow reader never holds up a write. Do: commit 10 rows, open a
/// read transaction on a reader and leave it open, start the log again on the writer, and commit
/// 50 rows past it. Expect: the restart happens with the read still open (the writer switches to
/// the log's free segment), a new reader reads all 60 rows, and the open read still reads the 10
/// committed before it began.
#[test]
fn d45_a_reader_that_never_finishes_holds_up_no_restart_and_no_commit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("slow.db");
    let mut writer = Db::open(&path).unwrap();
    writer.exec("CREATE TABLE t (n INTEGER)").unwrap();
    for n in 0..10 {
        writer.exec(&format!("INSERT INTO t VALUES ({n})")).unwrap();
    }
    let slow = writer.reader().unwrap();
    slow.exec("BEGIN").unwrap();
    assert_eq!(rows_in(&slow), 10);
    assert!(
        writer.restart_log().unwrap(),
        "the writer starts the log again with a read under way"
    );
    for n in 10..60 {
        writer.exec(&format!("INSERT INTO t VALUES ({n})")).unwrap();
    }
    assert_eq!(
        rows_in(&writer.reader().unwrap()),
        60,
        "a new read sees every commit"
    );
    assert_eq!(rows_in(&slow), 10, "the open read keeps its snapshot");
    slow.exec("COMMIT").unwrap();
    assert_eq!(rows_in(&slow), 60);
}

/// Decision 45 (Ada, 2026-11-24: one slow read never holds up a write; lane B27). Do: on a store
/// whose log's bound is 64 frames, its room 512 a segment, hold a read in a transaction of its own
/// and put rows until the log has filled its room with that read on the segment it closed; then let
/// the read go on to pages it had not read. The same with a room the rows never fill. Expect: every
/// row durable and read back at once throughout, and each of the log's two files within its room and
/// one turn's frames; the slow read answered that its snapshot is too old, and answered whole when
/// asked again; with room to spare, the slow read answered from its own snapshot.
#[test]
fn d45_a_slow_read_gives_way_when_the_logs_room_is_full() {
    use amphora_engine::{CompletionError, LimboError};
    for (room, ended) in [(512u64, true), (1 << 20, false)] {
        let lab = lab_set(
            Config {
                log_frames: 64,
                checkpoint_every: Duration::from_millis(1),
                ..Config::default()
            },
            |_| None,
            |db| db.keep_log(room),
        );
        for batch in 0..20 {
            let rows = (0..100).map(|n| row("slow", batch * 100 + n)).collect();
            lab.store.put(rows).unwrap();
        }
        lab.store.sync(soon()).unwrap();
        let (begun, is_begun) = std::sync::mpsc::channel::<()>();
        let (go_on, goes_on) = std::sync::mpsc::channel::<()>();
        let store = lab.store.clone();
        let slow = std::thread::spawn(move || {
            store.read(
                Some("slow"),
                Instant::now() + Duration::from_secs(120),
                move |r| {
                    r.exec("BEGIN")?;
                    r.query("SELECT seq FROM events WHERE rowid = 1", [], |_| Ok(true))?;
                    begun.send(())?;
                    goes_on.recv_timeout(Duration::from_secs(100))?;
                    let mut text = 0;
                    let read = r.query("SELECT sum(length(text)) FROM events", [], |r| {
                        text = r.get::<i64>(0).map_err(|e| anyhow::anyhow!("{e}"))?;
                        Ok(true)
                    });
                    r.exec(if read.is_ok() { "COMMIT" } else { "ROLLBACK" })?;
                    Ok(read.map(|_| text))
                },
                |_| Err(anyhow::anyhow!("the writer read")),
            )
        });
        is_begun.recv_timeout(Duration::from_secs(30)).unwrap();
        let files = ["-wal", "-wal2"].map(|suffix| {
            let mut p = lab.path.clone().into_os_string();
            p.push(suffix);
            std::path::PathBuf::from(p)
        });
        let mut most = 0;
        for seq in 0..1500 {
            lab.store.put(vec![row("fast", seq)]).unwrap();
            if seq % 3 == 0 {
                lab.store.sync(soon()).unwrap();
            }
            if seq % 30 == 0 {
                assert_eq!(count(&lab.store, "fast"), seq + 1);
                for file in &files {
                    most = most.max(std::fs::metadata(file).map_or(0, |m| m.len()));
                }
            }
        }
        lab.store.sync(soon()).unwrap();
        assert_eq!(count(&lab.store, "fast"), 1500);
        go_on.send(()).unwrap();
        let read = slow.join().unwrap().unwrap();
        if ended {
            let error = read.expect_err("the slow read kept the log past its room");
            assert!(
                error.chain().any(|cause| matches!(
                    cause.downcast_ref::<LimboError>(),
                    Some(
                        LimboError::SnapshotTooOld
                            | LimboError::CompletionError(CompletionError::SnapshotTooOld)
                    )
                )),
                "{error:#}"
            );
            assert!(
                most <= 32 + (room + 64) * (24 + 4096),
                "a file of the log grew to {most} bytes"
            );
        } else {
            let own: i64 = (0..2000)
                .map(|n| i64::try_from(format!("event {n} of slow").len()).unwrap())
                .sum();
            assert_eq!(read.unwrap(), own, "the slow read's own snapshot");
        }
        assert_eq!(count(&lab.store, "slow"), 2000);
    }
}

#[test]
fn what_is_put_is_read_back_by_its_key_at_once() {
    let lab = lab(Config::default(), None);
    for seq in 0..50 {
        lab.store.put(vec![row("a", seq)]).unwrap();
        assert_eq!(count(&lab.store, "a"), seq + 1);
    }
    assert_eq!(count(&lab.store, "b"), 0);
}

/// A read isn't held by a busy writer: it runs on its own connection, on its caller's thread, and
/// is answered while the writer is still held by a job of its own.
#[test]
fn a_read_is_not_held_by_a_busy_writer() {
    let lab = lab(Config::default(), None);
    lab.store.put(vec![row("a", 1)]).unwrap();
    lab.store.sync(soon()).unwrap();
    let busy = lab.store.clone();
    let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let job = std::thread::spawn(move || {
        busy.run(soon(), move |_| {
            started_tx.send(()).unwrap();
            // Held until the read below has been answered.
            done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            Ok(())
        })
    });
    started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(count(&lab.store, "a"), 1);
    assert!(
        !job.is_finished(),
        "the read was answered only once the writer let go"
    );
    done_tx.send(()).unwrap();
    job.join().unwrap().unwrap();
}

/// A read of what is committed doesn't wait for writes handed over and not yet committed: it
/// answers while the writer is held, with what was committed before.
#[test]
fn a_read_of_what_is_committed_waits_for_no_write() {
    let lab = lab(Config::default(), None);
    lab.store.put(vec![row("a", 1)]).unwrap();
    assert_eq!(count(&lab.store, "a"), 1);
    let busy = lab.store.clone();
    let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let job = std::thread::spawn(move || {
        busy.run(soon(), move |_| {
            held_tx.send(()).unwrap();
            go_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            Ok(())
        })
    });
    held_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    // Handed over, not committed: the writer is held.
    lab.store.put(vec![row("a", 2)]).unwrap();
    let committed = lab
        .store
        .read_committed(soon(), |r| count_on(r, "a"), |e| count_on(&mut e.db, "a"))
        .unwrap();
    assert_eq!(committed, 1);
    assert!(!job.is_finished(), "the read waited for the writer");
    go_tx.send(()).unwrap();
    job.join().unwrap().unwrap();
    assert_eq!(count(&lab.store, "a"), 2);
}

/// A sync waits for a flush begun after its commit; a read of what was committed doesn't.
#[test]
fn a_sync_waits_for_a_flush_begun_after_its_commit_and_a_read_does_not() {
    let flushes = Arc::new(AtomicUsize::new(0));
    let counted = flushes.clone();
    let slow: Flush = Box::new(move || {
        let until = Instant::now() + Duration::from_millis(250);
        while Instant::now() < until {
            std::thread::park_timeout(until - Instant::now());
        }
        counted.fetch_add(1, Ordering::Relaxed);
        Ok(())
    });
    let lab = lab(Config::default(), Some(slow));
    let began = Instant::now();
    lab.store.put(vec![row("a", 1)]).unwrap();
    assert_eq!(count(&lab.store, "a"), 1);
    assert!(
        began.elapsed() < Duration::from_millis(200),
        "the read waited for a flush: {:?}",
        began.elapsed()
    );
    lab.store.sync(soon()).unwrap();
    assert!(
        began.elapsed() >= Duration::from_millis(250),
        "the sync didn't wait for a flush: {:?}",
        began.elapsed()
    );
    assert!(flushes.load(Ordering::Relaxed) >= 1);
}

/// A batch's ticket is durable once a flush begun after its batch's commit ends, not before.
#[test]
fn a_ticket_is_durable_once_a_flush_begun_after_its_commit_ends() {
    let began = Instant::now();
    let slow: Flush = Box::new(move || {
        let until = Instant::now() + Duration::from_millis(250);
        while Instant::now() < until {
            std::thread::park_timeout(until - Instant::now());
        }
        Ok(())
    });
    let lab = lab(Config::default(), Some(slow));
    let ticket = lab.store.put(vec![row("a", 1)]).unwrap();
    lab.store.durable(&ticket, soon()).unwrap();
    assert!(
        began.elapsed() >= Duration::from_millis(250),
        "the ticket was durable before a flush ended: {:?}",
        began.elapsed()
    );
}

/// A sender of `put_then` is told once its batch is durable, after a flush begun after its commit,
/// and the sender of a batch that fails is told it failed (AC-T1.1.1).
#[test]
fn a_sender_is_told_once_its_batch_is_durable_or_that_it_failed() {
    let began = Instant::now();
    let slow: Flush = Box::new(move || {
        let until = Instant::now() + Duration::from_millis(100);
        while Instant::now() < until {
            std::thread::park_timeout(until - Instant::now());
        }
        Ok(())
    });
    let lab = lab(Config::default(), Some(slow));
    let (tx, rx) = std::sync::mpsc::channel();
    for (n, seq) in [(1, 1), (2, -1)] {
        let tell = tx.clone();
        lab.store
            .put_then(vec![row("a", seq)], soon(), move |answer| {
                tell.send((n, answer, began.elapsed())).unwrap();
            })
            .unwrap();
    }
    let mut told = std::collections::HashMap::new();
    for _ in 0..2 {
        let (n, answer, at) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        told.insert(n, (answer, at));
    }
    assert_eq!(told[&1].0, Ok(()));
    assert!(
        told[&1].1 >= Duration::from_millis(100),
        "told durable before a flush ended: {:?}",
        told[&1].1
    );
    assert_eq!(told[&2].0, Err(Refused::Failed));
    assert_eq!(count(&lab.store, "a"), 1);
}

/// A batch whose commit fails is refused to its sender, nothing of it written; the batches
/// committed with it are kept and made durable (AC-T1.1.1: no update is dropped unsaid).
#[test]
fn a_batch_that_fails_is_refused_to_its_sender_and_the_rest_of_its_commit_is_kept() {
    let lab = lab(Config::default(), None);
    // The writer held by a job while three batches queue behind it: they share one commit.
    let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let busy = lab.store.clone();
    let job = std::thread::spawn(move || {
        busy.run(soon(), move |_| {
            held_tx.send(()).unwrap();
            go_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            Ok(())
        })
    });
    held_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let first = lab.store.put(vec![row("a", 1), row("a", 2)]).unwrap();
    let bad = lab.store.put(vec![row("a", 3), row("a", -1)]).unwrap();
    let last = lab.store.put(vec![row("b", 1)]).unwrap();
    go_tx.send(()).unwrap();
    job.join().unwrap().unwrap();
    assert_eq!(lab.store.durable(&first, soon()), Ok(()));
    assert_eq!(lab.store.durable(&bad, soon()), Err(Refused::Failed));
    assert_eq!(lab.store.durable(&last, soon()), Ok(()));
    // Not even the failed batch's good row is there.
    assert_eq!(count(&lab.store, "a"), 2);
    assert_eq!(count(&lab.store, "b"), 1);
    assert_eq!(lab.store.counters().failed, 1);
}

/// Do: hand ten batches to a held writer, each a key's 400 rows of 400 bytes (about 45 pages of
/// the table), the writer keeping 200 pages (the fewest a cache keeps). Expect: more than one
/// commit and fewer than ten, each batch durable and whole; with Amphora's default pages, one
/// commit (docs/design/store.md, decision 23; AC-T1.1.1).
#[test]
fn batches_that_outgrow_the_writers_cache_take_more_than_one_commit_each_whole() {
    for (pages, one_commit) in [(Some(200), false), (None, true)] {
        // No row bound in the way: the commits are cut by pages alone.
        let config = Config {
            commit_rows: 100_000,
            ..Config::default()
        };
        let lab = lab_keeping(config, |_| None, pages);
        let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        let busy = lab.store.clone();
        let job = std::thread::spawn(move || {
            busy.run(soon(), move |_| {
                held_tx.send(()).unwrap();
                go_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                Ok(())
            })
        });
        held_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let tickets: Vec<_> = (0..10)
            .map(|b| {
                let key = format!("k{b}");
                let rows = (0..400)
                    .map(|seq| Row {
                        key: key.clone(),
                        seq,
                        text: "x".repeat(400),
                    })
                    .collect();
                lab.store.put(rows).unwrap()
            })
            .collect();
        go_tx.send(()).unwrap();
        job.join().unwrap().unwrap();
        for t in &tickets {
            assert_eq!(lab.store.durable(t, soon()), Ok(()));
        }
        for b in 0..10 {
            assert_eq!(count(&lab.store, &format!("k{b}")), 400, "batch {b}");
        }
        let commits = lab.store.counters().commits;
        if one_commit {
            assert_eq!(commits, 1, "a cache that holds them all");
        } else {
            assert!((2..10).contains(&commits), "{commits} commits");
        }
    }
}

/// Do: a transaction over ten batches of about 45 pages each, the connection keeping 200 pages
/// (the fewest a cache keeps); then one over batches whose first alone outgrows the cache. Expect:
/// the first takes more than one batch and fewer than ten and commits exactly those; the second
/// takes its first batch whole, alone (docs/design/store.md, decision 23).
#[test]
fn a_transaction_over_batches_ends_before_one_its_cache_might_not_hold() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Db::open(&dir.path().join("t.db")).unwrap();
    db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, text TEXT)")
        .unwrap();
    db.keep_pages(200).unwrap();
    let text = "x".repeat(400);
    let mut id = 0;
    let mut rows_of = |db: &mut Db, n: i64| -> anyhow::Result<()> {
        for _ in 0..n {
            id += 1;
            let params = [Value::from_i64(id), Value::from_text(text.clone())];
            db.query("INSERT INTO t VALUES (?1, ?2)", params, |_| Ok(true))?;
        }
        Ok(())
    };
    let count = |db: &mut Db| {
        let mut n = 0;
        db.query("SELECT count(*) FROM t", [], |r| {
            n = r.get::<i64>(0).map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(true)
        })
        .unwrap();
        n
    };
    let taken = db
        .transaction_while_room(0..10, |db, _| rows_of(db, 400))
        .unwrap();
    assert!((2..10).contains(&taken), "{taken} batches");
    assert_eq!(count(&mut db), 400 * i64::try_from(taken).unwrap());
    let before = count(&mut db);
    let taken = db
        .transaction_while_room([5_000, 100, 100], |db, n| rows_of(db, n))
        .unwrap();
    assert_eq!(taken, 1);
    assert_eq!(count(&mut db), before + 5_000);
}

/// The writer keeps no statement journal, so a statement that fails after writing can't be undone
/// alone: its transaction is never committed. One that returns the failure is rolled back; one
/// that ignores it and asks to commit is refused, rolled back whole; and the connection takes
/// the next transaction (docs/design/store.md, decision 7).
#[test]
fn a_transaction_holding_a_failed_statement_is_never_committed() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Db::open(&dir.path().join("t.db")).unwrap();
    db.exec("CREATE TABLE t (n INTEGER CHECK (n >= 0))")
        .unwrap();
    const ONE: &str = "INSERT INTO t (n) VALUES (?1)";
    // Writes its first row, then fails on its second.
    const TWO: &str = "INSERT INTO t (n) VALUES (?1), (?2)";
    let two = |db: &mut Db| db.query(TWO, [Value::from_i64(2), Value::from_i64(-1)], |_| Ok(true));
    let ignored = db.transaction(|db| {
        db.query(ONE, [Value::from_i64(1)], |_| Ok(true))?;
        assert!(two(db).is_err());
        Ok(())
    });
    assert!(
        ignored.is_err(),
        "a transaction with a failed statement committed"
    );
    let returned = db.transaction(|db| {
        db.query(ONE, [Value::from_i64(1)], |_| Ok(true))?;
        two(db)
    });
    assert!(returned.is_err());
    db.transaction(|db| db.query(ONE, [Value::from_i64(3)], |_| Ok(true)))
        .unwrap();
    let mut rows = Vec::new();
    db.query("SELECT n FROM t", [], |r| {
        rows.push(r.get::<i64>(0).map_err(|e| anyhow::anyhow!("{e}"))?);
        Ok(true)
    })
    .unwrap();
    assert_eq!(rows, vec![3]);
}

/// Under writes and reads that never pause, the log stays bounded: the checkpointer copies it and
/// the writer starts it again between commits, holding the readers back for the moment it takes.
#[test]
fn the_log_stays_bounded_under_writes_and_reads_that_never_pause() {
    let lab = lab(
        Config {
            log_frames: 256,
            ..Config::default()
        },
        None,
    );
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let (store, stop) = (lab.store.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    count(&store, "a");
                }
            })
        })
        .collect();
    let wal = {
        let mut p = lab.path.clone().into_os_string();
        p.push("-wal");
        std::path::PathBuf::from(p)
    };
    let mut most = 0;
    for seq in 0..4000 {
        lab.store.put(vec![row("a", seq)]).unwrap();
        if seq % 100 == 0 {
            most = most.max(std::fs::metadata(&wal).map_or(0, |m| m.len()));
        }
    }
    lab.store.sync(soon()).unwrap();
    stop.store(true, Ordering::Relaxed);
    for r in readers {
        r.join().unwrap();
    }
    assert_eq!(count(&lab.store, "a"), 4000);
    // 256 frames of 4 KiB is 1 MiB; a log that never started again holds every commit's pages.
    assert!(most < 16 << 20, "the log grew to {most} bytes");
}

/// Do: write commits of 500 rows on a store whose log's bound is 12,288 frames until the log holds
/// at least 10,240, past the 8,192 a pass once waited for. Expect: no checkpoint pass while the log is
/// within its bound, since a recovery reads the whole log whatever was copied (docs/design/
/// store.md, decision 35, 2026-11-11: the copy waits for the bound, not a picked count of frames);
/// then, once the log has passed its bound, passes that copy it, and every row read back
/// (AC-T1.4.4).
#[test]
fn a_log_within_its_bound_is_not_copied() {
    let config = Config {
        checkpoint_every: Duration::from_millis(1),
        log_frames: 12_288,
        ..Config::default()
    };
    let bound = config.log_frames;
    let lab = lab(config, None);
    let wal = {
        let mut p = lab.path.clone().into_os_string();
        p.push("-wal");
        std::path::PathBuf::from(p)
    };
    // The log's header and its frames, each a 24-byte header and a 4 KiB page.
    let frames = || std::fs::metadata(&wal).map_or(0, |m| m.len().saturating_sub(32) / (24 + 4096));
    // A key of 1,000 bytes, in the table and its index: a commit of 500 rows writes hundreds of
    // pages, so the log reaches its bound in a few dozen commits.
    let key = "a".repeat(1_000);
    let mut seq = 0;
    let put = |seq: &mut i64| {
        let rows = (0..500).map(|i| row(&key, *seq + i)).collect();
        lab.store.put(rows).unwrap();
        *seq += 500;
    };
    while frames() < 10_240 {
        put(&mut seq);
        lab.store.sync(soon()).unwrap();
    }
    assert!(frames() < bound, "{} frames", frames());
    assert_eq!(
        lab.store.counters().checkpoints,
        0,
        "passes with {} frames in a log of bound {bound}",
        frames()
    );
    while lab.store.counters().checkpoints == 0 {
        put(&mut seq);
        lab.store.sync(soon()).unwrap();
    }
    assert!(frames() > bound, "a pass at {} frames", frames());
    assert_eq!(count(&lab.store, &key), seq);
}

/// Do: let one flush go, then hold every flush until let go and write until the log is past its
/// bound (64 frames) and short of twice it, so the checkpointer has nothing durable to copy, and
/// watch it for 500 ms; then let the flushes go. Expect: about a pass a checkpoint period (100 ms
/// here) in those 500 ms, where it had passed again and again without pause, each pass taking locks
/// every commit takes; every row durable and read back after (docs/design/store.md, decision 25).
#[test]
fn a_checkpointer_with_nothing_durable_to_copy_waits_for_a_flush() {
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let go = std::sync::Mutex::new(go_rx);
    let config = Config {
        log_frames: 64,
        checkpoint_every: Duration::from_millis(100),
        ..Config::default()
    };
    let lab = lab_keeping(
        config,
        move |db| {
            let flush = db.flusher();
            let held: Flush = Box::new(move || {
                // Held until let go, or until the test lets go of every flush at once.
                let _ = go.lock().unwrap().recv();
                flush()
            });
            Some(held)
        },
        None,
    );
    let wal = {
        let mut p = lab.path.clone().into_os_string();
        p.push("-wal");
        std::path::PathBuf::from(p)
    };
    // The log's header and its frames, each a 24-byte header and a 4 KiB page.
    let frames = || std::fs::metadata(&wal).map_or(0, |m| m.len().saturating_sub(32) / (24 + 4096));
    // One flush let go: the log's durable end is known, and checkpoints copy only up to it.
    lab.store.put(vec![row("a", 0)]).unwrap();
    go_tx.send(()).unwrap();
    lab.store.sync(soon()).unwrap();
    let mut seq = 1;
    while frames() <= 64 {
        lab.store.put(vec![row("a", seq)]).unwrap();
        seq += 1;
        assert_eq!(count(&lab.store, "a"), seq);
    }
    assert!(frames() < 128, "{} frames", frames());
    let before = lab.store.counters().checkpoints;
    let until = Instant::now() + Duration::from_millis(500);
    while Instant::now() < until {
        std::thread::park_timeout(until - Instant::now());
    }
    let passes = lab.store.counters().checkpoints - before;
    assert!(passes <= 10, "{passes} passes with nothing durable to copy");
    drop(go_tx);
    lab.store.sync(soon()).unwrap();
    assert_eq!(count(&lab.store, "a"), seq);
}

/// While the engine replaces its file, its readers let it go and reads go to the writer; then they
/// open on the new one.
#[test]
fn reads_go_on_while_the_engine_replaces_its_file() {
    let lab = lab(Config::default(), None);
    lab.store.put(vec![row("a", 1)]).unwrap();
    assert_eq!(count(&lab.store, "a"), 1);
    lab.next
        .lock()
        .unwrap()
        .extend([ForReaders::Again, ForReaders::Release]);
    // Upkeep runs every 20 ms while nothing comes: both are taken.
    let until = Instant::now() + Duration::from_secs(10);
    while !lab.next.lock().unwrap().is_empty() {
        assert!(Instant::now() < until, "the upkeep wasn't asked");
        std::thread::park_timeout(Duration::from_millis(5));
    }
    lab.store.put(vec![row("a", 2)]).unwrap();
    assert_eq!(count(&lab.store, "a"), 2);
}

/// A flush the test holds while it runs: how many began, and whether the first may end yet.
struct Held {
    state: std::sync::Mutex<(u64, bool)>,
    moved: std::sync::Condvar,
}

impl Held {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: std::sync::Mutex::new((0, true)),
            moved: std::sync::Condvar::new(),
        })
    }

    /// A flush that counts itself and holds the first until [`Held::end_first`].
    fn flush(self: &Arc<Self>) -> Flush {
        let me = self.clone();
        Box::new(move || {
            let mut s = me.state.lock().unwrap();
            s.0 += 1;
            me.moved.notify_all();
            while s.0 == 1 && s.1 {
                s = me.moved.wait(s).unwrap();
            }
            Ok(())
        })
    }

    fn first_began(&self) {
        let mut s = self.state.lock().unwrap();
        while s.0 == 0 {
            s = self.moved.wait(s).unwrap();
        }
    }

    fn end_first(&self) {
        self.state.lock().unwrap().1 = false;
        self.moved.notify_all();
    }

    fn began(&self) -> u64 {
        self.state.lock().unwrap().0
    }
}

/// A flush waits for the batches handed to the writer before it begins: one committed during the
/// last flush and one the busy writer had not yet taken are made durable by one flush, the next
/// (docs/design/store.md, decision 14; AC-T1.1.1).
#[test]
fn a_batch_handed_before_a_flush_begins_is_made_durable_by_it() {
    let held = Held::new();
    let lab = lab(Config::default(), Some(held.flush()));
    let store = &lab.store;
    store.put(vec![row("p", 1)]).unwrap();
    held.first_began();
    let x = store.put(vec![row("x", 1)]).unwrap();
    assert_eq!(
        count(store, "x"),
        1,
        "x is committed during the first flush"
    );
    let job = Arc::new((
        std::sync::Mutex::new((false, false)),
        std::sync::Condvar::new(),
    ));
    std::thread::scope(|s| {
        let busy = job.clone();
        s.spawn(move || {
            store
                .run(soon(), move |_e| {
                    let (m, cv) = &*busy;
                    let mut j = m.lock().unwrap();
                    j.0 = true;
                    cv.notify_all();
                    while !j.1 {
                        j = cv.wait(j).unwrap();
                    }
                    Ok(())
                })
                .unwrap();
        });
        let (m, cv) = &*job;
        let mut j = m.lock().unwrap();
        while !j.0 {
            j = cv.wait(j).unwrap();
        }
        drop(j);
        let y = store.put(vec![row("y", 1)]).unwrap();
        held.end_first();
        m.lock().unwrap().1 = true;
        cv.notify_all();
        store.durable(&y, soon()).unwrap();
    });
    store.durable(&x, soon()).unwrap();
    assert_eq!(
        held.began(),
        2,
        "x and y were made durable by one flush after the first"
    );
}

/// A sender told its batch is durable that never sends again holds no flush: what was committed
/// during the flush that told it is made durable by the next (docs/design/store.md, decision 14;
/// AC-T1.1.1).
#[test]
fn a_sender_told_durable_that_sends_nothing_more_holds_no_flush() {
    let held = Held::new();
    let lab = lab(Config::default(), Some(held.flush()));
    let store = &lab.store;
    let p = store.put(vec![row("p", 1)]).unwrap();
    held.first_began();
    std::thread::scope(|s| {
        let told = s.spawn(|| store.durable(&p, soon()));
        let x = store.put(vec![row("x", 1)]).unwrap();
        assert_eq!(
            count(store, "x"),
            1,
            "x is committed during the first flush"
        );
        held.end_first();
        told.join().unwrap().unwrap();
        store.durable(&x, soon()).unwrap();
    });
    assert_eq!(held.began(), 2);
}
