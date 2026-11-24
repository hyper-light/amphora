//! Statements: `analyze.c`, `parse_clause.c`, `parse_agg.c`, `parse_target.c` and `parse_relation.c` (REL_19_0
//! src/backend/parser) for SELECT over a FROM item (a table, a generate_series, a LATERAL SELECT of expressions, and
//! inner and left joins of these) with WHERE, GROUP BY, ORDER BY and LIMIT, UPDATE of a column, INSERT of a VALUES row,
//! and INSERT of a SELECT from one generate_series. Every other clause refuses `0A000`.

use amphora_pg_catalog::Kind;
use amphora_pg_catalog::data::type_oid;
use amphora_pg_parser::tree::{Node, NodeId, SelectStmt, Tree, Val};

use crate::expr::{Analysis, Cast, ExprId, ExprKind, Exprs, Place, Scope, Table, Visible};
use crate::{Error, Session, coerce, format_type, sqlstate};

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
    /// Whether it is only what a SELECT groups or orders by (`resjunk`).
    pub junk: bool,
}

/// A place in a join tree (`RangeTblRef`, `JoinExpr`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Join {
    /// A table, by its place in the range table.
    Table(usize),
    /// An inner join, or a left one where `outer`, of two places in the tree.
    Of {
        left: usize,
        right: usize,
        outer: bool,
        on: Option<ExprId>,
    },
}

/// What a SELECT orders by (`SortGroupClause`): a target, and how.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    pub target: usize,
    pub descending: bool,
    pub nulls_first: bool,
}

/// A typed statement (Query).
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub command: Command,
    pub exprs: Exprs,
    /// The range table: what an INSERT or UPDATE assigns to is its first.
    pub tables: Vec<Table>,
    /// The join tree of what a SELECT, or an INSERT's, reads, and its root: none without FROM.
    pub joins: Vec<Join>,
    pub from: Option<usize>,
    pub targets: Vec<Target>,
    pub filter: Option<ExprId>,
    /// What a SELECT groups by: targets, by their places.
    pub group: Vec<usize>,
    pub sort: Vec<Sort>,
    /// A LIMIT, an int8.
    pub limit: Option<ExprId>,
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
        tables: Vec::new(),
        joins: Vec::new(),
        scopes: vec![Scope::default()],
        place: Place::Target,
        aggregates: false,
        params: params.to_vec(),
    };
    let Node::RawStmt {
        stmt: Some(stmt), ..
    } = tree.get(statement)?
    else {
        return Err(Error::unsupported("an empty statement"));
    };
    let analyzed = match tree.get(*stmt)? {
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
        command: analyzed.command,
        exprs: a.exprs,
        tables: a.tables,
        joins: a.joins,
        from: analyzed.from,
        targets: analyzed.targets,
        filter: analyzed.filter,
        group: analyzed.group,
        sort: analyzed.sort,
        limit: analyzed.limit,
        params: a.params,
    })
}

/// A statement's clauses, analyzed.
struct Analyzed {
    command: Command,
    from: Option<usize>,
    targets: Vec<Target>,
    filter: Option<ExprId>,
    group: Vec<usize>,
    sort: Vec<Sort>,
    limit: Option<ExprId>,
}

impl Analyzed {
    fn of(
        command: Command,
        from: Option<usize>,
        targets: Vec<Target>,
        filter: Option<ExprId>,
    ) -> Self {
        Self {
            command,
            from,
            targets,
            filter,
            group: Vec::new(),
            sort: Vec::new(),
            limit: None,
        }
    }
}

/// A step of the walk over FROM (`transformFromClauseItem`).
enum Step {
    Enter(NodeId),
    /// A join's right side, its left one made.
    Right(NodeId),
    /// A join, its sides made: how many names could mean before it.
    Leave(NodeId, usize),
}

impl Analysis<'_> {
    /// `transformSelectStmt` (analyze.c).
    fn select(&mut self, s: &SelectStmt) -> Result<Analyzed, Error> {
        let plain = SelectStmt {
            target_list: s.target_list.clone(),
            from_clause: s.from_clause.clone(),
            where_clause: s.where_clause,
            group_clause: s.group_clause.clone(),
            sort_clause: s.sort_clause.clone(),
            limit_count: s.limit_count,
            limit_option: s.limit_option,
            ..SelectStmt::default()
        };
        if plain != *s {
            return Err(Error::unsupported("a clause of this SELECT"));
        }
        let from = match s.from_clause.as_slice() {
            [] => None,
            [item] => Some(self.from_item(*item)?),
            _ => return Err(Error::unsupported("a FROM of several items")),
        };
        let targets = self.target_list(&s.target_list)?;
        let filter = self.filter(s.where_clause)?;
        let mut select = Analyzed::of(Command::Select, from, targets, filter);
        for item in &s.sort_clause {
            self.sort(*item, &mut select)?;
        }
        for item in &s.group_clause {
            self.group(*item, &mut select)?;
        }
        select.limit = self.limit(s.limit_count)?;
        // resolveTargetListUnknowns (parse_target.c:288).
        for target in &mut select.targets {
            self.resolve_unknown(target)?;
        }
        if self.aggregates || !select.group.is_empty() {
            self.check_grouping(&select)?;
        }
        Ok(select)
    }

    /// `transformTargetList` (parse_target.c) of expressions.
    fn target_list(&mut self, list: &[NodeId]) -> Result<Vec<Target>, Error> {
        let mut targets = Vec::with_capacity(list.len());
        for id in list {
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
                junk: false,
            });
        }
        Ok(targets)
    }

    /// A target that is an unknown literal is text.
    fn resolve_unknown(&mut self, target: &mut Target) -> Result<(), Error> {
        if self.exprs.get(target.expr)?.type_oid == type_oid::UNKNOWN {
            target.expr = self.coerce_type(target.expr, Cast::implicit(type_oid::TEXT))?;
        }
        Ok(())
    }

    /// `transformFromClauseItem` (parse_clause.c:1056): the place in the join tree of what FROM names, and
    /// `transformFromClause`'s leave for names to mean it.
    fn from_item(&mut self, root: NodeId) -> Result<usize, Error> {
        let mut steps = vec![Step::Enter(root)];
        let mut made: Vec<(usize, Vec<usize>)> = Vec::new();
        let missing = || Error::internal("a join has no side");
        while let Some(step) = steps.pop() {
            match step {
                Step::Enter(id) => {
                    let Node::JoinExpr {
                        // JOIN_INNER and JOIN_LEFT (src/include/nodes/nodes.h).
                        jointype,
                        is_natural,
                        larg,
                        using_clause,
                        join_using_alias,
                        alias,
                        ..
                    } = self.tree.get(id)?
                    else {
                        let table = self.item_read(id)?;
                        let at = self.tables.len();
                        self.tables.push(table);
                        made.push((self.joins.len(), vec![at]));
                        self.joins.push(Join::Table(at));
                        continue;
                    };
                    let plain = matches!(jointype, 0 | 1)
                        && !is_natural
                        && using_clause.is_empty()
                        && join_using_alias.is_none()
                        && alias.is_none();
                    if !plain {
                        return Err(Error::unsupported("this join"));
                    }
                    steps.push(Step::Right(id));
                    steps.push(Step::Enter(larg.ok_or_else(missing)?));
                }
                Step::Right(id) => {
                    let Node::JoinExpr { rarg, .. } = self.tree.get(id)? else {
                        return Err(missing());
                    };
                    let (_, left) = made.last().ok_or_else(missing)?;
                    let scope = self.scopes.last_mut().ok_or_else(missing)?;
                    let before = scope.visible.len();
                    scope.visible.extend(left.iter().map(|table| Visible {
                        table: *table,
                        lateral_only: true,
                    }));
                    steps.push(Step::Leave(id, before));
                    steps.push(Step::Enter(rarg.ok_or_else(missing)?));
                }
                Step::Leave(id, before) => {
                    let Node::JoinExpr {
                        jointype, quals, ..
                    } = self.tree.get(id)?
                    else {
                        return Err(missing());
                    };
                    if let Some(scope) = self.scopes.last_mut() {
                        scope.visible.truncate(before);
                    }
                    let (right, right_tables) = made.pop().ok_or_else(missing)?;
                    let (left, mut tables) = made.pop().ok_or_else(missing)?;
                    self.check_conflicts(&tables, &right_tables)?;
                    tables.extend(right_tables);
                    let on = match quals {
                        Some(quals) => Some(self.join_condition(*quals, &tables)?),
                        None => None,
                    };
                    made.push((self.joins.len(), tables));
                    self.joins.push(Join::Of {
                        left,
                        right,
                        outer: *jointype == 1,
                        on,
                    });
                }
            }
        }
        let (root, tables) = made.pop().ok_or_else(missing)?;
        if let Some(scope) = self.scopes.last_mut() {
            scope.visible.extend(visible(&tables));
        }
        Ok(root)
    }

    /// `checkNameSpaceConflicts` (parse_relation.c:438).
    fn check_conflicts(&self, left: &[usize], right: &[usize]) -> Result<(), Error> {
        for one in left.iter().filter_map(|at| self.tables.get(*at)) {
            for other in right.iter().filter_map(|at| self.tables.get(*at)) {
                let named = |t: &Table| t.outputs.is_none() || t.aliased;
                let relation = |t: &Table| t.oid != 0 && !t.aliased;
                let apart = one.refname != other.refname
                    || !named(one)
                    || !named(other)
                    || (relation(one) && relation(other) && one.oid != other.oid);
                if !apart {
                    return Err(Error::new(
                        sqlstate::DUPLICATE_ALIAS,
                        format!("table name \"{}\" specified more than once", one.refname),
                    ));
                }
            }
        }
        Ok(())
    }

    /// `transformJoinOnClause` (parse_clause.c:367): its names mean the join's two sides.
    fn join_condition(&mut self, quals: NodeId, tables: &[usize]) -> Result<ExprId, Error> {
        let mean = |a: &mut Self, visible: Vec<Visible>| match a.scopes.last_mut() {
            Some(scope) => std::mem::replace(&mut scope.visible, visible),
            None => visible,
        };
        let saved = mean(self, visible(tables));
        let on = self.placed(Place::JoinOn, |a| {
            let on = a.expression(quals)?;
            a.coerce_to_boolean(on, "JOIN/ON")
        })?;
        mean(self, saved);
        Ok(on)
    }

    /// What `f` analyzes, as part of `place`.
    fn placed<T>(
        &mut self,
        place: Place,
        f: impl FnOnce(&mut Self) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let outer = std::mem::replace(&mut self.place, place);
        let made = f(self)?;
        self.place = outer;
        Ok(made)
    }

    /// `transformRangeSubselect` (parse_clause.c:407) of a SELECT of expressions alone, which is one row of them:
    /// 19's planner puts them in its columns' place (`pull_up_simple_subquery`, src/backend/optimizer/prep/prepjointree.c).
    fn subselect(
        &mut self,
        lateral: bool,
        subquery: Option<NodeId>,
        alias: Option<NodeId>,
    ) -> Result<Table, Error> {
        let Some(Node::SelectStmt(s)) = subquery.map(|id| self.tree.get(id)).transpose()? else {
            return Err(Error::unsupported("this subquery in FROM"));
        };
        let plain = SelectStmt {
            target_list: s.target_list.clone(),
            limit_option: s.limit_option,
            ..SelectStmt::default()
        };
        if plain != **s || s.target_list.is_empty() {
            return Err(Error::unsupported(
                "a subquery in FROM of more than expressions",
            ));
        }
        let (aliasname, colnames) = match alias.map(|id| self.tree.get(id)).transpose()? {
            None => (None, Vec::new()),
            Some(Node::Alias {
                aliasname,
                colnames,
            }) => (Some(aliasname.clone()), self.strings(colnames)?),
            Some(_) => return Err(Error::unsupported("this alias")),
        };
        self.lateral(lateral);
        self.scopes.push(Scope::default());
        let mut targets =
            self.placed(Place::Subselect, |a| a.target_list(&s.target_list))?;
        for target in &mut targets {
            self.resolve_unknown(target)?;
        }
        self.scopes.pop();
        self.lateral(false);
        let mut columns = Vec::with_capacity(targets.len());
        let mut names = colnames.iter();
        for target in &targets {
            let e = self.exprs.get(target.expr)?;
            columns.push(crate::Column {
                name: names.next().unwrap_or(&target.name).clone(),
                type_oid: e.type_oid,
                typmod: e.typmod,
                not_null: false,
            });
        }
        let refname = aliasname
            .clone()
            .unwrap_or_else(|| "unnamed_subquery".to_owned());
        // addRangeTableEntryForSubquery (parse_relation.c:1675).
        if colnames.len() > columns.len() {
            return Err(Error::new(
                sqlstate::INVALID_COLUMN_REFERENCE,
                format!(
                    "table \"{refname}\" has {} columns available but {} columns specified",
                    columns.len(),
                    colnames.len()
                ),
            ));
        }
        Ok(Table {
            oid: 0,
            refname,
            aliased: aliasname.is_some(),
            columns,
            series: None,
            outputs: Some(targets.iter().map(|target| target.expr).collect()),
        })
    }

    /// `findTargetlistEntrySQL92` (parse_clause.c:2008): the target that GROUP BY or ORDER BY means, one added
    /// where it is none of them.
    fn target_of(
        &mut self,
        id: NodeId,
        targets: &mut Vec<Target>,
        place: Place,
    ) -> Result<usize, Error> {
        let clause = if place == Place::GroupBy {
            "GROUP BY"
        } else {
            "ORDER BY"
        };
        let found = match self.tree.get(id)? {
            Node::ColumnRef { fields, location } => match self.strings(fields)?.as_slice() {
                // GROUP BY prefers a column FROM gives.
                [name]
                    if place == Place::GroupBy
                        && self.column_named(name, true, *location)?.is_some() =>
                {
                    None
                }
                [name] => {
                    let mut found: Option<usize> = None;
                    for (at, target) in targets.iter().enumerate() {
                        if target.junk || target.name != *name {
                            continue;
                        }
                        let same = match found.and_then(|first| targets.get(first)) {
                            Some(first) => self.exprs.equal(first.expr, target.expr)?,
                            None => true,
                        };
                        if !same {
                            return Err(Error::new(
                                sqlstate::AMBIGUOUS_COLUMN,
                                format!("{clause} \"{name}\" is ambiguous"),
                            )
                            .at(*location));
                        }
                        found = found.or(Some(at));
                    }
                    found
                }
                _ => None,
            },
            Node::AConst { val, location } => {
                let Val::Integer(position) = val else {
                    return Err(Error::new(
                        sqlstate::SYNTAX_ERROR,
                        format!("non-integer constant in {clause}"),
                    )
                    .at(*location));
                };
                let at = usize::try_from(*position)
                    .ok()
                    .and_then(|position| position.checked_sub(1))
                    .filter(|at| targets.get(*at).is_some_and(|target| !target.junk));
                if at.is_none() {
                    return Err(Error::new(
                        sqlstate::INVALID_COLUMN_REFERENCE,
                        format!("{clause} position {position} is not in select list"),
                    )
                    .at(*location));
                }
                at
            }
            _ => None,
        };
        if let Some(at) = found {
            // checkTargetlistEntrySQL92 (:1952).
            if place == Place::GroupBy
                && let Some(target) = targets.get(at)
                && let Some(aggregate) = self.exprs.first(
                    target.expr,
                    |e| matches!(e.kind, ExprKind::Agg { .. }),
                    |_, _| Ok(false),
                )?
            {
                return Err(Error::new(
                    sqlstate::GROUPING_ERROR,
                    "aggregate functions are not allowed in GROUP BY".to_owned(),
                )
                .at(self.exprs.get(aggregate)?.location));
            }
            return Ok(at);
        }
        // findTargetlistEntrySQL99 (:2174).
        let expr = self.placed(place, |a| a.expression(id))?;
        for (at, target) in targets.iter().enumerate() {
            if self.exprs.equal(target.expr, expr)? {
                return Ok(at);
            }
        }
        targets.push(Target {
            name: String::new(),
            attnum: 0,
            expr,
            junk: true,
        });
        Ok(targets.len().saturating_sub(1))
    }

    /// `addTargetToSortList` and `addTargetToGroupList` (parse_clause.c:3402, :3546) on a target: an unknown
    /// literal is text, and its type has an order the engine's of its keys is (section 5).
    fn ordered(&mut self, targets: &mut [Target], at: usize) -> Result<(), Error> {
        let target = targets
            .get_mut(at)
            .ok_or_else(|| Error::internal("a target is missing"))?;
        self.resolve_unknown(target)?;
        let e = self.exprs.get(target.expr)?;
        match coerce::base_type(e.type_oid) {
            type_oid::BOOL
            | type_oid::CHAR
            | type_oid::NAME
            | type_oid::INT2
            | type_oid::INT4
            | type_oid::INT8
            | type_oid::OID
            | type_oid::REGCLASS
            | type_oid::TEXT
            | type_oid::VARCHAR
            | type_oid::BPCHAR
            | type_oid::TIMESTAMP
            | type_oid::TIMESTAMPTZ => Ok(()),
            other => Err(
                Error::unsupported(&format!("the order of type {}", format_type(other)))
                    .at(e.location),
            ),
        }
    }

    /// `transformSortClause` (parse_clause.c:2734) of one item.
    fn sort(&mut self, id: NodeId, select: &mut Analyzed) -> Result<(), Error> {
        let Node::SortBy {
            node: Some(node),
            sortby_dir,
            sortby_nulls,
            use_op,
            ..
        } = self.tree.get(id)?
        else {
            return Err(Error::unsupported("this ORDER BY"));
        };
        if !use_op.is_empty() {
            return Err(Error::unsupported("ORDER BY USING"));
        }
        // SORTBY_DESC, SORTBY_NULLS_FIRST and SORTBY_NULLS_LAST (src/include/nodes/parsenodes.h).
        let descending = *sortby_dir == 2;
        let nulls_first = match sortby_nulls {
            1 => true,
            2 => false,
            _ => descending,
        };
        let target = self.target_of(*node, &mut select.targets, Place::OrderBy)?;
        self.ordered(&mut select.targets, target)?;
        if select.sort.iter().all(|sort| sort.target != target) {
            select.sort.push(Sort {
                target,
                descending,
                nulls_first,
            });
        }
        Ok(())
    }

    /// `transformGroupClauseExpr` (parse_clause.c:2369).
    fn group(&mut self, id: NodeId, select: &mut Analyzed) -> Result<(), Error> {
        let target = self.target_of(id, &mut select.targets, Place::GroupBy)?;
        if !select.group.contains(&target) {
            self.ordered(&mut select.targets, target)?;
            select.group.push(target);
        }
        Ok(())
    }

    /// `transformLimitClause` (parse_clause.c:1883).
    fn limit(&mut self, clause: Option<NodeId>) -> Result<Option<ExprId>, Error> {
        let Some(id) = clause else { return Ok(None) };
        let given = self.placed(Place::Limit, |a| a.expression(id))?;
        let e = self.exprs.get(given)?;
        let (type_oid, location) = (e.type_oid, e.location);
        // coerce_to_specific_type (parse_coerce.c).
        let cast = Cast::assignment(type_oid::INT8, -1);
        let limit = match type_oid {
            type_oid::INT8 => given,
            _ => self.coerce_to_target(given, cast)?.ok_or_else(|| {
                Error::new(
                    sqlstate::DATATYPE_MISMATCH,
                    format!(
                        "argument of LIMIT must be type bigint, not type {}",
                        format_type(type_oid)
                    ),
                )
                .at(location)
            })?,
        };
        let column = |e: &crate::Expr| matches!(e.kind, ExprKind::Var { .. });
        if let Some(column) = self.exprs.first(limit, column, |_, _| Ok(false))? {
            return Err(Error::new(
                sqlstate::INVALID_COLUMN_REFERENCE,
                "argument of LIMIT must not contain variables".to_owned(),
            )
            .at(self.exprs.get(column)?.location));
        }
        Ok(Some(limit))
    }

    /// `parseCheckAggregates` (parse_agg.c:1139) and `substitute_grouped_columns` (:1350): a column outside an
    /// aggregate is one grouped by, or of a table whose primary key is (`check_functional_grouping`,
    /// src/backend/catalog/pg_constraint.c).
    fn check_grouping(&self, select: &Analyzed) -> Result<(), Error> {
        let grouped: Vec<ExprId> = select
            .group
            .iter()
            .filter_map(|at| select.targets.get(*at))
            .map(|target| target.expr)
            .collect();
        let by_key = |table: usize| {
            let Some(oid) = self.tables.get(table).map(|t| t.oid).filter(|oid| *oid != 0) else {
                return false;
            };
            let key = self.session.relations.primary_key(oid);
            let is_grouped = |attnum: &usize| {
                let kind = ExprKind::Var {
                    table,
                    attnum: *attnum,
                };
                grouped
                    .iter()
                    .any(|id| self.exprs.get(*id).is_ok_and(|e| e.kind == kind))
            };
            !key.is_empty() && key.iter().all(is_grouped)
        };
        for target in &select.targets {
            let ungrouped = self.exprs.first(
                target.expr,
                |e| matches!(e.kind, ExprKind::Var { table, .. } if !by_key(table)),
                |id, e| {
                    if matches!(e.kind, ExprKind::Agg { .. }) {
                        return Ok(true);
                    }
                    for one in &grouped {
                        if self.exprs.equal(*one, id)? {
                            return Ok(true);
                        }
                    }
                    Ok(false)
                },
            )?;
            let Some(column) = ungrouped else { continue };
            let e = self.exprs.get(column)?;
            let ExprKind::Var { table, attnum } = e.kind else {
                continue;
            };
            let table = self
                .tables
                .get(table)
                .ok_or_else(|| Error::internal("a table is missing"))?;
            let name = attnum.checked_sub(1).and_then(|at| table.columns.get(at));
            return Err(Error::new(
                sqlstate::GROUPING_ERROR,
                format!(
                    "column \"{}.{}\" must appear in the GROUP BY clause or be used in an aggregate function",
                    table.refname,
                    name.map_or("", |c| c.name.as_str())
                ),
            )
            .at(e.location));
        }
        Ok(())
    }

    /// `transformWhereClause` (parse_clause.c).
    fn filter(&mut self, clause: Option<NodeId>) -> Result<Option<ExprId>, Error> {
        match clause {
            None => Ok(None),
            Some(id) => self.placed(Place::Where, |a| {
                let filter = a.expression(id)?;
                Ok(Some(a.coerce_to_boolean(filter, "WHERE")?))
            }),
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

    /// `transformFromClauseItem` (parse_clause.c): a relation, or a function's rows.
    fn item_read(&mut self, id: NodeId) -> Result<Table, Error> {
        match self.tree.get(id)? {
            Node::RangeFunction { .. } => self.placed(Place::Function, |a| a.function_table(id)),
            Node::RangeSubselect {
                lateral,
                subquery,
                alias,
            } => self.subselect(*lateral, *subquery, *alias),
            _ => self.table(id),
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
            aliased: alias.is_some(),
            columns: relations.columns(oid),
            series: None,
            outputs: None,
        })
    }

    /// The table an INSERT or UPDATE assigns to, the range table's first.
    fn assigned_table(&mut self, relation: Option<NodeId>, what: &str) -> Result<Table, Error> {
        let table = self.table(relation.ok_or_else(|| Error::unsupported(what))?)?;
        self.tables.push(table.clone());
        Ok(table)
    }

    /// `transformUpdateStmt` (analyze.c): the WHERE clause first, then the assignment.
    fn update(
        &mut self,
        relation: Option<NodeId>,
        target_list: &[NodeId],
        where_clause: Option<NodeId>,
    ) -> Result<Analyzed, Error> {
        let table = self.assigned_table(relation, "this UPDATE")?;
        if let Some(scope) = self.scopes.last_mut() {
            scope.visible.extend(visible(&[0]));
        }
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
        let source = self.placed(Place::Update, |a| a.expression(*val))?;
        let attnum =
            attnum(&table, name).ok_or_else(|| undefined_column(&table, name).at(*location))?;
        let target = self.assigned(&table, attnum, source)?;
        Ok(Analyzed::of(Command::Update, None, vec![target], filter))
    }

    /// `transformInsertStmt` (analyze.c) for one VALUES row under a list of columns.
    fn insert(
        &mut self,
        relation: Option<NodeId>,
        cols: &[NodeId],
        select: Option<NodeId>,
    ) -> Result<Analyzed, Error> {
        let table = self.assigned_table(relation, "this INSERT")?;
        let attnums = self.insert_targets(&table, cols)?;
        let Some(Node::SelectStmt(select)) = select.map(|id| self.tree.get(id)).transpose()? else {
            return Err(Error::unsupported("INSERT without VALUES"));
        };
        if select.values_lists.is_empty() {
            return self.insert_of_select(table, &attnums, cols, select);
        }
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
        // A VALUES row can't name the target's columns: no table is in scope.
        let values = self.placed(Place::Values, |a| {
            let mut values = Vec::with_capacity(items.len());
            for item in items.iter().flatten() {
                values.push(a.expression(*item)?);
            }
            Ok(values)
        })?;
        let targets = self.row_targets(&table, &attnums, cols, values)?;
        Ok(Analyzed::of(Command::Insert, None, targets, None))
    }

    /// `transformInsertStmt` of a SELECT whose FROM is one generate_series: a column's unknown literal takes its
    /// column's type, as `p_resolve_unknowns` off leaves it.
    fn insert_of_select(
        &mut self,
        table: Table,
        attnums: &[usize],
        cols: &[NodeId],
        select: &SelectStmt,
    ) -> Result<Analyzed, Error> {
        let plain = SelectStmt {
            target_list: select.target_list.clone(),
            from_clause: select.from_clause.clone(),
            where_clause: select.where_clause,
            limit_option: select.limit_option,
            ..SelectStmt::default()
        };
        let [item] = select.from_clause.as_slice() else {
            return Err(Error::unsupported("INSERT of this SELECT"));
        };
        if plain != *select {
            return Err(Error::unsupported("INSERT of this SELECT"));
        }
        let Node::RangeFunction { .. } = self.tree.get(*item)? else {
            return Err(Error::unsupported("INSERT of a SELECT from a table"));
        };
        let from = self.from_item(*item)?;
        let values = self.placed(Place::InsertSelect, |a| {
            let mut values = Vec::with_capacity(select.target_list.len());
            for id in &select.target_list {
                let Node::ResTarget {
                    indirection,
                    val: Some(val),
                    ..
                } = a.tree.get(*id)?
                else {
                    return Err(Error::unsupported("this target"));
                };
                if !indirection.is_empty() {
                    return Err(Error::unsupported("this target"));
                }
                values.push(a.expression(*val)?);
            }
            Ok(values)
        })?;
        let filter = self.filter(select.where_clause)?;
        let targets = self.row_targets(&table, attnums, cols, values)?;
        Ok(Analyzed::of(Command::Insert, Some(from), targets, filter))
    }

    /// `transformInsertRow` (analyze.c): the values under an INSERT's columns, each assigned to its own.
    fn row_targets(
        &mut self,
        table: &Table,
        attnums: &[usize],
        cols: &[NodeId],
        values: Vec<ExprId>,
    ) -> Result<Vec<Target>, Error> {
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
            targets.push(self.assigned(table, *attnum, expr)?);
        }
        Ok(targets)
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
            junk: false,
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

/// Tables that names can mean, LATERAL or not.
fn visible(tables: &[usize]) -> Vec<Visible> {
    tables
        .iter()
        .map(|table| Visible {
            table: *table,
            lateral_only: false,
        })
        .collect()
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
