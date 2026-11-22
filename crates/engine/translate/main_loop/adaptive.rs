//! Amphora (PL.13; `docs/design/planner.md`, "Decision (lane pl13)"): an index nested-loop join
//! emitted with a hash join beside it, and the switch between them (`optimizer::adaptive`).
//!
//! The inner side's equality seek stays as planned. Ahead of it, `JoinSwitchStep` counts the outer
//! row: before the learned costs' break-even it falls through to the seek; at it, it jumps once to
//! a build of a hash table over the inner table keyed on the seek's columns, and from then on to a
//! probe of that table. A probe's match positions the index cursor on the very entry the seek would
//! have found (its key, the index's other columns and the rowid, all held in the table), so both loops share one body from the deferred table seek on,
//! and the close reads the next match from the hash table in place of the index's next entry.
//! Rows joined by seeks before the switch are not joined again: the switch comes between outer rows.
//!
//! The build is where the inner table is held whole, and so where the plan's estimate of it meets
//! its count (the re-plan, `optimizer::adaptive::MaterializePoint`): past the estimate's own error,
//! `JoinSwitchBuilt` sets the table's row counter (PL.9) to the rows counted, and the statement is
//! prepared again on them before its next run, as is every other statement planned on that table.

use super::*;
use crate::translate::emitter::AdaptiveJoinCtx;
use crate::translate::optimizer::adaptive::switch_point;
use crate::translate::optimizer::learned_costs::LearnedWeights;
use crate::vdbe::insn::{JoinSwitchBuiltData, JoinSwitchData};

/// ANALYZE's count of a table's rows and its statistics' relative standard error; `None` without
/// statistics.
fn analyzed_table(resolver: &Resolver, table: &str) -> Option<(f64, f64)> {
    let stats = resolver.schema().analyze_stats.table_stats(table)?;
    let analyzed_rows = stats
        .row_count
        .or_else(|| stats.index_stats.values().find_map(|stat| stat.total_rows))?
        as f64;
    Some((
        analyzed_rows,
        crate::row_changes::statistics_error(stats, analyzed_rows),
    ))
}

/// Amphora (PL.13, a materialization point): after a planner-chosen hash join's build, the check
/// of its rows against the plan's estimate: adaptive joins on, the table with statistics, and its
/// input the table itself (`filter_terms` is `None` for a materialized input). Unfiltered, the
/// build holds the table whole and counts its rows. Filtered (lane replan-5; `filter_terms` holds
/// whether the build has terms of its own and their fingerprint), it counts the share of the table
/// they keep, checked where the plan's estimate read a share for exactly those terms
/// (`PlanValues::filter_selectivity`): a build whose terms the sample did not run together, or
/// that reads a parameter, is not checked.
pub(super) fn emit_build_count(
    program: &mut ProgramBuilder,
    t_ctx: &TranslateCtx,
    table: &crate::schema::BTreeTable,
    hash_table_id: usize,
    filter_terms: Option<(bool, Option<u64>)>,
) {
    let Some((prefiltered, fingerprint)) = filter_terms else {
        return;
    };
    let Some((analyzed_rows, estimate_error, filter)) =
        counted_check(t_ctx, table, prefiltered, fingerprint)
    else {
        return;
    };
    program.emit_insn(Insn::BuildCounted {
        data: Box::new(crate::vdbe::insn::BuildCountedData {
            hash_table_id,
            table: table.name.clone(),
            analyzed_rows,
            estimate_error,
            filter,
        }),
    });
}

/// What a materialization point over one table's rows is checked against: ANALYZE's count of the
/// table, its error, and for a filtered input (`prefiltered`, its terms' `fingerprint`) the share
/// the plan read for exactly those terms. `None` where it is not checked: adaptive joins off, no
/// statistics, no rowid, or filter terms the plan's estimate did not read a share for.
fn counted_check(
    t_ctx: &TranslateCtx,
    table: &crate::schema::BTreeTable,
    prefiltered: bool,
    fingerprint: Option<u64>,
) -> Option<(f64, f64, Option<crate::vdbe::insn::FilteredBuild>)> {
    if !t_ctx.resolver.adaptive_joins() || !table.has_rowid {
        return None;
    }
    let (analyzed_rows, estimate_error) = analyzed_table(&t_ctx.resolver, &table.name)?;
    let filter = match (prefiltered, fingerprint) {
        (false, _) => None,
        (true, None) => return None,
        (true, Some(fingerprint)) => {
            let planned = t_ctx
                .resolver
                .plan_values()
                .and_then(|values| values.planned_filter(&table.name, fingerprint))?;
            // The share's error and the table count's, independent, add in quadrature.
            let error = planned.share_error().hypot(estimate_error);
            Some(crate::vdbe::insn::FilteredBuild {
                share: planned.share,
                planned_share: planned.planned_share,
                error,
            })
        }
    };
    Some((analyzed_rows, estimate_error, filter))
}

/// Amphora (PL.13, lane replan-5: the sorter's materialization point): an ORDER BY sorter fed by a
/// full scan of one table, nothing grouped, aggregated, windowed or made distinct in between, holds
/// the rows of that table its WHERE terms keep. Once the scan has put them all in, ahead of the
/// sort, their count is checked as a hash join's build is: against the table's rows, or with WHERE
/// terms against the share the plan read for exactly those terms (`counted_check`).
///
/// Fed by an index seek on the table (lane replan-9), it holds the rows the seek's terms and the
/// rest keep: every WHERE term counts, the seek's (consumed by it) among them, so a seek by one
/// `=` or IN list the column statistics estimate is checked against the share they gave.
///
/// A GROUP BY's sorter (`grouped`, lane replan-10) holds the same rows, one a row read, before
/// they are grouped: its input is the table's rows its WHERE terms keep, checked and corrected the
/// same way (the table's row counter, or the filter's stored share), and the GROUP BY's estimate is
/// made from them again. Only a sorter that takes every row: one beside a hash table of groups
/// (`GroupByRowSource::Sorter`'s `hash`) takes only the rows of groups past its budget, and is
/// not checked; nor is a GROUP BY under a window.
///
/// After a SELECT DISTINCT (lane replan-12), the sorter holds the distinct rows, not the rows
/// read: they are checked against the DISTINCT's own count ([distinct_sorter_check]).
pub(crate) fn emit_sorter_count(
    program: &mut ProgramBuilder,
    t_ctx: &TranslateCtx,
    plan: &crate::translate::plan::SelectPlan,
    cursor_id: CursorID,
    grouped: bool,
) {
    use crate::translate::plan::{Distinctness, Operation, Scan, Search};
    // An ORDER BY sorter after a GROUP BY or an aggregate holds groups, not the rows read.
    if (!grouped && (plan.group_by.is_some() || !plan.aggregates.is_empty()))
        || (grouped && plan.group_by.is_none())
        || plan.window.is_some()
        || plan.contains_constant_false_condition
    {
        return;
    }
    // PL.13 (lane replan-12): after a DISTINCT, the sorter holds its distinct rows, checked against
    // the DISTINCT's own count.
    if !matches!(plan.distinctness, Distinctness::NonDistinct) {
        if grouped {
            return;
        }
        if let Some((table, check)) = distinct_sorter_check(t_ctx, plan) {
            program.emit_insn(Insn::SorterCounted {
                data: Box::new(crate::vdbe::insn::SorterCountedData {
                    cursor_id,
                    table,
                    analyzed_rows: 0.0,
                    estimate_error: check.estimate_error,
                    filter: None,
                    joined: Some(check),
                    distinct: true,
                }),
            });
        }
        return;
    }
    let joined = match plan.joined_tables() {
        [joined] => joined,
        [_, _, ..] => {
            if let Some((table, check)) = joined_sorter_check(t_ctx, plan) {
                program.emit_insn(Insn::SorterCounted {
                    data: Box::new(crate::vdbe::insn::SorterCountedData {
                        cursor_id,
                        table,
                        analyzed_rows: 0.0,
                        estimate_error: check.estimate_error,
                        filter: None,
                        joined: Some(check),
                        distinct: false,
                    }),
                });
            }
            return;
        }
        [] => return,
    };
    let (seek, btree) = match (&joined.op, &joined.table) {
        (Operation::Scan(Scan::BTreeTable { index: None, .. }), Table::BTree(btree)) => {
            (false, btree)
        }
        (
            Operation::Search(
                Search::Seek { index: Some(_), .. } | Search::InSeek { index: Some(_), .. },
            ),
            Table::BTree(btree),
        ) => (true, btree),
        _ => return,
    };
    let mut terms = plan
        .where_clause
        .iter()
        .filter(|term| seek || !term.consumed)
        .peekable();
    let prefiltered = terms.peek().is_some();
    let fingerprint = crate::filter_shares::fingerprint(terms.map(|term| &term.expr));
    let Some((analyzed_rows, estimate_error, filter)) =
        counted_check(t_ctx, btree, prefiltered, fingerprint)
    else {
        return;
    };
    program.emit_insn(Insn::SorterCounted {
        data: Box::new(crate::vdbe::insn::SorterCountedData {
            cursor_id,
            table: btree.name.clone(),
            analyzed_rows,
            estimate_error,
            filter,
            joined: None,
            distinct: false,
        }),
    });
}

/// Amphora (PL.13, lane replan-10: a sorter fed by a join): an ORDER BY or GROUP BY sorter fed by
/// an inner join of two tables or more, or a LEFT, semi or anti join (lane replan-11: a LEFT
/// JOIN's unmatched rows kept, null-extended; the rows an EXISTS or NOT EXISTS keeps), holds the
/// join's rows its WHERE and ON terms keep, one a row. They are checked against the rows the plan
/// read for the whole join (`cardinality::JoinedSetRows::rows`), noted under the join's identity
/// (`subquery_rows::join_identity`), where a count past them is stored for the planner. The band
/// is the statistics' error of each table joined, independent, in quadrature, as a stored
/// subquery result's over several tables is. `None` where it is not checked: adaptive joins off,
/// a RIGHT or FULL JOIN, a table without statistics, terms reading a parameter, or no rows noted
/// for the join. With the check, the name of the first table, for EXPLAIN.
fn joined_sorter_check(
    t_ctx: &TranslateCtx,
    plan: &crate::translate::plan::SelectPlan,
) -> Option<(String, crate::vdbe::insn::CountedCheck)> {
    if !t_ctx.resolver.adaptive_joins() {
        return None;
    }
    let tables = plan.joined_tables();
    // A LEFT, semi or anti join is checked (lane replan-11); a RIGHT or FULL JOIN, whose
    // unmatched right rows come from a read of their own, is not.
    if tables.iter().any(|joined| {
        joined
            .join_info
            .as_ref()
            .is_some_and(|info| info.keeps_right_rows())
    }) {
        return None;
    }
    let identity = crate::subquery_rows::join_identity(tables, &plan.where_clause)?;
    let planned = t_ctx
        .resolver
        .plan_values()
        .and_then(|values| values.planned_subquery(identity))?;
    let mut squared_error = 0.0;
    let mut first = None;
    for joined in tables {
        let Table::BTree(btree) = &joined.table else {
            return None;
        };
        let (_, error) = analyzed_table(&t_ctx.resolver, &btree.name)?;
        squared_error += error * error;
        first.get_or_insert_with(|| btree.name.clone());
    }
    Some((
        first?,
        crate::vdbe::insn::CountedCheck {
            rows: planned.rows,
            planned_rows: planned.planned_rows,
            estimate_error: f64::sqrt(squared_error),
        },
    ))
}

/// Amphora (PL.13, lane replan-12: a DISTINCT sorter's own count): an ORDER BY sorter after a
/// SELECT DISTINCT takes a row only once the DISTINCT has dropped its duplicates
/// (`EmitOrderBy::sorter_insert`), so it holds the DISTINCT's rows, not the rows read: they
/// are checked against the rows the plan read for the DISTINCT (`optimizer::distinct_rows`), noted
/// under its identity (`subquery_rows::distinct_identity`), where a count past them is stored for
/// the planner. The estimate is the rows read, so the band is the statistics' error of each table
/// read, independent, in quadrature, as a stored subquery result's ([tables_error]). `None` where
/// it is not checked: adaptive joins off, a table without statistics, a term or column reading a
/// parameter, or no rows noted for the DISTINCT. With the check, the name of the first table, for
/// EXPLAIN.
fn distinct_sorter_check(
    t_ctx: &TranslateCtx,
    plan: &crate::translate::plan::SelectPlan,
) -> Option<(String, crate::vdbe::insn::CountedCheck)> {
    if !t_ctx.resolver.adaptive_joins() {
        return None;
    }
    let identity = crate::subquery_rows::distinct_identity(
        plan.joined_tables(),
        &plan.where_clause,
        &plan.result_columns,
    )?;
    let planned = t_ctx
        .resolver
        .plan_values()
        .and_then(|values| values.planned_subquery(identity))?;
    let estimate_error = tables_error(&t_ctx.resolver, plan)?;
    let first = plan.joined_tables().first()?.table.get_name().to_string();
    Some((
        first,
        crate::vdbe::insn::CountedCheck {
            rows: planned.rows,
            planned_rows: planned.planned_rows,
            estimate_error,
        },
    ))
}

/// Amphora (PL.13, lane replan-6: a stored subquery result's materialization point): an
/// uncorrelated FROM-clause subquery's result, once its fill has stored it whole, holds the rows
/// the plan estimated. Its rows are counted there and checked against the rows the plan read for
/// the subquery (`PlanValues::subquery_rows`: its estimate, or a count a fill stored).
///
/// The band is the estimate's own error. A GROUP BY whose rows ANALYZE's group count set is
/// checked against that count's error (lane replan-8, `optimizer::group_count_error`). Any other
/// estimate is the rows read: the statistics' error of each table the subquery reads, independent,
/// in quadrature; a subquery reading anything else (another subquery, a table without statistics)
/// or no table is not checked, as a build without statistics is not.
///
/// Where the result is stored decides the count. In its ephemeral table
/// (`MaterializedFromClauseSubqueryStorage::TableBacked`), the fill numbers its rows 1, 2, ...
/// (`EphemeralRowidMode::Auto`'s `NewRowid` on a table it only inserts into), so the last row's
/// rowid is the count, read without a scan. In its seek index (`DirectIndex`, lane replan-8) there
/// is no rowid to read: `Count` walks the index's pages once, each page's cells summed, no row
/// decoded. The seek index holds an entry for each row (`ephemeral_subquery_*` has a rowid: a
/// sequence column keeps equal rows apart), so its entries are the rows.
pub(crate) fn emit_subquery_count(
    program: &mut ProgramBuilder,
    resolver: &Resolver,
    plan: &crate::translate::plan::SelectPlan,
    identity: Option<u64>,
    cursor_id: CursorID,
    storage: crate::translate::subquery::MaterializedFromClauseSubqueryStorage,
) {
    if let Some(check) = subquery_count_check(resolver, plan, identity) {
        emit_subquery_counted(program, check, cursor_id, storage);
    }
}

/// What a stored subquery result is checked against: the rows the plan read for the subquery
/// `identity` names, and the band's error. Taken from the subquery's plan, so it can be taken
/// before the plan is emitted (an IN subquery's, lane replan-10, is consumed by its emission).
pub(crate) struct SubqueryCheck {
    planned: crate::subquery_rows::PlannedSubquery,
    estimate_error: f64,
}

/// [emit_subquery_count]'s check, `None` where none is made: adaptive joins off, no identity, no
/// rows the plan read for it, or an estimate whose error the statistics do not give.
pub(crate) fn subquery_count_check(
    resolver: &Resolver,
    plan: &crate::translate::plan::SelectPlan,
    identity: Option<u64>,
) -> Option<SubqueryCheck> {
    if !resolver.adaptive_joins() {
        return None;
    }
    let planned = identity.and_then(|identity| {
        resolver
            .plan_values()
            .and_then(|values| values.planned_subquery(identity))
    })?;
    let estimate_error =
        match crate::translate::optimizer::group_count_error(plan, resolver.schema()) {
            Some(error) => error,
            None => tables_error(resolver, plan)?,
        };
    Some(SubqueryCheck {
        planned,
        estimate_error,
    })
}

/// Amphora (PL.13, lane replan-11: a build fed by a materialized input): a hash join's build input
/// materialized from a join prefix (`emitter::emit_materialized_build_inputs`, key and payload
/// stored, one row of the ephemeral table a row of the prefix) holds the rows of the tables
/// `prefix` names that every WHERE term reading only them keeps. Once filled they are checked
/// against the rows the plan noted for the prefix (`optimizer::note_materialized_prefixes`, under
/// `subquery_rows::prefix_identity`), as a stored subquery result is: the fill numbers its rows
/// 1, 2, ... (`EphemeralRowidMode::Auto`), so the last rowid is the count. The band is each
/// prefix table's statistics' error, independent, in quadrature, as for a sorter fed by a join.
/// `None` where it is not checked: adaptive joins off, a table without statistics, terms reading
/// a parameter, or no rows noted for the prefix (an outer, semi or anti join in the query, a plan
/// for more than one run, a bushy plan).
pub(crate) fn join_prefix_check(
    resolver: &Resolver,
    plan: &crate::translate::plan::SelectPlan,
    prefix: &crate::translate::planner::TableMask,
) -> Option<SubqueryCheck> {
    if !resolver.adaptive_joins() {
        return None;
    }
    let identity = crate::subquery_rows::prefix_identity(
        &plan.table_references,
        &plan.non_from_clause_subqueries,
        &plan.where_clause,
        prefix,
    )?;
    let planned = resolver
        .plan_values()
        .and_then(|values| values.planned_subquery(identity))?;
    let tables = plan.joined_tables();
    let mut squared_error = 0.0;
    for table_idx in prefix.iter() {
        let Table::BTree(btree) = &tables.get(table_idx)?.table else {
            return None;
        };
        let (_, error) = analyzed_table(resolver, &btree.name)?;
        squared_error += error * error;
    }
    Some(SubqueryCheck {
        planned,
        estimate_error: f64::sqrt(squared_error),
    })
}

/// The relative error of the rows a query reads, its tables' statistics' errors, independent, in
/// quadrature; `None` where one has none (a table without statistics, a table-valued function), or
/// for a query reading no table. A FROM-clause subquery it reads (lane replan-10) adds its own:
/// its GROUP BY's group count error where ANALYZE's group count set its rows, else the errors of
/// the tables it reads, taken from a worklist of the plans still to read, bounded by the plan's own
/// size (no recursion at run time).
fn tables_error(resolver: &Resolver, plan: &crate::translate::plan::SelectPlan) -> Option<f64> {
    let mut squared_error = 0.0;
    let mut pending = vec![plan];
    while let Some(select) = pending.pop() {
        let tables = select.joined_tables();
        if tables.is_empty() {
            return None;
        }
        for joined in tables {
            match &joined.table {
                Table::BTree(btree) => {
                    let (_, error) = analyzed_table(resolver, &btree.name)?;
                    squared_error += error * error;
                }
                Table::FromClauseSubquery(subquery) => {
                    let crate::translate::plan::Plan::Select(inner) = subquery.plan.as_ref() else {
                        return None;
                    };
                    match crate::translate::optimizer::group_count_error(inner, resolver.schema()) {
                        Some(error) => squared_error += error * error,
                        None => pending.push(inner),
                    }
                }
                _ => return None,
            }
        }
    }
    Some(f64::sqrt(squared_error))
}

/// Counts a stored subquery result's rows where `storage` keeps them and checks them
/// ([SubqueryCheck]). An IN subquery's ephemeral index (lane replan-10) is a seek index without a
/// rowid whose inserts skip a key it holds (`IdxInsertFlags::no_op_duplicate`): its entries are the
/// result's distinct values, counted as `DirectIndex`'s are.
pub(crate) fn emit_subquery_counted(
    program: &mut ProgramBuilder,
    check: SubqueryCheck,
    cursor_id: CursorID,
    storage: crate::translate::subquery::MaterializedFromClauseSubqueryStorage,
) {
    use crate::translate::subquery::MaterializedFromClauseSubqueryStorage;
    let rows_reg = program.alloc_register();
    match storage {
        MaterializedFromClauseSubqueryStorage::TableBacked => {
            let counted = program.allocate_label();
            program.emit_insn(Insn::Integer {
                value: 0,
                dest: rows_reg,
            });
            program.emit_insn(Insn::Last {
                cursor_id,
                pc_if_empty: counted,
            });
            program.emit_insn(Insn::RowId {
                cursor_id,
                dest: rows_reg,
            });
            program.preassign_label_to_next_insn(counted);
        }
        MaterializedFromClauseSubqueryStorage::DirectIndex => {
            program.emit_insn(Insn::Count {
                cursor_id,
                target_reg: rows_reg,
                exact: false,
            });
        }
    }
    program.emit_insn(Insn::SubqueryCounted {
        data: Box::new(crate::vdbe::insn::SubqueryCountedData {
            rows_reg,
            rows: check.planned.rows,
            planned_rows: check.planned.planned_rows,
            estimate_error: check.estimate_error,
        }),
    });
}

/// An inner index seek that may switch to a hash join, with the switch's inputs.
pub(super) struct AdaptiveSeek<'a> {
    index: &'a Arc<Index>,
    seek_def: &'a SeekDef,
    weights: LearnedWeights,
    build_rows: f64,
    /// ANALYZE's count of the inner table's rows, and its statistics' relative standard error.
    analyzed_rows: f64,
    estimate_error: f64,
    index_cursor_id: CursorID,
}

impl<'a> AdaptiveSeek<'a> {
    /// The seek, if it can switch: an inner join's forward equality seek on every column of a
    /// prefix of a table's own index (no range, no skip, no `IS`), with the learned costs and the
    /// table's counted rows known, and a break-even under them. Anything else stays as planned.
    pub(super) fn eligible(
        t_ctx: &TranslateCtx,
        table: &JoinedTable,
        search: &'a Search,
        index_cursor_id: Option<CursorID>,
    ) -> Option<Self> {
        if !t_ctx.resolver.adaptive_joins() {
            return None;
        }
        if table.join_info.as_ref().is_some_and(|info| {
            info.is_outer() || info.is_semi_or_anti() || info.keeps_right_rows()
        }) {
            return None;
        }
        let Search::Seek {
            index: Some(index),
            seek_def,
            ..
        } = search
        else {
            return None;
        };
        let Table::BTree(btree) = &table.table else {
            return None;
        };
        let plain_equality = seek_def.skip == 0
            && !seek_def.prefix.is_empty()
            && seek_def.iter_dir == IterationDirection::Forwards
            && matches!(seek_def.start.last_component, SeekKeyComponent::None)
            && matches!(seek_def.end.last_component, SeekKeyComponent::None)
            && seek_def.prefix.iter().all(|constraint| {
                matches!(
                    constraint.eq,
                    Some((amphora_engine_parser::ast::Operator::Equals, _, _))
                )
            });
        if !btree.has_rowid
            || index.ephemeral
            || index.index_method.is_some()
            || !plain_equality
            || seek_def.prefix.len() > index.columns.len()
            || index.columns.iter().any(|column| column.expr.is_some())
        {
            return None;
        }
        let weights = t_ctx.resolver.learned_weights()?;
        let (analyzed_rows, estimate_error) = analyzed_table(&t_ctx.resolver, &btree.name)?;
        // The rows the plan counts the table at (PL.9): ANALYZE's with the changes since.
        let build_rows = match t_ctx.resolver.plan_values() {
            Some(values) => values.live_rows(&btree.name, analyzed_rows, estimate_error),
            None => analyzed_rows,
        };
        // No break-even (a probe costs a seek or more): the join never switches, so none is emitted.
        switch_point(&weights, build_rows).ok()??;
        Some(Self {
            index,
            seek_def,
            weights,
            build_rows,
            analyzed_rows,
            estimate_error,
            index_cursor_id: index_cursor_id?,
        })
    }

    /// Emits the switch, the hash table's build and its probe ahead of the seek; records what the
    /// close needs. The seek's own code follows it; the caller anchors the label returned, the
    /// shared body, at the seek's deferred table seek.
    pub(super) fn emit_prelude(
        &self,
        program: &mut ProgramBuilder,
        t_ctx: &mut TranslateCtx,
        table_references: &TableReferences,
        table: &JoinedTable,
        joined_table_index: usize,
        loop_end: BranchOffset,
    ) -> Result<BranchOffset> {
        let num_keys = self.seek_def.prefix.len();
        let key_columns = &self.index.columns[..num_keys];
        // The index's other columns ride in the hash table beside the rowid, so a match finds its
        // index entry by the whole of it: key, other columns, rowid.
        let other_columns = &self.index.columns[num_keys..];
        let num_payload = other_columns.len();
        let entry_len = num_keys + num_payload + 1;
        // Its own id: a hash join the plan chose keys its table by its build table's id.
        let hash_table_id = program.alloc_hash_table_id();
        let switch_id = hash_table_id;
        let affinities = super::seek::index_seek_affinities(self.seek_def, &self.seek_def.start);
        let collations: Vec<CollationSeq> = key_columns
            .iter()
            .map(|column| column.collation.unwrap_or_default())
            .collect();
        let build = program.allocate_label();
        let probe = program.allocate_label();
        let seek = program.allocate_label();
        let body = program.allocate_label();
        let found = program.allocate_label();

        // The bytes the hash table would hold, by the statistics: each row's key values and rowid.
        // Value's in-memory size is a format value (`size_of`); the table's own spill check
        // (`JoinSwitchBuilt` below) bounds what this estimate misses.
        let entry_bytes = entry_len as u64 * std::mem::size_of::<crate::Value>() as u64;
        let build_bytes = (self.build_rows.max(0.0) as u64).saturating_mul(entry_bytes);
        program.emit_insn(Insn::JoinSwitchStep {
            data: Box::new(JoinSwitchData {
                switch_id,
                seek: self.weights.seek,
                hash_build: self.weights.hash_build,
                hash_probe: self.weights.hash_probe,
                build_rows: self.build_rows,
                build_bytes,
                build_pc: build,
                probe_pc: probe,
            }),
        });
        program.emit_insn(Insn::Goto { target_pc: seek });

        // The build, once: every row of the inner table, keyed on the seek's columns.
        program.preassign_label_to_next_insn(build);
        let Table::BTree(btree) = &table.table else {
            crate::bail_parse_error!("an adaptive join's inner side must be a table");
        };
        let build_cursor_id = program.alloc_cursor_id_keyed_if_not_exists(
            CursorKey::hash_build(table.internal_id),
            CursorType::BTreeTable(btree.clone()),
        );
        program.emit_insn(Insn::OpenRead {
            cursor_id: build_cursor_id,
            root_page: btree.root_page,
            db: table.database_id,
        });
        let built = program.allocate_label();
        let build_row = program.allocate_label();
        program.emit_insn(Insn::Rewind {
            cursor_id: build_cursor_id,
            pc_if_empty: built,
        });
        program.preassign_label_to_next_insn(build_row);
        let build_keys = program.alloc_registers(num_keys + num_payload);
        for (i, column) in key_columns.iter().chain(other_columns).enumerate() {
            program.emit_column_or_rowid(build_cursor_id, column.pos_in_table, build_keys + i);
        }
        emit_affinity(program, build_keys, num_keys, &affinities);
        program.emit_insn(Insn::HashBuild {
            data: Box::new(HashBuildData {
                cursor_id: build_cursor_id,
                key_start_reg: build_keys,
                num_keys,
                hash_table_id,
                mem_budget: t_ctx.resolver.statement_memory(),
                collations,
                payload_start_reg: (num_payload > 0).then_some(build_keys + num_keys),
                num_payload,
                track_matched: false,
            }),
        });
        program.emit_insn(Insn::Next {
            cursor_id: build_cursor_id,
            pc_if_next: build_row,
            fullscan: true,
            is_index: false,
        });
        program.preassign_label_to_next_insn(built);
        program.emit_insn(Insn::HashBuildFinalize { hash_table_id });
        program.emit_insn(Insn::JoinSwitchBuilt {
            data: Box::new(JoinSwitchBuiltData {
                switch_id,
                hash_table_id,
                target_pc: seek,
                table: btree.name.clone(),
                analyzed_rows: self.analyzed_rows,
                estimate_error: self.estimate_error,
            }),
        });

        // The probe: this outer row's seek key, looked up in the table.
        program.preassign_label_to_next_insn(probe);
        let probe_keys = program.alloc_registers(entry_len);
        for (i, constraint) in self.seek_def.prefix.iter().enumerate() {
            let Some((_, expr, _)) = constraint.eq.as_ref() else {
                crate::bail_parse_error!("an adaptive join's seek must be an equality");
            };
            translate_expr(
                program,
                Some(table_references),
                expr,
                probe_keys + i,
                &t_ctx.resolver,
            )?;
        }
        emit_affinity(program, probe_keys, num_keys, &affinities);
        for i in 0..num_keys {
            // `=` never matches NULL, as the seek's own NULL check says.
            program.emit_insn(Insn::IsNull {
                reg: probe_keys + i,
                target_pc: loop_end,
            });
        }
        let payload_reg = (num_payload > 0).then_some(probe_keys + num_keys);
        let match_reg = probe_keys + num_keys + num_payload;
        program.emit_insn(Insn::HashProbe {
            hash_table_id: to_u32(hash_table_id),
            key_start_reg: to_u32(probe_keys),
            num_keys: to_u32(num_keys),
            dest_reg: to_u32(match_reg),
            target_pc: loop_end,
            deferred_target_pc: loop_end,
            payload_dest_reg: payload_reg.map(to_u32),
            num_payload: to_u32(num_payload),
            probe_rowid_reg: None,
        });

        // A match: the index entry with the key and the matched rowid, where the seek would be.
        program.preassign_label_to_next_insn(found);
        let hash_next = program.allocate_label();
        program.emit_insn(Insn::SeekGE {
            is_index: true,
            cursor_id: self.index_cursor_id,
            start_reg: probe_keys,
            num_regs: entry_len,
            target_pc: hash_next,
            eq_only: false,
            null_matching_mask: Default::default(),
        });
        program.emit_insn(Insn::IdxGT {
            cursor_id: self.index_cursor_id,
            start_reg: probe_keys,
            num_regs: entry_len,
            target_pc: hash_next,
        });
        program.emit_insn(Insn::Goto { target_pc: body });

        // The next match of the same outer row, from the close.
        program.preassign_label_to_next_insn(hash_next);
        program.emit_insn(Insn::HashNext {
            hash_table_id,
            dest_reg: match_reg,
            target_pc: loop_end,
            payload_dest_reg: payload_reg,
            num_payload,
        });
        program.emit_insn(Insn::Goto { target_pc: found });

        t_ctx.adaptive_joins.insert(
            joined_table_index,
            AdaptiveJoinCtx {
                switch_id,
                hash_next,
            },
        );
        program.preassign_label_to_next_insn(seek);
        Ok(body)
    }
}

fn emit_affinity(program: &mut ProgramBuilder, start_reg: usize, count: usize, affinities: &str) {
    if let Some(count) = std::num::NonZeroUsize::new(count) {
        if affinities.chars().any(|c| c != affinity::SQLITE_AFF_BLOB) {
            program.emit_insn(Insn::Affinity {
                start_reg,
                count,
                affinities: affinities.to_string(),
            });
        }
    }
}

/// The close of an adaptive join: once switched, the next row comes from the hash table's next
/// match (`hash_next`, emitted with the prelude), else from the index as planned.
pub(super) fn emit_close_switch(program: &mut ProgramBuilder, ctx: &AdaptiveJoinCtx) {
    program.emit_insn(Insn::JoinSwitchHash {
        switch_id: ctx.switch_id,
        target_pc: ctx.hash_next,
    });
}
