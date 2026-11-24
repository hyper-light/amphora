//! T-PG.8.2: statements over the unit's types analyze to PostgreSQL 19.0's types and SQLSTATEs, and
//! their constants to its values. Each line of statements.txt is a `TimeZone`, a statement, its
//! parameters' values, and what the server answered (scripts/postgres-answers.sh, the postgres:19
//! image, `PostgreSQL 19.0 (Debian 19.0-1.pgdg13+1)`): the SQLSTATE PREPARE refused it with, or its
//! parameters' types, its columns' types and type modifiers, and the value it selects or stores.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::disallowed_macros
)]

mod common;

use amphora_pg_analysis::stmt::Command;
use amphora_pg_analysis::{Query, Session, eval, input, output, sqlstate, tz};
use amphora_pg_catalog::{data::type_oid, type_by_oid};

const STATEMENTS: &str = include_str!("statements.txt");

fn type_name(oid: u32) -> &'static str {
    type_by_oid(oid).unwrap().name
}

/// The value a statement selects first, or stores first, as text: its SQLSTATE where it is refused.
fn value(query: &Query, args: &str, session: &Session<'_>) -> Result<String, &'static str> {
    let mut params = Vec::new();
    for (arg, type_oid) in args.split(';').filter(|a| !a.is_empty()).zip(&query.params) {
        params.push(Some(
            input(*type_oid, arg, session).map_err(|e| e.sqlstate)?,
        ));
    }
    let expr = query.targets[0].expr;
    let type_oid = query.exprs.get(expr).unwrap().type_oid;
    match eval(&query.exprs, expr, session, &params).map_err(|e| e.sqlstate)? {
        None => Ok("NULL".to_owned()),
        Some(v) => output(type_oid, &v, session).map_err(|e| e.sqlstate),
    }
}

/// What analysis answers, in statements.txt's form; `valued` where the server's answer has a value.
fn answer(zone: &str, sql: &str, args: &str, valued: bool) -> String {
    let zone = tz::time_zone(zone).unwrap_or_else(|e| panic!("{zone}: {e:?}"));
    common::session(&zone, |session| {
        let query = match common::prepare(sql, &[], session) {
            Ok(query) => query,
            Err(e) if e.sqlstate == sqlstate::FEATURE_NOT_SUPPORTED => return e.message,
            Err(e) => return e.sqlstate.to_owned(),
        };
        let params: Vec<&str> = query.params.iter().map(|t| type_name(*t)).collect();
        let mut results = Vec::new();
        if query.command == Command::Select {
            for target in &query.targets {
                let e = query.exprs.get(target.expr).unwrap();
                results.push(if e.typmod < 0 || !params.is_empty() {
                    type_name(e.type_oid).to_owned()
                } else {
                    format!("{}:{}", type_name(e.type_oid), e.typmod)
                });
            }
        }
        let types = format!("{} -> {}", params.join(","), results.join(","));
        if valued {
            let value = value(&query, args, session);
            format!("{types} = {}", value.unwrap_or_else(str::to_owned))
        } else {
            types
        }
    })
}

/// T-PG.8.2: every statement answers as the server did. A numeric's value isn't this unit's: only its
/// type is compared.
#[test]
fn t_pg_8_2_each_statement_is_typed_as_19_types_it() {
    let mut wrong = Vec::new();
    for line in STATEMENTS.lines() {
        let fields: Vec<&str> = line.split('|').collect();
        let [zone, sql, args, server] = fields.as_slice() else {
            panic!("{line}");
        };
        let server = match server.split_once(" = ") {
            Some((types, _)) if types == " -> numeric" => types,
            _ => server,
        };
        let got = answer(zone, sql, args, server.contains(" = "));
        if got != *server {
            wrong.push(format!("{zone}|{sql}|{args}: {got}, 19: {server}"));
        }
    }
    assert_eq!(wrong, Vec::<String>::new());
    assert_eq!(STATEMENTS.lines().count(), 441);
}

/// T-PG.8.3: a statement that resolves to a function with no body refuses `0A000` naming it, in
/// analysis (docs/design/postgres.md section 4).
#[test]
fn t_pg_8_3_a_function_with_no_body_is_refused_by_name() {
    let utc = tz::time_zone("UTC").unwrap();
    common::session(&utc, |session| {
        for (sql, function) in [
            ("SELECT ts - ts FROM t", "timestamp_mi"),
            ("SELECT 1.5 + 1", "numeric_add"),
            ("SELECT 1::\"numeric\"", "numeric"),
            ("SELECT now()", "now"),
        ] {
            let e = common::prepare(sql, &[], session).unwrap_err();
            assert_eq!(e.sqlstate, sqlstate::FEATURE_NOT_SUPPORTED, "{sql}");
            assert!(e.message.contains(function), "{sql}: {}", e.message);
        }
    });
}

/// T-PG.8.4: pgbench's relkind query (src/bin/pgbench/pgbench.c:868) names its result and takes its
/// parameter as a regclass, whose text a table's name resolves through `search_path`.
#[test]
fn t_pg_8_4_a_regclass_parameter_resolves_a_table() {
    let utc = tz::time_zone("UTC").unwrap();
    common::session(&utc, |session| {
        let sql = "SELECT relkind FROM pg_catalog.pg_class WHERE oid=$1::pg_catalog.regclass";
        let query = common::prepare(sql, &[], session).unwrap();
        assert_eq!(query.params, [type_oid::REGCLASS]);
        assert_eq!(query.targets[0].name, "relkind");
        let table = input(query.params[0], "pgbench_accounts", session).unwrap();
        assert_eq!(
            output(type_oid::REGCLASS, &table, session).unwrap(),
            "pgbench_accounts"
        );
        assert_eq!(output(type_oid::OID, &table, session).unwrap(), "16384");
    });
}
