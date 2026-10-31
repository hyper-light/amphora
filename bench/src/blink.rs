//! The versioned storage's B-link tree on its own (Amphora's docs/design/engine.md, stage 1), in
//! memory: at each thread count, that many threads put random keys into one tree at once, each
//! its share, then read random keys back; puts and gets a second, one JSON line a count.
//!
//! usage: amphora-bench --blink [--threads 1,2,4,8,16,32] [--keys 1000000] [--value 64]

use std::sync::Arc;
use std::time::Instant;

use amphora_engine::storage::versioned::blink::{Descender, Tree};
use amphora_engine::storage::versioned::pool::Pool;

fn mix(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ceb9fe1a85ec53);
    x ^ (x >> 33)
}

pub fn main() -> anyhow::Result<()> {
    let mut threads = vec![1usize, 2, 4, 8, 16, 32];
    let mut keys = 1_000_000u64;
    let mut value = 64usize;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut v = || args.next().ok_or_else(|| anyhow::anyhow!("{a} takes a value"));
        match a.as_str() {
            "--blink" => {}
            "--threads" => threads = v()?.split(',').map(str::parse).collect::<Result<_, _>>()?,
            "--keys" => keys = v()?.parse()?,
            "--value" => value = v()?.parse()?,
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    for &t in &threads {
        // A page holds about half its bytes of cells after splits: a frame for each 1,500 bytes of
        // rows, and a margin.
        let row = 8 + value as u64 + 6;
        let frames = usize::try_from(keys * row / 1_500 + 1_000)?;
        let pool = Arc::new(Pool::new(frames));
        let tree = Arc::new(Tree::create(&pool, Box::new(|a: &[u8], b: &[u8]| a.cmp(b))).map_err(|e| anyhow::anyhow!("{e}"))?);
        let each = keys / t as u64;
        let began = Instant::now();
        let handles: Vec<_> = (0..t as u64)
            .map(|w| {
                let (pool, tree) = (pool.clone(), tree.clone());
                std::thread::spawn(move || -> anyhow::Result<()> {
                    let mut d = Descender::new();
                    let v = vec![7u8; value];
                    for i in 0..each {
                        let k = mix(i * t as u64 + w).to_be_bytes();
                        tree.put(&pool, &mut d, &k, &v).map_err(|e| anyhow::anyhow!("{e}"))?;
                    }
                    Ok(())
                })
            })
            .collect();
        for h in handles {
            h.join().map_err(|_| anyhow::anyhow!("a writer panicked"))??;
        }
        let put_s = began.elapsed().as_secs_f64();
        let began = Instant::now();
        let handles: Vec<_> = (0..t as u64)
            .map(|w| {
                let (pool, tree) = (pool.clone(), tree.clone());
                std::thread::spawn(move || -> anyhow::Result<u64> {
                    let mut d = Descender::new();
                    let mut found = 0;
                    for i in 0..each {
                        let n = mix(i ^ (w << 40)) % (each * t as u64);
                        let k = mix(n).to_be_bytes();
                        if tree.get(&pool, &mut d, &k).map_err(|e| anyhow::anyhow!("{e}"))?.is_some() {
                            found += 1;
                        }
                    }
                    Ok(found)
                })
            })
            .collect();
        let mut found = 0;
        for h in handles {
            found += h.join().map_err(|_| anyhow::anyhow!("a reader panicked"))??;
        }
        let get_s = began.elapsed().as_secs_f64();
        let n = (each * t as u64) as f64;
        println!(
            "{}",
            serde_json::json!({
                "threads": t,
                "keys": each * t as u64,
                "value": value,
                "puts_per_s": (n / put_s).round(),
                "gets_per_s": (n / get_s).round(),
                "found": found,
                "pages": pool.allocated(),
            })
        );
    }
    Ok(())
}
