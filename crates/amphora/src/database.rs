//! One local Amphora database for SQL and namespaced Records (CP-T1.5, U05/U06).
//! All answers go through the existing commit pipeline and its durable callback. In this first
//! integration, reads are serialized too: they cannot acknowledge a value whose write isn't durable.
//! This is local durability only. U15 must put these owned commands through Amphora's hyper-durable
//! state machine before quorum durability or follower reads are exposed to a host.

use std::any::Any;
use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::sync::{OnceLock, Weak};
use std::time::Instant;

use amphora_api::{
    self as api, Change, Changes, Cond, Consistency, Migration, Mutation, Page, Records, Refusal,
    Rev, Row, Scan, SqlResult, Statement, Tx, Value, Write,
};
use amphora_engine::{Numeric, Value as EValue};
use amphora_engine_parser::{
    ast::{Cmd, Stmt},
    parser::Parser,
};

use crate::db::Db;
use crate::store::{self, Checkpointed, Engine, Flush, Opener, Store};

mod import;
pub(crate) mod replicated;
mod restore;
pub(crate) mod session;
mod watch;
pub use restore::Restore;
pub use watch::Watch;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS __amphora_namespaces (name TEXT PRIMARY KEY, head INTEGER NOT NULL, horizon INTEGER NOT NULL, feed INTEGER NOT NULL, live INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS __amphora_versions (namespace TEXT NOT NULL, family TEXT NOT NULL, key BLOB NOT NULL, rev INTEGER NOT NULL, seq INTEGER NOT NULL, value BLOB, PRIMARY KEY(namespace, family, key, rev));
CREATE INDEX IF NOT EXISTS __amphora_feed ON __amphora_versions(namespace, family, rev, seq);
CREATE TABLE IF NOT EXISTS __amphora_families (namespace TEXT NOT NULL, family TEXT NOT NULL, newest INTEGER NOT NULL, PRIMARY KEY(namespace, family));
CREATE TABLE IF NOT EXISTS __amphora_migrations (name TEXT PRIMARY KEY, sql TEXT NOT NULL);";
const LIVE_MAX: u64 = 10_000_000;

#[derive(Clone)]
pub struct Database {
    shared: Arc<Shared>,
}

struct Shared {
    writer: Writer,
    changed: Arc<Changed>,
    restoring: AtomicBool,
    // The engine, readers and flusher retain the same lease until asynchronous drain ends.
    _owner: Option<Arc<File>>,
}

enum Writer {
    Local(Store<Local>),
    Replicated {
        client: replicated::Client,
        log_path: PathBuf,
        node_id: u64,
    },
}
struct Owner {
    shared: Weak<Shared>,
    lease: Weak<File>,
}
static OWNERS: OnceLock<Mutex<BTreeMap<PathBuf, Owner>>> = OnceLock::new();

#[derive(Default)]
pub(crate) struct Changed {
    generation: Mutex<u64>,
    event: Condvar,
    published: Mutex<BTreeMap<String, Published>>,
    watchers: Mutex<watch::Watchers>,
}

#[derive(Default)]
struct Published {
    head: Rev,
    feed: Rev,
    families: BTreeMap<String, Rev>,
    importing: bool,
}

enum Publish {
    Records(String, Vec<String>),
    Compact(String),
    Imported(String),
    Call,
}

/// Lightweight namespace over the same database, pipeline and durable commit path.
#[derive(Clone)]
pub struct Namespace {
    database: Database,
    name: String,
}

enum Operation {
    Mutation(Mutation),
    Get(String, Vec<u8>, Consistency),
    Scan(String, Scan),
    Changes(String, Rev, usize),
    Head,
    Call(Mutex<Option<Call>>),
    Import(amphora_api::import::ImportPage),
    ImportProgress,
}

type Call = Box<dyn FnOnce(&mut Db) -> anyhow::Result<Box<dyn Any + Send>> + Send>;

enum Answer {
    Sql(Vec<SqlResult>),
    Migrated(bool),
    Row(Option<Row>),
    Page(Page),
    Rev(Rev),
    Changes(Changes),
    Compacted { head: Rev, feed: Rev },
    Called(Box<dyn Any + Send>),
    Imported(import::Imported),
    ImportProgress(Option<amphora_api::import::ImportProgress>),
}
type Reply = Arc<Mutex<Option<Result<Answer, Refusal>>>>;
struct Command {
    namespace: String,
    operation: Operation,
    answer: Reply,
    publication: Arc<Mutex<Option<BTreeMap<String, Published>>>>,
}
struct Local {
    db: Db,
    restoring: bool,
}

fn unavailable(e: impl std::fmt::Display) -> Refusal {
    Refusal::Unavailable(e.to_string())
}

fn canonical_path(path: &Path) -> Result<PathBuf, Refusal> {
    if path.exists() {
        return path.canonicalize().map_err(unavailable);
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(unavailable)?;
    let name = path
        .file_name()
        .ok_or_else(|| api::invalid("database path", "no file name"))?;
    Ok(parent.canonicalize().map_err(unavailable)?.join(name))
}

pub(crate) fn exclusive_file(path: &Path) -> Result<File, Refusal> {
    let path = canonical_path(path)?;
    let mut owner_path = path.into_os_string();
    owner_path.push(".owner");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(PathBuf::from(owner_path))
        .map_err(unavailable)?;
    file.try_lock().map_err(|e| unavailable(format!("database ownership is held or unavailable; drain the previous owner before handoff: {e}")))?;
    Ok(file)
}
fn refused(error: &anyhow::Error) -> Refusal {
    if let Some(refusal) = error.downcast_ref::<Refusal>() {
        return refusal.clone();
    }
    if let Some(crate::sql::Error::Refusal(refusal)) = error.downcast_ref::<crate::sql::Error>() {
        return refusal.clone();
    }
    unavailable(error)
}
fn locked<T>(m: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>, Refusal> {
    m.lock()
        .map_err(|_| unavailable("a database response lock was poisoned"))
}
fn integer(n: u64) -> anyhow::Result<EValue> {
    Ok(EValue::from_i64(i64::try_from(n)?))
}
fn text(s: &str) -> EValue {
    EValue::from_text(s.to_owned())
}
fn blob(b: &[u8]) -> anyhow::Result<EValue> {
    EValue::from_slice(b).map_err(|e| anyhow::anyhow!("allocating a value: {e}"))
}
fn rev(r: &amphora_engine::Row, n: usize) -> anyhow::Result<u64> {
    Ok(u64::try_from(r.get::<i64>(n).map_err(unavailable)?)?)
}
fn bytes(r: &amphora_engine::Row, n: usize) -> anyhow::Result<Option<Vec<u8>>> {
    match r.get_value(n) {
        EValue::Null => Ok(None),
        v => v
            .to_blob()
            .map(|v| Some(v.to_vec()))
            .ok_or_else(|| anyhow::anyhow!("record is not a blob")),
    }
}

pub(crate) fn bootstrap_db(db: &Db) -> Result<(), Refusal> {
    db.exec(SCHEMA).map_err(unavailable)?;
    db.exec(restore::SCHEMA).map_err(unavailable)?;
    db.exec(import::SCHEMA).map_err(unavailable)?;
    Ok(())
}

fn read_published(db: &Db) -> Result<BTreeMap<String, Published>, Refusal> {
    let mut published = BTreeMap::new();
    db.query(
        "SELECT name, head, feed FROM __amphora_namespaces",
        [],
        |r| {
            let name = r.get::<String>(0).map_err(unavailable)?;
            published.insert(
                name,
                Published {
                    head: rev(r, 1)?,
                    feed: rev(r, 2)?,
                    ..Published::default()
                },
            );
            Ok(true)
        },
    )
    .map_err(unavailable)?;
    db.query(
        "SELECT namespace, family, newest FROM __amphora_families",
        [],
        |r| {
            let namespace = r.get::<String>(0).map_err(unavailable)?;
            let family = r.get::<String>(1).map_err(unavailable)?;
            published
                .entry(namespace)
                .or_default()
                .families
                .insert(family, rev(r, 2)?);
            Ok(true)
        },
    )
    .map_err(unavailable)?;
    db.query(
        "SELECT namespace FROM __amphora_imports WHERE done = 0",
        [],
        |r| {
            published
                .entry(r.get::<String>(0).map_err(unavailable)?)
                .or_default()
                .importing = true;
            Ok(true)
        },
    )
    .map_err(unavailable)?;
    Ok(published)
}

pub(crate) fn reload_changed(db: &Db, changed: &Changed) -> Result<(), Refusal> {
    *locked(&changed.published)? = read_published(db)?;
    wake_changed(changed)
}

pub(crate) fn wake_changed(changed: &Changed) -> Result<(), Refusal> {
    {
        let mut generation = locked(&changed.generation)?;
        *generation = generation.wrapping_add(1);
        changed.event.notify_all();
    }
    let wakers: Vec<_> = locked(&changed.watchers)?
        .wakers
        .values()
        .cloned()
        .collect();
    for waker in wakers {
        waker.wake();
    }
    Ok(())
}

impl Database {
    fn open_owned(
        path: &Path,
        until: Instant,
        restoring: bool,
        initialize: impl FnOnce(&mut Db) -> Result<(), Refusal>,
    ) -> Result<Self, Refusal> {
        Self::owner(path, until, restoring, None, |path, owner| {
            let mut db = Db::open(path).map_err(unavailable)?;
            db.hold_owner(Arc::clone(&owner));
            initialize(&mut db)?;
            Self::from_db_mode(db, until, restoring, Some(owner))
        })
    }

    fn owner(
        path: &Path,
        until: Instant,
        restoring: bool,
        replica: Option<(&Path, u64)>,
        create: impl FnOnce(&Path, Arc<File>) -> Result<Self, Refusal>,
    ) -> Result<Self, Refusal> {
        let path = canonical_path(path)?;
        loop {
            if Instant::now() >= until {
                return Err(Refusal::Timeout);
            }
            let mut owners = locked(OWNERS.get_or_init(Mutex::default))?;
            owners.retain(|_, owner| {
                owner.shared.strong_count() > 0 || owner.lease.strong_count() > 0
            });
            if let Some(owner) = owners.get(&path) {
                if let Some(shared) = owner.shared.upgrade() {
                    if restoring || shared.restoring.load(Ordering::Acquire) {
                        return Err(api::invalid(
                            "database ownership",
                            "a live owner must drain before restore",
                        ));
                    }
                    if let Some((log_path, node_id)) = replica {
                        match &shared.writer {
                            Writer::Replicated {
                                log_path: existing,
                                node_id: id,
                                ..
                            } if existing == log_path && *id == node_id => {}
                            _ => {
                                return Err(api::invalid(
                                    "replica ownership",
                                    "a different live owner must drain before replicated handoff",
                                ));
                            }
                        }
                    }
                    return Ok(Self { shared });
                }
                // Store shutdown is asynchronous. Its engine/readers/flusher still retain the
                // lease, so wait for their actual drain instead of starting a second writer.
                if owner.lease.strong_count() > 0 {
                    drop(owners);
                    std::thread::park_timeout(
                        until
                            .saturating_duration_since(Instant::now())
                            .min(std::time::Duration::from_millis(1)),
                    );
                    continue;
                }
            }
            api::bound("open database owners", owners.len().saturating_add(1), 4096)?;
            let owner = Arc::new(exclusive_file(&path)?);
            let database = create(&path, Arc::clone(&owner))?;
            owners.insert(
                path,
                Owner {
                    shared: Arc::downgrade(&database.shared),
                    lease: Arc::downgrade(&owner),
                },
            );
            return Ok(database);
        }
    }

    pub fn open(path: &Path, until: Instant) -> Result<Self, Refusal> {
        Self::open_owned(path, until, false, |_| Ok(()))
    }

    /// Opens the one product-owned hyper-durable writer. The host carries authenticated wire
    /// records through `replica()`; it never drives another Raft controller or writes this file.
    pub fn open_replicated(
        path: &Path,
        log_path: &Path,
        config: api::replication::ReplicaConfig,
        until: Instant,
    ) -> Result<Self, Refusal> {
        let log_path = canonical_path(log_path)?;
        let node_id = config.node_id;
        Self::owner(
            path,
            until,
            false,
            Some((&log_path, node_id)),
            |path, owner| {
                let changed = Arc::new(Changed::default());
                let client = replicated::start(
                    path,
                    &log_path,
                    config,
                    Arc::clone(&changed),
                    Arc::clone(&owner),
                    until,
                )?;
                Ok(Self {
                    shared: Arc::new(Shared {
                        writer: Writer::Replicated {
                            client,
                            log_path: log_path.clone(),
                            node_id,
                        },
                        changed,
                        restoring: AtomicBool::new(false),
                        _owner: Some(owner),
                    }),
                })
            },
        )
    }

    pub fn replica(&self) -> Option<crate::replication::ReplicaHandle> {
        match &self.shared.writer {
            Writer::Replicated { client, .. } => Some(client.handle()),
            Writer::Local(_) => None,
        }
    }

    /// The same SQL/Records engine and commit pipeline on an in-memory I/O device.
    pub fn in_memory(until: Instant) -> Result<Self, Refusal> {
        let io = Arc::new(amphora_engine::MemoryIO::new());
        Self::from_db(
            Db::open_with(Path::new("amphora.db"), io).map_err(unavailable)?,
            until,
        )
    }

    fn from_db(db: Db, until: Instant) -> Result<Self, Refusal> {
        Self::from_db_mode(db, until, false, None)
    }

    fn from_db_mode(
        mut db: Db,
        until: Instant,
        restoring: bool,
        owner: Option<Arc<File>>,
    ) -> Result<Self, Refusal> {
        bootstrap_db(&mut db)?;
        let mut replicated = false;
        db.query(
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = '__amphora_replica'",
            [],
            |_| {
                replicated = true;
                Ok(false)
            },
        )
        .map_err(unavailable)?;
        if replicated {
            return Err(api::invalid(
                "database ownership",
                "replicated data must reopen through its replicated owner",
            ));
        }
        if !restoring && restore::incomplete(&mut db).map_err(unavailable)? {
            return Err(api::invalid(
                "database",
                "an incomplete restore must be resumed before ordinary access",
            ));
        }
        let changed = Changed::default();
        *locked(&changed.published)? = read_published(&mut db)?;
        let database = Self {
            shared: Arc::new(Shared {
                writer: Writer::Local(
                    Store::start(Local { db, restoring }, store::Config::default())
                        .map_err(unavailable)?,
                ),
                changed: Arc::new(changed),
                restoring: AtomicBool::new(restoring),
                _owner: owner,
            }),
        };
        // Bootstrap DDL used the same WAL; the barrier makes it durable before open succeeds.
        database.submit("", Operation::Mutation(Mutation::Sql(Vec::new())), until)?;
        Ok(database)
    }

    pub fn namespace(&self, name: &str) -> Result<Namespace, Refusal> {
        api::family(name)?;
        Ok(Namespace {
            database: self.clone(),
            name: name.to_owned(),
        })
    }

    /// Runs one adaptive SQL session atomically on the writer. The closure runs exactly once;
    /// an error rolls back the session, and a successful answer waits for the durable callback.
    /// A caller whose deadline expires after submission must treat the outcome as unknown.
    pub fn call<R, F>(&self, until: Instant, f: F) -> Result<R, Refusal>
    where
        R: Send + 'static,
        F: FnOnce(&mut crate::sql::Connection<'_>) -> anyhow::Result<R> + Send + 'static,
    {
        self.call_db(until, move |db| {
            let mut connection = crate::sql::Connection::new(db);
            let value = f(&mut connection)?;
            connection.finish()?;
            Ok(value)
        })
    }

    fn call_db<R, F>(&self, until: Instant, f: F) -> Result<R, Refusal>
    where
        R: Send + 'static,
        F: FnOnce(&mut Db) -> anyhow::Result<R> + Send + 'static,
    {
        let call: Call = Box::new(move |db| f(db).map(|v| Box::new(v) as Box<dyn Any + Send>));
        match self.submit("", Operation::Call(Mutex::new(Some(call))), until)? {
            Answer::Called(v) => v
                .downcast::<R>()
                .map(|v| *v)
                .map_err(|_| unavailable("wrong SQL session answer")),
            _ => Err(unavailable("wrong SQL session answer")),
        }
    }

    /// One atomic batch, including SELECT or DML RETURNING results. Parameters and SQL are owned;
    /// transaction-control, connection PRAGMAs and ATTACH are refused before anything is submitted.
    pub fn execute(
        &self,
        statements: Vec<Statement>,
        until: Instant,
    ) -> Result<Vec<SqlResult>, Refusal> {
        api::bound("statements", statements.len(), api::TX_MAX)?;
        let mut bytes = 0usize;
        for s in &statements {
            validate_sql(&s.sql, false)?;
            api::bound("parameters", s.params.len(), api::TX_MAX)?;
            bytes = bytes
                .checked_add(s.sql.len())
                .ok_or_else(|| api::invalid("SQL request", "size overflow"))?;
            for value in &s.params {
                bytes = bytes
                    .checked_add(value_bytes(value))
                    .ok_or_else(|| api::invalid("SQL request", "size overflow"))?;
            }
        }
        api::bound("SQL request bytes", bytes, api::REQUEST_MAX)?;
        match self.submit("", Operation::Mutation(Mutation::Sql(statements)), until)? {
            Answer::Sql(v) => Ok(v),
            _ => Err(unavailable("wrong SQL answer")),
        }
    }

    pub fn migrate(&self, migration: Migration, until: Instant) -> Result<bool, Refusal> {
        api::family(&migration.name)?;
        validate_sql(&migration.sql, true)?;
        match self.submit("", Operation::Mutation(Mutation::Migrate(migration)), until)? {
            Answer::Migrated(v) => Ok(v),
            _ => Err(unavailable("wrong migration answer")),
        }
    }

    fn submit(
        &self,
        namespace: &str,
        operation: Operation,
        until: Instant,
    ) -> Result<Answer, Refusal> {
        if Instant::now() >= until {
            return Err(Refusal::Timeout);
        }
        let store = match &self.shared.writer {
            Writer::Replicated { client, .. } => {
                return client.submit(namespace.to_owned(), operation, until);
            }
            Writer::Local(store) => store,
        };
        let wake = matches!(&operation, Operation::Mutation(Mutation::Records { tx, .. }) if !tx.writes.is_empty())
            || matches!(&operation, Operation::Mutation(Mutation::Compact { .. }));
        let wake = wake || matches!(&operation, Operation::Import(_) | Operation::Call(_));
        let publish = match &operation {
            Operation::Mutation(Mutation::Records { namespace, tx }) if !tx.writes.is_empty() => {
                let mut families = BTreeMap::new();
                for write in &tx.writes {
                    let (Write::Put { family, .. } | Write::Delete { family, .. }) = write;
                    families.insert(family.clone(), ());
                }
                Some(Publish::Records(
                    namespace.clone(),
                    families.into_keys().collect(),
                ))
            }
            Operation::Mutation(Mutation::Compact { namespace, .. }) => {
                Some(Publish::Compact(namespace.clone()))
            }
            Operation::Import(_) => Some(Publish::Imported(namespace.into())),
            Operation::Call(_) => Some(Publish::Call),
            _ => None,
        };
        let answer: Reply = Arc::new(Mutex::new(None));
        let publication = Arc::new(Mutex::new(None));
        let (tx, rx) = mpsc::sync_channel(1);
        let changed = Arc::clone(&self.shared.changed);
        store
            .put_then(
                vec![Command {
                    namespace: namespace.to_owned(),
                    operation,
                    answer: Arc::clone(&answer),
                    publication: Arc::clone(&publication),
                }],
                until,
                move |done| {
                    let mut result = match done {
                        Ok(()) => locked(&answer)
                            .and_then(|mut r| {
                                r.take()
                                    .ok_or_else(|| unavailable("the writer answered no result"))
                            })
                            .and_then(|v| v),
                        Err(store::Refused::Failed) => locked(&answer)
                            .ok()
                            .and_then(|mut r| r.take())
                            .and_then(Result::err)
                            .map_or_else(|| Err(unavailable("the transaction failed")), Err),
                        Err(e) => Err(unavailable(e)),
                    };
                    if let Some(publish) = publish
                        && result.is_ok()
                    {
                        let updated = (|| -> Result<(), Refusal> {
                            let mut published = locked(&changed.published)?;
                            match (publish, &result) {
                                (Publish::Records(namespace, families), Ok(Answer::Rev(at))) => {
                                    let state = published.entry(namespace).or_default();
                                    state.head = state.head.max(*at);
                                    for family in families {
                                        let newest = state.families.entry(family).or_default();
                                        *newest = (*newest).max(*at);
                                    }
                                }
                                (
                                    Publish::Compact(namespace),
                                    Ok(Answer::Compacted { head, feed }),
                                ) => {
                                    let state = published.entry(namespace).or_default();
                                    state.head = state.head.max(*head);
                                    state.feed = state.feed.max(*feed);
                                }
                                (Publish::Imported(namespace), Ok(Answer::Imported(imported))) => {
                                    let state = published.entry(namespace).or_default();
                                    if let Some(imported) = &imported.publication {
                                        state.head = state.head.max(imported.head);
                                        state.feed = state.feed.max(imported.feed);
                                        for (family, newest) in &imported.newest {
                                            let at =
                                                state.families.entry(family.clone()).or_default();
                                            *at = (*at).max(*newest);
                                        }
                                    }
                                    state.importing = !imported.done;
                                }
                                (Publish::Call, _) => {
                                    if let Some(snapshot) = locked(&publication)?.take() {
                                        for (namespace, incoming) in snapshot {
                                            let state = published.entry(namespace).or_default();
                                            state.head = state.head.max(incoming.head);
                                            state.feed = state.feed.max(incoming.feed);
                                            state.importing = incoming.importing;
                                            for (family, at) in incoming.families {
                                                let newest =
                                                    state.families.entry(family).or_default();
                                                *newest = (*newest).max(at);
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                            Ok(())
                        })();
                        if let Err(error) = updated {
                            result = Err(error);
                        }
                    }
                    if wake && result.is_ok() {
                        match locked(&changed.generation) {
                            Ok(mut generation) => {
                                // An equality token, never an externally visible revision. Wrapping
                                // requires 2^64 notifications while a waiter checks one token.
                                *generation = generation.wrapping_add(1);
                                changed.event.notify_all();
                            }
                            Err(error) => result = Err(error),
                        }
                        let waking = locked(&changed.watchers)
                            .map(|watchers| watchers.wakers.values().cloned().collect::<Vec<_>>());
                        match waking {
                            Ok(wakers) => {
                                for waker in wakers {
                                    waker.wake();
                                }
                            }
                            Err(error) => result = Err(error),
                        }
                    }
                    // A caller whose deadline elapsed no longer waits, but the committed result stands.
                    let _ = tx.send(result);
                },
            )
            .map_err(unavailable)?;
        rx.recv_timeout(until.saturating_duration_since(Instant::now()))
            .map_err(|e| match e {
                mpsc::RecvTimeoutError::Timeout => Refusal::Timeout,
                mpsc::RecvTimeoutError::Disconnected => unavailable("the notifier stopped"),
            })?
    }
}

impl Namespace {
    pub fn import_page(
        &self,
        page: amphora_api::import::ImportPage,
        until: Instant,
    ) -> Result<(), Refusal> {
        import::validate(&page)?;
        match self
            .database
            .submit(&self.name, Operation::Import(page), until)?
        {
            Answer::Imported(_) => Ok(()),
            _ => Err(unavailable("wrong import answer")),
        }
    }
    pub fn import_state(
        &self,
        until: Instant,
    ) -> Result<Option<amphora_api::import::ImportProgress>, Refusal> {
        match self
            .database
            .submit(&self.name, Operation::ImportProgress, until)?
        {
            Answer::ImportProgress(progress) => Ok(progress),
            _ => Err(unavailable("wrong import progress answer")),
        }
    }

    /// Checks only metadata published by successful durable callbacks. No disk/queue wait and
    /// no speculative writer state is observed; a changed family is ready on the first poll.
    pub fn try_wait(&self, family: &str, after: Rev) -> Result<Option<Rev>, Refusal> {
        api::family(family)?;
        let published = locked(&self.database.shared.changed.published)?;
        let Some(namespace) = published.get(&self.name) else {
            return Ok(None);
        };
        if namespace.importing {
            return Err(api::invalid("namespace", "import is incomplete"));
        }
        if after < namespace.feed {
            return Err(Refusal::Compacted {
                asked: after,
                horizon: namespace.feed,
            });
        }
        Ok(namespace
            .families
            .get(family)
            .copied()
            .filter(|r| *r > after))
    }
}

impl Records for Namespace {
    fn get(
        &self,
        family: &str,
        key: &[u8],
        at: Consistency,
        until: Instant,
    ) -> Result<Option<Row>, Refusal> {
        api::family(family)?;
        api::key(key)?;
        match self.database.submit(
            &self.name,
            Operation::Get(family.into(), key.into(), at),
            until,
        )? {
            Answer::Row(v) => Ok(v),
            _ => Err(unavailable("wrong record answer")),
        }
    }
    fn scan(&self, family: &str, mut scan: Scan, until: Instant) -> Result<Page, Refusal> {
        api::family(family)?;
        scan.limit = api::limit(scan.limit)?;
        api::bound("prefix", scan.prefix.len(), api::KEY_MAX)?;
        if let Some(k) = &scan.after {
            api::key(k)?;
        }
        match self
            .database
            .submit(&self.name, Operation::Scan(family.into(), scan), until)?
        {
            Answer::Page(v) => Ok(v),
            _ => Err(unavailable("wrong scan answer")),
        }
    }
    fn commit(&self, tx: Tx, until: Instant) -> Result<Rev, Refusal> {
        api::transaction(&tx)?;
        match self.database.submit(
            &self.name,
            Operation::Mutation(Mutation::Records {
                namespace: self.name.clone(),
                tx,
            }),
            until,
        )? {
            Answer::Rev(v) => Ok(v),
            _ => Err(unavailable("wrong commit answer")),
        }
    }
    fn changes(
        &self,
        family: &str,
        after: Rev,
        limit: usize,
        until: Instant,
    ) -> Result<Changes, Refusal> {
        api::family(family)?;
        let limit = api::limit(limit)?;
        match self.database.submit(
            &self.name,
            Operation::Changes(family.into(), after, limit),
            until,
        )? {
            Answer::Changes(v) => Ok(v),
            _ => Err(unavailable("wrong changes answer")),
        }
    }
    fn head(&self, until: Instant) -> Result<Rev, Refusal> {
        match self.database.submit(&self.name, Operation::Head, until)? {
            Answer::Rev(v) => Ok(v),
            _ => Err(unavailable("wrong head answer")),
        }
    }
    fn compact(&self, before: Rev, until: Instant) -> Result<(), Refusal> {
        match self.database.submit(
            &self.name,
            Operation::Mutation(Mutation::Compact {
                namespace: self.name.clone(),
                before,
            }),
            until,
        )? {
            Answer::Compacted { .. } => Ok(()),
            _ => Err(unavailable("wrong compact answer")),
        }
    }
    fn wait(&self, family: &str, after: Rev, until: Instant) -> Result<Rev, Refusal> {
        api::family(family)?;
        loop {
            let changed = &self.database.shared.changed;
            let before = *locked(&changed.generation)?;
            if let Some(at) = self.try_wait(family, after)? {
                return Ok(at);
            }
            let guard = locked(&changed.generation)?;
            if *guard != before {
                continue;
            }
            if Instant::now() >= until {
                return Err(Refusal::Timeout);
            }
            let (_guard, timeout) = changed
                .event
                .wait_timeout_while(
                    guard,
                    until.saturating_duration_since(Instant::now()),
                    |v| *v == before,
                )
                .map_err(unavailable)?;
            if timeout.timed_out() {
                return Err(Refusal::Timeout);
            }
        }
    }
}

impl Engine for Local {
    type Row = Command;
    type Reader = Db;
    fn key(_: &Command) -> &str {
        "database"
    }
    fn apply(&mut self, rows: &[Command]) -> anyhow::Result<usize> {
        let restoring = self.restoring;
        // An adaptive session contains an FnOnce: the pipeline must never retry it. Its own
        // transaction captures the application result before the pipeline's durable barrier.
        if rows
            .iter()
            .any(|r| matches!(r.operation, Operation::Call(_)))
        {
            for row in rows {
                let result = self
                    .db
                    .transaction(|db| {
                        if !restoring && restore::incomplete(db)? {
                            return Err(api::invalid("database", "restore is incomplete").into());
                        }
                        match &row.operation {
                            Operation::Call(call) => {
                                let f = locked(call)?.take().ok_or_else(|| {
                                    anyhow::anyhow!("SQL session was already executed")
                                })?;
                                let value = f(db)?;
                                *locked(&row.publication)? = Some(read_published(db)?);
                                Ok(Ok(Answer::Called(value)))
                            }
                            other => apply(db, &row.namespace, other),
                        }
                    })
                    .unwrap_or_else(|e| Err(refused(&e)));
                *locked(&row.answer)? = Some(result);
            }
            return Ok(rows.len());
        }
        let done = self.db.transaction(|db| {
            if !restoring && restore::incomplete(db)? {
                return Err(api::invalid("database", "restore is incomplete").into());
            }
            rows.iter()
                .map(|r| apply(db, &r.namespace, &r.operation))
                .collect::<anyhow::Result<Vec<_>>>()
        });
        match done {
            Ok(answers) => {
                for (r, a) in rows.iter().zip(answers) {
                    *locked(&r.answer)? = Some(a);
                }
                Ok(rows.len())
            }
            Err(e) => {
                for r in rows {
                    *locked(&r.answer)? = Some(Err(refused(&e)));
                }
                Err(e)
            }
        }
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
}

fn validate_sql(sql: &str, script: bool) -> Result<(), Refusal> {
    api::bound("SQL", sql.len(), api::SQL_MAX)?;
    let mut parser = Parser::new(sql.as_bytes());
    let mut count = 0usize;
    while let Some(cmd) = parser.next_cmd().map_err(unavailable)? {
        count = count
            .checked_add(1)
            .ok_or_else(|| unavailable("statement count overflow"))?;
        api::bound("statements", count, if script { api::TX_MAX } else { 1 })?;
        if let Cmd::Stmt(s) = cmd
            && matches!(
                s,
                Stmt::Begin { .. }
                    | Stmt::Commit { .. }
                    | Stmt::Rollback { .. }
                    | Stmt::Savepoint { .. }
                    | Stmt::Release { .. }
                    | Stmt::Attach { .. }
                    | Stmt::Detach { .. }
                    | Stmt::Vacuum { .. }
                    | Stmt::Pragma { .. }
            )
        {
            return Err(api::invalid(
                "SQL",
                "connection and transaction control belongs to Amphora",
            ));
        }
    }
    if count == 0 {
        return Err(api::invalid("SQL", "no statement"));
    }
    Ok(())
}

fn native(v: &Value) -> anyhow::Result<EValue> {
    Ok(match v {
        Value::Null => EValue::Null,
        Value::Integer(n) => EValue::from_i64(*n),
        Value::Real(n) => EValue::from_f64(*n),
        Value::Text(s) => text(s),
        Value::Blob(b) => blob(b)?,
    })
}
fn owned(v: &EValue) -> Value {
    match v {
        EValue::Null => Value::Null,
        EValue::Numeric(Numeric::Integer(n)) => Value::Integer(*n),
        EValue::Numeric(Numeric::Float(n)) => Value::Real(f64::from(*n)),
        EValue::Text(s) => Value::Text(s.as_str().into()),
        EValue::Blob(b) => Value::Blob(b.to_vec()),
    }
}
fn value_bytes(v: &Value) -> usize {
    match v {
        Value::Text(s) => s.len(),
        Value::Blob(b) => b.len(),
        _ => 8,
    }
}

fn apply(
    db: &Db,
    namespace: &str,
    operation: &Operation,
) -> anyhow::Result<Result<Answer, Refusal>> {
    match operation {
        Operation::Import(page) => {
            return import::apply_page(db, namespace, page)
                .map(|imported| Ok(Answer::Imported(imported)));
        }
        Operation::ImportProgress => {
            return import::progress(db, namespace)
                .map(|progress| Ok(Answer::ImportProgress(progress)));
        }
        Operation::Call(_) => {
            return Err(anyhow::anyhow!(
                "SQL session reached replayable command dispatch"
            ));
        }
        Operation::Mutation(Mutation::Sql(statements)) => {
            let mut results = Vec::with_capacity(statements.len());
            let mut bytes = 0usize;
            for s in statements {
                let mut rows = Vec::new();
                let params = s
                    .params
                    .iter()
                    .map(native)
                    .collect::<anyhow::Result<Vec<_>>>()?;
                db.query_owned(&s.sql, params, |r| {
                    bytes = bytes
                        .checked_add(std::mem::size_of::<Vec<Value>>())
                        .ok_or_else(|| unavailable("SQL result size overflow"))?;
                    let mut row = Vec::new();
                    for value in r.get_values() {
                        let value = owned(value);
                        bytes = bytes
                            .checked_add(std::mem::size_of::<Value>())
                            .and_then(|size| size.checked_add(value_bytes(&value)))
                            .ok_or_else(|| unavailable("SQL result size overflow"))?;
                        api::bound("SQL result bytes", bytes, api::RESULT_MAX)?;
                        row.push(value);
                    }
                    api::bound("SQL result bytes", bytes, api::RESULT_MAX)?;
                    rows.push(row);
                    api::bound("SQL result rows", rows.len(), api::SCAN_MAX)?;
                    Ok(true)
                })?;
                results.push(SqlResult {
                    rows,
                    affected: u64::try_from(db.changes())?,
                    last_insert_rowid: db.last_insert_rowid(),
                });
            }
            return Ok(Ok(Answer::Sql(results)));
        }
        Operation::Mutation(Mutation::Migrate(m)) => {
            let mut old = None;
            db.query(
                "SELECT sql FROM __amphora_migrations WHERE name = ?1",
                [text(&m.name)],
                |r| {
                    old = Some(r.get::<String>(0).map_err(unavailable)?);
                    Ok(false)
                },
            )?;
            if let Some(old) = old {
                return Ok(if old == m.sql {
                    Ok(Answer::Migrated(false))
                } else {
                    Err(api::invalid(
                        "migration",
                        "the applied name has different SQL",
                    ))
                });
            }
            db.exec(&m.sql)?;
            db.query(
                "INSERT INTO __amphora_migrations(name, sql) VALUES (?1, ?2)",
                [text(&m.name), text(&m.sql)],
                |_| Ok(true),
            )?;
            return Ok(Ok(Answer::Migrated(true)));
        }
        _ => {}
    }
    if import::incomplete(db, namespace)? {
        return Ok(Err(api::invalid("namespace", "import is incomplete")));
    }
    if matches!(
        operation,
        Operation::Mutation(Mutation::Records { .. } | Mutation::Compact { .. })
    ) {
        db.query("INSERT OR IGNORE INTO __amphora_namespaces(name, head, horizon, feed, live) VALUES (?1, 0, 0, 0, 0)", [text(namespace)], |_| Ok(true))?;
    }
    let mut state = (0, 0, 0, 0);
    db.query(
        "SELECT head, horizon, feed, live FROM __amphora_namespaces WHERE name = ?1",
        [text(namespace)],
        |r| {
            state = (rev(r, 0)?, rev(r, 1)?, rev(r, 2)?, rev(r, 3)?);
            Ok(false)
        },
    )?;
    let (head, horizon, feed, live) = state;
    match operation {
        Operation::Head => Ok(Ok(Answer::Rev(head))),
        Operation::Mutation(Mutation::Records { tx, .. }) => commit(db, namespace, tx, head, live),
        Operation::Get(f, k, at) => {
            let at = match snapshot(*at, head, horizon) {
                Ok(at) => at,
                Err(e) => return Ok(Err(e)),
            };
            Ok(Ok(Answer::Row(get(db, namespace, f, k, at)?)))
        }
        Operation::Scan(f, scan) => {
            let at = match snapshot(scan.at, head, horizon) {
                Ok(at) => at,
                Err(e) => return Ok(Err(e)),
            };
            let upper = successor(&scan.prefix);
            let mut rows = Vec::new();
            db.query("SELECT v.key, v.rev, v.value FROM __amphora_versions v WHERE namespace = ?1 AND family = ?2 AND key >= ?3 AND (?4 IS NULL OR key < ?4) AND (?5 IS NULL OR key > ?5) AND rev = (SELECT MAX(w.rev) FROM __amphora_versions w WHERE w.namespace = v.namespace AND w.family = v.family AND w.key = v.key AND w.rev <= ?6) AND value IS NOT NULL ORDER BY key LIMIT ?7",
                [text(namespace), text(f), blob(&scan.prefix)?, upper.as_ref().map(|v| blob(v)).transpose()?.unwrap_or(EValue::Null), scan.after.as_ref().map(|v| blob(v)).transpose()?.unwrap_or(EValue::Null), integer(at)?, integer(u64::try_from(scan.limit)?.checked_add(1).ok_or_else(|| anyhow::anyhow!("limit overflow"))?)?],
                |r| { rows.push(Row { key: bytes(r, 0)?.ok_or_else(|| anyhow::anyhow!("null key"))?, rev: rev(r, 1)?, value: bytes(r, 2)?.ok_or_else(|| anyhow::anyhow!("null value"))? }); Ok(true) })?;
            let more = rows.len() > scan.limit;
            rows.truncate(scan.limit);
            Ok(Ok(Answer::Page(Page { rows, more })))
        }
        Operation::Changes(f, after, limit) => {
            if *after < feed {
                return Ok(Err(Refusal::Compacted {
                    asked: *after,
                    horizon: feed,
                }));
            }
            if *after >= head {
                return Ok(Ok(Answer::Changes(Changes {
                    changes: Vec::new(),
                    head,
                })));
            }
            let mut changes = Vec::new();
            db.query("SELECT rev, key, value FROM __amphora_versions WHERE namespace = ?1 AND family = ?2 AND rev > ?3 ORDER BY rev, seq LIMIT ?4", [text(namespace), text(f), integer(*after)?, integer(u64::try_from(*limit)?.checked_add(1).ok_or_else(|| anyhow::anyhow!("limit overflow"))?)?], |r| { changes.push(Change { rev: rev(r, 0)?, key: bytes(r, 1)?.ok_or_else(|| anyhow::anyhow!("null key"))?, value: bytes(r, 2)? }); Ok(true) })?;
            if changes.len() > *limit {
                // The cursor is a revision, so a page must never cut a transaction's revision.
                // Otherwise advancing to its last change would silently skip the remaining keys.
                let next = changes.get(*limit).map(|c| c.rev);
                changes.truncate(*limit);
                if changes.last().map(|c| c.rev) == next {
                    changes.retain(|c| Some(c.rev) != next);
                    if changes.is_empty() {
                        return Ok(Err(api::invalid(
                            "change limit",
                            "a transaction does not fit; request up to TX_MAX changes",
                        )));
                    }
                }
            }
            Ok(Ok(Answer::Changes(Changes { changes, head })))
        }
        Operation::Mutation(Mutation::Compact { before, .. }) => {
            let before = (*before).min(head).max(horizon);
            db.query("DELETE FROM __amphora_versions WHERE namespace = ?1 AND rev < (SELECT MAX(w.rev) FROM __amphora_versions w WHERE w.namespace = __amphora_versions.namespace AND w.family = __amphora_versions.family AND w.key = __amphora_versions.key AND w.rev <= ?2)", [text(namespace), integer(before)?], |_| Ok(true))?;
            db.query(
                "UPDATE __amphora_namespaces SET horizon = ?2, feed = ?3 WHERE name = ?1",
                [
                    text(namespace),
                    integer(before)?,
                    integer(feed.max(before.saturating_sub(1)))?,
                ],
                |_| Ok(true),
            )?;
            Ok(Ok(Answer::Compacted {
                head,
                feed: feed.max(before.saturating_sub(1)),
            }))
        }
        Operation::Import(_)
        | Operation::ImportProgress
        | Operation::Call(_)
        | Operation::Mutation(Mutation::Sql(_) | Mutation::Migrate(_)) => Err(anyhow::anyhow!(
            "SQL operation reached the Records dispatch"
        )),
    }
}

fn snapshot(at: Consistency, head: Rev, horizon: Rev) -> Result<Rev, Refusal> {
    match at {
        Consistency::Linearizable => Ok(head),
        Consistency::Snapshot(r) if r > head => Err(Refusal::Ahead { asked: r, head }),
        Consistency::Snapshot(r) if r < horizon => Err(Refusal::Compacted { asked: r, horizon }),
        Consistency::Snapshot(r) => Ok(r),
    }
}
fn get(db: &Db, ns: &str, family: &str, key: &[u8], at: Rev) -> anyhow::Result<Option<Row>> {
    let mut out = None;
    db.query("SELECT rev, value FROM __amphora_versions WHERE namespace = ?1 AND family = ?2 AND key = ?3 AND rev <= ?4 ORDER BY rev DESC LIMIT 1", [text(ns), text(family), blob(key)?, integer(at)?], |r| { out = bytes(r, 1)?.map(|value| Ok::<_, anyhow::Error>(Row { key: key.into(), value, rev: rev(r, 0)? })).transpose()?; Ok(false) })?;
    Ok(out)
}
fn commit(
    db: &Db,
    ns: &str,
    tx: &Tx,
    head: Rev,
    live: u64,
) -> anyhow::Result<Result<Answer, Refusal>> {
    for c in &tx.conds {
        let (Cond::Absent { family, key } | Cond::At { family, key, .. }) = c;
        let now = get(db, ns, family, key, head)?.map(|r| r.rev);
        let holds = match c {
            Cond::Absent { .. } => now.is_none(),
            Cond::At { rev, .. } => now == Some(*rev),
        };
        if !holds {
            return Ok(Err(Refusal::Conflict {
                family: family.clone(),
                key: key.clone(),
                at: now,
            }));
        }
    }
    if tx.writes.is_empty() {
        return Ok(Ok(Answer::Rev(head)));
    }
    let mut final_writes = BTreeMap::new();
    for (seq, w) in tx.writes.iter().enumerate() {
        let (Write::Put { family, key, .. } | Write::Delete { family, key }) = w;
        final_writes.insert((family, key), (seq, w));
    }
    let mut live = live;
    for ((family, key), (_, w)) in &final_writes {
        let before = get(db, ns, family, key, head)?.is_some();
        match (before, matches!(w, Write::Put { .. })) {
            (false, true) => {
                live = live
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("live rows overflow"))?
            }
            (true, false) => {
                live = live
                    .checked_sub(1)
                    .ok_or_else(|| anyhow::anyhow!("live rows underflow"))?
            }
            _ => {}
        }
    }
    if live > LIVE_MAX {
        return Ok(Err(Refusal::Full {
            limit: usize::try_from(LIVE_MAX)?,
        }));
    }
    let at = head
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("revision overflow"))?;
    for ((family, key), (seq, w)) in final_writes {
        let value = match w {
            Write::Put { value, .. } => blob(value)?,
            Write::Delete { .. } => EValue::Null,
        };
        db.query("INSERT INTO __amphora_versions(namespace, family, key, rev, seq, value) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", [text(ns), text(family), blob(key)?, integer(at)?, integer(u64::try_from(seq)?)?, value], |_| Ok(true))?;
        db.query("INSERT INTO __amphora_families(namespace, family, newest) VALUES (?1, ?2, ?3) ON CONFLICT(namespace, family) DO UPDATE SET newest = excluded.newest", [text(ns), text(family), integer(at)?], |_| Ok(true))?;
    }
    db.query(
        "UPDATE __amphora_namespaces SET head = ?2, live = ?3 WHERE name = ?1",
        [text(ns), integer(at)?, integer(live)?],
        |_| Ok(true),
    )?;
    Ok(Ok(Answer::Rev(at)))
}
fn successor(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut end = prefix.to_vec();
    while let Some(v) = end.pop() {
        if let Some(next) = v.checked_add(1) {
            end.push(next);
            return Some(end);
        }
    }
    None
}
