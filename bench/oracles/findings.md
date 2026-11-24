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

## The order-check campaigns' other findings, on step 5's build, 2026-10-27

The 43 findings of the order check's two campaigns on d762dc2
(`bench/records/2026-10-26-adas-mac-studio-2-order-check/`), replayed with `--replay-check` by a
release build of 5f16a6f's bench (`replay-5f16a6f.tsv` there, a row a finding with its cause): 13
that failed on d762dc2 pass, each by a cause above (AR, BN, BP, BQ, BS, BT, BV, BW); 7 passed on
d762dc2 already, their files reproducing nothing; 21 fail and are not Amphora's (AH 5, Q 15, AN 1);
and 2 failed as Amphora's. Those two are fixed here, each by a test that fails before its fix and
holds its finding's query:

| cause | findings | fix | test |
|---|---:|---|---|
| CB. jsonb_array(), jsonb_object(), jsonb_group_array() and jsonb_group_object() copied a JSONB argument's bytes, where SQLite writes each argument as JSON text and makes the whole text JSONB again (jsonAppendSqlValue, jsonReturnStringAsBlob): jsonb_array(NULL, jsonb_patch('1e3', 0)) kept jsonb_patch's widened header, x'5b00d3000130' for SQLite's x'3b001330' | 1 (diff-0d96c0783d6fbbce) | `json/mod.rs` (`convert_dbtype_to_element_jsonb`; the aggregates at their end, `json_from_raw_bytes_agg`) | `jsonb_containers_write_a_jsonb_argument_as_its_text_would` |
| CC. A call under a COLLATE kept a false AND from folding: SQLite's COLLATE node takes no flag from its operand (sqlite3ExprAddCollateToken sets only EP_Collate and EP_Skip), so `(1e308 ISNULL) AND (likelihood(NULL, NULL) COLLATE RTRIM)` is 0 there and the call is never resolved, where Amphora saw the call, kept the AND and refused the query ("second argument to likelihood() must be a constant") | 1 (error-a2539ea61c26b70f) | `translate/expr/utils.rs` (`calls_function` stops at a COLLATE) | `a_call_under_a_collate_folds_away_with_a_false_and` |
| CD. Found by CB's test's controls, not by a campaign: an element appended to the root array (jsonb_insert() or jsonb_set(), by '$[#]' or the index past its end) left the array's header one byte short of what it holds, the placeholder the path's traversal puts in first left uncounted where an object's array counts it: jsonb_insert('[]', '$[#]', 0) was x'1b1330', SQLite's x'2b1330' | — | `json/jsonb.rs` (`count_placeholder`) | `an_element_appended_to_the_root_array_counts_in_its_header` |

None of the 43 is left failing as Amphora's.

## The row-value seek's nightly run: seed 20261030, 1,800 s, 2026-10-30

adacachy, `nightly.sh` against the reference SQLite 3.54.0 (LIKE_DOESNT_MATCH_BLOBS), on t1/keyset-seek's
33599f1 (row values seek the index their columns lead: `docs/design/planner.md` decision 14), for its goal's
"CERT and the order check clean": 68,800 databases, 2,787,918 checks. CERT failed none of its 343,997
checks (32 queries SQLite refuses too) and the order check none of its 343,997. The other oracles made 4
findings (`bench/records/2026-10-30-adacachy-keyset-oracles/`: the summary, the findings, and
`replay.tsv`, each finding's `--replay-check` on c87e332's engine and on 33599f1, with its cause). Each
fails on both builds alike, so none is the seek's:

| cause | findings | fix | test |
|---|---:|---|---|
| AH. Not Amphora's (above): a LEFT, RIGHT or FULL JOIN's LIKE ON term on an INTEGER PRIMARY KEY, a NOCASE-indexed or a UNIQUE NOCASE column, where the reference SQLite answers no rows; the system's SQLite 3.54.0, built without the option, answers as Amphora | 3 | — (SQLite's) | — |
| CE. An IN subquery's NULL check (a miss told from a NULL in the list) compared by the builder's current collation, what the expression translated last left there: a WHERE whose CASE ends on `c0 != ('a ' COLLATE RTRIM)`, or a vector's first column, so `'' IN (SELECT c0 FROM t2)` with t2 holding ' ' answered NULL for 0. SQLite compares each column as `x = y` does (`sqlite3ExprCodeIN`, step 6), the collation the list's index has | 1 (eet-5d073f0af8f98282) | `translate/expr/translator.rs` (the check reads the list index's collations) | `an_in_lists_null_check_compares_each_column_as_equality_does` |

## Main's wide run: seeds 911, 912, 1001, 1011, 1012 and 1015, 2026-11-24 (lane C8)

adacachy2, against the oracles' SQLite 3.54.0 (`sqlite.sh`), release builds of this module on the engine
alone (`bench/src/oracle`, no competitor linked): main 67cc7e69 for the seeds from 1001, and lane
S7's bench binary (0101aa38 by its record, twelve files of `translate` and `vdbe` before it) for 911 and 912. Seed 1001 runs every oracle; 911, 912, 1011 and 1012 every
one but CERT and the order check; 1015 those less the differential check. 268,876 databases, 10,848,439
checks in the runs that ended (CPU, user and system: 5,138 s timed for seeds 1001, 1011, 1012 and 1015 on main; the other runs were not timed, and by their wall time at the 0.84 of a core the process list showed (9 min 10 s in 11 min for three of them at 08:22Z) they make it about 2.5 hours in all (inferred)); the ones cut short for room (901 to 904, 1002, 1013, 1014) left
findings but no summary. Every wrong answer, error and panic was replayed on main's build as a user
meets it, under four settings (`--replay`: as the database opens, the parallel mode on, with statistics,
both), and against the system's SQLite 3.54.0 as well. The summaries, every finding's cause and its
query (`findings.tsv`) and the files of those that are Amphora's are in
`bench/records/2026-11-24-adacachy2-oracle-c8/`.

| cause | findings | fix | test |
|---|---:|---|---|
| CF. A NOT EXISTS or NOT IN whose condition is an OR with terms on the outer table in its branches lost the outer rows no branch names (at defaults, with no statistics: `NOT EXISTS (SELECT 1 FROM r WHERE r.k = o.k AND ((o.st = 'open' AND r.kind = 1) OR (o.st = 'closed' AND r.kind = 2)))` answered no row for SQLite's 2). The subquery becomes an anti join whose condition stays a WHERE term; the OR pass took terms out of it (an IN list its branches imply, the table's share of it, a term all branches hold) as WHERE terms on the outer table, filters on the rows the anti join must keep | 6 | `translate/optimizer/lift_common_subexpressions.rs` (`rewrite_or_terms`: a term taken from an OR that reads an anti-joined table must read it too) | `a_not_exists_keeps_the_rows_no_branch_of_its_or_names` |
| CG. A LEFT JOIN dropped left rows: a multi-index AND on the left table took the join's ON term (`ON a0.c2 <= 100`, or `ON a0.c0 < 1e15`) as a filter of that table, its branch seeking by the ON term's bound, so the rows it should null-extend were gone. `multi_index_can_consume_term` let any term through for a table no outer join null-extends. Lane C9's sweep (bench/sqlite_on_terms.py) saw it with statistics too, and its other form: a partial index of an inner-joined table was taken for proven by the ON term of a later LEFT JOIN (`a JOIN b ON b.aid = a.id LEFT JOIN c ON b.live = 1 AND c.id = b.id`, `b`'s index `WHERE live = 1`: 2 rows for SQLite's 4) | 3 | `translate/optimizer/multi_index.rs` (`multi_index_can_consume_term`: an outer join's ON term drives its right table's scan alone), `translate/optimizer/constraints.rs` (`partial_index_predicate_terms`: it proves no other table's partial index) | `a_left_join_keeps_the_left_rows_an_on_term_of_theirs_refuses`; tests/on_terms.rs |
| CH. Panic ("indexbtree_seek_unpacked: index_info required", `storage/btree.rs`), or a SELECT that emptied the table it read first, or rows lost: a RIGHT or FULL JOIN's unmatched-row read ran a copy of each WHERE subquery, taken before it was planned, whose IN list kept a cursor the planner had released and the outer table had taken (bytecode: `OpenEphemeral 0`, which empties what the cursor holds), and whose hash tables were the loops' own. The read takes no copy now: the shared row body decides a term on a correlated subquery, and an uncorrelated one's result is read once for both | 4 | d8adcbe2 | `a_right_or_full_join_over_nested_subqueries_returns_sqlites_rows` (`tests/oracle_findings.rs`), `outer_joins_with_nested_subqueries_answer_as_sqlite` (`tests/outer_join_subqueries.rs`) |
| CI, open. json_pretty's indent from a REAL is written `100`, not `100.0`, SQLite's text of it | 1 | — | — |
| CJ, open. `json_object(7, a0.c3, 255)`'s odd argument count raised where SQLite answers: in a correlated min() over an empty table, the side of a BETWEEN that reads no inner column is computed ahead of the rows (inferred, not traced) | 1 | — | — |
| CK, by design. A REAL's text where SQLite 3.54.0 writes 17 digits and floatingpoint.html 1.3.1 says 15 (`7.3890560989306504` for 7.38905609893065): Amphora follows the page (`numeric/fp.rs`, lane G10). Seen through text, hex(), CAST AS BLOB, JSONB, length(), instr(), replace() and concat_ws() | 245 | — | — |
| CL, open, estimates and no answer. CERT: an outer join estimated below its inner join when both an ON and a WHERE term filter one table (0.4 x 0.9 in the outer form, 0.4 x the square root of 0.9 in the inner; inferred from those numbers, not traced: decision 21's backoff combines a table's filters in one form and not in the other). The order check: `ON (x OR y)` and the same as a WHERE term differ (0.5 for 0.35). None failed on 2026-11-06 (8567e7b, 540,848 checks of each) | 372 | — | — |
| CM. A RIGHT or FULL JOIN whose right table is WITHOUT ROWID (an experimental feature) with a secondary index errored ("secondary index lacks primary key locator"), with a subquery or without: `SELECT count(*) FROM e AS a0 FULL JOIN w AS a2 ON a0.k = a2.v` over `w(k INT PRIMARY KEY, v INT) WITHOUT ROWID` with an index on v: SQLite 3. The read of the unmatched rows opened its index cursor with the index's declared columns, without the primary key its entries end in, which finds the row; the loops' cursor has it. Both take it from one place now (lane C10's sweep; lane C11's CQ) | — | lane C13: `translate/plan.rs` (`index_of_cursor`), `main_loop/close.rs` | `a_right_or_full_join_reads_a_without_rowid_tables_secondary_index` (`tests/oracle_findings.rs`), `on_clauses_over_ctes_and_joins_of_keyed_tables_answer_as_sqlite` (`tests/outer_join_subqueries.rs`) |
| CN (lane C10's sweep). Where a join is a hash join, a WHERE term on a subquery was not tested, or tested with another row's value: rows that SQLite drops came out, and rows were lost, with statistics (`SELECT a0.k, a1.k, a2.k FROM p AS a0 JOIN q AS a1 ON a0.k = a1.k LEFT JOIN e AS a2 ON a1.k = a2.v WHERE a2.k IN (SELECT n.k FROM n WHERE n.v = a0.k)`, e empty: SQLite 0 rows, Amphora 4) and at defaults (`... FROM n AS a0 JOIN p AS a1 ON a0.k = a1.k LEFT JOIN q AS a2 ON a1.k = a2.k LEFT JOIN g AS a3 ON a2.k = a3.v WHERE coalesce((SELECT count(s1.k) FROM n AS s1 WHERE s1.k IS a3.v), a1.k) = a0.k`: SQLite 0, Amphora 12). Four causes: a materialized build input took the loop of a table a correlated subquery reads; the unmatched rows had a copy of the WHERE terms that dropped one; a term with a correlated subquery was a hash key; an uncorrelated subquery was coded twice, its hash join built by the copy nothing ran | — | 7e9e52b1 | `a_hash_joins_rows_are_tested_by_where_terms_on_correlated_subqueries` (`tests/oracle_findings.rs`), `joins_filtered_by_subqueries_answer_as_sqlite_by_every_join_method` (`tests/outer_join_subqueries.rs`) |
| CO. A CTE read by a subquery in an ON clause (or in a table function's argument) was not found, in any join, or a table of its name was read: `WITH c AS (SELECT x FROM r) SELECT a.x, b.x FROM o AS a JOIN o AS b ON a.x IN (SELECT c.x FROM c)`: SQLite answers, Amphora "no such table: c". The CTEs that subqueries read were gathered from every clause but the FROM clause's own expressions (lane C10's sweep) | — | lane C13: `translate/planner.rs` (`collect_subquery_table_refs_in_from_exprs`) | `a_subquery_in_an_on_clause_reads_a_cte`, `a_write_whose_join_reads_a_cte_in_its_on_clause_writes_sqlites_rows`, and the sweep |
| CP (lane C10's sweep; lane C11's record names another finding CP). A table function's untyped column compared with a TEXT column as a number: `SELECT p.s, j.key FROM p, json_each('[5,6,7]') AS j WHERE p.s = j.key` with p.s = '1': SQLite 0 rows, Amphora 1 (`'1' = 1`); and in `NOT IN (SELECT key FROM json_each(...))`. json_each and json_tree declared key, value and atom ANY, which outside a STRICT table is NUMERIC affinity (datatype3.html 3.1, rule 5). SQLite 3.54.0 declares no type, BLOB affinity; json1.html 4.10 writes ANY, read as stricttables.html has it, any value as it is. type, fullkey, path, root (TEXT), id and parent (INTEGER) stay as the page declares them, where SQLite declares none | — | lane C21: `json/vtab.rs` | `a_table_functions_untyped_column_compares_as_it_is` (`tests/oracle_findings.rs`), `table_functions_untyped_columns_compare_as_sqlites_do` (`tests/columns_read.rs`) |
| CQ (lane C10's sweep; lane C11's CQ is CM). A table function right of a RIGHT or FULL JOIN whose argument holds a correlated subquery read the argument of the row before: `SELECT a.k, je.value FROM a FULL JOIN json_each((SELECT t.j FROM t WHERE t.k = a.k AND EXISTS (SELECT 1 FROM r JOIN l ON r.x = l.y))) AS je ON je.value = 7`: SQLite `1,7; 2,NULL`, Amphora `1,NULL; 2,7`; and, with an uncorrelated subquery that hash joins inside it and a left table with no row, lost rows (SQLite 2, Amphora 0). The subquery was put after the function's own loop, which reads the argument as it opens: the rule that holds a subquery to a RIGHT or FULL JOIN's right table, for the loops its unmatched rows run again, left out the table's own ON terms but not its arguments. And the unmatched rows' read coded the subquery a second time, its hash join built by the first. It is coded once and run at each read of the argument, the pass's too | — | lane C21: `optimizer/mod.rs` (`plan_correlated_subqueries`), `subquery.rs` (`assign_select_subquery_eval_phases`), the copy removed from `main_loop/close.rs` | `a_table_functions_correlated_argument_is_the_row_in_hands`, `table_functions_with_correlated_arguments_answer_as_sqlite` |
| CR (lane C10's sweep). A subquery in an EXISTS, NOT EXISTS or NOT IN, whose own IN has a column of its table on the left and a subquery correlated with the outer query, lost rows where the outer one is unnested (every join a hash join, `JoinMethods::HashJoinsOnly`; over hundreds of rows at defaults too, `SELECT count(*) FROM o WHERE NOT EXISTS (SELECT 1 FROM e WHERE e.v = o.v AND e.k IN (SELECT p.k FROM p WHERE o.v = p.v))`: SQLite 300, Amphora 0): `SELECT o.k FROM o WHERE NOT EXISTS (SELECT 1 FROM e WHERE e.k IN (SELECT p.s FROM p WHERE o.v = p.v))`, e empty: SQLite every row, Amphora none. The NOT EXISTS became an anti join to e, and the IN it brought into the query a semi join to p beside it, which a row the anti join kept had to match. The guard that keeps a subquery of a joined table nested read the subquery's own outer columns, not the term it stands in. The finding's own query (seed z1, `indexed`) was this and a LEFT JOIN around it | — | lane C21: `optimizer/unnest.rs` (`reads_semi_or_anti_joined_table`) | `an_in_beside_an_anti_joined_table_stays_in_its_subquery`, `subqueries_in_subqueries_answer_as_sqlite_by_every_join_method` |
| CX (lane C11's campaign). A trigger's new.c and old.c lost the column's collation: `WHEN new.name = 'admin'` over a NOCASE column, name 'ADMIN': SQLite fires, Amphora never; in a body too, and in IN, a row value, nullif, EXISTS and an index seek. The body had an `Expr::Variable` and the WHEN an `Expr::Register`, which carry none; and a body's new.id had no affinity, where the rowid's is INTEGER. What the reference keeps of its column is in the expression now (`ast::TriggerColumn`, SQLite's TK_TRIGGER) | 6 | lane C21: `trigger_exec.rs`, `collate.rs`, `expr/affinity.rs` | `a_triggers_new_and_old_keep_their_columns_collations`, `triggers_read_their_rows_as_sqlites_do` |
| DE (lane C11's campaign). A column lost its collation in an aggregate's argument where a sort makes the groups (a GROUP BY on a subquery's answer): `max(quote(c1 IS 'abc'))` (c1 NOCASE, 'ABC'): 1 / 0; in a FILTER, and in a window function over an expression. The columns the sorted rows give the aggregates' step were cached without their collation | 1 | lane C21: `group_by.rs`, `window.rs` | `an_aggregates_argument_compares_by_its_columns_collation`, `aggregates_of_collated_arguments_answer_as_sqlite` |
| DK, open (lane C21's sweep). A virtual table's rowid 0 reads NULL (`op_row_id`, vdbe/execute.rs: `(rowid != 0).then_some(rowid)`): `SELECT rowid FROM json_tree('[7]')` gives NULL for the root (SQLite 0), and a RIGHT or FULL JOIN to json_tree gives its root again as unmatched: `SELECT x.k, je.id FROM a AS x RIGHT JOIN json_tree('[7,1]') AS je ON 1` over two rows: SQLite 6, Amphora 7 | — | — | — |
| DF. A CTE read twice, once through another CTE that a subquery reads (or a WITH of the subquery's own), in any clause: a plain one lost rows (`WITH c AS (SELECT k FROM q), d AS (SELECT k FROM c) SELECT c.k FROM c WHERE (SELECT count(*) FROM d) = 3`: SQLite 3, Amphora 1), a recursive one panicked ("cursor id 0 is None"). The second read had a copy of the first's plan, its tables' IDs and so its cursors: a recursive CTE was never planned anew for a read in a subquery, and a lookup took a FROM clause's use of a CTE for its definition (lane C13's sweep) | — | lane C13: `translate/planner.rs` (`parse_table`), `translate/plan.rs` (`cte_definition`) | `a_cte_read_twice_once_through_another_gives_each_read_its_own_rows`, and the sweep |
| DG. A subquery whose own WITH defines a CTE with the name of a table (or a CTE) of the query around it could not read that table's columns: `SELECT c.k FROM q AS c WHERE c.k IN (WITH c AS (SELECT k FROM t) SELECT x.k FROM c AS x WHERE x.k = c.k)`: SQLite 2 rows, Amphora "no such column: c.k". The CTE's use hid the first outer entry of its name, whichever it was (lane C13's sweep) | — | lane C13: `translate/plan.rs` (`mark_outer_query_ref_cte_definition_only`) | `an_outer_tables_columns_stay_in_reach_under_a_cte_of_its_name`, and the sweep |
| DH. A CTE whose WITH clause stands inside another that defines the same name was not read by a subquery below it, which read the outer one: `WITH d AS (SELECT k FROM q) SELECT t.k FROM t WHERE EXISTS (WITH d AS (SELECT k FROM t WHERE k = 1) SELECT 1 FROM d AS s WHERE t.k IN (SELECT e.k FROM d AS e))`: SQLite 1, Amphora 2. The name took the outermost entry; it takes the innermost definition's now, and each use of a CTE is planned from where it is defined: an entry holds its WITH clause and the names around that, and not a plan or its syntax (lane C13's sweep) | — | lanes C13 and C18: `translate/plan.rs` (`find_cte_outer_query_ref_by_identifier`, `CteScope`), `translate/planner.rs` (`cte_with`, `plan_cte`, `parse_table`) | `a_cte_name_is_the_innermost_with_clauses_that_defines_it`, `each_use_of_a_cte_has_the_rows_and_the_names_of_its_definition`, and the sweep `ctes_of_with_clauses_inside_one_another_answer_as_sqlite` |
| DI. A CTE that only an unread CTE of an inner WITH names was planned, and its error raised; and one whose name an inner WITH defines; and two of one WITH clause were circular where a body's unread or inner CTE named the other: `WITH d AS (SELECT k FROM nosuch), f AS (SELECT k FROM q) SELECT t.k FROM t WHERE t.k IN (WITH x AS (SELECT k FROM d) SELECT k FROM f)`: SQLite 1 row, Amphora "no such table: nosuch". `collect_from_clause_table_refs` took every name in the bodies of inner CTEs, read or not, and the names those CTEs take (lane C13's sweep) | — | lane C18: `translate/planner.rs` (`collect_from_clause_table_refs`, `collect_subquery_table_refs_in_expr`, `RecursiveRefCounter::push_nested_ctes`) | `a_cte_that_nothing_reads_is_no_part_of_the_statement`, and the sweep `ctes_of_with_clauses_inside_one_another_answer_as_sqlite` |
| DK. Uses of a CTE that shared one plan, or looked names up from where they stood. Two CTEs of an inner WITH clause that read one outer CTE (or one and the body that defines it): `WITH c AS (SELECT k FROM q) SELECT * FROM (WITH a AS (SELECT k FROM c), b AS (SELECT k FROM c) SELECT a.k AS ak, b.k AS bk FROM a, b)`: SQLite 9 rows, Amphora 3; over a recursive c a panic, "cursor id 0 is None"; a CTE that reads a row of the query around it, read twice by another: SQLite 9, Amphora 3. A view's body read the CTEs of the query that reads the view; a subquery in FROM, under a WITH that defines an outer CTE's name, the outer one; a LIMIT's subquery no CTE, or the table of its name; a CTE's body no column of a table around it that has the CTE's name; an INSERT's WITH and its SELECT's were one clause; a column list that names too few was not refused for a CTE that another one reads; the left side of IN (SELECT) found no CTE (lane C18's sweep) | — | lane C18: `translate/planner.rs` (`parse_table`, `cte_name_entry`, `cte_with`, `parse_from_clause_table`), `translate/subquery.rs` (LIMIT), `translate/insert.rs` | `each_use_of_a_cte_has_the_rows_and_the_names_of_its_definition`, and the sweep `ctes_of_with_clauses_inside_one_another_answer_as_sqlite` |
| DL, open, not traced (lane C18's sweep). A FROM clause's subquery inside a HAVING's subquery loses the grouped row it reads: `SELECT q.k, count(*) FROM q GROUP BY q.k HAVING EXISTS (SELECT 1 FROM (SELECT a3.k AS k FROM q AS a3 WHERE a3.k = q.k) AS a4)`: SQLite 2 rows, Amphora 0 (the same without the FROM clause's subquery: 2); no CTE in it | — | — | — |
| DM, open, not traced (lane C18's probe). A compound's LIMIT that is a subquery is refused: `SELECT 1 UNION ALL SELECT 2 LIMIT (SELECT count(*) FROM q)`: SQLite answers, Amphora "datatype mismatch" | — | — | — |
| DN, open, not traced (lane C18's sweep). In a CTE g, a UNION [ALL] arm's `FROM g` is SQLite's recursive table, even where g's own body has a WITH that defines g; Amphora reads that inner g: `WITH g AS (WITH g AS (SELECT 1 AS k) SELECT k + 1 AS k FROM g WHERE k > 9 UNION ALL SELECT k FROM g) SELECT k FROM g`: SQLite no row, Amphora 1 | — | — | — |
| DJ, open, not traced (lane C12's sweep). A row value's NOT IN where a value is NULL answers otherwise than SQLite, by every join method and with no hash join: `SELECT a0.k, a1.k FROM g AS a0 FULL JOIN e AS a1 ON a0.k = a1.k WHERE (a1.k, a0.k) NOT IN (SELECT s1.k, s1.v FROM f AS s1 WHERE s1.k <= s1.k)` (e empty): SQLite 3 rows, Amphora 0; `SELECT a0.k, a1.k, a2.k FROM n AS a0 JOIN g AS a1 ON a0.k = a1.k JOIN p AS a2 ON a1.k = a2.v WHERE (a2.k, a1.k) NOT IN (SELECT s1.k, s1.v FROM n AS s1 WHERE s1.v IS NOT NULL)`: with nested loops only, SQLite leaves out the row with a2.k NULL that Amphora gives | — | — | — |
| AH. Not Amphora's (above): the reference SQLite's LIKE optimization in an outer join; the system's SQLite answers as Amphora | 13 | — (SQLite's) | — |
