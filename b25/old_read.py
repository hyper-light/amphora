# Lane B25: a test that restores every second of the archives old_write.py kept, for either arm.
p='crates/amphora/tests/backup.rs'
s=open(p).read()
s+='''
#[test]
fn b25_old_archives() {
    let from = std::env::var("B25_OLD_ARCHIVE").unwrap();
    let mut wrong = 0;
    for arm in ["r1", "r2"] {
        let kept = std::path::Path::new(&from).join(arm);
        let blobs = backup::FolderBlobs::open(&kept.join("archive"), 64 * 1024).unwrap();
        let text = std::fs::read_to_string(kept.join("history.txt")).unwrap();
        let mut lines = text.lines();
        let based: u64 = lines.next().unwrap().parse().unwrap();
        let history: Vec<(u64, Vec<i64>)> = lines
            .map(|l| {
                let (ns, ids) = l.split_once(' ').unwrap();
                let ids = ids
                    .split(',')
                    .filter(|i| !i.is_empty())
                    .map(|i| i.parse().unwrap())
                    .collect();
                (ns.parse().unwrap(), ids)
            })
            .collect();
        let last = history.last().unwrap().0.div_ceil(SECOND_NS);
        let dir = tempfile::tempdir().unwrap();
        let (mut whole, mut short, mut refused) = (Vec::new(), Vec::new(), BTreeMap::<String, Vec<u64>>::new());
        for second in based..=last {
            let target = dir.path().join(format!("at-{second}.db"));
            let expected = history
                .iter()
                .rfind(|(ns, _)| *ns <= second * SECOND_NS)
                .map(|(_, ids)| ids.clone())
                .unwrap_or_default();
            match backup::restore(&blobs, "db", second, &target, until()) {
                Ok(_) => {
                    let held: Vec<i64> = match std::panic::catch_unwind(|| rows(&Db::open(&target).unwrap())) {
                        Ok(rows) => rows.into_keys().collect(),
                        Err(_) => vec![-1],
                    };
                    if held == expected {
                        whole.push(second);
                    } else {
                        short.push((second, held.len(), expected.len()));
                    }
                }
                Err(e) => {
                    let kind: String = format!("{e:#}").chars().map(|c| if c.is_ascii_digit() { '#' } else { c }).collect();
                    let first = refused.entry(kind).or_default();
                    if first.is_empty() {
                        println!("{arm}: second {second}: {e:#}");
                    }
                    first.push(second);
                }
            }
        }
        wrong += short.len();
        println!(
            "{arm}: seconds {based}..={last}: {} whole ({:?}..{:?}), {} short with no error (first {:?}, last {:?})",
            whole.len(),
            whole.first(),
            whole.last(),
            short.len(),
            short.first(),
            short.last()
        );
        for (kind, seconds) in refused {
            println!("{arm}: {} refused ({:?}..{:?}): {kind}", seconds.len(), seconds.first(), seconds.last());
        }
    }
    assert_eq!(wrong, 0, "seconds restored short with no error");
}
'''
open(p,'w').write(s)
