#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros
)]
//! CP-T1.5 / U05/U06: independent processes share the same writer and Records revisions.
use amphora::{database::Database, transport::Client};
use amphora_api::{self as api, Records};
use std::{
    path::Path,
    process::Command,
    time::{Duration, Instant},
};
fn until() -> Instant {
    Instant::now() + Duration::from_secs(10)
}
fn child(path: &Path, mode: &str) -> std::process::ExitStatus {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "transport_child", "--nocapture"])
        .env("AMPHORA_TRANSPORT_TEST_PATH", path)
        .env("AMPHORA_TRANSPORT_TEST_MODE", mode)
        .status()
        .unwrap()
}
#[test]
fn transport_child() {
    let Some(path) = std::env::var_os("AMPHORA_TRANSPORT_TEST_PATH") else {
        return;
    };
    let path = Path::new(&path);
    match std::env::var("AMPHORA_TRANSPORT_TEST_MODE")
        .unwrap()
        .as_str()
    {
        "owner" => {
            let database = Database::open(path, until()).unwrap();
            let n = database
                .call(until(), |db| {
                    Ok(db.query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0))?)
                })
                .unwrap();
            assert_eq!(n, 1);
        }
        "cas" => {
            let client = Client::connect(path, until()).unwrap();
            let ns = client.namespace("ledger").unwrap();
            let tx = api::Tx {
                conds: vec![api::Cond::Absent {
                    family: "work".into(),
                    key: b"one".to_vec(),
                }],
                writes: vec![api::Write::Put {
                    family: "work".into(),
                    key: b"one".to_vec(),
                    value: b"claimed".to_vec(),
                }],
            };
            assert!(matches!(
                ns.commit(tx, until()),
                Err(api::Refusal::Conflict { .. })
            ));
            client
                .call(until(), |db| {
                    let n: i64 = db.query_row("SELECT count(*) FROM items", [], |r| r.get(0))?;
                    assert_eq!(n, 1);
                    db.execute("INSERT INTO items VALUES (?1)", [n + 1])?;
                    let row = db.records_get("ledger", "work", b"one")?.unwrap();
                    assert_eq!(row.value, b"claimed");
                    db.records_commit(
                        "ledger",
                        api::Tx {
                            conds: vec![api::Cond::At {
                                family: "work".into(),
                                key: b"one".to_vec(),
                                rev: row.rev,
                            }],
                            writes: vec![api::Write::Put {
                                family: "work".into(),
                                key: b"one".to_vec(),
                                value: b"child_claimed".to_vec(),
                            }],
                        },
                    )?;
                    Ok(())
                })
                .unwrap();
        }
        "disconnect" => {
            let client = Client::connect(path, until()).unwrap();
            let _: Result<(), _> = client.call(until(), |db| {
                db.execute("INSERT INTO items VALUES (99)", [])?;
                db.records_commit(
                    "ledger",
                    api::Tx {
                        conds: vec![],
                        writes: vec![api::Write::Put {
                            family: "work".into(),
                            key: b"disconnect".to_vec(),
                            value: b"discarded".to_vec(),
                        }],
                    },
                )?;
                std::process::exit(0)
            });
            panic!("disconnect child returned")
        }
        _ => panic!("unknown child mode"),
    }
}
#[test]
fn independent_processes_share_cas_and_adaptive_sql_and_abort_disconnect() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let database = Database::open(&path, until()).unwrap();
    let client = Client::connect(&path, until()).unwrap();
    client
        .call(until(), |db| {
            db.execute_batch("CREATE TABLE items(value INTEGER); INSERT INTO items VALUES(1)")?;
            Ok(())
        })
        .unwrap();
    let ns = client.namespace("ledger").unwrap();
    ns.commit(
        api::Tx {
            conds: vec![],
            writes: vec![api::Write::Put {
                family: "work".into(),
                key: b"one".to_vec(),
                value: b"claimed".to_vec(),
            }],
        },
        until(),
    )
    .unwrap();
    assert_eq!(ns.try_wait("work", 0, until()).unwrap(), Some(1));
    let restored = client.namespace("restored").unwrap();
    restored
        .import_page(
            api::import::ImportPage {
                source: "fixed-snapshot".into(),
                expected_cursor: vec![],
                next_cursor: vec![1],
                versions: vec![api::import::ImportVersion {
                    family: "work".into(),
                    key: b"old".to_vec(),
                    rev: 7,
                    seq: 0,
                    value: Some(b"retained".to_vec()),
                }],
                state: api::import::ImportState {
                    head: 7,
                    horizon: 0,
                    feed: 0,
                    newest: vec![("work".into(), 7)],
                },
                done: true,
            },
            until(),
        )
        .unwrap();
    assert!(restored.import_state(until()).unwrap().unwrap().done);
    assert_eq!(
        restored
            .get("work", b"old", api::Consistency::Linearizable, until())
            .unwrap()
            .unwrap()
            .rev,
        7
    );
    assert!(child(&path, "owner").success());
    assert!(child(&path, "cas").success());
    let result: Result<(), _> = client.call(until(), |db| {
        let n: i64 = db.query_row("SELECT count(*) FROM items", [], |r| r.get(0))?;
        assert_eq!(n, 2);
        db.execute("INSERT INTO items VALUES(3)", [])?;
        let row = db.records_get("ledger", "work", b"one")?.unwrap();
        assert_eq!(row.value, b"child_claimed");
        db.records_commit(
            "ledger",
            api::Tx {
                conds: vec![api::Cond::At {
                    family: "work".into(),
                    key: b"one".to_vec(),
                    rev: row.rev,
                }],
                writes: vec![api::Write::Put {
                    family: "work".into(),
                    key: b"one".to_vec(),
                    value: b"discarded".to_vec(),
                }],
            },
        )?;
        anyhow::bail!("application rollback")
    });
    assert!(result.is_err());
    assert!(child(&path, "disconnect").success());
    // The next writer operation waits behind disconnect cleanup, without a guessed sleep.
    let n = client
        .call(until(), |db| {
            db.query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0))
                .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(n, 2);
    assert_eq!(
        ns.get("work", b"one", api::Consistency::Linearizable, until())
            .unwrap()
            .unwrap()
            .value,
        b"child_claimed"
    );
    assert!(
        ns.get(
            "work",
            b"disconnect",
            api::Consistency::Linearizable,
            until()
        )
        .unwrap()
        .is_none()
    );
    let conflict: Result<(), _> = client.call(until(), |db| {
        db.execute("INSERT INTO items VALUES(100)", [])?;
        db.records_commit(
            "ledger",
            api::Tx {
                conds: vec![api::Cond::Absent {
                    family: "work".into(),
                    key: b"one".to_vec(),
                }],
                writes: vec![],
            },
        )?;
        Ok(())
    });
    assert!(matches!(
        conflict,
        Err(amphora::transport::Error::Refused(
            api::Refusal::Conflict { .. }
        ))
    ));
    let clone = database.clone();
    drop(database);
    assert_eq!(
        client.namespace("ledger").unwrap().head(until()).unwrap(),
        2
    );
    drop(clone);
}
