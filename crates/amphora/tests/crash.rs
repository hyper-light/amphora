//! Crash injection (T-T1.1.1; AC-T1.1.1: no update is dropped unsaid): the store on a simulated
//! disk that loses its power at a chosen change. What a flush made durable survives; each change
//! made since survives whole, torn or not at all, in any combination. After the restart, every batch
//! acknowledged durable is there, every batch the engine refused isn't, every other batch is there
//! whole or not at all, and the database passes Turso's integrity check.

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

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, Instant};

use amphora::store::{Checkpointed, Config, Engine, Flush, Opener, Refused, Store};
use amphora::turso::Db;
use turso_core::io::{FileId, FileSyncType};
use turso_core::{
    Buffer, Clock, Completion, CompletionError, File, IO, LimboError, MonotonicInstant, OpenFlags,
    Value, WallClockInstant,
};

/// A seeded generator (xorshift64*): a seed's workload and losses are its own.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// A change made to a file since its last flush.
#[derive(Clone)]
enum Change {
    Write(u64, Vec<u8>),
    Truncate(u64),
}

fn apply(bytes: &mut Vec<u8>, change: &Change) {
    match change {
        Change::Write(pos, data) => {
            let pos = usize::try_from(*pos).unwrap();
            let end = pos + data.len();
            if bytes.len() < end {
                bytes.resize(end, 0);
            }
            bytes[pos..end].copy_from_slice(data);
        }
        Change::Truncate(len) => bytes.resize(usize::try_from(*len).unwrap(), 0),
    }
}

/// What a file holds: what its last flush made durable, the changes made since (in order), and
/// what a reader sees (both).
#[derive(Default)]
struct Held {
    durable: Vec<u8>,
    since: Vec<Change>,
    seen: Vec<u8>,
}

/// Gives each disk its own files: Turso keeps one database open per file a process opens, so a
/// disk restarted on what survived must not name the crashed one's.
static DISKS: AtomicU64 = AtomicU64::new(0);

/// A disk whose power goes at the `crash_at`th change made after it is armed.
struct Disk {
    me: Weak<Disk>,
    id: u64,
    files: Mutex<HashMap<String, Arc<SimFile>>>,
    /// Held by each change as it is made, and by the power cut alone: a change is wholly before the
    /// cut or after it.
    order: RwLock<()>,
    changes: AtomicU64,
    crash_at: AtomicU64,
    /// What survived the cut, file by file.
    image: Mutex<Option<HashMap<String, Vec<u8>>>>,
    rng: Mutex<Rng>,
}

fn power_out() -> LimboError {
    LimboError::CompletionError(CompletionError::IOError(
        std::io::ErrorKind::Other,
        "the power is out",
    ))
}

impl Disk {
    fn new(seed: u64, image: HashMap<String, Vec<u8>>) -> Arc<Self> {
        Arc::new_cyclic(|me: &Weak<Disk>| {
            let files = image
                .into_iter()
                .map(|(path, bytes)| {
                    let file = Arc::new(SimFile {
                        disk: me.clone(),
                        held: Mutex::new(Held {
                            durable: bytes.clone(),
                            since: Vec::new(),
                            seen: bytes,
                        }),
                    });
                    (path, file)
                })
                .collect();
            Disk {
                me: me.clone(),
                id: DISKS.fetch_add(1, Ordering::Relaxed),
                files: Mutex::new(files),
                order: RwLock::new(()),
                changes: AtomicU64::new(0),
                crash_at: AtomicU64::new(u64::MAX),
                image: Mutex::new(None),
                rng: Mutex::new(Rng(seed | 1)),
            }
        })
    }

    /// The power goes at the `at`th change from now.
    fn arm(&self, at: u64) {
        self.changes.store(0, Ordering::SeqCst);
        self.crash_at.store(at, Ordering::SeqCst);
    }

    fn crashed(&self) -> bool {
        self.image.lock().unwrap().is_some()
    }

    /// `f` made to `held`, unless the power is out; the change that reaches the crash point cuts it.
    fn change(&self, held: &Mutex<Held>, f: impl FnOnce(&mut Held)) -> turso_core::Result<()> {
        {
            let _ordered = self.order.read().unwrap();
            let n = self.changes.fetch_add(1, Ordering::SeqCst) + 1;
            if n < self.crash_at.load(Ordering::SeqCst) && !self.crashed() {
                f(&mut held.lock().unwrap());
                return Ok(());
            }
        }
        self.cut();
        Err(power_out())
    }

    /// The power cut: each file keeps what its last flush made durable, and each change since
    /// whole, torn at a 512-byte boundary, or not at all.
    fn cut(&self) {
        let _alone = self.order.write().unwrap();
        let mut image = self.image.lock().unwrap();
        if image.is_some() {
            return;
        }
        let files = self.files.lock().unwrap();
        let mut rng = self.rng.lock().unwrap();
        let mut survived = HashMap::new();
        for (path, file) in files.iter() {
            let held = file.held.lock().unwrap();
            let mut bytes = held.durable.clone();
            for change in &held.since {
                match (rng.below(4), change) {
                    (0 | 1, change) => apply(&mut bytes, change),
                    (2, _) => {}
                    (_, Change::Write(pos, data)) => {
                        let kept =
                            usize::try_from(rng.below(data.len() as u64 / 512 + 1) * 512).unwrap();
                        let torn = Change::Write(*pos, data[..kept.min(data.len())].to_vec());
                        apply(&mut bytes, &torn);
                    }
                    (_, change) => apply(&mut bytes, change),
                }
            }
            survived.insert(path.clone(), bytes);
        }
        *image = Some(survived);
    }

    /// What survived: the power is cut now if it wasn't already.
    fn image(&self) -> HashMap<String, Vec<u8>> {
        self.cut();
        self.image.lock().unwrap().clone().unwrap()
    }
}

struct SimFile {
    disk: Weak<Disk>,
    held: Mutex<Held>,
}

impl SimFile {
    fn change(&self, f: impl FnOnce(&mut Held)) -> turso_core::Result<()> {
        let disk = self.disk.upgrade().ok_or_else(power_out)?;
        disk.change(&self.held, f)
    }

    fn write(&self, pos: u64, data: Vec<u8>, c: Completion) -> turso_core::Result<Completion> {
        let n = i32::try_from(data.len()).unwrap();
        self.change(|held| {
            let change = Change::Write(pos, data);
            apply(&mut held.seen, &change);
            held.since.push(change);
        })?;
        c.complete(n);
        Ok(c)
    }
}

impl File for SimFile {
    fn lock_file(&self, _exclusive: bool) -> turso_core::Result<()> {
        Ok(())
    }

    fn unlock_file(&self) -> turso_core::Result<()> {
        Ok(())
    }

    fn pread(&self, pos: u64, c: Completion) -> turso_core::Result<Completion> {
        let n = {
            let held = self.held.lock().unwrap();
            let dst = c.as_read().buf().as_mut_slice();
            let pos = usize::try_from(pos).unwrap();
            let n = held.seen.len().saturating_sub(pos).min(dst.len());
            dst[..n].copy_from_slice(&held.seen[pos..pos + n]);
            n
        };
        c.complete(i32::try_from(n).unwrap());
        Ok(c)
    }

    fn pwrite(
        &self,
        pos: u64,
        buffer: Arc<Buffer>,
        c: Completion,
    ) -> turso_core::Result<Completion> {
        self.write(pos, buffer.as_slice().to_vec(), c)
    }

    fn pwritev(
        &self,
        pos: u64,
        buffers: Vec<Arc<Buffer>>,
        c: Completion,
    ) -> turso_core::Result<Completion> {
        let data = buffers
            .iter()
            .flat_map(|b| b.as_slice().iter().copied())
            .collect();
        self.write(pos, data, c)
    }

    fn sync(&self, c: Completion, _sync_type: FileSyncType) -> turso_core::Result<Completion> {
        self.change(|held| {
            for change in held.since.drain(..) {
                apply(&mut held.durable, &change);
            }
        })?;
        c.complete(0);
        Ok(c)
    }

    fn truncate(&self, len: u64, c: Completion) -> turso_core::Result<Completion> {
        self.change(|held| {
            let change = Change::Truncate(len);
            apply(&mut held.seen, &change);
            held.since.push(change);
        })?;
        c.complete(0);
        Ok(c)
    }

    fn size(&self) -> turso_core::Result<u64> {
        Ok(self.held.lock().unwrap().seen.len() as u64)
    }
}

impl Clock for Disk {
    fn current_time_monotonic(&self) -> MonotonicInstant {
        MonotonicInstant::now()
    }

    fn current_time_wall_clock(&self) -> WallClockInstant {
        WallClockInstant::now()
    }
}

impl IO for Disk {
    fn open_file(
        &self,
        path: &str,
        flags: OpenFlags,
        _direct: bool,
    ) -> turso_core::Result<Arc<dyn File>> {
        let mut files = self.files.lock().unwrap();
        if let Some(file) = files.get(path) {
            return Ok(file.clone());
        }
        if !flags.contains(OpenFlags::Create) {
            return Err(LimboError::CompletionError(CompletionError::IOError(
                std::io::ErrorKind::NotFound,
                "open",
            )));
        }
        let file = Arc::new(SimFile {
            disk: self.me.clone(),
            held: Mutex::new(Held::default()),
        });
        files.insert(path.to_owned(), file.clone());
        Ok(file)
    }

    fn remove_file(&self, path: &str) -> turso_core::Result<()> {
        self.files.lock().unwrap().remove(path);
        Ok(())
    }

    fn file_id(&self, path: &str) -> turso_core::Result<FileId> {
        Ok(FileId::from_path_hash(&format!("{}:{path}", self.id)))
    }
}

/// Keyed events, each tagged with its batch; a row of negative seq fails the table's CHECK.
struct Events {
    db: Db,
}

struct Row {
    key: String,
    seq: i64,
    batch: usize,
}

const PATH: &str = "/crash/events.db";
const INSERT: &str = "INSERT INTO events (key, seq, batch) VALUES (?1, ?2, ?3)";

impl Engine for Events {
    type Row = Row;
    type Reader = Db;

    fn key(row: &Row) -> &str {
        &row.key
    }

    fn apply(&mut self, rows: &[Row]) -> anyhow::Result<usize> {
        self.db.transaction(|db| {
            for r in rows {
                let params = [
                    Value::from_text(r.key.clone()),
                    Value::from_i64(r.seq),
                    Value::from_i64(i64::try_from(r.batch).unwrap()),
                ];
                db.query(INSERT, params, |_| Ok(true))?;
            }
            Ok(rows.len())
        })
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

    fn restart_log(&mut self) -> anyhow::Result<bool> {
        self.db.restart_log()
    }
}

/// What became of a batch: how many rows it had, whether the engine was bound to refuse it, and
/// what its sender was told.
struct Sent {
    rows: usize,
    bad: bool,
    told: Option<Result<(), Refused>>,
}

/// The crash point is drawn up to here: a whole workload makes 174-197 changes (percentiles
/// 10-90, measured by this test), so most seeds lose the power midway.
const CHANGES: u64 = 200;

/// What a seed exercised, so a run can show its checks weren't vacuous.
#[derive(Default)]
struct Seen {
    /// The power went out before the workload was done.
    cut_midway: bool,
    /// Changes made to the disk until the cut, or the workload's end.
    changes: u64,
    /// The checkpoints and restarts of the log the store made before the cut.
    checkpoints: u64,
    restarts: u64,
    acknowledged: usize,
    refused: usize,
}

/// One seed: a workload on a disk that loses its power at a change the seed draws, then the
/// restart and its checks. The failure, if any, names what was wrong.
fn one_seed(seed: u64) -> Result<Seen, String> {
    let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let disk = Disk::new(seed, HashMap::new());
    let path = std::path::Path::new(PATH);
    let db = Db::open_with(path, disk.clone()).map_err(|e| format!("open: {e:#}"))?;
    db.exec("CREATE TABLE events (key TEXT, seq INTEGER CHECK (seq >= 0), batch INTEGER); CREATE INDEX by_key ON events (key, seq)")
        .map_err(|e| format!("schema: {e:#}"))?;
    // The schema durable first: the crash point is drawn among the workload's changes.
    db.flusher()().map_err(|e| format!("flush: {e}"))?;
    let config = Config {
        log_frames: 16,
        checkpoint_every: Duration::from_millis(1),
        ..Config::default()
    };
    let store = Store::start(Events { db }, config).map_err(|e| format!("start: {e:#}"))?;
    disk.arm(1 + rng.below(CHANGES));

    let soon = || Instant::now() + Duration::from_secs(10);
    let mut sent: Vec<Sent> = Vec::new();
    while sent.len() < 150 && !disk.crashed() {
        // A few batches handed over together, then each waited for: they share commits.
        let mut tickets = Vec::new();
        for _ in 0..1 + rng.below(3) {
            let batch = sent.len();
            let n = 1 + usize::try_from(rng.below(4)).unwrap();
            let bad = rng.below(10) == 0;
            let rows = (0..n)
                .map(|i| Row {
                    key: format!("k{}", rng.below(5)),
                    seq: if bad && i + 1 == n {
                        -1
                    } else {
                        i64::try_from(batch * 10 + i).unwrap()
                    },
                    batch,
                })
                .collect();
            tickets.push((batch, store.put_wait(rows, soon())));
            sent.push(Sent {
                rows: n,
                bad,
                told: None,
            });
        }
        for (batch, ticket) in tickets {
            sent[batch].told = Some(match ticket {
                Ok(t) => store.durable(&t, soon()),
                Err(e) => Err(e),
            });
        }
    }
    let counted = store.counters();
    drop(store);
    let mut seen = Seen {
        cut_midway: disk.crashed(),
        changes: disk.changes.load(Ordering::SeqCst),
        checkpoints: counted.checkpoints,
        restarts: counted.restarts,
        ..Seen::default()
    };

    // The restart, on what survived.
    let disk = Disk::new(seed, disk.image());
    let mut db = Db::open_with(path, disk).map_err(|e| format!("reopen: {e:#}"))?;
    let mut check = String::new();
    db.query("PRAGMA integrity_check", [], |r| {
        check = r.get::<String>(0).map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(true)
    })
    .map_err(|e| format!("integrity_check: {e:#}"))?;
    if check != "ok" {
        return Err(format!("integrity_check: {check}"));
    }
    let mut found: HashMap<usize, usize> = HashMap::new();
    db.query("SELECT batch FROM events", [], |r| {
        let batch = r.get::<i64>(0).map_err(|e| anyhow::anyhow!("{e}"))?;
        *found.entry(usize::try_from(batch).unwrap()).or_default() += 1;
        Ok(true)
    })
    .map_err(|e| format!("rows: {e:#}"))?;
    for (batch, s) in sent.iter().enumerate() {
        let there = found.remove(&batch).unwrap_or(0);
        let acked = matches!(s.told, Some(Ok(())));
        if acked && there != s.rows {
            return Err(format!(
                "batch {batch} was acknowledged durable; {there} of its {} rows survived",
                s.rows
            ));
        }
        if s.bad && there != 0 {
            return Err(format!(
                "batch {batch} was refused by the engine; {there} of its rows are there"
            ));
        }
        if s.bad && acked {
            return Err(format!(
                "batch {batch} holds a row the engine refuses, and was acknowledged"
            ));
        }
        if there != 0 && there != s.rows {
            return Err(format!(
                "batch {batch} survived in part: {there} of its {} rows",
                s.rows
            ));
        }
        seen.acknowledged += usize::from(acked);
        seen.refused += usize::from(matches!(s.told, Some(Err(Refused::Failed))));
    }
    if let Some((batch, n)) = found.into_iter().next() {
        return Err(format!("{n} rows of batch {batch}, which was never sent"));
    }
    Ok(seen)
}

/// `n` seeds from `from`: every one passes, and between them they cut the power midway in most,
/// and acknowledged and refused batches to check.
fn seeds(n: u64, from: u64) {
    let (mut failed, mut midway, mut acknowledged, mut refused) =
        (Vec::new(), 0u64, 0usize, 0usize);
    let (mut checkpoints, mut restarts) = (0u64, 0u64);
    // What whole workloads changed, to draw crash points from.
    let mut whole: Vec<u64> = Vec::new();
    for seed in from..from + n {
        match one_seed(seed) {
            Ok(seen) => {
                if !seen.cut_midway {
                    whole.push(seen.changes);
                }
                midway += u64::from(seen.cut_midway);
                acknowledged += seen.acknowledged;
                refused += seen.refused;
                checkpoints += seen.checkpoints;
                restarts += seen.restarts;
            }
            Err(e) => failed.push((seed, e)),
        }
    }
    assert!(
        failed.is_empty(),
        "{} of {n} seeds failed; the first: seed {} ({})",
        failed.len(),
        failed[0].0,
        failed[0].1
    );
    whole.sort_unstable();
    assert!(
        midway * 2 > n,
        "the power went out midway in only {midway} of {n} seeds (whole workloads changed the disk {:?} times, by percentile 10/50/90)",
        [whole.len() / 10, whole.len() / 2, whole.len() * 9 / 10]
            .map(|i| whole.get(i).copied().unwrap_or(0))
    );
    assert!(
        acknowledged > 0 && refused > 0,
        "{acknowledged} acknowledged and {refused} refused batches checked"
    );
    assert!(
        checkpoints > 0 && restarts > 0,
        "{checkpoints} checkpoints and {restarts} restarts of the log before a cut"
    );
}

/// T-T1.1.1, a sample run in every gate: a power cut at a change drawn from each seed's workload,
/// and the restart keeps every acknowledged batch, no refused one, and every other whole or not at
/// all.
#[test]
fn a_power_cut_at_any_change_loses_nothing_acknowledged_and_tears_no_batch() {
    seeds(200, 1);
}

/// T-T1.1.1 in full: 10,000 seeds (`cargo test --release --test crash -- --ignored`; from another
/// seed with AMPHORA_CRASH_FROM).
#[test]
#[ignore = "10,000 seeds: run on its own, in release"]
fn ten_thousand_power_cuts() {
    let from = std::env::var("AMPHORA_CRASH_FROM")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    seeds(10_000, from);
}
