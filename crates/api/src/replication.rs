//! The host transports authenticated messages; Amphora owns replication and membership.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplicaConfig {
    pub node_id: u64,
    pub voters: Vec<u64>,
    pub learners: Vec<u64>,
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
}
