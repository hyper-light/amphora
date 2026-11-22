//! CP-T1.13 (AC-T1.13.1, AC-T1.13.5, T-T1.13.1): three members of `amphora serve` in one
//! process, over loopback TCP: healthy once they elect, a wrong secret refused, and a drill's load
//! with one member split off at its transport losing no acknowledged write.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros
)]

use std::time::{Duration, Instant};

use amphora::serve::{self, Client, Config};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn three(dir: &std::path::Path) -> Vec<Config> {
    let secret = dir.join("secret");
    std::fs::write(&secret, "0123456789abcdef0123456789abcdef-test-secret\n").unwrap();
    let members = (1..=3)
        .map(|n| format!("{n}@127.0.0.1:{}", free_port()))
        .collect::<Vec<_>>()
        .join(",");
    (1..=3)
        .map(|n| {
            let text = format!("node = {n}\nmembers = {members}\ndata = {}\nclass = persisted\nsecret_file = {}\ndeadline_ms = 5000\ndrills = on\n", dir.join(format!("m{n}")).display(), secret.display());
            Config::parse(&text).unwrap()
        })
        .collect()
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
    let (tx, rx) = std::sync::mpsc::channel();
    // Each member starts once the one before it listens, as one member per process does in
    // production. (One process's opens of different databases no longer take turns, CP-T1.13.)
    for c in configs.clone() {
        let tx = tx.clone();
        let (up, listening) = std::sync::mpsc::channel();
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
        // A member whose open failed drops `up` unsent: its end is reported below, and health fails.
        let _ = listening.recv();
    }
    std::thread::spawn(move || {
        for line in rx {
            eprintln!("{line}");
        }
    });
    let c = &configs[0];
    let text = healthy(c);
    assert!(text.ends_with("healthy\n"), "{text}");

    // A client without the secret is refused before its first request.
    let wrong = b"not-the-cluster-secret-not-the-cluster-secret";
    let refused =
        Client::connect(&c.members[0].addr, wrong, c.deadline).and_then(|mut k| k.ask("status"));
    assert!(refused.is_err(), "{refused:?}");

    // Split member 3 off at its transport, write under load, heal, count.
    let secret = c.secret().unwrap();
    let mut three = Client::connect(&c.members[2].addr, &secret, c.deadline).unwrap();
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
    let key = before.secret().unwrap();
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

/// One member started on its own, and a client's `leader` request to it (CP-T1.13): with one of
/// three voters up there is no leader, so a request whose deadline passes is refused, and one made
/// then is answered, with the leader's ID and address, once the other two start and the group
/// elects. The answer comes from the step that names the leader: no client retries, no polling.
#[test]
fn cp_t1_13_a_leader_request_made_during_an_election_is_answered_once_it_is_chosen() {
    let dir = tempfile::tempdir().unwrap();
    let configs = three(dir.path());
    let start = |c: Config| {
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
    start(configs[0].clone());
    let c = &configs[0];
    let secret = c.secret().unwrap();

    // One voter of three can't elect: a wait that ends first is refused with its reason.
    let alone = Client::connect(&c.members[0].addr, &secret, c.deadline)
        .unwrap()
        .leader(Duration::from_millis(1));
    let why = format!("{:#}", alone.unwrap_err());
    assert!(why.contains("no leader within 1 ms"), "{why}");

    // A request made while no leader can be chosen, answered once one is.
    let mut waiting = Client::connect(&c.members[0].addr, &secret, c.deadline).unwrap();
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
        start(m);
    }
    let (id, addr) = answered.recv().unwrap().unwrap();
    let leader = c
        .members
        .iter()
        .find(|m| m.node == id)
        .unwrap_or_else(|| panic!("leader {id} isn't a member"));
    assert_eq!(addr, leader.addr);

    // Another member, asked once the group has a leader, names the one it follows.
    let (again, _) = Client::connect(&c.members[1].addr, &secret, c.deadline)
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
    let secret = c.secret().unwrap();
    let (leader, addr) = Client::connect(&c.members[0].addr, &secret, c.deadline)
        .unwrap()
        .leader(Duration::from_secs(60))
        .unwrap();

    // Writes before the cut, then the leader's disk cut while two writers write through it.
    let before = dir.path().join("before");
    serve::load(c, "before", 2, Duration::from_millis(200), &before).unwrap();
    Client::connect(&addr, &secret, c.deadline)
        .unwrap()
        .ask("disk on")
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
