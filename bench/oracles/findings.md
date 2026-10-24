# The oracles' findings

Each failure the oracles find (`amphora-bench --oracle`, `bench/src/oracle`; the oracles themselves in
`docs/research/oracles.md`) is cut down to its smallest failing form (`reduce.rs`) and written to its
run's `findings/<id>.md`: the statements, the queries, and what differed. Each is filed here under its
cause, with the fix and the test that keeps it found: `crates/engine/tests/oracle_findings.rs` (one
test a cause, each answering as SQLite 3.54.0 answers) and `crates/engine/tests/real_text.rs`. A
finding that is no fault of Amphora's is filed with why. The runs, their summaries, every finding and
`causes.tsv` (each finding's cause) are in `bench/records/2026-10-24-adacachy-oracles/`.

`--replay-check <finding>` reruns a finding on a build and exits 0 when it no longer fails: each of its
queries answers as SQLite's does (for a CERT finding, the more restrictive query's estimate is no more
than the other's). Every finding of the five runs below was rerun so on the build that closed G2
step 3: all answer as SQLite does but for the CERT findings of AG (87), which the planner's goal
holds (kiln goal 01a16e788f3e735fbadbbdaa30d38d29, "the oracles' CERT clean"), and the ones that
are not Amphora's (Q, AH, AN). BE and BD answer as SQLite does since the fix that followed: an AND
a literal 0 settles is folded before names are resolved, and a subquery a settled branch drops
never runs. What BE left, a subquery run before the loops where SQLite runs it later or never,
answers as SQLite since a subquery runs where it is read (`docs/design/executor.md`, decision 6;
`an_unread_subquery_never_runs`), but for a correlated one outside a direct-output SELECT (kiln goal
01a1704d3be97309b55a5863391f3182).

## Campaign 1: seed 11, 607 s, 2026-10-24

adacachy, against the system's SQLite 3.54.0, on commit 7e24a7f with the oracles (then uncommitted):
978 databases, 35,104 checks, 70 findings (135 more of the same). On the fixed build all 70 rerun as
SQLite answers, but for one not Amphora's (Q).

| cause | findings | fix | test |
|---|---:|---|---|
| A. A REAL written as text with the shortest digits (`1.0e+15`) where SQLite 3.54 writes `%!.17g` shortened while it reads back (`1000000000000000.0`): CAST, TEXT affinity, `\|\|`, CAST AS BLOB, length() | 16 | `numeric/fp.rs`: SQLite's `sqlite3FpDecode` and its rendering, ported | `real_text.rs` (4,756 doubles SQLite answered) |
| B. A BLOB in LIKE or GLOB: the system's SQLite matches it, Amphora by design does not (`docs/design/planner.md` decision 3: `SQLITE_LIKE_DOESNT_MATCH_BLOBS`, which SQLite recommends) | 28 | not Amphora's: the oracles' reference is SQLite built with the option (`sqlite.sh`, `--sqlite`) | `a_blob_matches_no_like_or_glob_even_beside_a_null` |
| B2. ... and a BLOB beside a NULL: SQLite with the option answers 0 before it looks at the NULL; Amphora answered NULL and looked only at the text's side | 1 | `vdbe/execute.rs` GLOB, LIKE | the same |
| C. Panic: an automatic index's filter, copied from an ON term (which keeps its BETWEEN), compared only after a WHERE-style rewrite | 1 | `optimizer/constraints.rs` compares rewritten forms; the `expect` is a typed error | `a_left_join_on_a_constant_between_returns_sqlites_rows` |
| D. `CASE x WHEN y` compared without affinity or collation | 3 | `expr/translator.rs` | `case_compares_as_equality_does` |
| E. An IN list compared by the collation of the element before | 1 | `expr/condition.rs` | `in_compares_by_its_left_operand` |
| F. format('') and printf('') NULL (SQLite 3.53) where 3.54 gives ''; %j and %J (3.54) missing | 3 | `functions/printf.rs` | `format_of_an_empty_format_is_empty_text`, `printf_writes_json_strings_with_j` |
| G. unhex's second argument read as text only when it is text | 2 | `vdbe/value.rs` | `unhex_reads_its_second_argument_as_text` |
| H. `\|\|` of two BLOBs gave a BLOB | 1 | `vdbe/value.rs` | `concatenation_is_text` |
| I. HAVING subqueries read NULL for columns no other clause names (the scan's spent cursor) | 2 | `GroupBy.carried` (`plan.rs` `outer_columns_read`), `group_by.rs`; nested subqueries keep the group's registers (`Resolver::fork_with_outer_expr_cache`) | `having_subqueries_read_the_groups_row` |
| J. JSON's REALs written with the shortest digits | 1 | `json/mod.rs` writes them as quote() does | `json_writes_a_real_as_quote_does` |
| K. JSON edits of a NULL document read the path first | 2 | `json/ops.rs`, `json/mod.rs` | `json_of_a_null_document_is_null` |
| L. A COLLATE alone as a WHERE term refused | 1 | `expr/condition.rs` | `a_collate_alone_is_its_operands_truth` |
| M. `0 AND f()` evaluated f(): SQLite folds a literal false AND and a literal true OR as it parses | 1 | `expr/utils.rs` `folded_and_or` | `a_literal_false_and_or_true_or_skips_the_other_side` |
| M2. A constant function call moved to the program's start ran where the query never reaches it; json_object's argument count checked at prepare | (refusals) | `expr/translator.rs` `translate_constant_call_once` (OP_Once in place, as SQLite) | `json_object_counts_its_arguments_when_it_runs` |
| N. json_array_insert and jsonb_array_insert (3.54) missing | 2 | `json/ops.rs`, `json/jsonb.rs` `ArrayInsertOperation` | `json_array_insert_inserts_as_sqlite_does` |
| P1. CERT: a multi-index union marked the query's terms consumed by its branches' own numbering | 1 | `optimizer/multi_index.rs` | `a_multi_index_or_keeps_the_other_terms_selectivity` |
| P2. CERT: a term a later table completed never counted | 2 | `optimizer/join.rs` `completed_terms_selectivity`, home tables | `a_term_counts_where_the_join_completes_it` |
| P3. CERT: two bounds read apart counted as two ranges, read together as one | 1 | the same: a term counts on its home table's constraints | `bounds_read_apart_make_the_same_range` |
| Q. Not Amphora's: json_valid(…, NULL) raised where Amphora computes an automatic index's key before finding the inner table empty; SQLite raises it too with an index on the column | 1 | the oracles count a value's error a derived query meets as evaluation order | — |

## Campaign 2: seed 12, 904 s, 2026-10-24

adacachy, against the oracles' SQLite (`sqlite.sh`), on the campaign 1 fixes: 3,824 databases,
154,695 checks, 84 findings (10 more of the same; 38 wrong answers, 37 estimates, 9 errors), none from
the setup's rows (campaign 1's 111 were cause A). Nine of them (Z) were a regression of this round's
own M2, found the run it shipped in. On the fixed build 80 rerun as SQLite answers; of the other four,
two are AG's and two not Amphora's (AH, Q).

| cause | findings | fix | test |
|---|---:|---|---|
| R. substr with no length took SQLITE_LIMIT_LENGTH where 3.54 takes the largest integer | 2 | `vdbe/value.rs` | `substr_takes_the_largest_length_when_none_is_given` |
| S. zeroblob('3.0') read as 0, not the integer the text begins with | 3 | `vdbe/value.rs` | `zeroblob_reads_the_integer_text_begins_with` |
| T. A whole JSON document that is the JSON null given as SQL NULL, not 'null' | 4 | `json/mod.rs` | `a_json_null_document_is_the_text_null` |
| U. NOT as a condition evaluated its operand as a value, losing its short circuits; an OR's NULL left side did not settle it where NULL goes where TRUE does | 4 | `expr/condition.rs`; a condition's jump honours a NULL target of its own (`emit_cond_jump`), the IN search only where NULL and FALSE meet | `not_keeps_its_operands_short_circuits` |
| V. `'1.5' NOTNULL` not folded as SQLite's parser folds a literal's NULL test | 2 | `expr/utils.rs` `literal_truth` | `a_literals_null_test_folds_like_a_literal` |
| W. A searched CASE's WHEN evaluated as a value, not a condition | 1 | `expr/translator.rs` | `a_case_when_is_a_condition` |
| X. replace() answered NULL for a NULL replacement before looking at an empty pattern | 4 | `vdbe/value.rs` | `replace_with_an_empty_pattern_keeps_the_text` |
| Y. JSONB of text kept control characters raw where SQLite escapes them | 1 | `json/mod.rs` `json_string_literal` | `jsonb_escapes_control_characters_in_text` |
| Z. M2's regression: the copy of a constant call read twice (a BETWEEN's left side) moved to the program's start, before anything was kept to copy | 9 | `expr/translator.rs`: the copy stays where it is read | `a_constant_call_read_twice_is_copied_where_it_is_read` |
| AA. Text and BLOBs read past their first NUL by printf, format and unhex; strftime's REAL format written with Rust's digits | 6 | `functions/printf.rs`, `vdbe/value.rs`, `types.rs` (`ValueRef` writes a REAL as SQLite does) | `functions_read_text_to_its_first_nul` |
| AB. json_remove skipped a NULL path where SQLite gives NULL | 1 | `json/ops.rs` | `json_remove_of_a_null_path_is_null` |
| AC. A column's collation carried through a function, an operator or a CASE (`format('abc', c)`, `c \|\| ''`), so DISTINCT, UNION and comparisons used it | 3 | `collate.rs`: a column's collation only where the expression is the column; `expr/translator.rs`, `expr/binary.rs` | `a_columns_collation_stops_at_a_function`, `distinct_and_union_carry_no_collation_through_a_function` |
| AD. sum() kept an integer overflow a later non-integer clears | 1 | `vdbe/execute.rs` | `sum_forgets_an_overflow_a_non_integer_follows` |
| AE. `x IN (SELECT y)` compared by y's collation where x's column collation comes first | 1 | `subquery.rs`, `collate.rs` `resolve_comparison_collseq_across` | `in_subquery_collates_as_equality_does` |
| AF. CERT: estimates that hung on join order: constraints over this table and earlier ones never ready, and a hash join's output read off its build input | 35 | `optimizer/join.rs` (ready when every table read is joined), `eqp.rs` (`output_rows`: the join's own estimate) | `a_hash_joins_estimate_is_the_joins` and the P tests |
| AG. CERT, open, the planner's goal's: adding `AND a0.c1` to a FULL JOIN's WHERE raised its estimate 32 times; adding a TRUE `OR` raised one 32 times | 2 | — (kiln goal 01a16e788f3e735fbadbbdaa30d38d29) | — |
| AH. Not Amphora's: SQLite answers 0 rows for `t0 a0 RIGHT JOIN t0 a1 ON (a1.c1 LIKE 'A%')` with a NOCASE index on the BLOB column c1, where every row of a1 must appear (`+a1.c1` gives 1); Amphora gives 1 | 1 | — (SQLite's) | — |
| Q. Not Amphora's: evaluation order (`like()`'s ESCAPE error met where Amphora reads the inner table first) | 1 | the oracles count it as a value's error | — |
| AI. JSONB edits wrote other bytes for the same value: SQLite's jsonBlobEdit puts a value one to eight bytes shorter in the old one's place with its header widened (jsonBlobOverwrite); json_set's text values are TEXTRAW there, as is a key a path makes; jsonb_patch copies the patch's own label | 1 | `json/jsonb.rs` `replace_element`, `widened_element`, `text_raw`, `append_to_object`; `json/mod.rs`, `json/ops.rs` | `a_jsonb_edit_keeps_a_shorter_values_place_as_sqlite_does` |
| AJ. A RIGHT or FULL JOIN's unmatched rows run the loops from its right table on again with the tables before it NULL: a correlated subquery of a later term ran in a loop that pass skips (keeping its last value), and an EXISTS filter on the earlier tables was moved into it where fewer rows were estimated, rejecting them | 2 | `optimizer/mod.rs` (a subquery runs where its term does), `optimizer/join.rs` (no filter moved into or past such a table) | `a_right_joins_unmatched_rows_rerun_their_subqueries`, `a_filter_is_never_moved_into_a_right_joins_unmatched_pass` |

## The injected bug's run: seed 20261024, 1,809 s, 2026-10-24

G2 step 3's test (`bench/oracles/injected.sh`): a bug put into Amphora's optimizer
(`bench/oracles/injected/is-null-rejects-nulls.patch`: `x IS NULL` counted as rejecting NULLs, so an
outer join under it was made an inner one), the bench built with it, and one nightly run of the
oracles on that build, adacachy, against the oracles' SQLite: 1,759 databases, 71,127 checks, 245
findings (4 more of the same). **Caught:** 125 findings answer as SQLite's on the clean build and not
on the injected one, the first at 0.47 s of the 1,800 s budget (diff-a78b77f9e0e7f06a); the run's
`verdict.txt` and `causes.tsv` are in `injected/`. The other 120 were Amphora's own, or the oracles':

| cause | findings | fix | test |
|---|---:|---|---|
| The injected bug | 125 | (taken back out of the tree) | the test itself |
| AK. A NULL under a NOT went where FALSE goes, not where the condition sent it (NOT swaps TRUE and FALSE, not NULL): NOT LIKE and NOT GLOB and arithmetic conditions fixed their own NULL jump, and an AND's or OR's NULL left side went to the NULL's place where it doesn't settle the result (NOT (NULL AND 0) is TRUE) | 90 | `expr/condition.rs` (AND, OR, NOT LIKE), `expr/binary.rs`, `expr/metadata.rs` (`ConditionMetadata::negated`, `emit_cond_jump`) | `a_null_under_not_goes_where_its_condition_sends_it` |
| AP. The oracles': the reducer cut a TLP aggregate case's `sum(e)` to `e`, and a DISTINCT case's DISTINCT away, so its queries, each answering as SQLite's, no longer stood in the relation checked | 22 | `bench/src/oracle/checks.rs`: such a case is skipped, so the reducer keeps it whole | — |
| AC (campaign 2's): a NOCASE column's collation carried through char() into the partitions' UNION, and through a CASE into a BETWEEN | 2 | campaign 2's | campaign 2's |
| AL. char() narrowed its integer to 32 bits before checking its range (9007199254740993 became 1) | 1 | `vdbe/value.rs` | `char_reads_its_integers_whole` |
| AM. log(B, X) required X to be a number; SQLite's logFunc reads X as sqlite3_value_double does, whatever its type ('1e30.0' is 1e30) | 1 | `vdbe/value.rs` | `log_with_a_base_reads_its_operand_as_a_number` |
| AN. Not Amphora's: `CAST(x'ff' AS TEXT)` is ill-formed UTF-8, for which SQLite promises no answer (its invalid UTF policy); Amphora's TEXT is well-formed UTF-8 (`docs/design/semantics.md`, decision 1). The oracles skip an answer of SQLite's holding such text, and their BLOBs are well-formed UTF-8 | 1 | — (`engines.rs`, `synth.rs`) | `char_reads_its_integers_whole` (a surrogate) |
| AG (campaign 2's), the planner's goal's: an inner join estimated 32 times its RIGHT JOIN; a WHERE with a constant-FALSE term and a scalar subquery estimated 1,000,000 rows over ANALYZE's 1; a NOT EXISTS raised a join's estimate 100 times | 3 | — | — |

## Campaign 3: seed 13, 920 s, 2026-10-24

adacachy, against the oracles' SQLite, on the fixes above: 24,321 databases, 983,406 checks, 61
findings (5 more of the same).

| cause | findings | fix | test |
|---|---:|---|---|
| AQ. A comparison kept in its operand's register the value its affinity made of it (SQLite's OP_Eq undoes that): a CASE base compared under one WHEN's affinity read converted under the next, and a constant converted in one row stayed so in the next | 5 | `vdbe/execute.rs` `op_comparison_slow` | `a_comparison_leaves_its_operands_as_they_were` |
| AR. Text holding a NUL after a number (`'5' \|\| char(0)`, `0 \|\| x'00'`): SQLite 3.54 reads the number before the NUL, an integer only as the whole text allows; Amphora took it for no number, or for an integer | 4 | `numeric/mod.rs` (`Numeric::from`, `from_value_strict`, `real_same_as_int`), `vdbe/affinity.rs`, `vdbe/execute.rs` (affinity) | `text_with_a_nul_is_a_number_up_to_the_nul` |
| AS. trim, ltrim and rtrim read their characters past the first NUL | 5 | `vdbe/value.rs` | `trim_reads_its_characters_to_the_first_nul` |
| AT. round read its precision as a number ('1.0e+308'), where SQLite takes the integer the text begins with | 1 | `vdbe/value.rs` | `round_reads_its_precision_as_an_integer` |
| AU. json_error_position: input after a whole top-level value is position 1 in SQLite (no error offset), and an exponent without digits is reported at its `e` or sign | 1 | `json/jsonb.rs` | `json_error_position_counts_as_sqlite_does` |
| AV. `x IN (SELECT realcol)` compared by NUMERIC where SQLite's IN takes the column's own REAL affinity, which makes an integer probe a REAL | 3 | `expr/affinity.rs` `in_subquery_affinity`, `subquery.rs` | `in_a_real_column_finds_an_integer_as_a_real` |
| AW. A correlated subquery run after the read that needs it: moved into a table's loop after the index seek whose key reads it (a panic, "cursor id 0 is None"), an EXISTS moved there where fewer rows were estimated (a stale value), and (campaign 4) a hash join's unmatched build rows checking a condition whose subquery ran only in the probe's path (a panic) | 2 (+1) | `optimizer/mod.rs` (a RIGHT or FULL JOIN's subqueries run at its right table), `optimizer/join.rs` (no deferral to a seek by the term), `main_loop/conditions.rs` (before such a hash join) | `a_seeks_subquery_runs_before_the_seek`, `a_hash_joins_unmatched_rows_see_their_subqueries` |
| AX. SQLite 3.54's evaluation order: a settled AND or OR drops its other side once its own are simplified (sqlite3ExprSimplifiedAndOr), the side without a subquery goes first (exprEvalRhsFirst), and a comparison's or arithmetic's subquery side is never computed beside a NULL (exprComputeOperands); Amphora read what SQLite never reads and raised its error | 6 | `expr/utils.rs` `simplified_and_or`, `has_subquery`; `expr/condition.rs`, `expr/translator.rs`, `expr/binary.rs` | `a_settled_and_or_and_a_null_beside_a_subquery_read_no_further` |
| AC (campaign 2's) through a CASE: its branches' column collation compared a FULL JOIN's ON by NOCASE | 1 | `expr/translator.rs` (a CASE takes only an explicit COLLATE) | `a_case_carries_no_columns_collation` |
| BA. Found beside AP's tlp-dabb1a3f92b77bab: count(DISTINCT x) and group_concat(DISTINCT x) keyed their set by the whole aggregate's collation, none since AC, not the argument's | — | `main_loop/init.rs` | `a_distinct_aggregate_dedupes_by_its_arguments_collation` |
| AP. The oracles': HAVING subqueries reading the group's bare columns (5; the bare-column check now looks into subqueries), the engine's Yield taken for an error (1), and a NOCASE group keeping other rows in the partitions than in the query, text holding a NUL (1; the loose comparison folds it as sqlite3StrNICmp compares it) | 7 | `bench/src/oracle/synth.rs`, `engines.rs` | — |
| AH. Not Amphora's: a FULL JOIN with a NOCASE index on a BLOB column and a LIKE ON term, where SQLite answers no rows though both tables' rows must appear | 1 | — (SQLite's) | — |
| Q. Not Amphora's: evaluation order (zeroblob's error in a subquery over an empty table, computed once for its outer row) | 1 | — | — |
| AG. CERT, the planner goal's | 24 | — | — |

## Campaign 4: seed 14, 917 s, 2026-10-24

adacachy, against the oracles' SQLite, on campaign 3's fixes: 27,091 databases, 1,096,012 checks, 70
findings (2 more of the same), 12 of them not CERT (campaign 3: 37).

| cause | findings | fix | test |
|---|---:|---|---|
| AW (campaign 3's): a hash join's unmatched build rows | 1 | see AW | `a_hash_joins_unmatched_rows_see_their_subqueries` |
| BB. nullif, and min and max of two or more, compared by BINARY where SQLite compares by the first argument's collation that has one (SQLITE_FUNC_NEEDCOLL) | 1 | `function.rs` (`FuncCtx::collation`), `expr/translator.rs`, `vdbe/execute.rs` | `nullif_min_and_max_compare_by_their_arguments_collation` |
| BC. A function's value kept what its last argument left of the collation, where SQLite takes the first explicit COLLATE among its arguments | 1 | `expr/translator.rs` `set_explicit_collation` | `a_functions_value_takes_its_arguments_first_collate` |
| V (campaign 2's): a NULL test of a parenthesized constant, or of another such test, not folded (`(- 1.5) NOTNULL`, `(2.0 NOTNULL) NOTNULL`), so the AND or OR it settles read a call that raised an error | 3 | `expr/utils.rs` `literal_truth` | `a_parenthesized_null_test_folds_like_a_literal` |
| AP. The oracles': -0.0 and 0 as one group's value, either shown (1); a REAL sum's partitions, summed again, rounding apart from it (1) | 2 | `engines.rs`, `checks.rs` | — |
| BD. SQLite resolves no name in `0 AND x`, which its parser makes the 0 first when neither side calls a function (sqlite3ExprAnd): a case cut down past a table its predicate reads ran there and failed here. Amphora folds it so before resolving names | 1 | `expr/utils.rs` `fold_false_ands`; `select.rs`, `planner.rs`, `expr/binding.rs` | `a_subquery_a_settled_branch_drops_never_runs` |
| BE. A subquery held only in an AND or OR branch a literal settles ran before the loop, raising its error: SQLite codes no side sqlite3ExprSimplifiedAndOr drops, and no WHERE term past a literal 0 it tests before its loops | 1 | `subquery.rs` `drop_settled_subqueries`, `null_beside_false_where_term` | `a_subquery_a_settled_branch_drops_never_runs` |
| Q. Not Amphora's: evaluation order (SQLite's min() over an index stops at the first row its WHERE passes; json_valid's flags error in a subquery) | 2 | — | — |
| AG. CERT, the planner goal's | 58 | — | — |
