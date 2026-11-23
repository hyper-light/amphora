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

/// IEEE 754 binary64: 52 stored significand bits; a biased exponent e and significand m
/// give m * 2^(e - 1075) (bias 1023 plus the 52 fraction bits), as decimal.c's decimalFromDouble.
const SIGNIFICAND_BITS: u32 = 52;
const SIGNIFICAND_MASK: u64 = (1 << SIGNIFICAND_BITS) - 1;
const EXPONENT_OFFSET: i64 = 1075;

/// decimal.c's `Decimal`: sign, a[] (one digit 0..=9 per byte, most significant first),
/// nFrac (how many of the digits follow the point) and isNull. `frac <= digits.len()` always.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Decimal {
    negative: bool,
    digits: Vec<u8>,
    frac: usize,
    is_null: bool,
}

impl Decimal {
    pub fn null() -> Self {
        Self { is_null: true, ..Self::default() }
    }

    fn zero() -> Self {
        Self { digits: vec![0], ..Self::default() }
    }

    fn int_len(&self) -> usize {
        self.digits.len().saturating_sub(self.frac)
    }

    fn parts(&self) -> (&[u8], &[u8]) {
        self.digits.split_at_checked(self.int_len()).unwrap_or((&self.digits, &[]))
    }

    /// decimal.c's decimal_result: NULL for a NULL decimal.
    pub fn to_text(&self) -> Option<String> {
        if self.is_null {
            return None;
        }
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
            let keep = int.get(lead.min(int.len().saturating_sub(1))..).unwrap_or(int);
            out.extend(keep.iter().map(|d| digit_char(*d)));
        }
        if !frac.is_empty() {
            out.push('.');
            out.extend(frac.iter().map(|d| digit_char(*d)));
        }
        Some(out)
    }

    /// decimal.c's decimal_result_sci: a sign always, one digit before the point, trailing
    /// zeros dropped, then "e" with a signed exponent of at least two digits.
    pub fn to_sci_text(&self) -> Option<String> {
        if self.is_null {
            return None;
        }
        let trailing = self.digits.iter().rev().take_while(|d| **d == 0).count();
        let kept = self.digits.len().saturating_sub(trailing);
        let sig = self.digits.get(..kept).unwrap_or(&[]);
        let lead = sig.iter().take_while(|d| **d == 0).count();
        let sig = sig.get(lead..).unwrap_or(&[]);
        let mut out = String::with_capacity(sig.len().saturating_add(8));
        let Some((first, rest)) = sig.split_first() else {
            out.push_str("+0.0e+00");
            return Some(out);
        };
        out.push(if self.negative { '-' } else { '+' });
        out.push(digit_char(*first));
        out.push('.');
        if rest.is_empty() {
            out.push('0');
        } else {
            out.extend(rest.iter().map(|d| digit_char(*d)));
        }
        let frac = i64::try_from(self.frac).ok()?.checked_sub(i64::try_from(trailing).ok()?)?;
        let exp = i64::try_from(sig.len()).ok()?.checked_sub(frac)?.checked_sub(1)?;
        out.push_str(&format!("e{exp:+03}"));
        Some(out)
    }

    /// The significant digits (0..=9, most significant first, no leading or trailing zeros) and
    /// where the point falls among them: the magnitude is 0.d1d2... x 10^point. None for NULL or
    /// zero.
    pub fn significant_digits(&self) -> Option<(&[u8], i32)> {
        if self.is_null {
            return None;
        }
        let lead = self.digits.iter().take_while(|d| **d == 0).count();
        let trail = self.digits.iter().rev().take_while(|d| **d == 0).count();
        let digits = self.digits.get(lead..self.digits.len().checked_sub(trail)?)?;
        if digits.is_empty() {
            return None;
        }
        let point = i32::try_from(self.int_len()).ok()?.checked_sub(i32::try_from(lead).ok()?)?;
        Some((digits, point))
    }

    /// The digits widened to `ndigit` with `nfrac` after the point (decimal.c's decimal_expand).
    fn expanded(&self, ndigit: usize, nfrac: usize) -> Option<Vec<u8>> {
        let add_frac = nfrac.checked_sub(self.frac)?;
        let add_sig = ndigit.checked_sub(self.digits.len())?.checked_sub(add_frac)?;
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
/// skipped, so text with no digits is zero. None only when a length overflows.
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
                        exp = exp.checked_mul(10)?.checked_add(i64::from(d.wrapping_sub(b'0')))?;
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
    if frac > digits.len() {
        return None;
    }
    Some(Decimal { negative, digits, frac, is_null: false })
}

/// decimal.c's decimalFromDouble: the exact decimal value of a REAL; NULL for NaN or infinity.
pub fn decimal_from_double(r: f64) -> Option<Decimal> {
    if !r.is_finite() {
        return Some(Decimal::null());
    }
    let negative = r < 0.0;
    let bits = r.abs().to_bits();
    let (m, e) = if bits == 0 {
        (0u64, 0i64)
    } else {
        let mut e = i64::try_from(bits.wrapping_shr(SIGNIFICAND_BITS)).ok()?;
        let mut m = bits & SIGNIFICAND_MASK;
        if e == 0 {
            m = m.wrapping_shl(1);
        } else {
            m |= 1u64.wrapping_shl(SIGNIFICAND_BITS);
        }
        while e < EXPONENT_OFFSET && m > 0 && m & 1 == 0 {
            m = m.wrapping_shr(1);
            e = e.checked_add(1)?;
        }
        (m, e.checked_sub(EXPONENT_OFFSET)?)
    };
    let text = if negative { format!("-{m}") } else { m.to_string() };
    let a = decimal_from_text(text.as_bytes())?;
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
        (*o, carry) = if s >= 10 { (s.wrapping_sub(10), 1) } else { (s, 0) };
    }
    out
}

/// x - y for digit strings of one length with x >= y.
fn sub_digits(x: &[u8], y: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; x.len()];
    let mut borrow = 0u8;
    for ((o, a), b) in out.iter_mut().rev().zip(x.iter().rev()).zip(y.iter().rev()) {
        let t = a.wrapping_add(10).wrapping_sub(*b).wrapping_sub(borrow);
        (*o, borrow) = if t >= 10 { (t.wrapping_sub(10), 0) } else { (t, 1) };
    }
    out
}

/// decimal.c's decimal_add: the result keeps the larger of the two fraction lengths.
pub fn decimal_add(a: &Decimal, b: &Decimal) -> Option<Decimal> {
    if a.is_null || b.is_null {
        return Some(Decimal::null());
    }
    let mut nsig = a.int_len();
    if nsig > 0 && a.digits.first() == Some(&0) {
        nsig = nsig.checked_sub(1)?;
    }
    let nsig = nsig.max(b.int_len());
    let nfrac = a.frac.max(b.frac);
    let ndigit = nsig.checked_add(nfrac)?.checked_add(1)?;
    let x = a.expanded(ndigit, nfrac)?;
    let y = b.expanded(ndigit, nfrac)?;
    let (digits, negative) = if a.negative == b.negative {
        (add_digits(&x, &y), a.negative)
    } else if x < y {
        (sub_digits(&y, &x), !a.negative)
    } else {
        (sub_digits(&x, &y), a.negative)
    };
    Some(Decimal { negative, digits, frac: nfrac, is_null: false })
}

/// decimal.c's decimalSubFunc: B's sign flipped, then decimal_add.
pub fn decimal_sub(a: &Decimal, b: &Decimal) -> Option<Decimal> {
    let flipped = Decimal { negative: !b.negative, ..b.clone() };
    decimal_add(a, &flipped)
}

/// decimal.c's decimalMul: trailing fraction zeros are dropped down to the smaller of the
/// two inputs' fraction lengths.
pub fn decimal_mul(a: &Decimal, b: &Decimal) -> Option<Decimal> {
    if a.is_null || b.is_null {
        return Some(Decimal::null());
    }
    let nb = b.digits.len();
    let mut acc = vec![0u8; a.digits.len().checked_add(nb)?.checked_add(2)?];
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
    Some(Decimal { negative: a.negative != b.negative, digits: acc, frac, is_null: false })
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
    let mag = ai.len().cmp(&bi.len()).then_with(|| ai.cmp(bi)).then_with(|| af.cmp(bf));
    if a_neg {
        mag.reverse()
    } else {
        mag
    }
}

/// The "decimal" collating sequence (decimal.c's decimalCollFunc).
pub fn decimal_collate(a: &str, b: &str) -> Ordering {
    match (decimal_from_text(a.as_bytes()), decimal_from_text(b.as_bytes())) {
        (Some(x), Some(y)) => decimal_cmp(&x, &y),
        _ => Ordering::Equal,
    }
}

/// decimal.c's decimal_new. `text_only` (decimal_add, _sub, _mul, _cmp, _sum) reads a REAL
/// or a BLOB as its text; otherwise a REAL is expanded exactly and an 8-byte BLOB is read
/// as a big-endian IEEE 754 double. A NULL is a NULL decimal.
fn decimal_new(v: &Value, text_only: bool) -> Option<Decimal> {
    match v.value_type() {
        ValueType::Null | ValueType::Error => Some(Decimal::null()),
        ValueType::Integer => decimal_from_text(v.to_integer().unwrap_or_default().to_string().as_bytes()),
        ValueType::Text => decimal_from_text(v.to_text().unwrap_or_default().as_bytes()),
        ValueType::Float => {
            let r = v.to_float().unwrap_or_default();
            if text_only {
                decimal_from_text(real_to_text(r).as_bytes())
            } else {
                decimal_from_double(r)
            }
        }
        ValueType::Blob => {
            let bytes = v.blob_ref().unwrap_or_default();
            if text_only {
                decimal_from_text(bytes)
            } else {
                match <[u8; 8]>::try_from(bytes) {
                    Ok(b) => decimal_from_double(f64::from_be_bytes(b)),
                    Err(_) => Some(Decimal::null()),
                }
            }
        }
    }
}

fn out_of_memory() -> Value {
    Value::error_with_message("out of memory".into())
}

fn text_result(d: Option<Decimal>, sci: bool) -> Value {
    let Some(d) = d else {
        return out_of_memory();
    };
    match if sci { d.to_sci_text() } else { d.to_text() } {
        Some(s) => Value::from_text(s),
        None => Value::null(),
    }
}

fn arity_error(name: &str) -> Value {
    Value::error_with_message(format!("wrong number of arguments to function {name}()"))
}

fn unary(args: &[Value], name: &str, sci: bool) -> Value {
    let [x] = args else {
        return arity_error(name);
    };
    text_result(decimal_new(x, false), sci)
}

fn binary(args: &[Value], name: &str, op: fn(&Decimal, &Decimal) -> Option<Decimal>) -> Value {
    let [a, b] = args else {
        return arity_error(name);
    };
    let (Some(a), Some(b)) = (decimal_new(a, true), decimal_new(b, true)) else {
        return out_of_memory();
    };
    text_result(op(&a, &b), false)
}

#[scalar(name = "decimal")]
fn decimal_sql(args: &[Value]) -> Value {
    unary(args, "decimal", false)
}

#[scalar(name = "decimal_exp")]
fn decimal_exp_sql(args: &[Value]) -> Value {
    unary(args, "decimal_exp", true)
}

#[scalar(name = "decimal_add")]
fn decimal_add_sql(args: &[Value]) -> Value {
    binary(args, "decimal_add", decimal_add)
}

#[scalar(name = "decimal_sub")]
fn decimal_sub_sql(args: &[Value]) -> Value {
    binary(args, "decimal_sub", decimal_sub)
}

#[scalar(name = "decimal_mul")]
fn decimal_mul_sql(args: &[Value]) -> Value {
    binary(args, "decimal_mul", decimal_mul)
}

#[scalar(name = "decimal_cmp")]
fn decimal_cmp_sql(args: &[Value]) -> Value {
    let [a, b] = args else {
        return arity_error("decimal_cmp");
    };
    let (Some(a), Some(b)) = (decimal_new(a, true), decimal_new(b, true)) else {
        return out_of_memory();
    };
    if a.is_null || b.is_null {
        return Value::null();
    }
    Value::from_integer(match decimal_cmp(&a, &b) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    })
}

/// decimal.c's decimalPow2Func acts only on an INTEGER argument; anything else is NULL.
#[scalar(name = "decimal_pow2")]
fn decimal_pow2_sql(args: &[Value]) -> Value {
    let [n] = args else {
        return arity_error("decimal_pow2");
    };
    if n.value_type() != ValueType::Integer {
        return Value::null();
    }
    let n = n.to_integer().unwrap_or_default();
    if !(-POW2_LIMIT..=POW2_LIMIT).contains(&n) {
        return Value::error_with_message("decimal_pow2(N): N must be an integer between -20000 and +20000".into());
    }
    text_result(decimal_pow2(n), true)
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
        match decimal_new(arg, true).and_then(|x| decimal_add(acc, &x)) {
            Some(sum) => *acc = sum,
            None => state.out_of_memory = true,
        }
    }

    fn finalize(state: Self::State) -> Result<Value, Self::Error> {
        if state.out_of_memory {
            return Err("out of memory");
        }
        Ok(match state.acc.as_ref().and_then(Decimal::to_text) {
            Some(s) => Value::from_text(s),
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

    fn text(s: &str) -> Decimal {
        decimal_from_text(s.as_bytes()).unwrap()
    }

    fn plain(d: Option<Decimal>) -> Option<String> {
        d.and_then(|d| d.to_text())
    }

    #[test]
    fn decimal_of_a_real_is_its_exact_expansion() {
        assert_eq!(plain(decimal_from_double(47.49)).as_deref(), Some("47.49000000000000198951966012828052043914794921875"));
        assert_eq!(plain(decimal_from_double(0.1)).as_deref(), Some("0.1000000000000000055511151231257827021181583404541015625"));
        assert_eq!(plain(decimal_from_double(12.0)).as_deref(), Some("12"));
        assert_eq!(plain(decimal_from_double(f64::NAN)), None);
    }

    #[test]
    fn significant_digits_drop_leading_and_trailing_zeros_and_place_the_point() {
        let sig = |r: f64| decimal_from_double(r).and_then(|d| d.significant_digits().map(|(s, p)| (s.to_vec(), p)));
        assert_eq!(sig(47.49).map(|(s, p)| (s.len(), s.get(..4).map(<[u8]>::to_vec), p)), Some((49, Some(vec![4, 7, 4, 9]), 2)));
        assert_eq!(sig(0.001).map(|(s, p)| (s.first().copied(), p)), Some((Some(1), -2)));
        assert_eq!(sig(-0.25), Some((vec![2, 5], 0)));
        assert_eq!(sig(1200.0), Some((vec![1, 2], 4)));
        assert_eq!(sig(0.0625), Some((vec![6, 2, 5], -1)));
        assert_eq!(sig(0.0), None);
        assert_eq!(sig(f64::INFINITY), None);
    }

    #[test]
    fn decimal_exp_writes_exponential_notation() {
        assert_eq!(
            decimal_from_double(47.49).and_then(|d| d.to_sci_text()).as_deref(),
            Some("+4.749000000000000198951966012828052043914794921875e+01")
        );
        assert_eq!(text("12").to_sci_text().as_deref(), Some("+1.2e+01"));
        assert_eq!(text("0012.500").to_sci_text().as_deref(), Some("+1.25e+01"));
        assert_eq!(text("0.05").to_sci_text().as_deref(), Some("+5.0e-02"));
    }

    #[test]
    fn decimal_of_text() {
        assert_eq!(text("0012.500").to_text().as_deref(), Some("12.500"));
        assert_eq!(text("-0.0").to_text().as_deref(), Some("0.0"));
        assert_eq!(text(" 1e3 ").to_text().as_deref(), Some("1000"));
        assert_eq!(text("abc").to_text().as_deref(), Some("0"));
        assert_eq!(text("12").to_text().as_deref(), Some("12"));
        assert_eq!(text("1.5e-3").to_text().as_deref(), Some("0.0015"));
    }

    #[test]
    fn decimal_pow2_is_exact_within_its_bound() {
        assert_eq!(plain(decimal_pow2(-1)).as_deref(), Some("0.5"));
        assert!(decimal_pow2(POW2_LIMIT).is_some());
        assert!(decimal_pow2(-POW2_LIMIT).is_some());
        assert!(decimal_pow2(POW2_LIMIT + 1).is_none());
        assert!(decimal_pow2(-POW2_LIMIT - 1).is_none());
    }

    #[test]
    fn decimal_add_sub_mul() {
        assert_eq!(plain(decimal_add(&text("0.1"), &text("0.2"))).as_deref(), Some("0.3"));
        assert_eq!(plain(decimal_add(&text("1.10"), &text("2.205"))).as_deref(), Some("3.305"));
        assert_eq!(plain(decimal_sub(&text("1"), &text("2.5"))).as_deref(), Some("-1.5"));
        assert_eq!(plain(decimal_mul(&text("0.10"), &text("0.20"))).as_deref(), Some("0.02"));
        assert_eq!(plain(decimal_mul(&text("1.23"), &text("2.34"))).as_deref(), Some("2.8782"));
        assert_eq!(plain(decimal_mul(&text("-3"), &text("4"))).as_deref(), Some("-12"));
    }

    #[test]
    fn decimal_cmp_orders_numerically() {
        assert_eq!(decimal_cmp(&text("10"), &text("10.0")), Ordering::Equal);
        assert_eq!(decimal_cmp(&text("1.5"), &text("10")), Ordering::Less);
        assert_eq!(decimal_cmp(&text("-1"), &text("1")), Ordering::Less);
        assert_eq!(decimal_cmp(&text("2"), &text("1")), Ordering::Greater);
        assert_eq!(decimal_cmp(&text("-2"), &text("-10")), Ordering::Greater);
    }

    #[test]
    fn decimal_collation_sorts_in_numeric_order() {
        let mut v = ["10", "9", "1.5", "-2", "9.0"];
        v.sort_by(|a, b| decimal_collate(a, b));
        assert_eq!(v, ["-2", "1.5", "9", "9.0", "10"]);
    }

    #[test]
    fn decimal_sum_is_exact() {
        let mut state = DecimalSumState::default();
        for x in ["0.1", "0.2", "0.3"] {
            DecimalSum::step(&mut state, &[Value::from_text(x.to_string())]);
        }
        let sum = DecimalSum::finalize(state).unwrap();
        assert_eq!(sum.to_text(), Some("0.6"));

        assert_eq!(DecimalSum::finalize(DecimalSumState::default()).unwrap().value_type(), ValueType::Null);

        let mut state = DecimalSumState::default();
        DecimalSum::step(&mut state, &[Value::null()]);
        assert_eq!(DecimalSum::finalize(state).unwrap().to_text(), Some("0"));
    }
}
