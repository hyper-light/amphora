//! Lane G11 (perf/numeric): the cost of writing a REAL as text (`numeric::format_float`, CAST(x AS
//! TEXT)), over the values a query writes: TPC-H q01's charge `l_extendedprice * (1 - l_discount) *
//! (1 + l_tax)` from dbgen's ranges, 2-place prices, and random finite doubles.
//!
//! `cargo run --release -p amphora-engine --example real_text -- <values per class> <passes>`: for
//! each class, one warm-up pass, then `<passes>` timed passes; prints each pass's ns per value, the
//! median, the share written with 15 or fewer significant digits, and an FNV-1a hash of every text,
//! so two builds that write the same texts print the same hash.

use amphora_engine::numeric::format_float;
use std::time::Instant;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// dbgen: l_extendedprice = l_quantity (1..=50) x p_retailprice (900.00..=2098.99 here: 90000 +
/// partkey-derived cents), l_discount 0.00..=0.10, l_tax 0.00..=0.08.
fn charges(n: usize, rng: &mut Rng) -> Vec<f64> {
    (0..n)
        .map(|_| {
            let quantity = (rng.below(50) + 1) as f64;
            let retail = (90_000 + rng.below(119_900)) as f64 / 100.0;
            let discount = rng.below(11) as f64 / 100.0;
            let tax = rng.below(9) as f64 / 100.0;
            quantity * retail * (1.0 - discount) * (1.0 + tax)
        })
        .collect()
}

fn prices(n: usize, rng: &mut Rng) -> Vec<f64> {
    (0..n)
        .map(|_| rng.below(10_000_000) as f64 / 100.0)
        .collect()
}

fn random_bits(n: usize, rng: &mut Rng) -> Vec<f64> {
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let r = f64::from_bits(rng.next());
        if r.is_finite() {
            out.push(r);
        }
    }
    out
}

fn fnv(hash: &mut u64, bytes: &[u8]) {
    for b in bytes.iter().chain(b"\n") {
        *hash ^= u64::from(*b);
        *hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
}

fn significant_digits(text: &str) -> usize {
    let mantissa = text.split(['e', 'E']).next().unwrap_or(text);
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let digits = digits.trim_start_matches('0').trim_end_matches('0');
    digits.len()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let n: usize = args.next().ok_or("usage: real_text <n> <passes>")?.parse()?;
    let passes: usize = args.next().ok_or("usage: real_text <n> <passes>")?.parse()?;
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let classes = [
        ("q01-charge", charges(n, &mut rng)),
        ("price-2dp", prices(n, &mut rng)),
        ("random-bits", random_bits(n, &mut rng)),
    ];
    for (name, values) in &classes {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        let mut short = 0usize;
        for v in values {
            let text = format_float(*v);
            fnv(&mut hash, text.as_bytes());
            if significant_digits(&text) <= 15 {
                short += 1;
            }
        }
        let mut per_value = Vec::with_capacity(passes);
        let mut bytes = 0usize;
        for _ in 0..passes {
            let start = Instant::now();
            for v in values {
                bytes = bytes.wrapping_add(std::hint::black_box(format_float(*v)).len());
            }
            per_value.push(start.elapsed().as_secs_f64() * 1e9 / values.len() as f64);
        }
        let runs: Vec<String> = per_value.iter().map(|x| format!("{x:.1}")).collect();
        per_value.sort_by(f64::total_cmp);
        let median = per_value.get(per_value.len() / 2).copied().unwrap_or(f64::NAN);
        println!(
            "{name}: n={} median_ns={median:.1} passes_ns=[{}] le15_digits={short} hash={hash:016x} bytes={bytes}",
            values.len(),
            runs.join(",")
        );
    }
    Ok(())
}
