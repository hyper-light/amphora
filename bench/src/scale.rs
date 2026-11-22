//! Scale-out and regions measured (CP-T1.9 to CP-T1.11; kiln plans "G13 Scale-out", steps 1 and 5,
//! and "G14 Regions", steps 1 and 2): Amphora, CockroachDB and TiDB on the same workloads, each a
//! cluster of `--nodes` nodes, its data in `--groups` groups (Amphora's replicated groups; the
//! others' ranges or regions, split at the same points).
//!
//! - `write` (the scale-out write class): `--writers` closed-loop writers, each its own session of
//!   `--batch` rows a write, through node `writer % nodes`, for `--seconds` after `--warmup-s`.
//! - `read` (the scale-out read class): `--rows` rows written first, then `--writers` closed-loop
//!   readers of one row each by its key, answered by its group's leader (leaseholder).
//! - `follower` (AC-T1.10.C2): the same rows read on a member that doesn't lead their group, at
//!   the time it can read without waiting: Amphora's closed time, CockroachDB's
//!   `follower_read_timestamp()`, TiDB's `tidb_replica_read = 'follower'`.
//! - `xgroup` (AC-T1.10.2): transactions writing `--span` keys, each in a different group, of
//!   `--keys` keys spread evenly over the groups (an upsert of each, so nothing is loaded first);
//!   Amphora's under `--protocol` (2pc or parallel).
//! - `region` (AC-T1.11.2): each node its own region, `--between-ms` one-way between any two
//!   (Amphora's relay holds each record that long; the containers' by netem on their peers' traffic
//!   only); each writer writes rows homed in its node's region.
//! - `regionloss` (G14 step 2; AC-T1.11.3, T-T1.11.1): writers run; after `--settle-s` region
//!   `--lose`'s nodes are killed (SIGKILL; Amphora's members cut off) and a probe writes through the
//!   rest until one write is acknowledged: the time to that acknowledgement. Writers go on for
//!   `--after-s`, the region is started again, and its own copy is polled every `--poll-us` until
//!   it holds every acknowledged row: the time to serve again. Every writer resends a refused write
//!   unchanged, so every row up to its last acknowledged one was acknowledged; `lost` counts those
//!   missing, read through the leader, and must be 0.
//! - `cutover` (G13 steps 1 and 2; AC-T1.9.2, AC-T1.9.3): writers run; after `--settle-s`, `--ops`
//!   range operations one after another, cycling split, leader move and (when a node holds no copy
//!   of the range) replica move: CockroachDB's `SPLIT AT` and `EXPERIMENTAL_RELOCATE`, TiDB's
//!   `SPLIT TABLE` and PD's `transfer-leader` and `transfer-peer` operators. An operation's pause is
//!   the longest acknowledgement of any write in flight while it ran (an upper bound on the pause
//!   of the writes it moved: every write it overlapped counts); `cutover_ms` summarises the pauses
//!   over the operations (the goal's cutover p99 is its p99), and `lost` counts acknowledged rows
//!   missing at the end, read through the leader, and must be 0. Amphora's is refused until the
//!   shell that carries out its splits, merges and moves over replicated groups is built
//!   (docs/design/reshard.md, "What is left").
//!
//! CockroachDB's and TiDB's nodes run on the machines `--hosts` names (`ENDPOINT=ADDR,...`, node `n`
//! on machine `n` mod their count; bench/servers/scale/lib.sh): one a machine for the runs of
//! record, all on one for a lab. Amphora's groups run in this process on the bench's machine.
//!
//! Every size, duration and interval is an input with no default: nothing is picked here. A system
//! without a measure's concept, or whose cluster can't be brought up here, is refused, typed (the
//! same record as `repl.rs`'s: `"refused": "no-concept" | "unavailable"`, with why).
//!
//! Amphora's groups run in this process, each `--nodes` members relayed as `repl.rs` relays them
//! (member `n` of every group is node `n`, in region `n`). CockroachDB and TiDB run in containers
//! through `bench/servers/scale/<system>.sh up N|down|kill N|start N|endpoint N|delay MS|images`.
//!
//! usage: amphora-bench --scale <write|read|follower|xgroup|region|regionloss|cutover>
//!        --system <amphora|cockroach|tidb> --nodes N --groups N --writers N --deadline-s N [--record FILE]
//!        write, region: --batch N --text-bytes N --warmup-s N --seconds N (region: --between-ms N --poll-us N)
//!        read, follower: --rows N --batch N --text-bytes N --warmup-s N --seconds N
//!        xgroup:     --keys N --span N --warmup-s N --seconds N (amphora: --protocol 2pc|parallel)
//!        regionloss: --batch N --text-bytes N --lose N --settle-s N --after-s N --poll-us N
//!        cutover:    --batch N --text-bytes N --ops N --settle-s N --poll-us N
//!        amphora:    --dir DIR --class persisted|replicated [--spanning N] --relay-records N --relay-pending N
//!        others:     --hosts ENDPOINT=ADDR,... [--scripts DIR (bench/servers/scale)] [--shell PROGRAM (bash)]

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::repl::Refusal;
use crate::{Hist, Rng, summary};

/// One row of the events table (`events(session, seq, body)`, or `regional` with its home region).
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub session: String,
    pub seq: u64,
    pub region: usize,
    pub body: String,
}

/// One client's connection to one node; a worker thread holds one at a time.
pub trait Session {
    /// `rows` in one transaction, into `regional` when `regional`; returns once acknowledged.
    fn write(&mut self, regional: bool, rows: &[Row]) -> anyhow::Result<()>;
    /// Whether row (`session`, `seq`) is there: read by its leader, or as a follower read.
    fn read(&mut self, session: &str, seq: u64, follower: bool) -> anyhow::Result<bool>;
    /// One transaction setting every key to `value`.
    fn add(&mut self, keys: &[i64], value: i64) -> anyhow::Result<()>;
    /// Rows of `session` up to `upto`: on this node's own copy when `own`, else by the leader.
    fn count(&mut self, session: &str, upto: u64, own: bool) -> anyhow::Result<u64>;
}

/// A cluster of `nodes()` nodes, 0 up.
pub trait System: Sync {
    fn nodes(&self) -> usize;
    /// What the measure needs before it runs: tables, splits, regions, injected latency, rows.
    fn prepare(&self, o: &Opts) -> anyhow::Result<()>;
    fn connect(&self, node: usize) -> anyhow::Result<Box<dyn Session + '_>>;
    fn kill(&self, node: usize) -> anyhow::Result<()>;
    fn start(&self, node: usize) -> anyhow::Result<()>;
    /// Range operation `op` of the cutover measure, returning once it has taken effect.
    fn reshard(&self, op: usize, o: &Opts) -> anyhow::Result<()>;
}

pub struct Opts {
    pub measure: String,
    pub system: String,
    pub nodes: usize,
    pub groups: usize,
    pub writers: usize,
    pub deadline: Duration,
    pub record: Option<PathBuf>,
    pub batch: Option<usize>,
    pub text_bytes: Option<usize>,
    pub warmup: Option<Duration>,
    pub seconds: Option<Duration>,
    pub rows: Option<usize>,
    pub keys: Option<usize>,
    pub span: Option<usize>,
    pub between: Option<Duration>,
    pub lose: Option<usize>,
    pub settle: Option<Duration>,
    pub after: Option<Duration>,
    pub poll: Option<Duration>,
    pub protocol: Option<String>,
    pub dir: Option<PathBuf>,
    pub class: Option<String>,
    pub spanning: Option<u32>,
    pub relay_records: Option<usize>,
    pub relay_pending: Option<usize>,
    pub ops: Option<usize>,
    pub hosts: Option<String>,
    pub scripts: PathBuf,
    pub shell: String,
}

fn need<T>(v: Option<T>, flag: &str, measure: &str) -> anyhow::Result<T> {
    v.ok_or_else(|| anyhow::anyhow!("{measure} takes {flag}: every size and duration is an input"))
}

pub const MEASURES: [&str; 7] = [
    "write",
    "read",
    "follower",
    "xgroup",
    "region",
    "regionloss",
    "cutover",
];

pub fn parse(args: &[String]) -> anyhow::Result<Opts> {
    let (mut measure, mut system, mut nodes, mut groups, mut writers, mut deadline) =
        (None, None, None, None, None, None);
    let mut o = Opts {
        measure: String::new(),
        system: String::new(),
        nodes: 0,
        groups: 0,
        writers: 0,
        deadline: Duration::ZERO,
        record: None,
        batch: None,
        text_bytes: None,
        warmup: None,
        seconds: None,
        rows: None,
        keys: None,
        span: None,
        between: None,
        lose: None,
        settle: None,
        after: None,
        poll: None,
        protocol: None,
        dir: None,
        class: None,
        spanning: None,
        relay_records: None,
        relay_pending: None,
        ops: None,
        hosts: None,
        scripts: PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/servers/scale")),
        shell: "bash".into(),
    };
    let secs = |v: &str| -> anyhow::Result<Duration> {
        Ok(Duration::try_from_secs_f64(v.parse::<f64>()?)?)
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut v = || {
            it.next()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("{a} takes a value"))
        };
        match a.as_str() {
            "--scale" => measure = Some(v()?),
            "--system" => system = Some(v()?),
            "--nodes" => nodes = Some(v()?.parse::<usize>()?),
            "--groups" => groups = Some(v()?.parse::<usize>()?),
            "--writers" => writers = Some(v()?.parse::<usize>()?),
            "--deadline-s" => deadline = Some(secs(&v()?)?),
            "--record" => o.record = Some(PathBuf::from(v()?)),
            "--batch" => o.batch = Some(v()?.parse()?),
            "--text-bytes" => o.text_bytes = Some(v()?.parse()?),
            "--warmup-s" => o.warmup = Some(secs(&v()?)?),
            "--seconds" => o.seconds = Some(secs(&v()?)?),
            "--rows" => o.rows = Some(v()?.parse()?),
            "--keys" => o.keys = Some(v()?.parse()?),
            "--span" => o.span = Some(v()?.parse()?),
            "--between-ms" => {
                o.between = Some(Duration::try_from_secs_f64(v()?.parse::<f64>()? / 1000.0)?)
            }
            "--lose" => o.lose = Some(v()?.parse()?),
            "--settle-s" => o.settle = Some(secs(&v()?)?),
            "--after-s" => o.after = Some(secs(&v()?)?),
            "--poll-us" => o.poll = Some(Duration::from_micros(v()?.parse()?)),
            "--protocol" => o.protocol = Some(v()?),
            "--dir" => o.dir = Some(PathBuf::from(v()?)),
            "--class" => o.class = Some(v()?),
            "--spanning" => o.spanning = Some(v()?.parse()?),
            "--relay-records" => o.relay_records = Some(v()?.parse()?),
            "--relay-pending" => o.relay_pending = Some(v()?.parse()?),
            "--ops" => o.ops = Some(v()?.parse()?),
            "--hosts" => o.hosts = Some(v()?),
            "--scripts" => o.scripts = PathBuf::from(v()?),
            "--shell" => o.shell = v()?,
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    o.measure = measure
        .ok_or_else(|| anyhow::anyhow!("--scale names the measure: {}", MEASURES.join(", ")))?;
    let m = o.measure.clone();
    anyhow::ensure!(
        MEASURES.contains(&m.as_str()),
        "unknown measure {m} ({})",
        MEASURES.join(", ")
    );
    o.system = need(system, "--system", &m)?;
    o.nodes = need(nodes, "--nodes", &m)?;
    o.groups = need(groups, "--groups", &m)?;
    o.writers = need(writers, "--writers", &m)?;
    o.deadline = need(deadline, "--deadline-s", &m)?;
    anyhow::ensure!(
        o.nodes >= 3,
        "--nodes takes at least 3: a quorum that survives a node"
    );
    anyhow::ensure!(
        o.groups >= 1 && o.writers >= 1,
        "--groups and --writers take at least 1"
    );
    let rows = |o: &Opts| -> anyhow::Result<()> {
        anyhow::ensure!(
            need(o.batch, "--batch", &m)? > 0,
            "--batch takes at least 1"
        );
        need(o.text_bytes, "--text-bytes", &m)?;
        Ok(())
    };
    let timed = |o: &Opts| -> anyhow::Result<()> {
        need(o.warmup, "--warmup-s", &m)?;
        need(o.seconds, "--seconds", &m)?;
        Ok(())
    };
    match m.as_str() {
        "write" => {
            rows(&o)?;
            timed(&o)?;
        }
        "read" | "follower" => {
            rows(&o)?;
            timed(&o)?;
            anyhow::ensure!(need(o.rows, "--rows", &m)? > 0, "--rows takes at least 1");
        }
        "xgroup" => {
            timed(&o)?;
            let (keys, span) = (need(o.keys, "--keys", &m)?, need(o.span, "--span", &m)?);
            anyhow::ensure!(
                span >= 2 && span <= o.groups,
                "--span takes 2 to --groups: one key in each of that many groups"
            );
            anyhow::ensure!(keys >= o.groups, "--keys takes at least --groups");
            if o.system == "amphora" {
                need(o.protocol.as_deref(), "--protocol", &m)?;
            }
        }
        "region" => {
            rows(&o)?;
            timed(&o)?;
            need(o.between, "--between-ms", &m)?;
            need(o.poll, "--poll-us", &m)?;
        }
        "cutover" => {
            rows(&o)?;
            anyhow::ensure!(need(o.ops, "--ops", &m)? > 0, "--ops takes at least 1");
            need(o.settle, "--settle-s", &m)?;
            need(o.poll, "--poll-us", &m)?;
        }
        _ => {
            rows(&o)?;
            anyhow::ensure!(
                need(o.lose, "--lose", &m)? < o.nodes,
                "--lose names a region, 0 to --nodes less one"
            );
            need(o.settle, "--settle-s", &m)?;
            need(o.after, "--after-s", &m)?;
            need(o.poll, "--poll-us", &m)?;
        }
    }
    if o.system != "amphora" {
        let hosts = need(o.hosts.as_deref(), "--hosts", &m)?;
        anyhow::ensure!(
            hosts.split(',').all(|h| h
                .split_once('=')
                .is_some_and(|(e, a)| !e.is_empty() && !a.is_empty())),
            "--hosts takes ENDPOINT=ADDR,...: each machine's Docker endpoint (local, ssh://..., tcp://...) and the address it's reached by"
        );
    }
    if o.system == "amphora" {
        need(o.dir.as_ref(), "--dir", &m)?;
        anyhow::ensure!(
            need(o.relay_records, "--relay-records", &m)? > 0,
            "--relay-records takes at least 1"
        );
        anyhow::ensure!(
            need(o.relay_pending, "--relay-pending", &m)? > 0,
            "--relay-pending takes at least 1"
        );
        match need(o.class.as_deref(), "--class", &m)? {
            "persisted" => {}
            "replicated" => anyhow::ensure!(
                need(o.spanning, "--spanning", &m)? >= 2,
                "--spanning takes at least 2"
            ),
            other => anyhow::bail!("unknown class {other} (persisted, replicated)"),
        }
    }
    Ok(o)
}

fn us(t: Instant) -> u64 {
    u64::try_from(t.elapsed().as_micros()).unwrap_or(u64::MAX)
}

/// Writer `w`'s session name: the same in every system, so splits fall at the same points.
pub fn session_name(w: usize) -> String {
    format!("s-{w:06}")
}

/// The first session of each group past the first: `writers` sessions in `groups` even runs.
pub fn session_splits(writers: usize, groups: usize) -> Vec<String> {
    (1..groups)
        .map(|g| g * writers / groups)
        .filter(|&w| w > 0 && w < writers)
        .map(session_name)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The first key of each group past the first: `keys` keys in `groups` even runs.
pub fn key_splits(keys: usize, groups: usize) -> Vec<i64> {
    (1..groups)
        .filter_map(|g| i64::try_from(g * keys / groups).ok())
        .collect()
}

/// The group key `k` falls in, by [`key_splits`].
pub fn key_group(k: i64, keys: usize, groups: usize) -> usize {
    let k = usize::try_from(k).unwrap_or(0);
    (0..groups)
        .rev()
        .find(|&g| k >= g * keys / groups)
        .unwrap_or(0)
}

/// A writer's rows: session `w`, sequence numbers from 1, homed in `region`.
struct Writer {
    rng: Rng,
    session: usize,
    seq: u64,
    region: usize,
}

impl Writer {
    fn new(session: usize, region: usize) -> Self {
        Self {
            rng: Rng(0x9e37_79b9_7f4a_7c15 ^ (session as u64).wrapping_add(1)),
            session,
            seq: 0,
            region,
        }
    }

    fn next(&mut self, batch: usize, text_bytes: usize) -> Vec<Row> {
        (0..batch)
            .map(|_| {
                self.seq += 1;
                let e = crate::event(&mut self.rng, self.session, self.seq, text_bytes);
                Row {
                    session: session_name(self.session),
                    seq: self.seq,
                    region: self.region,
                    body: e.text,
                }
            })
            .collect()
    }
}

/// A worker's connection: to node `node` first, then to the next live node whenever one fails.
struct Conn<'a> {
    sys: &'a dyn System,
    down: &'a [AtomicBool],
    node: usize,
    s: Option<Box<dyn Session + 'a>>,
}

impl<'a> Conn<'a> {
    fn new(sys: &'a dyn System, down: &'a [AtomicBool], node: usize) -> Self {
        Self {
            sys,
            down,
            node,
            s: None,
        }
    }

    /// `f` on a live node until it succeeds or `deadline` passes: what `f` does must be the same
    /// write every time (an upsert of the same rows), so a resend is never a second one.
    fn run<T>(
        &mut self,
        deadline: Duration,
        mut f: impl FnMut(&mut dyn Session) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let until = Instant::now() + deadline;
        let n = self.sys.nodes();
        let mut last = None;
        while Instant::now() < until {
            if self
                .down
                .get(self.node)
                .is_some_and(|d| d.load(Ordering::Relaxed))
            {
                self.s = None;
                self.node = (self.node + 1) % n;
                continue;
            }
            if self.s.is_none() {
                match self.sys.connect(self.node) {
                    Ok(s) => self.s = Some(s),
                    Err(e) => {
                        last = Some(e);
                        self.node = (self.node + 1) % n;
                        continue;
                    }
                }
            }
            let Some(s) = self.s.as_mut() else { continue };
            match f(s.as_mut()) {
                Ok(v) => return Ok(v),
                Err(e) => {
                    last = Some(e);
                    self.s = None;
                    self.node = (self.node + 1) % n;
                }
            }
        }
        Err(anyhow::anyhow!(
            "no live node answered within --deadline-s: {last:?}"
        ))
    }
}

/// `writers` closed-loop workers running `op` for `--seconds` after `--warmup-s`: the latency of
/// each op after the warm-up, and how many.
fn closed_loop<F>(sys: &dyn System, o: &Opts, op: F) -> anyhow::Result<serde_json::Value>
where
    F: Fn(usize, &mut Conn<'_>, &mut Rng) -> anyhow::Result<()> + Sync,
{
    let m = &o.measure;
    let (warmup, seconds) = (
        need(o.warmup, "--warmup-s", m)?,
        need(o.seconds, "--seconds", m)?,
    );
    let down: Vec<AtomicBool> = (0..sys.nodes()).map(|_| AtomicBool::new(false)).collect();
    let start = Instant::now();
    let (counting, ends) = (start + warmup, start + warmup + seconds);
    let hists = std::thread::scope(|scope| -> anyhow::Result<Vec<(Hist, u64)>> {
        let handles: Vec<_> = (0..o.writers)
            .map(|w| {
                let (down, op) = (&down, &op);
                scope.spawn(move || -> anyhow::Result<(Hist, u64)> {
                    let mut conn = Conn::new(sys, down, w % sys.nodes());
                    let mut rng = Rng(0x2545_f491_4f6c_dd1d ^ (w as u64).wrapping_add(1));
                    let (mut h, mut ops) = (Hist::new(), 0u64);
                    loop {
                        let t = Instant::now();
                        if t >= ends {
                            return Ok((h, ops));
                        }
                        op(w, &mut conn, &mut rng)?;
                        if t >= counting {
                            h.record(us(t));
                            ops += 1;
                        }
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().map_err(|_| anyhow::anyhow!("a worker panicked"))?)
            .collect()
    })?;
    let mut all = Hist::new();
    let mut ops = 0u64;
    for (h, n) in &hists {
        all.merge(h);
        ops += n;
    }
    let secs = seconds.as_secs_f64();
    Ok(
        serde_json::json!({ "writers": o.writers, "warmup_s": warmup.as_secs_f64(), "seconds": secs, "latency_ms": summary(all), "ops_per_s": ops as f64 / secs }),
    )
}

fn write(sys: &dyn System, o: &Opts, regional: bool) -> anyhow::Result<serde_json::Value> {
    let (batch, text) = (
        need(o.batch, "--batch", &o.measure)?,
        need(o.text_bytes, "--text-bytes", &o.measure)?,
    );
    let nodes = sys.nodes();
    let writers: Vec<Mutex<Writer>> = (0..o.writers)
        .map(|w| Mutex::new(Writer::new(w, w % nodes)))
        .collect();
    let mut v = closed_loop(sys, o, |w, conn, _| {
        let rows = writers
            .get(w)
            .ok_or_else(|| anyhow::anyhow!("no writer {w}"))?
            .lock()
            .map_err(|_| anyhow::anyhow!("writer {w} poisoned"))?
            .next(batch, text);
        conn.run(o.deadline, |s| s.write(regional, &rows))
    })?;
    if let Some(m) = v.as_object_mut() {
        m.insert("batch".into(), batch.into());
        m.insert("text_bytes".into(), text.into());
        if let Some(b) = o.between {
            m.insert("between_ms".into(), (b.as_secs_f64() * 1000.0).into());
        }
    }
    Ok(v)
}

/// `--rows` rows over the writers' sessions, `--batch` a write; returns each session's last seq.
fn preload(sys: &dyn System, o: &Opts) -> anyhow::Result<Vec<u64>> {
    let m = &o.measure;
    let (rows, batch, text) = (
        need(o.rows, "--rows", m)?,
        need(o.batch, "--batch", m)?,
        need(o.text_bytes, "--text-bytes", m)?,
    );
    let down: Vec<AtomicBool> = (0..sys.nodes()).map(|_| AtomicBool::new(false)).collect();
    let mut conn = Conn::new(sys, &down, 0);
    let mut writers: Vec<Writer> = (0..o.writers)
        .map(|w| Writer::new(w, w % sys.nodes()))
        .collect();
    let mut left = rows;
    let mut w = 0usize;
    while left > 0 {
        let n = left.min(batch);
        let wr = writers
            .get_mut(w % o.writers)
            .ok_or_else(|| anyhow::anyhow!("no writer"))?;
        let r = wr.next(n, text);
        conn.run(o.deadline, |s| s.write(false, &r))?;
        left -= n;
        w += 1;
    }
    Ok(writers.iter().map(|w| w.seq).collect())
}

fn read(sys: &dyn System, o: &Opts, follower: bool) -> anyhow::Result<serde_json::Value> {
    let last = preload(sys, o)?;
    // Before counting: a read of the last row written finds it, so every read after can.
    let down: Vec<AtomicBool> = (0..sys.nodes()).map(|_| AtomicBool::new(false)).collect();
    let mut conn = Conn::new(sys, &down, 1 % sys.nodes());
    let (lw, ls) = last
        .iter()
        .enumerate()
        .rev()
        .find(|(_, s)| **s > 0)
        .map(|(w, s)| (w, *s))
        .ok_or_else(|| anyhow::anyhow!("nothing preloaded"))?;
    let until = Instant::now() + o.deadline;
    while !conn.run(o.deadline, |s| s.read(&session_name(lw), ls, follower))? {
        anyhow::ensure!(
            Instant::now() < until,
            "the last row preloaded wasn't readable within --deadline-s"
        );
        std::thread::yield_now();
    }
    let missed = AtomicU64::new(0);
    let mut v = closed_loop(sys, o, |_, conn, rng| {
        let w = rng.below(last.len());
        let top = last.get(w).copied().unwrap_or(0);
        if top == 0 {
            return Ok(());
        }
        let seq = 1 + rng.next() % top;
        if !conn.run(o.deadline, |s| s.read(&session_name(w), seq, follower))? {
            missed.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    })?;
    if let Some(m) = v.as_object_mut() {
        m.insert("rows".into(), need(o.rows, "--rows", &o.measure)?.into());
        m.insert("missed".into(), missed.load(Ordering::Relaxed).into());
    }
    Ok(v)
}

fn xgroup(sys: &dyn System, o: &Opts) -> anyhow::Result<serde_json::Value> {
    let (keys, span) = (
        need(o.keys, "--keys", &o.measure)?,
        need(o.span, "--span", &o.measure)?,
    );
    let groups = o.groups;
    let mut v = closed_loop(sys, o, |_, conn, rng| {
        // `span` distinct groups, a key in each.
        let mut picked: Vec<usize> = Vec::with_capacity(span);
        while picked.len() < span {
            let g = rng.below(groups);
            if !picked.contains(&g) {
                picked.push(g);
            }
        }
        let ks: Vec<i64> = picked
            .iter()
            .map(|&g| {
                let (lo, hi) = (g * keys / groups, (g + 1) * keys / groups);
                i64::try_from(lo + rng.below(hi.saturating_sub(lo).max(1))).unwrap_or(0)
            })
            .collect();
        let value = i64::try_from(rng.next() >> 1).unwrap_or(0);
        conn.run(o.deadline, |s| s.add(&ks, value))
    })?;
    if let Some(m) = v.as_object_mut() {
        m.insert("keys".into(), keys.into());
        m.insert("span".into(), span.into());
        if let Some(p) = &o.protocol {
            m.insert("protocol".into(), p.clone().into());
        }
    }
    Ok(v)
}

fn region_loss(sys: &dyn System, o: &Opts) -> anyhow::Result<serde_json::Value> {
    let m = &o.measure;
    let (batch, text, lose) = (
        need(o.batch, "--batch", m)?,
        need(o.text_bytes, "--text-bytes", m)?,
        need(o.lose, "--lose", m)?,
    );
    let (settle, after, poll) = (
        need(o.settle, "--settle-s", m)?,
        need(o.after, "--after-s", m)?,
        need(o.poll, "--poll-us", m)?,
    );
    let nodes = sys.nodes();
    let down: Vec<AtomicBool> = (0..nodes).map(|_| AtomicBool::new(false)).collect();
    let stop = AtomicBool::new(false);
    let acked: Vec<AtomicU64> = (0..o.writers).map(|_| AtomicU64::new(0)).collect();
    let failed: Mutex<Option<anyhow::Error>> = Mutex::new(None);
    let mut first_ack_ms = 0.0;
    let serve_ms;
    std::thread::scope(|scope| -> anyhow::Result<()> {
        for w in 0..o.writers {
            let (down, stop, acked, failed) = (&down, &stop, &acked, &failed);
            scope.spawn(move || {
                let mut wr = Writer::new(w, w % nodes);
                let mut conn = Conn::new(sys, down, w % nodes);
                while !stop.load(Ordering::Relaxed) {
                    let rows = wr.next(batch, text);
                    match conn.run(o.deadline, |s| s.write(false, &rows)) {
                        Ok(()) => {
                            if let Some(a) = acked.get(w) {
                                a.store(wr.seq, Ordering::Relaxed);
                            }
                        }
                        Err(e) => {
                            if let Ok(mut f) = failed.lock() {
                                f.get_or_insert(e);
                            }
                            return;
                        }
                    }
                }
            });
        }
        let ran = (|| -> anyhow::Result<()> {
            std::thread::park_timeout(settle);
            let mut probe = Writer::new(o.writers, (lose + 1) % nodes);
            let rows = probe.next(batch, text);
            let mut pc = Conn::new(sys, &down, (lose + 1) % nodes);
            let at = Instant::now();
            down.get(lose)
                .ok_or_else(|| anyhow::anyhow!("no node {lose}"))?
                .store(true, Ordering::Relaxed);
            sys.kill(lose)?;
            pc.run(o.deadline, |s| s.write(false, &rows))?;
            first_ack_ms = us(at) as f64 / 1000.0;
            std::thread::park_timeout(after);
            stop.store(true, Ordering::Relaxed);
            Ok(())
        })();
        stop.store(true, Ordering::Relaxed);
        ran
    })?;
    if let Some(e) = failed
        .lock()
        .map_err(|_| anyhow::anyhow!("a writer's failure was poisoned"))?
        .take()
    {
        return Err(e);
    }
    // The region back: its own copy polled until it holds every acknowledged row.
    let at = Instant::now();
    sys.start(lose)?;
    if let Some(d) = down.get(lose) {
        d.store(false, Ordering::Relaxed);
    }
    let until = Instant::now() + o.deadline;
    loop {
        let caught = sys.connect(lose).and_then(|mut s| {
            for (w, a) in acked.iter().enumerate() {
                let want = a.load(Ordering::Relaxed);
                if s.count(&session_name(w), want, true)? < want {
                    return Ok(false);
                }
            }
            Ok(true)
        });
        if caught.is_ok_and(|c| c) {
            serve_ms = us(at) as f64 / 1000.0;
            break;
        }
        anyhow::ensure!(
            Instant::now() < until,
            "region {lose} didn't serve every acknowledged row within --deadline-s"
        );
        std::thread::park_timeout(poll);
    }
    let mut conn = Conn::new(sys, &down, (lose + 1) % nodes);
    let (mut acknowledged, mut lost) = (0u64, 0u64);
    for (w, a) in acked.iter().enumerate() {
        let want = a.load(Ordering::Relaxed);
        let held = conn.run(o.deadline, |s| s.count(&session_name(w), want, false))?;
        acknowledged += want;
        lost += want.saturating_sub(held);
    }
    Ok(serde_json::json!({
        "writers": o.writers, "batch": batch, "text_bytes": text, "lose": lose, "settle_s": settle.as_secs_f64(), "after_s": after.as_secs_f64(),
        "first_ack_ms": first_ack_ms, "serve_again_ms": serve_ms, "acknowledged_rows": acknowledged, "lost": lost,
    }))
}

/// The operations a write overlapped: it began after `ended` operations had ended and was
/// acknowledged once `begun` had begun, so it overlapped operations `ended` to `begun` less one.
pub fn overlapped(ended: usize, begun: usize, ops: usize) -> std::ops::Range<usize> {
    ended.min(ops)..begun.min(ops).max(ended.min(ops))
}

fn cutover(sys: &dyn System, o: &Opts) -> anyhow::Result<serde_json::Value> {
    let m = &o.measure;
    let (batch, text, ops, settle) = (
        need(o.batch, "--batch", m)?,
        need(o.text_bytes, "--text-bytes", m)?,
        need(o.ops, "--ops", m)?,
        need(o.settle, "--settle-s", m)?,
    );
    let nodes = sys.nodes();
    let down: Vec<AtomicBool> = (0..nodes).map(|_| AtomicBool::new(false)).collect();
    let (stop, begun, ended) = (
        AtomicBool::new(false),
        AtomicUsize::new(0),
        AtomicUsize::new(0),
    );
    let acked: Vec<AtomicU64> = (0..o.writers).map(|_| AtomicU64::new(0)).collect();
    // Each operation's pause: the longest acknowledgement of a write it overlapped, microseconds.
    let pause: Vec<AtomicU64> = (0..ops).map(|_| AtomicU64::new(0)).collect();
    let failed: Mutex<Option<anyhow::Error>> = Mutex::new(None);
    let mut took = Hist::new();
    std::thread::scope(|scope| -> anyhow::Result<()> {
        for w in 0..o.writers {
            let (down, stop, begun, ended, acked, pause, failed) =
                (&down, &stop, &begun, &ended, &acked, &pause, &failed);
            scope.spawn(move || {
                let mut wr = Writer::new(w, w % nodes);
                let mut conn = Conn::new(sys, down, w % nodes);
                while !stop.load(Ordering::Relaxed) {
                    let rows = wr.next(batch, text);
                    let (t, before) = (Instant::now(), ended.load(Ordering::SeqCst));
                    match conn.run(o.deadline, |s| s.write(false, &rows)) {
                        Ok(()) => {
                            let lat = us(t);
                            for k in overlapped(before, begun.load(Ordering::SeqCst), ops) {
                                if let Some(p) = pause.get(k) {
                                    p.fetch_max(lat, Ordering::Relaxed);
                                }
                            }
                            if let Some(a) = acked.get(w) {
                                a.store(wr.seq, Ordering::Relaxed);
                            }
                        }
                        Err(e) => {
                            if let Ok(mut f) = failed.lock() {
                                f.get_or_insert(e);
                            }
                            return;
                        }
                    }
                }
            });
        }
        let ran = (|| -> anyhow::Result<()> {
            std::thread::park_timeout(settle);
            for k in 0..ops {
                if failed.lock().map_or(true, |f| f.is_some()) {
                    break;
                }
                let t = Instant::now();
                begun.store(k + 1, Ordering::SeqCst);
                let done = sys.reshard(k, o);
                ended.store(k + 1, Ordering::SeqCst);
                done?;
                took.record(us(t));
            }
            Ok(())
        })();
        stop.store(true, Ordering::Relaxed);
        ran
    })?;
    if let Some(e) = failed
        .lock()
        .map_err(|_| anyhow::anyhow!("a writer's failure was poisoned"))?
        .take()
    {
        return Err(e);
    }
    let mut pauses = Hist::new();
    for p in &pause {
        pauses.record(p.load(Ordering::Relaxed));
    }
    let mut conn = Conn::new(sys, &down, 0);
    let (mut acknowledged, mut lost) = (0u64, 0u64);
    for (w, a) in acked.iter().enumerate() {
        let want = a.load(Ordering::Relaxed);
        let held = conn.run(o.deadline, |s| s.count(&session_name(w), want, false))?;
        acknowledged += want;
        lost += want.saturating_sub(held);
    }
    Ok(serde_json::json!({
        "writers": o.writers, "batch": batch, "text_bytes": text, "ops": ops, "settle_s": settle.as_secs_f64(),
        "cutover_ms": summary(pauses), "op_ms": summary(took), "acknowledged_rows": acknowledged, "lost": lost,
    }))
}

fn record_head(o: &Opts) -> serde_json::Value {
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_default();
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_owned())
        .unwrap_or_default();
    let load = std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(str::to_owned));
    let date = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut head = serde_json::json!({
        "bench": format!("scale_{}", o.measure), "system": o.system, "nodes": o.nodes, "groups": o.groups,
        "host": host, "cpus": std::thread::available_parallelism().map_or(0, usize::from), "commit": commit, "unix_s": date, "load1": load,
    });
    if let (Some(h), Some(class)) = (head.as_object_mut(), &o.class) {
        h.insert("class".into(), class.clone().into());
    }
    // The machines by the addresses they're reached at (their Docker endpoints left out).
    if let (Some(h), Some(hosts)) = (head.as_object_mut(), &o.hosts) {
        h.insert(
            "machines".into(),
            hosts
                .split(',')
                .filter_map(|s| s.split_once('=').map(|(_, a)| a.to_owned()))
                .collect::<Vec<_>>()
                .into(),
        );
    }
    head
}

fn emit(o: &Opts, mut head: serde_json::Value, body: serde_json::Value) -> anyhow::Result<()> {
    if let (Some(h), Some(b)) = (head.as_object_mut(), body.as_object()) {
        for (k, v) in b {
            h.insert(k.clone(), v.clone());
        }
    }
    let line = head.to_string();
    println!("{line}");
    if let Some(path) = &o.record {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(f, "{line}")?;
    }
    Ok(())
}

fn open(o: &Opts) -> anyhow::Result<Box<dyn System>> {
    Ok(match o.system.as_str() {
        "amphora" => Box::new(inproc::Amphora::open(o)?),
        "cockroach" | "tidb" => Box::new(sql::Containers::up(o)?),
        other => anyhow::bail!("unknown system {other} (amphora, cockroach, tidb)"),
    })
}

pub fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let o = parse(&args)?;
    let ran = open(&o).and_then(|sys| {
        sys.prepare(&o)?;
        match o.measure.as_str() {
            "write" => write(sys.as_ref(), &o, false),
            "read" => read(sys.as_ref(), &o, false),
            "follower" => read(sys.as_ref(), &o, true),
            "xgroup" => xgroup(sys.as_ref(), &o),
            "region" => write(sys.as_ref(), &o, true),
            "cutover" => cutover(sys.as_ref(), &o),
            _ => region_loss(sys.as_ref(), &o),
        }
    });
    let head = record_head(&o);
    match ran {
        Ok(body) => emit(&o, head, body),
        Err(e) => match e.downcast_ref::<Refusal>() {
            Some(r) => {
                let (kind, why) = match r {
                    Refusal::NoConcept(w) => ("no-concept", w.clone()),
                    Refusal::Unavailable(w) => ("unavailable", w.clone()),
                };
                emit(&o, head, serde_json::json!({ "refused": kind, "why": why }))?;
                Err(e)
            }
            None => Err(e),
        },
    }
}

/// Amphora's groups in this process: each `--nodes` replicated members and a relay of its own;
/// member `n` of every group is node `n`, and a killed node is cut off from every relay.
mod inproc {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Arc;

    use amphora::database::Database;
    use amphora::xgroup;
    use amphora_api::Value;
    use amphora_api::replication::{Durability, Fates, ReplicaConfig};

    struct Group {
        members: Vec<Database>,
        stop: Arc<AtomicBool>,
        relay: Option<std::thread::JoinHandle<()>>,
    }

    pub struct Amphora {
        groups: Vec<Group>,
        cut: Arc<Vec<AtomicBool>>,
        deadline: Duration,
        xg: Mutex<Option<Arc<xgroup::Cluster>>>,
        keys: usize,
        writers: usize,
    }

    const EVENTS: &str = "CREATE TABLE IF NOT EXISTS events(session TEXT NOT NULL, seq INTEGER NOT NULL, body TEXT NOT NULL, PRIMARY KEY(session, seq));\
        CREATE TABLE IF NOT EXISTS regional(region TEXT NOT NULL, session TEXT NOT NULL, seq INTEGER NOT NULL, body TEXT NOT NULL, PRIMARY KEY(region, session, seq));";

    /// Every member's outgoing records to the member they name, held `between` first (the
    /// region measure's one-way time; zero elsewhere), until `stop`. A cut member's records, out
    /// or in, are lost as a dead node's are. At most `pending` records are held: past it the relay
    /// takes no more from the members until the oldest are delivered.
    fn relay(
        handles: &[amphora::replication::ReplicaHandle],
        cut: &[AtomicBool],
        stop: &AtomicBool,
        wait: Duration,
        records: usize,
        between: Duration,
        pending: usize,
    ) {
        let is_cut = |i: usize| cut.get(i).is_some_and(|c| c.load(Ordering::Relaxed));
        let mut held = VecDeque::new();
        while !stop.load(Ordering::Relaxed) {
            let mut moved = 0usize;
            for (from, handle) in handles.iter().enumerate() {
                let room = pending.saturating_sub(held.len());
                if room == 0 {
                    break;
                }
                for record in handle.drain(records.min(room)).unwrap_or_default() {
                    moved += 1;
                    held.push_back((Instant::now() + between, from, record));
                }
            }
            let now = Instant::now();
            while held.front().is_some_and(|(due, _, _)| *due <= now) {
                let Some((_, from, record)) = held.pop_front() else {
                    break;
                };
                moved += 1;
                let to = usize::try_from(record.to).unwrap_or(0).saturating_sub(1);
                if is_cut(from) || is_cut(to) {
                    continue;
                }
                if let Some(target) = handles.get(to) {
                    let _ = target.receive(from as u64 + 1, record.bytes, Instant::now() + wait);
                }
            }
            if moved == 0 {
                std::thread::yield_now();
            }
        }
    }

    fn value_of(v: &str) -> Value {
        Value::Text(v.to_owned())
    }

    impl Amphora {
        pub fn open(o: &Opts) -> anyhow::Result<Self> {
            let m = &o.measure;
            let dir = need(o.dir.clone(), "--dir", m)?;
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir)?;
            let ids: Vec<u64> = (1..=o.nodes as u64).collect();
            let durability = match o.class.as_deref() {
                // Each member its own fate (the operator's number for what it fails with: here
                // its node ID, one member a node).
                Some("replicated") => Durability::Replicated(Fates {
                    of: ids.iter().map(|&i| (i, i)).collect(),
                    spanning: need(o.spanning, "--spanning", m)?,
                }),
                _ => Durability::Persisted,
            };
            let cut: Arc<Vec<AtomicBool>> =
                Arc::new((0..o.nodes).map(|_| AtomicBool::new(false)).collect());
            let between = if m == "region" {
                need(o.between, "--between-ms", m)?
            } else {
                Duration::ZERO
            };
            let (records, pending) = (
                need(o.relay_records, "--relay-records", m)?,
                need(o.relay_pending, "--relay-pending", m)?,
            );
            let mut groups = Vec::new();
            for g in 0..o.groups {
                let mut members = Vec::new();
                for &node_id in &ids {
                    let config = ReplicaConfig {
                        node_id,
                        voters: ids.clone(),
                        learners: Vec::new(),
                        bootstrap: None,
                    };
                    let db = Database::open_replicated_with(
                        &dir.join(format!("g{g}-state-{node_id}.db")),
                        &dir.join(format!("g{g}-raft-{node_id}.log")),
                        config,
                        durability.clone(),
                        Instant::now() + o.deadline,
                    )
                    .map_err(|e| anyhow::anyhow!("group {g} member {node_id}: {e:?}"))?;
                    members.push(db);
                }
                let handles: Vec<amphora::replication::ReplicaHandle> =
                    members.iter().filter_map(Database::replica).collect();
                anyhow::ensure!(
                    handles.len() == o.nodes,
                    "a member of group {g} has no replica handle"
                );
                let stop = Arc::new(AtomicBool::new(false));
                let (c, s, wait) = (cut.clone(), stop.clone(), o.deadline);
                let relay = std::thread::spawn(move || {
                    relay(&handles, &c, &s, wait, records, between, pending)
                });
                groups.push(Group {
                    members,
                    stop,
                    relay: Some(relay),
                });
            }
            Ok(Self {
                groups,
                cut,
                deadline: o.deadline,
                xg: Mutex::new(None),
                keys: o.keys.unwrap_or(0),
                writers: o.writers,
            })
        }

        fn is_cut(&self, n: usize) -> bool {
            self.cut.get(n).is_some_and(|c| c.load(Ordering::Relaxed))
        }

        fn group(&self, g: usize) -> anyhow::Result<&Group> {
            self.groups
                .get(g)
                .ok_or_else(|| anyhow::anyhow!("no group {g}"))
        }

        /// The leader a live member of group `g` names, if it's live too.
        fn leader(&self, g: usize) -> anyhow::Result<&Database> {
            let group = self.group(g)?;
            for (i, m) in group.members.iter().enumerate() {
                if self.is_cut(i) {
                    continue;
                }
                let Some(status) = m.replica().and_then(|h| h.status().ok()) else {
                    continue;
                };
                let l = usize::try_from(status.leader).unwrap_or(0);
                if l >= 1 && !self.is_cut(l - 1) {
                    return group
                        .members
                        .get(l - 1)
                        .ok_or_else(|| anyhow::anyhow!("no member {l}"));
                }
            }
            anyhow::bail!("group {g} names no live leader")
        }

        /// The group a session's rows are in: the writers' sessions in even runs, where
        /// `session_splits` splits the others (the probe's session, past them, is the last's).
        fn group_of(&self, session: &str) -> usize {
            let w: usize = session.trim_start_matches("s-").parse().unwrap_or(0);
            let n = self.groups.len();
            (0..n)
                .rev()
                .find(|&g| w >= g * self.writers / n.max(1))
                .unwrap_or(0)
        }

        fn exec(&self, g: usize, rows: Vec<(String, Vec<Value>)>) -> anyhow::Result<()> {
            self.leader(g)?
                .call(Instant::now() + self.deadline, move |c| {
                    for (sql, args) in rows {
                        c.prepare_cached(&sql)?.execute(args)?;
                    }
                    Ok(())
                })
                .map_err(|e| anyhow::anyhow!("group {g} refused a write: {e:?}"))
        }
    }

    impl Drop for Amphora {
        fn drop(&mut self) {
            if let Ok(mut xg) = self.xg.lock() {
                xg.take();
            }
            for g in &mut self.groups {
                g.stop.store(true, Ordering::Relaxed);
                if let Some(r) = g.relay.take() {
                    let _ = r.join();
                }
            }
        }
    }

    struct S<'a> {
        a: &'a Amphora,
        node: usize,
    }

    /// A count by `sql` over (`session`, `n`), as a closure every read path runs.
    macro_rules! counted {
        ($sql:expr, $session:expr, $n:expr) => {
            move |c| -> anyhow::Result<i64> {
                let mut s = c.prepare_cached($sql)?;
                let mut rows = s
                    .query_map(vec![Value::Text($session), Value::Integer($n)], |r| {
                        r.get::<_, i64>(0usize)
                    })?;
                Ok(rows.next().transpose()?.unwrap_or(0))
            }
        };
    }
    const ONE: &str = "SELECT count(*) FROM events WHERE session = ?1 AND seq = ?2";
    const UPTO: &str = "SELECT count(*) FROM events WHERE session = ?1 AND seq <= ?2";

    impl Session for S<'_> {
        fn write(&mut self, regional: bool, rows: &[Row]) -> anyhow::Result<()> {
            anyhow::ensure!(!self.a.is_cut(self.node), "node {} is down", self.node);
            let Some(first) = rows.first() else {
                return Ok(());
            };
            let g = self.a.group_of(&first.session);
            let stmts = rows
                .iter()
                .map(|r| -> anyhow::Result<(String, Vec<Value>)> {
                    let seq = Value::Integer(i64::try_from(r.seq)?);
                    Ok(if regional {
                        ("INSERT INTO regional(region, session, seq, body) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(region, session, seq) DO UPDATE SET body = excluded.body".into(), vec![Value::Text(format!("r{}", r.region)), value_of(&r.session), seq, value_of(&r.body)])
                    } else {
                        ("INSERT INTO events(session, seq, body) VALUES (?1, ?2, ?3) ON CONFLICT(session, seq) DO UPDATE SET body = excluded.body".into(), vec![value_of(&r.session), seq, value_of(&r.body)])
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            self.a.exec(g, stmts)
        }

        fn read(&mut self, session: &str, seq: u64, follower: bool) -> anyhow::Result<bool> {
            anyhow::ensure!(!self.a.is_cut(self.node), "node {} is down", self.node);
            let g = self.a.group_of(session);
            let until = Instant::now() + self.a.deadline;
            let (session, seq) = (session.to_owned(), i64::try_from(seq)?);
            let n = if follower {
                // A member that doesn't lead the group, at the time it can read without waiting.
                let leader = self
                    .a
                    .leader(g)?
                    .replica()
                    .and_then(|h| h.status().ok())
                    .map(|s| s.leader)
                    .unwrap_or(0);
                let group = self.a.group(g)?;
                let (i, m) = group
                    .members
                    .iter()
                    .enumerate()
                    .find(|(i, _)| *i as u64 + 1 != leader && !self.a.is_cut(*i))
                    .ok_or_else(|| anyhow::anyhow!("group {g} has no live follower"))?;
                let at = m
                    .follower_read_micros(until)
                    .map_err(|e| anyhow::anyhow!("member {i}: {e:?}"))?;
                m.query_closed(at, until, counted!(ONE, session, seq))
                    .map_err(|e| anyhow::anyhow!("member {i} refused a follower read: {e:?}"))?
                    .value
            } else {
                self.a
                    .leader(g)?
                    .query_local(until, counted!(ONE, session, seq))
                    .map_err(|e| anyhow::anyhow!("group {g} refused a read: {e:?}"))?
            };
            Ok(n > 0)
        }

        fn add(&mut self, keys: &[i64], value: i64) -> anyhow::Result<()> {
            let xg = self
                .a
                .xg
                .lock()
                .map_err(|_| anyhow::anyhow!("the coordinator was poisoned"))?
                .clone()
                .ok_or_else(|| anyhow::anyhow!("xgroup wasn't prepared"))?;
            let writes = keys
                .iter()
                .map(|k| (k.to_be_bytes().to_vec(), Some(value.to_be_bytes().to_vec())))
                .collect();
            match xg
                .transact(
                    xgroup::Txn {
                        reads: Vec::new(),
                        writes,
                        conds: Vec::new(),
                    },
                    Instant::now() + self.a.deadline,
                )
                .map_err(|e| anyhow::anyhow!("{e:?}"))?
            {
                xgroup::Answer::Committed { .. } => Ok(()),
                other => anyhow::bail!("{other:?}"),
            }
        }

        fn count(&mut self, session: &str, upto: u64, own: bool) -> anyhow::Result<u64> {
            let g = self.a.group_of(session);
            let until = Instant::now() + self.a.deadline;
            let (session, upto) = (session.to_owned(), i64::try_from(upto)?);
            let m = if own {
                anyhow::ensure!(!self.a.is_cut(self.node), "node {} is down", self.node);
                self.a
                    .group(g)?
                    .members
                    .get(self.node)
                    .ok_or_else(|| anyhow::anyhow!("no member {}", self.node))?
            } else {
                self.a.leader(g)?
            };
            let n = m
                .query_local(until, counted!(UPTO, session, upto))
                .map_err(|e| anyhow::anyhow!("group {g} refused a count: {e:?}"))?;
            Ok(u64::try_from(n)?)
        }
    }

    impl System for Amphora {
        fn nodes(&self) -> usize {
            self.cut.len()
        }

        fn prepare(&self, o: &Opts) -> anyhow::Result<()> {
            if o.measure == "cutover" {
                return Err(no_reshard_shell());
            }
            let until = Instant::now() + o.deadline;
            for g in 0..self.groups.len() {
                loop {
                    let done = self.leader(g).and_then(|l| {
                        l.call(until, |c| Ok(c.execute_batch(EVENTS)?))
                            .map_err(|e| anyhow::anyhow!("{e:?}"))
                    });
                    match done {
                        Ok(()) => break,
                        Err(e) => anyhow::ensure!(
                            Instant::now() < until,
                            "group {g} took no schema within --deadline-s: {e}"
                        ),
                    }
                    std::thread::yield_now();
                }
            }
            if o.measure == "xgroup" {
                let protocol = match need(o.protocol.as_deref(), "--protocol", &o.measure)? {
                    "2pc" => xgroup::Protocol::TwoPhase,
                    "parallel" => xgroup::Protocol::Parallel,
                    other => anyhow::bail!("unknown protocol {other} (2pc, parallel)"),
                };
                let leaders = (0..self.groups.len())
                    .map(|g| self.leader(g).cloned())
                    .collect::<anyhow::Result<Vec<_>>>()?;
                let (keys, groups) = (self.keys, self.groups.len());
                let route: xgroup::Route = Arc::new(move |k: &[u8]| {
                    let k = <[u8; 8]>::try_from(k).map_or(0, i64::from_be_bytes);
                    u32::try_from(key_group(k, keys, groups)).unwrap_or(0)
                });
                let cluster = xgroup::Cluster::open(leaders, route, 1, protocol, o.writers, until)
                    .map_err(|e| anyhow::anyhow!("{e:?}"))?;
                *self
                    .xg
                    .lock()
                    .map_err(|_| anyhow::anyhow!("the coordinator was poisoned"))? =
                    Some(Arc::new(cluster));
            }
            Ok(())
        }

        fn connect(&self, node: usize) -> anyhow::Result<Box<dyn Session + '_>> {
            anyhow::ensure!(
                node < self.nodes() && !self.is_cut(node),
                "node {node} is down"
            );
            Ok(Box::new(S { a: self, node }))
        }

        fn kill(&self, node: usize) -> anyhow::Result<()> {
            self.cut
                .get(node)
                .ok_or_else(|| anyhow::anyhow!("no node {node}"))?
                .store(true, Ordering::Relaxed);
            Ok(())
        }

        fn start(&self, node: usize) -> anyhow::Result<()> {
            self.cut
                .get(node)
                .ok_or_else(|| anyhow::anyhow!("no node {node}"))?
                .store(false, Ordering::Relaxed);
            Ok(())
        }

        fn reshard(&self, _op: usize, _o: &Opts) -> anyhow::Result<()> {
            Err(no_reshard_shell())
        }
    }

    fn no_reshard_shell() -> anyhow::Error {
        anyhow::Error::from(Refusal::Unavailable("Amphora's splits, merges and moves are a pure core so far: the shell that carries them out over replicated groups isn't built (docs/design/reshard.md, \"What is left\")".into()))
    }
}

/// CockroachDB (Postgres's protocol) and TiDB (MySQL's), each cluster brought up and down by its
/// script; each node in region `r<n>` (CockroachDB's `--locality`, TiKV's labels).
pub mod sql {
    use super::*;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Kind {
        Cockroach,
        Tidb,
    }

    pub struct Containers {
        kind: Kind,
        script: PathBuf,
        shell: String,
        hosts: String,
        endpoints: Vec<String>,
        deadline: Duration,
    }

    /// The statements that make a measure's tables, splits and regions, in the system's dialect.
    pub fn setup(kind: Kind, o: &Opts) -> Vec<String> {
        let mut s = Vec::new();
        match kind {
            Kind::Cockroach => {
                s.push("CREATE TABLE IF NOT EXISTS events (session STRING NOT NULL, seq INT8 NOT NULL, body STRING NOT NULL, PRIMARY KEY (session, seq))".to_owned());
                let splits = session_splits(o.writers, o.groups);
                if !splits.is_empty() {
                    s.push(format!(
                        "ALTER TABLE events SPLIT AT VALUES {}",
                        splits
                            .iter()
                            .map(|v| format!("('{v}')"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                    s.push("ALTER TABLE events SCATTER".into());
                }
                if o.measure == "xgroup" {
                    let keys = o.keys.unwrap_or(0);
                    s.push(
                        "CREATE TABLE IF NOT EXISTS kv (k INT8 PRIMARY KEY, v INT8 NOT NULL)"
                            .into(),
                    );
                    let ks = key_splits(keys, o.groups);
                    if !ks.is_empty() {
                        s.push(format!(
                            "ALTER TABLE kv SPLIT AT VALUES {}",
                            ks.iter()
                                .map(|k| format!("({k})"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                        s.push("ALTER TABLE kv SCATTER".into());
                    }
                }
                if o.measure == "region" {
                    s.push("ALTER DATABASE defaultdb SET PRIMARY REGION \"r0\"".into());
                    for n in 1..o.nodes {
                        s.push(format!("ALTER DATABASE defaultdb ADD REGION \"r{n}\""));
                    }
                    // Each row homed in its writer's gateway's region (crdb_region's default).
                    s.push("CREATE TABLE IF NOT EXISTS regional (session STRING NOT NULL, seq INT8 NOT NULL, body STRING NOT NULL, PRIMARY KEY (session, seq)) LOCALITY REGIONAL BY ROW".into());
                }
            }
            Kind::Tidb => {
                s.push("CREATE TABLE IF NOT EXISTS events (session VARCHAR(64) NOT NULL, seq BIGINT NOT NULL, body TEXT NOT NULL, PRIMARY KEY (session, seq) CLUSTERED)".to_owned());
                let splits = session_splits(o.writers, o.groups);
                if !splits.is_empty() {
                    s.push(format!(
                        "SPLIT TABLE events BY {}",
                        splits
                            .iter()
                            .map(|v| format!("('{v}', 0)"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                if o.measure == "xgroup" {
                    let keys = o.keys.unwrap_or(0);
                    s.push("CREATE TABLE IF NOT EXISTS kv (k BIGINT PRIMARY KEY CLUSTERED, v BIGINT NOT NULL)".into());
                    let ks = key_splits(keys, o.groups);
                    if !ks.is_empty() {
                        s.push(format!(
                            "SPLIT TABLE kv BY {}",
                            ks.iter()
                                .map(|k| format!("({k})"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                }
                if o.measure == "region" {
                    // A policy a region: its partition's leaders there (TiKV's `region` label).
                    let mut parts = Vec::new();
                    for n in 0..o.nodes {
                        s.push(format!("CREATE PLACEMENT POLICY IF NOT EXISTS amphora_bench_r{n} LEADER_CONSTRAINTS='[+region=r{n}]' FOLLOWERS={}", o.nodes - 1));
                        parts.push(format!("PARTITION p{n} VALUES IN ('r{n}') PLACEMENT POLICY = amphora_bench_r{n}"));
                    }
                    s.push(format!("CREATE TABLE IF NOT EXISTS regional (region VARCHAR(8) NOT NULL, session VARCHAR(64) NOT NULL, seq BIGINT NOT NULL, body TEXT NOT NULL, PRIMARY KEY (region, session, seq) CLUSTERED) PARTITION BY LIST COLUMNS (region) ({})", parts.join(", ")));
                }
            }
        }
        s
    }

    impl Containers {
        pub fn up(o: &Opts) -> anyhow::Result<Self> {
            let kind = if o.system == "cockroach" {
                Kind::Cockroach
            } else {
                Kind::Tidb
            };
            let mut c = Self {
                kind,
                script: PathBuf::from(format!("{}/{}.sh", o.scripts.display(), o.system)),
                shell: o.shell.clone(),
                hosts: need(o.hosts.clone(), "--hosts", &o.measure)?,
                endpoints: Vec::new(),
                deadline: o.deadline,
            };
            c.script(&["up", &o.nodes.to_string()]).map_err(|e| {
                Refusal::Unavailable(format!(
                    "{}'s cluster didn't come up through {}: {e}",
                    o.system,
                    c.script.display()
                ))
            })?;
            for n in 0..o.nodes {
                c.endpoints
                    .push(c.script(&["endpoint", &n.to_string()])?.trim().to_owned());
            }
            Ok(c)
        }

        /// The script run by `--shell`, given the run's deadline as its bound on waiting.
        fn script(&self, args: &[&str]) -> anyhow::Result<String> {
            let out = std::process::Command::new(&self.shell)
                .arg(&self.script)
                .args(args)
                .env("WAIT_S", self.deadline.as_secs().max(1).to_string())
                .env("HOSTS", &self.hosts)
                .output()?;
            anyhow::ensure!(
                out.status.success(),
                "{} {}: {}",
                self.script.display(),
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
            Ok(String::from_utf8(out.stdout)?)
        }

        fn endpoint(&self, n: usize) -> anyhow::Result<&str> {
            self.endpoints
                .get(n)
                .map(String::as_str)
                .ok_or_else(|| anyhow::anyhow!("no node {n}"))
        }
    }

    impl Drop for Containers {
        fn drop(&mut self) {
            if let Err(e) = self.script(&["down"]) {
                eprintln!("{} down: {e}", self.script.display());
            }
        }
    }

    struct Pg {
        c: postgres::Client,
    }

    struct My {
        c: mysql::Conn,
        follower: bool,
    }

    impl Session for Pg {
        fn write(&mut self, regional: bool, rows: &[Row]) -> anyhow::Result<()> {
            let table = if regional { "regional" } else { "events" };
            let sql = format!(
                "INSERT INTO {table} (session, seq, body) VALUES ($1, $2, $3) ON CONFLICT (session, seq) DO UPDATE SET body = excluded.body"
            );
            let mut t = self.c.transaction()?;
            for r in rows {
                t.execute(sql.as_str(), &[&r.session, &i64::try_from(r.seq)?, &r.body])?;
            }
            t.commit()?;
            Ok(())
        }

        fn read(&mut self, session: &str, seq: u64, follower: bool) -> anyhow::Result<bool> {
            let sql = if follower {
                "SELECT count(*) FROM events AS OF SYSTEM TIME follower_read_timestamp() WHERE session = $1 AND seq = $2"
            } else {
                "SELECT count(*) FROM events WHERE session = $1 AND seq = $2"
            };
            let n: i64 = self
                .c
                .query_one(sql, &[&session, &i64::try_from(seq)?])?
                .try_get(0)?;
            Ok(n > 0)
        }

        fn add(&mut self, keys: &[i64], value: i64) -> anyhow::Result<()> {
            let mut t = self.c.transaction()?;
            for k in keys {
                t.execute("UPSERT INTO kv (k, v) VALUES ($1, $2)", &[k, &value])?;
            }
            t.commit()?;
            Ok(())
        }

        fn count(&mut self, session: &str, upto: u64, _own: bool) -> anyhow::Result<u64> {
            // CockroachDB answers every read by its leaseholder: a node's own copy isn't readable.
            let n: i64 = self
                .c
                .query_one(
                    "SELECT count(*) FROM events WHERE session = $1 AND seq <= $2",
                    &[&session, &i64::try_from(upto)?],
                )?
                .try_get(0)?;
            Ok(u64::try_from(n)?)
        }
    }

    impl Session for My {
        fn write(&mut self, regional: bool, rows: &[Row]) -> anyhow::Result<()> {
            use mysql::prelude::Queryable;
            let mut t = self.c.start_transaction(mysql::TxOpts::default())?;
            for r in rows {
                if regional {
                    t.exec_drop("INSERT INTO regional (region, session, seq, body) VALUES (?, ?, ?, ?) ON DUPLICATE KEY UPDATE body = VALUES(body)", (format!("r{}", r.region), &r.session, r.seq, &r.body))?;
                } else {
                    t.exec_drop("INSERT INTO events (session, seq, body) VALUES (?, ?, ?) ON DUPLICATE KEY UPDATE body = VALUES(body)", (&r.session, r.seq, &r.body))?;
                }
            }
            t.commit()?;
            Ok(())
        }

        fn read(&mut self, session: &str, seq: u64, follower: bool) -> anyhow::Result<bool> {
            use mysql::prelude::Queryable;
            if follower != self.follower {
                self.c.query_drop(if follower {
                    "SET SESSION tidb_replica_read = 'follower'"
                } else {
                    "SET SESSION tidb_replica_read = 'leader'"
                })?;
                self.follower = follower;
            }
            let n: Option<i64> = self.c.exec_first(
                "SELECT count(*) FROM events WHERE session = ? AND seq = ?",
                (session, seq),
            )?;
            Ok(n.unwrap_or(0) > 0)
        }

        fn add(&mut self, keys: &[i64], value: i64) -> anyhow::Result<()> {
            use mysql::prelude::Queryable;
            let mut t = self.c.start_transaction(mysql::TxOpts::default())?;
            for k in keys {
                t.exec_drop(
                    "INSERT INTO kv (k, v) VALUES (?, ?) ON DUPLICATE KEY UPDATE v = VALUES(v)",
                    (k, value),
                )?;
            }
            t.commit()?;
            Ok(())
        }

        fn count(&mut self, session: &str, upto: u64, own: bool) -> anyhow::Result<u64> {
            use mysql::prelude::Queryable;
            // `own`: a follower read (ReadIndex, so current) rather than the leader's.
            self.c.query_drop(if own {
                "SET SESSION tidb_replica_read = 'follower'"
            } else {
                "SET SESSION tidb_replica_read = 'leader'"
            })?;
            self.follower = own;
            let n: Option<i64> = self.c.exec_first(
                "SELECT count(*) FROM events WHERE session = ? AND seq <= ?",
                (session, upto),
            )?;
            Ok(u64::try_from(n.unwrap_or(0))?)
        }
    }

    impl System for Containers {
        fn nodes(&self) -> usize {
            self.endpoints.len()
        }

        fn prepare(&self, o: &Opts) -> anyhow::Result<()> {
            let stmts = setup(self.kind, o);
            match self.kind {
                Kind::Cockroach => {
                    let ep = self.endpoint(0)?;
                    let mut c = ep
                        .parse::<postgres::Config>()?
                        .connect_timeout(self.deadline)
                        .connect(postgres::NoTls)?;
                    for q in &stmts {
                        c.batch_execute(q)
                            .map_err(|e| anyhow::anyhow!("{q}: {e}"))?;
                    }
                }
                Kind::Tidb => {
                    use mysql::prelude::Queryable;
                    let mut c = mysql::Conn::new(mysql::Opts::from_url(self.endpoint(0)?)?)?;
                    for q in &stmts {
                        c.query_drop(q).map_err(|e| anyhow::anyhow!("{q}: {e}"))?;
                    }
                    if o.measure == "region" {
                        // Placement is applied in the background: wait until every policy is.
                        let until = Instant::now() + o.deadline;
                        loop {
                            let states: Vec<(String, String, String)> =
                                c.query("SHOW PLACEMENT FOR TABLE regional")?;
                            if !states.is_empty()
                                && states.iter().all(|(_, _, st)| st == "SCHEDULED")
                            {
                                break;
                            }
                            anyhow::ensure!(
                                Instant::now() < until,
                                "TiDB's placement wasn't scheduled within --deadline-s: {states:?}"
                            );
                            std::thread::park_timeout(need(o.poll, "--poll-us", &o.measure)?);
                        }
                    }
                }
            }
            if o.measure == "region" {
                let ms = need(o.between, "--between-ms", &o.measure)?.as_secs_f64() * 1000.0;
                self.script(&["delay", &format!("{ms}")])?;
            }
            Ok(())
        }

        fn connect(&self, node: usize) -> anyhow::Result<Box<dyn Session + '_>> {
            let ep = self.endpoint(node)?;
            Ok(match self.kind {
                Kind::Cockroach => Box::new(Pg {
                    c: ep
                        .parse::<postgres::Config>()?
                        .connect_timeout(self.deadline)
                        .connect(postgres::NoTls)?,
                }),
                Kind::Tidb => {
                    let opts = mysql::OptsBuilder::from_opts(mysql::Opts::from_url(ep)?)
                        .tcp_connect_timeout(Some(self.deadline))
                        .read_timeout(Some(self.deadline))
                        .write_timeout(Some(self.deadline));
                    Box::new(My {
                        c: mysql::Conn::new(opts)?,
                        follower: false,
                    })
                }
            })
        }

        fn kill(&self, node: usize) -> anyhow::Result<()> {
            self.script(&["kill", &node.to_string()]).map(|_| ())
        }

        fn start(&self, node: usize) -> anyhow::Result<()> {
            self.script(&["start", &node.to_string()]).map(|_| ())
        }

        fn reshard(&self, op: usize, o: &Opts) -> anyhow::Result<()> {
            let session = session_name((op / 3) % o.writers);
            let what = Op::of(op, self.nodes());
            match self.kind {
                Kind::Cockroach => self.reshard_cockroach(what, &session, op),
                Kind::Tidb => self.reshard_tidb(what, &session, op, o),
            }
        }
    }

    /// The cutover measure's operations, cycled: a split, a leader move, a replica move (a leader
    /// move again when every node already holds a copy: three replicas, three nodes).
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Op {
        Split,
        Leader,
        Replica,
    }

    /// The replicas each range keeps by default: CockroachDB's `num_replicas` and PD's
    /// `max-replicas`, both 3 out of the box (their documented defaults; the scripts change neither).
    pub const REPLICAS: usize = 3;

    impl Op {
        pub fn of(op: usize, nodes: usize) -> Self {
            match op % 3 {
                0 => Self::Split,
                2 if nodes > REPLICAS => Self::Replica,
                _ => Self::Leader,
            }
        }
    }

    /// A replica move's target set: `replicas` with the first one that isn't `leader` swapped for
    /// the first store of `stores` that holds none.
    pub fn moved(replicas: &[i64], leader: i64, stores: &[i64]) -> Option<(i64, i64, Vec<i64>)> {
        let from = *replicas.iter().find(|&&r| r != leader)?;
        let to = *stores.iter().find(|s| !replicas.contains(s))?;
        Some((
            from,
            to,
            replicas
                .iter()
                .map(|&r| if r == from { to } else { r })
                .collect(),
        ))
    }

    impl Containers {
        fn reshard_cockroach(&self, what: Op, session: &str, op: usize) -> anyhow::Result<()> {
            let mut c = self
                .endpoint(0)?
                .parse::<postgres::Config>()?
                .connect_timeout(self.deadline)
                .connect(postgres::NoTls)?;
            let seq = i64::try_from(op)?.saturating_add(1);
            if what == Op::Split {
                c.batch_execute(&format!(
                    "ALTER TABLE events SPLIT AT VALUES ('{session}', {seq})"
                ))?;
                return Ok(());
            }
            let row = c.query_one(&format!("SELECT lease_holder, replicas FROM [SHOW RANGE FROM TABLE events FOR ROW ('{session}', 0)]"), &[])?;
            let (leader, replicas): (i64, Vec<i64>) = (row.try_get(0)?, row.try_get(1)?);
            if what == Op::Leader {
                let i = replicas.iter().position(|&r| r == leader).unwrap_or(0);
                let to = replicas
                    .get((i + 1) % replicas.len().max(1))
                    .copied()
                    .ok_or_else(|| anyhow::anyhow!("the range of {session} names no replicas"))?;
                c.batch_execute(&format!(
                    "ALTER TABLE events EXPERIMENTAL_RELOCATE LEASE VALUES ({to}, '{session}', 0)"
                ))?;
                return Ok(());
            }
            let stores: Vec<i64> = c
                .query(
                    "SELECT node_id FROM crdb_internal.gossip_nodes ORDER BY node_id",
                    &[],
                )?
                .iter()
                .map(|r| r.try_get(0))
                .collect::<Result<_, _>>()?;
            let (_, _, set) = moved(&replicas, leader, &stores).ok_or_else(|| {
                anyhow::anyhow!("no node without a copy of the range of {session}")
            })?;
            let set = set
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            c.batch_execute(&format!(
                "ALTER TABLE events EXPERIMENTAL_RELOCATE VALUES (ARRAY[{set}], '{session}', 0)"
            ))?;
            Ok(())
        }

        /// Each of the events table's Regions: its peers' stores and its leader's.
        fn tidb_regions(c: &mut mysql::Conn) -> anyhow::Result<Vec<(u64, Vec<i64>, i64)>> {
            use mysql::prelude::Queryable;
            let peers: Vec<(u64, i64, i64)> = c.query(
                "SELECT p.REGION_ID, p.STORE_ID, p.IS_LEADER FROM information_schema.tikv_region_peers p JOIN information_schema.tikv_region_status s ON p.REGION_ID = s.REGION_ID \
                 WHERE s.DB_NAME = 'test' AND s.TABLE_NAME = 'events' ORDER BY p.REGION_ID, p.STORE_ID",
            )?;
            let mut out: Vec<(u64, Vec<i64>, i64)> = Vec::new();
            for (region, store, leader) in peers {
                if out.last().is_none_or(|(r, _, _)| *r != region) {
                    out.push((region, Vec::new(), 0));
                }
                if let Some((_, stores, l)) = out.last_mut() {
                    stores.push(store);
                    if leader != 0 {
                        *l = store;
                    }
                }
            }
            Ok(out)
        }

        fn reshard_tidb(&self, what: Op, session: &str, op: usize, o: &Opts) -> anyhow::Result<()> {
            use mysql::prelude::Queryable;
            let mut c = mysql::Conn::new(mysql::Opts::from_url(self.endpoint(0)?)?)?;
            if what == Op::Split {
                c.query_drop(format!(
                    "SPLIT TABLE events BY ('{session}', {})",
                    op.saturating_add(1)
                ))?;
                return Ok(());
            }
            let regions = Self::tidb_regions(&mut c)?;
            anyhow::ensure!(!regions.is_empty(), "the events table has no Regions");
            let (region, replicas, leader) = regions
                .get((op / 3) % regions.len())
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no Region"))?;
            let done: Box<dyn Fn(&[i64], i64) -> bool> = if what == Op::Leader {
                let to = *replicas
                    .iter()
                    .find(|&&r| r != leader)
                    .ok_or_else(|| anyhow::anyhow!("Region {region} has one peer"))?;
                self.script(&[
                    "exec",
                    "0",
                    "operator",
                    "add",
                    "transfer-leader",
                    &region.to_string(),
                    &to.to_string(),
                ])?;
                Box::new(move |_, l| l == to)
            } else {
                let stores: Vec<i64> = c.query(
                    "SELECT STORE_ID FROM information_schema.tikv_store_status ORDER BY STORE_ID",
                )?;
                let (from, to, _) = moved(&replicas, leader, &stores)
                    .ok_or_else(|| anyhow::anyhow!("no store without a peer of Region {region}"))?;
                self.script(&[
                    "exec",
                    "0",
                    "operator",
                    "add",
                    "transfer-peer",
                    &region.to_string(),
                    &from.to_string(),
                    &to.to_string(),
                ])?;
                Box::new(move |peers, _| peers.contains(&to) && !peers.contains(&from))
            };
            // PD's operators run in the background: the operation has taken effect once the Region shows it.
            let until = Instant::now() + o.deadline;
            loop {
                let now = Self::tidb_regions(&mut c)?;
                if now
                    .iter()
                    .any(|(r, peers, l)| *r == region && done(peers, *l))
                {
                    return Ok(());
                }
                anyhow::ensure!(
                    Instant::now() < until,
                    "Region {region}'s {what:?} operation didn't take effect within --deadline-s"
                );
                std::thread::park_timeout(need(o.poll, "--poll-us", &o.measure)?);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    // G13 step 1: every size and duration is an input; a missing one is refused, naming it.
    #[test]
    fn g13_scale_inputs_are_required_not_picked() {
        let base = "--scale write --system cockroach --nodes 3 --groups 3 --writers 2 --deadline-s 5 --hosts local=127.0.0.1";
        let e = parse(&args(base)).err().unwrap().to_string();
        assert!(e.contains("--batch"), "{e}");
        assert!(
            parse(&args(&format!(
                "{base} --batch 1 --text-bytes 10 --warmup-s 1 --seconds 2"
            )))
            .is_ok()
        );
        let e = parse(&args(
            "--scale write --system tidb --groups 3 --writers 2 --deadline-s 5",
        ))
        .err()
        .unwrap()
        .to_string();
        assert!(e.contains("--nodes"), "{e}");
        let e = parse(&args("--scale region --system tidb --nodes 3 --groups 3 --writers 2 --deadline-s 5 --batch 1 --text-bytes 1 --warmup-s 1 --seconds 1")).err().unwrap().to_string();
        assert!(e.contains("--between-ms"), "{e}");
        let e = parse(&args("--scale xgroup --system amphora --nodes 3 --groups 3 --writers 2 --deadline-s 5 --keys 30 --span 2 --warmup-s 1 --seconds 1")).err().unwrap().to_string();
        assert!(e.contains("--protocol"), "{e}");
        let e = parse(&args("--scale regionloss --system cockroach --nodes 3 --groups 3 --writers 2 --deadline-s 5 --batch 1 --text-bytes 1 --lose 3 --settle-s 1 --after-s 1 --poll-us 10")).err().unwrap().to_string();
        assert!(e.contains("--lose"), "{e}");
        let e = parse(&args("--scale xgroup --system tidb --nodes 3 --groups 2 --writers 2 --deadline-s 5 --keys 30 --span 3 --warmup-s 1 --seconds 1")).err().unwrap().to_string();
        assert!(e.contains("--span"), "{e}");
    }

    // G13 step 1: the competitors split where Amphora's groups divide, and a key's group is the
    // run it falls in.
    #[test]
    fn g13_scale_splits_fall_at_the_same_points() {
        assert_eq!(session_splits(6, 3), vec!["s-000002", "s-000004"]);
        assert_eq!(
            session_splits(2, 4),
            vec!["s-000001"],
            "never more splits than sessions"
        );
        assert!(session_splits(5, 1).is_empty());
        assert_eq!(key_splits(30, 3), vec![10, 20]);
        assert_eq!(
            (
                key_group(0, 30, 3),
                key_group(9, 30, 3),
                key_group(10, 30, 3),
                key_group(29, 30, 3)
            ),
            (0, 0, 1, 2)
        );
    }

    // G13 step 1, G14 step 1: the same tables in each dialect; the region measure homes rows by
    // region (CockroachDB's REGIONAL BY ROW, TiDB's partition a placement policy each).
    #[test]
    fn g13_scale_setup_dialects() {
        let o = parse(&args("--scale region --system cockroach --nodes 3 --groups 2 --writers 4 --deadline-s 5 --batch 1 --text-bytes 1 --warmup-s 1 --seconds 1 --between-ms 30 --poll-us 1000 --hosts local=127.0.0.1")).unwrap();
        let c = sql::setup(sql::Kind::Cockroach, &o);
        assert!(
            c.iter()
                .any(|s| s == "ALTER TABLE events SPLIT AT VALUES ('s-000002')"),
            "{c:?}"
        );
        assert!(c.iter().any(|s| s.contains("ADD REGION \"r2\"")), "{c:?}");
        assert!(c.iter().any(|s| s.contains("REGIONAL BY ROW")), "{c:?}");
        let t = sql::setup(sql::Kind::Tidb, &o);
        assert!(
            t.iter()
                .any(|s| s == "SPLIT TABLE events BY ('s-000002', 0)"),
            "{t:?}"
        );
        assert_eq!(
            t.iter()
                .filter(|s| s.starts_with("CREATE PLACEMENT POLICY"))
                .count(),
            3
        );
        assert!(
            t.iter()
                .any(|s| s
                    .contains("PARTITION p2 VALUES IN ('r2') PLACEMENT POLICY = amphora_bench_r2")),
            "{t:?}"
        );
    }

    // G13 step 1: --hosts is required for the competitors and takes ENDPOINT=ADDR pairs; cutover
    // takes its operations' count.
    #[test]
    fn g13_scale_machines_are_arguments() {
        let base = "--scale cutover --system tidb --nodes 3 --groups 3 --writers 2 --deadline-s 5 --batch 1 --text-bytes 1 --ops 4 --settle-s 1 --poll-us 1000";
        let e = parse(&args(base)).err().unwrap().to_string();
        assert!(e.contains("--hosts"), "{e}");
        let e = parse(&args(&format!("{base} --hosts local")))
            .err()
            .unwrap()
            .to_string();
        assert!(e.contains("ENDPOINT=ADDR"), "{e}");
        let o = parse(&args(&format!(
            "{base} --hosts local=10.0.0.1,ssh://u@b=10.0.0.2"
        )))
        .unwrap();
        assert_eq!(
            o.hosts.as_deref(),
            Some("local=10.0.0.1,ssh://u@b=10.0.0.2")
        );
        assert_eq!(
            record_head(&o)["machines"],
            serde_json::json!(["10.0.0.1", "10.0.0.2"])
        );
        let e = parse(&args("--scale cutover --system cockroach --nodes 3 --groups 3 --writers 2 --deadline-s 5 --batch 1 --text-bytes 1 --settle-s 1 --poll-us 1 --hosts local=a")).err().unwrap().to_string();
        assert!(e.contains("--ops"), "{e}");
    }

    // G13 step 2: a write counts toward every operation it was in flight during, and only those.
    #[test]
    fn g13_cutover_write_overlaps_the_operations_in_flight() {
        assert_eq!(overlapped(0, 0, 5), 0..0, "before any operation");
        assert_eq!(
            overlapped(0, 1, 5),
            0..1,
            "acknowledged while the first ran"
        );
        assert_eq!(
            overlapped(1, 3, 5),
            1..3,
            "began after the first ended, outlived the third's start"
        );
        assert_eq!(overlapped(5, 5, 5), 5..5, "after the last");
        assert_eq!(overlapped(7, 9, 5), 5..5, "never past --ops");
    }

    // G13 steps 1 and 2: operations cycle split, leader move, replica move; a replica move swaps one
    // follower for a node holding no copy, and falls back to a leader move with no such node.
    #[test]
    fn g13_cutover_operations_cycle() {
        use sql::{Op, moved};
        assert_eq!(
            (0..6).map(|k| Op::of(k, 5)).collect::<Vec<_>>(),
            vec![
                Op::Split,
                Op::Leader,
                Op::Replica,
                Op::Split,
                Op::Leader,
                Op::Replica
            ]
        );
        assert_eq!(
            Op::of(2, 3),
            Op::Leader,
            "three nodes: every node holds a copy"
        );
        assert_eq!(
            moved(&[1, 2, 3], 1, &[1, 2, 3, 4, 5]),
            Some((2, 4, vec![1, 4, 3]))
        );
        assert_eq!(
            moved(&[1, 2, 3], 2, &[1, 2, 3, 4]),
            Some((1, 4, vec![4, 2, 3]))
        );
        assert_eq!(moved(&[1, 2, 3], 1, &[1, 2, 3]), None);
    }

    // G14 step 2: a writer's rows are numbered from 1 in one session, homed in its region, so a
    // resend is the same rows and everything up to the last acknowledged one was acknowledged.
    #[test]
    fn g14_scale_writer_numbers_its_rows() {
        let mut w = Writer::new(4, 1);
        let a = w.next(2, 10);
        let b = w.next(1, 10);
        assert_eq!(
            a.iter().chain(&b).map(|r| r.seq).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert!(
            a.iter()
                .chain(&b)
                .all(|r| r.session == "s-000004" && r.region == 1)
        );
    }
}
