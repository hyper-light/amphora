//! ClickHouse over its HTTP interface (servers/clickhouse.sh): kiln's events in one MergeTree table
//! ordered by (session, seq), the columns of `crate::schema`'s, two ways:
//! - **clickhouse**: as it ships. Since 26.2 that gathers small inserts into one part server-side
//!   (`async_insert` and `wait_for_async_insert` on by default), each acknowledged once the part is
//!   written, before it is flushed: an acknowledged write can be lost to a power cut;
//! - **clickhouse-durable**: the same gathering, asked for on each insert (so an older server does
//!   it too), and the table flushes each inserted part and its directory (`fsync_after_insert`,
//!   `fsync_part_directory`): each insert acknowledged once its part is on disk.
//!
//! Every run starts from an empty table. Rows go as JSONEachRow; reads and searches come back as
//! TSV, a row a line. Connections are kept alive in the agent's pool, as many as can be busy at once.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::Event;
use crate::pool::Blocking;

const TABLE: &str = "CREATE TABLE events (session String, id String, seq Int64, at Int64, parent Nullable(String), `by` Nullable(String), kind String, level Nullable(String), tool Nullable(String), status Nullable(String), text String, body String, rev Int64) ENGINE = MergeTree ORDER BY (session, seq)";

const EVENTS_LAST: &str = "SELECT session, id, seq, at, parent, `by`, kind, level, tool, status, text, body FROM events WHERE session = {s:String} ORDER BY seq DESC LIMIT {n:UInt64} FORMAT TSV";

const SEARCH: &str = "SELECT session, id FROM events WHERE text LIKE {w:String} ORDER BY at DESC LIMIT {n:UInt64} FORMAT TSV";

pub struct ClickHouse {
    agent: ureq::Agent,
    url: String,
    durable: bool,
    rev: AtomicU64,
}

impl ClickHouse {
    /// `connections`: how many requests can be in flight at once, each kept alive for the next.
    pub fn connect(url: &str, durable: bool, connections: usize) -> anyhow::Result<Self> {
        // Idle connections are let go well inside the server's keep-alive (keep_alive_timeout, 30 s
        // by default), so none is reused as the server closes it.
        let agent: ureq::Agent =
            ureq::Agent::config_builder().max_idle_connections(connections).max_idle_connections_per_host(connections).max_idle_age(Duration::from_secs(5)).no_delay(true).http_status_as_error(false).build().into();
        let ch = Self { agent, url: format!("{}/", url.trim_end_matches('/')), durable, rev: AtomicU64::new(0) };
        ch.post(None, &[], "DROP TABLE IF EXISTS events SYNC".to_owned())?;
        let settings = if durable { " SETTINGS fsync_after_insert = 1, fsync_part_directory = 1" } else { "" };
        ch.post(None, &[], format!("{TABLE}{settings}"))?;
        Ok(ch)
    }

    /// Posts `body`, with `query` (none: the body is the query) and `params` in the URL; the
    /// response's body once it succeeded.
    fn post(&self, query: Option<&str>, params: &[(&str, &str)], body: String) -> anyhow::Result<String> {
        let mut request = self.agent.post(&self.url);
        if let Some(q) = query {
            request = request.query("query", q);
        }
        for (k, v) in params {
            request = request.query(*k, *v);
        }
        let mut response = request.send(body)?;
        let status = response.status();
        let text = response.body_mut().read_to_string()?;
        if !status.is_success() {
            anyhow::bail!("ClickHouse said {status}: {}", text.trim());
        }
        Ok(text)
    }
}

impl Blocking for ClickHouse {
    fn write(&self, events: &[Event]) -> anyhow::Result<()> {
        let mut rows = String::with_capacity(events.len() * 2048);
        for e in events {
            let rev = self.rev.fetch_add(1, Ordering::Relaxed) + 1;
            let row = serde_json::json!({ "session": e.session, "id": e.id, "seq": e.seq, "at": e.at, "parent": null, "by": null, "kind": e.kind, "level": null, "tool": null, "status": null, "text": e.text, "body": e.body, "rev": rev });
            rows.push_str(&row.to_string());
            rows.push('\n');
        }
        let settings: &[(&str, &str)] = if self.durable { &[("async_insert", "1"), ("wait_for_async_insert", "1")] } else { &[] };
        self.post(Some("INSERT INTO events FORMAT JSONEachRow"), settings, rows)?;
        Ok(())
    }

    fn history(&self, session: &str, limit: usize) -> anyhow::Result<usize> {
        let out = self.post(None, &[("param_s", session), ("param_n", &limit.to_string())], EVENTS_LAST.to_owned())?;
        // Every column taken, as a reader of the page would: each row split into its columns.
        let mut n = 0;
        for line in out.lines() {
            std::hint::black_box(line.split('\t').collect::<Vec<&str>>());
            n += 1;
        }
        Ok(n)
    }

    fn search(&self, word: &str, limit: usize) -> anyhow::Result<usize> {
        let out = self.post(None, &[("param_w", &format!("%{word}%")), ("param_n", &limit.to_string())], SEARCH.to_owned())?;
        Ok(out.lines().count())
    }
}
