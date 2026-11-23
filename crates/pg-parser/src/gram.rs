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
use crate::tree::{Node, NodeId, TransactionStmtKind, Tree, TreeFault};

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

/// The rule's `$$`: bison's default (`$$ = $1`, before any action) for a rule with no action, the
/// ported action's otherwise, and [`Value::Unported`] when the action isn't ported (or was ported
/// from a text that has since changed) or a value it reads came from such a rule.
fn act(rule: i32, mut r: Reduce<'_>) -> Result<Value, Refusal> {
    if r.rhs.iter().any(|v| *v == Value::Unported) {
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
                Some(statements) if values.iter().all(|v| *v != Value::Unported) => {
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
            "SELECT 1",
            "select a, b from t where a not in (1, 2) order by 1 nulls first",
            "BEGIN; UPDATE pgbench_accounts SET abalance = abalance + 1 WHERE aid = $1; END;",
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
                assert_eq!(ACTION_HASH[rule as usize], Some(from), "rule {rule}");
                n += 1;
            }
        }
        assert_eq!(n, 10);
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
