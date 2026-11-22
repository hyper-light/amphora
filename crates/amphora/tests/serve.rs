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
    for c in configs.clone() {
        std::thread::spawn(move || serve::serve(&c, |_| {}));
    }
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
