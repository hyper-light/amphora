# Lane B26: a test that restores every second of the archive old_write.py kept, then opens the database it
# was of backed up to it again, ends a base, writes on and restores those seconds, for either arm.
p='crates/amphora/tests/backup.rs'
s=open(p).read()
s+='''
#[test]
fn b26_old_base() {
    let from = std::path::PathBuf::from(std::env::var("B26_OLD").unwrap());
    let dir = tempfile::tempdir().unwrap();
    let copied = std::process::Command::new("cp").arg("-a").arg(from.join(".")).arg(dir.path()).status().unwrap();
    assert!(copied.success());
    let blobs = backup::FolderBlobs::open(&dir.path().join("archive"), 64 * 1024).unwrap();
    let mut history: Vec<(u64, Rows)> = std::fs::read_to_string(from.join("history.txt")).unwrap().lines().map(|l| {
        let (ns, ids) = l.split_once(' ').unwrap();
        (ns.parse().unwrap(), ids.split(',').filter(|i| !i.is_empty()).map(|i| (i.parse().unwrap(), String::new())).collect())
    }).collect();
    let survey = |what: &str, first: u64, last: u64, history: &[(u64, Rows)]| {
        let (mut whole, mut short, mut refused) = (Vec::new(), Vec::new(), BTreeMap::<String, Vec<u64>>::new());
        for second in first..=last {
            let at_second = tempfile::tempdir().unwrap();
            let target = at_second.path().join("restored.db");
            let expected: Vec<i64> = at(history, second).map(|r| r.keys().copied().collect()).unwrap_or_default();
            match backup::restore(&blobs, "db", second, &target, until()) {
                Ok(_) => {
                    let held: Vec<i64> = rows(&Db::open(&target).unwrap()).into_keys().collect();
                    if held == expected { whole.push(second) } else { short.push((second, held.len(), expected.len())) }
                }
                Err(e) => refused.entry(format!("{e:#}")).or_default().push(second),
            }
        }
        println!("B26 {what}: seconds {first}..={last}: {} whole ({:?}..{:?}), {} short with no error {:?}", whole.len(), whole.first(), whole.last(), short.len(), short);
        for (why, seconds) in refused { println!("B26 {what}: {} refused ({:?}..{:?}): {why}", seconds.len(), seconds.first(), seconds.last()); }
        short.len()
    };
    let last = history.last().unwrap().0.div_ceil(SECOND_NS);
    let before = survey("the base made before", last, last + 1, &history);
    let now = Arc::new(AtomicU64::new((last + 3) * SECOND_NS));
    let read = Arc::clone(&now);
    let hooks = Hooks::new(Arc::new(move || read.load(Ordering::SeqCst)), 1 << 16);
    let mut db = Db::open(&dir.path().join("source.db")).unwrap();
    db.back_up(Arc::clone(&hooks));
    let mut shipper = Shipper::open("db", Arc::clone(&hooks), &blobs, &db, until()).unwrap();
    println!("B26 opened again: rows {}, a base needed {}", rows(&db).len(), shipper.base_needed());
    let mut model = history.last().unwrap().1.clone();
    for id in 100..103 {
        now.fetch_add(SECOND_NS, Ordering::SeqCst);
        db.transaction(|db| { db.query_owned("INSERT INTO t (id, v) VALUES (?1, 'w')", [Value::from_i64(id)], |_| Ok(true)).map(|_| ()) }).unwrap();
        model.insert(id, String::new());
        history.push((now.load(Ordering::SeqCst), model.clone()));
    }
    shipper.ship(&db, &blobs, until()).unwrap();
    let _ = db.checkpoint().unwrap();
    let mut base = shipper.begin_base(&db, &blobs, &amphora::memory::process(), 32 * 1024).unwrap();
    while shipper.copy_base(&db, &blobs, &mut base, until()).unwrap() {}
    println!("B26 a base made after: {} chunks kept of the one before", base.reused());
    shipper.end_base(&db, &blobs, base, until()).unwrap();
    let based = now.load(Ordering::SeqCst).div_ceil(SECOND_NS);
    for id in 103..106 {
        now.fetch_add(SECOND_NS, Ordering::SeqCst);
        db.transaction(|db| { db.query_owned("INSERT INTO t (id, v) VALUES (?1, 'w')", [Value::from_i64(id)], |_| Ok(true)).map(|_| ()) }).unwrap();
        model.insert(id, String::new());
        history.push((now.load(Ordering::SeqCst), model.clone()));
    }
    now.fetch_add(SECOND_NS, Ordering::SeqCst);
    shipper.ship(&db, &blobs, until()).unwrap();
    shipper.ship(&db, &blobs, until()).unwrap();
    let last = history.last().unwrap().0.div_ceil(SECOND_NS);
    let after = survey("the base made after", based, last, &history);
    assert_eq!((before, after), (0, 0), "seconds restored short with no error");
}
'''
open(p,'w').write(s)
