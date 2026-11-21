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

use amphora::backup::{self, BaseCopy, Hooks, SECOND_NS, Shipper, plan::Unrestorable};
use amphora::db::Db;
use amphora_api::blobs::SimBlobs;
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
            }
            11 | 12 => (db.flusher())().unwrap(),
            13 => {
                let mut reader = db.reader().unwrap();
                reader.checkpoint().unwrap();
            }
            14 => {
                // Refused while the log isn't shipped whole: tried again later, as the store does.
                let _ = db.restart_log().unwrap();
            }
            15 | 16 => {
                shipper.ship(&db, &blobs, until()).unwrap();
            }
            _ => match copy.as_mut() {
                None => {
                    shipper.ship(&db, &blobs, until()).unwrap();
                    copy = shipper.begin_base(&db, &blobs, 32 * 1024).ok();
                }
                Some(c) => {
                    if !shipper.copy_base(&db, &blobs, c, until()).unwrap() {
                        let c = copy.take().unwrap();
                        shipper.end_base(&db, &blobs, c, until()).unwrap();
                        first_base_end.get_or_insert(now.load(Ordering::SeqCst));
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
    }
    tick(&mut rng);
    shipper.ship(&db, &blobs, until()).unwrap();
    let last = history.last().unwrap().0;
    let from = first_base_end
        .expect("a base was copied")
        .div_ceil(SECOND_NS);
    let to = last.div_ceil(SECOND_NS);
    assert!(to > from, "the drill spans seconds after its first base");
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
            Err(e) => panic!("seed {seed:#x}: second {second} refused: {e:#}"),
        }
    }
    assert_eq!(restored, 12);
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
