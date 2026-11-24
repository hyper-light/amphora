//! Random databases and queries over the inventory (bench/corpus/features.json): tables of every
//! affinity and collation with their constraints and indexes, rows of every storage class and its
//! edges, and expressions from the inventory's operators and functions (each at the arities SQLite's
//! own `pragma_function_list` gives). Queries are trees, so the oracles can rewrite them and the
//! reducer can cut them down.

use super::engines::V;

/// SplitMix64: a seed gives the same run on every machine and version.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    pub fn chance(&mut self, p: f64) -> bool {
        ((self.next() >> 11) as f64 / (1u64 << 53) as f64) < p
    }

    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

// ---------------------------------------------------------------- the language

/// An expression, written fully parenthesized so precedence never decides what it means.
#[derive(Clone, Debug)]
pub enum E {
    Col(String, String),
    Lit(V),
    Un(&'static str, Box<E>),
    Bin(&'static str, Box<E>, Box<E>),
    Fn(String, Vec<E>),
    Agg(String, bool, Vec<E>),
    Case(Option<Box<E>>, Vec<(E, E)>, Option<Box<E>>),
    Cast(Box<E>, &'static str),
    Collate(Box<E>, &'static str),
    In(Box<E>, Vec<E>, bool),
    Between(Box<E>, Box<E>, Box<E>, bool),
    Null(Box<E>, bool),
    Exists(Box<Sel>, bool),
    Scalar(Box<Sel>),
    InSel(Box<E>, Box<Sel>, bool),
    /// `*`, a whole select list.
    Star,
    Win(Box<Win>),
    /// `(a, b) < (c, d)`.
    RowBin(&'static str, Vec<E>, Vec<E>),
    RowBetween(Vec<E>, Vec<E>, Vec<E>, bool),
    /// `(a, b) IN (SELECT ..)`, or a row compared with a subquery's one row.
    RowSel(&'static str, Vec<E>, Box<Sel>),
}

fn list(xs: &[E]) -> String {
    xs.iter().map(E::sql).collect::<Vec<_>>().join(", ")
}

fn terms(xs: &[(E, bool)]) -> String {
    xs.iter()
        .map(|(e, d)| format!("{}{}", e.sql(), if *d { " DESC" } else { "" }))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A window function's call.
#[derive(Clone, Debug)]
pub struct Win {
    pub func: &'static str,
    pub args: Vec<E>,
    pub filter: Option<E>,
    pub partition: Vec<E>,
    pub order: Vec<(E, bool)>,
    pub frame: Option<Frame>,
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub unit: &'static str,
    pub bounds: String,
    pub exclude: &'static str,
}

impl Win {
    fn sql(&self) -> String {
        let args = if self.args.is_empty() && self.func == "count" {
            "*".to_owned()
        } else {
            list(&self.args)
        };
        let mut over = Vec::new();
        if !self.partition.is_empty() {
            over.push(format!("PARTITION BY {}", list(&self.partition)));
        }
        if !self.order.is_empty() {
            over.push(format!("ORDER BY {}", terms(&self.order)));
        }
        if let Some(f) = &self.frame {
            over.push(format!("{} BETWEEN {}{}", f.unit, f.bounds, f.exclude));
        }
        format!(
            "{}({args}){} OVER ({})",
            self.func,
            match &self.filter {
                Some(f) => format!(" FILTER (WHERE {})", f.sql()),
                None => String::new(),
            },
            over.join(" ")
        )
    }

    /// Whether its value is certain: one that depends on which of two rows its ORDER BY leaves tied
    /// comes first has an ORDER BY that leaves none (a rowid of each table), and a sum is of small
    /// integers, so a frame that slides rounds nothing.
    fn certain(&self, from: &[From]) -> bool {
        let tied = matches!(
            self.func,
            "row_number" | "ntile" | "lag" | "lead" | "first_value" | "last_value" | "nth_value"
        ) || self.frame.as_ref().is_some_and(|f| f.unit == "ROWS");
        let sums = matches!(self.func, "sum" | "avg" | "total");
        (!tied
            || from.iter().all(|f| {
                self.order
                    .iter()
                    .any(|(e, _)| matches!(e, E::Col(a, c) if *a == f.alias && c == "rowid"))
            }))
            && (!sums || self.args.first().is_some_and(small))
    }
}

/// Whether an expression is a small integer or NULL: `(CAST(e AS INTEGER) % n)`, or a literal.
fn small(e: &E) -> bool {
    match e {
        E::Lit(V::Int(i)) => i.unsigned_abs() < 1 << 20,
        E::Lit(V::Null) => true,
        E::Bin("%", a, b) => {
            matches!(**a, E::Cast(_, "INTEGER")) && matches!(**b, E::Lit(V::Int(1..=1000)))
        }
        _ => false,
    }
}

/// The affinity an expression has, where its own text decides it (datatype3.html 3.2): a CAST's type,
/// or none for what any operator or function makes; `None` for a column's, a subquery's, or what
/// carries one of those through.
fn affinity(e: &E) -> Option<&'static str> {
    match e {
        E::Cast(_, t) => Some(t),
        E::Col(..) | E::Collate(..) | E::Scalar(_) | E::Star => None,
        E::Fn(f, _) if matches!(f.as_str(), "likely" | "unlikely" | "likelihood") => None,
        _ => Some(""),
    }
}

/// Whether two values of the expression that compare equal are the same value, so a DISTINCT, a GROUP
/// BY or a compound that keeps one of several equal ones keeps a certain one.
fn exact(e: &E) -> bool {
    match e {
        E::Lit(V::Real(_)) => false,
        E::Lit(_) | E::Cast(_, "INTEGER") => true,
        E::Fn(f, _) => f == "quote",
        E::Agg(f, _, args) => f == "count" || args.iter().all(|a| exact(a) || small(a)),
        _ => false,
    }
}

impl E {
    pub fn sql(&self) -> String {
        match self {
            E::Col(t, c) if t.is_empty() => c.clone(),
            E::Col(t, c) => format!("{t}.{c}"),
            E::Star => "*".into(),
            E::Win(w) => w.sql(),
            E::RowBin(op, a, b) => format!("(({}) {op} ({}))", list(a), list(b)),
            E::RowBetween(a, lo, hi, not) => format!(
                "(({}) {}BETWEEN ({}) AND ({}))",
                list(a),
                if *not { "NOT " } else { "" },
                list(lo),
                list(hi)
            ),
            E::RowSel(op, a, q) => format!("(({}) {op} ({}))", list(a), q.sql()),
            E::Lit(v) => v.sql(),
            // a space after every unary operator: `- -1` stays an expression, `--1` would be a comment
            E::Un(op, a) => format!("({op} {})", a.sql()),
            E::Bin(op, a, b) => format!("({} {op} {})", a.sql(), b.sql()),
            E::Fn(f, args) => format!(
                "{f}({})",
                args.iter().map(E::sql).collect::<Vec<_>>().join(", ")
            ),
            E::Agg(f, distinct, args) => format!(
                "{f}({}{})",
                if *distinct { "DISTINCT " } else { "" },
                if args.is_empty() {
                    "*".into()
                } else {
                    args.iter().map(E::sql).collect::<Vec<_>>().join(", ")
                }
            ),
            E::Case(base, whens, els) => {
                let mut s = String::from("(CASE");
                if let Some(b) = base {
                    s.push(' ');
                    s.push_str(&b.sql());
                }
                for (w, t) in whens {
                    s.push_str(&format!(" WHEN {} THEN {}", w.sql(), t.sql()));
                }
                if let Some(e) = els {
                    s.push_str(&format!(" ELSE {}", e.sql()));
                }
                s.push_str(" END)");
                s
            }
            E::Cast(a, t) => format!("CAST({} AS {t})", a.sql()),
            E::Collate(a, c) => format!("({} COLLATE {c})", a.sql()),
            E::In(a, list, not) => format!(
                "({} {}IN ({}))",
                a.sql(),
                if *not { "NOT " } else { "" },
                list.iter().map(E::sql).collect::<Vec<_>>().join(", ")
            ),
            E::Between(a, lo, hi, not) => format!(
                "({} {}BETWEEN {} AND {})",
                a.sql(),
                if *not { "NOT " } else { "" },
                lo.sql(),
                hi.sql()
            ),
            E::Null(a, not) => format!("({} {})", a.sql(), if *not { "NOTNULL" } else { "ISNULL" }),
            E::Exists(s, not) => {
                format!("({}EXISTS ({}))", if *not { "NOT " } else { "" }, s.sql())
            }
            E::Scalar(s) => format!("({})", s.sql()),
            E::InSel(a, s, not) => format!(
                "({} {}IN ({}))",
                a.sql(),
                if *not { "NOT " } else { "" },
                s.sql()
            ),
        }
    }

    /// Whether the expression holds an aggregate call outside any subquery.
    pub fn has_agg(&self) -> bool {
        let mut found = false;
        self.walk(&mut |e| found |= matches!(e, E::Agg(..)));
        found
    }

    /// Every node, depth first, not entering subqueries.
    pub fn walk(&self, f: &mut dyn FnMut(&E)) {
        f(self);
        for c in self.children() {
            c.walk(f);
        }
    }

    pub fn children(&self) -> Vec<&E> {
        match self {
            E::Col(..) | E::Lit(_) | E::Star | E::Exists(..) | E::Scalar(_) => vec![],
            E::Un(_, a) | E::Cast(a, _) | E::Collate(a, _) | E::Null(a, _) | E::InSel(a, _, _) => {
                vec![a]
            }
            E::Bin(_, a, b) => vec![a, b],
            E::Win(w) => w
                .args
                .iter()
                .chain(w.filter.iter())
                .chain(w.partition.iter())
                .chain(w.order.iter().map(|(e, _)| e))
                .collect(),
            E::RowBin(_, a, b) => a.iter().chain(b).collect(),
            E::RowBetween(a, lo, hi, _) => a.iter().chain(lo).chain(hi).collect(),
            E::RowSel(_, a, _) => a.iter().collect(),
            E::Fn(_, args) | E::Agg(_, _, args) => args.iter().collect(),
            E::Case(base, whens, els) => {
                let mut v: Vec<&E> = base.iter().map(|b| &**b).collect();
                for (w, t) in whens {
                    v.push(w);
                    v.push(t);
                }
                v.extend(els.iter().map(|e| &**e));
                v
            }
            E::In(a, list, _) => std::iter::once(&**a).chain(list.iter()).collect(),
            E::Between(a, lo, hi, _) => vec![a, lo, hi],
        }
    }

    pub fn children_mut(&mut self) -> Vec<&mut E> {
        match self {
            E::Col(..) | E::Lit(_) | E::Star | E::Exists(..) | E::Scalar(_) => vec![],
            E::Un(_, a) | E::Cast(a, _) | E::Collate(a, _) | E::Null(a, _) | E::InSel(a, _, _) => {
                vec![a]
            }
            E::Bin(_, a, b) => vec![a, b],
            E::Win(w) => w
                .args
                .iter_mut()
                .chain(w.filter.iter_mut())
                .chain(w.partition.iter_mut())
                .chain(w.order.iter_mut().map(|(e, _)| e))
                .collect(),
            E::RowBin(_, a, b) => a.iter_mut().chain(b).collect(),
            E::RowBetween(a, lo, hi, _) => a.iter_mut().chain(lo).chain(hi).collect(),
            E::RowSel(_, a, _) => a.iter_mut().collect(),
            E::Fn(_, args) | E::Agg(_, _, args) => args.iter_mut().collect(),
            E::Case(base, whens, els) => {
                let mut v: Vec<&mut E> = base.iter_mut().map(|b| &mut **b).collect();
                for (w, t) in whens.iter_mut() {
                    v.push(w);
                    v.push(t);
                }
                v.extend(els.iter_mut().map(|e| &mut **e));
                v
            }
            E::In(a, list, _) => std::iter::once(&mut **a).chain(list.iter_mut()).collect(),
            E::Between(a, lo, hi, _) => vec![a, lo, hi],
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Join {
    First,
    Inner,
    Left,
    Right,
    Full,
    Cross,
    Comma,
}

#[derive(Clone, Debug)]
pub enum Hint {
    None,
    NotIndexed,
    IndexedBy(String),
}

#[derive(Clone, Debug)]
pub struct From {
    /// A table, a view or a CTE by name; `sub` a derived table in its place.
    pub table: String,
    pub sub: Option<Box<Sel>>,
    pub alias: String,
    /// Its columns' names.
    pub cols: Vec<String>,
    /// Whether it has a rowid, the same in both engines.
    pub rowid: bool,
    pub join: Join,
    pub on: Option<E>,
    pub using: Vec<String>,
    pub natural: bool,
    pub hint: Hint,
}

impl From {
    pub fn table(t: &Table, alias: String) -> From {
        From {
            table: t.name.clone(),
            sub: None,
            alias,
            cols: t.cols.iter().map(|c| c.name.clone()).collect(),
            rowid: t.kind == Kind::Rowid,
            join: Join::First,
            on: None,
            using: vec![],
            natural: false,
            hint: Hint::None,
        }
    }

    /// Whether a table of the schema, which a hint (INDEXED BY, NOT INDEXED) may name.
    pub fn is_table(&self, schema: &Schema) -> bool {
        self.sub.is_none()
            && schema
                .tables
                .iter()
                .any(|t| t.name == self.table && t.kind.is_table())
    }
}

/// `name(c0, ..) AS [NOT MATERIALIZED] (body)`.
#[derive(Clone, Debug)]
pub struct Cte {
    pub name: String,
    pub recursive: bool,
    pub hint: &'static str,
    pub body: Sel,
}

/// The most rows a recursive CTE that ends by UNION's own duplicates may make, far past what its
/// arithmetic allows: an engine that keeps the duplicates ends there instead of never.
pub const RECURSION_ROWS: i64 = 200;

/// A SELECT, or a compound of them (`arms`, each with the operator before it), with its CTEs. `order`
/// and `limit` only where they leave the answer's rows certain: a LIMIT comes with an ORDER BY over
/// every column it returns.
#[derive(Clone, Debug, Default)]
pub struct Sel {
    pub with: Vec<Cte>,
    pub distinct: bool,
    pub cols: Vec<E>,
    /// Whether its columns are named c0, c1 and so on, for the query that reads them.
    pub named: bool,
    pub from: Vec<From>,
    pub wh: Option<E>,
    pub group: Vec<E>,
    pub having: Option<E>,
    pub arms: Vec<(&'static str, Sel)>,
    pub order: Vec<(E, bool)>,
    pub limit: Option<(i64, i64)>,
}

impl Sel {
    /// Its own expressions, not its arms', its sources' or its subqueries'.
    fn exprs(&self) -> impl Iterator<Item = &E> {
        self.cols
            .iter()
            .chain(self.from.iter().filter_map(|f| f.on.as_ref()))
            .chain(self.wh.iter())
            .chain(self.group.iter())
            .chain(self.having.iter())
            .chain(self.order.iter().map(|(e, _)| e))
    }

    /// Every query in it: itself, its CTEs, its derived tables, its arms and its subqueries.
    pub fn each(&self, f: &mut dyn FnMut(&Sel)) {
        f(self);
        for c in &self.with {
            c.body.each(f);
        }
        for q in self.from.iter().filter_map(|x| x.sub.as_ref()) {
            q.each(f);
        }
        for (_, q) in &self.arms {
            q.each(f);
        }
        for e in self.exprs() {
            e.walk(&mut |x| match x {
                E::Exists(q, _) | E::Scalar(q) | E::InSel(_, q, _) | E::RowSel(_, _, q) => {
                    q.each(f);
                }
                _ => {}
            });
        }
    }

    /// The `k`th query `sources` counts: its CTEs', derived tables' and arms' queries, and theirs.
    pub fn source_mut(&mut self, k: &mut usize) -> Option<&mut Sel> {
        let Sel {
            with, from, arms, ..
        } = self;
        let inner = with
            .iter_mut()
            .map(|c| &mut c.body)
            .chain(from.iter_mut().filter_map(|f| f.sub.as_deref_mut()))
            .chain(arms.iter_mut().map(|(_, q)| q));
        for q in inner {
            if *k == 0 {
                return Some(q);
            }
            *k -= 1;
            if let Some(found) = q.source_mut(k) {
                return Some(found);
            }
        }
        None
    }

    pub fn sources(&self) -> usize {
        let inner = self
            .with
            .iter()
            .map(|c| &c.body)
            .chain(self.from.iter().filter_map(|f| f.sub.as_deref()))
            .chain(self.arms.iter().map(|(_, q)| q));
        inner.map(|q| 1 + q.sources()).sum()
    }

    /// Whether its answer keeps one of several equal values, any of them: what it's compared by.
    pub fn loose(&self) -> bool {
        self.distinct
            || self.aggregates()
            || self
                .arms
                .iter()
                .any(|(op, q)| *op != "UNION ALL" || q.loose())
            || self.cols.iter().any(|c| {
                let mut found = false;
                c.walk(&mut |e| found |= matches!(e, E::Win(w) if matches!(w.func, "min" | "max")));
                found
            })
    }

    /// Whether the rows another query reads from it are certain, each value's own bytes and class.
    fn source_certain(&self) -> bool {
        let arms = || std::iter::once(self).chain(self.arms.iter().map(|(_, q)| q));
        let dedupes = self.arms.iter().any(|(op, _)| *op != "UNION ALL");
        // Which arm a compound's column takes its affinity from is indeterminate (datatype3.html 3.3):
        // its arms' are one affinity.
        let affinities = self.arms.is_empty()
            || (0..self.cols.len()).all(|k| {
                let mut of = arms().map(|q| q.cols.get(k).and_then(affinity));
                let first = of.next().flatten();
                first.is_some() && of.all(|a| a == first)
            });
        self.certain()
            && affinities
            && self.limit.is_none_or(|l| l == (RECURSION_ROWS, 0))
            && arms().all(|q| {
                (!(dedupes || q.distinct) || q.cols.iter().all(exact))
                    && q.group.iter().all(exact)
                    && q.cols.iter().all(|c| !c.has_agg() || exact(c))
                    && q.cols.iter().all(
                        |c| !matches!(c, E::Win(w) if matches!(w.func, "min" | "max") && !w.args.iter().all(exact)),
                    )
            })
    }

    /// Adds the features it shows to `out`: `views` and the table kinds by the schema's names.
    pub fn features(&self, schema: &Schema, out: &mut std::collections::BTreeSet<&'static str>) {
        self.each(&mut |q| {
            for (op, _) in &q.arms {
                out.insert(match *op {
                    "UNION ALL" => "union all",
                    "INTERSECT" => "intersect",
                    "EXCEPT" => "except",
                    _ => "union",
                });
            }
            for c in &q.with {
                out.insert(if c.recursive { "recursive" } else { "with" });
            }
            for f in &q.from {
                if f.sub.is_some() {
                    out.insert("derived");
                }
                if f.natural {
                    out.insert("natural");
                }
                if !f.using.is_empty() {
                    out.insert("using");
                }
                for t in schema.tables.iter().filter(|t| t.name == f.table) {
                    if t.kind == Kind::View {
                        out.insert("view");
                    }
                    if t.kind == Kind::WithoutRowid {
                        out.insert("without rowid");
                    }
                    if t.strict {
                        out.insert("strict");
                    }
                }
            }
            for e in q.exprs() {
                e.walk(&mut |x| match x {
                    E::Win(_) => {
                        out.insert("window");
                    }
                    E::RowBin(..) | E::RowBetween(..) | E::RowSel(..) => {
                        out.insert("row value");
                    }
                    E::Star => {
                        out.insert("star");
                    }
                    _ => {}
                });
            }
        });
    }

    /// Whether every value it returns is certain: in a grouped or aggregate query, no column read
    /// outside an aggregate unless the expression is one it groups by (a bare column reads any row
    /// of its group, and two engines, or two plans, may read different ones).
    pub fn certain(&self) -> bool {
        let windows = self
            .cols
            .iter()
            .chain(self.order.iter().map(|(e, _)| e))
            .all(|c| {
                let mut ok = true;
                c.walk(&mut |e| ok &= !matches!(e, E::Win(w) if !w.certain(&self.from)));
                ok
            });
        if !windows
            || !self.with.iter().all(|c| c.body.source_certain())
            || !self
                .from
                .iter()
                .all(|f| f.sub.as_ref().is_none_or(|q| q.source_certain()))
            || !self.arms.iter().all(|(_, q)| q.certain())
        {
            return false;
        }
        if !self.aggregates() {
            return true;
        }
        let grouped: Vec<String> = self.group.iter().map(E::sql).collect();
        let outer: Vec<&str> = self.from.iter().map(|f| f.alias.as_str()).collect();
        // A subquery reads the group's row too: a column of this query's tables in it, not one
        // grouped by, is bare (`HAVING (SELECT min(s.c) FROM t s WHERE a0.c1 <> 3)`).
        fn bare(e: &E, grouped: &[String], outer: &[&str], inside: bool) -> bool {
            if grouped.contains(&e.sql()) || (!inside && matches!(e, E::Agg(..))) {
                return false;
            }
            match e {
                E::Col(alias, _) => !inside || alias.is_empty() || outer.contains(&alias.as_str()),
                E::Star => true,
                E::Exists(q, _) | E::Scalar(q) => sel_bare(q, grouped, outer),
                E::InSel(a, q, _) => bare(a, grouped, outer, inside) || sel_bare(q, grouped, outer),
                E::RowSel(_, xs, q) => {
                    xs.iter().any(|a| bare(a, grouped, outer, inside))
                        || sel_bare(q, grouped, outer)
                }
                _ => e
                    .children()
                    .into_iter()
                    .any(|c| bare(c, grouped, outer, inside)),
            }
        }
        fn sel_bare(q: &Sel, grouped: &[String], outer: &[&str]) -> bool {
            q.cols
                .iter()
                .chain(q.from.iter().filter_map(|f| f.on.as_ref()))
                .chain(q.wh.iter())
                .chain(q.group.iter())
                .chain(q.having.iter())
                .chain(q.order.iter().map(|(e, _)| e))
                .any(|e| bare(e, grouped, outer, true))
        }
        !self
            .cols
            .iter()
            .chain(self.having.iter())
            .chain(self.order.iter().map(|(e, _)| e))
            .any(|e| bare(e, &grouped, &outer, false))
    }

    /// Its CTEs, its SELECT and its arms, then the ORDER BY and LIMIT of the whole.
    pub fn sql(&self) -> String {
        let mut s = String::new();
        if !self.with.is_empty() {
            s.push_str(if self.with.iter().any(|c| c.recursive) {
                "WITH RECURSIVE "
            } else {
                "WITH "
            });
            let ctes: Vec<String> = self
                .with
                .iter()
                .map(|c| {
                    let cols: Vec<String> =
                        (0..c.body.cols.len()).map(|i| format!("c{i}")).collect();
                    format!(
                        "{}({}) AS {}({})",
                        c.name,
                        cols.join(", "),
                        c.hint,
                        c.body.sql()
                    )
                })
                .collect();
            s.push_str(&ctes.join(", "));
            s.push(' ');
        }
        self.core(&mut s);
        for (op, q) in &self.arms {
            s.push_str(&format!(" {op} "));
            q.core(&mut s);
        }
        if !self.order.is_empty() {
            s.push_str(&format!(" ORDER BY {}", terms(&self.order)));
        }
        if let Some((l, o)) = self.limit {
            s.push_str(&format!(" LIMIT {l} OFFSET {o}"));
        }
        s
    }

    fn core(&self, s: &mut String) {
        s.push_str("SELECT ");
        if self.distinct {
            s.push_str("DISTINCT ");
        }
        let cols: Vec<String> = self
            .cols
            .iter()
            .enumerate()
            .map(|(i, e)| {
                if self.named {
                    format!("{} AS c{i}", e.sql())
                } else {
                    e.sql()
                }
            })
            .collect();
        s.push_str(&cols.join(", "));
        if !self.from.is_empty() {
            s.push_str(" FROM ");
            for (i, f) in self.from.iter().enumerate() {
                let join = match f.join {
                    Join::First => "",
                    Join::Inner => " JOIN ",
                    Join::Left => " LEFT JOIN ",
                    Join::Right => " RIGHT JOIN ",
                    Join::Full => " FULL JOIN ",
                    Join::Cross => " CROSS JOIN ",
                    Join::Comma => ", ",
                };
                if i > 0 {
                    if f.natural && !matches!(f.join, Join::First | Join::Comma) {
                        s.push_str(" NATURAL");
                    }
                    s.push_str(join);
                }
                match &f.sub {
                    Some(q) => s.push_str(&format!("({}) AS {}", q.sql(), f.alias)),
                    None => s.push_str(&format!("{} AS {}", f.table, f.alias)),
                }
                match &f.hint {
                    Hint::None => {}
                    Hint::NotIndexed => s.push_str(" NOT INDEXED"),
                    Hint::IndexedBy(ix) => s.push_str(&format!(" INDEXED BY {ix}")),
                }
                if let Some(on) = &f.on {
                    s.push_str(&format!(" ON {}", on.sql()));
                } else if i > 0 && !f.using.is_empty() {
                    s.push_str(&format!(" USING ({})", f.using.join(", ")));
                }
            }
        }
        if let Some(w) = &self.wh {
            s.push_str(&format!(" WHERE {}", w.sql()));
        }
        if !self.group.is_empty() {
            s.push_str(&format!(
                " GROUP BY {}",
                self.group.iter().map(E::sql).collect::<Vec<_>>().join(", ")
            ));
        }
        if let Some(h) = &self.having {
            s.push_str(&format!(" HAVING {}", h.sql()));
        }
    }

    /// Whether it groups or aggregates (its columns then read one row of a group, or none).
    pub fn aggregates(&self) -> bool {
        !self.group.is_empty() || self.having.is_some() || self.cols.iter().any(E::has_agg)
    }
}

// ---------------------------------------------------------------- schema

#[derive(Clone, Debug)]
pub struct Col {
    pub name: String,
    pub decl: &'static str,
    pub collate: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Rowid,
    WithoutRowid,
    /// The table triggers write to: its rows' order, so their rowids, may be any.
    Log,
    View,
    Cte,
}

impl Kind {
    pub fn is_table(self) -> bool {
        matches!(self, Kind::Rowid | Kind::WithoutRowid | Kind::Log)
    }
}

#[derive(Clone, Debug)]
pub struct Table {
    pub name: String,
    pub cols: Vec<Col>,
    pub kind: Kind,
    pub strict: bool,
    /// The columns its PRIMARY KEY or a UNIQUE constraint holds alone, for an UPSERT's target.
    pub unique: Vec<String>,
    /// The column that is its rowid's alias (an INTEGER PRIMARY KEY).
    pub alias: Option<usize>,
}

impl Table {
    /// The query that shows its rows, the same in both engines.
    pub fn rows_query(&self) -> String {
        match self.kind {
            Kind::Rowid => format!("SELECT rowid, * FROM {}", self.name),
            _ => format!("SELECT * FROM {}", self.name),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Schema {
    pub tables: Vec<Table>,
    pub indexes: Vec<(String, String)>,
}

/// A function the expressions may call: its arities as `pragma_function_list` gives them (-1 for
/// any number, -(k + 2) for at least k), and whether it aggregates.
#[derive(Clone, Debug)]
pub struct Func {
    pub name: String,
    pub nargs: Vec<i32>,
    pub aggregate: bool,
    /// The arities at which the name calls an aggregate or window function instead (`min(x)`,
    /// `max(x)`), which a scalar call never takes.
    pub agg_nargs: Vec<i32>,
}

/// Functions whose answer isn't the same each call, reads the connection's state, or depends on the
/// order rows arrive in (concatenating aggregates without an ORDER BY of their own): no oracle can
/// compare them. The date functions stay: given arguments (never none, which is now) they're fixed.
const UNSTABLE: &[&str] = &[
    "random",
    "randomblob",
    "changes",
    "total_changes",
    "last_insert_rowid",
    "sqlite_offset",
    "load_extension",
    "sqlite_compileoption_get",
    "sqlite_compileoption_used",
    "sqlite_source_id",
    "sqlite_version",
    "group_concat",
    "string_agg",
    "json_group_array",
    "json_group_object",
    "jsonb_group_array",
    "jsonb_group_object",
];

/// The inventory's functions with the arities SQLite gives them (pragma_function_list), the ones
/// whose answers aren't stable left out.
pub fn functions(
    features: &serde_json::Value,
    sqlite: &mut super::engines::Sqlite,
) -> anyhow::Result<Vec<Func>> {
    use super::engines::Engine;
    let names: std::collections::HashSet<String> = features["features"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| {
            f["kind"]
                .as_str()
                .is_some_and(|k| k.starts_with("function.") && k != "function.window")
        })
        .filter_map(|f| f["name"].as_str().map(str::to_owned))
        .collect();
    let mut by: std::collections::BTreeMap<String, Func> = std::collections::BTreeMap::new();
    let mut windows: std::collections::BTreeMap<String, Vec<i32>> =
        std::collections::BTreeMap::new();
    for row in sqlite
        .run("SELECT name, type, narg FROM pragma_function_list")
        .map_err(|e| anyhow::anyhow!(e))?
    {
        let (Some(V::Text(name)), Some(V::Text(kind)), Some(V::Int(narg))) =
            (row.first(), row.get(1), row.get(2))
        else {
            continue;
        };
        if !names.contains(name) || UNSTABLE.contains(&name.as_str()) {
            continue;
        }
        let narg = *narg as i32;
        if kind == "w" {
            windows.entry(name.clone()).or_default().push(narg);
            continue;
        }
        let f = by.entry(name.clone()).or_insert(Func {
            name: name.clone(),
            nargs: vec![],
            aggregate: kind == "a",
            agg_nargs: vec![],
        });
        if !f.nargs.contains(&narg) {
            f.nargs.push(narg);
        }
    }
    for f in by.values_mut().filter(|f| !f.aggregate) {
        f.agg_nargs = windows.remove(&f.name).unwrap_or_default();
    }
    Ok(by.into_values().collect())
}

const DECLS: &[&str] = &[
    "INTEGER",
    "INT",
    "TEXT",
    "VARCHAR(10)",
    "REAL",
    "DOUBLE",
    "NUMERIC",
    "DECIMAL(10,2)",
    "BLOB",
    "",
    "BOOLEAN",
];
const COLLATIONS: &[&str] = &["BINARY", "NOCASE", "RTRIM"];

/// Values of every storage class and their edges: what affinity, comparison and collation decide.
/// The BLOBs are well-formed UTF-8 (the high one is U+00FF's bytes, not FF alone): what SQLite
/// answers for ill-formed text it doesn't promise (docs/design/semantics.md, decision 1).
pub fn literal(r: &mut Rng) -> V {
    match r.below(10) {
        0 => V::Null,
        1..=3 => V::Int(*r.pick(&[
            0,
            1,
            -1,
            2,
            3,
            7,
            10,
            100,
            -100,
            255,
            i64::MAX,
            i64::MIN,
            9_007_199_254_740_993,
        ])),
        4 | 5 => V::Real(*r.pick(&[0.0, 0.5, 1.5, -2.5, 3.0, 1e15, 1e308, -1e-308, 2.0, 0.1])),
        6..=8 => V::Text(
            r.pick(&[
                "", " ", "a", "A", "abc", "ABC", "0", "1", "1.5", "10", "-1", "a ", " a", "%", "_",
                "true", "1e3", "x", "é", "0x10",
            ])
            .to_string(),
        ),
        _ => V::Blob(
            r.pick(&[
                &[][..],
                &[0u8][..],
                &[0x41][..],
                &[0xc3, 0xbf][..],
                &[1, 2][..],
            ])
            .to_vec(),
        ),
    }
}

/// A literal of the class a STRICT column of type `decl` takes, most of the time.
fn typed(r: &mut Rng, decl: &str) -> V {
    if r.chance(0.04) {
        return literal(r);
    }
    for _ in 0..40 {
        let v = literal(r);
        let fits = match (decl, &v) {
            (_, V::Null) => r.chance(0.3),
            ("INT" | "INTEGER", V::Int(_))
            | ("REAL", V::Int(_) | V::Real(_))
            | ("TEXT", V::Text(_))
            | ("BLOB", V::Blob(_))
            | ("ANY", _) => true,
            _ => false,
        };
        if fits {
            return v;
        }
    }
    V::Null
}

/// The log's name: what triggers write to.
const LOG: &str = "lg";

/// A random database: its statements, in order, and the schema they make.
pub fn database(r: &mut Rng, funcs: &[Func]) -> (Vec<String>, Schema) {
    let mut stmts = Vec::new();
    let mut schema = Schema::default();
    for t in 0..1 + r.below(3) {
        let name = format!("t{t}");
        let ncols = 1 + r.below(4);
        let mut cols = Vec::new();
        let mut defs = Vec::new();
        let strict = r.chance(0.15);
        let without_rowid = r.chance(0.15);
        let pk = r.below(if without_rowid { ncols } else { ncols + 2 });
        // a composite key, its parts the first columns
        let composite = without_rowid && ncols > 1 && r.chance(0.4);
        let mut unique = Vec::new();
        for c in 0..ncols {
            let col = Col {
                name: format!("c{c}"),
                decl: if strict {
                    r.pick(&["INTEGER", "INT", "REAL", "TEXT", "BLOB", "ANY"])
                } else {
                    r.pick(DECLS)
                },
                collate: if r.chance(0.3) {
                    Some(*r.pick(COLLATIONS))
                } else {
                    None
                },
            };
            let mut def = format!("{} {}", col.name, col.decl);
            if let Some(co) = col.collate {
                def.push_str(&format!(" COLLATE {co}"));
            }
            if c == pk && !composite {
                def.push_str(" PRIMARY KEY");
                unique.push(col.name.clone());
            } else if r.chance(0.15) {
                def.push_str(" UNIQUE");
                unique.push(col.name.clone());
            }
            if r.chance(0.1) {
                def.push_str(" NOT NULL");
            }
            defs.push(def);
            cols.push(col);
        }
        if composite {
            defs.push("PRIMARY KEY(c0, c1)".into());
        }
        let options: Vec<&str> = [(strict, "STRICT"), (without_rowid, "WITHOUT ROWID")]
            .into_iter()
            .filter(|(is, _)| *is)
            .map(|(_, option)| option)
            .collect();
        stmts.push(format!(
            "CREATE TABLE {name}({}){}{}",
            defs.join(", "),
            if options.is_empty() { "" } else { " " },
            options.join(", ")
        ));
        // the rowid's alias (an INTEGER PRIMARY KEY)
        let alias = if !without_rowid && pk < ncols && cols[pk].decl == "INTEGER" {
            Some(pk)
        } else {
            None
        };
        schema.tables.push(Table {
            name,
            cols,
            kind: if without_rowid {
                Kind::WithoutRowid
            } else {
                Kind::Rowid
            },
            strict,
            unique,
            alias,
        });
    }
    let log = r.chance(0.35);
    if log {
        stmts.push(format!("CREATE TABLE {LOG}(c0, c1, c2)"));
        let names: Vec<usize> = (0..1 + r.below(2)).collect();
        for n in names {
            let t = schema.tables[r.below(schema.tables.len())].clone();
            stmts.push(trigger(r, &format!("g{n}"), &t, &schema, funcs));
        }
    }
    let tables = schema.tables.clone();
    for t in &tables {
        let most = if r.chance(0.3) { 12 } else { 5 };
        for _ in 0..r.below(most) {
            let n = 1 + r.below(3);
            // the rowid's alias never takes the largest rowid: a NULL after it gets a rowid chosen
            // at random, a different one in each engine
            let rows: Vec<String> = (0..n)
                .map(|_| {
                    let row: Vec<String> = t
                        .cols
                        .iter()
                        .enumerate()
                        .map(|(c, col)| {
                            let v = if t.strict {
                                typed(r, col.decl)
                            } else {
                                literal(r)
                            };
                            match v {
                                V::Int(i64::MAX) if Some(c) == t.alias => V::Int(5).sql(),
                                v => v.sql(),
                            }
                        })
                        .collect();
                    format!("({})", row.join(", "))
                })
                .collect();
            let (or, upsert) = match r.below(10) {
                0..=3 => ("OR IGNORE ", String::new()),
                4 => ("OR REPLACE ", String::new()),
                5..=6 => ("", upsert(r, t, funcs)),
                _ => ("", String::new()),
            };
            stmts.push(format!(
                "INSERT {or}INTO {} VALUES {}{upsert}",
                t.name,
                rows.join(", ")
            ));
        }
    }
    for i in 0..r.below(4) {
        let t = schema.tables[r.below(schema.tables.len())].clone();
        let mut terms = Vec::new();
        for _ in 0..1 + r.below(2.min(t.cols.len())) {
            let c = r.pick(&t.cols).name.clone();
            let mut term = if r.chance(0.2) {
                format!("({c} + 1)")
            } else {
                c
            };
            if r.chance(0.2) {
                term.push_str(&format!(" COLLATE {}", r.pick(COLLATIONS)));
            }
            if r.chance(0.3) {
                term.push_str(" DESC");
            }
            terms.push(term);
        }
        let name = format!("i{i}");
        let partial = if r.chance(0.2) {
            format!(" WHERE {} IS NOT NULL", r.pick(&t.cols).name)
        } else {
            String::new()
        };
        stmts.push(format!(
            "CREATE {}INDEX {name} ON {}({}){partial}",
            if r.chance(0.2) { "UNIQUE " } else { "" },
            t.name,
            terms.join(", ")
        ));
        schema.indexes.push((name, t.name.clone()));
    }
    if r.chance(0.3) {
        let t = schema.tables[r.below(schema.tables.len())].clone();
        stmts.push(format!(
            "UPDATE {} SET {} = {} WHERE {} IS NOT NULL",
            t.name,
            r.pick(&t.cols).name,
            literal(r).sql(),
            r.pick(&t.cols).name
        ));
    }
    if r.chance(0.2) {
        let t = schema.tables[r.below(schema.tables.len())].clone();
        stmts.push(format!(
            "DELETE FROM {} WHERE {} = {}",
            t.name,
            r.pick(&t.cols).name,
            literal(r).sql()
        ));
    }
    if r.chance(0.35) {
        for n in 0..1 + r.below(2) {
            let name = format!("v{n}");
            let mut q = source(r, &schema, funcs);
            let listed = r.chance(0.3);
            q.named = !listed;
            let cols: Vec<String> = (0..q.cols.len()).map(|i| format!("c{i}")).collect();
            stmts.push(format!(
                "CREATE VIEW {name}{} AS {}",
                if listed {
                    format!("({})", cols.join(", "))
                } else {
                    String::new()
                },
                q.sql()
            ));
            if log && r.chance(0.4) {
                stmts.push(format!(
                    "CREATE TRIGGER o{n} INSTEAD OF INSERT ON {name} BEGIN INSERT INTO {LOG} VALUES ('{name}', new.c0, new.c{}); END",
                    cols.len() - 1
                ));
                let row: Vec<String> = cols.iter().map(|_| literal(r).sql()).collect();
                stmts.push(format!("INSERT INTO {name} VALUES ({})", row.join(", ")));
            }
            schema.tables.push(Table {
                name,
                cols: unnamed(&cols),
                kind: Kind::View,
                strict: false,
                unique: vec![],
                alias: None,
            });
        }
    }
    if log {
        schema.tables.push(Table {
            name: LOG.into(),
            cols: unnamed(&["c0".to_owned(), "c1".to_owned(), "c2".to_owned()]),
            kind: Kind::Log,
            strict: false,
            unique: vec![],
            alias: None,
        });
    }
    if r.chance(0.3) {
        stmts.push("ANALYZE".into());
    }
    (stmts, schema)
}

/// Columns by name alone: a view's, a CTE's, the log's.
fn unnamed(names: &[String]) -> Vec<Col> {
    names
        .iter()
        .map(|n| Col {
            name: n.clone(),
            decl: "",
            collate: None,
        })
        .collect()
}

/// A scope over named rows (an UPSERT's, a trigger's) and no table a subquery could read: what a
/// statement's own table holds halfway through it is the engine's choice.
fn rows_scope<'a>(
    empty: &'a Schema,
    rows: Vec<(String, Vec<String>)>,
    funcs: &'a [Func],
) -> Scope<'a> {
    Scope {
        schema: empty,
        tables: rows,
        funcs,
        aggs: false,
    }
}

/// An INSERT's ON CONFLICT clauses.
fn upsert(r: &mut Rng, t: &Table, funcs: &[Func]) -> String {
    let cols: Vec<String> = t.cols.iter().map(|c| c.name.clone()).collect();
    let empty = Schema::default();
    let s = rows_scope(
        &empty,
        vec![
            ("excluded".to_owned(), cols.clone()),
            (t.name.clone(), cols.clone()),
        ],
        funcs,
    );
    let mut out = String::new();
    let mut targets = t.unique.clone();
    if targets.len() > 1 && r.chance(0.5) {
        targets.reverse();
    }
    if !r.chance(0.3) {
        targets.truncate(1);
    }
    let last = targets.len().saturating_sub(1);
    let bare = targets.is_empty() || r.chance(0.2);
    let mut clauses: Vec<Option<&String>> = targets.iter().map(Some).collect();
    if clauses.is_empty() {
        clauses.push(None);
    }
    for (i, target) in clauses.into_iter().enumerate() {
        let target = match target {
            Some(c) if !(bare && i == last) => format!("({c})"),
            _ => String::new(),
        };
        out.push_str(&format!(" ON CONFLICT{target} DO "));
        if r.chance(0.25) {
            out.push_str("NOTHING");
            continue;
        }
        let sets: Vec<String> = (0..1 + r.below(2))
            .map(|_| format!("{} = {}", r.pick(&cols), s.expr(r, 1).sql()))
            .collect();
        out.push_str(&format!("UPDATE SET {}", sets.join(", ")));
        if r.chance(0.3) {
            out.push_str(&format!(" WHERE {}", s.pred(r, 1).sql()));
        }
    }
    out
}

/// A trigger on the table, writing to the log; one on INSERT (whose rows come in VALUES' order) may
/// change another table too, or raise.
fn trigger(r: &mut Rng, name: &str, t: &Table, schema: &Schema, funcs: &[Func]) -> String {
    let all: Vec<String> = t.cols.iter().map(|c| c.name.clone()).collect();
    let event = r.below(3);
    let before = r.chance(0.4);
    // the value a BEFORE INSERT trigger sees of a rowid not yet chosen is undefined
    // (lang_createtrigger.html)
    let cols: Vec<String> = all
        .iter()
        .enumerate()
        .filter(|(i, _)| !(event == 0 && before && Some(*i) == t.alias))
        .map(|(_, c)| c.clone())
        .collect();
    let (what, rows): (String, &[&str]) = match event {
        0 => ("INSERT".into(), &["new"]),
        1 if r.chance(0.4) => (format!("UPDATE OF {}", r.pick(&all)), &["new", "old"]),
        1 => ("UPDATE".into(), &["new", "old"]),
        _ => ("DELETE".into(), &["old"]),
    };
    let empty = Schema::default();
    let s = rows_scope(
        &empty,
        rows.iter()
            .map(|x| ((*x).to_owned(), cols.clone()))
            .filter(|(_, c)| !c.is_empty())
            .collect(),
        funcs,
    );
    let mut body = vec![format!(
        "INSERT INTO {LOG} VALUES ('{name}', {}, {})",
        s.expr(r, 1).sql(),
        s.expr(r, 1).sql()
    )];
    if event == 0 && r.chance(0.6) {
        let other = r.pick(&schema.tables);
        let (c, k) = (
            r.pick(&other.cols).name.clone(),
            r.pick(&other.cols).name.clone(),
        );
        body.push(match r.below(5) {
            0 => format!(
                "UPDATE {} SET {c} = {} WHERE {k} IS {}",
                other.name,
                s.expr(r, 1).sql(),
                s.expr(r, 1).sql()
            ),
            1 => format!(
                "INSERT OR IGNORE INTO {}({c}) VALUES ({})",
                other.name,
                s.expr(r, 1).sql()
            ),
            2 => format!(
                "DELETE FROM {} WHERE {c} IS {}",
                other.name,
                s.expr(r, 1).sql()
            ),
            3 => format!("SELECT RAISE(IGNORE) WHERE {}", s.pred(r, 1).sql()),
            _ => format!("SELECT RAISE(ABORT, 'no') WHERE {}", s.pred(r, 1).sql()),
        });
    }
    format!(
        "CREATE TRIGGER {name} {} {what} ON {}{}{} BEGIN {}; END",
        if before { "BEFORE" } else { "AFTER" },
        t.name,
        if r.chance(0.5) { " FOR EACH ROW" } else { "" },
        if r.chance(0.3) {
            format!(" WHEN {}", s.pred(r, 1).sql())
        } else {
            String::new()
        },
        body.join("; ")
    )
}

/// The statements' features, by their text.
pub fn statement_features(stmts: &[String], out: &mut std::collections::BTreeSet<&'static str>) {
    for st in stmts {
        for (what, is) in [
            ("trigger", st.starts_with("CREATE TRIGGER ")),
            ("view", st.starts_with("CREATE VIEW ")),
            (
                "upsert",
                st.starts_with("INSERT ") && st.contains(" ON CONFLICT"),
            ),
            (
                "strict",
                st.starts_with("CREATE TABLE ") && st.contains(") STRICT"),
            ),
            (
                "without rowid",
                st.starts_with("CREATE TABLE ") && st.ends_with("WITHOUT ROWID"),
            ),
        ] {
            if is {
                out.insert(what);
            }
        }
    }
}

// ---------------------------------------------------------------- expressions

const ARITH: &[&str] = &["+", "-", "*", "/", "%", "&", "|", "<<", ">>", "||"];
const COMPARE: &[&str] = &[
    "=",
    "==",
    "<>",
    "!=",
    "<",
    "<=",
    ">",
    ">=",
    "IS",
    "IS NOT",
    "IS DISTINCT FROM",
    "IS NOT DISTINCT FROM",
];
const TYPES: &[&str] = &["INTEGER", "TEXT", "REAL", "NUMERIC", "BLOB"];

/// What an expression may read: the FROM's tables (alias, table) and the functions.
pub struct Scope<'a> {
    pub schema: &'a Schema,
    /// The rows it reads: each alias and its columns (no alias: the columns USING or NATURAL joined).
    pub tables: Vec<(String, Vec<String>)>,
    pub funcs: &'a [Func],
    /// Whether aggregates may be called here (a select list or HAVING of an aggregate query).
    pub aggs: bool,
}

impl Scope<'_> {
    fn column(&self, r: &mut Rng) -> Option<E> {
        if self.tables.is_empty() {
            return None;
        }
        let (alias, cols) = r.pick(&self.tables);
        Some(E::Col(alias.clone(), r.pick(cols).clone()))
    }

    pub fn expr(&self, r: &mut Rng, depth: u32) -> E {
        if depth == 0 || r.chance(0.25) {
            return match self.column(r) {
                Some(c) if r.chance(0.6) => c,
                _ => E::Lit(literal(r)),
            };
        }
        let d = depth - 1;
        match r.below(14) {
            0 | 1 => E::Bin(
                r.pick(ARITH),
                Box::new(self.expr(r, d)),
                Box::new(self.expr(r, d)),
            ),
            2 => E::Un(r.pick(&["-", "+", "~"]), Box::new(self.expr(r, d))),
            3..=5 => self.call(r, d),
            6 => E::Case(
                if r.chance(0.3) {
                    Some(Box::new(self.expr(r, d)))
                } else {
                    None
                },
                (0..1 + r.below(2))
                    .map(|_| (self.pred(r, d), self.expr(r, d)))
                    .collect(),
                if r.chance(0.7) {
                    Some(Box::new(self.expr(r, d)))
                } else {
                    None
                },
            ),
            7 => E::Cast(Box::new(self.expr(r, d)), r.pick(TYPES)),
            8 => E::Collate(Box::new(self.expr(r, d)), r.pick(COLLATIONS)),
            9 if self.aggs => self.aggregate(r, d),
            10 => E::Fn(
                "json_extract".into(),
                vec![
                    E::Lit(V::Text(
                        r.pick(&["{\"a\":1,\"b\":[2,3]}", "[1,2,3]", "{\"a\":null}", "\"x\""])
                            .to_string(),
                    )),
                    E::Lit(V::Text(r.pick(&["$.a", "$.b[1]", "$[0]", "$"]).to_string())),
                ],
            ),
            _ => self.pred(r, d),
        }
    }

    fn call(&self, r: &mut Rng, d: u32) -> E {
        let scalars: Vec<&Func> = self.funcs.iter().filter(|f| !f.aggregate).collect();
        if scalars.is_empty() {
            return E::Lit(literal(r));
        }
        let f = *r.pick(&scalars);
        let n = match *r.pick(&f.nargs) {
            -1 => 1 + r.below(3),
            n if n < -1 => (n.unsigned_abs() - 2) as usize + r.below(3),
            n => n.max(0) as usize,
        };
        let n = if f.agg_nargs.contains(&(n as i32)) {
            n + 1
        } else {
            n
        };
        let mut args: Vec<E> = (0..n).map(|_| self.expr(r, d)).collect();
        // likelihood's second argument is a REAL literal from 0.0 to 1.0, or SQLite refuses the call
        if f.name == "likelihood"
            && let Some(p) = args.get_mut(1)
        {
            *p = E::Lit(V::Real(*r.pick(&[0.0, 0.0625, 0.5, 0.9375, 1.0])));
        }
        E::Fn(f.name.clone(), args)
    }

    /// An aggregate call, for a select list or HAVING.
    pub fn expr_agg(&self, r: &mut Rng) -> E {
        self.aggregate(r, 1)
    }

    fn aggregate(&self, r: &mut Rng, d: u32) -> E {
        let aggs: Vec<&Func> = self.funcs.iter().filter(|f| f.aggregate).collect();
        if aggs.is_empty() || r.chance(0.3) {
            return E::Agg("count".into(), false, vec![]);
        }
        let f = *r.pick(&aggs);
        let n = match *r.pick(&f.nargs) {
            -1 => 1,
            n => n.max(1) as usize,
        };
        // an aggregate's own arguments read rows, not aggregates
        let inner = Scope {
            schema: self.schema,
            tables: self.tables.clone(),
            funcs: self.funcs,
            aggs: false,
        };
        E::Agg(
            f.name.clone(),
            n == 1 && r.chance(0.2),
            (0..n)
                .map(|i| {
                    if i == 0 {
                        inner.expr(r, d)
                    } else {
                        E::Lit(literal(r))
                    }
                })
                .collect(),
        )
    }

    /// A predicate: what WHERE, ON, HAVING and CASE WHEN read the truth of.
    pub fn pred(&self, r: &mut Rng, depth: u32) -> E {
        let d = depth.saturating_sub(1);
        // seldom: a query has several predicates, and one that a row value fails tells nothing of the rest
        if depth > 0 && r.chance(0.015) {
            return self.row_value(r, d);
        }
        match r.below(12) {
            0..=3 => E::Bin(
                r.pick(COMPARE),
                Box::new(self.expr(r, d)),
                Box::new(self.expr(r, d)),
            ),
            4 if depth > 0 => E::Bin("AND", Box::new(self.pred(r, d)), Box::new(self.pred(r, d))),
            5 if depth > 0 => E::Bin("OR", Box::new(self.pred(r, d)), Box::new(self.pred(r, d))),
            6 => E::Un("NOT", Box::new(self.pred(r, d))),
            7 => E::In(
                Box::new(self.expr(r, d)),
                (0..1 + r.below(3)).map(|_| self.expr(r, d)).collect(),
                r.chance(0.3),
            ),
            8 => E::Between(
                Box::new(self.expr(r, d)),
                Box::new(self.expr(r, d)),
                Box::new(self.expr(r, d)),
                r.chance(0.3),
            ),
            9 => E::Null(Box::new(self.expr(r, d)), r.chance(0.5)),
            10 => E::Bin(
                r.pick(&["LIKE", "NOT LIKE", "GLOB", "NOT GLOB"]),
                Box::new(self.expr(r, d)),
                Box::new(E::Lit(V::Text(
                    r.pick(&["a%", "%a", "_", "%", "A%", "a*", "*", "[a-z]*", "abc"])
                        .to_string(),
                ))),
            ),
            _ if depth > 0 && !self.schema.tables.is_empty() => self.subquery(r, d),
            _ => self.expr(r, d),
        }
    }

    /// What a subquery reads: a table, or a tenth of the time a view or a CTE, if there is one.
    fn read(&self, r: &mut Rng) -> Table {
        let any = r.chance(0.1);
        let of: Vec<&Table> = self
            .schema
            .tables
            .iter()
            .filter(|t| any || t.kind.is_table())
            .collect();
        (*r.pick(&of)).clone()
    }

    /// Two rows compared, a row between two, a row among a subquery's, or with a subquery's one row.
    fn row_value(&self, r: &mut Rng, d: u32) -> E {
        let n = 2 + r.below(2);
        let row = |r: &mut Rng| (0..n).map(|_| self.expr(r, d)).collect::<Vec<E>>();
        match r.below(5) {
            0 | 1 => E::RowBin(
                r.pick(&["=", "<>", "<", "<=", ">", ">=", "IS", "IS NOT"]),
                row(r),
                row(r),
            ),
            2 => E::RowBetween(row(r), row(r), row(r), r.chance(0.3)),
            _ if self.schema.tables.is_empty() => E::RowBin("=", row(r), row(r)),
            k => {
                let t = self.read(r);
                let alias = format!("s{}", r.below(1000));
                let inner = Scope {
                    schema: self.schema,
                    tables: vec![(
                        alias.clone(),
                        t.cols.iter().map(|c| c.name.clone()).collect(),
                    )],
                    funcs: self.funcs,
                    aggs: false,
                };
                // one row (aggregates, of exact values) for a comparison, any number for IN
                let one = k == 4;
                let cols = (0..n)
                    .map(|_| {
                        let c = E::Col(alias.clone(), r.pick(&t.cols).name.clone());
                        if one {
                            E::Agg(
                                r.pick(&["min", "max", "count"]).to_string(),
                                false,
                                vec![E::Fn("quote".into(), vec![c])],
                            )
                        } else {
                            c
                        }
                    })
                    .collect();
                let q = Sel {
                    cols,
                    from: vec![From::table(&t, alias.clone())],
                    wh: if r.chance(0.6) {
                        Some(inner.pred(r, d))
                    } else {
                        None
                    },
                    ..Sel::default()
                };
                E::RowSel(
                    if one {
                        r.pick(&["=", "<", ">=", "IS", "<>"])
                    } else {
                        r.pick(&["IN", "IN", "NOT IN"])
                    },
                    row(r),
                    Box::new(q),
                )
            }
        }
    }

    /// A window function's call over the query's rows: over `rowids`, the FROM's aliases when each has
    /// a rowid, its ORDER BY may leave no two rows tied, and any function and frame is certain.
    pub fn window(&self, r: &mut Rng, rowids: &[String]) -> E {
        let total = !rowids.is_empty() && r.chance(0.75);
        let mut order: Vec<(E, bool)> = (0..r.below(3))
            .map(|_| (self.expr(r, 1), r.chance(0.4)))
            .collect();
        if total {
            for a in rowids {
                order.push((E::Col(a.clone(), "rowid".into()), r.chance(0.3)));
            }
        }
        let small = |r: &mut Rng| {
            E::Bin(
                "%",
                Box::new(E::Cast(Box::new(self.expr(r, 1)), "INTEGER")),
                Box::new(E::Lit(V::Int(*r.pick(&[7, 100, 1000])))),
            )
        };
        let mut framed = true;
        let (func, args): (&'static str, Vec<E>) = match r.below(if total { 20 } else { 11 }) {
            0 | 1 => ("count", vec![]),
            2 => ("count", vec![self.expr(r, 1)]),
            3 | 4 => ("sum", vec![small(r)]),
            5 => (r.pick(&["avg", "total"]), vec![small(r)]),
            6 | 7 => (r.pick(&["min", "max"]), vec![self.expr(r, 1)]),
            k @ 8..=10 => {
                framed = false;
                (
                    match k {
                        8 => "rank",
                        9 => "dense_rank",
                        _ => r.pick(&["percent_rank", "cume_dist"]),
                    },
                    vec![],
                )
            }
            11 | 12 => {
                framed = false;
                ("row_number", vec![])
            }
            13 => {
                framed = false;
                ("ntile", vec![E::Lit(V::Int(1 + r.below(5) as i64))])
            }
            k @ 14..=16 => {
                framed = false;
                let mut args = vec![self.expr(r, 1)];
                if r.chance(0.5) {
                    args.push(E::Lit(V::Int(r.below(3) as i64)));
                    if r.chance(0.5) {
                        args.push(self.expr(r, 1));
                    }
                }
                (if k == 14 { "lag" } else { "lead" }, args)
            }
            17 => ("first_value", vec![self.expr(r, 1)]),
            18 => ("last_value", vec![self.expr(r, 1)]),
            _ => (
                "nth_value",
                vec![self.expr(r, 1), E::Lit(V::Int(1 + r.below(4) as i64))],
            ),
        };
        let aggregate = matches!(func, "count" | "sum" | "avg" | "total" | "min" | "max");
        let frame = (framed && r.chance(0.6)).then(|| {
            let unit = match r.below(3) {
                0 if total => "ROWS",
                1 if !order.is_empty() => "GROUPS",
                _ => "RANGE",
            };
            let offsets = unit != "RANGE" || order.len() == 1;
            let bound = |r: &mut Rng, k: usize| match k {
                0 => "UNBOUNDED PRECEDING".to_owned(),
                1 => format!("{} PRECEDING", r.below(4)),
                2 => "CURRENT ROW".to_owned(),
                3 => format!("{} FOLLOWING", r.below(4)),
                _ => "UNBOUNDED FOLLOWING".to_owned(),
            };
            let from = if offsets {
                r.below(4)
            } else {
                *r.pick(&[0, 2])
            };
            let to = if offsets {
                from.max(1) + r.below(5 - from.max(1))
            } else {
                *r.pick(&[2, 4])
            };
            Frame {
                unit,
                bounds: format!("{} AND {}", bound(r, from), bound(r, to)),
                exclude: r.pick(&[
                    "",
                    "",
                    "",
                    " EXCLUDE NO OTHERS",
                    " EXCLUDE CURRENT ROW",
                    " EXCLUDE GROUP",
                    " EXCLUDE TIES",
                ]),
            }
        });
        E::Win(Box::new(Win {
            func,
            args,
            filter: if aggregate && r.chance(0.2) {
                Some(self.pred(r, 1))
            } else {
                None
            },
            partition: if r.chance(0.5) {
                (0..1 + r.below(2)).map(|_| self.expr(r, 1)).collect()
            } else {
                vec![]
            },
            order,
            frame,
        }))
    }

    /// EXISTS, IN or a scalar subquery over one table, correlated with the outer query or not.
    fn subquery(&self, r: &mut Rng, d: u32) -> E {
        let t = self.read(r);
        let alias = format!("s{}", r.below(1000));
        let mut tables = vec![(
            alias.clone(),
            t.cols.iter().map(|c| c.name.clone()).collect(),
        )];
        if r.chance(0.5) {
            tables.extend(self.tables.iter().cloned());
        }
        let inner = Scope {
            schema: self.schema,
            tables,
            funcs: self.funcs,
            aggs: false,
        };
        let sel = |cols: Vec<E>, r: &mut Rng| Sel {
            cols,
            from: vec![From::table(&t, alias.clone())],
            wh: if r.chance(0.8) {
                Some(inner.pred(r, d))
            } else {
                None
            },
            ..Sel::default()
        };
        let col = E::Col(alias.clone(), r.pick(&t.cols).name.clone());
        match r.below(3) {
            0 => E::Exists(Box::new(sel(vec![E::Lit(V::Int(1))], r)), r.chance(0.3)),
            1 => E::InSel(
                Box::new(self.expr(r, d)),
                Box::new(sel(vec![col], r)),
                r.chance(0.3),
            ),
            _ => E::Scalar(Box::new(sel(
                vec![E::Agg(
                    r.pick(&["count", "max", "min", "sum"]).to_string(),
                    false,
                    vec![col],
                )],
                r,
            ))),
        }
    }
}

/// A FROM over one to three of the tables, joined by every kind of join; a `rich` one over views, CTEs
/// and derived tables too, and by USING and NATURAL.
pub fn from(r: &mut Rng, schema: &Schema, funcs: &[Func], rich: bool) -> Vec<From> {
    joined(r, schema, funcs, rich, 3, "a")
}

fn joined(
    r: &mut Rng,
    schema: &Schema,
    funcs: &[Func],
    rich: bool,
    most: usize,
    prefix: &str,
) -> Vec<From> {
    let mut items: Vec<From> = Vec::new();
    // a FROM with a derived table, a view or a CTE holds one of them and one table more at most: their
    // rows are a join's, and three joins' product takes longer than it tells
    let mut heavy = false;
    for i in 0..1 + r.below(most) {
        if heavy && i > 1 {
            break;
        }
        let alias = format!("{prefix}{i}");
        let mut item = if rich && !heavy && i < 2 && r.chance(0.15) {
            let q = source(r, schema, funcs);
            From {
                table: String::new(),
                cols: (0..q.cols.len()).map(|c| format!("c{c}")).collect(),
                sub: Some(Box::new(q)),
                alias,
                rowid: false,
                join: Join::First,
                on: None,
                using: vec![],
                natural: false,
                hint: Hint::None,
            }
        } else {
            let ctes: Vec<&Table> = schema
                .tables
                .iter()
                .filter(|t| t.kind == Kind::Cte)
                .collect();
            let any: Vec<&Table> = schema
                .tables
                .iter()
                .filter(|t| (rich && !heavy && i < 2) || t.kind.is_table())
                .collect();
            let t = if !ctes.is_empty() && rich && !heavy && i < 2 && r.chance(0.6) {
                *r.pick(&ctes)
            } else {
                *r.pick(&any)
            };
            heavy |= !t.kind.is_table();
            From::table(t, alias)
        };
        heavy |= item.sub.is_some();
        if i > 0 {
            item.join = r
                .pick(&[
                    Join::Inner,
                    Join::Inner,
                    Join::Left,
                    Join::Left,
                    Join::Right,
                    Join::Full,
                    Join::Cross,
                    Join::Comma,
                ])
                .clone();
        }
        let on = matches!(
            item.join,
            Join::Inner | Join::Left | Join::Right | Join::Full
        );
        // the columns USING or NATURAL joins: a name of two tables to its left, not joined by one of
        // them, is ambiguous
        let merged = i == 1 || (i == 2 && (items[1].natural || !items[1].using.is_empty()));
        let shared: Vec<String> = item
            .cols
            .iter()
            .filter(|c| items.iter().any(|f| f.cols.contains(c)))
            .cloned()
            .collect();
        if rich
            && merged
            && !shared.is_empty()
            && item.join != Join::Comma
            && r.chance(if on { 0.2 } else { 0.1 })
        {
            if r.chance(0.5) {
                item.natural = true;
            } else {
                let mut using: Vec<String> =
                    shared.iter().filter(|_| r.chance(0.6)).cloned().collect();
                if using.is_empty() {
                    using.push(shared[0].clone());
                }
                item.using = using;
            }
            items.push(item);
            continue;
        }
        items.push(item);
        if on && r.chance(0.9) {
            let p = scope(schema, &items, funcs, false).pred(r, 2);
            if let Some(last) = items.last_mut() {
                last.on = Some(p);
            }
        }
    }
    items
}

/// The scope a FROM gives its query's expressions.
pub fn scope<'a>(schema: &'a Schema, from: &[From], funcs: &'a [Func], aggs: bool) -> Scope<'a> {
    let mut tables: Vec<(String, Vec<String>)> = from
        .iter()
        .filter(|f| !f.cols.is_empty())
        .map(|f| (f.alias.clone(), f.cols.clone()))
        .collect();
    // two tables joined by USING or NATURAL: their joined columns by name alone
    if let [a, b] = from {
        let both: Vec<String> = b
            .cols
            .iter()
            .filter(|c| a.cols.contains(c) && (b.natural || b.using.contains(c)))
            .cloned()
            .collect();
        if !both.is_empty() {
            tables.push((String::new(), both));
        }
    }
    Scope {
        schema,
        tables,
        funcs,
        aggs,
    }
}

/// The FROM's aliases when each of its tables has a rowid: what orders its rows with none tied.
fn rowids(from: &[From]) -> Vec<String> {
    if from.iter().all(|f| f.rowid) {
        from.iter().map(|f| f.alias.clone()).collect()
    } else {
        vec![]
    }
}

/// An expression two of whose equal values are the same value (`exact`).
fn exactly(r: &mut Rng, e: E) -> E {
    if exact(&e) {
        e
    } else if r.chance(0.6) {
        E::Fn("quote".into(), vec![e])
    } else {
        E::Cast(Box::new(e), "INTEGER")
    }
}

/// A query whose rows another one reads (a derived table, a CTE, a view), each row certain: a plain
/// one, a grouped or DISTINCT one, a compound, or one with a window function.
pub fn source(r: &mut Rng, schema: &Schema, funcs: &[Func]) -> Sel {
    let one = |r: &mut Rng, width: usize| {
        let prefix = format!("b{}x", r.below(100));
        let from = joined(r, schema, funcs, false, 2, &prefix);
        let s = scope(schema, &from, funcs, false);
        Sel {
            cols: (0..width).map(|_| s.expr(r, 2)).collect(),
            wh: if r.chance(0.5) {
                Some(s.pred(r, 2))
            } else {
                None
            },
            from,
            ..Sel::default()
        }
    };
    let width = 1 + r.below(3);
    let mut q = one(r, width);
    match r.below(8) {
        0 => {
            let key = q.cols.remove(0);
            let key = exactly(r, key);
            let key = if column_number(&key) {
                E::Cast(Box::new(key), "INTEGER")
            } else {
                key
            };
            q.group = vec![key.clone()];
            let s = scope(schema, &q.from, funcs, false);
            let e = s.expr(r, 1);
            let agg = match r.below(4) {
                0 => E::Agg("count".into(), false, vec![]),
                1 => E::Agg(
                    "count".into(),
                    r.chance(0.3),
                    vec![E::Fn("quote".into(), vec![e])],
                ),
                2 => E::Agg(
                    r.pick(&["min", "max"]).to_string(),
                    false,
                    vec![E::Fn("quote".into(), vec![e])],
                ),
                _ => E::Agg(
                    "sum".into(),
                    false,
                    vec![E::Bin(
                        "%",
                        Box::new(E::Cast(Box::new(e), "INTEGER")),
                        Box::new(E::Lit(V::Int(1000))),
                    )],
                ),
            };
            q.cols = vec![key, agg];
            if r.chance(0.3) {
                q.having = Some(E::Bin(
                    ">",
                    Box::new(E::Agg("count".into(), false, vec![])),
                    Box::new(E::Lit(V::Int(1))),
                ));
            }
        }
        1 => {
            q.distinct = true;
            q.cols = q.cols.into_iter().map(|e| exactly(r, e)).collect();
        }
        2 | 3 => {
            for _ in 0..1 + r.below(2) {
                let op = *r.pick(&["UNION ALL", "UNION ALL", "UNION", "INTERSECT", "EXCEPT"]);
                let arm = one(r, width);
                q.arms.push((op, arm));
            }
            // each column of one affinity in every arm (datatype3.html 3.3: which arm's it takes is
            // indeterminate), and exact where the compound keeps one of several equal values
            let dedupes = q.arms.iter().any(|(op, _)| *op != "UNION ALL");
            for k in 0..width {
                let cast = r.chance(0.4);
                let arms =
                    std::iter::once(&mut q.cols).chain(q.arms.iter_mut().map(|(_, a)| &mut a.cols));
                for cols in arms {
                    if let Some(c) = cols.get_mut(k) {
                        let e = std::mem::replace(c, E::Lit(V::Null));
                        *c = if cast {
                            E::Cast(Box::new(e), "INTEGER")
                        } else if dedupes {
                            E::Fn("quote".into(), vec![e])
                        } else {
                            E::Un("+", Box::new(e))
                        };
                    }
                }
            }
        }
        4 => {
            let s = scope(schema, &q.from, funcs, false);
            let mut w = s.window(r, &rowids(&q.from));
            if let E::Win(w) = &mut w
                && matches!(w.func, "min" | "max")
            {
                w.func = "count";
            }
            q.cols.push(w);
        }
        _ => {}
    }
    q.named = true;
    q
}

/// A recursive CTE's body, `name` its own name: a counter, a UNION that ends at its own duplicates, or
/// a walk along a table's rows to a depth.
fn recursion(r: &mut Rng, schema: &Schema, funcs: &[Func], name: &str) -> Sel {
    let own = |cols: usize| From {
        table: name.to_owned(),
        sub: None,
        alias: name.to_owned(),
        cols: (0..cols).map(|c| format!("c{c}")).collect(),
        rowid: false,
        join: Join::First,
        on: None,
        using: vec![],
        natural: false,
        hint: Hint::None,
    };
    let col = |c: &str| Box::new(E::Col(name.to_owned(), c.to_owned()));
    let int = |i: i64| Box::new(E::Lit(V::Int(i)));
    match r.below(3) {
        0 => Sel {
            cols: vec![E::Cast(int(r.below(4) as i64), "INTEGER")],
            arms: vec![(
                *r.pick(&["UNION ALL", "UNION"]),
                Sel {
                    cols: vec![E::Cast(
                        Box::new(E::Bin("+", col("c0"), int(1 + r.below(3) as i64))),
                        "INTEGER",
                    )],
                    from: vec![own(1)],
                    wh: Some(E::Bin("<", col("c0"), int(r.below(12) as i64))),
                    ..Sel::default()
                },
            )],
            limit: Some((RECURSION_ROWS, 0)),
            ..Sel::default()
        },
        1 => {
            let next = E::Bin(
                "%",
                Box::new(E::Bin(
                    "+",
                    Box::new(E::Bin("*", col("c0"), int(1 + r.below(5) as i64))),
                    int(r.below(5) as i64),
                )),
                int(2 + r.below(11) as i64),
            );
            Sel {
                cols: vec![E::Cast(int(r.below(12) as i64), "INTEGER")],
                arms: vec![(
                    "UNION",
                    Sel {
                        cols: vec![E::Cast(Box::new(next), "INTEGER")],
                        from: vec![own(1)],
                        ..Sel::default()
                    },
                )],
                limit: Some((RECURSION_ROWS, 0)),
                ..Sel::default()
            }
        }
        _ => {
            let tables: Vec<&Table> = schema.tables.iter().filter(|t| t.kind.is_table()).collect();
            let (t, u) = (*r.pick(&tables), *r.pick(&tables));
            let start = vec![From::table(t, "b0".into())];
            let s = scope(schema, &start, funcs, false);
            let first = s.expr(r, 1);
            let cols = vec![E::Un("+", Box::new(first)), E::Lit(V::Int(0))];
            let wh = if r.chance(0.4) {
                Some(s.pred(r, 1))
            } else {
                None
            };
            let mut walk = vec![own(2)];
            if r.chance(0.6) {
                let mut f = From::table(u, "b1".into());
                f.join = Join::Inner;
                f.on = Some(E::Bin(
                    r.pick(&["=", "IS"]),
                    Box::new(E::Col("b1".into(), r.pick(&u.cols).name.clone())),
                    col("c0"),
                ));
                walk.push(f);
                if r.chance(0.3) {
                    walk.reverse();
                    walk[0].join = Join::First;
                    let on = walk[0].on.take();
                    walk[1].join = Join::Inner;
                    walk[1].on = on;
                }
            }
            let s = scope(schema, &walk, funcs, false);
            let next = E::Un("+", Box::new(s.expr(r, 1)));
            Sel {
                cols,
                from: start,
                wh,
                arms: vec![(
                    "UNION ALL",
                    Sel {
                        cols: vec![next, E::Bin("+", col("c1"), int(1))],
                        from: walk,
                        wh: Some(E::Bin("<", col("c1"), int(1 + r.below(2) as i64))),
                        ..Sel::default()
                    },
                )],
                ..Sel::default()
            }
        }
    }
}

/// One or two CTEs, and the schema with them in it.
fn with(r: &mut Rng, schema: &Schema, funcs: &[Func]) -> (Vec<Cte>, Schema) {
    let mut known = schema.clone();
    let mut ctes = Vec::new();
    for i in 0..1 + r.below(2) {
        let name = format!("w{i}");
        let recursive = r.chance(0.4);
        let mut body = if recursive {
            recursion(r, schema, funcs, &name)
        } else {
            source(r, &known, funcs)
        };
        body.named = false;
        let cols: Vec<String> = (0..body.cols.len()).map(|c| format!("c{c}")).collect();
        ctes.push(Cte {
            name: name.clone(),
            recursive,
            hint: r.pick(&["", "", "MATERIALIZED ", "NOT MATERIALIZED "]),
            body,
        });
        known.tables.push(Table {
            name,
            cols: unnamed(&cols),
            kind: Kind::Cte,
            strict: false,
            unique: vec![],
            alias: None,
        });
    }
    (ctes, known)
}

/// A plain query: columns of the FROM, a WHERE, no grouping; what TLP, NoREC, DQP, EET and CERT
/// start from.
pub fn plain(r: &mut Rng, schema: &Schema, funcs: &[Func], rich: bool) -> Sel {
    let from = from(r, schema, funcs, rich);
    let s = scope(schema, &from, funcs, false);
    let cols = (0..1 + r.below(3)).map(|_| s.expr(r, 2)).collect();
    let wh = if r.chance(0.7) {
        Some(s.pred(r, 3))
    } else {
        None
    };
    Sel {
        distinct: r.chance(0.1),
        cols,
        from,
        wh,
        ..Sel::default()
    }
}

/// An inner join of two to four tables (a table may come twice), each joined by JOIN with an ON
/// or by a comma, with a WHERE most of the time: what the order check permutes.
pub fn inner_join(r: &mut Rng, schema: &Schema, funcs: &[Func]) -> Sel {
    let mut from = Vec::new();
    let mut tables: Vec<(String, Vec<String>)> = Vec::new();
    let base: Vec<&Table> = schema.tables.iter().filter(|t| t.kind.is_table()).collect();
    for i in 0..2 + r.below(3) {
        let t = *r.pick(&base);
        let alias = format!("a{i}");
        let mut item = From::table(t, alias.clone());
        tables.push((alias, item.cols.clone()));
        let join = if i == 0 {
            Join::First
        } else if r.chance(0.7) {
            Join::Inner
        } else {
            Join::Comma
        };
        let on = (join == Join::Inner && r.chance(0.9)).then(|| {
            Scope {
                schema,
                tables: tables.clone(),
                funcs,
                aggs: false,
            }
            .pred(r, 2)
        });
        item.join = join;
        item.on = on;
        from.push(item);
    }
    let s = scope(schema, &from, funcs, false);
    let cols = (0..1 + r.below(3)).map(|_| s.expr(r, 2)).collect();
    let wh = if r.chance(0.8) {
        Some(s.pred(r, 3))
    } else {
        None
    };
    Sel {
        cols,
        from,
        wh,
        ..Sel::default()
    }
}

/// Whether SQLite reads `e` as a result column's number in GROUP BY or ORDER BY: an integer
/// literal, behind any COLLATE or unary + or - (resolveOrderGroupBy's sqlite3ExprIsInteger).
fn column_number(e: &E) -> bool {
    match e {
        E::Lit(V::Int(_)) => true,
        E::Un("+" | "-", a) | E::Collate(a, _) => column_number(a),
        _ => false,
    }
}

/// A query grouped and aggregated, or neither; a `rich` one may have window functions, over its rows
/// or over its groups.
fn grouped(r: &mut Rng, schema: &Schema, funcs: &[Func], rich: bool) -> Sel {
    let mut q = plain(r, schema, funcs, rich);
    if r.chance(0.3) {
        let s = scope(schema, &q.from, funcs, true);
        // GROUP BY's terms read rows, not aggregates, and an integer there is a column's number
        // to SQLite, so one is cast: the same value, read as a value
        let rows = scope(schema, &q.from, funcs, false);
        q.group = (0..1 + r.below(2))
            .map(|_| rows.expr(r, 1))
            .map(|e| {
                if column_number(&e) {
                    E::Cast(Box::new(e), "INTEGER")
                } else {
                    e
                }
            })
            .collect();
        q.cols = q.group.clone();
        q.cols.push(s.aggregate(r, 1));
        if r.chance(0.4) {
            q.having = Some(s.pred(r, 2));
        }
        if rich && r.chance(0.2) {
            // a window over the groups, whose ORDER BY may tie, by functions that tie with it
            let count = E::Agg("count".into(), false, vec![]);
            let key = r.pick(&q.group).clone();
            q.cols.push(E::Win(Box::new(Win {
                func: r.pick(&["rank", "dense_rank", "sum", "count"]),
                args: vec![],
                filter: None,
                partition: if r.chance(0.3) {
                    vec![key.clone()]
                } else {
                    vec![]
                },
                order: vec![(if r.chance(0.5) { count } else { key }, r.chance(0.4))],
                frame: None,
            })));
            if let Some(E::Win(w)) = q.cols.last_mut()
                && w.func == "sum"
            {
                w.func = "count";
                w.args = vec![E::Agg("count".into(), false, vec![])];
            }
        }
    } else if r.chance(0.15) {
        let s = scope(schema, &q.from, funcs, true);
        q.cols = (0..1 + r.below(2)).map(|_| s.aggregate(r, 1)).collect();
    } else if rich && r.chance(0.25) {
        let s = scope(schema, &q.from, funcs, false);
        let rowids = rowids(&q.from);
        for _ in 0..1 + r.below(2) {
            let w = s.window(r, &rowids);
            q.cols.push(w);
        }
    }
    q
}

/// The query ordered by every column it returns, and limited, a fifth of the time.
fn limited(r: &mut Rng, mut q: Sel) -> Sel {
    let whole = q.cols.iter().any(|c| matches!(c, E::Star));
    if !q.loose() && !whole && r.chance(0.2) {
        // ordered by every column it returns, exactly (BINARY, then the storage class, since 'a'
        // and 'A' tie under NOCASE and 1 and 1.0 tie as numbers), so the rows a LIMIT keeps are
        // certain
        // (an integer column, the same in every row, orders nothing, and SQLite would read it as
        // a column's number)
        let desc = r.chance(0.5);
        q.order = q
            .cols
            .iter()
            .filter(|c| !column_number(c))
            .flat_map(|c| {
                [
                    (E::Collate(Box::new(c.clone()), "BINARY"), desc),
                    (E::Fn("typeof".into(), vec![c.clone()]), desc),
                ]
            })
            .collect();
        q.limit = Some((r.below(5) as i64, r.below(3) as i64));
    }
    q
}

/// A query over base tables, with no compound, CTE, derived table, view or window: what CERT starts
/// from.
pub fn classic(r: &mut Rng, schema: &Schema, funcs: &[Func]) -> Sel {
    let q = grouped(r, schema, funcs, false);
    limited(r, q)
}

/// A query of any shape the generator makes: over CTEs, derived tables and views, grouped and
/// aggregated, with window functions, compound, ordered and limited.
pub fn query(r: &mut Rng, schema: &Schema, funcs: &[Func]) -> Sel {
    let (ctes, known) = if r.chance(0.2) {
        with(r, schema, funcs)
    } else {
        (vec![], schema.clone())
    };
    let mut q = grouped(r, &known, funcs, true);
    if r.chance(0.18) {
        let width = q.cols.len();
        for _ in 0..1 + r.below(2) {
            let mut arm = grouped(r, &known, funcs, true);
            arm.cols.truncate(width);
            while arm.cols.len() < width {
                arm.cols.push(E::Lit(literal(r)));
            }
            let op = *r.pick(&["UNION ALL", "UNION", "UNION", "INTERSECT", "EXCEPT"]);
            q.arms.push((op, arm));
        }
        // a compound's ORDER BY names its columns by their numbers, or not at all
        if r.chance(0.4) {
            q.order = (0..1 + r.below(width))
                .map(|k| {
                    let n = E::Lit(V::Int(k as i64 + 1));
                    let term = if r.chance(0.4) {
                        E::Collate(Box::new(n), r.pick(COLLATIONS))
                    } else {
                        n
                    };
                    (term, r.chance(0.4))
                })
                .collect();
        }
    } else {
        if !q.aggregates() && !q.from.is_empty() && r.chance(0.06) {
            q.cols = vec![E::Star];
        }
        q = limited(r, q);
    }
    q.with = ctes;
    q
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    fn funcs() -> Vec<Func> {
        [
            ("abs", 1, false),
            ("lower", 1, false),
            ("coalesce", 2, false),
            ("count", 1, true),
            ("sum", 1, true),
            ("max", 1, true),
        ]
        .into_iter()
        .map(|(name, n, aggregate)| Func {
            name: name.into(),
            nargs: vec![n],
            aggregate,
            agg_nargs: vec![],
        })
        .collect()
    }

    /// Each feature with the text that shows it.
    const SHOWN: [(&str, &str); 14] = [
        ("union all", " UNION ALL SELECT "),
        ("union", " UNION SELECT "),
        ("intersect", " INTERSECT SELECT "),
        ("except", " EXCEPT SELECT "),
        ("with", "WITH "),
        ("recursive", "WITH RECURSIVE "),
        ("derived", "(SELECT "),
        ("window", " OVER ("),
        ("using", " USING ("),
        ("natural", " NATURAL "),
        ("row value", "(("),
        ("view", " v0 AS "),
        ("star", "* FROM "),
        ("without rowid", " t0 AS "),
    ];

    /// Do: generate the databases and queries of 600 seeds. Expect: each feature in many of them, every
    /// query said to show one showing it in its text.
    #[test]
    fn the_generator_makes_every_feature() {
        let funcs = funcs();
        let mut seen: BTreeMap<&str, u32> = BTreeMap::new();
        for seed in 0..600 {
            let mut r = Rng::new(seed);
            let (stmts, schema) = database(&mut r, &funcs);
            let mut held = BTreeSet::new();
            statement_features(&stmts, &mut held);
            for f in held {
                *seen.entry(f).or_default() += 1;
            }
            for _ in 0..20 {
                let q = query(&mut r, &schema, &funcs);
                let sql = q.sql();
                let mut got = BTreeSet::new();
                q.features(&schema, &mut got);
                for f in got {
                    *seen.entry(f).or_default() += 1;
                    if let Some((_, text)) = SHOWN.iter().find(|(name, _)| *name == f)
                        && !matches!(f, "view" | "without rowid")
                    {
                        assert!(sql.contains(text), "{f}: {sql}");
                    }
                }
            }
        }
        for f in SHOWN
            .iter()
            .map(|(f, _)| *f)
            .chain(["strict", "upsert", "trigger"])
        {
            assert!(seen.get(f).is_some_and(|n| *n > 20), "{f}: {seen:?}");
        }
    }

    /// Do: list what the statements of 600 databases say. Expect: UPSERTs with a target and without,
    /// triggers of each event and time, and views, STRICT and WITHOUT ROWID tables, each in one
    /// statement of one line, as a finding's file keeps it.
    #[test]
    fn the_statements_hold_every_kind_of_upsert_trigger_and_table() {
        let funcs = funcs();
        let all: Vec<String> = (0..600)
            .flat_map(|seed| database(&mut Rng::new(seed), &funcs).0)
            .collect();
        for text in [
            " ON CONFLICT(c0) DO UPDATE SET ",
            " ON CONFLICT DO UPDATE SET ",
            " DO NOTHING",
            "CREATE TRIGGER g0 BEFORE INSERT ON ",
            "CREATE TRIGGER g0 AFTER INSERT ON ",
            " AFTER UPDATE ON ",
            " UPDATE OF ",
            " BEFORE DELETE ON ",
            " INSTEAD OF INSERT ON v",
            "RAISE(IGNORE)",
            "RAISE(ABORT, 'no')",
            "CREATE VIEW v0 AS ",
            "CREATE VIEW v0(c0",
            ") STRICT",
            ") WITHOUT ROWID",
            ") STRICT, WITHOUT ROWID",
            "PRIMARY KEY(c0, c1)",
        ] {
            assert!(all.iter().any(|s| s.contains(text)), "{text}");
        }
        assert!(all.iter().all(|s| !s.contains(";\n")));
    }

    fn table(alias: &str) -> From {
        From::table(
            &Table {
                name: "t0".into(),
                cols: unnamed(&["c0".to_owned()]),
                kind: Kind::Rowid,
                strict: false,
                unique: vec![],
                alias: None,
            },
            alias.into(),
        )
    }

    fn row_number(order: Vec<(E, bool)>) -> E {
        E::Win(Box::new(Win {
            func: "row_number",
            args: vec![],
            filter: None,
            partition: vec![],
            order,
            frame: None,
        }))
    }

    /// Do: number a join's rows by a column, by one table's rowid, by both. Expect: certain only where
    /// the ORDER BY leaves no two rows tied.
    #[test]
    fn a_window_that_numbers_tied_rows_is_not_certain() {
        let col = |a: &str, c: &str| (E::Col(a.into(), c.into()), false);
        let q = |order| Sel {
            cols: vec![row_number(order)],
            from: vec![table("a0"), table("a1")],
            ..Sel::default()
        };
        assert!(!q(vec![col("a0", "c0")]).certain());
        assert!(!q(vec![col("a0", "rowid")]).certain());
        assert!(
            q(vec![
                col("a0", "c0"),
                col("a0", "rowid"),
                col("a1", "rowid")
            ])
            .certain()
        );
    }

    /// Do: read a derived table that keeps one of several equal values. Expect: certain only where two
    /// equal values of its columns are the same value.
    #[test]
    fn a_derived_table_that_keeps_one_of_equal_values_is_not_certain() {
        let over = |col: E| Sel {
            cols: vec![E::Col("a0".into(), "c0".into())],
            from: vec![From {
                table: String::new(),
                sub: Some(Box::new(Sel {
                    cols: vec![col],
                    distinct: true,
                    named: true,
                    from: vec![table("b0")],
                    ..Sel::default()
                })),
                ..table("a0")
            }],
            ..Sel::default()
        };
        let c0 = E::Col("b0".into(), "c0".into());
        assert!(!over(c0.clone()).certain());
        assert!(over(E::Fn("quote".into(), vec![c0])).certain());
    }

    /// Do: count the queries a query reads from and reach each. Expect: as many reached as counted, and
    /// none past them.
    #[test]
    fn every_source_of_a_query_is_reached() {
        let funcs = funcs();
        let mut most = 0;
        for seed in 0..200 {
            let mut r = Rng::new(seed);
            let (_, schema) = database(&mut r, &funcs);
            let mut q = query(&mut r, &schema, &funcs);
            let n = q.sources();
            most = most.max(n);
            for k in 0..n {
                assert!(
                    q.source_mut(&mut { k }).is_some(),
                    "{k} of {n}: {}",
                    q.sql()
                );
            }
            assert!(q.source_mut(&mut { n }).is_none(), "{}", q.sql());
        }
        assert!(most > 2);
    }
}
