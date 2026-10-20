//! kiln's conversation schema and statements, as its store has them after its three migrations
//! (kiln `crates/kiln/src/convo/sql.rs`, commit 2062317): every engine the bench drives runs these.

/// The tables and indexes, statement by statement.
pub const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS sessions (id TEXT PRIMARY KEY, info TEXT, meta TEXT, state TEXT, changes TEXT, updated_at INTEGER NOT NULL DEFAULT 0, closed_at INTEGER, epoch INTEGER NOT NULL DEFAULT 0, rev INTEGER NOT NULL DEFAULT 0, machine TEXT)",
    "CREATE TABLE IF NOT EXISTS events (session TEXT NOT NULL, id TEXT NOT NULL, seq INTEGER NOT NULL, at INTEGER NOT NULL, parent TEXT, by TEXT, kind TEXT NOT NULL, level TEXT, tool TEXT, status TEXT, text TEXT NOT NULL, body TEXT NOT NULL, rev INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (session, id))",
    "CREATE INDEX IF NOT EXISTS events_by_seq ON events (session, seq)",
    "CREATE INDEX IF NOT EXISTS events_by_time ON events (at)",
    "CREATE INDEX IF NOT EXISTS sessions_by_rev ON sessions (rev)",
    "CREATE INDEX IF NOT EXISTS events_by_rev ON events (rev)",
];

pub const META_SET: &str = "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value";

pub const SESSION_UPSERT: &str = "INSERT INTO sessions (id, info, meta, state, changes, updated_at, epoch, rev) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
    ON CONFLICT (id) DO UPDATE SET
        info = COALESCE(excluded.info, sessions.info),
        meta = COALESCE(excluded.meta, sessions.meta),
        state = COALESCE(excluded.state, sessions.state),
        changes = COALESCE(excluded.changes, sessions.changes),
        updated_at = excluded.updated_at,
        epoch = MAX(excluded.epoch, sessions.epoch),
        rev = excluded.rev";

pub const EVENT_UPSERT: &str = "INSERT INTO events (session, id, seq, at, parent, by, kind, level, tool, status, text, body, rev) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
    ON CONFLICT (session, id) DO UPDATE SET
        seq = excluded.seq, at = excluded.at, parent = excluded.parent, by = excluded.by, kind = excluded.kind,
        level = excluded.level, tool = excluded.tool, status = excluded.status, text = excluded.text, body = excluded.body,
        rev = excluded.rev";

/// A session's last `?2` events, newest first.
pub const EVENTS_LAST: &str = "SELECT session, id, seq, at, parent, by, kind, level, tool, status, text, body FROM events WHERE session = ?1 ORDER BY seq DESC LIMIT ?2";

/// The columns an event is read from.
pub const EVENT_COLUMNS: usize = 12;

/// Events whose text holds a word, newest first.
pub const SEARCH: &str = "SELECT session, id FROM events WHERE text LIKE ?1 ORDER BY at DESC LIMIT ?2";
