//! SQLite's decimal extension (ext/misc/decimal.c, 3.54.0), which its CLI builds in:
//! https://www.sqlite.org/floatingpoint.html section 2.2.

use std::cmp::Ordering;

use crate::ext::register_scalar_function;
use crate::numeric::fp::real_to_text;
use amphora_engine_ext::{scalar, AggFunc, AggregateDerive, ExtensionApi, Value, ValueType};

/// decimal_pow2(N)'s bound: "N must be an integer between -20000 and +20000"
/// (floatingpoint.html 2.2.2; decimal.c's decimalPow2).
pub const POW2_LIMIT: i64 = 20_000;

/// decimal.c's decimalNewFromText stops reading exponent digits once the exponent reaches this.
const EXPONENT_CAP: i64 = 1_000_000;

/// decimal.c's SQLITE_DECIMAL_MAX_DIGIT: a decimal longer than this is "out of memory".
const MAX_DIGIT: usize = 10_000_000;

/// IEEE 754 binary64: 52 stored significand bits; a biased exponent e and significand m
/// give m * 2^(e - 1075) (bias 1023 plus the 52 fraction bits), as decimal.c's decimalFromDouble.
const SIGNIFICAND_BITS: u32 = 52;
const SIGNIFICAND_MASK: i64 = (1 << SIGNIFICAND_BITS) - 1;
const EXPONENT_OFFSET: i64 = 1075;
/// decimalFromDouble's test for a NaN or an infinity: an unbiased exponent past 971.
const MAX_EXPONENT: i64 = 971;

/// decimal.c's `Decimal`: sign, a[] (one digit 0..=9 per byte, most significant first) and
/// nFrac (how many of the digits follow the point). `frac <= digits.len()` always. decimal.c's
/// isNull is never set by any path in 3.54.0, so it has no counterpart here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Decimal {
    negative: bool,
    digits: Vec<u8>,
    frac: usize,
}

impl Decimal {
    fn zero() -> Self {
        Self {
            digits: vec![0],
            ..Self::default()
        }
    }

    fn int_len(&self) -> usize {
        self.digits.len().saturating_sub(self.frac)
    }

    fn parts(&self) -> (&[u8], &[u8]) {
        self.digits
            .split_at_checked(self.int_len())
            .unwrap_or((&self.digits, &[]))
    }

    /// decimal.c's decimal_result.
    pub fn to_text(&self) -> String {
        let (int, frac) = self.parts();
        let mut out = String::with_capacity(self.digits.len().saturating_add(3));
        let zero_sign = self.digits.is_empty() || self.digits == [0];
        if self.negative && !zero_sign {
            out.push('-');
        }
        if int.is_empty() {
            out.push('0');
        } else {
            let lead = int.iter().take_while(|d| **d == 0).count();
            let keep = int
                .get(lead.min(int.len().saturating_sub(1))..)
                .unwrap_or(int);
            out.extend(keep.iter().map(|d| digit_char(*d)));
        }
        if !frac.is_empty() {
            out.push('.');
            out.extend(frac.iter().map(|d| digit_char(*d)));
        }
        out
    }

    /// decimal.c's decimal_result_sci: a sign always, one digit before the point, trailing
    /// zeros dropped while more than `keep` digits (leading zeros included) remain, then "e"
    /// with a signed exponent of at least two digits.
    pub fn to_sci_text(&self, keep: usize) -> Option<String> {
        let mut kept = self.digits.len();
        while kept > keep && kept.checked_sub(1).and_then(|i| self.digits.get(i)) == Some(&0) {
            kept = kept.checked_sub(1)?;
        }
        let trailing = self.digits.len().checked_sub(kept)?;
        let sig = self.digits.get(..kept).unwrap_or(&[]);
        let lead = sig.iter().take_while(|d| **d == 0).count();
        let sig = sig.get(lead..).unwrap_or(&[]);
        let mut out = String::with_capacity(sig.len().saturating_add(8));
        out.push(if self.negative { '-' } else { '+' });
        let Some((first, rest)) = sig.split_first() else {
            out.push_str("0.0e+00");
            return Some(out);
        };
        out.push(digit_char(*first));
        out.push('.');
        if rest.is_empty() {
            out.push('0');
        } else {
            out.extend(rest.iter().map(|d| digit_char(*d)));
        }
        let frac = i64::try_from(self.frac)
            .ok()?
            .checked_sub(i64::try_from(trailing).ok()?)?;
        let exp = i64::try_from(sig.len())
            .ok()?
            .checked_sub(frac)?
            .checked_sub(1)?;
        out.push_str(&format!("e{exp:+03}"));
        Some(out)
    }

    /// The significant digits (0..=9, most significant first, no leading or trailing zeros) and
    /// where the point falls among them: the magnitude is 0.d1d2... x 10^point. None for NULL or
    /// zero.
    pub fn significant_digits(&self) -> Option<(&[u8], i32)> {
        let lead = self.digits.iter().take_while(|d| **d == 0).count();
        let trail = self.digits.iter().rev().take_while(|d| **d == 0).count();
        let digits = self
            .digits
            .get(lead..self.digits.len().checked_sub(trail)?)?;
        if digits.is_empty() {
            return None;
        }
        let point = i32::try_from(self.int_len())
            .ok()?
            .checked_sub(i32::try_from(lead).ok()?)?;
        Some((digits, point))
    }

    /// decimal.c's decimal_round: N significant digits, rounding half away from zero, and the
    /// digits after them set to zero. None when the carry would need a digit past MAX_DIGIT.
    fn round(&mut self, n: usize) -> Option<()> {
        let zeros = self.digits.iter().take_while(|d| **d == 0).count();
        let n = n.checked_add(zeros)?;
        if self.digits.len() <= n {
            return Some(());
        }
        if self.digits.get(n).is_some_and(|d| *d >= 5) {
            if self.digits.iter().take(n).all(|d| *d == 9) {
                if self.digits.len().checked_add(2)? > MAX_DIGIT {
                    return None;
                }
                self.digits.insert(0, 0);
            }
            let mut i = n.checked_sub(1)?;
            let d = self.digits.get_mut(i)?;
            *d = d.wrapping_add(1);
            while i > 0 && self.digits.get(i).is_some_and(|d| *d > 9) {
                *self.digits.get_mut(i)? = 0;
                i = i.checked_sub(1)?;
                let d = self.digits.get_mut(i)?;
                *d = d.wrapping_add(1);
            }
        }
        self.digits.get_mut(n..)?.fill(0);
        Some(())
    }

    /// The digits widened to `ndigit` with `nfrac` after the point (decimal.c's decimal_expand).
    fn expanded(&self, ndigit: usize, nfrac: usize) -> Option<Vec<u8>> {
        let add_frac = nfrac.checked_sub(self.frac)?;
        let add_sig = ndigit
            .checked_sub(self.digits.len())?
            .checked_sub(add_frac)?;
        let mut out = vec![0u8; add_sig];
        out.extend_from_slice(&self.digits);
        out.resize(ndigit, 0);
        Some(out)
    }
}

fn digit_char(d: u8) -> char {
    char::from(b'0'.wrapping_add(d))
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// decimal.c's decimalNewFromText. Bytes other than digits, the point and an exponent are
/// skipped, so text with no digits is zero. None (decimal.c's out of memory) past MAX_DIGIT.
pub fn decimal_from_text(text: &[u8]) -> Option<Decimal> {
    let mut it = text.iter().copied().skip_while(|c| is_space(*c)).peekable();
    let mut negative = false;
    match it.peek() {
        Some(b'-') => {
            negative = true;
            it.next();
        }
        Some(b'+') => {
            it.next();
        }
        _ => {}
    }
    while it.next_if_eq(&b'0').is_some() {}
    let mut digits = Vec::new();
    let mut dot = None;
    let mut exp: i64 = 0;
    while let Some(c) = it.next() {
        match c {
            b'0'..=b'9' => digits.push(c.wrapping_sub(b'0')),
            b'.' => dot = Some(digits.len()),
            b'e' | b'E' => {
                let mut neg_exp = false;
                match it.peek() {
                    None => break,
                    Some(b'-') => {
                        neg_exp = true;
                        it.next();
                    }
                    Some(b'+') => {
                        it.next();
                    }
                    Some(_) => {}
                }
                for d in it.by_ref() {
                    if exp >= EXPONENT_CAP {
                        break;
                    }
                    if d.is_ascii_digit() {
                        exp = exp
                            .checked_mul(10)?
                            .checked_add(i64::from(d.wrapping_sub(b'0')))?;
                    }
                }
                if neg_exp {
                    exp = exp.checked_neg()?;
                }
                break;
            }
            _ => {}
        }
    }
    let nd = i64::try_from(digits.len()).ok()?;
    let mut nf = match dot {
        Some(d) => nd.checked_sub(i64::try_from(d).ok()?)?,
        None => 0,
    };
    if exp > 0 {
        if nf > 0 {
            if exp <= nf {
                nf = nf.checked_sub(exp)?;
                exp = 0;
            } else {
                exp = exp.checked_sub(nf)?;
                nf = 0;
            }
        }
        if exp > 0 {
            let new_len = digits.len().checked_add(usize::try_from(exp).ok()?)?;
            digits.resize(new_len, 0);
        }
    } else if exp < 0 {
        let mut e = exp.checked_neg()?;
        let extra = nd.checked_sub(nf)?.checked_sub(1)?;
        if extra != 0 {
            if extra >= e {
                nf = nf.checked_add(e)?;
                e = 0;
            } else {
                e = e.checked_sub(extra)?;
                nf = nd.checked_sub(1)?;
            }
        }
        if e > 0 {
            let mut padded = vec![0u8; usize::try_from(e).ok()?];
            padded.extend_from_slice(&digits);
            digits = padded;
            nf = nf.checked_add(e)?;
        }
    }
    if digits.iter().all(|d| *d == 0) {
        negative = false;
    }
    let frac = usize::try_from(nf).ok()?;
    if frac > digits.len() || digits.len() > MAX_DIGIT {
        return None;
    }
    Some(Decimal {
        negative,
        digits,
        frac,
    })
}

/// decimal.c's decimalFromDouble: the exact decimal value of a REAL; None for NaN or infinity.
/// As in decimal.c, the sign is taken off only when `r < 0.0`, so a NaN with its sign bit set
/// reads as a negative biased exponent and gives a number (floatingpoint.html is silent on
/// NaN bit patterns; this is sqlite3 3.54.0's answer for an 8-byte BLOB holding one).
pub fn decimal_from_double(r: f64) -> Option<Decimal> {
    let negative = r < 0.0;
    let r = if negative { r.abs() } else { r };
    let a = i64::from_ne_bytes(r.to_bits().to_ne_bytes());
    let (m, e) = if a == 0 || a == i64::MIN {
        (0i64, 0i64)
    } else {
        let mut e = a.wrapping_shr(SIGNIFICAND_BITS);
        let mut m = a & SIGNIFICAND_MASK;
        if e == 0 {
            m = m.wrapping_shl(1);
        } else {
            m |= 1i64.wrapping_shl(SIGNIFICAND_BITS);
        }
        while e < EXPONENT_OFFSET && m > 0 && m & 1 == 0 {
            m = m.wrapping_shr(1);
            e = e.checked_add(1)?;
        }
        if negative {
            m = m.checked_neg()?;
        }
        let e = e.checked_sub(EXPONENT_OFFSET)?;
        if e > MAX_EXPONENT {
            return None;
        }
        (m, e)
    };
    let a = decimal_from_text(m.to_string().as_bytes())?;
    let x = decimal_pow2(e)?;
    decimal_mul(&a, &x)
}

/// decimal.c's decimalPow2: 2^n exactly, None outside [-POW2_LIMIT, POW2_LIMIT].
pub fn decimal_pow2(n: i64) -> Option<Decimal> {
    if !(-POW2_LIMIT..=POW2_LIMIT).contains(&n) {
        return None;
    }
    let mut a = decimal_from_text(b"1.0")?;
    if n == 0 {
        return Some(a);
    }
    let mut x = decimal_from_text(if n > 0 { b"2.0" } else { b"0.5" })?;
    let mut k = n.unsigned_abs();
    loop {
        if k & 1 == 1 {
            a = decimal_mul(&a, &x)?;
        }
        k = k.wrapping_shr(1);
        if k == 0 {
            break;
        }
        x = decimal_mul(&x, &x)?;
    }
    Some(a)
}

fn add_digits(x: &[u8], y: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; x.len()];
    let mut carry = 0u8;
    for ((o, a), b) in out.iter_mut().rev().zip(x.iter().rev()).zip(y.iter().rev()) {
        let s = a.wrapping_add(*b).wrapping_add(carry);
        (*o, carry) = if s >= 10 {
            (s.wrapping_sub(10), 1)
        } else {
            (s, 0)
        };
    }
    out
}

/// x - y for digit strings of one length with x >= y.
fn sub_digits(x: &[u8], y: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; x.len()];
    let mut borrow = 0u8;
    for ((o, a), b) in out.iter_mut().rev().zip(x.iter().rev()).zip(y.iter().rev()) {
        let t = a.wrapping_add(10).wrapping_sub(*b).wrapping_sub(borrow);
        (*o, borrow) = if t >= 10 {
            (t.wrapping_sub(10), 0)
        } else {
            (t, 1)
        };
    }
    out
}

/// decimal.c's decimal_add: the result keeps the larger of the two fraction lengths. None
/// (decimal.c's out of memory) when the widened operands pass MAX_DIGIT.
pub fn decimal_add(a: &Decimal, b: &Decimal) -> Option<Decimal> {
    let mut nsig = a.int_len();
    if nsig > 0 && a.digits.first() == Some(&0) {
        nsig = nsig.checked_sub(1)?;
    }
    let nsig = nsig.max(b.int_len());
    let nfrac = a.frac.max(b.frac);
    let ndigit = nsig.checked_add(nfrac)?.checked_add(1)?;
    if ndigit.checked_add(1)? > MAX_DIGIT {
        return None;
    }
    let x = a.expanded(ndigit, nfrac)?;
    let y = b.expanded(ndigit, nfrac)?;
    let (digits, negative) = if a.negative == b.negative {
        (add_digits(&x, &y), a.negative)
    } else if x < y {
        (sub_digits(&y, &x), !a.negative)
    } else {
        (sub_digits(&x, &y), a.negative)
    };
    Some(Decimal {
        negative,
        digits,
        frac: nfrac,
    })
}

/// decimal.c's decimalSubFunc: B's sign flipped, then decimal_add.
pub fn decimal_sub(a: &Decimal, b: &Decimal) -> Option<Decimal> {
    let flipped = Decimal {
        negative: !b.negative,
        ..b.clone()
    };
    decimal_add(a, &flipped)
}

/// decimal.c's decimalMul: trailing fraction zeros are dropped down to the smaller of the
/// two inputs' fraction lengths. None (decimal.c's oom flag) when the product passes MAX_DIGIT.
pub fn decimal_mul(a: &Decimal, b: &Decimal) -> Option<Decimal> {
    let nb = b.digits.len();
    let len = a.digits.len().checked_add(nb)?.checked_add(2)?;
    if len > MAX_DIGIT {
        return None;
    }
    let mut acc = vec![0u8; len];
    for (i, f) in a.digits.iter().enumerate().rev() {
        let mut carry = 0u8;
        let mut k = i.checked_add(nb)?.checked_add(2)?;
        for g in b.digits.iter().rev() {
            let cell = acc.get_mut(k)?;
            let x = cell.wrapping_add(f.wrapping_mul(*g)).wrapping_add(carry);
            *cell = x.wrapping_rem(10);
            carry = x.wrapping_div(10);
            k = k.checked_sub(1)?;
        }
        let cell = acc.get_mut(k)?;
        let x = cell.wrapping_add(carry);
        *cell = x.wrapping_rem(10);
        let up = acc.get_mut(k.checked_sub(1)?)?;
        *up = up.wrapping_add(x.wrapping_div(10));
    }
    let min_frac = a.frac.min(b.frac);
    let mut frac = a.frac.checked_add(b.frac)?;
    while frac > min_frac && acc.last() == Some(&0) {
        acc.pop();
        frac = frac.checked_sub(1)?;
    }
    Some(Decimal {
        negative: a.negative != b.negative,
        digits: acc,
        frac,
    })
}

/// Numeric order: leading integer zeros and trailing fraction zeros don't count, and zero
/// has no sign ('10' = '10.0', '9' = '9.0').
pub fn decimal_cmp(a: &Decimal, b: &Decimal) -> Ordering {
    fn magnitude(d: &Decimal) -> (&[u8], &[u8]) {
        let (int, frac) = d.parts();
        let lead = int.iter().take_while(|x| **x == 0).count();
        let trail = frac.iter().rev().take_while(|x| **x == 0).count();
        (
            int.get(lead..).unwrap_or(&[]),
            frac.get(..frac.len().saturating_sub(trail)).unwrap_or(&[]),
        )
    }
    let (ai, af) = magnitude(a);
    let (bi, bf) = magnitude(b);
    let a_neg = a.negative && !(ai.is_empty() && af.is_empty());
    let b_neg = b.negative && !(bi.is_empty() && bf.is_empty());
    match (a_neg, b_neg) {
        (true, false) => return Ordering::Less,
        (false, true) => return Ordering::Greater,
        _ => {}
    }
    let mag = ai
        .len()
        .cmp(&bi.len())
        .then_with(|| ai.cmp(bi))
        .then_with(|| af.cmp(bf));
    if a_neg {
        mag.reverse()
    } else {
        mag
    }
}

/// The "decimal" collating sequence (decimal.c's decimalCollFunc).
pub fn decimal_collate(a: &str, b: &str) -> Ordering {
    match (
        decimal_from_text(a.as_bytes()),
        decimal_from_text(b.as_bytes()),
    ) {
        (Some(x), Some(y)) => decimal_cmp(&x, &y),
        _ => Ordering::Equal,
    }
}

/// decimal.c's decimal_new returns a decimal, no decimal (a NULL, a BLOB that is not 8 bytes
/// long when read as a REAL, a NaN or an infinity), or fails with out of memory (text past
/// MAX_DIGIT). What each function then does with "no decimal" differs, as in decimal.c.
enum Arg {
    Num(Decimal),
    Absent,
    OutOfMemory,
}

/// decimal.c's decimal_new. `text_only` (decimal_add, _sub, _mul, _cmp, _sum) reads a REAL
/// or a BLOB as its text; otherwise a REAL is expanded exactly and an 8-byte BLOB is read
/// as a big-endian IEEE 754 double.
fn decimal_new(v: &Value, text_only: bool) -> Arg {
    let parsed = |text: &[u8]| match decimal_from_text(text) {
        Some(d) => Arg::Num(d),
        None => Arg::OutOfMemory,
    };
    let exact = |r: f64| match decimal_from_double(r) {
        Some(d) => Arg::Num(d),
        None => Arg::Absent,
    };
    match v.value_type() {
        ValueType::Null | ValueType::Error => Arg::Absent,
        ValueType::Integer => parsed(v.to_integer().unwrap_or_default().to_string().as_bytes()),
        ValueType::Text => parsed(v.to_text().unwrap_or_default().as_bytes()),
        ValueType::Float => {
            let r = v.to_float().unwrap_or_default();
            if text_only {
                parsed(real_to_text(r).as_bytes())
            } else {
                exact(r)
            }
        }
        ValueType::Blob => {
            let bytes = v.blob_ref().unwrap_or_default();
            if text_only {
                parsed(bytes)
            } else {
                match <[u8; 8]>::try_from(bytes) {
                    Ok(b) => exact(f64::from_be_bytes(b)),
                    Err(_) => Arg::Absent,
                }
            }
        }
    }
}

/// sqlite3_value_int: an INTEGER's low 32 bits; a REAL truncated toward zero (saturating)
/// first; text and BLOBs by their leading integer; NULL is 0.
fn value_int(v: &Value) -> i32 {
    let wide = match v.value_type() {
        ValueType::Integer => v.to_integer().unwrap_or_default(),
        ValueType::Float => v.to_integer().unwrap_or_default(),
        ValueType::Text => leading_integer(v.to_text().unwrap_or_default().as_bytes()),
        ValueType::Blob => leading_integer(v.blob_ref().unwrap_or_default()),
        ValueType::Null | ValueType::Error => 0,
    };
    let [a, b, c, d, ..] = wide.to_le_bytes();
    i32::from_le_bytes([a, b, c, d])
}

fn leading_integer(text: &[u8]) -> i64 {
    let mut it = text.iter().copied().skip_while(|c| is_space(*c)).peekable();
    let negative = match it.peek() {
        Some(b'-') => {
            it.next();
            true
        }
        Some(b'+') => {
            it.next();
            false
        }
        _ => false,
    };
    let mut n: i64 = 0;
    for c in it.take_while(u8::is_ascii_digit) {
        let d = i64::from(c.wrapping_sub(b'0'));
        n = n
            .saturating_mul(10)
            .saturating_add(if negative { d.saturating_neg() } else { d });
    }
    n
}

fn out_of_memory() -> Value {
    Value::error_with_message("out of memory".into())
}

fn arity_error(name: &str) -> Value {
    Value::error_with_message(format!("wrong number of arguments to function {name}()"))
}

/// decimal.c's decimalFunc: decimal(X), decimal(X,N), decimal_exp(X), decimal_exp(X,N).
fn decimal_func(args: &[Value], name: &str, sci: bool) -> Value {
    let (x, n) = match args {
        [x] => (x, 0),
        [x, n] => (x, value_int(n)),
        _ => return arity_error(name),
    };
    let mut d = match decimal_new(x, false) {
        Arg::Num(d) => d,
        Arg::Absent => return Value::null(),
        Arg::OutOfMemory => return out_of_memory(),
    };
    let n = usize::try_from(n).unwrap_or(0);
    if n > 0 && d.round(n).is_none() {
        return out_of_memory();
    }
    if !sci {
        return Value::from_text(d.to_text());
    }
    match d.to_sci_text(n) {
        Some(s) => Value::from_text(s),
        None => out_of_memory(),
    }
}

#[scalar(name = "decimal")]
fn decimal_sql(args: &[Value]) -> Value {
    decimal_func(args, "decimal", false)
}

#[scalar(name = "decimal_exp")]
fn decimal_exp_sql(args: &[Value]) -> Value {
    decimal_func(args, "decimal_exp", true)
}

/// decimal.c's decimalAddFunc: a missing operand on either side is out of memory.
#[scalar(name = "decimal_add")]
fn decimal_add_sql(args: &[Value]) -> Value {
    add_func(args)
}

fn add_func(args: &[Value]) -> Value {
    let [a, b] = args else {
        return arity_error("decimal_add");
    };
    match (decimal_new(a, true), decimal_new(b, true)) {
        (Arg::Num(a), Arg::Num(b)) => match decimal_add(&a, &b) {
            Some(sum) => Value::from_text(sum.to_text()),
            None => out_of_memory(),
        },
        _ => out_of_memory(),
    }
}

/// decimal.c's decimalSubFunc: a missing right operand leaves the result NULL, a missing left
/// one (with a right one present) is out of memory.
#[scalar(name = "decimal_sub")]
fn decimal_sub_sql(args: &[Value]) -> Value {
    sub_func(args)
}

fn sub_func(args: &[Value]) -> Value {
    let [a, b] = args else {
        return arity_error("decimal_sub");
    };
    match (decimal_new(a, true), decimal_new(b, true)) {
        (Arg::OutOfMemory, _) | (_, Arg::OutOfMemory) => out_of_memory(),
        (_, Arg::Absent) => Value::null(),
        (Arg::Absent, Arg::Num(_)) => out_of_memory(),
        (Arg::Num(a), Arg::Num(b)) => match decimal_sub(&a, &b) {
            Some(d) => Value::from_text(d.to_text()),
            None => out_of_memory(),
        },
    }
}

/// decimal.c's decimalMulFunc: a missing operand, or a product past MAX_DIGIT, is NULL.
#[scalar(name = "decimal_mul")]
fn decimal_mul_sql(args: &[Value]) -> Value {
    mul_func(args)
}

fn mul_func(args: &[Value]) -> Value {
    let [a, b] = args else {
        return arity_error("decimal_mul");
    };
    match (decimal_new(a, true), decimal_new(b, true)) {
        (Arg::OutOfMemory, _) | (_, Arg::OutOfMemory) => out_of_memory(),
        (Arg::Num(a), Arg::Num(b)) => match decimal_mul(&a, &b) {
            Some(p) => Value::from_text(p.to_text()),
            None => Value::null(),
        },
        _ => Value::null(),
    }
}

/// decimal.c's decimalCmpFunc: Y is not read when X is missing.
#[scalar(name = "decimal_cmp")]
fn decimal_cmp_sql(args: &[Value]) -> Value {
    cmp_func(args)
}

fn cmp_func(args: &[Value]) -> Value {
    let [a, b] = args else {
        return arity_error("decimal_cmp");
    };
    let a = match decimal_new(a, true) {
        Arg::Num(a) => a,
        Arg::Absent => return Value::null(),
        Arg::OutOfMemory => return out_of_memory(),
    };
    let b = match decimal_new(b, true) {
        Arg::Num(b) => b,
        Arg::Absent => return Value::null(),
        Arg::OutOfMemory => return out_of_memory(),
    };
    Value::from_integer(match decimal_cmp(&a, &b) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    })
}

/// decimal.c's decimalPow2Func acts only on an INTEGER argument; anything else is NULL, and an
/// N outside [-POW2_LIMIT, POW2_LIMIT] is out of memory. decimal.c reads N as a C int, so
/// 3.54.0 wraps 4294967297 to 1; floatingpoint.html 2.2.2 says "N must be an integer between
/// -20000 and +20000", so the page wins and the whole INTEGER is checked against the bound.
#[scalar(name = "decimal_pow2")]
fn decimal_pow2_sql(args: &[Value]) -> Value {
    pow2_func(args)
}

fn pow2_func(args: &[Value]) -> Value {
    let [n] = args else {
        return arity_error("decimal_pow2");
    };
    if n.value_type() != ValueType::Integer {
        return Value::null();
    }
    match decimal_pow2(n.to_integer().unwrap_or_default()).and_then(|p| p.to_sci_text(0)) {
        Some(s) => Value::from_text(s),
        None => out_of_memory(),
    }
}

#[derive(Default)]
struct DecimalSumState {
    /// None until the first row: decimal_sum over no rows is NULL, over only NULLs '0'.
    acc: Option<Decimal>,
    out_of_memory: bool,
}

#[derive(AggregateDerive)]
struct DecimalSum;

impl AggFunc for DecimalSum {
    type State = DecimalSumState;
    type Error = &'static str;
    const NAME: &'static str = "decimal_sum";
    const ARGS: i32 = 1;

    fn step(state: &mut Self::State, args: &[Value]) {
        if state.out_of_memory {
            return;
        }
        let acc = state.acc.get_or_insert_with(Decimal::zero);
        let Some(arg) = args.first() else {
            return;
        };
        if arg.value_type() == ValueType::Null {
            return;
        }
        let sum = match decimal_new(arg, true) {
            Arg::Num(x) => decimal_add(acc, &x),
            Arg::Absent | Arg::OutOfMemory => None,
        };
        match sum {
            Some(sum) => *acc = sum,
            None => state.out_of_memory = true,
        }
    }

    fn finalize(state: Self::State) -> Result<Value, Self::Error> {
        if state.out_of_memory {
            return Err("out of memory");
        }
        Ok(match state.acc {
            Some(acc) => Value::from_text(acc.to_text()),
            None => Value::null(),
        })
    }
}

pub fn register_extension(ext_api: &mut ExtensionApi) {
    unsafe {
        register_scalar_function(ext_api.ctx, c"decimal".as_ptr(), decimal_sql);
        register_scalar_function(ext_api.ctx, c"decimal_exp".as_ptr(), decimal_exp_sql);
        register_scalar_function(ext_api.ctx, c"decimal_add".as_ptr(), decimal_add_sql);
        register_scalar_function(ext_api.ctx, c"decimal_sub".as_ptr(), decimal_sub_sql);
        register_scalar_function(ext_api.ctx, c"decimal_mul".as_ptr(), decimal_mul_sql);
        register_scalar_function(ext_api.ctx, c"decimal_cmp".as_ptr(), decimal_cmp_sql);
        register_scalar_function(ext_api.ctx, c"decimal_pow2".as_ptr(), decimal_pow2_sql);
        DecimalSum::register_DecimalSum(ext_api);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every expected answer below is what sqlite3 3.54.0 (2026-10-09) printed on adacachy2
    // for the same call (lane G15, bench/records/2026-11-23-adacachy2-perf-numeric-g15.md);
    // "NULL" is a NULL result and "OOM" its "out of memory" error.

    fn text(s: &str) -> Value {
        Value::from_text(s.to_string())
    }

    fn shown(v: Value) -> String {
        match v.value_type() {
            ValueType::Null => "NULL".to_string(),
            ValueType::Error => "OOM".to_string(),
            ValueType::Integer => v.to_integer().unwrap_or_default().to_string(),
            _ => v.to_text().unwrap_or_default().to_string(),
        }
    }

    fn call(f: fn(&[Value]) -> Value, args: &[Value]) -> String {
        shown(f(args))
    }

    fn dec(args: &[Value]) -> Value {
        decimal_func(args, "decimal", false)
    }

    fn dec_exp(args: &[Value]) -> Value {
        decimal_func(args, "decimal_exp", true)
    }

    fn sum(rows: &[Value]) -> String {
        let mut state = DecimalSumState::default();
        for row in rows {
            DecimalSum::step(&mut state, std::slice::from_ref(row));
        }
        match DecimalSum::finalize(state) {
            Ok(v) => shown(v),
            Err(_) => "OOM".to_string(),
        }
    }

    #[test]
    fn g15_nulls_follow_decimal_c_per_function() {
        let one = || Value::from_integer(1);
        assert_eq!(call(add_func, &[Value::null(), one()]), "OOM");
        assert_eq!(call(add_func, &[one(), Value::null()]), "OOM");
        assert_eq!(call(add_func, &[Value::null(), Value::null()]), "OOM");
        assert_eq!(call(sub_func, &[Value::null(), one()]), "OOM");
        assert_eq!(call(sub_func, &[one(), Value::null()]), "NULL");
        assert_eq!(call(sub_func, &[Value::null(), Value::null()]), "NULL");
        assert_eq!(call(mul_func, &[Value::null(), one()]), "NULL");
        assert_eq!(call(mul_func, &[one(), Value::null()]), "NULL");
        assert_eq!(call(cmp_func, &[Value::null(), one()]), "NULL");
        assert_eq!(call(cmp_func, &[one(), Value::null()]), "NULL");
        assert_eq!(call(dec, &[Value::null()]), "NULL");
        assert_eq!(call(dec_exp, &[Value::null()]), "NULL");
        assert_eq!(call(dec, &[Value::null(), Value::from_integer(2)]), "NULL");
        assert_eq!(call(pow2_func, &[Value::null()]), "NULL");
        assert_eq!(sum(&[Value::null()]), "0");
        assert_eq!(sum(&[]), "NULL");
    }

    #[test]
    fn g15_empty_and_malformed_text() {
        assert_eq!(call(dec, &[text("")]), "0");
        assert_eq!(call(dec_exp, &[text("")]), "+0.0e+00");
        assert_eq!(call(dec, &[text("abc")]), "0");
        assert_eq!(call(add_func, &[text(""), text("")]), "0");
        assert_eq!(call(dec, &[text("1e")]), "1");
        assert_eq!(call(dec, &[text("1e-")]), "1");
        assert_eq!(call(dec, &[text("1e1x2")]), "1000000000000");
        assert_eq!(call(dec, &[text("1x2.5")]), "12.5");
        assert_eq!(call(dec, &[text(".5e-1")]), "0.05");
        assert_eq!(call(add_func, &[text("  -  5"), text("1")]), "-4");
        assert_eq!(call(dec, &[Value::from_blob(vec![1, 2])]), "NULL");
        assert_eq!(
            call(
                dec,
                &[Value::from_blob(
                    0x7FF8_0000_0000_0000u64.to_be_bytes().to_vec()
                )]
            ),
            "NULL"
        );
        assert_eq!(
            call(
                dec,
                &[Value::from_blob(
                    0xFFF0_0000_0000_0000u64.to_be_bytes().to_vec()
                )]
            ),
            "NULL"
        );
        assert_eq!(
            sum(&[
                Value::from_integer(1),
                Value::null(),
                text("2.50"),
                Value::from_blob(b"1".to_vec())
            ]),
            "4.50"
        );
        assert_eq!(
            call(add_func, &[Value::from_float(1.5), Value::from_integer(2)]),
            "3.5"
        );
    }

    #[test]
    fn g15_signs_and_zeros() {
        assert_eq!(call(dec, &[text("-0.0")]), "0.0");
        assert_eq!(call(dec_exp, &[text("-0")]), "+0.0e+00");
        assert_eq!(call(dec_exp, &[text("-0.05")]), "-5.0e-02");
        assert_eq!(call(add_func, &[text("-1"), text("1")]), "-0");
        assert_eq!(call(sub_func, &[text("-1"), text("-1")]), "-0");
        assert_eq!(call(mul_func, &[text("-1"), text("0")]), "-0");
        assert_eq!(call(cmp_func, &[text("-0"), text("0")]), "0");
        assert_eq!(call(dec, &[text("-1.25"), Value::from_integer(2)]), "-1.30");
    }

    #[test]
    fn g15_rounding_to_n_significant_digits() {
        let n = Value::from_integer;
        assert_eq!(call(dec, &[text("1.23456"), n(3)]), "1.23000");
        assert_eq!(call(dec_exp, &[text("1.23456"), n(3)]), "+1.23e+00");
        assert_eq!(call(dec, &[text("9996"), n(3)]), "10000");
        assert_eq!(call(dec_exp, &[text("9996"), n(3)]), "+1.00e+04");
        assert_eq!(call(dec, &[text("0.0996"), n(2)]), "0.1000");
        assert_eq!(call(dec_exp, &[text("0.0996"), n(2)]), "+1.0e-01");
        assert_eq!(call(dec, &[text("1.25"), n(0)]), "1.25");
        assert_eq!(call(dec, &[text("1.25"), n(-1)]), "1.25");
        assert_eq!(call(dec, &[text("1.25"), Value::null()]), "1.25");
        assert_eq!(call(dec, &[text("1.25"), text("2")]), "1.30");
        assert_eq!(call(dec, &[text("1.25"), Value::from_float(2.9)]), "1.30");
        assert_eq!(call(dec, &[text("1.25"), n(4_294_967_298)]), "1.30");
        assert_eq!(call(dec_exp, &[text("1.5000"), n(3)]), "+1.50e+00");
        assert_eq!(call(dec_exp, &[text("1200"), n(0)]), "+1.2e+03");
        assert_eq!(call(dec_exp, &[text("1200"), n(3)]), "+1.20e+03");
        assert_eq!(call(dec, &[Value::from_float(0.5), n(1)]), "0.5");
        let big = call(dec, &[Value::from_float(1.5e300), n(1)]);
        assert_eq!((big.len(), big.get(..2)), (301, Some("20")));
        assert!(big.bytes().skip(1).all(|b| b == b'0'));
    }

    #[test]
    fn g15_huge_exponents() {
        assert_eq!(call(dec, &[text("1e9999999")]).len(), 10_000_000);
        assert_eq!(call(dec, &[text("1e99999999999")]).len(), 10_000_000);
        assert_eq!(call(dec, &[text("12e9999999")]), "OOM");
        assert_eq!(call(add_func, &[text("1e9999998"), text("1")]), "OOM");
        assert_eq!(
            call(add_func, &[text("1e9999997"), text("1")]).len(),
            9_999_998
        );
        assert_eq!(
            call(mul_func, &[text("1e5000000"), text("1e5000000")]),
            "NULL"
        );
        assert_eq!(call(cmp_func, &[text("12e9999999"), text("1")]), "OOM");
        assert_eq!(
            call(dec, &[text("99e9999997"), Value::from_integer(1)]),
            "OOM"
        );
        assert_eq!(sum(&[text("12e9999999")]), "OOM");
    }

    #[test]
    fn g15_decimal_pow2_fails_out_of_range() {
        // 3.54.0 answers +2.0e+00 here (its C int wraps); floatingpoint.html 2.2.2 bounds N.
        assert_eq!(
            call(pow2_func, &[Value::from_integer(4_294_967_297)]),
            "OOM"
        );
        assert_eq!(call(pow2_func, &[Value::from_integer(20_001)]), "OOM");
        assert_eq!(call(pow2_func, &[Value::from_integer(-20_001)]), "OOM");
        assert_eq!(call(pow2_func, &[Value::from_float(2.0)]), "NULL");
        assert_eq!(call(pow2_func, &[text("3")]), "NULL");
    }

    #[test]
    fn decimal_of_a_real_is_its_exact_expansion() {
        let plain = |r: f64| decimal_from_double(r).map(|d| d.to_text());
        assert_eq!(
            plain(47.49).as_deref(),
            Some("47.49000000000000198951966012828052043914794921875")
        );
        assert_eq!(
            plain(0.1).as_deref(),
            Some("0.1000000000000000055511151231257827021181583404541015625")
        );
        assert_eq!(plain(12.0).as_deref(), Some("12"));
        assert_eq!(plain(f64::NAN), None);
    }

    #[test]
    fn significant_digits_drop_leading_and_trailing_zeros_and_place_the_point() {
        let sig = |r: f64| {
            decimal_from_double(r)
                .and_then(|d| d.significant_digits().map(|(s, p)| (s.to_vec(), p)))
        };
        assert_eq!(
            sig(47.49).map(|(s, p)| (s.len(), s.get(..4).map(<[u8]>::to_vec), p)),
            Some((49, Some(vec![4, 7, 4, 9]), 2))
        );
        assert_eq!(
            sig(0.001).map(|(s, p)| (s.first().copied(), p)),
            Some((Some(1), -2))
        );
        assert_eq!(sig(-0.25), Some((vec![2, 5], 0)));
        assert_eq!(sig(1200.0), Some((vec![1, 2], 4)));
        assert_eq!(sig(0.0625), Some((vec![6, 2, 5], -1)));
        assert_eq!(sig(0.0), None);
        assert_eq!(sig(f64::INFINITY), None);
    }

    #[test]
    fn decimal_exp_writes_exponential_notation() {
        assert_eq!(
            call(dec_exp, &[Value::from_float(47.49)]),
            "+4.749000000000000198951966012828052043914794921875e+01"
        );
        assert_eq!(call(dec_exp, &[text("12")]), "+1.2e+01");
        assert_eq!(call(dec_exp, &[text("0012.500")]), "+1.25e+01");
        assert_eq!(call(dec_exp, &[text("0.05")]), "+5.0e-02");
    }

    #[test]
    fn decimal_add_sub_mul_and_sum() {
        assert_eq!(call(add_func, &[text("0.1"), text("0.2")]), "0.3");
        assert_eq!(call(add_func, &[text("1.10"), text("2.205")]), "3.305");
        assert_eq!(call(sub_func, &[text("1"), text("2.5")]), "-1.5");
        assert_eq!(call(mul_func, &[text("0.10"), text("0.20")]), "0.02");
        assert_eq!(call(mul_func, &[text("1.23"), text("2.34")]), "2.8782");
        assert_eq!(call(mul_func, &[text("-3"), text("4")]), "-12");
        assert_eq!(sum(&[text("0.1"), text("0.2"), text("0.3")]), "0.6");
    }

    #[test]
    fn decimal_cmp_orders_numerically() {
        assert_eq!(call(cmp_func, &[text("10"), text("10.0")]), "0");
        assert_eq!(call(cmp_func, &[text("1.5"), text("10")]), "-1");
        assert_eq!(call(cmp_func, &[text("-1"), text("1")]), "-1");
        assert_eq!(call(cmp_func, &[text("2"), text("1")]), "1");
        assert_eq!(call(cmp_func, &[text("-2"), text("-10")]), "1");
    }

    #[test]
    fn g15_nan_blobs_with_the_sign_bit_give_decimal_c_numbers() {
        let blob = |bits: u64| Value::from_blob(bits.to_be_bytes().to_vec());
        let plain = call(dec, &[blob(0xFFF8_0000_0000_0000)]);
        let sci = call(dec_exp, &[blob(0xFFF8_0000_0000_0000)]);
        assert_eq!((plain.len(), sci.len()), (1027, 724));
        assert_eq!(
            (sci.get(..24), sci.get(712..)),
            (Some("+8.344026969402005186588"), Some("3359375e-309"))
        );
        let plain = call(dec, &[blob(u64::MAX)]);
        let sci = call(dec_exp, &[blob(u64::MAX)]);
        assert_eq!((plain.len(), sci.len()), (1078, 776));
        assert_eq!(
            (sci.get(..24), sci.get(764..)),
            (Some("+1.112536929253600568028"), Some("8359375e-308"))
        );
    }

    /// floatingpoint.html 2.2.4 and 2.2.6 compare values in numeric order; 3.54.0 answers 1,
    /// 1 and 1 here and sorts '1e-2' after '0.5', counting a zero that "1e-2" and "0 012"
    /// leave before the point as a digit. The page wins.
    #[test]
    fn g15_cmp_and_collation_compare_values_as_the_page_says() {
        assert_eq!(call(cmp_func, &[text("1e-2"), text("0.01")]), "0");
        assert_eq!(call(cmp_func, &[text("1e-2"), text("0.5")]), "-1");
        assert_eq!(call(cmp_func, &[text("0 012"), text("12")]), "0");
        let mut v = ["10", "9", "1e-2", "0.5"];
        v.sort_by(|a, b| decimal_collate(a, b));
        assert_eq!(v, ["1e-2", "0.5", "9", "10"]);
    }

    #[test]
    fn decimal_collation_sorts_in_numeric_order() {
        let mut v = ["10", "9", "1.5", "-2", "9.0"];
        v.sort_by(|a, b| decimal_collate(a, b));
        assert_eq!(v, ["-2", "1.5", "9", "9.0", "10"]);
    }
}
