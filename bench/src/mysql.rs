//! MySQL (InnoDB), on kiln's conversation schema and statements (`crate::schema`, in MySQL's
//! dialect), as durable as it ships (servers/mysql.sh: the redo log and the binary log flushed at
//! every commit): a write is one transaction (the sessions it is the first write of upserted in it,
//! as `vanilla.rs` and `pg.rs` do), reads take every column. Text compares by bytes (`utf8mb4_bin`),
//! as SQLite's, Turso's and Amphora's BINARY does. `vitess.rs` speaks to Vitess on its own keyspace's
//! narrower table.
//!
//! Every run starts from empty tables, dropped and made again.

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use mysql::prelude::Queryable;
use mysql::{Opts, Pool, PoolConstraints, PoolOpts, TxOpts};

use crate::Event;
use crate::pool::Blocking;

/// `crate::schema::SCHEMA` in MySQL's dialect: keys as VARCHAR (a key can't be TEXT), `key` and
/// `by` quoted (keywords), BIGINT for SQLite's 64-bit INTEGER.
const TABLES: [&str; 3] = [
    "CREATE TABLE meta (`key` VARCHAR(64) PRIMARY KEY, value TEXT NOT NULL) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_bin",
    "CREATE TABLE sessions (id VARCHAR(64) PRIMARY KEY, info TEXT, meta TEXT, state TEXT, changes TEXT, updated_at BIGINT NOT NULL DEFAULT 0, closed_at BIGINT, epoch BIGINT NOT NULL DEFAULT 0, rev BIGINT NOT NULL DEFAULT 0, machine TEXT, KEY sessions_by_rev (rev)) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_bin",
    "CREATE TABLE events (session VARCHAR(64) NOT NULL, id VARCHAR(64) NOT NULL, seq BIGINT NOT NULL, at BIGINT NOT NULL, parent TEXT, `by` TEXT, kind VARCHAR(16) NOT NULL, level TEXT, tool TEXT, status TEXT, text TEXT NOT NULL, body MEDIUMTEXT NOT NULL, rev BIGINT NOT NULL DEFAULT 0, PRIMARY KEY (session, id), KEY events_by_seq (session, seq), KEY events_by_time (at), KEY events_by_rev (rev)) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_bin",
];

const SESSION_UPSERT: &str = "INSERT INTO sessions (id, info, meta, state, changes, updated_at, epoch, rev) VALUES (?, ?, ?, ?, ?, ?, ?, ?) AS new
    ON DUPLICATE KEY UPDATE
        info = COALESCE(new.info, sessions.info), meta = COALESCE(new.meta, sessions.meta),
        state = COALESCE(new.state, sessions.state), changes = COALESCE(new.changes, sessions.changes),
        updated_at = new.updated_at, epoch = GREATEST(new.epoch, sessions.epoch), rev = new.rev";

const EVENT_UPSERT: &str = "INSERT INTO events (session, id, seq, at, parent, `by`, kind, level, tool, status, text, body, rev) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) AS new
    ON DUPLICATE KEY UPDATE
        seq = new.seq, at = new.at, parent = new.parent, `by` = new.`by`, kind = new.kind, level = new.level,
        tool = new.tool, status = new.status, text = new.text, body = new.body, rev = new.rev";

const EVENTS_LAST: &str = "SELECT session, id, seq, at, parent, `by`, kind, level, tool, status, text, body FROM events WHERE session = ? ORDER BY seq DESC LIMIT ?";

const SEARCH: &str = "SELECT session, id FROM events WHERE text LIKE ? ORDER BY at DESC LIMIT ?";

pub struct MySql {
    pool: Pool,
    rev: AtomicU64,
    seen: Mutex<HashSet<String>>,
}

impl MySql {
    pub fn connect(dsn: &str, connections: usize) -> anyhow::Result<Self> {
        let constraints = PoolConstraints::new(connections.min(8), connections.max(8)).ok_or_else(|| anyhow::anyhow!("bad pool size"))?;
        let opts = mysql::OptsBuilder::from_opts(Opts::from_url(dsn)?).pool_opts(PoolOpts::default().with_constraints(constraints));
        let pool = Pool::new(opts)?;
        let mut conn = pool.get_conn()?;
        for table in ["events", "sessions", "meta"] {
            conn.query_drop(format!("DROP TABLE IF EXISTS {table}"))?;
        }
        for table in TABLES {
            conn.query_drop(table).map_err(|e| anyhow::anyhow!("{e} (in: {table})"))?;
        }
        Ok(Self { pool, rev: AtomicU64::new(0), seen: Mutex::new(HashSet::new()) })
    }
}

impl Blocking for MySql {
    fn write(&self, events: &[Event]) -> anyhow::Result<()> {
        let new: Vec<String> = {
            let mut seen = self.seen.lock().map_err(|_| anyhow::anyhow!("poisoned"))?;
            events.iter().filter(|e| seen.insert(e.session.clone())).map(|e| e.session.clone()).collect()
        };
        let mut conn = self.pool.get_conn()?;
        let mut tx = conn.start_transaction(TxOpts::default())?;
        for s in &new {
            let rev = self.rev.fetch_add(1, Ordering::Relaxed) + 1;
            tx.exec_drop(SESSION_UPSERT, (s, "{}", None::<&str>, None::<&str>, None::<&str>, 0i64, 0i64, rev))?;
        }
        for e in events {
            let rev = self.rev.fetch_add(1, Ordering::Relaxed) + 1;
            let params: Vec<mysql::Value> = vec![
                e.session.clone().into(),
                e.id.clone().into(),
                e.seq.into(),
                e.at.into(),
                mysql::Value::NULL,
                mysql::Value::NULL,
                e.kind.into(),
                mysql::Value::NULL,
                mysql::Value::NULL,
                mysql::Value::NULL,
                e.text.clone().into(),
                e.body.clone().into(),
                rev.into(),
            ];
            tx.exec_drop(EVENT_UPSERT, params)?;
        }
        tx.commit()?;
        Ok(())
    }

    fn history(&self, session: &str, limit: usize) -> anyhow::Result<usize> {
        let mut conn = self.pool.get_conn()?;
        // Every column taken, as a reader of the page would (the store builds its rows).
        let rows: Vec<mysql::Row> = conn.exec(EVENTS_LAST, (session, limit as u64))?;
        for r in &rows {
            let row: Vec<mysql::Value> = (0..12).map(|i| r.as_ref(i).cloned().unwrap_or(mysql::Value::NULL)).collect();
            std::hint::black_box(row);
        }
        Ok(rows.len())
    }

    fn search(&self, word: &str, limit: usize) -> anyhow::Result<usize> {
        let mut conn = self.pool.get_conn()?;
        let rows: Vec<(String, String)> = conn.exec(SEARCH, (format!("%{word}%"), limit as u64))?;
        Ok(rows.len())
    }
}
