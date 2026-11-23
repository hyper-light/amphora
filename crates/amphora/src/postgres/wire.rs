//! PostgreSQL's frames: read from borrowed bytes with every length checked before it is trusted,
//! and backend messages written into the caller's buffer.

use std::fmt;

/// Protocol 3.0, the earliest PostgreSQL 19 serves (`src/include/libpq/pqcomm.h:94`).
pub const PROTOCOL_3_0: u32 = 3 << 16;
/// Protocol 3.2, the latest (`pqcomm.h:95`); its cancel keys are 32 bytes.
pub const PROTOCOL_3_2: u32 = (3 << 16) | 2;
/// The request codes that share the startup packet's first word (`pqcomm.h:122`, `:128-129`).
pub const CANCEL_REQUEST_CODE: u32 = (1234 << 16) | 5678;
pub const SSL_REQUEST_CODE: u32 = (1234 << 16) | 5679;
pub const GSS_REQUEST_CODE: u32 = (1234 << 16) | 5680;

/// The longest startup packet body, its length word excluded (`pqcomm.h:148`,
/// `src/backend/tcop/backend_startup.c:566-571`).
pub const MAX_STARTUP_PACKET_LENGTH: usize = 10_000;
/// The longest body of a small message: Sync, Flush, Close, Describe, Execute, Terminate, CopyDone,
/// CopyFail (`src/include/libpq/libpq.h:33`, `src/backend/tcop/postgres.c:420-455`).
pub const SMALL_MESSAGE_LIMIT: usize = 10_000;
/// The longest body of Query, Parse, Bind, FunctionCall and CopyData: `MaxAllocSize - 1`
/// (`libpq.h:34`, `src/include/utils/memutils.h:40`).
pub const LARGE_MESSAGE_LIMIT: usize = 0x3fff_fffe;
/// The longest SASL message (`src/include/libpq/auth.h:33`).
pub const AUTH_TOKEN_LIMIT: usize = 65_535;

/// A SQLSTATE from `src/backend/utils/errcodes.txt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SqlState(&'static str);

impl SqlState {
    pub const FEATURE_NOT_SUPPORTED: Self = Self("0A000");
    pub const PROTOCOL_VIOLATION: Self = Self("08P01");
    pub const INVALID_AUTHORIZATION_SPECIFICATION: Self = Self("28000");
    pub const INVALID_PASSWORD: Self = Self("28P01");
    pub const CHARACTER_NOT_IN_REPERTOIRE: Self = Self("22021");
    pub const SYNTAX_ERROR: Self = Self("42601");
    pub const INTERNAL_ERROR: Self = Self("XX000");

    #[must_use]
    pub fn code(self) -> &'static str {
        self.0
    }
}

/// How far an error reaches: `Error` ends the statement, `Fatal` the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Fatal,
}

impl Severity {
    fn word(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Fatal => "FATAL",
        }
    }
}

/// What an ErrorResponse carries: severity, SQLSTATE, message and an optional detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PgError {
    pub severity: Severity,
    pub code: SqlState,
    pub message: String,
    pub detail: Option<String>,
}

impl PgError {
    pub fn fatal(code: SqlState, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Fatal,
            code,
            message: message.into(),
            detail: None,
        }
    }

    pub fn error(code: SqlState, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code,
            message: message.into(),
            detail: None,
        }
    }

    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// A FATAL protocol violation (`08P01`), the answer to a malformed frame.
    pub fn violation(message: impl Into<String>) -> Self {
        Self::fatal(SqlState::PROTOCOL_VIOLATION, message)
    }
}

impl fmt::Display for PgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {}: {}",
            self.severity.word(),
            self.code.0,
            self.message
        )
    }
}

impl std::error::Error for PgError {}

/// A frame at the head of the input: whole, or the total length the caller must hold (and charge
/// to its budget) before it is whole.
#[derive(Debug, PartialEq, Eq)]
pub enum Frame<'a> {
    Whole {
        tag: Option<u8>,
        body: &'a [u8],
        len: usize,
    },
    Need(usize),
}

fn length_word(input: &[u8], at: usize) -> Option<u32> {
    let bytes = input.get(at..at.checked_add(4)?)?;
    let word: [u8; 4] = bytes.try_into().ok()?;
    Some(u32::from_be_bytes(word))
}

/// The startup-phase packet at the head of `input`: a length word that counts itself, then the
/// body. A length outside 4 + 4 ..= 4 + [`MAX_STARTUP_PACKET_LENGTH`] is refused before any byte
/// of the body is held (`backend_startup.c:563-572`).
pub fn startup_frame(input: &[u8]) -> Result<Frame<'_>, PgError> {
    let Some(word) = length_word(input, 0) else {
        return Ok(Frame::Need(4));
    };
    let total = usize::try_from(word)
        .map_err(|_| PgError::violation("invalid length of startup packet"))?;
    let body_len = total
        .checked_sub(4)
        .ok_or_else(|| PgError::violation("invalid length of startup packet"))?;
    if !(4..=MAX_STARTUP_PACKET_LENGTH).contains(&body_len) {
        return Err(PgError::violation("invalid length of startup packet"));
    }
    match input.get(4..total) {
        Some(body) => Ok(Frame::Whole {
            tag: None,
            body,
            len: total,
        }),
        None => Ok(Frame::Need(total)),
    }
}

/// The typed message at the head of `input`: a type byte, a length word that counts itself, then
/// the body, whose length `limit` bounds for that type (`src/backend/libpq/pqcomm.c:1218-1228`).
/// `limit` answers `None` for a type the phase doesn't take: PostgreSQL's
/// "invalid frontend message type" (`postgres.c:466-468`).
pub fn message_frame(
    input: &[u8],
    limit: impl Fn(u8) -> Option<usize>,
) -> Result<Frame<'_>, PgError> {
    let Some(&tag) = input.first() else {
        return Ok(Frame::Need(5));
    };
    let max = limit(tag)
        .ok_or_else(|| PgError::violation(format!("invalid frontend message type {tag}")))?;
    let Some(word) = length_word(input, 1) else {
        return Ok(Frame::Need(5));
    };
    let counted =
        usize::try_from(word).map_err(|_| PgError::violation("invalid message length"))?;
    let body_len = counted
        .checked_sub(4)
        .ok_or_else(|| PgError::violation("invalid message length"))?;
    if body_len > max {
        return Err(PgError::violation("invalid message length"));
    }
    let total = counted
        .checked_add(1)
        .ok_or_else(|| PgError::violation("invalid message length"))?;
    match input.get(5..total) {
        Some(body) => Ok(Frame::Whole {
            tag: Some(tag),
            body,
            len: total,
        }),
        None => Ok(Frame::Need(total)),
    }
}

/// A cursor over one message's body; each read fails rather than run past the end, with
/// PostgreSQL's words for it (`src/backend/libpq/pqformat.c`).
pub struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    #[must_use]
    pub fn new(body: &'a [u8]) -> Self {
        Self { rest: body }
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], PgError> {
        let taken = self
            .rest
            .get(..n)
            .ok_or_else(|| PgError::violation("insufficient data left in message"))?;
        self.rest = self.rest.get(n..).unwrap_or_default();
        Ok(taken)
    }

    pub fn u32(&mut self) -> Result<u32, PgError> {
        let word: [u8; 4] = self
            .bytes(4)?
            .try_into()
            .map_err(|_| PgError::violation("insufficient data left in message"))?;
        Ok(u32::from_be_bytes(word))
    }

    pub fn i32(&mut self) -> Result<i32, PgError> {
        let word: [u8; 4] = self
            .bytes(4)?
            .try_into()
            .map_err(|_| PgError::violation("insufficient data left in message"))?;
        Ok(i32::from_be_bytes(word))
    }

    /// A NUL-terminated string, without its NUL.
    pub fn cstr(&mut self) -> Result<&'a [u8], PgError> {
        let end = self
            .rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| PgError::violation("invalid string in message"))?;
        let text = self.rest.get(..end).unwrap_or_default();
        self.rest = self.rest.get(end.saturating_add(1)..).unwrap_or_default();
        Ok(text)
    }

    #[must_use]
    pub fn rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.rest)
    }

    /// The body must be used up (`pq_getmsgend`).
    pub fn end(&self) -> Result<(), PgError> {
        if self.rest.is_empty() {
            Ok(())
        } else {
            Err(PgError::violation("invalid message format"))
        }
    }
}

/// Appends one backend message: its type byte, a length word that counts itself, then what
/// `body` writes. A body past `i32::MAX` bytes is refused and the buffer left as it was.
pub fn message(out: &mut Vec<u8>, tag: u8, body: impl FnOnce(&mut Vec<u8>)) -> Result<(), PgError> {
    let start = out.len();
    out.push(tag);
    out.extend_from_slice(&[0; 4]);
    body(out);
    let counted = out
        .len()
        .checked_sub(start)
        .and_then(|n| n.checked_sub(1))
        .and_then(|n| i32::try_from(n).ok());
    let at = start
        .checked_add(1)
        .and_then(|a| Some(a..a.checked_add(4)?));
    if let (Some(counted), Some(at)) = (counted, at)
        && let Some(slot) = out.get_mut(at)
    {
        slot.copy_from_slice(&counted.to_be_bytes());
        return Ok(());
    }
    out.truncate(start);
    Err(PgError::fatal(
        SqlState::INTERNAL_ERROR,
        "a message longer than the protocol's length word",
    ))
}

pub fn put_cstr(out: &mut Vec<u8>, text: &[u8]) {
    out.extend_from_slice(text);
    out.push(0);
}

/// ErrorResponse with the fields PostgreSQL sends a client: S, V, C, M and D when there is one
/// (`doc/src/sgml/protocol.sgml`, "Error and Notice Message Fields").
pub fn error_response(out: &mut Vec<u8>, error: &PgError) -> Result<(), PgError> {
    message(out, b'E', |out| {
        for (field, value) in [
            (b'S', error.severity.word()),
            (b'V', error.severity.word()),
            (b'C', error.code.0),
            (b'M', error.message.as_str()),
        ] {
            out.push(field);
            put_cstr(out, value.as_bytes());
        }
        if let Some(detail) = &error.detail {
            out.push(b'D');
            put_cstr(out, detail.as_bytes());
        }
        out.push(0);
    })
}
