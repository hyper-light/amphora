//! A session's statements on an engine connection of the PostgreSQL dialect (docs/design/postgres.md,
//! section 11's unit 9): a Query's text is parsed, analyzed and lowered into the engine's statement,
//! which runs there, and its answer is written as PostgreSQL 19 writes it, RowDescription and DataRow
//! in text, CommandComplete, under 19's transaction rules.

use std::num::NonZero;
use std::sync::Arc;

use amphora_engine::dialect::postgres::{from_engine, to_engine};
use amphora_engine::storage::versioned::memory::Source;
use amphora_engine::{
    ConnectOpts, Connection, LimboError, PlatformIO, PostgresDialect, Statement, Value,
};
use amphora_pg_analysis::datetime::TimeZone;
use amphora_pg_analysis::stmt::Command;
use amphora_pg_analysis::{Column, Error, Field, Relations, analyze, lower, output, utility};
use amphora_pg_catalog::data::{PG_TYPE, type_oid};
use amphora_pg_catalog::{Datum, SearchPath};
use amphora_pg_parser::gram;
use amphora_pg_parser::tree::{Node, NodeId, TransactionStmtKind, Tree};

use super::session::{Statements, TxStatus};
use super::wire::{self, PgError, Severity, SqlState};

/// Microseconds from PostgreSQL's epoch, 2000-01-01 UTC, to Unix's (`src/include/datatype/timestamp.h`,
/// `POSTGRES_EPOCH_JDATE - UNIX_EPOCH_JDATE` days).
const UNIX_EPOCH_MICROS: i64 = -946_684_800_000_000;

/// A database of the PostgreSQL dialect on the engine's versioned storage, where a transaction's writes
/// are row versions of its own: one open across its client's round trips holds no writer (section 2).
pub struct Database {
    engine: Arc<amphora_engine::Database>,
    memory: Arc<dyn Source>,
}

impl Database {
    /// The database at `path`, made as needed, its storage's memory taken from `memory`.
    pub fn open(path: &str, memory: Arc<dyn Source>) -> Result<Self, LimboError> {
        let engine = amphora_engine::Database::open_file(
            Arc::new(PlatformIO::new()?),
            path,
            Arc::new(PostgresDialect::new()),
        )?;
        let database = Self { engine, memory };
        // The connection that makes the storage: one it refuses is refused here, not at a login.
        database.connect()?;
        Ok(database)
    }

    fn connect(&self) -> Result<Arc<Connection>, LimboError> {
        self.engine.connect(
            ConnectOpts::new()
                .with_versioned_storage()
                .with_memory(Arc::clone(&self.memory)),
        )
    }

    /// A session of `role`: an executor on a connection of its own. A refusal ends the session, as
    /// PostgreSQL's does one it can't open a database for.
    pub fn session(&self, role: &str, time_zone: &str) -> Result<Executor, PgError> {
        let fatal = |error: PgError| PgError {
            severity: Severity::Fatal,
            ..error
        };
        let zone = amphora_pg_analysis::tz::time_zone(time_zone)
            .map_err(|error| fatal(refusal(error, "")))?;
        let conn = self.connect().map_err(|error| fatal(engine(error)))?;
        Ok(Executor::new(conn, role, zone, Box::new(system_clock)))
    }
}

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
                "SELECT attname, atttypid, atttypmod, attnotnull FROM pg_attribute WHERE attrelid = ?1 AND attnum > 0 AND attisdropped = 0 ORDER BY attnum",
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
                    not_null: oid_of(row.get(3)) == Some(1),
                });
            }
        }
        columns
    }
}

/// The transaction block a session is in between two statements, as far as xact.c's `TBlockState` tells
/// them apart (REL_19_0 src/backend/access/transam/xact.c:141-165).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Block {
    /// Each statement is its own transaction.
    None,
    /// A Query of several statements is one transaction (`TBLOCK_IMPLICIT_INPROGRESS`): it ends with the
    /// Query, unless BEGIN makes it the client's.
    Implicit,
    /// The client's, from BEGIN.
    Explicit,
    /// The client's, refused (`25P02`) but for what ends it.
    Failed,
}

/// A session's statements run on `conn`, in the search path of `role`, under `time_zone`. `clock` gives
/// the statement's start, in microseconds from 2000-01-01 UTC.
pub struct Executor {
    conn: Arc<Connection>,
    catalog: Catalog,
    path: SearchPath,
    time_zone: TimeZone,
    clock: Box<dyn Fn() -> i64>,
    block: Block,
    /// The open transaction's start (`GetCurrentTransactionStartTimestamp`).
    began: i64,
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
            block: Block::None,
            began: 0,
        }
    }

    fn statement(
        &mut self,
        sql: &str,
        tree: &Tree,
        id: NodeId,
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
            if !options.is_empty() || *chain {
                return Err(unsupported("this transaction command"));
            }
            return self.transaction(*kind, out);
        }
        if self.block == Block::Failed {
            return Err(aborted());
        }
        let now = if self.block == Block::None {
            (self.clock)()
        } else {
            self.began
        };
        let lowered = {
            let session = amphora_pg_analysis::Session {
                path: &self.path,
                relations: &self.catalog,
                time_zone: &self.time_zone,
                now: Some(now),
            };
            if let Some(utility) = utility(tree, id, &session).map_err(|e| refusal(e, sql))? {
                self.conn.execute(&utility.sql).map_err(engine)?;
                return complete(out, utility.tag);
            }
            let query = analyze(tree, id, &session, &[]).map_err(|e| refusal(e, sql))?;
            if !query.params.is_empty() {
                return Err(PgError::error(
                    SqlState::new("42P02"),
                    "there is no parameter $1",
                ));
            }
            let lowered = lower(&query, &session).map_err(|e| refusal(e, sql))?;
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
                    let mut text =
                        output(field.type_oid, &value, &session).map_err(|e| refusal(e, ""))?;
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

    /// BEGIN, COMMIT and ROLLBACK, each in each block (xact.c's `BeginTransactionBlock`,
    /// `EndTransactionBlock` and `UserAbortTransactionBlock`).
    fn transaction(&mut self, kind: TransactionStmtKind, out: &mut Vec<u8>) -> Result<(), PgError> {
        match (kind, self.block) {
            (TransactionStmtKind::Begin, Block::None) => {
                self.begin()?;
                self.block = Block::Explicit;
                complete(out, "BEGIN")
            }
            (TransactionStmtKind::Begin, Block::Implicit) => {
                self.block = Block::Explicit;
                complete(out, "BEGIN")
            }
            (TransactionStmtKind::Begin, Block::Explicit) => {
                let text = "there is already a transaction in progress";
                wire::notice_response(out, "WARNING", "25001", text)?;
                complete(out, "BEGIN")
            }
            (TransactionStmtKind::Begin, Block::Failed) => Err(aborted()),
            (TransactionStmtKind::Commit, Block::Explicit) => self.commit(out),
            (TransactionStmtKind::Commit, Block::Implicit) => {
                no_transaction(out)?;
                self.commit(out)
            }
            (TransactionStmtKind::Commit, Block::None) => {
                no_transaction(out)?;
                complete(out, "COMMIT")
            }
            (TransactionStmtKind::Rollback, Block::None | Block::Implicit) => {
                no_transaction(out)?;
                self.end();
                complete(out, "ROLLBACK")
            }
            (TransactionStmtKind::Commit, Block::Failed)
            | (TransactionStmtKind::Rollback, Block::Explicit | Block::Failed) => {
                self.end();
                complete(out, "ROLLBACK")
            }
        }
    }

    /// Commits the block's transaction: one the engine refuses ends without its writes.
    fn commit(&mut self, out: &mut Vec<u8>) -> Result<(), PgError> {
        let result = self.conn.execute("COMMIT").map_err(engine);
        self.end();
        result?;
        complete(out, "COMMIT")
    }

    fn begin(&mut self) -> Result<(), PgError> {
        self.began = (self.clock)();
        self.conn.execute("BEGIN").map_err(engine)
    }

    /// Leaves the block, without the writes of the engine's transaction if one is open.
    fn end(&mut self) {
        self.block = Block::None;
        if !self.conn.get_auto_commit() {
            // A refused ROLLBACK has nothing more to undo: the engine's own refusal is logged.
            if let Err(error) = self.conn.execute("ROLLBACK") {
                tracing::warn!(target: "amphora.postgres.execute", event = "rollback_failed", %error);
            }
        }
    }

    /// Answers with `error`, and ends what it ends: an implicit transaction, or the block but for END.
    /// The statement's own writes are the engine's to undo, the block's are END's.
    fn refuse(&mut self, error: &PgError, out: &mut Vec<u8>) -> Result<(), PgError> {
        match self.block {
            Block::None | Block::Implicit => self.end(),
            Block::Explicit | Block::Failed => self.block = Block::Failed,
        }
        wire::error_response(out, error)
    }

    /// A Query's statements in order (`exec_simple_query`, REL_19_0 src/backend/tcop/postgres.c): several
    /// run in an implicit block, begun again after one that ends it.
    fn statements(
        &mut self,
        sql: &str,
        tree: &Tree,
        statements: &[NodeId],
        out: &mut Vec<u8>,
    ) -> Result<(), PgError> {
        for id in statements {
            if statements.len() > 1 && self.block == Block::None {
                self.begin()?;
                self.block = Block::Implicit;
            }
            self.statement(sql, tree, *id, out)?;
        }
        if self.block == Block::Implicit {
            let result = self.conn.execute("COMMIT").map_err(engine);
            self.end();
            result?;
        }
        Ok(())
    }
}

/// A client that goes away mid-transaction leaves none of its writes (`AbortOutOfAnyTransaction` as a
/// backend exits, REL_19_0 src/backend/access/transam/xact.c).
impl Drop for Executor {
    fn drop(&mut self) {
        self.end();
    }
}

impl Statements for Executor {
    fn status(&self) -> TxStatus {
        match self.block {
            Block::None | Block::Implicit => TxStatus::Idle,
            Block::Explicit => TxStatus::InTransaction,
            Block::Failed => TxStatus::Failed,
        }
    }

    fn simple_query(&mut self, sql: &str, out: &mut Vec<u8>) -> Result<(), PgError> {
        let ran = gram::parse(sql.as_bytes())
            .map_err(|refused| {
                PgError::error(SqlState::new(refused.sqlstate), refused.message)
                    .with_position((refused.cursor > 0).then_some(refused.cursor))
            })
            .and_then(|(tree, statements)| self.statements(sql, &tree, &statements, out));
        match ran {
            Ok(()) => Ok(()),
            Err(error) => self.refuse(&error, out),
        }
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

/// A statement of a block that a refusal ended.
fn aborted() -> PgError {
    PgError::error(
        SqlState::new("25P02"),
        "current transaction is aborted, commands ignored until end of transaction block",
    )
}

/// The warning COMMIT and ROLLBACK give outside the client's block.
fn no_transaction(out: &mut Vec<u8>) -> Result<(), PgError> {
    let text = "there is no transaction in progress";
    wire::notice_response(out, "WARNING", "25P01", text)
}

fn unsupported(what: &str) -> PgError {
    refusal(Error::unsupported(what), "")
}

/// Analysis's refusal of a statement of `sql`: its place there as `parser_errposition` counts it
/// (REL_19_0 src/backend/parser/parse_node.c), in characters from 1.
fn refusal(error: Error, sql: &str) -> PgError {
    let position = usize::try_from(error.location)
        .ok()
        .and_then(|at| sql.get(..at))
        .map(|before| before.chars().count().saturating_add(1));
    PgError::error(SqlState::new(error.sqlstate), error.message).with_position(position)
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
