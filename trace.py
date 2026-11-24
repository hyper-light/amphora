import sys
tree = sys.argv[1].rstrip("/") + "/"

def edit(path, pairs):
    s = open(tree + path).read()
    for old, new in pairs:
        assert s.count(old) == 1, (path, s.count(old), old[:80])
        s = s.replace(old, new)
    open(tree + path, "w").write(s)

edit("crates/engine/storage/wal.rs", [
    ("""    fn restart_wal_header(&mut self, io: &dyn IO) {
""", """    fn restart_wal_header(&mut self, io: &dyn IO) {
        eprintln!("T28 restart_wal_header {:p} max_frame {} nbackfills {} file {:?} bt {}", self as *const _, self.metadata.max_frame.load(Ordering::Acquire), self.metadata.nbackfills.load(Ordering::Acquire), self.runtime.file.as_ref().map(|f| f.size().unwrap_or(0)), std::backtrace::Backtrace::force_capture().to_string().lines().filter(|l| l.contains("amphora_engine") || l.contains("amphora::")).take(9).map(|l| l.trim().to_string()).collect::<Vec<_>>().join(" <- "));
"""),
])
edit("crates/amphora/src/database/replicated.rs", [
    ("""            let image = u64::try_from(image).unwrap_or(u64::MAX).min(disk);
            (
                log_bound::log_room(available, image, logs, kept),
""", """            let image = u64::try_from(image).unwrap_or(u64::MAX).min(disk);
            eprintln!("T28 room {:?} kept {kept} in_use {:?} available {available} logs {logs} image {image} disk {disk} free {} -> room {}", copier.path().file_name(), copier.log_frames_in_use(), available.saturating_sub(image), log_bound::log_room(available, image, logs, kept));
            (
                log_bound::log_room(available, image, logs, kept),
"""),
    ("""                let mut restarted = false;
                if due && pass.ready && !*out && !lent {
""", """                let mut restarted = false;
                let t28_before = (frames, pass.room, past, due, needed, pass.ready, *out, machine.closing_reads(), freed, self.pass_grew, passed);
                if due && pass.ready && !*out && !lent {
"""),
    ("""                let past = frames > pass.room;
                self.log_full = past.then(|| {
""", """                let past = frames > pass.room;
                {
                    thread_local! { static LAST: std::cell::RefCell<std::collections::HashMap<u64, String>> = std::cell::RefCell::new(std::collections::HashMap::new()); }
                    let line = format!("T28 owner {} flush (frames, room, past, due, needed, ready, out, closing_reads, freed, grew, passed) {t28_before:?} restarted {restarted} then frames {frames} past {past} closing_reads {}", self.replica.id(), self.replica.machine().closing_reads());
                    LAST.with(|last| { let mut last = last.borrow_mut(); if last.get(&self.replica.id()) != Some(&line) { eprintln!("{line}"); last.insert(self.replica.id(), line); } });
                }
                self.log_full = past.then(|| {
"""),
    ("""        let restarts = handle.counters().database_log_restarts;
        let (views, refused, last) = voter
            .read_local(Instant::now() + HANG_READ, |db| {
                db.query_transaction(|db| {
                    let first = values(db)?;
""", """        let restarts = handle.counters().database_log_restarts;
        let t28_sizes = || ["state-1.db-wal", "state-1.db-wal2"].map(|name| std::fs::metadata(dir.path().join(name)).map_or(0, |file| file.len()));
        eprintln!("T28 TEST1 start frames {} room {} restarts {restarts} files {:?}", driven.owner(0).replica.machine().log_frames().unwrap(), room(&clients, 0), t28_sizes());
        let (views, refused, last) = voter
            .read_local(Instant::now() + HANG_READ, |db| {
                db.query_transaction(|db| {
                    let first = values(db)?;
"""),
    ("""                        driven.flushed(&mut pump, 0);
                        if handle.counters().database_log_restarts > restarts {
                            more += 1;
                        }
""", """                        driven.flushed(&mut pump, 0);
                        eprintln!("T28 TEST1 write {value} frames {} room {} restarts {} files {:?} closing_reads {}", driven.owner(0).replica.machine().log_frames().unwrap(), room(&clients, 0), handle.counters().database_log_restarts, t28_sizes(), driven.owner(0).replica.machine().closing_reads());
                        if handle.counters().database_log_restarts > restarts {
                            more += 1;
                        }
"""),
    ("""        let began = files.each_ref().map(bytes);
        let (mut refused, mut last, mut slow) = (None, 1u8, Vec::new());
""", """        let began = files.each_ref().map(bytes);
        eprintln!("T28 TEST2 start frames {} room {} restarts {restarts} files {began:?}", driven.owner(0).replica.machine().log_frames().unwrap(), room(&clients, 0));
        let (mut refused, mut last, mut slow) = (None, 1u8, Vec::new());
"""),
    ("""                    sizes.push(files.each_ref().map(bytes));
                    let fast = voter.read_local(Instant::now() + HANG_READ, values);
""", """                    sizes.push(files.each_ref().map(bytes));
                    eprintln!("T28 TEST2 write {value} frames {} room {} restarts {} files {:?} closing_reads {}", driven.owner(0).replica.machine().log_frames().unwrap(), room(&clients, 0), handle.counters().database_log_restarts, files.each_ref().map(bytes), driven.owner(0).replica.machine().closing_reads());
                    let fast = voter.read_local(Instant::now() + HANG_READ, values);
"""),
])
