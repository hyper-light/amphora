//! The oracle's answers checked in (crates/pg-oracle/corpus.digest), so `cargo test` holds the Rust
//! parser to PostgreSQL 19.0 with no C and no oracle built: for every statement psql sends from 19's
//! regression SQL, keyed by file and statement index, what 19's `raw_parser` answered (a hash of its
//! tree, or its refusal's SQLSTATE and cursor) and what its scanner gave the grammar (a hash of the
//! `--tokens` lines, or the scan's refusal). scripts/postgres-digest.sh regenerates the file from
//! the oracle and checks it whenever vendor/postgres changes.
//!
//! The file: a `@ <file name>` line per regression file in name order, then a line per statement in
//! the order [`split::statements`] gives them, `<tree> <tokens>`, each `T<16 hex digits>` (FNV-1a 64
//! of the oracle's body) or `E<SQLSTATE>:<cursor>`; `-` for a statement that isn't UTF-8 and so is
//! never sent.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::io;
use std::path::{Path, PathBuf};

use crate::{Verdict, split};

/// 19's regression SQL as vendored, the corpus the digest covers.
pub fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/postgres/src/test/regress/sql")
}

/// Every regression file by name, in name order, with the statements psql sends from it.
pub fn corpus(dir: &Path) -> io::Result<Vec<(String, Vec<Vec<u8>>)>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|x| x == "sql")
            && let Some(name) = path.file_name().and_then(|n| n.to_str())
        {
            files.push((name.to_owned(), split::statements(&std::fs::read(&path)?)));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

/// FNV-1a, 64 bits: the digest's hash, simple enough to recompute anywhere. Collisions only hide a
/// difference by chance; no one chooses these inputs against it.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// One answer of the oracle's, as the digest keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// Accepted: the hash of the answer's body (the tree, or the token lines).
    Accepted(u64),
    Refused {
        sqlstate: String,
        cursor: u32,
    },
}

impl From<&Verdict> for Answer {
    fn from(v: &Verdict) -> Self {
        match v {
            Verdict::Tree(body) => Self::Accepted(fnv1a(body.as_bytes())),
            Verdict::Refused {
                sqlstate, cursor, ..
            } => Self::Refused {
                sqlstate: sqlstate.clone(),
                cursor: *cursor,
            },
        }
    }
}

impl fmt::Display for Answer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Accepted(h) => write!(f, "T{h:016x}"),
            Self::Refused { sqlstate, cursor } => write!(f, "E{sqlstate}:{cursor}"),
        }
    }
}

/// A statement's two answers: the parse's and the scan's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub tree: Answer,
    pub tokens: Answer,
}

/// Each regression file's statements by index; `None` for a statement that isn't UTF-8.
pub type Digest = BTreeMap<String, Vec<Option<Statement>>>;

pub fn write(digest: &Digest) -> String {
    let mut out = String::new();
    for (file, statements) in digest {
        let _ = writeln!(out, "@ {file}");
        for s in statements {
            let _ = match s {
                Some(s) => writeln!(out, "{} {}", s.tree, s.tokens),
                None => writeln!(out, "-"),
            };
        }
    }
    out
}

/// A digest line that isn't one, with its 1-based line number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Malformed(pub usize);

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "corpus.digest line {} is malformed", self.0)
    }
}

impl std::error::Error for Malformed {}

fn answer(word: &str) -> Option<Answer> {
    if let Some(hex) = word.strip_prefix('T') {
        return (hex.len() == 16)
            .then(|| u64::from_str_radix(hex, 16).ok())
            .flatten()
            .map(Answer::Accepted);
    }
    let (sqlstate, cursor) = word.strip_prefix('E')?.split_once(':')?;
    (sqlstate.len() == 5 && sqlstate.bytes().all(|c| c.is_ascii_alphanumeric())).then_some(())?;
    Some(Answer::Refused {
        sqlstate: sqlstate.to_owned(),
        cursor: cursor.parse().ok()?,
    })
}

pub fn read(text: &str) -> Result<Digest, Malformed> {
    let mut digest = Digest::new();
    let mut current: Option<&mut Vec<Option<Statement>>> = None;
    for (at, line) in text.lines().enumerate() {
        let malformed = Malformed(at.saturating_add(1));
        if let Some(file) = line.strip_prefix("@ ") {
            current = Some(digest.entry(file.to_owned()).or_default());
            continue;
        }
        let statement = match line.split_once(' ') {
            None if line == "-" => None,
            Some((tree, tokens)) => Some(Statement {
                tree: answer(tree).ok_or(malformed.clone())?,
                tokens: answer(tokens).ok_or(malformed.clone())?,
            }),
            None => return Err(malformed),
        };
        current.as_mut().ok_or(malformed)?.push(statement);
    }
    Ok(digest)
}

#[cfg(test)]
mod tests {
    use super::{Answer, Digest, Statement, fnv1a, read, write};

    /// T-PG.3.5: FNV-1a 64's published vectors (the empty string and "a").
    #[test]
    fn t_pg_3_5_the_hash_is_fnv1a_64() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    /// T-PG.3.5: a digest written and read back is the same digest, keyed by file and index.
    #[test]
    fn t_pg_3_5_a_digest_reads_back_as_written() {
        let refused = Answer::Refused {
            sqlstate: "42601".into(),
            cursor: 8,
        };
        let mut digest = Digest::new();
        digest.insert(
            "a.sql".into(),
            vec![
                Some(Statement {
                    tree: Answer::Accepted(1),
                    tokens: Answer::Accepted(u64::MAX),
                }),
                None,
            ],
        );
        digest.insert(
            "b.sql".into(),
            vec![Some(Statement {
                tree: refused.clone(),
                tokens: refused,
            })],
        );
        let text = write(&digest);
        assert_eq!(
            text,
            "@ a.sql\nT0000000000000001 Tffffffffffffffff\n-\n@ b.sql\nE42601:8 E42601:8\n"
        );
        assert_eq!(read(&text).unwrap(), digest);
    }

    /// T-PG.3.5: a malformed line is refused with its line number, never skipped.
    #[test]
    fn t_pg_3_5_malformed_lines_are_refused() {
        for (text, line) in [
            ("T0000000000000001 T0000000000000001\n", 1),
            ("@ a.sql\nT01 T0000000000000001\n", 2),
            ("@ a.sql\nE4260:1 E42601:1\n", 2),
            ("@ a.sql\nE42601:x -\n", 2),
            ("@ a.sql\nx\n", 2),
        ] {
            assert_eq!(read(text).unwrap_err().0, line, "{text:?}");
        }
    }
}
