//! A session's statements on an engine connection of the PostgreSQL dialect (docs/design/postgres.md,
//! section 11's unit 9): a Query's text is parsed, analyzed and lowered into the engine's statement,
//! which runs there, and its answer is written as PostgreSQL 19 writes it, RowDescription, DataRow and
//! CommandComplete, under 19's transaction rules. The extended protocol's messages (unit 10) are
//! [`super::portal`]'s.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZero;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use amphora_engine::dialect::postgres::{from_engine, to_engine};
use amphora_engine::storage::versioned::memory::Source;
use amphora_engine::storage::versioned::switch::Level;
use amphora_engine::{
    ConnectOpts, Connection, LimboError, PlatformIO, PostgresDialect, Statement, StepResult, Value,
};
use amphora_pg_analysis::datetime::TimeZone;
use amphora_pg_analysis::stmt::Command;
use amphora_pg_analysis::utility::{Effect, Utility};
use amphora_pg_analysis::{
    Column, Error, Field, Lowered, Relations, Value as PgValue, analyze, binary, lower, output,
    utility,
};
use amphora_pg_catalog::data::{PG_TYPE, type_oid};
use amphora_pg_catalog::{Datum, SearchPath};
use amphora_pg_parser::gram;
use amphora_pg_parser::tree::{Node, NodeId, TransactionStmtKind, Tree, Val};

use super::copy::{CopyIn, Then};
use super::extended::{Formats, Message};
use super::portal::{Portal, Prepared, collect, setting_field};
use super::session::{Copy, Flow, Statements, TxStatus, WaitLimits};
use super::settings::{Settings, Transactional};
use super::wire::{self, PgError, Severity, SqlState};
use crate::memory::{Budget, Charge, Use};

/// Microseconds from PostgreSQL's epoch, 2000-01-01 UTC, to Unix's (`src/include/datatype/timestamp.h`,
/// `POSTGRES_EPOCH_JDATE - UNIX_EPOCH_JDATE` days).
const UNIX_EPOCH_MICROS: i64 = -946_684_800_000_000;

/// A database of the PostgreSQL dialect on the engine's versioned storage, where a transaction's writes
/// are row versions of its own: one open across its client's round trips holds no writer (section 2).
pub struct Database {
    engine: Arc<amphora_engine::Database>,
    memory: Arc<dyn Source>,
    waiting: Arc<Waiting>,
}

/// The sessions that wait for a row, each by the transaction that waits: what a deadlock's report names
/// them by (`DeadLockReport`, REL_19_0 src/backend/storage/lmgr/deadlock.c). Bounded by the sessions.
#[derive(Default)]
struct Waiting(Mutex<BTreeMap<u64, i32>>);

impl Waiting {
    fn change<R>(&self, f: impl FnOnce(&mut BTreeMap<u64, i32>) -> R) -> R {
        f(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// A session's role, and whether it is a superuser.
#[derive(Debug, Clone, Copy)]
pub struct SessionRole<'a> {
    pub name: &'a str,
    pub superuser: bool,
}

impl Database {
    /// The database at `path`, made as needed, its storage's memory taken from `memory`.
    pub fn open(path: &str, memory: Arc<dyn Source>) -> Result<Self, LimboError> {
        let engine = amphora_engine::Database::open_file(
            Arc::new(PlatformIO::new()?),
            path,
            Arc::new(PostgresDialect::new()),
        )?;
        let database = Self {
            engine,
            memory,
            waiting: Arc::default(),
        };
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

    /// A session of `role`, `process_id` its backend's: an executor on a connection of its own. A
    /// refusal ends the session, as PostgreSQL's does one it can't open a database for.
    pub fn session(
        &self,
        role: SessionRole<'_>,
        time_zone: &str,
        process_id: i32,
    ) -> Result<Executor, PgError> {
        let fatal = |error: PgError| PgError {
            severity: Severity::Fatal,
            ..error
        };
        let zone = amphora_pg_analysis::tz::time_zone(time_zone)
            .map_err(|error| fatal(refusal(error, "")))?;
        let conn = self.connect().map_err(|error| fatal(engine(error)))?;
        let mut executor = Executor::new(conn, role.name, zone, Box::new(system_clock));
        executor.process_id = process_id;
        executor.waiting = Arc::clone(&self.waiting);
        executor.superuser = role.superuser;
        Ok(executor)
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

    fn index(&self, relation: u32, name: &str) -> bool {
        let Some((_, table)) = self.name(relation) else {
            return false;
        };
        self.first(
            "SELECT 1 FROM sqlite_schema WHERE type = 'index' AND name = ?1 AND tbl_name = ?2",
            &[
                Value::build_text(name.to_owned()),
                Value::build_text(table),
            ],
        )
        .is_some()
    }
}

/// The transaction block a session is in between two statements, as far as xact.c's `TBlockState` tells
/// them apart (REL_19_0 src/backend/access/transam/xact.c:141-165).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Block {
    /// Each statement is its own transaction.
    None,
    /// The extended protocol's messages since the last Sync are one transaction (`TBLOCK_STARTED`),
    /// before an Execute of theirs has run one.
    Started,
    /// A Query of several statements, or the messages after the first Execute since the last Sync, are
    /// one transaction (`TBLOCK_IMPLICIT_INPROGRESS`): it ends with the Query or at Sync, unless BEGIN
    /// makes it the client's.
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
    pub(super) block: Block,
    /// The open transaction's start (`GetCurrentTransactionStartTimestamp`).
    pub(super) began: i64,
    /// The open transaction's isolation level (`XactIsoLevel`); READ COMMITTED out of one, 19's
    /// `default_transaction_isolation`.
    level: Level,
    /// Whether a statement of the block has taken a snapshot (`FirstSnapshotSet`).
    queried: bool,
    /// The Query a wait stopped, and the open transaction its statement waits for.
    stopped: Option<Stopped>,
    /// Why a stopped statement won't go on: its wait is part of a ring of waits, or went on too long.
    wait_refused: Option<PgError>,
    /// The session among those that wait, and by what number.
    waiting: Arc<Waiting>,
    process_id: i32,
    superuser: bool,
    settings: Transactional<Settings>,
    /// The prepared statements, and the unnamed one.
    pub(super) named: BTreeMap<String, Rc<Prepared>>,
    pub(super) unnamed: Option<Rc<Prepared>>,
    /// The transaction's portals, the unnamed one under no name.
    pub(super) portals: BTreeMap<String, Portal>,
    /// Whether an Execute has run since the last Sync (`XACT_FLAGS_PIPELINING`).
    pub(super) pipelining: bool,
    /// What the statements and portals are charged to: none bounds them by the client's messages alone.
    budget: Option<Arc<Budget>>,
    /// The COPY under way, and what goes on once it ended.
    pub(super) copying: Option<(Box<CopyIn>, Then)>,
    /// The tables the open transaction made or emptied, by OID (`rd_createSubid` and
    /// `rd_newRelfilelocatorSubid`, src/include/utils/rel.h): what COPY FREEZE asks for.
    fresh: BTreeSet<u32>,
}

/// A Query's statements, the next one to run at `at`.
pub(super) struct Query {
    sql: String,
    tree: Tree,
    statements: Vec<NodeId>,
    pub(super) at: usize,
}

struct Stopped {
    work: Work,
    holder: u64,
    /// The statement stopped partway, what it changed kept; none for one undone, to run again.
    running: Option<Box<Running>>,
}

/// What a wait stopped.
enum Work {
    Query(Query),
    /// An Execute of the portal of that name.
    Execute(String),
}

/// A statement that changes rows, under way.
pub(super) struct Running {
    statement: Statement,
    command: Command,
    lowered: Lowered,
    /// Whether it has a savepoint of its own in the block's transaction, to be undone to.
    in_block: bool,
}

/// Why a statement didn't complete: a refusal, or a row another session's open transaction (by its
/// id) changed.
pub(super) enum Stop {
    Refused(PgError),
    Wait {
        holder: u64,
        running: Option<Box<Running>>,
    },
    /// A COPY FROM STDIN began: its data is to come.
    Copy(Box<CopyIn>),
}

/// How far a statement's step got: to its end, with how many rows it changed, or to a row of the open
/// transaction named.
pub(super) enum Ran {
    Done(i64),
    Waits(u64),
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
            wait_refused: None,
            waiting: Arc::default(),
            process_id: 0,
            superuser: false,
            settings: Transactional::default(),
            named: BTreeMap::new(),
            unnamed: None,
            portals: BTreeMap::new(),
            pipelining: false,
            budget: None,
            copying: None,
            fresh: BTreeSet::new(),
        }
        .at_level(Level::ReadCommitted)
    }

    /// The executor whose prepared statements and portals are charged to `budget`, as its session is.
    #[must_use]
    pub fn charged_to(mut self, budget: Arc<Budget>) -> Self {
        self.budget = Some(budget);
        self
    }

    /// `bytes` of the session's, refused `53200` when there is no room for them.
    pub(super) fn charge(&self, bytes: usize) -> Result<Option<Charge>, PgError> {
        let Some(budget) = &self.budget else {
            return Ok(None);
        };
        let taken = budget.take(Use::Sessions, u64::try_from(bytes).unwrap_or(u64::MAX));
        taken.map(Some).map_err(|_| {
            PgError::error(SqlState::OUT_OF_MEMORY, "out of memory")
                .with_detail(format!("Failed on request of size {bytes}."))
        })
    }

    /// What analysis asks of the session; `now` is its transaction's start.
    pub(super) fn analysis(&self, now: Option<i64>) -> amphora_pg_analysis::Session<'_> {
        amphora_pg_analysis::Session {
            path: &self.path,
            relations: &self.catalog,
            time_zone: &self.time_zone,
            now,
        }
    }

    /// `start_xact_command` (postgres.c:2788): an extended message's transaction, begun by the first since
    /// the last Sync, and an implicit block from the first after an Execute.
    pub(super) fn start_command(&mut self) -> Result<(), PgError> {
        match self.block {
            Block::None => {
                self.block = Block::Started;
                self.pipelining = false;
                self.queried = false;
                self.began = (self.clock)();
            }
            Block::Started if self.pipelining => {
                self.block = Block::Implicit;
                self.ensure_transaction()?;
            }
            Block::Started | Block::Implicit | Block::Explicit | Block::Failed => {}
        }
        Ok(())
    }

    /// The engine's transaction of the messages since the last Sync, begun before the first of them
    /// that changes anything: what they change is kept at Sync or not at all.
    pub(super) fn ensure_transaction(&self) -> Result<(), PgError> {
        if matches!(self.block, Block::Started | Block::Implicit) && self.conn.get_auto_commit() {
            self.conn.execute("BEGIN").map_err(engine)?;
        }
        Ok(())
    }

    /// `GetTransactionSnapshot` (src/backend/utils/time/snapmgr.c): the statement's, under READ COMMITTED.
    pub(super) fn take_snapshot(&mut self) -> Result<(), PgError> {
        self.queried = true;
        self.conn.renew_snapshot().map_err(engine)
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

    pub(super) fn statement(
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
            Node::VariableShowStmt { name: Some(name) } => {
                return Ok(self.show(name, out)?);
            }
            Node::VariableSetStmt {
                kind,
                name: Some(name),
                args,
                is_local,
                ..
            } if Settings::has(name) => {
                return Ok(self.set(tree, *kind, name, args, *is_local, out)?);
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
        if let Node::VacuumStmt {
            is_vacuumcmd: true, ..
        } = top
            && !matches!(self.block, Block::None | Block::Started)
        {
            // PreventInTransactionBlock (REL_19_0 src/backend/access/transam/xact.c:3745).
            return Err(PgError::error(
                SqlState::new("25001"),
                "VACUUM cannot run inside a transaction block",
            )
            .into());
        }
        if let Node::CopyStmt { .. } = top {
            return Err(Stop::copy(self.copy_begin(sql, tree, id, out)?));
        }
        if self.block != Block::None {
            self.take_snapshot()?;
        }
        let now = if self.block == Block::None {
            (self.clock)()
        } else {
            self.began
        };
        let lowered = {
            let session = self.analysis(Some(now));
            if let Some(utility) = utility(tree, id, &session).map_err(|e| refusal(e, sql))? {
                drop(session);
                self.ensure_transaction()?;
                return Ok(self.run_utility(&utility, out)?);
            }
            let query = analyze(tree, id, &session, &[]).map_err(|e| refusal(e, sql))?;
            if !query.params.is_empty() {
                return Err(
                    PgError::error(SqlState::new("42P02"), "there is no parameter $1").into(),
                );
            }
            let lowered = lower(&query, &session, &[]).map_err(|e| refusal(e, sql))?;
            (query.command, lowered)
        };
        let (command, lowered) = lowered;
        self.run(command, lowered, out)
    }

    /// Runs a utility statement's parts, all or none, and its notices and tag.
    fn run_utility(&mut self, utility: &Utility, out: &mut Vec<u8>) -> Result<(), PgError> {
        for text in &utility.notices {
            wire::notice_response(out, "NOTICE", "00000", text)?;
        }
        // Outside a block the parts are one transaction.
        let own = self.block == Block::None && utility.sql.len() > 1;
        if own {
            self.conn.execute("BEGIN").map_err(engine)?;
        }
        let mut ran = Ok(());
        for sql in &utility.sql {
            ran = self.change_schema(sql);
            if ran.is_err() {
                break;
            }
        }
        if own && ran.is_ok() {
            ran = self.conn.execute("COMMIT").map_err(engine);
        }
        if let Err(error) = ran {
            if own && !self.conn.get_auto_commit() {
                // A refused ROLLBACK has nothing more to undo: the engine's own refusal is logged.
                if let Err(error) = self.conn.execute("ROLLBACK") {
                    tracing::warn!(target: "amphora.postgres.execute", event = "rollback_failed", %error);
                }
            }
            return Err(match &utility.effect {
                Effect::Keyed(index) if error.code == SqlState::new("23505") => PgError::error(
                    SqlState::new("23505"),
                    format!("could not create unique index \"{index}\""),
                ),
                _ => error,
            });
        }
        if self.block != Block::None {
            match &utility.effect {
                Effect::Created(table) => {
                    let public = self.catalog.namespace("public");
                    if let Some(oid) = public.and_then(|ns| self.catalog.relation(ns, table)) {
                        self.fresh.insert(oid);
                    }
                }
                Effect::Emptied(oids) => self.fresh.extend(oids),
                Effect::None | Effect::Keyed(_) => {}
            }
        }
        complete(out, utility.tag)
    }

    /// Whether the open transaction made or emptied the table (copyfrom.c's check for COPY FREEZE).
    pub(super) fn made_or_emptied(&self, relation: u32) -> bool {
        self.fresh.contains(&relation)
    }

    /// `ThereAreNoReadyPortals` (src/backend/utils/mmgr/portalmem.c): whether one bound, or stopped partway, waits.
    pub(super) fn holds_a_ready_portal(&self) -> bool {
        self.portals.values().any(Portal::is_ready)
    }

    /// A statement of the engine's for a COPY's rows.
    pub(super) fn prepare(&self, sql: &str) -> Result<Statement, PgError> {
        self.conn.prepare(sql).map_err(engine)
    }

    /// Commits the transaction a COPY began for itself.
    pub(super) fn conn_commit(&mut self) -> Result<(), PgError> {
        let result = self.conn.execute("COMMIT").map_err(engine);
        if result.is_err() {
            self.abandon();
        }
        result
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
    fn run(&self, command: Command, lowered: Lowered, out: &mut Vec<u8>) -> Result<(), Stop> {
        match command {
            Command::Select => {
                let lowered = &lowered;
                let mut statement = self.prepared(lowered).map_err(engine)?;
                let text = Formats::default();
                row_description(out, &lowered.fields, &text)?;
                let (rows, refused) = collect(&mut statement);
                for row in &rows {
                    self.data_row(&lowered.fields, &text, row, out)?;
                }
                if let Some(error) = refused {
                    return Err(engine(error).into());
                }
                complete(out, &format!("SELECT {}", rows.len()))?;
            }
            Command::Update | Command::Insert => {
                let running = self.begin_change(command, lowered)?;
                self.change(running, out)?;
            }
        }
        Ok(())
    }

    /// A statement that changes rows, ready to run: in a block, undone alone when it has to be.
    pub(super) fn begin_change(
        &self,
        command: Command,
        lowered: Lowered,
    ) -> Result<Box<Running>, PgError> {
        let in_block = !self.conn.get_auto_commit();
        if in_block {
            self.conn.open_statement().map_err(engine)?;
        }
        match self.prepared(&lowered) {
            Ok(statement) => Ok(Box::new(Running {
                statement,
                command,
                lowered,
                in_block,
            })),
            Err(error) => {
                if in_block {
                    self.conn.close_statement(true).map_err(engine)?;
                }
                Err(engine(error))
            }
        }
    }

    /// Runs a statement that changes rows to its end, and completes it. An UPDATE stops at a row that
    /// another session's open transaction changed, what it changed before kept, and goes on from there
    /// once that transaction has ended: with the row as it was before, or with the version that
    /// transaction committed, which is judged by the WHERE again (19's EvalPlanQual, in
    /// `Insn::LockRow`). A unique key taken so is an INSERT's whole row: undone and run again on a later
    /// snapshot under READ COMMITTED. Each turn of the loop follows a commit another session made
    /// since the last, so it ends with them.
    pub(super) fn change(&self, mut running: Box<Running>, out: &mut Vec<u8>) -> Result<(), Stop> {
        loop {
            let error = match self.step(&mut running.statement) {
                Ok(Ran::Done(changed)) => {
                    if running.in_block {
                        self.conn.close_statement(false).map_err(engine)?;
                    }
                    let tag = match running.command {
                        Command::Insert => format!("INSERT 0 {changed}"),
                        Command::Select | Command::Update => format!("UPDATE {changed}"),
                    };
                    return Ok(complete(out, &tag)?);
                }
                Ok(Ran::Waits(holder)) => {
                    return Err(Stop::Wait {
                        holder,
                        running: Some(running),
                    });
                }
                Err(error) => error,
            };
            let (command, lowered) = self.undo(running)?;
            match error {
                LimboError::RowTaken(Some(holder)) => {
                    return Err(Stop::Wait {
                        holder,
                        running: None,
                    });
                }
                LimboError::RowTaken(None) if self.level.statement_snapshots() => {
                    self.conn.renew_snapshot().map_err(engine)?;
                    running = self.begin_change(command, lowered)?;
                }
                error => return Err(engine(error).into()),
            }
        }
    }

    /// Ends a statement partway, what it changed undone.
    fn undo(&self, running: Box<Running>) -> Result<(Command, Lowered), PgError> {
        let Running {
            statement,
            command,
            lowered,
            in_block,
        } = *running;
        drop(statement);
        if in_block {
            self.conn.close_statement(true).map_err(engine)?;
        }
        Ok((command, lowered))
    }

    pub(super) fn step(&self, statement: &mut Statement) -> Result<Ran, LimboError> {
        loop {
            match statement.step()? {
                StepResult::Done => return Ok(Ran::Done(statement.n_change())),
                StepResult::Row => {}
                StepResult::Yield | StepResult::IO | StepResult::Sleep { .. } => {
                    if let Some(holder) = self.conn.take_row_wait() {
                        return Ok(Ran::Waits(holder));
                    }
                    statement.get_pager().io.step()?;
                }
                StepResult::Interrupt => return Err(LimboError::Interrupt),
                StepResult::Busy => return Err(LimboError::Busy),
            }
        }
    }

    pub(super) fn prepared(&self, lowered: &Lowered) -> Result<Statement, LimboError> {
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

    /// What SHOW answers, the setting's name and its value: `transaction_isolation`, the level of the open
    /// transaction (READ COMMITTED out of one), and [`Settings`].
    pub(super) fn shown(&self, name: &str) -> Result<(String, String), PgError> {
        if name == "transaction_isolation" {
            let level = match self.level {
                Level::ReadUncommitted => "read uncommitted",
                Level::ReadCommitted => "read committed",
                Level::RepeatableRead => "repeatable read",
                Level::Serializable => "serializable",
            };
            return Ok((name.to_owned(), level.to_owned()));
        }
        if !Settings::has(name) {
            return Err(unsupported("this statement"));
        }
        let (name, value) = self.settings.current().show(name)?;
        Ok((name.to_owned(), value))
    }

    /// SET TRANSACTION ISOLATION LEVEL (`ExecSetVariableStmt`'s VAR_SET_MULTI and
    /// `check_transaction_isolation`, REL_19_0 src/backend/utils/misc/guc_funcs.c and
    /// src/backend/commands/variable.c): out of a block it warns and is for the one statement.
    fn set_transaction(&mut self, level: Option<Level>, out: &mut Vec<u8>) -> Result<(), PgError> {
        if matches!(self.block, Block::None | Block::Started) {
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

    fn show(&self, name: &str, out: &mut Vec<u8>) -> Result<(), PgError> {
        let (name, value) = self.shown(name)?;
        let columns = [setting_field(name)];
        let text = Formats::default();
        row_description(out, &columns, &text)?;
        self.data_row(&columns, &text, &[Value::build_text(value)], out)?;
        complete(out, "SHOW")
    }

    /// SET, SET LOCAL and RESET of one of [`Settings`] (`ExecSetVariableStmt`, REL_19_0
    /// src/backend/utils/misc/guc_funcs.c): the value lasts as the transaction that set it does.
    fn set(
        &mut self,
        tree: &Tree,
        kind: i32,
        name: &str,
        args: &[NodeId],
        local: bool,
        out: &mut Vec<u8>,
    ) -> Result<(), PgError> {
        // VAR_SET_VALUE, VAR_SET_DEFAULT and VAR_RESET.
        let value = match (kind, args) {
            (0, [arg]) => {
                let Ok(Node::AConst { val, .. }) = tree.get(*arg) else {
                    return Err(unsupported("this setting's value"));
                };
                Some(match val {
                    Val::Integer(i) => i.to_string(),
                    Val::Float(text) | Val::String(text) => text.clone(),
                    Val::Boolean(b) => b.to_string(),
                })
            }
            (1 | 4, []) => None,
            _ => return Err(unsupported("this form of SET")),
        };
        let changed = self
            .settings
            .current()
            .with(name, value.as_deref(), self.superuser)?;
        if local && matches!(self.block, Block::None | Block::Started) {
            let text = "SET LOCAL can only be used in transaction blocks";
            wire::notice_response(out, "WARNING", "25P01", text)?;
        }
        if self.block == Block::None {
            if !local {
                self.settings.set(changed);
            }
        } else if local {
            self.settings.set_local(changed);
        } else {
            let session = self
                .settings
                .session()
                .with(name, value.as_deref(), self.superuser)?;
            self.settings.set_in_block(changed, session);
        }
        complete(out, if kind == 4 { "RESET" } else { "SET" })
    }

    /// A row's columns in their formats, as 19's output and send functions write them (`None` for NULL).
    pub(super) fn row(
        &self,
        fields: &[Field],
        formats: &Formats,
        row: &[Value],
    ) -> Result<Vec<Option<Vec<u8>>>, PgError> {
        let session = self.analysis(None);
        let mut written = Vec::with_capacity(fields.len());
        for (at, (field, stored)) in fields.iter().zip(row).enumerate() {
            let value = from_engine(field.type_oid, stored).map_err(engine)?;
            let bytes = match value {
                None => None,
                Some(mut value) => {
                    if let (type_oid::BPCHAR, PgValue::Text(text)) = (field.type_oid, &mut value) {
                        pad(text, field.typmod);
                    }
                    let bytes = if formats.of(at) == 0 {
                        output(field.type_oid, &value, &session).map(String::into_bytes)
                    } else {
                        binary::send(field.type_oid, &value)
                    };
                    Some(bytes.map_err(|e| refusal(e, ""))?)
                }
            };
            written.push(bytes);
        }
        Ok(written)
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
            (TransactionStmtKind::Begin | TransactionStmtKind::Start, Block::Started) => {
                self.set_level(level.unwrap_or(Level::ReadCommitted));
                self.ensure_transaction()?;
                self.block = Block::Explicit;
                complete(out, tag)
            }
            (TransactionStmtKind::Begin | TransactionStmtKind::Start, Block::Implicit) => {
                if level.is_some() {
                    return Err(unsupported("a level in this transaction block"));
                }
                self.ensure_transaction()?;
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
            (TransactionStmtKind::Commit, Block::Started | Block::Implicit) => {
                no_transaction(out)?;
                self.commit(out)
            }
            (TransactionStmtKind::Commit, Block::None) => {
                no_transaction(out)?;
                complete(out, "COMMIT")
            }
            (TransactionStmtKind::Rollback, Block::None | Block::Started | Block::Implicit) => {
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
        self.commit_block()?;
        complete(out, "COMMIT")
    }

    /// Commits the transaction of a block and leaves it.
    fn commit_block(&mut self) -> Result<(), PgError> {
        let result = if self.conn.get_auto_commit() {
            Ok(())
        } else {
            self.conn.execute("COMMIT").map_err(engine)
        };
        if result.is_ok() {
            self.settings.commit();
        }
        self.end();
        result
    }

    pub(super) fn begin(&mut self) -> Result<(), PgError> {
        self.began = (self.clock)();
        self.queried = false;
        self.conn.execute("BEGIN").map_err(engine)
    }

    /// Leaves the block, without the writes of the engine's transaction if one is open: its portals go
    /// with it (`AtCleanup_Portals`, REL_19_0 src/backend/utils/mmgr/portalmem.c).
    pub(super) fn end(&mut self) {
        self.block = Block::None;
        self.portals.clear();
        self.abandon();
        self.set_level(Level::ReadCommitted);
    }

    /// Ends the engine's transaction if one is open, without its writes, and what it set
    /// (`AbortTransaction`, REL_19_0 src/backend/access/transam/xact.c): the sessions that wait for a
    /// row of its go on.
    fn abandon(&mut self) {
        self.settings.rollback();
        if !self.conn.get_auto_commit() {
            // A refused ROLLBACK has nothing more to undo: the engine's own refusal is logged.
            if let Err(error) = self.conn.execute("ROLLBACK") {
                tracing::warn!(target: "amphora.postgres.execute", event = "rollback_failed", %error);
            }
        }
    }

    /// Answers with `error`, and ends what it ends: the transaction, at once
    /// (`AbortCurrentTransaction`), and the block but for END.
    pub(super) fn refused(&mut self, error: &PgError, out: &mut Vec<u8>) -> Result<(), PgError> {
        match self.block {
            Block::None | Block::Started | Block::Implicit => self.end(),
            Block::Explicit | Block::Failed => {
                self.block = Block::Failed;
                for portal in self.portals.values_mut() {
                    portal.fail();
                }
                self.abandon();
            }
        }
        wire::error_response(out, error)
    }

    /// A Query's statements in order (`exec_simple_query`, REL_19_0 src/backend/tcop/postgres.c): several
    /// run in an implicit block, begun again after one that ends it.
    fn statements(
        &mut self,
        query: &mut Query,
        mut running: Option<Box<Running>>,
        out: &mut Vec<u8>,
    ) -> Result<(), Stop> {
        while let Some(id) = query.statements.get(query.at).copied() {
            match running.take() {
                Some(running) => self.change(running, out)?,
                None => {
                    if query.statements.len() > 1 && self.block == Block::None {
                        self.set_level(Level::ReadCommitted);
                        self.begin()?;
                        self.block = Block::Implicit;
                    }
                    // exec_simple_query runs each statement in the unnamed portal.
                    self.portals.remove("");
                    self.statement(&query.sql, &query.tree, id, out)?;
                }
            }
            query.at = query.at.saturating_add(1);
        }
        if matches!(self.block, Block::Started | Block::Implicit) {
            self.commit_block()?;
        }
        Ok(())
    }

    /// Runs the rest of a Query, from `running` where a statement of it is under way: stopped where a
    /// statement waits, else answered to its end.
    pub(super) fn run_query(
        &mut self,
        mut query: Query,
        running: Option<Box<Running>>,
        out: &mut Vec<u8>,
    ) -> Result<Flow, PgError> {
        match self.statements(&mut query, running, out) {
            Ok(()) => Ok(Flow::Done),
            Err(Stop::Wait { holder, running }) => {
                self.stopped = Some(Stopped {
                    work: Work::Query(query),
                    holder,
                    running,
                });
                Ok(Flow::Waiting)
            }
            Err(Stop::Copy(copy)) => {
                self.copying = Some((copy, Then::Query(query)));
                Ok(Flow::CopyIn)
            }
            Err(Stop::Refused(error)) => {
                self.refused(&error, out)?;
                Ok(Flow::Done)
            }
        }
    }

    /// How an Execute of the portal `name` ended: the messages until Sync are skipped after a refusal.
    fn executed(
        &mut self,
        name: &str,
        ran: Result<(), Stop>,
        out: &mut Vec<u8>,
    ) -> Result<Flow, PgError> {
        match ran {
            Ok(()) => Ok(Flow::Done),
            Err(Stop::Wait { holder, running }) => {
                self.stopped = Some(Stopped {
                    work: Work::Execute(name.to_owned()),
                    holder,
                    running,
                });
                Ok(Flow::Waiting)
            }
            Err(Stop::Copy(copy)) => {
                self.copying = Some((copy, Then::Execute));
                Ok(Flow::CopyIn)
            }
            Err(Stop::Refused(error)) => {
                self.refused(&error, out)?;
                Ok(Flow::Refused)
            }
        }
    }
}

/// A client that goes away mid-transaction leaves none of its writes (`AbortOutOfAnyTransaction` as a
/// backend exits, REL_19_0 src/backend/access/transam/xact.c).
impl Drop for Executor {
    fn drop(&mut self) {
        self.stop_waiting();
        if let Some(Stopped {
            running: Some(running),
            ..
        }) = self.stopped.take()
        {
            drop(running.statement);
        }
        self.end();
    }
}

impl Statements for Executor {
    fn status(&self) -> TxStatus {
        match self.block {
            Block::None | Block::Started | Block::Implicit => TxStatus::Idle,
            Block::Explicit => TxStatus::InTransaction,
            Block::Failed => TxStatus::Failed,
        }
    }

    fn simple_query(&mut self, sql: &str, out: &mut Vec<u8>) -> Result<Flow, PgError> {
        // drop_unnamed_stmt (postgres.c:1067), and the transaction of messages no Sync has ended.
        self.unnamed = None;
        if self.block == Block::Started {
            self.start_command()?;
        }
        match gram::parse(sql.as_bytes()) {
            Ok((tree, statements)) => {
                if statements.is_empty() {
                    wire::message(out, b'I', |_| {})?;
                }
                let query = Query {
                    sql: sql.to_owned(),
                    tree,
                    statements,
                    at: 0,
                };
                self.run_query(query, None, out)
            }
            Err(refused) => {
                self.refused(&unparsed(refused), out)?;
                Ok(Flow::Done)
            }
        }
    }

    fn extended(&mut self, message: Message<'_>, out: &mut Vec<u8>) -> Result<Flow, PgError> {
        let done = match message {
            Message::Execute { portal, max_rows } => {
                let ran = self.execute(portal, max_rows, out);
                return self.executed(portal, ran, out);
            }
            Message::Parse {
                statement,
                sql,
                types,
            } => self.parse(statement, sql, &types, out),
            Message::Bind {
                portal,
                statement,
                values,
            } => self.bind(portal, statement, values, out),
            Message::Describe(object) => self.describe(object, out),
            Message::Close(object) => self.close(object, out),
        };
        match done {
            Ok(()) => Ok(Flow::Done),
            Err(error) => {
                self.refused(&error, out)?;
                Ok(Flow::Refused)
            }
        }
    }

    fn sync(&mut self, out: &mut Vec<u8>) -> Result<(), PgError> {
        if matches!(self.block, Block::Started | Block::Implicit)
            && let Err(error) = self.commit_block()
        {
            wire::error_response(out, &error)?;
        }
        Ok(())
    }

    fn refuse(&mut self, error: &PgError, out: &mut Vec<u8>) -> Result<(), PgError> {
        self.refused(error, out)
    }

    fn copy(&mut self, message: Copy<'_>, out: &mut Vec<u8>) -> Result<Flow, PgError> {
        self.copy_message(message, out)
    }

    fn poll_wait(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let Some(holder) = self.stopped.as_ref().map(|stopped| stopped.holder) else {
            return Poll::Ready(());
        };
        if let Some(waiter) = self.conn.transaction_id() {
            self.waiting
                .change(|waiting| waiting.insert(waiter, self.process_id));
        }
        match self.conn.wait_for(holder, cx.waker()) {
            Ok(true) => Poll::Pending,
            Ok(false) => Poll::Ready(()),
            Err(error) => {
                self.wait_refused = Some(engine(error));
                Poll::Ready(())
            }
        }
    }

    fn wait_limits(&self) -> WaitLimits {
        self.settings.current().wait_limits()
    }

    fn check_deadlock(&mut self) -> bool {
        let ring = self.conn.deadlock();
        if ring.is_empty() {
            return false;
        }
        let lines: Vec<String> = self.waiting.change(|waiting| {
            let pid = |id: u64| waiting.get(&id).copied().unwrap_or(0);
            ring.iter()
                .map(|(waiter, holder)| {
                    format!(
                        "Process {} waits for ShareLock on transaction {}; blocked by process {}.",
                        pid(*waiter),
                        transaction_number(*holder),
                        pid(*holder)
                    )
                })
                .collect()
        });
        self.stop_waiting();
        self.wait_refused = Some(
            PgError::error(SqlState::new("40P01"), "deadlock detected")
                .with_detail(lines.join("\n"))
                .with_hint("See server log for query details."),
        );
        true
    }

    fn lock_timed_out(&mut self) {
        self.stop_waiting();
        self.wait_refused = Some(PgError::error(
            SqlState::new("55P03"),
            "canceling statement due to lock timeout",
        ));
    }

    fn resume(&mut self, out: &mut Vec<u8>) -> Result<Flow, PgError> {
        self.stop_waiting();
        let Some(Stopped { work, running, .. }) = self.stopped.take() else {
            return Ok(Flow::Done);
        };
        if let Some(error) = self.wait_refused.take() {
            if let Some(running) = running {
                self.undo(running)?;
            }
            self.refused(&error, out)?;
            return Ok(match work {
                Work::Query(_) => Flow::Done,
                Work::Execute(_) => Flow::Refused,
            });
        }
        match work {
            Work::Query(query) => self.run_query(query, running, out),
            Work::Execute(name) => {
                let ran = match running {
                    Some(running) => self.change(running, out),
                    None => self.execute(&name, 0, out),
                };
                if ran.is_ok() {
                    self.pipelining = true;
                }
                self.executed(&name, ran, out)
            }
        }
    }
}

impl Executor {
    /// The session waits no more: its wait is over, or refused.
    fn stop_waiting(&self) {
        if let Some(waiter) = self.conn.transaction_id() {
            self.conn.stop_waiting();
            self.waiting.change(|waiting| waiting.remove(&waiter));
        }
    }
}

/// A transaction's number as a message names it: the engine's, without the bit that tells it from a
/// commit's timestamp.
fn transaction_number(id: u64) -> u64 {
    id & (u64::MAX >> 1)
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

/// The parser's refusal, at its cursor.
pub(super) fn unparsed(refused: gram::Refusal) -> PgError {
    let position = match refused.cursor {
        0 => None,
        at => Some(at),
    };
    PgError::error(SqlState::new(refused.sqlstate), refused.message).with_position(position)
}

fn internal(message: &str) -> PgError {
    PgError::error(SqlState::INTERNAL_ERROR, message)
}

/// A statement of a block that a refusal ended.
pub(super) fn aborted() -> PgError {
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
pub(super) fn refusal(error: Error, sql: &str) -> PgError {
    let position = usize::try_from(error.location)
        .ok()
        .and_then(|at| sql.get(..at))
        .map(|before| before.chars().count().saturating_add(1));
    PgError::error(SqlState::new(error.sqlstate), error.message).with_position(position)
}

/// What the engine refused.
pub(super) fn engine(error: LimboError) -> PgError {
    match error {
        LimboError::Postgres { sqlstate, message } => {
            PgError::error(SqlState::new(sqlstate), message)
        }
        LimboError::Constraint(message) => constraint(message),
        LimboError::ForeignKeyConstraint(message) => {
            PgError::error(SqlState::new("23000"), message)
        }
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

/// A constraint the engine refused: a key's is 19's of the table's `<table>_pkey` (its only index), and a NOT NULL's is
/// 19's own (`_bt_check_unique`, src/backend/access/nbtree/nbtinsert.c; `ReportNotNullViolationError`,
/// src/backend/executor/execMain.c).
fn constraint(message: String) -> PgError {
    let column = |prefix: &str| {
        let (table, column) = message.strip_prefix(prefix)?.split_once('.')?;
        Some((table.to_owned(), column.to_owned()))
    };
    if let Some((table, _)) = column("UNIQUE constraint failed: ") {
        return PgError::error(
            SqlState::new("23505"),
            format!("duplicate key value violates unique constraint \"{table}_pkey\""),
        );
    }
    if let Some((table, column)) = column("NOT NULL constraint failed: ") {
        return PgError::error(
            SqlState::new("23502"),
            format!(
                "null value in column \"{column}\" of relation \"{table}\" violates not-null constraint"
            ),
        );
    }
    PgError::error(SqlState::new("23000"), message)
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

pub(super) fn row_description(
    out: &mut Vec<u8>,
    fields: &[Field],
    formats: &Formats,
) -> Result<(), PgError> {
    let count =
        i16::try_from(fields.len()).map_err(|_| internal("a result has too many columns"))?;
    wire::message(out, b'T', |out| {
        out.extend_from_slice(&count.to_be_bytes());
        for (at, field) in fields.iter().enumerate() {
            wire::put_cstr(out, field.name.as_bytes());
            out.extend_from_slice(&field.table.to_be_bytes());
            let attnum = i16::try_from(field.attnum).unwrap_or(0);
            out.extend_from_slice(&attnum.to_be_bytes());
            out.extend_from_slice(&field.type_oid.to_be_bytes());
            out.extend_from_slice(&typlen(field.type_oid).to_be_bytes());
            out.extend_from_slice(&field.typmod.to_be_bytes());
            out.extend_from_slice(&formats.of(at).to_be_bytes());
        }
    })
}

/// CommandComplete.
pub(super) fn complete(out: &mut Vec<u8>, tag: &str) -> Result<(), PgError> {
    wire::message(out, b'C', |out| wire::put_cstr(out, tag.as_bytes()))
}
