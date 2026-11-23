//! Amphora (QT.5; `docs/design/executor.md`, decision 8): the cost weights of a seek, a hash
//! table's build and probe, and a sort, learned where the engine runs instead of picked. Each is a
//! rate, nanoseconds a unit of its work, kept per database: calibrated once a process from the
//! work itself (one page of keys searched, hashed, probed and sorted), then moved by the times the
//! database's operators take as its queries run. The planner reads each weight as a multiple of
//! the time a row's visit takes, so the four sit on the scale of the costs around them
//! (`CostModelParams::with_learned`). The bench harness `bench/join_methods` checks what they choose;
//! it is not where they come from.

use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// One lookup of a hash join's Bloom filter for a key it doesn't hold, the key's hash
    /// included (PL.5: what the filter costs every probe row, measured rather than priced as one
    /// key hash).
    BloomProbe,
    /// One step of a `WHERE` condition: two of the engine's values compared, as a condition's
    /// comparison compares them (`cpu_cost_per_where_step`).
    WhereStep,
    /// One page read from the database's file, as the pager times it from the read's submission
    /// to its completion (`sqlite3_ondisk::begin_read_page`). The planner's unit of cost.
    PageRead,
}

/// One batch's observations (an epoch's): their work and time summed, and how many.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Span {
    units: f64,
    nanos: f64,
    count: f64,
}

impl Span {
    fn add(&mut self, units: f64, nanos: f64) {
        self.units += units;
        self.nanos += nanos;
        self.count += 1.0;
    }

    /// Nanoseconds a unit over the span, None without work and time.
    fn per_unit(&self) -> Option<f64> {
        (self.units > 0.0 && self.nanos > 0.0).then(|| self.nanos / self.units)
    }
}

/// PL.8 (T-PL.8.6): how far a batch's rate must sit from the standing one before it replaces it:
/// a two-sided 95% test, Bonferroni-corrected for the eight kinds of work `Rates` tests as their
/// batches close (Dunn, "Multiple Comparisons Among Means", JASA 56(293), 1961), as `cost_fit`'s
/// `AGREE_Z` is for its weights. `MOVE_LEVEL` is the test's central probability, 1 - 0.05/8;
/// `MOVE_Z` its standard normal quantile, the least Student's t quantile can be (`student_t`).
const MOVE_LEVEL: f64 = 1.0 - 0.05 / 8.0;
const MOVE_Z: f64 = 2.734_368_786_533_176;

/// PL.8 (T-PL.8.6, lane pl8-14): the spread between a rate's batch means, the error its weight is
/// judged by. A burst of load slows a whole batch at once, so the times inside one batch agree
/// with each other however far the batch sits from the last: their own spread (the ratio
/// estimator's, before this) took each burst for evidence, and on a loaded host the hash probe's
/// weight climbed in significant steps through one run (10, 170, 220, 280 rows' visits) and
/// flipped `t_pl_8_2`'s plans. The spread between batches holds those bursts, as the method of
/// batch means reads a correlated series' error (Schmeiser, "Batch Size Effects in the Analysis of
/// Simulation Output", Operations Research 30(3), 1982).
///
/// It is read from successive differences of the batch means' logarithms (von Neumann, Kent,
/// Bellinson and Hart, "The Mean Square Successive Difference", Annals of Mathematical Statistics
/// 12(2), 1941): a load that slows the work multiplies its times, so a logarithm's spread is the
/// same at any level, and a successive difference stays small across a slow drift. The difference
/// into a batch that moved the rate is a change in the work, not its noise, and is left out.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Batches {
    /// The last batch mean's logarithm; None before the first batch closed.
    last: Option<f64>,
    /// How many successive differences `squares` sums the squares of.
    differences: f64,
    squares: f64,
}

impl Batches {
    /// Whether a batch whose mean's logarithm is `seen` replaces `standing`, the rate standing
    /// (nanoseconds a unit); and the batch counted. The first batch observed replaces the rate
    /// calibration measured or the open anchored: those are timed on other work than the
    /// database's operators, so there is no spread to weigh the two against. A later one replaces
    /// it only past the spread between batches (`refutes`).
    fn judge(&mut self, standing: Option<f64>, seen: f64) -> bool {
        let moves = match (standing, self.last) {
            (Some(standing), Some(_)) => self.refutes(standing.ln(), seen),
            _ => true,
        };
        if let (Some(last), false) = (self.last, moves) {
            self.differences += 1.0;
            self.squares += (seen - last) * (seen - last);
        }
        self.last = Some(seen);
        moves
    }

    /// Whether `seen` sits from `standing` (logarithms) past the spread between batches. The
    /// difference of two batch means has twice a batch's variance, and the mean square successive
    /// difference estimates twice it as well, so the gap is weighed against the root of the mean
    /// square successive difference itself: their ratio is Student's t with as many degrees of
    /// freedom as there are differences, tested at [MOVE_LEVEL]. None yet: no batch moves the rate.
    ///
    /// The test costs a bounded number of steps however long the process runs (lane pl8-15): the
    /// ratio's own probability (`t_central`, one term each two degrees of freedom) while there are
    /// few differences, and past that the quantile's expansion in `1 / nu` (`move_t_expanded`),
    /// once that expansion is exact to an f64. A ratio under the normal quantile never needs
    /// either: Student's t is never under it.
    fn refutes(&self, standing: f64, seen: f64) -> bool {
        let nu = self.differences;
        if nu < 1.0 {
            return false;
        }
        let ratio = (seen - standing).abs() / (self.squares / nu).sqrt();
        if ratio.is_nan() || ratio <= MOVE_Z {
            return false;
        }
        match move_t_expanded(nu) {
            Some(t) => ratio > t,
            None => t_central(ratio.atan2(nu.sqrt()), nu) > MOVE_LEVEL,
        }
    }
}

/// Student's t quantile at [MOVE_LEVEL] for `nu` degrees of freedom by its Cornish-Fisher
/// expansion about the normal quantile [MOVE_Z] (Abramowitz and Stegun, "Handbook of Mathematical
/// Functions", 1964, 26.7.5, its four terms); None while the last term is past an f64's resolution
/// of the sum, where the expansion is not yet the quantile and the closed form decides
/// (`Batches::refutes`). With [MOVE_Z] that is past about 13,000 degrees of freedom.
fn move_t_expanded(nu: f64) -> Option<f64> {
    let x = MOVE_Z;
    let x2 = x * x;
    let terms = [
        x * (x2 + 1.0) / 4.0,
        x * ((5.0 * x2 + 16.0) * x2 + 3.0) / 96.0,
        x * (((3.0 * x2 + 19.0) * x2 + 17.0) * x2 - 15.0) / 384.0,
        x * ((((79.0 * x2 + 776.0) * x2 + 1482.0) * x2 - 1920.0) * x2 - 945.0) / 92160.0,
    ];
    let (mut t, mut power, mut last) = (x, 1.0, 0.0);
    for term in terms {
        power *= nu;
        last = term / power;
        t += last;
    }
    if last.abs() <= f64::EPSILON * t {
        Some(t)
    } else {
        None
    }
}

/// Student's t quantile for `nu` degrees of freedom (a whole number, at least one) at central
/// probability `level`: the `t` with `P(|T| <= t) = level`. Found by bisection on
/// `theta = atan(t / sqrt(nu))` over `P(|T| <= t)`'s closed form for whole degrees of freedom
/// (`t_central`), until the halves meet at an f64's resolution. None outside its domain. The
/// tests' reference for `t_central` and `move_t_expanded`.
#[cfg(test)]
fn student_t(level: f64, nu: f64) -> Option<f64> {
    if !(level > 0.0 && level < 1.0 && nu.is_finite() && nu >= 1.0) {
        return None;
    }
    let nu = nu.floor();
    let (mut low, mut high) = (0.0_f64, std::f64::consts::FRAC_PI_2);
    loop {
        let mid = 0.5 * (low + high);
        if mid <= low || mid >= high {
            break;
        }
        if t_central(mid, nu) < level {
            low = mid;
        } else {
            high = mid;
        }
    }
    let t = nu.sqrt() * high.tan();
    if t.is_finite() {
        Some(t)
    } else {
        None
    }
}

/// `P(|T| <= sqrt(nu) tan(theta))` for Student's t with `nu` degrees of freedom, a whole number
/// (Abramowitz and Stegun, "Handbook of Mathematical Functions", 1964, 26.7.3 for odd `nu` and
/// 26.7.4 for even): a sum of powers of `cos(theta)` to `cos(theta)^(nu - 2)`, each term the last
/// times `cos(theta)^2 (p + 1) / (p + 2)` from power `p`.
fn t_central(theta: f64, nu: f64) -> f64 {
    let (sin, cos) = theta.sin_cos();
    let odd = nu % 2.0 == 1.0;
    let (mut term, mut power) = if odd { (cos, 1.0) } else { (1.0, 0.0) };
    let mut sum = 0.0;
    while power <= nu - 2.0 {
        sum += term;
        term *= cos * cos * (power + 1.0) / (power + 2.0);
        power += 2.0;
    }
    if odd {
        std::f64::consts::FRAC_2_PI * (theta + sin * sum)
    } else {
        sin * sum
    }
}

/// Amphora (PL.8, T-PL.8.6, lane pl8-16): the rule that moves a learned weight, behind this switch
/// so the two can be measured against each other (the coordinator's A/B; `Connection::set_weight_rule`).
/// Each forces its own failure: `Blend` follows every read, so noise and bursts of load flip
/// close plans; `BatchMeans` holds through them, but it can't judge a batch until it has seen the
/// spread between batches, so a change in the work's cost soon after start is learned slowly. The
/// harness `ab_tests` measures both (plan flips a batch, batches to follow a change).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WeightRule {
    /// The control, the rule before lane pl8-13: every read blends into the rate at once, the
    /// running mean of the batch under way and the one before it.
    Blend,
    /// The candidate (lanes pl8-14 and pl8-15): a closed batch replaces the standing rate only
    /// past the spread between batch means (`Batches::judge`).
    #[default]
    BatchMeans,
}

/// One kind's rate: the one standing, the batch under way, and the spread between batches. A
/// batch (an epoch) is as much work as calibration timed. Under [WeightRule::BatchMeans], when one
/// closes, its rate replaces the standing one only if the two differ significantly by the spread
/// between batches (`Batches::judge`); otherwise the standing rate stays exactly where it was. So
/// the weights the planner reads move on evidence alone, never by a batch's noise or a burst of
/// load, and a plan chosen on them is chosen again until the work really costs something else
/// (PL.8, T-PL.8.6). Under [WeightRule::Blend], the rate is the running mean over `previous`, the
/// last closed batch, and the batch under way.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Rate {
    rule: WeightRule,
    epoch: f64,
    standing: Option<f64>,
    current: Span,
    batches: Batches,
    /// [WeightRule::Blend]'s last closed batch (calibration's work, or the open's anchor, first).
    previous: Span,
}

impl Rate {
    /// A rate measured as `units` of work in `nanos`, also the batch's length (calibration's; a
    /// test's fixed rate).
    fn measured(units: f64, nanos: f64) -> Self {
        let mut rate = Self {
            epoch: units,
            ..Self::default()
        };
        rate.anchor(units, nanos);
        rate
    }

    /// Stands the rate at `units` of work in `nanos`, as calibration or the open's anchor timed it.
    fn anchor(&mut self, units: f64, nanos: f64) {
        if units > 0.0 && nanos > 0.0 {
            self.standing = Some(nanos / units);
            self.previous = Span {
                units,
                nanos,
                count: 1.0,
            };
        }
    }

    /// Moves to `rule` from the rate as it stands, a batch's worth of work at it: the batch under
    /// way and the spread between batches start again, so neither rule reads what the other kept.
    fn switch(&mut self, rule: WeightRule) {
        let now = self.nanos_per_unit();
        *self = Self {
            rule,
            epoch: self.epoch,
            ..Self::default()
        };
        if let Some(now) = now {
            self.anchor(self.epoch.max(1.0), now * self.epoch.max(1.0));
        }
    }

    /// Adds an observation. One with no work or a time that is not a finite, non-negative number
    /// says nothing about the rate, and is left out. A batch with one observation alone runs on
    /// until a second: one time alone is one moment's load.
    fn observe(&mut self, units: f64, nanos: f64) {
        if !(units.is_finite() && units > 0.0 && nanos.is_finite() && nanos >= 0.0) {
            return;
        }
        self.current.add(units, nanos);
        if self.rule == WeightRule::Blend {
            if self.current.units >= self.epoch {
                self.previous = std::mem::take(&mut self.current);
            }
            return;
        }
        if self.current.units < self.epoch || self.current.count < 2.0 {
            return;
        }
        let Some(mean) = self.current.per_unit() else {
            return;
        };
        self.current = Span::default();
        if self.batches.judge(self.standing, mean.ln()) {
            self.standing = Some(mean);
        }
    }

    /// PL.8 (T-PL.8.2, lane pl8-22): closes a batch of single timed runs (`Sampling`) at `median`,
    /// their median run's nanoseconds, judged as a batch mean is (`Batches::judge`). A timed run
    /// the scheduler preempted measures the preemption, milliseconds against the run's tens of
    /// nanoseconds, and one of those in a batch moved its mean a hundredfold: the first batch
    /// replaces calibration's rate unjudged, so under load a hash probe's weight jumped and the
    /// plans flipped (20 copies at once of `t_pl_8_2`, kiln run r124). The median holds while
    /// fewer than half the runs are preempted, as calibration's samples are read
    /// (`calibrate`, `t_qt_5_5_calibration_weights_hold_through_preempted_samples`).
    fn close_runs(&mut self, median: f64) {
        if !(median.is_finite() && median > 0.0) {
            return;
        }
        self.current = Span::default();
        if self.batches.judge(self.standing, median.ln()) {
            self.standing = Some(median);
        }
    }

    /// The spread between this rate's batch means, as a logarithm's (`Batches`): None under
    /// [WeightRule::Blend], which keeps none, or before a difference was counted.
    fn spread(&self) -> Option<f64> {
        let batches = &self.batches;
        let spread = (batches.squares / batches.differences).sqrt();
        if self.rule == WeightRule::BatchMeans && batches.differences >= 1.0 && spread.is_finite() {
            Some(spread)
        } else {
            None
        }
    }

    /// Nanoseconds a unit, None before any time was seen: the standing rate, or before there is
    /// one, the batch's so far (there is no standing rate for it to move).
    fn nanos_per_unit(&self) -> Option<f64> {
        match self.rule {
            WeightRule::Blend => Span {
                units: self.previous.units + self.current.units,
                nanos: self.previous.nanos + self.current.nanos,
                count: self.previous.count + self.current.count,
            }
            .per_unit(),
            WeightRule::BatchMeans => self.standing.or_else(|| self.current.per_unit()),
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
    bloom_probe: Rate,
    where_step: Rate,
    page: Rate,
    /// What timing one run costs: a pair of clock reads (step 3a, `Sampling`).
    timing_nanos: f64,
}

impl Rates {
    /// PL.8 (lane pl8-16): the share of a plan's cost its weights' error could explain
    /// (`LearnedCosts::cost_margin`). A plan's cost is a sum of work counts times weights in page
    /// reads, each weight a kind's rate over the page read's, so a term's error (as a logarithm's)
    /// is `sqrt(s_kind^2 + s_page^2)` from the spreads between batch means, and a sum of positive
    /// terms is off by no more than its worst term. Two plans priced on the same weights share
    /// their errors, so this bounds the gap between them from above. Tested at [MOVE_Z], the
    /// level the weights themselves move at; 0 where no spread was measured.
    fn cost_margin(&self) -> f64 {
        let page = self.page.spread().unwrap_or(0.0);
        let kind = [
            &self.row,
            &self.seek,
            &self.hash_build,
            &self.hash_probe,
            &self.sort,
            &self.bloom_probe,
            &self.where_step,
        ]
        .into_iter()
        .filter_map(Rate::spread)
        .fold(0.0_f64, f64::max);
        let margin = (MOVE_Z * (kind * kind + page * page).sqrt()).exp_m1();
        if margin.is_finite() && margin > 0.0 {
            margin
        } else {
            0.0
        }
    }

    /// Every kind's rate under `rule` (`Rate::switch`).
    fn switch(&mut self, rule: WeightRule) {
        for rate in [
            &mut self.row,
            &mut self.seek,
            &mut self.hash_build,
            &mut self.hash_probe,
            &mut self.sort,
            &mut self.bloom_probe,
            &mut self.where_step,
            &mut self.page,
        ] {
            rate.switch(rule);
        }
    }

    fn rate_mut(&mut self, work: Work) -> &mut Rate {
        match work {
            Work::Row => &mut self.row,
            Work::Seek => &mut self.seek,
            Work::HashBuild => &mut self.hash_build,
            Work::HashProbe => &mut self.hash_probe,
            Work::Sort => &mut self.sort,
            Work::BloomProbe => &mut self.bloom_probe,
            Work::WhereStep => &mut self.where_step,
            Work::PageRead => &mut self.page,
        }
    }

    /// The weights, with `miss`, the share of the connection's recent page fetches that read a
    /// file (`Pager::page_miss_ratio`), passed through for the planner.
    fn weights(&self, miss: Option<f64>) -> Option<LearnedWeights> {
        let row = self.row.nanos_per_unit()?;
        Some(LearnedWeights {
            row_in_pages: self.page.nanos_per_unit().map(|page| row / page),
            seek: self.seek.nanos_per_unit()? / row,
            hash_build: self.hash_build.nanos_per_unit()? / row,
            hash_probe: self.hash_probe.nanos_per_unit()? / row,
            sort: self.sort.nanos_per_unit()? / row,
            bloom_probe: self.bloom_probe.nanos_per_unit()? / row,
            where_step: self.where_step.nanos_per_unit()? / row,
            page_miss: miss.filter(|m| m.is_finite()).map(|m| m.clamp(0.0, 1.0)),
        })
    }
}

/// The four weights, each in rows' visits: what one unit of the work takes against one row read
/// and tested; and the row's visit itself in page reads, the planner's unit, once the database has
/// read a page from its file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LearnedWeights {
    /// A row's visit against a page read; None until the database has timed one (a database
    /// whose pages all came from memory so far has no page read to weigh it against).
    pub(crate) row_in_pages: Option<f64>,
    pub(crate) seek: f64,
    pub(crate) hash_build: f64,
    pub(crate) hash_probe: f64,
    pub(crate) sort: f64,
    /// A Bloom filter's lookup of a key it doesn't hold (`Work::BloomProbe`).
    pub(crate) bloom_probe: f64,
    /// A `WHERE` condition's step (`Work::WhereStep`).
    pub(crate) where_step: f64,
    /// The share of the connection's recent page fetches that read a file, None before its first
    /// fetch (`Pager::page_miss_ratio`): how many of the pages a plan would first read from a cold
    /// cache it reads from the file (`cost::file_reads`).
    pub(crate) page_miss: Option<f64>,
}

/// Amphora (AUDIT 3.3): one connection's count of its runs of each sampled operation
/// ([`LearnedCosts::sampled`]).
#[derive(Debug, Default)]
pub(crate) struct RunCounts {
    seek: RunCount,
    hash_build: RunCount,
    hash_probe: RunCount,
}

impl RunCounts {
    const fn count(&self, work: Work) -> Option<&RunCount> {
        match work {
            Work::Seek => Some(&self.seek),
            Work::HashBuild => Some(&self.hash_build),
            Work::HashProbe => Some(&self.hash_probe),
            Work::Row | Work::Sort | Work::BloomProbe | Work::WhereStep | Work::PageRead => None,
        }
    }
}

/// A count one connection keeps: a load and a store, not a read-modify-write, since one thread
/// runs a connection's statement at a time; two at once can only lose a count, which moves which
/// runs are timed, not how many in `every`.
#[derive(Debug, Default)]
struct RunCount(AtomicU64);

impl RunCount {
    /// This run's number, counting it.
    fn next(&self) -> u64 {
        let run = self.0.load(Ordering::Relaxed);
        self.0.store(run.wrapping_add(1), Ordering::Relaxed);
        run
    }
}

/// A database's running cost estimator, shared by its connections.
#[derive(Debug)]
pub(crate) struct LearnedCosts {
    rates: Mutex<Rates>,
    /// Which runs of the seek, the hash build and the hash probe are timed (step 3a).
    seek: Sampling,
    hash_build: Sampling,
    hash_probe: Sampling,
    /// What timing one run costs: two clock reads, as calibration timed them.
    timing_nanos: f64,
    /// PL.8: the join search's measured time a candidate, and what it chose.
    search: super::search_budget::SearchRate,
}

/// Amphora (QT.5, step 3a): which runs of one operation are timed, one in `every`, and the spread
/// of the times sampled since `every` was last set.
///
/// Timing a run costs `c` (`LearnedCosts::timing_nanos`), so timing one in `k` adds a share
/// `c / (k * mean)` to the operation's work; an epoch of the rate (`n` timed units) is then a
/// mean over `n` samples whose relative standard error is `cv / sqrt(n)`, `cv` the times' spread
/// (standard deviation over mean), and the epoch spans `n * k` runs. `every` is the `k` at which
/// the two are equal over an epoch's runs `n`: `c / (k * mean) = cv * sqrt(k / n)`, that is
/// `k = (c * sqrt(n) / sd)^(2/3)`; at least 1, at most `n`. A steadier operation is timed less
/// often, a costlier timer too; one whose times scatter is timed more. Until a spread is measured,
/// every run is timed.
#[derive(Debug)]
struct Sampling {
    every: AtomicU64,
    spread: Mutex<Spread>,
    /// PL.8 (T-PL.8.2, lane pl8-22): the timed runs of the batch under way, under
    /// [WeightRule::BatchMeans]: at most an epoch's, the work calibration timed a sample of
    /// (`Rate::close_runs`).
    batch: Mutex<Vec<f64>>,
}

/// Welford's running mean and sum of squared deviations (Welford, "Note on a method for
/// calculating corrected sums of squares and products", Technometrics 4(3), 1962).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Spread {
    count: f64,
    mean: f64,
    m2: f64,
}

impl Spread {
    fn add(&mut self, x: f64) {
        self.count += 1.0;
        let delta = x - self.mean;
        self.mean += delta / self.count;
        self.m2 += delta * (x - self.mean);
    }

    fn sd(&self) -> Option<f64> {
        (self.count > 1.0).then(|| (self.m2 / (self.count - 1.0)).max(0.0).sqrt())
    }
}

impl Sampling {
    fn new() -> Self {
        Self {
            every: AtomicU64::new(1),
            spread: Mutex::new(Spread::default()),
            batch: Mutex::new(Vec::new()),
        }
    }

    /// Whether the run counted `run` (from 0) is timed: one in `every`.
    fn sampled(&self, run: u64) -> bool {
        run.is_multiple_of(self.every.load(Ordering::Relaxed).max(1))
    }

    /// Adds a timed run's `nanos`; once an epoch's `epoch` samples are in, sets `every` from their
    /// spread and `timing_nanos` and starts the next.
    fn add(&self, nanos: f64, epoch: f64, timing_nanos: f64) {
        let mut spread = self.spread.lock();
        spread.add(nanos);
        if spread.count < epoch.max(2.0) {
            return;
        }
        if let Some(every) = sample_every(timing_nanos, epoch, spread.sd()) {
            self.every.store(every, Ordering::Relaxed);
        }
        *spread = Spread::default();
    }
}

/// `k = (c * sqrt(n) / sd)^(2/3)` between 1 and `n` (`Sampling`); None without a spread to go by.
fn sample_every(timing_nanos: f64, epoch: f64, sd: Option<f64>) -> Option<u64> {
    let sd = sd.filter(|sd| sd.is_finite() && *sd > 0.0)?;
    if !(timing_nanos.is_finite() && timing_nanos > 0.0 && epoch.is_finite() && epoch >= 1.0) {
        return None;
    }
    let k = (timing_nanos * epoch.sqrt() / sd).powf(2.0 / 3.0);
    // The nearest whole number of runs.
    Some(k.round().clamp(1.0, epoch) as u64)
}

impl LearnedCosts {
    /// An estimator that starts from this process's calibration.
    pub(crate) fn new() -> Self {
        Self::from_rates(process_calibration())
    }

    fn from_rates(rates: &Rates) -> Self {
        Self {
            rates: Mutex::new(*rates),
            seek: Sampling::new(),
            hash_build: Sampling::new(),
            hash_probe: Sampling::new(),
            timing_nanos: rates.timing_nanos,
            search: super::search_budget::SearchRate::default(),
        }
    }

    /// PL.8: the join search's measured rate and counts.
    pub(crate) fn search(&self) -> &super::search_budget::SearchRate {
        &self.search
    }

    fn sampling(&self, work: Work) -> Option<&Sampling> {
        match work {
            Work::Seek => Some(&self.seek),
            Work::HashBuild => Some(&self.hash_build),
            Work::HashProbe => Some(&self.hash_probe),
            Work::Row | Work::Sort | Work::BloomProbe | Work::WhereStep | Work::PageRead => None,
        }
    }

    /// Whether to time this run of `work` (step 3a: the seek, the hash build and the hash probe,
    /// one run in their `Sampling`'s `every`), counted in `runs`, the asking connection's own.
    ///
    /// Amphora (AUDIT 3.3): the runs were counted in one database-wide atomic, a `fetch_add` on
    /// a cache line every connection's every seek, hash build and hash probe wrote. Each
    /// connection counts its own now: one in `every` of its runs is timed, as one in `every` of
    /// all of them was, so the sample (and the rate it moves) is the same systematic sample, and
    /// the shared `every` is only read.
    pub(crate) fn sampled(&self, work: Work, runs: &RunCounts) -> bool {
        match (self.sampling(work), runs.count(work)) {
            (Some(sampling), Some(count)) => sampling.sampled(count.next()),
            _ => false,
        }
    }

    /// Records a timed run of `work`, one unit in `nanos`: it moves the rate, and its spread sets
    /// how often the runs are timed. Under [WeightRule::BatchMeans] a batch of runs closes at its
    /// median run (`Rate::close_runs`); under [WeightRule::Blend], the control, each run blends in.
    pub(crate) fn observe_sampled(&self, work: Work, nanos: f64) {
        if !(nanos.is_finite() && nanos >= 0.0) {
            return;
        }
        let sampling = self.sampling(work);
        let epoch = {
            let mut rates = self.rates.lock();
            let rate = rates.rate_mut(work);
            match (rate.rule, sampling) {
                (WeightRule::BatchMeans, Some(sampling)) => {
                    let mut batch = sampling.batch.lock();
                    batch.push(nanos);
                    if batch.len() as f64 >= rate.epoch.max(2.0) {
                        if let Some(median) = median(&mut batch) {
                            rate.close_runs(median);
                        }
                        batch.clear();
                    }
                }
                _ => rate.observe(1.0, nanos),
            }
            rate.epoch
        };
        if let Some(sampling) = sampling {
            sampling.add(nanos, epoch, self.timing_nanos);
        }
    }

    /// Records that `units` of `work` took `nanos`, as an operator timed it.
    pub(crate) fn observe(&self, work: Work, units: f64, nanos: f64) {
        self.rates.lock().rate_mut(work).observe(units, nanos);
    }

    /// Nanoseconds a unit of `work` takes as the rate stands, None before any time was seen: what
    /// G11 step 5's access paths weigh the column copy's measured scan against
    /// (`column_path::RowRates::learned`).
    pub(crate) fn nanos_per(&self, work: Work) -> Option<f64> {
        self.rates.lock().rate_mut(work).nanos_per_unit()
    }

    /// The weights as they stand, no fetch's miss share given; None only if the clock never moved
    /// while calibrating.
    #[cfg(test)]
    pub(crate) fn weights(&self) -> Option<LearnedWeights> {
        self.rates.lock().weights(None)
    }

    /// The weights as they stand for a connection whose recent page fetches read the file `miss`
    /// of the time (`Pager::page_miss_ratio`; None before its first fetch).
    pub(crate) fn weights_at(&self, miss: Option<f64>) -> Option<LearnedWeights> {
        self.rates.lock().weights(miss)
    }

    /// Anchors the page read's rate to `reads` page reads timed at open in `nanos`
    /// (`time_page_read`), before the database has read a page of its own. They count beside the
    /// database's own reads, and the first epoch of those replaces them.
    pub(crate) fn anchor_page_read(&self, reads: f64, nanos: f64) {
        let mut rates = self.rates.lock();
        if !(reads.is_finite() && reads > 0.0 && nanos.is_finite() && nanos > 0.0) {
            return;
        }
        rates.page.anchor(reads, nanos);
    }

    /// PL.8 (lane pl8-16): how much more than the search's best a statement's carried join order
    /// may cost and keep its place (`join::keep_carried`), as a share of the best's cost: what the
    /// weights' measured error could explain (`Rates::cost_margin`). 0 under [WeightRule::Blend].
    pub(crate) fn cost_margin(&self) -> f64 {
        self.rates.lock().cost_margin()
    }

    /// PL.8 (T-PL.8.6, lane pl8-16): moves every weight to `rule` (`WeightRule`), from the rates as
    /// they stand.
    pub(crate) fn set_rule(&self, rule: WeightRule) {
        let mut rates = self.rates.lock();
        rates.switch(rule);
        for sampling in [&self.seek, &self.hash_build, &self.hash_probe] {
            sampling.batch.lock().clear();
        }
    }

    /// The page reads an epoch of the page read's rate holds: the most the open's anchor times
    /// (`time_page_read`), as calibration times each kind of work up to a page of it.
    pub(crate) fn page_read_epoch(&self) -> usize {
        self.rates.lock().page.epoch.max(1.0) as usize
    }
}

/// The rates measured on the hardware the process runs on: once a machine, not once a process
/// (AUDIT §7.1, perf/coldstart). A calibration this machine's CPU already measured for this very
/// executable (`stored::Key`) is read back, exactly as it was measured; otherwise calibration runs
/// before the first plan, as it always did, and is kept for the next process. A machine whose CPU
/// or executable changed has another key, so it is measured again then (detected, never on a
/// timer); one whose CPU can't be told (`stored::Key::here` None) calibrates each process.
fn process_calibration() -> &'static Rates {
    static CALIBRATION: OnceLock<Rates> = OnceLock::new();
    CALIBRATION.get_or_init(|| {
        let key = stored::Key::here();
        if let Some(kept) = key.as_ref().and_then(stored::load) {
            return kept.rates();
        }
        let start = Instant::now();
        let measured = measure_calibration(&mut || start.elapsed().as_nanos() as f64);
        if let Some(key) = &key {
            stored::save(key, &measured);
        }
        measured.rates()
    })
}

/// What calibration measured (AUDIT §7.1): the timer's cost, a pair of clock reads, and each kind
/// of [CALIBRATED] work's (units, nanoseconds), the median sample's. The rates are made from it
/// (`Calibration::rates`), so one read back from a machine's store plans exactly as the
/// calibration that measured it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Calibration {
    timing_nanos: f64,
    kinds: [(f64, f64); 7],
}

/// The kinds of work calibration times, in [Calibration::kinds]' order.
const CALIBRATED: [Work; 7] = [
    Work::Row,
    Work::Seek,
    Work::HashBuild,
    Work::HashProbe,
    Work::Sort,
    Work::BloomProbe,
    Work::WhereStep,
];

/// The keys of one page calibration times each kind of work over (`calibrate`).
fn calibration_keys() -> usize {
    crate::storage::buffer_pool::BufferPool::DEFAULT_PAGE_SIZE / std::mem::size_of::<u64>()
}

impl Calibration {
    /// Whether every time is a finite, positive number: one whose clock never moved isn't kept.
    fn timed(&self) -> bool {
        let good = |x: f64| x.is_finite() && x > 0.0;
        good(self.timing_nanos) && self.kinds.iter().all(|(u, n)| good(*u) && good(*n))
    }

    fn rates(&self) -> Rates {
        let mut rates = Rates {
            timing_nanos: self.timing_nanos,
            ..Rates::default()
        };
        for (kind, (units, nanos)) in CALIBRATED.iter().zip(self.kinds) {
            *rates.rate_mut(*kind) = Rate::measured(units, nanos);
        }
        // A page read can't be timed here: there's no file yet. Each database anchors it at open
        // (`time_page_read`, `LearnedCosts::anchor_page_read`), then its own reads move it
        // (`LearnedCosts::observe`), its epoch the pages whose rows calibration visited.
        rates.page = Rate {
            epoch: (rates.row.epoch / calibration_keys() as f64).max(1.0),
            ..Rate::default()
        };
        rates
    }
}

/// AUDIT §7.1 (perf/coldstart): a machine's calibration, kept across processes in the user's
/// cache (`$XDG_CACHE_HOME/amphora/calibration`, else `~/.cache/amphora/calibration`), one file a
/// CPU and executable, each holding the key it was measured under in full and the times as their
/// f64 bits, so a read back is the measurement itself. Anything else found there (another key, a
/// torn or foreign file) is measured again and replaced; a store that can't be read or written
/// leaves calibration once a process, as before.
mod stored {
    use super::{Calibration, CALIBRATED};
    use std::path::PathBuf;

    /// What a calibration was measured under: the CPU (its identity as the OS reports it) and the
    /// executable timed on it (its path, length and modification time: a rebuild is other code).
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct Key {
        pub(super) dir: PathBuf,
        pub(super) cpu: String,
        pub(super) exe: String,
    }

    impl Key {
        /// This process's key, None where the CPU, the executable or the cache can't be told.
        pub(super) fn here() -> Option<Self> {
            let cpu = cpu_identity()?;
            let exe = std::env::current_exe().ok()?;
            let meta = std::fs::metadata(&exe).ok()?;
            let modified = meta
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_nanos();
            let exe = format!("{} {} {modified}", exe.display(), meta.len());
            Some(Self {
                dir: cache_dir()?.join("amphora").join("calibration"),
                cpu,
                exe,
            })
        }

        /// The file this key's calibration is kept in: named by a hash of the CPU and the
        /// executable's path, so a rebuild replaces its own file instead of adding one.
        pub(super) fn file(&self) -> PathBuf {
            let path = self.exe.rsplitn(3, ' ').last().unwrap_or(&self.exe);
            self.dir
                .join(format!("{:016x}", fnv1a(&[self.cpu.as_bytes(), b"\0", path.as_bytes()])))
        }

        fn header(&self) -> String {
            format!("amphora-calibration 1\ncpu {}\nexe {}\n", escape(&self.cpu), escape(&self.exe))
        }
    }

    /// FNV-1a over `parts` (Fowler, Noll and Vo; its published 64-bit offset basis and prime).
    fn fnv1a(parts: &[&[u8]]) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in parts.iter().flat_map(|part| part.iter()) {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    /// One line's worth: line breaks and backslashes escaped.
    fn escape(text: &str) -> String {
        text.replace('\\', "\\\\").replace('\n', "\\n")
    }

    /// The calibration kept for `key`, None if there is none or what's there is anything else.
    pub(super) fn load(key: &Key) -> Option<Calibration> {
        let text = std::fs::read_to_string(key.file()).ok()?;
        parse(key, &text)
    }

    pub(super) fn parse(key: &Key, text: &str) -> Option<Calibration> {
        // Whole lines only, each time its f64's sixteen hex digits: a file cut short is refused,
        // not read as another number.
        let body = text.strip_prefix(&key.header())?.strip_suffix('\n')?;
        let mut lines = body.split('\n');
        let bits = |line: Option<&str>, name: &str| -> Option<Vec<f64>> {
            let mut words = line?.split(' ');
            if words.next()? != name {
                return None;
            }
            words
                .map(|w| {
                    let digits = w.len() == 16 && w.bytes().all(|b| b.is_ascii_hexdigit());
                    digits.then(|| u64::from_str_radix(w, 16).ok().map(f64::from_bits))?
                })
                .collect()
        };
        let timing = bits(lines.next(), "timing")?;
        let [timing_nanos] = timing.as_slice() else {
            return None;
        };
        let mut kinds = [(0.0, 0.0); 7];
        for (slot, kind) in kinds.iter_mut().zip(CALIBRATED) {
            let pair = bits(lines.next(), &format!("{kind:?}"))?;
            let [units, nanos] = pair.as_slice() else {
                return None;
            };
            *slot = (*units, *nanos);
        }
        if lines.next().is_some() {
            return None;
        }
        let kept = Calibration {
            timing_nanos: *timing_nanos,
            kinds,
        };
        if kept.timed() {
            Some(kept)
        } else {
            None
        }
    }

    pub(super) fn render(key: &Key, calibration: &Calibration) -> String {
        let mut text = key.header();
        text.push_str(&format!("timing {:016x}\n", calibration.timing_nanos.to_bits()));
        for (kind, (units, nanos)) in CALIBRATED.iter().zip(calibration.kinds) {
            text.push_str(&format!("{kind:?} {:016x} {:016x}\n", units.to_bits(), nanos.to_bits()));
        }
        text
    }

    /// Keeps `calibration` for `key`: written to a file of its own, then renamed over the key's, so
    /// a process reading it sees the old file or the new one whole, never part of either. A
    /// calibration whose clock never moved isn't kept. Failure is logged and leaves no file behind.
    pub(super) fn save(key: &Key, calibration: &Calibration) {
        if !calibration.timed() {
            return;
        }
        let file = key.file();
        let partial = key.dir.join(format!(".{}", uuid::Uuid::new_v4().simple()));
        let written = std::fs::create_dir_all(&key.dir)
            .and_then(|()| std::fs::write(&partial, render(key, calibration)))
            .and_then(|()| std::fs::rename(&partial, &file));
        if let Err(err) = written {
            let _ = std::fs::remove_file(&partial);
            tracing::debug!("AUDIT 7.1: calibration not kept at {}: {err}", file.display());
        }
    }

    fn cache_dir() -> Option<PathBuf> {
        let absolute = |var: &str| {
            std::env::var_os(var)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };
        absolute("XDG_CACHE_HOME").or_else(|| absolute("HOME").map(|home| home.join(".cache")))
    }

    /// The CPU as Linux reports it: the first processor's record of `/proc/cpuinfo` (its model,
    /// stepping, microcode, flags and bugs), less what moves while it runs (its clock, `cpu MHz`,
    /// and `bogomips`, measured at each boot). Only the first record is read: the kernel
    /// generates the file a record at a time as it is read.
    #[cfg(target_os = "linux")]
    fn cpu_identity() -> Option<String> {
        use std::io::Read;
        let mut file = std::fs::File::open("/proc/cpuinfo").ok()?;
        let mut head = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let read = file.read(&mut chunk).ok()?;
            let got = chunk.get(..read)?;
            head.extend_from_slice(got);
            if read == 0 || head.windows(2).any(|w| w == b"\n\n") {
                break;
            }
        }
        cpu_record(std::str::from_utf8(&head).ok()?)
    }

    /// Other platforms name their CPU through calls this crate doesn't make (no unsafe code): each
    /// process calibrates, as before.
    #[cfg(not(target_os = "linux"))]
    fn cpu_identity() -> Option<String> {
        None
    }

    /// The first record of a `/proc/cpuinfo`, its fields that don't move, one a line; None without
    /// a whole record. Linux's alone: no other platform has the file.
    #[cfg(any(target_os = "linux", test))]
    pub(super) fn cpu_record(text: &str) -> Option<String> {
        let (record, _) = text.split_once("\n\n")?;
        let kept: Vec<&str> = record
            .lines()
            .filter(|line| {
                let name = line.split(':').next().unwrap_or("").trim();
                !matches!(name, "cpu MHz" | "bogomips" | "BogoMIPS")
            })
            .collect();
        (!kept.is_empty()).then(|| kept.join("\n"))
    }
}

/// SplitMix64's output function (Steele, Lea and Flood, "Fast splittable pseudorandom number
/// generators", OOPSLA 2014; its published constants): keys in no order, the same every run.
pub(crate) fn mix(i: u64) -> u64 {
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
/// (`BufferPool::DEFAULT_PAGE_SIZE`), from repeated samples whose spread says when to stop.
///
/// A sample is one kind's work repeated, up to once per key, until its time reaches the slowest
/// kind's single run (`measure`), so the clock's step weighs on each about equally. The kinds are
/// sampled in rounds, one sample of each kind a round, so a stretch of host load falls on every
/// kind alike and leaves their ratios, the planner's weights, where they were. Each kind's rate is
/// the median of its samples: a preempted sample is one long time, which moves a mean by its whole
/// length and a median by one rank.
///
/// A kind stops being sampled once its median's standard error is no larger than the timer's own
/// cost, a pair of clock reads: the median is then known as finely as one reading resolves, and
/// more samples can't sharpen it (`median_settled`). Its spread sets how many samples that takes:
/// few on a quiet host, more under load. The rounds stop at a page's keys, as each sample's
/// repetitions do, so calibration's time stays bounded however loaded the host is.
#[cfg(test)]
fn calibrate(clock: &mut dyn FnMut() -> f64) -> Rates {
    measure_calibration(clock).rates()
}

/// [calibrate]'s measurement, the times it makes its rates from (`Calibration`).
fn measure_calibration(clock: &mut dyn FnMut() -> f64) -> Calibration {
    let n = calibration_keys();
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
    // A hash join's filter over the page's keys, sized as the planner sizes one (PL.5,
    // `FilterSizing`) for one false positive expected across a page of keys it doesn't hold,
    // and looked up for such a page: the probe rows a filter is for. Its memory is bounded by the
    // page's keys' hashes and as many bytes again for the filter built beside them (QT.5, lane
    // qs5-19: a sized filter's bound holds both at once): its rate, one in a page's keys, takes
    // `ln(rows)/ln²2` bits a key, fewer than a hash's 64 for any page.
    let fp_per_key = 1.0 / (rows * rows);
    let mut filter =
        crate::vdbe::bloom_filter::BloomFilter::sized(crate::vdbe::bloom_filter::FilterSizing {
            fp_per_key,
            max_bytes: n.saturating_mul(2 * std::mem::size_of::<u64>()),
        });
    for k in &keys {
        filter.insert_i64(*k as i64);
    }
    filter.seal();
    let absent: Vec<i64> = (n as u64..2 * n as u64).map(|i| mix(i) as i64).collect();
    let mut bloom = || {
        for k in &absent {
            black_box(filter.contains_i64(black_box(*k)));
        }
    };
    // A condition's step: each key, as one of the engine's values, compared with another.
    let values: Vec<crate::Value> = keys
        .iter()
        .map(|k| crate::Value::from_i64(*k as i64))
        .collect();
    let pivot = crate::Value::from_i64(0);
    let mut step = || {
        for v in &values {
            black_box(black_box(v) < &pivot);
        }
    };
    let mut kinds: [(Work, f64, &mut dyn FnMut()); 7] = [
        (Work::Row, rows, &mut row),
        (Work::Seek, rows, &mut seek),
        (Work::HashBuild, rows, &mut build),
        (Work::HashProbe, rows, &mut probe),
        (Work::Sort, rows * rows.log2(), &mut sort),
        (Work::BloomProbe, rows, &mut bloom),
        (Work::WhereStep, rows, &mut step),
    ];
    let mut target: f64 = 0.0;
    for (_, _, work) in kinds.iter_mut() {
        target = target.max(measure(clock, 0.0, 1, &mut **work).1);
    }
    let mut calibration = Calibration {
        timing_nanos: 0.0,
        kinds: [(0.0, 0.0); 7],
    };
    // A pair of clock reads, timed as the kinds are, a page's keys' worth of times at most: what
    // timing one run costs (step 3a), and the finest a sample's reading resolves.
    let mut timing = || {
        let start = Instant::now();
        black_box(start.elapsed());
    };
    let (reps, nanos) = measure(clock, target, n, &mut timing);
    calibration.timing_nanos = nanos / reps;
    // Each kind's repetitions a sample, found as `measure` finds them; then its samples, a round
    // at a time.
    let mut sampled: [(usize, Vec<f64>, bool); 7] = Default::default();
    for ((_, _, work), (reps, _, _)) in kinds.iter_mut().zip(sampled.iter_mut()) {
        *reps = measure(clock, target, n, &mut **work).0 as usize;
    }
    for round in 1..=n {
        for ((_, _, work), (reps, samples, settled)) in kinds.iter_mut().zip(sampled.iter_mut()) {
            if *settled {
                continue;
            }
            let start = clock();
            for _ in 0..*reps {
                work();
            }
            samples.push(clock() - start);
        }
        // The spread is read each time the samples double, as `measure` doubles repetitions.
        if round.is_power_of_two() {
            for (_, samples, settled) in sampled.iter_mut() {
                *settled = *settled || median_settled(samples, calibration.timing_nanos);
            }
            if sampled.iter().all(|(_, _, settled)| *settled) {
                break;
            }
        }
    }
    for ((_, units, _), ((reps, samples, _), slot)) in kinds
        .iter()
        .zip(sampled.iter_mut().zip(calibration.kinds.iter_mut()))
    {
        let nanos = median(samples).unwrap_or(0.0);
        *slot = (*reps as f64 * *units, nanos);
    }
    calibration
}

/// The middle of `samples`, sorting them; None for none.
fn median(samples: &mut [f64]) -> Option<f64> {
    samples.sort_unstable_by(f64::total_cmp);
    let mid = samples.len() / 2;
    let upper = *samples.get(mid)?;
    if samples.len().is_multiple_of(2) {
        let lower = *samples.get(mid.checked_sub(1)?)?;
        Some((lower + upper) / 2.0)
    } else {
        Some(upper)
    }
}

/// Whether the median of `samples` (sorted here) is known to within `resolution`: half the
/// distance between the order statistics a standard error either side of the middle rank, no
/// wider. Among `n` samples the count below the true median is binomial, n/2 with standard
/// deviation sqrt(n)/2, so the samples at ranks n/2 -+ sqrt(n)/2 bracket the median by about one
/// standard error whatever the times' distribution (the order-statistic interval for a median:
/// Thompson, "On confidence ranges for the median and other expectation distributions for
/// populations of unknown distribution form", Annals of Mathematical Statistics 7(3), 1936).
/// Under two samples there's no spread to read.
fn median_settled(samples: &mut [f64], resolution: f64) -> bool {
    median_se(samples).is_some_and(|se| se <= resolution)
}

/// The standard error of the median of `samples` (sorted here): half the distance between the
/// order statistics a standard error either side of the middle rank (`median_settled`). None
/// under two samples.
fn median_se(samples: &mut [f64]) -> Option<f64> {
    let count = samples.len();
    if count < 2 {
        return None;
    }
    samples.sort_unstable_by(f64::total_cmp);
    let half = count as f64 / 2.0;
    let reach = (count as f64).sqrt() / 2.0;
    let last = (count - 1) as f64;
    let low = (half - reach).floor().clamp(0.0, last) as usize;
    let high = (half + reach).ceil().clamp(0.0, last) as usize;
    match (samples.get(low), samples.get(high)) {
        (Some(low), Some(high)) => Some((high - low) / 2.0),
        _ => None,
    }
}

/// Where a page read was timed (AUDIT §7.1, perf/coldstart): the device a database's file is on,
/// or memory. A page read costs what its device and the IO reading it make it cost, so one timed
/// there stands for every database the process opens there, until the database's own reads replace
/// it (`LearnedCosts::anchor_page_read`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum PageReadPlace {
    /// Files an IO keeps in memory (`IO::is_memory`) or a database named as memory.
    Memory,
    /// The device (`st_dev`) of the directory the database's file is in. A device the process
    /// hasn't timed (another filesystem mounted, a database moved to another disk) is a new place,
    /// timed at its first open.
    #[cfg(unix)]
    Device(u64),
}

/// The place a page read beside `db_path` through `io` is timed at; None where the place can't be
/// told (no device read, or a platform that gives none here), whose opens each time their own.
fn page_read_place(db_path: &str, io: &dyn crate::io::IO) -> Option<PageReadPlace> {
    if io.is_memory() || crate::database::is_memory_like(db_path) {
        return Some(PageReadPlace::Memory);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let parent = std::path::Path::new(db_path)
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        std::fs::metadata(parent)
            .ok()
            .map(|meta| PageReadPlace::Device(meta.dev()))
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// The page reads this process timed, by the IO's way of reading (`IO::page_read_backend`) and
/// the place (`PageReadPlace`): (reads, nanoseconds), as `time_page_read` gives them. Bounded by
/// the IO backends compiled in times the devices the process opens databases on (the mounted
/// filesystems); an entry is never evicted, as a device's read cost is the same all process long.
type PageReads = FxHashMap<(&'static str, PageReadPlace), (f64, f64)>;

fn page_reads() -> &'static Mutex<PageReads> {
    static PAGE_READS: OnceLock<Mutex<PageReads>> = OnceLock::new();
    PAGE_READS.get_or_init(|| Mutex::new(FxHashMap::default()))
}

/// AUDIT §7.1 (perf/coldstart): the page read a database opened at `db_path` through `io` starts
/// from: the one this process already timed on the same IO backend and device, else one timed now
/// beside it (`time_page_read`), and kept for the next open there. A database opened read-only
/// (`writable` false) takes none, as before: its open reads its own file. An IO that names
/// no backend (`IO::page_read_backend`), or a place that can't be told, times its own each open,
/// as every open did before.
pub(crate) fn page_read_anchor(
    db_path: &str,
    io: &crate::sync::Arc<dyn crate::io::IO>,
    page_size: usize,
    most_reads: usize,
    writable: bool,
) -> Option<(f64, f64)> {
    let key = io
        .page_read_backend()
        .zip(page_read_place(db_path, io.as_ref()));
    if !writable {
        return None;
    }
    if let Some(key) = &key {
        if let Some(timed) = page_reads().lock().get(key) {
            return Some(*timed);
        }
    }
    let timed = time_page_read(db_path, io, page_size, most_reads)?;
    if let Some(key) = key {
        page_reads().lock().entry(key).or_insert(timed);
    }
    Some(timed)
}

/// Times page reads at a database's open, before it has read a page of its own (QT.5): a scratch
/// page of `page_size` bytes, written beside the database through its own IO and read back as its
/// pager reads (`read_through_io`). (reads, nanoseconds), or None if no scratch page could be
/// written there (a database whose directory takes no file is one opened read-only, whose open
/// reads its own file).
///
/// The reads come from the OS's cache, as the database's own reads do while its pages are cached.
/// They are not read uncached (`O_DIRECT`, `F_NOCACHE`, `FILE_FLAG_NO_BUFFERING`): that prices a
/// cached database's page at a device read, which made row visits all but free against it and
/// flipped the planner's plans for databases held in memory (lane qs5-4, kiln run r326 on Windows:
/// three of `tests/planner.rs`'s plans). A database whose reads go to the device teaches the
/// estimator that with its own reads: the first epoch of them replaces these
/// (`LearnedCosts::anchor_page_read`).
///
/// The first read is not the rate: it pays for the IO's first use (on jupiter, Windows, the
/// process's first read took 121.7 us, the reads after it 24.7 to 38.9 us; lane qs5-5's probe,
/// kiln run r558), and a single read anchored on it flipped the key joins of
/// `tests/records_plans.rs`'s `a_join_on_a_whole_key_searches_it_without_statistics` (kiln run
/// r546; it passes with this, run r649). So the reads after it are timed, doubling in number, until
/// their time reaches the first's or they number `most_reads`, as calibration times each kind of
/// work until its time reaches the slowest single run (`measure`).
pub(crate) fn time_page_read(
    db_path: &str,
    io: &crate::sync::Arc<dyn crate::io::IO>,
    page_size: usize,
    most_reads: usize,
) -> Option<(f64, f64)> {
    let page: Vec<u8> = (0..page_size).map(|i| mix(i as u64) as u8).collect();
    let scratch = if crate::database::is_memory_like(db_path) {
        format!(":memory:-page-read-{}", uuid::Uuid::new_v4().simple())
    } else {
        format!("{db_path}-page-read-{}", uuid::Uuid::new_v4().simple())
    };
    match read_through_io(io, &scratch, page, most_reads) {
        Ok(timed) => Some(timed),
        Err(err) => {
            tracing::debug!("QT.5: no scratch page read beside {db_path}: {err}");
            None
        }
    }
}

/// Writes `page` to `scratch` through `io` and reads it back as the pager reads, each read from
/// its submission to its completion: the first read alone, then the reads after it as
/// `time_page_read` says; (those reads, their nanoseconds). The file is removed whatever happens.
fn read_through_io(
    io: &crate::sync::Arc<dyn crate::io::IO>,
    scratch: &str,
    page: Vec<u8>,
    most_reads: usize,
) -> crate::Result<(f64, f64)> {
    use crate::io::{Buffer, Completion, OpenFlags};
    use crate::sync::Arc;
    let file = io.open_file(scratch, OpenFlags::Create, false)?;
    let timed = (|| -> crate::Result<(f64, f64)> {
        let len = page.len();
        let written = file.pwrite(
            0,
            Arc::new(Buffer::new(page)),
            Completion::new_write(|_| {}),
        )?;
        io.wait_for_completion(written)?;
        let read_once = || -> crate::Result<()> {
            let read = file.pread(
                0,
                Completion::new_read(Arc::new(Buffer::new(vec![0; len])), move |res| match res {
                    Ok((_, n)) if n as usize == len => None,
                    Ok((_, n)) => Some(crate::error::CompletionError::ShortRead {
                        page_idx: 0,
                        expected: len,
                        actual: n.max(0) as usize,
                    }),
                    Err(err) => Some(err),
                }),
            )?;
            io.wait_for_completion(read)
        };
        let start = std::time::Instant::now();
        read_once()?;
        let first = start.elapsed().as_nanos() as f64;
        let most_reads = most_reads.max(1);
        let mut reads: usize = 1;
        loop {
            let start = std::time::Instant::now();
            for _ in 0..reads {
                read_once()?;
            }
            let nanos = start.elapsed().as_nanos() as f64;
            match reads.checked_mul(2) {
                Some(next) if nanos < first && next <= most_reads => reads = next,
                _ => return Ok((reads as f64, nanos)),
            }
        }
    })();
    drop(file);
    let removed = io.remove_file(scratch);
    let timed = timed?;
    removed?;
    Ok(timed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audit_7_1_key(dir: &std::path::Path) -> stored::Key {
        stored::Key {
            dir: dir.to_path_buf(),
            cpu: "vendor_id\t: GenuineTest\nmodel name\t: Test CPU".into(),
            exe: "/bin/amphora-test 1234 5678".into(),
        }
    }

    fn audit_7_1_calibration() -> Calibration {
        Calibration {
            timing_nanos: 21.5,
            kinds: [
                (512.0, 300.25),
                (512.0, 9_000.0),
                (256.0, 7_700.5),
                (512.0, 4_100.0),
                (4_608.0, 20_000.125),
                (512.0, 1_000.0),
                (512.0, 650.0),
            ],
        }
    }

    /// Do: keep a calibration for a key, then read it back (AUDIT §7.1). Expect: the very same
    /// times, so the rates, the weights and the page read's epoch are the measured ones exactly.
    #[test]
    fn audit_7_1_kept_calibration_reads_back_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let key = audit_7_1_key(dir.path());
        let measured = audit_7_1_calibration();
        stored::save(&key, &measured);
        let kept = stored::load(&key).unwrap();
        assert_eq!(kept, measured);
        assert_eq!(kept.rates(), measured.rates());
        assert_eq!(
            LearnedCosts::from_rates(&kept.rates()).weights(),
            LearnedCosts::from_rates(&measured.rates()).weights()
        );
        // The write left only the key's own file.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    /// Do: read a kept calibration under another CPU, a rebuilt executable, and from torn or
    /// foreign files. Expect: none of them is taken (each is measured again); a rebuild of the
    /// same executable keeps its file's name, so it replaces it rather than adding one.
    #[test]
    fn audit_7_1_another_cpu_or_build_measures_again() {
        let dir = tempfile::tempdir().unwrap();
        let key = audit_7_1_key(dir.path());
        let measured = audit_7_1_calibration();
        let text = stored::render(&key, &measured);
        assert_eq!(stored::parse(&key, &text), Some(measured));

        let other_cpu = stored::Key {
            cpu: "vendor_id\t: GenuineTest\nmodel name\t: Test CPU v2".into(),
            ..key.clone()
        };
        assert_eq!(stored::parse(&other_cpu, &text), None);
        assert_ne!(other_cpu.file(), key.file());

        let rebuilt = stored::Key {
            exe: "/bin/amphora-test 1234 9999".into(),
            ..key.clone()
        };
        assert_eq!(stored::parse(&rebuilt, &text), None);
        assert_eq!(rebuilt.file(), key.file());

        let torn = text.get(..text.len() - 5).unwrap();
        assert_eq!(stored::parse(&key, torn), None);
        assert_eq!(stored::parse(&key, &format!("{text}Extra 0 0\n")), None);
        assert_eq!(stored::parse(&key, "not a calibration"), None);
        assert_eq!(stored::load(&key), None);

        // A calibration whose clock never moved isn't kept.
        let unmoved = Calibration {
            timing_nanos: 0.0,
            ..measured
        };
        stored::save(&key, &unmoved);
        assert_eq!(stored::load(&key), None);
    }

    /// Do: read the CPU's identity from a `/proc/cpuinfo` whose clock moved between two reads,
    /// and from one cut off before its first record ends. Expect: the same identity for the first
    /// two; none for the last; another stepping is another identity (AUDIT §7.1).
    #[test]
    fn audit_7_1_cpu_identity_ignores_the_clock() {
        let at = |mhz: &str, stepping: &str| {
            format!(
                "processor\t: 0\nvendor_id\t: AuthenticAMD\nmodel name\t: Test 9950X\nstepping\t: {stepping}\ncpu MHz\t\t: {mhz}\nbogomips\t: 8583.{mhz}\nflags\t\t: fpu sse2\n\nprocessor\t: 1\ncpu MHz\t\t: 1\n"
            )
        };
        let first = stored::cpu_record(&at("4291.0", "0")).unwrap();
        assert_eq!(stored::cpu_record(&at("599.9", "0")).unwrap(), first);
        assert!(!first.contains("MHz") && !first.contains("bogomips"));
        assert!(first.contains("flags"));
        assert_ne!(stored::cpu_record(&at("4291.0", "1")).unwrap(), first);
        assert_eq!(stored::cpu_record("processor\t: 0\nvendor_id\t: A"), None);
    }

    /// Do: calibrate on this machine twice through the process's store (AUDIT §7.1). Expect: the
    /// process's calibration is sane, and where the machine can be told it is kept for the next
    /// process under this CPU and executable.
    #[test]
    fn audit_7_1_process_calibration_is_kept_for_the_machine() {
        let rates = process_calibration();
        assert!(LearnedCosts::from_rates(rates).weights().is_some());
        if let Some(key) = stored::Key::here() {
            if let Some(kept) = stored::load(&key) {
                assert!(kept.timed());
            }
        }
    }

    fn fixed_rates(row: f64, seek: f64, build: f64, probe: f64, sort: f64) -> Rates {
        Rates {
            row: Rate::measured(100.0, 100.0 * row),
            seek: Rate::measured(100.0, 100.0 * seek),
            hash_build: Rate::measured(100.0, 100.0 * build),
            hash_probe: Rate::measured(100.0, 100.0 * probe),
            sort: Rate::measured(100.0, 100.0 * sort),
            bloom_probe: Rate::measured(100.0, 100.0 * probe / 4.0),
            where_step: Rate::measured(100.0, 100.0 * row),
            page: Rate {
                epoch: 4.0,
                ..Rate::default()
            },
            timing_nanos: 20.0,
        }
    }

    /// Do: a timer of 20 ns, epochs of 100 samples, times spread 25 ns, 2,000 ns and 0.01 ns, and
    /// none. Expect: one run in (20 * 10 / 25)^(2/3) = 4 timed; every run when the times scatter
    /// past the timer's cost; one in the epoch when they barely move; no change without a spread
    /// (step 3a: the sampling follows each operation's spread, no picked rate).
    #[test]
    fn t_qt_5_5_sampling_follows_the_spread() {
        assert_eq!(sample_every(20.0, 100.0, Some(25.0)), Some(4));
        assert_eq!(sample_every(20.0, 100.0, Some(2_000.0)), Some(1));
        assert_eq!(sample_every(20.0, 100.0, Some(0.01)), Some(100));
        assert_eq!(sample_every(20.0, 100.0, Some(0.0)), None);
        assert_eq!(sample_every(20.0, 100.0, None), None);
    }

    /// Do: time every seek of an epoch, 100 ns and 150 ns in turn, then run 120 more. Expect:
    /// every run of the first epoch timed; then some but not all; the seek's rate moved toward
    /// the times observed, the hash probe's untouched (step 3a).
    #[test]
    fn t_qt_5_5_sampled_seeks_move_the_rate_and_thin_out() {
        let costs = LearnedCosts::from_rates(&fixed_rates(1.0, 10.0, 20.0, 15.0, 2.0));
        let runs = RunCounts::default();
        for i in 0..100 {
            assert!(costs.sampled(Work::Seek, &runs));
            costs.observe_sampled(Work::Seek, if i % 2 == 0 { 100.0 } else { 150.0 });
        }
        let timed = (0..120).filter(|_| costs.sampled(Work::Seek, &runs)).count();
        assert!(timed > 0 && timed < 120, "{timed}");
        let weights = costs.weights().unwrap();
        assert!((weights.seek - 125.0).abs() < 1e-9, "{weights:?}");
        assert_eq!(weights.hash_probe, 15.0);
        assert!(!costs.sampled(Work::Sort, &runs));
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
            weights.bloom_probe,
            weights.where_step,
        ] {
            assert!(w.is_finite() && w > 0.0, "{weights:?}");
        }
    }

    /// Do: start from known rates, then observe sorts at four times the calibrated rate for as
    /// much work as two epochs. Expect: the sort weight moves to four times its start, the others
    /// stay (T-QT.5.5: the estimator follows observed times).
    #[test]
    fn t_qt_5_5_observed_times_move_the_weight() {
        let costs = LearnedCosts::from_rates(&fixed_rates(1.0, 10.0, 20.0, 15.0, 2.0));
        let start = costs.weights().unwrap();
        assert_eq!(start.sort, 2.0);
        costs.observe(Work::Sort, 50.0, 50.0 * 8.0);
        let partway = costs.weights().unwrap();
        // PL.8 (T-PL.8.6): half an epoch is no evidence yet; the weight holds.
        assert_eq!(partway.sort, 2.0, "{partway:?}");
        for _ in 0..4 {
            costs.observe(Work::Sort, 50.0, 50.0 * 8.0);
        }
        let moved = costs.weights().unwrap();
        assert_eq!(moved.sort, 8.0);
        assert_eq!(moved.seek, start.seek);
        assert_eq!(moved.hash_build, start.hash_build);
        assert_eq!(moved.hash_probe, start.hash_probe);
    }

    /// Do: settle the seek's rate on a first batch at 125 ns, then close twelve batches whose
    /// means wander 10% either way of it, every time inside a batch alike, then one batch 20% slow
    /// (a burst of load within the wander), then one four times slower. Expect: the weight where
    /// the first batch put it through the wander and the burst, though each batch's own times
    /// have no spread at all; then at the slow batch's mean (T-PL.8.6, lane pl8-14: a weight's
    /// error is the spread between batch means, so a burst that slows one whole batch is no
    /// evidence, and a change past that spread is).
    #[test]
    fn t_pl_8_6_a_burst_of_load_inside_the_batches_spread_moves_nothing() {
        let costs = LearnedCosts::from_rates(&fixed_rates(1.0, 10.0, 20.0, 15.0, 2.0));
        let batch = |nanos: f64| {
            for _ in 0..100 {
                costs.observe_sampled(Work::Seek, nanos);
            }
        };
        batch(125.0);
        assert_eq!(costs.weights().unwrap().seek, 125.0);
        for i in 0..12 {
            batch(if i % 2 == 0 { 112.5 } else { 137.5 });
            assert_eq!(costs.weights().unwrap().seek, 125.0, "{i}");
        }
        batch(150.0);
        assert_eq!(costs.weights().unwrap().seek, 125.0);
        batch(500.0);
        assert_eq!(costs.weights().unwrap().seek, 500.0);
    }

    /// Do: a first batch of seeks timed at 125 ns, two of them preempted for 4 ms, then a later
    /// batch with a preempted run of its own. Expect: the weight at 125 throughout: a preempted run
    /// times the scheduler, not the seek (T-PL.8.2, lane pl8-22; the mean of the first batch was
    /// 80,122.5 ns, and a weight like it flipped the bushy plans under load in kiln run r124).
    #[test]
    fn t_pl_8_2_preempted_runs_move_no_weight() {
        let costs = LearnedCosts::from_rates(&fixed_rates(1.0, 10.0, 20.0, 15.0, 2.0));
        for i in 0..100 {
            let nanos = if i == 17 || i == 61 {
                4_000_000.0
            } else {
                125.0
            };
            costs.observe_sampled(Work::Seek, nanos);
        }
        assert_eq!(costs.weights().unwrap().seek, 125.0);
        for i in 0..300 {
            costs.observe_sampled(Work::Seek, if i == 150 { 4_000_000.0 } else { 125.0 });
        }
        assert_eq!(costs.weights().unwrap().seek, 125.0);
    }

    /// Do: a first batch, then a second far from it. Expect: the second holds: one batch mean
    /// beside another has no spread between batches to be judged by (T-PL.8.6, lane pl8-14).
    #[test]
    fn t_pl_8_6_two_batches_alone_move_nothing() {
        let mut rate = Rate::measured(2.0, 2.0);
        rate.observe(1.0, 4.0);
        rate.observe(1.0, 4.0);
        assert_eq!(rate.nanos_per_unit(), Some(4.0));
        rate.observe(1.0, 40.0);
        rate.observe(1.0, 40.0);
        assert_eq!(rate.nanos_per_unit(), Some(4.0));
    }

    /// Do: an epoch of one observation, however large its work. Expect: it closes only once a
    /// second joins it (T-PL.8.6: one time alone is one moment's load).
    #[test]
    fn t_pl_8_6_one_observation_alone_moves_nothing() {
        let costs = LearnedCosts::from_rates(&fixed_rates(1.0, 10.0, 20.0, 15.0, 2.0));
        costs.observe(Work::Sort, 1_000.0, 1_000.0 * 8.0);
        assert_eq!(costs.weights().unwrap().sort, 2.0);
        costs.observe(Work::Sort, 1_000.0, 1_000.0 * 8.0);
        assert_eq!(costs.weights().unwrap().sort, 8.0);
    }

    /// Do: find where the quantile's expansion first holds, and compare it there and far past it
    /// with the closed form's bisection. Expect: the expansion takes over only once exact to well
    /// inside the tables' precision, and it takes over at all, so a long-running process's test
    /// stays bounded (T-PL.8.6, lane pl8-15).
    #[test]
    fn t_pl_8_6_the_expanded_quantile_agrees_where_it_takes_over() {
        assert_eq!(move_t_expanded(100.0), None);
        let mut nu = 1.0_f64;
        while move_t_expanded(nu).is_none() {
            nu *= 2.0;
            assert!(nu < 1e9, "the expansion never takes over");
        }
        for nu in [nu, nu * 4.0] {
            let (expanded, exact) = (
                move_t_expanded(nu).unwrap(),
                student_t(MOVE_LEVEL, nu).unwrap(),
            );
            assert!(
                (expanded - exact).abs() < 1e-9,
                "{nu}: {expanded} vs {exact}"
            );
        }
    }

    /// Do: judge batches against a spread, with a ratio just past and just inside Student's t for
    /// three degrees of freedom. Expect: only the one past it moves the rate (T-PL.8.6).
    #[test]
    fn t_pl_8_6_a_batch_moves_the_rate_only_past_students_t() {
        let batches = Batches {
            last: Some(0.0),
            differences: 3.0,
            squares: 3.0,
        };
        let t = student_t(MOVE_LEVEL, 3.0).unwrap();
        assert!(batches.refutes(0.0, t * 1.001));
        assert!(!batches.refutes(0.0, t * 0.999));
        assert!(!batches.refutes(0.0, MOVE_Z));
    }

    /// Do: Student's t quantiles from the closed forms. Expect: the tables' values (Abramowitz and
    /// Stegun, table 26.10: 12.706 for one degree of freedom at 95%, 2.228 for ten, 4.032 for five
    /// at 99%), and the normal quantile [MOVE_Z] approached as the degrees of freedom grow
    /// (T-PL.8.6, lane pl8-14).
    #[test]
    fn t_pl_8_6_student_t_matches_its_tables() {
        let near = |got: Option<f64>, want: f64| got.is_some_and(|t| (t - want).abs() < 1e-3);
        assert!(
            near(student_t(0.95, 1.0), 12.706),
            "{:?}",
            student_t(0.95, 1.0)
        );
        assert!(
            near(student_t(0.95, 10.0), 2.228),
            "{:?}",
            student_t(0.95, 10.0)
        );
        assert!(
            near(student_t(0.99, 5.0), 4.032),
            "{:?}",
            student_t(0.99, 5.0)
        );
        assert!(
            near(student_t(0.95, 2.0), 4.303),
            "{:?}",
            student_t(0.95, 2.0)
        );
        let large = student_t(MOVE_LEVEL, 10_000.0).unwrap();
        assert!(large > MOVE_Z && large - MOVE_Z < 1e-3, "{large}");
        assert_eq!(student_t(0.95, 0.0), None);
    }

    /// Do: observe nothing-sized and non-finite times. Expect: the weights unchanged (T-QT.5.5).
    #[test]
    fn t_qt_5_5_meaningless_observations_leave_the_rates() {
        let costs = LearnedCosts::from_rates(&fixed_rates(1.0, 10.0, 20.0, 15.0, 2.0));
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
        let fast = LearnedCosts::from_rates(&fixed_rates(1.0, 10.0, 20.0, 15.0, 2.0));
        let slow = LearnedCosts::from_rates(&fixed_rates(2.0, 10.0, 20.0, 15.0, 2.0));
        let (fast, slow) = (fast.weights().unwrap(), slow.weights().unwrap());
        assert_eq!(slow.seek * 2.0, fast.seek);
        assert_eq!(slow.sort * 2.0, fast.sort);
    }

    /// Do: plan before the database read any page from its file, then after it timed page reads at
    /// 50 rows' visits each, then after reads turn four times slower for two epochs. Expect: no
    /// row-to-page weight at first, then a fiftieth, then a two-hundredth (the anchor learned where
    /// it runs, and followed as the device moves).
    #[test]
    fn t_qt_5_5_the_row_is_weighed_against_observed_page_reads() {
        let costs = LearnedCosts::from_rates(&fixed_rates(2.0, 10.0, 20.0, 15.0, 2.0));
        assert_eq!(costs.weights().unwrap().row_in_pages, None);
        costs.observe(Work::PageRead, 1.0, 100.0);
        assert_eq!(costs.weights().unwrap().row_in_pages, Some(2.0 / 100.0));
        for _ in 0..3 {
            costs.observe(Work::PageRead, 1.0, 100.0);
        }
        assert_eq!(costs.weights().unwrap().row_in_pages, Some(2.0 / 100.0));
        // T-PL.8.6 (lane pl8-14): batches 10% either way show the reads' noise between batches.
        for i in 0..8 {
            for _ in 0..4 {
                costs.observe(Work::PageRead, 1.0, if i % 2 == 0 { 90.0 } else { 110.0 });
            }
        }
        assert_eq!(costs.weights().unwrap().row_in_pages, Some(2.0 / 100.0));
        for _ in 0..4 {
            costs.observe(Work::PageRead, 1.0, 400.0);
        }
        assert_eq!(costs.weights().unwrap().row_in_pages, Some(2.0 / 400.0));
    }

    /// Do: the weights with no page read timed, and with one, applied to the planner's parameters.
    /// Expect: the row's cost stays the parameters' until a page read is timed, then is the
    /// learned ratio, and every other weight follows it (T-QT.5.5).
    #[test]
    fn t_qt_5_5_the_planner_takes_the_learned_row_cost() {
        let params = super::super::cost_params::CostModelParams::new();
        let costs = LearnedCosts::from_rates(&fixed_rates(2.0, 10.0, 20.0, 15.0, 2.0));
        let before = params.with_learned(costs.weights());
        assert_eq!(before.cpu_cost_per_row, params.cpu_cost_per_row);
        costs.observe(Work::PageRead, 1.0, 128.0);
        let after = params.with_learned(costs.weights());
        assert_eq!(after.cpu_cost_per_row, 2.0 / 128.0);
        assert_eq!(after.cpu_cost_per_seek, 10.0 / 128.0);
        assert_eq!(after.hash_insert_cost, 20.0 / 128.0);
        assert_eq!(after.bloom_lookup_cost, 15.0 / 4.0 / 128.0);
        assert_eq!(after.cpu_cost_per_where_step, 2.0 / 128.0);
    }

    /// Do: weigh with no fetch yet, with a share of 0.25, and with a share out of range. Expect:
    /// the share passed through as the planner's page miss, clamped to [0, 1] (T-QT.5.5).
    #[test]
    fn t_qt_5_5_the_page_miss_share_reaches_the_planner() {
        let costs = LearnedCosts::from_rates(&fixed_rates(2.0, 10.0, 20.0, 15.0, 2.0));
        let params = super::super::cost_params::CostModelParams::new();
        assert_eq!(params.with_learned(costs.weights_at(None)).page_miss, 1.0);
        for (miss, want) in [(Some(0.25), 0.25), (Some(3.0), 1.0), (Some(f64::NAN), 1.0)] {
            assert_eq!(params.with_learned(costs.weights_at(miss)).page_miss, want);
        }
    }

    /// Do: anchor the page read at 100 ns, as a database's open does, then read pages of its own
    /// at 400 ns each for one epoch. Expect: the row weighed against the anchor at first, then
    /// against the database's own reads alone (the anchor replaced, no fixed number left).
    #[test]
    fn t_qt_5_5_the_open_anchor_stands_until_the_database_reads() {
        let costs = LearnedCosts::from_rates(&fixed_rates(2.0, 10.0, 20.0, 15.0, 2.0));
        costs.anchor_page_read(1.0, 100.0);
        assert_eq!(costs.weights().unwrap().row_in_pages, Some(2.0 / 100.0));
        for _ in 0..4 {
            costs.observe(Work::PageRead, 1.0, 400.0);
        }
        assert_eq!(costs.weights().unwrap().row_in_pages, Some(2.0 / 400.0));
    }

    /// Do: time a scratch page read beside a new database file. Expect: a time, and the
    /// directory left as it was (the scratch page removed).
    #[cfg(feature = "fs")]
    #[test]
    fn t_qt_5_5_a_scratch_page_read_is_timed_beside_the_database() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("anchor.db");
        let io: crate::sync::Arc<dyn crate::io::IO> =
            crate::sync::Arc::new(crate::PlatformIO::new().unwrap());
        let (reads, nanos) = time_page_read(path.to_str().unwrap(), &io, 4096, 8).unwrap();
        assert!((1.0..=8.0).contains(&reads), "{reads}");
        assert!(nanos.is_finite() && nanos > 0.0, "{nanos}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    /// Do: time a scratch page read for a database in memory. Expect: a time, read through its
    /// own IO.
    #[test]
    fn t_qt_5_5_a_scratch_page_read_is_timed_in_memory() {
        let io: crate::sync::Arc<dyn crate::io::IO> = crate::sync::Arc::new(crate::MemoryIO::new());
        let (reads, nanos) = time_page_read(":memory:", &io, 4096, 8).unwrap();
        assert!((1.0..=8.0).contains(&reads), "{reads}");
        assert!(nanos.is_finite() && nanos > 0.0, "{nanos}");
    }

    /// An IO over memory that counts the files it opens and names `backend` as its way of
    /// reading (`IO::page_read_backend`), for AUDIT §7.1's page-read cache.
    struct Counting {
        inner: crate::MemoryIO,
        opened: std::sync::atomic::AtomicUsize,
        backend: Option<&'static str>,
    }

    impl crate::io::clock::Clock for Counting {
        fn current_time_monotonic(&self) -> crate::io::clock::MonotonicInstant {
            self.inner.current_time_monotonic()
        }
        fn current_time_wall_clock(&self) -> crate::io::clock::WallClockInstant {
            self.inner.current_time_wall_clock()
        }
    }

    impl crate::io::IO for Counting {
        fn open_file(
            &self,
            path: &str,
            flags: crate::io::OpenFlags,
            direct: bool,
        ) -> crate::Result<crate::sync::Arc<dyn crate::io::File>> {
            self.opened.fetch_add(1, Ordering::SeqCst);
            self.inner.open_file(path, flags, direct)
        }
        fn remove_file(&self, path: &str) -> crate::Result<()> {
            self.inner.remove_file(path)
        }
        fn is_memory(&self) -> bool {
            true
        }
        fn page_read_backend(&self) -> Option<&'static str> {
            self.backend
        }
    }

    fn counting(backend: Option<&'static str>) -> crate::sync::Arc<Counting> {
        crate::sync::Arc::new(Counting {
            inner: crate::MemoryIO::new(),
            opened: std::sync::atomic::AtomicUsize::new(0),
            backend,
        })
    }

    /// Do: take the open's page read twice in one process, through two IOs of one backend on one
    /// place. Expect (AUDIT §7.1): the first writes and times a scratch page, the second writes
    /// none and starts from the first's time.
    #[test]
    fn audit_7_1_a_second_open_on_a_device_reuses_its_page_read() {
        let first = counting(Some("audit-7-1-reuse"));
        let second = counting(Some("audit-7-1-reuse"));
        let first_io: crate::sync::Arc<dyn crate::io::IO> = first.clone();
        let second_io: crate::sync::Arc<dyn crate::io::IO> = second.clone();
        let timed = page_read_anchor(":memory:", &first_io, 4096, 8, true).unwrap();
        assert_eq!(first.opened.load(Ordering::SeqCst), 1);
        let again = page_read_anchor(":memory:", &second_io, 4096, 8, true).unwrap();
        assert_eq!(second.opened.load(Ordering::SeqCst), 0);
        assert_eq!(again, timed);
    }

    /// Do: take the open's page read twice through an IO that names no backend. Expect: each
    /// open times its own scratch page, as every open did before AUDIT §7.1.
    #[test]
    fn audit_7_1_an_io_naming_no_backend_times_each_open() {
        let io = counting(None);
        let dyn_io: crate::sync::Arc<dyn crate::io::IO> = io.clone();
        page_read_anchor(":memory:", &dyn_io, 4096, 8, true).unwrap();
        page_read_anchor(":memory:", &dyn_io, 4096, 8, true).unwrap();
        assert_eq!(io.opened.load(Ordering::SeqCst), 2);
    }

    /// Do: take the open's page read for a read-only open after the backend's place was timed.
    /// Expect: none and no scratch written, as before AUDIT §7.1.
    #[test]
    fn audit_7_1_a_read_only_open_takes_no_page_read() {
        let timer_io: crate::sync::Arc<dyn crate::io::IO> = counting(Some("audit-7-1-read-only"));
        page_read_anchor(":memory:", &timer_io, 4096, 8, true).unwrap();
        let reader = counting(Some("audit-7-1-read-only"));
        let reader_io: crate::sync::Arc<dyn crate::io::IO> = reader.clone();
        assert_eq!(page_read_anchor(":memory:", &reader_io, 4096, 8, false), None);
        assert_eq!(reader.opened.load(Ordering::SeqCst), 0);
    }

    /// Do: open a new, empty database file and ask for its weights before any query. Expect: the
    /// row already weighed in page reads (the open's anchor), not left to a fixed number.
    #[cfg(feature = "fs")]
    #[test]
    fn t_qt_5_5_a_new_database_has_a_page_read_before_its_first_query() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("new.db");
        let io: crate::sync::Arc<dyn crate::io::IO> =
            crate::sync::Arc::new(crate::PlatformIO::new().unwrap());
        let db = crate::Database::open_file(
            io,
            path.to_str().unwrap(),
            crate::sync::Arc::new(crate::SqliteDialect),
        )
        .unwrap();
        let row = db.learned_costs.weights().unwrap().row_in_pages.unwrap();
        assert!(row.is_finite() && row > 0.0, "{row}");
    }

    /// Do: ask whether medians are settled: four equal samples against no resolution, samples
    /// spread over 0 to 100 against 1 and against 50, and a single sample. Expect: settled, not,
    /// settled, and not (qs5-10: calibration samples each kind until its median is known to the
    /// timer's resolution, read from the samples' own spread, not a picked count).
    #[test]
    fn t_qt_5_5_a_median_settles_once_its_spread_is_within_the_resolution() {
        assert!(median_settled(&mut [10.0; 4], 0.0));
        let mut spread: Vec<f64> = (0..=100).map(f64::from).collect();
        assert!(!median_settled(&mut spread, 1.0));
        assert!(median_settled(&mut spread, 50.0));
        assert!(!median_settled(&mut [10.0], 10.0));
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&mut [4.0, 1.0, 3.0, 2.0]), Some(2.5));
        assert_eq!(median(&mut []), None);
    }

    /// Do: calibrate with a clock that moves 1,000 ns a reading, and with one that also jumps a
    /// millisecond every eleventh reading, as a preempted thread's does. Expect: the same weights
    /// from both (qs5-10: each kind's rate is the median of samples taken in rounds, so the host's
    /// load doesn't move the planner's weights).
    #[test]
    fn t_qt_5_5_calibration_weights_hold_through_preempted_samples() {
        let steady = {
            let mut t = 0.0;
            calibrate(&mut || {
                t += 1_000.0;
                t
            })
        };
        let preempted = {
            let (mut t, mut reads) = (0.0, 0u64);
            calibrate(&mut || {
                reads += 1;
                t += if reads.is_multiple_of(11) {
                    1_001_000.0
                } else {
                    1_000.0
                };
                t
            })
        };
        let steady = steady.weights(None).unwrap();
        let preempted = preempted.weights(None).unwrap();
        for (a, b) in [
            (steady.seek, preempted.seek),
            (steady.hash_build, preempted.hash_build),
            (steady.hash_probe, preempted.hash_probe),
            (steady.sort, preempted.sort),
            (steady.bloom_probe, preempted.bloom_probe),
            (steady.where_step, preempted.where_step),
        ] {
            assert!((a - b).abs() <= 1e-12 * a.abs(), "{steady:?} {preempted:?}");
        }
    }

    /// Do: calibrate with a clock that never moves. Expect: no weights, so the planner is told it
    /// has none rather than given zeros (T-QT.5.5).
    #[test]
    fn t_qt_5_5_a_stopped_clock_gives_no_weights() {
        let rates = calibrate(&mut || 0.0);
        assert_eq!(rates.weights(None), None);
    }
}

/// PL.8 (T-PL.8.6, lane pl8-16): the A/B of the weight rules (`WeightRule`), each forced into its
/// own failure on one seeded series of times, the same for both. A rate's work costs 100 ns a unit,
/// read by reads that scatter 30% (the logarithm's spread) and batches of which a quarter are
/// slowed together by up to 30% (a burst of load over a whole batch). It reports:
/// - plan flips: how often the weight crosses a boundary 5% above the true cost (two plans whose
///   costs cross there), read after every observation, as a statement prepared then would;
/// - batches to follow: after the cost quadruples at batch `k`, how many batches at the new cost
///   until the weight stays within half again of it; early `k` is batch means' failure.
///
/// The run of record: `cargo test -p amphora-engine --lib learned_costs::ab_tests -- --nocapture`
/// (more seeds and batches are the constants below; the coordinator's).
#[cfg(test)]
mod ab_tests {
    use super::*;

    /// The tests' calibrated batch: 100 units (`tests::fixed_rates`).
    const EPOCH: f64 = 100.0;
    const TRUE_NANOS: f64 = 100.0;

    fn unit(seed: u64, i: u64) -> f64 {
        (mix(seed.wrapping_mul(1 << 32).wrapping_add(i)) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// A read's own scatter: `exp(0.3 z)`, `z` near normal (twelve uniforms less six).
    fn scatter(seed: u64, i: u64) -> f64 {
        let z: f64 = (0..12).map(|k| unit(seed, 12 * i + k)).sum::<f64>() - 6.0;
        (0.3 * z).exp()
    }

    /// Batch `b`'s burst: a quarter of the batches slowed together, by up to 30%.
    fn burst(seed: u64, b: u64) -> f64 {
        let s = seed ^ 0xB0B;
        if unit(s, 2 * b) < 0.25 {
            1.0 + 0.3 * unit(s, 2 * b + 1)
        } else {
            1.0
        }
    }

    fn rate(rule: WeightRule, nanos: f64) -> Rate {
        let mut rate = Rate::measured(EPOCH, EPOCH * nanos);
        rate.switch(rule);
        rate
    }

    /// Times the weight crossed the boundary over `batches` batches.
    fn flips(rule: WeightRule, seed: u64, batches: u64) -> u64 {
        let boundary = TRUE_NANOS * 1.05;
        let mut rate = rate(rule, TRUE_NANOS);
        let above = |rate: &Rate| rate.nanos_per_unit().is_some_and(|n| n > boundary);
        let (mut side, mut flips) = (above(&rate), 0);
        for i in 0..batches * EPOCH as u64 {
            rate.observe(
                1.0,
                TRUE_NANOS * scatter(seed, i) * burst(seed, i / EPOCH as u64),
            );
            if above(&rate) != side {
                side = !side;
                flips += 1;
            }
        }
        flips
    }

    /// Batches at the new cost until the weight stays within half again of it, after the cost
    /// quadruples at batch `step`; None if it hadn't by `batches`.
    fn batches_to_follow(rule: WeightRule, seed: u64, step: u64, batches: u64) -> Option<u64> {
        let after = TRUE_NANOS * 4.0;
        let mut rate = rate(rule, TRUE_NANOS);
        let mut last_off = None;
        for b in 0..batches {
            let cost = if b < step { TRUE_NANOS } else { after };
            for k in 0..EPOCH as u64 {
                let i = b * EPOCH as u64 + k;
                rate.observe(1.0, cost * scatter(seed, i) * burst(seed, b));
            }
            let off = rate
                .nanos_per_unit()
                .is_none_or(|n| (n / after).ln().abs() > 1.5_f64.ln());
            if b >= step && off {
                last_off = Some(b);
            }
        }
        match last_off {
            None => Some(1),
            Some(b) if b + 1 == batches => None,
            Some(b) => Some(b - step + 2),
        }
    }

    /// Do: both rules over the same eight seeded runs of 200 batches, then a quadrupled cost at
    /// batch 1, 2, 4, 16 and 64 over four. Expect: the control flips the plan at every few reads
    /// (over ten times the candidate's flips); the candidate follows a change late in the run in
    /// one batch as the control does, but one right after start only after several (T-PL.8.8:
    /// each rule's failure, measured; the numbers are the report).
    #[test]
    fn t_pl_8_6_ab_plan_flips_and_batches_to_follow_a_change() {
        let rules = [WeightRule::Blend, WeightRule::BatchMeans];
        let [blend, batch] = rules.map(|rule| (1..=8).map(|s| flips(rule, s, 200)).sum::<u64>());
        println!("plan flips, 8 runs of 200 batches: blend {blend}, batch means {batch}");
        assert!(batch * 10 < blend, "blend {blend}, batch means {batch}");
        for step in [1, 2, 4, 16, 64] {
            let [blend, batch] = rules.map(|rule| {
                (1..=4)
                    .map(|s| batches_to_follow(rule, s, step, 200))
                    .collect::<Vec<_>>()
            });
            println!(
                "batches to follow a change at batch {step}: blend {blend:?}, batch means {batch:?}"
            );
            assert!(blend.iter().all(|b| *b == Some(1)), "{step}: {blend:?}");
            assert!(batch.iter().all(Option::is_some), "{step}: {batch:?}");
            if step == 1 {
                assert!(batch.iter().all(|b| b.is_some_and(|b| b > 1)), "{batch:?}");
            }
            if step == 64 {
                assert!(batch.iter().all(|b| *b == Some(1)), "{batch:?}");
            }
        }
    }

    /// Do: a rate's batches 10% either way of their mean, then the same under the blend. Expect: no
    /// margin before a spread is measured, then `exp(MOVE_Z * s) - 1` for the batches' spread `s`,
    /// and none under the blend, which measures no spread (T-PL.8.9, lane pl8-16).
    #[test]
    fn t_pl_8_cost_margin_is_the_weights_error() {
        let costs = LearnedCosts::from_rates(&Rates {
            row: Rate::measured(EPOCH, EPOCH),
            seek: Rate::measured(EPOCH, EPOCH * TRUE_NANOS),
            ..Rates::default()
        });
        assert_eq!(costs.cost_margin(), 0.0);
        for i in 0..4 {
            for _ in 0..100 {
                costs.observe_sampled(Work::Seek, if i % 2 == 0 { 90.0 } else { 110.0 });
            }
        }
        // The first batch replaced calibration's rate; then three differences of ln(110/90).
        let s = (110.0_f64 / 90.0).ln();
        let want = (MOVE_Z * s).exp_m1();
        let got = costs.cost_margin();
        assert!((got - want).abs() < 1e-12, "{got} vs {want}");
        costs.set_rule(WeightRule::Blend);
        assert_eq!(costs.cost_margin(), 0.0);
    }

    /// Do: a rate under each rule, switched to the other after a batch. Expect: the weight as it
    /// stood carries over, and each rule then moves as its own (T-PL.8.6, lane pl8-16).
    #[test]
    fn t_pl_8_6_a_switched_rule_starts_from_the_rate_as_it_stands() {
        let mut blend = rate(WeightRule::Blend, TRUE_NANOS);
        blend.observe(EPOCH, EPOCH * 200.0);
        assert_eq!(blend.nanos_per_unit(), Some(200.0));
        blend.observe(1.0, 1_000.0);
        assert!(blend.nanos_per_unit().is_some_and(|n| n > 200.0));
        let mut batch = rate(WeightRule::BatchMeans, TRUE_NANOS);
        batch.observe(1.0, 1_000.0);
        assert_eq!(batch.nanos_per_unit(), Some(TRUE_NANOS));
        batch.switch(WeightRule::Blend);
        assert_eq!(batch.nanos_per_unit(), Some(TRUE_NANOS));
        batch.observe(1.0, 1_000.0);
        assert!(batch.nanos_per_unit().is_some_and(|n| n > TRUE_NANOS));
        blend.switch(WeightRule::BatchMeans);
        let stood = blend.nanos_per_unit();
        blend.observe(1.0, 1.0);
        assert_eq!(blend.nanos_per_unit(), stood);
    }
}
