//! T-G5.4.1: writes between scans, each prototype under a random differential against a plain
//! scan of a model table: no row missed, none seen stale.

use super::*;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::collections::BTreeMap;

const SEEDS: u64 = 64;
const OPS: usize = 600;

/// The model table: rowid -> the filtered column (`None`: not an integer).
type Table = BTreeMap<i64, Option<i64>>;

fn value_for(rng: &mut ChaCha8Rng, rowid: i64) -> Option<i64> {
    if rng.random_bool(0.05) {
        None
    } else {
        // Mostly following the rowid (an append-ordered column, which zone maps can skip), with
        // noise and some values from anywhere.
        if rng.random_bool(0.9) {
            Some(rowid / 4 + rng.random_range(-8..=8))
        } else {
            Some(rng.random_range(-1_000..=1_000))
        }
    }
}

fn initial_table(rng: &mut ChaCha8Rng) -> Table {
    let mut table = Table::new();
    let mut rowid = 0;
    for _ in 0..rng.random_range(0..400) {
        rowid += rng.random_range(1..4);
        let value = value_for(rng, rowid);
        table.insert(rowid, value);
    }
    table
}

/// The plain scan: every row whose integer value the filter's bounds hold.
fn plain(table: &Table, lo: i64, hi: i64) -> Vec<i64> {
    table
        .iter()
        .filter(|(_, value)| value.is_some_and(|v| (lo..=hi).contains(&v)))
        .map(|(rowid, _)| *rowid)
        .collect()
}

/// A random write, applied to the model and returned for the hook.
fn random_write(rng: &mut ChaCha8Rng, table: &mut Table) -> RowWrite {
    let last = table.keys().next_back().copied().unwrap_or(0);
    let pick = rng.random_range(0..10);
    if table.is_empty() || pick < 4 {
        // Insert: mostly appended, sometimes into a gap or below every rowid.
        let rowid = match rng.random_range(0..4) {
            0 => {
                let r = rng.random_range(-50..=last + 1);
                if table.contains_key(&r) {
                    last + 1
                } else {
                    r
                }
            }
            _ => last + rng.random_range(1..4),
        };
        let value = value_for(rng, rowid);
        table.insert(rowid, value);
        return RowWrite::Insert { rowid, value };
    }
    let nth = rng.random_range(0..table.len());
    let (&rowid, &old) = table.iter().nth(nth).unwrap();
    if pick < 7 {
        table.remove(&rowid);
        RowWrite::Delete { rowid, old }
    } else {
        let new = value_for(rng, rowid);
        table.insert(rowid, new);
        RowWrite::Update { rowid, old, new }
    }
}

fn random_filter(rng: &mut ChaCha8Rng, table: &Table, repeats: &mut Vec<(i64, i64)>) -> (i64, i64) {
    // Filters repeat (what cracking feeds on), and the bounds reach both ends of i64.
    if !repeats.is_empty() && rng.random_bool(0.5) {
        return repeats[rng.random_range(0..repeats.len())];
    }
    let top = table.keys().next_back().copied().unwrap_or(0) / 4 + 10;
    let filter = match rng.random_range(0..10) {
        0 => (i64::MIN, rng.random_range(-20..top)),
        1 => (rng.random_range(-20..top), i64::MAX),
        2 => (5, 4),
        _ => {
            let lo = rng.random_range(-20..top);
            (lo, lo + rng.random_range(0..40))
        }
    };
    repeats.push(filter);
    filter
}

/// Do: build zone maps over random leaf runs, then interleave random inserts, deletes and updates
/// with filters. Expect: reading only the candidate ranges finds exactly the plain scan's rows,
/// every live row stays inside its zone's summary, and some zones were skipped (non-vacuity).
#[test]
fn t_g5_4_1_zone_maps_writes_between_scans_match_a_plain_scan() {
    let mut skipped = 0;
    let mut splits = 0;
    for seed in 0..SEEDS {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut table = initial_table(&mut rng);
        let mut builder = ZoneMapBuilder::new();
        for (&rowid, &value) in &table {
            builder.row(rowid, value).unwrap();
            if rng.random_bool(0.04) {
                builder.end_run();
            }
        }
        let mut skipping = Skipping::ZoneMaps(builder.finish());
        let mut repeats = Vec::new();
        for _ in 0..OPS {
            let write = random_write(&mut rng, &mut table);
            skipping.on_write(write).unwrap();
            if rng.random_bool(0.3) {
                let (lo, hi) = random_filter(&mut rng, &table, &mut repeats);
                let Candidates::Ranges(ranges) = skipping.candidates(lo, hi).unwrap() else {
                    panic!("zone maps name ranges");
                };
                let mut found = Vec::new();
                for range in &ranges {
                    found.extend(plain_range(&table, range.first, range.last, lo, hi));
                }
                assert_eq!(
                    found,
                    plain(&table, lo, hi),
                    "seed {seed}, filter {lo}..={hi}"
                );
            }
            let Skipping::ZoneMaps(zones) = &skipping else {
                unreachable!()
            };
            for (&rowid, &value) in &table {
                assert!(
                    zones.covers(rowid, value),
                    "seed {seed}: row {rowid} = {value:?} outside its zone"
                );
            }
        }
        let Skipping::ZoneMaps(zones) = &skipping else {
            unreachable!()
        };
        skipped += zones.stats().zones_skipped;
        splits += zones.stats().zones_split;
    }
    assert!(skipped > 0, "no zone was ever skipped");
    assert!(splits > 0, "no appended tail ever started a zone");
}

fn plain_range(table: &Table, first: i64, last: i64, lo: i64, hi: i64) -> Vec<i64> {
    table
        .range(first..=last)
        .filter(|(_, value)| value.is_some_and(|v| (lo..=hi).contains(&v)))
        .map(|(rowid, _)| *rowid)
        .collect()
}

/// Do: crack a random table's column with repeated filters, interleaving random inserts, deletes
/// and updates. Expect: the candidates, filtered, are the plain scan's rows; each matching entry the
/// cracker holds carries the row's current value (none stale); and repeated filters found their
/// bounds cracked (non-vacuity: fewer partitions than queries asked).
#[test]
fn t_g5_4_1_cracking_writes_between_scans_match_a_plain_scan() {
    let mut cracks = 0;
    let mut queries = 0;
    let mut moved = 0;
    for seed in 0..SEEDS {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut table = initial_table(&mut rng);
        let mut skipping =
            Skipping::Cracking(CrackerColumn::new(table.iter().map(|(r, v)| (*r, *v))));
        let mut repeats = Vec::new();
        for _ in 0..OPS {
            let write = random_write(&mut rng, &mut table);
            skipping.on_write(write).unwrap();
            if rng.random_bool(0.3) {
                let (lo, hi) = random_filter(&mut rng, &table, &mut repeats);
                let Candidates::Rowids(rowids) = skipping.candidates(lo, hi).unwrap() else {
                    panic!("cracking names rowids");
                };
                let found: Vec<i64> = rowids
                    .iter()
                    .filter(|rowid| table[*rowid].is_some_and(|v| (lo..=hi).contains(&v)))
                    .copied()
                    .collect();
                let expected = plain(&table, lo, hi);
                assert_eq!(found, expected, "seed {seed}, filter {lo}..={hi}");
                let Skipping::Cracking(cracker) = &mut skipping else {
                    unreachable!()
                };
                let mut entries = cracker.matching_entries(lo, hi).unwrap();
                for (value, rowid) in &entries {
                    assert_eq!(
                        table.get(rowid),
                        Some(&Some(*value)),
                        "seed {seed}: stale entry for row {rowid}"
                    );
                }
                entries.sort_by_key(|(_, rowid)| *rowid);
                let rowids: Vec<i64> = entries.iter().map(|(_, rowid)| *rowid).collect();
                assert_eq!(rowids, expected, "seed {seed}: entries for {lo}..={hi}");
            }
        }
        let Skipping::Cracking(cracker) = &mut skipping else {
            unreachable!()
        };
        // Every row the table holds is in the column once: integers as entries, others apart.
        let mut held: Vec<i64> = cracker
            .matching_entries(i64::MIN, i64::MAX)
            .unwrap()
            .iter()
            .map(|(_, r)| *r)
            .collect();
        held.extend(cracker.others());
        held.sort_unstable();
        assert_eq!(
            held,
            table.keys().copied().collect::<Vec<_>>(),
            "seed {seed}"
        );
        cracks += cracker.stats().cracks;
        queries += cracker.stats().queries;
        moved += cracker.stats().entries_moved;
    }
    assert!(
        cracks > 0 && cracks < 2 * queries,
        "cracks {cracks} for {queries} queries"
    );
    assert!(moved > 0, "no write ever rippled");
}

/// Do: hand the hook writes with the switch off, and ask for candidates. Expect: nothing kept,
/// every row a candidate.
#[test]
fn t_g5_4_1_switch_off_keeps_nothing() {
    let mut skipping = Skipping::Off;
    skipping
        .on_write(RowWrite::Insert {
            rowid: 1,
            value: Some(1),
        })
        .unwrap();
    assert_eq!(skipping.candidates(0, 0).unwrap(), Candidates::All);
}

/// Do: remove a row neither summary holds; build from rowids out of order. Expect: typed refusals.
#[test]
fn t_g5_4_1_disagreements_are_refused() {
    let mut zones = ZoneMapBuilder::new().finish();
    assert_eq!(
        zones.on_write(RowWrite::Delete {
            rowid: 3,
            old: Some(1)
        }),
        Err(SkipError::MissingRow {
            rowid: 3,
            value: Some(1)
        })
    );
    let mut cracker = CrackerColumn::new([(1, Some(5))]);
    assert_eq!(
        cracker.on_write(RowWrite::Delete {
            rowid: 1,
            old: Some(6)
        }),
        Err(SkipError::MissingRow {
            rowid: 1,
            value: Some(6)
        })
    );
    assert_eq!(
        cracker.on_write(RowWrite::Delete {
            rowid: 2,
            old: None
        }),
        Err(SkipError::MissingRow {
            rowid: 2,
            value: None
        })
    );
    let mut builder = ZoneMapBuilder::new();
    builder.row(5, Some(1)).unwrap();
    assert_eq!(
        builder.row(5, Some(2)),
        Err(SkipError::RowidOrder {
            previous: 5,
            rowid: 5
        })
    );
}

/// A real table's leaves (G5 step 4: run sizes from measured data): an in-memory database whose
/// rows span many leaves.
mod leaves {
    use super::super::{zone_map_of_table, LeafBuildError, RowWrite, ZoneMap};
    use crate::io::MemoryIO;
    use crate::sync::Arc;
    use crate::{Connection, Database, SqliteDialect, Value};
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    fn open() -> Arc<Connection> {
        let io = Arc::new(MemoryIO::new());
        let db = Database::open_file(io, ":memory:", Arc::new(SqliteDialect)).unwrap();
        db.connect(crate::ConnectOpts::new()).unwrap()
    }

    fn query(conn: &Arc<Connection>, sql: &str) -> Vec<Vec<Value>> {
        let mut stmt = conn.prepare(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
        stmt.run_collect_rows()
            .unwrap_or_else(|e| panic!("{sql}: {e}"))
    }

    fn rowids(conn: &Arc<Connection>, sql: &str) -> Vec<i64> {
        query(conn, sql)
            .into_iter()
            .map(|row| match row.first() {
                Some(Value::Numeric(crate::Numeric::Integer(rowid))) => *rowid,
                other => panic!("{sql}: rowid {other:?}"),
            })
            .collect()
    }

    fn build(conn: &Arc<Connection>, table: &str) -> ZoneMap {
        zone_map_of_table(conn, table, "a").unwrap()
    }

    /// A row's column `a` as the hook takes it (`None`: not an integer).
    fn value_of(conn: &Arc<Connection>, rowid: i64) -> Option<i64> {
        match query(conn, &format!("SELECT a FROM t WHERE rowid = {rowid}"))
            .first()
            .and_then(|row| row.first())
        {
            Some(Value::Numeric(crate::Numeric::Integer(a))) => Some(*a),
            _ => None,
        }
    }

    /// Do: build zone maps over two tables whose rows differ only in size. Expect: one zone per
    /// leaf, so the table with larger rows has fewer rows a zone; every row is in some zone.
    #[test]
    fn t_g5_4_1_zones_follow_the_tables_leaves() {
        let conn = open();
        for (table, pad) in [("small", 40), ("large", 400)] {
            conn.execute(format!("CREATE TABLE {table}(a INTEGER, pad BLOB)"))
                .unwrap();
            conn.execute(format!(
                "INSERT INTO {table} SELECT value, randomblob({pad}) FROM generate_series(1, 4000)"
            ))
            .unwrap();
        }
        let small = build(&conn, "small");
        let large = build(&conn, "large");
        assert_eq!(small.rows().unwrap(), 4000);
        assert_eq!(large.rows().unwrap(), 4000);
        assert!(small.zone_count() > 1, "small rows fill several leaves");
        assert!(
            large.zone_count() > small.zone_count(),
            "larger rows fill more leaves: {} against {}",
            large.zone_count(),
            small.zone_count()
        );
    }

    /// Do: ask for zone maps of what holds no stored column. Expect: typed refusals.
    #[test]
    fn t_g5_4_1_no_table_or_column_is_refused() {
        let conn = open();
        conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, a)")
            .unwrap();
        conn.execute("CREATE VIEW v AS SELECT a FROM t").unwrap();
        assert!(matches!(
            zone_map_of_table(&conn, "v", "a"),
            Err(LeafBuildError::NoTable { .. })
        ));
        assert!(matches!(
            zone_map_of_table(&conn, "t", "b"),
            Err(LeafBuildError::NoColumn { .. })
        ));
        assert!(matches!(
            zone_map_of_table(&conn, "t", "id"),
            Err(LeafBuildError::NoColumn { .. })
        ));
        assert_eq!(
            zone_map_of_table(&conn, "t", "a").unwrap().rows().unwrap(),
            0
        );
    }

    /// Do: zone maps built from a real table's leaves, SQL writes between scans fed to the hook,
    /// each filter read only from the candidate ranges. Expect: the same rows as a plain scan.
    #[test]
    fn t_g5_4_1_real_leaves_writes_between_scans_match_a_plain_scan() {
        let mut rng = ChaCha8Rng::seed_from_u64(54);
        let conn = open();
        conn.execute("CREATE TABLE t(a, pad BLOB)").unwrap();
        // A time-ordered column (it follows the rowid), some values not integers.
        conn.execute(
            "INSERT INTO t SELECT CASE WHEN value % 97 = 0 THEN 'x' ELSE value / 3 END, \
             randomblob(120) FROM generate_series(1, 3000)",
        )
        .unwrap();
        let mut zones = build(&conn, "t");
        let mut next = 3000i64;
        let mut skipped_any = false;
        for _ in 0..60 {
            for _ in 0..10 {
                let write = match rng.random_range(0..3) {
                    0 => {
                        next += 1;
                        let value = next / 3 + rng.random_range(-5..=5);
                        conn.execute(format!("INSERT INTO t VALUES ({value}, randomblob(120))"))
                            .unwrap();
                        RowWrite::Insert {
                            rowid: next,
                            value: Some(value),
                        }
                    }
                    pick => {
                        let rowid = rng.random_range(1..=next);
                        if query(&conn, &format!("SELECT 1 FROM t WHERE rowid = {rowid}"))
                            .is_empty()
                        {
                            continue;
                        }
                        let old = value_of(&conn, rowid);
                        if pick == 1 {
                            conn.execute(format!("DELETE FROM t WHERE rowid = {rowid}"))
                                .unwrap();
                            RowWrite::Delete { rowid, old }
                        } else {
                            let new = rng.random_range(-100..=1_200);
                            conn.execute(format!("UPDATE t SET a = {new} WHERE rowid = {rowid}"))
                                .unwrap();
                            RowWrite::Update {
                                rowid,
                                old,
                                new: Some(new),
                            }
                        }
                    }
                };
                zones.on_write(write).unwrap();
            }
            let lo = rng.random_range(-50..=1_100);
            let hi = lo + rng.random_range(0..60);
            let filter = format!("typeof(a) = 'integer' AND a BETWEEN {lo} AND {hi}");
            let plain = rowids(
                &conn,
                &format!("SELECT rowid FROM t WHERE {filter} ORDER BY rowid"),
            );
            let ranges = zones.candidate_ranges(lo, hi);
            let mut found = Vec::new();
            for range in &ranges {
                found.extend(rowids(
                    &conn,
                    &format!(
                        "SELECT rowid FROM t WHERE rowid BETWEEN {} AND {} AND {filter} ORDER BY rowid",
                        range.first, range.last
                    ),
                ));
            }
            assert_eq!(found, plain, "filter {lo}..={hi}, ranges {ranges:?}");
            skipped_any |= zones.stats().zones_skipped > 0;
        }
        assert!(skipped_any, "the zones skipped some leaves");
    }
}

/// G5 step 4 wired in (T-G5.4.2): the Insert and Delete opcodes feed the database's summaries, and
/// a reader skips only at the snapshot a summary holds.
mod wired {
    use super::super::{
        set_skipping, skipping_candidates, skipping_counts, Candidates, SkipKind, SkipRefusal,
        Skipping,
    };
    use crate::io::MemoryIO;
    use crate::sync::Arc;
    use crate::{Connection, Database, SqliteDialect, Value};
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    fn open() -> (Arc<Database>, Arc<Connection>) {
        let io = Arc::new(MemoryIO::new());
        let db = Database::open_file(io, ":memory:", Arc::new(SqliteDialect)).unwrap();
        let conn = db.connect(crate::ConnectOpts::new()).unwrap();
        (db, conn)
    }

    fn rowids(conn: &Arc<Connection>, sql: &str) -> Vec<i64> {
        let mut stmt = conn.prepare(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
        stmt.run_collect_rows()
            .unwrap_or_else(|e| panic!("{sql}: {e}"))
            .into_iter()
            .map(|row| match row.first() {
                Some(Value::Numeric(crate::Numeric::Integer(rowid))) => *rowid,
                other => panic!("{sql}: rowid {other:?}"),
            })
            .collect()
    }

    /// The rows a filter's candidates let through, read through SQL: what a scan that skips
    /// answers.
    fn skipped_answer(
        conn: &Arc<Connection>,
        candidates: &Candidates,
        lo: i64,
        hi: i64,
    ) -> Vec<i64> {
        let filter = format!("a BETWEEN {lo} AND {hi}");
        match candidates {
            Candidates::All => rowids(conn, &format!("SELECT rowid FROM t WHERE {filter} ORDER BY rowid")),
            Candidates::Ranges(ranges) => ranges
                .iter()
                .flat_map(|r| {
                    rowids(
                        conn,
                        &format!(
                            "SELECT rowid FROM t WHERE rowid BETWEEN {} AND {} AND {filter} ORDER BY rowid",
                            r.first, r.last
                        ),
                    )
                })
                .collect(),
            Candidates::Rowids(ids) => ids
                .iter()
                .flat_map(|id| rowids(conn, &format!("SELECT rowid FROM t WHERE rowid = {id} AND {filter}")))
                .collect(),
        }
    }

    fn fill(conn: &Arc<Connection>, rows: i64) {
        conn.execute("CREATE TABLE t (a INTEGER, pad TEXT)")
            .unwrap();
        conn.execute("BEGIN").unwrap();
        for i in 0..rows {
            conn.execute(format!("INSERT INTO t VALUES ({i}, '{}')", "x".repeat(40)))
                .unwrap();
        }
        conn.execute("COMMIT").unwrap();
    }

    fn random_write(rng: &mut ChaCha8Rng, conn: &Arc<Connection>, rows: i64) {
        let a = rng.random_range(0..rows);
        let sql = match rng.random_range(0..5) {
            0 => format!("INSERT INTO t VALUES ({a}, 'new')"),
            1 => "INSERT INTO t VALUES (NULL, 'null')".to_string(),
            2 => format!(
                "UPDATE t SET a = {} WHERE rowid = {}",
                rng.random_range(0..rows),
                rng.random_range(1..rows)
            ),
            3 => format!(
                "UPDATE t SET a = 'text' WHERE rowid = {}",
                rng.random_range(1..rows)
            ),
            _ => format!("DELETE FROM t WHERE rowid = {}", rng.random_range(1..rows)),
        };
        conn.execute(sql).unwrap();
    }

    fn writes_between_scans(kind: SkipKind) {
        let rows = 3000;
        for seed in 0..4u64 {
            let mut rng = ChaCha8Rng::seed_from_u64(seed);
            let (db, conn) = open();
            fill(&conn, rows);
            set_skipping(&conn, "t", "a", Some(kind)).unwrap();
            for _ in 0..30 {
                for _ in 0..rng.random_range(0..20) {
                    random_write(&mut rng, &conn, rows);
                }
                let lo = rng.random_range(-10..rows);
                let hi = lo.saturating_add(rng.random_range(0..rows / 10));
                let plain = rowids(
                    &conn,
                    &format!("SELECT rowid FROM t WHERE a BETWEEN {lo} AND {hi} ORDER BY rowid"),
                );
                let candidates = skipping_candidates(&conn, "t", lo, hi).unwrap();
                let mut skipped = skipped_answer(&conn, &candidates, lo, hi);
                skipped.sort_unstable();
                assert_eq!(skipped, plain, "seed {seed} {kind:?} filter {lo}..={hi}");
            }
            let counts = skipping_counts(&conn, "t").unwrap();
            // Each round asks twice: its SQL scan (which now skips, T-G5.4.3) and the direct question.
            assert!(counts.writes > 0 && counts.consulted == 60, "{counts:?}");
            let root = conn
                .with_schema_mut(|s| s.get_btree_table("t"))
                .unwrap()
                .unwrap()
                .root_page;
            let skipped_any = db.skipping.with_summary(root, |s| match s {
                Skipping::ZoneMaps(z) => z.stats().zones_skipped > 0,
                Skipping::Cracking(c) => c.crack_count() > 0,
                Skipping::Measured(m) => {
                    m.zone_map().is_some_and(|z| z.stats().zones_skipped > 0)
                        || m.cracker().is_some_and(|c| c.crack_count() > 0)
                }
                Skipping::Off => false,
            });
            assert_eq!(
                skipped_any,
                Some(true),
                "seed {seed} {kind:?}: nothing skipped"
            );
        }
    }

    #[test]
    fn t_g5_4_2_zone_maps_sql_writes_between_scans_answer_as_plain() {
        writes_between_scans(SkipKind::ZoneMaps);
    }

    #[test]
    fn t_g5_4_2_cracking_sql_writes_between_scans_answer_as_plain() {
        writes_between_scans(SkipKind::Cracking);
    }

    #[test]
    fn t_sku_2_measured_sql_writes_between_scans_answer_as_plain() {
        writes_between_scans(SkipKind::Measured);
    }

    /// The candidates `conn` gets in a read transaction of its own: its snapshot the newest
    /// commit (outside a transaction a connection's snapshot is its last one).
    fn fresh_candidates(
        conn: &Arc<Connection>,
        lo: i64,
        hi: i64,
    ) -> Result<Candidates, SkipRefusal> {
        conn.execute("BEGIN").unwrap();
        rowids(conn, "SELECT rowid FROM t WHERE rowid = 1");
        let candidates = skipping_candidates(conn, "t", lo, hi);
        conn.execute("COMMIT").unwrap();
        candidates
    }

    fn older_snapshot_refused(kind: SkipKind) {
        let (db, writer) = open();
        fill(&writer, 2000);
        set_skipping(&writer, "t", "a", Some(kind)).unwrap();
        let reader = db.connect(crate::ConnectOpts::new()).unwrap();
        // The reader's snapshot is taken, then the writer deletes and commits.
        reader.execute("BEGIN").unwrap();
        let before = rowids(
            &reader,
            "SELECT rowid FROM t WHERE a BETWEEN 0 AND 99 ORDER BY rowid",
        );
        writer.execute("DELETE FROM t WHERE a < 100").unwrap();
        assert_eq!(
            skipping_candidates(&reader, "t", 0, 99),
            Err(SkipRefusal::OlderSnapshot)
        );
        // Its plain scan still sees every row of its snapshot.
        assert_eq!(
            rowids(
                &reader,
                "SELECT rowid FROM t WHERE a BETWEEN 0 AND 99 ORDER BY rowid"
            ),
            before
        );
        reader.execute("COMMIT").unwrap();
        assert!(fresh_candidates(&reader, 0, 99).is_ok());
        // An open writer's rows are the summary's but not another reader's.
        writer.execute("BEGIN").unwrap();
        writer.execute("INSERT INTO t VALUES (50, 'open')").unwrap();
        assert_eq!(
            skipping_candidates(&reader, "t", 0, 99),
            Err(SkipRefusal::OlderSnapshot)
        );
        let own = skipping_candidates(&writer, "t", 0, 99).unwrap();
        assert_eq!(skipped_answer(&writer, &own, 0, 99).len(), 1);
        // Rolled back, the summary is ahead of the table: stale until built again.
        writer.execute("ROLLBACK").unwrap();
        assert_eq!(
            skipping_candidates(&reader, "t", 0, 99),
            Err(SkipRefusal::Stale)
        );
        set_skipping(&writer, "t", "a", Some(kind)).unwrap();
        let fresh = fresh_candidates(&reader, 0, 99).unwrap();
        assert!(skipped_answer(&reader, &fresh, 0, 99).is_empty());
        let counts = skipping_counts(&reader, "t").unwrap();
        assert_eq!(counts.refused_older, 0, "rebuilt: counts start again");
    }

    #[test]
    fn t_g5_4_2_zone_maps_older_snapshot_never_skips_stale() {
        older_snapshot_refused(SkipKind::ZoneMaps);
    }

    #[test]
    fn t_g5_4_2_cracking_older_snapshot_never_skips_stale() {
        older_snapshot_refused(SkipKind::Cracking);
    }

    #[test]
    fn t_sku_2_measured_older_snapshot_never_skips_stale() {
        older_snapshot_refused(SkipKind::Measured);
    }
}

/// G5 step 4 wired into the scan (T-G5.4.3): a SQL filter's scan skips the rows its summary
/// leaves out (the statement's rows-read counter shows it), answering exactly as the same SQL on a
/// database that keeps no summary, with random inserts, updates and deletes between queries.
mod scans {
    use super::super::{set_skipping, skipping_counts, SkipKind, Skipping};
    use crate::io::MemoryIO;
    use crate::sync::Arc;
    use crate::{Connection, Database, SqliteDialect};
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    fn open() -> (Arc<Database>, Arc<Connection>) {
        let io = Arc::new(MemoryIO::new());
        let db = Database::open_file(io, ":memory:", Arc::new(SqliteDialect)).unwrap();
        let conn = db.connect(crate::ConnectOpts::new()).unwrap();
        (db, conn)
    }

    /// A query's rows, printed, and the rows its scan read (the statement's counter).
    fn query(conn: &Arc<Connection>, sql: &str) -> (Vec<String>, u64) {
        let mut stmt = conn.prepare(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
        let rows = stmt
            .run_collect_rows()
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
        let rows = rows.into_iter().map(|row| format!("{row:?}")).collect();
        (rows, stmt.metrics().rows_read)
    }

    fn fill(conns: [&Arc<Connection>; 2], rows: i64) {
        for conn in conns {
            conn.execute("CREATE TABLE t (a INTEGER, b INTEGER, pad TEXT)")
                .unwrap();
            conn.execute("BEGIN").unwrap();
            for i in 0..rows {
                let sql = format!(
                    "INSERT INTO t VALUES ({i}, {}, '{}')",
                    i % 7,
                    "x".repeat(40)
                );
                conn.execute(sql).unwrap();
            }
            conn.execute("COMMIT").unwrap();
        }
    }

    fn random_write(rng: &mut ChaCha8Rng, rows: i64) -> String {
        let a = rng.random_range(0..rows);
        let rowid = rng.random_range(1..rows);
        match rng.random_range(0..6) {
            0 => format!("INSERT INTO t VALUES ({a}, 1, 'new')"),
            1 => "INSERT INTO t VALUES (NULL, 2, 'null')".to_string(),
            2 => format!("UPDATE t SET a = {a} WHERE rowid = {rowid}"),
            3 => format!("UPDATE t SET a = 'text' WHERE rowid = {rowid}"),
            4 => format!("UPDATE t SET b = {a} WHERE rowid = {rowid}"),
            _ => format!("DELETE FROM t WHERE rowid = {rowid}"),
        }
    }

    fn root_of(conn: &Arc<Connection>) -> i64 {
        conn.with_schema_mut(|s| s.get_btree_table("t"))
            .unwrap()
            .unwrap()
            .root_page
    }

    fn filters_skip_and_answer_as_unswitched(kind: SkipKind) {
        let rows = 4000;
        let queries = 24;
        for seed in 0..4u64 {
            let mut rng = ChaCha8Rng::seed_from_u64(seed);
            let (db, on) = open();
            let (_off_db, off) = open();
            // Every other seed reads leaves by column batches (QT.7's switch), on both databases.
            let vectorized = seed % 2 == 1;
            on.set_vectorized_scans(vectorized);
            off.set_vectorized_scans(vectorized);
            fill([&on, &off], rows);
            set_skipping(&on, "t", "a", Some(kind)).unwrap();
            let (mut read_on, mut read_off) = (0u64, 0u64);
            for round in 0..queries {
                for _ in 0..rng.random_range(0..15) {
                    let sql = random_write(&mut rng, rows);
                    on.execute(&sql).unwrap();
                    off.execute(&sql).unwrap();
                }
                let lo = rng.random_range(-10..rows);
                let hi = lo.saturating_add(rng.random_range(0..rows / 20));
                let filter = match round % 4 {
                    0 => format!("a BETWEEN {lo} AND {hi}"),
                    1 => format!("a >= {lo} AND a <= {hi} AND b <> 3"),
                    2 => format!("{lo} <= a AND a < {hi}"),
                    _ => format!("a = {lo}"),
                };
                let sql = format!("SELECT rowid, a, b FROM t WHERE {filter} ORDER BY rowid");
                let (answer_on, r_on) = query(&on, &sql);
                let (answer_off, r_off) = query(&off, &sql);
                assert_eq!(
                    answer_on, answer_off,
                    "seed {seed} {kind:?} vectorized {vectorized}: {sql}"
                );
                read_on += r_on;
                read_off += r_off;
            }
            let counts = skipping_counts(&on, "t").unwrap();
            assert_eq!(
                counts.consulted, queries,
                "seed {seed} {kind:?}: {counts:?}"
            );
            assert!(
                read_on < read_off,
                "seed {seed} {kind:?}: read {read_on} rows skipping, {read_off} without"
            );
            let skipped = db.skipping.with_summary(root_of(&on), |s| match s {
                Skipping::ZoneMaps(z) => z.stats().zones_skipped > 0,
                Skipping::Cracking(c) => c.crack_count() > 0,
                Skipping::Measured(m) => {
                    m.zone_map().is_some_and(|z| z.stats().zones_skipped > 0)
                        || m.cracker().is_some_and(|c| c.crack_count() > 0)
                }
                Skipping::Off => false,
            });
            assert_eq!(skipped, Some(true), "seed {seed} {kind:?}: nothing skipped");
        }
    }

    #[test]
    fn t_g5_4_3_zone_maps_sql_filter_skips_rows_and_answers_as_unswitched() {
        filters_skip_and_answer_as_unswitched(SkipKind::ZoneMaps);
    }

    #[test]
    fn t_g5_4_3_cracking_sql_filter_skips_rows_and_answers_as_unswitched() {
        filters_skip_and_answer_as_unswitched(SkipKind::Cracking);
    }

    #[test]
    fn t_sku_2_measured_sql_filter_skips_rows_and_answers_as_unswitched() {
        filters_skip_and_answer_as_unswitched(SkipKind::Measured);
    }

    fn transactions_answer_as_unswitched(kind: SkipKind) {
        let (db, on) = open();
        let (_off_db, off) = open();
        fill([&on, &off], 3000);
        set_skipping(&on, "t", "a", Some(kind)).unwrap();
        let sql = "SELECT rowid, a FROM t WHERE a BETWEEN 100 AND 199 ORDER BY rowid";
        let both = |sql: &str| {
            on.execute(sql).unwrap();
            off.execute(sql).unwrap();
        };
        // The open writer reads its own writes through the summary.
        both("BEGIN");
        both("INSERT INTO t VALUES (150, 0, 'open')");
        both("DELETE FROM t WHERE a = 120");
        assert_eq!(query(&on, sql).0, query(&off, sql).0);
        assert_eq!(skipping_counts(&on, "t").unwrap().consulted, 1);
        // Rolled back, the summary is stale: the scan reads every row and answers right.
        both("ROLLBACK");
        assert_eq!(query(&on, sql).0, query(&off, sql).0);
        let counts = skipping_counts(&on, "t").unwrap();
        assert_eq!(
            (counts.consulted, counts.refused_stale),
            (1, 1),
            "{counts:?}"
        );
        // Built again, a reader at an older snapshot is refused and keeps its snapshot's rows.
        set_skipping(&on, "t", "a", Some(kind)).unwrap();
        let reader = db.connect(crate::ConnectOpts::new()).unwrap();
        reader.execute("BEGIN").unwrap();
        let before = query(&reader, sql).0;
        both("DELETE FROM t WHERE a BETWEEN 100 AND 149");
        assert_eq!(query(&reader, sql).0, before);
        reader.execute("COMMIT").unwrap();
        assert_eq!(query(&reader, sql).0, query(&off, sql).0);
        let counts = skipping_counts(&on, "t").unwrap();
        assert_eq!(
            (counts.consulted, counts.refused_older),
            (2, 1),
            "{counts:?}"
        );
    }

    #[test]
    fn t_g5_4_3_zone_maps_transactions_and_older_snapshots_answer_as_unswitched() {
        transactions_answer_as_unswitched(SkipKind::ZoneMaps);
    }

    #[test]
    fn t_g5_4_3_cracking_transactions_and_older_snapshots_answer_as_unswitched() {
        transactions_answer_as_unswitched(SkipKind::Cracking);
    }

    #[test]
    fn t_sku_2_measured_transactions_and_older_snapshots_answer_as_unswitched() {
        transactions_answer_as_unswitched(SkipKind::Measured);
    }

    /// One run of a prepared statement bound to `values`: its rows, printed, and the rows it read.
    /// Lane skip-4 (T-SKU.3): a SQL statement turns skipping on; answers equal the unskipped
    /// scan's under random writes, rows are skipped, and `off` drops the summary.
    #[test]
    fn t_sku_3_pragma_turns_skipping_on_and_answers_as_unswitched() {
        let rows = 4000;
        let queries = 24;
        for (seed, setting) in [
            (0u64, "t.a"),
            (1, "t.a measured"),
            (2, "t.a ZONE_MAPS"),
            (3, "t.a cracking"),
            (4, "t.a capped"),
        ] {
            let mut rng = ChaCha8Rng::seed_from_u64(seed);
            let (db, on) = open();
            let (_off_db, off) = open();
            fill([&on, &off], rows);
            on.execute(format!("PRAGMA skipping = '{setting}'"))
                .unwrap();
            let (mut read_on, mut read_off) = (0u64, 0u64);
            for _ in 0..queries {
                for _ in 0..rng.random_range(0..15) {
                    let sql = random_write(&mut rng, rows);
                    on.execute(&sql).unwrap();
                    off.execute(&sql).unwrap();
                }
                let lo = rng.random_range(-10..rows);
                let hi = lo.saturating_add(rng.random_range(0..rows / 20));
                let sql = format!(
                    "SELECT rowid, a, b FROM t WHERE a BETWEEN {lo} AND {hi} ORDER BY rowid"
                );
                let (answer_on, r_on) = query(&on, &sql);
                let (answer_off, r_off) = query(&off, &sql);
                assert_eq!(answer_on, answer_off, "seed {seed} '{setting}': {sql}");
                read_on += r_on;
                read_off += r_off;
            }
            assert!(
                read_on < read_off,
                "seed {seed} '{setting}': read {read_on} skipping, {read_off} without"
            );
            assert_eq!(
                skipping_counts(&on, "t").unwrap().consulted,
                queries,
                "seed {seed} '{setting}'"
            );
            on.execute("PRAGMA skipping = 't.a off'").unwrap();
            let dropped = db
                .skipping
                .with_summary(root_of(&on), |s| matches!(s, Skipping::Off));
            assert_ne!(dropped, Some(false), "seed {seed}: 'off' kept a summary");
        }
    }

    /// Lane skip-4 (T-SKU.3): a setting that names no stored column, or no method kiln knows, is
    /// refused with why, and nothing is kept.
    #[test]
    fn t_sku_3_pragma_refuses_what_it_cant_summarize() {
        let (_db, on) = open();
        let (_other_db, other) = open();
        fill([&on, &other], 10);
        for bad in [
            "t",
            "t.",
            ".a",
            "t.nope",
            "nope.a",
            "t.a sideways",
            "t.a measured again",
        ] {
            let err = on.execute(format!("PRAGMA skipping = '{bad}'"));
            assert!(err.is_err(), "'{bad}' was taken");
        }
        assert!(
            on.execute("PRAGMA skipping").is_err(),
            "skipping is set, not read"
        );
        assert!(
            skipping_counts(&on, "t").is_none(),
            "a refused setting kept a summary"
        );
    }

    /// Lane skip-5 (T-SKU.6): the setting acts when its statement runs, not when it is prepared. A
    /// `PRAGMA skipping` prepared on an empty table and run once the rows are in skips them
    /// (zone maps, which a summary built over no rows can't: its rows grow as one zone), and run
    /// again after `off` turns skipping back on; answers equal the unskipped database's.
    #[test]
    fn t_sku_6_setting_prepared_before_the_data_skips_once_it_runs() {
        let rows = 4000;
        let (_db, on) = open();
        let (_off_db, off) = open();
        for conn in [&on, &off] {
            conn.execute("CREATE TABLE t (a INTEGER, b INTEGER, pad TEXT)")
                .unwrap();
        }
        let mut setting = on.prepare("PRAGMA skipping = 't.a zone_maps'").unwrap();
        let mut turn_off = on.prepare("PRAGMA skipping = 't.a off'").unwrap();
        for conn in [&on, &off] {
            conn.execute("BEGIN").unwrap();
            for i in 0..rows {
                conn.execute(format!(
                    "INSERT INTO t VALUES ({i}, {}, '{}')",
                    i % 7,
                    "x".repeat(40)
                ))
                .unwrap();
            }
            conn.execute("COMMIT").unwrap();
        }
        let mut rng = ChaCha8Rng::seed_from_u64(6);
        for pass in 0..2 {
            setting.reset().unwrap();
            setting.run_collect_rows().unwrap();
            let (mut read_on, mut read_off) = (0u64, 0u64);
            for _ in 0..16 {
                let lo = rng.random_range(0..rows);
                let hi = lo + rng.random_range(0..rows / 20);
                let sql = format!(
                    "SELECT rowid, a, b FROM t WHERE a BETWEEN {lo} AND {hi} ORDER BY rowid"
                );
                let (answer_on, r_on) = query(&on, &sql);
                let (answer_off, r_off) = query(&off, &sql);
                assert_eq!(answer_on, answer_off, "pass {pass}: {sql}");
                read_on += r_on;
                read_off += r_off;
            }
            assert!(
                read_on < read_off,
                "pass {pass}: read {read_on} rows skipping, {read_off} without"
            );
            turn_off.reset().unwrap();
            turn_off.run_collect_rows().unwrap();
            assert!(
                skipping_counts(&on, "t").is_none(),
                "pass {pass}: 'off' kept a summary"
            );
        }
    }

    fn run_bound(stmt: &mut crate::Statement, values: &[crate::Value]) -> (Vec<String>, u64) {
        stmt.reset().unwrap();
        stmt.reset_metrics();
        for (i, value) in values.iter().enumerate() {
            let index = std::num::NonZero::new(i + 1).unwrap();
            stmt.bind_at(index, value.clone()).unwrap();
        }
        let rows = stmt.run_collect_rows().unwrap();
        let rows = rows.into_iter().map(|row| format!("{row:?}")).collect();
        (rows, stmt.metrics().rows_read)
    }

    /// Lane skip-usable (T-SKU.1): a prepared statement's parameters bound its scan. Prepared
    /// once and bound again before each run, between random writes, it skips and answers as the
    /// database without a summary; a parameter bound to a real gives no bound and still answers.
    fn prepared_parameters_skip(kind: SkipKind) {
        use crate::Value;
        let rows = 4000;
        let (_db, on) = open();
        let (_off_db, off) = open();
        fill([&on, &off], rows);
        set_skipping(&on, "t", "a", Some(kind)).unwrap();
        let sqls = [
            "SELECT rowid, a, b FROM t WHERE a BETWEEN ? AND ? ORDER BY rowid",
            "SELECT rowid, a, b FROM t WHERE a >= ?1 AND ?2 >= a AND b <> 3 ORDER BY rowid",
            "SELECT rowid, a, b FROM t WHERE a > :lo AND a < :hi ORDER BY rowid",
        ];
        let mut stmts: Vec<_> = sqls
            .iter()
            .map(|sql| (on.prepare(sql).unwrap(), off.prepare(sql).unwrap()))
            .collect();
        let mut rng = ChaCha8Rng::seed_from_u64(11);
        let (mut read_on, mut read_off) = (0u64, 0u64);
        let runs = 24u64;
        for round in 0..runs {
            for _ in 0..rng.random_range(0..15) {
                let sql = random_write(&mut rng, rows);
                on.execute(&sql).unwrap();
                off.execute(&sql).unwrap();
            }
            let lo = rng.random_range(-10..rows);
            let hi = lo.saturating_add(rng.random_range(0..rows / 20));
            let lo_value = if round % 6 == 5 {
                Value::from_f64(lo as f64 + 0.5)
            } else {
                Value::from_i64(lo)
            };
            let values = [lo_value, Value::from_i64(hi)];
            let at = usize::try_from(round).unwrap() % stmts.len();
            let (stmt_on, stmt_off) = &mut stmts[at];
            let (answer_on, r_on) = run_bound(stmt_on, &values);
            let (answer_off, r_off) = run_bound(stmt_off, &values);
            assert_eq!(
                answer_on, answer_off,
                "{kind:?} round {round}: {}",
                sqls[at]
            );
            read_on += r_on;
            read_off += r_off;
        }
        let counts = skipping_counts(&on, "t").unwrap();
        assert_eq!(counts.consulted, runs, "{kind:?}: {counts:?}");
        assert!(
            read_on < read_off,
            "{kind:?}: read {read_on} rows skipping, {read_off} without"
        );
    }

    /// Lane skip-10 (T-SKU.13, kiln goal 623adc1c). Do: one prepared statement for each bound
    /// shape on the skipping column (`BETWEEN ? AND ?`, `>= ?`, `<= ?`, `IN (?, ?)`, `= ?`), each
    /// prepared once; each round, random writes, then the round's statement bound and run, then
    /// re-bound to other values and run again with no write between (the summary is the same,
    /// only the parameters moved); some IN runs bind one value to a real far from the other.
    /// Expect: every run answers as the database without a summary (a bound kept from the earlier
    /// binding would leave out rows the new one matches), every run bound by integers consulted
    /// the summary and none with a real did, and each shape read fewer rows in all than the plain
    /// scan.
    fn every_bound_shape_reads_its_parameters(kind: SkipKind) {
        use crate::Value;
        let rows = 4000i64;
        let (_db, on) = open();
        let (_off_db, off) = open();
        fill([&on, &off], rows);
        set_skipping(&on, "t", "a", Some(kind)).unwrap();
        let sqls = [
            "SELECT rowid, a, b FROM t WHERE a BETWEEN ? AND ? ORDER BY rowid",
            "SELECT count(*), sum(b) FROM t WHERE a >= ?",
            "SELECT count(*), sum(b) FROM t WHERE a <= ?",
            "SELECT rowid, a, b FROM t WHERE a IN (?, ?) ORDER BY rowid",
            "SELECT rowid, a, b FROM t WHERE a = ? ORDER BY rowid",
        ];
        let mut stmts: Vec<_> = sqls
            .iter()
            .map(|sql| (on.prepare(sql).unwrap(), off.prepare(sql).unwrap()))
            .collect();
        let mut rng = ChaCha8Rng::seed_from_u64(13);
        let mut read = [(0u64, 0u64); 5];
        let (mut runs, mut unbounded) = (0u64, 0u64);
        for round in 0..40u64 {
            for _ in 0..rng.random_range(0..15) {
                let sql = random_write(&mut rng, rows);
                on.execute(&sql).unwrap();
                off.execute(&sql).unwrap();
            }
            let at = usize::try_from(round).unwrap() % sqls.len();
            for bind in 0..2u64 {
                let lo = rng.random_range(0..rows);
                let near = lo.saturating_add(rng.random_range(0..rows / 40));
                let values = match at {
                    0 => vec![Value::from_i64(lo), Value::from_i64(near)],
                    // A tail and a head of the values: up to a twentieth of the rows each.
                    1 => vec![Value::from_i64(rows - rng.random_range(0..rows / 20))],
                    2 => vec![Value::from_i64(rng.random_range(0..rows / 20))],
                    3 if (round / 5 + bind) % 3 == 2 => {
                        unbounded += 1;
                        vec![Value::from_i64(lo), Value::from_f64((rows - 1 - lo) as f64)]
                    }
                    3 => vec![Value::from_i64(lo), Value::from_i64(near)],
                    _ => vec![Value::from_i64(lo)],
                };
                let (stmt_on, stmt_off) = &mut stmts[at];
                let (answer_on, r_on) = run_bound(stmt_on, &values);
                let (answer_off, r_off) = run_bound(stmt_off, &values);
                assert_eq!(
                    answer_on, answer_off,
                    "{kind:?} round {round} bind {bind}: {} {values:?}",
                    sqls[at]
                );
                runs += 1;
                read[at].0 += r_on;
                read[at].1 += r_off;
            }
        }
        let counts = skipping_counts(&on, "t").unwrap();
        assert!(unbounded > 0, "no run bound an IN value to a real");
        assert_eq!(counts.consulted, runs - unbounded, "{kind:?}: {counts:?}");
        for (at, (r_on, r_off)) in read.iter().enumerate() {
            assert!(
                r_on < r_off,
                "{kind:?} {}: read {r_on} rows skipping, {r_off} without",
                sqls[at]
            );
        }
    }

    /// Lane skip-10 (T-SKU.13). Do: a hint with an IN list of a constant and two parameters,
    /// and a `>=` term on the same column, read under three bindings. Expect: the list bounds its
    /// column by its smallest and largest value, intersected with the term; a parameter bound to
    /// anything but an integer leaves the list unbounded (the term alone bounds); another column
    /// is unbounded.
    #[test]
    fn t_sku_13_an_in_list_bounds_by_its_values_as_bound() {
        use super::super::{BoundOp, BoundTerm, BoundValue, ColumnBounds, ListTerm, SkipHint};
        let p = |index: usize| BoundValue::Parameter(std::num::NonZeroUsize::new(index).unwrap());
        let hint = SkipHint {
            cursor_id: 0,
            terms: vec![BoundTerm {
                column: 1,
                op: BoundOp::GreaterEquals,
                value: BoundValue::Integer(20),
            }],
            lists: vec![ListTerm {
                column: 1,
                values: vec![BoundValue::Integer(50), p(1), p(2)],
            }],
        };
        let bound = |lo, hi| Some(ColumnBounds { column: 1, lo, hi });
        // Parameters ?1 and ?2 bound to `values` (`None`: not an integer).
        let bound_to = |values: [Option<i64>; 2]| {
            move |index: std::num::NonZeroUsize| values.get(index.get() - 1).copied().flatten()
        };
        assert_eq!(
            hint.bounds(1, bound_to([Some(10), Some(90)])),
            bound(20, 90)
        );
        assert_eq!(
            hint.bounds(1, bound_to([Some(70), Some(60)])),
            bound(50, 70)
        );
        assert_eq!(
            hint.bounds(1, bound_to([Some(70), None])),
            bound(20, i64::MAX)
        );
        assert_eq!(hint.bounds(0, |_| Some(1)), None);
    }

    #[test]
    fn t_sku_13_zone_maps_every_bound_shape_reads_its_parameters() {
        every_bound_shape_reads_its_parameters(SkipKind::ZoneMaps);
    }

    #[test]
    fn t_sku_13_cracking_every_bound_shape_reads_its_parameters() {
        every_bound_shape_reads_its_parameters(SkipKind::Cracking);
    }

    #[test]
    fn t_sku_13_measured_every_bound_shape_reads_its_parameters() {
        every_bound_shape_reads_its_parameters(SkipKind::Measured);
    }

    /// Lane skip-11 (T-SKU.14). Do: a hint with a `>= 20` term and two IN lists on one column
    /// (three values, and a constant and two parameters), read under several bindings. Expect:
    /// the shortest list whose values are all integers on this run is asked value by value, only
    /// the values within the column's bounds (the term and every list's span) kept, ascending and
    /// distinct; a list holding a value that isn't an integer is passed over; no list, or no value
    /// within the bounds, asks for the bounds' one span instead (`None`).
    #[test]
    fn t_sku_14_spans_ask_each_value_within_the_bounds() {
        use super::super::{BoundOp, BoundTerm, BoundValue, ListTerm, SkipHint};
        let p = |index: usize| BoundValue::Parameter(std::num::NonZeroUsize::new(index).unwrap());
        let int = BoundValue::Integer;
        let hint = |lists: Vec<Vec<BoundValue>>| SkipHint {
            cursor_id: 0,
            terms: vec![BoundTerm {
                column: 1,
                op: BoundOp::GreaterEquals,
                value: int(20),
            }],
            lists: lists
                .into_iter()
                .map(|values| ListTerm { column: 1, values })
                .collect(),
        };
        let bound_to = |values: [Option<i64>; 2]| {
            move |index: std::num::NonZeroUsize| values.get(index.get() - 1).copied().flatten()
        };
        let one = hint(vec![vec![int(50), p(1), p(2)]]);
        // 10 is below the term's 20; the parameters' 90 repeats.
        assert_eq!(
            one.spans(1, bound_to([Some(10), Some(90)])),
            Some(vec![(50, 50), (90, 90)])
        );
        assert_eq!(
            one.spans(1, bound_to([Some(90), Some(90)])),
            Some(vec![(50, 50), (90, 90)])
        );
        assert_eq!(one.spans(1, bound_to([Some(70), None])), None);
        assert_eq!(one.spans(0, bound_to([Some(70), Some(80)])), None);
        // Every value below the term: the bounds alone (a contradiction) say more.
        let below = hint(vec![vec![int(1), int(2)]]);
        assert_eq!(below.spans(1, |_| None), None);
        // Two lists: the shorter is asked for, bounded by the longer's span (40 to 300).
        let two = hint(vec![vec![int(40), int(300), int(100)], vec![p(1), p(2)]]);
        assert_eq!(
            two.spans(1, bound_to([Some(500), Some(60)])),
            Some(vec![(60, 60)])
        );
        // The shorter holds a value that isn't an integer: the longer is asked for.
        assert_eq!(
            two.spans(1, bound_to([Some(500), None])),
            Some(vec![(40, 40), (100, 100), (300, 300)])
        );
        // No list: no spans.
        assert_eq!(hint(Vec::new()).spans(1, |_| Some(1)), None);
    }

    /// Lane skip-11 (T-SKU.14, kiln goal 623adc1c): each way of asking for an IN list, forced
    /// into its failure mode. Do: three databases with the same rows and the same writes between
    /// rounds (deletes, and inserts whose values lie past every filter's), one asking for an IN
    /// list's span (the control), one asking value by value (`PRAGMA skipping = 't.a <kind>
    /// values'`), one keeping no summary; two prepared statements, each re-bound every round:
    /// `a IN (?1, ?2)` bound to two values far apart (the span's failure mode: it reads the
    /// whole gap), and `a IN (?1, ..., ?64)` bound to 64 neighbouring values (asking by value's:
    /// 64 probes for the rows one span names). Expect: every answer as without a summary; on the
    /// gap, asking by value read under a tenth of the rows the span read; on the clustered list,
    /// a single kind (zone maps or cracking) read the same rows both ways; the span probed once a
    /// run, by value twice a gap run and 64 times a clustered run.
    fn in_lists_by_span_and_by_value(kind: &str) {
        use super::super::{ListProbe, SkipCounts};
        use crate::Value;
        const CLUSTER: i64 = 64;
        let rows = 4000i64;
        let (_span_db, span) = open();
        let (_values_db, values) = open();
        let (_off_db, off) = open();
        fill([&span, &values], rows);
        {
            // `fill` takes two: `off` is filled alike here.
            off.execute("CREATE TABLE t (a INTEGER, b INTEGER, pad TEXT)")
                .unwrap();
            off.execute("BEGIN").unwrap();
            for i in 0..rows {
                off.execute(format!(
                    "INSERT INTO t VALUES ({i}, {}, '{}')",
                    i % 7,
                    "x".repeat(40)
                ))
                .unwrap();
            }
            off.execute("COMMIT").unwrap();
        }
        span.execute(format!("PRAGMA skipping = 't.a {kind} span'"))
            .unwrap();
        values
            .execute(format!("PRAGMA skipping = 't.a {kind} values'"))
            .unwrap();
        let root = root_of(&values);
        assert_eq!(
            values.db.skipping.list_probe(root),
            Some(ListProbe::PerValue),
            "'{kind} values'"
        );
        assert_eq!(
            span.db.skipping.list_probe(root_of(&span)),
            Some(ListProbe::Span),
            "'{kind} span'"
        );
        let cluster_sql = format!(
            "SELECT rowid, a, b FROM t WHERE a IN ({}) ORDER BY rowid",
            (1..=CLUSTER)
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let sqls = [
            "SELECT rowid, a, b FROM t WHERE a IN (?1, ?2) ORDER BY rowid".to_string(),
            cluster_sql,
        ];
        let mut stmts: Vec<_> = sqls
            .iter()
            .map(|sql| {
                [
                    span.prepare(sql).unwrap(),
                    values.prepare(sql).unwrap(),
                    off.prepare(sql).unwrap(),
                ]
            })
            .collect();
        let mut rng = ChaCha8Rng::seed_from_u64(14);
        // Rows read (span, by value), per statement; runs per statement.
        let mut read = [[0u64; 2]; 2];
        let mut runs = [0u64; 2];
        let mut appended = rows;
        for round in 0..30u64 {
            for _ in 0..rng.random_range(0..6) {
                let sql = if rng.random_bool(0.5) {
                    format!("DELETE FROM t WHERE rowid = {}", rng.random_range(1..rows))
                } else {
                    // Past every value a filter asks for: the zones stay as tight as they were.
                    appended += 1;
                    format!("INSERT INTO t VALUES ({appended}, 1, 'new')")
                };
                for conn in [&span, &values, &off] {
                    conn.execute(&sql).unwrap();
                }
            }
            for bind in 0..2u64 {
                let at = usize::try_from((round + bind) % 2).unwrap();
                let bound: Vec<Value> = if at == 0 {
                    let lo = rng.random_range(0..rows / 10);
                    let hi = rows - 1 - rng.random_range(0..rows / 10);
                    vec![Value::from_i64(lo), Value::from_i64(hi)]
                } else {
                    let base = rng.random_range(0..rows - CLUSTER);
                    (0..CLUSTER).map(|i| Value::from_i64(base + i)).collect()
                };
                let [stmt_span, stmt_values, stmt_off] = &mut stmts[at];
                let (answer_span, r_span) = run_bound(stmt_span, &bound);
                let (answer_values, r_values) = run_bound(stmt_values, &bound);
                let (answer_off, _) = run_bound(stmt_off, &bound);
                assert_eq!(
                    answer_span, answer_off,
                    "'{kind} span' round {round}: {}",
                    sqls[at]
                );
                assert_eq!(
                    answer_values, answer_off,
                    "'{kind} values' round {round}: {}",
                    sqls[at]
                );
                if at == 1 && kind != "measured" && kind != "capped" {
                    assert_eq!(
                        r_values, r_span,
                        "'{kind}' round {round}: the clustered list's values read other rows than its span"
                    );
                }
                read[at][0] += r_span;
                read[at][1] += r_values;
                runs[at] += 1;
            }
        }
        let [gap, _] = read;
        assert!(
            gap[1].saturating_mul(10) < gap[0],
            "'{kind}': on the gap, by value read {} rows, the span {}",
            gap[1],
            gap[0]
        );
        let counts = |conn: &Arc<Connection>| -> SkipCounts { skipping_counts(conn, "t").unwrap() };
        let (by_span, by_value) = (counts(&span), counts(&values));
        let all = runs[0] + runs[1];
        assert_eq!(by_span.consulted, all, "'{kind} span': {by_span:?}");
        assert_eq!(by_span.probes, all, "'{kind} span': {by_span:?}");
        assert_eq!(by_value.consulted, all, "'{kind} values': {by_value:?}");
        assert_eq!(
            by_value.probes,
            2 * runs[0] + u64::try_from(CLUSTER).unwrap() * runs[1],
            "'{kind} values': {by_value:?}"
        );
    }

    #[test]
    fn t_sku_14_zone_maps_in_lists_by_span_and_by_value() {
        in_lists_by_span_and_by_value("zone_maps");
    }

    #[test]
    fn t_sku_14_cracking_in_lists_by_span_and_by_value() {
        in_lists_by_span_and_by_value("cracking");
    }

    #[test]
    fn t_sku_14_measured_in_lists_by_span_and_by_value() {
        in_lists_by_span_and_by_value("measured");
    }

    #[test]
    fn t_sku_14_capped_in_lists_by_span_and_by_value() {
        in_lists_by_span_and_by_value("capped");
    }

    /// Lane skip-11 (T-SKU.14): `PRAGMA skipping` takes how an IN list is asked for after the
    /// kind (and a measured kind's build mode), the span by default, and refuses it with `off`.
    #[test]
    fn t_sku_14_pragma_sets_how_in_lists_are_asked_for() {
        use super::super::{BuildMode, ListProbe};
        let (db, on) = open();
        let (_other_db, other) = open();
        fill([&on, &other], 10);
        let root = root_of(&on);
        for (setting, probe, build) in [
            ("t.a", ListProbe::Span, BuildMode::Replay),
            (
                "t.a zone_maps values",
                ListProbe::PerValue,
                BuildMode::Replay,
            ),
            ("t.a cracking SPAN", ListProbe::Span, BuildMode::Replay),
            (
                "t.a capped inline values",
                ListProbe::PerValue,
                BuildMode::Inline,
            ),
            (
                "t.a measured values",
                ListProbe::PerValue,
                BuildMode::Replay,
            ),
            ("t.a capped void", ListProbe::Span, BuildMode::VoidOnWrite),
        ] {
            on.execute(format!("PRAGMA skipping = '{setting}'"))
                .unwrap();
            assert_eq!(db.skipping.list_probe(root), Some(probe), "'{setting}'");
            assert_eq!(db.skipping.build_mode(root), Some(build), "'{setting}'");
        }
        for bad in [
            "t.a off values",
            "t.a values capped",
            "t.a capped values inline",
            "t.a zone_maps values again",
            "t.a capped each",
        ] {
            assert!(
                on.execute(format!("PRAGMA skipping = '{bad}'")).is_err(),
                "'{bad}' was taken"
            );
        }
    }

    /// Lane skip-5 (T-SKU.4): a skipping scan runs its body over column batches (QT.7's fused
    /// loops) up to each candidate run's end. Four databases, batches on and off by skipping on and
    /// off, take the same random writes and answer every filtered aggregate and row query alike;
    /// with batches on, the skipping scan's fused loops ran rows and it read fewer rows than the
    /// unskipped one, as it does row by row.
    fn batched_skipping_answers_as_every_switch(kind: SkipKind) {
        let rows = 4000;
        let queries = 32;
        for seed in 0..3u64 {
            let mut rng = ChaCha8Rng::seed_from_u64(seed);
            // (batches, skipping) for each database.
            let switches = [(true, true), (true, false), (false, true), (false, false)];
            let dbs: Vec<_> = switches.iter().map(|_| open()).collect();
            for ((_, conn), (batches, _)) in dbs.iter().zip(switches) {
                conn.set_vectorized_scans(batches);
            }
            fill([&dbs[0].1, &dbs[1].1], rows);
            fill([&dbs[2].1, &dbs[3].1], rows);
            for ((_, conn), (_, skipping)) in dbs.iter().zip(switches) {
                if skipping {
                    set_skipping(conn, "t", "a", Some(kind)).unwrap();
                }
            }
            let mut read = [0u64; 4];
            let mut fused_skipping = 0usize;
            for round in 0..queries {
                for _ in 0..rng.random_range(0..15) {
                    let sql = random_write(&mut rng, rows);
                    dbs.iter().for_each(|(_, conn)| conn.execute(&sql).unwrap());
                }
                let lo = rng.random_range(-10..rows);
                let hi = lo.saturating_add(rng.random_range(0..rows / 10));
                let sql = match round % 4 {
                    0 => format!("SELECT count(*), sum(b), min(a), max(a) FROM t WHERE a BETWEEN {lo} AND {hi}"),
                    1 => format!("SELECT count(*), total(a) FROM t WHERE a >= {lo} AND a <= {hi} AND b <> 3"),
                    2 => format!("SELECT b, count(*), max(a) FROM t WHERE {lo} <= a AND a < {hi} GROUP BY b ORDER BY b"),
                    _ => format!("SELECT rowid, a, b FROM t WHERE a BETWEEN {lo} AND {hi} ORDER BY rowid"),
                };
                let mut answers = Vec::new();
                for (at, (_, conn)) in dbs.iter().enumerate() {
                    let before = crate::vdbe::leaf_batch::tests::fused();
                    let (answer, r) = query(conn, &sql);
                    if at == 0 {
                        fused_skipping += crate::vdbe::leaf_batch::tests::fused() - before;
                    }
                    read[at] += r;
                    answers.push(answer);
                }
                for (at, answer) in answers.iter().enumerate().skip(1) {
                    assert_eq!(
                        answer, &answers[0],
                        "seed {seed} {kind:?} (batches, skipping) {:?} vs {:?}: {sql}",
                        switches[at], switches[0]
                    );
                }
            }
            assert!(
                fused_skipping > 0,
                "seed {seed} {kind:?}: no skipping row ran in a batch"
            );
            assert!(
                read[0] < read[1],
                "seed {seed} {kind:?}: batched read {} skipping, {} without",
                read[0],
                read[1]
            );
            assert!(
                read[2] < read[3],
                "seed {seed} {kind:?}: row by row read {} skipping, {} without",
                read[2],
                read[3]
            );
        }
    }

    /// Lane skip-7 (T-SKU.9). Do: a column scattered over the rowids (each filter's candidates
    /// are short runs spread over many leaves), column batches on, skipping on and off, filtered
    /// aggregates and row queries with writes between. Expect: answers as without skipping; with
    /// skipping, the rows decoded into batches are at most the rows the scan read, so no batch
    /// decodes past its candidate run's end (before, each window ran to its leaf's end).
    fn batches_decode_only_candidate_runs(kind: SkipKind) {
        let rows = 4000i64;
        for seed in 0..2u64 {
            let mut rng = ChaCha8Rng::seed_from_u64(seed);
            let (_db_skip, skip) = open();
            let (_db_plain, plain) = open();
            for conn in [&skip, &plain] {
                conn.set_vectorized_scans(true);
                conn.execute("CREATE TABLE t (a INTEGER, b INTEGER, pad TEXT)")
                    .unwrap();
                conn.execute("BEGIN").unwrap();
                for i in 0..rows {
                    let a = i * 7919 % rows;
                    conn.execute(format!(
                        "INSERT INTO t VALUES ({a}, {}, '{}')",
                        i % 7,
                        "x".repeat(40)
                    ))
                    .unwrap();
                }
                conn.execute("COMMIT").unwrap();
            }
            set_skipping(&skip, "t", "a", Some(kind)).unwrap();
            let (mut decoded, mut read) = (0usize, 0u64);
            for round in 0..24 {
                for _ in 0..rng.random_range(0..6) {
                    let a = rng.random_range(0..rows);
                    let rowid = rng.random_range(1..rows);
                    let sql = format!("UPDATE t SET a = {a} WHERE rowid = {rowid}");
                    skip.execute(&sql).unwrap();
                    plain.execute(&sql).unwrap();
                }
                let lo = rng.random_range(0..rows);
                let hi = lo + rng.random_range(0..rows / 40);
                let sql = match round % 2 {
                    0 => format!("SELECT count(*), sum(b), min(a), max(a) FROM t WHERE a BETWEEN {lo} AND {hi}"),
                    _ => format!("SELECT rowid, a, b FROM t WHERE a BETWEEN {lo} AND {hi} ORDER BY rowid"),
                };
                let before = crate::vdbe::leaf_batch::tests::decoded();
                let (answer, r) = query(&skip, &sql);
                decoded += crate::vdbe::leaf_batch::tests::decoded() - before;
                read += r;
                let (expected, _) = query(&plain, &sql);
                assert_eq!(answer, expected, "seed {seed} {kind:?}: {sql}");
            }
            assert!(
                decoded > 0,
                "seed {seed} {kind:?}: no skipping row was batched"
            );
            assert!(
                u64::try_from(decoded).unwrap() <= read,
                "seed {seed} {kind:?}: {decoded} rows decoded, {read} read"
            );
        }
    }

    #[test]
    fn t_sku_9_cracking_batches_decode_only_candidate_runs() {
        batches_decode_only_candidate_runs(SkipKind::Cracking);
    }

    #[test]
    fn t_sku_9_measured_batches_decode_only_candidate_runs() {
        batches_decode_only_candidate_runs(SkipKind::Measured);
    }

    #[test]
    fn t_sku_4_zone_maps_batched_skipping_answers_as_every_switch() {
        batched_skipping_answers_as_every_switch(SkipKind::ZoneMaps);
    }

    #[test]
    fn t_sku_4_cracking_batched_skipping_answers_as_every_switch() {
        batched_skipping_answers_as_every_switch(SkipKind::Cracking);
    }

    #[test]
    fn t_sku_4_measured_batched_skipping_answers_as_every_switch() {
        batched_skipping_answers_as_every_switch(SkipKind::Measured);
    }

    #[test]
    fn t_sku_1_zone_maps_prepared_parameters_skip_and_answer_as_unswitched() {
        prepared_parameters_skip(SkipKind::ZoneMaps);
    }

    #[test]
    fn t_sku_1_cracking_prepared_parameters_skip_and_answer_as_unswitched() {
        prepared_parameters_skip(SkipKind::Cracking);
    }

    #[test]
    fn t_sku_2_measured_prepared_parameters_skip_and_answer_as_unswitched() {
        prepared_parameters_skip(SkipKind::Measured);
    }
}

/// Lane skip-3 (T-SKU.2): the chooser under each method's failure mode. The scenario's cost of a
/// filter is the work its method did, from the method's own counters, plus the rows its candidates
/// make the scan read (the executor measures time instead; this keeps the test off the clock).
mod chooser {
    use super::*;

    fn n(count: usize) -> u64 {
        u64::try_from(count).unwrap_or(u64::MAX)
    }

    /// The rows a scan reads for `candidates` of `table`.
    fn rows_read(table: &Table, candidates: &Candidates) -> u64 {
        match candidates {
            Candidates::All => n(table.len()),
            Candidates::Ranges(ranges) => ranges
                .iter()
                .map(|r| n(table.range(r.first..=r.last).count()))
                .sum(),
            Candidates::Rowids(rowids) => n(rowids.len()),
        }
    }

    fn zone_work(zones: &ZoneMap) -> u64 {
        let s = zones.stats();
        s.zones_read + s.zones_skipped + s.zones_split
    }

    fn crack_work(cracker: &CrackerColumn) -> u64 {
        let s = cracker.stats();
        s.entries_partitioned + s.entries_searched + s.entries_moved
    }

    fn work_of(chooser: &Chooser, method: SkipMethod) -> u64 {
        match method {
            SkipMethod::ZoneMaps => chooser.zone_map().map_or(0, zone_work),
            SkipMethod::Cracking => chooser.cracker().map_or(0, crack_work),
        }
    }

    fn sound(table: &Table, candidates: &Candidates, lo: i64, hi: i64) {
        let expected = plain(table, lo, hi);
        let found: Vec<i64> = expected
            .iter()
            .copied()
            .filter(|rowid| match candidates {
                Candidates::All => true,
                Candidates::Ranges(ranges) => {
                    ranges.iter().any(|r| (r.first..=r.last).contains(rowid))
                }
                Candidates::Rowids(rowids) => rowids.binary_search(rowid).is_ok(),
            })
            .collect();
        assert_eq!(
            found, expected,
            "a candidate set missed a row for {lo}..={hi}"
        );
    }

    struct Run {
        zone_total: u64,
        crack_total: u64,
        chooser_total: u64,
        stats: ChooserStats,
        leader: Option<SkipMethod>,
        /// Each method's write upkeep inside the chooser over the run (zone maps, cracking): kept
        /// summaries both take every write, which the doubling bound on scans doesn't cover.
        upkeep: [u64; 2],
    }

    /// Runs the zone map alone, the cracker alone and the chooser in lockstep over one stream of
    /// writes and filters, the chooser handed each filter's cost as the scenario counts it.
    fn lockstep(
        seed: u64,
        rows: i64,
        value: impl Fn(&mut ChaCha8Rng, i64) -> Option<i64>,
        writes_per_filter: usize,
        width: i64,
        filters: usize,
    ) -> Run {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut table = Table::new();
        for rowid in 1..=rows {
            let v = value(&mut rng, rowid);
            table.insert(rowid, v);
        }
        // The scenario's stand-in for a table's leaves: a zone per 64 rows (the zone map's own
        // tests build zones from real leaves).
        let build = |table: &Table| {
            let mut builder = ZoneMapBuilder::new();
            for (i, (rowid, v)) in table.iter().enumerate() {
                if i % 64 == 0 {
                    builder.end_run();
                }
                builder.row(*rowid, *v).unwrap();
            }
            builder.finish()
        };
        let mut zones = build(&table);
        let mut cracker = CrackerColumn::new(table.iter().map(|(r, v)| (*r, *v)));
        let mut chooser = Chooser::new(
            build(&table),
            CrackerColumn::new(table.iter().map(|(r, v)| (*r, *v))),
        );
        let (mut zone_total, mut crack_total, mut chooser_total) = (0u64, 0u64, 0u64);
        let mut seen = [
            work_of(&chooser, SkipMethod::ZoneMaps),
            work_of(&chooser, SkipMethod::Cracking),
        ];
        let top = rows / 4 + 10;
        let mut upkeep = [0u64; 2];
        for _ in 0..filters {
            let upkeep_from = [
                work_of(&chooser, SkipMethod::ZoneMaps),
                work_of(&chooser, SkipMethod::Cracking),
            ];
            for _ in 0..writes_per_filter {
                let rowid = rng.random_range(1..=rows);
                let old = table[&rowid];
                let new = value(&mut rng, rowid);
                table.insert(rowid, new);
                let write = RowWrite::Update { rowid, old, new };
                zones.on_write(write).unwrap();
                cracker.on_write(write).unwrap();
                chooser.on_write(write).unwrap();
            }
            upkeep[0] += work_of(&chooser, SkipMethod::ZoneMaps) - upkeep_from[0];
            upkeep[1] += work_of(&chooser, SkipMethod::Cracking) - upkeep_from[1];
            let lo = rng.random_range(-20..top);
            let hi = lo + rng.random_range(0..=width);

            let before = zone_work(&zones);
            let c = Candidates::Ranges(zones.candidate_ranges(lo, hi));
            sound(&table, &c, lo, hi);
            zone_total += zone_work(&zones) - before + rows_read(&table, &c);

            let before = crack_work(&cracker);
            let c = Candidates::Rowids(cracker.candidate_rowids(lo, hi).unwrap());
            sound(&table, &c, lo, hi);
            crack_total += crack_work(&cracker) - before + rows_read(&table, &c);

            let (c, method) = chooser.candidates(lo, hi).unwrap();
            sound(&table, &c, lo, hi);
            let at = usize::from(method == SkipMethod::Cracking);
            let work = work_of(&chooser, method);
            // A method's cost includes the work its writes made since it last answered.
            let cost = work - seen[at] + rows_read(&table, &c);
            seen[at] = work;
            chooser.record(method, cost);
            chooser_total += cost;
        }
        Run {
            zone_total,
            crack_total,
            chooser_total,
            upkeep,
            stats: chooser.stats(),
            leader: chooser.leader(),
        }
    }

    /// Do: zone maps' failure mode: the column's values scattered at random over the rowids, so
    /// every zone spans the whole range and none is skipped; narrow repeated-ish filters, no
    /// writes. Expect: the chooser leads with cracking, measures zone maps again only a logarithmic
    /// number of times, and its total stays within the doubling bound of the better method alone.
    #[test]
    fn t_sku_2_zone_maps_failure_mode_chooses_cracking() {
        for seed in 0..8 {
            let rows = 4096;
            let filters = 256;
            let run = lockstep(
                seed,
                rows,
                |rng, _| Some(rng.random_range(0..rows / 4)),
                0,
                8,
                filters,
            );
            assert!(
                run.crack_total < run.zone_total,
                "seed {seed}: scenario didn't fail zone maps"
            );
            assert_eq!(
                run.leader,
                Some(SkipMethod::Cracking),
                "seed {seed}: {:?}",
                run.stats
            );
            let log = u64::from(usize::BITS - filters.leading_zeros());
            assert!(
                run.stats.zone_maps.trials <= 1 + log * (1 + run.stats.switches),
                "seed {seed}: zone maps measured {} times: {:?}",
                run.stats.zone_maps.trials,
                run.stats
            );
            assert!(
                run.chooser_total
                    <= 2 * run.crack_total + run.upkeep[0] + 2 * u64::try_from(rows).unwrap(),
                "seed {seed}: chooser {} vs cracking alone {}",
                run.chooser_total,
                run.crack_total
            );
        }
    }

    /// Do: cracking's failure mode: a column that follows the rowid (zones skip all but a few),
    /// heavy write churn between filters (each write ripples through the cracker's pieces) and
    /// wide filters. Expect: the chooser leads with zone maps, cracking measured a logarithmic
    /// number of times, and its total within the doubling bound of zone maps alone.
    #[test]
    fn t_sku_2_cracking_failure_mode_chooses_zone_maps() {
        for seed in 0..8 {
            let rows = 4096;
            let filters = 256;
            let run = lockstep(
                seed,
                rows,
                |rng, rowid| Some(rowid / 4 + rng.random_range(-2..=2)),
                64,
                rows / 8,
                filters,
            );
            assert!(
                run.zone_total < run.crack_total,
                "seed {seed}: scenario didn't fail cracking"
            );
            assert_eq!(
                run.leader,
                Some(SkipMethod::ZoneMaps),
                "seed {seed}: {:?}",
                run.stats
            );
            let log = u64::from(usize::BITS - filters.leading_zeros());
            assert!(
                run.stats.cracking.trials <= 1 + log * (1 + run.stats.switches),
                "seed {seed}: cracking measured {} times: {:?}",
                run.stats.cracking.trials,
                run.stats
            );
            assert!(
                run.chooser_total
                    <= 2 * run.zone_total + run.upkeep[1] + 2 * u64::try_from(rows).unwrap(),
                "seed {seed}: chooser {} vs zone maps alone {}",
                run.chooser_total,
                run.zone_total
            );
        }
    }

    /// A column's values for one phase of an upkeep scenario: (rng, rowid, rows).
    type ValueOf = fn(&mut ChaCha8Rng, i64, i64) -> Option<i64>;

    fn scattered(rng: &mut ChaCha8Rng, _rowid: i64, rows: i64) -> Option<i64> {
        Some(rng.random_range(0..rows / 4))
    }

    fn following(rng: &mut ChaCha8Rng, rowid: i64, _rows: i64) -> Option<i64> {
        Some(rowid / 4 + rng.random_range(-8..=8))
    }

    /// One phase: writes per filter, the values they write, the filters' widths (at least, at
    /// most, in values), the filters.
    struct Phase {
        writes: usize,
        value: ValueOf,
        width: (i64, i64),
        filters: usize,
    }

    struct UpkeepRun {
        stats: ChooserStats,
        /// The write upkeep both methods cost over the run.
        upkeep: u64,
        /// The scans' costs and the builds' over the run (lane skip-7: with `upkeep`, the run's
        /// whole cost).
        scans: u64,
        leader: Option<SkipMethod>,
    }

    /// The work a method's writes do: zone maps' zone updates and splits, the cracker's entries
    /// searched and moved.
    fn upkeep_work(chooser: &Chooser, method: SkipMethod) -> u64 {
        match method {
            SkipMethod::ZoneMaps => chooser.zone_map().map_or(0, |z| {
                let s = z.stats();
                s.writes + s.zones_split
            }),
            SkipMethod::Cracking => chooser.cracker().map_or(0, |c| {
                let s = c.stats();
                s.writes + s.entries_searched + s.entries_moved
            }),
        }
    }

    /// Lane skip-5 (T-SKU.5): phases of writes and filters through a chooser under `upkeep`, in
    /// the scenario's units (work done and rows read, as `lockstep`): each method's write upkeep
    /// handed back as the work its writes did, a build costing a read of every row, a dropped
    /// method built again when its trial is due. Every candidate set is checked against the plain
    /// scan.
    fn upkeep_run(seed: u64, upkeep: Upkeep, phases: &[Phase]) -> UpkeepRun {
        upkeep_run_led(seed, upkeep, Lead::for_upkeep(upkeep), phases)
    }

    /// [`upkeep_run`] under a lead rule of the test's (lane skip-7, T-SKU.7).
    fn upkeep_run_led(seed: u64, upkeep: Upkeep, lead: Lead, phases: &[Phase]) -> UpkeepRun {
        let rows = 4096i64;
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut table = Table::new();
        for rowid in 1..=rows {
            let v = (phases[0].value)(&mut rng, rowid, rows);
            table.insert(rowid, v);
        }
        // The scenario's stand-in for a table's leaves, as in `lockstep`: a zone per 64 rows.
        let mut builder = ZoneMapBuilder::new();
        for (i, (rowid, v)) in table.iter().enumerate() {
            if i % 64 == 0 {
                builder.end_run();
            }
            builder.row(*rowid, *v).unwrap();
        }
        let copy = |table: &Table| CrackerColumn::new(table.iter().map(|(r, v)| (*r, *v)));
        let build = Some(n(table.len()));
        let mut chooser =
            Chooser::with_rules(builder.finish(), copy(&table), upkeep, lead, [build, build]);
        let mut total = 0u64;
        let mut scans = 0u64;
        let top = rows / 4 + 10;
        for phase in phases {
            for _ in 0..phase.filters {
                for _ in 0..phase.writes {
                    let rowid = rng.random_range(1..=rows);
                    let old = table[&rowid];
                    let new = (phase.value)(&mut rng, rowid, rows);
                    table.insert(rowid, new);
                    let write = RowWrite::Update { rowid, old, new };
                    for method in [SkipMethod::ZoneMaps, SkipMethod::Cracking] {
                        let before = upkeep_work(&chooser, method);
                        if chooser.write_method(method, write).unwrap() {
                            let cost = upkeep_work(&chooser, method) - before;
                            total += cost;
                            chooser.record_upkeep(method, cost);
                        }
                    }
                }
                match chooser.rebuild_due() {
                    Some(SkipMethod::ZoneMaps) => {
                        let (size, mut clock) = (n(table.len()), 0u64);
                        chooser
                            .rebuild_zones(&mut || {
                                clock += size;
                                clock
                            })
                            .unwrap();
                        scans += size;
                    }
                    Some(SkipMethod::Cracking) => {
                        let before = chooser.method_stats(SkipMethod::Cracking).rebuilt;
                        chooser
                            .install_cracker(copy(&table), n(table.len()), &mut || 0)
                            .unwrap();
                        assert_eq!(
                            chooser.method_stats(SkipMethod::Cracking).rebuilt,
                            before + 1
                        );
                        // The build, and the cut points it was cut at again (T-SKU.8).
                        scans += n(table.len());
                        scans += chooser
                            .cracker()
                            .map_or(0, |c| c.stats().entries_partitioned);
                    }
                    None => {}
                }
                let lo = rng.random_range(-20..top - phase.width.0);
                let hi = lo + rng.random_range(phase.width.0..=phase.width.1);
                let before = [
                    work_of(&chooser, SkipMethod::ZoneMaps),
                    work_of(&chooser, SkipMethod::Cracking),
                ];
                let (c, method) = chooser.candidates(lo, hi).unwrap();
                sound(&table, &c, lo, hi);
                let at = usize::from(method == SkipMethod::Cracking);
                let cost =
                    work_of(&chooser, method).saturating_sub(before[at]) + rows_read(&table, &c);
                chooser.record(method, cost);
                scans += cost;
            }
        }
        UpkeepRun {
            stats: chooser.stats(),
            upkeep: total,
            scans,
            leader: chooser.leader(),
        }
    }

    /// Do: cracking's failure mode as a write-heavy table: the column follows the rowid, every
    /// filter comes after many writes, and filters are wide, so the cracker keeps paying to move
    /// entries while zone maps answer about as cheaply. Expect: capped, the cracker is dropped for
    /// its upkeep while it isn't leading and the run's write upkeep is lower than keeping both;
    /// kept both, nothing is dropped; every answer is sound.
    #[test]
    fn t_sku_5_write_heavy_table_drops_the_losing_cracker() {
        for seed in 0..4 {
            // Filters over half the column or more: zone maps read about what matches, the
            // cracker hands back as many rowids again.
            let phases = [Phase {
                writes: 64,
                value: following,
                width: (512, 1024),
                filters: 160,
            }];
            let capped = upkeep_run(seed, Upkeep::Capped, &phases);
            let kept = upkeep_run(seed, Upkeep::KeepBoth, &phases);
            assert!(
                capped.stats.cracking.dropped > 0,
                "seed {seed}: {:?}",
                capped.stats
            );
            assert_eq!(
                kept.stats.cracking.dropped + kept.stats.zone_maps.dropped,
                0
            );
            assert!(
                capped.upkeep < kept.upkeep,
                "seed {seed}: upkeep {} capped, {} keeping both",
                capped.upkeep,
                kept.upkeep
            );
        }
    }

    /// Do: a workload that shifts: first zone maps' failure mode (scattered values, narrow
    /// filters, no writes), so cracking leads; then heavy writes make the column follow the rowid
    /// and filters widen, where zone maps win, while their upkeep passes their build cost.
    /// Expect: capped, zone maps are dropped and built again (from the cracker) when their trial
    /// comes due; kept both, nothing is dropped; every answer is sound. Lane skip-7: run on the
    /// scan-only lead rule, as the mechanism under test is the drop and rebuild of the method not
    /// leading; under total cost (`Upkeep::Capped`'s own rule) the cracker's moves let zone maps
    /// lead as soon as the writes start, which `t_sku_7_*` checks.
    #[test]
    fn t_sku_5_shifting_workload_rebuilds_the_dropped_method() {
        for seed in 0..2 {
            let phases = [
                Phase {
                    writes: 0,
                    value: scattered,
                    width: (0, 8),
                    filters: 160,
                },
                Phase {
                    writes: 64,
                    value: following,
                    width: (512, 1024),
                    filters: 1024,
                },
            ];
            let capped = upkeep_run_led(seed, Upkeep::Capped, Lead::Scan, &phases);
            let kept = upkeep_run(seed, Upkeep::KeepBoth, &phases);
            // Lane skip-8: which kind each ends led by is reported, not asserted (T-SKU.5's text
            // claims none; skip-7's model had keeping both end led by cracking).
            eprintln!(
                "t_sku_5 seed {seed}: capped ends led by {:?}, kept both by {:?}",
                capped.leader, kept.leader
            );
            assert!(
                capped.stats.zone_maps.dropped > 0,
                "seed {seed}: {:?}",
                capped.stats
            );
            assert!(
                capped.stats.zone_maps.rebuilt > 0,
                "seed {seed}: {:?}",
                capped.stats
            );
            assert_eq!(
                kept.stats.cracking.dropped + kept.stats.zone_maps.dropped,
                0
            );
        }
    }

    /// Lane skip-7 (T-SKU.7). Do: costs handed in directly: cracking's scan a little cheaper than
    /// zone maps', then a stretch of writes costs the cracker five times what it costs zone maps.
    /// Expect: on the total-cost rule the next filter goes to zone maps and the leader changes
    /// (11 against 13); on the scan rule (the control) cracking keeps leading (8 against 10).
    #[test]
    fn t_sku_7_total_cost_leader_counts_the_write_upkeep() {
        for (lead, expect) in [
            (Lead::Total, SkipMethod::ZoneMaps),
            (Lead::Scan, SkipMethod::Cracking),
        ] {
            let mut chooser = Chooser::with_rules(
                ZoneMapBuilder::new().finish(),
                CrackerColumn::new([]),
                Upkeep::Capped,
                lead,
                [None, None],
            );
            let (_, first) = chooser.candidates(0, 0).unwrap();
            assert_eq!(first, SkipMethod::ZoneMaps);
            chooser.record(first, 10);
            let (_, second) = chooser.candidates(0, 0).unwrap();
            assert_eq!(second, SkipMethod::Cracking);
            chooser.record(second, 8);
            assert_eq!(chooser.leader(), Some(SkipMethod::Cracking), "{lead:?}");
            chooser.record_upkeep(SkipMethod::Cracking, 5);
            chooser.record_upkeep(SkipMethod::ZoneMaps, 1);
            let (_, third) = chooser.candidates(0, 0).unwrap();
            assert_eq!(third, expect, "{lead:?}: {:?}", chooser.stats());
            chooser.record(third, if third == SkipMethod::ZoneMaps { 10 } else { 8 });
            assert_eq!(chooser.leader(), Some(expect), "{lead:?}");
            assert_eq!(
                chooser.stats().switches,
                u64::from(expect == SkipMethod::ZoneMaps),
                "{lead:?}"
            );
            assert_eq!(
                chooser.method_stats(SkipMethod::Cracking).last_upkeep,
                Some(5)
            );
            assert_eq!(
                chooser.method_stats(SkipMethod::ZoneMaps).last_upkeep,
                Some(1)
            );
        }
    }

    /// Lane skip-7 (T-SKU.7), the A/B of the lead rule under the cap. Do: zone maps' scan failure
    /// mode (scattered values, narrow filters: zone maps read every row) under write churn (256
    /// writes a filter), where the cracker's moves cost far more than the rows zone maps read.
    /// Expect: on the scan rule cracking leads and its upkeep goes on being paid; on the total
    /// rule zone maps lead, the cracker is dropped, and the run's whole cost (scans, builds and
    /// write upkeep) is lower. Every answer is sound.
    #[test]
    fn t_sku_7_capped_total_cost_lead_pays_less_under_write_churn() {
        for seed in 0..2 {
            let phases = [Phase {
                writes: 256,
                value: scattered,
                width: (0, 8),
                filters: 160,
            }];
            let total = upkeep_run_led(seed, Upkeep::Capped, Lead::Total, &phases);
            let scan = upkeep_run_led(seed, Upkeep::Capped, Lead::Scan, &phases);
            assert_eq!(
                total.leader,
                Some(SkipMethod::ZoneMaps),
                "seed {seed}: {:?}",
                total.stats
            );
            assert_eq!(
                scan.leader,
                Some(SkipMethod::Cracking),
                "seed {seed}: {:?}",
                scan.stats
            );
            assert!(
                total.stats.cracking.dropped > 0,
                "seed {seed}: {:?}",
                total.stats
            );
            assert!(
                total.scans + total.upkeep < scan.scans + scan.upkeep,
                "seed {seed}: total-cost lead {} + {}, scan lead {} + {}",
                total.scans,
                total.upkeep,
                scan.scans,
                scan.upkeep
            );
        }
    }

    /// Lane skip-7 (T-SKU.7). Do: T-SKU.5's shifting workload on the cap's own rule (total cost).
    /// Expect: it ends led by zone maps with the cracker dropped for its upkeep, paying less
    /// upkeep than keeping both; every answer is sound.
    #[test]
    fn t_sku_7_shifting_workload_ends_led_by_zone_maps_on_total_cost() {
        for seed in 0..2 {
            let phases = [
                Phase {
                    writes: 0,
                    value: scattered,
                    width: (0, 8),
                    filters: 160,
                },
                Phase {
                    writes: 64,
                    value: following,
                    width: (512, 1024),
                    filters: 1024,
                },
            ];
            let capped = upkeep_run(seed, Upkeep::Capped, &phases);
            let kept = upkeep_run(seed, Upkeep::KeepBoth, &phases);
            assert_eq!(
                capped.leader,
                Some(SkipMethod::ZoneMaps),
                "seed {seed}: {:?}",
                capped.stats
            );
            assert!(
                capped.stats.cracking.dropped > 0,
                "seed {seed}: {:?}",
                capped.stats
            );
            assert!(
                capped.upkeep < kept.upkeep,
                "seed {seed}: {} capped, {} kept",
                capped.upkeep,
                kept.upkeep
            );
        }
    }

    /// Lane skip-7 (T-SKU.7). Do: capped, the cracker dropped for its upkeep while zone maps
    /// lead; then zone maps' own scans cost more than the cracker's last measure. Expect: the
    /// cracker is due a build and a trial at once (`rebuild_due`), zone maps answer until it is
    /// built, and zone maps, the last kind kept, are never dropped however much upkeep they cost.
    #[test]
    fn t_sku_7_a_dropped_kind_measured_cheaper_is_built_again_and_the_last_kept_stays() {
        let mut chooser = Chooser::with_rules(
            ZoneMapBuilder::new().finish(),
            CrackerColumn::new([]),
            Upkeep::Capped,
            Lead::Scan,
            [Some(10), Some(10)],
        );
        let (_, first) = chooser.candidates(0, 0).unwrap();
        chooser.record(first, 1);
        let (_, second) = chooser.candidates(0, 0).unwrap();
        chooser.record(second, 100);
        assert_eq!(chooser.leader(), Some(SkipMethod::ZoneMaps));
        chooser.record_upkeep(SkipMethod::Cracking, 11);
        assert!(!chooser.keeps(SkipMethod::Cracking));
        chooser.record(SkipMethod::ZoneMaps, 200);
        assert_eq!(chooser.next_method(), (SkipMethod::Cracking, true));
        assert_eq!(chooser.rebuild_due(), Some(SkipMethod::Cracking));
        let (candidates, method) = chooser.candidates(0, 0).unwrap();
        assert_eq!(method, SkipMethod::ZoneMaps);
        assert_ne!(candidates, Candidates::All);
        chooser.record_upkeep(SkipMethod::ZoneMaps, 1_000_000);
        assert!(chooser.keeps(SkipMethod::ZoneMaps), "{:?}", chooser.stats());
        assert_eq!(chooser.method_stats(SkipMethod::ZoneMaps).dropped, 0);
    }

    /// Lane skip-7 (T-SKU.8). Do: a cracker cut by a few filters, and a fresh copy of the same
    /// rows cut at its cut points. Expect: the copy holds the same cut points, answers each of the
    /// filters with the same rowids without partitioning anything more, and its cutting moved at
    /// most a copy's entries per halving of the cut points (n log k, not n k).
    #[test]
    fn t_sku_8_cracker_cut_again_at_its_cut_points_answers_without_cutting() {
        let rows: Vec<(i64, Option<i64>)> = (1..=2000i64)
            .map(|rowid| {
                (
                    rowid,
                    if rowid % 97 == 0 {
                        None
                    } else {
                        Some(rowid * 7919 % 1000)
                    },
                )
            })
            .collect();
        let filters = [
            (10, 50),
            (400, 420),
            (700, 900),
            (0, 5),
            (990, 999),
            (300, 300),
        ];
        let mut cut = CrackerColumn::new(rows.clone());
        for (lo, hi) in filters {
            cut.candidate_rowids(lo, hi).unwrap();
        }
        let bounds = cut.bounds();
        assert!(bounds.len() >= filters.len(), "{bounds:?}");
        let mut again = CrackerColumn::new(rows.clone());
        again.crack_at(&bounds).unwrap();
        assert_eq!(again.bounds(), bounds);
        let k = u64::try_from(bounds.len()).unwrap();
        let levels = u64::from(u64::BITS - k.leading_zeros());
        let entries = u64::try_from(rows.len()).unwrap();
        assert!(
            again.stats().entries_partitioned <= entries * levels,
            "{} partitioned for {k} cut points over {entries} rows",
            again.stats().entries_partitioned
        );
        for (lo, hi) in filters {
            let before = again.stats().entries_partitioned;
            assert_eq!(
                again.candidate_rowids(lo, hi).unwrap(),
                cut.candidate_rowids(lo, hi).unwrap()
            );
            assert_eq!(
                again.stats().entries_partitioned,
                before,
                "{lo}..={hi} was cut again"
            );
        }
    }

    /// Lane skip-7 (T-SKU.8). Do: a capped chooser whose cracker, cut by its trial, is dropped for
    /// its upkeep, then built again from the table. Expect: the cracker built again holds the
    /// dropped one's cut points, and its build counts once.
    #[test]
    fn t_sku_8_rebuilt_cracker_keeps_its_cut_points() {
        let rows: Vec<(i64, Option<i64>)> = (1..=500i64)
            .map(|rowid| (rowid, Some(rowid * 31 % 250)))
            .collect();
        let mut builder = ZoneMapBuilder::new();
        builder.end_run();
        for (rowid, value) in &rows {
            builder.row(*rowid, *value).unwrap();
        }
        let mut chooser = Chooser::with_rules(
            builder.finish(),
            CrackerColumn::new(rows.clone()),
            Upkeep::Capped,
            Lead::Scan,
            [Some(10), Some(10)],
        );
        let (_, first) = chooser.candidates(5, 20).unwrap();
        assert_eq!(first, SkipMethod::ZoneMaps);
        chooser.record(first, 1);
        let (_, second) = chooser.candidates(30, 40).unwrap();
        assert_eq!(second, SkipMethod::Cracking);
        chooser.record(second, 100);
        assert_eq!(chooser.leader(), Some(SkipMethod::ZoneMaps));
        let bounds = chooser.cracker().unwrap().bounds();
        assert_eq!(bounds, vec![30, 41]);
        chooser.record_upkeep(SkipMethod::Cracking, 11);
        assert!(
            !chooser.keeps(SkipMethod::Cracking),
            "{:?}",
            chooser.stats()
        );
        chooser
            .install_cracker(CrackerColumn::new(rows), 10, &mut || 0)
            .unwrap();
        assert_eq!(chooser.cracker().unwrap().bounds(), bounds);
        assert_eq!(chooser.method_stats(SkipMethod::Cracking).rebuilt, 1);
        assert_eq!(chooser.method_stats(SkipMethod::Cracking).dropped, 1);
    }

    fn empty_chooser() -> Chooser {
        Chooser::new(ZoneMapBuilder::new().finish(), CrackerColumn::new([]))
    }

    /// Do: costs handed in directly: zone maps cheap, then their cost jumps (writes scattered
    /// the column). Expect: each method tried once first; the leader changes on the leader's own
    /// next measure, with no trial needed; a switch is counted.
    #[test]
    fn t_sku_2_leader_changes_when_its_cost_rises() {
        let mut chooser = empty_chooser();
        assert_eq!(chooser.next_method(), (SkipMethod::ZoneMaps, true));
        chooser.record(SkipMethod::ZoneMaps, 10);
        assert_eq!(chooser.next_method(), (SkipMethod::Cracking, true));
        chooser.record(SkipMethod::Cracking, 100);
        assert_eq!(chooser.leader(), Some(SkipMethod::ZoneMaps));
        assert_eq!(chooser.next_method(), (SkipMethod::ZoneMaps, false));
        chooser.record(SkipMethod::ZoneMaps, 1000);
        assert_eq!(chooser.leader(), Some(SkipMethod::Cracking));
        assert_eq!(chooser.stats().switches, 1);
        assert_eq!(chooser.next_method(), (SkipMethod::Cracking, false));
    }

    /// Do: a leader confirmed again and again. Expect: the other method's trials come after the
    /// leader spent its cost times 1, 2, 4, 8...: geometrically rarer.
    #[test]
    fn t_sku_2_confirmed_leader_is_checked_geometrically_less_often() {
        let mut chooser = empty_chooser();
        chooser.record(SkipMethod::ZoneMaps, 1);
        chooser.record(SkipMethod::Cracking, 10);
        let mut gaps = Vec::new();
        let mut since = 0u64;
        for _ in 0..2000 {
            let (_, method) = chooser.candidates(0, 0).unwrap();
            if method == SkipMethod::Cracking {
                gaps.push(since);
                since = 0;
                chooser.record(method, 10);
            } else {
                since += 1;
                chooser.record(method, 1);
            }
        }
        assert_eq!(&gaps[..5], &[10, 20, 40, 80, 160], "{gaps:?}");
    }

    /// Do: a burst: once a trial is due, a thousand scans start before any cost comes back
    /// (connections at once). Expect: one trial of the other method, the rest stay with the leader.
    #[test]
    fn t_sku_2_burst_issues_one_trial() {
        let mut chooser = empty_chooser();
        chooser.record(SkipMethod::ZoneMaps, 1);
        chooser.record(SkipMethod::Cracking, 5);
        for _ in 0..5 {
            chooser.candidates(0, 0).unwrap();
            chooser.record(SkipMethod::ZoneMaps, 1);
        }
        let mut trials = 0;
        for _ in 0..1000 {
            let (_, method) = chooser.candidates(0, 0).unwrap();
            trials += usize::from(method == SkipMethod::Cracking);
        }
        assert_eq!(trials, 1, "{:?}", chooser.stats());
    }
}

/// Lane skip-8 (T-SKU.10): a measured summary's dropped method built again off the scan's path,
/// one build per summary at a time, installed only when no write came in during it.
mod builds {
    use super::super::{
        start_build, zone_map_of_table, BuildEnd, BuildMode, Chooser, CrackerColumn, Lead,
        RowWrite, SkipMethod, SkipRegistry, Skipping, Upkeep, ZoneMap, ZoneMapBuilder,
    };
    use crate::io::MemoryIO;
    use crate::sync::Arc;
    use crate::{Connection, Database, SqliteDialect};
    use std::sync::Barrier;

    const ROOT: i64 = 2;
    const AT: (u32, u64) = (0, 1);

    /// T-SKU.7's scenario: capped, the cracker dropped for its upkeep while zone maps led, then
    /// zone maps measured costlier than the cracker's last scan, so the cracker's build is due.
    fn cracker_due() -> Chooser {
        due_over(ZoneMapBuilder::new().finish(), CrackerColumn::new([]))
    }

    /// [`cracker_due`]'s scenario over a summary holding rows: `zones` and `cracker` as built.
    fn due_over(zones: ZoneMap, cracker: CrackerColumn) -> Chooser {
        let mut chooser = Chooser::with_rules(
            zones,
            cracker,
            Upkeep::Capped,
            Lead::Scan,
            [Some(10), Some(10)],
        );
        let (_, first) = chooser.candidates(0, 0).unwrap();
        chooser.record(first, 1);
        let (_, second) = chooser.candidates(0, 0).unwrap();
        chooser.record(second, 100);
        chooser.record_upkeep(SkipMethod::Cracking, 11);
        chooser.record(SkipMethod::ZoneMaps, 200);
        assert_eq!(chooser.rebuild_due(), Some(SkipMethod::Cracking));
        chooser
    }

    fn registry_due() -> SkipRegistry {
        let registry = SkipRegistry::default();
        registry.set(ROOT, 0, Skipping::Measured(Box::new(cracker_due())));
        registry
    }

    /// Rows `1..=n`, each row's value its rowid.
    fn table_rows(n: i64) -> Vec<(i64, Option<i64>)> {
        (1..=n).map(|rowid| (rowid, Some(rowid))).collect()
    }

    /// A registry whose summary holds `rows`, its cracker due a build, built by `mode`.
    fn registry_over(rows: &[(i64, Option<i64>)], mode: BuildMode) -> SkipRegistry {
        let mut zones = ZoneMapBuilder::new();
        for (rowid, value) in rows {
            zones.row(*rowid, *value).unwrap();
        }
        let chooser = due_over(zones.finish(), CrackerColumn::new(rows.iter().copied()));
        let registry = SkipRegistry::default();
        registry.set_with_build(ROOT, 0, Skipping::Measured(Box::new(chooser)), mode);
        registry
    }

    /// The rows the summary's cracker holds, by rowid; `None` while it is dropped.
    fn cracker_rows(registry: &SkipRegistry, root: i64) -> Option<Vec<(i64, Option<i64>)>> {
        registry
            .with_summary(root, |s| match s {
                Skipping::Measured(chooser) => chooser.cracker().map(|c| {
                    let mut rows = c.rows();
                    rows.sort_unstable();
                    rows
                }),
                _ => None,
            })
            .flatten()
    }

    /// A writer's insert of `rowid` (value `rowid`) at the newest commit `at`.
    fn insert(registry: &SkipRegistry, at: (u32, u64), rowid: i64) {
        registry.on_write(
            ROOT,
            9,
            at,
            RowWrite::Insert {
                rowid,
                value: Some(rowid),
            },
            &mut || 0,
        );
    }

    fn copy() -> CrackerColumn {
        CrackerColumn::new([(1, Some(5)), (2, Some(7))])
    }

    fn keeps_cracker(registry: &SkipRegistry, root: i64) -> bool {
        registry.with_summary(
            root,
            |s| matches!(s, Skipping::Measured(chooser) if chooser.keeps(SkipMethod::Cracking)),
        ) == Some(true)
    }

    /// Do: sixteen connections find the same build due at once. Expect: exactly one claims it;
    /// while it holds the claim no other gets one; handed back, the still-due build can be
    /// claimed again.
    #[test]
    fn t_sku_10_connections_asking_at_once_claim_one_build() {
        let registry = std::sync::Arc::new(registry_due());
        let askers = 16;
        let barrier = std::sync::Arc::new(Barrier::new(askers));
        let handles: Vec<_> = (0..askers)
            .map(|_| {
                let registry = std::sync::Arc::clone(&registry);
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    registry.claim_build(ROOT)
                })
            })
            .collect();
        let mut claims: Vec<_> = handles
            .into_iter()
            .filter_map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(claims.len(), 1, "{askers} askers: {claims:?}");
        let claim = claims.pop().unwrap();
        assert_eq!(claim.method, SkipMethod::Cracking);
        assert!(registry.building(ROOT));
        assert_eq!(registry.claim_build(ROOT), None);
        registry.abandon_build(claim);
        assert!(!registry.building(ROOT));
        let again = registry.claim_build(ROOT).expect("the trial is still due");
        registry.abandon_build(again);
    }

    /// Do: a build pinned at the newest commit, then a write reaches the summary before it hands
    /// its copy back. Expect: nothing installed (the copy would miss the write), the cracker stays
    /// dropped and the claim is free; a build pinned after the writer committed installs, and no
    /// build is due after it.
    #[test]
    fn t_sku_10_a_write_during_the_build_voids_it() {
        let registry = registry_over(&[], BuildMode::VoidOnWrite);
        let claim = registry.claim_build(ROOT).unwrap();
        assert!(registry.pin_build(&claim, 1, AT, AT));
        registry.on_write(
            ROOT,
            9,
            AT,
            RowWrite::Insert {
                rowid: 3,
                value: Some(1),
            },
            &mut || 0,
        );
        assert_eq!(
            registry.install_cracker(claim, copy(), 10, &mut || 0),
            BuildEnd::Invalidated
        );
        assert!(!keeps_cracker(&registry, ROOT));
        assert!(!registry.building(ROOT));
        let committed = (0, 2);
        let claim = registry.claim_build(ROOT).expect("the trial is still due");
        assert!(registry.pin_build(&claim, 1, committed, committed));
        assert_eq!(
            registry.install_cracker(claim, copy(), 10, &mut || 0),
            BuildEnd::Installed
        );
        assert!(keeps_cracker(&registry, ROOT));
        assert_eq!(registry.claim_build(ROOT), None, "built: none is due");
    }

    /// Do: a build asked for while another connection's write is open, one whose summary is
    /// set again during it, and one on a summary made stale. Expect: the first reads nothing (its
    /// snapshot misses the open write); the second installs nothing and leaves the new summary's
    /// own claim alone; a stale summary hands out no claim.
    #[test]
    fn t_sku_10_a_build_installs_only_on_the_state_it_read() {
        let registry = registry_due();
        registry.on_write(
            ROOT,
            9,
            AT,
            RowWrite::Insert {
                rowid: 3,
                value: Some(1),
            },
            &mut || 0,
        );
        let claim = registry.claim_build(ROOT).unwrap();
        assert!(!registry.pin_build(&claim, 1, AT, AT));
        registry.abandon_build(claim);
        assert!(!registry.building(ROOT));

        let old = registry.claim_build(ROOT).unwrap();
        registry.set(ROOT, 0, Skipping::Measured(Box::new(cracker_due())));
        let fresh = registry
            .claim_build(ROOT)
            .expect("the new summary's own build");
        assert_eq!(
            registry.install_cracker(old, copy(), 10, &mut || 0),
            BuildEnd::Superseded
        );
        assert!(registry.building(ROOT), "the old claim freed the new one");
        registry.abandon_build(fresh);

        registry.on_write(
            ROOT,
            9,
            AT,
            RowWrite::Insert {
                rowid: 4,
                value: Some(2),
            },
            &mut || 0,
        );
        registry.rolled_back(9);
        assert_eq!(registry.claim_build(ROOT), None);
    }

    /// T-SKU.10, AC-NS.2.1: pin after an unrelated commit while a told writer is
    /// open, then after a write between the snapshot sample and the pin.
    #[test]
    fn t_sku_10_versioned_build_pins_respect_writes() {
        use crate::skipping::SkipRefusal;
        let insert = RowWrite::Insert {
            rowid: 3,
            value: Some(1),
        };
        for mode in [BuildMode::Replay, BuildMode::VoidOnWrite] {
            let registry = registry_over(&[], mode);
            registry.on_write_told(ROOT, 9, AT, insert, &mut || 0);
            let claim = registry.claim_build(ROOT).unwrap();
            let other_commit = (0, 2);
            assert!(
                !registry.pin_build(&claim, 1, other_commit, other_commit),
                "another commit cannot end this writer"
            );
            registry.abandon_build(claim);
            registry.rolled_back(9);
            assert_eq!(
                registry.candidates(ROOT, 1, other_commit, other_commit, 1, 1),
                Err(SkipRefusal::Stale)
            );

            let registry = registry_over(&[], mode);
            let claim = registry.claim_build(ROOT).unwrap();
            registry.on_write_told(ROOT, 9, AT, insert, &mut || 0);
            registry.committed(9);
            assert!(
                !registry.pin_build(&claim, 1, AT, AT),
                "the sampled snapshot predates the write"
            );
        }
    }

    /// Do: a real table whose summary's cracker is due a build, and eight connections that find
    /// it due at once start the build. Expect: one build starts, on its own thread and
    /// connection, and installs a cracker holding every row of the table; none is due after.
    #[test]
    fn t_sku_10_a_due_cracker_is_built_once_from_the_table_off_the_scan() {
        let io = Arc::new(MemoryIO::new());
        let db = Database::open_file(io, ":memory:", Arc::new(SqliteDialect)).unwrap();
        let conn = db.connect(crate::ConnectOpts::new()).unwrap();
        let rows = 500;
        conn.execute("CREATE TABLE t (a INTEGER, pad TEXT)")
            .unwrap();
        conn.execute("BEGIN").unwrap();
        for i in 0..rows {
            conn.execute(format!("INSERT INTO t VALUES ({i}, 'x')"))
                .unwrap();
        }
        conn.execute("COMMIT").unwrap();
        let root = conn
            .with_schema_mut(|s| s.get_btree_table("t"))
            .unwrap()
            .unwrap()
            .root_page;
        db.skipping
            .set(root, 0, Skipping::Measured(Box::new(cracker_due())));
        let askers = 8;
        let barrier = std::sync::Arc::new(Barrier::new(askers));
        let handles: Vec<_> = (0..askers)
            .map(|_| {
                let asker = db.connect(crate::ConnectOpts::new()).unwrap();
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    start_build(&asker, root, 2)
                })
            })
            .collect();
        let builds: Vec<_> = handles
            .into_iter()
            .filter_map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(builds.len(), 1, "{askers} askers started {}", builds.len());
        let ends: Vec<BuildEnd> = builds
            .into_iter()
            .map(|build| build.join().unwrap())
            .collect();
        assert_eq!(ends, vec![BuildEnd::Installed]);
        let held = db
            .skipping
            .with_summary(root, |s| match s {
                Skipping::Measured(chooser) => chooser.cracker().map(|c| c.rows().len()),
                _ => None,
            })
            .flatten();
        assert_eq!(held, Some(500));
        assert!(!db.skipping.building(root));
        assert_eq!(db.skipping.claim_build(root), None);
    }

    /// A table `t (a INTEGER, pad TEXT)` of `rows` rows (`a` = 0, 1, ...) on `conn`, and its root.
    fn fill_table(conn: &Arc<Connection>, rows: i64) -> i64 {
        conn.execute("CREATE TABLE t (a INTEGER, pad TEXT)")
            .unwrap();
        conn.execute("BEGIN").unwrap();
        for i in 0..rows {
            conn.execute(format!("INSERT INTO t VALUES ({i}, 'x')"))
                .unwrap();
        }
        conn.execute("COMMIT").unwrap();
        conn.with_schema_mut(|s| s.get_btree_table("t"))
            .unwrap()
            .unwrap()
            .root_page
    }

    /// `t`'s summary on `conn`'s database, its cracker due a build, built by `mode`.
    fn set_due(conn: &Arc<Connection>, root: i64, rows: i64, mode: BuildMode) {
        let zones = zone_map_of_table(conn, "t", "a").unwrap();
        let cracker = CrackerColumn::new((0..rows).map(|i| (i.saturating_add(1), Some(i))));
        conn.db.skipping.set_with_build(
            root,
            0,
            Skipping::Measured(Box::new(due_over(zones, cracker))),
            mode,
        );
    }

    /// Lane skip-9 (T-SKU.11), void's failure mode. Do: a write reaches the summary during every
    /// build, eight trials in a row (a steady write rate), once void-on-write and once replaying.
    /// Expect: void-on-write installs none (each build voided, the cracker never comes back);
    /// replay installs the first, its copy holding the table's rows and the write that came in
    /// during it, and no build is due after.
    #[test]
    fn t_sku_11_steady_writes_starve_void_but_replay_installs() {
        let rows = table_rows(64);
        let trials = 8u64;
        for mode in [BuildMode::VoidOnWrite, BuildMode::Replay] {
            let registry = registry_over(&rows, mode);
            let mut ends = Vec::new();
            for trial in 1..=trials {
                let Some(claim) = registry.claim_build(ROOT) else {
                    break;
                };
                let committed = (0, trial);
                assert!(
                    registry.pin_build(&claim, 1, committed, committed),
                    "{mode:?} trial {trial}"
                );
                insert(&registry, committed, 1000 + i64::try_from(trial).unwrap());
                ends.push(registry.install_cracker(
                    claim,
                    CrackerColumn::new(rows.iter().copied()),
                    10,
                    &mut || 0,
                ));
            }
            let counts = registry.counts(ROOT).unwrap();
            match mode {
                BuildMode::VoidOnWrite => {
                    assert_eq!(ends, vec![BuildEnd::Invalidated; 8], "void: {ends:?}");
                    assert_eq!(counts.builds_invalidated, trials);
                    assert_eq!(counts.builds_installed, 0);
                    assert!(
                        !keeps_cracker(&registry, ROOT),
                        "void: starved, the cracker never came back"
                    );
                }
                _ => {
                    assert_eq!(ends, vec![BuildEnd::Installed], "replay: {ends:?}");
                    assert_eq!(counts.builds_installed, 1);
                    assert_eq!(counts.replayed_writes, 1);
                    let mut want = rows.clone();
                    want.push((1001, Some(1001)));
                    assert_eq!(cracker_rows(&registry, ROOT), Some(want));
                    assert_eq!(registry.claim_build(ROOT), None, "built: none is due");
                }
            }
        }
    }

    /// Lane skip-9 (T-SKU.11), replay's failure mode: its memory under a burst. Do: a build on a
    /// summary of 64 rows, then a burst of writes during it: an update, a delete and inserts, as
    /// many as the rows (the bound), and once one more. Expect: within the bound, the record
    /// holds the whole burst and the copy installed is the table after the burst, in order; one
    /// past it, the record never holds more than the bound, is dropped, and the build falls back
    /// to void (nothing installed, one overflow counted).
    #[test]
    fn t_sku_11_a_burst_past_the_replay_bound_falls_back_to_void() {
        let rows = table_rows(64);
        let bound = u64::try_from(rows.len()).unwrap();
        for burst in [bound, bound + 1] {
            let registry = registry_over(&rows, BuildMode::Replay);
            let claim = registry.claim_build(ROOT).unwrap();
            assert!(registry.pin_build(&claim, 1, AT, AT));
            let mut writes = vec![
                RowWrite::Update {
                    rowid: 1,
                    old: Some(1),
                    new: Some(500),
                },
                RowWrite::Delete {
                    rowid: 2,
                    old: Some(2),
                },
            ];
            let inserts = burst - 2;
            writes.extend(
                (0..i64::try_from(inserts).unwrap()).map(|i| RowWrite::Insert {
                    rowid: 100 + i,
                    value: Some(i),
                }),
            );
            for write in &writes {
                registry.on_write(ROOT, 9, AT, *write, &mut || 0);
            }
            let end = registry.install_cracker(
                claim,
                CrackerColumn::new(rows.iter().copied()),
                10,
                &mut || 0,
            );
            let counts = registry.counts(ROOT).unwrap();
            assert!(
                counts.replay_peak <= bound,
                "burst {burst}: the record held {} past its bound {bound}",
                counts.replay_peak
            );
            if burst <= bound {
                assert_eq!(end, BuildEnd::Installed, "burst {burst}");
                assert_eq!(counts.replay_peak, burst);
                assert_eq!(counts.replay_overflows, 0);
                assert_eq!(counts.replayed_writes, burst);
                let mut want: Vec<_> = rows
                    .iter()
                    .copied()
                    .filter(|(rowid, _)| *rowid > 2)
                    .collect();
                want.push((1, Some(500)));
                want.extend((0..i64::try_from(inserts).unwrap()).map(|i| (100 + i, Some(i))));
                want.sort_unstable();
                assert_eq!(
                    cracker_rows(&registry, ROOT),
                    Some(want),
                    "burst {burst}: replayed in order"
                );
            } else {
                assert_eq!(
                    end,
                    BuildEnd::Invalidated,
                    "burst {burst}: fell back to void"
                );
                assert_eq!(counts.replay_overflows, 1);
                assert_eq!(counts.replayed_writes, 0);
                assert!(!keeps_cracker(&registry, ROOT));
                assert!(!registry.building(ROOT));
                assert!(
                    registry.claim_build(ROOT).is_some(),
                    "the next trial claims again"
                );
            }
        }
    }

    /// Lane skip-9 (T-SKU.11), inline's failure mode: the scan that finds the trial due pays the
    /// build. Do: a real table of 500 rows whose cracker is due, and one filter through SQL,
    /// under each build mode. Expect: inline, the filter's own statement read every row of the
    /// table for the build and the cracker is kept when it returns; void-on-write and replay read
    /// none on the scan's path. Answers equal across modes.
    #[test]
    fn t_sku_11_inline_builds_on_the_scans_path_the_others_off_it() {
        let rows = 500;
        let mut answers = Vec::new();
        for mode in [BuildMode::Inline, BuildMode::VoidOnWrite, BuildMode::Replay] {
            let io = Arc::new(MemoryIO::new());
            let db = Database::open_file(io, ":memory:", Arc::new(SqliteDialect)).unwrap();
            let conn = db.connect(crate::ConnectOpts::new()).unwrap();
            let root = fill_table(&conn, rows);
            set_due(&conn, root, rows, mode);
            let mut stmt = conn
                .prepare("SELECT a FROM t WHERE a BETWEEN 10 AND 20")
                .unwrap();
            let answer: Vec<String> = stmt
                .run_collect_rows()
                .unwrap()
                .into_iter()
                .map(|row| format!("{row:?}"))
                .collect();
            answers.push(answer);
            let counts = db.skipping.counts(root).unwrap();
            assert_eq!(
                counts.consulted, 1,
                "{mode:?}: the filter skipped with the summary"
            );
            match mode {
                BuildMode::Inline => {
                    assert_eq!(
                        counts.inline_rows_read, 500,
                        "inline: the scan read the table for the build"
                    );
                    assert_eq!(counts.builds_installed, 1);
                    assert!(keeps_cracker(&db.skipping, root));
                }
                _ => assert_eq!(
                    counts.inline_rows_read, 0,
                    "{mode:?}: nothing read on the scan's path"
                ),
            }
        }
        assert!(
            answers.windows(2).all(|pair| pair.first() == pair.last()),
            "{answers:?}"
        );
        assert_eq!(answers.first().map(Vec::len), Some(11));
    }

    /// Lane skip-9 (T-SKU.11): `PRAGMA skipping` takes a measured summary's build mode, replay by
    /// default, and refuses one after a kind that never builds again.
    #[test]
    fn t_sku_11_pragma_sets_the_build_mode() {
        let io = Arc::new(MemoryIO::new());
        let db = Database::open_file(io, ":memory:", Arc::new(SqliteDialect)).unwrap();
        let conn = db.connect(crate::ConnectOpts::new()).unwrap();
        let root = fill_table(&conn, 10);
        for (setting, mode) in [
            ("t.a capped", BuildMode::Replay),
            ("t.a capped inline", BuildMode::Inline),
            ("t.a CAPPED void", BuildMode::VoidOnWrite),
            ("t.a measured replay", BuildMode::Replay),
        ] {
            conn.execute(format!("PRAGMA skipping = '{setting}'"))
                .unwrap();
            assert_eq!(db.skipping.build_mode(root), Some(mode), "'{setting}'");
        }
        for bad in [
            "t.a zone_maps replay",
            "t.a cracking void",
            "t.a off inline",
            "t.a capped sideways",
            "t.a capped void again",
        ] {
            assert!(
                conn.execute(format!("PRAGMA skipping = '{bad}'")).is_err(),
                "'{bad}' was taken"
            );
        }
    }

    /// Lane skip-9 (T-SKU.12). Do: an encrypted database (aes256gcm), its connection opened with
    /// the key, a table whose cracker is due, and the build started from that connection.
    /// Expect: the build's own connection opens with the same key, reads every row and installs.
    #[cfg(all(feature = "encryption", feature = "fs"))]
    #[test]
    fn t_sku_12_an_encrypted_databases_build_reads_with_its_key() {
        // A test key: 32 bytes, as aes256gcm takes (the cipher's key size).
        let hexkey = "5a".repeat(32);
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("skip-encrypted.db");
        let io: Arc<dyn crate::IO> = Arc::new(crate::io::PlatformIO::new().unwrap());
        let db = Database::open_file_with_flags(
            io,
            path.to_str().unwrap(),
            crate::OpenFlags::default(),
            crate::DatabaseOpts::new().with_encryption(true),
            Some(crate::EncryptionOpts {
                cipher: "aes256gcm".to_string(),
                hexkey: hexkey.clone(),
            }),
            Arc::new(SqliteDialect),
        )
        .unwrap();
        let key = crate::EncryptionKey::from_hex_string(&hexkey).unwrap();
        let conn = db
            .connect(crate::ConnectOpts::new().with_encryption_key(Some(key)))
            .unwrap();
        let rows = 300;
        let root = fill_table(&conn, rows);
        for mode in [BuildMode::VoidOnWrite, BuildMode::Replay] {
            set_due(&conn, root, rows, mode);
            let build = start_build(&conn, root, 2).expect("the cracker's build is due");
            assert_eq!(build.join().unwrap(), BuildEnd::Installed, "{mode:?}");
            let held = cracker_rows(&db.skipping, root).map(|rows| rows.len());
            assert_eq!(held, Some(300), "{mode:?}");
        }
    }

    /// One run of a prepared statement bound to `values`: its rows, printed.
    fn run_bound(stmt: &mut crate::Statement, values: &[crate::Value]) -> Vec<String> {
        stmt.reset().unwrap();
        for (i, value) in values.iter().enumerate() {
            stmt.bind_at(std::num::NonZero::new(i + 1).unwrap(), value.clone())
                .unwrap();
        }
        let rows = stmt.run_collect_rows().unwrap();
        rows.into_iter().map(|row| format!("{row:?}")).collect()
    }

    /// Lane skip-11 (T-SKU.15, kiln goal 623adc1c): every piece the goal names, end to end
    /// through SQL. Do: for each build mode (inline, void-on-write, replay) and each way of
    /// asking for an IN list (span, value by value), an encrypted (aes256gcm) file database whose
    /// capped summary has a cracker build due under that mode, and an in-memory database with the
    /// same rows and no summary; one statement prepared once for each bound shape (`BETWEEN ? AND
    /// ?`, `>= ?`, `<= ?`, `= ?`, `IN (?, ?)` with its values half the table apart, `IN` of 16
    /// neighbouring parameters, and `IN (?, ?)` with one value bound to a real), each bound and
    /// run, then re-bound and run again, every round, with random SQL writes (inserts, NULL and
    /// text values, updates, deletes) between rounds. Expect: every run answers as the database
    /// without a summary; every run bound by integers consulted the summary and none with a real
    /// did; the summary probed once a run for the span, and once a value by value.
    #[cfg(all(feature = "encryption", feature = "fs"))]
    #[test]
    fn t_sku_15_every_piece_end_to_end_through_sql() {
        use super::super::{skipping_counts, ListProbe};
        use crate::Value;
        use rand::{Rng, SeedableRng};
        const CLUSTER: i64 = 16;
        // A test key: 32 bytes, as aes256gcm takes (the cipher's key size).
        let hexkey = "5a".repeat(32);
        let rows = 2000i64;
        let cluster = (1..=CLUSTER)
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sqls = [
            "SELECT rowid, a FROM t WHERE a BETWEEN ? AND ? ORDER BY rowid".to_string(),
            "SELECT count(*), sum(rowid) FROM t WHERE a >= ?".to_string(),
            "SELECT count(*), sum(rowid) FROM t WHERE a <= ?".to_string(),
            "SELECT rowid, a FROM t WHERE a = ? ORDER BY rowid".to_string(),
            "SELECT rowid, a FROM t WHERE a IN (?, ?) ORDER BY rowid".to_string(),
            format!("SELECT rowid, a FROM t WHERE a IN ({cluster}) ORDER BY rowid"),
            "SELECT rowid, a FROM t WHERE a IN (?1, ?2) ORDER BY rowid".to_string(),
        ];
        let mut seed = 15u64;
        for mode in [BuildMode::Inline, BuildMode::VoidOnWrite, BuildMode::Replay] {
            for probe in [ListProbe::Span, ListProbe::PerValue] {
                seed += 1;
                let case = format!("{mode:?} {probe:?}");
                let dir = tempfile::TempDir::new().unwrap();
                let path = dir.path().join("skip-end-to-end.db");
                let io: Arc<dyn crate::IO> = Arc::new(crate::io::PlatformIO::new().unwrap());
                let db = Database::open_file_with_flags(
                    io,
                    path.to_str().unwrap(),
                    crate::OpenFlags::default(),
                    crate::DatabaseOpts::new().with_encryption(true),
                    Some(crate::EncryptionOpts {
                        cipher: "aes256gcm".to_string(),
                        hexkey: hexkey.clone(),
                    }),
                    Arc::new(SqliteDialect),
                )
                .unwrap();
                let key = crate::EncryptionKey::from_hex_string(&hexkey).unwrap();
                let on = db
                    .connect(crate::ConnectOpts::new().with_encryption_key(Some(key)))
                    .unwrap();
                let off_db = Database::open_file(
                    Arc::new(MemoryIO::new()),
                    ":memory:",
                    Arc::new(SqliteDialect),
                )
                .unwrap();
                let off = off_db.connect(crate::ConnectOpts::new()).unwrap();
                let root = fill_table(&on, rows);
                fill_table(&off, rows);
                set_due(&on, root, rows, mode);
                assert!(db.skipping.set_list_probe(root, probe), "{case}");
                let mut stmts: Vec<_> = sqls
                    .iter()
                    .map(|sql| (on.prepare(sql).unwrap(), off.prepare(sql).unwrap()))
                    .collect();
                let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(seed);
                let (mut integer_runs, mut probes) = (0u64, 0u64);
                for round in 0..12u64 {
                    for _ in 0..rng.random_range(0..8) {
                        let a = rng.random_range(0..rows);
                        let rowid = rng.random_range(1..rows);
                        let sql = match rng.random_range(0..6) {
                            0 => format!("INSERT INTO t VALUES ({a}, 'new')"),
                            1 => "INSERT INTO t VALUES (NULL, 'null')".to_string(),
                            2 => format!("UPDATE t SET a = {a} WHERE rowid = {rowid}"),
                            3 => format!("UPDATE t SET a = 'text' WHERE rowid = {rowid}"),
                            4 => format!("UPDATE t SET a = {}.5 WHERE rowid = {rowid}", a),
                            _ => format!("DELETE FROM t WHERE rowid = {rowid}"),
                        };
                        on.execute(&sql).unwrap();
                        off.execute(&sql).unwrap();
                    }
                    for (at, (stmt_on, stmt_off)) in stmts.iter_mut().enumerate() {
                        for bind in 0..2u64 {
                            let lo = rng.random_range(0..rows);
                            let (values, asked): (Vec<Value>, u64) = match at {
                                0 => {
                                    let hi = lo + rng.random_range(0..rows / 20);
                                    (vec![Value::from_i64(lo), Value::from_i64(hi)], 1)
                                }
                                1 => (
                                    vec![Value::from_i64(rows - rng.random_range(0..rows / 20))],
                                    1,
                                ),
                                2 => (vec![Value::from_i64(rng.random_range(0..rows / 20))], 1),
                                3 => (vec![Value::from_i64(lo)], 1),
                                4 => {
                                    let far = (lo + rows / 2) % rows;
                                    (vec![Value::from_i64(lo), Value::from_i64(far)], 2)
                                }
                                5 => (
                                    (0..CLUSTER).map(|i| Value::from_i64(lo + i)).collect(),
                                    u64::try_from(CLUSTER).unwrap(),
                                ),
                                _ => (
                                    vec![Value::from_i64(lo), Value::from_f64(lo as f64 + 0.5)],
                                    0,
                                ),
                            };
                            let answer_on = run_bound(stmt_on, &values);
                            let answer_off = run_bound(stmt_off, &values);
                            assert_eq!(
                                answer_on, answer_off,
                                "{case} round {round} bind {bind}: {} {values:?}",
                                sqls[at]
                            );
                            if asked > 0 {
                                integer_runs += 1;
                                probes += match probe {
                                    ListProbe::Span => 1,
                                    ListProbe::PerValue => asked,
                                };
                            }
                        }
                    }
                }
                let counts = skipping_counts(&on, "t").unwrap();
                assert_eq!(counts.consulted, integer_runs, "{case}: {counts:?}");
                assert_eq!(counts.probes, probes, "{case}: {counts:?}");
            }
        }
    }
}

/// G7 step 2 (`AC-NS.2.1`): the summaries on the versioned storage, where writers take no single
/// write lock: a scan asks the summary, and a writer's end is told rather than read from the
/// positions.
mod versioned {
    use super::super::{
        set_skipping, set_skipping_until, skipping_candidates, skipping_counts, BuildPolicy,
        RowWrite, SkipKind, SkipRefusal, SkipRegistry, Skipping, ZoneMapBuilder,
    };
    use crate::io::MemoryIO;
    use crate::sync::Arc;
    use crate::{ConnectOpts, Connection, Database, SqliteDialect, Value};

    /// A query's rowids and the rows its scan read (the statement's counter).
    fn rowids(conn: &Arc<Connection>, sql: &str) -> (Vec<i64>, u64) {
        let mut stmt = conn.prepare(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
        let rows = stmt
            .run_collect_rows()
            .unwrap_or_else(|e| panic!("{sql}: {e}"))
            .into_iter()
            .map(|row| match row.first() {
                Some(Value::Numeric(crate::Numeric::Integer(rowid))) => *rowid,
                other => panic!("{sql}: rowid {other:?}"),
            })
            .collect();
        (rows, stmt.metrics().rows_read)
    }

    #[test]
    fn t_ns_2_1_a_filter_uses_the_column_and_snapshot_checked_at_the_probe() {
        use super::super::{Candidates, CrackerColumn};
        let registry = SkipRegistry::default();
        let root = 5;
        let install = |column, value| {
            registry.set(
                root,
                column,
                Skipping::Cracking(CrackerColumn::new(vec![(1, Some(value))])),
            )
        };
        install(0, 7);
        let column = registry.column_of(root).unwrap();
        // The scan has computed bounds for column 0; an install on column 1 wins the race.
        install(1, 9);
        let snap = (0, 3);
        let candidates = registry.probe(root, 1, Some(column), &[(7, 7)], || Some((snap, snap)));
        assert_eq!(
            candidates,
            Err(SkipRefusal::Off),
            "column {column} still has row 1"
        );
        install(0, 7);
        registry.on_write_told(
            root,
            2,
            snap,
            RowWrite::Update {
                rowid: 1,
                old: Some(7),
                new: Some(9),
            },
            &mut || 0,
        );
        registry.committed(2);
        let newest = (0, 4);
        let candidates = registry.probe(root, 1, Some(column), &[(7, 7)], || Some((snap, newest)));
        assert_eq!(candidates, Err(SkipRefusal::OlderSnapshot));
        assert_eq!(
            registry.candidates(root, 1, newest, newest, 9, 9),
            Ok(Candidates::Rowids(vec![1]))
        );
    }

    /// Do: a told writer's write, another writer's commit beside it, then its own commit told.
    /// Expect: a reader at the newest commit is refused until the writer's commit is told (a
    /// newer commit isn't its commit); the writer sees its own rows; a second open writer's write
    /// makes the summary stale.
    #[test]
    fn t_ns_2_1_a_told_writer_ends_only_when_told() {
        let (root, writer, other, reader) = (5, 1, 2, 3);
        let registry = SkipRegistry::default();
        let mut zones = ZoneMapBuilder::new();
        for rowid in 1..=8 {
            zones.row(rowid, Some(rowid)).unwrap();
        }
        registry.set(root, 0, Skipping::ZoneMaps(zones.finish()));
        let insert = |rowid| RowWrite::Insert {
            rowid,
            value: Some(rowid),
        };
        registry.on_write_told(root, writer, (0, 3), insert(9), &mut || 0);
        // Another writer committed beside it: the newest commit is 4, without the open writes.
        assert_eq!(
            registry.candidates(root, reader, (0, 4), (0, 4), 1, 9),
            Err(SkipRefusal::OlderSnapshot)
        );
        assert!(registry
            .candidates(root, writer, (0, 3), (0, 4), 1, 9)
            .is_ok());
        registry.committed(writer);
        assert!(registry
            .candidates(root, reader, (0, 5), (0, 5), 1, 9)
            .is_ok());
        registry.on_write_told(root, writer, (0, 5), insert(10), &mut || 0);
        registry.on_write_told(root, other, (0, 5), insert(11), &mut || 0);
        assert_eq!(
            registry.candidates(root, other, (0, 5), (0, 5), 1, 11),
            Err(SkipRefusal::Stale)
        );
    }

    /// Do: a summary of each kind on a versioned table, a SQL filter, writes through SQL, and a
    /// reader whose snapshot is older than a commit. Expect: the scan asks the summary (counted
    /// consulted, fewer rows read) and answers as without it; the older reader is refused and
    /// still sees its snapshot's rows.
    #[test]
    fn t_ns_2_1_a_scan_on_the_versioned_storage_asks_the_summary() {
        let sql = "SELECT rowid FROM t WHERE a BETWEEN 100 AND 120 ORDER BY rowid";
        for kind in [SkipKind::ZoneMaps, SkipKind::Cracking, SkipKind::Measured] {
            let io = Arc::new(MemoryIO::new());
            let db = Database::open_file(io, ":memory:", Arc::new(SqliteDialect)).unwrap();
            let conn = db
                .connect(ConnectOpts::new().with_versioned_storage())
                .unwrap();
            assert!(conn.versioned_for(crate::MAIN_DB_ID).is_some());
            conn.execute("CREATE TABLE t (a INTEGER, pad TEXT)")
                .unwrap();
            conn.execute("BEGIN").unwrap();
            for i in 0..3000 {
                conn.execute(format!("INSERT INTO t VALUES ({i}, '{}')", "x".repeat(40)))
                    .unwrap();
            }
            conn.execute("COMMIT").unwrap();
            let (plain, read_plain) = rowids(&conn, sql);
            assert_eq!(plain, (101..=121).collect::<Vec<_>>());
            set_skipping(&conn, "t", "a", Some(kind)).unwrap();
            let (skipped, read_skipped) = rowids(&conn, sql);
            assert_eq!(skipped, plain, "{kind:?}");
            assert!(
                read_skipped < read_plain,
                "{kind:?}: read {read_skipped} rows skipping, {read_plain} not"
            );
            assert_eq!(
                skipping_counts(&conn, "t").unwrap().consulted,
                1,
                "{kind:?}"
            );
            // Writes reach the summary through the hook, each committed and told.
            conn.execute("DELETE FROM t WHERE a = 110").unwrap();
            conn.execute("INSERT INTO t VALUES (105, 'new')").unwrap();
            let mut expected: Vec<i64> = (101..=121).filter(|rowid| *rowid != 111).collect();
            expected.push(3001);
            assert_eq!(rowids(&conn, sql).0, expected, "{kind:?}");
            let counts = skipping_counts(&conn, "t").unwrap();
            assert_eq!((counts.writes, counts.consulted), (2, 2), "{kind:?}");
            // A reader whose snapshot a commit passed is refused, and reads its snapshot whole.
            let reader = db
                .connect(ConnectOpts::new().with_versioned_storage())
                .unwrap();
            reader.execute("BEGIN").unwrap();
            let (before, _) = rowids(&reader, sql);
            assert_eq!(before, expected, "{kind:?}");
            conn.execute("DELETE FROM t WHERE a BETWEEN 100 AND 120")
                .unwrap();
            assert_eq!(
                skipping_candidates(&reader, "t", 100, 120),
                Err(SkipRefusal::OlderSnapshot),
                "{kind:?}"
            );
            assert_eq!(rowids(&reader, sql).0, before, "{kind:?}");
            reader.execute("COMMIT").unwrap();
            assert!(rowids(&reader, sql).0.is_empty(), "{kind:?}");
        }
    }

    /// Do: a checked install whose check says rows were missed, then one whose check finds
    /// none. Expect: the first is kept stale (counted, a scan refused) and moves the installs; the
    /// second is consulted.
    #[test]
    fn t_ns_2_1_a_checked_install_keeps_a_build_that_missed_rows_stale() {
        let (root, reader) = (5, 1);
        let registry = SkipRegistry::default();
        let zones = || {
            let mut zones = ZoneMapBuilder::new();
            for rowid in 1..=8 {
                zones.row(rowid, Some(rowid)).unwrap();
            }
            Skipping::ZoneMaps(zones.finish())
        };
        let before = registry.installs();
        let why = "another open transaction has changed the table";
        assert_eq!(
            registry.set_checked(root, 0, zones(), || Some(why)),
            Some(why)
        );
        assert_ne!(registry.installs(), before);
        assert_eq!(
            registry.candidates(root, reader, (0, 1), (0, 1), 1, 8),
            Err(SkipRefusal::Stale)
        );
        assert_eq!(registry.counts(root).unwrap().built_stale, 1);
        assert_eq!(registry.set_checked(root, 0, zones(), || None), None);
        assert!(registry
            .candidates(root, reader, (0, 1), (0, 1), 1, 8)
            .is_ok());
        let counts = registry.counts(root).unwrap();
        assert_eq!((counts.built_stale, counts.consulted), (0, 1));
    }

    /// Do: one connection inserts a row the filter matches and stays open; another builds a
    /// summary beside it, at a snapshot without the row; the writer commits. Expect: the build is
    /// kept stale (counted), so the filter scans every row and finds the committed row (a summary
    /// kept current would skip its zone and lose it); built again with no writer open, it holds
    /// the row and is consulted. `BuildPolicy::Stale`'s failure mode, forced: until the next
    /// build every scan of the table reads all its rows.
    #[test]
    fn t_ns_2_1_a_build_beside_an_open_writer_is_kept_stale() {
        let sql = "SELECT rowid FROM t WHERE a BETWEEN 100 AND 120 ORDER BY rowid";
        for kind in [SkipKind::ZoneMaps, SkipKind::Cracking, SkipKind::Measured] {
            let io = Arc::new(MemoryIO::new());
            let db = Database::open_file(io, ":memory:", Arc::new(SqliteDialect)).unwrap();
            let open = || {
                db.connect(ConnectOpts::new().with_versioned_storage())
                    .unwrap()
            };
            let (builder, writer) = (open(), open());
            db.skipping.set_policy(BuildPolicy::Stale);
            builder
                .execute("CREATE TABLE t (a INTEGER, pad TEXT)")
                .unwrap();
            builder.execute("BEGIN").unwrap();
            for i in 0..3000 {
                builder
                    .execute(format!("INSERT INTO t VALUES ({i}, '{}')", "x".repeat(40)))
                    .unwrap();
            }
            builder.execute("COMMIT").unwrap();
            writer.execute("BEGIN").unwrap();
            writer
                .execute("INSERT INTO t VALUES (110, 'open')")
                .unwrap();
            set_skipping(&builder, "t", "a", Some(kind)).unwrap();
            let counts = skipping_counts(&builder, "t").unwrap();
            assert_eq!(counts.built_stale, 1, "{kind:?}");
            writer.execute("COMMIT").unwrap();
            let mut expected: Vec<i64> = (101..=121).collect();
            expected.push(3001);
            let (got, stale_read) = rowids(&builder, sql);
            assert_eq!(got, expected, "{kind:?}");
            assert!(
                stale_read >= 3001,
                "{kind:?}: a stale summary's scan read {stale_read} rows, not every row"
            );
            assert_eq!(
                skipping_candidates(&builder, "t", 100, 120),
                Err(SkipRefusal::Stale),
                "{kind:?}"
            );
            // Built again with no writer open: it holds the row and is consulted.
            set_skipping(&builder, "t", "a", Some(kind)).unwrap();
            let (got, read) = rowids(&builder, sql);
            assert_eq!(got, expected, "{kind:?}");
            assert!(
                read < stale_read,
                "{kind:?}: the rebuilt summary's scan read {read} rows"
            );
            let counts = skipping_counts(&builder, "t").unwrap();
            assert_eq!((counts.built_stale, counts.consulted), (0, 1), "{kind:?}");
            assert_eq!(db.skipping.wait_counts(), (0, 0), "{kind:?}");
        }
    }

    /// Do: as `t_ns_2_1_a_build_beside_an_open_writer_is_kept_stale`, under `BuildPolicy::Wait`
    /// (`AMPHORA_SKIP_BUILD=wait`, the A/B's other arm): one connection inserts a matching row
    /// and stays open; another builds a summary, on a thread of its own. Expect: the build waits
    /// (its failure mode, forced: blocked behind the open transaction for as long as it stays
    /// open), and is woken by the writer's commit, not a timer; it then holds the committed row,
    /// is not stale, and the filter is answered by it, reading fewer rows than the table's.
    #[test]
    fn t_ns_2_1_a_waiting_build_waits_for_the_open_writer() {
        let sql = "SELECT rowid FROM t WHERE a BETWEEN 100 AND 120 ORDER BY rowid";
        for kind in [SkipKind::ZoneMaps, SkipKind::Cracking, SkipKind::Measured] {
            let io = Arc::new(MemoryIO::new());
            let db = Database::open_file(io, ":memory:", Arc::new(SqliteDialect)).unwrap();
            let open = || {
                db.connect(ConnectOpts::new().with_versioned_storage())
                    .unwrap()
            };
            let (builder, writer) = (open(), open());
            db.skipping.set_policy(BuildPolicy::Wait);
            builder
                .execute("CREATE TABLE t (a INTEGER, pad TEXT)")
                .unwrap();
            builder.execute("BEGIN").unwrap();
            for i in 0..3000 {
                builder
                    .execute(format!("INSERT INTO t VALUES ({i}, '{}')", "x".repeat(40)))
                    .unwrap();
            }
            builder.execute("COMMIT").unwrap();
            let root = {
                let mut stmt = builder
                    .prepare("SELECT rootpage FROM sqlite_schema WHERE name = 't'")
                    .unwrap();
                match stmt
                    .run_collect_rows()
                    .unwrap()
                    .first()
                    .and_then(|r| r.first())
                {
                    Some(Value::Numeric(crate::Numeric::Integer(root))) => *root,
                    other => panic!("no root page for t: {other:?}"),
                }
            };
            writer.execute("BEGIN").unwrap();
            writer
                .execute("INSERT INTO t VALUES (110, 'open')")
                .unwrap();
            let build = {
                let db = db.clone();
                std::thread::spawn(move || {
                    let builder = db
                        .connect(ConnectOpts::new().with_versioned_storage())
                        .unwrap();
                    set_skipping(&builder, "t", "a", Some(kind))
                })
            };
            // The build's own event: it waits on the table once it found the writer open. Until
            // the writer ends nothing can wake it, so it stays unfinished however long this runs.
            let switch = writer.versioned_for(crate::MAIN_DB_ID).unwrap();
            while switch.waiting_on(root).unwrap() == 0 {
                std::thread::yield_now();
            }
            assert!(
                !build.is_finished(),
                "{kind:?}: the build ran beside the open writer"
            );
            writer.execute("COMMIT").unwrap();
            build.join().unwrap().unwrap();
            assert_eq!(db.skipping.wait_counts(), (1, 0), "{kind:?}");
            let mut expected: Vec<i64> = (101..=121).collect();
            expected.push(3001);
            let (got, read) = rowids(&builder, sql);
            assert_eq!(got, expected, "{kind:?}");
            assert!(
                read < 3001,
                "{kind:?}: the scan read {read} rows, every row"
            );
            let counts = skipping_counts(&builder, "t").unwrap();
            assert_eq!((counts.built_stale, counts.consulted), (0, 1), "{kind:?}");
        }
    }

    /// A versioned database under `BuildPolicy::Wait` with table `t` of 3000 rows (`a` 0..3000):
    /// the database, a connection, and the table's root page (lane g7s2-17's tests).
    fn waiting_table() -> (Arc<Database>, Arc<Connection>, i64) {
        let io = Arc::new(MemoryIO::new());
        let db = Database::open_file(io, ":memory:", Arc::new(SqliteDialect)).unwrap();
        db.skipping.set_policy(BuildPolicy::Wait);
        let conn = db
            .connect(ConnectOpts::new().with_versioned_storage())
            .unwrap();
        conn.execute("CREATE TABLE t (a INTEGER, pad TEXT)")
            .unwrap();
        conn.execute("BEGIN").unwrap();
        for i in 0..3000 {
            conn.execute(format!("INSERT INTO t VALUES ({i}, '{}')", "x".repeat(40)))
                .unwrap();
        }
        conn.execute("COMMIT").unwrap();
        let mut stmt = conn
            .prepare("SELECT rootpage FROM sqlite_schema WHERE name = 't'")
            .unwrap();
        let root = match stmt
            .run_collect_rows()
            .unwrap()
            .first()
            .and_then(|r| r.first())
        {
            Some(Value::Numeric(crate::Numeric::Integer(root))) => *root,
            other => panic!("no root page for t: {other:?}"),
        };
        (db, conn, root)
    }

    const FILTER: &str = "SELECT rowid FROM t WHERE a BETWEEN 100 AND 120 ORDER BY rowid";

    /// The filter's rows once row 3001 (`a` = 110) is in.
    fn with_the_new_row() -> Vec<i64> {
        let mut expected: Vec<i64> = (101..=121).collect();
        expected.push(3001);
        expected
    }

    /// Do (lane g7s2-17, `AC-NS.2.1`): under `BuildPolicy::Wait`, this thread opens a writer of
    /// the table on one connection, inserting a row the filter matches, and builds a summary on
    /// another. Expect: the wait is refused at once as the thread's own (before, it blocked for
    /// good: the writer can only end on this thread), counted, and the build goes in as `Stale`
    /// builds it: kept stale, so once the writer commits the filter reads every row and finds
    /// the new one.
    #[test]
    fn t_ns_2_1_a_waiting_build_beside_its_own_threads_writer_goes_in_stale() {
        for kind in [SkipKind::ZoneMaps, SkipKind::Cracking, SkipKind::Measured] {
            let (db, builder, _) = waiting_table();
            let writer = db
                .connect(ConnectOpts::new().with_versioned_storage())
                .unwrap();
            writer.execute("BEGIN").unwrap();
            writer
                .execute("INSERT INTO t VALUES (110, 'open')")
                .unwrap();
            set_skipping(&builder, "t", "a", Some(kind)).unwrap();
            assert_eq!(db.skipping.wait_refusals(), (1, 0), "{kind:?}");
            assert_eq!(db.skipping.wait_counts(), (0, 0), "{kind:?}");
            let counts = skipping_counts(&builder, "t").unwrap();
            assert_eq!(counts.built_stale, 1, "{kind:?}");
            writer.execute("COMMIT").unwrap();
            let (got, read) = rowids(&builder, FILTER);
            assert_eq!(got, with_the_new_row(), "{kind:?}");
            assert!(
                read >= 3001,
                "{kind:?}: a stale summary's scan read {read} rows"
            );
        }
    }

    /// Do (lane g7s2-17, `AC-NS.2.1`): under `BuildPolicy::Wait`, another thread opens a writer
    /// of the table and keeps it open; this thread builds a summary with a deadline already
    /// passed (its statement's, `set_skipping_until`). Expect: the wait gives up at the deadline
    /// rather than block for as long as the writer stays open, typed and counted, its waiter
    /// taken out; the build goes in stale, and once the writer commits the filter finds its row.
    #[test]
    fn t_ns_2_1_a_waiting_build_gives_up_at_its_deadline() {
        for kind in [SkipKind::ZoneMaps, SkipKind::Cracking, SkipKind::Measured] {
            let (db, builder, root) = waiting_table();
            let (begun_tx, begun) = std::sync::mpsc::channel();
            let (end_tx, end) = std::sync::mpsc::channel::<()>();
            let writer = {
                let db = db.clone();
                std::thread::spawn(move || {
                    let writer = db
                        .connect(ConnectOpts::new().with_versioned_storage())
                        .unwrap();
                    writer.execute("BEGIN").unwrap();
                    writer
                        .execute("INSERT INTO t VALUES (110, 'open')")
                        .unwrap();
                    begun_tx.send(()).unwrap();
                    end.recv().unwrap();
                    writer.execute("COMMIT").unwrap();
                })
            };
            begun.recv().unwrap();
            let passed = builder.get_pager().io.current_time_monotonic();
            set_skipping_until(&builder, "t", "a", Some(kind), Some(passed)).unwrap();
            assert_eq!(db.skipping.wait_refusals(), (0, 1), "{kind:?}");
            assert_eq!(db.skipping.wait_counts(), (0, 0), "{kind:?}");
            let switch = builder.versioned_for(crate::MAIN_DB_ID).unwrap();
            assert_eq!(switch.waiting_on(root).unwrap(), 0, "{kind:?}");
            let counts = skipping_counts(&builder, "t").unwrap();
            assert_eq!(counts.built_stale, 1, "{kind:?}");
            end_tx.send(()).unwrap();
            writer.join().unwrap();
            let (got, read) = rowids(&builder, FILTER);
            assert_eq!(got, with_the_new_row(), "{kind:?}");
            assert!(
                read >= 3001,
                "{kind:?}: a stale summary's scan read {read} rows"
            );
        }
    }

    #[test]
    fn a_waiting_build_stops_rebuilding_when_commits_cross_its_deadline() {
        let (db, writer, root) = waiting_table();
        let switch = writer.versioned_for(crate::MAIN_DB_ID).unwrap();
        let (paused, go_on) = switch.pause_next_build(root);
        let build = {
            let db = db.clone();
            std::thread::spawn(move || {
                let conn = db
                    .connect(ConnectOpts::new().with_versioned_storage())
                    .unwrap();
                let deadline = conn.get_pager().io.current_time_monotonic();
                let result =
                    set_skipping_until(&conn, "t", "a", Some(SkipKind::Cracking), Some(deadline));
                conn.versioned_for(crate::MAIN_DB_ID)
                    .unwrap()
                    .clear_build_pause();
                result
            })
        };
        paused.recv().unwrap();
        writer
            .execute("INSERT INTO t VALUES (110, 'slipped in')")
            .unwrap();
        go_on.send(()).unwrap();
        build.join().unwrap().unwrap();
        assert_eq!(
            db.skipping.wait_counts(),
            (0, 0),
            "retried past the deadline"
        );
        assert_eq!(db.skipping.wait_refusals(), (0, 1));
        assert_eq!(rowids(&writer, FILTER).0, with_the_new_row());
    }

    /// Do (lane g7s2-17, `AC-NS.2.1`): under `BuildPolicy::Wait`, a build on a thread of its own
    /// is paused by the storage's hook once it has read its rows, before it checks for writers
    /// (`Switch::pause_next_build`); then a writer inserts a row the filter matches, and either
    /// stays open or commits, before the build goes on. Expect, both ways: the build found the
    /// writer it missed and is done again (counted a rebuild); left open, the build waits for it
    /// (unfinished until it commits, counted a wait); the summary at the end holds the row, is
    /// not stale, and answers the filter reading fewer rows than the table's.
    #[test]
    fn t_ns_2_1_a_writer_slipped_in_beside_a_waiting_build_is_built_again() {
        for commit_first in [false, true] {
            for kind in [SkipKind::ZoneMaps, SkipKind::Cracking, SkipKind::Measured] {
                let (db, writer, root) = waiting_table();
                let switch = writer.versioned_for(crate::MAIN_DB_ID).unwrap();
                let (paused, go_on) = switch.pause_next_build(root);
                let build = {
                    let db = db.clone();
                    std::thread::spawn(move || {
                        let builder = db
                            .connect(ConnectOpts::new().with_versioned_storage())
                            .unwrap();
                        let built = set_skipping(&builder, "t", "a", Some(kind));
                        // A build that never paused (it failed first) lets the test hear it.
                        if let Some(switch) = builder.versioned_for(crate::MAIN_DB_ID) {
                            switch.clear_build_pause();
                        }
                        built
                    })
                };
                // The build has read its rows, none of the writer's.
                paused
                    .recv()
                    .unwrap_or_else(|_| panic!("{kind:?}: the build ended without its pause"));
                writer.execute("BEGIN").unwrap();
                writer
                    .execute("INSERT INTO t VALUES (110, 'slipped in')")
                    .unwrap();
                if commit_first {
                    writer.execute("COMMIT").unwrap();
                }
                go_on.send(()).unwrap();
                if !commit_first {
                    // It found the writer open, went in stale, and waits on it: nothing but the
                    // writer's end can wake it.
                    while switch.waiting_on(root).unwrap() == 0 {
                        std::thread::yield_now();
                    }
                    assert!(
                        !build.is_finished(),
                        "{kind:?}: the rebuild ran beside the writer"
                    );
                    writer.execute("COMMIT").unwrap();
                }
                build.join().unwrap().unwrap();
                assert_eq!(
                    db.skipping.wait_counts(),
                    (u64::from(!commit_first), 1),
                    "{kind:?} commit_first={commit_first}"
                );
                assert_eq!(db.skipping.wait_refusals(), (0, 0), "{kind:?}");
                let (got, read) = rowids(&writer, FILTER);
                assert_eq!(
                    got,
                    with_the_new_row(),
                    "{kind:?} commit_first={commit_first}"
                );
                assert!(
                    read < 3001,
                    "{kind:?}: the scan read {read} rows, every row"
                );
                let counts = skipping_counts(&writer, "t").unwrap();
                assert_eq!(
                    (counts.built_stale, counts.consulted),
                    (0, 1),
                    "{kind:?} commit_first={commit_first}"
                );
            }
        }
    }

    /// The A/B of stale against wait (G7 step 2, `AC-NS.2.1`; lane g7s2-16), the harness of the
    /// coordinator's run of record: `AMPHORA_SKIP_AB_WRITERS` connections (the ladder 1, 8, 32,
    /// 128, 512, 2048, 4096; 2 unless set) each commit `AMPHORA_SKIP_AB_TXNS` single-row inserts
    /// (16 unless set) into one table, while one connection builds its zone-map summary
    /// `AMPHORA_SKIP_AB_BUILDS` times (4 unless set) and after each build runs a filter. Each arm
    /// reports its builds' latency (p50, p99, p99.9, max), the builds kept stale, the waits and
    /// rebuilds, and the rows the filters read (a stale summary's filter reads every row).
    /// Expect, on each arm: every filter answers as a scan without the summary would (its rows
    /// counted from the table), and every build ends once the writers do.
    #[test]
    fn t_ns_2_1_stale_against_wait_under_writers() {
        let env = |name: &str, default: usize| {
            std::env::var(name)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        };
        let (writers, txns, builds) = (
            env("AMPHORA_SKIP_AB_WRITERS", 2),
            env("AMPHORA_SKIP_AB_TXNS", 16),
            env("AMPHORA_SKIP_AB_BUILDS", 4),
        );
        let sql = "SELECT count(*) FROM t WHERE a BETWEEN 100 AND 120";
        for policy in [BuildPolicy::Stale, BuildPolicy::Wait] {
            let io = Arc::new(MemoryIO::new());
            let db = Database::open_file(io, ":memory:", Arc::new(SqliteDialect)).unwrap();
            db.skipping.set_policy(policy);
            let builder = db
                .connect(ConnectOpts::new().with_versioned_storage())
                .unwrap();
            builder
                .execute("CREATE TABLE t (a INTEGER, pad TEXT)")
                .unwrap();
            builder.execute("BEGIN").unwrap();
            for i in 0..3000 {
                builder
                    .execute(format!(
                        "INSERT INTO t VALUES ({}, '{}')",
                        i % 1000,
                        "x".repeat(40)
                    ))
                    .unwrap();
            }
            builder.execute("COMMIT").unwrap();
            let next = Arc::new(std::sync::atomic::AtomicU64::new(3001));
            let threads: Vec<_> = (0..writers)
                .map(|w| {
                    let (db, next) = (db.clone(), next.clone());
                    std::thread::spawn(move || {
                        let conn = db
                            .connect(ConnectOpts::new().with_versioned_storage())
                            .unwrap();
                        for t in 0..txns {
                            let id = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let a = (w * 31 + t * 7) % 1000;
                            conn.execute(format!(
                                "INSERT INTO t (rowid, a, pad) VALUES ({id}, {a}, 'w')"
                            ))
                            .unwrap();
                        }
                    })
                })
                .collect();
            let (mut latencies, mut read, mut stale) = (Vec::new(), 0u64, 0u64);
            for _ in 0..builds {
                let started = std::time::Instant::now();
                set_skipping(&builder, "t", "a", Some(SkipKind::ZoneMaps)).unwrap();
                latencies.push(started.elapsed().as_nanos());
                stale += skipping_counts(&builder, "t").map_or(0, |c| c.built_stale);
                // The filter and the plain scan at one snapshot: writers commit meanwhile.
                builder.execute("BEGIN").unwrap();
                let mut stmt = builder.prepare(sql).unwrap();
                let got = stmt.run_collect_rows().unwrap();
                read += stmt.metrics().rows_read;
                let mut plain = builder.prepare("SELECT a FROM t").unwrap();
                let want = plain
                    .run_collect_rows()
                    .unwrap()
                    .iter()
                    .filter(|row| matches!(row.first(), Some(Value::Numeric(crate::Numeric::Integer(a))) if (100..=120).contains(a)))
                    .count();
                builder.execute("COMMIT").unwrap();
                assert_eq!(
                    got,
                    vec![vec![Value::from_i64(i64::try_from(want).unwrap())]],
                    "{policy:?}"
                );
            }
            for thread in threads {
                thread.join().unwrap();
            }
            latencies.sort_unstable();
            // The latency at `per_mille` of the builds (nearest rank).
            let at = |per_mille: usize| {
                let i = latencies.len().saturating_sub(1) * per_mille / 1000;
                latencies.get(i).copied().unwrap_or(0)
            };
            let (waits, rebuilds) = db.skipping.wait_counts();
            eprintln!(
                "skip_ab policy={policy:?} writers={writers} txns={txns} builds={builds} build_ns_p50={} p99={} p999={} max={} built_stale={stale} waits={waits} rebuilds={rebuilds} filter_rows_read={read}",
                at(500),
                at(990),
                at(999),
                latencies.last().copied().unwrap_or(0)
            );
        }
    }
}
