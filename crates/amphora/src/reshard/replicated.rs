//! The directory as a replicated group's state (docs/design/reshard.md, "What is left", 1): a
//! [`hyper_durable::StateMachine`] whose entries are the directory's [`Change`]s. Every member that
//! applies the same committed log holds the same [`Directory`], so routers on any member route by
//! one map, and the group's leader runs the [`Driver`] with its term as the driver's epoch: an
//! answer to a deposed leader's ask never matches the new leader's.
//!
//! The machine keeps nothing durable of its own, as focal's control groups don't: it reports the
//! point before the log's first entry, and the shell replays what the log states committed
//! (hyper-durable's `docs/durable.md` §4.3). Every entry is acted on at start (the driver takes
//! each operation up again from its phase), so each is applied only once the member's durable
//! commit covers it (§4.1, I5). Its images are the whole directory, which the plan's merge rule
//! keeps to at most `8C + 1` groups (`plan`), checked whole before one is installed.

use hyper_durable::{EntryRef, Fatal, Point, StateMachine};
use hyper_raft::proto::{ConfChangeV2, ConfState};

use super::directory::{Change, Directory};
use super::driver::{Driver, Out};
use super::{NodeId, Refused};

/// A change as the directory group's log carries it.
pub fn encode(change: &Change) -> Result<Vec<u8>, Refused> {
    let bytes = postcard::to_allocvec(change)
        .map_err(|_| Refused::Invalid("a directory change that doesn't encode"))?;
    if bytes.len() > crate::replication::COMMAND_BYTES {
        return Err(Refused::Invalid(
            "a directory change larger than a replicated command",
        ));
    }
    Ok(bytes)
}

fn decode(bytes: &[u8]) -> Result<Change, Refused> {
    if bytes.len() > crate::replication::COMMAND_BYTES {
        return Err(Refused::Invalid(
            "a directory change larger than a replicated command",
        ));
    }
    postcard::from_bytes(bytes).map_err(|_| Refused::Invalid("a directory entry that doesn't decode"))
}

/// The directory group's state machine.
#[derive(Debug, Clone)]
pub struct DirectoryMachine {
    directory: Directory,
    applied: Point,
    configuration: ConfState,
    /// The point before the log's first entry: what a restart opens at, replaying the rest.
    start: Point,
}

impl DirectoryMachine {
    /// A directory group's machine at its log's start: one group holding every position on
    /// `node` (AC-T1.9.1).
    #[must_use]
    pub fn new(node: NodeId, configuration: ConfState) -> Self {
        Self {
            directory: Directory::new(node),
            applied: Point::default(),
            configuration,
            start: Point::default(),
        }
    }

    /// The directory as of the last entry applied: what routers on this member route by.
    #[must_use]
    pub fn directory(&self) -> &Directory {
        &self.directory
    }

    /// The last entry applied.
    #[must_use]
    pub fn applied(&self) -> Point {
        self.applied
    }

    /// This member has become the group's leader in `term`: the driver it runs, taking every
    /// operation up again from its phase, and what it does first. Each [`Out::Commit`] is proposed
    /// to the group ([`encode`]), and an action that follows it is carried out only once it has
    /// applied (docs/design/reshard.md, "The directory").
    pub fn lead(&self, term: u64) -> Result<(Driver, Vec<Out>), Refused> {
        Driver::recover(self.directory.clone(), term)
    }
}

impl StateMachine for DirectoryMachine {
    /// Each entry's directory generation after it, or why its change was refused. A refused
    /// change is refused alike on every member (the directory is deterministic) and changes
    /// nothing, so it is the proposer's answer, never a member's failure.
    type Answer = Result<u64, Refused>;

    fn apply(&mut self, entry: &EntryRef<'_>, answers: &mut Vec<Self::Answer>) -> Result<(), Fatal> {
        self.applied = Point {
            index: entry.index,
            term: entry.term,
        };
        // A leader's empty entry on taking office carries no change.
        if entry.data.is_empty() {
            return Ok(());
        }
        let answer = decode(entry.data).and_then(|change| {
            self.directory.apply(&change)?;
            Ok(self.directory.generation())
        });
        answers.push(answer);
        Ok(())
    }

    fn apply_change(
        &mut self,
        at: Point,
        _change: &ConfChangeV2,
        configuration: &ConfState,
    ) -> Result<(), Fatal> {
        self.applied = at;
        self.configuration = configuration.clone();
        Ok(())
    }

    fn durable(&self) -> Point {
        self.start
    }

    fn configuration(&self) -> &ConfState {
        &self.configuration
    }

    fn acts_at_start(&self, _entry: &EntryRef<'_>) -> bool {
        true
    }

    fn image(&mut self, into: &mut Vec<u8>) -> Result<(Point, ConfState), Fatal> {
        into.clear();
        let bytes = postcard::to_allocvec(&self.directory)
            .map_err(|_| Fatal("the directory doesn't encode"))?;
        if bytes.len() > crate::replication::SNAPSHOT_BYTES {
            return Err(Fatal("the directory's image is larger than a snapshot"));
        }
        into.extend_from_slice(&bytes);
        Ok((self.applied, self.configuration.clone()))
    }

    fn image_bytes(&self) -> Option<u64> {
        None
    }

    fn install(&mut self, image: &[u8], at: Point, configuration: &ConfState) -> Result<(), Fatal> {
        if image.len() > crate::replication::SNAPSHOT_BYTES {
            return Err(Fatal("a directory image larger than a snapshot"));
        }
        let directory: Directory =
            postcard::from_bytes(image).map_err(|_| Fatal("a directory image that doesn't decode"))?;
        directory
            .check()
            .map_err(|_| Fatal("a directory image whose spans don't cover every position once"))?;
        self.directory = directory;
        self.applied = at;
        self.start = at;
        self.configuration = configuration.clone();
        Ok(())
    }

    fn persist(&mut self) -> Result<(), Fatal> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};

    use hyper_raft::proto::EntryType;

    use super::*;
    use crate::groups::Groups;
    use crate::reshard::directory::Proposal;
    use crate::reshard::holding::{Away, Fenced};
    use crate::reshard::{GroupId, OpId, Span, position};
    use crate::store::{Config, Engine, Refused as Store};

    /// A group's rows, in memory: what this test's groups write.
    #[derive(Default)]
    struct Mem {
        rows: BTreeMap<String, i64>,
    }

    impl Engine for Mem {
        type Row = (String, i64);
        type Reader = ();

        fn key(row: &Self::Row) -> &str {
            &row.0
        }

        fn apply(&mut self, rows: &[Self::Row]) -> anyhow::Result<usize> {
            for (k, v) in rows {
                self.rows.insert(k.clone(), *v);
            }
            Ok(rows.len())
        }
    }

    /// The directory group's members, applying one committed log in order.
    struct Log {
        members: Vec<DirectoryMachine>,
        index: u64,
    }

    impl Log {
        fn new(n: usize) -> Self {
            let m = DirectoryMachine::new(NodeId(0), ConfState::default());
            Self {
                members: vec![m; n],
                index: 0,
            }
        }

        /// `change` committed at the next index in `term`: what every member answers.
        fn commit(&mut self, term: u64, change: &Change) -> Vec<Result<u64, Refused>> {
            self.index += 1;
            let data = encode(change).unwrap();
            let entry = EntryRef {
                index: self.index,
                term,
                kind: EntryType::EntryNormal,
                context: &[],
                data: &data,
            };
            let mut answers = Vec::new();
            for m in &mut self.members {
                m.apply(&entry, &mut answers).unwrap();
            }
            answers
        }
    }

    fn soon() -> Instant {
        Instant::now() + Duration::from_secs(10)
    }

    /// The first key `k0`, `k1`, ... whose position `want` takes.
    fn key(want: impl Fn(u64) -> bool) -> String {
        (0..)
            .map(|i| format!("k{i}"))
            .find(|k| want(position(k)))
            .unwrap()
    }

    fn value(groups: &Groups<Mem>, key: &str) -> Option<i64> {
        let k = key.to_owned();
        groups
            .read(key, soon(), |()| Ok(None), move |e| Ok(e.rows.get(&k).copied()))
            .unwrap()
    }

    /// T-T1.9.R1 (AC-T1.9.1): do a split through the directory group's log, carrying out its
    /// steps at the groups; expect every member to hold the same directory, writes to the moving
    /// range refused while it is fenced (the pause), a router still on the old directory refused by
    /// the old group's holding after the flip, and once routed by the new directory the range
    /// written to and read from the new group, the rest staying where it was.
    #[test]
    fn t_t1_9_r1_routes_through_the_replicated_directory_as_a_split_moves_a_range() {
        let mut log = Log::new(3);
        let groups = Groups::on(log.members[0].directory(), Config::default(), |_| {
            Ok(Mem::default())
        })
        .unwrap();
        let at = 1u64 << 63;
        let (low, high) = (key(|p| p < at), key(|p| p >= at));
        let t = groups.put(vec![(low.clone(), 1)]).unwrap();
        groups.durable(&t, soon()).unwrap();
        assert_eq!(t.group(), GroupId(0));

        // The leader proposes the split; every member applies it.
        let (mut driver, first) = log.members[0].lead(1).unwrap();
        assert!(first.is_empty());
        let outs = driver
            .begin(&Proposal::Split {
                group: GroupId(0),
                at,
            })
            .unwrap();
        let Some(Out::Commit(begin)) = outs.first() else {
            panic!("a split begins with its commit: {outs:?}");
        };
        let Change::Begin { op, .. } = begin.clone() else {
            panic!("{begin:?}");
        };
        assert_eq!(op.span, Span::new(at, u64::MAX).unwrap());
        assert!(log.commit(1, begin).iter().all(Result::is_ok));

        // A new leader takes the operation up from the replicated directory, under its own term.
        let (taken, _) = log.members[1].lead(2).unwrap();
        assert!(!taken.idle(), "the new leader carries the split on");

        // The new group, taking the (empty) range in.
        groups.create(op.to, || Ok(Mem::default())).unwrap();
        groups
            .hold(op.to, |h| {
                h.receive(op.id, op.span)?;
                h.copied(op.id, None, true, 0)
            })
            .unwrap();

        // Cutover: the old group fences the range. A write to it is refused, typed; the rest
        // goes on.
        assert!(log.commit(2, &Change::Cutover(op.id)).iter().all(Result::is_ok));
        assert_eq!(
            groups.hold(op.from, |h| h.fence(op.id, op.span, 1)).unwrap(),
            Fenced::New(1)
        );
        assert_eq!(
            groups.put(vec![(high.clone(), 2)]).err(),
            Some(Store::Away(Away::Fenced(op.id)))
        );
        assert_eq!(groups.put(vec![(low.clone(), 3)]).unwrap().group(), op.from);

        // The flip: the range is the new group's. A router still on the old directory sends the
        // write to the old group, which refuses it.
        assert!(log.commit(2, &Change::Flip(op.id)).iter().all(Result::is_ok));
        groups.hold(op.to, |h| h.activate(op.id)).unwrap();
        groups.hold(op.from, |h| h.release(op.id)).unwrap();
        assert_eq!(
            groups.put(vec![(high.clone(), 4)]).err(),
            Some(Store::Away(Away::Elsewhere))
        );

        // Routed by the replicated directory: the range goes to the new group.
        groups.route(log.members[2].directory()).unwrap();
        assert_eq!(groups.group(&high), Some(op.to));
        assert_eq!(groups.group(&low), Some(op.from));
        let t = groups.put(vec![(high.clone(), 5)]).unwrap();
        assert_eq!(t.group(), op.to);
        groups.durable(&t, soon()).unwrap();
        assert_eq!(value(&groups, &high), Some(5));
        assert_eq!(value(&groups, &low), Some(3));
        assert!(log.commit(2, &Change::End(op.id)).iter().all(Result::is_ok));

        // Every member holds the same directory, every position in exactly one span.
        let d = log.members[0].directory().clone();
        d.check().unwrap();
        assert_eq!(d.spans().len(), 2);
        assert!(log.members.iter().all(|m| *m.directory() == d));
        // An older directory doesn't replace a newer one.
        let before = groups.generation();
        groups.route(&Directory::new(NodeId(0))).unwrap();
        assert_eq!(groups.generation(), before);
    }

    /// T-T1.9.R2: a change the directory refuses is refused alike on every member and changes
    /// nothing; an image carries the directory whole to a member that lacks the log, and an image
    /// that doesn't decode is refused.
    #[test]
    fn t_t1_9_r2_refused_changes_and_images_leave_members_alike() {
        let mut log = Log::new(2);
        let before = log.members[0].directory().clone();
        let answers = log.commit(1, &Change::Flip(OpId(7)));
        assert_eq!(answers, vec![Err(Refused::NoOp(OpId(7))); 2]);
        assert!(log.members.iter().all(|m| *m.directory() == before));

        let begin = before
            .propose(&Proposal::Split {
                group: GroupId(0),
                at: 1,
            })
            .unwrap();
        assert!(log.commit(1, &begin).iter().all(Result::is_ok));
        let mut image = Vec::new();
        let (at, conf) = log.members[0].image(&mut image).unwrap();
        let mut fresh = DirectoryMachine::new(NodeId(9), ConfState::default());
        fresh.install(&image, at, &conf).unwrap();
        assert_eq!(fresh.directory(), log.members[0].directory());
        assert_eq!(fresh.durable(), at);

        let cut = image.len() / 2;
        assert!(fresh.install(&image[..cut], at, &conf).is_err());
        assert_eq!(fresh.directory(), log.members[0].directory());
    }
}
