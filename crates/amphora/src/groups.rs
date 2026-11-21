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
//! refused ([`Refused::Away`]), never let write to a group that doesn't hold the key.

use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Instant;

use crate::meter::Counters;
use crate::reshard::directory::{Change, Directory, Proposal};
use crate::reshard::holding::{Away, Holding};
use crate::reshard::{self, GroupId, NodeId, Pos, position};
use crate::store::{Config, Engine, Refused, Store, Ticket};

/// A group: its store and what it holds.
struct Group<E: Engine> {
    store: Store<E>,
    holding: Holding,
}

impl<E: Engine> Clone for Group<E> {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            holding: self.holding,
        }
    }
}

/// The router's directory and the groups on this node. A write checks and hands over its batch
/// under the read lock, and a change of a holding (a fence) takes the write lock: so every batch
/// let in before a fence is in its group's writer's queue before the fence is.
struct Route<E: Engine> {
    directory: Directory,
    groups: BTreeMap<GroupId, Group<E>>,
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
        for change in [begin, Change::Cutover(id), Change::Flip(id), Change::End(id)] {
            directory.apply(&change)?;
        }
        last = made;
    }
    Ok(directory)
}

impl<E: Engine> Groups<E> {
    /// `groups` stores (at least one) over equal runs of positions ([`even`]), the `i`th started
    /// on `engine(i)` with `config`.
    pub fn start(
        groups: usize,
        config: Config,
        mut engine: impl FnMut(usize) -> anyhow::Result<E>,
    ) -> anyhow::Result<Self> {
        let directory = even(NodeId(0), groups)?;
        Self::on(&directory, config, |g| {
            engine(usize::try_from(g.0).map_err(|_| anyhow::anyhow!("group {g:?}"))?)
        })
    }

    /// A store for each group `directory` places, each holding its span, started on
    /// `engine(group)` with `config`.
    pub fn on(
        directory: &Directory,
        config: Config,
        mut engine: impl FnMut(GroupId) -> anyhow::Result<E>,
    ) -> anyhow::Result<Self> {
        directory.check()?;
        let mut groups = BTreeMap::new();
        for (group, _) in directory.groups() {
            let store = Store::start(engine(group)?, config)?;
            let holding = Holding::owning(directory.span_of(group));
            groups.insert(group, Group { store, holding });
        }
        let route = Route {
            directory: directory.clone(),
            groups,
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
        let mut route = self
            .route.write().unwrap_or_else(PoisonError::into_inner);
        if directory.generation() > route.directory.generation() {
            route.directory = directory.clone();
        }
        Ok(())
    }

    /// The generation of the directory routed by.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.route.read().unwrap_or_else(PoisonError::into_inner).directory.generation()
    }

    /// A new group (the driver's `Do::Create`), holding nothing until an operation hands it a
    /// span, its store started on `engine`. Making one that is here already does nothing.
    pub fn create(&self, group: GroupId, engine: impl FnOnce() -> anyhow::Result<E>) -> anyhow::Result<()> {
        let mut route = self.route.write().unwrap_or_else(PoisonError::into_inner);
        if route.groups.contains_key(&group) {
            return Ok(());
        }
        let store = Store::start(engine()?, self.config)?;
        route.groups.insert(
            group,
            Group {
                store,
                holding: Holding::owning(None),
            },
        );
        Ok(())
    }

    /// `f` on `group`'s holding: how the shell carries out a driver's action at the group (its
    /// receive, fence, activate, release, unfence or discard). Under the route's write lock, so
    /// every batch let in before is in the group's writer's queue before what `f` changes.
    pub fn hold<T>(
        &self,
        group: GroupId,
        f: impl FnOnce(&mut Holding) -> Result<T, reshard::Refused>,
    ) -> Result<T, reshard::Refused> {
        let mut route = self
            .route.write().unwrap_or_else(PoisonError::into_inner);
        let held = route.groups.get_mut(&group).ok_or(reshard::Refused::NoGroup(group))?;
        f(&mut held.holding)
    }

    /// How many groups are on this node.
    #[must_use]
    pub fn len(&self) -> usize {
        self.route.read().unwrap_or_else(PoisonError::into_inner).groups.len()
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
