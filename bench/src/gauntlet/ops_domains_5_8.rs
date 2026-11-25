//! Named real-world end-user operations for Domains 5–10 (180 ops -> 360 total with Domains 1–4):
//! - Domain 5: Observability, Distributed Tracing & Experimentation (40 ops)
//! - Domain 6: CI/CD Fleet & Test-Flake Intelligence (35 ops)
//! - Domain 7 & 8: Cluster Bookings, Temporal Overlaps & Wiki Knowledge Graph (35 ops)
//! - Domain 9: Cross-Domain Messy Analyst / BI SQL, 6–8-Table Joins, Funnels & ETL (40 ops)
//! - Domain 10: DBA, Views, Triggers, Temp Tables, Live DDL & `pg_catalog` / OID Durability (30 ops)

use crate::Rng;
use crate::suite::{Dialect, Kind, P, mix};

use super::loader::{BASE_TS_US, GauntletShape};
use super::ops_domains_1_4::GauntletOp;
use super::schema::{json_i64, json_ph, json_str, month_bucket, string_agg};

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

const fn ddl(domain: &'static str, name: &'static str, what: &'static str) -> GauntletOp {
    GauntletOp {
        name,
        domain,
        kind: Kind::Ddl,
        what,
    }
}

pub const OPS_5_10: &[GauntletOp] = &[
    // =========================================================================
    // Domain 5: Observability, Distributed Tracing & Experimentation (40 ops)
    // =========================================================================
    r(
        "observability",
        "obs_trace_waterfall_rec",
        "WITH RECURSIVE full distributed trace waterfall from root span",
    ),
    r(
        "observability",
        "obs_trace_spans_ordered",
        "all spans in a trace joined to service name ordered by start time",
    ),
    r(
        "observability",
        "obs_svc_latency_summary",
        "avg/min/max/count latency per service & op_name in time slice",
    ),
    r(
        "observability",
        "obs_svc_error_rate",
        "5xx and 429 error rates grouped by service & tier",
    ),
    r(
        "observability",
        "obs_slow_outliers_top30",
        "top 30 slowest spans > 500ms with service & JSON tags",
    ),
    r(
        "observability",
        "obs_canary_vs_baseline",
        "JSON canary=true vs baseline latency & error comparison",
    ),
    r(
        "observability",
        "obs_region_latency_json",
        "latency breakdown by JSON region tag across services",
    ),
    r(
        "observability",
        "obs_rolling_30_anomaly",
        "rolling 30-span moving avg latency per service via ROWS BETWEEN",
    ),
    r(
        "observability",
        "obs_multi_window_spans",
        "two distinct WINDOW specs (by service & by trace) in one SELECT",
    ),
    r(
        "observability",
        "obs_span_percent_rank",
        "PERCENT_RANK & NTILE(100) latency percentiles per service",
    ),
    r(
        "observability",
        "obs_orphan_spans_anti",
        "spans whose parent_span_id is non-NULL but missing (LEFT JOIN IS NULL)",
    ),
    r(
        "observability",
        "obs_trace_total_duration",
        "total span count, error count, and sum duration per trace",
    ),
    r(
        "observability",
        "obs_deepest_traces",
        "traces with >= 4 spans ordered by total duration DESC",
    ),
    r(
        "observability",
        "obs_team_tier_rollup",
        "service team x tier error & latency rollup",
    ),
    r(
        "observability",
        "obs_critical_path_span",
        "self-join finding child spans slower than their parent span",
    ),
    r(
        "observability",
        "obs_op_latency_buckets",
        "CASE latency histogram (<1ms, 1-10ms, 10-100ms, 100ms+)",
    ),
    r(
        "observability",
        "obs_exp_conversion_24h",
        "non-equi temporal join: conversions within 24h after exposure",
    ),
    r(
        "observability",
        "obs_exp_variant_lift",
        "experiment control vs treatment conversion rate & avg value",
    ),
    r(
        "observability",
        "obs_exp_srm_check",
        "sample ratio mismatch check: distinct accounts per variant",
    ),
    r(
        "observability",
        "obs_exp_active_summary",
        "running experiments joined to event volume and revenue",
    ),
    r(
        "observability",
        "obs_exp_first_exposure",
        "first exposure timestamp per account via ROW_NUMBER() = 1",
    ),
    r(
        "observability",
        "obs_exp_multi_touch",
        "accounts exposed to >= 2 running experiments (GROUP BY HAVING)",
    ),
    r(
        "observability",
        "obs_exp_unconverted_anti",
        "exposed accounts with zero conversion event (NOT EXISTS)",
    ),
    r(
        "observability",
        "obs_exp_value_quantiles",
        "conversion value deciles per variant via NTILE(10)",
    ),
    r(
        "observability",
        "obs_exp_daily_trend",
        "experiment exposures vs conversions by day bucket",
    ),
    r(
        "observability",
        "obs_spans_count_total",
        "whole-table COUNT(*) of spans",
    ),
    r(
        "observability",
        "obs_exp_events_count",
        "whole-table COUNT(*) of exp_events",
    ),
    r(
        "observability",
        "obs_svc_silent_check",
        "services with zero error spans in recent slice (NOT EXISTS)",
    ),
    r(
        "observability",
        "obs_trace_fanout_rank",
        "rank services by span fanout per trace using DENSE_RANK",
    ),
    r(
        "observability",
        "obs_span_lag_interarrival",
        "inter-span arrival delta per service via LAG window",
    ),
    w(
        "observability",
        "obs_ingest_span_single",
        "insert single telemetry span with JSON tags",
    ),
    w(
        "observability",
        "obs_ingest_spans_bulk_50",
        "bulk ingest 50 distributed tracing spans in one statement",
    ),
    w(
        "observability",
        "obs_ingest_exp_event",
        "record experiment exposure or conversion event",
    ),
    w(
        "observability",
        "obs_bulk_exp_events_50",
        "bulk ingest 50 experiment events in one statement",
    ),
    w(
        "observability",
        "obs_close_experiment",
        "transition experiment status and verify rollback/restore",
    ),
    w(
        "observability",
        "obs_update_span_tags",
        "patch tags_json on a span",
    ),
    w(
        "observability",
        "obs_prune_old_spans_reins",
        "insert & purge a batch of synthetic spans in 1 txn",
    ),
    w(
        "observability",
        "obs_register_service_tmp",
        "insert temporary microservice and remove it",
    ),
    w(
        "observability",
        "obs_trace_pair_txn",
        "insert parent + child span pair atomically",
    ),
    w(
        "observability",
        "obs_exp_upsert_toggle",
        "update experiment event value_float for correction",
    ),
    // =========================================================================
    // Domain 6: CI/CD Fleet & Test-Flake Intelligence (35 ops)
    // =========================================================================
    r(
        "cicd",
        "ci_queue_next_job",
        "priority queue peek: highest-priority oldest queued CI job",
    ),
    r(
        "cicd",
        "ci_queue_depth_by_repo",
        "queued & running job counts per repo_id",
    ),
    r(
        "cicd",
        "ci_runner_utilization",
        "jobs completed & avg runtime per runner_id",
    ),
    r(
        "cicd",
        "ci_stuck_running_jobs",
        "jobs claimed > 1h ago with finished_at IS NULL",
    ),
    r(
        "cicd",
        "ci_queue_wait_outliers",
        "top 25 jobs with longest claimed_at - created_at wait",
    ),
    r(
        "cicd",
        "ci_commit_status_rollup",
        "build pass/fail summary grouped by commit_sha",
    ),
    r(
        "cicd",
        "ci_flakiest_tests_top50",
        "tests with pass rate strictly between 10% and 90% (HAVING >= 5)",
    ),
    r(
        "cicd",
        "ci_pass_to_fail_regress",
        "pass->fail test regressions across consecutive runs via LAG",
    ),
    r(
        "cicd",
        "ci_slowest_tests_per_suite",
        "top 5 slowest tests per suite via ROW_NUMBER() OVER",
    ),
    r(
        "cicd",
        "ci_suite_pass_rate_pivot",
        "conditional pass/fail/flake/skip matrix per suite_name",
    ),
    r(
        "cicd",
        "ci_suite_duration_rank",
        "rank suites by p-high/avg/max duration using DENSE_RANK",
    ),
    r(
        "cicd",
        "ci_failing_jobs_with_tests",
        "failed CI jobs joined to their failing test names",
    ),
    r(
        "cicd",
        "ci_jobs_without_tests",
        "finished CI jobs that reported zero test_results (LEFT JOIN IS NULL)",
    ),
    r(
        "cicd",
        "ci_test_history_timeline",
        "last 50 executions of one specific test_name ordered by run_at DESC",
    ),
    r(
        "cicd",
        "ci_flaky_tests_intersect",
        "tests that both passed AND failed (INTERSECT)",
    ),
    r(
        "cicd",
        "ci_always_green_except",
        "tests that passed EXCEPT tests that ever failed",
    ),
    r(
        "cicd",
        "ci_repo_failure_trend",
        "monthly pass/fail rate per repo_id",
    ),
    r(
        "cicd",
        "ci_test_duration_delta",
        "test runtime regression vs prior run via LAG(duration_ms)",
    ),
    r(
        "cicd",
        "ci_runner_null_sort",
        "jobs sorted by runner_id ASC NULLS FIRST, priority DESC",
    ),
    r(
        "cicd",
        "ci_distinct_tests_per_job",
        "COUNT(DISTINCT test_name) and total duration per job",
    ),
    r(
        "cicd",
        "ci_high_prio_failures",
        "priority >= 8 failed jobs with aggregated failing suites",
    ),
    r(
        "cicd",
        "ci_test_results_count",
        "whole-table COUNT(*) of test_results",
    ),
    r(
        "cicd",
        "ci_jobs_count_total",
        "whole-table COUNT(*) of ci_jobs",
    ),
    r(
        "cicd",
        "ci_job_wallclock_buckets",
        "bucket job durations (<30s, 30-120s, 120s+) with CASE",
    ),
    r(
        "cicd",
        "ci_test_name_prefix_like",
        "filter test_results by suite::test prefix LIKE",
    ),
    w(
        "cicd",
        "ci_enqueue_job",
        "enqueue new CI job in queued status",
    ),
    w(
        "cicd",
        "ci_claim_and_finish_txn",
        "claim queued job for runner then mark passed with finished_at",
    ),
    w(
        "cicd",
        "ci_bulk_test_results_50",
        "bulk ingest 50 test_results rows in one statement",
    ),
    w(
        "cicd",
        "ci_requeue_stale_job",
        "reset job runner_id and claimed_at to NULL",
    ),
    w(
        "cicd",
        "ci_cancel_repo_jobs",
        "update queued jobs in a slice for a repo",
    ),
    w(
        "cicd",
        "ci_record_flake_retry",
        "insert retry test_result + update job finished_at in 1 txn",
    ),
    w(
        "cicd",
        "ci_job_savepoint_abort",
        "enqueue job then SAVEPOINT rollback bad test batch",
    ),
    w(
        "cicd",
        "ci_bump_job_priority",
        "bump priority on urgent CI jobs",
    ),
    w(
        "cicd",
        "ci_purge_temp_job_tree",
        "insert job + 5 test results then delete both cleanly",
    ),
    w(
        "cicd",
        "ci_bulk_jobs_25",
        "bulk enqueue 25 CI jobs in one statement",
    ),
    // =========================================================================
    // Domain 7 & 8: Cluster Bookings, Temporal Overlaps & Wiki Graph (35 ops)
    // =========================================================================
    r(
        "graph_sched",
        "sch_interval_overlap",
        "find bookings overlapping [start, end) on a resource",
    ),
    r(
        "graph_sched",
        "sch_double_booking_self",
        "self-join detecting overlapping confirmed bookings on same resource",
    ),
    r(
        "graph_sched",
        "sch_free_slot_gaps_lead",
        "gap-and-island free slot finder via LEAD(start_ts) - end_ts",
    ),
    r(
        "graph_sched",
        "sch_resource_utilization",
        "total booked seconds & booking count per resource_id",
    ),
    r(
        "graph_sched",
        "sch_owner_schedule",
        "upcoming non-cancelled bookings for an owner sorted by start_ts",
    ),
    r(
        "graph_sched",
        "sch_owner_conflicts",
        "self-join detecting an owner booked on two resources at once",
    ),
    r(
        "graph_sched",
        "sch_longest_bookings",
        "top 25 longest duration bookings (end_ts - start_ts)",
    ),
    r(
        "graph_sched",
        "sch_status_breakdown",
        "confirmed vs tentative vs cancelled counts per resource",
    ),
    r(
        "graph_sched",
        "sch_busy_resources_having",
        "resources with >= 5 confirmed bookings in window",
    ),
    r(
        "graph_sched",
        "sch_booking_rank_per_res",
        "rank bookings chronologically per resource via ROW_NUMBER",
    ),
    r(
        "graph_sched",
        "sch_bookings_count_total",
        "whole-table COUNT(*) of bookings",
    ),
    r(
        "graph_sched",
        "wiki_doc_by_slug",
        "load wiki document by (tenant_id, slug)",
    ),
    r(
        "graph_sched",
        "wiki_backlinks",
        "all inbound backlinks to a wiki document with source titles",
    ),
    r(
        "graph_sched",
        "wiki_outbound_links",
        "all outbound links from a wiki document with target titles",
    ),
    r(
        "graph_sched",
        "wiki_orphan_docs_anti",
        "documents with ZERO inbound links (LEFT JOIN ... IS NULL)",
    ),
    r(
        "graph_sched",
        "wiki_deadend_docs_notex",
        "documents with ZERO outbound links (NOT EXISTS)",
    ),
    r(
        "graph_sched",
        "wiki_hub_degree_top25",
        "top 25 wiki hubs by in-degree + out-degree",
    ),
    r(
        "graph_sched",
        "wiki_shortest_path_rec",
        "WITH RECURSIVE multi-hop reachability in doc_links graph",
    ),
    r(
        "graph_sched",
        "wiki_mutual_links_self",
        "bidirectional wiki links (A->B AND B->A self-join)",
    ),
    r(
        "graph_sched",
        "wiki_wide_doc_search",
        "substring search across wide 6 KiB markdown doc bodies",
    ),
    r(
        "graph_sched",
        "wiki_tenant_doc_versions",
        "version histogram and total body length per tenant",
    ),
    r(
        "graph_sched",
        "wiki_doc_link_union",
        "UNION of inbound and outbound neighbors for one document",
    ),
    r(
        "graph_sched",
        "wiki_most_updated_docs",
        "recently updated docs per tenant ranked via DENSE_RANK",
    ),
    r(
        "graph_sched",
        "wiki_docs_count_total",
        "whole-table COUNT(*) of documents",
    ),
    r(
        "graph_sched",
        "wiki_links_count_total",
        "whole-table COUNT(*) of doc_links",
    ),
    w(
        "graph_sched",
        "sch_create_booking_txn",
        "insert new confirmed booking and cancel/cleanup temp id",
    ),
    w(
        "graph_sched",
        "sch_reschedule_booking",
        "shift booking start_ts and end_ts forward",
    ),
    w(
        "graph_sched",
        "sch_cancel_booking",
        "toggle booking status cancelled <-> confirmed",
    ),
    w(
        "graph_sched",
        "sch_bulk_bookings_25",
        "bulk insert 25 cluster bookings in one statement",
    ),
    w(
        "graph_sched",
        "wiki_publish_revision",
        "increment document version, update body & updated_at",
    ),
    w(
        "graph_sched",
        "wiki_upsert_link",
        "insert doc_links edge ON CONFLICT DO UPDATE anchor_text",
    ),
    w(
        "graph_sched",
        "wiki_rewire_links_txn",
        "delete old link and insert replacement link atomically",
    ),
    w(
        "graph_sched",
        "wiki_create_doc_and_link",
        "create wide 8 KiB design doc + link + cleanup",
    ),
    w(
        "graph_sched",
        "wiki_bulk_links_25",
        "bulk insert 25 doc_links edges ON CONFLICT DO NOTHING",
    ),
    w(
        "graph_sched",
        "wiki_savepoint_edit",
        "update doc body with SAVEPOINT rollback on validation fail",
    ),
    // =========================================================================
    // Domain 9: Cross-Domain Messy Analyst / BI SQL & ETL (40 ops)
    // =========================================================================
    r(
        "bi_etl",
        "bi_6table_saas_join",
        "6-table join: tenants x projects x issues x users x labels x comments",
    ),
    r(
        "bi_etl",
        "bi_7table_agentic_saas",
        "7-table cross-domain join: tenants x workspaces x runs x turns x tools x diffs x projects",
    ),
    r(
        "bi_etl",
        "bi_8table_full_enterprise",
        "8-table enterprise mega-join across SaaS + Agentic + Wiki",
    ),
    r(
        "bi_etl",
        "bi_4step_funnel_cte",
        "4-step CTE funnel: accounts -> follows -> messages -> reactions",
    ),
    r(
        "bi_etl",
        "bi_commerce_funnel_cte",
        "4-step e-commerce funnel: customers -> orders -> items -> captured payments",
    ),
    r(
        "bi_etl",
        "bi_monthly_cohort_matrix",
        "customer cohort retention by signup month vs order month",
    ),
    r(
        "bi_etl",
        "bi_compound_union_except",
        "compound (A UNION B) EXCEPT C ORDER BY ... LIMIT ...",
    ),
    r(
        "bi_etl",
        "bi_4x_count_distinct",
        "4 distinct COUNT(DISTINCT ...) columns in one GROUP BY",
    ),
    r(
        "bi_etl",
        "bi_high_card_4col_groupby",
        "4-column high-cardinality GROUP BY with HAVING & ORDER BY",
    ),
    r(
        "bi_etl",
        "bi_unbounded_csv_export",
        "unbounded multi-column sorted export across orders + customers",
    ),
    r(
        "bi_etl",
        "bi_full_outer_reconcile",
        "FULL OUTER JOIN reconciling orders vs payments on slice",
    ),
    r(
        "bi_etl",
        "bi_cross_domain_org_score",
        "CTE joining SaaS issue volume + Agentic token burn + Wiki docs per tenant",
    ),
    r(
        "bi_etl",
        "bi_correlated_having_avg",
        "tenants whose avg issue estimate exceeds global avg (subquery in HAVING)",
    ),
    r(
        "bi_etl",
        "bi_scalar_subq_multi_col",
        "3 correlated scalar subqueries in SELECT projection per tenant",
    ),
    r(
        "bi_etl",
        "bi_exists_and_not_exists",
        "combined EXISTS + NOT EXISTS across orders, payments & ledger",
    ),
    r(
        "bi_etl",
        "bi_wide_sort_nulls_mixed",
        "wide sort with mixed ASC/DESC and NULLS FIRST/NULLS LAST",
    ),
    r(
        "bi_etl",
        "bi_nested_derived_3deep",
        "3-level nested derived tables with aggregation at each level",
    ),
    r(
        "bi_etl",
        "bi_window_running_pct",
        "cumulative revenue share % using SUM OVER / SUM OVER()",
    ),
    r(
        "bi_etl",
        "bi_window_frame_range_avg",
        "rolling 5-row centered window (2 PRECEDING AND 2 FOLLOWING)",
    ),
    r(
        "bi_etl",
        "bi_union_all_ledger_audit",
        "UNION ALL auditing both ledger_entries and token_cost_ledger balances",
    ),
    r(
        "bi_etl",
        "bi_case_nullif_coalesce",
        "complex arithmetic with COALESCE, NULLIF, ABS, ROUND, CASE",
    ),
    r(
        "bi_etl",
        "bi_in_subquery_multi",
        "WHERE id IN (SELECT ...) AND project_id IN (SELECT ...)",
    ),
    r(
        "bi_etl",
        "bi_string_concat_agg",
        "string concatenation + GROUP_CONCAT/STRING_AGG across 3-table join",
    ),
    r(
        "bi_etl",
        "bi_json_join_analytics",
        "join issues to tenants filtering on JSON fields from BOTH tables",
    ),
    r(
        "bi_etl",
        "bi_self_join_same_day",
        "self-join finding pairs of orders by same customer in window",
    ),
    r(
        "bi_etl",
        "bi_distinct_on_window",
        "deduplicated latest event per account using ROW_NUMBER() = 1",
    ),
    r(
        "bi_etl",
        "bi_cross_join_matrix",
        "small cross-join grid (tiers x statuses) LEFT JOINed to issues",
    ),
    r(
        "bi_etl",
        "bi_top_n_per_group_3way",
        "top 3 highest-spend orders per country using DENSE_RANK",
    ),
    r(
        "bi_etl",
        "bi_anti_join_3way_chain",
        "3-table LEFT JOIN chain finding customers with orders but no captured payment or ledger",
    ),
    r(
        "bi_etl",
        "bi_multi_cte_diamond",
        "diamond-shaped CTE graph referenced twice in final join",
    ),
    r(
        "bi_etl",
        "bi_unicode_collation_sort",
        "ORDER BY display_name, handle across CJK/emoji/accented accounts",
    ),
    r(
        "bi_etl",
        "bi_empty_filter_agg",
        "aggregate over empty predicate returning 1 row of (0, NULL, NULL)",
    ),
    r(
        "bi_etl",
        "bi_group_by_expr_bucket",
        "GROUP BY computed arithmetic bucket (price_cents / 5000)",
    ),
    r(
        "bi_etl",
        "bi_left_join_filter_on",
        "LEFT JOIN with predicate inside ON vs WHERE semantic contrast",
    ),
    r(
        "bi_etl",
        "bi_sum_of_distinct_vals",
        "SUM(DISTINCT priority) and COUNT(DISTINCT status) per tenant",
    ),
    w(
        "bi_etl",
        "etl_insert_select_rollup",
        "CREATE + INSERT INTO ... SELECT aggregated rollup + DROP scratch",
    ),
    w(
        "bi_etl",
        "etl_ctas_snapshot",
        "CREATE TABLE AS SELECT filtered join snapshot + DROP",
    ),
    w(
        "bi_etl",
        "etl_batched_purge_subq",
        "insert 20 staging rows then DELETE WHERE id IN (SELECT ...)",
    ),
    w(
        "bi_etl",
        "etl_update_from_agg",
        "update denormalized channel.message_count from subquery COUNT(*)",
    ),
    w(
        "bi_etl",
        "etl_bulk_upsert_25",
        "upsert 25 agent_memories rows ON CONFLICT(id) DO UPDATE",
    ),
    // =========================================================================
    // Domain 10: DBA, Views, Triggers, Temp Tables, Live DDL & Catalog (30 ops)
    // =========================================================================
    ddl(
        "dba_ddl",
        "ddl_create_drop_table",
        "CREATE TABLE + insert 5 rows + SELECT + DROP TABLE",
    ),
    ddl(
        "dba_ddl",
        "ddl_create_single_idx",
        "CREATE INDEX on scratch table + query + DROP INDEX",
    ),
    ddl(
        "dba_ddl",
        "ddl_create_comp_desc_idx",
        "CREATE composite mixed ASC/DESC index + ordered read + DROP",
    ),
    ddl(
        "dba_ddl",
        "ddl_create_unique_idx",
        "CREATE UNIQUE INDEX + lookup + DROP",
    ),
    ddl(
        "dba_ddl",
        "ddl_create_partial_idx",
        "CREATE partial index (WHERE active) + filtered read + DROP",
    ),
    ddl(
        "dba_ddl",
        "ddl_create_expr_idx",
        "CREATE expression index (LOWER(name)) + query + DROP",
    ),
    ddl(
        "dba_ddl",
        "ddl_rename_table",
        "CREATE TABLE + insert + ALTER TABLE RENAME TO + read + DROP",
    ),
    ddl(
        "dba_ddl",
        "ddl_rename_column",
        "CREATE TABLE + ALTER TABLE RENAME COLUMN + read + DROP",
    ),
    ddl(
        "dba_ddl",
        "ddl_add_column_backfill",
        "CREATE TABLE + ALTER TABLE ADD COLUMN + UPDATE backfill + DROP",
    ),
    ddl(
        "dba_ddl",
        "ddl_drop_column",
        "CREATE TABLE + ALTER TABLE DROP COLUMN + read + DROP",
    ),
    ddl(
        "dba_ddl",
        "ddl_view_create_join_drop",
        "CREATE VIEW + join view against base table + DROP VIEW",
    ),
    ddl(
        "dba_ddl",
        "ddl_nested_views",
        "CREATE VIEW v2 over VIEW v1 + aggregate query + DROP both",
    ),
    ddl(
        "dba_ddl",
        "ddl_trigger_after_insert",
        "CREATE AFTER INSERT TRIGGER auditing writes + verify audit + DROP",
    ),
    ddl(
        "dba_ddl",
        "ddl_trigger_after_update",
        "CREATE AFTER UPDATE TRIGGER tracking changes + verify + DROP",
    ),
    ddl(
        "dba_ddl",
        "ddl_trigger_after_delete",
        "CREATE AFTER DELETE TRIGGER logging deletions + verify + DROP",
    ),
    ddl(
        "dba_ddl",
        "ddl_temp_table_join",
        "CREATE TEMP TABLE + populate + join to issues + DROP",
    ),
    ddl(
        "dba_ddl",
        "ddl_analyze_stats",
        "run ANALYZE to refresh optimizer statistics",
    ),
    ddl(
        "dba_ddl",
        "ddl_reindex_table",
        "REINDEX on scratch table with secondary indexes",
    ),
    ddl(
        "dba_ddl",
        "ddl_wal_checkpoint_pass",
        "PRAGMA wal_checkpoint(PASSIVE)",
    ),
    ddl("dba_ddl", "ddl_integrity_check", "PRAGMA integrity_check"),
    r(
        "dba_ddl",
        "cat_list_tables",
        "catalog: list all user tables (sqlite_schema / pg_class)",
    ),
    r(
        "dba_ddl",
        "cat_list_indexes",
        "catalog: list all indexes across all 43 tables",
    ),
    r(
        "dba_ddl",
        "cat_table_columns",
        "catalog: inspect columns & types of issues table",
    ),
    r(
        "dba_ddl",
        "cat_pg_namespaces",
        "catalog: inspect pg_namespace / schema metadata",
    ),
    r(
        "dba_ddl",
        "cat_pg_types",
        "catalog: inspect pg_type built-in type registry",
    ),
    r(
        "dba_ddl",
        "cat_pg_class_oids",
        "catalog: inspect pg_class OIDs & relkind",
    ),
    r(
        "dba_ddl",
        "cat_pg_attributes_join",
        "catalog: join pg_class + pg_attribute + pg_type",
    ),
    ddl(
        "dba_ddl",
        "cat_oid_rename_stability",
        "create table -> rename -> verify pg_class OID / table exists -> drop",
    ),
    ddl(
        "dba_ddl",
        "ddl_if_not_exists_idemp",
        "idempotent CREATE TABLE/INDEX IF NOT EXISTS + DROP IF EXISTS",
    ),
    ddl(
        "dba_ddl",
        "ddl_wide_schema_20cols",
        "CREATE TABLE with 20 mixed-type columns + insert + read + DROP",
    ),
];

pub fn statements_5_10(op: &str, d: Dialect) -> Option<Vec<String>> {
    let jp = json_ph(d);
    let one = |s: String| Some(vec![s]);
    let is_pg_cat = matches!(d, Dialect::AmphoraPostgres | Dialect::Postgres);
    let is_ext_pg = d == Dialect::Postgres;
    let i_ty = if is_pg_cat { "BIGINT" } else { "INTEGER" };
    let b_ty = if is_pg_cat { "BOOLEAN" } else { "INTEGER" };
    let pk_ty = if is_ext_pg {
        "BIGINT PRIMARY KEY"
    } else {
        "INTEGER PRIMARY KEY"
    };

    let s = match op {
        // ======================= DOMAIN 5: OBSERVABILITY =====================
        "obs_trace_waterfall_rec" => "WITH RECURSIVE wf(id, parent_span_id, service_id, op_name, duration_us, depth) AS (SELECT id, parent_span_id, service_id, op_name, duration_us, 0 FROM spans WHERE trace_id = ? AND parent_span_id IS NULL UNION ALL SELECT s.id, s.parent_span_id, s.service_id, s.op_name, s.duration_us, wf.depth + 1 FROM spans s JOIN wf ON s.parent_span_id = wf.id WHERE wf.depth < 12) SELECT wf.id, sv.name, wf.op_name, wf.duration_us, wf.depth FROM wf JOIN services sv ON sv.id = wf.service_id ORDER BY wf.depth, wf.id LIMIT 60".into(),
        "obs_trace_spans_ordered" => "SELECT s.id, sv.name, s.op_name, s.status_code, s.duration_us, s.started_at FROM spans s JOIN services sv ON sv.id = s.service_id WHERE s.trace_id = ? ORDER BY s.started_at ASC, s.id ASC".into(),
        "obs_svc_latency_summary" => "SELECT sv.name, s.op_name, COUNT(*), AVG(s.duration_us), MIN(s.duration_us), MAX(s.duration_us) FROM spans s JOIN services sv ON sv.id = s.service_id WHERE s.id BETWEEN ? AND ? GROUP BY sv.name, s.op_name ORDER BY AVG(s.duration_us) DESC, sv.name ASC LIMIT 30".into(),
        "obs_svc_error_rate" => "SELECT sv.name, sv.tier, COUNT(*), SUM(CASE WHEN s.status_code >= 500 THEN 1 ELSE 0 END), SUM(CASE WHEN s.status_code = 429 THEN 1 ELSE 0 END) FROM spans s JOIN services sv ON sv.id = s.service_id WHERE s.id BETWEEN ? AND ? GROUP BY sv.name, sv.tier ORDER BY SUM(CASE WHEN s.status_code >= 500 THEN 1 ELSE 0 END) DESC, sv.name ASC LIMIT 25".into(),
        "obs_slow_outliers_top30" => "SELECT s.id, s.trace_id, sv.name, s.op_name, s.duration_us, s.tags_json FROM spans s JOIN services sv ON sv.id = s.service_id WHERE s.duration_us > 500000 ORDER BY s.duration_us DESC, s.id ASC LIMIT 30".into(),
        "obs_canary_vs_baseline" => format!("SELECT {} AS canary, COUNT(*), AVG(duration_us), SUM(CASE WHEN status_code >= 500 THEN 1 ELSE 0 END) FROM spans WHERE id BETWEEN ? AND ? GROUP BY canary ORDER BY canary", json_str(d, "tags_json", "canary")),
        "obs_region_latency_json" => format!("SELECT {} AS region, COUNT(*), AVG(duration_us), MAX(duration_us) FROM spans WHERE id BETWEEN ? AND ? GROUP BY region ORDER BY AVG(duration_us) DESC, region ASC", json_str(d, "tags_json", "region")),
        "obs_rolling_30_anomaly" => "SELECT id, service_id, duration_us, AVG(duration_us) OVER (PARTITION BY service_id ORDER BY started_at, id ROWS BETWEEN 29 PRECEDING AND CURRENT ROW) AS rolling_avg FROM spans WHERE service_id = ? ORDER BY started_at DESC, id DESC LIMIT 100".into(),
        "obs_multi_window_spans" => "SELECT id, service_id, trace_id, duration_us, ROW_NUMBER() OVER (PARTITION BY service_id ORDER BY duration_us DESC, id ASC), SUM(duration_us) OVER (PARTITION BY trace_id) FROM spans WHERE id BETWEEN ? AND ?".into(),
        "obs_span_percent_rank" => "SELECT id, service_id, duration_us, NTILE(100) OVER (PARTITION BY service_id ORDER BY duration_us ASC, id ASC) AS pct FROM spans WHERE service_id = ? ORDER BY pct DESC, id ASC LIMIT 50".into(),
        "obs_orphan_spans_anti" => "SELECT s.id, s.trace_id, s.parent_span_id FROM spans s LEFT JOIN spans p ON p.id = s.parent_span_id WHERE s.id BETWEEN ? AND ? AND s.parent_span_id IS NOT NULL AND p.id IS NULL ORDER BY s.id LIMIT 30".into(),
        "obs_trace_total_duration" => "SELECT trace_id, COUNT(*), SUM(duration_us), SUM(CASE WHEN status_code >= 500 THEN 1 ELSE 0 END) FROM spans WHERE trace_id BETWEEN ? AND ? GROUP BY trace_id ORDER BY SUM(duration_us) DESC, trace_id ASC LIMIT 25".into(),
        "obs_deepest_traces" => "SELECT trace_id, COUNT(*) AS cnt, SUM(duration_us) AS total_us FROM spans WHERE trace_id BETWEEN ? AND ? GROUP BY trace_id HAVING COUNT(*) >= 4 ORDER BY total_us DESC, trace_id ASC LIMIT 25".into(),
        "obs_team_tier_rollup" => "SELECT sv.team_name, sv.tier, COUNT(s.id), AVG(s.duration_us) FROM services sv LEFT JOIN spans s ON s.service_id = sv.id AND s.id BETWEEN ? AND ? GROUP BY sv.team_name, sv.tier ORDER BY sv.team_name, sv.tier".into(),
        "obs_critical_path_span" => "SELECT c.id, p.id, c.duration_us, p.duration_us FROM spans c JOIN spans p ON p.id = c.parent_span_id WHERE c.id BETWEEN ? AND ? AND c.duration_us > p.duration_us ORDER BY c.duration_us DESC, c.id ASC LIMIT 25".into(),
        "obs_op_latency_buckets" => "SELECT op_name, SUM(CASE WHEN duration_us < 1000 THEN 1 ELSE 0 END), SUM(CASE WHEN duration_us >= 1000 AND duration_us < 10000 THEN 1 ELSE 0 END), SUM(CASE WHEN duration_us >= 10000 AND duration_us < 100000 THEN 1 ELSE 0 END), SUM(CASE WHEN duration_us >= 100000 THEN 1 ELSE 0 END) FROM spans WHERE id BETWEEN ? AND ? GROUP BY op_name ORDER BY op_name".into(),
        "obs_exp_conversion_24h" => "SELECT e.variant, COUNT(DISTINCT e.account_id) AS exposed, COUNT(DISTINCT c.account_id) AS converted, COALESCE(SUM(c.value_float), 0.0) FROM exp_events e LEFT JOIN exp_events c ON c.experiment_id = e.experiment_id AND c.account_id = e.account_id AND c.event_kind = 'conversion' AND c.ts >= e.ts AND c.ts <= e.ts + 86400000000 WHERE e.experiment_id = ? AND e.event_kind = 'exposure' AND e.id BETWEEN ? AND ? GROUP BY e.variant ORDER BY e.variant".into(),
        "obs_exp_variant_lift" => "SELECT experiment_id, variant, SUM(CASE WHEN event_kind = 'exposure' THEN 1 ELSE 0 END), SUM(CASE WHEN event_kind = 'conversion' THEN 1 ELSE 0 END), AVG(CASE WHEN event_kind = 'conversion' THEN value_float ELSE NULL END) FROM exp_events WHERE experiment_id = ? GROUP BY experiment_id, variant ORDER BY variant".into(),
        "obs_exp_srm_check" => "SELECT experiment_id, variant, COUNT(DISTINCT account_id) FROM exp_events WHERE id BETWEEN ? AND ? GROUP BY experiment_id, variant ORDER BY experiment_id, variant".into(),
        "obs_exp_active_summary" => "SELECT ex.id, ex.key_name, COUNT(ev.id), COALESCE(SUM(ev.value_float), 0.0) FROM experiments ex LEFT JOIN exp_events ev ON ev.experiment_id = ex.id WHERE ex.status = 'running' GROUP BY ex.id, ex.key_name ORDER BY COUNT(ev.id) DESC, ex.id ASC".into(),
        "obs_exp_first_exposure" => "SELECT account_id, experiment_id, variant, ts FROM (SELECT account_id, experiment_id, variant, ts, ROW_NUMBER() OVER (PARTITION BY account_id ORDER BY ts ASC, id ASC) AS rn FROM exp_events WHERE event_kind = 'exposure' AND id BETWEEN ? AND ?) t WHERE rn = 1 ORDER BY account_id LIMIT 40".into(),
        "obs_exp_multi_touch" => "SELECT account_id, COUNT(DISTINCT experiment_id) AS exps FROM exp_events WHERE id BETWEEN ? AND ? GROUP BY account_id HAVING COUNT(DISTINCT experiment_id) >= 2 ORDER BY exps DESC, account_id ASC LIMIT 30".into(),
        "obs_exp_unconverted_anti" => "SELECT e.account_id, e.variant FROM exp_events e WHERE e.experiment_id = ? AND e.event_kind = 'exposure' AND e.id BETWEEN ? AND ? AND NOT EXISTS (SELECT 1 FROM exp_events c WHERE c.experiment_id = e.experiment_id AND c.account_id = e.account_id AND c.event_kind = 'conversion') ORDER BY e.account_id LIMIT 30".into(),
        "obs_exp_value_quantiles" => "SELECT variant, value_float, NTILE(10) OVER (PARTITION BY variant ORDER BY value_float DESC, id ASC) AS dec FROM exp_events WHERE experiment_id = ? AND event_kind = 'conversion' ORDER BY variant, dec, value_float DESC LIMIT 40".into(),
        "obs_exp_daily_trend" => "SELECT ts / 86400000000 AS day_no, event_kind, COUNT(*), SUM(value_float) FROM exp_events WHERE experiment_id = ? GROUP BY day_no, event_kind ORDER BY day_no, event_kind LIMIT 40".into(),
        "obs_spans_count_total" => "SELECT COUNT(*) FROM spans".into(),
        "obs_exp_events_count" => "SELECT COUNT(*) FROM exp_events".into(),
        "obs_svc_silent_check" => "SELECT sv.id, sv.name FROM services sv WHERE NOT EXISTS (SELECT 1 FROM spans s WHERE s.service_id = sv.id AND s.status_code >= 500 AND s.id BETWEEN ? AND ?) ORDER BY sv.id".into(),
        "obs_trace_fanout_rank" => "SELECT trace_id, service_id, cnt, DENSE_RANK() OVER (PARTITION BY trace_id ORDER BY cnt DESC, service_id ASC) AS rk FROM (SELECT trace_id, service_id, COUNT(*) AS cnt FROM spans WHERE trace_id BETWEEN ? AND ? GROUP BY trace_id, service_id) t ORDER BY trace_id, rk, service_id LIMIT 40".into(),
        "obs_span_lag_interarrival" => "SELECT id, service_id, started_at - COALESCE(LAG(started_at) OVER (PARTITION BY service_id ORDER BY started_at, id), started_at) AS delta_us FROM spans WHERE service_id = ? ORDER BY started_at DESC, id DESC LIMIT 50".into(),
        "obs_ingest_span_single" => format!("INSERT INTO spans (id, trace_id, parent_span_id, service_id, op_name, status_code, duration_us, tags_json, started_at) VALUES (?, ?, NULL, ?, 'http.get', 200, ?, {jp}, ?)"),
        "obs_ingest_spans_bulk_50" => {
            let row = format!("(?, ?, NULL, ?, 'rpc.call', 200, ?, {jp}, ?)");
            format!("INSERT INTO spans (id, trace_id, parent_span_id, service_id, op_name, status_code, duration_us, tags_json, started_at) VALUES {}", vec![row; 50].join(", "))
        }
        "obs_ingest_exp_event" => "INSERT INTO exp_events (id, experiment_id, account_id, variant, event_kind, value_float, ts) VALUES (?, ?, ?, 'treatment', 'conversion', ?, ?)".into(),
        "obs_bulk_exp_events_50" => format!("INSERT INTO exp_events (id, experiment_id, account_id, variant, event_kind, value_float, ts) VALUES {}", vec!["(?, ?, ?, 'control', 'exposure', 0.0, ?)"; 50].join(", ")),
        "obs_close_experiment" => return Some(vec![
            "UPDATE experiments SET status = 'ended' WHERE id = ?".into(),
            "UPDATE experiments SET status = 'running' WHERE id = ?".into(),
        ]),
        "obs_update_span_tags" => format!("UPDATE spans SET tags_json = {jp} WHERE id = ?"),
        "obs_prune_old_spans_reins" => return Some(vec![
            format!("INSERT INTO spans (id, trace_id, parent_span_id, service_id, op_name, status_code, duration_us, tags_json, started_at) VALUES (?, 1, NULL, 1, 'tmp', 200, 100, {jp}, ?)"),
            "DELETE FROM spans WHERE id = ?".into(),
        ]),
        "obs_register_service_tmp" => return Some(vec![
            "INSERT INTO services (id, name, team_name, tier) VALUES (?, ?, 'tmp-team', 3)".into(),
            "DELETE FROM services WHERE id = ?".into(),
        ]),
        "obs_trace_pair_txn" => return Some(vec![
            format!("INSERT INTO spans (id, trace_id, parent_span_id, service_id, op_name, status_code, duration_us, tags_json, started_at) VALUES (?, ?, NULL, 1, 'root', 200, 500, {jp}, ?)"),
            format!("INSERT INTO spans (id, trace_id, parent_span_id, service_id, op_name, status_code, duration_us, tags_json, started_at) VALUES (?, ?, ?, 2, 'child', 200, 200, {jp}, ?)"),
        ]),
        "obs_exp_upsert_toggle" => "UPDATE exp_events SET value_float = ? WHERE id = ?".into(),

        // ======================= DOMAIN 6: CI/CD =============================
        "ci_queue_next_job" => "SELECT id, repo_id, commit_sha, priority, created_at FROM ci_jobs WHERE status = 'queued' ORDER BY priority DESC, created_at ASC, id ASC LIMIT 1".into(),
        "ci_queue_depth_by_repo" => "SELECT repo_id, SUM(CASE WHEN status = 'queued' THEN 1 ELSE 0 END), SUM(CASE WHEN status = 'running' THEN 1 ELSE 0 END), COUNT(*) FROM ci_jobs GROUP BY repo_id ORDER BY repo_id".into(),
        "ci_runner_utilization" => "SELECT runner_id, COUNT(*), AVG(finished_at - claimed_at) FROM ci_jobs WHERE runner_id IS NOT NULL AND finished_at IS NOT NULL GROUP BY runner_id ORDER BY COUNT(*) DESC, runner_id ASC LIMIT 30".into(),
        "ci_stuck_running_jobs" => "SELECT id, repo_id, runner_id, claimed_at FROM ci_jobs WHERE status = 'running' AND finished_at IS NULL AND claimed_at < ? ORDER BY claimed_at ASC, id ASC LIMIT 30".into(),
        "ci_queue_wait_outliers" => "SELECT id, repo_id, priority, claimed_at - created_at AS wait_us FROM ci_jobs WHERE claimed_at IS NOT NULL ORDER BY wait_us DESC, id ASC LIMIT 25".into(),
        "ci_commit_status_rollup" => "SELECT commit_sha, COUNT(*), SUM(CASE WHEN status = 'passed' THEN 1 ELSE 0 END), SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END) FROM ci_jobs WHERE id BETWEEN ? AND ? GROUP BY commit_sha ORDER BY COUNT(*) DESC, commit_sha ASC LIMIT 25".into(),
        "ci_flakiest_tests_top50" => "SELECT test_name, COUNT(*) AS runs, SUM(CASE WHEN outcome = 'pass' THEN 1 ELSE 0 END) AS passes, SUM(CASE WHEN outcome = 'fail' OR outcome = 'flake' THEN 1 ELSE 0 END) AS fails FROM test_results WHERE id BETWEEN ? AND ? GROUP BY test_name HAVING COUNT(*) >= 5 AND SUM(CASE WHEN outcome = 'pass' THEN 1 ELSE 0 END) > 0 AND SUM(CASE WHEN outcome = 'fail' OR outcome = 'flake' THEN 1 ELSE 0 END) > 0 ORDER BY fails DESC, test_name ASC LIMIT 50".into(),
        "ci_pass_to_fail_regress" => "WITH h AS (SELECT test_name, run_at, outcome, LAG(outcome) OVER (PARTITION BY test_name ORDER BY run_at, id) AS prev_outcome FROM test_results WHERE id BETWEEN ? AND ?) SELECT test_name, run_at, prev_outcome, outcome FROM h WHERE prev_outcome = 'pass' AND outcome = 'fail' ORDER BY run_at DESC, test_name ASC LIMIT 30".into(),
        "ci_slowest_tests_per_suite" => "WITH avg_t AS (SELECT suite_name, test_name, AVG(duration_ms) AS avg_ms FROM test_results WHERE id BETWEEN ? AND ? GROUP BY suite_name, test_name), rk AS (SELECT suite_name, test_name, avg_ms, ROW_NUMBER() OVER (PARTITION BY suite_name ORDER BY avg_ms DESC, test_name ASC) AS rn FROM avg_t) SELECT suite_name, test_name, avg_ms, rn FROM rk WHERE rn <= 5 ORDER BY suite_name, rn".into(),
        "ci_suite_pass_rate_pivot" => "SELECT suite_name, SUM(CASE WHEN outcome = 'pass' THEN 1 ELSE 0 END), SUM(CASE WHEN outcome = 'fail' THEN 1 ELSE 0 END), SUM(CASE WHEN outcome = 'flake' THEN 1 ELSE 0 END), SUM(CASE WHEN outcome = 'skip' THEN 1 ELSE 0 END) FROM test_results GROUP BY suite_name ORDER BY suite_name".into(),
        "ci_suite_duration_rank" => "SELECT suite_name, avg_ms, max_ms, DENSE_RANK() OVER (ORDER BY avg_ms DESC) FROM (SELECT suite_name, AVG(duration_ms) AS avg_ms, MAX(duration_ms) AS max_ms FROM test_results WHERE id BETWEEN ? AND ? GROUP BY suite_name) s ORDER BY avg_ms DESC, suite_name ASC".into(),
        "ci_failing_jobs_with_tests" => "SELECT j.id, j.repo_id, j.commit_sha, t.suite_name, t.test_name FROM ci_jobs j JOIN test_results t ON t.job_id = j.id AND t.outcome = 'fail' WHERE j.status = 'failed' ORDER BY j.id DESC, t.id ASC LIMIT 30".into(),
        "ci_jobs_without_tests" => "SELECT j.id, j.repo_id, j.status FROM ci_jobs j LEFT JOIN test_results t ON t.job_id = j.id WHERE j.id BETWEEN ? AND ? AND t.id IS NULL ORDER BY j.id LIMIT 30".into(),
        "ci_test_history_timeline" => "SELECT id, job_id, outcome, duration_ms, run_at FROM test_results WHERE test_name = ? ORDER BY run_at DESC, id DESC LIMIT 50".into(),
        "ci_flaky_tests_intersect" => "SELECT test_name FROM test_results WHERE outcome = 'pass' AND id BETWEEN ? AND ? INTERSECT SELECT test_name FROM test_results WHERE outcome = 'fail' AND id BETWEEN ? AND ? ORDER BY 1 LIMIT 40".into(),
        "ci_always_green_except" => "SELECT test_name FROM test_results WHERE outcome = 'pass' AND id BETWEEN ? AND ? EXCEPT SELECT test_name FROM test_results WHERE outcome != 'pass' AND id BETWEEN ? AND ? ORDER BY 1 LIMIT 40".into(),
        "ci_repo_failure_trend" => format!("SELECT repo_id, {} AS ym, COUNT(*), SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END) FROM ci_jobs GROUP BY repo_id, ym ORDER BY repo_id, ym LIMIT 40", month_bucket(d, "created_at")),
        "ci_test_duration_delta" => "SELECT id, test_name, duration_ms, duration_ms - COALESCE(LAG(duration_ms) OVER (PARTITION BY test_name ORDER BY run_at, id), duration_ms) FROM test_results WHERE id BETWEEN ? AND ? ORDER BY id LIMIT 60".into(),
        "ci_runner_null_sort" => "SELECT id, runner_id, status, priority FROM ci_jobs WHERE id BETWEEN ? AND ? ORDER BY runner_id ASC NULLS FIRST, priority DESC, id ASC LIMIT 40".into(),
        "ci_distinct_tests_per_job" => "SELECT job_id, COUNT(DISTINCT test_name), SUM(duration_ms) FROM test_results WHERE job_id BETWEEN ? AND ? GROUP BY job_id ORDER BY SUM(duration_ms) DESC, job_id ASC LIMIT 25".into(),
        "ci_high_prio_failures" => format!("SELECT j.id, j.repo_id, {} FROM ci_jobs j JOIN test_results t ON t.job_id = j.id WHERE j.priority >= 8 AND t.outcome = 'fail' GROUP BY j.id, j.repo_id ORDER BY j.id DESC LIMIT 25", string_agg(d, "t.suite_name", ",")),
        "ci_test_results_count" => "SELECT COUNT(*) FROM test_results".into(),
        "ci_jobs_count_total" => "SELECT COUNT(*) FROM ci_jobs".into(),
        "ci_job_wallclock_buckets" => "SELECT CASE WHEN finished_at - claimed_at < 30000000 THEN 'fast' WHEN finished_at - claimed_at < 120000000 THEN 'med' ELSE 'slow' END AS bkt, COUNT(*) FROM ci_jobs WHERE finished_at IS NOT NULL AND claimed_at IS NOT NULL GROUP BY bkt ORDER BY bkt".into(),
        "ci_test_name_prefix_like" => "SELECT id, job_id, test_name, outcome, duration_ms FROM test_results WHERE test_name LIKE ? ORDER BY run_at DESC, id DESC LIMIT 30".into(),
        "ci_enqueue_job" => "INSERT INTO ci_jobs (id, repo_id, commit_sha, status, priority, runner_id, created_at, claimed_at, finished_at) VALUES (?, ?, ?, 'queued', ?, NULL, ?, NULL, NULL)".into(),
        "ci_claim_and_finish_txn" => return Some(vec![
            "UPDATE ci_jobs SET status = 'running', runner_id = ?, claimed_at = ? WHERE id = ?".into(),
            "UPDATE ci_jobs SET status = 'passed', finished_at = ? WHERE id = ?".into(),
        ]),
        "ci_bulk_test_results_50" => format!("INSERT INTO test_results (id, job_id, suite_name, test_name, outcome, duration_ms, run_at) VALUES {}", vec!["(?, ?, 'unit', ?, 'pass', ?, ?)"; 50].join(", ")),
        "ci_requeue_stale_job" => "UPDATE ci_jobs SET status = 'queued', runner_id = NULL, claimed_at = NULL WHERE id = ?".into(),
        "ci_cancel_repo_jobs" => "UPDATE ci_jobs SET priority = priority WHERE id BETWEEN ? AND ?".into(),
        "ci_record_flake_retry" => return Some(vec![
            "INSERT INTO test_results (id, job_id, suite_name, test_name, outcome, duration_ms, run_at) VALUES (?, ?, 'planner', 'planner::test_001', 'pass', 420, ?)".into(),
            "UPDATE ci_jobs SET finished_at = ? WHERE id = ?".into(),
        ]),
        "ci_job_savepoint_abort" => return Some(vec![
            "INSERT INTO ci_jobs (id, repo_id, commit_sha, status, priority, runner_id, created_at, claimed_at, finished_at) VALUES (?, 1, 'deadbeef', 'queued', 5, NULL, ?, NULL, NULL)".into(),
            "SAVEPOINT bad_batch".into(),
            "INSERT INTO test_results (id, job_id, suite_name, test_name, outcome, duration_ms, run_at) VALUES (?, ?, 'unit', 'unit::bad', 'fail', 10, ?)".into(),
            "ROLLBACK TO SAVEPOINT bad_batch".into(),
            "RELEASE SAVEPOINT bad_batch".into(),
        ]),
        "ci_bump_job_priority" => "UPDATE ci_jobs SET priority = 9 WHERE id = ?".into(),
        "ci_purge_temp_job_tree" => return Some(vec![
            "INSERT INTO ci_jobs (id, repo_id, commit_sha, status, priority, runner_id, created_at, claimed_at, finished_at) VALUES (?, 1, 'tmp000', 'passed', 1, 1, ?, ?, ?)".into(),
            "INSERT INTO test_results (id, job_id, suite_name, test_name, outcome, duration_ms, run_at) VALUES (?, ?, 'unit', 'unit::tmp', 'pass', 10, ?)".into(),
            "DELETE FROM test_results WHERE job_id = ?".into(),
            "DELETE FROM ci_jobs WHERE id = ?".into(),
        ]),
        "ci_bulk_jobs_25" => format!("INSERT INTO ci_jobs (id, repo_id, commit_sha, status, priority, runner_id, created_at, claimed_at, finished_at) VALUES {}", vec!["(?, 1, 'batchsha', 'queued', 3, NULL, ?, NULL, NULL)"; 25].join(", ")),

        // ======================= DOMAIN 7 & 8: SCHED & WIKI ==================
        "sch_interval_overlap" => "SELECT id, resource_id, owner_id, start_ts, end_ts, title FROM bookings WHERE resource_id = ? AND status != 'cancelled' AND start_ts < ? AND end_ts > ? ORDER BY start_ts ASC, id ASC LIMIT 30".into(),
        "sch_double_booking_self" => "SELECT a.id, b.id, a.resource_id, a.start_ts, b.start_ts FROM bookings a JOIN bookings b ON a.resource_id = b.resource_id AND a.id < b.id AND a.start_ts < b.end_ts AND a.end_ts > b.start_ts WHERE a.resource_id = ? AND a.status = 'confirmed' AND b.status = 'confirmed' ORDER BY a.id, b.id LIMIT 25".into(),
        "sch_free_slot_gaps_lead" => "WITH s AS (SELECT id, resource_id, start_ts, end_ts, LEAD(start_ts) OVER (PARTITION BY resource_id ORDER BY start_ts, id) AS next_start FROM bookings WHERE resource_id = ? AND status != 'cancelled') SELECT id, end_ts, next_start, next_start - end_ts AS gap_us FROM s WHERE next_start IS NOT NULL AND next_start > end_ts ORDER BY gap_us DESC, id ASC LIMIT 25".into(),
        "sch_resource_utilization" => "SELECT resource_id, COUNT(*), SUM(end_ts - start_ts) / 1000000 AS booked_sec FROM bookings WHERE status != 'cancelled' GROUP BY resource_id ORDER BY booked_sec DESC, resource_id ASC LIMIT 25".into(),
        "sch_owner_schedule" => "SELECT id, resource_id, start_ts, end_ts, status, title FROM bookings WHERE owner_id = ? AND status != 'cancelled' ORDER BY start_ts ASC, id ASC LIMIT 30".into(),
        "sch_owner_conflicts" => "SELECT a.id, b.id, a.owner_id, a.resource_id, b.resource_id FROM bookings a JOIN bookings b ON a.owner_id = b.owner_id AND a.id < b.id AND a.start_ts < b.end_ts AND a.end_ts > b.start_ts WHERE a.id BETWEEN ? AND ? AND a.status = 'confirmed' AND b.status = 'confirmed' ORDER BY a.id, b.id LIMIT 20".into(),
        "sch_longest_bookings" => "SELECT id, resource_id, owner_id, end_ts - start_ts AS dur_us FROM bookings ORDER BY dur_us DESC, id ASC LIMIT 25".into(),
        "sch_status_breakdown" => "SELECT resource_id, SUM(CASE WHEN status = 'confirmed' THEN 1 ELSE 0 END), SUM(CASE WHEN status = 'tentative' THEN 1 ELSE 0 END), SUM(CASE WHEN status = 'cancelled' THEN 1 ELSE 0 END) FROM bookings GROUP BY resource_id ORDER BY resource_id LIMIT 30".into(),
        "sch_busy_resources_having" => "SELECT resource_id, COUNT(*) FROM bookings WHERE status = 'confirmed' GROUP BY resource_id HAVING COUNT(*) >= 5 ORDER BY COUNT(*) DESC, resource_id ASC LIMIT 25".into(),
        "sch_booking_rank_per_res" => "SELECT id, resource_id, start_ts, ROW_NUMBER() OVER (PARTITION BY resource_id ORDER BY start_ts ASC, id ASC) FROM bookings WHERE resource_id BETWEEN ? AND ? ORDER BY resource_id, start_ts LIMIT 60".into(),
        "sch_bookings_count_total" => "SELECT COUNT(*) FROM bookings".into(),
        "wiki_doc_by_slug" => "SELECT id, tenant_id, slug, title, version, updated_at FROM documents WHERE tenant_id = ? AND slug = ?".into(),
        "wiki_backlinks" => "SELECT l.from_doc_id, d.title, l.anchor_text FROM doc_links l JOIN documents d ON d.id = l.from_doc_id WHERE l.to_doc_id = ? ORDER BY l.from_doc_id LIMIT 40".into(),
        "wiki_outbound_links" => "SELECT l.to_doc_id, d.title, l.anchor_text FROM doc_links l JOIN documents d ON d.id = l.to_doc_id WHERE l.from_doc_id = ? ORDER BY l.to_doc_id LIMIT 40".into(),
        "wiki_orphan_docs_anti" => "SELECT d.id, d.slug, d.title FROM documents d LEFT JOIN doc_links l ON l.to_doc_id = d.id WHERE l.from_doc_id IS NULL ORDER BY d.id ASC LIMIT 30".into(),
        "wiki_deadend_docs_notex" => "SELECT d.id, d.slug, d.title FROM documents d WHERE NOT EXISTS (SELECT 1 FROM doc_links l WHERE l.from_doc_id = d.id) ORDER BY d.id ASC LIMIT 30".into(),
        "wiki_hub_degree_top25" => "WITH ind AS (SELECT to_doc_id AS doc_id, COUNT(*) AS in_deg FROM doc_links GROUP BY to_doc_id), outd AS (SELECT from_doc_id AS doc_id, COUNT(*) AS out_deg FROM doc_links GROUP BY from_doc_id) SELECT d.id, d.title, COALESCE(ind.in_deg, 0), COALESCE(outd.out_deg, 0) FROM documents d LEFT JOIN ind ON ind.doc_id = d.id LEFT JOIN outd ON outd.doc_id = d.id ORDER BY COALESCE(ind.in_deg, 0) + COALESCE(outd.out_deg, 0) DESC, d.id ASC LIMIT 25".into(),
        "wiki_shortest_path_rec" => "WITH RECURSIVE walk(doc_id, depth) AS (SELECT id, 0 FROM documents WHERE id = ? UNION SELECT l.to_doc_id, w.depth + 1 FROM doc_links l JOIN walk w ON l.from_doc_id = w.doc_id WHERE w.depth < 3) SELECT doc_id, MIN(depth) FROM walk GROUP BY doc_id ORDER BY MIN(depth), doc_id LIMIT 40".into(),
        "wiki_mutual_links_self" => "SELECT a.from_doc_id, a.to_doc_id FROM doc_links a JOIN doc_links b ON a.from_doc_id = b.to_doc_id AND a.to_doc_id = b.from_doc_id WHERE a.from_doc_id < a.to_doc_id ORDER BY a.from_doc_id, a.to_doc_id LIMIT 30".into(),
        "wiki_wide_doc_search" => "SELECT id, tenant_id, title, version FROM documents WHERE body LIKE ? ORDER BY updated_at DESC, id ASC LIMIT 25".into(),
        "wiki_tenant_doc_versions" => "SELECT tenant_id, COUNT(*), AVG(version), SUM(LENGTH(body)) FROM documents GROUP BY tenant_id ORDER BY COUNT(*) DESC, tenant_id ASC LIMIT 25".into(),
        "wiki_doc_link_union" => "SELECT to_doc_id AS neighbor_id FROM doc_links WHERE from_doc_id = ? UNION SELECT from_doc_id AS neighbor_id FROM doc_links WHERE to_doc_id = ? ORDER BY neighbor_id LIMIT 50".into(),
        "wiki_most_updated_docs" => "SELECT tenant_id, id, version, DENSE_RANK() OVER (PARTITION BY tenant_id ORDER BY updated_at DESC, id ASC) AS rk FROM documents WHERE tenant_id BETWEEN ? AND ? ORDER BY tenant_id, rk, id LIMIT 50".into(),
        "wiki_docs_count_total" => "SELECT COUNT(*) FROM documents".into(),
        "wiki_links_count_total" => "SELECT COUNT(*) FROM doc_links".into(),
        "sch_create_booking_txn" => return Some(vec![
            "INSERT INTO bookings (id, resource_id, owner_id, start_ts, end_ts, status, title) VALUES (?, ?, ?, ?, ?, 'confirmed', 'GPU cluster run')".into(),
            "DELETE FROM bookings WHERE id = ?".into(),
        ]),
        "sch_reschedule_booking" => "UPDATE bookings SET start_ts = start_ts + 60000000, end_ts = end_ts + 60000000 WHERE id = ?".into(),
        "sch_cancel_booking" => return Some(vec![
            "UPDATE bookings SET status = 'cancelled' WHERE id = ?".into(),
            "UPDATE bookings SET status = 'confirmed' WHERE id = ?".into(),
        ]),
        "sch_bulk_bookings_25" => format!("INSERT INTO bookings (id, resource_id, owner_id, start_ts, end_ts, status, title) VALUES {}", vec!["(?, ?, ?, ?, ?, 'tentative', 'batch slot')"; 25].join(", ")),
        "wiki_publish_revision" => "UPDATE documents SET version = version + 1, body = ?, updated_at = ? WHERE id = ?".into(),
        "wiki_upsert_link" => "INSERT INTO doc_links (from_doc_id, to_doc_id, anchor_text) VALUES (?, ?, ?) ON CONFLICT (from_doc_id, to_doc_id) DO UPDATE SET anchor_text = excluded.anchor_text".into(),
        "wiki_rewire_links_txn" => return Some(vec![
            "DELETE FROM doc_links WHERE from_doc_id = ? AND to_doc_id = ?".into(),
            "INSERT INTO doc_links (from_doc_id, to_doc_id, anchor_text) VALUES (?, ?, 'rewired') ON CONFLICT (from_doc_id, to_doc_id) DO NOTHING".into(),
        ]),
        "wiki_create_doc_and_link" => return Some(vec![
            "INSERT INTO documents (id, tenant_id, slug, title, version, body, updated_at) VALUES (?, 1, ?, 'Temp RFC', 1, ?, ?)".into(),
            "INSERT INTO doc_links (from_doc_id, to_doc_id, anchor_text) VALUES (?, 1, 'parent')".into(),
            "DELETE FROM doc_links WHERE from_doc_id = ?".into(),
            "DELETE FROM documents WHERE id = ?".into(),
        ]),
        "wiki_bulk_links_25" => format!("INSERT INTO doc_links (from_doc_id, to_doc_id, anchor_text) VALUES {} ON CONFLICT (from_doc_id, to_doc_id) DO NOTHING", vec!["(?, ?, 'bulk ref')"; 25].join(", ")),
        "wiki_savepoint_edit" => return Some(vec![
            "SAVEPOINT wiki_rev".into(),
            "UPDATE documents SET version = version + 100 WHERE id = ?".into(),
            "ROLLBACK TO SAVEPOINT wiki_rev".into(),
            "RELEASE SAVEPOINT wiki_rev".into(),
        ]),

        // ======================= DOMAIN 9: CROSS-DOMAIN BI & ETL =============
        "bi_6table_saas_join" => "SELECT t.slug, p.key_code, i.id, i.priority, u.display_name, l.name, COUNT(c.id) FROM tenants t JOIN projects p ON p.tenant_id = t.id JOIN issues i ON i.project_id = p.id JOIN saas_users u ON u.id = i.author_id JOIN issue_labels il ON il.issue_id = i.id JOIN labels l ON l.id = il.label_id LEFT JOIN issue_comments c ON c.issue_id = i.id WHERE i.id BETWEEN ? AND ? GROUP BY t.slug, p.key_code, i.id, i.priority, u.display_name, l.name ORDER BY i.id ASC, l.name ASC LIMIT 30".into(),
        "bi_7table_agentic_saas" => "SELECT t.id, w.id, r.model, COUNT(DISTINCT tu.id), COUNT(DISTINCT tc.id), COUNT(DISTINCT d.id), COUNT(DISTINCT ps.id) FROM tenants t JOIN workspaces w ON w.tenant_id = t.id JOIN agent_runs r ON r.workspace_id = w.id JOIN agent_turns tu ON tu.run_id = r.id LEFT JOIN tool_calls tc ON tc.turn_id = tu.id LEFT JOIN file_diffs d ON d.run_id = r.id LEFT JOIN plan_steps ps ON ps.run_id = r.id WHERE r.id BETWEEN ? AND ? GROUP BY t.id, w.id, r.model ORDER BY COUNT(DISTINCT tc.id) DESC, r.model ASC LIMIT 25".into(),
        "bi_8table_full_enterprise" => "SELECT t.slug, p.name, i.status, au.role, COUNT(DISTINCT i.id) AS issues, COUNT(DISTINCT c.id) AS comments, COUNT(DISTINCT l.id) AS labels, COUNT(DISTINCT a.id) AS files FROM tenants t JOIN projects p ON p.tenant_id = t.id JOIN issues i ON i.project_id = p.id JOIN saas_users au ON au.id = i.author_id LEFT JOIN saas_users asg ON asg.id = i.assignee_id LEFT JOIN issue_comments c ON c.issue_id = i.id LEFT JOIN issue_labels il ON il.issue_id = i.id LEFT JOIN labels l ON l.id = il.label_id LEFT JOIN attachments a ON a.issue_id = i.id WHERE i.id BETWEEN ? AND ? GROUP BY t.slug, p.name, i.status, au.role ORDER BY issues DESC, t.slug ASC, p.name ASC, i.status ASC, au.role ASC LIMIT 25".into(),
        "bi_4step_funnel_cte" => "WITH s1 AS (SELECT id FROM accounts WHERE id BETWEEN ? AND ?), s2 AS (SELECT DISTINCT f.follower_id AS id FROM follows f JOIN s1 ON s1.id = f.follower_id), s3 AS (SELECT DISTINCT m.author_id AS id FROM messages m JOIN s2 ON s2.id = m.author_id), s4 AS (SELECT DISTINCT r.account_id AS id FROM reactions r JOIN s3 ON s3.id = r.account_id) SELECT (SELECT COUNT(*) FROM s1), (SELECT COUNT(*) FROM s2), (SELECT COUNT(*) FROM s3), (SELECT COUNT(*) FROM s4)".into(),
        "bi_commerce_funnel_cte" => "WITH c1 AS (SELECT id FROM customers WHERE id BETWEEN ? AND ?), c2 AS (SELECT DISTINCT o.customer_id AS id FROM orders o JOIN c1 ON c1.id = o.customer_id), c3 AS (SELECT DISTINCT o.customer_id AS id FROM orders o JOIN order_items oi ON oi.order_id = o.id JOIN c2 ON c2.id = o.customer_id), c4 AS (SELECT DISTINCT o.customer_id AS id FROM orders o JOIN payments p ON p.order_id = o.id AND p.status = 'captured' JOIN c3 ON c3.id = o.customer_id) SELECT (SELECT COUNT(*) FROM c1), (SELECT COUNT(*) FROM c2), (SELECT COUNT(*) FROM c3), (SELECT COUNT(*) FROM c4)".into(),
        "bi_monthly_cohort_matrix" => format!("SELECT {} AS cohort_m, {} AS order_m, COUNT(DISTINCT c.id), SUM(o.total_cents) FROM customers c JOIN orders o ON o.customer_id = c.id WHERE o.id BETWEEN ? AND ? GROUP BY cohort_m, order_m ORDER BY cohort_m, order_m LIMIT 40", month_bucket(d, "c.created_at"), month_bucket(d, "o.created_at")),
        "bi_compound_union_except" => "SELECT id FROM (SELECT author_id AS id FROM issues WHERE id BETWEEN ? AND ? UNION SELECT author_id AS id FROM issue_comments WHERE id BETWEEN ? AND ?) u EXCEPT SELECT id FROM saas_users WHERE NOT active ORDER BY id LIMIT 40".into(),
        "bi_4x_count_distinct" => "SELECT tenant_id, COUNT(DISTINCT project_id), COUNT(DISTINCT author_id), COUNT(DISTINCT assignee_id), COUNT(DISTINCT status) FROM issues GROUP BY tenant_id ORDER BY COUNT(DISTINCT project_id) DESC, tenant_id ASC LIMIT 25".into(),
        "bi_high_card_4col_groupby" => "SELECT tenant_id, project_id, status, priority, COUNT(*), AVG(estimate_hrs) FROM issues GROUP BY tenant_id, project_id, status, priority HAVING COUNT(*) >= 2 ORDER BY COUNT(*) DESC, tenant_id ASC, project_id ASC, priority ASC LIMIT 40".into(),
        "bi_unbounded_csv_export" => "SELECT o.id, c.country, c.tier, o.currency, o.status, o.total_cents, o.created_at FROM orders o JOIN customers c ON c.id = o.customer_id WHERE o.id BETWEEN ? AND ? ORDER BY c.country ASC, o.total_cents DESC, o.id ASC".into(),
        "bi_full_outer_reconcile" => "SELECT COALESCE(o.id, p.order_id) AS oid, o.status, p.status, COALESCE(o.total_cents, 0) - COALESCE(p.amount_cents, 0) FROM (SELECT id, status, total_cents FROM orders WHERE id BETWEEN ? AND ?) o FULL OUTER JOIN (SELECT order_id, status, amount_cents FROM payments WHERE order_id BETWEEN ? AND ?) p ON p.order_id = o.id ORDER BY oid LIMIT 50".into(),
        "bi_cross_domain_org_score" => "WITH iss AS (SELECT tenant_id, COUNT(*) AS icnt FROM issues GROUP BY tenant_id), tok AS (SELECT w.tenant_id, SUM(r.cost_micros) AS burn FROM workspaces w JOIN agent_runs r ON r.workspace_id = w.id GROUP BY w.tenant_id), doc AS (SELECT tenant_id, COUNT(*) AS dcnt FROM documents GROUP BY tenant_id) SELECT t.id, t.slug, COALESCE(iss.icnt, 0), COALESCE(tok.burn, 0), COALESCE(doc.dcnt, 0) FROM tenants t LEFT JOIN iss ON iss.tenant_id = t.id LEFT JOIN tok ON tok.tenant_id = t.id LEFT JOIN doc ON doc.tenant_id = t.id ORDER BY COALESCE(tok.burn, 0) DESC, t.id ASC LIMIT 25".into(),
        "bi_correlated_having_avg" => "SELECT tenant_id, COUNT(*), AVG(estimate_hrs) FROM issues WHERE estimate_hrs IS NOT NULL GROUP BY tenant_id HAVING AVG(estimate_hrs) > (SELECT AVG(estimate_hrs) FROM issues WHERE estimate_hrs IS NOT NULL) ORDER BY AVG(estimate_hrs) DESC, tenant_id ASC LIMIT 25".into(),
        "bi_scalar_subq_multi_col" => "SELECT t.id, t.slug, (SELECT COUNT(*) FROM projects p WHERE p.tenant_id = t.id), (SELECT COUNT(*) FROM saas_users u WHERE u.tenant_id = t.id AND u.active), (SELECT MAX(d.version) FROM documents d WHERE d.tenant_id = t.id) FROM tenants t WHERE t.id BETWEEN ? AND ? ORDER BY t.id".into(),
        "bi_exists_and_not_exists" => "SELECT o.id, o.total_cents FROM orders o WHERE o.id BETWEEN ? AND ? AND EXISTS (SELECT 1 FROM order_items oi WHERE oi.order_id = o.id) AND NOT EXISTS (SELECT 1 FROM payments p WHERE p.order_id = o.id AND p.status = 'failed') ORDER BY o.id LIMIT 30".into(),
        "bi_wide_sort_nulls_mixed" => "SELECT id, priority, estimate_hrs, due_at, title FROM issues WHERE id BETWEEN ? AND ? ORDER BY due_at DESC NULLS FIRST, estimate_hrs ASC NULLS LAST, priority ASC, id ASC LIMIT 50".into(),
        "bi_nested_derived_3deep" => "SELECT tier, COUNT(*), AVG(avg_order) FROM (SELECT c.tier, cust_agg.customer_id, cust_agg.avg_order FROM customers c JOIN (SELECT customer_id, AVG(total_cents) AS avg_order FROM (SELECT customer_id, total_cents FROM orders WHERE status != 'refunded' AND id BETWEEN ? AND ?) o0 GROUP BY customer_id) cust_agg ON cust_agg.customer_id = c.id) top GROUP BY tier ORDER BY tier".into(),
        "bi_window_running_pct" => "WITH by_c AS (SELECT currency, SUM(total_cents) AS rev FROM orders GROUP BY currency) SELECT currency, rev, SUM(rev) OVER (ORDER BY rev DESC, currency ASC ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW), SUM(rev) OVER () FROM by_c ORDER BY rev DESC, currency ASC".into(),
        "bi_window_frame_range_avg" => "SELECT id, total_cents, AVG(total_cents) OVER (ORDER BY id ROWS BETWEEN 2 PRECEDING AND 2 FOLLOWING) FROM orders WHERE id BETWEEN ? AND ? ORDER BY id".into(),
        "bi_union_all_ledger_audit" => "SELECT 'commerce' AS book, COUNT(*), COALESCE(SUM(amount_cents), 0) FROM ledger_entries UNION ALL SELECT 'tokens' AS book, COUNT(*), COALESCE(SUM(amount_micros), 0) FROM token_cost_ledger ORDER BY book".into(),
        "bi_case_nullif_coalesce" => "SELECT status, COUNT(*), ROUND(COALESCE(SUM(estimate_hrs), 0.0) / NULLIF(COUNT(estimate_hrs), 0), 2), ABS(MIN(priority) - MAX(priority)) FROM issues WHERE id BETWEEN ? AND ? GROUP BY status ORDER BY status".into(),
        "bi_in_subquery_multi" => "SELECT id, title, priority FROM issues WHERE id BETWEEN ? AND ? AND project_id IN (SELECT id FROM projects WHERE NOT archived) AND author_id IN (SELECT id FROM saas_users WHERE active) ORDER BY id LIMIT 30".into(),
        "bi_string_concat_agg" => format!("SELECT p.key_code, COUNT(i.id), {} FROM projects p JOIN issues i ON i.project_id = p.id WHERE i.id BETWEEN ? AND ? GROUP BY p.key_code ORDER BY COUNT(i.id) DESC, p.key_code ASC LIMIT 20", string_agg(d, "i.status", "|")),
        "bi_json_join_analytics" => format!("SELECT t.slug, {}, COUNT(*) FROM tenants t JOIN issues i ON i.tenant_id = t.id WHERE {} = 'enterprise' AND {} > 25 AND i.id BETWEEN ? AND ? GROUP BY t.slug, {} ORDER BY COUNT(*) DESC, t.slug ASC LIMIT 25", json_str(d, "i.metadata_json", "severity"), json_str(d, "t.settings_json", "tier"), json_i64(d, "i.metadata_json", "votes"), json_str(d, "i.metadata_json", "severity")),
        "bi_self_join_same_day" => "SELECT a.id, b.id, a.customer_id FROM orders a JOIN orders b ON a.customer_id = b.customer_id AND a.id < b.id WHERE a.id BETWEEN ? AND ? AND b.id BETWEEN ? AND ? ORDER BY a.id, b.id LIMIT 25".into(),
        "bi_distinct_on_window" => "SELECT account_id, id, event_kind, ts FROM (SELECT account_id, id, event_kind, ts, ROW_NUMBER() OVER (PARTITION BY account_id ORDER BY ts DESC, id DESC) AS rn FROM exp_events WHERE id BETWEEN ? AND ?) x WHERE rn = 1 ORDER BY account_id LIMIT 40".into(),
        "bi_cross_join_matrix" => "SELECT g.tier, s.status, COUNT(i.id) FROM (SELECT 'free' AS tier UNION ALL SELECT 'enterprise') g CROSS JOIN (SELECT 'open' AS status UNION ALL SELECT 'closed') s LEFT JOIN tenants t ON t.plan = g.tier LEFT JOIN issues i ON i.tenant_id = t.id AND i.status = s.status AND i.id BETWEEN ? AND ? GROUP BY g.tier, s.status ORDER BY g.tier, s.status".into(),
        "bi_top_n_per_group_3way" => "WITH r AS (SELECT c.country, o.id, o.total_cents, DENSE_RANK() OVER (PARTITION BY c.country ORDER BY o.total_cents DESC, o.id ASC) AS rk FROM orders o JOIN customers c ON c.id = o.customer_id WHERE o.id BETWEEN ? AND ?) SELECT country, id, total_cents, rk FROM r WHERE rk <= 3 ORDER BY country, rk, id LIMIT 40".into(),
        "bi_anti_join_3way_chain" => "SELECT c.id, o.id FROM customers c JOIN orders o ON o.customer_id = c.id LEFT JOIN payments p ON p.order_id = o.id AND p.status = 'captured' LEFT JOIN ledger_entries l ON l.order_id = o.id WHERE o.id BETWEEN ? AND ? AND p.id IS NULL AND l.id IS NULL ORDER BY o.id LIMIT 30".into(),
        "bi_multi_cte_diamond" => "WITH base AS (SELECT id, tenant_id, priority, estimate_hrs FROM issues WHERE id BETWEEN ? AND ?), hi AS (SELECT tenant_id, COUNT(*) AS hi_cnt FROM base WHERE priority <= 1 GROUP BY tenant_id), lo AS (SELECT tenant_id, AVG(estimate_hrs) AS lo_est FROM base WHERE priority >= 2 GROUP BY tenant_id) SELECT hi.tenant_id, hi.hi_cnt, COALESCE(lo.lo_est, 0.0) FROM hi LEFT JOIN lo ON lo.tenant_id = hi.tenant_id ORDER BY hi.hi_cnt DESC, hi.tenant_id ASC LIMIT 25".into(),
        "bi_unicode_collation_sort" => "SELECT id, display_name, handle FROM accounts WHERE id BETWEEN ? AND ? ORDER BY display_name ASC, handle ASC, id ASC LIMIT 40".into(),
        "bi_empty_filter_agg" => "SELECT COUNT(*), SUM(total_cents), AVG(total_cents), MIN(total_cents), MAX(total_cents) FROM orders WHERE id < 0".into(),
        "bi_group_by_expr_bucket" => "SELECT price_cents / 10000 AS price_band, COUNT(*), AVG(stock) FROM products GROUP BY price_band ORDER BY price_band LIMIT 30".into(),
        "bi_left_join_filter_on" => "SELECT p.id, COUNT(i.id) FROM projects p LEFT JOIN issues i ON i.project_id = p.id AND i.priority = 0 WHERE p.id BETWEEN ? AND ? GROUP BY p.id ORDER BY p.id".into(),
        "bi_sum_of_distinct_vals" => "SELECT tenant_id, SUM(DISTINCT priority), COUNT(DISTINCT status) FROM issues WHERE id BETWEEN ? AND ? GROUP BY tenant_id ORDER BY tenant_id LIMIT 25".into(),
        "etl_insert_select_rollup" => return Some(vec![
            format!("CREATE TABLE IF NOT EXISTS __etl_rollup (k {pk_ty}, cnt {i_ty} NOT NULL, tot {i_ty} NOT NULL)"),
            "DELETE FROM __etl_rollup".into(),
            "INSERT INTO __etl_rollup (k, cnt, tot) SELECT category_id, COUNT(*), SUM(price_cents) FROM products GROUP BY category_id".into(),
            "SELECT COUNT(*), SUM(cnt) FROM __etl_rollup".into(),
            "DROP TABLE __etl_rollup".into(),
        ]),
        "etl_ctas_snapshot" => return Some(vec![
            "DROP TABLE IF EXISTS __etl_ctas".into(),
            "CREATE TABLE __etl_ctas AS SELECT id, customer_id, total_cents FROM orders WHERE id BETWEEN ? AND ?".into(),
            "SELECT COUNT(*) FROM __etl_ctas".into(),
            "DROP TABLE __etl_ctas".into(),
        ]),
        "etl_batched_purge_subq" => return Some(vec![
            format!("CREATE TABLE IF NOT EXISTS __etl_stage (id {pk_ty}, flag {i_ty} NOT NULL)"),
            "INSERT INTO __etl_stage (id, flag) VALUES (?, 1), (?, 1), (?, 0), (?, 1), (?, 0)".into(),
            "DELETE FROM __etl_stage WHERE id IN (SELECT id FROM __etl_stage WHERE flag = 1)".into(),
            "DROP TABLE __etl_stage".into(),
        ]),
        "etl_update_from_agg" => "UPDATE channels SET message_count = (SELECT COUNT(*) FROM messages WHERE channel_id = channels.id) WHERE id = ?".into(),
        "etl_bulk_upsert_25" => format!("INSERT INTO agent_memories (id, workspace_id, scope, key_name, content, embedding_blob, score, expires_at, updated_at) VALUES {} ON CONFLICT (id) DO UPDATE SET score = excluded.score, updated_at = excluded.updated_at", vec!["(?, 1, 'workspace', ?, 'etl mem', ?, 0.85, NULL, ?)"; 25].join(", ")),

        // ======================= DOMAIN 10: DBA, DDL & CATALOG ===============
        "ddl_create_drop_table" => return Some(vec![
            format!("CREATE TABLE __g_tmp1 (id {pk_ty}, name TEXT NOT NULL, val {i_ty} NOT NULL)"),
            "INSERT INTO __g_tmp1 (id, name, val) VALUES (1, 'a', 10), (2, 'b', 20), (3, 'c', 30)".into(),
            "SELECT COUNT(*), SUM(val) FROM __g_tmp1".into(),
            "DROP TABLE __g_tmp1".into(),
        ]),
        "ddl_create_single_idx" => return Some(vec![
            format!("CREATE TABLE __g_idx1 (id {pk_ty}, k {i_ty} NOT NULL)"),
            "INSERT INTO __g_idx1 (id, k) VALUES (1, 9), (2, 3), (3, 7)".into(),
            "CREATE INDEX idx___g_idx1_k ON __g_idx1 (k)".into(),
            "SELECT id FROM __g_idx1 WHERE k = 7".into(),
            "DROP INDEX idx___g_idx1_k".into(),
            "DROP TABLE __g_idx1".into(),
        ]),
        "ddl_create_comp_desc_idx" => return Some(vec![
            format!("CREATE TABLE __g_idx2 (id {pk_ty}, a {i_ty} NOT NULL, b {i_ty} NOT NULL)"),
            "INSERT INTO __g_idx2 (id, a, b) VALUES (1, 1, 10), (2, 1, 30), (3, 2, 5)".into(),
            "CREATE INDEX idx___g_idx2_ab ON __g_idx2 (a ASC, b DESC)".into(),
            "SELECT id FROM __g_idx2 WHERE a = 1 ORDER BY b DESC".into(),
            "DROP TABLE __g_idx2".into(),
        ]),
        "ddl_create_unique_idx" => return Some(vec![
            format!("CREATE TABLE __g_idx3 (id {pk_ty}, code TEXT NOT NULL)"),
            "INSERT INTO __g_idx3 (id, code) VALUES (1, 'X1'), (2, 'X2')".into(),
            "CREATE UNIQUE INDEX idx___g_idx3_code ON __g_idx3 (code)".into(),
            "SELECT id FROM __g_idx3 WHERE code = 'X2'".into(),
            "DROP TABLE __g_idx3".into(),
        ]),
        "ddl_create_partial_idx" => return Some(vec![
            format!("CREATE TABLE __g_idx4 (id {pk_ty}, active {b_ty} NOT NULL, score {i_ty} NOT NULL)"),
            "INSERT INTO __g_idx4 (id, active, score) VALUES (1, ?, 10), (2, ?, 20), (3, ?, 30)".into(),
            "CREATE INDEX idx___g_idx4_part ON __g_idx4 (score) WHERE active".into(),
            "SELECT id FROM __g_idx4 WHERE active AND score >= 10 ORDER BY score".into(),
            "DROP TABLE __g_idx4".into(),
        ]),
        "ddl_create_expr_idx" => return Some(vec![
            format!("CREATE TABLE __g_idx5 (id {pk_ty}, email TEXT NOT NULL)"),
            "INSERT INTO __g_idx5 (id, email) VALUES (1, 'Alice@Ex.com'), (2, 'Bob@Ex.com')".into(),
            "CREATE INDEX idx___g_idx5_low ON __g_idx5 (LOWER(email))".into(),
            "SELECT id FROM __g_idx5 WHERE LOWER(email) = 'alice@ex.com'".into(),
            "DROP TABLE __g_idx5".into(),
        ]),
        "ddl_rename_table" => return Some(vec![
            format!("CREATE TABLE __g_ren_a (id {pk_ty}, v {i_ty})"),
            "INSERT INTO __g_ren_a (id, v) VALUES (1, 42)".into(),
            "ALTER TABLE __g_ren_a RENAME TO __g_ren_b".into(),
            "SELECT v FROM __g_ren_b WHERE id = 1".into(),
            "DROP TABLE __g_ren_b".into(),
        ]),
        "ddl_rename_column" => return Some(vec![
            format!("CREATE TABLE __g_renc (id {pk_ty}, old_col {i_ty})"),
            "INSERT INTO __g_renc (id, old_col) VALUES (1, 99)".into(),
            "ALTER TABLE __g_renc RENAME COLUMN old_col TO new_col".into(),
            "SELECT new_col FROM __g_renc WHERE id = 1".into(),
            "DROP TABLE __g_renc".into(),
        ]),
        "ddl_add_column_backfill" => return Some(vec![
            format!("CREATE TABLE __g_addc (id {pk_ty}, v {i_ty})"),
            "INSERT INTO __g_addc (id, v) VALUES (1, 7), (2, 8)".into(),
            format!("ALTER TABLE __g_addc ADD COLUMN tag TEXT DEFAULT 'v1'"),
            "UPDATE __g_addc SET tag = 'backfilled' WHERE id = 2".into(),
            "SELECT id, tag FROM __g_addc ORDER BY id".into(),
            "DROP TABLE __g_addc".into(),
        ]),
        "ddl_drop_column" => return Some(vec![
            format!("CREATE TABLE __g_dropc (id {pk_ty}, keep_v {i_ty}, drop_v {i_ty})"),
            "INSERT INTO __g_dropc (id, keep_v, drop_v) VALUES (1, 10, 20)".into(),
            "ALTER TABLE __g_dropc DROP COLUMN drop_v".into(),
            "SELECT keep_v FROM __g_dropc".into(),
            "DROP TABLE __g_dropc".into(),
        ]),
        "ddl_view_create_join_drop" => return Some(vec![
            "DROP VIEW IF EXISTS __g_v_open_issues".into(),
            "CREATE VIEW __g_v_open_issues AS SELECT id, tenant_id, project_id, priority FROM issues WHERE status = 'open' AND deleted_at IS NULL".into(),
            "SELECT p.key_code, COUNT(v.id) FROM projects p JOIN __g_v_open_issues v ON v.project_id = p.id WHERE p.id BETWEEN 1 AND 5 GROUP BY p.key_code ORDER BY p.key_code".into(),
            "DROP VIEW __g_v_open_issues".into(),
        ]),
        "ddl_nested_views" => return Some(vec![
            "DROP VIEW IF EXISTS __g_v2".into(),
            "DROP VIEW IF EXISTS __g_v1".into(),
            "CREATE VIEW __g_v1 AS SELECT id, category_id, price_cents FROM products WHERE active".into(),
            "CREATE VIEW __g_v2 AS SELECT category_id, COUNT(*) AS cnt, AVG(price_cents) AS avg_p FROM __g_v1 GROUP BY category_id".into(),
            "SELECT category_id, cnt FROM __g_v2 ORDER BY cnt DESC LIMIT 10".into(),
            "DROP VIEW __g_v2".into(),
            "DROP VIEW __g_v1".into(),
        ]),
        "ddl_trigger_after_insert" => {
            if is_ext_pg {
                return None;
            }
            return Some(vec![
                format!("CREATE TABLE __g_tbase (id {pk_ty}, v {i_ty})"),
                format!("CREATE TABLE __g_taudit (src_id {i_ty}, op TEXT)"),
                "CREATE TRIGGER __g_tr_ins AFTER INSERT ON __g_tbase BEGIN INSERT INTO __g_taudit VALUES (NEW.id, 'ins'); END".into(),
                "INSERT INTO __g_tbase VALUES (1, 100), (2, 200)".into(),
                "SELECT COUNT(*) FROM __g_taudit".into(),
                "DROP TRIGGER __g_tr_ins".into(),
                "DROP TABLE __g_taudit".into(),
                "DROP TABLE __g_tbase".into(),
            ]);
        }
        "ddl_trigger_after_update" => {
            if is_ext_pg {
                return None;
            }
            return Some(vec![
                format!("CREATE TABLE __g_tubase (id {pk_ty}, v {i_ty})"),
                format!("CREATE TABLE __g_tuaudit (src_id {i_ty}, old_v {i_ty}, new_v {i_ty})"),
                "CREATE TRIGGER __g_tr_upd AFTER UPDATE ON __g_tubase BEGIN INSERT INTO __g_tuaudit VALUES (NEW.id, OLD.v, NEW.v); END".into(),
                "INSERT INTO __g_tubase VALUES (1, 10)".into(),
                "UPDATE __g_tubase SET v = 25 WHERE id = 1".into(),
                "SELECT old_v, new_v FROM __g_tuaudit".into(),
                "DROP TRIGGER __g_tr_upd".into(),
                "DROP TABLE __g_tuaudit".into(),
                "DROP TABLE __g_tubase".into(),
            ]);
        }
        "ddl_trigger_after_delete" => {
            if is_ext_pg {
                return None;
            }
            return Some(vec![
                format!("CREATE TABLE __g_tdbase (id {pk_ty}, v {i_ty})"),
                format!("CREATE TABLE __g_tdaudit (src_id {i_ty})"),
                "CREATE TRIGGER __g_tr_del AFTER DELETE ON __g_tdbase BEGIN INSERT INTO __g_tdaudit VALUES (OLD.id); END".into(),
                "INSERT INTO __g_tdbase VALUES (1, 10), (2, 20)".into(),
                "DELETE FROM __g_tdbase WHERE id = 1".into(),
                "SELECT COUNT(*) FROM __g_tdaudit".into(),
                "DROP TRIGGER __g_tr_del".into(),
                "DROP TABLE __g_tdaudit".into(),
                "DROP TABLE __g_tdbase".into(),
            ]);
        }
        "ddl_temp_table_join" => return Some(vec![
            format!("CREATE TEMP TABLE __g_temp_ids (id {pk_ty})"),
            "INSERT INTO __g_temp_ids (id) VALUES (1), (2), (3)".into(),
            "SELECT t.id, p.name FROM __g_temp_ids t LEFT JOIN projects p ON p.id = t.id ORDER BY t.id".into(),
            "DROP TABLE __g_temp_ids".into(),
        ]),
        "ddl_analyze_stats" => "ANALYZE".into(),
        "ddl_reindex_table" => {
            if is_ext_pg {
                return Some(vec![
                    format!("CREATE TABLE __g_reidx (id {pk_ty}, k {i_ty})"),
                    "INSERT INTO __g_reidx VALUES (1, 10), (2, 20)".into(),
                    "CREATE INDEX idx___g_reidx ON __g_reidx (k)".into(),
                    "REINDEX TABLE __g_reidx".into(),
                    "DROP TABLE __g_reidx".into(),
                ]);
            }
            return Some(vec![
                format!("CREATE TABLE __g_reidx (id {pk_ty}, k {i_ty})"),
                "INSERT INTO __g_reidx VALUES (1, 10), (2, 20)".into(),
                "CREATE INDEX idx___g_reidx ON __g_reidx (k)".into(),
                "REINDEX __g_reidx".into(),
                "DROP TABLE __g_reidx".into(),
            ]);
        }
        "ddl_wal_checkpoint_pass" => {
            if is_ext_pg {
                return None;
            }
            "PRAGMA wal_checkpoint(PASSIVE)".into()
        }
        "ddl_integrity_check" => {
            if is_ext_pg {
                return None;
            }
            "PRAGMA integrity_check".into()
        }
        "cat_list_tables" => {
            if is_pg_cat {
                "SELECT relname FROM pg_class WHERE relkind = 'r' ORDER BY relname".into()
            } else {
                "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name".into()
            }
        }
        "cat_list_indexes" => {
            if is_pg_cat {
                "SELECT relname FROM pg_class WHERE relkind = 'i' ORDER BY relname".into()
            } else {
                "SELECT name FROM sqlite_schema WHERE type = 'index' AND name NOT LIKE 'sqlite_%' ORDER BY name".into()
            }
        }
        "cat_table_columns" => {
            if is_pg_cat {
                "SELECT a.attnum, a.attname, a.attnotnull FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid WHERE c.relname = 'issues' AND a.attnum > 0 ORDER BY a.attnum".into()
            } else {
                "SELECT cid, name, type, \"notnull\" FROM pragma_table_info('issues') ORDER BY cid".into()
            }
        }
        "cat_pg_namespaces" => {
            if !is_pg_cat {
                return None;
            }
            "SELECT oid, nspname FROM pg_namespace ORDER BY oid".into()
        }
        "cat_pg_types" => {
            if !is_pg_cat {
                return None;
            }
            "SELECT oid, typname FROM pg_type WHERE typname IN ('int4', 'int8', 'float8', 'bool', 'text', 'bytea', 'jsonb') ORDER BY typname".into()
        }
        "cat_pg_class_oids" => {
            if !is_pg_cat {
                return None;
            }
            "SELECT oid, relname, relkind FROM pg_class WHERE relname IN ('issues', 'orders', 'agent_runs', 'ledger_entries') ORDER BY relname".into()
        }
        "cat_pg_attributes_join" => {
            if !is_pg_cat {
                return None;
            }
            "SELECT c.relname, a.attname, t.typname FROM pg_class c JOIN pg_attribute a ON a.attrelid = c.oid JOIN pg_type t ON t.oid = a.atttypid WHERE c.relname = 'orders' AND a.attnum > 0 ORDER BY a.attnum".into()
        }
        "cat_oid_rename_stability" => {
            if is_pg_cat {
                return Some(vec![
                    format!("CREATE TABLE __g_oid_a (id {pk_ty}, v {i_ty})"),
                    "SELECT oid, relname FROM pg_class WHERE relname = '__g_oid_a'".into(),
                    "ALTER TABLE __g_oid_a RENAME TO __g_oid_b".into(),
                    "SELECT oid, relname FROM pg_class WHERE relname = '__g_oid_b'".into(),
                    "DROP TABLE __g_oid_b".into(),
                ]);
            }
            return Some(vec![
                format!("CREATE TABLE __g_oid_a (id {pk_ty}, v {i_ty})"),
                "ALTER TABLE __g_oid_a RENAME TO __g_oid_b".into(),
                "SELECT name FROM sqlite_schema WHERE name = '__g_oid_b'".into(),
                "DROP TABLE __g_oid_b".into(),
            ]);
        }
        "ddl_if_not_exists_idemp" => return Some(vec![
            format!("CREATE TABLE IF NOT EXISTS __g_idem (id {pk_ty}, k {i_ty})"),
            format!("CREATE TABLE IF NOT EXISTS __g_idem (id {pk_ty}, k {i_ty})"),
            "CREATE INDEX IF NOT EXISTS idx___g_idem ON __g_idem (k)".into(),
            "CREATE INDEX IF NOT EXISTS idx___g_idem ON __g_idem (k)".into(),
            "DROP INDEX IF EXISTS idx___g_idem".into(),
            "DROP TABLE IF EXISTS __g_idem".into(),
            "DROP TABLE IF EXISTS __g_idem".into(),
        ]),
        "ddl_wide_schema_20cols" => return Some(vec![
            format!("CREATE TABLE __g_wide20 (id {pk_ty}, c1 {i_ty}, c2 {i_ty}, c3 {i_ty}, c4 {i_ty}, c5 {i_ty}, c6 TEXT, c7 TEXT, c8 TEXT, c9 TEXT, c10 TEXT, c11 DOUBLE PRECISION, c12 DOUBLE PRECISION, c13 {b_ty}, c14 {b_ty}, c15 {i_ty}, c16 {i_ty}, c17 TEXT, c18 TEXT, c19 {i_ty})"),
            "INSERT INTO __g_wide20 VALUES (1, 1, 2, 3, 4, 5, 'a', 'b', 'c', 'd', 'e', 1.5, 2.5, ?, ?, 15, 16, 'x', 'y', 19)".into(),
            "SELECT id, c5, c10, c11, c19 FROM __g_wide20 WHERE id = 1".into(),
            "DROP TABLE __g_wide20".into(),
        ]),
        _ => return None,
    };
    one(s)
}

pub fn params_5_10(
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
        // Domain 5
        "obs_trace_waterfall_rec" | "obs_trace_spans_ordered" => {
            one(vec![P::I(pick(rng, s.traces))])
        }
        "obs_svc_latency_summary"
        | "obs_svc_error_rate"
        | "obs_canary_vs_baseline"
        | "obs_region_latency_json"
        | "obs_multi_window_spans"
        | "obs_orphan_spans_anti"
        | "obs_team_tier_rollup"
        | "obs_critical_path_span"
        | "obs_op_latency_buckets"
        | "obs_svc_silent_check" => {
            let (a, b) = span(rng, s.spans, 5_000);
            one(vec![P::I(a), P::I(b)])
        }
        "obs_rolling_30_anomaly" | "obs_span_percent_rank" | "obs_span_lag_interarrival" => {
            one(vec![P::I(pick(rng, s.services))])
        }
        "obs_trace_total_duration" | "obs_deepest_traces" | "obs_trace_fanout_rank" => {
            let (a, b) = span(rng, s.traces, 500);
            one(vec![P::I(a), P::I(b)])
        }
        "obs_exp_conversion_24h" | "obs_exp_unconverted_anti" => {
            let (a, b) = span(rng, s.exp_events, 5_000);
            one(vec![P::I(pick(rng, s.experiments)), P::I(a), P::I(b)])
        }
        "obs_exp_variant_lift" | "obs_exp_value_quantiles" | "obs_exp_daily_trend" => {
            one(vec![P::I(pick(rng, s.experiments))])
        }
        "obs_exp_srm_check" | "obs_exp_first_exposure" | "obs_exp_multi_touch" => {
            let (a, b) = span(rng, s.exp_events, 5_000);
            one(vec![P::I(a), P::I(b)])
        }
        "obs_slow_outliers_top30"
        | "obs_exp_active_summary"
        | "obs_spans_count_total"
        | "obs_exp_events_count" => one(vec![]),
        "obs_ingest_span_single" => one(vec![
            P::I(s.alloc_id()),
            P::I(pick(rng, s.traces)),
            P::I(pick(rng, s.services)),
            P::I(1500),
            P::T(r#"{"region":"US","canary":false}"#.into()),
            P::I(ts),
        ]),
        "obs_ingest_spans_bulk_50" => {
            let tr = pick(rng, s.traces);
            let sv = pick(rng, s.services);
            let mut v = Vec::with_capacity(50 * 6);
            for _ in 0..50 {
                v.extend([
                    P::I(s.alloc_id()),
                    P::I(tr),
                    P::I(sv),
                    P::I(800),
                    P::T(r#"{"region":"US","canary":true}"#.into()),
                    P::I(ts),
                ]);
            }
            one(v)
        }
        "obs_ingest_exp_event" => one(vec![
            P::I(s.alloc_id()),
            P::I(pick(rng, s.experiments)),
            P::I(pick(rng, s.accounts)),
            P::F(12.5),
            P::I(ts),
        ]),
        "obs_bulk_exp_events_50" => {
            let ex = pick(rng, s.experiments);
            let mut v = Vec::with_capacity(50 * 4);
            for _ in 0..50 {
                v.extend([
                    P::I(s.alloc_id()),
                    P::I(ex),
                    P::I(pick(rng, s.accounts)),
                    P::I(ts),
                ]);
            }
            one(v)
        }
        "obs_close_experiment" => {
            let id = own(rng, s.experiments);
            Some(vec![vec![P::I(id)], vec![P::I(id)]])
        }
        "obs_update_span_tags" => one(vec![
            P::T(r#"{"region":"DE","canary":true}"#.into()),
            P::I(own(rng, s.spans)),
        ]),
        "obs_prune_old_spans_reins" => {
            let id = s.alloc_id();
            Some(vec![
                vec![
                    P::I(id),
                    P::T(r#"{"region":"US","canary":false}"#.into()),
                    P::I(ts),
                ],
                vec![P::I(id)],
            ])
        }
        "obs_register_service_tmp" => {
            let id = s.alloc_id();
            Some(vec![
                vec![P::I(id), P::T(format!("tmp-svc-{id}"))],
                vec![P::I(id)],
            ])
        }
        "obs_trace_pair_txn" => {
            let p_id = s.alloc_id();
            let c_id = s.alloc_id();
            let tr = s.alloc_id();
            Some(vec![
                vec![
                    P::I(p_id),
                    P::I(tr),
                    P::T(r#"{"region":"US","canary":false}"#.into()),
                    P::I(ts),
                ],
                vec![
                    P::I(c_id),
                    P::I(tr),
                    P::I(p_id),
                    P::T(r#"{"region":"US","canary":false}"#.into()),
                    P::I(ts + 100),
                ],
            ])
        }
        "obs_exp_upsert_toggle" => one(vec![P::F(9.99), P::I(own(rng, s.exp_events))]),

        // Domain 6
        "ci_stuck_running_jobs" => one(vec![P::I(BASE_TS_US + s.ci_jobs * 10_000_000)]),
        "ci_commit_status_rollup" | "ci_jobs_without_tests" | "ci_runner_null_sort" => {
            let (a, b) = span(rng, s.ci_jobs, 2_000);
            one(vec![P::I(a), P::I(b)])
        }
        "ci_flakiest_tests_top50"
        | "ci_pass_to_fail_regress"
        | "ci_slowest_tests_per_suite"
        | "ci_suite_duration_rank"
        | "ci_test_duration_delta" => {
            let (a, b) = span(rng, s.test_results, 6_000);
            one(vec![P::I(a), P::I(b)])
        }
        "ci_flaky_tests_intersect" | "ci_always_green_except" => {
            let (a, b) = span(rng, s.test_results, 4_000);
            one(vec![P::I(a), P::I(b), P::I(a), P::I(b)])
        }
        "ci_test_history_timeline" => {
            let suites = [
                "unit",
                "integration",
                "planner",
                "storage",
                "postgres",
                "e2e",
            ];
            let h = rng.next();
            let su = suites[(h % suites.len() as u64) as usize];
            one(vec![P::T(format!("{su}::test_{:03}", h % 20))])
        }
        "ci_distinct_tests_per_job" => {
            let (a, b) = span(rng, s.ci_jobs, 500);
            one(vec![P::I(a), P::I(b)])
        }
        "ci_test_name_prefix_like" => one(vec![P::T("planner::test_00%".into())]),
        "ci_queue_next_job"
        | "ci_queue_depth_by_repo"
        | "ci_runner_utilization"
        | "ci_queue_wait_outliers"
        | "ci_suite_pass_rate_pivot"
        | "ci_failing_jobs_with_tests"
        | "ci_repo_failure_trend"
        | "ci_high_prio_failures"
        | "ci_test_results_count"
        | "ci_jobs_count_total"
        | "ci_job_wallclock_buckets" => one(vec![]),
        "ci_enqueue_job" => {
            let id = s.alloc_id();
            one(vec![
                P::I(id),
                P::I(1 + (rng.next() % 20) as i64),
                P::T(format!("{id:016x}")),
                P::I((rng.next() % 10) as i64),
                P::I(ts),
            ])
        }
        "ci_claim_and_finish_txn" => {
            let id = own(rng, s.ci_jobs);
            Some(vec![
                vec![P::I(1 + (c.0 as i64 % 64)), P::I(ts), P::I(id)],
                vec![P::I(ts + 5000), P::I(id)],
            ])
        }
        "ci_bulk_test_results_50" => {
            let job = own(rng, s.ci_jobs);
            let mut v = Vec::with_capacity(50 * 5);
            for k in 0..50 {
                v.extend([
                    P::I(s.alloc_id()),
                    P::I(job),
                    P::T(format!("unit::bulk_{k:03}")),
                    P::I(15 + k),
                    P::I(ts),
                ]);
            }
            one(v)
        }
        "ci_requeue_stale_job" | "ci_bump_job_priority" => one(vec![P::I(own(rng, s.ci_jobs))]),
        "ci_cancel_repo_jobs" => {
            let a = own(rng, s.ci_jobs);
            one(vec![P::I(a), P::I((a + 9).min(s.ci_jobs.max(1)))])
        }
        "ci_record_flake_retry" => {
            let job = own(rng, s.ci_jobs);
            Some(vec![
                vec![P::I(s.alloc_id()), P::I(job), P::I(ts)],
                vec![P::I(ts), P::I(job)],
            ])
        }
        "ci_job_savepoint_abort" => {
            let jid = s.alloc_id();
            let tid = s.alloc_id();
            Some(vec![
                vec![P::I(jid), P::I(ts)],
                vec![],
                vec![P::I(tid), P::I(jid), P::I(ts)],
                vec![],
                vec![],
            ])
        }
        "ci_purge_temp_job_tree" => {
            let jid = s.alloc_id();
            let tid = s.alloc_id();
            Some(vec![
                vec![P::I(jid), P::I(ts), P::I(ts), P::I(ts)],
                vec![P::I(tid), P::I(jid), P::I(ts)],
                vec![P::I(jid)],
                vec![P::I(jid)],
            ])
        }
        "ci_bulk_jobs_25" => {
            let mut v = Vec::with_capacity(25 * 2);
            for _ in 0..25 {
                v.extend([P::I(s.alloc_id()), P::I(ts)]);
            }
            one(v)
        }

        // Domain 7 & 8
        "sch_interval_overlap" => {
            let res = pick(rng, s.resources);
            let start = BASE_TS_US + (rng.next() % 50) as i64 * 7_200_000_000;
            one(vec![P::I(res), P::I(start + 14_400_000_000), P::I(start)])
        }
        "sch_double_booking_self" | "sch_free_slot_gaps_lead" => {
            one(vec![P::I(pick(rng, s.resources))])
        }
        "sch_owner_schedule" => one(vec![P::I(pick(rng, s.accounts))]),
        "sch_owner_conflicts" => {
            let (a, b) = span(rng, s.bookings, 3_000);
            one(vec![P::I(a), P::I(b)])
        }
        "sch_booking_rank_per_res" => {
            let (a, b) = span(rng, s.resources, 10);
            one(vec![P::I(a), P::I(b)])
        }
        "wiki_doc_by_slug" => {
            let id = pick(rng, s.documents);
            let h = mix(id as u64 ^ 0x8001);
            let tid = s.skewed_tenant(h);
            let w = crate::WORDS[(h % crate::WORDS.len() as u64) as usize];
            one(vec![P::I(tid), P::T(format!("doc-{w}-{id}"))])
        }
        "wiki_backlinks" | "wiki_outbound_links" | "wiki_shortest_path_rec" => {
            one(vec![P::I(pick(rng, s.documents))])
        }
        "wiki_doc_link_union" => {
            let id = pick(rng, s.documents);
            one(vec![P::I(id), P::I(id)])
        }
        "wiki_wide_doc_search" => one(vec![P::T("%quorum%".into())]),
        "wiki_most_updated_docs" => {
            let (a, b) = span(rng, s.tenants, 10);
            one(vec![P::I(a), P::I(b)])
        }
        "sch_resource_utilization"
        | "sch_longest_bookings"
        | "sch_status_breakdown"
        | "sch_busy_resources_having"
        | "sch_bookings_count_total"
        | "wiki_orphan_docs_anti"
        | "wiki_deadend_docs_notex"
        | "wiki_hub_degree_top25"
        | "wiki_mutual_links_self"
        | "wiki_tenant_doc_versions"
        | "wiki_docs_count_total"
        | "wiki_links_count_total" => one(vec![]),
        "sch_create_booking_txn" => {
            let id = s.alloc_id();
            Some(vec![
                vec![
                    P::I(id),
                    P::I(pick(rng, s.resources)),
                    P::I(pick(rng, s.accounts)),
                    P::I(ts),
                    P::I(ts + 3_600_000_000),
                ],
                vec![P::I(id)],
            ])
        }
        "sch_reschedule_booking" => one(vec![P::I(own(rng, s.bookings))]),
        "sch_cancel_booking" => {
            let id = own(rng, s.bookings);
            Some(vec![vec![P::I(id)], vec![P::I(id)]])
        }
        "sch_bulk_bookings_25" => {
            let res = pick(rng, s.resources);
            let owner = pick(rng, s.accounts);
            let mut v = Vec::with_capacity(25 * 5);
            for k in 0..25 {
                let st = ts + k * 7_200_000_000;
                v.extend([
                    P::I(s.alloc_id()),
                    P::I(res),
                    P::I(owner),
                    P::I(st),
                    P::I(st + 3_600_000_000),
                ]);
            }
            one(v)
        }
        "wiki_publish_revision" => one(vec![
            P::T("Updated RFC design doc text ".repeat(30)),
            P::I(ts),
            P::I(own(rng, s.documents)),
        ]),
        "wiki_upsert_link" => {
            let a = own(rng, s.documents);
            let b = 1 + (a % s.documents.max(1));
            one(vec![P::I(a), P::I(b), P::T("see updated".into())])
        }
        "wiki_rewire_links_txn" => {
            let a = own(rng, s.documents);
            let b = 1 + (a % s.documents.max(1));
            Some(vec![vec![P::I(a), P::I(b)], vec![P::I(a), P::I(b)]])
        }
        "wiki_create_doc_and_link" => {
            let id = s.alloc_id();
            let body = "Architecture decision paragraph. ".repeat(250);
            Some(vec![
                vec![
                    P::I(id),
                    P::T(format!("tmp-rfc-{id}")),
                    P::T(body),
                    P::I(ts),
                ],
                vec![P::I(id)],
                vec![P::I(id)],
                vec![P::I(id)],
            ])
        }
        "wiki_bulk_links_25" => {
            let a = own(rng, s.documents);
            let mut v = Vec::with_capacity(25 * 2);
            for k in 1..=25 {
                let b = 1 + ((a + k) % s.documents.max(1));
                v.extend([P::I(a), P::I(b)]);
            }
            one(v)
        }
        "wiki_savepoint_edit" => {
            let id = own(rng, s.documents);
            Some(vec![vec![], vec![P::I(id)], vec![], vec![]])
        }

        // Domain 9: Cross-Domain BI & ETL
        "bi_6table_saas_join"
        | "bi_wide_sort_nulls_mixed"
        | "bi_case_nullif_coalesce"
        | "bi_in_subquery_multi"
        | "bi_string_concat_agg"
        | "bi_json_join_analytics"
        | "bi_cross_join_matrix"
        | "bi_multi_cte_diamond"
        | "bi_sum_of_distinct_vals" => {
            let (a, b) = span(rng, s.issues, 3_000);
            one(vec![P::I(a), P::I(b)])
        }
        "bi_7table_agentic_saas" => {
            let (a, b) = span(rng, s.agent_runs, 500);
            one(vec![P::I(a), P::I(b)])
        }
        "bi_8table_full_enterprise" => {
            let (a, b) = span(rng, s.issues, 500);
            one(vec![P::I(a), P::I(b)])
        }
        "bi_4step_funnel_cte" | "bi_unicode_collation_sort" => {
            let (a, b) = span(rng, s.accounts, 2_000);
            one(vec![P::I(a), P::I(b)])
        }
        "bi_commerce_funnel_cte" => {
            let (a, b) = span(rng, s.customers, 2_000);
            one(vec![P::I(a), P::I(b)])
        }
        "bi_monthly_cohort_matrix"
        | "bi_unbounded_csv_export"
        | "bi_exists_and_not_exists"
        | "bi_nested_derived_3deep"
        | "bi_window_frame_range_avg"
        | "bi_top_n_per_group_3way"
        | "bi_anti_join_3way_chain" => {
            let (a, b) = span(rng, s.orders, 3_000);
            one(vec![P::I(a), P::I(b)])
        }
        "bi_compound_union_except" => {
            let (a, b) = span(rng, s.issues, 3_000);
            let (c0, c1) = span(rng, s.issue_comments, 3_000);
            one(vec![P::I(a), P::I(b), P::I(c0), P::I(c1)])
        }
        "bi_full_outer_reconcile" | "bi_self_join_same_day" => {
            let (a, b) = span(rng, s.orders, 1_500);
            one(vec![P::I(a), P::I(b), P::I(a), P::I(b)])
        }
        "bi_scalar_subq_multi_col" => {
            let (a, b) = span(rng, s.tenants, 15);
            one(vec![P::I(a), P::I(b)])
        }
        "bi_distinct_on_window" => {
            let (a, b) = span(rng, s.exp_events, 3_000);
            one(vec![P::I(a), P::I(b)])
        }
        "bi_left_join_filter_on" => {
            let (a, b) = span(rng, s.projects, 30);
            one(vec![P::I(a), P::I(b)])
        }
        "bi_4x_count_distinct"
        | "bi_high_card_4col_groupby"
        | "bi_cross_domain_org_score"
        | "bi_correlated_having_avg"
        | "bi_window_running_pct"
        | "bi_union_all_ledger_audit"
        | "bi_empty_filter_agg"
        | "bi_group_by_expr_bucket" => one(vec![]),
        "etl_insert_select_rollup" => Some(vec![vec![], vec![], vec![], vec![], vec![]]),
        "etl_ctas_snapshot" => {
            let (a, b) = span(rng, s.orders, 1_000);
            Some(vec![vec![], vec![P::I(a), P::I(b)], vec![], vec![]])
        }
        "etl_batched_purge_subq" => Some(vec![
            vec![],
            vec![P::I(1), P::I(2), P::I(3), P::I(4), P::I(5)],
            vec![],
            vec![],
        ]),
        "etl_update_from_agg" => one(vec![P::I(own(rng, s.channels))]),
        "etl_bulk_upsert_25" => {
            let mut v = Vec::with_capacity(25 * 4);
            for k in 0..25 {
                let id = 1 + ((own(rng, s.agent_memories) + k) % s.agent_memories.max(1));
                v.extend([
                    P::I(id),
                    P::T(format!("etl-{id}")),
                    P::B(vec![9u8; 64]),
                    P::I(ts),
                ]);
            }
            one(v)
        }

        // Domain 10: DBA, DDL & Catalog
        "ddl_create_partial_idx" => Some(vec![
            vec![],
            vec![P::Z(true), P::Z(false), P::Z(true)],
            vec![],
            vec![],
            vec![],
        ]),
        "ddl_wide_schema_20cols" => {
            Some(vec![vec![], vec![P::Z(true), P::Z(false)], vec![], vec![]])
        }
        "ddl_create_drop_table"
        | "ddl_rename_table"
        | "ddl_rename_column"
        | "ddl_drop_column"
        | "ddl_view_create_join_drop"
        | "ddl_temp_table_join"
        | "ddl_reindex_table"
        | "ddl_create_comp_desc_idx"
        | "ddl_create_unique_idx"
        | "ddl_create_expr_idx" => Some(vec![vec![]; 5]),
        "ddl_create_single_idx" | "ddl_add_column_backfill" => Some(vec![vec![]; 6]),
        "ddl_nested_views" | "ddl_if_not_exists_idemp" => Some(vec![vec![]; 7]),
        "ddl_trigger_after_insert" | "ddl_trigger_after_delete" => Some(vec![vec![]; 8]),
        "ddl_trigger_after_update" => Some(vec![vec![]; 9]),
        "cat_oid_rename_stability" => Some(vec![vec![]; 5]),
        "ddl_analyze_stats"
        | "ddl_wal_checkpoint_pass"
        | "ddl_integrity_check"
        | "cat_list_tables"
        | "cat_list_indexes"
        | "cat_table_columns"
        | "cat_pg_namespaces"
        | "cat_pg_types"
        | "cat_pg_class_oids"
        | "cat_pg_attributes_join" => one(vec![]),
        _ => None,
    }
}
