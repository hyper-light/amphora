//! INSERT, UPDATE and DELETE on a view, which its INSTEAD OF triggers carry out
//! (sqlite.org/lang_createtrigger.html, section 3): no row is written, and the statement's triggers
//! run in the place of each row's write.
//!
//! As SQLite does, every row the statement is for is found before the first trigger runs: an
//! INSERT's source (insert.c, `useTempTable`), and the view's rows an UPDATE's or a DELETE's WHERE
//! keeps (`sqlite3MaterializeView`). An UPDATE evaluates its SET expressions row by row, among the
//! triggers; an UPDATE FROM, as it joins (update.c, `updateFromSelect`).
use crate::schema::{BTreeCharacteristics, BTreeTable, Table, Trigger, View};
use crate::sync::Arc;
use crate::translate::emitter::update::update_trigger_context;
use crate::translate::emitter::Resolver;
use crate::translate::expr::{
    bind_and_rewrite_expr, emit_returning_results, emit_returning_scan_back,
    process_returning_clause, restore_returning_row_image_in_cache,
    seed_returning_row_image_in_cache, translate_expr_no_constant_opt, BindingBehavior,
    NoConstantOptReason, ReturningBufferCtx,
};
use crate::translate::plan::{
    ColumnUsedMask, EvalAt, JoinedTable, NonFromClauseSubquery, Operation, OuterQueryReference,
    Plan, QueryDestination, ResultSetColumn, SubqueryEvalPhase, SubqueryOrigin, SubqueryRun,
    TableReferences, UpdateSetClause,
};
use crate::translate::planner::{
    collect_from_clause_table_refs, collect_subquery_table_refs_in_expr, plan_ctes_as_outer_refs,
    ROWID_STRS,
};
use crate::translate::select::{
    emit_select_plan, prepare_select_plan, refuse_unsafe_virtual_table,
};
use crate::translate::subquery::{
    emit_non_from_clause_subquery, plan_subqueries_from_returning, plan_subqueries_from_update_sets,
};
use crate::translate::trigger_exec::{fire_trigger, instead_of_triggers, TriggerContext};
use crate::translate::update::split_update_set_values;
use crate::translate::without_rowid::emit_write_subqueries;
use crate::util::normalize_ident;
use crate::vdbe::builder::{CursorType, ProgramBuilder};
use crate::vdbe::insn::{to_u32, InsertFlags, Insn};
use crate::vdbe::BranchOffset;
use crate::{bail_parse_error, Connection, LimboError, Result};
use amphora_engine_parser::ast::{
    self, Expr, InsertBody, OneSelect, QualifiedName, ResolveType, ResultColumn, TableInternalId,
    TriggerEvent, TriggerTime, With,
};

fn internal(message: &str) -> LimboError {
    LimboError::InternalError(message.into())
}

fn offset(start: usize, n: usize) -> Result<usize> {
    start
        .checked_add(n)
        .ok_or_else(|| internal("register range overflow"))
}

/// The view `name` is in a database where no table has the name.
pub(crate) fn view_named(
    resolver: &Resolver,
    database_id: usize,
    name: &QualifiedName,
) -> Option<Arc<View>> {
    resolver.with_schema(database_id, |schema| {
        match schema.get_table(name.name.as_str()) {
            Some(_) => None,
            None => schema.get_view(name.name.as_str()),
        }
    })
}

/// A statement's view: its columns as a query of it types them (`sqlite3ViewGetColumnNames`), with
/// no rowid to name, and the triggers that carry the statement out.
struct Target {
    table: Arc<BTreeTable>,
    database_id: usize,
    internal_id: TableInternalId,
    triggers: Vec<Arc<Trigger>>,
}

impl Target {
    fn new(
        view: &View,
        database_id: usize,
        event: TriggerEvent,
        set_columns: &[String],
        resolver: &Resolver,
        program: &mut ProgramBuilder,
        connection: &Arc<Connection>,
    ) -> Result<Self> {
        let table = view_table(view, database_id, resolver, program, connection)?;
        let triggers = instead_of_triggers(resolver, database_id, &view.name, event, set_columns);
        if triggers.is_empty() {
            bail_parse_error!("cannot modify {} because it is a view", view.name);
        }
        Ok(Self {
            table,
            database_id,
            internal_id: program.table_reference_counter.next(),
            triggers,
        })
    }

    fn columns(&self) -> usize {
        self.table.columns().len()
    }

    /// The view as a statement's expressions read it, called `identifier`.
    fn reference(&self, identifier: &str) -> TableReferences {
        let table = Table::BTree(self.table.clone());
        TableReferences::new(
            vec![JoinedTable {
                op: Operation::default_scan_for(&table),
                unmatched_right_rows_plan: None,
                table,
                identifier: normalize_ident(identifier),
                internal_id: self.internal_id,
                join_info: None,
                col_used_mask: ColumnUsedMask::default(),
                column_use_counts: Vec::new(),
                expression_index_usages: Vec::new(),
                database_id: self.database_id,
                indexed: None,
                plan_estimate: None,
            }],
            vec![],
        )
    }

    /// Opens the statement: it writes to the view's database, and a trigger may refuse a row after
    /// others ran (`sqlite3BeginWriteOperation`).
    fn begin(&self, resolver: &Resolver, program: &mut ProgramBuilder) -> Result<()> {
        let schema_cookie = resolver.with_schema(self.database_id, |s| s.schema_version);
        program.begin_write_on_database(self.database_id, schema_cookie)?;
        program.set_multi_write(true);
        program.set_may_abort(true);
        Ok(())
    }

    /// Fires the statement's triggers for the row in hand. RAISE(IGNORE) goes to `skip`.
    fn fire(
        &self,
        program: &mut ProgramBuilder,
        resolver: &mut Resolver,
        connection: &Arc<Connection>,
        context: &TriggerContext,
        skip: BranchOffset,
    ) -> Result<()> {
        for trigger in &self.triggers {
            fire_trigger(
                program,
                resolver,
                trigger.clone(),
                context,
                connection,
                self.database_id,
                skip,
            )?;
        }
        Ok(())
    }

    /// The registers of a row at `image` as a trigger names them: the columns, then the rowid's,
    /// which a trigger on a view can't name.
    fn trigger_row(&self, image: usize, no_rowid: usize) -> Result<Vec<usize>> {
        (0..self.columns())
            .map(|column| offset(image, column))
            .chain(std::iter::once(Ok(no_rowid)))
            .collect()
    }
}

/// The view as a table of its columns.
fn view_table(
    view: &View,
    database_id: usize,
    resolver: &Resolver,
    program: &mut ProgramBuilder,
    connection: &Arc<Connection>,
) -> Result<Arc<BTreeTable>> {
    let select = select_from(
        vec![ResultColumn::Star],
        view_in_from(view, database_id, None, resolver)?,
        None,
        None,
    );
    let plan = prepare_select_plan(
        select,
        resolver,
        program,
        &[],
        QueryDestination::placeholder_for_subquery(),
        connection,
    )?;
    let Plan::Select(plan) = &plan else {
        return Err(internal("a view's query is not a SELECT"));
    };
    let Some(Table::FromClauseSubquery(rows)) = plan
        .table_references
        .joined_tables()
        .first()
        .map(|t| &t.table)
    else {
        return Err(internal("a view is not read as a subquery"));
    };
    Ok(Arc::new(BTreeTable::new(
        0,
        view.name.clone(),
        vec![],
        rows.columns.to_vec(),
        BTreeCharacteristics::empty(),
        vec![],
        vec![],
        vec![],
        None,
    )))
}

/// The view as a FROM clause's table, named by its database, which no CTE takes the place of.
fn view_in_from(
    view: &View,
    database_id: usize,
    alias: Option<&ast::Name>,
    resolver: &Resolver,
) -> Result<ast::FromClause> {
    let database = resolver
        .get_database_name_by_index(database_id)
        .ok_or_else(|| internal("a view's database has no name"))?;
    Ok(ast::FromClause {
        select: Box::new(ast::SelectTable::Table(
            QualifiedName::fullname(
                ast::Name::exact(database),
                ast::Name::exact(view.name.clone()),
            ),
            alias.cloned().map(ast::As::As),
            None,
        )),
        joins: vec![],
    })
}

fn select_from(
    columns: Vec<ResultColumn>,
    from: ast::FromClause,
    where_clause: Option<Box<Expr>>,
    with: Option<With>,
) -> ast::Select {
    ast::Select {
        with,
        body: ast::SelectBody {
            select: OneSelect::Select {
                distinctness: None,
                columns,
                from: Some(from),
                where_clause,
                group_by: None,
                window_clause: vec![],
            },
            compounds: vec![],
        },
        order_by: vec![],
        limit: None,
    }
}

/// A statement's rows, stored before its first trigger runs.
struct Rows {
    cursor: usize,
    columns: usize,
}

impl Rows {
    /// Stores the rows of the query `prepare` plans for the destination it's given.
    fn store(
        program: &mut ProgramBuilder,
        resolver: &Resolver,
        connection: &Arc<Connection>,
        prepare: impl FnOnce(&mut ProgramBuilder, QueryDestination) -> Result<Plan>,
    ) -> Result<Self> {
        let yield_reg = program.alloc_register();
        let start = program.allocate_label();
        let end = program.allocate_label();
        let plan = prepare(
            program,
            QueryDestination::CoroutineYield {
                yield_reg,
                coroutine_implementation_start: start,
            },
        )?;
        refuse_unsafe_virtual_table(program, &plan)?;
        program.emit_insn(Insn::InitCoroutine {
            yield_reg,
            jump_on_definition: end,
            start_offset: start,
        });
        program.preassign_label_to_next_insn(start);
        let columns =
            program.nested(|program| emit_select_plan(plan, resolver, program, connection))?;
        program.emit_insn(Insn::EndCoroutine { yield_reg });
        program.preassign_label_to_next_insn(end);
        let row = match program.reg_result_cols_start {
            Some(row) => row,
            None => offset(yield_reg, 1)?,
        };
        let cursor = open_ephemeral(program, columns);
        let next = program.allocate_label();
        let done = program.allocate_label();
        program.preassign_label_to_next_insn(next);
        program.emit_insn(Insn::Yield {
            yield_reg,
            end_offset: done,
            subtype_clear_start_reg: 0,
            subtype_clear_count: 0,
        });
        let record = program.alloc_register();
        let rowid = program.alloc_register();
        program.emit_insn(Insn::MakeRecord {
            start_reg: to_u32(row),
            count: to_u32(columns),
            dest_reg: to_u32(record),
            index_name: None,
            affinity_str: None,
        });
        program.emit_insn(Insn::NewRowid {
            cursor,
            rowid_reg: rowid,
            prev_largest_reg: 0,
        });
        program.emit_insn(Insn::Insert {
            cursor,
            key_reg: rowid,
            record_reg: record,
            flag: InsertFlags::new()
                .require_seek()
                .is_ephemeral_table_insert(),
            table_name: String::new(),
        });
        program.emit_insn(Insn::Goto { target_pc: next });
        program.preassign_label_to_next_insn(done);
        Ok(Self { cursor, columns })
    }

    /// Reads column `column` of the row in hand.
    fn column(&self, program: &mut ProgramBuilder, column: usize, dest: usize) {
        program.emit_insn(Insn::Column {
            cursor_id: self.cursor,
            column,
            dest,
            default: None,
        });
    }

    /// Emits `body` for each row. It's given the label a row ends at.
    fn each(
        &self,
        program: &mut ProgramBuilder,
        body: impl FnOnce(&mut ProgramBuilder, BranchOffset) -> Result<()>,
    ) -> Result<()> {
        let start = program.allocate_label();
        let next = program.allocate_label();
        let end = program.allocate_label();
        program.emit_insn(Insn::Rewind {
            cursor_id: self.cursor,
            pc_if_empty: end,
        });
        program.preassign_label_to_next_insn(start);
        body(program, next)?;
        program.preassign_label_to_next_insn(next);
        program.emit_insn(Insn::Next {
            cursor_id: self.cursor,
            pc_if_next: start,
            fullscan: false,
            is_index: false,
        });
        program.preassign_label_to_next_insn(end);
        Ok(())
    }
}

/// An ephemeral table of `columns` columns, which keep their values as they are.
fn open_ephemeral(program: &mut ProgramBuilder, columns: usize) -> usize {
    let table = BTreeTable::new(
        0,
        "view_rows".into(),
        vec![],
        (0..columns)
            .map(|_| crate::schema::Column::new_default_text(None, String::new(), None))
            .collect(),
        BTreeCharacteristics::HAS_ROWID,
        vec![],
        vec![],
        vec![],
        None,
    );
    let cursor = program.alloc_cursor_id(CursorType::BTreeTable(Arc::new(table)));
    program.emit_insn(Insn::OpenEphemeral {
        cursor_id: cursor,
        is_table: true,
    });
    cursor
}

/// A statement's RETURNING, whose columns are the row's as the triggers were given it.
struct Returning {
    refs: TableReferences,
    columns: Vec<ResultSetColumn>,
    subqueries: Vec<NonFromClauseSubquery>,
    /// A subquery reads the view, so it's evaluated for each row
    /// (`sqlite3ProcessReturningSubqueries`).
    reads_view: bool,
    buffer: Option<ReturningBufferCtx>,
}

impl Returning {
    fn plan(
        target: &Target,
        mut returning: Vec<ResultColumn>,
        with: Option<With>,
        resolver: &Resolver,
        program: &mut ProgramBuilder,
        connection: &Arc<Connection>,
    ) -> Result<Self> {
        // SQLite resolves RETURNING by the view's name alone, never an alias.
        let mut refs = target.reference(&target.table.name);
        let mut reads = Vec::new();
        for column in &returning {
            if let ResultColumn::Expr(expr, _) = column {
                collect_subquery_table_refs_in_expr(expr, &mut reads);
            }
        }
        plan_ctes_as_outer_refs(with, &reads, resolver, program, &mut refs, connection)?;
        let mut subqueries = Vec::new();
        plan_subqueries_from_returning(
            program,
            &mut subqueries,
            &mut refs,
            &mut returning,
            resolver,
            connection,
        )?;
        let columns = process_returning_clause(&mut returning, &mut refs, resolver)?;
        Ok(Self {
            refs,
            columns,
            subqueries,
            reads_view: reads
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&target.table.name)),
            buffer: None,
        })
    }

    /// Opens where the rows wait for the statement's end (lang_returning.html, section 2.1).
    fn open(&mut self, program: &mut ProgramBuilder) {
        if !self.columns.is_empty() {
            self.buffer = Some(ReturningBufferCtx {
                cursor_id: open_ephemeral(program, self.columns.len()),
                num_columns: self.columns.len(),
            });
        }
    }

    fn row(
        &mut self,
        target: &Target,
        image: usize,
        no_rowid: usize,
        program: &mut ProgramBuilder,
        resolver: &mut Resolver,
    ) -> Result<()> {
        if self.columns.is_empty() {
            return Ok(());
        }
        let layout = target.table.column_layout()?;
        let cache = seed_returning_row_image_in_cache(
            program, &self.refs, image, no_rowid, resolver, &layout,
        )?;
        for subquery in self
            .subqueries
            .iter_mut()
            .filter(|subquery| !subquery.has_been_evaluated())
        {
            let run = if subquery.correlated || self.reads_view {
                SubqueryRun::EveryRead
            } else {
                SubqueryRun::Once
            };
            let plan = subquery.consume_plan(EvalAt::Loop(0));
            emit_non_from_clause_subquery(
                program,
                resolver,
                subquery.internal_id,
                *plan,
                &subquery.query_type,
                run,
                true,
            )?;
        }
        restore_returning_row_image_in_cache(resolver, cache);
        emit_returning_results(
            program,
            &self.refs,
            &self.columns,
            image,
            no_rowid,
            resolver,
            self.buffer.as_ref(),
            &layout,
        )
    }

    fn finish(self, program: &mut ProgramBuilder) {
        if let Some(buffer) = &self.buffer {
            program.emit_insn(Insn::FkCheck { deferred: false });
            emit_returning_scan_back(program, buffer);
        }
        program.result_columns = self.columns;
        program.table_references.extend(self.refs);
    }
}

/// What `PRAGMA count_changes` counts: the rows the triggers fired for (lang_createtrigger.html,
/// section 3), which `changes()` doesn't.
struct RowCount(Option<usize>);

impl RowCount {
    fn open(program: &mut ProgramBuilder, connection: &Arc<Connection>) -> Self {
        Self(connection.get_count_changes().then(|| {
            let register = program.alloc_register();
            program.emit_insn(Insn::Integer {
                value: 0,
                dest: register,
            });
            program.view_row_count = Some(register);
            register
        }))
    }

    fn row(&self, program: &mut ProgramBuilder) {
        if let Some(register) = self.0 {
            program.emit_insn(Insn::AddImm { register, value: 1 });
        }
    }
}

/// A column of NULLs no statement can read: the rowid a view hasn't.
fn no_rowid(program: &mut ProgramBuilder) -> usize {
    let register = program.alloc_register();
    program.emit_null(register, None);
    register
}

fn names_rowid(name: &str) -> bool {
    ROWID_STRS.iter().any(|s| s.eq_ignore_ascii_case(name))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn translate_insert(
    view: &View,
    database_id: usize,
    or_conflict: Option<ResolveType>,
    name: &QualifiedName,
    columns: &[ast::Name],
    mut body: InsertBody,
    returning: Vec<ResultColumn>,
    with: Option<With>,
    resolver: &Resolver,
    program: &mut ProgramBuilder,
    connection: &Arc<Connection>,
) -> Result<()> {
    let target = Target::new(
        view,
        database_id,
        TriggerEvent::Insert,
        &[],
        resolver,
        program,
        connection,
    )?;
    target.begin(resolver, program)?;
    program
        .flags
        .set_has_statement_conflict(or_conflict.is_some());
    let shown = shown_name(name);
    // Where each of the view's columns, and the rowid, is in the source. A column named twice takes
    // the first of its values, and the rowid the last (insert.c, `aTabColMap` and `ipkColumn`).
    let mut source: Vec<Option<usize>> = vec![None; target.columns()];
    let mut rowid = None;
    for (position, column) in columns.iter().enumerate() {
        let column = column.as_str();
        match target.table.get_column(column) {
            Some((index, _)) => {
                if let Some(slot) = source.get_mut(index) {
                    slot.get_or_insert(position);
                }
            }
            None if names_rowid(column) => rowid = Some(position),
            None => bail_parse_error!("table {shown} has no column named {column}"),
        }
    }
    if columns.is_empty() {
        for (position, slot) in source.iter_mut().enumerate() {
            *slot = Some(position);
        }
    }
    let rows = match &mut body {
        InsertBody::DefaultValues => None,
        InsertBody::Select(select, upsert) => {
            let mut select = select.clone();
            let upserts = upsert.is_some();
            resolve_defaults(&mut select);
            let mut around = TableReferences::new_empty();
            match (&select.with, with.clone()) {
                (Some(_), Some(with)) => {
                    let mut reads = Vec::new();
                    collect_from_clause_table_refs(&select, &mut reads);
                    plan_ctes_as_outer_refs(
                        Some(with),
                        &reads,
                        resolver,
                        program,
                        &mut around,
                        connection,
                    )?;
                }
                (None, with) => select.with = with,
                (Some(_), None) => {}
            }
            let rows = store_source(
                select,
                around.outer_query_refs(),
                resolver,
                program,
                connection,
            )?;
            if columns.is_empty() && rows.columns != target.columns() {
                bail_parse_error!(
                    "table {shown} has {} columns but {} values were supplied",
                    target.columns(),
                    rows.columns
                );
            }
            if !columns.is_empty() && rows.columns != columns.len() {
                bail_parse_error!("{} values for {} columns", rows.columns, columns.len());
            }
            if upserts {
                bail_parse_error!("cannot UPSERT a view");
            }
            Some(rows)
        }
    };
    let mut resolver = resolver.fork();
    let mut returning = Returning::plan(&target, returning, with, &resolver, program, connection)?;
    returning.open(program);
    let count = RowCount::open(program, connection);
    let no_rowid = no_rowid(program);
    let image = program.alloc_registers(target.columns());
    let mut insert_row = |program: &mut ProgramBuilder, skip: BranchOffset| -> Result<()> {
        if let (Some(rows), Some(position)) = (&rows, rowid) {
            let register = program.alloc_register();
            rows.column(program, position, register);
            let null = program.allocate_label();
            program.emit_insn(Insn::IsNull {
                reg: register,
                target_pc: null,
            });
            program.emit_insn(Insn::MustBeInt {
                reg: register,
                target_pc: None,
            });
            program.preassign_label_to_next_insn(null);
        }
        for (column, source) in source.iter().enumerate() {
            let dest = offset(image, column)?;
            match (&rows, source) {
                (Some(rows), Some(position)) => rows.column(program, *position, dest),
                _ => program.emit_null(dest, None),
            }
        }
        // SQLite converts no value of an INSERT on a view (insert.c, `if( !isView )`).
        let context = update_trigger_context(
            program,
            &target.table,
            Some(target.trigger_row(image, no_rowid)?),
            None,
            or_conflict.unwrap_or(ResolveType::Abort),
            TriggerTime::Before,
        );
        target.fire(program, &mut resolver, connection, &context, skip)?;
        count.row(program);
        returning.row(&target, image, no_rowid, program, &mut resolver)
    };
    match &rows {
        Some(rows) => rows.each(program, insert_row)?,
        None => {
            let end = program.allocate_label();
            insert_row(program, end)?;
            program.preassign_label_to_next_insn(end);
        }
    }
    returning.finish(program);
    Ok(())
}

/// The target of an INSERT as its errors name it: its alias, or its name as it was written.
fn shown_name(name: &QualifiedName) -> String {
    match &name.alias {
        Some(alias) => alias.as_str().to_string(),
        None => crate::util::table_name_for_error(name),
    }
}

/// A view's columns have no default, so DEFAULT is NULL.
fn resolve_defaults(select: &mut ast::Select) {
    let arms = std::iter::once(&mut select.body.select).chain(
        select
            .body
            .compounds
            .iter_mut()
            .map(|compound| &mut compound.select),
    );
    for arm in arms {
        if let OneSelect::Values(rows) = arm {
            for value in rows.iter_mut().flatten() {
                if matches!(value.as_ref(), Expr::Default) {
                    **value = Expr::Literal(ast::Literal::Null);
                }
            }
        }
    }
}

/// Stores an INSERT's source.
fn store_source(
    select: ast::Select,
    around: &[OuterQueryReference],
    resolver: &Resolver,
    program: &mut ProgramBuilder,
    connection: &Arc<Connection>,
) -> Result<Rows> {
    Rows::store(program, resolver, connection, |program, destination| {
        prepare_select_plan(select, resolver, program, around, destination, connection)
    })
}

/// What an UPDATE sets: a column of the view, or (`None`) its rowid, which must be an integer and
/// goes nowhere.
struct SetTarget {
    column: Option<usize>,
    expr: Box<Expr>,
}

/// The columns `sets` names, as `UPDATE OF` triggers are asked by.
fn set_names(sets: &[ast::Set]) -> Vec<String> {
    sets.iter()
        .flat_map(|set| &set.col_names)
        .map(|name| normalize_ident(name.as_str()))
        .collect()
}

/// The target of each of the names `sets` assigns to.
fn set_columns(table: &BTreeTable, sets: &[ast::Set]) -> Result<Vec<Vec<Option<usize>>>> {
    sets.iter()
        .map(|set| {
            set.col_names
                .iter()
                .map(|name| match table.get_column(name.as_str()) {
                    Some((index, _)) => Ok(Some(index)),
                    None if names_rowid(name.as_str()) => Ok(None),
                    None => bail_parse_error!("no such column: {}", name.as_str()),
                })
                .collect()
        })
        .collect()
}

/// One expression for each target, in the order SQLite evaluates them: the rowid's, then the
/// columns' as the view has them. A target set twice takes the last.
fn set_targets(targets: Vec<Vec<Option<usize>>>, sets: Vec<ast::Set>) -> Result<Vec<SetTarget>> {
    let mut assigned: Vec<SetTarget> = Vec::new();
    for (columns, set) in targets.into_iter().zip(sets) {
        let values = split_update_set_values(*set.expr, columns.len())?;
        for (column, expr) in columns.into_iter().zip(values) {
            assigned.retain(|target| target.column != column);
            assigned.push(SetTarget {
                column,
                expr: Box::new(expr),
            });
        }
    }
    assigned.sort_by_key(|target| target.column);
    Ok(assigned)
}

pub(crate) fn translate_update(
    view: &View,
    database_id: usize,
    update: ast::Update,
    resolver: &Resolver,
    program: &mut ProgramBuilder,
    connection: &Arc<Connection>,
) -> Result<()> {
    let ast::Update {
        with,
        or_conflict,
        tbl_name: name,
        indexed,
        mut sets,
        from,
        where_clause,
        returning,
    } = update;
    let target = Target::new(
        view,
        database_id,
        TriggerEvent::Update,
        &set_names(&sets),
        resolver,
        program,
        connection,
    )?;
    refuse_index(&indexed)?;
    let targets = set_columns(&target.table, &sets)?;
    target.begin(resolver, program)?;
    program
        .flags
        .set_has_statement_conflict(or_conflict.is_some());
    let mut resolver = resolver.fork();
    let alias = name.alias.as_ref();
    let mut refs = target.reference(alias.unwrap_or(&name.name).as_str());
    let mut set_subqueries = Vec::new();
    // Where an UPDATE FROM's rows have the values it sets: after the view's columns.
    let joined = from.is_some();
    let (rows, sets) = match from {
        Some(from) => update_from_rows(
            &target,
            view,
            alias,
            from,
            where_clause,
            with.clone(),
            targets,
            sets,
            &resolver,
            program,
            connection,
        )?,
        None => {
            let rows = view_rows(
                &target,
                view,
                alias,
                where_clause,
                with.clone(),
                &resolver,
                program,
                connection,
            )?;
            let mut reads = Vec::new();
            for set in &sets {
                collect_subquery_table_refs_in_expr(&set.expr, &mut reads);
            }
            plan_ctes_as_outer_refs(
                with.clone(),
                &reads,
                &resolver,
                program,
                &mut refs,
                connection,
            )?;
            for set in &mut sets {
                bind_and_rewrite_expr(
                    &mut set.expr,
                    Some(&mut refs),
                    None,
                    &resolver,
                    BindingBehavior::ResultColumnsNotAllowed,
                )?;
            }
            plan_subqueries_from_update_sets(
                program,
                &mut set_subqueries,
                &mut refs,
                &mut sets,
                &resolver,
                connection,
            )?;
            (rows, set_targets(targets, sets)?)
        }
    };
    let mut returning = Returning::plan(&target, returning, with, &resolver, program, connection)?;
    returning.open(program);
    let count = RowCount::open(program, connection);
    let no_rowid = no_rowid(program);
    let old = program.alloc_registers(target.columns());
    let new = program.alloc_registers(target.columns());
    let rowid = program.alloc_register();
    let layout = target.table.column_layout()?;
    rows.each(program, |program, skip| {
        for column in 0..target.columns() {
            let dest = offset(old, column)?;
            rows.column(program, column, dest);
            program.emit_insn(Insn::Copy {
                src_reg: dest,
                dst_reg: offset(new, column)?,
                extra_amount: 0,
            });
        }
        let cache = seed_returning_row_image_in_cache(
            program,
            &refs,
            old,
            no_rowid,
            &mut resolver,
            &layout,
        )?;
        emit_write_subqueries(
            program,
            &resolver,
            &mut set_subqueries,
            false,
            target.database_id,
            &target.table.name,
        )?;
        for (position, set) in sets.iter().enumerate() {
            let dest = match set.column {
                Some(column) => offset(new, column)?,
                None => rowid,
            };
            if joined {
                rows.column(program, offset(target.columns(), position)?, dest);
            } else {
                translate_expr_no_constant_opt(
                    program,
                    Some(&refs),
                    &set.expr,
                    dest,
                    &resolver,
                    NoConstantOptReason::RegisterReuse,
                )?;
            }
            if set.column.is_none() {
                program.emit_insn(Insn::MustBeInt {
                    reg: rowid,
                    target_pc: None,
                });
            }
        }
        restore_returning_row_image_in_cache(&mut resolver, cache);
        for (index, column) in target.table.columns().iter().enumerate() {
            program.emit_column_affinity(offset(new, index)?, column.affinity());
        }
        let context = update_trigger_context(
            program,
            &target.table,
            Some(target.trigger_row(new, no_rowid)?),
            Some(target.trigger_row(old, no_rowid)?),
            or_conflict.unwrap_or(ResolveType::Abort),
            TriggerTime::Before,
        );
        target.fire(program, &mut resolver, connection, &context, skip)?;
        count.row(program);
        returning.row(&target, new, no_rowid, program, &mut resolver)
    })?;
    returning.finish(program);
    Ok(())
}

/// A view has no index to be read by.
fn refuse_index(indexed: &Option<ast::Indexed>) -> Result<()> {
    if let Some(ast::Indexed::IndexedBy(index)) = indexed {
        bail_parse_error!("no such index: {}", index.as_str());
    }
    Ok(())
}

/// Stores the view's rows `where_clause` keeps.
#[allow(clippy::too_many_arguments)]
fn view_rows(
    target: &Target,
    view: &View,
    alias: Option<&ast::Name>,
    where_clause: Option<Box<Expr>>,
    with: Option<With>,
    resolver: &Resolver,
    program: &mut ProgramBuilder,
    connection: &Arc<Connection>,
) -> Result<Rows> {
    let select = select_from(
        vec![ResultColumn::Star],
        view_in_from(view, target.database_id, alias, resolver)?,
        where_clause,
        with,
    );
    store_source(select, &[], resolver, program, connection)
}

/// Stores the rows of an UPDATE FROM: the view's columns, then the values it sets. Each row of the
/// join is a row to update (update.c, `updateFromSelect`: a view's rows have no key to be told by).
#[allow(clippy::too_many_arguments)]
fn update_from_rows(
    target: &Target,
    view: &View,
    alias: Option<&ast::Name>,
    from: ast::FromClause,
    where_clause: Option<Box<Expr>>,
    with: Option<With>,
    targets: Vec<Vec<Option<usize>>>,
    mut sets: Vec<ast::Set>,
    resolver: &Resolver,
    program: &mut ProgramBuilder,
    connection: &Arc<Connection>,
) -> Result<(Rows, Vec<SetTarget>)> {
    let called = alias.map_or(view.name.as_str(), |alias| alias.as_str());
    refuse_target_in_from(target, called, &from, resolver)?;
    let mut tables = view_in_from(view, target.database_id, alias, resolver)?;
    tables.joins.push(ast::JoinedSelectTable {
        operator: ast::JoinOperator::Comma,
        table: from.select,
        constraint: None,
    });
    tables.joins.extend(from.joins);
    let select = select_from(
        vec![ResultColumn::TableStar(ast::Name::exact(
            called.to_string(),
        ))],
        tables,
        where_clause,
        with,
    );
    let mut assigned = Vec::new();
    let rows = Rows::store(program, resolver, connection, |program, destination| {
        let mut plan =
            prepare_select_plan(select, resolver, program, &[], destination, connection)?;
        let Plan::Select(select) = &mut plan else {
            return Err(internal("an UPDATE FROM's join is not a SELECT"));
        };
        for set in &mut sets {
            bind_and_rewrite_expr(
                &mut set.expr,
                Some(&mut select.table_references),
                None,
                resolver,
                BindingBehavior::ResultColumnsNotAllowed,
            )?;
        }
        plan_subqueries_from_update_sets(
            program,
            &mut select.non_from_clause_subqueries,
            &mut select.table_references,
            &mut sets,
            resolver,
            connection,
        )?;
        for subquery in &mut select.non_from_clause_subqueries {
            if subquery.origin == SubqueryOrigin::DmlSet {
                subquery.eval_phase = SubqueryEvalPhase::BeforeLoop;
            }
        }
        assigned = set_targets(targets, sets)?;
        select
            .result_columns
            .extend(assigned.iter().map(|set| ResultSetColumn {
                expr: set.expr.as_ref().clone(),
                alias: None,
                implicit_column_name: None,
                contains_aggregates: false,
            }));
        Ok(plan)
    })?;
    Ok((rows, assigned))
}

/// An UPDATE's target can't be in its FROM by the same name.
fn refuse_target_in_from(
    target: &Target,
    called: &str,
    from: &ast::FromClause,
    resolver: &Resolver,
) -> Result<()> {
    let tables =
        std::iter::once(from.select.as_ref()).chain(from.joins.iter().map(|j| j.table.as_ref()));
    for table in tables {
        if let ast::SelectTable::Table(name, alias, _) = table {
            let shown = alias.as_ref().map_or(&name.name, |alias| alias.name());
            if shown.as_str().eq_ignore_ascii_case(called)
                && name.name.as_str().eq_ignore_ascii_case(&target.table.name)
                && resolver.resolve_existing_table_database_id_qualified(name)?
                    == target.database_id
            {
                bail_parse_error!("target object/alias may not appear in FROM clause: {called}");
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn translate_delete(
    view: &View,
    database_id: usize,
    name: &QualifiedName,
    where_clause: Option<Box<Expr>>,
    returning: Vec<ResultColumn>,
    indexed: &Option<ast::Indexed>,
    with: Option<With>,
    resolver: &Resolver,
    program: &mut ProgramBuilder,
    connection: &Arc<Connection>,
) -> Result<()> {
    let target = Target::new(
        view,
        database_id,
        TriggerEvent::Delete,
        &[],
        resolver,
        program,
        connection,
    )?;
    refuse_index(indexed)?;
    target.begin(resolver, program)?;
    let mut resolver = resolver.fork();
    let rows = view_rows(
        &target,
        view,
        name.alias.as_ref(),
        where_clause,
        with.clone(),
        &resolver,
        program,
        connection,
    )?;
    let mut returning = Returning::plan(&target, returning, with, &resolver, program, connection)?;
    returning.open(program);
    let count = RowCount::open(program, connection);
    let no_rowid = no_rowid(program);
    let old = program.alloc_registers(target.columns());
    rows.each(program, |program, skip| {
        for column in 0..target.columns() {
            rows.column(program, column, offset(old, column)?);
        }
        // SQLite codes a DELETE's triggers with OE_Default (delete.c): no conflict clause of an
        // enclosing statement reaches theirs.
        let context = TriggerContext::new(
            target.table.clone(),
            None,
            Some(target.trigger_row(old, no_rowid)?),
        );
        target.fire(program, &mut resolver, connection, &context, skip)?;
        count.row(program);
        returning.row(&target, old, no_rowid, program, &mut resolver)
    })?;
    returning.finish(program);
    Ok(())
}
