//! The schema the tests' statements are analyzed in: 19's own catalogs, pgbench's tables as pgbench.c's
//! createTables makes them, and a table with a column of each of the unit's types, as
//! scripts/postgres-answers.sql makes them on the server.

use amphora_pg_analysis::datetime::TimeZone;
use amphora_pg_analysis::{Column, Error, Query, Relations, Session, analyze};
use amphora_pg_catalog::data::{PG_ATTRIBUTE, PG_CLASS, PG_NAMESPACE, type_oid};
use amphora_pg_catalog::{Catalog, Datum, PG_CATALOG_NAMESPACE, SearchPath};
use amphora_pg_parser::gram;

pub const PUBLIC: u32 = 2200;

/// A table's name, and its columns' names, types and type modifiers.
type Table = (u32, &'static str, &'static [(&'static str, u32, i32)]);

const TABLES: &[Table] = &[
    (
        16384,
        "pgbench_accounts",
        &[
            ("aid", type_oid::INT4, -1),
            ("bid", type_oid::INT4, -1),
            ("abalance", type_oid::INT4, -1),
            ("filler", type_oid::BPCHAR, 88),
        ],
    ),
    (
        16388,
        "pgbench_history",
        &[
            ("tid", type_oid::INT4, -1),
            ("bid", type_oid::INT4, -1),
            ("aid", type_oid::INT4, -1),
            ("delta", type_oid::INT4, -1),
            ("mtime", type_oid::TIMESTAMP, -1),
            ("filler", type_oid::BPCHAR, 26),
        ],
    ),
    (
        16391,
        "t",
        &[
            ("i4", type_oid::INT4, -1),
            ("i8", type_oid::INT8, -1),
            ("tx", type_oid::TEXT, -1),
            ("bp", type_oid::BPCHAR, 9),
            ("ch", type_oid::CHAR, -1),
            ("ts", type_oid::TIMESTAMP, -1),
            ("tz", type_oid::TIMESTAMPTZ, -1),
            ("rc", type_oid::REGCLASS, -1),
        ],
    ),
];

pub struct Schema;

fn field(catalog: &Catalog, row: &[Datum<'static>], column: &str) -> Datum<'static> {
    row[catalog.column(column).unwrap()]
}

/// The catalog row whose `column` is `value`.
fn row(
    catalog: &'static Catalog,
    column: &str,
    value: Datum<'_>,
) -> Option<&'static [Datum<'static>]> {
    let rows = catalog.rows.iter();
    rows.copied().find(|r| field(catalog, r, column) == value)
}

fn oid(catalog: &Catalog, row: &[Datum<'static>]) -> Option<u32> {
    match field(catalog, row, "oid") {
        Datum::Oid(oid) => Some(oid),
        _ => None,
    }
}

impl Relations for Schema {
    fn namespace(&self, name: &str) -> Option<u32> {
        oid(
            &PG_NAMESPACE,
            row(&PG_NAMESPACE, "nspname", Datum::Name(name))?,
        )
    }

    fn relation(&self, namespace: u32, name: &str) -> Option<u32> {
        if namespace == PUBLIC {
            return TABLES.iter().find(|t| t.1 == name).map(|t| t.0);
        }
        let found = row(&PG_CLASS, "relname", Datum::Name(name))?;
        if field(&PG_CLASS, found, "relnamespace") != Datum::Oid(namespace) {
            return None;
        }
        oid(&PG_CLASS, found)
    }

    fn name(&self, oid: u32) -> Option<(u32, String)> {
        if let Some(t) = TABLES.iter().find(|t| t.0 == oid) {
            return Some((PUBLIC, t.1.to_owned()));
        }
        match field(
            &PG_CLASS,
            row(&PG_CLASS, "oid", Datum::Oid(oid))?,
            "relname",
        ) {
            Datum::Name(name) => Some((PG_CATALOG_NAMESPACE, name.to_owned())),
            _ => None,
        }
    }

    fn namespace_name(&self, oid: u32) -> Option<String> {
        let found = row(&PG_NAMESPACE, "oid", Datum::Oid(oid))?;
        match field(&PG_NAMESPACE, found, "nspname") {
            Datum::Name(name) => Some(name.to_owned()),
            _ => None,
        }
    }

    fn columns(&self, oid: u32) -> Vec<Column> {
        let column = |name: &str, type_oid: u32, typmod: i32| Column {
            name: name.to_owned(),
            type_oid,
            typmod,
        };
        if let Some(t) = TABLES.iter().find(|t| t.0 == oid) {
            return t.2.iter().map(|(n, t, m)| column(n, *t, *m)).collect();
        }
        let of = |r: &[Datum<'static>], name| field(&PG_ATTRIBUTE, r, name);
        let mut columns = Vec::new();
        for r in PG_ATTRIBUTE.rows {
            if of(r, "attrelid") != Datum::Oid(oid) {
                continue;
            }
            if let (Datum::Int2(1..), Datum::Name(name), Datum::Oid(t), Datum::Int4(m)) = (
                of(r, "attnum"),
                of(r, "attname"),
                of(r, "atttypid"),
                of(r, "atttypmod"),
            ) {
                columns.push(column(name, t, m));
            }
        }
        columns
    }
}

/// 19's default `search_path`, `"$user", public`, for a role with no schema of its name.
pub fn path() -> SearchPath {
    SearchPath::new(&["$user", "public"], "postgres", None, |name| {
        Schema.namespace(name)
    })
}

/// Runs `f` in a session under that path and a time zone.
pub fn session<T>(zone: &TimeZone, f: impl FnOnce(&Session<'_>) -> T) -> T {
    let path = path();
    f(&Session {
        path: &path,
        relations: &Schema,
        time_zone: zone,
        now: None,
    })
}

/// The one statement of `sql`, parsed and analyzed as PREPARE does with these parameter types.
pub fn prepare(sql: &str, params: &[u32], session: &Session<'_>) -> Result<Query, Error> {
    let (tree, statements) = gram::parse(sql.as_bytes())
        .map_err(|e| Error::new(e.sqlstate, format!("parse: {}", e.message)))?;
    analyze(&tree, statements[0], session, params)
}
