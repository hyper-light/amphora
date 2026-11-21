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
        if table
            .join_info
            .as_ref()
            .is_some_and(|info| info.is_outer() || info.is_semi_or_anti() || info.keeps_right_rows())
        {
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
        let stats = t_ctx.resolver.schema().analyze_stats.table_stats(&btree.name)?;
        let analyzed_rows = stats.row_count.or_else(|| {
            stats
                .index_stats
                .values()
                .find_map(|stat| stat.total_rows)
        })? as f64;
        let estimate_error = crate::row_changes::statistics_error(stats, analyzed_rows);
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
