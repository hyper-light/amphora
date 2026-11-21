//! Amphora (QT.5; `docs/design/executor.md`, decision 8): the cost weights of a seek, a hash
//! table's build and probe, and a sort, learned where the engine runs instead of picked. Each is a
//! rate, nanoseconds a unit of its work, kept per database: calibrated once a process from the
//! work itself (one page of keys searched, hashed, probed and sorted), then moved by the times the
//! database's operators take as its queries run. The planner reads each weight as a multiple of
//! the time a row's visit takes, so the four sit on the scale of the costs around them
//! (`CostModelParams::with_learned`). The bench harness `bench/join_methods` checks what they choose;
//! it is not where they come from.

use std::hint::black_box;
use std::sync::OnceLock;
use std::time::Instant;

use parking_lot::Mutex;
use rustc_hash::FxHashMap;

/// The kinds of work the planner weighs, each timed in its own unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Work {
    /// A row visited: read and tested once. The unit the others are weighed in.
    Row,
    /// One search of an ordered index for a key.
    Seek,
    /// One row added to a hash table.
    HashBuild,
    /// One lookup of a hash table.
    HashProbe,
    /// Sorting, in rows times log2 rows, as the planner counts a sort
    /// (`cost::estimate_sort_cpu_cost`).
    Sort,
}

/// Units of work and the nanoseconds they took.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Span {
    units: f64,
    nanos: f64,
}

/// One kind's running rate: the work of the epoch under way and of the one before it. An epoch is
/// as much work as calibration timed, so the rate follows the latest one to two calibrations' worth
/// of work and leaves older times behind as conditions move.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Rate {
    epoch: f64,
    previous: Span,
    current: Span,
}

impl Rate {
    /// The rate calibration measured: `units` of work in `nanos`, also the epoch's length.
    fn calibrated(units: f64, nanos: f64) -> Self {
        Self {
            epoch: units,
            previous: Span { units, nanos },
            current: Span::default(),
        }
    }

    /// Adds an observation. One with no work or a time that is not a finite, non-negative number
    /// says nothing about the rate, and is left out.
    fn observe(&mut self, units: f64, nanos: f64) {
        if !(units.is_finite() && units > 0.0 && nanos.is_finite() && nanos >= 0.0) {
            return;
        }
        self.current.units += units;
        self.current.nanos += nanos;
        if self.current.units >= self.epoch {
            self.previous = self.current;
            self.current = Span::default();
        }
    }

    /// Nanoseconds a unit, None before any time was seen.
    fn nanos_per_unit(&self) -> Option<f64> {
        let units = self.previous.units + self.current.units;
        let nanos = self.previous.nanos + self.current.nanos;
        if units > 0.0 && nanos > 0.0 {
            Some(nanos / units)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Rates {
    row: Rate,
    seek: Rate,
    hash_build: Rate,
    hash_probe: Rate,
    sort: Rate,
}

impl Rates {
    fn rate_mut(&mut self, work: Work) -> &mut Rate {
        match work {
            Work::Row => &mut self.row,
            Work::Seek => &mut self.seek,
            Work::HashBuild => &mut self.hash_build,
            Work::HashProbe => &mut self.hash_probe,
            Work::Sort => &mut self.sort,
        }
    }

    fn weights(&self) -> Option<LearnedWeights> {
        let row = self.row.nanos_per_unit()?;
        Some(LearnedWeights {
            seek: self.seek.nanos_per_unit()? / row,
            hash_build: self.hash_build.nanos_per_unit()? / row,
            hash_probe: self.hash_probe.nanos_per_unit()? / row,
            sort: self.sort.nanos_per_unit()? / row,
        })
    }
}

/// The four weights, each in rows' visits: what one unit of the work takes against one row read
/// and tested.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LearnedWeights {
    pub(crate) seek: f64,
    pub(crate) hash_build: f64,
    pub(crate) hash_probe: f64,
    pub(crate) sort: f64,
}

/// A database's running cost estimator, shared by its connections.
#[derive(Debug)]
pub(crate) struct LearnedCosts {
    rates: Mutex<Rates>,
}

impl LearnedCosts {
    /// An estimator that starts from this process's calibration.
    pub(crate) fn new() -> Self {
        Self::from_rates(*process_calibration())
    }

    fn from_rates(rates: Rates) -> Self {
        Self {
            rates: Mutex::new(rates),
        }
    }

    /// Records that `units` of `work` took `nanos`, as an operator timed it.
    pub(crate) fn observe(&self, work: Work, units: f64, nanos: f64) {
        self.rates.lock().rate_mut(work).observe(units, nanos);
    }

    /// The weights as they stand; None only if the clock never moved while calibrating.
    pub(crate) fn weights(&self) -> Option<LearnedWeights> {
        self.rates.lock().weights()
    }
}

/// The rates measured once a process, on the hardware it runs on.
fn process_calibration() -> &'static Rates {
    static CALIBRATION: OnceLock<Rates> = OnceLock::new();
    CALIBRATION.get_or_init(|| {
        let start = Instant::now();
        calibrate(&mut || start.elapsed().as_nanos() as f64)
    })
}

/// SplitMix64's output function (Steele, Lea and Flood, "Fast splittable pseudorandom number
/// generators", OOPSLA 2014; its published constants): keys in no order, the same every run.
fn mix(i: u64) -> u64 {
    let mut z = i.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Times `work` `reps` times over, doubling `reps` up to `most_reps` until the time reaches
/// `target`: (repetitions, nanoseconds).
fn measure(
    clock: &mut dyn FnMut() -> f64,
    target: f64,
    most_reps: usize,
    work: &mut dyn FnMut(),
) -> (f64, f64) {
    let mut reps: usize = 1;
    loop {
        let start = clock();
        for _ in 0..reps {
            work();
        }
        let nanos = clock() - start;
        match reps.checked_mul(2) {
            Some(next) if nanos < target && next <= most_reps => reps = next,
            _ => return (reps as f64, nanos),
        }
    }
}

/// Times each kind of work over one page of 8-byte keys, the unit a b-tree reads
/// (`BufferPool::DEFAULT_PAGE_SIZE`). Each kind is repeated, up to once per key, until its time
/// reaches the slowest kind's single run, so the clock's step weighs on each about equally.
fn calibrate(clock: &mut dyn FnMut() -> f64) -> Rates {
    let n = crate::storage::buffer_pool::BufferPool::DEFAULT_PAGE_SIZE / std::mem::size_of::<u64>();
    let keys: Vec<u64> = (0..n as u64).map(mix).collect();
    let mut ordered = keys.clone();
    ordered.sort_unstable();
    let table: FxHashMap<u64, u64> = keys.iter().map(|k| (*k, *k)).collect();
    let rows = n as f64;

    let mut row = || {
        black_box(keys.iter().filter(|k| black_box(**k) & 1 == 0).count());
    };
    let mut seek = || {
        for k in &keys {
            let _ = black_box(ordered.binary_search(k));
        }
    };
    let mut build = || {
        let mut built = FxHashMap::default();
        for k in &keys {
            built.insert(*k, *k);
        }
        black_box(&built);
    };
    let mut probe = || {
        for k in &keys {
            black_box(table.get(k));
        }
    };
    let mut sort = || {
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        black_box(&sorted);
    };
    let mut kinds: [(Work, f64, &mut dyn FnMut()); 5] = [
        (Work::Row, rows, &mut row),
        (Work::Seek, rows, &mut seek),
        (Work::HashBuild, rows, &mut build),
        (Work::HashProbe, rows, &mut probe),
        (Work::Sort, rows * rows.log2(), &mut sort),
    ];
    let mut target: f64 = 0.0;
    for (_, _, work) in kinds.iter_mut() {
        target = target.max(measure(clock, 0.0, 1, &mut **work).1);
    }
    let mut rates = Rates::default();
    for (kind, units, work) in kinds.iter_mut() {
        let (reps, nanos) = measure(clock, target, n, &mut **work);
        *rates.rate_mut(*kind) = Rate::calibrated(reps * *units, nanos);
    }
    rates
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_rates(row: f64, seek: f64, build: f64, probe: f64, sort: f64) -> Rates {
        Rates {
            row: Rate::calibrated(100.0, 100.0 * row),
            seek: Rate::calibrated(100.0, 100.0 * seek),
            hash_build: Rate::calibrated(100.0, 100.0 * build),
            hash_probe: Rate::calibrated(100.0, 100.0 * probe),
            sort: Rate::calibrated(100.0, 100.0 * sort),
        }
    }

    /// Do: calibrate on this machine with no operator observed. Expect: every weight a finite,
    /// positive number of rows' visits (T-QT.5.5: the learned costs start sane).
    #[test]
    fn t_qt_5_5_calibration_starts_sane_with_no_observations() {
        let weights = LearnedCosts::new().weights().expect("the clock moved");
        for w in [
            weights.seek,
            weights.hash_build,
            weights.hash_probe,
            weights.sort,
        ] {
            assert!(w.is_finite() && w > 0.0, "{weights:?}");
        }
    }

    /// Do: start from known rates, then observe sorts at four times the calibrated rate for as
    /// much work as two epochs. Expect: the sort weight moves to four times its start, the others
    /// stay (T-QT.5.5: the estimator follows observed times).
    #[test]
    fn t_qt_5_5_observed_times_move_the_weight() {
        let costs = LearnedCosts::from_rates(fixed_rates(1.0, 10.0, 20.0, 15.0, 2.0));
        let start = costs.weights().unwrap();
        assert_eq!(start.sort, 2.0);
        costs.observe(Work::Sort, 50.0, 50.0 * 8.0);
        let partway = costs.weights().unwrap();
        assert!(partway.sort > 2.0 && partway.sort < 8.0, "{partway:?}");
        for _ in 0..4 {
            costs.observe(Work::Sort, 50.0, 50.0 * 8.0);
        }
        let moved = costs.weights().unwrap();
        assert_eq!(moved.sort, 8.0);
        assert_eq!(moved.seek, start.seek);
        assert_eq!(moved.hash_build, start.hash_build);
        assert_eq!(moved.hash_probe, start.hash_probe);
    }

    /// Do: observe nothing-sized and non-finite times. Expect: the weights unchanged (T-QT.5.5).
    #[test]
    fn t_qt_5_5_meaningless_observations_leave_the_rates() {
        let costs = LearnedCosts::from_rates(fixed_rates(1.0, 10.0, 20.0, 15.0, 2.0));
        let start = costs.weights().unwrap();
        costs.observe(Work::Seek, 0.0, 5.0);
        costs.observe(Work::Seek, 10.0, f64::NAN);
        costs.observe(Work::Seek, 10.0, -1.0);
        assert_eq!(costs.weights().unwrap(), start);
    }

    /// Do: a slower row visit. Expect: every weight, counted in rows' visits, falls in proportion
    /// (T-QT.5.5: the weights are relative to the row).
    #[test]
    fn t_qt_5_5_weights_are_in_rows_visits() {
        let fast = LearnedCosts::from_rates(fixed_rates(1.0, 10.0, 20.0, 15.0, 2.0));
        let slow = LearnedCosts::from_rates(fixed_rates(2.0, 10.0, 20.0, 15.0, 2.0));
        let (fast, slow) = (fast.weights().unwrap(), slow.weights().unwrap());
        assert_eq!(slow.seek * 2.0, fast.seek);
        assert_eq!(slow.sort * 2.0, fast.sort);
    }

    /// Do: calibrate with a clock that never moves. Expect: no weights, so the planner is told it
    /// has none rather than given zeros (T-QT.5.5).
    #[test]
    fn t_qt_5_5_a_stopped_clock_gives_no_weights() {
        let rates = calibrate(&mut || 0.0);
        assert_eq!(rates.weights(), None);
    }
}
