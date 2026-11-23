//! Amphora (lane skip-8, T-SKU.10; executor.md decision 11): a measured summary's dropped method
//! built again off the scan's path.
//!
//! The scan that finds a dropped method's trial due claims its build ([`SkipRegistry::claim_build`])
//! and goes on with the leader; the build runs on a thread of its own. Zone maps are built from the
//! cracker's copy in memory; the cracker is read from the table on the build's own connection, in
//! a read transaction whose snapshot is pinned to the state the summary holds
//! ([`SkipRegistry::pin_build`]). A write the hook takes during the read voids the build
//! ([`BuildEnd::Invalidated`]), and the next scan that finds the trial due claims a new one.
//!
//! Bound: at most one build, so one thread and one connection, per summary at a time (the
//! claim), and a summary's builds come only at its trials, which the doubling schedule spaces
//! geometrically. Nothing here waits on a clock: the build starts on the scan that finds it due
//! and ends on its own completion.
//!
//! Lane skip-9 (T-SKU.11): the same build runs inline too ([`build_inline`]: skip-7's, on the
//! scan's own connection and read, the control of [`super::BuildMode`]), and a replaying build's
//! writes are applied by the registry as it installs.
//!
//! Lane skip-9 (T-SKU.12): the build's own connection opens the database with what the scan's
//! connection opened it with ([`BuildKeys`]): its encryption key, or its external page codec. The
//! key is held as the scan's connection holds it (an [`EncryptionKey`], zeroed when dropped, its
//! `Debug` redacted), moved into the build's connection, and never logged.
//!
//! [`SkipRegistry::claim_build`]: super::SkipRegistry::claim_build
//! [`SkipRegistry::pin_build`]: super::SkipRegistry::pin_build

use super::leaves::LeafCursor;
use super::{nanos_since, BuildClaim, BuildEnd, SkipMethod};
use crate::sync::Arc;
use crate::thread;
use crate::{ConnectOpts, Connection, Database, EncryptionKey, PageCodec};

/// Lane skip-9 (T-SKU.12): what the build's own connection opens the database with, taken from
/// the connection whose scan started the build. No `Debug`: it is never logged.
struct BuildKeys {
    encryption_key: Option<EncryptionKey>,
    page_codec: Option<Arc<dyn PageCodec>>,
}

impl BuildKeys {
    /// The key and page codec `conn` holds now.
    fn of(conn: &Connection) -> Self {
        Self {
            encryption_key: conn.encryption_key.read().clone(),
            page_codec: conn.get_pager().page_codec_external(),
        }
    }

    /// The settings the build's connection opens with: the key moves into them.
    fn connect_opts(self) -> ConnectOpts {
        let opts = ConnectOpts::new().with_encryption_key(self.encryption_key);
        match self.page_codec {
            Some(codec) => opts.with_page_codec(codec),
            None => opts,
        }
    }
}

/// Where a build reads the table: on a connection of its own (off the scan's path), or on the
/// scan's connection inside the read the scan holds (inline).
enum Reader<'a> {
    Own(BuildKeys),
    Scan(&'a Arc<Connection>),
}

/// A claim the build holds: handed back with nothing built if the build ends without handing it
/// back itself (an error, or a thread that could not start), so the summary's next trial can
/// claim a new build.
struct Held {
    db: Arc<Database>,
    claim: Option<BuildClaim>,
}

impl Drop for Held {
    fn drop(&mut self) {
        if let Some(claim) = self.claim.take() {
            self.db.skipping.abandon_build(claim);
        }
    }
}

/// Ends the build's read transaction however the build ends.
struct ReadTx(Arc<crate::storage::pager::Pager>);

impl Drop for ReadTx {
    fn drop(&mut self) {
        self.0.end_read_tx();
    }
}

/// Starts the build of the table at `root`'s dropped method (the claimed column of a table `columns`
/// wide) when its trial is due and no other build of it is under way. `None`: nothing started
/// (none due, one under way, or no thread could start; the claim is handed back). The handle
/// tells a test how the build ended; a scan lets it go. `conn` is the scan's connection: the
/// build's own opens the database with its key or page codec (lane skip-9, T-SKU.12), taken only
/// once a build is claimed.
pub(crate) fn start_build(
    conn: &Arc<Connection>,
    root: i64,
    columns: usize,
) -> Option<thread::JoinHandle<BuildEnd>> {
    let claim = conn.db.skipping.claim_build(root)?;
    let keys = BuildKeys::of(conn);
    let mut held = Held {
        db: Arc::clone(&conn.db),
        claim: Some(claim),
    };
    let spawned = thread::Builder::new()
        .name("amphora-skip-build".to_owned())
        .spawn(move || build(&mut held, Reader::Own(keys), columns));
    match spawned {
        Ok(handle) => Some(handle),
        Err(error) => {
            tracing::debug!(target: "amphora.skipping", root, %error, "skipping build could not start a thread; the leader keeps answering");
            None
        }
    }
}

/// Lane skip-9 (T-SKU.11): [`super::BuildMode::Inline`], skip-7's build: the scan on `conn`,
/// inside its read, that finds the trial due builds it now, on its own path. `None`: none due, or
/// another builder holds the claim.
pub(crate) fn build_inline(conn: &Arc<Connection>, root: i64, columns: usize) -> Option<BuildEnd> {
    let claim = conn.db.skipping.claim_build(root)?;
    let mut held = Held {
        db: Arc::clone(&conn.db),
        claim: Some(claim),
    };
    Some(build(&mut held, Reader::Scan(conn), columns))
}

fn build(held: &mut Held, reader: Reader<'_>, columns: usize) -> BuildEnd {
    let Some(method) = held.claim.as_ref().map(|claim| claim.method) else {
        return BuildEnd::Superseded;
    };
    let origin = std::time::Instant::now();
    let end = match method {
        SkipMethod::ZoneMaps => match held.claim.take() {
            Some(claim) => held
                .db
                .skipping
                .rebuild_zones(claim, &mut || nanos_since(origin)),
            None => BuildEnd::Superseded,
        },
        SkipMethod::Cracking => build_cracker(held, reader, columns, origin),
    };
    tracing::debug!(target: "amphora.skipping", method = ?method, end = ?end, "skipping build ended");
    end
}

/// Reads the cracker from the table at a snapshot pinned to the summary's state (on a connection
/// of its own, or the scan's), and installs it as the summary's build mode says.
fn build_cracker(
    held: &mut Held,
    reader: Reader<'_>,
    columns: usize,
    origin: std::time::Instant,
) -> BuildEnd {
    let inline = matches!(reader, Reader::Scan(_));
    let (conn, _read) = match reader {
        Reader::Scan(conn) => (Arc::clone(conn), None),
        Reader::Own(keys) => {
            let conn = match held.db.connect(keys.connect_opts()) {
                Ok(conn) => conn,
                Err(error) => {
                    tracing::debug!(target: "amphora.skipping", %error, "skipping build could not connect");
                    return BuildEnd::Failed;
                }
            };
            let pager = conn.get_pager();
            if let Err(error) = pager.begin_read_tx() {
                tracing::debug!(target: "amphora.skipping", %error, "skipping build could not begin a read");
                return BuildEnd::Failed;
            }
            (conn, Some(ReadTx(pager)))
        }
    };
    let Some(claim) = held.claim.as_ref() else {
        return BuildEnd::Superseded;
    };
    let root = claim.root_page;
    // Opening the versioned cursor begins its transaction. Pin that snapshot before
    // recording writes to replay, so the same write cannot also be in the base copy.
    let mut cursor = match LeafCursor::open(&conn, root, columns) {
        Ok(cursor) => cursor,
        Err(error) => {
            tracing::debug!(target: "amphora.skipping", root, %error, "cracker could not open its table");
            return BuildEnd::Failed;
        }
    };
    if let Err(end) = held
        .db
        .skipping
        .pin_build_at(claim, conn.skipping_id(), || conn.skipping_positions())
    {
        return end;
    }
    let cracker = match cursor.cracker(claim.column) {
        Ok(cracker) => cracker,
        Err(error) => {
            tracing::debug!(target: "amphora.skipping", root, %error, "cracker could not be built again");
            return BuildEnd::Failed;
        }
    };
    if inline {
        held.db
            .skipping
            .count_inline_read(root, cracker.row_count());
    }
    let cost = nanos_since(origin);
    match held.claim.take() {
        Some(claim) => held
            .db
            .skipping
            .install_cracker(claim, cracker, cost, &mut || nanos_since(origin)),
        None => BuildEnd::Superseded,
    }
}
