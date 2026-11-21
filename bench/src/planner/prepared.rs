//! TPC-H's 22 queries prepared once and run with qgen's parameters (PL.12, AC-PL.12.1;
//! bench/planner/README.md, "Prepared statements"). `--prepared <dir>` names the templates
//! (bench/planner/tpch/prepared, each query's substitution parameters as `?N`) and `--params
//! <jsonl>` the values qgen drew for them (`bench/planner/tpch/qgen_params.py`, one line a query
//! and seed). Each query is prepared once for each plan cache mode `--plan-cache-modes` names
//! (generic, the plan made at prepare whatever is bound; auto, custom plans while they pay for
//! their planning, then the generic one; custom), and every parameter set runs on each, `--runs`
//! passes over them, the modes' order alternating run by run so neither always runs on the
//! other's warmed cache. Each query's record keeps, for each mode, each seed's best time, their
//! sum and the custom plans the statement made, and the seeds whose answers differ between modes.

use std::time::Duration;

use amphora_engine::{PlanCacheMode, Value};

use super::engines::{Amphora, Outcome};

/// A plan cache mode by the name `--plan-cache-modes` gives it.
pub fn mode(name: &str) -> anyhow::Result<PlanCacheMode> {
    match name {
        "generic" => Ok(PlanCacheMode::ForceGeneric),
        "auto" => Ok(PlanCacheMode::Auto),
        "custom" => Ok(PlanCacheMode::ForceCustom),
        other => anyhow::bail!("--plan-cache-modes takes generic, auto and custom, not {other}"),
    }
}

/// One query's parameter sets, by seed, in the order `--params` lists them.
#[derive(Debug)]
pub struct Params {
    pub seeds: Vec<(u64, Vec<Value>)>,
}

/// `--params`' lines for query `query`: `{"query": "q06", "seed": 3, "params": [...]}`, a JSON
/// integer bound as an INTEGER, any other number as a REAL, a string as TEXT.
pub fn params(text: &str, query: &str) -> anyhow::Result<Params> {
    let mut seeds = Vec::new();
    for (n, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
        let r: serde_json::Value = serde_json::from_str(line).map_err(|e| anyhow::anyhow!("--params line {}: {e}", n + 1))?;
        if r["query"].as_str() != Some(query) {
            continue;
        }
        let seed = r["seed"].as_u64().ok_or_else(|| anyhow::anyhow!("--params line {}: no seed", n + 1))?;
        let list = r["params"].as_array().ok_or_else(|| anyhow::anyhow!("--params line {}: no params", n + 1))?;
        let mut values = Vec::with_capacity(list.len());
        for v in list {
            values.push(match v {
                serde_json::Value::Number(x) => match x.as_i64() {
                    Some(i) => Value::from_i64(i),
                    None => Value::from_f64(x.as_f64().ok_or_else(|| anyhow::anyhow!("--params line {}: {x} is no number", n + 1))?),
                },
                serde_json::Value::String(s) => Value::from_text(s.clone()),
                other => anyhow::bail!("--params line {}: {other} is neither a number nor a string", n + 1),
            });
        }
        seeds.push((seed, values));
    }
    Ok(Params { seeds })
}

/// One mode's runs of one query.
struct ModeRuns {
    name: String,
    mode: PlanCacheMode,
    stmt: amphora_engine::Statement,
    /// Each seed's best time over the passes; None while no run of it finished.
    best_ms: Vec<Option<f64>>,
    /// Each seed's first finished run: its rows and first row, to compare between modes.
    answer: Vec<Option<(u64, Option<String>)>>,
    /// The first run that timed out or failed, which ends the mode's runs.
    stopped: Option<String>,
}

/// Runs `sql`, prepared once for each of `modes`, with each of `params`' sets, `runs` passes over
/// them, and returns the query's record.
pub fn query(amphora: &mut Amphora, name: &str, sql: &str, params: &Params, modes: &[String], runs: usize, deadline: Duration) -> anyhow::Result<serde_json::Value> {
    let mut each = Vec::with_capacity(modes.len());
    for m in modes {
        let stmt = match amphora.prepare(sql) {
            Ok(stmt) => stmt,
            Err(e) => return Ok(serde_json::json!({ "record": "prepared", "query": name, "failed": e.to_string() })),
        };
        each.push(ModeRuns { name: m.clone(), mode: mode(m)?, stmt, best_ms: vec![None; params.seeds.len()], answer: vec![None; params.seeds.len()], stopped: None });
    }
    for pass in 0..runs {
        for (i, (_, values)) in params.seeds.iter().enumerate() {
            // The modes' order turns with each run: the first mode of one run is the last of the next.
            let turn = (pass + i) % each.len().max(1);
            for k in 0..each.len() {
                let Some(m) = each.get_mut((k + turn) % each.len()) else { continue };
                if m.stopped.is_some() {
                    continue;
                }
                match amphora.run_prepared(&mut m.stmt, values, m.mode, deadline) {
                    Outcome::Done(run) => {
                        let ms = run.prepare_ms + run.exec_ms;
                        if let Some(best) = m.best_ms.get_mut(i) {
                            *best = Some(best.map_or(ms, |b| b.min(ms)));
                        }
                        if let Some(answer) = m.answer.get_mut(i) {
                            if answer.is_none() {
                                *answer = Some((run.rows, run.first));
                            }
                        }
                    }
                    Outcome::TimedOut => m.stopped = Some("timed out".into()),
                    Outcome::Failed(e) => m.stopped = Some(e),
                }
            }
        }
    }
    let seeds: Vec<u64> = params.seeds.iter().map(|(s, _)| *s).collect();
    // A seed whose answers (its rows' count and first row) differ between the modes that ran it.
    let differ: Vec<u64> = (0..seeds.len())
        .filter(|&i| {
            let answers: Vec<_> = each.iter().filter_map(|m| m.answer.get(i).cloned().flatten()).collect();
            answers.windows(2).any(|w| matches!(w, [a, b] if a != b))
        })
        .filter_map(|i| seeds.get(i).copied())
        .collect();
    let mut modes_json = serde_json::Map::new();
    let mut totals = Vec::new();
    for m in &each {
        let finished = m.best_ms.iter().all(Option::is_some);
        let total: f64 = m.best_ms.iter().flatten().sum();
        totals.push((m.name.clone(), finished.then_some(total)));
        modes_json.insert(
            m.name.clone(),
            serde_json::json!({
                "best_ms": m.best_ms,
                "total_ms": finished.then_some(total),
                "custom_plans": m.stmt.metrics().custom_plans,
                "stopped": m.stopped,
            }),
        );
    }
    let generic = totals.iter().find(|(n, _)| n == "generic").and_then(|(_, t)| *t);
    let auto = totals.iter().find(|(n, _)| n == "auto").and_then(|(_, t)| *t);
    Ok(serde_json::json!({
        "record": "prepared",
        "query": name,
        "seeds": seeds,
        "runs": runs,
        "modes": modes_json,
        "auto_over_generic": generic.zip(auto).map(|(g, a)| if g > 0.0 { a / g } else { f64::NAN }),
        "differ": differ,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-PL.12.7: do: read `--params` lines for q06 among others' and a malformed
    /// value. Expect: q06's sets alone, in order, integers as INTEGER, decimals as REAL, dates as
    /// TEXT; a value neither number nor string refused with its line.
    #[test]
    fn t_pl_12_7_params_bind_by_their_json_type() {
        let text = concat!(
            r#"{"query": "q06", "seed": 1, "params": ["1994-01-01", "1995-01-01", 0.05, 0.07, 24]}"#,
            "\n",
            r#"{"query": "q07", "seed": 1, "params": ["FRANCE", "GERMANY"]}"#,
            "\n",
            r#"{"query": "q06", "seed": 2, "params": ["1995-01-01", "1996-01-01", 0.02, 0.04, 25]}"#,
            "\n"
        );
        let p = params(text, "q06").unwrap();
        assert_eq!(p.seeds.iter().map(|(s, _)| *s).collect::<Vec<_>>(), vec![1, 2]);
        let (_, first) = &p.seeds[0];
        assert_eq!(first[0], Value::from_text("1994-01-01".to_owned()));
        assert_eq!(first[2], Value::from_f64(0.05));
        assert_eq!(first[4], Value::from_i64(24));
        let bad = r#"{"query": "q06", "seed": 1, "params": [null]}"#;
        assert!(params(bad, "q06").unwrap_err().to_string().contains("line 1"));
        assert!(mode("generic").is_ok() && mode("auto").is_ok() && mode("bogus").is_err());
    }
}
