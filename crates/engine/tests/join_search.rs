//! The join search past 12 tables (docs/design/planner.md decision 18; planner rank 3, PL.3): a join
//! whose connected subsets fit the search's budget is searched exactly, whatever its table count,
//! and one past it is ordered by the greedy search, which adds at each step the table whose joined
//! set has the fewest estimated rows (AC-PL.3.1, T-PL.3.1).

use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, OpenOptions, PlatformIO, SqliteDialect,
};
use std::path::Path;
use std::sync::Arc;

/// A fact table `f` and `dims` dimensions `d1..`, `f.kN` naming `dN`'s key, none analyzed (every
/// table the planner's fallback row count), as the audit's 01-left-deep-greedy.sql has them.
fn star(dir: &Path, dims: usize) -> Arc<Connection> {
    let io = Arc::new(PlatformIO::new().unwrap());
    let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
    let db = Database::open(io, dir.join("star.db").to_str().unwrap(), options).unwrap();
    let conn = db.connect(ConnectOpts::new()).unwrap();
    let keys: Vec<String> = (1..=dims).map(|n| format!("k{n} INT")).collect();
    conn.execute(format!(
        "CREATE TABLE f(id INTEGER PRIMARY KEY, {}, v INT)",
        keys.join(", ")
    ))
    .unwrap();
    for n in 1..=dims {
        conn.execute(format!(
            "CREATE TABLE d{n}(id INTEGER PRIMARY KEY, v INT, w INT)"
        ))
        .unwrap();
    }
    conn
}

/// The star's join of every dimension, the last filtered.
fn star_query(dims: usize) -> String {
    let joins: Vec<String> = (1..=dims)
        .map(|n| format!("JOIN d{n} ON d{n}.id = f.k{n}"))
        .collect();
    format!(
        "SELECT count(*) FROM f {} WHERE d{dims}.v = 7",
        joins.join(" ")
    )
}

/// The plan's steps, as EXPLAIN QUERY PLAN words them.
fn plan(conn: &Arc<Connection>, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
    stmt.run_collect_rows()
        .unwrap()
        .into_iter()
        .map(|row| row[3].to_string())
        .collect()
}

/// The step that reads `table`.
fn step_of(steps: &[String], table: &str) -> usize {
    steps
        .iter()
        .position(|step| step.split_whitespace().nth(1) == Some(table))
        .unwrap_or_else(|| panic!("no step reads {table}: {steps:?}"))
}

/// Do: plan the audit's row 2 query, a fact table and 12 dimensions (13 tables), the last filtered.
/// Expect: the filtered dimension read among the first two tables, as the search of 12 tables
/// reads it, where the greedy search past 12 read it last at 5.7 times the cost
/// (bench/planner/audit/01-left-deep-greedy.out; AC-PL.3.1).
#[test]
fn thirteen_tables_within_the_budget_are_searched_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let conn = star(dir.path(), 12);
    let steps = plan(&conn, &star_query(12));
    assert_eq!(steps.len(), 13, "{steps:?}");
    assert!(step_of(&steps, "d12") <= 1, "{steps:?}");
}

/// Do: plan a fact table and 39 dimensions (40 tables), the last filtered: 2^39 connected subsets,
/// past any budget. Expect: planning ends with a plan of all 40 tables, the filtered dimension
/// among the first two, since its joined set has the fewest rows (T-PL.3.1).
#[test]
fn a_join_past_the_budget_is_ordered_by_the_fewest_rows() {
    let dir = tempfile::tempdir().unwrap();
    let conn = star(dir.path(), 39);
    let steps = plan(&conn, &star_query(39));
    assert_eq!(steps.len(), 40, "{steps:?}");
    assert!(step_of(&steps, "d39") <= 1, "{steps:?}");
}
