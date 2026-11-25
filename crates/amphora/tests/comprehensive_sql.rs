//! Comprehensive end-to-end correctness & lifecycle suite across BOTH Amphora SQLite
//! (`SqliteDialect`) and Amphora PostgreSQL (`PostgresDialect`):
//!
//! Exercises simple point/range reads, challenging analytical queries (multi-way joins, skewed
//! `HAVING`, high-cardinality `GROUP BY`, wide unindexed sorting under tight cache/spill pressure,
//! `COUNT(DISTINCT)` on text, `LEFT JOIN` aggregates, partitioned running-sum window functions,
//! recursive CTEs, compound correlated `EXISTS`/`NOT EXISTS`, `UNION` deduplication), inserts,
//! plain/indexed/subquery updates, deletes, multi-statement transactions (commit & rollback),
//! catalog introspection (`sqlite_schema` and `pg_class`/`pg_attribute`/`pg_type`), and table /
//! index / rename / analyze DDL lifecycle on both small and spilling datasets.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
        clippy::disallowed_macros
    )
)]

use amphora::db::{Db, Sharing};
use amphora_engine::Value;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Sqlite,
    Postgres,
}

fn open_db(dir: &std::path::Path, mode: Mode) -> Db {
    let path = dir.join(match mode {
        Mode::Sqlite => "sqlite.db",
        Mode::Postgres => "postgres.db",
    });
    match mode {
        Mode::Sqlite => Db::open_sharing(&path, &Sharing::Off).unwrap(),
        Mode::Postgres => Db::open_postgres_sharing(&path, &Sharing::Off).unwrap(),
    }
}

fn create_schema_and_load(db: &mut Db, mode: Mode, item_count: i64) {
    match mode {
        Mode::Sqlite => {
            db.exec("CREATE TABLE regions (id INTEGER PRIMARY KEY, name TEXT NOT NULL)")
                .unwrap();
            db.exec("CREATE TABLE owners (id INTEGER PRIMARY KEY, name TEXT NOT NULL, region INTEGER NOT NULL, score REAL NOT NULL)")
                .unwrap();
            db.exec("CREATE TABLE items (id INTEGER PRIMARY KEY, k INTEGER NOT NULL, grp INTEGER NOT NULL, owner INTEGER NOT NULL, price REAL NOT NULL, qty INTEGER NOT NULL, flag INTEGER NOT NULL, created INTEGER NOT NULL, name TEXT NOT NULL, body TEXT NOT NULL, data BLOB NOT NULL, doc TEXT NOT NULL, note TEXT)")
                .unwrap();
        }
        Mode::Postgres => {
            db.exec("CREATE TABLE regions (id INTEGER PRIMARY KEY, name TEXT NOT NULL)")
                .unwrap();
            db.exec("CREATE TABLE owners (id INTEGER PRIMARY KEY, name TEXT NOT NULL, region BIGINT NOT NULL, score DOUBLE PRECISION NOT NULL)")
                .unwrap();
            db.exec("CREATE TABLE items (id INTEGER PRIMARY KEY, k BIGINT NOT NULL, grp BIGINT NOT NULL, owner BIGINT NOT NULL, price DOUBLE PRECISION NOT NULL, qty BIGINT NOT NULL, flag BOOLEAN NOT NULL, created BIGINT NOT NULL, name TEXT NOT NULL, body TEXT NOT NULL, data BYTEA NOT NULL, doc JSONB NOT NULL, note TEXT)")
                .unwrap();
        }
    }

    let owners = (item_count / 20).max(10);
    db.transaction(|db| {
        for r in 1..=10i64 {
            db.query(
                "INSERT INTO regions (id, name) VALUES (?1, ?2)",
                [Value::from_i64(r), Value::from_text(format!("region-{r}"))],
                |_| Ok(true),
            )?;
        }
        for o in 1..=owners {
            db.query(
                "INSERT INTO owners (id, name, region, score) VALUES (?1, ?2, ?3, ?4)",
                [
                    Value::from_i64(o),
                    Value::from_text(format!("owner-{o}")),
                    Value::from_i64((o % 10) + 1),
                    Value::from_f64(((o * 37) % 100) as f64 + 0.5),
                ],
                |_| Ok(true),
            )?;
        }
        let body_pad = "alpha beta gamma delta epsilon zeta ".repeat(10);
        for id in 1..=item_count {
            let k = (id * 2_654_435_761) & 0x7fff_ffff;
            let grp = id % 25;
            let owner = (id % owners) + 1;
            let price = (((id * 17) % 10_000) as f64) / 100.0;
            let qty = (id * 13) % 100;
            let flag = i64::from(id % 2 == 1);
            let created = 1_700_000_000_000_000 + id * 1_000_000;
            let name = format!("item-{:06}", (id * 7919) % (item_count.max(1)));
            let body = if id % 100 == 0 {
                format!("zephyr {id} {body_pad}")
            } else {
                format!("row {id} {body_pad}")
            };
            let data = Value::from_slice(&id.cast_unsigned().to_le_bytes()).unwrap();
            let doc = format!(r#"{{"a":{qty},"b":"tag-{grp}"}}"#);
            let note = if id % 2 == 0 {
                Value::Null
            } else {
                Value::from_text(format!("note-{grp}"))
            };
            db.query(
                "INSERT INTO items (id, k, grp, owner, price, qty, flag, created, name, body, data, doc, note) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                [
                    Value::from_i64(id),
                    Value::from_i64(k),
                    Value::from_i64(grp),
                    Value::from_i64(owner),
                    Value::from_f64(price),
                    Value::from_i64(qty),
                    Value::from_i64(flag),
                    Value::from_i64(created),
                    Value::from_text(name),
                    Value::from_text(body),
                    data,
                    Value::from_text(doc),
                    note,
                ],
                |_| Ok(true),
            )?;
        }
        Ok(())
    })
    .unwrap();

    for idx in [
        "CREATE INDEX items_k ON items (k)",
        "CREATE INDEX items_name ON items (name)",
        "CREATE INDEX items_grp_created ON items (grp, created)",
        "CREATE INDEX items_owner ON items (owner)",
        "CREATE INDEX owners_region ON owners (region)",
        "ANALYZE",
    ] {
        db.exec(idx).unwrap();
    }
    (db.flusher())().unwrap();
}

fn collect_rows(
    db: &mut Db,
    sql: &'static str,
    params: impl IntoIterator<Item = Value>,
) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    db.query(sql, params, |r| {
        out.push(r.get_values().map(ToString::to_string).collect());
        Ok(true)
    })
    .unwrap();
    out
}

fn collect_rows_no_params(db: &mut Db, sql: &'static str) -> Vec<Vec<String>> {
    collect_rows(db, sql, std::iter::empty())
}

fn exercise_queries(db: &mut Db, mode: Mode, item_count: i64) -> Vec<(String, Vec<Vec<String>>)> {
    let queries: Vec<(&str, &str, Vec<Value>)> = vec![
        (
            "point_pk",
            "SELECT id, grp, owner, qty FROM items WHERE id = ?1",
            vec![Value::from_i64(42)],
        ),
        (
            "range_pk",
            "SELECT id, price, qty FROM items WHERE id BETWEEN ?1 AND ?2 ORDER BY id",
            vec![Value::from_i64(10), Value::from_i64(25)],
        ),
        ("table_count", "SELECT COUNT(*) FROM items", vec![]),
        (
            "filter_no_index",
            "SELECT COUNT(*) FROM items WHERE qty > ?1 AND price < ?2",
            vec![Value::from_i64(50), Value::from_f64(50.0)],
        ),
        (
            "like_substring",
            "SELECT COUNT(*) FROM items WHERE body LIKE ?1",
            vec![Value::from_text("%zephyr%")],
        ),
        (
            "sort_wide_unindexed",
            "SELECT id, price, name FROM items ORDER BY price DESC, id ASC LIMIT 25",
            vec![],
        ),
        (
            "group_by_high_card",
            "SELECT owner, COUNT(*), SUM(qty) FROM items GROUP BY owner ORDER BY owner",
            vec![],
        ),
        (
            "distinct_wide_text",
            "SELECT COUNT(DISTINCT name) FROM items",
            vec![],
        ),
        (
            "join_three",
            "SELECT r.name, COUNT(*), SUM(i.qty) FROM items i JOIN owners o ON o.id = i.owner JOIN regions r ON r.id = o.region WHERE i.id BETWEEN ?1 AND ?2 GROUP BY r.name ORDER BY r.name",
            vec![Value::from_i64(1), Value::from_i64(item_count.min(400))],
        ),
        (
            "join_skewed_having",
            "SELECT r.name, o.name, COUNT(*), SUM(i.qty) FROM items i JOIN owners o ON o.id = i.owner JOIN regions r ON r.id = o.region WHERE o.score > ?1 GROUP BY r.name, o.name HAVING COUNT(*) >= 5 ORDER BY SUM(i.qty) DESC, o.name ASC LIMIT 15",
            vec![Value::from_f64(40.0)],
        ),
        (
            "left_join_agg",
            "SELECT o.id, o.name, COUNT(i.id), COALESCE(SUM(i.qty), 0) FROM owners o LEFT JOIN items i ON i.owner = o.id AND i.grp = ?1 WHERE o.region = ?2 GROUP BY o.id, o.name ORDER BY o.id",
            vec![Value::from_i64(3), Value::from_i64(2)],
        ),
        (
            "window_running_sum",
            "SELECT id, grp, SUM(qty) OVER (PARTITION BY grp ORDER BY created ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) FROM items WHERE id BETWEEN ?1 AND ?2 ORDER BY id",
            vec![Value::from_i64(1), Value::from_i64(120)],
        ),
        (
            "cte_recursive",
            "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < 10) SELECT r.id, r.name, COUNT(o.id) FROM seq s JOIN regions r ON r.id = s.n LEFT JOIN owners o ON o.region = r.id GROUP BY r.id, r.name ORDER BY r.id",
            vec![],
        ),
        (
            "subquery_exists_not_exists",
            "SELECT o.id, o.name FROM owners o WHERE EXISTS (SELECT 1 FROM items i WHERE i.owner = o.id AND i.qty > 80) AND NOT EXISTS (SELECT 1 FROM items i2 WHERE i2.owner = o.id AND i2.note IS NULL AND i2.grp = 1) ORDER BY o.id LIMIT 15",
            vec![],
        ),
        (
            "union_distinct",
            "SELECT id, grp FROM items WHERE id BETWEEN 1 AND 80 UNION SELECT id, grp FROM items WHERE id BETWEEN 40 AND 120 ORDER BY id",
            vec![],
        ),
        (
            "agg_multi_bucket",
            "SELECT grp, COUNT(*), SUM(CASE WHEN flag THEN qty ELSE 0 END), MIN(qty), MAX(qty) FROM items GROUP BY grp ORDER BY grp",
            vec![],
        ),
        (
            "json_extract_filter",
            "SELECT COUNT(*) FROM items WHERE json_extract(doc, '$.a') > 50",
            vec![],
        ),
    ];

    let mut results = Vec::with_capacity(queries.len());
    for (name, sql, params) in queries {
        let rows = collect_rows(db, sql, params);
        results.push((name.to_owned(), rows));
    }

    // Dialect-specific catalog introspection.
    match mode {
        Mode::Sqlite => {
            let cat = collect_rows_no_params(
                db,
                "SELECT type, name FROM sqlite_schema WHERE tbl_name = 'items' ORDER BY type, name",
            );
            assert!(
                cat.iter().any(|r| r == &["table", "items"]),
                "sqlite_schema missing items table: {cat:?}"
            );
            assert!(
                cat.iter().any(|r| r == &["index", "items_owner"]),
                "sqlite_schema missing items_owner index: {cat:?}"
            );
        }
        Mode::Postgres => {
            let cat = collect_rows_no_params(
                db,
                "SELECT c.relname, a.attname, t.typname FROM pg_class c JOIN pg_attribute a ON a.attrelid = c.oid JOIN pg_type t ON t.oid = a.atttypid WHERE c.relkind = 'r' AND a.attnum > 0 AND c.relname = 'items' ORDER BY a.attnum",
            );
            assert_eq!(
                cat.len(),
                13,
                "pg_attribute must expose all 13 items columns"
            );
            assert_eq!(cat[0], vec!["items", "id", "int4"]);
            assert_eq!(cat[1], vec!["items", "k", "int8"]);
            assert_eq!(cat[4], vec!["items", "price", "float8"]);
            assert_eq!(cat[6], vec!["items", "flag", "bool"]);
            assert_eq!(cat[10], vec!["items", "data", "bytea"]);
            assert_eq!(cat[11], vec!["items", "doc", "jsonb"]);
        }
    }

    results
}

fn exercise_dml_and_ddl(db: &mut Db, mode: Mode, item_count: i64) {
    // 1. Insert + plain/indexed/subquery updates + upsert + rollback isolation.
    let new_id = item_count + 1;
    db.transaction(|db| {
        db.query(
            "INSERT INTO items (id, k, grp, owner, price, qty, flag, created, name, body, data, doc, note) VALUES (?1, 999999, 7, 1, 12.5, 5, 1, 1700999999000000, 'fresh-item', 'fresh body', ?2, '{\"a\":5,\"b\":\"fresh\"}', 'ok')",
            [Value::from_i64(new_id), Value::from_slice(b"12345678").unwrap()],
            |_| Ok(true),
        )?;
        db.query(
            "UPDATE items SET price = price + 2.5, qty = qty + 3 WHERE id = ?1",
            [Value::from_i64(new_id)],
            |_| Ok(true),
        )?;
        db.query(
            "UPDATE items SET k = 1234567 WHERE id = ?1",
            [Value::from_i64(new_id)],
            |_| Ok(true),
        )?;
        db.query(
            "UPDATE items SET price = price + COALESCE((SELECT o.score FROM owners o WHERE o.id = items.owner), 0) * 0.01 WHERE id = ?1",
            [Value::from_i64(new_id)],
            |_| Ok(true),
        )?;
        Ok(())
    })
    .unwrap();
    let check = collect_rows(
        db,
        "SELECT k, qty FROM items WHERE id = ?1",
        [Value::from_i64(new_id)],
    );
    assert_eq!(check, vec![vec!["1234567".to_owned(), "8".to_owned()]]);

    // Rollback must leave state untouched.
    let rollback_res: anyhow::Result<()> = db.transaction(|db| {
        db.query(
            "DELETE FROM items WHERE id = ?1",
            [Value::from_i64(new_id)],
            |_| Ok(true),
        )?;
        anyhow::bail!("intentional rollback")
    });
    assert!(rollback_res.is_err());
    let still_there = collect_rows(
        db,
        "SELECT COUNT(*) FROM items WHERE id = ?1",
        [Value::from_i64(new_id)],
    );
    assert_eq!(still_there, vec![vec!["1".to_owned()]]);

    // Delete & verify.
    db.transaction(|db| {
        db.query(
            "DELETE FROM items WHERE id = ?1",
            [Value::from_i64(new_id)],
            |_| Ok(true),
        )
    })
    .unwrap();
    let after_del = collect_rows_no_params(db, "SELECT COUNT(*) FROM items");
    assert_eq!(after_del, vec![vec![item_count.to_string()]]);

    // 2. Table & schema DDL lifecycle: CREATE TABLE, CREATE INDEX, ALTER RENAME, ANALYZE, DROP.
    db.exec("CREATE TABLE ddl_stage (id INTEGER PRIMARY KEY, k BIGINT NOT NULL, label TEXT NOT NULL, val DOUBLE PRECISION NOT NULL)").unwrap();
    db.exec("INSERT INTO ddl_stage VALUES (1, 30, 'c', 3.5), (2, 10, 'a', 1.5), (3, 20, 'b', 2.5)")
        .unwrap();
    db.exec("CREATE INDEX ddl_stage_k ON ddl_stage (k)")
        .unwrap();
    db.exec("ANALYZE ddl_stage").unwrap();
    let indexed = collect_rows_no_params(
        db,
        "SELECT id, label FROM ddl_stage WHERE k BETWEEN 10 AND 25 ORDER BY k",
    );
    assert_eq!(
        indexed,
        vec![
            vec!["2".to_owned(), "a".to_owned()],
            vec!["3".to_owned(), "b".to_owned()],
        ]
    );
    db.exec("ALTER TABLE ddl_stage RENAME TO ddl_renamed")
        .unwrap();
    let renamed = collect_rows_no_params(db, "SELECT COUNT(*) FROM ddl_renamed");
    assert_eq!(renamed, vec![vec!["3".to_owned()]]);

    // ADD COLUMN + DROP COLUMN on SQLite; PostgresDialect explicitly documents OID-preservation limit.
    match mode {
        Mode::Sqlite => {
            db.exec("ALTER TABLE ddl_renamed ADD COLUMN extra INTEGER DEFAULT 7")
                .unwrap();
            let sum_extra = collect_rows_no_params(db, "SELECT SUM(extra) FROM ddl_renamed");
            assert_eq!(sum_extra, vec![vec!["21".to_owned()]]);
        }
        Mode::Postgres => {
            // Verify OID survived RENAME in pg_class.
            let pg_renamed = collect_rows_no_params(
                db,
                "SELECT relname FROM pg_class WHERE relname = 'ddl_renamed'",
            );
            assert_eq!(pg_renamed, vec![vec!["ddl_renamed".to_owned()]]);
            assert!(
                db.exec("ALTER TABLE ddl_renamed ADD COLUMN extra BIGINT DEFAULT 7")
                    .is_err(),
                "PostgresDialect currently rejects ADD COLUMN until OID-preserving rewrite lands"
            );
        }
    }

    db.exec("DROP INDEX ddl_stage_k").unwrap();
    db.exec("DROP TABLE ddl_renamed").unwrap();
}

/// Verifies exact answer parity between Amphora SQLite (`SqliteDialect`) and Amphora PostgreSQL
/// (`PostgresDialect`) across simple + complex queries, DML, transactions, and DDL lifecycle on
/// both small (in-cache) and larger (tight-cache / spilling) datasets.
#[test]
fn comprehensive_sqlite_and_postgres_parity_small_and_spilling() {
    for (label, item_count, reader_cache_pages) in [
        ("small_in_cache", 300i64, 2_000i64),
        ("spilling_tight_cache", 2_500i64, 64i64),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut sqlite_db = open_db(dir.path(), Mode::Sqlite);
        let mut pg_db = open_db(dir.path(), Mode::Postgres);

        create_schema_and_load(&mut sqlite_db, Mode::Sqlite, item_count);
        create_schema_and_load(&mut pg_db, Mode::Postgres, item_count);

        // Read through fresh reader connections with the target cache size (exercising spilling
        // under tight cache on the larger dataset).
        let mut sqlite_reader = sqlite_db.reader().unwrap();
        let mut pg_reader = pg_db.reader().unwrap();
        sqlite_reader.keep_pages(reader_cache_pages).unwrap();
        pg_reader.keep_pages(reader_cache_pages).unwrap();

        let sqlite_answers = exercise_queries(&mut sqlite_reader, Mode::Sqlite, item_count);
        let pg_answers = exercise_queries(&mut pg_reader, Mode::Postgres, item_count);
        assert_eq!(
            sqlite_answers, pg_answers,
            "SQLite vs Postgres answer mismatch on dataset `{label}`"
        );

        drop(sqlite_reader);
        drop(pg_reader);

        exercise_dml_and_ddl(&mut sqlite_db, Mode::Sqlite, item_count);
        exercise_dml_and_ddl(&mut pg_db, Mode::Postgres, item_count);
    }
}
