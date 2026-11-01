//! The host transports authenticated messages; Amphora owns replication and membership.
use serde::{Deserialize, Serialize};

/// An explicit original database identity, shared by every initial group member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bootstrap {
    pub source_node: u64,
    pub source_id: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplicaConfig {
    pub node_id: u64,
    pub voters: Vec<u64>,
    pub learners: Vec<u64>,
    #[serde(default)]
    pub bootstrap: Option<Bootstrap>,
}

/// When a replicated database acknowledges a write (docs/design/replication.md, "Fast acknowledgement:
/// the replicated class").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Durability {
    /// Once a quorum's disks hold it and the proposer's own log is flushed.
    #[default]
    Persisted,
    /// Once members spanning `spanning` distinct fates hold it, in memory or on disk. Losing every
    /// member of one fate loses nothing acknowledged; losing every fate at once can lose what was
    /// acknowledged past the members' last durable promises.
    Replicated(Fates),
}

/// What each member fails with, and how many distinct fates a write's holders must span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fates {
    /// Each member and its fate: the operator's number for what it fails with, its host and its
    /// power feed. Every member states one.
    pub of: Vec<(u64, u64)>,
    /// The distinct fates a write's holders must span before it is acknowledged: at least 2.
    pub spanning: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outbound {
    pub to: u64,
    pub bytes: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplicaStatus {
    pub node_id: u64,
    pub leader: u64,
    pub term: u64,
    pub applied: u64,
    pub durable_commit: u64,
    pub voters: Vec<u64>,
    pub learners: Vec<u64>,
    pub fenced: Option<String>,
    /// Initial data is not exposed until its declared image is committed and applied.
    #[serde(default)]
    pub bootstrapping: bool,
}
