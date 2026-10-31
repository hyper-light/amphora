//! A statement on a database the suite left behind (`--query`): its plans and how long each run
//! took, without loading the data again. A diagnosis, not a record: nothing here is a measured run.
//!
//! `amphora-bench --query --target amphora|sqlite --dir <the suite's --dir> --sql <SQL> [--params
//! 11,90] [--repeat 5] [--explain]`. Parameters are integers, or `f:<real>`, `t:<text>`, `null`.
//! `AMPHORA_SHARED_PAGES` (`suite_db::amphora_sharing`) and `AMPHORA_READER_PAGES` set Amphora's
//! pages as the suite's do.

use std::path::Path;
use std::time::Instant;

use crate::suite::P;

struct Opts {
    target: String,
    dir: String,
    sql: String,
    params: Vec<P>,
    repeat: usize,
    explain: bool,
}

fn param(s: &str) -> anyhow::Result<P> {
    Ok(match s.split_once(':') {
        Some(("f", v)) => P::F(v.parse()?),
        Some(("t", v)) => P::T(v.to_owned()),
        _ if s == "null" => P::N,
        _ => P::I(s.parse()?),
    })
}

fn opts() -> anyhow::Result<Opts> {
    let mut o = Opts { target: String::new(), dir: String::new(), sql: String::new(), params: Vec::new(), repeat: 3, explain: false };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut v = || args.next().ok_or_else(|| anyhow::anyhow!("{a} takes a value"));
        match a.as_str() {
            "--query" => {}
            "--target" => o.target = v()?,
            "--dir" => o.dir = v()?,
            "--sql" => o.sql = v()?,
            "--params" => o.params = v()?.split(',').filter(|s| !s.is_empty()).map(param).collect::<anyhow::Result<_>>()?,
            "--repeat" => o.repeat = v()?.parse()?,
            "--explain" => o.explain = true,
            other => anyhow::bail!("unknown argument {other} (--query takes --target, --dir, --sql, --params, --repeat, --explain)"),
        }
    }
    anyhow::ensure!(!o.dir.is_empty() && !o.sql.is_empty(), "--query needs --dir and --sql");
    Ok(o)
}

pub fn main() -> anyhow::Result<()> {
    let o = opts()?;
    let path = Path::new(&o.dir).join("suite.db");
    anyhow::ensure!(path.exists(), "no database at {}", path.display());
    let mut run: Box<dyn FnMut(&str, &[P]) -> anyhow::Result<Vec<String>>> = match o.target.as_str() {
        "amphora" => {
            // The suite's readers read on connections of their own beside the writer's: so does this.
            let writer = amphora::db::Db::open_sharing(&path, &crate::suite_db::amphora_sharing()?)?;
            let mut db = writer.reader()?;
            if let Some(pages) = std::env::var("AMPHORA_READER_PAGES").ok().and_then(|v| v.parse().ok()) {
                db.keep_pages(pages)?;
            }
            Box::new(move |sql, ps| {
                let _ = &writer;
                // The engine keeps statements by their text's address.
                let sql: &'static str = Box::leak(sql.to_owned().into_boxed_str());
                let mut out = Vec::new();
                db.query(sql, ps.iter().map(crate::suite_db::amphora_value), |r| {
                    out.push(r.get_values().map(|v| v.to_string()).collect::<Vec<_>>().join(" | "));
                    Ok(true)
                })?;
                Ok(out)
            })
        }
        "sqlite" => {
            let c = rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            Box::new(move |sql, ps| {
                let mut stmt = c.prepare_cached(sql)?;
                let columns = stmt.column_count();
                let mut rows = stmt.query(rusqlite::params_from_iter(ps.iter().map(crate::suite_db::sqlite_value)))?;
                let mut out = Vec::new();
                while let Some(r) = rows.next()? {
                    let cells: Vec<String> = (0..columns).map(|i| r.get_ref(i).map(|v| format!("{v:?}")).unwrap_or_default()).collect();
                    out.push(cells.join(" | "));
                }
                Ok(out)
            })
        }
        t => anyhow::bail!("--query reads amphora or sqlite, not {t}"),
    };
    println!("== {} {} {:?}", o.target, o.sql, o.params);
    for line in run(&format!("EXPLAIN QUERY PLAN {}", o.sql), &o.params)? {
        println!("plan: {line}");
    }
    if o.explain {
        for line in run(&format!("EXPLAIN {}", o.sql), &o.params)? {
            println!("  {line}");
        }
    }
    for i in 0..o.repeat {
        let began = Instant::now();
        let rows = run(&o.sql, &o.params)?;
        println!("run {i}: {} rows in {:.3} ms", rows.len(), began.elapsed().as_secs_f64() * 1000.0);
        if i == 0 {
            for r in rows.iter().take(5) {
                println!("  {r}");
            }
        }
    }
    println!("rss {:?} MB, peak {:?} MB", crate::rss_mb(), crate::rss_max_mb());
    Ok(())
}
