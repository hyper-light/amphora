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

/// Both summaries of one column and the record that picks between them.
#[derive(Debug)]
pub struct Chooser {
    zones: ZoneMap,
    cracker: CrackerColumn,
    stats: ChooserStats,
    /// The leader as last decided (`None` until both are measured).
    leader: Option<SkipMethod>,
    /// Cost the leader's scans have spent since the other method was last measured.
    spent: u64,
    /// Measures of the other method in a row that left the leader leading.
    confirmed: u32,
}

impl Chooser {
    pub fn new(zones: ZoneMap, cracker: CrackerColumn) -> Self {
        Self {
            zones,
            cracker,
            stats: ChooserStats::default(),
            leader: None,
            spent: 0,
            confirmed: 0,
        }
    }

    pub fn stats(&self) -> ChooserStats {
        self.stats
    }

    pub fn zone_map(&self) -> &ZoneMap {
        &self.zones
    }

    pub fn cracker(&self) -> &CrackerColumn {
        &self.cracker
    }

    /// The write hook: both summaries take every write.
    pub fn on_write(&mut self, write: RowWrite) -> Result<(), SkipError> {
        self.zones.on_write(write)?;
        self.cracker.on_write(write)
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
        let (method, trial) = self.next_method();
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
        let candidates = match method {
            SkipMethod::ZoneMaps => Candidates::Ranges(self.zones.candidate_ranges(lo, hi)),
            SkipMethod::Cracking => Candidates::Rowids(self.cracker.candidate_rowids(lo, hi)?),
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
