//! T-PG.4.3 without the oracle: the Rust scanner's `--tokens` body (or its refusal) for every
//! regression statement, against the hash 19's oracle left in crates/pg-oracle/corpus.digest.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::arithmetic_side_effects, clippy::disallowed_macros)]

use amphora_pg_oracle::digest::{self, Answer};
use amphora_pg_parser::lex;

/// T-PG.4.3: the share of corpus statements whose tokens hash as 19's do.
#[test]
fn t_pg_4_3_scanner_against_the_corpus_digest() {
    let text = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../pg-oracle/corpus.digest")).unwrap();
    let recorded = digest::read(&text).unwrap();
    let (mut total, mut matched) = (0usize, 0usize);
    for (file, statements) in digest::corpus(&digest::corpus_dir()).unwrap() {
        let answers = recorded.get(&file).unwrap_or_else(|| panic!("{file} is not in the digest"));
        assert_eq!(answers.len(), statements.len(), "{file}: statements split as the digest's");
        for (sql, answer) in statements.iter().zip(answers) {
            let Some(answer) = answer else { continue };
            total += 1;
            let mine = match lex::tokens(sql) {
                Ok(body) => Answer::Accepted(digest::fnv1a(&body)),
                Err(e) => Answer::Refused { sqlstate: e.sqlstate.to_owned(), cursor: u32::try_from(e.cursor).unwrap() },
            };
            if mine == answer.tokens {
                matched += 1;
            }
        }
    }
    println!("tokens match 19's digest for {matched} of {total} statements");
    assert!(total > 0, "the digest names statements");
}
