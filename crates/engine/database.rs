//! The [`Database`] object and everything needed to open one: open-time
//! options, the async open state machine, the process-wide registry that
//! keeps a single `Database` per file, and the per-connection catalog of
//! attached databases.

use crate::types::IOResultOr;
use crate::util::IOExt;
#[cfg(feature = "io_memory_yield")]
use crate::MemoryYieldIO;
#[cfg(all(feature = "fs", target_os = "linux", feature = "io_uring", not(miri)))]
use crate::UringIO;
#[cfg(all(
    feature = "fs",
    target_os = "windows",
    feature = "experimental_win_iocp",
    not(miri)
))]
use crate::WindowsIOCP;
use crate::{
    alloc, bail_corrupt_error,
    busy::BusyHandler,
    ext,
    incremental::view::AllViewsTxState,
    io, io_error, io_yield_one, mvcc,
    progress::ProgressHandler,
    return_if_io,
    schema::{self, Schema},
    stats::{
        refresh_analyze_stats_on_connect, refresh_analyze_stats_on_connect_nonblock,
        RefreshAnalyzeStatsState,
    },
    storage::{
        self,
        checksum::CHECKSUM_REQUIRED_RESERVED_BYTES,
        encryption::{AtomicCipherMode, SQLITE_HEADER, TURSO_HEADER_PREFIX},
        journal_mode,
        page_cache::PageCache,
        page_transform::PageTransform,
        pager::{self, AutoVacuumMode, HeaderRef, HeaderRefMut, SharedPagerState},
        sqlite3_ondisk::{DatabaseHeader, PageSize, RawVersion, TextEncoding, Version},
    },
    sync::{
        self,
        atomic::{
            AtomicBool, AtomicI32, AtomicI64, AtomicIsize, AtomicU32, AtomicU64, AtomicU8,
            AtomicUsize, Ordering,
        },
        Arc, LazyLock, Mutex, RwLock, Weak,
    },
    turso_assert, turso_assert_greater_than_or_equal,
    types::{self, IOCompletions},
    vdbe::metrics::ConnectionMetrics,
    AtomicSyncMode, AtomicTempStore, AtomicTransactionState, Buffer, BufferPool, CipherMode,
    Completion, CompletionError, Connection, DatabaseStorage, Dialect, EncryptionKey, IOResult,
    InternalVirtualTable, LimboError, MemoryIO, MvStore, OpenFlags, PageCodec, PageCodecId,
    PageRef, Pager, PlatformIO, Result, SharedLog, SymbolTable, SyncMode, SyscallIO, TempStore,
    TransactionState, VirtualTable, Wal, WalAutoActions, WalFile, WalFileShared, IO,
};
use arc_swap::{ArcSwap, ArcSwapOption};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::any::Any;
#[cfg(host_shared_wal)]
use std::path::Path;
#[cfg(host_shared_wal)]
use std::sync::OnceLock;
use std::{fmt, ops::Deref};
#[cfg(feature = "fs")]
use storage::database::DatabaseFile;
#[cfg(host_shared_wal)]
use storage::shared_wal_coordination::MappedSharedWalCoordination;
use tracing::{instrument, Level};

/// Configuration for database features
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DatabaseOpts {
    pub enable_views: bool,
    pub enable_custom_types: bool,
    pub enable_encryption: bool,
    pub enable_index_method: bool,
    pub enable_autovacuum: bool,
    pub enable_vacuum: bool,
    pub enable_attach: bool,
    pub enable_generated_columns: bool,
    pub enable_multiprocess_wal: bool,
    pub enable_without_rowid: bool,
    pub enable_experimental_mvcc_passive_checkpoint: bool,
    pub unsafe_testing: bool,
    pub(crate) enable_load_extension: bool,
}

impl DatabaseOpts {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(feature = "cli_only")]
    pub fn turso_cli(mut self) -> Self {
        self.enable_load_extension = true;
        self
    }

    pub fn with_views(mut self, enable: bool) -> Self {
        self.enable_views = enable;
        self
    }

    pub fn with_custom_types(mut self, enable: bool) -> Self {
        self.enable_custom_types = enable;
        self
    }

    pub fn with_encryption(mut self, enable: bool) -> Self {
        self.enable_encryption = enable;
        self
    }

    pub fn with_index_method(mut self, enable: bool) -> Self {
        self.enable_index_method = enable;
        self
    }

    pub fn with_autovacuum(mut self, enable: bool) -> Self {
        self.enable_autovacuum = enable;
        self
    }

    pub fn with_vacuum(mut self, enable: bool) -> Self {
        self.enable_vacuum = enable;
        self
    }

    pub fn with_experimental_mvcc_passive_checkpoint(mut self, enable: bool) -> Self {
        self.enable_experimental_mvcc_passive_checkpoint = enable;
        self
    }

    pub fn with_attach(mut self, enable: bool) -> Self {
        self.enable_attach = enable;
        self
    }

    pub fn with_generated_columns(mut self, enable: bool) -> Self {
        self.enable_generated_columns = enable;
        self
    }

    pub fn with_multiprocess_wal(mut self, enable: bool) -> Self {
        self.enable_multiprocess_wal = enable;
        self
    }

    pub fn with_without_rowid(mut self, enable: bool) -> Self {
        self.enable_without_rowid = enable;
        self
    }

    pub fn with_unsafe_testing(mut self, enable: bool) -> Self {
        self.unsafe_testing = enable;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedWalCoordinationOpenTelemetryMode {
    Exclusive,
    MultiProcess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedWalOpenTelemetry {
    pub loaded_from_disk_scan: bool,
    pub reopened_max_frame: u64,
    pub reopened_nbackfills: u64,
    pub reopened_checkpoint_seq: u32,
    pub coordination_open_mode: Option<SharedWalCoordinationOpenTelemetryMode>,
    pub sanitized_backfill_proof_on_open: bool,
}

#[cfg(feature = "simulator")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedWalTestingSnapshot {
    pub max_frame: u64,
    pub nbackfills: u64,
    pub checkpoint_seq: u32,
    pub frame_index_overflowed: bool,
}

#[derive(Clone, Debug, Default)]
pub struct EncryptionOpts {
    pub cipher: String,
    pub hexkey: String,
}

impl EncryptionOpts {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Clone, Debug, Default)]
pub struct DatabaseAllocators<
    M: alloc::ConcurrentAllocator = alloc::DynAllocator,
    F: alloc::ConcurrentAllocator = alloc::DynAllocator,
> {
    pub mv_store: M,
    pub fts: F,
}

/// Options for opening a [`Database`].
///
/// Mirrors the `std::fs::OpenOptions` idiom: configure, then open.
///
/// ```ignore
/// let db = Database::open(
///     io,
///     "app.db",
///     OpenOptions::new(Arc::new(SqliteDialect)).flags(OpenFlags::ReadOnly),
/// )?;
/// ```
#[derive(Clone)]
pub struct OpenOptions {
    /// Pre-opened database storage for the file at the database path.
    storage: Option<Arc<dyn DatabaseStorage>>,
    /// WAL file path override. Defaults to `"{path}-wal"`. Only honored by
    /// [`Database::do_open`]/[`Database::do_open_async`]; the registry-aware
    /// [`Database::open`]/[`Database::open_async`] reject it, because the
    /// process-wide registry keys on the default WAL for a path.
    wal_path: Option<String>,
    flags: OpenFlags,
    db_opts: DatabaseOpts,
    pub(crate) native_extensions: crate::native_ext::NativeExtensions,
    encryption: Option<EncryptionOpts>,
    page_codec: Option<Arc<dyn PageCodec>>,
    durable_storage: Option<Arc<dyn crate::mvcc::persistent_storage::DurableStorage>>,
    allocators: DatabaseAllocators,
    /// SQL dialect the database is opened with. The dialect is fixed at open
    /// time and shared by every user of the registered instance; a registry
    /// hit with a different dialect is an error.
    dialect: Arc<dyn Dialect>,
}

impl OpenOptions {
    /// The dialect has no default: it is fixed at open time and shared by
    /// every user of the instance, so the caller must choose it explicitly.
    pub fn new(dialect: Arc<dyn Dialect>) -> Self {
        let options = Self {
            storage: None,
            wal_path: None,
            flags: OpenFlags::default(),
            db_opts: DatabaseOpts::default(),
            native_extensions: crate::native_ext::NativeExtensions::default(),
            encryption: None,
            page_codec: None,
            durable_storage: None,
            allocators: DatabaseAllocators::default(),
            dialect: dialect.clone(),
        };
        dialect.register_native_extensions(options)
    }

    pub fn storage(mut self, storage: Arc<dyn DatabaseStorage>) -> Self {
        self.storage = Some(storage);
        self
    }

    /// Override the WAL file path (defaults to `"{path}-wal"`). Only honored
    /// by [`Database::do_open`]/[`Database::do_open_async`]; passing it to the
    /// registry-aware entry points is an error.
    pub fn wal_path(mut self, wal_path: impl Into<String>) -> Self {
        self.wal_path = Some(wal_path.into());
        self
    }

    pub fn flags(mut self, flags: OpenFlags) -> Self {
        self.flags = flags;
        self
    }

    pub fn db_opts(mut self, db_opts: DatabaseOpts) -> Self {
        self.db_opts = db_opts;
        self
    }

    pub fn encryption(mut self, encryption: impl Into<Option<EncryptionOpts>>) -> Self {
        self.encryption = encryption.into();
        self
    }

    pub fn page_codec(mut self, page_codec: impl Into<Option<Arc<dyn PageCodec>>>) -> Self {
        self.page_codec = page_codec.into();
        self
    }

    pub fn durable_storage(
        mut self,
        durable_storage: impl Into<Option<Arc<dyn crate::mvcc::persistent_storage::DurableStorage>>>,
    ) -> Self {
        self.durable_storage = durable_storage.into();
        self
    }

    pub fn allocators(mut self, allocators: DatabaseAllocators) -> Self {
        self.allocators = allocators;
        self
    }
}

/// Returns true for in memory databases (i.e. databases backed by MemoryIO)
///
/// Turso treats every path with the `:memory:` prefix as a named
/// in-memory database.
pub(crate) fn is_memory_like(path: &str) -> bool {
    path.starts_with(":memory:") || path.starts_with("file::memory:") || path.is_empty()
}

/// Creates a read completion for database header reads that checks for short reads.
/// The header is always on page 1, so this function hardcodes that page index.
fn new_header_read_completion(buf: Arc<Buffer>) -> Completion {
    let expected = buf.len();
    Completion::new_read(buf, move |res| {
        let Ok((_buf, bytes_read)) = res else {
            return None; // IO error already captured in completion
        };
        if (bytes_read as usize) < expected {
            tracing::error!(
                "short read on database header: expected {expected} bytes, got {bytes_read}"
            );
            return Some(CompletionError::ShortRead {
                page_idx: 1, // header is on page 1
                expected,
                actual: bytes_read as usize,
            });
        }
        None
    })
}

/// Phase tracking for async database opening
#[derive(Default, Debug)]
pub enum OpenDbAsyncPhase {
    #[default]
    Init,
    /// Drives `Database::header_validation` (header validation + WAL recovery)
    /// as a sub state machine so WAL recovery on open does not block.
    ValidatingHeader,
    ReadingHeader,
    LoadingSchema,
    BootstrapMvStore,
    Done,
}

/// Result of [`Database::read_db_header_buf`].
pub(crate) enum DbHeaderRead {
    /// The full 512-byte header was read.
    Full(Arc<Buffer>),
    /// The file is shorter than the header. Only the first `len` bytes of
    /// `buf` came from the file, and `err` is the short-read error.
    Short {
        buf: Arc<Buffer>,
        len: usize,
        err: CompletionError,
    },
}

impl DbHeaderRead {
    /// The bytes that actually came from the file.
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Full(buf) => buf.as_slice(),
            Self::Short { buf, len, .. } => &buf.as_slice()[..*len],
        }
    }

    /// The short-read error, if the file is shorter than the header.
    fn short_read(&self) -> Option<CompletionError> {
        match self {
            Self::Full(_) => None,
            Self::Short { err, .. } => Some(*err),
        }
    }
}

/// Sub state machine for [`Database::read_db_header_buf`], the non-blocking
/// read of the 512-byte database header. Not an open phase: it is driven
/// from connect-time pager init ([`Database::_init`] via `init_pager`), which
/// runs outside the open state machine.
#[derive(Default)]
pub(crate) enum DbHeaderReadState {
    #[default]
    Start,
    Reading {
        buf: Arc<Buffer>,
        completion: Completion,
    },
}

/// Sub state machine for [`Database::_init`], driven from
/// [`HeaderValidationState::Start`]. Builds the `Pager` (reading page-size /
/// reserved bytes from the DB header), begins a read transaction, then reads
/// page 1 to determine the autovacuum mode — all without blocking.
#[derive(Default)]
pub(crate) enum InitState {
    #[default]
    Start,
    /// Driving `init_pager` (its only IO is the DB-header read).
    InitPager(DbHeaderReadState),
    /// Pager built and read-tx open; reading page 1 for the autovacuum mode.
    ReadPage1 { pager: Box<Pager> },
}

/// Sub state machine for [`Database::header_validation`], driven from
/// [`OpenDbAsyncPhase::ValidatingHeader`]. Keeps WAL recovery on open
/// non-blocking by yielding through its IO instead of `io.block`.
enum HeaderValidationState {
    Start {
        init: InitState,
    },
    /// Pager created; (re-entrant) header reads + validation. Holds the owned
    /// `Pager` because `set_wal` needs `&mut Pager`; it is `Arc`-wrapped only
    /// once validation completes. `is_readonly`/`log_exists` are captured in
    /// `Start` (before the autovacuum check may force ReadOnly) so re-entry
    /// observes the original values.
    Validate {
        pager: Box<Pager>,
        is_readonly: bool,
        log_exists: bool,
    },
    /// A modified header (e.g. Legacy→WAL conversion) must be written to disk
    /// before the WAL is attached. `completion` is the in-flight write.
    WriteHeader {
        pager: Box<Pager>,
        page: PageRef,
        open_mv_store: bool,
        completion: Option<Completion>,
    },
    /// Open/recover the shared WAL. On non-host builds `driver` drives the
    /// `OpenSharedWal` recovery scan; on host builds the WAL is produced
    /// synchronously (native, where `io.step` pumps).
    OpenWal {
        pager: Box<Pager>,
        open_mv_store: bool,
        driver: Option<storage::wal::OpenSharedWal>,
        /// Set once the WAL of an empty database file has been thrown away
        /// (see the orphan-WAL handling in [`Database::header_validation`]).
        /// The WAL is reopened after that, and the reopened WAL must come
        /// back empty, so this can only ever happen once per open.
        discarded_orphan_wal: bool,
    },
}

impl Default for HeaderValidationState {
    fn default() -> Self {
        Self::Start {
            init: InitState::default(),
        }
    }
}

/// How `Database::_connect` loads the connection's ANALYZE stats.
///
/// A new connection starts from a clone of the shared schema, which carries
/// no `sqlite_stat1` contents until some connection has loaded them, and the
/// planner uses those stats for its cost estimates. `_connect` therefore
/// refreshes them. The refresh is a `SELECT` over `sqlite_stat1`, and that
/// statement can have to wait: on page I/O, or on another transaction when
/// it speculatively reads a version whose writer is still preparing. Two
/// states exist because there are two kinds of caller for that wait:
///
/// - `Blocking`: the caller may block, so `_connect` runs the scan to
///   completion right here by pumping `io.step()`. This is the public
///   `connect` family for embedded users, where whatever the scan waits for
///   can make progress on its own, and the internal open-time and MVCC
///   bootstrap connections, for which the refresh is a no-op because the
///   database is not initialized yet.
/// - `Deferred`: the caller must not block, so `_connect` returns the bare
///   connection and the caller drives the same scan through
///   `refresh_analyze_stats_nonblock`, handing every wait to its own
///   scheduler. This is `connect_async`, for hosts that run every connection
///   cooperatively on one thread: there an explicit yield from the scan is a
///   request to run *other* connections, which `io.step()` can never satisfy,
///   so `Blocking` would spin forever.
///
/// Stats loading is never skipped outright; `Deferred` only moves it to the
/// caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatsRefresh {
    Blocking,
    Deferred,
}

/// Settings for a single connection, passed to [`Database::connect`] and
/// [`Database::connect_async`].
pub struct ConnectOpts {
    encryption_key: Option<EncryptionKey>,
    page_codec: Option<Arc<dyn PageCodec>>,
    versioned: bool,
    memory: Option<Arc<dyn crate::storage::versioned::memory::Source>>,
}

#[expect(
    clippy::new_without_default,
    reason = "callers must construct the settings with new() so that new required settings reach every caller"
)]
impl ConnectOpts {
    /// Creates settings for a connection without encryption and without an
    /// external page codec.
    pub fn new() -> Self {
        Self {
            encryption_key: None,
            page_codec: None,
            versioned: false,
            memory: None,
        }
    }

    /// Sets the key used to open an encrypted database. Use this when the key
    /// is known at connect time.
    pub fn with_encryption_key(mut self, encryption_key: Option<EncryptionKey>) -> Self {
        self.encryption_key = encryption_key;
        self
    }

    /// Sets the external page codec for this connection. The codec may
    /// contain sensitive key material, so it is installed only on the pager
    /// for this connection and is not cached on the shared `Database`.
    pub fn with_page_codec(mut self, page_codec: Arc<dyn PageCodec>) -> Self {
        self.page_codec = Some(page_codec);
        self
    }

    /// Amphora (G7 step 2, `AC-NS.2.1`): puts the database's B-trees on the versioned storage
    /// (`storage::versioned::switch`), made by the first connection that asks; every connection
    /// made after it uses it too. `AMPHORA_STORAGE=versioned` asks the same for every connection.
    pub fn with_versioned_storage(mut self) -> Self {
        self.versioned = true;
        self
    }

    /// Amphora (G7 step 2, `AC-NS.2.1`): where the versioned storage takes its memory from, its
    /// pages and row versions alike: the shell's process memory budget (`amphora::memory`, MB.1),
    /// which every database of the process shares. Without it the storages of the process share
    /// the machine's memory (`storage::versioned::memory::machine`). Read by the connection that
    /// makes the storage.
    pub fn with_memory(
        mut self,
        source: Arc<dyn crate::storage::versioned::memory::Source>,
    ) -> Self {
        self.memory = Some(source);
        self
    }
}

/// Resumable state for [`Database::connect_async`]. Create one per
/// connect and pass it to every call until `IOResult::Done`.
#[derive(Default)]
pub struct ConnectAsyncState {
    /// The connection, once created; handed out when the stats refresh ends.
    conn: Option<Arc<Connection>>,
    /// The in-flight `sqlite_stat1` scan.
    refresh: RefreshAnalyzeStatsState,
}

impl ConnectAsyncState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// State machine for async database opening
pub struct OpenDbAsyncState {
    phase: OpenDbAsyncPhase,
    db: Option<Arc<Database>>,
    pager: Option<Arc<Pager>>,
    conn: Option<Arc<Connection>>,
    encryption_key: Option<EncryptionKey>,
    make_from_btree_state: schema::MakeFromBtreeState,
    /// Schema lock held during LoadingSchema phase to ensure atomicity across IO yields
    schema_guard: Option<sync::ArcMutexGuard<Arc<Schema>>>,
    /// Registry key for insertion (computed once at start)
    pub(crate) registry_key: Option<DatabaseKey>,
    /// The database being built, held across the ValidatingHeader phase yields
    /// before it is wrapped in an `Arc`.
    building_db: Option<Database>,
    /// Sub state machine for `header_validation`, driven in ValidatingHeader.
    header_validation_state: HeaderValidationState,
    /// The dedicated bootstrap connection used by `BootstrapMvStore`, held
    /// across yields from `MvStore::bootstrap_nonblock`.
    mvcc_bootstrap_conn: Option<Arc<Connection>>,
    /// Sub state machine for `MvStore::bootstrap_nonblock`, driven in
    /// `BootstrapMvStore`.
    mvcc_bootstrap_state: mvcc::database::BootstrapState,
}

impl Default for OpenDbAsyncState {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenDbAsyncState {
    pub fn new() -> Self {
        Self {
            phase: OpenDbAsyncPhase::Init,
            db: None,
            pager: None,
            conn: None,
            encryption_key: None,
            make_from_btree_state: schema::MakeFromBtreeState::new(),
            schema_guard: None,
            registry_key: None,
            building_db: None,
            header_validation_state: HeaderValidationState::default(),
            mvcc_bootstrap_conn: None,
            mvcc_bootstrap_state: mvcc::database::BootstrapState::default(),
        }
    }
}

impl Drop for OpenDbAsyncState {
    fn drop(&mut self) {
        if let Some(registry_key) = self.registry_key.take() {
            let mut registry = DATABASE_MANAGER.lock();
            registry.remove(&registry_key);
        }
    }
}

/// Per-path entry in the database registry.
pub(crate) enum RegistryEntry {
    /// Another caller is currently opening this database. Callers that see
    /// this should yield and retry later.
    Opening,
    /// The database has been opened and is (or was) live.
    Ready(Weak<Database>),
}

/// The database manager ensures that there is a single, shared
/// `Database` object per a database file. We need because it is not safe
/// to have multiple independent WAL files open because coordination
/// happens at process-level POSIX file advisory locks.
///
/// Uses parking_lot::Mutex instead of crate::sync::Mutex because this static
/// must persist across shuttle test iterations. Shuttle resets its execution
/// state between iterations, but static variables persist - using shuttle's
/// Mutex here would cause panics when the second iteration tries to lock a
/// mutex that belongs to a stale execution context.
/// Registry key for the process-wide database manager.
/// File-backed databases are keyed by their OS-level identity (dev, ino),
/// matching SQLite's inodeList approach. Shared in-memory databases use
/// their name as the key.
///
/// IMPORTANT: The mutex must only be held for brief HashMap operations, never
/// across I/O yields. Holding it across yields deadlocks single-threaded
/// event loops because the blocked thread
/// can never resume the coroutine that owns the lock.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum DatabaseKey {
    File(io::FileId, Option<io::RegistryScope>),
    SharedMemory(String),
}

#[allow(clippy::type_complexity)]
pub(crate) static DATABASE_MANAGER: LazyLock<
    Arc<parking_lot::Mutex<HashMap<DatabaseKey, RegistryEntry>>>,
> = LazyLock::new(|| Arc::new(parking_lot::Mutex::new(HashMap::default())));

#[cfg(feature = "simulator")]
pub fn clear_database_registry() {
    DATABASE_MANAGER.lock().clear();
}

#[cfg(test)]
pub(crate) fn remove_file_from_database_registry(path: &str) {
    let file_id = io::get_file_id(path).unwrap();
    DATABASE_MANAGER
        .lock()
        .remove(&DatabaseKey::File(file_id, None));
}

/// AC-T1.1.1: scoped entries live exactly as long as their registered Database. A
/// dead weak entry must not retain a new virtual filesystem identity forever. Removal is
/// one key lookup, and an old Database cannot remove an Opening sentinel or a successor.
struct ScopedRegistration {
    key: DatabaseKey,
    owner: Weak<Database>,
}

impl Drop for ScopedRegistration {
    fn drop(&mut self) {
        let mut registry = DATABASE_MANAGER.lock();
        let owned = matches!(registry.get(&self.key), Some(RegistryEntry::Ready(owner)) if Weak::ptr_eq(owner, &self.owner));
        if owned {
            registry.remove(&self.key);
        }
    }
}

/// A committed WAL prefix captured by [`Database::wal_prefix`]. Its identity is opaque: an
/// external restore or generation restart can make an old prefix inapplicable to the current log.
#[derive(Debug, Clone)]
pub struct WalPrefix {
    header: Arc<storage::wal::HeaderCell>,
    generation: u32,
    frame: u64,
    already_durable: bool,
}

impl WalPrefix {
    /// This captured prefix was already covered by a completed successful flush. Later writes are
    /// not covered. A prefix of a shared multi-process WAL is conservatively never reported so.
    #[must_use]
    pub fn is_durable(&self) -> bool {
        self.already_durable
    }

    /// Whether `other` refers to the same WAL header cell and checkpoint generation (no log
    /// segment switch, restart, or external restore occurred between them).
    #[must_use]
    pub fn same_log_file(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.header, &other.header) && self.generation == other.generation
    }
}

/// The `Database` object contains per database file state that is shared
/// between multiple connections.
///
/// Do that `Database` object is cached and can be long lived. DO NOT store anything sensitive like
/// encryption key here.
pub struct Database<
    A: alloc::ConcurrentAllocator = alloc::DynAllocator,
    F: alloc::ConcurrentAllocator = alloc::DynAllocator,
> {
    pub(crate) mv_store: ArcSwapOption<mvcc::MvStore<mvcc::MvccClock, A>>,
    /// Amphora (G5 step 4): the skipping summaries its connections share (executor.md decision 11).
    pub(crate) skipping: crate::skipping::SkipRegistry,
    pub(crate) allocators: DatabaseAllocators<A, F>,
    pub(crate) schema: Arc<Mutex<Arc<Schema>>>,
    /// Monotonic epoch bumped whenever `schema` is mutated: lets `Connection::maybe_update_schema`
    /// verify its local snapshot is current with a single `Acquire` load instead of contending on
    /// `schema.lock()` on every statement.
    pub(crate) schema_epoch: AtomicU64,
    /// Freshness witness for `ANALYZE` statistics cached on `schema` (schema version, schema
    /// epoch, whether `sqlite_stat1` exists, and the in-process WAL token at scan time): avoids
    /// redundant connect-time `sqlite_stat1` scans while guaranteeing any direct `sqlite_stat1`
    /// DML, explicit-transaction `ANALYZE`, or external restore invalidates the cache.
    pub(crate) analyze_stats_freshness: Mutex<Option<crate::stats::AnalyzeStatsFreshness>>,
    /// Monotonic counter used to stagger per-connection `LearnedCosts::RunCounts` phases so
    /// concurrent connections do not all sample on the exact same iteration.
    pub(crate) cost_run_seed: AtomicU64,
    pub db_file: Arc<dyn DatabaseStorage>,
    pub path: String,
    wal_path: String,
    pub io: Arc<dyn IO>,
    pub(crate) buffer_pool: Arc<BufferPool>,
    // Shared structures of a Database are the parts that are common to multiple threads that might
    // create DB connections.
    _shared_page_cache: Arc<RwLock<PageCache>>,
    /// Amphora (QT.5): what its operators' work costs, learned as its queries run.
    /// Shared with its pagers, which time the pages they read from its file.
    pub(crate) learned_costs: Arc<crate::translate::optimizer::learned_costs::LearnedCosts>,
    /// Amphora (PL.9): each table's rows inserted less rows deleted since ANALYZE counted it.
    pub(crate) row_changes: Arc<crate::row_changes::RowChanges>,
    /// Amphora (PL.13, lane replan-5): filter shares filtered hash-join builds counted.
    pub(crate) filter_shares: Arc<crate::filter_shares::FilterShares>,
    /// Amphora (PL.13, lane replan-6): subquery results stored results counted.
    pub(crate) subquery_rows: Arc<crate::subquery_rows::SubqueryRows>,
    /// Amphora (PL.8, T-PL.8.10, lane pl8-17): where the exact join searches of statements
    /// prepared with the fast plan run under `BackgroundSearch::Worker`, and both placements'
    /// counters (`background_search.rs`).
    pub(crate) planner: crate::background_search::Planner,
    /// Amphora (PL.7): the cost weights fitted to its statements' measured times, and the switch
    /// the planner reads them behind.
    pub(crate) cost_fit: Arc<crate::translate::optimizer::cost_fit::DatabaseFit>,

    /// Optional per-database MVCC durable storage override.
    ///
    /// When set, MVCC will use this implementation for logical-log durability
    /// (commit, sync, checkpoint thresholds, etc.) instead of the built-in storage.
    pub(crate) durable_storage: Option<Arc<dyn crate::mvcc::persistent_storage::DurableStorage>>,
    pub(crate) shared_wal: Arc<SharedLog>,
    #[cfg(host_shared_wal)]
    shared_wal_coordination: OnceLock<Arc<MappedSharedWalCoordination>>,
    pub(crate) open_flags: OpenFlags,
    // Use parking lot RwLock here and not `crate::sync::RwLock` because it relies on `data_ptr` and that is experimental
    // in std.
    pub(crate) builtin_syms: parking_lot::RwLock<SymbolTable>,
    /// SQL dialect this database runs under, interpreting `sqlite_schema`
    /// SQL rows. Passed explicitly by every open path and shared by all
    /// connections; updated only when an empty follower installs a raw page
    /// image from a different-dialect source via [`Self::set_dialect`].
    dialect: parking_lot::RwLock<Arc<dyn Dialect>>,
    pub(crate) opts: DatabaseOpts,
    pub(crate) n_connections: AtomicUsize,
    /// Process-unique id minted at construction. Unlike the `Arc`'s heap
    /// address, this can never repeat within a process, so detach/reattach
    /// and close/reopen produce distinguishable values.
    pub(crate) incarnation: u64,

    pager_state: Arc<SharedPagerState>,

    // Encryption
    encryption_cipher_mode: AtomicCipherMode,
    page_codec_id: Option<PageCodecId>,
    /// Amphora (G7 step 2, `AC-NS.2.1`): the versioned storage its connections' B-trees are on,
    /// once a connection has asked for it (`storage::versioned::switch`).
    versioned: Mutex<Option<Arc<crate::storage::versioned::switch::Storage>>>,
    /// Last known OS file identity `(dev, ino)` of `self.path` for quiescent external restores:
    /// lets reload keep the existing locked descriptor on in-place overwrites and reopen + re-key
    /// `DATABASE_MANAGER` only when atomic rename replaces the inode.
    #[cfg(feature = "conn_raw_api")]
    file_id: Mutex<Option<io::FileId>>,
    /// Key under which this `Database` is currently registered in `DATABASE_MANAGER`, if any.
    #[cfg_attr(not(feature = "conn_raw_api"), allow(dead_code))]
    registry_key: Mutex<Option<DatabaseKey>>,
    /// Encryption key provided at `Database::open` time, retained solely for staging catalog
    /// validation during `reload_wal_after_external_restore` (never exposed to `connect` or
    /// registry lookups).
    #[cfg(feature = "conn_raw_api")]
    restore_encryption_key: Mutex<Option<EncryptionKey>>,
    // Last: release this Database's I/O fields before its scoped registry entry.
    registry_registration: Mutex<Option<ScopedRegistration>>,
}

// SAFETY: This needs to be audited for thread safety.
// See: https://github.com/tursodatabase/turso/issues/1552
crate::assert::assert_send_sync!(Database);

impl fmt::Debug for Database {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug_struct = f.debug_struct("Database");
        debug_struct
            .field("path", &self.path)
            .field("open_flags", &self.open_flags);

        // Database state information
        let db_state_value = if self.initialized() {
            "initialized"
        } else {
            "uninitialized"
        };
        debug_struct.field("db_state", &db_state_value);

        let mv_store_status = if self.get_mv_store().is_some() {
            "present"
        } else {
            "none"
        };
        debug_struct.field("mv_store", &mv_store_status);

        let init_lock_status = if self.pager_state.is_init_locked() {
            "locked"
        } else {
            "unlocked"
        };
        debug_struct.field("init_lock", &init_lock_status);

        let wal_status = match self.shared_wal.current().try_read() {
            Some(wal) if wal.metadata.enabled.load(Ordering::SeqCst) => "enabled",
            Some(_) => "disabled",
            None => "locked_for_write",
        };
        debug_struct.field("wal_state", &wal_status);

        // Page cache info (just basic stats, not full contents)
        let cache_info = match self._shared_page_cache.try_read() {
            Some(cache) => format!("( capacity {}, used: {} )", cache.capacity(), cache.len()),
            None => "locked".to_string(),
        };
        debug_struct.field("page_cache", &cache_info);

        debug_struct.field(
            "n_connections",
            &self
                .n_connections
                .load(crate::sync::atomic::Ordering::SeqCst),
        );
        debug_struct.finish()
    }
}

impl Database {
    /// Returns true if this database is backed by MemoryIO.
    pub fn is_in_memory_db(&self) -> bool {
        is_memory_like(&self.path)
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        opts: DatabaseOpts,
        flags: OpenFlags,
        path: impl Into<String>,
        wal_path: impl Into<String>,
        io: &Arc<dyn IO>,
        db_file: Arc<dyn DatabaseStorage>,
        encryption_opts: Option<EncryptionOpts>,
        allocators: DatabaseAllocators,
        page_codec_id: Option<PageCodecId>,
        dialect: Arc<dyn Dialect>,
        native_extensions: &crate::native_ext::NativeExtensions,
    ) -> Result<Self> {
        let path = path.into();
        let wal_path = wal_path.into();
        let shared_wal = SharedLog::new(WalFileShared::new_noop());
        let mv_store = ArcSwapOption::empty();

        let db_size = db_file.size()?;

        let shared_page_cache = Arc::new(RwLock::new(PageCache::default()));
        let syms = SymbolTable::new();
        let arena_size = if std::env::var("TESTING").is_ok_and(|v| v.eq_ignore_ascii_case("true")) {
            BufferPool::TEST_ARENA_SIZE
        } else {
            BufferPool::DEFAULT_ARENA_SIZE
        };

        let (encryption_cipher_mode, restore_encryption_key) =
            if let Some(encryption_opts) = encryption_opts {
                let cipher = CipherMode::try_from(encryption_opts.cipher.as_str())?;
                #[cfg(feature = "conn_raw_api")]
                let key = Some(EncryptionKey::from_hex_string(&encryption_opts.hexkey)?);
                #[cfg(not(feature = "conn_raw_api"))]
                let key: Option<()> = None;
                (Some(cipher), key)
            } else {
                (None, None)
            };
        #[cfg(not(feature = "conn_raw_api"))]
        let _ = restore_encryption_key;

        let init_page_1 = if db_size == 0 {
            let default_page_1 = pager::default_page1(encryption_cipher_mode.as_ref());

            Some(default_page_1)
        } else {
            None
        };

        let enable_custom_types = opts.enable_custom_types || dialect.requires_custom_types();

        let learned_costs = {
            use crate::translate::optimizer::learned_costs;
            let costs = learned_costs::LearnedCosts::new();
            // QT.5: the row's cost is weighed in page reads; before the database reads a page
            // of its own, one scratch page read beside it gives the first time. A database opened
            // read-only writes nothing beside it; its open reads its own file. AUDIT §7.1
            // (perf/coldstart): a page read this process already timed on the same IO backend and
            // device stands for this one, so only the first open on a device writes the scratch.
            if let Some((reads, nanos)) = learned_costs::page_read_anchor(
                &path,
                io,
                BufferPool::DEFAULT_PAGE_SIZE,
                costs.page_read_epoch(),
                !flags.contains(OpenFlags::ReadOnly),
            ) {
                costs.anchor_page_read(reads, nanos);
            }
            Arc::new(costs)
        };

        #[cfg(feature = "conn_raw_api")]
        let initial_file_id = (!is_memory_like(&path))
            .then(|| io.file_id(&path).ok())
            .flatten();

        let db = Database {
            mv_store,
            skipping: crate::skipping::SkipRegistry::default(),
            allocators,
            path,
            wal_path,
            schema: Arc::new(Mutex::new(Arc::new({
                let mut s = Schema::with_options(enable_custom_types, dialect.as_ref())?;
                s.generated_columns_enabled = opts.enable_generated_columns;
                s
            }))),
            schema_epoch: AtomicU64::new(0),
            analyze_stats_freshness: Mutex::new(None),
            cost_run_seed: AtomicU64::new(0),
            _shared_page_cache: shared_page_cache,
            learned_costs,
            row_changes: Arc::new(crate::row_changes::RowChanges::new()),
            filter_shares: Arc::new(crate::filter_shares::FilterShares::new()),
            subquery_rows: Arc::new(crate::subquery_rows::SubqueryRows::new()),
            planner: crate::background_search::Planner::default(),
            cost_fit: Arc::new(crate::translate::optimizer::cost_fit::DatabaseFit::new()),
            shared_wal,
            #[cfg(host_shared_wal)]
            shared_wal_coordination: OnceLock::new(),
            db_file,
            builtin_syms: parking_lot::RwLock::new(syms),
            dialect: parking_lot::RwLock::new(dialect),
            io: io.clone(),
            open_flags: flags,
            opts,
            buffer_pool: BufferPool::begin_init(io, arena_size),
            n_connections: AtomicUsize::new(0),
            incarnation: {
                // Deliberately std, not crate::sync: this static outlives a
                // shuttle test execution, and a shuttle-tracked atomic that
                // survives into the next execution corrupts shuttle's vector
                // clocks (task ids restart, the stale clock is longer than
                // the new task table, and clock bookkeeping underflows).
                // A plain std atomic is fine here: the counter only mints
                // process-unique ids and needs no ordering guarantees.
                static NEXT_DATABASE_INCARNATION: std::sync::atomic::AtomicU64 =
                    std::sync::atomic::AtomicU64::new(1);
                NEXT_DATABASE_INCARNATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            },

            pager_state: Arc::new(SharedPagerState::new(init_page_1)),

            encryption_cipher_mode: AtomicCipherMode::new(
                encryption_cipher_mode.unwrap_or(CipherMode::None),
            ),
            page_codec_id,

            durable_storage: None,
            versioned: Mutex::new(None),
            #[cfg(feature = "conn_raw_api")]
            file_id: Mutex::new(initial_file_id),
            registry_key: Mutex::new(None),
            #[cfg(feature = "conn_raw_api")]
            restore_encryption_key: Mutex::new(restore_encryption_key),
            registry_registration: Mutex::new(None),
        };

        db.register_global_builtin_extensions()
            .expect("unable to register global extensions");
        native_extensions.register(&db)?;
        Ok(db)
    }

    /// Deprecated convenience shim: prefer [`Database::open`] with
    /// [`OpenOptions`]. Equivalent to
    /// `Database::open(io, path, OpenOptions::new(dialect))`. Kept for existing
    /// callers; new code should not use it.
    #[cfg(feature = "fs")]
    pub fn open_file(
        io: Arc<dyn IO>,
        path: &str,
        dialect: Arc<dyn Dialect>,
    ) -> Result<Arc<Database>> {
        Self::open(io, path, OpenOptions::new(dialect))
    }

    /// Open or retrieve a shared named in-memory database.
    /// Multiple connections to the same `name` share a single `Database`,
    /// matching SQLite's `file:name?mode=memory&cache=shared` semantics.
    #[cfg(feature = "fs")]
    pub fn open_shared_memory(name: &str, dialect: Arc<dyn Dialect>) -> Result<Arc<Database>> {
        let key = DatabaseKey::SharedMemory(name.to_string());

        {
            let registry = DATABASE_MANAGER.lock();
            if let Some(RegistryEntry::Ready(weak)) = registry.get(&key) {
                if let Some(db) = weak.upgrade() {
                    Self::check_registry_dialect(&db, dialect.as_ref())?;
                    return Ok(db);
                }
            }
        }
        // `:memory:` paths bypass DATABASE_MANAGER internally, so no deadlock.
        let io: Arc<dyn IO> = Arc::new(MemoryIO::new());
        let db = Self::open_file(io, ":memory:", dialect.clone())?;

        let mut registry = DATABASE_MANAGER.lock();
        if let Some(RegistryEntry::Ready(weak)) = registry.get(&key) {
            if let Some(existing) = weak.upgrade() {
                Self::check_registry_dialect(&existing, dialect.as_ref())?;
                return Ok(existing);
            }
        }
        registry.insert(key, RegistryEntry::Ready(Arc::downgrade(&db)));
        Ok(db)
    }

    #[cfg(feature = "fs")]
    #[cfg(host_shared_wal)]
    fn effective_open_flags_for_path(
        io: &Arc<dyn IO>,
        path: &str,
        flags: OpenFlags,
        opts: DatabaseOpts,
    ) -> Result<OpenFlags> {
        if !opts.enable_multiprocess_wal {
            return Ok(flags);
        }

        if is_memory_like(path) {
            return Err(LimboError::InvalidArgument(format!(
                "experimental multiprocess WAL is not supported for in-memory database path '{path}'"
            )));
        }
        if !io.supports_shared_wal_coordination() {
            return Err(LimboError::InvalidArgument(format!(
                "experimental multiprocess WAL is not supported by the active IO backend for '{path}'"
            )));
        }
        if !Self::path_allows_shared_wal_coordination(Path::new(path))? {
            return Err(LimboError::InvalidArgument(format!(
                "experimental multiprocess WAL is not supported on the filesystem backing '{path}'"
            )));
        }

        if !flags.contains(OpenFlags::ReadOnly) {
            return Ok(flags | OpenFlags::NoLock);
        }

        Ok(flags)
    }

    #[cfg(feature = "fs")]
    #[cfg(not(host_shared_wal))]
    fn effective_open_flags_for_path(
        _io: &Arc<dyn IO>,
        _path: &str,
        flags: OpenFlags,
        _opts: DatabaseOpts,
    ) -> Result<OpenFlags> {
        // On unsupported platforms, keep the flag as a no-op so generic
        // cross-platform helpers/tests can request multiprocess WAL without
        // breaking legacy single-process behavior.
        Ok(flags)
    }

    fn validate_external_page_codec_options(
        opts: DatabaseOpts,
        has_external_page_codec: bool,
    ) -> Result<()> {
        if has_external_page_codec && opts.enable_multiprocess_wal {
            return Err(LimboError::InvalidArgument(
                "external page codecs are not supported with experimental multiprocess WAL"
                    .to_string(),
            ));
        }
        Ok(())
    }

    fn validate_open_options(options: &OpenOptions) -> Result<()> {
        Self::validate_external_page_codec_options(options.db_opts, options.page_codec.is_some())?;
        if options.encryption.is_some() && options.page_codec.is_some() {
            return Err(LimboError::InvalidArgument(
                "built-in encryption cannot be combined with an external page codec".to_string(),
            ));
        }
        Ok(())
    }

    #[cfg(feature = "fs")]
    #[cfg(host_shared_wal)]
    pub(crate) fn reject_live_multiprocess_wal_for_legacy_open(
        io: &Arc<dyn IO>,
        path: &str,
        wal_path: Option<&str>,
        opts: DatabaseOpts,
    ) -> Result<()> {
        if opts.enable_multiprocess_wal
            || is_memory_like(path)
            || !io.supports_shared_wal_coordination()
            || !Self::path_allows_shared_wal_coordination(Path::new(path))?
        {
            return Ok(());
        }

        // The coordination file is derived from the WAL path, so probe the
        // configured WAL (not a hard-coded `{path}-wal`) or a custom-WAL open
        // would check the wrong coordination file and miss a live authority.
        let wal_path = wal_path
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{path}-wal"));
        let coordination_path = storage::wal::coordination_path_for_wal_path(&wal_path);
        let Some(authority) =
            MappedSharedWalCoordination::open_existing(io, Path::new(&coordination_path), 64)?
        else {
            return Ok(());
        };

        if matches!(
            authority.open_mode(),
            storage::shared_wal_coordination::SharedWalCoordinationOpenMode::MultiProcess
        ) {
            return Err(LimboError::LockingError(format!(
                "Failed opening database '{path}'. Database is already open with experimental multiprocess WAL in another process"
            )));
        }

        Ok(())
    }

    #[cfg(feature = "fs")]
    #[cfg(not(host_shared_wal))]
    pub(crate) fn reject_live_multiprocess_wal_for_legacy_open(
        _io: &Arc<dyn IO>,
        _path: &str,
        _wal_path: Option<&str>,
        _opts: DatabaseOpts,
    ) -> Result<()> {
        Ok(())
    }

    #[cfg(feature = "fs")]
    #[cfg(host_shared_wal)]
    fn reject_live_legacy_wal_for_multiprocess_open(
        io: &Arc<dyn IO>,
        path: &str,
        flags: OpenFlags,
        opts: DatabaseOpts,
    ) -> Result<()> {
        if !opts.enable_multiprocess_wal || flags.contains(OpenFlags::ReadOnly) {
            return Ok(());
        }

        let probe_flags = (flags | OpenFlags::Create) & !OpenFlags::NoLock & !OpenFlags::ReadOnly;
        match io.open_file(path, probe_flags, true) {
            Ok(_probe_file) => Ok(()),
            Err(LimboError::LockingError(_)) => Err(LimboError::LockingError(format!(
                "Failed opening database '{path}'. Database is already open without experimental multiprocess WAL in another process"
            ))),
            Err(err) => Err(err),
        }
    }

    #[cfg(feature = "fs")]
    #[cfg(not(host_shared_wal))]
    fn reject_live_legacy_wal_for_multiprocess_open(
        _io: &Arc<dyn IO>,
        _path: &str,
        _flags: OpenFlags,
        _opts: DatabaseOpts,
    ) -> Result<()> {
        Ok(())
    }

    /// Check that a registry hit was opened with the dialect the caller
    /// requested. The dialect is fixed at open time and shared by every
    /// user of the registered instance, so a mismatch is an error rather
    /// than a silent share.
    fn check_registry_dialect(db: &Database, requested: &dyn Dialect) -> Result<()> {
        let current = db.dialect();
        let requested_name = requested.name();
        if current.name() != requested_name {
            return Err(LimboError::InvalidArgument(format!(
                "database is already open with dialect '{}'; requested '{}'",
                current.name(),
                requested_name
            )));
        }
        Ok(())
    }

    /// Look up a database in the process-wide registry by file identity.
    /// Returns the cached Database if found, with encryption validation.
    /// This avoids opening a file (and acquiring a file lock) when the
    /// database is already open in this process.
    fn lookup_in_registry(
        io: &dyn IO,
        path: &str,
        encryption_opts: &Option<EncryptionOpts>,
        dialect: &dyn Dialect,
        page_codec: Option<&dyn PageCodec>,
    ) -> Result<Option<Arc<Database>>> {
        if is_memory_like(path) {
            return Ok(None);
        }
        let file_id = match io.file_id(path) {
            Ok(id) => id,
            Err(_) => return Ok(None), // file doesn't exist yet
        };
        let key = DatabaseKey::File(file_id, io.registry_scope());
        let db = {
            let registry = DATABASE_MANAGER.lock();
            match registry.get(&key) {
                Some(RegistryEntry::Ready(weak)) => weak.upgrade(),
                _ => None,
            }
        };
        let Some(db) = db else {
            return Ok(None);
        };
        // The upgraded Arc may be the final owner; validation errors must drop it
        // after releasing the registry lock, because scoped cleanup takes that lock.

        // Validate encryption compatibility (key is not stored for security,
        // so we can only check cipher mode)
        let db_is_encrypted = !matches!(db.encryption_cipher_mode.get(), CipherMode::None);
        if db_is_encrypted && encryption_opts.is_none() {
            return Err(LimboError::InvalidArgument(
                "Database is encrypted but no encryption options provided".to_string(),
            ));
        }
        db.validate_page_codec(page_codec)?;

        Self::check_registry_dialect(&db, dialect)?;

        Ok(Some(db))
    }

    fn validate_page_codec(&self, page_codec: Option<&dyn PageCodec>) -> Result<()> {
        match (self.page_codec_id, page_codec) {
            (Some(_), None) => Err(LimboError::InvalidArgument(
                "Database was opened with an external page codec; reopen with a page codec"
                    .to_string(),
            )),
            (None, Some(_)) => Err(LimboError::InvalidArgument(
                "Database is already open without an external page codec".to_string(),
            )),
            (Some(expected), Some(codec)) if expected != codec.codec_id() => {
                Err(LimboError::InvalidArgument(
                    "page codec identity does not match the existing database".to_string(),
                ))
            }
            _ => Ok(()),
        }
    }

    /// Deprecated convenience shim: prefer [`Database::open`] with
    /// [`OpenOptions`]. Equivalent to `Database::open(io, path,
    /// OpenOptions::new(dialect).flags(flags).db_opts(opts).encryption(enc))`.
    /// Kept for existing callers; new code should not use it.
    #[cfg(feature = "fs")]
    pub fn open_file_with_flags(
        io: Arc<dyn IO>,
        path: &str,
        flags: OpenFlags,
        opts: DatabaseOpts,
        encryption_opts: Option<EncryptionOpts>,
        dialect: Arc<dyn Dialect>,
    ) -> Result<Arc<Database>> {
        Self::open(
            io,
            path,
            OpenOptions::new(dialect)
                .flags(flags)
                .db_opts(opts)
                .encryption(encryption_opts),
        )
    }

    /// Resolve `OpenOptions::storage` for a file-backed open when the caller
    /// did not supply pre-opened storage: optionally consult the registry, run
    /// the legacy/multiprocess WAL probes, open the file, and fill in
    /// `options.storage` and the effective flags in place. Returns `Some(db)`
    /// when a registry hit short-circuits the open (only possible when
    /// `use_registry` is set).
    #[cfg(feature = "fs")]
    fn resolve_default_storage(
        io: &Arc<dyn IO>,
        path: &str,
        options: &mut OpenOptions,
        use_registry: bool,
    ) -> Result<Option<Arc<Database>>> {
        // Check the registry before opening the file to avoid acquiring a file
        // lock that would conflict with an already-open Database in this process.
        if use_registry {
            if let Some(db) = Self::lookup_in_registry(
                io.as_ref(),
                path,
                &options.encryption,
                options.dialect.as_ref(),
                options.page_codec.as_deref(),
            )? {
                if options.durable_storage.is_some() && db.durable_storage.is_none() {
                    return Err(LimboError::InvalidArgument(
                        "database already open without custom durable storage; \
                         close the existing instance before reopening with a custom DurableStorage"
                            .to_string(),
                    ));
                }
                return Ok(Some(db));
            }
        }
        // Mixed legacy/multiprocess opens are incompatible, but the two modes
        // advertise themselves through different lock domains (`.tshm` vs DB
        // file lock). We therefore probe both directions around the actual file
        // open to narrow the TOCTOU window:
        //
        // 1. legacy open rejects an already-live multiprocess authority
        Self::reject_live_multiprocess_wal_for_legacy_open(
            io,
            path,
            options.wal_path.as_deref(),
            options.db_opts,
        )?;
        let effective_flags =
            Self::effective_open_flags_for_path(io, path, options.flags, options.db_opts)?;

        // 2. multiprocess open rejects an already-live legacy DB-file lock
        Self::reject_live_legacy_wal_for_multiprocess_open(
            io,
            path,
            options.flags,
            options.db_opts,
        )?;
        let file = io.open_file(path, effective_flags, true)?;

        // 3. legacy open re-checks after `open_file()` in case a multiprocess
        //    authority appeared between the initial probe and the actual open
        Self::reject_live_multiprocess_wal_for_legacy_open(
            io,
            path,
            options.wal_path.as_deref(),
            options.db_opts,
        )?;
        options.flags = effective_flags;
        options.storage = Some(Arc::new(DatabaseFile::new(file)));
        Ok(None)
    }

    #[cfg(not(feature = "fs"))]
    fn resolve_default_storage(
        _io: &Arc<dyn IO>,
        _path: &str,
        _options: &mut OpenOptions,
        _use_registry: bool,
    ) -> Result<Option<Arc<Database>>> {
        Err(LimboError::InvalidArgument(
            "OpenOptions::storage is required to open a database without the `fs` feature"
                .to_string(),
        ))
    }

    /// The registry-aware entry points reject a custom WAL path: the
    /// process-wide registry keys on the default WAL, so an instance reading a
    /// nonstandard WAL must go through [`Database::do_open`]/
    /// [`Database::do_open_async`] instead.
    fn reject_wal_path_for_registry_open(options: &OpenOptions) -> Result<()> {
        if options.wal_path.is_some() {
            return Err(LimboError::InvalidArgument(
                "OpenOptions::wal_path is only supported by Database::do_open/do_open_async, \
                 which skip the process-wide registry; the registry keys on the default WAL path"
                    .to_string(),
            ));
        }
        Ok(())
    }

    /// Open a database with the given [`OpenOptions`].
    ///
    /// Drives the IO loop internally. When `OpenOptions::storage` is unset,
    /// opens the file at `path` (consulting the process-wide registry first).
    pub fn open(io: Arc<dyn IO>, path: &str, mut options: OpenOptions) -> Result<Arc<Database>> {
        // Reject before resolving default storage: a registry hit there would
        // otherwise return the cached default-WAL instance and silently ignore
        // the custom wal_path before open_async runs its own check.
        Self::reject_wal_path_for_registry_open(&options)?;
        Self::validate_open_options(&options)?;
        if options.storage.is_none() {
            if let Some(db) = Self::resolve_default_storage(&io, path, &mut options, true)? {
                return Ok(db);
            }
        }
        let mut state = OpenDbAsyncState::new();
        loop {
            match Self::open_async(&mut state, io.clone(), path, &options)? {
                IOResult::Done(db) => return Ok(db),
                IOResult::IO(io_completion) => {
                    io_completion.wait(&*io)?;
                }
            }
        }
    }

    /// IOResult-driven twin of [`Database::open`]: the caller drives the IO
    /// loop and passes `state` between calls. `OpenOptions::storage` must be
    /// set.
    ///
    /// This matters for the sync engine, which must yield on IO when the
    /// schema table spans multiple pages (potentially needing network IO to
    /// load them).
    ///
    /// Uses the database registry to ensure a single Database instance per
    /// file within a process; an `Opening` sentinel prevents concurrent opens
    /// of the same path without holding the mutex across I/O yields. Callers
    /// that need a second Database instance for one file (e.g. a copied or
    /// revert WAL) use [`Database::do_open_async`] with `OpenOptions::wal_path`;
    /// passing `wal_path` here is an error, because the registry keys on the
    /// default WAL path.
    pub fn open_async(
        state: &mut OpenDbAsyncState,
        io: Arc<dyn IO>,
        path: &str,
        options: &OpenOptions,
    ) -> IOResultOr<Arc<Database>> {
        Self::reject_wal_path_for_registry_open(options)?;
        Self::validate_open_options(options)?;
        let Some(storage) = options.storage.clone() else {
            return Err(LimboError::InvalidArgument(
                "OpenOptions::storage is required for Database::open_async".to_string(),
            )
            .into());
        };
        // Re-derive lock-mode flags from opts: multiprocess WAL must open the
        // WAL file with NoLock or the second process fails to lock `-wal`.
        // Callers may hand us default flags on every poll, so this runs each
        // time; the rewrite is idempotent. The raw do_open_async path does not
        // do this — it is reserved for the registry-aware entry point.
        #[cfg(feature = "fs")]
        let flags = Self::effective_open_flags_for_path(&io, path, options.flags, options.db_opts)?;
        #[cfg(not(feature = "fs"))]
        let flags = options.flags;

        // turso-sync-engine creates 2 databases with different names in the same IO if MemoryIO is used
        // in this case we need to bypass registry (as this is MemoryIO DB) but also preserve original distinction in names (e.g. :memory:-draft and :memory:-synced)
        // so, we bypass registry for all in memory dbs (i.e. db paths which starts with ":memory:")
        if matches!(state.phase, OpenDbAsyncPhase::Init) && !is_memory_like(path) {
            // Reserve one backend-scoped identity atomically, but never validate or
            // drop a possibly last upgraded Database while holding the registry lock.
            if let Ok(file_id) = io.file_id(path) {
                let key = DatabaseKey::File(file_id, io.registry_scope());
                let cached = {
                    let mut registry = DATABASE_MANAGER.lock();
                    let cached = match registry.get(&key) {
                        Some(RegistryEntry::Ready(weak)) => weak.upgrade(),
                        Some(RegistryEntry::Opening) => {
                            return Ok(IOResult::IO(types::IOCompletions(
                                io::Completion::new_yield(),
                            )));
                        }
                        None => None,
                    };
                    if cached.is_none() {
                        registry.insert(key.clone(), RegistryEntry::Opening);
                    }
                    cached
                };
                if let Some(db) = cached {
                    tracing::debug!("took database {path:?} from the registry");
                    let db_is_encrypted =
                        !matches!(db.encryption_cipher_mode.get(), CipherMode::None);
                    if db_is_encrypted && options.encryption.is_none() {
                        return Err(LimboError::InvalidArgument(
                            "Database is encrypted but no encryption options provided".to_string(),
                        )
                        .into());
                    }
                    db.validate_page_codec(options.page_codec.as_deref())?;
                    Self::check_registry_dialect(&db, options.dialect.as_ref())?;
                    return Ok(IOResult::Done(db));
                }
                state.registry_key = Some(key);
            }
            // Lock is dropped here — the Opening sentinel prevents concurrent opens
            // of the same path without holding the mutex across yields.
        }

        // Open the database (no registry lock held; never re-consults it).
        let result = Self::do_open_async_guarded(
            state,
            io.clone(),
            path,
            None,
            storage,
            flags,
            options.db_opts,
            options.encryption.clone(),
            options.durable_storage.clone(),
            options.page_codec.clone(),
            options.allocators.clone(),
            options.dialect.clone(),
            &options.native_extensions,
        );

        match &result {
            Ok(IOResult::Done(db)) => {
                // Register the opened database and remove the Opening sentinel.
                if let Some(registry_key) = state.registry_key.take() {
                    let mut registry = DATABASE_MANAGER.lock();
                    registry.insert(
                        registry_key.clone(),
                        RegistryEntry::Ready(Arc::downgrade(db)),
                    );
                    *db.registry_key.lock() = Some(registry_key);
                }
            }
            Err(_) => {
                // On error, remove the Opening sentinel so other callers can proceed.
                if let Some(registry_key) = state.registry_key.take() {
                    let mut registry = DATABASE_MANAGER.lock();
                    registry.remove(&registry_key);
                }
            }
            Ok(IOResult::IO(_)) => {}
        }
        result
    }

    /// Synchronous [`Database::do_open_async`] that drives the IO loop.
    ///
    /// Test-only helper for scenarios that intentionally open a second
    /// Database instance for one file (e.g. reading through a copied WAL);
    /// production code uses the registry-aware [`Database::open`].
    #[cfg(all(feature = "fs", feature = "conn_raw_api"))]
    pub fn do_open(io: Arc<dyn IO>, path: &str, mut options: OpenOptions) -> Result<Arc<Database>> {
        Self::validate_open_options(&options)?;
        if options.storage.is_none() {
            // `use_registry = false`: the raw path never consults the registry,
            // so this only opens the file and never returns a cached Database.
            Self::resolve_default_storage(&io, path, &mut options, false)?;
        }
        let mut state = OpenDbAsyncState::new();
        loop {
            match Self::do_open_async(&mut state, io.clone(), path, &options)? {
                IOResult::Done(db) => return Ok(db),
                IOResult::IO(io_completion) => {
                    io_completion.wait(&*io)?;
                }
            }
        }
    }

    /// Raw open that never consults the process-wide registry, driven by the
    /// caller's IO loop. This is the only entry point that honors
    /// `OpenOptions::wal_path`. Prefer [`Database::open_async`] unless you
    /// deliberately need a second Database instance for a file (e.g. the sync
    /// engine's revert WAL).
    pub fn do_open_async(
        state: &mut OpenDbAsyncState,
        io: Arc<dyn IO>,
        path: &str,
        options: &OpenOptions,
    ) -> IOResultOr<Arc<Database>> {
        Self::validate_open_options(options)?;
        let Some(storage) = options.storage.clone() else {
            return Err(LimboError::InvalidArgument(
                "OpenOptions::storage is required for Database::do_open_async".to_string(),
            )
            .into());
        };
        Self::do_open_async_guarded(
            state,
            io,
            path,
            options.wal_path.as_deref(),
            storage,
            options.flags,
            options.db_opts,
            options.encryption.clone(),
            options.durable_storage.clone(),
            options.page_codec.clone(),
            options.allocators.clone(),
            options.dialect.clone(),
            &options.native_extensions,
        )
    }

    /// Run the open state machine and release the schema guard if it fails.
    /// Never touches the registry; both the registry-aware and raw entry
    /// points funnel through here.
    #[allow(clippy::too_many_arguments)]
    fn do_open_async_guarded(
        state: &mut OpenDbAsyncState,
        io: Arc<dyn IO>,
        path: &str,
        wal_path: Option<&str>,
        db_file: Arc<dyn DatabaseStorage>,
        flags: OpenFlags,
        opts: DatabaseOpts,
        encryption_opts: Option<EncryptionOpts>,
        durable_storage: Option<Arc<dyn crate::mvcc::persistent_storage::DurableStorage>>,
        page_codec: Option<Arc<dyn PageCodec>>,
        allocators: DatabaseAllocators,
        dialect: Arc<dyn Dialect>,
        native_extensions: &crate::native_ext::NativeExtensions,
    ) -> IOResultOr<Arc<Database>> {
        Self::validate_external_page_codec_options(opts, page_codec.is_some())?;
        if encryption_opts.is_some() && page_codec.is_some() {
            return Err(LimboError::InvalidArgument(
                "built-in encryption cannot be combined with an external page codec".to_string(),
            )
            .into());
        }
        let result = Self::do_open_async_internal(
            state,
            io,
            path,
            wal_path,
            db_file,
            flags,
            opts,
            encryption_opts,
            durable_storage,
            page_codec,
            allocators,
            dialect,
            native_extensions,
        );
        if result.is_err() {
            let _ = state.schema_guard.take();
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn do_open_async_internal(
        state: &mut OpenDbAsyncState,
        io: Arc<dyn IO>,
        path: &str,
        wal_path: Option<&str>,
        db_file: Arc<dyn DatabaseStorage>,
        flags: OpenFlags,
        opts: DatabaseOpts,
        encryption_opts: Option<EncryptionOpts>,
        durable_storage: Option<Arc<dyn crate::mvcc::persistent_storage::DurableStorage>>,
        page_codec: Option<Arc<dyn PageCodec>>,
        allocators: DatabaseAllocators,
        dialect: Arc<dyn Dialect>,
        native_extensions: &crate::native_ext::NativeExtensions,
    ) -> IOResultOr<Arc<Database>> {
        loop {
            tracing::debug!("do_open_async_internal: state.phase={:?}", state.phase);
            match &state.phase {
                OpenDbAsyncPhase::Init => {
                    // Parse encryption key from encryption_opts if provided
                    let encryption_key = if let Some(ref enc_opts) = encryption_opts {
                        Some(EncryptionKey::from_hex_string(&enc_opts.hexkey)?)
                    } else {
                        None
                    };

                    let wal_path = if let Some(wal_path) = wal_path {
                        wal_path
                    } else {
                        &format!("{path}-wal")
                    };
                    let mut db = Self::new(
                        opts,
                        flags,
                        path,
                        wal_path,
                        &io,
                        db_file.clone(),
                        encryption_opts.clone(),
                        allocators.clone(),
                        page_codec.as_deref().map(PageCodec::codec_id),
                        dialect.clone(),
                        native_extensions,
                    )?;
                    db.durable_storage.clone_from(&durable_storage);

                    // Header validation + WAL recovery runs as a sub state
                    // machine in the ValidatingHeader phase so it can yield
                    // through IO instead of blocking. Stash the owned db and
                    // the parsed key for that phase.
                    state.building_db = Some(db);
                    state.encryption_key = encryption_key;
                    state.header_validation_state = HeaderValidationState::default();
                    state.phase = OpenDbAsyncPhase::ValidatingHeader;
                }

                OpenDbAsyncPhase::ValidatingHeader => {
                    let db = state
                        .building_db
                        .as_mut()
                        .expect("building_db must be set in Init phase");
                    let mut hv_state = std::mem::take(&mut state.header_validation_state);
                    let result = db.header_validation(
                        &mut hv_state,
                        state.encryption_key.as_ref(),
                        page_codec.as_ref(),
                    );
                    state.header_validation_state = hv_state;
                    let pager = return_if_io!(result);

                    let mut db = state
                        .building_db
                        .take()
                        .expect("building_db must be set in Init phase");

                    #[cfg(debug_assertions)]
                    {
                        let wal_enabled = db
                            .shared_wal
                            .current()
                            .read()
                            .metadata
                            .enabled
                            .load(Ordering::SeqCst);
                        let mv_store_enabled = db.get_mv_store().is_some();
                        assert!(
                            db.is_readonly() || wal_enabled || mv_store_enabled,
                            "Either WAL or MVStore must be enabled"
                        );
                    }
                    let _ = &mut db;

                    // Wrap db in Arc before connecting
                    let db = Arc::new(db);
                    if let Some(key @ DatabaseKey::File(_, Some(_))) = &state.registry_key {
                        let registration = ScopedRegistration {
                            key: key.clone(),
                            owner: Arc::downgrade(&db),
                        };
                        let mut guard = db.registry_registration.lock();
                        if guard.is_some() {
                            return Err(LimboError::InternalError(
                                "database registry ownership was already installed".to_string(),
                            )
                            .into());
                        }
                        *guard = Some(registration);
                    }

                    // Check: https://github.com/tursodatabase/turso/pull/1761#discussion_r2154013123
                    let conn = db._connect(
                        false,
                        Some(pager.clone()),
                        state.encryption_key.clone(),
                        page_codec.clone(),
                        None,
                        StatsRefresh::Blocking,
                    )?;

                    // Acquire schema lock and hold it through ReadingHeader and LoadingSchema phases
                    // to ensure schema_version and make_from_btree are atomic
                    let guard = db.schema.lock_arc();

                    state.db = Some(db);
                    state.pager = Some(pager);
                    state.conn = Some(conn);
                    state.schema_guard = Some(guard);

                    state.phase = OpenDbAsyncPhase::ReadingHeader;
                }

                OpenDbAsyncPhase::ReadingHeader => {
                    let pager = state
                        .pager
                        .as_ref()
                        .expect("pager must be initialized in Init phase");
                    let (header_schema_cookie, pg_next_oid) = return_if_io!(pager.with_header(
                        |header| (header.schema_cookie.get(), header.pg_next_oid.get())
                    ));
                    if let Some(db) = state.db.as_ref() {
                        let dialect = db.dialect();
                        if pg_next_oid != 0 && dialect.name() == "sqlite" {
                            return Err(LimboError::Corrupt(format!(
                                "malformed database schema: database was created by the postgres frontend (pg_next_oid={pg_next_oid})"
                            ))
                            .into());
                        }
                        dialect.load_next_oid(pg_next_oid);
                    }
                    let guard = state
                        .schema_guard
                        .as_mut()
                        .expect("schema_guard must be acquired in Init phase");
                    // We logically exclusively own schema via the Opening sentinel in the
                    // registry which prevents concurrent opens of the same path.
                    // At this point we already created a connection which cloned the schema
                    // internally, so we can't use get_mut here.
                    //
                    // it's not ideal but correctness is OK - before prepare connection call maybe_update_schema and in case of divergence update schema ref from the db + we always check connection cookie in the VDBE program itself
                    let schema = Schema::try_make_mut(guard)?;
                    schema.schema_version = header_schema_cookie;

                    state.phase = OpenDbAsyncPhase::LoadingSchema;
                }

                OpenDbAsyncPhase::LoadingSchema => {
                    let pager = state
                        .pager
                        .as_ref()
                        .expect("pager must be initialized in Init phase");
                    let conn = state
                        .conn
                        .as_ref()
                        .expect("conn must be initialized in Init phase");
                    let syms = conn.syms.read();

                    let guard = state
                        .schema_guard
                        .as_mut()
                        .expect("schema_guard must be acquired in Init phase");
                    // while we logically exclusively own schema as we hold DATABASE_MANAGER lock in the top level `open_async` function
                    // at the moment we already created connection which cloned the schema internally
                    // so, we can't use get_mut here for now
                    //
                    // it's not ideal but correctness is OK - before prepare connection call maybe_update_schema and in case of divergence update schema ref from the db + we always check connection cookie in the VDBE program itself
                    let schema = Schema::try_make_mut(guard)?;

                    let dialect = conn.dialect();
                    let result = schema.make_from_btree(
                        &mut state.make_from_btree_state,
                        None,
                        pager,
                        &syms,
                        dialect.as_ref(),
                    );

                    match result {
                        Ok(IOResult::IO(io)) => return Ok(IOResult::IO(io)),
                        Ok(IOResult::Done(())) => {
                            if let Some(db) = state.db.as_ref() {
                                db.bump_schema_epoch();
                            }
                            // Release the schema lock
                            state.schema_guard = None;
                        }
                        Err(e) => return Err(e),
                    }

                    // Amphora (G7 step 2, `AC-NS.2.1`): with the B-trees on the versioned
                    // storage, `sqlite_schema` is its tree 1, not the pager's page 1, which
                    // holds none of them: the schema is read from there.
                    let conn = state.conn.as_ref().ok_or_else(|| {
                        LimboError::InternalError("open: no connection past Init".into())
                    })?;
                    if conn.versioned.is_some() {
                        conn.load_versioned_schema()?;
                    }

                    // Load custom types from __turso_internal_types if the table
                    // exists and custom types are enabled. The schema loaded by
                    // make_from_btree includes the table definition but not its
                    // contents. We need to read the stored type definitions so
                    // that DECODE/ENCODE and affinity metadata are available to
                    // all subsequent connections.
                    let conn = state
                        .conn
                        .as_ref()
                        .expect("conn must be initialized in Init phase");
                    if conn.experimental_custom_types_enabled() {
                        // Sync the connection's schema from the database so it
                        // can query __turso_internal_types.
                        conn.maybe_update_schema();
                        let type_sqls = conn.query_stored_type_definitions()?;
                        if !type_sqls.is_empty() {
                            let db = state
                                .db
                                .as_ref()
                                .expect("db must be initialized in Init phase");
                            db.with_schema_mut(|schema| schema.load_type_definitions(&type_sqls))?;
                        }
                    }

                    // Amphora: the online schema change states, which `make_from_btree` doesn't
                    // read (CP-T1.12): without them an index a change left delete-only or
                    // write-only would open public, read before its backfill gave every row its
                    // entry.
                    conn.maybe_update_schema();
                    let states = conn.query_index_states()?;
                    if !states.is_empty() {
                        let db = state.db.as_ref().ok_or_else(|| {
                            LimboError::InternalError("open: no database past Init".into())
                        })?;
                        db.with_schema_mut(|schema| schema.set_index_states(states))?;
                    }
                    let columns = conn.query_constraint_states()?;
                    if !columns.is_empty() {
                        let db = state.db.as_ref().ok_or_else(|| {
                            LimboError::InternalError("open: no database past Init".into())
                        })?;
                        db.with_schema_mut(|schema| schema.set_constraint_states(&columns))?;
                    }
                    let dropped = conn.query_dropped_columns()?;
                    if !dropped.is_empty() {
                        let db = state.db.as_ref().ok_or_else(|| {
                            LimboError::InternalError("open: no database past Init".into())
                        })?;
                        db.with_schema_mut(|schema| schema.set_dropped_columns(&dropped))?;
                    }

                    state.phase = OpenDbAsyncPhase::BootstrapMvStore;
                }

                OpenDbAsyncPhase::BootstrapMvStore => {
                    let db = state
                        .db
                        .as_ref()
                        .expect("db must be initialized in Init phase");
                    let pager = state
                        .pager
                        .as_ref()
                        .expect("pager must be initialized in Init phase");

                    if let Some(mv_store) = db.get_mv_store().as_ref() {
                        // Create the dedicated bootstrap connection once and
                        // hold it across yields. Re-entry reuses the existing
                        // connection and the persisted `BootstrapState`.
                        if state.mvcc_bootstrap_conn.is_none() {
                            state.mvcc_bootstrap_conn = Some(db._connect(
                                true,
                                Some(pager.clone()),
                                state.encryption_key.clone(),
                                page_codec.clone(),
                                None,
                                StatsRefresh::Blocking,
                            )?);
                        }
                        let conn = state.mvcc_bootstrap_conn.as_ref().expect("created above");
                        return_if_io!(
                            mv_store.bootstrap_nonblock(conn, &mut state.mvcc_bootstrap_state)
                        );
                        // Done — drop the bootstrap connection.
                        state.mvcc_bootstrap_conn = None;
                    }

                    state.phase = OpenDbAsyncPhase::Done;
                    return Ok(IOResult::Done(
                        state
                            .db
                            .take()
                            .expect("db must be initialized in Init phase"),
                    ));
                }

                OpenDbAsyncPhase::Done => {
                    panic!("do_open_async_internal called after completion");
                }
            }
        }
    }

    /// Necessary Pager initialization, so that we are prepared to read from Page 1.
    /// For encrypted databases, the encryption key must be provided to properly decrypt page 1.
    /// Blocking shim over [`Database::_init_nonblock`], retained for the
    /// synchronous callers (connection setup paths). The open state machine
    /// uses `_init_nonblock` directly so a fresh open never blocks here.
    pub(crate) fn _init(
        &self,
        encryption_key: Option<&EncryptionKey>,
        page_codec: Option<Arc<dyn PageCodec>>,
    ) -> Result<Pager> {
        let mut st = InitState::default();
        self.io
            .block(|| self._init_nonblock(&mut st, encryption_key, page_codec.as_ref()))
    }

    /// Necessary Pager initialization, so that we are prepared to read from
    /// Page 1. For encrypted databases, the encryption key must be provided to
    /// properly decrypt page 1. Non-blocking: drives `init_pager` (DB-header
    /// read) and the page-1 autovacuum read through their IO.
    pub(crate) fn _init_nonblock(
        &self,
        st: &mut InitState,
        encryption_key: Option<&EncryptionKey>,
        page_codec: Option<&Arc<dyn PageCodec>>,
    ) -> IOResultOr<Pager> {
        if encryption_key.is_some() && page_codec.is_some() {
            return Err(LimboError::InvalidArgument(
                "built-in encryption cannot be combined with an external page codec".to_string(),
            )
            .into());
        }
        loop {
            match st {
                InitState::Start => {
                    *st = InitState::InitPager(DbHeaderReadState::default());
                }
                InitState::InitPager(hdr_st) => {
                    let pager =
                        return_if_io!(self.init_pager(None, hdr_st, encryption_key, page_codec));
                    pager.enable_encryption(self.opts.enable_encryption);

                    // Set up encryption context BEFORE reading the header page.
                    // For encrypted databases, page 1 has:
                    // - Bytes 0-15: Turso magic header (replaces SQLite magic)
                    // - Bytes 16-100: Unencrypted header metadata
                    // - Bytes 100+: Encrypted content
                    // The encryption context is needed to properly decrypt page 1 when reopening.
                    if let Some(key) = encryption_key {
                        let cipher_mode = self.encryption_cipher_mode.get();
                        pager.set_encryption_context(cipher_mode, key)?;
                    } else if let Some(codec) = page_codec {
                        pager.set_page_codec(codec.clone())?;
                    }

                    // Start a read transaction before reading page 1 to prevent a concurrent
                    // checkpoint from truncating the WAL underneath bootstrap. Under heavy
                    // same-process connection churn, the shared WAL bootstrap path can
                    // briefly contend on short-lived in-process locks, so treat Busy here as
                    // a transient and retry rather than failing `connect()`.
                    let mut read_tx_attempts = 0u32;
                    loop {
                        match pager.begin_read_tx() {
                            Ok(()) => break,
                            Err(LimboError::Busy) => {
                                read_tx_attempts += 1;
                                if read_tx_attempts > 1 {
                                    return Err(LimboError::Busy.into());
                                }
                                pager.io.yield_now();
                            }
                            Err(err) => return Err(err.into()),
                        }
                    }

                    *st = InitState::ReadPage1 {
                        pager: Box::new(pager),
                    };
                }
                InitState::ReadPage1 { pager } => {
                    // Read page 1 within the read transaction to determine the
                    // autovacuum mode. The read tx stays open across an IO
                    // yield here (re-entry resumes the read); we only end it
                    // once the read completes or errors.
                    let mode = match HeaderRef::from_pager(pager) {
                        Ok(IOResult::Done(header_ref)) => {
                            let header = header_ref.borrow();
                            let validate_codec_header = || -> Result<()> {
                                if self.initialized() {
                                    let page_transform =
                                        pager.io_ctx.read().page_transform().clone();
                                    if let PageTransform::Codec(codec) = page_transform {
                                        let bootstrap_page_size =
                                            pager.get_page_size_unchecked().get() as usize;
                                        let bootstrap_reserved_space =
                                            pager.get_reserved_space().ok_or_else(|| {
                                                LimboError::InternalError(
                                                    "page codec reserved space was not initialized"
                                                        .to_string(),
                                                )
                                            })?;
                                        let decoded_page_size = header.page_size.get() as usize;
                                        if decoded_page_size != bootstrap_page_size {
                                            return Err(LimboError::InvalidArgument(format!(
                                            "page codec bootstrap page size {bootstrap_page_size} does not match decoded page-1 size {decoded_page_size}"
                                        )));
                                        }
                                        if header.reserved_space != bootstrap_reserved_space {
                                            return Err(LimboError::InvalidArgument(format!(
                                            "page codec bootstrap reserved space {bootstrap_reserved_space} does not match decoded page-1 reserved space {}",
                                            header.reserved_space
                                        )));
                                        }
                                        let required_reserved_space =
                                            codec.required_reserved_bytes();
                                        if header.reserved_space != required_reserved_space {
                                            return Err(LimboError::InvalidArgument(format!(
                                            "page codec requires exactly {required_reserved_space} reserved bytes, but decoded page 1 provides {}",
                                            header.reserved_space
                                        )));
                                        }
                                    }
                                }
                                Ok(())
                            };
                            if let Err(err) = validate_codec_header() {
                                pager.end_read_tx();
                                return Err(err.into());
                            }
                            if header.vacuum_mode_largest_root_page.get() > 0 {
                                if header.incremental_vacuum_enabled.get() > 0 {
                                    AutoVacuumMode::Incremental
                                } else {
                                    AutoVacuumMode::Full
                                }
                            } else {
                                AutoVacuumMode::None
                            }
                        }
                        Ok(IOResult::IO(io)) => return Ok(IOResult::IO(io)),
                        Err(err) => {
                            pager.end_read_tx();
                            return Err(err);
                        }
                    };

                    pager.end_read_tx();
                    pager.set_auto_vacuum_mode(mode);

                    let InitState::ReadPage1 { pager } = std::mem::take(st) else {
                        unreachable!("state is ReadPage1");
                    };
                    return Ok(IOResult::Done(*pager));
                }
            }
        }
    }

    /// Checks the Version numbers in the DatabaseHeader, and changes it according to the required options
    ///
    /// Will also open MVStore and WAL if needed.
    ///
    /// Driven as a sub state machine (see [`HeaderValidationState`]) from the
    /// `ValidatingHeader` open phase so that WAL recovery on open yields
    /// through its IO instead of blocking — this is what lets a fresh open
    /// make progress on runtimes (e.g. WASM) that cannot pump `io.step`
    /// synchronously.
    fn header_validation(
        &mut self,
        st: &mut HeaderValidationState,
        encryption_key: Option<&EncryptionKey>,
        page_codec: Option<&Arc<dyn PageCodec>>,
    ) -> IOResultOr<Arc<Pager>> {
        loop {
            match st {
                HeaderValidationState::Start { init } => {
                    // `_init` does not modify `open_flags` (the autovacuum
                    // override happens later in `Validate`), so capturing
                    // `is_readonly` across the `_init` yields is stable.
                    let pager =
                        return_if_io!(self._init_nonblock(init, encryption_key, page_codec));
                    let log_exists =
                        journal_mode::logical_log_exists(std::path::Path::new(&self.path));
                    let is_readonly = self.open_flags.contains(OpenFlags::ReadOnly);
                    turso_assert!(pager.wal.is_none(), "Pager should have no WAL yet");
                    *st = HeaderValidationState::Validate {
                        pager: Box::new(pager),
                        is_readonly,
                        log_exists,
                    };
                }
                HeaderValidationState::Validate {
                    pager,
                    is_readonly,
                    log_exists,
                } => {
                    let is_readonly = *is_readonly;
                    let log_exists = *log_exists;

                    // Re-entrant reads: both `with_header` and `from_pager`
                    // resume via their own state machines, and the autovacuum
                    // flag update is idempotent.
                    let is_autovacuumed_db = return_if_io!(pager.with_header(|header| {
                        header.vacuum_mode_largest_root_page.get() > 0
                            || header.incremental_vacuum_enabled.get() > 0
                    }));
                    if is_autovacuumed_db && !self.opts.enable_autovacuum {
                        tracing::warn!(
                            "Database has autovacuum enabled but --experimental-autovacuum flag is not set. Opening in readonly mode."
                        );
                        self.open_flags |= OpenFlags::ReadOnly;
                    }

                    let header: HeaderRefMut = return_if_io!(HeaderRefMut::from_pager(pager));
                    let header_mut = header.borrow_mut();

                    if !header_mut.text_encoding.is_utf8() {
                        return Err(LimboError::UnsupportedEncoding(
                            header_mut.text_encoding.to_string(),
                        )
                        .into());
                    }

                    let (read_version, write_version) =
                        { (header_mut.read_version, header_mut.write_version) };

                    if encryption_key.is_none() && header_mut.magic != SQLITE_HEADER {
                        tracing::error!(
                            "invalid value of database header magic bytes: {:?}",
                            header_mut.magic
                        );
                        return Err(LimboError::NotADB.into());
                    }
                    // when we open fresh db with encryption params - header will be SQLite at this point
                    if encryption_key.is_some()
                        && (header_mut.magic != SQLITE_HEADER
                            && !header_mut.magic.starts_with(TURSO_HEADER_PREFIX))
                    {
                        tracing::error!(
                            "invalid value of database header magic bytes: {:?}",
                            header_mut.magic
                        );
                        return Err(LimboError::NotADB.into());
                    }

                    // TODO: right now we don't support READ ONLY and no READ or WRITE in the Version header
                    // https://www.sqlite.org/fileformat.html#file_format_version_numbers
                    if read_version != write_version {
                        return Err(LimboError::Corrupt(format!(
                            "Read version `{read_version:?}` is not equal to Write version `{write_version:?} in database header`"
                        )).into());
                    }

                    let (read_version, _write_version) = (
                        read_version.to_version().map_err(|val| {
                            LimboError::Corrupt(format!("Invalid read_version: {val}"))
                        })?,
                        write_version.to_version().map_err(|val| {
                            LimboError::Corrupt(format!("Invalid write_version: {val}"))
                        })?,
                    );

                    // Validate fixed header fields per SQLite spec
                    if header_mut.max_embed_frac != 64 {
                        return Err(LimboError::Corrupt(format!(
                            "Invalid max_embed_frac: expected 64, got {}",
                            header_mut.max_embed_frac
                        ))
                        .into());
                    }
                    if header_mut.min_embed_frac != 32 {
                        return Err(LimboError::Corrupt(format!(
                            "Invalid min_embed_frac: expected 32, got {}",
                            header_mut.min_embed_frac
                        ))
                        .into());
                    }
                    if header_mut.leaf_frac != 32 {
                        return Err(LimboError::Corrupt(format!(
                            "Invalid leaf_frac: expected 32, got {}",
                            header_mut.leaf_frac
                        ))
                        .into());
                    }
                    let schema_format = header_mut.schema_format.get();
                    // If the database is completely empty, if it has no schema, then the schema format number can be zero.
                    if !(0..=4).contains(&schema_format) {
                        return Err(LimboError::Corrupt(format!(
                            "Invalid schema_format: expected 1-4, got {schema_format}"
                        ))
                        .into());
                    }
                    if !matches!(
                        header_mut.text_encoding,
                        TextEncoding::Unset
                            | TextEncoding::Utf8
                            | TextEncoding::Utf16Le
                            | TextEncoding::Utf16Be
                    ) {
                        return Err(LimboError::Corrupt(format!(
                            "Invalid text_encoding: {}",
                            header_mut.text_encoding
                        ))
                        .into());
                    }
                    if !matches!(
                        header_mut.text_encoding,
                        TextEncoding::Unset | TextEncoding::Utf8
                    ) {
                        return Err(LimboError::Corrupt(format!(
                            "Only utf8 text_encoding is supported by tursodb: got={}",
                            header_mut.text_encoding
                        ))
                        .into());
                    }

                    // Determine if we should open in MVCC mode based on the database header version
                    // MVCC is controlled only by the database header (set via PRAGMA journal_mode)
                    let open_mv_store = matches!(read_version, Version::Mvcc);
                    if open_mv_store && page_codec.is_some() {
                        return Err(LimboError::InvalidArgument(
                            "external page codecs are not supported with MVCC databases"
                                .to_string(),
                        )
                        .into());
                    }

                    // MVCC has no cross-process coordination: commit
                    // serialization, the logical-log append offset, and
                    // checkpoint exclusion are all process-local, so
                    // concurrent multiprocess access silently loses committed
                    // transactions and corrupts live views.
                    if open_mv_store && self.opts.enable_multiprocess_wal {
                        return Err(LimboError::InvalidArgument(format!(
                            "cannot open MVCC database '{}' with experimental multiprocess WAL: MVCC does not support multiprocess access",
                            self.path
                        )).into());
                    }

                    // Now check the Header Version to see which mode the DB file really is on
                    // Track if header was modified so we can write it to disk
                    let header_modified = match read_version {
                        Version::Legacy => {
                            if is_readonly {
                                tracing::warn!(
                                    "Database {} is opened in readonly mode, cannot convert Legacy mode to WAL. Running in Legacy mode.",
                                    self.path
                                );
                                false
                            } else {
                                // Convert Legacy to WAL mode
                                header_mut.read_version = RawVersion::from(Version::Wal);
                                header_mut.write_version = RawVersion::from(Version::Wal);
                                true
                            }
                        }
                        Version::Wal => false,
                        Version::Mvcc => false,
                    };

                    // In WAL mode, a logical log is always unexpected.
                    // In MVCC mode, WAL and logical-log coexistence can happen across interrupted checkpoint
                    // recovery and is reconciled in MvStore::bootstrap().
                    if !open_mv_store && log_exists {
                        return Err(LimboError::Corrupt(format!(
                            "MVCC logical log file exists for database {}, but database header indicates WAL mode. The database may be corrupted.",
                            self.path
                        )).into());
                    }

                    // Amphora: a header rewritten here goes straight to the file, so it is written
                    // with its checksum (checked again first if opening deferred it).
                    if header_modified {
                        pager.resume_checksum();
                    }
                    let page = header.page().clone();
                    // `header` (a cheap Arc<Page> wrapper, no lock) is dropped
                    // here; the page ref carries the (possibly modified) header
                    // buffer forward.
                    drop(header);

                    // Move the owned pager out of the state to build the next.
                    let HeaderValidationState::Validate { pager, .. } = std::mem::take(st) else {
                        unreachable!("state is Validate");
                    };
                    *st = if header_modified {
                        HeaderValidationState::WriteHeader {
                            pager,
                            page,
                            open_mv_store,
                            completion: None,
                        }
                    } else {
                        HeaderValidationState::OpenWal {
                            pager,
                            open_mv_store,
                            driver: None,
                            discarded_orphan_wal: false,
                        }
                    };
                }
                HeaderValidationState::WriteHeader {
                    pager,
                    page,
                    open_mv_store,
                    completion,
                } => {
                    // If header was modified, write it directly to disk before we attach the
                    // WAL / clear the cache (must hit the DB file, not the WAL).
                    let c = match completion.take() {
                        Some(c) => c,
                        None => storage::sqlite3_ondisk::begin_write_btree_page(pager, page, None)?,
                    };
                    if !c.succeeded() {
                        *completion = Some(c.clone());
                        io_yield_one!(c);
                    }
                    let open_mv_store = *open_mv_store;
                    let HeaderValidationState::WriteHeader { pager, .. } = std::mem::take(st)
                    else {
                        unreachable!("state is WriteHeader");
                    };
                    *st = HeaderValidationState::OpenWal {
                        pager,
                        open_mv_store,
                        driver: None,
                        discarded_orphan_wal: false,
                    };
                }
                HeaderValidationState::OpenWal {
                    open_mv_store,
                    driver,
                    discarded_orphan_wal,
                    ..
                } => {
                    // Always open shared WAL and set it in the Database and Pager.
                    // MVCC currently requires a WAL open to function.
                    let mut shared_wal = {
                        #[cfg(not(host_shared_wal))]
                        {
                            if driver.is_none() {
                                *driver = Some(WalFileShared::open_shared_if_exists_begin(
                                    &self.io,
                                    &self.wal_path,
                                    self.open_flags,
                                )?);
                            }
                            return_if_io!(driver.as_mut().expect("driver initialized above").poll())
                        }
                        #[cfg(host_shared_wal)]
                        {
                            // Native-only coordination path: `io.step` pumps
                            // synchronously here, so the blocking shims are
                            // fine. (Driver field is unused on host.)
                            let _ = &driver;
                            let flags = self.open_flags;
                            let shared_authority = self.open_shared_wal_coordination_for_open()?;
                            if let Some(authority) = shared_authority.as_ref() {
                                if !authority.frame_index_overflowed() {
                                    WalFileShared::open_shared_from_authority_if_exists(
                                        &self.io,
                                        &self.wal_path,
                                        flags,
                                        authority,
                                        &self.db_file,
                                    )?
                                } else {
                                    WalFileShared::open_shared_if_exists(
                                        &self.io,
                                        &self.wal_path,
                                        flags,
                                    )?
                                }
                            } else {
                                WalFileShared::open_shared_if_exists(
                                    &self.io,
                                    &self.wal_path,
                                    flags,
                                )?
                            }
                        }
                    };

                    // A WAL never belongs to a database file with zero pages:
                    // `Pager::allocate_page1` fsyncs page 1 to the main file
                    // before any commit can fsync WAL frames, so no crash of
                    // ours leaves frames next to an empty database file. When
                    // that pair shows up anyway, the database file was deleted
                    // (a common way to reset a database is to remove the `.db`
                    // and forget the `-wal`) or the image predates the page-1
                    // fsync. Either way the frames describe a database that no
                    // longer exists: replaying them would splice a dead
                    // database's pages into this one as soon as it grows past
                    // zero pages.
                    //
                    // Throw the orphan WAL away and open the database empty,
                    // which is what SQLite does — `pagerOpenWalIfPresent()`
                    // deletes a WAL it finds next to a zero-page database —
                    // and what upstream tests expect.
                    if shared_wal.read().last_checksum_and_max_frame().1 > 0
                        && self.db_file.size()? == 0
                    {
                        turso_assert!(
                            !*discarded_orphan_wal,
                            "orphan WAL reopened with frames after being discarded"
                        );
                        *discarded_orphan_wal = true;
                        tracing::warn!(
                            "discarding WAL '{}': it holds frames but database file '{}' has no pages, so the WAL does not belong to it",
                            self.wal_path,
                            self.path
                        );
                        if self.open_flags.contains(OpenFlags::ReadOnly) {
                            // A read-only open must not delete files, so just
                            // detach the WAL. This is the same state a
                            // read-only open reaches when there is no WAL file
                            // at all.
                            shared_wal = WalFileShared::new_noop();
                        } else {
                            drop(shared_wal);
                            *driver = None;
                            self.io.remove_file(&self.wal_path)?;
                            // Reopen the (now absent) WAL: `open_file` recreates
                            // it empty and recovery of a WAL shorter than its
                            // header finishes without any IO.
                            continue;
                        }
                    }

                    // Decision 45: the log's second segment, `-wal2`, recovered beside the first.
                    #[cfg(host_shared_wal)]
                    let second_segment = WalFileShared::open_second_segment_if_exists(
                        &self.io,
                        &storage::wal::second_segment_path(&self.wal_path),
                        self.open_flags,
                    )?;
                    #[cfg(not(host_shared_wal))]
                    let second_segment = WalFileShared::new_noop();
                    let shared_log = SharedLog::recover(
                        shared_wal,
                        second_segment,
                        Some(storage::wal::second_segment_path(&self.wal_path)),
                    )?;

                    let open_mv_store = *open_mv_store;
                    let HeaderValidationState::OpenWal { mut pager, .. } = std::mem::take(st)
                    else {
                        unreachable!("state is OpenWal");
                    };

                    self.shared_wal = shared_log;
                    let last_checksum_and_max_frame = self
                        .shared_wal
                        .current()
                        .read()
                        .last_checksum_and_max_frame();
                    let wal =
                        self.build_wal(last_checksum_and_max_frame, pager.buffer_pool.clone())?;
                    pager.set_wal(wal);

                    // Clear page cache after attaching WAL since pages may have been cached
                    // from disk reads before WAL was attached. The WAL may contain newer
                    // versions of these pages (e.g., page 1 with updated schema_cookie).
                    pager.clear_page_cache(true);
                    pager.set_schema_cookie(None);

                    if open_mv_store {
                        let canonical_path = self.get_database_canonical_path();
                        let enc_ctx = pager.io_ctx.read().encryption_context().cloned();
                        let mv_store = journal_mode::open_mv_store(
                            self.io.clone(),
                            &canonical_path,
                            self.open_flags,
                            self.durable_storage.clone(),
                            enc_ctx,
                            self.allocators.mv_store.clone(),
                            self.experimental_mvcc_passive_checkpoint_enabled(),
                        )?;
                        self.mv_store.store(Some(mv_store));
                    }

                    return Ok(IOResult::Done(Arc::new(*pager)));
                }
            }
        }
    }

    pub fn get_database_canonical_path(&self) -> String {
        if self.is_in_memory_db() {
            // For in-memory databases, SQLite shows empty string
            String::new()
        } else {
            // For file databases, try show the full absolute path if that doesn't fail
            match std::fs::canonicalize(&self.path) {
                Ok(abs_path) => abs_path.to_string_lossy().to_string(),
                Err(_) => self.path.to_string(),
            }
        }
    }

    #[cfg(feature = "conn_raw_api")]
    /// Rebuild the process-local shared WAL view after a caller restores the
    /// database and WAL files outside the pager.
    ///
    /// The caller must keep writers, flushers and checkpoints quiescent while replacing the files
    /// and rebuilding this view. Invalidating old durability proofs does not make concurrent
    /// external file replacement atomic with running operations.
    pub fn reload_wal_after_external_restore(self: &Arc<Self>) -> Result<()> {
        self.reload_wal_after_external_restore_with_encryption(None, None)
    }

    #[cfg(feature = "conn_raw_api")]
    /// Constructs an unregistered staging `Database` mirroring `self`'s options and IO backend so
    /// external-restore validation, catalog reparsing, and MVCC logical-log bootstrap run in total
    /// isolation before any live state on `self` is mutated.
    fn build_staging_for_restore(
        &self,
        db_file: Arc<dyn DatabaseStorage>,
        buffer_pool: Arc<BufferPool>,
        shared_wal: Arc<SharedLog>,
        cipher: CipherMode,
        encryption_key: Option<EncryptionKey>,
    ) -> Result<Arc<Self>> {
        let dialect = self.dialect();
        let enable_custom_types = self.opts.enable_custom_types || dialect.requires_custom_types();
        let mut empty_schema = Schema::with_options(enable_custom_types, dialect.as_ref())?;
        empty_schema.generated_columns_enabled = self.opts.enable_generated_columns;
        let db = Self {
            mv_store: ArcSwapOption::empty(),
            skipping: crate::skipping::SkipRegistry::default(),
            allocators: self.allocators.clone(),
            path: self.path.clone(),
            wal_path: self.wal_path.clone(),
            schema: Arc::new(Mutex::new(Arc::new(empty_schema))),
            schema_epoch: AtomicU64::new(0),
            analyze_stats_freshness: Mutex::new(None),
            cost_run_seed: AtomicU64::new(0),
            _shared_page_cache: Arc::new(RwLock::new(PageCache::default())),
            learned_costs: self.learned_costs.clone(),
            row_changes: Arc::new(crate::row_changes::RowChanges::new()),
            filter_shares: Arc::new(crate::filter_shares::FilterShares::new()),
            subquery_rows: Arc::new(crate::subquery_rows::SubqueryRows::new()),
            planner: crate::background_search::Planner::default(),
            cost_fit: Arc::new(crate::translate::optimizer::cost_fit::DatabaseFit::new()),
            shared_wal,
            #[cfg(host_shared_wal)]
            shared_wal_coordination: OnceLock::new(),
            db_file,
            builtin_syms: parking_lot::RwLock::new(SymbolTable::new()),
            dialect: parking_lot::RwLock::new(dialect),
            io: self.io.clone(),
            open_flags: self.open_flags,
            opts: self.opts,
            buffer_pool,
            n_connections: AtomicUsize::new(0),
            incarnation: self.incarnation,
            pager_state: Arc::new(SharedPagerState::new(None)),
            encryption_cipher_mode: AtomicCipherMode::new(cipher),
            page_codec_id: self.page_codec_id,
            durable_storage: self.durable_storage.clone(),
            versioned: Mutex::new(None),
            file_id: Mutex::new(None),
            registry_key: Mutex::new(None),
            restore_encryption_key: Mutex::new(encryption_key),
            registry_registration: Mutex::new(None),
        };
        db.register_global_builtin_extensions()
            .map_err(LimboError::InternalError)?;
        Ok(Arc::new(db))
    }

    #[cfg(feature = "conn_raw_api")]
    /// Validates page-1 header metadata of a restored populated image on `staging_pager`, matching
    /// cold [`Self::header_validation`], and returns `(open_mv_store, pg_next_oid)`.
    fn validate_restored_header(
        &self,
        staging_pager: &Arc<Pager>,
        encryption_key: Option<&EncryptionKey>,
        log_exists: bool,
    ) -> Result<(bool, u32)> {
        staging_pager.begin_read_tx()?;
        let header_res = self.io.block(|| HeaderRef::from_pager(staging_pager));
        let validated = (|| -> Result<(bool, u32)> {
            let header = header_res?;
            let header_ref = header.borrow();
            if !header_ref.text_encoding.is_utf8() {
                return Err(LimboError::UnsupportedEncoding(
                    header_ref.text_encoding.to_string(),
                ));
            }
            if encryption_key.is_none() && header_ref.magic != SQLITE_HEADER {
                return Err(LimboError::NotADB);
            }
            if encryption_key.is_some()
                && header_ref.magic != SQLITE_HEADER
                && !header_ref.magic.starts_with(TURSO_HEADER_PREFIX)
            {
                return Err(LimboError::NotADB);
            }
            let (read_version, write_version) = (header_ref.read_version, header_ref.write_version);
            if read_version != write_version {
                return Err(LimboError::Corrupt(format!(
                    "Read version `{read_version:?}` is not equal to Write version `{write_version:?}` in restored database header"
                )));
            }
            let read_version = read_version
                .to_version()
                .map_err(|val| LimboError::Corrupt(format!("Invalid read_version: {val}")))?;
            let _ = write_version
                .to_version()
                .map_err(|val| LimboError::Corrupt(format!("Invalid write_version: {val}")))?;
            if header_ref.max_embed_frac != 64 {
                return Err(LimboError::Corrupt(format!(
                    "Invalid max_embed_frac: expected 64, got {}",
                    header_ref.max_embed_frac
                )));
            }
            if header_ref.min_embed_frac != 32 {
                return Err(LimboError::Corrupt(format!(
                    "Invalid min_embed_frac: expected 32, got {}",
                    header_ref.min_embed_frac
                )));
            }
            if header_ref.leaf_frac != 32 {
                return Err(LimboError::Corrupt(format!(
                    "Invalid leaf_frac: expected 32, got {}",
                    header_ref.leaf_frac
                )));
            }
            let schema_format = header_ref.schema_format.get();
            if !(0..=4).contains(&schema_format) {
                return Err(LimboError::Corrupt(format!(
                    "Invalid schema_format: expected 1-4, got {schema_format}"
                )));
            }
            if !matches!(
                header_ref.text_encoding,
                TextEncoding::Unset | TextEncoding::Utf8
            ) {
                return Err(LimboError::Corrupt(format!(
                    "Only utf8 text_encoding is supported by tursodb: got={}",
                    header_ref.text_encoding
                )));
            }
            let open_mv_store = matches!(read_version, Version::Mvcc);
            if open_mv_store && self.opts.enable_multiprocess_wal {
                return Err(LimboError::InvalidArgument(format!(
                    "cannot restore MVCC database '{}' with experimental multiprocess WAL: MVCC does not support multiprocess access",
                    self.path
                )));
            }
            if !open_mv_store && log_exists {
                return Err(LimboError::Corrupt(format!(
                    "MVCC logical log file exists for database {}, but database header indicates WAL mode. The database may be corrupted.",
                    self.path
                )));
            }
            let pg_next_oid = header_ref.pg_next_oid.get();
            if pg_next_oid != 0 && self.dialect().name() == "sqlite" {
                return Err(LimboError::Corrupt(format!(
                    "malformed database schema: database was created by the postgres frontend (pg_next_oid={pg_next_oid})"
                )));
            }
            Ok((open_mv_store, pg_next_oid))
        })();
        staging_pager.end_read_tx();
        validated
    }

    #[cfg(feature = "conn_raw_api")]
    /// Like [`Self::reload_wal_after_external_restore`], supplying the caller's cipher/key when
    /// reloading an encrypted database so the restored image is validated and its catalog parsed
    /// before mutating any live WAL, file descriptor, buffer pool, MVCC store, or registry state.
    pub fn reload_wal_after_external_restore_with_encryption(
        self: &Arc<Self>,
        cipher_override: Option<CipherMode>,
        encryption_key: Option<EncryptionKey>,
    ) -> Result<()> {
        if self.page_codec_id.is_some() {
            return Err(LimboError::InvalidArgument(
                "reloading a WAL after external restore is not supported with an external page codec"
                    .to_string(),
            ));
        }
        let cipher = cipher_override.unwrap_or_else(|| self.encryption_cipher_mode.get());
        let encryption_key = encryption_key.or_else(|| self.restore_encryption_key.lock().clone());
        let encrypted = encryption_key.is_some() || !matches!(cipher, CipherMode::None);
        if !matches!(cipher, CipherMode::None) && encryption_key.is_none() {
            return Err(LimboError::InvalidArgument(
                "reloading an encrypted database after external restore requires the encryption key"
                    .to_string(),
            ));
        }

        // RAII guard for a `RegistryEntry::Opening` reservation placed on a renamed target inode
        // during Stage 1. Prevents concurrent `Database::open` from constructing a second handle
        // for the restored inode (Spec P1-4), and cleans up automatically if any staging step fails.
        struct ReloadRegistryReservation {
            key: Option<DatabaseKey>,
        }
        impl ReloadRegistryReservation {
            fn disarm(&mut self) -> Option<DatabaseKey> {
                self.key.take()
            }
        }
        impl Drop for ReloadRegistryReservation {
            fn drop(&mut self) {
                if let Some(key) = self.key.take() {
                    let mut registry = DATABASE_MANAGER.lock();
                    if matches!(registry.get(&key), Some(RegistryEntry::Opening)) {
                        registry.remove(&key);
                    }
                }
            }
        }
        let mut registry_reservation = ReloadRegistryReservation { key: None };

        let flags = self.open_flags;
        // Stage 1: inspect inode identity and reserve the replacement inode in `DATABASE_MANAGER`
        // without touching live state or dropping any upgraded `Arc<Database>` under the lock.
        type StagedRestoreFile = (
            Option<Arc<dyn io::File>>,
            Option<io::FileId>,
            Arc<dyn DatabaseStorage>,
        );
        let (reopened, new_file_id, candidate_storage): StagedRestoreFile = if !self
            .is_in_memory_db()
        {
            let current_fid = self.io.file_id(&self.path)?;
            let old_fid = *self.file_id.lock();
            if old_fid != Some(current_fid) {
                let new_key = DatabaseKey::File(current_fid, self.io.registry_scope());
                enum RegistryProbe {
                    Reserved,
                    AlreadySelf,
                    OpeningConflict,
                    OtherOwner(Arc<Database>),
                }
                let probe = {
                    let mut registry = DATABASE_MANAGER.lock();
                    let self_weak = Arc::downgrade(self);
                    match registry.get(&new_key) {
                        Some(RegistryEntry::Opening) => RegistryProbe::OpeningConflict,
                        Some(RegistryEntry::Ready(weak)) if Weak::ptr_eq(weak, &self_weak) => {
                            RegistryProbe::AlreadySelf
                        }
                        Some(RegistryEntry::Ready(weak)) => match weak.upgrade() {
                            Some(other) => RegistryProbe::OtherOwner(other),
                            None => {
                                registry.insert(new_key.clone(), RegistryEntry::Opening);
                                RegistryProbe::Reserved
                            }
                        },
                        None => {
                            registry.insert(new_key.clone(), RegistryEntry::Opening);
                            RegistryProbe::Reserved
                        }
                    }
                };
                match probe {
                    RegistryProbe::Reserved => {
                        registry_reservation.key = Some(new_key);
                    }
                    RegistryProbe::AlreadySelf => {}
                    RegistryProbe::OpeningConflict => {
                        return Err(LimboError::LockingError(format!(
                            "cannot reload '{}': restored file is currently being opened",
                            self.path
                        )));
                    }
                    RegistryProbe::OtherOwner(other) => {
                        // Dropped outside `DATABASE_MANAGER` so `ScopedRegistration::drop` cannot
                        // self-deadlock if `other` was the final strong reference.
                        drop(other);
                        return Err(LimboError::LockingError(format!(
                            "cannot reload '{}': restored inode is already open under another database handle",
                            self.path
                        )));
                    }
                }
                let file = self.io.open_file(&self.path, flags, true)?;
                let candidate: Arc<dyn DatabaseStorage> = Arc::new(DatabaseFile::new(file.clone()));
                (Some(file), Some(current_fid), candidate)
            } else {
                if std::env::var("LIMBO_DISABLE_FILE_LOCK").is_err()
                    && !flags.intersects(OpenFlags::ReadOnly | OpenFlags::NoLock)
                {
                    self.db_file.reacquire_lock()?;
                }
                let candidate = self
                    .db_file
                    .unshared_storage_clone()
                    .unwrap_or_else(|| self.db_file.clone());
                (None, Some(current_fid), candidate)
            }
        } else {
            (None, None, self.db_file.clone())
        };

        // Stage 2: recover candidate `-wal` and `-wal2` segments and validate page geometry.
        #[cfg(host_shared_wal)]
        let shared_authority = self.open_shared_wal_coordination_for_open()?;
        #[cfg(not(host_shared_wal))]
        let shared_authority: Option<()> = None;

        let new_first_wal = {
            #[cfg(host_shared_wal)]
            {
                if let Some(authority) = shared_authority.as_ref() {
                    if !authority.frame_index_overflowed() {
                        WalFileShared::open_shared_from_authority_if_exists(
                            &self.io,
                            &self.wal_path,
                            flags,
                            authority,
                            &candidate_storage,
                        )?
                    } else {
                        WalFileShared::open_shared_if_exists(&self.io, &self.wal_path, flags)?
                    }
                } else {
                    WalFileShared::open_shared_if_exists(&self.io, &self.wal_path, flags)?
                }
            }
            #[cfg(not(host_shared_wal))]
            {
                WalFileShared::open_shared_if_exists(&self.io, &self.wal_path, flags)?
            }
        };
        // Decision 45: recover `-wal2` alongside `-wal`, matching cold `Database::open`.
        let second_wal_path = storage::wal::second_segment_path(&self.wal_path);
        let new_second_wal =
            WalFileShared::open_second_segment_if_exists(&self.io, &second_wal_path, flags)?;
        let segment_seq = |segment: &Arc<RwLock<WalFileShared>>| {
            let shared = segment.read();
            shared
                .metadata
                .initialized
                .load(Ordering::Acquire)
                .then(|| shared.metadata.wal_header.lock().checkpoint_seq)
        };
        let (first_seq, second_seq) = (segment_seq(&new_first_wal), segment_seq(&new_second_wal));
        let segments = crate::storage::log_segments::Segments::recover(first_seq, second_seq, true)
            .ok_or_else(|| {
                LimboError::Corrupt(format!(
                    "restored WAL segments have checkpoint sequences {first_seq:?} and {second_seq:?}, neither one past the other"
                ))
            })?;

        let mut hdr_st = DbHeaderReadState::Start;
        let header = self
            .io
            .block(|| Self::read_db_header_buf_from(&candidate_storage, &mut hdr_st))?;
        let file_is_empty = matches!(&header, DbHeaderRead::Short { len: 0, .. });
        let (header_reserved_bytes, header_page_size) = if file_is_empty {
            (None, None)
        } else {
            let (reserved, page_size) = Self::plain_header_page_info(&header, encrypted)?;
            (Some(reserved), Some(page_size))
        };
        let (new_page_size, wal_enabled, last_checksum_and_max_frame, has_wal_frames) = {
            let current_wal = match segments.current() {
                storage::log_segments::SegmentId::First => new_first_wal.read(),
                storage::log_segments::SegmentId::Second => new_second_wal.read(),
            };
            let page_size =
                self.determine_actual_page_size(&current_wal, None, header_page_size)?;
            let wal_enabled = current_wal.metadata.enabled.load(Ordering::SeqCst);
            let last_checksum_and_max_frame = current_wal.last_checksum_and_max_frame();
            let current_frames = current_wal.metadata.max_frame.load(Ordering::Acquire);
            drop(current_wal);
            let other_frames = match segments.current() {
                storage::log_segments::SegmentId::First => new_second_wal
                    .read()
                    .metadata
                    .max_frame
                    .load(Ordering::Acquire),
                storage::log_segments::SegmentId::Second => new_first_wal
                    .read()
                    .metadata
                    .max_frame
                    .load(Ordering::Acquire),
            };
            (
                page_size,
                wal_enabled,
                last_checksum_and_max_frame,
                current_frames > 0 || other_frames > 0,
            )
        };
        let is_empty_image = file_is_empty && !has_wal_frames;
        let cipher_opt = if !matches!(cipher, CipherMode::None) {
            Some(cipher)
        } else {
            None
        };
        let new_reserved_bytes = header_reserved_bytes.or_else(|| {
            if let Some(c) = cipher_opt {
                u8::try_from(c.metadata_size()).ok()
            } else if is_empty_image {
                Some(CHECKSUM_REQUIRED_RESERVED_BYTES)
            } else {
                None
            }
        });
        let disable_checksums =
            new_reserved_bytes.is_some_and(|reserved| reserved != CHECKSUM_REQUIRED_RESERVED_BYTES);
        let log_exists = !self.is_in_memory_db()
            && journal_mode::logical_log_exists(std::path::Path::new(&self.path));

        // Stage 3: validate page 1 (header, journal mode, checksum/decryption), parse the catalog,
        // and (for MVCC images) open & bootstrap the replacement `MvStore` on an isolated
        // unregistered staging `Database` BEFORE mutating any live state on `self`.
        let (restored_schema, staged_mv_store, restored_init_page_1, restored_pg_next_oid): (
            Arc<Schema>,
            Option<Arc<MvStore>>,
            Option<PageRef>,
            u32,
        ) = if is_empty_image {
            if log_exists {
                return Err(LimboError::Corrupt(format!(
                    "MVCC logical log file exists for database {}, but restored database is empty (WAL mode). The database may be corrupted.",
                    self.path
                )));
            }
            let dialect = self.dialect();
            let mut empty =
                Schema::with_options(self.experimental_custom_types_enabled(), dialect.as_ref())?;
            empty.generated_columns_enabled = self.opts.enable_generated_columns;
            (
                Arc::new(empty),
                None,
                Some(pager::default_page1(cipher_opt.as_ref())),
                0,
            )
        } else {
            let staging_pool = BufferPool::begin_init(&self.io, BufferPool::TEST_ARENA_SIZE);
            staging_pool.finalize_with_page_size(new_page_size.get() as usize)?;
            let staging_shared_wal = SharedLog::recover(
                new_first_wal.clone(),
                new_second_wal.clone(),
                Some(second_wal_path),
            )?;
            let staging_wal: Option<Arc<dyn Wal>> = if wal_enabled {
                Some(Arc::new(WalFile::new(
                    self.io.clone(),
                    staging_shared_wal.clone(),
                    last_checksum_and_max_frame,
                    staging_pool.clone(),
                )))
            } else {
                None
            };
            let staging_pager = Arc::new(Pager::new(
                candidate_storage.clone(),
                staging_wal,
                self.io.clone(),
                PageCache::default(),
                staging_pool.clone(),
                Arc::new(SharedPagerState::new(None)),
            )?);
            staging_pager.set_page_size(new_page_size);
            if let Some(reserved) = new_reserved_bytes {
                staging_pager.set_reserved_space_bytes(reserved);
            }
            if disable_checksums {
                staging_pager.reset_checksum_context();
            }
            if let Some(ref key) = encryption_key {
                staging_pager.enable_encryption(true);
                staging_pager.set_encryption_context(cipher, key)?;
            }

            let (open_mv_store, pg_next_oid) =
                self.validate_restored_header(&staging_pager, encryption_key.as_ref(), log_exists)?;

            let staging_db = self.build_staging_for_restore(
                candidate_storage.clone(),
                staging_pool,
                staging_shared_wal.clone(),
                cipher,
                encryption_key.clone(),
            )?;
            // Parse the base B-tree catalog first (with `staging_db.mv_store == None`, matching
            // cold `OpenDbAsyncPhase::LoadingSchema`).
            let refresh_conn = staging_db.new_connection(
                true,
                staging_pager.clone(),
                encryption_key.clone(),
                0,
                None,
            )?;
            let parse_res = refresh_conn.force_reparse_schema();
            drop(refresh_conn);
            parse_res?;

            let staged_mv = if open_mv_store {
                let canonical_path = self.get_database_canonical_path();
                let enc_ctx = staging_pager.io_ctx.read().encryption_context().cloned();
                let mv_store = journal_mode::open_mv_store(
                    self.io.clone(),
                    &canonical_path,
                    self.open_flags,
                    self.durable_storage.clone(),
                    enc_ctx,
                    self.allocators.mv_store.clone(),
                    self.experimental_mvcc_passive_checkpoint_enabled(),
                )?;
                staging_db.mv_store.store(Some(mv_store.clone()));
                let mvcc_bootstrap_conn = staging_db.new_connection(
                    true,
                    staging_pager.clone(),
                    encryption_key,
                    0,
                    None,
                )?;
                let boot_res = match mv_store.bootstrap(mvcc_bootstrap_conn.clone()) {
                    Ok(()) => Ok(()),
                    Err(LimboError::SchemaUpdated) => {
                        mvcc_bootstrap_conn.force_reparse_schema()?;
                        mv_store.bootstrap(mvcc_bootstrap_conn.clone())
                    }
                    Err(error) => Err(error),
                };
                drop(mvcc_bootstrap_conn);
                boot_res?;
                Some(mv_store)
            } else {
                None
            };

            let final_schema = staging_db.clone_schema();
            drop(staging_db);
            drop(staging_pager);
            drop(staging_shared_wal);
            (final_schema, staged_mv, None, pg_next_oid)
        };

        let new_first_wal = Arc::try_unwrap(new_first_wal).map_err(|_| {
            LimboError::InternalError(
                "new WAL first segment unexpectedly shared during external restore reload"
                    .to_string(),
            )
        })?;
        let new_second_wal = Arc::try_unwrap(new_second_wal).map_err(|_| {
            LimboError::InternalError(
                "new WAL second segment unexpectedly shared during external restore reload"
                    .to_string(),
            )
        })?;

        // Stage 4: reconfigure the live buffer pool before mutating any live file/WAL/MVCC/
        // registry state so any allocation failure leaves the live database completely untouched.
        self.buffer_pool
            .reconfigure_page_size_after_external_restore(new_page_size.get() as usize)?;

        // Stage 5: infallible atomic commit to live state.
        if let Some(shared) = self.db_file.shared_pages() {
            shared.forget_all();
        }
        if let (Some(reopened_file), Some(new_fid)) = (reopened, new_file_id) {
            self.db_file.reopen_after_external_restore(reopened_file);
            *self.file_id.lock() = Some(new_fid);
            let new_key = DatabaseKey::File(new_fid, self.io.registry_scope());
            let self_weak = Arc::downgrade(self);
            let mut registry = DATABASE_MANAGER.lock();
            let reserved_key = registry_reservation.disarm();
            let mut reg_key = self.registry_key.lock();
            let mut promoted = false;
            if let Some(old_key) = reg_key.as_ref() {
                if *old_key != new_key {
                    let owned = matches!(
                        registry.get(old_key),
                        Some(RegistryEntry::Ready(owner)) if Weak::ptr_eq(owner, &self_weak)
                    );
                    if owned {
                        registry.remove(old_key);
                    }
                    registry.insert(new_key.clone(), RegistryEntry::Ready(self_weak));
                    *reg_key = Some(new_key.clone());
                    promoted = true;
                }
            }
            if !promoted {
                if let Some(key) = reserved_key {
                    if matches!(registry.get(&key), Some(RegistryEntry::Opening)) {
                        registry.remove(&key);
                    }
                }
            }
            if let Some(scoped) = self.registry_registration.lock().as_mut() {
                scoped.key = new_key;
            }
        }
        {
            let mut schema_guard = self.schema.lock();
            self.pager_state.set_restored_page_info(
                new_page_size,
                new_reserved_bytes,
                restored_init_page_1,
            );
            self.mv_store.store(staged_mv_store);
            self.dialect().load_next_oid(restored_pg_next_oid);
            *schema_guard = restored_schema;
            self.bump_schema_epoch();
            self.shared_wal.replace_after_external_restore(
                new_first_wal.into_inner(),
                new_second_wal.into_inner(),
                segments,
            );
        }
        Ok(())
    }

    #[instrument(skip_all, level = Level::DEBUG)]
    pub fn connect(self: &Arc<Database>, opts: ConnectOpts) -> Result<Arc<Connection>> {
        let ask = opts.versioned || crate::storage::versioned::switch::asked();
        let made = ask && self.versioned.lock().is_none();
        self.versioned_storage(ask, opts.memory.clone())?;
        let conn = self._connect(
            false,
            None,
            opts.encryption_key,
            opts.page_codec,
            None,
            StatsRefresh::Blocking,
        )?;
        // Amphora (G7 step 2): the database was opened on the pager's schema; the storage made
        // now holds its own (`sqlite_schema` as its tree 1), which the database takes.
        if made {
            conn.load_versioned_schema()?;
        }
        Ok(conn)
    }

    #[instrument(skip_all, level = Level::DEBUG)]
    pub fn connect_with_context<C: Any + Send + Sync>(
        self: &Arc<Database>,
        context: Arc<C>,
    ) -> Result<Arc<Connection>> {
        let ask = crate::storage::versioned::switch::asked();
        let made = ask && self.versioned.lock().is_none();
        self.versioned_storage(ask, None)?;
        let conn = self._connect(
            false,
            None,
            None,
            None,
            Some(context),
            StatsRefresh::Blocking,
        )?;
        if made {
            conn.load_versioned_schema()?;
        }
        Ok(conn)
    }

    /// Non-blocking [`Self::connect`].
    ///
    /// Creating the connection itself never waits, but the connect-time
    /// ANALYZE stats refresh runs a `SELECT` over `sqlite_stat1`, and that
    /// statement can have to wait: on page I/O, or on another transaction
    /// (a read that speculatively saw a version whose writer is still
    /// preparing waits for that writer at its own commit). `connect` pumps
    /// `io.step()` until the statement finishes, which is only correct when
    /// the other transaction can make progress on its own. In a host that
    /// schedules every connection cooperatively on one thread it cannot:
    /// the wait is an explicit yield asking the host to run *other*
    /// connections, and `io.step()` never does that, so `connect` spins.
    ///
    /// This variant hands every wait back to the caller as
    /// `IOResult::IO(..)`. Drive it like `open_async`: on `IO`, run the host
    /// scheduler if the completion is an explicit yield, wait for the
    /// completion, then call again with the same `state`. The stats refresh
    /// stays best-effort: a failure to prepare or scan `sqlite_stat1` is
    /// logged and the connection is returned without stats.
    ///
    /// Only the first call, which creates the connection, uses `opts`; later
    /// calls with the same `state` only resume the stats refresh.
    pub fn connect_async(
        self: &Arc<Database>,
        opts: ConnectOpts,
        state: &mut ConnectAsyncState,
    ) -> IOResultOr<Arc<Connection>> {
        let conn = match &state.conn {
            Some(conn) => conn.clone(),
            None => {
                let ask = opts.versioned || crate::storage::versioned::switch::asked();
                let made = ask && self.versioned.lock().is_none();
                self.versioned_storage(ask, opts.memory.clone())?;
                let conn = self._connect(
                    false,
                    None,
                    opts.encryption_key,
                    opts.page_codec,
                    None,
                    StatsRefresh::Deferred,
                )?;
                if made {
                    conn.load_versioned_schema()?;
                }
                state.conn = Some(conn.clone());
                conn
            }
        };
        match refresh_analyze_stats_on_connect_nonblock(&conn, &mut state.refresh) {
            Ok(IOResult::IO(io)) => return Ok(IOResult::IO(io)),
            Ok(IOResult::Done(())) => {}
            Err(err) => {
                tracing::warn!("Failed to refresh analyze stats on connect: {err}");
            }
        }
        *state = ConnectAsyncState::default();
        Ok(IOResult::Done(conn))
    }

    #[instrument(skip_all, level = Level::DEBUG)]
    fn _connect(
        self: &Arc<Database>,
        is_mvcc_bootstrap_connection: bool,
        pager: Option<Arc<Pager>>,
        encryption_key: Option<EncryptionKey>,
        page_codec: Option<Arc<dyn PageCodec>>,
        context: Option<Arc<dyn Any + Send + Sync>>,
        stats: StatsRefresh,
    ) -> Result<Arc<Connection>> {
        if self.page_codec_id.is_some() && page_codec.is_none() {
            return Err(LimboError::InvalidArgument(
                "database requires an external page codec".to_string(),
            ));
        }
        if self.page_codec_id.is_none() && page_codec.is_some() {
            return Err(LimboError::InvalidArgument(
                "database was opened without an external page codec".to_string(),
            ));
        }
        if let Some(page_codec) = page_codec.as_deref() {
            self.validate_page_codec(Some(page_codec))?;
        }
        let pager = if let Some(pager) = pager {
            pager
        } else {
            // Pass encryption key to _init so it can set up encryption context
            // before reading page 1. This is required for reopening encrypted databases.
            Arc::new(self._init(encryption_key.as_ref(), page_codec.clone())?)
        };
        let default_cache_size = pager
            .io
            .block(|| pager.with_header(|header| header.default_page_cache_size))
            .unwrap_or_default()
            .get();

        let conn = self.new_connection(
            is_mvcc_bootstrap_connection,
            pager,
            encryption_key,
            default_cache_size,
            context,
        )?;
        if stats == StatsRefresh::Blocking {
            refresh_analyze_stats_on_connect(&conn);
        }
        Ok(conn)
    }

    /// Create a connection and run the blocking connect-time stats refresh.
    /// Hosts that schedule connections cooperatively must use
    /// [`Self::connect_async`] instead; see its docs.
    pub(crate) fn _connect_with_pager_and_default_cache_size(
        self: &Arc<Database>,
        is_mvcc_bootstrap_connection: bool,
        pager: Arc<Pager>,
        encryption_key: Option<EncryptionKey>,
        default_cache_size: i32,
    ) -> Result<Arc<Connection>> {
        let conn = self.new_connection(
            is_mvcc_bootstrap_connection,
            pager,
            encryption_key,
            default_cache_size,
            None,
        )?;
        refresh_analyze_stats_on_connect(&conn);
        Ok(conn)
    }

    /// Build the `Connection` object. Does not touch the database: the
    /// ANALYZE stats refresh is the caller's job, blocking or not.
    fn new_connection(
        self: &Arc<Database>,
        is_mvcc_bootstrap_connection: bool,
        pager: Arc<Pager>,
        encryption_key: Option<EncryptionKey>,
        default_cache_size: i32,
        context: Option<Arc<dyn Any + Send + Sync>>,
    ) -> Result<Arc<Connection>> {
        let encryption_cipher = self.encryption_cipher_mode.get();
        let versioned = self
            // The switch in the environment opens a storage the database already has (one to
            // recover) as soon as the database opens; a fresh one is made by the first
            // connection asked for through `connect`, which carries its memory source
            // (`ConnectOpts::with_memory`; G7 step 2, `AC-NS.2.1`). Once made, every
            // connection is on it.
            .versioned_storage(
                crate::storage::versioned::switch::asked() && self.versioned_found(),
                None,
            )?
            .map(|s| Arc::new(crate::storage::versioned::switch::Switch::new(s)));
        let (initial_schema, initial_schema_epoch, initial_restore_epoch) = {
            let shared = self.schema.lock();
            (
                shared.clone(),
                self.schema_epoch.load(Ordering::Acquire),
                self.shared_wal.restore_epoch(),
            )
        };
        let conn = Arc::new(Connection {
            cost_runs: crate::translate::optimizer::learned_costs::RunCounts::staggered(
                self.cost_run_seed.fetch_add(1, Ordering::Relaxed),
            ),
            versioned,
            versioned_attached: RwLock::new(HashMap::default()),
            db: self.clone(),
            context,
            pager: ArcSwap::new(pager),
            schema: crate::connection::SchemaCell::new(initial_schema),
            local_schema_epoch: AtomicU64::new(initial_schema_epoch),
            last_restore_epoch: AtomicU64::new(initial_restore_epoch),
            database_schemas: RwLock::new(HashMap::default()),
            auto_commit: AtomicBool::new(true),
            transaction_state: AtomicTransactionState::new(TransactionState::None),
            poisoned_tx: AtomicBool::new(false),
            online_change_own: AtomicBool::new(false),
            statement_journals: AtomicBool::new(true),
            last_insert_rowid: AtomicI64::new(0),
            changes: AtomicI64::new(0),
            total_changes: AtomicI64::new(0),
            syms: parking_lot::RwLock::new(SymbolTable::new()),
            _shared_cache: false,
            cache_size: AtomicI32::new(default_cache_size),
            wal_auto_actions: AtomicU8::new(WalAutoActions::all_enabled().bits()),
            wal_autocheckpoint: AtomicU32::new(1000),
            #[cfg(feature = "conn_raw_api")]
            portable_logical_changes_enabled: AtomicBool::new(false),
            #[cfg(feature = "conn_raw_api")]
            mvcc_log_metadata: RwLock::new(HashMap::default()),
            capture_data_changes: RwLock::new(None),
            cdc_transaction_id: AtomicI64::new(-1),
            closed: AtomicBool::new(false),
            temp: crate::connection::TempDbContext::new(),
            attached_databases: RwLock::new(DatabaseCatalog::new()),
            has_non_main_pagers: AtomicBool::new(false),
            query_only: AtomicBool::new(false),
            fp_digits: AtomicU8::new(crate::numeric::fp::FpDigits::DEFAULT.get()),
            row_counters: AtomicBool::new(false),
            plan_cache_mode: AtomicU8::new(crate::PlanCacheMode::Auto.to_u8()),
            vdbe_trace: AtomicBool::new(false),
            dml_require_where: AtomicBool::new(false),
            count_changes: AtomicBool::new(false),
            fts_merge_threshold: AtomicI64::new(crate::index_method::DEFAULT_FTS_MERGE_THRESHOLD),
            dqs_dml: AtomicBool::new(true),
            sequence_inner_retries: AtomicU64::new(0),
            mv_tx: RwLock::new(None),
            attached_mv_txs: RwLock::new(HashMap::default()),
            #[cfg(any(test, injected_yields))]
            yield_injector: RwLock::new(None),
            #[cfg(any(test, injected_yields))]
            failure_injector: RwLock::new(None),
            #[cfg(any(test, injected_yields))]
            yield_instance_id_counter: AtomicU64::new(1),
            view_transaction_states: AllViewsTxState::new(),
            metrics: RwLock::new(ConnectionMetrics::new()),
            nestedness: AtomicI32::new(0),
            compiling_triggers: RwLock::new(Vec::new()),
            executing_triggers: RwLock::new(Vec::new()),
            encryption_key: RwLock::new(encryption_key),
            encryption_cipher_mode: AtomicCipherMode::new(encryption_cipher),
            sync_mode: AtomicSyncMode::new(SyncMode::Full),
            temp_store: AtomicTempStore::new(TempStore::Default),
            data_sync_retry: AtomicBool::new(false),
            busy_handler: RwLock::new(BusyHandler::None),
            busy_schedule: AtomicBool::new(false),
            progress_handler: ProgressHandler::new(),
            query_timeout_ms: AtomicU64::new(0),
            interrupt_requested: AtomicBool::new(false),
            is_mvcc_bootstrap_connection: AtomicBool::new(is_mvcc_bootstrap_connection),
            full_column_names: AtomicBool::new(false),
            short_column_names: AtomicBool::new(true),
            #[cfg(feature = "simulator")]
            subquery_unnesting_mode: crate::connection::AtomicSubqueryUnnestingMode::new(
                crate::connection::SubqueryUnnestingMode::Auto,
            ),
            join_methods: crate::connection::AtomicJoinMethods::new(
                crate::connection::JoinMethods::ByCost,
            ),
            join_search_budget: crate::connection::AtomicJoinSearchBudget::new(
                crate::connection::JoinSearchBudget::Fixed,
            ),
            carried_join_orders: crate::connection::AtomicCarriedJoinOrders::new(
                crate::connection::CarriedJoinOrders::Off,
            ),
            bushy_left_join_probes: crate::connection::AtomicBushyLeftJoinProbes::new(
                crate::connection::BushyLeftJoinProbes::Off,
            ),
            background_search: crate::connection::AtomicBackgroundSearch::new(
                crate::connection::BackgroundSearch::Off,
            ),
            idle_searches: crate::background_search::IdleSearches::default(),
            star_filters: crate::connection::AtomicStarFilters::new(
                crate::connection::StarFilters::ByCost,
            ),
            is_value_estimate: crate::connection::AtomicIsValueEstimate::new(
                crate::connection::IsValueEstimate::Equality,
            ),
            star_filter_order: crate::connection::AtomicStarFilterOrder::new(
                crate::connection::StarFilterOrder::Planned,
            ),
            sorted_fetches: AtomicBool::new(false),
            adaptive_joins: AtomicBool::new(false),
            adaptive_join_switches: AtomicU64::new(0),
            adaptive_replans: AtomicU64::new(0),
            adaptive_build_checks: AtomicU64::new(0),
            adaptive_filter_checks: AtomicU64::new(0),
            adaptive_filter_corrections: AtomicU64::new(0),
            adaptive_sort_checks: AtomicU64::new(0),
            adaptive_subquery_checks: AtomicU64::new(0),
            adaptive_handoff: AtomicBool::new(true),
            adaptive_handoffs: AtomicU64::new(0),
            pinned_cost_weights: crate::sync::RwLock::new(None),
            automatic_indexes: AtomicBool::new(true),
            parallel_mode: AtomicBool::new(false),
            parallel_limit: AtomicBool::new(false),
            parallel_cores: std::sync::atomic::AtomicUsize::new(0),
            parallel_join_direct: AtomicBool::new(false),
            parallel_outer_probe: crate::connection::AtomicOuterJoinProbe::new(
                crate::connection::OuterJoinProbe::Padded,
            ),
            morsel_costs: crate::sync::Mutex::new(None),
            morsel_body_rows: AtomicU64::new(0),
            morsel_join_links: AtomicU64::new(0),
            morsel_join_direct_rows: AtomicU64::new(0),
            morsel_outer: crate::vdbe::morsel_scan::OuterCounters::default(),
            parallel_searched_links: crate::connection::AtomicSearchedLinks::new(
                crate::connection::SearchedLinks::Off,
            ),
            real_sums: crate::connection::AtomicRealSums::new(crate::connection::RealSums::Serial),
            morsel_searched_links: AtomicU64::new(0),
            morsel_seek_links: AtomicU64::new(0),
            vectorized_scans: AtomicBool::new(false),
            enable_load_extension: AtomicBool::new(self.can_load_extensions()),
            fk_pragma: AtomicBool::new(false),
            fk_deferred_violations: AtomicIsize::new(0),
            n_active_writes: AtomicI32::new(0),
            statement_activity: Arc::new(AtomicU64::new(0)),
            check_constraints_pragma: AtomicBool::new(false),
            vtab_txn_states: RwLock::new(HashSet::default()),
            index_method_tx_cursors: crate::sync::Mutex::new(Vec::new()),
            has_index_method_tx_cursors: crate::sync::atomic::AtomicBool::new(false),
            named_savepoints: RwLock::new(Vec::new()),
            schema_reparse_in_progress: AtomicBool::new(false),
            prepare_context_generation: AtomicU64::new(0),
            sequence_currvals: RwLock::new(HashMap::default()),
        });
        self.n_connections
            .fetch_add(1, crate::sync::atomic::Ordering::SeqCst);
        let builtin_syms = self.builtin_syms.read();
        // add built-in extensions symbols to the connection to prevent having to load each time
        conn.syms.write().extend(&builtin_syms);
        Ok(conn)
    }

    /// Whether the database already has a versioned storage to recover: one its in-memory IO
    /// keeps beside the file, or the directory `<path>-versioned` of a file database.
    fn versioned_found(&self) -> bool {
        if self.io.is_memory() || is_memory_like(&self.path) {
            return self.io.versioned_kept(&self.path);
        }
        std::path::Path::new(&format!("{}-versioned", self.path)).is_dir()
    }

    /// The versioned storage the connections' B-trees are on: made now if `ask` and not yet made.
    pub(crate) fn versioned_storage(
        &self,
        ask: bool,
        memory: Option<Arc<dyn crate::storage::versioned::memory::Source>>,
    ) -> Result<Option<Arc<crate::storage::versioned::switch::Storage>>> {
        let mut slot = self.versioned.lock();
        if slot.is_none() && ask {
            // An IO whose files live in memory keeps the storage beside the file, so a database
            // reopened on it finds its trees (G7 step 2, `AC-NS.2.1`).
            let memory_io = self.io.is_memory();
            let mut make =
                || crate::storage::versioned::switch::open(&self.path, memory_io, memory.clone());
            let kept = self.io.versioned_beside(&self.path, &mut make)?;
            *slot = Some(match kept {
                Some(storage) => storage,
                None => make()?,
            });
        }
        Ok(slot.clone())
    }

    pub fn is_readonly(&self) -> bool {
        self.open_flags.contains(OpenFlags::ReadOnly)
    }

    /// Non-blocking read of the 512-byte database file header (page 1's
    /// header region). Yields the read completion via the supplied state until
    /// it finishes. The header may not be complete, in which case a
    /// [`DbHeaderRead::Short`] is returned.
    fn read_db_header_buf(&self, st: &mut DbHeaderReadState) -> IOResultOr<DbHeaderRead> {
        Self::read_db_header_buf_from(&self.db_file, st)
    }

    fn read_db_header_buf_from(
        db_file: &Arc<dyn DatabaseStorage>,
        st: &mut DbHeaderReadState,
    ) -> IOResultOr<DbHeaderRead> {
        loop {
            match st {
                DbHeaderReadState::Start => {
                    turso_assert!(
                        PageSize::MIN.is_multiple_of(512),
                        "header read must be a multiple of 512 for O_DIRECT"
                    );
                    let buf = Arc::new(Buffer::new_temporary(PageSize::MIN as usize));
                    let c = new_header_read_completion(buf.clone());
                    let c = db_file.read_header(c)?;
                    *st = DbHeaderReadState::Reading { buf, completion: c };
                }
                DbHeaderReadState::Reading { completion, .. } => {
                    let err = completion.get_error();
                    if err.is_none() && !completion.succeeded() {
                        let c = completion.clone();
                        io_yield_one!(c);
                    }
                    let DbHeaderReadState::Reading { buf, .. } = std::mem::take(st) else {
                        unreachable!("header read state must be Reading here");
                    };
                    return match err {
                        None => Ok(IOResult::Done(DbHeaderRead::Full(buf))),
                        Some(err @ CompletionError::ShortRead { actual, .. }) => {
                            Ok(IOResult::Done(DbHeaderRead::Short {
                                buf,
                                len: actual,
                                err,
                            }))
                        }
                        Some(err) => Err(err.into()),
                    };
                }
            }
        }
    }

    /// Determine the actual page size, in order of preference:
    /// 1. From the WAL header if it exists and is initialized
    /// 2. From `header_page_size` (read from the DB header by the caller) if
    ///    the database is initialized
    ///
    /// Otherwise, fall back to, in order of preference:
    /// 1. From the requested page size if it is provided
    /// 2. PageSize::default(), i.e. 4096
    fn determine_actual_page_size(
        &self,
        shared_wal: &WalFileShared,
        requested_page_size: Option<usize>,
        header_page_size: Option<PageSize>,
    ) -> Result<PageSize> {
        if shared_wal.metadata.enabled.load(Ordering::SeqCst) {
            let size_in_wal = shared_wal.page_size();
            if size_in_wal != 0 {
                let Some(page_size) = PageSize::new(size_in_wal) else {
                    bail_corrupt_error!("invalid page size in WAL: {size_in_wal}");
                };
                return Ok(page_size);
            }
        }
        if let Some(page_size) = header_page_size {
            Ok(page_size)
        } else {
            let Some(size) = requested_page_size else {
                return Ok(PageSize::default());
            };
            let Some(page_size) = PageSize::new(size as u32) else {
                bail_corrupt_error!("invalid requested page size: {size}");
            };
            Ok(page_size)
        }
    }

    #[cfg(all(unix, target_pointer_width = "64", target_os = "macos"))]
    fn filesystem_type_allows_shared_wal(fs_type: &str) -> bool {
        // Network and distributed filesystems where mmap'd shared memory
        // cannot guarantee cross-process coherency.
        !matches!(
            fs_type,
            "nfs" | "smbfs" | "afpfs" | "webdav" | "cifs" | "acfs"
        )
    }

    #[cfg(all(
        unix,
        target_pointer_width = "64",
        not(any(target_os = "linux", target_os = "android")),
        not(target_os = "macos")
    ))]
    fn filesystem_type_allows_shared_wal(_fs_type: &str) -> bool {
        true
    }

    #[cfg(all(
        unix,
        target_pointer_width = "64",
        any(target_os = "linux", target_os = "android")
    ))]
    pub(crate) fn filesystem_magic_allows_shared_wal(filesystem_magic: libc::c_long) -> bool {
        const AFS_SUPER_MAGIC: libc::c_long = 0x5346_414f;
        const CIFS_SUPER_MAGIC: libc::c_long = 0xFF53_4D42u32 as libc::c_long;
        const CODA_SUPER_MAGIC: libc::c_long = 0x7375_7245;
        const CEPH_SUPER_MAGIC: libc::c_long = 0x00C3_6400;
        const GFS2_SUPER_MAGIC: libc::c_long = 0x0116_1970;
        const LUSTRE_SUPER_MAGIC: libc::c_long = 0x0BD0_0BD0;
        const NCP_SUPER_MAGIC: libc::c_long = 0x564c;
        const NFS_SUPER_MAGIC: libc::c_long = 0x6969;
        const OCFS2_SUPER_MAGIC: libc::c_long = 0x7461_636f;
        const SMB2_SUPER_MAGIC: libc::c_long = 0xFE53_4D42u32 as libc::c_long;
        const V9FS_SUPER_MAGIC: libc::c_long = 0x0102_1997;

        !matches!(
            filesystem_magic,
            AFS_SUPER_MAGIC
                | CIFS_SUPER_MAGIC
                | CODA_SUPER_MAGIC
                | CEPH_SUPER_MAGIC
                | GFS2_SUPER_MAGIC
                | LUSTRE_SUPER_MAGIC
                | NCP_SUPER_MAGIC
                | NFS_SUPER_MAGIC
                | OCFS2_SUPER_MAGIC
                | SMB2_SUPER_MAGIC
                | V9FS_SUPER_MAGIC
        )
    }

    #[cfg(all(
        unix,
        target_pointer_width = "64",
        any(target_os = "linux", target_os = "android")
    ))]
    pub(crate) fn path_allows_shared_wal_coordination(path: &Path) -> Result<bool> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let probe_path = if path.exists() {
            path
        } else {
            path.parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."))
        };
        let c_path = CString::new(probe_path.as_os_str().as_bytes()).map_err(|_| {
            LimboError::InvalidArgument(format!(
                "path contains interior NUL bytes: {}",
                probe_path.display()
            ))
        })?;
        let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
        let rc = unsafe { libc::statfs(c_path.as_ptr(), stat.as_mut_ptr()) };
        if rc != 0 {
            return Err(io_error(
                std::io::Error::last_os_error(),
                "statfs shared WAL coordination path",
            ));
        }
        let stat = unsafe { stat.assume_init() };
        Ok(Self::filesystem_magic_allows_shared_wal(
            stat.f_type as libc::c_long,
        ))
    }

    #[cfg(all(
        unix,
        target_pointer_width = "64",
        not(any(target_os = "linux", target_os = "android"))
    ))]
    pub(crate) fn path_allows_shared_wal_coordination(path: &Path) -> Result<bool> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let probe_path = if path.exists() {
            path
        } else {
            path.parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."))
        };
        let c_path = CString::new(probe_path.as_os_str().as_bytes()).map_err(|_| {
            LimboError::InvalidArgument(format!(
                "path contains interior NUL bytes: {}",
                probe_path.display()
            ))
        })?;
        let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
        let rc = unsafe { libc::statfs(c_path.as_ptr(), stat.as_mut_ptr()) };
        if rc != 0 {
            return Err(io_error(
                std::io::Error::last_os_error(),
                "statfs shared WAL coordination path",
            ));
        }
        let stat = unsafe { stat.assume_init() };
        // macOS and other BSDs expose the filesystem type as a
        // null-terminated string in f_fstypename rather than an
        // integer magic number.
        let fs_type = unsafe {
            std::ffi::CStr::from_ptr(stat.f_fstypename.as_ptr())
                .to_str()
                .unwrap_or("")
        };
        Ok(Self::filesystem_type_allows_shared_wal(fs_type))
    }

    #[cfg(all(target_os = "windows", target_pointer_width = "64"))]
    pub(crate) fn path_allows_shared_wal_coordination(path: &Path) -> Result<bool> {
        use std::iter::once;
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumePathNameW};

        const DRIVE_REMOVABLE: u32 = 2;
        const DRIVE_FIXED: u32 = 3;
        const DRIVE_REMOTE: u32 = 4;
        const DRIVE_RAMDISK: u32 = 6;

        let probe_path = if path.exists() {
            path.to_path_buf()
        } else {
            path.parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf()
        };
        let probe_path = if probe_path.is_absolute() {
            probe_path
        } else {
            std::env::current_dir()
                .map_err(|err| io_error(err, "resolve shared WAL coordination path"))?
                .join(probe_path)
        };
        let probe_path_wide: Vec<u16> = probe_path
            .as_os_str()
            .encode_wide()
            .chain(once(0))
            .collect();
        let mut volume_path = vec![0u16; 261];
        let result = unsafe {
            GetVolumePathNameW(
                probe_path_wide.as_ptr(),
                volume_path.as_mut_ptr(),
                volume_path.len() as u32,
            )
        };
        if result == 0 {
            return Err(io_error(
                std::io::Error::last_os_error(),
                "GetVolumePathNameW shared WAL coordination path",
            ));
        }

        let drive_type = unsafe { GetDriveTypeW(volume_path.as_ptr()) };
        Ok(
            matches!(drive_type, DRIVE_FIXED | DRIVE_RAMDISK | DRIVE_REMOVABLE)
                && drive_type != DRIVE_REMOTE,
        )
    }

    #[cfg(host_shared_wal)]
    pub(crate) fn shared_wal_coordination(
        &self,
    ) -> Result<Option<Arc<MappedSharedWalCoordination>>> {
        let shared_wal = self.shared_wal.current().read();
        if !shared_wal.metadata.enabled.load(Ordering::Acquire) {
            return Ok(None);
        }
        drop(shared_wal);
        self.open_shared_wal_coordination_inner()
    }

    #[cfg(not(host_shared_wal))]
    pub(crate) fn shared_wal_coordination(&self) -> Result<Option<()>> {
        Ok(None)
    }

    #[cfg(host_shared_wal)]
    pub(crate) fn open_shared_wal_coordination_for_open(
        &self,
    ) -> Result<Option<Arc<MappedSharedWalCoordination>>> {
        self.open_shared_wal_coordination_inner()
    }

    #[cfg(host_shared_wal)]
    fn open_shared_wal_coordination_inner(
        &self,
    ) -> Result<Option<Arc<MappedSharedWalCoordination>>> {
        if !self.opts.enable_multiprocess_wal {
            return Ok(None);
        }
        if !self.io.supports_shared_wal_coordination() {
            return Err(LimboError::InvalidArgument(format!(
                "experimental multiprocess WAL is not supported by the active IO backend for '{}'",
                self.path
            )));
        }
        if is_memory_like(&self.path) || is_memory_like(&self.wal_path) {
            return Err(LimboError::InvalidArgument(format!(
                "experimental multiprocess WAL is not supported for in-memory database path '{}'",
                self.path
            )));
        }
        if !Self::path_allows_shared_wal_coordination(Path::new(&self.path))? {
            return Err(LimboError::InvalidArgument(format!(
                "experimental multiprocess WAL is not supported on the filesystem backing '{}'",
                self.path
            )));
        }
        if let Some(authority) = self.shared_wal_coordination.get() {
            return Ok(Some(authority.clone()));
        }

        let path = storage::wal::coordination_path_for_wal_path(&self.wal_path);
        let authority = if self.open_flags.contains(OpenFlags::ReadOnly) {
            let Some(authority) = MappedSharedWalCoordination::open_existing(
                &self.io,
                std::path::Path::new(&path),
                64,
            )?
            else {
                // Read-only opens cannot create `.tshm`. If no shared
                // coordination file exists, degrade to the legacy read-only WAL
                // path rather than failing the open. This keeps binding-level
                // option plumbing advisory for readers while writable opens
                // still enforce the stricter multiprocess contract.
                return Ok(None);
            };
            Arc::new(authority)
        } else {
            Arc::new(MappedSharedWalCoordination::create_or_open(
                &self.io,
                std::path::Path::new(&path),
                64,
            )?)
        };
        let _ = self.shared_wal_coordination.set(authority.clone());
        Ok(Some(
            self.shared_wal_coordination
                .get()
                .cloned()
                .unwrap_or(authority),
        ))
    }

    /// Capture the committed WAL prefix and its existing durability proof in one generation.
    /// No I/O is performed. The shared log guard excludes restart, restore and commit publication
    /// while its header identity, committed end and completed-flush proof are read together.
    pub fn wal_prefix(&self) -> WalPrefix {
        let shared = self.shared_wal.current().read();
        let generation = shared.metadata.wal_header.lock().checkpoint_seq;
        let frame = shared.metadata.max_frame.load(Ordering::Acquire);
        let durable = *shared.runtime.durable.lock();
        WalPrefix {
            header: Arc::clone(&shared.metadata.wal_header),
            generation,
            frame,
            already_durable: !self.opts.enable_multiprocess_wal
                && durable.is_some_and(|(g, f)| g == generation && f >= frame),
        }
    }

    /// A successful file sync begun after `prefix` was captured covers that prefix. Stale or
    /// foreign prefixes are ignored, including a same-generation external restore. Multi-process
    /// WALs have an external authority and do not accept process-local durability proofs.
    pub fn wal_prefix_durable(&self, prefix: WalPrefix) {
        if self.opts.enable_multiprocess_wal {
            return;
        }
        let shared = self.shared_wal.current().read();
        if !Arc::ptr_eq(&shared.metadata.wal_header, &prefix.header)
            || shared.metadata.wal_header.lock().checkpoint_seq != prefix.generation
        {
            return;
        }
        let mut durable = shared.runtime.durable.lock();
        match *durable {
            Some((g, f)) if g == prefix.generation && f >= prefix.frame => {}
            _ => *durable = Some((prefix.generation, prefix.frame)),
        }
    }

    /// Amphora: where the log ends, as (its generation, the header's `checkpoint_seq`; its last
    /// committed frame). A flush of the log's file begun after this makes every frame up to it
    /// durable ([`Database::wal_durable`]).
    /// Amphora: the writer's switch to the log's free segment, between its transactions (decision
    /// 45, [`SharedLog::switch`]), the free segment's file cut to nothing first if longer than
    /// `cut_past` bytes: whether it switched. False while another connection writes or
    /// the other segment is still closing: the caller asks again at its next turn. It waits for
    /// no read.
    pub fn switch_log(&self, cut_past: Option<u64>) -> Result<bool> {
        if self.opts.enable_multiprocess_wal {
            return Err(LimboError::InvalidArgument(
                "a multi-process log can't switch segments yet: its shared memory doesn't publish \
                 the switch (decision 45)"
                    .into(),
            ));
        }
        if !self.shared_wal.write_lock.write() {
            return Ok(false);
        }
        let switched = self.flush_and_switch(cut_past);
        self.shared_wal.write_lock.unlock();
        self.shared_wal.released.notify();
        Ok(switched?.is_some())
    }

    /// The current segment's file flushed, then the switch: every frame of a closing segment is
    /// durable, so a pass copies all of it, unbounded by the current segment's flushes.
    fn flush_and_switch(
        &self,
        cut_past: Option<u64>,
    ) -> Result<Option<storage::log_segments::SegmentId>> {
        if self.shared_wal.segments.load().closing().is_some() {
            return Ok(None);
        }
        let file = self.shared_wal.current().read().runtime.file.clone();
        if let Some(file) = file {
            let synced = file.sync(
                crate::io::Completion::new_sync(|_| {}),
                crate::io::FileSyncType::DataSync,
            )?;
            self.io.wait_for_completion(synced)?;
        }
        self.shared_wal.switch(self.io.as_ref(), cut_past)
    }

    /// Amphora: the segment of the log the writer appends to (decision 45).
    pub fn wal_segment(&self) -> storage::log_segments::SegmentId {
        self.shared_wal.current_id()
    }

    pub fn wal_end(&self) -> (u32, u64) {
        let shared = self.shared_wal.current().read();
        let generation = shared.metadata.wal_header.lock().checkpoint_seq;
        (
            generation,
            shared.metadata.max_frame.load(Ordering::Acquire),
        )
    }

    /// Amphora: the log's last committed frame and how many of its frames are copied into the
    /// file (its backfill), as checkpoints of this process left them. Amphora's checkpointer
    /// copies only once enough wait (`docs/design/store.md`, decision 35).
    pub fn wal_backfill(&self) -> (u64, u64) {
        let shared = self.shared_wal.current().read();
        (
            shared.metadata.max_frame.load(Ordering::Acquire),
            shared.metadata.nbackfills.load(Ordering::Acquire),
        )
    }

    /// Amphora: every frame up to `frame` of the log's generation `generation` is durable: a flush
    /// begun after [`Database::wal_end`] answered them has ended. A checkpoint of frames within it
    /// flushes no log of its own. A generation the log has since left is ignored (a restart holds
    /// the shared log's write lock, so none happens between the check and the update).
    ///
    /// This legacy tuple cannot identify a replaced WAL. After the first external reload (or a
    /// generation-counter wrap), publications through this method are conservatively ignored for
    /// this database instance; checkpoints then perform their own required sync. Multi-process WAL
    /// publications are also ignored. Use [`Database::wal_prefix`] and
    /// [`Database::wal_prefix_durable`] for identity-safe proof publication after reload.
    pub fn wal_durable(&self, generation: u32, frame: u64) {
        let shared = self.shared_wal.current().read();
        if self.opts.enable_multiprocess_wal
            || !shared.runtime.legacy_durable_allowed
            || shared.metadata.wal_header.lock().checkpoint_seq != generation
        {
            return;
        }
        let mut durable = shared.runtime.durable.lock();
        match *durable {
            Some((g, f)) if g == generation && f >= frame => {}
            _ => *durable = Some((generation, frame)),
        }
    }

    /// Amphora: the format new generations of the log take (`storage/packed.rs`; Amphora's
    /// `docs/design/store.md`, decisions 16 and 37): a generation takes its format when its header
    /// is written, so the log's current one keeps its own. A format with records is refused for a
    /// log shared between processes, whose other processes couldn't read them; a packed one for a
    /// database that shares its pages.
    pub fn wal_format(&self, format: crate::storage::packed::LogFormat) -> Result<()> {
        #[cfg(host_shared_wal)]
        if format.holds_records() && self.shared_wal_coordination()?.is_some() {
            return Err(LimboError::InvalidArgument(
                "a log of records is read by one process; this one is shared between processes"
                    .into(),
            ));
        }
        if format == crate::storage::packed::LogFormat::Packed
            && self.db_file.shared_pages().is_some()
        {
            return Err(LimboError::InvalidArgument(
                "a packed log's pages aren't shared; this database shares its pages".into(),
            ));
        }
        self.shared_wal
            .current()
            .read()
            .runtime
            .format
            .store(format.file_format(), Ordering::Release);
        Ok(())
    }

    /// Amphora: shares the pages this database's connections read among them, `pages` at most
    /// (`storage/shared_pages.rs`, Amphora's `docs/design/store.md` decision 20). Refused under
    /// another process's authority (the file can change under it unseen), with MVCC, and with a
    /// packed log.
    pub fn share_pages(&self, pages: usize) -> Result<()> {
        self.share_pages_in(&Arc::new(crate::storage::shared_pages::SharedPages::new(
            pages,
        )))
    }

    /// Amphora: shares this database's pages in `pool`, with every other database that shares
    /// it: one budget for them all, each database's pages under keys of its own (decision 20).
    /// Refused as [`Database::share_pages`] is.
    pub fn share_pages_in(
        &self,
        pool: &Arc<crate::storage::shared_pages::SharedPages>,
    ) -> Result<()> {
        #[cfg(host_shared_wal)]
        if self.shared_wal_coordination()?.is_some() {
            return Err(LimboError::InvalidArgument(
                "shared pages are one process's; this database is shared between processes".into(),
            ));
        }
        if self.mv_store.load().is_some() {
            return Err(LimboError::InvalidArgument(
                "shared pages aren't kept for an MVCC database".into(),
            ));
        }
        // Packing asked for now, or the log's generation on disk packed (recovery reads it as it
        // was written; nothing asks again after a restart of the process). A mixed log's pages
        // are shared (decision 37).
        let packed = {
            let shared = self.shared_wal.current().read();
            let packed = |format| {
                crate::storage::packed::LogFormat::of(format)
                    == Some(crate::storage::packed::LogFormat::Packed)
            };
            packed(shared.runtime.format.load(Ordering::Acquire))
                || packed(shared.metadata.wal_header.lock().file_format)
        };
        if packed {
            return Err(LimboError::InvalidArgument(
                "a packed log's pages aren't shared".into(),
            ));
        }
        self.db_file
            .share_pages(crate::storage::shared_pages::SharedFile::join(pool.clone()))
    }

    /// Amphora: the shared pages' lookups and additions so far, if they're shared: the pool's, for
    /// every database that shares it.
    pub fn shared_page_counts(&self) -> Option<crate::storage::shared_pages::Counts> {
        self.db_file
            .shared_pages()
            .map(|shared| shared.pages().counts())
    }

    /// Amphora: the last frame of the log known durable in its current generation.
    /// After external reload or generation wrap, only identity-bearing prefix publications advance
    /// this hint; legacy tuple publications are ignored and checkpoints supply any missing sync.
    pub fn wal_durable_frame(&self) -> Option<u64> {
        let shared = self.shared_wal.current().read();
        let generation = shared.metadata.wal_header.lock().checkpoint_seq;
        let durable = *shared.runtime.durable.lock();
        durable.and_then(|(g, f)| if g == generation { Some(f) } else { None })
    }

    pub fn shared_wal_open_telemetry(&self) -> Result<SharedWalOpenTelemetry> {
        let shared_wal = self.shared_wal.current().read();
        let loaded_from_disk_scan = shared_wal
            .metadata
            .loaded_from_disk_scan
            .load(Ordering::Acquire);
        let reopened_max_frame = shared_wal.metadata.max_frame.load(Ordering::Acquire);
        let reopened_nbackfills = shared_wal.metadata.nbackfills.load(Ordering::Acquire);
        let reopened_checkpoint_seq = shared_wal.metadata.wal_header.lock().checkpoint_seq;
        drop(shared_wal);

        #[cfg(host_shared_wal)]
        let (coordination_open_mode, sanitized_backfill_proof_on_open) =
            if let Some(authority) = self.shared_wal_coordination()? {
                let mode = match authority.open_mode() {
                storage::shared_wal_coordination::SharedWalCoordinationOpenMode::Exclusive => {
                    SharedWalCoordinationOpenTelemetryMode::Exclusive
                }
                storage::shared_wal_coordination::SharedWalCoordinationOpenMode::MultiProcess => {
                    SharedWalCoordinationOpenTelemetryMode::MultiProcess
                }
            };
                (Some(mode), authority.sanitized_backfill_proof_on_open())
            } else {
                (None, false)
            };
        #[cfg(not(host_shared_wal))]
        let (coordination_open_mode, sanitized_backfill_proof_on_open) = (None, false);

        Ok(SharedWalOpenTelemetry {
            loaded_from_disk_scan,
            reopened_max_frame,
            reopened_nbackfills,
            reopened_checkpoint_seq,
            coordination_open_mode,
            sanitized_backfill_proof_on_open,
        })
    }

    #[cfg(feature = "simulator")]
    pub fn shared_wal_snapshot_for_testing(&self) -> Result<Option<SharedWalTestingSnapshot>> {
        #[cfg(host_shared_wal)]
        if let Some(authority) = self.shared_wal_coordination()? {
            let snapshot = authority.snapshot();
            return Ok(Some(SharedWalTestingSnapshot {
                max_frame: snapshot.max_frame,
                nbackfills: snapshot.nbackfills,
                checkpoint_seq: snapshot.checkpoint_seq,
                frame_index_overflowed: authority.frame_index_overflowed(),
            }));
        }

        Ok(None)
    }

    #[cfg(feature = "simulator")]
    pub fn shared_wal_find_frame_for_testing(&self, page_id: u64) -> Result<Option<u64>> {
        #[cfg(host_shared_wal)]
        if let Some(authority) = self.shared_wal_coordination()? {
            let snapshot = authority.snapshot();
            return Ok(authority.find_frame(page_id, 0, snapshot.max_frame, None));
        }

        Ok(None)
    }

    #[cfg(feature = "simulator")]
    pub fn local_wal_find_frame_for_testing(&self, page_id: u64) -> Result<Option<u64>> {
        let shared = self.shared_wal.current().read();
        let max_frame = shared.metadata.max_frame.load(Ordering::Acquire);
        let frame_cache = shared.runtime.frame_cache.read();
        Ok(frame_cache.get(&page_id).and_then(|frames| {
            frames
                .iter()
                .rfind(|&&frame_id| frame_id <= max_frame)
                .copied()
        }))
    }

    #[cfg(feature = "simulator")]
    pub fn local_wal_max_frame_for_testing(&self) -> Result<u64> {
        Ok(self
            .shared_wal
            .current()
            .read()
            .metadata
            .max_frame
            .load(Ordering::Acquire))
    }

    #[cfg(feature = "simulator")]
    pub fn clear_backfill_proof_for_testing(&self) -> Result<()> {
        #[cfg(host_shared_wal)]
        {
            let authority = self.shared_wal_coordination()?.ok_or_else(|| {
                LimboError::InternalError("shared WAL authority is unavailable".into())
            })?;
            authority.clear_backfill_proof();
            Ok(())
        }

        #[cfg(not(host_shared_wal))]
        {
            Err(LimboError::InternalError(
                "shared WAL authority is unavailable on this platform".into(),
            ))
        }
    }

    pub(crate) fn build_wal(
        &self,
        last_checksum_and_max_frame: ((u32, u32), u64),
        buffer_pool: Arc<BufferPool>,
    ) -> Result<Arc<dyn Wal>> {
        #[cfg(host_shared_wal)]
        if let Some(authority) = self.shared_wal_coordination()? {
            // Amphora: other processes couldn't read a packed or mixed log's records.
            let format = self
                .shared_wal
                .current()
                .read()
                .metadata
                .wal_header
                .lock()
                .file_format;
            if crate::storage::packed::LogFormat::of(format).is_some_and(|f| f.holds_records()) {
                return Err(LimboError::InvalidArgument(
                    "this log holds records, and a log of records is read by one process".into(),
                ));
            }
            return Ok(Arc::new(WalFile::new_with_shared_coordination(
                self.io.clone(),
                self.shared_wal.clone(),
                authority,
                last_checksum_and_max_frame,
                buffer_pool,
            )));
        }

        Ok(Arc::new(WalFile::new(
            self.io.clone(),
            self.shared_wal.clone(),
            last_checksum_and_max_frame,
            buffer_pool,
        )))
    }

    fn init_pager(
        &self,
        requested_page_size: Option<usize>,
        hdr_st: &mut DbHeaderReadState,
        encryption_key: Option<&EncryptionKey>,
        page_codec: Option<&Arc<dyn PageCodec>>,
    ) -> IOResultOr<Pager> {
        let cipher = self.encryption_cipher_mode.get();
        let encrypted = encryption_key.is_some() || !matches!(cipher, CipherMode::None);

        // For an existing (initialized) database, read the 512-byte header
        // once (non-blocking) and recover both the reserved-space byte and the
        // on-disk page size from it.
        let (header_reserved_bytes, header_page_size) = if self.initialized() {
            let header = return_if_io!(self.read_db_header_buf(hdr_st));
            if let Some(codec) = page_codec {
                // A codec may transform the magic bytes, so a short codec
                // file cannot be judged here.
                if let Some(err) = header.short_read() {
                    return Err(err.into());
                }
                let header_info = codec.bootstrap_page_info(header.bytes())?;
                let page_size_u32 = u32::try_from(header_info.page_size).map_err(|_| {
                    LimboError::InvalidArgument(format!(
                        "page codec reported invalid page size {}",
                        header_info.page_size
                    ))
                })?;
                let Some(page_size) = PageSize::new(page_size_u32) else {
                    return Err(LimboError::InvalidArgument(format!(
                        "page codec reported invalid page size {}",
                        header_info.page_size
                    ))
                    .into());
                };
                if !page_size.has_valid_reserved_space(header_info.reserved_space) {
                    return Err(LimboError::InvalidArgument(format!(
                        "page codec reported invalid reserved space {} for page size {}",
                        header_info.reserved_space,
                        page_size.get()
                    ))
                    .into());
                }
                (Some(header_info.reserved_space), Some(page_size))
            } else {
                let (reserved, page_size) = Self::plain_header_page_info(&header, encrypted)?;
                (Some(reserved), Some(page_size))
            }
        } else {
            (None, None)
        };
        if let (Some(codec), Some(reserved_bytes)) = (page_codec, header_reserved_bytes) {
            let required_reserved_bytes = codec.required_reserved_bytes();
            if reserved_bytes != required_reserved_bytes {
                return Err(LimboError::InvalidArgument(format!(
                    "page codec requires exactly {required_reserved_bytes} reserved bytes, but database provides {reserved_bytes}"
                )).into());
            }
        }

        let reserved_bytes = header_reserved_bytes.or_else(|| {
            if !matches!(cipher, CipherMode::None) {
                // For encryption, use the cipher's metadata size
                Some(cipher.metadata_size() as u8)
            } else {
                None
            }
        });
        let disable_checksums = if let Some(reserved_bytes) = reserved_bytes {
            // if the required reserved bytes for checksums is not present, disable checksums
            reserved_bytes != CHECKSUM_REQUIRED_RESERVED_BYTES
        } else {
            false
        };
        // Check if WAL is enabled
        let shared_wal = self.shared_wal.current().read();

        let page_size =
            self.determine_actual_page_size(&shared_wal, requested_page_size, header_page_size)?;

        let buffer_pool = self.buffer_pool.clone();
        if self.initialized() {
            buffer_pool.finalize_with_page_size(page_size.get() as usize)?;
        }

        let wal_enabled = shared_wal.metadata.enabled.load(Ordering::SeqCst);
        let last_checksum_and_max_frame = shared_wal.last_checksum_and_max_frame();
        drop(shared_wal);
        let pager_wal: Option<Arc<dyn Wal>> = if wal_enabled {
            Some(self.build_wal(last_checksum_and_max_frame, buffer_pool.clone())?)
        } else {
            None
        };

        let pager = Pager::new(
            self.db_file.clone(),
            pager_wal,
            self.io.clone(),
            PageCache::default(),
            buffer_pool,
            self.pager_state.clone(),
        )?;
        pager.set_learned_costs(self.learned_costs.clone());
        if self.initialized() {
            pager.set_page_size(page_size);
        }
        if let Some(reserved_bytes) = reserved_bytes {
            pager.set_reserved_space_bytes(reserved_bytes);
        }
        if disable_checksums {
            pager.reset_checksum_context();
        }
        // Amphora: opening an initialized database reads its page 1 from the file before the log
        // is attached (header validation), and after a crash during a checkpoint that page may be
        // torn there while the log holds it whole: its checksum is checked once the log is
        // attached (`Pager::set_wal`), when what opening read is dropped.
        if pager.wal.is_none() && self.initialized() {
            pager.defer_checksum();
        }

        Ok(IOResult::Done(pager))
    }

    /// Recovers the page size and reserved-space byte from a database header
    /// that is not handled by a page codec.
    ///
    /// The magic bytes are not judged here: an encrypted database carries the
    /// Turso prefix instead, and its key may arrive only after connect (PRAGMA
    /// key). The magic is checked at open validation, where the key is known.
    fn plain_header_page_info(header: &DbHeaderRead, encrypted: bool) -> Result<(u8, PageSize)> {
        // SQLite zero-fills a file shorter than a page before judging its
        // header (btree.c lockBtree), so a short file is judged the same way.
        let mut bytes = [0u8; DatabaseHeader::SIZE];
        let read = header.bytes();
        let len = read.len().min(bytes.len());
        bytes[..len].copy_from_slice(&read[..len]);

        let reserved = bytes[20];
        let page_size = PageSize::new_from_header_u16(u16::from_be_bytes([bytes[16], bytes[17]]));

        let judge_as_sqlite = !encrypted && !bytes.starts_with(TURSO_HEADER_PREFIX);
        if judge_as_sqlite
            && !page_size
                .as_ref()
                .is_ok_and(|page_size| page_size.has_valid_reserved_space(reserved))
        {
            return Err(LimboError::NotADB);
        }
        if let Some(err) = header.short_read() {
            return Err(err.into());
        }
        Ok((reserved, page_size?))
    }

    #[cfg(feature = "fs")]
    pub fn io_for_path(path: &str) -> Result<Arc<dyn IO>> {
        let io: Arc<dyn IO> = if is_memory_like(path.trim()) {
            Arc::new(MemoryIO::new())
        } else {
            Arc::new(PlatformIO::new()?)
        };
        Ok(io)
    }

    #[cfg(feature = "fs")]
    pub fn io_for_vfs<S: AsRef<str> + std::fmt::Display>(vfs: S) -> Result<Arc<dyn IO>> {
        if let Some(io) = crate::io::get_registered_io(vfs.as_ref()) {
            return Ok(io);
        }
        let vfsmods = ext::add_builtin_vfs_extensions(None)?;
        let io: Arc<dyn IO> = match vfsmods
            .iter()
            .find(|v| v.0 == vfs.as_ref())
            .map(|v| v.1.clone())
        {
            Some(vfs) => vfs,
            None => match vfs.as_ref() {
                "memory" => Arc::new(MemoryIO::new()),
                #[cfg(feature = "io_memory_yield")]
                "memory_yield" => Arc::new(MemoryYieldIO::new()),
                "syscall" => Arc::new(SyscallIO::new()?),
                #[cfg(all(target_os = "linux", feature = "io_uring", not(miri)))]
                "io_uring" => Arc::new(UringIO::new()?),
                #[cfg(all(target_os = "windows", feature = "experimental_win_iocp", not(miri)))]
                "experimental_win_iocp" => Arc::new(WindowsIOCP::new()?),

                other => {
                    return Err(LimboError::InvalidArgument(format!("no such VFS: {other}")));
                }
            },
        };
        Ok(io)
    }

    /// Open a new database file with optionally specifying a VFS without an existing database
    /// connection and symbol table to register extensions.
    #[cfg(feature = "fs")]
    pub fn open_new<S>(
        path: &str,
        vfs: Option<S>,
        flags: OpenFlags,
        opts: DatabaseOpts,
        encryption_opts: Option<EncryptionOpts>,
        dialect: Arc<dyn Dialect>,
    ) -> Result<(Arc<dyn IO>, Arc<Database>)>
    where
        S: AsRef<str> + std::fmt::Display,
    {
        let io = vfs
            .map(|vfs| Self::io_for_vfs(vfs))
            .or_else(|| Some(Self::io_for_path(path)))
            .transpose()?
            .unwrap();
        let db =
            Self::open_file_with_flags(io.clone(), path, flags, opts, encryption_opts, dialect)?;
        Ok((io, db))
    }

    #[inline]
    pub(crate) fn initialized(&self) -> bool {
        self.pager_state.initialized()
    }

    pub(crate) fn can_load_extensions(&self) -> bool {
        self.opts.enable_load_extension
    }

    #[inline]
    pub(crate) fn bump_schema_epoch(&self) -> u64 {
        self.schema_epoch
            .fetch_add(1, Ordering::Release)
            .wrapping_add(1)
    }

    #[inline]
    pub(crate) fn with_schema_mut<T>(&self, f: impl FnOnce(&mut Schema) -> Result<T>) -> Result<T> {
        let mut schema_ref = self.schema.lock();
        let schema = Schema::try_make_mut(&mut schema_ref)?;
        let out = f(schema);
        self.bump_schema_epoch();
        out
    }

    pub(crate) fn replace_schema(&self, schema: Arc<Schema>) {
        let mut guard = self.schema.lock();
        *guard = schema;
        self.bump_schema_epoch();
    }

    /// Register an `InternalVirtualTable` into this database's catalog. The
    /// table is visible to connections opened after this call and is queryable
    /// like any other table.
    ///
    /// Intended for callers that want to surface state as a queryable table
    /// without going through `CREATE VIRTUAL TABLE` — for example, extensions
    /// contributing metadata tables or alternative-dialect catalogs.
    ///
    /// Call before opening connections. Connections that already exist will
    /// not pick up the new table unless they re-read the shared schema (e.g.
    /// via the usual schema-change path).
    pub fn register_internal_vtab<T>(&self, table: T) -> Result<String>
    where
        T: InternalVirtualTable + 'static,
    {
        self.with_schema_mut(|schema| schema.register_internal_vtab(table))
    }

    /// The SQL dialect this database runs under.
    pub fn dialect(&self) -> Arc<dyn Dialect> {
        self.dialect.read().clone()
    }

    /// Returns an independent dialect instance for an attached or temporary database so per-
    /// database mutable state (such as `PostgresDialect`'s `next_oid` counter) is never shared
    /// with or rewound by another file.
    pub(crate) fn dialect_for_child(&self) -> Arc<dyn Dialect> {
        let dialect = self.dialect();
        dialect.fresh_instance().unwrap_or(dialect)
    }

    /// Replaces this database's SQL dialect in place when installing a raw replicated or restored
    /// image whose page-1 header or catalog belongs to another dialect (e.g. an empty follower
    /// receiving a PostgreSQL baseline seed).
    pub fn set_dialect(&self, dialect: Arc<dyn Dialect>) {
        *self.dialect.write() = dialect;
    }

    pub fn register_virtual_table(&self, table: Arc<VirtualTable>) -> Result<String> {
        let name = table.name.clone();
        self.with_schema_mut(|schema| schema.add_virtual_table(table))?;
        Ok(name)
    }

    pub(crate) fn clone_schema(&self) -> Arc<Schema> {
        let schema = self.schema.lock();
        schema.clone()
    }

    pub(crate) fn update_schema_if_newer(&self, another: Arc<Schema>) {
        let mut schema = self.schema.lock();
        if schema.schema_version < another.schema_version {
            tracing::debug!(
                "DB schema is outdated: {} < {}",
                schema.schema_version,
                another.schema_version
            );
            *schema = another;
            self.bump_schema_epoch();
        } else {
            tracing::debug!(
                "DB schema is up to date: {} >= {}",
                schema.schema_version,
                another.schema_version
            );
        }
    }

    pub fn get_mv_store(&self) -> impl Deref<Target = Option<Arc<MvStore>>> {
        self.mv_store.load()
    }

    pub fn experimental_views_enabled(&self) -> bool {
        self.opts.enable_views
    }

    pub fn experimental_index_method_enabled(&self) -> bool {
        self.opts.enable_index_method
    }

    pub fn experimental_custom_types_enabled(&self) -> bool {
        self.opts.enable_custom_types || self.dialect().requires_custom_types()
    }

    pub fn experimental_encryption_enabled(&self) -> bool {
        self.opts.enable_encryption
    }

    pub fn experimental_autovacuum_enabled(&self) -> bool {
        self.opts.enable_autovacuum
    }

    pub fn experimental_vacuum_enabled(&self) -> bool {
        self.opts.enable_vacuum
    }

    pub fn experimental_mvcc_passive_checkpoint_enabled(&self) -> bool {
        self.opts.enable_experimental_mvcc_passive_checkpoint
    }

    pub fn experimental_attach_enabled(&self) -> bool {
        self.opts.enable_attach
    }

    pub fn experimental_generated_columns_enabled(&self) -> bool {
        self.opts.enable_generated_columns
    }

    pub fn experimental_multiprocess_wal_enabled(&self) -> bool {
        self.opts.enable_multiprocess_wal
    }

    pub fn experimental_without_rowid_enabled(&self) -> bool {
        self.opts.enable_without_rowid
    }

    /// check if database is currently in MVCC mode
    pub fn mvcc_enabled(&self) -> bool {
        self.mv_store.load().is_some()
    }

    #[cfg(feature = "test_helper")]
    pub fn set_pending_byte(val: u32) {
        Pager::set_pending_byte(val);
    }

    #[cfg(feature = "test_helper")]
    pub fn get_pending_byte() -> u32 {
        Pager::get_pending_byte()
    }
}

pub(crate) struct AttachedDatabase {
    pub(crate) db: Arc<Database>,
    pub(crate) pager: Arc<Pager>,
    pub(crate) sync_mode: SyncMode,
    /// Last `SharedLog::restore_epoch` observed by this connection for this attached database.
    pub(crate) last_restore_epoch: AtomicU64,
}

// Optimized for fast get() operations and supports unlimited attached databases.
pub(crate) struct DatabaseCatalog {
    pub(crate) name_to_index: HashMap<String, usize>,
    allocated: Vec<u64>,
    pub(crate) index_to_data: HashMap<usize, AttachedDatabase>,
}

#[allow(unused)]
impl DatabaseCatalog {
    pub(crate) fn new() -> Self {
        Self {
            name_to_index: HashMap::default(),
            index_to_data: HashMap::default(),
            allocated: vec![3], // 0 | 1, as those are reserved for main and temp
        }
    }

    pub(crate) fn get_database_by_index(&self, index: usize) -> Option<Arc<Database>> {
        self.index_to_data.get(&index).map(|entry| entry.db.clone())
    }

    pub(crate) fn get_name_by_index(&self, index: usize) -> Option<String> {
        self.name_to_index
            .iter()
            .find(|(_, &idx)| idx == index)
            .map(|(name, _)| name.clone())
    }

    pub(crate) fn get_database_by_name(&self, s: &str) -> Option<(usize, Arc<Database>)> {
        match self.name_to_index.get(s) {
            None => None,
            Some(idx) => self
                .index_to_data
                .get(idx)
                .map(|entry| (*idx, entry.db.clone())),
        }
    }

    pub(crate) fn get_pager_by_index(&self, idx: &usize) -> Option<Arc<Pager>> {
        self.index_to_data.get(idx).map(|entry| entry.pager.clone())
    }

    fn add(&mut self, s: &str) -> usize {
        turso_assert!(
            !self.name_to_index.contains_key(s),
            "lib: database name already exists in catalog",
            { "name": s }
        );

        let index = self.allocate_index();
        self.name_to_index.insert(s.to_string(), index);
        index
    }

    pub(crate) fn insert(&mut self, s: &str, db: Arc<Database>, pager: Arc<Pager>) -> usize {
        let idx = self.add(s);
        let initial_restore_epoch = db.shared_wal.restore_epoch();
        self.index_to_data.insert(
            idx,
            AttachedDatabase {
                db,
                pager,
                sync_mode: SyncMode::Full,
                last_restore_epoch: AtomicU64::new(initial_restore_epoch),
            },
        );
        idx
    }

    pub(crate) fn remove(&mut self, s: &str) -> Option<usize> {
        if let Some(index) = self.name_to_index.remove(s) {
            // Should be impossible to remove main or temp.
            turso_assert_greater_than_or_equal!(index, 2);
            self.deallocate_index(index);
            self.index_to_data.remove(&index);
            Some(index)
        } else {
            None
        }
    }

    #[inline(always)]
    fn deallocate_index(&mut self, index: usize) {
        let word_idx = index / 64;
        let bit_idx = index % 64;

        if word_idx < self.allocated.len() {
            self.allocated[word_idx] &= !(1u64 << bit_idx);
        }
    }

    fn allocate_index(&mut self) -> usize {
        for word_idx in 0..self.allocated.len() {
            let word = self.allocated[word_idx];

            if word != u64::MAX {
                let free_bit = Self::find_first_zero_bit(word);
                let index = word_idx * 64 + free_bit;

                self.allocated[word_idx] |= 1u64 << free_bit;

                return index;
            }
        }

        // Need to expand bitmap
        let word_idx = self.allocated.len();
        self.allocated.push(1u64); // Mark first bit as allocated
        word_idx * 64
    }

    #[inline(always)]
    fn find_first_zero_bit(word: u64) -> usize {
        // Invert to find first zero as first one
        let inverted = !word;

        // Use trailing zeros count (compiles to single instruction on most CPUs)
        inverted.trailing_zeros() as usize
    }
}

#[cfg(test)]
mod database_tests {
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };

    use super::{is_memory_like, Database, InitState};
    use crate::storage::encryption::EncryptionKey;
    use crate::storage::page_transform::{
        PageCodec, PageCodecContext, PageCodecHeaderInfo, PageCodecId, PageLocation,
    };
    use crate::storage::sqlite3_ondisk::DatabaseHeader;
    use crate::{
        storage::database::DatabaseFile, CompletionError, ConnectOpts, Connection, DatabaseOpts,
        DatabaseStorage, EncryptionOpts, IOResult, LimboError, OpenDbAsyncState, OpenFlags,
        OpenOptions, PlatformIO, SqliteDialect, IO,
    };

    /// AC-T1.1.1: virtual files share within their backend, never across independent
    /// backends, and their registry ownership ends with the final Database alias.
    #[cfg(feature = "fs")]
    #[test]
    fn virtual_filesystems_isolate_same_paths_and_release_registry_ownership() {
        fn check(left: Arc<dyn IO>, right: Arc<dyn IO>) {
            let path = "registry-scope-behavior.db";
            let open = |io: Arc<dyn IO>| {
                Database::open(io, path, OpenOptions::new(Arc::new(SqliteDialect))).unwrap()
            };
            let first = open(left.clone());
            let first_conn = first.connect(ConnectOpts::new()).unwrap();
            first_conn
                .execute("CREATE TABLE probe(value TEXT)")
                .unwrap();
            first_conn
                .execute("INSERT INTO probe VALUES ('left')")
                .unwrap();
            let second = open(right);
            let second_conn = second.connect(ConnectOpts::new()).unwrap();
            second_conn
                .execute("CREATE TABLE probe(value TEXT)")
                .unwrap();
            second_conn
                .execute("INSERT INTO probe VALUES ('right')")
                .unwrap();
            let alias = open(left.clone());
            let alias_conn = alias.connect(ConnectOpts::new()).unwrap();
            let value = |connection: &Arc<Connection>| {
                connection
                    .prepare("SELECT value FROM probe ORDER BY rowid")
                    .unwrap()
                    .run_collect_rows()
                    .unwrap()
                    .iter()
                    .map(|row| row.iter().map(ToString::to_string).collect::<Vec<_>>())
                    .collect::<Vec<_>>()
            };
            assert_eq!(value(&alias_conn), vec![vec!["left"]]);
            assert_eq!(value(&second_conn), vec![vec!["right"]]);
            let old_key =
                super::DatabaseKey::File(left.file_id(path).unwrap(), left.registry_scope());
            alias_conn
                .execute("INSERT INTO probe VALUES ('shared')")
                .unwrap();
            assert_eq!(value(&first_conn), vec![vec!["left"], vec!["shared"]]);
            // Unlinking replaces a virtual file's incarnation while existing handles still
            // own the preceding bytes, like unlink plus recreate on a native filesystem.
            left.remove_file(path).unwrap();
            left.remove_file(&format!("{path}-wal")).unwrap();
            assert!(left.file_id(path).is_err());
            let replacement = open(left.clone());
            let replacement_conn = replacement.connect(ConnectOpts::new()).unwrap();
            replacement_conn
                .execute("CREATE TABLE probe(value TEXT)")
                .unwrap();
            replacement_conn
                .execute("INSERT INTO probe VALUES ('new')")
                .unwrap();
            let replacement_alias = open(left.clone());
            let replacement_alias_conn = replacement_alias.connect(ConnectOpts::new()).unwrap();
            assert_eq!(value(&replacement_alias_conn), vec![vec!["new"]]);
            assert_eq!(value(&first_conn), vec![vec!["left"], vec!["shared"]]);
            let new_key =
                super::DatabaseKey::File(left.file_id(path).unwrap(), left.registry_scope());
            drop(first_conn);
            drop(first);
            assert!(
                super::DATABASE_MANAGER.lock().contains_key(&old_key),
                "an alias still owns the registry entry"
            );
            drop(alias_conn);
            drop(alias);
            assert!(
                !super::DATABASE_MANAGER.lock().contains_key(&old_key),
                "closed virtual files cannot retain registry entries"
            );
            assert!(
                super::DATABASE_MANAGER.lock().contains_key(&new_key),
                "old cleanup cannot remove the replacement"
            );
            drop(replacement_conn);
            drop(replacement);
            assert!(
                super::DATABASE_MANAGER.lock().contains_key(&new_key),
                "the replacement alias still owns its entry"
            );
            drop(replacement_alias_conn);
            drop(replacement_alias);
            assert!(
                !super::DATABASE_MANAGER.lock().contains_key(&new_key),
                "the final alias releases its entry"
            );
        }
        check(
            Arc::new(crate::MemoryIO::new()),
            Arc::new(crate::MemoryIO::new()),
        );
        #[cfg(feature = "io_memory_yield")]
        {
            check(
                Arc::new(crate::MemoryYieldIO::new()),
                Arc::new(crate::MemoryYieldIO::new()),
            );
            check(
                Arc::new(crate::MemoryIO::new()),
                Arc::new(crate::MemoryYieldIO::new()),
            );
        }
    }

    #[test]
    fn memory_path_classifies_named_memory_databases() {
        assert!(is_memory_like(":memory:"));
        assert!(is_memory_like(":memory:sync-draft"));
        assert!(is_memory_like("file::memory:?cache=shared"));
        assert!(is_memory_like(""));
        assert!(!is_memory_like("memory.db"));
        assert!(!is_memory_like("file:memory.db"));
    }

    #[cfg(feature = "fs")]
    #[test]
    fn io_for_path_uses_memory_io_for_named_memory_database() {
        let path = format!(":memory:named-io-selection-{}", std::process::id());
        assert!(std::fs::metadata(&path).is_err());

        let io = Database::io_for_path(&path).unwrap();

        io.open_file(&path, OpenFlags::Create, false).unwrap();
        assert!(io.file_id(&path).is_ok());
        assert!(std::fs::metadata(&path).is_err());
    }

    #[derive(Debug)]
    struct IdentityPageCodec;

    impl PageCodec for IdentityPageCodec {
        fn codec_id(&self) -> PageCodecId {
            PageCodecId::new(*b"identity-codec--")
        }

        fn required_reserved_bytes(&self) -> u8 {
            0
        }

        fn encode_page(
            &self,
            _context: PageCodecContext,
            input: &[u8],
            output: &mut [u8],
        ) -> crate::Result<()> {
            output.copy_from_slice(input);
            Ok(())
        }

        fn decode_page(
            &self,
            _context: PageCodecContext,
            input: &[u8],
            output: &mut [u8],
        ) -> crate::Result<()> {
            output.copy_from_slice(input);
            Ok(())
        }
    }

    #[derive(Debug)]
    struct InvalidHeaderPageCodec {
        page_size: usize,
        reserved_space: u8,
    }

    impl PageCodec for InvalidHeaderPageCodec {
        fn codec_id(&self) -> PageCodecId {
            PageCodecId::new(*b"invalid-header-c")
        }

        fn bootstrap_page_info(
            &self,
            _raw_page1_prefix: &[u8],
        ) -> crate::Result<PageCodecHeaderInfo> {
            Ok(PageCodecHeaderInfo {
                page_size: self.page_size,
                reserved_space: self.reserved_space,
            })
        }

        fn required_reserved_bytes(&self) -> u8 {
            0
        }

        fn encode_page(
            &self,
            _context: PageCodecContext,
            input: &[u8],
            output: &mut [u8],
        ) -> crate::Result<()> {
            output.copy_from_slice(input);
            Ok(())
        }

        fn decode_page(
            &self,
            _context: PageCodecContext,
            input: &[u8],
            output: &mut [u8],
        ) -> crate::Result<()> {
            output.copy_from_slice(input);
            Ok(())
        }
    }

    #[derive(Debug)]
    struct XorPageCodec {
        mask: u8,
        reserved_bytes: u8,
    }

    impl XorPageCodec {
        fn transform(&self, page: &[u8], output: &mut [u8]) {
            for (input, output) in page.iter().zip(output) {
                *output = input ^ self.mask;
            }
        }
    }

    impl PageCodec for XorPageCodec {
        fn codec_id(&self) -> PageCodecId {
            let mut id = *b"xor-page-codec--";
            id[15] = self.mask;
            id[14] = self.reserved_bytes;
            PageCodecId::new(id)
        }

        fn bootstrap_page_info(
            &self,
            raw_page1_prefix: &[u8],
        ) -> crate::Result<PageCodecHeaderInfo> {
            if raw_page1_prefix.len() < 21 {
                return Err(LimboError::NotADB);
            }

            let decoded_magic = raw_page1_prefix[..16]
                .iter()
                .map(|byte| byte ^ self.mask)
                .collect::<Vec<_>>();
            if decoded_magic.as_slice() != b"SQLite format 3\0" {
                return Err(LimboError::NotADB);
            }

            let ps_raw = u16::from_be_bytes([
                raw_page1_prefix[16] ^ self.mask,
                raw_page1_prefix[17] ^ self.mask,
            ]);
            let page_size = if ps_raw == 1 { 65536 } else { ps_raw as usize };
            Ok(PageCodecHeaderInfo {
                page_size,
                reserved_space: raw_page1_prefix[20] ^ self.mask,
            })
        }

        fn required_reserved_bytes(&self) -> u8 {
            self.reserved_bytes
        }

        fn encode_page(
            &self,
            _context: PageCodecContext,
            input: &[u8],
            output: &mut [u8],
        ) -> crate::Result<()> {
            self.transform(input, output);
            Ok(())
        }

        fn decode_page(
            &self,
            _context: PageCodecContext,
            input: &[u8],
            output: &mut [u8],
        ) -> crate::Result<()> {
            self.transform(input, output);
            Ok(())
        }
    }

    #[derive(Debug)]
    struct CountingPageCodec {
        inner: XorPageCodec,
        database_page1_decodes: Arc<AtomicUsize>,
    }

    impl PageCodec for CountingPageCodec {
        fn codec_id(&self) -> PageCodecId {
            self.inner.codec_id()
        }

        fn bootstrap_page_info(
            &self,
            raw_page1_prefix: &[u8],
        ) -> crate::Result<PageCodecHeaderInfo> {
            self.inner.bootstrap_page_info(raw_page1_prefix)
        }

        fn required_reserved_bytes(&self) -> u8 {
            self.inner.required_reserved_bytes()
        }

        fn encode_page(
            &self,
            context: PageCodecContext,
            input: &[u8],
            output: &mut [u8],
        ) -> crate::Result<()> {
            self.inner.encode_page(context, input, output)
        }

        fn decode_page(
            &self,
            context: PageCodecContext,
            input: &[u8],
            output: &mut [u8],
        ) -> crate::Result<()> {
            if context.page_no == DatabaseHeader::PAGE_ID as u32
                && context.location == PageLocation::Database
            {
                self.database_page1_decodes.fetch_add(1, Ordering::Relaxed);
            }
            self.inner.decode_page(context, input, output)
        }
    }

    #[derive(Debug)]
    struct FailOncePageCodec {
        inner: XorPageCodec,
        fail_decode: Arc<AtomicBool>,
        fail_context: PageCodecContext,
    }

    impl PageCodec for FailOncePageCodec {
        fn codec_id(&self) -> PageCodecId {
            self.inner.codec_id()
        }

        fn bootstrap_page_info(
            &self,
            raw_page1_prefix: &[u8],
        ) -> crate::Result<PageCodecHeaderInfo> {
            self.inner.bootstrap_page_info(raw_page1_prefix)
        }

        fn required_reserved_bytes(&self) -> u8 {
            self.inner.required_reserved_bytes()
        }

        fn encode_page(
            &self,
            context: PageCodecContext,
            input: &[u8],
            output: &mut [u8],
        ) -> crate::Result<()> {
            self.inner.encode_page(context, input, output)
        }

        fn decode_page(
            &self,
            context: PageCodecContext,
            input: &[u8],
            output: &mut [u8],
        ) -> crate::Result<()> {
            if context == self.fail_context && self.fail_decode.swap(false, Ordering::Relaxed) {
                return Err(LimboError::InternalError(
                    "injected page decode failure".into(),
                ));
            }
            self.inner.decode_page(context, input, output)
        }
    }

    #[derive(Debug, Default)]
    struct PageCodecFailureSwitches {
        wal_encode: AtomicBool,
        wal_decode: AtomicBool,
        database_encode: AtomicBool,
        database_decode_page: AtomicUsize,
    }

    #[derive(Debug)]
    struct FailOnceTransformPageCodec {
        inner: XorPageCodec,
        failures: Arc<PageCodecFailureSwitches>,
    }

    impl PageCodec for FailOnceTransformPageCodec {
        fn codec_id(&self) -> PageCodecId {
            self.inner.codec_id()
        }

        fn bootstrap_page_info(
            &self,
            raw_page1_prefix: &[u8],
        ) -> crate::Result<PageCodecHeaderInfo> {
            self.inner.bootstrap_page_info(raw_page1_prefix)
        }

        fn required_reserved_bytes(&self) -> u8 {
            self.inner.required_reserved_bytes()
        }

        fn encode_page(
            &self,
            context: PageCodecContext,
            input: &[u8],
            output: &mut [u8],
        ) -> crate::Result<()> {
            let failure = match context.location {
                PageLocation::Database => &self.failures.database_encode,
                PageLocation::Wal => &self.failures.wal_encode,
            };
            if failure.swap(false, Ordering::Relaxed) {
                return Err(LimboError::InternalError(format!(
                    "injected {:?} encode failure",
                    context.location
                )));
            }
            self.inner.encode_page(context, input, output)
        }

        fn decode_page(
            &self,
            context: PageCodecContext,
            input: &[u8],
            output: &mut [u8],
        ) -> crate::Result<()> {
            let fail = match context.location {
                PageLocation::Database => self
                    .failures
                    .database_decode_page
                    .compare_exchange(
                        context.page_no as usize,
                        0,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    )
                    .is_ok(),
                PageLocation::Wal => self.failures.wal_decode.swap(false, Ordering::Relaxed),
            };
            if fail {
                if let (Some(input), Some(output)) = (input.first(), output.first_mut()) {
                    *output = *input;
                }
                return Err(LimboError::InternalError(format!(
                    "injected {:?} decode failure",
                    context.location
                )));
            }
            self.inner.decode_page(context, input, output)
        }
    }

    #[derive(Debug)]
    struct TaggedPageCodec;

    impl TaggedPageCodec {
        const TAG_BYTES: usize = std::mem::size_of::<u64>();

        // This is deliberately a small test-only integrity tag, not a
        // cryptographic authenticator.
        fn tag(context: PageCodecContext, data: &[u8]) -> u64 {
            let location = match context.location {
                PageLocation::Database => 0x4442_5041_4745_0000,
                PageLocation::Wal => 0x5741_4c50_4147_4500,
            };
            data.iter()
                .fold(location ^ u64::from(context.page_no), |tag, byte| {
                    tag.rotate_left(5) ^ u64::from(*byte)
                })
        }
    }

    impl PageCodec for TaggedPageCodec {
        fn codec_id(&self) -> PageCodecId {
            PageCodecId::new(*b"tagged-page-----")
        }

        fn required_reserved_bytes(&self) -> u8 {
            Self::TAG_BYTES as u8
        }

        fn encode_page(
            &self,
            context: PageCodecContext,
            input: &[u8],
            output: &mut [u8],
        ) -> crate::Result<()> {
            if input.len() != output.len() || input.len() < Self::TAG_BYTES {
                return Err(LimboError::InvalidArgument(
                    "tagged codec requires equal page-sized buffers".into(),
                ));
            }
            let data_len = input.len() - Self::TAG_BYTES;
            output[..data_len].copy_from_slice(&input[..data_len]);
            output[data_len..]
                .copy_from_slice(&Self::tag(context, &input[..data_len]).to_le_bytes());
            Ok(())
        }

        fn decode_page(
            &self,
            context: PageCodecContext,
            input: &[u8],
            output: &mut [u8],
        ) -> crate::Result<()> {
            if input.len() != output.len() || input.len() < Self::TAG_BYTES {
                return Err(LimboError::InvalidArgument(
                    "tagged codec requires equal page-sized buffers".into(),
                ));
            }
            let data_len = input.len() - Self::TAG_BYTES;
            let stored = u64::from_le_bytes(
                input[data_len..]
                    .try_into()
                    .expect("tag length was checked above"),
            );
            let expected = Self::tag(context, &input[..data_len]);
            if stored != expected {
                return Err(LimboError::Corrupt(format!(
                    "invalid page tag for page {} at {:?}",
                    context.page_no, context.location
                )));
            }
            output[..data_len].copy_from_slice(&input[..data_len]);
            output[data_len..].fill(0);
            Ok(())
        }
    }

    #[derive(Debug)]
    struct LocationPageCodec;

    impl LocationPageCodec {
        const MASK: u8 = 0x6d;

        fn byte_key(page_no: u32, location: PageLocation, offset: usize) -> u8 {
            // Persisted transforms must not depend on the host pointer width.
            let page_id_byte = (page_no as u64).to_le_bytes()[offset % std::mem::size_of::<u64>()];
            let page_id_rotation =
                ((offset / std::mem::size_of::<u64>()) % (u8::BITS as usize)) as u32;
            Self::MASK
                ^ page_id_byte.rotate_left(page_id_rotation)
                ^ match location {
                    PageLocation::Database => 0,
                    PageLocation::Wal => 0xa5,
                }
        }

        fn transform(input: &[u8], output: &mut [u8], context: PageCodecContext) {
            for (offset, (input, output)) in input.iter().zip(output).enumerate() {
                *output = input ^ Self::byte_key(context.page_no, context.location, offset);
            }
        }
    }

    impl PageCodec for LocationPageCodec {
        fn codec_id(&self) -> PageCodecId {
            PageCodecId::new(*b"location-page-co")
        }

        fn bootstrap_page_info(
            &self,
            raw_page1_prefix: &[u8],
        ) -> crate::Result<PageCodecHeaderInfo> {
            if raw_page1_prefix.len() < 21 {
                return Err(LimboError::NotADB);
            }
            if !raw_page1_prefix[..16]
                .iter()
                .enumerate()
                .zip(b"SQLite format 3\0")
                .all(|((offset, encoded), expected)| {
                    *encoded
                        ^ Self::byte_key(
                            DatabaseHeader::PAGE_ID as u32,
                            PageLocation::Database,
                            offset,
                        )
                        == *expected
                })
            {
                return Err(LimboError::NotADB);
            }

            let page_size = u16::from_be_bytes([
                raw_page1_prefix[16]
                    ^ Self::byte_key(DatabaseHeader::PAGE_ID as u32, PageLocation::Database, 16),
                raw_page1_prefix[17]
                    ^ Self::byte_key(DatabaseHeader::PAGE_ID as u32, PageLocation::Database, 17),
            ]);
            Ok(PageCodecHeaderInfo {
                page_size: if page_size == 1 {
                    65_536
                } else {
                    page_size as usize
                },
                reserved_space: raw_page1_prefix[20]
                    ^ Self::byte_key(DatabaseHeader::PAGE_ID as u32, PageLocation::Database, 20),
            })
        }

        fn required_reserved_bytes(&self) -> u8 {
            1
        }

        fn encode_page(
            &self,
            context: PageCodecContext,
            input: &[u8],
            output: &mut [u8],
        ) -> crate::Result<()> {
            Self::transform(input, output, context);
            Ok(())
        }

        fn decode_page(
            &self,
            context: PageCodecContext,
            input: &[u8],
            output: &mut [u8],
        ) -> crate::Result<()> {
            Self::transform(input, output, context);
            Ok(())
        }
    }

    #[test]
    fn location_page_codec_uses_full_page_id() {
        let input = vec![0; 512];
        let mut page_one = vec![0; input.len()];
        let mut page_two_fifty_seven = vec![0; input.len()];
        LocationPageCodec::transform(
            &input,
            &mut page_one,
            PageCodecContext::new(1, PageLocation::Database),
        );
        LocationPageCodec::transform(
            &input,
            &mut page_two_fifty_seven,
            PageCodecContext::new(257, PageLocation::Database),
        );

        assert_ne!(page_one, page_two_fifty_seven);

        let mut decoded = vec![0; input.len()];
        LocationPageCodec::transform(
            &page_two_fifty_seven,
            &mut decoded,
            PageCodecContext::new(257, PageLocation::Database),
        );
        assert_eq!(decoded, input);
    }

    #[derive(Debug)]
    struct XorDefaultBootstrapPageCodec {
        mask: u8,
        reserved_bytes: u8,
    }

    impl XorDefaultBootstrapPageCodec {
        fn transform(&self, page: &[u8], output: &mut [u8]) {
            for (input, output) in page.iter().zip(output) {
                *output = input ^ self.mask;
            }
        }
    }

    impl PageCodec for XorDefaultBootstrapPageCodec {
        fn codec_id(&self) -> PageCodecId {
            let mut id = *b"xor-no-probe----";
            id[15] = self.mask;
            id[14] = self.reserved_bytes;
            PageCodecId::new(id)
        }

        fn required_reserved_bytes(&self) -> u8 {
            self.reserved_bytes
        }

        fn encode_page(
            &self,
            _context: PageCodecContext,
            input: &[u8],
            output: &mut [u8],
        ) -> crate::Result<()> {
            self.transform(input, output);
            Ok(())
        }

        fn decode_page(
            &self,
            _context: PageCodecContext,
            input: &[u8],
            output: &mut [u8],
        ) -> crate::Result<()> {
            self.transform(input, output);
            Ok(())
        }
    }

    #[cfg(feature = "fs")]
    fn open_with_page_codec_result(
        io: Arc<dyn IO>,
        path: &str,
        codec: Arc<dyn PageCodec>,
    ) -> crate::Result<Arc<Database>> {
        open_with_page_codec_with_opts_result(io, path, codec, DatabaseOpts::new())
    }

    #[cfg(feature = "fs")]
    fn open_with_page_codec_with_opts_result(
        io: Arc<dyn IO>,
        path: &str,
        codec: Arc<dyn PageCodec>,
        opts: DatabaseOpts,
    ) -> crate::Result<Arc<Database>> {
        let file = io.open_file(path, OpenFlags::Create, true).unwrap();
        let db_file: Arc<dyn DatabaseStorage> = Arc::new(DatabaseFile::new(file));
        let mut state = OpenDbAsyncState::new();
        let options = OpenOptions::new(Arc::new(SqliteDialect))
            .storage(db_file)
            .flags(OpenFlags::Create)
            .db_opts(opts)
            .page_codec(codec);

        loop {
            match Database::open_async(&mut state, io.clone(), path, &options)? {
                IOResult::Done(db) => return Ok(db),
                IOResult::IO(completion) => completion.wait(&*io)?,
            }
        }
    }

    #[cfg(feature = "fs")]
    fn open_with_page_codec(
        io: Arc<dyn IO>,
        path: &str,
        codec: Arc<dyn PageCodec>,
    ) -> Arc<Database> {
        open_with_page_codec_result(io, path, codec).unwrap()
    }

    #[cfg(feature = "fs")]
    fn count_test_rows(conn: &Arc<Connection>) -> i64 {
        let mut stmt = conn.prepare("select count(*) from test").unwrap();
        let mut count = 0;
        stmt.run_with_row_callback(|row| {
            count = row.get(0).unwrap();
            Ok(())
        })
        .unwrap();
        count
    }

    fn try_count_test_rows(conn: &Arc<Connection>) -> crate::Result<i64> {
        let mut stmt = conn.prepare("select count(*) from test")?;
        let mut count = 0;
        stmt.run_with_row_callback(|row| {
            count = row.get(0)?;
            Ok(())
        })?;
        Ok(count)
    }

    #[cfg(feature = "fs")]
    fn passive_checkpoint_busy(conn: &Arc<Connection>) -> crate::Result<i64> {
        let mut stmt = conn.prepare("PRAGMA wal_checkpoint(PASSIVE)")?;
        let mut busy = None;
        stmt.run_with_row_callback(|row| {
            busy = Some(row.get(0)?);
            Ok(())
        })?;
        busy.ok_or_else(|| {
            LimboError::InternalError("wal_checkpoint did not return a result row".to_owned())
        })
    }

    #[cfg(feature = "fs")]
    #[test]
    fn registry_reuses_cached_database_without_retaining_page_codec() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-registry.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let first_codec: Arc<dyn PageCodec> = Arc::new(IdentityPageCodec);
        let first_codec_weak = Arc::downgrade(&first_codec);
        let second_codec: Arc<dyn PageCodec> = Arc::new(IdentityPageCodec);

        let first = open_with_page_codec(io.clone(), path, first_codec);
        assert!(
            first_codec_weak.upgrade().is_none(),
            "cached Database must not retain the page codec"
        );
        let second = open_with_page_codec(io, path, second_codec);

        assert!(Arc::ptr_eq(&first, &second));
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_identity_must_match_cached_database() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-identity.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let first_codec: Arc<dyn PageCodec> = Arc::new(XorPageCodec {
            mask: 0xa5,
            reserved_bytes: 1,
        });
        let different_codec: Arc<dyn PageCodec> = Arc::new(XorPageCodec {
            mask: 0x5a,
            reserved_bytes: 1,
        });

        let db = open_with_page_codec(io.clone(), path, first_codec);

        let err = match db.connect(ConnectOpts::new().with_page_codec(different_codec.clone())) {
            Ok(_) => panic!("a cached codec-backed database must reject a different codec"),
            Err(err) => err,
        };
        assert!(err
            .to_string()
            .contains("page codec identity does not match the existing database"));

        let err = open_with_page_codec_result(io, path, different_codec).unwrap_err();
        assert!(err
            .to_string()
            .contains("page codec identity does not match the existing database"));
    }

    #[cfg(feature = "fs")]
    #[test]
    fn cached_database_rejects_encryption_and_page_codec() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-encryption-cached.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let codec: Arc<dyn PageCodec> = Arc::new(IdentityPageCodec);
        let _db = open_with_page_codec(io.clone(), path, codec.clone());

        let err = Database::open(
            io,
            path,
            OpenOptions::new(Arc::new(SqliteDialect))
                .flags(OpenFlags::Create)
                .encryption(EncryptionOpts {
                    cipher: "aes256gcm".to_string(),
                    hexkey: "00".repeat(32),
                })
                .page_codec(codec),
        )
        .unwrap_err();
        assert!(err
            .to_string()
            .contains("built-in encryption cannot be combined with an external page codec"));
    }

    #[cfg(feature = "fs")]
    #[test]
    fn external_page_codec_rejects_multiprocess_wal_before_opening_file() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-multiprocess-wal.db");
        let path_str = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());

        let err = Database::open(
            io,
            path_str,
            OpenOptions::new(Arc::new(SqliteDialect))
                .flags(OpenFlags::Create)
                .db_opts(DatabaseOpts::new().with_multiprocess_wal(true))
                .page_codec(Arc::new(IdentityPageCodec) as Arc<dyn PageCodec>),
        )
        .unwrap_err();

        assert!(matches!(
            err,
            LimboError::InvalidArgument(ref message)
                if message
                    == "external page codecs are not supported with experimental multiprocess WAL"
        ));
        assert!(!path.exists());
    }

    #[cfg(feature = "fs")]
    #[test]
    fn bypass_registry_page_codec_open_rejects_multiprocess_wal() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-bypass-multiprocess-wal.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let file = io.open_file(path, OpenFlags::Create, true).unwrap();
        let db_file: Arc<dyn DatabaseStorage> = Arc::new(DatabaseFile::new(file));
        let mut state = OpenDbAsyncState::new();

        let options = OpenOptions::new(Arc::new(SqliteDialect))
            .storage(db_file)
            .flags(OpenFlags::Create)
            .db_opts(DatabaseOpts::new().with_multiprocess_wal(true))
            .page_codec(Arc::new(IdentityPageCodec) as Arc<dyn PageCodec>);
        let err = match Database::do_open_async(&mut state, io, path, &options) {
            Err(err) => err,
            Ok(_) => panic!("multiprocess WAL must reject an external page codec"),
        };

        assert!(matches!(
            *err,
            LimboError::InvalidArgument(ref message)
                if message
                    == "external page codecs are not supported with experimental multiprocess WAL"
        ));
    }

    #[cfg(feature = "fs")]
    #[test]
    fn cached_page_codec_database_requires_codec_at_connection_time() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-registry-no-codec.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let codec: Arc<dyn PageCodec> = Arc::new(XorPageCodec {
            mask: 0xa5,
            reserved_bytes: 1,
        });
        let _db = open_with_page_codec(io.clone(), path, codec.clone());
        let db = open_with_page_codec(io.clone(), path, codec.clone());
        let conn = db
            .connect(ConnectOpts::new().with_page_codec(codec))
            .unwrap();
        conn.execute("create table test(id integer primary key)")
            .unwrap();
        drop(conn);

        assert!(
            Database::open_file_with_flags(
                io,
                path,
                OpenFlags::Create,
                DatabaseOpts::new(),
                None,
                Arc::new(SqliteDialect),
            )
            .is_err(),
            "opening without the codec must not reuse a codec-required database"
        );
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_checkpoint_decodes_database_page1_for_identity() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-checkpoint-identity.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let database_page1_decodes = Arc::new(AtomicUsize::new(0));
        let codec: Arc<dyn PageCodec> = Arc::new(CountingPageCodec {
            inner: XorPageCodec {
                mask: 0x5a,
                reserved_bytes: 1,
            },
            database_page1_decodes: database_page1_decodes.clone(),
        });
        let db = open_with_page_codec(io, path, codec.clone());
        let conn = db
            .connect(ConnectOpts::new().with_page_codec(codec))
            .unwrap();
        conn.execute("PRAGMA journal_mode = 'wal'").unwrap();
        conn.execute(
            "create table test(id integer primary key, value text);
             insert into test(value) values ('alpha');",
        )
        .unwrap();
        conn.set_sync_mode(crate::SyncMode::Full);

        database_page1_decodes.store(0, Ordering::Relaxed);
        let checkpoint = conn
            .checkpoint(crate::storage::wal::CheckpointMode::Passive {
                upper_bound_inclusive: None,
            })
            .unwrap();

        assert!(checkpoint.wal_checkpoint_backfilled > 0);
        assert_eq!(
            database_page1_decodes.load(Ordering::Relaxed),
            1,
            "checkpoint identity must decode database page 1 exactly once"
        );
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_checkpoint_recovers_after_page1_decode_failure() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-checkpoint-retry.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let fail_database_page1_decode = Arc::new(AtomicBool::new(false));
        let codec: Arc<dyn PageCodec> = Arc::new(FailOncePageCodec {
            inner: XorPageCodec {
                mask: 0x5a,
                reserved_bytes: 1,
            },
            fail_decode: fail_database_page1_decode.clone(),
            fail_context: PageCodecContext::new(
                DatabaseHeader::PAGE_ID as u32,
                PageLocation::Database,
            ),
        });
        let db = open_with_page_codec(io, path, codec.clone());
        let conn = db
            .connect(ConnectOpts::new().with_page_codec(codec))
            .unwrap();
        conn.execute("PRAGMA journal_mode = 'wal'").unwrap();
        conn.execute(
            "create table test(id integer primary key, value text);
             insert into test(value) values ('alpha');",
        )
        .unwrap();
        conn.set_sync_mode(crate::SyncMode::Full);

        fail_database_page1_decode.store(true, Ordering::Relaxed);
        let err = conn
            .checkpoint(crate::storage::wal::CheckpointMode::Passive {
                upper_bound_inclusive: None,
            })
            .unwrap_err();
        assert!(matches!(
            err,
            LimboError::CompletionError(CompletionError::PageCodecError { page_idx: 1 })
        ));

        let checkpoint = conn
            .checkpoint(crate::storage::wal::CheckpointMode::Passive {
                upper_bound_inclusive: None,
            })
            .unwrap();
        assert!(checkpoint.wal_checkpoint_backfilled > 0);
        assert_eq!(count_test_rows(&conn), 1);
    }

    #[cfg(feature = "fs")]
    #[test]
    fn pragma_checkpoint_codec_error_releases_checkpoint_lock() {
        if crate::storage::versioned::switch::pager_only(
            "this test exercises the pager's own pages: its page codec, its log and checkpoint; the versioned storage keeps neither",
        ) {
            return;
        }
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-pragma-checkpoint-retry.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let fail_wal_page2_decode = Arc::new(AtomicBool::new(false));
        let codec: Arc<dyn PageCodec> = Arc::new(FailOncePageCodec {
            inner: XorPageCodec {
                mask: 0x5a,
                reserved_bytes: 1,
            },
            fail_decode: fail_wal_page2_decode.clone(),
            fail_context: PageCodecContext::new(2, PageLocation::Wal),
        });
        let db = open_with_page_codec(io, path, codec.clone());
        let writer = db
            .connect(ConnectOpts::new().with_page_codec(codec.clone()))
            .unwrap();
        writer.wal_auto_actions_disable();
        writer.execute("PRAGMA journal_mode = 'wal'").unwrap();
        writer
            .execute(
                "create table test(id integer primary key, value text);
                 insert into test(value) values ('alpha');",
            )
            .unwrap();

        let pager = writer.get_pager();
        assert!(
            pager.wal_pos().1 > 0,
            "test setup must leave committed WAL frames"
        );
        assert_eq!(
            pager.wal_backfill_frame(),
            Some(0),
            "test setup must not checkpoint the WAL before injecting the codec error"
        );
        assert!(
            pager.wal_changed_pages_after(0).unwrap().contains(&2),
            "test setup must leave page 2 in the WAL"
        );

        let first = db
            .connect(ConnectOpts::new().with_page_codec(codec.clone()))
            .unwrap();
        assert!(first.get_pager().has_wal());
        fail_wal_page2_decode.store(true, Ordering::Relaxed);
        let first_error = passive_checkpoint_busy(&first).unwrap_err();
        assert!(matches!(first_error, LimboError::CheckpointFailed(_)));
        assert!(!fail_wal_page2_decode.load(Ordering::Relaxed));

        let second = db
            .connect(ConnectOpts::new().with_page_codec(codec))
            .unwrap();
        assert!(second.get_pager().has_wal());
        let other_connection_retry = passive_checkpoint_busy(&second);
        let same_connection_retry = passive_checkpoint_busy(&first);
        assert!(
            matches!(same_connection_retry, Ok(0)) && matches!(other_connection_retry, Ok(0)),
            "checkpoint failure must not leave stale state or retain its guard: same connection \
             returned {same_connection_retry:?}, other connection returned {other_connection_retry:?}"
        );
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_transaction_recovers_after_wal_encode_failure() {
        if crate::storage::versioned::switch::pager_only(
            "this test exercises the pager's own pages: its page codec, its log and checkpoint; the versioned storage keeps neither",
        ) {
            return;
        }
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-commit-retry.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let failures = Arc::new(PageCodecFailureSwitches::default());
        let codec: Arc<dyn PageCodec> = Arc::new(FailOnceTransformPageCodec {
            inner: XorPageCodec {
                mask: 0x5a,
                reserved_bytes: 1,
            },
            failures: failures.clone(),
        });
        let db = open_with_page_codec(io.clone(), path, codec.clone());
        let conn = db
            .connect(ConnectOpts::new().with_page_codec(codec.clone()))
            .unwrap();
        conn.execute("create table test(id integer primary key, value text)")
            .unwrap();

        failures.wal_encode.store(true, Ordering::Relaxed);
        let err = conn
            .execute("insert into test(value) values ('not-committed')")
            .unwrap_err();
        assert!(err.to_string().contains("injected Wal encode failure"));
        assert_eq!(count_test_rows(&conn), 0);

        conn.execute("insert into test(value) values ('committed')")
            .unwrap();
        assert_eq!(count_test_rows(&conn), 1);
        conn.checkpoint(crate::CheckpointMode::Full).unwrap();
        drop(conn);
        drop(db);

        let db = open_with_page_codec(io, path, codec.clone());
        let conn = db
            .connect(ConnectOpts::new().with_page_codec(codec))
            .unwrap();
        assert_eq!(count_test_rows(&conn), 1);
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_pager_read_recovers_after_decode_failure() {
        if crate::storage::versioned::switch::pager_only(
            "this test exercises the pager's own pages: its page codec, its log and checkpoint; the versioned storage keeps neither",
        ) {
            return;
        }
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-read-retry.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let failures = Arc::new(PageCodecFailureSwitches::default());
        let codec: Arc<dyn PageCodec> = Arc::new(FailOnceTransformPageCodec {
            inner: XorPageCodec {
                mask: 0x5a,
                reserved_bytes: 1,
            },
            failures: failures.clone(),
        });

        {
            let db = open_with_page_codec(io.clone(), path, codec.clone());
            let conn = db
                .connect(ConnectOpts::new().with_page_codec(codec.clone()))
                .unwrap();
            conn.execute(
                "create table test(id integer primary key, value text);
                 insert into test(value) values ('persisted');",
            )
            .unwrap();
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
        }

        let db = open_with_page_codec(io, path, codec.clone());
        let conn = db
            .connect(ConnectOpts::new().with_page_codec(codec.clone()))
            .unwrap();
        failures.database_decode_page.store(2, Ordering::Relaxed);
        let err = try_count_test_rows(&conn).unwrap_err();
        assert!(matches!(
            err,
            LimboError::CompletionError(CompletionError::PageCodecError { page_idx: 2 })
        ));

        assert_eq!(try_count_test_rows(&conn).unwrap(), 1);
        let second = db
            .connect(ConnectOpts::new().with_page_codec(codec))
            .unwrap();
        assert_eq!(try_count_test_rows(&second).unwrap(), 1);
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_checkpoint_recovers_after_backfill_transform_failures() {
        if crate::storage::versioned::switch::pager_only(
            "this test exercises the pager's own pages: its page codec, its log and checkpoint; the versioned storage keeps neither",
        ) {
            return;
        }
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-backfill-retry.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let failures = Arc::new(PageCodecFailureSwitches::default());
        let codec: Arc<dyn PageCodec> = Arc::new(FailOnceTransformPageCodec {
            inner: XorPageCodec {
                mask: 0x5a,
                reserved_bytes: 1,
            },
            failures: failures.clone(),
        });
        let db = open_with_page_codec(io.clone(), path, codec.clone());
        let conn = db
            .connect(ConnectOpts::new().with_page_codec(codec.clone()))
            .unwrap();
        conn.execute(
            "create table test(id integer primary key, value text);
             insert into test(value) values ('first');",
        )
        .unwrap();

        conn.pager.load().clear_page_cache(false);
        failures.wal_decode.store(true, Ordering::Relaxed);
        let err = conn.checkpoint(crate::CheckpointMode::Full).unwrap_err();
        assert!(matches!(
            err,
            LimboError::CompletionError(CompletionError::PageCodecError { .. })
        ));
        assert_eq!(count_test_rows(&conn), 1);
        conn.checkpoint(crate::CheckpointMode::Full).unwrap();

        conn.execute("insert into test(value) values ('second')")
            .unwrap();
        failures.database_encode.store(true, Ordering::Relaxed);
        let err = conn.checkpoint(crate::CheckpointMode::Full).unwrap_err();
        assert!(err.to_string().contains("injected Database encode failure"));
        assert_eq!(count_test_rows(&conn), 2);
        conn.checkpoint(crate::CheckpointMode::Full).unwrap();
        drop(conn);
        drop(db);

        let db = open_with_page_codec(io, path, codec.clone());
        let conn = db
            .connect(ConnectOpts::new().with_page_codec(codec))
            .unwrap();
        assert_eq!(count_test_rows(&conn), 2);
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_round_trips_wal_and_checkpointed_database_with_bootstrap_header() {
        if crate::storage::versioned::switch::pager_only(
            "this test exercises the pager's own pages: its page codec, its log and checkpoint; the versioned storage keeps neither",
        ) {
            return;
        }
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-roundtrip.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let codec: Arc<dyn PageCodec> = Arc::new(XorPageCodec {
            mask: 0x5a,
            reserved_bytes: 1,
        });

        {
            let db = open_with_page_codec(io.clone(), path, codec.clone());
            let conn = db
                .connect(ConnectOpts::new().with_page_codec(codec.clone()))
                .unwrap();
            conn.execute("PRAGMA journal_mode = 'wal'").unwrap();
            conn.execute(
                "create table test(id integer primary key, value text);
                 insert into test(value) values ('alpha'), ('bravo');",
            )
            .unwrap();
            assert_eq!(count_test_rows(&conn), 2);
            conn.set_sync_mode(crate::SyncMode::Full);
            let passive = conn
                .checkpoint(crate::storage::wal::CheckpointMode::Passive {
                    upper_bound_inclusive: None,
                })
                .unwrap();
            assert!(
                passive.wal_checkpoint_backfilled > 0,
                "PASSIVE checkpoint must backfill codec-transformed pages"
            );
            conn.execute("insert into test(value) values ('charlie')")
                .unwrap();
            let full = conn
                .checkpoint(crate::storage::wal::CheckpointMode::Full)
                .unwrap();
            assert!(
                full.wal_checkpoint_backfilled > 0,
                "FULL checkpoint must backfill codec-transformed pages"
            );
            assert_eq!(count_test_rows(&conn), 3);
        }

        let raw_database = std::fs::read(path).unwrap();
        assert_ne!(&raw_database[..16], b"SQLite format 3\0");

        let reopened = open_with_page_codec(io, path, codec.clone());
        let conn = reopened
            .connect(ConnectOpts::new().with_page_codec(codec))
            .unwrap();
        assert_eq!(count_test_rows(&conn), 3);
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_reopens_live_wal_then_checkpointed_database() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-live-wal-recovery.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let codec: Arc<dyn PageCodec> = Arc::new(LocationPageCodec);

        {
            let db = open_with_page_codec(io.clone(), path, codec.clone());
            let conn = db
                .connect(ConnectOpts::new().with_page_codec(codec.clone()))
                .unwrap();
            conn.execute("PRAGMA journal_mode = 'wal'").unwrap();
            conn.execute(
                "create table test(id integer primary key, value text);
                 insert into test(value) values ('alpha'), ('bravo'), ('charlie');",
            )
            .unwrap();
            assert_eq!(count_test_rows(&conn), 3);
        }
        let wal_path = format!("{path}-wal");
        assert!(
            std::fs::metadata(&wal_path).unwrap().len() > 0,
            "the first reopen must recover committed codec-transformed WAL frames"
        );

        {
            let db = open_with_page_codec(io.clone(), path, codec.clone());
            let conn = db
                .connect(ConnectOpts::new().with_page_codec(codec.clone()))
                .unwrap();
            assert_eq!(count_test_rows(&conn), 3);
            let checkpoint = conn
                .checkpoint(crate::storage::wal::CheckpointMode::Full)
                .unwrap();
            assert!(
                checkpoint.wal_checkpoint_backfilled > 0,
                "the recovery checkpoint must backfill codec-transformed WAL frames"
            );
        }
        let db = open_with_page_codec(io, path, codec.clone());
        let conn = db
            .connect(ConnectOpts::new().with_page_codec(codec))
            .unwrap();
        assert_eq!(count_test_rows(&conn), 3);
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_reopen_requires_recoverable_bootstrap_header() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-no-probe-roundtrip.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let codec: Arc<dyn PageCodec> = Arc::new(XorDefaultBootstrapPageCodec {
            mask: 0x3c,
            reserved_bytes: 1,
        });

        {
            let db = open_with_page_codec(io.clone(), path, codec.clone());
            let conn = db
                .connect(ConnectOpts::new().with_page_codec(codec.clone()))
                .unwrap();
            conn.execute(
                "create table test(id integer primary key, value text);
                 insert into test(value) values ('alpha'), ('bravo');",
            )
            .unwrap();
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
        }

        let err = open_with_page_codec_result(io, path, codec).unwrap_err();
        assert!(err
            .to_string()
            .contains("page codec reported invalid page size"));
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_rejects_mvcc_mode() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-mvcc.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let codec: Arc<dyn PageCodec> = Arc::new(IdentityPageCodec);
        let db = open_with_page_codec(io, path, codec.clone());
        let conn = db
            .connect(ConnectOpts::new().with_page_codec(codec))
            .unwrap();

        let err = conn.execute("PRAGMA journal_mode = 'mvcc'").unwrap_err();
        assert!(err
            .to_string()
            .contains("external page codecs are not supported with MVCC"));
    }

    #[cfg(feature = "fs")]
    #[test]
    fn plaintext_database_rejects_page_codec_connection() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("plaintext-codec-connection.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let db = Database::open_file_with_flags(
            io,
            path,
            OpenFlags::Create,
            DatabaseOpts::new(),
            None,
            Arc::new(SqliteDialect),
        )
        .unwrap();

        let err = match db.connect(ConnectOpts::new().with_page_codec(Arc::new(IdentityPageCodec)))
        {
            Ok(_) => panic!("plaintext databases must reject external page codecs"),
            Err(err) => err,
        };
        assert!(err
            .to_string()
            .contains("database was opened without an external page codec"));
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_database_rejects_codecless_connection() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-requires-connection-codec.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let db = open_with_page_codec(io, path, Arc::new(IdentityPageCodec));

        let err = match db.connect(ConnectOpts::new()) {
            Ok(_) => panic!("codec-backed databases must reject codec-less connections"),
            Err(err) => err,
        };
        assert!(err
            .to_string()
            .contains("database requires an external page codec"));
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_reopen_rejects_mismatched_reserved_space() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-reserved-space.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let initial_codec: Arc<dyn PageCodec> = Arc::new(XorPageCodec {
            mask: 0x5a,
            reserved_bytes: 1,
        });

        {
            let db = open_with_page_codec(io.clone(), path, initial_codec.clone());
            let conn = db
                .connect(ConnectOpts::new().with_page_codec(initial_codec))
                .unwrap();
            conn.execute("create table test(id integer primary key)")
                .unwrap();
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
        }

        for required_reserved_bytes in [0, 2] {
            let codec: Arc<dyn PageCodec> = Arc::new(XorPageCodec {
                mask: 0x5a,
                reserved_bytes: required_reserved_bytes,
            });
            let err = open_with_page_codec_result(io.clone(), path, codec).unwrap_err();
            assert!(err.to_string().contains(&format!(
                "page codec requires exactly {required_reserved_bytes} reserved bytes, but database provides 1"
            )));
        }
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_reopen_rejects_invalid_header_layout() {
        if crate::storage::versioned::switch::pager_only(
            "this test exercises the pager's own pages: its page codec, its log and checkpoint; the versioned storage keeps neither",
        ) {
            return;
        }
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-invalid-header-layout.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());

        {
            let db = open_with_page_codec(io.clone(), path, Arc::new(IdentityPageCodec));
            let conn = db
                .connect(ConnectOpts::new().with_page_codec(Arc::new(IdentityPageCodec)))
                .unwrap();
            conn.execute("create table test(id integer primary key)")
                .unwrap();
            conn.execute("pragma wal_checkpoint(truncate)").unwrap();
        }

        for (codec, expected_error) in [
            (
                InvalidHeaderPageCodec {
                    page_size: 513,
                    reserved_space: 0,
                },
                "page codec reported invalid page size 513",
            ),
            (
                InvalidHeaderPageCodec {
                    page_size: 512,
                    reserved_space: 33,
                },
                "page codec reported invalid reserved space 33 for page size 512",
            ),
            (
                InvalidHeaderPageCodec {
                    page_size: 8192,
                    reserved_space: 0,
                },
                "page codec bootstrap page size 8192 does not match decoded page-1 size 4096",
            ),
        ] {
            let err = open_with_page_codec_result(io.clone(), path, Arc::new(codec)).unwrap_err();
            assert!(err.to_string().contains(expected_error));
        }
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_reopen_reports_short_header_read() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-short-header.db");
        std::fs::write(&path, [0u8]).unwrap();
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());

        let err = open_with_page_codec_result(io, path, Arc::new(IdentityPageCodec)).unwrap_err();
        assert!(err.to_string().contains("short read on page 1"));
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_and_encryption_cannot_share_pager() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-encryption-conflict.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let codec: Arc<dyn PageCodec> = Arc::new(IdentityPageCodec);
        let db = open_with_page_codec(io, path, codec.clone());
        let encryption_key = EncryptionKey::from_hex_string(&"00".repeat(32)).unwrap();
        let mut state = InitState::default();

        let err = match db._init_nonblock(&mut state, Some(&encryption_key), Some(&codec)) {
            Ok(_) => panic!("encryption and an external page codec must not share a pager"),
            Err(err) => err,
        };
        assert!(err
            .to_string()
            .contains("built-in encryption cannot be combined with an external page codec"));
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_connection_rejects_builtin_encryption_settings() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-encryption-settings.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let codec: Arc<dyn PageCodec> = Arc::new(IdentityPageCodec);
        let db = open_with_page_codec(io, path, codec.clone());
        let conn = db
            .connect(ConnectOpts::new().with_page_codec(codec))
            .unwrap();

        let err = conn
            .set_encryption_key(EncryptionKey::from_hex_string(&"00".repeat(32)).unwrap())
            .unwrap_err();
        assert!(matches!(
            err,
            LimboError::InvalidArgument(message)
                if message
                    == "cannot configure built-in encryption while an external page codec is installed"
        ));
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_vacuum_preserves_encoded_database() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-vacuum.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let codec: Arc<dyn PageCodec> = Arc::new(XorPageCodec {
            mask: 0x4d,
            reserved_bytes: 1,
        });
        let db = open_with_page_codec_with_opts_result(
            io.clone(),
            path,
            codec.clone(),
            DatabaseOpts::new().with_vacuum(true),
        )
        .unwrap();
        let conn = db
            .connect(ConnectOpts::new().with_page_codec(codec.clone()))
            .unwrap();
        conn.execute(
            "create table test(id integer primary key, value text);
             insert into test(value) values ('alpha'), ('bravo');",
        )
        .unwrap();
        conn.execute("VACUUM").unwrap();
        assert_eq!(count_test_rows(&conn), 2);
        drop(conn);
        drop(db);

        assert_ne!(&std::fs::read(path).unwrap()[..16], b"SQLite format 3\0");
        let reopened = open_with_page_codec(io, path, codec.clone());
        let conn = reopened
            .connect(ConnectOpts::new().with_page_codec(codec))
            .unwrap();
        assert_eq!(count_test_rows(&conn), 2);
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_vacuum_into_preserves_encoded_database() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-vacuum-into.db");
        let output_path = temp_dir.path().join("codec-vacuum-into-copy.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let codec: Arc<dyn PageCodec> = Arc::new(XorPageCodec {
            mask: 0x4d,
            reserved_bytes: 1,
        });
        let db = open_with_page_codec_with_opts_result(
            io.clone(),
            path,
            codec.clone(),
            DatabaseOpts::new().with_vacuum(true),
        )
        .unwrap();
        let conn = db
            .connect(ConnectOpts::new().with_page_codec(codec.clone()))
            .unwrap();
        conn.execute(
            "create table test(id integer primary key, value text);
             insert into test(value) values ('secret-value');",
        )
        .unwrap();
        conn.execute(format!("VACUUM INTO '{}'", output_path.display()))
            .unwrap();

        let raw_output = std::fs::read(&output_path).unwrap();
        assert_ne!(&raw_output[..16], b"SQLite format 3\0");
        let output = open_with_page_codec(io, output_path.to_str().unwrap(), codec.clone());
        let output_conn = output
            .connect(ConnectOpts::new().with_page_codec(codec))
            .unwrap();
        assert_eq!(count_test_rows(&output_conn), 1);
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_connection_uses_plain_internal_temp_database() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-temp.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let codec: Arc<dyn PageCodec> = Arc::new(XorPageCodec {
            mask: 0x4d,
            reserved_bytes: 1,
        });
        let db = open_with_page_codec(io.clone(), path, codec.clone());
        let conn = db
            .connect(ConnectOpts::new().with_page_codec(codec.clone()))
            .unwrap();
        conn.execute(
            "create table test(id integer primary key, value text);
             insert into test(value) values ('main');
             create temp table temp_values(value integer);
             insert into temp_values values (3), (1), (2);",
        )
        .unwrap();

        let mut values = Vec::new();
        conn.prepare("select value from temp_values order by value")
            .unwrap()
            .run_with_row_callback(|row| {
                values.push(row.get::<i64>(0).unwrap());
                Ok(())
            })
            .unwrap();
        assert_eq!(values, vec![1, 2, 3]);
        assert_eq!(count_test_rows(&conn), 1);
        drop(conn);
        drop(db);

        let db = open_with_page_codec(io, path, codec.clone());
        let conn = db
            .connect(ConnectOpts::new().with_page_codec(codec))
            .unwrap();
        assert_eq!(count_test_rows(&conn), 1);
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_connections_preserve_old_reader_snapshot_during_checkpoint() {
        if crate::storage::versioned::switch::pager_only(
            "this test exercises the pager's own pages: its page codec, its log and checkpoint; the versioned storage keeps neither",
        ) {
            return;
        }
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-connections.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let codec = || -> Arc<dyn PageCodec> {
            Arc::new(XorPageCodec {
                mask: 0x5a,
                reserved_bytes: 1,
            })
        };
        let db = open_with_page_codec(io.clone(), path, codec());
        let writer = db
            .connect(ConnectOpts::new().with_page_codec(codec()))
            .unwrap();
        let old_reader = db
            .connect(ConnectOpts::new().with_page_codec(codec()))
            .unwrap();
        writer
            .execute(
                "create table test(id integer primary key, value text);
                 insert into test(value) values ('first');",
            )
            .unwrap();

        old_reader.execute("BEGIN").unwrap();
        assert_eq!(count_test_rows(&old_reader), 1);
        writer
            .execute("insert into test(value) values ('second')")
            .unwrap();
        assert_eq!(count_test_rows(&old_reader), 1);

        let new_reader = db
            .connect(ConnectOpts::new().with_page_codec(codec()))
            .unwrap();
        assert_eq!(count_test_rows(&new_reader), 2);
        let passive = writer
            .checkpoint(crate::CheckpointMode::Passive {
                upper_bound_inclusive: None,
            })
            .unwrap();
        assert!(passive.wal_total_backfilled < passive.wal_max_frame);
        assert_eq!(count_test_rows(&old_reader), 1);
        assert_eq!(count_test_rows(&new_reader), 2);

        old_reader.execute("COMMIT").unwrap();
        writer.checkpoint(crate::CheckpointMode::Full).unwrap();
        drop(new_reader);
        drop(old_reader);
        drop(writer);
        drop(db);

        let db = open_with_page_codec(io, path, codec());
        let conn = db
            .connect(ConnectOpts::new().with_page_codec(codec()))
            .unwrap();
        assert_eq!(count_test_rows(&conn), 2);
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_accepts_minimum_usable_space_boundary() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-minimum-usable-space.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let codec: Arc<dyn PageCodec> = Arc::new(XorPageCodec {
            mask: 0x5a,
            reserved_bytes: 32,
        });

        {
            let db = open_with_page_codec(io.clone(), path, codec.clone());
            let conn = db
                .connect(ConnectOpts::new().with_page_codec(codec.clone()))
                .unwrap();
            conn.execute("PRAGMA page_size = 512").unwrap();
            assert_eq!(conn.get_page_size().get(), 512);
            conn.execute(
                "create table test(id integer primary key, value blob);
                 insert into test(value) values (zeroblob(2000));",
            )
            .unwrap();
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
        }

        let db = open_with_page_codec(io, path, codec.clone());
        let conn = db
            .connect(ConnectOpts::new().with_page_codec(codec))
            .unwrap();
        assert_eq!(conn.get_page_size().get(), 512);
        assert_eq!(count_test_rows(&conn), 1);
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_reserved_tail_detects_persistent_page_corruption() {
        if crate::storage::versioned::switch::pager_only(
            "this test exercises the pager's own pages: its page codec, its log and checkpoint; the versioned storage keeps neither",
        ) {
            return;
        }
        use std::fs::OpenOptions as StdOpenOptions;
        use std::io::{Read, Seek, SeekFrom, Write};

        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-tagged-page.db");
        let path_str = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let codec: Arc<dyn PageCodec> = Arc::new(TaggedPageCodec);

        {
            let db = open_with_page_codec(io.clone(), path_str, codec.clone());
            let conn = db
                .connect(ConnectOpts::new().with_page_codec(codec.clone()))
                .unwrap();
            conn.execute("PRAGMA page_size = 512").unwrap();
            conn.execute(
                "create table test(id integer primary key, value blob);
                 insert into test(value) values (zeroblob(2000));",
            )
            .unwrap();
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
            assert_eq!(count_test_rows(&conn), 1);
        }

        let tag_offset = 2 * 512 - 1;
        let mut file = StdOpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        file.seek(SeekFrom::Start(tag_offset)).unwrap();
        let mut original = [0];
        file.read_exact(&mut original).unwrap();
        file.seek(SeekFrom::Start(tag_offset)).unwrap();
        file.write_all(&[original[0] ^ 1]).unwrap();
        file.sync_all().unwrap();
        drop(file);

        {
            let db = open_with_page_codec(io.clone(), path_str, codec.clone());
            let conn = db
                .connect(ConnectOpts::new().with_page_codec(codec.clone()))
                .unwrap();
            let err = try_count_test_rows(&conn).unwrap_err();
            assert!(matches!(
                err,
                LimboError::CompletionError(CompletionError::PageCodecError { page_idx: 2 })
            ));
        }

        let mut file = StdOpenOptions::new().write(true).open(&path).unwrap();
        file.seek(SeekFrom::Start(tag_offset)).unwrap();
        file.write_all(&original).unwrap();
        file.sync_all().unwrap();
        drop(file);

        let db = open_with_page_codec(io, path_str, codec.clone());
        let conn = db
            .connect(ConnectOpts::new().with_page_codec(codec))
            .unwrap();
        assert_eq!(count_test_rows(&conn), 1);
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_reopens_checkpointed_database_read_only() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-read-only.db");
        let path_str = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let codec: Arc<dyn PageCodec> = Arc::new(XorPageCodec {
            mask: 0x5a,
            reserved_bytes: 1,
        });

        {
            let db = open_with_page_codec(io.clone(), path_str, codec.clone());
            let conn = db
                .connect(ConnectOpts::new().with_page_codec(codec.clone()))
                .unwrap();
            conn.execute(
                "create table test(id integer primary key, value text);
                 insert into test(value) values ('read-only');",
            )
            .unwrap();
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
        }
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();

        let db = Database::open(
            io,
            path_str,
            OpenOptions::new(Arc::new(SqliteDialect))
                .flags(OpenFlags::ReadOnly)
                .page_codec(codec.clone()),
        )
        .unwrap();
        let conn = db
            .connect(ConnectOpts::new().with_page_codec(codec))
            .unwrap();
        assert_eq!(count_test_rows(&conn), 1);
        drop(conn);
        drop(db);
        assert_eq!(std::fs::metadata(path).unwrap().modified().unwrap(), before);
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_rejects_database_already_in_mvcc_mode() {
        if crate::storage::versioned::switch::pager_only(
            "this test exercises the pager's own pages: its page codec, its log and checkpoint; the versioned storage keeps neither",
        ) {
            return;
        }
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-existing-mvcc.db");
        let path_str = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());

        {
            let db = Database::open_file_with_flags(
                io.clone(),
                path_str,
                OpenFlags::Create,
                DatabaseOpts::new(),
                None,
                Arc::new(SqliteDialect),
            )
            .unwrap();
            let conn = db.connect(ConnectOpts::new()).unwrap();
            conn.execute("PRAGMA journal_mode = 'mvcc'").unwrap();
        }

        let codec: Arc<dyn PageCodec> = Arc::new(XorPageCodec {
            mask: 0,
            reserved_bytes: if cfg!(feature = "checksum") {
                crate::storage::checksum::CHECKSUM_REQUIRED_RESERVED_BYTES
            } else {
                0
            },
        });
        let err = open_with_page_codec_result(io.clone(), path_str, codec).unwrap_err();
        assert!(
            err.to_string()
                .contains("external page codecs are not supported with MVCC databases"),
            "unexpected error: {err}"
        );

        let db = Database::open_file_with_flags(
            io,
            path_str,
            OpenFlags::Create,
            DatabaseOpts::new(),
            None,
            Arc::new(SqliteDialect),
        )
        .unwrap();
        db.connect(ConnectOpts::new()).unwrap();
    }

    #[cfg(feature = "fs")]
    #[test]
    fn location_page_codec_round_trips_wal_checkpoint_and_overflow_pages() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("location-codec.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let codec: Arc<dyn PageCodec> = Arc::new(LocationPageCodec);

        {
            let db = open_with_page_codec(io.clone(), path, codec.clone());
            let conn = db
                .connect(ConnectOpts::new().with_page_codec(codec.clone()))
                .unwrap();
            conn.execute(
                "create table test(id integer primary key, value blob);
                 insert into test(value) values (zeroblob(12000));
                 delete from test where id = 1;
                 insert into test(value)
                 values (cast(replace(printf('%0*d', 16000, 0), '0', 'x') as blob));",
            )
            .unwrap();
            let checkpoint = conn
                .checkpoint(crate::storage::wal::CheckpointMode::Full)
                .unwrap();
            assert!(checkpoint.wal_checkpoint_backfilled > 0);
        }

        let raw_database = std::fs::read(path).unwrap();
        assert_ne!(&raw_database[..16], b"SQLite format 3\0");
        let reopened = open_with_page_codec(io, path, codec.clone());
        let conn = reopened
            .connect(ConnectOpts::new().with_page_codec(codec))
            .unwrap();
        let mut values: Vec<(i64, String)> = Vec::new();
        conn.prepare("select length(value), hex(substr(value, 1, 8)) from test order by id")
            .unwrap()
            .run_with_row_callback(|row| {
                values.push((row.get(0).unwrap(), row.get(1).unwrap()));
                Ok(())
            })
            .unwrap();
        assert_eq!(values, vec![(16_000, "7878787878787878".to_string())]);
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_attach_is_rejected() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let main_path = temp_dir.path().join("codec-main.db");
        let aux_path = temp_dir.path().join("codec-aux.db");
        let main_path = main_path.to_str().unwrap();
        let aux_path = aux_path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let codec: Arc<dyn PageCodec> = Arc::new(XorPageCodec {
            mask: 0x91,
            reserved_bytes: 1,
        });
        let db = open_with_page_codec_with_opts_result(
            io.clone(),
            main_path,
            codec.clone(),
            DatabaseOpts::new().with_attach(true),
        )
        .unwrap();
        let conn = db
            .connect(ConnectOpts::new().with_page_codec(codec.clone()))
            .unwrap();
        let err = conn
            .execute(format!("ATTACH '{aux_path}' AS aux"))
            .unwrap_err();
        assert!(err
            .to_string()
            .contains("ATTACH is unsupported for connections using an external page codec"));
        assert!(!std::path::Path::new(aux_path).exists());
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_rejects_reserved_space_mutation() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-reserved-space.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let codec: Arc<dyn PageCodec> = Arc::new(XorPageCodec {
            mask: 0x91,
            reserved_bytes: 1,
        });
        let db = open_with_page_codec(io, path, codec.clone());
        let conn = db
            .connect(ConnectOpts::new().with_page_codec(codec))
            .unwrap();

        let err = conn.set_reserved_bytes(0).unwrap_err();
        assert!(err
            .to_string()
            .contains("page codec requires exactly 1 reserved bytes"));
        assert_eq!(conn.get_reserved_bytes(), Some(1));
    }

    #[cfg(feature = "fs")]
    #[test]
    fn page_codec_rejects_page_size_incompatible_with_reserved_space() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("codec-page-size.db");
        let path = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let codec: Arc<dyn PageCodec> = Arc::new(XorPageCodec {
            mask: 0x5a,
            reserved_bytes: 33,
        });

        {
            let db = open_with_page_codec(io.clone(), path, codec.clone());
            let conn = db
                .connect(ConnectOpts::new().with_page_codec(codec.clone()))
                .unwrap();
            // 512 - 33 = 479 usable bytes < 480: must be rejected up front,
            // otherwise the engine creates a database it refuses to reopen.
            let err = conn.execute("PRAGMA page_size = 512").unwrap_err();
            assert!(err.to_string().contains("usable bytes"));
            assert_eq!(conn.get_page_size().get(), 4096);
            // The database must remain usable at a compatible page size.
            conn.execute("create table test(id integer primary key, value text)")
                .unwrap();
            conn.execute("insert into test(value) values ('alpha')")
                .unwrap();
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
        }

        let reopened = open_with_page_codec(io, path, codec.clone());
        let conn = reopened
            .connect(ConnectOpts::new().with_page_codec(codec))
            .unwrap();
        assert_eq!(count_test_rows(&conn), 1);
    }

    /// Review Spec P1 regression: live restore must publish the restored schema atomically with
    /// the bumped `restore_epoch` under `self.schema.lock()` so a concurrent `maybe_update_schema`
    /// observing the new epoch cannot adopt the old schema under an equal schema cookie.
    #[cfg(all(feature = "fs", feature = "conn_raw_api"))]
    #[test]
    fn review_p1_live_restore_publishes_schema_and_restore_epoch_atomically() {
        for is_postgres in [false, true] {
            let temp_dir = tempfile::TempDir::new().unwrap();
            let target_path = temp_dir.path().join("target.db");
            let donor_path = temp_dir.path().join("donor.db");
            let target_str = target_path.to_str().unwrap();
            let donor_str = donor_path.to_str().unwrap();
            let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
            let make_dialect = || -> Arc<dyn crate::Dialect> {
                if is_postgres {
                    Arc::new(crate::PostgresDialect::new())
                } else {
                    Arc::new(SqliteDialect)
                }
            };

            // Build donor with equal schema cookie (1 table) but different column name (`new_value`).
            {
                let donor = Database::open_file(io.clone(), donor_str, make_dialect()).unwrap();
                let dconn = donor.connect(ConnectOpts::new()).unwrap();
                dconn
                    .execute("CREATE TABLE t(id INTEGER PRIMARY KEY, new_value INTEGER)")
                    .unwrap();
                dconn.execute("INSERT INTO t VALUES (1, 22)").unwrap();
                dconn
                    .checkpoint(crate::storage::wal::CheckpointMode::Truncate {
                        upper_bound_inclusive: None,
                    })
                    .unwrap();
                dconn.close().unwrap();
            }

            let target = Database::open_file(io.clone(), target_str, make_dialect()).unwrap();
            let warmed = target.connect(ConnectOpts::new()).unwrap();
            warmed
                .execute("CREATE TABLE t(id INTEGER PRIMARY KEY, old_value INTEGER)")
                .unwrap();
            warmed.execute("INSERT INTO t VALUES (1, 11)").unwrap();
            let _ = warmed
                .prepare("SELECT old_value FROM t")
                .unwrap()
                .run_collect_rows()
                .unwrap();

            // Hammer `maybe_update_schema` across repeated live reloads to catch any publication window.
            let stop = Arc::new(AtomicBool::new(false));
            let warmed_bg = Arc::clone(&warmed);
            let stop_bg = Arc::clone(&stop);
            let poller = std::thread::spawn(move || {
                while !stop_bg.load(Ordering::Relaxed) {
                    warmed_bg.maybe_update_schema();
                }
            });

            for _ in 0..16 {
                std::fs::copy(&donor_path, &target_path).unwrap();
                let _ = std::fs::remove_file(format!("{target_str}-wal"));
                let _ = std::fs::remove_file(format!("{target_str}-wal2"));
                target.reload_wal_after_external_restore().unwrap();
            }
            stop.store(true, Ordering::Relaxed);
            poller.join().unwrap();

            assert!(
                warmed.execute("UPDATE t SET old_value = 44").is_err(),
                "is_postgres={is_postgres}: warmed connection must not accept stale column old_value after live restore"
            );
            let rows = warmed
                .prepare("SELECT new_value FROM t")
                .unwrap()
                .run_collect_rows()
                .unwrap();
            assert_eq!(rows, vec![vec![crate::Value::from_i64(22)]]);
        }
    }

    /// Review Spec P2 regression: PostgreSQL live restore (both in-place copy and atomic rename)
    /// must load the donor's persisted `pg_next_oid` so dropped table OIDs are never reused.
    #[cfg(all(feature = "fs", feature = "conn_raw_api"))]
    #[test]
    fn review_p2_postgres_live_restore_preserves_pg_next_oid() {
        for atomic_rename in [false, true] {
            let temp_dir = tempfile::TempDir::new().unwrap();
            let donor_path = temp_dir.path().join("donor.db");
            let cold_path = temp_dir.path().join("cold.db");
            let target_path = temp_dir.path().join("target.db");
            let donor_str = donor_path.to_str().unwrap();
            let cold_str = cold_path.to_str().unwrap();
            let target_str = target_path.to_str().unwrap();
            let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());

            // Donor creates 6 tables and drops 5 so `pg_next_oid` advances to 16390.
            {
                let donor = Database::open_file(
                    io.clone(),
                    donor_str,
                    Arc::new(crate::PostgresDialect::new()),
                )
                .unwrap();
                let conn = donor.connect(ConnectOpts::new()).unwrap();
                conn.execute("CREATE TABLE keep_t(id int4)").unwrap();
                for i in 0..5 {
                    conn.execute(format!("CREATE TABLE tmp_{i}(id int4)"))
                        .unwrap();
                    conn.execute(format!("DROP TABLE tmp_{i}")).unwrap();
                }
                conn.checkpoint(crate::storage::wal::CheckpointMode::Truncate {
                    upper_bound_inclusive: None,
                })
                .unwrap();
                conn.close().unwrap();
            }

            // Cold-open reference assigns `expected_oid`.
            std::fs::copy(&donor_path, &cold_path).unwrap();
            let expected_oid: i64 = {
                let cold = Database::open_file(
                    io.clone(),
                    cold_str,
                    Arc::new(crate::PostgresDialect::new()),
                )
                .unwrap();
                let conn = cold.connect(ConnectOpts::new()).unwrap();
                conn.execute("CREATE TABLE probe(id int4)").unwrap();
                let rows = conn
                    .prepare("SELECT oid FROM pg_class WHERE relname = 'probe'")
                    .unwrap()
                    .run_collect_rows()
                    .unwrap();
                match &rows[0][0] {
                    crate::Value::Numeric(crate::Numeric::Integer(v)) => *v,
                    other => panic!("unexpected oid value: {other:?}"),
                }
            };
            assert_eq!(expected_oid, 16390);

            // Live target starts at OID 16384 (1 table), then reloads donor in place or via rename.
            let target = Database::open_file(
                io.clone(),
                target_str,
                Arc::new(crate::PostgresDialect::new()),
            )
            .unwrap();
            let conn = target.connect(ConnectOpts::new()).unwrap();
            conn.execute("CREATE TABLE orig(id int4)").unwrap();

            if atomic_rename {
                let staged = temp_dir.path().join("staged.db");
                std::fs::copy(&donor_path, &staged).unwrap();
                std::fs::rename(&staged, &target_path).unwrap();
            } else {
                std::fs::copy(&donor_path, &target_path).unwrap();
            }
            let _ = std::fs::remove_file(format!("{target_str}-wal"));
            let _ = std::fs::remove_file(format!("{target_str}-wal2"));
            target.reload_wal_after_external_restore().unwrap();

            conn.execute("CREATE TABLE probe(id int4)").unwrap();
            let rows = conn
                .prepare("SELECT oid FROM pg_class WHERE relname = 'probe'")
                .unwrap()
                .run_collect_rows()
                .unwrap();
            let live_oid = match &rows[0][0] {
                crate::Value::Numeric(crate::Numeric::Integer(v)) => *v,
                other => panic!("unexpected oid value: {other:?}"),
            };
            assert_eq!(
                live_oid, expected_oid,
                "atomic_rename={atomic_rename}: live restore must preserve donor pg_next_oid"
            );
        }
    }

    /// Review Standards P2 regression: an atomic-rename restore on an unregistered raw `do_open`
    /// handle (`registry_key == None`) must clear its temporary `RegistryEntry::Opening` sentinel
    /// instead of leaving future `Database::open_file` calls livelocked.
    #[cfg(all(feature = "fs", feature = "conn_raw_api"))]
    #[test]
    fn review_p2_raw_do_open_atomic_rename_restore_does_not_leak_opening_entry() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let target_path = temp_dir.path().join("target.db");
        let donor_path = temp_dir.path().join("donor.db");
        let target_str = target_path.to_str().unwrap();
        let donor_str = donor_path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());

        {
            let donor =
                Database::open_file(io.clone(), donor_str, Arc::new(SqliteDialect)).unwrap();
            let conn = donor.connect(ConnectOpts::new()).unwrap();
            conn.execute("CREATE TABLE t(v INTEGER)").unwrap();
            conn.execute("INSERT INTO t VALUES (99)").unwrap();
            conn.checkpoint(crate::storage::wal::CheckpointMode::Truncate {
                upper_bound_inclusive: None,
            })
            .unwrap();
            conn.close().unwrap();
        }

        {
            let raw = Database::do_open(
                io.clone(),
                target_str,
                OpenOptions::new(Arc::new(SqliteDialect)),
            )
            .unwrap();
            let conn = raw.connect(ConnectOpts::new()).unwrap();
            conn.execute("CREATE TABLE t(v INTEGER)").unwrap();
            conn.execute("INSERT INTO t VALUES (1)").unwrap();

            // Atomic rename replaces target with a fresh inode.
            std::fs::rename(&donor_path, &target_path).unwrap();
            let _ = std::fs::remove_file(format!("{target_str}-wal"));
            let _ = std::fs::remove_file(format!("{target_str}-wal2"));
            raw.reload_wal_after_external_restore().unwrap();
            conn.close().unwrap();
        }

        // Normal registry-backed open must succeed immediately instead of livelocking on `Opening`.
        let reopened = Database::open_file(io, target_str, Arc::new(SqliteDialect)).unwrap();
        let conn = reopened.connect(ConnectOpts::new()).unwrap();
        let rows = conn
            .prepare("SELECT v FROM t")
            .unwrap()
            .run_collect_rows()
            .unwrap();
        assert_eq!(rows, vec![vec![crate::Value::from_i64(99)]]);
    }

    /// Review Round 5 Spec P1 regression: an in-flight `prepare()` that captured the pre-restore
    /// schema before an equal-cookie restore + second `prepare()` on the same connection must
    /// retain its pre-restore `PrepareContext` (generation & restore_epoch) so executing it forces
    /// reprepare and refuses writes to a removed column (`old_value`), leaving the donor's
    /// `new_value` untouched on both dialects.
    #[cfg(all(feature = "fs", feature = "conn_raw_api"))]
    #[test]
    fn review_p1_inflight_prepare_across_equal_cookie_restore_and_second_prepare_refuses_stale_column(
    ) {
        for is_postgres in [false, true] {
            let temp_dir = tempfile::TempDir::new().unwrap();
            let target_path = temp_dir.path().join("target.db");
            let donor_path = temp_dir.path().join("donor.db");
            let target_str = target_path.to_str().unwrap();
            let donor_str = donor_path.to_str().unwrap();
            let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
            let make_dialect = || -> Arc<dyn crate::Dialect> {
                if is_postgres {
                    Arc::new(crate::PostgresDialect::new())
                } else {
                    Arc::new(SqliteDialect)
                }
            };

            // Donor has equal schema cookie (1 table) with `new_value = 22`.
            {
                let donor = Database::open_file(io.clone(), donor_str, make_dialect()).unwrap();
                let dconn = donor.connect(ConnectOpts::new()).unwrap();
                dconn
                    .execute("CREATE TABLE t(id INTEGER PRIMARY KEY, new_value INTEGER)")
                    .unwrap();
                dconn.execute("INSERT INTO t VALUES (1, 22)").unwrap();
                dconn
                    .checkpoint(crate::storage::wal::CheckpointMode::Truncate {
                        upper_bound_inclusive: None,
                    })
                    .unwrap();
                dconn.close().unwrap();
            }

            let target = Database::open_file(io.clone(), target_str, make_dialect()).unwrap();
            let conn = target.connect(ConnectOpts::new()).unwrap();
            conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, old_value INTEGER)")
                .unwrap();
            conn.execute("INSERT INTO t VALUES (1, 11)").unwrap();

            // Simulate an in-flight `prepare("UPDATE t SET old_value = 44")` paused immediately
            // after capturing `(schema, prepare_context)` and before building bytecode.
            conn.maybe_update_schema();
            let (stale_schema, stale_ctx) = conn.snapshot_schema_and_prepare_context();

            // Complete an equal-cookie external restore, then prepare a second statement on the
            // SAME connection so `conn.schema`, `conn.last_restore_epoch`, and
            // `conn.prepare_context_generation` advance to the restored catalog.
            std::fs::copy(&donor_path, &target_path).unwrap();
            let _ = std::fs::remove_file(format!("{target_str}-wal"));
            let _ = std::fs::remove_file(format!("{target_str}-wal2"));
            target.reload_wal_after_external_restore().unwrap();
            let _second = conn.prepare("SELECT new_value FROM t").unwrap();

            // Now resume compiling the first statement against the pre-restore schema snapshot.
            let stale_program = crate::statement::plan_sql_on(
                &conn,
                conn.get_pager(),
                "UPDATE t SET old_value = 44",
                crate::QueryMode::Normal,
                crate::statement::StatementOrigin::Root,
                &crate::PrepareOptions::default(),
                stale_schema,
                stale_ctx,
            )
            .unwrap();
            let mut stmt = crate::Statement::new_with_origin(
                stale_program,
                conn.get_pager(),
                crate::QueryMode::Normal,
                "UPDATE t SET old_value = 44".len(),
                crate::statement::StatementOrigin::Root,
                false,
            );

            // Stepping the stale statement must detect the mismatch, reprepare against the donor
            // schema, and fail because `old_value` does not exist.
            assert!(
                stmt.run_ignore_rows().is_err(),
                "is_postgres={is_postgres}: paused prepare must not write through removed column old_value"
            );

            // Verify on a fresh connection that the donor's `new_value` is still 22.
            let fresh = target.connect(ConnectOpts::new()).unwrap();
            let rows = fresh
                .prepare("SELECT new_value FROM t")
                .unwrap()
                .run_collect_rows()
                .unwrap();
            assert_eq!(rows, vec![vec![crate::Value::from_i64(22)]]);
        }
    }

    /// Review Round 7 Spec P1 regression: rolling back a failed raw WAL session must retain the
    /// WAL write lock through `pager.rollback(...)` so a concurrent SQL writer cannot commit a
    /// transaction that the failed raw writer subsequently truncates from the shared frame index.
    #[cfg(feature = "conn_raw_api")]
    #[test]
    fn review_p1_failed_raw_wal_rollback_retains_write_lock_and_never_hides_committed_write() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Barrier,
        };

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("raw_rollback_race.db");
        let io: Arc<dyn IO> = Arc::new(crate::PlatformIO::new().unwrap());
        let db = Database::open_file(
            io.clone(),
            path.to_str().unwrap(),
            Arc::new(crate::SqliteDialect),
        )
        .unwrap();

        let setup = db.connect(ConnectOpts::new()).unwrap();
        setup
            .execute("CREATE TABLE t(v INTEGER); INSERT INTO t VALUES (1)")
            .unwrap();
        drop(setup);

        let raw = db.connect(ConnectOpts::new()).unwrap();
        let writer = db.connect(ConnectOpts::new()).unwrap();
        let page_size = raw.get_page_size().get() as usize;

        for expected in 2i64..=26 {
            // Read current page 1 bytes so our uncommitted raw frame is a valid page image.
            let mut page1 = vec![0u8; page_size];
            let mut read_tx_ok = false;
            raw.wal_insert_begin().unwrap();
            let wal_state = raw.wal_state().unwrap();
            if raw
                .try_wal_watermark_read_page(1, &mut page1, Some(wal_state.max_frame))
                .unwrap()
            {
                read_tx_ok = true;
            }
            assert!(read_tx_ok, "expected page 1 to be readable");

            let mut frame = Vec::with_capacity(24 + page_size);
            frame.extend_from_slice(&1u32.to_be_bytes());
            frame.extend_from_slice(&[0u8; 20]); // db_size = 0 (uncommitted frame)
            frame.extend_from_slice(&page1);
            raw.wal_insert_frame(wal_state.max_frame + 1, &frame)
                .unwrap();

            let barrier = Arc::new(Barrier::new(2));
            let writer_started = Arc::new(AtomicBool::new(false));
            let writer_conn = writer.clone();
            let barrier_w = Arc::clone(&barrier);
            let started_w = Arc::clone(&writer_started);

            let handle = std::thread::spawn(move || {
                barrier_w.wait();
                started_w.store(true, Ordering::Release);
                let sql = format!("UPDATE t SET v = {expected}");
                loop {
                    match writer_conn.execute(&sql) {
                        Ok(()) => break,
                        Err(LimboError::Busy) | Err(LimboError::BusySnapshot) => {
                            std::thread::yield_now();
                        }
                        Err(other) => {
                            panic!("unexpected writer error on round {expected}: {other}")
                        }
                    }
                }
            });

            barrier.wait();
            while !writer_started.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            for _ in 0..32 {
                std::thread::yield_now();
            }

            // Abort the raw WAL session: must finish rollback BEFORE releasing the WAL write lock.
            raw.wal_insert_end_commit_only(false).unwrap();
            handle.join().unwrap();

            // Fresh connection in the SAME process must see the committed value immediately.
            let fresh = db.connect(ConnectOpts::new()).unwrap();
            let rows = fresh
                .prepare("SELECT v FROM t")
                .unwrap()
                .run_collect_rows()
                .unwrap();
            assert_eq!(
                rows,
                vec![vec![crate::Value::from_i64(expected)]],
                "round {expected}: fresh in-process connection saw stale value after raw WAL rollback"
            );
        }

        drop(raw);
        drop(writer);
        drop(db);

        // Cold reopen must agree with the final committed value.
        let reopened =
            Database::open_file(io, path.to_str().unwrap(), Arc::new(crate::SqliteDialect))
                .unwrap();
        let conn = reopened.connect(ConnectOpts::new()).unwrap();
        let rows = conn
            .prepare("SELECT v FROM t")
            .unwrap()
            .run_collect_rows()
            .unwrap();
        assert_eq!(rows, vec![vec![crate::Value::from_i64(26)]]);
    }
}
