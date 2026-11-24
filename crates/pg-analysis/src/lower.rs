//! Lowering: a typed statement as the engine's planner runs it (docs/design/postgres.md section 4), its
//! canonical SQL text with every constant a parameter, and the columns of its result.
//!
//! An expression with no column in it is computed here, once, by [`func::eval`], a parameter of the
//! client's with the value its Bind gave it, and its value is a parameter of the engine's. One over columns is the engine's own comparison where that compares what section 5 stores
//! (integers, text, microseconds), else a call of the function [`PROC_FUNCTION`] names by its OID, which
//! the engine's PostgreSQL dialect runs by [`func::call`]. Anything else refuses `0A000`.

use std::borrow::Cow;

use amphora_pg_catalog::Proc;
use amphora_pg_catalog::data::type_oid;
use amphora_pg_parser::tree::BoolOp;

use crate::expr::{ExprId, ExprKind, Table};
use crate::func::{self, Value};
use crate::stmt::{Command, Query, Target};
use crate::{Error, Session, sqlstate};

/// The prefix of the names the engine's PostgreSQL dialect knows built-in functions by: an OID follows.
pub const PROC_FUNCTION: &str = "pg_proc_";

/// A result column, as RowDescription describes it (doc/src/sgml/protocol.sgml).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    /// The table and the column it is, by OID and `attnum`: 0 for an expression's.
    pub table: u32,
    pub attnum: usize,
    pub type_oid: u32,
    pub typmod: i32,
}

/// A statement as the engine runs it.
#[derive(Debug, Clone, PartialEq)]
pub struct Lowered {
    pub sql: String,
    /// `?1`'s type and value, `?2`'s, and so on.
    pub binds: Vec<(u32, Option<Value>)>,
    /// A SELECT's columns.
    pub fields: Vec<Field>,
}

/// The engine's statement of a typed one, its parameters' values `params`.
pub fn lower(
    query: &Query,
    session: &Session<'_>,
    params: &[Option<Value>],
) -> Result<Lowered, Error> {
    let mut writer = Writer {
        query,
        session,
        params,
        constant: query.exprs.constant(),
        folded: query.exprs.folded(),
        sql: String::new(),
        binds: Vec::new(),
    };
    let table = writer.table()?;
    let fields = fields(query)?;
    match query.command {
        Command::Select => {
            if query.targets.is_empty() {
                return Err(Error::unsupported("a SELECT of no columns"));
            }
            writer.list("SELECT ", &query.targets, |w, target| w.expr(target.expr))?;
            if let Some(from) = &query.table {
                writer.from(from)?;
            }
            writer.filter()?;
        }
        Command::Update => {
            let table = table.ok_or_else(|| Error::internal("an UPDATE has no table"))?;
            writer.sql.push_str("UPDATE ");
            writer.sql.push_str(&table);
            writer.list(" SET ", &query.targets, |w, target| {
                w.assigned_name(target)?;
                w.sql.push_str(" = ");
                w.expr(target.expr)
            })?;
            writer.filter()?;
        }
        Command::Insert => {
            let table = table.ok_or_else(|| Error::internal("an INSERT has no table"))?;
            writer.sql.push_str("INSERT INTO ");
            writer.sql.push_str(&table);
            writer.list(" (", &query.targets, |w, target| w.assigned_name(target))?;
            match &query.source {
                None => {
                    writer.list(") VALUES (", &query.targets, |w, target| {
                        w.expr(target.expr)
                    })?;
                    writer.sql.push(')');
                }
                Some(source) => {
                    writer.list(") SELECT ", &query.targets, |w, target| w.expr(target.expr))?;
                    writer.from(source)?;
                    writer.filter()?;
                }
            }
        }
    }
    Ok(Lowered {
        sql: writer.sql,
        binds: writer.binds,
        fields,
    })
}

/// A statement's result columns: none for one that isn't a SELECT.
pub fn fields(query: &Query) -> Result<Vec<Field>, Error> {
    if query.command != Command::Select {
        return Ok(Vec::new());
    }
    let mut fields = Vec::with_capacity(query.targets.len());
    for target in &query.targets {
        let expr = query.exprs.get(target.expr)?;
        let (table, attnum) = match (&expr.kind, &query.table) {
            (ExprKind::Var { attnum }, Some(table)) if table.oid != 0 => (table.oid, *attnum),
            _ => (0, 0),
        };
        fields.push(Field {
            name: target.name.clone(),
            table,
            attnum,
            type_oid: expr.type_oid,
            typmod: expr.typmod,
        });
    }
    Ok(fields)
}

/// A piece of an expression's text.
enum Piece {
    Text(Cow<'static, str>),
    Expr(ExprId),
}

struct Writer<'a> {
    query: &'a Query,
    session: &'a Session<'a>,
    /// `$n`'s values.
    params: &'a [Option<Value>],
    /// [`crate::expr::Exprs::constant`].
    constant: Vec<bool>,
    /// [`crate::expr::Exprs::folded`].
    folded: Vec<bool>,
    sql: String,
    binds: Vec<(u32, Option<Value>)>,
}

impl Writer<'_> {
    /// The quoted name of the table the statement is over: an engine table by that name, a catalog by
    /// its own. None where it is none, or a series.
    fn table(&self) -> Result<Option<String>, Error> {
        let Some(table) = self.query.table.as_ref().filter(|t| t.series.is_none()) else {
            return Ok(None);
        };
        let (_, name) = self
            .session
            .relations
            .name(table.oid)
            .ok_or_else(|| Error::internal("the statement's table is missing"))?;
        Ok(Some(quoted(&name)))
    }

    /// ` FROM` what a SELECT reads: a relation, or a series' rows, each a column the engine names `value`.
    fn from(&mut self, table: &Table) -> Result<(), Error> {
        self.sql.push_str(" FROM ");
        match &table.series {
            Some(series) => self.series(series),
            None => {
                let (_, name) = self
                    .session
                    .relations
                    .name(table.oid)
                    .ok_or_else(|| Error::internal("the statement's table is missing"))?;
                self.sql.push_str(&quoted(&name));
                Ok(())
            }
        }
    }

    /// `generate_series_step_int8` (src/backend/utils/adt/int8.c): the engine's own series, where 19's of a NULL has
    /// no rows and one of step 0 is refused as it runs. One whose next value would pass the largest bigint, where
    /// the engine's stops on, is refused.
    fn series(&mut self, series: &crate::expr::Series) -> Result<(), Error> {
        let query = self.query;
        let mut bounds = [None, None, Some(1i64)];
        let given = [Some(series.start), Some(series.stop), series.step];
        for (bound, expr) in bounds.iter_mut().zip(given) {
            let Some(expr) = expr else { continue };
            *bound = match func::eval(&query.exprs, expr, self.session, self.params)? {
                None => None,
                Some(Value::Int4(i)) => Some(i64::from(i)),
                Some(Value::Int8(i)) => Some(i),
                Some(_) => return Err(Error::internal("a series' bound isn't an integer")),
            };
        }
        let [Some(start), Some(stop), Some(step)] = bounds else {
            self.sql.push_str("generate_series(1, 0)");
            return Ok(());
        };
        if step == 0 {
            return Err(Error::new(
                sqlstate::INVALID_PARAMETER_VALUE,
                "step size cannot equal zero".to_owned(),
            )
            .of_its_run());
        }
        let past = i128::from(stop).saturating_add(i128::from(step));
        if i64::try_from(past).is_err() && (step > 0) == (start <= stop) {
            return Err(Error::unsupported("a generate_series this near the largest bigint")
                .at(series.location));
        }
        for (at, value) in [start, stop, step].into_iter().enumerate() {
            let separator = if at == 0 { "generate_series(" } else { ", " };
            let piece = self.bind(type_oid::INT8, Some(Value::Int8(value)));
            self.sql.push_str(separator);
            if let Piece::Text(text) = piece {
                self.sql.push_str(&text);
            }
        }
        self.sql.push(')');
        Ok(())
    }

    fn list(
        &mut self,
        open: &str,
        targets: &[Target],
        mut each: impl FnMut(&mut Self, &Target) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.sql.push_str(open);
        let mut first = true;
        for target in targets {
            if !first {
                self.sql.push_str(", ");
            }
            first = false;
            each(self, target)?;
        }
        Ok(())
    }

    /// The column a target assigns, by name.
    fn assigned_name(&mut self, target: &Target) -> Result<(), Error> {
        let name = self.column(target.attnum)?;
        self.sql.push_str(&quoted(&name));
        Ok(())
    }

    fn column(&self, attnum: usize) -> Result<String, Error> {
        column_of(self.query.table.as_ref(), attnum)
    }

    /// The name of a column read: of what the statement reads, which is a series' `value` for one.
    fn read_name(&self, attnum: usize) -> Result<String, Error> {
        let query = self.query;
        let scope = query.source.as_ref().or(query.table.as_ref());
        if scope.is_some_and(|table| table.series.is_some()) {
            return Ok("value".to_owned());
        }
        column_of(scope, attnum)
    }

    fn filter(&mut self) -> Result<(), Error> {
        if let Some(filter) = self.query.filter {
            self.sql.push_str(" WHERE ");
            self.expr(filter)?;
        }
        Ok(())
    }

    /// An expression's text: a flat walk, whose stack is its own (CLAUDE.md section 11).
    fn expr(&mut self, root: ExprId) -> Result<(), Error> {
        let mut pieces = vec![Piece::Expr(root)];
        while let Some(piece) = pieces.pop() {
            match piece {
                Piece::Text(text) => self.sql.push_str(&text),
                Piece::Expr(id) => {
                    let mut made = self.pieces(id)?;
                    made.reverse();
                    pieces.append(&mut made);
                }
            }
        }
        Ok(())
    }

    /// One expression's pieces, in order.
    fn pieces(&mut self, id: ExprId) -> Result<Vec<Piece>, Error> {
        let query = self.query;
        let expr = query.exprs.get(id)?;
        if self.constant.get(id.index()) == Some(&true) {
            let folded = self.folded.get(id.index()) == Some(&true);
            let value = func::eval(&query.exprs, id, self.session, self.params)
                .map_err(|e| if folded { e } else { e.of_its_run() })?;
            return Ok(vec![self.bind(expr.type_oid, value)]);
        }
        let refuse = |what: &str| Err(Error::unsupported(what).at(expr.location));
        match &expr.kind {
            ExprKind::Var { attnum } => {
                Ok(vec![Piece::Text(quoted(&self.read_name(*attnum)?).into())])
            }
            ExprKind::Op { function, args, .. } | ExprKind::Func { function, args, .. } => {
                match (args.as_slice(), comparison(function.src)) {
                    ([left, right], Some(op)) if self.plain(*left) && self.plain(*right) => {
                        Ok(vec![
                            Piece::Text("(".into()),
                            Piece::Expr(*left),
                            Piece::Text(op.into()),
                            Piece::Expr(*right),
                            Piece::Text(")".into()),
                        ])
                    }
                    _ if func::has_body(function) && !session_dependent(function.src) => {
                        Ok(self.call(function, args))
                    }
                    _ => refuse(&format!("function {} over a column", function.name)),
                }
            }
            ExprKind::Bool { op, args } => Ok(match op {
                BoolOp::Not => {
                    let mut pieces = vec![Piece::Text("(NOT ".into())];
                    pieces.extend(args.iter().map(|id| Piece::Expr(*id)));
                    pieces.push(Piece::Text(")".into()));
                    pieces
                }
                BoolOp::And => joined(args, " AND "),
                BoolOp::Or => joined(args, " OR "),
            }),
            ExprKind::NullTest { arg, not } => {
                let test = if *not { " IS NOT NULL)" } else { " IS NULL)" };
                Ok(vec![
                    Piece::Text("(".into()),
                    Piece::Expr(*arg),
                    Piece::Text(test.into()),
                ])
            }
            ExprKind::Relabel { arg, .. } => {
                let from = query.exprs.get(*arg)?.type_oid;
                if text(from) && text(expr.type_oid) {
                    return Ok(vec![Piece::Expr(*arg)]);
                }
                refuse("a cast to a binary-compatible type over a column")
            }
            ExprKind::Const(_) | ExprKind::CurrentTimestamp | ExprKind::Param(_) => {
                refuse("a parameter")
            }
            ExprKind::ViaIo { .. } => refuse("a cast by way of text over a column"),
        }
    }

    /// A parameter, with the value the engine is to bind to it.
    fn bind(&mut self, type_oid: u32, value: Option<Value>) -> Piece {
        self.binds.push((type_oid, value));
        Piece::Text(format!("?{}", self.binds.len()).into())
    }

    /// Whether an expression's value is the column's or a constant's, in the storage section 5 gives
    /// its type: what a comparison of the engine's compares.
    fn plain(&self, id: ExprId) -> bool {
        match self.query.exprs.get(id) {
            Ok(expr) => {
                matches!(expr.kind, ExprKind::Var { .. })
                    || self.constant.get(id.index()) == Some(&true)
            }
            Err(_) => false,
        }
    }

    fn call(&self, function: &'static Proc, args: &[ExprId]) -> Vec<Piece> {
        let mut pieces = vec![Piece::Text(
            format!("{PROC_FUNCTION}{}(", function.oid).into(),
        )];
        let mut first = true;
        for arg in args {
            if !first {
                pieces.push(Piece::Text(", ".into()));
            }
            first = false;
            pieces.push(Piece::Expr(*arg));
        }
        pieces.push(Piece::Text(")".into()));
        pieces
    }
}

/// A column's name, by its `attnum`.
fn column_of(table: Option<&Table>, attnum: usize) -> Result<String, Error> {
    table
        .and_then(|t| attnum.checked_sub(1).and_then(|i| t.columns.get(i)))
        .map(|c| c.name.clone())
        .ok_or_else(|| Error::internal("a column of the statement's table is missing"))
}

/// The arguments of a BoolExpr of several, joined.
fn joined(args: &[ExprId], operator: &'static str) -> Vec<Piece> {
    let mut pieces = vec![Piece::Text("(".into())];
    let mut first = true;
    for arg in args {
        if !first {
            pieces.push(Piece::Text(operator.into()));
        }
        first = false;
        pieces.push(Piece::Expr(*arg));
    }
    pieces.push(Piece::Text(")".into()));
    pieces
}

/// The engine's operator of a comparison whose arguments are stored as it compares them: the integers,
/// the text types (a `bpchar`'s trailing spaces stripped, as it is stored and bound) and timestamps. A
/// text type's is its bytes', as the C and builtin C.UTF-8 collations have it (section 5).
fn comparison(src: &str) -> Option<&'static str> {
    Some(match src {
        "int4eq" | "int8eq" | "int48eq" | "int84eq" | "texteq" | "bpchareq" | "timestamp_eq" => {
            " = "
        }
        "int4ne" | "int8ne" | "int48ne" | "int84ne" | "textne" | "bpcharne" | "timestamp_ne" => {
            " <> "
        }
        "int4lt" | "int8lt" | "int48lt" | "int84lt" | "text_lt" | "bpcharlt" | "timestamp_lt" => {
            " < "
        }
        "int4le" | "int8le" | "int48le" | "int84le" | "text_le" | "bpcharle" | "timestamp_le" => {
            " <= "
        }
        "int4gt" | "int8gt" | "int48gt" | "int84gt" | "text_gt" | "bpchargt" | "timestamp_gt" => {
            " > "
        }
        "int4ge" | "int8ge" | "int48ge" | "int84ge" | "text_ge" | "bpcharge" | "timestamp_ge" => {
            " >= "
        }
        _ => return None,
    })
}

/// Whether a function's value turns on more than its arguments: the session's `TimeZone` or its
/// `search_path`, which the engine's dialect doesn't have.
fn session_dependent(src: &str) -> bool {
    src.contains("timestamptz") || src.contains("regclass")
}

/// `text` and `varchar`, stored alike (section 5).
fn text(oid: u32) -> bool {
    matches!(oid, type_oid::TEXT | type_oid::VARCHAR)
}

/// A quoted identifier of the engine's, whose rule for a `"` is 19's: doubled.
pub(crate) fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Column, Relations, analyze};
    use amphora_pg_catalog::SearchPath;
    use amphora_pg_parser::gram;

    struct Accounts;

    impl Relations for Accounts {
        fn namespace(&self, name: &str) -> Option<u32> {
            match name {
                "public" => Some(2200),
                _ => None,
            }
        }

        fn relation(&self, namespace: u32, name: &str) -> Option<u32> {
            match (namespace, name) {
                (2200, "pgbench_accounts") => Some(16384),
                _ => None,
            }
        }

        fn name(&self, oid: u32) -> Option<(u32, String)> {
            (oid == 16384).then(|| (2200, "pgbench_accounts".to_owned()))
        }

        fn namespace_name(&self, oid: u32) -> Option<String> {
            (oid == 2200).then(|| "public".to_owned())
        }

        fn columns(&self, _oid: u32) -> Vec<Column> {
            [
                ("aid", type_oid::INT4, -1),
                ("abalance", type_oid::INT4, -1),
            ]
            .map(|(name, type_oid, typmod)| Column {
                name: name.to_owned(),
                type_oid,
                typmod,
                not_null: name == "aid",
            })
            .to_vec()
        }
    }

    fn lowered(sql: &str) -> Result<Lowered, Error> {
        let zone = crate::tz::time_zone("UTC").unwrap();
        let path = SearchPath::new(&["public"], "postgres", None, |name| {
            Accounts.namespace(name)
        });
        let session = Session {
            path: &path,
            relations: &Accounts,
            time_zone: &zone,
            now: Some(7),
        };
        let (tree, statements) = gram::parse(sql.as_bytes()).unwrap();
        let query = analyze(&tree, statements[0], &session, &[])?;
        lower(&query, &session, &[])
    }

    /// T-PG.9.2: TPC-B's account update lowers to the engine's update, its key compared by the engine,
    /// its sum by int4pl, so 22003 is raised where 19 raises it.
    #[test]
    fn t_pg_9_2_an_update_of_a_sum_calls_int4pl_over_the_column() {
        let got =
            lowered("UPDATE pgbench_accounts SET abalance = abalance + 5 WHERE aid = 3").unwrap();
        assert_eq!(
            got.sql,
            "UPDATE \"pgbench_accounts\" SET \"abalance\" = pg_proc_177(\"abalance\", ?1) WHERE (\"aid\" = ?2)"
        );
        assert_eq!(
            got.binds,
            [
                (type_oid::INT4, Some(Value::Int4(5))),
                (type_oid::INT4, Some(Value::Int4(3)))
            ]
        );
    }

    /// T-PG.9.2: a constant's value is computed once, and refuses with 19's SQLSTATE.
    #[test]
    fn t_pg_9_2_a_constant_is_computed_and_bound() {
        let got = lowered("SELECT 1 + 2, abalance FROM pgbench_accounts").unwrap();
        assert_eq!(got.sql, "SELECT ?1, \"abalance\" FROM \"pgbench_accounts\"");
        assert_eq!(got.binds, [(type_oid::INT4, Some(Value::Int4(3)))]);
        let refused = lowered("SELECT 1 / 0").unwrap_err();
        assert_eq!(refused.sqlstate, crate::sqlstate::DIVISION_BY_ZERO);
    }
}
