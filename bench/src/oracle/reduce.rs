//! Cutting a failing case down to what fails: statements dropped, an INSERT's rows dropped, the
//! query's clauses and columns dropped, and each expression node replaced by one of its children or
//! a literal, each cut kept only while the same oracle still fails the same way, until no cut does or
//! the time is spent. EET's two forms are cut together (the same clause from both), since a cut to
//! one alone would make them differ for no fault of Amphora's.

use std::time::Instant;

use super::checks::{Case, Failure, Kind};
use super::engines::V;
use super::synth::{Join, Sel, E};

#[derive(Clone, Debug)]
pub struct Repro {
    pub stmts: Vec<String>,
    pub case: Case,
}

/// The rows of an INSERT's VALUES, as the generator writes them: `(..), (..)`.
fn rows(stmt: &str) -> Option<(String, Vec<String>)> {
    let i = stmt.find(" VALUES (")?;
    let (head, tail) = stmt.split_at(i + " VALUES ".len());
    let inner = tail.strip_prefix('(')?.strip_suffix(')')?;
    Some((head.to_owned(), inner.split("), (").map(|r| format!("({r})")).collect()))
}

fn count(e: &E) -> usize {
    1 + e.children().into_iter().map(count).sum::<usize>()
}

fn nth_mut<'a>(e: &'a mut E, k: &mut usize) -> Option<&'a mut E> {
    if *k == 0 {
        return Some(e);
    }
    *k -= 1;
    for c in e.children_mut() {
        if let Some(x) = nth_mut(c, k) {
            return Some(x);
        }
    }
    None
}

/// Every expression of a query, as the roots an expression cut starts from.
fn roots(q: &mut Sel) -> Vec<&mut E> {
    let mut v: Vec<&mut E> = q.cols.iter_mut().collect();
    v.extend(q.wh.iter_mut());
    v.extend(q.having.iter_mut());
    v.extend(q.group.iter_mut());
    v.extend(q.order.iter_mut().map(|(e, _)| e));
    v.extend(q.from.iter_mut().filter_map(|f| f.on.as_mut()));
    v
}

/// The cuts to try, in order: whole statements, rows, clauses, expression nodes.
fn cuts(r: &Repro) -> Vec<Repro> {
    let mut out = Vec::new();
    for i in (0..r.stmts.len()).rev() {
        let mut c = r.clone();
        c.stmts.remove(i);
        out.push(c);
    }
    for (i, s) in r.stmts.iter().enumerate() {
        if let Some((head, rs)) = rows(s) {
            if rs.len() > 1 {
                for j in 0..rs.len() {
                    let mut c = r.clone();
                    let kept: Vec<&String> = rs.iter().enumerate().filter(|(k, _)| *k != j).map(|(_, x)| x).collect();
                    c.stmts[i] = format!("{head}{}", kept.iter().map(|x| x.as_str()).collect::<Vec<_>>().join(", "));
                    out.push(c);
                }
            }
        }
    }
    // a CERT compound's arms, each but the restricted one, with the operator before it (the first
    // arm's after it), so long as the restricted arm is still no EXCEPT's right side
    if let Case::Cert(q, _) = &r.case {
        for j in (0..q.arms.len()).rev().filter(|&j| j != q.arm) {
            let mut c = r.clone();
            if let Case::Cert(q, _) = &mut c.case {
                q.arms.remove(j);
                q.ops.remove(j.saturating_sub(1));
                if j < q.arm {
                    q.arm -= 1;
                }
                if q.arm == 0 || q.ops[q.arm - 1] != "EXCEPT" {
                    out.push(c);
                }
            }
        }
    }
    // clauses: the same cut to every query of the case (EET's two forms stay equivalent)
    let clause_cuts: Vec<Box<dyn Fn(&mut Sel) -> bool>> = vec![
        Box::new(|q: &mut Sel| q.wh.take().is_some()),
        Box::new(|q: &mut Sel| q.having.take().is_some()),
        Box::new(|q: &mut Sel| std::mem::take(&mut q.distinct)),
        Box::new(|q: &mut Sel| {
            let had = !q.order.is_empty() || q.limit.is_some();
            q.order.clear();
            q.limit = None;
            had
        }),
    ];
    let n_sels = {
        let mut c = r.clone();
        c.case.parts_mut().0.len()
    };
    for cut in &clause_cuts {
        let mut c = r.clone();
        let changed = c.case.parts_mut().0.into_iter().map(|q| cut(q)).fold(false, |a, b| a | b);
        if changed {
            out.push(c);
        }
    }
    let ncols = {
        let mut c = r.clone();
        c.case.parts_mut().0.first().map(|q| q.cols.len()).unwrap_or(0)
    };
    for k in 0..ncols {
        let mut c = r.clone();
        let mut ok = true;
        for q in c.case.parts_mut().0 {
            if q.cols.len() > 1 && k < q.cols.len() && q.group.is_empty() {
                q.cols.remove(k);
            } else {
                ok = false;
            }
        }
        if ok {
            out.push(c);
        }
    }
    let nfrom = {
        let mut c = r.clone();
        c.case.parts_mut().0.first().map(|q| q.from.len()).unwrap_or(0)
    };
    for k in (1..nfrom).rev() {
        let mut c = r.clone();
        for q in c.case.parts_mut().0 {
            if k < q.from.len() {
                q.from.remove(k);
            }
        }
        out.push(c);
        let mut c = r.clone();
        for q in c.case.parts_mut().0 {
            if let Some(f) = q.from.get_mut(k) {
                if f.on.is_some() || f.join != Join::Comma {
                    f.on = None;
                    f.join = Join::Comma;
                }
            }
        }
        out.push(c);
    }
    // expression nodes: not for EET, whose two forms an expression cut to one would part
    if n_sels == 1 || !matches!(r.case, Case::Eet(..)) {
        let mut probe = r.clone();
        let (sels, exprs) = probe.case.parts_mut();
        let mut sizes: Vec<(usize, usize, usize)> = Vec::new(); // (sel or usize::MAX for a loose expr, root, nodes)
        for (si, q) in sels.into_iter().enumerate() {
            for (ri, root) in roots(q).into_iter().enumerate() {
                sizes.push((si, ri, count(root)));
            }
        }
        for (ei, e) in exprs.into_iter().enumerate() {
            sizes.push((usize::MAX, ei, count(e)));
        }
        for (si, ri, n) in sizes {
            for k in 0..n {
                let mut c = r.clone();
                let replacements: Vec<E> = {
                    let node = target(&mut c, si, ri, k);
                    match node {
                        Some(e) => {
                            let mut v: Vec<E> = e.children().into_iter().cloned().collect();
                            if !matches!(e, E::Lit(_)) {
                                v.extend([E::Lit(V::Null), E::Lit(V::Int(0)), E::Lit(V::Int(1))]);
                            }
                            v
                        }
                        None => vec![],
                    }
                };
                for rep in replacements {
                    let mut c = r.clone();
                    if let Some(e) = target(&mut c, si, ri, k) {
                        *e = rep;
                        out.push(c);
                    }
                }
            }
        }
    }
    out
}

fn target(c: &mut Repro, si: usize, ri: usize, k: usize) -> Option<&mut E> {
    let (sels, exprs) = c.case.parts_mut();
    let root: &mut E = if si == usize::MAX {
        exprs.into_iter().nth(ri)?
    } else {
        let q = sels.into_iter().nth(si)?;
        roots(q).into_iter().nth(ri)?
    };
    let mut k = k;
    nth_mut(root, &mut k)
}

/// The smallest case `fails` still fails the same way on (it builds both databases from the
/// statements and runs the case), by `deadline`.
pub fn reduce(start: Repro, kind: Kind, deadline: Instant, mut fails: impl FnMut(&Repro) -> Option<Failure>) -> Repro {
    let mut cur = start;
    'outer: loop {
        for c in cuts(&cur) {
            if Instant::now() >= deadline {
                break 'outer;
            }
            if fails(&c).is_some_and(|f| f.kind == kind) {
                cur = c;
                continue 'outer;
            }
        }
        break;
    }
    cur
}
