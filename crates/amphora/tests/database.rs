//! U05/U06, CP-T1.5: product behavior at the owned SQL/Records boundary.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros
)]

use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use amphora::database::Database;
use amphora_api::{
    Cond, Consistency, Migration, Records, Refusal, Scan, Statement, Tx, Value, Write,
};

fn until() -> Instant {
    Instant::now() + Duration::from_secs(30)
}
fn sql(text: &str, params: Vec<Value>) -> Statement {
    Statement {
        sql: text.into(),
        params,
    }
}
fn put(key: &[u8], value: &[u8]) -> Write {
    Write::Put {
        family: "items".into(),
        key: key.into(),
        value: value.into(),
    }
}
fn absent(key: &[u8]) -> Cond {
    Cond::Absent {
        family: "items".into(),
        key: key.into(),
    }
}

#[test]
fn change_pages_never_lose_the_rest_of_a_multi_key_revision() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("records.db"), until()).unwrap();
    let records = db.namespace("server").unwrap();
    let first = records
        .commit(
            Tx {
                conds: vec![],
                writes: vec![put(b"first", b"1")],
            },
            until(),
        )
        .unwrap();
    let second = records
        .commit(
            Tx {
                conds: vec![],
                writes: vec![put(b"a", b"2"), put(b"b", b"2")],
            },
            until(),
        )
        .unwrap();
    let page = records.changes("items", 0, 2, until()).unwrap();
    assert_eq!(page.changes.len(), 1);
    assert_eq!(page.changes[0].rev, first);
    assert!(matches!(
        records.changes("items", first, 1, until()),
        Err(Refusal::Invalid { .. })
    ));
    let rest = records.changes("items", first, 2, until()).unwrap();
    assert_eq!(rest.changes.len(), 2);
    assert!(rest.changes.iter().all(|c| c.rev == second));
}

#[test]
fn adaptive_sql_runs_once_rolls_back_errors_and_returns_after_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("records.db");
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let db = Database::open(&path, until()).unwrap();
        let counted = Arc::clone(&calls);
        let result: Result<(), Refusal> = db.call(until(), move |c| {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            c.execute_batch("CREATE TABLE discarded(x)")?;
            anyhow::bail!("application refused its transaction")
        });
        assert!(result.is_err());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        let value = db
            .call(until(), |c| {
                let count: i64 = c.query_row(
                    "SELECT count(*) FROM sqlite_schema WHERE name = 'discarded'",
                    [],
                    |r| r.get(0usize),
                )?;
                assert_eq!(count, 0);
                c.execute_batch(
                    "CREATE TABLE decisions(n INTEGER); INSERT INTO decisions VALUES (41)",
                )?;
                let next: i64 =
                    c.query_row("SELECT n + 1 FROM decisions", [], |r| r.get(0usize))?;
                c.execute("UPDATE decisions SET n = ?1", [next])?;
                Ok(next)
            })
            .unwrap();
        assert_eq!(value, 42);
        db.call(until(), |connection| {
            assert_eq!(
                connection
                    .prepare_cached("SELECT * FROM decisions")?
                    .column_count(),
                1
            );
            connection
                .execute_batch("ALTER TABLE decisions ADD COLUMN label TEXT DEFAULT 'ready'")?;
            assert_eq!(
                connection
                    .prepare_cached("SELECT * FROM decisions")?
                    .column_count(),
                2
            );
            assert_eq!(
                connection.query_row("SELECT * FROM decisions", [], |row| row
                    .get::<_, String>(1usize))?,
                "ready"
            );
            Ok(())
        })
        .unwrap();
    }
    let db = Database::open(&path, until()).unwrap();
    assert_eq!(
        db.call(until(), |c| Ok(c.query_row(
            "SELECT n FROM decisions",
            [],
            |r| r.get::<_, i64>(0usize)
        )?))
        .unwrap(),
        42
    );
}

#[test]
fn cancelled_watches_release_capacity_and_durable_changes_are_ready_on_first_poll() {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};
    let db = Database::in_memory(until()).unwrap();
    let records = db.namespace("server").unwrap();
    let mut context = Context::from_waker(Waker::noop());
    for _ in 0..5000 {
        let mut watch = std::pin::pin!(records.watch("items", 0).unwrap());
        assert!(watch.as_mut().poll(&mut context).is_pending());
    }
    let at = records
        .commit(
            Tx {
                conds: vec![],
                writes: vec![put(b"ready", b"now")],
            },
            until(),
        )
        .unwrap();
    let mut watch = std::pin::pin!(records.watch("items", 0).unwrap());
    assert!(matches!(watch.as_mut().poll(&mut context), Poll::Ready(Ok(rev)) if rev == at));
}

#[test]
fn owned_sql_migrations_keep_quoted_semicolons_and_trigger_bodies() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("records.db"), until()).unwrap();
    let migration = Migration { name: "001-trigger".into(), sql: "CREATE TABLE input(id INTEGER PRIMARY KEY, text TEXT, bytes BLOB); CREATE TABLE log(text TEXT); CREATE TRIGGER added AFTER INSERT ON input BEGIN INSERT INTO log VALUES ('before;'); INSERT INTO log VALUES (new.text); END;".into() };
    assert!(db.migrate(migration.clone(), until()).unwrap());
    assert!(!db.migrate(migration, until()).unwrap());
    assert!(matches!(
        db.migrate(
            Migration {
                name: "001-trigger".into(),
                sql: "CREATE TABLE different(x)".into()
            },
            until()
        ),
        Err(Refusal::Invalid { .. })
    ));
    let results = db
        .execute(
            vec![
                sql(
                    "INSERT INTO input(text, bytes) VALUES (?1, ?2) RETURNING id, text, bytes",
                    vec![Value::Text("after;".into()), Value::Blob(vec![0, 255])],
                ),
                sql("SELECT text FROM log ORDER BY rowid", vec![]),
            ],
            until(),
        )
        .unwrap();
    assert_eq!(
        results[0].rows,
        vec![vec![
            Value::Integer(1),
            Value::Text("after;".into()),
            Value::Blob(vec![0, 255])
        ]]
    );
    assert_eq!(
        results[1].rows,
        vec![
            vec![Value::Text("before;".into())],
            vec![Value::Text("after;".into())]
        ]
    );
    assert!(matches!(
        db.execute(vec![sql("COMMIT", vec![])], until()),
        Err(Refusal::Invalid { .. })
    ));
    assert!(
        db.execute(vec![sql("SELECT 1; SELECT 2", vec![])], until())
            .is_err()
    );
}

#[test]
fn failed_migration_and_sql_batch_roll_back_whole_and_can_be_retried() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("records.db"), until()).unwrap();
    assert!(
        db.migrate(
            Migration {
                name: "001-bad".into(),
                sql: "CREATE TABLE partial(x); INSERT INTO missing VALUES (1)".into()
            },
            until()
        )
        .is_err()
    );
    let rows = db
        .execute(
            vec![sql(
                "SELECT name FROM sqlite_schema WHERE name = 'partial'",
                vec![],
            )],
            until(),
        )
        .unwrap();
    assert!(rows[0].rows.is_empty());
    assert!(
        db.migrate(
            Migration {
                name: "001-bad".into(),
                sql: "CREATE TABLE partial(x)".into()
            },
            until()
        )
        .unwrap()
    );
    assert!(
        db.execute(
            vec![
                sql("INSERT INTO partial VALUES (1)", vec![]),
                sql("INSERT INTO missing VALUES (2)", vec![])
            ],
            until()
        )
        .is_err()
    );
    let rows = db
        .execute(vec![sql("SELECT count(*) FROM partial", vec![])], until())
        .unwrap();
    assert_eq!(rows[0].rows, vec![vec![Value::Integer(0)]]);
}

#[test]
fn records_cas_snapshots_feeds_compaction_and_namespaces_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("records.db");
    let first;
    let second;
    {
        let db = Database::open(&path, until()).unwrap();
        let records = db.namespace("server").unwrap();
        first = records
            .commit(
                Tx {
                    conds: vec![absent(b"a")],
                    writes: vec![put(b"a", b"one"), put(&[255, 1], b"binary")],
                },
                until(),
            )
            .unwrap();
        assert!(matches!(
            records.commit(
                Tx {
                    conds: vec![absent(b"a")],
                    writes: vec![put(b"a", b"wrong")]
                },
                until()
            ),
            Err(Refusal::Conflict { .. })
        ));
        assert_eq!(records.head(until()).unwrap(), first);
        second = records
            .commit(
                Tx {
                    conds: vec![Cond::At {
                        family: "items".into(),
                        key: b"a".into(),
                        rev: first,
                    }],
                    writes: vec![put(b"a", b"middle"), put(b"a", b"two")],
                },
                until(),
            )
            .unwrap();
        assert_eq!(
            records
                .get("items", b"a", Consistency::Snapshot(first), until())
                .unwrap()
                .unwrap()
                .value,
            b"one"
        );
        let changes = records.changes("items", first, 100, until()).unwrap();
        assert_eq!(changes.changes.len(), 1);
        assert_eq!(changes.changes[0].value.as_deref(), Some(b"two".as_slice()));
        let page = records
            .scan(
                "items",
                Scan {
                    prefix: vec![255],
                    after: None,
                    limit: 10,
                    at: Consistency::Linearizable,
                },
                until(),
            )
            .unwrap();
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].key, vec![255, 1]);
        assert!(
            db.namespace("host")
                .unwrap()
                .get("items", b"a", Consistency::Linearizable, until())
                .unwrap()
                .is_none()
        );
        records.compact(second, until()).unwrap();
        assert!(matches!(
            records.get("items", b"a", Consistency::Snapshot(first), until()),
            Err(Refusal::Compacted { .. })
        ));
    }
    let db = Database::open(&path, until()).unwrap();
    let records = db.namespace("server").unwrap();
    assert_eq!(records.head(until()).unwrap(), second);
    assert_eq!(
        records
            .get("items", b"a", Consistency::Linearizable, until())
            .unwrap()
            .unwrap()
            .value,
        b"two"
    );
    assert!(matches!(
        records.get("items", b"a", Consistency::Snapshot(first), until()),
        Err(Refusal::Compacted { .. })
    ));
    assert_eq!(records.wait("items", first, until()).unwrap(), second);
}

#[test]
fn competing_conditions_commit_once_and_wait_observes_the_durable_change() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("records.db"), until()).unwrap();
    let records = db.namespace("server").unwrap();
    let gate = Arc::new(Barrier::new(3));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let (r, gate) = (records.clone(), gate.clone());
            std::thread::spawn(move || {
                gate.wait();
                r.commit(
                    Tx {
                        conds: vec![absent(b"same")],
                        writes: vec![put(b"same", b"once")],
                    },
                    until(),
                )
            })
        })
        .collect();
    gate.wait();
    let answers: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(answers.iter().filter(|a| a.is_ok()).count(), 1);
    assert_eq!(
        answers
            .iter()
            .filter(|a| matches!(a, Err(Refusal::Conflict { .. })))
            .count(),
        1
    );
    let previous = records.head(until()).unwrap();
    let gate = Arc::new(Barrier::new(2));
    let waiter = {
        let (r, gate) = (records.clone(), gate.clone());
        std::thread::spawn(move || {
            gate.wait();
            r.wait("items", previous, until())
        })
    };
    gate.wait();
    let next = records
        .commit(
            Tx {
                conds: vec![],
                writes: vec![put(b"next", b"event")],
            },
            until(),
        )
        .unwrap();
    assert_eq!(waiter.join().unwrap().unwrap(), next);
}

#[test]
fn sql_and_records_share_one_transaction_and_publish_only_durable_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("one.db");
    let db = Database::open(&path, until()).unwrap();
    let records = db.namespace("server").unwrap();
    let at = db
        .call(until(), |connection| {
            connection
                .execute_batch("CREATE TABLE paired(n INTEGER); INSERT INTO paired VALUES (1)")?;
            let at = connection.records_commit(
                "server",
                Tx {
                    conds: vec![absent(b"paired")],
                    writes: vec![put(b"paired", b"committed")],
                },
            )?;
            assert_eq!(
                connection
                    .records_get("server", "items", b"paired")?
                    .unwrap()
                    .value,
                b"committed"
            );
            Ok(at)
        })
        .unwrap();
    assert_eq!(records.try_wait("items", 0).unwrap(), Some(at));
    let refused: Result<(), Refusal> = db.call(until(), |connection| {
        connection.execute("UPDATE paired SET n = 2", [])?;
        connection.records_commit(
            "server",
            Tx {
                conds: vec![absent(b"paired")],
                writes: vec![put(b"paired", b"must-roll-back")],
            },
        )?;
        Ok(())
    });
    assert!(matches!(refused, Err(Refusal::Conflict { .. })));
    assert_eq!(records.head(until()).unwrap(), at);
    assert_eq!(
        db.call(until(), |c| Ok(c.query_row(
            "SELECT n FROM paired",
            [],
            |row| row.get::<_, i64>(0usize)
        )?))
        .unwrap(),
        1
    );
    drop(records);
    drop(db);
    let db = Database::open(&path, until()).unwrap();
    assert_eq!(
        db.namespace("server")
            .unwrap()
            .get("items", b"paired", Consistency::Linearizable, until())
            .unwrap()
            .unwrap()
            .value,
        b"committed"
    );
}
