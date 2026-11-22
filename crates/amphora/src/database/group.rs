//! A local database as a group's engine (CP-T1.9; docs/design/reshard.md, "What is left"): its
//! Records rows indexed by position, a log of its commits, and the copy path over them.
//!
//! A row routes by its namespace, family and key ([`crate::reshard::row_key`], hashed by
//! [`crate::reshard::position_of`]; decision "A row routes by its namespace, family and key").
//! A database becomes a group's when it first keeps a holding: from then on each commit is one
//! entry of its log (`__amphora_group_log`'s head), each Records key it writes is indexed by
//! position (`__amphora_positions`), and while the log is held (from a copy's first read until
//! its operation ends) each key a commit writes is noted with its entry (`__amphora_group_feed`),
//! the change feed a catch-up reads. A database that is no group's pays one lookup of a missing
//! row a write for it, and keeps none of these.
//!
//! What moves is a key's versions (every revision Records keeps of it), so a read at any
//! revision answers the same in the new group. The namespace's head is raised to the newest
//! revision moved in, so the new group's next commit is newer than every row it took.

use crate::db::Db;
use crate::reshard::holding::replays;
use crate::reshard::plan::Cut;
use crate::reshard::{Pos, Seq, Span, position_of, row_key, row_parts};
use crate::store::{Changed, Exported, Moved};

use super::{blob, bytes, integer, rev, text};

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS __amphora_group_log (one INTEGER PRIMARY KEY CHECK (one = 0), head INTEGER NOT NULL, pinned INTEGER, trimmed INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS __amphora_positions (pos INTEGER NOT NULL, namespace TEXT NOT NULL, family TEXT NOT NULL, key BLOB NOT NULL, PRIMARY KEY (pos, namespace, family, key));
CREATE TABLE IF NOT EXISTS __amphora_group_feed (seq INTEGER NOT NULL, pos INTEGER NOT NULL, namespace TEXT NOT NULL, family TEXT NOT NULL, key BLOB NOT NULL);
CREATE INDEX IF NOT EXISTS __amphora_group_feed_seq ON __amphora_group_feed (seq);
CREATE TABLE IF NOT EXISTS __amphora_group_stamps (namespace TEXT NOT NULL, family TEXT NOT NULL, key BLOB NOT NULL, stamp INTEGER NOT NULL, PRIMARY KEY (namespace, family, key));";

/// A u64's top bit: flipped, the u64's order is the order of the i64 of its bytes.
const TOP: u64 = 0x8000_0000_0000_0000;

/// A position as SQL orders it: the u64 with its top bit flipped, as an i64, so SQL's signed order
/// is the positions' order.
fn sql_pos(pos: Pos) -> amphora_engine::Value {
    amphora_engine::Value::from_i64(i64::from_ne_bytes((pos ^ TOP).to_ne_bytes()))
}

fn pos_of(v: i64) -> Pos {
    u64::from_ne_bytes(v.to_ne_bytes()) ^ TOP
}

fn pos_at(r: &amphora_engine::Row, n: usize) -> anyhow::Result<Pos> {
    Ok(pos_of(r.get::<i64>(n).map_err(|e| anyhow::anyhow!("{e}"))?))
}

fn text_at(r: &amphora_engine::Row, n: usize) -> anyhow::Result<String> {
    r.get::<String>(n).map_err(|e| anyhow::anyhow!("{e}"))
}

fn key_at(r: &amphora_engine::Row, n: usize) -> anyhow::Result<Vec<u8>> {
    bytes(r, n)?.ok_or_else(|| anyhow::anyhow!("a Records row with no key"))
}

/// A key's identity: namespace, family, key.
type Id = (String, String, Vec<u8>);

/// The log: its head, the entry it is held from (none: not held), and the last entry let go.
#[derive(Debug, Clone, Copy)]
struct Log {
    head: Seq,
    pinned: Option<Seq>,
    trimmed: Seq,
}

/// The log, if this database is a group's.
fn log(db: &Db) -> anyhow::Result<Option<Log>> {
    let mut out = None;
    db.query(
        "SELECT head, pinned, trimmed FROM __amphora_group_log WHERE one = 0",
        [],
        |r| {
            let pinned = match r.get_value(1) {
                amphora_engine::Value::Null => None,
                _ => Some(rev(r, 1)?),
            };
            out = Some(Log {
                head: rev(r, 0)?,
                pinned,
                trimmed: rev(r, 2)?,
            });
            Ok(false)
        },
    )?;
    Ok(out)
}

fn log_of(db: &Db) -> anyhow::Result<Log> {
    log(db)?.ok_or_else(|| anyhow::anyhow!("this database keeps no holding, so it is no group's"))
}

fn save(db: &Db, log: Log) -> anyhow::Result<()> {
    let pinned = log
        .pinned
        .map(integer)
        .transpose()?
        .unwrap_or(amphora_engine::Value::Null);
    db.query(
        "INSERT INTO __amphora_group_log(one, head, pinned, trimmed) VALUES (0, ?1, ?2, ?3) ON CONFLICT(one) DO UPDATE SET head = excluded.head, pinned = excluded.pinned, trimmed = excluded.trimmed",
        [integer(log.head)?, pinned, integer(log.trimmed)?],
        |_| Ok(true),
    )
}

/// One commit's entry, in the commit's own transaction, if this database is a group's: the head
/// moves on, and the log is let go up to it unless it is held.
pub(super) fn entry(db: &Db) -> anyhow::Result<()> {
    db.query(
        "UPDATE __amphora_group_log SET head = head + 1, trimmed = CASE WHEN pinned IS NULL THEN head + 1 ELSE trimmed END WHERE one = 0",
        [],
        |_| Ok(true),
    )
}

/// The database becomes a group's (its first holding kept), in that commit's transaction: its log
/// starts, and every Records key it holds already is indexed by position.
pub(super) fn begin(db: &Db) -> anyhow::Result<()> {
    if log(db)?.is_some() {
        return Ok(());
    }
    save(
        db,
        Log {
            head: 0,
            pinned: None,
            trimmed: 0,
        },
    )?;
    let mut ids: Vec<Id> = Vec::new();
    db.query(
        "SELECT DISTINCT namespace, family, key FROM __amphora_versions",
        [],
        |r| {
            ids.push((text_at(r, 0)?, text_at(r, 1)?, key_at(r, 2)?));
            Ok(true)
        },
    )?;
    for (ns, family, key) in &ids {
        indexed(db, ns, family, key)?;
    }
    Ok(())
}

fn position(ns: &str, family: &str, key: &[u8]) -> anyhow::Result<Pos> {
    let id = row_key(ns, family, key).ok_or_else(|| anyhow::anyhow!("a row key too long"))?;
    Ok(position_of(&id))
}

fn indexed(db: &Db, ns: &str, family: &str, key: &[u8]) -> anyhow::Result<Pos> {
    let pos = position(ns, family, key)?;
    db.query(
        "INSERT OR IGNORE INTO __amphora_positions(pos, namespace, family, key) VALUES (?1, ?2, ?3, ?4)",
        [sql_pos(pos), text(ns), text(family), blob(key)?],
        |_| Ok(true),
    )?;
    Ok(pos)
}

/// A Records key written by the commit under way: indexed by position, and noted in the feed with
/// the commit's entry while the log is held. Nothing if this database is no group's.
pub(super) fn wrote(db: &Db, ns: &str, family: &str, key: &[u8]) -> anyhow::Result<()> {
    let Some(log) = log(db)? else {
        return Ok(());
    };
    let pos = indexed(db, ns, family, key)?;
    if log.pinned.is_some() {
        let seq = log
            .head
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("the log's entries ran out"))?;
        db.query(
            "INSERT INTO __amphora_group_feed(seq, pos, namespace, family, key) VALUES (?1, ?2, ?3, ?4, ?5)",
            [integer(seq)?, sql_pos(pos), text(ns), text(family), blob(key)?],
            |_| Ok(true),
        )?;
    }
    Ok(())
}

/// The bytes the database's pages in use hold: its pages less its free ones, at its page size
/// (CP-T1.9; docs/design/reshard.md, decision "Load from the store's meter"). A file doesn't
/// shrink when rows leave it, so its length would count a swept range's pages long after.
pub(super) fn used_bytes(db: &Db) -> anyhow::Result<u64> {
    let one = |sql: &'static str| -> anyhow::Result<u64> {
        let mut out = 0;
        db.query(sql, [], |r| {
            out = rev(r, 0)?;
            Ok(false)
        })?;
        Ok(out)
    };
    let used = one("PRAGMA page_count")?.saturating_sub(one("PRAGMA freelist_count")?);
    Ok(used.saturating_mul(one("PRAGMA page_size")?))
}

/// Where the database's Records keys fall about half each side by position, read from the root
/// page of their position index alone (CP-T1.9, T-T1.9.R19; docs/design/reshard.md, decision "The
/// size cut from the B-tree's middle key"): the root's middle key, weighed by the root's children
/// on each side (a root that is a leaf: its keys). A B-tree keeps every leaf at one depth and splits
/// a full page into halves, so the root's children hold about as many keys each, and one page is
/// read whatever the rows. None: no group's, too few keys to cut, or a page it doesn't know.
pub(super) fn size_cut(db: &Db) -> anyhow::Result<Option<Cut>> {
    let mut root = None;
    db.query(
        "SELECT rootpage FROM sqlite_schema WHERE type = 'index' AND tbl_name = '__amphora_positions'",
        [],
        |r| {
            root = Some(r.get::<i64>(0).map_err(|e| anyhow::anyhow!("{e}"))?);
            Ok(false)
        },
    )?;
    let Some(root) = root else {
        return Ok(None);
    };
    let page = Some(db.page(root)?);
    // Page 1 starts with the file's header (SQLite's file format, §1.3: 100 bytes, a format value).
    let header = if root == 1 { 100 } else { 0 };
    Ok(page.and_then(|page| middle(&page, header)))
}

/// The middle cell of an index B-tree page as a cut: its key's first column (a position, as SQL
/// orders it) and, before and from it, the page's children (an interior page) or cells (a leaf).
/// The layout is SQLite's file format (§1.6, "B-tree Pages"; §2.1, "Record Format"), format values
/// all: a page header of 12 bytes (interior) or 8 (leaf), its cell count at bytes 3 and 4, then a
/// 2-byte pointer to each cell; an interior index cell is its left child's page number (4 bytes)
/// then its payload, a leaf's is its payload; a payload is its length, then its record.
fn middle(page: &[u8], header: usize) -> Option<Cut> {
    let interior = match page.get(header)? {
        0x02 => true,
        0x0a => false,
        _ => return None,
    };
    let count = |at: usize| -> Option<usize> {
        let two = page.get(at..at.checked_add(2)?)?;
        Some(usize::from(u16::from_be_bytes([
            *two.first()?,
            *two.get(1)?,
        ])))
    };
    let cells = count(header.checked_add(3)?)?;
    let i = cells.checked_div(2)?;
    // Keys before the middle cell: an interior page's children up to and with the cell's own left
    // child; a leaf's cells before it.
    let (left, right) = if interior {
        (i.checked_add(1)?, cells.checked_sub(i)?)
    } else {
        (i, cells.checked_sub(i)?)
    };
    if left == 0 || right == 0 {
        return None;
    }
    let pointers = header.checked_add(if interior { 12 } else { 8 })?;
    let cell = count(pointers.checked_add(i.checked_mul(2)?)?)?;
    let payload = cell.checked_add(if interior { 4 } else { 0 })?;
    let (_, n) = varint(page.get(payload..)?)?;
    let record = payload.checked_add(n)?;
    let (header_len, n) = varint(page.get(record..)?)?;
    let (serial, _) = varint(page.get(record.checked_add(n)?..)?)?;
    let body = record.checked_add(usize::try_from(header_len).ok()?)?;
    let at = pos_of(integer_of(serial, page.get(body..)?)?);
    Some(Cut {
        at,
        left: u64::try_from(left).ok()?,
        right: u64::try_from(right).ok()?,
    })
}

/// A varint of SQLite's file format (§1.6): one to nine bytes, big-endian, seven bits a byte with
/// the top bit set on all but the last, the ninth byte all eight bits. The value and its length.
fn varint(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut v: u64 = 0;
    for (n, b) in bytes.iter().take(9).enumerate() {
        if n == 8 {
            return Some(((v.checked_shl(8)?) | u64::from(*b), 9));
        }
        v = v.checked_shl(7)? | u64::from(b & 0x7f);
        if b & 0x80 == 0 {
            return Some((v, n.checked_add(1)?));
        }
    }
    None
}

/// An integer column of a record (SQLite's file format, §2.1): serial types 1 to 6 are big-endian
/// two's complement of 1, 2, 3, 4, 6 and 8 bytes, 8 and 9 the constants 0 and 1. None: another type.
fn integer_of(serial: u64, body: &[u8]) -> Option<i64> {
    let len = match serial {
        1..=4 => usize::try_from(serial).ok()?,
        5 => 6,
        6 => 8,
        8 => return Some(0),
        9 => return Some(1),
        _ => return None,
    };
    let bytes = body.get(..len)?;
    let mut v: i64 = if bytes.first()? & 0x80 == 0 { 0 } else { -1 };
    for b in bytes {
        v = v.checked_shl(8)? | i64::from(*b);
    }
    Some(v)
}

/// The log's head (0: no entry yet, or no group's).
pub(super) fn head(db: &Db) -> anyhow::Result<Seq> {
    Ok(log(db)?.map_or(0, |l| l.head))
}

/// A version of a key as it moves: its revision, its place in its commit, its value (none: a
/// delete).
type Version = (u64, u64, Option<Vec<u8>>);
/// A span's keys read in position order: each with its position, the last position taken, and
/// whether none is past it ([`keys_in`]).
type SpanKeys = (Vec<(Id, Pos)>, Option<Pos>, bool);

/// `id`'s versions as they stand now, in revision order (none: it has none).
fn versions(db: &Db, id: &Id) -> anyhow::Result<Vec<Version>> {
    let (ns, family, key) = id;
    let mut out = Vec::new();
    db.query(
        "SELECT rev, seq, value FROM __amphora_versions WHERE namespace = ?1 AND family = ?2 AND key = ?3 ORDER BY rev",
        [text(ns), text(family), blob(key)?],
        |r| {
            out.push((rev(r, 0)?, rev(r, 1)?, bytes(r, 2)?));
            Ok(true)
        },
    )?;
    Ok(out)
}

/// `ids` as they stand now, each stamped with the head and the entry that changed it.
fn moved(db: &Db, ids: Vec<(Id, Pos, Seq)>, head: Seq) -> anyhow::Result<Vec<Moved>> {
    let mut out = Vec::with_capacity(ids.len());
    for (id, pos, entry) in ids {
        let now = versions(db, &id)?;
        let (ns, family, key) = &id;
        let rows = if now.is_empty() {
            None
        } else {
            // Each version's commit stamp follows the versions (lane split-merge-19,
            // T-T1.9.R23): a reader that knows them only reads the versions and stops.
            let mut stamps = Vec::with_capacity(now.len());
            for (at, _, _) in &now {
                stamps.push(super::across::stamp_of_version(db, ns, family, key, *at)?);
            }
            let mut rows = postcard::to_allocvec(&now)
                .map_err(|_| anyhow::anyhow!("versions that don't encode"))?;
            rows.extend(
                postcard::to_allocvec(&stamps)
                    .map_err(|_| anyhow::anyhow!("stamps that don't encode"))?,
            );
            Some(rows)
        };
        let routing =
            row_key(ns, family, key).ok_or_else(|| anyhow::anyhow!("a row key too long"))?;
        out.push(Moved {
            pos,
            key: hex(&routing),
            rows,
            entry,
            at: head,
        });
    }
    Ok(out)
}

/// A routing key as [`Moved::key`] carries it: lowercase hex (a key of bytes in a string).
fn hex(bytes: &[u8]) -> String {
    let digit = |n: u8| char::from_digit(u32::from(n), 16).unwrap_or('0');
    bytes
        .iter()
        .flat_map(|b| [digit(b.wrapping_shr(4)), digit(b & 0xf)])
        .collect()
}

fn unhex(text: &str) -> anyhow::Result<Vec<u8>> {
    let digits = text.as_bytes();
    digits
        .chunks(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair)?;
            u8::from_str_radix(pair, 16).map_err(anyhow::Error::from)
        })
        .collect()
}

/// The keys at the first `positions` positions of `span` that `keep` takes, in position order:
/// the last position taken, and whether none is past it.
fn keys_in(
    db: &Db,
    span: Span,
    positions: usize,
    keep: &dyn Fn(Pos) -> bool,
) -> anyhow::Result<SpanKeys> {
    let mut keys: Vec<(Id, Pos)> = Vec::new();
    let (mut seen, mut last, mut done) = (0usize, None, true);
    db.query(
        "SELECT pos, namespace, family, key FROM __amphora_positions WHERE pos >= ?1 AND pos <= ?2 ORDER BY pos, namespace, family, key",
        [sql_pos(span.lo()), sql_pos(span.hi())],
        |r| {
            let pos = pos_at(r, 0)?;
            if !keep(pos) {
                return Ok(true);
            }
            if last != Some(pos) {
                if seen == positions {
                    done = false;
                    return Ok(false);
                }
                seen = seen.saturating_add(1);
                last = Some(pos);
            }
            keys.push(((text_at(r, 1)?, text_at(r, 2)?, key_at(r, 3)?), pos));
            Ok(true)
        },
    )?;
    Ok((keys, last, done))
}

/// [`crate::store::Engine::export`].
pub(super) fn export(db: &Db, span: Span, positions: usize) -> anyhow::Result<Exported> {
    let head = log_of(db)?.head;
    let (keys, last, done) = keys_in(db, span, positions, &|_| true)?;
    let ids = keys.into_iter().map(|(id, pos)| (id, pos, head)).collect();
    Ok(Exported {
        keys: moved(db, ids, head)?,
        at: head,
        last,
        done,
    })
}

/// [`crate::store::Engine::changed`].
pub(super) fn changed(
    db: &Db,
    after: Seq,
    upto: Seq,
    span: Span,
    entries: u64,
) -> anyhow::Result<Option<Changed>> {
    let log = log_of(db)?;
    if after < log.trimmed {
        return Ok(None);
    }
    let through = upto
        .min(log.head)
        .min(after.saturating_add(entries))
        .max(after);
    let mut ids: Vec<(Id, Pos, Seq)> = Vec::new();
    db.query(
        "SELECT namespace, family, key, pos, MAX(seq) FROM __amphora_group_feed WHERE seq > ?1 AND seq <= ?2 AND pos >= ?3 AND pos <= ?4 GROUP BY namespace, family, key, pos",
        [
            integer(after)?,
            integer(through)?,
            sql_pos(span.lo()),
            sql_pos(span.hi()),
        ],
        |r| {
            ids.push((
                (text_at(r, 0)?, text_at(r, 1)?, key_at(r, 2)?),
                pos_at(r, 3)?,
                rev(r, 4)?,
            ));
            Ok(true)
        },
    )?;
    Ok(Some(Changed {
        keys: moved(db, ids, log.head)?,
        through,
        head: log.head,
    }))
}

/// Whether `id`'s newest version is a value (the key is live in its namespace).
fn live(db: &Db, id: &Id) -> anyhow::Result<bool> {
    Ok(versions(db, id)?
        .last()
        .is_some_and(|(_, _, value)| value.is_some()))
}

/// A key of namespace `ns` moved in or out: the namespace's live keys counted again (`before` and
/// `now`: whether the key was live before and is now), its head raised to `newest`, the newest
/// revision moved in, and the namespace made if it isn't here yet.
fn account(db: &Db, ns: &str, before: bool, now: bool, newest: Option<u64>) -> anyhow::Result<()> {
    db.query(
        "INSERT OR IGNORE INTO __amphora_namespaces(name, head, horizon, feed, live) VALUES (?1, 0, 0, 0, 0)",
        [text(ns)],
        |_| Ok(true),
    )?;
    match (before, now) {
        (false, true) => db.query(
            "UPDATE __amphora_namespaces SET live = live + 1 WHERE name = ?1",
            [text(ns)],
            |_| Ok(true),
        )?,
        (true, false) => db.query(
            "UPDATE __amphora_namespaces SET live = live - 1 WHERE name = ?1 AND live > 0",
            [text(ns)],
            |_| Ok(true),
        )?,
        _ => {}
    }
    if let Some(newest) = newest {
        db.query(
            "UPDATE __amphora_namespaces SET head = MAX(__amphora_namespaces.head, ?2) WHERE name = ?1",
            [text(ns), integer(newest)?],
            |_| Ok(true),
        )?;
    }
    Ok(())
}

fn drop_key(db: &Db, id: &Id) -> anyhow::Result<()> {
    let (ns, family, key) = id;
    let params = || -> anyhow::Result<[amphora_engine::Value; 3]> {
        Ok([text(ns), text(family), blob(key)?])
    };
    db.query(
        "DELETE FROM __amphora_versions WHERE namespace = ?1 AND family = ?2 AND key = ?3",
        params()?,
        |_| Ok(true),
    )?;
    db.query(
        "DELETE FROM __amphora_positions WHERE namespace = ?1 AND family = ?2 AND key = ?3",
        params()?,
        |_| Ok(true),
    )?;
    db.query(
        "DELETE FROM __amphora_moved_stamps WHERE namespace = ?1 AND family = ?2 AND key = ?3",
        params()?,
        |_| Ok(true),
    )?;
    db.query(
        "DELETE FROM __amphora_group_stamps WHERE namespace = ?1 AND family = ?2 AND key = ?3",
        params()?,
        |_| Ok(true),
    )
}

/// [`crate::store::Engine::import`]'s writes, in the caller's transaction.
pub(super) fn import(db: &Db, moved: &[Moved]) -> anyhow::Result<()> {
    for m in moved {
        let routing = unhex(&m.key)?;
        let id: Id = row_parts(&routing)
            .ok_or_else(|| anyhow::anyhow!("a moved key that doesn't decode"))?;
        let (ns, family, key) = &id;
        let mut stamp = None;
        db.query(
            "SELECT stamp FROM __amphora_group_stamps WHERE namespace = ?1 AND family = ?2 AND key = ?3",
            [text(ns), text(family), blob(key)?],
            |r| {
                stamp = Some(rev(r, 0)?);
                Ok(false)
            },
        )?;
        if !replays(stamp, m.entry) {
            continue;
        }
        let before = live(db, &id)?;
        drop_key(db, &id)?;
        let Some(rows) = &m.rows else {
            account(db, ns, before, false, None)?;
            continue;
        };
        let (rows, rest): (Vec<Version>, &[u8]) = postcard::take_from_bytes(rows)
            .map_err(|_| anyhow::anyhow!("moved versions that don't decode"))?;
        // The versions' commit stamps, if the sender carried them (none from an older one).
        let stamps: Vec<Option<u64>> = if rest.is_empty() {
            Vec::new()
        } else {
            postcard::from_bytes(rest)
                .map_err(|_| anyhow::anyhow!("moved stamps that don't decode"))?
        };
        if !stamps.is_empty() && stamps.len() != rows.len() {
            anyhow::bail!("moved stamps that don't match their versions");
        }
        for (i, (at, seq, value)) in rows.iter().enumerate() {
            let stamp = stamps.get(i).copied().flatten();
            super::across::moved_in(db, ns, family, key, *at, stamp)?;
            if let (Some(stamp), Some(hooks)) = (stamp, db.stamps()) {
                // Every commit here from now on is stamped after the rows it took.
                hooks.marks.observe_micros(stamp)?;
            }
            let value = value
                .as_deref()
                .map(blob)
                .transpose()?
                .unwrap_or(amphora_engine::Value::Null);
            db.query(
                "INSERT INTO __amphora_versions(namespace, family, key, rev, seq, value) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                [text(ns), text(family), blob(key)?, integer(*at)?, integer(*seq)?, value],
                |_| Ok(true),
            )?;
        }
        let newest = rows.last().map(|(at, _, _)| *at);
        if let Some(at) = newest {
            db.query(
                "INSERT INTO __amphora_families(namespace, family, newest) VALUES (?1, ?2, ?3) ON CONFLICT(namespace, family) DO UPDATE SET newest = MAX(__amphora_families.newest, excluded.newest)",
                [text(ns), text(family), integer(at)?],
                |_| Ok(true),
            )?;
        }
        db.query(
            "INSERT INTO __amphora_positions(pos, namespace, family, key) VALUES (?1, ?2, ?3, ?4)",
            [sql_pos(m.pos), text(ns), text(family), blob(key)?],
            |_| Ok(true),
        )?;
        db.query(
            "INSERT INTO __amphora_group_stamps(namespace, family, key, stamp) VALUES (?1, ?2, ?3, ?4)",
            [text(ns), text(family), blob(key)?, integer(m.at)?],
            |_| Ok(true),
        )?;
        let now = rows.last().is_some_and(|(_, _, v)| v.is_some());
        account(db, ns, before, now, newest)?;
    }
    Ok(())
}

/// [`crate::store::Engine::keep_log`]'s change, in the caller's transaction.
pub(super) fn keep_log(db: &Db, from: Option<Seq>) -> anyhow::Result<()> {
    let mut log = log_of(db)?;
    log.pinned = match (log.pinned, from) {
        (Some(p), Some(f)) => Some(p.min(f)),
        (_, f) => f,
    };
    if log.pinned.is_none() {
        log.trimmed = log.head;
        db.query("DELETE FROM __amphora_group_feed", [], |_| Ok(true))?;
    }
    save(db, log)
}

/// [`crate::store::Engine::sweep`]'s deletes, in the caller's transaction: whether none is left.
pub(super) fn sweep(
    db: &Db,
    span: Span,
    positions: usize,
    garbage: &dyn Fn(Pos) -> bool,
) -> anyhow::Result<bool> {
    let (keys, _, done) = keys_in(db, span, positions, garbage)?;
    for (id, _) in &keys {
        let before = live(db, id)?;
        drop_key(db, id)?;
        account(db, &id.0, before, false, None)?;
    }
    Ok(done)
}
