//! QT.5 (T-QT.5.10, docs/design/planner.md, "Decision (lane qs5-18)"): the two row estimates of
//! `col IS ?` where planning doesn't know whether the value is NULL, measured against each other
//! behind `Connection::set_is_value_estimate`: `Equality` (SQLite's, the default) and `WorstCase`
//! (the larger of the `=` and `IS NULL` estimates, the control: what every NULL-matching `IS` took
//! before). Both must give the same answers; each scenario forces one rule's failure mode:
//!
//! - **Mostly NULL, NULL bound.** A column NULL in 19 rows of 20, its index searched for NULL. A run
//!   whose only parameter is NULL always takes the generic plan (`Statement`'s plan choice: nothing
//!   bound), so `Equality` searches the index for nearly every row and looks each up in the table,
//!   where `WorstCase` scans it once.
//! - **Distinct, values bound.** A column of distinct values, each run binding one. The generic plan
//!   under `WorstCase` scans the table for a one-row answer; with custom plans allowed it plans every
//!   run again, since each custom plan (its value known) costs far less than the generic one.
//!
//! The default test holds each rule's answers to the literal query's and asserts each failure mode
//! in cursor steps (seeks and advances, the statement's counters): no clock. The harness
//! (`#[ignore]`, the coordinator's to run at scale) reports, a line a scenario, rule, plan mode,
//! statistics and concurrency, the plan, its cursor steps a run, its custom plans and its latency
//! p50, p99 and p99.9, over `AMPHORA_QT510_ROWS` rows (default 1,000 and 1,000,000) and the
//! concurrency ladder 1, 8, 32, 128, 512, 2048, 4096 (`AMPHORA_QT510_LADDER` cuts it).

use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, IsValueEstimate, OpenOptions, PlanCacheMode,
    PlatformIO, SqliteDialect, Statement, Value,
};
use std::num::NonZero;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

const RULES: [IsValueEstimate; 2] = [IsValueEstimate::Equality, IsValueEstimate::WorstCase];

const QUERY: &str = "SELECT count(*), sum(length(pad)) FROM t WHERE a IS ?1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scenario {
    /// `a` NULL in 19 rows of 20, each run binding NULL: `Equality`'s failure mode.
    MostlyNullNullBound,
    /// `a` distinct, each run binding one of its values: `WorstCase`'s.
    DistinctValuesBound,
}

fn open_db(path: &Path) -> Arc<Database> {
    let io = Arc::new(PlatformIO::new().unwrap());
    let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
    Database::open(io, path.to_str().unwrap(), options).unwrap()
}

/// `t` of `rows` rows with an index on `a`, laid out for `scenario`; analyzed when `analyze`.
fn fill(conn: &Arc<Connection>, scenario: Scenario, rows: usize, analyze: bool) {
    let a = match scenario {
        Scenario::MostlyNullNullBound => "CASE WHEN x % 20 = 0 THEN x END",
        Scenario::DistinctValuesBound => "x",
    };
    for sql in [
        "CREATE TABLE t (id INTEGER PRIMARY KEY, a INT, pad BLOB)".to_owned(),
        "CREATE INDEX t_a ON t (a)".to_owned(),
        format!("WITH RECURSIVE s(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM s WHERE x < {rows}) INSERT INTO t SELECT x, {a}, zeroblob(100) FROM s"),
    ] {
        conn.execute(sql).unwrap();
    }
    if analyze {
        conn.execute("ANALYZE").unwrap();
    }
}

/// The value run `run` binds in `scenario`.
fn value(scenario: Scenario, rows: usize, run: usize) -> Value {
    match scenario {
        Scenario::MostlyNullNullBound => Value::Null,
        // A spread of existing keys, a different one each run.
        Scenario::DistinctValuesBound => {
            Value::from_i64(i64::try_from(run * 7_919 % rows + 1).unwrap())
        }
    }
}

/// The literal query's answer for `value`: the control every rule's run must equal.
fn literal_answer(conn: &Arc<Connection>, value: &Value) -> Vec<Vec<Value>> {
    let literal = match value {
        Value::Null => "NULL".to_owned(),
        other => other.to_string(),
    };
    conn.prepare(QUERY.replace("?1", &literal))
        .unwrap()
        .run_collect_rows()
        .unwrap()
}

/// The plan's lines of `QUERY` as prepared now, unbound (the generic plan).
fn plan(conn: &Arc<Connection>) -> String {
    let mut stmt = conn.prepare(format!("EXPLAIN QUERY PLAN {QUERY}")).unwrap();
    stmt.run_collect_rows()
        .unwrap()
        .iter()
        .map(|row| row[3].to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

/// One run of `stmt` with `value` bound: its answer, its cursor steps (seeks and advances) and
/// whether it was planned for its values (a custom plan).
fn run(stmt: &mut Statement, value: Value) -> (Vec<Vec<Value>>, u64, u64) {
    stmt.reset_metrics();
    stmt.bind_at(NonZero::new(1).unwrap(), value).unwrap();
    let rows = stmt.run_collect_rows().unwrap();
    stmt.reset().unwrap();
    let m = stmt.metrics();
    (
        rows,
        m.btree_seeks + m.btree_next + m.btree_prev,
        m.custom_plans,
    )
}

/// T-QT.5.10. Do: under each rule, the generic plan forced, run `a IS ?` ten times in each scenario
/// on 2,000 rows, without statistics and with them. Expect: every answer the literal query's; and
/// without statistics (a million rows assumed, so the estimates part), with NULL bound on the
/// mostly-NULL column `Equality` takes more cursor steps than `WorstCase` (it searches the index
/// and looks up nearly every row), and with distinct values bound `WorstCase` more than `Equality`
/// (it scans for a one-row answer). With statistics the two may plan alike; the harness measures.
#[test]
fn t_qt_5_10_each_is_estimate_answers_alike_and_meets_its_failure_mode() {
    let rows = 2_000;
    for analyze in [false, true] {
        for scenario in [Scenario::MostlyNullNullBound, Scenario::DistinctValuesBound] {
            let dir = tempfile::tempdir().unwrap();
            let db = open_db(&dir.path().join("is.db"));
            let conn = db.connect(ConnectOpts::new()).unwrap();
            fill(&conn, scenario, rows, analyze);
            conn.set_plan_cache_mode(PlanCacheMode::ForceGeneric);
            let mut steps = Vec::new();
            for rule in RULES {
                conn.set_is_value_estimate(rule);
                let mut stmt = conn.prepare(QUERY).unwrap();
                let mut total = 0;
                for i in 0..10 {
                    let v = value(scenario, rows, i);
                    let want = literal_answer(&conn, &v);
                    let (got, work, _) = run(&mut stmt, v);
                    assert_eq!(got, want, "{scenario:?} {rule:?} analyze={analyze} run {i}");
                    total += work;
                }
                steps.push((rule, total, plan(&conn)));
            }
            let [(_, equality, eq_plan), (_, worst, worst_plan)] = steps.as_slice() else {
                panic!("one measure for each of the two rules, got {}", steps.len());
            };
            let say = format!(
                "{scenario:?} analyze={analyze}: Equality {equality} steps ({eq_plan}), WorstCase {worst} steps ({worst_plan})"
            );
            println!("{say}");
            match (analyze, scenario) {
                (true, _) => {}
                (false, Scenario::MostlyNullNullBound) => assert!(equality > worst, "{say}"),
                (false, Scenario::DistinctValuesBound) => assert!(worst > equality, "{say}"),
            }
        }
    }
}

/// What one configuration measured: its run latencies, cursor steps and custom plans in all.
struct Measured {
    run_nanos: Vec<u64>,
    steps: u64,
    custom_plans: u64,
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    let Some(last) = sorted.len().checked_sub(1) else {
        return 0;
    };
    sorted[((last as f64 * p).round() as usize).min(last)]
}

/// `clients` connections to `db`, each its own thread, each preparing `QUERY` once and running it
/// `runs` times, every client starting together (a correlated burst of one template).
fn measure(
    db: &Arc<Database>,
    scenario: Scenario,
    rows: usize,
    rule: IsValueEstimate,
    mode: PlanCacheMode,
    clients: usize,
    runs: usize,
) -> Measured {
    let start = Arc::new(std::sync::Barrier::new(clients));
    let handles: Vec<_> = (0..clients)
        .map(|client| {
            let (db, start) = (db.clone(), start.clone());
            std::thread::spawn(move || {
                let conn = db.connect(ConnectOpts::new()).unwrap();
                conn.set_is_value_estimate(rule);
                conn.set_plan_cache_mode(mode);
                let mut stmt = conn.prepare(QUERY).unwrap();
                start.wait();
                let mut out = Measured {
                    run_nanos: Vec::with_capacity(runs),
                    steps: 0,
                    custom_plans: 0,
                };
                for i in 0..runs {
                    let began = Instant::now();
                    let (_, steps, custom) =
                        run(&mut stmt, value(scenario, rows, client * runs + i));
                    out.run_nanos
                        .push(u64::try_from(began.elapsed().as_nanos()).unwrap_or(u64::MAX));
                    out.steps += steps;
                    out.custom_plans += custom;
                }
                out
            })
        })
        .collect();
    let mut all = Measured {
        run_nanos: Vec::new(),
        steps: 0,
        custom_plans: 0,
    };
    for handle in handles {
        let one = handle.join().unwrap();
        all.run_nanos.extend(one.run_nanos);
        all.steps += one.steps;
        all.custom_plans += one.custom_plans;
    }
    all.run_nanos.sort_unstable();
    all
}

fn env_list(name: &str, default: &[usize]) -> Vec<usize> {
    std::env::var(name).map_or_else(
        |_| default.to_vec(),
        |text| text.split(',').map(|n| n.trim().parse().unwrap()).collect(),
    )
}

/// The coordinator's harness: every scenario, rule, plan mode (the generic plan forced, and the
/// default choice), with and without statistics, at each size and each step of the concurrency
/// ladder, one line each. Tiny instance: `AMPHORA_QT510_ROWS=1000 AMPHORA_QT510_LADDER=1,8`.
#[test]
#[ignore = "the coordinator's harness: run with --ignored --nocapture"]
fn qt_5_10_is_estimate_harness() {
    let sizes = env_list("AMPHORA_QT510_ROWS", &[1_000, 1_000_000]);
    let ladder = env_list("AMPHORA_QT510_LADDER", &[1, 8, 32, 128, 512, 2048, 4096]);
    let runs = env_list("AMPHORA_QT510_RUNS", &[20])[0];
    for &rows in &sizes {
        for analyze in [false, true] {
            for scenario in [Scenario::MostlyNullNullBound, Scenario::DistinctValuesBound] {
                let dir = tempfile::tempdir().unwrap();
                let db = open_db(&dir.path().join("is.db"));
                let conn = db.connect(ConnectOpts::new()).unwrap();
                fill(&conn, scenario, rows, analyze);
                for rule in RULES {
                    conn.set_is_value_estimate(rule);
                    let generic_plan = plan(&conn);
                    for mode in [PlanCacheMode::ForceGeneric, PlanCacheMode::Auto] {
                        for &clients in &ladder {
                            let m = measure(&db, scenario, rows, rule, mode, clients, runs);
                            let n = m.run_nanos.len().max(1) as u64;
                            println!(
                                "qt5-10 scenario={scenario:?} rows={rows} analyze={analyze} rule={rule:?} mode={mode:?} clients={clients} runs={} plan=\"{generic_plan}\" steps_per_run={} custom_plans={} run_ns_p50={} p99={} p999={}",
                                m.run_nanos.len(),
                                m.steps / n,
                                m.custom_plans,
                                percentile(&m.run_nanos, 0.5),
                                percentile(&m.run_nanos, 0.99),
                                percentile(&m.run_nanos, 0.999),
                            );
                        }
                    }
                }
            }
        }
    }
}
