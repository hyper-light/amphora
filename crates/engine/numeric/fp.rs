//! A REAL as SQLite 3.54 writes it in text: `sqlite3FpDecode` and the `%!.17g` its
//! `vdbeMemRenderNum` renders (REAL to TEXT), and `%!0.17g` (quote() and JSON: the same digits,
//! infinity as `9.0e+999`), ported from its amalgamation (sqlite3.c 3.54.0, printf.c and util.c).
//! The digits follow https://www.sqlite.org/floatingpoint.html 1.3.1 exactly: 15 significant digits
//! when they convert back to the same double, else 17. Where SQLite 3.54.0 measures otherwise (its
//! 3.54 shortcut tries fewer than 17 digits only after three zeros or two nines, so it writes
//! 716936.91809735901 for 716936.918097359), Amphora answers as the document says
//! (bench/records/2026-11-23-adacachy2-perf-numeric-g10.md). The 17 digits come from a fixed-point
//! product with a power of ten (Russ Cox's fpfmt as SQLite adapts it), the first 18 rounded and then
//! rounded at the 18th, which is the rounding the document's own example shows (2.8781999999999997).
//!
//! Every shift here moves by an amount the algorithm keeps within the word (SQLite asserts as much);
//! a shift out of range, which no double reaches, ends the conversion with `None`, and the caller
//! writes the double as Rust does.

/// The powers of ten the conversions scale by, from 10^-348 to 10^347.
const POWERS_OF_10_FIRST: i32 = -348;
const POWERS_OF_10_LAST: i32 = 347;

/// 10^0 to 10^26, each its top 64 bits (sqlite3.c powerOfTen's aBase[]).
const BASE: [u64; 27] = [
    0x8000_0000_0000_0000,
    0xa000_0000_0000_0000,
    0xc800_0000_0000_0000,
    0xfa00_0000_0000_0000,
    0x9c40_0000_0000_0000,
    0xc350_0000_0000_0000,
    0xf424_0000_0000_0000,
    0x9896_8000_0000_0000,
    0xbebc_2000_0000_0000,
    0xee6b_2800_0000_0000,
    0x9502_f900_0000_0000,
    0xba43_b740_0000_0000,
    0xe8d4_a510_0000_0000,
    0x9184_e72a_0000_0000,
    0xb5e6_20f4_8000_0000,
    0xe35f_a931_a000_0000,
    0x8e1b_c9bf_0400_0000,
    0xb1a2_bc2e_c500_0000,
    0xde0b_6b3a_7640_0000,
    0x8ac7_2304_89e8_0000,
    0xad78_ebc5_ac62_0000,
    0xd8d7_26b7_177a_8000,
    0x8786_7832_6eac_9000,
    0xa968_163f_0a57_b400,
    0xd3c2_1bce_cced_a100,
    0x8459_5161_4014_84a0,
    0xa56f_a5b9_9019_a5c8,
];

/// 10^(27k) for k from -13 to 12, each its top 64 bits (aScale[]) and the next 32 (aScaleLo[]);
/// index 13 is 10^-1, a special case.
const SCALE: [u64; 26] = [
    0x8049_a4ac_0c58_11ae,
    0xcf42_894a_5dce_35ea,
    0xa76c_5823_38ed_2621,
    0x873e_4f75_e222_4e68,
    0xda7f_5bf5_9096_6848,
    0xb080_392c_c434_9dec,
    0x8e93_8662_882a_f53e,
    0xe658_29b3_046b_0afa,
    0xba12_1a46_50e4_ddeb,
    0x964e_858c_91ba_2655,
    0xf2d5_6790_ab41_c2a2,
    0xc428_d05a_a475_1e4c,
    0x9e74_d1b7_91e0_7e48,
    0xcccc_cccc_cccc_cccc,
    0xcecb_8f27_f420_0f3a,
    0xa70c_3c40_a64e_6c51,
    0x86f0_ac99_b4e8_dafd,
    0xda01_ee64_1a70_8de9,
    0xb01a_e745_b101_e9e4,
    0x8e41_ade9_fbeb_c27d,
    0xe5d3_ef28_2a24_2e81,
    0xb9a7_4a06_37ce_2ee1,
    0x95f8_3d0a_1fb6_9cd9,
    0xf24a_01a7_3cf2_dccf,
    0xc3b8_3581_09e8_4f07,
    0x9e19_db92_b4e3_1ba9,
];

const SCALE_LO: [u32; 26] = [
    0x205b_896d,
    0x5206_4cad,
    0xaf2a_f2b8,
    0x5a77_44a7,
    0xaf39_a475,
    0xbd8d_794e,
    0x547e_b47b,
    0x0cb4_a5a3,
    0x92f3_4d62,
    0x3a6a_07f9,
    0xfae2_7299,
    0xaa97_e14c,
    0x775e_a265,
    0xcccc_cccc,
    0x0000_0000,
    0x9990_90b6,
    0x69a0_28bb,
    0xe80e_6f48,
    0x5ec0_5dd0,
    0x1458_8f14,
    0x8f16_68c9,
    0x6d95_3e2c,
    0x4abd_af10,
    0xbc63_3b39,
    0x0a86_2f81,
    0x6c07_a2c2,
];

const BIT63: u64 = 0x8000_0000_0000_0000;

fn hi64(x: u128) -> u64 {
    u64::try_from(x.wrapping_shr(64)).unwrap_or(u64::MAX)
}

fn lo64(x: u128) -> u64 {
    u64::try_from(x & u128::from(u64::MAX)).unwrap_or(0)
}

/// a*b: its high 64 bits and its low 64 (sqlite3Multiply128).
fn multiply128(a: u64, b: u64) -> (u64, u64) {
    let r = u128::from(a).wrapping_mul(u128::from(b));
    (hi64(r), lo64(r))
}

/// A = (a << 32) + a_lo times b: the top 64 bits of the 160-bit product and the 32 after them
/// (sqlite3Multiply160). The product of 64 and 64 bits fits 128, and the carry from a_lo*b >> 32
/// stays under it.
fn multiply160(a: u64, a_lo: u32, b: u64) -> (u64, u32) {
    let r = u128::from(a).wrapping_mul(u128::from(b)).wrapping_add(
        u128::from(a_lo)
            .wrapping_mul(u128::from(b))
            .wrapping_shr(32),
    );
    (
        hi64(r),
        u32::try_from(r.wrapping_shr(32) & 0xffff_ffff).unwrap_or(0),
    )
}

/// The top 64 bits of 10^p, and the 32 after them, for p from -348 to 347 (powerOfTen).
fn power_of_ten(p: i32) -> Option<(u64, u32)> {
    if !(POWERS_OF_10_FIRST..=POWERS_OF_10_LAST).contains(&p) {
        return None;
    }
    let (g, n) = if p < 0 {
        if p == -1 {
            return Some((*SCALE.get(13)?, *SCALE_LO.get(13)?));
        }
        let (g, n) = (p.checked_div(27)?, p.checked_rem(27)?);
        if n != 0 {
            (g.checked_sub(1)?, n.checked_add(27)?)
        } else {
            (g, n)
        }
    } else if p < 27 {
        return Some((*BASE.get(usize::try_from(p).ok()?)?, 0));
    } else {
        (p.checked_div(27)?, p.checked_rem(27)?)
    };
    let at = usize::try_from(g.checked_add(13)?).ok()?;
    let (s, s_lo) = (*SCALE.get(at)?, *SCALE_LO.get(at)?);
    if n == 0 {
        return Some((s, s_lo));
    }
    let (mut x, mut lo) = multiply160(s, s_lo, *BASE.get(usize::try_from(n).ok()?)?);
    if x & BIT63 == 0 {
        x = x.wrapping_shl(1) | u64::from(lo.wrapping_shr(31) & 1);
        lo = lo.wrapping_shl(1) | 1;
    }
    Some((x, lo))
}

/// floor(log2(10^p)) (pwr10to2): |p| stays under 400, so the product fits 32 bits.
fn pwr10to2(p: i32) -> i32 {
    p.wrapping_mul(108_853).wrapping_shr(15)
}

/// floor(log10(2^p)) (pwr2to10): |p| stays under 1200.
fn pwr2to10(p: i32) -> i32 {
    p.wrapping_mul(78_913).wrapping_shr(18)
}

fn shift_amount(by: i32) -> Option<u32> {
    u32::try_from(by).ok().filter(|s| *s < 64)
}

/// m * 2^e as d * 10^p with at least `n` significant digits in d (sqlite3Fp2Convert10). m has its
/// top bit set.
fn fp2_convert10(m: u64, e: i32, n: i32) -> Option<(u64, i32)> {
    let p = n
        .checked_sub(1)?
        .checked_sub(pwr2to10(e.checked_add(63)?))?;
    let (ten, _) = power_of_ten(p)?;
    let (h, _) = multiply128(m, ten);
    let base = e.checked_add(pwr10to2(p))?;
    let d = if n == 18 {
        let h = h.checked_shr(shift_amount(base.checked_add(2)?.checked_neg()?)?)?;
        h.wrapping_add(h.wrapping_shl(1) & 2).wrapping_shr(1)
    } else {
        h.checked_shr(shift_amount(base.checked_add(1)?.checked_neg()?)?)?
    };
    Some((d, p.checked_neg()?))
}

/// A double's decimal digits (sqlite3FpDecode's FpDecode): `digits` its significant digits,
/// `point` where the decimal point falls among them, `negative` its sign.
struct Decoded {
    negative: bool,
    digits: Vec<u8>,
    point: i32,
}

enum Decode {
    Finite(Decoded),
    Infinite { negative: bool },
    NaN,
}

/// sqlite3FpDecode(r, round, max_round) for round > 0 (the `%g` its printf renders; `%f`'s round <= 0
/// isn't needed here): `round` significant digits at most, or `max_round`. 17 digits are the first 18
/// rounded, then rounded at the 18th (5 or more rounds up): floatingpoint.html's "If you round to 17
/// digits, you get 2.8781999999999997" for 6481130223748880 x 2^-51.
fn decode(r: f64, round: i32, max_round: i32) -> Option<Decode> {
    let negative = r.is_sign_negative() && r != 0.0;
    let r = r.abs();
    if r == 0.0 {
        return Some(Decode::Finite(Decoded {
            negative: false,
            digits: vec![b'0'],
            point: 1,
        }));
    }
    let bits = r.to_bits();
    let biased = i32::try_from(bits.wrapping_shr(52) & 0x7ff).ok()?;
    if biased == 0x7ff {
        return Some(if bits == 0x7ff0_0000_0000_0000 {
            Decode::Infinite { negative }
        } else {
            Decode::NaN
        });
    }
    let mut v = bits & 0x000f_ffff_ffff_ffff;
    let e = if biased == 0 {
        let nn = v.leading_zeros();
        v = v.checked_shl(nn)?;
        (-1074_i32).checked_sub(i32::try_from(nn).ok()?)?
    } else {
        v = v.wrapping_shl(11) | BIT63;
        biased.checked_sub(1086)?
    };
    let (mut v, exp) = fp2_convert10(
        v,
        e,
        if round >= 18 {
            18
        } else {
            round.checked_add(1)?
        },
    )?;
    // the digits, most significant first, with room for a carry into a new first digit
    let mut buf: Vec<u8> = Vec::with_capacity(21);
    while v > 0 {
        buf.push(b'0'.checked_add(u8::try_from(v.checked_rem(10)?).ok()?)?);
        v = v.checked_div(10)?;
    }
    buf.push(b'0');
    buf.reverse();
    let mut start = 1usize;
    let mut n = i32::try_from(buf.len().checked_sub(1)?).ok()?;
    let mut point = n.checked_add(exp)?;
    let digit = |buf: &Vec<u8>, start: usize, i: i32| -> Option<u8> {
        buf.get(start.checked_add(usize::try_from(i).ok()?)?)
            .copied()
    };
    let mut round = round;
    if round > 0 && (round < n || n > max_round) {
        if round > max_round {
            round = max_round;
        }
        n = round;
        if digit(&buf, start, round)? >= b'5' {
            let mut j = round.checked_sub(1)?;
            loop {
                let at = start.checked_add(usize::try_from(j).ok()?)?;
                let d = buf.get_mut(at)?;
                *d = d.checked_add(1)?;
                if *d <= b'9' {
                    break;
                }
                *d = b'0';
                if j == 0 {
                    start = start.checked_sub(1)?;
                    *buf.get_mut(start)? = b'1';
                    n = n.checked_add(1)?;
                    point = point.checked_add(1)?;
                    break;
                }
                j = j.checked_sub(1)?;
            }
        }
    }
    while n > 1 && digit(&buf, start, n.checked_sub(1)?)? == b'0' {
        n = n.checked_sub(1)?;
    }
    let digits = buf
        .get(start..start.checked_add(usize::try_from(n).ok()?)?)?
        .to_vec();
    Some(Decode::Finite(Decoded {
        negative,
        digits,
        point,
    }))
}

/// floatingpoint.html 1.3.1's steps (a) and (b): r rounded to 15 significant digits, when that text
/// converts back to r ("Round to 15 digits. Convert the text result back to binary-64."); `None`
/// sends it to step (c), 17-digit rounding.
///
/// For a normal double the two steps are answered by its shortest round-trip digits (Rust's `{:e}`,
/// exact), without writing 15 digits and parsing them back: half an ulp is at most 2^-53 of r, below
/// half a unit in the 15th digit (at least 5 x 10^-16 of r), so a 15-digit text converts back exactly
/// when the shortest text has 15 digits or fewer, and then rounding r to 15 digits gives the shortest
/// text's digits. A subnormal's ulp is too wide for that (5e-324 converts back; the page's 15 digits
/// are 4.94065645841247e-324), so it takes the steps as written: Rust's `{:.14e}` rounds the exact
/// value, and its parse takes the nearest double (1.2.1: "The first number is closer to the desired
/// value of 47.49, so that is the one that gets used").
fn fifteen_digits(r: f64) -> Option<Decoded> {
    if r.is_subnormal() {
        return fifteen_digits_written(r);
    }
    if !r.is_normal() {
        return None;
    }
    let mut text = Text::default();
    std::fmt::write(&mut text, format_args!("{:e}", r.abs())).ok()?;
    let (mantissa, exp) = text.as_str()?.split_once('e')?;
    let digits: Vec<u8> = mantissa.bytes().filter(u8::is_ascii_digit).collect();
    if digits.len() > 15 {
        return None;
    }
    Some(Decoded {
        negative: r.is_sign_negative(),
        digits,
        point: exp.parse::<i32>().ok()?.checked_add(1)?,
    })
}

/// Steps (a) and (b) as written, for a subnormal: a tie at 15 digits never converts back, so the tie
/// rule of `{:.14e}` can't show.
fn fifteen_digits_written(r: f64) -> Option<Decoded> {
    let text = format!("{:.14e}", r.abs());
    if text.parse::<f64>().ok()? != r.abs() {
        return None;
    }
    let (mantissa, exp) = text.split_once('e')?;
    let mut digits: Vec<u8> = mantissa.bytes().filter(u8::is_ascii_digit).collect();
    while digits.len() > 1 && digits.last() == Some(&b'0') {
        digits.pop();
    }
    Some(Decoded {
        negative: r.is_sign_negative(),
        digits,
        point: exp.parse::<i32>().ok()?.checked_add(1)?,
    })
}

/// A double's shortest `{:e}` text, on the stack: at most 17 digits, a point, a sign and `e-324`.
#[derive(Default)]
struct Text {
    bytes: [u8; 32],
    len: usize,
}

impl Text {
    fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(self.bytes.get(..self.len)?).ok()
    }
}

impl std::fmt::Write for Text {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let end = self.len.checked_add(s.len()).ok_or(std::fmt::Error)?;
        self.bytes
            .get_mut(self.len..end)
            .ok_or(std::fmt::Error)?
            .copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// SQLITE_DBCONFIG_FP_DIGITS (c3ref/c_dbconfig_defensive.html): "a small integer that determines the
/// number of significant digits that SQLite will attempt to preserve when converting floating point
/// numbers (IEEE 754 "doubles") into text. The default value 17, as of SQLite version 3.52.0."
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FpDigits(u8);

impl FpDigits {
    pub const DEFAULT: Self = Self(17);

    /// "The first argument is a small integer, between 3 and 23, or zero. The FP_DIGITS setting is
    /// changed to that small integer, or left unaltered if the first argument is zero or out of
    /// range." `None` leaves it unaltered. The page's "between 3 and 23" takes 3; SQLite 3.54.0's
    /// `nIn>3 && nIn<24` refuses it (bench/records/2026-11-23-adacachy2-perf-numeric-g11.md).
    pub fn new(n: i32) -> Option<Self> {
        u8::try_from(n)
            .ok()
            .filter(|n| (3..=23).contains(n))
            .map(Self)
    }

    pub fn get(self) -> u8 {
        self.0
    }
}

/// `%!.*g` of r at `digits` significant digits, as SQLite's printf writes it (`zero_pad`: `%!0.*g`,
/// infinity as 9.0e+999 and NaN as null): fixed notation for exponents from -4 to digits - 1, else
/// d.ddde±XX; trailing zeros dropped but one after the point. At 17, floatingpoint.html 1.3.1's steps
/// (15 digits when they convert back); at any other setting, rounded to that many: "If you do not want
/// the (negligible) overhead of step (b) and (c), just set SQLITE_DBCONFIG_FP_DIGITS to 15."
fn render(r: f64, zero_pad: bool, digits: FpDigits) -> Option<String> {
    let round = i32::from(digits.get());
    let decoded = if digits == FpDigits::DEFAULT {
        fifteen_digits(r).map_or_else(|| decode(r, round, 20), |s| Some(Decode::Finite(s)))
    } else {
        decode(r, round, 20)
    };
    let s = match decoded? {
        Decode::Finite(s) => s,
        Decode::NaN => return Some(if zero_pad { "null" } else { "NaN" }.to_owned()),
        Decode::Infinite { negative } if zero_pad => Decoded {
            negative,
            digits: vec![b'9'],
            point: 1000,
        },
        Decode::Infinite { negative } => {
            return Some(if negative { "-Inf" } else { "Inf" }.to_owned());
        }
    };
    let n = i32::try_from(s.digits.len()).ok()?;
    let exp = s.point.checked_sub(1)?;
    let mut precision: i32 = round.checked_sub(1)?;
    let scientific = exp < -4 || exp > precision;
    if !scientific {
        precision = precision.checked_sub(exp)?;
    }
    let mut e2 = if scientific {
        0
    } else {
        s.point.checked_sub(1)?
    };
    let mut out = String::with_capacity(32);
    if s.negative {
        out.push('-');
    }
    let digit = |i: i32| -> Option<char> {
        s.digits
            .get(usize::try_from(i).ok()?)
            .map(|d| char::from(*d))
    };
    // digits before the point
    let mut j: i32 = 0;
    if e2 < 0 {
        out.push('0');
    } else {
        j = e2.checked_add(1)?.min(n);
        for i in 0..j {
            out.push(digit(i)?);
        }
        e2 = e2.checked_sub(j)?;
        if e2 >= 0 {
            for _ in 0..e2.checked_add(1)? {
                out.push('0');
            }
            e2 = -1;
        }
    }
    out.push('.');
    // zeros after the point before the first significant digit
    if e2 < -1 && precision > 0 {
        let nn = (-1_i32).checked_sub(e2)?.min(precision);
        for _ in 0..nn {
            out.push('0');
        }
        precision = precision.checked_sub(nn)?;
    }
    if precision > 0 {
        let nn = n.checked_sub(j)?.min(precision);
        for i in j..j.checked_add(nn)? {
            out.push(digit(i)?);
        }
    }
    while out.ends_with('0') {
        out.pop();
    }
    if out.ends_with('.') {
        out.push('0');
    }
    if scientific {
        let mut exp = s.point.checked_sub(1)?;
        out.push('e');
        if exp < 0 {
            out.push('-');
            exp = exp.checked_neg()?;
        } else {
            out.push('+');
        }
        if exp >= 100 {
            out.push(char::from(
                b'0'.checked_add(u8::try_from(exp.checked_div(100)?).ok()?)?,
            ));
            exp = exp.checked_rem(100)?;
        }
        out.push(char::from(
            b'0'.checked_add(u8::try_from(exp.checked_div(10)?).ok()?)?,
        ));
        out.push(char::from(
            b'0'.checked_add(u8::try_from(exp.checked_rem(10)?).ok()?)?,
        ));
    }
    Some(out)
}

/// A REAL as text, as SQLite converts one at the default FP_DIGITS (`%!.17g`).
pub fn real_to_text(r: f64) -> String {
    real_to_text_digits(r, FpDigits::DEFAULT)
}

/// A REAL as text at a connection's FP_DIGITS (`vdbeMemRenderNum`'s `%!.*g`).
pub fn real_to_text_digits(r: f64, digits: FpDigits) -> String {
    render(r, false, digits).unwrap_or_else(|| format!("{r:e}"))
}

/// A REAL as quote() and JSON write it (`%!0.17g`): infinity as 9.0e+999.
pub fn real_to_quoted(r: f64) -> String {
    render(r, true, FpDigits::DEFAULT).unwrap_or_else(|| format!("{r:e}"))
}
