//! The Meta-Scale Agentic End-User Gauntlet (`--gauntlet`).
//!
//! 100% focused on real end users, real messy production schemas, and Meta-scale fleets:
//! - **43 interconnected tables** & **65+ indexes** across 8 real application domains.
//! - **Scale presets from empty/single up to hundreds of millions of rows** (`empty`, `single`,
//!   `smoke`, `millions`, `tens`, `hundreds`/`sweat`, `meta`, or any exact `--scale N`), plus
//!   horizontal sharding (`--shards S`) for multi-billion-row fleet tests!
//! - **360 named end-user operations** (reads, ACID writes, bulk ingests, savepoints, BI/ETL,
//!   views, triggers, temp tables, live DDL, `sqlite_schema` & `pg_catalog` OIDs).
//! - **1,024+ parametric end-user queries** sweeping tenant skew, filter facets, sorts, `NULLS
//!   FIRST/LAST`, pagination, windows, and set operations with deterministic answer digests.
//! - **5 automated anti-gaming oracles** (pagination conservation, spill-invariance `4000 vs 48`,
//!   index-invariance vs `NOT INDEXED`, accounting/stock/integrity conservation, cold reopen &
//!   `pg_class` OID durability).
//! - **6 live multi-persona billion-agent chaos workloads** (`--logical-agents 1_000_000_000+`,
//!   open-loop submitters, thundering-herd bursts, 64–512 concurrent workers, live auditors).

pub mod chaos;
pub mod loader;
pub mod ops_domains_1_4;
pub mod ops_domains_5_8;
pub mod oracles;
pub mod parametric;
pub mod schema;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::suite::{Dialect, Kind, P, mix};
use crate::suite_db::{self, Target};
use crate::{Hist, Rng, summary};

use self::chaos::{ChaosConfig, SCENARIOS, run_scenario};
use self::loader::{GauntletShape, load_all};
use self::ops_domains_1_4::{GauntletOp, OPS_1_4, params_1_4, statements_1_4};
use self::ops_domains_5_8::{OPS_5_10, params_5_10, statements_5_10};

struct Opts {
    target: String,
    dir: String,
    dsn: Option<String>,
    scale_label: String,
    scale: i64,
    shards: usize,
    logical_agents: u64,
    clients: usize,
    chaos_workers: usize,
    submitters: usize,
    inflight: usize,
    seconds: f64,
    warmup_ms: u64,
    chaos_seconds: u64,
    phases: String,
    only: Option<String>,
    answers: bool,
    reuse: bool,
}

fn parse_scale(v: &str) -> anyhow::Result<i64> {
    Ok(match v {
        "empty" => 0,
        "single" => 1,
        "smoke" | "ci" => 25_000,         // ~500K total rows
        "millions" | "dev" => 150_000,    // ~3M total rows
        "tens" | "prod" => 1_500_000,     // ~30M total rows
        "hundreds" | "sweat" => 6_000_000, // ~117M+ total rows
        "meta" => 25_000_000,             // ~500M rows per shard!
        other => other.parse::<i64>().map_err(|e| {
            anyhow::anyhow!(
                "invalid --scale {other}: {e} (use empty, single, smoke, millions, tens, hundreds, sweat, meta, or an integer)"
            )
        })?,
    })
}

fn parse_opts() -> anyhow::Result<Opts> {
    let default_dir = format!(
        "{}/.cache/amphora-gauntlet",
        std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
    );
    let mut o = Opts {
        target: "amphora".into(),
        dir: default_dir,
        dsn: None,
        scale_label: "millions".into(),
        scale: 150_000,
        shards: 1,
        logical_agents: 1_000_000_000, // 1 Billion logical agents by default!
        clients: 1,
        chaos_workers: 64,
        submitters: 8,
        inflight: 16_384,
        seconds: 0.25,
        warmup_ms: 50,
        chaos_seconds: 5,
        phases: "all".into(),
        only: None,
        answers: false,
        reuse: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--gauntlet" {
            continue;
        }
        let mut v = || {
            args.next()
                .ok_or_else(|| anyhow::anyhow!("{a} takes a value"))
        };
        match a.as_str() {
            "--target" => o.target = v()?,
            "--dir" => o.dir = v()?,
            "--dsn" => o.dsn = Some(v()?),
            "--scale" => {
                let raw = v()?;
                o.scale = parse_scale(&raw)?;
                o.scale_label = raw;
            }
            "--shards" => o.shards = v()?.parse::<usize>()?.max(1),
            "--logical-agents" => o.logical_agents = v()?.parse::<u64>()?.max(1),
            "--clients" => o.clients = v()?.parse::<usize>()?.max(1),
            "--chaos-workers" => o.chaos_workers = v()?.parse::<usize>()?.max(4),
            "--submitters" => o.submitters = v()?.parse::<usize>()?.max(1),
            "--inflight" => o.inflight = v()?.parse::<usize>()?.max(256),
            "--seconds" => o.seconds = v()?.parse()?,
            "--warmup-ms" => o.warmup_ms = v()?.parse()?,
            "--chaos-seconds" => o.chaos_seconds = v()?.parse()?,
            "--phases" => o.phases = v()?,
            "--only" => o.only = Some(v()?),
            "--answers" => o.answers = true,
            "--reuse" => o.reuse = true,
            other => anyhow::bail!("unknown --gauntlet option {other}"),
        }
    }
    Ok(o)
}

fn phase_enabled(phases: &str, name: &str) -> bool {
    phases == "all" || phases.split(',').any(|p| p.trim() == name)
}

fn all_named_ops() -> Vec<GauntletOp> {
    let mut v = Vec::with_capacity(OPS_1_4.len() + OPS_5_10.len());
    v.extend_from_slice(OPS_1_4);
    v.extend_from_slice(OPS_5_10);
    v
}

fn lookup_stmts(op: &str, d: Dialect) -> Option<Vec<String>> {
    statements_1_4(op, d).or_else(|| statements_5_10(op, d))
}

fn lookup_params(
    op: &str,
    rng: &mut Rng,
    s: &GauntletShape,
    c: (usize, usize),
) -> Option<Vec<Vec<P>>> {
    params_1_4(op, rng, s, c).or_else(|| params_5_10(op, rng, s, c))
}

fn load_shard(
    target: &Arc<dyn Target>,
    shape: &GauntletShape,
    cedar: bool,
    pg_schema: &str,
) -> anyhow::Result<(u64, f64, f64, f64)> {
    let d = target.dialect();
    let t0 = Instant::now();
    if d == Dialect::Postgres {
        target.setup(&[
            (format!("DROP SCHEMA IF EXISTS {pg_schema} CASCADE"), vec![]),
            (format!("CREATE SCHEMA {pg_schema}"), vec![]),
        ])?;
    }
    let tbls: Vec<(String, Vec<P>)> = schema::tables(d, cedar)
        .into_iter()
        .map(|s| (s, vec![]))
        .collect();
    target.setup(&tbls)?;

    let rows_loaded = load_all(d, shape, |batch| target.setup(&batch), || target.settle())?;
    let load_s = t0.elapsed().as_secs_f64();

    let t_idx = Instant::now();
    let mut create_idxs: Vec<(String, Vec<P>)> = Vec::new();
    let mut maintenance: Vec<String> = Vec::new();
    for stmt in schema::indexes(d) {
        let upper = stmt.trim_start().to_ascii_uppercase();
        if upper.starts_with("VACUUM") || upper.starts_with("ANALYZE") {
            maintenance.push(stmt);
        } else {
            create_idxs.push((stmt, vec![]));
        }
    }
    if !create_idxs.is_empty() {
        target.setup(&create_idxs)?;
    }
    for stmt in maintenance {
        target.setup(&[(stmt, vec![])])?;
    }
    let index_s = t_idx.elapsed().as_secs_f64();

    let t_set = Instant::now();
    target.settle()?;
    let settle_s = t_set.elapsed().as_secs_f64();

    Ok((rows_loaded, load_s, index_s, settle_s))
}

fn pg_schema_for_shard(shard: usize, shards: usize) -> String {
    if shards <= 1 {
        "amphora_gauntlet".to_owned()
    } else {
        format!("amphora_gauntlet_s{shard:03}")
    }
}

/// All single-column `id` primary-key tables whose runtime writes allocate IDs via
/// `GauntletShape::alloc_id()`.
const ID_ALLOC_TABLES: &[&str] = &[
    "workspaces",
    "agent_runs",
    "agent_turns",
    "tool_calls",
    "file_diffs",
    "agent_memories",
    "plan_steps",
    "token_cost_ledger",
    "accounts",
    "channels",
    "messages",
    "moderation_reports",
    "tenants",
    "teams",
    "saas_users",
    "projects",
    "issues",
    "labels",
    "issue_comments",
    "attachments",
    "categories",
    "products",
    "customers",
    "orders",
    "order_items",
    "payments",
    "fx_rates",
    "ledger_entries",
    "services",
    "spans",
    "experiments",
    "exp_events",
    "ci_jobs",
    "test_results",
    "bookings",
    "documents",
];

/// Seeds `shape.next_id` past every existing primary key across all shards on `--reuse` so
/// repeated write runs never collide on primary keys.
fn seed_reuse_allocator(targets: &[Arc<dyn Target>], shape: &GauntletShape) -> anyhow::Result<()> {
    let mut max_id = 0i64;
    for t in targets {
        let mut sess = t.session()?;
        for table in ID_ALLOC_TABLES {
            let sql = format!("SELECT COALESCE(MAX(id), 0) FROM {table}");
            if let Ok(rows) = sess.answer(&[sql], &[vec![]]) {
                let v = rows
                    .first()
                    .and_then(|r| r.first())
                    .and_then(|c| c.as_deref())
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(0);
                max_id = max_id.max(v);
            }
        }
    }
    if max_id > 0 {
        shape.advance_next_id_past(max_id);
    }
    Ok(())
}

fn aggregate_shard_oracles(
    reports: Vec<(usize, oracles::OracleReport)>,
) -> (bool, serde_json::Value) {
    if reports.len() == 1 {
        let (_, r) = reports.into_iter().next().unwrap();
        return (
            r.passed,
            serde_json::json!({ "passed": r.passed, "details": r.details }),
        );
    }
    let passed = reports.iter().all(|(_, r)| r.passed);
    let failed_shards: Vec<usize> = reports
        .iter()
        .filter(|(_, r)| !r.passed)
        .map(|(sh, _)| *sh)
        .collect();
    let per_shard: Vec<serde_json::Value> = reports
        .into_iter()
        .map(|(sh, r)| serde_json::json!({ "shard": sh, "passed": r.passed, "details": r.details }))
        .collect();
    (
        passed,
        serde_json::json!({
            "passed": passed,
            "details": {
                "shards_checked": per_shard.len(),
                "failed_shards": failed_shards,
                "shards": per_shard,
            }
        }),
    )
}

fn shard_dir(base: &str, shard: usize, shards: usize) -> String {
    if shards <= 1 {
        base.to_owned()
    } else {
        Path::new(base)
            .join(format!("shard-{shard:03}"))
            .to_string_lossy()
            .into_owned()
    }
}

pub fn main() -> anyhow::Result<()> {
    let o = parse_opts()?;
    let shape = Arc::new(GauntletShape::new(o.scale, o.logical_agents));
    // Cap pooled reader handles per shard at 64 (and live OS worker threads at 512) so hundreds of
    // concurrent client threads multiplex cleanly over Amphora's reader pool, wait queue,
    // group-commit writer, and memory budget without exhausting OS thread or FD limits.
    let chaos_workers = o.chaos_workers.min(chaos::MAX_CHAOS_WORKERS);
    if chaos_workers < o.chaos_workers {
        eprintln!(
            "[gauntlet] capping --chaos-workers from {} to {} (MAX_CHAOS_WORKERS)",
            o.chaos_workers, chaos_workers
        );
    }
    let session_budget = o
        .clients
        .max(if phase_enabled(&o.phases, "chaos") {
            chaos_workers
        } else {
            1
        })
        .min(64);

    eprintln!(
        "[gauntlet] target={} scale={} (~{} rows/shard x {} shards = ~{} total rows) logical_agents={} phases={}",
        o.target,
        o.scale_label,
        shape.total_rows_estimate(),
        o.shards,
        shape.total_rows_estimate().saturating_mul(o.shards as i64),
        o.logical_agents,
        o.phases
    );

    // Open 1 or N shards (parallel load when shards > 1, each with its own PostgreSQL schema).
    let mut targets: Vec<Arc<dyn Target>> = Vec::with_capacity(o.shards);
    let mut pg_schemas: Vec<String> = Vec::with_capacity(o.shards);
    for sh in 0..o.shards {
        let dir = shard_dir(&o.dir, sh, o.shards);
        let pg_schema = pg_schema_for_shard(sh, o.shards);
        let t = suite_db::open_gauntlet(
            &o.target,
            &dir,
            o.dsn.as_deref(),
            session_budget,
            o.reuse,
            &pg_schema,
        )?;
        targets.push(t);
        pg_schemas.push(pg_schema);
    }
    let dialect = targets[0].dialect();
    let cedar = o.target == "cedardb";

    let mut total_rows_loaded = 0u64;
    let mut load_s = 0.0f64;
    let mut index_s = 0.0f64;
    let mut settle_s = 0.0f64;

    if !o.reuse {
        let t_all = Instant::now();
        if o.shards == 1 {
            let (r, ls, is, ss) = load_shard(&targets[0], &shape, cedar, &pg_schemas[0])?;
            total_rows_loaded = r;
            load_s = ls;
            index_s = is;
            settle_s = ss;
        } else {
            let mut handles = Vec::with_capacity(o.shards);
            for (t, pg_schema) in targets.iter().zip(&pg_schemas) {
                let t = t.clone();
                let shape = shape.clone();
                let pg_schema = pg_schema.clone();
                handles.push(std::thread::spawn(move || {
                    load_shard(&t, &shape, cedar, &pg_schema)
                }));
            }
            for h in handles {
                let (r, ls, is, ss) = h
                    .join()
                    .map_err(|_| anyhow::anyhow!("shard loader panicked"))??;
                total_rows_loaded += r;
                load_s = load_s.max(ls);
                index_s = index_s.max(is);
                settle_s = settle_s.max(ss);
            }
            let _ = t_all;
        }
    }

    for t in &targets {
        t.ready()?;
    }
    if o.reuse {
        seed_reuse_allocator(&targets, &shape)?;
    }

    let mut report = serde_json::Map::new();
    report.insert("target".into(), serde_json::json!(o.target));
    report.insert("scale_label".into(), serde_json::json!(o.scale_label));
    report.insert("scale".into(), serde_json::json!(o.scale));
    report.insert("shards".into(), serde_json::json!(o.shards));
    report.insert("logical_agents".into(), serde_json::json!(o.logical_agents));
    report.insert("rows_loaded".into(), serde_json::json!(total_rows_loaded));
    report.insert(
        "load_s".into(),
        serde_json::json!((load_s * 1000.0).round() / 1000.0),
    );
    report.insert(
        "index_s".into(),
        serde_json::json!((index_s * 1000.0).round() / 1000.0),
    );
    report.insert(
        "settle_s".into(),
        serde_json::json!((settle_s * 1000.0).round() / 1000.0),
    );

    // =========================================================================
    // Execute ALL deterministic read phases BEFORE any writes/DDL/chaos mutate
    // the dataset, so cross-engine differential checks compare identical state:
    //   1a. Named Read ops (timing + deterministic answer digests)
    //   2.  1,024+ Parametric read queries (timing + deterministic digests)
    //   3.  Live Read Oracles (Pagination Conservation, Index-Invariance, Pre-mutation Integrity)
    //   1b. Named Write & DDL ops
    //   4.  6 Live Billions-of-Agents Chaos Workloads (+ Post-chaos Integrity)
    //   5.  Spill-Invariance (4000 vs 48 pages) & Cold-Reopen + pg_class OID Durability
    // =========================================================================
    let mut named_ops_out = serde_json::Map::new();
    let mut named_answers_out = serde_json::Map::new();
    let mut named_passed = 0usize;
    let mut named_unsupported = 0usize;
    let mut named_failed = 0usize;

    let run_named_subset = |only_reads: bool,
                            sess: &mut dyn crate::suite_db::Session,
                            ops_out: &mut serde_json::Map<String, serde_json::Value>,
                            answers_out: &mut serde_json::Map<String, serde_json::Value>,
                            passed: &mut usize,
                            unsupported: &mut usize,
                            failed: &mut usize| {
        let win = Duration::from_secs_f64(o.seconds.max(0.01));
        let warm = Duration::from_millis(o.warmup_ms);
        let mut ops = all_named_ops();
        ops.retain(|op| {
            if only_reads {
                op.kind == Kind::Read
            } else {
                op.kind != Kind::Read
            }
        });
        ops.sort_by_key(|op| match op.kind {
            Kind::Read => 0u8,
            Kind::Write => 1u8,
            Kind::Ddl => 2u8,
        });

        for op in &ops {
            if let Some(ref f) = o.only
                && !op.name.contains(f.as_str())
                && !op.domain.contains(f.as_str())
            {
                continue;
            }
            let Some(stmts) = lookup_stmts(op.name, dialect) else {
                *unsupported += 1;
                ops_out.insert(
                    op.name.into(),
                    serde_json::json!({ "domain": op.domain, "what": op.what, "status": "unsupported" }),
                );
                continue;
            };
            let seed = op
                .name
                .bytes()
                .fold(0x1234_5678u64, |a, b| a.wrapping_mul(31) ^ u64::from(b));
            let mut rng = Rng(mix(seed));

            if o.answers && op.kind == Kind::Read {
                let mut ans_rng = Rng(rng.0);
                if let Some(ps) = lookup_params(op.name, &mut ans_rng, &shape, (0, 1)) {
                    match sess.answer(&stmts, &ps) {
                        Ok(rows) => {
                            let digest = parametric::digest_answer(&rows);
                            answers_out.insert(
                                op.name.into(),
                                serde_json::json!({
                                    "rows": rows.len(),
                                    "hash": digest,
                                    "sample": rows.into_iter().take(5).collect::<Vec<_>>(),
                                }),
                            );
                        }
                        Err(e) => {
                            answers_out.insert(
                                op.name.into(),
                                serde_json::json!({ "error": format!("{e:#}") }),
                            );
                        }
                    }
                }
            }

            let warm_end = Instant::now() + warm;
            let mut first_err: Option<String> = None;
            let mut err_count = 0u64;
            if op.kind != Kind::Ddl {
                while Instant::now() < warm_end {
                    let Some(ps) = lookup_params(op.name, &mut rng, &shape, (0, 1)) else {
                        break;
                    };
                    if let Err(e) = sess.run(op.kind, &stmts, &ps) {
                        err_count += 1;
                        if first_err.is_none() {
                            first_err = Some(format!("{e:#}"));
                        }
                        break;
                    }
                }
            }

            let mut hist = Hist::new();
            let mut rows_sum = 0usize;
            let mut iters = 0u64;
            let t_start = Instant::now();
            if first_err.is_none() {
                let deadline = t_start + win;
                loop {
                    let Some(ps) = lookup_params(op.name, &mut rng, &shape, (0, 1)) else {
                        break;
                    };
                    let t0 = Instant::now();
                    match sess.run(op.kind, &stmts, &ps) {
                        Ok(r) => {
                            hist.record(t0.elapsed().as_micros() as u64);
                            rows_sum += r;
                            iters += 1;
                        }
                        Err(e) => {
                            err_count += 1;
                            if first_err.is_none() {
                                first_err = Some(format!("{e:#}"));
                            }
                            break;
                        }
                    }
                    if op.kind == Kind::Ddl || Instant::now() >= deadline {
                        break;
                    }
                }
            }
            let elapsed_s = t_start.elapsed().as_secs_f64().max(0.000_001);
            if err_count > 0 {
                *failed += 1;
            } else {
                *passed += 1;
            }
            ops_out.insert(
                op.name.into(),
                serde_json::json!({
                    "domain": op.domain,
                    "kind": match op.kind { Kind::Read => "read", Kind::Write => "write", Kind::Ddl => "ddl" },
                    "ops_per_s": (iters as f64 / elapsed_s * 10.0).round() / 10.0,
                    "iters": iters,
                    "rows": rows_sum,
                    "lat_ms": summary(hist),
                    "errors": err_count,
                    "first_error": first_err,
                }),
            );
        }
    };

    // Phase 1a: Named Read ops (before any mutation)
    if phase_enabled(&o.phases, "named") {
        let mut sess = targets[0].session()?;
        run_named_subset(
            true,
            sess.as_mut(),
            &mut named_ops_out,
            &mut named_answers_out,
            &mut named_passed,
            &mut named_unsupported,
            &mut named_failed,
        );
    }

    let mut parametric_failed = 0usize;
    let mut oracle_failed = 0usize;
    let mut chaos_failed = 0usize;

    // Phase 2: 1,024+ Parametric End-User Queries (before any mutation!)
    if phase_enabled(&o.phases, "parametric") {
        let mut sess = targets[0].session()?;
        let p_out = parametric::run_parametric(
            sess.as_mut(),
            dialect,
            &shape,
            o.only.as_deref(),
            o.answers,
        );
        parametric_failed = p_out.failed;
        eprintln!(
            "[gauntlet parametric] total={} passed={} failed={} total_ms={:.1} max_ms={:.2} ({})",
            p_out.total, p_out.passed, p_out.failed, p_out.total_ms, p_out.max_ms, p_out.slowest_id
        );
        report.insert(
            "parametric".into(),
            serde_json::json!({
                "total": p_out.total,
                "passed": p_out.passed,
                "failed": p_out.failed,
                "total_ms": (p_out.total_ms * 10.0).round() / 10.0,
                "max_ms": (p_out.max_ms * 100.0).round() / 100.0,
                "slowest_id": p_out.slowest_id,
                "first_error": p_out.first_error,
            }),
        );
        if o.answers {
            report.insert(
                "parametric_digests".into(),
                serde_json::Value::Object(p_out.digests),
            );
        }
    }

    // Phase 3: Live Read-Only Oracles across ALL shards (Pagination Conservation,
    // Index-Invariance, Conservation & Integrity)
    let mut oracles_map = serde_json::Map::new();
    if phase_enabled(&o.phases, "oracles") {
        let mut o1_all = Vec::with_capacity(o.shards);
        let mut o3_all = Vec::with_capacity(o.shards);
        let mut o4_all = Vec::with_capacity(o.shards);
        for (sh, target) in targets.iter().enumerate() {
            let mut sess = target.session()?;
            o1_all.push((
                sh,
                oracles::oracle_pagination_conservation(sess.as_mut(), &shape),
            ));
            o3_all.push((
                sh,
                oracles::oracle_index_invariance(sess.as_mut(), dialect, &shape),
            ));
            o4_all.push((
                sh,
                oracles::oracle_conservation_and_integrity(sess.as_mut(), dialect, &shape),
            ));
        }
        let (o1_ok, o1_val) = aggregate_shard_oracles(o1_all);
        let (o3_ok, o3_val) = aggregate_shard_oracles(o3_all);
        let (o4_ok, o4_val) = aggregate_shard_oracles(o4_all);
        eprintln!(
            "[gauntlet oracles live] shards={} pagination={} index_inv={} conservation={}",
            o.shards, o1_ok, o3_ok, o4_ok
        );
        oracles_map.insert("pagination_conservation".into(), o1_val);
        oracles_map.insert("index_invariance".into(), o3_val);
        oracles_map.insert("conservation_integrity".into(), o4_val);
    }

    // Phase 1b: Named Write & DDL ops
    if phase_enabled(&o.phases, "named") {
        let mut sess = targets[0].session()?;
        run_named_subset(
            false,
            sess.as_mut(),
            &mut named_ops_out,
            &mut named_answers_out,
            &mut named_passed,
            &mut named_unsupported,
            &mut named_failed,
        );
        eprintln!(
            "[gauntlet named] passed={named_passed} failed={named_failed} unsupported={named_unsupported}"
        );
        report.insert(
            "named".into(),
            serde_json::json!({
                "passed": named_passed,
                "failed": named_failed,
                "unsupported": named_unsupported,
                "ops": named_ops_out,
            }),
        );
        if o.answers {
            report.insert(
                "named_answers".into(),
                serde_json::Value::Object(named_answers_out),
            );
        }
    }

    // =========================================================================
    // Phase 4: 6 Live Billions-of-Agents Multi-Persona Concurrent Chaos Workloads
    // =========================================================================
    if phase_enabled(&o.phases, "chaos") {
        let cfg = ChaosConfig {
            workers: chaos_workers,
            submitters: o.submitters,
            inflight_cap: o.inflight,
            seconds: o.chaos_seconds,
            logical_agents: shape.logical_agents,
            session_budget,
        };
        let mut chaos_out = Vec::new();
        for &scen in SCENARIOS {
            if let Some(ref f) = o.only
                && !scen.contains(f.as_str())
            {
                continue;
            }
            eprintln!(
                "[gauntlet chaos] starting {scen} (logical_agents={}, shards={}, workers={}, {}s)",
                cfg.logical_agents, o.shards, cfg.workers, cfg.seconds
            );
            let res = run_scenario(scen, &targets, &shape, &cfg)?;
            eprintln!(
                "[gauntlet chaos] finished {scen}: ops={} ({} ops/s) audits={} audit_fails={} errs={}",
                res["ops"],
                res["ops_per_s"],
                res["audit_checks"],
                res["audit_failures"],
                res["errors"]
            );
            chaos_out.push(res);
        }
        // Post-chaos conservation check across ALL shards!
        let mut post_all = Vec::with_capacity(o.shards);
        for (sh, target) in targets.iter().enumerate() {
            let mut sess = target.session()?;
            post_all.push((
                sh,
                oracles::oracle_conservation_and_integrity(sess.as_mut(), dialect, &shape),
            ));
        }
        let (_, post_val) = aggregate_shard_oracles(post_all);
        oracles_map.insert("post_chaos_conservation".into(), post_val);
        report.insert("chaos".into(), serde_json::Value::Array(chaos_out));
    }

    report.insert(
        "target_stats".into(),
        serde_json::Value::Array(targets.iter().map(|t| t.stats()).collect()),
    );

    // Capture pre-close durable snapshots and teardown weak-reference probes on ALL shards BEFORE
    // closing targets so the cold-reopen durability oracle verifies both genuine disk recovery
    // (old engine instance dropped) and exact row counts, sums, and digests on every shard.
    let pre_close_snapshots: Vec<oracles::PreCloseSnapshot> = if phase_enabled(&o.phases, "oracles")
    {
        let mut snaps = Vec::with_capacity(o.shards);
        for target in &targets {
            let mut sess = target.session()?;
            snaps.push(oracles::capture_pre_close_snapshot(sess.as_mut())?);
        }
        snaps
    } else {
        Vec::new()
    };
    let drop_probes: Vec<Option<Box<dyn Fn() -> bool + Send + Sync>>> =
        targets.iter().map(|t| t.instance_dropped_probe()).collect();

    // Drop live targets so background threads join and every engine instance releases cleanly
    // before cold-reopen & spill-invariance oracles!
    drop(targets);

    // =========================================================================
    // Phase 5: File-Level Oracles across ALL shards (Spill-Invariance & Cold Reopen/OIDs)
    // =========================================================================
    if phase_enabled(&o.phases, "oracles") {
        let mut o2_all = Vec::with_capacity(o.shards);
        let mut o5_all = Vec::with_capacity(o.shards);
        for sh in 0..o.shards {
            let sh_dir = shard_dir(&o.dir, sh, o.shards);
            let instance_dropped = drop_probes.get(sh).and_then(|p| p.as_ref()).map(|p| p());
            let (o2, mut o5) = oracles::oracle_spill_and_cold_reopen(
                &o.target,
                &sh_dir,
                &shape,
                pre_close_snapshots.get(sh),
            );
            if instance_dropped == Some(false) {
                o5.passed = false;
                if let Some(arr) = o5
                    .details
                    .get_mut("failures")
                    .and_then(|v| v.as_array_mut())
                {
                    arr.push(serde_json::json!(format!(
                        "shard {sh}: live engine instance remained alive after Target drop; reopen was not cold"
                    )));
                }
            } else if let Some(dropped) = instance_dropped
                && let Some(meta) = o5.details.get_mut("meta").and_then(|v| v.as_object_mut())
            {
                meta.insert(
                    "previous_instance_dropped".into(),
                    serde_json::json!(dropped),
                );
            }
            o2_all.push((sh, o2));
            o5_all.push((sh, o5));
        }
        let (o2_ok, o2_val) = aggregate_shard_oracles(o2_all);
        let (o5_ok, o5_val) = aggregate_shard_oracles(o5_all);
        eprintln!(
            "[gauntlet oracles reopen] shards={} spill_inv={} cold_reopen={}",
            o.shards, o2_ok, o5_ok
        );
        oracles_map.insert("spill_invariance".into(), o2_val);
        oracles_map.insert("cold_reopen_oid".into(), o5_val);
    }

    if !oracles_map.is_empty() {
        oracle_failed = oracles_map
            .values()
            .filter(|v| !v.get("passed").and_then(|p| p.as_bool()).unwrap_or(false))
            .count();
        report.insert("oracles".into(), serde_json::Value::Object(oracles_map));
    }
    if let Some(chaos_arr) = report.get("chaos").and_then(|v| v.as_array()) {
        chaos_failed = chaos_arr
            .iter()
            .filter(|sc| {
                sc.get("errors").and_then(|v| v.as_u64()).unwrap_or(0) > 0
                    || sc
                        .get("audit_failures")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0)
                        > 0
            })
            .count();
    }

    println!("{}", serde_json::Value::Object(report));
    if named_failed > 0 || parametric_failed > 0 || oracle_failed > 0 || chaos_failed > 0 {
        anyhow::bail!(
            "gauntlet failed: named_failed={named_failed} parametric_failed={parametric_failed} oracle_failed={oracle_failed} chaos_failed={chaos_failed}"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::pg_schema_for_shard;

    #[test]
    fn review_p1_pg_schemas_are_isolated_per_shard() {
        assert_eq!(pg_schema_for_shard(0, 1), "amphora_gauntlet");
        assert_eq!(pg_schema_for_shard(0, 2), "amphora_gauntlet_s000");
        assert_eq!(pg_schema_for_shard(1, 2), "amphora_gauntlet_s001");
        assert_ne!(pg_schema_for_shard(0, 2), pg_schema_for_shard(1, 2));
    }
}
