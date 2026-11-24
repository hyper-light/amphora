# Lane V18: inserts the cost diagnostic test (v18_normal_path_cost) into replicated.rs of a tree that holds lane V18.
# usage: python3 cost.py crates/amphora/src/database/replicated.rs   (then cargo fmt --all, build --ignored v18_normal_path_cost)
import sys
p=sys.argv[1]; s=open(p).read()
marker="    /// T-T1.8.W1, the stale-base refusal's type (PostgreSQL's 40001)"
assert s.count(marker)==1
test='''    /// Lane V18's cost (a diagnostic, never for main): the messages a follower's write adds on the
    /// normal path. Prints, for writes made one at a time at member 3 with no loss and no clock moved,
    /// the records the pump moved by sender, receiver and type, and the forward's entries' bytes.
    #[test]
    #[ignore]
    fn v18_normal_path_cost() {
        for durability in [Durability::Persisted, three_fates()] {
            let dir = tempfile::tempdir().unwrap();
            let mut driven = Driven::new();
            let clients: Vec<Client> = (1..=3)
                .map(|n| driven.open_with(dir.path(), n, vec![1, 2, 3], None, durability.clone()))
                .collect();
            let mut pump = Pump::new(&clients);
            let leader = driven.elect(&mut pump, &clients, 0, 1);
            assert_eq!(leader, 0);
            driven
                .call(&mut pump, &clients[leader], |db| {
                    db.exec("CREATE TABLE events(id INTEGER NOT NULL)")?;
                    Ok(Vec::new())
                })
                .unwrap();
            for _ in 0..ROUNDS {
                driven.quiet(&mut pump);
                if driven.owner(2).pending.is_none() {
                    break;
                }
            }
            let seen: Arc<Mutex<BTreeMap<String, (u64, u64)>>> = Arc::default();
            let counting = Arc::clone(&seen);
            pump.losing = Some(Box::new(move |from, message: &Message| {
                let bytes: usize = message.entries.iter().map(|entry| entry.data.len()).sum();
                let mut seen = counting.lock().unwrap();
                let slot = seen.entry(format!("{from}->{} {:?}", message.to, message.msg_type)).or_default();
                slot.0 += 1;
                slot.1 += u64::try_from(bytes).unwrap();
                false
            }));
            const K: u64 = 20;
            let ended = |how| clients[leader].handle().counters().write_slot_ended(how);
            let before = crate::meter::SlotEnd::ALL.map(ended);
            for n in 0..K {
                driven
                    .call(&mut pump, &clients[2], move |db| {
                        db.exec(&format!("INSERT INTO events VALUES ({})", 100 + n))?;
                        Ok(Vec::new())
                    })
                    .unwrap();
            }
            pump.losing = None;
            let after = crate::meter::SlotEnd::ALL.map(ended);
            println!("V18 COST {durability:?}: slot ends {before:?} -> {after:?} over {K} follower writes");
            let seen = seen.lock().unwrap();
            let total: u64 = seen.values().map(|(n, _)| n).sum();
            for (key, (n, bytes)) in seen.iter() {
                println!("V18 COST   {key}: {:.2} a write, {:.1} entry bytes a write", *n as f64 / K as f64, *bytes as f64 / K as f64);
            }
            println!("V18 COST   all records: {:.2} a write ({total} over {K})", total as f64 / K as f64);
            driven.quiet(&mut pump);
            driven.drain(&mut pump);
            pump.finish();
        }
    }

'''
s=s.replace(marker,test+marker)
open(p,'w').write(s)
