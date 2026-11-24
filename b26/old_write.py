# Lane B26: has the tree before lane B22's fix (ba250493) keep the archive, the database's files and the
# history of the scenario whose base is short (d45_a_base_begun_while_a_read_holds_the_closing_segment...).
p='crates/amphora/tests/backup.rs'
s=open(p).read()
old='''    let last = s.history.last().unwrap().0.div_ceil(SECOND_NS);
    let restored = s.restored(last).unwrap();
    let lost: Vec<i64> = s
        .model'''
assert s.count(old)==1, s.count(old)
new='''    if let Ok(to) = std::env::var("B26_OLD") {
        let to = std::path::Path::new(&to);
        std::fs::create_dir_all(to).unwrap();
        for what in ["archive", "source.db", "source.db-wal", "source.db-wal2"] {
            let copied = std::process::Command::new("cp").arg("-a").arg(s.dir.path().join(what)).arg(to).status().unwrap();
            assert!(copied.success());
        }
        let mut text = String::new();
        for (ns, rows) in &s.history {
            let ids: Vec<String> = rows.keys().map(|id| id.to_string()).collect();
            text.push_str(&format!("{ns} {}\\n", ids.join(",")));
        }
        std::fs::write(to.join("history.txt"), text).unwrap();
    }
'''+old
s=s.replace(old,new,1)
open(p,'w').write(s)
