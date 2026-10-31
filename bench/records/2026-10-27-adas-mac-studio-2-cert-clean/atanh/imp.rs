// The two atanh computations of crates/engine/vdbe/value.rs's cmath module, as they are there.

pub fn atanh_apple(x: f64) -> f64 {
    const TWO_POW_M27: f64 = 7.450580596923828e-9;
    let a = x.abs();
    if a < TWO_POW_M27 {
        return x;
    }
    (0.5 * ((a + a) / (1.0 - a)).ln_1p()).copysign(x)
}

/// Arithmetic on pairs (hi, lo) of doubles whose sum holds about 106 bits (Dekker 1971; Knuth,
/// TAOCP vol. 2, 4.2.2), enough that rounding the sum once gives the correctly rounded result.
mod double_double {
    type Dd = (f64, f64);

    // Below 2^-28, x³/3 is under half an ulp of x: atanh(x) rounds to x.
    const TWO_POW_M28: f64 = 3.725290298461914e-9;
    // 3 - 2√2: where (1 + a) / (1 - a) reaches √2, so atanh(a) is its own series.
    const Z_MAX: f64 = 0.1715728752538099;
    // ln(2) / 2 as the double nearest it and the double nearest the rest.
    const LN2_HALF: Dd = (0.34657359027997264, 1.1595234069231498e-17);
    // For |z| ≤ 3 - 2√2 the first term left out, z^47 / 47, is under 2^-116 of z.
    const SERIES_TERMS: usize = 22;
    // (1 + a) / (1 - a) is under 2^54 for a double a below 1: 54 halvings bring it under √2.
    const HALVINGS: usize = 64;

    /// atanh(x) = ln((1 + a) / (1 - a)) / 2 given x's sign, a = |x|. With (1 + a) / (1 - a) =
    /// 2^e m, m within [√2 / 2, √2], that is e ln(2) / 2 + atanh(z) for z = (m - 1) / (m + 1),
    /// and atanh(z) = z + z³/3 + z⁵/5 + ... converges fast for |z| ≤ 3 - 2√2.
    pub fn atanh_correctly_rounded(x: f64) -> f64 {
        let a = x.abs();
        if a >= 1.0 || a.is_nan() {
            return if a == 1.0 {
                f64::INFINITY.copysign(x)
            } else {
                f64::NAN
            };
        }
        if a < TWO_POW_M28 {
            return x;
        }
        let (z, e) = if a <= Z_MAX {
            ((a, 0.0), 0.0)
        } else {
            let mut m = div(two_sum(1.0, a), two_sum(1.0, -a));
            let mut e = 0.0;
            for _ in 0..HALVINGS {
                if m.0 <= std::f64::consts::SQRT_2 {
                    break;
                }
                m = (m.0 * 0.5, m.1 * 0.5);
                e += 1.0;
            }
            (div(add(m, (-1.0, 0.0)), add(m, (1.0, 0.0))), e)
        };
        let z2 = mul(z, z);
        let mut power = z;
        let mut series = z;
        let mut odd = 1.0;
        for _ in 0..SERIES_TERMS {
            power = mul(power, z2);
            odd += 2.0;
            series = add(series, div_by(power, odd));
        }
        let (p, q) = two_prod(e, LN2_HALF.0);
        add(add((p, q), (e * LN2_HALF.1, 0.0)), series)
            .0
            .copysign(x)
    }

    /// a + b exactly: the rounded sum and its rounding error (Knuth's TwoSum).
    pub(super) fn two_sum(a: f64, b: f64) -> Dd {
        let s = a + b;
        let v = s - a;
        (s, (a - (s - v)) + (b - v))
    }

    /// a + b exactly where |a| ≥ |b| (Dekker's FastTwoSum).
    pub(super) fn fast_two_sum(a: f64, b: f64) -> Dd {
        let s = a + b;
        (s, b - (s - a))
    }

    /// a × b exactly: the rounded product and its rounding error, from one fused multiply-add.
    pub(super) fn two_prod(a: f64, b: f64) -> Dd {
        let p = a * b;
        (p, a.mul_add(b, -p))
    }

    pub(super) fn add(a: Dd, b: Dd) -> Dd {
        let (s, e) = two_sum(a.0, b.0);
        let (t, f) = two_sum(a.1, b.1);
        let (s, e) = fast_two_sum(s, e + t);
        fast_two_sum(s, e + f)
    }

    pub(super) fn mul(a: Dd, b: Dd) -> Dd {
        let (p, e) = two_prod(a.0, b.0);
        fast_two_sum(p, e + (a.0 * b.1 + a.1 * b.0))
    }

    /// a / b: the quotient of the high parts, corrected by the remainder's.
    pub(super) fn div(a: Dd, b: Dd) -> Dd {
        let q = a.0 / b.0;
        let (p, e) = two_prod(b.0, q);
        let r = add(a, (-p, -(e + b.1 * q)));
        fast_two_sum(q, r.0 / b.0)
    }

    pub(super) fn div_by(a: Dd, c: f64) -> Dd {
        let q = a.0 / c;
        let (p, e) = two_prod(q, c);
        fast_two_sum(q, (((a.0 - p) - e) + a.1) / c)
    }
}

pub use double_double::atanh_correctly_rounded as atanh_cr;
