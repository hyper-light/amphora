//! Kiln's records reads (its versions table, `crates/kiln/src/records`), planned without statistics
//! as SQLite 3.54.0 plans them (reported by Sends T3 for CP-T1.5): a row's newest version, or its
//! version at a revision, sought by its key on the primary key; a scan of keys sought on the primary
//! key from its first key, in its order; a family's versions from a revision walked by revision; none
//! sorted. Without `sqlite_stat1` SQLite estimates by its defaults (`sqlite3DefaultRowEst`), so the
//! index that matches more equality columns, and the one that saves the sort, win (AC-T1.5.4: the
//! change feed at its latency; AC-T1.13.4: history reads at theirs).

use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, OpenOptions, PlatformIO, SqliteDialect,
};
use std::sync::Arc;

/// Kiln's versions table and index holding 4,000 versions of one family (kiln's own test's), with
/// no statistics.
fn versions(dir: &std::path::Path) -> Arc<Connection> {
    let io = Arc::new(PlatformIO::new().unwrap());
    let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
    let db = Database::open(io, dir.join("records.db").to_str().unwrap(), options).unwrap();
    let conn = db.connect(ConnectOpts::new()).unwrap();
    conn.execute("CREATE TABLE versions (family TEXT NOT NULL, key BLOB NOT NULL, rev INTEGER NOT NULL, n INTEGER NOT NULL, value BLOB, PRIMARY KEY (family, key, rev))").unwrap();
    conn.execute("CREATE INDEX versions_by_rev ON versions (family, rev, n)")
        .unwrap();
    conn.execute("WITH RECURSIVE s(x) AS (SELECT 0 UNION ALL SELECT x + 1 FROM s WHERE x < 3999) INSERT INTO versions SELECT 'inbox', CAST('s' || (x / 500) || '/m' || x AS BLOB), x + 1, 0, zeroblob(60) FROM s").unwrap();
    conn
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

/// Do: plan each of kiln's reads without statistics, as kiln words them and in their plain forms
/// (no ORDER BY on the equality columns, no INDEXED BY). Expect: SQLite 3.54.0's plan for each, one
/// search on the index it names, by the leading constraints it names, with no sort.
#[test]
fn kilns_reads_are_planned_without_statistics_as_sqlite_plans_them() {
    let dir = tempfile::tempdir().unwrap();
    let conn = versions(dir.path());
    let by_key = "INDEX sqlite_autoindex_versions_1 (family=? AND key";
    let by_rev = "INDEX versions_by_rev (family=? AND rev>?";
    for (sql, seek) in [
        // LATEST and GET_LATEST, as kiln words them and plainly.
        ("SELECT rev, value IS NULL FROM versions WHERE family = ?1 AND key = ?2 ORDER BY family DESC, key DESC, rev DESC LIMIT 1", format!("{by_key}=?)")),
        ("SELECT rev, value FROM versions WHERE family = ?1 AND key = ?2 ORDER BY rev DESC LIMIT 1", format!("{by_key}=?)")),
        // GET_AT and AT_DELETED.
        ("SELECT rev, value FROM versions WHERE family = ?1 AND key = ?2 AND rev <= ?3 ORDER BY family DESC, key DESC, rev DESC LIMIT 1", format!("{by_key}=? AND rev<")),
        ("SELECT rev, value IS NULL FROM versions WHERE family = ?1 AND key = ?2 AND rev <= ?3 ORDER BY rev DESC LIMIT 1", format!("{by_key}=? AND rev<")),
        // SCAN_TO and SCAN_ON.
        ("SELECT key, rev, value FROM versions WHERE family = ?1 AND key >= ?2 AND key < ?3 ORDER BY key, rev", format!("{by_key}>")),
        ("SELECT key, rev, value FROM versions WHERE family = ?1 AND key >= ?2 ORDER BY key, rev", format!("{by_key}>")),
        // FEED and WINDOW, as kiln words them and plainly.
        ("SELECT rev, key, value FROM versions INDEXED BY versions_by_rev WHERE family = ?1 AND rev > ?2 ORDER BY rev, n LIMIT ?3", format!("{by_rev})")),
        ("SELECT rev, key, value FROM versions WHERE family = ?1 AND rev > ?2 ORDER BY rev, n LIMIT ?3", format!("{by_rev})")),
        ("SELECT key, rev FROM versions INDEXED BY versions_by_rev WHERE family = ?1 AND rev > ?2 AND rev <= ?3 ORDER BY rev, n LIMIT ?4", format!("{by_rev} AND rev<")),
        ("SELECT key, rev FROM versions WHERE family = ?1 AND rev > ?2 AND rev <= ?3 ORDER BY rev, n LIMIT ?4", format!("{by_rev} AND rev<")),
        // PRUNE_BELOW and PRUNE_AT.
        ("DELETE FROM versions WHERE family = ?1 AND key = ?2 AND rev < ?3", format!("{by_key}=? AND rev<")),
        ("DELETE FROM versions WHERE family = ?1 AND key = ?2 AND rev = ?3", format!("{by_key}=? AND rev=?)")),
    ] {
        let steps = plan(&conn, sql);
        assert!(
            steps.len() == 1
                && steps[0].starts_with("SEARCH versions USING ")
                && steps[0].contains(&seek),
            "{sql}: {steps:?}"
        );
    }
}

/// Do: name an index with INDEXED BY where it can seek, where nothing seeks it and where it doesn't
/// exist, and forbid indexes with NOT INDEXED. Expect: SQLite 3.54.0's answer for each: the named
/// index sought or scanned, never ignored for a better one; a missing index refused when the
/// statement is prepared; a table scan under NOT INDEXED, where the primary key could seek.
#[test]
fn indexed_by_uses_the_named_index_or_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let conn = versions(dir.path());
    for (sql, want) in [
        (
            "SELECT rev FROM versions INDEXED BY versions_by_rev WHERE family = ?1 AND key = ?2 ORDER BY rev DESC LIMIT 1",
            "SEARCH versions USING INDEX versions_by_rev (family=?)",
        ),
        (
            "SELECT rev FROM versions INDEXED BY versions_by_rev WHERE key = ?1",
            "SCAN versions USING INDEX versions_by_rev",
        ),
        (
            "SELECT rev FROM versions INDEXED BY versions_by_rev WHERE family = ?1 OR key = ?2",
            "SCAN versions USING INDEX versions_by_rev",
        ),
        (
            "SELECT rev FROM versions NOT INDEXED WHERE family = ?1 AND key = ?2",
            "SCAN versions",
        ),
    ] {
        assert_eq!(plan(&conn, sql), [want], "{sql}");
    }
    let refused = conn
        .prepare("SELECT rev FROM versions INDEXED BY nosuch WHERE family = ?1")
        .err()
        .map(|e| e.to_string());
    assert!(
        refused
            .as_deref()
            .is_some_and(|e| e.contains("no such index: nosuch")),
        "{refused:?}"
    );
}

/// Do: plan, without statistics, a composite primary key's two-column prefix against a secondary
/// index on its first column, three equality columns against two, and an ORDER BY with a LIMIT that
/// an index's order takes against an equality prefix and a sort. Expect: SQLite 3.54.0's plan for
/// each, one search with no sort: the index matching more equality columns wins, and the one giving
/// the order a LIMIT reads in wins over one that would sort.
#[test]
fn more_equality_columns_and_the_order_a_limit_reads_in_win_without_statistics() {
    let dir = tempfile::tempdir().unwrap();
    let conn = versions(dir.path());
    for sql in [
        "CREATE TABLE p (a INT, b INT, c INT, d INT, PRIMARY KEY (a, b, c))",
        "CREATE INDEX p_ad ON p (a, d)",
        "CREATE TABLE q (a INT, b INT, c INT, v TEXT, PRIMARY KEY (a, b))",
        "CREATE INDEX q_ac ON q (a, c)",
        "CREATE TABLE r (a INT, b INT, c INT, d INT)",
        "CREATE INDEX r2 ON r (a, b)",
        "CREATE INDEX r3 ON r (a, b, c)",
    ] {
        conn.execute(sql).unwrap();
    }
    for (sql, want) in [
        (
            "SELECT d FROM p WHERE a = ?1 AND b = ?2",
            "SEARCH p USING INDEX sqlite_autoindex_p_1 (a=? AND b=?)",
        ),
        (
            "SELECT v FROM q WHERE a = ?1 ORDER BY c LIMIT 10",
            "SEARCH q USING INDEX q_ac (a=?)",
        ),
        (
            "SELECT v FROM q WHERE a = ?1 AND c > ?2 ORDER BY c LIMIT ?3",
            "SEARCH q USING INDEX q_ac (a=? AND c>?)",
        ),
        (
            "SELECT d FROM r WHERE a = ?1 AND b = ?2 AND c = ?3",
            "SEARCH r USING INDEX r3 (a=? AND b=? AND c=?)",
        ),
    ] {
        assert_eq!(plan(&conn, sql), [want], "{sql}");
    }
}
