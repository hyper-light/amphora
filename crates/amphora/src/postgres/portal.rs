//! A session's prepared statements and portals, as 19's backend keeps them (REL_19_0
//! src/backend/tcop/postgres.c: `exec_parse_message`, `exec_bind_message`, `exec_describe_*_message`,
//! `exec_execute_message`; src/backend/tcop/pquery.c; src/backend/utils/mmgr/portalmem.c). A prepared statement
//! lasts as long as its session; a portal as long as its transaction.
//!
//! A SELECT's portal takes its snapshot at its Bind, as 19's does (`PortalStart`), so it runs there: what
//! it holds until its Executes are its rows, charged to the process's budget, and no reader
//! (docs/design/postgres.md section 2, "Portals and long reads hold no connection between fetches").

use std::rc::Rc;

use amphora_engine::{LimboError, Statement, StepResult, Value as Stored};
use amphora_pg_analysis::stmt::Command;
use amphora_pg_analysis::{Field, Lowered, Value, analyze, binary, fields, input, lower};
use amphora_pg_catalog::data::type_oid;
use amphora_pg_parser::gram;
use amphora_pg_parser::tree::{Node, NodeId, TransactionStmtKind, Tree};

use super::execute::{Block, Executor, Stop, aborted, complete, engine, refusal, unparsed};
use super::extended::{Formats, Object, Values, text};
use super::wire::{self, PgError, SqlState};
use crate::memory::Charge;

/// A prepared statement (`CachedPlanSource`).
pub(super) struct Prepared {
    sql: String,
    tree: Tree,
    /// Its raw statement: none for an empty query string.
    statement: Option<NodeId>,
    what: What,
    /// `$n`'s types, `$1` first.
    params: Vec<u32>,
    /// Its result's columns: none where it gives no rows.
    fields: Option<Vec<Field>>,
    _held: Option<Charge>,
}

/// What a statement is, as the extended protocol tells them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum What {
    Empty,
    /// SELECT, INSERT or UPDATE, which analysis types.
    Optimizable,
    /// COMMIT or ROLLBACK, which a failed block takes (`IsTransactionExitStmt`).
    Exit,
    Show,
    Utility,
}

/// A portal (src/include/utils/portal.h).
pub(super) struct Portal {
    fields: Option<Vec<Field>>,
    formats: Formats,
    run: Run,
    _held: Option<Charge>,
}

/// What an Execute of a portal does.
enum Run {
    /// Answers EmptyQueryResponse.
    Empty,
    /// Sends the rows not sent yet, then what refused the next one.
    Rows {
        rows: std::vec::IntoIter<Vec<Stored>>,
        refused: Option<PgError>,
        tag: Tag,
    },
    Show(String),
    Change(Command, Lowered),
    /// Refuses what the planner leaves to the statement's run ([`amphora_pg_analysis::Error::runs`]).
    Refused(PgError),
    Utility(Rc<Prepared>),
    /// Nothing: it ran, or its transaction failed (`PORTAL_DONE`, `PORTAL_FAILED`).
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tag {
    Select,
    Show,
}

impl Portal {
    /// One being bound (`PORTAL_NEW`), or whose transaction failed.
    fn undefined() -> Self {
        Self {
            fields: None,
            formats: Formats::default(),
            run: Run::Done,
            _held: None,
        }
    }

    /// Its transaction failed (`AtAbort_Portals`): its rows go, its columns stay.
    pub(super) fn fail(&mut self) {
        self.run = Run::Done;
        self._held = None;
    }
}

/// A result's columns' bytes as they are held.
fn field_bytes(fields: &[Field]) -> usize {
    fields
        .iter()
        .map(|field| std::mem::size_of::<Field>().saturating_add(field.name.len()))
        .fold(0, usize::saturating_add)
}

/// A lowered INSERT's or UPDATE's bytes as they are held.
fn change_bytes(lowered: &Lowered) -> usize {
    lowered
        .binds
        .iter()
        .map(|(_, value)| {
            let text = match value {
                Some(Value::Text(text) | Value::Numeric(text) | Value::Unknown(text)) => text.len(),
                _ => 0,
            };
            std::mem::size_of::<(u32, Option<Value>)>().saturating_add(text)
        })
        .fold(lowered.sql.len(), usize::saturating_add)
}

/// A row's bytes as they are held.
fn row_bytes(row: &[Stored]) -> usize {
    row.iter()
        .map(|value| {
            let text = match value {
                Stored::Text(text) => text.as_str().len(),
                Stored::Blob(blob) => blob.len(),
                Stored::Null | Stored::Numeric(_) => 0,
            };
            std::mem::size_of::<Stored>().saturating_add(text)
        })
        .fold(0, usize::saturating_add)
}

/// A statement's rows to its end, and what refused the next one.
pub(super) fn collect(statement: &mut Statement) -> (Vec<Vec<Stored>>, Option<LimboError>) {
    let mut rows = Vec::new();
    loop {
        let refused = match statement.step() {
            Ok(StepResult::Done) => return (rows, None),
            Ok(StepResult::Row) => match statement.row() {
                Some(row) => {
                    rows.push(row.get_values().cloned().collect());
                    continue;
                }
                None => LimboError::InternalError("a row is missing".to_owned()),
            },
            Ok(StepResult::IO | StepResult::Yield | StepResult::Sleep { .. }) => {
                match statement.get_pager().io.step() {
                    Ok(()) => continue,
                    Err(error) => error,
                }
            }
            Ok(StepResult::Interrupt) => LimboError::Interrupt,
            Ok(StepResult::Busy) => LimboError::Busy,
            Err(error) => error,
        };
        return (rows, Some(refused));
    }
}

fn no_portal(name: &str) -> PgError {
    PgError::error(
        SqlState::UNDEFINED_CURSOR,
        format!("portal \"{name}\" does not exist"),
    )
}

fn raw(tree: &Tree, id: NodeId) -> Result<&Node, PgError> {
    let missing = || PgError::error(SqlState::INTERNAL_ERROR, "a parse tree node is missing");
    let Node::RawStmt {
        stmt: Some(stmt), ..
    } = tree.get(id).map_err(|_| missing())?
    else {
        return Err(missing());
    };
    tree.get(*stmt).map_err(|_| missing())
}

impl What {
    fn of(node: &Node) -> Self {
        match node {
            Node::SelectStmt(_) | Node::InsertStmt { .. } | Node::UpdateStmt { .. } => {
                Self::Optimizable
            }
            Node::TransactionStmt {
                kind: TransactionStmtKind::Commit | TransactionStmtKind::Rollback,
                ..
            } => Self::Exit,
            Node::VariableShowStmt { .. } => Self::Show,
            _ => Self::Utility,
        }
    }
}

impl Executor {
    /// `exec_parse_message`.
    pub(super) fn parse(
        &mut self,
        name: &str,
        sql: &str,
        types: &[u32],
        out: &mut Vec<u8>,
    ) -> Result<(), PgError> {
        self.start_command()?;
        if name.is_empty() {
            self.unnamed = None;
        }
        let (tree, statements) = gram::parse(sql.as_bytes()).map_err(unparsed)?;
        let statement = match statements.as_slice() {
            [] => None,
            [one] => Some(*one),
            _ => {
                return Err(PgError::error(
                    SqlState::SYNTAX_ERROR,
                    "cannot insert multiple commands into a prepared statement",
                ));
            }
        };
        let prepared = Rc::new(self.analyzed(name, sql, tree, statement, types)?);
        if name.is_empty() {
            self.unnamed = Some(prepared);
        } else if self.named.contains_key(name) {
            return Err(PgError::error(
                SqlState::DUPLICATE_PREPARED_STATEMENT,
                format!("prepared statement \"{name}\" already exists"),
            ));
        } else {
            self.named.insert(name.to_owned(), prepared);
        }
        wire::message(out, b'1', |_| {})
    }

    fn analyzed(
        &mut self,
        name: &str,
        sql: &str,
        tree: Tree,
        statement: Option<NodeId>,
        types: &[u32],
    ) -> Result<Prepared, PgError> {
        let mut what = What::Empty;
        let mut params = types.to_vec();
        let mut columns = None;
        if let Some(id) = statement {
            let node = raw(&tree, id)?;
            what = What::of(node);
            if self.block == Block::Failed && what != What::Exit {
                return Err(aborted());
            }
            match (what, node) {
                (What::Optimizable, _) => {
                    self.take_snapshot()?;
                    let query = analyze(&tree, id, &self.analysis(None), types)
                        .map_err(|e| refusal(e, sql))?;
                    if query.command == Command::Select {
                        columns = Some(fields(&query).map_err(|e| refusal(e, sql))?);
                    }
                    params = query.params;
                }
                (
                    _,
                    Node::VariableShowStmt {
                        name: Some(setting),
                    },
                ) => {
                    let (name, _) = self.shown(setting)?;
                    columns = Some(vec![setting_field(name)]);
                }
                _ => {}
            }
            if let Some(at) = params
                .iter()
                .position(|t| matches!(*t, 0 | type_oid::UNKNOWN))
            {
                return Err(PgError::error(
                    SqlState::new(amphora_pg_analysis::sqlstate::INDETERMINATE_DATATYPE),
                    format!(
                        "could not determine data type of parameter ${}",
                        at.saturating_add(1)
                    ),
                ));
            }
        }
        // The statement under its name, its text, its tree, and the strings in that, which are the text's.
        let held = [
            std::mem::size_of::<Prepared>(),
            name.len(),
            sql.len().saturating_mul(2),
            tree.bytes(),
            std::mem::size_of_val(params.as_slice()),
            columns.as_deref().map_or(0, field_bytes),
        ];
        let held = self.charge(held.into_iter().fold(0, usize::saturating_add))?;
        Ok(Prepared {
            sql: sql.to_owned(),
            tree,
            statement,
            what,
            params,
            fields: columns,
            _held: held,
        })
    }

    /// `FetchPreparedStatement` (src/backend/commands/prepare.c), and the unnamed statement.
    fn fetched(&self, name: &str) -> Result<Rc<Prepared>, PgError> {
        let (found, what) = if name.is_empty() {
            let what = "unnamed prepared statement does not exist".to_owned();
            (self.unnamed.as_ref(), what)
        } else {
            let what = format!("prepared statement \"{name}\" does not exist");
            (self.named.get(name), what)
        };
        found
            .map(Rc::clone)
            .ok_or_else(|| PgError::error(SqlState::UNDEFINED_PREPARED_STATEMENT, what))
    }

    /// `exec_bind_message`.
    pub(super) fn bind(
        &mut self,
        portal: &str,
        statement: &str,
        mut values: Values<'_>,
        out: &mut Vec<u8>,
    ) -> Result<(), PgError> {
        let prepared = self.fetched(statement)?;
        self.start_command()?;
        let violation = |text: String| PgError::error(SqlState::PROTOCOL_VIOLATION, text);
        let formats = values.formats()?;
        let count = values.count()?;
        if formats.given() > 1 && formats.given() != count {
            return Err(violation(format!(
                "bind message has {} parameter formats but {count} parameters",
                formats.given()
            )));
        }
        if count != prepared.params.len() {
            return Err(violation(format!(
                "bind message supplies {count} parameters, but prepared statement \"{statement}\" requires {}",
                prepared.params.len()
            )));
        }
        if self.block == Block::Failed && (prepared.what != What::Exit || count != 0) {
            return Err(aborted());
        }
        if !portal.is_empty() && self.portals.contains_key(portal) {
            return Err(PgError::error(
                SqlState::DUPLICATE_CURSOR,
                format!("cursor \"{portal}\" already exists"),
            ));
        }
        self.portals.insert(portal.to_owned(), Portal::undefined());
        if count > 0 || prepared.what == What::Optimizable {
            self.take_snapshot()?;
        }
        let mut params = Vec::new();
        for (at, type_oid) in prepared.params.iter().enumerate() {
            let number = at.saturating_add(1);
            let mut shown = None;
            let value = self
                .parameter(*type_oid, formats.of(at), number, &mut values, &mut shown)
                .map_err(|error| error.with_context(parameter_context(portal, number, shown)))?;
            params.push(value);
        }
        let result_formats = values.formats()?;
        values.end()?;
        let mut defined = self.define(portal, &prepared, &params)?;
        if let Some(columns) = &defined.fields {
            if result_formats.given() > 1 && result_formats.given() != columns.len() {
                return Err(violation(format!(
                    "bind message has {} result formats but query has {} columns",
                    result_formats.given(),
                    columns.len()
                )));
            }
            defined.formats = result_formats;
        }
        self.portals.insert(portal.to_owned(), defined);
        wire::message(out, b'2', |_| {})
    }

    /// A parameter's value: `shown` is its text as the error's context shows it, once it has one.
    fn parameter(
        &self,
        type_oid: u32,
        format: i16,
        number: usize,
        values: &mut Values<'_>,
        shown: &mut Option<String>,
    ) -> Result<Option<Value>, PgError> {
        let value = values.value()?;
        match (format, value) {
            (0, None) => Ok(None),
            (0, Some(bytes)) => {
                let given = text(bytes)?;
                *shown = Some(if given.is_empty() { "''" } else { "'...'" }.to_owned());
                let value =
                    input(type_oid, given, &self.analysis(None)).map_err(|e| refusal(e, ""))?;
                Ok(Some(value))
            }
            (1, None) => {
                binary::receivable(type_oid).map_err(|e| refusal(e, ""))?;
                Ok(None)
            }
            (1, Some(bytes)) => {
                let (value, read) = binary::receive(type_oid, bytes).map_err(|e| refusal(e, ""))?;
                if read != bytes.len() {
                    return Err(PgError::error(
                        SqlState::INVALID_BINARY_REPRESENTATION,
                        format!("incorrect binary data format in bind parameter {number}"),
                    ));
                }
                Ok(Some(value))
            }
            _ => Err(unsupported_format(format)),
        }
    }

    /// The portal of a statement over its parameters' values (`GetCachedPlan`, `PortalStart`).
    fn define(
        &self,
        name: &str,
        prepared: &Rc<Prepared>,
        params: &[Option<Value>],
    ) -> Result<Portal, PgError> {
        let mut held = 0usize;
        let run = match (prepared.what, prepared.statement) {
            (What::Empty, _) | (_, None) => Run::Empty,
            (What::Show, Some(id)) => match raw(&prepared.tree, id)? {
                Node::VariableShowStmt { name: Some(name) } => Run::Show(name.clone()),
                _ => Run::Utility(Rc::clone(prepared)),
            },
            (What::Utility | What::Exit, Some(_)) => Run::Utility(Rc::clone(prepared)),
            (What::Optimizable, Some(id)) => {
                let sql = prepared.sql.as_str();
                let session = self.analysis(Some(self.began));
                let query = analyze(&prepared.tree, id, &session, &prepared.params)
                    .map_err(|e| refusal(e, sql))?;
                let columns = match query.command {
                    Command::Select => Some(fields(&query).map_err(|e| refusal(e, sql))?),
                    Command::Insert | Command::Update => None,
                };
                if columns != prepared.fields {
                    return Err(PgError::error(
                        SqlState::FEATURE_NOT_SUPPORTED,
                        "cached plan must not change result type",
                    ));
                }
                match lower(&query, &session, params) {
                    Err(error) if error.runs => Run::Refused(refusal(error, sql)),
                    Err(error) => return Err(refusal(error, sql)),
                    Ok(lowered) if query.command == Command::Select => {
                        let mut statement = self.prepared(&lowered).map_err(engine)?;
                        let (rows, refused) = collect(&mut statement);
                        held = rows
                            .iter()
                            .map(|row| row_bytes(row))
                            .fold(0, usize::saturating_add);
                        Run::Rows {
                            rows: rows.into_iter(),
                            refused: refused.map(engine),
                            tag: Tag::Select,
                        }
                    }
                    Ok(lowered) => {
                        held = change_bytes(&lowered);
                        Run::Change(query.command, lowered)
                    }
                }
            }
        };
        // The portal under its name, its columns, and what it runs or sends.
        let held = [
            std::mem::size_of::<Portal>(),
            name.len(),
            prepared.fields.as_deref().map_or(0, field_bytes),
            held,
        ];
        Ok(Portal {
            fields: prepared.fields.clone(),
            formats: Formats::default(),
            run,
            _held: self.charge(held.into_iter().fold(0, usize::saturating_add))?,
        })
    }

    /// `exec_describe_statement_message` and `exec_describe_portal_message`.
    pub(super) fn describe(
        &mut self,
        object: Object<'_>,
        out: &mut Vec<u8>,
    ) -> Result<(), PgError> {
        self.start_command()?;
        match object {
            Object::Statement(name) => {
                let prepared = self.fetched(name)?;
                if self.block == Block::Failed && prepared.fields.is_some() {
                    return Err(aborted());
                }
                let count = u16::try_from(prepared.params.len()).map_err(|_| {
                    PgError::error(
                        SqlState::INTERNAL_ERROR,
                        "a statement has too many parameters",
                    )
                })?;
                wire::message(out, b't', |out| {
                    out.extend_from_slice(&count.to_be_bytes());
                    for type_oid in &prepared.params {
                        out.extend_from_slice(&type_oid.to_be_bytes());
                    }
                })?;
                described(out, prepared.fields.as_deref(), &Formats::default())
            }
            Object::Portal(name) => {
                let portal = self.portals.get(name).ok_or_else(|| no_portal(name))?;
                if self.block == Block::Failed && portal.fields.is_some() {
                    return Err(aborted());
                }
                described(out, portal.fields.as_deref(), &portal.formats)
            }
        }
    }

    /// Close (postgres.c:5009-5046): of a statement or portal there isn't is none.
    pub(super) fn close(&mut self, object: Object<'_>, out: &mut Vec<u8>) -> Result<(), PgError> {
        match object {
            Object::Statement("") => self.unnamed = None,
            Object::Statement(name) => {
                self.named.remove(name);
            }
            Object::Portal(name) => {
                self.portals.remove(name);
            }
        }
        wire::message(out, b'3', |_| {})
    }

    /// `exec_execute_message`.
    pub(super) fn execute(
        &mut self,
        name: &str,
        max_rows: i32,
        out: &mut Vec<u8>,
    ) -> Result<(), Stop> {
        let portal = self.portals.get(name).ok_or_else(|| no_portal(name))?;
        let exits = match &portal.run {
            Run::Empty => return Ok(wire::message(out, b'I', |_| {})?),
            Run::Utility(prepared) => prepared.what == What::Exit,
            _ => false,
        };
        self.start_command()?;
        if self.block == Block::Failed && !exits {
            return Err(aborted().into());
        }
        let Some(mut portal) = self.portals.remove(name) else {
            return Err(no_portal(name).into());
        };
        let ran = self.run_portal(name, &mut portal, max_rows, out);
        // A transaction's end takes its portals with it.
        if self.block != Block::None {
            self.portals.insert(name.to_owned(), portal);
        }
        if ran.is_ok() {
            self.pipelining = true;
        }
        ran
    }

    /// `PortalRun` (pquery.c): a SELECT's or SHOW's next rows, anything else once.
    fn run_portal(
        &mut self,
        name: &str,
        portal: &mut Portal,
        max_rows: i32,
        out: &mut Vec<u8>,
    ) -> Result<(), Stop> {
        if let Run::Show(setting) = &portal.run {
            let (_, value) = self.shown(setting)?;
            portal.run = Run::Rows {
                rows: vec![vec![Stored::build_text(value)]].into_iter(),
                refused: None,
                tag: Tag::Show,
            };
        }
        if let Run::Rows { .. } = portal.run {
            return Ok(self.send_rows(portal, max_rows, out)?);
        }
        match std::mem::replace(&mut portal.run, Run::Done) {
            Run::Change(command, lowered) => {
                self.ensure_transaction()?;
                self.take_snapshot()?;
                let running = self.begin_change(command, lowered.clone())?;
                let ran = self.change(running, out);
                if let Err(Stop::Wait { running: None, .. }) = &ran {
                    // Undone: to be run again, whole, once the wait has ended.
                    portal.run = Run::Change(command, lowered);
                }
                ran
            }
            Run::Refused(error) => Err(error.into()),
            Run::Utility(prepared) => match prepared.statement {
                Some(id) => self.statement(&prepared.sql, &prepared.tree, id, out),
                None => Ok(()),
            },
            Run::Empty | Run::Rows { .. } | Run::Show(_) | Run::Done => Err(PgError::error(
                SqlState::OBJECT_NOT_IN_PREREQUISITE_STATE,
                format!("portal \"{name}\" cannot be run"),
            )
            .into()),
        }
    }

    /// `PortalRunSelect` forward: DataRows, at most `max_rows` of them, and CommandComplete once there are
    /// fewer, else PortalSuspended.
    fn send_rows(
        &self,
        portal: &mut Portal,
        max_rows: i32,
        out: &mut Vec<u8>,
    ) -> Result<(), PgError> {
        let columns = portal.fields.as_deref().unwrap_or_default();
        let Run::Rows { rows, refused, tag } = &mut portal.run else {
            return Ok(());
        };
        let limit = usize::try_from(max_rows).ok().filter(|limit| *limit > 0);
        let mut sent = 0usize;
        while limit.is_none_or(|limit| sent < limit) {
            let Some(row) = rows.next() else {
                if let Some(error) = refused.take() {
                    return Err(error);
                }
                break;
            };
            self.data_row(columns, &portal.formats, &row, out)?;
            sent = sent.saturating_add(1);
        }
        match (limit, *tag) {
            (Some(limit), _) if sent == limit => wire::message(out, b's', |_| {}),
            (_, Tag::Select) => complete(out, &format!("SELECT {sent}")),
            (_, Tag::Show) => complete(out, "SHOW"),
        }
    }

    /// A DataRow of `row` in the columns' formats (`printtup`, src/backend/access/common/printtup.c).
    pub(super) fn data_row(
        &self,
        columns: &[Field],
        formats: &Formats,
        row: &[Stored],
        out: &mut Vec<u8>,
    ) -> Result<(), PgError> {
        for at in 0..columns.len() {
            if !matches!(formats.of(at), 0 | 1) {
                return Err(unsupported_format(formats.of(at)));
            }
        }
        let values = self.row(columns, formats, row)?;
        let count = i16::try_from(values.len())
            .map_err(|_| PgError::error(SqlState::INTERNAL_ERROR, "a row has too many columns"))?;
        wire::message(out, b'D', |out| {
            out.extend_from_slice(&count.to_be_bytes());
            for value in &values {
                match value {
                    None => out.extend_from_slice(&(-1i32).to_be_bytes()),
                    Some(bytes) => {
                        let len = i32::try_from(bytes.len()).unwrap_or(i32::MAX);
                        out.extend_from_slice(&len.to_be_bytes());
                        out.extend_from_slice(bytes);
                    }
                }
            }
        })
    }
}

/// A SHOW's column (`GetPGVariableResultDesc`, src/backend/utils/misc/guc_funcs.c).
pub(super) fn setting_field(name: String) -> Field {
    Field {
        name,
        table: 0,
        attnum: 0,
        type_oid: type_oid::TEXT,
        typmod: -1,
    }
}

/// RowDescription, or NoData for what gives no rows.
fn described(
    out: &mut Vec<u8>,
    columns: Option<&[Field]>,
    formats: &Formats,
) -> Result<(), PgError> {
    match columns {
        Some(columns) => super::execute::row_description(out, columns, formats),
        None => wire::message(out, b'n', |_| {}),
    }
}

fn unsupported_format(format: i16) -> PgError {
    PgError::error(
        SqlState::INVALID_PARAMETER_VALUE,
        format!("unsupported format code: {format}"),
    )
}

/// `bind_param_error_callback` (postgres.c:2596), a value shown as `log_parameter_max_length_on_error` at
/// its default of 0 leaves it.
fn parameter_context(portal: &str, number: usize, shown: Option<String>) -> String {
    let portal = if portal.is_empty() {
        "unnamed portal".to_owned()
    } else {
        format!("portal \"{portal}\"")
    };
    match shown {
        Some(shown) => format!("{portal} parameter ${number} = {shown}"),
        None => format!("{portal} parameter ${number}"),
    }
}
