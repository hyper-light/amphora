//! The digits of a REAL's text, which Amphora and SQLite 3.54.0 write apart by design: floatingpoint.html
//! 1.3.1 writes 15 significant digits when they convert back to the same double, else 17, and so does
//! Amphora (crates/engine/numeric/fp.rs; bench/records/2026-11-23-adacachy2-perf-numeric-g10.md), where
//! SQLite 3.54.0 writes 17 for some doubles whose 15 convert back (716936.91809735901 for
//! 716936.918097359). Both sides' texts are put in the page's digits before they are compared, so that
//! one difference is no finding. Any other is: a 17-digit text of another double, 16 digits, other
//! notation, or 15 digits that don't convert back.

use super::engines::{Rows, V, folded, same_rows_with};

/// The end of the number that starts at `i`: digits, and where a point and a digit follow, those and
/// an exponent.
fn number_end(b: &[u8], i: usize) -> usize {
    let digits = |mut k: usize| {
        while b.get(k).is_some_and(u8::is_ascii_digit) {
            k += 1;
        }
        k
    };
    let int = digits(i);
    if b.get(int) != Some(&b'.') || !b.get(int + 1).is_some_and(u8::is_ascii_digit) {
        return int;
    }
    let frac = digits(int + 1);
    if !matches!(b.get(frac), Some(b'e' | b'E')) {
        return frac;
    }
    let sign = frac + 1 + usize::from(matches!(b.get(frac + 1), Some(b'+' | b'-')));
    let exponent = digits(sign);
    if exponent > sign { exponent } else { frac }
}

/// Where each number of `text` starts and ends, the ones with a point.
fn reals(text: &str) -> Vec<(usize, usize)> {
    let b = text.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let starts =
            b[i].is_ascii_digit() && (i == 0 || !(b[i - 1].is_ascii_digit() || b[i - 1] == b'.'));
        if !starts {
            i += 1;
            continue;
        }
        let end = number_end(b, i);
        if text[i..end].contains('.') {
            found.push((i, end));
        }
        i = end.max(i + 1);
    }
    found
}

/// The page's text of the double `t` names, where `t` is that double's 17 digits and its 15 convert
/// back to it: the one difference by design. `None` for any other text, left as it is.
fn page_text(t: &str) -> Option<String> {
    let (mantissa, exponent) = match t.find(['e', 'E']) {
        Some(k) => t.split_at(k),
        None => (t, ""),
    };
    let (int, frac) = mantissa.split_once('.')?;
    // a REAL's text ends in no zero but the one after its point
    if frac.len() > 1 && frac.ends_with('0') {
        return None;
    }
    let digits = format!("{int}{frac}");
    let first = digits.find(|c| c != '0')?;
    let written = digits[first..].trim_end_matches('0');
    if !(16..=17).contains(&written.len()) {
        return None;
    }
    let value: f64 = t.parse().ok()?;
    let long = format!("{value:.16e}");
    let (long, long_point) = long.split_once('e')?;
    let exact: u64 = long.replace('.', "").parse().ok()?;
    let off = format!("{written:0<17}")
        .parse::<u64>()
        .ok()?
        .abs_diff(exact);
    // SQLite rounds at the 18th digit and then at the 17th, so its last digit may be one off
    if off > u64::from(written.len() == 17) {
        return None;
    }
    let short = format!("{value:.14e}");
    if short.parse::<f64>() != Ok(value) {
        return None;
    }
    let (short, point) = short.split_once('e')?;
    if point != long_point {
        return None;
    }
    let short = short.replace('.', "");
    let short = short.trim_end_matches('0');
    let text = if first < int.len() {
        let places = int.len();
        let padded = format!("{short:0<places$}");
        let (whole, rest) = padded.split_at(places);
        format!("{whole}.{}", if rest.is_empty() { "0" } else { rest })
    } else {
        format!("{int}.{}{short}", "0".repeat(first - int.len()))
    };
    Some(format!("{text}{exponent}"))
}

/// `text` with each REAL in it written as the page writes that double.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for (start, end) in reals(text) {
        if let Some(page) = page_text(&text[start..end]) {
            out.push_str(&text[copied..start]);
            out.push_str(&page);
            copied = end;
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// An answer with every REAL's text in it as the page writes it.
fn rows(answer: &Rows) -> Rows {
    let value = |v: &V| match v {
        V::Text(s) => V::Text(normalize(s)),
        V::Blob(b) => V::Blob(match std::str::from_utf8(b) {
            Ok(s) => normalize(s).into_bytes(),
            Err(_) => b.clone(),
        }),
        other => other.clone(),
    };
    answer
        .iter()
        .map(|row| row.iter().map(value).collect())
        .collect()
}

/// Whether Amphora's text is SQLite's but for the digits of the REALs in it, where one may end against
/// digits that aren't its own (`exp(2) || 0`): at each place they part, a number of SQLite's that
/// starts before it and is some double's 17 digits, whose 15 the other has there.
fn same_text(sqlite: &str, amphora: &str) -> bool {
    let (w, g) = (sqlite.as_bytes(), amphora.as_bytes());
    let (mut i, mut j) = (0, 0);
    loop {
        while i < w.len() && w.get(i) == g.get(j) {
            i += 1;
            j += 1;
        }
        if i == w.len() && j == g.len() {
            return true;
        }
        // the number may start at any digit the same run of digits and points holds before here
        let mut from = i;
        while from > 0
            && j + from > i
            && w.get(from - 1)
                .is_some_and(|b| b.is_ascii_digit() || *b == b'.')
        {
            from -= 1;
        }
        let found = (from..=i).find_map(|start| {
            (i + 1..=w.len().min(start + 25)).find_map(|end| {
                let page = page_text(sqlite.get(start..end)?)?;
                let there = j.checked_sub(i - start)?;
                amphora
                    .get(there..)?
                    .starts_with(&page)
                    .then(|| (end, there + page.len()))
            })
        });
        match found {
            Some((a, b)) => (i, j) = (a, b),
            None => return false,
        }
    }
}

fn same(amphora: &V, sqlite: &V) -> bool {
    amphora.same(sqlite) || matches!((sqlite, amphora), (V::Text(w), V::Text(g)) if same_text(w, g))
}

/// Whether Amphora's answer is SQLite's but for the digits of REALs' texts.
pub fn same_rows(amphora: &Rows, sqlite: &Rows, loose: bool) -> bool {
    same_rows_with(&rows(amphora), &rows(sqlite), loose, &same)
}

/// Whether Amphora's answer is SQLite's but for those digits, where some are inside a function's value
/// (a length, a text's hex, a JSONB's bytes) and can't be read: each of those values SQLite gives with
/// its texts at 15 digits instead (`fifteen`, the same rows in the same order). A text that shows a
/// REAL is compared by its digits alone.
pub fn same_inside(amphora: &Rows, sqlite: &Rows, fifteen: &Rows, loose: bool) -> bool {
    if amphora.len() != sqlite.len() || sqlite.len() != fifteen.len() {
        return false;
    }
    let alike = |g: &V, w: &V| same(g, w) || (loose && folded(g).same(&folded(w)));
    let value = |g: &V, w: &V, short: &V| {
        let shows = matches!(g, V::Text(s) if !reals(s).is_empty());
        alike(g, w) || (!shows && alike(g, short))
    };
    let mut taken = vec![false; sqlite.len()];
    amphora.iter().all(|row| {
        let place = (0..sqlite.len()).find(|&k| {
            !taken[k]
                && row.len() == sqlite[k].len()
                && row.len() == fifteen[k].len()
                && (0..row.len()).all(|c| value(&row[c], &sqlite[k][c], &fifteen[k][c]))
        });
        if let Some(k) = place {
            taken[k] = true;
        }
        place.is_some()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Rows {
        vec![vec![V::Text(s.to_owned())]]
    }

    fn alike(a: &str, b: &str) -> bool {
        same_rows(&text(b), &text(a), false)
    }

    /// Do: compare SQLite 3.54.0's 17 digits of a double with the page's 15 of the same double.
    /// Expect: the same text, in fixed and exponent notation, signed, and inside another text.
    #[test]
    fn seventeen_digits_and_the_pages_fifteen_of_one_double_are_the_same_text() {
        for (sqlite, page) in [
            ("7.3890560989306504", "7.38905609893065"),
            ("716936.91809735901", "716936.918097359"),
            ("-716936.91809735901", "-716936.918097359"),
            ("[1,7.3890560989306504,\"x\"]", "[1,7.38905609893065,\"x\"]"),
            ("1.1000000000000001e+300", "1.1e+300"),
            ("-3.3000000000000245e-310", "-3.30000000000002e-310"),
        ] {
            assert!(alike(sqlite, page), "{sqlite} {page}");
        }
        let d = 0.000123456789012345_f64;
        let digits = format!("{d:.16e}").replace('.', "");
        let long = format!("0.000{}", &digits[..17]);
        assert!(alike(&long, "0.000123456789012345"), "{long}");
    }

    /// Do: compare texts that differ otherwise: 16 digits, another double's 17, 15 digits that don't
    /// convert back, other notation.
    /// Expect: none is the same text.
    #[test]
    fn any_other_difference_of_digits_stays_one() {
        for (a, b) in [
            // 16 digits of the double whose page text has 15
            ("7.389056098930651", "7.38905609893065"),
            // the next double's 17 digits
            ("7.3890560989306513", "7.38905609893065"),
            // 15 digits of a double they don't convert back to (0.1 + 0.2 needs 17)
            ("0.30000000000000004", "0.3"),
            ("0.30000000000000004", "0.300000000000000"),
            ("1.0e+17", "100000000000000000.0"),
            ("716936.91809735901", "716936.918097360"),
            ("2.5", "2.50"),
            ("1", "1.0"),
        ] {
            assert!(!alike(a, b), "{a} {b}");
        }
    }

    /// Do: normalize texts with no REAL in them. Expect: each as it was.
    #[test]
    fn other_texts_are_left_as_they_are() {
        for s in [
            "",
            "abc",
            "é1.5é",
            "12345678901234567",
            "1.",
            ".5",
            "0x10",
            "1e3",
            "3.54.0",
            "2024-01-01 00:00:00.000",
            "0.0000000000000000",
        ] {
            assert_eq!(normalize(s), s);
        }
    }

    /// Do: compare texts whose REAL ends against other digits, or has others before it. Expect: the
    /// same text for the documented difference alone.
    #[test]
    fn a_real_that_ends_against_other_digits_is_the_same_text() {
        assert!(alike("0.881373587019543050", "0.8813735870195430"));
        assert!(alike("1.57.3890560989306504", "1.57.38905609893065"));
        assert!(alike("10.0078431372549019607", "10.00784313725490196"));
        assert!(alike(
            "7.38905609893065047.3890560989306504",
            "7.389056098930657.38905609893065"
        ));
        assert!(!alike("0.881373587019543050", "0.8813735870195431"));
        assert!(!alike("0.881373587019543050", "0.881373587019543"));
        assert!(!alike("0.300000000000000040", "0.30"));
    }

    /// Do: compare a length, and a text that shows the REAL, with SQLite's at 17 digits and at 15.
    /// Expect: the length may be either's, the text only what the page writes.
    #[test]
    fn a_length_may_be_of_fifteen_digits_and_a_shown_text_only_the_pages() {
        let one = |v: V| vec![vec![v]];
        let int = |i| one(V::Int(i));
        assert!(same_inside(&int(16), &int(18), &int(16), false));
        assert!(!same_inside(&int(17), &int(18), &int(16), false));
        let shown = |s: &str| one(V::Text(s.into()));
        // 0.1 + 0.2, whose 15 digits are another double's
        assert!(!same_inside(
            &shown("0.3"),
            &shown("0.30000000000000004"),
            &shown("0.3"),
            false
        ));
        assert!(same_inside(
            &shown("7.38905609893065"),
            &shown("7.3890560989306504"),
            &shown("7.38905609893065"),
            false
        ));
    }
}
