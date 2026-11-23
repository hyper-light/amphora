//! PostgreSQL 19's parser in Rust (docs/design/postgres.md section 3; section 11's units 4 to 6):
//! the tables flex and bison make from 19's own scan.l and gram.y, driven by flat loops with no
//! recursion, so it accepts what 19 accepts and refuses where 19 refuses.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::arithmetic_side_effects, clippy::disallowed_macros))]

pub mod scan;
#[rustfmt::skip]
pub mod scan_tables;
