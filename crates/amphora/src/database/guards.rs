//! A host's guards, kept beside the owner lease wherever it is held (CP-T1.5, U05;
//! docs/design/local-owner-transport.md, "Owner guards").
use std::any::Any;
use std::fs::File;
use std::sync::Arc;

/// What a host keeps alive as long as this process owns a database: `keep` is held, never read,
/// until the owner's engine, readers and flusher have drained. `key` is the host's assertion that
/// two groups protect the same files; Amphora compares it and does not verify it.
#[derive(Clone)]
pub struct Guards {
    key: [u8; 32],
    _keep: Arc<dyn Any + Send + Sync>,
}

impl Guards {
    #[must_use]
    pub fn new(key: [u8; 32], keep: Arc<dyn Any + Send + Sync>) -> Self {
        Self { key, _keep: keep }
    }

    pub(crate) fn key(&self) -> [u8; 32] {
        self.key
    }
}

/// The owner lease and the guards that came with it: cloned wherever the lease is held, so the
/// group lives as long as the last holder of the lease.
#[derive(Clone)]
pub(crate) struct Owned {
    lease: Arc<File>,
    // Declared after the lease, so dropped after it: the group outlives the lease it came with.
    guards: Option<Guards>,
}

impl Owned {
    pub(crate) fn new(lease: Arc<File>, guards: Option<Guards>) -> Self {
        Self { lease, guards }
    }

    pub(crate) fn lease(&self) -> &Arc<File> {
        &self.lease
    }

    /// Whether a guarded open made this owner: then nothing it opens makes a directory.
    pub(crate) fn guarded(&self) -> bool {
        self.guards.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use std::io::{BufRead, Read};
    use std::sync::mpsc::{Receiver, Sender, channel};

    fn until() -> Instant {
        Instant::now() + crate::transport::HANG
    }

    fn config() -> api::replication::ReplicaConfig {
        api::replication::ReplicaConfig {
            node_id: 1,
            voters: vec![1],
            learners: vec![],
            bootstrap: None,
        }
    }

    /// Says when it drops, and whether the database's lease was free then: a group dropped with
    /// the lease free outlived every holder of the lease.
    struct Witness {
        id: usize,
        lease: Option<PathBuf>,
        dropped: Sender<(usize, bool)>,
    }
    impl Drop for Witness {
        fn drop(&mut self) {
            let free = self
                .lease
                .as_ref()
                .is_some_and(|path| crate::transport::try_lease(path).unwrap().is_some());
            self.dropped.send((self.id, free)).unwrap();
        }
    }

    struct Witnesses {
        sender: Sender<(usize, bool)>,
        dropped: Receiver<(usize, bool)>,
    }
    impl Witnesses {
        fn new() -> Self {
            let (sender, dropped) = channel();
            Self { sender, dropped }
        }
        /// Guards of `key` whose witness `id` probes `lease` as it drops.
        fn guards(&self, key: u8, id: usize, lease: Option<&Path>) -> Guards {
            Guards::new(
                [key; 32],
                Arc::new(Witness {
                    id,
                    lease: lease.map(Path::to_path_buf),
                    dropped: self.sender.clone(),
                }),
            )
        }
        /// The witnesses dropped so far, in order.
        fn so_far(&self) -> Vec<usize> {
            self.dropped.try_iter().map(|(id, _)| id).collect()
        }
        /// Waits for the next witness to drop: its id and whether the lease was free.
        fn next(&self) -> (usize, bool) {
            self.dropped.recv_timeout(crate::transport::HANG).unwrap()
        }
    }

    fn owner_key(path: &Path) -> Option<Option<[u8; 32]>> {
        let path = path.canonicalize().unwrap();
        let owners = OWNERS.get_or_init(Mutex::default).lock().unwrap();
        owners.get(&path).map(|owner| owner.key)
    }

    fn reason(refusal: Result<impl Sized, Refusal>) -> String {
        match refusal {
            Err(Refusal::Invalid { why, .. }) => why,
            Err(other) => panic!("not an Invalid refusal: {other:?}"),
            Ok(_) => panic!("not refused"),
        }
    }

    #[test]
    fn repeated_guarded_opens_keep_one_group_that_outlives_every_handle() {
        // CP-T1.5 (U05), local-owner-transport.md "Owner guards": sixteen guarded opens of
        // the same key, and an unguarded open, reuse the first owner; each incoming group drops
        // at once and the owner's stays. It outlives every Database clone, and drops only with
        // the lease free: after the writer, readers and flusher have drained.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let witnesses = Witnesses::new();
        let first =
            Database::open_guarded(&path, witnesses.guards(1, 0, Some(&path)), until()).unwrap();
        first
            .execute(
                vec![Statement {
                    sql: "CREATE TABLE t(x)".into(),
                    params: vec![],
                }],
                until(),
            )
            .unwrap();
        let mut handles = vec![first.clone()];
        for id in 1..=16 {
            let again =
                Database::open_guarded(&path, witnesses.guards(1, id, None), until()).unwrap();
            assert!(Arc::ptr_eq(&again.shared, &first.shared));
            assert_eq!(witnesses.so_far(), vec![id], "open {id} kept its group");
            handles.push(again);
        }
        let unguarded = Database::open(&path, until()).unwrap();
        assert!(Arc::ptr_eq(&unguarded.shared, &first.shared));
        assert_eq!(owner_key(&path), Some(Some([1; 32])));
        handles.push(unguarded);
        drop(first);
        for handle in handles {
            assert!(
                witnesses.so_far().is_empty(),
                "the group dropped with a handle left"
            );
            let rows = handle
                .query(until(), |db| {
                    Ok(db.query_row("SELECT count(*) FROM t", [], |row| row.get::<usize, i64>(0))?)
                })
                .unwrap();
            assert_eq!(rows, 0);
            drop(handle);
        }
        assert_eq!(witnesses.next(), (0, true));
    }

    #[test]
    fn guarded_opens_refuse_a_live_owner_they_cant_reuse_before_any_effect() {
        // CP-T1.5 (U05), local-owner-transport.md "Owner guards": other guards, an unguarded
        // owner and another file at the path are each refused under the owner table's lock.
        // The incoming group drops at once, the live owner and its group stay as they were.
        let dir = tempfile::tempdir().unwrap();
        let witnesses = Witnesses::new();

        let guarded = dir.path().join("guarded");
        let owner =
            Database::open_guarded(&guarded, witnesses.guards(1, 0, None), until()).unwrap();
        let why = reason(Database::open_guarded(
            &guarded,
            witnesses.guards(2, 1, None),
            until(),
        ));
        assert!(why.contains("other guards"), "{why}");
        assert_eq!(witnesses.so_far(), vec![1]);
        assert_eq!(owner_key(&guarded), Some(Some([1; 32])));

        let unguarded = dir.path().join("unguarded");
        let plain = Database::open(&unguarded, until()).unwrap();
        let why = reason(Database::open_guarded(
            &unguarded,
            witnesses.guards(1, 2, None),
            until(),
        ));
        assert!(why.contains("unguarded"), "{why}");
        assert_eq!(witnesses.so_far(), vec![2]);
        assert_eq!(owner_key(&unguarded), Some(None));

        // Another file at the path: the identity the owner stored is not the file's there now.
        let other = dir.path().join("other");
        std::fs::write(&other, b"").unwrap();
        {
            let mut owners = OWNERS.get_or_init(Mutex::default).lock().unwrap();
            owners
                .get_mut(&guarded.canonicalize().unwrap())
                .unwrap()
                .identity = file_identity(&other).unwrap();
        }
        let why = reason(Database::open_guarded(
            &guarded,
            witnesses.guards(1, 3, None),
            until(),
        ));
        assert!(why.contains("not the live owner's file"), "{why}");
        let why = reason(Database::attach_replicated_guarded(
            &guarded,
            &dir.path().join("log"),
            config(),
            witnesses.guards(1, 4, None),
            until(),
        ));
        assert!(why.contains("not the live owner's file"), "{why}");
        assert_eq!(witnesses.so_far(), vec![3, 4]);
        assert!(!dir.path().join("log").exists());

        // The owners were untouched: each still serves.
        for database in [&owner, &plain] {
            database
                .execute(
                    vec![Statement {
                        sql: "CREATE TABLE t(x)".into(),
                        params: vec![],
                    }],
                    until(),
                )
                .unwrap();
        }
        drop(owner);
        assert_eq!(witnesses.next().0, 0);
        drop(plain);
    }

    #[test]
    fn a_guarded_open_makes_no_directory() {
        // CP-T1.5 (U05), local-owner-transport.md "Owner guards": a missing parent of the
        // database or of its log is refused before the owner table, the endpoint or the lease
        // is touched; nothing is made.
        let dir = tempfile::tempdir().unwrap();
        let witnesses = Witnesses::new();
        let missing = dir.path().join("missing");
        let why = reason(Database::open_guarded(
            &missing.join("db"),
            witnesses.guards(1, 0, None),
            until(),
        ));
        assert!(why.contains("parent directory does not exist"), "{why}");
        let path = dir.path().join("db");
        let log = missing.join("log");
        let why = reason(Database::open_replicated_guarded(
            &path,
            &log,
            config(),
            witnesses.guards(1, 1, None),
            until(),
        ));
        assert!(why.contains("parent directory does not exist"), "{why}");
        let why = reason(Database::attach_replicated_guarded(
            &path,
            &log,
            config(),
            witnesses.guards(1, 2, None),
            until(),
        ));
        assert!(why.contains("parent directory does not exist"), "{why}");
        assert_eq!(witnesses.so_far(), vec![0, 1, 2]);
        let made: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert!(made.is_empty(), "the refused opens made {made:?}");
    }

    #[test]
    fn guarded_replicated_owner_is_reused_by_the_same_guards_and_attached_by_them() {
        // CP-T1.5 (U05), local-owner-transport.md "Owner guards": a guarded attach finds no
        // owner, the guarded replicated open owns, a second open and an attach of the same key
        // reuse it and drop their groups, other guards are refused, and the group drops with
        // the lease free once the actor and its machine have drained.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let log = dir.path().join("log");
        let witnesses = Witnesses::new();
        let attached = Database::attach_replicated_guarded(
            &path,
            &log,
            config(),
            witnesses.guards(1, 0, None),
            until(),
        )
        .unwrap();
        assert!(attached.is_none());
        let owner = Database::open_replicated_guarded(
            &path,
            &log,
            config(),
            witnesses.guards(1, 1, Some(&path)),
            until(),
        )
        .unwrap();
        let again = Database::open_replicated_guarded(
            &path,
            &log,
            config(),
            witnesses.guards(1, 2, None),
            until(),
        )
        .unwrap();
        assert!(Arc::ptr_eq(&again.shared, &owner.shared));
        let attached = Database::attach_replicated_guarded(
            &path,
            &log,
            config(),
            witnesses.guards(1, 3, None),
            until(),
        )
        .unwrap()
        .unwrap();
        assert!(Arc::ptr_eq(&attached.shared, &owner.shared));
        let why = reason(Database::attach_replicated_guarded(
            &path,
            &log,
            config(),
            witnesses.guards(2, 4, None),
            until(),
        ));
        assert!(why.contains("other guards"), "{why}");
        assert_eq!(witnesses.so_far(), vec![0, 2, 3, 4]);
        drop((owner, again, attached));
        assert_eq!(witnesses.next(), (1, true));
    }

    #[test]
    fn a_guarded_owner_never_remakes_a_parent_removed_after_its_open_resolved_it() {
        // CP-T1.5 (U05), local-owner-transport.md "Owner guards": the parent is resolved, then
        // removed. The steps that follow resolution (the engine's open, the replicated
        // machine's, and the log's lease the replicated owner takes first) each refuse a
        // guarded owner and make nothing; an unguarded owner's engine makes it again, as before.
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("gone");
        std::fs::create_dir(&gone).unwrap();
        let path = existing_path(&gone.join("db")).unwrap();
        let log = existing_path(&gone.join("log")).unwrap();
        std::fs::remove_dir(&gone).unwrap();
        let owned = |guards: Option<Guards>| {
            Owned::new(
                Arc::new(File::create(dir.path().join("lease")).unwrap()),
                guards,
            )
        };
        let guarded = || owned(Some(Guards::new([1; 32], Arc::new(()))));
        let missing = "parent directory does not exist";

        let why = reason(open_db(&path, &guarded()));
        assert!(why.contains(missing), "{why}");
        let why = reason(crate::replication::Machine::open_with_owner(
            &path,
            hyper_raft::proto::ConfState {
                voters: vec![1],
                ..hyper_raft::proto::ConfState::default()
            },
            guarded(),
            1,
            None,
            false,
        ));
        assert!(why.contains(missing), "{why}");
        let why = reason(replicated::start(
            &dir.path().join("db"),
            &log,
            config(),
            api::replication::Durability::Persisted,
            Arc::new(Changed::default()),
            guarded(),
            until(),
        ));
        assert!(why.contains(missing), "{why}");
        assert!(!gone.exists(), "a guarded owner made its parent again");

        drop(open_db(&path, &owned(None)).unwrap());
        assert!(
            gone.is_dir(),
            "the unguarded open no longer makes its parent"
        );
    }

    /// Holds an unguarded owner of the database at `AMPHORA_GUARDS_HOLD` for a test in another
    /// process, until its standard input closes.
    #[test]
    fn owner_in_another_process() {
        let Some(path) = std::env::var_os("AMPHORA_GUARDS_HOLD") else {
            return;
        };
        let _owner = Database::open(Path::new(&path), until()).unwrap();
        println!("owner-ready");
        std::io::stdin().read_to_end(&mut Vec::new()).unwrap();
    }

    #[test]
    fn guarded_opens_refuse_another_process_owner_before_any_effect() {
        // CP-T1.5 (U05), local-owner-transport.md "Owner guards": while another process owns
        // the database, a guarded open and a guarded attach are refused, neither attaching as
        // its client nor leaving an owner entry; the incoming groups drop at once.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "database::guards::tests::owner_in_another_process",
                "--nocapture",
            ])
            .env("AMPHORA_GUARDS_HOLD", &path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        while line.trim() != "owner-ready" {
            line.clear();
            assert_ne!(
                stdout.read_line(&mut line).unwrap(),
                0,
                "the owner process ended"
            );
        }
        let witnesses = Witnesses::new();
        let why = reason(Database::open_guarded(
            &path,
            witnesses.guards(1, 0, None),
            until(),
        ));
        assert!(why.contains("another process owns"), "{why}");
        let why = reason(Database::attach_replicated_guarded(
            &path,
            &dir.path().join("log"),
            config(),
            witnesses.guards(1, 1, None),
            until(),
        ));
        assert!(why.contains("another process owns"), "{why}");
        assert_eq!(witnesses.so_far(), vec![0, 1]);
        assert_eq!(owner_key(&path), None);
        drop(child.stdin.take());
        assert!(child.wait().unwrap().success());
        // With the other process gone, the guarded open owns.
        let owner = Database::open_guarded(&path, witnesses.guards(1, 2, None), until()).unwrap();
        assert_eq!(owner_key(&path), Some(Some([1; 32])));
        drop(owner);
        assert_eq!(witnesses.next().0, 2);
    }
}
