//! AC-T1.8.6: a column of a recursive or ordinary CTE, a derived table or a compound, compared, joined,
//! sorted and grouped by as SQLite 3.54.0 does it: by the affinity and the collation of the expression the
//! column is made from. A recursive CTE's own arms read its columns as a BLOB column's, so a number never
//! met the text of a TEXT column (the oracles' finding CU, lane C16).

use amphora_engine::morsel::ParallelMode;
use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, MemoryIO, OpenOptions, SqliteDialect, Value,
};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// The settings a user meets a statement under: the parallel mode, and whether the database has
/// statistics.
const SETTINGS: [(&str, bool, bool); 4] = [
    ("default", false, false),
    ("parallel", true, false),
    ("statistics", false, true),
    ("parallel, statistics", true, true),
];

fn open(fill: &[&str], parallel: bool, statistics: bool) -> Arc<Connection> {
    static N: AtomicUsize = AtomicUsize::new(0);
    let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
    let name = format!("cte_columns_{}.db", N.fetch_add(1, Ordering::Relaxed));
    let db = Database::open(Arc::new(MemoryIO::new()), &name, options).unwrap();
    let conn = db.connect(ConnectOpts::new()).unwrap();
    for sql in fill {
        conn.execute(*sql).unwrap();
    }
    if statistics {
        conn.execute("ANALYZE").unwrap();
    }
    if parallel {
        conn.set_parallel_mode(ParallelMode::On);
    }
    conn
}

/// The rows of `sql` in its order, each row's values joined by commas.
fn rows(conn: &Arc<Connection>, sql: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn.prepare(sql).map_err(|error| error.to_string())?;
    Ok(stmt
        .run_collect_rows()
        .map_err(|error| error.to_string())?
        .iter()
        .map(|row| {
            row.iter()
                .map(|value| match value {
                    Value::Null => "NULL".to_owned(),
                    value => value.to_string(),
                })
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect())
}

/// FNV-1a 64 of lines, as `bench/sqlite_cte_columns.py` sums SQLite's rows.
fn lines_sum(lines: &[String]) -> u64 {
    lines
        .join("\n")
        .bytes()
        .fold(0xCBF2_9CE4_8422_2325_u64, |sum, byte| {
            (sum ^ u64::from(byte)).wrapping_mul(0x100_0000_01B3)
        })
}

/// The fixture: the statements that fill the database, and the parts its queries are put together from.
#[derive(Default)]
struct Sweep {
    fill: Vec<&'static str>,
    sources: BTreeMap<&'static str, &'static str>,
    shapes: BTreeMap<&'static str, &'static str>,
    steps: BTreeMap<&'static str, &'static str>,
    outers: BTreeMap<&'static str, &'static str>,
    /// Shape, source, tail, SQLite's answer.
    queries: Vec<[&'static str; 4]>,
}

impl Sweep {
    fn read() -> Self {
        let mut sweep = Self::default();
        for line in include_str!("fixtures/cte_columns.tsv").lines() {
            match line.split('\t').collect::<Vec<_>>().as_slice() {
                ["sql", sql] => sweep.fill.push(sql),
                ["source", name, text] => drop(sweep.sources.insert(name, text)),
                ["shape", name, text] => drop(sweep.shapes.insert(name, text)),
                ["tail", "step", name, text] => drop(sweep.steps.insert(name, text)),
                ["tail", "outer", name, text] => drop(sweep.outers.insert(name, text)),
                ["q", shape, source, tail, answer] => {
                    sweep.queries.push([shape, source, tail, answer]);
                }
                _ => assert!(line.starts_with('#'), "{line}"),
            }
        }
        sweep
    }

    fn sql(&self, shape: &str, source: &str, tail: &str) -> String {
        let text = self.shapes[shape];
        let step = shape.starts_with("step") || shape.starts_with("mixed_step");
        let tail = if step {
            self.steps[tail]
        } else {
            self.outers[tail]
        };
        let text = match text.strip_prefix("@TAIL:") {
            Some(from) => tail.replace("@W@", from.trim_end_matches('@')),
            None => text.replace("@TAIL@", &tail.replace("@W@", "w")),
        };
        text.replace("@E@", self.sources[source])
    }
}

/// Do: run every query of the fixture whose shape `shapes` names under one setting. Expect: SQLite's rows.
fn answers_as_sqlite(setting: usize, shapes: impl Fn(&str) -> bool) {
    let sweep = Sweep::read();
    let (setting, parallel, statistics) = SETTINGS[setting];
    let conn = open(&sweep.fill, parallel, statistics);
    let mut wrong: Vec<String> = Vec::new();
    let mut run = 0_usize;
    for [shape, source, tail, want] in &sweep.queries {
        if !shapes(shape) {
            continue;
        }
        run += 1;
        let sql = sweep.sql(shape, source, tail);
        let got = rows(&conn, &sql).map(|mut rows| {
            if !tail.starts_with("ordered") {
                rows.sort();
            }
            rows
        });
        let answer = match &got {
            Ok(rows) => format!("{}:{:016x}", rows.len(), lines_sum(rows)),
            Err(error) => format!("refused: {error}"),
        };
        if answer != *want {
            wrong.push(format!(
                "{shape} {source} {tail}: got {answer}, SQLite {want}: {sql}"
            ));
        }
    }
    eprintln!("{setting}: {run} queries, {} wrong", wrong.len());
    for line in &wrong {
        eprintln!("WRONG {setting}: {line}");
    }
    assert!(run > 0 && wrong.is_empty(), "{} wrong", wrong.len());
}

fn recursive(shape: &str) -> bool {
    shape.starts_with("rec")
        || shape.starts_with("step")
        || shape.contains("_rec_")
        || shape.contains("_step_")
}

#[test]
fn a_recursive_ctes_columns_compare_as_sqlites() {
    answers_as_sqlite(0, recursive);
}

#[test]
fn a_recursive_ctes_columns_compare_as_sqlites_parallel() {
    answers_as_sqlite(1, recursive);
}

#[test]
fn a_recursive_ctes_columns_compare_as_sqlites_statistics() {
    answers_as_sqlite(2, recursive);
}

#[test]
fn a_recursive_ctes_columns_compare_as_sqlites_parallel_statistics() {
    answers_as_sqlite(3, recursive);
}

#[test]
fn a_subquerys_columns_compare_as_sqlites() {
    answers_as_sqlite(0, |shape| !recursive(shape));
}

#[test]
fn a_subquerys_columns_compare_as_sqlites_parallel() {
    answers_as_sqlite(1, |shape| !recursive(shape));
}

#[test]
fn a_subquerys_columns_compare_as_sqlites_statistics() {
    answers_as_sqlite(2, |shape| !recursive(shape));
}

#[test]
fn a_subquerys_columns_compare_as_sqlites_parallel_statistics() {
    answers_as_sqlite(3, |shape| !recursive(shape));
}

/// Do: recursive CTEs whose arms compare the CTE's own columns, its columns of each arm's affinity or of
/// none, typed one after another, through compound anchors, nested recursive CTEs, correlated subqueries
/// and a UNION, under each setting. Expect: SQLite 3.54.0's rows.
#[test]
fn a_recursive_ctes_own_columns_are_typed_by_every_arm() {
    let fill = [
        "CREATE TABLE edge(src TEXT, dst TEXT)",
        "INSERT INTO edge VALUES ('1', '2'), ('2', '3')",
        "CREATE TABLE tt(c TEXT)",
        "INSERT INTO tt VALUES ('1')",
        "CREATE TABLE ti(c INTEGER)",
        "INSERT INTO ti VALUES (1)",
        "CREATE TABLE tc(c TEXT COLLATE NOCASE)",
        "INSERT INTO tc VALUES ('ABC')",
        "CREATE TABLE tp(c TEXT)",
        "INSERT INTO tp VALUES ('ABC'), ('abc')",
    ];
    // A nested CTE whose arms make its column other than its initial query does: BLOB, not none.
    const NESTED: &str = "WITH RECURSIVE q(v, e) AS (SELECT 1, 0 UNION ALL \
                          SELECT tt.c, q.e + 1 FROM q JOIN tt ON tt.c = q.v WHERE q.e < 2) SELECT v, e FROM q";
    let cases: Vec<(String, &[&str])> = vec![
        (
            "WITH RECURSIVE r(n, d) AS (SELECT 1, 0 UNION ALL SELECT r.n + 1, r.d + 1 FROM r JOIN edge ON edge.src = r.n WHERE r.d < 5) SELECT n, d FROM r".into(),
            &["1,0", "2,1", "3,2"],
        ),
        (
            "WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT r.n + 1 FROM edge JOIN r ON edge.src = r.n) SELECT n FROM r".into(),
            &["1", "2", "3"],
        ),
        (
            "WITH RECURSIVE r(n, d) AS (SELECT c, 0 FROM tt UNION ALL SELECT r.n, r.d + 1 FROM r WHERE r.n = 1 AND r.d < 2) SELECT n, d, typeof(n) FROM r".into(),
            &["1,0,text", "1,1,text", "1,2,text"],
        ),
        (
            "WITH RECURSIVE r(n, d) AS (SELECT c, 0 FROM ti UNION ALL SELECT r.n, r.d + 1 FROM r WHERE r.n = '1' AND r.d < 2) SELECT n, d, typeof(n) FROM r".into(),
            &["1,0,integer", "1,1,integer", "1,2,integer"],
        ),
        (
            "WITH RECURSIVE r(n, d) AS (SELECT c, 0 FROM tc UNION ALL SELECT r.n, r.d + 1 FROM r WHERE r.n = 'abc' AND r.d < 2) SELECT n, d FROM r".into(),
            &["ABC,0", "ABC,1", "ABC,2"],
        ),
        (
            "WITH RECURSIVE r(n, d) AS (SELECT 'ABC' COLLATE NOCASE, 0 UNION ALL SELECT r.n, r.d + 1 FROM r JOIN tp ON r.n = tp.c WHERE r.d < 1) SELECT n, d, n = 'abc' FROM r".into(),
            &["ABC,0,1", "ABC,1,1", "ABC,1,1"],
        ),
        (
            "WITH RECURSIVE r(a, b, d) AS (SELECT c, NULL, 0 FROM tt UNION ALL SELECT '1', r.a, r.d + 1 FROM r WHERE r.d < 1) SELECT a, b, d, a = 1, b = 1 FROM r".into(),
            &["1,1,1,1,1", "1,NULL,0,1,NULL"],
        ),
        (
            "WITH RECURSIVE r(a, b, d) AS (SELECT NULL, c, 0 FROM tt UNION ALL SELECT r.b, '1', r.d + 1 FROM r WHERE r.d < 1) SELECT a, b, d, a = 1, b = 1 FROM r".into(),
            &["1,1,1,0,1", "NULL,1,0,NULL,1"],
        ),
        (
            "WITH RECURSIVE r(a, d) AS (SELECT '1', 0 UNION ALL SELECT c, 0 FROM tt UNION ALL SELECT r.a, r.d + 1 FROM r WHERE r.d < 1) SELECT a, d, a = 1 FROM r".into(),
            &["1,0,1", "1,0,1", "1,1,1", "1,1,1"],
        ),
        (
            "WITH RECURSIVE r(a, d) AS (SELECT c, 0 FROM tt UNION ALL SELECT 1, r.d + 1 FROM r WHERE r.d < 1) SELECT a, d, a = 1, a = '1' FROM r".into(),
            &["1,0,0,1", "1,1,1,0"],
        ),
        (NESTED.into(), &["1,0"]),
        (
            format!("WITH RECURSIVE r(a, d) AS (SELECT 1, 0 UNION ALL SELECT x.v, r.d + 1 FROM r JOIN ({NESTED}) AS x ON x.e = r.d WHERE r.d < 2) SELECT a, d, typeof(a) FROM r"),
            &["1,0,integer", "1,1,integer"],
        ),
        (
            format!("WITH RECURSIVE r(a, d) AS (SELECT c, 0 FROM tt UNION ALL SELECT x.v, r.d + 1 FROM r JOIN ({NESTED}) AS x ON x.e = r.d AND r.a = 1 WHERE r.d < 2) SELECT a, d, typeof(a) FROM r"),
            &["1,0,text"],
        ),
        (
            "WITH RECURSIVE r(n, d) AS (SELECT 1, 0 UNION ALL SELECT r.n + 1, r.d + 1 FROM r WHERE EXISTS (SELECT 1 FROM edge WHERE edge.src = r.n)) SELECT n, d FROM r".into(),
            &["1,0", "2,1", "3,2"],
        ),
        (
            "WITH RECURSIVE r(n, d) AS (SELECT 1, 0 UNION ALL SELECT r.n + 1, r.d + 1 FROM r WHERE r.n IN (SELECT src FROM edge)) SELECT n, d FROM r".into(),
            &["1,0", "2,1", "3,2"],
        ),
        (
            "WITH RECURSIVE r(n) AS (SELECT 1 UNION SELECT r.n + 1 FROM r JOIN edge ON r.n = edge.src ORDER BY 1 DESC) SELECT n FROM r".into(),
            &["1", "2", "3"],
        ),
        // A row value's left column that declares no collation is BINARY's (datatype3.html 7.1).
        (
            "SELECT tp.c, tc.c FROM tp JOIN tc ON (tp.c, 1) = (tc.c, 1)".into(),
            &["ABC,ABC"],
        ),
        (
            "SELECT tp.c, tc.c FROM tp JOIN tc ON (tc.c, 1) = (tp.c, 1)".into(),
            &["ABC,ABC", "abc,ABC"],
        ),
    ];
    for (setting, parallel, statistics) in SETTINGS {
        let conn = open(&fill, parallel, statistics);
        for (sql, want) in &cases {
            let mut got = rows(&conn, sql).unwrap();
            got.sort();
            assert_eq!(got, *want, "{setting}: {sql}");
        }
    }
}

/// Do: nest recursive CTEs in each other's arms, each one's column typed otherwise by its arms than by its
/// initial query, so that each is planned twice. Expect: the rows, and that the plannings add up: each
/// level planned again for each level around it would be 2^24 plannings, which would not end here.
#[test]
fn nested_recursive_ctes_are_planned_again_a_bounded_number_of_times() {
    fn nested() -> Result<Vec<String>, String> {
        let conn = open(
            &["CREATE TABLE tt(c TEXT)", "INSERT INTO tt VALUES ('1')"],
            false,
            false,
        );
        let mut sql = "SELECT 1 AS v, 0 AS e".to_owned();
        for level in 0..24 {
            sql = format!(
                "WITH RECURSIVE q{level}(v, e) AS (SELECT 1, 0 UNION ALL SELECT tt.c, q{level}.e + 1 \
                 FROM q{level} JOIN tt ON tt.c = q{level}.v JOIN ({sql}) AS x{level} WHERE q{level}.e < 1) SELECT v, e FROM q{level}"
            );
        }
        rows(&conn, &sql)
    }
    // A debug build's planner takes some stack a level.
    let nested = std::thread::Builder::new()
        .stack_size(1 << 29)
        .spawn(nested)
        .unwrap();
    assert_eq!(nested.join().unwrap().unwrap(), ["1,0"]);
}

/// Do: compare the merged column of a FULL or RIGHT JOIN's USING or NATURAL, TEXT on the left and REAL on
/// the right, with numbers and texts, under each setting. Expect: SQLite 3.54.0's rows. The column has the
/// left's affinity, and TEXT affinity converts a number only to compare it with a text: Amphora compared
/// '0.0' with '0' (the oracles' finding CZ).
#[test]
fn a_merged_columns_numbers_compare_as_numbers() {
    let fill = [
        "CREATE TABLE a(k TEXT, x)",
        "CREATE TABLE b(k REAL, y)",
        "INSERT INTO a VALUES ('1', 'a1')",
        "INSERT INTO b VALUES (0.0, 'b0'), (2.5, 'b2'), (3, 'b3'), (10, 'b10')",
    ];
    let cases: [(&str, &[&str]); 5] = [
        (
            "SELECT y, k = 0, k = 0.0, k = '0', k = '0.0', k < 1, k > 2, k IN (0), k BETWEEN 0 AND 1 FROM a FULL JOIN b USING (k)",
            &[
                "NULL,0,0,0,0,0,0,0,1",
                "b0,1,1,0,1,1,0,1,1",
                "b10,0,0,0,0,0,1,0,0",
                "b2,0,0,0,0,0,1,0,0",
                "b3,0,0,0,0,0,1,0,0",
            ],
        ),
        ("SELECT y FROM a FULL JOIN b USING (k) WHERE 0 == k", &["b0"]),
        (
            "SELECT y FROM a RIGHT JOIN b USING (k) WHERE k >= 3",
            &["b10", "b3"],
        ),
        (
            "SELECT y FROM a NATURAL FULL JOIN b WHERE k BETWEEN 3.0 AND 'ABC'",
            &["b10", "b3"],
        ),
        (
            "SELECT y FROM a FULL JOIN b USING (k) WHERE k IN (3, 2.5)",
            &["b2", "b3"],
        ),
    ];
    for (setting, parallel, statistics) in SETTINGS {
        let conn = open(&fill, parallel, statistics);
        for (sql, want) in cases {
            let mut got = rows(&conn, sql).unwrap();
            got.sort();
            assert_eq!(got, want, "{setting}: {sql}");
        }
    }
}
