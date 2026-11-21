//! Amphora: the switch that puts a connection's B-trees on the versioned storage (G7 step 2,
//! `AC-NS.2.1`; Amphora's `docs/design/engine.md`, decision 13). Off by default: the pager's
//! B-trees stay the engine's storage until the engine's suites pass on this one.
//!
//! A database's connections share one versioned storage, made by the first connection that asks
//! for it: [`ConnectOpts::with_versioned_storage`](crate::ConnectOpts::with_versioned_storage), or
//! `AMPHORA_STORAGE=versioned` in the environment (so the engine's suites run on it unchanged).
//! With it on, the executor's cursors are [`VersionedCursor`]s on its trees, a tree's id is its
//! root page in `sqlite_schema` (tree 1 is `sqlite_schema` itself), and each SQL transaction is one
//! transaction of the storage, begun by its first cursor and committed or rolled back where the
//! pager's ends. The pager still runs beside it for what the trees don't hold yet: the database
//! header (the schema cookie, the user version).

use super::blink::Descender;
use super::cursor::{Access, KeyOrder, SharedTxn, VersionedCursor};
use super::disk::{Disk, Files, Simulated};
use super::durable::Database as VDatabase;
use crate::error::LimboError;
use crate::storage::page_cache::DEFAULT_PAGE_CACHE_SIZE_IN_PAGES;
use crate::sync::{Arc, Mutex};
use crate::types::IndexInfo;
use crate::{Pager, Result};

/// The environment variable that turns the switch on for every connection: `versioned`.
pub const ENV: &str = "AMPHORA_STORAGE";

/// Whether the environment asks for the versioned storage.
pub fn asked() -> bool {
    std::env::var(ENV).is_ok_and(|v| v == "versioned")
}

/// The trees a storage is made with: tree 0, which no root page names (SQLite's page numbers start
/// at 1), and tree 1, `sqlite_schema` (root page 1). The overflow tree comes after them (2), and
/// every tree made later takes the next id, which is its root page.
const TREES: usize = 2;

/// The versioned storage of the database at `path`: in memory for a memory database, else in the
/// directory `<path>-versioned`. Its pool holds as many pages as the pager's page cache does by
/// default (`DEFAULT_PAGE_CACHE_SIZE_IN_PAGES`, the engine's bound on the pages it keeps), and its
/// redo log takes a lane per thread the machine runs (decision 11).
pub fn open(path: &str) -> Result<Arc<VDatabase>> {
    let lanes = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let disk: Arc<dyn Disk> = if crate::database::is_memory_like(path) {
        Arc::new(Simulated::new(0, lanes)?)
    } else {
        let dir = std::path::PathBuf::from(format!("{path}-versioned"));
        std::fs::create_dir_all(&dir).map_err(|e| {
            LimboError::InternalError(format!(
                "versioned storage: making {}: {e}",
                dir.display()
            ))
        })?;
        Arc::new(Files::open(&dir, lanes)?)
    };
    Ok(Arc::new(VDatabase::open(
        disk,
        DEFAULT_PAGE_CACHE_SIZE_IN_PAGES,
        TREES,
        || KeyOrder::rowid().comparator(),
    )?))
}

/// One connection's side of the switch: the storage, and the transaction its cursors share.
pub struct Switch {
    db: Arc<VDatabase>,
    txn: Mutex<Option<SharedTxn>>,
    d: Mutex<Descender>,
}

impl Switch {
    pub fn new(db: Arc<VDatabase>) -> Self {
        Self {
            db,
            txn: Mutex::new(None),
            d: Mutex::new(Descender::new()),
        }
    }

    /// The connection's transaction, begun if none is open.
    fn txn(&self) -> Result<SharedTxn> {
        let mut slot = self.txn.lock();
        if let Some(txn) = slot.as_ref() {
            return Ok(txn.clone());
        }
        let txn = Arc::new(Mutex::new(Some(self.db.begin()?)));
        *slot = Some(txn.clone());
        Ok(txn)
    }

    /// A cursor on the tree at `root_page`, in the connection's transaction: keyed by rowid, or by
    /// record for `index` (an index's or a WITHOUT ROWID table's key).
    pub fn cursor(
        &self,
        root_page: i64,
        index: Option<Arc<IndexInfo>>,
        pager: Arc<Pager>,
    ) -> Result<VersionedCursor> {
        let tree = u64::try_from(root_page).map_err(|_| {
            LimboError::InternalError(format!(
                "versioned storage: no tree at root page {root_page}"
            ))
        })?;
        let order = match index {
            Some(info) => KeyOrder::index(info),
            None => KeyOrder::rowid(),
        };
        VersionedCursor::new(
            self.db.clone(),
            tree,
            order,
            Access::Write(self.txn()?),
            pager,
        )
    }

    /// Makes a table's tree, keyed by rowid: its root page. An index's tree is refused until the
    /// catalog records a comparison per tree (G7 step 2, piece 2): recovery would remake it in
    /// rowid order.
    pub fn make_table(&self) -> Result<i64> {
        let id = self
            .db
            .make_tree(&mut self.d.lock(), KeyOrder::rowid().comparator())?;
        i64::try_from(id).map_err(|_| LimboError::IntegerOverflow)
    }

    /// Ends the connection's transaction, if one is open: commits it (durable before it returns),
    /// or rolls it back. A cursor still holding it is refused its next write.
    pub fn finish(&self, rollback: bool) -> Result<()> {
        let Some(shared) = self.txn.lock().take() else {
            return Ok(());
        };
        let Some(txn) = shared.lock().take() else {
            return Ok(());
        };
        let mut d = self.d.lock();
        if rollback {
            self.db.abort(txn, &mut d)
        } else {
            self.db.commit(txn, &mut d, self.db.lane(), true).map(|_| ())
        }
    }
}
