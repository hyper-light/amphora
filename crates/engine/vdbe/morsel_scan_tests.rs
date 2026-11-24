//! T-QT.6.7 and T-QT.6.8: morsels over a table's own leaf pages, each worker with its own cursor in
//! the statement's snapshot, answer exactly as the serial VDBE does (docs/design/executor.md §8).

use super::*;
use crate::io::MemoryIO;
use crate::vdbe::morsel::{Aggregate, CmpOp, Predicate};
use crate::{ConnectOpts, Connection, Database, DatabaseOpts, OpenFlags, SqliteDialect};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::num::NonZeroUsize;

const COLUMNS: [&str; 4] = ["a", "b", "c", "p"];

fn open(name: &str) -> Arc<Connection> {
    let io = Arc::new(MemoryIO::new());
    let db = Database::open_file_with_flags(
        io,
        name,
        OpenFlags::default(),
        DatabaseOpts::new(),
        None,
        Arc::new(SqliteDialect),
    )
    .unwrap();
    db.connect(ConnectOpts::new()).unwrap()
}

fn sql_rows(conn: &Arc<Connection>, sql: &str) -> Vec<Vec<Value>> {
    conn.prepare(sql).unwrap().run_collect_rows().unwrap()
}

/// T-QT.5.18, T-QT.6.19: the kept-side outer join uses morsels for a hash plan.
/// Forcing merge must execute the planned merge under the same parallel setting.
#[test]
fn a_kept_side_merge_join_stays_serial() {
    let conn = open(":memory:merge-chain");
    let mut rng = ChaCha8Rng::seed_from_u64(518);
    fill(&conn, &mut rng, "t", 64);
    fill(&conn, &mut rng, "u", 48);
    let sql = "SELECT count(*) FROM t LEFT JOIN u ON u.a = t.a";
    conn.set_parallel_mode(ParallelMode::Off);
    let serial = sql_rows(&conn, sql);
    conn.set_parallel_mode(ParallelMode::On);
    conn.set_parallel_outer_probe(crate::OuterJoinProbe::Kept);
    for (method, morsels) in [
        (crate::JoinMethods::HashJoinsOnly, true),
        (crate::JoinMethods::MergeJoinsOnly, false),
    ] {
        conn.set_join_methods(method);
        let plan = sql_rows(&conn, &format!("EXPLAIN QUERY PLAN {sql}"));
        assert!(
            plan.iter()
                .flatten()
                .any(|v| v
                    .to_string()
                    .contains(if morsels { "HASH JOIN" } else { "MERGE JOIN" })),
            "{plan:?}"
        );
        let bytecode = sql_rows(&conn, &format!("EXPLAIN {sql}"));
        let hooked = bytecode.iter().any(|r| {
            r.get(1)
                .is_some_and(|v| v.to_string().contains("MorselAggregate"))
        });
        assert_eq!(
            sql_rows(&conn, sql),
            serial,
            "{method:?}: the serial answer"
        );
        assert_eq!(hooked, morsels, "{method:?}");
    }
}

fn literal(value: &Value) -> String {
    match value {
        Value::Null => "NULL".to_string(),
        Value::Text(text) => format!("'{}'", text.as_str()),
        other => other.to_string(),
    }
}

/// A table `name(a INTEGER, b INTEGER, c TEXT, p TEXT)` of `rows` random rows, some deleted, some
/// at negative and far rowids, padded so it spans many leaf pages.
fn fill(conn: &Arc<Connection>, rng: &mut ChaCha8Rng, name: &str, rows: usize) {
    conn.execute(format!(
        "CREATE TABLE {name}(a INTEGER, b INTEGER, c TEXT, p TEXT)"
    ))
    .unwrap();
    conn.execute("BEGIN").unwrap();
    for i in 0..rows {
        let a = if rng.random_range(0..10) == 0 {
            "NULL".to_string()
        } else {
            rng.random_range(-20i64..20).to_string()
        };
        let b = if rng.random_range(0..10) == 0 {
            "NULL".to_string()
        } else {
            rng.random_range(-1_000_000_000_000i64..1_000_000_000_000)
                .to_string()
        };
        let c = if rng.random_range(0..10) == 0 {
            "NULL".to_string()
        } else {
            format!("'t{}'", rng.random_range(0..12))
        };
        let rowid = match rng.random_range(0..20) {
            0 => format!("{}", -1 - i64::try_from(i).unwrap()),
            1 => format!("{}", 1_000_000_000_000i64 + i64::try_from(i).unwrap()),
            _ => "NULL".to_string(),
        };
        let pad = "x".repeat(rng.random_range(0..120));
        conn.execute(format!(
            "INSERT INTO {name}(rowid, a, b, c, p) VALUES ({rowid}, {a}, {b}, {c}, '{pad}')"
        ))
        .unwrap();
    }
    conn.execute(format!("DELETE FROM {name} WHERE (rowid % 7) = 3"))
        .unwrap();
    conn.execute("COMMIT").unwrap();
}

fn source(conn: &Arc<Connection>, name: &str) -> TableSource {
    let root = conn
        .schema
        .read()
        .get_table(name)
        .unwrap()
        .get_root_page()
        .unwrap();
    TableSource::open(conn.get_pager(), root, COLUMNS.len(), None, Vec::new()).unwrap()
}

fn random_query(rng: &mut ChaCha8Rng) -> (Vec<Predicate>, Vec<usize>, Vec<Aggregate>) {
    let ops = [
        CmpOp::Lt,
        CmpOp::Le,
        CmpOp::Eq,
        CmpOp::Ne,
        CmpOp::Gt,
        CmpOp::Ge,
    ];
    let mut filter = Vec::new();
    for _ in 0..rng.random_range(0..3) {
        filter.push(match rng.random_range(0..4) {
            0 => Predicate::IsNull {
                column: rng.random_range(0..3),
            },
            1 => Predicate::NotNull {
                column: rng.random_range(0..3),
            },
            2 => Predicate::Compare {
                column: 2,
                op: CmpOp::Eq,
                value: Value::build_text(format!("t{}", rng.random_range(0..12))),
            },
            _ => {
                let column = rng.random_range(0..2);
                let value = if column == 0 {
                    rng.random_range(-20i64..20)
                } else {
                    rng.random_range(-1_000_000_000_000i64..1_000_000_000_000)
                };
                Predicate::Compare {
                    column,
                    op: ops[rng.random_range(0..ops.len())],
                    value: Value::from_i64(value),
                }
            }
        });
    }
    let group_by = match rng.random_range(0..4) {
        0 => vec![],
        1 => vec![0],
        2 => vec![2],
        _ => vec![2, 0],
    };
    let mut aggregates = vec![Aggregate::CountStar];
    for _ in 0..rng.random_range(0..4) {
        aggregates.push(match rng.random_range(0..4) {
            0 => Aggregate::Count(rng.random_range(0..3)),
            1 => Aggregate::Sum(rng.random_range(0..2)),
            2 => Aggregate::Min(rng.random_range(0..3)),
            _ => Aggregate::Max(rng.random_range(0..3)),
        });
    }
    (filter, group_by, aggregates)
}

fn to_sql(
    name: &str,
    filter: &[Predicate],
    group_by: &[usize],
    aggregates: &[Aggregate],
) -> String {
    let mut select: Vec<String> = group_by.iter().map(|g| COLUMNS[*g].to_string()).collect();
    for aggregate in aggregates {
        select.push(match aggregate {
            Aggregate::CountStar => "count(*)".to_string(),
            Aggregate::Count(c) => format!("count({})", COLUMNS[*c]),
            Aggregate::Sum(c) => format!("sum({})", COLUMNS[*c]),
            Aggregate::Min(c) => format!("min({})", COLUMNS[*c]),
            Aggregate::Max(c) => format!("max({})", COLUMNS[*c]),
            Aggregate::Avg(c) => format!("avg({})", COLUMNS[*c]),
            Aggregate::Total(c) => format!("total({})", COLUMNS[*c]),
        });
    }
    let terms: Vec<String> = filter
        .iter()
        .map(|term| match term {
            Predicate::IsNull { column } => format!("{} IS NULL", COLUMNS[*column]),
            Predicate::NotNull { column } => format!("{} IS NOT NULL", COLUMNS[*column]),
            Predicate::Compare { column, op, value } => {
                let op = match op {
                    CmpOp::Lt => "<",
                    CmpOp::Le => "<=",
                    CmpOp::Eq => "=",
                    CmpOp::Ne => "<>",
                    CmpOp::Gt => ">",
                    CmpOp::Ge => ">=",
                };
                format!("{} {op} {}", COLUMNS[*column], literal(value))
            }
            Predicate::Parameter { .. } => panic!("the fuzz shapes bind no parameter"),
        })
        .collect();
    let mut sql = format!("SELECT {} FROM {name}", select.join(", "));
    if !terms.is_empty() {
        sql.push_str(&format!(" WHERE {}", terms.join(" AND ")));
    }
    if !group_by.is_empty() {
        let keys: Vec<&str> = group_by.iter().map(|g| COLUMNS[*g]).collect();
        sql.push_str(&format!(" GROUP BY {}", keys.join(", ")));
    }
    sql
}

/// Rows as text, sorted: SQL and the pipeline order groups by the same keys but may print a
/// grouping's text and integers in another order, so the answers are compared as sets of rows.
fn canonical(rows: &[Vec<Value>]) -> Vec<String> {
    let mut out: Vec<String> = rows.iter().map(|row| format!("{row:?}")).collect();
    out.sort();
    out
}

fn degrees(leaves: usize) -> Vec<Degree> {
    let mut out = vec![Degree::Serial];
    for workers in [2usize, 3, 4, 8] {
        for morsel in [1usize, 2, 5, leaves.max(1)] {
            out.push(Degree::Parallel {
                workers: NonZeroUsize::new(workers).unwrap(),
                morsel_rows: NonZeroUsize::new(morsel).unwrap(),
            });
        }
    }
    out
}

/// T-QT.6.7: random filters, groupings and aggregates over random tables of many leaf pages, run
/// with each worker's own cursor over its leaves at 1, 2, 3, 4 and 8 workers and several morsel
/// sizes, and through `ParallelMode::On` and `Off`, equal the VDBE's serial answer row for row.
#[test]
fn t_qt_6_7_leaf_morsels_answer_as_the_serial_vdbe() {
    // G7 step 2 (`AC-NS.2.1`): a test of the pager's leaves.
    if crate::storage::versioned::switch::pager_only(
        "it reads the pager's leaf pages at the table's root page (TableSource::open), which on the versioned storage is a tree's id, not a page; there a parallel aggregate is answered by the table's column copy or the serial loop (G11 step 5, CS.5)",
    ) {
        return;
    }
    let conn = open(":memory:qt-6-7");
    let mut rng = ChaCha8Rng::seed_from_u64(67);
    fill(&conn, &mut rng, "t", 3000);
    let mut costs = MachineCosts::measure().unwrap();
    // One read transaction: every worker's cursor reads the statement's one snapshot.
    conn.execute("BEGIN").unwrap();
    let count = sql_rows(&conn, "SELECT count(*) FROM t");
    let table = source(&conn, "t");
    assert!(
        table.leaves() > 8,
        "the table spans {} leaves",
        table.leaves()
    );
    let mut parallel_runs = 0usize;
    for _ in 0..60 {
        let (filter, group_by, aggregates) = random_query(&mut rng);
        let sql = to_sql("t", &filter, &group_by, &aggregates);
        let expected = canonical(&sql_rows(&conn, &sql));
        let shape = ScanAggregate {
            rows: &[],
            filter: &filter,
            group_by: &group_by,
            aggregates: &aggregates,
            sums: crate::RealSums::Serial,
        };
        let pipeline = TableAggregate {
            table: &table,
            shape: &shape,
        };
        for degree in degrees(table.leaves()) {
            let budget = MemoryBudget::new(usize::MAX);
            let (answer, stats) = pipeline.run_at(degree, &budget).unwrap();
            assert_eq!(canonical(&answer), expected, "{sql} at {degree:?}");
            if stats.workers > 1 {
                parallel_runs += 1;
            }
        }
        for mode in [ParallelMode::Off, ParallelMode::On] {
            let budget = MemoryBudget::new(usize::MAX);
            let (answer, _) = pipeline.run_mode(mode, &mut costs, &budget).unwrap();
            assert_eq!(canonical(&answer), expected, "{sql} in {mode:?}");
        }
    }
    // Non-vacuity: the parallel degrees really ran on several workers.
    assert!(parallel_runs > 0);
    assert_eq!(count, sql_rows(&conn, "SELECT count(*) FROM t"));
    conn.execute("COMMIT").unwrap();
}

/// T-QT.6.8: a hash join's probe side read by workers over the probe table's leaves equals the
/// VDBE's join, match for match and in its order (probe rowid, then build rowid).
#[test]
fn t_qt_6_8_leaf_morsel_probe_matches_the_serial_vdbe_join() {
    // G7 step 2 (`AC-NS.2.1`): a test of the pager's leaves.
    if crate::storage::versioned::switch::pager_only(
        "it reads the pager's leaf pages at the table's root page (TableSource::open), which on the versioned storage is a tree's id, not a page; there a parallel aggregate is answered by the table's column copy or the serial loop (G11 step 5, CS.5)",
    ) {
        return;
    }
    let conn = open(":memory:qt-6-8");
    let mut rng = ChaCha8Rng::seed_from_u64(68);
    fill(&conn, &mut rng, "t", 2500);
    fill(&conn, &mut rng, "u", 300);
    let mut costs = MachineCosts::measure().unwrap();
    conn.execute("BEGIN").unwrap();
    let probe_table = source(&conn, "t");
    let build_table = source(&conn, "u");
    for (probe_key, build_key) in [(vec![0usize], vec![0usize]), (vec![2, 0], vec![2, 0])] {
        let on: Vec<String> = probe_key
            .iter()
            .zip(&build_key)
            .map(|(p, b)| format!("t.{} = u.{}", COLUMNS[*p], COLUMNS[*b]))
            .collect();
        let sql = format!(
            "SELECT t.rowid, u.rowid FROM t JOIN u ON {} ORDER BY t.rowid, u.rowid",
            on.join(" AND ")
        );
        let expected: Vec<(i64, i64)> = sql_rows(&conn, &sql)
            .into_iter()
            .map(|row| match (&row[0], &row[1]) {
                (
                    Value::Numeric(crate::numeric::Numeric::Integer(t)),
                    Value::Numeric(crate::numeric::Numeric::Integer(u)),
                ) => (*t, *u),
                other => panic!("not two rowids: {other:?}"),
            })
            .collect();
        assert!(!expected.is_empty());
        let build_rows = build_table.rows().unwrap();
        let build_values: Vec<Vec<Value>> = build_rows.iter().map(|(_, row)| row.clone()).collect();
        let budget = MemoryBudget::new(usize::MAX);
        let probe = HashProbe::build(&build_values, &build_key, &[], &probe_key, &budget).unwrap();
        let pipeline = TableProbe {
            table: &probe_table,
            probe: &probe,
        };
        let as_rowids = |matches: Vec<(i64, usize)>| -> Vec<(i64, i64)> {
            matches
                .into_iter()
                .map(|(t, build)| (t, build_rows[build].0))
                .collect()
        };
        for degree in degrees(probe_table.leaves()) {
            let budget = MemoryBudget::new(usize::MAX);
            let (matches, _) = pipeline.run_at(degree, &budget).unwrap();
            assert_eq!(as_rowids(matches), expected, "{sql} at {degree:?}");
        }
        for mode in [ParallelMode::Off, ParallelMode::On] {
            let budget = MemoryBudget::new(usize::MAX);
            let (matches, _) = pipeline.run_mode(mode, &mut costs, &budget).unwrap();
            assert_eq!(as_rowids(matches), expected, "{sql} in {mode:?}");
        }
    }
    conn.execute("COMMIT").unwrap();
}

/// T-QT.6.9: a table whose INTEGER PRIMARY KEY is its rowid reads the rowid into that column,
/// and an empty table is one leaf with no rows.
#[test]
fn t_qt_6_9_rowid_alias_and_empty_table() {
    // G7 step 2 (`AC-NS.2.1`): a test of the pager's leaves.
    if crate::storage::versioned::switch::pager_only(
        "it reads the pager's leaf pages at the table's root page (TableSource::open), which on the versioned storage is a tree's id, not a page; there a parallel aggregate is answered by the table's column copy or the serial loop (G11 step 5, CS.5)",
    ) {
        return;
    }
    let conn = open(":memory:qt-6-9");
    conn.execute("CREATE TABLE e(id INTEGER PRIMARY KEY, v INTEGER)")
        .unwrap();
    conn.execute("BEGIN").unwrap();
    let root = conn
        .schema
        .read()
        .get_table("e")
        .unwrap()
        .get_root_page()
        .unwrap();
    let empty = TableSource::open(conn.get_pager(), root, 2, Some(0), Vec::new()).unwrap();
    assert_eq!(empty.leaves(), 1);
    assert!(empty.rows().unwrap().is_empty());
    conn.execute("COMMIT").unwrap();
    for i in 0..2000 {
        conn.execute(format!(
            "INSERT INTO e VALUES ({}, {})",
            i * 3 - 500,
            i % 17
        ))
        .unwrap();
    }
    conn.execute("BEGIN").unwrap();
    let expected = sql_rows(
        &conn,
        "SELECT sum(id), min(id), max(id), count(*) FROM e WHERE v > 4",
    );
    let table = TableSource::open(conn.get_pager(), root, 2, Some(0), Vec::new()).unwrap();
    assert!(table.leaves() > 2);
    let filter = [Predicate::Compare {
        column: 1,
        op: CmpOp::Gt,
        value: Value::from_i64(4),
    }];
    let aggregates = [
        Aggregate::Sum(0),
        Aggregate::Min(0),
        Aggregate::Max(0),
        Aggregate::CountStar,
    ];
    let shape = ScanAggregate {
        rows: &[],
        filter: &filter,
        group_by: &[],
        aggregates: &aggregates,
        sums: crate::RealSums::Serial,
    };
    let pipeline = TableAggregate {
        table: &table,
        shape: &shape,
    };
    for degree in degrees(table.leaves()) {
        let budget = MemoryBudget::new(usize::MAX);
        let (answer, _) = pipeline.run_at(degree, &budget).unwrap();
        assert_eq!(answer, expected, "at {degree:?}");
    }
    conn.execute("COMMIT").unwrap();
}

/// T-QT.6.10: the VDBE's hook. Random filters, groupings and aggregates in SQL answer the same,
/// row for row and in order, with `ParallelMode` off (the serial loop) and on, the machine stated
/// as 2, 3, 4 and 8 cores with workers cheap to start, so the run takes that many workers; and
/// the hook really ran (the parallel run measured the connection's costs).
#[test]
fn t_qt_6_10_vdbe_hook_answers_as_the_serial_loop() {
    // G7 step 2 (`AC-NS.2.1`): a test of the pager's leaf morsels.
    if crate::storage::versioned::switch::pager_only(
        "its subject is the morsel hook over the pager's leaf pages (that the workers ran); on the versioned storage the hook hands the statement to the table's column copy or the serial loop (G11 step 5, CS.5), so with parallel mode on it would compare the serial loop with itself",
    ) {
        return;
    }
    let conn = open(":memory:qt-6-10");
    let mut rng = ChaCha8Rng::seed_from_u64(610);
    fill(&conn, &mut rng, "t", 3000);
    let mut hooked = 0usize;
    for _ in 0..60 {
        let (filter, group_by, aggregates) = random_query(&mut rng);
        let sql = to_sql("t", &filter, &group_by, &aggregates);
        conn.set_parallel_mode(ParallelMode::Off);
        let serial = sql_rows(&conn, &sql);
        conn.set_parallel_mode(ParallelMode::On);
        for cores in [2usize, 3, 4, 8] {
            *conn.morsel_costs.lock() = Some(MachineCosts::stated(
                NonZeroUsize::new(cores).unwrap(),
                1.0,
                1.0,
            ));
            assert_eq!(sql_rows(&conn, &sql), serial, "{sql} on {cores} cores");
            // The stated costs are learned from by the run: a changed mean shows it ran.
            let ran = conn
                .morsel_costs
                .lock()
                .as_ref()
                .is_some_and(|costs| costs.worker_start().samples() > 1);
            if ran {
                hooked += 1;
            }
        }
    }
    conn.set_parallel_mode(ParallelMode::Off);
    // Non-vacuity: most queries qualify (every shape `random_query` makes does).
    assert!(hooked > 120, "the hook ran {hooked} times of 240");
}

/// A random inner equi-join of `t` and `u` on one or two columns, with filters on either table, a
/// grouping and aggregates over both tables' columns.
fn random_join(rng: &mut ChaCha8Rng) -> String {
    let tables = ["t", "u"];
    let ops = ["<", "<=", "=", "<>", ">", ">="];
    let keys: &[&str] = match rng.random_range(0..3) {
        0 => &["a"],
        1 => &["c"],
        _ => &["a", "c"],
    };
    let on: Vec<String> = keys.iter().map(|k| format!("t.{k} = u.{k}")).collect();
    let mut terms = Vec::new();
    for _ in 0..rng.random_range(0..3) {
        let table = tables[rng.random_range(0..2)];
        let op = ops[rng.random_range(0..ops.len())];
        terms.push(match rng.random_range(0..5) {
            0 => format!("{table}.a {op} {}", rng.random_range(-20i64..20)),
            1 => format!(
                "{table}.b {op} {}",
                rng.random_range(-1_000_000_000_000i64..1_000_000_000_000)
            ),
            2 => format!("{table}.c {op} 't{}'", rng.random_range(0..12)),
            3 => format!("{table}.c IS NULL"),
            _ => format!("{table}.a IS NOT NULL"),
        });
    }
    let column = |rng: &mut ChaCha8Rng| {
        format!(
            "{}.{}",
            tables[rng.random_range(0..2)],
            ["a", "b", "c"][rng.random_range(0..3)]
        )
    };
    let group_by: Vec<String> = (0..rng.random_range(0..3)).map(|_| column(rng)).collect();
    let mut select = group_by.clone();
    for _ in 0..rng.random_range(1..4) {
        select.push(match rng.random_range(0..5) {
            0 => "count(*)".to_string(),
            1 => format!("count({})", column(rng)),
            2 => format!(
                "sum({}.{})",
                tables[rng.random_range(0..2)],
                ["a", "b"][rng.random_range(0..2)]
            ),
            3 => format!("min({})", column(rng)),
            _ => format!("max({})", column(rng)),
        });
    }
    let mut sql = format!(
        "SELECT {} FROM t JOIN u ON {}",
        select.join(", "),
        on.join(" AND ")
    );
    if !terms.is_empty() {
        sql.push_str(&format!(" WHERE {}", terms.join(" AND ")));
    }
    if !group_by.is_empty() {
        sql.push_str(&format!(" GROUP BY {}", group_by.join(", ")));
    }
    sql
}

/// T-QT.6.11: the VDBE's hook for a hash join's probe. Random inner equi-joins of two tables of
/// many leaves, on one or two columns, with filters on either table and groupings and aggregates
/// over both tables' columns, answer the same, row for row and in order, with `ParallelMode` off
/// (the serial hash join) and on, the machine stated as 2, 3, 4 and 8 cores; and the hook ran for
/// most of them (the planner held to hash joins).
#[test]
fn t_qt_6_11_vdbe_hook_probes_a_hash_join_as_the_serial_loop() {
    // G7 step 2 (`AC-NS.2.1`): a test of the pager's leaf morsels.
    if crate::storage::versioned::switch::pager_only(
        "its subject is the morsel hook over the pager's leaf pages (that the workers ran); on the versioned storage the hook hands the statement to the table's column copy or the serial loop (G11 step 5, CS.5), so with parallel mode on it would compare the serial loop with itself",
    ) {
        return;
    }
    let conn = open(":memory:qt-6-11");
    let mut rng = ChaCha8Rng::seed_from_u64(611);
    fill(&conn, &mut rng, "t", 1500);
    fill(&conn, &mut rng, "u", 400);
    conn.set_join_methods(crate::JoinMethods::HashJoinsOnly);
    let queries = 40usize;
    let mut hooked = 0usize;
    for _ in 0..queries {
        let sql = random_join(&mut rng);
        conn.set_parallel_mode(ParallelMode::Off);
        let serial = sql_rows(&conn, &sql);
        conn.set_parallel_mode(ParallelMode::On);
        for cores in [2usize, 3, 4, 8] {
            *conn.morsel_costs.lock() = Some(MachineCosts::stated(
                NonZeroUsize::new(cores).unwrap(),
                1.0,
                1.0,
            ));
            assert_eq!(sql_rows(&conn, &sql), serial, "{sql} on {cores} cores");
            let ran = conn
                .morsel_costs
                .lock()
                .as_ref()
                .is_some_and(|costs| costs.worker_start().samples() > 1);
            if ran {
                hooked += 1;
            }
        }
    }
    conn.set_parallel_mode(ParallelMode::Off);
    conn.set_join_methods(crate::JoinMethods::ByCost);
    // Non-vacuity: most joins take the planner's hash join and the hook.
    assert!(
        hooked > queries * 2,
        "the hook ran {hooked} times of {}",
        queries * 4
    );
}

/// T-QT.6.12: a hash join's table built by the workers together, partitioned. The build side is
/// scanned over its own leaves at 1, 2, 3, 4 and 8 workers and filled into 1, 2, 3, 4 and 8
/// partitions; the probe's matches equal the serial VDBE join's, in order (a key's build rows in
/// build order whatever worker read them), and the runs were parallel and partitioned. A budget
/// too small for the table is a typed refusal, never a partial table.
#[test]
fn t_qt_6_12_partitioned_parallel_build_matches_the_serial_join() {
    // G7 step 2 (`AC-NS.2.1`): a test of the pager's leaves.
    if crate::storage::versioned::switch::pager_only(
        "it reads the pager's leaf pages at the table's root page (TableSource::open), which on the versioned storage is a tree's id, not a page; there a parallel aggregate is answered by the table's column copy or the serial loop (G11 step 5, CS.5)",
    ) {
        return;
    }
    let conn = open(":memory:qt-6-12");
    let mut rng = ChaCha8Rng::seed_from_u64(612);
    fill(&conn, &mut rng, "t", 1500);
    fill(&conn, &mut rng, "u", 1200);
    conn.execute("BEGIN").unwrap();
    let probe_table = source(&conn, "t");
    let build_table = source(&conn, "u");
    let mut parallel_scans = 0usize;
    let mut partitioned = 0usize;
    for (probe_key, build_key, filter) in [
        (vec![0usize], vec![0usize], vec![]),
        (
            vec![2, 0],
            vec![2, 0],
            vec![Predicate::NotNull { column: 1 }],
        ),
        (
            vec![2],
            vec![2],
            vec![Predicate::Compare {
                column: 0,
                op: CmpOp::Ge,
                value: Value::from_i64(-5),
            }],
        ),
    ] {
        let on: Vec<String> = probe_key
            .iter()
            .zip(&build_key)
            .map(|(p, b)| format!("t.{} = u.{}", COLUMNS[*p], COLUMNS[*b]))
            .collect();
        let mut sql = format!(
            "SELECT t.rowid, u.rowid FROM t JOIN u ON {}",
            on.join(" AND ")
        );
        for term in &filter {
            sql.push_str(match term {
                Predicate::NotNull { .. } => " AND u.b IS NOT NULL",
                _ => " AND u.a >= -5",
            });
        }
        sql.push_str(" ORDER BY t.rowid, u.rowid");
        let expected: Vec<(i64, i64)> = sql_rows(&conn, &sql)
            .into_iter()
            .map(|row| match (&row[0], &row[1]) {
                (
                    Value::Numeric(crate::numeric::Numeric::Integer(t)),
                    Value::Numeric(crate::numeric::Numeric::Integer(u)),
                ) => (*t, *u),
                other => panic!("not two rowids: {other:?}"),
            })
            .collect();
        assert!(!expected.is_empty(), "{sql}");
        // The rowid of each kept build row, read back from the build's own rows by the rowid
        // stored in no column: the test keeps `u`'s rows with their rowids beside them.
        let all = build_table.rows().unwrap();
        for partitions in [1usize, 2, 3, 4, 8] {
            let partitioner = Partitioner::new(NonZeroUsize::new(partitions).unwrap());
            for degree in degrees(build_table.leaves()) {
                let budget = MemoryBudget::new(usize::MAX);
                let build = TableBuild {
                    table: &build_table,
                    filter: &filter,
                    key: &build_key,
                    partitioner: &partitioner,
                    keep_unkeyed: false,
                };
                let (rows, table, stats) = build.run_at(degree, &budget).unwrap();
                if stats.scan.workers > 1 {
                    parallel_scans += 1;
                }
                if table.filled_partitions() > 1 {
                    partitioned += 1;
                }
                // Kept rows map back to rowids: rows are kept in rowid order, so walk `all`.
                let mut rowids = Vec::new();
                let mut next = all.iter();
                for row in &rows {
                    let (rowid, _) = next.by_ref().find(|(_, r)| r == row).unwrap();
                    rowids.push(*rowid);
                }
                let probe = HashProbe::over(table, &[], &probe_key);
                let pipeline = TableProbe {
                    table: &probe_table,
                    probe: &probe,
                };
                let (matches, _) = pipeline.run_at(degree, &budget).unwrap();
                let got: Vec<(i64, i64)> = matches
                    .into_iter()
                    .map(|(t, build)| (t, rowids[build]))
                    .collect();
                assert_eq!(
                    got, expected,
                    "{sql}: {partitions} partitions at {degree:?}"
                );
            }
        }
        // A budget the table can't fit in is refused by type, at every degree.
        for degree in degrees(build_table.leaves()) {
            let partitioner = Partitioner::new(NonZeroUsize::new(4).unwrap());
            let budget = MemoryBudget::new(4096);
            let build = TableBuild {
                table: &build_table,
                filter: &filter,
                key: &build_key,
                partitioner: &partitioner,
                keep_unkeyed: false,
            };
            assert!(
                matches!(
                    build.run_at(degree, &budget),
                    Err(MorselError::MemoryBudget { .. })
                ),
                "{sql} at {degree:?}"
            );
        }
    }
    conn.execute("COMMIT").unwrap();
    // Non-vacuity: most builds ran on several workers and filled several partitions.
    assert!(parallel_scans > 100, "{parallel_scans}");
    assert!(partitioned > 150, "{partitioned}");
}

/// A random inner equi-join of `t` and `u` returning rows: result columns of either table, filters
/// on either, and half the time an ORDER BY over the first one or two result columns, ascending or
/// descending.
fn random_join_rows(rng: &mut ChaCha8Rng) -> String {
    let tables = ["t", "u"];
    let ops = ["<", "<=", "=", "<>", ">", ">="];
    let keys: &[&str] = match rng.random_range(0..3) {
        0 => &["a"],
        1 => &["c"],
        _ => &["a", "c"],
    };
    let on: Vec<String> = keys.iter().map(|k| format!("t.{k} = u.{k}")).collect();
    let mut terms = Vec::new();
    for _ in 0..rng.random_range(0..3) {
        let table = tables[rng.random_range(0..2)];
        let op = ops[rng.random_range(0..ops.len())];
        terms.push(match rng.random_range(0..4) {
            0 => format!("{table}.a {op} {}", rng.random_range(-20i64..20)),
            1 => format!("{table}.c {op} 't{}'", rng.random_range(0..12)),
            2 => format!("{table}.c IS NULL"),
            _ => format!("{table}.a IS NOT NULL"),
        });
    }
    let column = |rng: &mut ChaCha8Rng| {
        format!(
            "{}.{}",
            tables[rng.random_range(0..2)],
            ["a", "b", "c", "p"][rng.random_range(0..4)]
        )
    };
    let select: Vec<String> = (0..rng.random_range(1..5)).map(|_| column(rng)).collect();
    let mut sql = format!(
        "SELECT {} FROM t JOIN u ON {}",
        select.join(", "),
        on.join(" AND ")
    );
    if !terms.is_empty() {
        sql.push_str(&format!(" WHERE {}", terms.join(" AND ")));
    }
    if rng.random_bool(0.5) {
        // The ORDER BY keys lead the select list, so a tie's rows can be told apart below.
        let keys = rng.random_range(1..=select.len().min(2));
        let order: Vec<String> = select[..keys]
            .iter()
            .map(|key| {
                let dir = if rng.random_bool(0.5) { " DESC" } else { "" };
                format!("{key}{dir}")
            })
            .collect();
        sql.push_str(&format!(" ORDER BY {} -- keys {keys}", order.join(", ")));
    }
    sql
}

/// How many ORDER BY keys lead `sql`'s select list, read from the comment `random_join_rows` ends it with.
fn order_keys(sql: &str) -> Option<usize> {
    let (_, keys) = sql.split_once("-- keys ")?;
    keys.trim().parse().ok()
}

/// T-QT.6.13: joins returning rows, no aggregates, run on morsels. Random inner equi-joins of two
/// tables of many leaves, with filters on either table and an ORDER BY or none, answer the same
/// with `ParallelMode` off (the serial hash join) and on, the machine stated as 2, 3, 4 and 8
/// cores: in order with an ORDER BY (the workers' sorted runs merged), as the same rows without
/// one; and the hook ran for most of them.
#[test]
fn t_qt_6_13_vdbe_hook_returns_a_hash_joins_rows_as_the_serial_loop() {
    // G7 step 2 (`AC-NS.2.1`): a test of the pager's leaf morsels.
    if crate::storage::versioned::switch::pager_only(
        "its subject is the morsel hook over the pager's leaf pages (that the workers ran); on the versioned storage the hook hands the statement to the table's column copy or the serial loop (G11 step 5, CS.5), so with parallel mode on it would compare the serial loop with itself",
    ) {
        return;
    }
    let conn = open(":memory:qt-6-13");
    let mut rng = ChaCha8Rng::seed_from_u64(613);
    fill(&conn, &mut rng, "t", 1500);
    fill(&conn, &mut rng, "u", 400);
    conn.set_join_methods(crate::JoinMethods::HashJoinsOnly);
    let queries = 40usize;
    let mut hooked = 0usize;
    let mut ordered = 0usize;
    for _ in 0..queries {
        let sql = random_join_rows(&mut rng);
        let has_order = order_keys(&sql).is_some();
        conn.set_parallel_mode(ParallelMode::Off);
        let serial = sql_rows(&conn, &sql);
        conn.set_parallel_mode(ParallelMode::On);
        for cores in [2usize, 3, 4, 8] {
            *conn.morsel_costs.lock() = Some(MachineCosts::stated(
                NonZeroUsize::new(cores).unwrap(),
                1.0,
                1.0,
            ));
            let parallel = sql_rows(&conn, &sql);
            if let Some(keys) = order_keys(&sql) {
                // In ORDER BY order, row for row in the keys; rows tied on them may come in
                // any order, so each run of ties is compared as a set.
                let key = |row: &Vec<Value>| format!("{:?}", &row[..keys]);
                let serial_keys: Vec<String> = serial.iter().map(key).collect();
                let parallel_keys: Vec<String> = parallel.iter().map(key).collect();
                assert_eq!(parallel_keys, serial_keys, "{sql} on {cores} cores");
                assert_eq!(
                    canonical(&parallel),
                    canonical(&serial),
                    "{sql} on {cores} cores"
                );
            } else {
                assert_eq!(
                    canonical(&parallel),
                    canonical(&serial),
                    "{sql} on {cores} cores"
                );
            }
            let ran = conn
                .morsel_costs
                .lock()
                .as_ref()
                .is_some_and(|costs| costs.worker_start().samples() > 1);
            if ran {
                hooked += 1;
                if has_order {
                    ordered += 1;
                }
            }
        }
    }
    conn.set_parallel_mode(ParallelMode::Off);
    conn.set_join_methods(crate::JoinMethods::ByCost);
    // Non-vacuity: most joins took the hook, ordered ones among them.
    assert!(
        hooked > queries * 2,
        "the hook ran {hooked} times of {}",
        queries * 4
    );
    assert!(ordered > 0, "no ordered join took the hook");
}

/// T-QT.6.14 (lane qs6-6): a hash join's rows under LIMIT and OFFSET run on morsels when the ORDER
/// BY tells every joined row apart (both tables' INTEGER PRIMARY KEYs among its keys) and answer
/// row for row as the serial loop, the machine stated as 2, 3, 4 and 8 cores; a LIMIT over an
/// order with ties stays serial.
#[test]
fn t_qt_6_14_vdbe_hook_limits_a_hash_joins_rows_as_the_serial_loop() {
    // G7 step 2 (`AC-NS.2.1`): a test of the pager's leaf morsels.
    if crate::storage::versioned::switch::pager_only(
        "its subject is the morsel hook over the pager's leaf pages (that the workers ran); on the versioned storage the hook hands the statement to the table's column copy or the serial loop (G11 step 5, CS.5), so with parallel mode on it would compare the serial loop with itself",
    ) {
        return;
    }
    let conn = open(":memory:qt-6-14");
    let mut rng = ChaCha8Rng::seed_from_u64(614);
    let pad = "x".repeat(64);
    for (name, rows) in [("t", 1500usize), ("u", 400)] {
        conn.execute(format!(
            "CREATE TABLE {name}(id INTEGER PRIMARY KEY, a INTEGER, c TEXT, p TEXT)"
        ))
        .unwrap();
        conn.execute("BEGIN").unwrap();
        for _ in 0..rows {
            let a = rng.random_range(-20i64..20);
            let c = rng.random_range(0..12);
            conn.execute(format!(
                "INSERT INTO {name}(a, c, p) VALUES ({a}, 't{c}', '{pad}')"
            ))
            .unwrap();
        }
        conn.execute("COMMIT").unwrap();
    }
    conn.set_join_methods(crate::JoinMethods::HashJoinsOnly);
    let run = |sql: &str, cores: usize| {
        *conn.morsel_costs.lock() = Some(MachineCosts::stated(
            NonZeroUsize::new(cores).unwrap(),
            1.0,
            1.0,
        ));
        let rows = sql_rows(&conn, sql);
        let ran = conn
            .morsel_costs
            .lock()
            .as_ref()
            .is_some_and(|costs| costs.worker_start().samples() > 1);
        (rows, ran)
    };
    let queries = 30usize;
    let mut hooked = 0usize;
    for _ in 0..queries {
        let dir = |rng: &mut ChaCha8Rng| if rng.random_bool(0.5) { " DESC" } else { "" };
        let lead = ["", "t.a, ", "u.c DESC, "][rng.random_range(0..3)];
        let (dt, du) = (dir(&mut rng), dir(&mut rng));
        let order = if rng.random_bool(0.5) {
            format!("{lead}t.id{dt}, u.id{du}")
        } else {
            format!("{lead}u.id{du}, t.id{dt}")
        };
        let limit = if rng.random_range(0..6) == 0 {
            -1
        } else {
            rng.random_range(1i64..200)
        };
        let offset = match rng.random_range(0..3) {
            0 => String::new(),
            1 => format!(" OFFSET {}", rng.random_range(0..300)),
            _ => " OFFSET -3".to_string(),
        };
        let filter = if rng.random_bool(0.5) {
            format!(" WHERE t.a > {}", rng.random_range(-20i64..20))
        } else {
            String::new()
        };
        let sql = format!(
            "SELECT t.id, u.id, t.c, u.a FROM t JOIN u ON t.a = u.a{filter} ORDER BY {order} LIMIT {limit}{offset}"
        );
        conn.set_parallel_mode(ParallelMode::Off);
        let serial = sql_rows(&conn, &sql);
        conn.set_parallel_mode(ParallelMode::On);
        for cores in [2usize, 3, 4, 8] {
            let (parallel, ran) = run(&sql, cores);
            assert_eq!(parallel, serial, "{sql} on {cores} cores");
            if ran {
                hooked += 1;
            }
        }
    }
    // An order with ties under a LIMIT could cut a different tie: the hook declines it.
    let tied = "SELECT t.a, u.c FROM t JOIN u ON t.a = u.a ORDER BY t.a LIMIT 10";
    let (_, ran) = run(tied, 4);
    assert!(!ran, "{tied} took the hook");
    conn.set_parallel_mode(ParallelMode::Off);
    conn.set_join_methods(crate::JoinMethods::ByCost);
    // Non-vacuity: most limited joins took the hook.
    assert!(
        hooked > queries * 2,
        "the hook ran {hooked} times of {}",
        queries * 4
    );
}

/// T-QT.6.15 (lane qs6-7): a one-table scan's aggregates whose filters and inputs are any
/// expression (functions, CASE, LIKE, arithmetic) run on morsels through a copy of the serial
/// loop's body, each worker with its own registers, and answer as the serial loop at 1, 2, 3, 4
/// and 8 stated cores; the body ran over every row of the table each time.
#[test]
fn t_qt_6_15_workers_run_a_copy_of_the_loop_body_as_the_serial_loop() {
    // G7 step 2 (`AC-NS.2.1`): a test of the pager's leaf morsels.
    if crate::storage::versioned::switch::pager_only(
        "its subject is the morsel hook over the pager's leaf pages (that the workers ran); on the versioned storage the hook hands the statement to the table's column copy or the serial loop (G11 step 5, CS.5), so with parallel mode on it would compare the serial loop with itself",
    ) {
        return;
    }
    let conn = open(":memory:qt-6-15");
    let mut rng = ChaCha8Rng::seed_from_u64(615);
    let pad = "x".repeat(64);
    let rows = 2500u64;
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, a INTEGER, b INTEGER, c TEXT, p TEXT)")
        .unwrap();
    conn.execute("BEGIN").unwrap();
    for _ in 0..rows {
        let a = rng.random_range(-50i64..50);
        let b = if rng.random_range(0..5) == 0 {
            "NULL".to_string()
        } else {
            rng.random_range(-9i64..9).to_string()
        };
        let c = match rng.random_range(0..8) {
            0 => "NULL".to_string(),
            k => format!("'t{k}{}'", ["", "a", "Ab", "_%"][rng.random_range(0..4)]),
        };
        conn.execute(format!(
            "INSERT INTO t(a, b, c, p) VALUES ({a}, {b}, {c}, '{pad}')"
        ))
        .unwrap();
    }
    conn.execute("COMMIT").unwrap();
    let filters = [
        "a > {n}",
        "abs(a) < {n}",
        "a % 3 = {k}",
        "c LIKE 't{k}%'",
        "c NOT LIKE '%a'",
        "CASE WHEN a > 0 THEN b ELSE -b END > {k}",
        "length(c) = 3",
        "a + b * 2 > {n}",
        "coalesce(b, 0) <> {k}",
        "upper(c) = 'T{k}AB'",
        "a BETWEEN -{k} AND {n}",
        "c IN ('t1', 't2a', 't3Ab')",
        "b IS NULL OR a < {n}",
        "substr(c, 2, 1) = '{k}'",
        "id % 7 = {k}",
        "max(a, b) > {k}",
        "iif(c IS NULL, 1, 0) = 0",
    ];
    let aggregates = [
        "COUNT(*)",
        "COUNT(b)",
        "SUM(a)",
        "SUM(a * 2 + b)",
        "SUM(CASE WHEN c LIKE '%a' THEN 1 ELSE 0 END)",
        "MIN(c)",
        "MAX(a - b)",
        "MIN(length(c))",
        "MAX(CASE WHEN a > 0 THEN c ELSE 'z' END)",
        "COUNT(CASE WHEN c LIKE '%1' THEN 1 END)",
        "MAX(lower(c) || id)",
        "MIN(abs(a) + 1)",
    ];
    let groups = ["", "a % 4", "c", "lower(c)", "b IS NULL", "abs(b)"];
    let fill = |template: &str, rng: &mut ChaCha8Rng| {
        template
            .replace("{n}", &rng.random_range(-40i64..40).to_string())
            .replace("{k}", &rng.random_range(0i64..4).to_string())
    };
    let queries = 40usize;
    let mut hooked = 0usize;
    for _ in 0..queries {
        let mut terms = Vec::new();
        for _ in 0..rng.random_range(0..3) {
            terms.push(fill(filters[rng.random_range(0..filters.len())], &mut rng));
        }
        let mut outs = Vec::new();
        for _ in 0..rng.random_range(1..4) {
            outs.push(aggregates[rng.random_range(0..aggregates.len())].to_string());
        }
        let group = groups[rng.random_range(0..groups.len())];
        let filter = if terms.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", terms.join(" AND "))
        };
        let (lead, grouped) = if group.is_empty() {
            (String::new(), String::new())
        } else {
            (format!("{group}, "), format!(" GROUP BY {group}"))
        };
        let sql = format!("SELECT {lead}{} FROM t{filter}{grouped}", outs.join(", "));
        conn.set_parallel_mode(ParallelMode::Off);
        let serial = sql_rows(&conn, &sql);
        conn.set_parallel_mode(ParallelMode::On);
        for cores in [1usize, 2, 3, 4, 8] {
            *conn.morsel_costs.lock() = Some(MachineCosts::stated(
                NonZeroUsize::new(cores).unwrap(),
                1.0,
                1.0,
            ));
            let before = conn
                .morsel_body_rows
                .load(std::sync::atomic::Ordering::Relaxed);
            let parallel = sql_rows(&conn, &sql);
            let after = conn
                .morsel_body_rows
                .load(std::sync::atomic::Ordering::Relaxed);
            assert_eq!(parallel, serial, "{sql} on {cores} cores");
            // The copied body ran over every row once: the parallel path answered.
            assert_eq!(
                after - before,
                rows,
                "{sql} on {cores} cores: the body's rows"
            );
            let workers = conn
                .morsel_costs
                .lock()
                .as_ref()
                .is_some_and(|costs| costs.worker_start().samples() > 1);
            if workers {
                hooked += 1;
            }
        }
    }
    conn.set_parallel_mode(ParallelMode::Off);
    // Non-vacuity: more than one worker ran the body for most statements.
    assert!(
        hooked > queries,
        "workers ran {hooked} times of {}",
        queries * 5
    );
}

/// T-QT.6.16 (lane qs6-8): a one-table scan's rows, any filter and any result expression, run on
/// morsels through the copy of the serial loop's body and come out as the serial loop's at 1, 2,
/// 3, 4 and 8 stated cores. Under a LIMIT with no ORDER BY, behind its switch (off, the control:
/// the serial loop answers and the body never runs), the row count is the serial one, every row
/// is in the serial result (here the very rows in order, as the morsels run are a prefix of the
/// table), and the workers stopped early: the body ran over fewer rows than the table holds.
#[test]
fn t_qt_6_16_a_scans_rows_run_in_parallel_and_stop_early_under_limit() {
    // G7 step 2 (`AC-NS.2.1`): a test of the pager's leaf morsels.
    if crate::storage::versioned::switch::pager_only(
        "its subject is the morsel hook over the pager's leaf pages (that the workers ran); on the versioned storage the hook hands the statement to the table's column copy or the serial loop (G11 step 5, CS.5), so with parallel mode on it would compare the serial loop with itself",
    ) {
        return;
    }
    let conn = open(":memory:qt-6-16");
    let mut rng = ChaCha8Rng::seed_from_u64(616);
    let pad = "x".repeat(64);
    let rows = 2500u64;
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, a INTEGER, b INTEGER, c TEXT, p TEXT)")
        .unwrap();
    conn.execute("BEGIN").unwrap();
    for _ in 0..rows {
        let a = rng.random_range(-50i64..50);
        let b = if rng.random_range(0..5) == 0 {
            "NULL".to_string()
        } else {
            rng.random_range(-9i64..9).to_string()
        };
        let c = match rng.random_range(0..8) {
            0 => "NULL".to_string(),
            k => format!("'t{k}{}'", ["", "a", "Ab", "_%"][rng.random_range(0..4)]),
        };
        conn.execute(format!(
            "INSERT INTO t(a, b, c, p) VALUES ({a}, {b}, {c}, '{pad}')"
        ))
        .unwrap();
    }
    conn.execute("COMMIT").unwrap();
    let filters = [
        "a > {n}",
        "abs(a) < {n}",
        "a % 3 = {k}",
        "c LIKE 't{k}%'",
        "c NOT LIKE '%a'",
        "CASE WHEN a > 0 THEN b ELSE -b END > {k}",
        "a + b * 2 > {n}",
        "coalesce(b, 0) <> {k}",
        "b IS NULL OR a < {n}",
        "id % 7 = {k}",
        "iif(c IS NULL, 1, 0) = 0",
    ];
    let outputs = [
        "id",
        "a",
        "c",
        "a * 2 + b",
        "upper(c)",
        "CASE WHEN a > 0 THEN c ELSE 'n' END",
        "length(p)",
        "coalesce(b, -1)",
        "id || c",
    ];
    let fill = |template: &str, rng: &mut ChaCha8Rng| {
        template
            .replace("{n}", &rng.random_range(-40i64..40).to_string())
            .replace("{k}", &rng.random_range(0i64..4).to_string())
    };
    let body_rows = || {
        conn.morsel_body_rows
            .load(std::sync::atomic::Ordering::Relaxed)
    };
    let stated = |cores: usize| {
        *conn.morsel_costs.lock() = Some(MachineCosts::stated(
            NonZeroUsize::new(cores).unwrap(),
            1.0,
            1.0,
        ));
    };
    let queries = 30usize;
    let mut hooked = 0usize;
    let mut limited = 0usize;
    let mut stopped = 0usize;
    for query in 0..queries {
        let mut terms = Vec::new();
        for _ in 0..rng.random_range(0..3) {
            terms.push(fill(filters[rng.random_range(0..filters.len())], &mut rng));
        }
        let mut outs = Vec::new();
        for _ in 0..rng.random_range(1..4) {
            outs.push(outputs[rng.random_range(0..outputs.len())]);
        }
        let filter = if terms.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", terms.join(" AND "))
        };
        let limit = match query % 3 {
            0 => String::new(),
            1 => format!(" LIMIT {}", rng.random_range(1..60)),
            _ => format!(
                " LIMIT {} OFFSET {}",
                rng.random_range(1..60),
                rng.random_range(0..20)
            ),
        };
        let sql = format!("SELECT {} FROM t{filter}{limit}", outs.join(", "));
        conn.set_parallel_mode(ParallelMode::Off);
        let serial = sql_rows(&conn, &sql);
        conn.set_parallel_mode(ParallelMode::On);
        if !limit.is_empty() {
            // The switch's control: the serial loop answers, the body never runs.
            conn.set_parallel_limit(false);
            stated(4);
            let before = body_rows();
            assert_eq!(sql_rows(&conn, &sql), serial, "{sql}: the control");
            assert_eq!(body_rows(), before, "{sql}: the control ran the body");
            conn.set_parallel_limit(true);
        }
        for cores in [1usize, 2, 3, 4, 8] {
            stated(cores);
            let before = body_rows();
            let parallel = sql_rows(&conn, &sql);
            let ran = body_rows() - before;
            assert_eq!(parallel.len(), serial.len(), "{sql} on {cores} cores: rows");
            for row in &parallel {
                assert!(serial.contains(row), "{sql} on {cores} cores: {row:?}");
            }
            assert_eq!(parallel, serial, "{sql} on {cores} cores");
            if limit.is_empty() {
                assert_eq!(ran, rows, "{sql} on {cores} cores: the body's rows");
            } else {
                assert!(ran > 0 && ran <= rows, "{sql} on {cores} cores: ran {ran}");
                limited += 1;
                if ran < rows {
                    stopped += 1;
                }
            }
            let workers = conn
                .morsel_costs
                .lock()
                .as_ref()
                .is_some_and(|costs| costs.worker_start().samples() > 1);
            if workers {
                hooked += 1;
            }
        }
        conn.set_parallel_limit(false);
    }
    conn.set_parallel_mode(ParallelMode::Off);
    // Non-vacuity: more than one worker ran for most statements, and most LIMITs stopped early.
    assert!(
        hooked > queries,
        "workers ran {hooked} times of {}",
        queries * 5
    );
    assert!(
        stopped * 2 > limited,
        "the workers stopped early {stopped} times of {limited}"
    );
}

/// A random inner equi-join of `tables` (lane qs6-10): each table after the first joined to a
/// random one before it (a chain, a star or between), on `a` or on `a` and `c`; filters on any
/// table; aggregates over a grouping, or the rows with an ORDER BY or none. Says whether it
/// returns rows.
fn random_chain(rng: &mut ChaCha8Rng, tables: &[&str]) -> (String, bool) {
    let ops = ["<", "<=", "=", "<>", ">", ">="];
    let mut terms = Vec::new();
    for (at, table) in tables.iter().enumerate().skip(1) {
        let other = tables[rng.random_range(0..at)];
        let keys: &[&str] = if rng.random_bool(0.5) {
            &["a"]
        } else {
            &["a", "c"]
        };
        for key in keys {
            terms.push(format!("{other}.{key} = {table}.{key}"));
        }
    }
    for _ in 0..rng.random_range(0..3) {
        let table = tables[rng.random_range(0..tables.len())];
        let op = ops[rng.random_range(0..ops.len())];
        terms.push(match rng.random_range(0..4) {
            0 => format!("{table}.a {op} {}", rng.random_range(-20i64..20)),
            1 => format!("{table}.c {op} 't{}'", rng.random_range(0..12)),
            2 => format!("{table}.c IS NULL"),
            _ => format!("{table}.a IS NOT NULL"),
        });
    }
    let column = |rng: &mut ChaCha8Rng| {
        format!(
            "{}.{}",
            tables[rng.random_range(0..tables.len())],
            ["a", "b", "c"][rng.random_range(0..3)]
        )
    };
    let rows = rng.random_bool(0.4);
    let mut group_by = Vec::new();
    let mut select: Vec<String>;
    if rows {
        select = (0..rng.random_range(1..5)).map(|_| column(rng)).collect();
    } else {
        group_by = (0..rng.random_range(0..3)).map(|_| column(rng)).collect();
        select = group_by.clone();
        for _ in 0..rng.random_range(1..4) {
            select.push(match rng.random_range(0..5) {
                0 => "count(*)".to_string(),
                1 => format!("count({})", column(rng)),
                2 => format!(
                    "sum({}.{})",
                    tables[rng.random_range(0..tables.len())],
                    ["a", "b"][rng.random_range(0..2)]
                ),
                3 => format!("min({})", column(rng)),
                _ => format!("max({})", column(rng)),
            });
        }
    }
    let mut sql = format!(
        "SELECT {} FROM {} WHERE {}",
        select.join(", "),
        tables.join(", "),
        terms.join(" AND ")
    );
    if !group_by.is_empty() {
        sql.push_str(&format!(" GROUP BY {}", group_by.join(", ")));
    }
    if rows && rng.random_bool(0.5) {
        // The ORDER BY keys lead the select list, so a tie's rows can be told apart below.
        let keys = rng.random_range(1..=select.len().min(2));
        let order: Vec<String> = select[..keys]
            .iter()
            .map(|key| {
                let dir = if rng.random_bool(0.5) { " DESC" } else { "" };
                format!("{key}{dir}")
            })
            .collect();
        sql.push_str(&format!(" ORDER BY {} -- keys {keys}", order.join(", ")));
    }
    (sql, rows)
}

/// T-QT.6.17 (lane qs6-10): joins of three and four tables run on morsels through a chain of hash
/// tables. Random inner equi-joins (chains, stars and between, on one or two columns, with filters
/// on any table), with aggregates or returning rows, answer the same with `ParallelMode` off (the
/// serial loop) and on, the machine stated as 1, 2, 3, 4 and 8 cores: row for row with
/// aggregates, in ORDER BY order with ties compared as sets, as the same rows without one. The
/// hook's counter of hash tables built shows each run that took it built one link per table past
/// the probe, and most runs took it.
#[test]
fn t_qt_6_17_joins_of_three_and_four_tables_probe_a_chain_of_hash_tables() {
    let conn = open(":memory:qt-6-17");
    let mut rng = ChaCha8Rng::seed_from_u64(617);
    fill(&conn, &mut rng, "t", 600);
    fill(&conn, &mut rng, "u", 120);
    fill(&conn, &mut rng, "v", 50);
    fill(&conn, &mut rng, "w", 30);
    conn.set_join_methods(crate::JoinMethods::HashJoinsOnly);
    let queries = 30usize;
    let mut chained = 0usize;
    let mut chained_rows = 0usize;
    for query in 0..queries {
        let tables: &[&str] = if query % 2 == 0 {
            &["t", "u", "v"]
        } else {
            &["t", "u", "v", "w"]
        };
        let (sql, rows) = random_chain(&mut rng, tables);
        conn.set_parallel_mode(ParallelMode::Off);
        let serial = sql_rows(&conn, &sql);
        conn.set_parallel_mode(ParallelMode::On);
        for cores in [1usize, 2, 3, 4, 8] {
            *conn.morsel_costs.lock() = Some(MachineCosts::stated(
                NonZeroUsize::new(cores).unwrap(),
                1.0,
                1.0,
            ));
            let before = conn
                .morsel_join_links
                .load(std::sync::atomic::Ordering::Relaxed);
            let parallel = sql_rows(&conn, &sql);
            let links = conn
                .morsel_join_links
                .load(std::sync::atomic::Ordering::Relaxed)
                - before;
            if !rows {
                assert_eq!(parallel, serial, "{sql} on {cores} cores");
            } else {
                if let Some(keys) = order_keys(&sql) {
                    let key = |row: &Vec<Value>| format!("{:?}", &row[..keys]);
                    let serial_keys: Vec<String> = serial.iter().map(key).collect();
                    let parallel_keys: Vec<String> = parallel.iter().map(key).collect();
                    assert_eq!(parallel_keys, serial_keys, "{sql} on {cores} cores");
                }
                assert_eq!(
                    canonical(&parallel),
                    canonical(&serial),
                    "{sql} on {cores} cores"
                );
            }
            // A run either took the hook, building a link for every table past the probe, or
            // left the statement to the serial loop before building any.
            let expected = u64::try_from(tables.len() - 1).unwrap();
            assert!(
                links == 0 || links == expected,
                "{sql} on {cores} cores built {links} links"
            );
            if links == expected {
                chained += 1;
                if rows {
                    chained_rows += 1;
                }
            }
        }
    }
    conn.set_parallel_mode(ParallelMode::Off);
    conn.set_join_methods(crate::JoinMethods::ByCost);
    // Non-vacuity: most runs took the chain, row-returning ones among them.
    assert!(
        chained > queries * 5 / 2,
        "the chain ran {chained} times of {}",
        queries * 5
    );
    assert!(chained_rows > 0, "no row-returning join took the chain");
}

/// Live test of record for T-QT.6.16's switch (coordinator, 2026-11-22): a large table, the
/// machine's own measured costs (not stated cores), many random LIMIT/OFFSET queries with no
/// ORDER BY. Each limited answer, parallel, must have the serial count and be a sub-multiset of
/// the full serial answer; latency is reported per mode (serial, switch off, switch on).
/// Sizes: QT616_ROWS, QT616_QUERIES (env). Run: --release --ignored --nocapture.
#[test]
#[ignore]
fn t_qt_6_16_live_parallel_limit_matches_serial_at_scale() {
    use std::collections::HashMap;
    use std::time::Instant;
    let rows: u64 = std::env::var("QT616_ROWS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200_000);
    let queries: usize = std::env::var("QT616_QUERIES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);
    let conn = open(":memory:qt-6-16-live");
    let mut rng = ChaCha8Rng::seed_from_u64(61616);
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, a INTEGER, b INTEGER, c TEXT, p TEXT)")
        .unwrap();
    conn.execute("BEGIN").unwrap();
    let pad = "x".repeat(64);
    for _ in 0..rows {
        let a = rng.random_range(-500i64..500);
        let b = if rng.random_range(0..5) == 0 {
            "NULL".to_string()
        } else {
            rng.random_range(-9i64..9).to_string()
        };
        let c = match rng.random_range(0..8) {
            0 => "NULL".to_string(),
            k => format!("'t{k}{}'", ["", "a", "Ab", "_%"][rng.random_range(0..4)]),
        };
        conn.execute(format!(
            "INSERT INTO t(a, b, c, p) VALUES ({a}, {b}, {c}, '{pad}')"
        ))
        .unwrap();
    }
    conn.execute("COMMIT").unwrap();
    let filters = [
        "a > {n}",
        "abs(a) < {n}",
        "a % 3 = {k}",
        "c LIKE 't{k}%'",
        "a + b * 2 > {n}",
        "coalesce(b, 0) <> {k}",
        "id % 7 = {k}",
    ];
    let outputs = ["id", "a", "c", "a * 2 + b", "upper(c)", "coalesce(b, -1)"];
    let fill = |t: &str, rng: &mut ChaCha8Rng| {
        t.replace("{n}", &rng.random_range(-400i64..400).to_string())
            .replace("{k}", &rng.random_range(0i64..4).to_string())
    };
    let (mut t_serial, mut t_off, mut t_on) = (Vec::new(), Vec::new(), Vec::new());
    let mut ran_parallel = 0usize;
    for _ in 0..queries {
        let mut terms = Vec::new();
        for _ in 0..rng.random_range(0..3) {
            terms.push(fill(filters[rng.random_range(0..filters.len())], &mut rng));
        }
        let mut outs = Vec::new();
        for _ in 0..rng.random_range(1..4) {
            outs.push(outputs[rng.random_range(0..outputs.len())]);
        }
        let filter = if terms.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", terms.join(" AND "))
        };
        let (lim, off) = (rng.random_range(1i64..2000), rng.random_range(0i64..500));
        let base = format!("SELECT {} FROM t{filter}", outs.join(", "));
        let sql = format!("{base} LIMIT {lim} OFFSET {off}");
        conn.set_parallel_mode(ParallelMode::Off);
        let full = sql_rows(&conn, &base);
        let s0 = Instant::now();
        let serial = sql_rows(&conn, &sql);
        t_serial.push(s0.elapsed());
        conn.set_parallel_mode(ParallelMode::On);
        conn.set_parallel_limit(false);
        let s1 = Instant::now();
        let control = sql_rows(&conn, &sql);
        t_off.push(s1.elapsed());
        assert_eq!(control, serial, "{sql}: switch off differs from serial");
        conn.set_parallel_limit(true);
        let before = conn
            .morsel_body_rows
            .load(std::sync::atomic::Ordering::Relaxed);
        let s2 = Instant::now();
        let parallel = sql_rows(&conn, &sql);
        t_on.push(s2.elapsed());
        if conn
            .morsel_body_rows
            .load(std::sync::atomic::Ordering::Relaxed)
            > before
        {
            ran_parallel += 1;
        }
        let want = (full.len() as i64 - off).clamp(0, lim) as usize;
        assert_eq!(parallel.len(), want, "{sql}: row count");
        let mut pool: HashMap<String, usize> = HashMap::new();
        for r in &full {
            *pool.entry(format!("{r:?}")).or_default() += 1;
        }
        for r in &parallel {
            let k = format!("{r:?}");
            let n = pool
                .get_mut(&k)
                .unwrap_or_else(|| panic!("{sql}: row {k} not in the serial result"));
            assert!(
                *n > 0,
                "{sql}: row {k} returned more times than the serial result holds it"
            );
            *n -= 1;
        }
    }
    let pct = |v: &mut Vec<std::time::Duration>, q: f64| {
        v.sort();
        v[((v.len() as f64 - 1.0) * q).round() as usize]
    };
    for (name, v) in [
        ("serial", &mut t_serial),
        ("switch off", &mut t_off),
        ("switch on", &mut t_on),
    ] {
        eprintln!(
            "QT616 {name}: p50 {:?} p99 {:?} p99.9 {:?}",
            pct(v, 0.5),
            pct(v, 0.99),
            pct(v, 0.999)
        );
    }
    eprintln!("QT616 rows {rows} queries {queries} ran_parallel {ran_parallel}");
}

/// A random inner equi-join of `tables` whose filters and outputs only the loop body states (lane
/// qs6-12): each table after the first joined to a random one before it on `a`, or on `a` and
/// `c`; filters that are functions, arithmetic, CASE, LIKE, or compare two tables' columns; and
/// aggregates over grouping expressions, or result expressions with an ORDER BY of plain columns
/// or none. A MAX over a REAL among them sends the statement to the serial loop. Says whether it
/// returns rows.
fn random_body_chain(rng: &mut ChaCha8Rng, tables: &[&str]) -> (String, bool) {
    let mut terms = Vec::new();
    for (at, table) in tables.iter().enumerate().skip(1) {
        let other = tables[rng.random_range(0..at)];
        let keys: &[&str] = if rng.random_bool(0.5) {
            &["a"]
        } else {
            &["a", "c"]
        };
        for key in keys {
            terms.push(format!("{other}.{key} = {table}.{key}"));
        }
    }
    let pick = |rng: &mut ChaCha8Rng| tables[rng.random_range(0..tables.len())];
    for _ in 0..rng.random_range(0..3) {
        let (t, u) = (pick(rng), pick(rng));
        let n = rng.random_range(-20i64..20);
        let k = rng.random_range(0i64..3);
        terms.push(match rng.random_range(0..9) {
            0 => format!("abs({t}.a) < {}", n.abs()),
            1 => format!("{t}.c LIKE 't{k}%'"),
            2 => format!("{t}.a + {u}.a > {n}"),
            3 => format!("coalesce({t}.b, 0) <> {k}"),
            4 => format!("length({t}.c) = 2"),
            5 => format!("CASE WHEN {t}.a > 0 THEN 1 ELSE 0 END = {}", k % 2),
            6 => format!("{t}.a % 3 = {k}"),
            7 => format!("{t}.a < {u}.a"),
            _ => format!("({t}.b IS NULL OR {t}.a > {n})"),
        });
    }
    let rows = rng.random_bool(0.4);
    let mut select: Vec<String> = Vec::new();
    let mut group_by: Vec<String> = Vec::new();
    let mut keys = 0usize;
    let ordered = rows && rng.random_bool(0.5);
    if rows {
        if ordered {
            // Plain-column ORDER BY keys lead the select list, so a tie's rows can be told apart.
            keys = rng.random_range(1..=2usize);
            for _ in 0..keys {
                select.push(format!(
                    "{}.{}",
                    pick(rng),
                    ["a", "c"][rng.random_range(0..2)]
                ));
            }
        }
        for _ in 0..rng.random_range(1..4) {
            let (t, u) = (pick(rng), pick(rng));
            select.push(match rng.random_range(0..5) {
                0 => format!("{t}.a + {u}.a"),
                1 => format!("upper({t}.c)"),
                2 => format!("{t}.c || '-' || {u}.c"),
                3 => format!("CASE WHEN {t}.a > {u}.a THEN {t}.c ELSE {u}.c END"),
                _ => format!("{t}.b"),
            });
        }
    } else {
        for _ in 0..rng.random_range(0..3) {
            let t = pick(rng);
            group_by.push(match rng.random_range(0..4) {
                0 => format!("{t}.a % 4"),
                1 => format!("lower({t}.c)"),
                2 => format!("{t}.b IS NULL"),
                _ => format!("{t}.c"),
            });
        }
        select = group_by.clone();
        for _ in 0..rng.random_range(1..4) {
            let (t, u) = (pick(rng), pick(rng));
            select.push(match rng.random_range(0..8) {
                0 => "count(*)".to_string(),
                1 => format!("sum({t}.a + {u}.a)"),
                2 => format!("sum(abs({t}.b) % 1000)"),
                3 => format!("min({t}.c || {u}.c)"),
                4 => format!("max(length({t}.c))"),
                5 => format!("count(CASE WHEN {t}.a > {u}.a THEN 1 END)"),
                6 => format!("max({t}.a - {u}.a)"),
                _ => format!("max({t}.a * 1.5)"),
            });
        }
    }
    let mut sql = format!(
        "SELECT {} FROM {} WHERE {}",
        select.join(", "),
        tables.join(", "),
        terms.join(" AND ")
    );
    if !group_by.is_empty() {
        sql.push_str(&format!(" GROUP BY {}", group_by.join(", ")));
    }
    if ordered {
        let order: Vec<String> = select[..keys]
            .iter()
            .map(|key| {
                let dir = if rng.random_bool(0.5) { " DESC" } else { "" };
                format!("{key}{dir}")
            })
            .collect();
        sql.push_str(&format!(" ORDER BY {} -- keys {keys}", order.join(", ")));
    }
    (sql, rows)
}

/// T-QT.6.18: an OR filter generated for an equijoin must still require matching keys.
#[test]
fn t_qt_6_18_a_generated_disjunction_keeps_the_join_keys() {
    let conn = open(":memory:qt-6-18-or");
    conn.set_parallel_mode(ParallelMode::Off);
    for table in ["t", "u"] {
        conn.execute(format!(
            "CREATE TABLE {table}(a INTEGER, b INTEGER, c TEXT)"
        ))
        .unwrap();
    }
    conn.execute("INSERT INTO t VALUES (1, 10, 'x')").unwrap();
    conn.execute("INSERT INTO u VALUES (2, 20, 'x')").unwrap();
    // Seed 66 generates a row query whose right-hand disjunct accepts both rows of u.
    let (sql, returns_rows) = random_body_chain(&mut ChaCha8Rng::seed_from_u64(66), &["t", "u"]);
    assert!(returns_rows && sql.contains(" IS NULL OR "), "{sql}");
    let disjoint = sql_rows(&conn, &sql);
    assert!(
        disjoint.is_empty(),
        "{sql} joined unequal keys: {disjoint:?}"
    );

    // A top-level OR is a different SQL shape and can intentionally join unequal keys.
    let top_level = "SELECT u.a FROM t, u WHERE t.a = u.a AND u.b IS NULL OR u.a > 0";
    for mode in [ParallelMode::Off, ParallelMode::On] {
        conn.set_parallel_mode(mode);
        assert_eq!(sql_rows(&conn, top_level), vec![vec![Value::from_i64(2)]]);
    }

    conn.execute("INSERT INTO u VALUES (1, 30, 'x')").unwrap();
    assert_eq!(sql_rows(&conn, &sql).len(), 1, "{sql}");
}

/// Compares a parallel answer with the serial one: row for row with aggregates; with rows, in
/// ORDER BY order with ties compared as sets, or as the same rows without an ORDER BY.
fn same_answer(sql: &str, rows: bool, parallel: &[Vec<Value>], serial: &[Vec<Value>], at: &str) {
    if !rows {
        assert_eq!(parallel, serial, "{sql} {at}");
        return;
    }
    if let Some(keys) = order_keys(sql) {
        let key = |row: &Vec<Value>| format!("{:?}", &row[..keys]);
        let serial_keys: Vec<String> = serial.iter().map(key).collect();
        let parallel_keys: Vec<String> = parallel.iter().map(key).collect();
        assert_eq!(parallel_keys, serial_keys, "{sql} {at}");
    }
    assert_eq!(canonical(parallel), canonical(serial), "{sql} {at}");
}

/// T-QT.6.18 (lane qs6-12): a join's workers run the copy of the serial loop's body over each
/// joined row. Random inner equi-joins of two, three and four tables whose filters and outputs
/// only the body states (functions, arithmetic, CASE, LIKE, terms over two tables' columns),
/// with aggregates or returning rows, answer the same with `ParallelMode` off (the serial loop)
/// and on, the machine stated as 1, 2, 3, 4 and 8 cores. A run that took the hook built one link
/// per table past the probe and ran the body; most runs took it.
#[test]
fn t_qt_6_18_join_workers_run_a_copy_of_the_loop_body_as_the_serial_loop() {
    let conn = open(":memory:qt-6-18");
    let mut rng = ChaCha8Rng::seed_from_u64(618);
    fill(&conn, &mut rng, "t", 600);
    fill(&conn, &mut rng, "u", 120);
    fill(&conn, &mut rng, "v", 50);
    fill(&conn, &mut rng, "w", 30);
    conn.set_join_methods(crate::JoinMethods::HashJoinsOnly);
    let queries = 36usize;
    let mut bodied = 0usize;
    let mut bodied_rows = 0usize;
    for query in 0..queries {
        let tables: &[&str] = match query % 3 {
            0 => &["t", "u"],
            1 => &["t", "u", "v"],
            _ => &["t", "u", "v", "w"],
        };
        let (sql, rows) = random_body_chain(&mut rng, tables);
        conn.set_parallel_mode(ParallelMode::Off);
        let serial = sql_rows(&conn, &sql);
        conn.set_parallel_mode(ParallelMode::On);
        for cores in [1usize, 2, 3, 4, 8] {
            *conn.morsel_costs.lock() = Some(MachineCosts::stated(
                NonZeroUsize::new(cores).unwrap(),
                1.0,
                1.0,
            ));
            let links_before = conn
                .morsel_join_links
                .load(std::sync::atomic::Ordering::Relaxed);
            let body_before = conn
                .morsel_body_rows
                .load(std::sync::atomic::Ordering::Relaxed);
            let parallel = sql_rows(&conn, &sql);
            let links = conn
                .morsel_join_links
                .load(std::sync::atomic::Ordering::Relaxed)
                - links_before;
            let body = conn
                .morsel_body_rows
                .load(std::sync::atomic::Ordering::Relaxed)
                - body_before;
            same_answer(&sql, rows, &parallel, &serial, &format!("on {cores} cores"));
            let expected = u64::try_from(tables.len() - 1).unwrap();
            assert!(
                links == 0 || links == expected,
                "{sql} on {cores} cores built {links} links"
            );
            // No body runs where no link was built: the hook either ran the chain or declined.
            assert!(links == expected || body == 0, "{sql} on {cores} cores");
            if links == expected && body > 0 {
                bodied += 1;
                if rows {
                    bodied_rows += 1;
                }
            }
        }
    }
    conn.set_parallel_mode(ParallelMode::Off);
    conn.set_join_methods(crate::JoinMethods::ByCost);
    // Non-vacuity: most runs took the chain and ran the body, row-returning ones among them.
    assert!(
        bodied > queries * 5 / 2,
        "the body ran {bodied} times of {}",
        queries * 5
    );
    assert!(bodied_rows > 0, "no row-returning join ran the body");
}

/// A random outer join of `t` and `u` (lane qs6-12), `kind` LEFT, RIGHT or FULL: ON `a`, or `a`
/// and `c`, with more ON terms on either table or both (so rows on either side match nothing),
/// WHERE terms that read the padded side's NULLs, and aggregates or rows, with an ORDER BY of
/// plain columns or none. Says whether it returns rows.
fn random_outer(rng: &mut ChaCha8Rng, kind: &str) -> (String, bool) {
    let mut on = vec!["t.a = u.a".to_string()];
    if rng.random_bool(0.4) {
        on.push("t.c = u.c".to_string());
    }
    for _ in 0..rng.random_range(0..3) {
        let n = rng.random_range(-20i64..20);
        let k = rng.random_range(0i64..3);
        on.push(match rng.random_range(0..6) {
            0 => format!("u.a > {n}"),
            1 => format!("t.a < {n}"),
            2 => format!("u.c LIKE 't{k}%'"),
            3 => format!("abs(t.b) % 3 = {k}"),
            4 => format!("t.a + u.a > {n}"),
            _ => "u.b IS NOT NULL".to_string(),
        });
    }
    let mut filter = Vec::new();
    for _ in 0..rng.random_range(0..2) {
        let n = rng.random_range(-20i64..20);
        let k = rng.random_range(0i64..3);
        filter.push(match rng.random_range(0..5) {
            0 => "u.a IS NULL".to_string(),
            1 => "t.a IS NULL".to_string(),
            2 => format!("t.c LIKE 't{k}%'"),
            3 => format!("coalesce(u.a, 0) >= {n}"),
            _ => format!("t.a > {n}"),
        });
    }
    let rows = rng.random_bool(0.4);
    let mut select: Vec<String> = Vec::new();
    let mut group_by: Vec<String> = Vec::new();
    let mut keys = 0usize;
    let ordered = rows && rng.random_bool(0.5);
    if rows {
        if ordered {
            keys = rng.random_range(1..=2usize);
            for _ in 0..keys {
                select.push(["t.a", "u.a", "t.c", "u.c"][rng.random_range(0..4)].to_string());
            }
        }
        for _ in 0..rng.random_range(1..4) {
            select.push(
                [
                    "t.a",
                    "u.c",
                    "coalesce(u.a, -1) + t.a",
                    "u.b IS NULL",
                    "t.c || coalesce(u.c, '?')",
                ][rng.random_range(0..5)]
                .to_string(),
            );
        }
    } else {
        for _ in 0..rng.random_range(0..3) {
            group_by.push(
                ["u.a IS NULL", "t.a % 3", "coalesce(u.c, 'none')", "t.c"][rng.random_range(0..4)]
                    .to_string(),
            );
        }
        select = group_by.clone();
        for _ in 0..rng.random_range(1..4) {
            select.push(
                [
                    "count(*)",
                    "count(u.a)",
                    "sum(t.a)",
                    "sum(coalesce(u.a, 0))",
                    "min(u.c)",
                    "max(t.c)",
                    "max(u.a * 1.5)",
                ][rng.random_range(0..7)]
                .to_string(),
            );
        }
    }
    let mut sql = format!(
        "SELECT {} FROM t {kind} JOIN u ON {}",
        select.join(", "),
        on.join(" AND ")
    );
    if !filter.is_empty() {
        sql.push_str(&format!(" WHERE {}", filter.join(" AND ")));
    }
    if !group_by.is_empty() {
        sql.push_str(&format!(" GROUP BY {}", group_by.join(", ")));
    }
    if ordered {
        let order: Vec<String> = select[..keys]
            .iter()
            .map(|key| {
                let dir = if rng.random_bool(0.5) { " DESC" } else { "" };
                format!("{key}{dir}")
            })
            .collect();
        sql.push_str(&format!(" ORDER BY {} -- keys {keys}", order.join(", ")));
    }
    (sql, rows)
}

/// T-QT.6.19 (lane qs6-12): outer joins run on morsels. Random LEFT, RIGHT and FULL joins of two
/// tables, ON terms on either table or both beside the keys, WHERE terms that read the padded
/// side's NULLs, with aggregates or returning rows, answer the same with `ParallelMode` off (the
/// serial loop) and on, the machine stated as 1, 2, 3, 4 and 8 cores: the matched rows, the
/// unmatched rows padded with NULLs (the build side's from the workers' merged bitmaps, a FULL
/// join's probe side's by the worker that read them), ON terms deciding matches and WHERE terms
/// testing the padded rows. Each kind took the hook in some runs, one link built and the body run.
#[test]
fn t_qt_6_19_outer_joins_pad_unmatched_rows_as_the_serial_loop() {
    // At each orientation of a two-table outer join (lane qs6-15), today's first.
    for probe in [crate::OuterJoinProbe::Padded, crate::OuterJoinProbe::Kept] {
        t_qt_6_19_outer_joins_pad_unmatched_rows_as_the_serial_loop_at(probe);
    }
}

/// T-QT.6.19 at one orientation of a two-table outer join, `probe` (lane qs6-15).
fn t_qt_6_19_outer_joins_pad_unmatched_rows_as_the_serial_loop_at(probe: crate::OuterJoinProbe) {
    let conn = open(&format!(":memory:qt-6-19-{probe:?}"));
    conn.set_parallel_outer_probe(probe);
    let mut rng = ChaCha8Rng::seed_from_u64(619);
    fill(&conn, &mut rng, "t", 500);
    fill(&conn, &mut rng, "u", 150);
    conn.set_join_methods(crate::JoinMethods::HashJoinsOnly);
    let kinds = ["LEFT", "RIGHT", "FULL"];
    let mut hooked = [0usize; 3];
    let queries = 36usize;
    let outer = |conn: &Arc<Connection>| {
        let o = &conn.morsel_outer;
        let load = |c: &std::sync::atomic::AtomicU64| c.load(std::sync::atomic::Ordering::Relaxed);
        [
            load(&o.padded_probes),
            load(&o.chains),
            load(&o.walk_pads),
            load(&o.bitmap_pads),
        ]
    };
    let outer_before = outer(&conn);
    for query in 0..queries {
        let at = query % kinds.len();
        let (sql, rows) = random_outer(&mut rng, kinds[at]);
        conn.set_parallel_mode(ParallelMode::Off);
        let serial = sql_rows(&conn, &sql);
        conn.set_parallel_mode(ParallelMode::On);
        for cores in [1usize, 2, 3, 4, 8] {
            *conn.morsel_costs.lock() = Some(MachineCosts::stated(
                NonZeroUsize::new(cores).unwrap(),
                1.0,
                1.0,
            ));
            let links_before = conn
                .morsel_join_links
                .load(std::sync::atomic::Ordering::Relaxed);
            let body_before = conn
                .morsel_body_rows
                .load(std::sync::atomic::Ordering::Relaxed);
            let parallel = sql_rows(&conn, &sql);
            let links = conn
                .morsel_join_links
                .load(std::sync::atomic::Ordering::Relaxed)
                - links_before;
            let body = conn
                .morsel_body_rows
                .load(std::sync::atomic::Ordering::Relaxed)
                - body_before;
            same_answer(&sql, rows, &parallel, &serial, &format!("on {cores} cores"));
            assert!(links <= 1, "{sql} on {cores} cores built {links} links");
            if links == 1 && body > 0 {
                hooked[at] += 1;
            }
        }
    }
    conn.set_parallel_mode(ParallelMode::Off);
    conn.set_join_methods(crate::JoinMethods::ByCost);
    // Non-vacuity: every kind ran on morsels.
    for (kind, runs) in kinds.iter().zip(hooked) {
        assert!(runs > 0, "no {kind} JOIN took the hook");
    }
    // Each orientation ran its own path (lane qs6-15): the padded table probed with a bitmap of
    // the kept one's rows, or the kept table probed in the chain, its rows padded in the walk.
    let after = outer(&conn);
    let [padded_probes, chains, walk_pads, bitmap_pads] =
        [0, 1, 2, 3].map(|at| after[at] - outer_before[at]);
    match probe {
        crate::OuterJoinProbe::Padded => {
            assert!(
                padded_probes > 0 && chains == 0,
                "{probe:?}: {padded_probes} padded probes, {chains} chains"
            );
            assert!(
                bitmap_pads > 0,
                "{probe:?}: no row padded after the bitmaps merged"
            );
        }
        crate::OuterJoinProbe::Kept => {
            assert!(
                padded_probes == 0 && chains > 0,
                "{probe:?}: {padded_probes} padded probes, {chains} chains"
            );
            assert!(walk_pads > 0, "{probe:?}: no row padded during the walk");
        }
    }
}

/// T-QT.6.21 (lane qs6-13): a join whose loop body is nothing but reads (no ON or WHERE term past
/// the keys and the tables' own filters, every output a column) has its outputs copied straight
/// from the joined row when `set_parallel_join_direct` is on, no body run. Simple equi-joins of
/// two tables, with aggregates (count, sum, min, max, GROUP BY) and returning rows, answer as the
/// serial loop at 1, 2, 3, 4 and 8 stated cores with the switch on and off; with it on their rows
/// are counted as copied, and with it off, or with an output or a WHERE term only the body can
/// compute, none is.
#[test]
fn t_qt_6_21_a_join_of_reads_only_copies_its_outputs_when_switched_on() {
    let conn = open(":memory:qt-6-21");
    let mut rng = ChaCha8Rng::seed_from_u64(621);
    fill(&conn, &mut rng, "t", 600);
    fill(&conn, &mut rng, "u", 150);
    conn.set_join_methods(crate::JoinMethods::HashJoinsOnly);
    // (the query, whether it returns rows, whether its body is nothing but reads)
    let queries = [
        (
            "SELECT count(*), sum(t.b), min(u.c), max(t.a) FROM t JOIN u ON t.a = u.a",
            false,
            true,
        ),
        (
            "SELECT t.c, count(*), sum(u.b) FROM t JOIN u ON t.a = u.a GROUP BY t.c",
            false,
            true,
        ),
        (
            "SELECT count(*), min(t.b) FROM t JOIN u ON t.a = u.a WHERE t.b > 0",
            false,
            true,
        ),
        ("SELECT t.b, u.c FROM t JOIN u ON t.a = u.a", true, true),
        (
            "SELECT count(*), sum(t.b + u.b) FROM t JOIN u ON t.a = u.a",
            false,
            false,
        ),
        (
            "SELECT count(*) FROM t JOIN u ON t.a = u.a WHERE t.b < u.b",
            false,
            false,
        ),
    ];
    let mut copied_any = false;
    for (sql, rows, reads_only) in queries {
        conn.set_parallel_mode(ParallelMode::Off);
        let serial = sql_rows(&conn, sql);
        conn.set_parallel_mode(ParallelMode::On);
        for direct in [false, true] {
            conn.set_parallel_join_direct(direct);
            for cores in [1usize, 2, 3, 4, 8] {
                *conn.morsel_costs.lock() = Some(MachineCosts::stated(
                    NonZeroUsize::new(cores).unwrap(),
                    1.0,
                    1.0,
                ));
                let links_before = conn
                    .morsel_join_links
                    .load(std::sync::atomic::Ordering::Relaxed);
                let copied_before = conn
                    .morsel_join_direct_rows
                    .load(std::sync::atomic::Ordering::Relaxed);
                let parallel = sql_rows(&conn, sql);
                let links = conn
                    .morsel_join_links
                    .load(std::sync::atomic::Ordering::Relaxed)
                    - links_before;
                let copied = conn
                    .morsel_join_direct_rows
                    .load(std::sync::atomic::Ordering::Relaxed)
                    - copied_before;
                let what = format!("on {cores} cores, direct {direct}");
                same_answer(sql, rows, &parallel, &serial, &what);
                if !(direct && reads_only) {
                    assert_eq!(copied, 0, "{sql} {what} copied {copied} rows");
                } else if links == 1 {
                    assert!(copied > 0, "{sql} {what} took the hook and copied nothing");
                    copied_any = true;
                }
            }
        }
    }
    conn.set_parallel_join_direct(false);
    conn.set_parallel_mode(ParallelMode::Off);
    conn.set_join_methods(crate::JoinMethods::ByCost);
    // Non-vacuity: some run took the hook and copied its outputs.
    assert!(copied_any, "no reads-only join copied its outputs");
}

/// T-QT.6.22 (lane qs6-13): outer joins under LIMIT and OFFSET run on morsels. Random LEFT, RIGHT
/// and FULL joins of two tables with INTEGER PRIMARY KEYs, ON terms beside the key and WHERE terms
/// that read the padded side's NULLs, their rows ordered by both keys (each either way, after an
/// optional lead key) under a LIMIT and an OFFSET, answer row for row as the serial loop at 1, 2,
/// 3, 4 and 8 stated cores: a padded row's NULL key still tells it apart. Each kind took the hook
/// in some runs (one link built, the body run); an order with ties under a LIMIT is declined.
#[test]
fn t_qt_6_22_outer_joins_under_limit_answer_as_the_serial_loop() {
    // At each orientation of a two-table outer join (lane qs6-15), today's first.
    for probe in [crate::OuterJoinProbe::Padded, crate::OuterJoinProbe::Kept] {
        t_qt_6_22_outer_joins_under_limit_answer_as_the_serial_loop_at(probe);
    }
}

/// T-QT.6.22 at one orientation of a two-table outer join, `probe` (lane qs6-15).
fn t_qt_6_22_outer_joins_under_limit_answer_as_the_serial_loop_at(probe: crate::OuterJoinProbe) {
    let conn = open(&format!(":memory:qt-6-22-{probe:?}"));
    conn.set_parallel_outer_probe(probe);
    let mut rng = ChaCha8Rng::seed_from_u64(622);
    let pad = "x".repeat(64);
    for (name, rows) in [("t", 900usize), ("u", 300)] {
        conn.execute(format!(
            "CREATE TABLE {name}(id INTEGER PRIMARY KEY, a INTEGER, b INTEGER, c TEXT, p TEXT)"
        ))
        .unwrap();
        conn.execute("BEGIN").unwrap();
        for _ in 0..rows {
            let a = if rng.random_range(0..10) == 0 {
                "NULL".to_string()
            } else {
                rng.random_range(-20i64..20).to_string()
            };
            let b = rng.random_range(-1000i64..1000);
            let c = rng.random_range(0..12);
            conn.execute(format!(
                "INSERT INTO {name}(a, b, c, p) VALUES ({a}, {b}, 't{c}', '{pad}')"
            ))
            .unwrap();
        }
        conn.execute("COMMIT").unwrap();
    }
    conn.set_join_methods(crate::JoinMethods::HashJoinsOnly);
    let run = |sql: &str, cores: usize| {
        *conn.morsel_costs.lock() = Some(MachineCosts::stated(
            NonZeroUsize::new(cores).unwrap(),
            1.0,
            1.0,
        ));
        let links_before = conn
            .morsel_join_links
            .load(std::sync::atomic::Ordering::Relaxed);
        let body_before = conn
            .morsel_body_rows
            .load(std::sync::atomic::Ordering::Relaxed);
        let rows = sql_rows(&conn, sql);
        let links = conn
            .morsel_join_links
            .load(std::sync::atomic::Ordering::Relaxed)
            - links_before;
        let body = conn
            .morsel_body_rows
            .load(std::sync::atomic::Ordering::Relaxed)
            - body_before;
        (rows, links == 1 && body > 0)
    };
    let kinds = ["LEFT", "RIGHT", "FULL"];
    let mut hooked = [0usize; 3];
    let queries = 36usize;
    for query in 0..queries {
        let at = query % kinds.len();
        let kind = kinds[at];
        let dir = |rng: &mut ChaCha8Rng| if rng.random_bool(0.5) { " DESC" } else { "" };
        let lead = ["", "t.a, ", "u.c DESC, "][rng.random_range(0..3)];
        let (dt, du) = (dir(&mut rng), dir(&mut rng));
        let order = if rng.random_bool(0.5) {
            format!("{lead}t.id{dt}, u.id{du}")
        } else {
            format!("{lead}u.id{du}, t.id{dt}")
        };
        let on = match rng.random_range(0..4) {
            0 => String::new(),
            1 => format!(" AND u.b > {}", rng.random_range(-1000i64..1000)),
            2 => format!(" AND t.b < {}", rng.random_range(-1000i64..1000)),
            _ => " AND t.b + u.b > 0".to_string(),
        };
        let filter = match rng.random_range(0..4) {
            0 | 1 => String::new(),
            2 => " WHERE u.a IS NULL OR u.b > 0".to_string(),
            _ => format!(" WHERE coalesce(t.a, 0) > {}", rng.random_range(-20i64..20)),
        };
        let limit = if rng.random_range(0..6) == 0 {
            -1
        } else {
            rng.random_range(1i64..200)
        };
        let offset = match rng.random_range(0..3) {
            0 => String::new(),
            1 => format!(" OFFSET {}", rng.random_range(0..300)),
            _ => " OFFSET -3".to_string(),
        };
        let sql = format!(
            "SELECT t.id, u.id, t.c, u.a FROM t {kind} JOIN u ON t.a = u.a{on}{filter} ORDER BY {order} LIMIT {limit}{offset}"
        );
        conn.set_parallel_mode(ParallelMode::Off);
        let serial = sql_rows(&conn, &sql);
        conn.set_parallel_mode(ParallelMode::On);
        for cores in [1usize, 2, 3, 4, 8] {
            let (parallel, ran) = run(&sql, cores);
            assert_eq!(parallel, serial, "{sql} on {cores} cores");
            if ran {
                hooked[at] += 1;
            }
        }
    }
    // An order with ties under a LIMIT could cut a different tie: the hook declines it.
    for kind in kinds {
        let tied =
            format!("SELECT t.a, u.c FROM t {kind} JOIN u ON t.a = u.a ORDER BY t.a LIMIT 10");
        let (_, ran) = run(&tied, 4);
        assert!(!ran, "{tied} took the hook");
    }
    conn.set_parallel_mode(ParallelMode::Off);
    conn.set_join_methods(crate::JoinMethods::ByCost);
    // Non-vacuity: every kind ran on morsels under a LIMIT.
    for (kind, runs) in kinds.iter().zip(hooked) {
        assert!(runs > 0, "no {kind} JOIN under a LIMIT took the hook");
    }
}

/// T-QT.6.23 (lane qs6-13): a LEFT join among three and four tables runs on morsels, its padded
/// table the chain's last link. Random queries joining inner tables in any order around one LEFT
/// JOIN (before or after it in the SQL), ON terms beside its key on either side or both, WHERE
/// terms that read the padded side's NULLs, with aggregates, returning rows, or ordered by every
/// table's INTEGER PRIMARY KEY under a LIMIT and an OFFSET, answer as the serial loop at 1, 2, 3,
/// 4 and 8 stated cores (row for row with aggregates or a LIMIT; in ORDER BY order with ties as
/// sets, or as the same rows, otherwise). Runs of both sizes took the hook (a link per table past
/// the probe, the body run).
#[test]
fn t_qt_6_23_left_joins_of_three_and_four_tables_pad_the_last_link() {
    // At each orientation of a two-table outer join (lane qs6-15), today's first.
    for probe in [crate::OuterJoinProbe::Padded, crate::OuterJoinProbe::Kept] {
        t_qt_6_23_left_joins_of_three_and_four_tables_pad_the_last_link_at(probe);
    }
}

/// T-QT.6.23 at one orientation of a two-table outer join, `probe` (lane qs6-15).
fn t_qt_6_23_left_joins_of_three_and_four_tables_pad_the_last_link_at(
    probe: crate::OuterJoinProbe,
) {
    let conn = open(&format!(":memory:qt-6-23-{probe:?}"));
    conn.set_parallel_outer_probe(probe);
    let mut rng = ChaCha8Rng::seed_from_u64(623);
    let pad = "x".repeat(48);
    for (name, rows) in [("t", 700usize), ("u", 160), ("v", 60), ("w", 30)] {
        conn.execute(format!(
            "CREATE TABLE {name}(id INTEGER PRIMARY KEY, a INTEGER, b INTEGER, c TEXT, p TEXT)"
        ))
        .unwrap();
        conn.execute("BEGIN").unwrap();
        for _ in 0..rows {
            let a = if rng.random_range(0..10) == 0 {
                "NULL".to_string()
            } else {
                rng.random_range(-20i64..20).to_string()
            };
            let b = rng.random_range(-1000i64..1000);
            let c = rng.random_range(0..12);
            conn.execute(format!(
                "INSERT INTO {name}(a, b, c, p) VALUES ({a}, {b}, 't{c}', '{pad}')"
            ))
            .unwrap();
        }
        conn.execute("COMMIT").unwrap();
    }
    conn.set_join_methods(crate::JoinMethods::HashJoinsOnly);
    let mut hooked = [0usize; 2];
    let queries = 40usize;
    for query in 0..queries {
        let four = query % 2 == 1;
        let on_extra = match rng.random_range(0..4) {
            0 => String::new(),
            1 => format!(" AND u.b > {}", rng.random_range(-1000i64..1000)),
            2 => format!(" AND t.b < {}", rng.random_range(-1000i64..1000)),
            _ => " AND t.b + u.b > 0".to_string(),
        };
        let left = format!("LEFT JOIN u ON u.c = t.c{on_extra}");
        let mut inner = vec!["JOIN v ON v.a = t.a".to_string()];
        if four {
            inner.push("JOIN w ON w.a = v.a".to_string());
        }
        // The LEFT JOIN last in the SQL, or first with the inner joins after it.
        let from = if rng.random_bool(0.5) {
            format!("t {} {left}", inner.join(" "))
        } else {
            format!("t {left} {}", inner.join(" "))
        };
        let filter = match rng.random_range(0..4) {
            0 | 1 => String::new(),
            2 => " WHERE u.id IS NULL OR u.b > 0".to_string(),
            _ => format!(
                " WHERE coalesce(u.a, 0) >= {}",
                rng.random_range(-20i64..20)
            ),
        };
        let tables = if four {
            "t.id, v.id, w.id, u.id"
        } else {
            "t.id, v.id, u.id"
        };
        let (sql, rows) = match rng.random_range(0..3) {
            0 => (
                format!(
                    "SELECT u.c IS NULL, count(*), count(u.a), sum(t.b), min(u.c), max(v.b) FROM {from}{filter} GROUP BY u.c IS NULL"
                ),
                false,
            ),
            1 => (
                format!("SELECT t.a, u.c, coalesce(u.b, -1) + v.b FROM {from}{filter}"),
                true,
            ),
            _ => {
                let dir = |rng: &mut ChaCha8Rng| if rng.random_bool(0.5) { " DESC" } else { "" };
                let mut keys: Vec<String> = tables
                    .split(", ")
                    .map(|key| format!("{key}{}", dir(&mut rng)))
                    .collect();
                if rng.random_bool(0.5) {
                    keys.reverse();
                }
                (
                    format!(
                        "SELECT {tables}, u.b FROM {from}{filter} ORDER BY {} LIMIT {} OFFSET {}",
                        keys.join(", "),
                        rng.random_range(1i64..150),
                        rng.random_range(0i64..100)
                    ),
                    false,
                )
            }
        };
        conn.set_parallel_mode(ParallelMode::Off);
        let serial = sql_rows(&conn, &sql);
        conn.set_parallel_mode(ParallelMode::On);
        for cores in [1usize, 2, 3, 4, 8] {
            *conn.morsel_costs.lock() = Some(MachineCosts::stated(
                NonZeroUsize::new(cores).unwrap(),
                1.0,
                1.0,
            ));
            let links_before = conn
                .morsel_join_links
                .load(std::sync::atomic::Ordering::Relaxed);
            let body_before = conn
                .morsel_body_rows
                .load(std::sync::atomic::Ordering::Relaxed);
            let parallel = sql_rows(&conn, &sql);
            let links = conn
                .morsel_join_links
                .load(std::sync::atomic::Ordering::Relaxed)
                - links_before;
            let body = conn
                .morsel_body_rows
                .load(std::sync::atomic::Ordering::Relaxed)
                - body_before;
            same_answer(&sql, rows, &parallel, &serial, &format!("on {cores} cores"));
            let want = if four { 3 } else { 2 };
            assert!(
                links == 0 || links == want,
                "{sql} on {cores} cores built {links} links"
            );
            if links == want && body > 0 {
                hooked[usize::from(four)] += 1;
            }
        }
    }
    conn.set_parallel_mode(ParallelMode::Off);
    conn.set_join_methods(crate::JoinMethods::ByCost);
    // Non-vacuity: joins of three and of four tables ran on morsels.
    assert!(hooked[0] > 0, "no LEFT join of three tables took the hook");
    assert!(hooked[1] > 0, "no LEFT join of four tables took the hook");
}

/// Tables `name(id INTEGER PRIMARY KEY, a INTEGER, b INTEGER, c TEXT, p TEXT)` of the given sizes
/// (lane qs6-14): `a` in -20..20 or NULL, `b` in -1000..1000, `c` one of twelve texts, `p` a pad so
/// each spans many leaf pages.
fn fill_with_ids(conn: &Arc<Connection>, rng: &mut ChaCha8Rng, tables: &[(&str, usize)]) {
    let pad = "x".repeat(48);
    for (name, rows) in tables {
        conn.execute(format!(
            "CREATE TABLE {name}(id INTEGER PRIMARY KEY, a INTEGER, b INTEGER, c TEXT, p TEXT)"
        ))
        .unwrap();
        conn.execute("BEGIN").unwrap();
        for _ in 0..*rows {
            let a = if rng.random_range(0..10) == 0 {
                "NULL".to_string()
            } else {
                rng.random_range(-20i64..20).to_string()
            };
            let b = rng.random_range(-1000i64..1000);
            let c = rng.random_range(0..12);
            conn.execute(format!(
                "INSERT INTO {name}(a, b, c, p) VALUES ({a}, {b}, 't{c}', '{pad}')"
            ))
            .unwrap();
        }
        conn.execute("COMMIT").unwrap();
    }
}

/// A random join of `names` in that order (lane qs6-14), the table after each `kinds[k]` (`JOIN`,
/// `LEFT JOIN`, `RIGHT JOIN` or `FULL JOIN`) joined to a random earlier table on `a` (or `c`),
/// with ON terms beside the key over its own table, the earlier one or both, WHERE terms that
/// read a padded side's NULLs, and aggregates, rows, or rows ordered by every table's id under a
/// LIMIT and an OFFSET. Says whether it returns rows in no stated order (compared as sets).
fn random_outer_chain(rng: &mut ChaCha8Rng, names: &[&str], kinds: &[&str]) -> (String, bool) {
    let mut from = names[0].to_string();
    for (at, kind) in kinds.iter().enumerate() {
        let x = names[at + 1];
        let p = names[rng.random_range(0..=at)];
        let key = if rng.random_bool(0.8) { "a" } else { "c" };
        let mut on = vec![format!("{x}.{key} = {p}.{key}")];
        for _ in 0..rng.random_range(0..2) {
            let n = rng.random_range(-1000i64..1000);
            let k = rng.random_range(0..12);
            on.push(match rng.random_range(0..4) {
                0 => format!("{x}.b > {n}"),
                1 => format!("{p}.b < {n}"),
                2 => format!("{x}.b + {p}.b > 0"),
                _ => format!("{x}.c <> 't{k}'"),
            });
        }
        from.push_str(&format!(" {kind} {x} ON {}", on.join(" AND ")));
    }
    let pick = |rng: &mut ChaCha8Rng| names[rng.random_range(0..names.len())];
    let filter = match rng.random_range(0..5) {
        0..=2 => String::new(),
        3 => {
            let x = pick(rng);
            format!(" WHERE {x}.id IS NULL OR {x}.b > 0")
        }
        _ => format!(
            " WHERE coalesce({}.a, 0) >= {}",
            pick(rng),
            rng.random_range(-20i64..20)
        ),
    };
    match rng.random_range(0..3) {
        0 => {
            let g = pick(rng);
            let (x, y, z, w) = (pick(rng), pick(rng), pick(rng), pick(rng));
            (
                format!(
                    "SELECT {g}.id IS NULL, count(*), count({x}.a), sum({y}.b), min({z}.c), max({w}.b) FROM {from}{filter} GROUP BY {g}.id IS NULL"
                ),
                false,
            )
        }
        1 => {
            let (x, y, z, w) = (pick(rng), pick(rng), pick(rng), pick(rng));
            (
                format!(
                    "SELECT {x}.a, {y}.c, coalesce({z}.b, -1) + coalesce({w}.b, 0) FROM {from}{filter}"
                ),
                true,
            )
        }
        _ => {
            let ids: Vec<String> = names.iter().map(|name| format!("{name}.id")).collect();
            let mut keys: Vec<String> = ids
                .iter()
                .map(|id| {
                    let dir = if rng.random_bool(0.5) { " DESC" } else { "" };
                    format!("{id}{dir}")
                })
                .collect();
            if rng.random_bool(0.5) {
                keys.reverse();
            }
            (
                format!(
                    "SELECT {}, {}.b FROM {from}{filter} ORDER BY {} LIMIT {} OFFSET {}",
                    ids.join(", "),
                    pick(rng),
                    keys.join(", "),
                    rng.random_range(1i64..150),
                    rng.random_range(0i64..100)
                ),
                false,
            )
        }
    }
}

/// Runs `sql` with `ParallelMode` off (the serial loop) and on at 1, 2, 3, 4 and 8 stated cores
/// (lane qs6-14) and checks the answers agree; says how many of the parallel runs took the hook
/// (a link built per table past the probe, the body run), or none when the serial loop refuses
/// the statement (its error printed: the morsels don't answer what the serial loop can't).
fn chain_runs(conn: &Arc<Connection>, sql: &str, rows: bool, tables: usize) -> Option<usize> {
    conn.set_parallel_mode(ParallelMode::Off);
    let serial = match conn
        .prepare(sql)
        .and_then(|mut stmt| stmt.run_collect_rows())
    {
        Ok(serial) => serial,
        Err(error) => {
            eprintln!("the serial loop refused {sql}: {error}");
            return None;
        }
    };
    conn.set_parallel_mode(ParallelMode::On);
    let want = u64::try_from(tables - 1).unwrap();
    let mut hooked = 0usize;
    for cores in [1usize, 2, 3, 4, 8] {
        *conn.morsel_costs.lock() = Some(MachineCosts::stated(
            NonZeroUsize::new(cores).unwrap(),
            1.0,
            1.0,
        ));
        let links_before = conn
            .morsel_join_links
            .load(std::sync::atomic::Ordering::Relaxed);
        let body_before = conn
            .morsel_body_rows
            .load(std::sync::atomic::Ordering::Relaxed);
        let parallel = sql_rows(conn, sql);
        let links = conn
            .morsel_join_links
            .load(std::sync::atomic::Ordering::Relaxed)
            - links_before;
        let body = conn
            .morsel_body_rows
            .load(std::sync::atomic::Ordering::Relaxed)
            - body_before;
        same_answer(sql, rows, &parallel, &serial, &format!("on {cores} cores"));
        assert!(
            links == 0 || links == want,
            "{sql} on {cores} cores built {links} links"
        );
        if links == want && body > 0 {
            hooked += 1;
        }
    }
    conn.set_parallel_mode(ParallelMode::Off);
    Some(hooked)
}

/// T-QT.6.24 (lane qs6-14): a FULL JOIN among three and four tables runs on morsels, the chain in
/// the FROM clause's order. Random joins with a FULL JOIN at a random link and inner, LEFT, RIGHT
/// or FULL joins at the others, keyed on an INTEGER or a TEXT column of a random earlier table, ON
/// terms beside the keys, WHERE terms that read the padded sides' NULLs, with aggregates,
/// returning rows, or ordered by every table's INTEGER PRIMARY KEY under a LIMIT and an OFFSET,
/// answer as the serial loop at 1, 2, 3, 4 and 8 stated cores (row for row with aggregates or a
/// LIMIT; as the same rows otherwise). Joins of both sizes with no WHERE term (every outer join
/// kept as written, so only the chain takes them) took the hook.
#[test]
fn t_qt_6_24_full_joins_of_three_and_four_tables_answer_as_the_serial_loop() {
    // At each orientation of a two-table outer join (lane qs6-15), today's first.
    for probe in [crate::OuterJoinProbe::Padded, crate::OuterJoinProbe::Kept] {
        t_qt_6_24_full_joins_of_three_and_four_tables_answer_as_the_serial_loop_at(probe);
    }
}

/// T-QT.6.24 at one orientation of a two-table outer join, `probe` (lane qs6-15).
fn t_qt_6_24_full_joins_of_three_and_four_tables_answer_as_the_serial_loop_at(
    probe: crate::OuterJoinProbe,
) {
    let conn = open(&format!(":memory:qt-6-24-{probe:?}"));
    conn.set_parallel_outer_probe(probe);
    let mut rng = ChaCha8Rng::seed_from_u64(624);
    fill_with_ids(
        &conn,
        &mut rng,
        &[("t", 400), ("u", 120), ("v", 50), ("w", 30)],
    );
    conn.set_join_methods(crate::JoinMethods::HashJoinsOnly);
    let kinds = ["JOIN", "LEFT JOIN", "RIGHT JOIN", "FULL JOIN"];
    let mut hooked = [0usize; 2];
    let queries = 40usize;
    for query in 0..queries {
        let names: &[&str] = if query % 2 == 0 {
            &["t", "u", "v"]
        } else {
            &["t", "u", "v", "w"]
        };
        let mut link_kinds: Vec<&str> = (1..names.len())
            .map(|_| kinds[rng.random_range(0..kinds.len())])
            .collect();
        let full = rng.random_range(0..link_kinds.len());
        link_kinds[full] = "FULL JOIN";
        let (sql, rows) = random_outer_chain(&mut rng, names, &link_kinds);
        if let Some(runs) = chain_runs(&conn, &sql, rows, names.len()) {
            // With no WHERE term the optimizer keeps every outer join as written, so only the
            // chain could have taken it.
            if !sql.contains(" WHERE ") {
                hooked[names.len() - 3] += runs;
            }
        }
    }
    conn.set_join_methods(crate::JoinMethods::ByCost);
    // Non-vacuity: FULL joins of three and of four tables ran on morsels.
    assert!(hooked[0] > 0, "no FULL join of three tables took the hook");
    assert!(hooked[1] > 0, "no FULL join of four tables took the hook");
}

/// T-QT.6.25 (lane qs6-14): more than one outer join in a statement runs on morsels. Random joins
/// of three and four tables with two or more LEFT, RIGHT or FULL joins (at random links, an inner
/// join at the rest), as T-QT.6.24's, answer as the serial loop at 1, 2, 3, 4 and 8 stated cores.
/// Joins of both sizes with no WHERE term took the hook, and so did one of LEFT joins alone.
#[test]
fn t_qt_6_25_more_than_one_outer_join_answers_as_the_serial_loop() {
    // At each orientation of a two-table outer join (lane qs6-15), today's first.
    for probe in [crate::OuterJoinProbe::Padded, crate::OuterJoinProbe::Kept] {
        t_qt_6_25_more_than_one_outer_join_answers_as_the_serial_loop_at(probe);
    }
}

/// T-QT.6.25 at one orientation of a two-table outer join, `probe` (lane qs6-15).
fn t_qt_6_25_more_than_one_outer_join_answers_as_the_serial_loop_at(probe: crate::OuterJoinProbe) {
    let conn = open(&format!(":memory:qt-6-25-{probe:?}"));
    conn.set_parallel_outer_probe(probe);
    let mut rng = ChaCha8Rng::seed_from_u64(625);
    fill_with_ids(
        &conn,
        &mut rng,
        &[("t", 400), ("u", 120), ("v", 50), ("w", 30)],
    );
    conn.set_join_methods(crate::JoinMethods::HashJoinsOnly);
    let outer = ["LEFT JOIN", "LEFT JOIN", "RIGHT JOIN", "FULL JOIN"];
    let mut hooked = [0usize; 2];
    let mut left_only = 0usize;
    let queries = 40usize;
    for query in 0..queries {
        let names: &[&str] = if query % 2 == 0 {
            &["t", "u", "v"]
        } else {
            &["t", "u", "v", "w"]
        };
        let links = names.len() - 1;
        // Two links (all of a three-table join's) outer, the rest inner or outer.
        let mut link_kinds: Vec<&str> = (0..links)
            .map(|_| {
                if rng.random_bool(0.5) {
                    "JOIN"
                } else {
                    outer[rng.random_range(0..outer.len())]
                }
            })
            .collect();
        let first = rng.random_range(0..links);
        let second = (first + rng.random_range(1..links)) % links;
        for at in [first, second] {
            if link_kinds[at] == "JOIN" {
                link_kinds[at] = outer[rng.random_range(0..outer.len())];
            }
        }
        // Every fourth statement: LEFT joins alone.
        if query % 4 == 3 {
            for kind in link_kinds.iter_mut().filter(|kind| **kind != "JOIN") {
                *kind = "LEFT JOIN";
            }
        }
        let lefts = link_kinds
            .iter()
            .all(|kind| *kind != "RIGHT JOIN" && *kind != "FULL JOIN");
        let (sql, rows) = random_outer_chain(&mut rng, names, &link_kinds);
        if let Some(runs) = chain_runs(&conn, &sql, rows, names.len()) {
            // As in T-QT.6.24: with no WHERE term only the chain could have taken it.
            if !sql.contains(" WHERE ") {
                hooked[names.len() - 3] += runs;
                if lefts {
                    left_only += runs;
                }
            }
        }
    }
    conn.set_join_methods(crate::JoinMethods::ByCost);
    // Non-vacuity: joins of three and of four tables ran on morsels, LEFT joins alone among them.
    assert!(
        hooked[0] > 0,
        "no join of three tables with two outer joins took the hook"
    );
    assert!(
        hooked[1] > 0,
        "no join of four tables with two outer joins took the hook"
    );
    assert!(
        left_only > 0,
        "no statement of LEFT joins alone took the hook"
    );
}

/// T-QT.6.27 (lane qs6-16): a join whose table the planner reads by a seek per row of the tables
/// before it (a foreign key to an INTEGER PRIMARY KEY, a seek in an index on the joined column),
/// planned by cost as it is outside the tests, runs on morsels at each candidate of
/// `Connection::set_parallel_searched_links`: `Hash`, that table a hash link read whole; `Seek`,
/// an inner join's primary-key link read by a seek per joined row. Inner joins of two and three
/// tables (a key beside the primary key too), a LEFT JOIN onto a primary key (an ON term beside
/// it), with aggregates, GROUP BY, rows, and rows ordered by both ids under a LIMIT and an OFFSET,
/// answer as the serial loop at 1, 2, 3, 4 and 8 stated cores at `Off`, `Hash` and `Seek`; `Hash`
/// built links from searched tables and no seek link, `Seek` built seek links, and `Off` (today's:
/// such a join stays on the serial loop) built neither.
#[test]
fn t_qt_6_27_joins_through_seeks_run_on_morsels_as_the_serial_loop() {
    let conn = open(":memory:qt-6-27");
    let mut rng = ChaCha8Rng::seed_from_u64(627);
    fill_with_ids(&conn, &mut rng, &[("f", 400), ("d", 60), ("e", 30)]);
    conn.execute("CREATE INDEX d_b ON d(b)").unwrap();
    let queries: [(&str, bool); 10] = [
        (
            "SELECT count(*), sum(d.b) FROM f JOIN d ON f.a = d.id",
            false,
        ),
        (
            "SELECT d.c, count(*), min(f.b), max(d.b) FROM f JOIN d ON f.a = d.id GROUP BY d.c",
            false,
        ),
        (
            "SELECT f.id, d.id, d.b FROM f JOIN d ON f.a = d.id WHERE f.b > 0",
            true,
        ),
        (
            "SELECT count(*), sum(e.b) FROM f JOIN d ON f.a = d.id JOIN e ON d.a = e.id",
            false,
        ),
        (
            "SELECT e.c, count(*), sum(f.b) FROM f JOIN d ON d.id = f.a JOIN e ON e.id = d.a \
             WHERE d.c <> 't4' GROUP BY e.c",
            false,
        ),
        (
            "SELECT count(*), sum(f.b) FROM f JOIN d ON f.b = d.b",
            false,
        ),
        (
            "SELECT count(*), sum(d.b) FROM f JOIN d ON f.a = d.id AND f.c = d.c",
            false,
        ),
        (
            "SELECT count(*), count(d.id), sum(d.b) FROM f LEFT JOIN d ON f.a = d.id",
            false,
        ),
        (
            "SELECT f.id, d.b FROM f LEFT JOIN d ON f.a = d.id AND d.c <> 't3'",
            true,
        ),
        (
            "SELECT f.id, d.id FROM f JOIN d ON f.a = d.id ORDER BY f.id, d.id \
             LIMIT 7 OFFSET 3 -- keys 2",
            true,
        ),
    ];
    let counts = |conn: &Arc<Connection>| {
        [&conn.morsel_searched_links, &conn.morsel_seek_links]
            .map(|c| c.load(std::sync::atomic::Ordering::Relaxed))
    };
    for links in [
        crate::SearchedLinks::Off,
        crate::SearchedLinks::Hash,
        crate::SearchedLinks::Seek,
    ] {
        conn.set_parallel_searched_links(links);
        let before = counts(&conn);
        for (sql, rows) in queries {
            conn.set_parallel_mode(ParallelMode::Off);
            let serial = sql_rows(&conn, sql);
            conn.set_parallel_mode(ParallelMode::On);
            for cores in [1usize, 2, 3, 4, 8] {
                *conn.morsel_costs.lock() = Some(MachineCosts::stated(
                    NonZeroUsize::new(cores).unwrap(),
                    1.0,
                    1.0,
                ));
                let parallel = sql_rows(&conn, sql);
                same_answer(
                    sql,
                    rows,
                    &parallel,
                    &serial,
                    &format!("on {cores} cores, searched links {links:?}"),
                );
            }
        }
        conn.set_parallel_mode(ParallelMode::Off);
        let after = counts(&conn);
        let [searched, seeks] = [0, 1].map(|at| after[at] - before[at]);
        match links {
            crate::SearchedLinks::Off => assert_eq!(
                (searched, seeks),
                (0, 0),
                "{links:?}: a searched table was linked"
            ),
            crate::SearchedLinks::Hash => {
                assert!(searched > 0, "{links:?}: no link from a searched table");
                assert_eq!(seeks, 0, "{links:?}: a link was read by seeks");
            }
            crate::SearchedLinks::Seek => {
                assert!(seeks > 0, "{links:?}: no link was read by seeks")
            }
        }
    }
    conn.set_parallel_searched_links(crate::SearchedLinks::Off);
}

/// T-QT.6.29 (lane qs6-17): a table with columns added by ALTER TABLE ADD COLUMN, each with a
/// DEFAULT, runs on morsels: a row written before a column was added reads its default (folded with
/// the column's affinity: TEXT DEFAULT 0 reads as the text '0'), as the serial loop's Column reads
/// it. Scans' aggregates, groups and rows, and joins whose either side has such a column, answer
/// as the serial loop at 1, 2, 3, 4 and 8 stated cores, and the hook ran each (its body's rows or
/// its links counted), where before it refused every table with a default.
#[test]
fn t_qt_6_29_short_rows_read_their_columns_defaults_as_the_serial_loop() {
    let conn = open(":memory:qt-6-29");
    // The join cases exercise the hash morsel worker. A chosen merge join stays serial.
    conn.set_join_methods(crate::JoinMethods::HashJoinsOnly);
    let mut rng = ChaCha8Rng::seed_from_u64(629);
    let pad = "x".repeat(64);
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, a INTEGER, p TEXT)")
        .unwrap();
    conn.execute("CREATE TABLE u(id INTEGER PRIMARY KEY, k INTEGER, p TEXT)")
        .unwrap();
    let insert = |conn: &Arc<Connection>, rng: &mut ChaCha8Rng, sql: &str, rows: usize| {
        conn.execute("BEGIN").unwrap();
        for _ in 0..rows {
            let a = rng.random_range(-20i64..20);
            let d = match rng.random_range(0..4) {
                0 => "NULL".to_string(),
                _ => rng.random_range(0i64..9).to_string(),
            };
            let e = ["NULL", "'y'", "'z'", "'zz'"][rng.random_range(0..4)];
            conn.execute(
                sql.replace("{a}", &a.to_string())
                    .replace("{d}", &d)
                    .replace("{e}", e)
                    .replace("{pad}", &pad),
            )
            .unwrap();
        }
        conn.execute("COMMIT").unwrap();
    };
    // Rows written before the columns exist: short rows, which read the defaults.
    insert(
        &conn,
        &mut rng,
        "INSERT INTO t(a, p) VALUES ({a}, '{pad}')",
        1500,
    );
    insert(
        &conn,
        &mut rng,
        "INSERT INTO u(k, p) VALUES ({a}, '{pad}')",
        300,
    );
    conn.execute("ALTER TABLE t ADD COLUMN d INTEGER DEFAULT 7")
        .unwrap();
    conn.execute("ALTER TABLE t ADD COLUMN e TEXT DEFAULT 'z'")
        .unwrap();
    conn.execute("ALTER TABLE t ADD COLUMN f TEXT DEFAULT 0")
        .unwrap();
    conn.execute("ALTER TABLE u ADD COLUMN g INTEGER DEFAULT -3")
        .unwrap();
    // Rows written after: whole rows, their own values (NULL among them).
    insert(
        &conn,
        &mut rng,
        "INSERT INTO t(a, p, d, e, f) VALUES ({a}, '{pad}', {d}, {e}, {d})",
        1000,
    );
    insert(
        &conn,
        &mut rng,
        "INSERT INTO u(k, p, g) VALUES ({a}, '{pad}', {d})",
        200,
    );
    let queries: [(&str, bool); 12] = [
        ("SELECT count(*), sum(d), min(e), max(f) FROM t", false),
        ("SELECT d, count(*) FROM t GROUP BY d", false),
        (
            "SELECT e, sum(a), count(d) FROM t WHERE d = 7 GROUP BY e",
            false,
        ),
        ("SELECT count(*) FROM t WHERE e = 'z'", false),
        ("SELECT f, count(*) FROM t WHERE f = '0' GROUP BY f", false),
        (
            "SELECT count(*), sum(d + a) FROM t WHERE e IS NOT NULL AND d > 3",
            false,
        ),
        ("SELECT id, d, e, f FROM t WHERE a > 10", true),
        ("SELECT id, d || e, f FROM t WHERE e = 'z' AND a < 0", true),
        (
            "SELECT t.d, count(*), sum(u.g) FROM t JOIN u ON t.a = u.k GROUP BY t.d",
            false,
        ),
        (
            "SELECT u.g, count(*), max(t.e) FROM t JOIN u ON t.a = u.k WHERE u.g = -3 GROUP BY u.g",
            false,
        ),
        (
            "SELECT t.id, u.id, t.d, u.g FROM t JOIN u ON t.a = u.k WHERE t.d = 7",
            true,
        ),
        (
            "SELECT t.id, u.id, u.g FROM t JOIN u ON t.d = u.g ORDER BY t.id, u.id -- keys 2",
            true,
        ),
    ];
    let ran = |conn: &Arc<Connection>| {
        [&conn.morsel_body_rows, &conn.morsel_join_links]
            .map(|c| c.load(std::sync::atomic::Ordering::Relaxed))
    };
    for (sql, rows) in queries {
        conn.set_parallel_mode(ParallelMode::Off);
        let serial = sql_rows(&conn, sql);
        assert!(!serial.is_empty(), "{sql}: no rows to compare");
        conn.set_parallel_mode(ParallelMode::On);
        for cores in [1usize, 2, 3, 4, 8] {
            *conn.morsel_costs.lock() = Some(MachineCosts::stated(
                NonZeroUsize::new(cores).unwrap(),
                1.0,
                1.0,
            ));
            let before = ran(&conn);
            let parallel = sql_rows(&conn, sql);
            let after = ran(&conn);
            same_answer(sql, rows, &parallel, &serial, &format!("on {cores} cores"));
            // Non-vacuity: the hook took the statement (it refused every such table before).
            assert!(
                after[0] > before[0] || after[1] > before[1],
                "{sql} on {cores} cores: the hook didn't run"
            );
        }
    }
    conn.set_parallel_mode(ParallelMode::Off);
    // The defaults the short rows read are the ones asked: the test isn't vacuous on them.
    let short = sql_rows(&conn, "SELECT d, e, f FROM t WHERE id = 1");
    assert_eq!(
        short,
        vec![vec![
            Value::from_i64(7),
            Value::build_text("z"),
            Value::build_text("0"),
        ]]
    );
}

/// T-QT.6.30 (lane qs6-17): AVG and TOTAL over integers run on morsels, in a scan's copied body and
/// over joined rows, grouped or not, over expressions and with NULLs and empty groups, and answer
/// as the serial loop, bit for bit, at 1, 2, 3, 4 and 8 stated cores: the hook ran each (its
/// body's rows or its links counted). Past a double's exact integers (2^53), or over a REAL, the
/// statement goes to the serial loop, which answers.
#[test]
fn t_qt_6_30_avg_and_total_run_on_morsels_as_the_serial_loop() {
    let conn = open(":memory:qt-6-30");
    conn.set_join_methods(crate::JoinMethods::HashJoinsOnly);
    let mut rng = ChaCha8Rng::seed_from_u64(630);
    let pad = "x".repeat(64);
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, a INTEGER, b INTEGER, c TEXT, p TEXT)")
        .unwrap();
    conn.execute("CREATE TABLE u(id INTEGER PRIMARY KEY, k INTEGER, g INTEGER, r REAL, p TEXT)")
        .unwrap();
    conn.execute("BEGIN").unwrap();
    for _ in 0..2500 {
        let a = rng.random_range(-30i64..30);
        let b = match rng.random_range(0..5) {
            0 => "NULL".to_string(),
            _ => rng.random_range(-1_000_000i64..1_000_000).to_string(),
        };
        let c = ["NULL", "'x'", "'y'", "'z'"][rng.random_range(0..4)];
        conn.execute(format!(
            "INSERT INTO t(a, b, c, p) VALUES ({a}, {b}, {c}, '{pad}')"
        ))
        .unwrap();
    }
    for _ in 0..300 {
        let k = rng.random_range(-30i64..30);
        let g = rng.random_range(-7i64..7);
        let r = f64::from(rng.random_range(-1000i32..1000)) / 7.0;
        conn.execute(format!(
            "INSERT INTO u(k, g, r, p) VALUES ({k}, {g}, {r}, '{pad}')"
        ))
        .unwrap();
    }
    // Two rows whose magnitudes pass 2^53 together, behind a filter only some statements keep.
    conn.execute("INSERT INTO t(a, b, c, p) VALUES (99, 6000000000000000, 'big', 'q')")
        .unwrap();
    conn.execute("INSERT INTO t(a, b, c, p) VALUES (99, -5000000000000000, 'big', 'q')")
        .unwrap();
    conn.execute("COMMIT").unwrap();
    let ran = |conn: &Arc<Connection>| {
        [&conn.morsel_body_rows, &conn.morsel_join_links]
            .map(|c| c.load(std::sync::atomic::Ordering::Relaxed))
    };
    // (statement, whether the hook answers it: false where the serial loop must).
    let queries: [(&str, bool); 12] = [
        ("SELECT avg(a), total(a), avg(b), total(b) FROM t WHERE a < 99", true),
        ("SELECT c, avg(b), total(a * 3 - b), count(*) FROM t WHERE a < 99 GROUP BY c", true),
        ("SELECT a % 5, avg(a + b), total(b) FROM t WHERE c <> 'big' GROUP BY a % 5", true),
        ("SELECT avg(b), total(b) FROM t WHERE a > 1000", true),
        ("SELECT avg(CASE WHEN c = 'x' THEN b END), total(CASE WHEN c = 'y' THEN 1 END) FROM t WHERE a < 99", true),
        ("SELECT avg(a), total(a) FROM t WHERE c = 'nothing'", true),
        ("SELECT t.c, avg(u.g), total(t.b + u.g) FROM t JOIN u ON t.a = u.k GROUP BY t.c", true),
        ("SELECT avg(t.b), total(u.g), avg(u.g) FROM t JOIN u ON t.a = u.k WHERE u.g > 0", true),
        ("SELECT avg(u.g), total(u.g), count(t.id) FROM u LEFT JOIN t ON t.a = u.k AND t.b > 900000", true),
        // The serial loop's: a sum past 2^53, a REAL.
        ("SELECT avg(b), total(b) FROM t", false),
        ("SELECT avg(r), total(r) FROM u", false),
        ("SELECT t.c, avg(u.r) FROM t JOIN u ON t.a = u.k GROUP BY t.c", false),
    ];
    for (sql, hooked) in queries {
        conn.set_parallel_mode(ParallelMode::Off);
        let serial = sql_rows(&conn, sql);
        assert!(!serial.is_empty(), "{sql}: no rows to compare");
        conn.set_parallel_mode(ParallelMode::On);
        for cores in [1usize, 2, 3, 4, 8] {
            *conn.morsel_costs.lock() = Some(MachineCosts::stated(
                NonZeroUsize::new(cores).unwrap(),
                1.0,
                1.0,
            ));
            let before = ran(&conn);
            let parallel = sql_rows(&conn, sql);
            let after = ran(&conn);
            // Bit for bit: a REAL's Debug is its exact digits.
            assert_eq!(
                format!("{parallel:?}"),
                format!("{serial:?}"),
                "{sql} on {cores} cores"
            );
            if hooked {
                assert!(
                    after[0] > before[0] || after[1] > before[1],
                    "{sql} on {cores} cores: the hook didn't run"
                );
            }
        }
    }
    conn.set_parallel_mode(ParallelMode::Off);
}

#[test]
fn an_aggregate_filter_can_read_a_column_absent_from_its_input() {
    let conn = open(":memory:qt-filter-column");
    conn.execute("CREATE TABLE t(v INTEGER, keep INTEGER)")
        .unwrap();
    conn.execute("INSERT INTO t VALUES (1, 0), (4, 1), (8, NULL)")
        .unwrap();
    let sql = "SELECT sum(v) FILTER (WHERE keep > 0) FROM t";
    for mode in [ParallelMode::Off, ParallelMode::On] {
        conn.set_parallel_mode(mode);
        assert_eq!(sql_rows(&conn, sql), vec![vec![Value::from_i64(4)]]);
    }
}

/// T-QT.6.31 (lane qs6-17): aggregates with a FILTER run on morsels: a row that fails an
/// aggregate's FILTER gives its input NULL in the copied body, which every aggregate the pipeline
/// folds skips. COUNT(*), COUNT, SUM, MIN, MAX, AVG and TOTAL with a FILTER, beside unfiltered
/// ones, grouped or not, in scans and joins, with FILTERs no row passes, answer as the serial loop
/// at 1, 2, 3, 4 and 8 stated cores, and the hook ran each.
#[test]
fn t_qt_6_31_filtered_aggregates_run_on_morsels_as_the_serial_loop() {
    let conn = open(":memory:qt-6-31");
    conn.set_join_methods(crate::JoinMethods::HashJoinsOnly);
    let mut rng = ChaCha8Rng::seed_from_u64(631);
    let pad = "x".repeat(64);
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, a INTEGER, b INTEGER, c TEXT, p TEXT)")
        .unwrap();
    conn.execute("CREATE TABLE u(id INTEGER PRIMARY KEY, k INTEGER, g INTEGER, p TEXT)")
        .unwrap();
    conn.execute("BEGIN").unwrap();
    for _ in 0..2500 {
        let a = rng.random_range(-30i64..30);
        let b = match rng.random_range(0..5) {
            0 => "NULL".to_string(),
            _ => rng.random_range(-1000i64..1000).to_string(),
        };
        let c = ["NULL", "'x'", "'y'", "'z'"][rng.random_range(0..4)];
        conn.execute(format!(
            "INSERT INTO t(a, b, c, p) VALUES ({a}, {b}, {c}, '{pad}')"
        ))
        .unwrap();
    }
    for _ in 0..300 {
        let k = rng.random_range(-30i64..30);
        let g = rng.random_range(-7i64..7);
        conn.execute(format!("INSERT INTO u(k, g, p) VALUES ({k}, {g}, '{pad}')"))
            .unwrap();
    }
    conn.execute("COMMIT").unwrap();
    let ran = |conn: &Arc<Connection>| {
        [&conn.morsel_body_rows, &conn.morsel_join_links]
            .map(|c| c.load(std::sync::atomic::Ordering::Relaxed))
    };
    let queries = [
        "SELECT count(*) FILTER (WHERE a > 0), count(*), sum(b) FILTER (WHERE c = 'x') FROM t",
        "SELECT c, count(b) FILTER (WHERE a % 2 = 0), min(b) FILTER (WHERE b > 10), max(a) FILTER (WHERE c IS NULL) FROM t GROUP BY c",
        "SELECT a % 4, avg(b) FILTER (WHERE b IS NOT NULL AND a < 5), total(b) FILTER (WHERE c <> 'y') FROM t GROUP BY a % 4",
        "SELECT count(*) FILTER (WHERE a > 1000), sum(b) FILTER (WHERE a > 1000), total(a) FILTER (WHERE a > 1000), avg(a) FILTER (WHERE 0) FROM t",
        "SELECT count(*) FILTER (WHERE b), sum(a) FILTER (WHERE c LIKE 'x%') FROM t WHERE a <> 3",
        "SELECT t.c, count(*) FILTER (WHERE u.g > 0), sum(t.b) FILTER (WHERE u.g < 0) FROM t JOIN u ON t.a = u.k GROUP BY t.c",
        "SELECT count(*) FILTER (WHERE t.b > u.g), max(u.g) FILTER (WHERE t.c = 'z') FROM t JOIN u ON t.a = u.k",
        "SELECT count(t.id) FILTER (WHERE t.b > 0), count(*) FROM u LEFT JOIN t ON t.a = u.k AND t.c = 'x'",
    ];
    for sql in queries {
        conn.set_parallel_mode(ParallelMode::Off);
        let serial = sql_rows(&conn, sql);
        assert!(!serial.is_empty(), "{sql}: no rows to compare");
        conn.set_parallel_mode(ParallelMode::On);
        for cores in [1usize, 2, 3, 4, 8] {
            *conn.morsel_costs.lock() = Some(MachineCosts::stated(
                NonZeroUsize::new(cores).unwrap(),
                1.0,
                1.0,
            ));
            let before = ran(&conn);
            let parallel = sql_rows(&conn, sql);
            let after = ran(&conn);
            assert_eq!(
                format!("{parallel:?}"),
                format!("{serial:?}"),
                "{sql} on {cores} cores"
            );
            assert!(
                after[0] > before[0] || after[1] > before[1],
                "{sql} on {cores} cores: the hook didn't run"
            );
        }
    }
    conn.set_parallel_mode(ParallelMode::Off);
}

/// T-QT.6.32 (lane qs6-17): a GROUP BY with a HAVING over its aggregates runs on morsels: every
/// aggregate's value is loaded after the result columns and the hook's row loop tests the
/// HAVING's terms on them, skipping a group that fails, as the serial loop does. HAVINGs over
/// aggregates in the result columns and aggregates only the HAVING names, ANDed and ORed, with
/// NULL aggregates, none passing, and a HAVING with no GROUP BY, in scans and joins, answer as the
/// serial loop at 1, 2, 3, 4 and 8 stated cores, and the hook ran each. A HAVING over a group key
/// or a bare column, which would lose the column's affinity read through a register, stays the
/// serial loop's.
#[test]
fn t_qt_6_32_having_over_aggregates_runs_on_morsels_as_the_serial_loop() {
    let conn = open(":memory:qt-6-32");
    conn.set_join_methods(crate::JoinMethods::HashJoinsOnly);
    let mut rng = ChaCha8Rng::seed_from_u64(632);
    let pad = "x".repeat(64);
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, a INTEGER, b INTEGER, c TEXT, p TEXT)")
        .unwrap();
    conn.execute("CREATE TABLE u(id INTEGER PRIMARY KEY, k INTEGER, g INTEGER, p TEXT)")
        .unwrap();
    conn.execute("BEGIN").unwrap();
    for _ in 0..2500 {
        let a = rng.random_range(-30i64..30);
        let b = match rng.random_range(0..5) {
            0 => "NULL".to_string(),
            _ => rng.random_range(-1000i64..1000).to_string(),
        };
        let c = ["NULL", "'x'", "'y'", "'z'"][rng.random_range(0..4)];
        conn.execute(format!(
            "INSERT INTO t(a, b, c, p) VALUES ({a}, {b}, {c}, '{pad}')"
        ))
        .unwrap();
    }
    for _ in 0..300 {
        let k = rng.random_range(-30i64..30);
        let g = rng.random_range(-7i64..7);
        conn.execute(format!("INSERT INTO u(k, g, p) VALUES ({k}, {g}, '{pad}')"))
            .unwrap();
    }
    conn.execute("COMMIT").unwrap();
    let ran = |conn: &Arc<Connection>| {
        [&conn.morsel_body_rows, &conn.morsel_join_links]
            .map(|c| c.load(std::sync::atomic::Ordering::Relaxed))
    };
    // (statement, whether the hook answers it).
    let queries: [(&str, bool); 10] = [
        ("SELECT a, count(*) FROM t GROUP BY a HAVING count(*) > 40", true),
        ("SELECT a % 7, sum(b) FROM t GROUP BY a % 7 HAVING sum(b) > 0 AND max(b) > 990", true),
        ("SELECT c, count(*) FROM t GROUP BY c HAVING min(b) < -995 OR count(b) < 450", true),
        ("SELECT a, avg(b) FROM t GROUP BY a HAVING sum(b) FILTER (WHERE c = 'x') IS NULL OR avg(b) > 30", true),
        ("SELECT a, count(*) FROM t GROUP BY a HAVING count(*) > 100000", true),
        ("SELECT count(*), sum(b) FROM t HAVING count(*) > 10", true),
        ("SELECT t.c, count(*), sum(u.g) FROM t JOIN u ON t.a = u.k GROUP BY t.c HAVING sum(u.g) <> 0 AND count(*) > 10", true),
        ("SELECT u.g, count(t.id) FROM u LEFT JOIN t ON t.a = u.k GROUP BY u.g HAVING count(t.id) > 50", true),
        // The serial loop's: a group key, a bare column.
        ("SELECT c, count(*) FROM t GROUP BY c HAVING c <> 'y' AND count(*) > 1", false),
        ("SELECT a % 3, count(*) FROM t GROUP BY a % 3 HAVING max(b) > b", false),
    ];
    for (sql, hooked) in queries {
        conn.set_parallel_mode(ParallelMode::Off);
        let serial = sql_rows(&conn, sql);
        conn.set_parallel_mode(ParallelMode::On);
        for cores in [1usize, 2, 3, 4, 8] {
            *conn.morsel_costs.lock() = Some(MachineCosts::stated(
                NonZeroUsize::new(cores).unwrap(),
                1.0,
                1.0,
            ));
            let before = ran(&conn);
            let parallel = sql_rows(&conn, sql);
            let after = ran(&conn);
            assert_eq!(
                format!("{parallel:?}"),
                format!("{serial:?}"),
                "{sql} on {cores} cores"
            );
            if hooked {
                assert!(
                    after[0] > before[0] || after[1] > before[1],
                    "{sql} on {cores} cores: the hook didn't run"
                );
            }
        }
    }
    conn.set_parallel_mode(ParallelMode::Off);
    // Non-vacuity: a HAVING kept some groups and dropped others.
    let all = sql_rows(&conn, "SELECT a, count(*) FROM t GROUP BY a").len();
    let kept = sql_rows(
        &conn,
        "SELECT a, count(*) FROM t GROUP BY a HAVING count(*) > 40",
    )
    .len();
    assert!(kept > 0 && kept < all, "{kept} of {all} groups kept");
}

/// T-QT.6.33 (lane qs6-17): a one-table search by a rowid range runs on morsels cut from the
/// leaves that hold the range (`TableSource::restrict`), every term the search was made from
/// tested on each row. Ranges open and closed at either end, through the INTEGER PRIMARY KEY and
/// through rowid, with other terms, aggregates, groups, rows and a LIMIT, over a table with gaps,
/// answer as the serial loop at 1, 2, 3, 4 and 8 stated cores; the body ran over some rows, and
/// no more than the leaves the range touches hold (`TableSource::restrict`: the first leaf whose
/// greatest rowid reaches the lower bound, through the first that reaches the upper), fewer than
/// the table holds where the range leaves whole leaves out. A range whose bound falls in the
/// first or last leaf touches them all (`id > 100`: the first leaf holds ids to ~115). A backward
/// search, and a range of bounds that aren't integer literals, stay the serial loop's.
#[test]
fn t_qt_6_33_a_rowid_range_runs_on_the_morsels_of_its_leaves() {
    let conn = open(":memory:qt-6-33");
    let mut rng = ChaCha8Rng::seed_from_u64(633);
    let pad = "x".repeat(64);
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, a INTEGER, b INTEGER, c TEXT, p TEXT)")
        .unwrap();
    conn.execute("BEGIN").unwrap();
    let mut id = 0i64;
    let rows = 3000u64;
    for _ in 0..rows {
        id += rng.random_range(1i64..4);
        let a = rng.random_range(-30i64..30);
        let b = match rng.random_range(0..5) {
            0 => "NULL".to_string(),
            _ => rng.random_range(-1000i64..1000).to_string(),
        };
        let c = ["NULL", "'x'", "'y'", "'z'"][rng.random_range(0..4)];
        conn.execute(format!(
            "INSERT INTO t(id, a, b, c, p) VALUES ({id}, {a}, {b}, {c}, '{pad}')"
        ))
        .unwrap();
    }
    conn.execute("COMMIT").unwrap();
    // (statement, whether it returns rows, whether the hook takes it, its rowid bounds, each
    // inclusive).
    type Range = (Option<i64>, Option<i64>);
    let queries: [(&str, bool, bool, Range); 11] = [
        (
            "SELECT count(*), sum(b), min(c), max(a) FROM t WHERE id > 4000",
            false,
            true,
            (Some(4001), None),
        ),
        (
            "SELECT count(*), sum(a) FROM t WHERE id >= 1000 AND id <= 2000",
            false,
            true,
            (Some(1000), Some(2000)),
        ),
        (
            "SELECT a % 5, count(*), avg(b) FROM t WHERE id BETWEEN 300 AND 5500 GROUP BY a % 5",
            false,
            true,
            (Some(300), Some(5500)),
        ),
        (
            "SELECT count(*), total(b) FROM t WHERE rowid < 2500 AND a > 0",
            false,
            true,
            (None, Some(2499)),
        ),
        (
            "SELECT c, count(*) FROM t WHERE id > 100 AND id < 7000 AND c IS NOT NULL GROUP BY c",
            false,
            true,
            (Some(101), Some(6999)),
        ),
        (
            "SELECT id, a, b FROM t WHERE id > 6000 AND b > 0",
            true,
            true,
            (Some(6001), None),
        ),
        (
            "SELECT id, c FROM t WHERE id >= 2500 AND id < 2600",
            true,
            true,
            (Some(2500), Some(2599)),
        ),
        (
            "SELECT id, b FROM t WHERE id > 1234 LIMIT 20 OFFSET 5",
            true,
            true,
            (Some(1235), None),
        ),
        (
            "SELECT count(*) FROM t WHERE id > 100000",
            false,
            true,
            (Some(100001), None),
        ),
        // The serial loop's: a backward search, a bound that isn't a literal.
        (
            "SELECT id FROM t WHERE id > 8000 ORDER BY id DESC",
            true,
            false,
            (Some(8001), None),
        ),
        (
            "SELECT count(*) FROM t WHERE id > abs(-50)",
            false,
            false,
            (Some(51), None),
        ),
    ];
    let root = conn
        .schema
        .read()
        .get_table("t")
        .unwrap()
        .get_root_page()
        .unwrap();
    // The rows of the leaves each range touches, as the hook cuts them.
    conn.execute("BEGIN").unwrap();
    let touched: Vec<u64> = queries
        .iter()
        .map(|(_, _, _, (lower, upper))| {
            let mut leaves =
                TableSource::open(conn.get_pager(), root, 5, Some(0), Vec::new()).unwrap();
            leaves.restrict(*lower, *upper);
            u64::try_from(leaves.rows().unwrap().len()).unwrap()
        })
        .collect();
    conn.execute("COMMIT").unwrap();
    // Non-vacuity: some ranges leave whole leaves out.
    assert!(
        touched.iter().any(|rows_touched| *rows_touched < rows),
        "no range cut a leaf: {touched:?}"
    );
    for ((sql, returns_rows, hooked, _), touched) in queries.into_iter().zip(touched) {
        conn.set_parallel_mode(ParallelMode::Off);
        let serial = sql_rows(&conn, sql);
        conn.set_parallel_mode(ParallelMode::On);
        conn.set_parallel_limit(true);
        for cores in [1usize, 2, 3, 4, 8] {
            *conn.morsel_costs.lock() = Some(MachineCosts::stated(
                NonZeroUsize::new(cores).unwrap(),
                1.0,
                1.0,
            ));
            let before = conn
                .morsel_body_rows
                .load(std::sync::atomic::Ordering::Relaxed);
            let parallel = sql_rows(&conn, sql);
            let after = conn
                .morsel_body_rows
                .load(std::sync::atomic::Ordering::Relaxed);
            if returns_rows {
                // In the serial loop's rowid order, row for row.
                assert_eq!(parallel, serial, "{sql} on {cores} cores");
            } else {
                assert_eq!(
                    format!("{parallel:?}"),
                    format!("{serial:?}"),
                    "{sql} on {cores} cores"
                );
            }
            let ran = after - before;
            if hooked {
                assert!(
                    ran <= touched,
                    "{sql} on {cores} cores: the body ran over {ran} rows, its range's leaves hold {touched} of {rows}: the leaves weren't cut"
                );
                if !sql.contains("100000") {
                    assert!(ran > 0, "{sql} on {cores} cores: the hook didn't run");
                }
            }
        }
    }
    conn.set_parallel_limit(false);
    conn.set_parallel_mode(ParallelMode::Off);
}

/// T-QT.6.34 (lane qs6-17): a one-table scan's rows under an ORDER BY run on morsels: each worker
/// sorts its morsels' rows by the keys and a stable merge joins the runs in leaf order. ORDER BYs
/// over INTEGER and TEXT columns and the rowid, ascending and descending, with ties, NULLs, a
/// filter, a rowid range, and a LIMIT and OFFSET over keys that hold the rowid, answer as the
/// serial loop at 1, 2, 3, 4 and 8 stated cores: in the keys' order, the same rows (row for row
/// where the keys tell every row apart), and the body ran over the table's rows. A LIMIT over keys
/// without the rowid, an expression key, NULLS LAST and a backward scan stay the serial loop's.
#[test]
fn t_qt_6_34_a_scans_rows_under_order_by_sort_on_morsels_as_the_serial_loop() {
    let conn = open(":memory:qt-6-34");
    let mut rng = ChaCha8Rng::seed_from_u64(634);
    let pad = "x".repeat(64);
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, a INTEGER, b INTEGER, c TEXT, p TEXT)")
        .unwrap();
    conn.execute("BEGIN").unwrap();
    let rows = 2500u64;
    for _ in 0..rows {
        let a = rng.random_range(-30i64..30);
        let b = match rng.random_range(0..5) {
            0 => "NULL".to_string(),
            _ => rng.random_range(-1000i64..1000).to_string(),
        };
        let c = ["NULL", "'x'", "'y'", "'z'", "'xy'"][rng.random_range(0..5)];
        conn.execute(format!(
            "INSERT INTO t(a, b, c, p) VALUES ({a}, {b}, {c}, '{pad}')"
        ))
        .unwrap();
    }
    conn.execute("COMMIT").unwrap();
    // (statement, whether the hook takes it); `-- keys n`: its keys lead the result columns.
    let queries: [(&str, bool); 12] = [
        ("SELECT a, id, b FROM t ORDER BY a -- keys 1", true),
        ("SELECT a, b, c FROM t ORDER BY a DESC, b -- keys 2", true),
        ("SELECT c, a, id FROM t WHERE a > 0 ORDER BY c, a DESC -- keys 2", true),
        ("SELECT b, id FROM t ORDER BY b DESC -- keys 1", true),
        ("SELECT c, id, a FROM t ORDER BY c DESC, id -- keys 2", true),
        ("SELECT a, id, c FROM t WHERE id > 700 AND id < 1900 ORDER BY a, id DESC -- keys 2", true),
        ("SELECT a, id FROM t ORDER BY a, id LIMIT 30 OFFSET 7 -- keys 2", true),
        ("SELECT c, rowid, b FROM t WHERE b IS NOT NULL ORDER BY c DESC, rowid LIMIT 50 -- keys 2", true),
        // The serial loop's: a LIMIT over keys without the rowid, an expression, NULLS LAST, and
        // the backward scan the planner may read ORDER BY id DESC by.
        ("SELECT id, a FROM t ORDER BY id DESC -- keys 1", false),
        ("SELECT a, id FROM t ORDER BY a LIMIT 10 -- keys 1", false),
        ("SELECT a, id FROM t ORDER BY a + b -- keys 0", false),
        ("SELECT b, id FROM t ORDER BY b NULLS LAST -- keys 1", false),
    ];
    for (sql, hooked) in queries {
        conn.set_parallel_mode(ParallelMode::Off);
        let serial = sql_rows(&conn, sql);
        conn.set_parallel_mode(ParallelMode::On);
        for cores in [1usize, 2, 3, 4, 8] {
            *conn.morsel_costs.lock() = Some(MachineCosts::stated(
                NonZeroUsize::new(cores).unwrap(),
                1.0,
                1.0,
            ));
            let before = conn
                .morsel_body_rows
                .load(std::sync::atomic::Ordering::Relaxed);
            let parallel = sql_rows(&conn, sql);
            let after = conn
                .morsel_body_rows
                .load(std::sync::atomic::Ordering::Relaxed);
            if sql.contains("LIMIT") {
                // The keys hold the rowid: the very rows, in order.
                assert_eq!(parallel, serial, "{sql} on {cores} cores");
            } else {
                same_answer(sql, true, &parallel, &serial, &format!("on {cores} cores"));
            }
            if hooked {
                assert!(
                    after > before,
                    "{sql} on {cores} cores: the hook didn't run"
                );
            }
        }
    }
    conn.set_parallel_mode(ParallelMode::Off);
}

/// T-QT.6.35 (lane qs6-17): a join whose tables the planner reads by a rowid range runs on
/// morsels: an inner join's probe and links read only the leaves that hold their ranges, and an
/// outer join's are read whole, every term tested on the joined row. Inner joins of two and three
/// tables with ranges on the probe, a link or both, with aggregates, groups and rows (ordered by
/// the ids under a LIMIT too), and LEFT and FULL joins with ranges in their WHERE and ON terms,
/// answer as the serial loop at 1, 2, 3, 4 and 8 stated cores, and the hook built links for each.
#[test]
fn t_qt_6_35_joins_over_rowid_ranges_run_on_morsels_as_the_serial_loop() {
    let conn = open(":memory:qt-6-35");
    let mut rng = ChaCha8Rng::seed_from_u64(635);
    fill_with_ids(&conn, &mut rng, &[("f", 3000), ("d", 600), ("e", 300)]);
    let queries: [(&str, bool); 9] = [
        ("SELECT count(*), sum(d.b) FROM f JOIN d ON f.a = d.a WHERE f.id > 2000", false),
        ("SELECT d.c, count(*), min(f.b) FROM f JOIN d ON f.c = d.c WHERE f.id BETWEEN 500 AND 1500 AND d.id < 300 GROUP BY d.c", false),
        ("SELECT f.id, d.id, d.b FROM f JOIN d ON f.a = d.a WHERE f.id >= 2900 AND d.id > 550", true),
        ("SELECT count(*), sum(e.b) FROM f JOIN d ON f.a = d.a JOIN e ON d.c = e.c WHERE d.id <= 100 AND e.id > 200", false),
        ("SELECT f.id, d.id FROM f JOIN d ON f.a = d.a WHERE f.id < 400 ORDER BY f.id, d.id LIMIT 9 OFFSET 2 -- keys 2", true),
        ("SELECT count(*), count(d.id), sum(d.b) FROM f LEFT JOIN d ON f.a = d.a AND d.id > 500 WHERE f.id > 2500", false),
        ("SELECT f.id, d.b FROM f LEFT JOIN d ON f.a = d.a WHERE f.id > 2950", true),
        ("SELECT count(*), count(f.id), count(d.id) FROM f FULL JOIN d ON f.a = d.a AND d.id > 590", false),
        ("SELECT count(*), sum(f.b) FROM f JOIN d ON f.a = d.a AND d.id < 50 WHERE f.id > 100", false),
    ];
    for (sql, rows) in queries {
        conn.set_parallel_mode(ParallelMode::Off);
        let serial = sql_rows(&conn, sql);
        conn.set_parallel_mode(ParallelMode::On);
        for cores in [1usize, 2, 3, 4, 8] {
            *conn.morsel_costs.lock() = Some(MachineCosts::stated(
                NonZeroUsize::new(cores).unwrap(),
                1.0,
                1.0,
            ));
            let before = conn
                .morsel_join_links
                .load(std::sync::atomic::Ordering::Relaxed);
            let parallel = sql_rows(&conn, sql);
            let after = conn
                .morsel_join_links
                .load(std::sync::atomic::Ordering::Relaxed);
            same_answer(sql, rows, &parallel, &serial, &format!("on {cores} cores"));
            assert!(
                after > before,
                "{sql} on {cores} cores: the hook built no link"
            );
        }
    }
    conn.set_parallel_mode(ParallelMode::Off);
}

/// Audit 3, prototypes 2 (morsel parallelism for REAL and NUMERIC): a one-table scan reading
/// columns of REAL and NUMERIC affinity runs its loop body's copy in the workers, a REAL column's
/// INTEGER loading as a REAL (`typeof` says so), GROUP BY values and MIN and MAX comparing as
/// numbers, and answers as the serial loop at 1, 2, 4, 8 and 16 stated cores. A SUM, AVG or TOTAL
/// over REALs still sends the statement to the serial loop (its compensated double sum is not the
/// exact one), which answers the same.
#[test]
fn audit_3_p2_real_and_numeric_columns_run_in_parallel_as_the_serial_loop() {
    if crate::storage::versioned::switch::pager_only(
        "its subject is the morsel hook over the pager's leaf pages (that the workers ran); on the versioned storage the hook hands the statement to the table's column copy or the serial loop (G11 step 5, CS.5)",
    ) {
        return;
    }
    let conn = open(":memory:audit-3-p2");
    let mut rng = ChaCha8Rng::seed_from_u64(32);
    let pad = "x".repeat(64);
    let rows = 2500u64;
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, a INTEGER, r REAL, n NUMERIC, p TEXT)")
        .unwrap();
    let reals = [
        "NULL", "0", "-0.0", "1", "2.5", "-3", "3.0", "1e300", "-2.5", "'7'", "'x'",
    ];
    let numerics = [
        "NULL", "0", "1", "'2'", "2.5", "'3.0'", "1e30", "-1.5", "'abc'", "4",
    ];
    conn.execute("BEGIN").unwrap();
    for _ in 0..rows {
        let a = rng.random_range(-50i64..50);
        let r = reals[rng.random_range(0..reals.len())];
        let n = numerics[rng.random_range(0..numerics.len())];
        conn.execute(format!(
            "INSERT INTO t(a, r, n, p) VALUES ({a}, {r}, {n}, '{pad}')"
        ))
        .unwrap();
    }
    conn.execute("COMMIT").unwrap();
    let parallel_queries = [
        "SELECT COUNT(*), MIN(r), MAX(r), MIN(n), MAX(n) FROM t",
        "SELECT r, COUNT(*), SUM(a), MIN(n), MAX(n) FROM t GROUP BY r",
        "SELECT n, COUNT(*), MIN(r), MAX(a) FROM t GROUP BY n",
        "SELECT typeof(r), typeof(n), COUNT(*), MIN(r), MAX(n) FROM t GROUP BY typeof(r), typeof(n)",
        "SELECT COUNT(*), SUM(a) FROM t WHERE r < 2",
        "SELECT COUNT(*), SUM(a) FROM t WHERE n >= 2 AND r BETWEEN -3 AND 3",
        "SELECT r * 2, COUNT(*) FROM t WHERE n <> 1 GROUP BY r * 2",
        "SELECT COUNT(*) FROM t WHERE r = 3 OR n = 3",
        "SELECT MIN(r + n), MAX(r - a) FROM t WHERE r IS NOT NULL",
        "SELECT id, r, n, typeof(r) FROM t WHERE r > 0 AND n < 3",
    ];
    let serial_queries = [
        "SELECT SUM(r) FROM t",
        "SELECT AVG(r), TOTAL(n) FROM t WHERE a > 0",
    ];
    let mut hooked = 0usize;
    for sql in parallel_queries.iter().chain(&serial_queries) {
        let parallel_expected = parallel_queries.contains(sql);
        conn.set_parallel_mode(ParallelMode::Off);
        let serial = sql_rows(&conn, sql);
        conn.set_parallel_mode(ParallelMode::On);
        for cores in [1usize, 2, 4, 8, 16] {
            *conn.morsel_costs.lock() = Some(MachineCosts::stated(
                NonZeroUsize::new(cores).unwrap(),
                1.0,
                1.0,
            ));
            let before = conn
                .morsel_body_rows
                .load(std::sync::atomic::Ordering::Relaxed);
            let parallel = sql_rows(&conn, sql);
            let after = conn
                .morsel_body_rows
                .load(std::sync::atomic::Ordering::Relaxed);
            assert_eq!(parallel, serial, "{sql} on {cores} cores");
            if parallel_expected {
                // The copied body ran over every row once: the parallel path answered.
                assert_eq!(
                    after - before,
                    rows,
                    "{sql} on {cores} cores: the body's rows"
                );
                if conn
                    .morsel_costs
                    .lock()
                    .as_ref()
                    .is_some_and(|costs| costs.worker_start().samples() > 1)
                {
                    hooked += 1;
                }
            }
        }
    }
    conn.set_parallel_mode(ParallelMode::Off);
    // Non-vacuity: more than one worker ran the body for most statements.
    assert!(
        hooked > parallel_queries.len(),
        "workers ran {hooked} times"
    );
}

/// The answer at each stated core count, after the serial loop's, with the copied body run over
/// every one of `rows` rows each time (the parallel path answered, not the serial loop).
fn g8_answers_at_every_core_count(conn: &Arc<Connection>, sql: &str, rows: u64) -> String {
    conn.set_parallel_mode(ParallelMode::Off);
    let serial = format!("{:?}", sql_rows(conn, sql));
    conn.set_parallel_mode(ParallelMode::On);
    for cores in [1usize, 2, 4, 8, 16] {
        *conn.morsel_costs.lock() = Some(MachineCosts::stated(
            NonZeroUsize::new(cores).unwrap(),
            1.0,
            1.0,
        ));
        let before = conn
            .morsel_body_rows
            .load(std::sync::atomic::Ordering::Relaxed);
        let parallel = format!("{:?}", sql_rows(conn, sql));
        let after = conn
            .morsel_body_rows
            .load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(parallel, serial, "{sql} on {cores} cores");
        assert_eq!(
            after - before,
            rows,
            "{sql} on {cores} cores: the body's rows"
        );
    }
    conn.set_parallel_mode(ParallelMode::Off);
    serial
}

/// Lane G8, arm (b) of the SUM-over-REAL A/B (`RealSums::Exact`): SUM, AVG and TOTAL over REALs,
/// INTEGERs and both mixed run on the workers and answer bit for bit as the serial loop at 1, 2,
/// 4, 8 and 16 stated cores, grouped or not, on prices, magnitudes from 1e-300 to 1e300 and
/// integers past 2^53 before and after the first REAL.
#[test]
fn g8_exact_real_sums_answer_as_the_serial_loop_at_every_worker_count() {
    if crate::storage::versioned::switch::pager_only(
        "its subject is the morsel hook over the pager's leaf pages (that the workers ran)",
    ) {
        return;
    }
    let conn = open(":memory:g8-exact");
    conn.set_real_sums(crate::RealSums::Exact);
    let mut rng = ChaCha8Rng::seed_from_u64(88);
    let pad = "x".repeat(64);
    let rows = 3000u64;
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, g INTEGER, r REAL, a INTEGER, p TEXT)")
        .unwrap();
    let reals = [
        "NULL", "-0.0", "1e300", "-1e300", "1e-300", "5e-324", "0.1", "-0.3", "1e16",
    ];
    let integers = [
        "NULL",
        "0",
        "1",
        "-7",
        "9007199254740993",
        "-4611686018427387905",
    ];
    conn.execute("BEGIN").unwrap();
    for _ in 0..rows {
        let g = rng.random_range(0i64..4);
        let r = if rng.random_range(0..3) == 0 {
            reals[rng.random_range(0..reals.len())].to_string()
        } else {
            format!(
                "{}.{:02}",
                rng.random_range(-100_000i64..100_000),
                rng.random_range(0..100)
            )
        };
        let a = integers[rng.random_range(0..integers.len())];
        conn.execute(format!(
            "INSERT INTO t(g, r, a, p) VALUES ({g}, {r}, {a}, '{pad}')"
        ))
        .unwrap();
    }
    conn.execute("COMMIT").unwrap();
    for sql in [
        "SELECT SUM(r), AVG(r), TOTAL(r) FROM t",
        "SELECT g, SUM(r), AVG(r), TOTAL(r), COUNT(*) FROM t GROUP BY g",
        "SELECT SUM(r * 3), AVG(r + a), TOTAL(r - g) FROM t WHERE g > 0",
        "SELECT g, SUM(CASE WHEN id % 3 = 0 THEN a ELSE r END), TOTAL(a), AVG(a) FROM t GROUP BY g",
    ] {
        g8_answers_at_every_core_count(&conn, sql, rows);
    }
}

/// Lane G9, arm (a) of the SUM-over-REAL A/B (`RealSums::RowOrder`): SUM, AVG and TOTAL over
/// REALs, INTEGERs and both mixed run on the workers and answer bit for bit as today's serial loop
/// (`RealSums::Serial`) at 1, 2, 4, 8 and 16 stated cores, grouped or not, on G8's data: prices,
/// magnitudes from 1e-300 to 1e300, -0.0 and integers past 2^53 before and after the first REAL.
#[test]
fn g9_row_order_real_sums_answer_as_today_at_every_worker_count() {
    if crate::storage::versioned::switch::pager_only(
        "its subject is the morsel hook over the pager's leaf pages (that the workers ran)",
    ) {
        return;
    }
    let conn = open(":memory:g9-row-order");
    conn.set_real_sums(crate::RealSums::RowOrder);
    let mut rng = ChaCha8Rng::seed_from_u64(88);
    let pad = "x".repeat(64);
    let rows = 3000u64;
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, g INTEGER, r REAL, a INTEGER, p TEXT)")
        .unwrap();
    let reals = [
        "NULL", "-0.0", "1e300", "-1e300", "1e-300", "5e-324", "0.1", "-0.3", "1e16",
    ];
    let integers = [
        "NULL",
        "0",
        "1",
        "-7",
        "9007199254740993",
        "-4611686018427387905",
    ];
    conn.execute("BEGIN").unwrap();
    for _ in 0..rows {
        let g = rng.random_range(0i64..4);
        let r = if rng.random_range(0..3) == 0 {
            reals[rng.random_range(0..reals.len())].to_string()
        } else {
            format!(
                "{}.{:02}",
                rng.random_range(-100_000i64..100_000),
                rng.random_range(0..100)
            )
        };
        let a = integers[rng.random_range(0..integers.len())];
        conn.execute(format!(
            "INSERT INTO t(g, r, a, p) VALUES ({g}, {r}, {a}, '{pad}')"
        ))
        .unwrap();
    }
    conn.execute("COMMIT").unwrap();
    for sql in [
        "SELECT SUM(r), AVG(r), TOTAL(r) FROM t",
        "SELECT g, SUM(r), AVG(r), TOTAL(r), COUNT(*) FROM t GROUP BY g",
        "SELECT SUM(r * 3), AVG(r + a), TOTAL(r - g) FROM t WHERE g > 0",
        "SELECT g, SUM(CASE WHEN id % 3 = 0 THEN a ELSE r END), TOTAL(a), AVG(a) FROM t GROUP BY g",
    ] {
        let row_order = g8_answers_at_every_core_count(&conn, sql, rows);
        conn.set_real_sums(crate::RealSums::Serial);
        let today = format!("{:?}", sql_rows(&conn, sql));
        conn.set_real_sums(crate::RealSums::RowOrder);
        assert_eq!(row_order, today, "{sql}: RowOrder against Serial");
    }
}

/// Lane G8, arm (b): the exact sum rounded once, on the serial loop and the workers alike, on the
/// cases a compensated sum can get wrong or that SQLite gives meaning to: cancellation, huge with
/// tiny, subnormals, both signs of zero, infinities (both met: NULL).
#[test]
fn g8_exact_real_sums_hold_the_hard_cases() {
    if crate::storage::versioned::switch::pager_only(
        "its subject is the morsel hook over the pager's leaf pages (that the workers ran)",
    ) {
        return;
    }
    let conn = open(":memory:g8-hard");
    conn.set_real_sums(crate::RealSums::Exact);
    let pad = "x".repeat(64);
    let rows = 1500usize;
    // Each case's exact sum from the values as the engine parsed them (its decimal parsing is
    // not this test's subject): `stored` in row order.
    type Exact = fn(&[f64]) -> Option<f64>;
    let cases: [(&str, &[&str], Exact); 11] = [
        ("cancellation", &["1e16", "1.0", "-1e16"], |v| Some(v[1])),
        ("tenths", &["0.1", "0.2", "-0.3"], |v| {
            // Each is a multiple of 2^-60 below 2^4: their sum in 2^-60ths is exact in an i128,
            // and an i128's conversion rounds to nearest, ties to even.
            let sixtieths: i128 = v.iter().map(|x| (x * 2f64.powi(60)) as i128).sum();
            Some(sixtieths as f64 * 2f64.powi(-60))
        }),
        ("huge with tiny", &["1e300", "1e-300", "-1e300"], |v| {
            Some(v[1])
        }),
        ("subnormals", &["5e-324"; 7], |v| Some(v[0] * 7.0)),
        ("zeros", &["-0.0", "-0.0"], |_| Some(0.0)),
        ("plus infinity", &["9e999", "1.0"], |_| Some(f64::INFINITY)),
        ("minus infinity", &["-9e999", "-1.0"], |_| {
            Some(f64::NEG_INFINITY)
        }),
        ("both infinities", &["9e999", "-9e999", "1.0"], |_| None),
        ("past 2^53", &["9007199254740992.0", "1.0", "1.0"], |v| {
            Some(v[0] + 2.0)
        }),
        // Lane R4: R1's bigpairs, a short order's +1e308s unmatched: past the range, +inf, as
        // SQLite answers; and a running sum that passes the range and comes back, exactly.
        (
            "bigpairs",
            &["1e308", "1e308", "-1e308", "-1e308", "1e308", "1e308"],
            |_| Some(f64::INFINITY),
        ),
        (
            "overflow and back",
            &["1e308", "1e308", "-1e308", "-1e308", "1.5"],
            |v| Some(v[4]),
        ),
    ];
    for (at, (name, values, exact)) in cases.iter().enumerate() {
        let table = format!("h{at}");
        conn.execute(format!(
            "CREATE TABLE {table}(id INTEGER PRIMARY KEY, r REAL, p TEXT)"
        ))
        .unwrap();
        conn.execute("BEGIN").unwrap();
        for row in 0..rows {
            // The case's values spread over the table's leaves, NULLs between them.
            let place = (0..values.len()).find(|k| (k + 1) * rows / (values.len() + 1) == row);
            let r = place.map_or("NULL", |k| values[k]);
            conn.execute(format!("INSERT INTO {table}(r, p) VALUES ({r}, '{pad}')"))
                .unwrap();
        }
        conn.execute("COMMIT").unwrap();
        let sql = format!("SELECT SUM(r), TOTAL(r), AVG(r) FROM {table}");
        let answer = g8_answers_at_every_core_count(&conn, &sql, rows as u64);
        let stored: Vec<f64> = sql_rows(
            &conn,
            &format!("SELECT r FROM {table} WHERE r IS NOT NULL ORDER BY id"),
        )
        .iter()
        .map(|row| match &row[0] {
            Value::Numeric(crate::numeric::Numeric::Float(f)) => f64::from(*f),
            other => panic!("{name}: a stored REAL is {other:?}"),
        })
        .collect();
        let sum = exact(&stored);
        let real = |value: Option<f64>| match value {
            Some(value) => Value::from_f64(value),
            None => Value::Null,
        };
        let expected = vec![vec![
            real(sum),
            real(sum),
            real(sum.map(|sum| sum / values.len() as f64)),
        ]];
        assert_eq!(answer, format!("{expected:?}"), "{name}");
    }
}

/// Lane R2 (reals), the bounded check of R1's side finding (r1455): with the bench's ladder
/// setting (parallel mode on, parallel LIMIT on, a core cap), `SELECT 1 FROM bench_loaded LIMIT 1`
/// on a one-row table found no row or failed. Here in memory and on a file reopened by a fresh
/// database, as the bench's next process opened it.
#[test]
fn r2_parallel_limit_finds_a_one_row_tables_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r2.db").to_str().unwrap().to_string();
    let file = |path: &str| {
        let io = Arc::new(crate::io::PlatformIO::new().unwrap());
        Database::open_file_with_flags(
            io,
            path,
            OpenFlags::default(),
            DatabaseOpts::new(),
            None,
            Arc::new(SqliteDialect),
        )
        .unwrap()
        .connect(ConnectOpts::new())
        .unwrap()
    };
    let fill = |conn: &Arc<Connection>| {
        conn.execute("CREATE TABLE big (a INTEGER, b TEXT)")
            .unwrap();
        conn.execute("BEGIN").unwrap();
        for i in 0..20000 {
            conn.execute(format!("INSERT INTO big VALUES ({i}, 'row {i}')"))
                .unwrap();
        }
        conn.execute("COMMIT").unwrap();
        conn.execute("CREATE TABLE bench_loaded (done INTEGER)")
            .unwrap();
        conn.execute("INSERT INTO bench_loaded VALUES (1)").unwrap();
    };
    let memory = open(":memory:r2-limit");
    fill(&memory);
    fill(&file(&path));
    let reopened = file(&path);
    let mut wrong = Vec::new();
    for (name, conn) in [("memory", &memory), ("file reopened", &reopened)] {
        conn.set_parallel_mode(ParallelMode::Off);
        let control = conn
            .prepare("SELECT 1 FROM bench_loaded LIMIT 1")
            .and_then(|mut s| s.run_collect_rows());
        let all = conn
            .prepare("SELECT count(*) FROM bench_loaded")
            .and_then(|mut s| s.run_collect_rows());
        eprintln!("r2 {name} serial control: {control:?}, count(*) {all:?}");
        conn.set_parallel_mode(ParallelMode::On);
        conn.set_parallel_limit(false);
        conn.set_parallel_cores(NonZeroUsize::new(4));
        let no_limit_switch = conn
            .prepare("SELECT 1 FROM bench_loaded LIMIT 1")
            .and_then(|mut s| s.run_collect_rows());
        let no_limit = conn
            .prepare("SELECT 1 FROM bench_loaded")
            .and_then(|mut s| s.run_collect_rows());
        eprintln!("r2 {name} parallel on, LIMIT switch off, 4 cores: {no_limit_switch:?}; no LIMIT: {no_limit:?}");
        for cores in [1usize, 2, 4, 8, 16, 32] {
            conn.set_parallel_mode(ParallelMode::On);
            conn.set_parallel_limit(true);
            conn.set_parallel_cores(NonZeroUsize::new(cores));
            let got = conn
                .prepare("SELECT 1 FROM bench_loaded LIMIT 1")
                .and_then(|mut s| s.run_collect_rows());
            eprintln!("r2 {name} at {cores} cores: {got:?}");
            if !matches!(&got, Ok(rows) if rows.len() == 1) {
                wrong.push(format!("{name} at {cores} cores: {got:?}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:?}");
}

/// Lane R4 (reals), the cause R3 traced (r1585): a result column that is a constant is hoisted
/// to the program's end, and the body copy's `keep` label followed it there, so every worker
/// skipped every row. Do: select only constants (and a column before a constant) from a
/// 20,000-row table in parallel mode at 1 to 32 cores, in memory and on a file reopened by a
/// fresh database. Expect every row, as the serial loop returns them.
#[test]
fn r4_parallel_constant_columns_return_every_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r4.db").to_str().unwrap().to_string();
    let file = |path: &str| {
        let io = Arc::new(crate::io::PlatformIO::new().unwrap());
        Database::open_file_with_flags(
            io,
            path,
            OpenFlags::default(),
            DatabaseOpts::new(),
            None,
            Arc::new(SqliteDialect),
        )
        .unwrap()
        .connect(ConnectOpts::new())
        .unwrap()
    };
    let fill = |conn: &Arc<Connection>| {
        conn.execute("CREATE TABLE big (a INTEGER, b TEXT)")
            .unwrap();
        conn.execute("BEGIN").unwrap();
        for i in 0..20000 {
            conn.execute(format!("INSERT INTO big VALUES ({i}, 'row {i}')"))
                .unwrap();
        }
        conn.execute("COMMIT").unwrap();
    };
    let memory = open(":memory:r4-constants");
    fill(&memory);
    fill(&file(&path));
    let reopened = file(&path);
    let sorted = |rows: Vec<Vec<Value>>| {
        let mut rows: Vec<String> = rows.iter().map(|row| format!("{row:?}")).collect();
        rows.sort();
        rows
    };
    let mut wrong = Vec::new();
    for (name, conn) in [("memory", &memory), ("file reopened", &reopened)] {
        for sql in [
            "SELECT 1 FROM big",
            "SELECT 1, 'two', 3.5, NULL FROM big",
            "SELECT a, 7 FROM big",
        ] {
            conn.set_parallel_mode(ParallelMode::Off);
            let serial = sorted(sql_rows(conn, sql));
            assert_eq!(serial.len(), 20000, "{name}: {sql} serially");
            for cores in [1usize, 2, 4, 8, 16, 32] {
                conn.set_parallel_mode(ParallelMode::On);
                conn.set_parallel_cores(NonZeroUsize::new(cores));
                match conn.prepare(sql).and_then(|mut s| s.run_collect_rows()) {
                    Ok(rows) if sorted(rows.clone()) == serial => {}
                    Ok(rows) => wrong.push(format!(
                        "{name}: {sql} at {cores} cores: {} rows",
                        rows.len()
                    )),
                    Err(error) => wrong.push(format!("{name}: {sql} at {cores} cores: {error}")),
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Lane R5: the constant-column sweep's tables, as its SQLite probe builds them
/// (`r5-scripts/sweep_sqlite.py`): `t` of 5,000 rows, `u` of 300, and `w`, an INSERT's target.
fn r5_tables(name: &str) -> Arc<Connection> {
    let conn = open(name);
    for sql in [
        "CREATE TABLE t (id INTEGER PRIMARY KEY, a INTEGER, b INTEGER, s TEXT)",
        "CREATE TABLE u (id INTEGER PRIMARY KEY, a INTEGER, c TEXT)",
        "CREATE TABLE w (x, y, z)",
        "INSERT INTO t SELECT value, value % 97, (value * 37) % 1000, 'k' || (value % 11) \
         FROM generate_series(1, 5000)",
        "INSERT INTO u SELECT value, (value * 7) % 97, 'c' || (value % 13) \
         FROM generate_series(1, 300)",
    ] {
        conn.execute(sql).unwrap();
    }
    conn
}

/// Rows as SQLite's shell lists them (`|` between columns, NULL empty), sorted unless the
/// statement orders them: their count and FNV-1a 64 hash, as the probe takes SQLite's.
fn r5_digest(rows: &[Vec<Value>], ordered: bool) -> (usize, u64) {
    let mut lines: Vec<String> = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("|")
        })
        .collect();
    if !ordered {
        lines.sort();
    }
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in lines.join("\n").bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (lines.len(), hash)
}

/// Lane R5: each statement (SQL, ordered, SQLite 3.54.0's count and hash from the probe on
/// adacachy2) serially and in parallel mode on a machine stated as 1, 2, 4 and 8 cores, against
/// SQLite's answer; an INSERT's answer is what `w` then holds. Returns the rows the workers ran
/// the loop body over.
fn r5_sweep(cases: &[(&str, bool, usize, u64)]) -> u64 {
    use std::sync::atomic::Ordering::Relaxed;
    let conn = r5_tables(":memory:r5-sweep");
    conn.set_join_methods(crate::JoinMethods::HashJoinsOnly);
    let answer = |sql: &str, ordered: bool| -> Result<(usize, u64), String> {
        let rows = if sql.starts_with("INSERT") {
            conn.execute("DELETE FROM w").map_err(|e| e.to_string())?;
            conn.execute(sql).map_err(|e| e.to_string())?;
            sql_rows(&conn, "SELECT x, y, z FROM w")
        } else {
            conn.prepare(sql)
                .and_then(|mut s| s.run_collect_rows())
                .map_err(|e| e.to_string())?
        };
        Ok(r5_digest(&rows, ordered))
    };
    let mut wrong = Vec::new();
    let mut body = 0u64;
    for &(sql, ordered, count, hash) in cases {
        let sqlite = Ok((count, hash));
        conn.set_parallel_mode(ParallelMode::Off);
        let serial = answer(sql, ordered);
        if serial != sqlite {
            wrong.push(format!("{sql} serially: {serial:x?}, SQLite {sqlite:x?}"));
        }
        conn.set_parallel_mode(ParallelMode::On);
        for cores in [1usize, 2, 4, 8] {
            conn.set_parallel_cores(NonZeroUsize::new(cores));
            *conn.morsel_costs.lock() = Some(MachineCosts::stated(
                NonZeroUsize::new(cores).unwrap(),
                1.0,
                1.0,
            ));
            let before = conn.morsel_body_rows.load(Relaxed);
            let parallel = answer(sql, ordered);
            body += conn.morsel_body_rows.load(Relaxed) - before;
            if parallel != sqlite {
                wrong.push(format!(
                    "{sql} at {cores} cores: {parallel:x?}, SQLite {sqlite:x?}"
                ));
            }
        }
    }
    conn.set_parallel_mode(ParallelMode::Off);
    assert!(wrong.is_empty(), "{wrong:#?}");
    body
}

/// Lane R5: a constant result column first, in the middle and last, over a plain scan. Do: run
/// each serially and in parallel mode. Expect SQLite's rows, with the workers running the body.
#[test]
fn r5_constant_columns_over_a_scan_answer_as_sqlite() {
    let body = r5_sweep(&[
        ("SELECT 7, a, s FROM t", false, 5000, 0xd0dcdf34374d0a58),
        ("SELECT a, 'x', s FROM t", false, 5000, 0x0bc780edde2ba5c0),
        ("SELECT a, s, 2.5 FROM t", false, 5000, 0x6daca8fafe52c888),
    ]);
    assert!(body > 0, "no worker ran the body");
}

/// Lane R5: the same over a filtered scan.
#[test]
fn r5_constant_columns_over_a_filtered_scan_answer_as_sqlite() {
    let body = r5_sweep(&[
        (
            "SELECT 7, a, s FROM t WHERE b < 300",
            false,
            1500,
            0x0332bba8c11f5ac9,
        ),
        (
            "SELECT a, 'x', s FROM t WHERE b < 300",
            false,
            1500,
            0xdf7afb91c5df82f1,
        ),
        (
            "SELECT a, s, 2.5 FROM t WHERE b < 300",
            false,
            1500,
            0x26c32b876e4bad13,
        ),
    ]);
    assert!(body > 0, "no worker ran the body");
}

/// Lane R5: the same over a hash join.
#[test]
fn r5_constant_columns_over_a_join_answer_as_sqlite() {
    let body = r5_sweep(&[
        (
            "SELECT 7, t.id, u.c FROM t JOIN u ON t.a = u.a",
            false,
            15466,
            0xfc3c2e2ce9a1f670,
        ),
        (
            "SELECT t.id, 'x', u.c FROM t JOIN u ON t.a = u.a",
            false,
            15466,
            0x66e20388227c40a0,
        ),
        (
            "SELECT t.id, u.c, 2.5 FROM t JOIN u ON t.a = u.a",
            false,
            15466,
            0xa947bd667b1dd3e6,
        ),
    ]);
    assert!(body > 0, "no worker ran the body");
}

/// Lane R5: the same beside GROUP BY's aggregates.
#[test]
fn r5_constant_columns_beside_group_by_answer_as_sqlite() {
    r5_sweep(&[
        (
            "SELECT 7, a, count(*) FROM t GROUP BY a",
            false,
            97,
            0xe1f7c10968cb8970,
        ),
        (
            "SELECT a, 'x', sum(b) FROM t GROUP BY a",
            false,
            97,
            0x26276f59c3027b09,
        ),
        (
            "SELECT a, count(*), 2.5 FROM t WHERE b < 500 GROUP BY a",
            false,
            97,
            0x670ea407ffae021b,
        ),
    ]);
}

/// Lane R5: the same under ORDER BY with LIMIT, in order.
#[test]
fn r5_constant_columns_under_order_by_limit_answer_as_sqlite() {
    r5_sweep(&[
        (
            "SELECT 7, id, s FROM t ORDER BY b, id LIMIT 1000",
            true,
            1000,
            0xc9689f7ab1e72f0e,
        ),
        (
            "SELECT id, 'x', s FROM t ORDER BY b DESC, id LIMIT 1000",
            true,
            1000,
            0x1cf4d7104ab7c83c,
        ),
        (
            "SELECT id, s, 2.5 FROM t WHERE b < 700 ORDER BY s, id LIMIT 1000",
            true,
            1000,
            0xfd75b0a963e7378b,
        ),
    ]);
}

/// Lane R5: the same in each arm of a compound SELECT, UNION ALL and UNION.
#[test]
fn r5_constant_columns_in_a_compound_select_answer_as_sqlite() {
    r5_sweep(&[
        (
            "SELECT 7, a, s FROM t WHERE b < 100 UNION ALL SELECT 8, a, c FROM u",
            false,
            800,
            0x799eb35b14b8e082,
        ),
        (
            "SELECT a, 'x', s FROM t WHERE b < 100 UNION ALL SELECT a, 'y', c FROM u",
            false,
            800,
            0x55575c8b0d78681a,
        ),
        (
            "SELECT a, s, 2.5 FROM t WHERE b < 100 UNION ALL SELECT a, c, 3.5 FROM u",
            false,
            800,
            0xacb474c2deb3d47c,
        ),
        (
            "SELECT a, s, 2.5 FROM t UNION SELECT a, c, 2.5 FROM u",
            false,
            1367,
            0x1466c9c5ba51a191,
        ),
    ]);
}

/// Lane R5: the same inside a subquery in FROM, and outside one the planner can't flatten.
#[test]
fn r5_constant_columns_with_a_subquery_in_from_answer_as_sqlite() {
    r5_sweep(&[
        (
            "SELECT k, a, s FROM (SELECT 7 AS k, a, s FROM t WHERE b < 500)",
            false,
            2500,
            0x68b803b21717e29c,
        ),
        (
            "SELECT a, k, s FROM (SELECT a, 'x' AS k, s FROM t WHERE b < 500)",
            false,
            2500,
            0xac9b4169d497366a,
        ),
        (
            "SELECT a, s, k FROM (SELECT a, s, 2.5 AS k FROM t WHERE b < 500)",
            false,
            2500,
            0x27f2aef530eed638,
        ),
        (
            "SELECT 7, q.a, q.n FROM (SELECT a, count(*) AS n FROM t GROUP BY a) AS q",
            false,
            97,
            0xe1f7c10968cb8970,
        ),
        (
            "SELECT q.a, 'x', q.n FROM (SELECT a, count(*) AS n FROM t GROUP BY a) AS q",
            false,
            97,
            0x3bec3e691717cd55,
        ),
        (
            "SELECT q.a, q.n, 2.5 FROM (SELECT a, count(*) AS n FROM t GROUP BY a) AS q",
            false,
            97,
            0xc1abb5df4e49981a,
        ),
    ]);
}

/// Lane R5: the same in INSERT ... SELECT. Expect the target to hold SQLite's rows.
#[test]
fn r5_constant_columns_in_insert_select_answer_as_sqlite() {
    r5_sweep(&[
        (
            "INSERT INTO w SELECT 7, a, s FROM t WHERE b < 600",
            false,
            3000,
            0x4f30bb65bd3b3791,
        ),
        (
            "INSERT INTO w SELECT a, 'x', s FROM t WHERE b < 600",
            false,
            3000,
            0x03bf73a80161d1a3,
        ),
        (
            "INSERT INTO w SELECT a, s, 2.5 FROM t WHERE b < 600",
            false,
            3000,
            0x4b0ee3eddba05fc1,
        ),
    ]);
}
