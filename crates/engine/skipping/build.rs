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
//! [`SkipRegistry::claim_build`]: super::SkipRegistry::claim_build
//! [`SkipRegistry::pin_build`]: super::SkipRegistry::pin_build

use super::{cracker_of_table, nanos_since, BuildClaim, BuildEnd, SkipMethod};
use crate::sync::Arc;
use crate::thread;
use crate::{ConnectOpts, Database};

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

/// Starts the build of the table at `root`'s dropped method (column `column` of a table `columns`
/// wide) when its trial is due and no other build of it is under way. `None`: nothing started
/// (none due, one under way, or no thread could start; the claim is handed back). The handle
/// tells a test how the build ended; a scan lets it go.
pub(crate) fn start_build(
    db: &Arc<Database>,
    root: i64,
    column: usize,
    columns: usize,
) -> Option<thread::JoinHandle<BuildEnd>> {
    let claim = db.skipping.claim_build(root)?;
    let mut held = Held {
        db: Arc::clone(db),
        claim: Some(claim),
    };
    let spawned = thread::Builder::new()
        .name("amphora-skip-build".to_owned())
        .spawn(move || build(&mut held, column, columns));
    match spawned {
        Ok(handle) => Some(handle),
        Err(error) => {
            tracing::debug!(target: "amphora.skipping", root, %error, "skipping build could not start a thread; the leader keeps answering");
            None
        }
    }
}

fn build(held: &mut Held, column: usize, columns: usize) -> BuildEnd {
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
        SkipMethod::Cracking => build_cracker(held, column, columns, origin),
    };
    tracing::debug!(target: "amphora.skipping", method = ?method, end = ?end, "skipping build ended");
    end
}

/// Reads the cracker from the table on a connection of its own, at a snapshot pinned to the
/// summary's state, and installs it unless a write came in since.
fn build_cracker(
    held: &mut Held,
    column: usize,
    columns: usize,
    origin: std::time::Instant,
) -> BuildEnd {
    let conn = match held.db.connect(ConnectOpts::new()) {
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
    let _read = ReadTx(Arc::clone(&pager));
    let Some((builder_pos, committed)) = conn.skipping_positions() else {
        return BuildEnd::Failed;
    };
    let Some(claim) = held.claim.as_ref() else {
        return BuildEnd::Superseded;
    };
    let root = claim.root_page;
    if !held
        .db
        .skipping
        .pin_build(claim, conn.skipping_id(), builder_pos, committed)
    {
        return BuildEnd::OlderSnapshot;
    }
    let cracker = match cracker_of_table(Arc::clone(&pager), root, column, columns) {
        Ok(cracker) => cracker,
        Err(error) => {
            tracing::debug!(target: "amphora.skipping", root, %error, "cracker could not be built again");
            return BuildEnd::Failed;
        }
    };
    let cost = nanos_since(origin);
    match held.claim.take() {
        Some(claim) => held
            .db
            .skipping
            .install_cracker(claim, cracker, cost, &mut || nanos_since(origin)),
        None => BuildEnd::Superseded,
    }
}
