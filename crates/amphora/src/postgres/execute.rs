//! A session's statements on an engine connection of the PostgreSQL dialect (docs/design/postgres.md,
//! section 11's unit 9): a Query's text is parsed, analyzed and lowered into the engine's statement,
//! which runs there, and its answer is written as PostgreSQL 19 writes it, RowDescription and DataRow
//! in text, CommandComplete, under 19's transaction rules.

use std::num::NonZero;
use std::sync::Arc;
use std::task::{Context, Poll};

use amphora_engine::dialect::postgres::{from_engine, to_engine};
use amphora_engine::storage::versioned::memory::Source;
use amphora_engine::storage::versioned::switch::Level;
use amphora_engine::{
    ConnectOpts, Connection, LimboError, PlatformIO, PostgresDialect, Statement, Value,
};
use amphora_pg_analysis::datetime::TimeZone;
use amphora_pg_analysis::stmt::Command;
use amphora_pg_analysis::{Column, Error, Field, Relations, analyze, lower, output, utility};
use amphora_pg_catalog::data::{PG_TYPE, type_oid};
use amphora_pg_catalog::{Datum, SearchPath};
use amphora_pg_parser::gram;
use amphora_pg_parser::tree::{Node, NodeId, TransactionStmtKind, Tree, Val};

use super::session::{Flow, Statements, TxStatus};
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
    /// The open transaction's isolation level (`XactIsoLevel`); READ COMMITTED out of one, 19's
    /// `default_transaction_isolation`.
    level: Level,
    /// Whether a statement of the block has taken a snapshot (`FirstSnapshotSet`).
    queried: bool,
    /// The Query a wait stopped, and the open transaction its statement waits for.
    stopped: Option<Stopped>,
    /// Why a stopped statement won't run again: its wait closed a ring of waits.
    refused: Option<PgError>,
}

/// A Query's statements, the next one to run at `at`.
struct Query {
    sql: String,
    tree: Tree,
    statements: Vec<NodeId>,
    at: usize,
}

struct Stopped {
    query: Query,
    holder: u64,
}

/// Why a statement didn't complete: a refusal, or a row another session's open transaction (by its
/// id) changed.
enum Stop {
    Refused(PgError),
    Wait(u64),
}

impl From<PgError> for Stop {
    fn from(error: PgError) -> Self {
        Self::Refused(error)
    }
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
            level: Level::ReadCommitted,
            queried: false,
            stopped: None,
            refused: None,
        }
        .at_level(Level::ReadCommitted)
    }

    fn at_level(mut self, level: Level) -> Self {
        self.set_level(level);
        self
    }

    /// The level of the transactions begun from now on.
    fn set_level(&mut self, level: Level) {
        self.level = level;
        self.conn.set_isolation(level);
    }

    fn statement(
        &mut self,
        sql: &str,
        tree: &Tree,
        id: NodeId,
        out: &mut Vec<u8>,
    ) -> Result<(), Stop> {
        let Node::RawStmt {
            stmt: Some(stmt), ..
        } = tree
            .get(id)
            .map_err(|_| internal("a parse tree node is missing"))?
        else {
            return Err(internal("an empty statement").into());
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
            if *chain {
                return Err(unsupported("this transaction command").into());
            }
            let level = isolation_level(tree, options)?;
            return Ok(self.transaction(*kind, level, out)?);
        }
        if self.block == Block::Failed {
            return Err(aborted().into());
        }
        let top = tree
            .get(*stmt)
            .map_err(|_| internal("a parse tree node is missing"))?;
        match top {
            Node::VariableShowStmt { name: Some(name) } if name == "transaction_isolation" => {
                return Ok(self.show_isolation(out)?);
            }
            Node::VariableSetStmt {
                kind: 3,
                name: Some(name),
                args,
                ..
            } if name == "TRANSACTION" => {
                let level = isolation_level(tree, args)?;
                return Ok(self.set_transaction(level, out)?);
            }
            _ => {}
        }
        self.queried |= self.block != Block::None;
        if self.block != Block::None {
            self.conn.renew_snapshot().map_err(engine)?;
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
                self.change_schema(&utility.sql)?;
                return Ok(complete(out, utility.tag)?);
            }
            let query = analyze(tree, id, &session, &[]).map_err(|e| refusal(e, sql))?;
            if !query.params.is_empty() {
                return Err(
                    PgError::error(SqlState::new("42P02"), "there is no parameter $1").into(),
                );
            }
            let lowered = lower(&query, &session).map_err(|e| refusal(e, sql))?;
            (query.command, lowered)
        };
        let (command, lowered) = lowered;
        self.run(command, &lowered, out)
    }

    /// Runs a statement that changes the catalog, whose transaction is validated in full from here on
    /// whatever its level (19 takes a lock that keeps the other sessions out of the table): an index it
    /// builds beside a commit that adds rows to the table would lack them, and is refused `40001`.
    fn change_schema(&self, sql: &str) -> Result<(), PgError> {
        // A transaction the statement begins, and one already open.
        self.conn.set_isolation(Level::Serializable);
        self.conn.validate_fully();
        let result = self.conn.execute(sql).map_err(engine);
        self.conn.set_isolation(self.level);
        result
    }

    /// Runs a lowered statement and writes its answer.
    fn run(
        &self,
        command: Command,
        lowered: &amphora_pg_analysis::Lowered,
        out: &mut Vec<u8>,
    ) -> Result<(), Stop> {
        match command {
            Command::Select => {
                let mut statement = self.prepared(lowered).map_err(engine)?;
                row_description(out, &lowered.fields)?;
                let rows = statement.run_collect_rows().map_err(engine)?;
                let mut count = 0usize;
                for row in &rows {
                    let values = self.row(&lowered.fields, row)?;
                    data_row(out, &values)?;
                    count = count.saturating_add(1);
                }
                complete(out, &format!("SELECT {count}"))?;
            }
            Command::Update => {
                let changed = self.change(lowered)?;
                complete(out, &format!("UPDATE {changed}"))?;
            }
            Command::Insert => {
                let changed = self.change(lowered)?;
                complete(out, &format!("INSERT 0 {changed}"))?;
            }
        }
        Ok(())
    }

    /// Runs a statement that changes rows, for how many it changed. A row another session's open
    /// transaction changed stops it, its changes undone and its transaction kept ([`Stop::Wait`]);
    /// one changed by a commit since its snapshot runs it again on a later one under READ COMMITTED
    /// (19's recheck of that row alone is section 11's unit 15), and refuses it under the others. Each
    /// turn of the loop follows a commit another session made since the last, so it ends with them.
    fn change(&self, lowered: &amphora_pg_analysis::Lowered) -> Result<i64, Stop> {
        let in_block = !self.conn.get_auto_commit();
        loop {
            if in_block {
                self.conn.open_statement().map_err(engine)?;
            }
            let ran = self.prepared(lowered).and_then(|mut statement| {
                statement.run_collect_rows()?;
                Ok(statement.n_change())
            });
            if in_block {
                self.conn.close_statement(ran.is_err()).map_err(engine)?;
            }
            match ran {
                Ok(changed) => return Ok(changed),
                Err(LimboError::RowTaken(Some(holder))) => return Err(Stop::Wait(holder)),
                Err(LimboError::RowTaken(None)) if self.level.statement_snapshots() => {
                    self.conn.renew_snapshot().map_err(engine)?;
                }
                Err(error) => return Err(engine(error).into()),
            }
        }
    }

    fn prepared(&self, lowered: &amphora_pg_analysis::Lowered) -> Result<Statement, LimboError> {
        let mut statement = self.conn.prepare(&lowered.sql)?;
        for (at, (type_oid, value)) in lowered.binds.iter().enumerate() {
            let value = match value {
                None => Value::Null,
                Some(value) => to_engine(*type_oid, value)?,
            };
            let index = at.checked_add(1).and_then(NonZero::new).ok_or_else(|| {
                LimboError::InternalError("a statement has too many parameters".to_owned())
            })?;
            statement.bind_at(index, value)?;
        }
        Ok(statement)
    }

    /// SHOW transaction_isolation: the level of the open transaction, READ COMMITTED out of one.
    fn show_isolation(&self, out: &mut Vec<u8>) -> Result<(), PgError> {
        let level = match self.level {
            Level::ReadUncommitted => "read uncommitted",
            Level::ReadCommitted => "read committed",
            Level::RepeatableRead => "repeatable read",
            Level::Serializable => "serializable",
        };
        row_description(
            out,
            &[Field {
                name: "transaction_isolation".to_owned(),
                table: 0,
                attnum: 0,
                type_oid: type_oid::TEXT,
                typmod: -1,
            }],
        )?;
        data_row(out, &[Some(level.to_owned())])?;
        complete(out, "SHOW")
    }

    /// SET TRANSACTION ISOLATION LEVEL (`ExecSetVariableStmt`'s VAR_SET_MULTI and
    /// `check_transaction_isolation`, REL_19_0 src/backend/utils/misc/guc_funcs.c and
    /// src/backend/commands/variable.c): out of a block it warns and is for the one statement.
    fn set_transaction(&mut self, level: Option<Level>, out: &mut Vec<u8>) -> Result<(), PgError> {
        if self.block == Block::None {
            let text = "SET TRANSACTION can only be used in transaction blocks";
            wire::notice_response(out, "WARNING", "25P01", text)?;
            return complete(out, "SET");
        }
        if let Some(level) = level
            && level != self.level
        {
            if self.queried {
                return Err(PgError::error(
                    SqlState::new("25001"),
                    "SET TRANSACTION ISOLATION LEVEL must be called before any query",
                ));
            }
            self.set_level(level);
        }
        complete(out, "SET")
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
    fn transaction(
        &mut self,
        kind: TransactionStmtKind,
        level: Option<Level>,
        out: &mut Vec<u8>,
    ) -> Result<(), PgError> {
        let tag = match kind {
            TransactionStmtKind::Start => "START TRANSACTION",
            _ => "BEGIN",
        };
        match (kind, self.block) {
            (TransactionStmtKind::Begin | TransactionStmtKind::Start, Block::None) => {
                self.set_level(level.unwrap_or(Level::ReadCommitted));
                self.begin()?;
                self.block = Block::Explicit;
                complete(out, tag)
            }
            (TransactionStmtKind::Begin | TransactionStmtKind::Start, Block::Implicit) => {
                if level.is_some() {
                    return Err(unsupported("a level in this transaction block"));
                }
                self.block = Block::Explicit;
                complete(out, tag)
            }
            (TransactionStmtKind::Begin | TransactionStmtKind::Start, Block::Explicit) => {
                if level.is_some() {
                    return Err(unsupported("a level in this transaction block"));
                }
                let text = "there is already a transaction in progress";
                wire::notice_response(out, "WARNING", "25001", text)?;
                complete(out, tag)
            }
            (TransactionStmtKind::Begin | TransactionStmtKind::Start, Block::Failed) => {
                Err(aborted())
            }
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
        self.queried = false;
        self.conn.execute("BEGIN").map_err(engine)
    }

    /// Leaves the block, without the writes of the engine's transaction if one is open.
    fn end(&mut self) {
        self.block = Block::None;
        self.set_level(Level::ReadCommitted);
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
    fn statements(&mut self, query: &mut Query, out: &mut Vec<u8>) -> Result<(), Stop> {
        while let Some(id) = query.statements.get(query.at).copied() {
            if query.statements.len() > 1 && self.block == Block::None {
                self.set_level(Level::ReadCommitted);
                self.begin()?;
                self.block = Block::Implicit;
            }
            self.statement(&query.sql, &query.tree, id, out)?;
            query.at = query.at.saturating_add(1);
        }
        if self.block == Block::Implicit {
            let result = self.conn.execute("COMMIT").map_err(engine);
            self.end();
            result?;
        }
        Ok(())
    }

    /// Runs the rest of a Query: stopped where a statement waits, else answered to its end.
    fn run_query(&mut self, mut query: Query, out: &mut Vec<u8>) -> Result<Flow, PgError> {
        match self.statements(&mut query, out) {
            Ok(()) => Ok(Flow::Done),
            Err(Stop::Wait(holder)) => {
                self.stopped = Some(Stopped { query, holder });
                Ok(Flow::Waiting)
            }
            Err(Stop::Refused(error)) => {
                self.refuse(&error, out)?;
                Ok(Flow::Done)
            }
        }
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

    fn simple_query(&mut self, sql: &str, out: &mut Vec<u8>) -> Result<Flow, PgError> {
        match gram::parse(sql.as_bytes()) {
            Ok((tree, statements)) => {
                let query = Query {
                    sql: sql.to_owned(),
                    tree,
                    statements,
                    at: 0,
                };
                self.run_query(query, out)
            }
            Err(refused) => {
                let position = match refused.cursor {
                    0 => None,
                    at => Some(at),
                };
                let error = PgError::error(SqlState::new(refused.sqlstate), refused.message)
                    .with_position(position);
                self.refuse(&error, out)?;
                Ok(Flow::Done)
            }
        }
    }

    fn poll_wait(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let Some(holder) = self.stopped.as_ref().map(|stopped| stopped.holder) else {
            return Poll::Ready(());
        };
        match self.conn.wait_for(holder, cx.waker()) {
            Ok(true) => Poll::Pending,
            Ok(false) => Poll::Ready(()),
            Err(error) => {
                self.refused = Some(engine(error));
                Poll::Ready(())
            }
        }
    }

    fn resume(&mut self, out: &mut Vec<u8>) -> Result<Flow, PgError> {
        let Some(Stopped { query, .. }) = self.stopped.take() else {
            return Ok(Flow::Done);
        };
        match self.refused.take() {
            Some(error) => {
                self.refuse(&error, out)?;
                Ok(Flow::Done)
            }
            None => self.run_query(query, out),
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

/// The level a transaction's modes name, none when they name none. Their other modes refuse `0A000`
/// here, as their grammar actions aren't ported: only ISOLATION LEVEL's is.
fn isolation_level(tree: &Tree, modes: &[NodeId]) -> Result<Option<Level>, PgError> {
    let mut level = None;
    for mode in modes {
        let Ok(Node::DefElem {
            defname,
            arg: Some(arg),
            ..
        }) = tree.get(*mode)
        else {
            return Err(unsupported("this transaction mode"));
        };
        let Ok(Node::AConst {
            val: Val::String(name),
            ..
        }) = tree.get(*arg)
        else {
            return Err(unsupported("this transaction mode"));
        };
        level = match (defname.as_str(), name.as_str()) {
            ("transaction_isolation", "read uncommitted") => Some(Level::ReadUncommitted),
            ("transaction_isolation", "read committed") => Some(Level::ReadCommitted),
            ("transaction_isolation", "repeatable read") => Some(Level::RepeatableRead),
            ("transaction_isolation", "serializable") => Some(Level::Serializable),
            _ => return Err(unsupported("this transaction mode")),
        };
    }
    Ok(level)
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
        LimboError::Deadlock => PgError::error(SqlState::new("40P01"), "deadlock detected"),
        LimboError::Busy
        | LimboError::BusySnapshot
        | LimboError::WriteWriteConflict
        | LimboError::RowTaken(_)
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
