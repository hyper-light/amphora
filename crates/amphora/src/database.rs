//! One product owner for SQL and namespaced Records (CP-T1.5, U05/U06/U15).
//! `open` uses the standalone durable commit pipeline; `open_replicated` uses the product's
//! hyper-durable actor, quorum commits and read fences. Other processes attach to that same owner
//! through authenticated local transport, without opening another engine or Raft controller.

use std::any::Any;
use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::sync::{OnceLock, Weak};
use std::time::{Duration, Instant};

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
use crate::reshard;
use crate::store::{self, Checkpointed, Engine, Flush, Opener, Store};

mod across;
mod application;
pub(crate) use application::Application;

/// The application floor installed in `db`, if any.
pub(crate) fn installed_floor(db: &Db) -> anyhow::Result<Option<Application>> {
    application::policy(db)
}
pub use application::WriterBoundary;

mod compacting;
pub(crate) mod compaction;
mod drain;
mod group;
mod guards;
mod import;
mod log_bound;
pub mod online;
mod pace;
mod patience;
pub(crate) mod remote;
pub(crate) mod replicated;
mod restore;
pub(crate) mod session;
mod slot;
mod watch;
pub use across::ReadPoint;
pub use guards::Guards;
pub(crate) use guards::Owned;
pub use online::SchemaChange;
pub use replicated::{LogDisk, ReadMode, SessionCounters, Token};
pub use restore::Restore;
pub use watch::Watch;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS __amphora_namespaces (name TEXT PRIMARY KEY, head INTEGER NOT NULL, horizon INTEGER NOT NULL, feed INTEGER NOT NULL, live INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS __amphora_versions (namespace TEXT NOT NULL, family TEXT NOT NULL, key BLOB NOT NULL, rev INTEGER NOT NULL, seq INTEGER NOT NULL, value BLOB, PRIMARY KEY(namespace, family, key, rev));
CREATE INDEX IF NOT EXISTS __amphora_feed ON __amphora_versions(namespace, family, rev, seq);
CREATE TABLE IF NOT EXISTS __amphora_families (namespace TEXT NOT NULL, family TEXT NOT NULL, newest INTEGER NOT NULL, PRIMARY KEY(namespace, family));
CREATE TABLE IF NOT EXISTS __amphora_migrations (name TEXT PRIMARY KEY, sql TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS __amphora_holding (one INTEGER PRIMARY KEY CHECK (one = 0), holding BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS __amphora_rev_stamps (namespace TEXT NOT NULL, rev INTEGER NOT NULL, stamp INTEGER NOT NULL, PRIMARY KEY(namespace, rev));
CREATE INDEX IF NOT EXISTS __amphora_rev_stamps_order ON __amphora_rev_stamps(namespace, stamp);
CREATE TABLE IF NOT EXISTS __amphora_moved_stamps (namespace TEXT NOT NULL, family TEXT NOT NULL, key BLOB NOT NULL, rev INTEGER NOT NULL, stamp INTEGER, PRIMARY KEY(namespace, family, key, rev));
CREATE INDEX IF NOT EXISTS __amphora_moved_stamps_order ON __amphora_moved_stamps(namespace, family, stamp);";
const LIVE_MAX: u64 = 10_000_000;
/// A request admitted to the writer's queue: what the request and its answer can hold
/// ([`Admission::bytes_for`]), charged to the memory budget's room for queues (MB.1;
/// replication.md, "Decision (lane constants): the queue bounds by drain rate"; QB.1). With no
/// room it waits for room until its deadline, woken only by charges given back and by its store's
/// end ([`Store::charge_queue`]; lane perf-A28), and is refused `Full` past it; past the budget's
/// whole limit it is refused `TooLarge` at once (lane perf-A19).
struct Admission {
    bytes: Arc<AtomicUsize>,
    charged: usize,
    _charge: crate::memory::Charge,
}
impl Admission {
    fn reserve(
        store: &Store<Local>,
        bytes: &Arc<AtomicUsize>,
        operation: &Operation,
        until: Instant,
    ) -> Result<Self, Refusal> {
        Self::charge(store, bytes, Self::bytes_for(operation), until)
    }

    fn fixed() -> usize {
        std::mem::size_of::<Command>().saturating_add(std::mem::size_of::<Answer>())
    }

    fn charge(
        store: &Store<Local>,
        bytes: &Arc<AtomicUsize>,
        charged: usize,
        until: Instant,
    ) -> Result<Self, Refusal> {
        let request = u64::try_from(charged).unwrap_or(u64::MAX);
        let charge = store.charge_queue(request, until)?;
        bytes.fetch_add(charged, Ordering::AcqRel);
        Ok(Self {
            bytes: Arc::clone(bytes),
            charged,
            _charge: charge,
        })
    }

    /// What `operation` and its answer can hold while queued and answered (AUDIT 2026-11-23 §1,
    /// MB.1): its command and answer, the bytes it carries, and the most its answer can carry by
    /// the API's own bounds. A Records get answers one row (a key of at most `KEY_MAX`, a value of
    /// at most `VALUE_MAX`); a Records write, a head, a compaction's step and a holding answer a
    /// revision or a fixed-size progress. A scan's page and a page of the changes feed hold at
    /// most their limit's rows and `RESULT_MAX` bytes of keys and values ([`fits`], lane
    /// perf-A6); an SQL batch carries its statements and parameters and answers a result per
    /// statement, their rows' bytes at most `RESULT_MAX` (Ada, AUDIT §1: their real worst case,
    /// lane perf-A5). The rest keep the full request bound, `REQUEST_MAX`: a migration, an import's
    /// page and its publication up to `REQUEST_MAX`. A call is charged the bytes its caller declared
    /// its answer holds ([`AnswerMax`], carried in `Operation::Call`). Counts saturate: a sum past the address space is
    /// refused by the budget, never wrapped.
    fn bytes_for(operation: &Operation) -> usize {
        let fixed = Self::fixed();
        let carried = match operation {
            Operation::Get(namespace, key, ..) => namespace
                .len()
                .saturating_add(key.len())
                .saturating_add(std::mem::size_of::<Row>())
                .saturating_add(api::KEY_MAX)
                .saturating_add(api::VALUE_MAX),
            Operation::Mutation(Mutation::Records { namespace, tx }) => {
                let conds = tx.conds.iter().fold(0usize, |n, cond| {
                    let (Cond::Absent { family, key } | Cond::At { family, key, .. }) = cond;
                    n.saturating_add(std::mem::size_of::<Cond>())
                        .saturating_add(family.len())
                        .saturating_add(key.len())
                });
                // A write's family is carried twice: in the write, and in the families its
                // publication names.
                let writes = tx.writes.iter().fold(0usize, |n, write| {
                    let (family, key, value) = match write {
                        Write::Put { family, key, value } => (family, key, value.len()),
                        Write::Delete { family, key } => (family, key, 0),
                    };
                    n.saturating_add(std::mem::size_of::<Write>())
                        .saturating_add(family.len().saturating_mul(2))
                        .saturating_add(key.len())
                        .saturating_add(value)
                });
                namespace
                    .len()
                    .saturating_mul(2)
                    .saturating_add(conds)
                    .saturating_add(writes)
            }
            Operation::Mutation(
                Mutation::Compact { namespace, .. } | Mutation::CompactStep { namespace, .. },
            ) => namespace.len().saturating_mul(2),
            Operation::Mutation(Mutation::Holding(holding)) => holding.len(),
            Operation::Head => 0,
            Operation::Scan(family, scan, _) => family
                .len()
                .saturating_add(scan.prefix.len())
                .saturating_add(scan.after.as_ref().map_or(0, Vec::len))
                .saturating_add(page_bytes::<Row>(scan.limit)),
            Operation::Changes(family, _, limit) => {
                family.len().saturating_add(page_bytes::<Change>(*limit))
            }
            Operation::Mutation(Mutation::Sql(statements)) => {
                let request = statements.iter().fold(0usize, |n, statement| {
                    statement.params.iter().fold(
                        n.saturating_add(std::mem::size_of::<Statement>())
                            .saturating_add(statement.sql.len()),
                        |n, value| {
                            n.saturating_add(std::mem::size_of::<Value>())
                                .saturating_add(value_bytes(value))
                        },
                    )
                });
                let results = statements
                    .len()
                    .saturating_mul(std::mem::size_of::<SqlResult>());
                request
                    .saturating_add(results)
                    .saturating_add(api::RESULT_MAX)
            }
            Operation::Call(_, answer) | Operation::Query(_, answer) => *answer,
            Operation::Mutation(Mutation::Migrate(_))
            | Operation::Import(_)
            | Operation::ImportProgress => api::REQUEST_MAX,
        };
        fixed.saturating_add(carried)
    }
}

/// Whether a page of `rows` rows carrying `bytes` bytes of keys and values takes one more row
/// carrying `payload`. A scan's page and a page of the changes feed end at their limit's rows or
/// at `RESULT_MAX` bytes, whichever comes first, and say that more follow, so the answer and what
/// admission charges for it are bounded whatever the limit (Ada, AUDIT 2026-11-23 §1: the real
/// worst case; lane perf-A6). A page's first row always fits, so a continued read advances: a
/// key and value the API admits are far under `RESULT_MAX`.
fn fits(rows: usize, bytes: usize, payload: usize, limit: usize) -> bool {
    rows < limit && (rows == 0 || bytes.saturating_add(payload) <= api::RESULT_MAX)
}

/// The most a page of up to `limit` rows of `T` holds by [`fits`]: each row's envelope, and keys
/// and values of at most `KEY_MAX` and `VALUE_MAX` a row up to `RESULT_MAX` in all.
fn page_bytes<T>(limit: usize) -> usize {
    let payload = limit
        .saturating_mul(api::KEY_MAX.saturating_add(api::VALUE_MAX))
        .min(api::RESULT_MAX);
    limit
        .saturating_mul(std::mem::size_of::<T>())
        .saturating_add(payload)
}

/// The bytes of column `n`'s blob, read in place (0 for a null), so a row past a page is weighed
/// without being copied.
fn blob_len(r: &amphora_engine::Row, n: usize) -> usize {
    r.get_value(n).to_blob().map_or(0, <[u8]>::len)
}

/// The memory budget's room for queues now, what a full writer queue was refused at.
fn queue_room() -> usize {
    crate::memory::process()
        .room(crate::memory::Use::Queues)
        .ok()
        .and_then(|room| usize::try_from(room).ok())
        .unwrap_or(0)
}
impl Drop for Admission {
    fn drop(&mut self) {
        self.bytes.fetch_sub(self.charged, Ordering::AcqRel);
    }
}

#[derive(Clone)]
pub struct Database {
    shared: Arc<Shared>,
    application: Option<Application>,
    /// The groups a namespace's rows are spread over (CP-T1.9, lane split-merge-18; the field
    /// agreed with lane xgroup-sql-6): with one, a namespace's scans, changes and head are read
    /// across every group, in the order of their commit stamps ([`Database::across`]).
    cluster: Option<Arc<crate::xgroup::sql::Cluster>>,
    /// Where a read across `cluster`'s groups takes its cut (lane split-merge-19, T-T1.9.R22).
    read_point: ReadPoint,
}

struct Shared {
    // Stop accepting before dropping the writer and its asynchronous engine lease.
    transport: Mutex<Option<crate::transport::Owner>>,
    path: OnceLock<PathBuf>,
    writer: Writer,
    changed: Arc<Changed>,
    restoring: AtomicBool,
    admitted: Arc<AtomicUsize>,
    /// A backed-up writer's stamps ([`Database::open_backed_up`]): what cross-group steps carry.
    stamps: OnceLock<Arc<crate::backup::Hooks>>,
    /// A backed-up writer's foreground, the calls it runs counted for its checks' pace
    /// (T-T1.12.C8).
    foreground: OnceLock<Arc<crate::backup::Foreground>>,
    /// Records reads waiting for a reader (AUDIT 2.5).
    reads: Reads,
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
/// The opens of this process in flight: each took its lease under [`OWNERS`] and makes its
/// database outside it, so opens of other databases run beside it (CP-T1.13). An open of a file
/// one of them reserved, by its path or by another (a hard link, a second mount, a name the
/// filesystem folds to the same), waits on [`OPENED`] until that open ends, then looks again: two
/// opens of one database still take turns. One reservation holds one lease, a descriptor of the
/// process's descriptor budget, so they are at most that budget's bound.
static OPENING: Mutex<Vec<Reservation>> = Mutex::new(Vec::new());
/// Signalled each time a reservation in [`OPENING`] ends, or an open starts waiting for one.
static OPENED: Condvar = Condvar::new();
/// The paths whose owner this process drained, the latest last. A lease there may still read held
/// for a moment: a child this process forked shares it until its exec (local-owner-transport.md,
/// "Owner guards", J1). A mark names a lease this process held, one descriptor of the process's
/// descriptor budget, so the marks kept are at most that budget's bound as it stands, the oldest
/// evicted first (local-owner-transport.md, "One descriptor budget", item 6). An evicted mark
/// makes an open that still finds the lease held refuse it as another process's, typed, rather
/// than wait. A path's mark goes once this process takes its lease again.
static DRAINED: OnceLock<Mutex<std::collections::VecDeque<PathBuf>>> = OnceLock::new();
/// The window this process last waited out for a drained path's lease, from its first probe to
/// taking it, in nanoseconds (0: none waited yet): how long a child it forked held the lease until
/// its exec, learned (J1; 62.8 µs once measured on fedora, t1/guards-flake). One last sample: it
/// bounds how late a probe is, never how long a wait lasts (the caller's deadline does).
static DRAIN_WINDOW: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
pub(crate) struct Changed {
    generation: Mutex<u64>,
    event: Condvar,
    published: Mutex<BTreeMap<String, Published>>,
    watchers: Mutex<watch::Watchers>,
    /// What a Records get of a row under a cross-group lock does ([`LockedReads`] as
    /// [`LockedReads::code`]; [`Database::set_locked_reads`]).
    locked_reads: AtomicU8,
    /// Records gets that met a cross-group lock and waited for it, once each.
    locked_waits: AtomicU64,
    /// The times a waiting get tried its row again, each wake counted: under
    /// [`LockedReads::Wait`] every change in the group wakes every waiting get, under
    /// [`LockedReads::WaitOnLock`] only its own row's resolution does (T-T1.10.W9).
    locked_wakes: AtomicU64,
    /// [`LockedReads::WaitOnLock`]'s waiter lists: each row a get waits on, by its cross-group key
    /// (`xgroup::sql::record_key`), with its own bell, rung when a resolution here drops that
    /// row's locks ([`Database::call_internal_ringing`]). A row's entry lives while a get waits
    /// on it, so it holds at most one entry per waiting get.
    lock_waiters: Mutex<BTreeMap<Vec<u8>, LockBell>>,
}

/// One row's waiter list under [`LockedReads::WaitOnLock`]: how many gets wait on it, how many
/// times a resolution rang it, and the bell they wait on (with [`Changed::lock_waiters`]'s lock).
#[derive(Default)]
struct LockBell {
    waiters: usize,
    rung: u64,
    bell: Arc<Condvar>,
}

/// A get leaving `row`'s waiter list: the list goes with its last waiter.
fn leave_list(lists: &mut BTreeMap<Vec<u8>, LockBell>, row: &[u8]) {
    if let Some(list) = lists.get_mut(row) {
        list.waiters = list.waiters.saturating_sub(1);
        if list.waiters == 0 {
            lists.remove(row);
        }
    }
}

/// What a Records get here does with a row a transaction across groups holds a lock on, between
/// its decision and this group's resolution of it (AC-T1.10.1, lane xgroup-sql-10; option 2 of
/// xgroup-sql-7's, the A/B sibling of the coordinator's `xgroup::sql::AnswerAt`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LockedReads {
    /// Today's, the control: it answers the row as last committed here, so a read straight after
    /// a cross-group answer (`AnswerAt::Decided`) misses the transaction's rows in a group that
    /// hasn't resolved it yet.
    #[default]
    Committed,
    /// It waits until the lock is gone, woken by this group's change event (each step and each
    /// commit here moves it; never a timer), then answers the row as it stands: the
    /// transaction's write once committed, the row before it once aborted. Refused `Timeout` at
    /// the read's deadline. Each read of a latest row looks the lock up first, single-group
    /// reads included. Reads at an older revision never wait: no lock is on what was committed.
    Wait,
    /// As [`LockedReads::Wait`], but a waiting get waits on its own row's waiter list, rung when
    /// a resolution here drops that row's locks, not on every change in the group (T-T1.10.W9,
    /// lane xgroup-sql-11: Wait's thundering herd at scale). The step that resolves rings it in
    /// the process that commits it (`Database::call_internal_ringing`), and every replica of a
    /// replicated group rings its own lists as it applies the resolution, whichever process ran
    /// the step (`ring_unlocked`; T-T1.10.W13, lane xgroup-sql-12).
    WaitOnLock,
}

impl LockedReads {
    /// The setting as [`Changed::locked_reads`] keeps it.
    const fn code(self) -> u8 {
        match self {
            Self::Committed => 0,
            Self::Wait => 1,
            Self::WaitOnLock => 2,
        }
    }
    const fn of(code: u8) -> Self {
        match code {
            1 => Self::Wait,
            2 => Self::WaitOnLock,
            _ => Self::Committed,
        }
    }
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
    /// A Records get, and what it does with a row under a cross-group lock ([`LockedReads`]).
    Get(String, Vec<u8>, Consistency, LockedReads),
    /// A Records scan, and what it does with a row in its range under a cross-group lock.
    Scan(String, Scan, LockedReads),
    Changes(String, Rev, usize),
    Head,
    /// A call, and the bytes its caller declared its answer holds ([`AnswerMax`]): what admission
    /// charges it, on a local writer and a replicated owner alike.
    Call(Mutex<Option<Call>>, usize),
    Import(amphora_api::import::ImportPage),
    ImportProgress,
    /// A replicated database's query ([`Database::query`]), and the bytes its answer holds: a
    /// call whose writing statements are refused.
    Query(Mutex<Option<Call>>, usize),
}

impl Operation {
    /// Whether the operation may write. A replicated owner's barrier for one that can't asks its
    /// leader for no write slot (CP-T1.8, lane V11).
    fn writes(&self) -> bool {
        match self {
            Operation::Mutation(_) | Operation::Call(..) | Operation::Import(_) => true,
            Operation::Get(..)
            | Operation::Scan(..)
            | Operation::Changes(..)
            | Operation::Head
            | Operation::ImportProgress
            | Operation::Query(..) => false,
        }
    }
}

type Call = Box<dyn FnOnce(&mut Db) -> anyhow::Result<Box<dyn Any + Send>> + Send>;

/// The most bytes a call's answer `R` holds in memory ([`AnswerBytes`]), declared by its caller
/// (AUDIT 2026-11-23 §1, MB.1): the call is charged it at admission, to the memory budget's room
/// for queues, and an answer past it is refused inside the call's transaction, before it
/// commits, so the call rolls back. The only charge a call takes: lane perf-A8's A/B (r949)
/// against a flat `REQUEST_MAX` and the answer's measured bytes, Ada's decision (lane perf-A9).
pub struct AnswerMax<R> {
    bytes: usize,
    answer: std::marker::PhantomData<fn() -> R>,
}

impl<R> Clone for AnswerMax<R> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<R> Copy for AnswerMax<R> {}

impl<R> std::fmt::Debug for AnswerMax<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("AnswerMax").field(&self.bytes).finish()
    }
}

impl<R> AnswerMax<R> {
    /// An answer of at most `bytes`, a bound the caller derives from what it reads: a count of
    /// rows, or the API's own bound on a result (`RESULT_MAX`).
    #[must_use]
    pub const fn bytes(bytes: usize) -> Self {
        Self {
            bytes,
            answer: std::marker::PhantomData,
        }
    }

    #[must_use]
    pub const fn get(self) -> usize {
        self.bytes
    }
}

impl<R: FixedAnswer> AnswerMax<R> {
    /// An answer whose type holds nothing on the heap: its type's own size, exactly what it
    /// holds.
    #[must_use]
    pub const fn fixed() -> Self {
        Self::bytes(std::mem::size_of::<R>())
    }
}

/// The bytes an answer holds in memory, inline and on the heap, checked against the bound its
/// call declared ([`AnswerMax`]).
pub trait AnswerBytes {
    fn answer_bytes(&self) -> usize;
}

/// An answer that holds nothing on the heap, so its bytes are its type's size
/// ([`AnswerMax::fixed`]).
pub trait FixedAnswer: AnswerBytes {}

macro_rules! inline_answer {
    ($($t:ty),*) => {$(
        impl AnswerBytes for $t {
            fn answer_bytes(&self) -> usize {
                std::mem::size_of::<Self>()
            }
        }
        impl FixedAnswer for $t {}
    )*};
}
inline_answer!(
    (),
    bool,
    i8,
    i16,
    i32,
    i64,
    u8,
    u16,
    u32,
    u64,
    usize,
    f64,
    Duration
);

impl AnswerBytes for String {
    fn answer_bytes(&self) -> usize {
        std::mem::size_of::<Self>().saturating_add(self.capacity())
    }
}

impl<T: AnswerBytes> AnswerBytes for Vec<T> {
    fn answer_bytes(&self) -> usize {
        let spare = self
            .capacity()
            .saturating_sub(self.len())
            .saturating_mul(std::mem::size_of::<T>());
        self.iter().fold(
            std::mem::size_of::<Self>().saturating_add(spare),
            |n, item| n.saturating_add(item.answer_bytes()),
        )
    }
}

impl<T: AnswerBytes> AnswerBytes for Option<T> {
    fn answer_bytes(&self) -> usize {
        let outer = std::mem::size_of::<Self>().saturating_sub(std::mem::size_of::<T>());
        outer.saturating_add(
            self.as_ref()
                .map_or(std::mem::size_of::<T>(), T::answer_bytes),
        )
    }
}

impl<T: FixedAnswer> FixedAnswer for Option<T> {}

macro_rules! tuple_answer {
    ($($name:ident . $index:tt),+) => {
        impl<$($name: AnswerBytes),+> AnswerBytes for ($($name,)+) {
            fn answer_bytes(&self) -> usize {
                let padding = std::mem::size_of::<Self>()
                    $(.saturating_sub(std::mem::size_of::<$name>()))+;
                padding $(.saturating_add(self.$index.answer_bytes()))+
            }
        }
        impl<$($name: FixedAnswer),+> FixedAnswer for ($($name,)+) {}
    };
}
tuple_answer!(A.0, B.1);
tuple_answer!(A.0, B.1, C.2);
tuple_answer!(A.0, B.1, C.2, D.3);

impl<T: AnswerBytes, const N: usize> AnswerBytes for [T; N] {
    fn answer_bytes(&self) -> usize {
        let inline =
            std::mem::size_of::<Self>().saturating_sub(std::mem::size_of::<T>().saturating_mul(N));
        self.iter()
            .fold(inline, |n, item| n.saturating_add(item.answer_bytes()))
    }
}

impl<T: FixedAnswer, const N: usize> FixedAnswer for [T; N] {}

impl AnswerBytes for Value {
    fn answer_bytes(&self) -> usize {
        let heap = match self {
            Value::Text(s) => s.capacity(),
            Value::Blob(b) => b.capacity(),
            _ => 0,
        };
        std::mem::size_of::<Self>().saturating_add(heap)
    }
}

impl<T: AnswerBytes, E: AnswerBytes> AnswerBytes for Result<T, E> {
    fn answer_bytes(&self) -> usize {
        let (inline, held) = match self {
            Ok(value) => (std::mem::size_of::<T>(), value.answer_bytes()),
            Err(error) => (std::mem::size_of::<E>(), error.answer_bytes()),
        };
        std::mem::size_of::<Self>()
            .saturating_sub(inline)
            .saturating_add(held)
    }
}

impl<T: FixedAnswer, E: FixedAnswer> FixedAnswer for Result<T, E> {}

impl AnswerBytes for Refusal {
    fn answer_bytes(&self) -> usize {
        let heap = match self {
            Refusal::Conflict { family, key, .. } => {
                family.capacity().saturating_add(key.capacity())
            }
            Refusal::TooLarge { what, .. } => what.capacity(),
            Refusal::Invalid { what, why } | Refusal::Serialization { what, why } => {
                what.capacity().saturating_add(why.capacity())
            }
            Refusal::Unavailable(text) | Refusal::Fenced(text) => text.capacity(),
            Refusal::Full { .. }
            | Refusal::Compacted { .. }
            | Refusal::Ahead { .. }
            | Refusal::Timeout
            | Refusal::UnknownOutcome
            | Refusal::NotLeader { .. } => 0,
        };
        std::mem::size_of::<Self>().saturating_add(heap)
    }
}

impl AnswerBytes for Row {
    fn answer_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(self.key.capacity())
            .saturating_add(self.value.capacity())
    }
}

impl AnswerBytes for api::Page {
    fn answer_bytes(&self) -> usize {
        std::mem::size_of::<Self>().saturating_add(heap_bytes(&self.rows))
    }
}

impl AnswerBytes for api::Change {
    fn answer_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(self.key.capacity())
            .saturating_add(self.value.as_ref().map_or(0, Vec::capacity))
    }
}

impl AnswerBytes for api::import::ImportProgress {
    fn answer_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(self.source.capacity())
            .saturating_add(self.cursor.capacity())
    }
}

/// The bytes `value` holds on the heap: its [`AnswerBytes`] past its own inline size, for an
/// answer type that holds it as a field.
pub(crate) fn heap_bytes<T: AnswerBytes>(value: &T) -> usize {
    value
        .answer_bytes()
        .saturating_sub(std::mem::size_of::<T>())
}

/// `value`, the answer of a call that declared `answer`; past it, refused, so the session that
/// made it rolls back.
fn within<R: AnswerBytes>(value: R, answer: AnswerMax<R>) -> Result<R, Refusal> {
    api::bound("call answer bytes", value.answer_bytes(), answer.bytes)?;
    Ok(value)
}

/// What one run of a [`Database::query_waiting`] session holds: its answer as declared, or the
/// cross-group key of the row it met under a lock, trimmed: a transaction's key, at most `KEY_MAX`
/// (each passes `api::key` in `coordinator::validate`).
fn met_max<R>(answer: AnswerMax<R>) -> AnswerMax<Result<R, Vec<u8>>> {
    let outer = std::mem::size_of::<Result<R, Vec<u8>>>();
    let read = outer
        .saturating_sub(std::mem::size_of::<R>())
        .saturating_add(answer.get());
    AnswerMax::bytes(read.max(outer.saturating_add(api::KEY_MAX)))
}

/// A call queued by [`Database::call_nowait`]: a future of its answer, woken once, when the
/// call has committed or with why it won't.
#[must_use = "a call's answer comes only through its Calling"]
pub struct Calling<R> {
    slot: Arc<Mutex<Slot<R>>>,
}

enum Slot<R> {
    Waiting(Option<std::task::Waker>),
    Answered(Result<R, Refusal>),
    Taken,
}

impl<R> std::future::Future for Calling<R> {
    type Output = Result<R, Refusal>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let mut slot = match locked(&self.slot) {
            Ok(slot) => slot,
            Err(error) => return std::task::Poll::Ready(Err(error)),
        };
        match std::mem::replace(&mut *slot, Slot::Taken) {
            Slot::Waiting(waker) => {
                let waker = match waker {
                    Some(waker) if waker.will_wake(cx.waker()) => waker,
                    _ => cx.waker().clone(),
                };
                *slot = Slot::Waiting(Some(waker));
                std::task::Poll::Pending
            }
            Slot::Answered(answer) => std::task::Poll::Ready(answer),
            Slot::Taken => {
                std::task::Poll::Ready(Err(unavailable("this call's answer was already taken")))
            }
        }
    }
}

/// The writer's side of a [`Calling`]: answers it once, `UnknownOutcome` if the store drops it
/// unanswered, so no caller waits on a call nobody will answer.
struct Answerer<R>(Option<Arc<Mutex<Slot<R>>>>);

impl<R> Answerer<R> {
    fn answer(&mut self, answer: Result<R, Refusal>) {
        let Some(slot) = self.0.take() else {
            return;
        };
        let waker = match locked(&slot) {
            Ok(mut slot) => match std::mem::replace(&mut *slot, Slot::Answered(answer)) {
                Slot::Waiting(waker) => waker,
                Slot::Answered(_) | Slot::Taken => None,
            },
            Err(_) => None,
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl<R> Drop for Answerer<R> {
    fn drop(&mut self) {
        self.answer(Err(Refusal::UnknownOutcome));
    }
}

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
    /// A get or a scan under [`LockedReads::Wait`] or [`LockedReads::WaitOnLock`] met a row under
    /// a cross-group lock, by its cross-group key: nothing read.
    Locked(Vec<u8>),
}
type Reply = Arc<Mutex<Option<Result<Answer, Refusal>>>>;
struct Command {
    namespace: String,
    application: Option<Application>,
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
            limit: queue_room(),
        },
        store::Refused::TooLarge { size, limit } => Refusal::TooLarge {
            what: "memory for a database batch".into(),
            size: usize::try_from(size).unwrap_or(usize::MAX),
            limit: usize::try_from(limit).unwrap_or(usize::MAX),
        },
        store::Refused::Timeout => Refusal::Timeout,
        ended @ (store::Refused::Disk | store::Refused::Stopped) => ended.ended(),
        store::Refused::Failed => unavailable("transaction could not be committed"),
        store::Refused::Groups => api::invalid("database batch", "cross-group submission"),
        store::Refused::Away(_) => unavailable(error),
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
    let most = crate::descriptors::process()?.bound();
    let mut drained = locked(DRAINED.get_or_init(Mutex::default))?;
    for path in gone {
        drained.retain(|marked| *marked != path);
        while drained.len() >= most && drained.pop_front().is_some() {}
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

/// The deadline passed waiting for a lease this process drained: logged with the reason, since
/// the Timeout itself carries none (an operator tells it from a slow open by this event).
fn lease_held_since_drain(path: &Path) {
    tracing::warn!(target: "amphora.records", event = "records.lease_held_since_drain", path = %path.display(), "the lease is still held since this process drained it: a child this process forked may share it until its exec");
}

/// The pause before the next probe of a drained path's lease: the probe's own cost the first
/// time, so probing takes at most as long as waiting; then twice the last (and never less than
/// the probe). It's no longer than the larger of the window the last drain took (`window`) and
/// this wait so far (`waited`): a lease let go within the learned window is taken within it, and
/// one held past it is taken within twice the wait, so a drain that outlasts the window doesn't
/// spin at the window's pace. Before any window is known it doubles to the deadline.
fn next_pause(
    last: Option<Duration>,
    probe: Duration,
    window: Option<Duration>,
    waited: Duration,
) -> Duration {
    // A probe the clock can't see takes its least step, so the pauses still grow.
    let probe = probe.max(Duration::from_nanos(1));
    match last {
        None => probe,
        Some(last) => {
            let doubled = last.saturating_mul(2).max(probe);
            window.map_or(doubled, |window| doubled.min(window.max(waited).max(probe)))
        }
    }
}

/// Parks before the next probe of a drained path's lease, no later than `until`, for
/// [`next_pause`] after `pause`, given the probe began at `probed` and the wait at `since`.
fn pause_for_lease(pause: &mut Option<Duration>, probed: Instant, since: Instant, until: Instant) {
    let nanos = DRAIN_WINDOW.load(Ordering::Relaxed);
    let window = (nanos > 0).then(|| Duration::from_nanos(nanos));
    let next = next_pause(*pause, probed.elapsed(), window, since.elapsed());
    *pause = Some(next);
    std::thread::park_timeout(until.saturating_duration_since(Instant::now()).min(next));
}

/// A drained path's lease taken after waiting since `since`: the window learned.
fn drain_window_taken(since: Instant) {
    let nanos = u64::try_from(since.elapsed().as_nanos()).unwrap_or(u64::MAX);
    DRAIN_WINDOW.store(nanos, Ordering::Relaxed);
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
    if crate::db::too_old(error) {
        return snapshot_too_old();
    }
    unavailable(format!("{error:#}"))
}
/// A read ended because its snapshot's segment was taken back for writes that needed the room
/// (replication.md, "A replica's database log bounded by what it measures", B): asked again, the
/// same read begins on a newer one.
pub(crate) fn snapshot_too_old() -> Refusal {
    Refusal::Serialization {
        what: "read".into(),
        why: "snapshot too old: the database's log took back the segment it began on, for a write that needed the room".into(),
    }
}
/// A query's session on `db`, a reader or the writer between its commits: once, before its
/// deadline, never on a database whose restore is unfinished.
fn run_query<R, F>(
    db: &mut Db,
    until: Instant,
    claim: Option<&Application>,
    session: &Mutex<Option<F>>,
) -> anyhow::Result<R>
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
        // As a replicated query is, run as a call: the handle's claim against the floor it reads.
        application::claim_holds(db, claim)?;
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
/// Where a Records read went ([`read_records`]).
enum ReadRoute {
    /// Answered on a reader.
    Answered(Result<Answer, Refusal>),
    /// The writer's to answer, the operation handed back: anything but a Records read, a read
    /// while a restore is unfinished (the writer knows whether it is the restore), or while the
    /// store has no reader to lend.
    Writer(Operation),
}

/// A Records read waiting for a reader ([`Reads`]): what it asks and, once read, its answer.
struct WaitingRead {
    namespace: String,
    operation: Mutex<Option<Operation>>,
    claim: Option<Application>,
    until: Instant,
    slot: Mutex<ReadSlot>,
    read: Condvar,
}

/// Where a waiting read is.
#[derive(Default)]
struct ReadSlot {
    /// Read: its answer, or None when the writer must answer it.
    answer: Option<Option<Result<Answer, Refusal>>>,
    /// Readers that stopped handed the reads still waiting to it, each its place among the
    /// readers: it reads them, once a hand-over, before it answers, even once its own read is
    /// answered (another reader may take it meanwhile). Counted: two readers may hand over to the
    /// same first waiting read before it wakes, and neither place is lost.
    leads: usize,
}

/// The Records reads waiting for a reader, and how many readers are reading them (AUDIT 2.5). A
/// read that finds fewer reading than the store has readers reads, taking every read waiting
/// with it into one read transaction, as the writer takes every queued write into one commit; one
/// that finds them all reading waits to be taken. A reader whose own read is answered stops after
/// its batch, handing what still waits to the first waiting (so no caller reads for others
/// without end), and one that finds none waiting stops: while reads wait, one of their callers
/// reads.
#[derive(Default)]
struct Reads {
    queue: Mutex<ReadQueue>,
}

#[derive(Default)]
struct ReadQueue {
    waiting: Vec<Arc<WaitingRead>>,
    reading: usize,
}

/// A Records read (get, scan, changes, head) answered on a reader beside the writer (AUDIT 2.5):
/// no writer transaction, no group log entry, no flush of its own. It keeps the writer path's
/// guarantees: it reads once every write handed over before it is committed (read-your-writes,
/// the order a writer batch gave it), on a snapshot taken after that, and answers once what it
/// read is durable, as an answer given after its batch's flush was. Reads waiting together share
/// one read transaction on one reader ([`Reads`]).
fn read_records(
    store: &Store<Local>,
    reads: &Reads,
    namespace: &str,
    operation: Operation,
    claim: &Option<Application>,
    until: Instant,
) -> Result<ReadRoute, Refusal> {
    if !matches!(
        operation,
        Operation::Get(..) | Operation::Scan(..) | Operation::Changes(..) | Operation::Head
    ) {
        return Ok(ReadRoute::Writer(operation));
    }
    store.committed(None, until).map_err(admission_refusal)?;
    let mine = Arc::new(WaitingRead {
        namespace: namespace.to_owned(),
        operation: Mutex::new(Some(operation)),
        claim: claim.clone(),
        until,
        slot: Mutex::new(ReadSlot::default()),
        read: Condvar::new(),
    });
    let reader = {
        let mut queue = locked(&reads.queue)?;
        queue.waiting.push(Arc::clone(&mine));
        let reader = queue.reading < store.readers();
        if reader {
            queue.reading = queue.reading.saturating_add(1);
        }
        reader
    };
    if reader {
        read_waiting(store, reads, &mine)?;
    }
    let answer = loop {
        let mut slot = locked(&mine.slot)?;
        if slot.leads > 0 {
            slot.leads = slot.leads.saturating_sub(1);
            drop(slot);
            read_waiting(store, reads, &mine)?;
            continue;
        }
        if let Some(answer) = slot.answer.take() {
            break answer;
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            drop(slot);
            return read_late(store, reads, &mine);
        }
        drop(
            mine.read
                .wait_timeout(slot, left)
                .map_err(|_| unavailable("a database response lock was poisoned"))?,
        );
    };
    let Some(result) = answer else {
        let operation = locked(&mine.operation)?
            .take()
            .ok_or_else(|| unavailable("a read was handed to the writer without its operation"))?;
        return Ok(ReadRoute::Writer(operation));
    };
    match store.sync(until) {
        Ok(()) => Ok(ReadRoute::Answered(result)),
        Err(store::Refused::Timeout) => Err(Refusal::Timeout),
        Err(_) => Err(Refusal::UnknownOutcome),
    }
}

/// A read whose deadline passed while it waited: it leaves the queue (nothing was written: it is
/// refused for its deadline), unless it was handed the reads still waiting, which it reads first.
fn read_late(
    store: &Store<Local>,
    reads: &Reads,
    mine: &Arc<WaitingRead>,
) -> Result<ReadRoute, Refusal> {
    let lead = {
        let mut queue = locked(&reads.queue)?;
        queue.waiting.retain(|w| !Arc::ptr_eq(w, mine));
        // A hand-over is made under the queue's lock: none comes after this.
        std::mem::take(&mut locked(&mine.slot)?.leads)
    };
    for _ in 0..lead {
        read_waiting(store, reads, mine)?;
    }
    Err(Refusal::Timeout)
}

/// Reads the reads waiting, a batch of one read transaction at a time, until none waits or
/// `mine` is answered; then stops reading, handing what still waits to the first of it. Every
/// read it took is answered, whatever happens to its batch.
fn read_waiting(store: &Store<Local>, reads: &Reads, mine: &WaitingRead) -> Result<(), Refusal> {
    loop {
        let batch = {
            let mut queue = locked(&reads.queue)?;
            let answered = locked(&mine.slot)?.answer.is_some();
            match queue.waiting.first() {
                None => {
                    queue.reading = queue.reading.saturating_sub(1);
                    return Ok(());
                }
                Some(next) if answered => {
                    let mut slot = locked(&next.slot)?;
                    slot.leads = slot.leads.saturating_add(1);
                    drop(slot);
                    next.read.notify_all();
                    return Ok(());
                }
                Some(_) => std::mem::take(&mut queue.waiting),
            }
        };
        let answers = read_batch(store, &batch);
        let mut answers = match answers {
            Ok(Some(answers)) => answers.into_iter().map(Some).collect::<Vec<_>>(),
            Ok(None) => batch.iter().map(|_| None).collect(),
            Err(refusal) => batch.iter().map(|_| Some(Err(refusal.clone()))).collect(),
        }
        .into_iter();
        for waiting in &batch {
            let answer = answers
                .next()
                .unwrap_or_else(|| Some(Err(unavailable("a read went unanswered"))));
            if let Ok(mut slot) = waiting.slot.lock() {
                slot.answer = Some(answer);
            }
            waiting.read.notify_all();
        }
    }
}

/// `batch` read in one read transaction on a reader: each read's answer, in order; None when the
/// writer must answer them (a restore unfinished, or no reader now).
fn read_batch(
    store: &Store<Local>,
    batch: &[Arc<WaitingRead>],
) -> Result<Option<Vec<Result<Answer, Refusal>>>, Refusal> {
    let until = batch
        .iter()
        .map(|w| w.until)
        .max()
        .unwrap_or_else(Instant::now);
    store
        .read_committed(
            until,
            |db| {
                db.query_transaction(|db| {
                    if restore::incomplete(db)? {
                        return Ok(None);
                    }
                    let mut answers = Vec::with_capacity(batch.len());
                    for waiting in batch {
                        answers.push(read_one(db, waiting));
                    }
                    Ok(Some(answers))
                })
            },
            |_| Ok(None),
        )
        .map_err(|e| match e.downcast_ref::<store::Refused>() {
            Some(store_refused) => admission_refusal(*store_refused),
            None => refused(&e),
        })
}

/// One read of a batch, on the batch's snapshot, its own deadline and claim checked.
fn read_one(db: &mut Db, waiting: &WaitingRead) -> Result<Answer, Refusal> {
    if Instant::now() >= waiting.until {
        return Err(Refusal::Timeout);
    }
    let operation = locked(&waiting.operation)?;
    let Some(operation) = operation.as_ref() else {
        return Err(unavailable("a read was taken without its operation"));
    };
    let _deadline = db.deadline(waiting.until);
    let result = application::check(db, waiting.claim.as_ref(), operation)
        .and_then(|()| apply(db, &waiting.namespace, operation))
        .unwrap_or_else(|e| Err(refused(&e)));
    if result.is_err() && Instant::now() >= waiting.until {
        return Err(Refusal::Timeout);
    }
    result
}
/// A read's answer and the time it was answered at (replication.md, "Follower reads at a closed
/// time"): the state it read is its group's at that time, every write at it or before and none
/// after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedRead<R> {
    pub value: R,
    /// Microseconds since the Unix epoch, on the group's own time.
    pub micros: u64,
}
/// [`run_query`] at this reader's closed time: the session runs only if the state the
/// transaction reads is at `at_least` or later. `Err` with the time it found (none before its
/// group's first stamped entry) if it is earlier; the session is left for the next try.
fn run_closed<R, F>(
    db: &mut Db,
    at_least: u64,
    until: Instant,
    claim: Option<&Application>,
    session: &Mutex<Option<F>>,
) -> anyhow::Result<Result<ClosedRead<R>, Option<u64>>>
where
    F: FnOnce(&mut crate::sql::Connection<'_>) -> anyhow::Result<R>,
{
    if Instant::now() >= until {
        return Err(Refusal::Timeout.into());
    }
    if restore::incomplete(db)? {
        return Err(api::invalid("database", "restore is incomplete").into());
    }
    let done = db.query_transaction(|db| {
        // The transaction's first read takes its snapshot: the time is the state's own.
        let time = crate::replication::closed::recorded(db)?;
        let micros = match time {
            Some(micros) if micros >= at_least => micros,
            found => return Ok(Err(found)),
        };
        let f = locked(session)?
            .take()
            .ok_or_else(|| anyhow::anyhow!("SQL query was already executed"))?;
        let _deadline = db.deadline(until);
        application::claim_holds(db, claim)?;
        let mut connection = crate::sql::Connection::new(db);
        let value = f(&mut connection)?;
        connection.finish()?;
        Ok(Ok(ClosedRead { value, micros }))
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
/// The owner table held, and the identity of the file an open is about.
type OwnersHeld = (
    std::sync::MutexGuard<'static, BTreeMap<PathBuf, Owner>>,
    Option<amphora_engine::io::FileId>,
);
/// The owner table, locked once no open of this process in flight has reserved the file at
/// `path`, and that file's identity: an open in flight there is waited for, woken when it ends, to
/// the deadline (CP-T1.13).
fn owners_unreserved(path: &Path, until: Instant) -> Result<OwnersHeld, Refusal> {
    loop {
        let owners = locked_until(OWNERS.get_or_init(Mutex::default), until)?;
        let file = file_identity(path)?;
        let lease = file_identity(&crate::transport::lease_file(path))?;
        let mut opening = locked(&OPENING)?;
        let mut reserved = None;
        for reservation in opening.iter() {
            if reservation.names(path, file, lease)? {
                reserved = Some(reservation.path.clone());
                break;
            }
        }
        let Some(reserved) = reserved else {
            return Ok((owners, file));
        };
        drop(owners);
        let mut waiting = 0;
        if let Some(reservation) = ongoing(&mut opening, &reserved) {
            reservation.waiting = reservation.waiting.saturating_add(1);
            waiting = reservation.waiting;
        }
        tracing::debug!(target: "amphora.records", event = "records.open_waits_for_open_in_flight", path = %path.display(), reserved = %reserved.display(), waiting, "an open of this file is in flight in this process: this one waits for it to end");
        OPENED.notify_all();
        while ongoing(&mut opening, &reserved).is_some() {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                if let Some(reservation) = ongoing(&mut opening, &reserved) {
                    reservation.waiting = reservation.waiting.saturating_sub(1);
                }
                return Err(Refusal::Timeout);
            }
            opening = OPENED
                .wait_timeout(opening, left)
                .map_err(|_| unavailable("a database response lock was poisoned"))?
                .0;
        }
    }
}

/// The reservation of `path` in [`OPENING`], while its open is in flight.
fn ongoing<'a>(opening: &'a mut [Reservation], path: &Path) -> Option<&'a mut Reservation> {
    opening
        .iter_mut()
        .find(|reservation| reservation.path == path)
}

/// An open in flight, in [`OPENING`]: the path it reserved and what else names its file.
struct Reservation {
    path: PathBuf,
    /// The database file's identity as it was reserved; None for one its open makes.
    file: Option<amphora_engine::io::FileId>,
    /// The identity of its lease's file, which every path to this database shares.
    lease: Option<amphora_engine::io::FileId>,
    /// The opens waiting for this one to end (records.open_waits_for_open_in_flight).
    waiting: usize,
}

impl Reservation {
    /// Whether an open of `path`, whose file and lease have these identities, reaches this
    /// reservation's database: by its path, its lease, or its file, which the open in flight may
    /// have made since it reserved it (a hard link to it made meanwhile).
    fn names(
        &self,
        path: &Path,
        file: Option<amphora_engine::io::FileId>,
        lease: Option<amphora_engine::io::FileId>,
    ) -> Result<bool, Refusal> {
        if self.path == path || (lease.is_some() && lease == self.lease) {
            return Ok(true);
        }
        let Some(file) = file else {
            return Ok(false);
        };
        let reserved = match self.file {
            Some(reserved) => Some(reserved),
            None => file_identity(&self.path)?,
        };
        Ok(reserved == Some(file))
    }
}

/// A path reserved in [`OPENING`] while its database is made; dropping it ends the reservation
/// and wakes the opens waiting for it.
struct Reserved(PathBuf);

impl Reserved {
    /// Reserves `path`, the lease of which the caller just took; the caller holds the owner table
    /// and found no reservation naming the path's file.
    fn take(path: &Path) -> Result<Self, Refusal> {
        let reservation = Reservation {
            path: path.to_path_buf(),
            file: file_identity(path)?,
            lease: file_identity(&crate::transport::lease_file(path))?,
            waiting: 0,
        };
        locked(&OPENING)?.push(reservation);
        Ok(Self(path.to_path_buf()))
    }
}

impl Drop for Reserved {
    fn drop(&mut self) {
        OPENING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|reservation| reservation.path != self.0);
        OPENED.notify_all();
    }
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
    db.exec(group::SCHEMA).map_err(unavailable)?;
    // Every group keeps the tables of transactions across groups, so a single-group commit can
    // respect their locks (AC-T1.10.1, lane xgroup-sql-6).
    db.exec(crate::xgroup::sql::SCHEMA).map_err(unavailable)?;
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

/// Rings the waiter list of each of `keys` a get waits on ([`LockedReads::WaitOnLock`]).
fn ring(changed: &Changed, keys: &[Vec<u8>]) -> Result<(), Refusal> {
    if keys.is_empty() {
        return Ok(());
    }
    let mut lists = locked(&changed.lock_waiters)?;
    for key in keys {
        if let Some(list) = lists.get_mut(key) {
            list.rung = list.rung.wrapping_add(1);
            list.bell.notify_all();
        }
    }
    Ok(())
}

/// A replica's ring ([`LockedReads::WaitOnLock`]; T-T1.10.W13, lane xgroup-sql-12): once its
/// owner applied entries, on that applied state `db`, it rings the list of every row a get here
/// waits on whose write lock is gone, so a replica applying a resolution rings its own readers,
/// whichever process ran the resolving step. Each row is looked up once, however many gets wait
/// on it, and only rows a get waits on: none, none looked up. A get that joins a list after the
/// rows were taken tries its row once more first ([`Database::wait_on_lock`]), so it reads a
/// resolution applied before that try, and the next application rings it for one after.
pub(crate) fn ring_unlocked(db: &Db, changed: &Changed) -> Result<(), Refusal> {
    let rows: Vec<Vec<u8>> = locked(&changed.lock_waiters)?.keys().cloned().collect();
    let mut gone = Vec::new();
    for row in rows {
        // A get joined a list only after it found its row locked in these tables, so they exist.
        if !crate::xgroup::sql::held(db, &row).map_err(|e| unavailable(format_args!("{e:#}")))? {
            gone.push(row);
        }
    }
    ring(changed, &gone)
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
        let mut pause = None;
        let mut waited: Option<Instant> = None;
        loop {
            let probed = Instant::now();
            if probed >= until {
                if waited.is_some() {
                    lease_held_since_drain(&path);
                }
                return Err(Refusal::Timeout);
            }
            let (mut owners, identity) = owners_unreserved(&path, until)?;
            forget_drained(&mut owners)?;
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
                                return Ok(Self {
                                    shared,
                                    application: None,
                                    cluster: None,
                                    read_point: ReadPoint::Latest,
                                });
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
                        return Ok(Self {
                            shared,
                            application: None,
                            cluster: None,
                            read_point: ReadPoint::Latest,
                        });
                    } else {
                        return Ok(Self {
                            shared,
                            application: None,
                            cluster: None,
                            read_point: ReadPoint::Latest,
                        });
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
            // The owner table holds no count of its own: each owner charges the descriptor it
            // holds, its lease or, a client's, the socket its calls open, to the process's
            // descriptor budget (local-owner-transport.md, "One descriptor budget", item 6).
            let descriptor = crate::descriptors::process()?.take("open descriptors", 1)?;
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
                    let since = *waited.get_or_insert(probed);
                    pause_for_lease(&mut pause, probed, since, until);
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
                match remote::attach(&path, replica, descriptor, until) {
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
            if let Some(since) = waited {
                drain_window_taken(since);
            }
            let key = guards.as_ref().map(Guards::key);
            let owner = Owned::new(Arc::new(lease), descriptor, guards);
            // The lease is this open's: reserve the path and make the database outside the owner
            // table, so other databases' opens go on meanwhile (CP-T1.13).
            let reserved = Reserved::take(&path)?;
            drop(owners);
            let database = create(&path, owner.clone())?;
            remote::publish(&database, &path)?;
            let identity = file_identity(&path)?;
            let mut owners = locked(OWNERS.get_or_init(Mutex::default))?;
            owners.insert(
                path,
                Owner {
                    shared: Arc::downgrade(&database.shared),
                    lease: Arc::downgrade(owner.lease()),
                    identity,
                    key,
                },
            );
            drop(reserved);
            return Ok(database);
        }
    }

    pub fn open(path: &Path, until: Instant) -> Result<Self, Refusal> {
        Self::open_owned(path, until, false, None, |_| Ok(()))
    }

    /// [`Database::open`], its writer backed up to `archive` (CP-T1.12; docs/design/backup.md):
    /// each commit stamped as it is made, the log started again only once shipped, and a thread
    /// that ships the log and copies bases as the database commits, until the database ends. A
    /// live owner of the database in this process is refused: it must drain first, as only the
    /// writer's own open can back it up.
    pub fn open_backed_up(
        path: &Path,
        archive: crate::backup::Archive,
        until: Instant,
    ) -> Result<(Self, crate::backup::Shipping), Refusal> {
        let mut prepared = None;
        let database = Self::open_owned(path, until, false, None, |db| {
            prepared = Some(
                crate::backup::Prepared::on(db, path, archive, until).map_err(|e| refused(&e))?,
            );
            Ok(())
        })?;
        let (Some(prepared), Writer::Local(store)) = (prepared, &database.shared.writer) else {
            return Err(api::invalid(
                "backup",
                "a live owner of this database isn't backed up: it must drain first",
            ));
        };
        // A second set can't happen: the database was made just now, once.
        let _ = database.shared.stamps.set(prepared.hooks());
        let _ = database.shared.foreground.set(prepared.foreground());
        let shipping = prepared
            .start(|watcher| store.on_commit(watcher))
            .map_err(|e| refused(&e))?;
        Ok((database, shipping))
    }

    /// The stamps of this database's commits, if it is backed up here (T-T1.12.X2).
    pub(crate) fn stamps(&self) -> Option<&Arc<crate::backup::Hooks>> {
        self.shared.stamps.get()
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
                    application: None,
                    cluster: None,
                    read_point: ReadPoint::Latest,
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
                        stamps: OnceLock::new(),
                        foreground: OnceLock::new(),
                        reads: Reads::default(),
                        _owner: Some(owner),
                    }),
                })
            },
        )
    }

    /// Sets the disk under the replicated log open at `log_path` as a fire drill says (CP-T1.13,
    /// T-T1.13.1; `amphora serve`'s `disk cut|hang|back`): cut, every read, write and flush of that
    /// log fails with an I/O error and its replica fences on the first, as when a real disk goes
    /// away; hung, each waits until the disk is back or cut, as when a disk stops answering; a
    /// member opened again finds its disk back. The member's database files aren't touched: every
    /// write is made durable in the log first. Refused when no replicated log of this process is
    /// open at `log_path`.
    pub fn set_log_disk(log_path: &Path, disk: LogDisk) -> Result<(), Refusal> {
        replicated::set_disk(&resolved_path(log_path)?, disk)
    }

    /// Waits until the hung disk under the replicated log at `log_path` holds an operation, at
    /// most until `until`, and answers how many it holds (CP-T1.13, T-T1.13.1; `amphora serve`'s
    /// `disk held`): a drill's sign that its hang holds the load, woken by the held operation.
    /// Answers at once when the disk isn't hung.
    pub fn log_disk_held(log_path: &Path, until: Instant) -> Result<u64, Refusal> {
        replicated::disk_held(&resolved_path(log_path)?, until)
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
            application: None,
            cluster: None,
            read_point: ReadPoint::Latest,
            shared: Arc::new(Shared {
                transport: Mutex::new(None),
                path: OnceLock::new(),
                // Every request is admitted by its charge to the memory budget's room for queues
                // (MB.1), so its store charges its commands nothing more (`Local::row_bytes`).
                writer: Writer::Local(
                    Store::start(Local { db, restoring }, store::Config::default())
                        .map_err(unavailable)?,
                ),
                changed: Arc::new(changed),
                restoring: AtomicBool::new(restoring),
                admitted: Arc::new(AtomicUsize::new(0)),
                stamps: OnceLock::new(),
                foreground: OnceLock::new(),
                reads: Reads::default(),
                _owner: owner,
            }),
        };
        // Bootstrap DDL used the same WAL; the barrier makes it durable before open succeeds.
        database.submit_selected("", Operation::Mutation(Mutation::Sql(Vec::new())), until)?;
        Ok(database)
    }

    /// Lets this handle go and waits, until `until` at most, for the owner's orderly end: every other
    /// handle in the process gone, what was written durable, and the file closed cleanly, so that
    /// plain SQLite reads it (docs/design/store.md, decision 45). An owner whose last handle is let
    /// go without it ends the same way, unwaited for. Refused [`Refusal::Timeout`] past `until`, and
    /// why the file wasn't closed cleanly: it then stays as a kill leaves it, recovered as it is
    /// opened. A client of another process's owner closes nothing.
    pub fn close(self, until: Instant) -> Result<(), Refusal> {
        let end = match &self.shared.writer {
            Writer::Local(store) => Some(store.ending()),
            Writer::Client(_) | Writer::Replicated { .. } => None,
        };
        drop(self);
        let Some(end) = end else {
            return Ok(());
        };
        end.wait(until)
            .map_err(|error| match error.downcast_ref::<store::Refused>() {
                Some(store::Refused::Timeout) => Refusal::Timeout,
                Some(refused) => refused.ended(),
                None => unavailable(format!("{error:#}")),
            })
    }

    /// This database's handle over `cluster`'s groups (lane split-merge-18, T-T1.9.R20): its
    /// namespaces' scans, changes and heads read every group, each revision in the order of its
    /// commit stamp, the hybrid logical clock the cross-group steps carry
    /// (docs/design/reshard.md, "Decision (lane split-merge-18): reads across groups by commit
    /// stamp"). Its `Records::commit` commits across them through the coordinator, answering the
    /// transaction's one commit stamp (lane split-merge-22, T-T1.9.R29). Every group must be backed
    /// up, so that it stamps its commits. The groups' own handles are the ones the cluster was
    /// opened over, without it.
    #[must_use]
    pub fn across(&self, cluster: Arc<crate::xgroup::sql::Cluster>) -> Self {
        self.across_at(cluster, ReadPoint::Latest)
    }

    /// [`Database::across`], each read's cut taken at `point` (lane split-merge-19, T-T1.9.R22):
    /// the A/B of decision "Decision (lane split-merge-19): the read point, A/B", until the lab's
    /// numbers keep one.
    #[must_use]
    pub fn across_at(&self, cluster: Arc<crate::xgroup::sql::Cluster>, point: ReadPoint) -> Self {
        Self {
            cluster: Some(cluster),
            read_point: point,
            ..self.clone()
        }
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
    /// `answer`: the most bytes its answer holds, charged at admission; an answer past it rolls
    /// the call back, refused ([`AnswerMax`]).
    pub fn call<R, F>(&self, until: Instant, answer: AnswerMax<R>, f: F) -> Result<R, Refusal>
    where
        R: AnswerBytes + Send + 'static,
        F: FnOnce(&mut crate::sql::Connection<'_>) -> anyhow::Result<R> + Send + 'static,
    {
        self.call_session(until, answer, f).map(|(value, _)| value)
    }

    /// [`Database::call`], answered with its write's own session token (replication.md,
    /// "Decision (lane ryw-1): read-your-writes by session tokens"; kiln goal 8119f25d,
    /// AC-T1.10.R4): the index and time of the entry its group's log took for it, so a read that
    /// carries it ([`Database::query_session`]) waits for this write and for no later one, whoever
    /// else writes. A session keeps the latest of its tokens (`Token::after`). None standalone,
    /// for an attached client (whose reads all see every write) and for a session that wrote
    /// nothing.
    pub fn call_session<R, F>(
        &self,
        until: Instant,
        answer: AnswerMax<R>,
        f: F,
    ) -> Result<(R, Option<Token>), Refusal>
    where
        R: AnswerBytes + Send + 'static,
        F: FnOnce(&mut crate::sql::Connection<'_>) -> anyhow::Result<R> + Send + 'static,
    {
        self.session(until, answer.bytes, move |connection| {
            Ok(within(f(connection)?, answer)?)
        })
    }

    /// One SQL session on the writer, or on an attached client's owner. `answer`: the bytes its
    /// answer is declared to hold, charged at admission ([`AnswerMax`]).
    fn session<R, F>(
        &self,
        until: Instant,
        answer: usize,
        f: F,
    ) -> Result<(R, Option<Token>), Refusal>
    where
        R: Send + 'static,
        F: FnOnce(&mut crate::sql::Connection<'_>) -> anyhow::Result<R> + Send + 'static,
    {
        let database = self.resolve(until)?;
        if let Some(client) = database.remote_client() {
            return client
                .call(until, f)
                .map(|value| (value, None))
                .map_err(Refusal::from);
        }
        database.call_db_session(until, answer, move |db| {
            let mut connection = crate::sql::Connection::new(db);
            let value = f(&mut connection)?;
            connection.finish()?;
            Ok(value)
        })
    }

    /// [`Database::call`] without blocking its caller (lane perf-A7): admitted and queued at
    /// once, its answer handed to the returned [`Calling`], a future woken once the call has
    /// committed or with why it won't, so a pool of a few threads holds any number of calls in
    /// flight. Admission refuses here, typed: past the budget's room for queues (`Full`), the only
    /// bound on the writer's queue (lane perf-A8). Only a local owner queues a call with no
    /// thread waiting on it: an attached client or a replicated database is refused here
    /// ([`Database::call`] answers there). A dropped [`Calling`] leaves the call to run and
    /// commit.
    pub fn call_nowait<R, F>(
        &self,
        until: Instant,
        answer: AnswerMax<R>,
        f: F,
    ) -> Result<Calling<R>, Refusal>
    where
        R: AnswerBytes + Send + 'static,
        F: FnOnce(&mut crate::sql::Connection<'_>) -> anyhow::Result<R> + Send + 'static,
    {
        if Instant::now() >= until {
            return Err(Refusal::Timeout);
        }
        let call: Call = Box::new(move |db| {
            let mut connection = crate::sql::Connection::new(db);
            let value = within(f(&mut connection)?, answer)?;
            connection.finish()?;
            Ok(Box::new(value) as Box<dyn Any + Send>)
        });
        let slot = Arc::new(Mutex::new(Slot::Waiting(None)));
        let mut answerer = Answerer(Some(Arc::clone(&slot)));
        self.submit_local(
            "",
            Operation::Call(Mutex::new(Some(call)), answer.bytes),
            until,
            Instant::now(),
            move |result: Result<Answer, Refusal>| {
                answerer.answer(result.and_then(|answer| {
                    match answer {
                        Answer::Called(v) => v
                            .downcast::<R>()
                            .map(|v| *v)
                            .map_err(|_| unavailable("wrong SQL session answer")),
                        _ => Err(unavailable("wrong SQL session answer")),
                    }
                }));
            },
        )?;
        Ok(Calling { slot })
    }

    /// [`Database::call`] on the owner's own session, its SQL free to name Amphora's own tables
    /// (`__amphora_*`): for a cross-group transaction's steps (CP-T1.10, `crate::xgroup`). An
    /// attached client is refused: its closure can't cross to the owner.
    pub(crate) fn call_internal<R, F>(
        &self,
        until: Instant,
        answer: AnswerMax<R>,
        f: F,
    ) -> Result<R, Refusal>
    where
        R: AnswerBytes + Send + 'static,
        F: FnOnce(&mut Db) -> anyhow::Result<R> + Send + 'static,
    {
        let database = self.resolve(until)?;
        if database.remote_client().is_some() {
            return Err(api::invalid(
                "cross-group step",
                "an attached client can't run Amphora's own SQL; run it on the owner",
            ));
        }
        let (value, write) =
            database.call_db_session(until, answer.get(), move |db| Ok(within(f(db)?, answer)?))?;
        if let (Writer::Replicated { client, .. }, Some(write)) = (&database.shared.writer, write) {
            // A follower can answer on the group's commit before its own apply. The step's
            // subsequent clock read must cover the applied entry, including any HLC lift.
            client
                .wait_closed(write, None, until)
                .map_err(|error| match error {
                    Refusal::Timeout => Refusal::UnknownOutcome,
                    other => other,
                })?;
        }
        Ok(value)
    }

    /// [`Database::call_internal`] for a cross-group step, `f` answering its value and the
    /// cross-group keys of the rows whose locks it dropped: once the step commits here, those
    /// rows' waiter lists are rung ([`LockedReads::WaitOnLock`]) before its answer is handed
    /// over, whether or not its caller still waits for it. A step answered too late for its
    /// caller (`UnknownOutcome`) has dropped its locks all the same, and the step sent again finds
    /// none left to drop, so a ring left to the caller never came (T-T1.10.W7, T-T1.10.W11). A
    /// replicated group rings once answered, as its replicas ring their own lists on applying it
    /// (`ring_unlocked`). A row no get waits on rings no one.
    pub(crate) fn call_internal_ringing<R, F>(
        &self,
        until: Instant,
        answer: AnswerMax<(R, Vec<Vec<u8>>)>,
        f: F,
    ) -> Result<R, Refusal>
    where
        R: AnswerBytes + Send + 'static,
        F: FnOnce(&mut Db) -> anyhow::Result<(R, Vec<Vec<u8>>)> + Send + 'static,
    {
        let database = self.resolve(until)?;
        if !matches!(database.shared.writer, Writer::Local(_)) {
            let (value, dropped) = database.call_internal(until, answer, f)?;
            ring(&database.shared.changed, &dropped)?;
            return Ok(value);
        }
        if Instant::now() >= until {
            return Err(Refusal::Timeout);
        }
        let call: Call = Box::new(move |db| {
            let value = within(f(db)?, answer)?;
            Ok(Box::new(value) as Box<dyn Any + Send>)
        });
        let changed = Arc::clone(&database.shared.changed);
        let (tx, rx) = mpsc::sync_channel(1);
        database.submit_local(
            "",
            Operation::Call(Mutex::new(Some(call)), answer.get()),
            until,
            until,
            move |result| {
                let answer = result.and_then(|answer| match answer {
                    Answer::Called(v) => v
                        .downcast::<(R, Vec<Vec<u8>>)>()
                        .map_err(|_| unavailable("wrong SQL session answer"))
                        .and_then(|v| {
                            let (value, dropped) = *v;
                            ring(&changed, &dropped).map(|()| value)
                        }),
                    _ => Err(unavailable("wrong SQL session answer")),
                });
                // A caller whose deadline elapsed no longer waits; its rows were rung all the same.
                let _ = tx.send(answer);
            },
        )?;
        rx.recv_timeout(until.saturating_duration_since(Instant::now()))
            .map_err(|_| Refusal::UnknownOutcome)?
    }

    /// A call on the owner's own session. `answer`: the most bytes its answer holds, charged at
    /// admission; an answer past it rolls the call back, refused ([`AnswerMax`]).
    fn call_db<R, F>(&self, until: Instant, answer: AnswerMax<R>, f: F) -> Result<R, Refusal>
    where
        R: AnswerBytes + Send + 'static,
        F: FnOnce(&mut Db) -> anyhow::Result<R> + Send + 'static,
    {
        self.call_db_session(until, answer.get(), move |db| Ok(within(f(db)?, answer)?))
            .map(|(value, _)| value)
    }

    /// `answer`: the bytes the call's answer is declared to hold, charged at admission
    /// ([`AnswerMax`]).
    fn call_db_session<R, F>(
        &self,
        until: Instant,
        answer: usize,
        f: F,
    ) -> Result<(R, Option<Token>), Refusal>
    where
        R: Send + 'static,
        F: FnOnce(&mut Db) -> anyhow::Result<R> + Send + 'static,
    {
        let call: Call = Box::new(move |db| f(db).map(|v| Box::new(v) as Box<dyn Any + Send>));
        let operation = Operation::Call(Mutex::new(Some(call)), answer);
        match self.resolve(until)?.submit_selected("", operation, until)? {
            (Answer::Called(v), token) => v
                .downcast::<R>()
                .map(|v| (*v, token))
                .map_err(|_| unavailable("wrong SQL session answer")),
            _ => Err(unavailable("wrong SQL session answer")),
        }
    }

    /// Runs one read-only SQL session on a reader beside the writer (docs/design/store.md,
    /// decision 33): it sees what was committed when it began, waits for neither the writer nor a
    /// flush, and writes nothing, its writing statements refused. A replicated database runs it
    /// past its group's barrier, on its replica's applied state, its writing statements refused
    /// as here; its barrier asks the leader for no write slot (CP-T1.8, lane V11). An attached
    /// client runs it as a [`Database::call`]. `answer`: the most bytes its answer holds, derived
    /// from what it reads, charged at admission where it runs as a replicated query or a call; an
    /// answer past it is refused ([`AnswerMax`]).
    pub fn query<R, F>(&self, until: Instant, answer: AnswerMax<R>, f: F) -> Result<R, Refusal>
    where
        R: AnswerBytes + Send + 'static,
        F: FnOnce(&mut crate::sql::Connection<'_>) -> anyhow::Result<R> + Send + 'static,
    {
        let database = self.resolve(until)?;
        let store = match &database.shared.writer {
            Writer::Local(store) => store,
            Writer::Replicated { .. } => {
                let query: Call = Box::new(move |db| {
                    let mut connection = crate::sql::Connection::new(db);
                    let value = within(f(&mut connection)?, answer)?;
                    connection.finish()?;
                    Ok(Box::new(value) as Box<dyn Any + Send>)
                });
                let operation = Operation::Query(Mutex::new(Some(query)), answer.get());
                return match database.submit_selected("", operation, until)? {
                    (Answer::Called(value), _) => value
                        .downcast::<R>()
                        .map(|value| *value)
                        .map_err(|_| unavailable("wrong SQL query answer")),
                    _ => Err(unavailable("wrong SQL query answer")),
                };
            }
            Writer::Client(_) => {
                return database
                    .session(until, answer.get(), move |c| Ok(within(f(c)?, answer)?))
                    .map(|(value, _)| value);
            }
        };
        database.query_reader(store, until, Some(answer.get()), move |c| {
            Ok(within(f(c)?, answer)?)
        })
    }

    /// [`Database::query`] on a local store: on a reader, on this thread, charged nothing; or, while
    /// the store has no reader to lend, queued on the writer and so charged at admission as a
    /// queued request is (MB.1): its job, its session, and `answer`, the bytes its answer declared
    /// ([`AnswerMax`]; none from [`Database::query_local`] and [`Database::query_closed`], which
    /// declare none).
    fn query_reader<R, F>(
        &self,
        store: &Store<Local>,
        until: Instant,
        answer: Option<usize>,
        f: F,
    ) -> Result<R, Refusal>
    where
        R: Send + 'static,
        F: FnOnce(&mut crate::sql::Connection<'_>) -> anyhow::Result<R> + Send + 'static,
    {
        let _in_flight = self
            .shared
            .foreground
            .get()
            .map(crate::backup::Foreground::call);
        let session = Arc::new(Mutex::new(Some(f)));
        let claim = self.application.clone();
        if let Some(read) = store.on_reader(until, |reader| {
            run_query(reader, until, claim.as_ref(), &session)
        }) {
            return read.map_err(|e| refused(&e));
        }
        let job =
            move |engine: &mut Local| run_query(&mut engine.db, until, claim.as_ref(), &session);
        let _admission = Admission::charge(
            store,
            &self.shared.admitted,
            std::mem::size_of_val(&job)
                .saturating_add(std::mem::size_of::<Mutex<Option<F>>>())
                .saturating_add(answer.unwrap_or(0)),
            until,
        )?;
        store.run(until, job).map_err(|e| refused(&e))
    }

    /// [`Database::query`] for a session whose Records reads wait on a held cross-group lock
    /// under this database's [`LockedReads`] (T-T1.10.W11, lane xgroup-sql-11). A session reads
    /// one snapshot, so it can't wait inside itself: under [`LockedReads::Wait`] or
    /// [`LockedReads::WaitOnLock`] its Records get of a row under a lock is refused, the row
    /// noted, and once the session ends the read waits as a get does and runs the session again,
    /// on a snapshot past the resolution; so `f` may run more than once and answers from its last
    /// run only. Under [`LockedReads::Committed`] it is [`Database::query`]. `answer`: what
    /// [`Database::query`] declares.
    pub fn query_waiting<R, F>(
        &self,
        until: Instant,
        answer: AnswerMax<R>,
        f: F,
    ) -> Result<R, Refusal>
    where
        R: AnswerBytes + Send + 'static,
        F: Fn(&mut crate::sql::Connection<'_>) -> anyhow::Result<R> + Send + Sync + 'static,
    {
        self.session_waiting(Some(answer), until, f)
    }

    /// [`Database::query_waiting`] as a local read ([`Database::query_local`]): on a replica, on
    /// its owner's applied state, asking no leader, its waits rung by this replica's own
    /// application of the resolution under [`LockedReads::WaitOnLock`] (T-T1.10.W13, lane
    /// xgroup-sql-12).
    pub fn query_local_waiting<R, F>(&self, until: Instant, f: F) -> Result<R, Refusal>
    where
        R: AnswerBytes + Send + 'static,
        F: Fn(&mut crate::sql::Connection<'_>) -> anyhow::Result<R> + Send + Sync + 'static,
    {
        self.session_waiting(None, until, f)
    }

    /// The session of [`Database::query_waiting`], at the leader's latest commit with the bound
    /// it declared (`latest`; none: of [`Database::query_local_waiting`]).
    fn session_waiting<R, F>(
        &self,
        latest: Option<AnswerMax<R>>,
        until: Instant,
        f: F,
    ) -> Result<R, Refusal>
    where
        R: AnswerBytes + Send + 'static,
        F: Fn(&mut crate::sql::Connection<'_>) -> anyhow::Result<R> + Send + Sync + 'static,
    {
        let reads = LockedReads::of(self.shared.changed.locked_reads.load(Ordering::Acquire));
        if reads == LockedReads::Committed {
            return match latest {
                Some(answer) => self.query(until, answer, f),
                None => self.query_local(until, f),
            };
        }
        let database = self.resolve(until)?;
        let f = Arc::new(f);
        let latest = latest.map(met_max);
        database.wait_locked(reads, until, || {
            let f = Arc::clone(&f);
            let run = move |c: &mut crate::sql::Connection<'_>| {
                let watch = session::Watch::begin();
                let answer = (*f)(c);
                match watch.met() {
                    Some(mut row) => {
                        row.shrink_to_fit();
                        Ok(Err(row))
                    }
                    None => answer.map(Ok),
                }
            };
            match latest {
                Some(answer) => database.query(until, answer, run),
                None => database.query_local(until, run),
            }
        })
    }

    /// Runs one read-only SQL session on this node's own state: a local read (replication.md, "A
    /// local read on a replica"). Standalone, it is [`Database::query`]. On a replica it runs on a
    /// reader of the owner's applied state, asking the group's leader nothing, so it answers with
    /// the leader gone or out of reach: a prefix of the group's committed history, maybe behind
    /// it, never ahead of it. An attached client's runs the same way on its owner's state.
    /// Refused while a replica hasn't taken its group's first image, once its owner ended, and
    /// while it is fenced.
    pub fn query_local<R, F>(&self, until: Instant, f: F) -> Result<R, Refusal>
    where
        R: Send + 'static,
        F: FnOnce(&mut crate::sql::Connection<'_>) -> anyhow::Result<R> + Send + 'static,
    {
        let database = self.resolve(until)?;
        match &database.shared.writer {
            Writer::Replicated { client, .. } => {
                let session = Mutex::new(Some(f));
                let claim = database.application.as_ref();
                client.read_local(until, |reader| run_query(reader, until, claim, &session))
            }
            Writer::Client(backend) => backend
                .client
                .for_application(database.application.clone())
                .query_local(until, f)
                .map_err(Refusal::from),
            Writer::Local(store) => database.query_reader(store, until, None, f),
        }
    }

    /// A follower read (replication.md, "Follower reads at a closed time"; AC-T1.10.C2): one
    /// read-only SQL session on this node's own state, as [`Database::query_local`] runs it, at
    /// this replica's closed time once that is `at_least` (microseconds since the Unix epoch). It
    /// answers with the time it read at: the state it saw is its group's at that time, every
    /// write at it or before and none after, whoever led when. It waits on the caller's thread
    /// for the time to come, until `until`, and is refused Timeout past it. Standalone, it is
    /// [`Database::query`] at the wall clock when it began: its writes carry no time. An attached
    /// client's is refused until the wire carries it.
    pub fn query_closed<R, F>(
        &self,
        at_least: u64,
        until: Instant,
        f: F,
    ) -> Result<ClosedRead<R>, Refusal>
    where
        R: Send + 'static,
        F: FnOnce(&mut crate::sql::Connection<'_>) -> anyhow::Result<R> + Send + 'static,
    {
        let database = self.resolve(until)?;
        match &database.shared.writer {
            Writer::Replicated { client, .. } => {
                let session = Mutex::new(Some(f));
                let claim = database.application.as_ref();
                client.read_closed(at_least, until, |reader| {
                    run_closed(reader, at_least, until, claim, &session)
                })
            }
            Writer::Client(_) => Err(api::invalid(
                "closed-time read",
                "an attached client's waits for the wire's next version",
            )),
            Writer::Local(store) => {
                let micros = replicated::wall_micros()?;
                database
                    .query_reader(store, until, None, f)
                    .map(|value| ClosedRead { value, micros })
            }
        }
    }

    /// Takes a stamp of the one cross-group clock in (lane split-merge-21, T-T1.9.R28), the hybrid
    /// logical clock's receive rule, before a step a coordinator carries it into: a backed-up
    /// group's writer stamps its commits after it, a replicated group's owner proposes past it.
    pub(crate) fn observe_clock(&self, until: Instant, micros: u64) -> Result<(), Refusal> {
        let database = self.resolve(until)?;
        if let Writer::Replicated { client, .. } = &database.shared.writer {
            client.observe(micros);
        }
        if let Some(hooks) = database.stamps() {
            hooks.marks.observe_micros(micros).map_err(unavailable)?;
        }
        Ok(())
    }

    /// A replicated group's closed point as a token, for the tests across groups (lane
    /// xgroup-sql), at the last local application. None standalone and for an
    /// attached client.
    #[cfg(test)]
    pub(crate) fn closed_token(&self, until: Instant) -> Result<Option<Token>, Refusal> {
        let database = self.resolve(until)?;
        match &database.shared.writer {
            Writer::Replicated { client, .. } => client.closed_token().map(Some),
            Writer::Client(_) | Writer::Local(_) => Ok(None),
        }
    }

    /// The stamp the lock step published, read locally after `call_internal` has waited for
    /// that step's apply (T-T1.9.R30). A fenced coordinator can lose its locks to recovery
    /// before this read; it cannot give a successful vote for a stamp it no longer knows.
    pub(crate) fn lock_reply_clock(
        &self,
        txn: crate::xgroup::TxnId,
        until: Instant,
    ) -> Result<Option<u64>, Refusal> {
        use crate::xgroup::{Tablet, sql::SqlTablet};
        let database = self.resolve(until)?;
        let read = move |db: &mut Db| {
            let _deadline = db.deadline(until);
            SqlTablet::new(db)
                .lock_stamp(txn)?
                .ok_or_else(|| Refusal::UnknownOutcome.into())
        };
        match &database.shared.writer {
            Writer::Replicated { client, .. } => client.read_local(until, read).map(Some),
            Writer::Local(_) if database.stamps().is_none() => Ok(None),
            Writer::Local(store) => store
                .read_committed(until, read, move |engine| read(&mut engine.db))
                .map(Some)
                .map_err(|e| refused(&e)),
            Writer::Client(_) => Err(api::invalid(
                "cross-group step",
                "lock state belongs to the group's owner",
            )),
        }
    }

    /// This group's clock on the one cross-group clock (lane split-merge-21, T-T1.9.R28; Ada's
    /// decision of 2026-11-22), microseconds since the Unix epoch: a replicated group's closed
    /// time, a backed-up group's writer clock (`Marks::latest_micros`, which
    /// stamps the rows reads across groups place); none for a group that
    /// keeps neither. Lock votes use the clock recorded at publication (`lock_reply_clock`).
    pub fn cross_clock(&self, until: Instant) -> Result<Option<u64>, Refusal> {
        let database = self.resolve(until)?;
        if let Writer::Replicated { client, .. } = &database.shared.writer {
            return client.closed_micros().map(Some);
        }
        database
            .stamps()
            .map(|hooks| hooks.marks.latest_micros().map_err(unavailable))
            .transpose()
    }

    /// One read-only SQL session in the read mode asked (kiln goal 8119f25d, the switch):
    /// - [`ReadMode::Latest`], the control and today's read: [`Database::query`], which waits for
    ///   the leader's latest commit whatever `token` says;
    /// - [`ReadMode::Session`]: with no token, [`Database::query_local`], which never waits for
    ///   the log; with one, on this replica once it has applied the token, waiting only while it
    ///   is behind and redirected to [`Database::query`] past its measured apply lag.
    ///
    /// Answers alike in both modes for every session that carries its tokens. `answer`: what
    /// [`Database::query`] declares, for a read at the leader's latest commit.
    pub fn query_session<R, F>(
        &self,
        mode: ReadMode,
        token: Option<Token>,
        until: Instant,
        answer: AnswerMax<R>,
        f: F,
    ) -> Result<R, Refusal>
    where
        R: AnswerBytes + Send + 'static,
        F: FnOnce(&mut crate::sql::Connection<'_>) -> anyhow::Result<R> + Send + 'static,
    {
        let (ReadMode::Session, Some(token)) = (mode, token) else {
            return match mode {
                ReadMode::Latest => self.query(until, answer, f),
                ReadMode::Session => self.query_local(until, f),
            };
        };
        let database = self.resolve(until)?;
        let Writer::Replicated { client, .. } = &database.shared.writer else {
            return self.query(until, answer, f);
        };
        let session = Mutex::new(Some(f));
        let claim = database.application.as_ref();
        let served = client.read_session(token, until, |reader| {
            if token.micros == 0 {
                return run_query(reader, until, claim, &session).map(Ok);
            }
            run_closed(reader, token.micros, until, claim, &session)
                .map(|read| read.map(|read| read.value))
        })?;
        match served {
            replicated::Served::Here(value) => Ok(value),
            replicated::Served::Redirect => {
                let f = locked(&session)?
                    .take()
                    .ok_or_else(|| api::invalid("SQL query", "was already executed"))?;
                self.query(until, answer, f)
            }
        }
    }

    /// What this database's Records gets do with a row under a cross-group lock ([`LockedReads`],
    /// the A/B switch; [`LockedReads::Committed`] until set), for every handle cloned from it.
    /// The wait settings look the locks up in the group's cross-group tables, so the group needs
    /// them first (`xgroup::sql::prepare`): without them its gets and scans are refused.
    pub fn set_locked_reads(&self, reads: LockedReads) {
        self.shared
            .changed
            .locked_reads
            .store(reads.code(), Ordering::Release);
    }

    /// The times a get waiting on a cross-group lock here tried its row again, each wake counted
    /// ([`LockedReads::Wait`] against [`LockedReads::WaitOnLock`], T-T1.10.W9).
    pub fn locked_wakes(&self) -> u64 {
        self.shared.changed.locked_wakes.load(Ordering::Acquire)
    }

    /// Records gets here that met a cross-group lock and waited for it ([`LockedReads::Wait`]),
    /// each counted once.
    pub fn locked_waits(&self) -> u64 {
        self.shared.changed.locked_waits.load(Ordering::Acquire)
    }

    /// Waits, on the change event, until `n` gets here have waited on a cross-group lock: a test
    /// opens a held resolution only once a read waits on it.
    #[cfg(test)]
    pub(crate) fn locked_waits_reach(&self, n: u64, until: Instant) -> Result<(), Refusal> {
        let changed = &self.shared.changed;
        let guard = locked(&changed.generation)?;
        let (_guard, timeout) = changed
            .event
            .wait_timeout_while(
                guard,
                until.saturating_duration_since(Instant::now()),
                |_| changed.locked_waits.load(Ordering::Acquire) < n,
            )
            .map_err(unavailable)?;
        if timeout.timed_out() {
            return Err(Refusal::Timeout);
        }
        Ok(())
    }

    /// A Records read under `reads`, [`LockedReads::Wait`] or [`LockedReads::WaitOnLock`]:
    /// `try_read` answers what it read, or the cross-group key of a row it met under a lock, and
    /// is tried again until it reads.
    fn wait_locked<T>(
        &self,
        reads: LockedReads,
        until: Instant,
        try_read: impl FnMut() -> Result<Result<T, Vec<u8>>, Refusal>,
    ) -> Result<T, Refusal> {
        if reads == LockedReads::WaitOnLock {
            self.wait_on_lock(until, try_read)
        } else {
            self.wait_on_group(until, try_read)
        }
    }

    /// Counts a read that met a cross-group lock, once a read, and tells the change event under
    /// its counter's lock, so whoever waits for a read to be waiting can't miss it.
    fn count_locked_wait(&self) -> Result<(), Refusal> {
        let changed = &self.shared.changed;
        let _generation = locked(&changed.generation)?;
        changed.locked_waits.fetch_add(1, Ordering::AcqRel);
        changed.event.notify_all();
        Ok(())
    }

    /// [`LockedReads::Wait`] (lane xgroup-sql-10): a locked read tries again on each change in
    /// the group. The change counter is read before each try, so a resolution landing between a
    /// try and the wait moves it and the read tries again; a wait can't miss it. Refused
    /// `Timeout` at `until`.
    fn wait_on_group<T>(
        &self,
        until: Instant,
        mut try_read: impl FnMut() -> Result<Result<T, Vec<u8>>, Refusal>,
    ) -> Result<T, Refusal> {
        let changed = &self.shared.changed;
        let mut waited = false;
        loop {
            let before = *locked(&changed.generation)?;
            if let Ok(v) = try_read()? {
                return Ok(v);
            }
            if !waited {
                waited = true;
                self.count_locked_wait()?;
            }
            let guard = locked(&changed.generation)?;
            if *guard != before {
                changed.locked_wakes.fetch_add(1, Ordering::AcqRel);
                continue;
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
            changed.locked_wakes.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// [`LockedReads::WaitOnLock`] (T-T1.10.W9, lane xgroup-sql-11): a locked read waits on the
    /// waiter list of the row it met, rung only by a resolution that drops that row's locks
    /// ([`Database::call_internal_ringing`]). It joins the list and tries once more before it
    /// first waits there, so a resolution between its try and joining isn't missed; from then on
    /// the list's ring count, read before each try, says whether one landed since. Meeting another
    /// row, it moves to that row's list the same way. It leaves its list however it ends. It is
    /// counted waiting ([`Database::locked_waits`]) as it first blocks.
    fn wait_on_lock<T>(
        &self,
        until: Instant,
        try_read: impl FnMut() -> Result<Result<T, Vec<u8>>, Refusal>,
    ) -> Result<T, Refusal> {
        let mut joined = None;
        let read = self.wait_joined(until, try_read, &mut joined);
        if let Some((row, _)) = joined {
            leave_list(&mut *locked(&self.shared.changed.lock_waiters)?, &row);
        }
        read
    }

    /// [`Database::wait_on_lock`]'s tries and waits; `joined`: the row whose list it is on, and
    /// that list's bell, for its caller to leave however it ends.
    fn wait_joined<T>(
        &self,
        until: Instant,
        mut try_read: impl FnMut() -> Result<Result<T, Vec<u8>>, Refusal>,
        joined: &mut Option<(Vec<u8>, Arc<Condvar>)>,
    ) -> Result<T, Refusal> {
        let changed = &self.shared.changed;
        let rung =
            |lists: &BTreeMap<Vec<u8>, LockBell>, row: &[u8]| lists.get(row).map_or(0, |l| l.rung);
        let mut waited = false;
        loop {
            let before = match joined.as_ref() {
                Some((row, _)) => Some(rung(&*locked(&changed.lock_waiters)?, row)),
                None => None,
            };
            let row = match try_read()? {
                Ok(v) => return Ok(v),
                Err(row) => row,
            };
            let mut lists = locked(&changed.lock_waiters)?;
            match (joined.as_ref(), before) {
                (Some((at, bell)), Some(before)) if *at == row => {
                    if rung(&lists, &row) != before {
                        changed.locked_wakes.fetch_add(1, Ordering::AcqRel);
                        continue;
                    }
                    if !waited {
                        // Counted under the list's lock, which a ring takes and the wait gives up
                        // only once blocked: whoever waits for a read to be waiting rings it
                        // asleep, never before it waits.
                        waited = true;
                        self.count_locked_wait()?;
                    }
                    let (_lists, timeout) = bell
                        .wait_timeout_while(
                            lists,
                            until.saturating_duration_since(Instant::now()),
                            |lists| rung(lists, &row) == before,
                        )
                        .map_err(unavailable)?;
                    if timeout.timed_out() {
                        return Err(Refusal::Timeout);
                    }
                    changed.locked_wakes.fetch_add(1, Ordering::AcqRel);
                }
                _ => {
                    if let Some((old, _)) = joined.take() {
                        leave_list(&mut lists, &old);
                    }
                    let list = lists.entry(row.clone()).or_default();
                    list.waiters = list.waiters.saturating_add(1);
                    *joined = Some((row, Arc::clone(&list.bell)));
                }
            }
        }
    }

    /// This replica's token reads, counted, with its measured apply lag. None standalone and for
    /// an attached client.
    pub fn session_counters(&self, until: Instant) -> Result<Option<SessionCounters>, Refusal> {
        let database = self.resolve(until)?;
        match &database.shared.writer {
            Writer::Replicated { client, .. } => client.session_counters().map(Some),
            Writer::Client(_) | Writer::Local(_) => Ok(None),
        }
    }

    /// The time a follower read here can ask and not wait for, most of the time: now, less the
    /// lag this replica measured between its closed time and its clock (the p99 of its last
    /// moves; replication.md, "Follower reads at a closed time", AC-T1.10.C3). Refused
    /// Unavailable before it has measured any. Standalone, now.
    pub fn follower_read_micros(&self, until: Instant) -> Result<u64, Refusal> {
        let database = self.resolve(until)?;
        let now = replicated::wall_micros()?;
        match &database.shared.writer {
            Writer::Replicated { client, .. } => client
                .closed_lag()?
                .map(|lag| now.saturating_sub(lag))
                .ok_or_else(|| {
                    Refusal::Unavailable(
                        "this replica has measured no replication delay yet".into(),
                    )
                }),
            Writer::Client(_) => Err(api::invalid(
                "closed-time read",
                "an attached client's waits for the wire's next version",
            )),
            Writer::Local(_) => Ok(now),
        }
    }

    /// One atomic batch, including SELECT or DML RETURNING results. Parameters and SQL are owned;
    /// transaction-control, connection PRAGMAs and ATTACH are refused before anything is submitted.
    pub fn execute(
        &self,
        statements: Vec<Statement>,
        until: Instant,
    ) -> Result<Vec<SqlResult>, Refusal> {
        self.execute_session(statements, until)
            .map(|(results, _)| results)
    }

    /// [`Database::execute`], answered with its write's own session token, as
    /// [`Database::call_session`] gives it (AC-T1.10.R4).
    pub fn execute_session(
        &self,
        statements: Vec<Statement>,
        until: Instant,
    ) -> Result<(Vec<SqlResult>, Option<Token>), Refusal> {
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
        match self.submit_session("", Operation::Mutation(Mutation::Sql(statements)), until)? {
            (Answer::Sql(v), token) => Ok((v, token)),
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
        self.submit_session(namespace, operation, until)
            .map(|(answer, _)| answer)
    }

    /// [`Database::submit`] with its write's own session token, if its group's log took one.
    fn submit_session(
        &self,
        namespace: &str,
        operation: Operation,
        until: Instant,
    ) -> Result<(Answer, Option<Token>), Refusal> {
        self.resolve(until)?
            .submit_selected(namespace, operation, until)
    }

    fn submit_selected(
        &self,
        namespace: &str,
        operation: Operation,
        until: Instant,
    ) -> Result<(Answer, Option<Token>), Refusal> {
        if Instant::now() >= until {
            return Err(Refusal::Timeout);
        }
        if let Operation::Mutation(Mutation::Compact { before, .. }) = &operation {
            self.namespace(namespace)?.compact(*before, until)?;
            return Ok((Answer::Done, None));
        }
        match &self.shared.writer {
            Writer::Replicated { client, .. } => {
                return client
                    .submit(
                        namespace.to_owned(),
                        operation,
                        self.application.clone(),
                        until,
                    )
                    .map(|answered| (answered.answer, answered.token));
            }
            Writer::Local(_) => {}
            Writer::Client(backend) => {
                return remote::submit(
                    &backend.client.for_application(self.application.clone()),
                    namespace,
                    operation,
                    until,
                )
                .map(|answer| (answer, None));
            }
        }
        let (tx, rx) = mpsc::sync_channel(1);
        self.submit_local(namespace, operation, until, until, move |result| {
            // A caller whose deadline elapsed no longer waits, but the committed result stands.
            let _ = tx.send(result);
        })?;
        rx.recv_timeout(until.saturating_duration_since(Instant::now()))
            .map_err(|e| match e {
                mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected => {
                    Refusal::UnknownOutcome
                }
            })?
            .map(|answer| (answer, None))
    }

    /// Admits `operation` to the local writer's queue, waiting for room in it until `room_until`,
    /// and hands `reply` its answer once durable, or why it won't be, on the store's notifier
    /// thread. Refused here, `reply` is dropped untold: the refusal is the answer.
    fn submit_local(
        &self,
        namespace: &str,
        operation: Operation,
        until: Instant,
        room_until: Instant,
        reply: impl FnOnce(Result<Answer, Refusal>) + Send + 'static,
    ) -> Result<(), Refusal> {
        let Writer::Local(store) = &self.shared.writer else {
            return Err(api::invalid(
                "call_nowait",
                "only a local owner queues a call without blocking; call answers here",
            ));
        };
        let admission = Admission::reserve(store, &self.shared.admitted, &operation, room_until)?;
        // In flight from here to its answer, as a backed-up writer's checks yield to it.
        let in_flight = self
            .shared
            .foreground
            .get()
            .map(crate::backup::Foreground::call);
        let operation = match read_records(
            store,
            &self.shared.reads,
            namespace,
            operation,
            &self.application,
            until,
        )? {
            ReadRoute::Answered(answer) => {
                drop(admission);
                drop(in_flight);
                reply(answer);
                return Ok(());
            }
            ReadRoute::Writer(operation) => operation,
        };
        let wake = matches!(&operation, Operation::Mutation(Mutation::Records { tx, .. }) if !tx.writes.is_empty())
            || matches!(
                &operation,
                Operation::Mutation(Mutation::Compact { .. } | Mutation::CompactStep { .. })
            );
        let wake = wake || matches!(&operation, Operation::Import(_) | Operation::Call(..));
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
            Operation::Call(..) => Some(Publish::Call),
            _ => None,
        };
        let answer: Reply = Arc::new(Mutex::new(None));
        let publication = Arc::new(Mutex::new(None));
        let changed = Arc::clone(&self.shared.changed);
        store
            .put_then(
                vec![Command {
                    namespace: namespace.to_owned(),
                    application: self.application.clone(),
                    operation,
                    answer: Arc::clone(&answer),
                    publication: Arc::clone(&publication),
                    until,
                }],
                room_until,
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
                    // Its charge and its count in flight end before its answer is handed over,
                    // so a caller holding the answer holds no charge but its answer's.
                    drop(admission);
                    drop(in_flight);
                    reply(result);
                },
            )
            .map(|_| ())
            .map_err(admission_refusal)
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
        self.changed(family, after, until)
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
        if let Some(cluster) = &self.database.cluster {
            let routing = crate::xgroup::sql::record_key(&self.name, family, key)?;
            return across::get(
                cluster.groups(),
                cluster.group_of(&routing),
                &self.name,
                family,
                key,
                at,
                until,
            );
        }
        let reads = LockedReads::of(
            self.database
                .shared
                .changed
                .locked_reads
                .load(Ordering::Acquire),
        );
        if reads == LockedReads::Committed {
            return match self.database.submit(
                &self.name,
                Operation::Get(family.into(), key.into(), at, LockedReads::Committed),
                until,
            )? {
                Answer::Row(v) => Ok(v),
                _ => Err(unavailable("wrong record answer")),
            };
        }
        let namespace = self.resolved(until)?;
        let try_get = || -> Result<Result<Option<Row>, Vec<u8>>, Refusal> {
            match namespace.database.submit(
                &namespace.name,
                Operation::Get(family.into(), key.into(), at, reads),
                until,
            )? {
                Answer::Row(v) => Ok(Ok(v)),
                Answer::Locked(row) => Ok(Err(row)),
                _ => Err(unavailable("wrong record answer")),
            }
        };
        namespace.database.wait_locked(reads, until, try_get)
    }

    fn scan(&self, family: &str, mut scan: Scan, until: Instant) -> Result<Page, Refusal> {
        api::family(family)?;
        scan.limit = api::limit(scan.limit)?;
        api::bound("prefix", scan.prefix.len(), api::KEY_MAX)?;
        if let Some(k) = &scan.after {
            api::key(k)?;
        }
        if let Some(cluster) = &self.database.cluster {
            return across::scan(
                cluster.groups(),
                self.database.read_point,
                &self.name,
                family,
                &scan,
                until,
            );
        }
        let reads = LockedReads::of(
            self.database
                .shared
                .changed
                .locked_reads
                .load(Ordering::Acquire),
        );
        if reads == LockedReads::Committed {
            return match self.database.submit(
                &self.name,
                Operation::Scan(family.into(), scan, LockedReads::Committed),
                until,
            )? {
                Answer::Page(v) => Ok(v),
                _ => Err(unavailable("wrong scan answer")),
            };
        }
        // T-T1.10.W10 (lane xgroup-sql-11): a scan of the latest rows waits as a get does, on the
        // first row in its range under a cross-group lock.
        let namespace = self.resolved(until)?;
        let try_scan = || -> Result<Result<Page, Vec<u8>>, Refusal> {
            match namespace.database.submit(
                &namespace.name,
                Operation::Scan(family.into(), scan.clone(), reads),
                until,
            )? {
                Answer::Page(v) => Ok(Ok(v)),
                Answer::Locked(row) => Ok(Err(row)),
                _ => Err(unavailable("wrong scan answer")),
            }
        };
        namespace.database.wait_locked(reads, until, try_scan)
    }
    fn commit(&self, tx: Tx, until: Instant) -> Result<Rev, Refusal> {
        api::transaction(&tx)?;
        // Across groups (lane split-merge-22, T-T1.9.R29): through the coordinator, answered with
        // the transaction's one commit stamp, the revision reads across groups give its rows.
        if let Some(cluster) = &self.database.cluster {
            return crate::xgroup::sql::commit_stamp(cluster, &self.name, &tx, until);
        }
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
        if let Some(cluster) = &self.database.cluster {
            return across::changes(
                cluster.groups(),
                self.database.read_point,
                &self.name,
                family,
                after,
                limit,
                until,
            );
        }
        match self.database.submit(
            &self.name,
            Operation::Changes(family.into(), after, limit),
            until,
        )? {
            Answer::Changes(v) => Ok(v),
            _ => Err(unavailable("wrong changes answer")),
        }
    }
    fn changed(&self, family: &str, after: Rev, until: Instant) -> Result<Option<Rev>, Refusal> {
        if let Some(cluster) = &self.database.cluster {
            api::family(family)?;
            return across::changed(
                cluster.groups(),
                self.database.read_point,
                &self.name,
                family,
                after,
                until,
            );
        }
        self.resolved(until)?
            .try_wait_selected(family, after, until)
    }
    fn head(&self, until: Instant) -> Result<Rev, Refusal> {
        if let Some(cluster) = &self.database.cluster {
            return across::head(
                cluster.groups(),
                self.database.read_point,
                &self.name,
                until,
            );
        }
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
        if let Some(cluster) = &self.database.cluster {
            return across::wait(
                cluster.groups(),
                self.database.read_point,
                &self.name,
                family,
                after,
                until,
            );
        }
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
    /// The database closed cleanly: plain SQLite then reads its file (docs/design/store.md,
    /// decision 45).
    fn close(self) -> anyhow::Result<()> {
        self.db.close()
    }
    /// None of its own: a command is charged at its admission ([`Admission`]), what it carries
    /// and what its answer can hold, until it is answered, its time in the writer's queue within.
    fn row_bytes(_: &Command) -> usize {
        0
    }
    /// A Records transaction routes by every key its conditions and writes name, and a get by
    /// its key, each as its row key ([`reshard::row_key`] of the namespace, family and key), so a
    /// transaction stays within one group (CP-T1.9; docs/design/reshard.md, decision "The routing
    /// key is bytes"). A row key too long to make has no route here, and its write is refused
    /// where the group indexes it (`group::position`). Any other command routes by none.
    fn routes(row: &Command, each: &mut dyn FnMut(&[u8])) {
        let mut route = |family: &str, key: &[u8]| {
            if let Some(k) = reshard::row_key(&row.namespace, family, key) {
                each(&k);
            }
        };
        match &row.operation {
            Operation::Mutation(Mutation::Records { tx, .. }) => {
                for cond in &tx.conds {
                    let (Cond::Absent { family, key } | Cond::At { family, key, .. }) = cond;
                    route(family, key);
                }
                for write in &tx.writes {
                    let (Write::Put { family, key, .. } | Write::Delete { family, key }) = write;
                    route(family, key);
                }
            }
            Operation::Get(family, key, ..) => route(family, key),
            _ => {}
        }
    }
    fn apply(&mut self, rows: &[Command]) -> anyhow::Result<usize> {
        let restoring = self.restoring;
        // An adaptive session contains an FnOnce: the pipeline must never retry it. Its own
        // transaction captures the application result before the pipeline's durable barrier.
        if rows.iter().any(|r| {
            matches!(
                r.operation,
                Operation::Call(..) | Operation::Mutation(Mutation::CompactStep { .. })
            )
        }) {
            for row in rows {
                if let Operation::Mutation(Mutation::CompactStep { before, .. }) = &row.operation {
                    if let Err(error) =
                        application::check(&self.db, row.application.as_ref(), &row.operation)
                    {
                        *locked(&row.answer)? = Some(Err(refused(&error)));
                        continue;
                    }
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
                        application::check(db, row.application.as_ref(), &row.operation)?;
                        if !restoring && restore::incomplete(db)? {
                            return Err(api::invalid("database", "restore is incomplete").into());
                        }
                        let result = (|| match &row.operation {
                            Operation::Call(call, _) => {
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
                        group::entry(db)?;
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
            let answers = rows
                .iter()
                .map(|r| {
                    if Instant::now() >= r.until {
                        return Err(Refusal::Timeout.into());
                    }
                    let _deadline = db.deadline(r.until);
                    application::check(db, r.application.as_ref(), &r.operation)?;
                    let result = apply(db, &r.namespace, &r.operation);
                    if result.is_err() && Instant::now() >= r.until {
                        return Err(Refusal::Timeout.into());
                    }
                    result
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            // The commit's entry in a group's log (CP-T1.9).
            group::entry(db)?;
            Ok(answers)
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

    fn uncopied(reader: &mut Db) -> anyhow::Result<Option<Checkpointed>> {
        Ok(Some(reader.uncopied()))
    }

    fn restart_log(&mut self) -> anyhow::Result<bool> {
        self.db.restart_log()
    }

    fn holding(&mut self) -> anyhow::Result<Option<Vec<u8>>> {
        let mut kept = None;
        self.db.query(
            "SELECT holding FROM __amphora_holding WHERE one = 0",
            [],
            |r| {
                kept = bytes(r, 0)?;
                Ok(false)
            },
        )?;
        Ok(kept)
    }

    /// The holding kept through [`Mutation::Holding`], the command a replicated owner's log
    /// carries, applied as one commit of its own after every commit before it, and flushed with
    /// them before it returns (CP-T1.9).
    /// The first holding kept makes the database a group's: its log starts and its Records keys
    /// are indexed by position, in the same commit ([`group`]). Each holding kept is one entry of
    /// the log, a fence's.
    fn keep_holding(&mut self, holding: &[u8]) -> anyhow::Result<()> {
        let operation = Operation::Mutation(Mutation::Holding(holding.to_vec()));
        self.db
            .transaction(|db| {
                group::begin(db)?;
                let kept = apply(db, "", &operation)?;
                group::entry(db)?;
                Ok(kept)
            })?
            .map_err(|refusal| anyhow::anyhow!("a holding not kept: {refusal:?}"))?;
        (self.db.flusher())()?;
        Ok(())
    }

    // The copy path (CP-T1.9; database/group.rs): over the Records rows, indexed by the position
    // of their namespace, family and key.

    fn head(&mut self) -> anyhow::Result<reshard::Seq> {
        group::head(&self.db)
    }

    fn bytes(&mut self) -> anyhow::Result<Option<u64>> {
        group::used_bytes(&self.db).map(Some)
    }

    fn size_cut(&mut self) -> anyhow::Result<Option<reshard::plan::Cut>> {
        // In a transaction of its own, so the root page read is the one its schema read named.
        self.db.transaction(|db| group::size_cut(db))
    }

    fn export(&mut self, span: reshard::Span, positions: usize) -> anyhow::Result<store::Exported> {
        group::export(&self.db, span, positions)
    }

    fn changed(
        &mut self,
        after: reshard::Seq,
        upto: reshard::Seq,
        span: reshard::Span,
        entries: u64,
    ) -> anyhow::Result<Option<store::Changed>> {
        group::changed(&self.db, after, upto, span, entries)
    }

    fn import(&mut self, moved: &[store::Moved]) -> anyhow::Result<()> {
        self.db.transaction(|db| group::import(db, moved))?;
        (self.db.flusher())()?;
        Ok(())
    }

    fn keep_log(&mut self, from: Option<reshard::Seq>) -> anyhow::Result<()> {
        self.db.transaction(|db| group::keep_log(db, from))?;
        (self.db.flusher())()?;
        Ok(())
    }

    fn sweep(
        &mut self,
        span: reshard::Span,
        positions: usize,
        garbage: &dyn Fn(reshard::Pos) -> bool,
    ) -> anyhow::Result<bool> {
        let done = self
            .db
            .transaction(|db| group::sweep(db, span, positions, garbage))?;
        (self.db.flusher())()?;
        Ok(done)
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
        Operation::Call(..) | Operation::Query(..) => {
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
        Operation::Mutation(Mutation::Holding(holding)) => {
            // A group's holding, kept by the same apply a local writer and a replicated owner's
            // log entry run (CP-T1.9): never written past the log.
            db.query(
                "INSERT INTO __amphora_holding(one, holding) VALUES (0, ?1) ON CONFLICT(one) DO UPDATE SET holding = excluded.holding",
                [blob(holding)?],
                |_| Ok(true),
            )?;
            return Ok(Ok(Answer::Done));
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
        Operation::Get(f, k, at, locks) => {
            let at = match snapshot(*at, head, horizon) {
                Ok(at) => at,
                Err(e) => return Ok(Err(e)),
            };
            if *locks != LockedReads::Committed
                && at == head
                && let Some(row) = crate::xgroup::sql::locked(db, namespace, f, k)?
            {
                return Ok(Ok(Answer::Locked(row)));
            }
            Ok(Ok(Answer::Row(get(db, namespace, f, k, at)?)))
        }
        Operation::Scan(f, scan, locks) => {
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
            if let Some(upper) = &upper {
                params.push(blob(upper)?);
            }
            let (mut rows, mut carried, mut more) = (Vec::new(), 0usize, false);
            db.query(query, params, |r| {
                let payload = blob_len(r, 0).saturating_add(blob_len(r, 2));
                if !fits(rows.len(), carried, payload, scan.limit) {
                    more = true;
                    return Ok(false);
                }
                carried = carried.saturating_add(payload);
                rows.push(Row {
                    key: bytes(r, 0)?.ok_or_else(|| anyhow::anyhow!("null key"))?,
                    rev: rev(r, 1)?,
                    value: bytes(r, 2)?.ok_or_else(|| anyhow::anyhow!("null value"))?,
                });
                Ok(true)
            })?;
            // A scan of the latest rows waiting on a held lock (T-T1.10.W10) looks for one in
            // the range its page covers: to its last row when it stopped short, else the whole.
            if *locks != LockedReads::Committed && at == head {
                let last = if more {
                    rows.last().map(|r| r.key.as_slice())
                } else {
                    None
                };
                if let Some(row) = crate::xgroup::sql::locked_in(
                    db,
                    namespace,
                    f,
                    lower,
                    continued,
                    upper.as_deref(),
                    last,
                )? {
                    return Ok(Ok(Answer::Locked(row)));
                }
            }
            // Grown one push at a time, the list holds the bytes admission charged once trimmed.
            rows.shrink_to_fit();
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
            let (mut changes, mut carried, mut next) = (Vec::new(), 0usize, None);
            db.query("SELECT rev, key, value FROM __amphora_versions WHERE namespace = ?1 AND family = ?2 AND rev > ?3 ORDER BY rev, seq LIMIT ?4", [text(namespace), text(f), integer(*after)?, integer(u64::try_from(*limit)?.checked_add(1).ok_or_else(|| anyhow::anyhow!("limit overflow"))?)?], |r| {
                let payload = blob_len(r, 1).saturating_add(blob_len(r, 2));
                if !fits(changes.len(), carried, payload, *limit) {
                    next = Some(rev(r, 0)?);
                    return Ok(false);
                }
                carried = carried.saturating_add(payload);
                changes.push(Change { rev: rev(r, 0)?, key: bytes(r, 1)?.ok_or_else(|| anyhow::anyhow!("null key"))?, value: bytes(r, 2)? });
                Ok(true)
            })?;
            // The cursor is a revision, so a page must never cut a transaction's revision.
            // Otherwise advancing to its last change would silently skip the remaining keys.
            if next.is_some() && changes.last().map(|c| c.rev) == next {
                changes.retain(|c| Some(c.rev) != next);
                if changes.is_empty() {
                    return Ok(Err(api::invalid(
                        "change limit",
                        "a transaction does not fit; request up to TX_MAX changes",
                    )));
                }
            }
            Ok(Ok(Answer::Changes(Changes { changes, head })))
        }
        Operation::Mutation(Mutation::Compact { .. } | Mutation::CompactStep { .. }) => {
            Err(api::invalid("compaction", "requires the bounded writer transaction").into())
        }
        Operation::Import(_)
        | Operation::ImportProgress
        | Operation::Call(..)
        | Operation::Query(..)
        | Operation::Mutation(Mutation::Sql(_) | Mutation::Migrate(_) | Mutation::Holding(_)) => {
            Err(anyhow::anyhow!(
                "SQL operation reached the Records dispatch"
            ))
        }
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
/// Whether a Records row has a value at `at` (its newest version by then is no deletion), as
/// [`get`] would say by answering one, without copying the value out (AUDIT 4.2).
fn live_at(db: &Db, ns: &str, family: &str, key: &[u8], at: Rev) -> anyhow::Result<bool> {
    let mut live = false;
    db.query("SELECT value IS NOT NULL FROM __amphora_versions WHERE namespace = ?1 AND family = ?2 AND key = ?3 AND rev <= ?4 ORDER BY rev DESC LIMIT 1", [text(ns), text(family), blob(key)?, integer(at)?], |r| { live = r.get::<i64>(0).map_err(unavailable)? != 0; Ok(false) })?;
    Ok(live)
}
fn get(db: &Db, ns: &str, family: &str, key: &[u8], at: Rev) -> anyhow::Result<Option<Row>> {
    let mut out = None;
    db.query("SELECT rev, value FROM __amphora_versions WHERE namespace = ?1 AND family = ?2 AND key = ?3 AND rev <= ?4 ORDER BY rev DESC LIMIT 1", [text(ns), text(family), blob(key)?, integer(at)?], |r| { out = bytes(r, 1)?.map(|value| Ok::<_, anyhow::Error>(Row { key: key.into(), value, rev: rev(r, 0)? })).transpose()?; Ok(false) })?;
    Ok(out)
}

/// The commit stamp of a Records row's newest version, as a condition of a transaction across
/// groups checks it under its lock (lane split-merge-20, T-T1.9.R27): none while it has no
/// value; 0 for one whose stamp isn't known (made before its group stamped its commits), as a
/// read across groups answers it.
pub(crate) fn newest_stamp(
    db: &Db,
    ns: &str,
    family: &str,
    key: &[u8],
) -> anyhow::Result<Option<u64>> {
    let mut head = 0;
    db.query(
        "SELECT head FROM __amphora_namespaces WHERE name = ?1",
        [text(ns)],
        |r| {
            head = rev(r, 0)?;
            Ok(false)
        },
    )?;
    let Some(row) = get(db, ns, family, key, head)? else {
        return Ok(None);
    };
    Ok(Some(
        across::stamp_of_version(db, ns, family, key, row.rev)?.unwrap_or(0),
    ))
}

/// A Records row's newest value, as a transaction across groups reads it under its lock
/// (AC-T1.10.1, lane xgroup-sql-6).
pub(crate) fn newest(
    db: &Db,
    ns: &str,
    family: &str,
    key: &[u8],
) -> anyhow::Result<Option<Vec<u8>>> {
    let mut head = 0;
    db.query(
        "SELECT head FROM __amphora_namespaces WHERE name = ?1",
        [text(ns)],
        |r| {
            head = rev(r, 0)?;
            Ok(false)
        },
    )?;
    Ok(get(db, ns, family, key, head)?.map(|row| row.value))
}

/// Writes one row a transaction across groups committed into this group's Records, inside the
/// step that resolves it (AC-T1.10.1, lane xgroup-sql-6): a revision and a feed row as a
/// single-group commit writes them. The step's rows in a namespace share one revision, `at` (none:
/// the first of them takes the namespace's next), each at its own `seq`; answers the revision.
/// `stamp` is the transaction's one commit stamp, the same in every group it wrote (lane
/// split-merge-21, T-T1.9.R28), when its coordinator chose one; none stamps the revision by this
/// group's clock.
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_across(
    db: &Db,
    ns: &str,
    family: &str,
    key: &[u8],
    value: Option<&[u8]>,
    at: Option<Rev>,
    seq: u64,
    stamp: Option<u64>,
) -> anyhow::Result<Rev> {
    db.query("INSERT OR IGNORE INTO __amphora_namespaces(name, head, horizon, feed, live) VALUES (?1, 0, 0, 0, 0)", [text(ns)], |_| Ok(true))?;
    let (mut head, mut live) = (0, 0);
    db.query(
        "SELECT head, live FROM __amphora_namespaces WHERE name = ?1",
        [text(ns)],
        |r| {
            head = rev(r, 0)?;
            live = rev(r, 1)?;
            Ok(false)
        },
    )?;
    let at = match at {
        Some(at) => at,
        None => head
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("revision overflow"))?,
    };
    let before = get(db, ns, family, key, head)?.is_some();
    live = match (before, value.is_some()) {
        (false, true) => live
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("live rows overflow"))?,
        (true, false) => live
            .checked_sub(1)
            .ok_or_else(|| anyhow::anyhow!("live rows underflow"))?,
        _ => live,
    };
    let stored = match value {
        Some(value) => blob(value)?,
        None => EValue::Null,
    };
    db.query("INSERT INTO __amphora_versions(namespace, family, key, rev, seq, value) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", [text(ns), text(family), blob(key)?, integer(at)?, integer(seq)?, stored], |_| Ok(true))?;
    across::stamp(db, ns, at, stamp)?;
    group::wrote(db, ns, family, key)?;
    db.query("INSERT INTO __amphora_families(namespace, family, newest) VALUES (?1, ?2, ?3) ON CONFLICT(namespace, family) DO UPDATE SET newest = excluded.newest", [text(ns), text(family), integer(at)?], |_| Ok(true))?;
    db.query(
        "UPDATE __amphora_namespaces SET head = ?2, live = ?3 WHERE name = ?1",
        [text(ns), integer(at)?, integer(live)?],
        |_| Ok(true),
    )?;
    Ok(at)
}

fn commit(
    db: &Db,
    ns: &str,
    tx: &Tx,
    head: Rev,
    live: u64,
) -> anyhow::Result<Result<Answer, Refusal>> {
    // A row a transaction across groups holds a lock on is refused as a conflict until that
    // transaction resolves, as its own lock step would refuse this one (AC-T1.10.1, lane
    // xgroup-sql-6): the caller retries, as after any conflict.
    let conds = tx.conds.iter().map(|c| {
        let (Cond::Absent { family, key } | Cond::At { family, key, .. }) = c;
        (family, key)
    });
    let writes = tx.writes.iter().map(|w| {
        let (Write::Put { family, key, .. } | Write::Delete { family, key }) = w;
        (family, key)
    });
    for (family, key) in conds.chain(writes) {
        if crate::xgroup::sql::lock_held(db, ns, family, key)? {
            return Ok(Err(Refusal::Conflict {
                family: family.clone(),
                key: key.clone(),
                at: get(db, ns, family, key, head)?.map(|r| r.rev),
            }));
        }
    }
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
        let before = live_at(db, ns, family, key, head)?;
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
    across::stamp(db, ns, at, None)?;
    for ((family, key), (seq, w)) in final_writes {
        let value = match w {
            Write::Put { value, .. } => blob(value)?,
            Write::Delete { .. } => EValue::Null,
        };
        db.query("INSERT INTO __amphora_versions(namespace, family, key, rev, seq, value) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", [text(ns), text(family), blob(key)?, integer(at)?, integer(u64::try_from(seq)?)?, value], |_| Ok(true))?;
        group::wrote(db, ns, family, key)?;
        db.query("INSERT INTO __amphora_families(namespace, family, newest) VALUES (?1, ?2, ?3) ON CONFLICT(namespace, family) DO UPDATE SET newest = excluded.newest", [text(ns), text(family), integer(at)?], |_| Ok(true))?;
    }
    db.query(
        "UPDATE __amphora_namespaces SET head = ?2, live = ?3 WHERE name = ?1",
        [text(ns), integer(at)?, integer(live)?],
        |_| Ok(true),
    )?;
    Ok(Ok(Answer::Rev(at)))
}
pub(crate) fn successor(prefix: &[u8]) -> Option<Vec<u8>> {
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
// Its callers run a child process sharing a lease, which only Unix builds do.
#[cfg(all(test, unix))]
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

#[cfg(test)]
mod owner_table {
    use super::*;

    /// The opens in flight that have reached their database's making, and the event of one more.
    #[derive(Default)]
    struct Meeting {
        arrived: Mutex<usize>,
        event: Condvar,
    }

    impl Meeting {
        /// Counts this open in, then waits, to `until`, for `all` opens to be making their
        /// databases at once: whether they all were.
        fn meet(&self, all: usize, until: Instant) -> bool {
            let mut arrived = self.arrived.lock().unwrap();
            *arrived += 1;
            self.event.notify_all();
            while *arrived < all {
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return false;
                }
                arrived = self.event.wait_timeout(arrived, left).unwrap().0;
            }
            true
        }
    }

    /// Do: open N databases at once in one process, each open waiting, while it makes its
    /// database, for every other to be making its own. Expect: they all meet, so the opens ran
    /// at the same time; with the owner table held through an open, the first would wait alone
    /// (CP-T1.13: one process's opens no longer take turns).
    #[test]
    fn cp_t1_13_opens_of_different_databases_overlap() {
        const N: usize = 4;
        let dir = tempfile::tempdir().unwrap();
        let meeting = Arc::new(Meeting::default());
        let until = Instant::now() + crate::transport::HANG;
        let opens: Vec<_> = (0..N)
            .map(|i| {
                let path = dir.path().join(format!("db-{i}"));
                let meeting = Arc::clone(&meeting);
                std::thread::spawn(move || {
                    let mut met = false;
                    let database = Database::open_owned(&path, until, false, None, |_| {
                        met = meeting.meet(N, until);
                        Ok(())
                    });
                    (database.is_ok(), met)
                })
            })
            .collect();
        for open in opens {
            assert_eq!(open.join().unwrap(), (true, true));
        }
    }

    /// Do: while an open of a database is in flight, link its file under a second name and open
    /// it by that name. Expect: the second open waits for the first, then shares its owner; it is
    /// neither refused as a hard-linked file nor a second owner of one file (CP-T1.13: a
    /// reservation names the file, not only the path the first open took).
    #[test]
    fn cp_t1_13_an_open_by_a_hard_link_waits_for_the_open_in_flight() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let link = dir.path().join("link");
        let until = Instant::now() + crate::transport::HANG;
        let second_done = Arc::new(AtomicBool::new(false));
        let mut second = None;
        let mut waited = false;
        let first = Database::open_owned(&path, until, false, None, |_| {
            std::fs::hard_link(&path, &link).map_err(unavailable)?;
            let reserved = resolved_path(&path)?;
            let (link, done) = (link.clone(), Arc::clone(&second_done));
            second = Some(std::thread::spawn(move || {
                let database = Database::open(&link, until);
                done.store(true, Ordering::Release);
                OPENED.notify_all();
                database
            }));
            // Wait for the second open to wait on this one's reservation, or to end without.
            let mut opening = locked(&OPENING)?;
            loop {
                let waiting = ongoing(&mut opening, &reserved).map_or(0, |r| r.waiting);
                if waiting > 0 || second_done.load(Ordering::Acquire) {
                    waited = waiting > 0;
                    return Ok(());
                }
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Ok(());
                }
                opening = OPENED.wait_timeout(opening, left).unwrap().0;
            }
        })
        .unwrap();
        let second = second.unwrap().join().unwrap().unwrap();
        assert!(waited, "the second open did not wait for the first");
        assert!(Arc::ptr_eq(&first.shared, &second.shared));
    }
}

#[cfg(test)]
mod admission_charge {
    use super::*;

    /// Lane perf-A19 (AUDIT 2026-11-23 §1, MB.1): a request admitted to a database waits for room
    /// until its own deadline, woken by another's charge given back, never refused at once. Do: with
    /// the memory budget's whole limit held by another charge, send a call whose deadline is an
    /// hour off; once its wait is counted, give that charge back. Expect: the call committed, one
    /// wait counted on the writer's store, and nothing held once it answers.
    #[test]
    fn a19_admission_waits_for_room_woken_by_another_charge_given_back() {
        use crate::memory::Use;
        use crate::memory::tests::{BUDGET, Fed, budget};
        let far = Instant::now() + Duration::from_secs(3600);
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(&dir.path().join("wait.db"), far).unwrap();
        let Writer::Local(store) = &database.shared.writer else {
            panic!("a local writer");
        };
        let fed = Fed::new(Some(1 << 30), Some(0));
        let (budget, _unset) = budget(&fed);
        let other = budget.take(Use::Queues, 1 << 30).unwrap();
        let (sending, on_sender) = (database.clone(), Arc::clone(&budget));
        let sender = std::thread::spawn(move || {
            BUDGET.with(|slot| *slot.borrow_mut() = Some(on_sender));
            sending.call(far, AnswerMax::fixed(), |c| {
                c.execute_batch("CREATE TABLE t(v)")?;
                Ok(())
            })
        });
        while store.counters().room_waits == 0 && !sender.is_finished() {
            std::thread::yield_now();
        }
        assert_eq!(store.counters().room_waits, 1);
        drop(other);
        assert_eq!(sender.join().unwrap(), Ok(()));
        assert_eq!(store.counters().room_waits, 1);
        assert_eq!(budget.held(Use::Queues), 0);
    }

    /// AUDIT 2026-11-23 §1 (lane perf-A12): the answers of `call_db` and `call_internal`'s
    /// callers (cross-group steps, restores, online changes, floor raises) count their heap, so
    /// a call answering one can declare a bound its answer is held to.
    #[test]
    fn a12_call_db_answers_count_their_heap() {
        let refusal = Refusal::Invalid {
            what: "ab".to_owned(),
            why: "cde".to_owned(),
        };
        assert_eq!(refusal.answer_bytes(), std::mem::size_of::<Refusal>() + 5);
        let failed: Result<u64, Refusal> = Err(refusal);
        assert_eq!(
            failed.answer_bytes(),
            std::mem::size_of::<Result<u64, Refusal>>() + 5
        );
        let held: Result<String, Refusal> = Ok(String::with_capacity(7));
        assert_eq!(
            held.answer_bytes(),
            std::mem::size_of::<Result<String, Refusal>>() + 7
        );
        let progress = api::import::ImportProgress {
            source: "src".to_owned(),
            cursor: vec![1, 2],
            done: false,
        };
        assert_eq!(
            progress.answer_bytes(),
            std::mem::size_of_val(&progress) + 5
        );
        let reads = vec![(vec![1u8; 3], Some(vec![2u8; 4])), (vec![5u8], None)];
        let heap = 2 * std::mem::size_of::<(Vec<u8>, Option<Vec<u8>>)>() + 3 + 4 + 1;
        let locked = crate::xgroup::Reply::Locked {
            reads: reads.clone(),
        };
        assert_eq!(locked.answer_bytes(), std::mem::size_of_val(&locked) + heap);
        let resolved = crate::xgroup::Reply::Resolved;
        assert_eq!(resolved.answer_bytes(), std::mem::size_of_val(&resolved));
        let committed = crate::xgroup::coordinator::Answer::Committed { reads, at: None };
        assert_eq!(
            committed.answer_bytes(),
            std::mem::size_of_val(&committed) + heap
        );
        let stepped = online::Stepped::Public {
            held: Duration::from_millis(1),
        };
        assert_eq!(stepped.answer_bytes(), std::mem::size_of_val(&stepped));
    }

    /// Do: a group's own scan at its worst case, `limit` rows each at the largest key and value,
    /// so its page fills every byte of keys and values admission charged it. Expect: the page
    /// holds no more than its charge, its list trimmed rather than grown by `push` past `limit`
    /// rows (AUDIT 2026-11-23 §1; lane perf-A14).
    #[test]
    fn a14_a_group_scan_at_its_worst_case_holds_what_admission_charged() {
        let until = || Instant::now() + Duration::from_secs(120);
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(&dir.path().join("records.db"), until()).unwrap();
        let records = database.namespace("ns").unwrap();
        let limit = 5u8;
        let writes = (0..limit)
            .map(|k| Write::Put {
                family: "f".into(),
                key: vec![k; api::KEY_MAX],
                value: vec![7; api::VALUE_MAX],
            })
            .collect();
        records
            .commit(
                Tx {
                    conds: Vec::new(),
                    writes,
                },
                until(),
            )
            .unwrap();
        let limit = usize::from(limit);
        let page = records
            .scan(
                "f",
                Scan {
                    prefix: Vec::new(),
                    after: None,
                    limit,
                    at: Consistency::Linearizable,
                },
                until(),
            )
            .unwrap();
        assert_eq!(page.rows.len(), limit);
        let charged = page_bytes::<Row>(limit);
        assert!(
            heap_bytes(&page) <= charged,
            "{} > {charged}",
            heap_bytes(&page)
        );
    }

    /// Do: a query at its worst case, the most blobs it reads (its `LIMIT`) each as large as
    /// any written, collected into a list sized to them, its bound derived from that read; then
    /// the same read declaring a byte less. Expect: the first answers, holding exactly what it
    /// declared; the second is refused, typed (AUDIT 2026-11-23 §1, MB.1; lane perf-A14).
    #[test]
    fn a14_a_query_at_its_worst_case_answers_within_its_declared_bound() {
        const ROWS: usize = 3;
        const BLOB: usize = 100;
        let until = || Instant::now() + Duration::from_secs(60);
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(&dir.path().join("query.db"), until()).unwrap();
        database
            .call(until(), AnswerMax::fixed(), |c| {
                c.execute_batch("CREATE TABLE t(v BLOB NOT NULL)")?;
                for _ in 0..ROWS {
                    c.execute(
                        "INSERT INTO t VALUES (?1)",
                        vec![Value::Blob(vec![7; BLOB])],
                    )?;
                }
                Ok(())
            })
            .unwrap();
        let declared =
            std::mem::size_of::<Vec<Vec<u8>>>() + ROWS * (std::mem::size_of::<Vec<u8>>() + BLOB);
        let read = |answer: AnswerMax<Vec<Vec<u8>>>| {
            database.query(until(), answer, |c| {
                let mut s = c.prepare("SELECT v FROM t LIMIT 3")?;
                let mut blobs = Vec::with_capacity(ROWS);
                for blob in s.query_map(Vec::<Value>::new(), |row| row.get::<_, Vec<u8>>(0usize))? {
                    blobs.push(blob?);
                }
                Ok(blobs)
            })
        };
        let blobs = read(AnswerMax::bytes(declared)).unwrap();
        assert_eq!(blobs.len(), ROWS);
        assert_eq!(blobs.answer_bytes(), declared);
        assert!(matches!(
            read(AnswerMax::bytes(declared - 1)),
            Err(Refusal::TooLarge { .. })
        ));
    }

    /// AUDIT 2026-11-23 §1, MB.1 (T-MB.1.6, lane perf-A28): a request waiting for the memory
    /// budget's room hears its database's writer end. Do: on a local database, another use holding the
    /// whole budget, a call waits for room on a thread; its disk fails. Expect: the call refused
    /// `Fenced` as a call after a failed disk is, none waiting, nothing held.
    #[test]
    fn a28_a_call_waiting_for_room_hears_its_disk_fail_holding_nothing() {
        use crate::memory::Use;
        use crate::memory::tests::{BUDGET, Fed};
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(
            &dir.path().join("a28.db"),
            Instant::now() + crate::transport::HANG,
        )
        .unwrap();
        let budget = crate::memory::Budget::new(Box::new(Fed::new(Some(64 << 20), Some(0))));
        let other = budget.take(Use::Images, 64 << 20).unwrap();
        let began = Instant::now();
        let call = {
            let (database, on_call) = (database.clone(), Arc::clone(&budget));
            std::thread::spawn(move || {
                BUDGET.with(|slot| *slot.borrow_mut() = Some(on_call));
                database.call(began + crate::transport::HANG, AnswerMax::fixed(), |_| {
                    Ok(())
                })
            })
        };
        while budget.waiting() == 0 {
            assert!(!call.is_finished(), "the call was answered");
            assert!(began.elapsed() < crate::transport::HANG, "a hang");
            std::thread::yield_now();
        }
        let Writer::Local(store) = &database.shared.writer else {
            panic!("a local database");
        };
        store.fail_disk();
        while !call.is_finished() {
            assert!(began.elapsed() < crate::transport::HANG * 2, "a hang");
            std::thread::yield_now();
        }
        let refused = call.join().unwrap().err();
        assert_eq!(
            refused,
            Some(Refusal::Fenced(
                "writer disk failed; reopen after recovery".into()
            )),
            "after {:?}",
            began.elapsed()
        );
        assert_eq!(budget.waiting(), 0);
        assert_eq!(budget.held(Use::Queues), 0);
        drop(other);
    }

    fn put(value: usize) -> Operation {
        Operation::Mutation(Mutation::Records {
            namespace: "ns".into(),
            tx: Tx {
                conds: vec![Cond::Absent {
                    family: "f".into(),
                    key: b"k".to_vec(),
                }],
                writes: vec![Write::Put {
                    family: "f".into(),
                    key: b"k".to_vec(),
                    value: vec![0; value],
                }],
            },
        })
    }

    /// Do: size the admission charge of a Records get, small and largest Records writes, a head,
    /// a call, SQL batches and scans. Expect: a get charged its one row's bound (a largest key
    /// and value) and no more; a write charged what it carries, its value counted byte for byte;
    /// a head its command alone; a call, whose answer no API bound holds, the full `REQUEST_MAX`;
    /// a scan and a page of changes their limit's rows' envelopes and keys and values at their
    /// largest up to `RESULT_MAX` (lane perf-A6); an SQL batch what it carries and a result per
    /// statement on top of `RESULT_MAX` (AUDIT 2026-11-23 §1, MB.1; lane perf-A5).
    #[test]
    fn mb1_a_request_is_charged_what_it_and_its_answer_can_hold() {
        let fixed = std::mem::size_of::<Command>() + std::mem::size_of::<Answer>();
        let get = Admission::bytes_for(&Operation::Get(
            "ns".into(),
            b"k".to_vec(),
            Consistency::Linearizable,
            LockedReads::Committed,
        ));
        assert!(get >= api::KEY_MAX + api::VALUE_MAX + fixed, "{get}");
        assert!(get < api::VALUE_MAX + (64 << 10), "{get}");
        let small = Admission::bytes_for(&put(100));
        assert!(small >= 100 + fixed && small < 4096, "{small}");
        let largest = Admission::bytes_for(&put(api::VALUE_MAX));
        assert_eq!(largest - small, api::VALUE_MAX - 100);
        assert_eq!(Admission::bytes_for(&Operation::Head), fixed);
        let call: Call = Box::new(|_| Ok(Box::new(()) as Box<dyn Any + Send>));
        let declared = AnswerMax::<[u64; 3]>::fixed().get();
        assert_eq!(
            Admission::bytes_for(&Operation::Call(Mutex::new(Some(call)), declared)),
            fixed + declared,
            "a call is charged the bytes its caller declared, not the request bound"
        );
        let row = std::mem::size_of::<Row>();
        let scan = |limit| {
            Admission::bytes_for(&Operation::Scan(
                "ns".into(),
                Scan {
                    prefix: b"p".to_vec(),
                    after: None,
                    limit,
                    at: Consistency::Linearizable,
                },
                LockedReads::Committed,
            ))
        };
        assert_eq!(
            scan(1),
            fixed + 3 + row + api::KEY_MAX + api::VALUE_MAX,
            "a one-row page at its largest"
        );
        assert_eq!(
            scan(api::SCAN_MAX),
            fixed + 3 + api::SCAN_MAX * row + api::RESULT_MAX,
            "the largest page: its rows' envelopes and RESULT_MAX, not 1000 largest rows (~1 GiB)"
        );
        let changes = |limit| Admission::bytes_for(&Operation::Changes("ns".into(), 0, limit));
        let change = std::mem::size_of::<Change>();
        assert_eq!(
            changes(1),
            fixed + 2 + change + api::KEY_MAX + api::VALUE_MAX
        );
        assert_eq!(
            changes(api::SCAN_MAX),
            fixed + 2 + api::SCAN_MAX * change + api::RESULT_MAX
        );
        let empty = Admission::bytes_for(&Operation::Mutation(Mutation::Sql(Vec::new())));
        assert_eq!(empty, fixed + api::RESULT_MAX);
        let one = Admission::bytes_for(&Operation::Mutation(Mutation::Sql(vec![Statement {
            sql: "SELECT ?1".into(),
            params: vec![Value::Blob(vec![0; 1000])],
        }])));
        assert_eq!(
            one - empty,
            std::mem::size_of::<Statement>()
                + "SELECT ?1".len()
                + std::mem::size_of::<Value>()
                + 1000
                + std::mem::size_of::<SqlResult>(),
            "a statement's text and parameters byte for byte, and its result's envelope"
        );
    }

    /// Do: write 40 rows of `VALUE_MAX` values (40 MiB, five transactions of eight), then scan
    /// them and read their changes at the largest limit, continuing from each page. Expect: the
    /// first page of each ends before its limit, at `RESULT_MAX` bytes of keys and values, and
    /// says more follow; no page carries more than that or cuts a transaction's revision; every
    /// row is reached once, in order, by continuing, so the charge's `RESULT_MAX` bounds every
    /// page (AUDIT 2026-11-23 §1; lane perf-A6).
    #[test]
    fn a6_a_page_ends_at_result_max_and_continuing_reaches_every_row() {
        let until = || Instant::now() + Duration::from_secs(120);
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(&dir.path().join("records.db"), until()).unwrap();
        let records = database.namespace("ns").unwrap();
        let keys: Vec<Vec<u8>> = (0..40u8).map(|i| vec![b'k', i]).collect();
        for tx in keys.chunks(8) {
            let writes = tx
                .iter()
                .map(|key| Write::Put {
                    family: "f".into(),
                    key: key.clone(),
                    value: vec![7; api::VALUE_MAX],
                })
                .collect();
            records
                .commit(
                    Tx {
                        conds: Vec::new(),
                        writes,
                    },
                    until(),
                )
                .unwrap();
        }
        let (mut cursor, mut scanned, mut pages) = (None, Vec::new(), 0);
        loop {
            let page = records
                .scan(
                    "f",
                    Scan {
                        prefix: Vec::new(),
                        after: cursor.clone(),
                        limit: api::SCAN_MAX,
                        at: Consistency::Linearizable,
                    },
                    until(),
                )
                .unwrap();
            let carried: usize = page.rows.iter().map(|r| r.key.len() + r.value.len()).sum();
            assert!(carried <= api::RESULT_MAX, "{carried}");
            assert!(!page.rows.is_empty());
            pages += 1;
            cursor = page.rows.last().map(|r| r.key.clone());
            scanned.extend(page.rows.into_iter().map(|r| r.key));
            assert_eq!(page.more, scanned.len() < keys.len(), "exact continuation");
            if !page.more {
                break;
            }
        }
        assert_eq!(scanned, keys);
        assert!(
            pages > 1,
            "a page of 40 MiB ended at RESULT_MAX, before its limit"
        );
        let (mut after, mut changed, mut pages) = (0, Vec::new(), 0);
        loop {
            let page = records.changes("f", after, api::SCAN_MAX, until()).unwrap();
            let Some(last) = page.changes.last() else {
                break;
            };
            let carried: usize = page
                .changes
                .iter()
                .map(|c| c.key.len() + c.value.as_ref().map_or(0, Vec::len))
                .sum();
            assert!(carried <= api::RESULT_MAX, "{carried}");
            assert_eq!(page.changes.len() % 8, 0, "whole transactions only");
            after = last.rev;
            pages += 1;
            changed.extend(page.changes.into_iter().map(|c| c.key));
            if after >= page.head {
                break;
            }
        }
        assert_eq!(changed, keys);
        assert!(
            pages > 1,
            "a page of changes ended at RESULT_MAX, before its limit"
        );
    }
}

#[cfg(test)]
mod drain_pause {
    use super::next_pause;
    use std::time::Duration;

    /// Do: ask the pauses of a drained path's lease, before any window is learned and after one
    /// is. Expect: the first pause the probe's own cost, then each twice the last, no longer than
    /// the learned window (or the probe, where that is longer); before any window, doubling
    /// without one; a probe the clock can't see still grows (J1, 2026-11-11: no picked 50 µs or
    /// 1 ms; CP-T1.5 U05).
    #[test]
    fn a_drained_lease_is_probed_from_its_probes_cost_to_the_learned_window() {
        let probe = Duration::from_micros(3);
        let window = Duration::from_micros(40);
        let early = Duration::from_micros(10);
        assert_eq!(next_pause(None, probe, None, early), probe);
        assert_eq!(next_pause(None, probe, Some(window), early), probe);
        let mut pause = None;
        let mut seen = Vec::new();
        for _ in 0..6 {
            let next = next_pause(pause, probe, Some(window), early);
            seen.push(next.as_micros());
            pause = Some(next);
        }
        assert_eq!(seen, [3, 6, 12, 24, 40, 40]);
        assert_eq!(
            next_pause(Some(Duration::from_millis(8)), probe, None, early),
            Duration::from_millis(16)
        );
        let slow = Duration::from_micros(90);
        assert_eq!(next_pause(Some(window), slow, Some(window), early), slow);
        let unseen = next_pause(None, Duration::ZERO, None, Duration::ZERO);
        assert!(next_pause(Some(unseen), Duration::ZERO, None, Duration::ZERO) > unseen);
    }

    /// Do: ask the pauses of a wait that has outlasted the learned window: a lease held to the
    /// deadline (K1) after a drain of 40 µs was learned. Expect: the pauses go on doubling past
    /// the window, each at most the wait so far, so a second of waiting takes about twenty probes,
    /// not the 25,000 a window's pace would (amphora-seam-owner-2's ruling,
    /// amphora-owner-t1-constants-code-20261111; CP-T1.5 U05).
    #[test]
    fn a_wait_that_outlasts_the_learned_window_keeps_doubling() {
        let probe = Duration::from_micros(3);
        let window = Duration::from_micros(40);
        let mut waited = Duration::ZERO;
        let mut pause = None;
        let mut probes = 0u32;
        while waited < Duration::from_secs(1) {
            let next = next_pause(pause, probe, Some(window), waited);
            assert!(
                next <= window.max(waited).max(probe),
                "{next:?} at {waited:?}"
            );
            waited += next + probe;
            pause = Some(next);
            probes += 1;
        }
        assert!(probes < 40, "{probes} probes in a second");
    }
}

#[cfg(test)]
mod group_holding {
    use super::*;
    use crate::groups::Groups;
    use crate::reshard::directory::Directory;
    use crate::reshard::holding::{Fenced, Holding};
    use crate::reshard::{GroupId, NodeId, OpId, Span, position};

    fn local(path: &Path) -> anyhow::Result<Local> {
        let db = Db::open(path)?;
        bootstrap_db(&db).map_err(|r| anyhow::anyhow!("{r:?}"))?;
        Ok(Local {
            db,
            restoring: false,
        })
    }

    /// One Records write of `key` in namespace `ns`, family `f` (a value, or none: a delete),
    /// committed through the engine's own `apply`: the revision it made.
    fn write(local: &mut Local, key: &str, value: Option<&[u8]>) -> Rev {
        let (family, key) = ("f".to_owned(), key.as_bytes().to_vec());
        let write = match value {
            Some(v) => Write::Put {
                family,
                key,
                value: v.to_vec(),
            },
            None => Write::Delete { family, key },
        };
        let answer: Reply = Arc::new(Mutex::new(None));
        let command = Command {
            namespace: "ns".into(),
            application: None,
            operation: Operation::Mutation(Mutation::Records {
                namespace: "ns".into(),
                tx: Tx {
                    conds: Vec::new(),
                    writes: vec![write],
                },
            }),
            answer: Arc::clone(&answer),
            publication: Arc::new(Mutex::new(None)),
            until: Instant::now() + Duration::from_secs(60),
        };
        local.apply(&[command]).unwrap();
        match answer.lock().unwrap().take() {
            Some(Ok(Answer::Rev(at))) => at,
            _ => panic!("a Records write not committed"),
        }
    }

    /// Each key of `keys` as `local` answers it now, and its namespace's live keys and head.
    fn state(local: &Local, keys: &[String]) -> (Vec<Option<Vec<u8>>>, u64, u64) {
        let rows = keys
            .iter()
            .map(|k| {
                get(
                    &local.db,
                    "ns",
                    "f",
                    k.as_bytes(),
                    u64::try_from(i64::MAX).unwrap(),
                )
                .unwrap()
                .map(|r| r.value)
            })
            .collect();
        let (mut live, mut head) = (0, 0);
        local
            .db
            .query(
                "SELECT live, head FROM __amphora_namespaces WHERE name = 'ns'",
                [],
                |r| {
                    live = rev(r, 0)?;
                    head = rev(r, 1)?;
                    Ok(false)
                },
            )
            .unwrap();
        (rows, live, head)
    }

    /// T-T1.9.R19 (CP-T1.9; docs/design/reshard.md, decision "The size cut from the B-tree's middle
    /// key"): a local database made a group's, then its Records keys written. Expect: with one key,
    /// no size cut; with eight, and again with more than one page of its position index holds (a
    /// page over 16 bytes a key, each key's cell being longer), a cut with weight on each side, and
    /// keys on each side of it.
    #[test]
    fn t_t1_9_r19_a_local_databases_size_cut_is_its_position_index_middle_key() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = local(&dir.path().join("g.db")).unwrap();
        db.keep_holding(&postcard::to_allocvec(&Holding::owning(Some(Span::ALL))).unwrap())
            .unwrap();
        write(&mut db, "k0", Some(b"v"));
        assert_eq!(db.size_cut().unwrap(), None);
        let mut page = 0;
        db.db
            .query("PRAGMA page_size", [], |r| {
                page = rev(r, 0)?;
                Ok(false)
            })
            .unwrap();
        let past_a_page = usize::try_from(page / 16 + 1).unwrap();
        let mut written = 1;
        for target in [8, past_a_page] {
            while written < target {
                write(&mut db, &format!("k{written}"), Some(b"v"));
                written += 1;
            }
            let cut = db.size_cut().unwrap().unwrap();
            assert!(cut.left > 0 && cut.right > 0, "{cut:?}");
            let below = (0..target)
                .filter(|i| {
                    let id =
                        crate::reshard::row_key("ns", "f", format!("k{i}").as_bytes()).unwrap();
                    crate::reshard::position_of(&id) < cut.at
                })
                .count();
            assert!(
                below > 0 && below < target,
                "{below} of {target} below {cut:?}"
            );
        }
    }

    /// T-T1.9.R9 (CP-T1.9; docs/design/reshard.md, "A row routes by its namespace, family and
    /// key"): a local database that already holds Records rows becomes a group's (its first
    /// holding kept), its log held, its keys copied by position in batches of seven positions
    /// into a second group's database, then changed (updates, deletes, new keys) and caught up
    /// from its log, then swept. Expect: the second answers every key as the first does, with
    /// the same live count and a head no older than every revision moved; a catch-up asked from
    /// before the log was held is answered "gone"; the sweep leaves the first with no key; and
    /// the log's head moved one entry a commit.
    #[test]
    fn t_t1_9_r9_a_local_databases_records_move_by_position_through_its_log() {
        let dir = tempfile::tempdir().unwrap();
        let mut old = local(&dir.path().join("old.db")).unwrap();
        let mut new = local(&dir.path().join("new.db")).unwrap();
        let mut keys: Vec<String> = (0..40).map(|i| format!("k{i}")).collect();
        for (i, k) in keys.iter().enumerate().take(10) {
            write(&mut old, k, Some(format!("before-{i}").as_bytes()));
        }
        let holding = postcard::to_allocvec(&Holding::owning(Some(Span::ALL))).unwrap();
        old.keep_holding(&holding).unwrap();
        new.keep_holding(&postcard::to_allocvec(&Holding::owning(None)).unwrap())
            .unwrap();
        assert_eq!(old.head().unwrap(), 1);
        for (i, k) in keys.iter().enumerate().skip(10).take(20) {
            write(&mut old, k, Some(format!("v-{i}").as_bytes()));
        }
        assert_eq!(old.head().unwrap(), 21);

        let first = old.head().unwrap();
        old.keep_log(Some(first)).unwrap();
        let mut from = Some(Span::ALL);
        let mut at = first;
        while let Some(rest) = from {
            let batch = old.export(rest, 7).unwrap();
            assert!(batch.keys.len() <= 7);
            new.import(&batch.keys).unwrap();
            at = batch.at;
            from = match (batch.done, batch.last) {
                (true, _) | (_, None) => None,
                (false, Some(last)) => Span::new(last + 1, Span::ALL.hi()),
            };
        }
        // Writes go on after the copy read: updates, deletes and new keys.
        for k in keys.iter().take(5) {
            write(&mut old, k, Some(b"after"));
        }
        for k in keys.iter().skip(5).take(5) {
            write(&mut old, k, None);
        }
        for k in keys.iter().skip(30) {
            write(&mut old, k, Some(b"new"));
        }
        let mut after = at;
        loop {
            let changed = old.changed(after, u64::MAX, Span::ALL, 3).unwrap().unwrap();
            new.import(&changed.keys).unwrap();
            after = changed.through;
            if after == changed.head {
                break;
            }
        }
        keys.push("never".into());
        let (want, live, head) = state(&old, &keys);
        let (got, new_live, new_head) = state(&new, &keys);
        assert_eq!(got, want);
        assert_eq!(new_live, live);
        assert_eq!(live, 35);
        assert!(new_head >= head);
        // A catch-up from before the log was held is refused: the log doesn't reach back.
        assert!(old.changed(0, first, Span::ALL, 10).unwrap().is_none());

        old.keep_log(None).unwrap();
        while !old.sweep(Span::ALL, 7, &|_| true).unwrap() {}
        let (gone, gone_live, _) = state(&old, &keys);
        assert!(gone.iter().all(Option::is_none));
        assert_eq!(gone_live, 0);
        assert!(old.export(Span::ALL, usize::MAX).unwrap().keys.is_empty());
    }

    /// `operation` as a command of namespace `ns`, and where its answer goes.
    fn command(operation: Operation) -> (Command, Reply) {
        let answer: Reply = Arc::new(Mutex::new(None));
        let command = Command {
            namespace: "ns".into(),
            application: None,
            operation,
            answer: Arc::clone(&answer),
            publication: Arc::new(Mutex::new(None)),
            until: Instant::now() + Duration::from_secs(60),
        };
        (command, answer)
    }

    /// A Records transaction of `ns` putting each of `puts` (family `f`).
    fn puts(puts: &[(&str, &str)]) -> Operation {
        let writes = puts
            .iter()
            .map(|(k, v)| Write::Put {
                family: "f".into(),
                key: k.as_bytes().to_vec(),
                value: v.as_bytes().to_vec(),
            })
            .collect();
        Operation::Mutation(Mutation::Records {
            namespace: "ns".into(),
            tx: Tx {
                conds: Vec::new(),
                writes,
            },
        })
    }

    /// `operation` handed to the group its keys route to and durable there: its answer, or the
    /// group's refusal.
    fn submit(groups: &Groups<Local>, operation: Operation) -> Result<Answer, store::Refused> {
        let until = Instant::now() + Duration::from_secs(60);
        let (command, answer) = command(operation);
        let ticket = groups.put_wait(vec![command], until)?;
        groups.durable(&ticket, until)?;
        match answer.lock().unwrap().take() {
            Some(Ok(answer)) => Ok(answer),
            Some(Err(e)) => panic!("refused by the engine: {e:?}"),
            None => panic!("no answer"),
        }
    }

    /// `key`'s value (family `f` of `ns`) as its group answers it now.
    fn value(groups: &Groups<Local>, key: &str) -> Option<Vec<u8>> {
        let get = Operation::Get(
            "f".into(),
            key.as_bytes().to_vec(),
            Consistency::Linearizable,
            LockedReads::Committed,
        );
        match submit(groups, get) {
            Ok(Answer::Row(row)) => row.map(|r| r.value),
            _ => panic!("{key} not read"),
        }
    }

    /// `key`'s routing key.
    fn routing(key: &str) -> Vec<u8> {
        reshard::row_key("ns", "f", key.as_bytes()).unwrap()
    }

    /// Carries `out` and every action and answer after it to `groups` until the driver is idle,
    /// writing the next of `keys` in a Records transaction before each action, its value kept in
    /// `acked` once it is durable (a write refused while its span is fenced, or flipped to a
    /// group that hasn't taken it up, isn't acknowledged): the operations ended, each done.
    fn drive(
        dir: &Path,
        groups: &Groups<Local>,
        driver: &mut crate::reshard::driver::Driver,
        out: Vec<crate::reshard::driver::Out>,
        keys: &[String],
        acked: &mut [Option<String>],
        next: &mut usize,
    ) -> usize {
        use crate::reshard::driver::{Answer as Answered, Out};
        let until = Instant::now() + Duration::from_secs(60);
        let mut pending: std::collections::VecDeque<Out> = out.into();
        let mut ended = 0;
        while let Some(o) = pending.pop_front() {
            match o {
                Out::Commit(_) => groups.route(driver.directory()).unwrap(),
                Out::Send(a) => {
                    let i = *next % keys.len();
                    let v = format!("v{next}");
                    *next += 1;
                    match submit(groups, puts(&[(&keys[i], &v)])) {
                        Ok(Answer::Rev(_)) => acked[i] = Some(v),
                        Ok(_) => panic!("a commit answered with no revision"),
                        Err(store::Refused::Away(_)) => {}
                        Err(e) => panic!("a write refused: {e:?}"),
                    }
                    let reply = groups
                        .carry(&a, until, &crate::groups::Alone, |g| {
                            local(&dir.join(format!("g{}.db", g.0)))
                        })
                        .unwrap();
                    pending.extend(
                        driver
                            .answer(Answered {
                                ask: a.ask,
                                op: a.op,
                                reply,
                            })
                            .unwrap(),
                    );
                }
                Out::Ended(e) => {
                    assert!(e.done, "operation given up: {e:?}");
                    ended += 1;
                }
            }
        }
        assert!(driver.idle());
        ended
    }

    /// T-T1.9.R13 (CP-T1.9; docs/design/reshard.md, decision "The routing key is bytes"): local
    /// databases as groups (`Groups<Local>`), Records commands routed by their row keys; one
    /// group split by the driver through the shell while a Records transaction is written before
    /// every action, then merged back the same way. Expect each operation done, some keys in the
    /// split's new group and not all, every acknowledged write read back through its group after
    /// each, writes acknowledged during both, and, while split, a transaction whose keys are in
    /// one group committed there and one whose keys (written or only a condition's) span both
    /// groups refused whole.
    #[test]
    fn t_t1_9_r13_r17_records_through_groups_of_local_databases_split_and_merge_under_writes() {
        use crate::reshard::directory::Proposal;
        use crate::reshard::driver::Driver;
        let dir = tempfile::tempdir().unwrap();
        let directory = Directory::new(NodeId(0));
        let groups = Groups::on(
            NodeId(0),
            &directory,
            store::Config::default(),
            &dir.path().join("retired"),
            |g| local(&dir.path().join(format!("g{}.db", g.0))),
        )
        .unwrap();
        let keys: Vec<String> = (0..48).map(|i| format!("k{i}")).collect();
        let mut acked: Vec<Option<String>> = vec![None; keys.len()];
        for (i, k) in keys.iter().enumerate() {
            assert!(matches!(
                submit(&groups, puts(&[(k, "first")])),
                Ok(Answer::Rev(_))
            ));
            acked[i] = Some("first".into());
        }
        let every_write_reads_back = |acked: &[Option<String>]| {
            for (k, want) in keys.iter().zip(acked) {
                let want = want.as_ref().map(|v| v.as_bytes().to_vec());
                assert_eq!(value(&groups, k), want, "{k}");
            }
        };
        let (mut driver, out) = Driver::recover(directory, 1).unwrap();
        assert!(out.is_empty());
        let mut next = 0;

        let out = driver
            .begin(&Proposal::Split {
                group: GroupId(0),
                at: 1 << 63,
            })
            .unwrap();
        let ended = drive(
            dir.path(),
            &groups,
            &mut driver,
            out,
            &keys,
            &mut acked,
            &mut next,
        );
        assert_eq!(ended, 1);
        assert_eq!(groups.len(), 2);
        // T-T1.9.R17: each local database's size, as the shell measures it, is its pages in use.
        let measured = groups
            .measure(Instant::now() + Duration::from_secs(60))
            .unwrap();
        assert_eq!(measured.len(), 2);
        assert!(measured.iter().all(|m| m.bytes > 0));
        let (old, new): (Vec<&String>, Vec<&String>) = keys
            .iter()
            .partition(|k| groups.group_of(&routing(k)) == Some(GroupId(0)));
        assert!(!old.is_empty() && !new.is_empty());
        every_write_reads_back(&acked);
        assert!(acked.iter().any(|v| v.as_deref() != Some("first")));

        // A transaction within one group commits there; one across both is refused whole.
        let (a, b) = (new[0].as_str(), new[1].as_str());
        assert!(matches!(
            submit(&groups, puts(&[(a, "pair"), (b, "pair")])),
            Ok(Answer::Rev(_))
        ));
        for k in [a, b] {
            let i = keys.iter().position(|x| x == k).unwrap();
            acked[i] = Some("pair".into());
        }
        let across = puts(&[(a, "across"), (old[0].as_str(), "across")]);
        assert_eq!(submit(&groups, across).err(), Some(store::Refused::Groups));
        let Operation::Mutation(Mutation::Records { namespace, mut tx }) = puts(&[(a, "cond")])
        else {
            panic!("a Records transaction")
        };
        tx.conds.push(Cond::Absent {
            family: "f".into(),
            key: old[0].as_bytes().to_vec(),
        });
        let cond = Operation::Mutation(Mutation::Records { namespace, tx });
        assert_eq!(submit(&groups, cond).err(), Some(store::Refused::Groups));
        every_write_reads_back(&acked);

        let out = driver
            .begin(&Proposal::Merge {
                from: GroupId(1),
                into: GroupId(0),
            })
            .unwrap();
        let before = next;
        let ended = drive(
            dir.path(),
            &groups,
            &mut driver,
            out,
            &keys,
            &mut acked,
            &mut next,
        );
        assert_eq!(ended, 1);
        assert!(next > before);
        assert_eq!(groups.len(), 1);
        assert!(
            keys.iter()
                .all(|k| groups.group_of(&routing(k)) == Some(GroupId(0)))
        );
        every_write_reads_back(&acked);
    }

    /// T-T1.9.R7 (CP-T1.9; docs/design/reshard.md, "A local database keeps its holding through
    /// its command"): a local database as a group's engine, its holding fenced through the shell
    /// ([`Groups::hold`]), then the group started again on the same file; expect the fenced
    /// holding answered after the restart, kept in the file as the `Holding` command left it.
    #[test]
    fn t_t1_9_r7_a_local_databases_holding_is_kept_through_its_command_across_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g0.db");
        let retired = dir.path().join("retired");
        let directory = Directory::new(NodeId(0));
        let until = Instant::now() + Duration::from_secs(60);
        let groups = Groups::on(
            NodeId(0),
            &directory,
            store::Config::default(),
            &retired,
            |_| local(&path),
        )
        .unwrap();
        assert_eq!(
            groups.holding_of(GroupId(0)).unwrap().owned(),
            Some(Span::ALL)
        );
        let pos = position("database");
        let span = Span::new(pos, pos).unwrap();
        let fenced = groups
            .hold(GroupId(0), until, |h| h.fence(OpId(1), span, 1))
            .unwrap();
        assert_eq!(fenced, Fenced::New(1));
        let held = groups.holding_of(GroupId(0)).unwrap();
        drop(groups);

        let again = Groups::on(
            NodeId(0),
            &directory,
            store::Config::default(),
            &retired,
            |_| local(&path),
        )
        .unwrap();
        assert_eq!(again.holding_of(GroupId(0)), Some(held));
        drop(again);
        let mut engine = local(&path).unwrap();
        assert_eq!(
            engine.holding().unwrap(),
            Some(postcard::to_allocvec(&held).unwrap())
        );
    }
}

#[cfg(test)]
mod records_reads {
    use super::*;

    /// Do: size a store's readers for machines of 1 to 64 cores and of 1 GiB to 1 TiB, and one
    /// that says nothing. Expect: one a core while a quarter of memory holds that many readers'
    /// 8 MiB caches, fewer when it doesn't, never none (AUDIT 2.5: no picked reader count).
    #[test]
    fn audit_2_5_readers_are_sized_from_cores_and_memory() {
        const GIB: u64 = 1 << 30;
        assert_eq!(store::reader_room(Some(32), Some(64 * GIB)), 32);
        assert_eq!(store::reader_room(Some(1), Some(1024 * GIB)), 1);
        // A quarter of 128 MiB is 32 MiB: four 8 MiB caches, though there are 64 cores.
        assert_eq!(store::reader_room(Some(64), Some(128 << 20)), 4);
        // The page pool's floor (16 MiB) holds two.
        assert_eq!(store::reader_room(Some(64), Some(1 << 20)), 2);
        assert_eq!(store::reader_room(Some(8), None), 8);
        assert_eq!(store::reader_room(None, None), 1);
        assert_eq!(store::reader_room(Some(0), Some(0)), 1);
    }

    fn put(key: &[u8], value: &[u8]) -> Tx {
        Tx {
            conds: Vec::new(),
            writes: vec![Write::Put {
                family: "f".into(),
                key: key.to_vec(),
                value: value.to_vec(),
            }],
        }
    }

    /// Do: from many more threads than the store has readers, each commit a key and get it at
    /// once, over and over, so reads wait for readers and share their read transactions. Expect:
    /// every get answers the value its own commit just wrote, at its revision or later
    /// (read-your-writes on readers batched together, AUDIT 2.5), and none is refused.
    #[test]
    fn audit_2_5_batched_reads_see_their_callers_writes() {
        let dir = tempfile::tempdir().unwrap();
        let until = || Instant::now() + Duration::from_secs(60);
        let db = Database::open(&dir.path().join("r.db"), until()).unwrap();
        let ns = db.namespace("t").unwrap();
        let clients = 4 * match &db.shared.writer {
            Writer::Local(store) => store.readers(),
            _ => panic!("a local database"),
        };
        std::thread::scope(|s| {
            for c in 0..clients {
                let ns = &ns;
                s.spawn(move || {
                    let key = format!("k{c}").into_bytes();
                    for round in 0..20u32 {
                        let value = round.to_le_bytes();
                        let at = ns.commit(put(&key, &value), until()).unwrap();
                        let row = ns
                            .get("f", &key, Consistency::Linearizable, until())
                            .unwrap()
                            .expect("the row just written");
                        assert_eq!(row.value, value, "client {c} round {round}");
                        assert!(row.rev >= at, "client {c} round {round}");
                    }
                });
            }
        });
        assert_eq!(
            locked(&db.shared.reads.queue).unwrap().reading,
            0,
            "every reader stopped once none waited"
        );
    }

    /// Do: release 8x as many gets as the store has readers at once, so readers finish their
    /// batches together and hand their places to the same first waiting reads; a quarter of them
    /// with deadlines short enough to run out while waiting, so some leave with places handed to
    /// them. Fifty rounds. Expect: after every round no read waits and no reader is counted (each
    /// hand-over gives back exactly one place, AUDIT 2.5), every get either answers the row or is
    /// refused for its deadline, and a get afterwards still finds a reader.
    #[test]
    fn audit_2_5_readers_finishing_together_lose_no_place() {
        let dir = tempfile::tempdir().unwrap();
        let until = || Instant::now() + Duration::from_secs(60);
        let db = Database::open(&dir.path().join("r.db"), until()).unwrap();
        let ns = db.namespace("t").unwrap();
        ns.commit(put(b"k", b"v"), until()).unwrap();
        let clients = 8 * match &db.shared.writer {
            Writer::Local(store) => store.readers(),
            _ => panic!("a local database"),
        };
        let start = std::sync::Barrier::new(clients);
        for round in 0..50 {
            std::thread::scope(|s| {
                for c in 0..clients {
                    let (ns, start) = (&ns, &start);
                    s.spawn(move || {
                        start.wait();
                        let deadline = if c % 4 == 0 {
                            Instant::now() + Duration::from_micros(200)
                        } else {
                            until()
                        };
                        match ns.get("f", b"k", Consistency::Linearizable, deadline) {
                            Ok(row) => assert_eq!(
                                row.expect("the row").value,
                                b"v",
                                "round {round} client {c}"
                            ),
                            Err(refusal) => assert!(
                                c % 4 == 0 && matches!(refusal, Refusal::Timeout),
                                "round {round} client {c}: {refusal:?}"
                            ),
                        }
                    });
                }
            });
            let queue = locked(&db.shared.reads.queue).unwrap();
            assert!(
                queue.waiting.is_empty(),
                "round {round}: no read left waiting"
            );
            assert_eq!(queue.reading, 0, "round {round}: every place given back");
        }
        let row = ns
            .get("f", b"k", Consistency::Linearizable, until())
            .unwrap();
        assert_eq!(row.expect("the row").value, b"v");
    }
}
