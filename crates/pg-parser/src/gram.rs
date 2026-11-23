//! bison 3.8.2's LR parse loop (its yacc.c skeleton's `yyparse`) over gram.y's tables
//! (crate::gram_tables), fed by parser.c's `base_yylex` (crate::lex::Lexer): a query string is
//! accepted or refused where PostgreSQL 19's `raw_parser` accepts or refuses it, at the same cursor
//! (docs/design/postgres.md section 3; section 11's units 5 and 6). Beside the state stack it keeps
//! yacc.c's value and location stacks, and on each reduce runs the rule's action ported to Rust
//! ([`ported`]), which builds 19's raw parse tree (crate::tree). A statement that reduces a rule
//! whose action isn't ported yet is answered 0A000.

use crate::gram_tables::{
    ACTION_HASH, CHECK, DEFACT, DEFGOTO, FINAL, LAST, MAXUTOK, NTOKENS, PACT, PACT_DEFAULT, PGOTO,
    R1, R2, TABLE, TABLE_ERROR, TRANSLATE,
};
use crate::lex::{self, Lexer, ScanError, cursor};
use crate::tree::{FuncCall, Node, NodeId, SelectStmt, TransactionStmtKind, Tree, TreeFault, Val};

/// yacc.c's YYMAXDEPTH, 10000 when the grammar doesn't define it (gram.y doesn't): a parse whose
/// stack would hold that many states is refused "memory exhausted", as 19 refuses it.
const MAX_DEPTH: usize = 10_000;

/// yacc.c's YYSYMBOL_YYEOF and YYSYMBOL_YYUNDEF.
const END: i32 = 0;
const UNDEFINED: i32 = 2;

const SYNTAX_ERROR: &str = "42601";
const FEATURE_NOT_SUPPORTED: &str = "0A000";

/// 19's refusal: its SQLSTATE, the ErrorResponse cursor (1-based characters, 0 for none) and message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub sqlstate: &'static str,
    pub cursor: usize,
    pub message: &'static str,
}

impl From<ScanError> for Refusal {
    fn from(e: ScanError) -> Self {
        Self {
            sqlstate: e.sqlstate,
            cursor: e.cursor,
            message: e.message,
        }
    }
}

impl From<TreeFault> for Refusal {
    fn from(_: TreeFault) -> Self {
        fault()
    }
}

fn fault() -> Refusal {
    Refusal {
        sqlstate: "XX000",
        cursor: 0,
        message: "parser tables fault",
    }
}

/// A name that isn't UTF-8: 19 refuses such bytes when the query arrives (pg_verify_mbstr).
fn not_utf8() -> Refusal {
    Refusal {
        sqlstate: "22021",
        cursor: 0,
        message: "invalid byte sequence for encoding \"UTF8\"",
    }
}

/// A table's entry as an `i32`; an index outside it is a generator fault, never an input's.
fn entry<T: Copy + Into<i32>>(table: &[T], i: i32) -> Result<i32, Refusal> {
    usize::try_from(i)
        .ok()
        .and_then(|i| table.get(i))
        .map(|&v| v.into())
        .ok_or_else(fault)
}

/// yacc.c's reading of `yychar`: any token number at or below 0 is the end of input, then YYTRANSLATE.
fn symbol(token: i32) -> Result<i32, Refusal> {
    match token {
        ..=0 => Ok(END),
        t if t <= MAXUTOK => entry(&TRANSLATE, t),
        _ => Ok(UNDEFINED),
    }
}

/// scanner_yyerror at the last token read (yylloc): 19 says "at end of input" there, else
/// "at or near" the token.
fn syntax_error(sql: &[u8], at: usize, message: &'static str) -> Refusal {
    Refusal {
        sqlstate: SYNTAX_ERROR,
        cursor: cursor(sql, at),
        message,
    }
}

/// One step of yacc.c's `yybackup`: shift to a state, reduce by a rule, or a syntax error.
enum Step {
    Shift(i32),
    Reduce(i32),
    Error,
}

/// A semantic value on yacc.c's value stack (gram.y's `%union`, as far as ported actions use it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// A token's value from the scanner, or an empty rule's unset `$$`.
    Token(lex::Value),
    /// `Node *`, NULL as `None`.
    Node(Option<NodeId>),
    /// `List *`, NIL as empty.
    List(Vec<NodeId>),
    Bool(bool),
    /// An `int` an action made (`ival`).
    Int(i32),
    /// func_alias_clause's `list_make2(alias, coldeflist)`.
    FuncAlias {
        alias: Option<NodeId>,
        coldeflist: Vec<NodeId>,
    },
    /// gram.y's GroupClause: GROUP BY's list and whether it said DISTINCT.
    Group {
        distinct: bool,
        list: Vec<NodeId>,
    },
    /// A value an unported action would have made: the statement is answered 0A000.
    Unported,
}

/// One reduce: the rule's right-hand side values ($1 at index 0) and locations (@1), the arena, and
/// raw_parser's answer (`pg_yyget_extra(yyscanner)->parsetree`).
struct Reduce<'a> {
    rhs: Vec<Value>,
    at: Vec<i32>,
    tree: &'a mut Tree,
    parsetree: &'a mut Option<Vec<NodeId>>,
}

impl Reduce<'_> {
    /// `$n`, taken (each action reads each value once).
    fn take(&mut self, n: usize) -> Value {
        n.checked_sub(1)
            .and_then(|i| self.rhs.get_mut(i))
            .map_or(Value::Token(lex::Value::None), |v| {
                std::mem::replace(v, Value::Token(lex::Value::None))
            })
    }

    /// `@n`.
    fn at(&self, n: usize) -> Result<i32, Refusal> {
        n.checked_sub(1)
            .and_then(|i| self.at.get(i))
            .copied()
            .ok_or_else(fault)
    }

    fn node(&mut self, n: usize) -> Result<Option<NodeId>, Refusal> {
        match self.take(n) {
            Value::Node(node) => Ok(node),
            _ => Err(fault()),
        }
    }

    fn list(&mut self, n: usize) -> Result<Vec<NodeId>, Refusal> {
        match self.take(n) {
            Value::List(list) => Ok(list),
            _ => Err(fault()),
        }
    }

    fn boolean(&mut self, n: usize) -> Result<bool, Refusal> {
        match self.take(n) {
            Value::Bool(b) => Ok(b),
            _ => Err(fault()),
        }
    }

    fn group(&mut self, n: usize) -> Result<(bool, Vec<NodeId>), Refusal> {
        match self.take(n) {
            Value::Group { distinct, list } => Ok((distinct, list)),
            _ => Err(fault()),
        }
    }

    /// `char *` from a token: an identifier's or literal's bytes, or a keyword's name.
    fn string(&mut self, n: usize) -> Result<String, Refusal> {
        match self.take(n) {
            Value::Token(lex::Value::Str(bytes)) => {
                String::from_utf8(bytes).map_err(|_| not_utf8())
            }
            Value::Token(lex::Value::Keyword(name)) => Ok(name.to_owned()),
            _ => Err(fault()),
        }
    }

    /// `int` from a token (ICONST, PARAM) or an action.
    fn int(&mut self, n: usize) -> Result<i32, Refusal> {
        match self.take(n) {
            Value::Token(lex::Value::Int(v)) | Value::Int(v) => Ok(v),
            _ => Err(fault()),
        }
    }

    fn push(&mut self, node: Node) -> Result<NodeId, Refusal> {
        Ok(self.tree.push(node)?)
    }

    fn make(&mut self, node: Node) -> Result<Value, Refusal> {
        Ok(Value::Node(Some(self.push(node)?)))
    }
}

/// A ported action: what the rule's C action computes as `$$`.
type Action = fn(&mut Reduce<'_>) -> Result<Value, Refusal>;

/// gram.y's actions ported so far, each beside the hash of the C text it was ported from
/// (gram_tables::ACTION_HASH, scripts/postgres-grammar.py). A rule whose hash moved on since is
/// unported again, so a regenerated grammar never runs a stale port (section 3).
fn ported(rule: i32) -> Option<(u64, Action)> {
    Some(match rule {
        // parse_toplevel: stmtmulti
        2 => (0x6fcc_5b58_98c0_15d4, parse_toplevel),
        // stmtmulti: stmtmulti ';' toplevel_stmt
        8 => (0xd79a_009f_fce0_46ba, stmtmulti_next),
        // stmtmulti: toplevel_stmt
        9 => (0x0595_800c_6e6f_2b66, stmtmulti_first),
        // stmt: /*EMPTY*/
        137 => (0xc962_de43_8599_45bc, |_| Ok(Value::Node(None))),
        // TransactionStmtLegacy: BEGIN_P opt_transaction transaction_mode_list_or_empty
        1501 => (0x59de_e058_603f_30b7, begin),
        // TransactionStmtLegacy: END_P opt_transaction opt_transaction_chain
        1502 => (0x88ca_638c_a2a9_d9bb, end),
        // transaction_mode_list_or_empty: /* EMPTY */
        1515 => (0x2cdf_e4df_9842_f348, |_| Ok(Value::List(Vec::new()))),
        // opt_transaction_chain: AND CHAIN
        1516 => (0xcafd_31ef_0f34_4799, |_| Ok(Value::Bool(true))),
        // opt_transaction_chain: AND NO CHAIN | /* EMPTY */
        1517 | 1518 => (0xc7dd_6751_cbba_2e22, |_| Ok(Value::Bool(false))),
        // InsertStmt: opt_with_clause INSERT INTO insert_target insert_rest
        1642 => (0x2105_41e7_4a78_96e0, insert_stmt),
        // insert_target: qualified_name
        1643 => (0x85ed_2683_fbb2_8bba, top),
        // insert_rest: '(' insert_column_list ')' SelectStmt
        1647 => (0xdb9b_ea3e_cc7b_6905, insert_rest),
        // insert_column_list: insert_column_item
        1652 => (0x971e_4de7_6334_f2bf, list1),
        // insert_column_list: insert_column_list ',' insert_column_item
        1653 => (0x4811_b036_e1ca_20fe, append),
        // insert_column_item: ColId opt_indirection
        1654 => (0x35fc_0e09_2286_8396, column_target),
        // opt_on_conflict: /* EMPTY */
        1658 => (0x0f49_8f67_e462_b676, null),
        // returning_clause: /* EMPTY */
        1663 => (0x90e3_6cac_3351_2d39, null),
        // UpdateStmt: opt_with_clause UPDATE relation_expr_opt_alias
        1690 => (0x3f00_fe1f_c4fc_070d, update_stmt),
        // set_clause_list: set_clause
        1691 => (0x1509_2582_214f_28d3, top),
        // set_clause: set_target '=' a_expr
        1693 => (0x4bef_6d70_91a3_86ad, set_clause),
        // set_target: ColId opt_indirection
        1695 => (0x5914_3a79_afda_e407, column_target),
        // select_no_parens: simple_select
        1735 => (0xfd2f_9ec2_19c8_d787, top),
        // simple_select: SELECT opt_all_clause opt_target_list
        1745 => (0x5a6d_b6e1_b8fa_52e2, simple_select),
        // simple_select: values_clause
        1747 => (0x9113_5026_c13a_e4a7, top),
        // opt_with_clause: /* EMPTY */
        1768 => (0xf8ee_ad8a_b545_1b54, null),
        // into_clause: /* EMPTY */
        1770 => (0xb6f0_5cf7_ff18_9c86, null),
        // opt_sort_clause: /* EMPTY */
        1792 => (0x1e2a_fd0a_a524_9ce8, nil),
        // group_clause: /* EMPTY */
        1825 => (0x0321_6edd_97cb_a203, no_group),
        // having_clause: /* EMPTY */
        1838 => (0x2ef6_8e2d_5a42_4abc, null),
        // values_clause: VALUES '(' expr_list ')'
        1854 => (0x3037_617e_9420_4e91, values_clause),
        // from_clause: FROM from_list
        1856 => (0x1509_2582_214f_28d3, top),
        // from_clause: /* EMPTY */
        1857 => (0x1e2a_fd0a_a524_9ce8, nil),
        // from_list: table_ref
        1858 => (0x41fd_9d1b_bdab_ed5c, list1),
        // table_ref: relation_expr opt_alias_clause
        1860 => (0x35dd_dca3_624f_7135, aliased_relation),
        // table_ref: func_table func_alias_clause
        1862 => (0x86e4_091c_e124_8a20, function_table),
        // alias_clause: AS ColId
        1879 => (0x59f2_2c8a_f16a_a2ef, alias),
        // opt_alias_clause: /* EMPTY */
        1883 => (0xd843_f1a2_034c_c3a2, null),
        // func_alias_clause: alias_clause
        1886 => (0x5fd2_8df3_dcf9_cac1, func_alias),
        // relation_expr: qualified_name
        1899 => (0x0f6f_0ebf_dca7_954d, relation_expr),
        // relation_expr_opt_alias: relation_expr
        1906 => (0x85ed_2683_fbb2_8bba, top),
        // func_table: func_expr_windowless opt_ordinality
        1912 => (0x4eed_841e_01ab_f3fb, func_table),
        // opt_ordinality: /* EMPTY */
        1920 => (0xf9a7_c703_e0b2_bd02, |_| Ok(Value::Bool(false))),
        // where_clause: WHERE a_expr
        1921 => (0x9113_5026_c13a_e4a7, top),
        // where_clause: /* EMPTY */
        1922 => (0x2ef6_8e2d_5a42_4abc, null),
        // where_or_current_clause: WHERE a_expr
        1923 => (0x9113_5026_c13a_e4a7, top),
        // a_expr: c_expr
        2048 => (0x9113_5026_c13a_e4a7, top),
        // a_expr: '-' a_expr
        2054 => (0x50eb_9947_0f67_cd77, negate),
        // a_expr: a_expr '+' a_expr
        2055 => (0x34ef_b0cc_0323_8888, |r| a_expr_op(r, "+")),
        // a_expr: a_expr '-' a_expr
        2056 => (0x94b8_110d_a054_53de, |r| a_expr_op(r, "-")),
        // a_expr: a_expr '/' a_expr
        2058 => (0x25dc_16de_5219_d95c, |r| a_expr_op(r, "/")),
        // a_expr: a_expr '=' a_expr
        2063 => (0x8a20_32d0_a75b_2e2e, |r| a_expr_op(r, "=")),
        // c_expr: columnref
        2140 => (0x9113_5026_c13a_e4a7, top),
        // c_expr: AexprConst
        2141 => (0x9113_5026_c13a_e4a7, top),
        // c_expr: PARAM opt_indirection
        2142 => (0x251d_c253_e0dc_bc54, param),
        // c_expr: '(' a_expr ')' opt_indirection
        2143 => (0x9592_5b42_d849_02b0, parenthesized),
        // c_expr: func_expr
        2145 => (0xcc48_388e_3a70_ab47, top),
        // func_application: func_name '(' func_arg_list opt_sort_clause ')'
        2155 => (0x568c_7c1f_de3b_fc0f, func_call),
        // func_application: func_name '(' '*' ')'
        2160 => (0xcf2d_9dd1_bad0_1139, func_star),
        // func_expr: func_application within_group_clause filter_clause null_treatment over_clause
        2161 => (0x7591_e707_3cfc_29c2, func_expr),
        // func_expr: func_expr_common_subexpr
        2163 => (0xcc48_388e_3a70_ab47, top),
        // func_expr_windowless: func_application
        2164 => (0x9113_5026_c13a_e4a7, top),
        // func_expr_common_subexpr: CURRENT_TIMESTAMP
        2171 => (0x0be0_fa0f_61cc_cb63, current_timestamp),
        // within_group_clause: /* EMPTY */
        2256 => (0x1e2a_fd0a_a524_9ce8, nil),
        // filter_clause: /* EMPTY */
        2258 => (0x2ef6_8e2d_5a42_4abc, null),
        // null_treatment: /* EMPTY */
        2261 => (0xedb2_3297_52e7_f668, |_| Ok(Value::Int(0))),
        // window_clause: /* EMPTY */
        2263 => (0x1e2a_fd0a_a524_9ce8, nil),
        // over_clause: /* EMPTY */
        2269 => (0x7ae2_46f7_a59d_d753, null),
        // expr_list: a_expr
        2324 => (0x57a1_7057_3bfa_0539, list1),
        // expr_list: expr_list ',' a_expr
        2325 => (0x3f88_1f70_44bd_239a, append),
        // func_arg_list: func_arg_expr
        2326 => (0x57a1_7057_3bfa_0539, list1),
        // func_arg_list: func_arg_list ',' func_arg_expr
        2327 => (0x3f88_1f70_44bd_239a, append),
        // func_arg_expr: a_expr
        2328 => (0x0daa_dfab_dabf_50ca, top),
        // columnref: ColId
        2372 => (0xe7f0_4492_1ba8_ebf2, columnref),
        // opt_indirection: /* EMPTY */
        2382 => (0x1e2a_fd0a_a524_9ce8, nil),
        // opt_target_list: target_list
        2454 => (0x8af9_73ea_c57a_9113, top),
        // target_list: target_el
        2456 => (0x5aa0_8862_5693_2d5f, list1),
        // target_list: target_list ',' target_el
        2457 => (0xc5c0_8990_2dd2_681e, append),
        // target_el: a_expr
        2460 => (0xfe9d_afba_fed1_0593, target_el),
        // qualified_name: ColId
        2464 => (0x74ff_14c1_e4b9_74b6, qualified_name),
        // func_name: type_function_name
        2471 => (0x2eeb_eadd_6516_86e3, func_name),
        // AexprConst: Iconst
        2473 => (0xedd7_3fba_5686_550f, int_const),
        // AexprConst: FCONST
        2474 => (0x38e4_cd19_2237_3ce1, float_const),
        // AexprConst: Sconst
        2475 => (0xeb6d_7390_faf1_1cfe, string_const),
        // Iconst: ICONST
        2486 => (0x35a8_92e8_20e5_a06b, top),
        // Sconst: SCONST
        2487 => (0xa007_892c_3e21_a49b, top),
        // ColId: IDENT
        2504 => (0xa007_892c_3e21_a49b, top),
        // type_function_name: IDENT
        2507 => (0x2c4d_653c_2123_81fb, top),
        _ => return None,
    })
}

fn parse_toplevel(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let list = r.list(1)?;
    *r.parsetree = Some(list.clone());
    Ok(Value::List(list))
}

/// gram.y's makeRawStmt: `stmt_len` 0 until updateRawStmtEnd.
fn raw_stmt(tree: &mut Tree, stmt: NodeId, stmt_location: i32) -> Result<NodeId, Refusal> {
    Ok(tree.push(Node::RawStmt {
        stmt: Some(stmt),
        stmt_location,
        stmt_len: 0,
    })?)
}

fn stmtmulti_next(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let mut list = r.list(1)?;
    if let Some(&last) = list.last() {
        // updateRawStmtEnd: a length already set stays ("select foo ;; select bar").
        let end = r.at(2)?;
        if let Node::RawStmt {
            stmt_location,
            stmt_len,
            ..
        } = r.tree.get_mut(last)?
            && *stmt_len <= 0
        {
            *stmt_len = end.checked_sub(*stmt_location).ok_or_else(fault)?;
        }
    }
    if let Some(stmt) = r.node(3)? {
        let at = r.at(3)?;
        list.push(raw_stmt(r.tree, stmt, at)?);
    }
    Ok(Value::List(list))
}

fn stmtmulti_first(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    Ok(Value::List(match r.node(1)? {
        Some(stmt) => {
            let at = r.at(1)?;
            vec![raw_stmt(r.tree, stmt, at)?]
        }
        None => Vec::new(),
    }))
}

fn transaction(
    r: &mut Reduce<'_>,
    kind: TransactionStmtKind,
    options: Vec<NodeId>,
    chain: bool,
) -> Result<Value, Refusal> {
    Ok(Value::Node(Some(r.tree.push(Node::TransactionStmt {
        kind,
        options,
        savepoint_name: None,
        gid: None,
        chain,
        location: -1,
    })?)))
}

fn begin(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let options = r.list(3)?;
    transaction(r, TransactionStmtKind::Begin, options, false)
}

fn end(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let chain = r.boolean(3)?;
    transaction(r, TransactionStmtKind::Commit, Vec::new(), chain)
}

/// `$$ = $n` of the rule's last symbol: gram.c reads it as `yyvsp[0]` whatever `n` is, so every
/// rule with that C text shares one hash.
fn top(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    Ok(r.rhs.pop().unwrap_or(Value::Token(lex::Value::None)))
}

fn null(_: &mut Reduce<'_>) -> Result<Value, Refusal> {
    Ok(Value::Node(None))
}

fn nil(_: &mut Reduce<'_>) -> Result<Value, Refusal> {
    Ok(Value::List(Vec::new()))
}

fn update_stmt(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let with_clause = r.node(1)?;
    let relation = r.node(3)?;
    let target_list = r.list(5)?;
    let from_clause = r.list(6)?;
    let where_clause = r.node(7)?;
    let returning_clause = r.node(8)?;
    r.make(Node::UpdateStmt {
        relation,
        target_list,
        where_clause,
        from_clause,
        returning_clause,
        with_clause,
    })
}

fn set_clause(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let target = r.node(1)?.ok_or_else(fault)?;
    let value = r.node(3)?;
    let Node::ResTarget { val, .. } = r.tree.get_mut(target)? else {
        return Err(fault());
    };
    *val = value;
    Ok(Value::List(vec![target]))
}

/// set_target and insert_column_item: a ResTarget naming a column. check_indirection's refusal
/// needs an `A_Star`, which only indirection_el's unported rules make.
fn column_target(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let name = r.string(1)?;
    let indirection = r.list(2)?;
    let location = r.at(1)?;
    r.make(Node::ResTarget {
        name: Some(name),
        indirection,
        val: None,
        location,
    })
}

fn relation_expr(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let relation = r.node(1)?.ok_or_else(fault)?;
    let Node::RangeVar { inh, alias, .. } = r.tree.get_mut(relation)? else {
        return Err(fault());
    };
    *inh = true;
    *alias = None;
    Ok(Value::Node(Some(relation)))
}

/// makeRangeVar.
fn range_var(schemaname: Option<String>, relname: String, location: i32) -> Node {
    Node::RangeVar {
        catalogname: None,
        schemaname,
        relname,
        inh: true,
        relpersistence: 'p',
        alias: None,
        location,
    }
}

fn qualified_name(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let relname = r.string(1)?;
    let location = r.at(1)?;
    r.make(range_var(None, relname, location))
}

/// makeSimpleA_Expr(AEXPR_OP, name, lexpr, rexpr, location).
fn simple_a_expr(
    r: &mut Reduce<'_>,
    name: &str,
    lexpr: Option<NodeId>,
    rexpr: Option<NodeId>,
    location: i32,
) -> Result<Value, Refusal> {
    let name = r.push(Node::String(name.to_owned()))?;
    r.make(Node::AExprOp {
        name: vec![name],
        lexpr,
        rexpr,
        location,
    })
}

/// `a_expr <op> a_expr`: makeSimpleA_Expr at the operator.
fn a_expr_op(r: &mut Reduce<'_>, name: &str) -> Result<Value, Refusal> {
    let lexpr = r.node(1)?;
    let rexpr = r.node(3)?;
    let location = r.at(2)?;
    simple_a_expr(r, name, lexpr, rexpr, location)
}

/// doNegate: a numeric constant takes the minus and the '-' sign's location; any other constant
/// still takes the location, then is negated by a '-' operator, as anything else is.
fn negate(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let n = r.node(2)?.ok_or_else(fault)?;
    let location = r.at(1)?;
    if let Node::AConst { val, location: at } = r.tree.get_mut(n)? {
        *at = location;
        match val {
            Val::Integer(v) => {
                *v = v.checked_neg().ok_or_else(fault)?;
                return Ok(Value::Node(Some(n)));
            }
            // doNegateFloat: past a leading '+', a '-' is stripped, else one is added.
            Val::Float(text) => {
                let digits = text.strip_prefix('+').unwrap_or(text);
                let negated = match digits.strip_prefix('-') {
                    Some(rest) => rest.to_owned(),
                    None => format!("-{digits}"),
                };
                *text = negated;
                return Ok(Value::Node(Some(n)));
            }
            Val::String(_) => {}
        }
    }
    simple_a_expr(r, "-", None, Some(n), location)
}

/// `PARAM opt_indirection`: a ParamRef; one with indirection takes A_Indirection, not ported.
fn param(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let number = r.int(1)?;
    if !r.list(2)?.is_empty() {
        return Ok(Value::Unported);
    }
    let location = r.at(1)?;
    r.make(Node::ParamRef { number, location })
}

/// makeColumnRef(colname, NIL, location).
fn columnref(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let name = r.string(1)?;
    let location = r.at(1)?;
    let field = r.push(Node::String(name))?;
    r.make(Node::ColumnRef {
        fields: vec![field],
        location,
    })
}

fn simple_select(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let target_list = r.list(3)?;
    let into_clause = r.node(4)?;
    let from_clause = r.list(5)?;
    let where_clause = r.node(6)?;
    let (group_distinct, group_clause) = r.group(7)?;
    let having_clause = r.node(8)?;
    let window_clause = r.list(9)?;
    r.make(Node::SelectStmt(Box::new(SelectStmt {
        target_list,
        into_clause,
        from_clause,
        where_clause,
        group_clause,
        group_distinct,
        having_clause,
        window_clause,
        ..SelectStmt::default()
    })))
}

fn no_group(_: &mut Reduce<'_>) -> Result<Value, Refusal> {
    Ok(Value::Group {
        distinct: false,
        list: Vec::new(),
    })
}

/// `VALUES '(' expr_list ')'`: the row as a List node, the one element of valuesLists.
fn values_clause(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let row = r.list(3)?;
    let row = r.push(Node::List(row.into_iter().map(Some).collect()))?;
    r.make(Node::SelectStmt(Box::new(SelectStmt {
        values_lists: vec![row],
        ..SelectStmt::default()
    })))
}

/// `list_make1($1)`.
fn list1(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    Ok(Value::List(vec![r.node(1)?.ok_or_else(fault)?]))
}

/// `lappend($1, $3)`.
fn append(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let mut list = r.list(1)?;
    list.push(r.node(3)?.ok_or_else(fault)?);
    Ok(Value::List(list))
}

/// `table_ref: relation_expr opt_alias_clause`.
fn aliased_relation(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let relation = r.node(1)?.ok_or_else(fault)?;
    let to = r.node(2)?;
    let Node::RangeVar { alias, .. } = r.tree.get_mut(relation)? else {
        return Err(fault());
    };
    *alias = to;
    Ok(Value::Node(Some(relation)))
}

/// `target_el: a_expr`: a ResTarget with no name.
fn target_el(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let val = r.node(1)?;
    let location = r.at(1)?;
    r.make(Node::ResTarget {
        name: None,
        indirection: Vec::new(),
        val,
        location,
    })
}

/// `insert_rest: '(' insert_column_list ')' SelectStmt`.
fn insert_rest(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let cols = r.list(2)?;
    let select_stmt = r.node(4)?;
    r.make(Node::InsertStmt {
        relation: None,
        cols,
        select_stmt,
        on_conflict_clause: None,
        returning_clause: None,
        with_clause: None,
        override_kind: 0,
    })
}

fn insert_stmt(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let with = r.node(1)?;
    let target = r.node(4)?;
    let stmt = r.node(5)?.ok_or_else(fault)?;
    let on_conflict = r.node(6)?;
    let returning = r.node(7)?;
    let Node::InsertStmt {
        relation,
        on_conflict_clause,
        returning_clause,
        with_clause,
        ..
    } = r.tree.get_mut(stmt)?
    else {
        return Err(fault());
    };
    *relation = target;
    *on_conflict_clause = on_conflict;
    *returning_clause = returning;
    *with_clause = with;
    Ok(Value::Node(Some(stmt)))
}

/// makeSQLValueFunction(SVFOP_CURRENT_TIMESTAMP, -1, @1); primnodes.h numbers that op 3.
fn current_timestamp(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let location = r.at(1)?;
    r.make(Node::SqlValueFunction {
        op: 3,
        typmod: -1,
        location,
    })
}

/// `table_ref: func_table func_alias_clause`: the alias and column definitions go on the
/// RangeFunction.
fn function_table(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let function = r.node(1)?.ok_or_else(fault)?;
    let Value::FuncAlias {
        alias: to_alias,
        coldeflist: to_coldeflist,
    } = r.take(2)
    else {
        return Err(fault());
    };
    let Node::RangeFunction {
        alias, coldeflist, ..
    } = r.tree.get_mut(function)?
    else {
        return Err(fault());
    };
    *alias = to_alias;
    *coldeflist = to_coldeflist;
    Ok(Value::Node(Some(function)))
}

/// `func_table: func_expr_windowless opt_ordinality`: one function, as `list_make2($1, NIL)`.
fn func_table(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let call = r.node(1)?;
    let ordinality = r.boolean(2)?;
    let pair = r.push(Node::List(vec![call, None]))?;
    r.make(Node::RangeFunction {
        lateral: false,
        ordinality,
        is_rowsfrom: false,
        functions: vec![pair],
        alias: None,
        coldeflist: Vec::new(),
    })
}

/// `func_application: func_name '(' func_arg_list opt_sort_clause ')'`.
fn func_call(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let funcname = r.list(1)?;
    let args = r.list(3)?;
    let agg_order = r.list(4)?;
    let location = r.at(1)?;
    r.make(Node::FuncCall(Box::new(FuncCall {
        funcname,
        args,
        agg_order,
        location,
        ..FuncCall::default()
    })))
}

/// `func_application: func_name '(' '*' ')'`: no arguments, marked agg_star.
fn func_star(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let funcname = r.list(1)?;
    let location = r.at(1)?;
    r.make(Node::FuncCall(Box::new(FuncCall {
        funcname,
        agg_star: true,
        location,
        ..FuncCall::default()
    })))
}

/// `func_expr: func_application within_group_clause filter_clause null_treatment over_clause`.
/// WITHIN GROUP, with the refusals that check it, isn't ported.
fn func_expr(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let call = r.node(1)?.ok_or_else(fault)?;
    if !r.list(2)?.is_empty() {
        return Ok(Value::Unported);
    }
    let filter = r.node(3)?;
    let nulls = r.int(4)?;
    let window = r.node(5)?;
    let Node::FuncCall(n) = r.tree.get_mut(call)? else {
        return Err(fault());
    };
    n.agg_filter = filter;
    n.ignore_nulls = nulls;
    n.over = window;
    Ok(Value::Node(Some(call)))
}

/// `func_name: type_function_name`: `list_make1(makeString($1))`.
fn func_name(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let name = r.string(1)?;
    Ok(Value::List(vec![r.push(Node::String(name))?]))
}

/// `func_alias_clause: alias_clause`: `list_make2($1, NIL)`.
fn func_alias(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    Ok(Value::FuncAlias {
        alias: r.node(1)?,
        coldeflist: Vec::new(),
    })
}

/// `alias_clause: AS ColId`.
fn alias(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let aliasname = r.string(2)?;
    r.make(Node::Alias {
        aliasname,
        colnames: Vec::new(),
    })
}

/// `c_expr: '(' a_expr ')' opt_indirection`: the expression; with indirection, A_Indirection, not
/// ported.
fn parenthesized(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let expr = r.take(2);
    if !r.list(4)?.is_empty() {
        return Ok(Value::Unported);
    }
    Ok(expr)
}

/// makeStringConst.
fn string_const(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let s = r.string(1)?;
    let location = r.at(1)?;
    r.make(Node::AConst {
        val: Val::String(s),
        location,
    })
}

/// makeFloatConst: the literal's text.
fn float_const(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let s = r.string(1)?;
    let location = r.at(1)?;
    r.make(Node::AConst {
        val: Val::Float(s),
        location,
    })
}

/// makeIntConst.
fn int_const(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let v = r.int(1)?;
    let location = r.at(1)?;
    r.make(Node::AConst {
        val: Val::Integer(v),
        location,
    })
}

/// The rule's `$$`: bison's default (`$$ = $1`, before any action) for a rule with no action, the
/// ported action's otherwise, and [`Value::Unported`] when the action isn't ported (or was ported
/// from a text that has since changed) or a value it reads came from such a rule.
fn act(rule: i32, mut r: Reduce<'_>) -> Result<Value, Refusal> {
    if r.rhs.contains(&Value::Unported) {
        return Ok(Value::Unported);
    }
    let Some(hash) = usize::try_from(rule)
        .ok()
        .and_then(|i| ACTION_HASH.get(i))
        .ok_or_else(fault)?
    else {
        return Ok(r.take(1));
    };
    match ported(rule) {
        Some((from, action)) if from == *hash => action(&mut r),
        _ => Ok(Value::Unported),
    }
}

/// What 19's grammar made of a query string it accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// raw_parser's List of RawStmt, in its arena.
    Tree { tree: Tree, statements: Vec<NodeId> },
    /// A rule's action that this parse needed isn't ported yet.
    Unported,
}

/// yacc.c's YYLLOC_DEFAULT as gram.y defines it: the first right-hand side location that isn't -1,
/// else -1 (an empty rule's).
fn location(at: &[i32]) -> i32 {
    at.iter().copied().find(|&l| l >= 0).unwrap_or(-1)
}

/// 19's grammar on a query string (`parse_toplevel` as `raw_parser`'s RAW_PARSE_DEFAULT runs it):
/// its tree when it accepts, else 19's refusal from its scanner or its grammar.
pub fn parse_tree(sql: &[u8]) -> Result<Parsed, Refusal> {
    let mut lexer = Lexer::new(sql);
    let mut stack: Vec<i32> = Vec::new();
    let mut values: Vec<Value> = vec![Value::Token(lex::Value::None)];
    let mut locations: Vec<i32> = vec![0];
    let mut tree = Tree::default();
    let mut parsetree: Option<Vec<NodeId>> = None;
    let mut state = 0;
    let mut lookahead: Option<(i32, lex::Value)> = None;
    let mut at = 0usize;
    loop {
        stack.push(state);
        if stack.len() >= MAX_DEPTH {
            return Err(syntax_error(sql, at, "memory exhausted"));
        }
        if state == FINAL {
            return Ok(match parsetree {
                Some(statements) if !values.contains(&Value::Unported) => {
                    Parsed::Tree { tree, statements }
                }
                _ => Parsed::Unported,
            });
        }
        let pact = entry(&PACT, state)?;
        let mut step = None;
        if PACT_DEFAULT != Some(pact) {
            let token = match &lookahead {
                Some((t, _)) => *t,
                None => {
                    let (t, value, offset) = match lexer.next_token()? {
                        Some(t) => (symbol(t.token)?, t.value, t.offset),
                        None => (END, lex::Value::None, sql.len()),
                    };
                    at = offset;
                    lookahead = Some((t, value));
                    t
                }
            };
            let i = pact.checked_add(token).ok_or_else(fault)?;
            if (0..=LAST).contains(&i) && entry(&CHECK, i)? == token {
                let action = entry(&TABLE, i)?;
                step = Some(if action > 0 {
                    Step::Shift(action)
                } else if TABLE_ERROR == Some(action) {
                    Step::Error
                } else {
                    Step::Reduce(action.checked_neg().ok_or_else(fault)?)
                });
            }
        }
        let rule = match step {
            Some(Step::Shift(next)) => {
                state = next;
                let (_, value) = lookahead.take().ok_or_else(fault)?;
                values.push(Value::Token(value));
                locations.push(i32::try_from(at).map_err(|_| fault())?);
                continue;
            }
            Some(Step::Reduce(rule)) => rule,
            Some(Step::Error) => return Err(error_at(sql, at, lookahead.map(|(t, _)| t))),
            None => match entry(&DEFACT, state)? {
                0 => return Err(error_at(sql, at, lookahead.map(|(t, _)| t))),
                rule => rule,
            },
        };
        let len = usize::try_from(entry(&R2, rule)?).map_err(|_| fault())?;
        let base = stack.len().checked_sub(len).ok_or_else(fault)?;
        stack.truncate(base);
        let rhs = values.split_off(values.len().checked_sub(len).ok_or_else(fault)?);
        let rhs_at = locations.split_off(locations.len().checked_sub(len).ok_or_else(fault)?);
        let loc = location(&rhs_at);
        let value = act(
            rule,
            Reduce {
                rhs,
                at: rhs_at,
                tree: &mut tree,
                parsetree: &mut parsetree,
            },
        )?;
        values.push(value);
        locations.push(loc);
        let top = stack.last().copied().ok_or_else(fault)?;
        let lhs = entry(&R1, rule)?.checked_sub(NTOKENS).ok_or_else(fault)?;
        let i = entry(&PGOTO, lhs)?.checked_add(top).ok_or_else(fault)?;
        state = if (0..=LAST).contains(&i) && entry(&CHECK, i)? == top {
            entry(&TABLE, i)?
        } else {
            entry(&DEFGOTO, lhs)?
        };
    }
}

/// 19's grammar on a query string: `Ok` when it accepts, else 19's refusal.
pub fn accepts(sql: &[u8]) -> Result<(), Refusal> {
    parse_tree(sql).map(|_| ())
}

fn error_at(sql: &[u8], at: usize, lookahead: Option<i32>) -> Refusal {
    if lookahead == Some(END) {
        syntax_error(sql, at, "syntax error at end of input")
    } else {
        syntax_error(sql, at, "syntax error at or near a token")
    }
}

/// A query string's answer: 19's tree, 19's refusal where 19 refuses, and 0A000 for a string 19's
/// grammar accepts through a rule whose action isn't ported yet.
pub fn parse(sql: &[u8]) -> Result<(Tree, Vec<NodeId>), Refusal> {
    match parse_tree(sql)? {
        Parsed::Tree { tree, statements } => Ok((tree, statements)),
        Parsed::Unported => Err(Refusal {
            sqlstate: FEATURE_NOT_SUPPORTED,
            cursor: 0,
            message: "a rule this statement needs has no ported action yet",
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{ACTION_HASH, accepts, parse, ported};
    use crate::gram_tables::NRULES;

    fn refused(sql: &str) -> (&'static str, usize) {
        let e = accepts(sql.as_bytes()).unwrap_err();
        (e.sqlstate, e.cursor)
    }

    /// T-PG.5.1: what 19 accepts, the tables accept; a string that needs an unported action is
    /// answered 0A000.
    #[test]
    fn t_pg_5_1_the_grammar_accepts_what_19_accepts() {
        for sql in [
            "SELECT 1 UNION SELECT 2",
            "select a, b from t where a not in (1, 2) order by 1 nulls first",
            "BEGIN; DELETE FROM pgbench_accounts WHERE aid = $1; END;",
            "WITH x AS (SELECT 1) SELECT * FROM x",
        ] {
            assert_eq!(accepts(sql.as_bytes()), Ok(()), "{sql}");
            assert_eq!(
                parse(sql.as_bytes()).unwrap_err().sqlstate,
                "0A000",
                "{sql}"
            );
        }
    }

    /// T-PG.6.3: BEGIN, END and empty statements parse to 19's trees (the oracle's text, lane PG7's
    /// capture on adacachy2), stmt_len set by the semicolon that ends a statement.
    #[test]
    fn t_pg_6_3_begin_and_end_parse_to_19s_trees() {
        let text = |sql: &str| {
            let (tree, statements) = parse(sql.as_bytes()).unwrap();
            tree.to_text(&statements).unwrap()
        };
        assert_eq!(
            text("BEGIN;"),
            "({RAWSTMT :stmt {TRANSACTIONSTMT :kind 0 :options <> :savepoint_name <> :gid <> :chain false :location -1} :stmt_location 0 :stmt_len 5})"
        );
        assert_eq!(
            text("END;"),
            "({RAWSTMT :stmt {TRANSACTIONSTMT :kind 2 :options <> :savepoint_name <> :gid <> :chain false :location -1} :stmt_location 0 :stmt_len 3})"
        );
        assert_eq!(
            text("BEGIN"),
            "({RAWSTMT :stmt {TRANSACTIONSTMT :kind 0 :options <> :savepoint_name <> :gid <> :chain false :location -1} :stmt_location 0 :stmt_len 0})"
        );
        assert_eq!(text(""), "<>");
        assert_eq!(text(";"), "<>");
        assert_eq!(
            text(" begin ;; end work and chain"),
            "({RAWSTMT :stmt {TRANSACTIONSTMT :kind 0 :options <> :savepoint_name <> :gid <> :chain false :location -1} :stmt_location 1 :stmt_len 6} {RAWSTMT :stmt {TRANSACTIONSTMT :kind 2 :options <> :savepoint_name <> :gid <> :chain true :location -1} :stmt_location 10 :stmt_len 0})"
        );
    }

    /// T-PG.6.1: every ported action names the hash of the C text it was ported from, and that is
    /// the hash the generator recorded for its rule (section 3: a changed action fails here).
    #[test]
    fn t_pg_6_1_each_ported_action_is_current() {
        let mut n = 0;
        for rule in 0..=NRULES {
            if let Some((from, _)) = ported(rule) {
                assert_eq!(
                    ACTION_HASH[usize::try_from(rule).unwrap()],
                    Some(from),
                    "rule {rule}"
                );
                n += 1;
            }
        }
        assert_eq!(n, 88);
    }

    /// T-PG.5.1: a syntax error is 42601 at the token 19 names (1-based characters), the end of
    /// input at one past the last character.
    #[test]
    fn t_pg_5_1_syntax_errors_point_where_19_points() {
        assert_eq!(refused("SELECT FROM WHERE"), ("42601", 13));
        assert_eq!(refused("SELECT 1 +"), ("42601", 11));
        assert_eq!(refused("NOTIFY ch, $1"), ("42601", 12));
        assert_eq!(refused("SELEC 1"), ("42601", 1));
        assert_eq!(refused("select 'é' from from"), ("42601", 17));
    }

    /// T-PG.5.1: the scanner's refusals come through before the grammar sees the token.
    #[test]
    fn t_pg_5_1_scanner_refusals_come_through() {
        assert_eq!(refused("SELECT $1abc"), ("42601", 8));
        assert_eq!(refused("select 'abc"), ("42601", 8));
    }

    /// T-PG.5.1: nesting past yacc.c's YYMAXDEPTH is refused, never a crash.
    #[test]
    fn t_pg_5_1_deep_nesting_is_refused_memory_exhausted() {
        let sql = format!("SELECT {}1{}", "(".repeat(20_000), ")".repeat(20_000));
        let e = accepts(sql.as_bytes()).unwrap_err();
        assert_eq!((e.sqlstate, e.message), ("42601", "memory exhausted"));
    }
}
