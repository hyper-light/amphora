//! Session tokens (replication.md, "Decision (lane ryw-1): read-your-writes by session tokens";
//! kiln goal 8119f25d, AC-T1.10.R1, R2 and R4, T-T1.10.R5 to R8), on the driven shell: a session
//! reads its own writes on a lagging follower and across a change of leader, waits for its own
//! writes and no one else's, and a read with no token never waits.

use super::*;

/// An hour of a wall clock, in the stamps' microseconds: a follower whose clock is this far ahead
/// measures an apply lag of about an hour, so a token read there waits rather than redirects.
const HOUR: u64 = 3_600_000_000;

/// A token read of member_items through `client`.
fn read_token(client: &Client, token: Token) -> Result<Served<Vec<u8>>, Refusal> {
    let until = Instant::now() + HANG_READ;
    client.read_session(token, until, |db| {
        db.query_transaction(|db| match crate::replication::closed::recorded(db)? {
            Some(time) if time >= token.micros => Ok(Ok(values(db)?)),
            found if token.micros == 0 => {
                let _ = found;
                Ok(Ok(values(db)?))
            }
            found => Ok(Err(found)),
        })
    })
}

/// The call that writes `value` to member_items.
fn insert(value: u8) -> impl FnOnce(&mut Db) -> anyhow::Result<Vec<u8>> + Send + 'static {
    move |db| {
        db.exec("CREATE TABLE IF NOT EXISTS member_items(value)")?;
        db.exec(&format!("INSERT INTO member_items VALUES({value})"))?;
        Ok(Vec::new())
    }
}

/// Writes `value` through `client` and gives back the token its answer carries.
fn write_token(driven: &mut Driven, pump: &mut Pump<'_>, client: &Client, value: u8) -> Token {
    let answer = submit(client, driven.until(), insert(value));
    driven.answer(pump, &answer).unwrap().token.unwrap()
}

/// Every record to member `owner` lost from now on: it falls behind, and the group commits
/// without it.
fn cut_off(pump: &mut Pump<'_>, owner: usize) {
    let to = u64::try_from(owner + 1).unwrap();
    pump.losing = Some(Box::new(move |_, record: &Message| record.to == to));
}

/// Records reach every member again, and the leader's heartbeats (its ticks alone send them)
/// make good what `owner` lost, until it has applied all the leader has.
fn rejoin(
    driven: &mut Driven,
    pump: &mut Pump<'_>,
    clients: &[Client],
    leader: usize,
    owner: usize,
) {
    pump.losing = None;
    let applied = |n: usize| clients.get(n).unwrap().handle().status().unwrap().applied;
    for _ in 0..ROUNDS {
        if applied(owner) >= applied(leader) {
            return;
        }
        driven.tick(pump, leader);
    }
    panic!("owner {owner} did not rejoin in {ROUNDS} driven rounds");
}

/// T-T1.10.R5 (AC-T1.10.R1): a lagging follower. Do: three voters, the follower's clock an hour
/// ahead (an apply lag of about an hour measured); 1 written and caught up; 2 written, its token
/// taken while the follower hasn't applied it; a token read on the follower, on a thread; then the
/// follower caught up. Expect: the read waits (counted) and is answered with 1 and 2, never 1
/// alone; a read with no token on the follower answers at once; the token orders after 1's.
#[test]
fn t_t1_10_r5_a_session_reads_its_write_on_a_lagging_follower() {
    let dir = tempfile::tempdir().unwrap();
    let mut driven = Driven::new();
    let clients: Vec<Client> = (1..=3)
        .map(|n| driven.open(dir.path(), n, vec![1, 2, 3], None))
        .collect();
    let mut pump = Pump::new(&clients);
    let leader = driven.elect(&mut pump, &clients, 0, 0);
    let follower = (leader + 1) % clients.len();
    let wall = driven.owner(follower).wall;
    driven.owner_mut(follower).wall = wall + HOUR;
    let first = write_token(&mut driven, &mut pump, clients.get(leader).unwrap(), 1);
    for owner in 0..clients.len() {
        catch_up(&mut driven, &mut pump, &clients, leader, owner);
    }
    cut_off(&mut pump, follower);
    let second = write_token(&mut driven, &mut pump, clients.get(leader).unwrap(), 2);
    assert!(
        first < second,
        "tokens {first:?} and {second:?} out of the log's order"
    );
    let behind = clients
        .get(follower)
        .unwrap()
        .handle()
        .status()
        .unwrap()
        .applied
        < second.index;
    // A read with no token never waits: the follower answers what it has.
    let local = clients
        .get(follower)
        .unwrap()
        .read_local(Instant::now() + HANG_READ, values)
        .unwrap();
    assert!(local == vec![1] || local == vec![1, 2], "{local:?}");
    let reader = clients.get(follower).unwrap().clone();
    let spawned = Instant::now();
    // Diag (A24): the reader thread starts this late, as a loaded machine would start it.
    let late = std::env::var("A24_READER_DELAY_US")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let waiting = std::thread::spawn(move || {
        if late > 0 {
            std::thread::sleep(Duration::from_micros(late));
        }
        let started = Instant::now();
        let at_start = reader.closed_token();
        let served = read_token(&reader, second);
        (started, at_start, served, Instant::now())
    });
    rejoin(&mut driven, &mut pump, &clients, leader, follower);
    let rejoined = Instant::now();
    for owner in 0..clients.len() {
        catch_up(&mut driven, &mut pump, &clients, leader, owner);
    }
    let caught = Instant::now();
    let (started, at_start, served, answered) = waiting.join().unwrap();
    let us = |at: Instant| at.saturating_duration_since(spawned).as_micros();
    eprintln!(
        "a24-r5 started_us={} rejoined_us={} caught_us={} answered_us={} second={second:?} at_start={at_start:?} counters={:?}",
        us(started),
        us(rejoined),
        us(caught),
        us(answered),
        clients.get(follower).unwrap().session_counters().unwrap(),
    );
    let served = served.unwrap();
    assert_eq!(served, Served::Here(vec![1, 2]));
    let counters = clients.get(follower).unwrap().session_counters().unwrap();
    assert!(counters.apply_lag_micros.is_some_and(|lag| lag >= HOUR / 2));
    if behind {
        assert_eq!(counters.waited, 1, "{counters:?}");
    }
    assert!(
        behind,
        "the follower had applied 2 before its read: nothing was tested"
    );
    driven.drain(&mut pump);
    pump.finish();
}

/// T-T1.10.R6 (AC-T1.10.R2): a replica behind its token that has measured no apply lag redirects
/// at once, and never answers without the write. Do: three voters; 1 written; a token read on a
/// follower that has applied nothing stamped. Expect: Redirect, counted; after catching up, the
/// same token is served there with 1.
#[test]
fn t_t1_10_r6_a_behind_replica_with_no_lag_learned_redirects() {
    let dir = tempfile::tempdir().unwrap();
    let mut driven = Driven::new();
    let clients: Vec<Client> = (1..=3)
        .map(|n| driven.open(dir.path(), n, vec![1, 2, 3], None))
        .collect();
    let mut pump = Pump::new(&clients);
    let leader = driven.elect(&mut pump, &clients, 0, 0);
    let cut = (leader + 1) % clients.len();
    cut_off(&mut pump, cut);
    let token = write_token(&mut driven, &mut pump, clients.get(leader).unwrap(), 1);
    // Leader: free, served at once.
    assert_eq!(
        read_token(clients.get(leader).unwrap(), token).unwrap(),
        Served::Here(vec![1])
    );
    let lagging = (0..clients.len())
        .filter(|owner| *owner != leader)
        .find(|owner| {
            clients
                .get(*owner)
                .unwrap()
                .handle()
                .status()
                .unwrap()
                .applied
                < token.index
        });
    if let Some(owner) = lagging {
        let client = clients.get(owner).unwrap();
        assert_eq!(read_token(client, token).unwrap(), Served::Redirect);
        assert_eq!(client.session_counters().unwrap().redirects, 1);
    }
    rejoin(&mut driven, &mut pump, &clients, leader, cut);
    for owner in 0..clients.len() {
        catch_up(&mut driven, &mut pump, &clients, leader, owner);
        assert_eq!(
            read_token(clients.get(owner).unwrap(), token).unwrap(),
            Served::Here(vec![1])
        );
    }
    assert!(
        lagging.is_some(),
        "every follower had applied 1: nothing was tested"
    );
    driven.drain(&mut pump);
    pump.finish();
}

/// T-T1.10.R7 (AC-T1.10.R1, R3): a change of leader. Do: three voters, each clock an hour ahead
/// of the one before it; the lead passed 1, 2, 3, 1, each leader writing the next value; after
/// each write the session (one token, the latest of its own) reads on every member, before and
/// after catching up. Expect: every read served sees every value the session wrote (never fewer),
/// and once caught up every member's answer equals the leader's state: the answer the control
/// (the leader's latest) gives.
#[test]
fn t_t1_10_r7_a_session_reads_its_writes_across_leader_changes() {
    let dir = tempfile::tempdir().unwrap();
    let mut driven = Driven::new();
    let clients: Vec<Client> = (1..=3)
        .map(|n| driven.open(dir.path(), n, vec![1, 2, 3], None))
        .collect();
    let wall = driven.owner(0).wall;
    for (owner, ahead) in [(1, HOUR), (2, 2 * HOUR)] {
        driven.owner_mut(owner).wall = wall + ahead;
    }
    let mut pump = Pump::new(&clients);
    let mut leader = driven.elect(&mut pump, &clients, 0, 0);
    let mut session = Token::default();
    let mut value = 0u8;
    let (mut served, mut redirected) = (0u32, 0u32);
    for next in [1usize, 2, 0, usize::MAX] {
        value += 1;
        let token = write_token(&mut driven, &mut pump, clients.get(leader).unwrap(), value);
        assert!(
            token > session,
            "a later write's token {token:?} not past {session:?}"
        );
        session = session.after(token);
        let want: Vec<u8> = (1..=value).collect();
        for (owner, client) in clients.iter().enumerate() {
            let caught = client.handle().status().unwrap().applied >= session.index;
            if !caught
                && client
                    .session_counters()
                    .unwrap()
                    .apply_lag_micros
                    .is_none()
            {
                assert_eq!(read_token(client, session).unwrap(), Served::Redirect);
                redirected += 1;
                continue;
            }
            if caught {
                assert_eq!(
                    read_token(client, session).unwrap(),
                    Served::Here(want.clone()),
                    "member {owner} at once"
                );
                served += 1;
            }
        }
        for owner in 0..clients.len() {
            catch_up(&mut driven, &mut pump, &clients, leader, owner);
        }
        let control = held(&mut driven, leader);
        assert_eq!(control, want);
        for client in &clients {
            assert_eq!(
                read_token(client, session).unwrap(),
                Served::Here(control.clone())
            );
            served += 1;
        }
        if next != usize::MAX {
            driven.transfer(&mut pump, &clients, leader, next);
            leader = next;
        }
    }
    assert!(served > 0);
    let _ = redirected;
    driven.drain(&mut pump);
    pump.finish();
}

/// T-T1.10.R8 (AC-T1.10.R4): concurrent writers, each session waiting only for its own writes.
/// Do: three voters, a follower's clock an hour ahead (so a read there waits rather than
/// redirects); sessions A and B submit 1 and 2 through the leader at once, and every member
/// catches up; the follower cut off, B writes 3. Expect: each answer carries its own write's
/// token (A's names 1's entry, before B's); on the follower, which holds A's write but not B's
/// last, A's read is served at once (counted at once, no wait, no redirect) though B wrote since;
/// B's read is answered once the follower rejoins, with 3.
#[test]
fn t_t1_10_r8_each_session_waits_only_for_its_own_writes() {
    let dir = tempfile::tempdir().unwrap();
    let mut driven = Driven::new();
    let clients: Vec<Client> = (1..=3)
        .map(|n| driven.open(dir.path(), n, vec![1, 2, 3], None))
        .collect();
    let mut pump = Pump::new(&clients);
    let leader = driven.elect(&mut pump, &clients, 0, 0);
    let follower = (leader + 1) % clients.len();
    let wall = driven.owner(follower).wall;
    driven.owner_mut(follower).wall = wall + HOUR;
    let writer = clients.get(leader).unwrap();
    // Both in flight at once: neither is answered before the other is submitted.
    let a = submit(writer, driven.until(), insert(1));
    let b = submit(writer, driven.until(), insert(2));
    let a = driven.answer(&mut pump, &a).unwrap().token.unwrap();
    let b = driven.answer(&mut pump, &b).unwrap().token.unwrap();
    assert!(
        a.index < b.index && a < b,
        "A's token {a:?} names a write at or after B's {b:?}"
    );
    for owner in 0..clients.len() {
        catch_up(&mut driven, &mut pump, &clients, leader, owner);
    }
    cut_off(&mut pump, follower);
    let b = b.after(write_token(&mut driven, &mut pump, writer, 3));
    let reader = clients.get(follower).unwrap();
    let behind = reader.handle().status().unwrap().applied < b.index;
    let before = reader.session_counters().unwrap();
    assert!(before.apply_lag_micros.is_some(), "{before:?}");
    assert_eq!(read_token(reader, a).unwrap(), Served::Here(vec![1, 2]));
    let after = reader.session_counters().unwrap();
    assert_eq!(after.at_once, before.at_once + 1, "{after:?}");
    assert_eq!(after.waited, before.waited, "{after:?}");
    assert_eq!(after.redirects, before.redirects, "{after:?}");
    let waiting = {
        let reader = reader.clone();
        std::thread::spawn(move || read_token(&reader, b))
    };
    rejoin(&mut driven, &mut pump, &clients, leader, follower);
    for owner in 0..clients.len() {
        catch_up(&mut driven, &mut pump, &clients, leader, owner);
    }
    assert_eq!(
        waiting.join().unwrap().unwrap(),
        Served::Here(vec![1, 2, 3])
    );
    assert!(
        behind,
        "the follower had applied B's 3 before A's read: nothing was tested"
    );
    driven.drain(&mut pump);
    pump.finish();
}
