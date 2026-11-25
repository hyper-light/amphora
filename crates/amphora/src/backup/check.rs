//! Continuous restore-and-check (docs/design/backup.md, "Restore-and-check"): the newest second the
//! archive holds whole is restored into a scratch file and checked, its rows against the source's
//! at that second and the file by `PRAGMA integrity_check`, as Meta's continuous restores and
//! GitLab's daily restore tests check theirs ("A backup nobody restores is not a backup",
//! ~/.cache/amphora-t1/research/backup-single-node.md §0.3). The outcome is a typed [`Check`] and
//! a count in [`CheckCounters`] (`amphora_backup_checks_total`).

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use amphora_api::blobs::Blobs;
use amphora_engine_parser::ast::{CreateTableBody, Stmt};
use sha2::{Digest as _, Sha256};

use super::drill::{Pace, Unpaced};
use super::plan::{self, SegmentKey, ThroughKey};
use super::restore::{Restored, restore_paced, through_marks};
use super::ship::list_all;
use super::{SECOND_NS, second_of};

use crate::db::Db;

/// A database's rows, table by table: each table's name, its row count and a SHA-256 over its rows
/// in rowid order, or a WITHOUT ROWID table's in its columns' order ([`row_order`]). Two databases
/// with the same rows have the same digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowDigest(pub Vec<(String, u64, [u8; 32])>);

/// The digest of every table of `db` but the engine's own (`sqlite_*`).
pub fn digest(db: &Db) -> anyhow::Result<RowDigest> {
    digest_paced(db, &mut Unpaced, u64::MAX)
}

/// [`digest`], a step of `pace`'s for each `step` bytes of rows read (T-T1.12.C7).
pub fn digest_paced(db: &Db, pace: &mut dyn Pace, step: u64) -> anyhow::Result<RowDigest> {
    let mut tables = Vec::new();
    db.query(
        "SELECT name, sql FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite!_%' ESCAPE '!' ORDER BY name",
        [],
        |r| {
            let name = r.get::<String>(0).map_err(|e| anyhow::anyhow!("{e}"))?;
            let sql = r.get::<String>(1).map_err(|e| anyhow::anyhow!("{e}"))?;
            tables.push((name, sql));
            Ok(true)
        },
    )?;
    let mut out = Vec::with_capacity(tables.len());
    let mut read = 0u64;
    pace.ready()?;
    for (table, sql) in tables {
        let quoted = table.replace('"', "\"\"");
        let (select_list, order) = row_query(db, &table, &sql)?;
        let mut hash = Sha256::new();
        let mut rows = 0u64;
        db.query_owned(
            &format!("SELECT {select_list} FROM \"{quoted}\" ORDER BY {order}"),
            [],
            |r| {
                let values: Vec<_> = r.get_values().collect();
                let line = format!("{values:?}\n");
                hash.update(line.as_bytes());
                rows = rows.saturating_add(1);
                read = read.saturating_add(u64::try_from(line.len()).unwrap_or(u64::MAX));
                if read >= step {
                    pace.done(None, read)?;
                    read = 0;
                    pace.ready()?;
                }
                Ok(true)
            },
        )?;
        out.push((table, rows, hash.finalize().into()));
    }
    pace.done(None, read)?;
    Ok(RowDigest(out))
}

/// The SELECT list and ORDER BY clause a digest reads `table`'s rows with, from its `CREATE TABLE`
/// text `sql`. For rowid tables, includes the first unshadowed SQLite rowid alias (`rowid`,
/// `_rowid_`, or `oid`) in both SELECT and ORDER BY so tables with identical visible columns at
/// different rowids produce distinct digests; for WITHOUT ROWID tables (or tables shadowing all
/// three rowid aliases), reads `*` ordered by every column in turn (T-T1.12.C1).
fn row_query(db: &Db, table: &str, sql: &str) -> anyhow::Result<(String, String)> {
    let Stmt::CreateTable {
        body: CreateTableBody::ColumnsAndConstraints {
            columns, options, ..
        },
        ..
    } = db
        .parse_table_sql_ast(sql)
        .map_err(|e| anyhow::anyhow!("table {table}: {e}"))?
    else {
        return Err(anyhow::anyhow!("table {table} is not a table of columns"));
    };
    if !options.contains_without_rowid()
        && let Some(alias) = ["rowid", "_rowid_", "oid"].into_iter().find(|candidate| {
            !columns
                .iter()
                .any(|col| col.col_name.as_str().eq_ignore_ascii_case(candidate))
        })
    {
        return Ok((format!("{alias}, *"), alias.to_owned()));
    }
    let all_cols = (1..=columns.len())
        .map(|column| column.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Ok(("*".to_owned(), all_cols))
}

/// The bytes of rows a paced digest reads a step: as many as one object of the store holds, the
/// size a restore's steps move (a base's chunk, a log segment's part).
pub(crate) fn step_bytes(blobs: &dyn Blobs) -> u64 {
    u64::try_from(blobs.object_max()).unwrap_or(u64::MAX)
}

/// Why a check failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckFailure {
    /// The archive didn't restore the second.
    Restore(String),
    /// The restored file failed `PRAGMA integrity_check`; its first lines.
    Integrity(String),
    /// The restored rows differ from the source's at that second.
    Rows {
        source: RowDigest,
        restored: RowDigest,
    },
}

/// What one restore-and-check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    /// The second restored, whole and equal to the source.
    Passed {
        second: u64,
        restored: Restored,
    },
    Failed {
        second: u64,
        why: CheckFailure,
    },
    /// The source could no longer say its rows at the second (it moved on since): nothing compared.
    SourceMoved {
        second: u64,
    },
    /// The archive holds no second whole yet.
    Empty,
    /// The second is before the archive's first commit now: its owner expired it ([`super::expire`]).
    /// Only a second older than the archive's oldest segment is this; one the archive should
    /// still restore and doesn't is `Failed`.
    Expired {
        second: u64,
    },
}

/// Checks run, by result: what `amphora_backup_checks_total{result}` reports.
#[derive(Debug, Default)]
pub struct CheckCounters {
    pub passed: AtomicU64,
    pub failed: AtomicU64,
    pub source_moved: AtomicU64,
    pub expired: AtomicU64,
}

/// `amphora_backup_checks_total`: its type and help line, as `CheckCounters` counts it.
pub const CHECK_METRIC: (&str, &str, &str) = (
    "amphora_backup_checks_total",
    "counter",
    "Restore-and-checks of the backup archive, by result (passed, failed, source_moved, expired).",
);

impl CheckCounters {
    fn count(&self, check: &Check) {
        let counter = match check {
            Check::Passed { .. } => &self.passed,
            Check::Failed { .. } => &self.failed,
            Check::SourceMoved { .. } => &self.source_moved,
            Check::Expired { .. } => &self.expired,
            Check::Empty => return,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

/// The newest second the archive under `prefix` holds whole: the last second its through point
/// covers, none before its first mark.
pub fn newest_second(
    blobs: &dyn Blobs,
    prefix: &str,
    until: Instant,
) -> anyhow::Result<Option<u64>> {
    let mut segments = Vec::new();
    for (key, _) in list_all(blobs, &format!("{prefix}/log/"), until)? {
        segments.push(SegmentKey::parse(prefix, &key)?);
    }
    let marks: Vec<ThroughKey> = through_marks(blobs, prefix, until)?
        .into_iter()
        .map(|(m, _)| m)
        .collect();
    plan::raise_through(&mut segments, &marks);
    // The log once over, as a restore reads it (T-T1.12.M6).
    let segments = plan::covering(&segments);
    let Some(first) = segments
        .iter()
        .find_map(|s| s.marks)
        .map(|(first, _)| first)
    else {
        return Ok(None);
    };
    let through = segments.iter().map(|s| s.through).max().unwrap_or(0);
    // A second `s` is whole once the archive is whole through its end, `s * SECOND_NS`.
    let newest = through / SECOND_NS;
    Ok(if newest >= second_of(first) {
        Some(newest)
    } else {
        None
    })
}

/// Whether a check is due: the archive's cadence sets it, never a timer. One is due when a base
/// landed since the last check (each base is checked once), or the log shipped since the last check
/// outweighs the log that check replayed (the log a restore replays is checked as fast as it grows).
#[must_use]
pub fn check_due(last: Option<(u64, u64)>, newest_base: u64, log_since: u64) -> bool {
    match last {
        None => true,
        Some((base, replayed)) => base != newest_base || log_since >= replayed.max(1),
    }
}

/// Restores the newest whole second under `prefix` into a new file at `scratch` and checks it:
/// `PRAGMA integrity_check`, then its rows against `source(second)` (the source's digest at that
/// second; none when it can no longer say). Counts the outcome. The scratch file is the caller's to
/// remove (it is kept on a failure to look into).
pub fn restore_and_check(
    blobs: &dyn Blobs,
    prefix: &str,
    scratch: &Path,
    source: &dyn Fn(u64) -> anyhow::Result<Option<RowDigest>>,
    counters: &CheckCounters,
    until: Instant,
) -> anyhow::Result<Check> {
    let Some(second) = newest_second(blobs, prefix, until)? else {
        return Ok(Check::Empty);
    };
    restore_and_check_at(
        blobs,
        prefix,
        second,
        scratch,
        source,
        counters,
        &mut Unpaced,
        until,
    )
}

/// [`restore_and_check`] at `second`, one the archive holds whole, step by step as `pace` lets it
/// (T-T1.12.C7): the backup's check thread checks the second its source's quiet snapshot holds
/// ([`super::Marks::quiet_second`]). A pace's [`super::Stopped`] stops it, refused and not counted.
#[allow(clippy::too_many_arguments)]
pub fn restore_and_check_at(
    blobs: &dyn Blobs,
    prefix: &str,
    second: u64,
    scratch: &Path,
    source: &dyn Fn(u64) -> anyhow::Result<Option<RowDigest>>,
    counters: &CheckCounters,
    pace: &mut dyn Pace,
    until: Instant,
) -> anyhow::Result<Check> {
    let check = check_second(blobs, prefix, second, scratch, source, pace, until)?;
    counters.count(&check);
    match &check {
        Check::Failed { second, why } => tracing::error!(
            target: "amphora.backup.check",
            event = "failed",
            second,
            why = ?why,
        ),
        other => tracing::info!(target: "amphora.backup.check", event = "checked", check = ?other),
    }
    Ok(check)
}

fn check_second(
    blobs: &dyn Blobs,
    prefix: &str,
    second: u64,
    scratch: &Path,
    source: &dyn Fn(u64) -> anyhow::Result<Option<RowDigest>>,
    pace: &mut dyn Pace,
    until: Instant,
) -> anyhow::Result<Check> {
    let restored = match restore_paced(blobs, prefix, second, scratch, pace, until) {
        Ok(restored) => restored,
        Err(e) if e.downcast_ref::<super::Stopped>().is_some() => return Err(e),
        Err(e) if matches!(e.downcast_ref(), Some(plan::Unrestorable::BeforeArchive(_))) => {
            return Ok(Check::Expired { second });
        }
        Err(e) => {
            return Ok(Check::Failed {
                second,
                why: CheckFailure::Restore(format!("{e:#}")),
            });
        }
    };
    let db = Db::open_existing_sharing(scratch, &crate::db::Sharing::Off)?;
    db.disable_inline_reads();
    let mut integrity = Vec::new();
    // The engine's check runs as one statement; each of its waits on the device is a step's end,
    // so it pauses inside the statement as the pace says (T-T1.12.C8).
    pace.ready()?;
    db.query_between(
        "PRAGMA integrity_check",
        |r| {
            integrity.push(r.get::<String>(0).map_err(|e| anyhow::anyhow!("{e}"))?);
            Ok(integrity.len() < 8)
        },
        &mut || {
            pace.done(None, 0)?;
            pace.ready()
        },
    )?;
    pace.done(None, 0)?;
    if integrity.as_slice() != ["ok"] {
        return Ok(Check::Failed {
            second,
            why: CheckFailure::Integrity(integrity.join("; ")),
        });
    }
    let got = digest_paced(&db, pace, step_bytes(blobs))?;
    let Some(want) = source(second)? else {
        return Ok(Check::SourceMoved { second });
    };
    if got != want {
        return Ok(Check::Failed {
            second,
            why: CheckFailure::Rows {
                source: want,
                restored: got,
            },
        });
    }
    Ok(Check::Passed { second, restored })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-T1.12.C2: a check is due at the first, at each new base, and once the log since the last
    /// check outweighs what it replayed; not otherwise.
    #[test]
    fn t_t1_12_c2_check_follows_the_archive_cadence() {
        assert!(check_due(None, 1, 0));
        assert!(check_due(Some((1, 100)), 2, 0));
        assert!(!check_due(Some((1, 100)), 1, 99));
        assert!(check_due(Some((1, 100)), 1, 100));
        assert!(check_due(Some((1, 0)), 1, 1));
        assert!(!check_due(Some((1, 0)), 1, 0));
    }
}
