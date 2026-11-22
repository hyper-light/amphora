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
            Chooser::with_upkeep(builder.finish(), copy(&table), upkeep, [build, build]);
        let mut total = 0u64;
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
                    }
                    Some(SkipMethod::Cracking) => {
                        chooser.install_cracker(copy(&table), n(table.len()));
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
            }
        }
        UpkeepRun {
            stats: chooser.stats(),
            upkeep: total,
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
    /// comes due; kept both, nothing is dropped; every answer is sound.
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
            let capped = upkeep_run(seed, Upkeep::Capped, &phases);
            let kept = upkeep_run(seed, Upkeep::KeepBoth, &phases);
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
        conn.execute("CREATE TABLE t (a INTEGER, pad TEXT)").unwrap();
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
            assert!(read >= 3001, "{kind:?}: a stale summary's scan read {read} rows");
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
            assert!(read >= 3001, "{kind:?}: a stale summary's scan read {read} rows");
        }
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
                        set_skipping(&builder, "t", "a", Some(kind))
                    })
                };
                // The build has read its rows, none of the writer's.
                paused.recv().unwrap();
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
                    assert!(!build.is_finished(), "{kind:?}: the rebuild ran beside the writer");
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
                assert_eq!(got, with_the_new_row(), "{kind:?} commit_first={commit_first}");
                assert!(read < 3001, "{kind:?}: the scan read {read} rows, every row");
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
