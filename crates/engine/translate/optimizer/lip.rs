//! Amphora (QT.5, T-QT.5.3): lookahead information passing across a star join (Zhu, Potti,
//! Saurabh and Patel, "Looking Ahead Makes Query Plans Robust", VLDB 2017). A left-deep plan
//! builds each hash join from the table just before its probe, so a fact table read first is only
//! cut down by its dimensions one loop at a time, after it has paid for every loop before. Here
//! each dimension joined after the fact table by an inner equality, with terms of its own, is read
//! once into a Bloom filter of its join keys, and the fact table's scan checks every filter, the
//! one expected to pass the fewest rows first, before any later loop runs.
//!
//! A filter only drops fact rows no inner join could keep: the dimension's terms used are a subset
//! of its own (so its filter holds a superset of the keys that can match), an outer, semi or anti
//! join's table neither gives nor takes one, and a plan with a RIGHT or FULL JOIN takes none, since
//! its unmatched rows are read again. Each filter is chosen by the planner's own costs: it is kept
//! when the work its misses no longer do is more than its lookups, its bits and the dimension's
//! extra read (`bloom_filter_rate_saving`). The filters share the statement's memory, the hash
//! tables' source (`CostModelParams::statement_memory`); a filter past its share is dropped and
//! passes every row.

use super::access_method::bloom_filter_rate_saving;
use super::constraints::BinaryExprSide;
use super::cost_params::CostModelParams;
use crate::schema::Table;
use crate::translate::expr::as_binary_components;
use crate::translate::expr::{walk_expr, WalkControl};
use crate::translate::plan::{
    HashJoinKey, JoinOrderMember, JoinedTable, LipFilter, Operation, Scan, TableReferences,
    WhereTerm, WhereTermOrigin,
};
use crate::StarFilters;
use amphora_engine_parser::ast::{self, Expr};

/// The filters a plan's star joins check at their fact tables' scans, in the order each fact scan
/// checks them.
pub(crate) fn choose(
    rule: StarFilters,
    tables: &TableReferences,
    where_clause: &[WhereTerm],
    join_order: &[JoinOrderMember],
    base_rows: &[f64],
    params: &CostModelParams,
) -> Vec<LipFilter> {
    if rule == StarFilters::Off {
        return Vec::new();
    }
    let joined = tables.joined_tables();
    if joined
        .iter()
        .any(|t| t.join_info.as_ref().is_some_and(|ji| ji.keeps_right_rows()))
    {
        return Vec::new();
    }
    let end_cost = joined
        .iter()
        .filter_map(|t| t.plan_estimate.map(|e| e.total_cost))
        .fold(0.0, f64::max);
    let mut chosen: Vec<LipFilter> = Vec::new();
    for (fact_pos, fact_member) in join_order.iter().enumerate() {
        let fact_idx = fact_member.original_idx;
        let Some(fact) = joined.get(fact_idx) else {
            continue;
        };
        if !plain_inner(fact)
            || !matches!(fact.op, Operation::Scan(Scan::BTreeTable { .. }))
            || !rowid_btree(fact)
        {
            continue;
        }
        let Some(fact_est) = fact.plan_estimate else {
            continue;
        };
        let mut candidates = Vec::new();
        for dim_member in join_order.iter().skip(fact_pos).skip(1) {
            let dim_idx = dim_member.original_idx;
            let Some(dim) = joined.get(dim_idx) else {
                continue;
            };
            if !plain_inner(dim) || !rowid_btree(dim) {
                continue;
            }
            let keys = keys_between(where_clause, fact, dim);
            let dim_terms = local_terms(where_clause, dim);
            if keys.is_empty() || dim_terms.is_empty() {
                continue;
            }
            // The share of fact rows with a match: the rows the dimension gives each row before
            // it, at most one (containment, as the join's estimate assumes).
            let pass = dim
                .plan_estimate
                .map_or(1.0, |e| e.rows_per_input.clamp(0.0, 1.0));
            candidates.push((pass, dim_idx, keys, dim_terms));
        }
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
        let probe_rows = fact_est.output_rows;
        // The work each fact row leads to in the loops after it, which a rejected row skips.
        let saved_per_miss = if probe_rows > 0.0 {
            (end_cost - fact_est.total_cost).max(0.0) / probe_rows
        } else {
            0.0
        };
        let mut reaching = probe_rows;
        for (pass, dim_idx, keys, dim_terms) in candidates {
            let dim_rows = base_rows
                .get(dim_idx)
                .copied()
                .unwrap_or(params.rows_per_table_fallback);
            let build_keys = dim_rows * pass;
            // The dimension read once more: its rows, its terms on each, and its pages at the
            // connection's measured share that miss the cache (`cost::file_reads`).
            let build_cost = dim_rows
                * (params.cpu_cost_per_row
                    + params.cpu_cost_per_where_step * dim_terms.len() as f64)
                + dim_rows / params.rows_per_table_page.max(1.0) * params.page_miss;
            let sized = bloom_filter_rate_saving(
                build_keys,
                reaching,
                reaching * pass,
                saved_per_miss,
                build_cost,
                params,
            );
            let fp_per_key = match (rule, sized) {
                (_, Some((rate, true))) => rate,
                (StarFilters::Always, Some((rate, false))) => rate,
                (StarFilters::Always, None) => 0.0,
                _ => continue,
            };
            chosen.push(LipFilter {
                fact_idx,
                dim_idx,
                keys,
                dim_terms,
                fp_per_key,
                max_bytes: 0,
                pass,
            });
            reaching *= pass;
        }
    }
    // No filter chosen leaves the whole budget, as dividing by max(1) did.
    let share = params
        .statement_memory
        .checked_div(chosen.len())
        .unwrap_or(params.statement_memory);
    for filter in &mut chosen {
        filter.max_bytes = share;
    }
    chosen
}

/// A table joined plainly: not an outer, semi or anti join's.
fn plain_inner(table: &JoinedTable) -> bool {
    table
        .join_info
        .as_ref()
        .is_none_or(|ji| !ji.is_outer() && !ji.is_semi_or_anti() && !ji.keeps_right_rows())
}

fn rowid_btree(table: &JoinedTable) -> bool {
    matches!(&table.table, Table::BTree(btree) if btree.has_rowid)
}

/// The `WHERE` equalities between a column of `fact` and one of `dim`, both of binary collation;
/// each key's build side is the dimension's.
fn keys_between(
    where_clause: &[WhereTerm],
    fact: &JoinedTable,
    dim: &JoinedTable,
) -> Vec<HashJoinKey> {
    let mut keys = Vec::new();
    for (idx, term) in where_clause.iter().enumerate() {
        if !matches!(
            term.origin,
            WhereTermOrigin::Where | WhereTermOrigin::Implied
        ) {
            continue;
        }
        let Ok(Some((lhs, op, rhs))) = as_binary_components(&term.expr) else {
            continue;
        };
        if !matches!(
            op,
            super::constraints::ConstraintOperator::AstNativeOperator(ast::Operator::Equals)
        ) {
            continue;
        }
        let side = match (column_of(lhs), column_of(rhs)) {
            (Some((l, lc)), Some((r, rc))) if l == fact.internal_id && r == dim.internal_id => {
                if binary_collation(fact, lc) && binary_collation(dim, rc) {
                    Some(BinaryExprSide::Rhs)
                } else {
                    None
                }
            }
            (Some((l, lc)), Some((r, rc))) if l == dim.internal_id && r == fact.internal_id => {
                if binary_collation(dim, lc) && binary_collation(fact, rc) {
                    Some(BinaryExprSide::Lhs)
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(build_side) = side {
            keys.push(HashJoinKey {
                where_clause_idx: idx,
                build_side,
            });
        }
    }
    keys
}

fn column_of(expr: &Expr) -> Option<(ast::TableInternalId, usize)> {
    match expr {
        Expr::Column { table, column, .. } => Some((*table, *column)),
        _ => None,
    }
}

fn binary_collation(table: &JoinedTable, column: usize) -> bool {
    table.columns().get(column).is_some_and(|c| {
        matches!(
            c.collation_opt(),
            None | Some(crate::translate::collate::CollationSeq::Binary)
        )
    })
}

/// The `WHERE` terms that read only `dim`, built of columns, constants and operators alone, so
/// the filter's build gives each row the answer the join's loop does.
fn local_terms(where_clause: &[WhereTerm], dim: &JoinedTable) -> Vec<usize> {
    where_clause
        .iter()
        .enumerate()
        .filter(|(_, term)| {
            matches!(
                term.origin,
                WhereTermOrigin::Where | WhereTermOrigin::Implied
            ) && reads_only(&term.expr, dim)
        })
        .map(|(idx, _)| idx)
        .collect()
}

fn reads_only(expr: &Expr, dim: &JoinedTable) -> bool {
    let mut only = true;
    let mut reads = false;
    let walked = walk_expr(expr, &mut |e: &Expr| -> crate::Result<WalkControl> {
        match e {
            Expr::Column { table, .. } | Expr::RowId { table, .. } => {
                reads = true;
                only &= *table == dim.internal_id;
            }
            Expr::Literal(_)
            | Expr::Binary(..)
            | Expr::Unary(..)
            | Expr::Between { .. }
            | Expr::InList { .. }
            | Expr::Like { .. }
            | Expr::IsNull(_)
            | Expr::NotNull(_)
            | Expr::Parenthesized(_)
            | Expr::Cast { .. } => {}
            _ => only = false,
        }
        Ok(WalkControl::Continue)
    });
    walked.is_ok() && only && reads
}
