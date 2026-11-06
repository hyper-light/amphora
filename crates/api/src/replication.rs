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
/// One change of a replicated group's members (replication.md, "Membership"). The group's leader
/// makes it through a joint configuration and answers once the group has left that configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemberChange {
    /// A new member. It is sent every entry and votes in nothing.
    AddLearner(u64),
    /// A learner made a voter.
    Promote(u64),
    /// A voter made a learner.
    Demote(u64),
    /// A member removed. Nothing more is sent to it.
    Remove(u64),
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
    /// The voters a joint configuration leaves (replication.md, "Membership"): empty unless the
    /// group is between two configurations.
    #[serde(default)]
    pub voters_outgoing: Vec<u64>,
}
