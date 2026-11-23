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
    String(String),
}

/// One node of 19's raw parse tree. A `List *` field is a `Vec` (NIL is empty, printed `<>`); a
/// `char *` field is an `Option<String>` (NULL is `None`, printed `<>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// value.h's String.
    String(String),
    /// A List of nodes, as an element of another list (`valuesLists`).
    List(Vec<NodeId>),
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
    /// SelectStmt as a simple SELECT or VALUES builds it (`op` SETOP_NONE, no set operands).
    SelectStmt {
        target_list: Vec<NodeId>,
        from_clause: Vec<NodeId>,
        where_clause: Option<NodeId>,
        values_lists: Vec<NodeId>,
    },
    RangeVar {
        relname: Option<String>,
        location: i32,
    },
    ResTarget {
        name: Option<String>,
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
    Int(i64),
    /// outToken: `<>` for NULL, `""` for empty, read.c's special characters backslashed.
    Token(Option<&'t str>),
    /// outNode on a node pointer: `<>` for NULL.
    Node(Option<NodeId>),
    /// outNode on a List field: `<>` for NIL.
    List(&'t [NodeId]),
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
                Piece::Int(n) => out.push_str(&n.to_string()),
                Piece::Token(s) => token(&mut out, s),
                Piece::Node(None) => out.push_str("<>"),
                Piece::Node(Some(id)) => {
                    let mut pieces = Vec::new();
                    self.pieces(self.get(id)?, &mut pieces);
                    stack.extend(pieces.into_iter().rev());
                }
                Piece::List([]) => out.push_str("<>"),
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
        use Piece::{Int, List, Node as N, Text, Token};
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
            Node::List(items) => p.push(List(items)),
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
                    Text(if *chain { " :chain true" } else { " :chain false" }),
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
            Node::SelectStmt {
                target_list,
                from_clause,
                where_clause,
                values_lists,
            } => p.extend([
                Text("{SELECTSTMT :distinctClause <> :intoClause <> :targetList "),
                List(target_list),
                Text(" :fromClause "),
                List(from_clause),
                Text(" :whereClause "),
                N(*where_clause),
                Text(" :groupClause <> :groupDistinct false :havingClause <> :windowClause <> :valuesLists "),
                List(values_lists),
                Text(" :sortClause <> :limitOffset <> :limitCount <> :limitOption 0 :lockingClause <> :withClause <> :op 0 :all false :larg <> :rarg <>}"),
            ]),
            Node::RangeVar {
                relname,
                location: at,
            } => {
                p.extend([
                    Text("{RANGEVAR :catalogname <> :schemaname <> :relname "),
                    Token(relname.as_deref()),
                    Text(" :inh true :relpersistence p :alias <>"),
                ]);
                location(p, " :location ", *at);
                p.push(Text("}"));
            }
            Node::ResTarget {
                name,
                val,
                location: at,
            } => {
                p.extend([
                    Text("{RESTARGET :name "),
                    Token(name.as_deref()),
                    Text(" :indirection <> :val "),
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
    use super::{Node, NodeId, TransactionStmtKind, Tree, Val};

    fn string(t: &mut Tree, s: &str) -> NodeId {
        t.push(Node::String(s.into())).unwrap()
    }

    fn column(t: &mut Tree, name: &str, location: i32) -> NodeId {
        let field = string(t, name);
        t.push(Node::ColumnRef {
            fields: vec![field],
            location,
        })
        .unwrap()
    }

    fn op(t: &mut Tree, name: &str, lexpr: NodeId, rexpr: NodeId, location: i32) -> NodeId {
        let name = string(t, name);
        t.push(Node::AExprOp {
            name: vec![name],
            lexpr: Some(lexpr),
            rexpr: Some(rexpr),
            location,
        })
        .unwrap()
    }

    fn int(t: &mut Tree, n: i32, location: i32) -> NodeId {
        t.push(Node::AConst {
            val: Val::Integer(n),
            location,
        })
        .unwrap()
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

    /// T-PG.6.2: pgbench's TPC-B UPDATE (simple protocol, a negative delta folded into its constant)
    /// prints byte for byte as the oracle printed it on adacachy2 (lane PG7).
    #[test]
    fn t_pg_6_2_pgbench_update_prints_as_19_prints_it() {
        let mut t = Tree::default();
        let relation = t
            .push(Node::RangeVar {
                relname: Some("pgbench_accounts".into()),
                location: 7,
            })
            .unwrap();
        let l = column(&mut t, "abalance", 39);
        let r = int(&mut t, -2741, 50);
        let sum = op(&mut t, "+", l, r, 48);
        let target = t
            .push(Node::ResTarget {
                name: Some("abalance".into()),
                val: Some(sum),
                location: 28,
            })
            .unwrap();
        let l = column(&mut t, "aid", 62);
        let r = int(&mut t, 34521, 68);
        let filter = op(&mut t, "=", l, r, 66);
        let stmt = t
            .push(Node::UpdateStmt {
                relation: Some(relation),
                target_list: vec![target],
                where_clause: Some(filter),
                from_clause: vec![],
                returning_clause: None,
                with_clause: None,
            })
            .unwrap();
        let raw = t
            .push(Node::RawStmt {
                stmt: Some(stmt),
                stmt_location: 0,
                stmt_len: 73,
            })
            .unwrap();
        assert_eq!(
            t.to_text(&[raw]).unwrap(),
            "({RAWSTMT :stmt {UPDATESTMT :relation {RANGEVAR :catalogname <> :schemaname <> :relname pgbench_accounts :inh true :relpersistence p :alias <> :location 7} :targetList ({RESTARGET :name abalance :indirection <> :val {A_EXPR :name (\"+\") :lexpr {COLUMNREF :fields (\"abalance\") :location 39} :rexpr {A_CONST :val -2741 :location 50} :rexpr_list_start 0 :rexpr_list_end 0 :location 48} :location 28}) :whereClause {A_EXPR :name (\"=\") :lexpr {COLUMNREF :fields (\"aid\") :location 62} :rexpr {A_CONST :val 34521 :location 68} :rexpr_list_start 0 :rexpr_list_end 0 :location 66} :fromClause <> :returningClause <> :withClause <>} :stmt_location 0 :stmt_len 73})"
        );
    }

    /// T-PG.6.2: pgbench's TPC-B INSERT (extended protocol): VALUES as a list of lists, $n as
    /// PARAMREF, CURRENT_TIMESTAMP as SQLVALUEFUNCTION op 3, as the oracle printed it (lane PG7).
    #[test]
    fn t_pg_6_2_pgbench_insert_prints_as_19_prints_it() {
        let mut t = Tree::default();
        let relation = t
            .push(Node::RangeVar {
                relname: Some("pgbench_history".into()),
                location: 12,
            })
            .unwrap();
        let mut cols = vec![];
        for (name, at) in [
            ("tid", 29),
            ("bid", 34),
            ("aid", 39),
            ("delta", 44),
            ("mtime", 51),
        ] {
            cols.push(
                t.push(Node::ResTarget {
                    name: Some(name.into()),
                    val: None,
                    location: at,
                })
                .unwrap(),
            );
        }
        let mut row = vec![];
        for (number, at) in [(1, 66), (2, 70), (3, 74), (4, 78)] {
            row.push(
                t.push(Node::ParamRef {
                    number,
                    location: at,
                })
                .unwrap(),
            );
        }
        row.push(
            t.push(Node::SqlValueFunction {
                op: 3,
                typmod: -1,
                location: 82,
            })
            .unwrap(),
        );
        let row = t.push(Node::List(row)).unwrap();
        let select = t
            .push(Node::SelectStmt {
                target_list: vec![],
                from_clause: vec![],
                where_clause: None,
                values_lists: vec![row],
            })
            .unwrap();
        let stmt = t
            .push(Node::InsertStmt {
                relation: Some(relation),
                cols,
                select_stmt: Some(select),
                on_conflict_clause: None,
                returning_clause: None,
                with_clause: None,
                override_kind: 0,
            })
            .unwrap();
        let raw = t
            .push(Node::RawStmt {
                stmt: Some(stmt),
                stmt_location: 0,
                stmt_len: 100,
            })
            .unwrap();
        assert_eq!(
            t.to_text(&[raw]).unwrap(),
            "({RAWSTMT :stmt {INSERTSTMT :relation {RANGEVAR :catalogname <> :schemaname <> :relname pgbench_history :inh true :relpersistence p :alias <> :location 12} :cols ({RESTARGET :name tid :indirection <> :val <> :location 29} {RESTARGET :name bid :indirection <> :val <> :location 34} {RESTARGET :name aid :indirection <> :val <> :location 39} {RESTARGET :name delta :indirection <> :val <> :location 44} {RESTARGET :name mtime :indirection <> :val <> :location 51}) :selectStmt {SELECTSTMT :distinctClause <> :intoClause <> :targetList <> :fromClause <> :whereClause <> :groupClause <> :groupDistinct false :havingClause <> :windowClause <> :valuesLists (({PARAMREF :number 1 :location 66} {PARAMREF :number 2 :location 70} {PARAMREF :number 3 :location 74} {PARAMREF :number 4 :location 78} {SQLVALUEFUNCTION :op 3 :type 0 :typmod -1 :location 82})) :sortClause <> :limitOffset <> :limitCount <> :limitOption 0 :lockingClause <> :withClause <> :op 0 :all false :larg <> :rarg <>} :onConflictClause <> :returningClause <> :withClause <> :override 0} :stmt_location 0 :stmt_len 100})"
        );
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
