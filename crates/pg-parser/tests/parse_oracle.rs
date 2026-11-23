//! T-PG.5.2: the grammar against PostgreSQL 19's own `raw_parser` (the oracle) on every statement of
//! 19's regression SQL: the share accepted or refused as 19 does at 19's cursor, and the first
//! mismatch of each kind with 19's message. Needs the built oracle: AMPHORA_PG_ORACLE=<path to pg-oracle>.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use amphora_pg_oracle::{Oracle, Verdict, split};
use amphora_pg_parser::gram;

fn category(oracle: &Verdict, rust: &Result<(), gram::Refusal>) -> Option<String> {
    match (oracle, rust) {
        (Verdict::Tree(_), Ok(())) => None,
        (Verdict::Tree(_), Err(e)) => Some(format!("19 accepts, refused {}", e.sqlstate)),
        (Verdict::Refused { sqlstate, message, .. }, Ok(())) => {
            let head: String = message.chars().take(40).collect();
            Some(format!("19 refuses {sqlstate} ({head}), accepted"))
        }
        (Verdict::Refused { sqlstate, cursor, .. }, Err(e)) => {
            if sqlstate != e.sqlstate {
                Some(format!("19 refuses {sqlstate}, refused {}", e.sqlstate))
            } else if usize::try_from(*cursor).unwrap() != e.cursor {
                Some(format!("19 refuses {sqlstate}: cursor"))
            } else {
                None
            }
        }
    }
}

fn show(v: &Verdict) -> String {
    match v {
        Verdict::Tree(_) => "accepted".to_owned(),
        Verdict::Refused {
            sqlstate,
            cursor,
            message,
        } => format!("E {sqlstate} {cursor} {message}"),
    }
}

/// T-PG.5.2: every regression statement's verdict and cursor, the grammar's against 19's.
#[test]
#[ignore = "needs AMPHORA_PG_ORACLE, the built oracle"]
fn t_pg_5_2_grammar_matches_19_on_the_regression_corpus() {
    let program = PathBuf::from(std::env::var_os("AMPHORA_PG_ORACLE").expect("AMPHORA_PG_ORACLE"));
    let mut oracle = Oracle::spawn(&program).unwrap();
    let dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/postgres/src/test/regress/sql");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "sql"))
        .collect();
    files.sort();
    let (mut total, mut skipped, mut matched) = (0usize, 0usize, 0usize);
    let mut kinds: BTreeMap<String, (usize, String)> = BTreeMap::new();
    for file in &files {
        let script = std::fs::read(file).unwrap();
        for (i, sql) in split::statements(&script).iter().enumerate() {
            if std::str::from_utf8(sql).is_err() || sql.contains(&0) {
                skipped += 1;
                continue;
            }
            total += 1;
            let verdict = oracle.parse(sql).unwrap();
            let mine = gram::accepts(sql);
            match category(&verdict, &mine) {
                None => matched += 1,
                Some(kind) => {
                    let entry = kinds.entry(kind).or_insert((0, String::new()));
                    if entry.0 == 0 {
                        let cut: String = String::from_utf8_lossy(sql).chars().take(200).collect();
                        let rust = match &mine {
                            Ok(()) => "accepted".to_owned(),
                            Err(e) => format!("E {} {} {}", e.sqlstate, e.cursor, e.message),
                        };
                        entry.1 = format!(
                            "{} #{i}\n  sql: {cut:?}\n  19:   {}\n  rust: {rust}",
                            file.file_name().unwrap().to_string_lossy(),
                            show(&verdict)
                        );
                    }
                    entry.0 += 1;
                }
            }
        }
    }
    println!(
        "statements {total} (skipped {skipped} not UTF-8 or with NUL), verdict and cursor match {matched} of {total}"
    );
    let mut by_count: Vec<_> = kinds.into_iter().collect();
    by_count.sort_by_key(|k| std::cmp::Reverse(k.1.0));
    for (kind, (n, first)) in &by_count {
        println!("[{n}] {kind}: first {first}");
    }
    assert_eq!(matched, total, "every statement accepted or refused as 19 does, where 19 does");
}
