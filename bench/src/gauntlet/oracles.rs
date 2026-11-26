//! Automated Anti-Gaming Oracles for the Meta-Scale Agentic End-User Gauntlet.
//!
//! These 5 oracles catch subtle real-world engine bugs that throughput numbers alone hide:
//! 1. **Multi-Page Tie-Heavy Pagination Conservation** (warm AND forced-spill tight cache):
//!    paginating an entire tie-heavy slice 25 rows at a time must yield 0 duplicates, 0 lost rows,
//!    and bit-for-bit identical order to the full unpaginated query.
//! 2. **Spill-Invariance Oracle (`cache_size = 4000` vs `cache_size = 48`)**:
//!    wide sorts, `NULLS FIRST/LAST`, `GROUP BY`, `HAVING`, `COUNT(DISTINCT)`, `DISTINCT`, windows,
//!    set ops, joins, and recursive CTEs must return identical results whether resident in RAM or
//!    spilling aggressively to disk.
//! 3. **Index-Invariance Oracle (Indexed vs `NOT INDEXED`)**:
//!    covering indexes, partial indexes (`WHERE due_at IS NOT NULL`, `WHERE deleted_at IS NULL`),
//!    mixed `ASC`/`DESC` composite indexes, and unique lookups must return identical results to
//!    forced full table scans.
//! 4. **Conservation, Referential & Physical Integrity Oracle**:
//!    `SUM(ledger_entries.amount_cents) == 0`, `SUM(token_cost_ledger.amount_micros) == 0`,
//!    `MIN(products.stock) >= 0`, empty tenant has 0 issues, zero orphan rows, and
//!    `PRAGMA integrity_check == ok`.
//! 5. **Cold-Reopen & `pg_class` OID Durability Oracle**:
//!    reopening from disk preserves committed writes, balances, row counts, and (on Postgres
//!    dialects) `pg_class.oid` stability across reopen and `ALTER TABLE RENAME`.

use std::path::Path;

use amphora::db::{Db, Sharing};

use crate::suite::{Dialect, P};
use crate::suite_db::{Cell, Session, amphora_value};

use super::loader::GauntletShape;

pub struct OracleReport {
    pub passed: bool,
    pub details: serde_json::Value,
}

fn direct_query_amphora(
    db: &mut Db,
    sql: &'static str,
    params: &[P],
) -> anyhow::Result<Vec<Vec<Option<String>>>> {
    let mut rows = Vec::new();
    db.query(sql, params.iter().map(amphora_value), |r| {
        let row = r
            .get_values()
            .map(|v| match v {
                amphora_engine::Value::Null => Cell::Null.text(),
                amphora_engine::Value::Numeric(amphora_engine::Numeric::Integer(i)) => {
                    Cell::Int(*i).text()
                }
                amphora_engine::Value::Numeric(amphora_engine::Numeric::Float(f)) => {
                    Cell::Real(f64::from(*f)).text()
                }
                amphora_engine::Value::Text(t) => Cell::Text(t.as_str()).text(),
                amphora_engine::Value::Blob(b) => Cell::Blob(b).text(),
            })
            .collect();
        rows.push(row);
        Ok(true)
    })?;
    Ok(rows)
}

fn direct_query_sqlite(
    conn: &rusqlite::Connection,
    sql: &str,
    params: &[P],
) -> anyhow::Result<Vec<Vec<Option<String>>>> {
    let mut stmt = conn.prepare(sql)?;
    let cols = stmt.column_count();
    let vals: Vec<rusqlite::types::Value> = params
        .iter()
        .map(|p| match p {
            P::I(i) => rusqlite::types::Value::Integer(*i),
            P::F(f) => rusqlite::types::Value::Real(*f),
            P::T(s) => rusqlite::types::Value::Text(s.clone()),
            P::B(b) => rusqlite::types::Value::Blob(b.clone()),
            P::Z(z) => rusqlite::types::Value::Integer(i64::from(*z)),
            P::N => rusqlite::types::Value::Null,
        })
        .collect();
    let mut rows = stmt.query(rusqlite::params_from_iter(vals.iter()))?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let mut row = Vec::with_capacity(cols);
        for i in 0..cols {
            let v = match r.get_ref(i)? {
                rusqlite::types::ValueRef::Null => Cell::Null.text(),
                rusqlite::types::ValueRef::Integer(n) => Cell::Int(n).text(),
                rusqlite::types::ValueRef::Real(f) => Cell::Real(f).text(),
                rusqlite::types::ValueRef::Text(b) => {
                    Cell::Text(std::str::from_utf8(b).unwrap_or("")).text()
                }
                rusqlite::types::ValueRef::Blob(b) => Cell::Blob(b).text(),
            };
            row.push(v);
        }
        out.push(row);
    }
    Ok(out)
}

/// Oracle 1: Full multi-page tie-heavy pagination conservation sweep.
pub fn oracle_pagination_conservation(
    session: &mut dyn Session,
    shape: &GauntletShape,
) -> OracleReport {
    if shape.issues <= 0 {
        return OracleReport {
            passed: true,
            details: serde_json::json!({ "skipped": "empty dataset" }),
        };
    }
    let slice_hi = shape.issues.min(350);
    let cases = [
        (
            "priority_status_ties",
            "SELECT id, priority, status FROM issues WHERE id BETWEEN 1 AND ? ORDER BY priority ASC, status ASC, id ASC",
        ),
        (
            "due_nulls_first_desc",
            "SELECT id, due_at, priority FROM issues WHERE id BETWEEN 1 AND ? ORDER BY due_at DESC NULLS FIRST, priority ASC, id ASC",
        ),
        (
            "due_nulls_last_asc",
            "SELECT id, due_at, priority FROM issues WHERE id BETWEEN 1 AND ? ORDER BY due_at ASC NULLS LAST, priority DESC, id ASC",
        ),
        (
            "est_nulls_last_desc",
            "SELECT id, estimate_hrs, status FROM issues WHERE id BETWEEN 1 AND ? ORDER BY estimate_hrs DESC NULLS LAST, id ASC",
        ),
    ];

    let mut checked = 0usize;
    let mut failures = Vec::new();
    for (name, base_sql) in cases {
        let full = match session.answer(&[base_sql.into()], &[vec![P::I(slice_hi)]]) {
            Ok(r) => r,
            Err(e) => {
                failures.push(format!("{name}: full query error: {e:#}"));
                continue;
            }
        };
        let mut stitched = Vec::with_capacity(full.len());
        let page_sz = 25i64;
        let mut off = 0i64;
        let page_sql = format!("{base_sql} LIMIT {page_sz} OFFSET ?");
        loop {
            match session.answer(&[page_sql.clone()], &[vec![P::I(slice_hi), P::I(off)]]) {
                Ok(page) => {
                    let n = page.len();
                    stitched.extend(page);
                    if (n as i64) < page_sz {
                        break;
                    }
                    off += page_sz;
                    if off > slice_hi + page_sz * 2 {
                        failures.push(format!("{name}: infinite pagination loop"));
                        break;
                    }
                }
                Err(e) => {
                    failures.push(format!("{name} offset={off}: {e:#}"));
                    break;
                }
            }
        }
        checked += 1;
        if stitched != full {
            failures.push(format!(
                "{name}: stitched {} rows != full {} rows (duplicate/lost/reordered page!)",
                stitched.len(),
                full.len()
            ));
        }
    }

    OracleReport {
        passed: failures.is_empty(),
        details: serde_json::json!({
            "cases_checked": checked,
            "slice_rows": slice_hi,
            "failures": failures,
        }),
    }
}

/// Oracle 3: Index-invariance (normal index scan vs forced `NOT INDEXED` full table scan).
pub fn oracle_index_invariance(
    session: &mut dyn Session,
    dialect: Dialect,
    shape: &GauntletShape,
) -> OracleReport {
    if matches!(
        dialect,
        Dialect::Postgres | Dialect::MySql | Dialect::ClickHouse
    ) {
        return OracleReport {
            passed: true,
            details: serde_json::json!({ "skipped": "NOT INDEXED is SQLite/Amphora syntax" }),
        };
    }
    let slice_hi = shape.issues.min(2_000).max(1);
    let checks: Vec<(&str, String, String, Vec<P>)> = vec![
        (
            "issues_inbox_composite_desc",
            "SELECT id, priority, updated_at FROM issues WHERE tenant_id = 1 AND status = 'open' AND deleted_at IS NULL ORDER BY priority ASC, updated_at DESC, id ASC LIMIT 40".into(),
            "SELECT id, priority, updated_at FROM issues NOT INDEXED WHERE tenant_id = 1 AND status = 'open' AND deleted_at IS NULL ORDER BY priority ASC, updated_at DESC, id ASC LIMIT 40".into(),
            vec![],
        ),
        (
            "issues_partial_due_not_null",
            "SELECT id, due_at FROM issues WHERE tenant_id = 1 AND due_at IS NOT NULL ORDER BY due_at ASC, id ASC LIMIT 40".into(),
            "SELECT id, due_at FROM issues NOT INDEXED WHERE tenant_id = 1 AND due_at IS NOT NULL ORDER BY due_at ASC, id ASC LIMIT 40".into(),
            vec![],
        ),
        (
            "issues_partial_open_not_deleted",
            "SELECT id, updated_at FROM issues WHERE tenant_id = 1 AND deleted_at IS NULL ORDER BY updated_at DESC, id ASC LIMIT 40".into(),
            "SELECT id, updated_at FROM issues NOT INDEXED WHERE tenant_id = 1 AND deleted_at IS NULL ORDER BY updated_at DESC, id ASC LIMIT 40".into(),
            vec![],
        ),
        (
            "issues_covering_count",
            "SELECT COUNT(*) FROM issues WHERE tenant_id = 1 AND status = 'open'".into(),
            "SELECT COUNT(*) FROM issues NOT INDEXED WHERE tenant_id = 1 AND status = 'open'".into(),
            vec![],
        ),
        (
            "messages_channel_seq_desc",
            "SELECT id, seq, author_id FROM messages WHERE channel_id = 1 AND seq <= ? ORDER BY seq DESC LIMIT 40".into(),
            "SELECT id, seq, author_id FROM messages NOT INDEXED WHERE channel_id = 1 AND seq <= ? ORDER BY seq DESC LIMIT 40".into(),
            vec![P::I(slice_hi)],
        ),
        (
            "products_active_stock_idx",
            "SELECT id, sku, stock FROM products WHERE active AND stock < 25 ORDER BY stock ASC, id ASC LIMIT 40".into(),
            "SELECT id, sku, stock FROM products NOT INDEXED WHERE active AND stock < 25 ORDER BY stock ASC, id ASC LIMIT 40".into(),
            vec![],
        ),
        (
            "ci_jobs_priority_queue_desc",
            "SELECT id, priority, created_at FROM ci_jobs WHERE status = 'queued' ORDER BY priority DESC, created_at ASC, id ASC LIMIT 30".into(),
            "SELECT id, priority, created_at FROM ci_jobs NOT INDEXED WHERE status = 'queued' ORDER BY priority DESC, created_at ASC, id ASC LIMIT 30".into(),
            vec![],
        ),
        (
            "agent_runs_ws_status_desc",
            "SELECT id, started_at, total_tokens FROM agent_runs WHERE workspace_id = 1 AND status = 'completed' ORDER BY started_at DESC, id ASC LIMIT 30".into(),
            "SELECT id, started_at, total_tokens FROM agent_runs NOT INDEXED WHERE workspace_id = 1 AND status = 'completed' ORDER BY started_at DESC, id ASC LIMIT 30".into(),
            vec![],
        ),
        (
            "bookings_resource_interval",
            "SELECT id, start_ts, end_ts FROM bookings WHERE resource_id = 1 ORDER BY start_ts ASC, end_ts ASC, id ASC LIMIT 40".into(),
            "SELECT id, start_ts, end_ts FROM bookings NOT INDEXED WHERE resource_id = 1 ORDER BY start_ts ASC, end_ts ASC, id ASC LIMIT 40".into(),
            vec![],
        ),
        (
            "ledger_account_posted",
            "SELECT id, posted_at, amount_cents FROM ledger_entries WHERE account_id = 1 ORDER BY posted_at ASC, id ASC LIMIT 40".into(),
            "SELECT id, posted_at, amount_cents FROM ledger_entries NOT INDEXED WHERE account_id = 1 ORDER BY posted_at ASC, id ASC LIMIT 40".into(),
            vec![],
        ),
        (
            "spans_service_time_desc",
            "SELECT id, started_at, duration_us FROM spans WHERE service_id = 1 ORDER BY started_at DESC, id ASC LIMIT 40".into(),
            "SELECT id, started_at, duration_us FROM spans NOT INDEXED WHERE service_id = 1 ORDER BY started_at DESC, id ASC LIMIT 40".into(),
            vec![],
        ),
        (
            "empty_tenant_indexed_vs_scan",
            "SELECT id, priority FROM issues WHERE tenant_id = ? AND status = 'open' ORDER BY priority ASC, id ASC LIMIT 25".into(),
            "SELECT id, priority FROM issues NOT INDEXED WHERE tenant_id = ? AND status = 'open' ORDER BY priority ASC, id ASC LIMIT 25".into(),
            vec![P::I(shape.empty_tenant)],
        ),
    ];

    let mut failures = Vec::new();
    for (name, idx_sql, scan_sql, params) in &checks {
        let r_idx = session.answer(&[idx_sql.clone()], &[params.clone()]);
        let r_scan = session.answer(&[scan_sql.clone()], &[params.clone()]);
        match (r_idx, r_scan) {
            (Ok(a), Ok(b)) => {
                if a != b {
                    failures.push(format!(
                        "{name}: indexed ({} rows) != NOT INDEXED ({} rows)",
                        a.len(),
                        b.len()
                    ));
                }
            }
            (Err(e), _) => failures.push(format!("{name} indexed error: {e:#}")),
            (_, Err(e)) => failures.push(format!("{name} scan error: {e:#}")),
        }
    }

    OracleReport {
        passed: failures.is_empty(),
        details: serde_json::json!({
            "checks": checks.len(),
            "failures": failures,
        }),
    }
}

/// Oracle 4: Accounting, stock, empty-tenant, referential, and `PRAGMA integrity_check`.
pub fn oracle_conservation_and_integrity(
    session: &mut dyn Session,
    dialect: Dialect,
    shape: &GauntletShape,
) -> OracleReport {
    let mut failures = Vec::new();
    let mut metrics = serde_json::Map::new();

    let scalar_i64 = |sess: &mut dyn Session, sql: &str, params: &[P]| -> anyhow::Result<i64> {
        let rows = sess.answer(&[sql.into()], &[params.to_vec()])?;
        let s = rows
            .first()
            .and_then(|r| r.first())
            .and_then(|c| c.as_deref())
            .unwrap_or("0");
        Ok(s.parse::<i64>().unwrap_or(0))
    };

    // 1. Commerce double-entry trial balance == 0 (and non-empty when orders were loaded)
    match session.answer(
        &["SELECT COUNT(*), COALESCE(SUM(amount_cents), 0) FROM ledger_entries".into()],
        &[vec![]],
    ) {
        Ok(rows) => {
            let cnt = rows
                .first()
                .and_then(|r| r.first())
                .and_then(|c| c.as_deref())
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0);
            let sum = rows
                .first()
                .and_then(|r| r.get(1))
                .and_then(|c| c.as_deref())
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0);
            metrics.insert("ledger_count".into(), serde_json::json!(cnt));
            metrics.insert("ledger_sum_cents".into(), serde_json::json!(sum));
            if sum != 0 {
                failures.push(format!("ledger_entries SUM(amount_cents) = {sum} != 0"));
            }
            if shape.orders > 0 && cnt == 0 {
                failures.push("ledger_entries has 0 rows despite non-empty scale".into());
            }
        }
        Err(e) => failures.push(format!("ledger check error: {e:#}")),
    }

    // 2. Agentic token double-entry trial balance == 0
    match scalar_i64(
        session,
        "SELECT COALESCE(SUM(amount_micros), 0) FROM token_cost_ledger",
        &[],
    ) {
        Ok(v) => {
            metrics.insert("token_ledger_sum_micros".into(), serde_json::json!(v));
            if v != 0 {
                failures.push(format!("token_cost_ledger SUM(amount_micros) = {v} != 0"));
            }
        }
        Err(e) => failures.push(format!("token ledger check error: {e:#}")),
    }

    // 3. No negative product stock
    match scalar_i64(session, "SELECT COALESCE(MIN(stock), 0) FROM products", &[]) {
        Ok(v) => {
            metrics.insert("min_product_stock".into(), serde_json::json!(v));
            if v < 0 {
                failures.push(format!("products MIN(stock) = {v} < 0"));
            }
        }
        Err(e) => failures.push(format!("stock check error: {e:#}")),
    }

    // 4. Empty tenant has literally 0 issues
    match scalar_i64(
        session,
        "SELECT COUNT(*) FROM issues WHERE tenant_id = ?",
        &[P::I(shape.empty_tenant)],
    ) {
        Ok(v) => {
            metrics.insert("empty_tenant_issues".into(), serde_json::json!(v));
            if v != 0 {
                failures.push(format!(
                    "empty tenant {} has {v} issues != 0",
                    shape.empty_tenant
                ));
            }
        }
        Err(e) => failures.push(format!("empty tenant check error: {e:#}")),
    }

    // 5. Referential checks: zero orphan order_items -> orders, agent_turns -> agent_runs,
    //    payments -> orders
    match scalar_i64(
        session,
        "SELECT COUNT(*) FROM order_items oi LEFT JOIN orders o ON o.id = oi.order_id WHERE o.id IS NULL",
        &[],
    ) {
        Ok(v) => {
            metrics.insert("orphan_order_items".into(), serde_json::json!(v));
            if v != 0 {
                failures.push(format!("found {v} orphaned order_items"));
            }
        }
        Err(e) => failures.push(format!("orphan order_items check error: {e:#}")),
    }
    match scalar_i64(
        session,
        "SELECT COUNT(*) FROM agent_turns t LEFT JOIN agent_runs r ON r.id = t.run_id WHERE r.id IS NULL",
        &[],
    ) {
        Ok(v) => {
            metrics.insert("orphan_agent_turns".into(), serde_json::json!(v));
            if v != 0 {
                failures.push(format!("found {v} orphaned agent_turns"));
            }
        }
        Err(e) => failures.push(format!("orphan agent_turns check error: {e:#}")),
    }
    match scalar_i64(
        session,
        "SELECT COUNT(*) FROM payments p LEFT JOIN orders o ON o.id = p.order_id WHERE o.id IS NULL",
        &[],
    ) {
        Ok(v) => {
            metrics.insert("orphan_payments".into(), serde_json::json!(v));
            if v != 0 {
                failures.push(format!("found {v} orphaned payments"));
            }
        }
        Err(e) => failures.push(format!("orphan payments check error: {e:#}")),
    }

    // 6. PRAGMA integrity_check on SQLite / Amphora / AmphoraPostgres
    if matches!(dialect, Dialect::Sqlite | Dialect::AmphoraPostgres) {
        match session.answer(&["PRAGMA integrity_check".into()], &[vec![]]) {
            Ok(rows) => {
                let first = rows
                    .first()
                    .and_then(|r| r.first())
                    .and_then(|c| c.as_deref())
                    .unwrap_or("");
                metrics.insert("integrity_check".into(), serde_json::json!(first));
                if first != "ok" {
                    failures.push(format!("PRAGMA integrity_check returned: {rows:?}"));
                }
            }
            Err(e) => failures.push(format!("integrity_check error: {e:#}")),
        }
    }

    OracleReport {
        passed: failures.is_empty(),
        details: serde_json::json!({
            "metrics": metrics,
            "failures": failures,
        }),
    }
}

/// Pre-close durable state snapshot captured from the live database before closing handles,
/// compared field-for-field after cold reopen so silent data loss (even balanced 0-row deletion!)
/// can never pass the durability oracle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreCloseSnapshot {
    pub ledger_count: i64,
    pub ledger_sum: i64,
    pub ledger_abs_sum: i64,
    pub ledger_digest: String,
    pub token_ledger_count: i64,
    pub token_ledger_sum: i64,
    pub token_ledger_abs_sum: i64,
    pub orders_count: i64,
    pub payments_count: i64,
    pub issues_count: i64,
}

fn parse_i64_cell(rows: &[Vec<Option<String>>], col: usize) -> i64 {
    rows.first()
        .and_then(|r| r.get(col))
        .and_then(|c| c.as_deref())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0)
}

fn snapshot_from_query<F>(mut run: F) -> anyhow::Result<PreCloseSnapshot>
where
    F: FnMut(&'static str) -> anyhow::Result<Vec<Vec<Option<String>>>>,
{
    let l = run(
        "SELECT COUNT(*), COALESCE(SUM(amount_cents), 0), COALESCE(SUM(ABS(amount_cents)), 0) FROM ledger_entries",
    )?;
    let l_rows =
        run("SELECT id, account_id, amount_cents FROM ledger_entries ORDER BY id LIMIT 200")?;
    let tl = run(
        "SELECT COUNT(*), COALESCE(SUM(amount_micros), 0), COALESCE(SUM(ABS(amount_micros)), 0) FROM token_cost_ledger",
    )?;
    let counts = run(
        "SELECT (SELECT COUNT(*) FROM orders), (SELECT COUNT(*) FROM payments), (SELECT COUNT(*) FROM issues)",
    )?;
    Ok(PreCloseSnapshot {
        ledger_count: parse_i64_cell(&l, 0),
        ledger_sum: parse_i64_cell(&l, 1),
        ledger_abs_sum: parse_i64_cell(&l, 2),
        ledger_digest: super::parametric::digest_answer(&l_rows),
        token_ledger_count: parse_i64_cell(&tl, 0),
        token_ledger_sum: parse_i64_cell(&tl, 1),
        token_ledger_abs_sum: parse_i64_cell(&tl, 2),
        orders_count: parse_i64_cell(&counts, 0),
        payments_count: parse_i64_cell(&counts, 1),
        issues_count: parse_i64_cell(&counts, 2),
    })
}

pub fn capture_pre_close_snapshot(session: &mut dyn Session) -> anyhow::Result<PreCloseSnapshot> {
    snapshot_from_query(|sql| session.answer(&[sql.into()], &[vec![]]))
}

fn verify_reopen_snapshot(
    reopened: &PreCloseSnapshot,
    pre_close: Option<&PreCloseSnapshot>,
    shape: &GauntletShape,
    reopen_meta: &mut serde_json::Map<String, serde_json::Value>,
    reopen_failures: &mut Vec<String>,
) {
    reopen_meta.insert(
        "cold_reopen_ledger".into(),
        serde_json::json!({
            "count": reopened.ledger_count,
            "sum": reopened.ledger_sum,
            "abs_sum": reopened.ledger_abs_sum,
            "digest": reopened.ledger_digest,
            "token_ledger_count": reopened.token_ledger_count,
            "token_ledger_sum": reopened.token_ledger_sum,
            "orders_count": reopened.orders_count,
            "payments_count": reopened.payments_count,
            "issues_count": reopened.issues_count,
        }),
    );
    if reopened.ledger_sum != 0 {
        reopen_failures.push(format!(
            "cold reopen ledger_entries SUM(amount_cents) = {} != 0",
            reopened.ledger_sum
        ));
    }
    if reopened.token_ledger_sum != 0 {
        reopen_failures.push(format!(
            "cold reopen token_cost_ledger SUM(amount_micros) = {} != 0",
            reopened.token_ledger_sum
        ));
    }
    if shape.orders > 0 && (reopened.ledger_count <= 0 || reopened.ledger_abs_sum <= 0) {
        reopen_failures.push(format!(
            "cold reopen lost ledger rows on non-empty scale: count={}, abs_sum={}",
            reopened.ledger_count, reopened.ledger_abs_sum
        ));
    }
    if let Some(before) = pre_close {
        if reopened != before {
            reopen_failures.push(format!(
                "cold reopen state diverged from pre-close snapshot: before={before:?}, after={reopened:?}"
            ));
        }
    }
}

/// Oracle 2 (Spill-Invariance: `cache=4000` vs `cache=48`) AND Oracle 5 (Cold-Reopen &
/// `pg_class` OID Durability) run directly on the settled database file after the live `Target`
/// has closed cleanly.
pub fn oracle_spill_and_cold_reopen(
    target_name: &str,
    dir: &str,
    shape: &GauntletShape,
    pre_close: Option<&PreCloseSnapshot>,
) -> (OracleReport, OracleReport) {
    if !matches!(target_name, "amphora" | "amphora-pg" | "sqlite") {
        let skipped = OracleReport {
            passed: true,
            details: serde_json::json!({ "skipped": format!("file-based reopen not run for {target_name}") }),
        };
        return (
            OracleReport {
                passed: true,
                details: serde_json::json!({ "skipped": format!("cache-size spill check not run for {target_name}") }),
            },
            skipped,
        );
    }

    let db_path = Path::new(dir).join("suite.db");
    let slice_hi = shape.issues.min(3_000).max(1);
    let ord_hi = shape.orders.min(3_000).max(1);
    let spill_queries: Vec<(&str, &str, Vec<P>)> = vec![
        (
            "wide_desc_sort",
            "SELECT id, priority, title, LENGTH(description) FROM issues WHERE id BETWEEN 1 AND ? ORDER BY updated_at DESC, priority ASC, id ASC LIMIT 60",
            vec![P::I(slice_hi)],
        ),
        (
            "due_nulls_first",
            "SELECT id, due_at, priority FROM issues WHERE id BETWEEN 1 AND ? ORDER BY due_at DESC NULLS FIRST, priority ASC, id ASC LIMIT 60",
            vec![P::I(slice_hi)],
        ),
        (
            "due_nulls_last",
            "SELECT id, due_at, priority FROM issues WHERE id BETWEEN 1 AND ? ORDER BY due_at ASC NULLS LAST, priority DESC, id ASC LIMIT 60",
            vec![P::I(slice_hi)],
        ),
        (
            "high_card_groupby",
            "SELECT tenant_id, project_id, status, priority, COUNT(*), COALESCE(SUM(estimate_hrs), 0) FROM issues WHERE id BETWEEN 1 AND ? GROUP BY tenant_id, project_id, status, priority HAVING COUNT(*) >= 1 ORDER BY COUNT(*) DESC, tenant_id ASC, project_id ASC, priority ASC LIMIT 50",
            vec![P::I(slice_hi)],
        ),
        (
            "multi_count_distinct",
            "SELECT tenant_id, COUNT(DISTINCT project_id), COUNT(DISTINCT author_id), COUNT(DISTINCT assignee_id) FROM issues WHERE id BETWEEN 1 AND ? GROUP BY tenant_id ORDER BY tenant_id LIMIT 30",
            vec![P::I(slice_hi)],
        ),
        (
            "window_multi_spec",
            "SELECT id, project_id, ROW_NUMBER() OVER (PARTITION BY project_id ORDER BY priority ASC, id ASC), COUNT(*) OVER (PARTITION BY assignee_id) FROM issues WHERE id BETWEEN 1 AND ? ORDER BY id LIMIT 80",
            vec![P::I(slice_hi.min(500))],
        ),
        (
            "window_running_sum",
            "SELECT id, account_id, SUM(amount_cents) OVER (PARTITION BY account_id ORDER BY posted_at, id ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) FROM ledger_entries WHERE account_id <= 5 ORDER BY account_id, posted_at, id LIMIT 100",
            vec![],
        ),
        (
            "setop_compound",
            "SELECT author_id FROM issues WHERE id BETWEEN 1 AND ? UNION SELECT author_id FROM issue_comments WHERE id BETWEEN 1 AND ? EXCEPT SELECT id FROM saas_users WHERE NOT active ORDER BY 1 LIMIT 50",
            vec![P::I(slice_hi), P::I(slice_hi)],
        ),
        (
            "join_6table",
            "SELECT t.slug, p.key_code, i.id, u.display_name, COUNT(c.id) FROM tenants t JOIN projects p ON p.tenant_id = t.id JOIN issues i ON i.project_id = p.id JOIN saas_users u ON u.id = i.author_id LEFT JOIN issue_comments c ON c.issue_id = i.id WHERE i.id BETWEEN 1 AND ? GROUP BY t.slug, p.key_code, i.id, u.display_name ORDER BY i.id ASC LIMIT 40",
            vec![P::I(slice_hi.min(500))],
        ),
        (
            "orders_export_sort",
            "SELECT o.id, c.country, o.total_cents FROM orders o JOIN customers c ON c.id = o.customer_id WHERE o.id BETWEEN 1 AND ? ORDER BY c.country ASC, o.total_cents DESC, o.id ASC LIMIT 80",
            vec![P::I(ord_hi)],
        ),
        (
            "recursive_categories",
            "WITH RECURSIVE ctree(id) AS (SELECT id FROM categories WHERE id = 1 UNION ALL SELECT c.id FROM categories c JOIN ctree ON c.parent_id = ctree.id) SELECT p.id, p.price_cents FROM products p JOIN ctree ON ctree.id = p.category_id ORDER BY p.price_cents ASC, p.id ASC LIMIT 40",
            vec![],
        ),
        (
            "distinct_pairs",
            "SELECT DISTINCT status, priority FROM issues WHERE id BETWEEN 1 AND ? ORDER BY status, priority",
            vec![P::I(slice_hi)],
        ),
        // Round 8 P2 regression: spilled pagination without secondary tie-breaker must keep exact
        // warm arrival order on tied keys (no tie reversal on spill).
        (
            "tied_pagination_arrival_order",
            "SELECT id, priority FROM issues WHERE id BETWEEN 1 AND ? ORDER BY priority ASC LIMIT 25 OFFSET 50",
            vec![P::I(slice_hi)],
        ),
        // Round 7 P2 + Round 4 P1: spilled wide-accumulator hash GROUP BY must charge wide text
        // accumulators and never emit duplicate groups.
        (
            "wide_accum_groupby",
            "SELECT project_id, COUNT(*), MIN(description), MAX(description) FROM issues WHERE id BETWEEN 1 AND ? GROUP BY project_id ORDER BY project_id LIMIT 50",
            vec![P::I(slice_hi)],
        ),
        // Round 6 P2 + Round 8 P3: bounded top-K large LIMIT/OFFSET over wide rows.
        (
            "large_limit_wide_topk",
            "SELECT id, priority, title, description FROM issues WHERE id BETWEEN 1 AND ? ORDER BY priority DESC, id DESC LIMIT 200 OFFSET 100",
            vec![P::I(slice_hi)],
        ),
        // Round 5 P1: unbounded ORDER BY without LIMIT over wide rows (linear multi-pass merge).
        (
            "unbounded_wide_sort",
            "SELECT id, priority, title, description FROM issues WHERE id BETWEEN 1 AND ? ORDER BY updated_at ASC, id ASC",
            vec![P::I(slice_hi.min(1_200))],
        ),
        // Round 2: LEFT JOIN hash probe with interleaved NULL and non-NULL join keys.
        (
            "hash_join_nullable_due",
            "SELECT COUNT(*), COUNT(i2.id) FROM issues i1 LEFT JOIN issues i2 ON i1.due_at = i2.due_at AND i2.id BETWEEN 1 AND ? WHERE i1.id BETWEEN 1 AND ?",
            vec![P::I(slice_hi.min(400)), P::I(slice_hi.min(400))],
        ),
    ];

    let mut spill_failures = Vec::new();
    let mut reopen_failures = Vec::new();
    let mut reopen_meta = serde_json::Map::new();

    if target_name == "sqlite" {
        match (
            rusqlite::Connection::open(&db_path),
            rusqlite::Connection::open(&db_path),
        ) {
            (Ok(warm), Ok(tight)) => {
                let _ = warm.execute_batch("PRAGMA cache_size = 4000;");
                let _ = tight.execute_batch("PRAGMA cache_size = 48;");
                for (name, sql, params) in &spill_queries {
                    match (
                        direct_query_sqlite(&warm, sql, params),
                        direct_query_sqlite(&tight, sql, params),
                    ) {
                        (Ok(a), Ok(b)) if a == b => {}
                        (Ok(a), Ok(b)) => spill_failures.push(format!(
                            "{name}: warm ({} rows) != tight-cache ({} rows)",
                            a.len(),
                            b.len()
                        )),
                        (Err(e), _) => spill_failures.push(format!("{name} warm err: {e:#}")),
                        (_, Err(e)) => spill_failures.push(format!("{name} tight err: {e:#}")),
                    }
                }
                match snapshot_from_query(|sql| direct_query_sqlite(&warm, sql, &[])) {
                    Ok(reopened) => verify_reopen_snapshot(
                        &reopened,
                        pre_close,
                        shape,
                        &mut reopen_meta,
                        &mut reopen_failures,
                    ),
                    Err(e) => reopen_failures.push(format!("sqlite reopen err: {e:#}")),
                }
                match direct_query_sqlite(&warm, "PRAGMA integrity_check", &[]) {
                    Ok(rows)
                        if rows
                            .first()
                            .and_then(|r| r.first())
                            .and_then(|c| c.as_deref())
                            == Some("ok") => {}
                    Ok(rows) => reopen_failures.push(format!(
                        "sqlite cold reopen integrity_check failed: {rows:?}"
                    )),
                    Err(e) => reopen_failures.push(format!("sqlite integrity_check err: {e:#}")),
                }
            }
            (Err(e), _) | (_, Err(e)) => {
                spill_failures.push(format!("open sqlite: {e:#}"));
                reopen_failures.push(format!("open sqlite: {e:#}"));
            }
        }
    } else {
        let is_pg = target_name == "amphora-pg";
        let open_db = |pages: i64| -> anyhow::Result<Db> {
            let db = if is_pg {
                Db::open_postgres_sharing(&db_path, &Sharing::Off)?
            } else {
                Db::open_sharing(&db_path, &Sharing::Off)?
            };
            db.keep_pages(pages)?;
            Ok(db)
        };
        match (open_db(4000), open_db(48)) {
            (Ok(mut warm), Ok(mut tight)) => {
                for (name, sql, params) in &spill_queries {
                    match (
                        direct_query_amphora(&mut warm, sql, params),
                        direct_query_amphora(&mut tight, sql, params),
                    ) {
                        (Ok(a), Ok(b)) if a == b => {}
                        (Ok(a), Ok(b)) => spill_failures.push(format!(
                            "{name}: warm ({} rows) != tight-cache=48 ({} rows)",
                            a.len(),
                            b.len()
                        )),
                        (Err(e), _) => spill_failures.push(format!("{name} warm err: {e:#}")),
                        (_, Err(e)) => spill_failures.push(format!("{name} tight err: {e:#}")),
                    }
                }
                // Cold-reopen check: verify integrity, exact pre-close snapshot durability, and
                // (on amphora-pg) durable pg_class OIDs surviving reopen & ALTER TABLE RENAME!
                match snapshot_from_query(|sql| direct_query_amphora(&mut warm, sql, &[])) {
                    Ok(reopened) => verify_reopen_snapshot(
                        &reopened,
                        pre_close,
                        shape,
                        &mut reopen_meta,
                        &mut reopen_failures,
                    ),
                    Err(e) => reopen_failures.push(format!("cold reopen snapshot err: {e:#}")),
                }
                match direct_query_amphora(&mut warm, "PRAGMA integrity_check", &[]) {
                    Ok(rows)
                        if rows
                            .first()
                            .and_then(|r| r.first())
                            .and_then(|c| c.as_deref())
                            == Some("ok") => {}
                    Ok(rows) => reopen_failures
                        .push(format!("cold reopen integrity_check failed: {rows:?}")),
                    Err(e) => {
                        reopen_failures.push(format!("cold reopen integrity_check err: {e:#}"))
                    }
                }
                // Round 9 P2 #4: whole-table COUNT(expr) whose NOT NULL arithmetic overflows to
                // NaN/NULL must return 0 on both SQLite and PostgreSQL dialects.
                match direct_query_amphora(
                    &mut warm,
                    "SELECT COUNT(((price_cents * 1.0 + 1e308) * 2.0) - ((price_cents * 1.0 + 1e308) * 2.0)) FROM products",
                    &[],
                ) {
                    Ok(rows)
                        if rows
                            .first()
                            .and_then(|r| r.first())
                            .and_then(|c| c.as_deref())
                            == Some("0") => {}
                    Ok(rows) => reopen_failures.push(format!(
                        "COUNT(NaN overflow expr) returned {rows:?}, expected [[\"0\"]]"
                    )),
                    Err(e) => reopen_failures.push(format!("COUNT(NaN overflow expr) err: {e:#}")),
                }
                // Round 9 P2 #5: backup::digest must ignore shadowing TEMP tables on both dialects.
                {
                    let digest_check = (|| -> anyhow::Result<()> {
                        let d_before = amphora::backup::digest(&warm)?;
                        warm.exec(
                            "CREATE TEMP TABLE products (id INTEGER PRIMARY KEY, category_id BIGINT, price_cents BIGINT)",
                        )?;
                        warm.exec("INSERT INTO temp.products (id, category_id, price_cents) VALUES (999999, 1, 12345)")?;
                        let d_shadowed = amphora::backup::digest(&warm)?;
                        warm.exec("DROP TABLE temp.products")?;
                        if d_before != d_shadowed {
                            anyhow::bail!(
                                "backup::digest changed under shadowing temp table: {d_before:?} != {d_shadowed:?}"
                            );
                        }
                        Ok(())
                    })();
                    if let Err(e) = digest_check {
                        reopen_failures.push(format!("backup::digest temp shadow check: {e:#}"));
                    }
                }
                if !is_pg {
                    // Round 9 P1 #2 (SQLite): grouped MAX with bare columns must match the winning row.
                    let bare_check = (|| -> anyhow::Result<()> {
                        let got = direct_query_amphora(
                            &mut warm,
                            "SELECT category_id, id, price_cents, MAX(price_cents * 1000000000.0 + id) FROM products GROUP BY category_id ORDER BY category_id LIMIT 20",
                            &[],
                        )?;
                        let expected = direct_query_amphora(
                            &mut warm,
                            "SELECT p.category_id, p.id, p.price_cents, p.price_cents * 1000000000.0 + p.id FROM products p WHERE (p.price_cents * 1000000000.0 + p.id) = (SELECT MAX(p2.price_cents * 1000000000.0 + p2.id) FROM products p2 WHERE p2.category_id = p.category_id) ORDER BY p.category_id LIMIT 20",
                            &[],
                        )?;
                        if got != expected {
                            anyhow::bail!(
                                "SQLite grouped MAX bare columns diverged: got={got:?} expected={expected:?}"
                            );
                        }
                        Ok(())
                    })();
                    if let Err(e) = bare_check {
                        reopen_failures.push(format!("SQLite grouped MAX bare check: {e:#}"));
                    }
                    // Round 9 P1 #1 (SQLite): a view returning literal 'pg_class' must reopen cleanly as SQLite.
                    let view_lit_check = (|| -> anyhow::Result<()> {
                        warm.exec("DROP VIEW IF EXISTS __oracle_pg_lit_v")?;
                        warm.exec("CREATE VIEW __oracle_pg_lit_v AS SELECT 'pg_class' AS tag")?;
                        let rows = direct_query_amphora(
                            &mut warm,
                            "SELECT tag FROM __oracle_pg_lit_v",
                            &[],
                        )?;
                        if rows != vec![vec![Some("pg_class".to_owned())]] {
                            anyhow::bail!("unexpected view literal rows: {rows:?}");
                        }
                        warm.exec("DROP VIEW __oracle_pg_lit_v")?;
                        Ok(())
                    })();
                    if let Err(e) = view_lit_check {
                        reopen_failures.push(format!("SQLite pg_class literal view check: {e:#}"));
                    }
                }
                if is_pg {
                    // Round 9 P1 #2 (Postgres): ungrouped bare non-aggregate columns must be rejected.
                    if direct_query_amphora(
                        &mut warm,
                        "SELECT category_id, price_cents, MAX(id) FROM products GROUP BY category_id",
                        &[],
                    )
                    .is_ok()
                    {
                        reopen_failures.push(
                            "amphora-pg unexpectedly accepted ungrouped bare column in GROUP BY"
                                .into(),
                        );
                    }
                    // Round 9 P2 #6 (Postgres): internal __amphora_* tables must never appear in pg_class.
                    match direct_query_amphora(
                        &mut warm,
                        "SELECT relname FROM pg_class WHERE relname LIKE '__amphora_%'",
                        &[],
                    ) {
                        Ok(rows) if rows.is_empty() => {}
                        Ok(rows) => reopen_failures.push(format!(
                            "amphora-pg leaked internal __amphora_* tables into pg_class: {rows:?}"
                        )),
                        Err(e) => {
                            reopen_failures.push(format!("pg_class internal leak check err: {e:#}"))
                        }
                    }
                    // Round 8 P2 (Postgres): two TEMP tables in one session must receive distinct OIDs.
                    let temp_oid_check = (|| -> anyhow::Result<()> {
                        warm.exec(
                            "CREATE TEMP TABLE __oracle_tmp_a (id INTEGER PRIMARY KEY, v BIGINT)",
                        )?;
                        warm.exec(
                            "CREATE TEMP TABLE __oracle_tmp_b (id INTEGER PRIMARY KEY, v BIGINT)",
                        )?;
                        warm.exec("INSERT INTO __oracle_tmp_a VALUES (1, 11)")?;
                        warm.exec("INSERT INTO __oracle_tmp_b VALUES (1, 22)")?;
                        let sqls = direct_query_amphora(
                            &mut warm,
                            "SELECT name, sql FROM sqlite_temp_schema WHERE type = 'table' AND name IN ('__oracle_tmp_a', '__oracle_tmp_b') ORDER BY name",
                            &[],
                        )?;
                        let vals = direct_query_amphora(
                            &mut warm,
                            "SELECT (SELECT v FROM __oracle_tmp_a WHERE id = 1), (SELECT v FROM __oracle_tmp_b WHERE id = 1)",
                            &[],
                        )?;
                        warm.exec("DROP TABLE __oracle_tmp_b")?;
                        warm.exec("DROP TABLE __oracle_tmp_a")?;
                        if vals != vec![vec![Some("11".to_owned()), Some("22".to_owned())]] {
                            anyhow::bail!("temp table values collided: {vals:?}");
                        }
                        if sqls.len() == 2 {
                            let s0 = sqls[0].get(1).and_then(|v| v.as_deref()).unwrap_or("");
                            let s1 = sqls[1].get(1).and_then(|v| v.as_deref()).unwrap_or("");
                            fn parse_oid(s: &str) -> Option<&str> {
                                s.split("pg_class.oid ")
                                    .nth(1)
                                    .and_then(|rest| rest.split_whitespace().next())
                            }
                            let (oid0, oid1) = (parse_oid(s0), parse_oid(s1));
                            if oid0.is_none() || oid1.is_none() || oid0 == oid1 {
                                anyhow::bail!(
                                    "temp tables did not receive distinct pg_class.oid markers: {s0:?} vs {s1:?}"
                                );
                            }
                        }
                        Ok(())
                    })();
                    if let Err(e) = temp_oid_check {
                        reopen_failures.push(format!("amphora-pg temp table OID check: {e:#}"));
                    }
                    let oid_before = direct_query_amphora(
                        &mut warm,
                        "SELECT oid, relname FROM pg_class WHERE relname = 'issues'",
                        &[],
                    );
                    drop(tight);
                    drop(warm);
                    match open_db(2000) {
                        Ok(mut reopened) => {
                            let oid_after = direct_query_amphora(
                                &mut reopened,
                                "SELECT oid, relname FROM pg_class WHERE relname = 'issues'",
                                &[],
                            );
                            match (oid_before, oid_after) {
                                (Ok(a), Ok(b)) if !a.is_empty() && a == b => {
                                    reopen_meta.insert("pg_class_issues_oid".into(), serde_json::json!(b));
                                }
                                (Ok(a), Ok(b)) => reopen_failures.push(format!(
                                    "pg_class OID drifted across cold reopen: before={a:?}, after={b:?}"
                                )),
                                (Err(e), _) | (_, Err(e)) => {
                                    reopen_failures.push(format!("pg_class OID check err: {e:#}"))
                                }
                            }
                            // Verify OID stability across ALTER TABLE RENAME on a scratch table
                            let r_oid_test = (|| -> anyhow::Result<()> {
                                reopened.exec("DROP TABLE IF EXISTS __oid_reopen_b")?;
                                reopened.exec("DROP TABLE IF EXISTS __oid_reopen_a")?;
                                reopened.exec("CREATE TABLE __oid_reopen_a (id INTEGER PRIMARY KEY, v BIGINT)")?;
                                let o1 = direct_query_amphora(
                                    &mut reopened,
                                    "SELECT oid FROM pg_class WHERE relname = '__oid_reopen_a'",
                                    &[],
                                )?;
                                reopened
                                    .exec("ALTER TABLE __oid_reopen_a RENAME TO __oid_reopen_b")?;
                                let o2 = direct_query_amphora(
                                    &mut reopened,
                                    "SELECT oid FROM pg_class WHERE relname = '__oid_reopen_b'",
                                    &[],
                                )?;
                                reopened.exec("DROP TABLE __oid_reopen_b")?;
                                if o1.is_empty() || o1 != o2 {
                                    anyhow::bail!(
                                        "OID changed on ALTER TABLE RENAME: {o1:?} -> {o2:?}"
                                    );
                                }
                                Ok(())
                            })();
                            if let Err(e) = r_oid_test {
                                reopen_failures.push(format!("pg_class rename OID err: {e:#}"));
                            }
                        }
                        Err(e) => reopen_failures.push(format!("second cold reopen err: {e:#}")),
                    }
                }
            }
            (Err(e), _) | (_, Err(e)) => {
                spill_failures.push(format!("open amphora: {e:#}"));
                reopen_failures.push(format!("open amphora: {e:#}"));
            }
        }
    }

    (
        OracleReport {
            passed: spill_failures.is_empty(),
            details: serde_json::json!({
                "queries_checked": spill_queries.len(),
                "warm_pages": 4000,
                "tight_pages": 48,
                "failures": spill_failures,
            }),
        },
        OracleReport {
            passed: reopen_failures.is_empty(),
            details: serde_json::json!({
                "meta": reopen_meta,
                "failures": reopen_failures,
            }),
        },
    )
}
