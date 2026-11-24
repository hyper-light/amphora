//! bison 3.8.2's LR parse loop (its yacc.c skeleton's `yyparse`) over gram.y's tables
//! (crate::gram_tables), fed by parser.c's `base_yylex` (crate::lex::Lexer): a query string is
//! accepted or refused where PostgreSQL 19's `raw_parser` accepts or refuses it, at the same cursor
//! (docs/design/postgres.md section 3; section 11's units 5 and 6). Beside the state stack it keeps
//! yacc.c's value and location stacks, and on each reduce runs the rule's action ported to Rust
//! ([`ported`]), which builds 19's raw parse tree (crate::tree). A statement that reduces a rule
//! whose action isn't ported yet is answered 0A000.

use std::borrow::Cow;

use crate::gram_tables::{
    ACTION_HASH, CHECK, DEFACT, DEFGOTO, FINAL, LAST, MAXUTOK, NTOKENS, PACT, PACT_DEFAULT, PGOTO,
    R1, R2, TABLE, TABLE_ERROR, TRANSLATE,
};
use crate::lex::{self, Lexer, ScanError, cursor};
use crate::tree::{
    BoolOp, ColumnDef, Constraint, CreateStmt, FuncCall, Node, NodeId, SelectStmt,
    TransactionStmtKind, Tree, TreeFault, Val,
};

/// yacc.c's YYMAXDEPTH, 10000 when the grammar doesn't define it (gram.y doesn't): a parse whose
/// stack would hold that many states is refused "memory exhausted", as 19 refuses it.
const MAX_DEPTH: usize = 10_000;

/// yacc.c's YYSYMBOL_YYEOF and YYSYMBOL_YYUNDEF.
const END: i32 = 0;
const UNDEFINED: i32 = 2;

const SYNTAX_ERROR: &str = "42601";
const FEATURE_NOT_SUPPORTED: &str = "0A000";
const INVALID_PARAMETER_VALUE: &str = "22023";

/// 19's refusal: its SQLSTATE, the ErrorResponse cursor (1-based characters, 0 for none) and message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub sqlstate: &'static str,
    pub cursor: usize,
    pub message: Cow<'static, str>,
}

impl From<ScanError> for Refusal {
    fn from(e: ScanError) -> Self {
        Self {
            sqlstate: e.sqlstate,
            cursor: e.cursor,
            message: e.message.into(),
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
        message: "parser tables fault".into(),
    }
}

/// A name that isn't UTF-8: 19 refuses such bytes when the query arrives (pg_verify_mbstr).
fn not_utf8() -> Refusal {
    Refusal {
        sqlstate: "22021",
        cursor: 0,
        message: "invalid byte sequence for encoding \"UTF8\"".into(),
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
fn syntax_error(sql: &[u8], at: usize, message: impl Into<Cow<'static, str>>) -> Refusal {
    Refusal {
        sqlstate: SYNTAX_ERROR,
        cursor: cursor(sql, at),
        message: message.into(),
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
    /// A `char` an action made (OptTemp's relpersistence).
    Char(char),
    /// func_alias_clause's `list_make2(alias, coldeflist)`.
    FuncAlias {
        alias: Option<NodeId>,
        coldeflist: Vec<NodeId>,
    },
    /// gram.y's SelectLimit, as far as insertSelectOptions stores it.
    Limit {
        offset: Option<NodeId>,
        count: Option<NodeId>,
        option: i32,
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
    sql: &'a [u8],
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

    /// `char *` that may be NULL: an empty rule's `$$ = NULL`, else a token's or action's string.
    fn opt_string(&mut self, n: usize) -> Result<Option<String>, Refusal> {
        match self.take(n) {
            Value::Node(None) => Ok(None),
            Value::Token(lex::Value::Str(bytes)) => {
                String::from_utf8(bytes).map(Some).map_err(|_| not_utf8())
            }
            Value::Token(lex::Value::Keyword(name)) => Ok(Some(name.to_owned())),
            _ => Err(fault()),
        }
    }

    fn char(&mut self, n: usize) -> Result<char, Refusal> {
        match self.take(n) {
            Value::Char(c) => Ok(c),
            _ => Err(fault()),
        }
    }

    /// An `ereport(ERROR, ..., parser_errposition(@n))` in an action: the cursor at `@n`, none
    /// for an empty rule's -1.
    fn refuse(&self, sqlstate: &'static str, n: usize, message: &'static str) -> Refusal {
        let cursor = self
            .at(n)
            .ok()
            .and_then(|at| usize::try_from(at).ok())
            .map_or(0, |at| cursor(self.sql, at));
        Refusal {
            sqlstate,
            cursor,
            message: message.into(),
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
        // iso_level: READ UNCOMMITTED
        244 => (0xc19a_4cd4_72ff_25cf, |_| Ok(word("read uncommitted"))),
        // iso_level: READ COMMITTED
        245 => (0x0a87_ab85_7b8e_b9da, |_| Ok(word("read committed"))),
        // iso_level: REPEATABLE READ
        246 => (0xebcf_5738_e5ad_bcb9, |_| Ok(word("repeatable read"))),
        // iso_level: SERIALIZABLE
        247 => (0x834e_82ee_a2bc_f55b, |_| Ok(word("serializable"))),
        // TransactionStmt: START TRANSACTION transaction_mode_list_or_empty
        1490 => (0xc00d_e3fe_f182_1598, start),
        // transaction_mode_item: ISOLATION LEVEL iso_level
        1506 => (0x08ac_5dea_e963_9f11, isolation_mode),
        // transaction_mode_list: transaction_mode_item
        1511 => (0x28e6_498e_a57f_7c96, list1),
        // transaction_mode_list: transaction_mode_list ',' transaction_mode_item
        1512 => (0x33d4_2538_18d1_c793, append),
        // transaction_mode_list: transaction_mode_list transaction_mode_item
        1513 => (0xf1ce_0b52_dae7_218a, append_second),
        // transaction_mode_list_or_empty: /* EMPTY */
        1515 => (0x2cdf_e4df_9842_f348, |_| Ok(Value::List(Vec::new()))),
        // opt_transaction_chain: AND CHAIN
        1516 => (0xcafd_31ef_0f34_4799, |_| Ok(Value::Bool(true))),
        // opt_transaction_chain: AND NO CHAIN | /* EMPTY */
        1517 | 1518 => (0xc7dd_6751_cbba_2e22, |_| Ok(Value::Bool(false))),
        // opt_qualified_name: /* EMPTY */
        141 => (0x86b3_9e15_ebb4_d128, nil),
        // opt_drop_behavior: /* EMPTY */
        148 => (0x784d_d6d1_0e75_e150, |_| Ok(Value::Int(0))),
        // set_rest: TRANSACTION transaction_mode_list
        218 => (0xb072_946f_10ae_6e3c, set_transaction),
        // VariableSetStmt: SET set_rest
        215 => (0x7e32_ef61_7adc_84a9, |r| set_stmt(r, 2, false)),
        // VariableSetStmt: SET LOCAL set_rest
        216 => (0xe59c_d0d1_bdbb_fdc4, |r| set_stmt(r, 3, true)),
        // VariableSetStmt: SET SESSION set_rest
        217 => (0x7e32_ef61_7adc_84a9, |r| set_stmt(r, 3, false)),
        // generic_set: var_name TO var_list
        221 => (0x3133_3e60_3a82_cdfb, generic_set),
        // generic_set: var_name '=' var_list
        222 => (0x3133_3e60_3a82_cdfb, generic_set),
        // generic_set: var_name TO DEFAULT
        225 => (0x292d_21e5_3b9c_b86e, |r| named_set_kind(r, 1)),
        // generic_set: var_name '=' DEFAULT
        226 => (0x292d_21e5_3b9c_b86e, |r| named_set_kind(r, 1)),
        // set_rest_more: generic_set
        227 => (0xb63c_8f94_4218_f133, top),
        // var_name: ColId
        238 => (0x2c4d_653c_2123_81fb, top),
        // var_name: var_name '.' ColId
        239 => (0x13e3_87cb_b06c_5602, dotted_name),
        // var_list: var_value
        240 => (0x0fda_94aa_d7e7_3ffc, list1),
        // var_list: var_list ',' var_value
        241 => (0x6976_767c_5e07_8201, append),
        // var_value: opt_boolean_or_string
        242 => (0x1721_9ff3_34b4_a393, string_const),
        // var_value: NumericOnly
        243 => (0x3860_77ae_8732_c4e2, numeric_const),
        // opt_boolean_or_string: TRUE_P
        248 => (0xb22a_f767_a16c_1ad4, |_| Ok(word("true"))),
        // opt_boolean_or_string: FALSE_P
        249 => (0x015b_a288_ef4e_5f5b, |_| Ok(word("false"))),
        // opt_boolean_or_string: ON
        250 => (0xbe4d_bf1d_d140_a8e1, |_| Ok(word("on"))),
        // opt_boolean_or_string: NonReservedWord_or_Sconst
        251 => (0x2c4d_653c_2123_81fb, top),
        // NonReservedWord_or_Sconst: NonReservedWord
        262 => (0xa007_892c_3e21_a49b, top),
        // NonReservedWord_or_Sconst: Sconst
        263 => (0x6314_522d_4765_c4bb, top),
        // VariableResetStmt: RESET reset_rest
        264 => (0xc42a_11c7_a7f1_a966, |r| Ok(r.take(2))),
        // reset_rest: generic_reset
        265 => (0x9197_a3d2_c91c_9897, top),
        // generic_reset: var_name
        269 => (0xfdf5_dca3_63d1_242a, |r| named_set_kind(r, 4)),
        // generic_reset: ALL
        270 => (0x03eb_20f3_52f6_bd28, |r| set_kind(r, 5, None)),
        // VariableShowStmt: SHOW var_name
        275 => (0xc00b_e3b1_de12_3e7b, show_stmt),
        // VariableShowStmt: SHOW ALL
        279 => (0xc23e_8a83_7ffd_4aab, show_all),
        // AlterTableStmt: ALTER TABLE relation_expr alter_table_cmds
        291 => (0xdc8a_b7d1_3e26_151c, alter_table),
        // alter_table_cmds: alter_table_cmd
        312 => (0x0fda_94aa_d7e7_3ffc, list1),
        // alter_table_cmd: ADD_P TableConstraint
        342 => (0x0079_2405_28f5_b05a, add_constraint),
        // reloptions: '(' reloption_list ')'
        390 => (0x1f11_a14d_e2c2_31b7, |r| Ok(r.take(2))),
        // reloption_list: reloption_elem
        393 => (0xeff5_8133_d35b_3656, list1),
        // reloption_list: reloption_list ',' reloption_elem
        394 => (0xfede_40a2_0cf3_8a13, append),
        // reloption_elem: ColLabel '=' def_arg
        395 => (0x7594_b8cb_e382_1389, |r| def_elem(r, 3)),
        // reloption_elem: ColLabel
        396 => (0x1fbd_2483_69fc_6fe1, |r| def_elem_named(r, None, None)),
        // CopyStmt: COPY opt_binary qualified_name opt_column_list
        425 => (0x14df_021f_ed0f_64e0, copy_stmt),
        // copy_from: FROM
        427 => (0xbecc_7a2e_167f_61d9, |_| Ok(Value::Bool(true))),
        // copy_from: TO
        428 => (0xf9a7_c703_e0b2_bd02, |_| Ok(Value::Bool(false))),
        // opt_program: /* EMPTY */
        430 => (0x1ee7_dacd_24a2_c362, |_| Ok(Value::Bool(false))),
        // copy_file_name: STDIN
        432 => (0x8ee1_1e76_ad01_e0db, null),
        // copy_file_name: STDOUT
        433 => (0x8ee1_1e76_ad01_e0db, null),
        // copy_options: copy_opt_list
        434 => (0xd863_0378_0bf9_2833, top),
        // copy_options: '(' copy_generic_opt_list ')'
        435 => (0xa34a_19b2_d9e6_9e97, |r| Ok(r.take(2))),
        // copy_opt_list: /* EMPTY */
        437 => (0x5195_4352_6e6c_0588, nil),
        // opt_binary: /* EMPTY */
        455 => (0x6f70_ddc5_f44b_ac02, null),
        // copy_delimiter: /* EMPTY */
        457 => (0x6f70_ddc5_f44b_ac02, null),
        // copy_generic_opt_list: copy_generic_opt_elem
        460 => (0x8674_90cb_f11d_4ff3, list1),
        // copy_generic_opt_list: copy_generic_opt_list ',' copy_generic_opt_elem
        461 => (0x9819_927b_0c58_6e18, append),
        // copy_generic_opt_elem: ColLabel copy_generic_opt_arg
        462 => (0x85bf_7c1b_879a_9e76, |r| def_elem(r, 2)),
        // copy_generic_opt_elem: FORMAT_LA copy_generic_opt_arg
        463 => (0xe73c_d016_326e_623c, |r| {
            let arg = r.node(2)?;
            def_elem_named(r, Some("format"), arg)
        }),
        // copy_generic_opt_arg: /* EMPTY */
        469 => (0xcc68_207a_5bd2_3fdc, null),
        // copy_generic_opt_arg: opt_boolean_or_string
        464 => (0xdd4e_0b68_a2ec_ef61, make_string),
        // CreateStmt: CREATE OptTemp TABLE qualified_name '(' OptTableElementList ')'
        473 => (0xb66d_16f9_b6f7_b41d, create_stmt),
        // OptTemp: /* EMPTY */
        486 => (0x7f78_a3e9_83b0_f1a8, |_| Ok(Value::Char('p'))),
        // OptTableElementList: TableElementList
        487 => (0xd863_0378_0bf9_2833, top),
        // TableElementList: TableElement
        491 => (0x57a1_7057_3bfa_0539, list1),
        // TableElementList: TableElementList ',' TableElement
        492 => (0x3f88_1f70_44bd_239a, append),
        // TableElement: columnDef
        495 => (0x9113_5026_c13a_e4a7, top),
        // columnDef: ColId Typename opt_column_storage opt_column_compression create_generic_options ColQualList
        500 => (0x08a1_38f2_aa0b_2a11, column_def),
        // opt_column_compression: /* EMPTY */
        506 => (0x8ee1_1e76_ad01_e0db, null),
        // opt_column_storage: /* EMPTY */
        510 => (0x8ee1_1e76_ad01_e0db, null),
        // ColQualList: ColQualList ColConstraint
        511 => (0xbcb1_c1ae_81cd_2000, qual_append),
        // ColQualList: /* EMPTY */
        512 => (0x1e2a_fd0a_a524_9ce8, nil),
        // ColConstraint: ColConstraintElem
        514 => (0x9113_5026_c13a_e4a7, top),
        // ColConstraintElem: NOT NULL_P opt_no_inherit
        517 => (0x580c_856a_167c_3fc2, not_null),
        // TableConstraint: ConstraintElem
        555 => (0x9113_5026_c13a_e4a7, top),
        // ConstraintElem: PRIMARY KEY '(' columnList opt_without_overlaps ')' opt_c_include opt_definition OptConsTableSpace
        560 => (0x5cb7_b4ab_11fe_3203, primary_key),
        // opt_no_inherit: /* EMPTY */
        569 => (0x0669_4456_7c57_3476, |_| Ok(Value::Bool(false))),
        // opt_without_overlaps: /* EMPTY */
        571 => (0xf9a7_c703_e0b2_bd02, |_| Ok(Value::Bool(false))),
        // opt_column_list: /* EMPTY */
        // opt_column_list: '(' columnList ')'
        572 => (0xd2b3_15b0_9702_83d7, |r| Ok(Value::List(r.list(2)?))),
        573 => (0x1e2a_fd0a_a524_9ce8, nil),
        // columnList: columnElem
        574 => (0x41fd_9d1b_bdab_ed5c, list1),
        // columnList: columnList ',' columnElem
        575 => (0x6976_767c_5e07_8201, append),
        // columnElem: ColId
        580 => (0x7baf_b980_ab5f_c89c, make_string),
        // opt_c_include: /* EMPTY */
        582 => (0x1e2a_fd0a_a524_9ce8, nil),
        // OptInherit: /* EMPTY */
        606 => (0x1e2a_fd0a_a524_9ce8, nil),
        // OptPartitionSpec: PartitionSpec
        607 => (0xc85e_a996_47dc_cd93, top),
        // OptPartitionSpec: /* EMPTY */
        608 => (0x582e_440b_6ba4_80f6, null),
        // PartitionSpec: PARTITION BY ColId '(' part_params ')'
        609 => (0x8fd4_ba17_e148_e5d7, partition_spec),
        // part_params: part_elem
        610 => (0x57ea_2889_6e5b_5822, list1),
        // part_elem: ColId opt_collate opt_qualified_name
        612 => (0xfa06_b422_7cbf_f8b1, part_elem),
        // table_access_method_clause: /* EMPTY */
        616 => (0x9a64_6afd_218c_483b, null),
        // OptWith: WITH reloptions
        617 => (0xb179_ffe1_a274_6173, top),
        // OptWith: /* EMPTY */
        619 => (0xb2e2_4c4d_0c3a_b6c8, nil),
        // OnCommitOption: /* EMPTY */
        623 => (0x181b_b893_5641_aa52, |_| Ok(Value::Int(0))),
        // OptTableSpace: /* EMPTY */
        625 => (0x8ee1_1e76_ad01_e0db, null),
        // OptConsTableSpace: /* EMPTY */
        627 => (0x8ee1_1e76_ad01_e0db, null),
        // NumericOnly: FCONST
        678 => (0x12df_1e0d_616c_3850, |r| float_value(r, 1, false)),
        // NumericOnly: '+' FCONST
        679 => (0xe112_18d7_b2ec_bcb0, |r| float_value(r, 2, false)),
        // NumericOnly: '-' FCONST
        680 => (0xd322_50a9_608f_c449, |r| float_value(r, 2, true)),
        // NumericOnly: SignedIconst
        681 => (0x699b_98a9_71ef_9ef1, integer),
        // create_generic_options: /* EMPTY */
        742 => (0xa50c_a63f_49f4_3a48, nil),
        // ConstraintAttributeSpec: /* EMPTY */
        845 => (0xcc31_e8d4_7f4c_796f, |_| Ok(Value::Int(0))),
        // def_arg: NumericOnly
        892 => (0x9396_22ca_37a3_3dd2, top),
        // DropStmt: DROP object_type_any_name IF_P EXISTS any_name_list opt_drop_behavior
        938 => (0x120f_6e9b_a03b_0c9a, drop_if_exists),
        // object_type_any_name: TABLE
        950 => (0x08cd_40da_a814_1242, |_| Ok(Value::Int(42))),
        // any_name_list: any_name
        979 => (0x1209_f60e_7c8e_69a8, any_name_list),
        // any_name_list: any_name_list ',' any_name
        980 => (0x721c_8e26_0e62_7609, any_name_append),
        // any_name: ColId
        981 => (0xad62_ca6c_ccde_7143, any_name),
        // attrs: '.' attr_name
        983 => (0x2eeb_eadd_6516_86e3, attrs_first),
        // attrs: attrs '.' attr_name
        984 => (0x4e4b_623b_606d_cb78, attrs_append),
        // TruncateStmt: TRUNCATE opt_table relation_expr_list opt_restart_seqs opt_drop_behavior
        987 => (0x1fdb_3a96_2e09_cf75, truncate_stmt),
        // opt_restart_seqs: /* EMPTY */
        990 => (0x2562_84d6_595c_3282, |_| Ok(Value::Bool(false))),
        // opt_collate: /* EMPTY */
        1139 => (0x1e2a_fd0a_a524_9ce8, nil),
        // opt_asc_desc: ASC
        1140 => (0xe77c_326d_e6dc_6368, |_| Ok(Value::Int(1))),
        // opt_nulls_order: /* EMPTY */
        1145 => (0x0b11_90e3_6dba_5283, |_| Ok(Value::Int(0))),
        // opt_definition: /* EMPTY */
        1233 => (0x1e2a_fd0a_a524_9ce8, nil),
        // TransactionStmt: ABORT_P opt_transaction opt_transaction_chain
        1489 => (0xd07f_6118_1eda_3390, rollback),
        // TransactionStmt: COMMIT opt_transaction opt_transaction_chain
        1491 => (0x88ca_638c_a2a9_d9bb, end),
        // TransactionStmt: ROLLBACK opt_transaction opt_transaction_chain
        1492 => (0xd07f_6118_1eda_3390, rollback),
        // VacuumStmt: VACUUM opt_full opt_freeze opt_verbose opt_analyze opt_vacuum_relation_list
        1590 => (0x250c_65d1_45e2_42d6, vacuum_stmt),
        // opt_analyze: analyze_keyword
        1596 => (0x4ab8_280b_bc3a_91b9, |_| Ok(Value::Bool(true))),
        // opt_analyze: /*EMPTY*/
        1597 => (0xf9a7_c703_e0b2_bd02, |_| Ok(Value::Bool(false))),
        // opt_verbose: /* EMPTY */
        1599 => (0xf9a7_c703_e0b2_bd02, |_| Ok(Value::Bool(false))),
        // opt_full: /* EMPTY */
        1601 => (0xf9a7_c703_e0b2_bd02, |_| Ok(Value::Bool(false))),
        // opt_freeze: /* EMPTY */
        1603 => (0xf9a7_c703_e0b2_bd02, |_| Ok(Value::Bool(false))),
        // opt_name_list: /* EMPTY */
        1605 => (0x1e2a_fd0a_a524_9ce8, nil),
        // vacuum_relation: relation_expr opt_name_list
        1606 => (0xcd1d_1c29_89b2_a1c9, vacuum_relation),
        // vacuum_relation_list: vacuum_relation
        1607 => (0x6a0d_9b4a_8e11_133c, list1),
        // opt_vacuum_relation_list: vacuum_relation_list
        1609 => (0xd863_0378_0bf9_2833, top),
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
        // select_with_parens: '(' select_no_parens ')'
        1733 => (0x35f8_c5c0_6c96_56bb, |r| Ok(r.take(2))),
        // select_no_parens: simple_select
        1735 => (0xfd2f_9ec2_19c8_d787, top),
        // select_no_parens: select_clause opt_sort_clause select_limit opt_for_locking_clause
        1738 => (0xeafc_1cea_3dce_5bd8, select_options),
        // select_clause: simple_select
        1743 => (0x9113_5026_c13a_e4a7, top),
        // simple_select: SELECT opt_all_clause opt_target_list
        1745 => (0x5a6d_b6e1_b8fa_52e2, simple_select),
        // simple_select: values_clause
        1747 => (0x9113_5026_c13a_e4a7, top),
        // opt_with_clause: /* EMPTY */
        1768 => (0xf8ee_ad8a_b545_1b54, null),
        // into_clause: /* EMPTY */
        1770 => (0xb6f0_5cf7_ff18_9c86, null),
        // set_quantifier: /* EMPTY */
        1784 => (0xb02b_6c1e_060b_daa0, |_| Ok(Value::Int(0))),
        // opt_sort_clause: sort_clause
        1791 => (0x728e_0d00_0e99_dcf3, top),
        // opt_sort_clause: /* EMPTY */
        1792 => (0x1e2a_fd0a_a524_9ce8, nil),
        // sort_clause: ORDER BY sortby_list
        1793 => (0xd863_0378_0bf9_2833, top),
        // sortby_list: sortby
        1794 => (0x6d50_77ee_7de0_e14f, list1),
        // sortby: a_expr opt_asc_desc opt_nulls_order
        1797 => (0x9a8c_51b5_784b_e329, sort_by),
        // select_limit: limit_clause
        1800 => (0x54a2_d13b_0eee_a2ce, top),
        // limit_clause: LIMIT select_limit_value
        1804 => (0x6bde_211d_e6c9_c953, limit),
        // select_limit_value: a_expr
        1812 => (0xe963_95cd_c2fd_e247, top),
        // group_clause: GROUP_P BY set_quantifier group_by_list
        1824 => (0x9266_6d88_fbf4_19a3, group_by),
        // group_clause: /* EMPTY */
        1825 => (0x0321_6edd_97cb_a203, no_group),
        // group_by_list: group_by_item
        1826 => (0x0fda_94aa_d7e7_3ffc, list1),
        // group_by_list: group_by_list ',' group_by_item
        1827 => (0xb1dd_ab8e_1708_cd31, append),
        // group_by_item: a_expr
        1828 => (0xe963_95cd_c2fd_e247, top),
        // having_clause: /* EMPTY */
        1838 => (0x2ef6_8e2d_5a42_4abc, null),
        // opt_for_locking_clause: /* EMPTY */
        1842 => (0x5195_4352_6e6c_0588, nil),
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
        // table_ref: LATERAL_P select_with_parens opt_alias_clause
        1867 => (0xa2b8_3f93_88bc_e040, lateral_subselect),
        // table_ref: joined_table
        1868 => (0x14fd_3bb9_6f1d_61e0, top),
        // joined_table: table_ref CROSS JOIN table_ref
        1873 => (0xc074_2d9d_5b67_fbca, cross_join),
        // joined_table: table_ref join_type JOIN table_ref join_qual
        1874 => (0x26e2_ab46_39e5_0dab, typed_join),
        // joined_table: table_ref JOIN table_ref join_qual
        1875 => (0xe461_6ad1_6cd3_fe21, inner_join),
        // alias_clause: AS ColId '(' name_list ')'
        1878 => (0xe7be_9fd2_ff3e_ffb5, alias_columns),
        // alias_clause: AS ColId
        1879 => (0x59f2_2c8a_f16a_a2ef, alias),
        // opt_alias_clause: alias_clause
        1882 => (0x9e57_90c4_a176_5385, top),
        // opt_alias_clause: /* EMPTY */
        1883 => (0xd843_f1a2_034c_c3a2, null),
        // func_alias_clause: alias_clause
        1886 => (0x5fd2_8df3_dcf9_cac1, func_alias),
        // join_type: LEFT opt_outer
        1892 => (0xa884_3c48_4bcd_af3d, |_| Ok(Value::Int(1))),
        // join_qual: ON a_expr
        1898 => (0x0daa_dfab_dabf_50ca, top),
        // relation_expr: qualified_name
        1899 => (0x0f6f_0ebf_dca7_954d, relation_expr),
        // relation_expr_list: relation_expr
        1904 => (0x90a5_ec2d_34a9_259b, list1),
        // relation_expr_list: relation_expr_list ',' relation_expr
        1905 => (0xfa14_5ca9_09c6_40c8, append),
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
        // Typename: SimpleTypename opt_array_bounds
        1964 => (0xc58b_b6eb_3ec8_28bf, typename),
        // opt_array_bounds: /* EMPTY */
        1972 => (0x5be7_5729_4b12_8d74, nil),
        // SimpleTypename: GenericType
        1973 => (0x0cb0_b34c_e5bd_f9f3, top),
        // SimpleTypename: Numeric
        1974 => (0x0cb0_b34c_e5bd_f9f3, top),
        // SimpleTypename: Character
        1976 => (0x0cb0_b34c_e5bd_f9f3, top),
        // SimpleTypename: ConstDatetime
        1977 => (0x54ea_3776_f177_7e53, top),
        // GenericType: type_function_name opt_type_modifiers
        1986 => (0xa0fc_f20c_7374_334f, |r| generic_type(r, false)),
        // GenericType: type_function_name attrs opt_type_modifiers
        1987 => (0x9241_9142_11fe_2eae, |r| generic_type(r, true)),
        // opt_type_modifiers: /* EMPTY */
        1989 => (0x5195_4352_6e6c_0588, nil),
        // Numeric: INT_P
        1990 => (0x7ebf_d128_317b_820d, |r| system_type_at(r, "int4")),
        // Numeric: BIGINT
        1993 => (0xc92a_6be6_f04b_95f1, |r| system_type_at(r, "int8")),
        // Character: CharacterWithLength
        2009 => (0x23a8_122b_16a8_44c6, top),
        // CharacterWithLength: character '(' Iconst ')'
        2013 => (0x6cd1_8050_a87f_85ea, character_with_length),
        // character: CHAR_P opt_varying
        2016 => (0xa771_08d3_389e_366e, character),
        // opt_varying: /* EMPTY */
        2022 => (0xf9a7_c703_e0b2_bd02, |_| Ok(Value::Bool(false))),
        // ConstDatetime: TIMESTAMP opt_timezone
        2024 => (0x4ef3_5933_9f92_c2dc, timestamp),
        // opt_timezone: /* EMPTY */
        2030 => (0xf9a7_c703_e0b2_bd02, |_| Ok(Value::Bool(false))),
        // a_expr: c_expr
        2048 => (0x9113_5026_c13a_e4a7, top),
        // a_expr: a_expr TYPECAST Typename
        2049 => (0xf291_a789_1854_d33d, type_cast),
        // a_expr: '-' a_expr
        2054 => (0x50eb_9947_0f67_cd77, negate),
        // a_expr: a_expr '+' a_expr
        2055 => (0x34ef_b0cc_0323_8888, |r| a_expr_op(r, "+")),
        // a_expr: a_expr '-' a_expr
        2056 => (0x94b8_110d_a054_53de, |r| a_expr_op(r, "-")),
        // a_expr: a_expr '/' a_expr
        2058 => (0x25dc_16de_5219_d95c, |r| a_expr_op(r, "/")),
        // a_expr: a_expr '<' a_expr
        2061 => (0x40e8_d732_92b6_8e93, |r| a_expr_op(r, "<")),
        // a_expr: a_expr '>' a_expr
        2062 => (0x03a0_38c5_bbe4_2bc1, |r| a_expr_op(r, ">")),
        // a_expr: a_expr '=' a_expr
        2063 => (0x8a20_32d0_a75b_2e2e, |r| a_expr_op(r, "=")),
        // a_expr: a_expr LESS_EQUALS a_expr
        2064 => (0x49ff_5b50_583e_4358, |r| a_expr_op(r, "<=")),
        // a_expr: a_expr GREATER_EQUALS a_expr
        2065 => (0x0dfb_6c26_8c24_8fa2, |r| a_expr_op(r, ">=")),
        // a_expr: a_expr NOT_EQUALS a_expr
        2066 => (0x9ded_22b4_57b1_9aff, |r| a_expr_op(r, "<>")),
        // a_expr: a_expr AND a_expr
        2069 => (0x9b8b_3cb2_7846_ac2b, and_expr),
        // a_expr: a_expr IS NOT NULL_P
        2087 => (0x2ce6_900c_856e_049e, is_not_null),
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
        // func_application: func_name '(' ')'
        2154 => (0x299c_3568_049b_5c90, func_no_args),
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
        // columnref: ColId indirection
        2373 => (0x9b92_967c_258a_d5db, qualified_columnref),
        // indirection_el: '.' attr_name
        2374 => (0x7baf_b980_ab5f_c89c, field),
        // indirection: indirection_el
        2380 => (0x0fda_94aa_d7e7_3ffc, list1),
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
        // qualified_name: ColId indirection
        2465 => (0x1609_9d21_21a1_3cbe, dotted_relation),
        // name_list: name
        2466 => (0x2eeb_eadd_6516_86e3, func_name),
        // name: ColId
        2468 => (0xa007_892c_3e21_a49b, top),
        // attr_name: ColLabel
        2469 => (0xa007_892c_3e21_a49b, top),
        // func_name: type_function_name
        2471 => (0x2eeb_eadd_6516_86e3, func_name),
        // func_name: ColId indirection
        2472 => (0x2898_bc6a_e871_63c8, qualified_func_name),
        // AexprConst: Iconst
        2473 => (0xedd7_3fba_5686_550f, int_const),
        // AexprConst: FCONST
        2474 => (0x38e4_cd19_2237_3ce1, float_const),
        // AexprConst: Sconst
        2475 => (0xeb6d_7390_faf1_1cfe, string_const),
        // AexprConst: TRUE_P
        2483 => (0x8819_5c83_6cd1_1185, true_const),
        // Iconst: ICONST
        2486 => (0x35a8_92e8_20e5_a06b, top),
        // Sconst: SCONST
        2487 => (0xa007_892c_3e21_a49b, top),
        // SignedIconst: Iconst
        2488 => (0xd710_1847_a287_d2cb, top),
        // SignedIconst: '+' Iconst
        2489 => (0x7192_1f1f_75bd_9188, |r| Ok(Value::Int(r.int(2)?))),
        // SignedIconst: '-' Iconst
        2490 => (0x39c0_f4c2_5e5c_17aa, negative_iconst),
        // ColId: IDENT
        2504 => (0xa007_892c_3e21_a49b, top),
        // type_function_name: IDENT
        2507 => (0x2c4d_653c_2123_81fb, top),
        // NonReservedWord: IDENT
        2510 => (0x2c4d_653c_2123_81fb, top),
        // NonReservedWord: unreserved_keyword
        2511 => (0x1d4d_f16c_54a7_2564, top),
        // NonReservedWord: col_name_keyword
        2512 => (0x0f87_a177_22a9_8584, top),
        // NonReservedWord: type_func_name_keyword
        2513 => (0x1d4d_f16c_54a7_2564, top),
        // ColLabel: IDENT
        2514 => (0xa007_892c_3e21_a49b, top),
        // ColLabel: type_func_name_keyword
        2517 => (0x1d4d_f16c_54a7_2564, top),
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

/// `START TRANSACTION transaction_mode_list_or_empty`: BEGIN's statement, kind TRANS_STMT_START.
fn start(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let options = r.list(3)?;
    transaction(r, TransactionStmtKind::Start, options, false)
}

/// `transaction_mode_item: ISOLATION LEVEL iso_level`: makeDefElem("transaction_isolation",
/// makeStringConst($3, @3), @1).
fn isolation_mode(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let level = r.string(3)?;
    let at = r.at(3)?;
    let arg = r.push(Node::AConst {
        val: Val::String(level),
        location: at,
    })?;
    let location = r.at(1)?;
    r.make(Node::DefElem {
        defnamespace: None,
        defname: "transaction_isolation".to_owned(),
        arg: Some(arg),
        defaction: 0,
        location,
    })
}

/// `lappend($1, $2)`.
fn append_second(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let mut list = r.list(1)?;
    list.push(r.node(2)?.ok_or_else(fault)?);
    Ok(Value::List(list))
}

/// `set_rest: TRANSACTION transaction_mode_list`: VAR_SET_MULTI, 3, named "TRANSACTION".
fn set_transaction(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let args = r.list(2)?;
    r.make(Node::VariableSetStmt {
        kind: 3,
        name: Some("TRANSACTION".to_owned()),
        args,
        jumble_args: true,
        is_local: false,
        location: -1,
    })
}

fn end(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let chain = r.boolean(3)?;
    transaction(r, TransactionStmtKind::Commit, Vec::new(), chain)
}

fn rollback(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let chain = r.boolean(3)?;
    transaction(r, TransactionStmtKind::Rollback, Vec::new(), chain)
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
            Val::Boolean(_) | Val::String(_) => {}
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

/// `select_no_parens: select_clause opt_sort_clause select_limit opt_for_locking_clause`:
/// insertSelectOptions. Its refusals need a clause the statement already has, which only unported
/// rules make, so such a statement declines as unported.
fn select_options(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let stmt = r.node(1)?.ok_or_else(fault)?;
    let sort = r.list(2)?;
    let Value::Limit {
        offset,
        count,
        option,
    } = r.take(3)
    else {
        return Err(fault());
    };
    let locking = r.list(4)?;
    let Node::SelectStmt(n) = r.tree.get_mut(stmt)? else {
        return Err(fault());
    };
    let conflict = (!sort.is_empty() && !n.sort_clause.is_empty())
        || (offset.is_some() && n.limit_offset.is_some())
        || (count.is_some() && n.limit_count.is_some())
        || option != 0;
    if conflict {
        return Ok(Value::Unported);
    }
    if !sort.is_empty() {
        n.sort_clause = sort;
    }
    n.locking_clause.extend(locking);
    if offset.is_some() {
        n.limit_offset = offset;
    }
    if count.is_some() {
        n.limit_count = count;
    }
    n.limit_option = option;
    Ok(Value::Node(Some(stmt)))
}

/// `sortby: a_expr opt_asc_desc opt_nulls_order`.
fn sort_by(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let node = r.node(1)?;
    let sortby_dir = r.int(2)?;
    let sortby_nulls = r.int(3)?;
    r.make(Node::SortBy {
        node,
        sortby_dir,
        sortby_nulls,
        use_op: Vec::new(),
        location: -1,
    })
}

/// `limit_clause: LIMIT select_limit_value`: LIMIT_OPTION_COUNT, 0.
fn limit(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    Ok(Value::Limit {
        offset: None,
        count: r.node(2)?,
        option: 0,
    })
}

/// `group_clause: GROUP_P BY set_quantifier group_by_list`; SET_QUANTIFIER_DISTINCT is 2.
fn group_by(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let quantifier = r.int(3)?;
    let list = r.list(4)?;
    Ok(Value::Group {
        distinct: quantifier == 2,
        list,
    })
}

/// `table_ref: LATERAL_P select_with_parens opt_alias_clause`.
fn lateral_subselect(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let subquery = r.node(2)?;
    let alias = r.node(3)?;
    r.make(Node::RangeSubselect {
        lateral: true,
        subquery,
        alias,
    })
}

/// A JoinExpr with no USING (join_qual's USING form isn't ported).
fn join(
    r: &mut Reduce<'_>,
    jointype: i32,
    larg: Option<NodeId>,
    rarg: Option<NodeId>,
    quals: Option<NodeId>,
) -> Result<Value, Refusal> {
    r.make(Node::JoinExpr {
        jointype,
        is_natural: false,
        larg,
        rarg,
        using_clause: Vec::new(),
        join_using_alias: None,
        quals,
        alias: None,
        rtindex: 0,
    })
}

/// `table_ref CROSS JOIN table_ref`: JOIN_INNER, 0.
fn cross_join(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let larg = r.node(1)?;
    let rarg = r.node(4)?;
    join(r, 0, larg, rarg, None)
}

/// `table_ref join_type JOIN table_ref join_qual`.
fn typed_join(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let larg = r.node(1)?;
    let jointype = r.int(2)?;
    let rarg = r.node(4)?;
    let quals = r.node(5)?;
    join(r, jointype, larg, rarg, quals)
}

/// `table_ref JOIN table_ref join_qual`: JOIN_INNER.
fn inner_join(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let larg = r.node(1)?;
    let rarg = r.node(3)?;
    let quals = r.node(4)?;
    join(r, 0, larg, rarg, quals)
}

/// `alias_clause: AS ColId '(' name_list ')'`.
fn alias_columns(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let aliasname = r.string(2)?;
    let colnames = r.list(4)?;
    r.make(Node::Alias {
        aliasname,
        colnames,
    })
}

/// makeAndExpr: an AND joins the BoolExpr of an AND on its left.
fn and_expr(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let left = r.node(1)?.ok_or_else(fault)?;
    let right = r.node(3)?.ok_or_else(fault)?;
    let location = r.at(2)?;
    if let Node::BoolExpr {
        boolop: BoolOp::And,
        args,
        ..
    } = r.tree.get_mut(left)?
    {
        args.push(right);
        return Ok(Value::Node(Some(left)));
    }
    r.make(Node::BoolExpr {
        boolop: BoolOp::And,
        args: vec![left, right],
        location,
    })
}

/// `a_expr IS NOT NULL_P`: IS_NOT_NULL, 1.
fn is_not_null(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let arg = r.node(1)?;
    let location = r.at(2)?;
    r.make(Node::NullTest {
        arg,
        nulltesttype: 1,
        argisrow: false,
        location,
    })
}

/// `ColId indirection` as a name list: the name, then the indirection's names (only
/// `'.' attr_name` is ported, so each is a String, as check_func_name and makeColumnRef require).
fn dotted(r: &mut Reduce<'_>) -> Result<Vec<NodeId>, Refusal> {
    let name = r.string(1)?;
    let rest = r.list(2)?;
    let mut names = vec![r.push(Node::String(name))?];
    names.extend(rest);
    Ok(names)
}

/// `columnref: ColId indirection`: makeColumnRef with field selection only.
fn qualified_columnref(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let location = r.at(1)?;
    let fields = dotted(r)?;
    r.make(Node::ColumnRef { fields, location })
}

/// `func_name: ColId indirection`: check_func_name(lcons(makeString($1), $2)).
fn qualified_func_name(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    Ok(Value::List(dotted(r)?))
}

/// `indirection_el: '.' attr_name`: makeString($2).
fn field(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let name = r.string(2)?;
    r.make(Node::String(name))
}

/// `qualified_name: ColId indirection`: makeRangeVarFromQualifiedName. Its refusal of more than
/// three names isn't ported: such a name declines as unported.
fn dotted_relation(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let name = r.string(1)?;
    let rest = r.list(2)?;
    let location = r.at(1)?;
    let mut names = Vec::with_capacity(rest.len());
    for id in rest {
        match r.tree.get(id)? {
            Node::String(s) => names.push(s.clone()),
            _ => return Err(fault()),
        }
    }
    let (catalogname, schemaname, relname) = match <[String; 1]>::try_from(names) {
        Ok([relname]) => (None, name, relname),
        Err(names) => match <[String; 2]>::try_from(names) {
            Ok([schema, relname]) => (Some(name), schema, relname),
            Err(_) => return Ok(Value::Unported),
        },
    };
    r.make(Node::RangeVar {
        catalogname,
        schemaname: Some(schemaname),
        relname,
        inh: true,
        relpersistence: 'p',
        alias: None,
        location,
    })
}

/// makeBoolAConst(true, @1).
fn true_const(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let location = r.at(1)?;
    r.make(Node::AConst {
        val: Val::Boolean(true),
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

/// A C string constant an action returns (`$$ = "on"`).
fn word(s: &'static str) -> Value {
    Value::Token(lex::Value::Keyword(s))
}

/// `makeString($1)` of a rule's one symbol (columnElem, copy_generic_opt_arg).
fn make_string(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let s = r.string(1)?;
    r.make(Node::String(s))
}

/// `any_name: ColId`: list_make1(makeString($1)).
fn any_name(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let name = r.string(1)?;
    Ok(Value::List(vec![r.push(Node::String(name))?]))
}

/// `$n`, a List, as an element of another List.
fn list_node(r: &mut Reduce<'_>, n: usize) -> Result<NodeId, Refusal> {
    let items = r.list(n)?;
    r.push(Node::List(items.into_iter().map(Some).collect()))
}

/// `any_name_list: any_name`: list_make1 of a name list.
fn any_name_list(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    Ok(Value::List(vec![list_node(r, 1)?]))
}

/// `any_name_list: any_name_list ',' any_name`: lappend of a name list.
fn any_name_append(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let mut list = r.list(1)?;
    list.push(list_node(r, 3)?);
    Ok(Value::List(list))
}

/// `ColQualList: ColQualList ColConstraint`: lappend($1, $2).
fn qual_append(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let mut list = r.list(1)?;
    list.push(r.node(2)?.ok_or_else(fault)?);
    Ok(Value::List(list))
}

/// `DROP object_type_any_name IF_P EXISTS any_name_list opt_drop_behavior`.
fn drop_if_exists(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let remove_type = r.int(2)?;
    let objects = r.list(5)?;
    let behavior = r.int(6)?;
    r.make(Node::DropStmt {
        objects,
        remove_type,
        behavior,
        missing_ok: true,
        concurrent: false,
    })
}

/// `TRUNCATE opt_table relation_expr_list opt_restart_seqs opt_drop_behavior`.
fn truncate_stmt(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let relations = r.list(3)?;
    let restart_seqs = r.boolean(4)?;
    let behavior = r.int(5)?;
    r.make(Node::TruncateStmt {
        relations,
        restart_seqs,
        behavior,
    })
}

/// SystemTypeName (pg_catalog's type `name`) with its typmods, at `location`.
fn system_type(
    r: &mut Reduce<'_>,
    name: String,
    typmods: Vec<NodeId>,
    location: i32,
) -> Result<Value, Refusal> {
    let names = vec![
        r.push(Node::String("pg_catalog".to_owned()))?,
        r.push(Node::String(name))?,
    ];
    r.make(Node::TypeName {
        names,
        type_oid: 0,
        setof: false,
        pct_type: false,
        typmods,
        typemod: -1,
        array_bounds: Vec::new(),
        location,
    })
}

/// SystemTypeName at @1 (`Numeric: INT_P`, `BIGINT`, ConstDatetime).
fn system_type_at(r: &mut Reduce<'_>, name: &str) -> Result<Value, Refusal> {
    let location = r.at(1)?;
    system_type(r, name.to_owned(), Vec::new(), location)
}

/// `ConstDatetime: TIMESTAMP opt_timezone`.
fn timestamp(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let name = if r.boolean(2)? {
        "timestamptz"
    } else {
        "timestamp"
    };
    system_type_at(r, name)
}

/// `character: CHAR_P opt_varying`.
fn character(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    Ok(word(if r.boolean(2)? { "varchar" } else { "bpchar" }))
}

/// `CharacterWithLength: character '(' Iconst ')'`: the length a typmod, makeIntConst at @3.
fn character_with_length(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let name = r.string(1)?;
    let length = r.int(3)?;
    let at = r.at(3)?;
    let location = r.at(1)?;
    let typmod = r.push(Node::AConst {
        val: Val::Integer(length),
        location: at,
    })?;
    system_type(r, name, vec![typmod], location)
}

/// `Typename: SimpleTypename opt_array_bounds`.
fn typename(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let id = r.node(1)?.ok_or_else(fault)?;
    let bounds = r.list(2)?;
    let Node::TypeName { array_bounds, .. } = r.tree.get_mut(id)? else {
        return Err(fault());
    };
    *array_bounds = bounds;
    Ok(Value::Node(Some(id)))
}

/// `columnDef`. SplitColQualList keeps the Constraints and takes out a COLLATE clause; only
/// ColConstraintElem's ported rules reach ColQualList, so every element is a Constraint.
fn column_def(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let colname = r.string(1)?;
    let type_name = r.node(2)?;
    let storage_name = r.opt_string(3)?;
    let compression = r.opt_string(4)?;
    let fdwoptions = r.list(5)?;
    let constraints = r.list(6)?;
    let location = r.at(1)?;
    r.make(Node::ColumnDef(Box::new(ColumnDef {
        colname: Some(colname),
        type_name,
        compression,
        is_local: true,
        storage_name,
        constraints,
        fdwoptions,
        location,
        ..ColumnDef::default()
    })))
}

/// `ColConstraintElem: NOT NULL_P opt_no_inherit`; parsenodes.h numbers CONSTR_NOTNULL 1.
fn not_null(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let is_no_inherit = r.boolean(3)?;
    let location = r.at(1)?;
    r.make(Node::Constraint(Box::new(Constraint {
        contype: 1,
        is_no_inherit,
        is_enforced: true,
        initially_valid: true,
        location,
        ..Constraint::default()
    })))
}

/// `ConstraintElem: PRIMARY KEY '(' columnList ...`; CONSTR_PRIMARY is 6. processCASbits with no
/// bits only clears deferrable and initdeferred; any bits come from ConstraintAttributeSpec's
/// unported rules, so a statement with them declines as unported.
fn primary_key(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let location = r.at(1)?;
    let keys = r.list(4)?;
    let without_overlaps = r.boolean(5)?;
    let including = r.list(7)?;
    let options = r.list(8)?;
    let indexspace = r.opt_string(9)?;
    if r.int(10)? != 0 {
        return Ok(Value::Unported);
    }
    r.make(Node::Constraint(Box::new(Constraint {
        contype: 6,
        keys,
        without_overlaps,
        including,
        options,
        indexspace,
        location,
        ..Constraint::default()
    })))
}

/// `CREATE OptTemp TABLE qualified_name '(' OptTableElementList ')' ...`.
fn create_stmt(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let persistence = r.char(2)?;
    let relation = r.node(4)?.ok_or_else(fault)?;
    let table_elts = r.list(6)?;
    let inh_relations = r.list(8)?;
    let partspec = r.node(9)?;
    let access_method = r.opt_string(10)?;
    let options = r.list(11)?;
    let oncommit = r.int(12)?;
    let tablespacename = r.opt_string(13)?;
    let Node::RangeVar { relpersistence, .. } = r.tree.get_mut(relation)? else {
        return Err(fault());
    };
    *relpersistence = persistence;
    r.make(Node::CreateStmt(Box::new(CreateStmt {
        relation: Some(relation),
        table_elts,
        inh_relations,
        partspec,
        options,
        oncommit,
        tablespacename,
        access_method,
        ..CreateStmt::default()
    })))
}

/// `NumericOnly: SignedIconst`: makeInteger.
fn integer(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let v = r.int(1)?;
    r.make(Node::Integer(v))
}

/// makeDefElem($1, $arg, @1) (reloption_elem, copy_generic_opt_elem).
fn def_elem(r: &mut Reduce<'_>, arg: usize) -> Result<Value, Refusal> {
    let defname = r.string(1)?;
    let arg = r.node(arg)?;
    let location = r.at(1)?;
    r.make(Node::DefElem {
        defnamespace: None,
        defname,
        arg,
        defaction: 0,
        location,
    })
}

/// makeDefElem(name, arg, @1): the name its own token, where none is given.
fn def_elem_named(
    r: &mut Reduce<'_>,
    name: Option<&str>,
    arg: Option<NodeId>,
) -> Result<Value, Refusal> {
    let defname = match name {
        Some(name) => name.to_owned(),
        None => r.string(1)?,
    };
    let location = r.at(1)?;
    r.make(Node::DefElem {
        defnamespace: None,
        defname,
        arg,
        defaction: 0,
        location,
    })
}

/// `vacuum_relation: relation_expr opt_name_list`: makeVacuumRelation($1, InvalidOid, $2).
fn vacuum_relation(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let relation = r.node(1)?;
    let va_cols = r.list(2)?;
    r.make(Node::VacuumRelation {
        relation,
        oid: 0,
        va_cols,
    })
}

/// `VACUUM opt_full opt_freeze opt_verbose opt_analyze opt_vacuum_relation_list`.
fn vacuum_stmt(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let mut options = Vec::new();
    for (n, defname) in [(2, "full"), (3, "freeze"), (4, "verbose"), (5, "analyze")] {
        if r.boolean(n)? {
            let location = r.at(n)?;
            options.push(r.push(Node::DefElem {
                defnamespace: None,
                defname: defname.to_owned(),
                arg: None,
                defaction: 0,
                location,
            })?);
        }
    }
    let rels = r.list(6)?;
    r.make(Node::VacuumStmt {
        options,
        rels,
        is_vacuumcmd: true,
    })
}

/// `alter_table_cmd: ADD_P TableConstraint`; AT_AddConstraint is 16.
fn add_constraint(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let def = r.node(2)?;
    r.make(Node::AlterTableCmd {
        subtype: 16,
        name: None,
        num: 0,
        newowner: None,
        def,
        behavior: 0,
        missing_ok: false,
        recurse: false,
    })
}

/// `ALTER TABLE relation_expr alter_table_cmds`; OBJECT_TABLE is 42.
fn alter_table(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let relation = r.node(3)?;
    let cmds = r.list(4)?;
    r.make(Node::AlterTableStmt {
        relation,
        cmds,
        objtype: 42,
        missing_ok: false,
    })
}

/// `COPY opt_binary qualified_name opt_column_list copy_from opt_program copy_file_name
/// copy_delimiter opt_with copy_options where_clause`, with its two refusals.
fn copy_stmt(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let binary = r.node(2)?;
    let relation = r.node(3)?;
    let attlist = r.list(4)?;
    let is_from = r.boolean(5)?;
    let is_program = r.boolean(6)?;
    let filename = r.opt_string(7)?;
    let delimiter = r.node(8)?;
    let user = r.list(10)?;
    let where_clause = r.node(11)?;
    if is_program && filename.is_none() {
        return Err(r.refuse(SYNTAX_ERROR, 8, "STDIN/STDOUT not allowed with PROGRAM"));
    }
    if !is_from && where_clause.is_some() {
        return Err(r.refuse(SYNTAX_ERROR, 11, "WHERE clause not allowed with COPY TO"));
    }
    let mut options: Vec<NodeId> = binary.into_iter().chain(delimiter).collect();
    options.extend(user);
    r.make(Node::CopyStmt {
        relation,
        query: None,
        attlist,
        is_from,
        is_program,
        filename,
        options,
        where_clause,
    })
}

/// `generic_set: var_name TO var_list` and `var_name '=' var_list`; VAR_SET_VALUE is 0.
fn generic_set(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let name = r.string(1)?;
    let args = r.list(3)?;
    let location = r.at(3)?;
    r.make(Node::VariableSetStmt {
        kind: 0,
        name: Some(name),
        args,
        jumble_args: false,
        is_local: false,
        location,
    })
}

/// `VariableSetStmt: SET set_rest`, `SET LOCAL set_rest` and `SET SESSION set_rest`: set_rest's
/// statement, `$n`, with is_local as the keyword says.
fn set_stmt(r: &mut Reduce<'_>, n: usize, local: bool) -> Result<Value, Refusal> {
    let stmt = r.node(n)?.ok_or_else(fault)?;
    let Node::VariableSetStmt { is_local, .. } = r.tree.get_mut(stmt)? else {
        return Err(fault());
    };
    *is_local = local;
    Ok(Value::Node(Some(stmt)))
}

/// A VariableSetStmt of VariableSetKind `kind` with no arguments, its location -1: generic_set's
/// DEFAULT (VAR_SET_DEFAULT, 1), generic_reset's var_name (VAR_RESET, 4) and ALL (VAR_RESET_ALL, 5).
fn set_kind(r: &mut Reduce<'_>, kind: i32, name: Option<String>) -> Result<Value, Refusal> {
    r.make(Node::VariableSetStmt {
        kind,
        name,
        args: Vec::new(),
        jumble_args: false,
        is_local: false,
        location: -1,
    })
}

/// `generic_set: var_name TO DEFAULT` and `var_name '=' DEFAULT`, or `generic_reset: var_name`.
fn named_set_kind(r: &mut Reduce<'_>, kind: i32) -> Result<Value, Refusal> {
    let name = r.string(1)?;
    set_kind(r, kind, Some(name))
}

/// `var_name: var_name '.' ColId`: psprintf("%s.%s", $1, $3).
fn dotted_name(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let head = r.string(1)?;
    let tail = r.string(3)?;
    Ok(Value::Token(lex::Value::Str(
        format!("{head}.{tail}").into_bytes(),
    )))
}

/// `NumericOnly: FCONST`, `'+' FCONST` and `'-' FCONST`: makeFloat($n), negated as doNegateFloat
/// negates it (a leading '+' dropped, then a leading '-' stripped or one put in front).
fn float_value(r: &mut Reduce<'_>, n: usize, negate: bool) -> Result<Value, Refusal> {
    let text = r.string(n)?;
    let text = if negate {
        let v = text.strip_prefix('+').unwrap_or(&text);
        v.strip_prefix('-')
            .map_or_else(|| format!("-{v}"), str::to_owned)
    } else {
        text
    };
    r.make(Node::Float(text))
}

/// `SignedIconst: '-' Iconst`: `- $2` (ICONST is at most INT_MAX, so its negation is an int).
fn negative_iconst(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let v = r.int(2)?;
    Ok(Value::Int(v.checked_neg().ok_or_else(fault)?))
}

/// `var_value: NumericOnly`: makeAConst($1, @1), the Integer's or the Float's A_Const.
fn numeric_const(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let id = r.node(1)?.ok_or_else(fault)?;
    let location = r.at(1)?;
    let val = match r.tree.get(id)? {
        Node::Integer(v) => Val::Integer(*v),
        Node::Float(s) => Val::Float(s.clone()),
        _ => return Err(fault()),
    };
    r.make(Node::AConst { val, location })
}

/// `a_expr: a_expr TYPECAST Typename`: makeTypeCast($1, $3, @2).
fn type_cast(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let arg = r.node(1)?;
    let type_name = r.node(3)?;
    let location = r.at(2)?;
    r.make(Node::TypeCast {
        arg,
        type_name,
        location,
    })
}

/// `GenericType: type_function_name opt_type_modifiers` (makeTypeName) and `type_function_name
/// attrs opt_type_modifiers` (makeTypeNameFromNameList of `lcons(makeString($1), $2)`), typmods
/// and location @1 set.
fn generic_type(r: &mut Reduce<'_>, attrs: bool) -> Result<Value, Refusal> {
    let first = r.string(1)?;
    let mut names = vec![r.push(Node::String(first))?];
    let typmods = if attrs {
        names.extend(r.list(2)?);
        r.list(3)?
    } else {
        r.list(2)?
    };
    let location = r.at(1)?;
    r.make(Node::TypeName {
        names,
        type_oid: 0,
        setof: false,
        pct_type: false,
        typmods,
        typemod: -1,
        array_bounds: Vec::new(),
        location,
    })
}

/// `attrs: '.' attr_name`: list_make1(makeString($2)).
fn attrs_first(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let name = r.string(2)?;
    Ok(Value::List(vec![r.push(Node::String(name))?]))
}

/// `attrs: attrs '.' attr_name`: lappend($1, makeString($3)).
fn attrs_append(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let mut list = r.list(1)?;
    let name = r.string(3)?;
    list.push(r.push(Node::String(name))?);
    Ok(Value::List(list))
}

/// `func_application: func_name '(' ')'`: makeFuncCall($1, NIL, COERCE_EXPLICIT_CALL, @1).
fn func_no_args(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let funcname = r.list(1)?;
    let location = r.at(1)?;
    r.make(Node::FuncCall(Box::new(FuncCall {
        funcname,
        location,
        ..FuncCall::default()
    })))
}

/// `PartitionSpec: PARTITION BY ColId '(' part_params ')'`: parsePartitionStrategy, which refuses
/// a strategy other than list, range or hash (pg_strcasecmp) at @3.
fn partition_spec(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let strategy = match r.string(3)?.to_ascii_lowercase().as_str() {
        "list" => 'l',
        "range" => 'r',
        "hash" => 'h',
        _ => {
            return Err(r.refuse(
                INVALID_PARAMETER_VALUE,
                3,
                "unrecognized partitioning strategy",
            ));
        }
    };
    let part_params = r.list(5)?;
    let location = r.at(1)?;
    r.make(Node::PartitionSpec {
        strategy,
        part_params,
        location,
    })
}

/// `part_elem: ColId opt_collate opt_qualified_name`.
fn part_elem(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let name = r.string(1)?;
    let collation = r.list(2)?;
    let opclass = r.list(3)?;
    let location = r.at(1)?;
    r.make(Node::PartitionElem {
        name: Some(name),
        expr: None,
        collation,
        opclass,
        location,
    })
}

/// `VariableShowStmt: SHOW var_name`.
fn show_stmt(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    let name = r.string(2)?;
    r.make(Node::VariableShowStmt { name: Some(name) })
}

/// `VariableShowStmt: SHOW ALL`: the name "all".
fn show_all(r: &mut Reduce<'_>) -> Result<Value, Refusal> {
    r.make(Node::VariableShowStmt {
        name: Some("all".to_owned()),
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
                sql,
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

/// scanner_yyerror's message (src/backend/parser/scan.l): the token's own text, which is what the
/// scanner holds from the token's start to where it stopped.
fn error_at(sql: &[u8], at: usize, lookahead: Option<i32>) -> Refusal {
    if lookahead == Some(END) {
        return syntax_error(sql, at, "syntax error at end of input");
    }
    let token = sql
        .get(at..)
        .and_then(|rest| rest.get(..lex::token_len(rest)))
        .map(String::from_utf8_lossy)
        .unwrap_or_default();
    syntax_error(sql, at, format!("syntax error at or near \"{token}\""))
}

/// A query string's answer: 19's tree, 19's refusal where 19 refuses, and 0A000 for a string 19's
/// grammar accepts through a rule whose action isn't ported yet.
pub fn parse(sql: &[u8]) -> Result<(Tree, Vec<NodeId>), Refusal> {
    match parse_tree(sql)? {
        Parsed::Tree { tree, statements } => Ok((tree, statements)),
        Parsed::Unported => Err(Refusal {
            sqlstate: FEATURE_NOT_SUPPORTED,
            cursor: 0,
            message: "a rule this statement needs has no ported action yet".into(),
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
        assert_eq!(n, 282);
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

    /// T-PG.5.1: a syntax error names the token as scanner_yyerror (scan.l) does, and a transaction
    /// command or a comparison ported from its rule prints as 19's does.
    #[test]
    fn t_pg_5_1_syntax_errors_name_the_token_19_names() {
        let message = |sql: &str| accepts(sql.as_bytes()).unwrap_err().message.into_owned();
        assert_eq!(
            message("SELECT FROM WHERE"),
            "syntax error at or near \"WHERE\""
        );
        assert_eq!(message("SELEC 1"), "syntax error at or near \"SELEC\"");
        assert_eq!(
            message("select 1 from 'é' where"),
            "syntax error at or near \"'é'\""
        );
        assert_eq!(message("SELECT 1 +"), "syntax error at end of input");
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
        assert_eq!(
            (e.sqlstate, e.message.as_ref()),
            ("42601", "memory exhausted")
        );
    }
}
