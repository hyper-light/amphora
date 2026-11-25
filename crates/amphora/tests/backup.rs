//! Restore drills (CP-T1.12, AC-T1.12.4; docs/design/backup.md): a database written at random, its
//! log checkpointed and started again at random, shipped and copied to the object store's simulator
//! between its commits; then restores to random seconds, each compared row by row with the database
//! as it stood at that second.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use amphora::backup::{self, Archive, BaseCopy, Hooks, SECOND_NS, Shipper, plan::Unrestorable};
use amphora::database::{AnswerMax, Database};
use amphora::db::Db;
use amphora::memory::Use;
use amphora_api::blobs::{Blobs, SimBlobs};
use amphora_engine::Value;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

type Rows = BTreeMap<i64, String>;

fn rows(db: &Db) -> Rows {
    let mut out = Rows::new();
    db.query("SELECT id, v FROM t ORDER BY id", [], |r| {
        out.insert(
            r.get::<i64>(0).map_err(|e| anyhow::anyhow!("{e}"))?,
            r.get::<String>(1).map_err(|e| anyhow::anyhow!("{e}"))?,
        );
        Ok(true)
    })
    .unwrap();
    out
}

fn until() -> Instant {
    Instant::now() + Duration::from_secs(60)
}

/// The rows as of `second`: after the last commit stamped at or before it.
fn at(history: &[(u64, Rows)], second: u64) -> Option<&Rows> {
    history
        .iter()
        .rev()
        .find(|(ns, _)| *ns <= second * SECOND_NS)
        .map(|(_, r)| r)
}

/// T-T1.12.D1 (AC-T1.12.4): restore drills at random seconds pass when every row is equal.
#[test]
fn t_t1_12_d1_restore_drills_at_random_seconds() {
    for seed in [
        0x9e37_79b9_7f4a_7c15u64,
        0x2545_f491_4f6c_dd1d,
        0x1234_5678_9abc_def1,
    ] {
        drill(seed);
    }
}

fn drill(seed: u64) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.db");
    let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS));
    let read = Arc::clone(&now);
    let hooks = Hooks::new(Arc::new(move || read.load(Ordering::SeqCst)), 1 << 16);
    let mut db = Db::open(&path).unwrap();
    db.back_up(Arc::clone(&hooks));
    // Small objects, so segments split into parts and bases into many chunks.
    let blobs = SimBlobs::new(64 * 1024, 1_000_000, 1 << 30);
    let mut rng = Rng(seed);
    let tick = |rng: &mut Rng| {
        now.fetch_add(1_000 + rng.below(900_000_000), Ordering::SeqCst);
    };
    db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")
        .unwrap();
    let mut model = Rows::new();
    let mut history: Vec<(u64, Rows)> = Vec::new();
    let mut shipper = Shipper::open("db", Arc::clone(&hooks), &blobs, &db, until()).unwrap();
    let mut copy: Option<BaseCopy> = None;
    let mut first_base_end: Option<u64> = None;
    let mut version = 0u64;
    // A restart forced while commits weren't shipped breaks the log (backup.md): from the last ship
    // before it until a base begun after it ends, seconds are refused, not restored wrong.
    let mut dirty = false;
    let mut shipped_at = now.load(Ordering::SeqCst);
    let mut breaks: Vec<(u64, u64)> = Vec::new();
    let mut bases: Vec<(u64, u64)> = Vec::new();
    let mut base_began = 0u64;
    for _ in 0..600 {
        tick(&mut rng);
        match rng.below(20) {
            0..=10 => {
                let stamp = now.load(Ordering::SeqCst);
                let mut next = model.clone();
                db.transaction(|db| {
                    for _ in 0..=rng.below(6) {
                        let id = rng.below(300) as i64;
                        version += 1;
                        if rng.below(5) == 0 {
                            db.query_owned(
                                "DELETE FROM t WHERE id = ?1",
                                [Value::from_i64(id)],
                                |_| Ok(true),
                            )?;
                            next.remove(&id);
                        } else {
                            let v = format!("{version}:{}", "x".repeat(rng.below(2500) as usize));
                            db.query_owned(
                                "INSERT OR REPLACE INTO t (id, v) VALUES (?1, ?2)",
                                [Value::from_i64(id), Value::from_text(v.clone())],
                                |_| Ok(true),
                            )?;
                            next.insert(id, v);
                        }
                    }
                    Ok(())
                })
                .unwrap();
                model = next;
                history.push((stamp, model.clone()));
                dirty = true;
            }
            11 | 12 => (db.flusher())().unwrap(),
            13 => {
                let mut reader = db.reader().unwrap();
                reader.checkpoint().unwrap();
            }
            14 => {
                // Refused while the log isn't shipped whole: tried again later, as the store does.
                if db.restart_log().unwrap() && dirty {
                    breaks.push((shipped_at, now.load(Ordering::SeqCst)));
                }
            }
            15 | 16 => {
                shipper.ship(&db, &blobs, until()).unwrap();
                (dirty, shipped_at) = (false, now.load(Ordering::SeqCst));
            }
            _ => match copy.as_mut() {
                None => {
                    shipper.ship(&db, &blobs, until()).unwrap();
                    (dirty, shipped_at) = (false, now.load(Ordering::SeqCst));
                    copy = shipper
                        .begin_base(&db, &blobs, &amphora::memory::process(), 32 * 1024)
                        .ok();
                    base_began = now.load(Ordering::SeqCst);
                }
                Some(c) => {
                    if !shipper.copy_base(&db, &blobs, c, until()).unwrap() {
                        let c = copy.take().unwrap();
                        shipper.end_base(&db, &blobs, c, until()).unwrap();
                        (dirty, shipped_at) = (false, now.load(Ordering::SeqCst));
                        first_base_end.get_or_insert(now.load(Ordering::SeqCst));
                        bases.push((base_began, now.load(Ordering::SeqCst)));
                    }
                }
            },
        }
    }
    // Everything shipped, a base ended if one was under way.
    tick(&mut rng);
    if let Some(mut c) = copy.take() {
        while shipper.copy_base(&db, &blobs, &mut c, until()).unwrap() {}
        shipper.end_base(&db, &blobs, c, until()).unwrap();
        first_base_end.get_or_insert(now.load(Ordering::SeqCst));
        bases.push((base_began, now.load(Ordering::SeqCst)));
    }
    tick(&mut rng);
    shipper.ship(&db, &blobs, until()).unwrap();
    let last = history.last().unwrap().0;
    let from = first_base_end
        .expect("a base was copied")
        .div_ceil(SECOND_NS);
    let to = last.div_ceil(SECOND_NS);
    assert!(to > from, "the drill spans seconds after its first base");
    let broken = |second: u64| {
        breaks.iter().any(|&(b, at)| {
            let mended = bases
                .iter()
                .filter(|(began, _)| *began > at)
                .map(|(_, ended)| ended.div_ceil(SECOND_NS))
                .min();
            second >= b.div_ceil(SECOND_NS) && mended.is_none_or(|m| second <= m)
        })
    };
    let mut restored = 0;
    for n in 0..12 {
        let second = from + rng.below(to - from + 1);
        let target = dir.path().join(format!("restored-{n}.db"));
        match backup::restore(&blobs, "db", second, &target, until()) {
            Ok(_) => {
                let db = Db::open(&target).unwrap();
                let expected = at(&history, second).cloned().unwrap_or_default();
                let got = rows(&db);
                let different = expected
                    .iter()
                    .filter(|(k, v)| got.get(k) != Some(v))
                    .count()
                    + got.keys().filter(|k| !expected.contains_key(k)).count();
                assert_eq!(
                    different, 0,
                    "seed {seed:#x}, second {second}: rows different"
                );
                restored += 1;
            }
            Err(e) if broken(second) => assert!(
                matches!(
                    e.downcast_ref::<Unrestorable>(),
                    Some(Unrestorable::NoBase(_) | Unrestorable::Inexact { .. })
                ),
                "seed {seed:#x}: second {second}, across a break, refused for another reason: {e:#}"
            ),
            Err(e) => panic!("seed {seed:#x}: second {second} refused: {e:#}"),
        }
    }
    assert!(restored > 0, "seed {seed:#x}: no second restored");
    // Before the archive's first commit and past what it holds, a second is refused.
    let before = backup::restore(&blobs, "db", 999, &dir.path().join("before.db"), until());
    assert!(matches!(
        before.unwrap_err().downcast_ref::<Unrestorable>(),
        Some(Unrestorable::BeforeArchive(_) | Unrestorable::NoBase(_))
    ));
    let after = backup::restore(
        &blobs,
        "db",
        to + 10_000,
        &dir.path().join("after.db"),
        until(),
    );
    assert!(matches!(
        after.unwrap_err().downcast_ref::<Unrestorable>(),
        Some(Unrestorable::NotYetArchived { .. })
    ));
}

/// Opens `path` backed up to `blobs` under "db", stamped by `now`.
fn open_backed_up<B: Blobs + 'static>(
    path: &std::path::Path,
    blobs: &Arc<B>,
    now: &Arc<AtomicU64>,
) -> (Database, backup::Shipping) {
    let read = Arc::clone(now);
    let archive = Archive {
        blobs: Arc::clone(blobs) as Arc<dyn Blobs>,
        prefix: "db".into(),
        clock: Arc::new(move || read.load(Ordering::SeqCst)),
        memory: 32 * 1024,
        patience: Duration::from_secs(60),
        chunks: backup::ChunkGrowth::Keep,
        check: backup::CheckPolicy::Off,
    };
    Database::open_backed_up(path, archive, until()).unwrap()
}

/// Writes rows `ids` in one commit each, a second apart, each one's rows kept in `history`.
fn write_seconds(
    database: &Database,
    now: &AtomicU64,
    ids: std::ops::Range<i64>,
    model: &mut Rows,
    history: &mut Vec<(u64, Rows)>,
) {
    for id in ids {
        now.fetch_add(SECOND_NS, Ordering::SeqCst);
        let v = format!(
            "{id}:{}",
            "y".repeat(usize::try_from(id).unwrap() * 97 % 3000)
        );
        let written = v.clone();
        database
            .call(until(), AnswerMax::fixed(), move |c| {
                c.execute(
                    "INSERT INTO t (id, v) VALUES (?1, ?2)",
                    vec![
                        amphora_api::Value::Integer(id),
                        amphora_api::Value::Text(written),
                    ],
                )?;
                Ok(())
            })
            .unwrap();
        model.insert(id, v);
        history.push((now.load(Ordering::SeqCst), model.clone()));
    }
}

/// T-T1.12.D2 (AC-T1.12.4): a database opened backed up ships as it commits, through its own
/// thread, across a close and a reopen. Do: open it backed up, write a table, wait for its first
/// base; write a row a second; close it; reopen it backed up and write a row a second more; close
/// it. Expect: every second after the first base restores with every row equal, those of the
/// reopened database's included, and no round failed.
#[test]
fn t_t1_12_d2_a_database_backed_up_ships_as_it_commits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.db");
    let blobs = Arc::new(SimBlobs::new(64 * 1024, 1_000_000, 1 << 30));
    let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS + 1));
    let mut model = Rows::new();
    let mut history: Vec<(u64, Rows)> = Vec::new();

    let (database, shipping) = open_backed_up(&path, &blobs, &now);
    database
        .call(until(), AnswerMax::fixed(), |c| {
            c.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")?;
            Ok(())
        })
        .unwrap();
    history.push((now.load(Ordering::SeqCst), model.clone()));
    shipping.wait(until(), |s| s.bases >= 1).unwrap();
    let based = now.load(Ordering::SeqCst).div_ceil(SECOND_NS);
    write_seconds(&database, &now, 1..20, &mut model, &mut history);
    // The close ships a second on: the archive is whole through the last commit's second.
    now.fetch_add(SECOND_NS, Ordering::SeqCst);
    drop(database);
    let first = shipping.wait(until(), |s| s.ended).unwrap();
    assert_eq!(first.failures, 0, "{:?}", first.last_failure);
    assert!(first.shipped.segments > 0);

    let (database, shipping) = open_backed_up(&path, &blobs, &now);
    write_seconds(&database, &now, 20..30, &mut model, &mut history);
    now.fetch_add(SECOND_NS, Ordering::SeqCst);
    drop(database);
    let second = shipping.wait(until(), |s| s.ended).unwrap();
    assert_eq!(second.failures, 0, "{:?}", second.last_failure);

    let last = history.last().unwrap().0.div_ceil(SECOND_NS);
    for second in based + 1..=last {
        let target = dir.path().join(format!("restored-{second}.db"));
        backup::restore(blobs.as_ref(), "db", second, &target, until())
            .unwrap_or_else(|e| panic!("second {second} refused: {e:#}"));
        let expected = at(&history, second).cloned().unwrap_or_default();
        assert_eq!(
            rows(&Db::open(&target).unwrap()),
            expected,
            "second {second}"
        );
    }
}

/// The digest of a database holding `rows` in table `t`: the source's rows at a second, as the
/// model kept them, in a database opened as the source was (Amphora's own tables beside them).
fn oracle_digest(dir: &std::path::Path, name: &str, rows: &Rows) -> backup::RowDigest {
    let path = dir.join(name);
    let mut sql = String::from("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL);");
    for (id, v) in rows {
        sql.push_str(&format!("INSERT INTO t (id, v) VALUES ({id}, '{v}');"));
    }
    let database = Database::open(&path, until()).unwrap();
    database
        .call(until(), AnswerMax::fixed(), move |c| {
            c.execute_batch(&sql)?;
            Ok(())
        })
        .unwrap();
    drop(database);
    backup::digest(&Db::open(&path).unwrap()).unwrap()
}

/// T-T1.12.C3 (AC-T1.12.4): the shipping thread checks its own archive. Do: back a database up
/// to a folder with a scratch path for checks, then write a row a second, each commit waking a
/// round. Expect: a check runs on its own and passes, against the source's own rows at a quiet
/// second; that second restores equal to the model's rows at it (so the snapshot the thread
/// digested held exactly that second's rows); the counters say one pass or more and no failure,
/// and the passed check's scratch file is gone.
#[test]
fn t_t1_12_c3_the_shipping_thread_checks_itself() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.db");
    let scratch = dir.path().join("check.db");
    let blobs =
        Arc::new(backup::FolderBlobs::open(&dir.path().join("archive"), 64 * 1024).unwrap());
    let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS + 1));
    let read = Arc::clone(&now);
    let archive = Archive {
        blobs: Arc::clone(&blobs) as Arc<dyn Blobs>,
        prefix: "db".into(),
        clock: Arc::new(move || read.load(Ordering::SeqCst)),
        memory: 32 * 1024,
        patience: Duration::from_secs(60),
        chunks: backup::ChunkGrowth::Keep,
        check: backup::CheckPolicy::Scratch(Some(scratch.clone())),
    };
    let (database, shipping) = Database::open_backed_up(&path, archive, until()).unwrap();
    database
        .call(until(), AnswerMax::fixed(), |c| {
            c.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")?;
            Ok(())
        })
        .unwrap();
    let mut model = Rows::new();
    let mut history: Vec<(u64, Rows)> = vec![(now.load(Ordering::SeqCst), model.clone())];
    shipping.wait(until(), |s| s.bases >= 1).unwrap();
    // A row a second until a check passed: a check needs a quiet second, then a round after it.
    let mut id = 1;
    let passed = loop {
        write_seconds(&database, &now, id..id + 1, &mut model, &mut history);
        id += 1;
        let status = shipping.status().unwrap();
        if let Some(backup::Check::Passed { second, .. }) = status.last_check {
            break second;
        }
        assert!(id < 400, "no check passed: {status:?}");
    };
    now.fetch_add(SECOND_NS, Ordering::SeqCst);
    drop(database);
    let status = shipping.wait(until(), |s| s.ended).unwrap();
    assert_eq!(status.failures, 0, "{:?}", status.last_failure);
    assert!(shipping.checks().passed.load(Ordering::Relaxed) >= 1);
    assert_eq!(shipping.checks().failed.load(Ordering::Relaxed), 0);
    assert_eq!(shipping.checks().source_moved.load(Ordering::Relaxed), 0);
    if matches!(status.last_check, Some(backup::Check::Passed { .. })) {
        assert!(!scratch.exists(), "a passed check's scratch file stays");
    }

    let again = dir.path().join("again.db");
    backup::restore(blobs.as_ref(), "db", passed, &again, until()).unwrap();
    let oracle_dir = tempfile::tempdir().unwrap();
    let want = at(&history, passed).cloned().unwrap_or_default();
    assert_eq!(
        backup::digest(&Db::open(&again).unwrap()).unwrap(),
        oracle_digest(oracle_dir.path(), "o.db", &want),
        "second {passed}"
    );
}

/// T-T1.12.C1 (AC-T1.12.4): restore-and-check on the folder store. Do: back a database up to a
/// folder (AMPHORA-BLOBS-8), write a row a second, close it; restore-and-check the newest second
/// against the model's rows, then against rows that differ. Expect: the newest second is the last
/// one written, the first check passes, the second fails naming the rows, and the counters say so.
#[test]
fn t_t1_12_c1_restore_and_check_on_a_folder() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.db");
    let blobs =
        Arc::new(backup::FolderBlobs::open(&dir.path().join("archive"), 64 * 1024).unwrap());
    let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS + 1));
    let mut model = Rows::new();
    let mut history: Vec<(u64, Rows)> = Vec::new();
    let (database, shipping) = open_backed_up(&path, &blobs, &now);
    database
        .call(until(), AnswerMax::fixed(), |c| {
            c.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")?;
            Ok(())
        })
        .unwrap();
    history.push((now.load(Ordering::SeqCst), model.clone()));
    shipping.wait(until(), |s| s.bases >= 1).unwrap();
    write_seconds(&database, &now, 1..12, &mut model, &mut history);
    now.fetch_add(SECOND_NS, Ordering::SeqCst);
    drop(database);
    let status = shipping.wait(until(), |s| s.ended).unwrap();
    assert_eq!(status.failures, 0, "{:?}", status.last_failure);

    let last = history.last().unwrap().0.div_ceil(SECOND_NS);
    let newest = backup::newest_second(blobs.as_ref(), "db", until())
        .unwrap()
        .unwrap();
    assert!(
        newest >= last,
        "newest {newest} before the last write's {last}"
    );
    let counters = backup::CheckCounters::default();
    let oracle_dir = tempfile::tempdir().unwrap();
    let source = |second: u64| -> anyhow::Result<Option<backup::RowDigest>> {
        let rows = at(&history, second).cloned().unwrap_or_default();
        Ok(Some(oracle_digest(
            oracle_dir.path(),
            &format!("o{second}.db"),
            &rows,
        )))
    };
    let check = backup::restore_and_check(
        blobs.as_ref(),
        "db",
        &dir.path().join("check-1.db"),
        &source,
        &counters,
        until(),
    )
    .unwrap();
    assert!(
        matches!(check, backup::Check::Passed { second, .. } if second == newest),
        "{check:?}"
    );

    let mut wrong = model.clone();
    wrong.insert(1, "changed".into());
    let wrong_source = |_second: u64| -> anyhow::Result<Option<backup::RowDigest>> {
        Ok(Some(oracle_digest(oracle_dir.path(), "wrong.db", &wrong)))
    };
    let check = backup::restore_and_check(
        blobs.as_ref(),
        "db",
        &dir.path().join("check-2.db"),
        &wrong_source,
        &counters,
        until(),
    )
    .unwrap();
    assert!(
        matches!(
            check,
            backup::Check::Failed {
                why: backup::CheckFailure::Rows { .. },
                ..
            }
        ),
        "{check:?}"
    );
    assert_eq!(counters.passed.load(Ordering::Relaxed), 1);
    assert_eq!(counters.failed.load(Ordering::Relaxed), 1);
}

/// T-T1.12.X1 (AC-T1.12.4): one cut across groups. Do: back two groups up to one folder under a
/// clock that moves a second at every reading, so every commit in either group has a second of its
/// own; run a cross-group transaction writing a key in each group, then a second one changing both;
/// then restore both groups to every second from their first bases to the end, each cut finishing
/// what it left half-done by the coordinator's records. Expect: at every cut the two keys are both
/// absent, both the first transaction's, or both the second's; never one without the other; never
/// back once forward; and the last cut holds the second's. Under both protocols.
#[test]
fn t_t1_12_x1_cross_group_cut_at_every_second() {
    for protocol in [
        amphora::xgroup::Protocol::TwoPhase,
        amphora::xgroup::Protocol::Parallel,
    ] {
        cross_group_cut(protocol, 0);
    }
}

/// T-T1.12.X2 (AC-T1.12.4): cuts across machines. Do: as T-T1.12.X1, but the two groups' clocks
/// differ as two nodes' do: the participant's reads 5 seconds ahead of the anchor's, so without a
/// carried clock its prepared write is stamped later than the anchor's decision. Expect: as
/// T-T1.12.X1 at every cut: never the decision kept and the participant's write lost.
#[test]
fn t_t1_12_x2_cross_group_cut_across_skewed_clocks() {
    for protocol in [
        amphora::xgroup::Protocol::TwoPhase,
        amphora::xgroup::Protocol::Parallel,
    ] {
        cross_group_cut(protocol, 5 * SECOND_NS);
    }
}

fn cross_group_cut(protocol: amphora::xgroup::Protocol, skew: u64) {
    use amphora::xgroup::{Answer, Cluster, Txn};
    let dir = tempfile::tempdir().unwrap();
    let blobs =
        Arc::new(backup::FolderBlobs::open(&dir.path().join("archive"), 64 * 1024).unwrap());
    let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS + 1));
    let open = |group: usize| {
        let read = Arc::clone(&now);
        let archive = Archive {
            blobs: Arc::clone(&blobs) as Arc<dyn Blobs>,
            prefix: format!("g{group}"),
            clock: Arc::new(move || {
                read.fetch_add(SECOND_NS, Ordering::SeqCst) + SECOND_NS + skew * group as u64
            }),
            memory: 32 * 1024,
            patience: Duration::from_secs(60),
            chunks: backup::ChunkGrowth::Keep,
            check: backup::CheckPolicy::Off,
        };
        Database::open_backed_up(&dir.path().join(format!("g{group}.db")), archive, until())
            .unwrap()
    };
    let route: amphora::xgroup::Route = Arc::new(|k: &[u8]| u32::from(k == b"b"));
    // The coordinator over both groups, backed up. A step past its learned timeout stops an
    // incarnation as a crash would (docs/design/xgroup.md); then the groups close, and open again
    // with the coordinator's next incarnation, which finishes what the last left.
    let start = || {
        let (g0, s0) = open(0);
        let (g1, s1) = open(1);
        let cluster =
            Cluster::open(vec![g0, g1], Arc::clone(&route), 7, protocol, 1, until()).unwrap();
        (cluster, vec![s0, s1])
    };
    let (mut cluster, mut shippings) = start();
    for shipping in &shippings {
        shipping.wait(until(), |s| s.bases >= 1).unwrap();
    }
    // Cuts begin once every group's clock is past its first base; the transactions come later
    // than that by the skew again, so the seconds between a decision and a skewed write are cut.
    let based = (now.load(Ordering::SeqCst) + skew).div_ceil(SECOND_NS);
    now.fetch_add(2 * skew, Ordering::SeqCst);
    let mut ended = Vec::new();
    for v in [b"1", b"2"] {
        let txn = Txn {
            reads: vec![],
            writes: vec![
                (b"a".to_vec(), Some(v.to_vec())),
                (b"b".to_vec(), Some(v.to_vec())),
            ],
            conds: Vec::new(),
        };
        // The first transaction's resolution may still hold the coordinator's one place. A stopped
        // coordinator is opened again and the transaction sent again: its writes are the same
        // values, so a second run changes nothing.
        let answer = loop {
            match cluster.transact(txn.clone(), until()) {
                Err(amphora_api::Refusal::Full { .. }) => std::thread::yield_now(),
                Err(amphora_api::Refusal::Unavailable(_)) => {
                    drop(cluster);
                    for shipping in &shippings {
                        shipping.wait(until(), |s| s.ended).unwrap();
                    }
                    ended.append(&mut shippings);
                    (cluster, shippings) = start();
                }
                other => break other.unwrap(),
            }
        };
        assert!(matches!(answer, Answer::Committed { .. }), "{answer:?}");
    }
    drop(cluster);
    ended.append(&mut shippings);
    for shipping in &ended {
        let status = shipping.wait(until(), |s| s.ended).unwrap();
        assert_eq!(status.failures, 0, "{:?}", status.last_failure);
    }
    let newest = [0, 1]
        .iter()
        .map(|g| {
            backup::newest_second(blobs.as_ref(), &format!("g{g}"), until())
                .unwrap()
                .unwrap()
        })
        .min()
        .unwrap();
    let mut seen: Vec<Option<Vec<u8>>> = Vec::new();
    for second in based + 1..=newest {
        let groups: Vec<(String, std::path::PathBuf)> = (0..2)
            .map(|g| {
                (
                    format!("g{g}"),
                    dir.path().join(format!("cut-{second}-g{g}.db")),
                )
            })
            .collect();
        backup::restore_cut(blobs.as_ref(), &groups, second, 7, protocol, until())
            .unwrap_or_else(|e| panic!("second {second}: {e:#}"));
        let restored: Vec<Database> = groups
            .iter()
            .map(|(_, p)| Database::open(p, until()).unwrap())
            .collect();
        let reader = Cluster::open(restored, Arc::clone(&route), 7, protocol, 1, until()).unwrap();
        let read = Txn {
            reads: vec![b"a".to_vec(), b"b".to_vec()],
            writes: vec![],
            conds: Vec::new(),
        };
        let Answer::Committed { reads, .. } = reader.transact(read, until()).unwrap() else {
            panic!("second {second}: the read didn't commit");
        };
        drop(reader);
        let (a, b) = (reads[0].1.clone(), reads[1].1.clone());
        assert_eq!(a, b, "second {second}: one group without the other");
        seen.push(a);
    }
    let order = |v: &Option<Vec<u8>>| match v.as_deref() {
        None => 0,
        Some(b"1") => 1,
        Some(_) => 2,
    };
    assert!(
        seen.windows(2).all(|w| order(&w[0]) <= order(&w[1])),
        "{seen:?}"
    );
    assert_eq!(seen.last().cloned().flatten(), Some(b"2".to_vec()));
    assert!(
        seen.iter().any(|v| v.as_deref() == Some(b"1")),
        "non-vacuous: a cut between the two"
    );
}

/// Writes rows `ids` with `tag`, one commit a second, each one's rows kept in `history`.
fn put(
    db: &mut Db,
    now: &AtomicU64,
    ids: impl Iterator<Item = i64>,
    tag: &str,
    model: &mut Rows,
    history: &mut Vec<(u64, Rows)>,
) {
    for id in ids {
        now.fetch_add(SECOND_NS, Ordering::SeqCst);
        let v = format!("{tag}{id}:{}", "z".repeat(500));
        let written = v.clone();
        db.transaction(|db| {
            db.query_owned(
                "INSERT OR REPLACE INTO t (id, v) VALUES (?1, ?2)",
                [Value::from_i64(id), Value::from_text(written.clone())],
                |_| Ok(true),
            )?;
            Ok(())
        })
        .unwrap();
        model.insert(id, v);
        history.push((now.load(Ordering::SeqCst), model.clone()));
    }
}

/// Decision 45 (lane B18): the backup ships from the segment the writer appends to. Do: write
/// 20 rows, ship them and copy a base, start the log again (the writer switches to `-wal2`), write
/// 20 more and ship, then restore the last second. Expect: the switch made `-wal2`, and the
/// restore holds the 20 rows written after it as well as the 20 before.
#[test]
fn d45_a_backup_after_a_switch_holds_the_writes_made_after_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.db");
    let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS));
    let read = Arc::clone(&now);
    let hooks = Hooks::new(Arc::new(move || read.load(Ordering::SeqCst)), 1 << 16);
    let mut db = Db::open(&path).unwrap();
    db.back_up(Arc::clone(&hooks));
    let blobs = backup::FolderBlobs::open(&dir.path().join("archive"), 64 * 1024).unwrap();
    db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")
        .unwrap();
    let mut model = Rows::new();
    let mut history: Vec<(u64, Rows)> = vec![(now.load(Ordering::SeqCst), model.clone())];
    let mut shipper = Shipper::open("db", Arc::clone(&hooks), &blobs, &db, until()).unwrap();
    put(&mut db, &now, 0..20, "a", &mut model, &mut history);
    shipper.ship(&db, &blobs, until()).unwrap();
    let mut base = shipper
        .begin_base(&db, &blobs, &amphora::memory::process(), 32 * 1024)
        .unwrap();
    while shipper.copy_base(&db, &blobs, &mut base, until()).unwrap() {}
    shipper.end_base(&db, &blobs, base, until()).unwrap();
    assert!(
        db.restart_log().unwrap(),
        "the log starts again once shipped"
    );
    let mut second = path.clone().into_os_string();
    second.push("-wal2");
    assert!(
        std::path::Path::new(&second).exists(),
        "the switch made the log's second segment"
    );
    put(&mut db, &now, 20..40, "b", &mut model, &mut history);
    now.fetch_add(SECOND_NS, Ordering::SeqCst);
    shipper.ship(&db, &blobs, until()).unwrap();
    shipper.ship(&db, &blobs, until()).unwrap();

    let last = history.last().unwrap().0.div_ceil(SECOND_NS);
    let target = dir.path().join("restored.db");
    backup::restore(&blobs, "db", last, &target, until())
        .unwrap_or_else(|e| panic!("second {last} refused: {e:#}"));
    let restored = rows(&Db::open(&target).unwrap());
    assert_eq!(restored.len(), 40, "every row, the 20 after the switch too");
    assert_eq!(restored, model);
}

/// T-T1.12.I1 (AC-T1.12.3, AC-T1.12.4): an incremental base. Do: write 400 rows, start the log
/// again and copy a base;
/// change 5 rows over two generations of the log, the shipper opened again between; copy a
/// second base; write on; restore seconds
/// across both; then let the first base expire and restore again. Expect: the second base keeps
/// most of the first's chunks and copies the changed ones; every second restored has every row
/// equal, from both bases; once the first base expires, the chunks the second keeps of it stay and
/// the newest second still restores equal.
#[test]
fn t_t1_12_i1_incremental_bases_keep_unchanged_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.db");
    let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS));
    let read = Arc::clone(&now);
    let hooks = Hooks::new(Arc::new(move || read.load(Ordering::SeqCst)), 1 << 16);
    let mut db = Db::open(&path).unwrap();
    db.back_up(Arc::clone(&hooks));
    let blobs = backup::FolderBlobs::open(&dir.path().join("archive"), 64 * 1024).unwrap();
    db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")
        .unwrap();
    let mut model = Rows::new();
    let mut history: Vec<(u64, Rows)> = vec![(now.load(Ordering::SeqCst), model.clone())];
    let mut shipper = Shipper::open("db", Arc::clone(&hooks), &blobs, &db, until()).unwrap();
    put(&mut db, &now, 0..400, "a", &mut model, &mut history);
    // The base begins just after the log starts again: a restore replays its generation whole, so
    // what that generation wrote before the base began counts as changed for the next.
    shipper.ship(&db, &blobs, until()).unwrap();
    assert!(db.restart_log().unwrap());
    put(&mut db, &now, 400..401, "c", &mut model, &mut history);
    shipper.ship(&db, &blobs, until()).unwrap();
    let copy_all = |shipper: &mut Shipper, db: &Db| {
        let mut copy = shipper
            .begin_base(db, &blobs, &amphora::memory::process(), 32 * 1024)
            .unwrap();
        while shipper.copy_base(db, &blobs, &mut copy, until()).unwrap() {}
        let reused = copy.reused();
        shipper.end_base(db, &blobs, copy, until()).unwrap();
        reused
    };
    assert_eq!(
        copy_all(&mut shipper, &db),
        0,
        "the first base copies everything"
    );
    let first_base = now.load(Ordering::SeqCst).div_ceil(SECOND_NS);

    // Five rows changed in one generation of the log, then the log started again.
    put(&mut db, &now, 0..5, "b", &mut model, &mut history);
    shipper.ship(&db, &blobs, until()).unwrap();
    // The shipper opens again on the archive: it reads back what the log changed since the base.
    shipper = Shipper::open("db", Arc::clone(&hooks), &blobs, &db, until()).unwrap();
    assert!(db.restart_log().unwrap());
    put(&mut db, &now, 401..402, "c", &mut model, &mut history);
    shipper.ship(&db, &blobs, until()).unwrap();
    let chunks = std::fs::metadata(&path).unwrap().len().div_ceil(32 * 1024) as usize;
    let reused = copy_all(&mut shipper, &db);
    assert!(
        reused > 0 && reused < chunks,
        "the second base keeps {reused} of {chunks} chunks"
    );
    let second_base = now.load(Ordering::SeqCst).div_ceil(SECOND_NS);
    put(&mut db, &now, 402..420, "d", &mut model, &mut history);
    now.fetch_add(SECOND_NS, Ordering::SeqCst);
    shipper.ship(&db, &blobs, until()).unwrap();
    shipper.ship(&db, &blobs, until()).unwrap();

    let last = history.last().unwrap().0.div_ceil(SECOND_NS);
    let check = |second: u64, name: &str| {
        let target = dir.path().join(name);
        let restored = backup::restore(&blobs, "db", second, &target, until())
            .unwrap_or_else(|e| panic!("second {second} refused: {e:#}"));
        let expected = at(&history, second).cloned().unwrap_or_default();
        assert_eq!(
            rows(&Db::open(&target).unwrap()),
            expected,
            "second {second}"
        );
        restored.base
    };
    let mut bases = std::collections::BTreeSet::new();
    let mut second = first_base + 1;
    while second <= last {
        bases.insert(check(second, &format!("r{second}.db")));
        second += 13;
    }
    bases.insert(check(last, "last.db"));
    assert!(bases.len() >= 2, "restores used both bases: {bases:?}");

    // The first base expires; what the second keeps of it stays.
    let now_ns = now.load(Ordering::SeqCst);
    let retention = now_ns - (second_base + 1) * SECOND_NS;
    let gone = backup::expire(&blobs, "db", now_ns, retention, until()).unwrap();
    assert!(
        gone > 0,
        "the first base and the log before the second expire"
    );
    check(last, "after-expiry.db");
    check(second_base + 1, "after-expiry-oldest.db");
}

/// T-T1.12.C4 (AC-T1.12.4): continuous restore drills are on by default. Do: back a database up to
/// a folder with the default policy, no scratch path given, then write a row a second until three
/// checks passed. Expect: the checks ran beside the database (`source.db.check`), passed with no
/// failure, the passed check's scratch file is gone, and the drills' costs were measured.
#[test]
fn t_t1_12_c4_continuous_drills_are_on_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.db");
    let blobs =
        Arc::new(backup::FolderBlobs::open(&dir.path().join("archive"), 64 * 1024).unwrap());
    let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS + 1));
    let read = Arc::clone(&now);
    let archive = Archive {
        blobs: Arc::clone(&blobs) as Arc<dyn Blobs>,
        prefix: "db".into(),
        clock: Arc::new(move || read.load(Ordering::SeqCst)),
        memory: 32 * 1024,
        patience: Duration::from_secs(60),
        chunks: backup::ChunkGrowth::Keep,
        check: backup::CheckPolicy::default(),
    };
    assert_eq!(archive.check, backup::CheckPolicy::Scratch(None));
    let (database, shipping) = Database::open_backed_up(&path, archive, until()).unwrap();
    database
        .call(until(), AnswerMax::fixed(), |c| {
            c.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")?;
            Ok(())
        })
        .unwrap();
    let mut model = Rows::new();
    let mut history: Vec<(u64, Rows)> = vec![(now.load(Ordering::SeqCst), model.clone())];
    shipping.wait(until(), |s| s.bases >= 1).unwrap();
    let mut id = 1;
    while shipping.checks().passed.load(Ordering::Relaxed) < 3 {
        write_seconds(&database, &now, id..id + 1, &mut model, &mut history);
        id += 1;
        assert!(
            id < 4_000,
            "three checks didn't pass: {:?}",
            shipping.status().unwrap()
        );
    }
    now.fetch_add(SECOND_NS, Ordering::SeqCst);
    drop(database);
    let status = shipping.wait(until(), |s| s.ended).unwrap();
    assert_eq!(status.failures, 0, "{:?}", status.last_failure);
    assert_eq!(shipping.checks().failed.load(Ordering::Relaxed), 0);
    assert!(shipping.drills().checks() >= 3);
    if matches!(status.last_check, Some(backup::Check::Passed { .. })) {
        assert!(!dir.path().join("source.db.check").exists());
    }
}

/// T-T1.12.D3 (AC-T1.12.4): a restore drill after a load. Do: back a database up to a folder
/// through its shipping thread, then load it from one writer per core at once, each commit a
/// second by the clock and each writer inserting, replacing and deleting rows of a shared range,
/// recording the rows after every commit; close it; then restore `AMPHORA_DRILL_SECONDS` seconds
/// drawn at random (16 by default, a tiny run) from the first base's to the last, each into a new
/// file. Expect: every row of every drilled second equal to the rows recorded at it.
#[test]
fn t_t1_12_d3_restore_drill_after_a_load() {
    let drills = std::env::var("AMPHORA_DRILL_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(16u64);
    let writers = std::thread::available_parallelism().map_or(1, std::num::NonZero::get) as u64;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.db");
    let blobs =
        Arc::new(backup::FolderBlobs::open(&dir.path().join("archive"), 64 * 1024).unwrap());
    let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS + 1));
    let (database, shipping) = open_backed_up(&path, &blobs, &now);
    database
        .call(until(), AnswerMax::fixed(), |c| {
            c.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")?;
            Ok(())
        })
        .unwrap();
    shipping.wait(until(), |s| s.bases >= 1).unwrap();
    let first = now.load(Ordering::SeqCst).div_ceil(SECOND_NS);
    // The model and the clock move under one lock with each commit, so the history records the
    // rows after every commit at the second it was stamped in.
    let state = std::sync::Mutex::new((Rows::new(), Vec::<(u64, Rows)>::new()));
    std::thread::scope(|scope| {
        for w in 0..writers {
            let database = &database;
            let now = &now;
            let state = &state;
            scope.spawn(move || {
                let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ (w + 1));
                for i in 0..40u64 {
                    let id = rng.below(64) as i64;
                    let delete = rng.below(4) == 0;
                    let v = format!("{w}:{i}:{}", "z".repeat(rng.below(3000) as usize));
                    let mut held = state.lock().unwrap();
                    now.fetch_add(1 + rng.below(SECOND_NS), Ordering::SeqCst);
                    let written = v.clone();
                    database
                        .call(until(), AnswerMax::fixed(), move |c| {
                            if delete {
                                c.execute(
                                    "DELETE FROM t WHERE id = ?1",
                                    vec![amphora_api::Value::Integer(id)],
                                )?;
                            } else {
                                c.execute(
                                    "INSERT OR REPLACE INTO t (id, v) VALUES (?1, ?2)",
                                    vec![
                                        amphora_api::Value::Integer(id),
                                        amphora_api::Value::Text(written),
                                    ],
                                )?;
                            }
                            Ok(())
                        })
                        .unwrap();
                    let (model, history) = &mut *held;
                    if delete {
                        model.remove(&id);
                    } else {
                        model.insert(id, v);
                    }
                    history.push((now.load(Ordering::SeqCst), model.clone()));
                }
            });
        }
    });
    let last = now.load(Ordering::SeqCst).div_ceil(SECOND_NS);
    now.fetch_add(SECOND_NS, Ordering::SeqCst);
    drop(database);
    let status = shipping.wait(until(), |s| s.ended).unwrap();
    assert_eq!(status.failures, 0, "{:?}", status.last_failure);
    let (_, history) = state.into_inner().unwrap();
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let empty = Rows::new();
    for n in 0..drills {
        let second = first + rng.below(last - first + 1);
        let target = dir.path().join(format!("drill-{n}.db"));
        backup::restore(blobs.as_ref(), "db", second, &target, until()).unwrap();
        let restored = Db::open(&target).unwrap();
        let want = at(&history, second).unwrap_or(&empty);
        assert_eq!(&rows(&restored), want, "second {second}");
    }
}

/// T-T1.12.C5, C7, C8 (AC-T1.12.4): checks competing with foreground load, the failure mode the
/// pace is for. Do: back a database up to a folder, then commit back to back from one writer per
/// core (`AMPHORA_DRILL_LOAD_WRITERS` to set another count; `AMPHORA_DRILL_LOAD_COMMITS` each, a
/// tiny run by default), each commit a second later,
/// under no checks (`off`, the floor), the control (`scratch`: unpaced checks whenever due),
/// `continuous` (paced, yielding fully to calls in flight, which one writer per core starves) and
/// `shared` (paced, with a minimum share over a retention period of `AMPHORA_DRILL_RETENTION_MS`,
/// the scenario's setting, 1,000 by default), `AMPHORA_DRILL_LOAD_RUNS` runs of each interleaved
/// (one by default; the spread across runs is what a policy's p99 is judged against); then, with
/// checks, a row a second until a check passes. Expect: no check failed, and checks pass once the
/// load ends. A write the database refuses `Full` under the load (its typed back-pressure) is
/// counted, not failed. It prints each run's instruments: commit latency p50/p99/p99.9, writes
/// refused, checks run and run under the load, deferrals, drills of older seconds, the pace's
/// pauses, waits and steps the share forced, check cost p50/p99/p99.9 and total.
#[test]
fn t_t1_12_c5_checks_competing_with_foreground_load() {
    let var = |name: &str, default: u64| -> u64 {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let commits = var("AMPHORA_DRILL_LOAD_COMMITS", 50);
    let runs = var("AMPHORA_DRILL_LOAD_RUNS", 1);
    let retention = Duration::from_millis(var("AMPHORA_DRILL_RETENTION_MS", 1_000));
    // One per core by default; `AMPHORA_DRILL_LOAD_WRITERS` walks the ladder (1 to 4,096), one
    // writer the laptop's extreme and thousands the fleet's.
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let writers = usize::try_from(var("AMPHORA_DRILL_LOAD_WRITERS", cores as u64))
        .unwrap()
        .max(1);
    for (run, policy) in
        (0..runs).flat_map(|run| ["off", "scratch", "continuous", "shared"].map(|p| (run, p)))
    {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.db");
        let blobs =
            Arc::new(backup::FolderBlobs::open(&dir.path().join("archive"), 64 * 1024).unwrap());
        let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS + 1));
        let read = Arc::clone(&now);
        let archive = Archive {
            blobs: Arc::clone(&blobs) as Arc<dyn Blobs>,
            prefix: "db".into(),
            clock: Arc::new(move || read.load(Ordering::SeqCst)),
            memory: 32 * 1024,
            patience: Duration::from_secs(60),
            chunks: backup::ChunkGrowth::Keep,
            check: match policy {
                "off" => backup::CheckPolicy::Off,
                "scratch" => backup::CheckPolicy::Scratch(Some(dir.path().join("scratch.db"))),
                "continuous" => backup::CheckPolicy::Continuous,
                _ => backup::CheckPolicy::Shared(retention),
            },
        };
        let (database, shipping) = Database::open_backed_up(&path, archive, until()).unwrap();
        database
            .call(until(), AnswerMax::fixed(), |c| {
                c.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")?;
                Ok(())
            })
            .unwrap();
        shipping.wait(until(), |s| s.bases >= 1).unwrap();
        // The load: every writer at once, a commit a second by the clock, as fast as they go.
        let refused = AtomicU64::new(0);
        let mut latencies: Vec<u64> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..writers)
                .map(|w| {
                    let database = &database;
                    let now = &now;
                    let refused = &refused;
                    scope.spawn(move || {
                        let mut took = Vec::new();
                        for i in 0..commits {
                            let id = i64::try_from(i * writers as u64 + w as u64 + 1).unwrap();
                            now.fetch_add(SECOND_NS, Ordering::SeqCst);
                            let began = Instant::now();
                            let wrote = database.call(until(), AnswerMax::fixed(), move |c| {
                                c.execute(
                                    "INSERT INTO t (id, v) VALUES (?1, ?2)",
                                    vec![
                                        amphora_api::Value::Integer(id),
                                        amphora_api::Value::Text("x".repeat(512)),
                                    ],
                                )?;
                                Ok(())
                            });
                            match wrote {
                                Ok(()) => {}
                                Err(amphora_api::Refusal::Full { .. }) => {
                                    refused.fetch_add(1, Ordering::Relaxed);
                                    continue;
                                }
                                Err(e) => panic!("{e:?}"),
                            }
                            took.push(u64::try_from(began.elapsed().as_nanos()).unwrap());
                        }
                        took
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap())
                .collect()
        });
        let under_load = shipping.drills().checks();
        // The load ends: a row a second until a check passes after it.
        let mut model = Rows::new();
        let mut history = Vec::new();
        let mut id = i64::try_from(commits * writers as u64 + 1).unwrap();
        let passed = shipping.checks().passed.load(Ordering::Relaxed);
        while policy != "off" && shipping.checks().passed.load(Ordering::Relaxed) == passed {
            write_seconds(&database, &now, id..id + 1, &mut model, &mut history);
            id += 1;
            assert!(id < 100_000, "no check passed after the load");
        }
        now.fetch_add(SECOND_NS, Ordering::SeqCst);
        drop(database);
        let status = shipping.wait(until(), |s| s.ended).unwrap();
        assert_eq!(status.failures, 0, "{:?}", status.last_failure);
        assert_eq!(shipping.checks().failed.load(Ordering::Relaxed), 0);
        let meter = shipping.drills();
        let cost = meter.cost_ns.load(Ordering::Relaxed);
        latencies.sort_unstable();
        let q = |p: f64| {
            let i = ((p * latencies.len() as f64).ceil() as usize).saturating_sub(1);
            latencies.get(i).copied().unwrap_or(0)
        };
        println!(
            "drill-load run={run} policy={policy} writers={writers} commits={} refused={} commit_ns p50={} p99={} p99.9={} checks={} under_load={under_load} passed={} deferred disk={} older={} paused={} waited_ns={} forced={} cost_ns p50={:?} p99={:?} p99.9={:?} cost_total_ns={cost}",
            latencies.len(),
            refused.load(Ordering::Relaxed),
            q(0.5),
            q(0.99),
            q(0.999),
            meter.checks(),
            shipping.checks().passed.load(Ordering::Relaxed),
            meter.deferred_disk.load(Ordering::Relaxed),
            meter.older.load(Ordering::Relaxed),
            meter.paused.load(Ordering::Relaxed),
            meter.waited_ns.load(Ordering::Relaxed),
            meter.forced.load(Ordering::Relaxed),
            meter.cost_quantile(1, 2),
            meter.cost_quantile(99, 100),
            meter.cost_quantile(999, 1000),
        );
    }
}

/// What a restore's pace saw: each step's end, whether it wrote and the bytes it moved.
struct Recording {
    paced: bool,
    steps: Vec<(bool, u64)>,
}

impl backup::Pace for Recording {
    fn ready(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn done(&mut self, file: Option<&std::fs::File>, bytes: u64) -> anyhow::Result<()> {
        self.steps.push((file.is_some(), bytes));
        Ok(())
    }

    fn paced(&self) -> bool {
        self.paced
    }
}

/// T-T1.12.C8 (AC-T1.12.4): a paced check pauses inside its replay and its integrity check. Do:
/// back a database up to a folder whose objects hold 64 KiB, write a row a second, close it;
/// restore its last second unpaced and paced, then restore-and-check it paced. Expect: both
/// restores hold the model's rows; the paced one writes in more steps, its replay folded in parts
/// of an object's worth of frames; the check passes, its integrity check ending steps inside the
/// statement (a step with no bytes for each wait on the device, beyond the restore's and the
/// integrity check's own last).
#[test]
fn t_t1_12_c8_replay_and_integrity_check_pause_in_steps() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.db");
    let blobs =
        Arc::new(backup::FolderBlobs::open(&dir.path().join("archive"), 64 * 1024).unwrap());
    let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS + 1));
    let mut model = Rows::new();
    let mut history: Vec<(u64, Rows)> = Vec::new();
    let (database, shipping) = open_backed_up(&path, &blobs, &now);
    database
        .call(until(), AnswerMax::fixed(), |c| {
            c.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")?;
            Ok(())
        })
        .unwrap();
    shipping.wait(until(), |s| s.bases >= 1).unwrap();
    write_seconds(&database, &now, 1..120, &mut model, &mut history);
    now.fetch_add(SECOND_NS, Ordering::SeqCst);
    drop(database);
    let status = shipping.wait(until(), |s| s.ended).unwrap();
    assert_eq!(status.failures, 0, "{:?}", status.last_failure);
    let second = history.last().unwrap().0.div_ceil(SECOND_NS);

    let mut unpaced = Recording {
        paced: false,
        steps: Vec::new(),
    };
    let mut paced = Recording {
        paced: true,
        steps: Vec::new(),
    };
    for (name, pace) in [("unpaced.db", &mut unpaced), ("paced.db", &mut paced)] {
        let target = dir.path().join(name);
        backup::restore_paced(blobs.as_ref(), "db", second, &target, pace, until()).unwrap();
        assert_eq!(rows(&Db::open(&target).unwrap()), model, "{name}");
    }
    let writes = |r: &Recording| r.steps.iter().filter(|(wrote, _)| *wrote).count();
    assert!(
        writes(&paced) > writes(&unpaced),
        "the paced replay folds in parts: {} writing steps paced, {} unpaced",
        writes(&paced),
        writes(&unpaced)
    );

    let mut checked = Recording {
        paced: true,
        steps: Vec::new(),
    };
    let oracle_dir = tempfile::tempdir().unwrap();
    let want = oracle_digest(oracle_dir.path(), "o.db", &model);
    let source = |_: u64| -> anyhow::Result<Option<backup::RowDigest>> { Ok(Some(want.clone())) };
    let check = backup::restore_and_check_at(
        blobs.as_ref(),
        "db",
        second,
        &dir.path().join("check.db"),
        &source,
        &backup::CheckCounters::default(),
        &mut checked,
        until(),
    )
    .unwrap();
    assert!(matches!(check, backup::Check::Passed { .. }), "{check:?}");
    // The restore's steps with no bytes are its replays' last folds; the check adds its integrity
    // check's last, and one for each wait on the device inside the statement.
    let empty = |r: &Recording| r.steps.iter().filter(|s| **s == (false, 0)).count();
    assert!(
        empty(&checked) > empty(&paced) + 1,
        "the integrity check ended no step inside its statement: {} empty steps, the restore's {}",
        empty(&checked),
        empty(&paced)
    );
}

/// The restore benchmark's Amphora leg (AC-T1.12.3; bench/backup_restore.sh runs it beside
/// PostgreSQL with pgBackRest and SQLite with Litestream on the same machine, each leg timed by the
/// same clock around its own restore command). Its phases, `AMPHORA_RESTORE_BENCH_PHASE`, each a
/// process of its own so the restore's time includes its process start as the other legs' do:
/// - `backup`: back a database up to a folder (`AMPHORA_RESTORE_BENCH_DIR`), write
///   `AMPHORA_RESTORE_BENCH_BYTES` of 4096-byte rows (a page's worth, the engine's page size: a
///   format value), a commit of `AMPHORA_RESTORE_BENCH_BATCH` rows each second, and close it;
///   prints `backup source_bytes=` (the file and its log, as the other legs count theirs).
/// - `restore`: restore the archive's newest second into `restored.db`, as a restore to the
///   latest does: it finds the second itself.
/// - `count`: count the restored rows with the database's own client; prints `count rows=`.
///
/// With no phase, all three in one process (a tiny instance; a temporary folder if no folder is
/// given), printing the restore's nanoseconds. The sizes are the caller's; the defaults are a tiny
/// instance. A chunk takes its stream's share of the process's room for chunks (`amphora::memory`'s
/// budget, read where it runs; T-T1.12.M1), and the folder store's objects are as large as the
/// process's room, a get reading one whole; every other bound is the store's `object_max()`. Each phase
/// gives up past the goal's bar, 30 minutes (AC-T1.12.3), as a restore past it has failed it.
#[test]
#[ignore = "a benchmark: run by bench/backup_restore.sh"]
fn restore_bench_amphora() {
    let var = |name: &str, default: u64| -> u64 {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let phase = std::env::var("AMPHORA_RESTORE_BENCH_PHASE").ok();
    let temp = tempfile::tempdir().unwrap();
    let root = match (std::env::var("AMPHORA_RESTORE_BENCH_DIR"), &phase) {
        (Ok(dir), _) => std::path::PathBuf::from(dir),
        (Err(_), None) => temp.path().to_path_buf(),
        (Err(_), Some(phase)) => {
            panic!("phase {phase} needs AMPHORA_RESTORE_BENCH_DIR: the phases share its folder")
        }
    };
    std::fs::create_dir_all(&root).unwrap();
    // The goal's bar (AC-T1.12.3: 100 GB in at most 30 minutes): a phase past it has failed.
    let bar = || Instant::now() + Duration::from_secs(30 * 60);
    let room = amphora::memory::process().image_room().unwrap();
    let blobs = Arc::new(backup::FolderBlobs::open(&root.join("archive"), room).unwrap());
    let path = root.join("source.db");
    let target = root.join("restored.db");
    let run = |name: &str| phase.as_deref().is_none_or(|p| p == name);
    assert!(
        phase
            .as_deref()
            .is_none_or(|p| ["backup", "restore", "count"].contains(&p)),
        "AMPHORA_RESTORE_BENCH_PHASE is backup, restore or count, not {phase:?}"
    );

    if run("backup") {
        let bytes = var("AMPHORA_RESTORE_BENCH_BYTES", 4 << 20);
        let batch = var("AMPHORA_RESTORE_BENCH_BATCH", 256).max(1);
        let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS + 1));
        let read = Arc::clone(&now);
        let archive = Archive {
            blobs: Arc::clone(&blobs) as Arc<dyn Blobs>,
            prefix: "db".into(),
            clock: Arc::new(move || read.load(Ordering::SeqCst)),
            memory: room,
            patience: Duration::from_secs(30 * 60),
            chunks: backup::ChunkGrowth::Keep,
            check: backup::CheckPolicy::Off,
        };
        let (database, shipping) = Database::open_backed_up(&path, archive, bar()).unwrap();
        database
            .call(bar(), AnswerMax::fixed(), |c| {
                c.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")?;
                Ok(())
            })
            .unwrap();
        let rows = bytes.div_ceil(4096);
        let mut id = 0u64;
        while id < rows {
            let end = (id + batch).min(rows);
            now.fetch_add(SECOND_NS, Ordering::SeqCst);
            let first = id;
            database
                .call(bar(), AnswerMax::fixed(), move |c| {
                    for i in first..end {
                        c.execute(
                            "INSERT INTO t (id, v) VALUES (?1, ?2)",
                            vec![
                                amphora_api::Value::Integer(i64::try_from(i).unwrap()),
                                amphora_api::Value::Text("x".repeat(4096)),
                            ],
                        )?;
                    }
                    Ok(())
                })
                .unwrap();
            id = end;
        }
        now.fetch_add(SECOND_NS, Ordering::SeqCst);
        drop(database);
        let status = shipping.wait(bar(), |s| s.ended).unwrap();
        assert_eq!(status.failures, 0, "{:?}", status.last_failure);
        let size = |p: &std::path::Path| std::fs::metadata(p).map_or(0, |m| m.len());
        let mut wal = path.as_os_str().to_owned();
        wal.push("-wal");
        let source = size(&path) + size(std::path::Path::new(&wal));
        println!("backup source_bytes={source}");
    }

    if run("restore") {
        let began = Instant::now();
        let newest = backup::newest_second(blobs.as_ref(), "db", bar())
            .unwrap()
            .expect("the archive holds no second to restore");
        backup::restore(blobs.as_ref(), "db", newest, &target, bar()).unwrap();
        println!(
            "restore system=amphora second={newest} ns={}",
            began.elapsed().as_nanos()
        );
    }

    if run("count") {
        let restored = Db::open(&target).unwrap();
        let mut rows = 0i64;
        restored
            .query("SELECT count(*) FROM t", [], |r| {
                rows = r.get::<i64>(0).map_err(|e| anyhow::anyhow!("{e}"))?;
                Ok(false)
            })
            .unwrap();
        println!("count rows={rows}");
    }
}

/// Readings a test sets: a memory limit and nothing resident, so a use's room is the limit less
/// what the budget's charges hold.
struct Room(u64);

impl amphora::memory::Readings for Room {
    fn read(&self) -> amphora::memory::Reading {
        amphora::memory::Reading {
            limit: Some(self.0),
            resident: Some(0),
        }
    }
}

fn room(limit: u64) -> Arc<amphora::memory::Budget> {
    amphora::memory::Budget::new(Box::new(Room(limit)))
}

/// A database in `dir` of `count` rows of 4,000 bytes, written in one commit, its log folded into
/// the file and started again, then one row more, shipped to `blobs`: its rows, the clock and the
/// second of its last commit.
fn written(
    dir: &std::path::Path,
    count: i64,
    blobs: &dyn Blobs,
) -> (Db, Shipper, Rows, Arc<AtomicU64>, u64) {
    written_within(dir, count, blobs, &amphora::memory::process())
}

/// [`written`], its shipper's parts charged to `memory`.
fn written_within(
    dir: &std::path::Path,
    count: i64,
    blobs: &dyn Blobs,
    memory: &Arc<amphora::memory::Budget>,
) -> (Db, Shipper, Rows, Arc<AtomicU64>, u64) {
    let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS));
    let read = Arc::clone(&now);
    let hooks = Hooks::new(Arc::new(move || read.load(Ordering::SeqCst)), 1 << 16);
    let mut db = Db::open(&dir.join("source.db")).unwrap();
    db.back_up(Arc::clone(&hooks));
    db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")
        .unwrap();
    let mut shipper =
        Shipper::open_within("db", Arc::clone(&hooks), blobs, &db, memory, until()).unwrap();
    let mut model = Rows::new();
    now.fetch_add(SECOND_NS, Ordering::SeqCst);
    db.transaction(|db| {
        for id in 0..count {
            let v = format!("{id}:{}", "z".repeat(4000));
            db.query_owned(
                "INSERT INTO t (id, v) VALUES (?1, ?2)",
                [Value::from_i64(id), Value::from_text(v.clone())],
                |_| Ok(true),
            )?;
            model.insert(id, v);
        }
        Ok(())
    })
    .unwrap();
    // The log folded into the file and started again (as in T-T1.12.I1), so the base holds the
    // rows; a row after, so the shipper has seen the new generation.
    shipper.ship(&db, blobs, until()).unwrap();
    assert!(db.restart_log().unwrap());
    now.fetch_add(SECOND_NS, Ordering::SeqCst);
    let v = "after".to_owned();
    db.transaction(|db| {
        db.query_owned(
            "INSERT INTO t (id, v) VALUES (?1, ?2)",
            [Value::from_i64(count), Value::from_text(v.clone())],
            |_| Ok(true),
        )?;
        Ok(())
    })
    .unwrap();
    model.insert(count, v);
    let second = now.load(Ordering::SeqCst).div_ceil(SECOND_NS);
    shipper.ship(&db, blobs, until()).unwrap();
    (db, shipper, model, now, second)
}

/// The newest base under "db": the latest to end, as a restore orders them.
fn newest_base(blobs: &dyn Blobs) -> backup::plan::BaseKey {
    blobs
        .list("db/base/", None, 100_000, until())
        .unwrap()
        .objects
        .iter()
        .map(|(key, _)| backup::plan::BaseKey::parse("db", key).unwrap())
        .max_by_key(|b| (b.end, b.id))
        .unwrap()
}

/// Copies a base whole under `memory`: its chunks' size and count.
fn copy_whole(
    shipper: &mut Shipper,
    db: &Db,
    blobs: &dyn Blobs,
    memory: &Arc<amphora::memory::Budget>,
) -> (u64, u64) {
    let mut copy = shipper.begin_base(db, blobs, memory, usize::MAX).unwrap();
    let chunk = copy.chunk_bytes();
    let mut chunks = 0;
    loop {
        chunks += 1;
        let more = shipper.copy_base(db, blobs, &mut copy, until()).unwrap();
        assert_eq!(
            memory.held(Use::Chunks),
            0,
            "a chunk's charge is given back once it's put"
        );
        if !more {
            break;
        }
    }
    shipper.end_base(db, blobs, copy, until()).unwrap();
    (chunk, chunks)
}

/// Archives the second of the last commit: a second later, shipped again.
fn archive_through(shipper: &mut Shipper, db: &Db, blobs: &dyn Blobs, now: &AtomicU64) {
    now.fetch_add(SECOND_NS, Ordering::SeqCst);
    shipper.ship(db, blobs, until()).unwrap();
    shipper.ship(db, blobs, until()).unwrap();
}

/// T-T1.12.M1: a file far larger than the memory room. Do: write about 4 MB, copy a base under a
/// 64 KiB budget, another use taking the room once mid-copy; restore under the same budget. Expect:
/// each chunk, read and sealed, fits the room (so the first chunk is no larger than the rest), the
/// chunk taken while the room is gone refused typed and copied once it's back, every charge given
/// back, and the restore whole.
#[test]
fn t_t1_12_m1_a_file_far_larger_than_the_room() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = SimBlobs::new(1 << 30, 1 << 20, 1 << 30);
    let (db, mut shipper, model, now, second) = written(dir.path(), 1000, &blobs);
    let limit = 64 * 1024u64;
    let memory = room(limit);
    let file = std::fs::metadata(dir.path().join("source.db"))
        .unwrap()
        .len();
    assert!(
        file > 32 * limit,
        "the file ({file} bytes) is far past the room"
    );

    let mut copy = shipper
        .begin_base(&db, &blobs, &memory, usize::MAX)
        .unwrap();
    let chunk = copy.chunk_bytes();
    assert!(
        chunk > 0 && 2 * chunk + 32 <= limit,
        "a chunk of {chunk} bytes, read and sealed, fits {limit}"
    );
    assert_eq!(memory.streams(Use::Chunks), 1, "the copy's stream");
    let taken = memory.take(Use::Images, limit - chunk).unwrap();
    let refused = shipper
        .copy_base(&db, &blobs, &mut copy, until())
        .unwrap_err();
    assert!(
        matches!(
            refused.downcast_ref::<amphora_api::Refusal>(),
            Some(amphora_api::Refusal::TooLarge { .. })
        ),
        "{refused:#}"
    );
    drop(taken);
    let mut chunks = 0u64;
    loop {
        chunks += 1;
        let more = shipper.copy_base(&db, &blobs, &mut copy, until()).unwrap();
        assert_eq!(memory.held(Use::Chunks), 0);
        if !more {
            break;
        }
    }
    assert_eq!(chunks, file.div_ceil(chunk), "every chunk copied once");
    shipper.end_base(&db, &blobs, copy, until()).unwrap();
    assert_eq!(memory.streams(Use::Chunks), 0, "the copy's stream closed");
    archive_through(&mut shipper, &db, &blobs, &now);

    let target = dir.path().join("restored.db");
    backup::restore_within(
        &blobs,
        "db",
        second,
        &target,
        &mut backup::Unpaced,
        &memory,
        until(),
    )
    .unwrap();
    assert_eq!(rows(&Db::open(&target).unwrap()), model);
    assert_eq!(memory.held(Use::Chunks), 0);
    assert_eq!(memory.streams(Use::Chunks), 0);
}

/// T-T1.12.M2: a backup under a large room restored under a small one. Do: copy a base under a
/// 1 GiB budget, one chunk holding the whole file; restore it under 64 KiB, under a page, and under
/// no room; then alter a byte of the chunk in the store and restore again. Expect: the small
/// restores whole, the chunk fetched in parts that fit their room, every charge given back; no
/// room refused, typed, before anything is written; the altered chunk refused by its sum, though
/// read in parts.
#[test]
fn t_t1_12_m2_a_large_rooms_backup_restored_in_a_small_room() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = SimBlobs::new(1 << 30, 1 << 20, 1 << 30);
    let (db, mut shipper, model, now, second) = written(dir.path(), 100, &blobs);
    let (chunk, chunks) = copy_whole(&mut shipper, &db, &blobs, &room(1 << 30));
    let file = std::fs::metadata(dir.path().join("source.db"))
        .unwrap()
        .len();
    assert_eq!(chunks, 1, "the whole file fits one chunk of the large room");
    assert!(chunk >= file && file > 64 * 1024);
    let copied = newest_base(&blobs);
    archive_through(&mut shipper, &db, &blobs, &now);

    let mut base = None;
    for (name, limit) in [("small.db", 64 * 1024), ("page.db", 4096)] {
        let memory = room(limit);
        let target = dir.path().join(name);
        let restored = backup::restore_within(
            &blobs,
            "db",
            second,
            &target,
            &mut backup::Unpaced,
            &memory,
            until(),
        )
        .unwrap_or_else(|e| panic!("restore under {limit} bytes: {e:#}"));
        assert_eq!(rows(&Db::open(&target).unwrap()), model, "under {limit}");
        assert_eq!(memory.held(Use::Chunks), 0);
        assert_eq!(memory.streams(Use::Chunks), 0);
        // The rule (`plan::chain`): the latest base that ended at or before the cut. This one was
        // copied after the second's last commit with nothing committed since, so it ended at the
        // cut's own frame and is the one taken.
        assert_eq!(copied.end, restored.cut, "the base ended at the cut");
        assert_eq!(restored.base, copied.id, "under {limit}");
        base = Some(restored.base);
    }

    let none = dir.path().join("none.db");
    let refused = backup::restore_within(
        &blobs,
        "db",
        second,
        &none,
        &mut backup::Unpaced,
        &room(0),
        until(),
    )
    .unwrap_err();
    assert!(
        matches!(
            refused.downcast_ref::<Unrestorable>(),
            Some(Unrestorable::PastRoom { .. } | Unrestorable::SegmentPastRoom { .. })
        ),
        "{refused:#}"
    );
    assert!(!none.exists(), "nothing written");

    let key = backup::plan::chunk_key("db", base.unwrap(), 0);
    let mut altered = blobs.get(&key, until()).unwrap();
    *altered.last_mut().unwrap() ^= 1;
    blobs.delete(&key, until()).unwrap();
    blobs.create(&key, &altered, until()).unwrap();
    let refused = backup::restore_within(
        &blobs,
        "db",
        second,
        &dir.path().join("altered.db"),
        &mut backup::Unpaced,
        &room(4096),
        until(),
    )
    .unwrap_err();
    assert!(
        format!("{refused:#}").contains("doesn't match its sum"),
        "{refused:#}"
    );
}

/// `count` rows of 4,000 bytes from `from`, in one commit a second later.
fn one_commit(db: &mut Db, now: &AtomicU64, from: i64, count: i64, model: &mut Rows) {
    now.fetch_add(SECOND_NS, Ordering::SeqCst);
    db.transaction(|db| {
        for id in from..from + count {
            let v = format!("{id}:{}", "y".repeat(4000));
            db.query_owned(
                "INSERT INTO t (id, v) VALUES (?1, ?2)",
                [Value::from_i64(id), Value::from_text(v.clone())],
                |_| Ok(true),
            )?;
            model.insert(id, v);
        }
        Ok(())
    })
    .unwrap();
}

/// A store that says how it was read: the largest answer a get of a log segment gave, and the
/// gets of one whole.
struct Reads<'a> {
    inner: &'a dyn Blobs,
    largest: AtomicU64,
    wholes: AtomicU64,
    /// Ranged gets of a base's chunks.
    chunk_gets: AtomicU64,
}

impl<'a> Reads<'a> {
    fn new(inner: &'a dyn Blobs) -> Self {
        Self {
            inner,
            largest: AtomicU64::new(0),
            wholes: AtomicU64::new(0),
            chunk_gets: AtomicU64::new(0),
        }
    }
}

impl Blobs for Reads<'_> {
    fn object_max(&self) -> usize {
        self.inner.object_max()
    }
    fn create(
        &self,
        key: &str,
        bytes: &[u8],
        until: Instant,
    ) -> Result<(), amphora_api::blobs::BlobRefusal> {
        self.inner.create(key, bytes, until)
    }
    fn get(&self, key: &str, until: Instant) -> Result<Vec<u8>, amphora_api::blobs::BlobRefusal> {
        if key.contains("/log/") {
            self.wholes.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.get(key, until)
    }
    fn get_range(
        &self,
        key: &str,
        offset: u64,
        len: usize,
        until: Instant,
    ) -> Result<Vec<u8>, amphora_api::blobs::BlobRefusal> {
        let got = self.inner.get_range(key, offset, len, until)?;
        if key.contains("/pages/") {
            self.chunk_gets.fetch_add(1, Ordering::SeqCst);
        }
        if key.contains("/log/") {
            self.largest.fetch_max(got.len() as u64, Ordering::SeqCst);
        }
        Ok(got)
    }
    fn list(
        &self,
        prefix: &str,
        after: Option<&str>,
        limit: usize,
        until: Instant,
    ) -> Result<amphora_api::blobs::Listing, amphora_api::blobs::BlobRefusal> {
        self.inner.list(prefix, after, limit, until)
    }
    fn delete(&self, key: &str, until: Instant) -> Result<(), amphora_api::blobs::BlobRefusal> {
        self.inner.delete(key, until)
    }
}

/// The largest log segment under "db" in `blobs`.
fn largest_segment(blobs: &dyn Blobs) -> u64 {
    blobs
        .list("db/log/", None, 1000, until())
        .unwrap()
        .objects
        .iter()
        .map(|(_, size)| *size)
        .max()
        .unwrap_or(0)
}

/// T-T1.12.M3: large log segments restored under a far smaller room. Do: copy a base, then ship
/// about 2 MB of log in one commit to a store whose objects hold it, so it's one segment; restore
/// under 64 KiB and under 8 KiB. Expect: the restore whole, each segment fetched in ranged parts of
/// half the room at most and never whole, every charge given back.
#[test]
fn t_t1_12_m3_large_segments_restored_in_a_small_room() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = SimBlobs::new(1 << 30, 1 << 20, 1 << 30);
    let (mut db, mut shipper, mut model, now, _) =
        written_within(dir.path(), 10, &blobs, &room(1 << 30));
    copy_whole(&mut shipper, &db, &blobs, &room(1 << 30));
    one_commit(&mut db, &now, 100, 500, &mut model);
    shipper.ship(&db, &blobs, until()).unwrap();
    let second = now.load(Ordering::SeqCst).div_ceil(SECOND_NS);
    archive_through(&mut shipper, &db, &blobs, &now);
    let largest = largest_segment(&blobs);
    for (name, limit) in [("small.db", 64 * 1024u64), ("smaller.db", 8 * 1024)] {
        assert!(
            largest > 16 * limit,
            "a segment of {largest} bytes, the room {limit}"
        );
        let memory = room(limit);
        let reads = Reads::new(&blobs);
        let target = dir.path().join(name);
        backup::restore_within(
            &reads,
            "db",
            second,
            &target,
            &mut backup::Unpaced,
            &memory,
            until(),
        )
        .unwrap_or_else(|e| panic!("restore under {limit} bytes: {e:#}"));
        assert_eq!(rows(&Db::open(&target).unwrap()), model, "under {limit}");
        let part = reads.largest.load(Ordering::SeqCst);
        assert!(
            part > 0 && part <= limit / 2,
            "parts of {part} bytes under {limit}"
        );
        assert_eq!(
            reads.wholes.load(Ordering::SeqCst),
            0,
            "no segment read whole"
        );
        assert_eq!(memory.held(Use::Chunks), 0);
        assert_eq!(memory.streams(Use::Chunks), 0);
    }
}

/// T-T1.12.M3: shipping's parts are charged and bounded by the room. Do: ship under a 64 KiB
/// budget, then about 2 MB in one commit; take the room away and ship; give it back and ship again;
/// restore. Expect: every segment, sealed, within half the room; no charge held between rounds;
/// the round without room refused typed, its commits shipped by the next and the restore whole.
#[test]
fn t_t1_12_m3_shipping_charges_its_parts() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = SimBlobs::new(1 << 30, 1 << 20, 1 << 30);
    let limit = 64 * 1024u64;
    let memory = room(limit);
    let (mut db, mut shipper, mut model, now, _) = written_within(dir.path(), 10, &blobs, &memory);
    copy_whole(&mut shipper, &db, &blobs, &room(1 << 30));
    one_commit(&mut db, &now, 100, 500, &mut model);
    let taken = memory.take(Use::Images, limit - 1024).unwrap();
    let refused = shipper.ship(&db, &blobs, until()).unwrap_err();
    assert!(
        matches!(
            refused.downcast_ref::<amphora_api::Refusal>(),
            Some(amphora_api::Refusal::TooLarge { .. })
        ),
        "{refused:#}"
    );
    drop(taken);
    assert_eq!(memory.held(Use::Chunks), 0);
    let shipped = shipper.ship(&db, &blobs, until()).unwrap();
    assert!(shipped.segments > 16, "{} segments", shipped.segments);
    assert_eq!(memory.held(Use::Chunks), 0);
    assert_eq!(memory.streams(Use::Chunks), 0);
    let largest = largest_segment(&blobs);
    assert!(
        largest <= limit / 2,
        "a segment of {largest} bytes under {limit}"
    );
    let second = now.load(Ordering::SeqCst).div_ceil(SECOND_NS);
    archive_through(&mut shipper, &db, &blobs, &now);
    let target = dir.path().join("restored.db");
    backup::restore_within(
        &blobs,
        "db",
        second,
        &target,
        &mut backup::Unpaced,
        &room(1 << 30),
        until(),
    )
    .unwrap();
    assert_eq!(rows(&Db::open(&target).unwrap()), model);
}

/// What a second base cost under a policy for its chunks' size (T-T1.12.M5).
#[derive(Debug)]
struct Grown {
    first_chunk: u64,
    chunk: u64,
    chunks: u64,
    reused: u64,
    file: u64,
    restore_gets: u64,
}

/// A base copied under a 64 KiB room, two rows written over two generations of the log, then a
/// second base under a 1 GiB room with `growth`, and a restore under that room.
fn grown(growth: backup::ChunkGrowth) -> Grown {
    let dir = tempfile::tempdir().unwrap();
    let blobs = SimBlobs::new(1 << 30, 1 << 20, 1 << 30);
    let (mut db, mut shipper, mut model, now, _) =
        written_within(dir.path(), 300, &blobs, &room(1 << 30));
    shipper.set_chunk_growth(growth);
    let (first_chunk, _) = copy_whole(&mut shipper, &db, &blobs, &room(64 * 1024));
    one_commit(&mut db, &now, 1000, 1, &mut model);
    shipper.ship(&db, &blobs, until()).unwrap();
    assert!(db.restart_log().unwrap());
    one_commit(&mut db, &now, 1001, 1, &mut model);
    shipper.ship(&db, &blobs, until()).unwrap();

    let large = room(1 << 30);
    let mut copy = shipper.begin_base(&db, &blobs, &large, usize::MAX).unwrap();
    let chunk = copy.chunk_bytes();
    let mut chunks = 0u64;
    loop {
        chunks += 1;
        if !shipper.copy_base(&db, &blobs, &mut copy, until()).unwrap() {
            break;
        }
    }
    let reused = copy.reused() as u64;
    shipper.end_base(&db, &blobs, copy, until()).unwrap();
    let second_base = newest_base(&blobs);
    let file = std::fs::metadata(dir.path().join("source.db"))
        .unwrap()
        .len();
    let second = now.load(Ordering::SeqCst).div_ceil(SECOND_NS);
    archive_through(&mut shipper, &db, &blobs, &now);
    let reads = Reads::new(&blobs);
    let target = dir.path().join("restored.db");
    let restored = backup::restore_within(
        &reads,
        "db",
        second,
        &target,
        &mut backup::Unpaced,
        &large,
        until(),
    )
    .unwrap();
    assert_eq!(rows(&Db::open(&target).unwrap()), model);
    // The latest base that ended at or before the cut (`plan::chain`): the second, copied after the
    // last commit with nothing committed since, so ended at the cut's frame. Its chunks are the
    // ones fetched.
    assert_eq!(
        second_base.end, restored.cut,
        "the second base ended at the cut"
    );
    assert_eq!(restored.base, second_base.id);
    Grown {
        first_chunk,
        chunk,
        chunks,
        reused,
        file,
        restore_gets: reads.chunk_gets.load(Ordering::SeqCst),
    }
}

/// T-T1.12.M5 (lane backups-8's A/B): a backup begun with little room, its next base made with
/// far more. Do: the same writes under each policy for the chunks' size. Expect each one's cost:
/// kept, the second base copies only the chunks the log changed but a restore fetches each of its
/// many small chunks; grown, the second base copies the whole file again but a restore fetches it
/// in one request. Both restore whole. Prints both for the record (the A/B of record, at the
/// ladder's sizes, is the coordinator's).
#[test]
fn t_t1_12_m5_chunks_kept_or_grown() {
    let keep = grown(backup::ChunkGrowth::Keep);
    let grow = grown(backup::ChunkGrowth::Grow);
    println!("chunk growth A/B: keep {keep:?}");
    println!("chunk growth A/B: grow {grow:?}");

    assert_eq!(keep.chunk, keep.first_chunk, "kept: the first base's size");
    assert!(keep.chunks > 16, "kept: {} chunks", keep.chunks);
    assert!(keep.reused > 0, "kept: unchanged chunks kept");
    let keep_copied = (keep.chunks - keep.reused) * keep.chunk;
    assert!(
        keep_copied < keep.file / 2,
        "kept: {keep_copied} bytes copied of {}",
        keep.file
    );
    assert_eq!(keep.restore_gets, keep.chunks, "kept: a request per chunk");

    assert!(grow.chunk > grow.first_chunk && grow.chunk.is_multiple_of(grow.first_chunk));
    assert!(grow.chunk >= grow.file, "grown: one chunk holds the file");
    assert_eq!(grow.reused, 0, "grown: the whole file copied again");
    assert_eq!(grow.restore_gets, 1, "grown: one request");
}

/// A store that loses the answer to the first create of a base's chunk: the chunk is stored, the
/// create refused. It counts the gets of a chunk whole.
struct LosesAnswer<'a> {
    inner: &'a dyn Blobs,
    lost: AtomicU64,
    wholes: AtomicU64,
}

impl Blobs for LosesAnswer<'_> {
    fn object_max(&self) -> usize {
        self.inner.object_max()
    }
    fn create(
        &self,
        key: &str,
        bytes: &[u8],
        until: Instant,
    ) -> Result<(), amphora_api::blobs::BlobRefusal> {
        self.inner.create(key, bytes, until)?;
        if key.contains("/pages/") && self.lost.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(amphora_api::blobs::BlobRefusal::Unavailable(
                "the answer was lost".into(),
            ));
        }
        Ok(())
    }
    fn get(&self, key: &str, until: Instant) -> Result<Vec<u8>, amphora_api::blobs::BlobRefusal> {
        if key.contains("/pages/") {
            self.wholes.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.get(key, until)
    }
    fn get_range(
        &self,
        key: &str,
        offset: u64,
        len: usize,
        until: Instant,
    ) -> Result<Vec<u8>, amphora_api::blobs::BlobRefusal> {
        self.inner.get_range(key, offset, len, until)
    }
    fn list(
        &self,
        prefix: &str,
        after: Option<&str>,
        limit: usize,
        until: Instant,
    ) -> Result<amphora_api::blobs::Listing, amphora_api::blobs::BlobRefusal> {
        self.inner.list(prefix, after, limit, until)
    }
    fn delete(&self, key: &str, until: Instant) -> Result<(), amphora_api::blobs::BlobRefusal> {
        self.inner.delete(key, until)
    }
}

/// T-T1.12.M4: a chunk whose create's answer was lost. Do: copy a base under a 64 KiB budget to a
/// store that loses the first chunk's answer; copy that chunk again, then the rest; restore.
/// Expect: the lost answer refused, the chunk found stored and compared in parts within its charge
/// (never read whole), every charge given back, and the restore whole.
#[test]
fn t_t1_12_m4_a_lost_answer_compared_in_parts() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = SimBlobs::new(1 << 30, 1 << 20, 1 << 30);
    let (db, mut shipper, model, now, second) = written(dir.path(), 100, &blobs);
    let memory = room(64 * 1024);
    let lossy = LosesAnswer {
        inner: &blobs,
        lost: AtomicU64::new(0),
        wholes: AtomicU64::new(0),
    };
    let mut copy = shipper
        .begin_base(&db, &lossy, &memory, usize::MAX)
        .unwrap();
    assert!(shipper.copy_base(&db, &lossy, &mut copy, until()).is_err());
    assert_eq!(memory.held(Use::Chunks), 0);
    while shipper.copy_base(&db, &lossy, &mut copy, until()).unwrap() {
        assert_eq!(memory.held(Use::Chunks), 0);
    }
    shipper.end_base(&db, &lossy, copy, until()).unwrap();
    assert!(lossy.lost.load(Ordering::SeqCst) > 1);
    assert_eq!(
        lossy.wholes.load(Ordering::SeqCst),
        0,
        "no chunk read whole"
    );
    archive_through(&mut shipper, &db, &blobs, &now);
    let target = dir.path().join("restored.db");
    backup::restore_within(
        &blobs,
        "db",
        second,
        &target,
        &mut backup::Unpaced,
        &memory,
        until(),
    )
    .unwrap();
    assert_eq!(rows(&Db::open(&target).unwrap()), model);
}

/// A store that refuses the `n`th create of a log segment once armed: before storing it
/// (`lose: false`), or after (`lose: true`, its answer lost).
struct FailsMidSend<'a> {
    inner: &'a dyn Blobs,
    lose: bool,
    /// Log creates to let through before the one refused; none while disarmed.
    left: std::sync::Mutex<Option<u64>>,
    refused: AtomicU64,
}

impl Blobs for FailsMidSend<'_> {
    fn object_max(&self) -> usize {
        self.inner.object_max()
    }
    fn create(
        &self,
        key: &str,
        bytes: &[u8],
        until: Instant,
    ) -> Result<(), amphora_api::blobs::BlobRefusal> {
        let refuse = key.contains("/log/") && {
            let mut left = self.left.lock().unwrap();
            match *left {
                Some(0) => {
                    *left = None;
                    true
                }
                Some(n) => {
                    *left = Some(n - 1);
                    false
                }
                None => false,
            }
        };
        if !refuse {
            return self.inner.create(key, bytes, until);
        }
        self.refused.fetch_add(1, Ordering::SeqCst);
        if self.lose {
            self.inner.create(key, bytes, until)?;
        }
        Err(amphora_api::blobs::BlobRefusal::Unavailable(
            "refused mid-send".into(),
        ))
    }
    fn get(&self, key: &str, until: Instant) -> Result<Vec<u8>, amphora_api::blobs::BlobRefusal> {
        self.inner.get(key, until)
    }
    fn get_range(
        &self,
        key: &str,
        offset: u64,
        len: usize,
        until: Instant,
    ) -> Result<Vec<u8>, amphora_api::blobs::BlobRefusal> {
        self.inner.get_range(key, offset, len, until)
    }
    fn list(
        &self,
        prefix: &str,
        after: Option<&str>,
        limit: usize,
        until: Instant,
    ) -> Result<amphora_api::blobs::Listing, amphora_api::blobs::BlobRefusal> {
        self.inner.list(prefix, after, limit, until)
    }
    fn delete(&self, key: &str, until: Instant) -> Result<(), amphora_api::blobs::BlobRefusal> {
        self.inner.delete(key, until)
    }
}

/// The log segments under "db", parsed, in key order.
fn segment_keys(blobs: &dyn Blobs) -> Vec<(backup::plan::SegmentKey, String)> {
    blobs
        .list("db/log/", None, 100_000, until())
        .unwrap()
        .objects
        .into_iter()
        .map(|(key, _)| (backup::plan::SegmentKey::parse("db", &key).unwrap(), key))
        .collect()
}

/// T-T1.12.M6: a send the store refuses partway, sent again. Do: copy a base, commit about 400 KB
/// to a store of 64 KiB objects, so it ships in several parts; refuse the third part's create
/// (before storing it, and in a second run after, its answer lost); commit again; ship. Expect:
/// the send refused, then sent again as it was, its parts stored found the same; no two segments
/// over the same frames; the commit since shipped after it; restores of both seconds whole.
#[test]
fn t_t1_12_m6_a_send_refused_partway_sent_again() {
    for lose in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let blobs = SimBlobs::new(64 * 1024, 1 << 20, 1 << 30);
        let store = FailsMidSend {
            inner: &blobs,
            lose,
            left: std::sync::Mutex::new(None),
            refused: AtomicU64::new(0),
        };
        let (mut db, mut shipper, mut model, now, _) =
            written_within(dir.path(), 10, &store, &room(1 << 30));
        copy_whole(&mut shipper, &db, &store, &room(1 << 30));
        one_commit(&mut db, &now, 100, 100, &mut model);
        let first = now.load(Ordering::SeqCst).div_ceil(SECOND_NS);
        let first_model = model.clone();
        *store.left.lock().unwrap() = Some(2);
        assert!(shipper.ship(&db, &store, until()).is_err(), "lose {lose}");
        assert_eq!(store.refused.load(Ordering::SeqCst), 1);
        one_commit(&mut db, &now, 1000, 1, &mut model);
        let shipped = shipper.ship(&db, &store, until()).unwrap();
        assert!(shipped.segments > 2, "{} parts", shipped.segments);
        let second = now.load(Ordering::SeqCst).div_ceil(SECOND_NS);
        archive_through(&mut shipper, &db, &store, &now);

        let keys: Vec<_> = segment_keys(&blobs).into_iter().map(|(s, _)| s).collect();
        assert_eq!(
            backup::plan::cover(&keys).len(),
            keys.len(),
            "lose {lose}: no segment over another's frames"
        );
        for pair in keys.windows(2) {
            if pair[0].generation == pair[1].generation {
                assert_eq!(pair[1].first, pair[0].last + 1, "lose {lose}: {pair:?}");
            }
        }
        for (name, at, want) in [
            ("first.db", first, &first_model),
            ("second.db", second, &model),
        ] {
            let target = dir.path().join(name);
            backup::restore_within(
                &blobs,
                "db",
                at,
                &target,
                &mut backup::Unpaced,
                &room(1 << 30),
                until(),
            )
            .unwrap_or_else(|e| panic!("lose {lose}, {name}: {e:#}"));
            assert_eq!(
                &rows(&Db::open(&target).unwrap()),
                want,
                "lose {lose}, {name}"
            );
        }
    }
}

/// A log segment's layout (the archive's format 1, `backup/object.rs`), as this test reads and
/// makes one: its SHA-256, then its body in postcard.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SegmentIdentity {
    seq: u32,
    salt1: u32,
    salt2: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SegmentMark {
    frame: u64,
    ns: u64,
    unknown_from: Option<u64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SegmentBody {
    format: u8,
    generation: u64,
    identity: SegmentIdentity,
    first: u64,
    header: Vec<u8>,
    marks: Vec<SegmentMark>,
    frames: Vec<u8>,
}

fn segment_body(blobs: &dyn Blobs, key: &str) -> SegmentBody {
    postcard::from_bytes(&blobs.get(key, until()).unwrap()[32..]).unwrap()
}

fn sealed_segment(body: &SegmentBody) -> Vec<u8> {
    use sha2::Digest as _;
    let plain = postcard::to_allocvec(body).unwrap();
    let mut out = sha2::Sha256::digest(&plain).to_vec();
    out.extend_from_slice(&plain);
    out
}

/// T-T1.12.M6: segments that overlap, as a send made again in other parts leaves them. Do: ship
/// about 400 KB under a 64 KiB room, so in parts; put the last part again beginning two frames
/// earlier, inside the part before it, and remove it as it was; restore; then alter a byte of those
/// two frames, sealed again, and restore. Expect: the restore whole, the overlap found to agree
/// with the log it wrote; the altered one refused for differing there, though its sum holds.
#[test]
fn t_t1_12_m6_overlapping_segments_checked_to_agree() {
    let dir = tempfile::tempdir().unwrap();
    // Parts bounded by the shipper's room, so the store takes a part made larger.
    let blobs = SimBlobs::new(1 << 30, 1 << 20, 1 << 30);
    let (mut db, mut shipper, mut model, now, _) =
        written_within(dir.path(), 10, &blobs, &room(64 * 1024));
    copy_whole(&mut shipper, &db, &blobs, &room(1 << 30));
    one_commit(&mut db, &now, 100, 100, &mut model);
    shipper.ship(&db, &blobs, until()).unwrap();
    let second = now.load(Ordering::SeqCst).div_ceil(SECOND_NS);
    archive_through(&mut shipper, &db, &blobs, &now);

    let keys = segment_keys(&blobs);
    let [.., (before_key, before_name), (last_key, last_name)] = keys.as_slice() else {
        panic!("{} segments", keys.len());
    };
    assert_eq!(before_key.generation, last_key.generation);
    assert!(before_key.marks.is_none() && last_key.marks.is_some());
    let before = segment_body(&blobs, before_name);
    let last = segment_body(&blobs, last_name);
    let frame = before.frames.len() / (before_key.last - before_key.first + 1) as usize;
    assert!(before_key.last - before_key.first >= 2);
    let mut overlapping = last.clone();
    overlapping.first = last.first - 2;
    overlapping.frames = before.frames[before.frames.len() - 2 * frame..].to_vec();
    overlapping.frames.extend_from_slice(&last.frames);
    let key = backup::plan::SegmentKey {
        first: overlapping.first,
        ..last_key.clone()
    }
    .key("db");
    blobs.delete(last_name, until()).unwrap();
    blobs
        .create(&key, &sealed_segment(&overlapping), until())
        .unwrap();
    let target = dir.path().join("restored.db");
    backup::restore_within(
        &blobs,
        "db",
        second,
        &target,
        &mut backup::Unpaced,
        &room(1 << 30),
        until(),
    )
    .unwrap();
    assert_eq!(rows(&Db::open(&target).unwrap()), model);

    let mut altered = overlapping;
    altered.frames[frame + 100] ^= 1;
    blobs.delete(&key, until()).unwrap();
    blobs
        .create(&key, &sealed_segment(&altered), until())
        .unwrap();
    let refused = backup::restore_within(
        &blobs,
        "db",
        second,
        &dir.path().join("altered.db"),
        &mut backup::Unpaced,
        &room(1 << 30),
        until(),
    )
    .unwrap_err();
    assert!(
        format!("{refused:#}").contains("differs from it"),
        "{refused:#}"
    );
}

/// T-T1.12.M3: a backlog of marks under a small room. Do: ship under a 10 KiB budget; make 150
/// commits, a second apart, while the store refuses every round; then let it take them and ship.
/// Expect: the round after ships every commit, in more than one send, each send's marks no more
/// than leave its head room for a frame (none refused for its head alone), and the restore of the
/// last second whole.
#[test]
fn t_t1_12_m3_a_backlog_of_marks_ships_over_sends() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = SimBlobs::new(1 << 30, 1 << 20, 1 << 30);
    let store = FailsMidSend {
        inner: &blobs,
        lose: false,
        left: std::sync::Mutex::new(None),
        refused: AtomicU64::new(0),
    };
    let memory = room(10 * 1024);
    let (mut db, mut shipper, mut model, now, _) = written_within(dir.path(), 10, &store, &memory);
    copy_whole(&mut shipper, &db, &store, &room(1 << 30));
    for i in 0..150 {
        one_commit(&mut db, &now, 100 + i, 1, &mut model);
        *store.left.lock().unwrap() = Some(0);
        assert!(shipper.ship(&db, &store, until()).is_err(), "round {i}");
    }
    assert_eq!(store.refused.load(Ordering::SeqCst), 150);
    *store.left.lock().unwrap() = None;
    let before = segment_keys(&blobs)
        .iter()
        .filter(|(s, _)| s.marks.is_some())
        .count();
    shipper.ship(&db, &store, until()).unwrap();
    let sends = segment_keys(&blobs)
        .iter()
        .filter(|(s, _)| s.marks.is_some())
        .count()
        - before;
    assert!(sends > 1, "{sends} sends");
    assert_eq!(memory.held(Use::Chunks), 0);
    let second = now.load(Ordering::SeqCst).div_ceil(SECOND_NS);
    archive_through(&mut shipper, &db, &store, &now);
    let target = dir.path().join("restored.db");
    backup::restore_within(
        &blobs,
        "db",
        second,
        &target,
        &mut backup::Unpaced,
        &room(1 << 30),
        until(),
    )
    .unwrap();
    assert_eq!(rows(&Db::open(&target).unwrap()), model);
}

/// Review P1 regression: PostgreSQL-dialect databases must support base backup + incremental WAL
/// point-in-time restore (with and without a WAL segment switch), restore-and-check verification,
/// and replica source inspection (`is_replica` / `inspect_source`).
#[test]
fn review_p1_postgres_dialect_backup_restore_and_replica_inspection() {
    use amphora::db::Sharing;

    for switch_wal in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pg_source.db");
        let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS + 1));
        let read = Arc::clone(&now);
        let hooks = Hooks::new(Arc::new(move || read.load(Ordering::SeqCst)), 1 << 16);
        let mut db = Db::open_postgres_sharing(&path, &Sharing::Off).unwrap();
        db.back_up(Arc::clone(&hooks));
        let blobs = SimBlobs::new(1 << 20, 1_000_000, 1 << 30);

        db.exec("CREATE TABLE items (id BIGINT PRIMARY KEY, label TEXT NOT NULL, active BOOLEAN NOT NULL, meta JSONB NOT NULL)")
            .unwrap();
        let mut shipper = Shipper::open("pg", Arc::clone(&hooks), &blobs, &db, until()).unwrap();

        let insert_batch = |db: &mut Db, range: std::ops::RangeInclusive<i64>| {
            db.transaction(|db| {
                for id in range {
                    db.query_owned(
                        "INSERT INTO items (id, label, active, meta) VALUES (?1, ?2, ?3, ?4)",
                        [
                            Value::from_i64(id),
                            Value::from_text(format!("row-{id}")),
                            Value::from_i64(i64::from(id % 2 == 0)),
                            Value::from_text(format!(r#"{{"n":{id}}}"#)),
                        ],
                        |_| Ok(true),
                    )?;
                }
                Ok(())
            })
            .unwrap();
            (db.flusher())().unwrap();
            now.fetch_add(SECOND_NS, Ordering::SeqCst);
        };

        // First 20 rows -> ship -> take base backup.
        insert_batch(&mut db, 1..=20);
        shipper.ship(&db, &blobs, until()).unwrap();
        copy_whole(&mut shipper, &db, &blobs, &room(1 << 30));

        if switch_wal {
            db.checkpoint().unwrap();
            let _ = db.restart_log().unwrap();
        }

        // Next 20 rows -> ship -> archive through this second.
        insert_batch(&mut db, 21..=40);
        let second = now.load(Ordering::SeqCst).div_ceil(SECOND_NS);
        archive_through(&mut shipper, &db, &blobs, &now);

        let expected_digest = backup::digest(&db).unwrap();

        // Point-in-time restore must replay PostgreSQL-dialect base + WAL cleanly.
        let restored_path = dir.path().join("restored.db");
        backup::restore(&blobs, "pg", second, &restored_path, until())
            .unwrap_or_else(|e| panic!("switch_wal={switch_wal}: restore failed: {e:#}"));
        let restored_db = Db::open_postgres_sharing(&restored_path, &Sharing::Off).unwrap();
        let mut got = Vec::new();
        restored_db
            .query("SELECT id, label FROM items ORDER BY id", [], |r| {
                got.push((
                    r.get::<i64>(0).map_err(|e| anyhow::anyhow!("{e}"))?,
                    r.get::<String>(1).map_err(|e| anyhow::anyhow!("{e}"))?,
                ));
                Ok(true)
            })
            .unwrap();
        let want: Vec<(i64, String)> = (1..=40).map(|id| (id, format!("row-{id}"))).collect();
        assert_eq!(got, want, "switch_wal={switch_wal}: restored rows mismatch");
        assert_eq!(
            backup::digest(&restored_db).unwrap(),
            expected_digest,
            "switch_wal={switch_wal}: restored digest mismatch"
        );
        drop(restored_db);

        // Restore-and-check must also pass against the PostgreSQL-dialect source digest.
        let check_path = dir.path().join("checked.db");
        let counters = backup::CheckCounters::default();
        let check = backup::restore_and_check_at(
            &blobs,
            "pg",
            second,
            &check_path,
            &|_| Ok(Some(expected_digest.clone())),
            &counters,
            &mut backup::Unpaced,
            until(),
        )
        .unwrap();
        assert!(
            matches!(check, backup::Check::Passed { .. }),
            "switch_wal={switch_wal}: restore_and_check_at failed: {check:?}"
        );

        // Drop source writer handle so replica bootstrap inspection can acquire the exclusive lease.
        drop(shipper);
        drop(db);

        // Reopening source confirms all 40 rows are durable on disk.
        {
            let reopened = Db::open_postgres_sharing(&path, &Sharing::Off).unwrap();
            let mut cnt = 0i64;
            reopened
                .query("SELECT COUNT(*) FROM items", [], |r| {
                    cnt = r.get::<i64>(0).map_err(|e| anyhow::anyhow!("{e}"))?;
                    Ok(false)
                })
                .unwrap();
            assert_eq!(cnt, 40, "switch_wal={switch_wal}: source reopen row count");
        }

        // Replica source inspection (`is_replica` and `inspect_source`) must open PostgreSQL-dialect
        // source databases without rejecting their schema as corrupt.
        assert!(
            !amphora::replication::bootstrap::is_replica(&path)
                .unwrap_or_else(|e| panic!("switch_wal={switch_wal}: is_replica failed: {e}")),
            "switch_wal={switch_wal}: source must not be reported as replica"
        );
        let bootstrap = amphora::replication::bootstrap::inspect_source(&path, 1)
            .unwrap_or_else(|e| panic!("switch_wal={switch_wal}: inspect_source failed: {e}"));
        assert_eq!(bootstrap.source_node, 1);
    }
}

/// Review P2 regression: backup row-digest verification must include legal user tables whose name
/// starts with `sqlite` without an underscore (e.g. `sqliteLedger`) AND must incorporate rowids so
/// databases with identical visible columns at different rowids do not verify as equal.
#[test]
fn review_p2_digest_includes_sqlite_prefix_tables_and_rowids() {
    let dir = tempfile::tempdir().unwrap();

    // 1. Legal user table `sqliteLedger` (no underscore after `sqlite`) must be included in digest.
    let a = Db::open(&dir.path().join("ledger_a.db")).unwrap();
    let b = Db::open(&dir.path().join("ledger_b.db")).unwrap();
    for (db, count) in [(&a, 31i64), (&b, 40i64)] {
        db.exec("CREATE TABLE sqliteLedger (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")
            .unwrap();
        for i in 1..=count {
            db.query_owned(
                "INSERT INTO sqliteLedger (id, v) VALUES (?1, ?2)",
                [Value::from_i64(i), Value::from_text(format!("r-{i}"))],
                |_| Ok(true),
            )
            .unwrap();
        }
    }
    let da = backup::digest(&a).unwrap();
    let db_dig = backup::digest(&b).unwrap();
    assert!(
        da.0.iter()
            .any(|(name, rows, _)| name.eq_ignore_ascii_case("sqliteLedger") && *rows == 31),
        "sqliteLedger must be included in RowDigest: {da:?}"
    );
    assert_ne!(
        da, db_dig,
        "databases with different row counts in sqliteLedger must not have equal digests"
    );

    // 2. Regular rowid table with same visible column value at rowid 9 vs rowid 1 must differ.
    let r1 = Db::open(&dir.path().join("rowid_a.db")).unwrap();
    let r2 = Db::open(&dir.path().join("rowid_b.db")).unwrap();
    r1.exec("CREATE TABLE notes (v TEXT NOT NULL)").unwrap();
    r2.exec("CREATE TABLE notes (v TEXT NOT NULL)").unwrap();
    r1.exec("INSERT INTO notes (rowid, v) VALUES (9, 'same')")
        .unwrap();
    r2.exec("INSERT INTO notes (rowid, v) VALUES (1, 'same')")
        .unwrap();
    assert_ne!(
        backup::digest(&r1).unwrap(),
        backup::digest(&r2).unwrap(),
        "identical visible columns at different rowids must produce different digests"
    );

    // 3. Table shadowing `rowid` as a user column must fall back to `_rowid_` for hidden rowid.
    let s1 = Db::open(&dir.path().join("shadow_a.db")).unwrap();
    let s2 = Db::open(&dir.path().join("shadow_b.db")).unwrap();
    s1.exec("CREATE TABLE shadowed (rowid TEXT NOT NULL, v TEXT NOT NULL)")
        .unwrap();
    s2.exec("CREATE TABLE shadowed (rowid TEXT NOT NULL, v TEXT NOT NULL)")
        .unwrap();
    s1.exec("INSERT INTO shadowed (_rowid_, rowid, v) VALUES (9, 'lbl', 'same')")
        .unwrap();
    s2.exec("INSERT INTO shadowed (_rowid_, rowid, v) VALUES (1, 'lbl', 'same')")
        .unwrap();
    assert_ne!(
        backup::digest(&s1).unwrap(),
        backup::digest(&s2).unwrap(),
        "table shadowing rowid column must still distinguish hidden _rowid_ values"
    );
}
