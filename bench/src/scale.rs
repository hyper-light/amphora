//! Scale-out and regions measured (CP-T1.9 to CP-T1.11; kiln plans "G13 Scale-out", steps 1 and 5,
//! and "G14 Regions", steps 1 and 2): Amphora, CockroachDB and TiDB on the same workloads, each a
//! cluster of `--nodes` nodes, its data in `--groups` groups (Amphora's replicated groups; the
//! others' ranges or regions, split at the same points).
//!
//! - `write` (the scale-out write class): `--writers` closed-loop writers, each its own session of
//!   `--batch` rows a write, through node `writer % nodes`, for `--seconds` after `--warmup-s`.
//! - `read` (the scale-out read class): `--rows` rows written first, then `--writers` closed-loop
//!   readers of one row each by its key, answered by its group's leader (leaseholder). Before they
//!   start, one read of the last row preloaded ([`Session::read_after`]): it waits by the system's
//!   own control for that write where the system has one (Amphora's session token from the write's
//!   answer), else it is one read at the system's strongest consistency; never asked again.
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
//!   `--after-s`, the region is started again, and its own copy is read once for each writer's
//!   acknowledged rows ([`Session::count_own_after`]), each read waiting on that writer's last
//!   acknowledged write by the system's own control: the time to serve again. A system with no read
//!   that waits for a write on a node's own copy has that figure refused, typed and said why, and
//!   the rest of the measure stands. Every writer resends a refused write
//!   unchanged, so every row up to its last acknowledged one was acknowledged; `lost` counts those
//!   missing, read through the leader, and must be 0.
//! - `cutover` (G13 steps 1 and 2; AC-T1.9.2, AC-T1.9.3): writers run; after `--settle-s`, `--ops`
//!   range operations one after another, cycling split, leader move and (when a node holds no copy
//!   of the range) replica move: CockroachDB's `SPLIT AT` and `EXPERIMENTAL_RELOCATE`, TiDB's
//!   `SPLIT TABLE` and PD's `transfer-leader` and `transfer-peer` operators; `--concurrent-ops`
//!   of them at once. Operation `k` works on the range that takes session `(k / 3) % writers`'s new
//!   rows; its moved writers are that session's group (by [`session_splits`]: pass `--groups` equal
//!   to `--writers` for a range a session), the rest are the other ranges. Four figures (G13 step 2):
//!   `unavailable_ms`, each operation's longest gap between two acknowledgements of its moved
//!   writers that overlaps it; `moved_ms` against `baseline_ms`, the latency of the moved writers'
//!   writes that overlapped their operation against every write acknowledged before the first
//!   operation began; `others_ms`, the other writers' writes that overlapped an operation (the
//!   interference); and `stale_routes`, the writes the bench sent again after a refusal
//!   (`resends`, moved and others) beside the system's own count of requests it sent on a stale
//!   route (`system`: CockroachDB's DistSender not-leaseholder and range-key-mismatch errors,
//!   TiDB's TiKV client Region errors, summed over the nodes, the change over the operations).
//!   Beside them stays the upper bound: an operation's pause is the longest acknowledgement of
//!   any write in flight while it ran (every write it overlapped counts); `cutover_ms` summarises
//!   the pauses over the operations. `lost` counts acknowledged rows missing at the end, read
//!   through the leader, and must be 0. Amphora's is refused until the
//!   shell that carries out its splits, merges and moves over replicated groups is built
//!   (docs/design/reshard.md, "What is left").
//!
//! Every system's nodes run on the machines `--hosts` names (`ENDPOINT=ADDR,...`, node `n` on
//! machine `n` mod their count; bench/servers/scale/lib.sh): one a machine for the runs of record,
//! all on one for a lab. Natively by default (each system's own release binaries as processes, data
//! on the machine's disk; Ada's fair comparisons: the same client, OS, disk and filesystem for all),
//! or in containers (`AMPHORA_BENCH_SCALE_MODE=docker`, not Amphora's server). `amphora` is
//! Amphora's own server (`amphora serve`), reached over TCP by its client protocol as the others
//! are by theirs ([`served`]); `amphora-inproc` keeps its groups in this process for the measures
//! the server doesn't serve yet (follower, xgroup, region). Each record says which client and
//! platform produced it (`platform`).
//!
//! Every size, duration and interval is an input with no default: nothing is picked here. A system
//! without a measure's concept, or whose cluster can't be brought up here, is refused, typed (the
//! same record as `repl.rs`'s: `"refused": "no-concept" | "unavailable"`, with why).
//!
//! TiDB's placement (the region measure) and PD's operators (cutover's leader and replica moves) are
//! applied in the background, and PD offers no watch or long-poll for Region or placement changes:
//! those waits read PD's state every `schedule.patrol-region-interval`, PD's own period for checking
//! each Region against its rules, read from the cluster (`SHOW CONFIG`), never a picked interval;
//! each record says so (`waits`; lane scaleout-harness-11).
//!
//! `amphora-inproc`'s groups run in this process, each `--nodes` members relayed as `repl.rs` relays
//! them (member `n` of every group is node `n`, in region `n`). The others are brought up through
//! `bench/servers/scale/<system>.sh up N|down|kill N|start N|endpoint N|delay MS|rmi`.
//!
//! usage: amphora-bench --scale <write|read|follower|xgroup|region|regionloss|cutover>
//!        --system <amphora|amphora-inproc|cockroach|tidb> --nodes N --groups N --writers N --deadline-s N [--record FILE]
//!        write, region: --batch N --text-bytes N --warmup-s N --seconds N (region: --between-ms N)
//!        read, follower: --rows N --batch N --text-bytes N --warmup-s N --seconds N
//!        xgroup:     --keys N --span N --warmup-s N --seconds N (amphora: --protocol 2pc|parallel)
//!        regionloss: --batch N --text-bytes N --lose N --settle-s N --after-s N
//!        cutover:    --batch N --text-bytes N --ops N --concurrent-ops N --settle-s N
//!        amphora:    --class persisted|replicated [--spanning N]
//!        amphora-inproc: --dir DIR --class persisted|replicated [--spanning N] --relay-records N --relay-pending N
//!        all but amphora-inproc: --hosts ENDPOINT=ADDR,... [--scripts DIR (bench/servers/scale)] [--shell PROGRAM (bash)]

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
///
/// Every system is sent the same work the same way (Ada's fair comparisons): each write, read and
/// transaction is one SQL statement, sent as text with its values written in as literals
/// ([`Dialect`]), answered in one round trip and committed on its own. Amphora's client protocol
/// takes text only (`exec SQL`, `count SQL`), so CockroachDB gets Postgres's simple query protocol and
/// TiDB MySQL's text protocol (COM_QUERY): each server parses every statement it gets. An explicit
/// transaction with one statement a row, or a statement prepared before each run, would take more
/// round trips on some systems than on others, which would be an error in the experiment.
/// `amphora-inproc` makes in-process calls, and its records say so.
pub trait Session {
    /// `rows` in one statement (so they commit together), into `regional` when `regional`; returns
    /// once acknowledged.
    fn write(&mut self, regional: bool, rows: &[Row]) -> anyhow::Result<()>;
    /// Whether row (`session`, `seq`) is there: read by its leader, or as a follower read.
    fn read(&mut self, session: &str, seq: u64, follower: bool) -> anyhow::Result<bool>;
    /// One statement (one transaction) setting every key to `value`.
    fn add(&mut self, keys: &[i64], value: i64) -> anyhow::Result<()>;
    /// Rows of `session` up to `upto`: on this node's own copy when `own`, else by the leader.
    fn count(&mut self, session: &str, upto: u64, own: bool) -> anyhow::Result<u64>;
    /// [`Session::write`], and the write's [`Mark`]: what a read waits on to see it.
    fn write_marked(&mut self, regional: bool, rows: &[Row]) -> anyhow::Result<Mark> {
        self.write(regional, rows).map(|()| None)
    }
    /// The read measures' warm-up: one read of row (`session`, `seq`), written with `mark`, as
    /// [`Session::read`] reads it. Where the system has a control that makes a read wait for a
    /// given write, the read waits on it; else it is the system's read as it is (the leader's,
    /// the strongest consistency the bench asks). It is asked once, never again.
    fn read_after(
        &mut self,
        session: &str,
        seq: u64,
        follower: bool,
        _mark: Mark,
    ) -> anyhow::Result<bool> {
        self.read(session, seq, follower)
    }
    /// Rows of `session` up to `upto` on this node's own copy, once it holds the write `mark`
    /// names: one read that waits by the system's own control, never a poll. By default the
    /// system's own-copy count as it is, for systems whose reads are current wherever they're
    /// asked (CockroachDB's go to the leaseholder; TiDB's follower reads are ReadIndex reads).
    fn count_own_after(&mut self, session: &str, upto: u64, _mark: Mark) -> anyhow::Result<u64> {
        self.count(session, upto, true)
    }
}

/// What a read waits on to see a given write: Amphora's session token from the write's answer
/// (replication.md, "Decision (lane ryw-1): read-your-writes by session tokens"), None for a
/// system that gives a client no such mark.
pub type Mark = Option<amphora::database::Token>;

/// `f` on each node in the configured order, from node 0, until one succeeds (schema setup's
/// reach, lane scaleout-harness-10): the next node is tried only once the one before has failed,
/// each try bounded by --deadline-s through its own client, so no node is asked again at once. Given
/// up at `until`, or once every node failed, naming each failure.
fn first_reachable<T>(
    nodes: usize,
    until: Instant,
    mut f: impl FnMut(usize) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let mut failed = Vec::new();
    for n in 0..nodes {
        if Instant::now() >= until {
            failed.push(format!("--deadline-s passed before node {n}"));
            break;
        }
        match f(n) {
            Ok(v) => return Ok(v),
            Err(e) => failed.push(format!("node {n}: {e:#}")),
        }
    }
    anyhow::bail!("no node answered: {}", failed.join("; "))
}

/// A measure's first failure, and the phases that end on it (lane scaleout-harness-10): a phase
/// (`--settle-s`, `--after-s`) lasts its length, an input and the one wait here that is about
/// time, or ends as soon as a worker fails, woken by that failure. A condition variable's wait for
/// a duration, so a spurious wake doesn't cut a phase short as a bare park_timeout's would.
#[derive(Default)]
struct Halt {
    failed: Mutex<Option<anyhow::Error>>,
    told: std::sync::Condvar,
}

impl Halt {
    /// Keeps the first failure and wakes the phase waiting.
    fn fail(&self, e: anyhow::Error) {
        if let Ok(mut f) = self.failed.lock() {
            f.get_or_insert(e);
            self.told.notify_all();
        }
    }
    /// Whether a worker failed (a poisoned slot counts as one).
    fn failed(&self) -> bool {
        self.failed.lock().map_or(true, |f| f.is_some())
    }
    /// Waits `d`, or until a worker fails: whether the phase ran its length with none failed.
    fn hold(&self, d: Duration) -> bool {
        let Ok(f) = self.failed.lock() else {
            return false;
        };
        self.told
            .wait_timeout_while(f, d, |f| f.is_none())
            .is_ok_and(|(f, _)| f.is_none())
    }
    /// The first failure, if any: the measure's error.
    fn take(self) -> anyhow::Result<()> {
        match self
            .failed
            .into_inner()
            .map_err(|_| anyhow::anyhow!("a failure was poisoned"))?
        {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
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
    /// The system's own count, summed over its nodes, of requests it sent on a stale route.
    fn stale_routes(&self) -> anyhow::Result<u64>;
    /// What the measure's waits on the system's background work woke on, for the record, when
    /// any read its state at the system's own period (TiDB's: PD has no watch).
    fn waits(&self) -> Option<serde_json::Value> {
        None
    }
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
    pub protocol: Option<String>,
    pub dir: Option<PathBuf>,
    pub class: Option<String>,
    pub spanning: Option<u32>,
    pub relay_records: Option<usize>,
    pub relay_pending: Option<usize>,
    pub ops: Option<usize>,
    pub concurrent_ops: Option<usize>,
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
        protocol: None,
        dir: None,
        class: None,
        spanning: None,
        relay_records: None,
        relay_pending: None,
        ops: None,
        concurrent_ops: None,
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
            "--protocol" => o.protocol = Some(v()?),
            "--dir" => o.dir = Some(PathBuf::from(v()?)),
            "--class" => o.class = Some(v()?),
            "--spanning" => o.spanning = Some(v()?.parse()?),
            "--relay-records" => o.relay_records = Some(v()?.parse()?),
            "--relay-pending" => o.relay_pending = Some(v()?.parse()?),
            "--ops" => o.ops = Some(v()?.parse()?),
            "--concurrent-ops" => o.concurrent_ops = Some(v()?.parse()?),
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
            if o.system == "amphora-inproc" {
                need(o.protocol.as_deref(), "--protocol", &m)?;
            }
        }
        "region" => {
            rows(&o)?;
            timed(&o)?;
            need(o.between, "--between-ms", &m)?;
        }
        "cutover" => {
            rows(&o)?;
            anyhow::ensure!(need(o.ops, "--ops", &m)? > 0, "--ops takes at least 1");
            anyhow::ensure!(
                need(o.concurrent_ops, "--concurrent-ops", &m)? > 0,
                "--concurrent-ops takes at least 1"
            );
            need(o.settle, "--settle-s", &m)?;
        }
        _ => {
            rows(&o)?;
            anyhow::ensure!(
                need(o.lose, "--lose", &m)? < o.nodes,
                "--lose names a region, 0 to --nodes less one"
            );
            need(o.settle, "--settle-s", &m)?;
            need(o.after, "--after-s", &m)?;
        }
    }
    if o.system != "amphora-inproc" {
        let hosts = need(o.hosts.as_deref(), "--hosts", &m)?;
        anyhow::ensure!(
            hosts.split(',').all(|h| h
                .split_once('=')
                .is_some_and(|(e, a)| !e.is_empty() && !a.is_empty())),
            "--hosts takes ENDPOINT=ADDR,...: each machine's Docker endpoint (local, ssh://..., tcp://...) and the address it's reached by"
        );
    }
    if o.system == "amphora" {
        match need(o.class.as_deref(), "--class", &m)? {
            // Amphora's server refuses a member config naming both (serve::Config::parse).
            "persisted" => {
                anyhow::ensure!(o.spanning.is_none(), "--spanning is the replicated class's")
            }
            "replicated" => anyhow::ensure!(
                need(o.spanning, "--spanning", &m)? >= 2,
                "--spanning takes at least 2"
            ),
            other => anyhow::bail!("unknown class {other} (persisted, replicated)"),
        }
    }
    if o.system == "amphora-inproc" {
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
            // Amphora's server refuses a member config naming both (serve::Config::parse).
            "persisted" => {
                anyhow::ensure!(o.spanning.is_none(), "--spanning is the replicated class's")
            }
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

/// The SQL text each system over the network is sent ([`Session`]): one statement for each write,
/// read or transaction, with its values as literals.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dialect {
    /// Amphora's server (SQLite's SQL) and CockroachDB: standard SQL's string literals.
    Standard,
    /// TiDB (MySQL's SQL).
    Mysql,
}

impl Dialect {
    /// `v` as a string literal. Standard SQL doubles `'`. MySQL also reads `\` in a string as an
    /// escape, unless the server's sql_mode has NO_BACKSLASH_ESCAPES (TiDB's default doesn't), so
    /// `\` is doubled there too.
    pub fn lit(self, v: &str) -> String {
        let v = v.replace('\'', "''");
        match self {
            Self::Standard => format!("'{v}'"),
            Self::Mysql => format!("'{}'", v.replace('\\', "\\\\")),
        }
    }

    /// The upsert of every row in `rows` as one statement: `events(session, seq, body)`, or, when
    /// `regional`, the region measure's table (TiDB's names each row's region; CockroachDB's
    /// REGIONAL BY ROW homes it at the gateway).
    pub fn upsert(self, regional: bool, rows: &[Row]) -> String {
        let table = if regional { "regional" } else { "events" };
        let values = rows
            .iter()
            .map(|r| match (self, regional) {
                (Self::Mysql, true) => format!(
                    "({}, {}, {}, {})",
                    self.lit(&format!("r{}", r.region)),
                    self.lit(&r.session),
                    r.seq,
                    self.lit(&r.body)
                ),
                _ => format!(
                    "({}, {}, {})",
                    self.lit(&r.session),
                    r.seq,
                    self.lit(&r.body)
                ),
            })
            .collect::<Vec<_>>()
            .join(", ");
        match (self, regional) {
            (Self::Mysql, true) => format!(
                "INSERT INTO regional (region, session, seq, body) VALUES {values} ON DUPLICATE KEY UPDATE body = VALUES(body)"
            ),
            (Self::Mysql, false) => format!(
                "INSERT INTO events (session, seq, body) VALUES {values} ON DUPLICATE KEY UPDATE body = VALUES(body)"
            ),
            (Self::Standard, _) => format!(
                "INSERT INTO {table} (session, seq, body) VALUES {values} ON CONFLICT (session, seq) DO UPDATE SET body = excluded.body"
            ),
        }
    }

    /// The cross-group measure's transaction as one statement: every key in `keys` set to `value`.
    pub fn kv_upsert(self, keys: &[i64], value: i64) -> String {
        let values = keys
            .iter()
            .map(|k| format!("({k}, {value})"))
            .collect::<Vec<_>>()
            .join(", ");
        match self {
            Self::Mysql => format!(
                "INSERT INTO kv (k, v) VALUES {values} ON DUPLICATE KEY UPDATE v = VALUES(v)"
            ),
            Self::Standard => format!("UPSERT INTO kv (k, v) VALUES {values}"),
        }
    }

    /// The count of `session`'s rows at `seq` (`upto`: at or below it), `from` naming the table
    /// and how it's read.
    pub fn count(self, from: &str, session: &str, seq: u64, upto: bool) -> String {
        format!(
            "SELECT count(*) FROM {from} WHERE session = {} AND seq {} {seq}",
            self.lit(session),
            if upto { "<=" } else { "=" }
        )
    }
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
        f: impl FnMut(&mut dyn Session) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.counted(deadline, f).map(|(v, _)| v)
    }

    /// [`Conn::run`], and how many times `f` was refused before it succeeded.
    fn counted<T>(
        &mut self,
        deadline: Duration,
        mut f: impl FnMut(&mut dyn Session) -> anyhow::Result<T>,
    ) -> anyhow::Result<(T, u64)> {
        let until = Instant::now() + deadline;
        let mut refused = 0u64;
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
                Ok(v) => return Ok((v, refused)),
                Err(e) => {
                    refused += 1;
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

/// What [`preload`] wrote: each session's last seq, and the last write's session, seq and mark.
struct Preloaded {
    last: Vec<u64>,
    end: (usize, u64, Mark),
}

/// `--rows` rows over the writers' sessions, `--batch` a write.
fn preload(sys: &dyn System, o: &Opts) -> anyhow::Result<Preloaded> {
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
    let mut end = (0, 0, None);
    while left > 0 {
        let n = left.min(batch);
        let wr = writers
            .get_mut(w % o.writers)
            .ok_or_else(|| anyhow::anyhow!("no writer"))?;
        let r = wr.next(n, text);
        let mark = conn.run(o.deadline, |s| s.write_marked(false, &r))?;
        end = (w % o.writers, wr.seq, mark);
        left -= n;
        w += 1;
    }
    Ok(Preloaded {
        last: writers.iter().map(|w| w.seq).collect(),
        end,
    })
}

/// What the read measures' warm-up does on each system (the same rule for all, lane
/// scaleout-harness-10: a system's own control for reading a given write where it has one, else one
/// read at its strongest consistency), for the record.
fn warmup_rule(system: &str, follower: bool) -> &'static str {
    match (system, follower) {
        ("amphora-inproc", false) => {
            "one read on the group's leader carrying the session token of the preload's last write (Database::query_session, ReadMode::Session): it waits until the leader has applied that write"
        }
        ("amphora-inproc", true) => {
            "one follower read at the closed time of the preload's last write, from its session token (Database::query_closed): it waits until the follower's closed time passes that write. Follower reads after it read at Database::follower_read_micros (now less the replica's measured lag), so those asked within that lag of the last write can miss rows: those count in `missed`, as CockroachDB's do"
        }
        ("amphora", _) => {
            "one read through the leader, which answers a write once it has applied it: amphora serve's protocol carries no session token yet, so there is no write to wait on over the wire"
        }
        ("cockroach", false) => {
            "one read, served by the range's leaseholder: CockroachDB's reads are consistent (serializable), so it sees every acknowledged write"
        }
        ("cockroach", true) => {
            "one read by the leaseholder, at the strongest consistency: CockroachDB has no control that makes a follower read wait for a given write (docs.cockroachlabs.com/docs/stable/follower-reads: an AS OF SYSTEM TIME not far enough in the past reads the leaseholder; bounded staleness reads the least stale time served locally without blocking). Follower reads at follower_read_timestamp() can miss rows written within its lag: those count in `missed`"
        }
        (_, false) => {
            "one read by the Region's leader: TiDB's reads are consistent, so it sees every acknowledged write"
        }
        (_, true) => {
            "one follower read (tidb_replica_read = 'follower'): a ReadIndex read, which asks the leader for its commit index and waits until the follower has applied it (docs.pingcap.com/tidb/stable/follower-read), so it sees every acknowledged write"
        }
    }
}

fn read(sys: &dyn System, o: &Opts, follower: bool) -> anyhow::Result<serde_json::Value> {
    let Preloaded {
        last,
        end: (lw, ls, mark),
    } = preload(sys, o)?;
    // Before counting: one read of the last row written, waiting for it by the system's own
    // control where it has one (`warmup_rule` says what each system's does). It is asked once: a
    // miss is the system's answer, and an error (lane scaleout-harness-10).
    let down: Vec<AtomicBool> = (0..sys.nodes()).map(|_| AtomicBool::new(false)).collect();
    let mut conn = Conn::new(sys, &down, 1 % sys.nodes());
    anyhow::ensure!(ls > 0, "nothing preloaded");
    anyhow::ensure!(
        conn.run(o.deadline, |s| s.read_after(
            &session_name(lw),
            ls,
            follower,
            mark
        ))?,
        "the warm-up's read didn't find the last row preloaded ({}, {ls}): {}",
        session_name(lw),
        warmup_rule(&o.system, follower)
    );
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
        m.insert("warmup".into(), warmup_rule(&o.system, follower).into());
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
    let (settle, after) = (
        need(o.settle, "--settle-s", m)?,
        need(o.after, "--after-s", m)?,
    );
    let nodes = sys.nodes();
    let down: Vec<AtomicBool> = (0..nodes).map(|_| AtomicBool::new(false)).collect();
    let stop = AtomicBool::new(false);
    // Each writer's last acknowledged seq and that write's mark, which the region's copy waits on.
    let acked: Vec<Mutex<(u64, Mark)>> = (0..o.writers).map(|_| Mutex::new((0, None))).collect();
    let halt = Halt::default();
    let mut first_ack_ms = 0.0;
    std::thread::scope(|scope| -> anyhow::Result<()> {
        for w in 0..o.writers {
            let (down, stop, acked, halt) = (&down, &stop, &acked, &halt);
            scope.spawn(move || {
                let mut wr = Writer::new(w, w % nodes);
                let mut conn = Conn::new(sys, down, w % nodes);
                while !stop.load(Ordering::Relaxed) {
                    let rows = wr.next(batch, text);
                    match conn.run(o.deadline, |s| s.write_marked(false, &rows)) {
                        Ok(mark) => {
                            if let Some(Ok(mut a)) = acked.get(w).map(Mutex::lock) {
                                *a = (wr.seq, mark);
                            }
                        }
                        Err(e) => {
                            halt.fail(e);
                            return;
                        }
                    }
                }
            });
        }
        let ran = (|| -> anyhow::Result<()> {
            if !halt.hold(settle) {
                return Ok(());
            }
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
            halt.hold(after);
            stop.store(true, Ordering::Relaxed);
            Ok(())
        })();
        stop.store(true, Ordering::Relaxed);
        ran
    })?;
    halt.take()?;
    let acked = acked
        .into_iter()
        .map(|a| a.into_inner())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| anyhow::anyhow!("a writer's acknowledgement was poisoned"))?;
    // The region back (`start` returns once its node is ready): its own copy read once for each
    // writer, each read waiting on the writer's last acknowledged write by the system's own control
    // (Session::count_own_after), never a poll (lane scaleout-harness-10). A system with no such
    // read has this figure refused, typed and said why; the rest of the measure stands.
    let at = Instant::now();
    sys.start(lose)?;
    if let Some(d) = down.get(lose) {
        d.store(false, Ordering::Relaxed);
    }
    let caught = (|| -> anyhow::Result<f64> {
        let mut s = sys.connect(lose)?;
        for (w, (want, mark)) in acked.iter().enumerate() {
            let held = s.count_own_after(&session_name(w), *want, *mark)?;
            anyhow::ensure!(
                held >= *want,
                "region {lose}'s copy held {held} of writer {w}'s {want} acknowledged rows once it had waited for them"
            );
        }
        Ok(us(at) as f64 / 1000.0)
    })();
    let serve_ms = match caught {
        Ok(ms) => serde_json::json!(ms),
        Err(e) => match e.downcast_ref::<Refusal>() {
            Some(Refusal::NoConcept(why)) => {
                serde_json::json!({ "refused": "no-concept", "why": why })
            }
            Some(Refusal::Unavailable(why)) => {
                serde_json::json!({ "refused": "unavailable", "why": why })
            }
            None => return Err(e),
        },
    };
    let mut conn = Conn::new(sys, &down, (lose + 1) % nodes);
    let (mut acknowledged, mut lost) = (0u64, 0u64);
    for (w, (want, _)) in acked.iter().enumerate() {
        let held = conn.run(o.deadline, |s| s.count(&session_name(w), *want, false))?;
        acknowledged += want;
        lost += want.saturating_sub(held);
    }
    Ok(serde_json::json!({
        "writers": o.writers, "batch": batch, "text_bytes": text, "lose": lose, "settle_s": settle.as_secs_f64(), "after_s": after.as_secs_f64(),
        "first_ack_ms": first_ack_ms, "serve_again_ms": serve_ms, "serve_again": serve_rule(&o.system), "acknowledged_rows": acknowledged, "lost": lost,
    }))
}

/// How each system's region-loss measure waits for the region's copy to serve again, for the
/// record (lane scaleout-harness-10).
fn serve_rule(system: &str) -> &'static str {
    match system {
        "amphora-inproc" => {
            "each writer's rows counted on the region's own member at the closed time of the writer's last acknowledged write, from its session token (Database::query_closed): the read waits until the member's closed time passes that write"
        }
        "amphora" => {
            "refused: amphora serve's protocol has no read that waits for a write (no session token on the wire yet), and its count on a member's own copy (query_local) doesn't wait"
        }
        "cockroach" => {
            "each writer's rows counted once through the region's node: CockroachDB reads by the leaseholder, consistent, so one read sees every acknowledged row once the node answers"
        }
        _ => {
            "each writer's rows counted once through the region's tidb-server as a follower read (tidb_replica_read = 'follower', ReadIndex: it waits until the follower has applied the leader's commit index; docs.pingcap.com/tidb/stable/follower-read)"
        }
    }
}

/// The operations a write overlapped: it began after `ended` operations had ended and was
/// acknowledged once `begun` had begun, so it overlapped operations `ended` to `begun` less one.
/// One write of the cutover measure: its writer, when it was sent and acknowledged (microseconds
/// from the measure's start), and how many sends of it were refused first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sent {
    writer: usize,
    t0: u64,
    t1: u64,
    resends: u64,
}

/// One range operation: when it began and took effect (microseconds from the measure's start), and
/// the writers whose new rows its range takes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Moved {
    t0: u64,
    t1: u64,
    writers: Vec<usize>,
}

/// The cutover measure's figures (G13 step 2), all latencies in microseconds.
struct Figures {
    /// Every write acknowledged before the first operation began.
    baseline: Vec<u64>,
    /// The moved writers' writes that overlapped their operation.
    moved: Vec<u64>,
    /// The other writers' writes that overlapped an operation.
    others: Vec<u64>,
    /// Each operation's longest gap between two acknowledgements of its moved writers that overlaps it.
    unavailable: Vec<u64>,
    /// Each operation's upper bound: the longest acknowledgement of any write it overlapped.
    pauses: Vec<u64>,
    moved_resends: u64,
    other_resends: u64,
}

/// The writers of the group session `w` is in, by [`session_splits`].
pub fn group_writers(w: usize, writers: usize, groups: usize) -> Vec<usize> {
    let g = |x: usize| key_group(i64::try_from(x).unwrap_or(0), writers, groups);
    (0..writers).filter(|&x| g(x) == g(w)).collect()
}

fn hist(samples: &[u64]) -> Hist {
    let mut h = Hist::new();
    for &v in samples {
        h.record(v);
    }
    h
}

/// The figures from every write and every operation.
fn figures(writes: &[Sent], ops: &[Moved]) -> Figures {
    let mut f = Figures {
        baseline: Vec::new(),
        moved: Vec::new(),
        others: Vec::new(),
        unavailable: Vec::new(),
        pauses: Vec::new(),
        moved_resends: 0,
        other_resends: 0,
    };
    let first = ops.iter().map(|m| m.t0).min().unwrap_or(u64::MAX);
    let over = |w: &Sent, m: &Moved| w.t0 < m.t1 && w.t1 > m.t0;
    for w in writes {
        let lat = w.t1.saturating_sub(w.t0);
        if w.t1 <= first {
            f.baseline.push(lat);
        }
        let mut overlapping = ops.iter().filter(|m| over(w, m)).peekable();
        if overlapping.peek().is_none() {
            continue;
        }
        if overlapping.any(|m| m.writers.contains(&w.writer)) {
            f.moved.push(lat);
            f.moved_resends += w.resends;
        } else {
            f.others.push(lat);
            f.other_resends += w.resends;
        }
    }
    for m in ops {
        let pause = writes
            .iter()
            .filter(|w| over(w, m))
            .map(|w| w.t1.saturating_sub(w.t0))
            .max()
            .unwrap_or(0);
        f.pauses.push(pause);
        let mut acks: Vec<u64> = writes
            .iter()
            .filter(|w| m.writers.contains(&w.writer))
            .map(|w| w.t1)
            .collect();
        acks.sort_unstable();
        let gap = acks
            .windows(2)
            .filter_map(|p| match p {
                [a, b] if *a < m.t1 && *b > m.t0 => Some(b.saturating_sub(*a)),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        f.unavailable.push(gap);
    }
    f
}

fn cutover(sys: &dyn System, o: &Opts) -> anyhow::Result<serde_json::Value> {
    let m = &o.measure;
    let (batch, text, ops, concurrent, settle) = (
        need(o.batch, "--batch", m)?,
        need(o.text_bytes, "--text-bytes", m)?,
        need(o.ops, "--ops", m)?,
        need(o.concurrent_ops, "--concurrent-ops", m)?,
        need(o.settle, "--settle-s", m)?,
    );
    let nodes = sys.nodes();
    let down: Vec<AtomicBool> = (0..nodes).map(|_| AtomicBool::new(false)).collect();
    let (stop, next) = (AtomicBool::new(false), AtomicUsize::new(0));
    let acked: Vec<AtomicU64> = (0..o.writers).map(|_| AtomicU64::new(0)).collect();
    // Each writer's writes and each operation's span: held to the end, one entry a write (the run
    // is bounded by --ops and --deadline-s, so they are too).
    let sent: Vec<Mutex<Vec<Sent>>> = (0..o.writers).map(|_| Mutex::new(Vec::new())).collect();
    let spans: Mutex<Vec<Moved>> = Mutex::new(Vec::new());
    let halt = Halt::default();
    let fail = |e: anyhow::Error| halt.fail(e);
    let start = Instant::now();
    let at = || u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);
    let stale_before = sys.stale_routes()?;
    std::thread::scope(|scope| -> anyhow::Result<()> {
        for w in 0..o.writers {
            let (down, stop, acked, sent, fail, at) = (&down, &stop, &acked, &sent, &fail, &at);
            scope.spawn(move || {
                let mut wr = Writer::new(w, w % nodes);
                let mut conn = Conn::new(sys, down, w % nodes);
                let mut mine = Vec::new();
                while !stop.load(Ordering::Relaxed) {
                    let rows = wr.next(batch, text);
                    let t0 = at();
                    match conn.counted(o.deadline, |s| s.write(false, &rows)) {
                        Ok(((), resends)) => {
                            mine.push(Sent {
                                writer: w,
                                t0,
                                t1: at(),
                                resends,
                            });
                            if let Some(a) = acked.get(w) {
                                a.store(wr.seq, Ordering::Relaxed);
                            }
                        }
                        Err(e) => {
                            fail(e);
                            break;
                        }
                    }
                }
                if let Some(Ok(mut s)) = sent.get(w).map(Mutex::lock) {
                    *s = mine;
                }
            });
        }
        halt.hold(settle);
        let runners: Vec<_> = (0..concurrent)
            .map(|_| {
                let (next, spans, halt, fail, at) = (&next, &spans, &halt, &fail, &at);
                scope.spawn(move || {
                    loop {
                        if halt.failed() {
                            return;
                        }
                        let k = next.fetch_add(1, Ordering::SeqCst);
                        if k >= ops {
                            return;
                        }
                        let t0 = at();
                        if let Err(e) = sys.reshard(k, o) {
                            fail(e);
                            return;
                        }
                        let writers = group_writers((k / 3) % o.writers, o.writers, o.groups);
                        if let Ok(mut s) = spans.lock() {
                            s.push(Moved {
                                t0,
                                t1: at(),
                                writers,
                            });
                        }
                    }
                })
            })
            .collect();
        for r in runners {
            r.join()
                .map_err(|_| anyhow::anyhow!("a range operation panicked"))?;
        }
        stop.store(true, Ordering::Relaxed);
        Ok(())
    })?;
    halt.take()?;
    let stale = sys.stale_routes()?.saturating_sub(stale_before);
    let mut writes = Vec::new();
    for s in &sent {
        writes.extend(
            s.lock()
                .map_err(|_| anyhow::anyhow!("a writer's writes were poisoned"))?
                .iter()
                .copied(),
        );
    }
    let spans = spans
        .into_inner()
        .map_err(|_| anyhow::anyhow!("the operations' spans were poisoned"))?;
    let mut took = Hist::new();
    for s in &spans {
        took.record(s.t1.saturating_sub(s.t0));
    }
    let f = figures(&writes, &spans);
    let mut conn = Conn::new(sys, &down, 0);
    let (mut acknowledged, mut lost) = (0u64, 0u64);
    for (w, a) in acked.iter().enumerate() {
        let want = a.load(Ordering::Relaxed);
        let held = conn.run(o.deadline, |s| s.count(&session_name(w), want, false))?;
        acknowledged += want;
        lost += want.saturating_sub(held);
    }
    Ok(serde_json::json!({
        "writers": o.writers, "batch": batch, "text_bytes": text, "ops": ops, "concurrent_ops": concurrent, "settle_s": settle.as_secs_f64(),
        "unavailable_ms": summary(hist(&f.unavailable)), "moved_ms": summary(hist(&f.moved)), "baseline_ms": summary(hist(&f.baseline)),
        "others_ms": summary(hist(&f.others)),
        "stale_routes": { "resends": { "moved": f.moved_resends, "others": f.other_resends }, "system": stale },
        "cutover_ms": summary(hist(&f.pauses)), "op_ms": summary(took), "acknowledged_rows": acknowledged, "lost": lost,
    }))
}

fn record_head(o: &Opts) -> serde_json::Value {
    // bash doesn't export HOSTNAME to its children: the kernel's name is the fallback.
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .or_else(|| {
            std::fs::read_to_string("/proc/sys/kernel/hostname")
                .ok()
                .map(|h| h.trim().to_owned())
        })
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
    // Which client and platform produced the record (Ada's fair comparisons): the OS, how the
    // system ran (natively as its own processes, in containers, or in the bench's process) and the
    // wire client the bench drove it through.
    let mode = std::env::var("AMPHORA_BENCH_SCALE_MODE").unwrap_or_else(|_| "native".into());
    let (ran, client) = match o.system.as_str() {
        "amphora" => (
            "native: amphora serve, one process a member".to_owned(),
            "Amphora's client protocol over TCP: amphora::serve::Client, the server's own",
        ),
        "amphora-inproc" => ("in the bench's process".to_owned(), "in-process calls"),
        "cockroach" => (
            format!("{mode}: cockroach start, one process a node"),
            "Postgres wire, simple query protocol: the bench's own client (bench/src/pgtext.rs), over std's TcpStream",
        ),
        _ => (
            format!("{mode}: pd-server, tikv-server, tidb-server a node"),
            "MySQL wire, text protocol (COM_QUERY): rust-mysql-simple (mysql crate)",
        ),
    };
    let os = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|t| {
            t.lines().find_map(|l| {
                l.strip_prefix("PRETTY_NAME=")
                    .map(|v| v.trim_matches('"').to_owned())
            })
        })
        .unwrap_or_else(|| std::env::consts::OS.to_owned());
    let kernel = std::process::Command::new("uname")
        .arg("-r")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_owned());
    // How each request is sent ([`Session`]): no system over a wire is given prepared statements,
    // as Amphora's protocol is text only (lane scaleout-harness-9).
    let statements = if o.system == "amphora-inproc" {
        "in-process calls, each statement prepared and its parameters bound (no wire)"
    } else {
        "one SQL statement a write, read or transaction, as text with literals: one round trip, committed on its own; no prepared statements, on any system"
    };
    // How each system routes a write: forwarded by the node it was sent to, or sent straight to
    // the leader. Each is how that system is built to be used, stated so the paths are compared
    // as they are (lane scaleout-harness-9).
    let routes = match o.system.as_str() {
        "amphora" => {
            "straight to the leader: the session asks the member it was given which member leads, then sends each write on its own link to the leader (asked again after a refusal)"
        }
        "amphora-inproc" => "straight to the leader: each write is a call on the group's leader",
        "cockroach" => {
            "forwarded: each write goes to the node the session connected to (its gateway), which sends it on to the range's leaseholder"
        }
        _ => {
            "forwarded: each write goes to the tidb-server the session connected to, which sends it on to the Region's leader on TiKV"
        }
    };
    // What gives a request up: --deadline-s for every system, through its client's own bound.
    let deadline = match o.system.as_str() {
        "amphora" => "--deadline-s: amphora::serve::Client's deadline on each request",
        "amphora-inproc" => "--deadline-s: each call's deadline",
        "cockroach" => "--deadline-s: the socket's read and write timeouts",
        _ => "--deadline-s: rust-mysql-simple's read and write timeouts",
    };
    if let Some(h) = head.as_object_mut() {
        h.insert(
            "platform".into(),
            serde_json::json!({ "os": os, "kernel": kernel, "arch": std::env::consts::ARCH, "ran": ran, "client": client, "driver": "amphora-bench --scale: closed-loop writers, Instant-timed per acknowledgement", "statements": statements, "routes": routes, "deadline": deadline }),
        );
        if let Ok(b) = std::env::var("AMPHORA_BENCH_SCALE_BIN") {
            h.insert("binaries".into(), b.into());
        }
    }
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
    use std::io::Write;
    let line = head.to_string();
    writeln!(std::io::stdout().lock(), "{line}")?;
    if let Some(path) = &o.record {
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
        "amphora" => Box::new(served::Amphora::up(o)?),
        "amphora-inproc" => Box::new(inproc::Amphora::open(o)?),
        "cockroach" | "tidb" => Box::new(sql::Containers::up(o)?),
        other => anyhow::bail!("unknown system {other} (amphora, amphora-inproc, cockroach, tidb)"),
    })
}

pub fn main() -> anyhow::Result<()> {
    let args = std::env::args_os()
        .skip(1)
        .map(|a| {
            a.into_string()
                .map_err(|a| anyhow::anyhow!("argument {a:?} isn't Unicode"))
        })
        .collect::<anyhow::Result<Vec<String>>>()?;
    let o = parse(&args)?;
    let ran = open(&o).and_then(|sys| {
        sys.prepare(&o)?;
        let mut body = match o.measure.as_str() {
            "write" => write(sys.as_ref(), &o, false),
            "read" => read(sys.as_ref(), &o, false),
            "follower" => read(sys.as_ref(), &o, true),
            "xgroup" => xgroup(sys.as_ref(), &o),
            "region" => write(sys.as_ref(), &o, true),
            "cutover" => cutover(sys.as_ref(), &o),
            _ => region_loss(sys.as_ref(), &o),
        }?;
        if let (Some(b), Some(w)) = (body.as_object_mut(), sys.waits()) {
            b.insert("waits".into(), w);
        }
        Ok(body)
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

    use amphora::database::{AnswerMax, Database, ReadMode};
    use amphora::xgroup;
    use amphora_api::Value;
    use amphora_api::replication::{Durability, Fates, ReplicaConfig};

    struct Group {
        members: Vec<Database>,
        bell: Arc<Bell>,
        relay: Option<std::thread::JoinHandle<()>>,
    }

    /// The relay's one wake across a group's members: each member's listener rings it when a
    /// record joins that member's outgoing queue or its owner ends (`ReplicaHandle::listen`), and
    /// the group's drop stops it. A ring given before the relay waits is kept, so none is missed
    /// between its last drain and its wait.
    #[derive(Default)]
    struct Bell {
        rung: Mutex<Rung>,
        ringing: std::sync::Condvar,
    }
    #[derive(Default)]
    struct Rung {
        rung: bool,
        stopped: bool,
    }
    impl Bell {
        fn ring(&self) {
            if let Ok(mut r) = self.rung.lock() {
                r.rung = true;
                self.ringing.notify_one();
            }
        }
        fn stop(&self) {
            if let Ok(mut r) = self.rung.lock() {
                r.stopped = true;
                self.ringing.notify_one();
            }
        }
        /// Whether the relay goes on: not stopped, and the bell not poisoned.
        fn going(&self) -> bool {
            self.rung.lock().is_ok_and(|r| !r.stopped)
        }
        /// Waits for a ring or the stop, and no later than `due` when it's given, then takes the
        /// ring: whether the relay goes on.
        fn wait(&self, due: Option<Instant>) -> bool {
            let Ok(r) = self.rung.lock() else {
                return false;
            };
            let idle = |r: &mut Rung| !r.rung && !r.stopped;
            let r = match due {
                None => self.ringing.wait_while(r, idle).ok(),
                Some(due) => self
                    .ringing
                    .wait_timeout_while(r, due.saturating_duration_since(Instant::now()), idle)
                    .ok()
                    .map(|(r, _)| r),
            };
            match r {
                Some(mut r) => {
                    r.rung = false;
                    !r.stopped
                }
                None => false,
            }
        }
    }
    impl std::task::Wake for Bell {
        fn wake(self: Arc<Self>) {
            self.ring();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.ring();
        }
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
    /// region measure's one-way time; zero elsewhere), until `bell` is stopped. A cut member's records, out
    /// or in, are lost as a dead node's are. At most `pending` records are held: past it the relay
    /// takes no more from the members until the oldest are delivered. With nothing to move it
    /// waits on `bell` (a member's record or end, or the stop), and while records are held, no
    /// later than the oldest's due time: it never spins (Ada's event-driven rule).
    fn relay(
        handles: &[amphora::replication::ReplicaHandle],
        cut: &[AtomicBool],
        bell: &Bell,
        wait: Duration,
        records: usize,
        between: Duration,
        pending: usize,
    ) {
        let is_cut = |i: usize| cut.get(i).is_some_and(|c| c.load(Ordering::Relaxed));
        let mut held = VecDeque::new();
        while bell.going() {
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
            if moved == 0 && !bell.wait(held.front().map(|(due, _, _)| *due)) {
                break;
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
                let bell = Arc::new(Bell::default());
                for h in &handles {
                    h.listen(std::task::Waker::from(Arc::clone(&bell)))
                        .map_err(|e| anyhow::anyhow!("group {g}'s relay can't listen: {e:?}"))?;
                }
                let (c, b, wait) = (cut.clone(), bell.clone(), o.deadline);
                let relay = std::thread::spawn(move || {
                    relay(&handles, &c, &b, wait, records, between, pending)
                });
                groups.push(Group {
                    members,
                    bell,
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

        /// The leader a live member of group `g` names, if it's live too. While no live member
        /// names a live leader (none elected yet, or the one named is cut off), it waits on a
        /// live member for the leader change: a leader of a later term than the one it names, on
        /// the owner's word (ReplicaHandle::wait_leader, lane ryw-5), up to --deadline-s, never
        /// a retry (lane scaleout-harness-10).
        fn leader(&self, g: usize) -> anyhow::Result<&Database> {
            let group = self.group(g)?;
            let named = |l: u64| {
                usize::try_from(l)
                    .ok()
                    .and_then(|l| l.checked_sub(1))
                    .filter(|&i| !self.is_cut(i))
                    .and_then(|i| group.members.get(i))
            };
            let mut waiting = None;
            for (i, m) in group.members.iter().enumerate() {
                if self.is_cut(i) {
                    continue;
                }
                let Some(h) = m.replica() else {
                    continue;
                };
                let Ok(status) = h.status() else {
                    continue;
                };
                if let Some(leader) = named(status.leader) {
                    return Ok(leader);
                }
                // A leader of the term it names, when it names none; else one of a later term.
                let past = if status.leader == 0 {
                    status.term.saturating_sub(1)
                } else {
                    status.term
                };
                waiting.get_or_insert((h, past));
            }
            let (h, past) =
                waiting.ok_or_else(|| anyhow::anyhow!("group {g} has no live member"))?;
            let status = h
                .wait_leader(past, Instant::now() + self.deadline)
                .map_err(|e| anyhow::anyhow!("group {g} elected no leader: {e:?}"))?;
            named(status.leader).ok_or_else(|| {
                anyhow::anyhow!(
                    "group {g}'s next leader, member {}, is cut off",
                    status.leader
                )
            })
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

        /// `rows` on group `g`'s leader, answered with the write's own session token
        /// (Database::call_session).
        fn exec(&self, g: usize, rows: Vec<(String, Vec<Value>)>) -> anyhow::Result<Mark> {
            self.leader(g)?
                .call_session(
                    Instant::now() + self.deadline,
                    AnswerMax::fixed(),
                    move |c| {
                        for (sql, args) in rows {
                            c.prepare_cached(&sql)?.execute(args)?;
                        }
                        Ok(())
                    },
                )
                .map(|((), token)| token)
                .map_err(|e| anyhow::anyhow!("group {g} refused a write: {e:?}"))
        }

        /// A live member of group `g` that doesn't lead it, for a follower read.
        fn follower(&self, g: usize) -> anyhow::Result<(usize, &Database)> {
            let leader = self
                .leader(g)?
                .replica()
                .and_then(|h| h.status().ok())
                .map(|s| s.leader)
                .unwrap_or(0);
            self.group(g)?
                .members
                .iter()
                .enumerate()
                .find(|(i, _)| *i as u64 + 1 != leader && !self.is_cut(*i))
                .ok_or_else(|| anyhow::anyhow!("group {g} has no live follower"))
        }
    }

    impl Drop for Amphora {
        fn drop(&mut self) {
            if let Ok(mut xg) = self.xg.lock() {
                xg.take();
            }
            for g in &mut self.groups {
                g.bell.stop();
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
    /// The group time a mark names, which a closed-time read waits for: every write at or before it
    /// is applied once a replica's closed time has passed it.
    fn closed_at(token: amphora::database::Token) -> anyhow::Result<u64> {
        anyhow::ensure!(
            token.micros > 0,
            "the write's session token names no group time to wait for: {token:?}"
        );
        Ok(token.micros)
    }

    const ONE: &str = "SELECT count(*) FROM events WHERE session = ?1 AND seq = ?2";
    const UPTO: &str = "SELECT count(*) FROM events WHERE session = ?1 AND seq <= ?2";

    impl Session for S<'_> {
        fn write(&mut self, regional: bool, rows: &[Row]) -> anyhow::Result<()> {
            self.write_marked(regional, rows).map(drop)
        }

        /// The write, and its own session token from the write's answer.
        fn write_marked(&mut self, regional: bool, rows: &[Row]) -> anyhow::Result<Mark> {
            anyhow::ensure!(!self.a.is_cut(self.node), "node {} is down", self.node);
            let Some(first) = rows.first() else {
                return Ok(None);
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
                let (i, m) = self.a.follower(g)?;
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

        /// One read carrying the preload's last token: on the leader, once it has applied it
        /// (ReadMode::Session); as a follower read, at the token's closed time, once the
        /// follower's closed time has passed it. Each waits on the replica's own events.
        fn read_after(
            &mut self,
            session: &str,
            seq: u64,
            follower: bool,
            mark: Mark,
        ) -> anyhow::Result<bool> {
            anyhow::ensure!(!self.a.is_cut(self.node), "node {} is down", self.node);
            let token = mark
                .ok_or_else(|| anyhow::anyhow!("the preload's last write gave no session token"))?;
            let g = self.a.group_of(session);
            let until = Instant::now() + self.a.deadline;
            let (session, seq) = (session.to_owned(), i64::try_from(seq)?);
            let n = if follower {
                let (i, m) = self.a.follower(g)?;
                m.query_closed(closed_at(token)?, until, counted!(ONE, session, seq))
                    .map_err(|e| anyhow::anyhow!("member {i} refused a follower read: {e:?}"))?
                    .value
            } else {
                self.a
                    .leader(g)?
                    .query_session(
                        ReadMode::Session,
                        Some(token),
                        until,
                        AnswerMax::fixed(),
                        counted!(ONE, session, seq),
                    )
                    .map_err(|e| anyhow::anyhow!("group {g} refused a read: {e:?}"))?
            };
            Ok(n > 0)
        }

        /// The count on this node's own member at the closed time of `mark` (the writer's last
        /// acknowledged write): it waits until the member's closed time passes that write.
        fn count_own_after(&mut self, session: &str, upto: u64, mark: Mark) -> anyhow::Result<u64> {
            anyhow::ensure!(!self.a.is_cut(self.node), "node {} is down", self.node);
            let Some(token) = mark else {
                // Nothing acknowledged for this writer: nothing to wait for.
                return self.count(session, upto, true);
            };
            let g = self.a.group_of(session);
            let until = Instant::now() + self.a.deadline;
            let (session, upto) = (session.to_owned(), i64::try_from(upto)?);
            let m = self
                .a
                .group(g)?
                .members
                .get(self.node)
                .ok_or_else(|| anyhow::anyhow!("no member {}", self.node))?;
            let n = m
                .query_closed(closed_at(token)?, until, counted!(UPTO, session, upto))
                .map_err(|e| anyhow::anyhow!("member {} refused a count: {e:?}", self.node))?
                .value;
            Ok(u64::try_from(n)?)
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
                let members = &self.group(g)?.members;
                // The schema goes to the leader a member names or, with none named yet or a member
                // out of reach, to the next member in the configured order, from the first (lane
                // scaleout-harness-10): a call waits in its member's queue until a leader is
                // known, then runs or is refused naming the leader, which takes it next. Each try
                // waits on the replica's own events, never a spin (lane scaleout-harness-9), and
                // all are given up at --deadline-s. The schema's statements are idempotent, so a
                // try whose outcome is unknown is made again.
                let mut to = self.leader(g).ok();
                let (mut next, mut out) = (0usize, 0usize);
                loop {
                    let member = match to {
                        Some(m) => m,
                        None => {
                            let m = members
                                .get(next % members.len().max(1))
                                .ok_or_else(|| anyhow::anyhow!("group {g} has no members"))?;
                            next += 1;
                            m
                        }
                    };
                    let refused = match member
                        .call(until, AnswerMax::fixed(), |c| Ok(c.execute_batch(EVENTS)?))
                    {
                        Ok(()) => break,
                        Err(e) => e,
                    };
                    anyhow::ensure!(
                        Instant::now() < until,
                        "group {g} took no schema within --deadline-s: {refused:?}"
                    );
                    to = match refused {
                        amphora_api::Refusal::NotLeader { leader: Some(l) } => usize::try_from(l)
                            .ok()
                            .and_then(|l| l.checked_sub(1))
                            .and_then(|i| members.get(i)),
                        amphora_api::Refusal::NotLeader { leader: None }
                        | amphora_api::Refusal::UnknownOutcome => self.leader(g).ok(),
                        // This member can't take it (out of reach or ended): the next in order,
                        // each once; given up once every member was.
                        e @ (amphora_api::Refusal::Unavailable(_)
                        | amphora_api::Refusal::Fenced(_)) => {
                            out += 1;
                            anyhow::ensure!(
                                out < members.len(),
                                "group {g}: no member could take its schema: {e:?}"
                            );
                            None
                        }
                        other => anyhow::bail!("group {g} refused its schema: {other:?}"),
                    };
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

        fn stale_routes(&self) -> anyhow::Result<u64> {
            Err(no_reshard_shell())
        }

        fn reshard(&self, _op: usize, _o: &Opts) -> anyhow::Result<()> {
            Err(no_reshard_shell())
        }
    }

    fn no_reshard_shell() -> anyhow::Error {
        anyhow::Error::from(Refusal::Unavailable("Amphora's splits, merges and moves are a pure core so far: the shell that carries them out over replicated groups isn't built (docs/design/reshard.md, \"What is left\")".into()))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        // Lane scaleout-harness-10 (T-SH.10): a write's session token makes the warm-up's read and
        // the region's catch-up wait for that write, by the replica's own events. Do: three
        // members, one group; write a row and take its mark; read it once on the leader and once
        // as a follower read; cut a member off, write a second row, put the member back and count
        // on its own copy once. Expect: a mark; both reads find the row; the member's one count
        // holds both rows, no poll in between.
        #[test]
        fn scaleout_harness_10_a_token_read_waits_for_its_write() {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path().join("amphora-sh10");
            let o = parse(
                &format!("--scale follower --system amphora-inproc --nodes 3 --groups 1 --writers 1 --deadline-s 60 --batch 1 --text-bytes 8 --rows 2 --warmup-s 1 --seconds 1 --class persisted --relay-records 64 --relay-pending 4096 --dir {}", dir.display())
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
            let a = Amphora::open(&o).unwrap();
            a.prepare(&o).unwrap();
            let mut w = Writer::new(0, 0);
            let mut s = a.connect(0).unwrap();
            let first = w.next(1, 8);
            let mark = s.write_marked(false, &first).unwrap();
            assert!(mark.is_some(), "a replicated write gives its session token");
            assert!(s.read_after(&session_name(0), 1, false, mark).unwrap());
            assert!(s.read_after(&session_name(0), 1, true, mark).unwrap());
            let (back, _) = a.follower(0).unwrap();
            a.kill(back).unwrap();
            // The second write through a node still up: it goes to the group's leader.
            let mut up = a.connect((back + 1) % 3).unwrap();
            let second = w.next(1, 8);
            let mark = up.write_marked(false, &second).unwrap();
            a.start(back).unwrap();
            let mut own = a.connect(back).unwrap();
            assert_eq!(own.count_own_after(&session_name(0), 2, mark).unwrap(), 2);
            drop((s, up, own));
            drop(a);
            let _ = std::fs::remove_dir_all(&dir);
        }

        // Lane scaleout-harness-9 (AC-SH.5): the relay's bell keeps a ring given before its wait,
        // a due time ends a wait no ring came to, and the stop ends the relay, its own ring kept
        // as well. Do: ring then wait; wait on a due time already past; stop then wait, from
        // another thread too. Expect: go on, go on, then stopped.
        #[test]
        fn scaleout_harness_9_the_relays_bell_keeps_a_ring_and_stops() {
            let bell = Arc::new(Bell::default());
            bell.ring();
            assert!(bell.wait(None), "a ring before the wait is kept");
            assert!(bell.wait(Some(Instant::now())), "a due time ends the wait");
            assert!(bell.going());
            let b = Arc::clone(&bell);
            let stopper = std::thread::spawn(move || b.stop());
            assert!(!bell.wait(None), "the stop ends the wait");
            stopper.join().unwrap();
            assert!(!bell.going());
            std::task::Waker::from(Arc::clone(&bell)).wake();
            assert!(!bell.wait(None), "a ring after the stop doesn't undo it");
        }
    }
}

/// Amphora as its own server (`amphora serve`, CP-T1.13), one member a node, brought up and down by
/// bench/servers/scale/amphora.sh as CockroachDB's and TiDB's are by theirs, and reached over TCP by
/// the server's own client (`amphora::serve::Client`, one code path with the server's): one request a
/// line (`exec SQL`, `count SQL`, `status`), one answer a line (`ok ...` or `err ...`). The server serves one
/// replicated database: `--groups` takes 1. A write goes to the member that leads (a member's
/// `status` names it), as a client of a single-leader database sends it; a refused one is sent again
/// by the bench's closed loop after the leader is asked again.
mod served {
    use super::*;
    use amphora::serve::Client as Link;

    const EVENTS: &str = "CREATE TABLE IF NOT EXISTS events(session TEXT NOT NULL, seq INTEGER NOT NULL, body TEXT NOT NULL, PRIMARY KEY(session, seq))";

    pub struct Amphora {
        c: sql::Scripted,
        /// Each node's address and the cluster's secret.
        members: Vec<(String, Vec<u8>)>,
        deadline: Duration,
    }

    fn no_concept(what: &str) -> anyhow::Error {
        anyhow::Error::from(Refusal::Unavailable(format!(
            "Amphora's server doesn't serve {what} yet (its client protocol takes exec, count and status)"
        )))
    }

    impl Amphora {
        pub fn up(o: &Opts) -> anyhow::Result<Self> {
            if o.groups != 1 {
                return Err(anyhow::Error::from(Refusal::Unavailable("Amphora's server serves one replicated database: --groups 1 (groups across servers aren't served yet)".into())));
            }
            let mut env = vec![
                (
                    "CLASS".to_owned(),
                    need(o.class.clone(), "--class", &o.measure)?,
                ),
                ("DEADLINE_MS".to_owned(), o.deadline.as_millis().to_string()),
            ];
            if let Some(sp) = o.spanning {
                env.push(("SPANNING".to_owned(), sp.to_string()));
            }
            let c = sql::Scripted::up(o, env)?;
            let mut members = Vec::new();
            for n in 0..o.nodes {
                let ep = c.run(&["endpoint", &n.to_string()])?;
                let ep = ep.trim();
                let (addr, secret) = ep.split_once(' ').ok_or_else(|| {
                    anyhow::anyhow!("amphora.sh's endpoint {ep}: not ADDR SECRET_FILE")
                })?;
                let mut bytes = std::fs::read(secret)?;
                while bytes.last().is_some_and(u8::is_ascii_whitespace) {
                    bytes.pop();
                }
                members.push((addr.to_owned(), bytes));
            }
            Ok(Self {
                c,
                members,
                deadline: o.deadline,
            })
        }

        fn link(&self, node: usize) -> anyhow::Result<Link> {
            let (addr, secret) = self
                .members
                .get(node)
                .ok_or_else(|| anyhow::anyhow!("no node {node}"))?;
            Link::connect(
                addr,
                &amphora::serve::Keys::plain(secret.clone()),
                self.deadline,
            )
        }

        /// The node that leads, as node `via` names it.
        fn leader(&self, via: &mut Link) -> anyhow::Result<usize> {
            let status = via.ask("status")?;
            let l: usize = status
                .split_whitespace()
                .find_map(|f| f.strip_prefix("leader="))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            anyhow::ensure!(l >= 1 && l <= self.members.len(), "no leader yet: {status}");
            Ok(l - 1)
        }
    }

    struct S<'a> {
        a: &'a Amphora,
        own: Link,
        /// The leader's node and a link to it, asked again after a refusal.
        lead: Option<(usize, Link)>,
    }

    impl S<'_> {
        fn ask_leader(&mut self, line: &str) -> anyhow::Result<String> {
            if self.lead.is_none() {
                let l = self.a.leader(&mut self.own)?;
                self.lead = Some((l, self.a.link(l)?));
            }
            let Some((_, link)) = self.lead.as_mut() else {
                anyhow::bail!("no leader link");
            };
            let answer = link.ask(line);
            if answer.is_err() {
                self.lead = None;
            }
            answer
        }
    }

    impl Session for S<'_> {
        fn write(&mut self, regional: bool, rows: &[Row]) -> anyhow::Result<()> {
            if regional {
                return Err(no_concept("regional tables"));
            }
            if rows.is_empty() {
                return Ok(());
            }
            self.ask_leader(&format!("exec {}", Dialect::Standard.upsert(false, rows)))
                .map(|_| ())
        }

        fn read(&mut self, session: &str, seq: u64, follower: bool) -> anyhow::Result<bool> {
            if follower {
                return Err(no_concept("follower reads"));
            }
            let n: u64 = self
                .ask_leader(&format!(
                    "count {}",
                    Dialect::Standard.count("events", session, seq, false)
                ))?
                .parse()?;
            Ok(n > 0)
        }

        fn add(&mut self, _keys: &[i64], _value: i64) -> anyhow::Result<()> {
            Err(no_concept("transactions across groups"))
        }

        fn count(&mut self, session: &str, upto: u64, own: bool) -> anyhow::Result<u64> {
            let q = format!(
                "count {}",
                Dialect::Standard.count("events", session, upto, true)
            );
            Ok(if own {
                self.own.ask(&q)?
            } else {
                self.ask_leader(&q)?
            }
            .parse()?)
        }

        fn count_own_after(
            &mut self,
            _session: &str,
            _upto: u64,
            _mark: Mark,
        ) -> anyhow::Result<u64> {
            Err(anyhow::Error::from(Refusal::Unavailable("amphora serve's protocol has no read that waits for a write on a member's own copy (no session token on the wire yet; its count is a local read, which doesn't wait)".into())))
        }
    }

    impl System for Amphora {
        fn nodes(&self) -> usize {
            self.members.len()
        }

        fn prepare(&self, o: &Opts) -> anyhow::Result<()> {
            match o.measure.as_str() {
                "write" | "read" | "regionloss" => {}
                "cutover" => return Err(anyhow::Error::from(Refusal::Unavailable("Amphora's splits, merges and moves are a pure core so far: the shell that carries them out over replicated groups isn't built (docs/design/reshard.md, \"What is left\")".into()))),
                other => return Err(no_concept(&format!("the {other} measure"))),
            }
            // The schema through the leader a member names. With no leader named yet, the member
            // asked takes it itself: its call waits in its queue until a leader is known, then
            // runs there or is refused. Each try waits on the member's answer and its election:
            // no timer, no spin (lane scaleout-harness-9). The members are asked in the
            // configured order, from member 0, each only once the one before failed (it couldn't
            // be reached, or its try was refused), all given up at --deadline-s: none is asked
            // again at once (lane scaleout-harness-10). The schema's statements are idempotent.
            let until = Instant::now() + o.deadline;
            let exec = format!("exec {EVENTS}");
            first_reachable(self.members.len(), until, |n| {
                let mut via = self.link(n)?;
                match self.leader(&mut via) {
                    Ok(l) => self.link(l)?.ask(&exec),
                    Err(_) => via.ask(&exec),
                }
            })
            .map(drop)
            .map_err(|e| anyhow::anyhow!("Amphora took no schema: {e}"))
        }

        fn connect(&self, node: usize) -> anyhow::Result<Box<dyn Session + '_>> {
            Ok(Box::new(S {
                a: self,
                own: self.link(node)?,
                lead: None,
            }))
        }

        fn kill(&self, node: usize) -> anyhow::Result<()> {
            self.c.run(&["kill", &node.to_string()]).map(|_| ())
        }

        fn start(&self, node: usize) -> anyhow::Result<()> {
            self.c.run(&["start", &node.to_string()]).map(|_| ())
        }

        fn stale_routes(&self) -> anyhow::Result<u64> {
            Err(no_concept("a count of stale routes"))
        }

        fn reshard(&self, _op: usize, _o: &Opts) -> anyhow::Result<()> {
            Err(no_concept("range operations"))
        }
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

    /// A cluster brought up by its script (bench/servers/scale/<system>.sh) and taken down by it
    /// when dropped.
    pub struct Scripted {
        script: PathBuf,
        shell: String,
        hosts: String,
        deadline: Duration,
        env: Vec<(String, String)>,
    }

    impl Scripted {
        /// `up --nodes` through the system's script, `env` added to what it's given.
        pub fn up(o: &Opts, env: Vec<(String, String)>) -> anyhow::Result<Self> {
            let c = Self {
                script: PathBuf::from(format!("{}/{}.sh", o.scripts.display(), o.system)),
                shell: o.shell.clone(),
                hosts: need(o.hosts.clone(), "--hosts", &o.measure)?,
                deadline: o.deadline,
                env,
            };
            c.run(&["up", &o.nodes.to_string()]).map_err(|e| {
                Refusal::Unavailable(format!(
                    "{}'s cluster didn't come up through {}: {e}",
                    o.system,
                    c.script.display()
                ))
            })?;
            Ok(c)
        }

        /// The script run by `--shell`, given the run's deadline as its bound on waiting.
        pub fn run(&self, args: &[&str]) -> anyhow::Result<String> {
            let out = std::process::Command::new(&self.shell)
                .arg(&self.script)
                .args(args)
                .env("WAIT_S", self.deadline.as_secs().max(1).to_string())
                .env("HOSTS", &self.hosts)
                .envs(self.env.iter().map(|(k, v)| (k, v)))
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
    }

    impl Drop for Scripted {
        fn drop(&mut self) {
            if let Err(e) = self.run(&["down"]) {
                use std::io::Write;
                // Drop can't return the failure: it goes to stderr, and the nodes stay for `down`.
                writeln!(
                    std::io::stderr().lock(),
                    "{} down: {e}",
                    self.script.display()
                )
                .ok();
            }
        }
    }

    pub struct Containers {
        kind: Kind,
        s: Scripted,
        endpoints: Vec<String>,
        deadline: Duration,
        /// PD's `schedule.patrol-region-interval`, read once, when a wait first needs it.
        patrol: std::sync::OnceLock<Duration>,
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
                s: Scripted::up(o, Vec::new())?,
                endpoints: Vec::new(),
                deadline: o.deadline,
                patrol: std::sync::OnceLock::new(),
            };
            for n in 0..o.nodes {
                c.endpoints
                    .push(c.script(&["endpoint", &n.to_string()])?.trim().to_owned());
            }
            Ok(c)
        }

        fn script(&self, args: &[&str]) -> anyhow::Result<String> {
            self.s.run(args)
        }

        fn endpoint(&self, n: usize) -> anyhow::Result<&str> {
            self.endpoints
                .get(n)
                .map(String::as_str)
                .ok_or_else(|| anyhow::anyhow!("no node {n}"))
        }

        /// How often a wait on PD's background work reads its state: PD's own
        /// `schedule.patrol-region-interval`, the period at which its checker inspects each
        /// Region against its placement rules (docs.pingcap.com/tidb/stable/pd-configuration-file).
        /// PD has no watch or long-poll for Region, operator or placement changes (its HTTP API
        /// and pd-ctl answer each request with the state then; the gRPC `SyncRegions` stream is
        /// PD's own leader-to-follower copy), so the cluster's own period is the wait's, never a
        /// picked one. The slowest PD's, should they differ (lane scaleout-harness-11).
        fn pd_patrol(&self, c: &mut mysql::Conn) -> anyhow::Result<Duration> {
            use mysql::prelude::Queryable;
            if let Some(d) = self.patrol.get() {
                return Ok(*d);
            }
            let rows: Vec<(String, String, String, String)> = c.query(
                "SHOW CONFIG WHERE type = 'pd' AND name = 'schedule.patrol-region-interval'",
            )?;
            let mut slowest: Option<Duration> = None;
            for (_, instance, _, value) in &rows {
                let d = go_duration(value).map_err(|e| {
                    anyhow::anyhow!(
                        "PD {instance}'s schedule.patrol-region-interval {value:?}: {e}"
                    )
                })?;
                slowest = Some(slowest.map_or(d, |s| s.max(d)));
            }
            let d = slowest.ok_or_else(|| {
                anyhow::anyhow!("SHOW CONFIG names no PD's schedule.patrol-region-interval")
            })?;
            anyhow::ensure!(!d.is_zero(), "PD's schedule.patrol-region-interval is zero");
            Ok(*self.patrol.get_or_init(|| d))
        }
    }

    /// A Go duration as PD's configuration prints it (Go's `time.Duration` text: decimal numbers,
    /// each with a unit of ns, us, µs, ms, s, m or h, as `10ms` or `1m30s`).
    pub fn go_duration(text: &str) -> anyhow::Result<Duration> {
        let mut total = Duration::ZERO;
        let mut rest = text.trim();
        anyhow::ensure!(!rest.is_empty(), "no duration");
        if rest == "0" {
            return Ok(total);
        }
        while !rest.is_empty() {
            let digits = rest
                .find(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
                .ok_or_else(|| anyhow::anyhow!("{text:?}: a number without a unit"))?;
            let (number, after) = rest.split_at(digits);
            let unit_len = after
                .find(|ch: char| ch.is_ascii_digit() || ch == '.')
                .unwrap_or(after.len());
            let (unit, next) = after.split_at(unit_len);
            let unit_ns: u128 = match unit {
                "ns" => 1,
                "us" | "µs" | "μs" => 1_000,
                "ms" => 1_000_000,
                "s" => 1_000_000_000,
                "m" => 60_000_000_000,
                "h" => 3_600_000_000_000,
                _ => anyhow::bail!("{text:?}: unknown unit {unit:?}"),
            };
            // Whole and fractional parts in integers, so `10ms` is exactly 10 ms.
            let (whole, frac) = number.split_once('.').unwrap_or((number, ""));
            anyhow::ensure!(
                !(whole.is_empty() && frac.is_empty()) && !frac.contains('.'),
                "{text:?}: no number before {unit:?}"
            );
            let digits_of = |d: &str| -> anyhow::Result<u128> {
                if d.is_empty() {
                    Ok(0)
                } else {
                    d.parse::<u128>()
                        .map_err(|e| anyhow::anyhow!("{text:?}: {e}"))
                }
            };
            let scale = u32::try_from(frac.len())
                .ok()
                .and_then(|n| 10u128.checked_pow(n))
                .ok_or_else(|| anyhow::anyhow!("{text:?}: too many decimals"))?;
            let (whole, frac) = (digits_of(whole)?, digits_of(frac)?);
            let ns = whole
                .checked_mul(unit_ns)
                .zip(frac.checked_mul(unit_ns))
                .and_then(|(w, f)| w.checked_add(f / scale))
                .ok_or_else(|| anyhow::anyhow!("{text:?}: too long"))?;
            let ns = u64::try_from(ns).map_err(|_| anyhow::anyhow!("{text:?}: too long"))?;
            total = total
                .checked_add(Duration::from_nanos(ns))
                .ok_or_else(|| anyhow::anyhow!("{text:?}: too long"))?;
            rest = next;
        }
        Ok(total)
    }

    struct Pg {
        c: crate::pgtext::Client,
    }

    struct My {
        c: mysql::Conn,
        follower: bool,
    }

    impl Pg {
        /// A count's one row, by the simple query protocol (one round trip, the text parsed by the
        /// server as Amphora's is).
        fn counted(&mut self, sql: &str) -> anyhow::Result<u64> {
            let rows = self.c.simple_query(sql)?;
            let n = rows
                .first()
                .ok_or_else(|| anyhow::anyhow!("{sql}: no row"))?
                .first()
                .cloned()
                .flatten()
                .ok_or_else(|| anyhow::anyhow!("{sql}: a NULL count"))?;
            Ok(n.parse()?)
        }
    }

    impl Session for Pg {
        fn write(&mut self, regional: bool, rows: &[Row]) -> anyhow::Result<()> {
            if rows.is_empty() {
                return Ok(());
            }
            self.c
                .simple_query(&Dialect::Standard.upsert(regional, rows))
                .map(drop)
        }

        fn read(&mut self, session: &str, seq: u64, follower: bool) -> anyhow::Result<bool> {
            let from = if follower {
                "events AS OF SYSTEM TIME follower_read_timestamp()"
            } else {
                "events"
            };
            Ok(self.counted(&Dialect::Standard.count(from, session, seq, false))? > 0)
        }

        fn add(&mut self, keys: &[i64], value: i64) -> anyhow::Result<()> {
            self.c
                .simple_query(&Dialect::Standard.kv_upsert(keys, value))
                .map(drop)
        }

        fn count(&mut self, session: &str, upto: u64, _own: bool) -> anyhow::Result<u64> {
            // CockroachDB answers every read by its leaseholder: a node's own copy isn't readable.
            self.counted(&Dialect::Standard.count("events", session, upto, true))
        }
    }

    impl Session for My {
        fn write(&mut self, regional: bool, rows: &[Row]) -> anyhow::Result<()> {
            use mysql::prelude::Queryable;
            if rows.is_empty() {
                return Ok(());
            }
            Ok(self.c.query_drop(Dialect::Mysql.upsert(regional, rows))?)
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
            let n: Option<i64> = self
                .c
                .query_first(Dialect::Mysql.count("events", session, seq, false))?;
            Ok(n.unwrap_or(0) > 0)
        }

        fn add(&mut self, keys: &[i64], value: i64) -> anyhow::Result<()> {
            use mysql::prelude::Queryable;
            Ok(self.c.query_drop(Dialect::Mysql.kv_upsert(keys, value))?)
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
            let n: Option<i64> = self
                .c
                .query_first(Dialect::Mysql.count("events", session, upto, true))?;
            Ok(u64::try_from(n.unwrap_or(0))?)
        }
    }

    impl System for Containers {
        fn nodes(&self) -> usize {
            self.endpoints.len()
        }

        fn prepare(&self, o: &Opts) -> anyhow::Result<()> {
            let stmts = setup(self.kind, o);
            // The schema through the first node, in the configured order, that can be reached:
            // the next is tried once the one before failed to connect, all given up at
            // --deadline-s (lane scaleout-harness-10).
            let until = Instant::now() + o.deadline;
            let nodes = self.nodes();
            match self.kind {
                Kind::Cockroach => {
                    let mut c = first_reachable(nodes, until, |n| {
                        Ok(self
                            .endpoint(n)?
                            .parse::<postgres::Config>()?
                            .connect_timeout(self.deadline)
                            .connect(postgres::NoTls)?)
                    })
                    .map_err(|e| anyhow::anyhow!("CockroachDB took no schema: {e}"))?;
                    for q in &stmts {
                        c.batch_execute(q)
                            .map_err(|e| anyhow::anyhow!("{q}: {e}"))?;
                    }
                }
                Kind::Tidb => {
                    use mysql::prelude::Queryable;
                    let mut c = first_reachable(nodes, until, |n| {
                        let opts = mysql::OptsBuilder::from_opts(mysql::Opts::from_url(
                            self.endpoint(n)?,
                        )?)
                        .tcp_connect_timeout(Some(self.deadline))
                        .read_timeout(Some(self.deadline))
                        .write_timeout(Some(self.deadline));
                        Ok(mysql::Conn::new(opts)?)
                    })
                    .map_err(|e| anyhow::anyhow!("TiDB took no schema: {e}"))?;
                    for q in &stmts {
                        c.query_drop(q).map_err(|e| anyhow::anyhow!("{q}: {e}"))?;
                    }
                    if o.measure == "region" {
                        // Placement is applied in the background: wait until every policy is,
                        // reading it at PD's own period (no watch: Self::pd_patrol).
                        let patrol = self.pd_patrol(&mut c)?;
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
                            #[expect(
                                clippy::disallowed_methods,
                                reason = "PD has no watch for this state (Self::pd_patrol): read at PD's own patrol period"
                            )]
                            std::thread::sleep(patrol);
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
                // Each read and write given up after --deadline-s, as TiDB's below.
                Kind::Cockroach => Box::new(Pg {
                    c: crate::pgtext::Client::connect(ep, self.deadline)?,
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

        fn stale_routes(&self) -> anyhow::Result<u64> {
            let names: &[&str] = match self.kind {
                Kind::Cockroach => &[
                    "distsender_errors_notleaseholder",
                    "distsender_rpc_err_rangekeymismatcherrtype",
                ],
                Kind::Tidb => &["tidb_tikvclient_region_err_total"],
            };
            let mut total = 0u64;
            for n in 0..self.nodes() {
                total += stale_count(&self.script(&["metrics", &n.to_string()])?, names);
            }
            Ok(total)
        }

        fn reshard(&self, op: usize, o: &Opts) -> anyhow::Result<()> {
            let session = session_name((op / 3) % o.writers);
            let what = Op::of(op, self.nodes());
            match self.kind {
                Kind::Cockroach => self.reshard_cockroach(what, &session, op),
                Kind::Tidb => self.reshard_tidb(what, &session, op, o),
            }
        }

        fn waits(&self) -> Option<serde_json::Value> {
            self.patrol.get().map(|d| waits_record(*d))
        }
    }

    /// The record's `waits` for TiDB: what its waits on PD's background work read, and how often.
    pub fn waits_record(patrol: Duration) -> serde_json::Value {
        serde_json::json!({
            "on": "TiDB's placement (SHOW PLACEMENT) and PD's operators (the Region's peers and leader), read until applied",
            "every_us": u64::try_from(patrol.as_micros()).unwrap_or(u64::MAX),
            "every_is": "PD's schedule.patrol-region-interval, read from the cluster by SHOW CONFIG: PD's own period for checking each Region against its rules; PD has no watch or long-poll for Region or placement changes",
        })
    }

    /// The sum of the samples of metrics `names` in a Prometheus text exposition (every label set).
    pub fn stale_count(text: &str, names: &[&str]) -> u64 {
        text.lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| {
                let (key, value) = l.rsplit_once(' ')?;
                let name = key.split('{').next()?;
                if !names.contains(&name) {
                    return None;
                }
                value.parse::<f64>().ok()
            })
            .map(|v| v.max(0.0) as u64)
            .sum()
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

    /// A Region bound decoded by `TIDB_DECODE_KEY` as (session, seq): `None` when it names no row of
    /// the table (its start or end, or another table).
    fn bound(decoded: &str) -> Option<(String, i64)> {
        let v: serde_json::Value = serde_json::from_str(decoded).ok()?;
        let h = v.get("handle")?;
        let session = h.get("session")?.as_str()?.to_owned();
        let seq = match h.get("seq")? {
            serde_json::Value::String(s) => s.parse().ok()?,
            n => n.as_i64()?,
        };
        Some((session, seq))
    }

    /// Whether the Region from `lo` to `hi` (decoded) holds (`session`, the largest sequence number).
    pub fn holds(lo: &str, hi: &str, session: &str) -> bool {
        let key = (session.to_owned(), i64::MAX);
        bound(lo).is_none_or(|b| b <= key) && bound(hi).is_none_or(|b| key < b)
    }

    /// Whether a Region's peers' stores and its leader's store show an operation taken effect.
    type Taken = Box<dyn Fn(&[i64], i64) -> bool>;

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

        /// The Region that takes `session`'s new rows: the one whose keys, decoded by TiDB, hold
        /// (`session`, the largest sequence number).
        fn tidb_region_of(c: &mut mysql::Conn, session: &str) -> anyhow::Result<u64> {
            use mysql::prelude::Queryable;
            let bounds: Vec<(u64, String, String)> = c.query(
                "SELECT REGION_ID, TIDB_DECODE_KEY(START_KEY), TIDB_DECODE_KEY(END_KEY) FROM information_schema.tikv_region_status \
                 WHERE DB_NAME = 'test' AND TABLE_NAME = 'events' AND IS_INDEX = 0",
            )?;
            bounds
                .iter()
                .find(|(_, lo, hi)| holds(lo, hi, session))
                .map(|(r, _, _)| *r)
                .ok_or_else(|| {
                    anyhow::anyhow!("no Region of events holds {session}'s new rows: {bounds:?}")
                })
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
            let region = Self::tidb_region_of(&mut c, session)?;
            let (region, replicas, leader) = Self::tidb_regions(&mut c)?
                .into_iter()
                .find(|(r, _, _)| *r == region)
                .ok_or_else(|| anyhow::anyhow!("Region {region} has no peers"))?;
            let done: Taken = if what == Op::Leader {
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
            // PD's operators run in the background: the operation has taken effect once the Region
            // shows it, read at PD's own period (no watch: Self::pd_patrol).
            let patrol = self.pd_patrol(&mut c)?;
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
                #[expect(
                    clippy::disallowed_methods,
                    reason = "PD has no watch for this state (Self::pd_patrol): read at PD's own patrol period"
                )]
                std::thread::sleep(patrol);
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
        let e = parse(&args("--scale xgroup --system amphora-inproc --nodes 3 --groups 3 --writers 2 --deadline-s 5 --keys 30 --span 2 --warmup-s 1 --seconds 1")).err().unwrap().to_string();
        assert!(e.contains("--protocol"), "{e}");
        // Amphora's server is brought up on the machines it's given, as the others are.
        let served = "--scale write --system amphora --nodes 3 --groups 1 --writers 2 --deadline-s 5 --batch 1 --text-bytes 1 --warmup-s 1 --seconds 1";
        let e = parse(&args(served)).err().unwrap().to_string();
        assert!(e.contains("--hosts"), "{e}");
        let e = parse(&args(&format!("{served} --hosts local=127.0.0.1")))
            .err()
            .unwrap()
            .to_string();
        assert!(e.contains("--class"), "{e}");
        assert!(
            parse(&args(&format!(
                "{served} --hosts local=127.0.0.1 --class persisted"
            )))
            .is_ok()
        );
        let e = parse(&args(&format!(
            "{served} --hosts local=127.0.0.1 --class persisted --spanning 2"
        )))
        .err()
        .unwrap()
        .to_string();
        assert!(e.contains("--spanning"), "{e}");
        let e = parse(&args("--scale regionloss --system cockroach --nodes 3 --groups 3 --writers 2 --deadline-s 5 --batch 1 --text-bytes 1 --lose 3 --settle-s 1 --after-s 1")).err().unwrap().to_string();
        assert!(e.contains("--lose"), "{e}");
        let e = parse(&args("--scale xgroup --system tidb --nodes 3 --groups 2 --writers 2 --deadline-s 5 --keys 30 --span 3 --warmup-s 1 --seconds 1")).err().unwrap().to_string();
        assert!(e.contains("--span"), "{e}");
    }

    // AC-SH.3: every system over the network is sent a write as one statement, its rows as
    // literals, quoted as its dialect reads them.
    #[test]
    fn ac_sh_3_every_system_is_sent_one_statement_a_write() {
        let rows = [
            Row {
                session: "s-000001".into(),
                seq: 7,
                region: 2,
                body: "it's a \\ b".into(),
            },
            Row {
                session: "s-000001".into(),
                seq: 8,
                region: 2,
                body: "c".into(),
            },
        ];
        assert_eq!(
            Dialect::Standard.upsert(false, &rows),
            "INSERT INTO events (session, seq, body) VALUES ('s-000001', 7, 'it''s a \\ b'), ('s-000001', 8, 'c') ON CONFLICT (session, seq) DO UPDATE SET body = excluded.body"
        );
        assert_eq!(
            Dialect::Mysql.upsert(true, &rows),
            "INSERT INTO regional (region, session, seq, body) VALUES ('r2', 's-000001', 7, 'it''s a \\\\ b'), ('r2', 's-000001', 8, 'c') ON DUPLICATE KEY UPDATE body = VALUES(body)"
        );
        assert_eq!(
            Dialect::Standard.kv_upsert(&[3, 13], 5),
            "UPSERT INTO kv (k, v) VALUES (3, 5), (13, 5)"
        );
        assert_eq!(
            Dialect::Mysql.count("events", "s-000001", 9, true),
            "SELECT count(*) FROM events WHERE session = 's-000001' AND seq <= 9"
        );
    }

    // Lane scaleout-harness-9 (G13): each system's record says how it routes a write (forwarded
    // or straight to the leader), what gives a request up, and that no system over a wire is
    // given prepared statements.
    #[test]
    fn scaleout_harness_9_records_name_routes_deadlines_and_statements() {
        let tail = "--nodes 3 --groups 1 --writers 2 --deadline-s 5 --batch 1 --text-bytes 1 --warmup-s 1 --seconds 1 --class persisted --hosts local=127.0.0.1";
        for (system, route) in [
            ("amphora", "straight to the leader"),
            ("cockroach", "forwarded"),
            ("tidb", "forwarded"),
        ] {
            let o = parse(&args(&format!("--scale write --system {system} {tail}"))).unwrap();
            let p = &record_head(&o)["platform"];
            let routes = p["routes"].as_str().unwrap();
            assert!(routes.starts_with(route), "{system}: {routes}");
            assert!(
                p["deadline"].as_str().unwrap().starts_with("--deadline-s"),
                "{system}"
            );
            assert!(
                p["statements"]
                    .as_str()
                    .unwrap()
                    .contains("no prepared statements"),
                "{system}"
            );
        }
    }

    // Lane scaleout-harness-10 (T-SH.8): schema setup asks each node in the configured order, the
    // next only once the one before failed, and gives up naming each failure, or at its deadline.
    #[test]
    fn scaleout_harness_10_schema_asks_each_node_in_turn_then_gives_up() {
        let later = Instant::now() + Duration::from_secs(3600);
        let mut asked = Vec::new();
        let got = first_reachable(3, later, |n| {
            asked.push(n);
            if n == 1 {
                Ok(n)
            } else {
                anyhow::bail!("refused")
            }
        });
        assert_eq!(got.unwrap(), 1);
        assert_eq!(
            asked,
            vec![0, 1],
            "node 0 failed, so node 1 was asked, and no more"
        );
        let mut asked = Vec::new();
        let e = first_reachable(3, later, |n| -> anyhow::Result<()> {
            asked.push(n);
            anyhow::bail!("unreachable {n}")
        })
        .unwrap_err()
        .to_string();
        assert_eq!(asked, vec![0, 1, 2], "each node once, none asked again");
        for n in 0..3 {
            assert!(e.contains(&format!("node {n}: unreachable {n}")), "{e}");
        }
        let mut asked = 0;
        let e = first_reachable(3, Instant::now(), |_| -> anyhow::Result<()> {
            asked += 1;
            Ok(())
        })
        .unwrap_err()
        .to_string();
        assert_eq!(asked, 0);
        assert!(e.contains("--deadline-s passed before node 0"), "{e}");
    }

    // Lane scaleout-harness-10 (T-SH.11): a measure's phase lasts its length unless a worker
    // fails, and then ends on the failure, which the measure returns. Do: hold a zero-length
    // phase; fail from another thread while a long phase holds; fail again. Expect: the first
    // ran its length, the second ended on the failure, and the first failure is the error.
    #[test]
    fn scaleout_harness_10_a_phase_ends_on_a_workers_failure() {
        let halt = Halt::default();
        assert!(halt.hold(Duration::ZERO));
        assert!(!halt.failed());
        std::thread::scope(|scope| {
            scope.spawn(|| halt.fail(anyhow::anyhow!("first")));
            assert!(
                !halt.hold(Duration::from_secs(3600)),
                "the failure ends the phase"
            );
        });
        halt.fail(anyhow::anyhow!("second"));
        assert!(halt.failed());
        assert_eq!(halt.take().unwrap_err().to_string(), "first");
    }

    /// A session that counts what it's asked, for the default warm-up and catch-up reads.
    #[derive(Default)]
    struct Asked {
        reads: Vec<(String, u64, bool)>,
        counts: Vec<(String, u64, bool)>,
    }

    impl Session for Asked {
        fn write(&mut self, _regional: bool, _rows: &[Row]) -> anyhow::Result<()> {
            Ok(())
        }
        fn read(&mut self, session: &str, seq: u64, follower: bool) -> anyhow::Result<bool> {
            self.reads.push((session.to_owned(), seq, follower));
            Ok(true)
        }
        fn add(&mut self, _keys: &[i64], _value: i64) -> anyhow::Result<()> {
            Ok(())
        }
        fn count(&mut self, session: &str, upto: u64, own: bool) -> anyhow::Result<u64> {
            self.counts.push((session.to_owned(), upto, own));
            Ok(upto)
        }
    }

    // Lane scaleout-harness-10 (T-SH.9): a system with no control for waiting on a write gets one
    // read for the warm-up and one own-copy count for the region's catch-up, never asked again;
    // regionloss takes no --poll-us; each record says what each system's warm-up and catch-up do.
    #[test]
    fn scaleout_harness_10_warmup_and_catch_up_read_once_by_each_systems_rule() {
        let mut s = Asked::default();
        assert_eq!(s.write_marked(false, &[]).unwrap(), None);
        assert!(s.read_after("s-000001", 7, true, None).unwrap());
        assert_eq!(s.reads, vec![("s-000001".to_owned(), 7, true)]);
        assert_eq!(s.count_own_after("s-000002", 9, None).unwrap(), 9);
        assert_eq!(s.counts, vec![("s-000002".to_owned(), 9, true)]);
        assert!(parse(&args("--scale regionloss --system cockroach --nodes 3 --groups 3 --writers 2 --deadline-s 5 --batch 1 --text-bytes 1 --lose 1 --settle-s 1 --after-s 1 --hosts local=a")).is_ok());
        for system in ["amphora", "amphora-inproc", "cockroach", "tidb"] {
            for follower in [false, true] {
                assert!(!warmup_rule(system, follower).is_empty(), "{system}");
            }
            assert!(!serve_rule(system).is_empty(), "{system}");
        }
        assert!(warmup_rule("amphora-inproc", true).contains("session token"));
        assert!(warmup_rule("tidb", true).contains("ReadIndex"));
        assert!(serve_rule("amphora").starts_with("refused"));
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
        let o = parse(&args("--scale region --system cockroach --nodes 3 --groups 2 --writers 4 --deadline-s 5 --batch 1 --text-bytes 1 --warmup-s 1 --seconds 1 --between-ms 30 --hosts local=127.0.0.1")).unwrap();
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
        let base = "--scale cutover --system tidb --nodes 3 --groups 3 --writers 2 --deadline-s 5 --batch 1 --text-bytes 1 --ops 4 --concurrent-ops 1 --settle-s 1";
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
        let e = parse(&args("--scale cutover --system cockroach --nodes 3 --groups 3 --writers 2 --deadline-s 5 --batch 1 --text-bytes 1 --settle-s 1 --hosts local=a")).err().unwrap().to_string();
        assert!(e.contains("--ops"), "{e}");
    }

    // Lane scaleout-harness-11 (T-SH.12): TiDB's waits on PD's background work read its state at
    // PD's own schedule.patrol-region-interval, as PD prints it; --poll-us isn't taken by region or
    // cutover, and the record says what the waits read and at which of the system's own periods.
    #[test]
    fn scaleout_harness_11_pd_waits_read_at_pds_own_period() {
        let d = |t: &str| sql::go_duration(t);
        assert_eq!(d("10ms").unwrap(), Duration::from_millis(10));
        assert_eq!(d("1m30s").unwrap(), Duration::from_secs(90));
        assert_eq!(d("1.5s").unwrap(), Duration::from_millis(1500));
        assert_eq!(d("500µs").unwrap(), Duration::from_micros(500));
        assert_eq!(d("2h0m0s").unwrap(), Duration::from_secs(7200));
        assert_eq!(d("0").unwrap(), Duration::ZERO);
        for bad in ["", "10", "ms", "10x", "1..5s", "99999999999999999999h"] {
            assert!(d(bad).is_err(), "{bad:?} was taken");
        }
        let region = "--scale region --system tidb --nodes 3 --groups 2 --writers 4 --deadline-s 5 --batch 1 --text-bytes 1 --warmup-s 1 --seconds 1 --between-ms 30 --hosts local=127.0.0.1";
        let cutover = "--scale cutover --system tidb --nodes 3 --groups 3 --writers 2 --deadline-s 5 --batch 1 --text-bytes 1 --ops 4 --concurrent-ops 1 --settle-s 1 --hosts local=127.0.0.1";
        for base in [region, cutover] {
            assert!(parse(&args(base)).is_ok(), "{base}");
            let e = parse(&args(&format!("{base} --poll-us 1000")))
                .err()
                .unwrap()
                .to_string();
            assert!(e.contains("--poll-us"), "{e}");
        }
        let w = sql::waits_record(Duration::from_millis(10));
        assert_eq!(w["every_us"], serde_json::json!(10_000));
        assert!(
            w["every_is"]
                .as_str()
                .unwrap()
                .contains("schedule.patrol-region-interval"),
            "{w}"
        );
    }

    // G13 step 2: a write counts toward every operation it was in flight during, and only those.
    // G13 step 2: the four figures beside the upper bound, from the writes and operations seen.
    #[test]
    fn g13_cutover_figures_split_moved_from_others() {
        let w = |writer, t0, t1, resends| Sent {
            writer,
            t0,
            t1,
            resends,
        };
        // Before the operation: the baseline. Writer 0 moves; writer 1 doesn't.
        let writes = [
            w(0, 0, 10, 0),
            w(1, 0, 12, 0),
            w(0, 10, 20, 0),
            // Writer 0's write across the operation (100..200): a stale route sent again twice.
            w(0, 90, 260, 2),
            w(1, 150, 170, 1),
            w(0, 260, 270, 0),
        ];
        let ops = [Moved {
            t0: 100,
            t1: 200,
            writers: vec![0],
        }];
        let f = figures(&writes, &ops);
        assert_eq!((f.moved_resends, f.other_resends), (2, 1));
        // The longest gap between writer 0's acknowledgements overlapping 100..200: 20 to 260.
        assert_eq!(f.unavailable, vec![240]);
        // The upper bound: the longest acknowledgement of any overlapping write.
        assert_eq!(f.pauses, vec![170]);
        assert_eq!(f.baseline, vec![10, 12, 10]);
        assert_eq!((f.moved, f.others), (vec![170], vec![20]));
    }

    // G13 step 2: a write overlapping a concurrent operation that moved its range counts as moved.
    #[test]
    fn g13_cutover_concurrent_operations_attribute_each_write() {
        let writes = [Sent {
            writer: 1,
            t0: 5,
            t1: 50,
            resends: 0,
        }];
        let ops = [
            Moved {
                t0: 0,
                t1: 40,
                writers: vec![0],
            },
            Moved {
                t0: 10,
                t1: 60,
                writers: vec![1],
            },
        ];
        let f = figures(&writes, &ops);
        assert_eq!((f.moved.len(), f.others.len()), (1, 0));
    }

    #[test]
    fn g13_cutover_moved_writers_are_the_sessions_group() {
        assert_eq!(group_writers(1, 4, 2), vec![0, 1]);
        assert_eq!(group_writers(3, 4, 2), vec![2, 3]);
        assert_eq!(group_writers(2, 4, 4), vec![2]);
        assert_eq!(group_writers(0, 3, 1), vec![0, 1, 2]);
    }

    #[test]
    fn g13_cutover_stale_routes_sum_every_label_set() {
        let text = "# HELP x\ndistsender_errors_notleaseholder 3\ndistsender_rpc_err_rangekeymismatcherrtype{node=\"1\"} 2.0\nother 9\n";
        let names = [
            "distsender_errors_notleaseholder",
            "distsender_rpc_err_rangekeymismatcherrtype",
        ];
        assert_eq!(sql::stale_count(text, &names), 5);
        let tidb = "tidb_tikvclient_region_err_total{type=\"not_leader\"} 4\ntidb_tikvclient_region_err_total{type=\"epoch_not_match\"} 1\n";
        assert_eq!(
            sql::stale_count(tidb, &["tidb_tikvclient_region_err_total"]),
            5
        );
    }

    #[test]
    fn g13_cutover_tidb_region_of_a_session() {
        let k = |s: &str, q: i64| {
            format!("{{\"handle\":{{\"session\":\"{s}\",\"seq\":\"{q}\"}},\"table_id\":7}}")
        };
        let table = "{\"table_id\":7}";
        assert!(sql::holds(table, &k("s-000001", 0), "s-000000"));
        assert!(!sql::holds(table, &k("s-000001", 0), "s-000001"));
        assert!(sql::holds(&k("s-000001", 4), "", "s-000001"));
        assert!(!sql::holds(
            &k("s-000001", 4),
            &k("s-000001", 9),
            "s-000001"
        ));
    }

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
