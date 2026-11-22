//! Embedded owner protocol. Variant order is append-only.
use amphora_api::{
    Changes, Consistency, Migration, Page, Refusal, Rev, Row, Scan, SqlResult, Statement, Tx, Value,
};
use serde::{Deserialize, Serialize};

pub(super) const FRAME_MAX: usize = amphora_api::RESULT_MAX + (1 << 20);
pub(super) const COMMAND_MAX: usize = 4096;
pub(super) const WAIT_MAX: usize = 8;
pub(super) const WATCH_BATCH_MAX: usize = 64;
pub(super) const WATCH_REPLY_MAX: usize = 64 << 10;
pub(super) const SESSION_MAX: std::time::Duration = std::time::Duration::from_secs(30);
/// How long each side gives the other to authenticate: admission policy, not a measured tuning
/// claim (local-owner-transport.md, its resource limits).
pub(super) const GREETING_MAX: std::time::Duration = std::time::Duration::from_secs(2);
pub(super) const CAP_BOUNDED_COMPACTION: u64 = 1;
/// The bound on a greeting's frames: the 256 bytes they have always had, past the largest one, a
/// Hello and its Speaks (at most 52 bytes as postcard's varints encode them).
pub(super) const GREETING_FRAME_MAX: usize = 256;
/// The newest protocol this side speaks. Postcard is not self-describing, so a field added to any
/// message changes it (local-owner-transport.md, "Versions"). 3: ReplicaStatus gained voters_outgoing; 4: the owner's Identity gained durability.
/// 5: Request gained Application, the client's claim before its ordinary request. 6: Request gained
/// LocalSession, a local read. 7: Request gained Pull and Answer Pulled, a node's half of a copy
/// or catch-up batch for another node's new group (CP-T1.9; reshard.md, "Moves across nodes"). 8: Request gained
/// Step and Answer Stepped, one step of a transaction across groups (CP-T1.10; xgroup.md, "Over the transport"). 9: Answer
/// gained SteppedClosed, a step's answer with the group's closed time after it (AC-T1.10.1, T-T1.10.W4; xgroup.md,
/// "Reading its own writes across groups"), and the greeting negotiates the version both ends use
/// (local-owner-transport.md, "Negotiated versions").
pub(super) const VERSION: u32 = 9;
/// The oldest protocol this side still speaks: 8, the version before negotiation, whose owners
/// take only a greeting of exactly 8 (local-owner-transport.md, "Negotiated versions"). A format
/// value: every message of version 8 keeps its shape here, and nothing newer is sent at 8.
pub(super) const VERSION_MIN: u32 = 8;
/// The first version whose peers take [`Answer::SteppedClosed`]; an owner answers a step at an
/// older agreed version with [`Answer::Stepped`].
pub(super) const VERSION_STEPPED_CLOSED: u32 = 9;

/// A client's greeting. Since version 9 `version` is the oldest version the client speaks, and
/// [`Speaks`] follows it in the same frame; a version-8 client sends 8 and nothing after it.
#[derive(Serialize, Deserialize)]
pub(super) struct Hello {
    pub version: u32,
    pub nonce: [u8; 32],
    pub millis: u64,
}
/// Follows a [`Hello`] in its frame since version 9: the newest version the client speaks. A
/// version-8 owner decodes the Hello alone and leaves this unread (postcard decodes a frame's
/// leading value and ignores what follows it).
#[derive(Serialize, Deserialize)]
pub(super) struct Speaks {
    pub newest: u32,
}
#[derive(Serialize, Deserialize)]
pub(super) struct Challenge {
    pub nonce: [u8; 32],
    pub proof: [u8; 32],
}
/// Follows a [`Challenge`] in its frame since version 9: the version the owner chose, the newest
/// both ends speak. A version-8 client leaves it unread; a client that finds none greeted a
/// version-8 owner, which took the greeting only because its oldest version is 8.
#[derive(Serialize, Deserialize)]
pub(super) struct Agreed {
    pub version: u32,
}
#[derive(Serialize, Deserialize)]
pub(super) enum Request {
    Session,
    Execute(#[serde(deserialize_with = "super::bounded::statements")] Vec<Statement>),
    Migrate(Migration),
    Records {
        namespace: String,
        op: Record,
    },
    Probe,
    Identity,
    RestoreProgress,
    WatchBatch(
        #[serde(deserialize_with = "super::bounded::watch_requests")] Vec<(String, String, Rev)>,
    ),
    ReplicaStatus,
    Capabilities,
    // Appended: one bounded claim before the existing ordinary request. It
    // declares a client format; only the actual owner's floor grants admission.
    Application(crate::database::Application),
    // Appended: a session read-only on the owner's own state, asking no leader (replication.md,
    // "A local read on a replica").
    LocalSession,
    // Appended: a node's pull from one of its groups, for a move's new group on another node
    // (CP-T1.9; reshard.md, "Moves across nodes"), answered by a node endpoint (`Node`).
    Pull(crate::groups::Pull),
    // Appended: one step of a transaction across groups on this owner's group `here`, with the
    // newest commit stamp the coordinator heard of (CP-T1.10, T-T1.12.X2; xgroup.md, "Over the
    // transport"). Its sizes are held by the frame's bound, as every request's are.
    Step {
        here: crate::xgroup::GroupId,
        carried: u64,
        envelope: crate::xgroup::Envelope,
    },
}
#[derive(Serialize, Deserialize)]
pub(super) enum Record {
    Get(String, Vec<u8>, Consistency),
    Scan(String, Scan),
    Commit(#[serde(deserialize_with = "super::bounded::transaction")] Tx),
    Changes(String, Rev, usize),
    Wait(String, Rev),
    Head,
    Compact(Rev),
    TryWait(String, Rev),
    ImportPage(
        #[serde(deserialize_with = "super::bounded::import_page")] amphora_api::import::ImportPage,
    ),
    ImportState,
    CompactStep(Rev),
}
#[derive(Serialize, Deserialize)]
pub(super) enum Command {
    Batch(String),
    Query(
        String,
        #[serde(deserialize_with = "super::bounded::params")] Vec<Value>,
        bool,
    ),
    Columns(String),
    Commit,
    Abort,
    RecordsGet(String, String, Vec<u8>),
    RecordsCommit(
        String,
        #[serde(deserialize_with = "super::bounded::transaction")] Tx,
    ),
}
#[derive(Serialize, Deserialize)]
pub(super) enum Answer {
    Ready,
    Rows {
        #[serde(deserialize_with = "super::bounded::rows")]
        rows: Vec<Vec<Value>>,
        changes: i64,
        rowid: i64,
    },
    Columns(usize),
    Done,
    Sql(#[serde(deserialize_with = "super::bounded::sql_results")] Vec<SqlResult>),
    Migrated(bool),
    Row(Option<Row>),
    Page(#[serde(deserialize_with = "super::bounded::page")] Page),
    Rev(Rev),
    Changes(#[serde(deserialize_with = "super::bounded::changes")] Changes),
    OptionalRev(Option<Rev>),
    ImportState(Option<amphora_api::import::ImportProgress>),
    SessionRow {
        row: Option<Row>,
        changes: i64,
        rowid: i64,
    },
    SessionRev {
        rev: Rev,
        changes: i64,
        rowid: i64,
    },
    Identity(crate::database::remote::Identity),
    WatchBatch(
        #[serde(deserialize_with = "super::bounded::watch_answers")]
        Vec<Result<Option<Rev>, Refusal>>,
    ),
    ReplicaStatus(Option<amphora_api::replication::ReplicaStatus>),
    // Untyped SQL failures retain caller context; typed refusals use Response::Err.
    SqlError(String),
    Capabilities(u64),
    Compaction(amphora_api::CompactionProgress),
    // Appended: a Pull's answer.
    Pulled(crate::groups::Pulled),
    // Appended: a Step's answer, with the group's newest commit stamp after it (none: the group
    // makes no stamps). Its shape is version 8's, kept for a peer that agreed on 8.
    Stepped {
        reply: crate::xgroup::Reply,
        latest: Option<u64>,
    },
    // Appended (version 9): a Step's answer to a peer that agreed on 9 or later, Stepped with the
    // group's closed time after the step, the microseconds of its session token (none: the group
    // keeps no closed time, its reads see every write).
    SteppedClosed {
        reply: crate::xgroup::Reply,
        latest: Option<u64>,
        closed: Option<u64>,
    },
}
pub(super) type Response = Result<Answer, Refusal>;
