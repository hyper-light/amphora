# Lane B25: has B24's red tree (46e2480f) keep the archive its suspect 2 scenario wrote, with its history.
p='crates/amphora/tests/backup.rs'
s=open(p).read()
old='''    let last = history.last().unwrap().0.div_ceil(SECOND_NS);
    let mut refused = Vec::new();
    for second in based..=last {
        let target = dir.path().join(format!("at-{second}.db"));'''
assert s.count(old)==1, s.count(old)
new='''    if let Ok(to) = std::env::var("B25_OLD_ARCHIVE") {
        let to = std::path::Path::new(&to).join(format!("r{restarts}"));
        std::fs::create_dir_all(&to).unwrap();
        let copied = std::process::Command::new("cp")
            .arg("-a")
            .arg(dir.path().join("archive"))
            .arg(&to)
            .status()
            .unwrap();
        assert!(copied.success());
        let mut text = format!("{based}\\n");
        for (ns, rows) in &history {
            let ids: Vec<String> = rows.keys().map(|id| id.to_string()).collect();
            text.push_str(&format!("{ns} {}\\n", ids.join(",")));
        }
        std::fs::write(to.join("history.txt"), text).unwrap();
    }

'''+old
s=s.replace(old,new,1)
open(p,'w').write(s)
