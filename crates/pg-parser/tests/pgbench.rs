//! T-PG.6.4: every statement pgbench sends parses to PostgreSQL 19's tree, byte for byte as the
//! oracle prints it (`nodeToStringWithLocations`). The statements are pgbench.c's at the vendored
//! commit with its variables substituted as `-M simple` substitutes them (a negative and a positive
//! delta) and numbered `$n` as `-M extended` numbers them; the texts are the oracle's on adacachy2
//! (lane PG8, run r1711).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros
)]

use amphora_pg_parser::gram;

/// Each statement beside the oracle's text for it.
const PGBENCH: &[(&str, &str)] = &[
    (
        r#"UPDATE pgbench_accounts SET abalance = abalance + -2741 WHERE aid = 34521;"#,
        r#"({RAWSTMT :stmt {UPDATESTMT :relation {RANGEVAR :catalogname <> :schemaname <> :relname pgbench_accounts :inh true :relpersistence p :alias <> :location 7} :targetList ({RESTARGET :name abalance :indirection <> :val {A_EXPR :name ("+") :lexpr {COLUMNREF :fields ("abalance") :location 39} :rexpr {A_CONST :val -2741 :location 50} :rexpr_list_start 0 :rexpr_list_end 0 :location 48} :location 28}) :whereClause {A_EXPR :name ("=") :lexpr {COLUMNREF :fields ("aid") :location 62} :rexpr {A_CONST :val 34521 :location 68} :rexpr_list_start 0 :rexpr_list_end 0 :location 66} :fromClause <> :returningClause <> :withClause <>} :stmt_location 0 :stmt_len 73})"#,
    ),
    (
        r#"UPDATE pgbench_accounts SET abalance = abalance + 2741 WHERE aid = 34521;"#,
        r#"({RAWSTMT :stmt {UPDATESTMT :relation {RANGEVAR :catalogname <> :schemaname <> :relname pgbench_accounts :inh true :relpersistence p :alias <> :location 7} :targetList ({RESTARGET :name abalance :indirection <> :val {A_EXPR :name ("+") :lexpr {COLUMNREF :fields ("abalance") :location 39} :rexpr {A_CONST :val 2741 :location 50} :rexpr_list_start 0 :rexpr_list_end 0 :location 48} :location 28}) :whereClause {A_EXPR :name ("=") :lexpr {COLUMNREF :fields ("aid") :location 61} :rexpr {A_CONST :val 34521 :location 67} :rexpr_list_start 0 :rexpr_list_end 0 :location 65} :fromClause <> :returningClause <> :withClause <>} :stmt_location 0 :stmt_len 72})"#,
    ),
    (
        r#"UPDATE pgbench_tellers SET tbalance = tbalance + -2741 WHERE tid = 7;"#,
        r#"({RAWSTMT :stmt {UPDATESTMT :relation {RANGEVAR :catalogname <> :schemaname <> :relname pgbench_tellers :inh true :relpersistence p :alias <> :location 7} :targetList ({RESTARGET :name tbalance :indirection <> :val {A_EXPR :name ("+") :lexpr {COLUMNREF :fields ("tbalance") :location 38} :rexpr {A_CONST :val -2741 :location 49} :rexpr_list_start 0 :rexpr_list_end 0 :location 47} :location 27}) :whereClause {A_EXPR :name ("=") :lexpr {COLUMNREF :fields ("tid") :location 61} :rexpr {A_CONST :val 7 :location 67} :rexpr_list_start 0 :rexpr_list_end 0 :location 65} :fromClause <> :returningClause <> :withClause <>} :stmt_location 0 :stmt_len 68})"#,
    ),
    (
        r#"UPDATE pgbench_branches SET bbalance = bbalance + -2741 WHERE bid = 1;"#,
        r#"({RAWSTMT :stmt {UPDATESTMT :relation {RANGEVAR :catalogname <> :schemaname <> :relname pgbench_branches :inh true :relpersistence p :alias <> :location 7} :targetList ({RESTARGET :name bbalance :indirection <> :val {A_EXPR :name ("+") :lexpr {COLUMNREF :fields ("bbalance") :location 39} :rexpr {A_CONST :val -2741 :location 50} :rexpr_list_start 0 :rexpr_list_end 0 :location 48} :location 28}) :whereClause {A_EXPR :name ("=") :lexpr {COLUMNREF :fields ("bid") :location 62} :rexpr {A_CONST :val 1 :location 68} :rexpr_list_start 0 :rexpr_list_end 0 :location 66} :fromClause <> :returningClause <> :withClause <>} :stmt_location 0 :stmt_len 69})"#,
    ),
    (
        r#"UPDATE pgbench_accounts SET abalance = abalance + $1 WHERE aid = $2;"#,
        r#"({RAWSTMT :stmt {UPDATESTMT :relation {RANGEVAR :catalogname <> :schemaname <> :relname pgbench_accounts :inh true :relpersistence p :alias <> :location 7} :targetList ({RESTARGET :name abalance :indirection <> :val {A_EXPR :name ("+") :lexpr {COLUMNREF :fields ("abalance") :location 39} :rexpr {PARAMREF :number 1 :location 50} :rexpr_list_start 0 :rexpr_list_end 0 :location 48} :location 28}) :whereClause {A_EXPR :name ("=") :lexpr {COLUMNREF :fields ("aid") :location 59} :rexpr {PARAMREF :number 2 :location 65} :rexpr_list_start 0 :rexpr_list_end 0 :location 63} :fromClause <> :returningClause <> :withClause <>} :stmt_location 0 :stmt_len 67})"#,
    ),
    (
        r#"UPDATE pgbench_tellers SET tbalance = tbalance + $1 WHERE tid = $2;"#,
        r#"({RAWSTMT :stmt {UPDATESTMT :relation {RANGEVAR :catalogname <> :schemaname <> :relname pgbench_tellers :inh true :relpersistence p :alias <> :location 7} :targetList ({RESTARGET :name tbalance :indirection <> :val {A_EXPR :name ("+") :lexpr {COLUMNREF :fields ("tbalance") :location 38} :rexpr {PARAMREF :number 1 :location 49} :rexpr_list_start 0 :rexpr_list_end 0 :location 47} :location 27}) :whereClause {A_EXPR :name ("=") :lexpr {COLUMNREF :fields ("tid") :location 58} :rexpr {PARAMREF :number 2 :location 64} :rexpr_list_start 0 :rexpr_list_end 0 :location 62} :fromClause <> :returningClause <> :withClause <>} :stmt_location 0 :stmt_len 66})"#,
    ),
    (
        r#"UPDATE pgbench_branches SET bbalance = bbalance + $1 WHERE bid = $2;"#,
        r#"({RAWSTMT :stmt {UPDATESTMT :relation {RANGEVAR :catalogname <> :schemaname <> :relname pgbench_branches :inh true :relpersistence p :alias <> :location 7} :targetList ({RESTARGET :name bbalance :indirection <> :val {A_EXPR :name ("+") :lexpr {COLUMNREF :fields ("bbalance") :location 39} :rexpr {PARAMREF :number 1 :location 50} :rexpr_list_start 0 :rexpr_list_end 0 :location 48} :location 28}) :whereClause {A_EXPR :name ("=") :lexpr {COLUMNREF :fields ("bid") :location 59} :rexpr {PARAMREF :number 2 :location 65} :rexpr_list_start 0 :rexpr_list_end 0 :location 63} :fromClause <> :returningClause <> :withClause <>} :stmt_location 0 :stmt_len 67})"#,
    ),
];

/// T-PG.6.4: each pgbench statement parses to the oracle's tree.
#[test]
fn t_pg_6_4_pgbench_statements_parse_to_19s_trees() {
    for (sql, oracle) in PGBENCH {
        let (tree, statements) =
            gram::parse(sql.as_bytes()).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
        assert_eq!(&tree.to_text(&statements).unwrap(), oracle, "{sql}");
    }
}
