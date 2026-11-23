//! The premise behind lane snapshots' pull (docs/design/replication.md, "Decision (lane
//! snapshots)"): a source that pushed pages from one read view held for a whole transfer would keep
//! its checkpoint from copying past the view and its log from starting again, so its log would grow
//! by every write it takes for as long as the transfer lasts. A view that lasts one answer leaves
//! both alone. Measured here in frames and bytes of log, never time.

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

use amphora::db::Db;
use amphora_engine::Value;

/// A directory on the build's own file system (not a tmpfs `/tmp`).
fn lab() -> tempfile::TempDir {
    tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap()
}

fn log_bytes(path: &std::path::Path) -> u64 {
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    std::fs::metadata(wal).map_or(0, |m| m.len())
}

/// How a source reads while its writer commits.
#[derive(Clone, Copy, Debug, PartialEq)]
enum View {
    /// No reader.
    None,
    /// One short view between commits, as each answer of the pull reads.
    PerAnswer,
    /// One view begun before the first commit and kept to the end, as a push from a pinned view.
    Held,
}

/// The log after one commit and the store's two log steps (a passive checkpoint, then a restart):
/// its file's bytes, its frames and those of them the checkpoint left uncopied (both before the
/// restart), and whether the restart happened.
#[derive(Clone, Copy, Debug)]
struct Sample {
    commit: usize,
    log_bytes: u64,
    frames: u64,
    uncopied: u64,
    restarted: bool,
}

const ROWS: i64 = 256;
const ROWS_A_COMMIT: i64 = 4;

/// A table of `ROWS` rows of a page each, then `commits` commits that each rewrite `ROWS_A_COMMIT`
/// of them (the database's size stays put, so only the log can grow), each followed by the
/// store's checkpoint and restart, the most often the store could run them.
fn run(view: View, commits: usize) -> (Vec<Sample>, u64, Db, tempfile::TempDir) {
    let dir = lab();
    let path = dir.path().join("t.db");
    let mut db = Db::open(&path).unwrap();
    db.exec("CREATE TABLE t (n INTEGER PRIMARY KEY, body BLOB NOT NULL)")
        .unwrap();
    db.transaction(|db| {
        for n in 0..ROWS {
            db.query(
                "INSERT INTO t (n, body) VALUES (?1, randomblob(4000))",
                [Value::from_i64(n)],
                |_| Ok(true),
            )?;
        }
        Ok(())
    })
    .unwrap();
    // The load's log cut to nothing, so the log's file holds what the commits below write; then no
    // restart cuts it, as within the store's default bound.
    db.checkpoint().unwrap();
    db.keep_log(0);
    assert!(db.restart_log().unwrap(), "the log starts empty");
    db.keep_log(u64::MAX);
    let rewrite = |db: &mut Db, first: i64| {
        db.transaction(|db| {
            for n in first..first + ROWS_A_COMMIT {
                db.query(
                    "UPDATE t SET body = randomblob(4000) WHERE n = ?1",
                    [Value::from_i64(n)],
                    |_| Ok(true),
                )?;
            }
            Ok(())
        })
        .unwrap();
    };
    // One commit before any view, so a view begun now marks a point inside the log, as a
    // transfer's view would, not an empty log.
    rewrite(&mut db, 0);
    let mark = db.uncopied().log;
    let reader = db.reader().unwrap();
    let read_one = |n: i64| {
        let mut seen = 0i64;
        reader
            .query("SELECT length(body) FROM t WHERE n = ?1", [Value::from_i64(n)], |r| {
                seen = r.get::<i64>(0).map_err(|e| anyhow::anyhow!("{e}"))?;
                Ok(true)
            })
            .unwrap();
        assert_eq!(seen, 4000, "row {n} reads whole");
    };
    if view == View::Held {
        reader.exec("BEGIN").unwrap();
        read_one(0);
    }
    let mut samples = Vec::with_capacity(commits);
    for commit in 0..commits {
        if view == View::PerAnswer {
            reader.exec("BEGIN").unwrap();
            read_one(i64::try_from(commit).unwrap() % ROWS);
            reader.exec("ROLLBACK").unwrap();
        }
        rewrite(&mut db, (i64::try_from(commit).unwrap() * ROWS_A_COMMIT) % ROWS);
        db.checkpoint().unwrap();
        let after = db.uncopied();
        let restarted = db.restart_log().unwrap();
        samples.push(Sample {
            commit: commit + 1,
            log_bytes: log_bytes(&path),
            frames: after.log,
            uncopied: after.left,
            restarted,
        });
    }
    if view == View::Held {
        reader.exec("ROLLBACK").unwrap();
    }
    (samples, mark, db, dir)
}

fn table(view: View, samples: &[Sample]) {
    for s in samples
        .iter()
        .filter(|s| [1, 10, 100, 1000, 2000].contains(&s.commit))
    {
        eprintln!(
            "{view:?}\tcommit {}\tlog file {} bytes\t{} frames\t{} uncopied after the checkpoint\trestarted {}",
            s.commit, s.log_bytes, s.frames, s.uncopied, s.restarted
        );
    }
}

/// Do: one commit, then 2000 commits of four rewritten rows each, every one followed by a
/// checkpoint and a restart, once with no reader, once with a short view before each commit (a
/// pull's answers), and once with one view begun after the first commit and held across all the
/// rest (a push from a pinned view). Expect: without a held view the log starts again after every
/// commit and holds one commit's frames; with one, the checkpoint copies nothing past the view's
/// mark, no restart happens, and the log grows by every commit's frames, until the view ends and
/// the next restart empties it.
#[test]
fn snapshots_a_view_held_across_writes_stops_the_checkpoint_and_grows_the_log() {
    const COMMITS: usize = 2000;
    let mut bounded = Vec::new();
    for view in [View::None, View::PerAnswer] {
        let (samples, _, _db, _dir) = run(view, COMMITS);
        table(view, &samples);
        assert!(
            samples.iter().all(|s| s.restarted && s.uncopied == 0),
            "{view:?}: every commit's frames are copied and its log starts again"
        );
        let early = samples[..10].iter().map(|s| s.frames).max().unwrap();
        let most = samples.iter().map(|s| s.frames).max().unwrap();
        assert!(most <= early, "{view:?}: the log holds one commit's frames ({most} > {early})");
        let file = samples.iter().map(|s| s.log_bytes).max().unwrap();
        assert_eq!(file, samples.last().unwrap().log_bytes, "{view:?}: the log's file stops growing");
        bounded.push(file);
    }
    let (held, mark, mut db, _dir) = run(View::Held, COMMITS);
    table(View::Held, &held);
    eprintln!("Held\tthe view's mark: {mark} frames");
    assert!(mark > 0, "the view marks a point inside the log");
    assert!(
        held.iter().all(|s| !s.restarted),
        "no restart while the view is held"
    );
    assert!(
        held.iter().all(|s| s.uncopied >= s.frames - mark),
        "the checkpoint copies nothing past the held view's mark"
    );
    let per_commit = held[0].frames - mark;
    assert!(per_commit >= u64::try_from(ROWS_A_COMMIT).unwrap(), "a commit writes its rows' pages");
    assert!(
        held.windows(2).all(|w| w[1].frames > w[0].frames),
        "the log's frames grow at every commit"
    );
    let grown = held.last().unwrap();
    assert!(
        grown.frames >= mark + per_commit * u64::try_from(COMMITS).unwrap() / 2,
        "{} frames after {COMMITS} commits of {per_commit}",
        grown.frames
    );
    let most_bounded = bounded.into_iter().max().unwrap();
    eprintln!(
        "held/bounded log bytes after {COMMITS} commits: {} / {} = {}x",
        grown.log_bytes,
        most_bounded,
        grown.log_bytes / most_bounded.max(1)
    );
    assert!(grown.log_bytes > 100 * most_bounded, "the held view's log outgrows the bounded ones");
    db.checkpoint().unwrap();
    assert!(db.restart_log().unwrap(), "the view ended, the log starts again");
    assert_eq!(db.uncopied().left, 0, "every frame copied once the view ended");
}
