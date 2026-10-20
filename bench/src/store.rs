//! Amphora: kiln's conversation schema as an engine on `amphora::store` (one writer with a group
//! commit, durable by its flusher, readers of their own). A write is handed to the store and waited
//! for until it is durable; a read takes the session's latest page whole.

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use amphora::store::{Config, Engine, Flush, Opener, Store};
use amphora::turso::Db;
use turso_core::Value;

use crate::schema;
use crate::{Driver, Event};

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

    fn apply(&mut self, rows: Vec<Row>) -> anyhow::Result<usize> {
        let n = rows.len();
        let mut rev = self.rev;
        self.db.transaction(|db| {
            for row in &rows {
                rev += 1;
                match row {
                    Row::Session(id) => {
                        let params = [t(id), t("{}"), Value::Null, Value::Null, Value::Null, Value::from_i64(0), Value::from_i64(0), Value::from_i64(rev)];
                        db.query(schema::SESSION_UPSERT, &params, |_| Ok(true))?;
                    }
                    Row::Event(e) => {
                        let params = [
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
                        ];
                        db.query(schema::EVENT_UPSERT, &params, |_| Ok(true))?;
                    }
                }
            }
            // The store's number, kept with its rows (kiln's store keeps its own so).
            db.query(schema::META_SET, &[t("rev"), Value::from_text(rev.to_string())], |_| Ok(true))?;
            Ok(())
        })?;
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

    fn checkpoint(reader: &mut Db) -> anyhow::Result<bool> {
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
    db.query(schema::EVENTS_LAST, &params, |r| {
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
    db.query(schema::SEARCH, &params, |_| {
        n += 1;
        Ok(true)
    })?;
    Ok(n)
}

const WAIT: Duration = Duration::from_secs(60);

pub struct Amphora {
    store: Store<Convo>,
    seen: Mutex<HashSet<String>>,
}

impl Amphora {
    pub fn open(dir: &str) -> anyhow::Result<Self> {
        let dir = std::path::Path::new(dir);
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir)?;
        let db = Db::open(&dir.join("bench.db"))?;
        for s in schema::SCHEMA {
            db.exec(s)?;
        }
        // AMPHORA_FLUSHERS: flusher threads (the store's default when unset).
        let flushers = std::env::var("AMPHORA_FLUSHERS").ok().and_then(|v| v.parse().ok()).unwrap_or(Config::default().flushers);
        let env = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<i64>().ok());
        // AMPHORA_WRITER_PAGES: pages the writer keeps (Amphora's default when unset).
        if let Some(p) = env("AMPHORA_WRITER_PAGES") {
            db.keep_pages(p)?;
        }
        let store = Store::start(Convo { db, rev: 0, reader_pages: env("AMPHORA_READER_PAGES") }, Config { flushers, ..Config::default() })?;
        Ok(Self { store, seen: Mutex::new(HashSet::new()) })
    }
}

impl Driver for Amphora {
    fn write(&self, events: &[Event]) -> anyhow::Result<()> {
        let mut rows = Vec::with_capacity(events.len() + 1);
        {
            let mut seen = self.seen.lock().map_err(|_| anyhow::anyhow!("poisoned"))?;
            for e in events {
                if seen.insert(e.session.clone()) {
                    rows.push(Row::Session(e.session.clone()));
                }
            }
        }
        rows.extend(events.iter().cloned().map(Row::Event));
        self.store.put_wait(rows, Instant::now() + WAIT)?;
        // Durable: flushed after its commit.
        self.store.sync(Instant::now() + WAIT)?;
        Ok(())
    }

    fn history(&self, session: &str, limit: usize) -> anyhow::Result<usize> {
        let (s, w) = (session.to_owned(), session.to_owned());
        self.store.read(Some(session), Instant::now() + WAIT, move |db| page(db, &s, limit), move |e| page(&mut e.db, &w, limit))
    }

    fn search(&self, word: &str, limit: usize) -> anyhow::Result<usize> {
        let (s, w) = (word.to_owned(), word.to_owned());
        self.store.read(None, Instant::now() + WAIT, move |db| search_on(db, &s, limit), move |e| search_on(&mut e.db, &w, limit))
    }

    fn stats(&self) -> serde_json::Value {
        let c = self.store.counters();
        serde_json::json!({
            "commits": c.commits, "rows": c.rows, "busy_ms": c.busy_ms, "apply_ms": c.apply_ms,
            "flushes": c.flushes, "flush_ms": c.flush_ms, "dropped": c.dropped, "timeouts": c.timeouts,
        })
    }
}
