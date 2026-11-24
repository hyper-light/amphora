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
    /// Var: a column of the statement's table, by its `attnum`.
    Var { attnum: usize },
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

    /// For each expression, whether it is computed from constants alone: there is no column and no
    /// parameter in it. Analysis makes an expression after its arguments.
    pub(crate) fn constant(&self) -> Vec<bool> {
        let mut constant: Vec<bool> = Vec::with_capacity(self.nodes.len());
        for expr in &self.nodes {
            let at = constant.len();
            let known = |ids: &[ExprId]| {
                ids.iter()
                    .all(|id| id.index() < at && constant.get(id.index()) == Some(&true))
            };
            let one = match &expr.kind {
                ExprKind::Const(_) | ExprKind::CurrentTimestamp => true,
                ExprKind::Param(_) | ExprKind::Var { .. } => false,
                ExprKind::Op { args, .. }
                | ExprKind::Func { args, .. }
                | ExprKind::Bool { args, .. } => known(args),
                ExprKind::Relabel { arg, .. }
                | ExprKind::ViaIo { arg, .. }
                | ExprKind::NullTest { arg, .. } => known(std::slice::from_ref(arg)),
            };
            constant.push(one);
        }
        constant
    }

    /// Every expression, in the order analysis made them.
    pub fn iter(&self) -> impl Iterator<Item = &Expr> {
        self.nodes.iter()
    }
}

/// The table whose columns a statement's names can mean (`ParseNamespaceItem`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub oid: u32,
    /// The name its columns are qualified by: its alias, else its own.
    pub refname: String,
    pub columns: Vec<crate::Column>,
}

/// A statement's analysis under way (`ParseState`).
pub struct Analysis<'a> {
    pub tree: &'a Tree,
    pub session: &'a Session<'a>,
    pub exprs: Exprs,
    pub table: Option<Table>,
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
                if f.agg_star
                    || f.agg_distinct
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
                self.make_call(&name, args, f.location)
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

    /// `transformColumnRef` (parse_expr.c) for `column` and `table.column`.
    fn column(&mut self, fields: &[String], location: i32) -> Result<ExprId, Error> {
        let (table, name) = match fields {
            [name] => (None, name),
            [table, name] => (Some(table), name),
            _ => {
                return Err(Error::unsupported("a column name of this many parts").at(location));
            }
        };
        let in_scope = self
            .table
            .as_ref()
            .filter(|t| table.is_none_or(|name| t.refname == *name));
        let found =
            in_scope.and_then(|t| t.columns.iter().enumerate().find(|(_, c)| c.name == *name));
        if let Some((i, c)) = found {
            let kind = ExprKind::Var {
                attnum: i.saturating_add(1),
            };
            return self.exprs.push(kind, c.type_oid, c.typmod, location);
        }
        // errorMissingRTE and errorMissingColumn (parse_relation.c).
        let (sqlstate, message) = match table {
            Some(table) if in_scope.is_none() => (
                sqlstate::UNDEFINED_TABLE,
                format!("missing FROM-clause entry for table \"{table}\""),
            ),
            Some(table) => (
                sqlstate::UNDEFINED_COLUMN,
                format!("column {table}.{name} does not exist"),
            ),
            None => (
                sqlstate::UNDEFINED_COLUMN,
                format!("column \"{name}\" does not exist"),
            ),
        };
        Err(Error::new(sqlstate, message).at(location))
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
        if *setof || *pct_type || !array_bounds.is_empty() {
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
        let t = found.ok_or_else(|| {
            Error::new(
                sqlstate::UNDEFINED_OBJECT,
                format!("type \"{}\" does not exist", name.text()),
            )
            .at(*location)
        })?;
        if typmods.is_empty() {
            return Ok((t.oid, -1, *location));
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
        Ok((t.oid, typmod, *location))
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

    /// `make_fn_arguments` (parse_func.c:2012).
    fn arguments(&mut self, args: Vec<ExprId>, declared: &[u32]) -> Result<Vec<ExprId>, Error> {
        let mut coerced = Vec::with_capacity(args.len());
        for (arg, target) in args.into_iter().zip(declared.iter().copied()) {
            coerced.push(self.coerce_type(arg, Cast::implicit(target))?);
        }
        Ok(coerced)
    }

    /// `ParseFuncOrColumn` (parse_func.c:92) for a plain call of a function or of a type's name.
    fn make_call(
        &mut self,
        name: &Name,
        args: Vec<ExprId>,
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
                    return Err(Error::new(
                        sqlstate::WRONG_OBJECT_TYPE,
                        format!("{} is a procedure", name.text()),
                    )
                    .at(location));
                }
                self.body(function, location)?;
                if function.kind != b'f' || function.retset || variadic > 0 || defaults > 0 {
                    return Err(
                        Error::unsupported(&format!("the call of {}", name.text())).at(location)
                    );
                }
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
        if input == target {
            return Ok(id);
        }
        if target == type_oid::ANY || coerce::is_polymorphic(target) {
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
