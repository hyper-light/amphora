//! Lane C21 probe: reads a case file (db/sql/q lines, tab separated), prints each query under each setting that differs
//! from its third field (or every answer when there is none).
use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, JoinMethods, MemoryIO, Numeric, OpenOptions,
    SqliteDialect, StepResult, Value, IO,
};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn open() -> (Arc<MemoryIO>, Arc<Connection>) {
    static N: AtomicUsize = AtomicUsize::new(0);
    let io = Arc::new(MemoryIO::new());
    let options = OpenOptions::new(Arc::new(SqliteDialect))
        .db_opts(DatabaseOpts::new().with_without_rowid(true));
    let name = format!("probe_{}.db", N.fetch_add(1, Ordering::Relaxed));
    let db = Database::open(io.clone(), &name, options).unwrap();
    (io, db.connect(ConnectOpts::new()).unwrap())
}

fn answer(io: &MemoryIO, conn: &Arc<Connection>, sql: &str) -> Result<String, String> {
    let mut stmt = conn.prepare(sql).map_err(|error| error.to_string())?;
    let mut lines = Vec::new();
    loop {
        match stmt.step().map_err(|error| error.to_string())? {
            StepResult::Row => {
                let row = stmt.row().unwrap();
                let values: Vec<String> = row
                    .get_values()
                    .map(|value| match value {
                        Value::Null => "NULL".to_owned(),
                        Value::Numeric(Numeric::Float(f)) => format!("{:?}", f64::from(*f)),
                        Value::Text(t) => format!("\x27{}\x27", t.as_str()),
                        Value::Blob(b) => format!(
                            "x\x27{}\x27",
                            b.iter().map(|x| format!("{x:02x}")).collect::<String>()
                        ),
                        value => value.to_string(),
                    })
                    .collect();
                lines.push(values.join(","));
            }
            StepResult::Done => break,
            StepResult::IO => io.step().map_err(|error| error.to_string())?,
            other => return Err(format!("step: {other:?}")),
        }
    }
    if !sql.starts_with("EXPLAIN") {
        lines.sort();
    }
    let sep = if sql.starts_with("EXPLAIN") {
        "\n"
    } else {
        ";"
    };
    Ok(format!("{}:{}", lines.len(), lines.join(sep)))
}

fn run() {
    let path = std::env::args().nth(1).expect("case file");
    let only = std::env::args().nth(2);
    let text = std::fs::read_to_string(path).unwrap();
    let mut dbs: Vec<(String, Vec<String>, Vec<(String, Option<String>)>)> = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split("\t").collect();
        match fields.as_slice() {
            ["db", name] => dbs.push((name.to_string(), Vec::new(), Vec::new())),
            ["db", kind, name] => dbs.push((format!("{kind}:{name}"), Vec::new(), Vec::new())),
            ["sql", sql] => dbs.last_mut().unwrap().1.push(sql.to_string()),
            ["q", sql] => dbs.last_mut().unwrap().2.push((sql.to_string(), None)),
            ["q", sql, want] => dbs
                .last_mut()
                .unwrap()
                .2
                .push((sql.to_string(), Some(want.to_string()))),
            _ => assert!(line.is_empty() || line.starts_with("#"), "{line}"),
        }
    }
    let settings = [
        ("default", false, false, JoinMethods::ByCost),
        ("parallel", true, false, JoinMethods::ByCost),
        ("stats", false, true, JoinMethods::ByCost),
        ("both", true, true, JoinMethods::ByCost),
        ("hash", false, false, JoinMethods::HashJoinsOnly),
    ];
    let mut total = [(0usize, 0usize, 0usize, 0usize); 5];
    for (name, fill, queries) in &dbs {
        for (s, (label, parallel, stats, joins)) in settings.iter().enumerate() {
            if only.as_ref().is_some_and(|o| o != label) {
                continue;
            }
            let (mut io, mut conn) = open();
            let fill_in = |conn: &Arc<Connection>| {
                for statement in fill {
                    if let Err(error) = conn.execute(statement.as_str()) {
                        println!("FILL\t{name}\t{label}\t{statement}\t{error}");
                    }
                }
                if *parallel {
                    conn.set_parallel_mode(amphora_engine::morsel::ParallelMode::On);
                }
                if *stats {
                    conn.execute("ANALYZE").unwrap();
                }
                conn.set_join_methods(*joins);
            };
            fill_in(&conn);
            for (sql, want) in queries {
                let got = match catch_unwind(AssertUnwindSafe(|| answer(&io, &conn, sql))) {
                    Ok(Ok(rows)) => rows,
                    Ok(Err(error)) => format!("ERROR {error}"),
                    Err(panic) => {
                        let said = panic
                            .downcast_ref::<String>()
                            .cloned()
                            .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                            .unwrap_or_default();
                        (io, conn) = open();
                        fill_in(&conn);
                        format!("PANIC {said}")
                    }
                };
                total[s].0 += 1;
                match want {
                    Some(want) if *want == got => {}
                    Some(want) => {
                        total[s].1 += 1;
                        if got.starts_with("ERROR") {
                            total[s].2 += 1;
                        }
                        if got.starts_with("PANIC") {
                            total[s].3 += 1;
                        }
                        println!("DIFF\t{name}\t{label}\t{sql}\n\twant {want}\n\tgot  {got}");
                    }
                    None => println!("ANSWER\t{name}\t{label}\t{sql}\n\t{got}"),
                }
            }
        }
    }
    for (s, (label, ..)) in settings.iter().enumerate() {
        println!(
            "TOTAL\t{label}\t{} run, {} differ ({} errors, {} panics)",
            total[s].0, total[s].1, total[s].2, total[s].3
        );
    }
}

fn main() {
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(run)
        .unwrap()
        .join()
        .unwrap();
}
