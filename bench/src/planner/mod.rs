//! The planner bench (`--planner`, bench/planner/README.md): JOB (the IMDb tables and the 113
//! queries of gregrahn/join-order-benchmark) or TPC-H (dbgen's tables at a scale factor and its 22
//! queries) loaded into Amphora, SQLite, DuckDB and PostgreSQL alike, analyzed, and every query
//! run on each. Amphora's runs read back each table access's and join's rows beside the
//! planner's estimate of them (`Connection::set_row_counters`); every engine's runs are timed,
//! best of `--runs`, each begun once the load average is under `--max-load`. Each line of
//! `--out` is a JSON record: the run's hardware, commit, date and engines first, then one a
//! table loaded and one a query and engine. bench/planner/summary.py reads them.

mod data;
mod engines;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use data::Format;
use engines::{for_dialect, Amphora, Count, Duck, Outcome, Postgres, Sqlite, Target};

struct Opts {
    bench: String,
    data: PathBuf,
    queries: PathBuf,
    sf: f64,
    dir: PathBuf,
    targets: Vec<String>,
    runs: usize,
    timeout: Duration,
    count_timeout: Duration,
    dsn: String,
    cache_kib: i64,
    only: Vec<String>,
    max_load: f64,
    load_wait: Duration,
    out: PathBuf,
}

fn opts() -> anyhow::Result<Opts> {
    let mut o = Opts {
        bench: String::new(),
        data: PathBuf::new(),
        queries: PathBuf::new(),
        sf: 1.0,
        dir: PathBuf::new(),
        targets: vec!["amphora".into()],
        runs: 3,
        timeout: Duration::from_secs(600),
        count_timeout: Duration::from_secs(1800),
        dsn: "host=127.0.0.1 port=55432 user=postgres".into(),
        cache_kib: 2 << 20,
        only: Vec::new(),
        max_load: 2.0,
        load_wait: Duration::from_secs(3600),
        out: PathBuf::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut v = || args.next().ok_or_else(|| anyhow::anyhow!("{a} takes a value"));
        match a.as_str() {
            "--planner" => {}
            "--bench" => o.bench = v()?,
            "--data" => o.data = v()?.into(),
            "--queries" => o.queries = v()?.into(),
            "--sf" => o.sf = v()?.parse()?,
            "--dir" => o.dir = v()?.into(),
            "--targets" => o.targets = v()?.split(',').map(str::to_owned).collect(),
            "--runs" => o.runs = v()?.parse()?,
            "--timeout-s" => o.timeout = Duration::from_secs(v()?.parse()?),
            "--count-timeout-s" => o.count_timeout = Duration::from_secs(v()?.parse()?),
            "--dsn" => o.dsn = v()?,
            "--cache-mib" => o.cache_kib = v()?.parse::<i64>()? * 1024,
            "--only" => o.only = v()?.split(',').map(str::to_owned).collect(),
            "--max-load" => o.max_load = v()?.parse()?,
            "--load-wait-s" => o.load_wait = Duration::from_secs(v()?.parse()?),
            "--out" => o.out = v()?.into(),
            other => anyhow::bail!("unknown argument {other} (--planner takes --bench job|tpch, --data, --queries, --sf, --dir, --targets, --runs, --timeout-s, --count-timeout-s, --dsn, --cache-mib, --only, --max-load, --load-wait-s, --out)"),
        }
    }
    anyhow::ensure!(matches!(o.bench.as_str(), "job" | "tpch"), "--bench is job or tpch");
    anyhow::ensure!(!o.data.as_os_str().is_empty() && !o.queries.as_os_str().is_empty() && !o.dir.as_os_str().is_empty() && !o.out.as_os_str().is_empty(), "--planner needs --data, --queries, --dir and --out");
    anyhow::ensure!(o.runs > 0, "--runs is at least 1");
    Ok(o)
}

/// The benchmark's schema, its indexes, its tables' files and its queries, by name.
struct Bench {
    schema: String,
    indexes: String,
    tables: Vec<(String, PathBuf)>,
    format: Format,
    queries: Vec<(String, String)>,
    /// The revision of the queries' or the data's source.
    source_revision: Option<String>,
}

const TPCH_TABLES: [&str; 8] = ["region", "nation", "part", "supplier", "partsupp", "customer", "orders", "lineitem"];

fn bench(o: &Opts) -> anyhow::Result<Bench> {
    let read = |p: PathBuf| std::fs::read_to_string(&p).map_err(|e| anyhow::anyhow!("{}: {e}", p.display()));
    if o.bench == "job" {
        let schema = read(o.queries.join("schema.sql"))?;
        // Every CREATE TABLE's name, in order.
        let tables = schema
            .split("CREATE TABLE ")
            .skip(1)
            .filter_map(|s| s.split_whitespace().next())
            .map(|t| (t.to_owned(), o.data.join(format!("{t}.csv"))))
            .collect();
        let mut queries: Vec<(String, String)> = Vec::new();
        for entry in std::fs::read_dir(&o.queries)? {
            let path = entry?.path();
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_owned();
            if path.extension().is_some_and(|e| e == "sql") && name.starts_with(|c: char| c.is_ascii_digit()) {
                queries.push((name, read(path)?));
            }
        }
        // 1a, 1b, ... 2a, ... 33c: by number, then letter.
        queries.sort_by_key(|(n, _)| {
            let digits: String = n.chars().take_while(char::is_ascii_digit).collect();
            (digits.parse::<u32>().unwrap_or(u32::MAX), n.clone())
        });
        let revision = std::process::Command::new("git").arg("-C").arg(&o.queries).args(["rev-parse", "HEAD"]).output().ok().map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned());
        return Ok(Bench { schema, indexes: read(o.queries.join("fkindexes.sql"))?, tables, format: Format::Csv, queries, source_revision: revision });
    }
    let here = o.queries.parent().ok_or_else(|| anyhow::anyhow!("--queries for tpch is bench/planner/tpch/queries"))?;
    // TPC-H's Q11 keeps the parts worth more than 0.0001 of the whole at scale factor 1, less as it grows.
    let fraction = format!("{}", 0.0001 / o.sf);
    let mut queries = Vec::new();
    for n in 1..=22 {
        let name = format!("q{n:02}");
        queries.push((name.clone(), read(o.queries.join(format!("{name}.sql")))?.replace("{FRACTION}", &fraction)));
    }
    Ok(Bench {
        schema: read(here.join("schema.sql"))?,
        indexes: read(here.join("indexes.sql"))?,
        tables: TPCH_TABLES.iter().map(|t| (t.to_string(), o.data.join(format!("{t}.tbl")))).collect(),
        format: Format::Pipe,
        queries,
        source_revision: read(o.data.join("REVISION")).ok().map(|s| s.trim().to_owned()),
    })
}

/// Linux's one-minute load average.
#[cfg(not(windows))]
fn load_average() -> Option<f64> {
    std::fs::read_to_string("/proc/loadavg").ok()?.split_whitespace().next()?.parse().ok()
}

/// Windows keeps no load average. Its nearest: the cores busy (`% Processor Time` of every
/// core, as cores) and the threads waiting for one (`Processor Queue Length`), averaged over
/// three one-second samples, the running and runnable threads Linux's load counts.
#[cfg(windows)]
fn load_average() -> Option<f64> {
    let cores = std::thread::available_parallelism().map(usize::from).ok()? as f64;
    let out = powershell("$s = Get-Counter '\\Processor(_Total)\\% Processor Time','\\System\\Processor Queue Length' -SampleInterval 1 -MaxSamples 3; $busy = ($s | ForEach-Object { $_.CounterSamples[0].CookedValue } | Measure-Object -Average).Average; $queue = ($s | ForEach-Object { $_.CounterSamples[1].CookedValue } | Measure-Object -Average).Average; \"$busy $queue\"");
    let mut parts = out.split_whitespace().filter_map(|v| v.parse::<f64>().ok());
    let (busy, queue) = (parts.next()?, parts.next()?);
    Some(busy / 100.0 * cores + queue)
}

#[cfg(windows)]
fn powershell(script: &str) -> String {
    std::process::Command::new("powershell").args(["-NoProfile", "-Command", script]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_default()
}

/// Waits, a second at a time and for at most `wait`, until the load average is under `max`: the
/// load it began at.
fn quiet(max: f64, wait: Duration) -> Option<f64> {
    let until = Instant::now() + wait;
    loop {
        let load = load_average();
        if load.is_none_or(|l| l < max) || Instant::now() >= until {
            return load;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// The machine: its processor, memory, disk and system.
#[cfg(not(windows))]
fn host(dir: &Path) -> serde_json::Value {
    serde_json::json!({
        "host": sh("hostname -s"),
        "cpu": sh("lscpu 2>/dev/null | sed -n 's/^Model name: *//p' | head -1"),
        "mem_gib": sh("awk '/MemTotal/ { printf \"%.1f\", $2 / 1048576 }' /proc/meminfo"),
        "disk": sh(&format!("cat /sys/block/$(findmnt -no SOURCE -T '{}' | sed 's/\\[.*//; s#/dev/##; s/p[0-9]*$//')/device/model 2>/dev/null | xargs", dir.display())),
        "fs": sh(&format!("findmnt -no FSTYPE -T '{}'", dir.display())),
        "kernel": format!("Linux {}", sh("uname -r")),
        "load_kind": "Linux's one-minute load average",
    })
}

#[cfg(windows)]
fn host(dir: &Path) -> serde_json::Value {
    let drive = dir.to_string_lossy().chars().next().unwrap_or('C');
    serde_json::json!({
        "host": powershell("$env:COMPUTERNAME"),
        "cpu": powershell("(Get-CimInstance Win32_Processor).Name"),
        "mem_gib": powershell("'{0:N1}' -f ((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB)"),
        "disk": powershell(&format!("(Get-Partition -DriveLetter {drive} | Get-Disk).FriendlyName")),
        "fs": powershell(&format!("(Get-Volume -DriveLetter {drive}).FileSystemType")),
        "kernel": powershell("$v = Get-CimInstance Win32_OperatingSystem; \"$($v.Caption) $($v.Version)\""),
        "load_kind": "Windows: busy cores plus the processor queue, averaged over 3 s (it keeps no load average)",
    })
}

/// The time now, UTC, to the second: `YYYY-MM-DDTHH:MM:SSZ` (Hinnant's civil-from-days).
fn utc_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3600, rest % 3600 / 60, rest % 60)
}

#[cfg(not(windows))]
fn sh(cmd: &str) -> String {
    std::process::Command::new("sh").args(["-c", cmd]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_default()
}

fn open(o: &Opts, target: &str) -> anyhow::Result<Box<dyn Target>> {
    let dir = o.dir.join(target);
    Ok(match target {
        "amphora" => Box::new(Amphora::open(&dir, o.cache_kib)?),
        "sqlite" => Box::new(Sqlite::open(&dir, o.cache_kib)?),
        "duckdb" => Box::new(Duck::open(&dir, Some(1))?),
        // A database of its own: DuckDB locks its file to one database a process.
        "duckdb-mt" => Box::new(Duck::open(&dir, None)?),
        "postgres" => {
            // A database of the benchmark's own on the server, made the first time.
            let name = format!("{}_{}", o.bench, o.sf.to_string().replace('.', "_"));
            let mut server = postgres::Client::connect(&o.dsn, postgres::NoTls)?;
            if server.query_opt("SELECT 1 FROM pg_database WHERE datname = $1", &[&name])?.is_none() {
                server.batch_execute(&format!("CREATE DATABASE {name}"))?;
            }
            Box::new(Postgres::open(&format!("{} dbname={name}", o.dsn))?)
        }
        t => anyhow::bail!("--planner runs amphora, sqlite, duckdb, duckdb-mt or postgres, not {t}"),
    })
}

fn outcome_json(outcome: &Outcome, load: Option<f64>) -> serde_json::Value {
    match outcome {
        Outcome::Done(r) => serde_json::json!({ "status": "done", "rows": r.rows, "first": r.first, "prepare_ms": r.prepare_ms, "exec_ms": r.exec_ms, "ms": r.prepare_ms + r.exec_ms, "load": load }),
        Outcome::TimedOut => serde_json::json!({ "status": "timed_out", "load": load }),
        Outcome::Failed(e) => serde_json::json!({ "status": "failed", "error": e, "load": load }),
    }
}

fn counts_json(counts: &[Count]) -> serde_json::Value {
    counts
        .iter()
        .map(|c| serde_json::json!({ "plan": c.plan, "kind": c.kind, "table": c.table, "position": c.position, "est_input": c.est_input, "est_output": c.est_output, "rows": c.rows }))
        .collect()
}

pub fn main() -> anyhow::Result<()> {
    let o = opts()?;
    let b = bench(&o)?;
    let mut out = std::fs::OpenOptions::new().create(true).append(true).open(&o.out)?;
    let mut write = |v: serde_json::Value| -> anyhow::Result<()> {
        writeln!(out, "{v}")?;
        out.flush()?;
        Ok(())
    };
    let mut targets: Vec<Box<dyn Target>> = Vec::new();
    for t in &o.targets {
        let mut target = open(&o, t)?;
        // A load that ended (its indexes built and its statistics taken) leaves `bench_loaded`; a
        // database without it is loaded again from nothing.
        if !target.loaded("bench_loaded")? {
            for (table, _) in &b.tables {
                target.exec(&format!("DROP TABLE IF EXISTS {table}"))?;
            }
            let dialect = target.dialect();
            target.exec(&for_dialect(&b.schema, dialect))?;
            for (table, path) in &b.tables {
                let began = Instant::now();
                let rows = target.load(table, path, b.format)?;
                write(serde_json::json!({ "record": "load", "target": target.name(), "table": table, "rows": rows, "s": began.elapsed().as_secs_f64() }))?;
            }
            let began = Instant::now();
            target.exec(&b.indexes)?;
            target.analyze()?;
            target.exec("CREATE TABLE bench_loaded (done INTEGER)")?;
            target.exec("INSERT INTO bench_loaded VALUES (1)")?;
            write(serde_json::json!({ "record": "load", "target": target.name(), "table": "indexes and statistics", "s": began.elapsed().as_secs_f64() }))?;
        }
        targets.push(target);
    }
    let bench_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let git = |args: &[&str]| {
        std::process::Command::new("git").arg("-C").arg(bench_dir).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_default()
    };
    let mut engines = Vec::new();
    for t in &mut targets {
        engines.push(t.about()?);
    }
    write(serde_json::json!({
        "record": "run",
        "date": utc_now(),
        "host": host(&o.dir),
        "cores": std::thread::available_parallelism().map(usize::from).unwrap_or(0),
        "commit": git(&["rev-parse", "HEAD"]),
        "dirty_files": git(&["status", "--porcelain"]).lines().count(),
        "args": std::env::args().skip(1).collect::<Vec<_>>().join(" "),
        "bench": o.bench, "sf": if o.bench == "tpch" { Some(o.sf) } else { None },
        "source_revision": b.source_revision,
        "queries": b.queries.len(),
        "runs": o.runs, "timeout_s": o.timeout.as_secs(), "count_timeout_s": o.count_timeout.as_secs(),
        "max_load": o.max_load,
        "engines": engines,
    }))?;
    for (name, sql) in &b.queries {
        if !o.only.is_empty() && !o.only.contains(name) {
            continue;
        }
        for target in &mut targets {
            let sql = for_dialect(sql.trim().trim_end_matches(';'), target.dialect());
            let mut record = serde_json::json!({ "record": "query", "bench": o.bench, "query": name, "target": target.name() });
            if let Some(amphora) = target.amphora() {
                record["plan"] = match amphora.plan_json(&sql) {
                    Ok(plan) => serde_json::from_str(&plan).unwrap_or(serde_json::Value::String(plan)),
                    Err(e) => serde_json::json!({ "error": e.to_string() }),
                };
                let load = quiet(o.max_load, o.load_wait);
                let (outcome, counts) = amphora.counted(&sql, o.count_timeout);
                record["counted"] = outcome_json(&outcome, load);
                record["counts"] = counts_json(&counts);
            }
            let mut runs = Vec::new();
            for _ in 0..o.runs {
                let load = quiet(o.max_load, o.load_wait);
                let outcome = target.run(&sql, o.timeout);
                let stop = !matches!(outcome, Outcome::Done(_));
                runs.push(outcome_json(&outcome, load));
                // A query that timed out or failed once does so again: one is its record.
                if stop {
                    break;
                }
            }
            record["runs"] = serde_json::Value::Array(runs);
            eprintln!("{name} {}: {}", target.name(), record["runs"].as_array().and_then(|r| r.iter().filter_map(|r| r["ms"].as_f64()).reduce(f64::min)).map_or_else(|| "no time".to_owned(), |ms| format!("{ms:.1} ms")));
            write(record)?;
        }
    }
    Ok(())
}
