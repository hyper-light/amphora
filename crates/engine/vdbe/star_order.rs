//! Amphora (QT.5, T-QT.5.11; `docs/design/planner.md`, "Decision (lane qs5-18): a star join's
//! filters checked in their measured order"): the order a fact row checks its star join's Bloom
//! filters in, kept as the rows run. The planner orders them by its estimates of the rows each
//! passes (`optimizer/lip.rs`); an estimate can be wrong by orders of magnitude, and every check a
//! row makes before the filter that rejects it is wasted. So the probe counts each filter's checks
//! and passes and keeps the filters sorted by their measured share of rows passed, the lowest
//! first: for filters of equal cost, the order that checks fewest filters per row (Babu, Motwani,
//! Munagala, Nishizawa and Widom, "Adaptive Ordering of Pipelined Stream Filters", SIGMOD 2004,
//! §2; Zhu, Potti, Saurabh and Patel, "Looking Ahead Makes Query Plans Robust", VLDB 2017, §4.3).
//!
//! A filter's share is estimated by Laplace's rule of succession, `(passes + 1) / (checks + 2)`,
//! so a filter never yet checked counts as passing half its rows, not as a division by zero. The
//! order moves on every row, as each observation arrives: no batch, no interval. A filter checked
//! after another is measured only on the rows that one passed; for filters whose rejections are
//! independent the share is the same, and A-Greedy's correlated case is left for its own step.

/// The filters of one star probe, by their position in its instruction, in their measured order.
#[derive(Debug, Clone)]
pub(crate) struct StarOrder {
    /// Each filter's position in the instruction, the one to check first first.
    order: Vec<usize>,
    /// By position in the instruction: the rows each filter checked, and those it passed.
    checks: Vec<u64>,
    passes: Vec<u64>,
}

impl StarOrder {
    /// `filters` filters in the planner's order, none yet checked.
    pub(crate) fn new(filters: usize) -> Self {
        Self {
            order: (0..filters).collect(),
            checks: vec![0; filters],
            passes: vec![0; filters],
        }
    }

    /// The filter to check `nth`, by its position in the instruction.
    pub(crate) fn nth(&self, nth: usize) -> Option<usize> {
        self.order.get(nth).copied()
    }

    pub(crate) fn len(&self) -> usize {
        self.order.len()
    }

    /// Counts a check of the filter at `filter` (its position in the instruction), passed or not.
    /// The order moves only in [Self::settle], once the row's checks are done.
    pub(crate) fn observe(&mut self, filter: usize, passed: bool) {
        if let Some(checks) = self.checks.get_mut(filter) {
            *checks = checks.saturating_add(1);
        }
        if passed {
            if let Some(passes) = self.passes.get_mut(filter) {
                *passes = passes.saturating_add(1);
            }
        }
    }

    /// Whether filter `a` has passed a smaller share of its rows than filter `b`, each share by the
    /// rule of succession, compared by cross-multiplying (exact, in 128 bits).
    fn passes_less(&self, a: usize, b: usize) -> bool {
        let share = |f: usize| {
            (
                u128::from(self.passes.get(f).copied().unwrap_or(0)).saturating_add(1),
                u128::from(self.checks.get(f).copied().unwrap_or(0)).saturating_add(2),
            )
        };
        let ((pa, ca), (pb, cb)) = (share(a), share(b));
        pa.saturating_mul(cb) < pb.saturating_mul(ca)
    }

    /// Sorts the filters by their measured shares, the lowest first, keeping the order of filters
    /// whose shares are equal. An insertion sort: one row's checks move the order by little, so it
    /// is linear in the filters on all but the rows that reorder it.
    pub(crate) fn settle(&mut self) {
        for i in 1..self.order.len() {
            let mut j = i;
            while let Some(k) = j.checked_sub(1) {
                let (Some(&here), Some(&before)) = (self.order.get(j), self.order.get(k)) else {
                    break;
                };
                if !self.passes_less(here, before) {
                    break;
                }
                self.order.swap(j, k);
                j = k;
            }
        }
    }

    /// The order, for the tests.
    #[cfg(test)]
    pub(crate) fn order(&self) -> &[usize] {
        &self.order
    }
}

#[cfg(test)]
mod tests {
    use super::StarOrder;

    /// Runs `rows` rows through `order`, each filter passing a row when `passes(filter, row)`,
    /// checking in the order kept and stopping at the first that rejects: the checks made.
    fn run(order: &mut StarOrder, rows: u64, passes: impl Fn(usize, u64) -> bool) -> u64 {
        let mut checks = 0;
        for row in 0..rows {
            for nth in 0..order.len() {
                let Some(filter) = order.nth(nth) else {
                    break;
                };
                checks += 1;
                let passed = passes(filter, row);
                order.observe(filter, passed);
                if !passed {
                    break;
                }
            }
            order.settle();
        }
        checks
    }

    /// T-QT.5.11. Do: three filters in the planner's order passing 9, 8 and 1 rows in 10, over
    /// 1,000 rows. Expect: the one passing 1 in 10 moves first, and the rows take fewer checks
    /// than in the planner's order.
    #[test]
    fn t_qt_5_11_the_filter_that_passes_fewest_rows_moves_first() {
        let share = [9, 8, 1];
        let passes = |filter: usize, row: u64| row % 10 < share[filter];
        let mut measured = StarOrder::new(3);
        let checks = run(&mut measured, 1_000, passes);
        assert_eq!(measured.nth(0), Some(2));
        // The planner's order: each row checks filters 0, 1 and 2 until one rejects it.
        let planned: u64 = (0..1_000u64)
            .map(|row| {
                let mut n = 0;
                for (filter, _) in share.iter().enumerate() {
                    n += 1;
                    if !passes(filter, row) {
                        break;
                    }
                }
                n
            })
            .sum();
        assert!(checks < planned, "measured {checks}, planned {planned}");
    }

    /// T-QT.5.11. Do: two filters of equal shares, and filters never checked. Expect: the planner's
    /// order kept (a tie moves nothing), and an empty probe has no filter to check.
    #[test]
    fn t_qt_5_11_equal_shares_keep_the_planners_order() {
        let mut order = StarOrder::new(2);
        run(&mut order, 100, |_, row| row % 2 == 0);
        assert_eq!(order.order(), &[0, 1]);
        let mut unchecked = StarOrder::new(3);
        unchecked.settle();
        assert_eq!(unchecked.order(), &[0, 1, 2]);
        assert_eq!(StarOrder::new(0).nth(0), None);
    }

    /// T-QT.5.11. Do: a filter that passes every row for 500 rows and then none, beside one passing
    /// half. Expect: the order follows the change: the half filter first while the other passes
    /// all, then the other first once its share falls below a half.
    #[test]
    fn t_qt_5_11_the_order_follows_a_change_in_the_rows() {
        let passes = |filter: usize, row: u64| match filter {
            0 => row < 500,
            _ => row % 2 == 0,
        };
        let mut order = StarOrder::new(2);
        run(&mut order, 500, passes);
        assert_eq!(order.order(), &[1, 0]);
        let mut later = order.clone();
        // Rows 500.. reach filter 0 only when filter 1 passes them, so it falls slowly; run on
        // until it has seen more rejections than passes.
        let mut rows = 500;
        while later.order() != [0, 1] && rows < 10_000 {
            for nth in 0..later.len() {
                let Some(filter) = later.nth(nth) else {
                    break;
                };
                let passed = passes(filter, rows);
                later.observe(filter, passed);
                if !passed {
                    break;
                }
            }
            later.settle();
            rows += 1;
        }
        assert_eq!(later.order(), &[0, 1], "after {rows} rows");
    }
}
