//! PostgreSQL's wire protocol, 3.0 and 3.2, as PostgreSQL 19.0 speaks it (docs/design/postgres.md,
//! decision 2; unit 1 of its section 11). A pure core: bytes in, messages out, no socket, no clock
//! and no randomness but what the caller passes in (CLAUDE.md §9).
//!
//! - [`wire`]: frames read from borrowed bytes, each length bounded before the caller charges it,
//!   and backend messages written.
//! - [`scram`]: SCRAM-SHA-256 and SCRAM-SHA-256-PLUS on the server's side (RFC 5802, RFC 7677).
//! - [`session`]: one connection's state machine, from its first byte to Terminate.
//! - [`listener`]: the shell that drives sessions over hyper-rt's sockets (unit 2), each charged to
//!   the process's memory budget.
//!
//! PostgreSQL's source is cited as `path:line` at tag `REL_19_0`.

pub mod listener;
pub mod scram;
pub mod session;
pub mod wire;
