//! Expressions: `parse_expr.c`, `parse_node.c`'s constants, `parse_coerce.c`'s coercion nodes and
//! `parse_param.c`'s parameters (REL_19_0 src/backend/parser), as a typed tree in an arena. The walk
//! is a stack of its own, never a recursion (CLAUDE.md section 11).

use amphora_pg_catalog::data::type_oid;
use amphora_pg_catalog::{Kind, Operator, Proc, column_type, proc_by_oid, type_by_oid, type_named};
use amphora_pg_parser::tree::{BoolOp, Node, NodeId, Tree, Val};

use crate::coerce::{self, Context, Path};
use crate::func::{self, Value};
use crate::resolve::{self, Call, Name};
use crate::{Error, Session, format_type, sqlstate};

/// An expression's place in its [`Exprs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExprId(u32);

impl ExprId {
    /// Its place in its [`Exprs`]' list.
    pub(crate) fn index(self) -> usize {
        usize::try_from(self.0).unwrap_or(usize::MAX)
    }
}

/// `CoercionForm` (src/include/nodes/primnodes.h), for the functions a cast may be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    ExplicitCall,
    ExplicitCast,
    ImplicitCast,
}

/// One node of 19's typed tree (primnodes.h), without its collations.
#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// Const: `None` is NULL.
    Const(Option<Value>),
    /// Param of kind PARAM_EXTERN.
    Param(i32),
    /// Var: a column of a table of the statement's range table, by its place there and its `attnum`.
    Var { table: usize, attnum: usize },
    /// Aggref: `count`, of nothing where its argument is `*`.
    Agg {
        function: &'static Proc,
        args: Vec<ExprId>,
    },
    /// OpExpr: its operator and that operator's function.
    Op {
        operator: u32,
        function: &'static Proc,
        args: Vec<ExprId>,
    },
    /// FuncExpr.
    Func {
        function: &'static Proc,
        args: Vec<ExprId>,
        form: Form,
    },
    /// RelabelType.
    Relabel { arg: ExprId, form: Form },
    /// CoerceViaIO.
    ViaIo { arg: ExprId, form: Form },
    /// BoolExpr.
    Bool { op: BoolOp, args: Vec<ExprId> },
    /// NullTest: IS NOT NULL when `not`.
    NullTest { arg: ExprId, not: bool },
    /// SQLValueFunction CURRENT_TIMESTAMP (SVFOP_CURRENT_TIMESTAMP).
    CurrentTimestamp,
}

/// A coercion to make (`coerce_to_target_type`'s arguments): the type and type modifier, what asks for
/// it, how it shows, and where.
#[derive(Debug, Clone, Copy)]
pub struct Cast {
    pub target: u32,
    pub typmod: i32,
    pub context: Context,
    pub form: Form,
    pub location: i32,
}

impl Cast {
    /// The coercion an argument's or a result's place asks for.
    pub fn implicit(target: u32) -> Self {
        Self {
            target,
            typmod: -1,
            context: Context::Implicit,
            form: Form::ImplicitCast,
            location: -1,
        }
    }

    /// The coercion an assignment asks for.
    pub fn assignment(target: u32, typmod: i32) -> Self {
        Self {
            typmod,
            context: Context::Assignment,
            ..Self::implicit(target)
        }
    }
}

/// An expression with its type (`exprType`), type modifier (`exprTypmod`) and location.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub type_oid: u32,
    pub typmod: i32,
    pub location: i32,
}

/// The expressions of one statement.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Exprs {
    nodes: Vec<Expr>,
}

impl Exprs {
    pub fn get(&self, id: ExprId) -> Result<&Expr, Error> {
        usize::try_from(id.0)
            .ok()
            .and_then(|i| self.nodes.get(i))
            .ok_or_else(|| Error::internal("an expression is missing"))
    }

    fn get_mut(&mut self, id: ExprId) -> Result<&mut Expr, Error> {
        usize::try_from(id.0)
            .ok()
            .and_then(|i| self.nodes.get_mut(i))
            .ok_or_else(|| Error::internal("an expression is missing"))
    }

    pub fn push(
        &mut self,
        kind: ExprKind,
        type_oid: u32,
        typmod: i32,
        location: i32,
    ) -> Result<ExprId, Error> {
        let id = u32::try_from(self.nodes.len())
            .map_err(|_| Error::internal("a statement has too many expressions"))?;
        self.nodes.push(Expr {
            kind,
            type_oid,
            typmod,
            location,
        });
        Ok(ExprId(id))
    }

    /// For each expression, whether it is computed from constants and parameters alone: there is no
    /// column in it. Analysis makes an expression after its arguments.
    pub(crate) fn constant(&self) -> Vec<bool> {
        self.each(|expr| !matches!(expr.kind, ExprKind::Var { .. } | ExprKind::Agg { .. }))
    }

    /// `equal` (src/backend/nodes/equalfuncs.c): whether two expressions are the same, their locations
    /// and how their coercions show aside.
    pub(crate) fn equal(&self, one: ExprId, other: ExprId) -> Result<bool, Error> {
        let mut pairs = vec![(one, other)];
        while let Some((one, other)) = pairs.pop() {
            if one == other {
                continue;
            }
            let (one, other) = (self.get(one)?, self.get(other)?);
            let mut below = |one: &[ExprId], other: &[ExprId]| {
                pairs.extend(one.iter().copied().zip(other.iter().copied()));
                one.len() == other.len()
            };
            let same = match (&one.kind, &other.kind) {
                (ExprKind::Const(a), ExprKind::Const(b)) => a == b,
                (ExprKind::Param(a), ExprKind::Param(b)) => a == b,
                (ExprKind::Var { .. }, ExprKind::Var { .. }) => one.kind == other.kind,
                (ExprKind::CurrentTimestamp, ExprKind::CurrentTimestamp) => true,
                (
                    ExprKind::Op { operator, args, .. },
                    ExprKind::Op {
                        operator: b,
                        args: others,
                        ..
                    },
                ) => operator == b && below(args, others),
                (
                    ExprKind::Func { function, args, .. },
                    ExprKind::Func {
                        function: b,
                        args: others,
                        ..
                    },
                )
                | (
                    ExprKind::Agg { function, args },
                    ExprKind::Agg {
                        function: b,
                        args: others,
                    },
                ) => function == b && below(args, others),
                (
                    ExprKind::Bool { op, args },
                    ExprKind::Bool {
                        op: b,
                        args: others,
                    },
                ) => op == b && below(args, others),
                (ExprKind::Relabel { arg, .. }, ExprKind::Relabel { arg: b, .. })
                | (ExprKind::ViaIo { arg, .. }, ExprKind::ViaIo { arg: b, .. }) => {
                    below(std::slice::from_ref(arg), std::slice::from_ref(b))
                }
                (ExprKind::NullTest { arg, not }, ExprKind::NullTest { arg: b, not: other }) => {
                    not == other && below(std::slice::from_ref(arg), std::slice::from_ref(b))
                }
                _ => false,
            };
            if !same || one.type_oid != other.type_oid || one.typmod != other.typmod {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// What an expression is made of, in order.
    fn below(&self, id: ExprId) -> Result<&[ExprId], Error> {
        Ok(match &self.get(id)?.kind {
            ExprKind::Const(_)
            | ExprKind::CurrentTimestamp
            | ExprKind::Param(_)
            | ExprKind::Var { .. } => &[],
            ExprKind::Op { args, .. }
            | ExprKind::Func { args, .. }
            | ExprKind::Agg { args, .. }
            | ExprKind::Bool { args, .. } => args,
            ExprKind::Relabel { arg, .. }
            | ExprKind::ViaIo { arg, .. }
            | ExprKind::NullTest { arg, .. } => std::slice::from_ref(arg),
        })
    }

    /// The first expression within `root` that `is` so, in the order they were written, and none of those
    /// within one that `past` says to pass over.
    pub(crate) fn first(
        &self,
        root: ExprId,
        is: impl Fn(&Expr) -> bool,
        past: impl Fn(ExprId, &Expr) -> Result<bool, Error>,
    ) -> Result<Option<ExprId>, Error> {
        let mut rest = vec![root];
        while let Some(id) = rest.pop() {
            let expr = self.get(id)?;
            if past(id, expr)? {
                continue;
            }
            if is(expr) {
                return Ok(Some(id));
            }
            rest.extend(self.below(id)?.iter().rev());
        }
        Ok(None)
    }

    /// For each expression, whether 19's planner computes it (`eval_const_expressions`, REL_19_0
    /// src/backend/optimizer/util/clauses.c): there is nothing in it but constants, a Bind's parameters
    /// and immutable functions.
    pub(crate) fn folded(&self) -> Vec<bool> {
        let immutable = |function: &Proc| function.volatile == b'i';
        let io = |oid: u32| proc_by_oid(oid).is_some_and(immutable);
        self.each(|expr| match &expr.kind {
            ExprKind::Var { .. } | ExprKind::Agg { .. } | ExprKind::CurrentTimestamp => false,
            ExprKind::Op { function, .. } | ExprKind::Func { function, .. } => immutable(function),
            ExprKind::ViaIo { arg, .. } => {
                let output = self.get(*arg).ok().and_then(|e| type_by_oid(e.type_oid));
                let input = type_by_oid(expr.type_oid);
                output.is_some_and(|t| io(t.output)) && input.is_some_and(|t| io(t.input))
            }
            _ => true,
        })
    }

    /// For each expression, whether `holds` is so of it and of each expression in it.
    fn each(&self, holds: impl Fn(&Expr) -> bool) -> Vec<bool> {
        let mut held: Vec<bool> = Vec::with_capacity(self.nodes.len());
        for expr in &self.nodes {
            let at = held.len();
            let all = |ids: &[ExprId]| {
                ids.iter()
                    .all(|id| id.index() < at && held.get(id.index()) == Some(&true))
            };
            let below = match &expr.kind {
                ExprKind::Const(_)
                | ExprKind::CurrentTimestamp
                | ExprKind::Param(_)
                | ExprKind::Var { .. } => true,
                ExprKind::Op { args, .. }
                | ExprKind::Func { args, .. }
                | ExprKind::Agg { args, .. }
                | ExprKind::Bool { args, .. } => all(args),
                ExprKind::Relabel { arg, .. }
                | ExprKind::ViaIo { arg, .. }
                | ExprKind::NullTest { arg, .. } => all(std::slice::from_ref(arg)),
            };
            held.push(below && holds(expr));
        }
        held
    }

    /// Every expression, in the order analysis made them.
    pub fn iter(&self) -> impl Iterator<Item = &Expr> {
        self.nodes.iter()
    }
}

/// A table of a statement's range table (`RangeTblEntry`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// 0 for a function's rows or a subquery's.
    pub oid: u32,
    /// The name its columns are qualified by: its alias, else its own.
    pub refname: String,
    /// Whether that name is an alias.
    pub aliased: bool,
    pub columns: Vec<crate::Column>,
    /// The rows it gives where it is no relation: `generate_series` of integers.
    pub series: Option<Series>,
    /// What its columns are where it is a SELECT of expressions alone, one row of them.
    pub outputs: Option<Vec<ExprId>>,
}

/// A table that names can mean (`ParseNamespaceItem`): only from a LATERAL item of FROM where
/// `lateral_only`. A join's type is INNER or LEFT here, so `p_lateral_ok` always holds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Visible {
    pub table: usize,
    pub lateral_only: bool,
}

/// What the names of one query can mean (`ParseState`'s `p_namespace` and `p_lateral_active`).
#[derive(Debug, Default)]
pub(crate) struct Scope {
    pub visible: Vec<Visible>,
    pub lateral: bool,
}

/// What an expression is part of (`ParseExprKind`), as far as an aggregate in it is judged by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place {
    Target,
    Where,
    JoinOn,
    Function,
    Subselect,
    InsertSelect,
    GroupBy,
    OrderBy,
    Limit,
    Update,
    Values,
}

/// `generate_series` of integers: its bounds and step (`generate_series_step_int4` and `_int8`, REL_19_0
/// src/backend/utils/adt/int.c and int8.c).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Series {
    pub start: ExprId,
    pub stop: ExprId,
    /// 1 where there is none.
    pub step: Option<ExprId>,
    pub location: i32,
}

/// A statement's analysis under way (`ParseState`).
pub struct Analysis<'a> {
    pub tree: &'a Tree,
    pub session: &'a Session<'a>,
    pub exprs: Exprs,
    /// The range table, and the join tree over it.
    pub tables: Vec<Table>,
    pub joins: Vec<crate::stmt::Join>,
    /// What names mean, the innermost query's last.
    pub(crate) scopes: Vec<Scope>,
    pub(crate) place: Place,
    /// `p_hasAggs`.
    pub(crate) aggregates: bool,
    /// `$n`'s types (`VarParamState`): 0 not seen, UNKNOWN not resolved yet.
    pub params: Vec<u32>,
}

/// A step of the walk over a raw expression.
enum Step {
    Enter(NodeId),
    /// Make this node of its arguments' expressions, the last ones made.
    Leave(NodeId),
}

/// `MaxAllocSize / sizeof(Oid)` (src/include/utils/memutils.h): the highest parameter number.
const MAX_PARAMS: i32 = 0x3fff_ffff / 4;

impl Analysis<'_> {
    pub fn node(&self, id: NodeId) -> Result<&Node, Error> {
        Ok(self.tree.get(id)?)
    }

    fn type_of(&self, id: ExprId) -> Result<u32, Error> {
        Ok(self.exprs.get(id)?.type_oid)
    }

    /// `transformExprRecurse` (parse_expr.c): a raw expression's typed one.
    pub fn expression(&mut self, root: NodeId) -> Result<ExprId, Error> {
        let mut steps = vec![Step::Enter(root)];
        let mut made: Vec<ExprId> = Vec::new();
        while let Some(step) = steps.pop() {
            match step {
                Step::Enter(id) => {
                    if let Some(leaf) = self.leaf(id)? {
                        made.push(leaf);
                        continue;
                    }
                    steps.push(Step::Leave(id));
                    for child in self.children(id)?.into_iter().rev() {
                        steps.push(Step::Enter(child));
                    }
                }
                Step::Leave(id) => {
                    let n = self.children(id)?.len();
                    let at = made.len().saturating_sub(n);
                    let args = made.split_off(at);
                    made.push(self.make(id, args)?);
                }
            }
        }
        made.pop()
            .ok_or_else(|| Error::internal("an expression has no value"))
    }

    /// The raw expressions a node's own is made of, in the order 19 transforms them.
    fn children(&self, id: NodeId) -> Result<Vec<NodeId>, Error> {
        Ok(match self.node(id)? {
            Node::AExprOp { lexpr, rexpr, .. } => lexpr.iter().chain(rexpr).copied().collect(),
            Node::TypeCast { arg, .. } | Node::NullTest { arg, .. } => {
                arg.iter().copied().collect()
            }
            Node::BoolExpr { args, .. } => args.clone(),
            Node::FuncCall(f) => f.args.clone(),
            _ => Vec::new(),
        })
    }

    /// A node with nothing below it, or the refusal 19 gives before transforming what is below.
    fn leaf(&mut self, id: NodeId) -> Result<Option<ExprId>, Error> {
        let node = self.tree.get(id)?;
        Ok(Some(match node {
            Node::AConst { val, location } => self.constant(val, *location)?,
            Node::ParamRef { number, location } => self.param(*number, *location)?,
            Node::ColumnRef { fields, location } => {
                let fields = self.strings(fields)?;
                self.column(&fields, *location)?
            }
            // SVFOP_CURRENT_TIMESTAMP (src/include/nodes/primnodes.h); transformSQLValueFunction
            // (parse_expr.c) types it timestamptz.
            Node::SqlValueFunction {
                op: 3, location, ..
            } => self.exprs.push(
                ExprKind::CurrentTimestamp,
                type_oid::TIMESTAMPTZ,
                -1,
                *location,
            )?,
            // transformTypeCast looks its type up before it transforms its argument.
            Node::TypeCast { type_name, .. } => {
                self.type_name(*type_name)?;
                return Ok(None);
            }
            Node::AExprOp { .. }
            | Node::BoolExpr { .. }
            | Node::NullTest { .. }
            | Node::FuncCall(_) => return Ok(None),
            _ => return Err(Error::unsupported("this expression")),
        }))
    }

    fn make(&mut self, id: NodeId, args: Vec<ExprId>) -> Result<ExprId, Error> {
        let tree = self.tree;
        match tree.get(id)? {
            Node::AExprOp {
                name,
                lexpr,
                location,
                ..
            } => {
                let name = self.name(name)?;
                match (lexpr, args.as_slice()) {
                    (None, [right]) => self.make_op(&name, None, *right, *location),
                    (Some(_), [left, right]) => self.make_op(&name, Some(*left), *right, *location),
                    _ => Err(Error::new(
                        sqlstate::SYNTAX_ERROR,
                        "postfix operators are not supported".to_owned(),
                    )),
                }
            }
            Node::TypeCast {
                type_name,
                location,
                ..
            } => {
                let (target, typmod, type_location) = self.type_name(*type_name)?;
                let location = if *location < 0 {
                    type_location
                } else {
                    *location
                };
                let arg = single(&args)?;
                let input = self.type_of(arg)?;
                let cast = Cast {
                    target,
                    typmod,
                    context: Context::Explicit,
                    form: Form::ExplicitCast,
                    location,
                };
                self.coerce_to_target(arg, cast)?.ok_or_else(|| {
                    Error::new(
                        sqlstate::CANNOT_COERCE,
                        format!(
                            "cannot cast type {} to {}",
                            format_type(input),
                            format_type(target)
                        ),
                    )
                    .at(location)
                })
            }
            Node::BoolExpr {
                boolop, location, ..
            } => {
                let name = match boolop {
                    BoolOp::And => "AND",
                    BoolOp::Or => "OR",
                    BoolOp::Not => "NOT",
                };
                let mut coerced = Vec::with_capacity(args.len());
                for arg in args {
                    coerced.push(self.coerce_to_boolean(arg, name)?);
                }
                let kind = ExprKind::Bool {
                    op: *boolop,
                    args: coerced,
                };
                self.exprs.push(kind, type_oid::BOOL, -1, *location)
            }
            Node::NullTest {
                nulltesttype,
                location,
                ..
            } => {
                let kind = ExprKind::NullTest {
                    arg: single(&args)?,
                    // NullTestType's IS_NOT_NULL (primnodes.h).
                    not: *nulltesttype == 1,
                };
                self.exprs.push(kind, type_oid::BOOL, -1, *location)
            }
            Node::FuncCall(f) => {
                if f.agg_distinct
                    || f.func_variadic
                    || f.agg_within_group
                    || f.over.is_some()
                    || f.agg_filter.is_some()
                    || !f.agg_order.is_empty()
                {
                    return Err(
                        Error::unsupported("an aggregate or window function call").at(f.location)
                    );
                }
                let name = self.name(&f.funcname)?;
                self.make_call(&name, args, f.agg_star, f.location)
            }
            _ => Err(Error::internal("an expression has no value")),
        }
    }

    /// `make_const` (parse_node.c:345).
    fn constant(&mut self, val: &Val, location: i32) -> Result<ExprId, Error> {
        let (value, type_oid) = match val {
            Val::Integer(i) => (Value::Int4(*i), type_oid::INT4),
            Val::Float(text) => match parse_int64_literal(text) {
                Some(i) => match i32::try_from(i) {
                    Ok(i) => (Value::Int4(i), type_oid::INT4),
                    Err(_) => (Value::Int8(i), type_oid::INT8),
                },
                None => (Value::Numeric(text.clone()), type_oid::NUMERIC),
            },
            Val::Boolean(b) => (Value::Bool(*b), type_oid::BOOL),
            Val::String(s) => (Value::Unknown(s.clone()), type_oid::UNKNOWN),
        };
        self.exprs
            .push(ExprKind::Const(Some(value)), type_oid, -1, location)
    }

    /// `variable_paramref_hook` (parse_param.c).
    fn param(&mut self, number: i32, location: i32) -> Result<ExprId, Error> {
        let index = usize::try_from(number)
            .ok()
            .and_then(|n| n.checked_sub(1))
            .filter(|_| number <= MAX_PARAMS)
            .ok_or_else(|| {
                Error::new(
                    sqlstate::UNDEFINED_PARAMETER,
                    format!("there is no parameter ${number}"),
                )
                .at(location)
            })?;
        if self.params.len() <= index {
            self.params.resize(index.saturating_add(1), 0);
        }
        let slot = self
            .params
            .get_mut(index)
            .ok_or_else(|| Error::internal("a parameter is missing"))?;
        if *slot == 0 {
            *slot = type_oid::UNKNOWN;
        }
        let type_oid = *slot;
        self.exprs
            .push(ExprKind::Param(number), type_oid, -1, location)
    }

    /// `transformColumnRef` (parse_expr.c:510) for `column`, `table.column` and `schema.table.column`.
    fn column(&mut self, fields: &[String], location: i32) -> Result<ExprId, Error> {
        let (schema, table, name) = match fields {
            [name] => {
                let found = match self.column_named(name, false, location)? {
                    Some(found) => found,
                    None if self.table_named(None, name, location)?.is_some() => {
                        return Err(Error::unsupported("a whole-row reference").at(location));
                    }
                    None => return Err(no_column(None, name).at(location)),
                };
                return self.var(found, location);
            }
            [table, name] => (None, table, name),
            [schema, table, name] => (Some(schema.as_str()), table, name),
            _ => {
                return Err(Error::unsupported("a column name of this many parts").at(location));
            }
        };
        let Some(at) = self.table_named(schema, table, location)? else {
            return Err(self.no_table(schema, table).at(location));
        };
        match self.column_of(at, name, location)? {
            Some(attnum) => self.var((at, attnum), location),
            None => Err(no_column(Some(table), name).at(location)),
        }
    }

    fn var(&mut self, (table, attnum): (usize, usize), location: i32) -> Result<ExprId, Error> {
        let column = self
            .tables
            .get(table)
            .and_then(|t| attnum.checked_sub(1).and_then(|at| t.columns.get(at)))
            .ok_or_else(|| Error::internal("a column is missing"))?;
        let (type_oid, typmod) = (column.type_oid, column.typmod);
        let kind = ExprKind::Var { table, attnum };
        self.exprs.push(kind, type_oid, typmod, location)
    }

    /// `scanNSItemForColumn` (parse_relation.c:716): a table's column of that name.
    fn column_of(&self, table: usize, name: &str, location: i32) -> Result<Option<usize>, Error> {
        let columns = self.tables.get(table).map(|t| t.columns.as_slice());
        let mut named = columns
            .unwrap_or_default()
            .iter()
            .enumerate()
            .filter(|(_, c)| c.name == name);
        let first = named.next();
        if named.next().is_some() {
            return Err(ambiguous(name).at(location));
        }
        Ok(first.map(|(at, _)| at.saturating_add(1)))
    }

    /// `colNameToVar` (parse_relation.c:926): the table and column a bare name means, in the innermost query
    /// that has one, or in the innermost alone.
    pub(crate) fn column_named(
        &self,
        name: &str,
        innermost: bool,
        location: i32,
    ) -> Result<Option<(usize, usize)>, Error> {
        for scope in self.scopes.iter().rev() {
            let mut found = None;
            for item in &scope.visible {
                if item.lateral_only && !scope.lateral {
                    continue;
                }
                let Some(attnum) = self.column_of(item.table, name, location)? else {
                    continue;
                };
                if found.is_some() {
                    return Err(ambiguous(name).at(location));
                }
                found = Some((item.table, attnum));
            }
            if found.is_some() || innermost {
                return Ok(found);
            }
        }
        Ok(None)
    }

    /// `refnameNamespaceItem` (parse_relation.c:130): the table a name means.
    fn table_named(
        &self,
        schema: Option<&str>,
        name: &str,
        location: i32,
    ) -> Result<Option<usize>, Error> {
        let relations = self.session.relations;
        let oid = match schema {
            None => None,
            Some(schema) => {
                let namespace = relations.namespace(schema);
                let Some(oid) = namespace.and_then(|ns| relations.relation(ns, name)) else {
                    return Ok(None);
                };
                Some(oid)
            }
        };
        for scope in self.scopes.iter().rev() {
            let mut found = None;
            for item in &scope.visible {
                let Some(table) = self.tables.get(item.table) else {
                    continue;
                };
                // A subquery without an alias has no name (addRangeTableEntryForSubquery).
                let named = match oid {
                    Some(oid) => table.oid == oid && !table.aliased,
                    None => table.refname == name && (table.outputs.is_none() || table.aliased),
                };
                if !named || (item.lateral_only && !scope.lateral) {
                    continue;
                }
                if found.is_some() {
                    let name = oid.map_or_else(|| format!("\"{name}\""), |oid| oid.to_string());
                    return Err(Error::new(
                        sqlstate::AMBIGUOUS_ALIAS,
                        format!("table reference {name} is ambiguous"),
                    )
                    .at(location));
                }
                found = Some(item.table);
            }
            if found.is_some() {
                return Ok(found);
            }
        }
        Ok(None)
    }

    /// `errorMissingRTE` (parse_relation.c:3723).
    fn no_table(&self, schema: Option<&str>, name: &str) -> Error {
        let relations = self.session.relations;
        let oid = match schema {
            Some(schema) => relations
                .namespace(schema)
                .and_then(|ns| relations.relation(ns, name)),
            None => self
                .session
                .path
                .resolve(Kind::Relation, |ns| relations.relation(ns, name)),
        };
        // searchRangeTableForRel (:360): the tables read so far, whether names can mean them here or not.
        let there = self
            .tables
            .iter()
            .any(|table| (table.oid != 0 && Some(table.oid) == oid) || table.refname == name);
        let what = if there {
            "invalid reference to"
        } else {
            "missing"
        };
        Error::new(
            sqlstate::UNDEFINED_TABLE,
            format!("{what} FROM-clause entry for table \"{name}\""),
        )
    }

    /// A list of String nodes' values.
    pub fn strings(&self, list: &[NodeId]) -> Result<Vec<String>, Error> {
        list.iter()
            .map(|id| match self.node(*id)? {
                Node::String(s) => Ok(s.clone()),
                _ => Err(Error::unsupported("this name")),
            })
            .collect()
    }

    /// `DeconstructQualifiedName` (src/backend/catalog/namespace.c) of a list of String nodes.
    fn name(&self, list: &[NodeId]) -> Result<Name, Error> {
        match <[String; 1]>::try_from(self.strings(list)?) {
            Ok([name]) => Ok(Name { schema: None, name }),
            Err(parts) => match <[String; 2]>::try_from(parts) {
                Ok([schema, name]) => Ok(Name {
                    schema: Some(schema),
                    name,
                }),
                Err(parts) => Err(Error::unsupported(&format!("the name {}", parts.join(".")))),
            },
        }
    }

    /// `typenameTypeIdAndMod` (parse_type.c): a TypeName's type, type modifier and location.
    fn type_name(&self, id: Option<NodeId>) -> Result<(u32, i32, i32), Error> {
        let Some(Node::TypeName {
            names,
            setof,
            pct_type,
            typmods,
            array_bounds,
            location,
            ..
        }) = id.map(|id| self.node(id)).transpose()?
        else {
            return Err(Error::internal("a cast has no type"));
        };
        if *setof || *pct_type {
            return Err(Error::unsupported("this type's name").at(*location));
        }
        let name = self.name(names)?;
        let found = match &name.schema {
            Some(schema) => {
                let namespace = self.session.relations.namespace(schema).ok_or_else(|| {
                    Error::new(
                        sqlstate::INVALID_SCHEMA_NAME,
                        format!("schema \"{schema}\" does not exist"),
                    )
                    .at(*location)
                })?;
                type_named(namespace, &name.name)
            }
            None => self
                .session
                .path
                .resolve(Kind::Type, |namespace| type_named(namespace, &name.name)),
        };
        let undefined = || {
            let brackets = if array_bounds.is_empty() { "" } else { "[]" };
            Error::new(
                sqlstate::UNDEFINED_OBJECT,
                format!("type \"{}{brackets}\" does not exist", name.text()),
            )
            .at(*location)
        };
        let t = found.ok_or_else(undefined)?;
        // LookupTypeNameExtended (parse_type.c): an array's dimensions say nothing of its type.
        let array = |oid: u32| match type_by_oid(oid).map(|t| t.array) {
            _ if array_bounds.is_empty() => Ok(oid),
            Some(array) if array != 0 => Ok(array),
            _ => Err(undefined()),
        };
        if typmods.is_empty() {
            return Ok((array(t.oid)?, -1, *location));
        }
        let mut modifiers = Vec::with_capacity(typmods.len());
        for m in typmods {
            match self.node(*m)? {
                Node::AConst {
                    val: Val::Integer(i),
                    ..
                } => modifiers.push(i64::from(*i)),
                _ => return Err(Error::unsupported("this type modifier").at(*location)),
            }
        }
        let quoted = format!("\"{}\"", t.name);
        let typmod = column_type(&quoted, &modifiers)
            .map_err(|e| {
                Error::new(sqlstate::INVALID_PARAMETER_VALUE, e.to_string()).at(*location)
            })?
            .typmod;
        Ok((array(t.oid)?, typmod, *location))
    }

    /// The functions and operators written so far (section 4): one with no body refuses here.
    fn body(&self, function: &'static Proc, location: i32) -> Result<(), Error> {
        if func::has_body(function) {
            return Ok(());
        }
        let args: Vec<String> = function.args.iter().map(|t| format_type(*t)).collect();
        Err(
            Error::unsupported(&format!("function {}({})", function.name, args.join(", ")))
                .at(location),
        )
    }

    /// `make_op` (parse_oper.c:705).
    fn make_op(
        &mut self,
        name: &Name,
        left: Option<ExprId>,
        right: ExprId,
        location: i32,
    ) -> Result<ExprId, Error> {
        let left_type = left.map(|l| self.type_of(l)).transpose()?;
        let right_type = self.type_of(right)?;
        let operator = resolve::operator(
            self.session.path,
            |schema| self.session.relations.namespace(schema),
            name,
            left_type,
            right_type,
        )
        .map_err(|e| e.at(location))?;
        self.operator(operator, left, right, location)
    }

    fn operator(
        &mut self,
        operator: &'static Operator,
        left: Option<ExprId>,
        right: ExprId,
        location: i32,
    ) -> Result<ExprId, Error> {
        let function = proc_by_oid(operator.code).ok_or_else(|| {
            Error::new(
                sqlstate::UNDEFINED_FUNCTION,
                format!("operator is only a shell: {}", operator.name),
            )
            .at(location)
        })?;
        self.body(function, location)?;
        let declared: Vec<u32> = left
            .map(|_| operator.left)
            .into_iter()
            .chain([operator.right])
            .collect();
        let args = self.arguments(left.into_iter().chain([right]).collect(), &declared)?;
        let kind = ExprKind::Op {
            operator: operator.oid,
            function,
            args,
        };
        self.exprs.push(kind, operator.result, -1, location)
    }

    /// A function in FROM (`transformRangeFunction`, parse_clause.c): `generate_series` of integers, whose rows are a
    /// column of the same name as its alias or its own (`addRangeTableEntryForFunction`, parse_relation.c).
    pub(crate) fn function_table(&mut self, id: NodeId) -> Result<Table, Error> {
        let refuse = || Error::unsupported("this function in FROM");
        let Node::RangeFunction {
            lateral: false,
            ordinality: false,
            is_rowsfrom: false,
            functions,
            alias,
            coldeflist,
        } = self.tree.get(id)?
        else {
            return Err(refuse());
        };
        let [function] = functions.as_slice() else {
            return Err(refuse());
        };
        let Node::List(items) = self.tree.get(*function)? else {
            return Err(refuse());
        };
        let ([Some(call), None], true) = (items.as_slice(), coldeflist.is_empty()) else {
            return Err(refuse());
        };
        let Node::FuncCall(f) = self.tree.get(*call)? else {
            return Err(refuse());
        };
        if f.agg_star
            || f.agg_distinct
            || f.func_variadic
            || f.agg_within_group
            || f.over.is_some()
            || f.agg_filter.is_some()
            || !f.agg_order.is_empty()
        {
            return Err(refuse().at(f.location));
        }
        let name = self.name(&f.funcname)?;
        // transformRangeFunction: a function's arguments are LATERAL, said or not.
        self.lateral(true);
        let mut args = Vec::with_capacity(f.args.len());
        for arg in &f.args {
            args.push(self.expression(*arg)?);
        }
        self.lateral(false);
        let mut inputs = Vec::with_capacity(args.len());
        for arg in &args {
            inputs.push(self.type_of(*arg)?);
        }
        let call = resolve::function(
            self.session.path,
            |schema| self.session.relations.namespace(schema),
            &name,
            &inputs,
            false,
        )
        .map_err(|e| e.at(f.location))?;
        let Call::Function {
            function,
            args: declared,
            variadic: 0,
            defaults: 0,
        } = call
        else {
            return Err(refuse().at(f.location));
        };
        let steps = match function.src {
            "generate_series_int4" | "generate_series_int8" => 2,
            "generate_series_step_int4" | "generate_series_step_int8" => 3,
            _ => return Err(refuse().at(f.location)),
        };
        let args = self.arguments(args, &declared)?;
        let mut bounds = args.into_iter();
        let (Some(start), Some(stop)) = (bounds.next(), bounds.next()) else {
            return Err(refuse().at(f.location));
        };
        let step = bounds.next().filter(|_| steps == 3);
        let own = name.name.clone();
        let (refname, column) = match alias.map(|id| self.tree.get(id)).transpose()? {
            None => (own.clone(), own),
            Some(Node::Alias {
                aliasname,
                colnames,
            }) => match self.strings(colnames)?.as_slice() {
                [] => (aliasname.clone(), aliasname.clone()),
                [column] => (aliasname.clone(), column.clone()),
                _ => return Err(refuse().at(f.location)),
            },
            Some(_) => return Err(refuse().at(f.location)),
        };
        Ok(Table {
            oid: 0,
            refname,
            aliased: alias.is_some(),
            columns: vec![crate::Column {
                name: column,
                type_oid: function.result,
                typmod: -1,
                not_null: false,
            }],
            series: Some(Series {
                start,
                stop,
                step,
                location: f.location,
            }),
            outputs: None,
        })
    }

    /// `p_lateral_active` of the innermost query.
    pub(crate) fn lateral(&mut self, active: bool) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.lateral = active;
        }
    }

    /// `make_fn_arguments` (parse_func.c:2012).
    fn arguments(&mut self, args: Vec<ExprId>, declared: &[u32]) -> Result<Vec<ExprId>, Error> {
        let mut coerced = Vec::with_capacity(args.len());
        for (arg, target) in args.into_iter().zip(declared.iter().copied()) {
            coerced.push(self.coerce_type(arg, Cast::implicit(target))?);
        }
        Ok(coerced)
    }

    /// `ParseFuncOrColumn` (parse_func.c:92) for a plain call of a function, of an aggregate or of a type's name.
    fn make_call(
        &mut self,
        name: &Name,
        args: Vec<ExprId>,
        star: bool,
        location: i32,
    ) -> Result<ExprId, Error> {
        let mut inputs = Vec::with_capacity(args.len());
        for arg in &args {
            inputs.push(self.type_of(*arg)?);
        }
        let literal = match args.first() {
            Some(arg) => matches!(self.exprs.get(*arg)?.kind, ExprKind::Const(_)),
            None => false,
        };
        let call = resolve::function(
            self.session.path,
            |schema| self.session.relations.namespace(schema),
            name,
            &inputs,
            literal,
        )
        .map_err(|e| e.at(location))?;
        let wrong =
            |message: String| Err(Error::new(sqlstate::WRONG_OBJECT_TYPE, message).at(location));
        let aggregate = matches!(&call, Call::Function { function, .. } if function.kind == b'a');
        if star && !aggregate {
            let name = name.text();
            return wrong(format!(
                "{name}(*) specified, but {name} is not an aggregate function"
            ));
        }
        match call {
            Call::Coercion(target) => {
                let cast = Cast {
                    target,
                    typmod: -1,
                    context: Context::Explicit,
                    form: Form::ExplicitCall,
                    location,
                };
                self.coerce_type(single(&args)?, cast)
            }
            Call::Function {
                function,
                args: declared,
                variadic,
                defaults,
            } => {
                if function.kind == b'p' {
                    return wrong(format!("{} is a procedure", name.text()));
                }
                if aggregate && function.name == "count" {
                    let args = self.arguments(args, &declared)?;
                    if args.is_empty() && !star {
                        return wrong(format!(
                            "{}(*) must be used to call a parameterless aggregate function",
                            name.text()
                        ));
                    }
                    return self.aggregate(function, args, location);
                }
                self.body(function, location)?;
                if function.kind != b'f' || function.retset || variadic > 0 || defaults > 0 {
                    return Err(
                        Error::unsupported(&format!("the call of {}", name.text())).at(location)
                    );
                }
                let declared =
                    coerce::resolve_generics(&inputs, declared).map_err(|e| e.at(location))?;
                let args = self.arguments(args, &declared)?;
                let kind = ExprKind::Func {
                    function,
                    args,
                    form: Form::ExplicitCall,
                };
                self.exprs.push(kind, function.result, -1, location)
            }
        }
    }

    /// `transformAggregateCall` (parse_agg.c:113) and `check_agglevels_and_constraints` (:308).
    fn aggregate(
        &mut self,
        function: &'static Proc,
        args: Vec<ExprId>,
        location: i32,
    ) -> Result<ExprId, Error> {
        let any = |_: ExprId, _: &Expr| Ok(false);
        for arg in &args {
            let nested = |e: &Expr| matches!(e.kind, ExprKind::Agg { .. });
            if let Some(inner) = self.exprs.first(*arg, nested, any)? {
                return Err(Error::new(
                    sqlstate::GROUPING_ERROR,
                    "aggregate function calls cannot be nested".to_owned(),
                )
                .at(self.exprs.get(inner)?.location));
            }
        }
        let place = match self.place {
            Place::Target | Place::OrderBy => None,
            Place::Where => Some("WHERE"),
            Place::JoinOn => Some("JOIN conditions"),
            Place::Function => Some("functions in FROM"),
            Place::GroupBy => Some("GROUP BY"),
            Place::Limit => Some("LIMIT"),
            Place::Update => Some("UPDATE"),
            Place::Values => Some("VALUES"),
            Place::InsertSelect => {
                return Err(Error::unsupported("an aggregate in an INSERT's SELECT").at(location));
            }
            // Its level is its arguments' columns' query's, which is the one the subquery is read in: a
            // subquery here reads no table of its own.
            Place::Subselect => {
                let column = |e: &Expr| matches!(e.kind, ExprKind::Var { .. });
                let mut outer = false;
                for arg in &args {
                    outer = outer || self.exprs.first(*arg, column, any)?.is_some();
                }
                if !outer {
                    return Err(
                        Error::unsupported("an aggregate in a subquery in FROM").at(location)
                    );
                }
                Some("FROM clause of their own query level")
            }
        };
        if let Some(place) = place {
            return Err(Error::new(
                sqlstate::GROUPING_ERROR,
                format!("aggregate functions are not allowed in {place}"),
            )
            .at(location));
        }
        self.aggregates = true;
        let kind = ExprKind::Agg { function, args };
        self.exprs.push(kind, function.result, -1, location)
    }

    /// `coerce_to_boolean` (parse_coerce.c:1163).
    pub fn coerce_to_boolean(&mut self, id: ExprId, construct: &str) -> Result<ExprId, Error> {
        let e = self.exprs.get(id)?;
        let (type_oid, location) = (e.type_oid, e.location);
        if type_oid == type_oid::BOOL {
            return Ok(id);
        }
        let cast = Cast::assignment(type_oid::BOOL, -1);
        self.coerce_to_target(id, cast)?.ok_or_else(|| {
            Error::new(
                sqlstate::DATATYPE_MISMATCH,
                format!(
                    "argument of {construct} must be type boolean, not type {}",
                    format_type(type_oid)
                ),
            )
            .at(location)
        })
    }

    /// `coerce_to_target_type` (parse_coerce.c:79): none when the cast isn't allowed here.
    pub fn coerce_to_target(&mut self, id: ExprId, cast: Cast) -> Result<Option<ExprId>, Error> {
        let input = self.type_of(id)?;
        if !coerce::can_coerce(&[input], &[cast.target], cast.context) {
            return Ok(None);
        }
        let result = self.coerce_type(id, cast)?;
        Ok(Some(self.coerce_typmod(result, cast)?))
    }

    /// `coerce_type` (parse_coerce.c:158), where `can_coerce` has allowed it.
    pub fn coerce_type(&mut self, id: ExprId, cast: Cast) -> Result<ExprId, Error> {
        let Cast {
            target,
            form,
            location,
            ..
        } = cast;
        let e = self.exprs.get(id)?;
        let input = e.type_oid;
        if input == target || target == type_oid::ANY {
            return Ok(id);
        }
        if coerce::is_polymorphic(target) {
            return Err(
                Error::unsupported(&format!("a value of type {}", format_type(target)))
                    .at(e.location),
            );
        }
        if input == type_oid::UNKNOWN {
            match &e.kind {
                ExprKind::Const(value) => {
                    let at = e.location;
                    let value = match value {
                        Some(Value::Unknown(text)) => {
                            Some(func::input(target, text, self.session).map_err(|e| e.at(at))?)
                        }
                        _ => None,
                    };
                    return self.exprs.push(ExprKind::Const(value), target, -1, at);
                }
                ExprKind::Param(number) => {
                    let number = *number;
                    return self.resolve_param(id, number, target, location);
                }
                _ => {}
            }
        }
        match coerce::pathway(target, input, cast.context) {
            Path::Relabel => {
                self.exprs
                    .push(ExprKind::Relabel { arg: id, form }, target, -1, location)
            }
            Path::None => Err(Error::internal(&format!(
                "failed to find conversion function from {} to {}",
                format_type(input),
                format_type(target)
            ))),
            path => self.coercion(id, path, cast),
        }
    }

    /// `build_coercion_expression` (parse_coerce.c:842).
    fn coercion(&mut self, id: ExprId, path: Path, cast: Cast) -> Result<ExprId, Error> {
        let Cast {
            target,
            typmod,
            context,
            form,
            location,
        } = cast;
        match path {
            Path::Func(oid) => {
                let function = proc_by_oid(oid)
                    .ok_or_else(|| Error::internal("a cast's function is missing"))?;
                self.body(function, location)?;
                let mut args = vec![id];
                // The target's type modifier and whether the cast is explicit, where the function
                // takes them.
                let mut length = -1;
                if function.args.len() >= 2 {
                    let modifier = Some(Value::Int4(typmod));
                    args.push(self.exprs.push(
                        ExprKind::Const(modifier),
                        type_oid::INT4,
                        -1,
                        -1,
                    )?);
                    length = typmod;
                }
                if function.args.len() == 3 {
                    let explicit = Some(Value::Bool(context == Context::Explicit));
                    args.push(self.exprs.push(
                        ExprKind::Const(explicit),
                        type_oid::BOOL,
                        -1,
                        -1,
                    )?);
                }
                let kind = ExprKind::Func {
                    function,
                    args,
                    form,
                };
                // exprTypmod of a length coercion (src/backend/nodes/nodeFuncs.c, exprIsLengthCoercion).
                self.exprs.push(kind, target, length, location)
            }
            Path::ViaIo => {
                let input = self.type_of(id)?;
                for function in [
                    type_by_oid(input).and_then(|t| proc_by_oid(t.output)),
                    type_by_oid(target).and_then(|t| proc_by_oid(t.input)),
                ]
                .into_iter()
                .flatten()
                {
                    self.body(function, location)?;
                }
                self.exprs
                    .push(ExprKind::ViaIo { arg: id, form }, target, -1, location)
            }
            _ => {
                Err(Error::unsupported(&format!("a cast to {}", format_type(target))).at(location))
            }
        }
    }

    /// `coerce_type_typmod` (parse_coerce.c:756).
    fn coerce_typmod(&mut self, id: ExprId, cast: Cast) -> Result<ExprId, Error> {
        let Cast {
            target,
            typmod,
            form,
            location,
            ..
        } = cast;
        let e = self.exprs.get(id)?;
        if e.typmod == typmod {
            return Ok(id);
        }
        let path = if typmod < 0 {
            Path::None
        } else {
            coerce::typmod_function(target)
        };
        match path {
            Path::None => self.exprs.push(
                ExprKind::Relabel { arg: id, form },
                target,
                typmod,
                location,
            ),
            path => self.coercion(id, path, cast),
        }
    }

    /// `variable_coerce_param_hook` (parse_param.c): the type a use gives an untyped parameter.
    fn resolve_param(
        &mut self,
        id: ExprId,
        number: i32,
        target: u32,
        location: i32,
    ) -> Result<ExprId, Error> {
        let at = self.exprs.get(id)?.location;
        let slot = usize::try_from(number)
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|i| self.params.get_mut(i))
            .ok_or_else(|| Error::internal("a parameter is missing"))?;
        if *slot == type_oid::UNKNOWN {
            *slot = target;
        } else if *slot != target {
            return Err(Error::new(
                sqlstate::AMBIGUOUS_PARAMETER,
                format!("inconsistent types deduced for parameter ${number}"),
            )
            .at(at));
        }
        let param = self.exprs.get_mut(id)?;
        param.type_oid = target;
        if location >= 0 && (param.location < 0 || location < param.location) {
            param.location = location;
        }
        Ok(id)
    }
}

/// `errorMissingColumn` (parse_relation.c:3794).
fn no_column(table: Option<&str>, name: &str) -> Error {
    let message = match table {
        Some(table) => format!("column {table}.{name} does not exist"),
        None => format!("column \"{name}\" does not exist"),
    };
    Error::new(sqlstate::UNDEFINED_COLUMN, message)
}

fn ambiguous(name: &str) -> Error {
    Error::new(
        sqlstate::AMBIGUOUS_COLUMN,
        format!("column reference \"{name}\" is ambiguous"),
    )
}

/// The one argument of a node that has one.
fn single(args: &[ExprId]) -> Result<ExprId, Error> {
    match args {
        [arg] => Ok(*arg),
        _ => Err(Error::internal("an expression has no argument")),
    }
}

/// `pg_strtoint64_safe` (src/backend/utils/adt/numutils.c) on a numeric literal as scan.l reads one:
/// digits with underscores between them, or 0x, 0o or 0b digits; none when it isn't an integer that
/// fits int8.
fn parse_int64_literal(text: &str) -> Option<i64> {
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let lower = digits.to_ascii_lowercase();
    let (radix, digits) = match lower.get(..2) {
        Some("0x") => (16, lower.get(2..)?),
        Some("0o") => (8, lower.get(2..)?),
        Some("0b") => (2, lower.get(2..)?),
        _ => (10, lower.as_str()),
    };
    let digits = digits.replace('_', "");
    let signed = if negative {
        format!("-{digits}")
    } else {
        digits
    };
    i64::from_str_radix(&signed, radix).ok()
}
