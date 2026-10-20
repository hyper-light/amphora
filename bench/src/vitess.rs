//! Clustered MySQL: Vitess, through vtgate's MySQL protocol. The keyspace is sharded by session
//! (`vitess/schema/convo/vschema.json`); a write is one autocommitted multi-row insert into its
//! narrower table. Plain MySQL runs kiln's whole schema (`mysql.rs`).

use mysql::prelude::Queryable;
use mysql::{Opts, Pool, PoolConstraints, PoolOpts};

use crate::Event;
use crate::pool::Blocking;

pub struct Vitess {
    pool: Pool,
}

impl Vitess {
    pub fn connect(dsn: &str, connections: usize) -> anyhow::Result<Self> {
        let constraints = PoolConstraints::new(connections.min(8), connections.max(8)).ok_or_else(|| anyhow::anyhow!("bad pool size"))?;
        let opts = mysql::OptsBuilder::from_opts(Opts::from_url(dsn)?).pool_opts(PoolOpts::default().with_constraints(constraints));
        let pool = Pool::new(opts)?;
        // Every run starts from an empty table, as the local store starts from an empty folder (the
        // events' ids repeat from run to run).
        let mut conn = pool.get_conn()?;
        if conn.query_drop("TRUNCATE TABLE events").is_err() {
            conn.query_drop("DELETE FROM events")?;
        }
        Ok(Self { pool })
    }
}

impl Blocking for Vitess {
    fn write(&self, events: &[Event]) -> anyhow::Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        let mut conn = self.pool.get_conn()?;
        let rows = vec!["(?, ?, ?, ?, ?, ?, ?)"; events.len()].join(", ");
        let mut params: Vec<mysql::Value> = Vec::with_capacity(events.len() * 7);
        for e in events {
            params.extend([
                e.session.clone().into(),
                e.id.clone().into(),
                e.seq.into(),
                e.at.into(),
                e.kind.into(),
                e.text.clone().into(),
                e.body.clone().into(),
            ]);
        }
        conn.exec_drop(format!("INSERT INTO events (session, id, seq, at, kind, text, body) VALUES {rows}"), params)?;
        Ok(())
    }

    fn history(&self, session: &str, limit: usize) -> anyhow::Result<usize> {
        let mut conn = self.pool.get_conn()?;
        let rows: Vec<(String, u64)> = conn.exec("SELECT id, seq FROM events WHERE session = ? ORDER BY seq DESC LIMIT ?", (session, limit as u64))?;
        Ok(rows.len())
    }

    fn search(&self, word: &str, limit: usize) -> anyhow::Result<usize> {
        let mut conn = self.pool.get_conn()?;
        let rows: Vec<(String, String)> = conn.exec("SELECT session, id FROM events WHERE text LIKE ? ORDER BY at DESC LIMIT ?", (format!("%{word}%"), limit as u64))?;
        Ok(rows.len())
    }
}
