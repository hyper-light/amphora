//! A store in groups (CP-T1.1; docs/design/store.md, decision 10): stores on one node, each a
//! database file with a writer, a log, a flusher, a checkpointer and readers of its own, keys spread
//! over them by position (their FNV-1a hash), so writes go on in parallel on as many cores. A batch
//! is one group's: a batch with rows in more than one group is refused ([`Refused::Groups`]);
//! operations spanning groups atomically are CP-T1.10's.
//!
//! Which group holds a position is the directory's (CP-T1.9; docs/design/reshard.md): the router
//! routes by the directory it was last given ([`Groups::route`], from the directory group's
//! replicated state, [`crate::reshard::replicated`]), and each group answers only from its own
//! holding ([`Holding`]), checked on every write and read: a router with a stale directory is
//! refused ([`Refused::Away`]), never let write to a group that doesn't hold the key. The holding
//! is kept by the group's own engine with its rows ([`Engine::keep_holding`]), durable before it
//! is acted on, and a group started again answers from what it kept, whatever directory it is
//! started by: never from a router's memory.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use crate::meter::Counters;
use crate::reshard::directory::{Change, Directory, Proposal};
use crate::reshard::driver::{Action, Do, Failure, Reply};
use crate::reshard::holding::{Away, Fenced, Holding};
use crate::reshard::{self, GroupId, NodeId, Pos, Span, position};
use crate::store::{Config, Engine, Refused, Store, Ticket};

/// A group: its store, what it holds, and what the copy path's work has cost its writer.
struct Group<E: Engine> {
    store: Store<E>,
    holding: Holding,
    costs: Arc<Mutex<Costs>>,
}

impl<E: Engine> Clone for Group<E> {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            holding: self.holding,
            costs: Arc::clone(&self.costs),
        }
    }
}

impl<E: Engine> Group<E> {
    fn new(store: Store<E>, holding: Holding) -> Self {
        Self {
            store,
            holding,
            costs: Arc::default(),
        }
    }
}

/// The copy path's work on a group's writer (CP-T1.9): reading a span's keys, reading the keys a
/// run of the log changed, writing moved keys, sweeping.
#[derive(Debug, Clone, Copy)]
enum Work {
    Export,
    Changed,
    Import,
    Sweep,
}

/// What one kind of work has cost a group's writer, learned from every batch of it run there:
/// its units (positions read or swept, log entries replayed) and how long they held the writer;
/// and the units of the last batch.
#[derive(Debug, Default, Clone, Copy)]
struct Cost {
    units: u64,
    ns: u64,
    last: u64,
}

impl Cost {
    fn ran(&mut self, units: u64, took: Duration) {
        self.units = self.units.saturating_add(units);
        self.ns = self
            .ns
            .saturating_add(u64::try_from(took.as_nanos()).unwrap_or(u64::MAX));
        self.last = units;
    }

    /// The units a batch takes to hold the writer about `target` ns, a commit's mean cost there
    /// (docs/design/reshard.md, decision "Batches learned where they run"): the target over the
    /// mean cost of a unit measured so far, at least one. Not yet measured, one unit, and from
    /// then on learned. No commit measured on the node (no write waits on it), the batch doubles
    /// from the last, as TCP's slow start probes (RFC 5681, section 3.1), so a batch's size is
    /// never picked.
    fn within(&self, target: Option<u128>) -> u64 {
        match target {
            Some(t) if self.units > 0 => {
                let n = t
                    .saturating_mul(u128::from(self.units))
                    .checked_div(u128::from(self.ns.max(1)))
                    .unwrap_or(1);
                u64::try_from(n).unwrap_or(u64::MAX).max(1)
            }
            Some(_) => 1,
            None => self.last.saturating_mul(2).max(1),
        }
    }
}

/// A group's [`Cost`] of each [`Work`].
#[derive(Debug, Default)]
struct Costs([Cost; 4]);

impl Costs {
    fn of(&mut self, work: Work) -> &mut Cost {
        let [export, changed, import, sweep] = &mut self.0;
        match work {
            Work::Export => export,
            Work::Changed => changed,
            Work::Import => import,
            Work::Sweep => sweep,
        }
    }
}

/// The router's directory and the groups on this node. A write checks and hands over its batch
/// under the read lock, and a change of a holding (a fence) takes the write lock: so every batch
/// let in before a fence is in its group's writer's queue before the fence is.
struct Route<E: Engine> {
    directory: Directory,
    groups: BTreeMap<GroupId, Group<E>>,
    /// Groups retired here that the directory routed by doesn't yet show retired (an id it has
    /// given out, a group it no longer places): a create, release or retire asked of one again
    /// is answered as the driver's contract says, never by making an empty group. Kept in `book`
    /// before a retire is answered, so a restart keeps it too; an id leaves it once the directory
    /// routed by shows it retired, so it holds no more than the directory lags this node by.
    retired: BTreeSet<GroupId>,
    book: PathBuf,
}

impl<E: Engine> Route<E> {
    /// Whether `group` was retired, here or as the directory routed by shows it.
    fn was_retired(&self, group: GroupId) -> bool {
        self.retired.contains(&group) || shows_retired(&self.directory, group)
    }
}

/// Whether `directory` shows `group` retired: its id given out and the group placed nowhere.
fn shows_retired(directory: &Directory, group: GroupId) -> bool {
    directory.given_out(group) && directory.node_of(group).is_none()
}

/// The retired groups kept in `book` (none: no book yet, a node started now).
fn read_book(book: &Path) -> anyhow::Result<BTreeSet<GroupId>> {
    match std::fs::read(book) {
        Ok(bytes) => {
            let ids: Vec<u64> = postcard::from_bytes(&bytes)
                .map_err(|_| anyhow::anyhow!("a book of retired groups that doesn't decode"))?;
            Ok(ids.into_iter().map(GroupId).collect())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeSet::new()),
        Err(e) => Err(e.into()),
    }
}

/// `retired` kept in `book`, replaced whole and durable before it returns.
fn keep_book(book: &Path, retired: &BTreeSet<GroupId>) -> anyhow::Result<()> {
    let ids: Vec<u64> = retired.iter().map(|g| g.0).collect();
    let bytes = postcard::to_allocvec(&ids)
        .map_err(|_| anyhow::anyhow!("a book of retired groups that doesn't encode"))?;
    reshard::replicated::replace(book, &bytes)?;
    Ok(())
}

/// A store in groups: cheap to clone, usable from any thread; clones share one route.
pub struct Groups<E: Engine> {
    route: Arc<RwLock<Route<E>>>,
    config: Config,
}

impl<E: Engine> Clone for Groups<E> {
    fn clone(&self) -> Self {
        Self {
            route: Arc::clone(&self.route),
            config: self.config,
        }
    }
}

/// A batch handed to a group's writer: [`Groups::durable`] says what became of it.
#[derive(Debug, Clone)]
pub struct GroupTicket {
    group: GroupId,
    ticket: Ticket,
}

impl GroupTicket {
    /// The group its batch went to.
    #[must_use]
    pub fn group(&self) -> GroupId {
        self.group
    }
}

/// FNV-1a: a key's position stays put from build to build and machine to machine.
pub(crate) fn fnv(key: &str) -> u64 {
    key.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// A directory of `n` groups (at least one) on `node`, the `i`th holding the `i`th of `n` equal
/// runs of positions, made by splits from the one group every database starts as: the same changes
/// the directory group's log carries, so it is a directory like any other.
pub fn even(node: NodeId, n: usize) -> Result<Directory, reshard::Refused> {
    let mut directory = Directory::new(node);
    let n = u128::try_from(n.max(1)).map_err(|_| reshard::Refused::Invalid("too many groups"))?;
    let mut last = GroupId(0);
    for i in 1..n {
        let at = i
            .checked_shl(64)
            .and_then(|i| i.checked_div(n))
            .and_then(|at| Pos::try_from(at).ok())
            .ok_or(reshard::Refused::Invalid("a cut past the last position"))?;
        let begin = directory.propose(&Proposal::Split { group: last, at })?;
        let Change::Begin { op, .. } = &begin else {
            return Err(reshard::Refused::Invalid("a proposal begins an operation"));
        };
        let (id, made) = (op.id, op.to);
        for change in [
            begin,
            Change::Cutover(id),
            Change::Flip(id),
            Change::End(id),
        ] {
            directory.apply(&change)?;
        }
        last = made;
    }
    Ok(directory)
}

/// `holding` as an engine keeps it.
fn encode(holding: &Holding) -> anyhow::Result<Vec<u8>> {
    postcard::to_allocvec(holding).map_err(|_| anyhow::anyhow!("a holding that doesn't encode"))
}

/// The holding `engine` keeps, or, if it keeps none (a group made now), one holding `first`, kept
/// before the group takes anything.
fn kept<E: Engine>(engine: &mut E, first: Option<Span>) -> anyhow::Result<Holding> {
    if let Some(bytes) = engine.holding()? {
        return postcard::from_bytes(&bytes)
            .map_err(|_| anyhow::anyhow!("a kept holding that doesn't decode"));
    }
    let holding = Holding::owning(first);
    engine.keep_holding(&encode(&holding)?)?;
    Ok(holding)
}

impl<E: Engine> Groups<E> {
    /// `groups` stores (at least one) over equal runs of positions ([`even`]), the `i`th started
    /// on `engine(i)` with `config`, the groups retired here kept in `retired` ([`Groups::on`]).
    pub fn start(
        groups: usize,
        config: Config,
        retired: &Path,
        mut engine: impl FnMut(usize) -> anyhow::Result<E>,
    ) -> anyhow::Result<Self> {
        let directory = even(NodeId(0), groups)?;
        Self::on(&directory, config, retired, |g| {
            engine(usize::try_from(g.0).map_err(|_| anyhow::anyhow!("group {g:?}"))?)
        })
    }

    /// A store for each group `directory` places, started on `engine(group)` with `config`,
    /// each holding what its engine kept, or, if it kept nothing (a group made now), its span.
    /// `retired` is the file this node keeps the groups retired here in (one per node; none yet:
    /// a node started now): a group retired before a restart stays retired after it.
    pub fn on(
        directory: &Directory,
        config: Config,
        retired: &Path,
        mut engine: impl FnMut(GroupId) -> anyhow::Result<E>,
    ) -> anyhow::Result<Self> {
        directory.check()?;
        let book = retired.to_path_buf();
        let retired = read_book(&book)?;
        let mut groups = BTreeMap::new();
        for (group, _) in directory.groups() {
            if retired.contains(&group) {
                continue;
            }
            let mut engine = engine(group)?;
            let holding = kept(&mut engine, directory.span_of(group))?;
            let store = Store::start(engine, config)?;
            groups.insert(group, Group::new(store, holding));
        }
        let route = Route {
            directory: directory.clone(),
            groups,
            retired,
            book,
        };
        Ok(Self {
            route: Arc::new(RwLock::new(route)),
            config,
        })
    }

    /// Route by `directory` from now on, if it is newer than the one routed by: the directory
    /// group's, as this member has applied it.
    pub fn route(&self, directory: &Directory) -> Result<(), reshard::Refused> {
        directory.check()?;
        let mut route = self.route.write().unwrap_or_else(PoisonError::into_inner);
        if directory.generation() > route.directory.generation() {
            route.directory = directory.clone();
            // The groups the directory now shows retired leave the book. If it can't be kept,
            // it keeps them all, which is as true: they are dropped at the next directory.
            let shown: Vec<GroupId> = route
                .retired
                .iter()
                .copied()
                .filter(|g| shows_retired(directory, *g))
                .collect();
            if !shown.is_empty() {
                let mut left = route.retired.clone();
                left.retain(|g| !shown.contains(g));
                if keep_book(&route.book, &left).is_ok() {
                    route.retired = left;
                }
            }
        }
        Ok(())
    }

    /// The generation of the directory routed by.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.route
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .directory
            .generation()
    }

    /// A new group (the driver's `Do::Create`), holding nothing until an operation hands it a
    /// span (or what its engine kept, if it is made again after a restart), its store started on
    /// `engine`. Making one that is here already does nothing.
    pub fn create(
        &self,
        group: GroupId,
        engine: impl FnOnce() -> anyhow::Result<E>,
    ) -> anyhow::Result<()> {
        let mut route = self.route.write().unwrap_or_else(PoisonError::into_inner);
        if route.groups.contains_key(&group) {
            return Ok(());
        }
        let mut engine = engine()?;
        let holding = kept(&mut engine, None)?;
        let store = Store::start(engine, self.config)?;
        route.groups.insert(group, Group::new(store, holding));
        Ok(())
    }

    /// `f` on `group`'s holding: how the shell carries out a driver's action at the group (its
    /// receive, copy and catch-up progress, fence, activate, release, unfence or discard). What
    /// `f` changes is kept by the group's engine, after every batch let in before it and durable
    /// with them, before the group acts on it; if it can't be kept (or not by `until`), the group
    /// holds what it held, and the driver asks again (each action is idempotent at its group).
    /// Under the route's write lock, so no batch is let in between. A refusal of `f` is a
    /// [`reshard::Refused`] in the error.
    pub fn hold<T>(
        &self,
        group: GroupId,
        until: Instant,
        f: impl FnOnce(&mut Holding) -> Result<T, reshard::Refused>,
    ) -> anyhow::Result<T> {
        let mut route = self.route.write().unwrap_or_else(PoisonError::into_inner);
        let held = route
            .groups
            .get_mut(&group)
            .ok_or(reshard::Refused::NoGroup(group))?;
        let mut next = held.holding;
        let out = f(&mut next)?;
        if next != held.holding {
            let bytes = encode(&next)?;
            held.store.run(until, move |e| e.keep_holding(&bytes))?;
            held.holding = next;
        }
        Ok(out)
    }

    /// The store of `group`, if it is on this node.
    fn store_of(&self, group: GroupId) -> Option<Store<E>> {
        let route = self.route.read().unwrap_or_else(PoisonError::into_inner);
        route.groups.get(&group).map(|g| g.store.clone())
    }

    /// The holding `group` answers from now, if it is on this node.
    #[must_use]
    pub fn holding_of(&self, group: GroupId) -> Option<Holding> {
        let route = self.route.read().unwrap_or_else(PoisonError::into_inner);
        route.groups.get(&group).map(|g| g.holding)
    }

    /// Carries out a driver's action at this node's groups and answers it (CP-T1.9; the shell of
    /// docs/design/reshard.md, "What is left"): a group's holding through [`Groups::hold`], kept
    /// before it is acted on; a copy or catch-up batch read on the old group's writer, after every
    /// batch handed to it before, and written to the new group's in one transaction; a fence as
    /// an entry of the group's own log, numbered on its writer, so every batch let in before it
    /// is in the log before it. `engine` starts a group's engine (`Do::Create`). Both groups of a
    /// copy or catch-up must be on this node (a move to another node carries rows over the
    /// network, not built yet): a group not here is [`Failure::NoGroup`].
    ///
    /// A batch is as many positions (copy, sweep) or log entries (catch-up) as hold each writer
    /// it runs on about as long as a commit there does, learned from the batches run on it before
    /// ([`Cost::within`]), so a batch delays a write no longer than a commit of its own. An error
    /// is the group's (its engine or its writer): the driver asks again
    /// ([`reshard::driver::Driver::restarted`]).
    pub fn carry(
        &self,
        action: &Action,
        until: Instant,
        engine: impl FnOnce(GroupId) -> anyhow::Result<E>,
    ) -> anyhow::Result<Reply> {
        let op = action.op;
        let refused = |e: anyhow::Error| match e.downcast::<reshard::Refused>() {
            Ok(reshard::Refused::NoGroup(_)) => Ok(Reply::Failed(Failure::NoGroup)),
            Ok(r) => Ok(Reply::Failed(Failure::Refused(r))),
            Err(e) => Err(e),
        };
        match action.what {
            Do::Create { group, .. } => {
                let retired = self
                    .route
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .was_retired(group);
                if retired {
                    return Ok(Reply::Failed(Failure::Refused(reshard::Refused::Ended(op))));
                }
                self.create(group, || engine(group))?;
                Ok(Reply::Created)
            }
            Do::Copy { from, to, span } => {
                let (Some(src), Some(_)) = (self.store_of(from), self.store_of(to)) else {
                    return Ok(Reply::Failed(Failure::NoGroup));
                };
                let progress = match self.hold(to, until, |h| h.receive(op, span)) {
                    Ok(p) => p,
                    Err(e) => return refused(e),
                };
                let rest = match progress.copied {
                    Some(p) if p >= span.hi() => None,
                    Some(p) => p.checked_add(1).and_then(|lo| Span::new(lo, span.hi())),
                    None => Some(span),
                };
                let first = progress.first_at.is_none();
                let batch = self
                    .batch(from, Work::Export)
                    .min(self.batch(to, Work::Import));
                let positions = usize::try_from(batch).unwrap_or(usize::MAX);
                // The old group's log is held from the first copy's position on, read at the same
                // entry as the batch, so no entry catch-up needs is let go between them.
                let (exported, took) = src.run(until, move |e| {
                    let began = Instant::now();
                    let head = e.head()?;
                    if first {
                        e.keep_log(Some(head))?;
                    }
                    let exported = match rest {
                        Some(rest) => e.export(rest, positions)?,
                        None => crate::store::Exported {
                            keys: Vec::new(),
                            at: head,
                            last: None,
                            done: true,
                        },
                    };
                    Ok((exported, began.elapsed()))
                })?;
                let units = u64::try_from(exported.keys.len())
                    .unwrap_or(u64::MAX)
                    .max(1);
                self.ran(from, Work::Export, units, took);
                let dst = self.store_of(to).ok_or(reshard::Refused::NoGroup(to))?;
                let at = exported.at;
                let (last, done) = (exported.last, exported.done);
                if !exported.keys.is_empty() {
                    let keys = exported.keys;
                    let took = dst.run(until, move |e| {
                        let began = Instant::now();
                        e.import(&keys)?;
                        Ok(began.elapsed())
                    })?;
                    self.ran(to, Work::Import, units, took);
                }
                match self.hold(to, until, |h| h.copied(op, last, done, at)) {
                    Ok(progress) => Ok(Reply::Copied { progress, head: at }),
                    Err(e) => refused(e),
                }
            }
            Do::Replay {
                from,
                to,
                span,
                upto,
            } => {
                let (Some(src), Some(dst)) = (self.store_of(from), self.store_of(to)) else {
                    return Ok(Reply::Failed(Failure::NoGroup));
                };
                let Some(incoming) = self
                    .holding_of(to)
                    .and_then(|h| h.incoming())
                    .filter(|i| i.op == op)
                else {
                    return Ok(Reply::Failed(Failure::Refused(reshard::Refused::Ended(op))));
                };
                let after = incoming.progress.replayed;
                let entries = self
                    .batch(from, Work::Changed)
                    .min(self.batch(to, Work::Import));
                let (changed, took) = src.run(until, move |e| {
                    let began = Instant::now();
                    let changed = e.changed(after, upto, span, entries)?;
                    Ok((changed, began.elapsed()))
                })?;
                let Some(changed) = changed else {
                    return Ok(Reply::Failed(Failure::LogGone));
                };
                let (through, head) = (changed.through, changed.head);
                let units = through.saturating_sub(after).max(1);
                self.ran(from, Work::Changed, units, took);
                if !changed.keys.is_empty() {
                    let keys = changed.keys;
                    let took = dst.run(until, move |e| {
                        let began = Instant::now();
                        e.import(&keys)?;
                        Ok(began.elapsed())
                    })?;
                    self.ran(to, Work::Import, units, took);
                }
                match self.hold(to, until, |h| h.replayed(op, through)) {
                    Ok(progress) => Ok(Reply::Replayed { progress, head }),
                    Err(e) => refused(e),
                }
            }
            Do::Fence { group, span } => match self.fence(group, op, span, until) {
                Ok(at) => Ok(Reply::Fenced { at }),
                Err(e) => refused(e),
            },
            Do::Activate { group } => match self.hold(group, until, |h| h.activate(op)) {
                Ok(()) => Ok(Reply::Activated),
                Err(e) => refused(e),
            },
            Do::Unfence { group } => {
                let Some(store) = self.store_of(group) else {
                    return Ok(Reply::Unfenced);
                };
                if let Err(e) = self.hold(group, until, |h| h.unfence(op)) {
                    return refused(e);
                }
                store.run(until, |e| e.keep_log(None))?;
                Ok(Reply::Unfenced)
            }
            Do::Release { group, span } | Do::Discard { group, span } => {
                let release = matches!(action.what, Do::Release { .. });
                let Some(store) = self.store_of(group) else {
                    // A retired group held nothing when it was retired: a release asked of it
                    // again is done. A group never made has nothing to discard.
                    let retired = {
                        let route = self.route.read().unwrap_or_else(PoisonError::into_inner);
                        route.retired.contains(&group) || route.directory.given_out(group)
                    };
                    return Ok(match (release, retired) {
                        (true, true) => Reply::Released { done: true },
                        (true, false) => Reply::Failed(Failure::NoGroup),
                        (false, _) => Reply::Discarded { done: true },
                    });
                };
                let r = if release {
                    self.hold(group, until, |h| h.release(op))
                } else {
                    self.hold(group, until, |h| h.discard(op))
                };
                if let Err(e) = r {
                    return refused(e);
                }
                let holding = self
                    .holding_of(group)
                    .ok_or(reshard::Refused::NoGroup(group))?;
                let batch = self.batch(group, Work::Sweep);
                let positions = usize::try_from(batch).unwrap_or(usize::MAX);
                let (done, took) = store.run(until, move |e| {
                    let began = Instant::now();
                    if release {
                        e.keep_log(None)?;
                    }
                    let done = e.sweep(span, positions, &|pos| holding.garbage(pos))?;
                    Ok((done, began.elapsed()))
                })?;
                self.ran(group, Work::Sweep, batch, took);
                Ok(if release {
                    Reply::Released { done }
                } else {
                    Reply::Discarded { done }
                })
            }
            Do::Retire { group } => {
                let mut route = self.route.write().unwrap_or_else(PoisonError::into_inner);
                if let Some(held) = route.groups.get(&group) {
                    let h = held.holding;
                    if h.owned().is_some() || h.incoming().is_some() || h.fenced().is_some() {
                        return Ok(Reply::Failed(Failure::Refused(reshard::Refused::Invalid(
                            "a group retired while it holds a span",
                        ))));
                    }
                    route.groups.remove(&group);
                }
                if !route.was_retired(group) {
                    // Kept before it is answered: a create asked again after a restart is refused,
                    // never made an empty group.
                    let mut next = route.retired.clone();
                    next.insert(group);
                    keep_book(&route.book, &next)?;
                    route.retired = next;
                }
                Ok(Reply::Retired)
            }
        }
    }

    /// `group` fences `span` for `op`: the fence's entry numbered on the group's writer, after
    /// every batch let in before it (under the route's write lock, so none is let in between),
    /// and the holding kept as that entry. The entry's number.
    fn fence(
        &self,
        group: GroupId,
        op: reshard::OpId,
        span: Span,
        until: Instant,
    ) -> anyhow::Result<reshard::Seq> {
        let mut route = self.route.write().unwrap_or_else(PoisonError::into_inner);
        let held = route
            .groups
            .get_mut(&group)
            .ok_or(reshard::Refused::NoGroup(group))?;
        let holding = held.holding;
        let (at, kept) = held.store.run(until, move |e| {
            let mut next = holding;
            let next_entry = e
                .head()?
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("the log's entries ran out"))?;
            match next.fence(op, span, next_entry)? {
                Fenced::Already(at) => Ok((at, None)),
                Fenced::New(at) => {
                    e.keep_holding(&encode(&next)?)?;
                    Ok((at, Some(next)))
                }
            }
        })?;
        if let Some(kept) = kept {
            held.holding = kept;
        }
        Ok(at)
    }

    /// The units of `work` a batch on `group`'s writer takes ([`Cost::within`]): to hold it
    /// about as long as a commit of its own does, or, if it has committed nothing yet, a commit
    /// of this node's groups (one disk, one machine).
    fn batch(&self, group: GroupId, work: Work) -> u64 {
        let route = self.route.read().unwrap_or_else(PoisonError::into_inner);
        let mean = |c: &Counters| {
            u128::from(c.apply_us)
                .saturating_mul(1000)
                .checked_div(u128::from(c.commits))
        };
        let Some(held) = route.groups.get(&group) else {
            return 1;
        };
        let target = mean(&held.store.counters()).or_else(|| {
            let (mut us, mut commits) = (0u64, 0u64);
            for g in route.groups.values() {
                let c = g.store.counters();
                us = us.saturating_add(c.apply_us);
                commits = commits.saturating_add(c.commits);
            }
            mean(&Counters {
                commits,
                apply_us: us,
                ..Counters::default()
            })
        });
        let mut costs = held.costs.lock().unwrap_or_else(PoisonError::into_inner);
        costs.of(work).within(target)
    }

    /// `units` of `work` held `group`'s writer for `took`.
    fn ran(&self, group: GroupId, work: Work, units: u64, took: Duration) {
        let route = self.route.read().unwrap_or_else(PoisonError::into_inner);
        if let Some(held) = route.groups.get(&group) {
            let mut costs = held.costs.lock().unwrap_or_else(PoisonError::into_inner);
            costs.of(work).ran(units, took);
        }
    }

    /// How many groups are on this node.
    #[must_use]
    pub fn len(&self) -> usize {
        self.route
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .groups
            .len()
    }

    /// Whether no group is on this node (every group's span has moved to other nodes).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The group the directory routed by places `key` in (none only of a directory broken in
    /// memory, which [`Groups::route`] never takes).
    #[must_use]
    pub fn group(&self, key: &str) -> Option<GroupId> {
        let route = self.route.read().unwrap_or_else(PoisonError::into_inner);
        route.directory.owner(position(key))
    }

    /// The store of the group `positions` all route to, once its holding answers every one of
    /// them: under the route's read lock, held while `then` hands the batch over.
    fn routed<T>(
        &self,
        mut positions: impl Iterator<Item = Pos>,
        then: impl FnOnce(GroupId, &Store<E>) -> Result<T, Refused>,
    ) -> Result<T, Refused> {
        let route = self.route.read().unwrap_or_else(PoisonError::into_inner);
        let first = positions.next().unwrap_or(0);
        let group = route.directory.owner(first).ok_or(Refused::Groups)?;
        // A group the directory places on another node doesn't answer here.
        let held = route
            .groups
            .get(&group)
            .ok_or(Refused::Away(Away::Elsewhere))?;
        held.holding.answers(first).map_err(Refused::Away)?;
        for pos in positions {
            if route.directory.owner(pos) != Some(group) {
                return Err(Refused::Groups);
            }
            held.holding.answers(pos).map_err(Refused::Away)?;
        }
        then(group, &held.store)
    }

    fn at<T>(&self, group: GroupId, then: impl FnOnce(&Store<E>) -> T) -> Result<T, Refused> {
        let route = self.route.read().unwrap_or_else(PoisonError::into_inner);
        let held = route.groups.get(&group).ok_or(Refused::Groups)?;
        Ok(then(&held.store))
    }

    /// [`Store::put`] to the rows' group.
    pub fn put(&self, rows: Vec<E::Row>) -> Result<GroupTicket, Refused> {
        let positions: Vec<Pos> = rows.iter().map(|r| position(E::key(r))).collect();
        self.routed(positions.into_iter(), |group, store| {
            let ticket = store.put(rows)?;
            Ok(GroupTicket { group, ticket })
        })
    }

    /// [`Store::put_wait`] to the rows' group. It waits for room under the route's read lock, so a
    /// fence waits for it too, no longer than `until`.
    pub fn put_wait(&self, rows: Vec<E::Row>, until: Instant) -> Result<GroupTicket, Refused> {
        let positions: Vec<Pos> = rows.iter().map(|r| position(E::key(r))).collect();
        self.routed(positions.into_iter(), |group, store| {
            let ticket = store.put_wait(rows, until)?;
            Ok(GroupTicket { group, ticket })
        })
    }

    /// [`Store::put_then`] to the rows' group.
    pub fn put_then(
        &self,
        rows: Vec<E::Row>,
        until: Instant,
        then: impl FnOnce(Result<(), Refused>) + Send + 'static,
    ) -> Result<GroupTicket, Refused> {
        let positions: Vec<Pos> = rows.iter().map(|r| position(E::key(r))).collect();
        self.routed(positions.into_iter(), |group, store| {
            let ticket = store.put_then(rows, until, then)?;
            Ok(GroupTicket { group, ticket })
        })
    }

    /// [`Store::durable`] in the ticket's group.
    pub fn durable(&self, ticket: &GroupTicket, until: Instant) -> Result<(), Refused> {
        let store = self.at(ticket.group, Store::clone)?;
        store.durable(&ticket.ticket, until)
    }

    /// [`Store::sync`] in every group: everything handed over so far, settled and durable.
    pub fn sync(&self, until: Instant) -> Result<(), Refused> {
        self.stores().iter().try_for_each(|s| s.sync(until))
    }

    /// [`Store::read`] in `key`'s group, once its writes handed over before are committed. It
    /// reads under the route's read lock, so no fence and flip can pass it and leave it reading a
    /// span its group no longer holds.
    pub fn read<T: Send + 'static>(
        &self,
        key: &str,
        until: Instant,
        f: impl FnOnce(&mut E::Reader) -> anyhow::Result<T>,
        on_writer: impl FnOnce(&mut E) -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        self.routed(std::iter::once(position(key)), |_, store| {
            Ok(store.read(Some(key), until, f, on_writer))
        })?
    }

    /// [`Store::read_committed`] in `key`'s group: what is committed there now.
    pub fn read_committed<T: Send + 'static>(
        &self,
        key: &str,
        until: Instant,
        f: impl FnOnce(&mut E::Reader) -> anyhow::Result<T>,
        on_writer: impl FnOnce(&mut E) -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        self.routed(std::iter::once(position(key)), |_, store| {
            Ok(store.read_committed(until, f, on_writer))
        })?
    }

    /// Every group's store on this node, in group order.
    fn stores(&self) -> Vec<Store<E>> {
        let route = self.route.read().unwrap_or_else(PoisonError::into_inner);
        route.groups.values().map(|g| g.store.clone()).collect()
    }

    /// `f` on a reader of every group (or `on_writer` on a group's writer while it has none), once
    /// each group's writes handed over before are committed (`committed`: on what each has
    /// committed now, waiting for none): one answer a group, in group order.
    pub fn read_each<T: Send + 'static>(
        &self,
        until: Instant,
        committed: bool,
        f: impl Fn(&mut E::Reader) -> anyhow::Result<T>,
        on_writer: impl Fn(&mut E) -> anyhow::Result<T> + Clone + Send + 'static,
    ) -> anyhow::Result<Vec<T>> {
        let stores = self.stores();
        let mut answers = Vec::with_capacity(stores.len());
        for store in &stores {
            let w = on_writer.clone();
            let answer = if committed {
                store.read_committed(until, &f, move |e| w(e))?
            } else {
                store.read(None, until, &f, move |e| w(e))?
            };
            answers.push(answer);
        }
        Ok(answers)
    }

    /// Each group's counters, in group order.
    #[must_use]
    pub fn counters(&self) -> Vec<Counters> {
        self.stores().iter().map(Store::counters).collect()
    }
}
