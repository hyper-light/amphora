//! Amphora: the column copies' rebuy A/B, defined once (lane colcopy-7, AC-CB.4.1, AC-CB.4.2): the
//! scenarios that force each policy's failure mode, the statements they run, and a count of the
//! copies' trace events by the reason each gives. The engine's A/B test (`columns_sql::tests::ab`)
//! and the bench's comparison against DuckDB (`amphora-bench --colcopy`) both run these, so the
//! two measure the same work.
//!
//! The sizes are each scenario's definition, not tuning: rows, tables and statements scale by one
//! factor `k`, and the room left the copies is a share of what one table's copy needs at that
//! scale (`need`, measured where it runs), never a byte count.

use crate::sync::{Arc, Mutex};
use std::collections::BTreeMap;

/// The analytic statements, on table `t`: filters, aggregates and GROUP BY over every column kind.
pub const QUERIES: [&str; 7] = [
    "SELECT count(*), sum(v), min(v), max(v) FROM t",
    "SELECT count(s), min(s), max(s) FROM t WHERE v >= 500",
    "SELECT g, count(*), sum(v) FROM t GROUP BY g",
    "SELECT s, count(*), max(id) FROM t WHERE g <> 3 GROUP BY s",
    "SELECT g, s, sum(v) FROM t WHERE s IS NOT NULL GROUP BY g, s",
    "SELECT count(*) FROM t WHERE s IS NULL",
    "SELECT min(id), max(id), sum(id) FROM t WHERE v < 100",
];

/// Table `t{i}`.
pub fn schema(i: usize) -> String {
    format!("CREATE TABLE t{i} (id INTEGER PRIMARY KEY, g INTEGER, v INTEGER, s TEXT)")
}

/// How a scenario presses the copies' room: the bytes left them are a share of what one table's
/// copy needs (the scenario's definition).
#[derive(Clone, Copy, Debug)]
pub enum Press {
    /// `copies` copies' worth of room, held steady.
    Steady { copies: f64 },
    /// Half a copy's room; each statement another use frees `piece` of a copy's need and takes it
    /// back at the next, so the pieces never add up to one copy.
    Bursty { piece: f64 },
    /// Half a copy's room until half the statements have run, then a copy's worth more, the
    /// tables growing all along.
    Grows,
    /// Half a copy's room, and every other round (one statement on each table) room for every
    /// table's copy twice over, taken back at the next round, the tables growing all along: the
    /// copies granted in a wave are refused in the round after it, together.
    Waves,
    /// No other use: as much room as the machine's source.
    None,
}

/// One scenario of the A/B.
pub struct Scenario {
    pub name: &'static str,
    /// The failure mode it forces, in words, for the records.
    pub forces: &'static str,
    pub tables: usize,
    pub rows: i64,
    pub statements: usize,
    pub press: Press,
    /// Rows each table grows by before each statement on it.
    pub grow: i64,
}

fn share(need: u64, f: f64) -> u64 {
    (need as f64 * f) as u64
}

impl Scenario {
    /// The room left the copies before any burst or wave: the records' `free_bytes` (None: no
    /// cap).
    pub fn free(&self, need: u64) -> Option<u64> {
        match self.press {
            Press::Steady { copies } => Some(share(need, copies)),
            Press::Bursty { .. } | Press::Grows | Press::Waves => Some(share(need, 0.5)),
            Press::None => None,
        }
    }

    /// The room left the copies at statement `k`.
    pub fn free_at(&self, k: usize, need: u64) -> Option<u64> {
        let base = self.free(need)?;
        let round = k / self.tables.max(1);
        Some(match self.press {
            Press::Bursty { piece } if k.is_multiple_of(2) => {
                base.saturating_add(share(need, piece))
            }
            Press::Grows if k >= self.statements / 2 => base.saturating_add(share(need, 1.0)),
            Press::Waves if round % 2 == 1 => {
                base.saturating_add(share(need, self.tables as f64 * 2.0))
            }
            _ => base,
        })
    }

    /// The table statement `k` reads.
    pub fn table(&self, k: usize) -> usize {
        k % self.tables.max(1)
    }

    /// Statement `k`: the queries in turn, each on the table in turn.
    pub fn query(&self, k: usize) -> String {
        QUERIES
            .get(k % QUERIES.len())
            .copied()
            .unwrap_or_default()
            .replace("FROM t", &format!("FROM t{}", self.table(k)))
    }
}

/// The A/B's scenarios at scale `k`. "Many tables" is 64, past the 12 of the control with no cap,
/// each table read four times a `k` so policy A pays its rent and buys again on every one.
pub fn scenarios(k: i64) -> Vec<Scenario> {
    let n = |s: i64| usize::try_from(s).unwrap_or(1);
    vec![
        Scenario {
            name: "laptop_cap_one_big_table",
            forces: "A: buys again and again, each a whole-table read, under room for half a copy",
            tables: 1,
            rows: 2000 * k,
            statements: n(12 * k),
            press: Press::Steady { copies: 0.5 },
            grow: 0,
        },
        Scenario {
            name: "many_tables_small_cap",
            forces: "both: six copies competing for two copies' room",
            tables: 6,
            rows: 300 * k,
            statements: n(24 * k),
            press: Press::Steady { copies: 2.0 },
            grow: 0,
        },
        Scenario {
            name: "steady_pressure",
            forces: "A: two copies thrashing in one and a half copies' room",
            tables: 2,
            rows: 500 * k,
            statements: n(16 * k),
            press: Press::Steady { copies: 1.5 },
            grow: 0,
        },
        Scenario {
            name: "bursty_pressure",
            forces: "B: waits that never end, room freed in pieces that never add up to a copy",
            tables: 2,
            rows: 500 * k,
            statements: n(16 * k),
            press: Press::Bursty { piece: 0.125 },
            grow: 0,
        },
        Scenario {
            name: "growing_while_waiting",
            forces: "B: the room a waiting copy needs growing with its table, the room coming late",
            tables: 1,
            rows: 500 * k,
            statements: n(16 * k),
            press: Press::Grows,
            grow: 20 * k,
        },
        Scenario {
            name: "huge_cap_many_tables",
            forces: "neither: the control, no cap",
            tables: 12,
            rows: 300 * k,
            statements: n(36 * k),
            press: Press::None,
            grow: 0,
        },
        Scenario {
            name: "many_tables_heavy_pressure",
            forces: "both at fleet scale: 64 tables in half a copy's room (A buys again on every table; B's waits never end)",
            tables: 64,
            rows: 100 * k,
            statements: n(256 * k),
            press: Press::Steady { copies: 0.5 },
            grow: 0,
        },
        Scenario {
            name: "many_tables_waves",
            forces: "both at fleet scale: room for every copy at once, then none, the tables growing (copies bought and let go together; B's waits all end at once)",
            tables: 64,
            rows: 100 * k,
            statements: n(256 * k),
            press: Press::Waves,
            grow: k,
        },
    ]
}

/// The copies' trace events (target `amphora::columns`), counted by their message, which says
/// what the copy did and why (AC-CB.3): counted on the thread that runs [`Traces::during`]'s
/// closure, where a statement's copy is bought, let go and waits.
#[derive(Clone, Default)]
pub struct Traces {
    seen: Arc<Mutex<BTreeMap<String, u64>>>,
}

/// The target the copies' events go to.
pub const TARGET: &str = "amphora::columns";

impl Traces {
    /// Runs `f` with this counter as the thread's subscriber.
    pub fn during<T>(&self, f: impl FnOnce() -> T) -> T {
        use tracing_subscriber::layer::SubscriberExt;
        let subscriber = tracing_subscriber::registry().with(self.clone());
        tracing::subscriber::with_default(subscriber, f)
    }

    /// Each message seen and how often.
    pub fn counts(&self) -> BTreeMap<String, u64> {
        self.seen.lock().clone()
    }

    /// Whether this build keeps the debug events counted: a build with `tracing_release` compiles
    /// them out, and its records say so rather than count none.
    pub fn compiled() -> bool {
        tracing::level_filters::STATIC_MAX_LEVEL >= tracing::level_filters::LevelFilter::DEBUG
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Traces {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() != TARGET {
            return;
        }
        let mut message = Message(String::new());
        event.record(&mut message);
        let mut seen = self.seen.lock();
        let n = seen.entry(message.0).or_insert(0);
        *n = n.saturating_add(1);
    }
}

struct Message(String);

impl tracing::field::Visit for Message {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-CB.4.2: the shared definition at the tiny scale. Expect: each scenario reads every table
    /// and names it in its statements; the control has no cap; under waves the room is the base
    /// in even rounds and room for every copy twice over in odd ones; bursts alternate.
    #[test]
    fn cb4_scenarios_press_as_defined() {
        let need = 1000u64;
        for s in scenarios(1) {
            assert!(s.tables >= 1 && s.statements >= s.tables, "{}", s.name);
            let read: std::collections::BTreeSet<usize> =
                (0..s.statements).map(|k| s.table(k)).collect();
            assert_eq!(read.len(), s.tables, "{}", s.name);
            let last = s.tables - 1;
            assert!(
                s.query(last).contains(&format!("FROM t{last}")),
                "{}",
                s.name
            );
            match s.press {
                Press::None => assert_eq!(s.free_at(0, need), None, "{}", s.name),
                Press::Waves => {
                    assert_eq!(s.free_at(0, need), Some(500));
                    let wave = 500 + 1000 * s.tables as u64 * 2;
                    assert_eq!(s.free_at(s.tables, need), Some(wave));
                    assert_eq!(s.free_at(2 * s.tables, need), Some(500));
                }
                Press::Bursty { piece } => {
                    assert_eq!(s.free_at(1, need), Some(500));
                    assert_eq!(s.free_at(0, need), Some(500 + (1000.0 * piece) as u64));
                }
                Press::Steady { copies } => {
                    assert_eq!(s.free_at(7, need), Some((1000.0 * copies) as u64));
                }
                Press::Grows => {
                    assert_eq!(s.free_at(0, need), Some(500));
                    assert_eq!(s.free_at(s.statements / 2, need), Some(1500));
                }
            }
        }
    }
}
