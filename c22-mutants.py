"""Lane C22: each part of the fix taken out, one tree at a time."""
import subprocess, sys
root = sys.argv[1] + "/crates/engine/"
MUTANTS = {
 "statement": [("vdbe/insn.rs", "            ResolveType::Fail => Some(Self::Fail),\n            ResolveType::Rollback => Some(Self::Rollback),\n            ResolveType::Abort | ResolveType::Ignore | ResolveType::Replace => None,", "            _ => None,")],
 "subprogram": [("vdbe/execute.rs", "                            state.constraint_action = statement.constraint_action();\n", "")],
 "fk": [("translate/fkeys.rs", "or_conflict: Some(ACTION_CONFLICT)", "or_conflict: None")],
 "mustbeint": [("vdbe/builder.rs", "                | Insn::MustBeInt {\n                    target_pc: None,\n                    ..\n                }\n", "")],
 "ignore": [("translate/insert.rs", "        } else if matches!(preflight.effective_on_conflict, ResolveType::Ignore) {\n            // No upsert covers this constraint and IGNORE settles it: skip this row.\n            program.emit_insn(Insn::Goto {\n                target_pc: ctx.loop_labels.row_done,\n            });\n", "")],
 "place": [("schema.rs", "        let Some(key_at) = self.unique_sets.iter().position(|set| set.is_primary_key) else {\n            return false;\n        };\n        let Some((key, newer))", "        if self.unique_sets.len() < 1000 { return false; }\n        let Some(key_at) = self.unique_sets.iter().position(|set| set.is_primary_key) else {\n            return false;\n        };\n        let Some((key, newer))")],
 "notnull": [("translate/emitter/update.rs", "    let conflict = if program.flags.has_statement_conflict() {\n        program.resolve_type\n    } else {\n        table_column\n            .notnull_conflict_clause\n            .unwrap_or(ResolveType::Abort)\n    };\n    let description", "    let conflict = program.resolve_type;\n    let description")],
}
name = sys.argv[2]
for path, old, new in MUTANTS[name]:
    p = root + path
    s = open(p).read()
    assert old in s, (name, path)
    open(p, "w").write(s.replace(old, new))
