//! A corpus file run on one engine, record by record, as SQLite's runner runs it (bench/corpus/slt.py,
//! `run_file`): the records SQLite runs (its `skipif`/`onlyif`), each statement to its end, each
//! query's answer rendered, sorted and compared with SQLite's. Before the first query after any
//! statement the engine settles into its steady state (its log copied into the file and emptied),
//! so what a query reads, and what it costs, is the database's, not a leftover log's (G2 step 4).

use std::time::Instant;

use crate::answer::{Render, Value, answer, compare, expected_values};
use crate::complete::split;
use crate::slt::{Item, Kind, Record, parse};

/// The engine whose conditions the corpus's records follow: every answer in it is SQLite's.
pub const AS_ENGINE: &str = "sqlite";

/// One engine, a fresh database per file.
pub trait Engine {
    /// Runs one statement to its end; its rows, at most `rows` of them kept (past that, the rest
    /// are counted and dropped, and the answer already disagrees), or the engine's error.
    fn run(&mut self, sql: &str, rows: usize) -> Result<Vec<Vec<Value>>, String>;
    /// Copies the log into the database and empties it; an error says the steady state wasn't
    /// reached.
    fn settle(&mut self) -> Result<(), String>;
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
    /// Settles that failed (the steady state not reached before a query).
    pub unsettled: u64,
}

/// Runs `text` (after `fixture`'s statements, a file of tables a source's files share) on `engine`,
/// handing each record's outcome to `each`. Records after `deadline` aren't run (Unreached).
pub fn run_file(
    engine: &mut dyn Engine,
    render: &mut dyn Render,
    text: &str,
    fixture: Option<&str>,
    deadline: Option<Instant>,
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
                        .run(&part, 0)
                        .map_err(|e| format!("the fixture's line {}: {e}", r.line))?;
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
        if deadline.is_some_and(|d| Instant::now() >= d) {
            counts.unreached = counts.unreached.saturating_add(1);
            each(Ran {
                record: &record,
                outcome: Outcome::Unreached,
                nanos: 0,
            });
            continue;
        }
        let began = Instant::now();
        let outcome = match &record.kind {
            Kind::Statement { ok } => {
                dirty = true;
                let ran = split(&record.sql)
                    .iter()
                    .try_for_each(|part| engine.run(part, 0).map(drop));
                match (ran, ok) {
                    (Ok(()), true) | (Err(_), false) => Outcome::Agree,
                    (Ok(()), false) => Outcome::Accepted,
                    (Err(e), true) => Outcome::Refused(e),
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
                match engine.run(&record.sql, keep) {
                    Err(e) => Outcome::Refused(e),
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
