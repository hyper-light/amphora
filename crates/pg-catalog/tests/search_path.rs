//! T-PG.7.2: an unqualified table, type or function name resolves as PostgreSQL 19 resolves it.
//! Each expectation cites 19's documentation (doc/src/sgml/config.sgml, `search_path`) or its
//! source (src/backend/catalog/namespace.c) at REL_19_0.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::disallowed_macros
)]

use amphora_pg_catalog::data::{PG_CLASS, PG_NAMESPACE, PG_PROC, PG_TYPE};
use amphora_pg_catalog::{Catalog, Datum, Kind, PG_CATALOG_NAMESPACE, Proc, SearchPath};

const PUBLIC: u32 = 2200;
const S: u32 = 16384;
const ALICE: u32 = 16385;
const TEMP: u32 = 16386;
const PUBLIC_PG_CLASS: u32 = 16390;
const PUBLIC_T: u32 = 16391;
const TEMP_T: u32 = 16392;
const S_INT4: u32 = 16393;
const TEXT: u32 = 25;
const INT4: u32 = 23;
const ANYRANGE: u32 = 3831;
const ANYMULTIRANGE: u32 = 4537;

/// A user function, as CREATE FUNCTION's defaults make it.
const USER_PROC: Proc = Proc {
    oid: 0,
    name: "lower",
    namespace: PUBLIC,
    kind: b'f',
    strict: false,
    retset: false,
    volatile: b'v',
    variadic: 0,
    defaults: 0,
    result: TEXT,
    args: &[TEXT],
    src: "",
};

static USER_PROCS: &[Proc] = &[
    Proc {
        oid: 16394,
        ..USER_PROC
    },
    Proc {
        oid: 16395,
        namespace: TEMP,
        ..USER_PROC
    },
    Proc {
        oid: 16396,
        args: &[INT4],
        ..USER_PROC
    },
];

fn namespace(name: &str) -> Option<u32> {
    let user = [("s", S), ("alice", ALICE), ("pg_temp_3", TEMP)];
    builtin(&PG_NAMESPACE, "nspname", name, None)
        .or_else(|| user.iter().find(|(n, _)| *n == name).map(|(_, oid)| *oid))
}

/// The OID of the built-in row of `catalog` whose `column` is `name` (in `namespace`, if given).
fn builtin(
    catalog: &Catalog,
    column: &str,
    name: &str,
    namespace: Option<(&str, u32)>,
) -> Option<u32> {
    let (oid, by) = (
        catalog.column("oid").unwrap(),
        catalog.column(column).unwrap(),
    );
    let ns = namespace.map(|(c, oid)| (catalog.column(c).unwrap(), oid));
    catalog.rows.iter().find_map(|row| {
        let in_ns = ns.is_none_or(|(c, want)| row[c] == Datum::Oid(want));
        match (row[oid], row[by]) {
            (Datum::Oid(oid), Datum::Name(n)) if n == name && in_ns => Some(oid),
            _ => None,
        }
    })
}

fn relation(path: &SearchPath, name: &str) -> Option<u32> {
    let user = [
        (PUBLIC, "pg_class", PUBLIC_PG_CLASS),
        (PUBLIC, "t", PUBLIC_T),
        (TEMP, "t", TEMP_T),
    ];
    path.resolve(Kind::Relation, |ns| {
        builtin(&PG_CLASS, "relname", name, Some(("relnamespace", ns))).or_else(|| {
            user.iter()
                .find(|(n, r, _)| *n == ns && *r == name)
                .map(|(_, _, oid)| *oid)
        })
    })
}

fn type_(path: &SearchPath, name: &str) -> Option<u32> {
    path.resolve(Kind::Type, |ns| {
        builtin(&PG_TYPE, "typname", name, Some(("typnamespace", ns))).or(
            if ns == S && name == "int4" {
                Some(S_INT4)
            } else {
                None
            },
        )
    })
}

fn lower_candidates(path: &SearchPath) -> Vec<u32> {
    path.function_candidates("lower", PG_PROC.iter().chain(USER_PROCS))
        .iter()
        .map(|p| p.oid)
        .collect()
}

fn pg_catalog_lower(args: u32) -> u32 {
    PG_PROC
        .iter()
        .find(|p| p.name == "lower" && p.namespace == PG_CATALOG_NAMESPACE && p.args == [args])
        .unwrap()
        .oid
}

#[test]
fn t_pg_7_2_pg_catalog_is_searched_before_the_path_unless_listed() {
    // config.sgml:10345-10349: "If pg_catalog is not in the path then it will be searched before
    // searching any of the path items." 19's default path is "$user", public
    // (src/backend/utils/misc/guc_parameters.dat:2609).
    let default = SearchPath::new(&["$user", "public"], "postgres", None, namespace);
    assert_eq!(
        default.namespaces(Kind::Relation).collect::<Vec<_>>(),
        [PG_CATALOG_NAMESPACE, PUBLIC]
    );
    assert_eq!(
        relation(&default, "pg_class"),
        Some(1259),
        "pg_catalog's pg_class wins over public's"
    );
    assert_eq!(relation(&default, "t"), Some(PUBLIC_T));

    // config.sgml:10346-10348: "If it is mentioned in the path then it will be searched in the
    // specified order."
    let listed = SearchPath::new(&["public", "pg_catalog"], "postgres", None, namespace);
    assert_eq!(relation(&listed, "pg_class"), Some(PUBLIC_PG_CLASS));
}

#[test]
fn t_pg_7_2_a_type_name_resolves_through_the_path() {
    // The same rule for types (namespace.c:1009-1024, TypenameGetTypidExtended walks the path).
    let s_only = SearchPath::new(&["s"], "postgres", None, namespace);
    assert_eq!(
        type_(&s_only, "int4"),
        Some(INT4),
        "pg_catalog, searched first, holds int4"
    );
    let s_first = SearchPath::new(&["s", "pg_catalog"], "postgres", None, namespace);
    assert_eq!(type_(&s_first, "int4"), Some(S_INT4));
    assert_eq!(type_(&s_first, "text"), Some(TEXT));
}

#[test]
fn t_pg_7_2_a_function_name_resolves_through_the_path() {
    // FuncnameGetCandidates (namespace.c:1199-1400): every function of the name in a schema on the
    // path; one in an earlier schema hides one with the same argument types in a later schema.
    let default = SearchPath::new(&["$user", "public"], "postgres", None, namespace);
    assert_eq!(
        lower_candidates(&default),
        [
            pg_catalog_lower(TEXT),
            pg_catalog_lower(ANYRANGE),
            pg_catalog_lower(ANYMULTIRANGE),
            16396
        ]
    );
    let public_first = SearchPath::new(&["public", "pg_catalog"], "postgres", None, namespace);
    let candidates = lower_candidates(&public_first);
    assert_eq!(
        candidates[..2],
        [16394, 16396],
        "public's lower(text) hides pg_catalog's"
    );
    assert!(!candidates.contains(&pg_catalog_lower(TEXT)));
}

#[test]
fn t_pg_7_2_the_temporary_schema_is_first_for_tables_and_never_for_functions() {
    // config.sgml:10357-10364: the temporary schema "is always searched if it exists ... If it is
    // not listed in the path then it is searched first (even before pg_catalog). However, the
    // temporary schema is only searched for relation (table, view, sequence, etc.) and data type
    // names. It is never searched for function or operator names."
    let unlisted = SearchPath::new(&["public"], "postgres", Some(TEMP), namespace);
    assert_eq!(
        unlisted.namespaces(Kind::Relation).collect::<Vec<_>>(),
        [TEMP, PG_CATALOG_NAMESPACE, PUBLIC]
    );
    assert_eq!(relation(&unlisted, "t"), Some(TEMP_T));
    let after_public = SearchPath::new(&["public", "pg_temp"], "postgres", Some(TEMP), namespace);
    assert_eq!(relation(&after_public, "t"), Some(PUBLIC_T));
    let temp_first = SearchPath::new(
        &["pg_temp", "public", "pg_catalog"],
        "postgres",
        Some(TEMP),
        namespace,
    );
    assert_eq!(relation(&temp_first, "t"), Some(TEMP_T));
    assert!(
        !lower_candidates(&temp_first).contains(&16395),
        "functions skip pg_temp"
    );
    assert_eq!(lower_candidates(&temp_first)[0], 16394);
}

#[test]
fn t_pg_7_2_user_and_missing_schemas() {
    // config.sgml:10338-10341: "$user" names the schema of the current user's name, "(If not, $user
    // is ignored.)"; config.sgml:10331-10333: a schema that does not exist "is silently ignored".
    // finalNamespacePath drops repeats (namespace.c:4285-4291).
    let alice = SearchPath::new(&["$user", "public"], "alice", None, namespace);
    assert_eq!(
        alice.namespaces(Kind::Relation).collect::<Vec<_>>(),
        [PG_CATALOG_NAMESPACE, ALICE, PUBLIC]
    );
    let repeats = SearchPath::new(
        &["public", "nosuch", "public", "pg_temp"],
        "postgres",
        None,
        namespace,
    );
    assert_eq!(
        repeats.namespaces(Kind::Relation).collect::<Vec<_>>(),
        [PG_CATALOG_NAMESPACE, PUBLIC]
    );
}
