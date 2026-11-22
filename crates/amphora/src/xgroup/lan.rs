//! The measurement that chooses G13 step 3's protocol (CP-T1.10: AC-T1.10.2, AC-T1.10.5; xgroup.md,
//! "Over the transport"): transactions across groups run through one protocol between groups on
//! separate nodes, each step one request over the transport ([`Member::Node`]). [`measure`] times
//! their commits; [`elle`] runs a seeded history and checks it Elle-style. Sizes, durations, seeds
//! and the nodes' endpoints are the caller's; nothing here is tuned.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use amphora_api::Refusal;

use super::GroupId;
use super::coordinator::{Answer, Protocol, Txn};
use super::sql::{Cluster, Member, Route};

/// A harness key: a prefix of 8-byte numbers (a run's coordinator, and a seed's number, so runs on
/// the same groups never share a key: a seed's values are unique only within its run), then a
/// number whose remainder by the group count is the key's group ([`route`]).
fn key(prefix: &[u64], n: u64) -> Vec<u8> {
    let mut k: Vec<u8> = prefix.iter().flat_map(|p| p.to_be_bytes()).collect();
    k.extend_from_slice(&n.to_be_bytes());
    k
}

/// The route harness keys take: their last eight bytes' remainder by `groups`.
#[must_use]
pub fn route(groups: GroupId) -> Route {
    Arc::new(move |k: &[u8]| {
        let tail = k
            .len()
            .checked_sub(8)
            .and_then(|i| k.get(i..))
            .and_then(|t| <[u8; 8]>::try_from(t).ok())
            .map_or(0, u64::from_be_bytes);
        let group = tail.checked_rem(u64::from(groups)).unwrap_or(0);
        GroupId::try_from(group).unwrap_or(0)
    })
}

/// SplitMix64 (Steele, Lea and Flood, OOPSLA 2014): the workload's choices from a seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform enough below `n` for a workload's choices (none: 0).
    fn below(&mut self, n: u64) -> u64 {
        self.next().checked_rem(n).unwrap_or(0)
    }
}

/// Bits below a value's top bit that pick its bucket ([`Latencies`]): a format value, giving buckets
/// no wider than 1/128 of the values in them, so a percentile read from them is high by under 1%.
const SUB_BITS: u32 = 7;
const SUB: u64 = 1 << SUB_BITS;
/// Every bucket a u64 of nanoseconds can fall in: the exact ones below [`SUB`], then [`SUB`] per
/// power of two above.
const BUCKETS: usize = (64 - SUB_BITS as usize + 1) * (1usize << SUB_BITS);

/// Commit latencies in log-linear buckets as HdrHistogram keeps them (Tene, HdrHistogram): fixed
/// memory however long the run, each percentile read as its bucket's upper end.
#[derive(Debug, Clone)]
pub struct Latencies {
    counts: Vec<u64>,
    total: u64,
}

impl Default for Latencies {
    fn default() -> Self {
        Self {
            counts: vec![0; BUCKETS],
            total: 0,
        }
    }
}

fn bucket(nanos: u64) -> usize {
    let index = if nanos < SUB {
        nanos
    } else {
        let top = 63u32.saturating_sub(nanos.leading_zeros());
        let shift = top.saturating_sub(SUB_BITS);
        let sub = (nanos >> shift).saturating_sub(SUB);
        u64::from(shift)
            .saturating_add(1)
            .saturating_mul(SUB)
            .saturating_add(sub)
    };
    usize::try_from(index).unwrap_or(usize::MAX)
}

fn upper(index: usize) -> u64 {
    let index = u64::try_from(index).unwrap_or(u64::MAX);
    if index < SUB {
        return index;
    }
    let shift = u32::try_from((index / SUB).saturating_sub(1)).unwrap_or(u32::MAX);
    let sub = index % SUB;
    SUB.saturating_add(sub)
        .saturating_add(1)
        .checked_shl(shift)
        .map_or(u64::MAX, |v| v.saturating_sub(1))
}

impl Latencies {
    pub fn record(&mut self, took: Duration) {
        let nanos = u64::try_from(took.as_nanos()).unwrap_or(u64::MAX);
        if let Some(count) = self.counts.get_mut(bucket(nanos)) {
            *count = count.saturating_add(1);
            self.total = self.total.saturating_add(1);
        }
    }

    pub fn merge(&mut self, other: &Latencies) {
        for (mine, theirs) in self.counts.iter_mut().zip(&other.counts) {
            *mine = mine.saturating_add(*theirs);
        }
        self.total = self.total.saturating_add(other.total);
    }

    #[must_use]
    pub fn count(&self) -> u64 {
        self.total
    }

    /// The `per_mille`th quantile by nearest rank (500: p50; 990: p99; 999: p99.9); none recorded: 0.
    #[must_use]
    pub fn quantile(&self, per_mille: u64) -> Duration {
        let rank = self.total.saturating_mul(per_mille).div_ceil(1000).max(1);
        let mut seen = 0u64;
        for (index, count) in self.counts.iter().enumerate() {
            seen = seen.saturating_add(*count);
            if *count > 0 && seen >= rank {
                return Duration::from_nanos(upper(index));
            }
        }
        Duration::ZERO
    }
}

/// What [`measure`] runs: `clients` callers each running one transaction after another for
/// `duration`, each transaction reading and writing one key in each of `span` groups (at least two,
/// at most the groups), its keys among `keys` per group, so `keys` sets how often they conflict.
#[derive(Debug, Clone, Copy)]
pub struct Load {
    pub clients: usize,
    pub keys: u64,
    pub span: usize,
    pub duration: Duration,
}

/// One protocol's measurement: what its transactions came to and their commit latencies.
#[derive(Debug, Clone)]
pub struct Measured {
    pub protocol: Protocol,
    pub groups: usize,
    pub load: Load,
    pub committed: u64,
    pub aborted: u64,
    pub unknown: u64,
    pub elapsed: Duration,
    pub latencies: Latencies,
}

impl Measured {
    /// Committed transactions a second.
    #[must_use]
    pub fn throughput(&self) -> f64 {
        let secs = self.elapsed.as_secs_f64();
        if secs > 0.0 {
            self.committed as f64 / secs
        } else {
            0.0
        }
    }

    /// The measurement as one line of bench/records' JSON Lines.
    #[must_use]
    pub fn record(&self, host: &str) -> String {
        let micros = |d: Duration| d.as_secs_f64() * 1e6;
        format!(
            "{{\"bench\":\"xgroup_lan\",\"host\":{host:?},\"protocol\":\"{}\",\"groups\":{},\"clients\":{},\"keys_per_group\":{},\"span\":{},\"secs\":{:.3},\"committed\":{},\"aborted\":{},\"unknown\":{},\"commits_per_sec\":{:.1},\"p50_us\":{:.1},\"p99_us\":{:.1},\"p999_us\":{:.1}}}",
            protocol_name(self.protocol),
            self.groups,
            self.load.clients,
            self.load.keys,
            self.load.span,
            self.elapsed.as_secs_f64(),
            self.committed,
            self.aborted,
            self.unknown,
            self.throughput(),
            micros(self.latencies.quantile(500)),
            micros(self.latencies.quantile(990)),
            micros(self.latencies.quantile(999)),
        )
    }
}

/// The protocol's name in records and on the command line.
#[must_use]
pub fn protocol_name(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::TwoPhase => "two-phase",
        Protocol::Parallel => "parallel",
    }
}

fn group_count(members: &[Member]) -> Result<GroupId, Refusal> {
    GroupId::try_from(members.len())
        .map_err(|_| amphora_api::invalid("cross-group harness", "too many groups"))
}

/// Runs `load` through coordinator `id` running `protocol` over `members` (group `i` is
/// `members[i]`, group 0 the registry), everything by `until`.
pub fn measure(
    members: Vec<Member>,
    id: u64,
    protocol: Protocol,
    load: Load,
    until: Instant,
) -> Result<Measured, Refusal> {
    let groups = group_count(&members)?;
    let count = members.len();
    if load.span < 2 || load.span > count || load.keys == 0 || load.clients == 0 {
        return Err(amphora_api::invalid(
            "cross-group harness",
            "a load needs a span of two groups to the groups there are, a key per group and a client",
        ));
    }
    let cluster = Cluster::open_members(members, route(groups), id, protocol, load.clients, until)?;
    let start = Instant::now();
    let end = start.checked_add(load.duration).unwrap_or(until).min(until);
    let width = u64::from(groups);
    let results: Vec<Result<(Latencies, u64, u64), Refusal>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..load.clients)
            .map(|client| {
                let cluster = &cluster;
                scope.spawn(move || {
                    let mut rng = Rng(id ^ u64::try_from(client).unwrap_or(0).rotate_left(32));
                    let mut latencies = Latencies::default();
                    let (mut aborted, mut unknown) = (0u64, 0u64);
                    let mut written = 0u64;
                    while Instant::now() < end {
                        let first = rng.below(width);
                        let mut txn = Txn::default();
                        for i in 0..u64::try_from(load.span).unwrap_or(0) {
                            let group = first.saturating_add(i).checked_rem(width).unwrap_or(0);
                            let n = rng
                                .below(load.keys)
                                .saturating_mul(width)
                                .saturating_add(group);
                            let k = key(&[id], n);
                            written = written.wrapping_add(1);
                            txn.reads.push(k.clone());
                            txn.writes.push((k, Some(written.to_be_bytes().to_vec())));
                        }
                        let began = Instant::now();
                        match cluster.transact(txn, until) {
                            Ok(Answer::Committed { .. }) => latencies.record(began.elapsed()),
                            Ok(Answer::Aborted(_)) => aborted = aborted.saturating_add(1),
                            Ok(Answer::Unknown)
                            | Err(Refusal::UnknownOutcome | Refusal::Timeout) => {
                                unknown = unknown.saturating_add(1)
                            }
                            Err(refusal) => return Err(refusal),
                        }
                    }
                    Ok((latencies, aborted, unknown))
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join().unwrap_or_else(|_| {
                    Err(Refusal::Unavailable(
                        "a harness client's thread panicked".into(),
                    ))
                })
            })
            .collect()
    });
    let elapsed = start.elapsed();
    let mut measured = Measured {
        protocol,
        groups: count,
        load,
        committed: 0,
        aborted: 0,
        unknown: 0,
        elapsed,
        latencies: Latencies::default(),
    };
    for result in results {
        let (latencies, aborted, unknown) = result?;
        measured.latencies.merge(&latencies);
        measured.aborted = measured.aborted.saturating_add(aborted);
        measured.unknown = measured.unknown.saturating_add(unknown);
    }
    measured.committed = measured.latencies.count();
    Ok(measured)
}

/// What [`elle`] runs for a seed: `ops` transactions over up to `clients` callers at once and up to
/// `keys` keys per group, the seed choosing within them (callers, keys, each transaction's keys
/// and which of them it writes). The seed fixes the workload; the interleaving is the network's.
#[derive(Debug, Clone, Copy)]
pub struct Shape {
    pub ops: usize,
    pub clients: usize,
    pub keys: u64,
}

/// What became of one transaction in a history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Heard {
    Committed,
    Aborted,
    Unknown,
}

/// One transaction of a history: when it began and was answered, what it wrote (each a unique
/// value read-modify-written), and what it read if it committed.
#[derive(Debug, Clone)]
struct Op {
    invoke: Instant,
    done: Option<Instant>,
    heard: Heard,
    writes: BTreeMap<Vec<u8>, Vec<u8>>,
    reads: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
}

/// A seed's run: its transactions' outcomes and the anomalies the check found.
#[derive(Debug, Clone, Default)]
pub struct Checked {
    pub committed: usize,
    pub aborted: usize,
    /// Answered neither way: a seed with one isn't a clean seed, and the check says why it can't place it.
    pub unknown: usize,
    /// Committed across more than one group.
    pub crossed: usize,
    pub anomalies: Vec<String>,
}

/// Runs seed `seed`'s history through coordinator `id` running `protocol` over `members`, then
/// reads every key it touched and checks the history Elle-style (Kingsbury and Alvaro, PVLDB 14(3),
/// as the simulator's check does, `xgroup::tests`): every write is a read-modify-write of a unique
/// value, so each key's versions form a chain from what transactions read; a lost update, a read of
/// an aborted or unwritten value, a commit missing from its chain or the final values, or a cycle in
/// the ww, wr, rw and real-time edges is an anomaly.
pub fn elle(
    members: Vec<Member>,
    id: u64,
    protocol: Protocol,
    seed: u64,
    shape: Shape,
    until: Instant,
) -> Result<Checked, Refusal> {
    let groups = group_count(&members)?;
    if shape.ops == 0 || shape.clients == 0 || shape.keys == 0 || groups == 0 {
        return Err(amphora_api::invalid(
            "cross-group harness",
            "a seed's shape needs a transaction, a client and a key per group",
        ));
    }
    let width = u64::from(groups);
    let mut rng = Rng(seed);
    let clients = usize::try_from(rng.below(u64::try_from(shape.clients).unwrap_or(1)))
        .unwrap_or(0)
        .saturating_add(1);
    let keys = rng
        .below(shape.keys)
        .saturating_add(1)
        .saturating_mul(width);
    // The seed's keys: its coordinator's and its own prefix, so runs on the same groups never share one.
    let prefix = [id, seed];
    let route_of = route(groups);
    let cluster =
        Cluster::open_members(members, Arc::clone(&route_of), id, protocol, clients, until)?;
    let next = AtomicUsize::new(0);
    let unique = AtomicU64::new(0);
    let results: Vec<Result<Vec<Op>, Refusal>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..clients)
            .map(|client| {
                let (cluster, next, unique) = (&cluster, &next, &unique);
                scope.spawn(move || {
                    let mut rng = Rng(seed
                        ^ u64::try_from(client)
                            .unwrap_or(0)
                            .wrapping_add(1)
                            .rotate_left(40));
                    let mut ops = Vec::new();
                    while next.fetch_add(1, Ordering::AcqRel) < shape.ops {
                        let size = rng.below(width.saturating_add(1)).saturating_add(1);
                        let picked: BTreeSet<u64> = (0..size).map(|_| rng.below(keys)).collect();
                        let mut txn = Txn::default();
                        let mut writes = BTreeMap::new();
                        for n in picked {
                            let k = key(&prefix, n);
                            txn.reads.push(k.clone());
                            // Two in three of its keys written, as the simulator's runs write them.
                            if rng.below(3) != 0 {
                                let value =
                                    unique.fetch_add(1, Ordering::AcqRel).to_be_bytes().to_vec();
                                txn.writes.push((k.clone(), Some(value.clone())));
                                writes.insert(k, value);
                            }
                        }
                        let invoke = Instant::now();
                        let (heard, reads) = match cluster.transact(txn, until) {
                            Ok(Answer::Committed { reads }) => {
                                (Heard::Committed, reads.into_iter().collect())
                            }
                            Ok(Answer::Aborted(_)) => (Heard::Aborted, BTreeMap::new()),
                            Ok(Answer::Unknown)
                            | Err(Refusal::UnknownOutcome | Refusal::Timeout) => {
                                (Heard::Unknown, BTreeMap::new())
                            }
                            Err(refusal) => return Err(refusal),
                        };
                        let done = (heard != Heard::Unknown).then(Instant::now);
                        ops.push(Op {
                            invoke,
                            done,
                            heard,
                            writes,
                            reads,
                        });
                    }
                    Ok(ops)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join().unwrap_or_else(|_| {
                    Err(Refusal::Unavailable(
                        "a harness client's thread panicked".into(),
                    ))
                })
            })
            .collect()
    });
    let mut ops = Vec::new();
    for result in results {
        ops.extend(result?);
    }
    // The final values: one read of every key, again until it commits (a lock a resolution still
    // holds aborts it) or the run's time is up.
    let all: Vec<Vec<u8>> = (0..keys).map(|n| key(&prefix, n)).collect();
    let finals = loop {
        let txn = Txn {
            reads: all.clone(),
            writes: Vec::new(),
        };
        match cluster.transact(txn, until)? {
            Answer::Committed { reads } => break reads.into_iter().collect::<BTreeMap<_, _>>(),
            Answer::Aborted(_) | Answer::Unknown if Instant::now() < until => {
                std::thread::yield_now()
            }
            Answer::Aborted(_) | Answer::Unknown => return Err(Refusal::Timeout),
        }
    };
    let mut checked = Checked {
        anomalies: check(&ops, &finals),
        ..Checked::default()
    };
    for op in &ops {
        match op.heard {
            Heard::Committed => {
                checked.committed = checked.committed.saturating_add(1);
                let touched: BTreeSet<GroupId> = op.writes.keys().map(|k| route_of(k)).collect();
                if touched.len() > 1 {
                    checked.crossed = checked.crossed.saturating_add(1);
                }
            }
            Heard::Aborted => checked.aborted = checked.aborted.saturating_add(1),
            Heard::Unknown => checked.unknown = checked.unknown.saturating_add(1),
        }
    }
    Ok(checked)
}

/// The Elle-style check of a history over the transport: what the client heard decides each
/// transaction (an unknown one is placed nowhere, and named if anything of it was read).
fn check(ops: &[Op], finals: &BTreeMap<Vec<u8>, Option<Vec<u8>>>) -> Vec<String> {
    let mut anomalies = Vec::new();
    let mut writer: BTreeMap<&[u8], usize> = BTreeMap::new();
    for (i, op) in ops.iter().enumerate() {
        for value in op.writes.values() {
            writer.insert(value, i);
        }
    }
    let heard = |i: usize| ops.get(i).map_or(Heard::Aborted, |op| op.heard);
    let committed: Vec<usize> = (0..ops.len())
        .filter(|i| heard(*i) == Heard::Committed)
        .collect();
    let observed = committed
        .iter()
        .filter_map(|i| ops.get(*i))
        .flat_map(|op| op.reads.values())
        .chain(finals.values())
        .flatten();
    for value in observed {
        match writer.get(value.as_slice()).map(|w| (*w, heard(*w))) {
            None => anomalies.push(format!("a read of {value:?}, which nobody wrote")),
            Some((w, Heard::Aborted)) => {
                anomalies.push(format!("G1a: a read of {value:?}, written by aborted #{w}"))
            }
            Some((w, Heard::Unknown)) => anomalies.push(format!(
                "a read of {value:?}, written by #{w}, whose outcome is unknown"
            )),
            Some((_, Heard::Committed)) => {}
        }
    }
    // Each version's successor, per key.
    let mut next: BTreeMap<(&[u8], Option<&[u8]>), usize> = BTreeMap::new();
    for i in &committed {
        let Some(op) = ops.get(*i) else { continue };
        for key in op.writes.keys() {
            let Some(prev) = op.reads.get(key) else {
                anomalies.push(format!("#{i} wrote {key:?} without reading it"));
                continue;
            };
            if let Some(other) = next.insert((key.as_slice(), prev.as_deref()), *i) {
                anomalies.push(format!(
                    "lost update: #{other} and #{i} both wrote {key:?} after {prev:?}"
                ));
            }
        }
    }
    let mut edges: BTreeSet<(usize, usize)> = BTreeSet::new();
    let keys: BTreeSet<&[u8]> = ops
        .iter()
        .flat_map(|op| op.writes.keys().map(Vec::as_slice))
        .collect();
    for key in keys {
        let mut version: Option<&[u8]> = None;
        let mut chain: Vec<usize> = Vec::new();
        while let Some(w) = next.get(&(key, version)).copied() {
            for r in &committed {
                let read = ops.get(*r).and_then(|op| op.reads.get(key));
                if *r != w && read.is_some_and(|v| v.as_deref() == version) {
                    edges.insert((*r, w)); // rw
                }
            }
            if let Some(prev) = chain.last() {
                edges.insert((*prev, w)); // ww
            }
            chain.push(w);
            version = ops
                .get(w)
                .and_then(|op| op.writes.get(key))
                .map(Vec::as_slice);
            if chain.len() > ops.len() {
                anomalies.push(format!("{key:?}'s versions loop"));
                break;
            }
        }
        let writes = committed
            .iter()
            .filter(|i| ops.get(**i).is_some_and(|op| op.writes.contains_key(key)))
            .count();
        if chain.len() != writes {
            anomalies.push(format!(
                "{key:?}: {writes} committed writes, {} in its chain",
                chain.len()
            ));
        }
        let last = finals.get(key).and_then(Option::as_deref);
        if last != version {
            anomalies.push(format!("{key:?} ends {last:?}, its chain {version:?}"));
        }
    }
    for r in &committed {
        let Some(op) = ops.get(*r) else { continue };
        for value in op.reads.values().flatten() {
            if let Some(w) = writer.get(value.as_slice()).filter(|w| **w != *r) {
                edges.insert((*w, *r)); // wr
            }
        }
    }
    // Real time: a transaction answered before another began precedes it.
    for a in &committed {
        let Some(done) = ops.get(*a).and_then(|op| op.done) else {
            continue;
        };
        for b in &committed {
            if ops.get(*b).is_some_and(|op| done < op.invoke) {
                edges.insert((*a, *b));
            }
        }
    }
    // A cycle, found without recursion: Kahn's algorithm leaves the nodes on cycles.
    let mut indegree: BTreeMap<usize, usize> = committed.iter().map(|i| (*i, 0)).collect();
    for (_, to) in &edges {
        if let Some(d) = indegree.get_mut(to) {
            *d = d.saturating_add(1);
        }
    }
    let mut free: Vec<usize> = indegree
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(i, _)| *i)
        .collect();
    let mut left = indegree.len();
    while let Some(i) = free.pop() {
        left = left.saturating_sub(1);
        for (_, to) in edges.range((i, 0)..).take_while(|(from, _)| *from == i) {
            if let Some(d) = indegree.get_mut(to) {
                *d = d.saturating_sub(1);
                if *d == 0 {
                    free.push(*to);
                }
            }
        }
    }
    if left != 0 {
        anomalies.push(format!("a dependency cycle among {left} transactions"));
    }
    anomalies
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use crate::reshard::NodeId;
    use crate::transport::Nodes;

    /// Two groups, each its own database whose owner listens at its own endpoint; the coordinator
    /// reaches both only over the transport.
    fn two_endpoints(dir: &std::path::Path, until: Instant) -> (Vec<Database>, Vec<Member>) {
        let paths: Vec<_> = (0..2).map(|g| dir.join(format!("group{g}.db"))).collect();
        let databases: Vec<Database> = paths
            .iter()
            .map(|p| Database::open(p, until).unwrap())
            .collect();
        for database in &databases {
            super::super::sql::prepare(database, until).unwrap();
        }
        let nodes = Arc::new(Nodes::new(
            paths
                .iter()
                .enumerate()
                .map(|(g, p)| (NodeId(u64::try_from(g).unwrap()), p.clone())),
        ));
        let members = (0..2)
            .map(|g| Member::Node {
                nodes: Arc::clone(&nodes),
                node: NodeId(g),
            })
            .collect();
        (databases, members)
    }

    fn until() -> Instant {
        Instant::now() + crate::transport::HANG
    }

    /// T-T1.9.R18: the harness over two groups' network endpoints, each its database owner's
    /// listener at a network address under mutual TLS, the coordinator dialing both.
    #[test]
    fn t_t1_9_r18_the_lan_harness_runs_over_network_endpoints() {
        let dir = tempfile::tempdir().unwrap();
        let pki = crate::transport::network::Pki::new();
        let databases: Vec<Database> = (0..2)
            .map(|g| Database::open(&dir.path().join(format!("group{g}.db")), until()).unwrap())
            .collect();
        for database in &databases {
            super::super::sql::prepare(database, until()).unwrap();
        }
        let listening: Vec<_> = databases
            .iter()
            .enumerate()
            .map(|(g, d)| {
                d.listen(
                    "127.0.0.1:0".parse().unwrap(),
                    &pki.credentials(&format!("node-{g}")),
                )
                .unwrap()
            })
            .collect();
        let nodes = Arc::new(
            Nodes::network(
                listening.iter().enumerate().map(|(g, l)| {
                    (
                        NodeId(u64::try_from(g).unwrap()),
                        l.address().unwrap(),
                        format!("node-{g}"),
                    )
                }),
                &pki.credentials("coordinator"),
            )
            .unwrap(),
        );
        let members: Vec<Member> = (0..2)
            .map(|g| Member::Node {
                nodes: Arc::clone(&nodes),
                node: NodeId(g),
            })
            .collect();
        let load = Load {
            clients: 2,
            keys: 8,
            span: 2,
            duration: Duration::from_millis(200),
        };
        let measured = measure(members, 1, Protocol::Parallel, load, until()).unwrap();
        assert!(measured.committed > 0, "{measured:?}");
        assert_eq!(measured.unknown, 0);
    }

    #[test]
    fn ac_t1_10_2_the_lan_harness_measures_both_protocols_over_two_endpoints() {
        let dir = tempfile::tempdir().unwrap();
        let (_databases, members) = two_endpoints(dir.path(), until());
        for (id, protocol) in [(1, Protocol::TwoPhase), (2, Protocol::Parallel)] {
            let load = Load {
                clients: 2,
                keys: 8,
                span: 2,
                duration: Duration::from_millis(200),
            };
            let measured = measure(members.clone(), id, protocol, load, until()).unwrap();
            assert!(measured.committed > 0, "{protocol:?}: {measured:?}");
            assert_eq!(measured.unknown, 0, "{protocol:?}");
            let q = |p| measured.latencies.quantile(p);
            assert!(q(500) <= q(990) && q(990) <= q(999), "{protocol:?}");
            assert!(measured.record("test").contains(protocol_name(protocol)));
        }
    }

    #[test]
    fn ac_t1_10_5_seeded_histories_over_two_endpoints_show_no_anomaly() {
        let dir = tempfile::tempdir().unwrap();
        let (_databases, members) = two_endpoints(dir.path(), until());
        let shape = Shape {
            ops: 24,
            clients: 3,
            keys: 3,
        };
        for (id, protocol) in [(1, Protocol::TwoPhase), (2, Protocol::Parallel)] {
            let mut crossed = 0;
            for seed in 0..3 {
                let checked = elle(members.clone(), id, protocol, seed, shape, until()).unwrap();
                assert_eq!(
                    checked.anomalies,
                    Vec::<String>::new(),
                    "{protocol:?} seed {seed}"
                );
                assert_eq!(checked.unknown, 0, "{protocol:?} seed {seed}");
                crossed += checked.crossed;
            }
            assert!(crossed > 0, "{protocol:?}: nothing committed across groups");
        }
    }

    #[test]
    fn ac_t1_10_5_the_lan_check_finds_a_lost_update_and_an_aborted_read() {
        let now = Instant::now();
        let op = |heard, writes: &[(&[u8], &[u8])], reads: &[(&[u8], Option<&[u8]>)]| Op {
            invoke: now,
            done: Some(now),
            heard,
            writes: writes
                .iter()
                .map(|(k, v)| (k.to_vec(), v.to_vec()))
                .collect(),
            reads: reads
                .iter()
                .map(|(k, v)| (k.to_vec(), v.map(<[u8]>::to_vec)))
                .collect(),
        };
        let lost = [
            op(Heard::Committed, &[(b"k", b"a")], &[(b"k", None)]),
            op(Heard::Committed, &[(b"k", b"b")], &[(b"k", None)]),
        ];
        let finals = [(b"k".to_vec(), Some(b"b".to_vec()))].into();
        assert!(
            check(&lost, &finals)
                .iter()
                .any(|a| a.starts_with("lost update"))
        );
        let dirty = [
            op(Heard::Aborted, &[(b"k", b"a")], &[]),
            op(Heard::Committed, &[], &[(b"k", Some(b"a"))]),
        ];
        let finals = [(b"k".to_vec(), None)].into();
        assert!(check(&dirty, &finals).iter().any(|a| a.starts_with("G1a")));
    }

    #[test]
    fn ac_t1_10_2_latency_buckets_hold_a_value_within_a_128th() {
        for nanos in [
            0u64,
            1,
            127,
            128,
            129,
            1_000,
            999_999,
            10_000_000,
            u64::MAX / 3,
        ] {
            let b = bucket(nanos);
            assert!(b < BUCKETS, "{nanos}");
            let top = upper(b);
            assert!(top >= nanos && top - nanos <= nanos / 128, "{nanos}: {top}");
        }
    }
}
