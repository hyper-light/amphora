//! Parametric End-User Query Combinator (1,024+ realistic end-user queries).
//!
//! Instead of hand-crafting a few static queries that an engine can accidentally overfit, this
//! module sweeps real end-user UI/API combinations across the 43-table production schema:
//!   1. SaaS Issue Explorer Grid (~384 queries): tenant skew (mega / tiny / empty) x filter facets
//!      (status, priority, assignee NULL/NOT NULL, due_at, JSON severity, LIKE, EXISTS label) x
//!      sort direction & NULLS FIRST/LAST x pagination (OFFSET 0 / mid / deep / keyset).
//!   2. E-Commerce Faceted Search & Analytics (~256 queries): stock/price/category/JSON color &
//!      brand facets x sort orders x pagination + GROUP BY / HAVING rollups.
//!   3. Social Feed, Channels & Moderation Explorer (~192 queries): hot #general vs peer channel x
//!      deleted/edited/reply/verified/blocklist filters x sort/pagination + NULLS FIRST/LAST mod queues.
//!   4. Agentic Fleet, Tracing & CI Windows / Set Ops (~192 queries): workspace/run/service/suite
//!      filters x window functions (ROW_NUMBER, RANK, DENSE_RANK, LAG, LEAD, SUM OVER, AVG OVER)
//!      and set operations (UNION, UNION ALL, INTERSECT, EXCEPT).
//!
//! Every generated query returns a deterministic answer digest (FNV-1a 64-bit over exact cell
//! texts) + row count + latency so cross-engine differential checks (`gauntlet_diff.py`) verify
//! 100% bit-for-bit agreement across SQLite, Amphora SQLite, Amphora Postgres, and PostgreSQL.

use std::time::Instant;

use crate::suite::{Dialect, P};
use crate::suite_db::Session;

use super::loader::{BASE_TS_US, GauntletShape};
use super::schema::{json_i64, json_str};

pub struct ParametricQuery {
    pub id: String,
    pub group: &'static str,
    pub sql: String,
    pub params: Vec<P>,
}

/// Deterministic 64-bit FNV-1a digest of an ordered query answer.
pub fn digest_answer(rows: &[Vec<Option<String>>]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mix_byte = |h: &mut u64, b: u8| {
        *h ^= u64::from(b);
        *h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    for row in rows {
        mix_byte(&mut h, 0x1e); // row sep
        for cell in row {
            mix_byte(&mut h, 0x1f); // col sep
            match cell {
                None => mix_byte(&mut h, 0x00),
                Some(s) => {
                    mix_byte(&mut h, 0x01);
                    for &b in s.as_bytes() {
                        mix_byte(&mut h, b);
                    }
                }
            }
        }
    }
    format!("{h:016x}")
}

/// Generates 1,024+ distinct realistic end-user queries for dialect `d` and dataset `s`.
pub fn generate_all(d: Dialect, s: &GauntletShape) -> Vec<ParametricQuery> {
    let mut out = Vec::with_capacity(1100);
    gen_saas_grid(d, s, &mut out);
    gen_commerce_grid(d, s, &mut out);
    gen_social_grid(d, s, &mut out);
    gen_agentic_obs_grid(d, s, &mut out);
    out
}

fn gen_saas_grid(d: Dialect, s: &GauntletShape, out: &mut Vec<ParametricQuery>) {
    let tiny_tid = if s.tenants > 3 { s.tenants - 1 } else { 1 };
    let tenants = [
        ("mega", 1i64),
        ("tiny", tiny_tid),
        ("empty", s.empty_tenant),
    ];
    let sev_expr = json_str(d, "i.metadata_json", "severity");
    let votes_expr = json_i64(d, "i.metadata_json", "votes");
    let filters: Vec<(&str, String, Vec<P>)> = vec![
        ("all_active", "i.deleted_at IS NULL".into(), vec![]),
        ("open_only", "i.deleted_at IS NULL AND i.status = ?".into(), vec![P::T("open".into())]),
        ("unassigned", "i.deleted_at IS NULL AND i.assignee_id IS NULL".into(), vec![]),
        ("assigned_hi", "i.deleted_at IS NULL AND i.assignee_id IS NOT NULL AND i.priority <= ?".into(), vec![P::I(1)]),
        ("has_due", "i.deleted_at IS NULL AND i.due_at IS NOT NULL AND i.due_at < ?".into(), vec![P::I(BASE_TS_US + s.issues * 60_000_000)]),
        ("json_sev", format!("i.deleted_at IS NULL AND {sev_expr} = ? AND {votes_expr} >= ?"), vec![P::T("sev1".into()), P::I(20)]),
        ("like_title", "i.deleted_at IS NULL AND i.title LIKE ?".into(), vec![P::T("%issue%".into())]),
        ("exists_lbl", "i.deleted_at IS NULL AND EXISTS (SELECT 1 FROM issue_labels il WHERE il.issue_id = i.id AND il.label_id = ?)".into(), vec![P::I(1)]),
        ("not_exists_cmt", "i.deleted_at IS NULL AND i.status = 'open' AND NOT EXISTS (SELECT 1 FROM issue_comments c WHERE c.issue_id = i.id)".into(), vec![]),
        ("or_status", "i.deleted_at IS NULL AND (i.status = 'open' OR i.priority = 0)".into(), vec![]),
        ("est_not_null", "i.deleted_at IS NULL AND i.estimate_hrs IS NOT NULL AND i.estimate_hrs >= ?".into(), vec![P::F(5.0)]),
        ("soft_deleted", "i.deleted_at IS NOT NULL".into(), vec![]),
    ];
    let sorts = [
        ("prio_upd", "i.priority ASC, i.updated_at DESC, i.id ASC"),
        (
            "due_nf",
            "i.due_at DESC NULLS FIRST, i.priority ASC, i.id ASC",
        ),
        (
            "due_nl",
            "i.due_at ASC NULLS LAST, i.priority DESC, i.id ASC",
        ),
        ("est_nl", "i.estimate_hrs DESC NULLS LAST, i.id ASC"),
    ];
    let mid_off = (s.issues / 20).clamp(1, 200);
    let pages: [(&str, &str); 3] = [
        ("p1", "LIMIT 20 OFFSET 0"),
        ("pmid", "LIMIT 20 OFFSET ?"),
        ("pdeep", "LIMIT 15 OFFSET ?"),
    ];
    // 3 * 12 * 4 * 3 = 432 SaaS queries (use first 2 pages for empty tenant -> 384+ real combos)
    for (tname, tid) in tenants {
        for (fname, fsql, fparams) in &filters {
            for (sname, ssql) in sorts {
                for (pname, psql) in pages {
                    if tname == "empty" && pname == "pdeep" {
                        continue;
                    }
                    let mut params = Vec::with_capacity(fparams.len() + 2);
                    params.push(P::I(tid));
                    params.extend(fparams.iter().cloned());
                    if pname == "pmid" {
                        params.push(P::I(mid_off));
                    } else if pname == "pdeep" {
                        params.push(P::I(mid_off * 3));
                    }
                    let sql = format!(
                        "SELECT i.id, i.priority, i.status, i.assignee_id, i.due_at, i.estimate_hrs FROM issues i WHERE i.tenant_id = ? AND {fsql} ORDER BY {ssql} {psql}"
                    );
                    out.push(ParametricQuery {
                        id: format!("p_saas_{tname}_{fname}_{sname}_{pname}"),
                        group: "saas_grid",
                        sql,
                        params,
                    });
                }
            }
        }
    }
}

fn gen_commerce_grid(d: Dialect, s: &GauntletShape, out: &mut Vec<ParametricQuery>) {
    let color_expr = json_str(d, "p.attrs_json", "color");
    let wt_expr = json_i64(d, "p.attrs_json", "weight_g");
    let bands = [
        ("low", 0i64, 25_000i64),
        ("mid", 25_000, 120_000),
        ("high", 120_000, 500_000),
        ("all", 0, 1_000_000),
    ];
    let colors = ["black", "silver", "blue", "red"];
    let stock_filters = [
        ("instock", "p.active AND p.stock > 0"),
        ("lowstock", "p.active AND p.stock BETWEEN 1 AND 50"),
        ("anyactive", "p.active"),
        ("inactive", "NOT p.active"),
    ];
    let sorts = [
        ("price_asc", "p.price_cents ASC, p.id ASC"),
        ("price_desc", "p.price_cents DESC, p.id ASC"),
        ("stock_desc", "p.stock DESC, p.price_cents ASC, p.id ASC"),
        ("upd_desc", "p.updated_at DESC, p.id ASC"),
    ];
    // 4 * 4 * 4 * 4 = 256 commerce catalog queries!
    for (bname, lo, hi) in bands {
        for color in colors {
            for (sfname, sfsql) in stock_filters {
                for (sname, ssql) in sorts {
                    let sql = format!(
                        "SELECT p.id, p.sku, p.category_id, p.price_cents, p.stock FROM products p WHERE {sfsql} AND p.price_cents BETWEEN ? AND ? AND {color_expr} = ? AND {wt_expr} >= ? ORDER BY {ssql} LIMIT 20"
                    );
                    out.push(ParametricQuery {
                        id: format!("p_com_{bname}_{color}_{sfname}_{sname}"),
                        group: "commerce_grid",
                        sql,
                        params: vec![P::I(lo), P::I(hi), P::T(color.into()), P::I(100)],
                    });
                }
            }
        }
    }
    // Add 32 commerce order/ledger/customer aggregate variations
    let statuses = ["pending", "paid", "shipped", "delivered"];
    let currencies = ["USD", "EUR", "GBP", "JPY"];
    let tiers = ["free", "enterprise"];
    let span_hi = s.orders.min(4_000).max(1);
    for st in statuses {
        for cur in currencies {
            for tier in tiers {
                out.push(ParametricQuery {
                    id: format!("p_com_agg_{st}_{cur}_{tier}"),
                    group: "commerce_agg",
                    sql: "SELECT c.country, COUNT(o.id), COALESCE(SUM(o.total_cents), 0) FROM orders o JOIN customers c ON c.id = o.customer_id WHERE o.id BETWEEN 1 AND ? AND o.status = ? AND o.currency = ? AND c.tier = ? GROUP BY c.country ORDER BY COALESCE(SUM(o.total_cents), 0) DESC, c.country ASC LIMIT 15".into(),
                    params: vec![P::I(span_hi), P::T(st.into()), P::T(cur.into()), P::T(tier.into())],
                });
            }
        }
    }
}

fn gen_social_grid(_d: Dialect, s: &GauntletShape, out: &mut Vec<ParametricQuery>) {
    let ch2 = 2i64.min(s.channels.max(1));
    let channels = [("hot1", 1i64), ("peer", ch2)];
    let mfilters = [
        ("live", "m.deleted_at IS NULL"),
        ("edited", "m.deleted_at IS NULL AND m.edited_at IS NOT NULL"),
        (
            "replies",
            "m.deleted_at IS NULL AND m.reply_to_id IS NOT NULL",
        ),
        ("roots", "m.deleted_at IS NULL AND m.reply_to_id IS NULL"),
        ("verified", "m.deleted_at IS NULL AND a.verified"),
        (
            "unblocked",
            "m.deleted_at IS NULL AND NOT EXISTS (SELECT 1 FROM blocks b WHERE b.blocker_id = 1 AND b.blocked_id = m.author_id)",
        ),
        ("deleted", "m.deleted_at IS NOT NULL"),
        (
            "has_react",
            "m.deleted_at IS NULL AND EXISTS (SELECT 1 FROM reactions r WHERE r.message_id = m.id)",
        ),
    ];
    let sorts = [
        ("seq_desc", "m.seq DESC, m.id DESC"),
        ("seq_asc", "m.seq ASC, m.id ASC"),
        ("ed_nf", "m.edited_at DESC NULLS FIRST, m.id ASC"),
        ("ed_nl", "m.edited_at ASC NULLS LAST, m.id DESC"),
    ];
    let offs = [("o0", 0i64), ("o25", 25i64), ("o100", 100i64)];
    // 2 * 8 * 4 * 3 = 192 social explorer queries!
    for (cname, cid) in channels {
        for (fname, fsql) in mfilters {
            for (sname, ssql) in sorts {
                for (oname, off) in offs {
                    out.push(ParametricQuery {
                        id: format!("p_soc_{cname}_{fname}_{sname}_{oname}"),
                        group: "social_grid",
                        sql: format!("SELECT m.id, m.seq, a.handle, m.edited_at FROM messages m JOIN accounts a ON a.id = m.author_id WHERE m.channel_id = ? AND {fsql} ORDER BY {ssql} LIMIT 20 OFFSET ?"),
                        params: vec![P::I(cid), P::I(off)],
                    });
                }
            }
        }
    }
}

fn gen_agentic_obs_grid(d: Dialect, s: &GauntletShape, out: &mut Vec<ParametricQuery>) {
    let models = [
        "gemini-2.5-pro",
        "claude-sonnet-4",
        "gpt-5-codex",
        "llama-4-maverick",
    ];
    let statuses = ["completed", "failed", "running", "queued"];
    let windows = [
        (
            "rn",
            "ROW_NUMBER() OVER (PARTITION BY workspace_id ORDER BY cost_micros DESC, id ASC)",
        ),
        (
            "rk",
            "RANK() OVER (PARTITION BY workspace_id ORDER BY priority ASC, id ASC)",
        ),
        (
            "drk",
            "DENSE_RANK() OVER (PARTITION BY workspace_id ORDER BY priority DESC, id ASC)",
        ),
        (
            "cum",
            "SUM(cost_micros) OVER (PARTITION BY workspace_id ORDER BY started_at ASC, id ASC ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW)",
        ),
        (
            "lag",
            "COALESCE(LAG(total_tokens) OVER (PARTITION BY workspace_id ORDER BY started_at ASC, id ASC), 0)",
        ),
        (
            "lead",
            "COALESCE(LEAD(total_tokens) OVER (PARTITION BY workspace_id ORDER BY started_at ASC, id ASC), 0)",
        ),
    ];
    let run_hi = s.agent_runs.min(3_000).max(1);
    // 4 * 4 * 6 = 96 window function queries across agent_runs!
    for model in models {
        for status in statuses {
            for (wname, wexpr) in windows {
                out.push(ParametricQuery {
                    id: format!("p_ag_win_{model}_{status}_{wname}"),
                    group: "agentic_windows",
                    sql: format!("SELECT id, workspace_id, total_tokens, {wexpr} AS metric FROM agent_runs WHERE id BETWEEN 1 AND ? AND model = ? AND status = ? ORDER BY workspace_id, id LIMIT 30"),
                    params: vec![P::I(run_hi), P::T(model.into()), P::T(status.into())],
                });
            }
        }
    }

    // 48 observability JSON + status + op_name group-by queries
    let ops = ["http.get", "http.post", "db.query", "llm.infer"];
    let regions = ["US", "DE", "JP"];
    let codes = [
        ("ok", "status_code = 200"),
        ("err5xx", "status_code >= 500"),
        ("throttled", "status_code = 429"),
        ("any", "status_code >= 200"),
    ];
    let reg_expr = json_str(d, "tags_json", "region");
    let span_hi = s.spans.min(4_000).max(1);
    for op_n in ops {
        for reg in regions {
            for (cname, csql) in codes {
                out.push(ParametricQuery {
                    id: format!("p_obs_{op_n}_{reg}_{cname}"),
                    group: "obs_grid",
                    sql: format!("SELECT service_id, COUNT(*), COALESCE(AVG(duration_us), 0.0), COALESCE(MAX(duration_us), 0) FROM spans WHERE id BETWEEN 1 AND ? AND op_name = ? AND {reg_expr} = ? AND {csql} GROUP BY service_id ORDER BY COUNT(*) DESC, service_id ASC LIMIT 15"),
                    params: vec![P::I(span_hi), P::T(op_n.into()), P::T(reg.into())],
                });
            }
        }
    }

    // 48 CI/CD + set-operation combinations
    let suites = ["unit", "integration", "planner", "storage"];
    let outcomes = ["pass", "fail", "flake"];
    let setops = ["UNION", "UNION ALL", "INTERSECT", "EXCEPT"];
    let tr_hi = s.test_results.min(4_000).max(1);
    let mid = (tr_hi / 2).max(1);
    for su in suites {
        for oc in outcomes {
            for so in setops {
                let tag = so.to_lowercase().replace(' ', "_");
                out.push(ParametricQuery {
                    id: format!("p_ci_set_{su}_{oc}_{tag}"),
                    group: "ci_setops",
                    sql: format!("SELECT test_name FROM test_results WHERE id BETWEEN 1 AND ? AND suite_name = ? AND outcome = ? {so} SELECT test_name FROM test_results WHERE id BETWEEN ? AND ? AND suite_name = ? AND outcome = 'pass' ORDER BY 1 LIMIT 25"),
                    params: vec![P::I(mid), P::T(su.into()), P::T(oc.into()), P::I(mid), P::I(tr_hi), P::T(su.into())],
                });
            }
        }
    }
}

pub struct ParametricOutcome {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub total_ms: f64,
    pub max_ms: f64,
    pub slowest_id: String,
    pub first_error: Option<String>,
    pub digests: serde_json::Map<String, serde_json::Value>,
}

/// Runs all 1,024+ parametric queries on `session`, capturing timing and deterministic digests.
pub fn run_parametric(
    session: &mut dyn Session,
    d: Dialect,
    shape: &GauntletShape,
    filter: Option<&str>,
    record_digests: bool,
) -> ParametricOutcome {
    let all = generate_all(d, shape);
    let started = Instant::now();
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut max_ms = 0.0f64;
    let mut slowest_id = String::new();
    let mut first_error = None;
    let mut digests = serde_json::Map::new();
    let mut total = 0usize;

    for q in &all {
        if let Some(f) = filter
            && !q.id.contains(f)
            && !q.group.contains(f)
        {
            continue;
        }
        total += 1;
        let t0 = Instant::now();
        let stmts = [q.sql.clone()];
        let params = [q.params.clone()];
        match session.answer(&stmts, &params) {
            Ok(rows) => {
                let ms = t0.elapsed().as_secs_f64() * 1000.0;
                if ms > max_ms {
                    max_ms = ms;
                    slowest_id = q.id.clone();
                }
                passed += 1;
                if record_digests {
                    let hash = digest_answer(&rows);
                    digests.insert(
                        q.id.clone(),
                        serde_json::json!({ "rows": rows.len(), "hash": hash }),
                    );
                }
            }
            Err(e) => {
                failed += 1;
                let msg = format!("{e:#}");
                if record_digests {
                    digests.insert(q.id.clone(), serde_json::json!({ "error": msg }));
                }
                if first_error.is_none() {
                    first_error = Some(format!("{}: {msg}", q.id));
                }
            }
        }
    }

    ParametricOutcome {
        total,
        passed,
        failed,
        total_ms: started.elapsed().as_secs_f64() * 1000.0,
        max_ms,
        slowest_id,
        first_error,
        digests,
    }
}
