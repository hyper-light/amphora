//! Amphora's resolved-page state machine for hyper-durable (U15, CP-T1.8).
//!
//! The designated local owner evaluates SQL once after a quorum read fence, including for
//! follower-origin requests, then rolls back its speculative transaction after capture. Only
//! resolved page deltas enter Raft: defaults, random functions and adaptive reads never run again
//! during application. Applied index, term and membership share the deltas' atomic WAL commit and
//! actual disk flush. The product Database actor drives ReadIndex, elections and durability through
//! hyper-durable, which owns quorum decisions, commit fencing, recovery and snapshots; its
//! GroupStore uses hyper-log. The host only carries authenticated replication messages.

use std::path::Path;

use amphora_api::{self as api, Refusal};
use amphora_engine::Value;
use hyper_durable::{EntryRef, Fatal, Point, StateMachine};
use hyper_raft::proto::{ConfChangeV2, ConfState};
use hyper_raft::wire::Record;
use serde::{Deserialize, Serialize};

use crate::db::Db;

pub use crate::database::replicated::{ReceiveReceipt, ReplicaHandle};
pub mod bootstrap;
pub(crate) mod floor;

const BASELINE_DELTA: &[u8] = b"amphora.delta.v1\0";
const BASELINE_SNAPSHOT: &[u8] = b"amphora.snapshot.v1\0";
#[derive(Serialize, Deserialize)]
struct BaselineSnapshot {
    declaration: api::replication::Bootstrap,
    pages: Vec<(u32, Vec<u8>)>,
}

const META: &str = "CREATE TABLE IF NOT EXISTS __amphora_replica (id INTEGER PRIMARY KEY CHECK(id = 1), applied TEXT NOT NULL, term TEXT NOT NULL, configuration BLOB NOT NULL)";
/// One resolved transaction's page budget. Oversize transactions are refused before proposal.
pub const COMMAND_BYTES: usize = api::REQUEST_MAX;
/// Images are bounded until streaming snapshots are implemented; compaction refuses larger ones.
pub const SNAPSHOT_BYTES: usize = 256 << 20;
const ENVELOPE_BYTES: usize = 1 << 20;

#[derive(Debug, Serialize, Deserialize)]
pub struct Prepared {
    base_index: u64,
    base_term: u64,
    pages: Vec<(u32, Vec<u8>)>,
    #[serde(skip)]
    baseline: Option<api::replication::Bootstrap>,
}

impl Prepared {
    pub(crate) fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    pub fn encode(&self) -> Result<Vec<u8>, Refusal> {
        let bytes = if let Some(declaration) = &self.baseline {
            let mut bytes = BASELINE_DELTA.to_vec();
            bytes.extend(postcard::to_allocvec(&(declaration, self)).map_err(unavailable)?);
            bytes
        } else {
            postcard::to_allocvec(self).map_err(unavailable)?
        };
        api::bound(
            "replicated command",
            bytes.len(),
            COMMAND_BYTES.saturating_add(ENVELOPE_BYTES),
        )?;
        Ok(bytes)
    }

    fn decode(bytes: &[u8]) -> Result<Self, Refusal> {
        api::bound(
            "replicated command",
            bytes.len(),
            COMMAND_BYTES.saturating_add(ENVELOPE_BYTES),
        )?;
        let prepared: Self = if let Some(payload) = bytes.strip_prefix(BASELINE_DELTA) {
            let (declaration, mut prepared): (api::replication::Bootstrap, Self) =
                postcard::from_bytes(payload).map_err(unavailable)?;
            prepared.baseline = Some(declaration);
            prepared
        } else {
            postcard::from_bytes(bytes).map_err(unavailable)?
        };
        validate_pages(&prepared.pages, COMMAND_BYTES)?;
        Ok(prepared)
    }
}

/// A prepared transaction's base, the fields its encoding starts with: read without its pages.
#[derive(Deserialize)]
struct Base {
    base_index: u64,
    base_term: u64,
}

/// The base fence: a prepared transaction applies only on the point it was prepared at. Every
/// entry applied moves the point, so the entry before it in the log decides.
fn on_base(base_index: u64, base_term: u64, point: Point) -> Result<(), Refusal> {
    if base_index == point.index && base_term == point.term {
        Ok(())
    } else {
        Err(api::invalid(
            "prepared transaction",
            "the database advanced since preparation",
        ))
    }
}

/// What applying the prepared transaction `data` will answer, the entry before it in the log being
/// `before`: decided from the log, so that a follower in the replicated class answers on its
/// group's commit (replication.md, "A write made at a follower is answered on the leader's
/// commit"). The outer error is a malformed entry, whose apply fences.
pub(crate) fn verdict(data: &[u8], before: Point) -> Result<Result<(), Refusal>, Refusal> {
    api::bound(
        "replicated command",
        data.len(),
        COMMAND_BYTES.saturating_add(ENVELOPE_BYTES),
    )?;
    let base: Base = if let Some(payload) = data.strip_prefix(BASELINE_DELTA) {
        let (_, base): (api::replication::Bootstrap, Base) =
            postcard::from_bytes(payload).map_err(unavailable)?;
        base
    } else {
        postcard::from_bytes(data).map_err(unavailable)?
    };
    Ok(on_base(base.base_index, base.base_term, before))
}

/// A local apply result is released only after both Raft commitment and the real state flush.
#[derive(Debug)]
pub struct Applied {
    pub index: u64,
    pub context: Vec<u8>,
    pub result: Result<(), Refusal>,
}

/// Owned exclusively by one hyper-durable Replica. It has no application-side writer alongside it.
pub struct Machine {
    db: Db,
    /// The last entry applied.
    point: Point,
    /// The last entry a finished flush of the database's log covers: what a restart opens with
    /// applied ([`StateMachine::durable`]). Behind `point` only in the replicated class.
    flushed: Point,
    /// The replicated class (docs/design/replication.md, "Fast acknowledgement"): an ordinary entry
    /// is applied without a flush of its own, and the owner flushes the log behind it.
    replicated: bool,
    configuration: ConfState,
    baseline: bootstrap::Baseline,
    // The largest image this machine makes or installs: SNAPSHOT_BYTES, lowered only by a test.
    snapshot_bytes: usize,
    // The index of the last configuration change applied, or more: the change's own index when
    // this machine applied it, the applied point when it opened or installed an image. The floor's
    // proof counts an attestation before marker J only if it was made at or after it, so an
    // overestimate only delays J (replication.md, "The upgrade that introduces the join rows").
    configuration_at: u64,
    // A test's stand-in for a member running a binary from before the floor: it writes no join
    // rows, as such a binary doesn't.
    #[cfg(test)]
    floor_aware: bool,
    // A test's record of this member's image digest at each applied index.
    #[cfg(test)]
    digests: Option<Vec<(u64, u64)>>,
    _owner: crate::database::Owned,
}

/// A floor this group's members didn't prove never enters replicated mode: one a standalone
/// owner installed, held where joins aren't tracked, is refused at open, at the seed and at an
/// image's install (replication.md, "Raising the floor", d). A floor raised on proof always has
/// joins tracked.
fn unproven_floor(db: &Db) -> anyhow::Result<()> {
    if crate::database::installed_floor(db)?.is_some() && floor::tracked(db)?.is_none() {
        return Err(Refusal::Fenced(
            "a floor not proven by this group's members; raise it again once joins are tracked"
                .into(),
        )
        .into());
    }
    Ok(())
}

fn unavailable(e: impl std::fmt::Display) -> Refusal {
    Refusal::Unavailable(e.to_string())
}
fn failed(e: impl std::fmt::Display) -> Fatal {
    tracing::error!(target: "amphora.replication", event = "machine_failed", error = %e, "replica must reopen from durable state");
    Fatal("Amphora state machine failed; see amphora.replication event")
}
fn text(n: u64) -> Value {
    Value::from_text(n.to_string())
}
fn validate_pages(pages: &[(u32, Vec<u8>)], bound: usize) -> Result<(), Refusal> {
    let mut bytes = 0usize;
    let mut previous = 0;
    for (id, page) in pages {
        if *id <= previous || page.len() != 4096 {
            return Err(api::invalid(
                "replicated pages",
                "page IDs must be sorted, unique and positive; pages must be 4096 bytes",
            ));
        }
        previous = *id;
        bytes = bytes
            .checked_add(page.len())
            .ok_or_else(|| api::invalid("replicated pages", "size overflow"))?;
    }
    api::bound("replicated page bytes", bytes, bound)
}
fn marker(db: &mut Db, point: Point, configuration: &ConfState) -> anyhow::Result<()> {
    let mut bytes = Vec::new();
    configuration.encode(&mut bytes);
    // Installing an image can already carry these exact values. Force a real page write:
    // imported raw frames have no commit marker, and an optimized no-op UPDATE must not leave
    // them as an unpublished WAL suffix while the machine reports the new durable point.
    db.query("DELETE FROM __amphora_replica WHERE id = 1", [], |_| {
        Ok(true)
    })?;
    db.query("INSERT INTO __amphora_replica(id, applied, term, configuration) VALUES (1, ?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET applied = excluded.applied, term = excluded.term, configuration = excluded.configuration", [text(point.index), text(point.term), Value::from_slice(&bytes)?], |_| Ok(true))
}

impl Machine {
    pub fn open(path: &Path, initial: ConfState) -> Result<Self, Refusal> {
        let owner = crate::database::Owned::new(
            std::sync::Arc::new(crate::database::exclusive_file(path)?),
            None,
        );
        Self::open_with_owner(path, initial, owner, 0, None, false)
    }

    pub(crate) fn open_with_owner(
        path: &Path,
        initial: ConfState,
        owner: crate::database::Owned,
        node_id: u64,
        bootstrap: Option<api::replication::Bootstrap>,
        replicated: bool,
    ) -> Result<Self, Refusal> {
        Self::open_db(
            |owner| crate::database::open_db(path, owner),
            initial,
            owner,
            node_id,
            bootstrap,
            replicated,
        )
    }

    /// [`Machine::open_with_owner`] on `io`: a test's disk, whose power the test cuts.
    #[cfg(test)]
    pub(crate) fn open_on(
        path: &Path,
        io: std::sync::Arc<dyn amphora_engine::IO>,
        initial: ConfState,
        owner: crate::database::Owned,
        node_id: u64,
        replicated: bool,
    ) -> Result<Self, Refusal> {
        Self::open_db(
            |_| Db::open_with(path, io).map_err(unavailable),
            initial,
            owner,
            node_id,
            None,
            replicated,
        )
    }

    fn open_db(
        open: impl FnOnce(&crate::database::Owned) -> Result<Db, Refusal>,
        initial: ConfState,
        owner: crate::database::Owned,
        node_id: u64,
        bootstrap: Option<api::replication::Bootstrap>,
        replicated: bool,
    ) -> Result<Self, Refusal> {
        if let Some(declaration) = &bootstrap
            && (node_id == 0 || !initial.voters.contains(&declaration.source_node))
        {
            return Err(api::invalid(
                "replica baseline",
                "the declared source must be an initial voter and the local node ID must be positive",
            ));
        }
        let mut db = open(&owner)?;
        db.hold_owner(owner.clone());
        // Check before retaining a baseline image or writing a replica marker.
        unproven_floor(&db).map_err(|e| crate::database::refused(&e))?;
        let mut marked = false;
        db.query(
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = '__amphora_replica'",
            [],
            |_| {
                marked = true;
                Ok(false)
            },
        )
        .map_err(unavailable)?;
        // Check unfinished legacy adoption before retaining any image or creating a marker.
        let mut restore_table = false;
        db.query(
            "SELECT 1 FROM sqlite_schema WHERE name = '__amphora_restore'",
            [],
            |_| {
                restore_table = true;
                Ok(false)
            },
        )
        .map_err(unavailable)?;
        if restore_table {
            let mut incomplete = false;
            db.query(
                "SELECT 1 FROM __amphora_restore WHERE done = 0 LIMIT 1",
                [],
                |_| {
                    incomplete = true;
                    Ok(false)
                },
            )
            .map_err(unavailable)?;
            if incomplete {
                return Err(api::invalid(
                    "replica baseline",
                    "complete the pinned restore before replicated startup",
                ));
            }
        }
        let baseline = bootstrap::Baseline::open(&mut db, node_id, bootstrap, marked)?;
        crate::database::bootstrap_db(&db)?;
        let mut incomplete = false;
        db.query("SELECT 1 FROM __amphora_restore WHERE done = 0", [], |_| {
            incomplete = true;
            Ok(false)
        })
        .map_err(unavailable)?;
        if incomplete {
            return Err(api::invalid(
                "replica baseline",
                "complete the pinned restore before replicated startup",
            ));
        }
        db.exec(META).map_err(unavailable)?;
        let mut state = None;
        db.query(
            "SELECT applied, term, configuration FROM __amphora_replica WHERE id = 1",
            [],
            |row| {
                let index = row.get::<String>(0).map_err(unavailable)?.parse()?;
                let term = row.get::<String>(1).map_err(unavailable)?.parse()?;
                let bytes = row
                    .get_value(2)
                    .to_blob()
                    .ok_or_else(|| anyhow::anyhow!("membership is not a blob"))?;
                state = Some((Point { index, term }, ConfState::decode(bytes)?));
                Ok(false)
            },
        )
        .map_err(unavailable)?;
        if marked && state.is_none() {
            return Err(Refusal::Fenced(
                "replica table lacks its durable applied marker".into(),
            ));
        }
        let (point, configuration) = state.unwrap_or((Point::default(), initial));
        db.transaction(|db| marker(db, point, &configuration))
            .map_err(unavailable)?;
        db.flusher()().map_err(unavailable)?;
        Ok(Self {
            db,
            point,
            flushed: point,
            replicated,
            configuration,
            baseline,
            snapshot_bytes: SNAPSHOT_BYTES,
            configuration_at: point.index,
            #[cfg(test)]
            floor_aware: true,
            #[cfg(test)]
            digests: None,
            _owner: owner,
        })
    }

    /// A member that joins catches up by this machine's image once the log no longer reaches
    /// back, and an image is bounded (replication.md, "Membership"): past its ceiling, a joiner
    /// is refused before it is added, never left unable to catch up.
    pub(crate) fn admits_joiner(&mut self) -> Result<(), Refusal> {
        let size = self.db.image_bytes().map_err(unavailable)?;
        api::bound(
            "database a joiner installs as a snapshot",
            size,
            self.snapshot_bytes,
        )
    }
    #[cfg(test)]
    pub(crate) fn set_snapshot_bytes(&mut self, bytes: usize) {
        self.snapshot_bytes = bytes;
    }

    /// The last configuration change's index as this machine knows it, or more.
    pub(crate) fn configuration_at(&self) -> u64 {
        self.configuration_at
    }
    /// What opening this machine again does to `configuration_at`: it starts at the applied
    /// point, as the machine no longer knows the last change's own index.
    #[cfg(test)]
    pub(crate) fn reopen_configuration_at(&mut self) {
        self.configuration_at = self.point.index;
    }
    #[cfg(test)]
    pub(crate) fn set_floor_aware(&mut self, aware: bool) {
        self.floor_aware = aware;
    }
    #[cfg(test)]
    fn writes_joins(&self) -> bool {
        self.floor_aware
    }
    #[cfg(not(test))]
    fn writes_joins(&self) -> bool {
        true
    }
    #[cfg(test)]
    pub(crate) fn record_digests(&mut self) {
        self.digests = Some(Vec::new());
    }
    #[cfg(test)]
    pub(crate) fn digests(&self) -> &[(u64, u64)] {
        self.digests.as_deref().unwrap_or_default()
    }
    #[cfg(test)]
    fn record_digest(&mut self, at: u64) -> Result<(), Fatal> {
        use std::hash::{Hash, Hasher};
        if self.digests.is_none() {
            return Ok(());
        }
        let pages = self.db.image_pages(self.snapshot_bytes).map_err(failed)?;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for (id, page) in &pages {
            // A page's last eight bytes are the engine's checksum of the rest, written when the
            // page is written out (engine storage/checksum.rs); whether a read sees it before or
            // after that is timing, not content. The digest is of the content.
            id.hash(&mut hasher);
            page.get(..page.len().saturating_sub(8)).hash(&mut hasher);
        }
        if let Some(digests) = self.digests.as_mut() {
            digests.push((at, hasher.finish()));
        }
        Ok(())
    }

    pub(crate) fn bootstrap_pending(&self) -> bool {
        self.baseline.pending()
    }
    pub(crate) fn bootstrap_source(&self) -> Option<u64> {
        self.baseline.source()
    }
    pub(crate) fn prepare_bootstrap(&self, entry_room: usize) -> Result<Option<Vec<u8>>, Refusal> {
        unproven_floor(&self.db).map_err(|e| crate::database::refused(&e))?;
        self.baseline.proposal(entry_room)
    }
    fn require_baseline(&self) -> Result<(), Refusal> {
        if self.baseline.pending() {
            return Err(api::invalid(
                "replica baseline",
                "initial image has not committed; canonical operations are withheld",
            ));
        }
        Ok(())
    }

    pub(crate) fn prepare_db<R>(
        &mut self,
        f: impl FnOnce(&mut Db) -> anyhow::Result<R>,
    ) -> Result<(Prepared, R), Refusal> {
        self.require_baseline()?;
        let (answer, pages) = self
            .db
            .prepare_pages(COMMAND_BYTES, |db| {
                let answer = f(db)?;
                Ok(answer)
            })
            .map_err(|e| crate::database::refused(&e))?;
        validate_pages(&pages, COMMAND_BYTES)?;
        Ok((
            Prepared {
                base_index: self.point.index,
                base_term: self.point.term,
                pages,
                baseline: self.baseline.declaration().cloned(),
            },
            answer,
        ))
    }

    pub(crate) fn prepare_compaction(
        &mut self,
        namespace: &str,
        before: api::Rev,
        until: std::time::Instant,
        room: usize,
    ) -> Result<(Prepared, api::CompactionProgress), Refusal> {
        use crate::database::compaction;
        self.require_baseline()?;
        let room = room.min(COMMAND_BYTES);
        let pages = compaction::page_limit(room)?;
        let (answer, image) = compaction::retry(|visits| {
            let result = self.db.compaction_attempt(pages, true, |db| {
                let _deadline = db.deadline(until);
                compaction::ready(db, namespace, until, false)?;
                compaction::step(db, namespace, before, visits)
            });
            compaction::deadline_result(result, until)
        })?;
        validate_pages(&image, COMMAND_BYTES)?;
        let prepared = Prepared {
            base_index: self.point.index,
            base_term: self.point.term,
            pages: image,
            baseline: self.baseline.declaration().cloned(),
        };
        // The hard page fence proves the worst-case bound; retain the exact serialization check
        // before proposal as a format-change and arithmetic guard.
        api::bound("bounded compaction command", prepared.encode()?.len(), room)?;
        Ok((prepared, answer))
    }

    pub(crate) fn read_db<R>(
        &mut self,
        f: impl FnOnce(&mut Db) -> anyhow::Result<R>,
    ) -> Result<R, Refusal> {
        let (prepared, answer) = self.prepare_db(f)?;
        if !prepared.pages.is_empty() {
            return Err(api::invalid(
                "quorum read",
                "the session attempted to write",
            ));
        }
        Ok(answer)
    }

    /// Only a confirmed leader may prepare. Its owner serializes proposals until this one is
    /// applied or refused. The base fence also refuses any preparation stale by application time.
    pub fn prepare<R>(
        &mut self,
        f: impl FnOnce(&mut crate::sql::Connection<'_>) -> anyhow::Result<R>,
    ) -> Result<(Prepared, R), Refusal> {
        self.require_baseline()?;
        let (answer, pages) = self
            .db
            .prepare_pages(COMMAND_BYTES, |db| {
                let mut connection = crate::sql::Connection::new(db);
                let value = f(&mut connection)?;
                connection.finish()?;
                Ok(value)
            })
            .map_err(|e| crate::database::refused(&e))?;
        validate_pages(&pages, COMMAND_BYTES)?;
        Ok((
            Prepared {
                base_index: self.point.index,
                base_term: self.point.term,
                pages,
                baseline: self.baseline.declaration().cloned(),
            },
            answer,
        ))
    }

    /// The owner calls this only after hyper-durable releases the matching ReadIndex context.
    pub fn read<R>(
        &mut self,
        f: impl FnOnce(&mut crate::sql::Connection<'_>) -> anyhow::Result<R>,
    ) -> Result<R, Refusal> {
        self.require_baseline()?;
        let (answer, pages) = self
            .db
            .prepare_pages(COMMAND_BYTES, |db| {
                let mut connection = crate::sql::Connection::new(db);
                let value = f(&mut connection)?;
                connection.finish()?;
                Ok(value)
            })
            .map_err(|e| crate::database::refused(&e))?;
        if !pages.is_empty() {
            return Err(api::invalid(
                "quorum read",
                "the session attempted to write",
            ));
        }
        Ok(answer)
    }

    /// Applies `pages` at `at`, and flushes the log with them unless `flush` says the owner will.
    fn advance(
        &mut self,
        at: Point,
        pages: &[(u32, Vec<u8>)],
        configuration: &ConfState,
        flush: bool,
    ) -> Result<(), Fatal> {
        self.advance_with(at, pages, configuration, flush, |_| Ok(()))
    }

    /// Applies `pages` and the applied marker, and `also` in the same transaction.
    fn advance_with(
        &mut self,
        at: Point,
        pages: &[(u32, Vec<u8>)],
        configuration: &ConfState,
        flush: bool,
        also: impl FnOnce(&mut Db) -> anyhow::Result<()>,
    ) -> Result<(), Fatal> {
        self.db
            .apply_pages(pages, |db| {
                marker(db, at, configuration)?;
                also(db)
            })
            .map_err(failed)?;
        if flush {
            self.db.flusher()().map_err(failed)?;
            self.flushed = at;
        }
        self.point = at;
        self.configuration = configuration.clone();
        #[cfg(test)]
        self.record_digest(at.index)?;
        Ok(())
    }

    /// The last entry applied that no finished flush covers yet: in the replicated class only.
    pub(crate) fn unflushed(&self) -> Option<Point> {
        if self.point.index > self.flushed.index {
            Some(self.point)
        } else {
            None
        }
    }

    /// Entries applied that no finished flush covers yet: never below none, as a flush never
    /// covers more than was applied.
    pub(crate) fn flush_backlog(&self) -> u64 {
        self.point.index.saturating_sub(self.flushed.index)
    }

    /// A flush of the database's log, to run on the owner's flushing thread: what was committed
    /// before it began is durable once it ends (`Db::flusher`).
    pub(crate) fn log_flusher(&self) -> crate::store::Flush {
        self.db.flusher()
    }

    /// A flush begun once `at` was applied has finished: a restart opens with `at` applied.
    pub(crate) fn flushed_through(&mut self, at: Point) {
        if at.index > self.flushed.index && at.index <= self.point.index {
            self.flushed = at;
        }
    }
}

impl StateMachine for Machine {
    type Answer = Applied;

    fn apply(&mut self, entry: &EntryRef<'_>, answers: &mut Vec<Applied>) -> Result<(), Fatal> {
        if entry.index <= self.point.index {
            return Err(failed("a committed entry was applied twice"));
        }
        let at = Point {
            index: entry.index,
            term: entry.term,
        };
        if !entry.data.is_empty()
            && self
                .baseline
                .apply_seed(&mut self.db, entry.data, |db| {
                    crate::database::bootstrap_db(db)?;
                    db.exec(META)?;
                    marker(db, at, &self.configuration)
                })
                .map_err(failed)?
        {
            self.point = at;
            self.flushed = at;
            answers.push(Applied {
                index: entry.index,
                context: entry.context.to_vec(),
                result: Ok(()),
            });
            return Ok(());
        }
        let mut result = Ok(());
        let mut pages = Vec::new();
        if !entry.data.is_empty() {
            self.require_baseline().map_err(failed)?;
            let prepared = Prepared::decode(entry.data).map_err(failed)?;
            if prepared.baseline.as_ref() != self.baseline.declaration() {
                return Err(failed("resolved delta baseline identity differs"));
            }
            result = on_base(prepared.base_index, prepared.base_term, self.point);
            if result.is_ok() {
                pages = prepared.pages;
            }
        }
        self.advance(at, &pages, &self.configuration.clone(), !self.replicated)?;
        if !entry.data.is_empty() {
            answers.push(Applied {
                index: entry.index,
                context: entry.context.to_vec(),
                result,
            });
        }
        Ok(())
    }

    fn apply_change(
        &mut self,
        at: Point,
        _: &ConfChangeV2,
        configuration: &ConfState,
    ) -> Result<(), Fatal> {
        if self.writes_joins() {
            // The join rows go with the change in its own transaction (replication.md, "Joins"):
            // a change another lead replaces is never applied, so it writes nothing.
            self.advance_with(at, &[], configuration, true, |db| {
                floor::apply_joins(db, at.index, configuration)
            })?;
        } else {
            self.advance(at, &[], configuration, true)?;
        }
        self.configuration_at = at.index;
        Ok(())
    }
    fn durable(&self) -> Point {
        self.flushed
    }
    fn configuration(&self) -> &ConfState {
        &self.configuration
    }
    /// Every entry outside the replicated class, which the shell applies only on the member's durable
    /// commit (durable.md §4.1). In the class only the seed's: an ordinary entry is applied on the
    /// commit, held or classic, and a restart that finds it past the member's log opens the log at
    /// the machine's point (durable.md §4.3).
    fn acts_at_start(&self, entry: &EntryRef<'_>) -> bool {
        !self.replicated || bootstrap::is_seed(entry.data)
    }

    fn image(&mut self, into: &mut Vec<u8>) -> Result<(Point, ConfState), Fatal> {
        self.require_baseline().map_err(failed)?;
        let pages = self.db.image_pages(self.snapshot_bytes).map_err(failed)?;
        let bytes = if let Some(declaration) = self.baseline.declaration() {
            let mut bytes = BASELINE_SNAPSHOT.to_vec();
            bytes.extend(
                postcard::to_allocvec(&BaselineSnapshot {
                    declaration: declaration.clone(),
                    pages,
                })
                .map_err(failed)?,
            );
            bytes
        } else {
            postcard::to_allocvec(&pages).map_err(failed)?
        };
        api::bound(
            "snapshot",
            bytes.len(),
            self.snapshot_bytes.saturating_add(ENVELOPE_BYTES),
        )
        .map_err(failed)?;
        into.extend_from_slice(&bytes);
        Ok((self.point, self.configuration.clone()))
    }
    fn image_bytes(&self) -> Option<u64> {
        None
    }
    fn install(&mut self, image: &[u8], at: Point, configuration: &ConfState) -> Result<(), Fatal> {
        api::bound(
            "snapshot",
            image.len(),
            self.snapshot_bytes.saturating_add(ENVELOPE_BYTES),
        )
        .map_err(failed)?;
        let pages: Vec<(u32, Vec<u8>)> =
            if let Some(payload) = image.strip_prefix(BASELINE_SNAPSHOT) {
                let snapshot: BaselineSnapshot = postcard::from_bytes(payload).map_err(failed)?;
                if self.baseline.declaration() != Some(&snapshot.declaration) {
                    return Err(failed("snapshot baseline declaration differs"));
                }
                snapshot.pages
            } else {
                if self.baseline.declaration().is_some() {
                    return Err(failed("snapshot lacks declared baseline identity"));
                }
                postcard::from_bytes(image).map_err(failed)?
            };
        validate_pages(&pages, self.snapshot_bytes).map_err(failed)?;
        if pages.first().map(|p| p.0) != Some(1) {
            return Err(failed("snapshot lacks its database header"));
        }
        self.db
            .apply_pages(&pages, |db| {
                // Inside the image's transaction: a refusal rolls it back before its marker.
                unproven_floor(db)?;
                db.exec(META)?;
                marker(db, at, configuration)?;
                self.baseline.completed_marker(db)
            })
            .map_err(failed)?;
        self.db.flusher()().map_err(failed)?;
        self.point = at;
        self.flushed = at;
        self.configuration = configuration.clone();
        self.configuration_at = at.index;
        self.baseline.completed();
        #[cfg(test)]
        self.record_digest(at.index)?;
        Ok(())
    }
    fn persist(&mut self) -> Result<(), Fatal> {
        self.db.flusher()().map_err(failed)?;
        self.flushed = self.point;
        Ok(())
    }
}
