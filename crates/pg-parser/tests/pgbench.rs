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
    (
        r#"SELECT abalance FROM pgbench_accounts WHERE aid = 34521;"#,
        r#"({RAWSTMT :stmt {SELECTSTMT :distinctClause <> :intoClause <> :targetList ({RESTARGET :name <> :indirection <> :val {COLUMNREF :fields ("abalance") :location 7} :location 7}) :fromClause ({RANGEVAR :catalogname <> :schemaname <> :relname pgbench_accounts :inh true :relpersistence p :alias <> :location 21}) :whereClause {A_EXPR :name ("=") :lexpr {COLUMNREF :fields ("aid") :location 44} :rexpr {A_CONST :val 34521 :location 50} :rexpr_list_start 0 :rexpr_list_end 0 :location 48} :groupClause <> :groupDistinct false :havingClause <> :windowClause <> :valuesLists <> :sortClause <> :limitOffset <> :limitCount <> :limitOption 0 :lockingClause <> :withClause <> :op 0 :all false :larg <> :rarg <>} :stmt_location 0 :stmt_len 55})"#,
    ),
    (
        r#"SELECT abalance FROM pgbench_accounts WHERE aid = $1;"#,
        r#"({RAWSTMT :stmt {SELECTSTMT :distinctClause <> :intoClause <> :targetList ({RESTARGET :name <> :indirection <> :val {COLUMNREF :fields ("abalance") :location 7} :location 7}) :fromClause ({RANGEVAR :catalogname <> :schemaname <> :relname pgbench_accounts :inh true :relpersistence p :alias <> :location 21}) :whereClause {A_EXPR :name ("=") :lexpr {COLUMNREF :fields ("aid") :location 44} :rexpr {PARAMREF :number 1 :location 50} :rexpr_list_start 0 :rexpr_list_end 0 :location 48} :groupClause <> :groupDistinct false :havingClause <> :windowClause <> :valuesLists <> :sortClause <> :limitOffset <> :limitCount <> :limitOption 0 :lockingClause <> :withClause <> :op 0 :all false :larg <> :rarg <>} :stmt_location 0 :stmt_len 52})"#,
    ),
    (
        r#"INSERT INTO pgbench_history (tid, bid, aid, delta, mtime) VALUES (7, 1, 34521, -2741, CURRENT_TIMESTAMP);"#,
        r#"({RAWSTMT :stmt {INSERTSTMT :relation {RANGEVAR :catalogname <> :schemaname <> :relname pgbench_history :inh true :relpersistence p :alias <> :location 12} :cols ({RESTARGET :name tid :indirection <> :val <> :location 29} {RESTARGET :name bid :indirection <> :val <> :location 34} {RESTARGET :name aid :indirection <> :val <> :location 39} {RESTARGET :name delta :indirection <> :val <> :location 44} {RESTARGET :name mtime :indirection <> :val <> :location 51}) :selectStmt {SELECTSTMT :distinctClause <> :intoClause <> :targetList <> :fromClause <> :whereClause <> :groupClause <> :groupDistinct false :havingClause <> :windowClause <> :valuesLists (({A_CONST :val 7 :location 66} {A_CONST :val 1 :location 69} {A_CONST :val 34521 :location 72} {A_CONST :val -2741 :location 79} {SQLVALUEFUNCTION :op 3 :type 0 :typmod -1 :location 86})) :sortClause <> :limitOffset <> :limitCount <> :limitOption 0 :lockingClause <> :withClause <> :op 0 :all false :larg <> :rarg <>} :onConflictClause <> :returningClause <> :withClause <> :override 0} :stmt_location 0 :stmt_len 104})"#,
    ),
    (
        r#"INSERT INTO pgbench_history (tid, bid, aid, delta, mtime) VALUES (7, 1, 34521, 2741, CURRENT_TIMESTAMP);"#,
        r#"({RAWSTMT :stmt {INSERTSTMT :relation {RANGEVAR :catalogname <> :schemaname <> :relname pgbench_history :inh true :relpersistence p :alias <> :location 12} :cols ({RESTARGET :name tid :indirection <> :val <> :location 29} {RESTARGET :name bid :indirection <> :val <> :location 34} {RESTARGET :name aid :indirection <> :val <> :location 39} {RESTARGET :name delta :indirection <> :val <> :location 44} {RESTARGET :name mtime :indirection <> :val <> :location 51}) :selectStmt {SELECTSTMT :distinctClause <> :intoClause <> :targetList <> :fromClause <> :whereClause <> :groupClause <> :groupDistinct false :havingClause <> :windowClause <> :valuesLists (({A_CONST :val 7 :location 66} {A_CONST :val 1 :location 69} {A_CONST :val 34521 :location 72} {A_CONST :val 2741 :location 79} {SQLVALUEFUNCTION :op 3 :type 0 :typmod -1 :location 85})) :sortClause <> :limitOffset <> :limitCount <> :limitOption 0 :lockingClause <> :withClause <> :op 0 :all false :larg <> :rarg <>} :onConflictClause <> :returningClause <> :withClause <> :override 0} :stmt_location 0 :stmt_len 103})"#,
    ),
    (
        r#"INSERT INTO pgbench_history (tid, bid, aid, delta, mtime) VALUES ($1, $2, $3, $4, CURRENT_TIMESTAMP);"#,
        r#"({RAWSTMT :stmt {INSERTSTMT :relation {RANGEVAR :catalogname <> :schemaname <> :relname pgbench_history :inh true :relpersistence p :alias <> :location 12} :cols ({RESTARGET :name tid :indirection <> :val <> :location 29} {RESTARGET :name bid :indirection <> :val <> :location 34} {RESTARGET :name aid :indirection <> :val <> :location 39} {RESTARGET :name delta :indirection <> :val <> :location 44} {RESTARGET :name mtime :indirection <> :val <> :location 51}) :selectStmt {SELECTSTMT :distinctClause <> :intoClause <> :targetList <> :fromClause <> :whereClause <> :groupClause <> :groupDistinct false :havingClause <> :windowClause <> :valuesLists (({PARAMREF :number 1 :location 66} {PARAMREF :number 2 :location 70} {PARAMREF :number 3 :location 74} {PARAMREF :number 4 :location 78} {SQLVALUEFUNCTION :op 3 :type 0 :typmod -1 :location 82})) :sortClause <> :limitOffset <> :limitCount <> :limitOption 0 :lockingClause <> :withClause <> :op 0 :all false :larg <> :rarg <>} :onConflictClause <> :returningClause <> :withClause <> :override 0} :stmt_location 0 :stmt_len 100})"#,
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
