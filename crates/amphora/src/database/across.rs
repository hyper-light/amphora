//! A namespace's reads across the groups its rows are spread over (CP-T1.9, lane split-merge-18,
//! T-T1.9.R20; docs/design/reshard.md, "Decision (lane split-merge-18): reads across groups by
//! commit stamp"). Each group counts its own revisions, so a revision orders one group's writes
//! only. What orders them all is each revision's commit stamp: the backup's hybrid logical clock
//! (`backup::Marks`), which the cross-group steps carry from group to group, so a commit made after
//! another, by any path of steps, is stamped after it. A commit writes its revision's stamp beside
//! it (`__amphora_rev_stamps`), and the revision a read across groups answers is that stamp.
//!
//! A read takes its cut first: a stamp from every group (`Marks::next`), the cut the latest of
//! them, which every group then observes, so each commit it stamps from then on is later. Then one
//! call on each group's writer reads as of the cut: the writer runs it after every commit that took
//! its stamp before, so every revision stamped at or before the cut is there, and none made after.
//! The cut is closed in every group and holds every write answered before the read began.
//!
//! A version moved in from another group keeps its revision there, which says nothing here, so it
//! carries its commit stamp with it (`__amphora_moved_stamps`, lane split-merge-19, T-T1.9.R23):
//! its own place in the order across groups, read by stamp rather than by revision.

use std::sync::Arc;
use std::time::Instant;

use amphora_api::{self as api, Change, Changes, Consistency, Page, Refusal, Rev, Scan};

use super::{blob, bytes, integer, rev, text, unavailable};
use crate::backup::Hooks;
use crate::database::Database;
use crate::db::Db;
use crate::xgroup::sql::Member;

/// Writes the commit stamp of revision `at` of namespace `ns`, in the commit that makes it, on a
/// writer that stamps its commits; a revision already stamped (a cross-group step's rows share one)
/// keeps its stamp. An unbacked writer stamps nothing: it has no place in the order across groups.
///
/// The stamp is the cross-group clock's, in microseconds (lane split-merge-21, T-T1.9.R28): the
/// writer's own fresh one, or `pinned`, the commit stamp a transaction across groups chose for all
/// its rows in every group, which the writer's clock then takes in so its later commits are later.
pub(super) fn stamp(db: &Db, ns: &str, at: Rev, pinned: Option<u64>) -> anyhow::Result<()> {
    let Some(hooks) = db.stamps() else {
        return Ok(());
    };
    let mut stamped = false;
    db.query(
        "SELECT 1 FROM __amphora_rev_stamps WHERE namespace = ?1 AND rev = ?2",
        [text(ns), integer(at)?],
        |_| {
            stamped = true;
            Ok(false)
        },
    )?;
    if stamped {
        return Ok(());
    }
    let stamp = match pinned {
        Some(pinned) => {
            hooks.marks.observe_micros(pinned)?;
            pinned
        }
        None => hooks.marks.next_micros()?,
    };
    db.query(
        "INSERT INTO __amphora_rev_stamps(namespace, rev, stamp) VALUES (?1, ?2, ?3)",
        [text(ns), integer(at)?, integer(stamp)?],
        |_| Ok(true),
    )?;
    Ok(())
}

/// Trims the stamps of revisions compaction took through (CP-T1.9, T-T1.9.R21), at most `visits`
/// of them, inside compaction's page-fenced step: whether every stamp it may trim is gone. A stamp
/// is kept while a version of its revision is (a scan answers a row's revision as its stamp), and
/// so is the newest stamp at or before the feed: it is the horizon a refused read is told, and it
/// places a cursor older than it below the feed, so `through` stays exact for every cursor
/// compaction hasn't passed. So the table holds at most one stamp a kept revision, plus one.
///
/// It sweeps in rounds, each through the stamp kept when it began (`__amphora_stamp_trim`): a
/// revision whose versions compaction dropped since the last round, or that moved out, is found by
/// the next. A round walks the revisions kept below the feed, as the version pass walks the
/// versions below its cutoff.
pub(super) fn trim(db: &Db, ns: &str, feed: Rev, visits: usize) -> anyhow::Result<bool> {
    let mut keep = None;
    db.query(
        "SELECT rev FROM __amphora_rev_stamps WHERE namespace = ?1 AND rev <= ?2 ORDER BY rev DESC LIMIT 1",
        [text(ns), integer(feed)?],
        |r| {
            keep = Some(rev(r, 0)?);
            Ok(false)
        },
    )?;
    let Some(keep) = keep else {
        return Ok(true);
    };
    // Lazy DDL in the same committed step, as compaction's own state (`compaction::step`).
    db.exec("CREATE TABLE IF NOT EXISTS __amphora_stamp_trim(namespace TEXT PRIMARY KEY, through INTEGER NOT NULL, swept INTEGER NOT NULL)")?;
    let mut state = None;
    db.query(
        "SELECT through, swept FROM __amphora_stamp_trim WHERE namespace = ?1",
        [text(ns)],
        |r| {
            state = Some((rev(r, 0)?, rev(r, 1)?));
            Ok(false)
        },
    )?;
    let (mut through, mut cursor) = state.unwrap_or((0, 0));
    if cursor >= through {
        if keep <= through {
            return Ok(true);
        }
        (through, cursor) = (keep, 0);
    }
    let mut visited = 0usize;
    while visited < visits {
        let mut next = None;
        db.query(
            "SELECT rev FROM __amphora_rev_stamps WHERE namespace = ?1 AND rev > ?2 AND rev < ?3 ORDER BY rev LIMIT 1",
            [text(ns), integer(cursor)?, integer(through)?],
            |r| {
                next = Some(rev(r, 0)?);
                Ok(false)
            },
        )?;
        let Some(at) = next else {
            cursor = through;
            break;
        };
        if !written_at(db, ns, at)? {
            db.query(
                "DELETE FROM __amphora_rev_stamps WHERE namespace = ?1 AND rev = ?2",
                [text(ns), integer(at)?],
                |_| Ok(true),
            )?;
        }
        cursor = at;
        visited = visited
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("stamp trim count overflow"))?;
    }
    db.query(
        "INSERT INTO __amphora_stamp_trim(namespace, through, swept) VALUES (?1, ?2, ?3) ON CONFLICT(namespace) DO UPDATE SET through = excluded.through, swept = excluded.swept",
        [text(ns), integer(through)?, integer(cursor)?],
        |_| Ok(true),
    )?;
    Ok(cursor >= through && keep <= through)
}

/// Whether a version of revision `at` of `ns` is still here, in any of its families: one seek of
/// each family's feed index.
fn written_at(db: &Db, ns: &str, at: Rev) -> anyhow::Result<bool> {
    let mut family: Option<String> = None;
    loop {
        let mut next = None;
        let found = |r: &amphora_engine::Row| -> anyhow::Result<bool> {
            next = Some(r.get::<String>(0).map_err(unavailable)?);
            Ok(false)
        };
        match &family {
            None => db.query(
                "SELECT family FROM __amphora_families WHERE namespace = ?1 ORDER BY family LIMIT 1",
                [text(ns)],
                found,
            )?,
            Some(after) => db.query(
                "SELECT family FROM __amphora_families WHERE namespace = ?1 AND family > ?2 ORDER BY family LIMIT 1",
                [text(ns), text(after)],
                found,
            )?,
        }
        let Some(name) = next else {
            return Ok(false);
        };
        let mut kept = false;
        db.query(
            "SELECT 1 FROM __amphora_versions WHERE namespace = ?1 AND family = ?2 AND rev = ?3 LIMIT 1",
            [text(ns), text(&name), integer(at)?],
            |_| {
                kept = true;
                Ok(false)
            },
        )?;
        if kept {
            return Ok(true);
        }
        family = Some(name);
    }
}

/// Each group's database and the stamps it makes, in the cluster's order. A group on a node is
/// refused (its endpoint answers only what a node asks, decision "The transport between
/// machines"), as is a group that stamps no commits.
fn stamped(groups: &[Member]) -> Result<Vec<(Database, Arc<Hooks>)>, Refusal> {
    groups
        .iter()
        .enumerate()
        .map(|(i, member)| match member {
            Member::Local(group) => group
                .stamps()
                .map(|hooks| (group.clone(), Arc::clone(hooks)))
                .ok_or_else(|| {
                    Refusal::Unavailable(format!(
                        "group {i} stamps no commits, so its revisions have no place in the order across groups: open it backed up"
                    ))
                }),
            Member::Node { .. } => Err(Refusal::Unavailable(format!(
                "group {i} is on a node: a read across groups reaches only the groups in this process"
            ))),
        })
        .collect()
}

/// Where a read across groups takes its cut (lane split-merge-19, T-T1.9.R22; reshard.md,
/// "Decision (lane split-merge-19): the read point, A/B"). Each takes a fresh stamp from every
/// group, and each cut is closed: every group stamps all it makes from then on after its own fresh
/// stamp, so after the cut. They differ in what a read begun after a write holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ReadPoint {
    /// The latest of the stamps, which every group then observes, pushing its clock past it: a
    /// read holds every write answered before it began. The control.
    #[default]
    Latest,
    /// The earliest of the stamps, observed by none: no group's clock is pushed, but a write a
    /// group whose clock runs ahead of the slowest one answered is missing from the reads begun
    /// after it, until the slowest group's clock passes its stamp.
    Earliest,
}

/// The cut, by `point`, with a trace event of how far apart the groups' clocks read: what the
/// latest point pushes the slow ones by, and how far the earliest point's reads trail.
fn cut(groups: &[(Database, Arc<Hooks>)], point: ReadPoint) -> Result<u64, Refusal> {
    let (mut earliest, mut latest) = (None::<u64>, 0u64);
    for (_, hooks) in groups {
        let stamp = hooks.marks.next_micros().map_err(unavailable)?;
        earliest = Some(earliest.map_or(stamp, |e| e.min(stamp)));
        latest = latest.max(stamp);
    }
    let earliest = earliest.unwrap_or_default();
    tracing::trace!(target: "amphora.records", event = "read_point_cut", point = ?point, groups = groups.len(), spread_us = latest.saturating_sub(earliest), "a read across groups took its cut");
    match point {
        ReadPoint::Latest => {
            for (_, hooks) in groups {
                hooks.marks.observe_micros(latest).map_err(unavailable)?;
            }
            Ok(latest)
        }
        ReadPoint::Earliest => Ok(earliest),
    }
}

/// The newest revision of `ns` here through which every revision is stamped at or before `at`:
/// one below the oldest stamped after it, or the namespace's head when none is. A revision that
/// resolves a transaction across groups carries the stamp its coordinator chose, which can be
/// older than a revision before it (lane split-merge-21, T-T1.9.R28), so the stamps need not rise
/// with the revisions: the oldest by revision bounds the prefix, not the first by stamp.
fn through(db: &Db, ns: &str, at: u64) -> anyhow::Result<Rev> {
    let mut after = None;
    db.query(
        // Revisions count from 1: 0 is none stamped after it.
        "SELECT COALESCE(MIN(rev), 0) FROM __amphora_rev_stamps WHERE namespace = ?1 AND stamp > ?2",
        [text(ns), integer(at)?],
        |r| {
            after = Some(rev(r, 0)?).filter(|&first| first > 0);
            Ok(false)
        },
    )?;
    if let Some(after) = after {
        return Ok(after.saturating_sub(1));
    }
    let mut head = 0;
    db.query(
        "SELECT head FROM __amphora_namespaces WHERE name = ?1",
        [text(ns)],
        |r| {
            head = rev(r, 0)?;
            Ok(false)
        },
    )?;
    Ok(head)
}

/// The stamp of revision `at` of `ns` here; none for a revision made before its group stamped
/// its commits, or moved in from another group.
fn stamp_of(db: &Db, ns: &str, at: Rev) -> anyhow::Result<Option<u64>> {
    let mut stamp = None;
    db.query(
        "SELECT stamp FROM __amphora_rev_stamps WHERE namespace = ?1 AND rev = ?2",
        [text(ns), integer(at)?],
        |r| {
            stamp = Some(rev(r, 0)?);
            Ok(false)
        },
    )?;
    Ok(stamp)
}

/// The stamp of the version of `key` at revision `at`: its own, carried in, for a version moved
/// in from another group (none: its sender carried none), else its revision's here.
pub(super) fn stamp_of_version(
    db: &Db,
    ns: &str,
    family: &str,
    key: &[u8],
    at: Rev,
) -> anyhow::Result<Option<u64>> {
    let mut moved = None;
    db.query(
        "SELECT stamp FROM __amphora_moved_stamps WHERE namespace = ?1 AND family = ?2 AND key = ?3 AND rev = ?4",
        [text(ns), text(family), blob(key)?, integer(at)?],
        |r| {
            moved = Some(match r.get_value(0) {
                amphora_engine::Value::Null => None,
                _ => Some(rev(r, 0)?),
            });
            Ok(false)
        },
    )?;
    match moved {
        Some(stamp) => Ok(stamp),
        None => stamp_of(db, ns, at),
    }
}

/// Notes a version moved in from another group with the commit stamp it carried (none: its
/// sender carried none), so reads across groups place it by that stamp.
pub(super) fn moved_in(
    db: &Db,
    ns: &str,
    family: &str,
    key: &[u8],
    at: Rev,
    stamp: Option<u64>,
) -> anyhow::Result<()> {
    let stamp = match stamp {
        Some(stamp) => integer(stamp)?,
        None => amphora_engine::Value::Null,
    };
    db.query(
        "INSERT OR REPLACE INTO __amphora_moved_stamps(namespace, family, key, rev, stamp) VALUES (?1, ?2, ?3, ?4, ?5)",
        [text(ns), text(family), blob(key)?, integer(at)?, stamp],
        |_| Ok(true),
    )
}

/// Whether a version `w` of `v`'s key is in a read as of stamp `?5` (lane split-merge-20,
/// T-T1.9.R26; lane split-merge-21, T-T1.9.R28): each version is placed by its own stamp, never by
/// its revision. One moved in by the stamp it carried (its revision counts its old group's
/// commits), one made here by its revision's stamp: a revision that resolves a transaction across
/// groups is stamped by the transaction's one commit stamp, which can be older than revisions
/// before it here. A version with neither was made before its group stamped, so before every
/// stamped one: stamp 0. `?4` is unused here; it keeps the parameters' places.
/// A key's versions still rise in stamp as in revision: while a transaction holds a key's lock no
/// other commit writes it, its stamp is past the lock's, which is past every earlier version's, and
/// its group's clock takes it in as it resolves, so the newest of them in the read is its row.
macro_rules! newest_in_the_read {
    () => {
        "(SELECT MAX(w.rev) FROM __amphora_versions w LEFT JOIN __amphora_moved_stamps m ON m.namespace = w.namespace AND m.family = w.family AND m.key = w.key AND m.rev = w.rev LEFT JOIN __amphora_rev_stamps s ON m.rev IS NULL AND s.namespace = w.namespace AND s.rev = w.rev WHERE w.namespace = v.namespace AND w.family = v.family AND w.key = v.key AND COALESCE(m.stamp, s.stamp, 0) <= ?5 AND ?4 >= 0)"
    };
}

/// The refusals a read as of stamp `at` gets here before it reads: compaction took the versions
/// it would answer (`r`, the newest revision stamped at or before it, below the horizon), or a
/// version of `family` moved in without its stamp, which the read can't place.
fn refused_at(db: &Db, ns: &str, family: &str, at: u64, r: Rev) -> anyhow::Result<Option<Refusal>> {
    let mut floor = 0;
    db.query(
        "SELECT horizon FROM __amphora_namespaces WHERE name = ?1",
        [text(ns)],
        |row| {
            floor = rev(row, 0)?;
            Ok(false)
        },
    )?;
    if r < floor {
        let mut horizon = 0;
        db.query(
            "SELECT COALESCE(MAX(stamp), 0) FROM __amphora_rev_stamps WHERE namespace = ?1 AND rev <= ?2",
            [text(ns), integer(floor)?],
            |row| {
                horizon = rev(row, 0)?;
                Ok(false)
            },
        )?;
        return Ok(Some(Refusal::Compacted { asked: at, horizon }));
    }
    unstamped(db, ns, family)
}

/// One group's page of `family` as of stamp `at` (lane split-merge-20, T-T1.9.R26): each key's
/// newest version in the read (`newest_in_the_read`), its revision answered as its stamp. A row
/// moved in after `at` from a group that wrote it before is in the page, as it was then.
fn page_at(
    db: &Db,
    ns: &str,
    family: &str,
    scan: &Scan,
    at: u64,
) -> anyhow::Result<Result<Held<Page>, Refusal>> {
    let r = through(db, ns, at)?;
    if let Some(refusal) = refused_at(db, ns, family, at, r)? {
        return Ok(Err(refusal));
    }
    let upper = super::successor(&scan.prefix);
    // The whole range the scan covers, not only this page's rows: a held put of a row the page
    // doesn't show yet would land in it.
    if crate::xgroup::sql::held_at(db, ns, family, &scan.prefix, upper.as_deref(), at)? {
        return Ok(Ok(Held::Locked));
    }
    // The same four shapes as a group's own scan, the cursor and the upper bound outside the
    // read's OR; a cursor below the prefix is the prefix's own first seek.
    let (lower, continued) = match scan.after.as_ref() {
        Some(after) if after >= &scan.prefix => (after.as_slice(), true),
        _ => (scan.prefix.as_slice(), false),
    };
    let query = match (continued, upper.is_some()) {
        (false, false) => concat!(
            "SELECT v.key, v.rev, v.value FROM __amphora_versions v WHERE v.namespace = ?1 AND v.family = ?2 AND v.key >= ?3 AND v.rev = ",
            newest_in_the_read!(),
            " AND v.value IS NOT NULL ORDER BY v.key LIMIT ?6"
        ),
        (false, true) => concat!(
            "SELECT v.key, v.rev, v.value FROM __amphora_versions v WHERE v.namespace = ?1 AND v.family = ?2 AND v.key >= ?3 AND v.key < ?7 AND v.rev = ",
            newest_in_the_read!(),
            " AND v.value IS NOT NULL ORDER BY v.key LIMIT ?6"
        ),
        (true, false) => concat!(
            "SELECT v.key, v.rev, v.value FROM __amphora_versions v WHERE v.namespace = ?1 AND v.family = ?2 AND v.key > ?3 AND v.rev = ",
            newest_in_the_read!(),
            " AND v.value IS NOT NULL ORDER BY v.key LIMIT ?6"
        ),
        (true, true) => concat!(
            "SELECT v.key, v.rev, v.value FROM __amphora_versions v WHERE v.namespace = ?1 AND v.family = ?2 AND v.key > ?3 AND v.key < ?7 AND v.rev = ",
            newest_in_the_read!(),
            " AND v.value IS NOT NULL ORDER BY v.key LIMIT ?6"
        ),
    };
    let mut params = vec![
        text(ns),
        text(family),
        blob(lower)?,
        integer(r)?,
        integer(at)?,
        integer(
            u64::try_from(scan.limit)?
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("limit overflow"))?,
        )?,
    ];
    if let Some(upper) = upper {
        params.push(blob(&upper)?);
    }
    let mut rows = Vec::new();
    db.query(query, params, |row| {
        rows.push(api::Row {
            key: bytes(row, 0)?.ok_or_else(|| anyhow::anyhow!("null key"))?,
            rev: rev(row, 1)?,
            value: bytes(row, 2)?.ok_or_else(|| anyhow::anyhow!("null value"))?,
        });
        Ok(true)
    })?;
    let more = rows.len() > scan.limit;
    rows.truncate(scan.limit);
    for row in &mut rows {
        row.rev = stamp_of_version(db, ns, family, &row.key, row.rev)?.unwrap_or(0);
    }
    Ok(Ok(Held::Read(Page { rows, more })))
}

/// One key's row here as of stamp `at` (lane split-merge-20, T-T1.9.R26): its newest version in
/// the read (`newest_in_the_read`), as [`page_at`] reads each key; none while that is a delete.
fn row_at(
    db: &Db,
    ns: &str,
    family: &str,
    key: &[u8],
    at: u64,
) -> anyhow::Result<Result<Held<Option<api::Row>>, Refusal>> {
    let r = through(db, ns, at)?;
    if let Some(refusal) = refused_at(db, ns, family, at, r)? {
        return Ok(Err(refusal));
    }
    let mut next = key.to_vec();
    next.push(0);
    if crate::xgroup::sql::held_at(db, ns, family, key, Some(&next), at)? {
        return Ok(Ok(Held::Locked));
    }
    let mut found = None;
    db.query(
        concat!(
            "SELECT v.rev, v.value FROM __amphora_versions v WHERE v.namespace = ?1 AND v.family = ?2 AND v.key = ?3 AND v.rev = ",
            newest_in_the_read!()
        ),
        [text(ns), text(family), blob(key)?, integer(r)?, integer(at)?],
        |row| {
            found = Some((rev(row, 0)?, bytes(row, 1)?));
            Ok(false)
        },
    )?;
    let Some((version, Some(value))) = found else {
        return Ok(Ok(Held::Read(None)));
    };
    let stamp = stamp_of_version(db, ns, family, key, version)?.unwrap_or(0);
    Ok(Ok(Held::Read(Some(api::Row {
        key: key.to_vec(),
        rev: stamp,
        value,
    }))))
}

/// A group's read as of a stamp, or that a transaction across groups held a lock in its way
/// (lane split-merge-21, T-T1.9.R28).
enum Held<T> {
    Read(T),
    /// A write lock whose stamp is at or before the read's: the transaction's commit stamp may be
    /// too, so its rows may belong in the read. The read waits for it to resolve.
    Locked,
}

#[cfg(test)]
impl<T> Held<T> {
    /// The read; a lock in its way fails the test.
    fn read(self) -> T {
        match self {
            Held::Read(answer) => answer,
            Held::Locked => panic!("a lock held the read"),
        }
    }
}

/// `read` of `family`'s changes through `cut`, unless a write lock anywhere in the family holds a
/// stamp at or before it (lane split-merge-21, T-T1.9.R28): the transaction's rows may land at or
/// before the cut, behind a cursor a reader would then have passed.
fn held_whole<T>(
    db: &Db,
    ns: &str,
    family: &str,
    cut: u64,
    read: impl FnOnce(&Db) -> anyhow::Result<Result<T, Refusal>>,
) -> anyhow::Result<Result<Held<T>, Refusal>> {
    if crate::xgroup::sql::held_at(db, ns, family, &[], None, cut)? {
        return Ok(Ok(Held::Locked));
    }
    Ok(read(db)?.map(Held::Read))
}

/// Reads `read` in `group` as of a stamp, waiting out each lock in its way (lane split-merge-21,
/// T-T1.9.R28): no read across groups answers part of a transaction, its rows resolved in one
/// group and not yet in another. It waits on the event, never a timer: a waker registered in the
/// group's publisher, then the read again, so a resolution between the two isn't missed; it reads
/// again only once the group's count of resolutions moved, so the wakes of other steps and reads
/// (each call wakes every watcher) never send it reading. `Timeout` at `until`. A read that waited
/// is counted on the trace (`held_read`).
fn waiting<T, F>(group: &Database, until: Instant, read: F) -> Result<T, Refusal>
where
    T: Send + 'static,
    F: Fn(&Db) -> anyhow::Result<Result<Held<T>, Refusal>> + Send + Sync + 'static,
{
    let read = Arc::new(read);
    let signal = Arc::new(Signal::default());
    let mut registered = None::<Registered>;
    loop {
        let seen = group.resolutions(until)?;
        let try_read = Arc::clone(&read);
        if let Held::Read(answer) = group.call_internal(until, move |db| try_read(&*db))?? {
            return Ok(answer);
        }
        if registered.is_none() {
            let publisher = Arc::clone(&group.resolve(until)?.shared.changed);
            let waker = std::task::Waker::from(Arc::clone(&signal));
            let id = super::locked(&publisher.watchers)?.register(waker)?;
            registered = Some(Registered(vec![(publisher, id)]));
            tracing::trace!(target: "amphora.records", event = "held_read", "a read across groups waits for a lock to resolve");
            continue;
        }
        while group.resolutions(until)? == seen {
            let woken = super::locked(&signal.woken)?;
            let (mut woken, timeout) = signal
                .event
                .wait_timeout_while(
                    woken,
                    until.saturating_duration_since(Instant::now()),
                    |w| !*w,
                )
                .map_err(unavailable)?;
            if !*woken && timeout.timed_out() {
                return Err(Refusal::Timeout);
            }
            *woken = false;
        }
    }
}

/// The namespace's head across groups: the cut, a stamp every group has made all its revisions
/// at or before by the time this answers.
pub(super) fn head(
    groups: &[Member],
    point: ReadPoint,
    _ns: &str,
    until: Instant,
) -> Result<Rev, Refusal> {
    let groups = stamped(groups)?;
    let cut = cut(&groups, point)?;
    // Each writer's commits that took their stamps before the cut end before this call runs.
    for (group, _) in &groups {
        group.call_internal(until, |_| Ok(()))?;
    }
    Ok(cut)
}

/// A scan across groups, as of the cut (Linearizable) or of a stamp a read across groups answered
/// (`Snapshot`): each group's page as of its newest revision stamped at or before it, merged by
/// key. Each row's revision is its stamp (0 for one made before its group stamped its commits).
pub(super) fn scan(
    groups: &[Member],
    point: ReadPoint,
    ns: &str,
    family: &str,
    scan: &Scan,
    until: Instant,
) -> Result<Page, Refusal> {
    let groups = stamped(groups)?;
    let cut = cut(&groups, point)?;
    let at = match scan.at {
        Consistency::Linearizable => cut,
        Consistency::Snapshot(asked) if asked > cut => {
            return Err(Refusal::Ahead { asked, head: cut });
        }
        Consistency::Snapshot(asked) => asked,
    };
    let mut rows = Vec::new();
    let mut more = false;
    for (group, _) in &groups {
        let (ns, family, scan) = (ns.to_owned(), family.to_owned(), scan.clone());
        let page = waiting(group, until, move |db| page_at(db, &ns, &family, &scan, at))?;
        more |= page.more;
        rows.extend(page.rows);
    }
    // Each group's keys are its own, and each page its smallest after the cursor, so the union's
    // first `limit` keys are all among them.
    rows.sort_by(|a, b| a.key.cmp(&b.key));
    more |= rows.len() > scan.limit;
    rows.truncate(scan.limit);
    Ok(Page { rows, more })
}

/// One group's changes after `after` through `cut`, by stamp, at most `limit` and never cutting a
/// stamp; whether it had more.
fn changes_in(
    db: &Db,
    ns: &str,
    family: &str,
    after: u64,
    cut: u64,
    limit: usize,
) -> anyhow::Result<Result<(Vec<Change>, bool), Refusal>> {
    let from = through(db, ns, after)?;
    if let Some(refusal) = compacted(db, ns, after, from)? {
        return Ok(Err(refusal));
    }
    let take = integer(
        u64::try_from(limit)?
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("limit overflow"))?,
    )?;
    // This group's own versions, by their revisions' stamps, not by revision (lane
    // split-merge-21, T-T1.9.R28): a revision resolving a transaction across groups carries its
    // coordinator's stamp, which can be older than revisions before it here. One made before the
    // group stamped has none, and is before every stamped one, as `through` places it. A version
    // moved in is placed by its own stamp, below.
    let mut found: Vec<(u64, u64, Change)> = Vec::new();
    db.query(
        "SELECT s.stamp, v.key, v.value, v.seq FROM __amphora_versions v JOIN __amphora_rev_stamps s ON s.namespace = v.namespace AND s.rev = v.rev LEFT JOIN __amphora_moved_stamps m ON m.namespace = v.namespace AND m.family = v.family AND m.key = v.key AND m.rev = v.rev WHERE v.namespace = ?1 AND v.family = ?2 AND s.stamp > ?3 AND s.stamp <= ?4 AND m.rev IS NULL ORDER BY s.stamp, v.seq LIMIT ?5",
        [
            text(ns),
            text(family),
            integer(after)?,
            integer(cut)?,
            take.clone(),
        ],
        |r| {
            let stamp = rev(r, 0)?;
            found.push((
                stamp,
                rev(r, 3)?,
                Change {
                    rev: stamp,
                    key: bytes(r, 1)?.ok_or_else(|| anyhow::anyhow!("null key"))?,
                    value: bytes(r, 2)?,
                },
            ));
            Ok(true)
        },
    )?;
    let own_more = found.len() > limit;
    let last = found.last().map(|(stamp, _, _)| *stamp);
    // The versions moved in, by the stamps they carried; one whose sender carried none has no
    // place in the order, so the read is refused rather than answered without it.
    if let Some(refusal) = unstamped(db, ns, family)? {
        return Ok(Err(refusal));
    }
    let mut moved: Vec<(Vec<u8>, Rev, u64)> = Vec::new();
    db.query(
        "SELECT key, rev, stamp FROM __amphora_moved_stamps WHERE namespace = ?1 AND family = ?2 AND stamp > ?3 AND stamp <= ?4 ORDER BY stamp, key LIMIT ?5",
        [text(ns), text(family), integer(after)?, integer(cut)?, take],
        |r| {
            moved.push((
                bytes(r, 0)?.ok_or_else(|| anyhow::anyhow!("null key"))?,
                rev(r, 1)?,
                rev(r, 2)?,
            ));
            Ok(true)
        },
    )?;
    let moved_more = moved.len() > limit;
    let mut moved_last = None;
    for (key, at, stamp) in moved {
        let mut version = None;
        db.query(
            "SELECT seq, value FROM __amphora_versions WHERE namespace = ?1 AND family = ?2 AND key = ?3 AND rev = ?4",
            [text(ns), text(family), blob(&key)?, integer(at)?],
            |r| {
                version = Some((rev(r, 0)?, bytes(r, 1)?));
                Ok(false)
            },
        )?;
        moved_last = Some(stamp);
        if let Some((seq, value)) = version {
            found.push((
                stamp,
                seq,
                Change {
                    rev: stamp,
                    key,
                    value,
                },
            ));
        }
    }
    // Each side that had more bounds the answer at its last stamp read, so neither's next is
    // passed.
    let mut bound = cut;
    if own_more && let Some(stamp) = last {
        bound = bound.min(stamp);
    }
    if moved_more && let Some(stamp) = moved_last {
        bound = bound.min(stamp);
    }
    found.retain(|(stamp, _, _)| *stamp <= bound);
    found.sort_by_key(|(stamp, seq, _)| (*stamp, *seq));
    let more = own_more || moved_more || found.len() > limit;
    let found: Vec<Change> = found.into_iter().map(|(_, _, change)| change).collect();
    Ok(whole(found, limit).map(|changes| (changes, more)))
}

/// The refusal a read of the changes after stamp `after` gets once compaction took them: `from`,
/// the newest revision here stamped at or before it, is below the feed.
fn compacted(db: &Db, ns: &str, after: u64, from: Rev) -> anyhow::Result<Option<Refusal>> {
    let mut feed = 0;
    db.query(
        "SELECT feed FROM __amphora_namespaces WHERE name = ?1",
        [text(ns)],
        |r| {
            feed = rev(r, 0)?;
            Ok(false)
        },
    )?;
    if from >= feed {
        return Ok(None);
    }
    // The newest stamp compaction may have taken changes through.
    let mut horizon = 0;
    db.query(
        "SELECT COALESCE(MAX(stamp), 0) FROM __amphora_rev_stamps WHERE namespace = ?1 AND rev <= ?2",
        [text(ns), integer(feed)?],
        |r| {
            horizon = rev(r, 0)?;
            Ok(false)
        },
    )?;
    Ok(Some(Refusal::Compacted {
        asked: after,
        horizon,
    }))
}

/// The refusal a read of `family`'s changes gets while a version moved in without its commit
/// stamp is here: it has no place in the order, so the read is refused rather than answered
/// without it.
fn unstamped(db: &Db, ns: &str, family: &str) -> anyhow::Result<Option<Refusal>> {
    let mut found = None;
    db.query(
        "SELECT key, rev FROM __amphora_moved_stamps WHERE namespace = ?1 AND family = ?2 AND stamp IS NULL LIMIT 1",
        [text(ns), text(family)],
        |r| {
            found = Some(rev(r, 1)?);
            Ok(false)
        },
    )?;
    Ok(found.map(|at| {
        Refusal::Unavailable(format!(
            "a version at revision {at} moved in from another group without its commit stamp: the order across groups doesn't place it"
        ))
    }))
}

/// The stamp of `family`'s newest change here after stamp `after` through `cut`, its own or moved
/// in; none while there is none.
fn newest_in(
    db: &Db,
    ns: &str,
    family: &str,
    after: u64,
    cut: u64,
) -> anyhow::Result<Result<Option<u64>, Refusal>> {
    let from = through(db, ns, after)?;
    if let Some(refusal) = compacted(db, ns, after, from)? {
        return Ok(Err(refusal));
    }
    if let Some(refusal) = unstamped(db, ns, family)? {
        return Ok(Err(refusal));
    }
    // The newest own version's stamp in the range, by stamp, as `changes_in` places them; 0 is
    // none (every stamp in the range is past `after`).
    let mut own = None;
    db.query(
        "SELECT COALESCE(MAX(s.stamp), 0) FROM __amphora_versions v JOIN __amphora_rev_stamps s ON s.namespace = v.namespace AND s.rev = v.rev LEFT JOIN __amphora_moved_stamps m ON m.namespace = v.namespace AND m.family = v.family AND m.key = v.key AND m.rev = v.rev WHERE v.namespace = ?1 AND v.family = ?2 AND s.stamp > ?3 AND s.stamp <= ?4 AND m.rev IS NULL",
        [text(ns), text(family), integer(after)?, integer(cut)?],
        |r| {
            own = Some(rev(r, 0)?).filter(|&stamp| stamp > 0);
            Ok(false)
        },
    )?;
    let mut moved = None;
    db.query(
        "SELECT stamp FROM __amphora_moved_stamps WHERE namespace = ?1 AND family = ?2 AND stamp > ?3 AND stamp <= ?4 ORDER BY stamp DESC LIMIT 1",
        [text(ns), text(family), integer(after)?, integer(cut)?],
        |r| {
            moved = Some(rev(r, 0)?);
            Ok(false)
        },
    )?;
    Ok(Ok(own.max(moved)))
}

/// `wait`'s answer now across groups (`Records::changed`): the stamp of `family`'s newest change
/// in any group after stamp `after`, through the cut, or none while there is none.
pub(super) fn changed(
    groups: &[Member],
    point: ReadPoint,
    ns: &str,
    family: &str,
    after: Rev,
    until: Instant,
) -> Result<Option<Rev>, Refusal> {
    let groups = stamped(groups)?;
    let cut = cut(&groups, point)?;
    if after >= cut {
        return Ok(None);
    }
    let mut newest = None;
    for (group, _) in &groups {
        let (ns, family) = (ns.to_owned(), family.to_owned());
        let found = waiting(group, until, move |db| {
            held_whole(db, &ns, &family, cut, |db| {
                newest_in(db, &ns, &family, after, cut)
            })
        })?;
        newest = newest.max(found);
    }
    Ok(newest)
}

/// What wakes a wait across groups: every group's publisher wakes it as it publishes a change
/// (`Changed::watchers`), setting the flag the parked waiter waits on.
#[derive(Default)]
struct Signal {
    woken: std::sync::Mutex<bool>,
    event: std::sync::Condvar,
}

impl std::task::Wake for Signal {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        // A poisoned flag wakes nobody; the waiter's deadline still ends its wait.
        if let Ok(mut woken) = self.woken.lock() {
            *woken = true;
            self.event.notify_all();
        }
    }
}

/// A wait's registration in each group's publisher, removed as the wait ends, however it ends.
struct Registered(Vec<(Arc<super::Changed>, u64)>);

impl Drop for Registered {
    fn drop(&mut self) {
        for (changed, id) in &self.0 {
            if let Ok(mut watchers) = changed.watchers.lock() {
                watchers.wakers.remove(id);
            }
        }
    }
}

/// `family`'s newest revision each group has published in `ns`. A wait compares it after each
/// wake: one that published nothing new for the family (a read's own call wakes every watcher)
/// parks it again without reading.
fn published(registered: &Registered, ns: &str, family: &str) -> Result<Vec<Rev>, Refusal> {
    registered
        .0
        .iter()
        .map(|(changed, _)| {
            Ok(super::locked(&changed.published)?
                .get(ns)
                .and_then(|p| p.families.get(family))
                .copied()
                .unwrap_or(0))
        })
        .collect()
}

/// `Records::wait` across groups (lane split-merge-19, T-T1.9.R25): the stamp of `family`'s newest
/// change after stamp `after` in any group, once there is one, or `Timeout` at `until`. It wakes on
/// the event, never a timer: a waker registered in every group's publisher before the first check,
/// so a change published after it wakes the wait, which reads again only once a group published
/// something new for the family.
pub(super) fn wait(
    groups: &[Member],
    point: ReadPoint,
    ns: &str,
    family: &str,
    after: Rev,
    until: Instant,
) -> Result<Rev, Refusal> {
    let signal = Arc::new(Signal::default());
    let waker = std::task::Waker::from(Arc::clone(&signal));
    let mut registered = Registered(Vec::new());
    for member in groups {
        // A group on a node is refused by the check, as every read across groups refuses it.
        if let Member::Local(group) = member {
            let publisher = Arc::clone(&group.resolve(until)?.shared.changed);
            let id = super::locked(&publisher.watchers)?.register(waker.clone())?;
            registered.0.push((publisher, id));
        }
    }
    loop {
        // Read before the check: a change published after it moves what is read, and wakes.
        let seen = published(&registered, ns, family)?;
        if let Some(at) = changed(groups, point, ns, family, after, until)? {
            return Ok(at);
        }
        loop {
            let woken = super::locked(&signal.woken)?;
            let (mut woken, timeout) = signal
                .event
                .wait_timeout_while(
                    woken,
                    until.saturating_duration_since(Instant::now()),
                    |w| !*w,
                )
                .map_err(unavailable)?;
            if !*woken && timeout.timed_out() {
                return Err(Refusal::Timeout);
            }
            *woken = false;
            drop(woken);
            if published(&registered, ns, family)? != seen {
                break;
            }
        }
    }
}

/// One key's row across groups, read in the group its row is in (`group`, by the cluster's
/// route): as of that group's fresh stamp (Linearizable), or of a stamp asked, which the group
/// first observes, so none of its later commits is stamped at or before it. The row's revision is
/// its stamp. One group is read, and no other group's clock is pushed, whatever the read point.
pub(super) fn get(
    groups: &[Member],
    group: crate::xgroup::GroupId,
    ns: &str,
    family: &str,
    key: &[u8],
    at: Consistency,
    until: Instant,
) -> Result<Option<api::Row>, Refusal> {
    let groups = stamped(groups)?;
    let Some((database, hooks)) = usize::try_from(group).ok().and_then(|g| groups.get(g)) else {
        return Err(Refusal::Unavailable(format!(
            "the route names group {group}, which the cluster doesn't have"
        )));
    };
    let at = match at {
        Consistency::Linearizable => hooks.marks.next_micros().map_err(unavailable)?,
        Consistency::Snapshot(asked) => {
            hooks.marks.observe_micros(asked).map_err(unavailable)?;
            asked
        }
    };
    let (ns, family, key) = (ns.to_owned(), family.to_owned(), key.to_vec());
    waiting(database, until, move |db| {
        row_at(db, &ns, &family, &key, at)
    })
}

/// `changes` cut to `limit`, never through a stamp: a cursor is a stamp, so advancing to the last
/// change of a cut one would skip the rest of its keys.
fn whole(mut changes: Vec<Change>, limit: usize) -> Result<Vec<Change>, Refusal> {
    if changes.len() <= limit {
        return Ok(changes);
    }
    let next = changes.get(limit).map(|c| c.rev);
    changes.truncate(limit);
    if changes.last().map(|c| c.rev) == next {
        changes.retain(|c| Some(c.rev) != next);
        if changes.is_empty() {
            return Err(api::invalid(
                "change limit",
                "a transaction does not fit; request up to TX_MAX changes",
            ));
        }
    }
    Ok(changes)
}

/// The changes across groups after stamp `after`, in stamp order (a stamp's rows in group order,
/// each group's in its commit's order), at most `limit`; the head is the cut. A group that had more
/// than `limit` bounds the answer at its last stamp returned, so no group's later change is passed.
pub(super) fn changes(
    groups: &[Member],
    point: ReadPoint,
    ns: &str,
    family: &str,
    after: Rev,
    limit: usize,
    until: Instant,
) -> Result<Changes, Refusal> {
    let groups = stamped(groups)?;
    let cut = cut(&groups, point)?;
    if after >= cut {
        return Ok(Changes {
            changes: Vec::new(),
            head: cut,
        });
    }
    let mut all = Vec::new();
    let mut bound = cut;
    for (group, _) in &groups {
        let (ns, family) = (ns.to_owned(), family.to_owned());
        let (found, more) = waiting(group, until, move |db| {
            held_whole(db, &ns, &family, cut, |db| {
                changes_in(db, &ns, &family, after, cut, limit)
            })
        })?;
        if more && let Some(last) = found.last() {
            bound = bound.min(last.rev);
        }
        all.extend(found);
    }
    all.retain(|change| change.rev <= bound);
    all.sort_by_key(|change| change.rev);
    Ok(Changes {
        changes: whole(all, limit)?,
        head: cut,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use amphora_api::Records;
    use amphora_api::blobs::Blobs;

    use super::*;
    use crate::backup::{Archive, CheckPolicy, FolderBlobs, MICRO_NS, SECOND_NS};
    use crate::store::Moved;
    use crate::xgroup::sql::record_key;
    use crate::xgroup::{Answer, Cluster, Protocol, Txn, Why};

    fn until() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    /// Each stamped revision of `ns` here, and each revision a version of it is kept at.
    fn stamps_and_versions(db: &Database, ns: &'static str) -> (Vec<Rev>, Vec<Rev>) {
        db.call_internal(until(), move |db| {
            let (mut stamps, mut versions) = (Vec::new(), Vec::new());
            db.query(
                "SELECT rev FROM __amphora_rev_stamps WHERE namespace = ?1 ORDER BY rev",
                [text(ns)],
                |r| {
                    stamps.push(rev(r, 0)?);
                    Ok(true)
                },
            )?;
            db.query(
                "SELECT DISTINCT rev FROM __amphora_versions WHERE namespace = ?1 ORDER BY rev",
                [text(ns)],
                |r| {
                    versions.push(rev(r, 0)?);
                    Ok(true)
                },
            )?;
            Ok((stamps, versions))
        })
        .unwrap()
    }

    /// T-T1.9.R21 (CP-T1.9; reshard.md, "Decision (lane split-merge-19): compaction trims the
    /// commit stamps"). Do: one backed-up database; 600 commits, each overwriting the next of 8
    /// keys, compacting to the head after every 50; after each compaction, the stamps and the
    /// revisions whose versions are kept. Expect: every kept revision still stamped (a scan across
    /// groups answers a row's revision as its stamp), and at most one stamp a kept revision plus
    /// one, the newest at or before the feed, however long the run: 8 or 9, never the commits so
    /// far.
    #[test]
    fn t_t1_9_r21_compaction_keeps_the_stamp_table_bounded() {
        const KEYS: u64 = 8;
        let dir = tempfile::tempdir().unwrap();
        let blobs = Arc::new(FolderBlobs::open(&dir.path().join("archive"), 64 * 1024).unwrap());
        let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS + 1));
        let archive = Archive {
            blobs: Arc::clone(&blobs) as Arc<dyn Blobs>,
            prefix: "g0".into(),
            clock: Arc::new(move || now.fetch_add(1, Ordering::SeqCst)),
            memory: 32 * 1024,
            patience: Duration::from_secs(60),
            check: CheckPolicy::Off,
        };
        let (db, _sink) =
            Database::open_backed_up(&dir.path().join("g0.db"), archive, until()).unwrap();
        let ns = db.namespace("n").unwrap();
        // A longer run for the coordinator's: AMPHORA_R21_COMMITS (600 by default).
        let commits: u64 = std::env::var("AMPHORA_R21_COMMITS").map_or(600, |n| n.parse().unwrap());
        for i in 0..commits {
            let head = ns
                .commit(
                    api::Tx {
                        conds: vec![],
                        writes: vec![api::Write::Put {
                            family: "f".into(),
                            key: (i % KEYS).to_be_bytes().to_vec(),
                            value: i.to_be_bytes().to_vec(),
                        }],
                    },
                    until(),
                )
                .unwrap();
            if (i + 1) % 50 != 0 {
                continue;
            }
            ns.compact(head, until()).unwrap();
            let (stamps, versions) = stamps_and_versions(&db, "n");
            assert!(
                versions.iter().all(|at| stamps.binary_search(at).is_ok()),
                "a kept revision lost its stamp: {versions:?} {stamps:?}"
            );
            let bound = versions.len().checked_add(1).unwrap();
            assert!(
                stamps.len() <= bound,
                "after commit {i}: {} stamps for {} kept revisions",
                stamps.len(),
                versions.len()
            );
            assert!(
                u64::try_from(versions.len()).unwrap() <= KEYS,
                "{versions:?}"
            );
        }
    }

    /// `groups` backed-up groups in `dir`, each read by `clock(group)`, under one coordinator
    /// routing by `route`.
    fn groups_on(
        dir: &std::path::Path,
        groups: u64,
        clock: impl Fn(u64) -> crate::backup::Clock,
        route: crate::xgroup::Route,
    ) -> (Vec<Database>, Vec<crate::backup::Shipping>, Arc<Cluster>) {
        let blobs = Arc::new(FolderBlobs::open(&dir.join("archive"), 64 * 1024).unwrap());
        let (mut dbs, mut sinks) = (Vec::new(), Vec::new());
        for group in 0..groups {
            let archive = Archive {
                blobs: Arc::clone(&blobs) as Arc<dyn Blobs>,
                prefix: format!("g{group}"),
                clock: clock(group),
                memory: 32 * 1024,
                patience: Duration::from_secs(60),
                check: CheckPolicy::Off,
            };
            let (db, sink) =
                Database::open_backed_up(&dir.join(format!("g{group}.db")), archive, until())
                    .unwrap();
            dbs.push(db);
            sinks.push(sink);
        }
        let cluster =
            Arc::new(Cluster::open(dbs.clone(), route, 5, Protocol::TwoPhase, 1, until()).unwrap());
        (dbs, sinks, cluster)
    }

    /// One row `key` written into `group`'s own Records, as a single-group commit.
    fn put(group: &Database, key: Vec<u8>) {
        group
            .namespace("n")
            .unwrap()
            .commit(
                api::Tx {
                    conds: vec![],
                    writes: vec![api::Write::Put {
                        family: "f".into(),
                        key,
                        value: vec![1],
                    }],
                },
                until(),
            )
            .unwrap();
    }

    /// T-T1.9.R22 (CP-T1.9; reshard.md, "Decision (lane split-merge-19): the read point, A/B").
    /// Do: two backed-up groups, the second's clock 500 seconds behind the first's, the one clock
    /// moving a microsecond (the cross-group stamps' unit) at each read; a row written in the
    /// first; the changes across them read at the earliest point, then at the latest; then a row
    /// written in each group, and read again from each read's head. Expect: the earliest misses
    /// the row (the slow group's stamp is the cut: its own clock, or the first's as the
    /// coordinator's opening steps carried it in, the hybrid logical clock's receive rule, either
    /// way before the row's) and its head is behind the row's stamp; the latest holds it; after
    /// its head, the latest point finds both later rows (its cut is closed); the earliest never
    /// answers a change at or before a head it answered, nor a head behind one.
    ///
    /// The clock moves a microsecond a read, not a nanosecond (lane split-merge-25): stamps are
    /// microseconds since T-T1.9.R28, and a clock that moves less than one between reads leaves
    /// every stamp to the logical count, so the slow group, carried to the first's stamp when the
    /// coordinator opened, counts up to the row's stamp on its own and the earliest cut rightly
    /// holds the row ("until the slowest group's clock passes its stamp", [`ReadPoint::Earliest`]).
    #[test]
    fn t_t1_9_r22_the_read_points_a_write_answered_before_a_read() {
        let dir = tempfile::tempdir().unwrap();
        let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS + 1));
        // Rows are written into each group directly, so the route is never asked.
        let (dbs, _sinks, cluster) = groups_on(
            dir.path(),
            2,
            |group| {
                let read = Arc::clone(&now);
                let behind = group * 500 * SECOND_NS;
                Arc::new(move || read.fetch_add(MICRO_NS, Ordering::SeqCst) - behind)
            },
            Arc::new(|_: &[u8]| 0),
        );
        let latest = dbs[0]
            .across_at(Arc::clone(&cluster), ReadPoint::Latest)
            .namespace("n")
            .unwrap();
        let earliest = dbs[0]
            .across_at(Arc::clone(&cluster), ReadPoint::Earliest)
            .namespace("n")
            .unwrap();
        put(&dbs[0], vec![0, b'a']);

        // The earliest point first: the latest's read pushes the slow group's clock past the row.
        let stale = earliest.changes("f", 0, 100, until()).unwrap();
        assert!(stale.changes.is_empty(), "{stale:?}");
        let held = latest.changes("f", 0, 100, until()).unwrap();
        assert_eq!(held.changes.len(), 1, "{held:?}");
        assert!(stale.head < held.changes[0].rev, "{stale:?} {held:?}");

        put(&dbs[0], vec![0, b'b']);
        put(&dbs[1], vec![1, b'b']);
        let after = latest.changes("f", held.head, 100, until()).unwrap();
        assert_eq!(after.changes.len(), 2, "{after:?}");
        let again = earliest.changes("f", stale.head, 100, until()).unwrap();
        assert!(
            again.changes.iter().all(|c| c.rev > stale.head),
            "{again:?}"
        );
        assert!(again.head >= stale.head, "{again:?} {stale:?}");
    }

    /// The A/B lab of the read point (T-T1.9.R22; reshard.md, "Decision (lane split-merge-19):
    /// the read point, A/B"), each candidate's failure mode forced: many groups, for the latest
    /// point's clock push, and one group whose clock lags the rest by `AMPHORA_AB_LAG_NS`, for the
    /// earliest point's staleness. Every group's clock is the machine's, the lagging one's behind
    /// by the lag. Each round writes a row in every group, then reads the changes across them
    /// from the last cursor, timed, as the cut alone is. It prints, a line a candidate and count
    /// of groups: the read's and the cut's latency p50, p99 and p99.9, and the staleness: the rows
    /// answered before the read began that it missed, at the median and worst, and how far the cut
    /// trailed the newest stamp. No assertion on time: this measures; the run of record is the
    /// coordinator's (the command is in the decision record).
    #[test]
    #[ignore = "lab: AMPHORA_AB_GROUPS, AMPHORA_AB_ROUNDS, AMPHORA_AB_LAG_NS"]
    fn lab_read_point_a_b() {
        let env = |name: &str| std::env::var(name).ok();
        let counts: Vec<u64> = env("AMPHORA_AB_GROUPS")
            .unwrap_or_else(|| "2,16,128".into())
            .split(',')
            .map(|n| n.trim().parse().unwrap())
            .collect();
        let rounds: usize = env("AMPHORA_AB_ROUNDS").map_or(200, |n| n.parse().unwrap());
        let lag: u64 = env("AMPHORA_AB_LAG_NS").map_or(SECOND_NS, |n| n.parse().unwrap());
        let wall = || {
            u64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            )
            .unwrap()
        };
        // The per-mille'th smallest: the nearest-rank percentile.
        let pct = |sorted: &[u128], per_mille: usize| -> u128 {
            let at = (sorted.len() * per_mille).div_ceil(1000);
            sorted[at.clamp(1, sorted.len()) - 1]
        };
        println!(
            "point groups rounds lag_ns | read_us p50 p99 p99.9 | cut_us p50 p99 p99.9 | missed p50 max | trail_us p50 max"
        );
        for &groups in &counts {
            for point in [ReadPoint::Latest, ReadPoint::Earliest] {
                let dir = tempfile::tempdir().unwrap();
                // Rows are written into each group directly, so the route is never asked.
                let (dbs, _sinks, cluster) = groups_on(
                    dir.path(),
                    groups,
                    |group| {
                        let behind = if group == 0 { lag } else { 0 };
                        Arc::new(move || wall() - behind)
                    },
                    Arc::new(|_: &[u8]| 0),
                );
                let across = dbs[0]
                    .across_at(Arc::clone(&cluster), point)
                    .namespace("n")
                    .unwrap();
                let hooks = stamped(cluster.groups()).unwrap();
                let (mut reads, mut cuts, mut missed, mut trail) =
                    (Vec::new(), Vec::new(), Vec::new(), Vec::new());
                let (mut cursor, mut written, mut seen) = (0, 0usize, 0usize);
                for round in 0..rounds {
                    for (g, db) in dbs.iter().enumerate() {
                        let mut key = vec![u8::try_from(g).unwrap()];
                        key.extend_from_slice(&round.to_be_bytes());
                        put(db, key);
                        written += 1;
                    }
                    let newest = hooks
                        .iter()
                        .map(|(_, h)| h.marks.latest_micros().unwrap())
                        .max()
                        .unwrap_or(0);
                    let began = Instant::now();
                    let page = across.changes("f", cursor, api::TX_MAX, until()).unwrap();
                    reads.push(began.elapsed().as_micros());
                    seen += page.changes.len();
                    missed.push(written - seen);
                    trail.push(newest.saturating_sub(page.head));
                    if let Some(last) = page.changes.last() {
                        cursor = last.rev;
                    }
                    let began = Instant::now();
                    cut(&hooks, point).unwrap();
                    cuts.push(began.elapsed().as_micros());
                }
                reads.sort_unstable();
                cuts.sort_unstable();
                missed.sort_unstable();
                trail.sort_unstable();
                println!(
                    "{point:?} {groups} {rounds} {lag} | {} {} {} | {} {} {} | {} {} | {} {}",
                    pct(&reads, 500),
                    pct(&reads, 990),
                    pct(&reads, 999),
                    pct(&cuts, 500),
                    pct(&cuts, 990),
                    pct(&cuts, 999),
                    missed[missed.len() / 2],
                    missed.last().unwrap(),
                    trail[trail.len() / 2],
                    trail.last().unwrap(),
                );
            }
        }
    }

    /// T-T1.9.R23 (CP-T1.9; reshard.md, "Decision (lane split-merge-19): a moved row carries its
    /// commit stamp"). Do: two databases made groups', each with three commits of its own at
    /// revisions 1 to 3, stamped as a backed-up writer would (these are unbacked, so the test
    /// writes the stamps): the first's at 10, 30 and 50, the second's at 20, 40 and 60; the first's
    /// keys exported and imported into the second; then the second's changes read by stamp, from
    /// the start, after 30, and two at a time; and the same keys imported into a third database
    /// as a sender that carries no stamps would send them. Expect: each moved row answers its own
    /// stamp, not that of the second's own revision of the same number; the changes interleave by
    /// stamp (a1 b1 a2 b2 a3 b3), after 30 are b2 a3 b3, two at a time a1 b1 with more; and the
    /// third's changes refused, naming the stamp missing, rather than answered without the rows.
    #[test]
    fn t_t1_9_r23_rows_moved_in_carry_their_commit_stamps() {
        let dir = tempfile::tempdir().unwrap();
        let open = |name: &str| {
            let db = Db::open(&dir.path().join(name)).unwrap();
            crate::database::bootstrap_db(&db).unwrap();
            db
        };
        let commit = |db: &Db, key: &[u8], stamp: u64| {
            let at = crate::database::write_across(
                db,
                "n",
                "f",
                key,
                Some(b"v".as_slice()),
                None,
                0,
                None,
            )
            .unwrap();
            db.query(
                "INSERT INTO __amphora_rev_stamps(namespace, rev, stamp) VALUES ('n', ?1, ?2)",
                [integer(at).unwrap(), integer(stamp).unwrap()],
                |_| Ok(true),
            )
            .unwrap();
        };
        let (old, mut new, mut plain) = (open("old.db"), open("new.db"), open("plain.db"));
        for (i, key) in [b"a1", b"a2", b"a3"].into_iter().enumerate() {
            commit(&old, key.as_slice(), 10 + 20 * u64::try_from(i).unwrap());
        }
        for (i, key) in [b"b1", b"b2", b"b3"].into_iter().enumerate() {
            commit(&new, key.as_slice(), 20 + 20 * u64::try_from(i).unwrap());
        }
        crate::database::group::begin(&old).unwrap();
        crate::database::group::begin(&new).unwrap();
        crate::database::group::begin(&plain).unwrap();
        let batch = crate::database::group::export(&old, crate::reshard::Span::ALL, 100).unwrap();
        assert_eq!(batch.keys.len(), 3, "{batch:?}");
        new.transaction(|db| crate::database::group::import(db, &batch.keys))
            .unwrap();

        assert_eq!(
            stamp_of_version(&new, "n", "f", b"a1", 1).unwrap(),
            Some(10)
        );
        assert_eq!(
            stamp_of_version(&new, "n", "f", b"b1", 1).unwrap(),
            Some(20)
        );
        let read = |after, limit| {
            changes_in(&new, "n", "f", after, 1_000, limit)
                .unwrap()
                .unwrap()
        };
        let keys = |changes: &[Change]| -> Vec<Vec<u8>> {
            changes.iter().map(|c| c.key.clone()).collect()
        };
        let (all, more) = read(0, 100);
        let want: Vec<Vec<u8>> = ["a1", "b1", "a2", "b2", "a3", "b3"]
            .iter()
            .map(|k| k.as_bytes().to_vec())
            .collect();
        assert_eq!((keys(&all), more), (want.clone(), false));
        let revs: Vec<Rev> = all.iter().map(|c| c.rev).collect();
        assert_eq!(revs, [10, 20, 30, 40, 50, 60]);
        let (after, _) = read(30, 100);
        assert_eq!(keys(&after), want[3..]);
        let (two, more) = read(0, 2);
        assert_eq!((keys(&two), more), (want[..2].to_vec(), true));

        // A sender that carries no stamps sends the versions alone.
        let bare: Vec<Moved> = batch
            .keys
            .iter()
            .map(|m| {
                let rows = m.rows.as_ref().map(|rows| {
                    let (versions, _) =
                        postcard::take_from_bytes::<Vec<(u64, u64, Option<Vec<u8>>)>>(rows)
                            .unwrap();
                    postcard::to_allocvec(&versions).unwrap()
                });
                Moved { rows, ..m.clone() }
            })
            .collect();
        plain
            .transaction(|db| crate::database::group::import(db, &bare))
            .unwrap();
        let refused = changes_in(&plain, "n", "f", 0, 1_000, 100).unwrap();
        assert!(
            matches!(&refused, Err(Refusal::Unavailable(why)) if why.contains("without its commit stamp")),
            "{refused:?}"
        );
    }

    /// T-T1.9.R26 (CP-T1.9; reshard.md, "Decision (lane split-merge-20): a read as of a stamp
    /// places moved rows by their stamps"). Do: two databases made groups' (unbacked, so the test
    /// writes the stamps): the old one writes a1 at 10, a2 at 30 and a1 again at 50 (its
    /// revisions 1 to 3); the new one b1, b2, b3 at 20, 22, 24 (its own revisions 1 to 3); the
    /// old one's keys moved into the new one after all of them; then the new one writes b1 again
    /// at 70 and the moved a2 again at 80. Then the new one is scanned, and each key read, as of
    /// stamps before the move: 5, 15, 25, 35, 55, 75 and one past every stamp, and as of 55 two
    /// at a time. Expect: each read holds exactly the versions stamped at or before it, a moved
    /// row by the stamp it carried, never by its old group's revision number: at 15 a1 alone
    /// (the new group had no revision yet), at 25 a1's first value and the b-keys but not a2 nor
    /// a1's second (whose old revisions 2 and 3 are at or below the new group's), at 55 a1's
    /// second; each row answering its stamp; the pages of two alike.
    #[test]
    fn t_t1_9_r26_a_read_as_of_an_older_stamp_places_moved_rows_by_their_stamps() {
        let dir = tempfile::tempdir().unwrap();
        let open = |name: &str| {
            let db = Db::open(&dir.path().join(name)).unwrap();
            crate::database::bootstrap_db(&db).unwrap();
            db
        };
        let commit = |db: &Db, key: &[u8], value: &[u8], stamp: u64| {
            let at = crate::database::write_across(db, "n", "f", key, Some(value), None, 0, None)
                .unwrap();
            db.query(
                "INSERT INTO __amphora_rev_stamps(namespace, rev, stamp) VALUES ('n', ?1, ?2)",
                [integer(at).unwrap(), integer(stamp).unwrap()],
                |_| Ok(true),
            )
            .unwrap();
        };
        let (old, mut new) = (open("old.db"), open("new.db"));
        commit(&old, b"a1", b"1", 10);
        commit(&old, b"a2", b"1", 30);
        commit(&old, b"a1", b"2", 50);
        commit(&new, b"b1", b"1", 20);
        commit(&new, b"b2", b"1", 22);
        commit(&new, b"b3", b"1", 24);
        crate::database::group::begin(&old).unwrap();
        crate::database::group::begin(&new).unwrap();
        let batch = crate::database::group::export(&old, crate::reshard::Span::ALL, 100).unwrap();
        assert_eq!(batch.keys.len(), 2, "{batch:?}");
        new.transaction(|db| crate::database::group::import(db, &batch.keys))
            .unwrap();
        commit(&new, b"b1", b"2", 70);
        commit(&new, b"a2", b"3", 80);

        type Seen = Vec<(String, String, Rev)>;
        let seen = |rows: &[api::Row]| -> Seen {
            rows.iter()
                .map(|r| {
                    (
                        String::from_utf8(r.key.clone()).unwrap(),
                        String::from_utf8(r.value.clone()).unwrap(),
                        r.rev,
                    )
                })
                .collect()
        };
        let want = |rows: &[(&str, &str, Rev)]| -> Seen {
            rows.iter()
                .map(|(k, v, r)| ((*k).to_owned(), (*v).to_owned(), *r))
                .collect()
        };
        let scan = |at: u64, after: Option<&[u8]>, limit: usize| {
            let scan = Scan {
                prefix: vec![],
                after: after.map(<[u8]>::to_vec),
                limit,
                at: Consistency::Linearizable,
            };
            page_at(&new, "n", "f", &scan, at).unwrap().unwrap().read()
        };
        let b = [("b1", "1", 20), ("b2", "1", 22), ("b3", "1", 24)];
        let cases: [(u64, Vec<(&str, &str, Rev)>); 7] = [
            (5, vec![]),
            (15, vec![("a1", "1", 10)]),
            (25, [vec![("a1", "1", 10)], b.to_vec()].concat()),
            (
                35,
                [vec![("a1", "1", 10), ("a2", "1", 30)], b.to_vec()].concat(),
            ),
            (
                55,
                [vec![("a1", "2", 50), ("a2", "1", 30)], b.to_vec()].concat(),
            ),
            (
                75,
                vec![
                    ("a1", "2", 50),
                    ("a2", "1", 30),
                    ("b1", "2", 70),
                    ("b2", "1", 22),
                    ("b3", "1", 24),
                ],
            ),
            (
                1_000,
                vec![
                    ("a1", "2", 50),
                    ("a2", "3", 80),
                    ("b1", "2", 70),
                    ("b2", "1", 22),
                    ("b3", "1", 24),
                ],
            ),
        ];
        for (at, rows) in &cases {
            let page = scan(*at, None, 100);
            assert_eq!(seen(&page.rows), want(rows), "as of {at}");
            assert!(!page.more, "as of {at}");
            for key in ["a1", "a2", "b1", "b2", "b3"] {
                let row = row_at(&new, "n", "f", key.as_bytes(), *at)
                    .unwrap()
                    .unwrap()
                    .read();
                let expected: Seen = want(rows)
                    .into_iter()
                    .filter(|(k, _, _)| k == key)
                    .collect();
                assert_eq!(seen(row.as_slice()), expected, "{key} as of {at}");
            }
        }
        let first = scan(55, None, 2);
        assert_eq!(
            (seen(&first.rows), first.more),
            (want(&[("a1", "2", 50), ("a2", "1", 30)]), true)
        );
        let next = scan(55, Some(b"a2".as_slice()), 2);
        assert_eq!((seen(&next.rows), next.more), (want(&b[..2]), true));
    }

    /// T-T1.9.R27 (CP-T1.9; reshard.md, "Decision (lane split-merge-20): conditions across groups
    /// name commit stamps"). Do, under each protocol: two backed-up groups, the second's clock 500
    /// seconds behind the first's, keys ending in b in the second; 1a, 1b and 7a written, each in
    /// its own group, and read across them for their stamps; 1a written again. Then across both
    /// groups: a commit conditioned on 1a as of its first stamp and 1b as of its own; one on 1a as
    /// of its new stamp, 1b as of its own and 9b absent; one on 1b absent; one on 8a (never
    /// written) as of the head. Then in one group: on 7a absent, then on 7a as of the head. And a
    /// condition on a key-value row. Expect: the first aborted, 1a unmet, and none of its rows
    /// written; the second committed; the third aborted on 1b, the fourth on 8a; in one group the
    /// first aborted on 7a, the second committed; the key-value condition refused.
    #[test]
    fn t_t1_9_r27_a_commit_across_groups_holds_to_conditions_on_stamps() {
        for protocol in [Protocol::TwoPhase, Protocol::Parallel] {
            let dir = tempfile::tempdir().unwrap();
            let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS + 1));
            // A row key ends in its own key, so "1b" routes to group 1.
            let route: crate::xgroup::Route =
                Arc::new(|key: &[u8]| u32::from(key.last() == Some(&b'b')));
            let (dbs, _sinks, first) = groups_on(
                dir.path(),
                2,
                |group| {
                    let read = Arc::clone(&now);
                    let behind = group * 500 * SECOND_NS;
                    Arc::new(move || read.fetch_add(1, Ordering::SeqCst) - behind)
                },
                Arc::clone(&route),
            );
            drop(first);
            let cluster =
                Arc::new(Cluster::open(dbs.clone(), route, 6, protocol, 1, until()).unwrap());
            let across = dbs[0].across(Arc::clone(&cluster)).namespace("n").unwrap();
            let key = |k: &[u8]| record_key("n", "f", k).unwrap();
            let commit = |writes: &[&[u8]], conds: Vec<(Vec<u8>, Option<u64>)>| {
                let txn = Txn {
                    reads: vec![],
                    writes: writes.iter().map(|k| (key(*k), Some(vec![1]))).collect(),
                    conds,
                };
                cluster.commit(txn, until())
            };
            let stamp = |k: &[u8]| {
                across
                    .get("f", k, Consistency::Linearizable, until())
                    .unwrap()
                    .map(|row| row.rev)
            };
            for k in [b"1a", b"1b", b"7a"] {
                let answer = commit(&[k.as_slice()], vec![]).unwrap();
                assert!(matches!(answer, Answer::Committed { .. }), "{answer:?}");
            }
            let (a, b) = (stamp(b"1a").unwrap(), stamp(b"1b").unwrap());
            commit(&[b"1a".as_slice()], vec![]).unwrap();
            let again = stamp(b"1a").unwrap();
            assert!(again > a, "{again} {a} ({protocol:?})");

            let stale = commit(
                &[b"2a".as_slice(), b"2b".as_slice()],
                vec![(key(b"1a"), Some(a)), (key(b"1b"), Some(b))],
            );
            assert_eq!(
                stale,
                Ok(Answer::Aborted(Why::Unmet(key(b"1a")))),
                "{protocol:?}"
            );
            assert_eq!((stamp(b"2a"), stamp(b"2b")), (None, None), "{protocol:?}");
            let held = commit(
                &[b"3a".as_slice(), b"3b".as_slice()],
                vec![
                    (key(b"1a"), Some(again)),
                    (key(b"1b"), Some(b)),
                    (key(b"9b"), None),
                ],
            )
            .unwrap();
            assert!(
                matches!(held, Answer::Committed { .. }),
                "{held:?} ({protocol:?})"
            );
            let there = commit(
                &[b"4a".as_slice(), b"4b".as_slice()],
                vec![(key(b"1b"), None)],
            );
            assert_eq!(
                there,
                Ok(Answer::Aborted(Why::Unmet(key(b"1b")))),
                "{protocol:?}"
            );
            let head = across.head(until()).unwrap();
            let missing = commit(
                &[b"5a".as_slice(), b"5b".as_slice()],
                vec![(key(b"8a"), Some(head))],
            );
            assert_eq!(
                missing,
                Ok(Answer::Aborted(Why::Unmet(key(b"8a")))),
                "{protocol:?}"
            );

            // In one group, no coordinator step: the same check.
            let one = commit(&[b"6a".as_slice()], vec![(key(b"7a"), None)]);
            assert_eq!(
                one,
                Ok(Answer::Aborted(Why::Unmet(key(b"7a")))),
                "{protocol:?}"
            );
            let one = commit(&[b"6a".as_slice()], vec![(key(b"7a"), Some(head))]).unwrap();
            assert!(matches!(one, Answer::Committed { .. }), "{one:?}");

            let plain = cluster.commit(
                Txn {
                    reads: vec![],
                    writes: vec![(b"kv".to_vec(), Some(vec![1]))],
                    conds: vec![(b"kv".to_vec(), None)],
                },
                until(),
            );
            assert!(matches!(&plain, Err(Refusal::Invalid { .. })), "{plain:?}");
        }
    }

    /// T-T1.9.R24 (CP-T1.9; reshard.md, "Decision (lane split-merge-19): one key and what changed,
    /// across groups"). Do: two backed-up groups, the second's clock 500 seconds behind the
    /// first's, routing a key ending in b to the second; rows 1a and 1b written in their groups;
    /// then across them: each key read, a key never written, 1a as of just before its stamp, and
    /// what changed after nothing and after the head; then 2b written, and what changed after the
    /// head again. Expect: each row with its own stamp as its revision (the changes' stamps), the
    /// key never written none, 1a before its stamp none; what changed after nothing the newest
    /// stamp (1a's: the second group's clock is behind), after the head none, and after 2b its
    /// stamp, past the head.
    #[test]
    fn t_t1_9_r24_one_key_and_what_changed_across_groups() {
        let dir = tempfile::tempdir().unwrap();
        let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS + 1));
        let (dbs, _sinks, cluster) = groups_on(
            dir.path(),
            2,
            |group| {
                let read = Arc::clone(&now);
                let behind = group * 500 * SECOND_NS;
                Arc::new(move || read.fetch_add(1, Ordering::SeqCst) - behind)
            },
            // A row key ends in its own key, so "1b" routes to group 1.
            Arc::new(|key: &[u8]| u32::from(key.last() == Some(&b'b'))),
        );
        put(&dbs[0], b"1a".to_vec());
        put(&dbs[1], b"1b".to_vec());
        let across = dbs[0].across(Arc::clone(&cluster)).namespace("n").unwrap();
        let all = across.changes("f", 0, 100, until()).unwrap();
        let stamp = |key: &[u8]| {
            all.changes
                .iter()
                .find(|c| c.key == key)
                .map(|c| c.rev)
                .unwrap()
        };
        let get = |key: &[u8], at| across.get("f", key, at, until()).unwrap();
        let a = get(b"1a".as_slice(), Consistency::Linearizable).unwrap();
        let b = get(b"1b".as_slice(), Consistency::Linearizable).unwrap();
        assert_eq!(
            (a.rev, b.rev),
            (stamp(b"1a".as_slice()), stamp(b"1b".as_slice()))
        );
        assert!(get(b"1c".as_slice(), Consistency::Linearizable).is_none());
        assert!(
            get(
                b"1a".as_slice(),
                Consistency::Snapshot(stamp(b"1a".as_slice()) - 1)
            )
            .is_none()
        );
        assert_eq!(
            get(
                b"1a".as_slice(),
                Consistency::Snapshot(stamp(b"1a".as_slice()))
            ),
            Some(a)
        );

        let newest = all.changes.iter().map(|c| c.rev).max();
        assert_eq!(across.changed("f", 0, until()).unwrap(), newest);
        assert_eq!(across.changed("f", all.head, until()).unwrap(), None);
        put(&dbs[1], b"2b".to_vec());
        let later = across.changed("f", all.head, until()).unwrap().unwrap();
        assert!(later > all.head, "{later} {}", all.head);
    }

    /// T-T1.9.R25 (CP-T1.9; reshard.md, "Decision (lane split-merge-19): one key and what changed,
    /// across groups"). Do: two backed-up groups; a row written in the first; a wait across them
    /// after the head, on a thread of its own, while a row is written in the second; then a wait
    /// after the newest stamp with a deadline and nothing written. Expect: the first wait answers
    /// the second row's stamp, past the head, as `changed` does; the second is refused `Timeout`.
    #[test]
    fn t_t1_9_r25_a_wait_across_groups_wakes_on_any_groups_change() {
        let dir = tempfile::tempdir().unwrap();
        let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS + 1));
        // Rows are written into each group directly, so the route is never asked.
        let (dbs, _sinks, cluster) = groups_on(
            dir.path(),
            2,
            |_| {
                let read = Arc::clone(&now);
                Arc::new(move || read.fetch_add(1, Ordering::SeqCst))
            },
            Arc::new(|_: &[u8]| 0),
        );
        put(&dbs[0], b"1a".to_vec());
        let across = dbs[0].across(Arc::clone(&cluster)).namespace("n").unwrap();
        let head = across.head(until()).unwrap();
        let waiting = {
            let across = across.clone();
            std::thread::spawn(move || across.wait("f", head, until()))
        };
        put(&dbs[1], b"1b".to_vec());
        let woke = waiting.join().unwrap().unwrap();
        assert!(woke > head, "{woke} {head}");
        assert_eq!(across.changed("f", head, until()).unwrap(), Some(woke));

        let soon = Instant::now() + Duration::from_millis(50);
        let refused = across.wait("f", woke, soon);
        assert!(matches!(refused, Err(Refusal::Timeout)), "{refused:?}");
    }

    /// T-T1.9.R20 (CP-T1.9; reshard.md, "Decision (lane split-merge-18): reads across groups by
    /// commit stamp"). Do: two groups, backed up, the second's clock 500 seconds behind the
    /// first's, under one coordinator; Records rows a1, b1, a2, b2 written in that order through
    /// it, a-keys to group 0, b-keys to group 1; then read across groups: the changes whole, the
    /// changes a page of one at a time, after b1's stamp, and the head; a scan of three, then on;
    /// a row written after the head. Expect: the changes in the order written whatever the clocks
    /// read, their stamps rising; the pages of one the same changes; after b1, a2 and b2; the head
    /// at or past b2's stamp, and the later row's stamp past it; the scan a1, a2, b1 with more,
    /// then b2 and no more; a group that stamps nothing refused.
    #[test]
    fn t_t1_9_r20_reads_across_groups_in_the_order_of_commit_stamps() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = Arc::new(FolderBlobs::open(&dir.path().join("archive"), 64 * 1024).unwrap());
        let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS + 1));
        let open = |group: u64| {
            let read = Arc::clone(&now);
            let behind = group * 500 * SECOND_NS;
            let archive = Archive {
                blobs: Arc::clone(&blobs) as Arc<dyn Blobs>,
                prefix: format!("g{group}"),
                clock: Arc::new(move || read.fetch_add(1, Ordering::SeqCst) - behind),
                memory: 32 * 1024,
                patience: Duration::from_secs(60),
                check: CheckPolicy::Off,
            };
            Database::open_backed_up(&dir.path().join(format!("g{group}.db")), archive, until())
                .unwrap()
        };
        let (g0, _s0) = open(0);
        let (g1, _s1) = open(1);
        let route: crate::xgroup::Route =
            Arc::new(|key: &[u8]| u32::from(key.last() == Some(&b'b')));
        let cluster = Arc::new(
            Cluster::open(
                vec![g0.clone(), g1.clone()],
                route,
                3,
                Protocol::TwoPhase,
                1,
                until(),
            )
            .unwrap(),
        );
        let across = g0.across(Arc::clone(&cluster)).namespace("n").unwrap();
        // The route reads a key's last byte; a row key ends in its own key, so "1a" is group 0's.
        let write = |key: &[u8]| {
            let txn = Txn {
                reads: vec![],
                writes: vec![(record_key("n", "f", key).unwrap(), Some(key.to_vec()))],
                conds: Vec::new(),
            };
            cluster.commit(txn, until()).unwrap();
        };
        let written: [&[u8]; 4] = [b"1a", b"1b", b"2a", b"2b"];
        for key in written {
            write(key);
        }

        let all = across.changes("f", 0, 100, until()).unwrap();
        let keys: Vec<&[u8]> = all.changes.iter().map(|c| c.key.as_slice()).collect();
        assert_eq!(keys, written);
        assert!(
            all.changes.windows(2).all(|w| w[0].rev < w[1].rev),
            "{all:?}"
        );
        let mut paged = Vec::new();
        let mut cursor = 0;
        loop {
            let page = across.changes("f", cursor, 1, until()).unwrap();
            let Some(last) = page.changes.last() else {
                break;
            };
            cursor = last.rev;
            paged.extend(page.changes);
        }
        assert_eq!(paged, all.changes);
        let after_b1 = across
            .changes("f", all.changes[1].rev, 100, until())
            .unwrap();
        assert_eq!(after_b1.changes, all.changes[2..]);
        let head = across.head(until()).unwrap();
        assert!(head >= all.changes[3].rev, "{head} under {all:?}");

        let scan = |after: Option<Vec<u8>>, limit| {
            across
                .scan(
                    "f",
                    Scan {
                        prefix: vec![],
                        after,
                        limit,
                        at: Consistency::Linearizable,
                    },
                    until(),
                )
                .unwrap()
        };
        let first = scan(None, 3);
        let keys: Vec<&[u8]> = first.rows.iter().map(|r| r.key.as_slice()).collect();
        assert_eq!((keys, first.more), (vec![&b"1a"[..], b"1b", b"2a"], true));
        let rest = scan(first.rows.last().map(|r| r.key.clone()), 3);
        let keys: Vec<&[u8]> = rest.rows.iter().map(|r| r.key.as_slice()).collect();
        assert_eq!((keys, rest.more), (vec![&b"2b"[..]], false));

        write(b"3b");
        let later = across.changes("f", head, 100, until()).unwrap();
        assert_eq!(later.changes.len(), 1, "{later:?}");
        assert!(later.changes[0].rev > head);

        let plain = Database::in_memory(until()).unwrap();
        let unstamped = Cluster::open(
            vec![g0.clone(), plain],
            Arc::new(|_: &[u8]| 0),
            4,
            Protocol::TwoPhase,
            1,
            until(),
        )
        .unwrap();
        let refused = g0
            .across(Arc::new(unstamped))
            .namespace("n")
            .unwrap()
            .head(until());
        assert!(
            matches!(&refused, Err(Refusal::Unavailable(why)) if why.contains("stamps no commits")),
            "{refused:?}"
        );
    }

    /// T-T1.9.R28 (CP-T1.9; reshard.md, "Decision (lane split-merge-21): one clock, one commit
    /// stamp"). Do: two backed-up groups, the second's clock 500 seconds behind the first's; the
    /// head across them read; a transaction across them locks a row in each, its steps served as
    /// the transport serves them, its commit stamp the latest stamp the groups answered after their
    /// locks; the first group resolves it at that stamp. Then each group read as of the stamp, and
    /// the second as of the head before the locks; then a scan across both as of the stamp, begun
    /// before the second group resolves it, which it then does; then a commit of the second
    /// group's own. Expect: the first group answers its row at the commit stamp; the second's read
    /// as of the stamp is held by the lock, and as of the head before answers no row unheld; the
    /// scan answers both rows, each at the one commit stamp, never one alone, and so do the
    /// changes after the head before; the commit after it in the second group is stamped after it.
    #[test]
    fn t_t1_9_r28_no_read_across_groups_sees_half_a_transaction() {
        use crate::xgroup::{Envelope, GroupId, Outcome, Reply, Step, TxnId};
        let dir = tempfile::tempdir().unwrap();
        let now = Arc::new(AtomicU64::new(1_000 * SECOND_NS + 1));
        let (dbs, _sinks, cluster) = groups_on(
            dir.path(),
            2,
            |group| {
                let read = Arc::clone(&now);
                let behind = group * 500 * SECOND_NS;
                Arc::new(move || read.fetch_add(1, Ordering::SeqCst) - behind)
            },
            Arc::new(|_: &[u8]| 0),
        );
        let across = dbs[0]
            .across_at(Arc::clone(&cluster), ReadPoint::Latest)
            .namespace("n")
            .unwrap();
        let feed = dbs[0]
            .across_at(Arc::clone(&cluster), ReadPoint::Latest)
            .namespace("n")
            .unwrap();
        let before = across.head(until()).unwrap();
        let txn = TxnId {
            coordinator: 7,
            epoch: 1,
            seq: 1,
        };
        let envelope = |step: Step| Envelope {
            coordinator: 7,
            epoch: 1,
            floor: None,
            step,
        };
        let key = |g: usize| vec![u8::try_from(g).unwrap(), b'a'];
        let serve = |g: usize, carried: u64, step: Step| {
            crate::xgroup::sql::serve(
                &dbs[g],
                GroupId::try_from(g).unwrap(),
                carried,
                envelope(step),
                until(),
            )
            .unwrap()
        };
        let mut carried = 0;
        for g in 0..2 {
            let lock = Step::Lock {
                txn,
                anchor: 0,
                reads: vec![],
                writes: vec![(record_key("n", "f", &key(g)).unwrap(), Some(vec![1]))],
                stage: None,
            };
            let (reply, latest) = serve(g, carried, lock);
            assert!(matches!(reply, Reply::Locked { .. }), "{reply:?}");
            carried = carried.max(latest.unwrap());
        }
        let at = carried;
        let resolve = Step::ResolveAt {
            txn,
            outcome: Outcome::Committed,
            at,
        };
        assert_eq!(serve(0, at, resolve.clone()).0, Reply::Resolved);

        let read = |g: usize, stamp: u64| {
            let k = key(g);
            dbs[g]
                .call_internal(until(), move |db| {
                    Ok(match row_at(db, "n", "f", &k, stamp)? {
                        Ok(Held::Read(row)) => Some(row.map(|r| r.rev)),
                        Ok(Held::Locked) => None,
                        Err(refusal) => anyhow::bail!("{refusal}"),
                    })
                })
                .unwrap()
        };
        assert_eq!(
            read(0, at),
            Some(Some(at)),
            "the first group's row at the one stamp"
        );
        assert_eq!(
            read(1, at),
            None,
            "the second group's read held by the lock"
        );
        assert_eq!(
            read(1, before),
            Some(None),
            "a read before the lock's stamp isn't held"
        );

        let scanning = std::thread::spawn(move || {
            across.scan(
                "f",
                Scan {
                    prefix: vec![],
                    after: None,
                    limit: 10,
                    at: Consistency::Snapshot(at),
                },
                until(),
            )
        });
        assert_eq!(serve(1, at, resolve).0, Reply::Resolved);
        let page = scanning.join().unwrap().unwrap();
        let rows: Vec<(Vec<u8>, Rev)> = page.rows.iter().map(|r| (r.key.clone(), r.rev)).collect();
        assert_eq!(rows, vec![(key(0), at), (key(1), at)], "{page:?}");
        let changes = feed.changes("f", before, 10, until()).unwrap();
        let mut changed: Vec<(Vec<u8>, Rev)> = changes
            .changes
            .iter()
            .map(|c| (c.key.clone(), c.rev))
            .collect();
        changed.sort();
        assert_eq!(changed, rows, "{changes:?}");

        put(&dbs[1], vec![1, b'z']);
        let later = dbs[1]
            .call_internal(until(), |db| {
                crate::database::newest_stamp(db, "n", "f", &[1, b'z'])
            })
            .unwrap()
            .unwrap();
        assert!(later > at, "{later} after {at}");
    }
}
