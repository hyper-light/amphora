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
                assert_eq!(found, plain(&table, lo, hi), "seed {seed}, filter {lo}..={hi}");
            }
            let Skipping::ZoneMaps(zones) = &skipping else {
                unreachable!()
            };
            for (&rowid, &value) in &table {
                assert!(zones.covers(rowid, value), "seed {seed}: row {rowid} = {value:?} outside its zone");
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
        let mut skipping = Skipping::Cracking(CrackerColumn::new(table.iter().map(|(r, v)| (*r, *v))));
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
                    assert_eq!(table.get(rowid), Some(&Some(*value)), "seed {seed}: stale entry for row {rowid}");
                }
                entries.sort_by_key(|(_, rowid)| *rowid);
                let rowids: Vec<i64> = entries.iter().map(|(_, rowid)| *rowid).collect();
                assert_eq!(rowids, expected, "seed {seed}: entries for {lo}..={hi}");
            }
        }
        let Skipping::Cracking(cracker) = &skipping else {
            unreachable!()
        };
        // Every row the table holds is in the column once: integers as entries, others apart.
        let mut held: Vec<i64> = cracker.matching_entries(i64::MIN, i64::MAX).unwrap().iter().map(|(_, r)| *r).collect();
        held.extend(cracker.others());
        held.sort_unstable();
        assert_eq!(held, table.keys().copied().collect::<Vec<_>>(), "seed {seed}");
        cracks += cracker.stats().cracks;
        queries += cracker.stats().queries;
        moved += cracker.stats().entries_moved;
    }
    assert!(cracks > 0 && cracks < 2 * queries, "cracks {cracks} for {queries} queries");
    assert!(moved > 0, "no write ever rippled");
}

/// Do: hand the hook writes with the switch off, and ask for candidates. Expect: nothing kept,
/// every row a candidate.
#[test]
fn t_g5_4_1_switch_off_keeps_nothing() {
    let mut skipping = Skipping::Off;
    skipping.on_write(RowWrite::Insert { rowid: 1, value: Some(1) }).unwrap();
    assert_eq!(skipping.candidates(0, 0).unwrap(), Candidates::All);
}

/// Do: remove a row neither summary holds; build from rowids out of order. Expect: typed refusals.
#[test]
fn t_g5_4_1_disagreements_are_refused() {
    let mut zones = ZoneMapBuilder::new().finish();
    assert_eq!(
        zones.on_write(RowWrite::Delete { rowid: 3, old: Some(1) }),
        Err(SkipError::MissingRow { rowid: 3, value: Some(1) })
    );
    let mut cracker = CrackerColumn::new([(1, Some(5))]);
    assert_eq!(
        cracker.on_write(RowWrite::Delete { rowid: 1, old: Some(6) }),
        Err(SkipError::MissingRow { rowid: 1, value: Some(6) })
    );
    assert_eq!(
        cracker.on_write(RowWrite::Delete { rowid: 2, old: None }),
        Err(SkipError::MissingRow { rowid: 2, value: None })
    );
    let mut builder = ZoneMapBuilder::new();
    builder.row(5, Some(1)).unwrap();
    assert_eq!(builder.row(5, Some(2)), Err(SkipError::RowidOrder { previous: 5, rowid: 5 }));
}
