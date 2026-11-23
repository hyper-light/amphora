//! Amphora (lane skip-3, T-SKU.2; executor.md decision 11): zone maps or cracking for a column,
//! whichever measures cheaper on the filters it is actually given.
//!
//! A [`Chooser`] keeps both summaries of one column, each fed every write, and answers each filter
//! with one of them. The caller times each scan that used a summary (from asking for its candidates
//! to the scan's end) and hands the cost back with [`Chooser::record`]; the core reads no clock.
//!
//! The rule, with no picked weight or period:
//!
//! - Each method is tried once before either is preferred (zone maps first: they cost a write
//!   nothing beyond a zone's counts, a cracker moves entries).
//! - The leader is the method whose latest measured scan was cheaper. Its own scans keep measuring
//!   it, so when its cost rises past the other's last measure (zone maps after writes scatter the
//!   column; a cracker after write churn or for wide ranges), it loses the lead at once.
//! - The other method is measured again once the leader has spent, since that method was last
//!   measured, as much as one of its scans cost, times 2 to the number of measures in a row that
//!   confirmed the leader: the doubling trick (Auer, Cesa-Bianchi, Freund and Schapire, SIAM J.
//!   Comput. 2002; ski rental's break-even rule, Karlin et al. 1988, for the first trial). A
//!   steady leader is checked geometrically less often; a cost that changes is caught at the
//!   next trial, and the count of confirmations starts again from nothing.
//!
//! The 2 is the doubling trick's base (its bound is stated for it), not a tuned number.
//!
//! Lane skip-5 (T-SKU.5): both summaries take every write, so the method that isn't leading
//! keeps costing its write upkeep. Behind the [`Upkeep`] switch (today's [`Upkeep::KeepBoth`] is
//! the control), [`Upkeep::Capped`] drops it once the write upkeep it cost since it last answered
//! a filter passes its measured cost to build again: ski rental's break-even rule (Karlin,
//! Manasse, Rudolph and Sleator, Algorithmica 1988: rent until the rent paid equals the price,
//! then buy, within twice the best in hindsight). The doubling schedule still measures it again:
//! its trial's budget adds its rebuild cost, and when the trial is due the caller builds it again
//! ([`Chooser::rebuild_due`]): zone maps from the cracker's own copy of the column (in the
//! dropped map's zones), the cracker from the table. Write upkeep and build costs come from the
//! caller in the scans' unit; the core reads no clock.
//!
//! Lane skip-7 (T-SKU.7): what the leader is chosen by, behind the [`Lead`] switch. Kept both,
//! every write is applied to both summaries whichever leads, so their upkeep is paid either way
//! and only the scan differs: [`Lead::Scan`] (T-SKU.2's rule, the control). Capped, the method not
//! leading is the one whose upkeep is shed, so the choice decides which upkeep goes on being paid:
//! [`Lead::Total`] compares each method's latest scan plus the write upkeep it cost over the
//! latest stretch between filters (skip-5's decision: total cost). A method's upkeep is measured
//! over every stretch it is kept, so a cracker's moves under write churn count against it before
//! its next scan does. A leader's upkeep also counts toward what it has spent before the other is
//! measured again, and the other's trial budget is its own total. A cracker built again keeps the
//! cut points it had when it was dropped (T-SKU.8): its filters find their pieces cut as before.

use super::{Candidates, CrackerColumn, RowWrite, SkipError, ZoneMap};

/// A summary a [`Chooser`] may answer a filter with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipMethod {
    ZoneMaps,
    Cracking,
}

impl SkipMethod {
    fn other(self) -> Self {
        match self {
            SkipMethod::ZoneMaps => SkipMethod::Cracking,
            SkipMethod::Cracking => SkipMethod::ZoneMaps,
        }
    }
}

/// One method's record in a [`Chooser`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MethodStats {
    /// Filters answered with it as the leader.
    pub chosen: u64,
    /// Filters answered with it to measure it (it wasn't the leader, or wasn't measured yet).
    pub trials: u64,
    /// Costs handed back for it.
    pub measured: u64,
    /// Its latest measured cost (the caller's unit: nanoseconds from the executor).
    pub last_cost: Option<u64>,
    /// Every measured cost added up.
    pub total_cost: u64,
    /// Write upkeep handed back for it, added up (lane skip-5, T-SKU.5).
    pub upkeep: u64,
    /// The write upkeep it cost over the latest stretch between two filters while it was kept
    /// (lane skip-7, T-SKU.7); `None` until a filter closed a stretch.
    pub last_upkeep: Option<u64>,
    /// Times it was dropped for its upkeep, and built again for a trial.
    pub dropped: u64,
    pub rebuilt: u64,
}

/// What a [`Chooser`] does with the method that isn't leading (lane skip-5, T-SKU.5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Upkeep {
    /// Keep both, every write applied to each (the control: T-SKU.2's rule).
    #[default]
    KeepBoth,
    /// Drop the method not leading once its write upkeep passes its cost to build again.
    Capped,
}

/// What a [`Chooser`]'s leader is chosen by (lane skip-7, T-SKU.7).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Lead {
    /// The latest measured scan alone (T-SKU.2's rule; the control, and [`Upkeep::KeepBoth`]'s:
    /// both upkeeps are paid whichever leads).
    #[default]
    Scan,
    /// The latest measured scan plus the write upkeep over the latest stretch between filters
    /// ([`Upkeep::Capped`]'s: the method not leading has its upkeep shed).
    Total,
}

impl Lead {
    /// The rule an upkeep rule goes with: total cost where the choice decides whose upkeep is
    /// paid, the scan alone where both are paid anyway.
    pub fn for_upkeep(upkeep: Upkeep) -> Self {
        match upkeep {
            Upkeep::KeepBoth => Lead::Scan,
            Upkeep::Capped => Lead::Total,
        }
    }
}

/// What a [`Chooser`] has done: per method, and how often the leader changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChooserStats {
    pub zone_maps: MethodStats,
    pub cracking: MethodStats,
    /// Times the leader changed after both were measured.
    pub switches: u64,
}

impl ChooserStats {
    fn of(&mut self, method: SkipMethod) -> &mut MethodStats {
        match method {
            SkipMethod::ZoneMaps => &mut self.zone_maps,
            SkipMethod::Cracking => &mut self.cracking,
        }
    }

    fn get(&self, method: SkipMethod) -> &MethodStats {
        match method {
            SkipMethod::ZoneMaps => &self.zone_maps,
            SkipMethod::Cracking => &self.cracking,
        }
    }
}

fn slot(method: SkipMethod) -> usize {
    match method {
        SkipMethod::ZoneMaps => 0,
        SkipMethod::Cracking => 1,
    }
}

/// Both summaries of one column and the record that picks between them.
#[derive(Debug)]
pub struct Chooser {
    /// `None`: dropped for its upkeep ([`Upkeep::Capped`]), until built again.
    zones: Option<ZoneMap>,
    cracker: Option<CrackerColumn>,
    /// The dropped zone map's zone starts: it is built again in the same zones.
    zone_starts: Vec<i64>,
    /// The dropped cracker's cut points: it is cut at them again when built again (T-SKU.8).
    crack_bounds: Vec<i64>,
    upkeep: Upkeep,
    lead: Lead,
    /// Write upkeep each method has cost since the last filter (T-SKU.7).
    upkeep_window: [u64; 2],
    /// Whether each method was kept since the last filter: a stretch it was dropped for part of
    /// is no measure of its upkeep (T-SKU.7).
    kept_stretch: [bool; 2],
    /// Each method's latest measured cost to build (zone maps, cracking); `None`: never measured,
    /// so never dropped.
    build_cost: [Option<u64>; 2],
    /// Write upkeep each method has cost since it last answered a filter.
    upkeep_since: [u64; 2],
    stats: ChooserStats,
    /// The leader as last decided (`None` until both are measured).
    leader: Option<SkipMethod>,
    /// Cost the leader's scans have spent since the other method was last measured.
    spent: u64,
    /// Measures of the other method in a row that left the leader leading.
    confirmed: u32,
}

impl Chooser {
    /// Both kept, every write to each ([`Upkeep::KeepBoth`]).
    pub fn new(zones: ZoneMap, cracker: CrackerColumn) -> Self {
        Self::with_upkeep(zones, cracker, Upkeep::KeepBoth, [None, None])
    }

    /// Lane skip-5 (T-SKU.5): with an upkeep rule and each method's measured build cost (zone
    /// maps, cracking), in the scans' unit; the leader chosen as the upkeep rule goes with
    /// ([`Lead::for_upkeep`]).
    pub fn with_upkeep(
        zones: ZoneMap,
        cracker: CrackerColumn,
        upkeep: Upkeep,
        build_cost: [Option<u64>; 2],
    ) -> Self {
        Self::with_rules(zones, cracker, upkeep, Lead::for_upkeep(upkeep), build_cost)
    }

    /// Lane skip-7 (T-SKU.7): with an upkeep rule and a lead rule of the caller's (the A/B's
    /// candidates), and each method's measured build cost.
    pub fn with_rules(
        zones: ZoneMap,
        cracker: CrackerColumn,
        upkeep: Upkeep,
        lead: Lead,
        build_cost: [Option<u64>; 2],
    ) -> Self {
        Self {
            zones: Some(zones),
            cracker: Some(cracker),
            zone_starts: Vec::new(),
            crack_bounds: Vec::new(),
            upkeep,
            lead,
            upkeep_window: [0, 0],
            kept_stretch: [true, true],
            build_cost,
            upkeep_since: [0, 0],
            stats: ChooserStats::default(),
            leader: None,
            spent: 0,
            confirmed: 0,
        }
    }

    pub fn stats(&self) -> ChooserStats {
        self.stats
    }

    /// The zone map; `None` while it is dropped for its upkeep.
    pub fn zone_map(&self) -> Option<&ZoneMap> {
        self.zones.as_ref()
    }

    /// The cracker; `None` while it is dropped for its upkeep.
    pub fn cracker(&self) -> Option<&CrackerColumn> {
        self.cracker.as_ref()
    }

    pub fn upkeep(&self) -> Upkeep {
        self.upkeep
    }

    pub fn lead(&self) -> Lead {
        self.lead
    }

    /// Whether `method`'s summary is kept now.
    pub fn keeps(&self, method: SkipMethod) -> bool {
        match method {
            SkipMethod::ZoneMaps => self.zones.is_some(),
            SkipMethod::Cracking => self.cracker.is_some(),
        }
    }

    /// The write hook: each kept summary takes every write (its upkeep uncounted: see
    /// [`Chooser::on_write_timed`]).
    pub fn on_write(&mut self, write: RowWrite) -> Result<(), SkipError> {
        self.on_write_timed(write, &mut || 0)
    }

    /// The write hook, each kept summary's write timed by `now` (the caller's clock, in the
    /// scans' unit) and handed to [`Chooser::record_upkeep`].
    pub fn on_write_timed(
        &mut self,
        write: RowWrite,
        now: &mut dyn FnMut() -> u64,
    ) -> Result<(), SkipError> {
        for method in [SkipMethod::ZoneMaps, SkipMethod::Cracking] {
            let started = now();
            if !self.write_method(method, write)? {
                continue;
            }
            let cost = now().saturating_sub(started);
            self.record_upkeep(method, cost);
        }
        Ok(())
    }

    /// Applies `write` to `method`'s summary; `false` when it is dropped.
    pub fn write_method(&mut self, method: SkipMethod, write: RowWrite) -> Result<bool, SkipError> {
        match method {
            SkipMethod::ZoneMaps => match self.zones.as_mut() {
                Some(zones) => zones.on_write(write).map(|()| true),
                None => Ok(false),
            },
            SkipMethod::Cracking => match self.cracker.as_mut() {
                Some(cracker) => cracker.on_write(write).map(|()| true),
                None => Ok(false),
            },
        }
    }

    /// A write cost `method`'s summary `cost` of upkeep. Under [`Upkeep::Capped`], the method not
    /// leading is dropped once its upkeep since it last answered a filter passes its build cost.
    pub fn record_upkeep(&mut self, method: SkipMethod, cost: u64) {
        let entry = self.stats.of(method);
        entry.upkeep = entry.upkeep.saturating_add(cost);
        if let Some(window) = self.upkeep_window.get_mut(slot(method)) {
            *window = window.saturating_add(cost);
        }
        let Some(since) = self.upkeep_since.get_mut(slot(method)) else {
            return;
        };
        *since = since.saturating_add(cost);
        let since = *since;
        let Some(build) = self.build_cost.get(slot(method)).copied().flatten() else {
            return;
        };
        if self.upkeep != Upkeep::Capped || self.leader.is_none_or(|leader| leader == method) {
            return;
        }
        // Lane skip-7: the last kind kept is never dropped (the leader by cost may be the one
        // dropped, until its build: see `next_method`).
        if !self.keeps(method.other()) {
            return;
        }
        if since <= build {
            return;
        }
        match method {
            SkipMethod::ZoneMaps => {
                if let Some(zones) = self.zones.take() {
                    self.zone_starts = zones.starts();
                }
            }
            SkipMethod::Cracking => {
                if let Some(cracker) = self.cracker.take() {
                    self.crack_bounds = cracker.bounds();
                }
            }
        }
        if let Some(kept) = self.kept_stretch.get_mut(slot(method)) {
            *kept = false;
        }
        let entry = self.stats.of(method);
        entry.dropped = entry.dropped.saturating_add(1);
        tracing::debug!(
            target: "amphora.skipping",
            method = ?method,
            upkeep = since,
            build_cost = build,
            "skipping method dropped: its write upkeep since it last answered passed its cost to build again"
        );
    }

    /// The method a trial is due for that is dropped: build it again before the next filter
    /// (zone maps with [`Chooser::rebuild_zones`], the cracker from the table with
    /// [`Chooser::install_cracker`]). Until then the leader answers.
    pub fn rebuild_due(&self) -> Option<SkipMethod> {
        let (method, trial) = self.next_method();
        if trial && !self.keeps(method) {
            Some(method)
        } else {
            None
        }
    }

    /// Builds the dropped zone map again from the cracker's copy of the column, in the dropped
    /// map's zones; `cost` is filled by the caller's clock around it (`now`).
    pub fn rebuild_zones(&mut self, now: &mut dyn FnMut() -> u64) -> Result<(), SkipError> {
        let started = now();
        let Some(cracker) = self.cracker.as_ref() else {
            return Ok(());
        };
        let mut rows = cracker.rows();
        rows.sort_unstable_by_key(|(rowid, _)| *rowid);
        let mut builder = super::ZoneMapBuilder::new();
        let mut next_start = 0usize;
        for (rowid, value) in rows {
            let passed = self.zone_starts.get(next_start..).map_or(0, |rest| {
                rest.iter().take_while(|start| **start <= rowid).count()
            });
            for _ in 0..passed {
                builder.end_run();
            }
            next_start = next_start
                .checked_add(passed)
                .unwrap_or(self.zone_starts.len());
            builder.row(rowid, value)?;
        }
        self.zones = Some(builder.finish());
        self.zone_starts = Vec::new();
        self.rebuilt(SkipMethod::ZoneMaps, now().saturating_sub(started));
        Ok(())
    }

    /// The cracker the caller built again from the table, and what that cost. Lane skip-7
    /// (T-SKU.8): it is cut again at the dropped cracker's cut points first, timed by `now` (the
    /// caller's clock, in the scans' unit) and added to its build cost; a refused cut leaves the
    /// method dropped.
    pub fn install_cracker(
        &mut self,
        mut cracker: CrackerColumn,
        cost: u64,
        now: &mut dyn FnMut() -> u64,
    ) -> Result<(), SkipError> {
        let started = now();
        cracker.crack_at(&self.crack_bounds)?;
        let cost = cost.saturating_add(now().saturating_sub(started));
        self.cracker = Some(cracker);
        self.crack_bounds = Vec::new();
        self.rebuilt(SkipMethod::Cracking, cost);
        Ok(())
    }

    /// `method`'s cost as the lead rule counts it: its latest scan, plus (under [`Lead::Total`])
    /// the write upkeep of its latest stretch between filters.
    fn cost_of(&self, method: SkipMethod) -> Option<u64> {
        let stats = self.stats.get(method);
        let scan = stats.last_cost?;
        Some(match self.lead {
            Lead::Scan => scan,
            Lead::Total => scan.saturating_add(stats.last_upkeep.unwrap_or(0)),
        })
    }

    /// The method the lead rule picks, once both are measured (zone maps on a tie).
    fn leading(&self) -> Option<SkipMethod> {
        let zone = self.cost_of(SkipMethod::ZoneMaps)?;
        let crack = self.cost_of(SkipMethod::Cracking)?;
        Some(if crack < zone {
            SkipMethod::Cracking
        } else {
            SkipMethod::ZoneMaps
        })
    }

    /// A filter ends the stretch of writes since the last one: the upkeep of each method kept over
    /// all of it is its latest measure (one dropped for any of it, or built again during it,
    /// keeps the measure it had while kept), and under [`Lead::Total`] the leader's counts toward
    /// what it has spent.
    fn close_stretch(&mut self) {
        for method in [SkipMethod::ZoneMaps, SkipMethod::Cracking] {
            let keeps = self.keeps(method);
            let (Some(window), Some(kept)) = (
                self.upkeep_window.get_mut(slot(method)),
                self.kept_stretch.get_mut(slot(method)),
            ) else {
                continue;
            };
            let upkeep = std::mem::take(window);
            let whole = std::mem::replace(kept, keeps);
            if !(keeps && whole) {
                continue;
            }
            self.stats.of(method).last_upkeep = Some(upkeep);
            if self.lead == Lead::Total && self.leader == Some(method) {
                self.spent = self.spent.saturating_add(upkeep);
            }
        }
    }

    fn rebuilt(&mut self, method: SkipMethod, cost: u64) {
        if let Some(build) = self.build_cost.get_mut(slot(method)) {
            *build = Some(cost);
        }
        if let Some(since) = self.upkeep_since.get_mut(slot(method)) {
            *since = 0;
        }
        let entry = self.stats.of(method);
        entry.rebuilt = entry.rebuilt.saturating_add(1);
        tracing::debug!(
            target: "amphora.skipping",
            method = ?method,
            build_cost = cost,
            "skipping method built again for its trial"
        );
    }

    /// The method that answers the next filter, and whether it is a trial (measuring a method
    /// that isn't the leader).
    pub fn next_method(&self) -> (SkipMethod, bool) {
        let Some(leader) = self.leading() else {
            return match self.stats.zone_maps.last_cost {
                None => (SkipMethod::ZoneMaps, true),
                Some(_) => (SkipMethod::Cracking, true),
            };
        };
        // Lane skip-7: the kept kind's cost passed the dropped one's last measure, so the dropped
        // one is due a build and a trial now ([`Chooser::rebuild_due`]); the kept one answers
        // until it is built.
        if !self.keeps(leader) {
            return (leader, true);
        }
        let other_cost = self.cost_of(leader.other()).unwrap_or(0);
        // Lane skip-5 (T-SKU.5): a dropped method's trial also pays its build.
        let other_cost = if self.keeps(leader.other()) {
            other_cost
        } else {
            let build = self.build_cost.get(slot(leader.other())).copied().flatten();
            other_cost.saturating_add(build.unwrap_or(0))
        };
        let budget = 1u64
            .checked_shl(self.confirmed)
            .map_or(u64::MAX, |factor| other_cost.saturating_mul(factor));
        if self.spent >= budget {
            (leader.other(), true)
        } else {
            (leader, false)
        }
    }

    /// The rows a filter `lo <= value <= hi` may need, and the method that named them: hand its
    /// cost back with [`Chooser::record`].
    pub fn candidates(&mut self, lo: i64, hi: i64) -> Result<(Candidates, SkipMethod), SkipError> {
        self.candidates_spans(&[(lo, hi)])
    }

    /// Lane skip-11 (T-SKU.14): [`Chooser::candidates`] for a filter whose rows lie in any of
    /// `spans` (each `(lo, hi)`, inclusive: an IN list asked value by value). It is one filter
    /// however many spans it has: one method answers every span, one trial or choice is counted,
    /// and the one cost handed back ([`Chooser::record`]) covers every probe and every row the
    /// scan read, so a span and a list of values are measured alike.
    pub fn candidates_spans(
        &mut self,
        spans: &[(i64, i64)],
    ) -> Result<(Candidates, SkipMethod), SkipError> {
        self.close_stretch();
        let (method, trial) = match self.next_method() {
            // A dropped method waits for its build (`rebuild_due`): the leader answers.
            (method, true) if !self.keeps(method) => (method.other(), false),
            chosen => chosen,
        };
        if let Some(since) = self.upkeep_since.get_mut(slot(method)) {
            *since = 0;
        }
        if trial && self.leader.is_some() {
            // The trial is under way: scans that start before its cost comes back (a burst of
            // connections) stay with the leader, so one trial is issued per budget, not one each.
            self.spent = 0;
        }
        let entry = self.stats.of(method);
        if trial {
            entry.trials = entry.trials.saturating_add(1);
        } else {
            entry.chosen = entry.chosen.saturating_add(1);
        }
        let candidates = match (method, self.zones.as_mut(), self.cracker.as_mut()) {
            (SkipMethod::ZoneMaps, Some(zones), _) => {
                Candidates::Ranges(zones.candidate_ranges_each(spans))
            }
            (SkipMethod::Cracking, _, Some(cracker)) => {
                Candidates::Rowids(cracker.candidate_rowids_each(spans)?)
            }
            // Neither kept can't happen (only the method not leading is dropped): every row.
            _ => Candidates::All,
        };
        Ok((candidates, method))
    }

    /// A scan answered by `method` cost `cost` (measured by the caller, in one unit throughout).
    pub fn record(&mut self, method: SkipMethod, cost: u64) {
        let entry = self.stats.of(method);
        entry.measured = entry.measured.saturating_add(1);
        entry.last_cost = Some(cost);
        entry.total_cost = entry.total_cost.saturating_add(cost);
        let Some(leader) = self.leading() else {
            return;
        };
        let (zone, crack) = (
            self.cost_of(SkipMethod::ZoneMaps).unwrap_or(0),
            self.cost_of(SkipMethod::Cracking).unwrap_or(0),
        );
        match self.leader {
            Some(previous) if previous == method && previous == leader => {
                self.spent = self.spent.saturating_add(cost);
            }
            Some(previous) if previous == leader => {
                // The other method was measured and the leader still leads.
                self.spent = 0;
                self.confirmed = self.confirmed.saturating_add(1);
            }
            previous => {
                if previous.is_some() {
                    self.stats.switches = self.stats.switches.saturating_add(1);
                    tracing::debug!(
                        target: "amphora.skipping",
                        leader = ?leader,
                        lead = ?self.lead,
                        zone_maps_cost = zone,
                        cracking_cost = crack,
                        "skipping method changed: its latest measured cost (by the lead rule) was cheaper"
                    );
                }
                self.leader = Some(leader);
                self.spent = 0;
                self.confirmed = 0;
            }
        }
    }

    /// The method leading now, if both have been measured.
    pub fn leader(&self) -> Option<SkipMethod> {
        self.leader
    }

    /// The method's stats.
    pub fn method_stats(&self, method: SkipMethod) -> MethodStats {
        *self.stats.get(method)
    }
}
