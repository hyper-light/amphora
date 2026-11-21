//! A corpus file run on one engine, record by record, as SQLite's runner runs it (bench/corpus/slt.py,
//! `run_file`): the records SQLite runs (its `skipif`/`onlyif`), each statement to its end, each
//! query's answer rendered, sorted and compared with SQLite's. Before the first query after any
//! statement the engine settles into its steady state (its log copied into the file and emptied),
//! so what a query reads, and what it costs, is the database's, not a leftover log's (G2 step 4).

use std::time::{Duration, Instant};

use crate::answer::{Render, Value, answer, compare, expected_values};
use crate::complete::split;
use crate::slt::{Item, Kind, Record, parse};

/// The engine whose conditions the corpus's records follow: every answer in it is SQLite's.
pub const AS_ENGINE: &str = "sqlite";

/// Why a statement didn't run to its end.
#[derive(Clone, Debug, PartialEq)]
pub enum Stop {
    /// The engine's error.
    Refused(String),
    /// Its bound came first: the engine stopped it (its interrupt, or its process ended).
    PastBudget,
}

/// One engine, a fresh database per file (the seam every adapter fills: crate::engines).
pub trait Engine {
    /// Runs one statement to its end; its rows, at most `rows` of them kept (past that, the rest
    /// are counted and dropped, and the answer already disagrees), or why it stopped short. When
    /// `until` comes first the engine stops the statement itself (G2 step 7: one record can't hold
    /// a run past its budget) and answers [`Stop::PastBudget`].
    fn run(
        &mut self,
        sql: &str,
        rows: usize,
        until: Option<Instant>,
    ) -> Result<Vec<Vec<Value>>, Stop>;
    /// Copies the log into the database and empties it; an error says the steady state wasn't
    /// reached.
    fn settle(&mut self) -> Result<(), String>;
    /// Whether stopping a statement ended the engine's database (a shell's process stopped): the
    /// file's later records aren't run (Unreached).
    fn ended(&self) -> bool {
        false
    }
}

/// What became of one record.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Agree,
    /// A query's answer differs: the file's and the engine's.
    Answer {
        want: String,
        got: String,
    },
    /// The engine refused what SQLite runs.
    Refused(String),
    /// The engine ran what SQLite refuses.
    Accepted,
    /// The run's deadline came first: the record wasn't run.
    Unreached,
    /// The record ran past its bound and the engine stopped it.
    PastBudget,
}

/// The time a file's records may take, both from the run's own budget (never a picked number).
#[derive(Clone, Copy, Debug, Default)]
pub struct Limits {
    /// Records not yet begun at it aren't run (Unreached): the fill's end.
    pub deadline: Option<Instant>,
    /// A begun record is stopped this long after it began, or at the deadline if that's sooner: a
    /// record past the whole budget can't be in it.
    pub record: Option<Duration>,
}

impl Limits {
    fn until(&self, began: Instant) -> Option<Instant> {
        let own = self.record.and_then(|r| began.checked_add(r));
        match (own, self.deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

pub struct Ran<'a> {
    pub record: &'a Record,
    pub outcome: Outcome,
    /// Time spent running and comparing it, in nanoseconds.
    pub nanos: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub agree: u64,
    pub disagree: u64,
    pub skipped: u64,
    pub unreached: u64,
    /// Records the engine stopped at their bound.
    pub past_budget: u64,
    /// Settles that failed (the steady state not reached before a query).
    pub unsettled: u64,
}

/// Runs `text` (after `fixture`'s statements, a file of tables a source's files share) on `engine`,
/// handing each record's outcome to `each`. Records after the deadline aren't run (Unreached), and
/// a record past its bound is stopped by the engine (PastBudget).
pub fn run_file(
    engine: &mut dyn Engine,
    render: &mut dyn Render,
    text: &str,
    fixture: Option<&str>,
    limits: Limits,
    each: &mut dyn FnMut(Ran<'_>),
) -> Result<Counts, String> {
    let mut counts = Counts::default();
    if let Some(f) = fixture {
        for item in parse(f)? {
            if let Item::Record(r) = item
                && matches!(r.kind, Kind::Statement { .. })
            {
                for part in split(&r.sql) {
                    engine
                        .run(&part, 0, None)
                        .map_err(|e| format!("the fixture's line {}: {e:?}", r.line))?;
                }
            }
        }
    }
    let mut threshold = 0usize;
    let mut dirty = true;
    for item in parse(text)? {
        let record = match item {
            Item::HashThreshold(n) => {
                threshold = n;
                continue;
            }
            Item::Halt(conditions) => {
                if conditions.iter().all(|(only, e)| {
                    if *only {
                        e == AS_ENGINE
                    } else {
                        e != AS_ENGINE
                    }
                }) {
                    break;
                }
                continue;
            }
            Item::Record(r) => r,
        };
        if !record.runs_on(AS_ENGINE) {
            counts.skipped = counts.skipped.saturating_add(1);
            continue;
        }
        if engine.ended() || limits.deadline.is_some_and(|d| Instant::now() >= d) {
            counts.unreached = counts.unreached.saturating_add(1);
            each(Ran {
                record: &record,
                outcome: Outcome::Unreached,
                nanos: 0,
            });
            continue;
        }
        let began = Instant::now();
        let until = limits.until(began);
        let outcome = match &record.kind {
            Kind::Statement { ok } => {
                dirty = true;
                let ran = split(&record.sql)
                    .iter()
                    .try_for_each(|part| engine.run(part, 0, until).map(drop));
                match (ran, ok) {
                    (Err(Stop::PastBudget), _) => Outcome::PastBudget,
                    (Ok(()), true) | (Err(Stop::Refused(_)), false) => Outcome::Agree,
                    (Ok(()), false) => Outcome::Accepted,
                    (Err(Stop::Refused(e)), true) => Outcome::Refused(e),
                }
            }
            Kind::Query {
                types,
                sort,
                expected,
            } => {
                if dirty {
                    if engine.settle().is_err() {
                        counts.unsettled = counts.unsettled.saturating_add(1);
                    }
                    dirty = !record.reads_only();
                }
                // One row past the right answer's values already disagrees.
                let width = types.len().max(1);
                let keep = expected.as_ref().map_or(usize::MAX, |e| {
                    expected_values(e).div_ceil(width).saturating_add(1)
                });
                match engine.run(&record.sql, keep, until) {
                    Err(Stop::PastBudget) => Outcome::PastBudget,
                    Err(Stop::Refused(e)) => Outcome::Refused(e),
                    Ok(rows) => {
                        let got = answer(&rows, types, *sort, render)?;
                        match expected {
                            None => Outcome::Agree,
                            Some(want) => match compare(want, &got, threshold) {
                                Ok(()) => Outcome::Agree,
                                Err((want, got)) => Outcome::Answer { want, got },
                            },
                        }
                    }
                }
            }
        };
        let nanos = u64::try_from(began.elapsed().as_nanos()).unwrap_or(u64::MAX);
        if outcome == Outcome::Agree {
            counts.agree = counts.agree.saturating_add(1);
        } else if outcome == Outcome::PastBudget {
            counts.past_budget = counts.past_budget.saturating_add(1);
        } else {
            counts.disagree = counts.disagree.saturating_add(1);
        }
        each(Ran {
            record: &record,
            outcome,
            nanos,
        });
    }
    Ok(counts)
}
