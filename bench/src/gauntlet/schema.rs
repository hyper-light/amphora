//! The Meta-Scale Agentic End-User Gauntlet schema: 8 production application domains,
//! 43 interconnected tables, and 65+ realistic indexes across SQLite, Amphora Postgres
//! (`PostgresDialect`), and external PostgreSQL.
//!
//! Every column type is valid in SQLite, PostgreSQL 19 (`amphora_pg_catalog`), and real
//! PostgreSQL so the exact same end-user workloads run across all engines without gaming.

use crate::suite::Dialect;

/// Dialect-aware column type helpers so every table definition uses native types per engine.
struct Types {
    pk: &'static str,
    i64_ty: &'static str,
    f64_ty: &'static str,
    bool_ty: &'static str,
    text_ty: &'static str,
    blob_ty: &'static str,
    json_ty: &'static str,
    coll_c: &'static str,
}

impl Types {
    fn for_dialect(d: Dialect, cedar: bool) -> Self {
        match d {
            Dialect::Sqlite => Self {
                pk: "INTEGER PRIMARY KEY",
                i64_ty: "INTEGER",
                f64_ty: "REAL",
                bool_ty: "INTEGER",
                text_ty: "TEXT",
                blob_ty: "BLOB",
                json_ty: "TEXT",
                coll_c: "",
            },
            Dialect::AmphoraPostgres => Self {
                pk: "INTEGER PRIMARY KEY",
                i64_ty: "BIGINT",
                f64_ty: "DOUBLE PRECISION",
                bool_ty: "BOOLEAN",
                text_ty: "TEXT",
                blob_ty: "BYTEA",
                json_ty: "JSONB",
                coll_c: "",
            },
            Dialect::Postgres => Self {
                pk: "BIGINT PRIMARY KEY",
                i64_ty: "BIGINT",
                f64_ty: "DOUBLE PRECISION",
                bool_ty: "BOOLEAN",
                text_ty: "TEXT",
                blob_ty: "BYTEA",
                json_ty: "JSONB",
                coll_c: if cedar { "" } else { " COLLATE \"C\"" },
            },
            Dialect::MySql => Self {
                pk: "BIGINT PRIMARY KEY",
                i64_ty: "BIGINT",
                f64_ty: "DOUBLE",
                bool_ty: "BOOLEAN",
                text_ty: "TEXT",
                blob_ty: "LONGBLOB",
                json_ty: "JSON",
                coll_c: "",
            },
            Dialect::ClickHouse => Self {
                pk: "Int64",
                i64_ty: "Int64",
                f64_ty: "Float64",
                bool_ty: "Bool",
                text_ty: "String",
                blob_ty: "String",
                json_ty: "String",
                coll_c: "",
            },
        }
    }
}

/// All 43 production tables across the 8 end-user domains.
pub fn tables(d: Dialect, cedar: bool) -> Vec<String> {
    let t = Types::for_dialect(d, cedar);
    let pk = t.pk;
    let i = t.i64_ty;
    let f = t.f64_ty;
    let b = t.bool_ty;
    let s = t.text_ty;
    let c = t.coll_c;
    let bl = t.blob_ty;
    let j = t.json_ty;

    vec![
        // -------------------------------------------------------------------------
        // Domain 1: Agentic Coding & Multi-Agent Orchestration Fleet (9 tables)
        // -------------------------------------------------------------------------
        format!("CREATE TABLE workspaces (id {pk}, tenant_id {i} NOT NULL, repo_url {s}{c} NOT NULL, default_branch {s}{c} NOT NULL, created_at {i} NOT NULL)"),
        format!("CREATE TABLE agent_runs (id {pk}, workspace_id {i} NOT NULL, parent_run_id {i}, model {s}{c} NOT NULL, status {s}{c} NOT NULL, priority {i} NOT NULL, started_at {i} NOT NULL, finished_at {i}, total_tokens {i} NOT NULL, cost_micros {i} NOT NULL, summary {s}{c})"),
        format!("CREATE TABLE agent_turns (id {pk}, run_id {i} NOT NULL, turn_no {i} NOT NULL, role {s}{c} NOT NULL, prompt_tokens {i} NOT NULL, completion_tokens {i} NOT NULL, latency_ms {i} NOT NULL, content {s}{c} NOT NULL, created_at {i} NOT NULL)"),
        format!("CREATE TABLE tool_calls (id {pk}, run_id {i} NOT NULL, turn_id {i} NOT NULL, parent_span_id {i}, tool_name {s}{c} NOT NULL, status {s}{c} NOT NULL, exit_code {i}, duration_ms {i} NOT NULL, args_json {j} NOT NULL, result_json {j} NOT NULL, created_at {i} NOT NULL)"),
        format!("CREATE TABLE file_diffs (id {pk}, run_id {i} NOT NULL, path {s}{c} NOT NULL, lines_added {i} NOT NULL, lines_removed {i} NOT NULL, patch_body {s}{c} NOT NULL, created_at {i} NOT NULL)"),
        format!("CREATE TABLE agent_memories (id {pk}, workspace_id {i} NOT NULL, scope {s}{c} NOT NULL, key_name {s}{c} NOT NULL, content {s}{c} NOT NULL, embedding_blob {bl} NOT NULL, score {f} NOT NULL, expires_at {i}, updated_at {i} NOT NULL)"),
        format!("CREATE TABLE plan_steps (id {pk}, run_id {i} NOT NULL, step_order {i} NOT NULL, title {s}{c} NOT NULL, status {s}{c} NOT NULL, owner_agent {s}{c} NOT NULL, est_seconds {i} NOT NULL, updated_at {i} NOT NULL)"),
        format!("CREATE TABLE plan_edges (from_step_id {i} NOT NULL, to_step_id {i} NOT NULL, kind {s}{c} NOT NULL, PRIMARY KEY (from_step_id, to_step_id))"),
        format!("CREATE TABLE token_cost_ledger (id {pk}, workspace_id {i} NOT NULL, run_id {i} NOT NULL, account_code {s}{c} NOT NULL, direction {s}{c} NOT NULL, amount_micros {i} NOT NULL, posted_at {i} NOT NULL)"),

        // -------------------------------------------------------------------------
        // Domain 2: Social Graph, News Feed, Messaging & Moderation (8 tables)
        // -------------------------------------------------------------------------
        format!("CREATE TABLE accounts (id {pk}, handle {s}{c} NOT NULL UNIQUE, display_name {s}{c} NOT NULL, bio {s}{c}, verified {b} NOT NULL, follower_tier {i} NOT NULL, locale {s}{c} NOT NULL, created_at {i} NOT NULL)"),
        format!("CREATE TABLE follows (follower_id {i} NOT NULL, followee_id {i} NOT NULL, created_at {i} NOT NULL, PRIMARY KEY (follower_id, followee_id))"),
        format!("CREATE TABLE blocks (blocker_id {i} NOT NULL, blocked_id {i} NOT NULL, reason {s}{c} NOT NULL, created_at {i} NOT NULL, PRIMARY KEY (blocker_id, blocked_id))"),
        format!("CREATE TABLE channels (id {pk}, kind {s}{c} NOT NULL, slug {s}{c} NOT NULL UNIQUE, name {s}{c} NOT NULL, last_message_at {i} NOT NULL, message_count {i} NOT NULL)"),
        format!("CREATE TABLE channel_members (channel_id {i} NOT NULL, account_id {i} NOT NULL, role {s}{c} NOT NULL, last_read_seq {i} NOT NULL, muted {b} NOT NULL, PRIMARY KEY (channel_id, account_id))"),
        format!("CREATE TABLE messages (id {pk}, channel_id {i} NOT NULL, seq {i} NOT NULL, author_id {i} NOT NULL, reply_to_id {i}, body {s}{c} NOT NULL, edited_at {i}, deleted_at {i}, created_at {i} NOT NULL)"),
        format!("CREATE TABLE reactions (message_id {i} NOT NULL, account_id {i} NOT NULL, emoji {s}{c} NOT NULL, created_at {i} NOT NULL, PRIMARY KEY (message_id, account_id, emoji))"),
        format!("CREATE TABLE moderation_reports (id {pk}, target_type {s}{c} NOT NULL, target_id {i} NOT NULL, reporter_id {i} NOT NULL, severity {i} NOT NULL, status {s}{c} NOT NULL, notes {s}{c}, created_at {i} NOT NULL)"),

        // -------------------------------------------------------------------------
        // Domain 3: Multi-Tenant B2B SaaS Workspace (9 tables)
        // -------------------------------------------------------------------------
        format!("CREATE TABLE tenants (id {pk}, slug {s}{c} NOT NULL UNIQUE, name {s}{c} NOT NULL, plan {s}{c} NOT NULL, seat_limit {i} NOT NULL, settings_json {j} NOT NULL, created_at {i} NOT NULL)"),
        format!("CREATE TABLE teams (id {pk}, tenant_id {i} NOT NULL, name {s}{c} NOT NULL, lead_user_id {i})"),
        format!("CREATE TABLE saas_users (id {pk}, tenant_id {i} NOT NULL, email {s}{c} NOT NULL UNIQUE, display_name {s}{c} NOT NULL, role {s}{c} NOT NULL, active {b} NOT NULL, last_seen_at {i} NOT NULL)"),
        format!("CREATE TABLE projects (id {pk}, tenant_id {i} NOT NULL, parent_project_id {i}, key_code {s}{c} NOT NULL, name {s}{c} NOT NULL, archived {b} NOT NULL, budget_cents {i} NOT NULL)"),
        format!("CREATE TABLE issues (id {pk}, tenant_id {i} NOT NULL, project_id {i} NOT NULL, number {i} NOT NULL, author_id {i} NOT NULL, assignee_id {i}, status {s}{c} NOT NULL, priority {i} NOT NULL, title {s}{c} NOT NULL, description {s}{c} NOT NULL, metadata_json {j} NOT NULL, estimate_hrs {f}, due_at {i}, created_at {i} NOT NULL, updated_at {i} NOT NULL, deleted_at {i})"),
        format!("CREATE TABLE labels (id {pk}, tenant_id {i} NOT NULL, name {s}{c} NOT NULL, color {s}{c} NOT NULL)"),
        format!("CREATE TABLE issue_labels (issue_id {i} NOT NULL, label_id {i} NOT NULL, added_by {i} NOT NULL, added_at {i} NOT NULL, PRIMARY KEY (issue_id, label_id))"),
        format!("CREATE TABLE issue_comments (id {pk}, tenant_id {i} NOT NULL, issue_id {i} NOT NULL, author_id {i} NOT NULL, body {s}{c} NOT NULL, created_at {i} NOT NULL)"),
        format!("CREATE TABLE attachments (id {pk}, tenant_id {i} NOT NULL, issue_id {i} NOT NULL, filename {s}{c} NOT NULL, mime_type {s}{c} NOT NULL, size_bytes {i} NOT NULL, payload {bl} NOT NULL)"),

        // -------------------------------------------------------------------------
        // Domain 4: E-Commerce, Inventory & Double-Entry Financial Ledger (8 tables)
        // -------------------------------------------------------------------------
        format!("CREATE TABLE categories (id {pk}, parent_id {i}, slug {s}{c} NOT NULL UNIQUE, name {s}{c} NOT NULL)"),
        format!("CREATE TABLE products (id {pk}, category_id {i} NOT NULL, sku {s}{c} NOT NULL UNIQUE, name {s}{c} NOT NULL, price_cents {i} NOT NULL, price_float {f} NOT NULL, stock {i} NOT NULL, active {b} NOT NULL, attrs_json {j} NOT NULL, updated_at {i} NOT NULL)"),
        format!("CREATE TABLE customers (id {pk}, email {s}{c} NOT NULL UNIQUE, country {s}{c} NOT NULL, tier {s}{c} NOT NULL, created_at {i} NOT NULL)"),
        format!("CREATE TABLE orders (id {pk}, customer_id {i} NOT NULL, status {s}{c} NOT NULL, subtotal_cents {i} NOT NULL, tax_cents {i} NOT NULL, total_cents {i} NOT NULL, currency {s}{c} NOT NULL, created_at {i} NOT NULL, shipped_at {i})"),
        format!("CREATE TABLE order_items (id {pk}, order_id {i} NOT NULL, product_id {i} NOT NULL, qty {i} NOT NULL, unit_price_cents {i} NOT NULL)"),
        format!("CREATE TABLE payments (id {pk}, order_id {i} NOT NULL, provider {s}{c} NOT NULL, status {s}{c} NOT NULL, amount_cents {i} NOT NULL, idempotency_key {s}{c} NOT NULL UNIQUE, created_at {i} NOT NULL)"),
        format!("CREATE TABLE fx_rates (id {pk}, currency {s}{c} NOT NULL, rate_float {f} NOT NULL, effective_at {i} NOT NULL)"),
        format!("CREATE TABLE ledger_entries (id {pk}, account_id {i} NOT NULL, order_id {i}, direction {s}{c} NOT NULL, amount_cents {i} NOT NULL, note {s}{c}, posted_at {i} NOT NULL)"),

        // -------------------------------------------------------------------------
        // Domain 5: Observability, Distributed Tracing & Experimentation (4 tables)
        // -------------------------------------------------------------------------
        format!("CREATE TABLE services (id {pk}, name {s}{c} NOT NULL UNIQUE, team_name {s}{c} NOT NULL, tier {i} NOT NULL)"),
        format!("CREATE TABLE spans (id {pk}, trace_id {i} NOT NULL, parent_span_id {i}, service_id {i} NOT NULL, op_name {s}{c} NOT NULL, status_code {i} NOT NULL, duration_us {i} NOT NULL, tags_json {j} NOT NULL, started_at {i} NOT NULL)"),
        format!("CREATE TABLE experiments (id {pk}, key_name {s}{c} NOT NULL UNIQUE, status {s}{c} NOT NULL, started_at {i} NOT NULL)"),
        format!("CREATE TABLE exp_events (id {pk}, experiment_id {i} NOT NULL, account_id {i} NOT NULL, variant {s}{c} NOT NULL, event_kind {s}{c} NOT NULL, value_float {f} NOT NULL, ts {i} NOT NULL)"),

        // -------------------------------------------------------------------------
        // Domain 6: CI/CD Build Fleet & Test-Flake Intelligence (2 tables)
        // -------------------------------------------------------------------------
        format!("CREATE TABLE ci_jobs (id {pk}, repo_id {i} NOT NULL, commit_sha {s}{c} NOT NULL, status {s}{c} NOT NULL, priority {i} NOT NULL, runner_id {i}, created_at {i} NOT NULL, claimed_at {i}, finished_at {i})"),
        format!("CREATE TABLE test_results (id {pk}, job_id {i} NOT NULL, suite_name {s}{c} NOT NULL, test_name {s}{c} NOT NULL, outcome {s}{c} NOT NULL, duration_ms {i} NOT NULL, run_at {i} NOT NULL)"),

        // -------------------------------------------------------------------------
        // Domain 7 & 8: Cluster Bookings, Docs & Wiki Graph (3 tables)
        // -------------------------------------------------------------------------
        format!("CREATE TABLE bookings (id {pk}, resource_id {i} NOT NULL, owner_id {i} NOT NULL, start_ts {i} NOT NULL, end_ts {i} NOT NULL, status {s}{c} NOT NULL, title {s}{c} NOT NULL)"),
        format!("CREATE TABLE documents (id {pk}, tenant_id {i} NOT NULL, slug {s}{c} NOT NULL, title {s}{c} NOT NULL, version {i} NOT NULL, body {s}{c} NOT NULL, updated_at {i} NOT NULL)"),
        format!("CREATE TABLE doc_links (from_doc_id {i} NOT NULL, to_doc_id {i} NOT NULL, anchor_text {s}{c} NOT NULL, PRIMARY KEY (from_doc_id, to_doc_id))"),
    ]
}

/// Realistic secondary, composite, mixed-direction, and partial indexes across all 43 tables.
pub fn indexes(d: Dialect) -> Vec<String> {
    let mut idxs: Vec<String> = vec![
        // Domain 1: Agentic
        "CREATE INDEX idx_workspaces_tenant ON workspaces (tenant_id)".into(),
        "CREATE INDEX idx_agent_runs_ws_status ON agent_runs (workspace_id, status, started_at DESC)".into(),
        "CREATE INDEX idx_agent_runs_parent ON agent_runs (parent_run_id)".into(),
        "CREATE INDEX idx_agent_runs_model ON agent_runs (model, status)".into(),
        "CREATE INDEX idx_agent_turns_run_no ON agent_turns (run_id, turn_no)".into(),
        "CREATE INDEX idx_tool_calls_run ON tool_calls (run_id, created_at)".into(),
        "CREATE INDEX idx_tool_calls_parent ON tool_calls (parent_span_id)".into(),
        "CREATE INDEX idx_tool_calls_name_status ON tool_calls (tool_name, status)".into(),
        "CREATE INDEX idx_file_diffs_run ON file_diffs (run_id)".into(),
        "CREATE INDEX idx_file_diffs_path ON file_diffs (path)".into(),
        "CREATE INDEX idx_agent_mem_ws_scope ON agent_memories (workspace_id, scope, score DESC)".into(),
        "CREATE INDEX idx_plan_steps_run_order ON plan_steps (run_id, step_order)".into(),
        "CREATE INDEX idx_plan_edges_to ON plan_edges (to_step_id)".into(),
        "CREATE INDEX idx_token_ledger_ws ON token_cost_ledger (workspace_id, posted_at)".into(),
        "CREATE INDEX idx_token_ledger_run ON token_cost_ledger (run_id)".into(),
        // Domain 2: Social & Messaging
        "CREATE INDEX idx_accounts_tier ON accounts (follower_tier, created_at)".into(),
        "CREATE INDEX idx_follows_followee ON follows (followee_id, created_at DESC)".into(),
        "CREATE INDEX idx_blocks_blocked ON blocks (blocked_id)".into(),
        "CREATE INDEX idx_channel_members_acct ON channel_members (account_id)".into(),
        "CREATE INDEX idx_messages_chan_seq ON messages (channel_id, seq DESC)".into(),
        "CREATE INDEX idx_messages_author ON messages (author_id, created_at DESC)".into(),
        "CREATE INDEX idx_messages_reply ON messages (reply_to_id)".into(),
        "CREATE INDEX idx_reactions_acct ON reactions (account_id)".into(),
        "CREATE INDEX idx_mod_reports_status ON moderation_reports (status, severity DESC, created_at)".into(),
        // Domain 3: SaaS
        "CREATE INDEX idx_teams_tenant ON teams (tenant_id)".into(),
        "CREATE INDEX idx_saas_users_tenant ON saas_users (tenant_id, active)".into(),
        "CREATE INDEX idx_projects_tenant_parent ON projects (tenant_id, parent_project_id)".into(),
        "CREATE INDEX idx_issues_inbox ON issues (tenant_id, status, priority, updated_at DESC)".into(),
        "CREATE INDEX idx_issues_project ON issues (project_id, status)".into(),
        "CREATE INDEX idx_issues_assignee ON issues (tenant_id, assignee_id, status)".into(),
        "CREATE INDEX idx_issues_due ON issues (tenant_id, due_at) WHERE due_at IS NOT NULL".into(),
        "CREATE INDEX idx_issues_open_partial ON issues (tenant_id, updated_at DESC) WHERE deleted_at IS NULL".into(),
        "CREATE INDEX idx_labels_tenant_name ON labels (tenant_id, name)".into(),
        "CREATE INDEX idx_issue_labels_label ON issue_labels (label_id, issue_id)".into(),
        "CREATE INDEX idx_issue_comments_issue ON issue_comments (issue_id, created_at)".into(),
        "CREATE INDEX idx_issue_comments_author ON issue_comments (tenant_id, author_id)".into(),
        "CREATE INDEX idx_attachments_issue ON attachments (issue_id)".into(),
        "CREATE INDEX idx_attachments_mime ON attachments (tenant_id, mime_type)".into(),
        // Domain 4: E-Commerce & Ledger
        "CREATE INDEX idx_categories_parent ON categories (parent_id)".into(),
        "CREATE INDEX idx_products_cat_price ON products (category_id, price_cents)".into(),
        "CREATE INDEX idx_products_active_stock ON products (active, stock)".into(),
        "CREATE INDEX idx_customers_country_tier ON customers (country, tier)".into(),
        "CREATE INDEX idx_orders_cust_created ON orders (customer_id, created_at DESC)".into(),
        "CREATE INDEX idx_orders_status_created ON orders (status, created_at DESC)".into(),
        "CREATE INDEX idx_order_items_order ON order_items (order_id)".into(),
        "CREATE INDEX idx_order_items_product ON order_items (product_id, order_id)".into(),
        "CREATE INDEX idx_payments_order ON payments (order_id)".into(),
        "CREATE INDEX idx_payments_status ON payments (status, created_at)".into(),
        "CREATE INDEX idx_fx_rates_curr_eff ON fx_rates (currency, effective_at DESC)".into(),
        "CREATE INDEX idx_ledger_acct_posted ON ledger_entries (account_id, posted_at, id)".into(),
        "CREATE INDEX idx_ledger_order ON ledger_entries (order_id)".into(),
        // Domain 5: Observability & Experiments
        "CREATE INDEX idx_spans_trace ON spans (trace_id, started_at)".into(),
        "CREATE INDEX idx_spans_parent ON spans (parent_span_id)".into(),
        "CREATE INDEX idx_spans_svc_time ON spans (service_id, started_at DESC, duration_us)".into(),
        "CREATE INDEX idx_exp_events_exp_acct ON exp_events (experiment_id, account_id, ts)".into(),
        "CREATE INDEX idx_exp_events_exp_kind ON exp_events (experiment_id, variant, event_kind, ts)".into(),
        // Domain 6: CI/CD
        "CREATE INDEX idx_ci_jobs_queue ON ci_jobs (status, priority DESC, created_at ASC)".into(),
        "CREATE INDEX idx_ci_jobs_repo_sha ON ci_jobs (repo_id, commit_sha)".into(),
        "CREATE INDEX idx_test_results_job ON test_results (job_id)".into(),
        "CREATE INDEX idx_test_results_name_time ON test_results (test_name, run_at DESC)".into(),
        "CREATE INDEX idx_test_results_suite_out ON test_results (suite_name, outcome)".into(),
        // Domain 7 & 8: Bookings & Wiki
        "CREATE INDEX idx_bookings_res_span ON bookings (resource_id, start_ts, end_ts)".into(),
        "CREATE INDEX idx_bookings_owner ON bookings (owner_id, start_ts)".into(),
        "CREATE INDEX idx_documents_tenant_slug ON documents (tenant_id, slug)".into(),
        "CREATE INDEX idx_doc_links_to ON doc_links (to_doc_id)".into(),
    ];

    idxs.extend(match d {
        Dialect::Sqlite | Dialect::AmphoraPostgres => vec!["ANALYZE".into()],
        Dialect::Postgres => vec!["VACUUM ANALYZE".into()],
        Dialect::MySql => vec![],
        Dialect::ClickHouse => vec![],
    });
    idxs
}

/// JSON field extraction expression per dialect.
pub fn json_str(d: Dialect, col: &str, field: &str) -> String {
    match d {
        Dialect::Sqlite | Dialect::AmphoraPostgres => format!("json_extract({col}, '$.{field}')"),
        Dialect::Postgres => format!("{col}->>'{field}'"),
        Dialect::MySql => format!("JSON_UNQUOTE(JSON_EXTRACT({col}, '$.{field}'))"),
        Dialect::ClickHouse => format!("JSONExtractString({col}, '{field}')"),
    }
}

pub fn json_i64(d: Dialect, col: &str, field: &str) -> String {
    match d {
        Dialect::Sqlite | Dialect::AmphoraPostgres => format!("json_extract({col}, '$.{field}')"),
        Dialect::Postgres => format!("({col}->>'{field}')::bigint"),
        Dialect::MySql => format!("CAST(JSON_EXTRACT({col}, '$.{field}') AS SIGNED)"),
        Dialect::ClickHouse => format!("JSONExtractInt({col}, '{field}')"),
    }
}

/// Placeholder for a JSON parameter value (`?::text::jsonb` on external PostgreSQL).
pub fn json_ph(d: Dialect) -> &'static str {
    match d {
        Dialect::Postgres => "?::text::jsonb",
        _ => "?",
    }
}

/// GROUP_CONCAT / STRING_AGG expression per dialect.
pub fn string_agg(d: Dialect, expr: &str, sep: &str) -> String {
    match d {
        Dialect::Sqlite | Dialect::AmphoraPostgres => format!("GROUP_CONCAT({expr}, '{sep}')"),
        Dialect::Postgres => format!("STRING_AGG({expr}, '{sep}')"),
        Dialect::MySql => format!("GROUP_CONCAT({expr} SEPARATOR '{sep}')"),
        Dialect::ClickHouse => format!("arrayStringConcat(groupArray({expr}), '{sep}')"),
    }
}

/// Case-insensitive prefix match operator (`GLOB` on SQLite/AmphoraPG for binary ASCII prefix index
/// use when needed, `LIKE` elsewhere).
pub fn month_bucket(d: Dialect, col_us: &str) -> String {
    match d {
        Dialect::Sqlite | Dialect::AmphoraPostgres => {
            format!("strftime('%Y-%m', {col_us} / 1000000, 'unixepoch')")
        }
        Dialect::Postgres => format!("to_char(to_timestamp({col_us} / 1000000.0), 'YYYY-MM')"),
        Dialect::MySql => format!("DATE_FORMAT(FROM_UNIXTIME({col_us} DIV 1000000), '%Y-%m')"),
        Dialect::ClickHouse => {
            format!("formatDateTime(toDateTime(intDiv({col_us}, 1000000)), '%Y-%m')")
        }
    }
}
