# Lane B26: a test that restores on a small disk (B26_DISK, a tmpfs) at each of several amounts of it left free,
# and says what each restore the disk cut short left there that opens as a database, for either arm.
p='crates/amphora/tests/backup.rs'
s=open(p).read()
s+='''
#[test]
fn b26_full_disk() {
    let Some(disk) = std::env::var_os("B26_DISK") else { return };
    let disk = std::path::PathBuf::from(disk);
    let (s, last) = several_generations();
    let (mut refused, mut wrong) = (0, 0);
    for left_free in (8u64 * 1024..).step_by(12 * 1024) {
        let dir = disk.join("r");
        std::fs::create_dir_all(&dir).unwrap();
        let free = fs4::available_space(&disk).unwrap();
        assert!(free > left_free, "the disk is too small: {free}");
        std::fs::write(disk.join("filler"), vec![1u8; usize::try_from(free - left_free).unwrap()]).unwrap();
        let free = fs4::available_space(&disk).unwrap();
        let target = dir.join("restored.db");
        let restored = backup::restore(&s.blobs, "db", last, &target, until());
        let files: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        let done = match &restored {
            Ok(_) => { println!("B26 disk: {free} bytes free: restored, {} of {} rows", opens_as_a_database(&dir).join(", "), s.model.len()); true }
            Err(e) => {
                let opens = opens_as_a_database(&dir);
                refused += 1;
                wrong += usize::from(!opens.is_empty());
                println!("B26 disk: {free} bytes free: refused ({e:#}); left {files:?}; {}", if opens.is_empty() { "nothing left opens".to_owned() } else { opens.join(", ") });
                false
            }
        };
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_file(disk.join("filler")).unwrap();
        if done { break; }
    }
    println!("B26 disk: {refused} restores refused, {wrong} left a database of {} rows", s.model.len());
    assert!(refused > 3, "non-vacuous");
    assert_eq!(wrong, 0, "restores that left a database");
}
'''
open(p,'w').write(s)
s=open(p).read()
s+='''
#[test]
fn b26_left() {
    use std::io::{BufRead as _, Write as _};
    let (s, last) = several_generations();
    let mut survey = |what: &str, restore: &mut dyn FnMut(usize, &std::path::Path) -> bool| {
        let (mut steps, mut opened) = (0usize, Vec::new());
        loop {
            let dir = tempfile::tempdir_in(s.dir.path()).unwrap();
            if restore(steps, dir.path()) { break; }
            opened.extend(opens_as_a_database(dir.path()));
            steps += 1;
        }
        println!("B26 {what}: {steps} points, {} left a database of 50 rows: {:?}", opened.len(), opened);
        opened.len()
    };
    let errors = survey("an error at each step", &mut |at, dir| {
        let mut pace = Fails { at, steps: 0 };
        backup::restore_paced(&s.blobs, "db", last, &dir.join("restored.db"), &mut pace, until()).is_ok()
    });
    let reads = survey("a read of the store refused", &mut |at, dir| {
        let blobs = FailsRead { inner: &s.blobs, at: u64::try_from(at).unwrap(), reads: AtomicU64::new(0) };
        backup::restore(&blobs, "db", last, &dir.join("restored.db"), until()).is_ok()
    });
    let this = std::env::current_exe().unwrap();
    let kills = survey("a kill at each step", &mut |at, dir| {
        let mut child = restore_child_by(this.to_str().unwrap(), &[], &s, last, dir).env("AMPHORA_RESTORE_ASKS", "1").spawn().unwrap();
        let mut go = child.stdin.take().unwrap();
        let mut said = std::io::BufReader::new(child.stdout.take().unwrap()).lines();
        let mut steps = 0;
        let ended = loop {
            let Some(line) = said.next() else { break true };
            if line.unwrap() != "step" { continue; }
            if steps == at { child.kill().unwrap(); break false; }
            steps += 1;
            writeln!(go).unwrap();
        };
        child.wait().unwrap();
        ended
    });
    assert_eq!((errors, reads, kills), (0, 0, 0), "restores that left a database");
}
'''
open(p,'w').write(s)
