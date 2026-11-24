//! U05/CP-T1.1: one bounded prepared-statement cache for static and dynamic SQL. Checking a
//! statement out removes it until the caller returns it, so stepping and nested row callbacks
//! never hold the cache mutex, and a statement in use is never handed out twice. It keeps at most
//! `amphora_api::TX_MAX` statements and `amphora_api::SQL_MAX` bytes of their text. Returning a
//! statement makes it most recently used; eviction takes the least recently used, until both
//! budgets hold. Oversized entries execute normally and are simply not retained. The recency order
//! is a list linked through the map, so taking, returning and evicting a statement each cost a few
//! map operations, never a search of the order (docs/design/store.md, decision 34). An entry also
//! carries whether the session's validation accepted its text: only the session sets that mark,
//! and it goes with the entry when the entry is evicted, replaced or dropped for a schema change
//! or a restore (decision 36).

use amphora_engine::Statement;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::sync::{Arc, Mutex, MutexGuard};

/// Fast non-cryptographic FNV-1a 64-bit hasher for per-connection statement text lookups
/// (matches the FNV-1a hash used in `store.rs` and `groups.rs`; avoids SipHash overhead on
/// single-connection SQL caches).
#[derive(Clone, Copy, Default)]
struct FnvBuildHasher;

struct FnvHasher(u64);

impl Hasher for FnvHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut h = self.0;
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.0 = h;
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

impl BuildHasher for FnvBuildHasher {
    type Hasher = FnvHasher;

    #[inline]
    fn build_hasher(&self) -> Self::Hasher {
        FnvHasher(0xcbf2_9ce4_8422_2325)
    }
}

#[derive(Debug, thiserror::Error)]
pub(super) enum Error {
    #[error("the prepared-statement cache lock was poisoned")]
    Poisoned,
    #[error("the prepared-statement cache accounting is inconsistent")]
    Accounting,
    #[error("resetting a prepared statement failed: {0}")]
    Reset(String),
}
type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy)]
struct Limits {
    entries: usize,
    sql_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        // Administrative bounds, not a throughput tuning claim: one API batch's statement
        // count and one maximum SQL request's text retained per connection (U05).
        Self {
            entries: amphora_api::TX_MAX,
            sql_bytes: amphora_api::SQL_MAX,
        }
    }
}

/// A kept statement, whether the session's validation accepted its text, and its neighbours in
/// the recency order, linked by flat slab indices (no `Arc<str>` cloning or extra hash lookups on
/// unlink/link). When the MRU entry is checked out on the fast path, `statement` is temporarily
/// `None` and the slot stays in place so returning the same statement needs zero hashing, zero
/// `Arc<str>` refcount ops, and zero list relinking.
struct Slot {
    key: Arc<str>,
    statement: Option<Statement>,
    validated: bool,
    older: Option<usize>,
    newer: Option<usize>,
}

#[derive(Default)]
struct State {
    index: HashMap<Arc<str>, usize, FnvBuildHasher>,
    slots: Vec<Option<Slot>>,
    free: Vec<usize>,
    oldest: Option<usize>,
    newest: Option<usize>,
    /// Slot currently checked out in-place via the MRU fast path (`statement == None`).
    checked_out: Option<usize>,
    /// Stored statements currently available in the cache (excludes `checked_out`).
    live_entries: usize,
    sql_bytes: usize,
    schema: Option<u64>,
}
impl State {
    fn unlink_raw(&mut self, idx: usize) -> Result<Slot> {
        let slot = self
            .slots
            .get_mut(idx)
            .and_then(Option::take)
            .ok_or(Error::Accounting)?;
        match slot.older {
            Some(older) => {
                self.slots
                    .get_mut(older)
                    .and_then(Option::as_mut)
                    .ok_or(Error::Accounting)?
                    .newer = slot.newer;
            }
            None => self.oldest = slot.newer,
        }
        match slot.newer {
            Some(newer) => {
                self.slots
                    .get_mut(newer)
                    .and_then(Option::as_mut)
                    .ok_or(Error::Accounting)?
                    .older = slot.older;
            }
            None => self.newest = slot.older,
        }
        self.free.push(idx);
        Ok(slot)
    }

    fn unlink(&mut self, idx: usize) -> Result<Slot> {
        let slot = self.unlink_raw(idx)?;
        self.live_entries = self.live_entries.checked_sub(1).ok_or(Error::Accounting)?;
        self.sql_bytes = self
            .sql_bytes
            .checked_sub(slot.key.len())
            .ok_or(Error::Accounting)?;
        Ok(slot)
    }

    /// Removes any MRU slot left checked out in-place (e.g., if a caller errored, ran a nested
    /// query, or switched to a different statement) so `index.len() == live_entries` always holds.
    fn flush_checked_out(&mut self) -> Result<()> {
        let Some(idx) = self.checked_out.take() else {
            return Ok(());
        };
        let slot = self.unlink_raw(idx)?;
        if self.index.remove(slot.key.as_ref()) != Some(idx) {
            return Err(Error::Accounting);
        }
        Ok(())
    }

    /// Fast-path checkout of the MRU slot in-place when `sql` matches `self.newest`, avoiding
    /// hash calculation, `HashMap` mutation, list unlink, and `Arc<str>` refcount ops.
    fn take_fast(&mut self, sql: &str) -> Result<Option<Taken>> {
        if self.checked_out.is_none()
            && let Some(idx) = self.newest
            && let Some(slot) = self.slots.get_mut(idx).and_then(Option::as_mut)
            && slot.key.as_ref() == sql
            && let Some(statement) = slot.statement.take()
        {
            self.checked_out = Some(idx);
            self.live_entries = self.live_entries.checked_sub(1).ok_or(Error::Accounting)?;
            self.sql_bytes = self
                .sql_bytes
                .checked_sub(slot.key.len())
                .ok_or(Error::Accounting)?;
            return Ok(Some(Taken {
                slot_idx: Some(idx),
                text: None,
                statement,
                validated: slot.validated,
            }));
        }
        self.flush_checked_out()?;
        self.remove(sql)
    }

    /// Takes `sql`'s statement out with its kept text and mark, its neighbours joined where it was.
    fn remove(&mut self, sql: &str) -> Result<Option<Taken>> {
        let Some(idx) = self.index.remove(sql) else {
            return Ok(None);
        };
        let slot = self.unlink(idx)?;
        let statement = slot.statement.ok_or(Error::Accounting)?;
        Ok(Some(Taken {
            slot_idx: None,
            text: Some(slot.key),
            statement,
            validated: slot.validated,
        }))
    }

    /// Evicts and returns the least recently used statement.
    fn remove_oldest(&mut self) -> Result<Statement> {
        let oldest_idx = self.oldest.ok_or(Error::Accounting)?;
        let slot = self.unlink(oldest_idx)?;
        if self.index.remove(slot.key.as_ref()) != Some(oldest_idx) {
            return Err(Error::Accounting);
        }
        slot.statement.ok_or(Error::Accounting)
    }

    /// Makes `current` the schema identity entries are kept under. A change of schema hands back
    /// every entry kept under the old one, for the caller to drop once the lock is released.
    fn align(&mut self, current: u64) -> Option<State> {
        if self.schema == Some(current) {
            return None;
        }
        let previous = std::mem::take(self);
        self.schema = Some(current);
        Some(previous)
    }

    /// Keeps `statement` as the most recently used, its text making `sql_bytes` in all: the
    /// caller has checked both budgets and taken out any statement kept for the same text.
    fn push(
        &mut self,
        key: Arc<str>,
        statement: Statement,
        validated: bool,
        sql_bytes: usize,
    ) -> Result<()> {
        let idx = if let Some(free_idx) = self.free.pop() {
            free_idx
        } else {
            let idx = self.slots.len();
            self.slots.push(None);
            idx
        };
        let prev_newest = self.newest;
        match prev_newest {
            Some(newest) => {
                self.slots
                    .get_mut(newest)
                    .and_then(Option::as_mut)
                    .ok_or(Error::Accounting)?
                    .newer = Some(idx);
            }
            None => self.oldest = Some(idx),
        }
        self.newest = Some(idx);
        let slot = Slot {
            key: Arc::clone(&key),
            statement: Some(statement),
            validated,
            older: prev_newest,
            newer: None,
        };
        *self.slots.get_mut(idx).ok_or(Error::Accounting)? = Some(slot);
        if self.index.insert(key, idx).is_some() {
            return Err(Error::Accounting);
        }
        self.live_entries = self.live_entries.checked_add(1).ok_or(Error::Accounting)?;
        self.sql_bytes = sql_bytes;
        Ok(())
    }
}

/// A statement taken out of the cache: its fast-path MRU slot index (when checked out in-place),
/// the text it was kept under (handed back to [`Cache::put`] so a slow-path return keys it
/// without copying the text again), the statement, and its validation mark.
pub(super) struct Taken {
    pub(super) slot_idx: Option<usize>,
    pub(super) text: Option<Arc<str>>,
    pub(super) statement: Statement,
    pub(super) validated: bool,
}

#[derive(Default)]
pub(super) struct Cache {
    state: Mutex<State>,
    limits: Limits,
}
impl Cache {
    fn locked(&self) -> Result<MutexGuard<'_, State>> {
        self.state.lock().map_err(|_| Error::Poisoned)
    }

    /// Aligns the cache to the connection's monotonic `schema_identity`, which advances on every
    /// schema replacement or in-place mutation without cloning `Arc<Schema>`.
    pub(super) fn schema(&self, current: u64) -> Result<()> {
        let previous = self.locked()?.align(current);
        drop(previous);
        Ok(())
    }

    /// Takes `sql`'s statement out, with whether the session's validation accepted its text,
    /// under the lock that also makes `schema` current ([`Cache::schema`]).
    pub(super) fn take(&self, sql: &str, schema: u64) -> Result<Option<Taken>> {
        let (previous, taken) = {
            let mut state = self.locked()?;
            let previous = state.align(schema);
            (previous, state.take_fast(sql))
        };
        drop(previous);
        taken
    }

    /// Only successfully executed/prepared statements are returned here. Reset before taking
    /// the lock, and clear parameters so cached statements retain no request values or blobs.
    /// `validated` says the session's validation accepted the text; nothing else may set it.
    /// `current` becomes the schema entries are kept under ([`Cache::schema`]), and the statement
    /// is kept only if it was `prepared` under that same schema. `slot_idx` and `text` come from
    /// [`Taken`] when the statement originated from the cache.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn put(
        &self,
        sql: &str,
        slot_idx: Option<usize>,
        text: Option<Arc<str>>,
        mut statement: Statement,
        validated: bool,
        prepared: u64,
        current: u64,
    ) -> Result<()> {
        if prepared != current {
            self.schema(current)?;
            drop(statement);
            return Ok(());
        }
        statement.reset().map_err(|e| Error::Reset(e.to_string()))?;
        statement.clear_bindings();
        if self.limits.entries == 0 || sql.len() > self.limits.sql_bytes {
            return self.schema(current);
        }
        let mut retired = Vec::new();
        let previous;
        {
            let mut state = self.locked()?;
            previous = state.align(current);
            if let Some(idx) = slot_idx
                && state.checked_out == Some(idx)
                && state.newest == Some(idx)
                && let Some(bytes) = state.sql_bytes.checked_add(sql.len())
                && state.live_entries < self.limits.entries
                && bytes <= self.limits.sql_bytes
                && let Some(slot) = state.slots.get_mut(idx).and_then(Option::as_mut)
                && slot.statement.is_none()
                && slot.key.as_ref() == sql
            {
                slot.statement = Some(statement);
                slot.validated = validated;
                state.checked_out = None;
                state.live_entries = state.live_entries.checked_add(1).ok_or(Error::Accounting)?;
                state.sql_bytes = bytes;
                drop(state);
                drop(previous);
                return Ok(());
            }
            state.flush_checked_out()?;
            if let Some(prior) = state.remove(sql)? {
                retired.push(prior.statement);
            }
            // Both budgets are administrative and finite. At most the entry budget's number
            // of evictions can occur; the extra iteration admits the new statement afterward.
            let attempts = self
                .limits
                .entries
                .checked_add(1)
                .ok_or(Error::Accounting)?;
            let mut admitted = false;
            for _ in 0..attempts {
                let bytes = state
                    .sql_bytes
                    .checked_add(sql.len())
                    .ok_or(Error::Accounting)?;
                if state.live_entries < self.limits.entries && bytes <= self.limits.sql_bytes {
                    let key = text.unwrap_or_else(|| Arc::from(sql));
                    state.push(key, statement, validated, bytes)?;
                    admitted = true;
                    break;
                }
                retired.push(state.remove_oldest()?);
            }
            if !admitted {
                return Err(Error::Accounting);
            }
        }
        // Finalization can touch the engine; no engine work happens while holding the mutex.
        drop(retired);
        drop(previous);
        Ok(())
    }

    pub(super) fn clear(&self) -> Result<()> {
        let previous = {
            let mut state = self.locked()?;
            std::mem::take(&mut *state)
        };
        drop(previous);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amphora_engine::{StepResult, Value};
    use std::num::NonZero;
    use std::path::Path;

    fn put(cache: &Cache, db: &crate::db::Db, sql: &str, statement: Statement) -> Result<()> {
        let schema = db.conn.schema_identity();
        cache.put(sql, None, None, statement, false, schema, schema)
    }

    fn take(cache: &Cache, db: &crate::db::Db, sql: &str) -> Result<Option<(Statement, bool)>> {
        let taken = cache.take(sql, db.conn.schema_identity())?;
        Ok(taken.map(|taken| (taken.statement, taken.validated)))
    }

    #[test]
    fn u05_cached_statements_release_parameters_and_evict_by_both_budgets() -> anyhow::Result<()> {
        let db = crate::db::Db::open_with(
            Path::new("cache-test.db"),
            Arc::new(amphora_engine::MemoryIO::new()),
        )?;
        let cache = Cache {
            state: Mutex::default(),
            limits: Limits {
                entries: 2,
                sql_bytes: 32,
            },
        };
        let mut statement = db.conn.prepare("SELECT ?1")?;
        statement.bind_at(
            NonZero::new(1).ok_or_else(|| anyhow::anyhow!("parameter index"))?,
            Value::from_text("prior request".to_owned()),
        )?;
        put(&cache, &db, "SELECT ?1", statement)?;
        let (mut statement, _) = take(&cache, &db, "SELECT ?1")?
            .ok_or_else(|| anyhow::anyhow!("cached statement missing"))?;
        assert!(matches!(statement.step()?, StepResult::Row));
        assert!(
            matches!(
                statement.row().map(|row| row.get_value(0)),
                Some(Value::Null)
            ),
            "request values are cleared before retention"
        );
        put(&cache, &db, "SELECT ?1", statement)?;
        put(&cache, &db, "SELECT 2", db.conn.prepare("SELECT 2")?)?;
        put(&cache, &db, "SELECT 3", db.conn.prepare("SELECT 3")?)?;
        assert!(
            take(&cache, &db, "SELECT ?1")?.is_none(),
            "least recently used entry was evicted"
        );
        let (statement, _) = take(&cache, &db, "SELECT 2")?
            .ok_or_else(|| anyhow::anyhow!("second statement missing"))?;
        put(&cache, &db, "SELECT 2", statement)?;
        put(&cache, &db, "SELECT 4", db.conn.prepare("SELECT 4")?)?;
        assert!(
            take(&cache, &db, "SELECT 3")?.is_none(),
            "checkout and return refreshes recency"
        );
        assert!(take(&cache, &db, "SELECT 2")?.is_some());
        cache.clear()?;
        assert!(take(&cache, &db, "SELECT 4")?.is_none());
        let bytes = Cache {
            state: Mutex::default(),
            limits: Limits {
                entries: 3,
                sql_bytes: 16,
            },
        };
        put(&bytes, &db, "SELECT 2", db.conn.prepare("SELECT 2")?)?;
        put(&bytes, &db, "SELECT 3", db.conn.prepare("SELECT 3")?)?;
        put(
            &bytes,
            &db,
            "SELECT 12345",
            db.conn.prepare("SELECT 12345")?,
        )?;
        assert!(
            take(&bytes, &db, "SELECT 2")?.is_none() && take(&bytes, &db, "SELECT 3")?.is_none(),
            "SQL byte budget evicts while entry slots remain"
        );
        assert!(take(&bytes, &db, "SELECT 12345")?.is_some());
        Ok(())
    }

    // Kiln plan 01a18d12142c7789a40e959ccdf52bd1, step 2. Do: fill a cache at its own bounds with
    // TX_MAX + 1 statements, taking and returning the first before the last goes in. Expect: the
    // second-oldest evicted, the first kept, TX_MAX held (AC-T1.1.1; store.md, decision 34).
    #[test]
    fn a_full_cache_evicts_its_least_recently_used_statement() -> anyhow::Result<()> {
        let db = crate::db::Db::open_with(
            Path::new("lru-test.db"),
            Arc::new(amphora_engine::MemoryIO::new()),
        )?;
        let cache = Cache::default();
        let max = amphora_api::TX_MAX;
        for n in 0..max {
            let sql = format!("SELECT {n}");
            put(&cache, &db, &sql, db.conn.prepare(&sql)?)?;
        }
        let (first, _) = take(&cache, &db, "SELECT 0")?
            .ok_or_else(|| anyhow::anyhow!("the first statement is missing"))?;
        put(&cache, &db, "SELECT 0", first)?;
        let last = format!("SELECT {max}");
        put(&cache, &db, &last, db.conn.prepare(&last)?)?;
        assert_eq!(
            cache.locked()?.index.len(),
            max,
            "the cache holds its bound"
        );
        assert!(
            take(&cache, &db, "SELECT 1")?.is_none(),
            "the least recently used, the second-oldest, went"
        );
        assert!(
            take(&cache, &db, "SELECT 0")?.is_some(),
            "the first, taken and returned, stayed"
        );
        assert!(take(&cache, &db, &last)?.is_some());
        Ok(())
    }
}
