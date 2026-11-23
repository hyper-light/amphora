//! AC-PG.5.1: crates/pg-parser/src/gram_tables.rs records the sha256 of the gram.y it was generated
//! from (scripts/postgres-grammar.sh); it must be the vendored gram.y's, so a grammar that changes
//! under vendor/postgres fails here until the tables are regenerated.
#![allow(clippy::unwrap_used, clippy::disallowed_macros)]

use amphora_pg_parser::gram_tables::GRAM_Y_SHA256;
use sha2::{Digest, Sha256};

#[test]
fn ac_pg_5_1_the_tables_are_generated_from_the_vendored_gram_y() {
    let gram_y = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../vendor/postgres/src/backend/parser/gram.y"),
    )
    .unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(gram_y)),
        GRAM_Y_SHA256,
        "gram_tables.rs was generated from another gram.y: regenerate it with scripts/postgres-grammar.sh"
    );
}
