//! The extended protocol's messages (doc/src/sgml/protocol.sgml, "Message Formats") as 19's backend reads them
//! (REL_19_0 src/backend/tcop/postgres.c:4839-5040): a message that came whole and is malformed inside
//! refuses its statement, not the connection (`src/backend/libpq/pqformat.c`).

use amphora_pg_analysis::{Error, binary};

use super::wire::{PgError, Reader, SqlState};

/// What a Describe or a Close names; the unnamed one's name is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Object<'a> {
    Statement(&'a str),
    Portal(&'a str),
}

/// A message of the extended protocol but Sync and Flush.
pub enum Message<'a> {
    /// `types` are the parameters' the client gives, 0 for one left to analysis.
    Parse {
        statement: &'a str,
        sql: &'a str,
        types: Vec<u32>,
    },
    /// `values`: the parameters and the result's formats, read once the statement is found
    /// ([`Values`]), as `exec_bind_message` reads them.
    Bind {
        portal: &'a str,
        statement: &'a str,
        values: Values<'a>,
    },
    Describe(Object<'a>),
    /// At most `max_rows` rows, all of them where it isn't positive.
    Execute {
        portal: &'a str,
        max_rows: i32,
    },
    Close(Object<'a>),
}

/// The text of a message's part (`pq_getmsgstring` and `pg_client_to_server`).
pub fn text(bytes: &[u8]) -> Result<&str, PgError> {
    binary::text(bytes).map_err(refused)
}

fn refused(error: Error) -> PgError {
    PgError::error(SqlState::new(error.sqlstate), error.message)
}

impl<'a> Message<'a> {
    /// The message of type `tag`: `P`, `B`, `D`, `E` or `C`.
    pub fn read(tag: u8, body: &'a [u8]) -> Result<Self, PgError> {
        let mut body = Values(Reader::new(body));
        Ok(match tag {
            b'P' => {
                let statement = body.string()?;
                let sql = body.string()?;
                let count = body.count()?;
                let mut types = Vec::new();
                for _ in 0..count {
                    types.push(body.0.u32().map_err(PgError::of_statement)?);
                }
                body.end()?;
                Self::Parse {
                    statement,
                    sql,
                    types,
                }
            }
            b'B' => Self::Bind {
                portal: body.string()?,
                statement: body.string()?,
                values: body,
            },
            b'E' => {
                let portal = body.string()?;
                let max_rows = body.0.i32().map_err(PgError::of_statement)?;
                body.end()?;
                Self::Execute { portal, max_rows }
            }
            b'D' | b'C' => {
                let kind = body.0.byte().map_err(PgError::of_statement)?;
                let name = body.string()?;
                body.end()?;
                let object = match kind {
                    b'S' => Object::Statement(name),
                    b'P' => Object::Portal(name),
                    _ => {
                        let word = if tag == b'D' { "DESCRIBE" } else { "CLOSE" };
                        return Err(PgError::error(
                            SqlState::PROTOCOL_VIOLATION,
                            format!("invalid {word} message subtype {kind}"),
                        ));
                    }
                };
                if tag == b'D' {
                    Self::Describe(object)
                } else {
                    Self::Close(object)
                }
            }
            _ => {
                return Err(PgError::error(
                    SqlState::PROTOCOL_VIOLATION,
                    format!("invalid frontend message type {tag}"),
                ));
            }
        })
    }
}

/// What of a message isn't read yet.
pub struct Values<'a>(Reader<'a>);

impl<'a> Values<'a> {
    fn string(&mut self) -> Result<&'a str, PgError> {
        text(self.0.cstr().map_err(PgError::of_statement)?)
    }

    pub fn count(&mut self) -> Result<usize, PgError> {
        Ok(usize::from(self.0.u16().map_err(PgError::of_statement)?))
    }

    /// A list of format codes behind its count.
    pub fn formats(&mut self) -> Result<Formats, PgError> {
        let count = self.count()?;
        let mut codes = Vec::new();
        for _ in 0..count {
            codes.push(self.0.u16().map_err(PgError::of_statement)?.cast_signed());
        }
        Ok(Formats(codes))
    }

    /// A parameter's bytes behind their length, `None` for NULL's -1.
    pub fn value(&mut self) -> Result<Option<&'a [u8]>, PgError> {
        let length = self.0.i32().map_err(PgError::of_statement)?;
        if length == -1 {
            return Ok(None);
        }
        // A length past the message's end, as a negative one is, is the same refusal.
        let length = usize::try_from(length).unwrap_or(usize::MAX);
        Ok(Some(self.0.bytes(length).map_err(PgError::of_statement)?))
    }

    pub fn end(&self) -> Result<(), PgError> {
        self.0.end().map_err(PgError::of_statement)
    }
}

/// Format codes: none is text for each, one is for each, else one each.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Formats(Vec<i16>);

impl Formats {
    /// How many codes were given.
    pub fn given(&self) -> usize {
        self.0.len()
    }

    /// The code of the parameter or column at `at`, from 0.
    pub fn of(&self, at: usize) -> i16 {
        match self.0.as_slice() {
            [] => 0,
            [one] => *one,
            each => each.get(at).copied().unwrap_or(0),
        }
    }
}
