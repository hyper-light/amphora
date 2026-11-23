//! PostgreSQL 19's raw parse tree (parsenodes.h, primnodes.h, value.h) in an arena, and its text form
//! as the oracle prints it (`nodeToStringWithLocations`: outfuncs.c and the outfuncs.funcs.c that
//! gen_node_support.pl writes, fields in struct order). Children are arena indexes, so building,
//! printing and dropping a tree never recurse (CLAUDE.md section 11); the printer walks an explicit
//! stack. Only the nodes ported actions build are here (docs/design/postgres.md section 11's unit 6).

/// A node's place in its [`Tree`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeId(u32);

/// parsenodes.h's TransactionStmtKind, printed as its number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionStmtKind {
    Begin = 0,
    Commit = 2,
}

/// value.h's ValUnion as an A_Const holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Val {
    Integer(i32),
    /// A numeric literal's text (FCONST), printed as it is.
    Float(String),
    Boolean(bool),
    String(String),
}

/// One node of 19's raw parse tree. A `List *` field is a `Vec` (NIL is empty, printed `<>`); a
/// `char *` field is an `Option<String>` (NULL is `None`, printed `<>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// value.h's String.
    String(String),
    /// A List of nodes as an element of another list (`valuesLists`, RangeFunction's `functions`);
    /// its own elements may be NULL (`list_make2($1, NIL)`).
    List(Vec<Option<NodeId>>),
    RawStmt {
        stmt: Option<NodeId>,
        stmt_location: i32,
        stmt_len: i32,
    },
    TransactionStmt {
        kind: TransactionStmtKind,
        options: Vec<NodeId>,
        savepoint_name: Option<String>,
        gid: Option<String>,
        chain: bool,
        location: i32,
    },
    UpdateStmt {
        relation: Option<NodeId>,
        target_list: Vec<NodeId>,
        where_clause: Option<NodeId>,
        from_clause: Vec<NodeId>,
        returning_clause: Option<NodeId>,
        with_clause: Option<NodeId>,
    },
    InsertStmt {
        relation: Option<NodeId>,
        cols: Vec<NodeId>,
        select_stmt: Option<NodeId>,
        on_conflict_clause: Option<NodeId>,
        returning_clause: Option<NodeId>,
        with_clause: Option<NodeId>,
        override_kind: i32,
    },
    SelectStmt(Box<SelectStmt>),
    RangeVar {
        catalogname: Option<String>,
        schemaname: Option<String>,
        relname: String,
        inh: bool,
        relpersistence: char,
        alias: Option<NodeId>,
        location: i32,
    },
    ResTarget {
        name: Option<String>,
        indirection: Vec<NodeId>,
        val: Option<NodeId>,
        location: i32,
    },
    /// An A_Expr of kind AEXPR_OP.
    AExprOp {
        name: Vec<NodeId>,
        lexpr: Option<NodeId>,
        rexpr: Option<NodeId>,
        location: i32,
    },
    ColumnRef {
        fields: Vec<NodeId>,
        location: i32,
    },
    AConst {
        val: Val,
        location: i32,
    },
    ParamRef {
        number: i32,
        location: i32,
    },
    SqlValueFunction {
        op: i32,
        typmod: i32,
        location: i32,
    },
    Alias {
        aliasname: String,
        colnames: Vec<NodeId>,
    },
    RangeFunction {
        lateral: bool,
        ordinality: bool,
        is_rowsfrom: bool,
        functions: Vec<NodeId>,
        alias: Option<NodeId>,
        coldeflist: Vec<NodeId>,
    },
    FuncCall(Box<FuncCall>),
    JoinExpr {
        jointype: i32,
        is_natural: bool,
        larg: Option<NodeId>,
        rarg: Option<NodeId>,
        using_clause: Vec<NodeId>,
        join_using_alias: Option<NodeId>,
        quals: Option<NodeId>,
        alias: Option<NodeId>,
        rtindex: i32,
    },
    RangeSubselect {
        lateral: bool,
        subquery: Option<NodeId>,
        alias: Option<NodeId>,
    },
    /// primnodes.h's BoolExpr as the grammar makes it.
    BoolExpr {
        boolop: BoolOp,
        args: Vec<NodeId>,
        location: i32,
    },
    NullTest {
        arg: Option<NodeId>,
        nulltesttype: i32,
        argisrow: bool,
        location: i32,
    },
    SortBy {
        node: Option<NodeId>,
        sortby_dir: i32,
        sortby_nulls: i32,
        use_op: Vec<NodeId>,
        location: i32,
    },
}

/// primnodes.h's BoolExprType, printed by name as _outBoolExpr prints it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoolOp {
    And,
    Or,
    Not,
}

/// parsenodes.h's FuncCall, fields in struct order; `default()` is makeNode's zeroed node, whose
/// `funcformat` 0 is COERCE_EXPLICIT_CALL and `ignore_nulls` 0 NO_NULLTREATMENT.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FuncCall {
    pub funcname: Vec<NodeId>,
    pub args: Vec<NodeId>,
    pub agg_order: Vec<NodeId>,
    pub agg_filter: Option<NodeId>,
    pub over: Option<NodeId>,
    pub ignore_nulls: i32,
    pub agg_within_group: bool,
    pub agg_star: bool,
    pub agg_distinct: bool,
    pub func_variadic: bool,
    pub funcformat: i32,
    pub location: i32,
}

/// parsenodes.h's SelectStmt, fields in struct order; `default()` is makeNode's zeroed node.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SelectStmt {
    pub distinct_clause: Vec<NodeId>,
    pub into_clause: Option<NodeId>,
    pub target_list: Vec<NodeId>,
    pub from_clause: Vec<NodeId>,
    pub where_clause: Option<NodeId>,
    pub group_clause: Vec<NodeId>,
    pub group_distinct: bool,
    pub having_clause: Option<NodeId>,
    pub window_clause: Vec<NodeId>,
    pub values_lists: Vec<NodeId>,
    pub sort_clause: Vec<NodeId>,
    pub limit_offset: Option<NodeId>,
    pub limit_count: Option<NodeId>,
    pub limit_option: i32,
    pub locking_clause: Vec<NodeId>,
    pub with_clause: Option<NodeId>,
    pub op: i32,
    pub all: bool,
    pub larg: Option<NodeId>,
    pub rarg: Option<NodeId>,
}

/// A tree's arena: a node index that isn't in it, or an arena past `u32` nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeFault;

/// The nodes one query string's parse built.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Tree {
    nodes: Vec<Node>,
}

/// One piece of a node's text form, in the order outfuncs.c writes it.
enum Piece<'t> {
    Text(&'static str),
    /// A value's text written as it is (`_outFloat`).
    Raw(&'t str),
    Int(i64),
    /// outToken: `<>` for NULL, `""` for empty, read.c's special characters backslashed.
    Token(Option<&'t str>),
    /// outChar: `<>` for `'\0'`, else the character as outToken writes it.
    Char(char),
    /// outNode on a node pointer: `<>` for NULL.
    Node(Option<NodeId>),
    /// outNode on a List field: `<>` for NIL.
    List(&'t [NodeId]),
    /// outNode on a List whose elements may be NULL.
    Items(&'t [Option<NodeId>]),
}

impl Tree {
    pub fn push(&mut self, node: Node) -> Result<NodeId, TreeFault> {
        let id = u32::try_from(self.nodes.len()).map_err(|_| TreeFault)?;
        self.nodes.push(node);
        Ok(NodeId(id))
    }

    pub fn get(&self, id: NodeId) -> Result<&Node, TreeFault> {
        usize::try_from(id.0)
            .ok()
            .and_then(|i| self.nodes.get(i))
            .ok_or(TreeFault)
    }

    pub fn get_mut(&mut self, id: NodeId) -> Result<&mut Node, TreeFault> {
        usize::try_from(id.0)
            .ok()
            .and_then(|i| self.nodes.get_mut(i))
            .ok_or(TreeFault)
    }

    /// `nodeToStringWithLocations` of a List of nodes (raw_parser's answer): `<>` when NIL.
    pub fn to_text(&self, list: &[NodeId]) -> Result<String, TreeFault> {
        let mut out = String::new();
        let mut stack = vec![Piece::List(list)];
        while let Some(piece) = stack.pop() {
            match piece {
                Piece::Text(s) => out.push_str(s),
                Piece::Raw(s) => out.push_str(s),
                Piece::Int(n) => out.push_str(&n.to_string()),
                Piece::Token(s) => token(&mut out, s),
                Piece::Char('\0') => out.push_str("<>"),
                Piece::Char(c) => token(&mut out, Some(c.encode_utf8(&mut [0; 4]))),
                Piece::Node(None) => out.push_str("<>"),
                Piece::Node(Some(id)) => {
                    let mut pieces = Vec::new();
                    self.pieces(self.get(id)?, &mut pieces);
                    stack.extend(pieces.into_iter().rev());
                }
                Piece::List([]) | Piece::Items([]) => out.push_str("<>"),
                Piece::Items(items) => {
                    stack.push(Piece::Text(")"));
                    for (i, &item) in items.iter().enumerate().rev() {
                        stack.push(Piece::Node(item));
                        if i > 0 {
                            stack.push(Piece::Text(" "));
                        }
                    }
                    stack.push(Piece::Text("("));
                }
                Piece::List(items) => {
                    stack.push(Piece::Text(")"));
                    for (i, &item) in items.iter().enumerate().rev() {
                        stack.push(Piece::Node(Some(item)));
                        if i > 0 {
                            stack.push(Piece::Text(" "));
                        }
                    }
                    stack.push(Piece::Text("("));
                }
            }
        }
        Ok(out)
    }

    /// A node's pieces, field by field as outfuncs writes them (WRITE_*_FIELD: ` :name value`).
    fn pieces<'t>(&'t self, node: &'t Node, p: &mut Vec<Piece<'t>>) {
        use Piece::{Char, Int, Items, List, Node as N, Raw, Text, Token};
        let truth = |b: bool| if b { "true" } else { "false" };
        let location = |p: &mut Vec<Piece<'t>>, name: &'static str, at: i32| {
            p.push(Text(name));
            p.push(Int(i64::from(at)));
        };
        match node {
            Node::String(s) => {
                p.push(Text("\""));
                if !s.is_empty() {
                    p.push(Token(Some(s)));
                }
                p.push(Text("\""));
            }
            Node::List(items) => p.push(Items(items)),
            Node::RawStmt {
                stmt,
                stmt_location,
                stmt_len,
            } => {
                p.extend([Text("{RAWSTMT :stmt "), N(*stmt)]);
                location(p, " :stmt_location ", *stmt_location);
                location(p, " :stmt_len ", *stmt_len);
                p.push(Text("}"));
            }
            Node::TransactionStmt {
                kind,
                options,
                savepoint_name,
                gid,
                chain,
                location: at,
            } => {
                p.extend([
                    Text("{TRANSACTIONSTMT :kind "),
                    Int(*kind as i64),
                    Text(" :options "),
                    List(options),
                    Text(" :savepoint_name "),
                    Token(savepoint_name.as_deref()),
                    Text(" :gid "),
                    Token(gid.as_deref()),
                    Text(if *chain {
                        " :chain true"
                    } else {
                        " :chain false"
                    }),
                ]);
                location(p, " :location ", *at);
                p.push(Text("}"));
            }
            Node::UpdateStmt {
                relation,
                target_list,
                where_clause,
                from_clause,
                returning_clause,
                with_clause,
            } => p.extend([
                Text("{UPDATESTMT :relation "),
                N(*relation),
                Text(" :targetList "),
                List(target_list),
                Text(" :whereClause "),
                N(*where_clause),
                Text(" :fromClause "),
                List(from_clause),
                Text(" :returningClause "),
                N(*returning_clause),
                Text(" :withClause "),
                N(*with_clause),
                Text("}"),
            ]),
            Node::InsertStmt {
                relation,
                cols,
                select_stmt,
                on_conflict_clause,
                returning_clause,
                with_clause,
                override_kind,
            } => p.extend([
                Text("{INSERTSTMT :relation "),
                N(*relation),
                Text(" :cols "),
                List(cols),
                Text(" :selectStmt "),
                N(*select_stmt),
                Text(" :onConflictClause "),
                N(*on_conflict_clause),
                Text(" :returningClause "),
                N(*returning_clause),
                Text(" :withClause "),
                N(*with_clause),
                Text(" :override "),
                Int(i64::from(*override_kind)),
                Text("}"),
            ]),
            Node::SelectStmt(n) => {
                p.extend([
                    Text("{SELECTSTMT :distinctClause "),
                    List(&n.distinct_clause),
                    Text(" :intoClause "),
                    N(n.into_clause),
                    Text(" :targetList "),
                    List(&n.target_list),
                    Text(" :fromClause "),
                    List(&n.from_clause),
                    Text(" :whereClause "),
                    N(n.where_clause),
                    Text(" :groupClause "),
                    List(&n.group_clause),
                    Text(" :groupDistinct "),
                    Text(truth(n.group_distinct)),
                    Text(" :havingClause "),
                    N(n.having_clause),
                    Text(" :windowClause "),
                    List(&n.window_clause),
                    Text(" :valuesLists "),
                    List(&n.values_lists),
                    Text(" :sortClause "),
                    List(&n.sort_clause),
                    Text(" :limitOffset "),
                    N(n.limit_offset),
                    Text(" :limitCount "),
                    N(n.limit_count),
                    Text(" :limitOption "),
                    Int(i64::from(n.limit_option)),
                    Text(" :lockingClause "),
                    List(&n.locking_clause),
                    Text(" :withClause "),
                    N(n.with_clause),
                    Text(" :op "),
                    Int(i64::from(n.op)),
                    Text(" :all "),
                    Text(truth(n.all)),
                    Text(" :larg "),
                    N(n.larg),
                    Text(" :rarg "),
                    N(n.rarg),
                    Text("}"),
                ]);
            }
            Node::RangeVar {
                catalogname,
                schemaname,
                relname,
                inh,
                relpersistence,
                alias,
                location: at,
            } => {
                p.extend([
                    Text("{RANGEVAR :catalogname "),
                    Token(catalogname.as_deref()),
                    Text(" :schemaname "),
                    Token(schemaname.as_deref()),
                    Text(" :relname "),
                    Token(Some(relname)),
                    Text(if *inh { " :inh true" } else { " :inh false" }),
                    Text(" :relpersistence "),
                    Char(*relpersistence),
                    Text(" :alias "),
                    N(*alias),
                ]);
                location(p, " :location ", *at);
                p.push(Text("}"));
            }
            Node::ResTarget {
                name,
                indirection,
                val,
                location: at,
            } => {
                p.extend([
                    Text("{RESTARGET :name "),
                    Token(name.as_deref()),
                    Text(" :indirection "),
                    List(indirection),
                    Text(" :val "),
                    N(*val),
                ]);
                location(p, " :location ", *at);
                p.push(Text("}"));
            }
            Node::AExprOp {
                name,
                lexpr,
                rexpr,
                location: at,
            } => {
                p.extend([
                    Text("{A_EXPR :name "),
                    List(name),
                    Text(" :lexpr "),
                    N(*lexpr),
                    Text(" :rexpr "),
                    N(*rexpr),
                    Text(" :rexpr_list_start 0 :rexpr_list_end 0"),
                ]);
                location(p, " :location ", *at);
                p.push(Text("}"));
            }
            Node::ColumnRef {
                fields,
                location: at,
            } => {
                p.extend([Text("{COLUMNREF :fields "), List(fields)]);
                location(p, " :location ", *at);
                p.push(Text("}"));
            }
            Node::AConst { val, location: at } => {
                p.push(Text("{A_CONST :val "));
                match val {
                    Val::Integer(n) => p.push(Int(i64::from(*n))),
                    Val::Float(s) => p.push(Raw(s)),
                    Val::Boolean(b) => p.push(Text(truth(*b))),
                    Val::String(s) => {
                        p.push(Text("\""));
                        if !s.is_empty() {
                            p.push(Token(Some(s)));
                        }
                        p.push(Text("\""));
                    }
                }
                location(p, " :location ", *at);
                p.push(Text("}"));
            }
            Node::ParamRef {
                number,
                location: at,
            } => {
                p.extend([Text("{PARAMREF :number "), Int(i64::from(*number))]);
                location(p, " :location ", *at);
                p.push(Text("}"));
            }
            Node::SqlValueFunction {
                op,
                typmod,
                location: at,
            } => {
                p.extend([
                    Text("{SQLVALUEFUNCTION :op "),
                    Int(i64::from(*op)),
                    Text(" :type 0 :typmod "),
                    Int(i64::from(*typmod)),
                ]);
                location(p, " :location ", *at);
                p.push(Text("}"));
            }
            Node::Alias {
                aliasname,
                colnames,
            } => p.extend([
                Text("{ALIAS :aliasname "),
                Token(Some(aliasname)),
                Text(" :colnames "),
                List(colnames),
                Text("}"),
            ]),
            Node::RangeFunction {
                lateral,
                ordinality,
                is_rowsfrom,
                functions,
                alias,
                coldeflist,
            } => p.extend([
                Text("{RANGEFUNCTION :lateral "),
                Text(truth(*lateral)),
                Text(" :ordinality "),
                Text(truth(*ordinality)),
                Text(" :is_rowsfrom "),
                Text(truth(*is_rowsfrom)),
                Text(" :functions "),
                List(functions),
                Text(" :alias "),
                N(*alias),
                Text(" :coldeflist "),
                List(coldeflist),
                Text("}"),
            ]),
            Node::JoinExpr {
                jointype,
                is_natural,
                larg,
                rarg,
                using_clause,
                join_using_alias,
                quals,
                alias,
                rtindex,
            } => p.extend([
                Text("{JOINEXPR :jointype "),
                Int(i64::from(*jointype)),
                Text(" :isNatural "),
                Text(truth(*is_natural)),
                Text(" :larg "),
                N(*larg),
                Text(" :rarg "),
                N(*rarg),
                Text(" :usingClause "),
                List(using_clause),
                Text(" :join_using_alias "),
                N(*join_using_alias),
                Text(" :quals "),
                N(*quals),
                Text(" :alias "),
                N(*alias),
                Text(" :rtindex "),
                Int(i64::from(*rtindex)),
                Text("}"),
            ]),
            Node::RangeSubselect {
                lateral,
                subquery,
                alias,
            } => p.extend([
                Text("{RANGESUBSELECT :lateral "),
                Text(truth(*lateral)),
                Text(" :subquery "),
                N(*subquery),
                Text(" :alias "),
                N(*alias),
                Text("}"),
            ]),
            Node::BoolExpr {
                boolop,
                args,
                location: at,
            } => {
                p.extend([
                    Text(match boolop {
                        BoolOp::And => "{BOOLEXPR :boolop and :args ",
                        BoolOp::Or => "{BOOLEXPR :boolop or :args ",
                        BoolOp::Not => "{BOOLEXPR :boolop not :args ",
                    }),
                    List(args),
                ]);
                location(p, " :location ", *at);
                p.push(Text("}"));
            }
            Node::NullTest {
                arg,
                nulltesttype,
                argisrow,
                location: at,
            } => {
                p.extend([
                    Text("{NULLTEST :arg "),
                    N(*arg),
                    Text(" :nulltesttype "),
                    Int(i64::from(*nulltesttype)),
                    Text(" :argisrow "),
                    Text(truth(*argisrow)),
                ]);
                location(p, " :location ", *at);
                p.push(Text("}"));
            }
            Node::SortBy {
                node,
                sortby_dir,
                sortby_nulls,
                use_op,
                location: at,
            } => {
                p.extend([
                    Text("{SORTBY :node "),
                    N(*node),
                    Text(" :sortby_dir "),
                    Int(i64::from(*sortby_dir)),
                    Text(" :sortby_nulls "),
                    Int(i64::from(*sortby_nulls)),
                    Text(" :useOp "),
                    List(use_op),
                ]);
                location(p, " :location ", *at);
                p.push(Text("}"));
            }
            Node::FuncCall(n) => {
                p.extend([
                    Text("{FUNCCALL :funcname "),
                    List(&n.funcname),
                    Text(" :args "),
                    List(&n.args),
                    Text(" :agg_order "),
                    List(&n.agg_order),
                    Text(" :agg_filter "),
                    N(n.agg_filter),
                    Text(" :over "),
                    N(n.over),
                    Text(" :ignore_nulls "),
                    Int(i64::from(n.ignore_nulls)),
                    Text(" :agg_within_group "),
                    Text(truth(n.agg_within_group)),
                    Text(" :agg_star "),
                    Text(truth(n.agg_star)),
                    Text(" :agg_distinct "),
                    Text(truth(n.agg_distinct)),
                    Text(" :func_variadic "),
                    Text(truth(n.func_variadic)),
                    Text(" :funcformat "),
                    Int(i64::from(n.funcformat)),
                ]);
                location(p, " :location ", n.location);
                p.push(Text("}"));
            }
        }
    }
}

/// outfuncs.c's outToken.
fn token(out: &mut String, s: Option<&str>) {
    let Some(s) = s else {
        out.push_str("<>");
        return;
    };
    let mut chars = s.chars();
    let (first, second) = (chars.next(), chars.next());
    match first {
        None => {
            out.push_str("\"\"");
            return;
        }
        Some('<' | '"') => out.push('\\'),
        Some(c) if c.is_ascii_digit() => out.push('\\'),
        Some('+' | '-') if second.is_some_and(|c| c.is_ascii_digit() || c == '.') => out.push('\\'),
        Some(_) => {}
    }
    for c in s.chars() {
        if matches!(c, ' ' | '\n' | '\t' | '(' | ')' | '{' | '}' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
}

#[cfg(test)]
mod tests {
    use super::{Node, NodeId, TransactionStmtKind, Tree};

    fn string(t: &mut Tree, s: &str) -> NodeId {
        t.push(Node::String(s.into())).unwrap()
    }

    /// T-PG.6.2: BEGIN's tree prints as the oracle printed it for `BEGIN` (lane PG6's capture).
    #[test]
    fn t_pg_6_2_begin_prints_as_19_prints_it() {
        let mut t = Tree::default();
        let stmt = t
            .push(Node::TransactionStmt {
                kind: TransactionStmtKind::Begin,
                options: vec![],
                savepoint_name: None,
                gid: None,
                chain: false,
                location: -1,
            })
            .unwrap();
        let raw = t
            .push(Node::RawStmt {
                stmt: Some(stmt),
                stmt_location: 0,
                stmt_len: 0,
            })
            .unwrap();
        assert_eq!(
            t.to_text(&[raw]).unwrap(),
            "({RAWSTMT :stmt {TRANSACTIONSTMT :kind 0 :options <> :savepoint_name <> :gid <> :chain false :location -1} :stmt_location 0 :stmt_len 0})"
        );
        assert_eq!(t.to_text(&[]).unwrap(), "<>");
    }

    /// T-PG.6.2: outToken backslashes what read.c treats specially, as outfuncs.c does.
    #[test]
    fn t_pg_6_2_strings_escape_as_out_token_escapes_them() {
        let mut t = Tree::default();
        let ids: Vec<_> = ["a b", "1x", "-1", "-x", "<a", "", "(}\\"]
            .into_iter()
            .map(|s| string(&mut t, s))
            .collect();
        assert_eq!(
            t.to_text(&ids).unwrap(),
            "(\"a\\ b\" \"\\1x\" \"\\-1\" \"-x\" \"\\<a\" \"\" \"\\(\\}\\\\\")"
        );
    }
}
