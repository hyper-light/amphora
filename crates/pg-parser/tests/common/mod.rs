//! What the corpus tests share (T-PG.5.2).

/// Statements 19's grammar accepts and one of its actions then refuses (measured on 93a716b1, a
/// commit of branch perf/postgres-pg5 that main took in 9ea06947: 10 0A000, 4 22023, 21 42601;
/// the oracle gives the same count on c834f19b); lower it as actions are ported.
pub const REFUSED_IN_ACTIONS: usize = 35;
