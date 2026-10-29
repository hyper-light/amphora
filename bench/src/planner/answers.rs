//! The planner bench's answers (`--answers <dir>`, bench/planner/README.md): each engine's whole
//! result of each query as it returned it, written once a pass from the query's first timed run,
//! for bench/planner/answers.py to compare between engines and against the parent build (kiln plan
//! 01a188991617756c84d73f0e9e100330, step 1 (f)).
//!
//! `<dir>/<benchmark>/<query>/<engine>.tsv` holds a line a row, in the engine's order, each ended by
//! a newline, its values between tabs in PostgreSQL's COPY text format: NULL is `\N`, and a
//! backslash, tab, newline or carriage return within a value is `\\`, `\t`, `\n` or `\r`.
//! `query.sql` beside them is the query, whose ORDER BY says how answers.py compares them.
//!
//! An answer is bounded by `MAX_ROWS` and `MAX_BYTES`. One past either, a value the writer has no
//! text for, or a run that didn't finish leaves a refusal in the file's place: one line, `\!` (which
//! no row begins with), its kind and why. Every file is written beside its place and renamed into
//! it, so a pass stopped part way leaves the old file or the new one, never part of one.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use super::civil;

/// The most rows an answer holds; past it the answer is refused. TPC-H's largest answers are q16's
/// 27,840 rows (every brand, type and size it keeps, reached at SF 10) and q11's, which grows by
/// about 870 rows a unit of scale factor (1,048 at SF 1, 8,685 at SF 10); every JOB query is MIN
/// aggregates, one row (bench/records/2026-10-26-jupiter-planner-tpch-sf1-a.jsonl, -tpch-sf10-a
/// and -job-a). 2^20 holds q11 past SF 1,000.
pub const MAX_ROWS: u64 = 1 << 20;

/// The most bytes an answer's file holds, and so what one answer keeps in memory while its run goes
/// on; past it the answer is refused. q16 at SF 10 is about 1 MB of text (35 bytes a row) and JOB's
/// longest row, 7c's MIN over biographies, 2.3 KB (the same records); 32 MiB holds q11's rows of
/// about 27 bytes to the row cap.
pub const MAX_BYTES: usize = 32 << 20;

/// A BLOB's refusal: TPC-H and JOB give none, so the writer has no text for one.
const BLOB: &str = "a BLOB value, which the answer writer has no text for";

/// Why an answer's file holds no rows.
#[derive(Debug)]
pub enum Refusal {
    /// More rows than the cap (`MAX_ROWS`).
    Rows(u64),
    /// More bytes than the cap (`MAX_BYTES`).
    Bytes(usize),
    /// A value the writer has no text for, or couldn't read or format: what, and why.
    Value(String),
    /// The run passed its deadline.
    TimedOut,
    /// The engine refused the query: its words.
    Failed(String),
}

impl Refusal {
    /// Its line: `\!`, its kind, a space and why, escaped as a value is so it stays one line.
    fn line(&self) -> Vec<u8> {
        let (kind, why) = match self {
            Refusal::Rows(cap) => ("too_many_rows", format!("more than {cap} rows, the answer writer's cap")),
            Refusal::Bytes(cap) => ("too_many_bytes", format!("more than {cap} bytes, the answer writer's cap")),
            Refusal::Value(why) => ("unwritten_value", why.clone()),
            Refusal::TimedOut => ("timed_out", "the run passed its deadline".to_owned()),
            Refusal::Failed(words) => ("failed", words.clone()),
        };
        let mut line = format!("\\!{kind} ").into_bytes();
        escape_into(&mut line, why.as_bytes());
        line.push(b'\n');
        line
    }
}

/// A byte's escape within a value, if it takes one (COPY's text format).
fn escape(byte: u8) -> Option<&'static [u8]> {
    match byte {
        b'\\' => Some(b"\\\\"),
        b'\t' => Some(b"\\t"),
        b'\n' => Some(b"\\n"),
        b'\r' => Some(b"\\r"),
        _ => None,
    }
}

/// `value`'s length once escaped; None past `usize`.
fn escaped_len(value: &[u8]) -> Option<usize> {
    value.iter().try_fold(0usize, |n, &b| n.checked_add(escape(b).map_or(1, <[u8]>::len)))
}

fn escape_into(out: &mut Vec<u8>, value: &[u8]) {
    for &b in value {
        match escape(b) {
            Some(e) => out.extend_from_slice(e),
            None => out.push(b),
        }
    }
}

/// One answer as it is written: its rows' text so far, within its bounds.
pub struct Answer {
    text: Vec<u8>,
    rows: u64,
    /// Whether the row being written has a value: each value but a row's first follows a tab.
    in_row: bool,
    /// A formatted value's text before it goes in, kept from one value to the next.
    scratch: String,
    refused: Option<Refusal>,
    max_rows: u64,
    max_bytes: usize,
}

impl Answer {
    pub fn new(max_rows: u64, max_bytes: usize) -> Self {
        Self { text: Vec::new(), rows: 0, in_row: false, scratch: String::new(), refused: None, max_rows, max_bytes }
    }

    /// Stops the answer for `why` and lets its rows go: the file will hold the refusal.
    fn refuse(&mut self, why: Refusal) {
        if self.refused.is_none() {
            self.refused = Some(why);
            self.text = Vec::new();
        }
    }

    /// Whether `need` more bytes fit under the cap; the answer is refused if not.
    fn room(&mut self, need: usize) -> bool {
        let fits = self.text.len().checked_add(need).is_some_and(|total| total <= self.max_bytes);
        if !fits {
            self.refuse(Refusal::Bytes(self.max_bytes));
        }
        fits
    }

    /// One value, after the tab that parts it from its row's value before: its bytes escaped, or
    /// as they are for the NULL marker.
    fn put(&mut self, value: &[u8], escaped: bool) {
        if self.refused.is_some() {
            return;
        }
        let len = if escaped { escaped_len(value) } else { Some(value.len()) };
        // A length past `usize` can't fit: it asks for every byte there is.
        if !self.room(len.and_then(|n| n.checked_add(usize::from(self.in_row))).unwrap_or(usize::MAX)) {
            return;
        }
        if self.in_row {
            self.text.push(b'\t');
        }
        if escaped {
            escape_into(&mut self.text, value);
        } else {
            self.text.extend_from_slice(value);
        }
        self.in_row = true;
    }

    fn null(&mut self) {
        self.put(b"\\N", false);
    }

    fn text(&mut self, value: &[u8]) {
        self.put(value, true);
    }

    /// A value as its `Display` text: an integer, a DECIMAL's digits, a date.
    fn formatted(&mut self, value: impl std::fmt::Display) {
        if self.refused.is_some() {
            return;
        }
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.clear();
        match write!(scratch, "{value}") {
            Ok(()) => self.put(scratch.as_bytes(), true),
            Err(_) => self.refuse(Refusal::Value("a value whose text could not be formatted".to_owned())),
        }
        self.scratch = scratch;
    }

    /// A REAL: Rust's shortest text that reads back as the same double (`{:?}`), the writer's own
    /// for every engine (theirs differ in form: `1.0e+16` where this is `1e16`), and always with a
    /// `.` or an exponent (`323.0`), so it never reads as an integer.
    fn real(&mut self, value: f64) {
        self.formatted(format_args!("{value:?}"));
    }

    /// Ends a row: its newline, and the row counted against the cap.
    fn end_row(&mut self) {
        if self.refused.is_some() {
            return;
        }
        match self.rows.checked_add(1) {
            Some(rows) if rows <= self.max_rows => self.rows = rows,
            _ => return self.refuse(Refusal::Rows(self.max_rows)),
        }
        if self.room(1) {
            self.text.push(b'\n');
            self.in_row = false;
        }
    }

    /// One of Amphora's rows.
    pub fn amphora(&mut self, row: &amphora_engine::Row) {
        use amphora_engine::{Numeric, Value};
        for value in row.get_values() {
            match value {
                Value::Null => self.null(),
                Value::Numeric(Numeric::Integer(n)) => self.formatted(n),
                Value::Numeric(Numeric::Float(f)) => self.real(f64::from(*f)),
                Value::Text(t) => self.text(t.as_str().as_bytes()),
                Value::Blob(_) => self.refuse(Refusal::Value(BLOB.to_owned())),
            }
        }
        self.end_row();
    }

    /// One of SQLite's rows.
    pub fn sqlite(&mut self, row: &rusqlite::Row<'_>) {
        use rusqlite::types::ValueRef;
        for i in 0..row.as_ref().column_count() {
            match row.get_ref(i) {
                Ok(ValueRef::Null) => self.null(),
                Ok(ValueRef::Integer(n)) => self.formatted(n),
                Ok(ValueRef::Real(f)) => self.real(f),
                Ok(ValueRef::Text(t)) => self.text(t),
                Ok(ValueRef::Blob(_)) => self.refuse(Refusal::Value(BLOB.to_owned())),
                Err(e) => self.refuse(Refusal::Value(format!("a value SQLite's client could not read: {e}"))),
            }
        }
        self.end_row();
    }

    /// One of DuckDB's rows: the integer types TPC-H and JOB give (INTEGER, BIGINT, and HUGEINT
    /// for a sum), DOUBLE as the others' REALs, DECIMAL as its exact digits, DATE as `YYYY-MM-DD`
    /// (TPC-H's dates as Amphora and SQLite keep them). Any other type is refused by its name.
    pub fn duckdb(&mut self, row: &duckdb::Row<'_>) {
        use duckdb::types::ValueRef;
        for i in 0..row.as_ref().column_count() {
            match row.get_ref(i) {
                Ok(ValueRef::Null) => self.null(),
                Ok(ValueRef::Int(n)) => self.formatted(n),
                Ok(ValueRef::BigInt(n)) => self.formatted(n),
                Ok(ValueRef::HugeInt(n)) => self.formatted(n),
                Ok(ValueRef::Double(f)) => self.real(f),
                Ok(ValueRef::Decimal(d)) => self.formatted(d),
                Ok(ValueRef::Text(t)) => self.text(t),
                Ok(ValueRef::Date32(days)) => match civil(i64::from(days)) {
                    Some((year, month, day)) => self.formatted(format_args!("{year:04}-{month:02}-{day:02}")),
                    None => self.refuse(Refusal::Value(format!("a DATE {days} days from 1970-01-01, past the calendar's arithmetic"))),
                },
                Ok(other) => self.refuse(Refusal::Value(format!("a {} value, which the answer writer has no text for", other.data_type()))),
                Err(e) => self.refuse(Refusal::Value(format!("a value DuckDB's client could not read: {e}"))),
            }
        }
        self.end_row();
    }

    /// One of PostgreSQL's rows: each value the text its simple protocol sends.
    pub fn postgres(&mut self, row: &postgres::SimpleQueryRow) {
        for i in 0..row.len() {
            match row.try_get(i) {
                Ok(Some(text)) => self.text(text.as_bytes()),
                Ok(None) => self.null(),
                Err(e) => self.refuse(Refusal::Value(format!("a value PostgreSQL's client could not read: {e}"))),
            }
        }
        self.end_row();
    }

    /// The file's bytes: the rows, or the refusal in their place.
    pub fn finish(self) -> Vec<u8> {
        match self.refused {
            Some(why) => why.line(),
            None => self.text,
        }
    }

    pub fn write(self, path: &Path) -> std::io::Result<()> {
        write_file(path, &self.finish())
    }
}

/// The directory `query`'s answers go in under `root`, made if new, its text in `query.sql`.
pub fn query_dir(root: &Path, query: &str, sql: &str) -> std::io::Result<PathBuf> {
    let dir = root.join(query);
    std::fs::create_dir_all(&dir)?;
    write_file(&dir.join("query.sql"), sql.as_bytes())?;
    Ok(dir)
}

/// Writes `why` in an answer's place: the run that would have given it didn't finish.
pub fn write_refusal(path: &Path, why: &Refusal) -> std::io::Result<()> {
    write_file(path, &why.line())
}

/// Writes `bytes` beside `path`, then renames them into it: a pass stopped part way leaves the old
/// file or the new one, never part of one.
fn write_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let part = path.with_extension("part");
    std::fs::write(&part, bytes)?;
    std::fs::rename(&part, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Do: write two rows of every kind of value the engines give (kiln plan
    /// 01a188991617756c84d73f0e9e100330, step 1 (f)). Expect: COPY's text format, NULL apart from
    /// the empty text and from the text `\N`, each REAL the shortest text of its double.
    #[test]
    fn values_are_written_as_copy_text() {
        let mut a = Answer::new(MAX_ROWS, MAX_BYTES);
        a.null();
        a.text(b"");
        a.text(b"a\tb\nc\\d\re");
        a.text(b"\\N");
        a.formatted(-42i64);
        a.real(0.1 + 0.2);
        a.real(323.0);
        a.real(1e16);
        a.end_row();
        a.formatted(7);
        a.end_row();
        assert_eq!(a.finish(), b"\\N\t\ta\\tb\\nc\\\\d\\re\t\\\\N\t-42\t0.30000000000000004\t323.0\t1e16\n7\n");
    }

    /// Do: write an answer of no rows. Expect: an empty file, apart from one row of empty text.
    #[test]
    fn no_rows_is_an_empty_file() {
        assert_eq!(Answer::new(MAX_ROWS, MAX_BYTES).finish(), b"");
        let mut a = Answer::new(MAX_ROWS, MAX_BYTES);
        a.text(b"");
        a.end_row();
        assert_eq!(a.finish(), b"\n");
    }

    /// Do: give an answer as many rows as its cap, then one more. Expect: the rows at the cap
    /// written, and past it the refusal in their place, never the first rows alone.
    #[test]
    fn rows_past_the_cap_are_refused() {
        let rows = |n: i64| {
            let mut a = Answer::new(2, MAX_BYTES);
            for i in 0..n {
                a.formatted(i);
                a.end_row();
            }
            a.finish()
        };
        assert_eq!(rows(2), b"0\n1\n");
        assert_eq!(rows(3), b"\\!too_many_rows more than 2 rows, the answer writer's cap\n");
    }

    /// Do: give an answer a row that fills its byte cap exactly, then one of as many bytes that
    /// passes it once its tab is escaped. Expect: the first written, the second refused.
    #[test]
    fn bytes_past_the_cap_are_refused() {
        let one = |value: &[u8]| {
            let mut a = Answer::new(MAX_ROWS, 8);
            a.text(value);
            a.end_row();
            a.finish()
        };
        assert_eq!(one(b"1234567"), b"1234567\n");
        assert_eq!(one(b"123456\t"), b"\\!too_many_bytes more than 8 bytes, the answer writer's cap\n");
    }

    /// Do: refuse an answer with an engine's words over two lines. Expect: one line, escaped.
    #[test]
    fn a_refusal_is_one_line() {
        assert_eq!(Refusal::Failed("no such\ttable:\nx".to_owned()).line(), b"\\!failed no such\\ttable:\\nx\n");
        let mut a = Answer::new(MAX_ROWS, MAX_BYTES);
        a.text(b"kept?");
        a.refuse(Refusal::Value(BLOB.to_owned()));
        a.text(b"after");
        a.end_row();
        assert_eq!(a.finish(), b"\\!unwritten_value a BLOB value, which the answer writer has no text for\n");
    }
}
