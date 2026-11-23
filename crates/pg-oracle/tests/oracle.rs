//! The oracle against PostgreSQL 19.0 itself (section 11's unit 3), on 19's regression SQL as
//! vendored (vendor/postgres). These need the built oracle, so they run on demand, after
//! scripts/postgres-oracle-extract.sh: `AMPHORA_PG_ORACLE=<work dir>/pg-oracle cargo test -p
//! amphora-pg-oracle --test oracle -- --ignored --nocapture`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::arithmetic_side_effects, clippy::disallowed_macros)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use amphora_pg_oracle::{Oracle, Verdict, split};

fn env(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("{name} is unset: see this file's head")))
}

/// T-PG.3.3: the reference answers taken from a real PostgreSQL 19.0 server.
#[test]
#[ignore = "needs AMPHORA_PG_ORACLE, built by scripts/postgres-oracle-extract.sh"]
fn t_pg_3_3_refuses_what_a_19_0_server_refuses_where_it_refuses_it() {
    let mut oracle = Oracle::spawn(&env("AMPHORA_PG_ORACLE")).unwrap();
    assert_eq!(
        oracle.parse(b"SELECT $1abc").unwrap(),
        Verdict::Refused { sqlstate: "42601".into(), cursor: 8, message: "trailing junk after parameter at or near \"$1abc\"".into() }
    );
    match oracle.parse(b"NOTIFY ch, $1").unwrap() {
        Verdict::Refused { sqlstate, cursor, .. } => assert_eq!((sqlstate.as_str(), cursor), ("42601", 12)),
        tree => panic!("19.0 refuses NOTIFY ch, $1; the oracle gave {tree:?}"),
    }
    match oracle.parse(b"SELECT 1").unwrap() {
        Verdict::Tree(tree) => assert!(tree.starts_with("({RAWSTMT :stmt {SELECTSTMT"), "{tree}"),
        refused => panic!("SELECT 1 refused: {refused:?}"),
    }
}

/// T-PG.3.4: every statement psql sends from 19's regression SQL gets the oracle's verdict.
#[test]
#[ignore = "needs AMPHORA_PG_ORACLE, built by scripts/postgres-oracle-extract.sh"]
fn t_pg_3_4_every_regression_statement_gets_a_verdict() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor/postgres/src/test/regress/sql");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x == "sql")).collect();
    files.sort();
    let mut oracle = Oracle::spawn(&env("AMPHORA_PG_ORACLE")).unwrap();
    let (mut statements, mut accepted, mut not_utf8, mut bytes) = (0usize, 0usize, 0usize, 0usize);
    let mut refused: BTreeMap<String, usize> = BTreeMap::new();
    for file in &files {
        for sql in split::statements(&std::fs::read(file).unwrap()) {
            if std::str::from_utf8(&sql).is_err() {
                not_utf8 += 1;
                continue;
            }
            statements += 1;
            bytes += sql.len();
            match oracle.parse(&sql).unwrap_or_else(|e| panic!("{}: {e} on {}", file.display(), String::from_utf8_lossy(&sql))) {
                Verdict::Tree(_) => accepted += 1,
                Verdict::Refused { sqlstate, .. } => *refused.entry(sqlstate).or_default() += 1,
            }
        }
    }
    println!("{} files, {statements} statements ({bytes} bytes): {accepted} accepted, refused by SQLSTATE {refused:?}; {not_utf8} not UTF-8, not sent", files.len());
    assert!(!files.is_empty() && accepted > 0, "the corpus ran no accepted statement");
}
