//! The parser oracle of docs/design/postgres.md section 3 (section 11's unit 3), for tests only:
//! PostgreSQL 19's own `raw_parser`, unpatched, taken out of `REL_19_0` and linked into
//! `pg-oracle` by scripts/postgres-oracle-extract.sh, run as a subprocess so no C and no `unsafe`
//! enter Amphora. Each statement gets 19's verdict: its tree (`nodeToStringWithLocations`) or its
//! refusal (SQLSTATE, message, cursor). [`split`] gives the statements psql sends for a script, so
//! 19's regression SQL is a corpus.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::arithmetic_side_effects, clippy::disallowed_macros))]

pub mod split;

use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// PostgreSQL 19's verdict on one statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Accepted: the List of RawStmt, as `nodeToStringWithLocations` prints it.
    Tree(String),
    /// Refused, with ErrorResponse's C, M and P fields (P is 0 when the error has no position).
    Refused { sqlstate: String, cursor: u32, message: String },
}

#[derive(Debug)]
pub enum OracleError {
    Io(io::Error),
    /// A NUL byte ends a statement on the oracle's input, as it ends a Query message's string.
    Nul,
    /// The oracle exited before it answered.
    Ended,
    /// The oracle's answer broke its frame format (scripts/postgres-oracle.c).
    Frame(&'static str),
}

impl fmt::Display for OracleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "the oracle's pipe failed: {e}"),
            Self::Nul => f.write_str("a statement with a NUL byte can't be sent"),
            Self::Ended => f.write_str("the oracle exited before it answered"),
            Self::Frame(what) => write!(f, "the oracle's answer is malformed: {what}"),
        }
    }
}

impl std::error::Error for OracleError {}

impl From<io::Error> for OracleError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// One `pg-oracle` process, asked one statement at a time.
pub struct Oracle {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl Oracle {
    pub fn spawn(program: &Path) -> Result<Self, OracleError> {
        let mut child = Command::new(program).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;
        let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(OracleError::Ended);
        };
        Ok(Self { child, input, output: BufReader::new(output) })
    }

    pub fn parse(&mut self, sql: &[u8]) -> Result<Verdict, OracleError> {
        if sql.contains(&0) {
            return Err(OracleError::Nul);
        }
        self.input.write_all(sql)?;
        self.input.write_all(&[0])?;
        self.input.flush()?;
        read_verdict(&mut self.output)
    }
}

impl Drop for Oracle {
    fn drop(&mut self) {
        // The process is this value's own: ended with it, its exit status not needed.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn number<T: std::str::FromStr>(word: Option<&str>, what: &'static str) -> Result<T, OracleError> {
    word.and_then(|w| w.parse().ok()).ok_or(OracleError::Frame(what))
}

/// One frame: `T <bytes>\n<tree>\n` or `E <SQLSTATE> <cursor> <bytes>\n<message>\n`.
pub fn read_verdict(r: &mut impl BufRead) -> Result<Verdict, OracleError> {
    let mut head = String::new();
    if r.read_line(&mut head)? == 0 {
        return Err(OracleError::Ended);
    }
    let head = head.strip_suffix('\n').ok_or(OracleError::Frame("a head without its newline"))?;
    let mut words = head.split(' ');
    let kind = words.next();
    let refusal = match kind {
        Some("T") => None,
        Some("E") => {
            let sqlstate = words.next().filter(|s| s.len() == 5 && s.bytes().all(|c| c.is_ascii_alphanumeric())).ok_or(OracleError::Frame("a SQLSTATE that isn't five characters"))?;
            Some((sqlstate.to_owned(), number::<u32>(words.next(), "a cursor that isn't a number")?))
        }
        _ => return Err(OracleError::Frame("a frame that is neither T nor E")),
    };
    let len: u64 = number(words.next(), "a length that isn't a number")?;
    if words.next().is_some() {
        return Err(OracleError::Frame("a head with more words than its kind takes"));
    }
    let mut body = Vec::new();
    if r.by_ref().take(len).read_to_end(&mut body)? as u64 != len {
        return Err(OracleError::Ended);
    }
    let mut end = [0u8; 1];
    r.read_exact(&mut end)?;
    if end != *b"\n" {
        return Err(OracleError::Frame("a body longer than its stated length"));
    }
    let body = String::from_utf8(body).map_err(|_| OracleError::Frame("a body that isn't UTF-8"))?;
    Ok(match refusal {
        None => Verdict::Tree(body),
        Some((sqlstate, cursor)) => Verdict::Refused { sqlstate, cursor, message: body },
    })
}

#[cfg(test)]
mod tests {
    use super::{OracleError, Verdict, read_verdict};

    fn read(frames: &str) -> Result<Verdict, OracleError> {
        read_verdict(&mut frames.as_bytes())
    }

    /// T-PG.3.1: the oracle's frames read back as the verdicts it printed.
    #[test]
    fn t_pg_3_1_frames_read_back_as_verdicts() {
        assert_eq!(read("T 4\n(a\n)\n").unwrap(), Verdict::Tree("(a\n)".into()));
        assert_eq!(
            read("E 42601 8 29\ntrailing junk after parameter\n").unwrap(),
            Verdict::Refused { sqlstate: "42601".into(), cursor: 8, message: "trailing junk after parameter".into() }
        );
        let mut two = "T 0\n\nE 0A000 0 1\nx\n".as_bytes();
        assert_eq!(read_verdict(&mut two).unwrap(), Verdict::Tree(String::new()));
        assert!(matches!(read_verdict(&mut two).unwrap(), Verdict::Refused { cursor: 0, .. }));
        assert!(matches!(read_verdict(&mut two), Err(OracleError::Ended)));
    }

    /// T-PG.3.1: a malformed frame is refused, never read past.
    #[test]
    fn t_pg_3_1_malformed_frames_are_refused() {
        for frame in ["X 1\na\n", "T\n", "T 3\nab\n", "T 1\nab\n", "E 4260 1 1\na\n", "E 42601 x 1\na\n", "T 1 1\na\n", "T 1", "T 1\n\u{0}"] {
            assert!(read(frame).is_err(), "{frame:?}");
        }
    }
}
