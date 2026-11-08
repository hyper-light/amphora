//! One product owner for SQL and namespaced Records (CP-T1.5, U05/U06/U15).
//! `open` uses the standalone durable commit pipeline; `open_replicated` uses the product's
//! hyper-durable actor, quorum commits and read fences. Other processes attach to that same owner
//! through authenticated local transport, without opening another engine or Raft controller.

use std::any::Any;
use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

pub(crate) mod compaction;
mod guards;
mod import;
mod patience;
pub(crate) mod remote;
pub(crate) mod replicated;
mod restore;
pub(crate) mod session;
mod watch;
pub use guards::Guards;
pub(crate) use guards::Owned;
pub use restore::Restore;
pub use watch::Watch;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS __amphora_namespaces (name TEXT PRIMARY KEY, head INTEGER NOT NULL, horizon INTEGER NOT NULL, feed INTEGER NOT NULL, live INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS __amphora_versions (namespace TEXT NOT NULL, family TEXT NOT NULL, key BLOB NOT NULL, rev INTEGER NOT NULL, seq INTEGER NOT NULL, value BLOB, PRIMARY KEY(namespace, family, key, rev));
CREATE INDEX IF NOT EXISTS __amphora_feed ON __amphora_versions(namespace, family, rev, seq);
CREATE TABLE IF NOT EXISTS __amphora_families (namespace TEXT NOT NULL, family TEXT NOT NULL, newest INTEGER NOT NULL, PRIMARY KEY(namespace, family));
CREATE TABLE IF NOT EXISTS __amphora_migrations (name TEXT PRIMARY KEY, sql TEXT NOT NULL);";
const LIVE_MAX: u64 = 10_000_000;
const QUEUED_BYTES: usize = 512 << 20;
struct Admission(Arc<AtomicUsize>);
impl Admission {
    fn reserve(bytes: &Arc<AtomicUsize>) -> Result<Self, Refusal> {
        bytes
            .try_update(Ordering::AcqRel, Ordering::Acquire, |held| {
                held.checked_add(api::REQUEST_MAX)
                    .filter(|next| *next <= QUEUED_BYTES)
            })
            .map_err(|_| Refusal::Full {
                limit: QUEUED_BYTES,
            })?;
        Ok(Self(Arc::clone(bytes)))
    }
}
impl Drop for Admission {
    fn drop(&mut self) {
        self.0.fetch_sub(api::REQUEST_MAX, Ordering::AcqRel);
    }
}

#[derive(Clone)]
pub struct Database {
    shared: Arc<Shared>,
}

struct Shared {
    // Stop accepting before dropping the writer and its asynchronous engine lease.
    transport: Mutex<Option<crate::transport::Owner>>,
    path: OnceLock<PathBuf>,
    writer: Writer,
    changed: Arc<Changed>,
    restoring: AtomicBool,
    admitted: Arc<AtomicUsize>,
    // The engine, readers and flusher retain the same lease until asynchronous drain ends.
    _owner: Option<Owned>,
}

enum Writer {
    Local(Store<Local>),
    Client(remote::Backend),
    Replicated {
        client: replicated::Client,
        log_path: PathBuf,
        node_id: u64,
        config: api::replication::ReplicaConfig,
        durability: api::replication::Durability,
    },
}
struct Owner {
    shared: Weak<Shared>,
    lease: Weak<File>,
    identity: Option<amphora_engine::io::FileId>,
    // The key of the guards this owner was opened with; None for an unguarded owner or a client.
    key: Option<[u8; 32]>,
}
static OWNERS: OnceLock<Mutex<BTreeMap<PathBuf, Owner>>> = OnceLock::new();
/// The paths whose owner this process drained, the latest last. A lease there may still read held
/// for a moment: a child this process forked shares it until its exec (local-owner-transport.md,
/// "Owner guards", J1). At most `DRAINED_MAX`, the oldest evicted first; a path's mark goes once
/// this process takes its lease again.
static DRAINED: OnceLock<Mutex<std::collections::VecDeque<PathBuf>>> = OnceLock::new();
/// The owner table's own bound (`open database owners`): no more paths can have drained at once.
const DRAINED_MAX: usize = 4096;
/// The first pause before a drained path's lease is probed again: the measured window, 62.8 µs
/// on fedora (t1/guards-flake), rounded down. Each pause doubles, up to `LONGEST_PAUSE`.
const FIRST_PAUSE: std::time::Duration = std::time::Duration::from_micros(50);
/// The drain wait's own park: no probe waits longer between looks.
const LONGEST_PAUSE: std::time::Duration = std::time::Duration::from_millis(1);

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
    Done,
    Sql(Vec<SqlResult>),
    Migrated(bool),
    Row(Option<Row>),
    Page(Page),
    Rev(Rev),
    Changes(Changes),
    Compacted(api::CompactionProgress),
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
    until: Instant,
}
struct Local {
    db: Db,
    restoring: bool,
}

fn unavailable(e: impl std::fmt::Display) -> Refusal {
    Refusal::Unavailable(e.to_string())
}

fn admission_refusal(error: store::Refused) -> Refusal {
    match error {
        store::Refused::Full => Refusal::Full {
            limit: QUEUED_BYTES,
        },
        store::Refused::Timeout => Refusal::Timeout,
        store::Refused::Disk => Refusal::Fenced("writer disk failed; reopen after recovery".into()),
        store::Refused::Stopped => unavailable("database writer stopped"),
        store::Refused::Failed => unavailable("transaction could not be committed"),
        store::Refused::Groups => api::invalid("database batch", "cross-group submission"),
    }
}

/// A durability with its fates in member order, so that two statements of one class compare equal
/// wherever the owner's is checked (replication.md, "Fates and durability checked").
fn canonical(durability: api::replication::Durability) -> api::replication::Durability {
    match durability {
        api::replication::Durability::Replicated(mut fates) => {
            fates.of.sort_unstable();
            api::replication::Durability::Replicated(fates)
        }
        persisted @ api::replication::Durability::Persisted => persisted,
    }
}

fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// `path` made canonical, its parent directory made first if it is missing.
fn canonical_path(path: &Path) -> Result<PathBuf, Refusal> {
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) && !path.exists() {
        return Err(api::invalid("database path", "dangling symlink"));
    }
    if !path.try_exists().map_err(unavailable)? {
        std::fs::create_dir_all(parent(path)).map_err(unavailable)?;
    }
    existing_path(path)
}

/// `path` made canonical without making anything, as it will be once the directories it names are
/// made: the deepest of its ancestors that exists made canonical, then the names below it. An
/// unguarded open decides every refusal on this path before the step that needs its directory
/// makes it (local-owner-transport.md, "Owner guards").
fn resolved_path(path: &Path) -> Result<PathBuf, Refusal> {
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) && !path.exists() {
        return Err(api::invalid("database path", "dangling symlink"));
    }
    if path.try_exists().map_err(unavailable)? {
        return path.canonicalize().map_err(unavailable);
    }
    let mut missing = vec![
        path.file_name()
            .ok_or_else(|| api::invalid("database path", "no file name"))?,
    ];
    let mut ancestor = parent(path);
    loop {
        // The transport's bound on a database path's depth (endpoint.rs).
        api::bound("database directory depth", missing.len(), 256)?;
        match ancestor.canonicalize() {
            Ok(mut resolved) => {
                while let Some(name) = missing.pop() {
                    resolved.push(name);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(ancestor.file_name().ok_or_else(|| {
                    api::invalid(
                        "database path",
                        "it names a missing directory by `..`, which can't be resolved before it is made",
                    )
                })?);
                ancestor = parent(ancestor);
            }
            Err(error) => return Err(unavailable(error)),
        }
    }
}

/// `path` made canonical without making anything: a guarded open's parent directory exists, or it
/// is refused (local-owner-transport.md, "Owner guards").
fn existing_path(path: &Path) -> Result<PathBuf, Refusal> {
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) && !path.exists() {
        return Err(api::invalid("database path", "dangling symlink"));
    }
    if path.try_exists().map_err(unavailable)? {
        return path.canonicalize().map_err(unavailable);
    }
    let name = path
        .file_name()
        .ok_or_else(|| api::invalid("database path", "no file name"))?;
    match parent(path).canonicalize() {
        Ok(parent) => Ok(parent.join(name)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(api::invalid(
            "database path",
            "its parent directory does not exist, and a guarded open makes none",
        )),
        Err(error) => Err(unavailable(error)),
    }
}

/// A guarded open's admission to a live owner, under the owner table's lock and before any
/// endpoint or lease effect (local-owner-transport.md, "Owner guards"): the owner is this
/// process's, the file at the path now is the file it opened, and it holds guards of the same key.
fn admit_guarded(
    owner: &Owner,
    shared: &Shared,
    identity: Option<amphora_engine::io::FileId>,
    guards: &Guards,
) -> Result<(), Refusal> {
    if matches!(shared.writer, Writer::Client(_)) {
        return Err(remote_owner());
    }
    if owner.identity.is_none() || owner.identity != identity {
        return Err(api::invalid(
            "database ownership",
            "the file at this path is not the live owner's file: the live owner must drain first",
        ));
    }
    match owner.key {
        Some(key) if key == guards.key() => Ok(()),
        Some(_) => Err(api::invalid(
            "database ownership",
            "the live owner holds other guards: it must drain first",
        )),
        None => Err(api::invalid(
            "database ownership",
            "the live owner is unguarded: it must drain first",
        )),
    }
}

/// Another process owns the database. Its identity can't show which files it opened, so a guarded
/// open neither attaches to it as a client nor takes its place.
/// Forgets the owners that have drained, marking the paths whose lease this process held. A
/// client's entry never held one (its lease is `Weak::new()`).
fn forget_drained(owners: &mut BTreeMap<PathBuf, Owner>) -> Result<(), Refusal> {
    let gone: Vec<PathBuf> = owners
        .iter()
        .filter(|(_, owner)| {
            owner.shared.strong_count() == 0
                && owner.lease.strong_count() == 0
                && !owner.lease.ptr_eq(&Weak::new())
        })
        .map(|(path, _)| path.clone())
        .collect();
    owners.retain(|_, owner| owner.shared.strong_count() > 0 || owner.lease.strong_count() > 0);
    if gone.is_empty() {
        return Ok(());
    }
    let mut drained = locked(DRAINED.get_or_init(Mutex::default))?;
    for path in gone {
        drained.retain(|marked| *marked != path);
        if drained.len() >= DRAINED_MAX {
            drained.pop_front();
        }
        drained.try_reserve(1).map_err(unavailable)?;
        drained.push_back(path);
    }
    Ok(())
}

/// Whether this process drained the owner at `path` and has not owned it since.
fn drained_here(path: &Path) -> Result<bool, Refusal> {
    Ok(locked(DRAINED.get_or_init(Mutex::default))?
        .iter()
        .any(|marked| marked == path))
}

/// This process holds `path`'s lease again: its mark goes.
fn owned_again(path: &Path) -> Result<(), Refusal> {
    locked(DRAINED.get_or_init(Mutex::default))?.retain(|marked| marked != path);
    Ok(())
}

/// Parks before the next probe of a drained path's lease, no later than `until`, and doubles
/// `pause` up to `LONGEST_PAUSE`.
fn pause_for_lease(pause: &mut std::time::Duration, until: Instant) {
    std::thread::park_timeout(until.saturating_duration_since(Instant::now()).min(*pause));
    *pause = pause.saturating_mul(2).min(LONGEST_PAUSE);
}

fn remote_owner() -> Refusal {
    api::invalid(
        "database ownership",
        "another process owns this database, and a guarded open must own it: that owner must drain first",
    )
}

fn file_identity(path: &Path) -> Result<Option<amphora_engine::io::FileId>, Refusal> {
    if !path.try_exists().map_err(unavailable)? {
        return Ok(None);
    }
    let path = path
        .to_str()
        .ok_or_else(|| api::invalid("database path", "not UTF-8"))?;
    amphora_engine::io::get_file_id(path)
        .map(Some)
        .map_err(unavailable)
}

fn try_exclusive_file(path: &Path) -> Result<Option<File>, Refusal> {
    let path = canonical_path(path)?;
    crate::transport::try_lease(&path)
}
pub(crate) fn exclusive_file(path: &Path) -> Result<File, Refusal> {
    try_exclusive_file(path)?
        .ok_or_else(|| unavailable("database ownership is held by another owner"))
}

/// [`exclusive_file`] for `owner`'s files: a guarded owner's makes no parent directory, and a
/// parent removed since its open resolved it is refused, never made again: the host's guards
/// protect the directory that was there (local-owner-transport.md, "Owner guards").
pub(crate) fn exclusive_file_of(path: &Path, owner: &Owned) -> Result<File, Refusal> {
    if !owner.guarded() {
        return exclusive_file(path);
    }
    crate::transport::try_lease(&existing_path(path)?)?
        .ok_or_else(|| unavailable("database ownership is held by another owner"))
}

/// The database at `path` for `owner`, as [`exclusive_file_of`] makes its directories.
pub(crate) fn open_db(path: &Path, owner: &Owned) -> Result<Db, Refusal> {
    if !owner.guarded() {
        return Db::open(path).map_err(unavailable);
    }
    let path = existing_path(path)?;
    Db::open_made(&path, &crate::db::Sharing::Off).map_err(unavailable)
}
pub(crate) fn refused(error: &anyhow::Error) -> Refusal {
    if let Some(refusal) = error.downcast_ref::<Refusal>() {
        return refusal.clone();
    }
    if let Some(crate::sql::Error::Refusal(refusal)) = error.downcast_ref::<crate::sql::Error>() {
        return refusal.clone();
    }
    unavailable(format!("{error:#}"))
}
/// A query's session on `db`, a reader or the writer between its commits: once, before its
/// deadline, never on a database whose restore is unfinished.
fn run_query<R, F>(db: &mut Db, until: Instant, session: &Mutex<Option<F>>) -> anyhow::Result<R>
where
    F: FnOnce(&mut crate::sql::Connection<'_>) -> anyhow::Result<R>,
{
    let f = locked(session)?
        .take()
        .ok_or_else(|| anyhow::anyhow!("SQL query was already executed"))?;
    if Instant::now() >= until {
        return Err(Refusal::Timeout.into());
    }
    if restore::incomplete(db)? {
        return Err(api::invalid("database", "restore is incomplete").into());
    }
    let done = db.query_transaction(|db| {
        let _deadline = db.deadline(until);
        let mut connection = crate::sql::Connection::new(db);
        let value = f(&mut connection)?;
        connection.finish()?;
        Ok(value)
    });
    if done.is_err() && Instant::now() >= until {
        return Err(Refusal::Timeout.into());
    }
    done
}
fn locked<T>(m: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>, Refusal> {
    m.lock()
        .map_err(|_| unavailable("a database response lock was poisoned"))
}
fn locked_until<T>(m: &Mutex<T>, until: Instant) -> Result<std::sync::MutexGuard<'_, T>, Refusal> {
    loop {
        if Instant::now() >= until {
            return Err(Refusal::Timeout);
        }
        match m.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(unavailable("database selection lock was poisoned"));
            }
            Err(std::sync::TryLockError::WouldBlock) => std::thread::park_timeout(
                until
                    .saturating_duration_since(Instant::now())
                    .min(std::time::Duration::from_millis(1)),
            ),
        }
    }
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
        guards: Option<Guards>,
        initialize: impl FnOnce(&mut Db) -> Result<(), Refusal>,
    ) -> Result<Self, Refusal> {
        Self::owner(path, until, restoring, None, guards, |path, owner| {
            let mut db = open_db(path, &owner)?;
            db.hold_owner(owner.clone());
            initialize(&mut db)?;
            Self::from_db_mode(db, until, restoring, Some(owner))
        })
    }

    fn owner(
        path: &Path,
        until: Instant,
        restoring: bool,
        replica: Option<(
            &Path,
            &api::replication::ReplicaConfig,
            &api::replication::Durability,
        )>,
        guards: Option<Guards>,
        create: impl FnOnce(&Path, Owned) -> Result<Self, Refusal>,
    ) -> Result<Self, Refusal> {
        let path = match guards {
            Some(_) => existing_path(path)?,
            // Its parent is made by the lease, past every refusal before it.
            None => resolved_path(path)?,
        };
        let mut pause = FIRST_PAUSE;
        loop {
            if Instant::now() >= until {
                return Err(Refusal::Timeout);
            }
            let mut owners = locked_until(OWNERS.get_or_init(Mutex::default), until)?;
            forget_drained(&mut owners)?;
            let identity = file_identity(&path)?;
            if let Some(owner) = owners.get(&path).or_else(|| {
                identity.and_then(|id| owners.values().find(|owner| owner.identity == Some(id)))
            }) {
                if let Some(shared) = owner.shared.upgrade() {
                    // A reused owner keeps its own group; the incoming guards are dropped.
                    if let Some(guards) = &guards {
                        admit_guarded(owner, &shared, identity, guards)?;
                    }
                    if restoring || shared.restoring.load(Ordering::Acquire) {
                        return Err(api::invalid(
                            "database ownership",
                            "a live owner must drain before restore",
                        ));
                    }
                    if let Writer::Client(backend) = &shared.writer {
                        match remote::validate(&backend.client, replica, until) {
                            Ok(_) => {
                                backend.configure(replica)?;
                                return Ok(Self { shared });
                            }
                            // Identity probes cannot submit application work. Only a successful
                            // OS lease below permits starting the predecessor's replacement.
                            Err(
                                Refusal::Unavailable(_)
                                | Refusal::Timeout
                                | Refusal::UnknownOutcome,
                            ) => (),
                            Err(error) => return Err(error),
                        }
                    } else if let Some((log_path, config, durability)) = replica {
                        match &shared.writer {
                            Writer::Replicated {
                                log_path: existing,
                                node_id: id,
                                config: current,
                                durability: acknowledged,
                                ..
                            } if existing == log_path
                                && *id == config.node_id
                                && current == config
                                && acknowledged == durability => {}
                            _ => {
                                return Err(api::invalid(
                                    "replica ownership",
                                    "a different live owner must drain before replicated handoff",
                                ));
                            }
                        }
                        return Ok(Self { shared });
                    } else {
                        return Ok(Self { shared });
                    }
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
            let lease = match guards {
                // The path is canonical and its parent exists: the lease makes no directory above.
                Some(_) => crate::transport::try_lease(&path)?,
                None => try_exclusive_file(&path)?,
            };
            let Some(lease) = lease else {
                // This process drained the owner here: a child it forked may share the lease until
                // its exec, so the lease is waited for, to the deadline, never refused as another
                // process's. An unguarded open waits as a client, below.
                if (guards.is_some() || restoring) && drained_here(&path)? {
                    drop(owners);
                    pause_for_lease(&mut pause, until);
                    continue;
                }
                if guards.is_some() {
                    return Err(remote_owner());
                }
                if restoring {
                    return Err(api::invalid(
                        "restore",
                        "another process owns the destination",
                    ));
                }
                match remote::attach(&path, replica, until) {
                    Ok(database) => {
                        owners.insert(
                            path,
                            Owner {
                                shared: Arc::downgrade(&database.shared),
                                lease: Weak::new(),
                                identity,
                                key: None,
                            },
                        );
                        return Ok(database);
                    }
                    Err(Refusal::Unavailable(_)) => {
                        drop(owners);
                        std::thread::park_timeout(
                            until
                                .saturating_duration_since(Instant::now())
                                .min(std::time::Duration::from_millis(1)),
                        );
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            };
            owned_again(&path)?;
            let key = guards.as_ref().map(Guards::key);
            let owner = Owned::new(Arc::new(lease), guards);
            let database = create(&path, owner.clone())?;
            remote::publish(&database, &path)?;
            let identity = file_identity(&path)?;
            owners.insert(
                path,
                Owner {
                    shared: Arc::downgrade(&database.shared),
                    lease: Arc::downgrade(owner.lease()),
                    identity,
                    key,
                },
            );
            return Ok(database);
        }
    }

    pub fn open(path: &Path, until: Instant) -> Result<Self, Refusal> {
        Self::open_owned(path, until, false, None, |_| Ok(()))
    }

    /// [`Database::open`], this process owning the database with `guards` kept beside its lease
    /// until the owner has drained (local-owner-transport.md, "Owner guards"). It makes no
    /// directory: a missing parent is refused. A live owner in this process is reused only if
    /// it was opened at this file with guards of the same key, and then keeps its own group; an
    /// unguarded owner, other guards, another file at the path, or another process's owner is
    /// refused before any endpoint or lease effect, until that owner drains.
    pub fn open_guarded(path: &Path, guards: Guards, until: Instant) -> Result<Self, Refusal> {
        Self::open_owned(path, until, false, Some(guards), |_| Ok(()))
    }

    /// Opens the one product-owned hyper-durable writer. The host carries authenticated wire
    /// records through `replica()`; it never drives another Raft controller or writes this file.
    pub fn open_replicated(
        path: &Path,
        log_path: &Path,
        config: api::replication::ReplicaConfig,
        until: Instant,
    ) -> Result<Self, Refusal> {
        Self::replicated(
            path,
            log_path,
            config,
            api::replication::Durability::Persisted,
            None,
            until,
        )
    }

    /// [`Database::open_replicated`] with `guards`, as [`Database::open_guarded`] keeps them; the
    /// log's parent directory must exist too.
    pub fn open_replicated_guarded(
        path: &Path,
        log_path: &Path,
        config: api::replication::ReplicaConfig,
        guards: Guards,
        until: Instant,
    ) -> Result<Self, Refusal> {
        Self::replicated(
            path,
            log_path,
            config,
            api::replication::Durability::Persisted,
            Some(guards),
            until,
        )
    }

    /// [`Database::open_replicated`], its writes acknowledged as `durability` says: the replicated
    /// class answers once members spanning the fates asked hold a write (docs/design/replication.md,
    /// "Fast acknowledgement: the replicated class"). A configuration whose voters span fewer fates
    /// is refused, typed.
    pub fn open_replicated_with(
        path: &Path,
        log_path: &Path,
        config: api::replication::ReplicaConfig,
        durability: api::replication::Durability,
        until: Instant,
    ) -> Result<Self, Refusal> {
        Self::replicated(path, log_path, config, durability, None, until)
    }

    fn replicated(
        path: &Path,
        log_path: &Path,
        config: api::replication::ReplicaConfig,
        durability: api::replication::Durability,
        guards: Option<Guards>,
        until: Instant,
    ) -> Result<Self, Refusal> {
        // The log's parent is made by its lease, once this process owns the database.
        let log_path = match guards {
            Some(_) => existing_path(log_path)?,
            None => resolved_path(log_path)?,
        };
        log_path
            .to_str()
            .ok_or_else(|| api::invalid("replica path", "not UTF-8"))?;
        replicated::validate(&config)?;
        let durability = canonical(durability);
        replicated::validate_durability(&config, &durability)?;
        let node_id = config.node_id;
        Self::owner(
            path,
            until,
            false,
            Some((&log_path, &config, &durability)),
            guards,
            |path, owner| {
                let changed = Arc::new(Changed::default());
                let client = replicated::start(
                    path,
                    &log_path,
                    config.clone(),
                    durability.clone(),
                    Arc::clone(&changed),
                    owner.clone(),
                    until,
                )?;
                Ok(Self {
                    shared: Arc::new(Shared {
                        transport: Mutex::new(None),
                        path: OnceLock::new(),
                        writer: Writer::Replicated {
                            client,
                            log_path: log_path.clone(),
                            node_id,
                            config: config.clone(),
                            durability: durability.clone(),
                        },
                        changed,
                        restoring: AtomicBool::new(false),
                        admitted: Arc::new(AtomicUsize::new(0)),
                        _owner: Some(owner),
                    }),
                })
            },
        )
    }

    pub fn replica(&self) -> Option<crate::replication::ReplicaHandle> {
        match &self.shared.writer {
            Writer::Replicated { client, .. } => Some(client.handle()),
            Writer::Client(backend) => backend.replica(),
            Writer::Local(_) => None,
        }
    }

    /// The same SQL/Records engine and commit pipeline on an in-memory I/O device.
    pub fn in_memory(until: Instant) -> Result<Self, Refusal> {
        let io = Arc::new(amphora_engine::MemoryIO::new());
        Self::from_db(
            Db::open_with(Path::new(":memory:amphora"), io).map_err(unavailable)?,
            until,
        )
    }

    fn from_db(db: Db, until: Instant) -> Result<Self, Refusal> {
        Self::from_db_mode(db, until, false, None)
    }

    fn from_db_mode(
        db: Db,
        until: Instant,
        restoring: bool,
        owner: Option<Owned>,
    ) -> Result<Self, Refusal> {
        bootstrap_db(&db)?;
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
        if !restoring && restore::incomplete(&db).map_err(unavailable)? {
            return Err(api::invalid(
                "database",
                "an incomplete restore must be resumed before ordinary access",
            ));
        }
        let changed = Changed::default();
        *locked(&changed.published)? = read_published(&db)?;
        let database = Self {
            shared: Arc::new(Shared {
                transport: Mutex::new(None),
                path: OnceLock::new(),
                writer: Writer::Local(
                    Store::start(Local { db, restoring }, store::Config::default())
                        .map_err(unavailable)?,
                ),
                changed: Arc::new(changed),
                restoring: AtomicBool::new(restoring),
                admitted: Arc::new(AtomicUsize::new(0)),
                _owner: owner,
            }),
        };
        // Bootstrap DDL used the same WAL; the barrier makes it durable before open succeeds.
        database.submit_selected("", Operation::Mutation(Mutation::Sql(Vec::new())), until)?;
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
        let database = self.resolve(until)?;
        if let Some(client) = database.remote_client() {
            return client.call(until, f).map_err(Refusal::from);
        }
        database.call_db(until, move |db| {
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

    /// Runs one read-only SQL session on a reader beside the writer (docs/design/store.md,
    /// decision 33): it sees what was committed when it began, waits for neither the writer nor a
    /// flush, and writes nothing, its writing statements refused. A remote or replicated database
    /// runs it as a [`Database::call`].
    pub fn query<R, F>(&self, until: Instant, f: F) -> Result<R, Refusal>
    where
        R: Send + 'static,
        F: FnOnce(&mut crate::sql::Connection<'_>) -> anyhow::Result<R> + Send + 'static,
    {
        let database = self.resolve(until)?;
        let Writer::Local(store) = &database.shared.writer else {
            return database.call(until, f);
        };
        // The store runs the session on a reader, or on the writer while it has none to lend.
        let session = Arc::new(Mutex::new(Some(f)));
        let on_writer = Arc::clone(&session);
        store
            .read_committed(
                until,
                |reader| run_query(reader, until, &session),
                move |engine| run_query(&mut engine.db, until, &on_writer),
            )
            .map_err(|e| refused(&e))
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
        self.resolve(until)?
            .submit_selected(namespace, operation, until)
    }

    fn submit_selected(
        &self,
        namespace: &str,
        operation: Operation,
        until: Instant,
    ) -> Result<Answer, Refusal> {
        if Instant::now() >= until {
            return Err(Refusal::Timeout);
        }
        if let Operation::Mutation(Mutation::Compact { before, .. }) = &operation {
            self.namespace(namespace)?.compact(*before, until)?;
            return Ok(Answer::Done);
        }
        let store = match &self.shared.writer {
            Writer::Replicated { client, .. } => {
                return client.submit(namespace.to_owned(), operation, until);
            }
            Writer::Local(store) => store,
            Writer::Client(backend) => {
                return remote::submit(&backend.client, namespace, operation, until);
            }
        };
        let admission = Admission::reserve(&self.shared.admitted)?;
        let wake = matches!(&operation, Operation::Mutation(Mutation::Records { tx, .. }) if !tx.writes.is_empty())
            || matches!(
                &operation,
                Operation::Mutation(Mutation::Compact { .. } | Mutation::CompactStep { .. })
            );
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
            Operation::Mutation(
                Mutation::Compact { namespace, .. } | Mutation::CompactStep { namespace, .. },
            ) => Some(Publish::Compact(namespace.clone())),
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
                    until,
                }],
                until,
                move |done| {
                    let _admission = admission;
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
                        Err(store::Refused::Groups) => {
                            Err(admission_refusal(store::Refused::Groups))
                        }
                        Err(_) => Err(Refusal::UnknownOutcome),
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
                                (Publish::Compact(namespace), Ok(Answer::Compacted(progress))) => {
                                    let state = published.entry(namespace).or_default();
                                    state.head = state.head.max(progress.head);
                                    state.feed = state.feed.max(progress.feed);
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
            .map_err(admission_refusal)?;
        rx.recv_timeout(until.saturating_duration_since(Instant::now()))
            .map_err(|e| match e {
                mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected => {
                    Refusal::UnknownOutcome
                }
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
            Answer::Imported(_) | Answer::Done => Ok(()),
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

    /// Checks metadata published by successful durable callbacks. Local owners do not wait for
    /// disk or the writer queue; clients have a bounded two-second owner/metadata exchange.
    pub fn try_wait(&self, family: &str, after: Rev) -> Result<Option<Rev>, Refusal> {
        let until = Instant::now()
            .checked_add(std::time::Duration::from_secs(2))
            .ok_or(Refusal::Timeout)?;
        self.resolved(until)?
            .try_wait_selected(family, after, until)
    }

    pub(super) fn resolved(&self, until: Instant) -> Result<Namespace, Refusal> {
        Ok(Namespace {
            database: self.database.resolve(until)?,
            name: self.name.clone(),
        })
    }

    pub(super) fn try_wait_selected(
        &self,
        family: &str,
        after: Rev,
        until: Instant,
    ) -> Result<Option<Rev>, Refusal> {
        api::family(family)?;
        if let Some(remote) = self.remote()? {
            return remote.try_wait(family, after, until);
        }
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
        api::compact(self, before, until)
    }
    fn compact_step(
        &self,
        before: Rev,
        until: Instant,
    ) -> Result<api::CompactionProgress, Refusal> {
        match self.database.submit(
            &self.name,
            Operation::Mutation(Mutation::CompactStep {
                namespace: self.name.clone(),
                before,
            }),
            until,
        )? {
            Answer::Compacted(progress) => Ok(progress),
            _ => Err(Refusal::UnknownOutcome),
        }
    }
    fn wait(&self, family: &str, after: Rev, until: Instant) -> Result<Rev, Refusal> {
        api::family(family)?;
        let namespace = self.resolved(until)?;
        if let Some(remote) = namespace.remote()? {
            return remote.wait(family, after, until);
        }
        loop {
            let changed = &namespace.database.shared.changed;
            let before = *locked(&changed.generation)?;
            if let Some(at) = namespace.try_wait_selected(family, after, until)? {
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
        if rows.iter().any(|r| {
            matches!(
                r.operation,
                Operation::Call(_) | Operation::Mutation(Mutation::CompactStep { .. })
            )
        }) {
            for row in rows {
                if let Operation::Mutation(Mutation::CompactStep { before, .. }) = &row.operation {
                    let result = compaction::local(
                        &mut self.db,
                        &row.namespace,
                        *before,
                        row.until,
                        restoring,
                    )
                    .map(Answer::Compacted);
                    *locked(&row.answer)? = Some(result);
                    continue;
                }
                let result = self
                    .db
                    .transaction(|db| {
                        if Instant::now() >= row.until {
                            return Err(Refusal::Timeout.into());
                        }
                        let _deadline = db.deadline(row.until);
                        if !restoring && restore::incomplete(db)? {
                            return Err(api::invalid("database", "restore is incomplete").into());
                        }
                        let result = (|| match &row.operation {
                            Operation::Call(call) => {
                                let f = locked(call)?.take().ok_or_else(|| {
                                    anyhow::anyhow!("SQL session was already executed")
                                })?;
                                let value = f(db)?;
                                *locked(&row.publication)? = Some(read_published(db)?);
                                Ok(Ok(Answer::Called(value)))
                            }
                            other => apply(db, &row.namespace, other),
                        })();
                        if result.is_err() && Instant::now() >= row.until {
                            return Err(Refusal::Timeout.into());
                        }
                        result
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
                .map(|r| {
                    if Instant::now() >= r.until {
                        return Err(Refusal::Timeout.into());
                    }
                    let _deadline = db.deadline(r.until);
                    let result = apply(db, &r.namespace, &r.operation);
                    if result.is_err() && Instant::now() >= r.until {
                        return Err(Refusal::Timeout.into());
                    }
                    result
                })
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
    crate::sql::validate_reserved_sql(sql)?;
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
        Operation::Mutation(
            Mutation::Records { .. } | Mutation::Compact { .. } | Mutation::CompactStep { .. }
        )
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
            // Four finite prepared-query shapes keep cursor/upper bounds outside OR guards.
            // A cursor below the prefix is equivalent to the initial inclusive prefix seek.
            let (lower, continued) = match scan.after.as_ref() {
                Some(after) if after >= &scan.prefix => (after.as_slice(), true),
                _ => (scan.prefix.as_slice(), false),
            };
            let query = match (continued, upper.is_some()) {
                (false, false) => {
                    "SELECT v.key, v.rev, v.value FROM __amphora_versions v WHERE v.namespace = ?1 AND v.family = ?2 AND v.key >= ?3 AND v.rev = (SELECT MAX(w.rev) FROM __amphora_versions w WHERE w.namespace = v.namespace AND w.family = v.family AND w.key = v.key AND w.rev <= ?4) AND v.value IS NOT NULL ORDER BY v.key LIMIT ?5"
                }
                (false, true) => {
                    "SELECT v.key, v.rev, v.value FROM __amphora_versions v WHERE v.namespace = ?1 AND v.family = ?2 AND v.key >= ?3 AND v.key < ?6 AND v.rev = (SELECT MAX(w.rev) FROM __amphora_versions w WHERE w.namespace = v.namespace AND w.family = v.family AND w.key = v.key AND w.rev <= ?4) AND v.value IS NOT NULL ORDER BY v.key LIMIT ?5"
                }
                (true, false) => {
                    "SELECT v.key, v.rev, v.value FROM __amphora_versions v WHERE v.namespace = ?1 AND v.family = ?2 AND v.key > ?3 AND v.rev = (SELECT MAX(w.rev) FROM __amphora_versions w WHERE w.namespace = v.namespace AND w.family = v.family AND w.key = v.key AND w.rev <= ?4) AND v.value IS NOT NULL ORDER BY v.key LIMIT ?5"
                }
                (true, true) => {
                    "SELECT v.key, v.rev, v.value FROM __amphora_versions v WHERE v.namespace = ?1 AND v.family = ?2 AND v.key > ?3 AND v.key < ?6 AND v.rev = (SELECT MAX(w.rev) FROM __amphora_versions w WHERE w.namespace = v.namespace AND w.family = v.family AND w.key = v.key AND w.rev <= ?4) AND v.value IS NOT NULL ORDER BY v.key LIMIT ?5"
                }
            };
            let mut params = vec![
                text(namespace),
                text(f),
                blob(lower)?,
                integer(at)?,
                integer(
                    u64::try_from(scan.limit)?
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!("limit overflow"))?,
                )?,
            ];
            if let Some(upper) = upper {
                params.push(blob(&upper)?);
            }
            let mut rows = Vec::new();
            db.query(query, params, |r| {
                rows.push(Row {
                    key: bytes(r, 0)?.ok_or_else(|| anyhow::anyhow!("null key"))?,
                    rev: rev(r, 1)?,
                    value: bytes(r, 2)?.ok_or_else(|| anyhow::anyhow!("null value"))?,
                });
                Ok(true)
            })?;
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
        Operation::Mutation(Mutation::Compact { .. } | Mutation::CompactStep { .. }) => {
            Err(api::invalid("compaction", "requires the bounded writer transaction").into())
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

/// Whether this is the copy of the test binary that runs `test` alone, in a process of its own: a
/// test that fills or reads the process's owner table can't share it with the tests beside it. The
/// parent runs the child and checks that it passed.
#[cfg(test)]
pub(crate) fn in_child(test: &str) -> bool {
    const CHILD: &str = "AMPHORA_DATABASE_CHILD";
    if std::env::var_os(CHILD).is_some_and(|name| name == test) {
        return true;
    }
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env(CHILD, test)
        .status()
        .unwrap();
    assert!(status.success(), "{test} in a child process: {status}");
    false
}
