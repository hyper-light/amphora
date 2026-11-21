//! A store in groups (CP-T1.1; AC-T1.1.1: no update dropped unsaid): rows go to their key's group
//! and are read back from it, each group's file holds only its keys' rows, and a batch with rows in
//! more than one group is refused, nothing of it written.

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

use std::collections::HashSet;
use std::time::{Duration, Instant};

use amphora::db::Db;
use amphora::groups::Groups;
use amphora::reshard::directory::{Directory, Proposal};
use amphora::reshard::driver::{Answer, Driver, Out};
use amphora::reshard::holding::{Away, Fenced, replays};
use amphora::reshard::{GroupId, NodeId, OpId, Pos, Seq, Span, position};
use amphora::store::{
    Changed, Checkpointed, Config, Engine, Exported, Flush, Moved, Opener, Refused,
};
use amphora_engine::Value;

struct Events {
    db: Db,
    /// Its log (CP-T1.9's copy path): the last entry's number, the entry it is held from (none:
    /// not held), and the last entry let go.
    head: u64,
    pinned: Option<u64>,
    trimmed: u64,
}

/// A position as SQL orders it: an i64 whose order is the u64's.
fn sql_pos(pos: Pos) -> Value {
    Value::from_i64(i64::from_ne_bytes((pos ^ (1 << 63)).to_ne_bytes()))
}

fn pos_of(v: i64) -> Pos {
    u64::from_ne_bytes(v.to_ne_bytes()) ^ (1 << 63)
}

fn opt(v: Option<u64>) -> Value {
    v.map_or(Value::Null, |v| Value::from_i64(i64::try_from(v).unwrap()))
}

/// Keys with their positions, the last position taken, and whether none is past it.
type KeysIn = (Vec<(String, Pos)>, Option<Pos>, bool);

impl Events {
    fn open(db: Db) -> anyhow::Result<Self> {
        let mut meta = (0, None, 0);
        db.query(LOG, [], |r| {
            let n = |i| r.get::<i64>(i).ok().map(|v| u64::try_from(v).unwrap());
            meta = (n(0).unwrap(), n(1), n(2).unwrap());
            Ok(false)
        })?;
        Ok(Self {
            db,
            head: meta.0,
            pinned: meta.1,
            trimmed: meta.2,
        })
    }

    fn save_log(db: &mut Db, head: u64, pinned: Option<u64>, trimmed: u64) -> anyhow::Result<()> {
        db.query(
            SAVE_LOG,
            [opt(Some(head)), opt(pinned), opt(Some(trimmed))],
            |_| Ok(true),
        )
    }

    /// Each of `keys`' rows as they stand now, stamped with the head.
    fn rows_of(&self, keys: Vec<(String, Pos, u64)>) -> anyhow::Result<Vec<Moved>> {
        let mut moved = Vec::with_capacity(keys.len());
        for (key, pos, entry) in keys {
            let mut seqs: Vec<i64> = Vec::new();
            self.db.query(SEQS, [Value::from_text(key.clone())], |r| {
                seqs.push(r.get::<i64>(0).map_err(|e| anyhow::anyhow!("{e}"))?);
                Ok(true)
            })?;
            let rows = (!seqs.is_empty()).then(|| postcard::to_allocvec(&seqs).unwrap());
            moved.push(Moved {
                pos,
                key,
                rows,
                entry,
                at: self.head,
            });
        }
        Ok(moved)
    }

    /// The keys at the first `positions` positions of `span` that `keep` takes, in position
    /// order, and whether none is past them.
    fn keys_in(
        &self,
        span: Span,
        positions: usize,
        keep: &dyn Fn(Pos) -> bool,
    ) -> anyhow::Result<KeysIn> {
        let mut keys: Vec<(String, Pos)> = Vec::new();
        let (mut seen, mut last, mut done) = (0, None, true);
        self.db
            .query(IN_SPAN, [sql_pos(span.lo()), sql_pos(span.hi())], |r| {
                let key = r.get::<String>(0).map_err(|e| anyhow::anyhow!("{e}"))?;
                let pos = pos_of(r.get::<i64>(1).map_err(|e| anyhow::anyhow!("{e}"))?);
                if !keep(pos) {
                    return Ok(true);
                }
                if last != Some(pos) {
                    if seen == positions {
                        done = false;
                        return Ok(false);
                    }
                    seen += 1;
                    last = Some(pos);
                }
                if keys.last().is_none_or(|k| k.0 != key) {
                    keys.push((key, pos));
                }
                Ok(true)
            })?;
        keys.dedup();
        Ok((keys, last, done))
    }
}

struct Row {
    key: String,
    seq: i64,
}

const INSERT: &str = "INSERT INTO events (key, seq, pos) VALUES (?1, ?2, ?3)";

impl Engine for Events {
    type Row = Row;
    type Reader = Db;

    fn key(row: &Row) -> &str {
        &row.key
    }

    fn apply(&mut self, rows: &[Row]) -> anyhow::Result<usize> {
        let head = self.head + 1;
        let pinned = self.pinned;
        let trimmed = if pinned.is_some() { self.trimmed } else { head };
        let n = self.db.transaction(|db| {
            for r in rows {
                let pos = sql_pos(position(&r.key));
                db.query(
                    INSERT,
                    [
                        Value::from_text(r.key.clone()),
                        Value::from_i64(r.seq),
                        pos.clone(),
                    ],
                    |_| Ok(true),
                )?;
                if pinned.is_some() {
                    db.query(
                        FEED,
                        [opt(Some(head)), pos, Value::from_text(r.key.clone())],
                        |_| Ok(true),
                    )?;
                }
            }
            Self::save_log(db, head, pinned, trimmed)?;
            Ok(rows.len())
        })?;
        self.head = head;
        self.trimmed = trimmed;
        Ok(n)
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

    fn holding(&mut self) -> anyhow::Result<Option<Vec<u8>>> {
        let mut kept = None;
        self.db.query(KEPT, [], |r| {
            kept = r.get_value(0).to_blob().map(<[u8]>::to_vec);
            Ok(false)
        })?;
        Ok(kept)
    }

    fn keep_holding(&mut self, holding: &[u8]) -> anyhow::Result<()> {
        let bytes = Value::from_slice(holding).map_err(|e| anyhow::anyhow!("{e}"))?;
        let (head, pinned) = (self.head + 1, self.pinned);
        let trimmed = if pinned.is_some() { self.trimmed } else { head };
        self.db.transaction(|db| {
            db.query(KEEP, [bytes], |_| Ok(true))?;
            Self::save_log(db, head, pinned, trimmed)
        })?;
        (self.db.flusher())()?;
        self.head = head;
        self.trimmed = trimmed;
        Ok(())
    }

    fn head(&mut self) -> anyhow::Result<Seq> {
        Ok(self.head)
    }

    fn export(&mut self, span: Span, positions: usize) -> anyhow::Result<Exported> {
        let (keys, last, done) = self.keys_in(span, positions, &|_| true)?;
        let head = self.head;
        let keys = self.rows_of(keys.into_iter().map(|(k, p)| (k, p, head)).collect())?;
        Ok(Exported {
            keys,
            at: head,
            last,
            done,
        })
    }

    fn changed(
        &mut self,
        after: Seq,
        upto: Seq,
        span: Span,
        entries: u64,
    ) -> anyhow::Result<Option<Changed>> {
        if after < self.trimmed {
            return Ok(None);
        }
        let through = upto.min(self.head).min(after + entries).max(after);
        let mut keys: Vec<(String, Pos, u64)> = Vec::new();
        self.db.query(
            CHANGED,
            [
                opt(Some(after)),
                opt(Some(through)),
                sql_pos(span.lo()),
                sql_pos(span.hi()),
            ],
            |r| {
                let key = r.get::<String>(0).map_err(|e| anyhow::anyhow!("{e}"))?;
                let pos = pos_of(r.get::<i64>(1).map_err(|e| anyhow::anyhow!("{e}"))?);
                let seq = u64::try_from(r.get::<i64>(2).map_err(|e| anyhow::anyhow!("{e}"))?)?;
                match keys.iter_mut().find(|k| k.0 == key) {
                    Some(k) => k.2 = k.2.max(seq),
                    None => keys.push((key, pos, seq)),
                }
                Ok(true)
            },
        )?;
        Ok(Some(Changed {
            keys: self.rows_of(keys)?,
            through,
            head: self.head,
        }))
    }

    fn import(&mut self, moved: &[Moved]) -> anyhow::Result<()> {
        self.db.transaction(|db| {
            for m in moved {
                let key = Value::from_text(m.key.clone());
                let mut stamp = None;
                db.query(STAMP, [key.clone()], |r| {
                    stamp = Some(u64::try_from(
                        r.get::<i64>(0).map_err(|e| anyhow::anyhow!("{e}"))?,
                    )?);
                    Ok(false)
                })?;
                if !replays(stamp, m.entry) {
                    continue;
                }
                db.query(DROP_KEY, [key.clone()], |_| Ok(true))?;
                db.query(DROP_STAMP, [key.clone()], |_| Ok(true))?;
                if let Some(rows) = &m.rows {
                    let seqs: Vec<i64> = postcard::from_bytes(rows)?;
                    for seq in seqs {
                        db.query(
                            INSERT,
                            [key.clone(), Value::from_i64(seq), sql_pos(m.pos)],
                            |_| Ok(true),
                        )?;
                    }
                    db.query(KEEP_STAMP, [key.clone(), opt(Some(m.at))], |_| Ok(true))?;
                }
            }
            Ok(())
        })?;
        (self.db.flusher())()?;
        Ok(())
    }

    fn keep_log(&mut self, from: Option<Seq>) -> anyhow::Result<()> {
        let pinned = match (self.pinned, from) {
            (Some(p), Some(f)) => Some(p.min(f)),
            (_, f) => f,
        };
        let trimmed = if pinned.is_some() {
            self.trimmed
        } else {
            self.head
        };
        let head = self.head;
        self.db.transaction(|db| {
            if pinned.is_none() {
                db.query(DROP_FEED, [], |_| Ok(true))?;
            }
            Self::save_log(db, head, pinned, trimmed)
        })?;
        (self.db.flusher())()?;
        self.pinned = pinned;
        self.trimmed = trimmed;
        Ok(())
    }

    fn sweep(
        &mut self,
        span: Span,
        positions: usize,
        garbage: &dyn Fn(Pos) -> bool,
    ) -> anyhow::Result<bool> {
        let (keys, _, done) = self.keys_in(span, positions, garbage)?;
        self.db.transaction(|db| {
            for (key, _) in &keys {
                db.query(DROP_KEY, [Value::from_text(key.clone())], |_| Ok(true))?;
                db.query(DROP_STAMP, [Value::from_text(key.clone())], |_| Ok(true))?;
            }
            Ok(())
        })?;
        (self.db.flusher())()?;
        Ok(done)
    }
}

const LOG: &str = "SELECT head, pinned, trimmed FROM log WHERE id = 1";
const SAVE_LOG: &str =
    "INSERT OR REPLACE INTO log (id, head, pinned, trimmed) VALUES (1, ?1, ?2, ?3)";
const FEED: &str = "INSERT INTO feed (seq, pos, key) VALUES (?1, ?2, ?3)";
const DROP_FEED: &str = "DELETE FROM feed";
const CHANGED: &str =
    "SELECT key, pos, seq FROM feed WHERE seq > ?1 AND seq <= ?2 AND pos >= ?3 AND pos <= ?4";
const IN_SPAN: &str = "SELECT key, pos FROM events WHERE pos >= ?1 AND pos <= ?2 ORDER BY pos, key";
const SEQS: &str = "SELECT seq FROM events WHERE key = ?1";
const STAMP: &str = "SELECT stamp FROM stamps WHERE key = ?1";
const KEEP_STAMP: &str = "INSERT OR REPLACE INTO stamps (key, stamp) VALUES (?1, ?2)";
const DROP_STAMP: &str = "DELETE FROM stamps WHERE key = ?1";
const DROP_KEY: &str = "DELETE FROM events WHERE key = ?1";

/// The group's holding, kept with its rows (CP-T1.9).
const KEPT: &str = "SELECT bytes FROM holding WHERE id = 1";
const KEEP: &str = "INSERT OR REPLACE INTO holding (id, bytes) VALUES (1, ?1)";

fn soon() -> Instant {
    Instant::now() + Duration::from_secs(10)
}

fn row(key: &str, seq: i64) -> Row {
    Row {
        key: key.to_owned(),
        seq,
    }
}

const GROUPS: usize = 4;

/// A group's engine on its own file in `dir`.
fn events(dir: &std::path::Path, name: &str) -> anyhow::Result<Events> {
    let db = Db::open(&dir.join(format!("events-{name}.db")))?;
    db.exec(
        "CREATE TABLE IF NOT EXISTS events (key TEXT, seq INTEGER, pos INTEGER);
         CREATE INDEX IF NOT EXISTS by_key ON events (key);
         CREATE INDEX IF NOT EXISTS by_pos ON events (pos);
         CREATE TABLE IF NOT EXISTS holding (id INTEGER PRIMARY KEY, bytes BLOB);
         CREATE TABLE IF NOT EXISTS log (id INTEGER PRIMARY KEY, head INTEGER, pinned INTEGER, trimmed INTEGER);
         CREATE TABLE IF NOT EXISTS feed (seq INTEGER, pos INTEGER, key TEXT);
         CREATE INDEX IF NOT EXISTS feed_by_seq ON feed (seq);
         CREATE TABLE IF NOT EXISTS stamps (key TEXT PRIMARY KEY, stamp INTEGER)",
    )?;
    Events::open(db)
}

fn lab(dir: &std::path::Path) -> Groups<Events> {
    Groups::start(GROUPS, Config::default(), |i| events(dir, &i.to_string())).unwrap()
}

const COUNT: &str = "SELECT count(*) FROM events WHERE key = ?1";
const KEYS: &str = "SELECT DISTINCT key FROM events";

fn count(groups: &Groups<Events>, key: &str) -> i64 {
    let (k, w) = (key.to_owned(), key.to_owned());
    let count_on = |db: &mut Db, key: &str| {
        let mut n = 0;
        db.query(COUNT, [Value::from_text(key.to_owned())], |r| {
            n = r.get::<i64>(0).map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(true)
        })?;
        Ok(n)
    };
    groups
        .read(
            key,
            soon(),
            move |db| count_on(db, &k),
            move |e| count_on(&mut e.db, &w),
        )
        .unwrap()
}

/// Rows go to their key's group and are read back from it, and each group's file holds only the
/// keys whose group it is.
#[test]
fn rows_go_to_their_keys_group_and_read_back_from_it() {
    let dir = tempfile::tempdir().unwrap();
    let groups = lab(dir.path());
    let keys: Vec<String> = (0..20).map(|i| format!("k{i}")).collect();
    let mut tickets = Vec::new();
    for (i, key) in keys.iter().enumerate() {
        let rows = (0..=i)
            .map(|seq| row(key, i64::try_from(seq).unwrap()))
            .collect();
        tickets.push(groups.put(rows).unwrap());
    }
    for t in &tickets {
        groups.durable(t, soon()).unwrap();
    }
    for (i, key) in keys.iter().enumerate() {
        assert_eq!(count(&groups, key), i64::try_from(i).unwrap() + 1);
    }
    let held = groups
        .read_each(
            soon(),
            false,
            |db| {
                let mut keys = Vec::new();
                db.query(KEYS, [], |r| {
                    keys.push(r.get::<String>(0).map_err(|e| anyhow::anyhow!("{e}"))?);
                    Ok(true)
                })?;
                Ok(keys)
            },
            |_| Ok(Vec::new()),
        )
        .unwrap();
    let mut seen = HashSet::new();
    for (group, keys) in held.iter().enumerate() {
        for key in keys {
            assert_eq!(
                groups.group(key),
                Some(GroupId(u64::try_from(group).unwrap())),
                "{key} is in group {group}'s file"
            );
            assert!(seen.insert(key.clone()));
        }
    }
    assert_eq!(seen.len(), keys.len());
    // Twenty keys over four groups: more than one group holds some (the spread isn't vacuous).
    assert!(held.iter().filter(|k| !k.is_empty()).count() > 1);
}

/// A batch with rows in more than one group is refused, typed, and nothing of it is written.
#[test]
fn a_batch_spanning_groups_is_refused_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let groups = lab(dir.path());
    let a = "a".to_owned();
    let b = (0..)
        .map(|i| format!("b{i}"))
        .find(|k| groups.group(k) != groups.group(&a))
        .unwrap();
    assert_eq!(
        groups.put(vec![row(&a, 1), row(&b, 1)]).err(),
        Some(Refused::Groups)
    );
    groups.sync(soon()).unwrap();
    assert_eq!(count(&groups, &a), 0);
    assert_eq!(count(&groups, &b), 0);
}

/// T-T1.9.R3 (CP-T1.9; docs/design/reshard.md, "A group's holding"): fence one key's position
/// at its group, then start the groups again on the same files; expect the group to answer from
/// the holding its own database kept with its rows: the fenced key still refused, typed, and the
/// group's other keys still taken, with no router's memory carried over.
#[test]
fn t_t1_9_r3_a_groups_holding_is_kept_with_its_rows_across_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let groups = lab(dir.path());
    let key = "k0";
    let group = groups.group(key).unwrap();
    let other = (1..)
        .map(|i| format!("k{i}"))
        .find(|k| groups.group(k) == Some(group))
        .unwrap();
    let pos = position(key);
    let span = Span::new(pos, pos).unwrap();
    let fenced = groups
        .hold(group, soon(), |h| h.fence(OpId(1), span, 1))
        .unwrap();
    assert_eq!(fenced, Fenced::New(1));
    let away = Some(Refused::Away(Away::Fenced(OpId(1))));
    assert_eq!(groups.put(vec![row(key, 1)]).err(), away);
    groups.sync(soon()).unwrap();
    drop(groups);

    let again = lab(dir.path());
    assert_eq!(again.put(vec![row(key, 2)]).err(), away);
    let t = again.put(vec![row(&other, 3)]).unwrap();
    assert_eq!(t.group(), group);
    again.durable(&t, soon()).unwrap();
    assert_eq!(count(&again, &other), 1);
}

/// Carries `out` and every action and answer after it to `groups` until the driver is idle,
/// writing a row to the next of `keys` before each action, and counting each write acknowledged
/// in `acked` (a write refused while its span is fenced is not acknowledged).
fn drive(
    dir: &std::path::Path,
    groups: &Groups<Events>,
    driver: &mut Driver,
    out: Vec<Out>,
    keys: &[String],
    acked: &mut [i64],
    next: &mut usize,
) -> usize {
    let mut pending: std::collections::VecDeque<Out> = out.into();
    let mut ended = 0;
    while let Some(o) = pending.pop_front() {
        match o {
            Out::Commit(_) => groups.route(driver.directory()).unwrap(),
            Out::Send(a) => {
                let i = *next % keys.len();
                *next += 1;
                match groups.put(vec![row(&keys[i], acked[i])]) {
                    Ok(t) => {
                        groups.durable(&t, soon()).unwrap();
                        acked[i] += 1;
                    }
                    // Fenced for the cutover, or flipped to a group that hasn't taken it up yet:
                    // asked again later, not acknowledged.
                    Err(Refused::Away(_)) => {}
                    Err(e) => panic!("a write refused: {e:?}"),
                }
                let reply = groups
                    .carry(&a, soon(), |g| events(dir, &g.0.to_string()))
                    .unwrap();
                pending.extend(
                    driver
                        .answer(Answer {
                            ask: a.ask,
                            op: a.op,
                            reply,
                        })
                        .unwrap(),
                );
            }
            Out::Ended(e) => {
                assert!(e.done, "operation given up: {e:?}");
                ended += 1;
            }
        }
    }
    assert!(driver.idle());
    ended
}

/// Every key's rows: read through its group, `acked[i]` of them, and in exactly one group's file,
/// the directory's.
fn every_key_in_its_group(groups: &Groups<Events>, keys: &[String], acked: &[i64]) {
    for (i, key) in keys.iter().enumerate() {
        assert_eq!(count(groups, key), acked[i], "{key}'s rows");
    }
    let held = groups
        .read_each(
            soon(),
            false,
            |db| {
                let mut keys = Vec::new();
                db.query(KEYS, [], |r| {
                    keys.push(r.get::<String>(0).map_err(|e| anyhow::anyhow!("{e}"))?);
                    Ok(true)
                })?;
                Ok(keys)
            },
            |_| Ok(Vec::new()),
        )
        .unwrap();
    let mut seen = HashSet::new();
    for keys in &held {
        for key in keys {
            assert!(seen.insert(key.clone()), "{key} in two groups' files");
        }
    }
    assert_eq!(seen.len(), keys.len());
}

/// T-T1.9.R4 (CP-T1.9 step 3; docs/design/reshard.md, "Copy, then catch-up, then a brief
/// cutover"): a database of one group with rows, split by the driver through the shell
/// ([`Groups::carry`]) while writes go on, then merged back; expect the moving range's rows, those
/// there before the split and those written during its copy and catch-up, in the new group, every
/// key's acknowledged rows read back from exactly one group, and after the merge the retired
/// group gone and every row back in the one left.
#[test]
fn t_t1_9_r4_a_split_copies_the_moving_ranges_rows_and_a_merge_brings_them_back() {
    let dir = tempfile::tempdir().unwrap();
    let directory = Directory::new(NodeId(0));
    // Small batches (the test's parameter) so the copy, catch-up and sweep each take several.
    let config = Config {
        commit_rows: 4,
        ..Config::default()
    };
    let groups = Groups::on(&directory, config, |g| events(dir.path(), &g.0.to_string())).unwrap();
    let keys: Vec<String> = (0..64).map(|i| format!("k{i}")).collect();
    let mut acked = vec![0i64; keys.len()];
    for (i, key) in keys.iter().enumerate() {
        let t = groups.put(vec![row(key, 0), row(key, 1)]).unwrap();
        groups.durable(&t, soon()).unwrap();
        acked[i] = 2;
    }
    let mut next = 0;
    let (mut driver, out) = Driver::recover(directory, 1).unwrap();
    assert!(out.is_empty());

    let out = driver
        .begin(&Proposal::Split {
            group: GroupId(0),
            at: 1 << 63,
        })
        .unwrap();
    let ended = drive(
        dir.path(),
        &groups,
        &mut driver,
        out,
        &keys,
        &mut acked,
        &mut next,
    );
    assert_eq!(ended, 1);
    assert_eq!(groups.len(), 2);
    let moved: Vec<&String> = keys
        .iter()
        .filter(|k| groups.group(k) == Some(GroupId(1)))
        .collect();
    assert!(!moved.is_empty() && moved.len() < keys.len());
    let new = groups.holding_of(GroupId(1)).unwrap();
    assert_eq!(new.owned(), Span::new(1 << 63, u64::MAX));
    every_key_in_its_group(&groups, &keys, &acked);
    // Writes went on through the split: some acknowledged past the two rows each key began with.
    assert!(acked.iter().any(|&n| n > 2));

    let out = driver
        .begin(&Proposal::Merge {
            from: GroupId(1),
            into: GroupId(0),
        })
        .unwrap();
    let ended = drive(
        dir.path(),
        &groups,
        &mut driver,
        out,
        &keys,
        &mut acked,
        &mut next,
    );
    assert_eq!(ended, 1);
    assert_eq!(groups.len(), 1);
    assert!(groups.holding_of(GroupId(1)).is_none());
    assert_eq!(
        groups.holding_of(GroupId(0)).unwrap().owned(),
        Some(Span::ALL)
    );
    every_key_in_its_group(&groups, &keys, &acked);
}

/// What a lab run saw: the operations that ended, flipped, and each cutover's pause (from the old
/// group's fence carried out to the new group's taking the span up: the time the span took no
/// writes), and the longest a client's write waited on a refusal before it was acknowledged.
struct LabRun {
    ended: u64,
    flipped: u64,
    pauses: Vec<Duration>,
    longest_refused: Duration,
    acked: u64,
}

fn env(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Groups on one node, split and merged by the planner from what is measured (each group's rows
/// and the writes acknowledged in each window, and this machine's cores), carried out by the shell
/// while `clients` threads write on, until `target` operations have ended. `preload` rows a key
/// are written before the clients start (the partition's size). Every write acknowledged is
/// checked at the end: read back from its key's group, and every key in exactly one group's file.
fn lab_run(keys: usize, preload: usize, clients: usize, target: u64) -> LabRun {
    use amphora::reshard::plan::{GroupLoad, NodeRoom, cut, plan};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    let dir = tempfile::tempdir().unwrap();
    let directory = Directory::new(NodeId(0));
    let groups = Groups::on(&directory, Config::default(), |g| {
        events(dir.path(), &g.0.to_string())
    })
    .unwrap();
    let keys: Arc<Vec<String>> = Arc::new((0..keys).map(|i| format!("k{i}")).collect());
    let acked = Arc::new(Mutex::new(vec![0i64; keys.len()]));
    for chunk in keys.chunks(64) {
        for _ in 0..preload {
            let mut a = acked.lock().unwrap();
            let rows = chunk
                .iter()
                .map(|k| {
                    let i = k[1..].parse::<usize>().unwrap();
                    a[i] += 1;
                    row(k, a[i] - 1)
                })
                .collect::<Vec<_>>();
            // A preload batch spans groups only once there are several: there is one here.
            let t = groups.put_wait(rows, soon()).unwrap();
            groups.durable(&t, soon()).unwrap();
        }
    }
    let window: Arc<Mutex<std::collections::BTreeMap<Pos, u64>>> = Arc::default();
    let stop = Arc::new(AtomicBool::new(false));
    let hot = Arc::new(AtomicUsize::new(0));
    let longest = Arc::new(Mutex::new(Duration::ZERO));
    let total = Arc::new(AtomicUsize::new(0));
    let threads: Vec<_> = (0..clients)
        .map(|t| {
            let (groups, keys, acked, window, stop, hot, longest, total) = (
                groups.clone(),
                Arc::clone(&keys),
                Arc::clone(&acked),
                Arc::clone(&window),
                Arc::clone(&stop),
                Arc::clone(&hot),
                Arc::clone(&longest),
                Arc::clone(&total),
            );
            std::thread::spawn(move || {
                let mut x = 0x9e37_79b9_7f4a_7c15u64 ^ (t as u64 + 1);
                let mine: Vec<usize> = (t..keys.len()).step_by(clients).collect();
                while !stop.load(Ordering::Relaxed) {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    // Most writes hit a run of a sixteenth of the keys that moves now and then.
                    let i = if x % 4 != 0 {
                        let run = (mine.len() / 16).max(1);
                        mine[(hot.load(Ordering::Relaxed) + (x >> 8) as usize % run) % mine.len()]
                    } else {
                        mine[(x >> 8) as usize % mine.len()]
                    };
                    let key = &keys[i];
                    let seq = acked.lock().unwrap()[i];
                    let asked = Instant::now();
                    loop {
                        match groups.put_wait(vec![row(key, seq)], soon()) {
                            Ok(ticket) => {
                                groups.durable(&ticket, soon()).unwrap();
                                break;
                            }
                            Err(Refused::Away(_)) => std::thread::yield_now(),
                            Err(e) => panic!("a write refused: {e:?}"),
                        }
                    }
                    let waited = asked.elapsed();
                    {
                        let mut l = longest.lock().unwrap();
                        *l = (*l).max(waited);
                    }
                    acked.lock().unwrap()[i] += 1;
                    *window.lock().unwrap().entry(position(key)).or_default() += 1;
                    total.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect();

    let cores = std::thread::available_parallelism().map_or(1, |n| n.get() as u64);
    let (mut driver, _) = Driver::recover(directory, 1).unwrap();
    let mut run = LabRun {
        ended: 0,
        flipped: 0,
        pauses: Vec::new(),
        longest_refused: Duration::ZERO,
        acked: 0,
    };
    let mut fenced: std::collections::HashMap<OpId, Instant> = std::collections::HashMap::new();
    let mut planned_at = 0;
    while run.ended < target {
        // A window: at least one write a key since the last plan (the test's parameter).
        let seen = total.load(Ordering::Relaxed);
        if seen < planned_at + keys.len() {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        planned_at = seen;
        let dirc = driver.directory().clone();
        let w: Vec<(Pos, u64)> = std::mem::take(&mut *window.lock().unwrap())
            .into_iter()
            .collect();
        let mut loads = Vec::new();
        for (g, _) in dirc.groups() {
            let Some(span) = groups.holding_of(g).and_then(|h| h.owned()) else {
                continue;
            };
            let rows: Vec<(Pos, u64)> = keys
                .iter()
                .map(|k| position(k))
                .filter(|p| span.contains(*p))
                .map(|p| (p, 1))
                .collect();
            let load: Vec<(Pos, u64)> = w
                .iter()
                .copied()
                .filter(|(p, _)| span.contains(*p))
                .collect();
            loads.push(GroupLoad {
                group: g,
                bytes: rows.len() as u64,
                load: load.iter().map(|(_, c)| c).sum(),
                by_size: cut(&rows),
                by_load: cut(&load),
            });
        }
        let proposals = plan(
            &dirc,
            &[NodeRoom {
                node: NodeId(0),
                cores,
            }],
            &loads,
        );
        if proposals.is_empty() {
            hot.fetch_add(keys.len() / 7 + 1, Ordering::Relaxed);
        }
        for p in proposals {
            let Ok(out) = driver.begin(&p) else { continue };
            let mut pending: std::collections::VecDeque<Out> = out.into();
            while let Some(o) = pending.pop_front() {
                match o {
                    Out::Commit(_) => groups.route(driver.directory()).unwrap(),
                    Out::Send(a) => {
                        let reply = groups
                            .carry(&a, soon(), |g| events(dir.path(), &g.0.to_string()))
                            .unwrap();
                        match (&a.what, &reply) {
                            (amphora::reshard::driver::Do::Fence { .. }, _) => {
                                fenced.entry(a.op).or_insert_with(Instant::now);
                            }
                            (_, amphora::reshard::driver::Reply::Activated) => {
                                if let Some(at) = fenced.remove(&a.op) {
                                    run.pauses.push(at.elapsed());
                                }
                            }
                            _ => {}
                        }
                        pending.extend(
                            driver
                                .answer(Answer {
                                    ask: a.ask,
                                    op: a.op,
                                    reply,
                                })
                                .unwrap(),
                        );
                    }
                    Out::Ended(e) => {
                        run.ended += 1;
                        run.flipped += u64::from(e.done);
                    }
                }
            }
        }
    }
    stop.store(true, Ordering::Relaxed);
    for t in threads {
        t.join().unwrap();
    }
    groups.sync(soon()).unwrap();
    let acked = acked.lock().unwrap().clone();
    every_key_in_its_group(&groups, &keys, &acked);
    run.acked = acked.iter().map(|&n| u64::try_from(n).unwrap()).sum();
    run.longest_refused = *longest.lock().unwrap();
    run
}

fn p99(mut d: Vec<Duration>) -> Duration {
    d.sort_unstable();
    d.get(d.len().saturating_sub(1) * 99 / 100)
        .copied()
        .unwrap_or_default()
}

/// The cutover-pause harness (AC-T1.9.2): one group of `AMPHORA_RESHARD_PAUSE_ROWS` rows (a tiny
/// partition by default; the coordinator's run sizes it to 10 GB) split by the planner while
/// clients write on; prints each cutover's pause and the longest a write waited on a refusal.
/// Expect every acknowledged write read back from exactly one group; the pause's bound is judged
/// from the printed numbers, not asserted here (tests assert behaviour, not the clock).
#[test]
fn t_t1_9_p1_cutover_pause_harness() {
    let keys = env("AMPHORA_RESHARD_PAUSE_KEYS", 256) as usize;
    let rows = env("AMPHORA_RESHARD_PAUSE_ROWS", 1024) as usize;
    let ops = env("AMPHORA_RESHARD_PAUSE_OPS", 1);
    let clients = env("AMPHORA_RESHARD_CLIENTS", 4) as usize;
    let run = lab_run(keys, rows.div_ceil(keys.max(1)), clients, ops);
    assert!(run.flipped >= 1, "no cutover (non-vacuity)");
    println!(
        "cutover pause: {} operations, {} flipped, pause p99 {:?} max {:?}, longest write refused {:?}, {} writes acknowledged",
        run.ended,
        run.flipped,
        p99(run.pauses.clone()),
        run.pauses.iter().max().copied().unwrap_or_default(),
        run.longest_refused,
        run.acked,
    );
}

/// The lab harness (AC-T1.9.3): `AMPHORA_RESHARD_OPS` automatic operations (a few by default;
/// the coordinator's run takes 1,000), each chosen by the planner from what is measured and
/// carried out while clients write on; expect writes lost exactly 0: every acknowledged write
/// read back from its key's group, and every key in exactly one group's file.
#[test]
fn t_t1_9_l1_automatic_operations_lose_no_write() {
    let keys = env("AMPHORA_RESHARD_KEYS", 256) as usize;
    let ops = env("AMPHORA_RESHARD_OPS", 4);
    let clients = env("AMPHORA_RESHARD_CLIENTS", 4) as usize;
    let run = lab_run(keys, 1, clients, ops);
    assert!(run.ended >= ops);
    println!(
        "lab: {} operations, {} flipped, {} writes acknowledged, 0 lost, pause p99 {:?}",
        run.ended,
        run.flipped,
        run.acked,
        p99(run.pauses.clone()),
    );
}
