//! Amphora: kiln's conversation schema as an engine on `amphora::store` (one writer with a group
//! commit, durable by its flusher, readers of their own). A write is handed to the store and waited
//! for until it is durable; a read takes the session's latest page whole.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use amphora::groups::Groups;
use amphora::store::{Checkpointed, Config, Engine, Flush, Opener};
use amphora::db::Db;
use amphora_engine::Value;

use crate::schema;
use crate::{Done, Driver, Event};

pub enum Row {
    Session(String),
    Event(Event),
}

pub struct Convo {
    db: Db,
    rev: i64,
    /// AMPHORA_READER_PAGES: pages each reader keeps (Amphora's default when unset).
    reader_pages: Option<i64>,
}

/// The slowest COMMIT and commit, in µs (where a slow commit waits); the first is unused.
static SLOWEST: (AtomicU64, AtomicU64, AtomicU64) = (AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0));

fn one_row() -> bool {
    static ONE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ONE.get_or_init(|| std::env::var_os("AMPHORA_ONE_ROW").is_some())
}

/// How many events one statement takes, largest first.
const CHUNKS: [usize; 4] = [64, 16, 4, 1];

/// The upsert of `rows` events in one statement (one of [`CHUNKS`]).
fn event_upsert(rows: usize) -> &'static str {
    static SQL: std::sync::OnceLock<Vec<(usize, String)>> = std::sync::OnceLock::new();
    let all = SQL.get_or_init(|| CHUNKS.iter().map(|&n| (n, schema::event_upsert_rows(n))).collect());
    all.iter().find(|(n, _)| *n == rows).map_or(schema::EVENT_UPSERT, |(_, sql)| sql.as_str())
}

/// An event's thirteen columns, in the upsert's order.
fn event_params(e: &Event, rev: i64) -> [Value; 13] {
    [
        t(&e.session),
        t(&e.id),
        Value::from_i64(i64::try_from(e.seq).unwrap_or(i64::MAX)),
        Value::from_i64(e.at),
        Value::Null,
        Value::Null,
        t(e.kind),
        Value::Null,
        Value::Null,
        Value::Null,
        t(&e.text),
        t(&e.body),
        Value::from_i64(rev),
    ]
}

fn t(s: &str) -> Value {
    Value::from_text(s.to_owned())
}

impl Engine for Convo {
    type Row = Row;
    type Reader = Db;

    fn key(row: &Row) -> &str {
        match row {
            Row::Session(id) => id,
            Row::Event(e) => &e.session,
        }
    }

    fn apply(&mut self, rows: &[Row]) -> anyhow::Result<usize> {
        let n = rows.len();
        let began = Instant::now();
        // Each row its number, in order, as one statement a row would give it.
        let first = self.rev;
        let mut events: Vec<(&Event, i64)> = Vec::with_capacity(n);
        let applied = self.db.transaction(|db| {
            let mut rev = first;
            for row in rows {
                rev += 1;
                match row {
                    Row::Session(id) => {
                        let params = [t(id), t("{}"), Value::Null, Value::Null, Value::Null, Value::from_i64(0), Value::from_i64(0), Value::from_i64(rev)];
                        db.query(schema::SESSION_UPSERT, params, |_| Ok(true))?;
                    }
                    Row::Event(e) => events.push((e, rev)),
                }
            }
            // The events many to a statement: 64, 16, 4 or 1 at a time.
            let mut at = 0;
            while at < events.len() {
                // AMPHORA_ONE_ROW: a statement a row (to compare).
                let take = if one_row() { 1 } else { CHUNKS.iter().copied().find(|&c| c <= events.len() - at).unwrap_or(1) };
                let params: Vec<Value> = events[at..at + take].iter().flat_map(|(e, rev)| event_params(e, *rev)).collect();
                db.query(event_upsert(take), params, |_| Ok(true))?;
                at += take;
            }
            // The store's number, kept with its rows (kiln's store keeps its own so).
            db.query(schema::META_SET, [t("rev"), Value::from_text(rev.to_string())], |_| Ok(true))?;
            Ok((Instant::now(), rev))
        });
        let (rows_done, rev) = applied?;
        // Where a slow commit spent its time: its COMMIT, and all of it.
        SLOWEST.1.fetch_max(u64::try_from(rows_done.elapsed().as_micros()).unwrap_or(u64::MAX), Ordering::Relaxed);
        SLOWEST.2.fetch_max(u64::try_from(began.elapsed().as_micros()).unwrap_or(u64::MAX), Ordering::Relaxed);
        self.rev = rev;
        Ok(n)
    }

    fn flusher(&self) -> Option<Flush> {
        Some(self.db.flusher())
    }

    fn opener(&self) -> Option<Opener<Db>> {
        let (open, pages) = (self.db.opener(), self.reader_pages);
        Some(std::sync::Arc::new(move || {
            let r = open()?;
            if let Some(p) = pages {
                r.keep_pages(p)?;
            }
            Ok(r)
        }))
    }

    fn checkpoint(reader: &mut Db) -> anyhow::Result<Checkpointed> {
        reader.checkpoint()
    }

    fn restart_log(&mut self) -> anyhow::Result<bool> {
        self.db.restart_log()
    }
}

/// A session's latest page, every column taken.
fn page(db: &mut Db, session: &str, limit: usize) -> anyhow::Result<usize> {
    let mut n = 0;
    let params = [t(session), Value::from_i64(i64::try_from(limit).unwrap_or(i64::MAX))];
    db.query(schema::EVENTS_LAST, params, |r| {
        let row: Vec<Value> = (0..schema::EVENT_COLUMNS).map(|i| r.get_value(i).clone()).collect();
        std::hint::black_box(row);
        n += 1;
        Ok(true)
    })?;
    Ok(n)
}

fn search_on(db: &mut Db, word: &str, limit: usize) -> anyhow::Result<usize> {
    let mut n = 0;
    let params = [Value::from_text(format!("%{word}%")), Value::from_i64(i64::try_from(limit).unwrap_or(i64::MAX))];
    db.query(schema::SEARCH, params, |_| {
        n += 1;
        Ok(true)
    })?;
    Ok(n)
}

const WAIT: Duration = Duration::from_secs(60);

pub struct Amphora {
    /// AMPHORA_GROUPS: the groups sessions are spread over (1 when unset), each a database file, a
    /// writer and a flusher of its own.
    groups: Groups<Convo>,
    seen: Mutex<HashSet<String>>,
}

impl Amphora {
    pub fn open(dir: &str) -> anyhow::Result<Self> {
        let dir = std::path::Path::new(dir);
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir)?;
        let env = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<i64>().ok());
        let groups = env("AMPHORA_GROUPS").and_then(|g| usize::try_from(g).ok()).unwrap_or(1);
        let defaults = Config::default();
        let config = Config {
            // AMPHORA_FLUSHERS: flusher threads a group.
            flushers: env("AMPHORA_FLUSHERS").and_then(|f| usize::try_from(f).ok()).unwrap_or(defaults.flushers),
            // AMPHORA_CHECKPOINT_MS: how often a checkpointer copies its log.
            checkpoint_every: env("AMPHORA_CHECKPOINT_MS").and_then(|ms| u64::try_from(ms).ok()).map_or(defaults.checkpoint_every, Duration::from_millis),
            // AMPHORA_LOG_FRAMES: a log's bound in frames.
            log_frames: env("AMPHORA_LOG_FRAMES").and_then(|f| u64::try_from(f).ok()).unwrap_or(defaults.log_frames),
            // AMPHORA_COMMIT_ROWS: rows that make a commit at once.
            commit_rows: env("AMPHORA_COMMIT_ROWS").and_then(|r| usize::try_from(r).ok()).unwrap_or(defaults.commit_rows),
            // AMPHORA_QUEUE: batches a group's writer queues at most.
            queue: env("AMPHORA_QUEUE").and_then(|q| usize::try_from(q).ok()).unwrap_or(defaults.queue),
            ..defaults
        };
        // AMPHORA_PACK_LOG=1: packed logs (docs/design/store.md, decisions 16 and 17).
        let pack_log = env("AMPHORA_PACK_LOG") == Some(1);
        let groups = Groups::start(groups, config, |g| {
            let db = Db::open(&dir.join(format!("bench-{g}.db")))?;
            if pack_log {
                db.pack_log(true)?;
            }
            for s in schema::SCHEMA {
                db.exec(s)?;
            }
            // AMPHORA_WRITER_PAGES: pages a writer keeps (Amphora's default when unset).
            if let Some(p) = env("AMPHORA_WRITER_PAGES") {
                db.keep_pages(p)?;
            }
            Ok(Convo { db, rev: 0, reader_pages: env("AMPHORA_READER_PAGES") })
        })?;
        Ok(Self { groups, seen: Mutex::new(HashSet::new()) })
    }
}

impl Driver for Amphora {
    fn write(&self, events: Vec<Event>, done: Done) -> anyhow::Result<()> {
        let mut rows = Vec::with_capacity(events.len() + 1);
        {
            let mut seen = self.seen.lock().map_err(|_| anyhow::anyhow!("poisoned"))?;
            for e in &events {
                if seen.insert(e.session.clone()) {
                    rows.push(Row::Session(e.session.clone()));
                }
            }
        }
        rows.extend(events.into_iter().map(Row::Event));
        // Told once durable: committed, then flushed by a flush begun after its commit.
        // A write is one session's events: one group's batch.
        self.groups.put_then(rows, Instant::now() + WAIT, move |answer| done(answer.map_err(anyhow::Error::from)))?;
        Ok(())
    }

    fn history(&self, session: &str, limit: usize) -> anyhow::Result<usize> {
        let (s, w) = (session.to_owned(), session.to_owned());
        // What is committed now: the bench's readers aren't the writers of what they read, and read
        // as any database's readers do, waiting for no one's writes.
        self.groups.read_committed(session, Instant::now() + WAIT, move |db| page(db, &s, limit), move |e| page(&mut e.db, &w, limit))
    }

    fn search(&self, word: &str, limit: usize) -> anyhow::Result<usize> {
        let (s, w) = (word.to_owned(), word.to_owned());
        let each = self.groups.read_each(Instant::now() + WAIT, true, move |db| search_on(db, &s, limit), move |e| search_on(&mut e.db, &w, limit))?;
        Ok(each.into_iter().sum::<usize>().min(limit))
    }

    fn stats(&self) -> serde_json::Value {
        let mut all = amphora::meter::Counters::default();
        let mut longest = vec![0u64; 5];
        for c in self.groups.counters() {
            all.commits += c.commits;
            all.rows += c.rows;
            all.busy_ms = all.busy_ms.max(c.busy_ms);
            all.apply_ms += c.apply_ms;
            all.flushes += c.flushes;
            all.flush_ms += c.flush_ms;
            all.dropped += c.dropped;
            all.failed += c.failed;
            all.timeouts += c.timeouts;
            all.checkpoints += c.checkpoints;
            all.checkpoint_ms += c.checkpoint_ms;
            all.restarts += c.restarts;
            all.restart_wait_ms += c.restart_wait_ms;
            all.restart_ms += c.restart_ms;
            for (l, x) in longest.iter_mut().zip(&c.longest_us) {
                *l = (*l).max(*x);
            }
        }
        let c = all;
        serde_json::json!({
            "groups": self.groups.len(),
            "commits": c.commits, "rows": c.rows, "busy_ms": c.busy_ms, "apply_ms": c.apply_ms,
            "flushes": c.flushes, "flush_ms": c.flush_ms, "dropped": c.dropped, "failed": c.failed, "timeouts": c.timeouts,
            "checkpoints": c.checkpoints, "checkpoint_ms": c.checkpoint_ms, "restarts": c.restarts, "restart_wait_ms": c.restart_wait_ms,
            "restart_ms": c.restart_ms, "longest_us": longest,
            "slowest_commit_stmt_us": SLOWEST.1.load(Ordering::Relaxed),
            "slowest_apply_us": SLOWEST.2.load(Ordering::Relaxed),
        })
    }
}
