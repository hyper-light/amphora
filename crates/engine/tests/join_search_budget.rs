//! PL.8 (docs/design/planner.md decision 19): the join search budget's rules measured against each
//! other, behind `Connection::set_join_search_budget` (`Fixed`, the control; `RunShare`;
//! `RunShareByExecutions`). The budget only picks the search, so every rule must give the same
//! answers; the harness (`#[ignore]`, the coordinator's to run at scale) reports what each rule
//! costs in planning and running, one line a scenario and rule.
//!
//! Scenarios, each forcing a rule's failure mode: tiny repeated queries (planning outweighs the
//! run), chains of 50 and of 63 tables, the most a join may name (the exact search's subsets
//! explode), an underestimated join (the bound the rules derive from is wrong), a cold database
//! against a warm one (no measured rates, then measured ones), and a statistics change under load
//! (plans made again mid-stream). JOB is the coordinator's: `AMPHORA_JOB_DIR` names a directory of
//! its `.sql` files and an `imdb.db` loaded with its data, and the harness runs each of its
//! queries.

use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, JoinSearchBudget, OpenOptions, PlatformIO,
    SqliteDialect,
};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

const RULES: [JoinSearchBudget; 3] = [
    JoinSearchBudget::Fixed,
    JoinSearchBudget::RunShare,
    JoinSearchBudget::RunShareByExecutions,
];

fn open(path: &Path) -> Arc<Connection> {
    let io = Arc::new(PlatformIO::new().unwrap());
    let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
    let db = Database::open(io, path.to_str().unwrap(), options).unwrap();
    db.connect(ConnectOpts::new()).unwrap()
}

/// A run's rows, columns joined by `|`, sorted.
fn rows_of(rows: Vec<Vec<amphora_engine::Value>>) -> Vec<String> {
    let mut rows: Vec<String> = rows
        .into_iter()
        .map(|row| {
            row.iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>()
                .join("|")
        })
        .collect();
    rows.sort();
    rows
}

/// `tables` tables `t1..`, each `rows` rows, `tN.k` naming a row of `tN+1`; the chain's query.
fn chain(conn: &Arc<Connection>, tables: usize, rows: usize) -> String {
    for n in 1..=tables {
        conn.execute(format!(
            "CREATE TABLE t{n}(id INTEGER PRIMARY KEY, k INT, v INT)"
        ))
        .unwrap();
        conn.execute(format!(
            "WITH RECURSIVE s(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM s WHERE x < {rows}) INSERT INTO t{n} SELECT x, (x * 7) % {rows} + 1, x % 3 FROM s"
        ))
        .unwrap();
    }
    let joins: Vec<String> = (2..=tables)
        .map(|n| format!("JOIN t{n} ON t{n}.id = t{}.k", n - 1))
        .collect();
    format!(
        "SELECT count(*), sum(t{tables}.v) FROM t1 {} WHERE t1.v = 1",
        joins.join(" ")
    )
}

/// A fact table whose every row names one dimension row, where the planner's defaults expect a
/// spread: the join's rows are underestimated by the fact table's size.
fn skewed(conn: &Arc<Connection>, rows: usize) -> String {
    for sql in [
        "CREATE TABLE f(id INTEGER PRIMARY KEY, a INT, b INT)".to_owned(),
        "CREATE TABLE da(id INTEGER PRIMARY KEY, v INT)".to_owned(),
        "CREATE TABLE db(id INTEGER PRIMARY KEY, v INT)".to_owned(),
        format!("WITH RECURSIVE s(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM s WHERE x < {rows}) INSERT INTO f SELECT x, 1, x % 5 FROM s"),
        format!("WITH RECURSIVE s(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM s WHERE x < {rows}) INSERT INTO da SELECT x, x % 2 FROM s"),
        "WITH RECURSIVE s(x) AS (SELECT 0 UNION ALL SELECT x + 1 FROM s WHERE x < 4) INSERT INTO db SELECT x, x FROM s".to_owned(),
    ] {
        conn.execute(sql).unwrap();
    }
    "SELECT count(*) FROM f JOIN da ON da.id = f.a JOIN db ON db.id = f.b WHERE da.v = 1".to_owned()
}

/// What one scenario under one rule did: each run's answers, and the times its prepares and runs
/// took.
struct Measured {
    answers: Vec<Vec<String>>,
    prepare_nanos: Vec<u64>,
    run_nanos: Vec<u64>,
}

fn nanos(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

/// Runs `sql` `runs` times on `conn` under `rule`, prepared afresh each time (as a plan cache's
/// misses are) when `reprepare`, else prepared once (as its hits are). `between` runs after each
/// run: a statistics change under load.
fn measure(
    conn: &Arc<Connection>,
    rule: JoinSearchBudget,
    sql: &str,
    runs: usize,
    reprepare: bool,
    between: &dyn Fn(usize),
) -> Measured {
    conn.set_join_search_budget(rule);
    let mut out = Measured {
        answers: Vec::new(),
        prepare_nanos: Vec::new(),
        run_nanos: Vec::new(),
    };
    let mut kept = None;
    for run in 0..runs {
        if reprepare || kept.is_none() {
            let start = Instant::now();
            kept = Some(conn.prepare(sql).unwrap());
            out.prepare_nanos.push(nanos(start));
        }
        let stmt = kept.as_mut().unwrap();
        let start = Instant::now();
        let rows = stmt.run_collect_rows().unwrap();
        out.run_nanos.push(nanos(start));
        stmt.reset().unwrap();
        out.answers.push(rows_of(rows));
        between(run);
    }
    out
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    let Some(last) = sorted.len().checked_sub(1) else {
        return 0;
    };
    sorted[((last as f64 * p).round() as usize).min(last)]
}

fn report(scenario: &str, rule: JoinSearchBudget, m: &Measured) {
    let mut prepare = m.prepare_nanos.clone();
    let mut run = m.run_nanos.clone();
    prepare.sort_unstable();
    run.sort_unstable();
    let total: u64 = prepare.iter().chain(run.iter()).sum();
    println!(
        "pl8-budget scenario={scenario} rule={rule:?} prepares={} prepare_ns_p50={} p99={} p999={} runs={} run_ns_p50={} p99={} p999={} total_ns={total}",
        prepare.len(),
        percentile(&prepare, 0.5),
        percentile(&prepare, 0.99),
        percentile(&prepare, 0.999),
        run.len(),
        percentile(&run, 0.5),
        percentile(&run, 0.99),
        percentile(&run, 0.999),
    );
}

type Setup = fn(&Arc<Connection>, usize) -> String;

/// The scenarios at `scale` (1: a test's size), each run under every rule on a database of its
/// own, every rule's answers checked against the control's, run by run.
fn scenarios(scale: usize, print: bool) {
    let dir = tempfile::tempdir().unwrap();
    let mut made = 0;
    let mut fresh = || {
        made += 1;
        open(&dir.path().join(format!("pl8-{made}.db")))
    };
    let cases: [(&str, Setup, usize, bool); 6] = [
        (
            "tiny-repeated-hits",
            |c, s| chain(c, 2, 4 * s),
            200 * scale,
            false,
        ),
        (
            "tiny-repeated-misses",
            |c, s| chain(c, 2, 4 * s),
            200 * scale,
            true,
        ),
        ("chain-50", |c, s| chain(c, 50, 3 * s), 3, true),
        // 63 tables: the most a join may name (`TableMask` is a u64, as SQLite's `Bitmask`); 100 needs
        // a wider mask first.
        ("chain-63", |c, s| chain(c, 63, 3 * s), 3, true),
        ("underestimated", |c, s| skewed(c, 200 * s), 5 * scale, true),
        // A fresh database has measured nothing (the derived rules take the control's budget);
        // its later runs plan on the rates its first ones measured.
        (
            "cold-then-warm",
            |c, s| chain(c, 12, 20 * s),
            20 * scale,
            true,
        ),
    ];
    for (name, setup, runs, reprepare) in cases {
        let mut control: Option<Vec<Vec<String>>> = None;
        for rule in RULES {
            let conn = fresh();
            let sql = setup(&conn, scale);
            let m = measure(&conn, rule, &sql, runs, reprepare, &|_| {});
            if print {
                report(name, rule, &m);
            }
            match &control {
                None => control = Some(m.answers),
                Some(want) => assert_eq!(&m.answers, want, "{name} under {rule:?}"),
            }
        }
    }
    // A statistics change under load: rows added and ANALYZE run halfway, so the prepared
    // statement is planned again on new statistics while the stream runs.
    let mut control: Option<Vec<Vec<String>>> = None;
    for rule in RULES {
        let conn = fresh();
        let sql = chain(&conn, 8, 10 * scale);
        let runs = 20 * scale;
        let changer = conn.clone();
        let between = move |run: usize| {
            if run == runs / 2 {
                changer
                    .execute("INSERT INTO t1 SELECT id + 1000000, k, 1 FROM t1")
                    .unwrap();
                changer.execute("ANALYZE").unwrap();
            }
        };
        let m = measure(&conn, rule, &sql, runs, false, &between);
        if print {
            report("stats-change-under-load", rule, &m);
        }
        match &control {
            None => control = Some(m.answers),
            Some(want) => assert_eq!(&m.answers, want, "stats change under {rule:?}"),
        }
    }
}

/// Do: run every scenario at a test's size under each budget rule. Expect: each rule's answers,
/// run by run, the control's: the budget picks the search and never changes a result (T-PL.8.3).
#[test]
fn t_pl_8_3_every_budget_rule_gives_the_controls_answers() {
    scenarios(1, false);
}

/// The harness: every scenario at `AMPHORA_PL8_SCALE` (default 1), one line a scenario and rule.
/// The coordinator's to run at scale; it asserts only answers.
#[test]
#[ignore = "the coordinator's harness: run with --ignored --nocapture"]
fn pl8_budget_harness() {
    let scale = std::env::var("AMPHORA_PL8_SCALE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    scenarios(scale, true);
}

/// JOB under each rule: `AMPHORA_JOB_DIR` holds `imdb.db` and the queries' `.sql` files; each
/// query's answers checked against the control's, one line a query and rule.
#[test]
#[ignore = "the coordinator's harness: needs AMPHORA_JOB_DIR"]
fn pl8_budget_job() {
    let dir = std::env::var("AMPHORA_JOB_DIR").expect("AMPHORA_JOB_DIR names JOB's directory");
    let dir = Path::new(&dir);
    let conn = open(&dir.join("imdb.db"));
    let mut queries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
        .collect();
    queries.sort();
    for query in queries {
        let sql = std::fs::read_to_string(&query).unwrap();
        let name = query.file_stem().unwrap().to_string_lossy().into_owned();
        let mut control: Option<Vec<Vec<String>>> = None;
        for rule in RULES {
            let m = measure(&conn, rule, &sql, 3, true, &|_| {});
            report(&format!("job-{name}"), rule, &m);
            match &control {
                None => control = Some(m.answers),
                Some(want) => assert_eq!(&m.answers, want, "JOB {name} under {rule:?}"),
            }
        }
    }
}
