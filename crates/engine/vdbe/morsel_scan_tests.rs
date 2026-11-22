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
    TableSource::open(conn.get_pager(), root, COLUMNS.len(), None).unwrap()
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
    let empty = TableSource::open(conn.get_pager(), root, 2, Some(0)).unwrap();
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
    let table = TableSource::open(conn.get_pager(), root, 2, Some(0)).unwrap();
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
            _ => format!("{t}.b IS NULL OR {t}.a > {n}"),
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
    let conn = open(":memory:qt-6-19");
    let mut rng = ChaCha8Rng::seed_from_u64(619);
    fill(&conn, &mut rng, "t", 500);
    fill(&conn, &mut rng, "u", 150);
    conn.set_join_methods(crate::JoinMethods::HashJoinsOnly);
    let kinds = ["LEFT", "RIGHT", "FULL"];
    let mut hooked = [0usize; 3];
    let queries = 36usize;
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
    let conn = open(":memory:qt-6-22");
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
    let conn = open(":memory:qt-6-23");
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
