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
use amphora::database::Database;
use amphora::db::Db;
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
                    copy = shipper.begin_base(&db, &blobs, 32 * 1024).ok();
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
            .call(until(), move |c| {
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
        .call(until(), |c| {
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
/// model kept them.
fn oracle_digest(dir: &std::path::Path, name: &str, rows: &Rows) -> backup::RowDigest {
    let db = Db::open(&dir.join(name)).unwrap();
    db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")
        .unwrap();
    for (id, v) in rows {
        db.exec(&format!("INSERT INTO t (id, v) VALUES ({id}, '{v}')"))
            .unwrap();
    }
    backup::digest(&db).unwrap()
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
        .call(until(), |c| {
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
        cross_group_cut(protocol);
    }
}

fn cross_group_cut(protocol: amphora::xgroup::Protocol) {
    use amphora::xgroup::{Answer, Cluster, Protocol, Txn};
    let parallel = matches!(protocol, Protocol::Parallel);
    let fresh = || match parallel {
        true => Protocol::Parallel,
        false => Protocol::TwoPhase,
    };
    let dir = tempfile::tempdir().unwrap();
    let blobs =
        Arc::new(backup::FolderBlobs::open(&dir.path().join("archive"), 64 * 1024).unwrap());
    let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS + 1));
    let open = |group: usize| {
        let read = Arc::clone(&now);
        let archive = Archive {
            blobs: Arc::clone(&blobs) as Arc<dyn Blobs>,
            prefix: format!("g{group}"),
            clock: Arc::new(move || read.fetch_add(SECOND_NS, Ordering::SeqCst) + SECOND_NS),
            memory: 32 * 1024,
            patience: Duration::from_secs(60),
        };
        Database::open_backed_up(&dir.path().join(format!("g{group}.db")), archive, until())
            .unwrap()
    };
    let (g0, s0) = open(0);
    let (g1, s1) = open(1);
    let route: amphora::xgroup::Route = Arc::new(|k: &[u8]| u32::from(k == b"b"));
    let cluster = Cluster::open(vec![g0, g1], Arc::clone(&route), 7, fresh(), 1, until()).unwrap();
    s0.wait(until(), |s| s.bases >= 1).unwrap();
    s1.wait(until(), |s| s.bases >= 1).unwrap();
    let based = now.load(Ordering::SeqCst).div_ceil(SECOND_NS);
    for v in [b"1", b"2"] {
        let txn = Txn {
            reads: vec![],
            writes: vec![
                (b"a".to_vec(), Some(v.to_vec())),
                (b"b".to_vec(), Some(v.to_vec())),
            ],
        };
        assert!(matches!(
            cluster.transact(txn, until()).unwrap(),
            Answer::Committed { .. }
        ));
    }
    drop(cluster);
    for shipping in [&s0, &s1] {
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
        backup::restore_cut(blobs.as_ref(), &groups, second, 7, fresh(), until())
            .unwrap_or_else(|e| panic!("second {second}: {e:#}"));
        let restored: Vec<Database> = groups
            .iter()
            .map(|(_, p)| Database::open(p, until()).unwrap())
            .collect();
        let reader = Cluster::open(restored, Arc::clone(&route), 7, fresh(), 1, until()).unwrap();
        let read = Txn {
            reads: vec![b"a".to_vec(), b"b".to_vec()],
            writes: vec![],
        };
        let Answer::Committed { reads } = reader.transact(read, until()).unwrap() else {
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
