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
    upkeep: Upkeep,
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
    /// maps, cracking), in the scans' unit.
    pub fn with_upkeep(
        zones: ZoneMap,
        cracker: CrackerColumn,
        upkeep: Upkeep,
        build_cost: [Option<u64>; 2],
    ) -> Self {
        Self {
            zones: Some(zones),
            cracker: Some(cracker),
            zone_starts: Vec::new(),
            upkeep,
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
        if since <= build {
            return;
        }
        match method {
            SkipMethod::ZoneMaps => {
                if let Some(zones) = self.zones.take() {
                    self.zone_starts = zones.starts();
                }
            }
            SkipMethod::Cracking => self.cracker = None,
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
        (trial && !self.keeps(method)).then_some(method)
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
            while self
                .zone_starts
                .get(next_start)
                .is_some_and(|start| *start <= rowid)
            {
                builder.end_run();
                next_start += 1;
            }
            builder.row(rowid, value)?;
        }
        self.zones = Some(builder.finish());
        self.zone_starts = Vec::new();
        self.rebuilt(SkipMethod::ZoneMaps, now().saturating_sub(started));
        Ok(())
    }

    /// The cracker the caller built again from the table, and what that cost.
    pub fn install_cracker(&mut self, cracker: CrackerColumn, cost: u64) {
        self.cracker = Some(cracker);
        self.rebuilt(SkipMethod::Cracking, cost);
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
        let zone = self.stats.zone_maps.last_cost;
        let crack = self.stats.cracking.last_cost;
        let (Some(zone), Some(crack)) = (zone, crack) else {
            return match zone {
                None => (SkipMethod::ZoneMaps, true),
                Some(_) => (SkipMethod::Cracking, true),
            };
        };
        let leader = if crack < zone {
            SkipMethod::Cracking
        } else {
            SkipMethod::ZoneMaps
        };
        let other_cost = if leader == SkipMethod::ZoneMaps {
            crack
        } else {
            zone
        };
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
                Candidates::Ranges(zones.candidate_ranges(lo, hi))
            }
            (SkipMethod::Cracking, _, Some(cracker)) => {
                Candidates::Rowids(cracker.candidate_rowids(lo, hi)?)
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
        let (Some(zone), Some(crack)) = (
            self.stats.zone_maps.last_cost,
            self.stats.cracking.last_cost,
        ) else {
            return;
        };
        let leader = if crack < zone {
            SkipMethod::Cracking
        } else {
            SkipMethod::ZoneMaps
        };
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
                        zone_maps_cost = zone,
                        cracking_cost = crack,
                        "skipping method changed: the latest measured scan was cheaper"
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
