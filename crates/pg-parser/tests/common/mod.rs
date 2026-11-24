//! What the corpus tests share (T-PG.5.2).

/// Statements 19's grammar accepts and one of its actions then refuses, that the tables accept
/// (measured on 7dc1f992, adacachy2: 10 0A000, 3 22023, 20 42601; 34 before PartitionSpec's action
/// refused create_table.sql #83's PARTITION BY MAGIC, 35 before CopyStmt's refused copy2.sql #62's
/// COPY TO with WHERE); lower it as actions are ported.
pub const REFUSED_IN_ACTIONS: usize = 33;

/// Regression statements whose every rule has a ported action and whose tree is 19's (FNV-1a 64 of
/// the oracle's text, corpus.digest): 6748 measured on 7dc1f992 (adacachy2, cpus=1) with PARTITION
/// BY ported (6686 with pgbench -i's DDL, COPY, commit, and psql's SET and SHOW, 5397 with every
/// pgbench DML statement, 5169 before its partition query, 1074 with TPC-B, 857 with its UPDATE, 738
/// with BEGIN and END only); raise it as actions are ported, never lower it.
pub const TREES_EQUAL: usize = 6748;
