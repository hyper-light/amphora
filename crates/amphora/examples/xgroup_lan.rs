//! G13 step 3's LAN harness (CP-T1.10: AC-T1.10.2, AC-T1.10.5; docs/design/xgroup.md, "Over the
//! transport"). Each group is a database on a node of its own, its owner listening at the
//! database's endpoint; the coordinator reaches every group over the transport.
//!
//! ```text
//! xgroup_lan node  --path DB --secs S                 serve one group for S seconds
//!                  [--listen ADDR --cert PEM --key PEM --roots PEM]
//! xgroup_lan bench --node 0=DB0 --node 1=DB1 [--protocol two-phase|parallel|both]
//!                  --clients N --keys K --span W --secs S [--id ID] [--host NAME] [--out FILE]
//! xgroup_lan elle  --node 0=DB0 --node 1=DB1 [--protocol ...] --seeds N --ops M --clients C
//!                  --keys K [--id ID] [--secs S]
//! xgroup_lan refusals (--node G=DB ... | --dir DIR --groups G) [--protocol ...] --clients C
//!                  --txns T --keys K --burst B --every E [--refusal not-leader|full] --secs S
//! ```
//!
//! `bench` appends one JSON line per protocol to `--out` (bench/records). `elle` prints each seed
//! with an anomaly and the totals, and exits non-zero on any anomaly or unknown outcome.
//!
//! Between machines (T-T1.9.R18; reshard.md, "The transport between machines"): `node --listen
//! ADDR` also serves the group at a network address under mutual TLS, with the node's certificate
//! chain, key and the authority's roots (PEM); `bench` and `elle` given `--cert`, `--key` and
//! `--roots` dial `--node G=NAME@ADDR`, NAME the name the node's certificate carries.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use amphora::reshard::NodeId;
use amphora::transport::{Nodes, network::Credentials};
use amphora::xgroup::lan::{self, Load, Shape};
use amphora::xgroup::{Member, Protocol};

struct Args {
    command: String,
    flags: BTreeMap<String, Vec<String>>,
}

impl Args {
    fn parse() -> anyhow::Result<Self> {
        let mut args = std::env::args_os()
            .skip(1)
            .map(|a| {
                a.into_string()
                    .map_err(|a| anyhow::anyhow!("{} isn't Unicode", a.to_string_lossy()))
            })
            .collect::<anyhow::Result<Vec<String>>>()?
            .into_iter();
        let command = args
            .next()
            .ok_or_else(|| anyhow::anyhow!("a command: node, bench, elle or refusals"))?;
        let mut flags: BTreeMap<String, Vec<String>> = BTreeMap::new();
        while let Some(flag) = args.next() {
            let name = flag
                .strip_prefix("--")
                .ok_or_else(|| anyhow::anyhow!("{flag} isn't a flag"))?;
            let value = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("--{name} needs a value"))?;
            flags.entry(name.to_owned()).or_default().push(value);
        }
        Ok(Self { command, flags })
    }

    fn one(&self, name: &str) -> anyhow::Result<&str> {
        self.flags
            .get(name)
            .and_then(|v| v.last())
            .map(String::as_str)
            .ok_or_else(|| anyhow::anyhow!("--{name} is needed"))
    }

    fn number<T: std::str::FromStr>(&self, name: &str) -> anyhow::Result<T>
    where
        T::Err: std::fmt::Display,
    {
        self.one(name)?
            .parse()
            .map_err(|e| anyhow::anyhow!("--{name}: {e}"))
    }

    fn or<T: std::str::FromStr>(&self, name: &str, default: T) -> anyhow::Result<T>
    where
        T::Err: std::fmt::Display,
    {
        if self.flags.contains_key(name) {
            self.number(name)
        } else {
            Ok(default)
        }
    }

    fn secs(&self) -> anyhow::Result<Duration> {
        Ok(Duration::try_from_secs_f64(self.number::<f64>("secs")?)?)
    }

    /// This node's credentials, `--cert`, `--key` and `--roots` (PEM), if given.
    fn credentials(&self) -> anyhow::Result<Option<Credentials>> {
        if !self.flags.contains_key("cert") {
            return Ok(None);
        }
        Ok(Some(Credentials::from_pem_files(
            Path::new(self.one("cert")?),
            Path::new(self.one("key")?),
            Path::new(self.one("roots")?),
        )?))
    }

    /// The groups, `--node G=ENDPOINT` each: group G's owner at ENDPOINT, its database's path, or
    /// with credentials `NAME@ADDR`, its network endpoint and the name its certificate carries.
    fn members(&self) -> anyhow::Result<Vec<Member>> {
        let mut endpoints = BTreeMap::new();
        for node in self.flags.get("node").into_iter().flatten() {
            let (group, endpoint) = node
                .split_once('=')
                .ok_or_else(|| anyhow::anyhow!("--node {node}: GROUP=ENDPOINT"))?;
            endpoints.insert(group.parse::<u64>()?, endpoint.to_owned());
        }
        let count = u64::try_from(endpoints.len())?;
        if endpoints.keys().copied().ne(0..count) {
            anyhow::bail!("--node: the groups are numbered 0 to one less than how many there are");
        }
        let nodes = Arc::new(match self.credentials()? {
            None => Nodes::new(
                endpoints
                    .into_iter()
                    .map(|(g, p)| (NodeId(g), PathBuf::from(p))),
            ),
            Some(credentials) => {
                let mut network = Vec::new();
                for (g, endpoint) in endpoints {
                    let (name, address) = endpoint
                        .split_once('@')
                        .ok_or_else(|| anyhow::anyhow!("--node {g}={endpoint}: GROUP=NAME@ADDR"))?;
                    network.push((NodeId(g), address.parse()?, name.to_owned()));
                }
                Nodes::network(network, &credentials)?
            }
        });
        Ok((0..count)
            .map(|g| Member::Node {
                nodes: Arc::clone(&nodes),
                node: NodeId(g),
            })
            .collect())
    }

    fn protocols(&self) -> anyhow::Result<Vec<Protocol>> {
        match self
            .flags
            .get("protocol")
            .and_then(|v| v.last())
            .map_or("both", String::as_str)
        {
            "two-phase" => Ok(vec![Protocol::TwoPhase]),
            "parallel" => Ok(vec![Protocol::Parallel]),
            "both" => Ok(vec![Protocol::TwoPhase, Protocol::Parallel]),
            other => anyhow::bail!("--protocol {other}: two-phase, parallel or both"),
        }
    }
}

fn later(by: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(by).unwrap_or(now)
}

/// A line of the command's output on stdout: its records and what it serves.
macro_rules! out {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        writeln!(std::io::stdout().lock(), $($arg)*)?;
    }};
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse()?;
    match args.command.as_str() {
        "node" => {
            let path = PathBuf::from(args.one("path")?);
            let secs = args.secs()?;
            let served = later(secs);
            let database = amphora::database::Database::open(&path, served)?;
            amphora::xgroup::sql::prepare(&database, served)?;
            let _listening = match args.credentials()? {
                Some(credentials) => {
                    let listening = database.listen(args.one("listen")?.parse()?, &credentials)?;
                    if let Some(address) = listening.address() {
                        out!("listening at {address}");
                    }
                    Some(listening)
                }
                None => None,
            };
            out!("serving {} for {secs:?}", path.display());
            while Instant::now() < served {
                std::thread::park_timeout(served.saturating_duration_since(Instant::now()));
            }
            Ok(())
        }
        "bench" => {
            let members = args.members()?;
            let load = Load {
                clients: args.number("clients")?,
                keys: args.number("keys")?,
                span: args.number("span")?,
                duration: args.secs()?,
            };
            let id: u64 = args.or("id", 1)?;
            let host = args.or("host", String::from("unnamed"))?;
            let mut lines = Vec::new();
            for (i, protocol) in args.protocols()?.into_iter().enumerate() {
                // The run's own time, and as long again for its coordinator to open and finish.
                let until = later(load.duration.saturating_mul(2));
                let measured = lan::measure(
                    members.clone(),
                    id.wrapping_add(u64::try_from(i)?),
                    protocol,
                    load,
                    until,
                )?;
                let line = measured.record(&host);
                out!("{line}");
                lines.push(line);
            }
            if let Ok(out) = args.one("out") {
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(out)?;
                for line in lines {
                    writeln!(file, "{line}")?;
                }
            }
            Ok(())
        }
        "elle" => {
            let members = args.members()?;
            let seeds: u64 = args.number("seeds")?;
            let shape = Shape {
                ops: args.number("ops")?,
                clients: args.number("clients")?,
                keys: args.number("keys")?,
            };
            let id: u64 = args.or("id", 1)?;
            let budget = Duration::try_from_secs_f64(args.number::<f64>("secs")?)?;
            let mut failed = 0u64;
            for (i, protocol) in args.protocols()?.into_iter().enumerate() {
                let id = id.wrapping_add(u64::try_from(i)?);
                let (mut committed, mut crossed, mut aborted, mut unknown, mut anomalies) =
                    (0usize, 0usize, 0usize, 0usize, 0usize);
                for seed in 0..seeds {
                    let checked =
                        lan::elle(members.clone(), id, protocol, seed, shape, later(budget))?;
                    committed = committed.saturating_add(checked.committed);
                    crossed = crossed.saturating_add(checked.crossed);
                    aborted = aborted.saturating_add(checked.aborted);
                    unknown = unknown.saturating_add(checked.unknown);
                    anomalies = anomalies.saturating_add(checked.anomalies.len());
                    if !checked.anomalies.is_empty() || checked.unknown > 0 {
                        failed = failed.saturating_add(1);
                        out!(
                            "seed {seed} {}: {} unknown, {:#?}",
                            lan::protocol_name(protocol),
                            checked.unknown,
                            checked.anomalies
                        );
                    }
                }
                out!(
                    "{{\"bench\":\"xgroup_elle\",\"protocol\":\"{}\",\"seeds\":{seeds},\"committed\":{committed},\"crossed\":{crossed},\"aborted\":{aborted},\"unknown\":{unknown},\"anomalies\":{anomalies}}}",
                    lan::protocol_name(protocol)
                );
            }
            if failed > 0 {
                anyhow::bail!("{failed} seeds with an anomaly or an unknown outcome");
            }
            Ok(())
        }
        "refusals" => refusals(&args),
        other => anyhow::bail!("{other}: node, bench, elle or refusals"),
    }
}

/// What one run of `refusals` counted over every incarnation of its coordinator.
#[derive(Default)]
struct Tally {
    committed: u64,
    crossed: u64,
    aborted_conflict: u64,
    aborted_lost: u64,
    aborted_late: u64,
    unknown: u64,
    failed: u64,
    stopped_seen: u64,
    reopened: u64,
    refused_not_leader: u64,
    refused_full: u64,
    refused_other: u64,
    retries: u64,
    held_txns_most: u64,
    stops: u64,
    latencies: lan::Latencies,
}

impl Tally {
    fn fold(&mut self, counted: &amphora::xgroup::sql::Counted) {
        self.refused_not_leader = self
            .refused_not_leader
            .saturating_add(counted.refused_not_leader);
        self.refused_full = self.refused_full.saturating_add(counted.refused_full);
        self.refused_other = self.refused_other.saturating_add(counted.refused_other);
        self.retries = self.retries.saturating_add(counted.retries);
        self.held_txns_most = self.held_txns_most.max(counted.held_txns_most);
        self.stops = self.stops.saturating_add(counted.stops);
    }
}

/// SplitMix64 (Steele, Lea and Flood, "Fast splittable pseudorandom number generators", 2014):
/// each caller's keys, seeded by its number so a run is the same run again.
struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// `refusals` (AC-T1.10.6; docs/design/xgroup.md, "A refusal before the step"): the switch
/// `Options::on_refused` at scale, `Stop` (the control) against `Retry`, under both protocols.
/// `--clients` callers each run `--txns` transactions over `--keys` keys (each reads one key and
/// writes the two after another, routed by their first byte), while every group refuses together,
/// as a fleet restarting at once would: of each `--every` steps sent, the first `--burst` are
/// refused `--refusal` (not-leader or full) before they are taken. The groups are the `--node`s, or
/// `--groups` local databases under `--dir`. `--secs`: each transaction's deadline. A coordinator
/// that stops is opened again, as its process would be, and counted. One JSON line a run.
fn refusals(args: &Args) -> anyhow::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, PoisonError, RwLock};

    use amphora::xgroup::sql::{Fault, OnRefused, Options};
    use amphora::xgroup::{Answer, Cluster, Route, Txn, Why};
    use amphora_api::Refusal;

    let clients: usize = args.number("clients")?;
    let txns: u64 = args.number("txns")?;
    let keys: u64 = args.number("keys")?;
    let burst: u64 = args.number("burst")?;
    let every: u64 = args.number("every")?;
    let deadline = args.secs()?;
    let id: u64 = args.or("id", 1)?;
    let refusal_name = args.or("refusal", String::from("not-leader"))?;
    let refusal: fn() -> Refusal = match refusal_name.as_str() {
        "not-leader" => || Refusal::NotLeader { leader: None },
        "full" => || Refusal::Full { limit: 0 },
        other => anyhow::bail!("--refusal {other}: not-leader or full"),
    };
    if every == 0 || burst > every || keys == 0 || clients == 0 {
        anyhow::bail!("--every above 0 and at least --burst; --keys and --clients above 0");
    }
    let key = |k: u64| k.checked_rem(keys).unwrap_or(0).to_le_bytes().to_vec();
    let mut run = 0u64;
    for protocol in args.protocols()? {
        for (setting, on_refused) in [("stop", OnRefused::Stop), ("retry", OnRefused::Retry)] {
            let id = id.wrapping_add(run);
            run = run.wrapping_add(1);
            let members: Vec<Member> = if args.flags.contains_key("dir") {
                let dir = PathBuf::from(args.one("dir")?)
                    .join(format!("{}-{setting}", lan::protocol_name(protocol)));
                std::fs::create_dir_all(&dir)?;
                let groups: u32 = args.number("groups")?;
                (0..groups)
                    .map(|g| -> anyhow::Result<Member> {
                        let path = dir.join(format!("group{g}.db"));
                        let database = amphora::database::Database::open(&path, later(deadline))?;
                        amphora::xgroup::sql::prepare(&database, later(deadline))?;
                        Ok(Member::Local(database))
                    })
                    .collect::<anyhow::Result<_>>()?
            } else {
                args.members()?
            };
            let count = u32::try_from(members.len())?;
            if count == 0 {
                anyhow::bail!("no groups: --node, or --dir and --groups");
            }
            let route: Route = Arc::new(move |key: &[u8]| {
                key.first()
                    .map_or(0, |b| u32::from(*b).checked_rem(count).unwrap_or(0))
            });
            let open = |fault: Option<Fault>| {
                Cluster::open_with(
                    members.clone(),
                    Arc::clone(&route),
                    id,
                    protocol,
                    clients,
                    Options { on_refused, fault },
                    later(deadline),
                )
            };
            // Every group answers once first, so the steps' learned times have a sample.
            {
                let warm = open(None)?;
                let txn = Txn {
                    reads: Vec::new(),
                    writes: vec![(key(0), Some(vec![0])), (key(1), Some(vec![0]))],
                    conds: Vec::new(),
                };
                warm.commit(txn, later(deadline))?;
            }
            let sent = Arc::new(AtomicU64::new(0));
            let fault: Fault = {
                let sent = Arc::clone(&sent);
                Arc::new(move |_, envelope| {
                    let n = sent.fetch_add(1, Ordering::AcqRel);
                    (envelope.step.txn().is_some()
                        && n.checked_rem(every).is_some_and(|at| at < burst))
                    .then(refusal)
                })
            };
            let current = RwLock::new((0u64, Arc::new(open(Some(Arc::clone(&fault)))?)));
            let tally = Mutex::new(Tally::default());
            let began = Instant::now();
            std::thread::scope(|scope| -> anyhow::Result<()> {
                let callers: Vec<_> = (0..clients)
                    .map(|c| {
                        let (current, tally, open, fault, key, route) =
                            (&current, &tally, &open, &fault, &key, &route);
                        scope.spawn(move || -> anyhow::Result<()> {
                            let mut rng = Seeded(u64::try_from(c)?);
                            for t in 0..txns {
                                let k = rng.next();
                                let (a, b, read) =
                                    (key(k), key(k.wrapping_add(1)), key(k.wrapping_add(2)));
                                let crossed = route(&a) != route(&b) || route(&a) != route(&read);
                                let value = [c.to_le_bytes(), t.to_le_bytes()].concat();
                                let txn = Txn {
                                    reads: vec![read],
                                    writes: vec![(a, Some(value.clone())), (b, Some(value))],
                                    conds: Vec::new(),
                                };
                                let (generation, cluster) = {
                                    let held =
                                        current.read().unwrap_or_else(PoisonError::into_inner);
                                    (held.0, Arc::clone(&held.1))
                                };
                                let asked = Instant::now();
                                let answer = cluster.commit(txn, later(deadline));
                                let took = asked.elapsed();
                                drop(cluster);
                                let mut stopped = false;
                                {
                                    let mut tally =
                                        tally.lock().unwrap_or_else(PoisonError::into_inner);
                                    match answer {
                                        Ok(Answer::Committed { .. }) => {
                                            tally.committed = tally.committed.saturating_add(1);
                                            tally.crossed =
                                                tally.crossed.saturating_add(u64::from(crossed));
                                            tally.latencies.record(took);
                                        }
                                        Ok(Answer::Aborted(Why::Conflict(_))) => {
                                            tally.aborted_conflict =
                                                tally.aborted_conflict.saturating_add(1);
                                        }
                                        Ok(Answer::Aborted(Why::Lost)) => {
                                            tally.aborted_lost =
                                                tally.aborted_lost.saturating_add(1);
                                        }
                                        Ok(Answer::Aborted(Why::Late)) => {
                                            tally.aborted_late =
                                                tally.aborted_late.saturating_add(1);
                                        }
                                        Ok(Answer::Unknown)
                                        | Err(Refusal::Timeout | Refusal::UnknownOutcome) => {
                                            tally.unknown = tally.unknown.saturating_add(1);
                                        }
                                        Err(Refusal::Unavailable(why))
                                            if why.contains("stopped") =>
                                        {
                                            tally.stopped_seen =
                                                tally.stopped_seen.saturating_add(1);
                                            stopped = true;
                                        }
                                        // It names no conditions, so none can fail.
                                        Ok(Answer::Aborted(Why::Unmet(_))) | Err(_) => {
                                            tally.failed = tally.failed.saturating_add(1);
                                        }
                                    }
                                }
                                if stopped {
                                    let mut held =
                                        current.write().unwrap_or_else(PoisonError::into_inner);
                                    if held.0 == generation {
                                        let counted = held.1.counted();
                                        let mut tally =
                                            tally.lock().unwrap_or_else(PoisonError::into_inner);
                                        tally.fold(&counted);
                                        tally.reopened = tally.reopened.saturating_add(1);
                                        drop(tally);
                                        *held = (
                                            generation.wrapping_add(1),
                                            Arc::new(open(Some(Arc::clone(fault)))?),
                                        );
                                    }
                                }
                            }
                            Ok(())
                        })
                    })
                    .collect();
                for caller in callers {
                    caller
                        .join()
                        .map_err(|_| anyhow::anyhow!("a caller's thread panicked"))??;
                }
                Ok(())
            })?;
            let secs = began.elapsed().as_secs_f64();
            let (_, last) = current.into_inner().unwrap_or_else(PoisonError::into_inner);
            let mut tally = tally.into_inner().unwrap_or_else(PoisonError::into_inner);
            tally.fold(&last.counted());
            let [p50, p99, p999] = [500, 990, 999].map(|m| tally.latencies.quantile(m).as_micros());
            let total = u64::try_from(clients)?.saturating_mul(txns);
            out!(
                "{{\"bench\":\"xgroup_refusals\",\"protocol\":\"{}\",\"on_refused\":\"{setting}\",\"refusal\":\"{refusal_name}\",\"groups\":{count},\"clients\":{clients},\"txns\":{total},\"keys\":{keys},\"burst\":{burst},\"every\":{every},\"steps_sent\":{},\"committed\":{},\"crossed\":{},\"aborted_conflict\":{},\"aborted_lost\":{},\"aborted_late\":{},\"unknown\":{},\"failed\":{},\"stopped_seen\":{},\"reopened\":{},\"refused_not_leader\":{},\"refused_full\":{},\"refused_other\":{},\"retries\":{},\"held_txns_most\":{},\"stops\":{},\"p50_us\":{p50},\"p99_us\":{p99},\"p999_us\":{p999},\"secs\":{secs:.3},\"committed_per_s\":{:.1}}}",
                lan::protocol_name(protocol),
                sent.load(Ordering::Acquire),
                tally.committed,
                tally.crossed,
                tally.aborted_conflict,
                tally.aborted_lost,
                tally.aborted_late,
                tally.unknown,
                tally.failed,
                tally.stopped_seen,
                tally.reopened,
                tally.refused_not_leader,
                tally.refused_full,
                tally.refused_other,
                tally.retries,
                tally.held_txns_most,
                tally.stops,
                f64::from(u32::try_from(tally.committed).unwrap_or(u32::MAX))
                    / secs.max(f64::MIN_POSITIVE),
            );
        }
    }
    Ok(())
}
