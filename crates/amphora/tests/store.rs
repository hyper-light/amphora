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

use amphora::store::{Config, Engine, Flush, ForReaders, Opener, Refused, Store, Upkept};
use amphora::turso::Db;
use turso_core::Value;

struct Events {
    db: Db,
    /// The log's bound for the checkpointer (small, so a test turns it over quickly).
    frames: u64,
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
    type Reader = (Db, u64);

    fn key(row: &Row) -> &str {
        &row.key
    }

    fn apply(&mut self, rows: Vec<Row>) -> anyhow::Result<usize> {
        let n = rows.len();
        self.db.transaction(|db| {
            for r in &rows {
                let params = [
                    Value::from_text(r.key.clone()),
                    Value::from_i64(r.seq),
                    Value::from_text(r.text.clone()),
                ];
                db.query(INSERT, &params, |_| Ok(true))?;
            }
            Ok(())
        })?;
        Ok(n)
    }

    fn flusher(&self) -> Option<Flush> {
        Some(self.db.flusher())
    }

    fn opener(&self) -> Option<Opener<(Db, u64)>> {
        let (open, frames) = (self.db.opener(), self.frames);
        Some(Arc::new(move || Ok((open()?, frames))))
    }

    fn checkpoint(reader: &mut (Db, u64)) -> anyhow::Result<bool> {
        let frames = reader.1;
        reader.0.checkpoint_past(frames)
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

fn count_on(db: &mut Db, key: &str) -> anyhow::Result<i64> {
    let mut n = 0;
    db.query(COUNT, &[Value::from_text(key.to_owned())], |r| {
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

fn lab(config: Config, frames: u64, flush: Option<Flush>) -> Lab {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.db");
    let db = Db::open(&path).unwrap();
    db.exec("CREATE TABLE events (key TEXT, seq INTEGER, text TEXT); CREATE INDEX by_key ON events (key, seq)")
        .unwrap();
    let next = Arc::new(std::sync::Mutex::new(Vec::new()));
    let engine = Events {
        db,
        frames,
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
            move |r| count_on(&mut r.0, &k),
            move |e| count_on(&mut e.db, &w),
        )
        .unwrap()
}

/// What is put is read back by its key at once: a read waits for its key's writes handed over
/// before it, and no longer.
#[test]
fn what_is_put_is_read_back_by_its_key_at_once() {
    let lab = lab(Config::default(), amphora::turso::LOG_FRAMES, None);
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
    let lab = lab(Config::default(), amphora::turso::LOG_FRAMES, None);
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
    let lab = lab(Config::default(), amphora::turso::LOG_FRAMES, Some(slow));
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

/// A full queue refuses what it can't take, typed and counted; a sender that waits for room is
/// taken once the writer frees some.
#[test]
fn a_full_queue_refuses_typed_and_counted_and_a_wait_for_room_is_taken() {
    let config = Config {
        queue: 1,
        ..Config::default()
    };
    let lab = lab(config, amphora::turso::LOG_FRAMES, None);
    // The writer held by a job, the queue filled behind it.
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
    lab.store.put(vec![row("a", 1)]).unwrap();
    assert_eq!(lab.store.put(vec![row("a", 2)]), Err(Refused::Full));
    assert_eq!(lab.store.counters().dropped, 1);
    // A wait for room is taken once the writer goes on.
    let waiting = lab.store.clone();
    let waiter = std::thread::spawn(move || waiting.put_wait(vec![row("a", 3)], soon()));
    go_tx.send(()).unwrap();
    waiter.join().unwrap().unwrap();
    job.join().unwrap().unwrap();
    lab.store.sync(soon()).unwrap();
    // The refused row isn't there; the others are.
    assert_eq!(count(&lab.store, "a"), 2);
}

/// Under writes and reads that never pause, the log stays bounded: the checkpointer copies it and
/// the writer starts it again between commits, holding the readers back for the moment it takes.
#[test]
fn the_log_stays_bounded_under_writes_and_reads_that_never_pause() {
    let lab = lab(Config::default(), 256, None);
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

/// While the engine replaces its file, its readers let it go and reads go to the writer; then they
/// open on the new one.
#[test]
fn reads_go_on_while_the_engine_replaces_its_file() {
    let lab = lab(Config::default(), amphora::turso::LOG_FRAMES, None);
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
