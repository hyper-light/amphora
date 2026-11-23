//! PL.8 (docs/design/planner.md decision 19): what sizes the join search, and what it did.
//!
//! The budget picks the algorithm; it never cuts a search short. While the connected subsets the
//! exact search builds fit it, the exact search plans the join (DPccp, then the bushy pairs);
//! past it the greedy search does (Neumann and Radke, "Adaptive Optimization of Very Large Join
//! Queries", SIGMOD 2018). Either way the query gets a whole plan.
//!
//! The rules (`JoinSearchBudget`), behind a switch so they can be measured against each other:
//! - `Fixed`: Neumann and Radke's 10,000 subsets (`JOIN_SEARCH_BUDGET`), the control.
//! - `RunShare`: planning may take as long as the run. This is the rent-or-buy bound (Karlin,
//!   Manasse, Rudolph and Sleator, "Competitive snoopy caching", Algorithmica 3(1), 1988):
//!   spending on the search at most what the run costs keeps planning and running together within
//!   twice the better choice, whatever the search would have saved. The run's time is the
//!   left-to-right plan's estimated cost, an upper bound on the best plan's estimate, so an
//!   estimate the search would lower never starves it; in row visits, times the database's
//!   measured nanoseconds a row visit (`learned_costs`). The budget is that time over the measured
//!   nanoseconds a candidate (a subset or a bushy pair) takes (`SearchRate`).
//! - `RunShareByExecutions`: `RunShare` times the runs the plan is expected to serve, so a
//!   template that runs often may plan longer. A prepared statement planned again after r runs is
//!   expected to serve r more (Gott, "Implications of the Copernican principle for our future
//!   prospects", Nature 363, 1993: seen at a random point of a run of unknown length, the median of
//!   what remains is what has passed); a statement planned for the first time, or a custom plan
//!   made for one run's values, serves one.
//!
//! Every rule allows at least the tables' count squared: the greedy search tries that many joins,
//! so the exact search costs no more wherever its subsets fit that. Until the database has timed a
//! row visit and a candidate, the derived rules take the control's budget.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::join::JOIN_SEARCH_BUDGET;
use super::learned_costs::{LearnedCosts, Work};
use crate::JoinSearchBudget;

/// The rule and the measured rates a statement's join searches are sized by.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SearchBudget {
    pub rule: JoinSearchBudget,
    /// The database's measured nanoseconds a row visit, None before one was timed.
    pub nanos_per_row: Option<f64>,
    /// The database's measured nanoseconds a candidate of the exact search, None before one ran.
    pub nanos_per_candidate: Option<f64>,
    /// The runs the plan is expected to serve, at least one.
    pub executions: f64,
    /// Amphora (PL.8, T-PL.8.10, lane pl8-17): the fast plan, its exact search to follow
    /// (`background_search.rs`): the budget is the joins the greedy search tries, the tables' count
    /// squared, so the fast plan costs no more than the greedy search would.
    pub fast: bool,
}

/// Which search planned a join.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Algorithm {
    Exact,
    Greedy,
}

impl SearchBudget {
    /// The control: today's fixed budget, whatever was measured.
    pub(crate) const FIXED: Self = Self {
        rule: JoinSearchBudget::Fixed,
        nanos_per_row: None,
        nanos_per_candidate: None,
        executions: 1.0,
        fast: false,
    };

    /// The budget `rule` takes with `costs`' measured rates.
    pub(crate) fn new(
        rule: JoinSearchBudget,
        costs: &LearnedCosts,
        expected_runs: Option<u64>,
    ) -> Self {
        Self {
            rule,
            nanos_per_row: costs.nanos_per(Work::Row),
            nanos_per_candidate: costs.search().nanos_per_candidate(),
            executions: expected_runs.map_or(1.0, |runs| runs as f64).max(1.0),
            fast: false,
        }
    }

    /// This budget, or the fast plan's where `fast` (`SearchBudget::fast`).
    pub(crate) fn fast(self, fast: bool) -> Self {
        Self { fast, ..self }
    }

    /// The subsets the exact search may build for a join of `tables` tables whose left-to-right
    /// plan costs `bound_cost` row visits (None when there is none).
    pub(crate) fn subsets(&self, bound_cost: Option<f64>, tables: usize) -> usize {
        if self.fast {
            return tables.saturating_mul(tables);
        }
        let derived = match self.rule {
            JoinSearchBudget::Fixed => return JOIN_SEARCH_BUDGET,
            JoinSearchBudget::RunShare => self.derived(bound_cost, 1.0),
            JoinSearchBudget::RunShareByExecutions => self.derived(bound_cost, self.executions),
        };
        derived
            .unwrap_or(JOIN_SEARCH_BUDGET)
            .max(tables.saturating_mul(tables))
    }

    fn derived(&self, bound_cost: Option<f64>, executions: f64) -> Option<usize> {
        let running = bound_cost? * self.nanos_per_row? * executions.max(1.0);
        let per_candidate = self.nanos_per_candidate.filter(|nanos| *nanos > 0.0)?;
        let candidates = running / per_candidate;
        // `as` saturates: an unbounded run allows every subset.
        (!candidates.is_nan() && candidates >= 0.0).then_some(candidates as usize)
    }
}

/// What the join searches did, summed: a statement's (`end_statement`) or a database's
/// (`Database::join_search_stats`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JoinSearchStats {
    /// Joins the exact search planned.
    pub exact_searches: u64,
    /// The candidates they tried: subsets built and bushy pairs paired.
    pub exact_candidates: u64,
    /// Their time, nanoseconds.
    pub exact_nanos: u64,
    /// Joins whose subsets passed the budget, planned by the greedy search.
    pub greedy_searches: u64,
    /// Their time, nanoseconds.
    pub greedy_nanos: u64,
}

impl JoinSearchStats {
    const ZERO: Self = Self {
        exact_searches: 0,
        exact_candidates: 0,
        exact_nanos: 0,
        greedy_searches: 0,
        greedy_nanos: 0,
    };
}

thread_local! {
    /// The searches of the statement this thread is planning, taken at its end.
    static STATEMENT: Cell<JoinSearchStats> = const { Cell::new(JoinSearchStats::ZERO) };
}

fn nanos(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
}

/// Notes a join's search: which algorithm planned it, its candidates and its time.
pub(crate) fn note_search(algorithm: Algorithm, candidates: usize, elapsed: Duration) {
    let candidates = u64::try_from(candidates).unwrap_or(u64::MAX);
    let reason = match algorithm {
        Algorithm::Exact => "the connected subsets fit the budget",
        Algorithm::Greedy => "the connected subsets passed the budget",
    };
    tracing::debug!(
        target: "amphora::join_search",
        ?algorithm,
        candidates,
        nanos = nanos(elapsed),
        reason,
        "join planned"
    );
    STATEMENT.with(|cell| {
        let mut stats = cell.get();
        match algorithm {
            Algorithm::Exact => {
                stats.exact_searches = stats.exact_searches.saturating_add(1);
                stats.exact_candidates = stats.exact_candidates.saturating_add(candidates);
                stats.exact_nanos = stats.exact_nanos.saturating_add(nanos(elapsed));
            }
            Algorithm::Greedy => {
                stats.greedy_searches = stats.greedy_searches.saturating_add(1);
                stats.greedy_nanos = stats.greedy_nanos.saturating_add(nanos(elapsed));
            }
        }
        cell.set(stats);
    });
}

/// The searches noted on this thread since the last call: the statement just planned.
pub(crate) fn end_statement() -> JoinSearchStats {
    STATEMENT.with(|cell| cell.replace(JoinSearchStats::ZERO))
}

/// A database's join searches, summed: the candidates' measured rate and the instruments.
#[derive(Debug, Default)]
pub(crate) struct SearchRate {
    exact_searches: AtomicU64,
    exact_candidates: AtomicU64,
    exact_nanos: AtomicU64,
    greedy_searches: AtomicU64,
    greedy_nanos: AtomicU64,
}

impl SearchRate {
    pub(crate) fn add(&self, stats: JoinSearchStats) {
        let add = |counter: &AtomicU64, value: u64| {
            if value == 0 {
                return;
            }
            // A saturating add: the counters are instruments, read alone (`Relaxed`).
            let mut old = counter.load(Ordering::Relaxed);
            while let Err(seen) = counter.compare_exchange_weak(
                old,
                old.saturating_add(value),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                old = seen;
            }
        };
        add(&self.exact_searches, stats.exact_searches);
        add(&self.exact_candidates, stats.exact_candidates);
        add(&self.exact_nanos, stats.exact_nanos);
        add(&self.greedy_searches, stats.greedy_searches);
        add(&self.greedy_nanos, stats.greedy_nanos);
    }

    /// Nanoseconds a candidate of the exact search, None before one was timed.
    pub(crate) fn nanos_per_candidate(&self) -> Option<f64> {
        let candidates = self.exact_candidates.load(Ordering::Relaxed);
        let nanos = self.exact_nanos.load(Ordering::Relaxed);
        (candidates > 0 && nanos > 0).then(|| nanos as f64 / candidates as f64)
    }

    pub(crate) fn stats(&self) -> JoinSearchStats {
        JoinSearchStats {
            exact_searches: self.exact_searches.load(Ordering::Relaxed),
            exact_candidates: self.exact_candidates.load(Ordering::Relaxed),
            exact_nanos: self.exact_nanos.load(Ordering::Relaxed),
            greedy_searches: self.greedy_searches.load(Ordering::Relaxed),
            greedy_nanos: self.greedy_nanos.load(Ordering::Relaxed),
        }
    }
}

impl crate::Database {
    /// Amphora (PL.8): this database's join searches so far: which algorithm planned each join,
    /// the exact search's candidates and the time each took.
    pub fn join_search_stats(&self) -> JoinSearchStats {
        self.learned_costs.search().stats()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measured(rule: JoinSearchBudget, executions: f64) -> SearchBudget {
        SearchBudget {
            rule,
            nanos_per_row: Some(10.0),
            nanos_per_candidate: Some(1_000.0),
            executions,
            fast: false,
        }
    }

    /// Do: a fast budget for 8 tables under each rule. Expect: 64 subsets, the joins the greedy
    /// search tries, whatever the rule or the run (T-PL.8.10, lane pl8-17).
    #[test]
    fn t_pl_8_10_the_fast_budget_is_the_greedy_search_s_joins() {
        for rule in [
            JoinSearchBudget::Fixed,
            JoinSearchBudget::RunShare,
            JoinSearchBudget::RunShareByExecutions,
        ] {
            let budget = measured(rule, 1_000.0).fast(true);
            assert_eq!(budget.subsets(Some(1.0e9), 8), 64);
            assert_eq!(
                budget.fast(false).subsets(Some(1.0e9), 8),
                measured(rule, 1_000.0).subsets(Some(1.0e9), 8)
            );
        }
    }

    // PL.8 (decision 19): the control is today's budget, whatever was measured.
    #[test]
    fn pl8_the_fixed_rule_is_the_control() {
        let budget = measured(JoinSearchBudget::Fixed, 1.0);
        assert_eq!(budget.subsets(Some(1.0), 3), JOIN_SEARCH_BUDGET);
        assert_eq!(budget.subsets(Some(1e12), 3), JOIN_SEARCH_BUDGET);
        assert_eq!(SearchBudget::FIXED.subsets(None, 70), JOIN_SEARCH_BUDGET);
    }

    // PL.8: planning may take as long as the run: 10^8 row visits at 10 ns are a second, a
    // million candidates of a microsecond.
    #[test]
    fn pl8_the_run_share_rule_divides_the_runs_time_by_a_candidates() {
        let budget = measured(JoinSearchBudget::RunShare, 50.0);
        assert_eq!(budget.subsets(Some(1e8), 3), 1_000_000);
        assert_eq!(budget.subsets(Some(1e4), 3), 100);
    }

    // PL.8: the greedy search tries the tables' count squared, so the exact search may always.
    #[test]
    fn pl8_a_cheap_run_still_allows_the_greedy_searchs_work() {
        let budget = measured(JoinSearchBudget::RunShare, 1.0);
        assert_eq!(budget.subsets(Some(1.0), 8), 64);
    }

    // PL.8: a template expected to run often may plan longer.
    #[test]
    fn pl8_expected_runs_multiply_the_budget() {
        let once = measured(JoinSearchBudget::RunShareByExecutions, 1.0);
        let often = measured(JoinSearchBudget::RunShareByExecutions, 1_000.0);
        assert_eq!(once.subsets(Some(1e4), 3), 100);
        assert_eq!(often.subsets(Some(1e4), 3), 100_000);
    }

    // PL.8: until a row visit and a candidate were timed, the derived rules take the control's.
    #[test]
    fn pl8_unmeasured_rates_take_the_control() {
        let mut budget = measured(JoinSearchBudget::RunShare, 1.0);
        budget.nanos_per_candidate = None;
        assert_eq!(budget.subsets(Some(1e4), 3), JOIN_SEARCH_BUDGET);
        let budget = measured(JoinSearchBudget::RunShare, 1.0);
        assert_eq!(budget.subsets(None, 3), JOIN_SEARCH_BUDGET);
        assert_eq!(budget.subsets(Some(f64::INFINITY), 3), usize::MAX);
    }

    // PL.8: the statement's searches fold into the database's rate.
    #[test]
    fn pl8_searches_sum_into_the_rate() {
        let _ = end_statement();
        note_search(Algorithm::Exact, 40, Duration::from_nanos(4_000));
        note_search(Algorithm::Greedy, 0, Duration::from_nanos(7));
        let statement = end_statement();
        assert_eq!(statement.exact_searches, 1);
        assert_eq!(statement.greedy_searches, 1);
        assert_eq!(end_statement(), JoinSearchStats::default());
        let rate = SearchRate::default();
        assert_eq!(rate.nanos_per_candidate(), None);
        rate.add(statement);
        assert_eq!(rate.nanos_per_candidate(), Some(100.0));
        assert_eq!(rate.stats(), statement);
    }
}
