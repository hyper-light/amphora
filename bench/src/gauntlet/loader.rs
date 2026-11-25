//! Deterministic, realistic, skewed multi-domain data generator and streaming loader for the
//! Meta-Scale Agentic End-User Gauntlet (`--gauntlet`).
//!
//! Key end-user realism properties:
//! - **Extreme power-law skew**: Tenant 1 holds ~55% of all SaaS issues/comments/attachments;
//!   the last tenant is **completely empty** (catches empty-tenant dashboard bugs). Accounts 1..10
//!   are celebrities with massive follower fan-in; normal users have a handful of followers.
//! - **Real messy data**: Unicode (CJK, emoji, Cyrillic, accented Latin), mixed-case emails,
//!   NULLable FKs/dates/estimates, heavy priority/status ties, and multi-KB overflow payloads
//!   (4–24 KiB file diffs, 1–12 KiB markdown descriptions, up to 32 KiB binary attachments).
//! - **Exact accounting & graph invariants**: every `ledger_entries` & `token_cost_ledger` pair
//!   sums to 0; every hierarchy (`projects`, `categories`, `spans`, `tool_calls`, `plan_edges`) is
//!   a valid DAG/tree; every foreign key points to a real parent row.

use std::sync::atomic::{AtomicI64, Ordering};

use crate::WORDS;
use crate::suite::{Dialect, P, mix};

use super::schema::json_ph;

pub const BASE_TS_US: i64 = 1_735_689_600_000_000; // 2025-01-01T00:00:00Z in microseconds

const UNICODE_NAMES: &[&str] = &[
    "Ada Lovelace",
    "田中 太郎",
    "María García-López",
    "Алексей Петров",
    "김민수",
    "Fatima Al-Mansoor",
    "Søren Kierkegaard",
    "Chloé Moreau 🚀",
    "Priya Nair",
    "Kwame Mensah",
    "Δημήτρης Παπαδόπουλος",
    "Wei-Lin Chen ⚡",
];

const MODELS: &[&str] = &[
    "gemini-2.5-pro",
    "claude-sonnet-4",
    "gpt-5-codex",
    "llama-4-maverick",
    "amphora-local-32b",
];

const TOOLS: &[&str] = &[
    "read_file",
    "edit_file",
    "run_command",
    "grep_search",
    "list_dir",
    "web_search",
    "run_tests",
    "git_diff",
    "sql_query",
    "deploy_canary",
];

const ISSUE_STATUSES: &[&str] = &["open", "in_progress", "in_review", "blocked", "closed"];
const ORDER_STATUSES: &[&str] = &["pending", "paid", "shipped", "delivered", "refunded"];
const CURRENCIES: &[&str] = &["USD", "EUR", "GBP", "JPY", "CAD"];
const COUNTRIES: &[&str] = &["US", "DE", "JP", "GB", "CA", "FR", "BR", "IN", "AU", "KR"];
const TIERS: &[&str] = &["free", "pro", "team", "enterprise"];
const EMOJIS: &[&str] = &["👍", "🔥", "🚀", "👀", "❤️", "🎉", "🤔", "✅"];
const OUTCOMES: &[&str] = &[
    "pass", "pass", "pass", "pass", "pass", "pass", "pass", "fail", "flake", "skip",
];

/// Exact row counts across all 43 tables derived from one core scale factor `n`.
///
/// Total row count across all 43 tables is approximately **19.5 × `scale`**:
/// - `scale = 0` (`empty`): 0 rows (boundary bug check)
/// - `scale = 1` (`single`): ~45 rows (1-row boundary bug check)
/// - `scale = 25_000` (`ci`): ~500K rows (~300 MiB)
/// - `scale = 100_000` (`millions` / `dev`): ~2M rows (~1.2 GiB)
/// - `scale = 1_000_000` (`tens` / `prod`): ~20M rows (~12 GiB)
/// - `scale = 6_000_000` (`sweat` / `hundreds`): ~115M rows (~70 GiB)
/// - `scale = 25_000_000` (`meta`): ~500M rows per shard (multi-billion across `--shards`)
pub struct GauntletShape {
    pub scale: i64,
    /// Logical agent/user population simulated by chaos & routing workloads (up to billions!).
    pub logical_agents: u64,
    // Domain 1: Agentic
    pub workspaces: i64,
    pub agent_runs: i64,
    pub agent_turns: i64,
    pub tool_calls: i64,
    pub file_diffs: i64,
    pub agent_memories: i64,
    pub plan_steps: i64,
    pub token_ledger_pairs: i64,
    // Domain 2: Social
    pub accounts: i64,
    pub celebrities: i64,
    pub channels: i64,
    pub messages: i64,
    pub mod_reports: i64,
    // Domain 3: SaaS
    pub tenants: i64,
    pub empty_tenant: i64,
    pub teams: i64,
    pub saas_users: i64,
    pub projects: i64,
    pub issues: i64,
    pub labels: i64,
    pub issue_comments: i64,
    pub attachments: i64,
    // Domain 4: E-Commerce & Ledger
    pub categories: i64,
    pub products: i64,
    pub customers: i64,
    pub orders: i64,
    pub order_items: i64,
    pub fx_rates: i64,
    pub ledger_accounts: i64,
    // Domain 5: Observability & Experiments
    pub services: i64,
    pub traces: i64,
    pub spans: i64,
    pub experiments: i64,
    pub exp_events: i64,
    // Domain 6: CI/CD
    pub ci_jobs: i64,
    pub test_results: i64,
    // Domain 7 & 8: Bookings & Wiki
    pub resources: i64,
    pub bookings: i64,
    pub documents: i64,
    // Monotonic ID allocator for runtime writes
    pub next_id: AtomicI64,
}

impl GauntletShape {
    pub fn new(scale: i64, logical_agents: u64) -> Self {
        if scale <= 0 {
            return Self {
                scale: 0,
                logical_agents: logical_agents.max(1),
                workspaces: 0,
                agent_runs: 0,
                agent_turns: 0,
                tool_calls: 0,
                file_diffs: 0,
                agent_memories: 0,
                plan_steps: 0,
                token_ledger_pairs: 0,
                accounts: 0,
                celebrities: 0,
                channels: 0,
                messages: 0,
                mod_reports: 0,
                tenants: 0,
                empty_tenant: 1,
                teams: 0,
                saas_users: 0,
                projects: 0,
                issues: 0,
                labels: 0,
                issue_comments: 0,
                attachments: 0,
                categories: 0,
                products: 0,
                customers: 0,
                orders: 0,
                order_items: 0,
                fx_rates: 0,
                ledger_accounts: 100,
                services: 0,
                traces: 0,
                spans: 0,
                experiments: 0,
                exp_events: 0,
                ci_jobs: 0,
                test_results: 0,
                resources: 0,
                bookings: 0,
                documents: 0,
                next_id: AtomicI64::new(100_000_000),
            };
        }
        if scale == 1 {
            return Self {
                scale: 1,
                logical_agents: logical_agents.max(1),
                workspaces: 1,
                agent_runs: 1,
                agent_turns: 1,
                tool_calls: 1,
                file_diffs: 1,
                agent_memories: 1,
                plan_steps: 2,
                token_ledger_pairs: 1,
                accounts: 2,
                celebrities: 1,
                channels: 1,
                messages: 1,
                mod_reports: 1,
                tenants: 2,
                empty_tenant: 2,
                teams: 1,
                saas_users: 1,
                projects: 1,
                issues: 1,
                labels: 1,
                issue_comments: 1,
                attachments: 1,
                categories: 1,
                products: 1,
                customers: 1,
                orders: 1,
                order_items: 1,
                fx_rates: 5,
                ledger_accounts: 10,
                services: 1,
                traces: 1,
                spans: 1,
                experiments: 1,
                exp_events: 2,
                ci_jobs: 1,
                test_results: 1,
                resources: 1,
                bookings: 1,
                documents: 2,
                next_id: AtomicI64::new(100_000_000),
            };
        }

        // Scale parent dimensions smoothly up to tens/hundreds of millions without artificial tiny caps.
        let tenants = (scale / 250).clamp(12, 200_000);
        let accounts = (scale / 4).clamp(100, 10_000_000);
        let agent_runs = (scale / 5).max(50);
        let orders = (scale / 2).max(100);
        let traces = (scale / 6).max(50);
        let ci_jobs = (scale / 10).max(50);
        Self {
            scale,
            logical_agents: logical_agents.max(1),
            workspaces: (scale / 100).clamp(20, 1_000_000),
            agent_runs,
            agent_turns: scale,
            tool_calls: scale.saturating_mul(2),
            file_diffs: (scale / 4).max(20),
            agent_memories: (scale / 5).max(50),
            plan_steps: (scale / 2).max(50),
            token_ledger_pairs: agent_runs,
            accounts,
            celebrities: 10.min(accounts),
            channels: (scale / 50).clamp(20, 500_000),
            messages: scale,
            mod_reports: (scale / 20).max(20),
            tenants,
            empty_tenant: tenants,
            teams: (tenants * 3).max(10),
            saas_users: (scale / 10).clamp(50, 5_000_000),
            projects: (tenants * 4).clamp(20, 500_000),
            issues: scale,
            labels: (tenants * 6).clamp(12, 500_000),
            issue_comments: scale,
            attachments: (scale / 20).clamp(10, 500_000),
            categories: 64,
            products: (scale / 10).clamp(100, 2_000_000),
            customers: (scale / 5).clamp(100, 10_000_000),
            orders,
            order_items: orders.saturating_mul(2),
            fx_rates: 500,
            ledger_accounts: 200,
            services: 40,
            traces,
            spans: scale,
            experiments: 20,
            exp_events: scale,
            ci_jobs,
            test_results: scale.saturating_mul(2),
            resources: (scale / 100).clamp(20, 100_000),
            bookings: (scale / 2).max(50),
            documents: (scale / 10).clamp(20, 2_000_000),
            next_id: AtomicI64::new(scale.saturating_mul(20).max(100_000_000)),
        }
    }

    pub fn total_rows_estimate(&self) -> i64 {
        self.workspaces
            + self.agent_runs
            + self.agent_turns
            + self.tool_calls
            + self.file_diffs
            + self.agent_memories
            + self.plan_steps * 2
            + self.token_ledger_pairs * 2
            + self.accounts
            + self.accounts * 3
            + self.channels
            + self.channels * 4
            + self.messages
            + self.messages / 2
            + self.mod_reports
            + self.tenants
            + self.teams
            + self.saas_users
            + self.projects
            + self.issues
            + self.labels
            + self.issues * 2
            + self.issue_comments
            + self.attachments
            + self.categories
            + self.products
            + self.customers
            + self.orders
            + self.order_items
            + self.orders
            + self.fx_rates
            + self.orders * 2
            + self.services
            + self.spans
            + self.experiments
            + self.exp_events
            + self.ci_jobs
            + self.test_results
            + self.bookings
            + self.documents
            + self.documents * 2
    }

    /// Skewed tenant picker: ~55% tenant 1 (mega-enterprise), ~30% tenants 2..=5, ~15% rest,
    /// NEVER `empty_tenant` (which stays genuinely empty!).
    pub fn skewed_tenant(&self, h: u64) -> i64 {
        let active = (self.tenants - 1).max(1);
        if active == 1 {
            return 1;
        }
        let r = h % 100;
        if r < 55 {
            1
        } else if r < 85 && active >= 5 {
            2 + (h / 100 % 4) as i64
        } else {
            1 + (h / 100 % active as u64) as i64
        }
    }

    pub fn alloc_id(&self) -> i64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Advances the monotonic ID allocator past `max_id` (used on `--reuse` so repeated runs never
    /// collide with primary keys written by earlier runs).
    pub fn advance_next_id_past(&self, max_id: i64) {
        let floor = max_id.saturating_add(1);
        self.next_id.fetch_max(floor, Ordering::AcqRel);
    }
}

fn word(h: u64) -> &'static str {
    WORDS[(h % WORDS.len() as u64) as usize]
}

fn make_text(seed: u64, min_bytes: usize, rare_tag: Option<&str>) -> String {
    let mut out = String::with_capacity(min_bytes + 32);
    if let Some(tag) = rare_tag {
        out.push_str(tag);
        out.push(' ');
    }
    let mut h = seed;
    while out.len() < min_bytes {
        h = mix(h);
        out.push_str(word(h));
        out.push(' ');
    }
    out
}

fn make_blob(seed: u64, bytes: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes);
    let mut h = seed;
    while out.len() < bytes {
        h = mix(h);
        let chunk = h.to_le_bytes();
        let take = (bytes - out.len()).min(8);
        out.extend_from_slice(&chunk[..take]);
    }
    out
}

/// Helper that emits multi-row INSERT statements in batches of `batch_rows`, reusing each batch's
/// SQL string (so statement caches stay tiny even at 500M rows!), flushing transactions every
/// `tx_stmts` statements, calling `on_checkpoint` every `checkpoint_every` rows so WAL never
/// balloons during 10M–500M row loads, and printing periodic progress to stderr.
struct BatchEmitter<'a, F, C> {
    tx: Vec<(String, Vec<P>)>,
    tx_stmts: usize,
    rows_total: u64,
    rows_since_ckpt: u64,
    rows_since_log: u64,
    checkpoint_every: u64,
    started: std::time::Instant,
    est_total: u64,
    each: &'a mut F,
    on_checkpoint: &'a mut C,
}

impl<'a, F, C> BatchEmitter<'a, F, C>
where
    F: FnMut(Vec<(String, Vec<P>)>) -> anyhow::Result<()>,
    C: FnMut() -> anyhow::Result<()>,
{
    fn new(
        tx_stmts: usize,
        checkpoint_every: u64,
        est_total: u64,
        each: &'a mut F,
        on_checkpoint: &'a mut C,
    ) -> Self {
        Self {
            tx: Vec::with_capacity(tx_stmts),
            tx_stmts,
            rows_total: 0,
            rows_since_ckpt: 0,
            rows_since_log: 0,
            checkpoint_every,
            started: std::time::Instant::now(),
            est_total,
            each,
            on_checkpoint,
        }
    }

    fn flush_tx(&mut self) -> anyhow::Result<()> {
        if !self.tx.is_empty() {
            let batch = std::mem::take(&mut self.tx);
            (self.each)(batch)?;
        }
        if self.checkpoint_every > 0 && self.rows_since_ckpt >= self.checkpoint_every {
            self.rows_since_ckpt = 0;
            (self.on_checkpoint)()?;
        }
        if self.est_total >= 250_000 && self.rows_since_log >= 250_000 {
            self.rows_since_log = 0;
            let secs = self.started.elapsed().as_secs_f64().max(0.001);
            let rate = self.rows_total as f64 / secs;
            let pct = (self.rows_total as f64 / self.est_total.max(1) as f64 * 100.0).min(100.0);
            eprintln!(
                "[gauntlet load] {:>11} / ~{} rows ({pct:5.1}%) | {rate:>8.0} rows/s | {secs:.1}s",
                self.rows_total, self.est_total
            );
        }
        Ok(())
    }

    fn emit_rows(
        &mut self,
        prefix: &str,
        row_tpl: &str,
        count: i64,
        batch_rows: usize,
        mut row_fn: impl FnMut(i64, &mut Vec<P>),
    ) -> anyhow::Result<()> {
        if count <= 0 {
            return Ok(());
        }
        let full_sql = format!("{prefix} VALUES {}", vec![row_tpl; batch_rows].join(", "));
        let mut id = 1i64;
        while id <= count {
            let last = (id + batch_rows as i64 - 1).min(count);
            let n = (last - id + 1) as usize;
            let mut params = Vec::with_capacity(n * 10);
            for cur in id..=last {
                row_fn(cur, &mut params);
            }
            let sql = if n == batch_rows {
                full_sql.clone()
            } else {
                format!("{prefix} VALUES {}", vec![row_tpl; n].join(", "))
            };
            self.tx.push((sql, params));
            self.rows_total += n as u64;
            self.rows_since_ckpt += n as u64;
            self.rows_since_log += n as u64;
            if self.tx.len() >= self.tx_stmts {
                self.flush_tx()?;
            }
            id = last + 1;
        }
        Ok(())
    }

    fn emit_iter<T>(
        &mut self,
        prefix: &str,
        row_tpl: &str,
        batch_rows: usize,
        items: impl IntoIterator<Item = T>,
        mut row_fn: impl FnMut(T, &mut Vec<P>),
    ) -> anyhow::Result<()> {
        let full_sql = format!("{prefix} VALUES {}", vec![row_tpl; batch_rows].join(", "));
        let mut params = Vec::with_capacity(batch_rows * 6);
        let mut n = 0usize;
        for item in items {
            row_fn(item, &mut params);
            n += 1;
            if n >= batch_rows {
                self.tx
                    .push((full_sql.clone(), std::mem::take(&mut params)));
                self.rows_total += n as u64;
                self.rows_since_ckpt += n as u64;
                self.rows_since_log += n as u64;
                n = 0;
                if self.tx.len() >= self.tx_stmts {
                    self.flush_tx()?;
                }
            }
        }
        if n > 0 {
            let sql = if n == batch_rows {
                full_sql
            } else {
                format!("{prefix} VALUES {}", vec![row_tpl; n].join(", "))
            };
            self.tx.push((sql, params));
            self.rows_total += n as u64;
            self.rows_since_ckpt += n as u64;
            self.rows_since_log += n as u64;
            if self.tx.len() >= self.tx_stmts {
                self.flush_tx()?;
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> anyhow::Result<()> {
        if !self.tx.is_empty() {
            let batch = std::mem::take(&mut self.tx);
            (self.each)(batch)?;
        }
        Ok(())
    }
}

/// Streams all 43 tables' deterministic production-realistic rows into `each`, returning the exact
/// total row count inserted. Calls `on_checkpoint` periodically during multi-million-row loads so
/// WAL files remain bounded.
pub fn load_all(
    d: Dialect,
    shape: &GauntletShape,
    mut each: impl FnMut(Vec<(String, Vec<P>)>) -> anyhow::Result<()>,
    mut on_checkpoint: impl FnMut() -> anyhow::Result<()>,
) -> anyhow::Result<u64> {
    if shape.scale <= 0 {
        return Ok(0);
    }
    let jp = json_ph(d);
    let est = shape.total_rows_estimate().max(0) as u64;
    // Checkpoint every ~500K rows when loading millions so WAL stays bounded on disk.
    let ckpt_every = if est >= 1_000_000 { 500_000 } else { 0 };
    let mut em = BatchEmitter::new(60, ckpt_every, est, &mut each, &mut on_checkpoint);

    // =========================================================================
    // Domain 3 parents first (tenants, teams, saas_users, projects, labels)
    // =========================================================================
    let tenant_tpl = format!("(?, ?, ?, ?, ?, {jp}, ?)");
    em.emit_rows(
        "INSERT INTO tenants (id, slug, name, plan, seat_limit, settings_json, created_at)",
        &tenant_tpl,
        shape.tenants,
        100,
        |id, p| {
            let h = mix(id as u64 ^ 0x1001);
            let plan = if id == 1 {
                "enterprise"
            } else {
                TIERS[(h % TIERS.len() as u64) as usize]
            };
            let sso = id == 1 || (h & 3 == 0);
            let region = COUNTRIES[(h % COUNTRIES.len() as u64) as usize];
            p.extend([
                P::I(id),
                P::T(format!("org-{id:05}")),
                P::T(format!(
                    "Org {id} {}",
                    UNICODE_NAMES[(id as usize) % UNICODE_NAMES.len()]
                )),
                P::T(plan.into()),
                P::I(if id == 1 {
                    50_000
                } else {
                    10 + (h % 500) as i64
                }),
                P::T(format!(
                    r#"{{"sso":{sso},"region":"{region}","tier":"{plan}","retention_days":{}}}"#,
                    30 + (h % 335)
                )),
                P::I(BASE_TS_US - id * 86_400_000_000),
            ]);
        },
    )?;

    em.emit_rows(
        "INSERT INTO saas_users (id, tenant_id, email, display_name, role, active, last_seen_at)",
        "(?, ?, ?, ?, ?, ?, ?)",
        shape.saas_users,
        100,
        |id, p| {
            let h = mix(id as u64 ^ 0x1002);
            let tid = shape.skewed_tenant(h);
            let roles = ["member", "member", "member", "admin", "viewer", "bot"];
            let mixed_case = if id % 3 == 0 {
                format!("User.{id}@Org{tid}.Example.COM")
            } else {
                format!("user{id}@org{tid}.example.com")
            };
            p.extend([
                P::I(id),
                P::I(tid),
                P::T(mixed_case),
                P::T(format!(
                    "{} #{id}",
                    UNICODE_NAMES[(h % UNICODE_NAMES.len() as u64) as usize]
                )),
                P::T(roles[(h % roles.len() as u64) as usize].into()),
                P::Z(h % 20 != 0),
                P::I(BASE_TS_US + (h % 2_500_000) as i64 * 1_000_000),
            ]);
        },
    )?;

    em.emit_rows(
        "INSERT INTO teams (id, tenant_id, name, lead_user_id)",
        "(?, ?, ?, ?)",
        shape.teams,
        100,
        |id, p| {
            let h = mix(id as u64 ^ 0x1003);
            let tid = shape.skewed_tenant(h);
            let lead = if h % 10 == 0 || shape.saas_users == 0 {
                P::N
            } else {
                P::I(1 + (h % shape.saas_users as u64) as i64)
            };
            p.extend([
                P::I(id),
                P::I(tid),
                P::T(format!("team-{}-{id}", word(h))),
                lead,
            ]);
        },
    )?;

    em.emit_rows(
        "INSERT INTO projects (id, tenant_id, parent_project_id, key_code, name, archived, budget_cents)",
        "(?, ?, ?, ?, ?, ?, ?)",
        shape.projects,
        100,
        |id, p| {
            let h = mix(id as u64 ^ 0x1004);
            let tid = shape.skewed_tenant(h);
            // Valid parent tree (parent_id strictly < id -> guaranteed acyclic, depth up to ~5).
            let parent = if id <= 4 || h % 3 == 0 {
                P::N
            } else {
                P::I(1 + (h % (id - 1) as u64) as i64)
            };
            p.extend([
                P::I(id),
                P::I(tid),
                parent,
                P::T(format!("PRJ{id}")),
                P::T(format!("Project {} {id}", word(h))),
                P::Z(h % 15 == 0),
                P::I(((h % 500) as i64 + 1) * 100_000),
            ]);
        },
    )?;

    em.emit_rows(
        "INSERT INTO labels (id, tenant_id, name, color)",
        "(?, ?, ?, ?)",
        shape.labels,
        100,
        |id, p| {
            let h = mix(id as u64 ^ 0x1005);
            let tid = shape.skewed_tenant(h);
            let names = [
                "bug", "urgent", "feature", "perf", "security", "wontfix", "docs", "infra",
            ];
            let colors = [
                "#d73a4a", "#0075ca", "#cfd3d7", "#a2eeef", "#7057ff", "#e4e669",
            ];
            p.extend([
                P::I(id),
                P::I(tid),
                P::T(format!(
                    "{}-{}",
                    names[(id as usize) % names.len()],
                    id % 12
                )),
                P::T(colors[(h % colors.len() as u64) as usize].into()),
            ]);
        },
    )?;

    // =========================================================================
    // Domain 3 high-volume: issues, issue_labels, issue_comments, attachments
    // =========================================================================
    let issue_tpl = format!("(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, {jp}, ?, ?, ?, ?, ?)");
    em.emit_rows(
        "INSERT INTO issues (id, tenant_id, project_id, number, author_id, assignee_id, status, priority, title, description, metadata_json, estimate_hrs, due_at, created_at, updated_at, deleted_at)",
        &issue_tpl,
        shape.issues,
        50,
        |id, p| {
            let h = mix(id as u64 ^ 0x1006);
            let tid = shape.skewed_tenant(h);
            let proj = 1 + (h % shape.projects.max(1) as u64) as i64;
            let author = 1 + (mix(h ^ 1) % shape.saas_users.max(1) as u64) as i64;
            // ~30% unassigned (NULL assignee)
            let assignee = if h % 10 < 3 {
                P::N
            } else {
                P::I(1 + (mix(h ^ 2) % shape.saas_users.max(1) as u64) as i64)
            };
            // Heavy ties on status & priority (0..=4)
            let status = ISSUE_STATUSES[(h % ISSUE_STATUSES.len() as u64) as usize];
            let priority = (mix(h ^ 3) % 5) as i64;
            let created = BASE_TS_US + id * 60_000_000;
            let updated = created + (h % 500_000) as i64 * 1_000_000;
            let due = if h % 3 == 0 {
                P::N
            } else {
                P::I(created + ((h % 30) as i64 - 10) * 86_400_000_000)
            };
            let deleted = if h % 25 == 0 { P::I(updated + 1_000_000) } else { P::N };
            let est = if h % 4 == 0 {
                P::N
            } else {
                P::F(((h % 40) as f64) * 0.5)
            };
            // Mix of normal (180B), medium (1.5 KiB), and overflow (10 KiB) markdown descriptions!
            let desc_len = if id % 200 == 0 {
                10_240
            } else if id % 25 == 0 {
                1_536
            } else {
                180
            };
            let rare = if id % 500 == 0 { Some("sev0-outage-zephyr") } else { None };
            let sev = ["sev1", "sev2", "sev3", "sev4"][(h % 4) as usize];
            p.extend([
                P::I(id),
                P::I(tid),
                P::I(proj),
                P::I(id),
                P::I(author),
                assignee,
                P::T(status.into()),
                P::I(priority),
                P::T(format!("[{}] {} issue #{id}", word(h), word(mix(h ^ 7)))),
                P::T(make_text(h ^ 0xabc, desc_len, rare)),
                P::T(format!(r#"{{"severity":"{sev}","component":"{}","votes":{}}}"#, word(mix(h ^ 9)), h % 100)),
                est,
                due,
                P::I(created),
                P::I(updated),
                deleted,
            ]);
        },
    )?;

    // Up to 2 distinct labels per issue (composite PK (issue_id, label_id))
    let label_pairs = (1..=shape.issues).flat_map(|iid| {
        let h = mix(iid as u64 ^ 0x1007);
        let l1 = 1 + (h % shape.labels.max(1) as u64) as i64;
        let l2 = 1 + ((h / 7 + 1) % shape.labels.max(1) as u64) as i64;
        let by = 1 + (h % shape.saas_users.max(1) as u64) as i64;
        let ts = BASE_TS_US + iid * 60_000_000;
        let mut v = vec![(iid, l1, by, ts)];
        if l2 != l1 && h % 2 == 0 {
            v.push((iid, l2, by, ts + 1000));
        }
        v
    });
    em.emit_iter(
        "INSERT INTO issue_labels (issue_id, label_id, added_by, added_at)",
        "(?, ?, ?, ?)",
        100,
        label_pairs,
        |(iid, lid, by, ts), p| {
            p.extend([P::I(iid), P::I(lid), P::I(by), P::I(ts)]);
        },
    )?;

    em.emit_rows(
        "INSERT INTO issue_comments (id, tenant_id, issue_id, author_id, body, created_at)",
        "(?, ?, ?, ?, ?, ?)",
        shape.issue_comments,
        100,
        |id, p| {
            let h = mix(id as u64 ^ 0x1008);
            // Leave ~20% of issues without comments for anti-join stale-issue queries!
            let active_issues = ((shape.issues * 4) / 5).max(1);
            let iid = 1 + (h % active_issues as u64) as i64;
            let tid = shape.skewed_tenant(mix(iid as u64 ^ 0x1006));
            let author = 1 + (mix(h ^ 11) % shape.saas_users.max(1) as u64) as i64;
            let rare = if id % 700 == 0 {
                Some("LGTM-quasar")
            } else {
                None
            };
            p.extend([
                P::I(id),
                P::I(tid),
                P::I(iid),
                P::I(author),
                P::T(make_text(h ^ 0xdef, 160, rare)),
                P::I(BASE_TS_US + id * 45_000_000),
            ]);
        },
    )?;

    let mimes = [
        "image/png",
        "application/pdf",
        "text/plain",
        "application/octet-stream",
    ];
    em.emit_rows(
        "INSERT INTO attachments (id, tenant_id, issue_id, filename, mime_type, size_bytes, payload)",
        "(?, ?, ?, ?, ?, ?, ?)",
        shape.attachments,
        20,
        |id, p| {
            let h = mix(id as u64 ^ 0x1009);
            let iid = 1 + (h % shape.issues.max(1) as u64) as i64;
            let tid = shape.skewed_tenant(mix(iid as u64 ^ 0x1006));
            // Mix of small thumbnails (256 B) and multi-page overflow blobs (8 KiB & 24 KiB)!
            let sz = if id % 20 == 0 {
                24_576
            } else if id % 5 == 0 {
                8_192
            } else {
                256
            };
            let mime = mimes[(h % mimes.len() as u64) as usize];
            p.extend([
                P::I(id),
                P::I(tid),
                P::I(iid),
                P::T(format!("artifact-{id}.bin")),
                P::T(mime.into()),
                P::I(sz as i64),
                P::B(make_blob(h, sz)),
            ]);
        },
    )?;

    // =========================================================================
    // Domain 1: Agentic Coding & Multi-Agent Fleet (9 tables)
    // =========================================================================
    em.emit_rows(
        "INSERT INTO workspaces (id, tenant_id, repo_url, default_branch, created_at)",
        "(?, ?, ?, ?, ?)",
        shape.workspaces,
        100,
        |id, p| {
            let h = mix(id as u64 ^ 0x2001);
            p.extend([
                P::I(id),
                P::I(shape.skewed_tenant(h)),
                P::T(format!("https://git.internal/org/repo-{id}")),
                P::T(if id % 5 == 0 {
                    "master".into()
                } else {
                    "main".into()
                }),
                P::I(BASE_TS_US - id * 3_600_000_000),
            ]);
        },
    )?;

    let run_statuses = [
        "queued",
        "running",
        "completed",
        "completed",
        "completed",
        "failed",
        "cancelled",
    ];
    em.emit_rows(
        "INSERT INTO agent_runs (id, workspace_id, parent_run_id, model, status, priority, started_at, finished_at, total_tokens, cost_micros, summary)",
        "(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        shape.agent_runs,
        100,
        |id, p| {
            let h = mix(id as u64 ^ 0x2002);
            let ws = 1 + (h % shape.workspaces.max(1) as u64) as i64;
            let parent = if id <= 5 || h % 4 != 0 {
                P::N
            } else {
                P::I(1 + (h % (id - 1) as u64) as i64)
            };
            let status = run_statuses[(h % run_statuses.len() as u64) as usize];
            let started = BASE_TS_US + id * 15_000_000;
            let finished = if matches!(status, "queued" | "running") {
                P::N
            } else {
                P::I(started + ((h % 300) as i64 + 1) * 1_000_000)
            };
            let tokens = 500 + (h % 250_000) as i64;
            let cost = tokens * 3;
            p.extend([
                P::I(id),
                P::I(ws),
                parent,
                P::T(MODELS[(h % MODELS.len() as u64) as usize].into()),
                P::T(status.into()),
                P::I((h % 5) as i64),
                P::I(started),
                finished,
                P::I(tokens),
                P::I(cost),
                if h % 6 == 0 { P::N } else { P::T(format!("Run {id}: {}", word(h))) },
            ]);
        },
    )?;

    em.emit_rows(
        "INSERT INTO agent_turns (id, run_id, turn_no, role, prompt_tokens, completion_tokens, latency_ms, content, created_at)",
        "(?, ?, ?, ?, ?, ?, ?, ?, ?)",
        shape.agent_turns,
        100,
        |id, p| {
            let h = mix(id as u64 ^ 0x2003);
            let run_id = 1 + ((id - 1) % shape.agent_runs.max(1));
            let turn_no = 1 + ((id - 1) / shape.agent_runs.max(1));
            let role = ["user", "assistant", "tool"][(id as usize) % 3];
            p.extend([
                P::I(id),
                P::I(run_id),
                P::I(turn_no),
                P::T(role.into()),
                P::I(200 + (h % 8000) as i64),
                P::I(50 + (h % 2000) as i64),
                P::I(100 + (h % 15_000) as i64),
                P::T(make_text(h, 200, None)),
                P::I(BASE_TS_US + id * 5_000_000),
            ]);
        },
    )?;

    let tc_tpl = format!("(?, ?, ?, ?, ?, ?, ?, ?, {jp}, {jp}, ?)");
    em.emit_rows(
        "INSERT INTO tool_calls (id, run_id, turn_id, parent_span_id, tool_name, status, exit_code, duration_ms, args_json, result_json, created_at)",
        &tc_tpl,
        shape.tool_calls,
        80,
        |id, p| {
            let h = mix(id as u64 ^ 0x2004);
            let turn_id = 1 + ((id - 1) % shape.agent_turns.max(1));
            let run_id = 1 + ((turn_id - 1) % shape.agent_runs.max(1));
            // Valid parent tree within tool_calls (parent < id, chains up to depth ~4)
            let parent = if id <= 10 || h % 3 == 0 {
                P::N
            } else {
                P::I((id - 1 - (h % 6) as i64).max(1))
            };
            let tool = TOOLS[(h % TOOLS.len() as u64) as usize];
            let failed = h % 12 == 0;
            let status = if failed { "error" } else { "ok" };
            let exit = if tool == "run_command" || tool == "run_tests" {
                P::I(if failed { 1 } else { 0 })
            } else {
                P::N
            };
            p.extend([
                P::I(id),
                P::I(run_id),
                P::I(turn_id),
                parent,
                P::T(tool.into()),
                P::T(status.into()),
                exit,
                P::I(5 + (h % 45_000) as i64),
                P::T(format!(r#"{{"tool":"{tool}","path":"src/mod_{}.rs","retry":{}}}"#, h % 200, h % 3)),
                P::T(format!(r#"{{"ok":{},"bytes":{},"warnings":{}}}"#, !failed, 64 + (h % 4096), h % 5)),
                P::I(BASE_TS_US + id * 2_000_000),
            ]);
        },
    )?;

    em.emit_rows(
        "INSERT INTO file_diffs (id, run_id, path, lines_added, lines_removed, patch_body, created_at)",
        "(?, ?, ?, ?, ?, ?, ?)",
        shape.file_diffs,
        25,
        |id, p| {
            let h = mix(id as u64 ^ 0x2005);
            let run_id = 1 + (h % shape.agent_runs.max(1) as u64) as i64;
            let sz = if id % 15 == 0 { 12_288 } else if id % 4 == 0 { 4_096 } else { 512 };
            p.extend([
                P::I(id),
                P::I(run_id),
                P::T(format!("crates/mod_{}/src/file_{}.rs", h % 20, h % 200)),
                P::I(1 + (h % 400) as i64),
                P::I((h % 200) as i64),
                P::T(make_text(h ^ 0x555, sz, Some("@@ -1,10 +1,14 @@"))),
                P::I(BASE_TS_US + id * 20_000_000),
            ]);
        },
    )?;

    let scopes = ["workspace", "user", "global", "session"];
    em.emit_rows(
        "INSERT INTO agent_memories (id, workspace_id, scope, key_name, content, embedding_blob, score, expires_at, updated_at)",
        "(?, ?, ?, ?, ?, ?, ?, ?, ?)",
        shape.agent_memories,
        50,
        |id, p| {
            let h = mix(id as u64 ^ 0x2006);
            let ws = 1 + (h % shape.workspaces.max(1) as u64) as i64;
            let expires = if h % 3 == 0 { P::N } else { P::I(BASE_TS_US + ((h % 90) as i64) * 86_400_000_000) };
            p.extend([
                P::I(id),
                P::I(ws),
                P::T(scopes[(h % scopes.len() as u64) as usize].into()),
                P::T(format!("mem-{}-{id}", word(h))),
                P::T(make_text(h, 240, None)),
                P::B(make_blob(h ^ 0x999, 128)),
                P::F(((h % 1000) as f64) / 1000.0),
                expires,
                P::I(BASE_TS_US + id * 10_000_000),
            ]);
        },
    )?;

    let step_statuses = ["pending", "running", "done", "done", "failed"];
    em.emit_rows(
        "INSERT INTO plan_steps (id, run_id, step_order, title, status, owner_agent, est_seconds, updated_at)",
        "(?, ?, ?, ?, ?, ?, ?, ?)",
        shape.plan_steps,
        100,
        |id, p| {
            let h = mix(id as u64 ^ 0x2007);
            let run_id = 1 + ((id - 1) % shape.agent_runs.max(1));
            let order = 1 + ((id - 1) / shape.agent_runs.max(1));
            p.extend([
                P::I(id),
                P::I(run_id),
                P::I(order),
                P::T(format!("Step {order}: {}", word(h))),
                P::T(step_statuses[(h % step_statuses.len() as u64) as usize].into()),
                P::T(format!("agent-{}", h % 16)),
                P::I(10 + (h % 600) as i64),
                P::I(BASE_TS_US + id * 3_000_000),
            ]);
        },
    )?;

    // DAG edges: step i -> step i + agent_runs (strictly acyclic!)
    let edges = (1..=shape.plan_steps).filter_map(|from| {
        let to = from + shape.agent_runs.max(1);
        if to <= shape.plan_steps {
            Some((from, to, if from % 4 == 0 { "soft" } else { "blocks" }))
        } else {
            None
        }
    });
    em.emit_iter(
        "INSERT INTO plan_edges (from_step_id, to_step_id, kind)",
        "(?, ?, ?)",
        100,
        edges,
        |(from, to, kind), p| {
            p.extend([P::I(from), P::I(to), P::T(kind.into())]);
        },
    )?;

    // Strict double-entry token cost ledger: every run posts +cost (debit) and -cost (credit) -> SUM == 0!
    let token_pairs = (1..=shape.token_ledger_pairs).flat_map(|rid| {
        let h = mix(rid as u64 ^ 0x2002);
        let ws = 1 + (h % shape.workspaces.max(1) as u64) as i64;
        let cost = (500 + (h % 250_000) as i64) * 3;
        let ts = BASE_TS_US + rid * 15_000_000;
        let id_d = rid * 2 - 1;
        let id_c = rid * 2;
        [
            (id_d, ws, rid, "compute_expense", "debit", cost, ts),
            (id_c, ws, rid, "workspace_credit", "credit", -cost, ts),
        ]
    });
    em.emit_iter(
        "INSERT INTO token_cost_ledger (id, workspace_id, run_id, account_code, direction, amount_micros, posted_at)",
        "(?, ?, ?, ?, ?, ?, ?)",
        100,
        token_pairs,
        |(id, ws, rid, acct, dir, amt, ts), p| {
            p.extend([
                P::I(id),
                P::I(ws),
                P::I(rid),
                P::T(acct.into()),
                P::T(dir.into()),
                P::I(amt),
                P::I(ts),
            ]);
        },
    )?;

    // =========================================================================
    // Domain 2: Social Graph, News Feed, Messaging & Moderation (8 tables)
    // =========================================================================
    let locales = [
        "en-US", "ja-JP", "es-ES", "de-DE", "ko-KR", "ar-SA", "fr-FR", "pt-BR",
    ];
    em.emit_rows(
        "INSERT INTO accounts (id, handle, display_name, bio, verified, follower_tier, locale, created_at)",
        "(?, ?, ?, ?, ?, ?, ?, ?)",
        shape.accounts,
        100,
        |id, p| {
            let h = mix(id as u64 ^ 0x3001);
            let is_celeb = id <= shape.celebrities;
            p.extend([
                P::I(id),
                P::T(format!("@{}_{id}", word(h))),
                P::T(format!("{} {id}", UNICODE_NAMES[(id as usize) % UNICODE_NAMES.len()])),
                if h % 5 == 0 { P::N } else { P::T(make_text(h, 90, None)) },
                P::Z(is_celeb || h % 20 == 0),
                P::I(if is_celeb { 3 } else { (h % 3) as i64 }),
                P::T(locales[(h % locales.len() as u64) as usize].into()),
                P::I(BASE_TS_US - id * 60_000_000),
            ]);
        },
    )?;

    // Follows: every account follows 1-2 celebrities (huge fan-in!) + 1-2 peers
    let follow_rows = (1..=shape.accounts).flat_map(|uid| {
        let h = mix(uid as u64 ^ 0x3002);
        let ts = BASE_TS_US + uid * 10_000_000;
        let mut out = Vec::with_capacity(3);
        if shape.celebrities > 0 {
            let c1 = 1 + (h % shape.celebrities as u64) as i64;
            if c1 != uid {
                out.push((uid, c1, ts));
            }
            if shape.celebrities > 1 && h % 2 == 0 {
                let c2 = 1 + ((h / 13 + 1) % shape.celebrities as u64) as i64;
                if c2 != uid && c2 != c1 {
                    out.push((uid, c2, ts + 100));
                }
            }
        }
        if shape.accounts > 2 {
            let peer = 1 + (mix(h ^ 0x42) % shape.accounts as u64) as i64;
            if peer != uid && !out.iter().any(|(_, f, _)| *f == peer) {
                out.push((uid, peer, ts + 200));
            }
        }
        out
    });
    em.emit_iter(
        "INSERT INTO follows (follower_id, followee_id, created_at)",
        "(?, ?, ?)",
        100,
        follow_rows,
        |(a, b, ts), p| {
            p.extend([P::I(a), P::I(b), P::I(ts)]);
        },
    )?;

    // Blocks: ~10% of accounts block 1 account
    let block_rows = (1..=shape.accounts).filter_map(|uid| {
        let h = mix(uid as u64 ^ 0x3003);
        if h % 10 == 0 && shape.accounts > 1 {
            let target = 1 + ((uid + (h % 50) as i64) % shape.accounts);
            if target != uid {
                return Some((uid, target, "spam", BASE_TS_US + uid * 5_000_000));
            }
        }
        None
    });
    em.emit_iter(
        "INSERT INTO blocks (blocker_id, blocked_id, reason, created_at)",
        "(?, ?, ?, ?)",
        100,
        block_rows,
        |(a, b, r, ts), p| {
            p.extend([P::I(a), P::I(b), P::T(r.into()), P::I(ts)]);
        },
    )?;

    em.emit_rows(
        "INSERT INTO channels (id, kind, slug, name, last_message_at, message_count)",
        "(?, ?, ?, ?, ?, ?)",
        shape.channels,
        100,
        |id, p| {
            let h = mix(id as u64 ^ 0x3004);
            let kind = ["public", "private", "dm"][(id as usize) % 3];
            p.extend([
                P::I(id),
                P::T(kind.into()),
                P::T(format!("ch-{}-{id}", word(h))),
                P::T(format!("#{} {id}", word(h))),
                P::I(BASE_TS_US + id * 100_000_000),
                P::I(shape.messages / shape.channels.max(1)),
            ]);
        },
    )?;

    let member_rows = (1..=shape.channels).flat_map(|cid| {
        let count = 4i64.min(shape.accounts);
        (0..count).map(move |k| {
            let aid = 1 + ((cid * 3 + k) % shape.accounts.max(1));
            (
                cid,
                aid,
                if k == 0 { "owner" } else { "member" },
                k * 5,
                k == 3,
            )
        })
    });
    em.emit_iter(
        "INSERT INTO channel_members (channel_id, account_id, role, last_read_seq, muted)",
        "(?, ?, ?, ?, ?)",
        100,
        member_rows,
        |(cid, aid, role, seq, muted), p| {
            p.extend([
                P::I(cid),
                P::I(aid),
                P::T(role.into()),
                P::I(seq),
                P::Z(muted),
            ]);
        },
    )?;

    em.emit_rows(
        "INSERT INTO messages (id, channel_id, seq, author_id, reply_to_id, body, edited_at, deleted_at, created_at)",
        "(?, ?, ?, ?, ?, ?, ?, ?, ?)",
        shape.messages,
        100,
        |id, p| {
            let h = mix(id as u64 ^ 0x3005);
            // Channel 1 is hot (#general) holding 25% of messages
            let cid = if h % 4 == 0 {
                1
            } else {
                1 + (h % shape.channels.max(1) as u64) as i64
            };
            let author = if h % 5 == 0 && shape.celebrities > 0 {
                1 + (h % shape.celebrities as u64) as i64
            } else {
                1 + (mix(h ^ 7) % shape.accounts.max(1) as u64) as i64
            };
            let reply = if id > 4 && h % 4 == 0 {
                P::I((id - 1 - (h % 4) as i64).max(1))
            } else {
                P::N
            };
            let created = BASE_TS_US + id * 10_000_000;
            let edited = if h % 9 == 0 { P::I(created + 5_000_000) } else { P::N };
            let deleted = if h % 40 == 0 { P::I(created + 10_000_000) } else { P::N };
            let rare = if id % 600 == 0 {
                Some("緊急アラート 🚀 nebula-signal")
            } else {
                None
            };
            p.extend([
                P::I(id),
                P::I(cid),
                P::I(id),
                P::I(author),
                reply,
                P::T(make_text(h, 120, rare)),
                edited,
                deleted,
                P::I(created),
            ]);
        },
    )?;

    let reaction_rows = (1..=shape.messages).filter_map(|mid| {
        let h = mix(mid as u64 ^ 0x3006);
        if h % 2 == 0 {
            let aid = 1 + (h % shape.accounts.max(1) as u64) as i64;
            let emoji = EMOJIS[(h % EMOJIS.len() as u64) as usize];
            Some((mid, aid, emoji, BASE_TS_US + mid * 10_000_000 + 5000))
        } else {
            None
        }
    });
    em.emit_iter(
        "INSERT INTO reactions (message_id, account_id, emoji, created_at)",
        "(?, ?, ?, ?)",
        100,
        reaction_rows,
        |(mid, aid, emoji, ts), p| {
            p.extend([P::I(mid), P::I(aid), P::T(emoji.into()), P::I(ts)]);
        },
    )?;

    let rep_statuses = ["open", "triaged", "resolved", "dismissed"];
    em.emit_rows(
        "INSERT INTO moderation_reports (id, target_type, target_id, reporter_id, severity, status, notes, created_at)",
        "(?, ?, ?, ?, ?, ?, ?, ?)",
        shape.mod_reports,
        100,
        |id, p| {
            let h = mix(id as u64 ^ 0x3007);
            p.extend([
                P::I(id),
                P::T(if h % 2 == 0 { "message".into() } else { "account".into() }),
                P::I(1 + (h % shape.messages.max(1) as u64) as i64),
                P::I(1 + (mix(h ^ 3) % shape.accounts.max(1) as u64) as i64),
                P::I(1 + (h % 5) as i64),
                P::T(rep_statuses[(h % rep_statuses.len() as u64) as usize].into()),
                if h % 3 == 0 { P::N } else { P::T(format!("report note {id}")) },
                P::I(BASE_TS_US + id * 30_000_000),
            ]);
        },
    )?;

    // =========================================================================
    // Domain 4: E-Commerce, Inventory & Double-Entry Financial Ledger (8 tables)
    // =========================================================================
    em.emit_rows(
        "INSERT INTO categories (id, parent_id, slug, name)",
        "(?, ?, ?, ?)",
        shape.categories,
        100,
        |id, p| {
            let parent = if id <= 8 { P::N } else { P::I((id - 1) / 2) };
            p.extend([
                P::I(id),
                parent,
                P::T(format!("cat-{id}")),
                P::T(format!("Category {} {id}", word(mix(id as u64 ^ 0x4001)))),
            ]);
        },
    )?;

    let prod_tpl = format!("(?, ?, ?, ?, ?, ?, ?, ?, {jp}, ?)");
    em.emit_rows(
        "INSERT INTO products (id, category_id, sku, name, price_cents, price_float, stock, active, attrs_json, updated_at)",
        &prod_tpl,
        shape.products,
        100,
        |id, p| {
            let h = mix(id as u64 ^ 0x4002);
            let price_c = 199 + (h % 250_000) as i64;
            // Hot SKUs 1..=10 get deep stock for flash-sale contention
            let stock = if id <= 10 { 50_000 } else { (h % 500) as i64 };
            let brand = word(mix(h ^ 5));
            let color = ["black", "silver", "blue", "red", "white"][(h % 5) as usize];
            p.extend([
                P::I(id),
                P::I(1 + (h % shape.categories.max(1) as u64) as i64),
                P::T(format!("SKU-{id:07}")),
                P::T(format!("{} {} #{id}", brand, word(h))),
                P::I(price_c),
                P::F(price_c as f64 / 100.0),
                P::I(stock),
                P::Z(h % 15 != 0),
                P::T(format!(r#"{{"brand":"{brand}","color":"{color}","weight_g":{}}}"#, 50 + (h % 5000))),
                P::I(BASE_TS_US + id * 1_000_000),
            ]);
        },
    )?;

    em.emit_rows(
        "INSERT INTO customers (id, email, country, tier, created_at)",
        "(?, ?, ?, ?, ?)",
        shape.customers,
        100,
        |id, p| {
            let h = mix(id as u64 ^ 0x4003);
            p.extend([
                P::I(id),
                P::T(format!("cust{id}@shop.example.com")),
                P::T(COUNTRIES[(h % COUNTRIES.len() as u64) as usize].into()),
                P::T(TIERS[(h % TIERS.len() as u64) as usize].into()),
                P::I(BASE_TS_US - (h % 365) as i64 * 86_400_000_000),
            ]);
        },
    )?;

    em.emit_rows(
        "INSERT INTO fx_rates (id, currency, rate_float, effective_at)",
        "(?, ?, ?, ?)",
        shape.fx_rates,
        100,
        |id, p| {
            let ci = ((id - 1) as usize) % CURRENCIES.len();
            let step = (id - 1) / CURRENCIES.len() as i64;
            let base = [1.0, 1.08, 1.27, 0.0065, 0.73][ci];
            let jitter = ((mix(id as u64) % 40) as f64 - 20.0) * 0.0005;
            p.extend([
                P::I(id),
                P::T(CURRENCIES[ci].into()),
                P::F((base + jitter).max(0.001)),
                P::I(BASE_TS_US + step * 86_400_000_000),
            ]);
        },
    )?;

    em.emit_rows(
        "INSERT INTO orders (id, customer_id, status, subtotal_cents, tax_cents, total_cents, currency, created_at, shipped_at)",
        "(?, ?, ?, ?, ?, ?, ?, ?, ?)",
        shape.orders,
        100,
        |id, p| {
            let h = mix(id as u64 ^ 0x4004);
            let cust = 1 + (h % shape.customers.max(1) as u64) as i64;
            let status = ORDER_STATUSES[(h % ORDER_STATUSES.len() as u64) as usize];
            let sub = 1000 + (h % 150_000) as i64;
            let tax = sub / 10;
            let tot = sub + tax;
            let created = BASE_TS_US + (id % 90) * 86_400_000_000 + (id * 1_000_000);
            let shipped = if matches!(status, "shipped" | "delivered") {
                P::I(created + 86_400_000_000)
            } else {
                P::N
            };
            p.extend([
                P::I(id),
                P::I(cust),
                P::T(status.into()),
                P::I(sub),
                P::I(tax),
                P::I(tot),
                P::T(CURRENCIES[(h % CURRENCIES.len() as u64) as usize].into()),
                P::I(created),
                shipped,
            ]);
        },
    )?;

    em.emit_rows(
        "INSERT INTO order_items (id, order_id, product_id, qty, unit_price_cents)",
        "(?, ?, ?, ?, ?)",
        shape.order_items,
        100,
        |id, p| {
            let h = mix(id as u64 ^ 0x4005);
            let oid = 1 + ((id - 1) % shape.orders.max(1));
            // Skew: 20% hit top-20 popular products for co-purchase self-joins!
            let pid = if h % 5 == 0 {
                1 + (h % 20.min(shape.products.max(1)) as u64) as i64
            } else {
                1 + (h % shape.products.max(1) as u64) as i64
            };
            p.extend([
                P::I(id),
                P::I(oid),
                P::I(pid),
                P::I(1 + (h % 5) as i64),
                P::I(500 + (h % 50_000) as i64),
            ]);
        },
    )?;

    // Leave ~5% of orders without payments for unreconciled-order detection
    let pay_count = (shape.orders * 19) / 20;
    let providers = ["stripe", "adyen", "apple_pay", "ach"];
    let pay_statuses = ["captured", "captured", "captured", "authorized", "failed"];
    em.emit_rows(
        "INSERT INTO payments (id, order_id, provider, status, amount_cents, idempotency_key, created_at)",
        "(?, ?, ?, ?, ?, ?, ?)",
        pay_count,
        100,
        |id, p| {
            let h = mix(id as u64 ^ 0x4004);
            let sub = 1000 + (h % 150_000) as i64;
            let tot = sub + sub / 10;
            let created = BASE_TS_US + (id % 90) * 86_400_000_000 + (id * 1_000_000) + 500_000;
            p.extend([
                P::I(id),
                P::I(id),
                P::T(providers[(h % providers.len() as u64) as usize].into()),
                P::T(pay_statuses[(h % pay_statuses.len() as u64) as usize].into()),
                P::I(tot),
                P::T(format!("idem-order-{id}")),
                P::I(created),
            ]);
        },
    )?;

    // Strict double-entry ledger: every paid order posts +tot (debit) and -tot (credit) -> SUM == 0!
    let ledger_rows = (1..=pay_count).flat_map(|oid| {
        let h = mix(oid as u64 ^ 0x4004);
        let sub = 1000 + (h % 150_000) as i64;
        let tot = sub + sub / 10;
        let acct_a = 1 + (h % (shape.ledger_accounts / 2).max(1) as u64) as i64;
        let acct_b = (shape.ledger_accounts / 2).max(1)
            + 1
            + (h % (shape.ledger_accounts / 2).max(1) as u64) as i64;
        let ts = BASE_TS_US + (oid % 90) * 86_400_000_000 + oid * 1_000_000;
        [
            (oid * 2 - 1, acct_a, oid, "debit", tot, ts),
            (oid * 2, acct_b, oid, "credit", -tot, ts),
        ]
    });
    em.emit_iter(
        "INSERT INTO ledger_entries (id, account_id, order_id, direction, amount_cents, note, posted_at)",
        "(?, ?, ?, ?, ?, ?, ?)",
        100,
        ledger_rows,
        |(id, acct, oid, dir, amt, ts), p| {
            p.extend([
                P::I(id),
                P::I(acct),
                P::I(oid),
                P::T(dir.into()),
                P::I(amt),
                P::T(format!("order-{oid}-{dir}")),
                P::I(ts),
            ]);
        },
    )?;

    // =========================================================================
    // Domain 5: Observability, Distributed Tracing & Experimentation (4 tables)
    // =========================================================================
    em.emit_rows(
        "INSERT INTO services (id, name, team_name, tier)",
        "(?, ?, ?, ?)",
        shape.services,
        100,
        |id, p| {
            p.extend([
                P::I(id),
                P::T(format!("svc-{}-{id}", word(mix(id as u64 ^ 0x5001)))),
                P::T(format!("infra-team-{}", 1 + (id % 8))),
                P::I(1 + (id % 3)),
            ]);
        },
    )?;

    let span_tpl = format!("(?, ?, ?, ?, ?, ?, ?, {jp}, ?)");
    let ops = [
        "http.get",
        "http.post",
        "db.query",
        "cache.get",
        "rpc.call",
        "llm.infer",
    ];
    em.emit_rows(
        "INSERT INTO spans (id, trace_id, parent_span_id, service_id, op_name, status_code, duration_us, tags_json, started_at)",
        &span_tpl,
        shape.spans,
        100,
        |id, p| {
            let h = mix(id as u64 ^ 0x5002);
            let trace_id = 1 + ((id - 1) % shape.traces.max(1));
            let pos = (id - 1) / shape.traces.max(1);
            let parent = if pos == 0 {
                P::N
            } else {
                P::I(id - shape.traces.max(1))
            };
            let svc = 1 + (h % shape.services.max(1) as u64) as i64;
            let code = if h % 25 == 0 { 500 } else if h % 15 == 0 { 429 } else { 200 };
            let dur = if h % 100 == 0 { 500_000 + (h % 2_000_000) as i64 } else { 100 + (h % 20_000) as i64 };
            let region = COUNTRIES[(h % COUNTRIES.len() as u64) as usize];
            p.extend([
                P::I(id),
                P::I(trace_id),
                parent,
                P::I(svc),
                P::T(ops[(h % ops.len() as u64) as usize].into()),
                P::I(code),
                P::I(dur),
                P::T(format!(r#"{{"region":"{region}","canary":{}}}"#, h % 10 == 0)),
                P::I(BASE_TS_US + id * 1_000_000),
            ]);
        },
    )?;

    em.emit_rows(
        "INSERT INTO experiments (id, key_name, status, started_at)",
        "(?, ?, ?, ?)",
        shape.experiments,
        100,
        |id, p| {
            p.extend([
                P::I(id),
                P::T(format!("exp_{}_{id}", word(mix(id as u64 ^ 0x5003)))),
                P::T(if id % 4 == 0 {
                    "ended".into()
                } else {
                    "running".into()
                }),
                P::I(BASE_TS_US - id * 86_400_000_000),
            ]);
        },
    )?;

    em.emit_rows(
        "INSERT INTO exp_events (id, experiment_id, account_id, variant, event_kind, value_float, ts)",
        "(?, ?, ?, ?, ?, ?, ?)",
        shape.exp_events,
        100,
        |id, p| {
            let h = mix(id as u64 ^ 0x5004);
            let exp = 1 + (h % shape.experiments.max(1) as u64) as i64;
            let acct = 1 + ((id / 2) % shape.accounts.max(1));
            let variant = if (acct + exp) % 2 == 0 { "control" } else { "treatment" };
            let is_conv = id % 2 == 0;
            let kind = if is_conv { "conversion" } else { "exposure" };
            let base = BASE_TS_US + ((id / 2) * 30_000_000);
            let ts = if is_conv { base + ((h % 80_000) as i64) * 1_000_000 } else { base };
            p.extend([
                P::I(id),
                P::I(exp),
                P::I(acct),
                P::T(variant.into()),
                P::T(kind.into()),
                P::F(if is_conv { 1.0 + (h % 50) as f64 } else { 0.0 }),
                P::I(ts),
            ]);
        },
    )?;

    // =========================================================================
    // Domain 6: CI/CD Fleet & Test-Flake Intelligence (2 tables)
    // =========================================================================
    let job_statuses = [
        "queued", "queued", "running", "passed", "passed", "passed", "failed",
    ];
    em.emit_rows(
        "INSERT INTO ci_jobs (id, repo_id, commit_sha, status, priority, runner_id, created_at, claimed_at, finished_at)",
        "(?, ?, ?, ?, ?, ?, ?, ?, ?)",
        shape.ci_jobs,
        100,
        |id, p| {
            let h = mix(id as u64 ^ 0x6001);
            let status = job_statuses[(h % job_statuses.len() as u64) as usize];
            let created = BASE_TS_US + id * 10_000_000;
            let runner = if status == "queued" { P::N } else { P::I(1 + (h % 64) as i64) };
            let claimed = if status == "queued" { P::N } else { P::I(created + 2_000_000) };
            let finished = if matches!(status, "queued" | "running") { P::N } else { P::I(created + 60_000_000) };
            p.extend([
                P::I(id),
                P::I(1 + (h % 20) as i64),
                P::T(format!("{:016x}", mix((id / 4) as u64 ^ 0xdead))),
                P::T(status.into()),
                P::I((h % 10) as i64),
                runner,
                P::I(created),
                claimed,
                finished,
            ]);
        },
    )?;

    let suites = [
        "unit",
        "integration",
        "planner",
        "storage",
        "postgres",
        "e2e",
    ];
    em.emit_rows(
        "INSERT INTO test_results (id, job_id, suite_name, test_name, outcome, duration_ms, run_at)",
        "(?, ?, ?, ?, ?, ?, ?)",
        shape.test_results,
        100,
        |id, p| {
            let h = mix(id as u64 ^ 0x6002);
            let job = 1 + (h % shape.ci_jobs.max(1) as u64) as i64;
            let suite = suites[(h % suites.len() as u64) as usize];
            let test_idx = h % 200;
            // Tests 0..19 are deliberately flaky (~40% fail/flake) so flake intelligence finds them!
            let outcome = if test_idx < 20 {
                if mix(id as u64) % 5 < 2 { "fail" } else { "pass" }
            } else {
                OUTCOMES[(h % OUTCOMES.len() as u64) as usize]
            };
            p.extend([
                P::I(id),
                P::I(job),
                P::T(suite.into()),
                P::T(format!("{suite}::test_{test_idx:03}")),
                P::T(outcome.into()),
                P::I(1 + (h % 12_000) as i64),
                P::I(BASE_TS_US + id * 5_000_000),
            ]);
        },
    )?;

    // =========================================================================
    // Domain 7 & 8: Bookings, Docs & Wiki Graph (3 tables)
    // =========================================================================
    let b_statuses = ["confirmed", "confirmed", "tentative", "cancelled"];
    em.emit_rows(
        "INSERT INTO bookings (id, resource_id, owner_id, start_ts, end_ts, status, title)",
        "(?, ?, ?, ?, ?, ?, ?)",
        shape.bookings,
        100,
        |id, p| {
            let h = mix(id as u64 ^ 0x7001);
            let res = 1 + (h % shape.resources.max(1) as u64) as i64;
            let owner = 1 + (mix(h ^ 2) % shape.accounts.max(1) as u64) as i64;
            // Deliberately create realistic clusters + occasional overlaps + gaps!
            let start = BASE_TS_US
                + ((id / shape.resources.max(1)) * 7_200 + (h % 3_600) as i64) * 1_000_000;
            let dur = (1_800 + (h % 5_400) as i64) * 1_000_000;
            p.extend([
                P::I(id),
                P::I(res),
                P::I(owner),
                P::I(start),
                P::I(start + dur),
                P::T(b_statuses[(h % b_statuses.len() as u64) as usize].into()),
                P::T(format!("Booking {} #{id}", word(h))),
            ]);
        },
    )?;

    em.emit_rows(
        "INSERT INTO documents (id, tenant_id, slug, title, version, body, updated_at)",
        "(?, ?, ?, ?, ?, ?, ?)",
        shape.documents,
        50,
        |id, p| {
            let h = mix(id as u64 ^ 0x8001);
            let sz = if id % 25 == 0 { 6_144 } else { 320 };
            p.extend([
                P::I(id),
                P::I(shape.skewed_tenant(h)),
                P::T(format!("doc-{}-{id}", word(h))),
                P::T(format!("Design Doc: {} #{id}", word(mix(h ^ 3)))),
                P::I(1 + (h % 20) as i64),
                P::T(make_text(h, sz, None)),
                P::I(BASE_TS_US + id * 30_000_000),
            ]);
        },
    )?;

    // Wiki link graph (leave last 15% of docs as orphans with 0 inbound links!)
    let linked_docs = ((shape.documents * 85) / 100).max(1);
    let doc_link_rows = (1..=shape.documents).flat_map(|from| {
        let h = mix(from as u64 ^ 0x8002);
        let mut v = Vec::new();
        if linked_docs > 1 {
            let t1 = 1 + (h % linked_docs as u64) as i64;
            if t1 != from {
                v.push((from, t1, format!("see doc {t1}")));
            }
            let t2 = 1 + (mix(h ^ 9) % linked_docs as u64) as i64;
            if t2 != from && t2 != t1 {
                v.push((from, t2, format!("ref doc {t2}")));
            }
        }
        v
    });
    em.emit_iter(
        "INSERT INTO doc_links (from_doc_id, to_doc_id, anchor_text)",
        "(?, ?, ?)",
        100,
        doc_link_rows,
        |(a, b, txt), p| {
            p.extend([P::I(a), P::I(b), P::T(txt)]);
        },
    )?;

    em.flush()?;
    Ok(em.rows_total)
}
