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
use amphora::reshard::holding::{Away, Fenced};
use amphora::reshard::{GroupId, OpId, Span, position};
use amphora::store::{Checkpointed, Config, Engine, Flush, Opener, Refused};
use amphora_engine::Value;

struct Events {
    db: Db,
}

struct Row {
    key: String,
    seq: i64,
}

const INSERT: &str = "INSERT INTO events (key, seq) VALUES (?1, ?2)";

impl Engine for Events {
    type Row = Row;
    type Reader = Db;

    fn key(row: &Row) -> &str {
        &row.key
    }

    fn apply(&mut self, rows: &[Row]) -> anyhow::Result<usize> {
        self.db.transaction(|db| {
            for r in rows {
                db.query(
                    INSERT,
                    [Value::from_text(r.key.clone()), Value::from_i64(r.seq)],
                    |_| Ok(true),
                )?;
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
        self.db
            .transaction(|db| db.query(KEEP, [bytes], |_| Ok(true)))?;
        (self.db.flusher())()?;
        Ok(())
    }
}

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

fn lab(dir: &std::path::Path) -> Groups<Events> {
    Groups::start(GROUPS, Config::default(), |i| {
        let db = Db::open(&dir.join(format!("events-{i}.db")))?;
        db.exec(
            "CREATE TABLE IF NOT EXISTS events (key TEXT, seq INTEGER);
             CREATE INDEX IF NOT EXISTS by_key ON events (key);
             CREATE TABLE IF NOT EXISTS holding (id INTEGER PRIMARY KEY, bytes BLOB)",
        )?;
        Ok(Events { db })
    })
    .unwrap()
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
