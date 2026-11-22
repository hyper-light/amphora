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
//!
//! Whole or fading counts (lane qs5-19, `StarFilterOrder::Measured` against `MeasuredFading`, the
//! coordinator's A/B): whole counts weigh every row the run has seen alike, so once the rows change
//! (data loaded in order, a key range that shifts halfway) the order turns only when the new rows
//! outnumber the old. Fading counts keep each filter's counts to the rows that tell its share from
//! its nearest neighbour's in the order: the rows `n` at which the share's standard error,
//! `sqrt(p(1 - p) / n)`, equals the distance `d` between the two, so `n = p(1 - p) / d²`. Rows
//! beyond those can't change which comes first, only how slowly the order follows a change; past
//! them both counts are scaled to `n`, keeping the share, so each new row weighs `1 / (n + 1)`. Far
//! apart shares fade fast and close ones slowly; equal ones never. Each candidate's failure mode
//! is forced in this file's tests: whole counts that stay wrong after a shift, and fading ones
//! that turn on noise between close shares (`reorders`).
//!
//! The counts are `f64`: whole counts stay exact integers to 2⁵³ rows a run, past which a check
//! no longer moves them (a share then known to one part in 2⁵³), and fading ones take fractions.

/// The filters of one star probe, by their position in its instruction, in their measured order.
#[derive(Debug, Clone)]
pub(crate) struct StarOrder {
    /// Each filter's position in the instruction, the one to check first first.
    order: Vec<usize>,
    /// By position in the instruction: the rows each filter checked, and those it passed.
    checks: Vec<f64>,
    passes: Vec<f64>,
    /// Whether the counts fade (`StarFilterOrder::MeasuredFading`).
    fading: bool,
}

impl StarOrder {
    /// `filters` filters in the planner's order, none yet checked, counted as `rule` says.
    pub(crate) fn new(filters: usize, rule: crate::StarFilterOrder) -> Self {
        Self {
            order: (0..filters).collect(),
            checks: vec![0.0; filters],
            passes: vec![0.0; filters],
            fading: rule == crate::StarFilterOrder::MeasuredFading,
        }
    }

    /// The filter to check `nth`, by its position in the instruction.
    pub(crate) fn nth(&self, nth: usize) -> Option<usize> {
        self.order.get(nth).copied()
    }

    /// Counts a check of the filter at `filter` (its position in the instruction), passed or not.
    /// The order moves only in [Self::settle], once the row's checks are done.
    pub(crate) fn observe(&mut self, filter: usize, passed: bool) {
        if let Some(checks) = self.checks.get_mut(filter) {
            *checks += 1.0;
        }
        if passed {
            if let Some(passes) = self.passes.get_mut(filter) {
                *passes += 1.0;
            }
        }
    }

    /// The filter's share of rows passed, by the rule of succession.
    fn share(&self, filter: usize) -> f64 {
        let passes = self.passes.get(filter).copied().unwrap_or(0.0);
        let checks = self.checks.get(filter).copied().unwrap_or(0.0);
        (passes + 1.0) / (checks + 2.0)
    }

    /// Whether filter `a` has passed a smaller share of its rows than filter `b`, compared by
    /// cross-multiplying, exact for whole counts.
    fn passes_less(&self, a: usize, b: usize) -> bool {
        let count = |counts: &[f64], f: usize| counts.get(f).copied().unwrap_or(0.0);
        let (pa, ca) = (count(&self.passes, a) + 1.0, count(&self.checks, a) + 2.0);
        let (pb, cb) = (count(&self.passes, b) + 1.0, count(&self.checks, b) + 2.0);
        pa * cb < pb * ca
    }

    /// Sorts the filters by their measured shares, the lowest first, keeping the order of filters
    /// whose shares are equal, then fades the counts where they fade. An insertion sort: one row's
    /// checks move the order by little, so it is linear in the filters on all but the rows that
    /// reorder it. Returns whether the order moved.
    pub(crate) fn settle(&mut self) -> bool {
        let mut moved = false;
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
                moved = true;
                j = k;
            }
        }
        if self.fading {
            self.fade();
        }
        moved
    }

    /// Scales each filter's counts down to the rows that tell its share from its neighbours' in
    /// the order (the module's note), where it has counted more.
    fn fade(&mut self) {
        for at in 0..self.order.len() {
            let Some(&filter) = self.order.get(at) else {
                continue;
            };
            let share = self.share(filter);
            let near = [at.checked_sub(1), at.checked_add(1)]
                .into_iter()
                .flatten()
                .filter_map(|other| self.order.get(other).copied())
                .map(|other| (share - self.share(other)).abs())
                .fold(f64::INFINITY, f64::min);
            if near.is_nan() || near <= 0.0 || near.is_infinite() {
                continue;
            }
            let rows = (share * (1.0 - share) / (near * near)).ceil();
            let (Some(checks), Some(passes)) = (
                self.checks.get(filter).copied(),
                self.passes.get(filter).copied(),
            ) else {
                continue;
            };
            if rows.is_nan() || rows < 1.0 || rows >= checks {
                continue;
            }
            if let Some(passes_kept) = self.passes.get_mut(filter) {
                *passes_kept = passes * (rows / checks);
            }
            if let Some(checks_kept) = self.checks.get_mut(filter) {
                *checks_kept = rows;
            }
        }
    }

    /// The filters in the order, for the tests.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.order.len()
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
    use crate::StarFilterOrder;

    /// Runs `rows` rows through `order`, each filter passing a row when `passes(filter, row)`,
    /// checking in the order kept and stopping at the first that rejects: the checks made.
    fn run(order: &mut StarOrder, rows: u64, passes: impl Fn(usize, u64) -> bool) -> u64 {
        run_from(order, 0..rows, passes).0
    }

    /// As [run], over the rows `rows`: the checks made and the rows after which the order moved.
    fn run_from(
        order: &mut StarOrder,
        rows: std::ops::Range<u64>,
        passes: impl Fn(usize, u64) -> bool,
    ) -> (u64, u64) {
        let (mut checks, mut reorders) = (0, 0);
        for row in rows {
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
            if order.settle() {
                reorders += 1;
            }
        }
        (checks, reorders)
    }

    /// A draw in [0, 1) for `filter` at `row`, the same on every run (SplitMix64's finalizer,
    /// Steele, Lea and Flood, "Fast Splittable Pseudorandom Number Generators", OOPSLA 2014).
    fn draw(filter: usize, row: u64) -> f64 {
        let mut x = row
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add((filter as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9));
        x ^= x >> 31;
        x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^= x >> 29;
        x as f64 / 18_446_744_073_709_551_616.0
    }

    /// T-QT.5.11. Do: three filters in the planner's order passing 9, 8 and 1 rows in 10, over
    /// 1,000 rows. Expect: the one passing 1 in 10 moves first, and the rows take fewer checks
    /// than in the planner's order.
    #[test]
    fn t_qt_5_11_the_filter_that_passes_fewest_rows_moves_first() {
        let share = [9, 8, 1];
        let passes = |filter: usize, row: u64| row % 10 < share[filter];
        let mut measured = StarOrder::new(3, StarFilterOrder::Measured);
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
        let mut order = StarOrder::new(2, StarFilterOrder::Measured);
        run(&mut order, 100, |_, row| row % 2 == 0);
        assert_eq!(order.order(), &[0, 1]);
        let mut unchecked = StarOrder::new(3, StarFilterOrder::Measured);
        unchecked.settle();
        assert_eq!(unchecked.order(), &[0, 1, 2]);
        assert_eq!(StarOrder::new(0, StarFilterOrder::Measured).nth(0), None);
    }

    /// T-QT.5.11. Do: a filter that passes every row for 500 rows and then none, beside one passing
    /// half. Expect: the order follows the change: the half filter first while the other passes
    /// all, then the other first once its share falls below a half.
    #[test]
    fn t_qt_5_11_the_order_follows_a_change_in_the_rows() {
        let passes = |filter: usize, row: u64| match filter {
            0 => row < 500,
            _ => row.is_multiple_of(2),
        };
        let mut order = StarOrder::new(2, StarFilterOrder::Measured);
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

    /// T-QT.5.11 (lane qs5-19), whole counts' failure mode: rows that change halfway. Do: a filter
    /// passing every row for 5,000 rows and then none, beside one passing half, 5,000 rows on each
    /// side of the change, with whole and with fading counts. Expect: whole counts still check the
    /// half filter first at the end, as the old rows outnumber the new; fading ones turn, and the
    /// second half takes them fewer checks.
    #[test]
    fn t_qt_5_11_fading_counts_follow_rows_that_shift_halfway() {
        let passes = |filter: usize, row: u64| match filter {
            0 => row < 5_000,
            _ => row.is_multiple_of(2),
        };
        let mut whole = StarOrder::new(2, StarFilterOrder::Measured);
        let mut fading = StarOrder::new(2, StarFilterOrder::MeasuredFading);
        run_from(&mut whole, 0..5_000, passes);
        run_from(&mut fading, 0..5_000, passes);
        assert_eq!(whole.order(), &[1, 0]);
        assert_eq!(fading.order(), &[1, 0]);
        let (whole_checks, _) = run_from(&mut whole, 5_000..10_000, passes);
        let (fading_checks, _) = run_from(&mut fading, 5_000..10_000, passes);
        assert_eq!(
            whole.order(),
            &[1, 0],
            "whole counts stay with the old rows"
        );
        assert_eq!(fading.order(), &[0, 1], "fading counts follow the new rows");
        assert!(
            fading_checks < whole_checks,
            "after the change: fading {fading_checks} checks, whole {whole_checks}"
        );
    }

    /// T-QT.5.11 (lane qs5-19), fading counts' failure mode: noise between close shares. Do: two
    /// filters passing rows at random, a half and three in five of them, over 10,000 rows, with
    /// whole and with fading counts. Expect: both end with the half filter first; fading counts
    /// turn the order many times more often on the noise, and so check more.
    #[test]
    fn t_qt_5_11_fading_counts_turn_on_noise_between_close_shares() {
        let passes = |filter: usize, row: u64| draw(filter, row) < [0.5, 0.6][filter.min(1)];
        let mut whole = StarOrder::new(2, StarFilterOrder::Measured);
        let mut fading = StarOrder::new(2, StarFilterOrder::MeasuredFading);
        let (whole_checks, whole_reorders) = run_from(&mut whole, 0..10_000, passes);
        let (fading_checks, fading_reorders) = run_from(&mut fading, 0..10_000, passes);
        assert_eq!(whole.order(), &[0, 1]);
        assert_eq!(fading.order(), &[0, 1]);
        assert!(
            fading_reorders > 10 * whole_reorders.max(1),
            "fading turned {fading_reorders} times, whole {whole_reorders}"
        );
        assert!(
            fading_checks > whole_checks,
            "fading {fading_checks} checks, whole {whole_checks}"
        );
    }

    /// T-QT.5.11 (lane qs5-19). Do: three filters passing nine, eight and one row in ten at random,
    /// over 10,000 rows, with fading counts. Expect: the order of their shares, kept on far fewer
    /// rows than the run's: shares far apart need few rows to tell them apart.
    #[test]
    fn t_qt_5_11_fading_keeps_the_rows_that_tell_shares_apart() {
        let mut far = StarOrder::new(3, StarFilterOrder::MeasuredFading);
        run(&mut far, 10_000, |filter, row| {
            draw(filter, row) < [0.9, 0.8, 0.1][filter.min(2)]
        });
        assert_eq!(far.order(), &[2, 1, 0]);
        assert!(far.checks.iter().all(|&c| c < 1_000.0), "{:?}", far.checks);
    }
}
