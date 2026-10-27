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
}

impl E {
    pub fn sql(&self) -> String {
        match self {
            E::Col(t, c) => format!("{t}.{c}"),
            E::Lit(v) => v.sql(),
            // a space after every unary operator: `- -1` stays an expression, `--1` would be a comment
            E::Un(op, a) => format!("({op} {})", a.sql()),
            E::Bin(op, a, b) => format!("({} {op} {})", a.sql(), b.sql()),
            E::Fn(f, args) => format!("{f}({})", args.iter().map(E::sql).collect::<Vec<_>>().join(", ")),
            E::Agg(f, distinct, args) => format!("{f}({}{})", if *distinct { "DISTINCT " } else { "" }, if args.is_empty() { "*".into() } else { args.iter().map(E::sql).collect::<Vec<_>>().join(", ") }),
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
            E::In(a, list, not) => format!("({} {}IN ({}))", a.sql(), if *not { "NOT " } else { "" }, list.iter().map(E::sql).collect::<Vec<_>>().join(", ")),
            E::Between(a, lo, hi, not) => format!("({} {}BETWEEN {} AND {})", a.sql(), if *not { "NOT " } else { "" }, lo.sql(), hi.sql()),
            E::Null(a, not) => format!("({} {})", a.sql(), if *not { "NOTNULL" } else { "ISNULL" }),
            E::Exists(s, not) => format!("({}EXISTS ({}))", if *not { "NOT " } else { "" }, s.sql()),
            E::Scalar(s) => format!("({})", s.sql()),
            E::InSel(a, s, not) => format!("({} {}IN ({}))", a.sql(), if *not { "NOT " } else { "" }, s.sql()),
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
            E::Col(..) | E::Lit(_) | E::Exists(..) | E::Scalar(_) => vec![],
            E::Un(_, a) | E::Cast(a, _) | E::Collate(a, _) | E::Null(a, _) | E::InSel(a, _, _) => vec![a],
            E::Bin(_, a, b) => vec![a, b],
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
            E::Col(..) | E::Lit(_) | E::Exists(..) | E::Scalar(_) => vec![],
            E::Un(_, a) | E::Cast(a, _) | E::Collate(a, _) | E::Null(a, _) | E::InSel(a, _, _) => vec![a],
            E::Bin(_, a, b) => vec![a, b],
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
    pub table: String,
    pub alias: String,
    pub join: Join,
    pub on: Option<E>,
    pub hint: Hint,
}

/// A SELECT. `order` and `limit` only where they leave the answer's rows certain: a LIMIT comes
/// with an ORDER BY over every column it returns.
#[derive(Clone, Debug)]
pub struct Sel {
    pub distinct: bool,
    pub cols: Vec<E>,
    pub from: Vec<From>,
    pub wh: Option<E>,
    pub group: Vec<E>,
    pub having: Option<E>,
    pub order: Vec<(E, bool)>,
    pub limit: Option<(i64, i64)>,
}

impl Sel {
    /// Whether every value it returns is certain: in a grouped or aggregate query, no column read
    /// outside an aggregate unless the expression is one it groups by (a bare column reads any row
    /// of its group, and two engines, or two plans, may read different ones).
    pub fn certain(&self) -> bool {
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
                E::Col(alias, _) => !inside || outer.contains(&alias.as_str()),
                E::Exists(q, _) | E::Scalar(q) => sel_bare(q, grouped, outer),
                E::InSel(a, q, _) => bare(a, grouped, outer, inside) || sel_bare(q, grouped, outer),
                _ => e.children().into_iter().any(|c| bare(c, grouped, outer, inside)),
            }
        }
        fn sel_bare(q: &Sel, grouped: &[String], outer: &[&str]) -> bool {
            q.cols.iter()
                .chain(q.from.iter().filter_map(|f| f.on.as_ref()))
                .chain(q.wh.iter())
                .chain(q.group.iter())
                .chain(q.having.iter())
                .chain(q.order.iter().map(|(e, _)| e))
                .any(|e| bare(e, grouped, outer, true))
        }
        !self.cols.iter().chain(self.having.iter()).chain(self.order.iter().map(|(e, _)| e)).any(|e| bare(e, &grouped, &outer, false))
    }

    pub fn sql(&self) -> String {
        let mut s = String::from("SELECT ");
        if self.distinct {
            s.push_str("DISTINCT ");
        }
        s.push_str(&self.cols.iter().map(E::sql).collect::<Vec<_>>().join(", "));
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
                    s.push_str(join);
                }
                s.push_str(&format!("{} AS {}", f.table, f.alias));
                match &f.hint {
                    Hint::None => {}
                    Hint::NotIndexed => s.push_str(" NOT INDEXED"),
                    Hint::IndexedBy(ix) => s.push_str(&format!(" INDEXED BY {ix}")),
                }
                if let Some(on) = &f.on {
                    s.push_str(&format!(" ON {}", on.sql()));
                }
            }
        }
        if let Some(w) = &self.wh {
            s.push_str(&format!(" WHERE {}", w.sql()));
        }
        if !self.group.is_empty() {
            s.push_str(&format!(" GROUP BY {}", self.group.iter().map(E::sql).collect::<Vec<_>>().join(", ")));
        }
        if let Some(h) = &self.having {
            s.push_str(&format!(" HAVING {}", h.sql()));
        }
        if !self.order.is_empty() {
            s.push_str(&format!(" ORDER BY {}", self.order.iter().map(|(e, d)| format!("{}{}", e.sql(), if *d { " DESC" } else { "" })).collect::<Vec<_>>().join(", ")));
        }
        if let Some((l, o)) = self.limit {
            s.push_str(&format!(" LIMIT {l} OFFSET {o}"));
        }
        s
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

#[derive(Clone, Debug)]
pub struct Table {
    pub name: String,
    pub cols: Vec<Col>,
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
const UNSTABLE: &[&str] = &["random", "randomblob", "changes", "total_changes", "last_insert_rowid", "sqlite_offset", "load_extension", "sqlite_compileoption_get", "sqlite_compileoption_used", "sqlite_source_id", "sqlite_version", "group_concat", "string_agg", "json_group_array", "json_group_object", "jsonb_group_array", "jsonb_group_object"];

/// The inventory's functions with the arities SQLite gives them (pragma_function_list), the ones
/// whose answers aren't stable left out.
pub fn functions(features: &serde_json::Value, sqlite: &mut super::engines::Sqlite) -> anyhow::Result<Vec<Func>> {
    use super::engines::Engine;
    let names: std::collections::HashSet<String> = features["features"].as_array().into_iter().flatten()
        .filter(|f| f["kind"].as_str().is_some_and(|k| k.starts_with("function.") && k != "function.window"))
        .filter_map(|f| f["name"].as_str().map(str::to_owned)).collect();
    let mut by: std::collections::BTreeMap<String, Func> = std::collections::BTreeMap::new();
    let mut windows: std::collections::BTreeMap<String, Vec<i32>> = std::collections::BTreeMap::new();
    for row in sqlite.run("SELECT name, type, narg FROM pragma_function_list").map_err(|e| anyhow::anyhow!(e))? {
        let (Some(V::Text(name)), Some(V::Text(kind)), Some(V::Int(narg))) = (row.first(), row.get(1), row.get(2)) else { continue };
        if !names.contains(name) || UNSTABLE.contains(&name.as_str()) {
            continue;
        }
        let narg = *narg as i32;
        if kind == "w" {
            windows.entry(name.clone()).or_default().push(narg);
            continue;
        }
        let f = by.entry(name.clone()).or_insert(Func { name: name.clone(), nargs: vec![], aggregate: kind == "a", agg_nargs: vec![] });
        if !f.nargs.contains(&narg) {
            f.nargs.push(narg);
        }
    }
    for f in by.values_mut().filter(|f| !f.aggregate) {
        f.agg_nargs = windows.remove(&f.name).unwrap_or_default();
    }
    Ok(by.into_values().collect())
}

const DECLS: &[&str] = &["INTEGER", "INT", "TEXT", "VARCHAR(10)", "REAL", "DOUBLE", "NUMERIC", "DECIMAL(10,2)", "BLOB", "", "BOOLEAN"];
const COLLATIONS: &[&str] = &["BINARY", "NOCASE", "RTRIM"];

/// Values of every storage class and their edges: what affinity, comparison and collation decide.
/// The BLOBs are well-formed UTF-8 (the high one is U+00FF's bytes, not FF alone): what SQLite
/// answers for ill-formed text it doesn't promise (docs/design/semantics.md, decision 1).
pub fn literal(r: &mut Rng) -> V {
    match r.below(10) {
        0 => V::Null,
        1..=3 => V::Int(*r.pick(&[0, 1, -1, 2, 3, 7, 10, 100, -100, 255, i64::MAX, i64::MIN, 9_007_199_254_740_993])),
        4 | 5 => V::Real(*r.pick(&[0.0, 0.5, 1.5, -2.5, 3.0, 1e15, 1e308, -1e-308, 2.0, 0.1])),
        6..=8 => V::Text(r.pick(&["", " ", "a", "A", "abc", "ABC", "0", "1", "1.5", "10", "-1", "a ", " a", "%", "_", "true", "1e3", "x", "é", "0x10"]).to_string()),
        _ => V::Blob(r.pick(&[&[][..], &[0u8][..], &[0x41][..], &[0xc3, 0xbf][..], &[1, 2][..]]).to_vec()),
    }
}

/// A random database: its statements, in order, and the schema they make.
pub fn database(r: &mut Rng) -> (Vec<String>, Schema) {
    let mut stmts = Vec::new();
    let mut schema = Schema::default();
    for t in 0..1 + r.below(3) {
        let name = format!("t{t}");
        let ncols = 1 + r.below(4);
        let mut cols = Vec::new();
        let mut defs = Vec::new();
        let pk = r.below(ncols + 2);
        for c in 0..ncols {
            let col = Col { name: format!("c{c}"), decl: *r.pick(DECLS), collate: if r.chance(0.3) { Some(*r.pick(COLLATIONS)) } else { None } };
            let mut def = format!("{} {}", col.name, col.decl);
            if let Some(co) = col.collate {
                def.push_str(&format!(" COLLATE {co}"));
            }
            if c == pk {
                def.push_str(" PRIMARY KEY");
            } else if r.chance(0.15) {
                def.push_str(" UNIQUE");
            }
            if r.chance(0.1) {
                def.push_str(" NOT NULL");
            }
            defs.push(def);
            cols.push(col);
        }
        stmts.push(format!("CREATE TABLE {name}({})", defs.join(", ")));
        for _ in 0..r.below(12) {
            let n = 1 + r.below(3);
            // the rowid's alias (an INTEGER PRIMARY KEY) never takes the largest rowid: a NULL after it
            // gets a rowid chosen at random, a different one in each engine
            let alias = (pk < ncols && cols[pk].decl == "INTEGER").then_some(pk);
            let rows: Vec<String> = (0..n)
                .map(|_| format!("({})", (0..ncols).map(|c| match literal(r) {
                    V::Int(i64::MAX) if Some(c) == alias => V::Int(5).sql(),
                    v => v.sql(),
                }).collect::<Vec<_>>().join(", ")))
                .collect();
            stmts.push(format!("INSERT {}INTO {name} VALUES {}", if r.chance(0.5) { "OR IGNORE " } else { "" }, rows.join(", ")));
        }
        schema.tables.push(Table { name, cols });
    }
    for i in 0..r.below(4) {
        let t = schema.tables[r.below(schema.tables.len())].clone();
        let mut terms = Vec::new();
        for _ in 0..1 + r.below(2.min(t.cols.len())) {
            let c = r.pick(&t.cols).name.clone();
            let mut term = if r.chance(0.2) { format!("({c} + 1)") } else { c };
            if r.chance(0.2) {
                term.push_str(&format!(" COLLATE {}", r.pick(COLLATIONS)));
            }
            if r.chance(0.3) {
                term.push_str(" DESC");
            }
            terms.push(term);
        }
        let name = format!("i{i}");
        let partial = if r.chance(0.2) { format!(" WHERE {} IS NOT NULL", r.pick(&t.cols).name) } else { String::new() };
        stmts.push(format!("CREATE {}INDEX {name} ON {}({}){partial}", if r.chance(0.2) { "UNIQUE " } else { "" }, t.name, terms.join(", ")));
        schema.indexes.push((name, t.name.clone()));
    }
    if r.chance(0.3) {
        let t = schema.tables[r.below(schema.tables.len())].clone();
        stmts.push(format!("UPDATE {} SET {} = {} WHERE {} IS NOT NULL", t.name, r.pick(&t.cols).name, literal(r).sql(), r.pick(&t.cols).name));
    }
    if r.chance(0.2) {
        let t = schema.tables[r.below(schema.tables.len())].clone();
        stmts.push(format!("DELETE FROM {} WHERE {} = {}", t.name, r.pick(&t.cols).name, literal(r).sql()));
    }
    if r.chance(0.3) {
        stmts.push("ANALYZE".into());
    }
    (stmts, schema)
}

// ---------------------------------------------------------------- expressions

const ARITH: &[&str] = &["+", "-", "*", "/", "%", "&", "|", "<<", ">>", "||"];
const COMPARE: &[&str] = &["=", "==", "<>", "!=", "<", "<=", ">", ">=", "IS", "IS NOT", "IS DISTINCT FROM", "IS NOT DISTINCT FROM"];
const TYPES: &[&str] = &["INTEGER", "TEXT", "REAL", "NUMERIC", "BLOB"];

/// What an expression may read: the FROM's tables (alias, table) and the functions.
pub struct Scope<'a> {
    pub schema: &'a Schema,
    pub tables: Vec<(String, String)>,
    pub funcs: &'a [Func],
    /// Whether aggregates may be called here (a select list or HAVING of an aggregate query).
    pub aggs: bool,
}

impl Scope<'_> {
    fn column(&self, r: &mut Rng) -> Option<E> {
        if self.tables.is_empty() {
            return None;
        }
        let (alias, table) = r.pick(&self.tables).clone();
        let t = self.schema.tables.iter().find(|t| t.name == table)?;
        Some(E::Col(alias, r.pick(&t.cols).name.clone()))
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
            0 | 1 => E::Bin(r.pick(ARITH), Box::new(self.expr(r, d)), Box::new(self.expr(r, d))),
            2 => E::Un(r.pick(&["-", "+", "~"]), Box::new(self.expr(r, d))),
            3..=5 => self.call(r, d),
            6 => E::Case(
                if r.chance(0.3) { Some(Box::new(self.expr(r, d))) } else { None },
                (0..1 + r.below(2)).map(|_| (self.pred(r, d), self.expr(r, d))).collect(),
                if r.chance(0.7) { Some(Box::new(self.expr(r, d))) } else { None },
            ),
            7 => E::Cast(Box::new(self.expr(r, d)), r.pick(TYPES)),
            8 => E::Collate(Box::new(self.expr(r, d)), r.pick(COLLATIONS)),
            9 if self.aggs => self.aggregate(r, d),
            10 => E::Fn("json_extract".into(), vec![E::Lit(V::Text(r.pick(&["{\"a\":1,\"b\":[2,3]}", "[1,2,3]", "{\"a\":null}", "\"x\""]).to_string())), E::Lit(V::Text(r.pick(&["$.a", "$.b[1]", "$[0]", "$"]).to_string()))]),
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
        let n = if f.agg_nargs.contains(&(n as i32)) { n + 1 } else { n };
        let mut args: Vec<E> = (0..n).map(|_| self.expr(r, d)).collect();
        // likelihood's second argument is a REAL literal from 0.0 to 1.0, or SQLite refuses the call
        if f.name == "likelihood" {
            if let Some(p) = args.get_mut(1) {
                *p = E::Lit(V::Real(*r.pick(&[0.0, 0.0625, 0.5, 0.9375, 1.0])));
            }
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
        let inner = Scope { schema: self.schema, tables: self.tables.clone(), funcs: self.funcs, aggs: false };
        E::Agg(f.name.clone(), n == 1 && r.chance(0.2), (0..n).map(|i| if i == 0 { inner.expr(r, d) } else { E::Lit(literal(r)) }).collect())
    }

    /// A predicate: what WHERE, ON, HAVING and CASE WHEN read the truth of.
    pub fn pred(&self, r: &mut Rng, depth: u32) -> E {
        let d = depth.saturating_sub(1);
        match r.below(12) {
            0..=3 => E::Bin(r.pick(COMPARE), Box::new(self.expr(r, d)), Box::new(self.expr(r, d))),
            4 if depth > 0 => E::Bin("AND", Box::new(self.pred(r, d)), Box::new(self.pred(r, d))),
            5 if depth > 0 => E::Bin("OR", Box::new(self.pred(r, d)), Box::new(self.pred(r, d))),
            6 => E::Un("NOT", Box::new(self.pred(r, d))),
            7 => E::In(Box::new(self.expr(r, d)), (0..1 + r.below(3)).map(|_| self.expr(r, d)).collect(), r.chance(0.3)),
            8 => E::Between(Box::new(self.expr(r, d)), Box::new(self.expr(r, d)), Box::new(self.expr(r, d)), r.chance(0.3)),
            9 => E::Null(Box::new(self.expr(r, d)), r.chance(0.5)),
            10 => E::Bin(r.pick(&["LIKE", "NOT LIKE", "GLOB", "NOT GLOB"]), Box::new(self.expr(r, d)), Box::new(E::Lit(V::Text(r.pick(&["a%", "%a", "_", "%", "A%", "a*", "*", "[a-z]*", "abc"]).to_string())))),
            _ if depth > 0 && !self.schema.tables.is_empty() => self.subquery(r, d),
            _ => self.expr(r, d),
        }
    }

    /// EXISTS, IN or a scalar subquery over one table, correlated with the outer query or not.
    fn subquery(&self, r: &mut Rng, d: u32) -> E {
        let t = r.pick(&self.schema.tables).clone();
        let alias = format!("s{}", r.below(1000));
        let mut tables = vec![(alias.clone(), t.name.clone())];
        if r.chance(0.5) {
            tables.extend(self.tables.iter().cloned());
        }
        let inner = Scope { schema: self.schema, tables, funcs: self.funcs, aggs: false };
        let sel = |cols: Vec<E>, r: &mut Rng| Sel {
            distinct: false,
            cols,
            from: vec![From { table: t.name.clone(), alias: alias.clone(), join: Join::First, on: None, hint: Hint::None }],
            wh: if r.chance(0.8) { Some(inner.pred(r, d)) } else { None },
            group: vec![],
            having: None,
            order: vec![],
            limit: None,
        };
        let col = E::Col(alias.clone(), r.pick(&t.cols).name.clone());
        match r.below(3) {
            0 => E::Exists(Box::new(sel(vec![E::Lit(V::Int(1))], r)), r.chance(0.3)),
            1 => E::InSel(Box::new(self.expr(r, d)), Box::new(sel(vec![col], r)), r.chance(0.3)),
            _ => E::Scalar(Box::new(sel(vec![E::Agg(r.pick(&["count", "max", "min", "sum"]).to_string(), false, vec![col])], r))),
        }
    }
}

/// A FROM over one to three of the tables, joined by every kind of join.
pub fn from(r: &mut Rng, schema: &Schema, funcs: &[Func]) -> Vec<From> {
    let mut items = Vec::new();
    let mut scope: Vec<(String, String)> = Vec::new();
    for i in 0..1 + r.below(3) {
        let t = r.pick(&schema.tables).name.clone();
        let alias = format!("a{i}");
        scope.push((alias.clone(), t.clone()));
        let join = if i == 0 { Join::First } else { r.pick(&[Join::Inner, Join::Inner, Join::Left, Join::Left, Join::Right, Join::Full, Join::Cross, Join::Comma]).clone() };
        let on = match join {
            Join::Inner | Join::Left | Join::Right | Join::Full if r.chance(0.9) => {
                let s = Scope { schema, tables: scope.clone(), funcs, aggs: false };
                Some(s.pred(r, 2))
            }
            _ => None,
        };
        items.push(From { table: t, alias, join, on, hint: Hint::None });
    }
    items
}

/// The scope a FROM gives its query's expressions.
pub fn scope<'a>(schema: &'a Schema, from: &[From], funcs: &'a [Func], aggs: bool) -> Scope<'a> {
    Scope { schema, tables: from.iter().map(|f| (f.alias.clone(), f.table.clone())).collect(), funcs, aggs }
}

/// A plain query: columns of the FROM, a WHERE, no grouping; what TLP, NoREC, DQP, EET and CERT
/// start from.
pub fn plain(r: &mut Rng, schema: &Schema, funcs: &[Func]) -> Sel {
    let from = from(r, schema, funcs);
    let s = scope(schema, &from, funcs, false);
    let cols = (0..1 + r.below(3)).map(|_| s.expr(r, 2)).collect();
    let wh = if r.chance(0.7) { Some(s.pred(r, 3)) } else { None };
    Sel { distinct: r.chance(0.1), cols, from, wh, group: vec![], having: None, order: vec![], limit: None }
}

/// An inner join of two to four tables (a table may come twice), each joined by JOIN with an ON
/// or by a comma, with a WHERE most of the time: what the order check permutes.
pub fn inner_join(r: &mut Rng, schema: &Schema, funcs: &[Func]) -> Sel {
    let mut from = Vec::new();
    let mut tables: Vec<(String, String)> = Vec::new();
    for i in 0..2 + r.below(3) {
        let t = r.pick(&schema.tables).name.clone();
        let alias = format!("a{i}");
        tables.push((alias.clone(), t.clone()));
        let join = if i == 0 { Join::First } else if r.chance(0.7) { Join::Inner } else { Join::Comma };
        let on = (join == Join::Inner && r.chance(0.9)).then(|| Scope { schema, tables: tables.clone(), funcs, aggs: false }.pred(r, 2));
        from.push(From { table: t, alias, join, on, hint: Hint::None });
    }
    let s = scope(schema, &from, funcs, false);
    let cols = (0..1 + r.below(3)).map(|_| s.expr(r, 2)).collect();
    let wh = if r.chance(0.8) { Some(s.pred(r, 3)) } else { None };
    Sel { distinct: false, cols, from, wh, group: vec![], having: None, order: vec![], limit: None }
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

/// A query of any shape the generator makes: grouped and aggregated, ordered and limited.
pub fn query(r: &mut Rng, schema: &Schema, funcs: &[Func]) -> Sel {
    let mut q = plain(r, schema, funcs);
    if r.chance(0.3) {
        let s = scope(schema, &q.from, funcs, true);
        // GROUP BY's terms read rows, not aggregates, and an integer there is a column's number
        // to SQLite, so one is cast: the same value, read as a value
        let rows = scope(schema, &q.from, funcs, false);
        q.group = (0..1 + r.below(2)).map(|_| rows.expr(r, 1)).map(|e| if column_number(&e) { E::Cast(Box::new(e), "INTEGER") } else { e }).collect();
        q.cols = q.group.clone();
        q.cols.push(s.aggregate(r, 1));
        if r.chance(0.4) {
            q.having = Some(s.pred(r, 2));
        }
    } else if r.chance(0.15) {
        let s = scope(schema, &q.from, funcs, true);
        q.cols = (0..1 + r.below(2)).map(|_| s.aggregate(r, 1)).collect();
    }
    if !q.aggregates() && r.chance(0.2) {
        // ordered by every column it returns, exactly (BINARY, then the storage class, since 'a'
        // and 'A' tie under NOCASE and 1 and 1.0 tie as numbers), so the rows a LIMIT keeps are
        // certain
        // (an integer column, the same in every row, orders nothing, and SQLite would read it as
        // a column's number)
        let desc = r.chance(0.5);
        q.order = q.cols.iter().filter(|c| !column_number(c)).flat_map(|c| [(E::Collate(Box::new(c.clone()), "BINARY"), desc), (E::Fn("typeof".into(), vec![c.clone()]), desc)]).collect();
        q.limit = Some((r.below(5) as i64, r.below(3) as i64));
    }
    q
}
