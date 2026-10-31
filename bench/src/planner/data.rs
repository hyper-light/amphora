//! The benchmarks' rows as their files hold them: JOB's IMDb tables as PostgreSQL's COPY wrote
//! them (CSV, `"` quoting, `\` escaping a quote or itself inside quotes, an unquoted empty field
//! NULL) and TPC-H's as dbgen wrote them (`|` between fields, nothing quoted, the closing `|`
//! taken off by bench/planner/tpch/dbgen.sh).

use std::io::BufRead;

/// How a table's file separates and quotes its fields.
#[derive(Clone, Copy, Debug)]
pub enum Format {
    /// JOB's IMDb CSV files.
    Csv,
    /// TPC-H's `.tbl` files.
    Pipe,
}

/// The longest record a file may hold: a longer one is refused, not read into memory without end.
const MAX_RECORD_BYTES: usize = 1 << 24;

/// Reads one record at a time into fields reused from the last.
pub struct Records<R> {
    input: R,
    format: Format,
    line: Vec<u8>,
    /// The record's fields; `None` is NULL.
    pub fields: Vec<Option<String>>,
    pub records: u64,
}

impl<R: BufRead> Records<R> {
    pub fn new(input: R, format: Format) -> Self {
        Self { input, format, line: Vec::new(), fields: Vec::new(), records: 0 }
    }

    /// The next record into `fields`; false at the end of the file.
    pub fn next(&mut self) -> anyhow::Result<bool> {
        self.line.clear();
        if self.input.read_until(b'\n', &mut self.line)? == 0 {
            return Ok(false);
        }
        self.fields.clear();
        match self.format {
            Format::Pipe => {
                let text = std::str::from_utf8(trim_newline(&self.line))?;
                self.fields.extend(text.split('|').map(|f| Some(f.to_owned())));
            }
            Format::Csv => self.csv()?,
        }
        self.records += 1;
        Ok(true)
    }

    /// One CSV record, reading further lines while a quoted field holds a newline.
    fn csv(&mut self) -> anyhow::Result<()> {
        let mut field: Vec<u8> = Vec::new();
        let mut quoted = false;
        let mut was_quoted = false;
        let mut at = 0usize;
        loop {
            let Some(&byte) = self.line.get(at) else {
                // The line ended inside quotes: the field goes on in the next.
                anyhow::ensure!(quoted, "a CSV record ended without its newline");
                anyhow::ensure!(self.line.len() < MAX_RECORD_BYTES, "a CSV record longer than {MAX_RECORD_BYTES} bytes");
                anyhow::ensure!(self.input.read_until(b'\n', &mut self.line)? > 0, "the file ended inside a quoted field");
                continue;
            };
            at += 1;
            if quoted {
                match byte {
                    b'\\' if matches!(self.line.get(at), Some(b'"') | Some(b'\\')) => {
                        field.push(self.line[at]);
                        at += 1;
                    }
                    b'"' if self.line.get(at) == Some(&b'"') => {
                        field.push(b'"');
                        at += 1;
                    }
                    b'"' => quoted = false,
                    other => field.push(other),
                }
                continue;
            }
            match byte {
                b'"' => {
                    quoted = true;
                    was_quoted = true;
                }
                b',' | b'\n' => {
                    let end = byte == b'\n';
                    if end && field.last() == Some(&b'\r') {
                        field.pop();
                    }
                    let text = String::from_utf8(std::mem::take(&mut field))?;
                    self.fields.push(if text.is_empty() && !was_quoted { None } else { Some(text) });
                    was_quoted = false;
                    if end {
                        return Ok(());
                    }
                }
                other => field.push(other),
            }
        }
    }
}

fn trim_newline(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(format: Format, text: &str) -> Vec<Vec<Option<String>>> {
        let mut records = Records::new(text.as_bytes(), format);
        let mut out = Vec::new();
        while records.next().unwrap() {
            out.push(records.fields.clone());
        }
        out
    }

    /// Do: read COPY's CSV with a NULL, an empty quoted string, escapes and a newline in quotes.
    /// Expect: each field as PostgreSQL would load it.
    #[test]
    fn csv_reads_as_copy_wrote() {
        let rows = read(Format::Csv, "1,,\"\",\"a \\\"b\\\" \\\\ c\"\n2,\"two\nlines\",x,\n");
        assert_eq!(rows[0], [Some("1".into()), None, Some(String::new()), Some("a \"b\" \\ c".into())]);
        assert_eq!(rows[1], [Some("2".into()), Some("two\nlines".into()), Some("x".into()), None]);
    }

    /// Do: read dbgen's rows. Expect: each field between the pipes.
    #[test]
    fn pipe_reads_as_dbgen_wrote() {
        let rows = read(Format::Pipe, "1|AFRICA|lar deposits\n");
        assert_eq!(rows[0], [Some("1".into()), Some("AFRICA".into()), Some("lar deposits".into())]);
    }
}
