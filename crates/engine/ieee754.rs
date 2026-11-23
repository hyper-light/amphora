//! SQLite's ieee754 extension (ext/misc/ieee754.c, 3.54.0), which floatingpoint.html §2.1 describes
//! and SQLite's CLI includes by default: a REAL as M × 2^E and back, and as its 8-byte big-endian
//! binary-64 encoding.

use crate::ext::register_scalar_function;
use amphora_engine_ext::{scalar, ExtensionApi, Value, ValueType};

pub fn register_extension(ext_api: &mut ExtensionApi) {
    unsafe {
        register_scalar_function(ext_api.ctx, c"ieee754".as_ptr(), ieee754);
        register_scalar_function(ext_api.ctx, c"ieee754_mantissa".as_ptr(), ieee754_mantissa);
        register_scalar_function(ext_api.ctx, c"ieee754_exponent".as_ptr(), ieee754_exponent);
        register_scalar_function(ext_api.ctx, c"ieee754_to_blob".as_ptr(), ieee754_to_blob);
        register_scalar_function(ext_api.ctx, c"ieee754_from_blob".as_ptr(), ieee754_from_blob);
    }
}

/// ieee754.c's bias: a double with biased exponent e and integer mantissa m is m × 2^(e-1075).
const BIAS: i64 = 1075;
const FRACTION: u64 = (1 << 52) - 1;

/// The double an argument names: an 8-byte BLOB read as its big-endian encoding, anything else
/// as sqlite3_value_double reads it (NULL and text that isn't a number are 0.0).
fn argument_double(v: &Value) -> f64 {
    if v.value_type() == ValueType::Blob {
        if let Some(bytes) = v.blob_ref().and_then(|b| <[u8; 8]>::try_from(b).ok()) {
            return f64::from_bits(u64::from_be_bytes(bytes));
        }
    }
    v.to_float().unwrap_or(0.0)
}

/// (M, E) as ieee754.c's one-argument form computes them: M odd unless zero, E from -1074 to 972
/// (972 for the infinities and NaNs, whose M is 2^52 or more), and zero as (0, -1075).
fn split(r: f64) -> (i64, i64) {
    let negative = r < 0.0;
    let bits = if negative { (-r).to_bits() } else { r.to_bits() };
    if bits == 0 {
        return (0, -BIAS);
    }
    // -0.0 is not below 0.0, so its sign bit stays in `bits` as ieee754.c's signed shift reads it.
    let mut e = i64::from_ne_bytes(bits.to_ne_bytes()).wrapping_shr(52);
    let mut m = bits & FRACTION;
    if e == 0 {
        m = m.wrapping_shl(1);
    } else {
        m |= 1 << 52;
    }
    while e < BIAS && m > 0 && m & 1 == 0 {
        m = m.wrapping_shr(1);
        e = e.wrapping_add(1);
    }
    let m = i64::try_from(m).unwrap_or(i64::MAX);
    (if negative { m.wrapping_neg() } else { m }, e.wrapping_sub(BIAS))
}

/// M × 2^E as ieee754.c's two-argument form builds it: bits below the 53rd are dropped, a value
/// below the subnormals is zero, one past the largest double is infinity.
fn join(m: i64, e: i64) -> Option<f64> {
    // "Limit the range of e. Ticket 22dea1cfdb9151e4 2021-03-02"
    let mut e = e.clamp(-10_000, 10_000);
    let negative = m < 0;
    let mut m = if negative {
        // ieee754.c returns no result when -m overflows
        u64::try_from(m.checked_neg()?).ok()?
    } else {
        if m == 0 && e > -1000 && e < 1000 {
            return Some(0.0);
        }
        u64::try_from(m).ok()?
    };
    while m.wrapping_shr(32) & 0xffe0_0000 != 0 {
        m = m.wrapping_shr(1);
        e = e.wrapping_add(1);
    }
    while m != 0 && m.wrapping_shr(32) & 0xfff0_0000 == 0 {
        m = m.wrapping_shl(1);
        e = e.wrapping_sub(1);
    }
    e = e.wrapping_add(BIAS);
    if e <= 0 {
        let by = 1_i64.wrapping_sub(e);
        m = if by >= 64 {
            0
        } else {
            m.wrapping_shr(u32::try_from(by).ok()?)
        };
        e = 0;
    } else if e > 0x7ff {
        e = 0x7ff;
    }
    let mut a = (m & FRACTION) | u64::try_from(e).ok()?.wrapping_shl(52);
    if negative {
        a |= 1 << 63;
    }
    Some(f64::from_bits(a))
}

fn real(r: f64) -> Value {
    // sqlite3_result_double stores a NaN as NULL
    if r.is_nan() {
        Value::null()
    } else {
        Value::from_float(r)
    }
}

/// floatingpoint.html §2.1.1: ieee754(F) is the text 'ieee754(M,E)'; ieee754(M,E) is the REAL.
#[scalar(name = "ieee754")]
fn ieee754(args: &[Value]) -> Value {
    match args {
        [f] => {
            let (m, e) = split(argument_double(f));
            Value::from_text(format!("ieee754({m},{e})"))
        }
        [m, e] => join(m.to_integer().unwrap_or(0), e.to_integer().unwrap_or(0))
            .map_or_else(Value::null, real),
        _ => Value::error_with_message("wrong number of arguments to function ieee754()".into()),
    }
}

/// §2.1.2: the M of ieee754(F).
#[scalar(name = "ieee754_mantissa")]
fn ieee754_mantissa(args: &[Value]) -> Value {
    match args {
        [f] => Value::from_integer(split(argument_double(f)).0),
        _ => Value::error_with_message(
            "wrong number of arguments to function ieee754_mantissa()".into(),
        ),
    }
}

/// §2.1.2: the E of ieee754(F).
#[scalar(name = "ieee754_exponent")]
fn ieee754_exponent(args: &[Value]) -> Value {
    match args {
        [f] => Value::from_integer(split(argument_double(f)).1),
        _ => Value::error_with_message(
            "wrong number of arguments to function ieee754_exponent()".into(),
        ),
    }
}

/// §2.1.3: "converts the floating point number F into an 8-byte BLOB that is the big-endian
/// binary-64 encoding of that number"; anything but an INTEGER or a REAL gives NULL.
#[scalar(name = "ieee754_to_blob")]
fn ieee754_to_blob(args: &[Value]) -> Value {
    match args {
        [f] => match f.value_type() {
            ValueType::Integer | ValueType::Float => f.to_float().map_or_else(Value::null, |r| {
                Value::from_blob(r.to_bits().to_be_bytes().to_vec())
            }),
            _ => Value::null(),
        },
        _ => Value::error_with_message(
            "wrong number of arguments to function ieee754_to_blob()".into(),
        ),
    }
}

/// §2.1.3: "converting an 8-byte blob into the floating-point value that the binary-64 encoding
/// represents"; anything but an 8-byte BLOB gives NULL.
#[scalar(name = "ieee754_from_blob")]
fn ieee754_from_blob(args: &[Value]) -> Value {
    match args {
        [b] => match b.blob_ref().and_then(|b| <[u8; 8]>::try_from(b).ok()) {
            Some(bytes) => real(f64::from_bits(u64::from_be_bytes(bytes))),
            None => Value::null(),
        },
        _ => Value::error_with_message(
            "wrong number of arguments to function ieee754_from_blob()".into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{join, split};

    /// floatingpoint.html §2.1: "SELECT ieee754(47.49)" is ieee754(6683623321994527,-47), and the
    /// two-argument form goes back.
    #[test]
    fn ieee754_splits_and_joins_the_pages_examples() {
        assert_eq!(split(47.49), (6_683_623_321_994_527, -47));
        assert_eq!(join(6_683_623_321_994_527, -47), Some(47.49));
        assert_eq!(split(-47.49), (-6_683_623_321_994_527, -47));
        assert_eq!(split(f64::from_bits(1)), (1, -1074));
        assert_eq!(split(f64::MAX), (9_007_199_254_740_991, 971));
        assert_eq!(split(f64::INFINITY), (4_503_599_627_370_496, 972));
        assert_eq!(split(0.0), (0, -1075));
    }

    /// The edges sqlite3 3.54.0 answers: dropped low bits, below the subnormals, past the largest.
    #[test]
    fn ieee754_join_truncates_as_sqlite_does() {
        assert_eq!(join(9_007_199_254_740_993, 0), Some(9_007_199_254_740_992.0));
        assert_eq!(join(1, -1075), Some(0.0));
        assert_eq!(join(3, -1075), Some(f64::from_bits(1)));
        assert_eq!(join(0, 2000), Some(f64::INFINITY));
        assert_eq!(join(4_503_599_627_370_496, 972), Some(f64::INFINITY));
        assert_eq!(join(-1, 0), Some(-1.0));
        assert_eq!(join(i64::MIN, 0), None);
    }
}
