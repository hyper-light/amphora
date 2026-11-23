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
