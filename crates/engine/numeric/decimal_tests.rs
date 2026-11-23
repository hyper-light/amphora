//! Lane G9: a decimal literal or text becomes the correctly rounded double (round to nearest, ties
//! to even) of every digit written, as Rust's parse gives it. The corpus holds random doubles'
//! shortest and longer digits, the halfway point between each double and the next and a hair
//! either side of it, subnormals, overflow edges, mantissas of up to 800 digits, and the literal
//! forms (signs, `E`, leading zeros, `.5`, `5.`). With AMPHORA_DECIMAL_CORPUS naming a file, the
//! corpus is written there, one literal a line, for the SQLite check (bench/decimal_sqlite.py).

use super::{str_to_f64, StrToF64};
use crate::io::MemoryIO;
use crate::vdbe::affinity::{try_for_float, ParsedNumber};
use crate::{ConnectOpts, Database, DatabaseOpts, OpenFlags, SqliteDialect, Value};
use std::sync::Arc;

struct SplitMix(u64);

impl SplitMix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn finite(&mut self) -> f64 {
        loop {
            let x = f64::from_bits(self.next() >> 1);
            if x.is_finite() {
                return x;
            }
        }
    }
}

const LIMB: u64 = 1_000_000_000;

fn times(limbs: &mut Vec<u64>, factor: u64) {
    let mut carry = 0;
    for limb in limbs.iter_mut() {
        let v = *limb * factor + carry;
        *limb = v % LIMB;
        carry = v / LIMB;
    }
    while carry > 0 {
        limbs.push(carry % LIMB);
        carry /= LIMB;
    }
}

fn digits(limbs: &[u64]) -> String {
    let mut out = limbs.last().unwrap().to_string();
    for limb in limbs.iter().rev().skip(1) {
        out.push_str(&format!("{limb:09}"));
    }
    out
}

/// The exact decimal of the point halfway between `x` and the next double up: digits D and an
/// exponent e with the point at D * 10^e.
fn halfway_above(x: f64) -> (String, i64) {
    let bits = x.to_bits();
    let biased = (bits >> 52) as i64;
    let fraction = bits & ((1 << 52) - 1);
    let (mantissa, exp2) = if biased == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1 << 52), biased - 1075)
    };
    let odd = 2 * mantissa + 1;
    let mut limbs = vec![odd % LIMB, odd / LIMB % LIMB, odd / LIMB / LIMB];
    while limbs.len() > 1 && *limbs.last().unwrap() == 0 {
        limbs.pop();
    }
    let exp2 = exp2 - 1;
    let (base, batch, per) = if exp2 >= 0 {
        (2, 1 << 30, 30)
    } else {
        (5, 1_220_703_125, 13)
    };
    let mut n = exp2.unsigned_abs();
    while n >= per {
        times(&mut limbs, batch);
        n -= per;
    }
    for _ in 0..n {
        times(&mut limbs, base);
    }
    (digits(&limbs), exp2.min(0))
}

fn decrement(digits: &str) -> String {
    let mut bytes = digits.as_bytes().to_vec();
    for b in bytes.iter_mut().rev() {
        if *b == b'0' {
            *b = b'9';
        } else {
            *b -= 1;
            break;
        }
    }
    String::from_utf8(bytes).unwrap()
}

fn corpus() -> Vec<String> {
    let mut mix = SplitMix(0x0067_3900_0000_0009);
    let mut out: Vec<String> = [
        "0.1",
        "1e-300",
        "1e300",
        "4.9e-324",
        "5e-324",
        "2.4703282292062327e-324",
        "2.4703282292062328e-324",
        "2.2250738585072011e-308",
        "2.2250738585072014e-308",
        "1.7976931348623157e308",
        "1.7976931348623158e308",
        "1.7976931348623159e308",
        "1e-400",
        "1e400",
        "1e-99999",
        "1e99999",
        "9007199254740993",
        "9007199254740993.0",
        "18446744073709551615",
        "18446744073709551616.5",
        "123456789012345678901234567890",
        "0.000000000000000000000000000001e30",
        "+.5",
        "-5.",
        "5.E3",
        ".5e-3",
        "000123.4500e+005",
        "1e-0000300",
        "7.0e-10",
        "8.98846567431158e307",
        "3.0e-324",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    for _ in 0..40_000 {
        let x = mix.finite();
        out.push(format!("{x:e}"));
        out.push(format!("{x:.16e}"));
        out.push(format!("{x:.25e}"));
    }
    for i in 0..30_000 {
        let x = if i % 4 == 0 {
            f64::from_bits(mix.below(1 << 52))
        } else {
            mix.finite()
        };
        let (d, e) = halfway_above(x);
        out.push(format!("{d}e{e}"));
        out.push(format!("{d}000000000000000000001e{}", e - 21));
        out.push(format!("{}{}e{}", decrement(&d), "9".repeat(21), e - 21));
    }
    for _ in 0..10_000 {
        let len = 1 + mix.below(800) as usize;
        let mut d: String = (0..len)
            .map(|_| char::from(b'0' + mix.below(10) as u8))
            .collect();
        d.push(char::from(b'1' + mix.below(9) as u8));
        d.insert(mix.below(d.len() as u64 + 1) as usize, '.');
        let e = mix.below(1400) as i64 - 1000;
        out.push(format!("{d}e{e}"));
    }

    let plain = out.len();
    for i in (0..plain).step_by(7) {
        let lit = out[i].clone();
        if lit.starts_with(['+', '-']) {
            continue;
        }
        out.push(format!("-{lit}"));
        out.push(format!("+{lit}"));
        out.push(format!("00{}", lit.replace('e', "E")));
        out.push(
            lit.replace("e-", "e-0")
                .replace("e", "e+0")
                .replace("e+0-", "e-"),
        );
    }
    out
}

fn bits(value: f64) -> u64 {
    value.to_bits()
}

#[test]
fn g9_decimal_text_converts_to_the_correctly_rounded_double() {
    let corpus = corpus();
    if let Some(path) = std::env::var_os("AMPHORA_DECIMAL_CORPUS") {
        std::fs::write(path, corpus.join("\n")).unwrap();
    }

    let mut wrong = Vec::new();
    for lit in &corpus {
        let want = lit.parse::<f64>().unwrap();
        let text = str_to_f64(lit).map(f64::from);
        if text.map(bits) != Some(bits(want)) {
            wrong.push(format!("str_to_f64({lit}) = {text:?}, not {want:e}"));
        }
        let affinity = match try_for_float(lit.as_bytes()).1 {
            ParsedNumber::Float(f) => Some(f),
            ParsedNumber::Integer(i) => Some(i as f64),
            ParsedNumber::None => None,
        };
        if affinity.map(bits) != Some(bits(want)) {
            wrong.push(format!("try_for_float({lit}) = {affinity:?}, not {want:e}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} literals rounded wrong, first: {:#?}",
        wrong.len(),
        corpus.len(),
        &wrong[..wrong.len().min(10)]
    );
}

#[test]
fn g9_prefix_texts_take_the_correctly_rounded_double_of_their_number() {
    let cases = [
        ("1e-300abc", 1e-300, true),
        ("2.4703282292062328e-324 ", 5e-324, true),
        ("1e", 1.0, false),
        ("1.5e+", 1.5, true),
        ("1.7976931348623159e308x", f64::INFINITY, true),
        ("0.1.2", 0.1, true),
    ];
    for (text, want, fractional) in cases {
        let got = str_to_f64(text).unwrap();
        let (value, is_fractional) = match got {
            StrToF64::Fractional(v) | StrToF64::FractionalPrefix(v) => (f64::from(v), true),
            StrToF64::Decimal(v) | StrToF64::DecimalPrefix(v) => (f64::from(v), false),
        };
        assert_eq!(bits(value), bits(want), "{text}");
        assert_eq!(is_fractional, fractional, "{text}");
    }
}

#[test]
fn g9_sql_literals_casts_and_real_columns_hold_the_correctly_rounded_double() {
    let io = Arc::new(MemoryIO::new());
    let db = Database::open_file_with_flags(
        io,
        "g9-decimal.db",
        OpenFlags::default(),
        DatabaseOpts::new(),
        None,
        Arc::new(SqliteDialect),
    )
    .unwrap();
    let conn = db.connect(ConnectOpts::new()).unwrap();
    conn.execute("CREATE TABLE t(r REAL)").unwrap();

    let sample: Vec<String> = corpus().into_iter().step_by(97).collect();
    for chunk in sample.chunks(50) {
        let literals = chunk.join(", ");
        let casts: Vec<String> = chunk
            .iter()
            .map(|lit| format!("CAST('{lit}' AS REAL)"))
            .collect();
        let rows = conn
            .prepare(format!("SELECT {literals}, {}", casts.join(", ")))
            .unwrap()
            .run_collect_rows()
            .unwrap();
        conn.execute("DELETE FROM t").unwrap();
        let texts: Vec<String> = chunk.iter().map(|lit| format!("('{lit}')")).collect();
        conn.execute(format!("INSERT INTO t VALUES {}", texts.join(", ")))
            .unwrap();
        let stored = conn
            .prepare("SELECT r FROM t ORDER BY rowid")
            .unwrap()
            .run_collect_rows()
            .unwrap();

        // A zero's sign is not this test's subject: a REAL column stores -0.0 as the integer 0.
        let same = |v: &Value, want: f64| {
            let got = v.as_float();
            bits(got) == bits(want) || (got == 0.0 && want == 0.0)
        };
        for (i, lit) in chunk.iter().enumerate() {
            let want = lit.parse::<f64>().unwrap();
            assert!(same(&rows[0][i], want), "SELECT {lit}: {:?}", rows[0][i]);
            assert!(
                same(&rows[0][chunk.len() + i], want),
                "CAST('{lit}' AS REAL): {:?}",
                rows[0][chunk.len() + i]
            );
            assert!(
                same(&stored[i][0], want),
                "'{lit}' into a REAL column: {:?}",
                stored[i][0]
            );
        }
    }
}
