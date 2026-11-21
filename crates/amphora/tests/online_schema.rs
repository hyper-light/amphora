//! Online schema changes by F1's states (CP-T1.12, T-T1.12.1; docs/design/online-schema.md): an
//! index built while writes go on is public only once whole, reads never use it before, and rows
//! that break a unique one undo the change.

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

use std::time::{Duration, Instant};

use amphora::database::Database;
use amphora::database::online::{self, Plan, Stepped};
use amphora::db::Db;
use amphora_api::{Migration, Refusal};
use amphora_engine::Value;

fn plan(sql: &str) -> Plan {
    Plan::parse(&Migration {
        name: "ix".into(),
        sql: sql.into(),
    })
    .unwrap()
}

fn count(db: &Db, sql: &str, params: Vec<Value>) -> anyhow::Result<i64> {
    let mut n = 0;
    db.query_owned(sql, params, |r| {
        n = r.get::<i64>(0).map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(true)
    })?;
    Ok(n)
}

fn plan_text(db: &Db, sql: &str) -> String {
    let mut out = String::new();
    db.query_owned(&format!("EXPLAIN QUERY PLAN {sql}"), [], |r| {
        out.push_str(&r.get::<String>(3).map_err(|e| anyhow::anyhow!("{e}"))?);
        out.push('\n');
        Ok(true)
    })
    .unwrap();
    out
}

fn integrity(db: &Db) -> String {
    let mut check = String::new();
    db.query("PRAGMA integrity_check", [], |r| {
        check = r.get::<String>(0).map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(true)
    })
    .unwrap();
    check
}

fn table(rows: i64) -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Db::open(&dir.path().join("t.db")).unwrap();
    db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, a INTEGER, b TEXT); CREATE TABLE __amphora_migrations (name TEXT PRIMARY KEY, sql TEXT NOT NULL)")
        .unwrap();
    db.transaction(|db| {
        for i in 0..rows {
            db.query(
                "INSERT INTO t (a, b) VALUES (?1, ?2)",
                [Value::from_i64(i % 97), Value::from_text(format!("r{i}"))],
                |_| Ok(true),
            )?;
        }
        Ok(())
    })
    .unwrap();
    (dir, db)
}

fn write(db: &mut Db, i: i64) {
    db.transaction(|db| {
        db.query(
            "INSERT INTO t (a, b) VALUES (?1, 'w')",
            [Value::from_i64(i % 97)],
            |_| Ok(true),
        )?;
        db.query(
            "UPDATE t SET a = ?1 WHERE id = ?2",
            [Value::from_i64((i * 7) % 97), Value::from_i64(i * 13 + 1)],
            |_| Ok(true),
        )?;
        db.query(
            "DELETE FROM t WHERE id = ?1",
            [Value::from_i64(i * 17 + 2)],
            |_| Ok(true),
        )
    })
    .unwrap();
}

/// Every value's rows read through the index equal those read without it.
fn whole(db: &Db) {
    assert_eq!(integrity(db), "ok");
    let all = count(db, "SELECT count(*) FROM t NOT INDEXED", vec![]).unwrap();
    assert_eq!(
        count(
            db,
            "SELECT count(*) FROM t INDEXED BY ix WHERE a >= 0",
            vec![]
        )
        .unwrap(),
        all
    );
    for v in 0..97 {
        assert_eq!(
            count(
                db,
                "SELECT count(*) FROM t INDEXED BY ix WHERE a = ?1",
                vec![Value::from_i64(v)]
            )
            .unwrap(),
            count(
                db,
                "SELECT count(*) FROM t NOT INDEXED WHERE a = ?1",
                vec![Value::from_i64(v)]
            )
            .unwrap(),
            "value {v}"
        );
    }
}

#[test]
fn t_t1_12_1_writes_during_the_change_keep_the_index_and_reads_wait_for_public() {
    // CP-T1.12: build ix on 1,000 rows in batches of 7 while each step's gap inserts, updates and
    // deletes rows. Expect: no plan, the writer's or a reader's, names ix before its cutover; after
    // it, ix has every row's entry and only theirs, and the integrity check passes.
    let (_dir, mut db) = table(1000);
    let reader = db.reader().unwrap();
    let ix = plan("CREATE INDEX ix ON t (a)");
    let query = "SELECT b FROM t WHERE a = 5";
    let mut steps = 0;
    loop {
        let stepped = db.transaction(|db| online::step(db, &ix, 7)).unwrap();
        steps += 1;
        if let Stepped::Public { .. } = stepped {
            break;
        }
        assert!(
            !plan_text(&db, query).contains("ix"),
            "the writer read ix before public"
        );
        // The reader reads first: a read prepares under the schema the writer published, where a
        // cached EXPLAIN, which opens no transaction, keeps the plan of the schema it was made in.
        assert!(count(&reader, "SELECT count(*) FROM t INDEXED BY ix", vec![]).is_err());
        assert!(
            !plan_text(&reader, query).contains("ix"),
            "a reader read ix before public"
        );
        assert!(count(&db, "SELECT count(*) FROM t INDEXED BY ix", vec![]).is_err());
        assert_eq!(integrity(&db), "ok");
        write(&mut db, steps);
        assert!(steps < 1000, "the change never ended");
    }
    assert!(
        steps > 100,
        "the backfill ran in {steps} steps, not batches of 7"
    );
    let writer_plan = plan_text(&db, query);
    assert!(
        writer_plan.contains("ix"),
        "the writer's plan after public: {writer_plan}"
    );
    assert_eq!(
        count(
            &reader,
            "SELECT count(*) FROM t INDEXED BY ix WHERE a = 5",
            vec![]
        )
        .unwrap(),
        count(
            &reader,
            "SELECT count(*) FROM t NOT INDEXED WHERE a = 5",
            vec![]
        )
        .unwrap(),
        "a reader's read through ix after public"
    );
    let reader_plan = plan_text(&reader, query);
    assert!(
        reader_plan.contains("ix"),
        "a reader's plan after public: {reader_plan}"
    );
    whole(&db);
    assert_eq!(
        db.transaction(|db| online::step(db, &ix, 7)).unwrap(),
        Stepped::Already
    );
}

#[test]
fn t_t1_12_1_a_write_only_unique_index_refuses_a_duplicate_write() {
    // CP-T1.12, F1's write-only state enforces the constraint it adds: once ix is write-only, two
    // rows written with the same a are refused; rows from before it aren't checked until backfill.
    let (_dir, mut db) = table(10);
    let ix = plan("CREATE UNIQUE INDEX ix ON t (a)");
    assert_eq!(
        db.transaction(|db| online::step(db, &ix, 7)).unwrap(),
        Stepped::DeleteOnly
    );
    assert_eq!(
        db.transaction(|db| online::step(db, &ix, 7)).unwrap(),
        Stepped::WriteOnly
    );
    db.exec("INSERT INTO t (a, b) VALUES (500, 'x')").unwrap();
    assert!(db.exec("INSERT INTO t (a, b) VALUES (500, 'y')").is_err());
    // An index or table in a change is the change's: neither drops under it.
    assert!(db.exec("DROP INDEX ix").is_err());
    assert!(db.exec("DROP TABLE t").is_err());
}

#[test]
fn t_t1_12_1_rows_that_break_a_unique_index_leave_the_old_schema() {
    // CP-T1.12, T-T1.12.1: a unique index over 300 rows whose a repeats. Expect: the change is
    // refused as BROKEN, ix is gone with every record of the change, the rows are untouched, and
    // the same index without UNIQUE then builds.
    let dir = tempfile::tempdir().unwrap();
    let until = || Instant::now() + Duration::from_secs(60);
    let database = Database::open(&dir.path().join("d.db"), until()).unwrap();
    database
        .migrate(
            Migration {
                name: "t".into(),
                sql: "CREATE TABLE t (id INTEGER PRIMARY KEY, a INTEGER); WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 300) INSERT INTO t (a) SELECT i % 50 FROM n".into(),
            },
            until(),
        )
        .unwrap();
    let broken = database
        .change_schema(
            Migration {
                name: "ux".into(),
                sql: "CREATE UNIQUE INDEX ix ON t (a)".into(),
            },
            until(),
        )
        .unwrap_err();
    assert!(
        matches!(&broken, Refusal::Invalid { what, .. } if what == online::BROKEN),
        "{broken:?}"
    );
    // The index is gone; its state and change rows too, or the same name couldn't stage again.
    let left = database
        .call(until(), |c| {
            let index: i64 = c.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name = 'ix'",
                [],
                |r| r.get(0usize),
            )?;
            let rows: i64 = c.query_row("SELECT count(*) FROM t", [], |r| r.get(0usize))?;
            Ok((index, rows))
        })
        .unwrap();
    assert_eq!(left, (0, 300));
    let built = database
        .change_schema(
            Migration {
                name: "nx".into(),
                sql: "CREATE INDEX ix ON t (a)".into(),
            },
            until(),
        )
        .unwrap();
    assert!(
        built.applied && built.cutover.is_some() && built.rows >= 300,
        "{built:?}"
    );
    let again = database
        .change_schema(
            Migration {
                name: "nx".into(),
                sql: "CREATE INDEX ix ON t (a)".into(),
            },
            until(),
        )
        .unwrap();
    assert!(!again.applied);
}
