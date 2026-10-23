//! The planner's choices on the shapes Amphora's bench suite (`bench --suite`) found it getting
//! wrong, each a statement of the suite on a smaller copy of its tables (docs/design/planner.md):
//! a join from a key range looks each row's owner up by key instead of probing the range's table
//! once for every owner, and a sort by the key reads in key order instead of sorting a covering
//! index (AC-T1.1.2, AC-T1.13.4: reads at their latency).

use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, OpenOptions, PlatformIO, SqliteDialect,
};
use std::path::Path;
use std::sync::Arc;

const OWNERS: i64 = 400;
const ITEMS: i64 = 20_000;

/// A scatter of `id` over 32 bits, so an owner's items lie all over the table as the suite's do.
fn mix(id: i64) -> i64 {
    id.wrapping_mul(2_654_435_761) & 0xffff_ffff
}

/// The suite's tables, indexes and statistics, with a fiftieth of its rows.
fn tables(dir: &Path) -> Arc<Connection> {
    let io = Arc::new(PlatformIO::new().unwrap());
    let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
    let db = Database::open(io, dir.join("suite.db").to_str().unwrap(), options).unwrap();
    let conn = db.connect(ConnectOpts::new()).unwrap();
    conn.execute("CREATE TABLE regions (id INTEGER PRIMARY KEY, name TEXT NOT NULL)")
        .unwrap();
    conn.execute("CREATE TABLE owners (id INTEGER PRIMARY KEY, name TEXT NOT NULL, region INTEGER NOT NULL, score REAL NOT NULL)").unwrap();
    conn.execute("CREATE TABLE items (id INTEGER PRIMARY KEY, grp INTEGER NOT NULL, owner INTEGER NOT NULL, price REAL NOT NULL, created INTEGER NOT NULL, name TEXT NOT NULL)").unwrap();
    conn.execute("BEGIN").unwrap();
    let regions: Vec<String> = (1..=20)
        .map(|id| format!("({id}, 'region-{id}')"))
        .collect();
    conn.execute(format!("INSERT INTO regions VALUES {}", regions.join(", ")))
        .unwrap();
    let owners: Vec<String> = (1..=OWNERS)
        .map(|id| format!("({id}, 'owner-{id}', {}, {}.5)", id % 20 + 1, id % 97))
        .collect();
    conn.execute(format!("INSERT INTO owners VALUES {}", owners.join(", ")))
        .unwrap();
    for first in (1..=ITEMS).step_by(500) {
        let rows: Vec<String> = (first..first + 500)
            .map(|id| {
                format!(
                    "({id}, {}, {}, {}.25, {}, 'name-{:08x}')",
                    mix(id) % 100,
                    mix(id ^ 0x5555) % OWNERS + 1,
                    mix(id) % 10_000,
                    1_700_000_000 + id * 60,
                    mix(id)
                )
            })
            .collect();
        conn.execute(format!("INSERT INTO items VALUES {}", rows.join(", ")))
            .unwrap();
    }
    conn.execute("COMMIT").unwrap();
    for index in [
        "CREATE INDEX items_name ON items (name)",
        "CREATE INDEX items_grp_created ON items (grp, created)",
        "CREATE INDEX items_owner ON items (owner)",
        "CREATE INDEX owners_region ON owners (region)",
        "ANALYZE",
    ] {
        conn.execute(index).unwrap();
    }
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

/// Do: join items to their owners for 20 consecutive ids (the suite's join_pk). Expect: the id
/// range is read and each row's owner looked up by key, not every owner's items probed and
/// filtered to the range (400 index searches and 20,000 index rows for 20 results).
#[test]
fn a_join_from_a_key_range_looks_each_owner_up_by_key() {
    let dir = tempfile::tempdir().unwrap();
    let conn = tables(dir.path());
    let steps = plan(&conn, "SELECT i.id, i.price, o.name FROM items i JOIN owners o ON o.id = i.owner WHERE i.id BETWEEN ? AND ?");
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert!(
        steps[0].starts_with("SEARCH i USING INTEGER PRIMARY KEY (rowid>"),
        "{steps:?}"
    );
    assert_eq!(
        steps[1], "SEARCH o USING INTEGER PRIMARY KEY (rowid=?)",
        "{steps:?}"
    );
}

/// Do: join three tables by key from an id range and group (the suite's join_three). Expect: the
/// range first, then a key lookup into each of the other two.
#[test]
fn three_tables_joined_by_key_start_from_the_key_range() {
    let dir = tempfile::tempdir().unwrap();
    let conn = tables(dir.path());
    let steps = plan(&conn, "SELECT r.name, COUNT(*), AVG(i.price) FROM items i JOIN owners o ON o.id = i.owner JOIN regions r ON r.id = o.region WHERE i.id BETWEEN ? AND ? GROUP BY r.name");
    assert!(
        steps[0].starts_with("SEARCH i USING INTEGER PRIMARY KEY (rowid>"),
        "{steps:?}"
    );
    assert_eq!(
        steps[1], "SEARCH o USING INTEGER PRIMARY KEY (rowid=?)",
        "{steps:?}"
    );
    assert_eq!(
        steps[2], "SEARCH r USING INTEGER PRIMARY KEY (rowid=?)",
        "{steps:?}"
    );
}

/// Do: page through the table by key, 20 rows past an offset, either way (the suite's
/// offset_deep). Expect: the table read in key order, which stops after offset + 20 rows, never a
/// covering index read whole and sorted.
#[test]
fn a_page_by_key_reads_in_key_order_without_sorting() {
    let dir = tempfile::tempdir().unwrap();
    let conn = tables(dir.path());
    for order in ["ASC", "DESC"] {
        let steps = plan(
            &conn,
            &format!("SELECT id, name FROM items ORDER BY id {order} LIMIT 20 OFFSET ?"),
        );
        assert_eq!(steps, vec!["SCAN items".to_owned()], "ORDER BY id {order}");
    }
}

/// Do: GROUP BY the first column of an index that lacks the aggregated columns, over the whole
/// table, with the connection's cache a fraction of the table and then holding all of it. Expect:
/// a scan of the table, its groups hashed (docs/design/executor.md, decision 4), while each lookup
/// through the index would read a page of its own (docs/design/planner.md, decision 5); the
/// index's order, ungrouped by hash or sort, once the cache holds the table.
#[test]
fn a_whole_table_group_by_scans_and_sorts_where_lookups_would_read_pages() {
    let dir = tempfile::tempdir().unwrap();
    let conn = tables(dir.path());
    let sql = "SELECT grp, COUNT(*), AVG(price) FROM items GROUP BY grp";
    conn.execute("PRAGMA cache_size = 200").unwrap();
    assert_eq!(
        plan(&conn, sql),
        vec![
            "SCAN items".to_owned(),
            "USE HASH TABLE FOR GROUP BY".to_owned()
        ]
    );
    conn.execute("PRAGMA cache_size = 100000").unwrap();
    assert_eq!(
        plan(&conn, sql),
        vec!["SCAN items USING INDEX items_grp_created".to_owned()]
    );
}

/// Do: join a grouped CTE (the sums of a few hundred owners' items) to the owners it names,
/// written as a CTE and as a subquery in FROM. Expect: its rows drive, each owner looked up by
/// key, rather than every owner scanned to probe an index built on it (docs/design/planner.md,
/// decision 6: the owners' pages fit in the cache, so after one search a search of them reads
/// nothing); and the rows the same join without a subquery returns.
#[test]
fn a_grouped_cte_drives_lookups_of_the_owners_it_names() {
    let dir = tempfile::tempdir().unwrap();
    let conn = tables(dir.path());
    let grouped = "SELECT owner, SUM(price) AS s FROM items WHERE grp = 7 GROUP BY owner";
    let forms = [
        format!("WITH t AS ({grouped}) SELECT o.name, t.s FROM t JOIN owners o ON o.id = t.owner ORDER BY t.s DESC, o.name LIMIT 5"),
        format!("SELECT o.name, t.s FROM ({grouped}) t JOIN owners o ON o.id = t.owner ORDER BY t.s DESC, o.name LIMIT 5"),
    ];
    let joined = "SELECT o.name, SUM(i.price) FROM items i JOIN owners o ON o.id = i.owner WHERE i.grp = 7 GROUP BY o.id ORDER BY 2 DESC, o.name LIMIT 5";
    let mut want = conn.prepare(joined).unwrap();
    let want: Vec<String> = want
        .run_collect_rows()
        .unwrap()
        .iter()
        .map(|row| format!("{} {}", row[0], row[1]))
        .collect();
    assert_eq!(want.len(), 5);
    for sql in &forms {
        assert_eq!(
            plan(&conn, sql),
            vec![
                "SCAN t".to_owned(),
                "SEARCH items USING INDEX items_grp_created (grp=?)".to_owned(),
                "USE HASH TABLE FOR GROUP BY".to_owned(),
                "SEARCH o USING INTEGER PRIMARY KEY (rowid=?)".to_owned(),
                "USE SORTER FOR ORDER BY".to_owned(),
            ],
            "{sql}"
        );
        let mut stmt = conn.prepare(sql).unwrap();
        let got: Vec<String> = stmt
            .run_collect_rows()
            .unwrap()
            .iter()
            .map(|row| format!("{} {}", row[0], row[1]))
            .collect();
        assert_eq!(got, want, "{sql}");
    }
}
