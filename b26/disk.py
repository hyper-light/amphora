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
    for left_free in (8u64 * 1024..).step_by(40 * 1024) {
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
