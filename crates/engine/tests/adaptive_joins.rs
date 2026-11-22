//! Amphora (PL.13, T-PL.13.1; `docs/design/planner.md`, "Decision (lane pl13)"): the adaptive
//! join on the real engine. Random tables with NULL and duplicate keys are joined with the switch
//! on and off (`Connection::set_adaptive_joins`); the answers must be identical, and equal to the
//! nested loop computed here from the rows. The switch's count
//! (`Connection::adaptive_join_switches`) must move when the outer rows reach the switch point the
//! statement's plan states, and not before.

use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, JoinMethods, Numeric, OpenOptions, PlatformIO,
    SqliteDialect, StepResult, Value,
};
use std::sync::Arc;

fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// A key from 0 to `keys - 1`, or NULL about one row in eleven.
fn key(state: &mut u64, keys: u64) -> Option<i64> {
    let draw = next(state) % (keys * 11 / 10 + 1);
    (draw < keys).then_some(draw as i64)
}

fn sql_value(v: Option<i64>) -> String {
    v.map_or_else(|| "NULL".to_string(), |v| v.to_string())
}

fn rows(conn: &Arc<Connection>, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(sql).unwrap();
    let mut out: Vec<String> = stmt
        .run_collect_rows()
        .unwrap()
        .into_iter()
        .map(|row| format!("{row:?}"))
        .collect();
    out.sort();
    out
}

/// The outer row at which the statement's adaptive join switches, as its plan states it: None
/// when the plan has no switch (no break-even under the learned costs) or says it never comes.
fn switch_point(conn: &Arc<Connection>, sql: &str) -> Option<u64> {
    let mut stmt = conn.prepare(format!("EXPLAIN {sql}")).unwrap();
    let program = stmt.run_collect_rows().unwrap();
    program.into_iter().find_map(|row| {
        if row[1].to_string() != "JoinSwitchStep" {
            return None;
        }
        let comment = row[7].to_string();
        let point = comment.rsplit("at outer row ").next()?;
        point.trim().parse::<f64>().ok().map(|p| p as u64)
    })
}

fn open(name: &str) -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempfile::tempdir().unwrap();
    let io = Arc::new(PlatformIO::new().unwrap());
    let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
    let db = Database::open(io, dir.path().join(name).to_str().unwrap(), options).unwrap();
    let conn = db.connect(ConnectOpts::new()).unwrap();
    // A seek four rows' visits, a hash table's build and probe one each: a break-even on any
    // machine, where the learned weights may have none (a probe timed at a seek's cost or more).
    conn.pin_cost_weights(Some((4.0, 1.0, 1.0)));
    (dir, conn)
}

/// Do: random joins (one key and two, extra filters, three tables, an aggregate) over random
/// tables, each run with nested loops only, with the switch off and on, and by cost. Expect: one
/// answer, the model's for the plain join; and, where the plan states a switch point under the
/// outer table's rows, the switch fired (T-PL.13.1).
#[test]
fn t_pl_13_1_answers_identical_with_switching_on_and_off() {
    let mut fired = 0;
    let mut stated = 0;
    for seed in 1..=4u64 {
        let (_dir, conn) = open("adaptive.db");
        let mut state = 0x9e37_79b9_7f4a_7c15_u64 ^ seed.wrapping_mul(0x2545_f491_4f6c_dd1d);
        conn.execute("CREATE TABLE a (id INTEGER PRIMARY KEY, k, j, x)")
            .unwrap();
        conn.execute("CREATE TABLE b (id INTEGER PRIMARY KEY, k, j, y)")
            .unwrap();
        conn.execute("CREATE INDEX b_k ON b (k)").unwrap();
        conn.execute("CREATE INDEX b_kj ON b (k, j)").unwrap();
        conn.execute("CREATE TABLE c (id INTEGER PRIMARY KEY, k, z)")
            .unwrap();
        conn.execute("CREATE INDEX c_k ON c (k)").unwrap();
        let keys = 5 + next(&mut state) % 30;
        let a_rows = 500 + next(&mut state) % 1500;
        let b_rows = 20 + next(&mut state) % 80;
        let mut a = Vec::new();
        let mut b = Vec::new();
        conn.execute("BEGIN").unwrap();
        for id in 1..=a_rows as i64 {
            let (k, j) = (key(&mut state, keys), key(&mut state, 3));
            a.push((id, k));
            conn.execute(format!(
                "INSERT INTO a VALUES ({id}, {}, {}, {})",
                sql_value(k),
                sql_value(j),
                (id * 37) % 100
            ))
            .unwrap();
        }
        for id in 1..=b_rows as i64 {
            let (k, j) = (key(&mut state, keys), key(&mut state, 3));
            b.push((id, k));
            conn.execute(format!(
                "INSERT INTO b VALUES ({id}, {}, {}, {})",
                sql_value(k),
                sql_value(j),
                id % 7
            ))
            .unwrap();
            conn.execute(format!(
                "INSERT INTO c VALUES ({id}, {}, {})",
                sql_value(key(&mut state, keys)),
                (id * 13) % 100
            ))
            .unwrap();
        }
        conn.execute("COMMIT").unwrap();
        conn.execute("ANALYZE").unwrap();

        let inner = "SELECT a.id, b.id FROM a JOIN b ON b.k = a.k";
        let queries = [
            inner,
            "SELECT a.id, b.id, b.y FROM a JOIN b ON b.k = a.k AND b.j = a.j",
            "SELECT a.id, b.y FROM a JOIN b ON b.k = a.k WHERE b.y > 2 AND a.x < 70",
            "SELECT a.id, b.id, c.id FROM a JOIN b ON b.k = a.k JOIN c ON c.k = b.k",
            "SELECT a.k, count(*), sum(b.y) FROM a JOIN b ON b.k = a.k GROUP BY a.k",
            "SELECT count(*) FROM a JOIN b ON b.k = a.k + 1",
        ];

        let int = |v: i64| Value::Numeric(Numeric::Integer(v));
        let mut model = Vec::new();
        for &(aid, ak) in &a {
            for &(bid, bk) in &b {
                if ak.is_some() && bk == ak {
                    model.push(format!("{:?}", vec![int(aid), int(bid)]));
                }
            }
        }
        model.sort();

        let mut wrong = Vec::new();
        for sql in queries {
            conn.set_join_methods(JoinMethods::NestedLoopsOnly);
            conn.set_adaptive_joins(false);
            let off = rows(&conn, sql);
            conn.set_adaptive_joins(true);
            let point = switch_point(&conn, sql);
            let before = conn.adaptive_join_switches();
            let on = rows(&conn, sql);
            let switched = conn.adaptive_join_switches() - before;
            if let Some(point) = point {
                if point <= a_rows {
                    stated += 1;
                    fired += usize::from(switched > 0);
                }
            }
            conn.set_join_methods(JoinMethods::ByCost);
            let by_cost = rows(&conn, sql);
            if on != off {
                wrong.push(format!(
                    "seed {seed}: {sql}\n  switch on gives {} rows, off {} (point {point:?}, switched {switched})",
                    on.len(),
                    off.len()
                ));
            }
            if by_cost != off {
                wrong.push(format!(
                    "seed {seed}: {sql}\n  by cost gives {} rows, nested loops {}",
                    by_cost.len(),
                    off.len()
                ));
            }
            if sql == inner && off != model {
                wrong.push(format!(
                    "seed {seed}: {sql}\n  gives {} rows, the model {}",
                    off.len(),
                    model.len()
                ));
            }
        }
        conn.set_adaptive_joins(false);
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }
    // Every join whose plan states a point under its outer rows switched, and some did.
    assert!(
        stated > 0,
        "no plan stated a switch point under its outer rows"
    );
    assert_eq!(fired, stated, "switches fired where the plan's point came");
}

/// Do: one join, its outer rows cut to just under the switch point its plan states, then to the
/// point and past it. Expect: no switch under the point each run's plan states, exactly one at it, and the same answer as with the switch
/// off both times; where the learned costs give no break-even, no switch at all (T-PL.13.1).
#[test]
fn t_pl_13_1_switch_fires_past_the_point_and_not_before() {
    let (_dir, conn) = open("point.db");
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    conn.execute("CREATE TABLE a (id INTEGER PRIMARY KEY, k)")
        .unwrap();
    conn.execute("CREATE TABLE b (id INTEGER PRIMARY KEY, k)")
        .unwrap();
    conn.execute("CREATE INDEX b_k ON b (k)").unwrap();
    conn.execute("BEGIN").unwrap();
    for id in 1..=50 {
        conn.execute(format!(
            "INSERT INTO b VALUES ({id}, {})",
            sql_value(key(&mut state, 20))
        ))
        .unwrap();
    }
    conn.execute("COMMIT").unwrap();
    conn.execute("ANALYZE").unwrap();
    conn.set_join_methods(JoinMethods::NestedLoopsOnly);
    conn.set_adaptive_joins(true);

    let sql = |n: u64| format!("SELECT a.id, b.id FROM a JOIN b ON b.k = a.k WHERE a.id <= {n}");
    let point = switch_point(&conn, &sql(1)).unwrap();
    assert!(point >= 1, "a switch point of {point}");
    // The outer table holds the point's rows and more; the point is the inner side's alone.
    conn.execute("BEGIN").unwrap();
    for id in 1..=point + 10 {
        conn.execute(format!(
            "INSERT INTO a VALUES ({id}, {})",
            sql_value(key(&mut state, 20))
        ))
        .unwrap();
    }
    conn.execute("COMMIT").unwrap();

    // The learned costs move as statements run, and the point with them: each run is held to
    // the point its own plan states.
    for n in [point - 1, point, point + 10] {
        let query = sql(n);
        conn.set_adaptive_joins(false);
        let off = rows(&conn, &query);
        conn.set_adaptive_joins(true);
        let stated = switch_point(&conn, &query);
        let before = conn.adaptive_join_switches();
        let on = rows(&conn, &query);
        let want = u64::from(stated.is_some_and(|stated| n >= stated));
        assert_eq!(
            conn.adaptive_join_switches() - before,
            want,
            "{n} outer rows against a point of {stated:?}"
        );
        assert_eq!(on, off, "{n} outer rows: switch on and off differ");
    }
}

/// An inner table `b` of 50 rows whose count the planner holds far past them (a transaction
/// inserted 2,000 rows into it, its count took them, and it rolled back), and an outer table `a`
/// with ten rows more than the switch point the plain join's plan states, the point returned.
fn inflated_inner(name: &str) -> (tempfile::TempDir, Arc<Connection>, u64) {
    let (dir, conn) = open(name);
    let mut state = 0xd1b5_4a32_d192_ed03_u64;
    conn.execute("CREATE TABLE a (id INTEGER PRIMARY KEY, k)")
        .unwrap();
    conn.execute("CREATE TABLE b (id INTEGER PRIMARY KEY, k)")
        .unwrap();
    conn.execute("CREATE INDEX b_k ON b (k)").unwrap();
    conn.execute("BEGIN").unwrap();
    for id in 1..=50 {
        conn.execute(format!(
            "INSERT INTO b VALUES ({id}, {})",
            sql_value(key(&mut state, 20))
        ))
        .unwrap();
    }
    conn.execute("COMMIT").unwrap();
    conn.execute("ANALYZE").unwrap();
    // The count takes a finished INSERT's rows; the rollback takes them back out of the table only.
    conn.execute("BEGIN").unwrap();
    conn.execute(
        "INSERT INTO b(k) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 2000) SELECT i % 20 FROM n",
    )
    .unwrap();
    conn.execute("ROLLBACK").unwrap();
    conn.set_join_methods(JoinMethods::NestedLoopsOnly);
    conn.set_adaptive_joins(true);

    let sql = "SELECT a.id, b.id FROM a JOIN b ON b.k = a.k";
    let point = switch_point(&conn, sql).unwrap();
    conn.execute("BEGIN").unwrap();
    for id in 1..=point + 10 {
        conn.execute(format!(
            "INSERT INTO a VALUES ({id}, {})",
            sql_value(key(&mut state, 20))
        ))
        .unwrap();
    }
    conn.execute("COMMIT").unwrap();
    (dir, conn, point)
}

/// Do: an inner table whose count the planner holds far past its rows (a transaction inserted rows
/// into it, its count took them, and it rolled back), joined with the switch on past the point its
/// plan states; then the same join again. Expect: the hash table's build counts the true rows, past
/// the estimate's error, and hands the statement to a second prepare (one re-plan counted) with the
/// answer the switch-off one; run again, the statement is planned on the counted rows and no
/// re-plan comes (T-PL.13.3).
#[test]
fn t_pl_13_3_a_build_past_the_estimate_plans_again() {
    let (_dir, conn, point) = inflated_inner("replan.db");
    let sql = "SELECT a.id, b.id FROM a JOIN b ON b.k = a.k";

    conn.set_adaptive_joins(false);
    let off = rows(&conn, sql);
    conn.set_adaptive_joins(true);
    let stated = switch_point(&conn, sql);
    let (switches, replans) = (conn.adaptive_join_switches(), conn.adaptive_replans());
    let on = rows(&conn, sql);
    assert_eq!(on, off, "switch on and off differ");
    assert_eq!(stated, Some(point));
    assert_eq!(
        conn.adaptive_join_switches() - switches,
        1,
        "the join switched"
    );
    assert_eq!(
        conn.adaptive_replans() - replans,
        1,
        "the build's rows planned again"
    );

    // Planned again on the counted rows: the point comes sooner, and the build's count meets the
    // estimate.
    let replanned = switch_point(&conn, sql).unwrap();
    assert!(
        replanned < point,
        "the point {replanned} on the counted rows, {point} before"
    );
    let replans = conn.adaptive_replans();
    let again = rows(&conn, sql);
    assert_eq!(again, off, "the second plan's answer");
    assert_eq!(
        conn.adaptive_replans(),
        replans,
        "a second re-plan on the counted rows"
    );
}

/// Do: T-PL.13.3's join, its build counted past the estimate's band, each time on its own copy of
/// the tables: an aggregate over it, which returns no row before its end, with the in-run hand-off
/// on and off (`Connection::set_adaptive_handoff`); and the plain join, whose rows come before the
/// build, with it on; and the aggregate again with it on while another statement on the connection
/// is part way through its rows in the same implicit transaction. Expect: every answer the
/// switch-off one, each row once; the aggregate's run handed off once with it on (planned once
/// again, and switched again on the new plan before its one answer), none with it off; the plain
/// join hands nothing off, as it has returned rows; nor does the aggregate beside the other
/// statement, whose transaction a restart would end, and that statement reads on (T-PL.13.4).
#[test]
fn t_pl_13_4_a_run_past_the_estimate_starts_again_on_the_new_plan() {
    let aggregate =
        "SELECT count(*), sum(a.id * 1000 + b.id), min(b.id), max(a.id) FROM a JOIN b ON b.k = a.k";
    let plain = "SELECT a.id, b.id FROM a JOIN b ON b.k = a.k";
    let mut aggregates = Vec::new();
    for (sql, handoff, sibling, handed_off) in [
        (aggregate, true, false, 1),
        (aggregate, false, false, 0),
        (plain, true, false, 0),
        (aggregate, true, true, 0),
    ] {
        let (_dir, conn, point) = inflated_inner("handoff.db");
        let case = format!(
            "{sql} with the hand-off {}{}",
            if handoff { "on" } else { "off" },
            if sibling {
                ", another statement under way"
            } else {
                ""
            }
        );
        conn.set_adaptive_joins(false);
        let off = rows(&conn, sql);
        conn.set_adaptive_joins(true);
        conn.set_adaptive_handoff(handoff);
        assert_eq!(switch_point(&conn, sql), Some(point), "{case}: the point");
        let (switches, replans, handoffs) = (
            conn.adaptive_join_switches(),
            conn.adaptive_replans(),
            conn.adaptive_handoffs(),
        );
        let mut other = sibling.then(|| conn.prepare("SELECT id FROM a").unwrap());
        if let Some(other) = other.as_mut() {
            assert!(
                matches!(other.step().unwrap(), StepResult::Row),
                "{case}: the other's row"
            );
        }
        let on = rows(&conn, sql);
        if let Some(mut other) = other {
            let mut read = 1;
            loop {
                match other.step().unwrap() {
                    StepResult::Row => read += 1,
                    StepResult::IO => other._io().step().unwrap(),
                    _ => break,
                }
            }
            assert_eq!(read, point + 10, "{case}: the other statement's rows");
        }
        assert_eq!(on, off, "{case}: switch on and off differ");
        assert_eq!(
            conn.adaptive_handoffs() - handoffs,
            handed_off,
            "{case}: hand-offs"
        );
        assert_eq!(conn.adaptive_replans() - replans, 1, "{case}: re-plans");
        assert_eq!(
            conn.adaptive_join_switches() - switches,
            1 + handed_off,
            "{case}: switches"
        );
        if sql == aggregate {
            assert_eq!(on.len(), 1, "{case}: one answer row");
            aggregates.push(on);
        }
        // Run again: planned on the counted rows already, nothing hands off or plans again.
        let (replans, handoffs) = (conn.adaptive_replans(), conn.adaptive_handoffs());
        assert_eq!(rows(&conn, sql), off, "{case}: the second run's answer");
        assert_eq!(conn.adaptive_replans(), replans, "{case}: a second re-plan");
        assert_eq!(
            conn.adaptive_handoffs(),
            handoffs,
            "{case}: a second hand-off"
        );
    }
    assert_eq!(
        aggregates[0], aggregates[1],
        "the hand-off on and off differ"
    );
}

/// Do: a hash join the planner chose itself (hash joins only), its build side a table whose count
/// the planner holds far past its rows (inserted, counted, rolled back), run with adaptive joins
/// off and on; then again. Expect: one answer both ways; off, no build counted; on, the build's
/// rows counted against the estimate, past its error, the statement planned again once; run again
/// on the counted rows, the build is counted within the band and no re-plan comes (PL.13, the
/// materialization point at a planner-chosen hash join's build).
#[test]
fn pl_13_a_chosen_hash_joins_build_past_the_estimate_plans_again() {
    let (_dir, conn) = open("chosen_build.db");
    // A build row priced past a probe row: with the two equal, building a or b costs the same and
    // the machine's timed weights (a page's, the fitted ones) pick the side, b about one run in five,
    // whose count is right and plans nothing again. Priced apart, the plan builds the side it counts
    // smaller, a, whatever the machine.
    conn.pin_cost_weights(Some((4.0, 3.0, 1.0)));
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    conn.execute("CREATE TABLE a (id INTEGER PRIMARY KEY, k)")
        .unwrap();
    conn.execute("CREATE TABLE b (id INTEGER PRIMARY KEY, k)")
        .unwrap();
    conn.execute("BEGIN").unwrap();
    for id in 1..=50 {
        conn.execute(format!(
            "INSERT INTO a VALUES ({id}, {})",
            sql_value(key(&mut state, 20))
        ))
        .unwrap();
    }
    conn.execute(
        "INSERT INTO b(k) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000) SELECT i % 20 FROM n",
    )
    .unwrap();
    conn.execute("COMMIT").unwrap();
    conn.execute("ANALYZE").unwrap();
    conn.execute("BEGIN").unwrap();
    conn.execute(
        "INSERT INTO a(k) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 2000) SELECT i % 20 FROM n",
    )
    .unwrap();
    conn.execute("ROLLBACK").unwrap();
    conn.set_join_methods(JoinMethods::HashJoinsOnly);
    let sql = "SELECT a.id, b.id FROM a JOIN b ON b.k = a.k";

    conn.set_adaptive_joins(false);
    let checks = conn.adaptive_build_checks();
    let off = rows(&conn, sql);
    assert_eq!(
        conn.adaptive_build_checks(),
        checks,
        "counted with adaptive joins off"
    );

    conn.set_adaptive_joins(true);
    let replans = conn.adaptive_replans();
    let on = rows(&conn, sql);
    assert_eq!(on, off, "adaptive joins on and off differ");
    assert!(
        conn.adaptive_build_checks() > checks,
        "the build was not counted"
    );
    assert_eq!(
        conn.adaptive_replans() - replans,
        1,
        "the build's rows planned again"
    );

    let (checks, replans) = (conn.adaptive_build_checks(), conn.adaptive_replans());
    let again = rows(&conn, sql);
    assert_eq!(again, off, "the second plan's answer");
    assert!(
        conn.adaptive_build_checks() > checks,
        "the second build was not counted"
    );
    assert_eq!(
        conn.adaptive_replans(),
        replans,
        "a second re-plan on the counted rows"
    );
}

fn collect(stmt: &mut amphora_engine::Statement) -> Vec<String> {
    let mut out: Vec<String> = stmt
        .run_collect_rows()
        .unwrap()
        .into_iter()
        .map(|row| format!("{row:?}"))
        .collect();
    stmt.reset().unwrap();
    out.sort();
    out
}

/// Do: a hash join the planner chose itself (hash joins only), its build side a table filtered by a
/// range one row in a hundred passed as ANALYZE sampled it, then updated so every row passes (its
/// count unchanged, so its row counter, PL.9, cannot tell); run with adaptive joins off, then on in
/// one prepared statement twice, and in a statement prepared while that one lives. Expect: one
/// answer every way; off, no filtered build counted; on, the filtered build counted past its
/// share's error, the counted share stored and the statement planned again once; the second run
/// and the new statement plan on the stored share, and nothing plans again (PL.13, lane replan-5:
/// a filtered build's materialization point).
#[test]
fn pl_13_a_filtered_builds_share_past_its_estimate_plans_again() {
    let (_dir, conn) = open("filtered_build.db");
    conn.execute("CREATE TABLE a (id INTEGER PRIMARY KEY, k)")
        .unwrap();
    conn.execute("CREATE TABLE b (id INTEGER PRIMARY KEY, k, v)")
        .unwrap();
    conn.execute(
        "INSERT INTO a(k) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 100) SELECT i % 20 FROM n",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO b(k, v) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000) SELECT i % 20, i FROM n",
    )
    .unwrap();
    conn.execute("ANALYZE").unwrap();
    conn.execute("UPDATE b SET v = v % 50").unwrap();
    conn.set_join_methods(JoinMethods::HashJoinsOnly);
    let sql = "SELECT a.id, b.id FROM a JOIN b ON b.k = a.k WHERE b.v < 50";

    conn.set_adaptive_joins(false);
    let (checks, corrections) = (
        conn.adaptive_filter_checks(),
        conn.adaptive_filter_corrections(),
    );
    let off = rows(&conn, sql);
    assert_eq!(off.len(), 100 * 5000 / 20, "the join's rows");
    assert_eq!(
        conn.adaptive_filter_checks(),
        checks,
        "counted with adaptive joins off"
    );

    conn.set_adaptive_joins(true);
    let replans = conn.adaptive_replans();
    let mut held = conn.prepare(sql).unwrap();
    assert_eq!(collect(&mut held), off, "adaptive joins on and off differ");
    assert!(
        conn.adaptive_filter_checks() > checks,
        "the filtered build was not counted"
    );
    assert_eq!(
        conn.adaptive_filter_corrections() - corrections,
        1,
        "the counted share stored"
    );
    assert_eq!(conn.adaptive_replans() - replans, 1, "planned again once");

    let replans = conn.adaptive_replans();
    assert_eq!(collect(&mut held), off, "the second run's answer");
    assert_eq!(
        rows(&conn, sql),
        off,
        "a statement planned on the stored share"
    );
    assert_eq!(conn.adaptive_replans(), replans, "a second re-plan");
    assert_eq!(
        conn.adaptive_filter_corrections() - corrections,
        1,
        "a second share stored"
    );
}

/// Do: an ORDER BY over one table's full scan, with no WHERE term on a table counted far past its
/// rows (inserted, counted, rolled back) and with a range one row in a hundred passed as ANALYZE
/// sampled it and every row after an UPDATE; each run with adaptive joins off, then on twice in one
/// prepared statement. Expect: one answer, in order, every way; off, no sorter counted; on, the
/// sorter's input counted against the estimate past its error and the statement planned again
/// once; the second run plans on the counted rows and nothing plans again (PL.13, lane replan-5:
/// the sorter's materialization point).
#[test]
fn pl_13_a_sorters_input_past_the_estimate_plans_again() {
    let (_dir, conn) = open("sorter.db");
    conn.execute("CREATE TABLE b (id INTEGER PRIMARY KEY, k, v)")
        .unwrap();
    conn.execute(
        "INSERT INTO b(k, v) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000) SELECT i % 20, i FROM n",
    )
    .unwrap();
    conn.execute("ANALYZE").unwrap();
    conn.execute("UPDATE b SET v = v % 50").unwrap();
    conn.execute("BEGIN").unwrap();
    conn.execute(
        "INSERT INTO b(k, v) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 50000) SELECT i % 20, i FROM n",
    )
    .unwrap();
    conn.execute("ROLLBACK").unwrap();
    for (case, sql, rows_expected) in [
        ("unfiltered", "SELECT id, k FROM b ORDER BY k, id", 5000),
        (
            "filtered",
            "SELECT id, k FROM b WHERE v < 50 ORDER BY k, id",
            5000,
        ),
    ] {
        let ordered = |stmt: &mut amphora_engine::Statement| -> Vec<String> {
            let out = stmt
                .run_collect_rows()
                .unwrap()
                .into_iter()
                .map(|row| format!("{row:?}"))
                .collect();
            stmt.reset().unwrap();
            out
        };
        conn.set_adaptive_joins(false);
        let checks = conn.adaptive_sort_checks();
        let off = ordered(&mut conn.prepare(sql).unwrap());
        assert_eq!(off.len(), rows_expected, "{case}: the rows");
        assert_eq!(conn.adaptive_sort_checks(), checks, "{case}: counted off");

        conn.set_adaptive_joins(true);
        let replans = conn.adaptive_replans();
        let mut held = conn.prepare(sql).unwrap();
        assert_eq!(ordered(&mut held), off, "{case}: on and off differ");
        assert!(
            conn.adaptive_sort_checks() > checks,
            "{case}: the sorter was not counted"
        );
        assert_eq!(
            conn.adaptive_replans() - replans,
            1,
            "{case}: planned again once"
        );
        let (checks, replans) = (conn.adaptive_sort_checks(), conn.adaptive_replans());
        assert_eq!(ordered(&mut held), off, "{case}: the second run's answer");
        assert!(
            conn.adaptive_sort_checks() > checks,
            "{case}: the second sorter was not counted"
        );
        assert_eq!(conn.adaptive_replans(), replans, "{case}: a second re-plan");
    }
}

/// The rows of one prepared statement's run, in order, the statement reset after.
fn run_rows(stmt: &mut amphora_engine::Statement) -> Vec<String> {
    let out = stmt
        .run_collect_rows()
        .unwrap()
        .into_iter()
        .map(|row| format!("{row:?}"))
        .collect();
    stmt.reset().unwrap();
    out
}

/// Do: a stored subquery result whose filter keeps one row in a hundred as ANALYZE sampled it and
/// every row after an UPDATE: a CTE referenced twice (stored once, shared) and a GROUP BY in a
/// materialized CTE; each run with adaptive joins off, then on twice in one prepared statement,
/// then once more prepared anew. Expect: one answer every way; off, no stored result counted; on,
/// the fill's rows counted against the estimate, past its error stored and the statement planned
/// again once; the second run and a new statement plan on the counted rows and nothing plans again
/// (PL.13, lane replan-6, T-PL.13.7: the stored subquery result's materialization point).
#[test]
fn t_pl_13_7_a_stored_subquery_result_past_the_estimate_plans_again() {
    let (_dir, conn) = open("subquery.db");
    conn.execute("CREATE TABLE b (id INTEGER PRIMARY KEY, k, v)")
        .unwrap();
    conn.execute(
        "INSERT INTO b(k, v) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000) SELECT i % 2000, i FROM n",
    )
    .unwrap();
    conn.execute("ANALYZE").unwrap();
    conn.execute("UPDATE b SET v = v % 50").unwrap();
    for (case, sql, rows_expected) in [
        (
            "a CTE referenced twice",
            "WITH c AS (SELECT id, k FROM b WHERE v < 50) SELECT x.id, x.k, y.k FROM c AS x JOIN c AS y ON y.id = x.id ORDER BY x.id",
            5000,
        ),
        (
            "a GROUP BY subquery",
            "WITH g AS MATERIALIZED (SELECT k, count(*) AS n FROM b WHERE v < 50 GROUP BY k) SELECT b.id, g.n FROM b JOIN g ON g.k = b.k ORDER BY b.id",
            5000,
        ),
    ] {
        conn.set_adaptive_joins(false);
        let checks = conn.adaptive_subquery_checks();
        let off = run_rows(&mut conn.prepare(sql).unwrap());
        assert_eq!(off.len(), rows_expected, "{case}: the rows");
        assert_eq!(conn.adaptive_subquery_checks(), checks, "{case}: counted off");

        conn.set_adaptive_joins(true);
        let replans = conn.adaptive_replans();
        let mut held = conn.prepare(sql).unwrap();
        assert_eq!(run_rows(&mut held), off, "{case}: on and off differ");
        assert!(
            conn.adaptive_subquery_checks() > checks,
            "{case}: the stored result was not counted"
        );
        assert_eq!(
            conn.adaptive_replans() - replans,
            1,
            "{case}: planned again once"
        );
        let (checks, replans) = (conn.adaptive_subquery_checks(), conn.adaptive_replans());
        assert_eq!(run_rows(&mut held), off, "{case}: the second run's answer");
        assert!(
            conn.adaptive_subquery_checks() > checks,
            "{case}: the second fill was not counted"
        );
        assert_eq!(conn.adaptive_replans(), replans, "{case}: a second re-plan");
        assert_eq!(
            run_rows(&mut conn.prepare(sql).unwrap()),
            off,
            "{case}: a new statement's answer"
        );
        assert_eq!(
            conn.adaptive_replans(),
            replans,
            "{case}: a new statement planned again"
        );
    }
}

/// The plan's steps as EXPLAIN QUERY PLAN states them, and the program's opcodes.
fn steps_and_opcodes(conn: &Arc<Connection>, sql: &str) -> (Vec<String>, Vec<String>) {
    let column = |sql: String, at: usize| -> Vec<String> {
        conn.prepare(sql)
            .unwrap()
            .run_collect_rows()
            .unwrap()
            .into_iter()
            .map(|row| row[at].to_string())
            .collect()
    };
    (
        column(format!("EXPLAIN QUERY PLAN {sql}"), 3),
        column(format!("EXPLAIN {sql}"), 1),
    )
}

/// Do: a GROUP BY subquery joined on its key with nested loops only, so the subquery is stored in
/// the seek index its key's lookups read (`DirectMaterializedIndex`), over a range whose share an
/// UPDATE moved (ANALYZE sampled one row in a hundred kept, every row after); run with adaptive
/// joins off, then on twice in one prepared statement, then once more prepared anew. Expect: the
/// plan reads the subquery through its seek index; one answer every way; off, nothing counted; on,
/// the index's entries counted against the estimate, past its error stored and the statement
/// planned again once; the second run and a new statement plan on the counted rows and nothing
/// plans again (PL.13, lane replan-8, T-PL.13.8: the seek index's materialization point).
#[test]
fn t_pl_13_8_a_subquery_stored_in_its_seek_index_past_the_estimate_plans_again() {
    let (_dir, conn) = open("seek_index.db");
    conn.set_join_methods(JoinMethods::NestedLoopsOnly);
    conn.execute("CREATE TABLE b (id INTEGER PRIMARY KEY, k, v)")
        .unwrap();
    conn.execute(
        "INSERT INTO b(k, v) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000) SELECT i % 2000, i FROM n",
    )
    .unwrap();
    conn.execute("ANALYZE").unwrap();
    conn.execute("UPDATE b SET v = v % 50").unwrap();
    let sql = "SELECT b.id, s.n FROM b JOIN (SELECT k, count(*) AS n FROM b WHERE v < 50 GROUP BY k) AS s ON s.k = b.k ORDER BY b.id";

    conn.set_adaptive_joins(false);
    let checks = conn.adaptive_subquery_checks();
    let off = run_rows(&mut conn.prepare(sql).unwrap());
    assert_eq!(off.len(), 5000, "the rows");
    assert_eq!(conn.adaptive_subquery_checks(), checks, "counted off");

    conn.set_adaptive_joins(true);
    let (steps, opcodes) = steps_and_opcodes(&conn, sql);
    assert!(
        steps
            .iter()
            .any(|step| step.contains("USING INDEX ephemeral_subquery_")),
        "the subquery is not read through its seek index: {steps:?}"
    );
    assert!(
        opcodes.iter().any(|opcode| opcode == "SubqueryCounted"),
        "the seek index is not counted: {opcodes:?}"
    );
    let replans = conn.adaptive_replans();
    let mut held = conn.prepare(sql).unwrap();
    assert_eq!(run_rows(&mut held), off, "on and off differ");
    assert!(
        conn.adaptive_subquery_checks() > checks,
        "the seek index was not counted"
    );
    assert_eq!(conn.adaptive_replans() - replans, 1, "planned again once");
    let (checks, replans) = (conn.adaptive_subquery_checks(), conn.adaptive_replans());
    assert_eq!(run_rows(&mut held), off, "the second run's answer");
    assert!(
        conn.adaptive_subquery_checks() > checks,
        "the second fill was not counted"
    );
    assert_eq!(conn.adaptive_replans(), replans, "a second re-plan");
    assert_eq!(
        run_rows(&mut conn.prepare(sql).unwrap()),
        off,
        "a new statement's answer"
    );
    assert_eq!(
        conn.adaptive_replans(),
        replans,
        "a new statement planned again"
    );
}

/// Do: a materialized GROUP BY subquery whose groups ANALYZE's index on its key gave: 5,000 rows in
/// 2,000 groups, stored as 3 rows a group (`ceil`), so read as 1,667 groups, up to half again as
/// many (`1 / (3 - 1)`). Run it with adaptive joins on as analyzed, then after an UPDATE gives
/// every row a group of its own. Expect: as analyzed, the 2,000 groups counted fall within the
/// group count's error, though past the statistics' rows error (1 / sqrt(1,024)): nothing planned
/// again; after the UPDATE, 5,000 groups fall past it: planned again once; one answer with
/// adaptive joins off and on each time (PL.13, lane replan-8, T-PL.13.9: a grouped result's band).
#[test]
fn t_pl_13_9_a_grouped_result_is_checked_against_its_group_counts_error() {
    let (_dir, conn) = open("groups.db");
    conn.execute("CREATE TABLE g (id INTEGER PRIMARY KEY, k, v)")
        .unwrap();
    conn.execute("CREATE INDEX g_k ON g(k)").unwrap();
    conn.execute(
        "INSERT INTO g(k, v) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000) SELECT i % 2000, i FROM n",
    )
    .unwrap();
    conn.execute("ANALYZE").unwrap();
    let sql = "WITH s AS MATERIALIZED (SELECT k, count(*) AS n FROM g GROUP BY k) SELECT g.id, s.n FROM g JOIN s ON s.k = g.k ORDER BY g.id";
    for (case, replanned) in [("as analyzed", 0), ("a group a row", 1)] {
        if replanned == 1 {
            conn.execute("UPDATE g SET k = id").unwrap();
        }
        conn.set_adaptive_joins(false);
        let off = run_rows(&mut conn.prepare(sql).unwrap());
        assert_eq!(off.len(), 5000, "{case}: the rows");

        conn.set_adaptive_joins(true);
        let (checks, replans) = (conn.adaptive_subquery_checks(), conn.adaptive_replans());
        let mut held = conn.prepare(sql).unwrap();
        assert_eq!(run_rows(&mut held), off, "{case}: on and off differ");
        assert!(
            conn.adaptive_subquery_checks() > checks,
            "{case}: the grouped result was not counted"
        );
        assert_eq!(
            conn.adaptive_replans() - replans,
            replanned,
            "{case}: planned again"
        );
        let replans = conn.adaptive_replans();
        assert_eq!(run_rows(&mut held), off, "{case}: the second run's answer");
        assert_eq!(conn.adaptive_replans(), replans, "{case}: a second re-plan");
    }
}

/// Do: a hash join the planner chose itself (hash joins only), its build side a table filtered by
/// one `=`, and then by one IN list, on a column whose statistics (decision 20) give the value one
/// row in 5,000 (every value distinct as ANALYZE counted them), then updated so half the rows hold
/// it (the count unchanged, so the row counter, PL.9, cannot tell); each run with adaptive joins
/// off, then on in one prepared statement twice, and in a statement prepared after. Expect: one
/// answer every way; off, no filtered build counted; on, the filtered build counted against the
/// share the column statistics gave, past its error, the counted share stored and the statement
/// planned again once; the second run and the new statement plan on the stored share, and nothing
/// plans again (PL.13, lane replan-9, T-PL.13.10: a filter the column statistics estimate, which
/// the sample does not run, at its build's materialization point).
#[test]
fn t_pl_13_10_a_build_filtered_by_a_column_statistics_term_plans_again() {
    let (_dir, conn) = open("column_stats_build.db");
    conn.execute("CREATE TABLE a (id INTEGER PRIMARY KEY, k)")
        .unwrap();
    conn.execute("CREATE TABLE b (id INTEGER PRIMARY KEY, k, v)")
        .unwrap();
    conn.execute(
        "INSERT INTO a(k) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 100) SELECT i % 20 FROM n",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO b(k, v) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000) SELECT i % 20, i FROM n",
    )
    .unwrap();
    conn.execute("ANALYZE").unwrap();
    conn.execute("UPDATE b SET v = 7 WHERE id % 2 = 0").unwrap();
    conn.set_join_methods(JoinMethods::HashJoinsOnly);
    for (case, sql) in [
        (
            "=",
            "SELECT a.id, b.id FROM a JOIN b ON b.k = a.k WHERE b.v = 7",
        ),
        (
            "IN",
            "SELECT a.id, b.id FROM a JOIN b ON b.k = a.k WHERE b.v IN (7, 8)",
        ),
    ] {
        conn.set_adaptive_joins(false);
        let (checks, corrections) = (
            conn.adaptive_filter_checks(),
            conn.adaptive_filter_corrections(),
        );
        let off = rows(&conn, sql);
        // b's even ids hold 7 (8 is b.v of no row now): 250 of each even k, each k five rows of a.
        assert_eq!(off.len(), 10 * 250 * 5, "{case}: the join's rows");
        assert_eq!(
            conn.adaptive_filter_checks(),
            checks,
            "{case}: counted with adaptive joins off"
        );

        conn.set_adaptive_joins(true);
        let replans = conn.adaptive_replans();
        let mut held = conn.prepare(sql).unwrap();
        assert_eq!(collect(&mut held), off, "{case}: on and off differ");
        assert!(
            conn.adaptive_filter_checks() > checks,
            "{case}: the filtered build was not counted"
        );
        assert_eq!(
            conn.adaptive_filter_corrections() - corrections,
            1,
            "{case}: the counted share stored"
        );
        assert_eq!(
            conn.adaptive_replans() - replans,
            1,
            "{case}: planned again once"
        );

        let replans = conn.adaptive_replans();
        assert_eq!(collect(&mut held), off, "{case}: the second run's answer");
        assert_eq!(
            rows(&conn, sql),
            off,
            "{case}: a statement planned on the stored share"
        );
        assert_eq!(conn.adaptive_replans(), replans, "{case}: a second re-plan");
        assert_eq!(
            conn.adaptive_filter_corrections() - corrections,
            1,
            "{case}: a second share stored"
        );
    }
}

/// Do: an ORDER BY over one table read by an index seek on one `=`, and then one IN list, on a
/// column whose statistics gave the value one row in 5,000 (every value distinct as ANALYZE counted
/// them), then updated so half the rows hold it; each run with adaptive joins off, then on twice in
/// one prepared statement, and once more prepared anew. Expect: one answer, in order, every way;
/// off, no sorter counted; on, the sorter's input counted against the share the column statistics
/// gave, past its error, the counted share stored and the statement planned again once; the second
/// run and a new statement plan on the stored share, and nothing plans again (PL.13, lane
/// replan-9, T-PL.13.11: a sorter fed by an index seek, at its materialization point).
#[test]
fn t_pl_13_11_a_sorter_fed_by_an_index_seek_plans_again() {
    let (_dir, conn) = open("seek_sorter.db");
    conn.execute("CREATE TABLE c (id INTEGER PRIMARY KEY, k, v)")
        .unwrap();
    conn.execute("CREATE INDEX c_v ON c (v)").unwrap();
    conn.execute(
        "INSERT INTO c(k, v) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000) SELECT i % 20, i FROM n",
    )
    .unwrap();
    conn.execute("ANALYZE").unwrap();
    conn.execute("UPDATE c SET v = 7 WHERE id % 2 = 0").unwrap();
    for (case, sql) in [
        ("=", "SELECT id, k FROM c WHERE v = 7 ORDER BY k, id"),
        ("IN", "SELECT id, k FROM c WHERE v IN (7, 8) ORDER BY k, id"),
    ] {
        conn.set_adaptive_joins(false);
        let checks = conn.adaptive_sort_checks();
        let off = run_rows(&mut conn.prepare(sql).unwrap());
        assert_eq!(off.len(), 2500, "{case}: the rows");
        assert_eq!(conn.adaptive_sort_checks(), checks, "{case}: counted off");

        conn.set_adaptive_joins(true);
        let (replans, corrections) = (conn.adaptive_replans(), conn.adaptive_filter_corrections());
        let mut held = conn.prepare(sql).unwrap();
        assert_eq!(run_rows(&mut held), off, "{case}: on and off differ");
        assert!(
            conn.adaptive_sort_checks() > checks,
            "{case}: the sorter was not counted"
        );
        assert_eq!(
            conn.adaptive_filter_corrections() - corrections,
            1,
            "{case}: the counted share stored"
        );
        assert_eq!(
            conn.adaptive_replans() - replans,
            1,
            "{case}: planned again once"
        );

        let replans = conn.adaptive_replans();
        assert_eq!(run_rows(&mut held), off, "{case}: the second run's answer");
        assert_eq!(
            run_rows(&mut conn.prepare(sql).unwrap()),
            off,
            "{case}: a statement planned on the stored share"
        );
        assert_eq!(conn.adaptive_replans(), replans, "{case}: a second re-plan");
    }
}

/// Do: `a.x IN (SELECT s.y FROM s WHERE s.k <= 1000)`, `s.y` distinct in every row as ANALYZE
/// counted it (1,000 values read as the plan estimates), then updated so the rows read hold five
/// values (the rows and the row counter unchanged); each run with adaptive joins off, then on in
/// one prepared statement twice, and in a statement prepared after. Expect: one answer every way;
/// off, no IN result counted; on, the ephemeral index's distinct values counted against the ones
/// the plan read, past its error stored and the statement planned again once; the second run and
/// the new statement plan on the stored count, and nothing plans again (PL.13, lane replan-10,
/// T-PL.13.12: an IN subquery's stored result at its materialization point).
#[test]
fn t_pl_13_12_an_in_subquerys_stored_result_past_the_estimate_plans_again() {
    let (_dir, conn) = open("in_subquery.db");
    conn.execute("CREATE TABLE a (id INTEGER PRIMARY KEY, x)")
        .unwrap();
    conn.execute("CREATE INDEX a_x ON a(x)").unwrap();
    conn.execute("CREATE TABLE s (id INTEGER PRIMARY KEY, k, y)")
        .unwrap();
    conn.execute(
        "INSERT INTO a(x) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000) SELECT i % 1000 FROM n",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO s(k, y) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 2000) SELECT i, i FROM n",
    )
    .unwrap();
    conn.execute("ANALYZE").unwrap();
    conn.execute("UPDATE s SET y = y % 5").unwrap();
    let sql = "SELECT a.id, a.x FROM a WHERE a.x IN (SELECT s.y FROM s WHERE s.k <= 1000)";
    let sorted = |stmt: &mut amphora_engine::Statement| {
        let mut rows = run_rows(stmt);
        rows.sort();
        rows
    };

    conn.set_adaptive_joins(false);
    let checks = conn.adaptive_subquery_checks();
    let off = sorted(&mut conn.prepare(sql).unwrap());
    // x in 0..5: five values of a's 1,000, five rows each.
    assert_eq!(off.len(), 25, "the rows");
    assert_eq!(conn.adaptive_subquery_checks(), checks, "counted off");

    conn.set_adaptive_joins(true);
    let replans = conn.adaptive_replans();
    let mut held = conn.prepare(sql).unwrap();
    assert_eq!(sorted(&mut held), off, "on and off differ");
    assert!(
        conn.adaptive_subquery_checks() > checks,
        "the IN subquery's result was not counted"
    );
    assert_eq!(conn.adaptive_replans() - replans, 1, "planned again once");
    let (checks, replans) = (conn.adaptive_subquery_checks(), conn.adaptive_replans());
    assert_eq!(sorted(&mut held), off, "the second run's answer");
    assert!(
        conn.adaptive_subquery_checks() > checks,
        "the second fill was not counted"
    );
    assert_eq!(conn.adaptive_replans(), replans, "a second re-plan");
    assert_eq!(
        sorted(&mut conn.prepare(sql).unwrap()),
        off,
        "a new statement's answer"
    );
    assert_eq!(
        conn.adaptive_replans(),
        replans,
        "a new statement planned again"
    );
}

/// Do: `a.x IN (SELECT ...)` over a subquery of 4,000 rows, `a.x` 5,000 distinct values indexed;
/// the subquery's column distinct in every row, and then one of five values, as ANALYZE counted
/// them; adaptive joins off. Expect: 4,000 values to look up, four in five of `a`'s rows, read by a
/// scan of `a`; five values, five rows, looked up in `a_x`. The subquery's values are its rows at
/// most and its column's distinct values at most, not a cap at the square root of `a`'s rows,
/// which took both for 70 values (PL.13, lane replan-10, T-PL.13.13: the IN subquery's estimate
/// from statistics).
#[test]
fn t_pl_13_13_an_in_subquerys_values_come_from_its_statistics() {
    let (_dir, conn) = open("in_values.db");
    conn.execute("CREATE TABLE a (id INTEGER PRIMARY KEY, x)")
        .unwrap();
    conn.execute("CREATE INDEX a_x ON a(x)").unwrap();
    conn.execute("CREATE TABLE s (id INTEGER PRIMARY KEY, y, z)")
        .unwrap();
    conn.execute(
        "INSERT INTO a(x) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000) SELECT i FROM n",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO s(y, z) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 4000) SELECT i, i % 5 FROM n",
    )
    .unwrap();
    conn.execute("ANALYZE").unwrap();
    conn.set_adaptive_joins(false);
    let outer_step = |sql: &str| -> String {
        let (steps, _) = steps_and_opcodes(&conn, sql);
        steps
            .into_iter()
            .find(|step| {
                step.contains(" a") && (step.starts_with("SCAN") || step.starts_with("SEARCH"))
            })
            .unwrap_or_else(|| panic!("no step reads a: {sql}"))
    };
    let many = outer_step("SELECT a.id FROM a WHERE a.x IN (SELECT s.y FROM s)");
    assert!(
        many.starts_with("SCAN a"),
        "4,000 values, four in five rows, read by a scan: {many}"
    );
    let few = outer_step("SELECT a.id FROM a WHERE a.x IN (SELECT s.z FROM s)");
    assert!(
        few.starts_with("SEARCH a USING") && few.contains("a_x"),
        "five values looked up in a_x: {few}"
    );
}

/// Do: a GROUP BY read through its sorter (a `count(DISTINCT ...)` keeps the groups out of a hash
/// table), over one table's full scan: the table counted far past its rows (inserted, counted,
/// rolled back), then a range whose share an UPDATE moved (one row in a hundred at ANALYZE, every
/// row after); each run with adaptive joins off, then on twice in one prepared statement. Expect:
/// one answer every way; off, no sorter counted; on, the sorter's input counted once all in, ahead
/// of its sort, against the table's rows or the range's share, past its error corrected and the
/// statement planned again once; the second run's sorter counted, and nothing plans again (PL.13,
/// lane replan-10, T-PL.13.14: a GROUP BY's sorter at its materialization point).
#[test]
fn t_pl_13_14_a_group_by_sorters_input_past_the_estimate_plans_again() {
    let (_dir, conn) = open("group_sorter.db");
    conn.execute("CREATE TABLE b (id INTEGER PRIMARY KEY, k, v)")
        .unwrap();
    conn.execute(
        "INSERT INTO b(k, v) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000) SELECT i % 20, i FROM n",
    )
    .unwrap();
    conn.execute("ANALYZE").unwrap();
    conn.execute("UPDATE b SET v = v % 50").unwrap();
    conn.execute("BEGIN").unwrap();
    conn.execute(
        "INSERT INTO b(k, v) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 50000) SELECT i % 20, i FROM n",
    )
    .unwrap();
    conn.execute("ROLLBACK").unwrap();
    for (case, sql) in [
        (
            "unfiltered",
            "SELECT k, count(DISTINCT v) FROM b GROUP BY k ORDER BY k",
        ),
        (
            "filtered",
            "SELECT k, count(DISTINCT v) FROM b WHERE v < 50 GROUP BY k ORDER BY k",
        ),
    ] {
        conn.set_adaptive_joins(false);
        let checks = conn.adaptive_sort_checks();
        let off = run_rows(&mut conn.prepare(sql).unwrap());
        assert_eq!(off.len(), 20, "{case}: the groups");
        assert_eq!(conn.adaptive_sort_checks(), checks, "{case}: counted off");

        conn.set_adaptive_joins(true);
        let replans = conn.adaptive_replans();
        let mut held = conn.prepare(sql).unwrap();
        assert_eq!(run_rows(&mut held), off, "{case}: on and off differ");
        assert!(
            conn.adaptive_sort_checks() > checks,
            "{case}: the GROUP BY sorter was not counted"
        );
        assert_eq!(
            conn.adaptive_replans() - replans,
            1,
            "{case}: planned again once"
        );
        let (checks, replans) = (conn.adaptive_sort_checks(), conn.adaptive_replans());
        assert_eq!(run_rows(&mut held), off, "{case}: the second run's answer");
        assert!(
            conn.adaptive_sort_checks() > checks,
            "{case}: the second sorter was not counted"
        );
        assert_eq!(conn.adaptive_replans(), replans, "{case}: a second re-plan");
    }
}

/// Do: an ORDER BY over an inner join `a.k = b.k`, `b.k` distinct in every row as ANALYZE counted
/// it (the join's estimate a's 1,000 rows), then updated to ten values (the rows and both row
/// counters unchanged), so the join holds 4,500 rows; run with adaptive joins off, then on twice
/// in one prepared statement, and in a statement prepared after. Expect: one answer, in order,
/// every way; off, no sorter counted; on, the sorter's input counted once all in against the rows
/// the plan read for the whole join, past its error stored and the statement planned again once;
/// the second run's sorter counted on the stored rows and nothing plans again (PL.13, lane
/// replan-10, T-PL.13.15: a sorter fed by a join at its materialization point).
#[test]
fn t_pl_13_15_a_sorter_fed_by_a_join_past_the_estimate_plans_again() {
    let (_dir, conn) = open("joined_sorter.db");
    conn.execute("CREATE TABLE a (id INTEGER PRIMARY KEY, k)")
        .unwrap();
    conn.execute("CREATE TABLE b (id INTEGER PRIMARY KEY, k, v)")
        .unwrap();
    conn.execute(
        "INSERT INTO a(k) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 1000) SELECT i FROM n",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO b(k, v) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000) SELECT i, i % 7 FROM n",
    )
    .unwrap();
    conn.execute("ANALYZE").unwrap();
    conn.execute("UPDATE b SET k = k % 10").unwrap();
    let sql = "SELECT a.id, b.id, b.v FROM a JOIN b ON b.k = a.k ORDER BY b.v, b.id, a.id";

    conn.set_adaptive_joins(false);
    let checks = conn.adaptive_sort_checks();
    let off = run_rows(&mut conn.prepare(sql).unwrap());
    // a.k 1 to 9 each meets the 500 rows of b holding it.
    assert_eq!(off.len(), 4500, "the rows");
    assert_eq!(conn.adaptive_sort_checks(), checks, "counted off");

    conn.set_adaptive_joins(true);
    let replans = conn.adaptive_replans();
    let mut held = conn.prepare(sql).unwrap();
    assert_eq!(run_rows(&mut held), off, "on and off differ");
    assert!(
        conn.adaptive_sort_checks() > checks,
        "the join's sorter was not counted"
    );
    assert_eq!(conn.adaptive_replans() - replans, 1, "planned again once");
    let (checks, replans) = (conn.adaptive_sort_checks(), conn.adaptive_replans());
    assert_eq!(run_rows(&mut held), off, "the second run's answer");
    assert!(
        conn.adaptive_sort_checks() > checks,
        "the second sorter was not counted"
    );
    assert_eq!(conn.adaptive_replans(), replans, "a second re-plan");
    assert_eq!(
        run_rows(&mut conn.prepare(sql).unwrap()),
        off,
        "a new statement's answer"
    );
    assert_eq!(
        conn.adaptive_replans(),
        replans,
        "a new statement planned again"
    );
}

/// Do: a hash join the planner chose itself (hash joins only), its build side a table filtered by
/// `b.v = 7 AND b.w < 50`: the `=` priced by the column statistics (every value distinct as ANALYZE
/// counted them), the range by the sample (one row in a hundred), a mix the sample does not run
/// together; then half the rows set to `v = 7` (the count unchanged); run with adaptive joins off,
/// then on twice in one prepared statement, and in a statement prepared after. Expect: one answer
/// every way; on, the filtered build counted against the share the plan read for both terms
/// together (their product), past its error the counted share stored and the statement planned
/// again once; nothing plans again after (PL.13, lane replan-10, T-PL.13.16: a mix of filter terms
/// at a build's materialization point).
#[test]
fn t_pl_13_16_a_build_filtered_by_a_mix_of_terms_plans_again() {
    let (_dir, conn) = open("mixed_filter_build.db");
    conn.execute("CREATE TABLE a (id INTEGER PRIMARY KEY, k)")
        .unwrap();
    conn.execute("CREATE TABLE b (id INTEGER PRIMARY KEY, k, v, w)")
        .unwrap();
    conn.execute(
        "INSERT INTO a(k) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 100) SELECT i % 20 FROM n",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO b(k, v, w) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000) SELECT i % 20, i, i FROM n",
    )
    .unwrap();
    conn.execute("ANALYZE").unwrap();
    conn.execute("UPDATE b SET v = 7 WHERE id % 2 = 0").unwrap();
    conn.set_join_methods(JoinMethods::HashJoinsOnly);
    let sql = "SELECT a.id, b.id FROM a JOIN b ON b.k = a.k WHERE b.v = 7 AND b.w < 50";

    conn.set_adaptive_joins(false);
    let (checks, corrections) = (
        conn.adaptive_filter_checks(),
        conn.adaptive_filter_corrections(),
    );
    let off = rows(&conn, sql);
    // b's even ids under 50 (24 rows) hold 7, each meeting the five rows of a with its k.
    assert_eq!(off.len(), 24 * 5, "the join's rows");
    assert_eq!(conn.adaptive_filter_checks(), checks, "counted off");

    conn.set_adaptive_joins(true);
    let replans = conn.adaptive_replans();
    let mut held = conn.prepare(sql).unwrap();
    assert_eq!(collect(&mut held), off, "on and off differ");
    assert!(
        conn.adaptive_filter_checks() > checks,
        "the filtered build was not counted"
    );
    assert_eq!(
        conn.adaptive_filter_corrections() - corrections,
        1,
        "the counted share stored"
    );
    assert_eq!(conn.adaptive_replans() - replans, 1, "planned again once");

    let replans = conn.adaptive_replans();
    assert_eq!(collect(&mut held), off, "the second run's answer");
    assert_eq!(
        rows(&conn, sql),
        off,
        "a statement planned on the stored share"
    );
    assert_eq!(conn.adaptive_replans(), replans, "a second re-plan");
    assert_eq!(
        conn.adaptive_filter_corrections() - corrections,
        1,
        "a second share stored"
    );
}

/// Do: a materialized CTE `o` over another materialized CTE `i` (all of b), `o` filtered by a range
/// on `i`'s column, which no statistics estimate (19 of b's 20 keys pass); run with adaptive joins
/// off, then on twice in one prepared statement, and in a statement prepared after. Expect: one
/// answer, in order, every way; on, `o`'s stored result counted against the rows the plan read for
/// it, with the band of the statistics of b that `i` reads, past it stored and the statement planned
/// again once; nothing plans again after (PL.13, lane replan-10, T-PL.13.17: a stored subquery
/// result over another subquery at its materialization point).
#[test]
fn t_pl_13_17_a_stored_subquery_over_a_subquery_plans_again() {
    let (_dir, conn) = open("subquery_over_subquery.db");
    conn.execute("CREATE TABLE b (id INTEGER PRIMARY KEY, k, v)")
        .unwrap();
    conn.execute(
        "INSERT INTO b(k, v) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000) SELECT i % 20, i FROM n",
    )
    .unwrap();
    conn.execute("ANALYZE").unwrap();
    let sql = "WITH i AS MATERIALIZED (SELECT id, k FROM b), o AS MATERIALIZED (SELECT id, k FROM i WHERE k < 19) SELECT id, k FROM o ORDER BY id";

    conn.set_adaptive_joins(false);
    let checks = conn.adaptive_subquery_checks();
    let off = run_rows(&mut conn.prepare(sql).unwrap());
    assert_eq!(off.len(), 4750, "the rows");
    assert_eq!(conn.adaptive_subquery_checks(), checks, "counted off");

    conn.set_adaptive_joins(true);
    let replans = conn.adaptive_replans();
    let mut held = conn.prepare(sql).unwrap();
    assert_eq!(run_rows(&mut held), off, "on and off differ");
    assert!(
        conn.adaptive_subquery_checks() > checks,
        "the stored results were not counted"
    );
    assert_eq!(conn.adaptive_replans() - replans, 1, "planned again once");
    let replans = conn.adaptive_replans();
    assert_eq!(run_rows(&mut held), off, "the second run's answer");
    assert_eq!(conn.adaptive_replans(), replans, "a second re-plan");
    assert_eq!(
        run_rows(&mut conn.prepare(sql).unwrap()),
        off,
        "a new statement's answer"
    );
    assert_eq!(
        conn.adaptive_replans(),
        replans,
        "a new statement planned again"
    );
}

/// The rows each `opcode` of the statement's program is checked against, as its EXPLAIN comment
/// states them: the number after the comment's last `marker`, up to " rows".
fn planned_rows(conn: &Arc<Connection>, sql: &str, opcode: &str, marker: &str) -> Vec<f64> {
    let mut stmt = conn.prepare(format!("EXPLAIN {sql}")).unwrap();
    let program = stmt.run_collect_rows().unwrap();
    program
        .into_iter()
        .filter(|row| row[1].to_string() == opcode)
        .map(|row| {
            let comment = row[7].to_string();
            let after = comment.rsplit(marker).next().unwrap_or_default();
            let rows = after.split(" rows").next().unwrap_or_default();
            rows.trim()
                .parse::<f64>()
                .unwrap_or_else(|_| panic!("{opcode}'s planned rows in {comment:?}"))
        })
        .collect()
}

/// Do: three tables chained by hash joins only (`r.k = s.k`, `s.j = t.j`), so the third join's
/// build input is materialized from a two-table prefix (`MATERIALIZE hash build input`, the
/// materialization PL.6 requires): `{r, s}` (estimated at 20 rows: `r`'s 20 rows hold two values of
/// `k`, `s`'s 200 rows distinct ones, none of them `r`'s) or `{s, t}` (2,000: each of `s`'s 200
/// values of `j` ten rows of `t`). Which one the first plan holds is read from its EXPLAIN (the rows
/// its `SubqueryCounted` checks), and only then the data changed, the tables' counts unchanged, so
/// that prefix's rows are ten times or more off its estimate and the other's on it: for `{r, s}`
/// every `s.k` set to 1 (2,000 rows; `{s, t}` 1,990), for `{s, t}` two of `s`'s keys set to 1 (`{r,
/// s}` 20 rows) and `t`'s values of `j` past 20 moved off `s`'s (`{s, t}` 200). Whatever the planner
/// picks, the test's premise is forced, not guessed. Run with adaptive joins off, then on twice in
/// one prepared statement, and in a statement prepared after. Expect: one answer every way; off,
/// nothing counted; on, the prefix counted, past its error the count stored and the statement
/// planned again once; nothing plans again after, whichever prefix the new plan holds (PL.13, lane
/// replan-11, robust in lane replan-12, T-PL.13.18: a build fed by a materialized input, at its
/// materialization point).
#[test]
fn t_pl_13_18_a_build_fed_by_a_materialized_prefix_plans_again() {
    let (_dir, conn) = open("materialized_prefix.db");
    for sql in [
        "CREATE TABLE r (id INTEGER PRIMARY KEY, k)",
        "CREATE TABLE s (id INTEGER PRIMARY KEY, k, j)",
        "CREATE TABLE t (id INTEGER PRIMARY KEY, j)",
        "INSERT INTO r(k) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 20) SELECT i % 2 FROM n",
        "INSERT INTO s(k, j) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 200) SELECT i + 1000, i FROM n",
        "INSERT INTO t(j) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 2000) SELECT i % 200 FROM n",
        "ANALYZE",
    ] {
        conn.execute(sql).unwrap();
    }
    conn.set_join_methods(JoinMethods::HashJoinsOnly);
    let sql = "SELECT count(*) FROM r JOIN s ON s.k = r.k JOIN t ON t.j = s.j";

    // The plan first, on the statistics alone: the UPDATEs below change no table's count.
    conn.set_adaptive_joins(true);
    let (steps, _) = steps_and_opcodes(&conn, sql);
    assert!(
        steps
            .iter()
            .any(|step| step.contains("MATERIALIZE hash build input")),
        "the prefix is not materialized: {steps:?}"
    );
    let planned = planned_rows(&conn, sql, "SubqueryCounted", "subquery's ");
    let [estimate] = planned[..] else {
        panic!("one materialized prefix counted: {planned:?} in {steps:?}");
    };
    // `{r, s}`'s 20 rows or `{s, t}`'s 2,000: the nearer, by ratio.
    let (updates, count): (&[&str], u64) = if estimate * estimate < 20.0 * 2000.0 {
        // r's ten rows with k = 1, each with all of s; s.j 1 to 199 ten rows of t each, 200 none.
        (&["UPDATE s SET k = 1"], 10 * 199 * 10)
    } else {
        // s's ids 1 and 2 (j 1 and 2) with r's ten rows with k = 1, each with t's ten rows of its j.
        (
            &[
                "UPDATE s SET k = 1 WHERE id <= 2",
                "UPDATE t SET j = j + 1000 WHERE j > 20",
            ],
            2 * 10 * 10,
        )
    };
    for update in updates {
        conn.execute(update).unwrap();
    }

    conn.set_adaptive_joins(false);
    let checks = conn.adaptive_subquery_checks();
    let off = run_rows(&mut conn.prepare(sql).unwrap());
    assert_eq!(off.len(), 1, "the count's row");
    assert!(
        off[0].contains(&count.to_string()),
        "the count (the prefix at {estimate} rows): {off:?}"
    );
    assert_eq!(conn.adaptive_subquery_checks(), checks, "counted off");

    conn.set_adaptive_joins(true);
    assert_eq!(
        planned_rows(&conn, sql, "SubqueryCounted", "subquery's "),
        planned,
        "the plan after the UPDATEs"
    );
    let replans = conn.adaptive_replans();
    let mut held = conn.prepare(sql).unwrap();
    assert_eq!(run_rows(&mut held), off, "on and off differ");
    assert!(
        conn.adaptive_subquery_checks() > checks,
        "the materialized prefix was not counted"
    );
    assert_eq!(
        conn.adaptive_replans() - replans,
        1,
        "planned again once (the prefix at {estimate} rows)"
    );
    let replans = conn.adaptive_replans();
    assert_eq!(run_rows(&mut held), off, "the second run's answer");
    assert_eq!(conn.adaptive_replans(), replans, "a second re-plan");
    assert_eq!(
        run_rows(&mut conn.prepare(sql).unwrap()),
        off,
        "a new statement's answer"
    );
    assert_eq!(
        conn.adaptive_replans(),
        replans,
        "a new statement planned again"
    );
}

/// Do: T-PL.13.15's join as a LEFT JOIN: `a` (1,000 rows, keys 1 to 1,000) LEFT JOIN `b` (5,000
/// rows, keys distinct at ANALYZE: about one match a row of `a`, 1,000 estimated), then every
/// `b.k` set to its tenth, so `a`'s keys 1 to 9 meet 500 rows of `b` each and the other 991 rows of
/// `a` none, kept null-extended (5,491), the tables' counts unchanged; ORDER BY columns of both
/// sides, so a sorter holds the join's rows. Run with adaptive joins off, then on twice in one
/// prepared statement, and in a statement prepared after. Expect: one answer, in order, every way;
/// on, the sorter counted against the rows the plan read for the whole LEFT JOIN, past its error
/// the count stored and the statement planned again once; nothing plans again after (PL.13, lane
/// replan-11, T-PL.13.19: an outer join feeding a sorter, at its materialization point).
#[test]
fn t_pl_13_19_a_sorter_fed_by_a_left_join_past_the_estimate_plans_again() {
    let (_dir, conn) = open("left_joined_sorter.db");
    conn.execute("CREATE TABLE a (id INTEGER PRIMARY KEY, k)")
        .unwrap();
    conn.execute("CREATE TABLE b (id INTEGER PRIMARY KEY, k, v)")
        .unwrap();
    conn.execute(
        "INSERT INTO a(k) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 1000) SELECT i FROM n",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO b(k, v) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000) SELECT i, i % 7 FROM n",
    )
    .unwrap();
    conn.execute("ANALYZE").unwrap();
    conn.execute("UPDATE b SET k = k % 10").unwrap();
    let sql = "SELECT a.id, b.id, b.v FROM a LEFT JOIN b ON b.k = a.k ORDER BY b.v, b.id, a.id";

    conn.set_adaptive_joins(false);
    let checks = conn.adaptive_sort_checks();
    let off = run_rows(&mut conn.prepare(sql).unwrap());
    assert_eq!(off.len(), 9 * 500 + 991, "the rows");
    assert_eq!(conn.adaptive_sort_checks(), checks, "counted off");

    conn.set_adaptive_joins(true);
    let replans = conn.adaptive_replans();
    let mut held = conn.prepare(sql).unwrap();
    assert_eq!(run_rows(&mut held), off, "on and off differ");
    assert!(
        conn.adaptive_sort_checks() > checks,
        "the LEFT JOIN's sorter was not counted"
    );
    assert_eq!(conn.adaptive_replans() - replans, 1, "planned again once");
    let replans = conn.adaptive_replans();
    assert_eq!(run_rows(&mut held), off, "the second run's answer");
    assert_eq!(conn.adaptive_replans(), replans, "a second re-plan");
    assert_eq!(
        run_rows(&mut conn.prepare(sql).unwrap()),
        off,
        "a new statement's answer"
    );
    assert_eq!(
        conn.adaptive_replans(),
        replans,
        "a new statement planned again"
    );
}

/// Do: an EXISTS and a NOT EXISTS of `b` on `a`'s key, planned by hash joins only (each unnested
/// to a hash semi or anti join), ORDER BY `a.k` DESC (no index on it: a sorter holds the rows
/// kept). At ANALYZE `b`'s 5,000 keys are ten values none of `a`'s 1,000 holds. For each, the rows
/// the plan checks its sorter against are read from its EXPLAIN first, and only then `b`'s keys
/// set, the tables' counts unchanged, so the rows kept are the farther of none and all 1,000 from
/// that estimate (`b`'s keys covering all of `a`'s, or none): at least twice or half of it, past
/// any band of statistics' error under one, whatever the planner estimates. Run each with adaptive
/// joins off, then on twice in one prepared statement, and in a statement prepared after. Expect:
/// one answer, in order, every way; on, the sorter counted against the rows the plan read for the
/// whole join, past its error the count stored and the statement planned again once; nothing plans
/// again after (PL.13, lane replan-11, robust in lane replan-12, T-PL.13.20: a semi or anti join
/// feeding a sorter, at its materialization point).
#[test]
fn t_pl_13_20_a_sorter_fed_by_a_semi_or_anti_join_past_the_estimate_plans_again() {
    let (_dir, conn) = open("semi_joined_sorter.db");
    for sql in [
        "CREATE TABLE a (id INTEGER PRIMARY KEY, k)",
        "CREATE TABLE b (id INTEGER PRIMARY KEY, k)",
        "INSERT INTO a(k) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 1000) SELECT i FROM n",
        "INSERT INTO b(k) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000) SELECT 2000 + i % 10 FROM n",
        "ANALYZE",
    ] {
        conn.execute(sql).unwrap();
    }
    conn.set_join_methods(JoinMethods::HashJoinsOnly);
    // Whether the case keeps a's rows where b's keys cover a's.
    for (case, sql, kept_when_covered) in [
        (
            "an EXISTS",
            "SELECT a.id, a.k FROM a WHERE EXISTS (SELECT 1 FROM b WHERE b.k = a.k) ORDER BY a.k DESC",
            true,
        ),
        (
            "a NOT EXISTS",
            "SELECT a.id, a.k FROM a WHERE NOT EXISTS (SELECT 1 FROM b WHERE b.k = a.k) ORDER BY a.k DESC",
            false,
        ),
    ] {
        conn.set_adaptive_joins(true);
        let planned = planned_rows(&conn, sql, "SorterCounted", " at ");
        let [estimate] = planned[..] else {
            panic!("{case}: one sorter counted against the join: {planned:?}");
        };
        let all = estimate < 500.0;
        let rows_expected = if all { 1000 } else { 0 };
        let cover = all == kept_when_covered;
        conn.execute(if cover {
            "UPDATE b SET k = id % 1000 + 1"
        } else {
            "UPDATE b SET k = 2000 + id % 10"
        })
        .unwrap();
        assert_eq!(
            planned_rows(&conn, sql, "SorterCounted", " at "),
            planned,
            "{case}: the plan after the UPDATE"
        );

        conn.set_adaptive_joins(false);
        let checks = conn.adaptive_sort_checks();
        let off = run_rows(&mut conn.prepare(sql).unwrap());
        assert_eq!(off.len(), rows_expected, "{case}: the rows");
        assert_eq!(conn.adaptive_sort_checks(), checks, "{case}: counted off");

        conn.set_adaptive_joins(true);
        let replans = conn.adaptive_replans();
        let mut held = conn.prepare(sql).unwrap();
        assert_eq!(run_rows(&mut held), off, "{case}: on and off differ");
        assert!(
            conn.adaptive_sort_checks() > checks,
            "{case}: the sorter was not counted"
        );
        assert_eq!(
            conn.adaptive_replans() - replans,
            1,
            "{case}: planned again once"
        );
        let replans = conn.adaptive_replans();
        assert_eq!(run_rows(&mut held), off, "{case}: the second run's answer");
        assert_eq!(conn.adaptive_replans(), replans, "{case}: a second re-plan");
        assert_eq!(
            run_rows(&mut conn.prepare(sql).unwrap()),
            off,
            "{case}: a new statement's answer"
        );
        assert_eq!(
            conn.adaptive_replans(),
            replans,
            "{case}: a new statement planned again"
        );
    }
}

/// Do: a SELECT DISTINCT of `b.k` ORDER BY it descending (no index on it: a sorter after the
/// DISTINCT holds its rows), over 5,000 rows holding 20 values of `k`, unfiltered and filtered by a
/// range; the plan estimates the DISTINCT's rows as the rows read, which its EXPLAIN states and
/// the test checks first (at least twice the 20 distinct rows, past any band of statistics' error
/// under one). Run each with adaptive joins off, then on twice in one prepared statement, and in a
/// statement prepared after. Expect: one answer, in order, every way; off, no sorter counted; on,
/// the sorter's distinct rows counted against the DISTINCT's own estimate, past its error the
/// count stored under the DISTINCT's identity and the statement planned again once; nothing plans
/// again after, the new plans reading the stored count (PL.13, lane replan-12, T-PL.13.21: a
/// DISTINCT sorter's own count, at its materialization point).
#[test]
fn t_pl_13_21_a_distinct_sorters_rows_past_the_estimate_plan_again() {
    let (_dir, conn) = open("distinct_sorter.db");
    for sql in [
        "CREATE TABLE b (id INTEGER PRIMARY KEY, k, v)",
        "INSERT INTO b(k, v) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000) SELECT i % 20, i FROM n",
        "ANALYZE",
    ] {
        conn.execute(sql).unwrap();
    }
    for (case, sql) in [
        ("unfiltered", "SELECT DISTINCT k FROM b ORDER BY k DESC"),
        (
            "filtered",
            "SELECT DISTINCT k FROM b WHERE v > 0 ORDER BY k DESC",
        ),
    ] {
        conn.set_adaptive_joins(true);
        let planned = planned_rows(&conn, sql, "SorterCounted", " at ");
        let [estimate] = planned[..] else {
            panic!("{case}: one sorter counted against the DISTINCT: {planned:?}");
        };
        assert!(
            estimate >= 2.0 * 20.0,
            "{case}: the DISTINCT estimated at {estimate} rows, near its 20"
        );

        conn.set_adaptive_joins(false);
        let checks = conn.adaptive_sort_checks();
        let off = run_rows(&mut conn.prepare(sql).unwrap());
        assert_eq!(off.len(), 20, "{case}: the distinct rows");
        assert_eq!(conn.adaptive_sort_checks(), checks, "{case}: counted off");

        conn.set_adaptive_joins(true);
        let replans = conn.adaptive_replans();
        let mut held = conn.prepare(sql).unwrap();
        assert_eq!(run_rows(&mut held), off, "{case}: on and off differ");
        assert!(
            conn.adaptive_sort_checks() > checks,
            "{case}: the DISTINCT's sorter was not counted"
        );
        assert_eq!(
            conn.adaptive_replans() - replans,
            1,
            "{case}: planned again once"
        );
        let (checks, replans) = (conn.adaptive_sort_checks(), conn.adaptive_replans());
        assert_eq!(run_rows(&mut held), off, "{case}: the second run's answer");
        assert!(
            conn.adaptive_sort_checks() > checks,
            "{case}: the second sorter was not counted"
        );
        assert_eq!(conn.adaptive_replans(), replans, "{case}: a second re-plan");
        assert_eq!(
            run_rows(&mut conn.prepare(sql).unwrap()),
            off,
            "{case}: a new statement's answer"
        );
        assert_eq!(
            conn.adaptive_replans(),
            replans,
            "{case}: a new statement planned again"
        );
        assert_eq!(
            planned_rows(&conn, sql, "SorterCounted", " at "),
            vec![20.0],
            "{case}: a new plan reads the stored count"
        );
    }
}
