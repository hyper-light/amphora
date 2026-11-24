//! Amphora (QT.6, the query-speed plan's step 6): the VDBE's hook for morsel-driven parallel
//! execution. A top-level SELECT whose whole work is one table scan with its filter and
//! aggregates is handed to `Insn::MorselAggregate`, which reads the table's leaf pages in
//! morsels, a cursor per worker (`vdbe/morsel_scan.rs`, docs/design/executor.md section 8). The
//! statement's serial loop is still emitted after it and runs when the parallel run declines a
//! value, so the answer never depends on the switch.
//!
//! Only the shapes whose answers equal the serial loop's value for value qualify: one rowid
//! table read whole or by a rowid range between integer literals or bound parameters (lane R11),
//! no index, every column it reads stored as written (INTEGER, TEXT or BLOB affinity, BINARY
//! collation, no generated column, a short row padded with its defaults); terms comparing a
//! column with an integer or text literal of its own kind, or testing it for NULL; plain-column
//! GROUP BY keys; COUNT, SUM and AVG over INTEGER or REAL columns (lane R11), MIN and MAX (and in
//! the loop body's copy, AVG and TOTAL over integers); no HAVING, ORDER BY, LIMIT, DISTINCT,
//! window or subquery.
//!
//! A join's probe with its aggregates qualifies too (`shape_join`): two or more rowid tables the
//! planner joins inner, by hash joins and scans, keyed by equalities of columns of one affinity;
//! every other term and every output run in a copy of the serial loop's body over each joined row
//! (lane qs6-12). Each table but the probe is a link of a chain of hash tables (lane qs6-10), read and
//! built once, by the workers together (each scans its own build leaves and scatters its rows by
//! partition; each partition is filled by one worker); each worker probes over its own run of the
//! probe table's leaves, each probe row through every link in turn, and folds the joined rows into
//! its partial aggregates. Such a join with no aggregates returns its rows instead: each worker
//! keeps its joined rows, sorted when there is an ORDER BY over the tables' columns, and the runs
//! merge in leaf order, stably, so the rows come out as the serial loop gives them.

use crate::function::AggFunc;
use crate::schema::{BTreeTable, Table};
use crate::translate::collate::CollationSeq;
use crate::translate::emitter::Resolver;
use crate::translate::expr::{
    translate_condition_expr, translate_expr, walk_expr, walk_expr_mut, ConditionMetadata,
    WalkControl,
};
use crate::translate::plan::{
    Distinctness, HashJoinType, IterationDirection, JoinType, JoinedTable, Operation,
    QueryDestination, Scan, Search, SeekDef, SeekKeyComponent, SelectPlan, WhereTermOrigin,
};
use crate::types::{KeyInfo, SeekOp, Value};
use crate::vdbe::affinity::Affinity;
use crate::vdbe::builder::ProgramBuilder;
use crate::vdbe::insn::{
    Insn, MorselAggregateData, MorselBody, MorselJoinData, MorselKeeps, MorselOuter, MorselRows,
};
use crate::vdbe::morsel::{Aggregate, CmpOp, Predicate};
use amphora_engine_parser::ast::{
    Expr, Literal, Operator, SortOrder, TableInternalId, UnaryOperator,
};

/// The labels the hook's code jumps to: the serial loop it falls back to, and the statement's end.
pub(crate) struct MorselHook {
    pub end: crate::vdbe::BranchOffset,
}

/// Emits the parallel run and its row loop ahead of the serial loop when `plan` qualifies.
pub(crate) fn emit_morsel_hook(
    program: &mut ProgramBuilder,
    plan: &SelectPlan,
    resolver: &Resolver,
) -> crate::Result<Option<MorselHook>> {
    let (Some(window), Some(group_order)) = (window(plan), group_order(plan)) else {
        return Ok(None);
    };
    // A one-table scan runs a copy of its loop body in the workers (lane qs6-7), and so does a
    // join, over each joined row (lane qs6-12); the simple shape's own filter and aggregates stay
    // for the versioned storage's column copy.
    let simple = shape(plan);
    let is_simple = simple.is_some();
    // Whether a table the planner reads by a seek may be a link; how it is read (a hash table,
    // or a seek per joined row) is the executor's (lane qs6-16).
    let searched_links = resolver.searched_links() != crate::SearchedLinks::Off;
    let (data, looped) = match (body_plan(plan), simple) {
        (looped, Some(data)) => (data, looped),
        (Some(looped), None) => {
            let data = MorselAggregateData {
                root_page: looped.table.root_page,
                columns: looped.table.columns().len(),
                rowid_alias: rowid_alias(looped.table),
                reals: Vec::new(),
                defaults: defaults(looped.table),
                rowid_range: looped.rowid_range,
                filter: Vec::new(),
                group_by: looped.group_by.clone(),
                group_order: Vec::new(),
                aggregates: looped.aggregates.clone(),
                result_map: looped.result_map.clone(),
                joins: Vec::new(),
                // A scan's ORDER BY, sorted by the workers as a join's rows are (lane qs6-17).
                joined_rows: looped.sorted.clone(),
                body: None,
                outer: None,
                target_pc_serial: crate::vdbe::BranchOffset::Placeholder,
            };
            (data, Some(looped))
        }
        // Which table a two-table outer join probes (lane qs6-15): `shape_join` takes the padded
        // one, `shape_chain` the kept one, as the connection's switch says.
        // A table the planner reads by a seek is a hash link when the switch says so (lane
        // qs6-16).
        (None, None) => match shape_join(plan, resolver.outer_probe(), searched_links)
            .or_else(|| shape_chain(plan, resolver.outer_probe(), searched_links))
        {
            Some((data, looped)) => (data, Some(looped)),
            None => return Ok(None),
        },
    };
    // On the versioned storage the hook is the column copy's, which only a table declared
    // COLUMNAR keeps (docs/design/columnar.md, decision 1, item 1).
    if resolver.morsel_hook() == crate::translate::emitter::MorselHookFor::Copies
        && !declared(plan, data.root_page)
    {
        return Ok(None);
    }
    let body = looped.as_ref().map(|looped| MorselBody {
        on: (!looped.on.is_empty()).then(|| program.allocate_label()),
        links_on: looped
            .links_on
            .iter()
            .map(|on| {
                (!on.is_empty()).then(|| (program.allocate_label(), program.allocate_label()))
            })
            .collect(),
        start: program.allocate_label(),
        keep: program.allocate_label(),
        skip: program.allocate_label(),
        columns_start: program.alloc_registers(looped.width),
        reals: real_places(&looped.places),
        rowid: program.alloc_register(),
        out_start: program.alloc_registers(looped.outs.len()),
        out_count: looped.outs.len(),
        group_by: looped.group_by.clone(),
        aggregates: looped.aggregates.clone(),
        no_real: looped.no_real.clone(),
        simple: is_simple,
        rows: looped.rows,
        // A join's LIMIT cuts rows the workers all produce (its ORDER BY tells every row apart),
        // and so does a sorted scan's (lane qs6-17), so only an unsorted scan's rows stop early.
        want: if looped.rows && data.joins.is_empty() && looped.sorted.is_none() {
            window
                .limit
                .map(|limit| limit.saturating_add(window.offset))
                .and_then(|want| u64::try_from(want).ok())
        } else {
            None
        },
        direct: if data.joins.is_empty() {
            None
        } else {
            direct_reads(looped)
        },
    });
    let serial = program.allocate_label();
    let end = program.allocate_label();
    let next = program.allocate_label();
    let shown = plan.result_columns.len();
    // A HAVING (lane qs6-17): every aggregate's value is loaded after the result columns, for the
    // row loop to test its terms on.
    let having = plan.group_by.as_ref().is_some_and(|g| g.having.is_some());
    let mut data = data;
    let count = if having {
        let keys = data.group_by.len();
        for at in 0..plan.aggregates.len() {
            data.result_map.push(keys.checked_add(at).ok_or_else(|| {
                crate::LimboError::InternalError("a HAVING's place overflows".into())
            })?);
        }
        shown.saturating_add(plan.aggregates.len())
    } else {
        shown
    };
    let dest_start = program.alloc_registers(count);
    let having_terms = if having {
        let first = dest_start.checked_add(shown).ok_or_else(|| {
            crate::LimboError::InternalError("a HAVING's register overflows".into())
        })?;
        // `body_outputs` took the HAVING only where its terms qualify.
        having_terms(plan, first).ok_or_else(|| {
            crate::LimboError::InternalError("a HAVING the hook took doesn't qualify".into())
        })?
    } else {
        Vec::new()
    };
    // A LIMIT and an OFFSET count the hook's rows down as the serial loop's do (QT.6, lane qs6-6).
    let offset = (window.offset > 0).then(|| {
        let reg = program.alloc_register();
        program.emit_insn(Insn::Integer {
            value: window.offset,
            dest: reg,
        });
        reg
    });
    let limit = window.limit.map(|value| {
        let reg = program.alloc_register();
        program.emit_insn(Insn::Integer { value, dest: reg });
        reg
    });
    program.emit_insn(Insn::MorselAggregate {
        data: Box::new(MorselAggregateData {
            target_pc_serial: serial,
            body: body.clone().map(Box::new),
            group_order,
            ..data
        }),
    });
    program.preassign_label_to_next_insn(next);
    program.emit_insn(Insn::MorselNext {
        dest_start,
        count,
        target_pc_done: end,
    });
    if let Some(reg) = offset {
        program.emit_insn(Insn::IfPos {
            reg,
            target_pc: next,
            decrement_by: 1,
        });
    }
    // A group that fails a HAVING term is skipped, as the serial loop skips it.
    for term in &having_terms {
        let passed = program.allocate_label();
        translate_condition_expr(
            program,
            &plan.table_references,
            term,
            ConditionMetadata {
                jump_if_condition_is_true: false,
                jump_target_when_true: passed,
                jump_target_when_false: next,
                jump_target_when_null: next,
            },
            resolver,
        )?;
        program.preassign_label_to_next_insn(passed);
    }
    program.emit_insn(Insn::ResultRow {
        start_reg: dest_start,
        count: shown,
    });
    if let Some(reg) = limit {
        program.emit_insn(Insn::DecrJumpZero {
            reg,
            target_pc: end,
        });
    }
    program.emit_insn(Insn::Goto { target_pc: next });
    if let (Some(looped), Some(body)) = (&looped, &body) {
        emit_body(program, resolver, plan, looped, body)?;
    }
    program.preassign_label_to_next_insn(serial);
    Ok(Some(MorselHook { end }))
}

/// A join's body that is nothing but reads (lane qs6-13, T-QT.6.21): with no ON or WHERE term
/// left, each output a column of a table in the row or its rowid, the place each output's value
/// has in the joined row (`None`: the probe's own rowid), the very register the body's copy would
/// read (`emit_body`); otherwise none.
fn direct_reads(looped: &BodyPlan) -> Option<Vec<Option<usize>>> {
    if !looped.on.is_empty()
        || !looped.filter.is_empty()
        || !looped.agg_filters.is_empty()
        || looped.links_on.iter().any(|on| !on.is_empty())
    {
        return None;
    }
    let place = |id: TableInternalId| looped.places.iter().find(|p| p.id == id);
    let mut direct = Vec::new();
    for expr in &looped.outs {
        direct.push(match expr {
            Expr::Column { table, column, .. } => Some(place(*table)?.offset.checked_add(*column)?),
            Expr::RowId { table, .. } => {
                let p = place(*table)?;
                match p.rowid {
                    RowidAt::Own => None,
                    RowidAt::Column(column) => Some(p.offset.checked_add(column)?),
                    RowidAt::Nowhere => return None,
                }
            }
            _ => return None,
        });
    }
    Some(direct)
}

/// The loop body as the workers run it (QT.6, lane qs6-7): its filter terms, and the expressions
/// whose values the pipeline folds (the GROUP BY values, then each aggregate's input) or returns
/// (the result columns, then a join's ORDER BY keys), any expression the serial loop takes. A
/// one-table scan's row is the table's columns; a join's (lane qs6-12) is each table's in turn.
struct BodyPlan<'p> {
    /// The scan's table, or a join's probe.
    table: &'p BTreeTable,
    /// Each table the body reads, and where its columns and rowid are in the row.
    places: Vec<BodyTable<'p>>,
    /// The row's width: every table's columns.
    width: usize,
    /// An outer join's ON terms the hash keys and the probe's own `Predicate`s don't hold (lane
    /// qs6-12): a joined row that fails them did not match.
    on: Vec<&'p Expr>,
    /// A [`MorselOuter::Chain`]'s links' own ON terms (lane qs6-14), a list per link: a joined
    /// row up to the link that fails them did not match there.
    links_on: Vec<Vec<&'p Expr>>,
    filter: Vec<&'p Expr>,
    outs: Vec<&'p Expr>,
    group_by: Vec<usize>,
    aggregates: Vec<Aggregate>,
    no_real: Vec<usize>,
    result_map: Vec<usize>,
    /// Each aggregate's FILTER and its input's place in `outs` (lane qs6-17).
    agg_filters: Vec<(usize, &'p Expr)>,
    /// The scan's least and greatest rowid, `(None, None)` for the whole table (lane qs6-17).
    rowid_range: (Option<i64>, Option<i64>),
    /// A one-table scan's ORDER BY (lane qs6-17); a join's is its data's `joined_rows`.
    sorted: Option<MorselRows>,
    /// The outputs are the result columns, returned as rows (lane qs6-8).
    rows: bool,
}

/// A table in the body's row: its id, its first column's place, and where its rowid is read.
struct BodyTable<'p> {
    id: TableInternalId,
    table: &'p BTreeTable,
    offset: usize,
    rowid: RowidAt,
}

/// Where the body reads a table's rowid: the row's own (the scan's or the probe's), its INTEGER
/// PRIMARY KEY column, or nowhere (a build table without one: a body reading it is declined).
#[derive(Clone, Copy, PartialEq, Eq)]
enum RowidAt {
    Own,
    Column(usize),
    Nowhere,
}

/// The body's outputs: the GROUP BY values, then each aggregate's input, or the result columns
/// when `rows`; the places a SUM reads; each result column's place in the pipeline's row; and
/// each aggregate's FILTER with its input's place (lane qs6-17).
struct BodyOutputs<'p> {
    outs: Vec<&'p Expr>,
    group_by: Vec<usize>,
    aggregates: Vec<Aggregate>,
    sums: Vec<usize>,
    result_map: Vec<usize>,
    agg_filters: Vec<(usize, &'p Expr)>,
}

fn body_outputs(plan: &SelectPlan, rows: bool) -> Option<BodyOutputs<'_>> {
    let mut outs: Vec<&Expr> = Vec::new();
    let mut group_by = Vec::new();
    if let Some(group) = &plan.group_by {
        // A HAVING over the aggregates alone is the row loop's (`having_terms`, lane qs6-17).
        if group.having.is_some() && having_terms(plan, 0).is_none() {
            return None;
        }
        for expr in &group.exprs {
            group_by.push(outs.len());
            outs.push(expr);
        }
    }
    let mut aggregates = Vec::new();
    let mut sums = Vec::new();
    let mut agg_filters = Vec::new();
    for aggregate in &plan.aggregates {
        if !matches!(aggregate.distinctness, Distinctness::NonDistinct) {
            return None;
        }
        let at = outs.len();
        // An aggregate's FILTER (lane qs6-17): its input reads NULL on a row that fails it, which
        // every aggregate the pipeline folds skips, as the serial loop skips the row.
        if let Some(filter) = &aggregate.filter_expr {
            agg_filters.push((at, filter));
        }
        aggregates.push(match (&aggregate.func, aggregate.args.as_slice()) {
            // COUNT(*) FILTER (WHERE c) counts the rows where c holds: c itself as the input, a
            // true value never NULL.
            (AggFunc::Count0, []) => match &aggregate.filter_expr {
                None => Aggregate::CountStar,
                Some(filter) => {
                    outs.push(filter);
                    Aggregate::Count(at)
                }
            },
            (AggFunc::Count, [arg]) => {
                outs.push(arg);
                Aggregate::Count(at)
            }
            (AggFunc::Sum, [arg]) => {
                outs.push(arg);
                sums.push(at);
                Aggregate::Sum(at)
            }
            (AggFunc::Min, [arg]) => {
                outs.push(arg);
                Aggregate::Min(at)
            }
            (AggFunc::Max, [arg]) => {
                outs.push(arg);
                Aggregate::Max(at)
            }
            // Over integers only, as a SUM: a REAL sends the statement to the serial loop, whose
            // compensated double sum isn't the exact one (lane qs6-17).
            (AggFunc::Avg, [arg]) => {
                outs.push(arg);
                sums.push(at);
                Aggregate::Avg(at)
            }
            (AggFunc::Total, [arg]) => {
                outs.push(arg);
                sums.push(at);
                Aggregate::Total(at)
            }
            _ => return None,
        });
    }
    let keys = group_by.len();
    let mut result_map = Vec::new();
    if rows {
        for result in &plan.result_columns {
            result_map.push(outs.len());
            outs.push(&result.expr);
        }
    }
    for result in plan.result_columns.iter().filter(|_| !rows) {
        let key = if result.contains_aggregates {
            None
        } else {
            outs.iter().take(keys).position(|g| **g == result.expr)
        };
        let at = match key {
            Some(at) => at,
            None => keys.checked_add(
                plan.aggregates
                    .iter()
                    .position(|a| a.original_expr == result.expr)?,
            )?,
        };
        result_map.push(at);
    }
    Some(BodyOutputs {
        outs,
        group_by,
        aggregates,
        sums,
        result_map,
        agg_filters,
    })
}

/// The places in the joined row of the columns of REAL affinity (audit 3, prototypes 2), which
/// the workers turn from INTEGER to REAL as they load a row ([`MorselBody::reals`]).
fn real_places(places: &[BodyTable]) -> Vec<usize> {
    places
        .iter()
        .flat_map(|p| {
            p.table
                .columns()
                .iter()
                .enumerate()
                .filter(|(_, col)| col.affinity() == Affinity::Real)
                .filter_map(move |(at, _)| p.offset.checked_add(at))
        })
        .collect()
}

/// Whether every column `exprs` read is one of `places`' tables', stored as written and compared
/// bytewise (the pipeline compares GROUP BY values and MIN and MAX bytewise), every rowid one the
/// row holds; no COLLATE, no subquery.
fn body_reads(exprs: &[&Expr], places: &[BodyTable]) -> Option<()> {
    body_reads_numeric(exprs, places, false)
}

/// [`body_reads`], with `numeric` also columns of REAL and NUMERIC affinity (audit 3, prototypes
/// 2): a one-table scan's, whose workers load a REAL column's INTEGER as a REAL
/// ([`MorselBody::reals`]), compare GROUP BY values as numbers (`morsel::key_part`: an integral
/// REAL is its INTEGER) and MIN and MAX by `Value`'s order (an INTEGER against a REAL by value),
/// ties keeping the earliest row in scan order as the serial loop does. A join keeps them out:
/// its probe row's matches come in its hash tables' order, not the serial loop's.
fn body_reads_numeric(exprs: &[&Expr], places: &[BodyTable], numeric: bool) -> Option<()> {
    let place = |id: TableInternalId| places.iter().find(|p| p.id == id);
    for expr in exprs {
        let mut fine = true;
        walk_expr(expr, &mut |e: &Expr| -> crate::Result<WalkControl> {
            match e {
                Expr::Column {
                    table: t, column, ..
                } => {
                    fine &= place(*t).is_some_and(|p| {
                        p.table.columns().get(*column).is_some_and(|col| {
                            (matches!(
                                col.affinity(),
                                Affinity::Integer
                                    | Affinity::Text
                                    | Affinity::Blob
                                    | Affinity::None
                            ) || (numeric
                                && matches!(col.affinity(), Affinity::Real | Affinity::Numeric)))
                                && matches!(
                                    col.collation(),
                                    CollationSeq::Binary | CollationSeq::Unset
                                )
                        })
                    });
                }
                Expr::RowId { table: t, .. } => {
                    fine &= place(*t).is_some_and(|p| p.rowid != RowidAt::Nowhere)
                }
                Expr::Collate(..)
                | Expr::Exists(..)
                | Expr::InSelect { .. }
                | Expr::Subquery(..)
                | Expr::TriggerRegister(..)
                | Expr::Register(..) => fine = false,
                _ => {}
            }
            Ok(WalkControl::Continue)
        })
        .ok()?;
        if !fine {
            return None;
        }
    }
    Some(())
}

fn body_plan(plan: &SelectPlan) -> Option<BodyPlan<'_>> {
    // A scan with no aggregates returns its rows (lane qs6-8). A LIMIT and OFFSET cut the rows
    // in scan order, or in the ORDER BY's, which each worker sorts its morsels' rows by and a
    // stable merge joins (lane qs6-17, `scan_order`).
    let rows = plan.aggregates.is_empty();
    if !(if rows { plain_rows(plan) } else { plain(plan) }) {
        return None;
    }
    let [joined] = plan.table_references.joined_tables() else {
        return None;
    };
    let rowid_range = scanned_rowids(&joined.op)?;
    let table = stored(joined)?;
    // A rowid range's leaves hold rows past its bounds: the terms it was made from are tested on
    // every row, consumed or not (lane qs6-17).
    let ranged = rowid_range != (None, None);
    let filter: Vec<&Expr> = plan
        .where_clause
        .iter()
        .filter(|term| ranged || !term.consumed)
        .map(|term| &term.expr)
        .collect();
    let BodyOutputs {
        mut outs,
        group_by,
        aggregates,
        sums,
        result_map,
        agg_filters,
    } = body_outputs(plan, rows)?;
    let sorted = scan_order(plan, joined.internal_id, table, &mut outs)?;
    let places = vec![BodyTable {
        id: joined.internal_id,
        table,
        offset: 0,
        rowid: RowidAt::Own,
    }];
    body_reads_numeric(&filter, &places, true)?;
    body_reads_numeric(&outs, &places, true)?;
    body_reads_numeric(&filter_exprs(&agg_filters), &places, true)?;
    Some(BodyPlan {
        table,
        places,
        width: table.columns().len(),
        on: Vec::new(),
        links_on: Vec::new(),
        filter,
        outs,
        group_by,
        aggregates,
        no_real: sums,
        result_map,
        agg_filters,
        rowid_range,
        sorted,
        rows,
    })
}

/// A one-table scan's ORDER BY as its workers sort by (lane qs6-17): each key a column of the
/// table (or its rowid) of INTEGER or TEXT affinity and BINARY collation, no NULLS FIRST or LAST
/// named (SQLite's default, NULLs least, is the values' own order), so the workers' sort is the
/// serial sorter's; each key's expression joins `outs` after the result columns. Under a LIMIT or
/// OFFSET the keys must hold the rowid, which tells every row apart, so the rows cut are the serial
/// loop's to the row. `Some(None)`: no ORDER BY; None: the serial loop's.
fn scan_order<'p>(
    plan: &'p SelectPlan,
    id: TableInternalId,
    table: &BTreeTable,
    outs: &mut Vec<&'p Expr>,
) -> Option<Option<MorselRows>> {
    if plan.order_by.is_empty() {
        return Some(None);
    }
    let alias = rowid_alias(table);
    let mut whole = false;
    let width = outs.len();
    let mut order = Vec::new();
    for (expr, sort, nulls) in &plan.order_by {
        if nulls.is_some() {
            return None;
        }
        let expr: &'p Expr = expr;
        let column = match expr {
            Expr::Column {
                table: t, column, ..
            } if *t == id => Some(*column),
            Expr::RowId { table: t, .. } if *t == id => None,
            _ => return None,
        };
        let plain = match column {
            None => true,
            Some(column) => table.columns().get(column).is_some_and(|col| {
                matches!(col.affinity(), Affinity::Integer | Affinity::Text)
                    && matches!(col.collation(), CollationSeq::Binary | CollationSeq::Unset)
            }),
        };
        if !plain {
            return None;
        }
        whole |= column.is_none() || column == alias;
        order.push((outs.len(), matches!(sort, SortOrder::Desc)));
        outs.push(expr);
    }
    if (plan.limit.is_some() || plan.offset.is_some()) && !whole {
        return None;
    }
    Some(Some(MorselRows { order, width }))
}

/// The rowids a one-table scan reads (lane qs6-17): the whole table read forward (`(None, None)`), or a
/// forward search of the table b-tree by rowid between integer literals, its least and greatest
/// rowid (each bound taken inclusively: the morsels are cut from the leaves that hold them, and
/// the terms the search was made from decide each row). None: another access, the serial loop's.
fn scanned_rowids(op: &Operation) -> Option<(Option<i64>, Option<i64>)> {
    match op {
        // Forward only: a backward scan may stand for an ORDER BY ... DESC the planner removed,
        // and the morsels' rows come out in rowid order.
        Operation::Scan(Scan::BTreeTable {
            index: None,
            iter_dir: IterationDirection::Forwards,
        }) => Some((None, None)),
        Operation::Search(Search::Seek {
            index: None,
            seek_def,
        }) if seek_def.prefix.is_empty()
            && seek_def.skip == 0
            && seek_def.iter_dir == IterationDirection::Forwards =>
        {
            let bound = |key: &SeekKeyComponent<Expr>| -> Option<Option<i64>> {
                match key {
                    SeekKeyComponent::None => Some(None),
                    SeekKeyComponent::Expr(expr) => integer(expr).map(Some),
                    SeekKeyComponent::Row(_) | SeekKeyComponent::Null => None,
                }
            };
            let lower = bound(&seek_def.start.last_component)?;
            let upper = bound(&seek_def.end.last_component)?;
            // An empty range is the serial loop's: it reads nothing.
            if let (Some(lower), Some(upper)) = (lower, upper) {
                if lower > upper {
                    return None;
                }
            }
            Some((lower, upper))
        }
        _ => None,
    }
}

/// A forward search of the table b-tree by a rowid range (lane R11), as terms on the rowid's
/// alias column, which the serial loop reads its rows by: `alias >= x` or `alias > x` from its
/// start, `alias <= y` or `alias < y` from its end (an end `GT` stops past `y`), each bound an
/// integer literal or a bound parameter. The pipelines test each row's rowid by them, and read
/// only the rowids they admit (`morsel::rowid_range`). None: another search, the serial loop's.
///
/// [`Predicate::bind`] converts a bound value as SQLite converts it, and [`Predicate::holds`]
/// compares it, so each type answers as in SQLite 3.54.0 (its source, `vdbe.c` and
/// `wherecode.c`):
/// - The start, `OP_SeekGE` or `OP_SeekGT` on the table cursor: an integer seeks itself; a real,
///   the rowids at or past it (`sqlite3IntFloatCompare` moves the seek past the integer it
///   truncates to); text that reads as a number, that number (`applyNumericAffinity`); other text,
///   a blob and NULL, no row (`jump_to_p2`).
/// - The end, `OP_Gt` or `OP_Ge` of the rowid with it under NUMERIC affinity, `SQLITE_JUMPIFNULL`
///   (`sqlite3WhereCodeOneLoopStart`, case 3): an integer, a real and text that reads as a number
///   compare by value; other text and a blob are above every rowid, so none is past them; NULL,
///   no row.
fn rowid_bounds(seek_def: &SeekDef, alias: usize) -> Option<Vec<Predicate>> {
    if !seek_def.prefix.is_empty()
        || seek_def.skip != 0
        || seek_def.iter_dir != IterationDirection::Forwards
    {
        return None;
    }
    let mut terms = Vec::new();
    for (key, op) in [
        (
            &seek_def.start,
            match seek_def.start.op {
                SeekOp::GE { eq_only: false } => Some(CmpOp::Ge),
                SeekOp::GT => Some(CmpOp::Gt),
                _ => None,
            },
        ),
        (
            &seek_def.end,
            match seek_def.end.op {
                SeekOp::GT => Some(CmpOp::Le),
                SeekOp::GE { eq_only: false } => Some(CmpOp::Lt),
                _ => None,
            },
        ),
    ] {
        match &key.last_component {
            SeekKeyComponent::None => {}
            SeekKeyComponent::Expr(Expr::Variable(variable)) => {
                terms.push(Predicate::Parameter {
                    column: alias,
                    op: op?,
                    index: std::num::NonZeroUsize::new(
                        usize::try_from(variable.index.get()).ok()?,
                    )?,
                    affinity: Affinity::Numeric,
                });
            }
            SeekKeyComponent::Expr(expr) => terms.push(Predicate::Compare {
                column: alias,
                op: op?,
                value: Value::from_i64(integer(expr)?),
            }),
            SeekKeyComponent::Row(_) | SeekKeyComponent::Null => return None,
        }
    }
    Some(terms)
}

/// How a GROUP BY's groups come out (`MorselAggregateData::group_order`); none when the planner
/// states no order for a key, the serial loop's.
fn group_order(plan: &SelectPlan) -> Option<Vec<KeyInfo>> {
    let Some(group) = &plan.group_by else {
        return Some(Vec::new());
    };
    (0..group.exprs.len())
        .map(|key| {
            Some(KeyInfo {
                sort_order: *group.sort_order.get(key)?,
                collation: CollationSeq::Binary,
                nulls_order: *group.nulls_order.get(key)?,
            })
        })
        .collect()
}

/// A GROUP BY's HAVING terms as the hook's row loop tests them (lane qs6-17): each aggregate in
/// them rewritten to the register its value is loaded into, `first + i` for the plan's aggregate
/// `i`. Only terms over the aggregates and
/// constants qualify: a group key or a bare column read through a register would lose the
/// column's affinity in a comparison, which the serial loop applies. None: the statement is the
/// serial loop's; an empty list: no HAVING.
fn having_terms(plan: &SelectPlan, first: usize) -> Option<Vec<Expr>> {
    let Some(having) = plan.group_by.as_ref().and_then(|g| g.having.as_ref()) else {
        return Some(Vec::new());
    };
    let mut terms = Vec::new();
    for term in having {
        let mut term = term.clone();
        let mut fine = true;
        walk_expr_mut(
            &mut term,
            &mut |e: &mut Expr| -> crate::Result<WalkControl> {
                if let Some(at) = plan.aggregates.iter().position(|a| a.original_expr == *e) {
                    match first.checked_add(at) {
                        Some(reg) => *e = Expr::Register(reg),
                        None => fine = false,
                    }
                    return Ok(WalkControl::SkipChildren);
                }
                match e {
                    Expr::Column { .. }
                    | Expr::RowId { .. }
                    | Expr::Collate(..)
                    | Expr::Exists(..)
                    | Expr::InSelect { .. }
                    | Expr::Subquery(..)
                    | Expr::TriggerRegister(..)
                    | Expr::Register(..) => fine = false,
                    _ => {}
                }
                Ok(WalkControl::Continue)
            },
        )
        .ok()?;
        if !fine {
            return None;
        }
        terms.push(term);
    }
    Some(terms)
}

/// The FILTER expressions of `agg_filters`, for [`body_reads`] (lane qs6-17).
fn filter_exprs<'p>(agg_filters: &[(usize, &'p Expr)]) -> Vec<&'p Expr> {
    agg_filters.iter().map(|(_, filter)| *filter).collect()
}

/// Emits the body's copy after the hook's row loop, where the serial loop never reaches: each
/// column of the table read from its register (the worker writes the row there), each filter
/// term jumping to `skip` when it fails, each output into its register, then `keep`.
fn emit_body(
    program: &mut ProgramBuilder,
    resolver: &Resolver,
    plan: &SelectPlan,
    looped: &BodyPlan,
    body: &MorselBody,
) -> crate::Result<()> {
    let mut resolver = resolver.fork();
    resolver.enable_expr_to_reg_cache();
    let mut reads = Vec::new();
    let overflow = || crate::LimboError::InternalError("register overflow".into());
    let place = |id: TableInternalId| looped.places.iter().find(|p| p.id == id);
    for expr in looped
        .links_on
        .iter()
        .flatten()
        .chain(looped.on.iter())
        .chain(looped.filter.iter())
        .chain(looped.outs.iter())
        .chain(looped.agg_filters.iter().map(|(_, filter)| filter))
    {
        walk_expr(expr, &mut |e: &Expr| -> crate::Result<WalkControl> {
            match e {
                Expr::Column { table, column, .. } => {
                    if let Some(p) = place(*table) {
                        let reg = body
                            .columns_start
                            .checked_add(p.offset)
                            .and_then(|reg| reg.checked_add(*column))
                            .ok_or_else(overflow)?;
                        reads.push((e.clone(), reg));
                    }
                }
                Expr::RowId { table, .. } => {
                    if let Some(p) = place(*table) {
                        let reg = match p.rowid {
                            RowidAt::Own => body.rowid,
                            RowidAt::Column(column) => body
                                .columns_start
                                .checked_add(p.offset)
                                .and_then(|reg| reg.checked_add(column))
                                .ok_or_else(overflow)?,
                            RowidAt::Nowhere => {
                                return Err(crate::LimboError::InternalError(
                                    "the loop body reads a rowid its row lacks".into(),
                                ))
                            }
                        };
                        reads.push((e.clone(), reg));
                    }
                }
                _ => {}
            }
            Ok(WalkControl::Continue)
        })?;
    }
    for (expr, reg) in reads {
        resolver.cache_scalar_expr_reg(
            std::borrow::Cow::Owned(expr),
            reg,
            false,
            &plan.table_references,
        )?;
    }
    // A chain's links' ON code (lane qs6-14), each its own block: a row up to the link that
    // reaches the block's end matched there.
    for (terms, labels) in looped.links_on.iter().zip(&body.links_on) {
        let Some((from, pass)) = labels else {
            continue;
        };
        program.preassign_label_to_next_insn(*from);
        for term in terms {
            let passed = program.allocate_label();
            translate_condition_expr(
                program,
                &plan.table_references,
                term,
                ConditionMetadata {
                    jump_if_condition_is_true: false,
                    jump_target_when_true: passed,
                    jump_target_when_false: body.skip,
                    jump_target_when_null: body.skip,
                },
                &resolver,
            )?;
            program.preassign_label_to_next_insn(passed);
        }
        program.preassign_label_to_next_insn(*pass);
        program.emit_insn(Insn::Noop);
    }
    // An outer join's ON terms come first: a row that reaches `start` matched (lane qs6-12).
    if let Some(on) = body.on {
        program.preassign_label_to_next_insn(on);
    }
    for term in &looped.on {
        let passed = program.allocate_label();
        translate_condition_expr(
            program,
            &plan.table_references,
            term,
            ConditionMetadata {
                jump_if_condition_is_true: false,
                jump_target_when_true: passed,
                jump_target_when_false: body.skip,
                jump_target_when_null: body.skip,
            },
            &resolver,
        )?;
        program.preassign_label_to_next_insn(passed);
    }
    program.preassign_label_to_next_insn(body.start);
    for term in &looped.filter {
        let passed = program.allocate_label();
        translate_condition_expr(
            program,
            &plan.table_references,
            term,
            ConditionMetadata {
                jump_if_condition_is_true: false,
                jump_target_when_true: passed,
                jump_target_when_false: body.skip,
                jump_target_when_null: body.skip,
            },
            &resolver,
        )?;
        program.preassign_label_to_next_insn(passed);
    }
    for (at, expr) in looped.outs.iter().enumerate() {
        let reg = body
            .out_start
            .checked_add(at)
            .ok_or_else(|| crate::LimboError::InternalError("register overflow".into()))?;
        let filtered = looped
            .agg_filters
            .iter()
            .find(|(place, _)| *place == at)
            .map(|(_, filter)| *filter);
        let Some(filter) = filtered else {
            translate_expr(program, Some(&plan.table_references), expr, reg, &resolver)?;
            continue;
        };
        // An aggregate's FILTER (lane qs6-17): a row that fails it gives its input NULL.
        let passed = program.allocate_label();
        let failed = program.allocate_label();
        let done = program.allocate_label();
        translate_condition_expr(
            program,
            &plan.table_references,
            filter,
            ConditionMetadata {
                jump_if_condition_is_true: false,
                jump_target_when_true: passed,
                jump_target_when_false: failed,
                jump_target_when_null: failed,
            },
            &resolver,
        )?;
        program.preassign_label_to_next_insn(passed);
        translate_expr(program, Some(&plan.table_references), expr, reg, &resolver)?;
        program.emit_insn(Insn::Goto { target_pc: done });
        program.preassign_label_to_next_insn(failed);
        program.emit_insn(Insn::Null {
            dest: reg,
            dest_end: None,
        });
        program.preassign_label_to_next_insn(done);
    }
    // A label lands after the instruction emitted before it, wherever that one moves, and an
    // output that is a constant moves to the program's end (lane R4): `keep` is placed after a
    // Noop of the body's own, so it stays on the body's next one.
    program.emit_insn(Insn::Noop);
    program.preassign_label_to_next_insn(body.keep);
    program.emit_insn(Insn::Noop);
    program.preassign_label_to_next_insn(body.skip);
    program.emit_insn(Insn::Noop);
    Ok(())
}

/// A statement's LIMIT and OFFSET as the hook's row loop counts them (QT.6, lane qs6-6).
struct Window {
    limit: Option<i64>,
    offset: i64,
}

/// An integer literal, or the negation of one.
fn integer(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Literal(Literal::Numeric(text)) => text.parse::<i64>().ok(),
        Expr::Unary(UnaryOperator::Negative, inner) => match inner.as_ref() {
            Expr::Literal(Literal::Numeric(text)) => {
                text.parse::<i64>().ok().and_then(i64::checked_neg)
            }
            _ => None,
        },
        _ => None,
    }
}

/// The LIMIT and OFFSET when each is absent or an integer literal, read as SQLite reads them: a
/// negative LIMIT is none, a negative OFFSET is none. LIMIT 0 and any other expression decline
/// the hook; the serial loop answers them.
/// Whether the table the hook reads, at `root`, is declared `COLUMNAR`.
fn declared(plan: &SelectPlan, root: i64) -> bool {
    plan.table_references
        .joined_tables()
        .iter()
        .any(|t| matches!(&t.table, Table::BTree(b) if b.root_page == root && b.columnar))
}

fn window(plan: &SelectPlan) -> Option<Window> {
    let limit = match plan.limit.as_deref() {
        None => None,
        Some(expr) => match integer(expr)? {
            0 => return None,
            n if n < 0 => None,
            n => Some(n),
        },
    };
    let offset = match plan.offset.as_deref() {
        None => 0,
        Some(expr) => integer(expr)?.max(0),
    };
    Some(Window { limit, offset })
}

fn shape(plan: &SelectPlan) -> Option<MorselAggregateData> {
    if !plain(plan) {
        return None;
    }
    let [joined] = plan.table_references.joined_tables() else {
        return None;
    };
    let table = stored(joined)?;
    let mut filter = match &joined.op {
        Operation::Scan(Scan::BTreeTable { index: None, .. }) => Vec::new(),
        Operation::Search(Search::Seek {
            index: None,
            seek_def,
        }) => rowid_bounds(seek_def, rowid_alias(table)?)?,
        _ => return None,
    };
    let columns = table.columns();
    let id = joined.internal_id;
    // A column the hook reads: of this table, stored as written, compared bytewise.
    let column = |expr: &Expr| -> Option<usize> {
        let Expr::Column {
            table: t, column, ..
        } = expr
        else {
            return None;
        };
        if *t != id {
            return None;
        }
        let col = columns.get(*column)?;
        let plain = matches!(
            col.affinity(),
            Affinity::Integer | Affinity::Text | Affinity::Blob | Affinity::None
        ) && matches!(col.collation(), CollationSeq::Binary | CollationSeq::Unset);
        if plain {
            Some(*column)
        } else {
            None
        }
    };
    // A column a bound parameter is compared with: INTEGER or REAL (lane R10), which the copy and
    // the morsels read as the serial loop does (`MorselAggregateData::reals`), compared bytewise.
    let compared = |expr: &Expr| -> Option<usize> {
        let Expr::Column {
            table: t, column, ..
        } = expr
        else {
            return None;
        };
        let col = columns.get(*column)?;
        if *t == id
            && matches!(col.affinity(), Affinity::Integer | Affinity::Real)
            && matches!(col.collation(), CollationSeq::Binary | CollationSeq::Unset)
        {
            Some(*column)
        } else {
            None
        }
    };
    for term in &plan.where_clause {
        if term.consumed {
            continue;
        }
        filter.push(
            predicate(&term.expr, &column, table)
                .or_else(|| parameter(&term.expr, &compared, &plan.table_references))?,
        );
    }
    // A column SUM and AVG take: INTEGER or REAL (lane R11), each value read as the serial loop
    // reads it; a REAL met is the serial loop's under `RealSums::Serial`, which the copy and the
    // morsels hand back.
    let summed = |expr: &Expr| -> Option<usize> {
        let Expr::Column {
            table: t, column, ..
        } = expr
        else {
            return None;
        };
        let col = columns.get(*column)?;
        if *t == id && matches!(col.affinity(), Affinity::Integer | Affinity::Real) {
            Some(*column)
        } else {
            None
        }
    };
    let (group_by, aggregates, result_map) = outputs(plan, &column, &summed)?;
    Some(MorselAggregateData {
        root_page: table.root_page,
        columns: columns.len(),
        rowid_alias: rowid_alias(table),
        reals: reals(table),
        defaults: defaults(table),
        rowid_range: (None, None),
        filter,
        group_by,
        group_order: Vec::new(),
        aggregates,
        result_map,
        joins: Vec::new(),
        joined_rows: None,
        body: None,
        outer: None,
        target_pc_serial: crate::vdbe::BranchOffset::Placeholder,
    })
}

fn rowid_alias(table: &BTreeTable) -> Option<usize> {
    table.get_rowid_alias_column().map(|(at, _)| at)
}

/// The table's columns of REAL affinity ([`MorselAggregateData::reals`]).
fn reals(table: &BTreeTable) -> Vec<usize> {
    table
        .columns()
        .iter()
        .enumerate()
        .filter(|(_, c)| c.affinity() == Affinity::Real)
        .map(|(at, _)| at)
        .collect()
}

fn predicate(
    expr: &Expr,
    column: &impl Fn(&Expr) -> Option<usize>,
    table: &BTreeTable,
) -> Option<Predicate> {
    match expr {
        Expr::IsNull(inner) => Some(Predicate::IsNull {
            column: column(inner)?,
        }),
        Expr::NotNull(inner) => Some(Predicate::NotNull {
            column: column(inner)?,
        }),
        // `x IS NULL` and `x IS NOT NULL` as the parser may give them (lane qs6-10).
        Expr::Binary(inner, Operator::Is, null)
            if matches!(null.as_ref(), Expr::Literal(Literal::Null)) =>
        {
            Some(Predicate::IsNull {
                column: column(inner)?,
            })
        }
        Expr::Binary(inner, Operator::IsNot, null)
            if matches!(null.as_ref(), Expr::Literal(Literal::Null)) =>
        {
            Some(Predicate::NotNull {
                column: column(inner)?,
            })
        }
        Expr::Binary(left, op, right) => {
            let (at, op, value) = match (column(left), column(right)) {
                (Some(at), None) => (at, *op, literal(right)?),
                (None, Some(at)) => (at, flip(*op)?, literal(left)?),
                _ => return None,
            };
            let op = match op {
                Operator::Less => CmpOp::Lt,
                Operator::LessEquals => CmpOp::Le,
                Operator::Equals => CmpOp::Eq,
                Operator::NotEquals => CmpOp::Ne,
                Operator::Greater => CmpOp::Gt,
                Operator::GreaterEquals => CmpOp::Ge,
                _ => return None,
            };
            // A literal of the column's own kind needs no conversion before it is compared.
            let affinity = table.columns().get(at)?.affinity();
            let same_kind = match &value {
                Value::Text(_) => affinity == Affinity::Text,
                Value::Numeric(crate::numeric::Numeric::Integer(_)) => {
                    affinity == Affinity::Integer
                }
                _ => false,
            };
            if same_kind {
                Some(Predicate::Compare {
                    column: at,
                    op,
                    value,
                })
            } else {
                None
            }
        }
        _ => None,
    }
}

/// `column op ?` (lane R9), or `? op column`, against an INTEGER or REAL column (lane R10): the
/// parameter's value is read when the statement runs and converted as the serial loop's comparison
/// converts it, by the comparison's affinity (SQLite's datatype3 section 4.2: NUMERIC applied to
/// the parameter, which has no affinity), so a row passes exactly where the serial loop's term
/// holds.
fn parameter(
    expr: &Expr,
    column: &impl Fn(&Expr) -> Option<usize>,
    refs: &crate::translate::plan::TableReferences,
) -> Option<Predicate> {
    let Expr::Binary(left, op, right) = expr else {
        return None;
    };
    let (at, op, variable) = match (column(left), column(right), left.as_ref(), right.as_ref()) {
        (Some(at), None, _, Expr::Variable(v)) => (at, *op, v),
        (None, Some(at), Expr::Variable(v), _) => (at, flip(*op)?, v),
        _ => return None,
    };
    let op = match op {
        Operator::Less => CmpOp::Lt,
        Operator::LessEquals => CmpOp::Le,
        Operator::Equals => CmpOp::Eq,
        Operator::NotEquals => CmpOp::Ne,
        Operator::Greater => CmpOp::Gt,
        Operator::GreaterEquals => CmpOp::Ge,
        _ => return None,
    };
    let affinity = crate::translate::expr::comparison_affinity(
        left.as_ref(),
        right.as_ref(),
        Some(refs),
        None,
    );
    if affinity != Affinity::Numeric {
        return None;
    }
    let index = std::num::NonZeroUsize::new(usize::try_from(variable.index.get()).ok()?)?;
    Some(Predicate::Parameter {
        column: at,
        op,
        index,
        affinity,
    })
}

fn flip(op: Operator) -> Option<Operator> {
    Some(match op {
        Operator::Less => Operator::Greater,
        Operator::LessEquals => Operator::GreaterEquals,
        Operator::Greater => Operator::Less,
        Operator::GreaterEquals => Operator::LessEquals,
        Operator::Equals => Operator::Equals,
        Operator::NotEquals => Operator::NotEquals,
        _ => return None,
    })
}

/// An integer or a text literal's value; anything else declines the hook.
fn literal(expr: &Expr) -> Option<Value> {
    match expr {
        Expr::Literal(Literal::Numeric(text)) => text.parse::<i64>().ok().map(Value::from_i64),
        Expr::Unary(UnaryOperator::Negative, inner) => match inner.as_ref() {
            Expr::Literal(Literal::Numeric(text)) => text
                .parse::<i64>()
                .ok()
                .and_then(i64::checked_neg)
                .map(Value::from_i64),
            _ => None,
        },
        Expr::Literal(Literal::String(quoted)) => {
            let inner = quoted.strip_prefix('\'')?.strip_suffix('\'')?;
            Some(Value::build_text(inner.replace("''", "'")))
        }
        _ => None,
    }
}

/// The checks every shape shares: a top-level SELECT whose rows are its aggregates' groups, with
/// none of what the pipelines don't run (ORDER BY, LIMIT, DISTINCT, windows, subqueries).
fn plain(plan: &SelectPlan) -> bool {
    !plan.aggregates.is_empty()
        && plan.order_by.is_empty()
        && plan.limit.is_none()
        && plan.offset.is_none()
        && plain_any(plan)
}

/// The checks a join's rows take (QT.6): no aggregates and no GROUP BY, an ORDER BY allowed, a
/// LIMIT and OFFSET allowed (`shape_join` asks the order that makes them exact), and the rest as
/// [`plain`].
fn plain_rows(plan: &SelectPlan) -> bool {
    plan.aggregates.is_empty()
        && plan.group_by.is_none()
        && plan.result_columns.iter().all(|r| !r.contains_aggregates)
        && plain_any(plan)
}

/// What every shape refuses: DISTINCT, windows, subqueries, rows not returned.
fn plain_any(plan: &SelectPlan) -> bool {
    !(plan.contains_constant_false_condition
        || !matches!(plan.query_destination, QueryDestination::ResultRows)
        || !matches!(plan.distinctness, Distinctness::NonDistinct)
        || !plan.values.is_empty()
        || plan.window.is_some()
        || !plan.non_from_clause_subqueries.is_empty()
        || plan.simple_aggregate.is_some()
        || !plan.table_references.outer_query_refs().is_empty())
}

/// A rowid table whose rows the morsels read as stored, no generated column. A row written
/// before a column was added (ALTER TABLE ADD COLUMN) reads that column's default, as the serial
/// loop's Column reads it ([`defaults`], lane qs6-17), except where the serial loop computes it
/// at run time: a STRICT table's custom type, whose ENCODE the default goes through.
fn stored(joined: &JoinedTable) -> Option<&BTreeTable> {
    let Table::BTree(table) = &joined.table else {
        return None;
    };
    let table: &BTreeTable = table;
    // A STRICT table's column names one of SQLite's six datatypes or a custom type (SQLite's
    // STRICT tables, section 2: INT, INTEGER, REAL, TEXT, BLOB, ANY).
    let custom = |ty: &str| {
        !["INT", "INTEGER", "REAL", "TEXT", "BLOB", "ANY"]
            .iter()
            .any(|builtin| ty.eq_ignore_ascii_case(builtin))
    };
    if !table.has_rowid
        || table.root_page <= 0
        || table.has_virtual_columns()
        || table.columns().iter().any(|c| {
            c.is_generated() || (c.default.is_some() && table.is_strict && custom(&c.ty_str))
        })
    {
        return None;
    }
    Some(table)
}

/// Each column's value in a row written before the column was added: its DEFAULT folded with its
/// affinity, as the serial loop's Column has it (`folded_default`), NULL where it has none or
/// doesn't fold. Empty when every one is NULL, the common table, so its scans pad nothing.
fn defaults(table: &BTreeTable) -> Vec<Value> {
    let folded = |c: &crate::schema::Column| {
        c.default
            .as_ref()
            .and_then(|d| crate::vdbe::builder::folded_default(d, c.affinity()))
    };
    if table.columns().iter().all(|c| folded(c).is_none()) {
        return Vec::new();
    }
    table
        .columns()
        .iter()
        .map(|c| folded(c).unwrap_or(Value::Null))
        .collect()
}

/// The GROUP BY keys, the aggregates and each result column's place in the pipeline's row (the
/// keys, then the aggregates), the columns read through `column`, and those SUM and AVG take
/// through `summed`.
fn outputs(
    plan: &SelectPlan,
    column: &impl Fn(&Expr) -> Option<usize>,
    summed: &impl Fn(&Expr) -> Option<usize>,
) -> Option<(Vec<usize>, Vec<Aggregate>, Vec<usize>)> {
    let mut group_by = Vec::new();
    if let Some(group) = &plan.group_by {
        if group.having.is_some() {
            return None;
        }
        for expr in &group.exprs {
            group_by.push(column(expr)?);
        }
    }
    let mut aggregates = Vec::new();
    for aggregate in &plan.aggregates {
        if aggregate.filter_expr.is_some()
            || !matches!(aggregate.distinctness, Distinctness::NonDistinct)
        {
            return None;
        }
        aggregates.push(match (&aggregate.func, aggregate.args.as_slice()) {
            (AggFunc::Count0, []) => Aggregate::CountStar,
            (AggFunc::Count, [arg]) => Aggregate::Count(column(arg)?),
            (AggFunc::Sum, [arg]) => Aggregate::Sum(summed(arg)?),
            (AggFunc::Avg, [arg]) => Aggregate::Avg(summed(arg)?),
            (AggFunc::Min, [arg]) => Aggregate::Min(column(arg)?),
            (AggFunc::Max, [arg]) => Aggregate::Max(column(arg)?),
            _ => return None,
        });
    }
    let mut result_map = Vec::new();
    for result in &plan.result_columns {
        let at = match group_by
            .iter()
            .position(|g| Some(*g) == column(&result.expr))
        {
            Some(at) if !result.contains_aggregates => at,
            _ => {
                let found = plan
                    .aggregates
                    .iter()
                    .position(|a| a.original_expr == result.expr)?;
                group_by.len().checked_add(found)?
            }
        };
        result_map.push(at);
    }
    Some((group_by, aggregates, result_map))
}

/// Whether the planner reads the table at `at` in the join order by a seek the hook may replace
/// with a hash link read whole (lane qs6-16, `Connection::set_parallel_searched_links`): a rowid
/// lookup or a seek in one of the table's indexes, past the first table (a seek there has no row
/// before it to key it). The hook tests every WHERE and ON term, consumed or not, on the joined
/// row, so the rows a seek would have skipped fail the very terms the seek was made from (a
/// partial index's rows too: the planner takes one only when the terms imply its condition). A
/// link is still keyed only by an equality of one affinity with a table before it, else the
/// hook declines (`next` finds no link). An IN-list seek is not taken.
fn searched_by_seek(op: &Operation, at: usize, searched_links: bool) -> bool {
    searched_links
        && at > 0
        && matches!(
            op,
            Operation::Search(Search::RowidEq { .. } | Search::Seek { .. })
        )
}

/// A join's probe with the aggregates over its rows, or its rows (QT.6): two or more rowid tables,
/// each joined inner and read whole, by a scan of its own rows or the planner's hash join (at least
/// one hash join; a seek in an index the statement builds from the table, an ephemeral one, reads
/// it whole as well). The hook's probe is the first table in the join order that no hash join
/// builds from; every other table is a link of a chain of hash tables (lane qs6-10), keyed by its
/// equalities with the tables before it in the chain (columns of INTEGER or TEXT affinity, one
/// affinity on both sides, BINARY collation), and taken in the join order among the tables such an
/// equality links to the chain. Every such equality is a key of the link that joins the later of
/// its two tables. The pipeline's row is the probe table's columns, then each link's.
///
/// A term a table's own `Predicate` holds is tested on that table's rows, before the probe or the
/// build. Every other term, and the outputs, are the loop body's copy (lane qs6-12), run per
/// joined row: any expression the serial loop takes over the tables' columns. Which joined row a
/// MIN, a MAX or a group's key values take on a tie (the serial loop's order of a probe row's
/// matches is its hash tables') cannot change the answer, since only an INTEGER and a REAL can
/// compare equal and differ: a REAL in such a place sends the statement to the serial loop.
fn shape_join(
    plan: &SelectPlan,
    probe: crate::OuterJoinProbe,
    searched_links: bool,
) -> Option<(MorselAggregateData, BodyPlan<'_>)> {
    // A join with no aggregates returns its rows (QT.6): the workers' runs merge in order.
    let rows_mode = plan.aggregates.is_empty();
    if !(if rows_mode {
        plain_rows(plan)
    } else {
        plain(plan)
    }) {
        return None;
    }
    let joined = plan.table_references.joined_tables();
    if joined.len() < 2 {
        return None;
    }
    let mut built = Vec::new();
    // The tables the planner reads by a seek, links read whole (lane qs6-16).
    let mut searched = Vec::new();
    let mut tables: Vec<&BTreeTable> = Vec::new();
    // Each table's rowid range, `(None, None)` where it is read whole (lane qs6-17).
    let mut ranges: Vec<(Option<i64>, Option<i64>)> = Vec::new();
    // An outer join (lane qs6-12): its table and which rows it keeps that match nothing.
    let mut outer: Option<(usize, MorselOuter)> = None;
    for (at, table) in joined.iter().enumerate() {
        let kind = match table.join_info.as_ref().map(|j| &j.join_type) {
            None | Some(JoinType::Inner) => None,
            Some(JoinType::LeftOuter) => Some(MorselOuter::Left),
            Some(JoinType::FullOuter) => Some(MorselOuter::Full),
            Some(_) => return None,
        };
        if let Some(kind) = kind {
            if outer.is_some() {
                return None;
            }
            outer = Some((at, kind));
        }
        match &table.op {
            Operation::Scan(Scan::BTreeTable { index: None, .. }) => {}
            // A forward search by a rowid range between literals reads the leaves that hold it
            // (lane qs6-17); every term is tested on the joined row, the range's among them.
            op if scanned_rowids(op).is_some() => {}
            // A seek in an index the statement builds from the whole table reads it whole too.
            Operation::Search(Search::Seek {
                index: Some(index), ..
            }) if index.ephemeral => {}
            // An outer join's hash join keeps the build rows that match nothing (LEFT), and the
            // probe rows too (FULL); the probe is the table the join pads with NULLs. A sort-merge
            // join (QT.5 step 4, lane qs5-21) stays serial for now: its probe merges forward from
            // the previous probe row's run, which a worker's morsel would start over.
            Operation::HashJoin(op)
                if op.probe_table_idx != op.build_table_idx
                    && op.method == crate::translate::plan::HashJoinMethod::Hash
                    && matches!(
                        (&op.join_type, kind),
                        (HashJoinType::Inner, None)
                            | (HashJoinType::LeftOuter, Some(MorselOuter::Left))
                            | (HashJoinType::FullOuter, Some(MorselOuter::Full))
                    ) =>
            {
                built.push(op.build_table_idx)
            }
            op if searched_by_seek(op, at, searched_links) => searched.push(at),
            _ => return None,
        }
        tables.push(stored(table)?);
        ranges.push(scanned_rowids(&table.op).unwrap_or((None, None)));
    }
    // A range's leaves drop the rows it leaves out before the join, right only where each would
    // fail a term of an inner join; in an outer join the range is read whole, and the terms decide
    // (a kept table's row outside an ON term's range is still kept).
    if outer.is_some() {
        ranges.fill((None, None));
    }
    // An outer join of two tables (lane qs6-12): the probe is the table the join pads, the
    // other the build, whatever loop the planner chose (it makes no FULL hash join); the hook keys
    // its hash table by the ON equalities. Under a LIMIT or OFFSET (lane qs6-13) its rows are cut
    // as an inner join's are, only when the ORDER BY holds every table's INTEGER PRIMARY KEY
    // (below): a padded row's NULL key still tells it apart, since a probe row is padded at most
    // once and a build row too, so the order is the serial loop's to the row.
    // The table an outer join pads with NULLs: the probe of two tables', the last link of more.
    let padded_at = outer.map(|(at, _)| at);
    let (probe_at, outer) = match outer {
        None => {
            if built.is_empty() && searched.is_empty() {
                return None;
            }
            (
                (0..tables.len()).find(|at| !built.contains(at) && !searched.contains(at))?,
                None,
            )
        }
        Some((at, kind)) if tables.len() == 2 => {
            // Probing the kept table instead is the chain's (lane qs6-15).
            if built.contains(&at) || probe == crate::OuterJoinProbe::Kept {
                return None;
            }
            (at, Some(kind))
        }
        // A LEFT join among more tables (lane qs6-13): the padded table is the chain's last link,
        // probed per joined row of the others; the probe is one of the tables it keeps.
        Some((at, MorselOuter::Left)) => {
            let probe = (0..tables.len())
                .find(|t| *t != at && !built.contains(t) && !searched.contains(t))
                .or_else(|| (0..tables.len()).find(|t| *t != at && !searched.contains(t)))?;
            (probe, Some(MorselOuter::LeftLink))
        }
        Some(_) => return None,
    };

    // A column the hook reads: its table's place in the join order, and its own place.
    let side = |expr: &Expr| -> Option<(usize, usize)> {
        let (id, column) = match expr {
            Expr::Column {
                table: t, column, ..
            } => (*t, Some(*column)),
            Expr::RowId { table: t, .. } => (*t, None),
            _ => return None,
        };
        let at = joined.iter().position(|t| t.internal_id == id)?;
        let table = tables.get(at)?;
        // A table's rowid is its INTEGER PRIMARY KEY column, where it has one (lane qs6-6).
        let column = match column {
            Some(column) => column,
            None => rowid_alias(table)?,
        };
        let col = table.columns().get(column)?;
        let plain = matches!(col.affinity(), Affinity::Integer | Affinity::Text)
            && matches!(col.collation(), CollationSeq::Binary | CollationSeq::Unset);
        if plain {
            Some((at, column))
        } else {
            None
        }
    };
    let affinity = |(at, column): (usize, usize)| {
        tables
            .get(at)
            .and_then(|t| t.columns().get(column))
            .map(|c| c.affinity())
    };
    // Every term, consumed or not (one the planner made into a hash join's key still holds of
    // every row the join reads): an equality of two tables' columns, a filter on one table, or
    // a term the loop body tests on the joined row.
    let mut edges: Vec<((usize, usize), (usize, usize))> = Vec::new();
    let mut filters: Vec<Vec<Predicate>> = tables.iter().map(|_| Vec::new()).collect();
    let mut residual: Vec<&Expr> = Vec::new();
    let mut on_terms: Vec<&Expr> = Vec::new();
    for term in &plan.where_clause {
        if matches!(term.origin, WhereTermOrigin::TableFunction(_)) {
            return None;
        }
        // An outer join's ON terms decide which rows match; its WHERE terms test the joined row,
        // matched or padded (lane qs6-12). Only an ON equality keys an outer join.
        let on_term = outer.is_some() && term.origin.is_outer_join();
        if let Expr::Binary(left, Operator::Equals, right) = &term.expr {
            if let (Some(l), Some(r)) = (side(left), side(right)) {
                // Keys of one affinity compare with no conversion, as a hash table does; an
                // equality of two kinds is the body's to test.
                // An outer join's ON equality with its padded table keys that link; every other
                // equality with it (a WHERE term, or an ON term between the tables it keeps) is
                // the body's to test (lanes qs6-12 and qs6-13).
                let keys = padded_at.is_none_or(|p| on_term == (l.0 == p || r.0 == p));
                if l.0 != r.0 && affinity(l)? == affinity(r)? && keys {
                    edges.push((l, r));
                    continue;
                }
            }
        }
        let found = (0..tables.len()).find_map(|at| {
            let on = |expr: &Expr| side(expr).filter(|&(t, _)| t == at).map(|(_, c)| c);
            let table: &BTreeTable = tables.get(at)?;
            predicate(&term.expr, &on, table).map(|p| (at, p))
        });
        // A table's own term is tested on its rows when that drops no row the join keeps: in a
        // LEFT join, an ON term on the probe (a probe row failing it matches nothing, and the
        // probe's unmatched rows don't appear) and a WHERE term on the build (a build row failing
        // it fails it padded too); in a FULL join, none.
        let pushed = match (outer, found) {
            (_, None) => None,
            (None, found) => found,
            (Some(MorselOuter::Left | MorselOuter::LeftLink), Some((at, p)))
                if padded_at.is_some_and(|padded| on_term == (at == padded)) =>
            {
                Some((at, p))
            }
            (Some(_), Some(_)) => None,
        };
        match pushed {
            Some((at, found)) => filters.get_mut(at)?.push(found),
            None if on_term => on_terms.push(&term.expr),
            None => residual.push(&term.expr),
        }
    }
    // The chain: each table's place in the joined row once it is joined.
    let mut offsets: Vec<Option<usize>> = tables.iter().map(|_| None).collect();
    *offsets.get_mut(probe_at)? = Some(0);
    let probe_table: &BTreeTable = tables.get(probe_at)?;
    let mut width = probe_table.columns().len();
    let mut joins = Vec::new();
    while joins.len().saturating_add(1) < tables.len() {
        let joined_place = |at: usize| offsets.get(at).copied().flatten();
        // The next link: the first table in the join order an equality links to the chain; none
        // is a cross product, which the hook declines.
        // A LEFT join's padded table among more (lane qs6-13) links last.
        let last = joins.len().saturating_add(2) == tables.len();
        let next = (0..tables.len()).find(|at| {
            joined_place(*at).is_none()
                && (outer != Some(MorselOuter::LeftLink) || last || Some(*at) != padded_at)
                && edges.iter().any(|(l, r)| {
                    (l.0 == *at && joined_place(r.0).is_some())
                        || (r.0 == *at && joined_place(l.0).is_some())
                })
        })?;
        let mut probe_key = Vec::new();
        let mut build_key = Vec::new();
        for (l, r) in &edges {
            let (other, own) = if l.0 == next {
                (r, l)
            } else if r.0 == next {
                (l, r)
            } else {
                continue;
            };
            let Some(base) = joined_place(other.0) else {
                continue;
            };
            probe_key.push(base.checked_add(other.1)?);
            build_key.push(own.1);
        }
        let table: &BTreeTable = tables.get(next)?;
        *offsets.get_mut(next)? = Some(width);
        width = width.checked_add(table.columns().len())?;
        joins.push(MorselJoinData {
            root_page: table.root_page,
            columns: table.columns().len(),
            rowid_alias: rowid_alias(table),
            defaults: defaults(table),
            rowid_range: *ranges.get(next)?,
            filter: std::mem::take(filters.get_mut(next)?),
            probe_key,
            build_key,
            keeps: MorselKeeps::default(),
            searched: searched.contains(&next),
        });
    }
    // The body's row: each table's columns at its place in the chain, the probe's rowid its own,
    // a link's its INTEGER PRIMARY KEY (lane qs6-12).
    let mut places = Vec::new();
    for (at, table) in joined.iter().enumerate() {
        let own: &BTreeTable = tables.get(at)?;
        places.push(BodyTable {
            id: table.internal_id,
            table: own,
            offset: offsets.get(at).copied().flatten()?,
            rowid: if at == probe_at {
                RowidAt::Own
            } else {
                rowid_alias(own).map_or(RowidAt::Nowhere, RowidAt::Column)
            },
        });
    }
    let (
        BodyOutputs {
            outs,
            group_by,
            aggregates,
            result_map,
            agg_filters,
            ..
        },
        no_real,
        joined_rows,
    ) = join_outputs(plan, &tables, &side)?;
    body_reads(&on_terms, &places)?;
    body_reads(&residual, &places)?;
    body_reads(&outs, &places)?;
    body_reads(&filter_exprs(&agg_filters), &places)?;
    let data = MorselAggregateData {
        root_page: probe_table.root_page,
        columns: probe_table.columns().len(),
        rowid_alias: rowid_alias(probe_table),
        reals: Vec::new(),
        defaults: defaults(probe_table),
        rowid_range: *ranges.get(probe_at)?,
        filter: std::mem::take(filters.get_mut(probe_at)?),
        group_by: group_by.clone(),
        group_order: Vec::new(),
        aggregates: aggregates.clone(),
        result_map: result_map.clone(),
        joins,
        joined_rows,
        body: None,
        outer,
        target_pc_serial: crate::vdbe::BranchOffset::Placeholder,
    };
    let looped = BodyPlan {
        table: probe_table,
        places,
        width,
        on: on_terms,
        links_on: Vec::new(),
        filter: residual,
        outs,
        group_by,
        aggregates,
        no_real,
        result_map,
        agg_filters,
        rowid_range: (None, None),
        sorted: None,
        rows: rows_mode,
    };
    Some((data, looped))
}

/// A join's outputs (QT.6): the body's, the ORDER BY keys after the result columns when it
/// returns rows; the places where a REAL sends the statement to the serial loop (where a REAL
/// could tie with an INTEGER and differ: a SUM's input, a group's key values, a MIN's or a MAX's
/// input); and how its rows come out. `side` places a column in the join: its table's place in
/// `tables` and its own.
fn join_outputs<'p>(
    plan: &'p SelectPlan,
    tables: &[&BTreeTable],
    side: &dyn Fn(&Expr) -> Option<(usize, usize)>,
) -> Option<(BodyOutputs<'p>, Vec<usize>, Option<MorselRows>)> {
    let rows_mode = plan.aggregates.is_empty();
    let mut outputs = body_outputs(plan, rows_mode)?;
    let mut no_real = outputs.sums.clone();
    no_real.extend(outputs.group_by.iter().copied());
    for aggregate in &outputs.aggregates {
        if let Aggregate::Min(at) | Aggregate::Max(at) = aggregate {
            no_real.push(*at);
        }
    }
    if !rows_mode {
        return Some((outputs, no_real, None));
    }
    // The workers don't run the planner's loops, so an ORDER BY those give is the hook's to sort by.
    let order_by = if plan.order_by.is_empty() {
        &plan.loop_order
    } else {
        &plan.order_by
    };
    // A LIMIT or OFFSET cuts rows in order, so the hook takes one only when its rows' order is
    // the serial loop's to the row: the ORDER BY keys hold every table's INTEGER PRIMARY KEY,
    // which tell every joined row apart (QT.6, lanes qs6-6 and qs6-10), a padded row's NULL keys
    // too, since an outer join pads a row of the tables it keeps at most once (lane qs6-13).
    if plan.limit.is_some() || plan.offset.is_some() {
        let ordered = (0..tables.len()).all(|at| {
            tables
                .get(at)
                .and_then(|table| rowid_alias(table))
                .is_some_and(|alias| {
                    order_by
                        .iter()
                        .any(|(expr, _, _)| side(expr) == Some((at, alias)))
                })
        });
        if !ordered {
            return None;
        }
    }
    // The ORDER BY keys follow the result columns among the outputs: each a column of INTEGER or
    // TEXT affinity and BINARY collation, no NULLS FIRST or LAST named (SQLite's default, NULLs
    // least, is the values' own order), so the workers' sort is the serial sorter's.
    let width = outputs.outs.len();
    let mut order = Vec::new();
    for (expr, sort, nulls) in order_by {
        if nulls.is_some() {
            return None;
        }
        side(expr)?;
        order.push((outputs.outs.len(), matches!(sort, SortOrder::Desc)));
        outputs.outs.push(expr);
    }
    Some((outputs, no_real, Some(MorselRows { order, width })))
}

/// A column a join's hook reads (QT.6): its table's place in the FROM clause and its own place,
/// a rowid read as its table's INTEGER PRIMARY KEY column; only a column of INTEGER or TEXT
/// affinity and BINARY collation.
fn join_side(
    joined: &[JoinedTable],
    tables: &[&BTreeTable],
    expr: &Expr,
) -> Option<(usize, usize)> {
    let (id, column) = match expr {
        Expr::Column {
            table: t, column, ..
        } => (*t, Some(*column)),
        Expr::RowId { table: t, .. } => (*t, None),
        _ => return None,
    };
    let at = joined.iter().position(|t| t.internal_id == id)?;
    let table = tables.get(at)?;
    let column = match column {
        Some(column) => column,
        None => rowid_alias(table)?,
    };
    let col = table.columns().get(column)?;
    let plain = matches!(col.affinity(), Affinity::Integer | Affinity::Text)
        && matches!(col.collation(), CollationSeq::Binary | CollationSeq::Unset);
    if plain {
        Some((at, column))
    } else {
        None
    }
}

/// The last place in the FROM clause among the tables `expr` reads (0 when it reads none); none
/// when it reads a table the join doesn't hold.
fn reach(joined: &[JoinedTable], expr: &Expr) -> Option<usize> {
    let mut last = 0usize;
    let mut known = true;
    walk_expr(expr, &mut |e: &Expr| -> crate::Result<WalkControl> {
        if let Expr::Column { table, .. } | Expr::RowId { table, .. } = e {
            match joined.iter().position(|t| t.internal_id == *table) {
                Some(at) => last = last.max(at),
                None => known = false,
            }
        }
        Ok(WalkControl::Continue)
    })
    .ok()?;
    if known {
        Some(last)
    } else {
        None
    }
}

/// Joins of three or more tables with a RIGHT or FULL JOIN among them, or with more than one outer
/// join (lane qs6-14, T-QT.6.24 and T-QT.6.25; [`MorselOuter::Chain`]), with their aggregates or
/// their rows. The tables are joined in the FROM clause's order, as SQL states the joins: the
/// probe is the first, each other a link keyed by its own ON equalities with the tables before it
/// (columns of one affinity), what it keeps the join's (LEFT the left rows, RIGHT the right rows,
/// FULL both). Each table is a rowid table read whole (a scan, the planner's hash join, or a seek
/// in an index the statement builds from it).
///
/// Where each term runs:
/// - a link's own ON term over its table alone filters its build rows, unless the link keeps its
///   right rows (those that fail it still appear, padded); any other ON term of the link runs in
///   its ON code (`MorselBody::links_on`), over the row up to the link, and decides whether a
///   match passed. An ON term reading a table after its link declines the hook.
/// - a WHERE term acts where it can't change the joined rows a link sees: at the probe or an
///   inner link with no link after it that keeps its right rows (such a link's padded rows skip
///   the earlier tests, and every row reaching it marks its build rows whatever the WHERE terms
///   later say). There a term over one table filters that table's rows and an equality with an
///   earlier table keys an inner link; every other WHERE term runs over the whole joined row.
/// - a link with no key is a cross product, which the hook declines.
///
/// With the switch at [`crate::OuterJoinProbe::Kept`] (lane qs6-15) it takes a LEFT or FULL JOIN
/// of two tables too: the left table probes, and its rows no match passed are padded during the
/// walk (a FULL join's right rows that nothing matched still come from the link's bitmap).
fn shape_chain(
    plan: &SelectPlan,
    probe: crate::OuterJoinProbe,
    searched_links: bool,
) -> Option<(MorselAggregateData, BodyPlan<'_>)> {
    let rows_mode = plan.aggregates.is_empty();
    if !(if rows_mode {
        plain_rows(plan)
    } else {
        plain(plan)
    }) {
        return None;
    }
    let joined = plan.table_references.joined_tables();
    let mut keeps = Vec::new();
    let mut tables: Vec<&BTreeTable> = Vec::new();
    // Each table's rowid range, `(None, None)` where it is read whole (lane qs6-17).
    let mut ranges: Vec<(Option<i64>, Option<i64>)> = Vec::new();
    // The tables the planner reads by a seek, links read whole (lane qs6-16).
    let mut searched = Vec::new();
    for (at, table) in joined.iter().enumerate() {
        let kept = match table.join_info.as_ref().map(|j| &j.join_type) {
            None | Some(JoinType::Inner) => MorselKeeps::default(),
            Some(JoinType::LeftOuter) => MorselKeeps {
                left: true,
                right: false,
            },
            Some(JoinType::RightOuter) => MorselKeeps {
                left: false,
                right: true,
            },
            Some(JoinType::FullOuter) => MorselKeeps {
                left: true,
                right: true,
            },
            Some(_) => return None,
        };
        // The first table joins nothing: it is the probe.
        if (at == 0 && kept != MorselKeeps::default())
            || table.join_info.as_ref().is_some_and(|j| j.lateral)
        {
            return None;
        }
        match &table.op {
            Operation::Scan(Scan::BTreeTable { index: None, .. }) => {}
            Operation::HashJoin(op)
                if op.method == crate::translate::plan::HashJoinMethod::Hash => {}
            // As in `shape_join` (lane qs6-17).
            op if scanned_rowids(op).is_some() => {}
            Operation::Search(Search::Seek {
                index: Some(index), ..
            }) if index.ephemeral => {}
            op if searched_by_seek(op, at, searched_links) => searched.push(at),
            _ => return None,
        }
        keeps.push(kept);
        tables.push(stored(table)?);
        ranges.push(scanned_rowids(&table.op).unwrap_or((None, None)));
    }
    // As in `shape_join`: in an outer join a range is read whole.
    if keeps.iter().any(|kept| *kept != MorselKeeps::default()) {
        ranges.fill((None, None));
    }
    // Only what the other join shapes don't take: two or more outer joins, or a RIGHT or FULL
    // JOIN among three or more tables; and a LEFT or FULL JOIN of two tables when the switch
    // says its kept table probes (lane qs6-15).
    let outer = keeps.iter().filter(|k| k.left || k.right).count();
    let right = keeps.iter().any(|k| k.right);
    let two_kept = probe == crate::OuterJoinProbe::Kept
        && tables.len() == 2
        && keeps.get(1).is_some_and(|k| k.left);
    if !(outer >= 2 || (right && tables.len() > 2) || two_kept) {
        return None;
    }
    let last_right = keeps.iter().rposition(|k| k.right);
    // Where a WHERE term may act: the probe or an inner link, with no link after it that keeps
    // its right rows.
    let where_at = |at: usize| {
        keeps.get(at).is_some_and(|k| !k.left && !k.right)
            && last_right.is_none_or(|last| at > last)
    };
    let side = |expr: &Expr| join_side(joined, &tables, expr);
    let affinity = |(at, column): (usize, usize)| {
        tables
            .get(at)
            .and_then(|t| t.columns().get(column))
            .map(|c| c.affinity())
    };
    // (the later table's column, the earlier table's) for each key.
    let mut edges: Vec<((usize, usize), (usize, usize))> = Vec::new();
    let mut filters: Vec<Vec<Predicate>> = tables.iter().map(|_| Vec::new()).collect();
    let mut links_on: Vec<Vec<&Expr>> = tables.iter().map(|_| Vec::new()).collect();
    let mut residual: Vec<&Expr> = Vec::new();
    for term in &plan.where_clause {
        if matches!(term.origin, WhereTermOrigin::TableFunction(_)) {
            return None;
        }
        // An inferred equality fails a row a later link padded (T-QT.6.24: `u.a = v.a` from
        // `u.a = t.a` and `v.a = t.a`, over a FULL JOIN's unmatched right row). The stated terms
        // it came from are tested where they hold.
        if term.origin == WhereTermOrigin::Implied {
            continue;
        }
        // An ON term's link: its join's right table; a WHERE term has none.
        let link = match term.origin.join_origin() {
            Some(origin) => {
                let at = joined
                    .iter()
                    .position(|t| t.internal_id == origin.right_table())?;
                if at == 0 || reach(joined, &term.expr)? > at {
                    return None;
                }
                Some(at)
            }
            None => None,
        };
        if let Expr::Binary(left, Operator::Equals, right) = &term.expr {
            if let (Some(l), Some(r)) = (side(left), side(right)) {
                let (later, earlier) = if l.0 > r.0 { (l, r) } else { (r, l) };
                let keys = match link {
                    Some(link) => link == later.0,
                    None => where_at(later.0),
                };
                if later.0 != earlier.0 && affinity(l)? == affinity(r)? && keys {
                    edges.push((later, earlier));
                    continue;
                }
            }
        }
        let found = (0..tables.len()).find_map(|at| {
            let on = |expr: &Expr| side(expr).filter(|&(t, _)| t == at).map(|(_, c)| c);
            let table: &BTreeTable = tables.get(at)?;
            predicate(&term.expr, &on, table).map(|p| (at, p))
        });
        let pushed = found.filter(|(at, _)| match link {
            Some(link) => link == *at && !keeps.get(*at).is_some_and(|k| k.right),
            None => where_at(*at),
        });
        match (pushed, link) {
            (Some((at, found)), _) => filters.get_mut(at)?.push(found),
            (None, Some(link)) => links_on.get_mut(link)?.push(&term.expr),
            (None, None) => residual.push(&term.expr),
        }
    }
    // The chain: the tables in the FROM clause's order, each one's columns after the last's.
    let mut offsets = Vec::new();
    let mut width = 0usize;
    for table in &tables {
        offsets.push(width);
        width = width.checked_add(table.columns().len())?;
    }
    let mut joins = Vec::new();
    for (at, table) in tables.iter().enumerate().skip(1) {
        let mut probe_key = Vec::new();
        let mut build_key = Vec::new();
        for (later, earlier) in edges.iter().filter(|(later, _)| later.0 == at) {
            probe_key.push(offsets.get(earlier.0)?.checked_add(earlier.1)?);
            build_key.push(later.1);
        }
        if probe_key.is_empty() {
            return None;
        }
        joins.push(MorselJoinData {
            root_page: table.root_page,
            columns: table.columns().len(),
            rowid_alias: rowid_alias(table),
            defaults: defaults(table),
            rowid_range: *ranges.get(at)?,
            filter: std::mem::take(filters.get_mut(at)?),
            probe_key,
            build_key,
            keeps: *keeps.get(at)?,
            searched: searched.contains(&at),
        });
    }
    // The body's row: each table's columns at its place, the probe's rowid its own (NULL in a
    // row padded for it), a link's its INTEGER PRIMARY KEY.
    let mut places = Vec::new();
    for (at, table) in joined.iter().enumerate() {
        let own: &BTreeTable = tables.get(at)?;
        places.push(BodyTable {
            id: table.internal_id,
            table: own,
            offset: *offsets.get(at)?,
            rowid: if at == 0 {
                RowidAt::Own
            } else {
                rowid_alias(own).map_or(RowidAt::Nowhere, RowidAt::Column)
            },
        });
    }
    let (
        BodyOutputs {
            outs,
            group_by,
            aggregates,
            result_map,
            agg_filters,
            ..
        },
        no_real,
        joined_rows,
    ) = join_outputs(plan, &tables, &side)?;
    for on in &links_on {
        body_reads(on, &places)?;
    }
    body_reads(&residual, &places)?;
    body_reads(&outs, &places)?;
    body_reads(&filter_exprs(&agg_filters), &places)?;
    let probe_table: &BTreeTable = tables.first()?;
    let data = MorselAggregateData {
        root_page: probe_table.root_page,
        columns: probe_table.columns().len(),
        rowid_alias: rowid_alias(probe_table),
        reals: Vec::new(),
        defaults: defaults(probe_table),
        rowid_range: *ranges.first()?,
        filter: std::mem::take(filters.first_mut()?),
        group_by: group_by.clone(),
        group_order: Vec::new(),
        aggregates: aggregates.clone(),
        result_map: result_map.clone(),
        joins,
        joined_rows,
        body: None,
        outer: Some(MorselOuter::Chain),
        target_pc_serial: crate::vdbe::BranchOffset::Placeholder,
    };
    let looped = BodyPlan {
        table: probe_table,
        places,
        width,
        on: Vec::new(),
        // A list per link: the probe's place holds none.
        links_on: links_on.into_iter().skip(1).collect(),
        filter: residual,
        outs,
        group_by,
        aggregates,
        no_real,
        result_map,
        agg_filters,
        rowid_range: (None, None),
        sorted: None,
        rows: rows_mode,
    };
    Some((data, looped))
}
