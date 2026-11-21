//! G7 step 3: three ways to reach a cached page against each other (the engine's
//! `storage::reach`): point reads and scans at each client count, with the whole file cached and
//! with a pool a tenth of it; one JSON line a run. Each client makes as many accesses as the file
//! has pages; the tables' shards follow the machine's threads.
//!
//! usage: amphora-bench --reach --pages <the file's pages> [--clients 1,32]

use amphora_engine::storage::reach::bench::run_all;

pub fn main() -> anyhow::Result<()> {
    let mut pages = None;
    // The plan's client counts (G7 step 3: "at 1 and 32 clients").
    let mut clients = vec![1usize, 32];
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut v = || {
            args.next()
                .ok_or_else(|| anyhow::anyhow!("{a} takes a value"))
        };
        match a.as_str() {
            "--reach" => {}
            "--pages" => pages = Some(v()?.parse::<u64>()?),
            "--clients" => clients = v()?.split(',').map(str::parse).collect::<Result<_, _>>()?,
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    let pages = pages.ok_or_else(|| {
        anyhow::anyhow!("--pages: the file's page count (a corpus database's size over 4096)")
    })?;
    run_all(pages, &clients, |r| println!("{}", r.json())).map_err(|e| anyhow::anyhow!("{e}"))
}
