//! AC-T1.1.1, AC-T1.8.6: composite-key mutations preserve SQLite's
//! per-statement conflict policy when statement journals are enabled. Product
//! sessions deliberately poison their enclosing transaction instead of keeping
//! statement journals; their separate public-API tests cover that contract.
use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, LimboError, MemoryIO, OpenOptions,
    SqliteDialect, StepResult, IO,
};
use amphora_engine_parser::ast::ResolveType;
use std::sync::Arc;

fn sql(
    io: &Arc<MemoryIO>,
    connection: &Arc<Connection>,
    sql: &str,
) -> Result<Vec<Vec<String>>, LimboError> {
    let mut statement = connection.prepare(sql)?;
    // The product resets checked-out statements before binding, including new
    // statements. Exercise that same lifecycle independently of its no-journal policy.
    statement.reset()?;
    let mut rows = Vec::new();
    loop {
        match statement.step()? {
            StepResult::Row => rows.push(
                statement
                    .row()
                    .unwrap()
                    .get_values()
                    .map(ToString::to_string)
                    .collect(),
            ),
            StepResult::Done => return Ok(rows),
            StepResult::IO | StepResult::Yield => io.step()?,
            other => panic!("unexpected in-memory step: {other:?}"),
        }
    }
}

#[test]
fn upsert_update_aborts_without_changing_outer_fail_policy() {
    let io = Arc::new(MemoryIO::new());
    let database = Database::open(
        io.clone(),
        "without_rowid_dml.db",
        OpenOptions::new(Arc::new(SqliteDialect))
            .db_opts(DatabaseOpts::new().with_without_rowid(true)),
    )
    .unwrap();
    let connection = database.connect(ConnectOpts::new()).unwrap();
    for statement in [
        "CREATE TABLE t(k TEXT PRIMARY KEY,u TEXT UNIQUE,n TEXT NOT NULL CHECK(length(n)>0)) WITHOUT ROWID",
        "INSERT INTO t VALUES('conflict','u1','kept'),('other','u2','other')",
        "CREATE TABLE marker(value INTEGER)",
        "INSERT INTO marker VALUES(0)",
        "BEGIN IMMEDIATE",
    ] { sql(&io, &connection, statement).unwrap(); }

    for assignment in ["n=NULL", "u='u2'", "n=''"] {
        let statement = format!("INSERT OR FAIL INTO t VALUES('fresh','uf','new'),('conflict','u3','new') ON CONFLICT(k) DO UPDATE SET {assignment}");
        let error = sql(&io, &connection, &statement).unwrap_err();
        assert!(
            matches!(error, LimboError::Raise(ResolveType::Abort, _)),
            "{assignment}: {error:?}"
        );
        assert_eq!(
            sql(&io, &connection, "SELECT k,u,n FROM t ORDER BY k").unwrap(),
            vec![vec!["conflict", "u1", "kept"], vec!["other", "u2", "other"]]
        );
        sql(&io, &connection, "UPDATE marker SET value=value+1").unwrap();
    }
    // A conflict outside DO UPDATE retains the outer FAIL policy and its
    // earlier candidate. Setting the whole program to ABORT would lose this row.
    let error = sql(&io, &connection, "INSERT OR FAIL INTO t VALUES('retained','ur','new'),('collision','u2','new') ON CONFLICT(k) DO UPDATE SET n=NULL").unwrap_err();
    assert!(
        matches!(
            error,
            LimboError::Constraint(_) | LimboError::Raise(ResolveType::Fail, _)
        ),
        "{error:?}"
    );
    sql(&io, &connection, "COMMIT").unwrap();
    assert_eq!(
        sql(&io, &connection, "SELECT k FROM t ORDER BY k").unwrap(),
        vec![vec!["conflict"], vec!["other"], vec!["retained"]]
    );
    assert_eq!(
        sql(&io, &connection, "SELECT value FROM marker").unwrap(),
        vec![vec!["3"]]
    );
}

#[test]
fn replace_refuses_unsupported_delete_trigger_and_generated_image_before_writing() {
    let io = Arc::new(MemoryIO::new());
    let database = Database::open(
        io.clone(),
        "without_rowid_replace_refusal.db",
        OpenOptions::new(Arc::new(SqliteDialect)).db_opts(
            DatabaseOpts::new()
                .with_without_rowid(true)
                .with_generated_columns(true),
        ),
    )
    .unwrap();
    let connection = database.connect(ConnectOpts::new()).unwrap();
    for statement in [
        "CREATE TABLE triggered(k TEXT PRIMARY KEY,v INTEGER) WITHOUT ROWID",
        "CREATE TABLE audit(value INTEGER)",
        "INSERT INTO triggered VALUES('a',1)",
        "CREATE TRIGGER deleted AFTER DELETE ON triggered BEGIN INSERT INTO audit VALUES(1); END",
        "CREATE TABLE generated(k TEXT PRIMARY KEY,v INTEGER,computed INTEGER GENERATED ALWAYS AS(v+1) VIRTUAL) WITHOUT ROWID",
    ] {
        sql(&io, &connection, statement).unwrap();
    }
    for table in ["triggered", "generated"] {
        let error = sql(
            &io,
            &connection,
            &format!("INSERT OR REPLACE INTO {table}(k,v) VALUES('a',2)"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("WITHOUT ROWID"), "{error:?}");
        assert_eq!(
            sql(&io, &connection, &format!("SELECT k,v FROM {table}")).unwrap(),
            if table == "triggered" {
                vec![vec!["a", "1"]]
            } else {
                Vec::<Vec<&str>>::new()
            }
        );
    }
    assert!(sql(&io, &connection, "SELECT * FROM audit")
        .unwrap()
        .is_empty());
}
