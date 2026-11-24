//! Statements: `analyze.c`, `parse_target.c` and `parse_relation.c` (REL_19_0 src/backend/parser) for
//! SELECT over one table with a WHERE, UPDATE of a column, and INSERT of a VALUES row. Every other
//! clause refuses `0A000`.

use amphora_pg_catalog::Kind;
use amphora_pg_catalog::data::type_oid;
use amphora_pg_parser::tree::{Node, NodeId, SelectStmt, Tree};

use crate::expr::{Analysis, Cast, ExprId, ExprKind, Exprs, Table};
use crate::{Error, Session, format_type, sqlstate};

/// What a statement does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Select,
    Insert,
    Update,
}

/// A target list entry (TargetEntry): a SELECT's column, or an INSERT's or UPDATE's column and the
/// value assigned to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub name: String,
    /// The assigned column's `attnum`; 0 for a SELECT's.
    pub attnum: usize,
    pub expr: ExprId,
}

/// A typed statement (Query).
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub command: Command,
    pub exprs: Exprs,
    /// The table selected from or assigned to.
    pub table: Option<Table>,
    pub targets: Vec<Target>,
    pub filter: Option<ExprId>,
    /// `$n`'s types, `$1` first.
    pub params: Vec<u32>,
}

/// `parse_analyze_varparams` (analyze.c): a raw statement's typed one. `params` are the types its
/// parameters were given, 0 for one left to be deduced.
pub fn analyze(
    tree: &Tree,
    statement: NodeId,
    session: &Session<'_>,
    params: &[u32],
) -> Result<Query, Error> {
    let mut a = Analysis {
        tree,
        session,
        exprs: Exprs::default(),
        table: None,
        params: params.to_vec(),
    };
    let Node::RawStmt {
        stmt: Some(stmt), ..
    } = tree.get(statement)?
    else {
        return Err(Error::unsupported("an empty statement"));
    };
    let (command, targets, filter) = match tree.get(*stmt)? {
        Node::SelectStmt(select) => a.select(select)?,
        Node::InsertStmt {
            relation,
            cols,
            select_stmt,
            on_conflict_clause: None,
            returning_clause: None,
            with_clause: None,
            override_kind: 0,
        } => a.insert(*relation, cols, *select_stmt)?,
        Node::UpdateStmt {
            relation,
            target_list,
            where_clause,
            from_clause,
            returning_clause: None,
            with_clause: None,
        } if from_clause.is_empty() => a.update(*relation, target_list, *where_clause)?,
        _ => return Err(Error::unsupported("this statement")),
    };
    a.check_params()?;
    Ok(Query {
        command,
        exprs: a.exprs,
        table: a.table,
        targets,
        filter,
        params: a.params,
    })
}

type Analyzed = (Command, Vec<Target>, Option<ExprId>);

impl Analysis<'_> {
    /// `transformSelectStmt` (analyze.c).
    fn select(&mut self, s: &SelectStmt) -> Result<Analyzed, Error> {
        let plain = SelectStmt {
            target_list: s.target_list.clone(),
            from_clause: s.from_clause.clone(),
            where_clause: s.where_clause,
            limit_option: s.limit_option,
            ..SelectStmt::default()
        };
        if plain != *s {
            return Err(Error::unsupported("a clause of this SELECT"));
        }
        self.table = match s.from_clause.as_slice() {
            [] => None,
            [item] => Some(self.table(*item)?),
            _ => return Err(Error::unsupported("a FROM of several items")),
        };
        let mut targets = Vec::with_capacity(s.target_list.len());
        for id in &s.target_list {
            let Node::ResTarget {
                name,
                indirection,
                val: Some(val),
                ..
            } = self.tree.get(*id)?
            else {
                return Err(Error::unsupported("this target"));
            };
            if !indirection.is_empty() {
                return Err(Error::unsupported("this target"));
            }
            let expr = self.expression(*val)?;
            let name = match name {
                Some(name) => name.clone(),
                None => self.column_name(*val)?,
            };
            targets.push(Target {
                name,
                attnum: 0,
                expr,
            });
        }
        let filter = self.filter(s.where_clause)?;
        // resolveTargetListUnknowns (parse_target.c:288).
        for target in &mut targets {
            if self.exprs.get(target.expr)?.type_oid == type_oid::UNKNOWN {
                target.expr = self.coerce_type(target.expr, Cast::implicit(type_oid::TEXT))?;
            }
        }
        Ok((Command::Select, targets, filter))
    }

    /// `transformWhereClause` (parse_clause.c).
    fn filter(&mut self, clause: Option<NodeId>) -> Result<Option<ExprId>, Error> {
        match clause {
            None => Ok(None),
            Some(id) => {
                let filter = self.expression(id)?;
                Ok(Some(self.coerce_to_boolean(filter, "WHERE")?))
            }
        }
    }

    /// `FigureColname` (parse_target.c:1705): a column reference's, call's or CURRENT_TIMESTAMP's
    /// name, else the type of the outermost cast, else `?column?`.
    fn column_name(&self, root: NodeId) -> Result<String, Error> {
        let mut id = root;
        let mut cast = None;
        loop {
            let name = match self.tree.get(id)? {
                Node::TypeCast {
                    arg: Some(arg),
                    type_name: Some(type_name),
                    ..
                } => {
                    if cast.is_none()
                        && let Node::TypeName { names, .. } = self.tree.get(*type_name)?
                    {
                        cast = self.strings(names)?.pop();
                    }
                    id = *arg;
                    continue;
                }
                Node::ColumnRef { fields, .. } => self.strings(fields)?.pop(),
                Node::FuncCall(f) => self.strings(&f.funcname)?.pop(),
                Node::SqlValueFunction { op: 3, .. } => Some("current_timestamp".to_owned()),
                _ => None,
            };
            return Ok(name.or(cast).unwrap_or_else(|| "?column?".to_owned()));
        }
    }

    /// `parserOpenTable` (parse_relation.c) and the table's columns.
    fn table(&self, id: NodeId) -> Result<Table, Error> {
        let Node::RangeVar {
            catalogname,
            schemaname,
            relname,
            alias,
            location,
            ..
        } = self.tree.get(id)?
        else {
            return Err(Error::unsupported("this FROM item"));
        };
        if let Some(catalog) = catalogname {
            return Err(Error::unsupported(&format!("a name in database {catalog}")).at(*location));
        }
        let relations = self.session.relations;
        let found = match schemaname {
            Some(schema) => relations
                .namespace(schema)
                .and_then(|ns| relations.relation(ns, relname)),
            None => self
                .session
                .path
                .resolve(Kind::Relation, |ns| relations.relation(ns, relname)),
        };
        let Some(oid) = found else {
            let name = schemaname
                .as_ref()
                .map_or_else(|| relname.clone(), |s| format!("{s}.{relname}"));
            return Err(Error::new(
                sqlstate::UNDEFINED_TABLE,
                format!("relation \"{name}\" does not exist"),
            )
            .at(*location));
        };
        let refname = match alias.map(|id| self.tree.get(id)).transpose()? {
            None => relname.clone(),
            Some(Node::Alias {
                aliasname,
                colnames,
            }) if colnames.is_empty() => aliasname.clone(),
            Some(_) => return Err(Error::unsupported("an alias with column names")),
        };
        Ok(Table {
            oid,
            refname,
            columns: relations.columns(oid),
        })
    }

    /// `transformUpdateStmt` (analyze.c): the WHERE clause first, then the assignment.
    fn update(
        &mut self,
        relation: Option<NodeId>,
        target_list: &[NodeId],
        where_clause: Option<NodeId>,
    ) -> Result<Analyzed, Error> {
        let table = self.table(relation.ok_or_else(|| Error::unsupported("this UPDATE"))?)?;
        self.table = Some(table.clone());
        let filter = self.filter(where_clause)?;
        let [target] = target_list else {
            return Err(Error::unsupported("an UPDATE of several columns"));
        };
        let Node::ResTarget {
            name: Some(name),
            indirection,
            val: Some(val),
            location,
        } = self.tree.get(*target)?
        else {
            return Err(Error::unsupported("this assignment"));
        };
        if !indirection.is_empty() {
            return Err(Error::unsupported("this assignment"));
        }
        let source = self.expression(*val)?;
        let attnum =
            attnum(&table, name).ok_or_else(|| undefined_column(&table, name).at(*location))?;
        let target = self.assigned(&table, attnum, source)?;
        Ok((Command::Update, vec![target], filter))
    }

    /// `transformInsertStmt` (analyze.c) for one VALUES row under a list of columns.
    fn insert(
        &mut self,
        relation: Option<NodeId>,
        cols: &[NodeId],
        select: Option<NodeId>,
    ) -> Result<Analyzed, Error> {
        let table = self.table(relation.ok_or_else(|| Error::unsupported("this INSERT"))?)?;
        let attnums = self.insert_targets(&table, cols)?;
        let Some(Node::SelectStmt(select)) = select.map(|id| self.tree.get(id)).transpose()? else {
            return Err(Error::unsupported("INSERT without VALUES"));
        };
        let plain = SelectStmt {
            values_lists: select.values_lists.clone(),
            limit_option: select.limit_option,
            ..SelectStmt::default()
        };
        let [values] = select.values_lists.as_slice() else {
            return Err(Error::unsupported("INSERT of several rows"));
        };
        if plain != **select {
            return Err(Error::unsupported("INSERT of a SELECT"));
        }
        let Node::List(items) = self.tree.get(*values)? else {
            return Err(Error::unsupported("these VALUES"));
        };
        // A VALUES row can't name the target's columns: there is no table in scope yet.
        let mut values = Vec::with_capacity(items.len());
        for item in items.iter().flatten() {
            values.push(self.expression(*item)?);
        }
        // transformInsertRow.
        if let Some(extra) = values.get(attnums.len()) {
            let location = self.exprs.get(*extra)?.location;
            return Err(Error::new(
                sqlstate::SYNTAX_ERROR,
                "INSERT has more expressions than target columns".to_owned(),
            )
            .at(location));
        }
        if values.len() < attnums.len() {
            let location = match cols.get(values.len()).map(|id| self.tree.get(*id)) {
                Some(Ok(Node::ResTarget { location, .. })) => *location,
                _ => -1,
            };
            return Err(Error::new(
                sqlstate::SYNTAX_ERROR,
                "INSERT has more target columns than expressions".to_owned(),
            )
            .at(location));
        }
        let mut targets = Vec::with_capacity(values.len());
        for (attnum, expr) in attnums.iter().zip(values) {
            targets.push(self.assigned(&table, *attnum, expr)?);
        }
        self.table = Some(table);
        Ok((Command::Insert, targets, None))
    }

    /// `checkInsertTargets` (parse_target.c:1017): the columns an INSERT names.
    fn insert_targets(&self, table: &Table, cols: &[NodeId]) -> Result<Vec<usize>, Error> {
        if cols.is_empty() {
            return Err(Error::unsupported("INSERT without its columns"));
        }
        let mut attnums = Vec::with_capacity(cols.len());
        for id in cols {
            let Node::ResTarget {
                name: Some(name),
                indirection,
                location,
                ..
            } = self.tree.get(*id)?
            else {
                return Err(Error::unsupported("this column"));
            };
            if !indirection.is_empty() {
                return Err(Error::unsupported("this column"));
            }
            let attnum =
                attnum(table, name).ok_or_else(|| undefined_column(table, name).at(*location))?;
            if attnums.contains(&attnum) {
                return Err(Error::new(
                    sqlstate::DUPLICATE_COLUMN,
                    format!("column \"{name}\" specified more than once"),
                )
                .at(*location));
            }
            attnums.push(attnum);
        }
        Ok(attnums)
    }

    /// `transformAssignedExpr` (parse_target.c:454): the value cast to its column's type and type
    /// modifier as an assignment; 42804 when there is no such cast.
    fn assigned(&mut self, table: &Table, attnum: usize, expr: ExprId) -> Result<Target, Error> {
        let column = attnum
            .checked_sub(1)
            .and_then(|i| table.columns.get(i))
            .ok_or_else(|| Error::unsupported("this column"))?;
        let e = self.exprs.get(expr)?;
        let (type_oid, location) = (e.type_oid, e.location);
        let cast = Cast::assignment(column.type_oid, column.typmod);
        let cast = self.coerce_to_target(expr, cast)?;
        let expr = cast.ok_or_else(|| {
            Error::new(
                sqlstate::DATATYPE_MISMATCH,
                format!(
                    "column \"{}\" is of type {} but expression is of type {}",
                    column.name,
                    format_type(column.type_oid),
                    format_type(type_oid)
                ),
            )
            .at(location)
        })?;
        Ok(Target {
            name: column.name.clone(),
            attnum,
            expr,
        })
    }

    /// `check_variable_parameters` (parse_param.c), then the parameters no use gave a type
    /// (`PrepareQuery`, src/backend/commands/prepare.c; `exec_parse_message`, postgres.c).
    fn check_params(&self) -> Result<(), Error> {
        for e in self.exprs.iter() {
            if let ExprKind::Param(number) = e.kind {
                let deduced = usize::try_from(number)
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .and_then(|i| self.params.get(i));
                if deduced.is_some_and(|t| *t != e.type_oid) {
                    return Err(Error::new(
                        sqlstate::AMBIGUOUS_PARAMETER,
                        format!("could not determine data type of parameter ${number}"),
                    )
                    .at(e.location));
                }
            }
        }
        for (i, t) in self.params.iter().enumerate() {
            if matches!(*t, 0 | type_oid::UNKNOWN) {
                return Err(Error::new(
                    sqlstate::INDETERMINATE_DATATYPE,
                    format!(
                        "could not determine data type of parameter ${}",
                        i.saturating_add(1)
                    ),
                ));
            }
        }
        Ok(())
    }
}

/// `attnameAttNum` (parse_relation.c) for user columns.
fn attnum(table: &Table, name: &str) -> Option<usize> {
    let at = table.columns.iter().position(|c| c.name == name)?;
    at.checked_add(1)
}

fn undefined_column(table: &Table, name: &str) -> Error {
    Error::new(
        sqlstate::UNDEFINED_COLUMN,
        format!(
            "column \"{name}\" of relation \"{}\" does not exist",
            table.refname
        ),
    )
}
