//! The `amphora-serve` target: Amphora as a server (`amphora serve` in its container,
//! servers/amphora.sh) through its own client, `amphora::serve::SqlClient` (docs/design/
//! client-protocol.md, decision 2), as every other server runs through its own. The statements are
//! the embedded SQLite targets' (schema.rs): Amphora speaks SQLite's dialect. What it connects with
//! is the member's own config (`--dsn` names it; servers/amphora.sh writes it): the member's
//! address, the cluster's secret and the deadline of one request.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use amphora::serve::{Config, Keys, SqlClient};
use amphora_api::{Refusal, Statement, Value};

use crate::Event;
use crate::pool::Blocking;
use crate::schema as sql;

/// The member config servers/amphora.sh writes, unless `--dsn` names another.
pub fn default_config() -> String {
    format!(
        "{}/.config/amphora-bench/amphora-serve/member.conf",
        std::env::var("HOME").unwrap_or_default()
    )
}

pub struct AmphoraServe {
    addr: String,
    keys: Keys,
    deadline: Duration,
    pool: Mutex<Vec<SqlClient>>,
    rev: AtomicU64,
    seen: Mutex<HashSet<String>>,
}

fn st(sql: &str, params: Vec<Value>) -> Statement {
    Statement {
        sql: sql.to_owned(),
        params,
    }
}

fn text(s: &str) -> Value {
    Value::Text(s.to_owned())
}

impl AmphoraServe {
    /// The member `config` names, its tables made anew.
    pub fn connect(config: &str) -> anyhow::Result<Self> {
        let config = Config::load(Path::new(config))?;
        let addr = config
            .members
            .iter()
            .find(|m| m.node == config.node)
            .map(|m| m.addr.clone())
            .ok_or_else(|| anyhow::anyhow!("the config names no member {}", config.node))?;
        let s = Self {
            addr,
            keys: config.keys()?,
            deadline: config.deadline,
            pool: Mutex::new(Vec::new()),
            rev: AtomicU64::new(0),
            seen: Mutex::new(HashSet::new()),
        };
        let mut c = s.connection()?;
        let mut schema: Vec<Statement> = ["events", "sessions", "meta"]
            .iter()
            .map(|t| st(&format!("DROP TABLE IF EXISTS {t}"), Vec::new()))
            .collect();
        schema.extend(sql::SCHEMA.iter().map(|s| st(s, Vec::new())));
        c.write(schema)?;
        s.give(c);
        Ok(s)
    }

    fn connection(&self) -> anyhow::Result<SqlClient> {
        if let Some(c) = self
            .pool
            .lock()
            .map_err(|_| anyhow::anyhow!("poisoned"))?
            .pop()
        {
            return Ok(c);
        }
        SqlClient::connect(&self.addr, &self.keys, self.deadline)
    }

    /// Back to the pool after `done`, unless the connection failed under it: a refusal is the
    /// member's answer and leaves the connection whole; any other error may have cut it mid-frame.
    fn give_after<T>(&self, c: SqlClient, done: &anyhow::Result<T>) {
        let whole = match done {
            Ok(_) => true,
            Err(e) => e.downcast_ref::<Refusal>().is_some(),
        };
        if whole {
            self.give(c);
        }
    }

    fn give(&self, c: SqlClient) {
        if let Ok(mut pool) = self.pool.lock() {
            pool.push(c);
        }
    }

    fn read(&self, statement: Statement) -> anyhow::Result<usize> {
        let mut c = self.connection()?;
        let done = c.read(statement).map(|table| {
            let n = table.rows.len();
            // Every column taken, as a reader of the page would.
            std::hint::black_box(table);
            n
        });
        self.give_after(c, &done);
        done
    }
}

impl Blocking for AmphoraServe {
    fn write(&self, events: &[Event]) -> anyhow::Result<()> {
        let new: Vec<String> = {
            let mut seen = self.seen.lock().map_err(|_| anyhow::anyhow!("poisoned"))?;
            events
                .iter()
                .filter(|e| seen.insert(e.session.clone()))
                .map(|e| e.session.clone())
                .collect()
        };
        let mut statements = Vec::with_capacity(new.len() + events.len());
        for s in &new {
            let rev = self.rev.fetch_add(1, Ordering::Relaxed) + 1;
            statements.push(st(
                sql::SESSION_UPSERT,
                vec![
                    text(s),
                    text("{}"),
                    Value::Null,
                    Value::Null,
                    Value::Null,
                    Value::Integer(0),
                    Value::Integer(0),
                    Value::Integer(rev as i64),
                ],
            ));
        }
        for e in events {
            let rev = self.rev.fetch_add(1, Ordering::Relaxed) + 1;
            statements.push(st(
                sql::EVENT_UPSERT,
                vec![
                    text(&e.session),
                    text(&e.id),
                    Value::Integer(e.seq as i64),
                    Value::Integer(e.at),
                    Value::Null,
                    Value::Null,
                    text(e.kind),
                    Value::Null,
                    Value::Null,
                    Value::Null,
                    text(&e.text),
                    text(&e.body),
                    Value::Integer(rev as i64),
                ],
            ));
        }
        // One request: the statements are one transaction on the member's writer, all or none.
        let mut c = self.connection()?;
        let done = c.write(statements).map(|_| ());
        self.give_after(c, &done);
        done
    }

    fn history(&self, session: &str, limit: usize) -> anyhow::Result<usize> {
        self.read(st(
            sql::EVENTS_LAST,
            vec![text(session), Value::Integer(limit as i64)],
        ))
    }

    fn search(&self, word: &str, limit: usize) -> anyhow::Result<usize> {
        self.read(st(
            sql::SEARCH,
            vec![
                Value::Text(format!("%{word}%")),
                Value::Integer(limit as i64),
            ],
        ))
    }
}
