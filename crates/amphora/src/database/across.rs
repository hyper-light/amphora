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

use std::sync::Arc;
use std::time::Instant;

use amphora_api::{self as api, Change, Changes, Consistency, Page, Refusal, Rev, Scan};

use super::{Answer, Operation, bytes, integer, rev, text, unavailable};
use crate::backup::Hooks;
use crate::database::Database;
use crate::db::Db;
use crate::xgroup::sql::Member;

/// Writes the commit stamp of revision `at` of namespace `ns`, in the commit that makes it, on a
/// writer that stamps its commits; a revision already stamped (a cross-group step's rows share one)
/// keeps its stamp. An unbacked writer stamps nothing: it has no place in the order across groups.
pub(super) fn stamp(db: &Db, ns: &str, at: Rev) -> anyhow::Result<()> {
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
    let stamp = hooks.marks.next()?;
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
    db.exec("CREATE TABLE IF NOT EXISTS __amphora_stamp_trim(namespace TEXT PRIMARY KEY, through INTEGER NOT NULL, cursor INTEGER NOT NULL)")?;
    let mut state = None;
    db.query(
        "SELECT through, cursor FROM __amphora_stamp_trim WHERE namespace = ?1",
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
        "INSERT INTO __amphora_stamp_trim(namespace, through, cursor) VALUES (?1, ?2, ?3) ON CONFLICT(namespace) DO UPDATE SET through = excluded.through, cursor = excluded.cursor",
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

/// The cut: the latest of a stamp taken from every group, observed by every group.
fn cut(groups: &[(Database, Arc<Hooks>)]) -> Result<u64, Refusal> {
    let mut cut = 0;
    for (_, hooks) in groups {
        cut = cut.max(hooks.marks.next().map_err(unavailable)?);
    }
    for (_, hooks) in groups {
        hooks.marks.observe(cut).map_err(unavailable)?;
    }
    Ok(cut)
}

/// The newest revision of `ns` here stamped at or before `at`: one below the first stamped after
/// it (the stamps rise with the revisions), or the namespace's head when none is.
fn through(db: &Db, ns: &str, at: u64) -> anyhow::Result<Rev> {
    let mut after = None;
    db.query(
        "SELECT rev FROM __amphora_rev_stamps WHERE namespace = ?1 AND stamp > ?2 ORDER BY stamp LIMIT 1",
        [text(ns), integer(at)?],
        |r| {
            after = Some(rev(r, 0)?);
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

/// The namespace's head across groups: the cut, a stamp every group has made all its revisions
/// at or before by the time this answers.
pub(super) fn head(groups: &[Member], _ns: &str, until: Instant) -> Result<Rev, Refusal> {
    let groups = stamped(groups)?;
    let cut = cut(&groups)?;
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
    ns: &str,
    family: &str,
    scan: &Scan,
    until: Instant,
) -> Result<Page, Refusal> {
    let groups = stamped(groups)?;
    let cut = cut(&groups)?;
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
        let page = group.call_internal(until, move |db| {
            let r = through(db, &ns, at)?;
            let operation = Operation::Scan(
                family,
                Scan {
                    at: Consistency::Snapshot(r),
                    ..scan
                },
            );
            let mut page = match super::apply(db, &ns, &operation)? {
                Ok(Answer::Page(page)) => page,
                Ok(_) => return Ok(Err(unavailable("wrong scan answer"))),
                Err(refusal) => return Ok(Err(refusal)),
            };
            for row in &mut page.rows {
                row.rev = stamp_of(db, &ns, row.rev)?.unwrap_or(0);
            }
            Ok(Ok(page))
        })??;
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
    let mut feed = 0;
    db.query(
        "SELECT feed FROM __amphora_namespaces WHERE name = ?1",
        [text(ns)],
        |r| {
            feed = rev(r, 0)?;
            Ok(false)
        },
    )?;
    let (from, to) = (through(db, ns, after)?, through(db, ns, cut)?);
    if from < feed {
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
        return Ok(Err(Refusal::Compacted {
            asked: after,
            horizon,
        }));
    }
    let mut found = Vec::new();
    db.query(
        "SELECT rev, key, value FROM __amphora_versions WHERE namespace = ?1 AND family = ?2 AND rev > ?3 AND rev <= ?4 ORDER BY rev, seq LIMIT ?5",
        [
            text(ns),
            text(family),
            integer(from)?,
            integer(to)?,
            integer(
                u64::try_from(limit)?
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("limit overflow"))?,
            )?,
        ],
        |r| {
            found.push(Change {
                rev: rev(r, 0)?,
                key: bytes(r, 1)?.ok_or_else(|| anyhow::anyhow!("null key"))?,
                value: bytes(r, 2)?,
            });
            Ok(true)
        },
    )?;
    let mut last = None;
    for change in &mut found {
        if last.is_none_or(|(at, _)| at != change.rev) {
            let Some(stamp) = stamp_of(db, ns, change.rev)? else {
                return Ok(Err(Refusal::Unavailable(format!(
                    "revision {} has no commit stamp (moved in from another group): the order across groups doesn't place it yet",
                    change.rev
                ))));
            };
            last = Some((change.rev, stamp));
        }
        if let Some((_, stamp)) = last {
            change.rev = stamp;
        }
    }
    let more = found.len() > limit;
    Ok(whole(found, limit).map(|changes| (changes, more)))
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
    ns: &str,
    family: &str,
    after: Rev,
    limit: usize,
    until: Instant,
) -> Result<Changes, Refusal> {
    let groups = stamped(groups)?;
    let cut = cut(&groups)?;
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
        let (found, more) = group.call_internal(until, move |db| {
            changes_in(db, &ns, &family, after, cut, limit)
        })??;
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
    use crate::backup::{Archive, CheckPolicy, FolderBlobs, SECOND_NS};
    use crate::xgroup::sql::record_key;
    use crate::xgroup::{Cluster, Protocol, Txn};

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
        for i in 0..600u64 {
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
}
