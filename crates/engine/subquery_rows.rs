//! Amphora (PL.13, lane replan-6: a stored subquery result's materialization point;
//! `docs/design/planner.md`, "Decision (lane pl13)", "Stored subquery results"): the rows an
//! uncorrelated FROM-clause subquery's stored result held once its fill finished, where they fell
//! outside the band of the planner's estimate. The planner reads the count where it sets the
//! subquery's rows (`optimizer::base_row_estimate`), and a plan made on a count re-plans when the
//! count changes, as one made on a filter's share does (`filter_shares`). Only with adaptive joins
//! on (`Connection::set_adaptive_joins`): off, nothing is kept or read.
//!
//! The key is the subquery's identity ([`identity`]): a fingerprint of its syntax, taken when the
//! statement is planned. The plan's own printed form is no key: optimizing and emitting the plan
//! rewrite it (terms consumed, a destination set), so the plan the fill runs is not the plan the
//! estimate read.
//!
//! Bound: an entry for each subquery some live plan read. The store holds its entries weakly; the
//! plans that read one, and the fills that count it, hold it. Dead entries are swept once the map
//! has doubled since the last sweep's live count, so each insert pays a constant share of the sweep
//! (geometric growth's amortization, as `Vec` grows).

use crate::sync::atomic::{AtomicU64, Ordering};
use crate::sync::{Arc, Mutex, Weak};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// The rows a subquery's stored result held, as its fill counted them; none until one did.
#[derive(Debug)]
pub struct CountedRows {
    /// The count's bits as an `f64`; [NO_ROWS] until counted.
    bits: AtomicU64,
}

/// A format value: NaN's bits, which no count has.
const NO_ROWS: u64 = 0x7ff8_0000_0000_0000;

impl CountedRows {
    fn new() -> Self {
        Self {
            bits: AtomicU64::new(NO_ROWS),
        }
    }

    pub(crate) fn bits(&self) -> u64 {
        self.bits.load(Ordering::Acquire)
    }

    /// The counted rows, `None` until a fill counted them.
    pub(crate) fn get(&self) -> Option<f64> {
        let bits = self.bits();
        (bits != NO_ROWS).then(|| f64::from_bits(bits))
    }

    /// Stores a count; true when it differs from the one held.
    pub(crate) fn set(&self, rows: u64) -> bool {
        let bits = (rows as f64).to_bits();
        self.bits.swap(bits, Ordering::AcqRel) != bits
    }
}

/// Every counted subquery result of the database, by the subquery's identity.
#[derive(Debug, Default)]
pub struct SubqueryRows {
    entries: Mutex<Entries>,
}

#[derive(Debug, Default)]
struct Entries {
    map: HashMap<u64, Weak<CountedRows>>,
    live_at_sweep: usize,
}

impl SubqueryRows {
    pub fn new() -> Self {
        Self::default()
    }

    /// The rows of the subquery `identity` names, made unset the first time.
    pub(crate) fn rows(&self, identity: u64) -> Arc<CountedRows> {
        let mut entries = self.entries.lock();
        if let Some(rows) = entries.map.get(&identity).and_then(Weak::upgrade) {
            return rows;
        }
        if entries.map.len() >= entries.live_at_sweep.saturating_mul(2).max(1) {
            entries.map.retain(|_, held| held.strong_count() > 0);
            entries.live_at_sweep = entries.map.len();
        }
        let rows = Arc::new(CountedRows::new());
        entries.map.insert(identity, Arc::downgrade(&rows));
        rows
    }

    /// Whether a fill has counted the rows `identity` names, read without making an entry
    /// (PL.13, lane replan-11): the join search asks it of every set of tables it estimates.
    pub(crate) fn counted(&self, identity: u64) -> bool {
        self.entries
            .lock()
            .map
            .get(&identity)
            .and_then(Weak::upgrade)
            .is_some_and(|rows| rows.get().is_some())
    }

    /// How many entries the store holds, dead ones not yet swept included.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.lock().map.len()
    }
}

/// A subquery's identity: the fingerprint of its syntax as parsed, the same each time the same
/// text is planned, so every reference to one CTE, and every statement that writes the same
/// subquery, share one count. `None` for a subquery that reads a parameter, whose rows change with
/// the value bound. The parameter test reads the syntax's printed form for a `Variable(` node; a
/// string literal holding that text is taken for one too, and that subquery is only not checked.
pub(crate) fn identity(select: &impl std::fmt::Debug) -> Option<u64> {
    let syntax = format!("{select:?}");
    if syntax.contains("Variable(") {
        return None;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    syntax.hash(&mut hasher);
    Some(hasher.finish())
}

/// An uncorrelated `x IN (SELECT ...)` subquery's identity (PL.13, lane replan-10): its syntax's
/// fingerprint as [identity] takes it, apart from the same text's as a FROM-clause subquery, whose
/// count is its rows, where this one's is the distinct values its ephemeral index keeps.
pub(crate) fn in_identity(select: &impl std::fmt::Debug) -> Option<u64> {
    identity(&("IN", select))
}

/// The identity of a join's whole result (PL.13, lane replan-10): its tables' names, in any order,
/// and the fingerprint of its WHERE and ON terms (`filter_shares::fingerprint`, order-free).
/// `None` for terms that read a parameter, whose rows change with the value bound, or for no term.
/// Its count is the rows a sorter fed by the join held; the planner reads it for the set of all
/// the join's tables (`cardinality::JoinedSetRows::rows`). A LEFT JOIN (lane replan-11) keeps its
/// right table's unmatched rows, which an inner join of the same terms drops, and an ON term of it
/// keeps rows a WHERE term of the same text would drop; a semi or anti join's table (an EXISTS or
/// NOT EXISTS unnested) keeps or drops the other tables' rows and adds none: each such table and
/// term adds a part of its own, so an inner join's identity is the one [prefix_identity] gives the
/// same tables.
pub(crate) fn join_identity(
    tables: &[crate::translate::plan::JoinedTable],
    where_clause: &[crate::translate::plan::WhereTerm],
) -> Option<u64> {
    let mut each = Vec::new();
    for term in where_clause {
        let fingerprint = crate::filter_shares::term_fingerprint(&term.expr)?;
        each.push(if term.origin.is_outer_join() {
            identity(&("ON", fingerprint))?
        } else {
            fingerprint
        });
    }
    let mut names = Vec::new();
    for joined in tables {
        let name = crate::util::normalize_ident(joined.table.get_name());
        match joined.join_info.as_ref() {
            Some(info) if info.is_outer() => each.push(identity(&("LEFT", &name))?),
            Some(info) if info.is_semi() => each.push(identity(&("SEMI", &name))?),
            Some(info) if info.is_anti() => each.push(identity(&("ANTI", &name))?),
            _ => {}
        }
        names.push(name);
    }
    join_identity_of(names, each)
}

/// [join_identity] of the tables named `names` (normalized) and the terms whose
/// [`term_fingerprint`](crate::filter_shares::term_fingerprint)s are `terms`, both in any order.
pub(crate) fn join_identity_of(mut names: Vec<String>, terms: Vec<u64>) -> Option<u64> {
    let terms = crate::filter_shares::combine(terms)?;
    names.sort_unstable();
    identity(&("JOIN", names, terms))
}

/// The identity of the rows of a join prefix (PL.13, lane replan-11: a build fed by a materialized
/// input): the tables `set` holds and every WHERE term that reads only them, keyed as a whole
/// join's is ([join_identity]), so the prefix of one query and the whole of another that joins the
/// same tables by the same terms share one count. `None` for a term reading a parameter among
/// them, or none at all. The planner notes the prefix's estimate under it once the join order is
/// chosen (`optimizer::note_materialized_prefixes`), reads a count stored there in its place
/// (`cardinality::JoinedSetRows::rows`), and the materialization counts its rows against it
/// (`main_loop::join_prefix_check`).
pub(crate) fn prefix_identity(
    table_references: &crate::translate::plan::TableReferences,
    subqueries: &[crate::translate::plan::NonFromClauseSubquery],
    where_clause: &[crate::translate::plan::WhereTerm],
    set: &crate::translate::planner::TableMask,
) -> Option<u64> {
    let joined = table_references.joined_tables();
    let mut names = Vec::new();
    for table_idx in set.iter() {
        names.push(crate::util::normalize_ident(
            joined.get(table_idx)?.table.get_name(),
        ));
    }
    let mut terms = Vec::new();
    for term in where_clause {
        let mask = crate::translate::planner::table_mask_from_expr(
            &term.expr,
            table_references,
            subqueries,
        )
        .ok()?;
        if set.contains_all_set_bits_of(&mask) {
            terms.push(crate::filter_shares::term_fingerprint(&term.expr)?);
        }
    }
    join_identity_of(names, terms)
}

/// A subquery's rows a plan read: its identity, where a count is stored, the count's bits when the
/// plan read it, and the rows planned on (the counted ones where a fill counted them, else the
/// subquery's own estimate).
#[derive(Debug, Clone)]
pub struct PlannedSubquery {
    pub(crate) identity: u64,
    pub(crate) rows: Arc<CountedRows>,
    pub(crate) read: u64,
    pub(crate) planned_rows: f64,
}

impl PlannedSubquery {
    /// Whether a fill has stored another count since the plan read it.
    pub(crate) fn moved(&self) -> bool {
        self.rows.bits() != self.read
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PL.13 (lane replan-6): a count is unset until stored, a plan that read it sees it move once
    /// another is stored, the same text has one identity and a parameter none, and the store
    /// sweeps the entries no plan holds.
    #[test]
    fn pl_13_subquery_rows_move_and_dead_entries_sweep() {
        let store = SubqueryRows::new();
        let rows = store.rows(7);
        assert!(rows.get().is_none());
        let planned = PlannedSubquery {
            identity: 7,
            rows: rows.clone(),
            read: rows.bits(),
            planned_rows: 50.0,
        };
        assert!(!planned.moved());
        assert!(rows.set(5000));
        assert!(!rows.set(5000), "the same count stored again is no change");
        assert!(planned.moved());
        assert_eq!(store.rows(7).get(), Some(5000.0));
        assert_eq!(identity(&"SELECT 1"), identity(&"SELECT 1"));
        assert_ne!(identity(&"SELECT 1"), identity(&"SELECT 2"));
        assert_eq!(identity(&"Variable(\"1\")"), None);
        assert_ne!(in_identity(&"SELECT 1"), identity(&"SELECT 1"));
        assert_eq!(in_identity(&"SELECT 1"), in_identity(&"SELECT 1"));
        assert_eq!(in_identity(&"Variable(\"1\")"), None);
        drop((planned, rows));
        for identity in 100..200 {
            drop(store.rows(identity));
        }
        assert!(store.len() <= 2, "dead entries kept: {}", store.len());
    }
}
