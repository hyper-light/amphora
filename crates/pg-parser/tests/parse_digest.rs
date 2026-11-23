//! T-PG.5.2 without the oracle: the grammar's verdict on every regression statement (accepted, or
//! refused with a SQLSTATE at a cursor) against 19's, as the oracle left it in crates/pg-oracle/corpus.digest.
//! 19 refuses some statements inside gram.y's actions (its `ereport(ERROR` calls), after the grammar
//! accepted them; no action is ported yet (section 11's unit 6 ports them), so those are counted
//! apart and may only fall.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros
)]

use std::collections::BTreeMap;

use amphora_pg_oracle::digest::{self, Answer};
use amphora_pg_parser::gram;

mod common;
use common::REFUSED_IN_ACTIONS;

/// T-PG.5.2: every statement 19's grammar accepts, the tables accept; every one it refuses, they
/// refuse with its SQLSTATE at its cursor.
#[test]
fn t_pg_5_2_grammar_against_the_corpus_digest() {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../pg-oracle/corpus.digest"),
    )
    .unwrap();
    let recorded = digest::read(&text).unwrap();
    let (mut total, mut matched, mut in_actions) = (0usize, 0usize, 0usize);
    let mut kinds: BTreeMap<String, (usize, String)> = BTreeMap::new();
    for (file, statements) in digest::corpus(&digest::corpus_dir()).unwrap() {
        let answers = recorded
            .get(&file)
            .unwrap_or_else(|| panic!("{file} is not in the digest"));
        assert_eq!(answers.len(), statements.len(), "{file}: statements split as the digest's");
        for (i, (sql, answer)) in statements.iter().zip(answers).enumerate() {
            let Some(answer) = answer else { continue };
            total += 1;
            let kind = match (&answer.tree, gram::accepts(sql)) {
                (Answer::Accepted(_), Ok(())) => None,
                (Answer::Refused { sqlstate, cursor }, Err(e))
                    if sqlstate == e.sqlstate && usize::try_from(*cursor).unwrap() == e.cursor =>
                {
                    None
                }
                (Answer::Accepted(_), Err(e)) => Some(format!("19 accepts, refused {}", e.sqlstate)),
                (Answer::Refused { sqlstate, .. }, Ok(())) => {
                    in_actions += 1;
                    println!("refused in an action: {file} #{i} {}", answer.tree);
                    Some(format!("19 refuses {sqlstate}, accepted"))
                }
                (Answer::Refused { sqlstate, .. }, Err(e)) => Some(format!("19 refuses {sqlstate}, refused {} or elsewhere", e.sqlstate)),
            };
            match kind {
                None => matched += 1,
                Some(kind) => {
                    let entry = kinds.entry(kind).or_insert((0, String::new()));
                    if entry.0 == 0 {
                        let cut: String = String::from_utf8_lossy(sql).chars().take(200).collect();
                        entry.1 = format!("{file} #{i} 19: {} sql: {cut:?}", answer.tree);
                    }
                    entry.0 += 1;
                }
            }
        }
    }
    println!("the grammar's verdict is 19's for {matched} of {total} statements");
    for (kind, (n, first)) in &kinds {
        println!("[{n}] {kind}: first {first}");
    }
    assert!(total > 0, "the digest names statements");
    assert_eq!(
        matched + in_actions,
        total,
        "every statement accepted or refused as 19 does, where 19 does, but those refused in actions"
    );
    assert!(
        in_actions <= REFUSED_IN_ACTIONS,
        "{in_actions} statements 19 refuses in an action are accepted, more than {REFUSED_IN_ACTIONS}"
    );
}
