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
`an_unread_subquery_never_runs`), a correlated one too: in an aggregate's or a GROUP BY's output, a
window query, a RIGHT JOIN query and a write statement (`a_correlated_subquery_no_row_reads_never_runs`).

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

## The order check: one estimate whatever the join order (planner.md decision 13), 2026-10-26

The order check (`checks.rs`, `Case::Order`) plans an inner join of two to four tables with its
FROM list in every order, its ON terms moved to WHERE, and each order forced by CROSS JOIN, and
holds every form to the estimate of the query as written within 1e-9 relative. On dbef789 a run of
it alone (seed 11, 180 s, adas-mac-studio-3) failed 33 of 435 checks. Each cause below was found
by it, cut down by the reducer, and fixed with a test in `crates/engine/tests/planner.rs` that fails
before its fix and passes after.

| cause | fix | test |
|---|---|---|
| BF. A join's estimate was the access path's: a seek's rows per outer row times the filters it didn't consume, each term's share moved by the order its tables came in. A seek keyed by one bound of a range took the bounds as two (0.4 × 0.4) where the filter form took a closed range (0.005); a seek and NOT INDEXED estimated one join 100 times apart; a LEFT JOIN came below its inner join | `optimizer/cardinality.rs` (`JoinedSetRows`: one estimate per set of tables), `optimizer/join.rs` (homes tied by name) | `every_order_of_an_inner_join_gets_one_estimate`, `a_constraint_seeks_or_filters_to_one_estimate`, `an_outer_join_is_estimated_at_least_its_inner_join` |
| BG. An ON term kept its BETWEEN where a WHERE term has it split into two comparisons, so the ON form constrained nothing | `planner.rs` `split_rewritten_ands` | `an_on_term_and_a_where_term_give_one_estimate` |
| BH. An EXISTS unnested to a semi or anti join left its condition homed on the outer table, a filter of it, where the correlated form counts nothing | `optimizer/join.rs` (`build_where_term_info`: a term reading a semi or anti join's table has no home) | `an_on_term_and_a_where_term_give_one_estimate` |
| BI. A term no row passes: `WHERE (x'' ISNULL) AND NULL`, folded by the parser, planned impossible with no estimate at all, while `ON NULL`, `x'' ISNULL` alone and (campaign on a2f7ca6, seed 11) `ON (0 OR (255 ISNULL))` planned every row | `optimizer/mod.rs` (an impossible plan estimates 0 rows at each access), `cardinality.rs` `never_true` | `an_on_term_and_a_where_term_give_one_estimate` |
| BJ. A scalar subquery unnested to a table grouped by its correlation key was estimated at that table's rows times the key's selectivity for each outer row (100,000) where it gives one, and the correlated form one | `cardinality.rs` `grouped_by_keys` | `an_on_term_and_a_where_term_give_one_estimate` |
| BK. A term reading one table on both sides (`(SELECT max(...) WHERE s.c2 = a0.c1) IS NOT ('-1' <= a0.c2)`) gave a constraint for each side and counted twice there, 0.81 for 0.9 (campaign on a2f7ca6, seed 12) | `cardinality.rs` (a term counts once per table) | `an_on_term_and_a_where_term_give_one_estimate` |
| BL. CERT, of BG's split: with an ON term's BETWEEN split, a bound by the other table's column (`a1.c1 BETWEEN 0 AND a0.c2`) made a closed range on the right table in the outer form but was counted on the left table in the inner form, so an outer join came below its inner join (CERT on a2f7ca6 and 878db51, seed 20261025: 83 of 84 findings new). Only bounds by a pseudo-constant make a range now, as PostgreSQL's `clauselist_selectivity` pairs them; a right table's WHERE filters count what they add to its ON terms | `cardinality.rs` (`selectivity_of`, `filters_given_on`) | `an_outer_join_is_estimated_at_least_its_inner_join` |
| BM. CERT: a comparison with a NULL literal (`a0.c0 >= NULL`, from `BETWEEN NULL AND x`), an AND holding one, or a never-true term the settled-AND handling had consumed, estimated as if it passed rows; and `WHERE NULL AND (SELECT ...)` reported its uncorrelated subquery's rows as the query's `output_rows` | `cardinality.rs` `never_true`, `matches_none`; `emitter/select.rs`, `eqp.rs` (`join_output_rows`) | `an_on_term_and_a_where_term_give_one_estimate` |

A form of the query Amphora refuses is a finding only where SQLite answers it, as for every derived
query (`derived`): `ON 0 WHERE min()` is planned impossible and never checks min()'s arguments, so
it runs where the forms with the WHERE folded, and SQLite, refuse it ("wrong number of arguments").
That is not an estimate, and SQLite refuses it too; it is left to the refusal checks.

CERT beside it (cause AG, the planner goal's: `--oracles cert`, seed 20261025, 300 s, against the
oracles' SQLite, adas-mac-studio-2): dbef789 found 71 findings; on d762dc2 every one of them passes
`--replay-check`, and a run of d762dc2 found 6, each failing on dbef789 too (`--replay-check` with
dbef789's binary): none new. What is left of AG is step 5's (kiln plan 01a16eab7f457673b4cf9933adacfc4d).

The campaigns on the change's last commit, d762dc2 (`bench/records/2026-10-26-adas-mac-studio-order-check.md`):
seed 11, 1,800 s, 375,206 order checks, none failed; seed 12, 1,800 s, 385,255, none failed.
Run again with their summaries kept (`bench/records/2026-10-26-adas-mac-studio-2-order-check/`):
seed 11, 502,632 order checks, none failed; seed 12, 530,789, none failed. Their 43 other findings
replay alike on dbef789 and d762dc2 (36 fail on both, 7 pass on both: `replay.jsonl` there): none
is the change's.

## CERT clean: step 5 of the planner's plan, 2026-10-27

Kiln plan 01a16eab7f457673b4cf9933adacfc4d, step 5: AG's open causes reproduced and fixed, CERT made
to cover every shape the generator makes, and the oracles run with seeds 11, 12 and 13 and
`nightly.sh` for three nights' seeds, against the oracles' SQLite (`sqlite.sh`), on
adas-mac-studio-2 (`bench/records/2026-10-27-adas-mac-studio-2-cert-clean/`).

**CERT's estimates.** Every CERT finding of campaigns 1 to 4 and the injected run (126) passes
`--replay-check` on step 5's last tree (10f2461), and so do the five of step 5's own runs
(`replay-cert.tsv` in the record). Campaign 2's two AG findings pass since decision 13's joined-set rows (d762dc2); what AG
left was one cause (BN). EXPLAIN QUERY PLAN FORMAT=JSON's top-level `output_rows` is now the
statement's rows for every shape (decision 13 of `docs/design/planner.md`), and CERT checks grouped,
aggregated, DISTINCT, ordered, limited and compound queries as well as plain ones.

**CERT's "no estimate".** Campaign 2's 2,049 were no query planned without an estimate: a CERT run of
d762dc2 logging each of them (120 s, 105,989) found every one a query Amphora refused, nearly all
refused by SQLite too. The generator read SQLite's `narg` of -(k + 2), "at least k arguments", as
none (`iif()`, `coalesce()`, `concat()`, `concat_ws()`), called min and max with the one argument
that makes them aggregates, gave likelihood a probability that isn't a REAL literal, and made GROUP
BY and ORDER BY terms SQLite refuses (an aggregate in a GROUP BY; an integer, which is a column's
number). It now makes queries SQLite takes; a query Amphora can't plan counts as SQLite refusing it
or as a refusal, and "no estimate" is only a query planned without one.

| cause | findings | fix | test |
|---|---:|---|---|
| AG (campaign 2's): a FULL JOIN estimated below the inner join a conjunct makes it (cert-70d3e0baf8df5914) | 1 | decision 13's joined-set rows (d762dc2) | `a_full_join_is_estimated_at_least_its_inner_join` |
| AG (campaign 2's): an always-true conjunct moved the join order, and with it a seek's closed range against two filters' (cert-bae9c4dcf94932e8) | 1 | the same | `an_always_true_conjunct_raises_no_estimate` |
| AG (campaigns 3 and 4, the injected run) | 85 | the same: each passes `--replay-check` | decision 13's tests in `planner.rs` |
| BN. CERT: an IS NULL of a column no row holds NULL in (`NULL IS c`, `c IS NOT DISTINCT FROM NULL`) counted as IS NULL's tenth of the rows on a table no join null-extends, where its LEFT JOIN form keeps only the unmatched rows (cert-e21949a38a9b16d7, cert-2732f12b21087328, cert-2c3a9d2455aef93d, cert-d03ea211f1fab06d on d762dc2); then an inner join's ON term on a table a later RIGHT JOIN null-extends (cert-60e65956aed5c593, step 5's first nightly run) | 5 | `optimizer/cardinality.rs`: `Class::Never`, an ON term of the table's own outer join matching none, a table null-extended only by the joins up to the term's own (SQLite's NOT NULL strength reduction) | `an_is_null_of_a_column_never_null_keeps_no_inner_row` |
| AP. The oracles': CERT's "no estimate", queries Amphora refused, and the generator's calls and terms SQLite refuses (above) | — | `synth.rs`, `checks.rs` | `every_top_level_plan_shape_has_an_estimate` |

**The other oracles.** With calls at arities SQLite takes, the generator reached functions it had
never called validly, and the first runs (a 120 s run of every oracle, seed 1012, then the six of the
step's seeds, 1,800 s each) found causes campaign 2 had not:

| cause | findings | fix | test |
|---|---:|---|---|
| BO. concat_ws gave NULL for a BLOB separator and left BLOB arguments out | 10 | `vdbe/value.rs` (SQLite's `concatFuncCore`) | `concat_ws_writes_every_argument_but_null_a_blob_as_its_text` |
| BP. Panic ("cursor id N is None"): a RIGHT or FULL JOIN's unmatched-row pass ran a copy of a subquery whose multi-index OR opened cursors of its own and sought on the original's, never opened where the main loop matched nothing | 9 | `main_loop/init.rs` (a branch's cursor is the one its key names) | `an_unmatched_row_pass_seeks_its_own_subquerys_cursors` |
| BQ. A multi-index OR's branch took the range a GLOB implies for the GLOB, keeping every row in it (`(NULL < c0) OR (c0 GLOB '[a-z]*')`) | 3 | `optimizer/multi_index.rs` (an implied constraint consumes no term) | `a_multi_index_or_still_filters_a_globs_range` |
| AR (campaign 3's), the Affinity instruction: a decimal number before a NUL stayed text where a seek's key or an IN probe takes affinity | 8 | `vdbe/affinity.rs` | `a_seek_or_probe_reads_the_number_before_a_nul` |
| BR. A rowid seek refused a REAL that is exactly -2^63 | 2 | `vdbe/execute.rs` (`seek_rowid_key`, SQLite's OP_SeekRowid) | `a_rowid_seek_finds_the_smallest_rowid_by_a_real` |
| BS. A multi-index AND sought indexes its comparisons' affinity or collation can't use (`CAST(1e15 AS INTEGER) BETWEEN c1 AND c0` by c0's TEXT index) | 1 | `optimizer/constraints.rs` (`comparison_fits_column`) | `a_multi_index_and_seeks_only_indexes_its_comparisons_fit` |
| AV (campaign 3's), unnested: a correlated IN unnested to a semi-join on `=`, which compares an integer with a lone REAL column exactly where IN makes the probe a REAL | 1 | `optimizer/unnest.rs` (`in_compares_as_equality`) | `an_unnested_in_compares_as_in_does` |
| BT. sum() turned its integer total into a REAL whole when text or a BLOB came, losing the low bits SQLite's split keeps (kahanBabuskaNeumaierInit) | 6 | `vdbe/execute.rs` | `sum_keeps_a_large_integer_totals_low_bits_where_it_turns_real` |
| BU. abs() dropped a negative zero's sign, and REAL arithmetic read text by the integer it parsed to | 4 | `vdbe/value.rs`, `numeric/mod.rs` | `a_negative_zero_keeps_its_sign_through_abs_and_text_arithmetic` |
| BV. unicode() of a BLOB read it as lossy text (65533) where SQLite reads its first character as `sqlite3Utf8Read` does | 4 | `vdbe/value.rs` | `unicode_reads_a_blobs_first_character_as_sqlite_does` |
| BW. timediff() took dates before Julian day 0, which SQLite's date functions refuse (`validJulianDay`) | 1 | `functions/datetime.rs` | `timediff_of_a_date_before_julian_day_zero_is_null` |
| BX. atanh() near -1 lost half its digits (std's, not computed on \|x\|) | 1 | the libm crate's `atanh`, then BY's | `atanh_is_odd_and_keeps_its_digits_near_minus_one` |
| The pager's "dirty pages should be empty for read txn" (eet-abd78b596ec27e54) and a pivot missing in the same database (pqs-263b8b1d13a60214): neither reproduces from its file, both from their database (seed 11, state 50178) on 564bd15, neither on step 5's build | 2 | among this table's fixes | — |
| AH. Not Amphora's: SQLite's LIKE optimization, built with SQLITE_LIKE_DOESNT_MATCH_BLOBS, drops an outer join's rows for a LIKE ON term on a NOCASE or indexed column (`t2 LEFT JOIN t0 ON t0.c0 LIKE 'a%'` with t0 empty: no rows); the system's SQLite, without the option, answers as Amphora | 11 | — (SQLite's) | — |
| Q. Not Amphora's: evaluation order. PQS met a JSON function's error SQLite raises too on a value the query reads, where SQLite's plan stopped sooner (an EXISTS at its first match, min() over an index at its first row; 32 across 16 messages), and sum()'s overflow, which SQLite meets too reading the rows in rowid order (2) | 34 | the oracles: PQS now skips such an error as the other checks do | — |
| AN. Not Amphora's: `lower()` of a JSONB value, text that isn't well-formed UTF-8 | 1 | the oracles: PQS skips it | — |
| AP. The oracles': TLP held a REAL sum's partitions to the query's where infinities make a NaN in one order of the rows and not in another (5); a LIMIT the reducer left without its ORDER BY (2); relations SQLite's own answers break, a GROUP BY that keeps 0 and 0.0 apart under a HAVING (TLP) and EET's `- 0.0` against `-(CASE .. 0.0 END)` (2) | 9 | `checks.rs`: such cases skipped | — |

**The second runs.** The six runs again on 19a78a5 (2026-10-27, 07:02 to 07:32Z): CERT failed none
of its 418,760 to 432,615 checks a run, with no "no estimate" (40 to 50 queries SQLite refuses too, a
run); the other oracles made 46 findings (45 distinct):

| cause | findings | fix | test |
|---|---:|---|---|
| BY. atanh()'s last digit: the libm crate's (BX's fix) is an ulp below SQLite's C library at 0.5, 0.54930614433405478 where macOS's and glibc 2.44's give 0.54930614433405489; and the platforms' C libraries differ from each other (0.25) | 18 | `vdbe/value.rs`: macOS's computation on Apple platforms, the correctly rounded value elsewhere (decision 2 of `docs/design/semantics.md`) | `atanh_has_its_platforms_c_library_digits` |
| BZ. A RIGHT or FULL JOIN's unmatched-row pass under an OFFSET: a row the OFFSET skipped jumped past the Return of the subroutine the pass enters the body by, and the pass ran on as the main loop, keeping or losing rows | 9 | `main_loop/body.rs` (`offset_continue_label`: the Return, as SQLite's sqlite3WhereEnd) | `an_offset_skips_a_right_joins_rows_in_either_pass` |
| CA. json_error_position put a signed number's leading zero error at the digit after the zero, where SQLite puts it at the zero: timediff(0, 0), '+0000-00-00 00:00:00.000', gave 3, not 2 | 1 | `json/jsonb.rs` (one past the number's start, as jsonTranslateTextToBlob) | `json_error_position_reports_a_signed_leading_zero_at_the_zero` |
| AH. Not Amphora's (above) | 13 | — (SQLite's) | — |
| Q. Not Amphora's: evaluation order. A JSON call reading no column, in an EXISTS over an empty table, which Amphora computes once ahead of the rows and SQLite on each (campaign 2's zeroblob) | 4 | the oracles: PQS evaluates such a call on no rows | — |
| AP. The oracles': a DISTINCT query's LIMIT, where DISTINCT keeps 0 or 0.0, whichever the plan meets first, and the ORDER BY tells them apart | 1 | `checks.rs`: a DISTINCT or grouped query's LIMIT keeps no certain rows | — |

Each of BY's, BZ's and CA's findings passes `--replay-check` on 10f2461, and none of the others does
(`replay-round2.tsv` in the record): AH's are SQLite's, and the oracles now skip Q's and AP's states
rather than Amphora answering otherwise.

**The third runs.** The six runs on 10f2461 (2026-10-27, 08:15 to 08:45Z): CERT failed none of its
2,607,858 checks (431,435 to 438,746 a run), with no "no estimate" (41 to 52 queries SQLite refuses
too, a run) and none Amphora refused. The other oracles made 13 findings, each the differential
check's and each AH's, the same 13 as the second runs': no cause campaign 2 did not find
(`third-runs/*/causes.tsv` in the record).
