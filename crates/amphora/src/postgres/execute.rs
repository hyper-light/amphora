//! A session's statements on an engine connection of the PostgreSQL dialect (docs/design/postgres.md,
//! section 11's unit 9): a Query's text is parsed, analyzed and lowered into the engine's statement,
//! which runs there, and its answer is written as PostgreSQL 19 writes it, RowDescription and DataRow
//! in text, CommandComplete, under 19's transaction rules.

use std::num::NonZero;
use std::sync::Arc;

use amphora_engine::dialect::postgres::{from_engine, to_engine};
use amphora_engine::{Connection, LimboError, Statement, Value};
use amphora_pg_analysis::datetime::TimeZone;
use amphora_pg_analysis::stmt::Command;
use amphora_pg_analysis::{Column, Error, Field, Relations, analyze, lower, output};
use amphora_pg_catalog::data::{PG_TYPE, type_oid};
use amphora_pg_catalog::{Datum, SearchPath};
use amphora_pg_parser::gram;
use amphora_pg_parser::tree::{Node, NodeId, TransactionStmtKind, Tree};

use super::session::{Statements, TxStatus};
use super::wire::{self, PgError, SqlState};

/// Microseconds from PostgreSQL's epoch, 2000-01-01 UTC, to Unix's (`src/include/datatype/timestamp.h`,
/// `POSTGRES_EPOCH_JDATE - UNIX_EPOCH_JDATE` days).
const UNIX_EPOCH_MICROS: i64 = -946_684_800_000_000;

/// The session's `search_path` (its role's schema, then `public`), its catalogs read from the engine's
/// schema through `pg_class` and `pg_attribute` (section 9).
struct Catalog {
    conn: Arc<Connection>,
}

impl Catalog {
    fn rows(&self, sql: &str, binds: &[Value]) -> Option<Vec<Vec<Value>>> {
        let mut statement = self.conn.prepare(sql).ok()?;
        for (at, value) in binds.iter().enumerate() {
            statement
                .bind_at(NonZero::new(at.checked_add(1)?)?, value.clone())
                .ok()?;
        }
        statement.run_collect_rows().ok()
    }

    fn first(&self, sql: &str, binds: &[Value]) -> Option<Vec<Value>> {
        self.rows(sql, binds)?.into_iter().next()
    }
}

fn oid_of(value: Option<&Value>) -> Option<u32> {
    let Some(Value::Numeric(amphora_engine::Numeric::Integer(i))) = value else {
        return None;
    };
    u32::try_from(*i).ok()
}

fn text_of(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::Text(text)) => Some(text.as_str().to_owned()),
        _ => None,
    }
}

fn int(value: u32) -> Value {
    Value::Numeric(amphora_engine::Numeric::Integer(i64::from(value)))
}

impl Relations for Catalog {
    fn namespace(&self, name: &str) -> Option<u32> {
        let found = self.first(
            "SELECT oid FROM pg_namespace WHERE nspname = ?1",
            &[Value::build_text(name.to_owned())],
        )?;
        oid_of(found.first())
    }

    fn relation(&self, namespace: u32, name: &str) -> Option<u32> {
        let found = self.first(
            "SELECT oid FROM pg_class WHERE relname = ?1 AND relnamespace = ?2",
            &[Value::build_text(name.to_owned()), int(namespace)],
        )?;
        oid_of(found.first())
    }

    fn name(&self, oid: u32) -> Option<(u32, String)> {
        let found = self.first(
            "SELECT relnamespace, relname FROM pg_class WHERE oid = ?1",
            &[int(oid)],
        )?;
        Some((oid_of(found.first())?, text_of(found.get(1))?))
    }

    fn namespace_name(&self, oid: u32) -> Option<String> {
        let found = self.first(
            "SELECT nspname FROM pg_namespace WHERE oid = ?1",
            &[int(oid)],
        )?;
        text_of(found.first())
    }

    fn columns(&self, oid: u32) -> Vec<Column> {
        let rows = self
            .rows(
                "SELECT attname, atttypid, atttypmod FROM pg_attribute WHERE attrelid = ?1 AND attnum > 0 AND attisdropped = 0 ORDER BY attnum",
                &[int(oid)],
            )
            .unwrap_or_default();
        let mut columns = Vec::with_capacity(rows.len());
        for row in rows {
            let typmod = match row.get(2) {
                Some(Value::Numeric(amphora_engine::Numeric::Integer(i))) => {
                    i32::try_from(*i).unwrap_or(-1)
                }
                _ => -1,
            };
            if let (Some(name), Some(type_oid)) = (text_of(row.first()), oid_of(row.get(1))) {
                columns.push(Column {
                    name,
                    type_oid,
                    typmod,
                });
            }
        }
        columns
    }
}

/// A session's statements run on `conn`, in the search path of `role`, under `time_zone`. `clock` gives
/// the statement's start, in microseconds from 2000-01-01 UTC.
pub struct Executor {
    conn: Arc<Connection>,
    catalog: Catalog,
    path: SearchPath,
    time_zone: TimeZone,
    clock: Box<dyn Fn() -> i64>,
    status: TxStatus,
    /// The open transaction's start (`GetCurrentTransactionStartTimestamp`).
    began: i64,
    /// A transaction a Query of several statements began for itself.
    implicit: bool,
}

/// `now` as [`Executor`]'s clock gives it, from the system's.
#[must_use]
pub fn system_clock() -> i64 {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_micros());
    i64::try_from(unix)
        .unwrap_or(i64::MAX)
        .saturating_add(UNIX_EPOCH_MICROS)
}

impl Executor {
    #[must_use]
    pub fn new(
        conn: Arc<Connection>,
        role: &str,
        time_zone: TimeZone,
        clock: Box<dyn Fn() -> i64>,
    ) -> Self {
        let catalog = Catalog {
            conn: Arc::clone(&conn),
        };
        let path = SearchPath::new(&["$user", "public"], role, None, |name| {
            catalog.namespace(name)
        });
        Self {
            conn,
            catalog,
            path,
            time_zone,
            clock,
            status: TxStatus::Idle,
            began: 0,
            implicit: false,
        }
    }

    fn statement(
        &mut self,
        tree: &Tree,
        id: NodeId,
        several: bool,
        out: &mut Vec<u8>,
    ) -> Result<(), PgError> {
        let Node::RawStmt {
            stmt: Some(stmt), ..
        } = tree
            .get(id)
            .map_err(|_| internal("a parse tree node is missing"))?
        else {
            return Err(internal("an empty statement"));
        };
        if let Node::TransactionStmt {
            kind,
            options,
            chain,
            savepoint_name: None,
            gid: None,
            ..
        } = tree
            .get(*stmt)
            .map_err(|_| internal("a parse tree node is missing"))?
        {
            if several {
                return Err(unsupported(
                    "transaction control in a Query of several statements",
                ));
            }
            if !options.is_empty() || *chain {
                return Err(unsupported("this transaction command"));
            }
            return self.transaction(*kind, out);
        }
        if self.status == TxStatus::Failed {
            return Err(PgError::error(
                SqlState::new("25P02"),
                "current transaction is aborted, commands ignored until end of transaction block",
            ));
        }
        let now = if self.status == TxStatus::InTransaction || self.implicit {
            self.began
        } else {
            (self.clock)()
        };
        let lowered = {
            let session = amphora_pg_analysis::Session {
                path: &self.path,
                relations: &self.catalog,
                time_zone: &self.time_zone,
                now: Some(now),
            };
            let query = analyze(tree, id, &session, &[]).map_err(refusal)?;
            if !query.params.is_empty() {
                return Err(PgError::error(
                    SqlState::new("42P02"),
                    "there is no parameter $1",
                ));
            }
            let lowered = lower(&query, &session).map_err(refusal)?;
            (query.command, lowered)
        };
        let (command, lowered) = lowered;
        self.run(command, &lowered, out)
    }

    /// Runs a lowered statement and writes its answer.
    fn run(
        &self,
        command: Command,
        lowered: &amphora_pg_analysis::Lowered,
        out: &mut Vec<u8>,
    ) -> Result<(), PgError> {
        let mut statement = self.prepared(lowered)?;
        match command {
            Command::Select => {
                row_description(out, &lowered.fields)?;
                let rows = statement.run_collect_rows().map_err(engine)?;
                let mut count = 0usize;
                for row in &rows {
                    let values = self.row(&lowered.fields, row)?;
                    data_row(out, &values)?;
                    count = count.saturating_add(1);
                }
                complete(out, &format!("SELECT {count}"))
            }
            Command::Update => {
                statement.run_collect_rows().map_err(engine)?;
                complete(out, &format!("UPDATE {}", statement.n_change()))
            }
            Command::Insert => {
                statement.run_collect_rows().map_err(engine)?;
                complete(out, &format!("INSERT 0 {}", statement.n_change()))
            }
        }
    }

    fn prepared(&self, lowered: &amphora_pg_analysis::Lowered) -> Result<Statement, PgError> {
        let mut statement = self.conn.prepare(&lowered.sql).map_err(engine)?;
        for (at, (type_oid, value)) in lowered.binds.iter().enumerate() {
            let value = match value {
                None => Value::Null,
                Some(value) => to_engine(*type_oid, value).map_err(engine)?,
            };
            let index = at
                .checked_add(1)
                .and_then(NonZero::new)
                .ok_or_else(|| internal("a statement has too many parameters"))?;
            statement.bind_at(index, value).map_err(engine)?;
        }
        Ok(statement)
    }

    /// A row's columns in text, as 19's output functions write them (`None` for NULL).
    fn row(&self, fields: &[Field], row: &[Value]) -> Result<Vec<Option<String>>, PgError> {
        let session = amphora_pg_analysis::Session {
            path: &self.path,
            relations: &self.catalog,
            time_zone: &self.time_zone,
            now: None,
        };
        let mut texts = Vec::with_capacity(fields.len());
        for (field, stored) in fields.iter().zip(row) {
            let value = from_engine(field.type_oid, stored).map_err(engine)?;
            let text = match value {
                None => None,
                Some(value) => {
                    let mut text = output(field.type_oid, &value, &session).map_err(refusal)?;
                    if field.type_oid == type_oid::BPCHAR {
                        pad(&mut text, field.typmod);
                    }
                    Some(text)
                }
            };
            texts.push(text);
        }
        Ok(texts)
    }

    fn transaction(&mut self, kind: TransactionStmtKind, out: &mut Vec<u8>) -> Result<(), PgError> {
        match (kind, self.status) {
            (TransactionStmtKind::Begin, TxStatus::Idle) => {
                self.begin()?;
                self.status = TxStatus::InTransaction;
                complete(out, "BEGIN")
            }
            (TransactionStmtKind::Begin, TxStatus::InTransaction) => {
                wire::notice_response(
                    out,
                    "WARNING",
                    "25001",
                    "there is already a transaction in progress",
                )?;
                complete(out, "BEGIN")
            }
            (TransactionStmtKind::Begin, TxStatus::Failed) => Err(PgError::error(
                SqlState::new("25P02"),
                "current transaction is aborted, commands ignored until end of transaction block",
            )),
            (TransactionStmtKind::Commit, TxStatus::InTransaction) => {
                let result = self.conn.execute("COMMIT").map_err(engine);
                self.status = TxStatus::Idle;
                if let Err(error) = result {
                    self.undo();
                    return Err(error);
                }
                complete(out, "COMMIT")
            }
            (TransactionStmtKind::Commit, TxStatus::Failed) => {
                self.status = TxStatus::Idle;
                self.undo();
                complete(out, "ROLLBACK")
            }
            (TransactionStmtKind::Commit, TxStatus::Idle) => {
                wire::notice_response(
                    out,
                    "WARNING",
                    "25P01",
                    "there is no transaction in progress",
                )?;
                complete(out, "COMMIT")
            }
        }
    }

    fn begin(&mut self) -> Result<(), PgError> {
        self.began = (self.clock)();
        self.conn.execute("BEGIN").map_err(engine)
    }

    /// Ends the engine's transaction without its writes, if one is open.
    fn undo(&self) {
        if !self.conn.get_auto_commit() {
            // A refused ROLLBACK has nothing more to undo: the engine's own refusal is logged.
            if let Err(error) = self.conn.execute("ROLLBACK") {
                tracing::warn!(target: "amphora.postgres.execute", event = "rollback_failed", %error);
            }
        }
    }

    /// Answers with `error`, and ends what it ends: an implicit transaction, or the block's writes.
    fn refuse(&mut self, error: &PgError, out: &mut Vec<u8>) -> Result<(), PgError> {
        if self.implicit {
            self.implicit = false;
            self.undo();
        } else if self.status == TxStatus::InTransaction {
            self.status = TxStatus::Failed;
            // Nothing of the block may stay: the statement's own effects are the engine's to undo at
            // the statement, the block's at END.
        }
        wire::error_response(out, error)
    }
}

impl Statements for Executor {
    fn status(&self) -> TxStatus {
        self.status
    }

    fn simple_query(&mut self, sql: &str, out: &mut Vec<u8>) -> Result<(), PgError> {
        let (tree, statements) = match gram::parse(sql.as_bytes()) {
            Ok(parsed) => parsed,
            Err(refused) => {
                let error = PgError::error(SqlState::new(refused.sqlstate), refused.message);
                return self.refuse(&error, out);
            }
        };
        let several = statements.len() > 1;
        if several && self.status == TxStatus::Idle {
            self.implicit = true;
            if let Err(error) = self.begin() {
                self.implicit = false;
                return self.refuse(&error, out);
            }
        }
        for id in statements {
            if let Err(error) = self.statement(&tree, id, several, out) {
                return self.refuse(&error, out);
            }
        }
        if self.implicit {
            self.implicit = false;
            if let Err(error) = self.conn.execute("COMMIT").map_err(engine) {
                self.undo();
                return self.refuse(&error, out);
            }
        }
        Ok(())
    }
}

/// `bpchar`'s trailing spaces, stripped in storage, put back to `char(n)`'s n characters.
fn pad(text: &mut String, typmod: i32) {
    let Some(width) = usize::try_from(typmod).ok().and_then(|t| t.checked_sub(4)) else {
        return;
    };
    let missing = width.saturating_sub(text.chars().count());
    text.extend(std::iter::repeat_n(' ', missing));
}

fn internal(message: &str) -> PgError {
    PgError::error(SqlState::INTERNAL_ERROR, message)
}

fn unsupported(what: &str) -> PgError {
    refusal(Error::unsupported(what))
}

/// Analysis's refusal, as it is.
fn refusal(error: Error) -> PgError {
    PgError::error(SqlState::new(error.sqlstate), error.message)
}

/// What the engine refused.
fn engine(error: LimboError) -> PgError {
    match error {
        LimboError::Postgres { sqlstate, message } => {
            PgError::error(SqlState::new(sqlstate), message)
        }
        LimboError::Constraint(message) | LimboError::ForeignKeyConstraint(message) => {
            PgError::error(SqlState::new("23000"), message)
        }
        LimboError::Busy
        | LimboError::BusySnapshot
        | LimboError::WriteWriteConflict
        | LimboError::SerializationFailure => PgError::error(
            SqlState::new("40001"),
            "could not serialize access due to concurrent update",
        ),
        error => PgError::error(SqlState::INTERNAL_ERROR, error.to_string()),
    }
}

/// `pg_type.typlen`.
fn typlen(oid: u32) -> i16 {
    let (Some(at), Some(len)) = (PG_TYPE.column("oid"), PG_TYPE.column("typlen")) else {
        return -1;
    };
    PG_TYPE
        .rows
        .iter()
        .find(|row| row.get(at) == Some(&Datum::Oid(oid)))
        .and_then(|row| match row.get(len) {
            Some(Datum::Int2(len)) => Some(*len),
            _ => None,
        })
        .unwrap_or(-1)
}

fn row_description(out: &mut Vec<u8>, fields: &[Field]) -> Result<(), PgError> {
    let count =
        i16::try_from(fields.len()).map_err(|_| internal("a result has too many columns"))?;
    wire::message(out, b'T', |out| {
        out.extend_from_slice(&count.to_be_bytes());
        for field in fields {
            wire::put_cstr(out, field.name.as_bytes());
            out.extend_from_slice(&field.table.to_be_bytes());
            let attnum = i16::try_from(field.attnum).unwrap_or(0);
            out.extend_from_slice(&attnum.to_be_bytes());
            out.extend_from_slice(&field.type_oid.to_be_bytes());
            out.extend_from_slice(&typlen(field.type_oid).to_be_bytes());
            out.extend_from_slice(&field.typmod.to_be_bytes());
            out.extend_from_slice(&0i16.to_be_bytes());
        }
    })
}

fn data_row(out: &mut Vec<u8>, values: &[Option<String>]) -> Result<(), PgError> {
    let count = i16::try_from(values.len()).map_err(|_| internal("a row has too many columns"))?;
    wire::message(out, b'D', |out| {
        out.extend_from_slice(&count.to_be_bytes());
        for value in values {
            match value {
                None => out.extend_from_slice(&(-1i32).to_be_bytes()),
                Some(text) => {
                    let len = i32::try_from(text.len()).unwrap_or(i32::MAX);
                    out.extend_from_slice(&len.to_be_bytes());
                    out.extend_from_slice(text.as_bytes());
                }
            }
        }
    })
}

/// CommandComplete.
fn complete(out: &mut Vec<u8>, tag: &str) -> Result<(), PgError> {
    wire::message(out, b'C', |out| wire::put_cstr(out, tag.as_bytes()))
}
