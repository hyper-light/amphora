//! T-QT.6.20 (lane qs6-13): the A/B of a simple join's latency, old path against new. Lane
//! qs6-12 replaced the join workers' direct fold over the joined row's columns with the copy of
//! the serial loop's body, run on every joined row; this harness times simple two-table
//! equi-joins with aggregates (and one returning rows) on whichever commit it is built on, at a
//! ladder of stated cores, so the same file run on 054ace65 (the fold) and on the lane's branch
//! (the body) gives the two sides of one comparison: one driver, one machine, one timing method.
//!
//! It is a harness, not a check: `#[ignore]`d, run by name with `--ignored --nocapture`, and its
//! sizes are the run's to state (`AMPHORA_AB_PROBE_ROWS`, `AMPHORA_AB_BUILD_ROWS`,
//! `AMPHORA_AB_RUNS`, `AMPHORA_AB_CORES`, `AMPHORA_AB_LABEL`); the defaults are a tiny instance
//! for a smoke run, not a measurement. Each line it prints is a TSV record: label, cores, query,
//! runs, p50, p99 and p99.9 in microseconds, and the hash tables the runs built (0 would mean the
//! parallel join never ran, and the line says nothing about it).

use crate::io::MemoryIO;
use crate::vdbe::morsel::{MachineCosts, ParallelMode};
use crate::{ConnectOpts, Connection, Database, DatabaseOpts, OpenFlags, SqliteDialect};
use std::num::NonZeroUsize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

fn open(name: &str) -> Arc<Connection> {
    let io = Arc::new(MemoryIO::new());
    let db = Database::open_file_with_flags(
        io,
        name,
        OpenFlags::default(),
        DatabaseOpts::new(),
        None,
        Arc::new(SqliteDialect),
    )
    .unwrap();
    db.connect(ConnectOpts::new()).unwrap()
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// `name(k INTEGER, g INTEGER, v INTEGER)` of `rows` rows: `k` cycles over `keys` values, `g`
/// over a few groups, `v` the row's number (INTEGERs only, so the body path never refuses a REAL).
fn fill(conn: &Arc<Connection>, name: &str, rows: usize, keys: usize) {
    conn.execute(format!(
        "CREATE TABLE {name}(k INTEGER, g INTEGER, v INTEGER)"
    ))
    .unwrap();
    conn.execute("BEGIN").unwrap();
    let batch = 500usize;
    let mut at = 0usize;
    while at < rows {
        let end = (at + batch).min(rows);
        let values: Vec<String> = (at..end)
            .map(|i| format!("({}, {}, {i})", i % keys.max(1), i % 13))
            .collect();
        conn.execute(format!(
            "INSERT INTO {name}(k, g, v) VALUES {}",
            values.join(", ")
        ))
        .unwrap();
        at = end;
    }
    conn.execute("COMMIT").unwrap();
}

/// `name(k INTEGER, g INTEGER, v INTEGER)` of `rows` rows whose keys are unique: row `i`'s `k` is
/// `i + shift`, so two such tables match on as many keys as their ranges share (T-QT.6.26).
fn fill_shifted(conn: &Arc<Connection>, name: &str, rows: usize, shift: usize) {
    conn.execute(format!(
        "CREATE TABLE {name}(k INTEGER, g INTEGER, v INTEGER)"
    ))
    .unwrap();
    conn.execute("BEGIN").unwrap();
    let batch = 500usize;
    let mut at = 0usize;
    while at < rows {
        let end = (at + batch).min(rows);
        let values: Vec<String> = (at..end)
            .map(|i| format!("({}, {}, {i})", i + shift, i % 13))
            .collect();
        conn.execute(format!(
            "INSERT INTO {name}(k, g, v) VALUES {}",
            values.join(", ")
        ))
        .unwrap();
        at = end;
    }
    conn.execute("COMMIT").unwrap();
}

/// The `q`-quantile of sorted `samples` (nearest rank).
fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = ((q * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted.get(rank - 1).copied().unwrap_or(f64::NAN)
}

#[test]
#[ignore = "a harness: run by name with --ignored --nocapture"]
fn t_qt_6_20_simple_join_latency_old_against_new() {
    let probe_rows = env_usize("AMPHORA_AB_PROBE_ROWS", 4000);
    let build_rows = env_usize("AMPHORA_AB_BUILD_ROWS", 400);
    let runs = env_usize("AMPHORA_AB_RUNS", 20).max(1);
    let label = std::env::var("AMPHORA_AB_LABEL").unwrap_or_else(|_| "unlabelled".into());
    let cores: Vec<usize> = std::env::var("AMPHORA_AB_CORES")
        .unwrap_or_else(|_| "1,2,4,8,32".into())
        .split(',')
        .filter_map(|c| c.trim().parse().ok())
        .filter(|c| *c > 0)
        .collect();
    let conn = open(":memory:qt-6-20");
    fill(&conn, "t", probe_rows, build_rows);
    fill(&conn, "u", build_rows, build_rows);
    conn.set_join_methods(crate::JoinMethods::HashJoinsOnly);
    let queries = [
        ("count", "SELECT count(*) FROM t JOIN u ON t.k = u.k"),
        (
            "group_sum",
            "SELECT u.g, count(*), sum(t.v) FROM t JOIN u ON t.k = u.k GROUP BY u.g",
        ),
        (
            "filter_min_max",
            "SELECT min(t.v), max(u.v), sum(t.v) FROM t JOIN u ON t.k = u.k WHERE t.g < 9",
        ),
        ("rows", "SELECT t.v, u.v FROM t JOIN u ON t.k = u.k"),
    ];
    // The machine's own measured costs of a worker and a dispatch, stated at each rung's cores, so
    // the cost model decides the workers as it would here (the same on both sides of the A/B).
    let measured = MachineCosts::measure().unwrap();
    let worker_start = measured.worker_start().mean().unwrap_or(0.0);
    let dispatch = measured.dispatch().mean().unwrap_or(0.0);
    println!("label\tcores\tquery\truns\tp50_us\tp99_us\tp999_us\tlinks");
    for (name, sql) in queries {
        conn.set_parallel_mode(ParallelMode::Off);
        let serial = conn.prepare(sql).unwrap().run_collect_rows().unwrap();
        conn.set_parallel_mode(ParallelMode::On);
        // Lane qs6-13's switch (T-QT.6.21): each rung with the body run on every joined row and
        // with a body of nothing but reads copied; this side of the A/B only (054ace65 has no
        // switch, its fold path its one).
        for (direct, &c) in [false, true]
            .into_iter()
            .flat_map(|d| cores.iter().map(move |c| (d, c)))
        {
            conn.set_parallel_join_direct(direct);
            let label = format!("{label}-{}", if direct { "direct" } else { "body" });
            *conn.morsel_costs.lock() = Some(MachineCosts::stated(
                NonZeroUsize::new(c).unwrap(),
                worker_start,
                dispatch,
            ));
            let before = conn.morsel_join_links.load(Ordering::Relaxed);
            let mut samples = Vec::with_capacity(runs);
            for run in 0..runs {
                let began = Instant::now();
                let rows = conn.prepare(sql).unwrap().run_collect_rows().unwrap();
                samples.push(began.elapsed().as_secs_f64() * 1e6);
                if run == 0 {
                    // The A/B is of equal answers only: rows compared as sets (no ORDER BY).
                    let (mut a, mut b) = (rows.clone(), serial.clone());
                    a.sort_by(|x, y| format!("{x:?}").cmp(&format!("{y:?}")));
                    b.sort_by(|x, y| format!("{x:?}").cmp(&format!("{y:?}")));
                    assert_eq!(a, b, "{sql} on {c} cores");
                }
            }
            let links = conn
                .morsel_join_links
                .load(Ordering::Relaxed)
                .saturating_sub(before);
            samples.sort_by(f64::total_cmp);
            println!(
                "{label}\t{c}\t{name}\t{runs}\t{:.1}\t{:.1}\t{:.1}\t{links}",
                quantile(&samples, 0.5),
                quantile(&samples, 0.99),
                quantile(&samples, 0.999)
            );
        }
    }
}

/// T-QT.6.26 (lane qs6-16; step 3 of lane qs6-14's plan for unifying the outer-join paths): the
/// extremes that tell a two-table outer join's two orientations apart ([`crate::OuterJoinProbe`]).
/// `Padded` builds its hash table on the kept table and marks its matched rows in a bitmap per
/// worker; `Kept` builds on the padded table and pads during the walk. So each scenario forces one
/// side's failure mode: the hash table and bitmaps sized by a huge kept table (`tiny_padded`), the
/// hash table of a huge padded one (`huge_padded`), the bitmaps' merge and the rest pass with
/// nothing left to pad (`all_matched`: every key but one on each side matches) or with all of it
/// (`none_matched`: one key matches). Tables `k` (kept) and `p` (padded) have unique keys.
///
/// A harness, as T-QT.6.20: `#[ignore]`d, its sizes the run's (`AMPHORA_AB_BIG_ROWS`,
/// `AMPHORA_AB_SMALL_ROWS`, `AMPHORA_AB_RUNS`, `AMPHORA_AB_CORES`, `AMPHORA_AB_LABEL`), the
/// defaults a tiny instance. Each line is a TSV record per scenario, query, orientation and stated
/// cores: p50, p99 and p99.9 in microseconds; the statements that ran each path and on the serial
/// loop (a budget refusal falls back to it); per statement, the rows padded in the walk and after
/// the bitmaps merged and the bitmap words; and the bytes charged to the statement's memory budget
/// (hash tables, bitmaps, partials), the mean and the most. Each rung's first answer is checked
/// against the serial loop's.
#[test]
#[ignore = "a harness: run by name with --ignored --nocapture"]
fn t_qt_6_26_outer_join_orientation_extremes() {
    use std::sync::atomic::AtomicU64;
    let big = env_usize("AMPHORA_AB_BIG_ROWS", 4000).max(2);
    let small = env_usize("AMPHORA_AB_SMALL_ROWS", 40).max(1);
    let runs = env_usize("AMPHORA_AB_RUNS", 20).max(1);
    let label = std::env::var("AMPHORA_AB_LABEL").unwrap_or_else(|_| "unlabelled".into());
    let cores: Vec<usize> = std::env::var("AMPHORA_AB_CORES")
        .unwrap_or_else(|_| "1,2,4,8,32".into())
        .split(',')
        .filter_map(|c| c.trim().parse().ok())
        .filter(|c| *c > 0)
        .collect();
    // (scenario, kept rows, padded rows, the padded keys' shift)
    let scenarios = [
        ("tiny_padded", big, small, 0),
        ("huge_padded", small, big, 0),
        ("all_matched", big, big, 1),
        ("none_matched", big, big, big - 1),
    ];
    let queries = [
        (
            "left_count",
            "SELECT count(*), count(p.v), sum(k.v) FROM k LEFT JOIN p ON k.k = p.k",
        ),
        (
            "full_count",
            "SELECT count(*), count(k.v), count(p.v) FROM k FULL JOIN p ON k.k = p.k",
        ),
        (
            "left_group",
            "SELECT k.g, count(p.v), sum(p.v) FROM k LEFT JOIN p ON k.k = p.k GROUP BY k.g",
        ),
        (
            "left_rows",
            "SELECT k.v, p.v FROM k LEFT JOIN p ON k.k = p.k",
        ),
    ];
    let measured = MachineCosts::measure().unwrap();
    let worker_start = measured.worker_start().mean().unwrap_or(0.0);
    let dispatch = measured.dispatch().mean().unwrap_or(0.0);
    println!(
        "label\tscenario\tkept_rows\tpadded_rows\tquery\tprobe\tcores\truns\tp50_us\tp99_us\tp999_us\t\
         padded_probes\tchains\tserial\twalk_pads\tbitmap_pads\tbitmap_words\tcharged_mean_b\tcharged_max_b"
    );
    for (scenario, kept_rows, padded_rows, shift) in scenarios {
        let conn = open(&format!(":memory:qt-6-26-{scenario}"));
        fill_shifted(&conn, "k", kept_rows, 0);
        fill_shifted(&conn, "p", padded_rows, shift);
        conn.set_join_methods(crate::JoinMethods::HashJoinsOnly);
        for (name, sql) in queries {
            conn.set_parallel_mode(ParallelMode::Off);
            let mut serial = conn.prepare(sql).unwrap().run_collect_rows().unwrap();
            serial.sort_by(|x, y| format!("{x:?}").cmp(&format!("{y:?}")));
            conn.set_parallel_mode(ParallelMode::On);
            for (probe, &c) in [crate::OuterJoinProbe::Padded, crate::OuterJoinProbe::Kept]
                .into_iter()
                .flat_map(|p| cores.iter().map(move |c| (p, c)))
            {
                conn.set_parallel_outer_probe(probe);
                *conn.morsel_costs.lock() = Some(MachineCosts::stated(
                    NonZeroUsize::new(c).unwrap(),
                    worker_start,
                    dispatch,
                ));
                let o = &conn.morsel_outer;
                let read = |n: &AtomicU64| n.load(Ordering::Relaxed);
                let counters = || {
                    [
                        read(&o.padded_probes),
                        read(&o.chains),
                        read(&o.walk_pads),
                        read(&o.bitmap_pads),
                        read(&o.bitmap_words),
                        read(&o.charged_bytes),
                    ]
                };
                o.charged_peak.store(0, Ordering::Relaxed);
                let before = counters();
                let mut samples = Vec::with_capacity(runs);
                for run in 0..runs {
                    let began = Instant::now();
                    let mut rows = conn.prepare(sql).unwrap().run_collect_rows().unwrap();
                    samples.push(began.elapsed().as_secs_f64() * 1e6);
                    if run == 0 {
                        // Equal answers only: rows compared as sets (no ORDER BY).
                        rows.sort_by(|x, y| format!("{x:?}").cmp(&format!("{y:?}")));
                        assert_eq!(rows, serial, "{scenario}: {sql} at {probe:?} on {c} cores");
                    }
                }
                let after = counters();
                let [padded_probes, chains, walk_pads, bitmap_pads, bitmap_words, charged] =
                    [0, 1, 2, 3, 4, 5].map(|at| after[at].saturating_sub(before[at]));
                let ran = padded_probes.saturating_add(chains);
                let per = |total: u64| total / ran.max(1);
                samples.sort_by(f64::total_cmp);
                println!(
                    "{label}\t{scenario}\t{kept_rows}\t{padded_rows}\t{name}\t{probe:?}\t{c}\t{runs}\t\
                     {:.1}\t{:.1}\t{:.1}\t{padded_probes}\t{chains}\t{}\t{}\t{}\t{}\t{}\t{}",
                    quantile(&samples, 0.5),
                    quantile(&samples, 0.99),
                    quantile(&samples, 0.999),
                    u64::try_from(runs).unwrap_or(u64::MAX).saturating_sub(ran),
                    per(walk_pads),
                    per(bitmap_pads),
                    per(bitmap_words),
                    per(charged),
                    o.charged_peak.load(Ordering::Relaxed),
                );
            }
        }
    }
}

/// `name(id INTEGER PRIMARY KEY, k INTEGER, g INTEGER, v INTEGER)` of `rows` rows, ids 1 to
/// `rows`; `k` cycles over `1..=keys`, a foreign key into another such table of `keys` rows
/// (T-QT.6.28).
fn fill_keyed(conn: &Arc<Connection>, name: &str, rows: usize, keys: usize) {
    conn.execute(format!(
        "CREATE TABLE {name}(id INTEGER PRIMARY KEY, k INTEGER, g INTEGER, v INTEGER)"
    ))
    .unwrap();
    conn.execute("BEGIN").unwrap();
    let batch = 500usize;
    let mut at = 0usize;
    while at < rows {
        let end = (at + batch).min(rows);
        let values: Vec<String> = (at..end)
            .map(|i| format!("({}, {}, {}, {i})", i + 1, i % keys.max(1) + 1, i % 13))
            .collect();
        conn.execute(format!(
            "INSERT INTO {name}(id, k, g, v) VALUES {}",
            values.join(", ")
        ))
        .unwrap();
        at = end;
    }
    conn.execute("COMMIT").unwrap();
}

/// T-QT.6.28 (lane qs6-16): the extremes that tell the candidates of
/// [`crate::SearchedLinks`] apart, a join planned by cost onto a primary key (the planner reads
/// the joined table by a rowid seek per probe row). `Off`, the control, is the serial loop; `Hash`
/// reads the joined table whole into a hash link, so a tiny probe against a huge table
/// (`tiny_probe`) is its failure mode; `Seek` seeks per probe row, so a huge probe against a tiny
/// table (`huge_probe`, every seek into the same few pages, against a hash table that fits in
/// cache) is its; `equal` sits between. The probe `f` has `AMPHORA_AB_BIG_ROWS` rows or
/// `AMPHORA_AB_SMALL_ROWS`, the joined table `d` the other.
///
/// A harness, as T-QT.6.20: `#[ignore]`d, its sizes the run's (`AMPHORA_AB_BIG_ROWS`,
/// `AMPHORA_AB_SMALL_ROWS`, `AMPHORA_AB_RUNS`, `AMPHORA_AB_CORES`, `AMPHORA_AB_LABEL`), the defaults
/// a tiny instance. Each line is a TSV record per scenario, query, setting and stated cores: p50,
/// p99 and p99.9 in microseconds, and per statement the links built from searched tables and the
/// ones read by seeks (0 at `Hash` or `Seek`: the planner chose another loop than a seek, or the
/// serial loop answered). Each rung's first answer is checked against the serial loop's.
#[test]
#[ignore = "a harness: run by name with --ignored --nocapture"]
fn t_qt_6_28_searched_link_extremes() {
    let big = env_usize("AMPHORA_AB_BIG_ROWS", 4000).max(1);
    let small = env_usize("AMPHORA_AB_SMALL_ROWS", 40).max(1);
    let runs = env_usize("AMPHORA_AB_RUNS", 20).max(1);
    let label = std::env::var("AMPHORA_AB_LABEL").unwrap_or_else(|_| "unlabelled".into());
    let cores: Vec<usize> = std::env::var("AMPHORA_AB_CORES")
        .unwrap_or_else(|_| "1,2,4,8,32".into())
        .split(',')
        .filter_map(|c| c.trim().parse().ok())
        .filter(|c| *c > 0)
        .collect();
    // (scenario, probe rows, joined rows)
    let scenarios = [
        ("tiny_probe", small, big),
        ("huge_probe", big, small),
        ("equal", big, big),
    ];
    let queries = [
        (
            "count",
            "SELECT count(*), sum(d.v) FROM f JOIN d ON f.k = d.id",
        ),
        (
            "group_sum",
            "SELECT d.g, count(*), sum(f.v) FROM f JOIN d ON f.k = d.id GROUP BY d.g",
        ),
        ("rows", "SELECT f.v, d.v FROM f JOIN d ON f.k = d.id"),
    ];
    let measured = MachineCosts::measure().unwrap();
    let worker_start = measured.worker_start().mean().unwrap_or(0.0);
    let dispatch = measured.dispatch().mean().unwrap_or(0.0);
    println!(
        "label\tscenario\tprobe_rows\tjoined_rows\tquery\tlinks\tcores\truns\tp50_us\tp99_us\tp999_us\t\
         searched_per_stmt\tseeks_per_stmt"
    );
    for (scenario, probe_rows, joined_rows) in scenarios {
        let conn = open(&format!(":memory:qt-6-28-{scenario}"));
        fill_keyed(&conn, "d", joined_rows, joined_rows);
        fill_keyed(&conn, "f", probe_rows, joined_rows);
        for (name, sql) in queries {
            conn.set_parallel_mode(ParallelMode::Off);
            let mut serial = conn.prepare(sql).unwrap().run_collect_rows().unwrap();
            serial.sort_by(|x, y| format!("{x:?}").cmp(&format!("{y:?}")));
            conn.set_parallel_mode(ParallelMode::On);
            for (links, &c) in [
                crate::SearchedLinks::Off,
                crate::SearchedLinks::Hash,
                crate::SearchedLinks::Seek,
            ]
            .into_iter()
            .flat_map(|l| cores.iter().map(move |c| (l, c)))
            {
                conn.set_parallel_searched_links(links);
                *conn.morsel_costs.lock() = Some(MachineCosts::stated(
                    NonZeroUsize::new(c).unwrap(),
                    worker_start,
                    dispatch,
                ));
                let counts = || {
                    [&conn.morsel_searched_links, &conn.morsel_seek_links]
                        .map(|n| n.load(Ordering::Relaxed))
                };
                let before = counts();
                let mut samples = Vec::with_capacity(runs);
                for run in 0..runs {
                    let began = Instant::now();
                    let mut rows = conn.prepare(sql).unwrap().run_collect_rows().unwrap();
                    samples.push(began.elapsed().as_secs_f64() * 1e6);
                    if run == 0 {
                        rows.sort_by(|x, y| format!("{x:?}").cmp(&format!("{y:?}")));
                        assert_eq!(rows, serial, "{scenario}: {sql} at {links:?} on {c} cores");
                    }
                }
                let after = counts();
                let per = |at: usize| after[at].saturating_sub(before[at]) as f64 / runs as f64;
                samples.sort_by(f64::total_cmp);
                println!(
                    "{label}\t{scenario}\t{probe_rows}\t{joined_rows}\t{name}\t{links:?}\t{c}\t{runs}\t\
                     {:.1}\t{:.1}\t{:.1}\t{:.2}\t{:.2}",
                    quantile(&samples, 0.5),
                    quantile(&samples, 0.99),
                    quantile(&samples, 0.999),
                    per(0),
                    per(1),
                );
            }
        }
        conn.set_parallel_searched_links(crate::SearchedLinks::Off);
    }
}
