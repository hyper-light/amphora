//! Learned plan hints (Bao) trained on measured runs and checked on held-out queries against the
//! cost model alone (G6 step 3, AC-G6.3.1, T-G6.3.1; `optimizer::plan_hints`).
//!
//! The queries (each `*.sql` file of `--queries`, one statement a file, or `--tiny`'s own) are
//! sorted by name and split in turn: the first, third, fifth... train the model, the rest are held
//! out, so none is picked. Training: `--rounds` passes over the training queries, each run under
//! the hint set Thompson sampling chooses from the model, its time training the model. Held out:
//! each query is its own class; the model's best hint set for it is run against the unhinted plan
//! in turns, `--runs` times each, into the guard, which admits it or drops it; then the hint set
//! the guard leaves the class (`final`) and the unhinted plan are timed again in turns, best of
//! `--runs`. Every held-out query's answers under the unhinted plan, the proposed hint set and the
//! final one are compared with SQLite's on the same data (`--sqlite`). One JSON object a line on
//! stdout: a line per held-out query, then the summary. Exits 1 when any answer differs.
//!
//!   plan-hints --amphora <db> --sqlite <db> --queries <dir> --rounds <n> --runs <n> --seed <n>
//!   plan-hints --tiny <dir> --rounds <n> --runs <n> --seed <n>

use amphora_engine::{
    candidate_plans, ConnectOpts, Connection, Database, DatabaseOpts, HintChooser, HintSet,
    Numeric, OpenOptions, PlatformIO, SqliteDialect, Value,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

struct Opts {
    amphora: PathBuf,
    sqlite: PathBuf,
    queries: Vec<(String, String)>,
    rounds: usize,
    runs: usize,
    seed: u64,
}

fn main() {
    match run() {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("plan-hints: {e}");
            std::process::exit(2);
        }
    }
}

fn arg<T: std::str::FromStr>(args: &[String], name: &str) -> Result<T, String> {
    let at = args
        .iter()
        .position(|a| a == name)
        .ok_or_else(|| format!("{name} is required"))?;
    args.get(at + 1)
        .ok_or_else(|| format!("{name} needs a value"))?
        .parse()
        .map_err(|_| format!("{name}: not a valid value"))
}

fn opts() -> Result<Opts, String> {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.into_string().map_err(|_| "an argument is not Unicode".to_string()))
        .collect::<Result<_, _>>()?;
    let rounds = arg(&args, "--rounds")?;
    let runs: usize = arg(&args, "--runs")?;
    let seed = arg(&args, "--seed")?;
    if runs < 2 {
        return Err("--runs needs at least 2: the guard compares spreads".to_string());
    }
    if let Ok(dir) = arg::<PathBuf>(&args, "--tiny") {
        let (amphora, sqlite, queries) = tiny(&dir)?;
        return Ok(Opts { amphora, sqlite, queries, rounds, runs, seed });
    }
    let dir: PathBuf = arg(&args, "--queries")?;
    let mut queries = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().is_some_and(|e| e == "sql") {
            let sql = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            queries.push((name, sql.trim().trim_end_matches(';').to_string()));
        }
    }
    Ok(Opts {
        amphora: arg(&args, "--amphora")?,
        sqlite: arg(&args, "--sqlite")?,
        queries,
        rounds,
        runs,
        seed,
    })
}

fn open(path: &Path) -> Result<Arc<Connection>, String> {
    let io = Arc::new(PlatformIO::new().map_err(|e| e.to_string())?);
    let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
    let db = Database::open(io, &path.display().to_string(), options).map_err(|e| e.to_string())?;
    db.connect(ConnectOpts::new()).map_err(|e| e.to_string())
}

/// `--tiny`: three small tables with NULL and repeated join keys, the same in Amphora and SQLite,
/// and joins over them, inner, left, semi, anti, grouped and sorted.
fn tiny(dir: &Path) -> Result<(PathBuf, PathBuf, Vec<(String, String)>), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let (amphora, sqlite) = (dir.join("tiny.amphora"), dir.join("tiny.sqlite"));
    for path in [&amphora, &sqlite] {
        let _ = std::fs::remove_file(path);
    }
    let mut script = vec![
        "CREATE TABLE a (id INTEGER PRIMARY KEY, k, x)".to_string(),
        "CREATE TABLE b (id INTEGER PRIMARY KEY, k, y)".to_string(),
        "CREATE INDEX b_k ON b (k)".to_string(),
        "CREATE TABLE c (k, z)".to_string(),
        "CREATE INDEX c_z ON c (z)".to_string(),
    ];
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut key = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let draw = state % 220;
        if draw < 200 { (draw % 40).to_string() } else { "NULL".to_string() }
    };
    for id in 1..=2000u64 {
        script.push(format!("INSERT INTO a VALUES ({id}, {}, {})", key(), (id * 37) % 100));
        script.push(format!("INSERT INTO b VALUES ({id}, {}, {})", key(), id % 7));
        script.push(format!("INSERT INTO c VALUES ({}, {})", key(), (id * 13) % 100));
    }
    script.push("ANALYZE".to_string());
    let conn = open(&amphora)?;
    for sql in &script {
        conn.execute(sql).map_err(|e| format!("{sql}: {e}"))?;
    }
    let lite = rusqlite::Connection::open(&sqlite).map_err(|e| e.to_string())?;
    lite.execute_batch(&format!("BEGIN;{};COMMIT;", script.join(";"))).map_err(|e| e.to_string())?;
    let queries = [
        "SELECT a.id, b.id FROM a JOIN b ON b.k = a.k WHERE a.x < 20",
        "SELECT a.id, b.id FROM a LEFT JOIN b ON b.k = a.k WHERE a.x < 20",
        "SELECT a.id, c.z FROM a JOIN c ON c.k = a.k WHERE a.x < 10",
        "SELECT count(*) FROM a JOIN c ON c.k = a.k AND c.z > 40",
        "SELECT a.id FROM a WHERE EXISTS (SELECT 1 FROM b WHERE b.k = a.k)",
        "SELECT a.id FROM a WHERE NOT EXISTS (SELECT 1 FROM c WHERE c.k = a.k)",
        "SELECT count(*), sum(b.y) FROM a JOIN b ON b.k = a.k AND b.y = a.x % 7",
        "SELECT a.k, count(*) FROM a JOIN c ON c.k = a.k GROUP BY a.k",
        "SELECT c.z, count(*) FROM c JOIN a ON a.k = c.k WHERE c.z BETWEEN 10 AND 30 GROUP BY c.z ORDER BY c.z",
        "SELECT count(*) FROM a JOIN b ON b.k = a.k JOIN c ON c.k = b.k WHERE c.z < 5",
    ];
    let named = queries.iter().enumerate().map(|(i, q)| (format!("tiny{i:02}"), q.to_string())).collect();
    Ok((amphora, sqlite, named))
}

/// One run's nanoseconds: the statement prepared under the connection's hints, then stepped to
/// its end (the rows counted, not kept).
fn timed(conn: &Arc<Connection>, sql: &str) -> Result<f64, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| format!("{sql}: {e}"))?;
    let start = Instant::now();
    let mut rows = 0u64;
    stmt.run_with_row_callback(|_| {
        rows += 1;
        Ok(())
    })
    .map_err(|e| format!("{sql}: {e}"))?;
    let nanos = start.elapsed().as_nanos() as f64;
    Ok(nanos.max(1.0))
}

fn amphora_answer(conn: &Arc<Connection>, sql: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| format!("{sql}: {e}"))?;
    let rows = stmt.run_collect_rows().map_err(|e| format!("{sql}: {e}"))?;
    let mut out: Vec<String> = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|v| match v {
                    Value::Null => "NULL".to_string(),
                    Value::Numeric(Numeric::Integer(i)) => i.to_string(),
                    Value::Numeric(Numeric::Float(f)) => format!("{:?}", f64::from(*f)),
                    Value::Text(t) => t.as_str().to_string(),
                    // Blobs are compared as present: JOB's and the corpus's joins return none.
                    Value::Blob(_) => "blob".to_string(),
                })
                .collect::<Vec<_>>()
                .join("|")
        })
        .collect();
    out.sort();
    Ok(out)
}

fn sqlite_answer(conn: &rusqlite::Connection, sql: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| format!("sqlite {sql}: {e}"))?;
    let columns = stmt.column_count();
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let mut cells = Vec::with_capacity(columns);
        for i in 0..columns {
            use rusqlite::types::ValueRef;
            cells.push(match row.get_ref(i).map_err(|e| e.to_string())? {
                ValueRef::Null => "NULL".to_string(),
                ValueRef::Integer(i) => i.to_string(),
                ValueRef::Real(f) => format!("{f:?}"),
                ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
                ValueRef::Blob(_) => "blob".to_string(),
            });
        }
        out.push(cells.join("|"));
    }
    out.sort();
    Ok(out)
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn run() -> Result<bool, String> {
    let opts = opts()?;
    let mut queries = opts.queries.clone();
    queries.sort();
    let (train, held): (Vec<_>, Vec<_>) = queries.iter().enumerate().partition(|(i, _)| i % 2 == 0);
    let conn = open(&opts.amphora)?;
    let lite = rusqlite::Connection::open(&opts.sqlite).map_err(|e| e.to_string())?;
    let mut chooser = HintChooser::new(opts.seed, held.len().max(1)).map_err(|e| e.to_string())?;

    // Training: Thompson sampling over the training queries, every run's time the model's.
    let mut train_runs = 0u64;
    for _ in 0..opts.rounds {
        for (_, (_, sql)) in &train {
            let candidates = candidate_plans(&conn, sql).map_err(|e| format!("{sql}: {e}"))?;
            let hint = chooser.explore(&candidates);
            let Some((_, features)) = candidates.iter().find(|(h, _)| *h == hint) else {
                return Err(format!("{sql}: the chosen hint set has no plan"));
            };
            hint.apply(&conn);
            let nanos = timed(&conn, sql);
            HintSet::UNHINTED.apply(&conn);
            chooser.model.observe(features, nanos?).map_err(|e| e.to_string())?;
            train_runs += 1;
        }
    }

    // Held out: the guard's trial of the model's choice, then the class's final plan timed.
    let (mut admitted, mut slower, mut differ) = (0usize, 0usize, 0usize);
    for (class, (_, (name, sql))) in held.iter().enumerate() {
        let class = class as u64;
        let candidates = candidate_plans(&conn, sql).map_err(|e| format!("{sql}: {e}"))?;
        let proposed = chooser.best(&candidates);
        let mut verdict = amphora_engine::Verdict::OnTrial;
        for _ in 0..opts.runs {
            HintSet::UNHINTED.apply(&conn);
            let base = timed(&conn, sql)?;
            chooser.guard.record(class, HintSet::UNHINTED, base).map_err(|e| e.to_string())?;
            if proposed != HintSet::UNHINTED {
                proposed.apply(&conn);
                let hinted = timed(&conn, sql)?;
                verdict = chooser.guard.record(class, proposed, hinted).map_err(|e| e.to_string())?;
            }
        }
        let fin = chooser.guard.use_for(class);
        let (mut base_best, mut final_best) = (f64::INFINITY, f64::INFINITY);
        for _ in 0..opts.runs {
            HintSet::UNHINTED.apply(&conn);
            base_best = base_best.min(timed(&conn, sql)?);
            fin.apply(&conn);
            final_best = final_best.min(timed(&conn, sql)?);
        }
        HintSet::UNHINTED.apply(&conn);
        let reference = sqlite_answer(&lite, sql)?;
        let mut same = amphora_answer(&conn, sql)? == reference;
        for hint in [proposed, fin] {
            hint.apply(&conn);
            same &= amphora_answer(&conn, sql)? == reference;
        }
        HintSet::UNHINTED.apply(&conn);
        let ratio = final_best / base_best;
        admitted += usize::from(fin != HintSet::UNHINTED);
        // AC-G6.3.1's bound: no held-out query slower by more than 10% with hints on. A class the
        // guard leaves on the unhinted plan runs without hints: its two timings differ by noise.
        slower += usize::from(fin != HintSet::UNHINTED && ratio > 1.10);
        differ += usize::from(!same);
        println!(
            "{{\"query\":{},\"proposed\":{},\"verdict\":\"{verdict:?}\",\"final\":{},\"unhinted_ns\":{base_best:.0},\"final_ns\":{final_best:.0},\"ratio\":{ratio:.4},\"answers\":\"{}\"}}",
            json_str(name),
            json_str(&proposed.name()),
            json_str(&fin.name()),
            if same { "identical to sqlite" } else { "DIFFER" },
        );
    }
    println!(
        "{{\"summary\":true,\"train_queries\":{},\"held_out\":{},\"train_runs\":{train_runs},\"model_runs\":{},\"admitted\":{admitted},\"slower_than_10pct\":{slower},\"answers_differ\":{differ},\"guard_classes\":{}}}",
        train.len(),
        held.len(),
        chooser.model.runs(),
        chooser.guard.len(),
    );
    Ok(differ == 0)
}
