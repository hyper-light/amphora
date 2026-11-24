//! T-PG.8.1: an operator, cast or function means what PostgreSQL 19.0 says it means, for every operator
//! name in its catalog and every pair of the unit's types. The answers are the server's (the
//! postgres:19 image, `PostgreSQL 19.0 (Debian 19.0-1.pgdg13+1)`, run by scripts/postgres-answers.sh).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::disallowed_macros
)]

use std::collections::BTreeMap;

use amphora_pg_analysis::coerce::{self, Context, Path};
use amphora_pg_analysis::resolve::{self, Call, Name};
use amphora_pg_catalog::data::{PG_OPERATOR, PG_PROC, TYPES};
use amphora_pg_catalog::{PG_CATALOG_NAMESPACE, SearchPath, type_named};

const OPERATORS: &str = include_str!("operators.txt");
const CASTS: &str = include_str!("casts.txt");
const FUNCTIONS: &str = include_str!("functions.txt");

const UNIT: &[&str] = &[
    "int4",
    "int8",
    "text",
    "bpchar",
    "\"char\"",
    "timestamp",
    "timestamptz",
    "regclass",
    "unknown",
];

fn type_oid(name: &str) -> u32 {
    type_named(PG_CATALOG_NAMESPACE, name.trim_matches('"'))
        .unwrap_or_else(|| panic!("{name}"))
        .oid
}

/// T-PG.8.1, operators: 19's operator for each name and pair of types, or its SQLSTATE. A pair the
/// answers don't list is `42883`.
#[test]
fn t_pg_8_1_each_operator_resolves_as_19_resolves_it() {
    let answers: BTreeMap<(&str, &str, &str), &str> = OPERATORS
        .lines()
        .map(|line| {
            let f: Vec<&str> = line.split(' ').collect();
            ((f[0], f[1], f[2]), f[3])
        })
        .collect();
    let path = SearchPath::new(&[], "", None, |_| None);
    let mut asked = 0;
    let mut answered = 0;
    let mut wrong = Vec::new();
    let mut forms: Vec<(&str, u8)> = PG_OPERATOR.iter().map(|o| (o.name, o.kind)).collect();
    forms.dedup();
    for (name, kind) in forms {
        let lefts: &[&str] = if kind == b'b' { UNIT } else { &["-"] };
        for left in lefts {
            for right in UNIT {
                asked += 1;
                let answer = answers.get(&(name, *left, *right)).copied();
                answered += usize::from(answer.is_some());
                let got = resolve::operator(
                    &path,
                    |_| None,
                    &Name {
                        schema: None,
                        name: name.to_owned(),
                    },
                    (kind == b'b').then(|| type_oid(left)),
                    type_oid(right),
                );
                let got = got.map_or_else(|e| e.sqlstate.to_owned(), |o| o.oid.to_string());
                if got != answer.unwrap_or("42883") {
                    wrong.push(format!("{left} {name} {right}: {got}, 19: {answer:?}"));
                }
            }
        }
    }
    assert_eq!(wrong, Vec::<String>::new(), "{} of {asked}", wrong.len());
    assert_eq!((asked, answered), (5697, answers.len()));
}

/// T-PG.8.1, functions: 19's function for each name and its one or two arguments' types (`coercion` for a
/// cast written as a call), or its SQLSTATE. A call the answers don't list is `42883`, and one that 19
/// refuses only once it has chosen the function (an aggregate with no OVER, a polymorphic argument of no
/// known type) is one it chose a function for.
#[test]
fn t_pg_8_1_each_function_resolves_as_19_resolves_it() {
    let answers: BTreeMap<(&str, &str, &str), &str> = FUNCTIONS
        .lines()
        .map(|line| {
            let f: Vec<&str> = line.split(' ').collect();
            ((f[0], f[1], f[2]), f[3])
        })
        .collect();
    let path = SearchPath::new(&[], "", None, |_| None);
    let mut wrong = Vec::new();
    let (mut asked, mut answered) = (0, 0);
    let mut names: Vec<&str> = PG_PROC.iter().map(|p| p.name).collect();
    names.sort_unstable();
    names.dedup();
    for name in names {
        let procs = || PG_PROC.iter().filter(|p| p.name == name);
        let two = procs().filter(|p| p.args.len() == 2).count() > 1;
        let one = procs().any(|p| !p.args.is_empty() && p.args.len() - p.defaults <= 1);
        let mut seconds: Vec<&str> = if one || !two { vec!["-"] } else { vec![] };
        seconds.extend(if two { UNIT } else { &[] });
        let firsts: &[&str] = if one || two { UNIT } else { &[] };
        for first in firsts {
            for second in &seconds {
                asked += 1;
                let answer = answers.get(&(name, *first, *second)).copied();
                answered += usize::from(answer.is_some());
                let mut args = vec![type_oid(first)];
                args.extend((*second != "-").then(|| type_oid(second)));
                let call = resolve::function(
                    &path,
                    |_| None,
                    &Name {
                        schema: None,
                        name: name.to_owned(),
                    },
                    &args,
                    true,
                );
                let got = match call {
                    Ok(Call::Function { function, .. }) => function.oid.to_string(),
                    Ok(Call::Coercion(_)) => "coercion".to_owned(),
                    Err(e) => e.sqlstate.to_owned(),
                };
                let chosen = !matches!(got.as_str(), "42883" | "42725");
                let right = match answer {
                    None => got == "42883",
                    Some("0A000" | "42804" | "42809" | "42P16") => chosen,
                    Some(answer) => got == answer,
                };
                if !right {
                    wrong.push(format!("{name}({first}, {second}): {got}, 19: {answer:?}"));
                }
            }
        }
    }
    assert_eq!(wrong, Vec::<String>::new(), "{} of {asked}", wrong.len());
    assert_eq!((asked, answered), (14382, answers.len()));
}

/// T-PG.8.1, casts: how each of the unit's types becomes another, as an explicit cast (19's node: `F`
/// and its function, `R` RelabelType, `I` CoerceViaIO, `C` the type itself or a literal read as it),
/// whether as an assignment, and whether implicitly.
#[test]
fn t_pg_8_1_each_cast_is_19s() {
    let mut wrong = Vec::new();
    for line in CASTS.lines() {
        let f: Vec<&str> = line.split(' ').collect();
        let (source, target) = (type_oid(f[0]), type_oid(f[1]));
        let explicit = if source == target || f[0] == "unknown" {
            "C".to_owned()
        } else {
            match coerce::pathway(target, source, Context::Explicit) {
                Path::Func(oid) => format!("F{oid}"),
                Path::Relabel => "R".to_owned(),
                Path::ViaIo => "I".to_owned(),
                Path::Array => "A".to_owned(),
                Path::None => "42846".to_owned(),
            }
        };
        let allowed = |context, refusal: &str| {
            if coerce::can_coerce(&[source], &[target], context) {
                "ok".to_owned()
            } else {
                refusal.to_owned()
            }
        };
        let got = [
            explicit,
            allowed(Context::Assignment, "42804"),
            allowed(Context::Implicit, "42883"),
        ];
        if got != f[2..] {
            wrong.push(format!("{line}: {got:?}"));
        }
    }
    assert_eq!(wrong, Vec::<String>::new());
    assert_eq!(CASTS.lines().count(), 216);
    assert!(TYPES.windows(2).all(|t| t[0].oid < t[1].oid));
}
