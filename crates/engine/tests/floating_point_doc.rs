//! The SQLite dialect's numbers follow https://www.sqlite.org/floatingpoint.html exactly (Ada,
//! 2026-11-23; lane G10). One test per rule the page states, each quoting its sentence. Where
//! SQLite 3.54.0 measures otherwise, the test holds Amphora to the page and says what 3.54 answers.
//! bench/records/2026-11-23-adacachy2-perf-numeric-g10.md lists the rules and the corpus counts.

use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, MemoryIO, OpenOptions, SqliteDialect,
    StepResult, Value, IO,
};
use std::num::NonZero;
use std::sync::Arc;

#[rustfmt::skip]
#[path = "fixtures/real_text.rs"]
mod fixture;

fn open() -> (Arc<MemoryIO>, Arc<Connection>) {
    let io = Arc::new(MemoryIO::new());
    let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
    let db = Database::open(io.clone(), "floating_point_doc.db", options).unwrap();
    (io, db.connect(ConnectOpts::new()).unwrap())
}

fn row(io: &MemoryIO, conn: &Arc<Connection>, sql: &str, bind: Option<Value>) -> Vec<Value> {
    let mut stmt = conn.prepare(sql).unwrap();
    if let Some(v) = bind {
        stmt.bind_at(NonZero::new(1).unwrap(), v).unwrap();
    }
    loop {
        match stmt.step().unwrap() {
            StepResult::Row => return stmt.row().unwrap().get_values().cloned().collect(),
            StepResult::IO => io.step().unwrap(),
            other => panic!("{sql}: {other:?}"),
        }
    }
}

fn texts(io: &MemoryIO, conn: &Arc<Connection>, sql: &str) -> Vec<String> {
    row(io, conn, sql, None)
        .iter()
        .map(ToString::to_string)
        .collect()
}

fn real(v: &Value) -> f64 {
    match v {
        Value::Numeric(amphora_engine::Numeric::Float(f)) => f64::from(*f),
        other => panic!("not a REAL: {other:?}"),
    }
}

/// "This gives a storage range of -9223372036854775808 to +9223372036854775807, inclusive.
/// Integers within this range are exact."
#[test]
fn g10_integers_are_exact_over_the_64_bit_range() {
    let (io, conn) = open();
    assert_eq!(
        texts(
            &io,
            &conn,
            "SELECT 9223372036854775807, -9223372036854775808, typeof(9223372036854775807), 9223372036854775807 - 1"
        ),
        [
            "9223372036854775807",
            "-9223372036854775808",
            "integer",
            "9223372036854775806"
        ]
    );
}

/// "A binary-64 can also be 0.0 (and -0.0), positive and negative infinity and "NaN" or
/// "Not-a-Number"."
#[test]
fn g10_reals_hold_signed_zero_and_infinities() {
    let (io, conn) = open();
    let v = row(&io, &conn, "SELECT 1e999, -1e999, -0.0", None);
    assert_eq!(real(&v[0]), f64::INFINITY);
    assert_eq!(real(&v[1]), f64::NEG_INFINITY);
    assert_eq!(real(&v[2]), 0.0);
    assert_eq!(
        texts(
            &io,
            &conn,
            "SELECT CAST(1e999 AS TEXT), CAST(-1e999 AS TEXT), CAST(-0.0 AS TEXT)"
        ),
        ["Inf", "-Inf", "0.0"]
    );
}

/// "If you provide a "price" value of 47.49, that number will be represented in binary-64 as:
/// 6683623321994527 × 2-47" and "The first number is closer to the desired value of 47.49, so that
/// is the one that gets used."
#[test]
fn g10_decimal_text_takes_the_closer_double() {
    let (io, conn) = open();
    let want = 6_683_623_321_994_527_f64 * 2_f64.powi(-47);
    let v = row(
        &io,
        &conn,
        "SELECT 47.49, CAST('47.49' AS REAL), CAST(' 47.49 ' AS REAL) + 0",
        None,
    );
    for got in &v {
        assert_eq!(real(got).to_bits(), want.to_bits(), "{v:?}");
    }
}

/// "But if you do this computation using binary-64 floating-point, the answer you get is
/// 0.00000095367431640625"
#[test]
fn g10_subtraction_of_near_equal_reals_gives_the_documented_binary64() {
    let (io, conn) = open();
    let v = row(
        &io,
        &conn,
        "SELECT 1152693165.1106291898 - 1152693165.1106280772",
        None,
    );
    assert_eq!(real(&v[0]).to_bits(), 2_f64.powi(-20).to_bits());
}

/// "It would look at the 16th digit, see that it was zero and hence just truncate, then remove
/// trailing zeros, and you are left with "47.49", which is the answer most people want."
#[test]
fn g10_47_49_displays_as_47_49() {
    let (io, conn) = open();
    assert_eq!(
        texts(&io, &conn, "SELECT CAST(47.49 AS TEXT), quote(47.49)"),
        ["47.49", "47.49"]
    );
}

/// "The floating point hardware in your CPU truncates this to: 6481130223748880 × 2-51" and "If
/// you round to 17 digits, you get 2.8781999999999997 instead." and "Converting the text
/// "2.8781999999999997" back to binary-64 does go back to original value though."
#[test]
fn g10_product_that_needs_17_digits_displays_them() {
    let (io, conn) = open();
    let v = row(&io, &conn, "SELECT 1.23 * 2.34", None);
    assert_eq!(
        real(&v[0]).to_bits(),
        (6_481_130_223_748_880_f64 * 2_f64.powi(-51)).to_bits()
    );
    assert_eq!(
        texts(
            &io,
            &conn,
            "SELECT CAST(1.23 * 2.34 AS TEXT), CAST(CAST(1.23 * 2.34 AS TEXT) AS REAL) = 1.23 * 2.34"
        ),
        ["2.8781999999999997", "1"]
    );
}

/// "Round to 15 digits." "Convert the text result back to binary-64." "If the binary-64 value from
/// the previous step is different, then: Redo step (a) with 17-digit rounding." SQLite 3.54.0
/// answers the first two with 17 digits ('716936.91809735901', '4.9406564584124654e-324'): its
/// shortcut tries fewer digits only after three zeros or two nines. The page's own ieee754 example
/// shows the smallest subnormal as 4.94065645841247e-324.
#[test]
fn g10_display_is_15_digits_when_they_convert_back_else_17() {
    let (io, conn) = open();
    let cases = [
        (716_936.918_097_359_f64, "716936.918097359"),
        (f64::from_bits(1), "4.94065645841247e-324"),
        (0.1 + 0.2, "0.30000000000000004"),
        (1.0 / 3.0, "0.33333333333333332"),
        (1e15, "1000000000000000.0"),
        (1e17, "1.0e+17"),
    ];
    for (r, want) in cases {
        let v = row(
            &io,
            &conn,
            "SELECT CAST(?1 AS TEXT)",
            Some(Value::from_f64(r)),
        );
        assert_eq!(v[0].to_string(), want, "{r:e}");
    }
}

/// "Rounding to 17 significant digits or more results in text that, when coverted back into
/// binary-64, gives the exact same value that we started with." Every double of the real_text
/// fixture reads back from its text bit for bit.
#[test]
fn g10_displayed_text_converts_back_to_the_same_double() {
    let (io, conn) = open();
    let mut wrong = Vec::new();
    for (bits, _, _) in fixture::CASES {
        let r = f64::from_bits(*bits);
        let v = row(
            &io,
            &conn,
            "SELECT CAST(CAST(?1 AS TEXT) AS REAL)",
            Some(Value::from_f64(r)),
        );
        if real(&v[0]).to_bits() != r.to_bits() {
            wrong.push(format!("{r:e}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} don't read back: {wrong:?}",
        wrong.len()
    );
}

/// Writes `bits<TAB>CAST(x AS TEXT)` for a seeded corpus to `$AMPHORA_REAL_TEXT_CORPUS`, for
/// bench/real_text_sqlite.py to count against sqlite3 and the page's algorithm. No file asked, no
/// work.
#[test]
fn g10_real_text_corpus() {
    let Ok(path) = std::env::var("AMPHORA_REAL_TEXT_CORPUS") else {
        return;
    };
    let (io, conn) = open();
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut doubles: Vec<f64> = fixture::CASES
        .iter()
        .map(|(b, _, _)| f64::from_bits(*b))
        .collect();
    for _ in 0..100_000 {
        doubles.push(f64::from_bits(next()));
        let digits = i32::try_from(next() % 16).unwrap();
        let scale = 10_f64.powi(digits);
        let whole = f64::from(u32::try_from(next() % 2_000_000).unwrap()) - 1e6;
        let frac = f64::from(u32::try_from(next() % 1_000_000).unwrap()) / 1e6;
        doubles.push(((whole + frac) * scale).round() / scale);
        let a = f64::from(u32::try_from(next() % 100_000).unwrap()) / 100.0;
        let b = f64::from(u32::try_from(next() % 100_000).unwrap()) / 100.0;
        doubles.push(a * b);
    }
    let mut out = String::new();
    for r in doubles.into_iter().filter(|r| r.is_finite()) {
        let v = row(
            &io,
            &conn,
            "SELECT CAST(?1 AS TEXT)",
            Some(Value::from_f64(r)),
        );
        out.push_str(&format!("{:#018x}\t{}\n", r.to_bits(), v[0]));
    }
    std::fs::write(path, out).unwrap();
}

/// Lane G11: SQLITE_DBCONFIG_FP_DIGITS (c3ref/c_dbconfig_defensive.html), read by the `bench`
/// feature's public `numeric`.
#[cfg(feature = "bench")]
mod fp_digits {
    use amphora_engine::numeric::fp::{real_to_text_digits, FpDigits};

    fn at(n: i32) -> FpDigits {
        FpDigits::new(n).unwrap()
    }

    /// "The default value 17, as of SQLite version 3.52.0."
    #[test]
    fn g11_fp_digits_default_is_17_and_takes_the_pages_steps() {
        assert_eq!(FpDigits::DEFAULT.get(), 17);
        assert_eq!(
            real_to_text_digits(0.1 + 0.2, FpDigits::DEFAULT),
            "0.30000000000000004"
        );
        assert_eq!(real_to_text_digits(47.49, FpDigits::DEFAULT), "47.49");
    }

    /// "The first argument is a small integer, between 3 and 23, or zero. The FP_DIGITS setting is
    /// changed to that small integer, or left unaltered if the first argument is zero or out of
    /// range." SQLite 3.54.0 refuses 3 (`nIn>3`); the page takes it.
    #[test]
    fn g11_fp_digits_takes_3_to_23_and_leaves_zero_or_out_of_range_unaltered() {
        for n in 3..=23 {
            assert_eq!(FpDigits::new(n).map(FpDigits::get), u8::try_from(n).ok());
        }
        for n in [0, 1, 2, 24, 255, 256, -1, i32::MIN, i32::MAX] {
            assert_eq!(FpDigits::new(n), None, "{n}");
        }
    }

    /// floatingpoint.html 1.3.1: "In C-code, one can choose option (A) by running code like this:
    /// sqlite3_db_config(db, SQLITE_DBCONFIG_FP_DIGITS, 15, 0);" and "If you do not want the
    /// (negligible) overhead of step (b) and (c), just set SQLITE_DBCONFIG_FP_DIGITS to 15."
    #[test]
    fn g11_fp_digits_15_rounds_to_15_always() {
        let product = 6_481_130_223_748_880.0 * 2f64.powi(-51);
        assert_eq!(
            real_to_text_digits(product, FpDigits::DEFAULT),
            "2.8781999999999997"
        );
        assert_eq!(real_to_text_digits(product, at(15)), "2.8782");
        assert_eq!(real_to_text_digits(0.1 + 0.2, at(15)), "0.3");
        assert_eq!(real_to_text_digits(1e16, at(15)), "1.0e+16");
        assert_eq!(
            real_to_text_digits(1e16, FpDigits::DEFAULT),
            "10000000000000000.0"
        );
        assert_eq!(real_to_text_digits(-0.5, at(15)), "-0.5");
    }

    /// "a small integer that determines the number of significant digits that SQLite will attempt to
    /// preserve when converting floating point numbers (IEEE 754 "doubles") into text."
    #[test]
    fn g11_fp_digits_keeps_that_many_significant_digits() {
        assert_eq!(real_to_text_digits(1.0 / 3.0, at(3)), "0.333");
        assert_eq!(real_to_text_digits(2.0 / 3.0, at(5)), "0.66667");
        assert_eq!(real_to_text_digits(123_456.0, at(3)), "1.23e+05");
        // sqlite3FpDecode carries 19 digits at most (its 64-bit product), so 20 asks more than it has
        assert_eq!(real_to_text_digits(0.1, at(20)), "0.1000000000000000056");
    }
}

/// Lane G12: SQLITE_DBCONFIG_FP_DIGITS per connection, read wherever a statement writes a REAL as
/// text. c3ref/c_dbconfig_defensive.html: "The SQLITE_DBCONFIG_FP_DIGITS setting is a small integer
/// that determines the number of significant digits that SQLite will attempt to preserve when
/// converting floating point numbers (IEEE 754 "doubles") into text."
mod fp_digits_per_connection {
    use super::{open, row, texts};
    use amphora_engine::Value;

    const SUM: &str = "(0.1 + 0.2)";

    fn at_15() -> (
        std::sync::Arc<amphora_engine::MemoryIO>,
        std::sync::Arc<amphora_engine::Connection>,
    ) {
        let (io, conn) = open();
        assert_eq!(conn.db_config_fp_digits(15), 15);
        (io, conn)
    }

    /// "The first argument is a small integer, between 3 and 23, or zero. The FP_DIGITS setting is
    /// changed to that small integer, or left unaltered if the first argument is zero or out of
    /// range." and "the value of the FP_DIGITS setting, after possibly being modified by the first
    /// arguments, is written into the integer to which the second argument points."
    #[test]
    fn g12_db_config_fp_digits_sets_3_to_23_and_reports_the_setting() {
        let (_io, conn) = open();
        assert_eq!(conn.db_config_fp_digits(0), 17, "The default value 17");
        assert_eq!(conn.db_config_fp_digits(3), 3);
        assert_eq!(conn.db_config_fp_digits(24), 3);
        assert_eq!(conn.db_config_fp_digits(2), 3);
        assert_eq!(conn.db_config_fp_digits(-1), 3);
        assert_eq!(conn.db_config_fp_digits(23), 23);
        assert_eq!(conn.db_config_fp_digits(0), 23);
        let (_io, other) = open();
        assert_eq!(
            other.db_config_fp_digits(0),
            17,
            "a setting of one connection only"
        );
    }

    /// lang_expr.html: "Casting an INTEGER or REAL value into TEXT renders the value as if via
    /// sqlite3_snprintf()"; to BLOB, "first casting the value to TEXT".
    #[test]
    fn g12_cast_to_text_and_blob_write_at_the_connections_digits() {
        let (io, conn) = open();
        let sql = format!("SELECT CAST({SUM} AS TEXT), CAST({SUM} AS BLOB)");
        assert_eq!(
            row(&io, &conn, &sql, None),
            vec![
                Value::build_text("0.30000000000000004"),
                Value::Blob(b"0.30000000000000004".to_vec())
            ]
        );
        conn.db_config_fp_digits(15);
        assert_eq!(
            row(&io, &conn, &sql, None),
            vec![Value::build_text("0.3"), Value::Blob(b"0.3".to_vec())]
        );
        conn.db_config_fp_digits(5);
        assert_eq!(
            texts(&io, &conn, "SELECT CAST(2.0/3.0 AS TEXT)"),
            vec!["0.66667"]
        );
    }

    /// `||`, concat() and concat_ws() write each REAL operand as text.
    #[test]
    fn g12_concatenation_writes_at_the_connections_digits() {
        let (io, conn) = at_15();
        assert_eq!(
            texts(
                &io,
                &conn,
                &format!("SELECT 'x' || {SUM}, concat({SUM}, 'y'), concat_ws({SUM}, 'a', {SUM})")
            ),
            vec!["x0.3", "0.3y", "a0.30.3"]
        );
    }

    /// datatype3.html: "A column with TEXT affinity stores all data using storage classes NULL, TEXT
    /// or BLOB. If numerical data is inserted into a column with TEXT affinity it is converted into
    /// text form before being stored."
    #[test]
    fn g12_text_affinity_stores_at_the_connections_digits() {
        let (io, conn) = at_15();
        conn.execute("CREATE TABLE t(c TEXT)").unwrap();
        conn.execute(format!("INSERT INTO t VALUES ({SUM})"))
            .unwrap();
        assert_eq!(
            row(&io, &conn, "SELECT c FROM t", None),
            vec![Value::build_text("0.3")]
        );
    }

    /// datatype3.html: "If one operand has TEXT affinity and the other has no affinity, then TEXT
    /// affinity is applied to the other operand." The REAL becomes the connection's text first.
    #[test]
    fn g12_text_affinity_in_a_comparison_converts_at_the_connections_digits() {
        let (io, conn) = open();
        conn.execute("CREATE TABLE t(c TEXT)").unwrap();
        conn.execute("INSERT INTO t VALUES ('0.3')").unwrap();
        let sql = format!("SELECT c = {SUM} FROM t");
        assert_eq!(texts(&io, &conn, &sql), vec!["0"]);
        conn.db_config_fp_digits(15);
        assert_eq!(texts(&io, &conn, &sql), vec!["1"]);
    }

    /// LIKE reads a REAL operand as text.
    #[test]
    fn g12_like_reads_a_real_at_the_connections_digits() {
        let (io, conn) = at_15();
        assert_eq!(
            texts(&io, &conn, &format!("SELECT {SUM} LIKE '0.3'")),
            vec!["1"]
        );
    }

    /// printf.html: "%s ... the argument is a string"; a REAL argument is read as text, as is a REAL
    /// format string. printf's own %g keeps its precision rules (printf.html), not FP_DIGITS.
    #[test]
    fn g12_printf_text_forms_write_at_the_connections_digits() {
        let (io, conn) = at_15();
        assert_eq!(
            texts(&io, &conn, &format!("SELECT printf('%s|%q|%Q', {SUM}, {SUM}, {SUM}), printf({SUM}), printf('%!.17g', {SUM})")),
            vec!["0.3|0.3|'0.3'", "0.3", "0.30000000000000004"]
        );
    }

    /// The design: `Value`'s `Display` carries no connection and writes the default, 17 ("The default
    /// value 17, as of SQLite version 3.52.0"); a statement's conversions read the connection.
    #[test]
    fn g12_value_display_writes_the_default_whatever_the_connection() {
        let (io, conn) = at_15();
        let values = row(
            &io,
            &conn,
            &format!("SELECT {SUM}, CAST({SUM} AS TEXT)"),
            None,
        );
        assert_eq!(
            values.iter().map(ToString::to_string).collect::<Vec<_>>(),
            vec!["0.30000000000000004", "0.3"]
        );
    }
}

/// Lane G13: the page's remaining sentences, each a test. The expected texts are the page's own
/// where it shows one, else sqlite3 3.54.0's (bench/records/2026-11-23-adacachy2-perf-numeric-g13.md).
mod page_sentences {
    use super::{open, texts};
    use amphora_engine::StepResult;

    fn one(sql: &str) -> String {
        let (io, conn) = open();
        texts(&io, &conn, sql).join("|")
    }

    fn fails(sql: &str) -> bool {
        let (io, conn) = open();
        let Ok(mut stmt) = conn.prepare(sql) else {
            return true;
        };
        loop {
            match stmt.step() {
                Err(_) => return true,
                Ok(StepResult::IO) => io.step().unwrap(),
                Ok(StepResult::Row) => {}
                Ok(_) => return false,
            }
        }
    }

    /// "This gives a range of positive values between approximately 1.7976931348623157e+308 and
    /// 4.9406564584124654e-324 with an equivalent range of negative values."
    #[test]
    fn g13_reals_range_from_the_largest_to_the_smallest_binary64_either_sign() {
        assert_eq!(
            one("SELECT 1.7976931348623157e+308 = ieee754_from_blob(x'7fefffffffffffff'), 4.9406564584124654e-324 = ieee754_from_blob(x'0000000000000001'), -1.7976931348623157e+308 = ieee754_from_blob(x'ffefffffffffffff'), -4.9406564584124654e-324 = ieee754_from_blob(x'8000000000000001')"),
            "1|1|1|1"
        );
    }

    /// "The binary-64 floating-point format uses 64 bits per number."
    #[test]
    fn g13_a_real_is_64_bits() {
        assert_eq!(
            one("SELECT quote(ieee754_to_blob(47.49)), length(ieee754_to_blob(47.49))"),
            "X'4047BEB851EB851F'|8"
        );
    }

    /// "Floating point values are approximate."
    #[test]
    fn g13_floating_point_values_are_approximate() {
        assert_eq!(
            one("SELECT 0.1 + 0.2 = 0.3, decimal(0.1)"),
            "0|0.1000000000000000055511151231257827021181583404541015625"
        );
    }

    /// "But if you do this computation using binary-64 floating-point, the answer you get is
    /// 0.00000095367431640625"
    #[test]
    fn g13_the_near_equal_subtraction_is_exactly_the_pages_binary64() {
        assert_eq!(
            one("SELECT decimal(1152693165.1106291898 - 1152693165.1106280772)"),
            "0.00000095367431640625"
        );
    }

    /// "An IEEE 754 floating-point value is an integer multiplied by a power of two: M × 2^E";
    /// 47.49 "will be represented in binary-64 as: 6683623321994527 × 2^-47".
    #[test]
    fn g13_a_real_is_m_times_2_to_the_e() {
        assert_eq!(
            one("SELECT ieee754(47.49), ieee754_mantissa(47.49), ieee754_exponent(47.49)"),
            "ieee754(6683623321994527,-47)|6683623321994527|-47"
        );
    }

    /// "For Binary-64, M is a 53-bit integer and E is an 11-bit integer that is offset so that
    /// represents a range of values between -1074 and +972, inclusive."
    #[test]
    fn g13_e_runs_from_minus_1074_to_972() {
        assert_eq!(
            one("SELECT ieee754_exponent(4.9406564584124654e-324), ieee754_exponent(1e999), ieee754(1.7976931348623157e+308)"),
            "-1074|972|ieee754(9007199254740991,971)"
        );
    }

    /// "Which works out to be: 47.49000000000000198951966012828052043914794921875" / "If we reduce M
    /// by one to 6683623321994526 ... we get: 47.4899999999999948840923025272786617279052734375" /
    /// "The first number is closer to the desired value of 47.49, so that is the one that gets used."
    #[test]
    fn g13_47_49_and_the_next_smaller_double() {
        assert_eq!(
            one("SELECT decimal(ieee754(6683623321994527,-47)), decimal(ieee754(6683623321994526,-47)), ieee754(6683623321994526,-47) < 47.49"),
            "47.49000000000000198951966012828052043914794921875|47.4899999999999948840923025272786617279052734375|1"
        );
    }

    /// "the only cents value that can be exactly represented are 0.00, 0.25, 0.50, and 0.75."
    #[test]
    fn g13_only_quarter_cents_are_exact() {
        assert_eq!(
            one("WITH RECURSIVE c(v) AS (SELECT 0 UNION ALL SELECT v+1 FROM c WHERE v<99) SELECT group_concat(v) FROM c WHERE decimal_cmp(decimal(v/100.0), printf('0.%02d', v)) = 0"),
            "0,25,50,75"
        );
    }

    /// "the price is going up by about 0.0000000301 dollars per second. The error in the recorded
    /// value of 47.49 represents about 66 nanoseconds worth of inflation." / "the value actually
    /// stored (47.4900000000000019895196601282805204391479492187)"
    #[test]
    fn g13_the_error_in_47_49_is_66_nanoseconds_of_inflation() {
        assert_eq!(
            one("SELECT substr(decimal(47.49),1,49), round(47.49*0.02/(365.25*86400),10), round(CAST(decimal_sub(decimal(47.49), '47.49') AS REAL) / (47.49*0.02/(365.25*86400)) * 1e9)"),
            "47.4900000000000019895196601282805204391479492187|3.01e-08|66.0"
        );
    }

    /// "One rarely wants to see all 49 signficant digits of 6683623321994527×pow(2,-47)."
    #[test]
    fn g13_47_49_has_49_significant_digits() {
        assert_eq!(one("SELECT length(decimal(47.49)) - 1"), "49");
    }

    /// "Rounding to 15 significant digits or less can often gives this exact round-trip result,
    /// but not always."
    #[test]
    fn g13_15_digits_round_trip_often_but_not_always() {
        assert_eq!(
            one("SELECT printf('%.15g', 47.49), CAST(printf('%.15g', 47.49) AS REAL) = 47.49, printf('%.15g', 1.23*2.34), CAST(printf('%.15g', 1.23*2.34) AS REAL) = 1.23*2.34"),
            "47.49|1|2.8782|0"
        );
    }

    /// "The number 1.23 is: 2769713770832855 × 2^-51 = 1.229999999999999982236431605997495353221893310546875"
    /// / "And 2.34 is: 5269211564023480 × 2^-51 = 2.339999999999999857891452847979962825775146484375"
    #[test]
    fn g13_1_23_and_2_34_as_binary64() {
        assert_eq!(
            one("SELECT ieee754(2769713770832855,-51) = 1.23, decimal(1.23), ieee754(5269211564023480,-51) = 2.34, decimal(2.34)"),
            "1|1.229999999999999982236431605997495353221893310546875|1|2.339999999999999857891452847979962825775146484375"
        );
    }

    /// "If you do an exact multiplication of those two values you get:
    /// 2.878199999999999783639736961049495926597557229698714817531408904915934954260592348873615264892578125"
    #[test]
    fn g13_the_exact_product_of_1_23_and_2_34() {
        assert_eq!(
            one("SELECT decimal_mul(decimal(1.23), decimal(2.34))"),
            "2.878199999999999783639736961049495926597557229698714817531408904915934954260592348873615264892578125"
        );
    }

    /// "The floating point hardware in your CPU truncates this to: 6481130223748880 × 2^-51 =
    /// 2.87819999999999964757080306299030780792236328125"
    #[test]
    fn g13_the_binary64_product_of_1_23_and_2_34() {
        assert_eq!(
            one("SELECT ieee754(6481130223748880,-51) = 1.23*2.34, decimal(1.23*2.34)"),
            "1|2.87819999999999964757080306299030780792236328125"
        );
    }

    /// "However, 2.8782 and 2.8781999999999997 are not the same number in binary-64." /
    /// "2.8782 → 6481130223748881 × 2^-51" / "2.8781999999999997 → 6481130223748880 × 2^-51"
    #[test]
    fn g13_2_8782_and_2_8781999999999997_differ_by_one_in_m() {
        assert_eq!(
            one("SELECT ieee754(6481130223748881,-51) = 2.8782, ieee754(6481130223748880,-51) = 2.8781999999999997, 2.8782 = 2.8781999999999997"),
            "1|1|0"
        );
    }

    /// §2.1.1: "the 2-argument version of ieee754() takes the M and E values and converts them into
    /// the corresponding F value: select ieee754(6683623321994527,-47) as x; ... 47.49"
    #[test]
    fn g13_ieee754_of_m_and_e_is_the_real() {
        assert_eq!(one("SELECT ieee754(6683623321994527,-47)"), "47.49");
    }

    /// §2.1.3: "SELECT ieee754_from_blob(x'0000000000000001') AS F; ... 4.94065645841247e-324" and
    /// "SELECT quote(ieee754_to_blob(4.94065645841247e-324)) ... X'0000000000000001'". sqlite3 3.54.0
    /// writes 4.9406564584124654e-324; the page's 15 digits convert back, so 1.3.1 writes those.
    #[test]
    fn g13_ieee754_blobs_are_the_big_endian_encoding() {
        assert_eq!(
            one("SELECT ieee754_from_blob(x'0000000000000001'), quote(ieee754_to_blob(4.94065645841247e-324))"),
            "4.94065645841247e-324|X'0000000000000001'"
        );
    }

    /// §2.2.1: "These functions compute the sum, difference, and product, respectively, of the two
    /// inputs A and B and return the result as text. The inputs can be either decimal text or numeric
    /// values. Numeric inputs are converted into decimal text before the computation is performed."
    #[test]
    fn g13_decimal_add_sub_mul_return_text() {
        assert_eq!(
            one("SELECT decimal_add('0.1','0.2'), decimal_sub('1','0.9'), decimal_mul('1.23','2.34'), decimal_add(0.1, 0.2), typeof(decimal_add(1, 2)), decimal_add('1.10','2.205'), decimal_mul('0.5','0.20')"),
            "0.3|0.1|2.8782|0.3|text|3.305|0.1"
        );
    }

    /// §2.2.1: "There is no "decimal_div(A,B)" function"
    #[test]
    fn g13_there_is_no_decimal_div() {
        assert!(fails("SELECT decimal_div(1,2)"));
    }

    /// §2.2.2: "The decimal_pow2(N) function computes the exact decimal representation of the N-th
    /// power of 2. N must be an integer between -20000 and +20000."
    #[test]
    fn g13_decimal_pow2_is_exact_between_minus_and_plus_20000() {
        assert_eq!(
            one("SELECT decimal_pow2(-1), decimal_pow2(10), length(decimal_pow2(20000)), length(decimal_pow2(-20000))"),
            "+5.0e-01|+1.024e+03|6029|13988"
        );
        assert!(fails("SELECT decimal_pow2(20001)"));
        assert!(fails("SELECT decimal_pow2(-20001)"));
    }

    /// §2.2.3: "If the input X is a floating point value, it is expanded to its exact decimal
    /// equivalent" / "The decimal_exp(X) function returns the result in exponential notation (with a
    /// "e+NN" at the end) and decimal(X) returns a pure decimal (without the "e+NN")."
    #[test]
    fn g13_decimal_and_decimal_exp_expand_exactly() {
        assert_eq!(
            one("SELECT decimal(47.49), decimal_exp(47.49), decimal(12), decimal_exp(12), decimal('0012.500')"),
            "47.49000000000000198951966012828052043914794921875|+4.749000000000000198951966012828052043914794921875e+01|12|+1.2e+01|12.500"
        );
    }

    /// §2.2.4: "The result will be negative, zero, or positive if A is less than, equal to, or
    /// greater than B, respectively."
    #[test]
    fn g13_decimal_cmp_orders_by_value() {
        assert_eq!(
            one("SELECT decimal_cmp('1.5','10'), decimal_cmp('10','10.0'), decimal_cmp('-1','1'), decimal_cmp(2, 1)"),
            "-1|0|-1|1"
        );
    }

    /// §2.2.5: "decimal_sum() computes its result to arbitrary precision and is therefore precise."
    #[test]
    fn g13_decimal_sum_is_precise() {
        assert_eq!(
            one("SELECT decimal_sum(x) FROM (SELECT '0.1' AS x UNION ALL SELECT '0.2' UNION ALL SELECT '0.3')"),
            "0.6"
        );
    }

    /// §2.2.6: "the "decimal" collating sequence that compares decimal text strings in numeric order."
    #[test]
    fn g13_the_decimal_collation_orders_numerically() {
        assert_eq!(
            one("SELECT group_concat(x, ',') FROM (SELECT x FROM (SELECT '10' AS x UNION ALL SELECT '9' UNION ALL SELECT '1.5' UNION ALL SELECT '-2') ORDER BY x COLLATE decimal)"),
            "-2,1.5,9,10"
        );
    }
}
