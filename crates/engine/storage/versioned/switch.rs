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

use super::blink::{Comparator, Descender};
use super::cursor::{Access, KeyOrder, SharedTxn, VersionedCursor};
use super::disk::{Disk, Files, Simulated};
use super::durable::Database as VDatabase;
use super::version::Savepoint;
use crate::error::LimboError;
use crate::storage::page_cache::DEFAULT_PAGE_CACHE_SIZE_IN_PAGES;
use crate::sync::{Arc, Mutex, OnceLock};
use crate::types::IndexInfo;
use crate::{Pager, Result};
use std::collections::HashMap;

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

/// A database's versioned storage, shared by its connections, and the order of each index tree made
/// while it runs: known once a cursor first opens the tree with its index's key (an index's tree is
/// empty until then, so nothing is compared before), and kept for every later cursor and the
/// tree's own comparison, which share it.
pub struct Storage {
    db: Arc<VDatabase>,
    orders: Mutex<HashMap<u64, Arc<OnceLock<KeyOrder>>>>,
}

/// The versioned storage of the database at `path`: in memory for a memory database, else in the
/// directory `<path>-versioned`. Its pool holds as many pages as the pager's page cache does by
/// default (`DEFAULT_PAGE_CACHE_SIZE_IN_PAGES`, the engine's bound on the pages it keeps), and its
/// redo log takes a lane per thread the machine runs (decision 11).
pub fn open(path: &str) -> Result<Arc<Storage>> {
    let lanes = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let disk: Arc<dyn Disk> = if crate::database::is_memory_like(path) {
        Arc::new(Simulated::new(0, lanes)?)
    } else {
        let dir = std::path::PathBuf::from(format!("{path}-versioned"));
        std::fs::create_dir_all(&dir).map_err(|e| {
            LimboError::InternalError(format!("versioned storage: making {}: {e}", dir.display()))
        })?;
        Arc::new(Files::open(&dir, lanes)?)
    };
    let db = Arc::new(VDatabase::open(
        disk,
        DEFAULT_PAGE_CACHE_SIZE_IN_PAGES,
        TREES,
        || KeyOrder::rowid().comparator(),
    )?);
    Ok(Arc::new(Storage {
        db,
        orders: Mutex::new(HashMap::new()),
    }))
}

/// A comparison that orders as `slot`'s order once it is known, and by bytes before (when the tree
/// holds nothing to compare).
fn late(slot: Arc<OnceLock<KeyOrder>>) -> Comparator {
    Box::new(move |a: &[u8], b: &[u8]| match slot.get() {
        Some(order) => order.compare(a, b),
        None => a.cmp(b),
    })
}

fn tree_of(root_page: i64) -> Result<u64> {
    u64::try_from(root_page).map_err(|_| {
        LimboError::InternalError(format!(
            "versioned storage: no tree at root page {root_page}"
        ))
    })
}

/// One connection's side of the switch: the storage, and the transaction its cursors share.
pub struct Switch {
    storage: Arc<Storage>,
    db: Arc<VDatabase>,
    txn: Mutex<Option<SharedTxn>>,
    /// The savepoints of the write statements under way in the transaction, newest last: one a
    /// statement, so bounded by the statements nested in one another.
    statements: Mutex<Vec<Savepoint>>,
    d: Mutex<Descender>,
}

impl Switch {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self {
            db: storage.db.clone(),
            storage,
            txn: Mutex::new(None),
            statements: Mutex::new(Vec::new()),
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
        let tree = tree_of(root_page)?;
        let order = match index {
            Some(info) => {
                let slot = self.storage.orders.lock().get(&tree).cloned();
                match slot {
                    Some(slot) => slot.get_or_init(|| KeyOrder::index(info)).clone(),
                    None => KeyOrder::index(info),
                }
            }
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

    /// Makes a tree: a table's, keyed by rowid, or an index's (or a WITHOUT ROWID table's), keyed
    /// by record in the order its first cursor brings. Its root page.
    pub fn make_tree(&self, table: bool) -> Result<i64> {
        let mut d = self.d.lock();
        let id = if table {
            self.db.make_tree(&mut d, KeyOrder::rowid().comparator())?
        } else {
            let slot = Arc::new(OnceLock::new());
            let mut orders = self.storage.orders.lock();
            let id = self.db.make_tree(&mut d, late(slot.clone()))?;
            orders.insert(id, slot);
            id
        };
        i64::try_from(id).map_err(|_| LimboError::IntegerOverflow)
    }

    /// Drops the tree at `root_page` (DROP TABLE, DROP INDEX): durable before it returns, as
    /// decision 12 makes it, not undone by a rollback.
    pub fn drop_tree(&self, root_page: i64) -> Result<()> {
        let tree = tree_of(root_page)?;
        self.db.drop_tree(&mut self.d.lock(), tree)?;
        self.storage.orders.lock().remove(&tree);
        Ok(())
    }

    /// Removes every row of the tree at `root_page` in the connection's transaction: how many.
    pub fn clear(&self, root_page: i64, pager: Arc<Pager>) -> Result<u64> {
        let tree = tree_of(root_page)?;
        let slot = self.storage.orders.lock().get(&tree).cloned();
        let order = match slot.as_ref().and_then(|s| s.get()) {
            Some(order) => order.clone(),
            // A table's tree, or an index's no cursor has opened: empty.
            None => KeyOrder::rowid(),
        };
        let mut cursor = VersionedCursor::new(
            self.db.clone(),
            tree,
            order,
            Access::Write(self.txn()?),
            pager,
        )?;
        cursor.clear_rows()
    }

    /// Opens a savepoint for a write statement in an explicit transaction (SQLite's statement
    /// journal), so the statement can be undone alone (`AC-NS.2.3`'s savepoints).
    pub fn begin_statement(&self) -> Result<()> {
        let shared = self.txn()?;
        let sp = shared
            .lock()
            .as_ref()
            .ok_or_else(|| LimboError::TxError("versioned storage: no transaction".into()))?
            .savepoint();
        self.statements.lock().push(sp);
        Ok(())
    }

    /// Ends the newest statement's savepoint: kept, or rolled back to (its changes undone, the
    /// transaction's earlier ones kept).
    pub fn end_statement(&self, rollback: bool) -> Result<()> {
        let Some(sp) = self.statements.lock().pop() else {
            return Ok(());
        };
        if !rollback {
            return Ok(());
        }
        let Some(shared) = self.txn.lock().clone() else {
            return Ok(());
        };
        let mut guard = shared.lock();
        let Some(txn) = guard.as_mut() else {
            return Ok(());
        };
        self.db.rollback_to(&mut self.d.lock(), txn, sp)
    }

    /// Ends the connection's transaction, if one is open: commits it (durable before it returns),
    /// or rolls it back. A cursor still holding it is refused its next write.
    pub fn finish(&self, rollback: bool) -> Result<()> {
        self.statements.lock().clear();
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
            self.db
                .commit(txn, &mut d, self.db.lane(), true)
                .map(|_| ())
        }
    }
}
