//! T-PG.4.3: the Rust scanner against PostgreSQL 19's own (the oracle's `--tokens`) on every
//! statement of 19's regression SQL: the share whose token stream matches byte for byte, and the
//! first mismatch of each kind. Needs the built oracle: AMPHORA_PG_ORACLE=<path to pg-oracle>.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::arithmetic_side_effects, clippy::disallowed_macros)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use amphora_pg_oracle::{Oracle, Verdict, split};
use amphora_pg_parser::lex;

fn first_token(line: Option<&str>) -> String {
    line.map_or_else(|| "end".to_owned(), |l| l.split(' ').next().unwrap_or("").to_owned())
}

fn category(oracle: &Verdict, rust: &Result<Vec<u8>, lex::ScanError>) -> Option<String> {
    match (oracle, rust) {
        (Verdict::Tree(body), Ok(mine)) => {
            if body.as_bytes() == mine.as_slice() {
                return None;
            }
            let mine = String::from_utf8_lossy(mine);
            let (mut a, mut b) = (body.lines(), mine.lines());
            loop {
                let (x, y) = (a.next(), b.next());
                if x != y {
                    let (tx, ty) = (first_token(x), first_token(y));
                    return Some(if tx == ty { format!("token {tx}: value or offset") } else { format!("token {tx} vs {ty}") });
                }
            }
        }
        (Verdict::Tree(_), Err(e)) => Some(format!("tokens vs error {}", e.sqlstate)),
        (Verdict::Refused { sqlstate, .. }, Ok(_)) => Some(format!("error {sqlstate} vs tokens")),
        (Verdict::Refused { sqlstate, cursor, .. }, Err(e)) => {
            if sqlstate != e.sqlstate {
                Some(format!("error {sqlstate} vs error {}", e.sqlstate))
            } else if usize::try_from(*cursor).unwrap() != e.cursor {
                Some(format!("error {sqlstate}: cursor"))
            } else {
                None
            }
        }
    }
}

fn show(v: &Verdict) -> String {
    match v {
        Verdict::Tree(b) => b.chars().take(600).collect(),
        Verdict::Refused { sqlstate, cursor, message } => format!("E {sqlstate} {cursor} {message}"),
    }
}

/// T-PG.4.3: every regression statement's tokens, the Rust scanner's against 19's.
#[test]
#[ignore = "needs AMPHORA_PG_ORACLE, the built oracle"]
fn t_pg_4_3_scanner_matches_19_on_the_regression_corpus() {
    let program = PathBuf::from(std::env::var_os("AMPHORA_PG_ORACLE").expect("AMPHORA_PG_ORACLE"));
    let mut oracle = Oracle::spawn_tokens(&program).unwrap();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/postgres/src/test/regress/sql");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x == "sql")).collect();
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
            let mine = lex::tokens(sql);
            match category(&verdict, &mine) {
                None => matched += 1,
                Some(kind) => {
                    let entry = kinds.entry(kind).or_insert((0, String::new()));
                    if entry.0 == 0 {
                        let cut: String = String::from_utf8_lossy(sql).chars().take(200).collect();
                        let rust = match &mine {
                            Ok(b) => String::from_utf8_lossy(b).chars().take(600).collect(),
                            Err(e) => format!("E {} {} {}", e.sqlstate, e.cursor, e.message),
                        };
                        entry.1 = format!("{} #{i}\n  sql: {cut:?}\n  19:   {:?}\n  rust: {rust:?}", file.file_name().unwrap().to_string_lossy(), show(&verdict));
                    }
                    entry.0 += 1;
                }
            }
        }
    }
    println!("statements {total} (skipped {skipped} not UTF-8 or with NUL), tokens match exactly {matched} of {total}");
    let mut by_count: Vec<_> = kinds.into_iter().collect();
    by_count.sort_by(|a, b| b.1.0.cmp(&a.1.0));
    for (kind, (n, first)) in &by_count {
        println!("[{n}] {kind}: first {first}");
    }
}
