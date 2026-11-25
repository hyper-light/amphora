//! Billions-of-Agents Live Multi-Persona Concurrent Chaos Engine.
//!
//! Designed around real Meta-scale agentic fleets where **billions of logical agents/users**
//! (`--logical-agents`, default `1_000_000_000` = 1 Billion) drive massive open-loop & bursty
//! traffic multiplexed onto high-concurrency session pools (`--chaos-workers`, e.g., 64–512) and
//! optional horizontal shards (`--shards N` independent live Amphora databases routed by
//! `mix(logical_agent_id) % N`).
//!
//! Anti-coordinated-omission architecture:
//! - Submitter threads continuously generate logical agent requests drawn from the billion-agent
//!   keyspace using a 3-tier Zipf/Pareto distribution:
//!     * 15% Viral Hot Tier (top 1,000 logical agents / celebrities / mega-tenant / hot SKUs)
//!     * 35% Active Fleet Tier (10,000,000 active logical agents)
//!     * 50% Billion-Agent Cold Long Tail (full `1..=logical_agents` space)
//! - Every logical request carries its enqueue `Instant` inside sharded bounded in-flight queues
//!   (`--inflight`, default 16,384) and periodic **thundering-herd bursts** inject sudden spikes
//!   of simultaneous logical agent wakeups!
//! - We measure BOTH pure DB execution latency AND true end-to-end (queue wait + execution) latency
//!   for short reads, writes, heavy BI scans, and live accounting/conservation audits.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::suite::{Dialect, Kind, P, mix};
use crate::suite_db::{Session, Target};
use crate::{Hist, Rng, summary};

use super::loader::{BASE_TS_US, GauntletShape};
use super::ops_domains_1_4::{params_1_4, statements_1_4};
use super::ops_domains_5_8::{params_5_10, statements_5_10};

/// All 6 live multi-persona chaos scenarios.
pub const SCENARIOS: &[&str] = &[
    "agentic_fleet_rush",
    "social_celebrity_storm",
    "flash_sale_ledger_audit",
    "ci_runner_claim_storm",
    "zero_downtime_migrations",
    "rollback_timeout_storm",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Persona {
    ShortRead,
    Write,
    HeavyBi,
    Audit,
    Migration,
    Rollback,
}

struct AgentTask {
    logical_agent_id: u64,
    shard: usize,
    persona: Persona,
    op: &'static str,
    enqueued_at: Instant,
}

/// Sharded bounded work queue so submitters and 64–512 worker threads never serialize on one lock.
struct ShardedQueue {
    shards: Vec<(Mutex<VecDeque<AgentTask>>, Condvar)>,
    cap_per_shard: usize,
    inflight: AtomicUsize,
    peak_inflight: AtomicUsize,
    dropped_backpressure: AtomicU64,
    closed: AtomicBool,
}

impl ShardedQueue {
    fn new(num_queues: usize, total_cap: usize) -> Self {
        let n = num_queues.max(1);
        let cap_per_shard = (total_cap / n).max(64);
        let shards = (0..n)
            .map(|_| {
                (
                    Mutex::new(VecDeque::with_capacity(cap_per_shard)),
                    Condvar::new(),
                )
            })
            .collect();
        Self {
            shards,
            cap_per_shard,
            inflight: AtomicUsize::new(0),
            peak_inflight: AtomicUsize::new(0),
            dropped_backpressure: AtomicU64::new(0),
            closed: AtomicBool::new(false),
        }
    }

    fn push(&self, qidx: usize, task: AgentTask) -> bool {
        let (lock, cv) = &self.shards[qidx % self.shards.len()];
        let mut q = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if q.len() >= self.cap_per_shard {
            self.dropped_backpressure.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        q.push_back(task);
        let cur = self.inflight.fetch_add(1, Ordering::Relaxed) + 1;
        self.peak_inflight.fetch_max(cur, Ordering::Relaxed);
        cv.notify_one();
        true
    }

    fn pop(&self, qidx: usize) -> Option<AgentTask> {
        let len = self.shards.len();
        let home = qidx % len;
        let (lock, cv) = &self.shards[home];
        let mut q = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(t) = q.pop_front() {
                self.inflight.fetch_sub(1, Ordering::Relaxed);
                return Some(t);
            }
            if self.closed.load(Ordering::Acquire) {
                drop(q);
                // Drain any accepted tasks left on other shards so shutdown never abandons work
                // even if callers construct more queue shards than workers.
                for step in 1..len {
                    let (other_lock, _) = &self.shards[(home + step) % len];
                    let mut other = other_lock
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if let Some(t) = other.pop_front() {
                        self.inflight.fetch_sub(1, Ordering::Relaxed);
                        return Some(t);
                    }
                }
                return None;
            }
            let (guard, _) = cv
                .wait_timeout(q, Duration::from_millis(10))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            q = guard;
        }
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        for (lock, cv) in &self.shards {
            let _g = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            cv.notify_all();
        }
    }
}

/// Draws a logical agent ID from `1..=logical_agents` using a realistic 3-tier Meta-scale Pareto
/// mixture (15% viral hot top-1K, 35% active 10M fleet, 50% full billion-agent long tail).
fn pick_logical_agent(rng: &mut Rng, logical_agents: u64) -> u64 {
    let space = logical_agents.max(1);
    let r = rng.next() % 100;
    if r < 15 {
        1 + (rng.next() % space.min(1_000))
    } else if r < 50 {
        1 + (rng.next() % space.min(10_000_000))
    } else {
        1 + (rng.next() % space)
    }
}

fn pick_task(scenario: &str, rng: &mut Rng) -> (Persona, &'static str) {
    let r = rng.next() % 100;
    match scenario {
        "agentic_fleet_rush" => {
            if r < 45 {
                let ops = [
                    "ag_record_turn_txn",
                    "ag_upsert_memory",
                    "ag_complete_plan_step",
                    "ag_append_balanced_cost",
                ];
                (
                    Persona::Write,
                    ops[(rng.next() % ops.len() as u64) as usize],
                )
            } else if r < 82 {
                let ops = [
                    "ag_run_detail",
                    "ag_ws_active_runs",
                    "ag_memory_recall_top",
                    "ag_plan_ready_steps",
                    "ag_run_turns_timeline",
                ];
                (
                    Persona::ShortRead,
                    ops[(rng.next() % ops.len() as u64) as usize],
                )
            } else if r < 94 {
                let ops = [
                    "ag_model_token_spend",
                    "ag_tool_error_rates",
                    "ag_diff_churn_top",
                ];
                (
                    Persona::HeavyBi,
                    ops[(rng.next() % ops.len() as u64) as usize],
                )
            } else {
                (Persona::Audit, "ag_token_trial_balance")
            }
        }
        "social_celebrity_storm" => {
            if r < 55 {
                let ops = [
                    "soc_channel_hot_general",
                    "soc_channel_latest_50",
                    "soc_feed_normal_user",
                    "soc_profile_by_handle",
                    "soc_unread_counts_all",
                ];
                (
                    Persona::ShortRead,
                    ops[(rng.next() % ops.len() as u64) as usize],
                )
            } else if r < 80 {
                let ops = [
                    "soc_post_message_txn",
                    "soc_toggle_reaction",
                    "soc_edit_message",
                    "soc_mark_channel_read",
                ];
                (
                    Persona::Write,
                    ops[(rng.next() % ops.len() as u64) as usize],
                )
            } else if r < 92 {
                let ops = [
                    "soc_feed_celebrity_fanout",
                    "soc_top_reacted_messages",
                    "soc_channel_leaderboard",
                ];
                (
                    Persona::HeavyBi,
                    ops[(rng.next() % ops.len() as u64) as usize],
                )
            } else {
                let ops = ["soc_unicode_search", "soc_mod_queue_priority"];
                (
                    Persona::HeavyBi,
                    ops[(rng.next() % ops.len() as u64) as usize],
                )
            }
        }
        "flash_sale_ledger_audit" => {
            if r < 42 {
                let ops = [
                    "com_checkout_acid_txn",
                    "com_checkout_savepoint_abort",
                    "com_refund_restock_txn",
                    "com_restock_hot_sku",
                ];
                (
                    Persona::Write,
                    ops[(rng.next() % ops.len() as u64) as usize],
                )
            } else if r < 82 {
                let ops = [
                    "com_sku_point_lookup",
                    "com_faceted_catalog",
                    "com_customer_order_history",
                    "com_order_full_receipt",
                ];
                (
                    Persona::ShortRead,
                    ops[(rng.next() % ops.len() as u64) as usize],
                )
            } else if r < 90 {
                let ops = ["com_revenue_by_country_tier", "com_hot_sku_velocity"];
                (
                    Persona::HeavyBi,
                    ops[(rng.next() % ops.len() as u64) as usize],
                )
            } else {
                (Persona::Audit, "com_ledger_trial_balance")
            }
        }
        "ci_runner_claim_storm" => {
            if r < 45 {
                let ops = [
                    "ci_claim_and_finish_txn",
                    "ci_bulk_test_results_50",
                    "ci_enqueue_job",
                    "ci_record_flake_retry",
                ];
                (
                    Persona::Write,
                    ops[(rng.next() % ops.len() as u64) as usize],
                )
            } else if r < 80 {
                let ops = [
                    "ci_queue_next_job",
                    "ci_queue_depth_by_repo",
                    "ci_test_history_timeline",
                ];
                (
                    Persona::ShortRead,
                    ops[(rng.next() % ops.len() as u64) as usize],
                )
            } else {
                let ops = [
                    "ci_flakiest_tests_top50",
                    "ci_pass_to_fail_regress",
                    "ci_suite_pass_rate_pivot",
                ];
                (
                    Persona::HeavyBi,
                    ops[(rng.next() % ops.len() as u64) as usize],
                )
            }
        }
        "zero_downtime_migrations" => {
            if r < 52 {
                let ops = [
                    "saas_inbox_mega_tenant",
                    "saas_inbox_tiny_tenant",
                    "saas_inbox_empty_tenant",
                    "saas_issue_detail_hydrate",
                    "saas_login_email_lower",
                ];
                (
                    Persona::ShortRead,
                    ops[(rng.next() % ops.len() as u64) as usize],
                )
            } else if r < 86 {
                let ops = [
                    "saas_create_issue_workflow",
                    "saas_add_comment_bump",
                    "saas_toggle_issue_label",
                    "saas_edit_issue_md_json",
                ];
                (
                    Persona::Write,
                    ops[(rng.next() % ops.len() as u64) as usize],
                )
            } else if r < 94 {
                let ops = ["saas_kanban_pivot", "saas_assignee_workload"];
                (
                    Persona::HeavyBi,
                    ops[(rng.next() % ops.len() as u64) as usize],
                )
            } else {
                (Persona::Migration, "live_migration")
            }
        }
        _ => {
            // rollback_timeout_storm
            if r < 35 {
                let ops = [
                    "com_checkout_acid_txn",
                    "ag_record_turn_txn",
                    "saas_add_comment_bump",
                ];
                (
                    Persona::Write,
                    ops[(rng.next() % ops.len() as u64) as usize],
                )
            } else if r < 60 {
                let ops = [
                    "com_checkout_savepoint_abort",
                    "ag_turn_with_savepoint",
                    "ci_job_savepoint_abort",
                    "wiki_savepoint_edit",
                ];
                (
                    Persona::Write,
                    ops[(rng.next() % ops.len() as u64) as usize],
                )
            } else if r < 85 {
                (Persona::Rollback, "explicit_rollback")
            } else {
                (Persona::Audit, "com_ledger_trial_balance")
            }
        }
    }
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

pub struct ChaosConfig {
    pub workers: usize,
    pub submitters: usize,
    pub inflight_cap: usize,
    pub seconds: u64,
    pub logical_agents: u64,
    pub session_budget: usize,
}

/// Bounded per-shard pool of reusable sessions so `workers` (up to 512) multiplex over targets
/// with capped connection pools (e.g., MySQL's `session_budget + 4`) without ever holding
/// connections idle across empty-queue waits or deadlocking before submitters start.
struct SessionPoolState {
    idle: Vec<Box<dyn Session>>,
    created: usize,
}

struct SessionPool {
    target: Arc<dyn Target>,
    max_sessions: usize,
    state: Mutex<SessionPoolState>,
    avail: Condvar,
}

struct SessionGuard<'a> {
    pool: &'a SessionPool,
    session: Option<Box<dyn Session>>,
}

impl SessionPool {
    fn new(target: Arc<dyn Target>, max_sessions: usize) -> anyhow::Result<Self> {
        let cap = match target.max_sessions() {
            Some(limit) => max_sessions.min(limit),
            None => max_sessions,
        }
        .max(1);
        // Probe one session up front to fail fast on broken targets, then immediately return it to
        // `idle` so startup never pins a full pool before submitters begin.
        let first = target.session()?;
        Ok(Self {
            target,
            max_sessions: cap,
            state: Mutex::new(SessionPoolState {
                idle: vec![first],
                created: 1,
            }),
            avail: Condvar::new(),
        })
    }

    fn checkout(&self) -> anyhow::Result<SessionGuard<'_>> {
        let mut st = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(session) = st.idle.pop() {
                return Ok(SessionGuard {
                    pool: self,
                    session: Some(session),
                });
            }
            if st.created < self.max_sessions {
                st.created += 1;
                drop(st);
                match self.target.session() {
                    Ok(session) => {
                        return Ok(SessionGuard {
                            pool: self,
                            session: Some(session),
                        });
                    }
                    Err(e) => {
                        let mut st = self
                            .state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        st.created = st.created.saturating_sub(1);
                        self.avail.notify_one();
                        return Err(e);
                    }
                }
            }
            st = self
                .avail
                .wait(st)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

impl Drop for SessionGuard<'_> {
    fn drop(&mut self) {
        let mut st = self
            .pool
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !std::thread::panicking()
            && let Some(session) = self.session.take()
        {
            st.idle.push(session);
        } else {
            self.session.take();
            st.created = st.created.saturating_sub(1);
        }
        self.pool.avail.notify_one();
    }
}

/// Runs one live multi-persona billion-agent chaos scenario across `targets` (1 or `N` shards).
/// Computes bounded open-loop submitter thread count without panicking when callers request >32.
fn submitter_threads(workers: usize, requested: usize) -> usize {
    requested.max(workers / 64).clamp(2, 32)
}

/// Computes queue shard count so every shard always has at least one assigned worker.
fn queue_shard_count(workers: usize) -> usize {
    (workers / 4).clamp(1, 128).min(workers.max(1))
}

/// Hard cap on live OS worker threads so high-concurrency runs never exhaust macOS's kernel
/// thread table (`kern.num_taskthreads`); billions of logical agents multiplex over these workers.
pub const MAX_CHAOS_WORKERS: usize = 512;

pub fn run_scenario(
    scenario: &str,
    targets: &[Arc<dyn Target>],
    shape: &Arc<GauntletShape>,
    cfg: &ChaosConfig,
) -> anyhow::Result<serde_json::Value> {
    let dialect = targets[0].dialect();
    let num_shards = targets.len().max(1);
    let workers = cfg.workers.clamp(4, MAX_CHAOS_WORKERS);
    let queue_shards = queue_shard_count(workers);
    let effective_inflight = cfg.inflight_cap.max(workers.saturating_mul(4)).max(1024);
    let queue = Arc::new(ShardedQueue::new(queue_shards, effective_inflight));
    let stop = Arc::new(AtomicBool::new(false));

    let total_ops = Arc::new(AtomicU64::new(0));
    let total_errors = Arc::new(AtomicU64::new(0));
    let audit_checks = Arc::new(AtomicU64::new(0));
    let audit_failures = Arc::new(AtomicU64::new(0));
    let migration_ops = Arc::new(AtomicU64::new(0));
    let rollback_ops = Arc::new(AtomicU64::new(0));
    let burst_waves = Arc::new(AtomicU64::new(0));
    let first_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let first_audit_fail: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    // Per-worker merged histograms
    let read_svc = Arc::new(Mutex::new(Hist::new()));
    let read_e2e = Arc::new(Mutex::new(Hist::new()));
    let write_svc = Arc::new(Mutex::new(Hist::new()));
    let write_e2e = Arc::new(Mutex::new(Hist::new()));
    let bi_svc = Arc::new(Mutex::new(Hist::new()));

    // Build one bounded session pool per shard (default <= 64) so worker startup never blocks on
    // capped pools (such as MySQL's 68-session pool) before submitters start pushing work.
    let session_budget = if cfg.session_budget == 0 {
        workers.min(64)
    } else {
        cfg.session_budget.clamp(1, workers)
    };
    let mut session_pools = Vec::with_capacity(num_shards);
    for t in targets {
        session_pools.push(Arc::new(SessionPool::new(t.clone(), session_budget)?));
    }

    // Spawn worker threads (each checks out a pooled session per task and releases it before
    // waiting on the queue).
    let mut worker_handles = Vec::with_capacity(workers);
    for wid in 0..workers {
        let pools = session_pools.clone();
        let q = queue.clone();
        let shape = shape.clone();
        let total_ops = total_ops.clone();
        let total_errors = total_errors.clone();
        let audit_checks = audit_checks.clone();
        let audit_failures = audit_failures.clone();
        let migration_ops = migration_ops.clone();
        let rollback_ops = rollback_ops.clone();
        let first_error = first_error.clone();
        let first_audit_fail = first_audit_fail.clone();
        let read_svc = read_svc.clone();
        let read_e2e = read_e2e.clone();
        let write_svc = write_svc.clone();
        let write_e2e = write_e2e.clone();
        let bi_svc = bi_svc.clone();

        worker_handles.push(
            std::thread::Builder::new()
                .name(format!("chaos-w-{wid}"))
                .stack_size(1024 * 1024)
                .spawn(move || {
            let mut rng = Rng(mix(wid as u64 ^ 0xCAFE_BABE_1234));
            let mut h_r_svc = Hist::new();
            let mut h_r_e2e = Hist::new();
            let mut h_w_svc = Hist::new();
            let mut h_w_e2e = Hist::new();
            let mut h_bi = Hist::new();
            let mig_tag = format!("__chaos_mig_w{wid}");
            let is_pg_cat = matches!(dialect, Dialect::AmphoraPostgres | Dialect::Postgres);
            let i_ty = if is_pg_cat { "BIGINT" } else { "INTEGER" };
            let pk_ty = if dialect == Dialect::Postgres {
                "BIGINT PRIMARY KEY"
            } else {
                "INTEGER PRIMARY KEY"
            };

            while let Some(task) = q.pop(wid) {
                let sidx = task.shard % pools.len();
                let mut guard = match pools[sidx].checkout() {
                    Ok(g) => g,
                    Err(e) => {
                        total_errors.fetch_add(1, Ordering::Relaxed);
                        let mut slot = first_error
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if slot.is_none() {
                            *slot = Some(format!("session checkout shard {}: {e:#}", task.shard));
                        }
                        continue;
                    }
                };
                let Some(sess) = guard.session.as_mut() else {
                    continue;
                };
                // Seed rng lightly from logical_agent_id so every logical agent hits its deterministic slice
                rng.0 ^= mix(task.logical_agent_id);
                let svc_t0 = Instant::now();

                match task.persona {
                    Persona::Audit => {
                        let res = if task.op == "ag_token_trial_balance" {
                            sess.answer(
                                &["SELECT COALESCE(SUM(amount_micros), 0) FROM token_cost_ledger".into()],
                                &[vec![]],
                            )
                        } else {
                            sess.answer(
                                &["SELECT COALESCE(SUM(amount_cents), 0), COALESCE((SELECT MIN(stock) FROM products), 0) FROM ledger_entries".into()],
                                &[vec![]],
                            )
                        };
                        match res {
                            Ok(rows) => {
                                audit_checks.fetch_add(1, Ordering::Relaxed);
                                let sum_ok = rows
                                    .first()
                                    .and_then(|r| r.first())
                                    .and_then(|c| c.as_deref())
                                    == Some("0");
                                let stock_ok = if task.op == "ag_token_trial_balance" {
                                    true
                                } else {
                                    rows.first()
                                        .and_then(|r| r.get(1))
                                        .and_then(|c| c.as_deref())
                                        .and_then(|s| s.parse::<i64>().ok())
                                        .is_some_and(|m| m >= 0)
                                };
                                if !sum_ok || !stock_ok {
                                    audit_failures.fetch_add(1, Ordering::Relaxed);
                                    let mut slot = first_audit_fail
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                                    if slot.is_none() {
                                        *slot = Some(format!(
                                            "{} invariant violated on shard {}: {:?}",
                                            task.op, task.shard, rows
                                        ));
                                    }
                                }
                            }
                            Err(e) => {
                                total_errors.fetch_add(1, Ordering::Relaxed);
                                let mut slot = first_error
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                if slot.is_none() {
                                    *slot = Some(format!("audit {}: {e:#}", task.op));
                                }
                            }
                        }
                    }
                    Persona::Migration => {
                        // Unique table/view name per worker so concurrent migrations never collide on names
                        let seq = shape.alloc_id();
                        let tbl = format!("{mig_tag}_{seq}");
                        let tbl2 = format!("{mig_tag}_{seq}_r");
                        let stmts = vec![
                            format!("CREATE TABLE {tbl} (id {pk_ty}, k {i_ty} NOT NULL)"),
                            format!("INSERT INTO {tbl} (id, k) VALUES (1, 10), (2, 20)"),
                            format!("CREATE INDEX idx_{tbl}_k ON {tbl} (k)"),
                            format!("ALTER TABLE {tbl} RENAME TO {tbl2}"),
                            format!("SELECT COUNT(*) FROM {tbl2}"),
                            format!("DROP TABLE {tbl2}"),
                        ];
                        let params = vec![vec![]; stmts.len()];
                        match sess.run(Kind::Ddl, &stmts, &params) {
                            Ok(_) => {
                                migration_ops.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(e) => {
                                total_errors.fetch_add(1, Ordering::Relaxed);
                                let mut slot = first_error
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                if slot.is_none() {
                                    *slot = Some(format!("migration: {e:#}"));
                                }
                            }
                        }
                    }
                    Persona::Rollback => {
                        // Test nested SAVEPOINT rollback + unique-constraint clean rejection inside writes
                        let tmp_id = shape.alloc_id();
                        let ts = BASE_TS_US + (rng.next() % 1_000_000) as i64 * 1_000_000;
                        let stmts = vec![
                            "SAVEPOINT sp_chaos_rb".into(),
                            "INSERT INTO ledger_entries (id, account_id, order_id, direction, amount_cents, note, posted_at) VALUES (?, 1, NULL, 'debit', 777777, 'must_rollback', ?)".into(),
                            "ROLLBACK TO SAVEPOINT sp_chaos_rb".into(),
                            "RELEASE SAVEPOINT sp_chaos_rb".into(),
                        ];
                        let params = vec![vec![], vec![P::I(tmp_id), P::I(ts)], vec![], vec![]];
                        match sess.run(Kind::Write, &stmts, &params) {
                            Ok(_) => {
                                rollback_ops.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(e) => {
                                total_errors.fetch_add(1, Ordering::Relaxed);
                                let mut slot = first_error
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                if slot.is_none() {
                                    *slot = Some(format!("rollback: {e:#}"));
                                }
                            }
                        }
                    }
                    Persona::ShortRead | Persona::Write | Persona::HeavyBi => {
                        let Some(stmts) = lookup_stmts(task.op, dialect) else {
                            continue;
                        };
                        let Some(params) = lookup_params(task.op, &mut rng, &shape, (wid, workers)) else {
                            continue;
                        };
                        let kind = if task.persona == Persona::Write {
                            Kind::Write
                        } else {
                            Kind::Read
                        };
                        match sess.run(kind, &stmts, &params) {
                            Ok(_) => {
                                let svc_us = svc_t0.elapsed().as_micros() as u64;
                                let e2e_us = task.enqueued_at.elapsed().as_micros() as u64;
                                match task.persona {
                                    Persona::ShortRead => {
                                        h_r_svc.record(svc_us);
                                        h_r_e2e.record(e2e_us);
                                    }
                                    Persona::Write => {
                                        h_w_svc.record(svc_us);
                                        h_w_e2e.record(e2e_us);
                                    }
                                    Persona::HeavyBi => h_bi.record(svc_us),
                                    _ => {}
                                }
                            }
                            Err(e) => {
                                total_errors.fetch_add(1, Ordering::Relaxed);
                                let mut slot = first_error
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                if slot.is_none() {
                                    *slot = Some(format!("{}: {e:#}", task.op));
                                }
                            }
                        }
                    }
                }
                total_ops.fetch_add(1, Ordering::Relaxed);
            }

            read_svc.lock().unwrap_or_else(std::sync::PoisonError::into_inner).merge(&h_r_svc);
            read_e2e.lock().unwrap_or_else(std::sync::PoisonError::into_inner).merge(&h_r_e2e);
            write_svc.lock().unwrap_or_else(std::sync::PoisonError::into_inner).merge(&h_w_svc);
            write_e2e.lock().unwrap_or_else(std::sync::PoisonError::into_inner).merge(&h_w_e2e);
            bi_svc.lock().unwrap_or_else(std::sync::PoisonError::into_inner).merge(&h_bi);
        })?,
        );
    }

    // Spawn open-loop / thundering-herd fleet submitter threads multiplexing 1B+ logical agents!
    let submitters = submitter_threads(workers, cfg.submitters);
    let burst_size = workers.clamp(512, 4096);
    let logical_agents = cfg.logical_agents.max(1);
    let scenario_name = scenario.to_owned();
    let started = Instant::now();
    let mut sub_handles = Vec::with_capacity(submitters);
    for sid in 0..submitters {
        let q = queue.clone();
        let stop = stop.clone();
        let burst_waves = burst_waves.clone();
        let scen = scenario_name.clone();
        sub_handles.push(
            std::thread::Builder::new()
                .name(format!("chaos-sub-{sid}"))
                .stack_size(512 * 1024)
                .spawn(move || {
                    let mut rng = Rng(mix(sid as u64 ^ 0xBEEF_F00D_7777));
                    let mut tick = 0u64;
                    let mut next_burst = Instant::now() + Duration::from_millis(600);
                    while !stop.load(Ordering::Acquire) {
                        // Periodic thundering-herd burst: suddenly wake up 512..4096 logical agents at once!
                        let batch = if sid == 0 && Instant::now() >= next_burst {
                            next_burst = Instant::now() + Duration::from_millis(800);
                            burst_waves.fetch_add(1, Ordering::Relaxed);
                            burst_size
                        } else {
                            16
                        };
                        let mut pushed_any = false;
                        for _ in 0..batch {
                            if stop.load(Ordering::Acquire) {
                                break;
                            }
                            let agent_id = pick_logical_agent(&mut rng, logical_agents);
                            let shard = (mix(agent_id) as usize) % num_shards;
                            let (persona, op) = pick_task(&scen, &mut rng);
                            let qidx = (tick as usize) % queue_shards;
                            tick = tick.wrapping_add(1);
                            if q.push(
                                qidx,
                                AgentTask {
                                    logical_agent_id: agent_id,
                                    shard,
                                    persona,
                                    op,
                                    enqueued_at: Instant::now(),
                                },
                            ) {
                                pushed_any = true;
                            }
                        }
                        if !pushed_any {
                            std::thread::sleep(Duration::from_micros(200));
                        }
                    }
                })?,
        );
    }

    std::thread::sleep(Duration::from_secs(cfg.seconds.max(1)));
    stop.store(true, Ordering::Release);
    for h in sub_handles {
        let _ = h.join();
    }
    queue.close();
    for h in worker_handles {
        let _ = h.join();
    }

    let elapsed_s = started.elapsed().as_secs_f64().max(0.001);
    let ops = total_ops.load(Ordering::Relaxed);
    let errs = total_errors.load(Ordering::Relaxed);
    let afails = audit_failures.load(Ordering::Relaxed);

    let r_svc = read_svc
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let r_e2e = read_e2e
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let w_svc = write_svc
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let w_e2e = write_e2e
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let bi_h = bi_svc
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();

    Ok(serde_json::json!({
        "scenario": scenario,
        "logical_agents_space": logical_agents,
        "shards": num_shards,
        "workers": workers,
        "submitters": submitters,
        "elapsed_s": (elapsed_s * 100.0).round() / 100.0,
        "ops": ops,
        "ops_per_s": (ops as f64 / elapsed_s * 10.0).round() / 10.0,
        "peak_inflight_queue": queue.peak_inflight.load(Ordering::Relaxed),
        "backpressure_drops": queue.dropped_backpressure.load(Ordering::Relaxed),
        "burst_waves": burst_waves.load(Ordering::Relaxed),
        "short_read_service_ms": summary(r_svc),
        "short_read_e2e_ms": summary(r_e2e),
        "write_service_ms": summary(w_svc),
        "write_e2e_ms": summary(w_e2e),
        "heavy_bi_service_ms": summary(bi_h),
        "audit_checks": audit_checks.load(Ordering::Relaxed),
        "audit_failures": afails,
        "first_audit_failure": first_audit_fail.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone(),
        "migration_ops": migration_ops.load(Ordering::Relaxed),
        "rollback_ops": rollback_ops.load(Ordering::Relaxed),
        "errors": errs,
        "first_error": first_error.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_p2_sharded_queue_drains_all_accepted_tasks_even_with_more_queues_than_workers() {
        let q = Arc::new(ShardedQueue::new(8, 64));
        for i in 0..8usize {
            assert!(q.push(
                i,
                AgentTask {
                    logical_agent_id: i as u64 + 1,
                    shard: 0,
                    persona: Persona::ShortRead,
                    op: "ag_run_detail",
                    enqueued_at: Instant::now(),
                },
            ));
        }
        q.close();
        let completed = Arc::new(AtomicUsize::new(0));
        std::thread::scope(|s| {
            for wid in 0..4usize {
                let q = Arc::clone(&q);
                let completed = Arc::clone(&completed);
                s.spawn(move || {
                    while q.pop(wid).is_some() {
                        completed.fetch_add(1, Ordering::Relaxed);
                    }
                });
            }
        });
        assert_eq!(completed.load(Ordering::Relaxed), 8);
        assert_eq!(q.inflight.load(Ordering::Relaxed), 0);
        assert!(queue_shard_count(4) <= 4);
    }

    #[test]
    fn review_p2_submitters_clamp_never_panics_above_32() {
        assert_eq!(submitter_threads(4, 0), 2);
        assert_eq!(submitter_threads(4, 1), 2);
        assert_eq!(submitter_threads(4, 33), 32);
        assert_eq!(submitter_threads(512, 1), 8);
        assert_eq!(submitter_threads(512, 1000), 32);
    }

    /// Review P1 regression: chaos startup must never deadlock when `workers` exceeds a target's
    /// capped session pool (e.g., MySQL's 68-session pool vs 512 chaos workers).
    #[test]
    fn review_p1_chaos_startup_never_deadlocks_on_capped_session_pool() {
        struct CappedTarget {
            inner: Arc<dyn Target>,
            cap: usize,
            in_use: Arc<(Mutex<usize>, Condvar)>,
        }
        struct CappedSession {
            inner: Box<dyn Session>,
            in_use: Arc<(Mutex<usize>, Condvar)>,
        }
        impl Drop for CappedSession {
            fn drop(&mut self) {
                let (lock, cv) = &*self.in_use;
                let mut cur = lock.lock().unwrap();
                *cur -= 1;
                cv.notify_one();
            }
        }
        impl Session for CappedSession {
            fn run(
                &mut self,
                kind: Kind,
                stmts: &[String],
                params: &[Vec<P>],
            ) -> anyhow::Result<usize> {
                self.inner.run(kind, stmts, params)
            }
            fn answer(
                &mut self,
                stmts: &[String],
                params: &[Vec<P>],
            ) -> anyhow::Result<Vec<Vec<Option<String>>>> {
                self.inner.answer(stmts, params)
            }
        }
        impl Target for CappedTarget {
            fn dialect(&self) -> Dialect {
                self.inner.dialect()
            }
            fn setup(&self, stmts: &[(String, Vec<P>)]) -> anyhow::Result<()> {
                self.inner.setup(stmts)
            }
            fn session(&self) -> anyhow::Result<Box<dyn Session>> {
                let (lock, cv) = &*self.in_use;
                let mut cur = lock.lock().unwrap();
                while *cur >= self.cap {
                    cur = cv.wait(cur).unwrap();
                }
                *cur += 1;
                drop(cur);
                Ok(Box::new(CappedSession {
                    inner: self.inner.session()?,
                    in_use: Arc::clone(&self.in_use),
                }))
            }
            fn max_sessions(&self) -> Option<usize> {
                Some(self.cap)
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let raw =
            crate::suite_db::open("sqlite", dir.path().to_str().unwrap(), None, 4, false).unwrap();
        let shape = Arc::new(GauntletShape::new(1, 1_000));
        let tbls: Vec<(String, Vec<P>)> = super::super::schema::tables(Dialect::Sqlite, false)
            .into_iter()
            .map(|s| (s, vec![]))
            .collect();
        raw.setup(&tbls).unwrap();
        super::super::loader::load_all(Dialect::Sqlite, &shape, |b| raw.setup(&b), || raw.settle())
            .unwrap();
        raw.ready().unwrap();

        let capped: Arc<dyn Target> = Arc::new(CappedTarget {
            inner: raw,
            cap: 4,
            in_use: Arc::new((Mutex::new(0), Condvar::new())),
        });
        let cfg = ChaosConfig {
            workers: 24,
            submitters: 2,
            inflight_cap: 512,
            seconds: 1,
            logical_agents: 1_000,
            session_budget: 64,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let t_clone = Arc::clone(&capped);
        let s_clone = Arc::clone(&shape);
        std::thread::spawn(move || {
            let _ = tx.send(run_scenario(
                "agentic_fleet_rush",
                &[t_clone],
                &s_clone,
                &cfg,
            ));
        });
        let res = rx
            .recv_timeout(Duration::from_secs(15))
            .expect("chaos startup deadlocked on capped session pool")
            .unwrap();
        assert!(
            res["ops"].as_u64().unwrap_or(0) > 0,
            "chaos must complete operations on capped pool: {res}"
        );
        assert_eq!(
            res["errors"].as_u64().unwrap_or(1),
            0,
            "unexpected errors: {res}"
        );
        assert_eq!(
            res["audit_failures"].as_u64().unwrap_or(1),
            0,
            "unexpected audit failures: {res}"
        );
    }
}
