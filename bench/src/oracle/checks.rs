//! The oracles (docs/research/oracles.md), each a check a generated case passes or fails. A case is
//! data (its queries, its predicate, its plan variant, its rewrite), so the reducer can cut it down
//! and check it again. SQLite is the reference: every query is also compared with SQLite's answer
//! (Diff), and PQS rectifies its predicate by SQLite's evaluation; TLP, NoREC, DQP and EET hold
//! within Amphora alone, and CERT on Amphora's estimates.

use super::engines::{same_rows_by, show, Amphora, Engine, Rows, Sqlite, ILL_FORMED, V};
use super::synth::{self, Func, Hint, Join, Rng, Schema, Scope, Sel, E};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Oracle {
    /// The same statements leave the same rows in both engines.
    Same,
    Diff,
    Tlp,
    Norec,
    Pqs,
    Dqp,
    Eet,
    Cert,
    /// An inner join estimated at the same rows whatever order its tables are written or read in.
    Order,
}

impl Oracle {
    /// The oracles a run rotates through (`Same` runs once a database, after its statements).
    pub const ALL: [Oracle; 8] = [Oracle::Diff, Oracle::Tlp, Oracle::Norec, Oracle::Pqs, Oracle::Dqp, Oracle::Eet, Oracle::Cert, Oracle::Order];

    pub fn name(self) -> &'static str {
        match self {
            Oracle::Same => "same",
            Oracle::Diff => "diff",
            Oracle::Tlp => "tlp",
            Oracle::Norec => "norec",
            Oracle::Pqs => "pqs",
            Oracle::Dqp => "dqp",
            Oracle::Eet => "eet",
            Oracle::Cert => "cert",
            Oracle::Order => "order",
        }
    }

    pub fn parse(s: &str) -> Option<Oracle> {
        Oracle::ALL.into_iter().find(|o| o.name() == s)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Agg {
    Count,
    Sum,
    Min,
    Max,
}

/// Another plan for the same query, from what Amphora offers a query (docs/research/oracles.md):
/// a table read without its indexes or through one, the join order the FROM writes, or WHERE given
/// a likelihood hint. (The connection's subquery unnesting mode is a lever too, but only the
/// engine's simulator build has it, and the bench measures the production build.)
#[derive(Clone, Debug)]
pub enum Variant {
    NotIndexed(usize),
    IndexedBy(usize, String),
    CrossJoins,
    Likely,
    Unlikely,
}

/// How CERT makes its query more restrictive.
#[derive(Clone, Debug)]
pub enum More {
    /// A conjunct added to WHERE.
    Conjunct(E),
    /// The outer join at this place made inner.
    Inner(usize),
}

impl More {
    pub fn apply(&self, q: &Sel) -> Sel {
        let mut r = q.clone();
        match self {
            More::Conjunct(p) => r.wh = Some(and(&q.wh, p.clone())),
            More::Inner(i) => {
                if let Some(f) = r.from.get_mut(*i) {
                    f.join = Join::Inner;
                }
            }
        }
        r
    }
}

/// What CERT estimates: a SELECT, or a compound of SELECTs (`ops[i]` between arm i and arm i + 1,
/// read left to right), and the arm its restriction applies to. Never the right arm of an EXCEPT:
/// fewer rows there leave more in the answer.
#[derive(Clone, Debug)]
pub struct CertQuery {
    pub arms: Vec<Sel>,
    pub ops: Vec<&'static str>,
    pub arm: usize,
}

impl CertQuery {
    fn sql_of(arms: &[Sel], ops: &[&str]) -> String {
        let mut s = arms.first().map(Sel::sql).unwrap_or_default();
        for (op, arm) in ops.iter().zip(arms.iter().skip(1)) {
            s.push_str(&format!(" {op} {}", arm.sql()));
        }
        s
    }

    pub fn sql(&self) -> String {
        Self::sql_of(&self.arms, &self.ops)
    }

    /// The same query with `more` applied to its restricted arm.
    pub fn restricted(&self, more: &More) -> String {
        let arms: Vec<Sel> = self.arms.iter().enumerate().map(|(i, q)| if i == self.arm { more.apply(q) } else { q.clone() }).collect();
        Self::sql_of(&arms, &self.ops)
    }
}

#[derive(Clone, Debug)]
pub enum Case {
    /// The tables whose rows both engines must hold alike.
    Same(Vec<String>),
    Diff(Sel),
    TlpWhere(Sel, E),
    TlpDistinct(Sel, E),
    TlpGroup(Sel, E),
    TlpHaving(Sel, E),
    TlpAgg(Sel, E, Agg),
    Norec(Sel, E),
    Pqs(Sel, i64, E),
    Dqp(Sel, Variant),
    Eet(Sel, Sel),
    Cert(CertQuery, More),
    /// An inner join of two to four tables.
    Order(Sel),
}

impl Case {
    pub fn oracle(&self) -> Oracle {
        match self {
            Case::Same(_) => Oracle::Same,
            Case::Diff(_) => Oracle::Diff,
            Case::TlpWhere(..) | Case::TlpDistinct(..) | Case::TlpGroup(..) | Case::TlpHaving(..) | Case::TlpAgg(..) => Oracle::Tlp,
            Case::Norec(..) => Oracle::Norec,
            Case::Pqs(..) => Oracle::Pqs,
            Case::Dqp(..) => Oracle::Dqp,
            Case::Eet(..) => Oracle::Eet,
            Case::Cert(..) => Oracle::Cert,
            Case::Order(_) => Oracle::Order,
        }
    }

    /// The case's queries and expressions, for the reducer to cut down.
    pub fn parts_mut(&mut self) -> (Vec<&mut Sel>, Vec<&mut E>) {
        match self {
            Case::Same(_) => (vec![], vec![]),
            Case::Diff(q) | Case::Dqp(q, _) | Case::Order(q) => (vec![q], vec![]),
            Case::TlpWhere(q, p) | Case::TlpDistinct(q, p) | Case::TlpGroup(q, p) | Case::TlpHaving(q, p) | Case::TlpAgg(q, p, _) | Case::Norec(q, p) | Case::Pqs(q, _, p) => (vec![q], vec![p]),
            Case::Eet(q, t) => (vec![q, t], vec![]),
            Case::Cert(q, More::Conjunct(p)) => (q.arms.iter_mut().collect(), vec![p]),
            Case::Cert(q, More::Inner(_)) => (q.arms.iter_mut().collect(), vec![]),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Two answers that must agree don't.
    Wrong,
    /// Amphora refuses one form of a query and answers another, or one SQLite answers.
    Error,
    /// Amphora panicked: production code never does.
    Panic,
    /// A more restrictive query estimated to return more rows (CERT), or one join order estimated
    /// apart from another (Order).
    Estimate,
}

#[derive(Clone, Debug)]
pub struct Failure {
    pub kind: Kind,
    pub detail: String,
    pub queries: Vec<String>,
    /// Amphora's own error, for an `Error`: what names the finding (one cause reaches every oracle).
    pub cause: Option<String>,
}

pub enum Outcome {
    Pass { plan_changed: bool },
    /// Not checkable (why): SQLite refused the query, or Amphora refused its base form.
    Skip(&'static str),
    /// Amphora refused what SQLite answers: a missing feature, not a wrong answer (counted).
    Refused(String),
    Fail(Failure),
}

fn fail(kind: Kind, detail: String, queries: Vec<String>) -> Outcome {
    Outcome::Fail(Failure { kind, detail, queries, cause: None })
}

fn error(detail: String, cause: &str, queries: Vec<String>) -> Outcome {
    Outcome::Fail(Failure { kind: Kind::Error, detail, queries, cause: Some(cause.to_owned()) })
}

/// An error a value raises when it's evaluated (a JSON function's malformed input or path, a blob
/// too big, an overflow, a function's argument count checked as it runs): which plan evaluates it
/// on which rows is the engine's choice, so a query derived from another may meet it where its base
/// didn't, in SQLite as in Amphora.
fn value_error(e: &str) -> bool {
    ["too big", "malformed JSON", "JSON path", "bad JSON path", "JSON cannot hold", "overflow", "Unexpected character", "must be TEXT", "number of arguments", "argument count", "must be between", "not an array element", "single character", "Unexpected end of input"].iter().any(|m| e.contains(m))
}

/// Whether a TLP aggregate case's columns are still the one aggregate its partitions combine to:
/// count or sum of a value, min or max of one under BINARY (what the combined query applies).
fn is_partitioned_aggregate(cols: &[E], f: Agg) -> bool {
    let [E::Agg(name, false, args)] = cols else { return false };
    match f {
        Agg::Count => name == "count" && args.len() == 1,
        Agg::Sum => name == "sum" && args.len() == 1,
        Agg::Min => name == "min" && matches!(args.as_slice(), [E::Collate(_, "BINARY")]),
        Agg::Max => name == "max" && matches!(args.as_slice(), [E::Collate(_, "BINARY")]),
    }
}

fn and(w: &Option<E>, p: E) -> E {
    match w {
        Some(w) => E::Bin("AND", Box::new(w.clone()), Box::new(p)),
        None => p,
    }
}

/// The three ways a predicate can come out for a row: TRUE, FALSE, NULL.
fn partitions(p: &E) -> [E; 3] {
    [p.clone(), E::Un("NOT", Box::new(p.clone())), E::Null(Box::new(p.clone()), false)]
}

/// A query derived from the case's errors on Amphora: a failure only if SQLite runs it (a predicate
/// the base query never evaluated may raise an error in both: json_replace('a ') does).
fn derived(s: &mut Sqlite, sql: &str, what: &str, e: &str, queries: Vec<String>) -> Outcome {
    if value_error(e) {
        return Outcome::Skip("a derived query meets a value's error (evaluation order)");
    }
    // A case cut down past a table its predicate reads: SQLite resolves no name in `0 AND x`,
    // which its parser folds first, Amphora resolves them all.
    if e.contains("no such table") || e.contains("no such column") {
        return Outcome::Skip("a derived query reads a table no longer in its FROM");
    }
    if s.run(sql).is_err() {
        return Outcome::Skip("a derived query errors on SQLite too");
    }
    error(format!("{what} errors on Amphora, not on SQLite: {e}"), e, queries)
}

fn loose(q: &Sel) -> bool {
    q.distinct || q.aggregates()
}

fn wrong(what: &str, a: &Result<Rows, String>, b: &Result<Rows, String>, queries: Vec<String>) -> Outcome {
    fail(Kind::Wrong, format!("{what}\n  expected: {}\n  Amphora:  {}", show(b), show(a)), queries)
}

/// The queries a case runs, without running them: what a panic's finding shows.
pub fn sqls(case: &Case) -> Vec<String> {
    let with_where = |q: &Sel, p: &E| -> Vec<String> {
        std::iter::once(q.sql()).chain(partitions(p).into_iter().map(|part| {
            let mut qp = q.clone();
            qp.wh = Some(and(&q.wh, part));
            qp.sql()
        })).collect()
    };
    match case {
        Case::Same(tables) => tables.iter().map(|t| format!("SELECT rowid, * FROM {t}")).collect(),
        Case::Diff(q) | Case::Dqp(q, _) => vec![q.sql()],
        Case::TlpWhere(q, p) | Case::TlpDistinct(q, p) | Case::TlpGroup(q, p) | Case::TlpAgg(q, p, _) | Case::Norec(q, p) | Case::Pqs(q, _, p) => with_where(q, p),
        Case::TlpHaving(q, p) => std::iter::once(q.sql()).chain(partitions(p).into_iter().map(|part| {
            let mut qp = q.clone();
            qp.having = Some(and(&q.having, part));
            qp.sql()
        })).collect(),
        Case::Eet(q, t) => vec![q.sql(), t.sql()],
        Case::Cert(q, more) => vec![format!("EXPLAIN QUERY PLAN FORMAT=JSON {}", q.sql()), format!("EXPLAIN QUERY PLAN FORMAT=JSON {}", q.restricted(more))],
        Case::Order(q) => std::iter::once(q.clone()).chain(orders(q)).map(|o| format!("EXPLAIN QUERY PLAN FORMAT=JSON {}", o.sql())).collect(),
    }
}

/// Runs the case. Every query a case runs on Amphora that errors where its base form answered is a
/// failure; a base form Amphora refuses is a skip (or, against SQLite, a refusal counted).
pub fn check(case: &Case, a: &mut Amphora, s: &mut Sqlite) -> Outcome {
    let mut c = case.clone();
    // CERT reads estimates, not answers: which row of its group a bare column reads is no matter.
    if !matches!(case, Case::Cert(..)) && c.parts_mut().0.iter().any(|q| !q.certain()) {
        return Outcome::Skip("a bare column: its row is any of its group's");
    }
    match case {
        Case::Same(tables) => {
            for t in tables {
                let sql = format!("SELECT rowid, * FROM {t}");
                let (got, want) = (a.run(&sql), s.run(&sql));
                match (&got, &want) {
                    (Ok(g), Ok(w)) if same_rows_by(g, w, false) => {}
                    (Err(_), Err(_)) => {}
                    (_, Err(e)) if e == ILL_FORMED => return Outcome::Skip("SQLite's rows hold ill-formed UTF-8 (garbage in, garbage out)"),
                    _ => return wrong(&format!("the statements leave {t} with other rows"), &got, &want, vec![sql]),
                }
            }
            Outcome::Pass { plan_changed: false }
        }
        Case::Diff(q) => {
            let sql = q.sql();
            let want = match s.run(&sql) {
                Ok(want) => want,
                Err(e) if e == ILL_FORMED => return Outcome::Skip("SQLite's answer holds ill-formed UTF-8 (garbage in, garbage out)"),
                Err(_) => return Outcome::Skip("sqlite refused"),
            };
            match a.run(&sql) {
                Err(e) => Outcome::Refused(e),
                Ok(got) if same_rows_by(&got, &want, loose(q)) => Outcome::Pass { plan_changed: false },
                Ok(got) => wrong("Amphora's answer isn't SQLite's", &Ok(got), &Ok(want), vec![sql]),
            }
        }
        Case::TlpWhere(q, p) | Case::TlpHaving(q, p) => {
            let base = q.sql();
            let Ok(all) = a.run(&base) else { return Outcome::Skip("amphora refused the base") };
            let having = matches!(case, Case::TlpHaving(..));
            let mut sqls = vec![base];
            let mut union = Vec::new();
            for part in partitions(p) {
                let mut qp = q.clone();
                if having {
                    qp.having = Some(and(&q.having, part));
                } else {
                    qp.wh = Some(and(&q.wh, part));
                }
                let sql = qp.sql();
                sqls.push(sql.clone());
                match a.run(&sql) {
                    Ok(rows) => union.extend(rows),
                    Err(e) => return derived(s, &sql, "a partition", &e, sqls),
                }
            }
            if same_rows_by(&all, &union, loose(q)) {
                Outcome::Pass { plan_changed: false }
            } else {
                wrong("the query's rows aren't its three partitions' (p, NOT p, p IS NULL)", &Ok(union), &Ok(all), sqls)
            }
        }
        Case::TlpDistinct(q, p) | Case::TlpGroup(q, p) => {
            // Its rows are distinct, as the partitions' UNION's are, only while the query is
            // DISTINCT or groups by exactly what it selects: one cut down past that is no case.
            let distinct = match case {
                Case::TlpDistinct(..) => q.distinct,
                _ => !q.group.is_empty() && q.cols.iter().map(E::sql).eq(q.group.iter().map(E::sql)),
            };
            if !distinct {
                return Outcome::Skip("the query's rows are no longer distinct");
            }
            let base = q.sql();
            let Ok(all) = a.run(&base) else { return Outcome::Skip("amphora refused the base") };
            let parts: Vec<String> = partitions(p).into_iter().map(|part| {
                let mut qp = q.clone();
                qp.wh = Some(and(&q.wh, part));
                qp.sql()
            }).collect();
            // the engine's own UNION decides which values are equal, as its DISTINCT and GROUP BY do
            let union = parts.join(" UNION ");
            match a.run(&union) {
                Err(e) => derived(s, &union, "the partitions' UNION", &e, vec![base, union.clone()]),
                Ok(rows) if same_rows_by(&all, &rows, true) => Outcome::Pass { plan_changed: false },
                Ok(rows) => wrong("the query's rows aren't the UNION of its partitions'", &Ok(rows), &Ok(all), vec![base, union]),
            }
        }
        Case::TlpAgg(q, p, f) => {
            // The partitions combine to the query only while it is the aggregate itself: one cut
            // down past it (sum(e) to e, min(e COLLATE BINARY) to min(e)) is no case at all.
            if !is_partitioned_aggregate(&q.cols, *f) {
                return Outcome::Skip("the query is no longer the partitioned aggregate");
            }
            let base = q.sql();
            let Ok(all) = a.run(&base) else { return Outcome::Skip("amphora refused the base") };
            // A REAL sum is summed in its own order (Kahan-Babuska-Neumaier, as SQLite's): its
            // partitions' sums, summed again, may round apart from it.
            if matches!(f, Agg::Sum) && all.iter().flatten().any(|v| matches!(v, V::Real(_))) {
                return Outcome::Skip("a REAL sum's partitions round apart from it");
            }
            let parts: Vec<String> = partitions(p).into_iter().map(|part| {
                let mut qp = q.clone();
                qp.wh = Some(and(&q.wh, part));
                qp.sql()
            }).collect();
            let outer = match f {
                Agg::Count | Agg::Sum => "sum(m)",
                Agg::Min => "min(m COLLATE BINARY)",
                Agg::Max => "max(m COLLATE BINARY)",
            };
            // the first arm names the column and adds no row
            let combined = format!("SELECT {outer} FROM (SELECT NULL AS m WHERE 0 UNION ALL {})", parts.join(" UNION ALL "));
            match a.run(&combined) {
                Err(e) if e.contains("overflow") => Outcome::Skip("a partition's sum overflows"),
                Err(e) => derived(s, &combined, "the partitions' aggregate", &e, vec![base, combined.clone()]),
                Ok(rows) if same_rows_by(&all, &rows, true) => Outcome::Pass { plan_changed: false },
                Ok(rows) => wrong("the query's aggregate isn't its partitions' combined", &Ok(rows), &Ok(all), vec![base, combined]),
            }
        }
        Case::Norec(q, p) => {
            let mut opt = q.clone();
            opt.cols = vec![E::Lit(V::Int(1))];
            opt.wh = Some(p.clone());
            let mut unopt = q.clone();
            // 1 where p is TRUE (truth, as WHERE reads it: 5 is TRUE), else 0
            unopt.cols = vec![E::Case(None, vec![(p.clone(), E::Lit(V::Int(1)))], Some(Box::new(E::Lit(V::Int(0)))))];
            unopt.wh = None;
            let (os, us) = (opt.sql(), unopt.sql());
            let o = a.run(&os);
            let u = a.run(&us);
            match (o, u) {
                (Err(_), Err(_)) => Outcome::Skip("amphora refused both"),
                (Ok(_), Err(e)) => derived(s, &us, "the unoptimized form", &e, vec![os.clone(), us.clone()]),
                (Err(e), Ok(_)) => derived(s, &os, "the optimized form", &e, vec![os.clone(), us.clone()]),
                (Ok(o), Ok(u)) => {
                    let n = o.len() as i64;
                    let m = u.iter().filter(|r| matches!(r.first(), Some(V::Int(1)))).count() as i64;
                    if n == m {
                        Outcome::Pass { plan_changed: false }
                    } else {
                        fail(Kind::Wrong, format!("WHERE p keeps {n} rows; p is TRUE for {m} of them"), vec![os, us])
                    }
                }
            }
        }
        Case::Pqs(q, pivot, p) => {
            let t = &q.from[0];
            let eval = format!("SELECT ({}) IS TRUE, ({}) IS FALSE FROM {} AS {} WHERE {}.rowid = {pivot}", p.sql(), p.sql(), t.table, t.alias, t.alias);
            let Ok(ev) = s.run(&eval) else { return Outcome::Skip("sqlite can't evaluate it") };
            let Some(row) = ev.first() else { return Outcome::Skip("no pivot") };
            let rectified = match (row.first(), row.get(1)) {
                (Some(V::Int(1)), _) => p.clone(),
                (_, Some(V::Int(1))) => E::Un("NOT", Box::new(p.clone())),
                _ => E::Null(Box::new(p.clone()), false),
            };
            let mut find = q.clone();
            find.cols = vec![E::Col(t.alias.clone(), "rowid".into())];
            find.wh = Some(rectified);
            let sql = find.sql();
            let has = |rows: &Rows| rows.iter().any(|r| matches!(r.first(), Some(V::Int(i)) if i == pivot));
            match s.run(&sql) {
                Ok(rows) if has(&rows) => {}
                _ => return Outcome::Skip("sqlite misses its own pivot"),
            }
            match a.run(&sql) {
                Err(e) => error(format!("Amphora errors where SQLite finds the pivot: {e}"), &e, vec![eval, sql]),
                Ok(rows) if has(&rows) => Outcome::Pass { plan_changed: false },
                Ok(rows) => fail(Kind::Wrong, format!("the pivot (rowid {pivot}), whose predicate SQLite rectified to TRUE, is missing\n  Amphora: {}", show(&Ok(rows))), vec![eval, sql]),
            }
        }
        Case::Dqp(q, v) => {
            let base = q.sql();
            let Ok(want) = a.run(&base) else { return Outcome::Skip("amphora refused the base") };
            let plan = |a: &mut Amphora, sql: &str| a.run(&format!("EXPLAIN QUERY PLAN {sql}")).ok().map(|r| show(&Ok(r)));
            let before = plan(a, &base);
            let q2 = variant(q, v);
            let sql = q2.sql();
            let got = a.run(&sql);
            let after = plan(a, &sql);
            match got {
                Err(_) if matches!(v, Variant::IndexedBy(..)) => Outcome::Skip("the index can't serve the query"),
                Err(e) => derived(s, &sql, "another plan", &e, vec![base, sql.clone()]),
                Ok(rows) if same_rows_by(&rows, &want, loose(q)) => Outcome::Pass { plan_changed: before != after },
                Ok(rows) => wrong("another plan gives other rows", &Ok(rows), &Ok(want), vec![base, sql]),
            }
        }
        Case::Eet(q, t) => {
            let (qs, ts) = (q.sql(), t.sql());
            let Ok(want) = a.run(&qs) else { return Outcome::Skip("amphora refused the base") };
            match a.run(&ts) {
                Err(e) => derived(s, &ts, "the equivalent form", &e, vec![qs.clone(), ts.clone()]),
                Ok(got) if same_rows_by(&got, &want, loose(q)) => Outcome::Pass { plan_changed: false },
                Ok(got) => wrong("an equivalent form gives other rows", &Ok(got), &Ok(want), vec![qs, ts]),
            }
        }
        Case::Cert(q, more) => {
            let (qs, rs) = (q.sql(), q.restricted(more));
            let aliases: Vec<String> = q.arms.iter().flat_map(|arm| arm.from.iter().map(|f| f.alias.clone())).collect();
            // A query Amphora can't plan has no estimate to check: SQLite refuses it too (a call
            // to a function at an arity it hasn't), or it's a refusal counted as the differential
            // check counts one. Only a query planned without an estimate is "no estimate".
            let mut got = [None, None];
            for (slot, sql) in got.iter_mut().zip([&qs, &rs]) {
                match estimate(a, sql, &aliases) {
                    Ok(e) => *slot = e,
                    Err(_) if s.run(&format!("EXPLAIN QUERY PLAN {sql}")).is_err() => return Outcome::Skip("sqlite refused"),
                    Err(e) => return Outcome::Refused(e),
                }
            }
            let [Some(eq), Some(er)] = got else { return Outcome::Skip("no estimate") };
            if no_more(er, eq) {
                Outcome::Pass { plan_changed: false }
            } else {
                fail(Kind::Estimate, format!("the more restrictive query is estimated at {er} rows, the query at {eq}"), vec![qs, rs])
            }
        }
        Case::Order(q) => {
            if q.from.len() < 2 || !q.from.iter().all(|f| matches!(f.join, Join::First | Join::Inner | Join::Comma | Join::Cross)) {
                return Outcome::Skip("no longer an inner join of two tables or more");
            }
            let aliases: Vec<String> = q.from.iter().map(|f| f.alias.clone()).collect();
            let base = q.sql();
            let want = match estimate(a, &base, &aliases) {
                Ok(Some(want)) => want,
                Ok(None) => return Outcome::Skip("no estimate"),
                Err(_) => return Outcome::Skip("amphora refused the base"),
            };
            for o in orders(q) {
                let sql = o.sql();
                // A form Amphora refuses is a finding only where SQLite answers it, as for every
                // derived query: the estimate is this check's, the refusal the others'.
                if let Err(e) = a.run(&format!("EXPLAIN QUERY PLAN {sql}")) {
                    return derived(s, &sql, "another join order", &e, vec![base, sql.clone()]);
                }
                match estimate(a, &sql, &aliases) {
                    Ok(Some(got)) if same_estimate(got, want) => {}
                    Ok(Some(got)) => return fail(Kind::Estimate, format!("one join order is estimated at {got} rows, the query as written at {want}"), vec![base, sql]),
                    Ok(None) | Err(_) => return fail(Kind::Estimate, format!("one join order has no estimate, the query as written {want} rows"), vec![base, sql]),
                }
            }
            Outcome::Pass { plan_changed: false }
        }
    }
}

/// Whether two estimates of the same rows agree to within 1e-9 of the larger, floating point's
/// rounding over a product of a few factors taken in another order.
pub fn same_estimate(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs())
}

/// Every order of an inner join's tables, each written two ways: its tables listed in that order
/// with every ON term moved to WHERE (an inner join's ON filters as WHERE does), and the same order
/// forced by CROSS JOIN, which the planner never reorders (SQLite's `jointype` rule, which Amphora
/// keeps). The estimate of the set of tables joined is one number in every one of them.
pub fn orders(q: &Sel) -> Vec<Sel> {
    let mut flat = q.clone();
    let mut wh = q.wh.clone();
    for f in flat.from.iter_mut() {
        if let Some(on) = f.on.take() {
            wh = Some(match wh {
                Some(w) => E::Bin("AND", Box::new(w), Box::new(on)),
                None => on,
            });
        }
    }
    flat.wh = wh;
    let n = flat.from.len();
    // Heap's algorithm, iterative: every permutation of 0..n, n at most 4 here (24 orders).
    let mut perm: Vec<usize> = (0..n).collect();
    let mut c = vec![0usize; n];
    let mut perms = vec![perm.clone()];
    let mut i = 1;
    while i < n {
        if c[i] < i {
            if i % 2 == 0 {
                perm.swap(0, i);
            } else {
                perm.swap(c[i], i);
            }
            perms.push(perm.clone());
            c[i] += 1;
            i = 1;
        } else {
            c[i] = 0;
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(perms.len() * 2);
    for p in perms {
        for cross in [false, true] {
            let mut o = flat.clone();
            o.from = p.iter().enumerate().map(|(k, &j)| {
                let mut f = flat.from[j].clone();
                f.join = if k == 0 { Join::First } else if cross { Join::Cross } else { Join::Comma };
                f
            }).collect();
            out.push(o);
        }
    }
    out
}

/// The optimizer's estimate of a query's rows: the output of the last top-level access to one of
/// the query's own tables (`aliases`), its join order's end. A subquery's scans can sit at the top
/// level too (an uncorrelated EXISTS runs once, first), and so can a RIGHT JOIN's pass over its
/// unmatched rows: neither is the query's cardinality.
/// Whether `more` is no more than `base`, beyond floating point's rounding.
pub fn no_more(more: f64, base: f64) -> bool {
    more <= base * (1.0 + 1e-9) + 1e-9
}

/// The estimate of the last table the plan reads at its top level, whatever its alias (a replayed
/// finding's queries, whose aliases the file doesn't list).
pub fn estimate_any(a: &mut Amphora, sql: &str) -> Option<f64> {
    let rows = a.run(&format!("EXPLAIN QUERY PLAN FORMAT=JSON {sql}")).ok()?;
    let text = match rows.first()?.first()? {
        V::Text(t) => t.clone(),
        _ => return None,
    };
    let j: serde_json::Value = serde_json::from_str(&text).ok()?;
    j["output_rows"].as_f64().or_else(|| {
        j["nodes"].as_array()?.iter()
            .filter(|n| n["parent"].is_null())
            .filter_map(|n| n["op"]["estimate"]["output_rows"].as_f64())
            .next_back()
    })
}

/// The plan's estimate of `sql`'s rows, or Amphora's refusal to plan it.
fn estimate(a: &mut Amphora, sql: &str, aliases: &[String]) -> Result<Option<f64>, String> {
    let rows = a.run(&format!("EXPLAIN QUERY PLAN FORMAT=JSON {sql}"))?;
    let Some(V::Text(text)) = rows.first().and_then(|r| r.first()) else { return Ok(None) };
    let Ok(j) = serde_json::from_str::<serde_json::Value>(text) else { return Ok(None) };
    // The join's own estimate where the plan gives it; else the last table read at the top.
    Ok(j["output_rows"].as_f64().or_else(|| {
        j["nodes"].as_array()?.iter()
            .filter(|n| n["parent"].is_null() && n["op"]["alias"].as_str().is_some_and(|al| aliases.iter().any(|x| x == al)))
            .filter_map(|n| n["op"]["estimate"]["output_rows"].as_f64())
            .last()
    }))
}

fn variant(q: &Sel, v: &Variant) -> Sel {
    let mut q = q.clone();
    match v {
        Variant::NotIndexed(i) => q.from[*i].hint = Hint::NotIndexed,
        Variant::IndexedBy(i, ix) => q.from[*i].hint = Hint::IndexedBy(ix.clone()),
        Variant::CrossJoins => {
            for f in q.from.iter_mut().skip(1) {
                if matches!(f.join, Join::Inner | Join::Comma) {
                    f.join = Join::Cross;
                }
            }
        }
        Variant::Likely | Variant::Unlikely => {
            let hint = if matches!(v, Variant::Likely) { "likely" } else { "unlikely" };
            q.wh = q.wh.take().map(|w| E::Fn(hint.into(), vec![w]));
        }
    }
    q
}

/// A HAVING predicate: aggregates, the group's own expressions and literals, never a bare column
/// (whose row in the group would be anyone's).
fn having_pred(r: &mut Rng, s: &Scope, group: &[E], depth: u32) -> E {
    let term = |r: &mut Rng| match r.below(3) {
        0 => s.expr_agg(r),
        1 if !group.is_empty() => r.pick(group).clone(),
        _ => E::Lit(synth::literal(r)),
    };
    match r.below(6) {
        0 if depth > 0 => E::Bin("AND", Box::new(having_pred(r, s, group, depth - 1)), Box::new(having_pred(r, s, group, depth - 1))),
        1 if depth > 0 => E::Bin("OR", Box::new(having_pred(r, s, group, depth - 1)), Box::new(having_pred(r, s, group, depth - 1))),
        2 => E::Null(Box::new(term(r)), r.chance(0.5)),
        _ => E::Bin(*r.pick(&["=", "<", ">", "<=", ">=", "<>", "IS", "IS NOT"]), Box::new(term(r)), Box::new(term(r))),
    }
}

/// A case for the oracle, over the database (its schema; SQLite for the rows PQS pivots on).
pub fn build(o: Oracle, r: &mut Rng, schema: &Schema, funcs: &[Func], s: &mut Sqlite) -> Option<Case> {
    Some(match o {
        Oracle::Same => Case::Same(schema.tables.iter().map(|t| t.name.clone()).collect()),
        Oracle::Diff => Case::Diff(synth::query(r, schema, funcs)),
        Oracle::Tlp => {
            let mut q = synth::plain(r, schema, funcs);
            q.distinct = false;
            let sc = synth::scope(schema, &q.from, funcs, false);
            let p = sc.pred(r, 3);
            match r.below(5) {
                0 => Case::TlpWhere(q, p),
                1 => {
                    q.distinct = true;
                    Case::TlpDistinct(q, p)
                }
                2 => {
                    q.group = (0..1 + r.below(2)).map(|_| sc.expr(r, 1)).collect();
                    q.cols = q.group.clone();
                    Case::TlpGroup(q, p)
                }
                3 => {
                    let sa = synth::scope(schema, &q.from, funcs, true);
                    q.group = (0..1 + r.below(2)).map(|_| sc.expr(r, 1)).collect();
                    q.cols = q.group.clone();
                    q.cols.push(sa.expr_agg(r));
                    q.having = if r.chance(0.5) { Some(having_pred(r, &sa, &q.group, 1)) } else { None };
                    let hp = having_pred(r, &sa, &q.group, 2);
                    Case::TlpHaving(q, hp)
                }
                _ => {
                    let f = *r.pick(&[Agg::Count, Agg::Sum, Agg::Min, Agg::Max]);
                    let e = sc.expr(r, 2);
                    q.cols = vec![match f {
                        Agg::Count => E::Agg("count".into(), false, vec![e]),
                        Agg::Sum => E::Agg("sum".into(), false, vec![e]),
                        Agg::Min => E::Agg("min".into(), false, vec![E::Collate(Box::new(e), "BINARY")]),
                        Agg::Max => E::Agg("max".into(), false, vec![E::Collate(Box::new(e), "BINARY")]),
                    }];
                    Case::TlpAgg(q, p, f)
                }
            }
        }
        Oracle::Norec => {
            let q = synth::plain(r, schema, funcs);
            let p = synth::scope(schema, &q.from, funcs, false).pred(r, 3);
            Case::Norec(q, p)
        }
        Oracle::Pqs => {
            let t = r.pick(&schema.tables).name.clone();
            let rowids = s.run(&format!("SELECT rowid FROM {t}")).ok()?;
            let pivot = match rowids.get(r.below(rowids.len()))?.first()? {
                V::Int(i) => *i,
                _ => return None,
            };
            let from = vec![synth::From { table: t, alias: "a0".into(), join: Join::First, on: None, hint: Hint::None }];
            let p = synth::scope(schema, &from, funcs, false).pred(r, 3);
            Case::Pqs(Sel { distinct: false, cols: vec![], from, wh: None, group: vec![], having: None, order: vec![], limit: None }, pivot, p)
        }
        Oracle::Dqp => {
            let q = synth::query(r, schema, funcs);
            let mut vs = vec![];
            for (i, f) in q.from.iter().enumerate() {
                vs.push(Variant::NotIndexed(i));
                for (ix, t) in &schema.indexes {
                    if *t == f.table {
                        vs.push(Variant::IndexedBy(i, ix.clone()));
                    }
                }
            }
            if q.from.iter().skip(1).any(|f| matches!(f.join, Join::Inner | Join::Comma)) {
                vs.push(Variant::CrossJoins);
            }
            if q.wh.is_some() {
                vs.push(Variant::Likely);
                vs.push(Variant::Unlikely);
            }
            let v = r.pick(&vs).clone();
            Case::Dqp(q, v)
        }
        Oracle::Eet => {
            let q = synth::query(r, schema, funcs);
            let t = transform(&q, r, schema, funcs);
            Case::Eet(q, t)
        }
        Oracle::Cert => {
            // Any shape the generator makes, grouped, aggregated, DISTINCT, ordered and limited,
            // but with no HAVING: fewer rows in a group can pass a HAVING more rows fail
            // (`count(*) < 2`), so a restriction there is none.
            let arm = |r: &mut Rng| {
                let mut q = synth::query(r, schema, funcs);
                q.having = None;
                q
            };
            let mut arms = vec![arm(r)];
            let mut ops = Vec::new();
            // A compound select a quarter of the time: two or three arms of the first's width, no
            // ORDER BY or LIMIT in any (SQLite takes them only at the compound's end).
            if r.chance(0.25) {
                for _ in 0..1 + r.below(2) {
                    let mut next = arm(r);
                    let width = arms[0].cols.len();
                    next.cols.truncate(width);
                    while next.cols.len() < width {
                        next.cols.push(E::Lit(synth::literal(r)));
                    }
                    ops.push(*r.pick(&["UNION ALL", "UNION", "INTERSECT", "EXCEPT"]));
                    arms.push(next);
                }
                for a in arms.iter_mut() {
                    a.order.clear();
                    a.limit = None;
                }
            }
            let places: Vec<usize> = (0..arms.len()).filter(|&k| k == 0 || ops[k - 1] != "EXCEPT").collect();
            let k = *r.pick(&places);
            let q = &arms[k];
            let outer = q.from.iter().enumerate().skip(1).find(|(_, f)| matches!(f.join, Join::Left | Join::Right | Join::Full)).map(|(i, _)| i);
            let more = match outer {
                Some(i) if r.chance(0.5) => More::Inner(i),
                _ => More::Conjunct(synth::scope(schema, &q.from, funcs, false).pred(r, 2)),
            };
            Case::Cert(CertQuery { arms, ops, arm: k }, more)
        }
        Oracle::Order => Case::Order(synth::inner_join(r, schema, funcs)),
    })
}

// ---------------------------------------------------------------- EET

fn true_expr(q: &E) -> E {
    E::Bin("OR", Box::new(E::Bin("OR", Box::new(q.clone()), Box::new(E::Un("NOT", Box::new(q.clone()))))), Box::new(E::Null(Box::new(q.clone()), false)))
}

fn false_expr(q: &E) -> E {
    E::Bin("AND", Box::new(E::Bin("AND", Box::new(q.clone()), Box::new(E::Un("NOT", Box::new(q.clone()))))), Box::new(E::Null(Box::new(q.clone()), true)))
}

/// Visits every rewritable node: `truth` where only its truth is read (WHERE, HAVING, ON, a searched
/// CASE's condition, an operand of AND, OR or NOT); `value` where its affinity and collation are its
/// own to lose, since a CASE has neither (datatype3.html 3.2, 7.1): not a column, a CAST, a COLLATE,
/// a unary + (which keeps a column's collation), a scalar subquery (which takes its column's
/// affinity), or likely(), unlikely() and likelihood() (which SQLite sees through).
fn visit(e: &mut E, truth: bool, f: &mut dyn FnMut(&mut E, bool, bool)) {
    let transparent = matches!(e, E::Fn(name, _) if matches!(name.as_str(), "likely" | "unlikely" | "likelihood"));
    let value = !transparent && !matches!(e, E::Col(..) | E::Cast(..) | E::Collate(..) | E::Scalar(_) | E::Un("+", _));
    f(e, truth, value);
    if let E::Case(base, whens, els) = e {
        let searched = base.is_none();
        if let Some(b) = base {
            visit(b, false, f);
        }
        for (w, t) in whens.iter_mut() {
            visit(w, searched, f);
            visit(t, false, f);
        }
        if let Some(x) = els {
            visit(x, false, f);
        }
        return;
    }
    let logic = matches!(e, E::Bin("AND" | "OR", ..) | E::Un("NOT", _));
    for c in e.children_mut() {
        visit(c, logic, f);
    }
}

fn has_collate(e: &E) -> bool {
    let mut found = false;
    e.walk(&mut |x| found |= matches!(x, E::Collate(..)));
    found
}

/// The query with one to three of EET's rules applied (Table 2), each where it holds in SQLite.
pub fn transform(q: &Sel, r: &mut Rng, schema: &Schema, funcs: &[Func]) -> Sel {
    let mut t = q.clone();
    let agg = q.aggregates();
    // what a rule's random predicate may read: the rows' columns, unless the query groups (a bare
    // column there reads any row of the group)
    let sc = synth::scope(schema, &q.from, funcs, false);
    for _ in 0..1 + r.below(3) {
        let mut slots = 0usize;
        let mut count = |_: &mut E, truth: bool, value: bool| slots += usize::from(truth) + usize::from(value);
        for_each_root(&mut t, &mut count);
        if slots == 0 {
            break;
        }
        let mut k = r.below(slots);
        let in_on = k >= slots_before_on(&mut t);
        let rule = r.below(6);
        // a COLLATE anywhere in a comparison's operand decides its collation (datatype3.html 7.1):
        // the rule's own parts bring none
        // in an ON clause, a literal: SQLite lets an inner join's ON name a table to its right, Amphora
        // doesn't (a finding of its own, made once, not by every EET case)
        let mut q1 = if agg || in_on { E::Lit(synth::literal(r)) } else { sc.pred(r, 1) };
        if q1.has_agg() || has_collate(&q1) {
            q1 = E::Lit(synth::literal(r));
        }
        let other = E::Lit(synth::literal(r));
        let mut apply = |e: &mut E, truth: bool, value: bool| {
            for (is, ok) in [(true, truth), (false, value)] {
                if !ok {
                    continue;
                }
                if k > 0 {
                    k -= 1;
                    continue;
                }
                k = usize::MAX;
                let old = e.clone();
                *e = if is {
                    if rule % 2 == 0 { E::Bin("OR", Box::new(false_expr(&q1)), Box::new(old)) } else { E::Bin("AND", Box::new(true_expr(&q1)), Box::new(old)) }
                } else {
                    match rule % 4 {
                        0 => E::Case(None, vec![(false_expr(&q1), other.clone())], Some(Box::new(old))),
                        1 => E::Case(None, vec![(true_expr(&q1), old)], Some(Box::new(other.clone()))),
                        _ => E::Case(None, vec![(q1.clone(), old.clone())], Some(Box::new(old))),
                    }
                };
                return;
            }
        };
        for_each_root(&mut t, &mut apply);
    }
    t
}

/// Each expression of the query, with whether it's read for its truth at the root; the ON clauses
/// last, so the slots before them (slots_before_on) tell a rule whether it's in one.
fn for_each_root(q: &mut Sel, f: &mut dyn FnMut(&mut E, bool, bool)) {
    for c in q.cols.iter_mut() {
        visit(c, false, f);
    }
    if let Some(w) = q.wh.as_mut() {
        visit(w, true, f);
    }
    if let Some(h) = q.having.as_mut() {
        visit(h, true, f);
    }
    for fr in q.from.iter_mut() {
        if let Some(on) = fr.on.as_mut() {
            visit(on, true, f);
        }
    }
}

/// How many slots the query's expressions outside its ON clauses hold.
fn slots_before_on(q: &mut Sel) -> usize {
    let mut n = 0usize;
    let mut count = |_: &mut E, truth: bool, value: bool| n += usize::from(truth) + usize::from(value);
    for c in q.cols.iter_mut() {
        visit(c, false, &mut count);
    }
    if let Some(w) = q.wh.as_mut() {
        visit(w, true, &mut count);
    }
    if let Some(h) = q.having.as_mut() {
        visit(h, true, &mut count);
    }
    n
}
