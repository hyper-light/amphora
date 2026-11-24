//! A heap's storage parameters, as `CREATE TABLE ... WITH (...)` gives them (REL_19_0
//! src/backend/access/common/reloptions.c: `boolRelOpts`, `ternaryRelOpts`, `intRelOpts`, `realRelOpts` and
//! `enumRelOpts` of `RELOPT_KIND_HEAP`; `parse_one_reloption`). None of them changes an answer here: how rows are
//! placed, kept and cleaned is the versioned storage's own.

use amphora_pg_parser::tree::{Node, NodeId, Tree};

use crate::func::parse_bool;
use crate::{Error, sqlstate};

/// What a parameter's value may be.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Bool,
    Int(i32, i32),
    Real(f64, f64),
    Enum(&'static [&'static str]),
}

/// A heap's parameters by name; `toast_tuple_target`'s highest is `MaximumBytesPerTuple(1)` at 8 kB blocks
/// (src/include/access/heaptoast.h), `vacuum_index_cleanup`'s values `StdRdOptIndexCleanupValues`.
const HEAP: &[(&str, Kind)] = &[
    ("autovacuum_enabled", Kind::Bool),
    ("user_catalog_table", Kind::Bool),
    ("vacuum_truncate", Kind::Bool),
    ("fillfactor", Kind::Int(10, 100)),
    ("autovacuum_parallel_workers", Kind::Int(-1, 1024)),
    ("autovacuum_vacuum_threshold", Kind::Int(0, i32::MAX)),
    ("autovacuum_vacuum_max_threshold", Kind::Int(-1, i32::MAX)),
    ("autovacuum_vacuum_insert_threshold", Kind::Int(-1, i32::MAX)),
    ("autovacuum_analyze_threshold", Kind::Int(0, i32::MAX)),
    ("autovacuum_vacuum_cost_limit", Kind::Int(1, 10_000)),
    ("autovacuum_freeze_min_age", Kind::Int(0, 1_000_000_000)),
    ("autovacuum_multixact_freeze_min_age", Kind::Int(0, 1_000_000_000)),
    ("autovacuum_freeze_max_age", Kind::Int(100_000, 2_000_000_000)),
    ("autovacuum_multixact_freeze_max_age", Kind::Int(10_000, 2_000_000_000)),
    ("autovacuum_freeze_table_age", Kind::Int(0, 2_000_000_000)),
    ("autovacuum_multixact_freeze_table_age", Kind::Int(0, 2_000_000_000)),
    ("log_autovacuum_min_duration", Kind::Int(-1, i32::MAX)),
    ("log_autoanalyze_min_duration", Kind::Int(-1, i32::MAX)),
    ("toast_tuple_target", Kind::Int(128, 8160)),
    ("parallel_workers", Kind::Int(0, 1024)),
    ("autovacuum_vacuum_cost_delay", Kind::Real(0.0, 100.0)),
    ("autovacuum_vacuum_scale_factor", Kind::Real(0.0, 100.0)),
    ("autovacuum_vacuum_insert_scale_factor", Kind::Real(0.0, 100.0)),
    ("autovacuum_analyze_scale_factor", Kind::Real(0.0, 100.0)),
    ("vacuum_max_eager_freeze_failure_rate", Kind::Real(0.0, 1.0)),
    (
        "vacuum_index_cleanup",
        Kind::Enum(&["auto", "on", "off", "true", "false", "yes", "no", "1", "0"]),
    ),
];

/// `defGetString` (src/backend/commands/define.c) of a parameter, whose value is `true` where it has none.
fn text(tree: &Tree, arg: Option<NodeId>) -> Result<String, Error> {
    let Some(arg) = arg else {
        return Ok("true".to_owned());
    };
    match tree.get(arg)? {
        Node::Integer(i) => Ok(i.to_string()),
        Node::Float(text) | Node::String(text) => Ok(text.clone()),
        _ => Err(Error::unsupported("this storage parameter's value")),
    }
}

fn refuse(message: String) -> Error {
    Error::new(sqlstate::INVALID_PARAMETER_VALUE, message)
}

/// `parse_int`'s reading of a parameter's text: an integer, else a number rounded to the nearest.
fn integer(text: &str) -> Option<i32> {
    let text = text.trim();
    if let Ok(i) = text.parse::<i32>() {
        return Some(i);
    }
    let real = text.parse::<f64>().ok().filter(|r| r.is_finite())?;
    format!("{:.0}", real.round_ties_even()).parse().ok()
}

/// The parameters of a CREATE TABLE's WITH, checked as 19 checks them.
pub(crate) fn heap(tree: &Tree, options: &[NodeId]) -> Result<(), Error> {
    let mut given: Vec<&str> = Vec::with_capacity(options.len());
    for option in options {
        let Node::DefElem {
            defnamespace: None,
            defname,
            arg,
            defaction: 0,
            ..
        } = tree.get(*option)?
        else {
            return Err(Error::unsupported("this storage parameter"));
        };
        let Some((name, kind)) = HEAP.iter().find(|(name, _)| name == defname) else {
            return Err(refuse(format!("unrecognized parameter \"{defname}\"")));
        };
        if given.contains(name) {
            return Err(refuse(format!(
                "parameter \"{name}\" specified more than once"
            )));
        }
        given.push(name);
        let value = text(tree, *arg)?;
        let out_of_bounds =
            || refuse(format!("value {value} out of bounds for option \"{name}\""));
        match kind {
            Kind::Bool => {
                parse_bool(&value).ok_or_else(|| {
                    refuse(format!("invalid value for boolean option \"{name}\": {value}"))
                })?;
            }
            Kind::Int(low, high) => {
                let i = integer(&value).ok_or_else(|| {
                    refuse(format!("invalid value for integer option \"{name}\": {value}"))
                })?;
                if !(*low..=*high).contains(&i) {
                    return Err(out_of_bounds());
                }
            }
            Kind::Real(low, high) => {
                let real = value.trim().parse::<f64>().map_err(|_| {
                    refuse(format!(
                        "invalid value for floating point option \"{name}\": {value}"
                    ))
                })?;
                if !(*low..=*high).contains(&real) {
                    return Err(out_of_bounds());
                }
            }
            Kind::Enum(values) => {
                if !values.iter().any(|v| v.eq_ignore_ascii_case(&value)) {
                    return Err(refuse(format!(
                        "invalid value for enum option \"{name}\": {value}"
                    )));
                }
            }
        }
    }
    Ok(())
}
