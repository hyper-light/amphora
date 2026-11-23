//! CP-T1.13 (AC-T1.13.1, AC-T1.13.5, T-T1.13.1): three members of `amphora serve` in one
//! process, over loopback TCP: healthy once they elect, a wrong secret refused, and a drill's load
//! with one member split off at its transport losing no acknowledged write; the same links inside
//! mutual TLS.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros
)]

use std::time::{Duration, Instant};

use amphora::serve::{self, Client, Config, Keys, SqlClient, Table};
use amphora_api::{Refusal, Statement, Value};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn three(dir: &std::path::Path) -> Vec<Config> {
    three_with(dir, |_| String::new())
}

/// Three members' configs, each with `more(n)`'s lines.
fn three_with(dir: &std::path::Path, more: impl Fn(u64) -> String) -> Vec<Config> {
    let secret = dir.join("secret");
    std::fs::write(&secret, "0123456789abcdef0123456789abcdef-test-secret\n").unwrap();
    let members = (1..=3)
        .map(|n| format!("{n}@127.0.0.1:{}", free_port()))
        .collect::<Vec<_>>()
        .join(",");
    (1..=3)
        .map(|n| {
            let text = format!("node = {n}\nmembers = {members}\ndata = {}\nclass = persisted\nsecret_file = {}\ndeadline_ms = 5000\ndrills = on\n{}", dir.join(format!("m{n}")).display(), secret.display(), more(n));
            Config::parse(&text).unwrap()
        })
        .collect()
}

/// The members of `configs`, each in a thread of its own, started together: one process's opens
/// of different databases run at once (CP-T1.13), so each open has its whole deadline. Returns once
/// each listens or has ended; a member's end is reported, and health then fails.
fn start(configs: &[Config]) {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut listening = Vec::new();
    for c in configs.iter().cloned() {
        let tx = tx.clone();
        let (up, listens) = std::sync::mpsc::channel();
        listening.push(listens);
        std::thread::spawn(move || {
            let node = c.node;
            let started = Instant::now();
            let ended = serve::serve(&c, move |addr| {
                eprintln!(
                    "member {node} listening on {addr}, open took {:?}",
                    started.elapsed()
                );
                let _ = up.send(());
            });
            let _ = tx.send(format!("member {node} ended: {ended:?}"));
        });
    }
    // A member whose open failed drops `up` unsent.
    for listens in listening {
        let _ = listens.recv();
    }
    std::thread::spawn(move || {
        for line in rx {
            eprintln!("{line}");
        }
    });
}

fn healthy(c: &Config) -> String {
    let until = Instant::now() + Duration::from_secs(60);
    loop {
        let (text, ok) = serve::health(c).unwrap();
        if ok {
            return text;
        }
        assert!(Instant::now() < until, "never healthy:\n{text}");
        std::thread::yield_now();
    }
}

#[test]
fn t_t1_13_1_three_members_elect_refuse_strangers_and_lose_nothing_split() {
    let dir = tempfile::tempdir().unwrap();
    let configs = three(dir.path());
    start(&configs);
    let c = &configs[0];
    let text = healthy(c);
    assert!(text.ends_with("healthy\n"), "{text}");

    // A client without the secret is refused before its first request.
    let wrong = Keys::plain(b"not-the-cluster-secret-not-the-cluster-secret".to_vec());
    let refused =
        Client::connect(&c.members[0].addr, &wrong, c.deadline).and_then(|mut k| k.ask("status"));
    assert!(refused.is_err(), "{refused:?}");

    // Split member 3 off at its transport, write under load, heal, count.
    let keys = c.keys().unwrap();
    let mut three = Client::connect(&c.members[2].addr, &keys, c.deadline).unwrap();
    three.ask("cut on").unwrap();
    let ledger = dir.path().join("ledger");
    let summary = serve::load(c, "split", 2, Duration::from_secs(1), &ledger).unwrap();
    assert!(summary.contains("acknowledged"), "{summary}");
    three.ask("cut off").unwrap();
    let (report, ok) = serve::check(c, &ledger, Duration::from_secs(60)).unwrap();
    assert!(ok, "{summary}{report}");
    assert!(report.contains("nothing lost"), "{report}");
    let acked: u64 = std::fs::read_to_string(&ledger)
        .unwrap()
        .lines()
        .map(|l| l.split_whitespace().nth(2).unwrap().parse::<u64>().unwrap())
        .sum();
    assert!(acked > 0, "{summary}");
}

/// A region lost (T-T1.13.1): three voters each in a region of their own and a spare in a fourth;
/// the third region's member is lost for good (its records cut at its transport, as a region whose
/// network is gone), the spare added as a learner, promoted, and the lost member removed, while the
/// two left keep the majority. The cluster the config then names is healthy and holds every write.
#[test]
fn t_t1_13_1_a_region_lost_is_replaced_by_a_spare() {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    std::fs::write(
        &secret,
        "0123456789abcdef0123456789abcdef-test-secret
",
    )
    .unwrap();
    let members = (1..=4)
        .map(|n| format!("{n}@127.0.0.1:{}", free_port()))
        .collect::<Vec<_>>()
        .join(",");
    let config = |n: u64, voters: &str| {
        let text = format!(
            "node = {n}
members = {members}
voters = {voters}
data = {}
class = persisted
secret_file = {}
deadline_ms = 5000
drills = on
",
            dir.path().join(format!("m{n}")).display(),
            secret.display()
        );
        Config::parse(&text).unwrap()
    };
    for n in 1..=4 {
        let c = config(n, "1,2,3");
        let (up, listening) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let ended = serve::serve(&c, move |_| {
                let _ = up.send(());
            });
            eprintln!("member {n} ended: {ended:?}");
        });
        let _ = listening.recv();
    }
    let before = config(1, "1,2,3");
    healthy(&before);
    let key = before.keys().unwrap();
    let ask = |n: usize, request: &str| {
        Client::connect(&before.members[n - 1].addr, &key, before.deadline)
            .unwrap()
            .ask(request)
            .unwrap()
    };
    ask(1, "exec CREATE TABLE t(x INTEGER)");
    ask(1, "exec INSERT INTO t VALUES (1)");

    // The third region goes: its member hears nothing and is heard by no one, from now on.
    ask(3, "cut on");
    let path = dir.path().join("member.conf");
    std::fs::write(
        &path,
        format!(
            "node = 1
members = {members}
voters = 1,2,3
data = {}
class = persisted
secret_file = {}
deadline_ms = 5000
",
            dir.path().join("m1").display(),
            secret.display()
        ),
    )
    .unwrap();
    let member = |what: &str, n: &str| {
        let args = ["member", path.to_str().unwrap(), what, n].map(str::to_owned);
        let reply = serve::command(&args, |_| {}).unwrap();
        assert_eq!(reply.status, 0, "member {what} {n}: {}", reply.text);
        reply.text
    };
    member("add", "4");
    member("promote", "4");
    let after = member("remove", "3");
    assert!(after.contains("voters=1,2,4"), "{after}");

    let now = config(1, "1,2,4");
    healthy(&now);
    ask(1, "exec INSERT INTO t VALUES (2)");
    let until = Instant::now() + Duration::from_secs(60);
    loop {
        if ask(4, "count SELECT count(*) FROM t") == "2" {
            break;
        }
        assert!(Instant::now() < until, "the spare never held both writes");
        std::thread::yield_now();
    }
}

/// An authority for tests: one root, a certificate per member naming 127.0.0.1, as members on one
/// machine are reached (Ed25519, as the owner transport's tests issue them).
struct Authority {
    issuer: rcgen::Issuer<'static, rcgen::KeyPair>,
    root: String,
}

impl Authority {
    fn new() -> Self {
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
        let root = params.self_signed(&key).unwrap().pem();
        Self {
            issuer: rcgen::Issuer::new(params, key),
            root,
        }
    }

    /// The config lines of a member whose files, under `dir` and named for `who`, this authority
    /// issued.
    fn lines(&self, dir: &std::path::Path, who: &str) -> String {
        let params = rcgen::CertificateParams::new(vec!["127.0.0.1".to_owned()]).unwrap();
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
        let certificate = params.signed_by(&key, &self.issuer).unwrap();
        let (cert, key_file, roots) = (
            dir.join(format!("{who}.pem")),
            dir.join(format!("{who}.key")),
            dir.join(format!("{who}-roots.pem")),
        );
        std::fs::write(&cert, certificate.pem()).unwrap();
        std::fs::write(&key_file, key.serialize_pem()).unwrap();
        std::fs::write(&roots, &self.root).unwrap();
        format!(
            "cert_file = {}\nkey_file = {}\nroots_file = {}\n",
            cert.display(),
            key_file.display(),
            roots.display()
        )
    }
}

/// CP-T1.13 (T-T1.13.1): with cert_file, key_file and roots_file, every link runs inside mutual
/// TLS: the three elect over it, a client of the config is answered, and a client with the secret
/// but no TLS, or with a certificate from another authority, is refused before its first request.
#[test]
fn t_t1_13_1_links_run_inside_mutual_tls() {
    let dir = tempfile::tempdir().unwrap();
    let ours = Authority::new();
    let configs = three_with(dir.path(), |n| {
        ours.lines(dir.path(), &format!("member-{n}"))
    });
    start(&configs);
    let c = &configs[0];
    let text = healthy(c);
    assert!(text.ends_with("healthy\n"), "{text}");
    let keys = c.keys().unwrap();
    let mut client = Client::connect(&c.members[1].addr, &keys, c.deadline).unwrap();
    client.ask("exec CREATE TABLE t(x INTEGER)").unwrap();
    client.ask("exec INSERT INTO t VALUES (1)").unwrap();
    assert_eq!(client.ask("count SELECT count(*) FROM t").unwrap(), "1");

    let plain = Keys::plain(c.secret().unwrap());
    let refused =
        Client::connect(&c.members[0].addr, &plain, c.deadline).and_then(|mut k| k.ask("status"));
    assert!(
        refused.is_err(),
        "a client without TLS was answered: {refused:?}"
    );

    let theirs = Authority::new();
    let stranger = Config::parse(&format!(
        "node = 1\nmembers = {}\ndata = {}\nclass = persisted\nsecret_file = {}\ndeadline_ms = 5000\n{}",
        c.members
            .iter()
            .map(|m| format!("{}@{}", m.node, m.addr))
            .collect::<Vec<_>>()
            .join(","),
        dir.path().join("stranger").display(),
        c.secret_file.display(),
        theirs.lines(dir.path(), "stranger"),
    ))
    .unwrap();
    let refused = Client::connect(&c.members[0].addr, &stranger.keys().unwrap(), c.deadline)
        .and_then(|mut k| k.ask("status"));
    assert!(
        refused.is_err(),
        "another authority's client was answered: {refused:?}"
    );
}

/// A certificate renewed without a restart (CP-T1.13): each member's TLS files replaced in place,
/// then `reload` asked of each over a link made with the old keys; from then on the members present
/// the new certificates and verify under the new roots, so a client of the new files is answered
/// and one of the old is refused. (The new files come from another authority here only so the
/// switch shows; `scripts/cluster.sh renew` reissues from the same one.)
#[test]
fn cp_t1_13_a_renewed_certificate_is_taken_up_by_reload_without_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let before = Authority::new();
    let configs = three_with(dir.path(), |n| {
        before.lines(dir.path(), &format!("member-{n}"))
    });
    start(&configs);
    let c = &configs[0];
    healthy(c);
    let old = c.keys().unwrap();

    // The renewal: every member's files replaced where its config names them.
    let after = Authority::new();
    for n in 1..=3 {
        after.lines(dir.path(), &format!("member-{n}"));
    }
    let new = c.keys().unwrap();
    let early =
        Client::connect(&c.members[0].addr, &new, c.deadline).and_then(|mut k| k.ask("status"));
    assert!(early.is_err(), "answered before its reload: {early:?}");

    for m in &c.members {
        Client::connect(&m.addr, &old, c.deadline)
            .unwrap()
            .ask("reload")
            .unwrap();
    }
    for m in &c.members {
        let status = Client::connect(&m.addr, &new, c.deadline)
            .and_then(|mut k| k.ask("status"))
            .unwrap();
        assert!(status.contains("leader="), "{status}");
        let stale = Client::connect(&m.addr, &old, c.deadline).and_then(|mut k| k.ask("status"));
        assert!(
            stale.is_err(),
            "the old certificate still answered: {stale:?}"
        );
    }
    let text = healthy(c);
    assert!(text.ends_with("healthy\n"), "{text}");
}

/// One member started on its own, and a client's `leader` request to it (CP-T1.13): with one of
/// three voters up there is no leader, so a request whose deadline passes is refused, and one made
/// then is answered, with the leader's ID and address, once the other two start and the group
/// elects. The answer comes from the step that names the leader: no client retries, no polling.
#[test]
fn cp_t1_13_a_leader_request_made_during_an_election_is_answered_once_it_is_chosen() {
    let dir = tempfile::tempdir().unwrap();
    let configs = three(dir.path());
    let one = |c: Config| {
        let (up, listening) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let node = c.node;
            let ended = serve::serve(&c, move |_| {
                let _ = up.send(());
            });
            eprintln!("member {node} ended: {ended:?}");
        });
        listening.recv().unwrap();
    };
    one(configs[0].clone());
    let c = &configs[0];
    let keys = c.keys().unwrap();

    // One voter of three can't elect: a wait that ends first is refused with its reason.
    let alone = Client::connect(&c.members[0].addr, &keys, c.deadline)
        .unwrap()
        .leader(Duration::from_millis(1));
    let why = format!("{:#}", alone.unwrap_err());
    assert!(why.contains("no leader within 1 ms"), "{why}");

    // A request made while no leader can be chosen, answered once one is.
    let mut waiting = Client::connect(&c.members[0].addr, &keys, c.deadline).unwrap();
    let (sent, answered) = std::sync::mpsc::channel();
    let (asking, asked) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = asking.send(());
        let _ = sent.send(waiting.leader(Duration::from_secs(60)));
    });
    asked.recv().unwrap();
    assert!(
        answered.try_recv().is_err(),
        "answered before a majority was up"
    );
    for m in configs[1..].iter().cloned() {
        one(m);
    }
    let (id, addr) = answered.recv().unwrap().unwrap();
    let leader = c
        .members
        .iter()
        .find(|m| m.node == id)
        .unwrap_or_else(|| panic!("leader {id} isn't a member"));
    assert_eq!(addr, leader.addr);

    // Another member, asked once the group has a leader, names the one it follows.
    let (again, _) = Client::connect(&c.members[1].addr, &keys, c.deadline)
        .unwrap()
        .leader(Duration::from_secs(60))
        .unwrap();
    assert!(configs.iter().any(|m| m.node == again), "{again}");
}

/// A disk cut under load (T-T1.13.1, AC-T1.13.5): the leader's log disk goes away while writers
/// write; its replica fences on the first refused write or flush and the member ends, naming the
/// disk; the two left elect, keep taking writes, and hold every write acknowledged before, during
/// and after the cut. (Started again, a member finds its disk back and catches up from its own
/// durable log: scripts/drills.sh's `disk` drill, one member per process.)
#[test]
fn t_t1_13_1_a_leaders_disk_cut_under_load_loses_no_acknowledged_write() {
    let dir = tempfile::tempdir().unwrap();
    let configs = three(dir.path());
    let mut ended = std::collections::BTreeMap::new();
    for c in configs.clone() {
        let (up, listening) = std::sync::mpsc::channel();
        let (end, ends) = std::sync::mpsc::channel();
        let node = c.node;
        std::thread::spawn(move || {
            let _ = end.send(serve::serve(&c, move |_| {
                let _ = up.send(());
            }));
        });
        listening.recv().unwrap();
        ended.insert(node, ends);
    }
    let c = &configs[0];
    healthy(c);
    let keys = c.keys().unwrap();
    let (leader, addr) = Client::connect(&c.members[0].addr, &keys, c.deadline)
        .unwrap()
        .leader(Duration::from_secs(60))
        .unwrap();

    // Writes before the cut, then the leader's disk cut while two writers write through it.
    let before = dir.path().join("before");
    serve::load(c, "before", 2, Duration::from_millis(200), &before).unwrap();
    Client::connect(&addr, &keys, c.deadline)
        .unwrap()
        .ask("disk cut")
        .unwrap();
    let during = dir.path().join("during");
    let summary = serve::load(c, "during", 2, Duration::from_secs(2), &during).unwrap();

    // The cut member ended on its own, fenced (the receive's bound is a hang guard).
    let why = ended[&leader]
        .recv_timeout(Duration::from_secs(60))
        .expect("the member whose disk was cut never ended");
    let why = why.expect_err("a member whose disk was cut ran on");
    eprintln!("member {leader} ended: {why:#}");

    // Every acknowledged write, before and during the cut, on both members left.
    let mut left = c.clone();
    left.voters = configs
        .iter()
        .map(|m| m.node)
        .filter(|n| *n != leader)
        .collect();
    for ledger in [&before, &during] {
        let (report, ok) = serve::check(&left, ledger, Duration::from_secs(60)).unwrap();
        assert!(ok, "{summary}{report}");
        assert!(report.contains("nothing lost"), "{report}");
    }
    let acked: u64 = std::fs::read_to_string(&during)
        .unwrap()
        .lines()
        .map(|l| l.split_whitespace().nth(2).unwrap().parse::<u64>().unwrap())
        .sum();
    assert!(acked > 0, "the two left took no writes: {summary}");
}

/// A disk hung under load (T-T1.13.1, AC-T1.13.5): the leader's log disk stops answering while
/// writers write, its network going on (so it can keep its lease while nothing it takes becomes
/// durable), then comes back; every acknowledged write is then on every member.
#[test]
fn t_t1_13_1_a_leaders_disk_hung_under_load_loses_no_acknowledged_write() {
    let dir = tempfile::tempdir().unwrap();
    let configs = three(dir.path());
    for c in configs.clone() {
        let (up, listening) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let node = c.node;
            let ended = serve::serve(&c, move |_| {
                let _ = up.send(());
            });
            eprintln!("member {node} ended: {ended:?}");
        });
        listening.recv().unwrap();
    }
    let c = configs[0].clone();
    healthy(&c);
    let keys = c.keys().unwrap();
    let (_, addr) = Client::connect(&c.members[0].addr, &keys, c.deadline)
        .unwrap()
        .leader(Duration::from_secs(60))
        .unwrap();
    let mut leader = Client::connect(&addr, &keys, c.deadline).unwrap();
    let ledger = dir.path().join("hang");
    let load = {
        let (c, ledger) = (c.clone(), ledger.clone());
        std::thread::spawn(move || serve::load(&c, "hang", 2, Duration::from_secs(3), &ledger))
    };
    leader.ask("disk hang").unwrap();
    // The hang holds the load: back only once a write of the load waits on the hung disk.
    let held = leader.ask("disk held").unwrap();
    assert!(held.starts_with("held=") && held != "held=0", "{held}");
    leader.ask("disk back").unwrap();
    let summary = load.join().unwrap().unwrap();
    healthy(&c);
    let (report, ok) = serve::check(&c, &ledger, Duration::from_secs(60)).unwrap();
    assert!(ok, "{summary}{report}");
    assert!(report.contains("nothing lost"), "{report}");
}

/// A refused log write ends the member (CP-T1.13, T-T1.13.1), traced through the replica: a lone
/// voter's log disk cut, its next write fails at hyper-log (the device's I/O error, `Fault::Failed`),
/// hyper-durable fences the replica on it, the owner ends on the fence, and the drain the member's
/// sender waits in (`ReplicaHandle::drain_wait`, `serve`'s loop) answers Fenced at once, woken by
/// the fence, not at its deadline, so `serve` returns and the process ends.
#[test]
fn t_t1_13_1_a_refused_log_write_ends_the_replica_and_wakes_its_drain() {
    use amphora::database::{Database, LogDisk};
    use amphora_api::{Refusal, replication::ReplicaConfig};
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("raft.log");
    // A hang guard on the calls; the drain's deadline is the one the verdict reads.
    let until = || Instant::now() + Duration::from_secs(600);
    let config = ReplicaConfig {
        node_id: 1,
        voters: vec![1],
        learners: vec![],
        bootstrap: None,
    };
    let db =
        Database::open_replicated(&dir.path().join("state.db"), &log, config, until()).unwrap();
    let handle = db.replica().unwrap();
    handle.wait_leader(0, until()).unwrap();
    db.call(until(), |c| {
        c.execute_batch("CREATE TABLE t(v INTEGER); INSERT INTO t VALUES(1)")?;
        Ok(())
    })
    .unwrap();

    // The member's sender waits for its next record; a lone voter sends none.
    let drain_until = until();
    let (waiting, waits) = std::sync::mpsc::channel();
    let drain = {
        let handle = handle.clone();
        std::thread::spawn(move || {
            waiting.send(()).unwrap();
            let drained = handle.drain_wait(1, drain_until);
            (drained, Instant::now())
        })
    };
    waits.recv().unwrap();

    Database::set_log_disk(&log, LogDisk::Cut).unwrap();
    let refused = db.call(until(), |c| {
        c.execute_batch("INSERT INTO t VALUES(2)")?;
        Ok(())
    });
    // Fenced, or UnknownOutcome when the fence ends the owner with the write pending.
    assert!(
        matches!(refused, Err(Refusal::Fenced(_) | Refusal::UnknownOutcome)),
        "a write on a cut disk: {refused:?}"
    );
    let (drained, at) = drain.join().unwrap();
    assert!(
        matches!(&drained, Err(Refusal::Fenced(_))),
        "the drain of a fenced replica: {drained:?}"
    );
    assert!(
        at < drain_until,
        "the drain waited out its deadline: the fence didn't wake it"
    );
    let ended = handle.status();
    assert!(
        matches!(&ended, Err(Refusal::Fenced(_))),
        "the replica's status after the refused write: {ended:?}"
    );
}

/// A hung disk waits with no time limit, so only a drill cluster may ask for one (CP-T1.13,
/// T-T1.13.1): a member whose config says `drills = off` refuses `disk` (and `cut`), naming the
/// switch, and its log's disk is untouched: its writes go on.
#[test]
fn t_t1_13_1_a_member_without_drills_refuses_the_disk_request() {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    std::fs::write(&secret, "0123456789abcdef0123456789abcdef-test-secret\n").unwrap();
    let text = format!(
        "node = 1\nmembers = 1@127.0.0.1:{}\ndata = {}\nclass = persisted\nsecret_file = {}\ndeadline_ms = 5000\ndrills = off\n",
        free_port(),
        dir.path().join("m1").display(),
        secret.display()
    );
    let c = Config::parse(&text).unwrap();
    assert!(!c.drills);
    let (up, listening) = std::sync::mpsc::channel();
    {
        let c = c.clone();
        std::thread::spawn(move || {
            let ended = serve::serve(&c, move |_| {
                let _ = up.send(());
            });
            eprintln!("member 1 ended: {ended:?}");
        });
    }
    listening.recv().unwrap();
    let keys = c.keys().unwrap();
    let mut member = Client::connect(&c.members[0].addr, &keys, c.deadline).unwrap();
    member.leader(Duration::from_secs(60)).unwrap();
    for request in ["disk hang", "disk cut", "disk back", "cut on"] {
        let refused = member
            .ask(request)
            .expect_err("a member without drills took a drill's request");
        assert!(
            format!("{refused:#}").contains("drills are off"),
            "{request}: {refused:#}"
        );
    }
    member
        .ask("exec CREATE TABLE t(v INTEGER); INSERT INTO t VALUES(1)")
        .unwrap();
    assert_eq!(
        member.ask("count SELECT count(*) FROM t").unwrap().trim(),
        "1"
    );
}

/// CP-T1.13, AC-T1.2.3 (docs/design/client-protocol.md, decision 2): rows from `amphora serve`.
/// Do: three members; on the leader's `Q` connection, create a table and insert a row of every
/// SQLite type through bound parameters; then a write whose second statement the engine refuses;
/// then read back, once with a bound parameter. Expect: each statement's table, the insert's
/// count and rowid; the refused write typed as its statement, with its first statement rolled
/// back too; the rows typed as written with their columns' names; the connection answering after
/// the refusal; a read and a write whose answers pass one frame answered whole (version 2); and a
/// link without the cluster's secret refused before its first request.
#[test]
fn cp_t1_13_sql_takes_bound_parameters_and_answers_typed_rows_with_their_names() {
    let dir = tempfile::tempdir().unwrap();
    let configs = three(dir.path());
    start(&configs);
    let c = &configs[0];
    healthy(c);
    let keys = c.keys().unwrap();
    let (_, leader) = Client::connect(&c.members[0].addr, &keys, c.deadline)
        .unwrap()
        .leader(Duration::from_secs(60))
        .unwrap();
    let mut sql = SqlClient::connect(&leader, &keys, c.deadline).unwrap();
    let st = |text: &str, params: Vec<Value>| Statement {
        sql: text.to_owned(),
        params,
    };
    let row = vec![
        Value::Integer(7),
        Value::Real(2.5),
        Value::Text("seven".into()),
        Value::Blob(vec![0, 255]),
        Value::Null,
    ];

    let made = sql
        .write(vec![st("CREATE TABLE t (id INTEGER PRIMARY KEY, r REAL, s TEXT, b BLOB, n)", vec![])])
        .unwrap();
    assert_eq!(made.len(), 1, "{made:?}");
    let wrote = sql
        .write(vec![st("INSERT INTO t VALUES (?1, ?2, ?3, ?4, ?5)", row.clone())])
        .unwrap();
    assert_eq!((wrote[0].affected, wrote[0].last_insert_rowid), (1, 7), "{wrote:?}");

    let refused = sql
        .write(vec![
            st("INSERT INTO t (id) VALUES (8)", vec![]),
            st("INSERT INTO nowhere VALUES (1)", vec![]),
        ])
        .unwrap_err();
    match refused.downcast_ref::<Refusal>() {
        Some(Refusal::Invalid { what, .. }) => assert_eq!(what, "statement 1"),
        other => panic!("refused as {other:?}: {refused:#}"),
    }

    let read = sql
        .read(st("SELECT id, r, s, b, n FROM t ORDER BY id", vec![]))
        .unwrap();
    assert_eq!(
        read,
        Table {
            columns: ["id", "r", "s", "b", "n"].map(String::from).to_vec(),
            rows: vec![row],
            affected: read.affected,
            last_insert_rowid: read.last_insert_rowid,
        }
    );
    let one = sql
        .read(st("SELECT s AS name FROM t WHERE id = ?1", vec![Value::Integer(7)]))
        .unwrap();
    assert_eq!(
        (one.columns, one.rows),
        (vec!["name".to_owned()], vec![vec![Value::Text("seven".into())]])
    );

    // Version 2: a read and a write whose answers pass one frame come whole, in order.
    sql.write(vec![st("CREATE TABLE big (id INTEGER PRIMARY KEY, v BLOB)", vec![])])
        .unwrap();
    let mib = 1 << 20;
    for half in 0..2i64 {
        let rows = (1..=20i64)
            .map(|n| st("INSERT INTO big VALUES (?1, ?2)", vec![Value::Integer(half * 20 + n), Value::Blob(vec![9; mib])]))
            .collect();
        sql.write(rows).unwrap();
    }
    let ids = |table: &Table| table.rows.iter().map(|row| row[0].clone()).collect::<Vec<_>>();
    let every = (1..=40).map(Value::Integer).collect::<Vec<_>>();
    let big = sql.read(st("SELECT id, v FROM big ORDER BY id", vec![])).unwrap();
    assert_eq!(ids(&big), every);
    assert!(big.rows.iter().all(|row| row[1] == Value::Blob(vec![9; mib])));
    let held = sql
        .write(vec![
            st("INSERT INTO t (id) VALUES (9)", vec![]),
            st("SELECT id, v FROM big ORDER BY id", vec![]),
        ])
        .unwrap();
    assert_eq!((held[0].affected, ids(&held[1])), (1, every));

    let wrong = Keys::plain(b"not-the-cluster-secret-not-the-cluster-secret".to_vec());
    assert!(SqlClient::connect(&leader, &wrong, c.deadline).is_err());
}

#[test]
fn p5_probe_large_replicated_writes() {
    let _ = tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).with_ansi(false).with_writer(std::io::stderr).try_init();
    let sizes: Vec<i64> = std::env::var("P5_SIZES").unwrap_or("1,2,4,8".into()).split(',').map(|s| s.parse().unwrap()).collect();
    for count in sizes {
        let dir = tempfile::tempdir().unwrap();
        let configs = three(dir.path());
        start(&configs);
        let c = &configs[0];
        healthy(c);
        let keys = c.keys().unwrap();
        let (_, leader) = Client::connect(&c.members[0].addr, &keys, c.deadline).unwrap().leader(Duration::from_secs(60)).unwrap();
        let mut sql = SqlClient::connect(&leader, &keys, c.deadline).unwrap();
        let st = |text: &str, params: Vec<Value>| Statement { sql: text.to_owned(), params };
        sql.write(vec![st("CREATE TABLE big (id INTEGER PRIMARY KEY, v BLOB)", vec![])]).unwrap();
        let mib = 1 << 20;
        let status = |when: &str| for m in &c.members { eprintln!("P5PROBE {when} {}: {:?}", m.addr, Client::connect(&m.addr, &keys, c.deadline).and_then(|mut k| k.ask("status")).map(|s| s.replace('\n', " | "))); };
        status("before");
        eprintln!("P5PROBE begin {count} MiB at leader {leader}");
        let rows = (0..count).map(|n| st("INSERT INTO big VALUES (?1, ?2)", vec![Value::Integer(n), Value::Blob(vec![9; mib])])).collect();
        let began = Instant::now();
        let got = sql.write(rows);
        eprintln!("P5PROBE write {count} MiB: {:?} after {:?}", got.as_ref().map(|t| t.len()).map_err(|e| format!("{e:#}")), began.elapsed());
        status("after");
        let mut sql = SqlClient::connect(&leader, &keys, c.deadline).unwrap();
        let began = Instant::now();
        let held = sql.write(vec![st("INSERT INTO big (id) VALUES (100000)", vec![]), st("SELECT id, v FROM big ORDER BY id", vec![])]);
        eprintln!("P5PROBE then write+select: {:?} after {:?}", held.as_ref().map(|t| t.len()).map_err(|e| format!("{e:#}")), began.elapsed());
        let (_, now) = Client::connect(&c.members[0].addr, &keys, c.deadline).unwrap().leader(Duration::from_secs(60)).unwrap();
        eprintln!("P5PROBE end {count} MiB leader now {now}");
    }
}

#[test]
fn p5_probe_replicated_write_answering_40_mib() {
    let dir = tempfile::tempdir().unwrap();
    let configs = three(dir.path());
    start(&configs);
    let c = &configs[0];
    healthy(c);
    let keys = c.keys().unwrap();
    let (_, leader) = Client::connect(&c.members[0].addr, &keys, c.deadline).unwrap().leader(Duration::from_secs(60)).unwrap();
    let mut sql = SqlClient::connect(&leader, &keys, c.deadline).unwrap();
    let st = |text: &str, params: Vec<Value>| Statement { sql: text.to_owned(), params };
    let status = |when: &str| for m in &c.members { eprintln!("P5PROBE40 {when} {}: {:?}", m.addr, Client::connect(&m.addr, &keys, c.deadline).and_then(|mut k| k.ask("status")).map(|s| s.replace('\n', " | "))); };
    sql.write(vec![st("CREATE TABLE big (id INTEGER PRIMARY KEY, v BLOB)", vec![])]).unwrap();
    let mib = 1 << 20;
    for part in 0..10i64 {
        let rows = (0..4i64).map(|n| st("INSERT INTO big VALUES (?1, ?2)", vec![Value::Integer(part * 4 + n), Value::Blob(vec![9; mib])])).collect();
        let began = Instant::now();
        let got = sql.write(rows);
        eprintln!("P5PROBE40 part {part}: {:?} after {:?}", got.as_ref().map(|t| t.len()).map_err(|e| format!("{e:#}")), began.elapsed());
    }
    let began = Instant::now();
    let read = sql.read(st("SELECT id, v FROM big ORDER BY id", vec![]));
    eprintln!("P5PROBE40 read: {:?} after {:?}", read.as_ref().map(|t| t.rows.len()).map_err(|e| format!("{e:#}")), began.elapsed());
    status("before");
    let mut sql = SqlClient::connect(&leader, &keys, c.deadline).unwrap();
    let began = Instant::now();
    let held = sql.write(vec![st("INSERT INTO big (id) VALUES (100000)", vec![]), st("SELECT id, v FROM big ORDER BY id", vec![])]);
    eprintln!("P5PROBE40 write+select: {:?} after {:?}", held.as_ref().map(|t| t.iter().map(|t| t.rows.len()).collect::<Vec<_>>()).map_err(|e| format!("{e:#}")), began.elapsed());
    status("after");
}
