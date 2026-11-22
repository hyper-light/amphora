//! The peer store (CP-T1.12; docs/design/backup.md, "The peer store"): a database in one process
//! backed up to a folder another process keeps, over the owner transport.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros
)]

use std::collections::BTreeMap;
use std::io::{BufRead as _, Read as _, Write as _};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use amphora::backup::{self, Archive, SECOND_NS};
use amphora::database::Database;
use amphora::db::Db;
use amphora::transport::{PeerBlobs, PeerStore};
use amphora_api::blobs::{Blobs, conformance};

/// The transport tests' hang guard (tests/transport.rs, local-owner-transport.md, "The tests'
/// budgets"): no verdict rests on it.
const HANG: Duration = Duration::from_secs(120);

fn until() -> Instant {
    Instant::now() + HANG
}

/// The store's process: a folder store under `AMPHORA_PEER_STORE_DIR` served at its endpoint
/// until its parent closes its input. Run only as the child of the test below.
#[test]
fn peer_store_child() {
    let Ok(dir) = std::env::var("AMPHORA_PEER_STORE_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let folder: Arc<dyn Blobs> =
        Arc::new(backup::FolderBlobs::open(&dir.join("archive"), 64 * 1024).unwrap());
    let store = PeerStore::start(&dir.join("store"), folder).unwrap();
    println!("store-ready");
    std::io::stdout().flush().unwrap();
    // Serves until the parent closes its end: the read returns at its end of input.
    let mut rest = Vec::new();
    std::io::stdin().read_to_end(&mut rest).unwrap();
    drop(store);
}

fn rows(db: &Db) -> BTreeMap<i64, String> {
    let mut out = BTreeMap::new();
    db.query("SELECT id, v FROM t ORDER BY id", [], |r| {
        out.insert(
            r.get::<i64>(0).map_err(|e| anyhow::anyhow!("{e}"))?,
            r.get::<String>(1).map_err(|e| anyhow::anyhow!("{e}"))?,
        );
        Ok(true)
    })
    .unwrap();
    out
}

/// T-T1.12.N1 (AC-T1.12.4): a peer store in another process on the same host. Do: start a child
/// process serving a folder store at an endpoint; from this one, run the seam's conformance suite
/// through its client, then back a database up to it, write a row a second, close it, and restore
/// its last second through the client and straight from the child's folder; then close the
/// child's input. Expect: the suite passes; both restores hold every row written; the child ends
/// cleanly.
#[test]
fn t_t1_12_n1_a_peer_store_in_another_process() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "peer_store_child", "--nocapture"])
        .env("AMPHORA_PEER_STORE_DIR", dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        line.clear();
        assert_ne!(
            stdout.read_line(&mut line).unwrap(),
            0,
            "the store's process ended"
        );
        if line.trim() == "store-ready" {
            break;
        }
    }

    let peer = Arc::new(PeerBlobs::connect(&dir.path().join("store"), until()).unwrap());
    assert_eq!(peer.object_max(), 64 * 1024);
    conformance(peer.as_ref(), "suite", until()).unwrap();

    let source = tempfile::tempdir().unwrap();
    let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS + 1));
    let read = Arc::clone(&now);
    let archive = Archive {
        blobs: Arc::clone(&peer) as Arc<dyn Blobs>,
        prefix: "db".into(),
        clock: Arc::new(move || read.load(Ordering::SeqCst)),
        memory: peer.object_max(),
        patience: HANG,
        check: backup::CheckPolicy::Off,
    };
    let (database, shipping) =
        Database::open_backed_up(&source.path().join("source.db"), archive, until()).unwrap();
    database
        .call(until(), |c| {
            c.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")?;
            Ok(())
        })
        .unwrap();
    shipping.wait(until(), |s| s.bases >= 1).unwrap();
    let mut model = BTreeMap::new();
    for id in 1..40i64 {
        now.fetch_add(SECOND_NS, Ordering::SeqCst);
        let v = format!(
            "{id}:{}",
            "p".repeat(usize::try_from(id).unwrap() * 97 % 3000)
        );
        let written = v.clone();
        database
            .call(until(), move |c| {
                c.execute(
                    "INSERT INTO t (id, v) VALUES (?1, ?2)",
                    vec![
                        amphora_api::Value::Integer(id),
                        amphora_api::Value::Text(written),
                    ],
                )?;
                Ok(())
            })
            .unwrap();
        model.insert(id, v);
    }
    let last = now.load(Ordering::SeqCst).div_ceil(SECOND_NS);
    now.fetch_add(SECOND_NS, Ordering::SeqCst);
    drop(database);
    let status = shipping.wait(until(), |s| s.ended).unwrap();
    assert_eq!(status.failures, 0, "{:?}", status.last_failure);

    let through_peer = source.path().join("through-peer.db");
    backup::restore(peer.as_ref(), "db", last, &through_peer, until()).unwrap();
    assert_eq!(rows(&Db::open(&through_peer).unwrap()), model);
    let folder = backup::FolderBlobs::open(&dir.path().join("archive"), 64 * 1024).unwrap();
    let from_folder = source.path().join("from-folder.db");
    backup::restore(&folder, "db", last, &from_folder, until()).unwrap();
    assert_eq!(rows(&Db::open(&from_folder).unwrap()), model);

    drop(child.stdin.take());
    assert!(child.wait().unwrap().success());
}
