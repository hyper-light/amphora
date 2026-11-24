//! Amphora (QT.5, T-QT.5.3): a star join's Bloom filters (`optimizer/lip.rs`): each dimension read
//! once into its filter before the fact table's first row, and every filter checked at each fact
//! row after the fact table's own terms.

use super::*;
use crate::translate::expr::comparison_affinity;
use crate::translate::plan::HashJoinKey;

/// A built filter, as the fact table's scan checks it.
pub(super) struct StarProbe {
    cursor_id: CursorID,
    keys: Vec<HashJoinKey>,
    affinities: String,
}

/// Read each dimension whose filter `fact_idx`'s scan checks, once a statement.
pub(super) fn emit_builds(
    program: &mut ProgramBuilder,
    t_ctx: &TranslateCtx,
    table_references: &TableReferences,
    predicates: &[WhereTerm],
    fact_idx: usize,
) -> Result<Vec<StarProbe>> {
    let mut probes = Vec::new();
    for filter in table_references
        .lip_filters()
        .iter()
        .filter(|f| f.fact_idx == fact_idx)
    {
        let Some(dim) = table_references.joined_tables().get(filter.dim_idx) else {
            continue;
        };
        let Some(btree) = dim.btree() else {
            continue;
        };
        let cursor_id = program.alloc_cursor_id(CursorType::BTreeTable(btree.clone()));
        let affinities: String = filter
            .keys
            .iter()
            .map(|key| {
                comparison_affinity(
                    key.get_build_expr(predicates),
                    key.get_probe_expr(predicates),
                    Some(table_references),
                    None,
                )
                .aff_mask()
            })
            .collect();
        let num_keys = filter.keys.len();
        let built = program.allocate_label();
        let loop_start = program.allocate_label();
        let skip = program.allocate_label();
        let loop_end = program.allocate_label();
        program.emit_insn(Insn::Once {
            target_pc_when_reentered: built,
        });
        program.emit_insn(Insn::OpenRead {
            cursor_id,
            root_page: btree.root_page,
            db: dim.database_id,
        });
        program.emit_insn(Insn::Rewind {
            cursor_id,
            pc_if_empty: loop_end,
        });
        program.set_table_cursor_override(dim.internal_id, cursor_id);
        program.preassign_label_to_next_insn(loop_start);
        for &term in &filter.dim_terms {
            if let Some(cond) = predicates.get(term) {
                super::conditions::emit_where_term(
                    program,
                    table_references,
                    cond,
                    skip,
                    &t_ctx.resolver,
                )?;
            }
        }
        let key_reg = program.alloc_registers(num_keys);
        for (idx, key) in filter.keys.iter().enumerate() {
            translate_expr(
                program,
                Some(table_references),
                key.get_build_expr(predicates),
                key_reg.checked_add(idx).ok_or_else(|| {
                    crate::LimboError::InternalError("star join: a key register past usize".into())
                })?,
                &t_ctx.resolver,
            )?;
        }
        if let Some(count) = std::num::NonZeroUsize::new(num_keys) {
            program.emit_insn(Insn::Affinity {
                start_reg: key_reg,
                count,
                affinities: affinities.clone(),
            });
        }
        program.emit_insn(Insn::FilterAdd {
            cursor_id,
            key_reg,
            num_keys,
            sizing: Some(crate::vdbe::bloom_filter::FilterSizing {
                fp_per_key: filter.fp_per_key,
                max_bytes: filter.max_bytes,
            }),
        });
        program.preassign_label_to_next_insn(skip);
        program.emit_insn(Insn::Next {
            cursor_id,
            pc_if_next: loop_start,
            fullscan: false,
            is_index: false,
        });
        program.preassign_label_to_next_insn(loop_end);
        program.clear_table_cursor_override(dim.internal_id);
        program.preassign_label_to_next_insn(built);
        probes.push(StarProbe {
            cursor_id,
            keys: filter.keys.clone(),
            affinities,
        });
    }
    Ok(probes)
}

/// Check the fact row's keys against each filter, in the planner's order or the run's measured one
/// (`Connection::set_star_filter_order`, T-QT.5.11): one `StarProbes`; a miss goes to `skip`. Every
/// filter's keys are read first, the fact row's columns, so the checks can run in any order.
pub(super) fn emit_probes(
    program: &mut ProgramBuilder,
    t_ctx: &TranslateCtx,
    table_references: &TableReferences,
    predicates: &[WhereTerm],
    probes: &[StarProbe],
    skip: BranchOffset,
) -> Result<()> {
    if probes.is_empty() {
        return Ok(());
    }
    let mut keys = Vec::with_capacity(probes.len());
    for probe in probes {
        let num_keys = probe.keys.len();
        let key_reg = program.alloc_registers(num_keys);
        for (idx, key) in probe.keys.iter().enumerate() {
            translate_expr(
                program,
                Some(table_references),
                key.get_probe_expr(predicates),
                key_reg.checked_add(idx).ok_or_else(|| {
                    crate::LimboError::InternalError("star join: a key register past usize".into())
                })?,
                &t_ctx.resolver,
            )?;
        }
        if let Some(count) = std::num::NonZeroUsize::new(num_keys) {
            program.emit_insn(Insn::Affinity {
                start_reg: key_reg,
                count,
                affinities: probe.affinities.clone(),
            });
        }
        keys.push(crate::vdbe::insn::StarProbeKeys {
            cursor_id: probe.cursor_id,
            key_reg,
            num_keys,
        });
    }
    program.emit_insn(Insn::StarProbes {
        data: Box::new(crate::vdbe::insn::StarProbesData {
            probes: keys.into_boxed_slice(),
            target_pc: skip,
            order: t_ctx.resolver.star_filter_order(),
        }),
    });
    Ok(())
}
