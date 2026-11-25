//! Named real-world end-user operations for Domains 1–4:
//! - Domain 1: Agentic Coding & Multi-Agent Orchestration Fleet
//! - Domain 2: Social Graph, News Feed, Messaging & Moderation
//! - Domain 3: Multi-Tenant B2B SaaS Workspace
//! - Domain 4: Global E-Commerce, Inventory & Double-Entry Financial Ledger

use crate::Rng;
use crate::suite::{Dialect, Kind, P, mix};

use super::loader::{BASE_TS_US, GauntletShape};
use super::schema::{json_i64, json_ph, json_str, month_bucket, string_agg};

/// One named end-user workflow in the gauntlet.
#[derive(Clone, Copy)]
pub struct GauntletOp {
    pub name: &'static str,
    pub domain: &'static str,
    pub kind: Kind,
    pub what: &'static str,
}

const fn r(domain: &'static str, name: &'static str, what: &'static str) -> GauntletOp {
    GauntletOp {
        name,
        domain,
        kind: Kind::Read,
        what,
    }
}

const fn w(domain: &'static str, name: &'static str, what: &'static str) -> GauntletOp {
    GauntletOp {
        name,
        domain,
        kind: Kind::Write,
        what,
    }
}

pub const OPS_1_4: &[GauntletOp] = &[
    // =========================================================================
    // Domain 1: Agentic Coding & Multi-Agent Fleet (40 ops)
    // =========================================================================
    r("agentic", "ag_run_detail", "load an agent run with workspace metadata by primary key"),
    r("agentic", "ag_ws_active_runs", "list latest active/running agent runs in a workspace"),
    r("agentic", "ag_run_turns_timeline", "fetch full conversation turns for an agent run in order"),
    r("agentic", "ag_tool_trace_tree", "WITH RECURSIVE tool-call call-tree reconstruction"),
    r("agentic", "ag_run_subtree_rollup", "WITH RECURSIVE parent->child subagent run token rollup"),
    r("agentic", "ag_slowest_tools", "top 20 slowest tool calls with JSON args & run model"),
    r("agentic", "ag_tool_error_rates", "error rate and avg/max duration per tool_name and model"),
    r("agentic", "ag_tool_json_filter", "filter tool calls on JSON retry count and output bytes"),
    r("agentic", "ag_model_token_spend", "total tokens and cost by model and status across fleet"),
    r("agentic", "ag_ws_daily_burn", "monthly/bucketed token cost per workspace"),
    r("agentic", "ag_diff_read_wide", "fetch wide multi-KB file diff patches for a run"),
    r("agentic", "ag_diff_search_path", "find recent runs that modified a given source file path"),
    r("agentic", "ag_diff_churn_top", "top 25 highest-churn source paths across all agent runs"),
    r("agentic", "ag_memory_recall_top", "recall top 20 non-expired memories by score for a workspace"),
    r("agentic", "ag_memory_blob_read", "read binary embedding blobs for a workspace's top memories"),
    r("agentic", "ag_plan_ready_steps", "DAG scheduler: find pending plan steps with NO unfinished dependency"),
    r("agentic", "ag_plan_critical_path", "WITH RECURSIVE longest path / depth in plan_edges DAG"),
    r("agentic", "ag_token_trial_balance", "double-entry token cost ledger trial balance (must sum to 0)"),
    r("agentic", "ag_ws_running_cost", "running cumulative token spend per workspace (window SUM)"),
    r("agentic", "ag_turn_latency_rank", "rank turns by latency within each run (RANK/DENSE_RANK)"),
    r("agentic", "ag_failed_runs_with_tools", "failed runs joined to their failing tool calls and diffs"),
    r("agentic", "ag_idle_workspaces", "workspaces with zero recent runs (LEFT JOIN ... IS NULL)"),
    r("agentic", "ag_multi_agent_models_distinct", "distinct models and tools used per workspace"),
    r("agentic", "ag_heavy_runs_having", "runs exceeding token threshold with >= 3 tool calls"),
    r("agentic", "ag_parent_vs_child_cost", "self-join comparing parent agent runs against spawned child runs"),
    r("agentic", "ag_turn_token_delta", "turn-over-turn prompt token growth via LAG window"),
    r("agentic", "ag_memories_expiring_union", "UNION of expired memories and low-score memories for GC"),
    r("agentic", "ag_fleet_kpi_pivot", "conditional multi-bucket KPI pivot across all runs"),
    r("agentic", "ag_export_runs_sorted", "unbounded multi-column sorted export of a workspace's runs"),
    r("agentic", "ag_run_count_total", "whole-table COUNT(*) of tool_calls"),
    w("agentic", "ag_record_turn_txn", "ACID turn: insert turn + tool call + update run tokens + balanced ledger"),
    w("agentic", "ag_upsert_memory", "insert/update workspace memory with 128B embedding blob"),
    w("agentic", "ag_save_wide_diff", "insert 12 KiB overflow file diff patch and clean up"),
    w("agentic", "ag_complete_plan_step", "mark plan step done and bump updated_at"),
    w("agentic", "ag_bulk_tool_calls_50", "bulk insert 50 tool_calls in one statement"),
    w("agentic", "ag_update_run_status", "transition agent_run status and record finished_at"),
    w("agentic", "ag_prune_memory_reinsert", "delete expired/old memory and reinsert fresh entry"),
    w("agentic", "ag_cancel_subruns", "update child runs by parent_run_id"),
    w("agentic", "ag_append_balanced_cost", "insert matched debit+credit token_cost_ledger pair"),
    w("agentic", "ag_turn_with_savepoint", "record turn with SAVEPOINT rollback of failed tool retry"),

    // =========================================================================
    // Domain 2: Social Graph, News Feed, Messaging & Moderation (40 ops)
    // =========================================================================
    r("social", "soc_profile_by_handle", "profile lookup by unique @handle"),
    r("social", "soc_feed_normal_user", "home feed for a normal user excluding blocked authors"),
    r("social", "soc_feed_celebrity_fanout", "all followers of celebrity account #1 (extreme skew!)"),
    r("social", "soc_mutual_follows", "mutual connections via INTERSECT of follows"),
    r("social", "soc_fof_recommendations", "2-hop friend-of-friend recommendations excluding already-followed & blocked"),
    r("social", "soc_channel_latest_50", "latest 50 non-deleted messages in a channel with authors"),
    r("social", "soc_channel_hot_general", "latest 50 messages in hot #general channel #1"),
    r("social", "soc_channel_scrollback", "keyset cursor scroll-back (channel_id = ? AND seq < ?)"),
    r("social", "soc_channel_deep_offset", "deep OFFSET pagination in message timeline"),
    r("social", "soc_unread_counts_all", "unread message counts across all channels an account belongs to"),
    r("social", "soc_thread_replies_tree", "WITH RECURSIVE full reply thread tree from root message"),
    r("social", "soc_message_reactions", "reaction emoji counts for a channel's recent messages"),
    r("social", "soc_top_reacted_messages", "top 20 most-reacted messages joined to author and channel"),
    r("social", "soc_unicode_search", "Unicode/CJK/emoji substring search across message bodies"),
    r("social", "soc_user_activity_history", "an account's recent messages and reactions via UNION ALL"),
    r("social", "soc_blocked_excluded_feed", "verify NOT EXISTS blocklist filtering across channels"),
    r("social", "soc_mod_queue_priority", "open moderation reports sorted by severity DESC, created_at ASC"),
    r("social", "soc_mod_repeat_offenders", "targets with >= 2 moderation reports (GROUP BY HAVING)"),
    r("social", "soc_celebrity_vs_tier_stats", "follower counts and engagement grouped by follower_tier & locale"),
    r("social", "soc_silent_accounts", "accounts that have never posted a message (LEFT JOIN IS NULL)"),
    r("social", "soc_verified_only_authors", "recent messages by verified accounts across public channels"),
    r("social", "soc_channel_leaderboard", "top 15 most active channels with distinct author counts"),
    r("social", "soc_user_mention_like", "case-insensitive handle prefix autocomplete"),
    r("social", "soc_msg_edit_ratio", "edited vs deleted vs active message breakdown per channel"),
    r("social", "soc_reaction_diversity", "COUNT(DISTINCT emoji) and COUNT(DISTINCT account_id) per channel"),
    r("social", "soc_follow_asymmetry", "accounts you follow EXCEPT accounts following you back"),
    r("social", "soc_channel_seq_gaps", "detect message seq gaps via LEAD window"),
    r("social", "soc_mod_unassigned_nulls", "reports with NULL notes sorted NULLS FIRST"),
    r("social", "soc_top_reactors_rank", "rank top reactors per emoji using DENSE_RANK"),
    r("social", "soc_msg_count_total", "whole-table COUNT(*) of messages"),
    w("social", "soc_post_message_txn", "post message + update channel last_message_at & count in 1 txn"),
    w("social", "soc_toggle_reaction", "UPSERT reaction (ON CONFLICT DO NOTHING) then DELETE"),
    w("social", "soc_edit_message", "edit message body and stamp edited_at"),
    w("social", "soc_soft_delete_restore", "soft-delete message (SET deleted_at) and restore to NULL"),
    w("social", "soc_follow_unfollow", "follow account (ON CONFLICT DO NOTHING) and unfollow"),
    w("social", "soc_mark_channel_read", "advance channel_members.last_read_seq for a user"),
    w("social", "soc_file_mod_report", "insert moderation report and triage its status"),
    w("social", "soc_update_profile_unicode", "update account display_name & bio with Unicode/emoji"),
    w("social", "soc_block_and_unfollow", "block user and delete follow edges in 1 transaction"),
    w("social", "soc_bulk_messages_50", "bulk import 50 chat messages in one statement"),

    // =========================================================================
    // Domain 3: Multi-Tenant B2B SaaS Workspace (50 ops)
    // =========================================================================
    r("saas", "saas_login_email_lower", "case-insensitive user login lookup LOWER(email)"),
    r("saas", "saas_inbox_mega_tenant", "paginated open inbox for mega-enterprise tenant #1 (55% of rows!)"),
    r("saas", "saas_inbox_tiny_tenant", "same inbox SQL for small tenant (extreme selectivity contrast)"),
    r("saas", "saas_inbox_empty_tenant", "same inbox SQL for brand-new EMPTY tenant (must return 0 rows!)"),
    r("saas", "saas_inbox_keyset_cursor", "keyset cursor pagination (priority, updated_at, id) on mega-tenant"),
    r("saas", "saas_inbox_deep_offset", "deep OFFSET pagination with heavy priority ties"),
    r("saas", "saas_inbox_badge_count", "open non-deleted issue badge count for tenant #1"),
    r("saas", "saas_inbox_unassigned", "unassigned open issues (assignee_id IS NULL) sorted NULLS LAST"),
    r("saas", "saas_inbox_overdue", "overdue issues (partial index due_at IS NOT NULL)"),
    r("saas", "saas_inbox_label_and", "issues having BOTH label A AND label B (multi-EXISTS)"),
    r("saas", "saas_inbox_label_exclude", "open issues NOT having label X (NOT EXISTS)"),
    r("saas", "saas_issue_detail_hydrate", "full issue hydration: author + LEFT JOIN assignee + project + labels + comments"),
    r("saas", "saas_search_title_desc", "global search bar: title LIKE ? OR description LIKE ?"),
    r("saas", "saas_search_json_sev1", "filter issues by JSON metadata severity = 'sev1' and votes > 50"),
    r("saas", "saas_project_tree_rollup", "WITH RECURSIVE project tree rolling up open issue count & est_hrs"),
    r("saas", "saas_kanban_pivot", "status x priority kanban matrix via conditional SUM(CASE)"),
    r("saas", "saas_stale_no_comments", "open issues with ZERO comments (LEFT JOIN ... IS NULL)"),
    r("saas", "saas_assignee_workload", "open issue count & total estimate_hrs per assignee (incl. unassigned)"),
    r("saas", "saas_top_contributors", "top 20 contributors across issues authored + comments posted (UNION ALL)"),
    r("saas", "saas_attachment_blob_read", "download multi-KB binary attachment payload by issue"),
    r("saas", "saas_attachment_storage_by_mime", "total attachment bytes by tenant and mime_type"),
    r("saas", "saas_tenant_plan_summary", "5-table org health summary across tenants, projects, issues, users"),
    r("saas", "saas_comments_thread", "all comments on an issue joined to commenter display_name"),
    r("saas", "saas_user_permissions_exists", "permission-checked issue fetch via correlated EXISTS on saas_users"),
    r("saas", "saas_due_nulls_first_desc", "sort issues by due_at DESC NULLS FIRST, priority ASC, id ASC"),
    r("saas", "saas_due_nulls_last_asc", "sort issues by due_at ASC NULLS LAST, priority DESC, id ASC"),
    r("saas", "saas_wide_desc_sort_limit", "sort wide 10 KiB descriptions by updated_at DESC LIMIT 30"),
    r("saas", "saas_wide_desc_export_all", "unbounded wide sort of a project's issues (forces sorter work)"),
    r("saas", "saas_issue_age_buckets", "bucket open issues by age (0-7d, 7-30d, 30d+) with CASE"),
    r("saas", "saas_label_cooccurrence", "self-join on issue_labels finding top co-occurring label pairs"),
    r("saas", "saas_not_in_null_trap", "projects whose id NOT IN nullable parent_project_id (3VL trap!)"),
    r("saas", "saas_empty_scalar_subquery", "correlated scalar subquery returning NULL when no comments match"),
    r("saas", "saas_multi_window_issues", "two distinct WINDOW specs (by project & by assignee) in one SELECT"),
    r("saas", "saas_high_card_group_proj", "GROUP BY (tenant_id, project_id, status) with HAVING & aggregates"),
    r("saas", "saas_distinct_commenters", "COUNT(DISTINCT author_id) and COUNT(DISTINCT issue_id) per tenant"),
    r("saas", "saas_tenant_json_sso", "enterprise tenants with JSON sso=true and their active seat usage"),
    r("saas", "saas_issues_per_month", "monthly issue creation & closure trend via date bucketing"),
    r("saas", "saas_active_vs_deleted", "active vs soft-deleted issue ratios per tenant"),
    r("saas", "saas_cross_tenant_top_orgs", "top 15 tenants by issue + comment volume"),
    r("saas", "saas_issue_count_total", "whole-table COUNT(*) of issues"),
    w("saas", "saas_create_issue_workflow", "create issue + attach 2 labels + initial comment in 1 transaction"),
    w("saas", "saas_edit_issue_md_json", "update issue description (markdown) and metadata_json"),
    w("saas", "saas_bulk_reassign_20", "bulk-reassign 20 issues in a range to another user"),
    w("saas", "saas_toggle_issue_label", "UPSERT issue_labels (DO NOTHING) then DELETE"),
    w("saas", "saas_soft_delete_restore", "soft-delete 5 issues and restore them"),
    w("saas", "saas_upload_replace_blob", "insert 16 KiB attachment, shrink to 256 B, grow back, delete"),
    w("saas", "saas_add_comment_bump", "insert comment and bump parent issue.updated_at in 1 txn"),
    w("saas", "saas_archive_project_subq", "update project archived flag using correlated subquery count"),
    w("saas", "saas_unique_email_rollback", "attempt duplicate email insert -> catch UNIQUE violation cleanly"),
    w("saas", "saas_delete_issue_bundle", "delete issue + its labels + comments and re-insert cleanly"),

    // =========================================================================
    // Domain 4: Global E-Commerce, Inventory & Financial Ledger (50 ops)
    // =========================================================================
    r("commerce", "com_sku_point_lookup", "product page lookup by unique SKU"),
    r("commerce", "com_category_tree_browse", "WITH RECURSIVE category subtree product listing"),
    r("commerce", "com_faceted_catalog", "active in-stock products by category, price range & JSON color"),
    r("commerce", "com_facet_sidebar_counts", "sidebar facet counts per category and stock availability"),
    r("commerce", "com_also_bought_self_join", "customers who bought product X also bought Y (self-join order_items)"),
    r("commerce", "com_customer_order_history", "customer's last 20 orders with item counts & payment status"),
    r("commerce", "com_latest_order_per_cust", "most recent order per customer via ROW_NUMBER() = 1"),
    r("commerce", "com_customer_ltv_aov", "customer LTV & AOV with NULLIF zero-division guard"),
    r("commerce", "com_asof_fx_conversion", "temporal as-of FX rate join (latest effective_at <= order.created_at)"),
    r("commerce", "com_ledger_running_balance", "running balance per account via SUM OVER (PARTITION BY account_id)"),
    r("commerce", "com_ledger_trial_balance", "full double-entry trial balance (SUM(amount_cents) must equal 0)"),
    r("commerce", "com_ledger_account_stmt", "paginated ledger statement for an account with order details"),
    r("commerce", "com_unreconciled_orders", "orders with NO captured payment (LEFT JOIN ... IS NULL)"),
    r("commerce", "com_duplicate_payments", "detect orders with > 1 payment row (GROUP BY HAVING)"),
    r("commerce", "com_monthly_revenue_mom", "monthly revenue and MoM growth via LAG window"),
    r("commerce", "com_rolling_7d_revenue", "7-row/day moving revenue average (ROWS BETWEEN 6 PRECEDING AND CURRENT ROW)"),
    r("commerce", "com_spend_deciles_ntile", "customer spend deciles via NTILE(10)"),
    r("commerce", "com_top5_skus_per_cat", "top 5 best-selling SKUs per category via DENSE_RANK"),
    r("commerce", "com_revenue_by_country_tier", "revenue & distinct buyers by customer country and tier"),
    r("commerce", "com_low_stock_alert", "active products with stock < 20 ordered by recent order velocity"),
    r("commerce", "com_refund_rate_by_provider", "payment capture vs failure rates by payment provider"),
    r("commerce", "com_order_full_receipt", "5-table receipt: order + customer + items + products + payment"),
    r("commerce", "com_q1_not_q2_buyers", "buyers in first half EXCEPT buyers in second half (churn)"),
    r("commerce", "com_repeat_buyers_intersect", "customers in both early & late periods (INTERSECT)"),
    r("commerce", "com_high_value_orders_having", "orders with >= 2 items and total > threshold"),
    r("commerce", "com_unpurchased_products", "active products never ordered (NOT EXISTS)"),
    r("commerce", "com_price_float_vs_cents", "arithmetic consistency check between price_cents and price_float"),
    r("commerce", "com_json_brand_rollup", "revenue grouped by JSON product brand attribute"),
    r("commerce", "com_shipped_sla_outliers", "orders with longest shipping delay (shipped_at - created_at)"),
    r("commerce", "com_ledger_debit_credit_split", "debit vs credit totals and counts per account"),
    r("commerce", "com_orders_count_total", "whole-table COUNT(*) of order_items"),
    r("commerce", "com_multi_currency_summary", "order totals by currency and status"),
    r("commerce", "com_vip_customers_cte", "CTE identifying top 1% spenders joined to their favorite category"),
    r("commerce", "com_basket_size_dist", "distribution of items-per-order via derived table GROUP BY"),
    r("commerce", "com_hot_sku_velocity", "order volume across hot SKUs 1..10"),
    w("commerce", "com_checkout_acid_txn", "full checkout: decrement stock + insert order + 2 items + payment + balanced ledger"),
    w("commerce", "com_checkout_savepoint_abort", "checkout with SAVEPOINT rollback on simulated OOS item"),
    w("commerce", "com_refund_restock_txn", "refund order + restock product + balanced reversal ledger pair"),
    w("commerce", "com_webhook_idempotent_upsert", "idempotent payment webhook ON CONFLICT(idempotency_key) DO UPDATE"),
    w("commerce", "com_update_product_price", "update product price_cents, price_float, updated_at"),
    w("commerce", "com_restock_hot_sku", "increment stock on a hot SKU"),
    w("commerce", "com_mark_order_shipped", "mark order shipped and stamp shipped_at"),
    w("commerce", "com_update_product_json", "update product attrs_json specification"),
    w("commerce", "com_bulk_orders_25", "bulk insert 25 orders in one statement"),
    w("commerce", "com_update_customer_tier", "upgrade customer tier using subquery LTV check"),
    w("commerce", "com_delete_cancel_order", "insert temp order + items then delete in one txn"),
    w("commerce", "com_fx_rate_append", "append new FX rate tick"),
    w("commerce", "com_ledger_transfer_txn", "balanced inter-account ledger transfer (debit + credit = 0)"),
    w("commerce", "com_bulk_price_adjust_cat", "adjust prices across a category range"),
    w("commerce", "com_cart_cleanup_txn", "delete & reinsert order_items in a slice"),
];

pub fn statements_1_4(op: &str, d: Dialect) -> Option<Vec<String>> {
    let jp = json_ph(d);
    let one = |s: String| Some(vec![s]);
    let s = match op {
        // ======================= DOMAIN 1: AGENTIC ===========================
        "ag_run_detail" => "SELECT r.id, r.model, r.status, r.total_tokens, r.cost_micros, w.repo_url, w.default_branch FROM agent_runs r JOIN workspaces w ON w.id = r.workspace_id WHERE r.id = ?".into(),
        "ag_ws_active_runs" => "SELECT id, model, status, priority, started_at, total_tokens FROM agent_runs WHERE workspace_id = ? AND status IN ('queued', 'running', 'completed') ORDER BY started_at DESC LIMIT 25".into(),
        "ag_run_turns_timeline" => "SELECT turn_no, role, prompt_tokens, completion_tokens, latency_ms, content FROM agent_turns WHERE run_id = ? ORDER BY turn_no ASC".into(),
        "ag_tool_trace_tree" => "WITH RECURSIVE tree(id, parent_span_id, tool_name, duration_ms, depth) AS (SELECT id, parent_span_id, tool_name, duration_ms, 0 FROM tool_calls WHERE id = ? UNION ALL SELECT c.id, c.parent_span_id, c.tool_name, c.duration_ms, t.depth + 1 FROM tool_calls c JOIN tree t ON c.parent_span_id = t.id WHERE t.depth < 6) SELECT id, tool_name, duration_ms, depth FROM tree ORDER BY depth, id LIMIT 50".into(),
        "ag_run_subtree_rollup" => "WITH RECURSIVE sub(id, total_tokens, cost_micros, depth) AS (SELECT id, total_tokens, cost_micros, 0 FROM agent_runs WHERE id = ? UNION ALL SELECT r.id, r.total_tokens, r.cost_micros, s.depth + 1 FROM agent_runs r JOIN sub s ON r.parent_run_id = s.id WHERE s.depth < 5) SELECT COUNT(*), SUM(total_tokens), SUM(cost_micros) FROM sub".into(),
        "ag_slowest_tools" => "SELECT tc.id, tc.tool_name, tc.duration_ms, tc.args_json, r.model FROM tool_calls tc JOIN agent_runs r ON r.id = tc.run_id WHERE tc.run_id BETWEEN ? AND ? ORDER BY tc.duration_ms DESC, tc.id ASC LIMIT 20".into(),
        "ag_tool_error_rates" => "SELECT tc.tool_name, r.model, COUNT(*), SUM(CASE WHEN tc.status = 'error' THEN 1 ELSE 0 END), AVG(tc.duration_ms), MAX(tc.duration_ms) FROM tool_calls tc JOIN agent_runs r ON r.id = tc.run_id WHERE tc.id BETWEEN ? AND ? GROUP BY tc.tool_name, r.model ORDER BY COUNT(*) DESC, tc.tool_name ASC, r.model ASC LIMIT 25".into(),
        "ag_tool_json_filter" => format!("SELECT id, tool_name, duration_ms FROM tool_calls WHERE id BETWEEN ? AND ? AND {} >= ? AND {} > ? LIMIT 30", json_i64(d, "args_json", "retry"), json_i64(d, "result_json", "bytes")),
        "ag_model_token_spend" => "SELECT model, status, COUNT(*), SUM(total_tokens), SUM(cost_micros) FROM agent_runs GROUP BY model, status ORDER BY SUM(cost_micros) DESC, model ASC".into(),
        "ag_ws_daily_burn" => format!("SELECT workspace_id, {} AS ym, COUNT(*), SUM(cost_micros) FROM agent_runs WHERE id BETWEEN ? AND ? GROUP BY workspace_id, ym ORDER BY SUM(cost_micros) DESC LIMIT 20", month_bucket(d, "started_at")),
        "ag_diff_read_wide" => "SELECT id, path, lines_added, lines_removed, patch_body FROM file_diffs WHERE run_id = ? ORDER BY id".into(),
        "ag_diff_search_path" => "SELECT d.id, d.run_id, d.lines_added, r.model, r.status FROM file_diffs d JOIN agent_runs r ON r.id = d.run_id WHERE d.path = ? ORDER BY d.created_at DESC LIMIT 20".into(),
        "ag_diff_churn_top" => "SELECT path, COUNT(*), SUM(lines_added + lines_removed) AS churn FROM file_diffs GROUP BY path ORDER BY churn DESC, path ASC LIMIT 25".into(),
        "ag_memory_recall_top" => "SELECT id, scope, key_name, content, score FROM agent_memories WHERE workspace_id = ? AND (expires_at IS NULL OR expires_at > ?) ORDER BY score DESC, id ASC LIMIT 20".into(),
        "ag_memory_blob_read" => "SELECT id, key_name, embedding_blob FROM agent_memories WHERE workspace_id = ? ORDER BY score DESC LIMIT 10".into(),
        "ag_plan_ready_steps" => "SELECT ps.id, ps.run_id, ps.step_order, ps.title, ps.owner_agent FROM plan_steps ps WHERE ps.status = 'pending' AND NOT EXISTS (SELECT 1 FROM plan_edges pe JOIN plan_steps dep ON dep.id = pe.from_step_id WHERE pe.to_step_id = ps.id AND dep.status != 'done') ORDER BY ps.id LIMIT 30".into(),
        "ag_plan_critical_path" => "WITH RECURSIVE cp(step_id, total_est, depth) AS (SELECT id, est_seconds, 1 FROM plan_steps WHERE id BETWEEN ? AND ? UNION ALL SELECT e.to_step_id, cp.total_est + s.est_seconds, cp.depth + 1 FROM plan_edges e JOIN cp ON cp.step_id = e.from_step_id JOIN plan_steps s ON s.id = e.to_step_id WHERE cp.depth < 8) SELECT MAX(depth), MAX(total_est) FROM cp".into(),
        "ag_token_trial_balance" => "SELECT COUNT(*), SUM(amount_micros) FROM token_cost_ledger".into(),
        "ag_ws_running_cost" => "SELECT id, workspace_id, amount_micros, SUM(amount_micros) OVER (PARTITION BY workspace_id ORDER BY posted_at, id ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) FROM token_cost_ledger WHERE workspace_id = ? AND direction = 'debit' ORDER BY posted_at, id LIMIT 100".into(),
        "ag_turn_latency_rank" => "SELECT id, run_id, turn_no, latency_ms, RANK() OVER (PARTITION BY run_id ORDER BY latency_ms DESC), DENSE_RANK() OVER (PARTITION BY run_id ORDER BY latency_ms DESC) FROM agent_turns WHERE run_id BETWEEN ? AND ?".into(),
        "ag_failed_runs_with_tools" => "SELECT r.id, r.model, tc.tool_name, tc.exit_code, d.path FROM agent_runs r JOIN tool_calls tc ON tc.run_id = r.id AND tc.status = 'error' LEFT JOIN file_diffs d ON d.run_id = r.id WHERE r.status = 'failed' ORDER BY r.id DESC LIMIT 30".into(),
        "ag_idle_workspaces" => "SELECT w.id, w.repo_url FROM workspaces w LEFT JOIN agent_runs r ON r.workspace_id = w.id AND r.started_at > ? WHERE r.id IS NULL ORDER BY w.id LIMIT 20".into(),
        "ag_multi_agent_models_distinct" => "SELECT r.workspace_id, COUNT(DISTINCT r.model), COUNT(DISTINCT tc.tool_name) FROM agent_runs r JOIN tool_calls tc ON tc.run_id = r.id WHERE r.workspace_id BETWEEN ? AND ? GROUP BY r.workspace_id ORDER BY r.workspace_id".into(),
        "ag_heavy_runs_having" => "SELECT r.id, r.model, COUNT(tc.id) AS calls, SUM(tc.duration_ms) AS total_ms FROM agent_runs r JOIN tool_calls tc ON tc.run_id = r.id WHERE r.total_tokens > ? GROUP BY r.id, r.model HAVING COUNT(tc.id) >= 3 ORDER BY total_ms DESC, r.id ASC LIMIT 20".into(),
        "ag_parent_vs_child_cost" => "SELECT p.id, p.model, p.cost_micros, COUNT(c.id), COALESCE(SUM(c.cost_micros), 0) FROM agent_runs p JOIN agent_runs c ON c.parent_run_id = p.id GROUP BY p.id, p.model, p.cost_micros ORDER BY COUNT(c.id) DESC, p.id ASC LIMIT 20".into(),
        "ag_turn_token_delta" => "SELECT id, run_id, turn_no, prompt_tokens, prompt_tokens - COALESCE(LAG(prompt_tokens) OVER (PARTITION BY run_id ORDER BY turn_no), 0) FROM agent_turns WHERE run_id BETWEEN ? AND ?".into(),
        "ag_memories_expiring_union" => "SELECT id, workspace_id FROM agent_memories WHERE expires_at IS NOT NULL AND expires_at < ? UNION SELECT id, workspace_id FROM agent_memories WHERE score < 0.05 ORDER BY id LIMIT 50".into(),
        "ag_fleet_kpi_pivot" => "SELECT model, COUNT(*), SUM(CASE WHEN status = 'completed' THEN 1 ELSE 0 END), SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), AVG(total_tokens) FROM agent_runs GROUP BY model ORDER BY model".into(),
        "ag_export_runs_sorted" => "SELECT id, model, status, priority, total_tokens, cost_micros FROM agent_runs WHERE workspace_id = ? ORDER BY status ASC, priority DESC, finished_at NULLS LAST, id ASC".into(),
        "ag_run_count_total" => "SELECT COUNT(*) FROM tool_calls".into(),
        "ag_record_turn_txn" => return Some(vec![
            "INSERT INTO agent_turns (id, run_id, turn_no, role, prompt_tokens, completion_tokens, latency_ms, content, created_at) VALUES (?, ?, ?, 'assistant', ?, ?, ?, ?, ?)".into(),
            format!("INSERT INTO tool_calls (id, run_id, turn_id, parent_span_id, tool_name, status, exit_code, duration_ms, args_json, result_json, created_at) VALUES (?, ?, ?, NULL, 'run_tests', 'ok', 0, ?, {jp}, {jp}, ?)"),
            "UPDATE agent_runs SET total_tokens = total_tokens + ?, cost_micros = cost_micros + ? WHERE id = ?".into(),
            "INSERT INTO token_cost_ledger (id, workspace_id, run_id, account_code, direction, amount_micros, posted_at) VALUES (?, ?, ?, 'compute_expense', 'debit', ?, ?), (?, ?, ?, 'workspace_credit', 'credit', ?, ?)".into(),
        ]),
        "ag_upsert_memory" => "INSERT INTO agent_memories (id, workspace_id, scope, key_name, content, embedding_blob, score, expires_at, updated_at) VALUES (?, ?, 'workspace', ?, ?, ?, ?, NULL, ?) ON CONFLICT (id) DO UPDATE SET content = excluded.content, score = excluded.score, updated_at = excluded.updated_at".into(),
        "ag_save_wide_diff" => return Some(vec![
            "INSERT INTO file_diffs (id, run_id, path, lines_added, lines_removed, patch_body, created_at) VALUES (?, ?, ?, 120, 40, ?, ?)".into(),
            "DELETE FROM file_diffs WHERE id = ?".into(),
        ]),
        "ag_complete_plan_step" => "UPDATE plan_steps SET status = 'done', updated_at = ? WHERE id = ?".into(),
        "ag_bulk_tool_calls_50" => {
            let row = format!("(?, ?, ?, NULL, 'read_file', 'ok', NULL, ?, {jp}, {jp}, ?)");
            format!("INSERT INTO tool_calls (id, run_id, turn_id, parent_span_id, tool_name, status, exit_code, duration_ms, args_json, result_json, created_at) VALUES {}", vec![row; 50].join(", "))
        }
        "ag_update_run_status" => "UPDATE agent_runs SET status = 'completed', finished_at = ? WHERE id = ?".into(),
        "ag_prune_memory_reinsert" => return Some(vec![
            "DELETE FROM agent_memories WHERE id = ?".into(),
            "INSERT INTO agent_memories (id, workspace_id, scope, key_name, content, embedding_blob, score, expires_at, updated_at) VALUES (?, ?, 'workspace', ?, ?, ?, 0.9, NULL, ?)".into(),
        ]),
        "ag_cancel_subruns" => "UPDATE agent_runs SET priority = priority WHERE parent_run_id = ?".into(),
        "ag_append_balanced_cost" => "INSERT INTO token_cost_ledger (id, workspace_id, run_id, account_code, direction, amount_micros, posted_at) VALUES (?, ?, ?, 'compute_expense', 'debit', ?, ?), (?, ?, ?, 'workspace_credit', 'credit', ?, ?)".into(),
        "ag_turn_with_savepoint" => return Some(vec![
            "INSERT INTO agent_turns (id, run_id, turn_no, role, prompt_tokens, completion_tokens, latency_ms, content, created_at) VALUES (?, ?, ?, 'assistant', 100, 50, 250, 'ok turn', ?)".into(),
            "SAVEPOINT tool_retry".into(),
            "UPDATE agent_runs SET total_tokens = total_tokens + 999999 WHERE id = ?".into(),
            "ROLLBACK TO SAVEPOINT tool_retry".into(),
            "RELEASE SAVEPOINT tool_retry".into(),
        ]),

        // ======================= DOMAIN 2: SOCIAL ============================
        "soc_profile_by_handle" => "SELECT id, handle, display_name, bio, verified, follower_tier, locale FROM accounts WHERE handle = ?".into(),
        "soc_feed_normal_user" => "SELECT m.id, m.channel_id, m.body, m.created_at, a.handle, a.display_name FROM follows f JOIN messages m ON m.author_id = f.followee_id JOIN accounts a ON a.id = m.author_id WHERE f.follower_id = ? AND m.deleted_at IS NULL AND NOT EXISTS (SELECT 1 FROM blocks b WHERE b.blocker_id = f.follower_id AND b.blocked_id = m.author_id) ORDER BY m.created_at DESC, m.id DESC LIMIT 30".into(),
        "soc_feed_celebrity_fanout" => "SELECT f.follower_id, a.handle, a.verified FROM follows f JOIN accounts a ON a.id = f.follower_id WHERE f.followee_id = 1 ORDER BY f.created_at DESC LIMIT 100".into(),
        "soc_mutual_follows" => "SELECT followee_id FROM follows WHERE follower_id = ? INTERSECT SELECT follower_id FROM follows WHERE followee_id = ? ORDER BY 1 LIMIT 50".into(),
        "soc_fof_recommendations" => "SELECT f2.followee_id, COUNT(*) AS mutuals FROM follows f1 JOIN follows f2 ON f2.follower_id = f1.followee_id WHERE f1.follower_id = ? AND f2.followee_id != ? AND NOT EXISTS (SELECT 1 FROM follows f3 WHERE f3.follower_id = ? AND f3.followee_id = f2.followee_id) AND NOT EXISTS (SELECT 1 FROM blocks b WHERE b.blocker_id = ? AND b.blocked_id = f2.followee_id) GROUP BY f2.followee_id ORDER BY mutuals DESC, f2.followee_id ASC LIMIT 15".into(),
        "soc_channel_latest_50" => "SELECT m.id, m.seq, m.body, m.created_at, a.handle, a.display_name FROM messages m JOIN accounts a ON a.id = m.author_id WHERE m.channel_id = ? AND m.deleted_at IS NULL ORDER BY m.seq DESC LIMIT 50".into(),
        "soc_channel_hot_general" => "SELECT m.id, m.seq, m.body, a.handle FROM messages m JOIN accounts a ON a.id = m.author_id WHERE m.channel_id = 1 AND m.deleted_at IS NULL ORDER BY m.seq DESC LIMIT 50".into(),
        "soc_channel_scrollback" => "SELECT id, seq, author_id, body FROM messages WHERE channel_id = ? AND seq < ? AND deleted_at IS NULL ORDER BY seq DESC LIMIT 50".into(),
        "soc_channel_deep_offset" => "SELECT id, seq, author_id, body FROM messages WHERE channel_id = 1 ORDER BY seq DESC LIMIT 20 OFFSET ?".into(),
        "soc_unread_counts_all" => "SELECT cm.channel_id, c.name, COUNT(m.id) AS unread FROM channel_members cm JOIN channels c ON c.id = cm.channel_id LEFT JOIN messages m ON m.channel_id = cm.channel_id AND m.seq > cm.last_read_seq AND m.deleted_at IS NULL WHERE cm.account_id = ? GROUP BY cm.channel_id, c.name ORDER BY unread DESC, cm.channel_id ASC".into(),
        "soc_thread_replies_tree" => "WITH RECURSIVE th(id, reply_to_id, author_id, body, depth) AS (SELECT id, reply_to_id, author_id, body, 0 FROM messages WHERE id = ? UNION ALL SELECT m.id, m.reply_to_id, m.author_id, m.body, th.depth + 1 FROM messages m JOIN th ON m.reply_to_id = th.id WHERE th.depth < 6) SELECT id, author_id, depth, body FROM th ORDER BY depth, id LIMIT 50".into(),
        "soc_message_reactions" => "SELECT r.message_id, r.emoji, COUNT(*) FROM reactions r JOIN messages m ON m.id = r.message_id WHERE m.channel_id = ? GROUP BY r.message_id, r.emoji ORDER BY COUNT(*) DESC, r.message_id ASC LIMIT 30".into(),
        "soc_top_reacted_messages" => "SELECT m.id, c.slug, a.handle, COUNT(r.account_id) AS rx FROM messages m JOIN channels c ON c.id = m.channel_id JOIN accounts a ON a.id = m.author_id JOIN reactions r ON r.message_id = m.id WHERE m.id BETWEEN ? AND ? GROUP BY m.id, c.slug, a.handle ORDER BY rx DESC, m.id ASC LIMIT 20".into(),
        "soc_unicode_search" => "SELECT id, channel_id, body FROM messages WHERE body LIKE ? LIMIT 25".into(),
        "soc_user_activity_history" => "SELECT id AS item_id, created_at, 'message' AS kind FROM messages WHERE author_id = ? UNION ALL SELECT message_id AS item_id, created_at, 'reaction' AS kind FROM reactions WHERE account_id = ? ORDER BY created_at DESC LIMIT 30".into(),
        "soc_blocked_excluded_feed" => "SELECT m.id, m.author_id, m.body FROM messages m WHERE m.channel_id = 1 AND NOT EXISTS (SELECT 1 FROM blocks b WHERE b.blocker_id = ? AND b.blocked_id = m.author_id) ORDER BY m.seq DESC LIMIT 40".into(),
        "soc_mod_queue_priority" => "SELECT r.id, r.target_type, r.target_id, r.severity, a.handle FROM moderation_reports r JOIN accounts a ON a.id = r.reporter_id WHERE r.status = 'open' ORDER BY r.severity DESC, r.created_at ASC, r.id ASC LIMIT 30".into(),
        "soc_mod_repeat_offenders" => "SELECT target_type, target_id, COUNT(*), MAX(severity) FROM moderation_reports GROUP BY target_type, target_id HAVING COUNT(*) >= 2 ORDER BY COUNT(*) DESC, target_id ASC LIMIT 25".into(),
        "soc_celebrity_vs_tier_stats" => "SELECT a.follower_tier, a.locale, COUNT(DISTINCT a.id), COUNT(f.follower_id) FROM accounts a LEFT JOIN follows f ON f.followee_id = a.id WHERE a.id BETWEEN ? AND ? GROUP BY a.follower_tier, a.locale ORDER BY a.follower_tier DESC, a.locale ASC".into(),
        "soc_silent_accounts" => "SELECT a.id, a.handle FROM accounts a LEFT JOIN messages m ON m.author_id = a.id WHERE a.id BETWEEN ? AND ? AND m.id IS NULL ORDER BY a.id LIMIT 30".into(),
        "soc_verified_only_authors" => "SELECT m.id, a.handle, c.slug FROM messages m JOIN accounts a ON a.id = m.author_id AND a.verified JOIN channels c ON c.id = m.channel_id AND c.kind = 'public' WHERE m.id BETWEEN ? AND ? ORDER BY m.created_at DESC LIMIT 30".into(),
        "soc_channel_leaderboard" => "SELECT c.id, c.slug, COUNT(m.id) AS msgs, COUNT(DISTINCT m.author_id) AS authors FROM channels c JOIN messages m ON m.channel_id = c.id WHERE m.id BETWEEN ? AND ? GROUP BY c.id, c.slug ORDER BY msgs DESC, c.id ASC LIMIT 15".into(),
        "soc_user_mention_like" => "SELECT id, handle, display_name FROM accounts WHERE LOWER(handle) LIKE LOWER(?) ORDER BY follower_tier DESC, id ASC LIMIT 20".into(),
        "soc_msg_edit_ratio" => "SELECT channel_id, COUNT(*), SUM(CASE WHEN edited_at IS NOT NULL THEN 1 ELSE 0 END), SUM(CASE WHEN deleted_at IS NOT NULL THEN 1 ELSE 0 END) FROM messages WHERE id BETWEEN ? AND ? GROUP BY channel_id ORDER BY COUNT(*) DESC LIMIT 20".into(),
        "soc_reaction_diversity" => "SELECT m.channel_id, COUNT(DISTINCT r.emoji), COUNT(DISTINCT r.account_id) FROM reactions r JOIN messages m ON m.id = r.message_id WHERE r.message_id BETWEEN ? AND ? GROUP BY m.channel_id ORDER BY m.channel_id LIMIT 25".into(),
        "soc_follow_asymmetry" => "SELECT followee_id FROM follows WHERE follower_id = ? EXCEPT SELECT follower_id FROM follows WHERE followee_id = ? ORDER BY 1 LIMIT 50".into(),
        "soc_channel_seq_gaps" => "SELECT id, seq, LEAD(seq) OVER (PARTITION BY channel_id ORDER BY seq) - seq AS gap FROM messages WHERE channel_id = ? ORDER BY seq LIMIT 100".into(),
        "soc_mod_unassigned_nulls" => "SELECT id, severity, notes FROM moderation_reports ORDER BY notes NULLS FIRST, severity DESC, id ASC LIMIT 30".into(),
        "soc_top_reactors_rank" => "SELECT emoji, account_id, cnt, DENSE_RANK() OVER (PARTITION BY emoji ORDER BY cnt DESC) AS rk FROM (SELECT emoji, account_id, COUNT(*) AS cnt FROM reactions WHERE message_id BETWEEN ? AND ? GROUP BY emoji, account_id) t ORDER BY emoji, rk, account_id LIMIT 40".into(),
        "soc_msg_count_total" => "SELECT COUNT(*) FROM messages".into(),
        "soc_post_message_txn" => return Some(vec![
            "INSERT INTO messages (id, channel_id, seq, author_id, reply_to_id, body, edited_at, deleted_at, created_at) VALUES (?, ?, ?, ?, NULL, ?, NULL, NULL, ?)".into(),
            "UPDATE channels SET last_message_at = ?, message_count = message_count + 1 WHERE id = ?".into(),
        ]),
        "soc_toggle_reaction" => return Some(vec![
            "INSERT INTO reactions (message_id, account_id, emoji, created_at) VALUES (?, ?, '🔥', ?) ON CONFLICT (message_id, account_id, emoji) DO NOTHING".into(),
            "DELETE FROM reactions WHERE message_id = ? AND account_id = ? AND emoji = '🔥'".into(),
        ]),
        "soc_edit_message" => "UPDATE messages SET body = ?, edited_at = ? WHERE id = ?".into(),
        "soc_soft_delete_restore" => return Some(vec![
            "UPDATE messages SET deleted_at = ? WHERE id = ?".into(),
            "UPDATE messages SET deleted_at = NULL WHERE id = ?".into(),
        ]),
        "soc_follow_unfollow" => return Some(vec![
            "INSERT INTO follows (follower_id, followee_id, created_at) VALUES (?, ?, ?) ON CONFLICT (follower_id, followee_id) DO NOTHING".into(),
            "DELETE FROM follows WHERE follower_id = ? AND followee_id = ?".into(),
        ]),
        "soc_mark_channel_read" => "UPDATE channel_members SET last_read_seq = last_read_seq + 1 WHERE channel_id = ? AND account_id = ?".into(),
        "soc_file_mod_report" => return Some(vec![
            "INSERT INTO moderation_reports (id, target_type, target_id, reporter_id, severity, status, notes, created_at) VALUES (?, 'message', ?, ?, 4, 'open', 'auto-flagged', ?)".into(),
            "UPDATE moderation_reports SET status = 'triaged' WHERE id = ?".into(),
        ]),
        "soc_update_profile_unicode" => "UPDATE accounts SET display_name = ?, bio = ? WHERE id = ?".into(),
        "soc_block_and_unfollow" => return Some(vec![
            "INSERT INTO blocks (blocker_id, blocked_id, reason, created_at) VALUES (?, ?, 'user', ?) ON CONFLICT (blocker_id, blocked_id) DO NOTHING".into(),
            "DELETE FROM follows WHERE follower_id = ? AND followee_id = ?".into(),
            "DELETE FROM blocks WHERE blocker_id = ? AND blocked_id = ?".into(),
        ]),
        "soc_bulk_messages_50" => format!("INSERT INTO messages (id, channel_id, seq, author_id, reply_to_id, body, edited_at, deleted_at, created_at) VALUES {}", vec!["(?, ?, ?, ?, NULL, ?, NULL, NULL, ?)"; 50].join(", ")),

        // ======================= DOMAIN 3: SAAS ==============================
        "saas_login_email_lower" => "SELECT id, tenant_id, email, display_name, role, active FROM saas_users WHERE LOWER(email) = LOWER(?)".into(),
        "saas_inbox_mega_tenant" => "SELECT id, number, title, status, priority, assignee_id, updated_at FROM issues WHERE tenant_id = 1 AND status = 'open' AND deleted_at IS NULL ORDER BY priority ASC, updated_at DESC, id ASC LIMIT 25".into(),
        "saas_inbox_tiny_tenant" => "SELECT id, number, title, status, priority, assignee_id, updated_at FROM issues WHERE tenant_id = ? AND status = 'open' AND deleted_at IS NULL ORDER BY priority ASC, updated_at DESC, id ASC LIMIT 25".into(),
        "saas_inbox_empty_tenant" => "SELECT id, number, title, status, priority, assignee_id, updated_at FROM issues WHERE tenant_id = ? AND status = 'open' AND deleted_at IS NULL ORDER BY priority ASC, updated_at DESC, id ASC LIMIT 25".into(),
        "saas_inbox_keyset_cursor" => "SELECT id, priority, updated_at, title FROM issues WHERE tenant_id = 1 AND status = 'open' AND deleted_at IS NULL AND (priority > ? OR (priority = ? AND updated_at < ?)) ORDER BY priority ASC, updated_at DESC, id ASC LIMIT 25".into(),
        "saas_inbox_deep_offset" => "SELECT id, priority, status, title FROM issues WHERE tenant_id = 1 AND deleted_at IS NULL ORDER BY priority ASC, status ASC, id ASC LIMIT 25 OFFSET ?".into(),
        "saas_inbox_badge_count" => "SELECT COUNT(*) FROM issues WHERE tenant_id = 1 AND status = 'open' AND deleted_at IS NULL".into(),
        "saas_inbox_unassigned" => "SELECT id, title, priority, due_at FROM issues WHERE tenant_id = 1 AND assignee_id IS NULL AND status = 'open' ORDER BY due_at ASC NULLS LAST, priority ASC, id ASC LIMIT 30".into(),
        "saas_inbox_overdue" => "SELECT id, project_id, title, due_at FROM issues WHERE tenant_id = 1 AND due_at IS NOT NULL AND due_at < ? AND status != 'closed' ORDER BY due_at ASC, id ASC LIMIT 30".into(),
        "saas_inbox_label_and" => "SELECT i.id, i.title, i.priority FROM issues i WHERE i.tenant_id = 1 AND EXISTS (SELECT 1 FROM issue_labels il WHERE il.issue_id = i.id AND il.label_id = ?) AND EXISTS (SELECT 1 FROM issue_labels il2 WHERE il2.issue_id = i.id AND il2.label_id = ?) ORDER BY i.id LIMIT 30".into(),
        "saas_inbox_label_exclude" => "SELECT i.id, i.title, i.priority FROM issues i WHERE i.tenant_id = 1 AND i.status = 'open' AND NOT EXISTS (SELECT 1 FROM issue_labels il WHERE il.issue_id = i.id AND il.label_id = ?) ORDER BY i.updated_at DESC, i.id ASC LIMIT 30".into(),
        "saas_issue_detail_hydrate" => format!("SELECT i.id, i.title, i.status, i.priority, p.name, au.display_name, asg.display_name, COUNT(DISTINCT c.id), {} FROM issues i JOIN projects p ON p.id = i.project_id JOIN saas_users au ON au.id = i.author_id LEFT JOIN saas_users asg ON asg.id = i.assignee_id LEFT JOIN issue_comments c ON c.issue_id = i.id LEFT JOIN issue_labels il ON il.issue_id = i.id LEFT JOIN labels l ON l.id = il.label_id WHERE i.id = ? GROUP BY i.id, i.title, i.status, i.priority, p.name, au.display_name, asg.display_name", string_agg(d, "l.name", ",")),
        "saas_search_title_desc" => "SELECT id, tenant_id, title FROM issues WHERE tenant_id = 1 AND (title LIKE ? OR description LIKE ?) ORDER BY updated_at DESC LIMIT 20".into(),
        "saas_search_json_sev1" => format!("SELECT id, title, priority FROM issues WHERE tenant_id = 1 AND {} = 'sev1' AND {} > 50 ORDER BY priority ASC, id ASC LIMIT 30", json_str(d, "metadata_json", "severity"), json_i64(d, "metadata_json", "votes")),
        "saas_project_tree_rollup" => "WITH RECURSIVE ptree(id, name, depth) AS (SELECT id, name, 0 FROM projects WHERE id = ? UNION ALL SELECT p.id, p.name, pt.depth + 1 FROM projects p JOIN ptree pt ON p.parent_project_id = pt.id WHERE pt.depth < 6) SELECT pt.id, pt.name, pt.depth, COUNT(i.id), COALESCE(SUM(i.estimate_hrs), 0) FROM ptree pt LEFT JOIN issues i ON i.project_id = pt.id AND i.status != 'closed' GROUP BY pt.id, pt.name, pt.depth ORDER BY pt.depth, pt.id".into(),
        "saas_kanban_pivot" => "SELECT status, SUM(CASE WHEN priority = 0 THEN 1 ELSE 0 END), SUM(CASE WHEN priority = 1 THEN 1 ELSE 0 END), SUM(CASE WHEN priority = 2 THEN 1 ELSE 0 END), SUM(CASE WHEN priority >= 3 THEN 1 ELSE 0 END) FROM issues WHERE tenant_id = 1 AND deleted_at IS NULL GROUP BY status ORDER BY status".into(),
        "saas_stale_no_comments" => "SELECT i.id, i.title, i.created_at FROM issues i LEFT JOIN issue_comments c ON c.issue_id = i.id WHERE i.tenant_id = 1 AND i.status = 'open' AND c.id IS NULL ORDER BY i.created_at ASC, i.id ASC LIMIT 30".into(),
        "saas_assignee_workload" => "SELECT assignee_id, COUNT(*), COALESCE(SUM(estimate_hrs), 0) FROM issues WHERE tenant_id = 1 AND status IN ('open', 'in_progress') GROUP BY assignee_id ORDER BY COUNT(*) DESC, assignee_id ASC NULLS LAST LIMIT 25".into(),
        "saas_top_contributors" => "SELECT user_id, COUNT(*) AS actions FROM (SELECT author_id AS user_id FROM issues WHERE tenant_id = 1 AND id BETWEEN ? AND ? UNION ALL SELECT author_id AS user_id FROM issue_comments WHERE tenant_id = 1 AND id BETWEEN ? AND ?) u GROUP BY user_id ORDER BY actions DESC, user_id ASC LIMIT 20".into(),
        "saas_attachment_blob_read" => "SELECT id, filename, mime_type, size_bytes, payload FROM attachments WHERE issue_id = ? ORDER BY id".into(),
        "saas_attachment_storage_by_mime" => "SELECT tenant_id, mime_type, COUNT(*), SUM(size_bytes) FROM attachments GROUP BY tenant_id, mime_type ORDER BY SUM(size_bytes) DESC, tenant_id ASC LIMIT 25".into(),
        "saas_tenant_plan_summary" => "WITH pc AS (SELECT tenant_id, COUNT(*) AS projs FROM projects GROUP BY tenant_id), uc AS (SELECT tenant_id, COUNT(*) AS users FROM saas_users GROUP BY tenant_id), ic AS (SELECT tenant_id, COUNT(*) AS issues FROM issues GROUP BY tenant_id) SELECT t.id, t.plan, COALESCE(pc.projs, 0), COALESCE(uc.users, 0), COALESCE(ic.issues, 0) FROM tenants t LEFT JOIN pc ON pc.tenant_id = t.id LEFT JOIN uc ON uc.tenant_id = t.id LEFT JOIN ic ON ic.tenant_id = t.id WHERE t.id BETWEEN ? AND ? ORDER BY t.id".into(),
        "saas_comments_thread" => "SELECT c.id, c.created_at, u.display_name, c.body FROM issue_comments c JOIN saas_users u ON u.id = c.author_id WHERE c.issue_id = ? ORDER BY c.created_at ASC, c.id ASC".into(),
        "saas_user_permissions_exists" => "SELECT i.id, i.title, i.status FROM issues i WHERE i.id = ? AND EXISTS (SELECT 1 FROM saas_users u WHERE u.id = ? AND u.tenant_id = i.tenant_id AND u.active)".into(),
        "saas_due_nulls_first_desc" => "SELECT id, due_at, priority FROM issues WHERE id BETWEEN ? AND ? ORDER BY due_at DESC NULLS FIRST, priority ASC, id ASC LIMIT 40".into(),
        "saas_due_nulls_last_asc" => "SELECT id, due_at, priority FROM issues WHERE id BETWEEN ? AND ? ORDER BY due_at ASC NULLS LAST, priority DESC, id ASC LIMIT 40".into(),
        "saas_wide_desc_sort_limit" => "SELECT id, priority, title, description FROM issues WHERE id BETWEEN ? AND ? ORDER BY updated_at DESC, priority ASC, id ASC LIMIT 30".into(),
        "saas_wide_desc_export_all" => "SELECT id, status, priority, title, description FROM issues WHERE project_id = ? ORDER BY priority ASC, updated_at DESC, id ASC".into(),
        "saas_issue_age_buckets" => "SELECT CASE WHEN updated_at - created_at < 86400000000 THEN 'fast' WHEN updated_at - created_at < 604800000000 THEN 'week' ELSE 'aged' END AS bucket, COUNT(*), AVG(estimate_hrs) FROM issues WHERE tenant_id = 1 GROUP BY bucket ORDER BY bucket".into(),
        "saas_label_cooccurrence" => "SELECT a.label_id, b.label_id, COUNT(*) AS cnt FROM issue_labels a JOIN issue_labels b ON a.issue_id = b.issue_id AND a.label_id < b.label_id WHERE a.issue_id BETWEEN ? AND ? GROUP BY a.label_id, b.label_id ORDER BY cnt DESC, a.label_id ASC, b.label_id ASC LIMIT 20".into(),
        "saas_not_in_null_trap" => "SELECT id, name FROM projects WHERE id NOT IN (SELECT parent_project_id FROM projects WHERE tenant_id = 1) ORDER BY id LIMIT 20".into(),
        "saas_empty_scalar_subquery" => "SELECT i.id, (SELECT MAX(c.created_at) FROM issue_comments c WHERE c.issue_id = i.id AND c.author_id = -999) AS never_match FROM issues i WHERE i.id BETWEEN ? AND ? ORDER BY i.id LIMIT 30".into(),
        "saas_multi_window_issues" => "SELECT id, project_id, assignee_id, ROW_NUMBER() OVER (PARTITION BY project_id ORDER BY priority ASC, id ASC), COUNT(*) OVER (PARTITION BY assignee_id) FROM issues WHERE id BETWEEN ? AND ?".into(),
        "saas_high_card_group_proj" => "SELECT tenant_id, project_id, status, COUNT(*), COALESCE(SUM(estimate_hrs), 0) FROM issues GROUP BY tenant_id, project_id, status HAVING COUNT(*) >= 3 ORDER BY COUNT(*) DESC, project_id ASC LIMIT 30".into(),
        "saas_distinct_commenters" => "SELECT tenant_id, COUNT(DISTINCT author_id), COUNT(DISTINCT issue_id) FROM issue_comments GROUP BY tenant_id ORDER BY COUNT(DISTINCT author_id) DESC, tenant_id ASC LIMIT 20".into(),
        "saas_tenant_json_sso" => format!("SELECT t.id, t.slug, t.seat_limit, COUNT(u.id) FROM tenants t LEFT JOIN saas_users u ON u.tenant_id = t.id AND u.active WHERE {} = 'enterprise' GROUP BY t.id, t.slug, t.seat_limit ORDER BY COUNT(u.id) DESC, t.id ASC LIMIT 20", json_str(d, "t.settings_json", "tier")),
        "saas_issues_per_month" => format!("SELECT {} AS ym, COUNT(*), SUM(CASE WHEN status = 'closed' THEN 1 ELSE 0 END) FROM issues GROUP BY ym ORDER BY ym", month_bucket(d, "created_at")),
        "saas_active_vs_deleted" => "SELECT tenant_id, SUM(CASE WHEN deleted_at IS NULL THEN 1 ELSE 0 END), SUM(CASE WHEN deleted_at IS NOT NULL THEN 1 ELSE 0 END) FROM issues GROUP BY tenant_id ORDER BY tenant_id LIMIT 25".into(),
        "saas_cross_tenant_top_orgs" => "WITH ic AS (SELECT tenant_id, COUNT(*) AS issues FROM issues GROUP BY tenant_id), cc AS (SELECT tenant_id, COUNT(*) AS comments FROM issue_comments GROUP BY tenant_id) SELECT t.id, t.slug, COALESCE(ic.issues, 0), COALESCE(cc.comments, 0) FROM tenants t LEFT JOIN ic ON ic.tenant_id = t.id LEFT JOIN cc ON cc.tenant_id = t.id ORDER BY COALESCE(ic.issues, 0) + COALESCE(cc.comments, 0) DESC, t.id ASC LIMIT 20".into(),
        "saas_issue_count_total" => "SELECT COUNT(*) FROM issues".into(),
        "saas_create_issue_workflow" => return Some(vec![
            format!("INSERT INTO issues (id, tenant_id, project_id, number, author_id, assignee_id, status, priority, title, description, metadata_json, estimate_hrs, due_at, created_at, updated_at, deleted_at) VALUES (?, 1, ?, ?, ?, ?, 'open', 1, ?, ?, {jp}, 3.5, NULL, ?, ?, NULL)"),
            "INSERT INTO issue_labels (issue_id, label_id, added_by, added_at) VALUES (?, ?, ?, ?)".into(),
            "INSERT INTO issue_comments (id, tenant_id, issue_id, author_id, body, created_at) VALUES (?, 1, ?, ?, 'Initial triage comment', ?)".into(),
        ]),
        "saas_edit_issue_md_json" => format!("UPDATE issues SET description = ?, metadata_json = {jp}, updated_at = ? WHERE id = ?"),
        "saas_bulk_reassign_20" => "UPDATE issues SET assignee_id = ?, updated_at = ? WHERE id BETWEEN ? AND ?".into(),
        "saas_toggle_issue_label" => return Some(vec![
            "INSERT INTO issue_labels (issue_id, label_id, added_by, added_at) VALUES (?, ?, ?, ?) ON CONFLICT (issue_id, label_id) DO NOTHING".into(),
            "DELETE FROM issue_labels WHERE issue_id = ? AND label_id = ?".into(),
        ]),
        "saas_soft_delete_restore" => return Some(vec![
            "UPDATE issues SET deleted_at = ? WHERE id BETWEEN ? AND ?".into(),
            "UPDATE issues SET deleted_at = NULL WHERE id BETWEEN ? AND ?".into(),
        ]),
        "saas_upload_replace_blob" => return Some(vec![
            "INSERT INTO attachments (id, tenant_id, issue_id, filename, mime_type, size_bytes, payload) VALUES (?, 1, ?, 'big.bin', 'application/octet-stream', 16384, ?)".into(),
            "UPDATE attachments SET size_bytes = 256, payload = ? WHERE id = ?".into(),
            "UPDATE attachments SET size_bytes = 16384, payload = ? WHERE id = ?".into(),
            "DELETE FROM attachments WHERE id = ?".into(),
        ]),
        "saas_add_comment_bump" => return Some(vec![
            "INSERT INTO issue_comments (id, tenant_id, issue_id, author_id, body, created_at) VALUES (?, 1, ?, ?, ?, ?)".into(),
            "UPDATE issues SET updated_at = ? WHERE id = ?".into(),
        ]),
        "saas_archive_project_subq" => "UPDATE projects SET archived = (SELECT COUNT(*) = 0 FROM issues WHERE project_id = projects.id) WHERE id = ?".into(),
        "saas_unique_email_rollback" => return Some(vec![
            "INSERT INTO saas_users (id, tenant_id, email, display_name, role, active, last_seen_at) VALUES (?, 1, ?, 'Temp', 'member', ?, ?)".into(),
            "DELETE FROM saas_users WHERE id = ?".into(),
        ]),
        "saas_delete_issue_bundle" => return Some(vec![
            format!("INSERT INTO issues (id, tenant_id, project_id, number, author_id, assignee_id, status, priority, title, description, metadata_json, estimate_hrs, due_at, created_at, updated_at, deleted_at) VALUES (?, 1, 1, ?, 1, NULL, 'open', 2, 'tmp', 'tmp', {jp}, NULL, NULL, ?, ?, NULL)"),
            "INSERT INTO issue_labels (issue_id, label_id, added_by, added_at) VALUES (?, 1, 1, ?)".into(),
            "DELETE FROM issue_labels WHERE issue_id = ?".into(),
            "DELETE FROM issues WHERE id = ?".into(),
        ]),

        // ======================= DOMAIN 4: COMMERCE ==========================
        "com_sku_point_lookup" => "SELECT id, category_id, name, price_cents, stock, active, attrs_json FROM products WHERE sku = ?".into(),
        "com_category_tree_browse" => "WITH RECURSIVE ctree(id) AS (SELECT id FROM categories WHERE id = ? UNION ALL SELECT c.id FROM categories c JOIN ctree ON c.parent_id = ctree.id) SELECT p.id, p.sku, p.name, p.price_cents, p.stock FROM products p JOIN ctree ON ctree.id = p.category_id WHERE p.active AND p.stock > 0 ORDER BY p.price_cents ASC, p.id ASC LIMIT 30".into(),
        "com_faceted_catalog" => format!("SELECT id, sku, name, price_cents, stock FROM products WHERE active AND stock > 0 AND price_cents BETWEEN ? AND ? AND {} = ? ORDER BY price_cents ASC, id ASC LIMIT 30", json_str(d, "attrs_json", "color")),
        "com_facet_sidebar_counts" => "SELECT category_id, COUNT(*), SUM(CASE WHEN stock > 0 THEN 1 ELSE 0 END), MIN(price_cents), MAX(price_cents) FROM products WHERE active GROUP BY category_id ORDER BY COUNT(*) DESC, category_id ASC LIMIT 20".into(),
        "com_also_bought_self_join" => "SELECT b.product_id, COUNT(*) AS pair_cnt FROM order_items a JOIN order_items b ON a.order_id = b.order_id AND a.product_id != b.product_id WHERE a.product_id = ? GROUP BY b.product_id ORDER BY pair_cnt DESC, b.product_id ASC LIMIT 15".into(),
        "com_customer_order_history" => "SELECT o.id, o.status, o.total_cents, o.created_at, COUNT(oi.id), MAX(p.status) FROM orders o LEFT JOIN order_items oi ON oi.order_id = o.id LEFT JOIN payments p ON p.order_id = o.id WHERE o.customer_id = ? GROUP BY o.id, o.status, o.total_cents, o.created_at ORDER BY o.created_at DESC LIMIT 20".into(),
        "com_latest_order_per_cust" => "SELECT customer_id, id, total_cents, status FROM (SELECT customer_id, id, total_cents, status, ROW_NUMBER() OVER (PARTITION BY customer_id ORDER BY created_at DESC, id DESC) AS rn FROM orders WHERE customer_id BETWEEN ? AND ?) t WHERE rn = 1 ORDER BY customer_id".into(),
        "com_customer_ltv_aov" => "SELECT c.id, c.tier, COUNT(o.id) AS orders, COALESCE(SUM(o.total_cents), 0) AS ltv, COALESCE(SUM(o.total_cents), 0) / NULLIF(COUNT(o.id), 0) AS aov FROM customers c LEFT JOIN orders o ON o.customer_id = c.id AND o.status != 'refunded' WHERE c.id BETWEEN ? AND ? GROUP BY c.id, c.tier ORDER BY ltv DESC, c.id ASC LIMIT 25".into(),
        "com_asof_fx_conversion" => "SELECT o.id, o.currency, o.total_cents, (SELECT r.rate_float FROM fx_rates r WHERE r.currency = o.currency AND r.effective_at <= o.created_at ORDER BY r.effective_at DESC LIMIT 1) AS fx FROM orders o WHERE o.id BETWEEN ? AND ? ORDER BY o.id LIMIT 50".into(),
        "com_ledger_running_balance" => "SELECT id, posted_at, direction, amount_cents, SUM(amount_cents) OVER (PARTITION BY account_id ORDER BY posted_at, id ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) FROM ledger_entries WHERE account_id = ? ORDER BY posted_at, id LIMIT 100".into(),
        "com_ledger_trial_balance" => "SELECT COUNT(*), SUM(amount_cents) FROM ledger_entries".into(),
        "com_ledger_account_stmt" => "SELECT l.id, l.posted_at, l.direction, l.amount_cents, o.status, o.currency FROM ledger_entries l LEFT JOIN orders o ON o.id = l.order_id WHERE l.account_id = ? ORDER BY l.posted_at DESC, l.id DESC LIMIT 40".into(),
        "com_unreconciled_orders" => "SELECT o.id, o.customer_id, o.total_cents, o.status FROM orders o LEFT JOIN payments p ON p.order_id = o.id AND p.status = 'captured' WHERE p.id IS NULL ORDER BY o.created_at DESC, o.id ASC LIMIT 30".into(),
        "com_duplicate_payments" => "SELECT order_id, COUNT(*), SUM(amount_cents) FROM payments GROUP BY order_id HAVING COUNT(*) > 1 ORDER BY COUNT(*) DESC LIMIT 20".into(),
        "com_monthly_revenue_mom" => format!("WITH m AS (SELECT {} AS ym, SUM(total_cents) AS rev FROM orders WHERE status != 'refunded' GROUP BY ym) SELECT ym, rev, LAG(rev) OVER (ORDER BY ym) AS prev_rev FROM m ORDER BY ym", month_bucket(d, "created_at")),
        "com_rolling_7d_revenue" => "WITH d AS (SELECT created_at / 86400000000 AS day_no, SUM(total_cents) AS rev FROM orders GROUP BY day_no) SELECT day_no, rev, AVG(rev) OVER (ORDER BY day_no ROWS BETWEEN 6 PRECEDING AND CURRENT ROW) FROM d ORDER BY day_no".into(),
        "com_spend_deciles_ntile" => "WITH s AS (SELECT customer_id, SUM(total_cents) AS spend FROM orders WHERE customer_id BETWEEN ? AND ? GROUP BY customer_id) SELECT customer_id, spend, NTILE(10) OVER (ORDER BY spend DESC, customer_id ASC) AS decile FROM s ORDER BY decile, spend DESC, customer_id ASC LIMIT 50".into(),
        "com_top5_skus_per_cat" => "WITH sales AS (SELECT p.category_id, p.id AS product_id, SUM(oi.qty * oi.unit_price_cents) AS rev FROM order_items oi JOIN products p ON p.id = oi.product_id WHERE oi.id BETWEEN ? AND ? GROUP BY p.category_id, p.id), ranked AS (SELECT category_id, product_id, rev, DENSE_RANK() OVER (PARTITION BY category_id ORDER BY rev DESC, product_id ASC) AS rk FROM sales) SELECT category_id, product_id, rev, rk FROM ranked WHERE rk <= 5 ORDER BY category_id, rk, product_id LIMIT 50".into(),
        "com_revenue_by_country_tier" => "SELECT c.country, c.tier, COUNT(DISTINCT c.id), COUNT(o.id), SUM(o.total_cents) FROM orders o JOIN customers c ON c.id = o.customer_id GROUP BY c.country, c.tier ORDER BY SUM(o.total_cents) DESC, c.country ASC LIMIT 25".into(),
        "com_low_stock_alert" => "SELECT p.id, p.sku, p.stock, COALESCE(SUM(oi.qty), 0) AS sold FROM products p LEFT JOIN order_items oi ON oi.product_id = p.id WHERE p.active AND p.stock < 20 GROUP BY p.id, p.sku, p.stock ORDER BY sold DESC, p.id ASC LIMIT 25".into(),
        "com_refund_rate_by_provider" => "SELECT provider, COUNT(*), SUM(CASE WHEN status = 'captured' THEN 1 ELSE 0 END), SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), SUM(amount_cents) FROM payments GROUP BY provider ORDER BY provider".into(),
        "com_order_full_receipt" => "SELECT o.id, c.email, oi.qty, p.sku, p.name, pay.provider, pay.status FROM orders o JOIN customers c ON c.id = o.customer_id JOIN order_items oi ON oi.order_id = o.id JOIN products p ON p.id = oi.product_id LEFT JOIN payments pay ON pay.order_id = o.id WHERE o.id = ? ORDER BY oi.id".into(),
        "com_q1_not_q2_buyers" => "SELECT customer_id FROM orders WHERE id BETWEEN ? AND ? EXCEPT SELECT customer_id FROM orders WHERE id BETWEEN ? AND ? ORDER BY 1 LIMIT 50".into(),
        "com_repeat_buyers_intersect" => "SELECT customer_id FROM orders WHERE id BETWEEN ? AND ? INTERSECT SELECT customer_id FROM orders WHERE id BETWEEN ? AND ? ORDER BY 1 LIMIT 50".into(),
        "com_high_value_orders_having" => "SELECT o.id, o.customer_id, COUNT(oi.id), SUM(oi.qty * oi.unit_price_cents) FROM orders o JOIN order_items oi ON oi.order_id = o.id WHERE o.id BETWEEN ? AND ? GROUP BY o.id, o.customer_id HAVING COUNT(oi.id) >= 2 ORDER BY SUM(oi.qty * oi.unit_price_cents) DESC, o.id ASC LIMIT 20".into(),
        "com_unpurchased_products" => "SELECT p.id, p.sku, p.price_cents FROM products p WHERE p.active AND NOT EXISTS (SELECT 1 FROM order_items oi WHERE oi.product_id = p.id) ORDER BY p.id LIMIT 30".into(),
        "com_price_float_vs_cents" => "SELECT COUNT(*) FROM products WHERE ABS(price_float * 100.0 - price_cents) > 0.5".into(),
        "com_json_brand_rollup" => format!("SELECT {} AS brand, COUNT(*), SUM(oi.qty * oi.unit_price_cents) FROM order_items oi JOIN products p ON p.id = oi.product_id WHERE oi.id BETWEEN ? AND ? GROUP BY brand ORDER BY SUM(oi.qty * oi.unit_price_cents) DESC LIMIT 20", json_str(d, "p.attrs_json", "brand")),
        "com_shipped_sla_outliers" => "SELECT id, customer_id, shipped_at - created_at AS lag_us FROM orders WHERE shipped_at IS NOT NULL ORDER BY lag_us DESC, id ASC LIMIT 25".into(),
        "com_ledger_debit_credit_split" => "SELECT account_id, SUM(CASE WHEN direction = 'debit' THEN amount_cents ELSE 0 END), SUM(CASE WHEN direction = 'credit' THEN amount_cents ELSE 0 END), COUNT(*) FROM ledger_entries GROUP BY account_id ORDER BY account_id LIMIT 40".into(),
        "com_orders_count_total" => "SELECT COUNT(*) FROM order_items".into(),
        "com_multi_currency_summary" => "SELECT currency, status, COUNT(*), SUM(total_cents) FROM orders GROUP BY currency, status ORDER BY currency, status".into(),
        "com_vip_customers_cte" => "WITH vip AS (SELECT customer_id, SUM(total_cents) AS spend FROM orders GROUP BY customer_id ORDER BY spend DESC LIMIT 25) SELECT v.customer_id, v.spend, COUNT(DISTINCT p.category_id) FROM vip v JOIN orders o ON o.customer_id = v.customer_id JOIN order_items oi ON oi.order_id = o.id JOIN products p ON p.id = oi.product_id GROUP BY v.customer_id, v.spend ORDER BY v.spend DESC".into(),
        "com_basket_size_dist" => "SELECT items_cnt, COUNT(*) FROM (SELECT order_id, COUNT(*) AS items_cnt FROM order_items WHERE order_id BETWEEN ? AND ? GROUP BY order_id) b GROUP BY items_cnt ORDER BY items_cnt".into(),
        "com_hot_sku_velocity" => "SELECT p.id, p.sku, p.stock, COUNT(oi.id), COALESCE(SUM(oi.qty), 0) FROM products p LEFT JOIN order_items oi ON oi.product_id = p.id WHERE p.id BETWEEN 1 AND 10 GROUP BY p.id, p.sku, p.stock ORDER BY p.id".into(),
        "com_checkout_acid_txn" => return Some(vec![
            "UPDATE products SET stock = stock - 1, updated_at = ? WHERE id = ? AND stock >= 1".into(),
            "INSERT INTO orders (id, customer_id, status, subtotal_cents, tax_cents, total_cents, currency, created_at, shipped_at) VALUES (?, ?, 'paid', 2000, 200, 2200, 'USD', ?, NULL)".into(),
            "INSERT INTO order_items (id, order_id, product_id, qty, unit_price_cents) VALUES (?, ?, ?, 1, 2000)".into(),
            "INSERT INTO payments (id, order_id, provider, status, amount_cents, idempotency_key, created_at) VALUES (?, ?, 'stripe', 'captured', 2200, ?, ?)".into(),
            "INSERT INTO ledger_entries (id, account_id, order_id, direction, amount_cents, note, posted_at) VALUES (?, 1, ?, 'debit', 2200, 'checkout', ?), (?, 101, ?, 'credit', -2200, 'checkout', ?)".into(),
        ]),
        "com_checkout_savepoint_abort" => return Some(vec![
            "SAVEPOINT sp_checkout".into(),
            "UPDATE products SET stock = stock - 1 WHERE id = ? AND stock >= 1".into(),
            "ROLLBACK TO SAVEPOINT sp_checkout".into(),
            "RELEASE SAVEPOINT sp_checkout".into(),
        ]),
        "com_refund_restock_txn" => return Some(vec![
            "UPDATE products SET stock = stock + 1, updated_at = ? WHERE id = ?".into(),
            "INSERT INTO ledger_entries (id, account_id, order_id, direction, amount_cents, note, posted_at) VALUES (?, 101, ?, 'debit', 1500, 'refund', ?), (?, 1, ?, 'credit', -1500, 'refund', ?)".into(),
        ]),
        "com_webhook_idempotent_upsert" => "INSERT INTO payments (id, order_id, provider, status, amount_cents, idempotency_key, created_at) VALUES (?, ?, 'stripe', 'captured', 2200, ?, ?) ON CONFLICT (idempotency_key) DO UPDATE SET status = excluded.status WHERE excluded.created_at >= payments.created_at".into(),
        "com_update_product_price" => "UPDATE products SET price_cents = ?, price_float = ?, updated_at = ? WHERE id = ?".into(),
        "com_restock_hot_sku" => "UPDATE products SET stock = stock + 1, updated_at = ? WHERE id = ?".into(),
        "com_mark_order_shipped" => "UPDATE orders SET status = 'shipped', shipped_at = ? WHERE id = ?".into(),
        "com_update_product_json" => format!("UPDATE products SET attrs_json = {jp}, updated_at = ? WHERE id = ?"),
        "com_bulk_orders_25" => format!("INSERT INTO orders (id, customer_id, status, subtotal_cents, tax_cents, total_cents, currency, created_at, shipped_at) VALUES {}", vec!["(?, ?, 'pending', 1000, 100, 1100, 'USD', ?, NULL)"; 25].join(", ")),
        "com_update_customer_tier" => "UPDATE customers SET tier = CASE WHEN (SELECT COALESCE(SUM(total_cents), 0) FROM orders WHERE customer_id = customers.id) > 100000 THEN 'enterprise' ELSE tier END WHERE id = ?".into(),
        "com_delete_cancel_order" => return Some(vec![
            "INSERT INTO orders (id, customer_id, status, subtotal_cents, tax_cents, total_cents, currency, created_at, shipped_at) VALUES (?, ?, 'pending', 500, 50, 550, 'USD', ?, NULL)".into(),
            "INSERT INTO order_items (id, order_id, product_id, qty, unit_price_cents) VALUES (?, ?, 1, 1, 500)".into(),
            "DELETE FROM order_items WHERE order_id = ?".into(),
            "DELETE FROM orders WHERE id = ?".into(),
        ]),
        "com_fx_rate_append" => "INSERT INTO fx_rates (id, currency, rate_float, effective_at) VALUES (?, 'USD', 1.0, ?)".into(),
        "com_ledger_transfer_txn" => "INSERT INTO ledger_entries (id, account_id, order_id, direction, amount_cents, note, posted_at) VALUES (?, ?, NULL, 'debit', ?, 'xfer', ?), (?, ?, NULL, 'credit', ?, 'xfer', ?)".into(),
        "com_bulk_price_adjust_cat" => "UPDATE products SET updated_at = ? WHERE id BETWEEN ? AND ?".into(),
        "com_cart_cleanup_txn" => return Some(vec![
            "INSERT INTO order_items (id, order_id, product_id, qty, unit_price_cents) VALUES (?, ?, 1, 2, 1000)".into(),
            "DELETE FROM order_items WHERE id = ?".into(),
        ]),
        _ => return None,
    };
    one(s)
}

pub fn params_1_4(
    op: &str,
    rng: &mut Rng,
    s: &GauntletShape,
    c: (usize, usize),
) -> Option<Vec<Vec<P>>> {
    let pick = |rng: &mut Rng, max: i64| -> i64 { 1 + (rng.next() % max.max(1) as u64) as i64 };
    let span = |rng: &mut Rng, max: i64, len: i64| -> (i64, i64) {
        let len = len.min(max.max(1)).max(1);
        let a = 1 + (rng.next() % ((max - len + 1).max(1) as u64)) as i64;
        (a, a + len - 1)
    };
    let own = |rng: &mut Rng, max: i64| -> i64 {
        let n = c.1.max(1) as i64;
        let per = (max / n).max(1);
        let first = 1 + per * (c.0 as i64 % n);
        let last = (first + per - 1).min(max.max(1));
        first + (rng.next() % ((last - first + 1).max(1) as u64)) as i64
    };
    let one = |v: Vec<P>| Some(vec![v]);
    let ts = BASE_TS_US + (rng.next() % 1_000_000) as i64 * 1_000_000;

    match op {
        // Domain 1
        "ag_run_detail" | "ag_run_turns_timeline" | "ag_run_subtree_rollup" | "ag_diff_read_wide" => {
            one(vec![P::I(pick(rng, s.agent_runs))])
        }
        "ag_ws_active_runs" | "ag_memory_blob_read" | "ag_ws_running_cost" | "ag_export_runs_sorted" => {
            one(vec![P::I(pick(rng, s.workspaces))])
        }
        "ag_tool_trace_tree" => one(vec![P::I(pick(rng, s.tool_calls))]),
        "ag_slowest_tools" | "ag_turn_latency_rank" | "ag_turn_token_delta" => {
            let (a, b) = span(rng, s.agent_runs, 100);
            one(vec![P::I(a), P::I(b)])
        }
        "ag_tool_error_rates" => {
            let (a, b) = span(rng, s.tool_calls, 5_000);
            one(vec![P::I(a), P::I(b)])
        }
        "ag_tool_json_filter" => {
            let (a, b) = span(rng, s.tool_calls, 5_000);
            one(vec![P::I(a), P::I(b), P::I(1), P::I(512)])
        }
        "ag_ws_daily_burn" => {
            let (a, b) = span(rng, s.agent_runs, 2_000);
            one(vec![P::I(a), P::I(b)])
        }
        "ag_diff_search_path" => {
            let h = rng.next();
            one(vec![P::T(format!("crates/mod_{}/src/file_{}.rs", h % 20, h % 200))])
        }
        "ag_memory_recall_top" => one(vec![P::I(pick(rng, s.workspaces)), P::I(BASE_TS_US)]),
        "ag_plan_critical_path" => {
            let (a, b) = span(rng, s.plan_steps, 200);
            one(vec![P::I(a), P::I(b)])
        }
        "ag_idle_workspaces" | "ag_memories_expiring_union" => {
            one(vec![P::I(BASE_TS_US + 10 * 86_400_000_000)])
        }
        "ag_multi_agent_models_distinct" => {
            let (a, b) = span(rng, s.workspaces, 20);
            one(vec![P::I(a), P::I(b)])
        }
        "ag_heavy_runs_having" => one(vec![P::I(100_000)]),
        "ag_model_token_spend" | "ag_diff_churn_top" | "ag_plan_ready_steps"
        | "ag_token_trial_balance" | "ag_failed_runs_with_tools" | "ag_parent_vs_child_cost"
        | "ag_fleet_kpi_pivot" | "ag_run_count_total" => one(vec![]),
        "ag_record_turn_txn" => {
            let run = own(rng, s.agent_runs);
            let ws = pick(rng, s.workspaces);
            let tid = s.alloc_id();
            let tcid = s.alloc_id();
            let ld = s.alloc_id();
            let lc = s.alloc_id();
            let cost = 1200i64;
            Some(vec![
                vec![P::I(tid), P::I(run), P::I(99), P::I(300), P::I(100), P::I(450), P::T("agent step".into()), P::I(ts)],
                vec![P::I(tcid), P::I(run), P::I(tid), P::I(120), P::T(r#"{"cmd":"cargo test"}"#.into()), P::T(r#"{"ok":true,"bytes":512}"#.into()), P::I(ts)],
                vec![P::I(400), P::I(cost), P::I(run)],
                vec![P::I(ld), P::I(ws), P::I(run), P::I(cost), P::I(ts), P::I(lc), P::I(ws), P::I(run), P::I(-cost), P::I(ts)],
            ])
        }
        "ag_upsert_memory" => {
            let id = own(rng, s.agent_memories);
            one(vec![P::I(id), P::I(1), P::T(format!("k-{id}")), P::T("updated memory".into()), P::B(vec![42u8; 128]), P::F(0.95), P::I(ts)])
        }
        "ag_save_wide_diff" => {
            let id = s.alloc_id();
            let body = "+line of generated patch content for overflow test\n".repeat(250);
            Some(vec![
                vec![P::I(id), P::I(pick(rng, s.agent_runs)), P::T("crates/tmp/patch.rs".into()), P::T(body), P::I(ts)],
                vec![P::I(id)],
            ])
        }
        "ag_complete_plan_step" => one(vec![P::I(ts), P::I(own(rng, s.plan_steps))]),
        "ag_bulk_tool_calls_50" => {
            let run = own(rng, s.agent_runs);
            let turn = own(rng, s.agent_turns);
            let mut v = Vec::with_capacity(50 * 7);
            for _ in 0..50 {
                let id = s.alloc_id();
                v.extend([P::I(id), P::I(run), P::I(turn), P::I(42), P::T(r#"{"p":"a.rs"}"#.into()), P::T(r#"{"ok":true}"#.into()), P::I(ts)]);
            }
            one(v)
        }
        "ag_update_run_status" => one(vec![P::I(ts), P::I(own(rng, s.agent_runs))]),
        "ag_prune_memory_reinsert" => {
            let id = own(rng, s.agent_memories);
            Some(vec![
                vec![P::I(id)],
                vec![P::I(id), P::I(1), P::T(format!("k-{id}")), P::T("fresh memory".into()), P::B(vec![7u8; 128]), P::I(ts)],
            ])
        }
        "ag_cancel_subruns" => one(vec![P::I(own(rng, s.agent_runs))]),
        "ag_append_balanced_cost" => {
            let ld = s.alloc_id();
            let lc = s.alloc_id();
            let ws = pick(rng, s.workspaces);
            let run = own(rng, s.agent_runs);
            one(vec![P::I(ld), P::I(ws), P::I(run), P::I(900), P::I(ts), P::I(lc), P::I(ws), P::I(run), P::I(-900), P::I(ts)])
        }
        "ag_turn_with_savepoint" => {
            let tid = s.alloc_id();
            let run = own(rng, s.agent_runs);
            Some(vec![
                vec![P::I(tid), P::I(run), P::I(101), P::I(ts)],
                vec![],
                vec![P::I(run)],
                vec![],
                vec![],
            ])
        }

        // Domain 2
        "soc_profile_by_handle" => {
            let id = pick(rng, s.accounts);
            let h = mix(id as u64 ^ 0x3001);
            one(vec![P::T(format!("@{}_{id}", crate::WORDS[(h % crate::WORDS.len() as u64) as usize]))])
        }
        "soc_feed_normal_user" | "soc_unread_counts_all" | "soc_blocked_excluded_feed" => {
            one(vec![P::I(pick(rng, s.accounts))])
        }
        "soc_mutual_follows" | "soc_user_activity_history" | "soc_follow_asymmetry" => {
            let a = pick(rng, s.accounts);
            one(vec![P::I(a), P::I(a)])
        }
        "soc_fof_recommendations" => {
            let a = pick(rng, s.accounts);
            one(vec![P::I(a), P::I(a), P::I(a), P::I(a)])
        }
        "soc_channel_latest_50" | "soc_message_reactions" | "soc_channel_seq_gaps" => {
            one(vec![P::I(pick(rng, s.channels))])
        }
        "soc_channel_scrollback" => one(vec![P::I(pick(rng, s.channels)), P::I(pick(rng, s.messages))]),
        "soc_channel_deep_offset" => one(vec![P::I((rng.next() % (s.messages / 8).max(1) as u64) as i64)]),
        "soc_thread_replies_tree" => one(vec![P::I(pick(rng, s.messages))]),
        "soc_top_reacted_messages" | "soc_verified_only_authors" | "soc_channel_leaderboard"
        | "soc_msg_edit_ratio" | "soc_reaction_diversity" | "soc_top_reactors_rank" => {
            let (a, b) = span(rng, s.messages, 5_000);
            one(vec![P::I(a), P::I(b)])
        }
        "soc_unicode_search" => one(vec![P::T("%nebula-signal%".into())]),
        "soc_celebrity_vs_tier_stats" | "soc_silent_accounts" => {
            let (a, b) = span(rng, s.accounts, 500);
            one(vec![P::I(a), P::I(b)])
        }
        "soc_user_mention_like" => {
            let w = crate::WORDS[(rng.next() % crate::WORDS.len() as u64) as usize];
            one(vec![P::T(format!("@{w}%"))])
        }
        "soc_feed_celebrity_fanout" | "soc_channel_hot_general" | "soc_mod_queue_priority"
        | "soc_mod_repeat_offenders" | "soc_mod_unassigned_nulls" | "soc_msg_count_total" => one(vec![]),
        "soc_post_message_txn" => {
            let id = s.alloc_id();
            let ch = own(rng, s.channels);
            let au = pick(rng, s.accounts);
            Some(vec![
                vec![P::I(id), P::I(ch), P::I(id), P::I(au), P::T("Live chat update 🚀".into()), P::I(ts)],
                vec![P::I(ts), P::I(ch)],
            ])
        }
        "soc_toggle_reaction" => {
            let m = own(rng, s.messages);
            let a = own(rng, s.accounts);
            Some(vec![vec![P::I(m), P::I(a), P::I(ts)], vec![P::I(m), P::I(a)]])
        }
        "soc_edit_message" => one(vec![P::T("Edited message body".into()), P::I(ts), P::I(own(rng, s.messages))]),
        "soc_soft_delete_restore" => {
            let m = own(rng, s.messages);
            Some(vec![vec![P::I(ts), P::I(m)], vec![P::I(m)]])
        }
        "soc_follow_unfollow" => {
            let a = own(rng, s.accounts);
            let b = pick(rng, s.accounts);
            Some(vec![vec![P::I(a), P::I(b), P::I(ts)], vec![P::I(a), P::I(b)]])
        }
        "soc_mark_channel_read" => one(vec![P::I(own(rng, s.channels)), P::I(1)]),
        "soc_file_mod_report" => {
            let id = s.alloc_id();
            Some(vec![
                vec![P::I(id), P::I(pick(rng, s.messages)), P::I(pick(rng, s.accounts)), P::I(ts)],
                vec![P::I(id)],
            ])
        }
        "soc_update_profile_unicode" => one(vec![P::T("田中 太郎 ⚡".into()), P::T("Updated bio 🚀".into()), P::I(own(rng, s.accounts))]),
        "soc_block_and_unfollow" => {
            let a = own(rng, s.accounts);
            let b = pick(rng, s.accounts);
            Some(vec![vec![P::I(a), P::I(b), P::I(ts)], vec![P::I(a), P::I(b)], vec![P::I(a), P::I(b)]])
        }
        "soc_bulk_messages_50" => {
            let ch = own(rng, s.channels);
            let au = pick(rng, s.accounts);
            let mut v = Vec::with_capacity(50 * 6);
            for _ in 0..50 {
                let id = s.alloc_id();
                v.extend([P::I(id), P::I(ch), P::I(id), P::I(au), P::T("batch msg".into()), P::I(ts)]);
            }
            one(v)
        }

        // Domain 3
        "saas_login_email_lower" => {
            let id = pick(rng, s.saas_users);
            let tid = s.skewed_tenant(mix(id as u64 ^ 0x1002));
            one(vec![P::T(format!("user{id}@org{tid}.example.com"))])
        }
        "saas_inbox_tiny_tenant" => {
            let tid = if s.tenants > 3 { s.tenants - 1 } else { 1 };
            one(vec![P::I(tid)])
        }
        "saas_inbox_empty_tenant" => one(vec![P::I(s.empty_tenant)]),
        "saas_inbox_keyset_cursor" => one(vec![P::I(2), P::I(2), P::I(BASE_TS_US + s.issues * 30_000_000)]),
        "saas_inbox_deep_offset" => one(vec![P::I((rng.next() % (s.issues / 4).max(1) as u64) as i64)]),
        "saas_inbox_overdue" => one(vec![P::I(BASE_TS_US + s.issues * 60_000_000)]),
        "saas_inbox_label_and" => one(vec![P::I(pick(rng, s.labels)), P::I(pick(rng, s.labels))]),
        "saas_inbox_label_exclude" => one(vec![P::I(pick(rng, s.labels))]),
        "saas_issue_detail_hydrate" | "saas_attachment_blob_read" | "saas_comments_thread" => {
            one(vec![P::I(pick(rng, s.issues))])
        }
        "saas_search_title_desc" => one(vec![P::T("%zephyr%".into()), P::T("%sev0-outage-zephyr%".into())]),
        "saas_project_tree_rollup" | "saas_wide_desc_export_all" => one(vec![P::I(pick(rng, s.projects))]),
        "saas_top_contributors" => {
            let (a, b) = span(rng, s.issues, 5_000);
            let (c0, c1) = span(rng, s.issue_comments, 5_000);
            one(vec![P::I(a), P::I(b), P::I(c0), P::I(c1)])
        }
        "saas_tenant_plan_summary" => {
            let (a, b) = span(rng, s.tenants, 10);
            one(vec![P::I(a), P::I(b)])
        }
        "saas_user_permissions_exists" => one(vec![P::I(pick(rng, s.issues)), P::I(pick(rng, s.saas_users))]),
        "saas_due_nulls_first_desc" | "saas_due_nulls_last_asc" | "saas_wide_desc_sort_limit"
        | "saas_label_cooccurrence" | "saas_multi_window_issues" => {
            let (a, b) = span(rng, s.issues, 4_000);
            one(vec![P::I(a), P::I(b)])
        }
        "saas_empty_scalar_subquery" => {
            let (a, b) = span(rng, s.issues, 100);
            one(vec![P::I(a), P::I(b)])
        }
        "saas_inbox_mega_tenant" | "saas_inbox_badge_count" | "saas_inbox_unassigned"
        | "saas_search_json_sev1" | "saas_kanban_pivot" | "saas_stale_no_comments"
        | "saas_assignee_workload" | "saas_attachment_storage_by_mime" | "saas_issue_age_buckets"
        | "saas_not_in_null_trap" | "saas_high_card_group_proj" | "saas_distinct_commenters"
        | "saas_tenant_json_sso" | "saas_issues_per_month" | "saas_active_vs_deleted"
        | "saas_cross_tenant_top_orgs" | "saas_issue_count_total" => one(vec![]),
        "saas_create_issue_workflow" => {
            let iid = s.alloc_id();
            let cid = s.alloc_id();
            let proj = pick(rng, s.projects);
            let u = pick(rng, s.saas_users);
            let lbl = pick(rng, s.labels);
            Some(vec![
                vec![P::I(iid), P::I(proj), P::I(iid), P::I(u), P::I(u), P::T("New production incident".into()), P::T("Detailed incident body".into()), P::T(r#"{"severity":"sev1","votes":1}"#.into()), P::I(ts), P::I(ts)],
                vec![P::I(iid), P::I(lbl), P::I(u), P::I(ts)],
                vec![P::I(cid), P::I(iid), P::I(u), P::I(ts)],
            ])
        }
        "saas_edit_issue_md_json" => one(vec![P::T("Updated markdown body".repeat(12)), P::T(r#"{"severity":"sev2","votes":7}"#.into()), P::I(ts), P::I(own(rng, s.issues))]),
        "saas_bulk_reassign_20" => {
            let a = own(rng, s.issues);
            one(vec![P::I(pick(rng, s.saas_users)), P::I(ts), P::I(a), P::I((a + 19).min(s.issues.max(1)))])
        }
        "saas_toggle_issue_label" => {
            let i = own(rng, s.issues);
            let l = pick(rng, s.labels);
            Some(vec![vec![P::I(i), P::I(l), P::I(1), P::I(ts)], vec![P::I(i), P::I(l)]])
        }
        "saas_soft_delete_restore" => {
            let a = own(rng, s.issues);
            let b = (a + 4).min(s.issues.max(1));
            Some(vec![vec![P::I(ts), P::I(a), P::I(b)], vec![P::I(a), P::I(b)]])
        }
        "saas_upload_replace_blob" => {
            let id = s.alloc_id();
            let iid = own(rng, s.issues);
            Some(vec![
                vec![P::I(id), P::I(iid), P::B(vec![0xABu8; 16384])],
                vec![P::B(vec![0xCDu8; 256]), P::I(id)],
                vec![P::B(vec![0xEFu8; 16384]), P::I(id)],
                vec![P::I(id)],
            ])
        }
        "saas_add_comment_bump" => {
            let cid = s.alloc_id();
            let iid = own(rng, s.issues);
            Some(vec![
                vec![P::I(cid), P::I(iid), P::I(pick(rng, s.saas_users)), P::T("Following up".into()), P::I(ts)],
                vec![P::I(ts), P::I(iid)],
            ])
        }
        "saas_archive_project_subq" => one(vec![P::I(own(rng, s.projects))]),
        "saas_unique_email_rollback" => {
            let id = s.alloc_id();
            Some(vec![vec![P::I(id), P::T(format!("temp-{id}@org1.example.com")), P::Z(true), P::I(ts)], vec![P::I(id)]])
        }
        "saas_delete_issue_bundle" => {
            let id = s.alloc_id();
            Some(vec![
                vec![P::I(id), P::I(id), P::T(r#"{"severity":"sev4"}"#.into()), P::I(ts), P::I(ts)],
                vec![P::I(id), P::I(ts)],
                vec![P::I(id)],
                vec![P::I(id)],
            ])
        }

        // Domain 4
        "com_sku_point_lookup" => one(vec![P::T(format!("SKU-{:07}", pick(rng, s.products)))]),
        "com_category_tree_browse" => one(vec![P::I(1 + (rng.next() % 8.min(s.categories.max(1)) as u64) as i64)]),
        "com_faceted_catalog" => {
            let colors = ["black", "silver", "blue", "red", "white"];
            one(vec![P::I(500), P::I(50_000), P::T(colors[(rng.next() % colors.len() as u64) as usize].into())])
        }
        "com_also_bought_self_join" => one(vec![P::I(1 + (rng.next() % 20.min(s.products.max(1)) as u64) as i64)]),
        "com_customer_order_history" => one(vec![P::I(pick(rng, s.customers))]),
        "com_latest_order_per_cust" | "com_customer_ltv_aov" | "com_spend_deciles_ntile" => {
            let (a, b) = span(rng, s.customers, 500);
            one(vec![P::I(a), P::I(b)])
        }
        "com_asof_fx_conversion" => {
            let (a, b) = span(rng, s.orders, 100);
            one(vec![P::I(a), P::I(b)])
        }
        "com_ledger_running_balance" | "com_ledger_account_stmt" => {
            one(vec![P::I(pick(rng, s.ledger_accounts))])
        }
        "com_top5_skus_per_cat" | "com_json_brand_rollup" => {
            let (a, b) = span(rng, s.order_items, 5_000);
            one(vec![P::I(a), P::I(b)])
        }
        "com_order_full_receipt" => one(vec![P::I(pick(rng, s.orders))]),
        "com_q1_not_q2_buyers" | "com_repeat_buyers_intersect" => {
            let half = (s.orders / 2).max(1);
            one(vec![P::I(1), P::I(half.min(3_000)), P::I(half + 1), P::I((half + 3_000).min(s.orders.max(1)))])
        }
        "com_high_value_orders_having" | "com_basket_size_dist" => {
            let (a, b) = span(rng, s.orders, 2_000);
            one(vec![P::I(a), P::I(b)])
        }
        "com_facet_sidebar_counts" | "com_ledger_trial_balance" | "com_unreconciled_orders"
        | "com_duplicate_payments" | "com_monthly_revenue_mom" | "com_rolling_7d_revenue"
        | "com_revenue_by_country_tier" | "com_low_stock_alert" | "com_refund_rate_by_provider"
        | "com_unpurchased_products" | "com_price_float_vs_cents" | "com_shipped_sla_outliers"
        | "com_ledger_debit_credit_split" | "com_orders_count_total" | "com_multi_currency_summary"
        | "com_vip_customers_cte" | "com_hot_sku_velocity" => one(vec![]),
        "com_checkout_acid_txn" => {
            let pid = 1 + (rng.next() % 10.min(s.products.max(1)) as u64) as i64;
            let oid = s.alloc_id();
            let oiid = s.alloc_id();
            let payid = s.alloc_id();
            let ld = s.alloc_id();
            let lc = s.alloc_id();
            Some(vec![
                vec![P::I(ts), P::I(pid)],
                vec![P::I(oid), P::I(pick(rng, s.customers)), P::I(ts)],
                vec![P::I(oiid), P::I(oid), P::I(pid)],
                vec![P::I(payid), P::I(oid), P::T(format!("idem-{payid}")), P::I(ts)],
                vec![P::I(ld), P::I(oid), P::I(ts), P::I(lc), P::I(oid), P::I(ts)],
            ])
        }
        "com_checkout_savepoint_abort" => {
            let pid = own(rng, s.products);
            Some(vec![vec![], vec![P::I(pid)], vec![], vec![]])
        }
        "com_refund_restock_txn" => {
            let pid = own(rng, s.products);
            let oid = pick(rng, s.orders);
            let ld = s.alloc_id();
            let lc = s.alloc_id();
            Some(vec![
                vec![P::I(ts), P::I(pid)],
                vec![P::I(ld), P::I(oid), P::I(ts), P::I(lc), P::I(oid), P::I(ts)],
            ])
        }
        "com_webhook_idempotent_upsert" => {
            let oid = own(rng, s.orders);
            one(vec![P::I(s.alloc_id()), P::I(oid), P::T(format!("idem-order-{oid}")), P::I(ts)])
        }
        "com_update_product_price" => {
            let cents = 999 + (rng.next() % 10_000) as i64;
            one(vec![P::I(cents), P::F(cents as f64 / 100.0), P::I(ts), P::I(own(rng, s.products))])
        }
        "com_restock_hot_sku" => one(vec![P::I(ts), P::I(1 + (rng.next() % 10.min(s.products.max(1)) as u64) as i64)]),
        "com_mark_order_shipped" => one(vec![P::I(ts), P::I(own(rng, s.orders))]),
        "com_update_product_json" => one(vec![P::T(r#"{"brand":"updated","color":"black","weight_g":420}"#.into()), P::I(ts), P::I(own(rng, s.products))]),
        "com_bulk_orders_25" => {
            let cust = pick(rng, s.customers);
            let mut v = Vec::with_capacity(25 * 3);
            for _ in 0..25 {
                v.extend([P::I(s.alloc_id()), P::I(cust), P::I(ts)]);
            }
            one(v)
        }
        "com_update_customer_tier" => one(vec![P::I(own(rng, s.customers))]),
        "com_delete_cancel_order" => {
            let oid = s.alloc_id();
            let oiid = s.alloc_id();
            Some(vec![
                vec![P::I(oid), P::I(pick(rng, s.customers)), P::I(ts)],
                vec![P::I(oiid), P::I(oid)],
                vec![P::I(oid)],
                vec![P::I(oid)],
            ])
        }
        "com_fx_rate_append" => one(vec![P::I(s.alloc_id()), P::I(ts)]),
        "com_ledger_transfer_txn" => {
            let ld = s.alloc_id();
            let lc = s.alloc_id();
            let a1 = 1 + (rng.next() % 50) as i64;
            let a2 = 51 + (rng.next() % 50) as i64;
            let amt = 500 + (rng.next() % 5000) as i64;
            one(vec![P::I(ld), P::I(a1), P::I(amt), P::I(ts), P::I(lc), P::I(a2), P::I(-amt), P::I(ts)])
        }
        "com_bulk_price_adjust_cat" => {
            let a = own(rng, s.products);
            one(vec![P::I(ts), P::I(a), P::I((a + 9).min(s.products.max(1)))])
        }
        "com_cart_cleanup_txn" => {
            let id = s.alloc_id();
            Some(vec![vec![P::I(id), P::I(own(rng, s.orders))], vec![P::I(id)]])
        }
        _ => None,
    }
}
