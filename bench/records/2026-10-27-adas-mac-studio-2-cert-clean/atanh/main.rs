// atanh: each computation in crates/engine/vdbe/value.rs (imp.rs holds them as they are there)
// against this platform's C library atanh, called directly. A measurement program, not part of
// the workspace: rustc -O --edition 2021 main.rs -o check && ./check 3000000
mod imp;
extern "C" {
    fn atanh(x: f64) -> f64;
}
fn main() {
    let n: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(2_000_000);
    let mut s: u64 = 0x9E3779B97F4A7C15;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    let (mut cr_bad, mut apple_bad, mut total) = (0usize, 0usize, 0usize);
    let mut shown = 0;
    let mut check = |x: f64, cr_bad: &mut usize, apple_bad: &mut usize| {
        let lib = unsafe { atanh(x) };
        let cr = imp::atanh_cr(x);
        let ap = imp::atanh_apple(x);
        if cr.to_bits() != lib.to_bits() && !(cr.is_nan() && lib.is_nan()) {
            *cr_bad += 1;
            if shown < 8 {
                println!("cr    x {x:e} libc {lib:e} ours {cr:e}");
                shown += 1;
            }
        }
        if ap.to_bits() != lib.to_bits() && !(ap.is_nan() && lib.is_nan()) {
            *apple_bad += 1;
        }
    };
    for v in [0.5, -0.5, 0.1, 0.25, 0.3, 1.0, -1.0, 2.0, -0.0, 0.0, 7.450580596923828e-9, 3.725290298461914e-9, 0.1715728752538099, 0.17157287525380993, 0.9999999999999999, -0.99999999999999989, 1e-308, 5e-324, f64::NAN] {
        check(v, &mut cr_bad, &mut apple_bad);
        total += 1;
    }
    for i in 0..n {
        let u = (next() >> 11) as f64 / (1u64 << 53) as f64;
        let x = match i % 4 {
            0 => u * 2.0 - 1.0,
            1 => u * 0.2,
            2 => (0.5 + u * 0.5) * 2f64.powi(-((next() % 60) as i32 + 1)),
            _ => 1.0 - 2f64.powf(-1.0 - u * 51.0),
        };
        let x = if next() & 1 == 0 { x } else { -x };
        check(x, &mut cr_bad, &mut apple_bad);
        total += 1;
    }
    println!("{} args: correctly rounded differs from libc for {cr_bad}, apple's formula for {apple_bad}", total);
}
