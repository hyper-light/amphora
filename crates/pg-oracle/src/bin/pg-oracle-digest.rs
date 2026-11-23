//! Writes crates/pg-oracle/corpus.digest to stdout: every regression statement's answers from the
//! oracle built by scripts/postgres-oracle-extract.sh, its tree and its tokens (scripts/postgres-digest.sh).
//! usage: pg-oracle-digest <pg-oracle>

use std::io::Write as _;
use std::path::PathBuf;

use amphora_pg_oracle::Oracle;
use amphora_pg_oracle::digest::{self, Answer, Digest, Statement};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let program = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("usage: pg-oracle-digest <pg-oracle>")?,
    );
    let (mut trees, mut tokens) = (Oracle::spawn(&program)?, Oracle::spawn_tokens(&program)?);
    let mut out = Digest::new();
    for (file, statements) in digest::corpus(&digest::corpus_dir())? {
        let mut answers = Vec::with_capacity(statements.len());
        for sql in statements {
            answers.push(match std::str::from_utf8(&sql) {
                Err(_) => None,
                Ok(_) => Some(Statement {
                    tree: Answer::from(&trees.parse(&sql)?),
                    tokens: Answer::from(&tokens.parse(&sql)?),
                }),
            });
        }
        out.insert(file, answers);
    }
    std::io::stdout()
        .lock()
        .write_all(digest::write(&out).as_bytes())?;
    Ok(())
}
