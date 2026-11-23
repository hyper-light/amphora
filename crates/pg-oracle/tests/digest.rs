//! The checked-in digest against the vendored corpus, with no oracle (crates/pg-oracle/src/digest.rs).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros
)]

use amphora_pg_oracle::digest;

/// T-PG.3.5: the digest holds an answer for every statement psql sends from 19's regression SQL, no
/// more and no fewer, and skips exactly the statements that aren't UTF-8; regenerate it with
/// scripts/postgres-digest.sh when this fails.
#[test]
fn t_pg_3_5_the_digest_answers_every_corpus_statement() {
    let text =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/corpus.digest")).unwrap();
    let digest = digest::read(&text).unwrap();
    let corpus = digest::corpus(&digest::corpus_dir()).unwrap();
    assert_eq!(
        digest.keys().collect::<Vec<_>>(),
        corpus.iter().map(|(f, _)| f).collect::<Vec<_>>(),
        "the digest's files are the corpus's"
    );
    let (mut sent, mut accepted) = (0usize, 0usize);
    for (file, statements) in &corpus {
        let answers = &digest[file];
        assert_eq!(answers.len(), statements.len(), "{file}: statements");
        for (i, (sql, answer)) in statements.iter().zip(answers).enumerate() {
            assert_eq!(
                answer.is_some(),
                std::str::from_utf8(sql).is_ok(),
                "{file} statement {i}: answered iff UTF-8"
            );
            sent += usize::from(answer.is_some());
            accepted += usize::from(matches!(
                answer,
                Some(digest::Statement {
                    tree: digest::Answer::Accepted(_),
                    ..
                })
            ));
        }
    }
    assert!(sent > 0 && accepted > 0, "the digest answers no statement");
}
