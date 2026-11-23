//! Amphora (PL.13, a filtered build's materialization point; `docs/design/planner.md`, "Decision
//! (lane replan-5)"): the share of a table's rows a set of filter terms keeps, as a finished hash
//! join's build over the filtered table counted it, where it fell outside the band of the share the
//! planner's sample gave (decision 21). The planner reads it where the sample sets the terms' share
//! (`cardinality::sampled_filters`), and a plan made on a share re-plans when the share changes, as
//! one made on a table's row count does when the count moves (`row_changes::PlannedTable::moved`).
//! Only with adaptive joins on (`Connection::set_adaptive_joins`): off, nothing is kept or read.
//!
//! Bound: an entry for each (table, terms) pair some live plan read. The store holds its entries
//! weakly; the plans that read one, and the builds that count it, hold it. Dead entries are swept
//! once the map has doubled since the last sweep's live count, so each insert pays a constant share
//! of the sweep (geometric growth's amortization, as `Vec` grows).

use crate::sync::atomic::{AtomicU64, Ordering};
use crate::sync::{Arc, Mutex, Weak};
use crate::util::normalize_ident;
use amphora_engine_parser::ast;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// The share a set of terms keeps of its table's rows, as a build counted it; none until one did.
#[derive(Debug)]
pub struct FilterShare {
    /// The share's bits; [NO_SHARE] until counted.
    bits: AtomicU64,
}

/// A format value: NaN's bits, which no share in [0, 1] has.
const NO_SHARE: u64 = 0x7ff8_0000_0000_0000;

impl FilterShare {
    fn new() -> Self {
        Self {
            bits: AtomicU64::new(NO_SHARE),
        }
    }

    pub(crate) fn bits(&self) -> u64 {
        self.bits.load(Ordering::Acquire)
    }

    /// The counted share, `None` until a build counted one.
    pub(crate) fn get(&self) -> Option<f64> {
        let bits = self.bits();
        (bits != NO_SHARE).then(|| f64::from_bits(bits))
    }

    /// Stores a counted share; true when it differs from the one held.
    pub(crate) fn set(&self, share: f64) -> bool {
        let bits = share.clamp(0.0, 1.0).to_bits();
        self.bits.swap(bits, Ordering::AcqRel) != bits
    }
}

/// Every counted share of the database, by its table's normalized name and its terms' fingerprint.
#[derive(Debug, Default)]
pub struct FilterShares {
    entries: Mutex<Entries>,
}

#[derive(Debug, Default)]
struct Entries {
    map: HashMap<(String, u64), Weak<FilterShare>>,
    live_at_sweep: usize,
}

impl FilterShares {
    pub fn new() -> Self {
        Self::default()
    }

    /// The share of `table`'s rows the terms `fingerprint` names keep, made unset the first time.
    pub(crate) fn share(&self, table: &str, fingerprint: u64) -> Arc<FilterShare> {
        let key = (normalize_ident(table), fingerprint);
        let mut entries = self.entries.lock();
        if let Some(share) = entries.map.get(&key).and_then(Weak::upgrade) {
            return share;
        }
        if entries.map.len() >= entries.live_at_sweep.saturating_mul(2).max(1) {
            entries.map.retain(|_, held| held.strong_count() > 0);
            entries.live_at_sweep = entries.map.len();
        }
        let share = Arc::new(FilterShare::new());
        entries.map.insert(key, Arc::downgrade(&share));
        share
    }

    /// How many entries the store holds, dead ones not yet swept included.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.lock().map.len()
    }
}

/// The fingerprint of a set of a table's filter terms, whatever their order; `None` for a set
/// that reads a parameter, whose share changes with the value bound (a custom plan's own sample
/// run reads it). Within one statement the terms' table references are the same each time it is
/// planned; two statements naming the table at different places in their FROM clauses keep two
/// entries (their shares are not shared, never wrong).
pub(crate) fn fingerprint<'a>(terms: impl Iterator<Item = &'a ast::Expr>) -> Option<u64> {
    let mut each: Vec<u64> = Vec::new();
    for expr in terms {
        each.push(term_fingerprint(expr)?);
    }
    combine(each)
}

/// One term's part of [fingerprint] (PL.13, lane replan-11): the hash of its printed form; `None`
/// for a term that reads a parameter.
pub(crate) fn term_fingerprint(expr: &ast::Expr) -> Option<u64> {
    let mut reads_parameter = false;
    let _ = crate::translate::expr::walk_expr(expr, &mut |e: &ast::Expr| {
        if matches!(e, ast::Expr::Variable(_)) {
            reads_parameter = true;
        }
        Ok(crate::translate::expr::WalkControl::Continue)
    });
    if reads_parameter {
        return None;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format!("{expr:?}").hash(&mut hasher);
    Some(hasher.finish())
}

/// [fingerprint] of the terms whose [term_fingerprint]s are `each`, in any order; `None` for none.
pub(crate) fn combine(mut each: Vec<u64>) -> Option<u64> {
    if each.is_empty() {
        return None;
    }
    each.sort_unstable();
    each.dedup();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    each.hash(&mut hasher);
    Some(hasher.finish())
}

/// A share a plan read: the table, its terms, the share planned on, and the sample's size where the
/// share came from the sample (`None` where a build counted it).
#[derive(Debug, Clone)]
pub struct PlannedFilter {
    pub(crate) table: String,
    pub(crate) fingerprint: u64,
    pub(crate) share: Arc<FilterShare>,
    pub(crate) read: u64,
    pub(crate) planned_share: f64,
    pub(crate) sample_rows: Option<usize>,
}

impl PlannedFilter {
    /// Whether a build has stored another share since the plan read it.
    pub(crate) fn moved(&self) -> bool {
        self.share.bits() != self.read
    }

    /// The relative standard error of the share planned on: a sample's binomial one,
    /// `sqrt((1 - p) / (p n))` for a share `p` of `n` sample rows (decision 21); a counted share's,
    /// none of its own.
    pub(crate) fn share_error(&self) -> f64 {
        match self.sample_rows {
            Some(n) if n > 0 && self.planned_share > 0.0 => {
                ((1.0 - self.planned_share).max(0.0) / (self.planned_share * n as f64)).sqrt()
            }
            _ => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PL.13 (lane replan-5): a share is unset until stored, a plan that read it sees it move once
    /// another is stored, and the store sweeps the entries no plan holds.
    #[test]
    fn pl_13_filter_share_moves_and_dead_entries_sweep() {
        let shares = FilterShares::new();
        let share = shares.share("T", 7);
        assert!(share.get().is_none());
        let planned = PlannedFilter {
            table: "t".into(),
            fingerprint: 7,
            share: share.clone(),
            read: share.bits(),
            planned_share: 0.01,
            sample_rows: Some(1024),
        };
        assert!(!planned.moved());
        assert!(share.set(0.5));
        assert!(!share.set(0.5), "the same share stored again is no change");
        assert!(planned.moved());
        assert_eq!(
            shares.share("t", 7).get(),
            Some(0.5),
            "names are normalized"
        );
        drop((planned, share));
        for fingerprint in 100..200 {
            drop(shares.share("t", fingerprint));
        }
        assert!(shares.len() <= 2, "dead entries kept: {}", shares.len());
    }
}
