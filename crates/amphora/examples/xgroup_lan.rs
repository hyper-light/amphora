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
            .ok_or_else(|| anyhow::anyhow!("a command: node, bench or elle"))?;
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
        other => anyhow::bail!("{other}: node, bench or elle"),
    }
}
