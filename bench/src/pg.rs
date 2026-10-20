//! Servers speaking Postgres's protocol, on kiln's conversation schema and statements
//! (`crate::schema`, in Postgres's dialect), each as durable as PostgreSQL ships: a write is one
//! transaction (the sessions it is the first write of upserted in it, as `vanilla.rs` does),
//! acknowledged once its commit is flushed.
//! - **PostgreSQL** (servers/postgres.sh);
//! - **OrioleDB**: PostgreSQL with every table in OrioleDB's access method (`USING orioledb`), its
//!   row-level log written into PostgreSQL's and flushed at commit the same way
//!   (servers/orioledb.sh);
//! - **CedarDB**: on a drive with a volatile write cache, as most are, it acknowledges a commit
//!   before flushing it unless told otherwise (its docs, "Write Caching"), so each connection sets
//!   `async_commit = off` (servers/cedardb.sh).
//!
//! Every run starts from empty tables, dropped and made again (the events' ids repeat from run to
//! run). A client a pool thread at most, kept in a pool as `vanilla.rs` keeps its connections, each
//! with the workload's statements prepared on it.

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use postgres::{Client, NoTls, Statement};

use crate::Event;
use crate::pool::Blocking;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Postgres,
    Orioledb,
    Cedardb,
}

/// `crate::schema::SCHEMA` in Postgres's dialect: SQLite's INTEGER (64 bits) as BIGINT, `by`
/// quoted (a keyword in Postgres), and text compared by bytes (`COLLATE "C"`, as SQLite's, Turso's
/// and Amphora's BINARY does, where each image's default compares by its locale's rules, slower).
/// CedarDB refuses the clause: its tables take its own default.
const TABLES: [&str; 3] = [
    "CREATE TABLE meta (key TEXT COLLATE \"C\" PRIMARY KEY, value TEXT NOT NULL)",
    "CREATE TABLE sessions (id TEXT COLLATE \"C\" PRIMARY KEY, info TEXT, meta TEXT, state TEXT, changes TEXT, updated_at BIGINT NOT NULL DEFAULT 0, closed_at BIGINT, epoch BIGINT NOT NULL DEFAULT 0, rev BIGINT NOT NULL DEFAULT 0, machine TEXT)",
    "CREATE TABLE events (session TEXT COLLATE \"C\" NOT NULL, id TEXT COLLATE \"C\" NOT NULL, seq BIGINT NOT NULL, at BIGINT NOT NULL, parent TEXT, \"by\" TEXT, kind TEXT NOT NULL, level TEXT, tool TEXT, status TEXT, text TEXT COLLATE \"C\" NOT NULL, body TEXT NOT NULL, rev BIGINT NOT NULL DEFAULT 0, PRIMARY KEY (session, id))",
];

const INDEXES: [&str; 4] = [
    "CREATE INDEX events_by_seq ON events (session, seq)",
    "CREATE INDEX events_by_time ON events (at)",
    "CREATE INDEX sessions_by_rev ON sessions (rev)",
    "CREATE INDEX events_by_rev ON events (rev)",
];

const SESSION_UPSERT: &str = "INSERT INTO sessions (id, info, meta, state, changes, updated_at, epoch, rev) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
    ON CONFLICT (id) DO UPDATE SET
        info = COALESCE(EXCLUDED.info, sessions.info),
        meta = COALESCE(EXCLUDED.meta, sessions.meta),
        state = COALESCE(EXCLUDED.state, sessions.state),
        changes = COALESCE(EXCLUDED.changes, sessions.changes),
        updated_at = EXCLUDED.updated_at,
        epoch = GREATEST(EXCLUDED.epoch, sessions.epoch),
        rev = EXCLUDED.rev";

const EVENT_UPSERT: &str = "INSERT INTO events (session, id, seq, at, parent, \"by\", kind, level, tool, status, text, body, rev) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
    ON CONFLICT (session, id) DO UPDATE SET
        seq = EXCLUDED.seq, at = EXCLUDED.at, parent = EXCLUDED.parent, \"by\" = EXCLUDED.\"by\", kind = EXCLUDED.kind,
        level = EXCLUDED.level, tool = EXCLUDED.tool, status = EXCLUDED.status, text = EXCLUDED.text, body = EXCLUDED.body,
        rev = EXCLUDED.rev";

const EVENTS_LAST: &str = "SELECT session, id, seq, at, parent, \"by\", kind, level, tool, status, text, body FROM events WHERE session = $1 ORDER BY seq DESC LIMIT $2";

const SEARCH: &str = "SELECT session, id FROM events WHERE text LIKE $1 ORDER BY at DESC LIMIT $2";

/// A client with the workload's statements prepared on it.
struct Conn {
    client: Client,
    session_upsert: Statement,
    event_upsert: Statement,
    last: Statement,
    search: Statement,
}

pub struct Pg {
    dsn: String,
    engine: Engine,
    pool: Mutex<Vec<Conn>>,
    rev: AtomicU64,
    seen: Mutex<HashSet<String>>,
}

impl Pg {
    pub fn connect(dsn: &str, engine: Engine) -> anyhow::Result<Self> {
        let pg = Self { dsn: dsn.to_owned(), engine, pool: Mutex::new(Vec::new()), rev: AtomicU64::new(0), seen: Mutex::new(HashSet::new()) };
        let mut client = pg.client()?;
        if engine == Engine::Orioledb {
            client.batch_execute("CREATE EXTENSION IF NOT EXISTS orioledb")?;
        }
        for table in ["events", "sessions", "meta"] {
            client.batch_execute(&format!("DROP TABLE IF EXISTS {table}"))?;
        }
        let using = if engine == Engine::Orioledb { " USING orioledb" } else { "" };
        for table in TABLES {
            let table = if engine == Engine::Cedardb { table.replace(" COLLATE \"C\"", "") } else { table.to_owned() };
            client.batch_execute(&format!("{table}{using}")).map_err(|e| anyhow::anyhow!("{e} (in: {table}{using})"))?;
        }
        for index in INDEXES {
            client.batch_execute(index).map_err(|e| anyhow::anyhow!("{e} (in: {index})"))?;
        }
        let conn = Self::prepare(client)?;
        pg.give(conn);
        Ok(pg)
    }

    /// A new client, as durable as the module's docs say.
    fn client(&self) -> anyhow::Result<Client> {
        let mut client = Client::connect(&self.dsn, NoTls)?;
        if self.engine == Engine::Cedardb {
            client.batch_execute("SET async_commit = off")?;
        }
        Ok(client)
    }

    fn prepare(mut client: Client) -> anyhow::Result<Conn> {
        Ok(Conn { session_upsert: client.prepare(SESSION_UPSERT)?, event_upsert: client.prepare(EVENT_UPSERT)?, last: client.prepare(EVENTS_LAST)?, search: client.prepare(SEARCH)?, client })
    }

    fn connection(&self) -> anyhow::Result<Conn> {
        if let Some(c) = self.pool.lock().map_err(|_| anyhow::anyhow!("poisoned"))?.pop() {
            return Ok(c);
        }
        Self::prepare(self.client()?)
    }

    /// Back to the pool, unless its connection is gone.
    fn give(&self, c: Conn) {
        if c.client.is_closed() {
            return;
        }
        if let Ok(mut pool) = self.pool.lock() {
            pool.push(c);
        }
    }
}

impl Blocking for Pg {
    fn write(&self, events: &[Event]) -> anyhow::Result<()> {
        let new: Vec<String> = {
            let mut seen = self.seen.lock().map_err(|_| anyhow::anyhow!("poisoned"))?;
            events.iter().filter(|e| seen.insert(e.session.clone())).map(|e| e.session.clone()).collect()
        };
        let mut c = self.connection()?;
        let done = (|| -> anyhow::Result<()> {
            let mut tx = c.client.transaction()?;
            for s in &new {
                let rev = i64::try_from(self.rev.fetch_add(1, Ordering::Relaxed) + 1)?;
                tx.execute(&c.session_upsert, &[s, &"{}", &None::<&str>, &None::<&str>, &None::<&str>, &0i64, &0i64, &rev])?;
            }
            for e in events {
                let rev = i64::try_from(self.rev.fetch_add(1, Ordering::Relaxed) + 1)?;
                let seq = i64::try_from(e.seq)?;
                tx.execute(&c.event_upsert, &[&e.session, &e.id, &seq, &e.at, &None::<&str>, &None::<&str>, &e.kind, &None::<&str>, &None::<&str>, &None::<&str>, &e.text, &e.body, &rev])?;
            }
            tx.commit()?;
            Ok(())
        })();
        self.give(c);
        done
    }

    fn history(&self, session: &str, limit: usize) -> anyhow::Result<usize> {
        let mut c = self.connection()?;
        let done = (|| -> anyhow::Result<usize> {
            let rows = c.client.query(&c.last, &[&session, &i64::try_from(limit)?])?;
            // Every column taken, as a reader of the page would (the store builds its rows).
            for r in &rows {
                let row: (String, String, i64, i64, Option<String>, Option<String>, String, Option<String>, Option<String>, Option<String>, String, String) =
                    (r.try_get(0)?, r.try_get(1)?, r.try_get(2)?, r.try_get(3)?, r.try_get(4)?, r.try_get(5)?, r.try_get(6)?, r.try_get(7)?, r.try_get(8)?, r.try_get(9)?, r.try_get(10)?, r.try_get(11)?);
                std::hint::black_box(row);
            }
            Ok(rows.len())
        })();
        self.give(c);
        done
    }

    fn search(&self, word: &str, limit: usize) -> anyhow::Result<usize> {
        let mut c = self.connection()?;
        let done = (|| -> anyhow::Result<usize> { Ok(c.client.query(&c.search, &[&format!("%{word}%"), &i64::try_from(limit)?])?.len()) })();
        self.give(c);
        done
    }
}
