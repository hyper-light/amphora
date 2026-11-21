//! `amphora-corpus run <pool dir> [--budget-s <seconds>] [--times <file>]`: a pool of corpus files
//! run on Amphora, each on a fresh in-memory database, in the pool's order (`order.tsv`: a line a
//! file, `required` or `fill`, then its path; every `.test` file in path order when there is none).
//! Required files run whole; fill files run until the budget, measured on this machine as it runs,
//! is spent, so the subset's size is what this machine runs in the budget (G2 step 7). `--times`
//! writes each record's outcome and time, a JSON line each, then a line for the file whole
//! (`file_ns`, its fresh database and fixture included; `unsettled`, its failed settles), for re-ranking the pool from the machines
//! that run it (bench/corpus/subset.py). A file `<source>/base.test` is the fixture of every file under `<source>/`
//! (bench/corpus/build_corpus.py's rule). Exits 1 on any disagreement.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use amphora_corpus::amphora::{Amphora, AmphoraRender};
use amphora_corpus::run::{Counts, Outcome, run_file};

struct Args {
    pool: PathBuf,
    budget: Option<Duration>,
    times: Option<PathBuf>,
}

fn args() -> Result<Args, String> {
    let mut it = std::env::args_os().skip(1).map(|a| {
        a.into_string()
            .map_err(|a| format!("an argument that isn't Unicode: {a:?}"))
    });
    match it.next().transpose()?.as_deref() {
        Some("run") => {}
        other => {
            return Err(format!(
                "usage: amphora-corpus run <pool dir> [--budget-s <seconds>] [--times <file>] (got {other:?})"
            ));
        }
    }
    let pool = PathBuf::from(it.next().transpose()?.ok_or("run takes a pool directory")?);
    let (mut budget, mut times) = (None, None);
    while let Some(a) = it.next().transpose()? {
        let v = it
            .next()
            .transpose()?
            .ok_or_else(|| format!("{a} takes a value"))?;
        match a.as_str() {
            "--budget-s" => {
                budget = Some(
                    Duration::try_from_secs_f64(
                        v.parse::<f64>()
                            .map_err(|e| format!("--budget-s {v}: {e}"))?,
                    )
                    .map_err(|e| format!("--budget-s {v}: {e}"))?,
                )
            }
            "--times" => times = Some(PathBuf::from(v)),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Args {
        pool,
        budget,
        times,
    })
}

/// The pool's files in order, each with whether it's required.
fn order(pool: &Path) -> Result<Vec<(bool, PathBuf)>, String> {
    let manifest = pool.join("order.tsv");
    if manifest.exists() {
        let text = std::fs::read_to_string(&manifest)
            .map_err(|e| format!("{}: {e}", manifest.display()))?;
        return text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| match l.split_once('\t') {
                Some(("required", p)) => Ok((true, pool.join(p))),
                Some(("fill", p)) => Ok((false, pool.join(p))),
                _ => Err(format!(
                    "{}: a line that isn't `required|fill<TAB>path`: {l}",
                    manifest.display()
                )),
            })
            .collect();
    }
    // A flat walk with an explicit stack (no recursion, CLAUDE.md §11).
    let mut files = Vec::new();
    let mut dirs = vec![pool.to_path_buf()];
    while let Some(d) = dirs.pop() {
        for entry in std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))? {
            let p = entry.map_err(|e| e.to_string())?.path();
            if p.is_dir() {
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x == "test")
                && p.file_name().is_some_and(|n| n != "base.test")
            {
                files.push(p);
            }
        }
    }
    files.sort();
    Ok(files.into_iter().map(|p| (true, p)).collect())
}

/// `<pool>/<source>/base.test` for a file under `<pool>/<source>/`.
fn fixture(pool: &Path, file: &Path) -> Option<PathBuf> {
    let source = file.strip_prefix(pool).ok()?.components().next()?;
    let f = pool.join(source).join("base.test");
    if f.exists() && f != file {
        Some(f)
    } else {
        None
    }
}

fn main() -> ExitCode {
    let mut out = std::io::stdout().lock();
    match go(&mut out) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            let _ = writeln!(out, "amphora-corpus: {e}");
            ExitCode::from(2)
        }
    }
}

fn go(out: &mut dyn Write) -> Result<bool, String> {
    let a = args()?;
    let began = Instant::now();
    let deadline = a.budget.and_then(|b| began.checked_add(b));
    let mut times = match &a.times {
        Some(p) => Some(std::io::BufWriter::new(
            std::fs::File::create(p).map_err(|e| format!("{}: {e}", p.display()))?,
        )),
        None => None,
    };
    let mut render = AmphoraRender::new()?;
    let mut total = Counts::default();
    let (mut files, mut write_error) = (0u64, None);
    for (required, file) in order(&a.pool)? {
        let text =
            std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let fix = match fixture(&a.pool, &file) {
            Some(f) => {
                Some(std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?)
            }
            None => None,
        };
        let rel = file
            .strip_prefix(&a.pool)
            .unwrap_or(&file)
            .display()
            .to_string();
        let file_began = Instant::now();
        let mut engine = Amphora::in_memory()?;
        let counts = run_file(&mut engine, &mut render, &text, fix.as_deref(), if required { None } else { deadline }, &mut |ran| {
            let outcome = match &ran.outcome {
                Outcome::Agree => "agree".to_owned(),
                Outcome::Unreached => "unreached".to_owned(),
                other => {
                    let shown = match other {
                        Outcome::Answer { want, got } => format!("SQLite answers {want}\n    Amphora answers {got}"),
                        Outcome::Refused(e) => format!("Amphora refused it: {e}"),
                        Outcome::Accepted => "Amphora ran what SQLite refuses".to_owned(),
                        _ => String::new(),
                    };
                    if let Err(e) = writeln!(out, "{rel}:{}: {}\n    {shown}", ran.record.line, ran.record.sql.replace('\n', " ")) {
                        write_error = Some(e.to_string());
                    }
                    "disagree".to_owned()
                }
            };
            if let Some(t) = times.as_mut() {
                let line = serde_json::json!({ "file": rel, "line": ran.record.line, "outcome": outcome, "ns": ran.nanos });
                if let Err(e) = writeln!(t, "{line}") {
                    write_error = Some(e.to_string());
                }
            }
        })
        .map_err(|e| format!("{rel}: {e}"))?;
        if let Some(t) = times.as_mut() {
            // The file whole, its fresh database and fixture included: what it costs a run's budget;
            // and its failed settles, which fail a run as a disagreement does.
            let ns = u64::try_from(file_began.elapsed().as_nanos()).unwrap_or(u64::MAX);
            let line = serde_json::json!({ "file": rel, "file_ns": ns, "required": required, "unsettled": counts.unsettled });
            writeln!(t, "{line}").map_err(|e| e.to_string())?;
        }
        files = files.saturating_add(1);
        total.agree = total.agree.saturating_add(counts.agree);
        total.disagree = total.disagree.saturating_add(counts.disagree);
        total.skipped = total.skipped.saturating_add(counts.skipped);
        total.unreached = total.unreached.saturating_add(counts.unreached);
        total.unsettled = total.unsettled.saturating_add(counts.unsettled);
    }
    if let Some(mut t) = times {
        t.flush().map_err(|e| e.to_string())?;
    }
    if let Some(e) = write_error {
        return Err(e);
    }
    writeln!(
        out,
        "{files} files in {:.1} s: {} records agree, {} disagree, {} skipped (not SQLite's), {} past the budget, {} settles failed",
        began.elapsed().as_secs_f64(),
        total.agree,
        total.disagree,
        total.skipped,
        total.unreached,
        total.unsettled
    )
    .map_err(|e| e.to_string())?;
    Ok(total.disagree == 0 && total.unsettled == 0)
}
