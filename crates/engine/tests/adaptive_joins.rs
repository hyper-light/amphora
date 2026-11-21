//! Amphora (PL.13, T-PL.13.1; `docs/design/planner.md`, "Decision (lane pl13)"): the adaptive
//! join on the real engine. Random tables with NULL and duplicate keys are joined with the switch
//! on and off (`Connection::set_adaptive_joins`); the answers must be identical, and equal to the
//! nested loop computed here from the rows. The switch's count
//! (`Connection::adaptive_join_switches`) must move when the outer rows reach the switch point the
//! statement's plan states, and not before.

use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, JoinMethods, Numeric, OpenOptions, PlatformIO,
    SqliteDialect, Value,
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
    // Every join whose plan states a point under its outer rows switched.
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
    let Some(point) = switch_point(&conn, &sql(1)) else {
        // No break-even under this machine's learned costs: the join never switches.
        let before = conn.adaptive_join_switches();
        rows(&conn, "SELECT a.id, b.id FROM a JOIN b ON b.k = a.k");
        assert_eq!(conn.adaptive_join_switches(), before);
        return;
    };
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
