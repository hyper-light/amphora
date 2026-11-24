//! One PostgreSQL connection as a state machine: the opening (direct TLS, SSLRequest,
//! GSSENCRequest, CancelRequest, StartupMessage), SCRAM, the session's parameters, and the simple
//! query loop. [`Session::step`] reads at most one frame from the bytes the caller holds and
//! appends its answer to the caller's buffer; it allocates nothing the frame's length didn't
//! bound, and the caller charges a frame's length before holding it ([`Next::Need`]).

use super::scram::{self, Binding, ScramSecret, ScramServer};
use super::wire::{self, Frame, PROTOCOL_3_2, PgError, Reader, SqlState};

/// What PostgreSQL 19 reports as `server_version`.
pub const SERVER_VERSION: &str = "19.0";
/// The longest name PostgreSQL keeps, in bytes: `NAMEDATALEN - 1` (`src/include/pg_config_manual.h:39`).
const NAME_MAX: usize = 63;
/// A cancel key's bytes in a CancelRequest (`src/backend/tcop/backend_startup.c:63`, `:960`).
const CANCEL_KEY_MAX: usize = 256;

/// The server's side of a connection, fixed for its life.
#[derive(Debug, Clone)]
pub struct Config {
    /// What a TLS handshake would bind SCRAM to; `None` answers SSLRequest `N` and refuses direct TLS.
    pub tls: Option<Binding>,
    /// The session's `TimeZone` before any SET (PostgreSQL's comes from initdb).
    pub time_zone: String,
    /// A per-server secret for the mock verifier of a role that doesn't exist.
    pub mock_nonce: [u8; 32],
}

/// What this connection draws from the caller's randomness: its process id and cancel key for
/// BackendKeyData, and the SCRAM server nonce's raw bytes.
#[derive(Clone)]
pub struct Seed {
    pub process_id: i32,
    pub cancel_key: [u8; 32],
    pub scram_nonce: [u8; scram::RAW_NONCE_LEN],
}

/// A role that may log in.
#[derive(Debug, Clone)]
pub struct Role {
    pub secret: ScramSecret,
    pub superuser: bool,
}

/// The roles, as the catalog keeps them.
pub trait Roles {
    fn role(&self, user: &str) -> Option<Role>;
}

/// What the caller does after a step.
#[derive(Debug, PartialEq, Eq)]
pub enum Next {
    /// Step again with the rest of the input.
    Continue,
    /// The next frame is this many bytes long in all: charge them, hold them, step again.
    Need(usize),
    /// Send what was written, then take the TLS handshake (answered `S`).
    StartTls,
    /// The first byte opened a TLS handshake: take it, then refuse it unless its ALPN is
    /// `postgresql` ([`direct_tls_alpn`]).
    StartDirectTls,
    /// A CancelRequest for another session; this connection then ends with no reply.
    Cancel { process_id: i32, key: Vec<u8> },
    /// Send what was written, then close.
    Close,
    /// Close with nothing sent, and log why: PostgreSQL's COMMERROR, which goes to the server's log
    /// and never to the client (`src/backend/tcop/backend_startup.c:566-571`).
    CloseLogged(PgError),
}

/// A step's result: the input bytes it used and what comes next.
#[derive(Debug, PartialEq, Eq)]
pub struct Step {
    pub consumed: usize,
    pub next: Next,
}

/// Direct TLS is taken only with ALPN `postgresql` (`src/include/libpq/pqcomm.h:189`); without it
/// PostgreSQL closes the connection (`backend_startup.c:470-477`).
pub fn direct_tls_alpn(alpn: Option<&[u8]>) -> Result<(), PgError> {
    if alpn == Some(b"postgresql".as_slice()) {
        Ok(())
    } else {
        Err(PgError::violation(
            "received direct SSL connection request without ALPN protocol negotiation extension",
        ))
    }
}

/// The startup packet's settings that outlive authentication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Startup {
    /// The version the session speaks: the client's, capped at 3.2 (`backend_startup.c:755-763`).
    pub version: u32,
    pub user: String,
    pub database: String,
    pub application_name: String,
    pub client_encoding: &'static str,
    /// Other settings, kept as sent until the session applies them as SET does.
    pub options: Vec<(String, String)>,
}

enum Phase {
    Opening {
        first: bool,
        ssl_done: bool,
        gss_done: bool,
    },
    SaslInitial {
        startup: Startup,
        role: Option<Role>,
        binding: Binding,
    },
    SaslFinal {
        startup: Startup,
        superuser: bool,
        exchange: ScramServer,
    },
    /// Idle, or after an extended-protocol error until the next Sync.
    Ready {
        startup: Startup,
        skip_to_sync: bool,
    },
    Closed,
}

/// A transaction block's state, what ReadyForQuery reports (`src/backend/tcop/postgres.c`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxStatus {
    Idle,
    InTransaction,
    Failed,
}

impl TxStatus {
    fn byte(self) -> u8 {
        match self {
            Self::Idle => b'I',
            Self::InTransaction => b'T',
            Self::Failed => b'E',
        }
    }
}

/// What runs a session's statements: the shell that has a database, or a test's.
pub trait Statements {
    fn status(&self) -> TxStatus;
    /// Runs a Query message's statements and appends the messages that answer them, all but the
    /// ReadyForQuery that ends them. A statement's refusal is an ErrorResponse in `out`; `Err` is a
    /// message that can't be written.
    fn simple_query(&mut self, sql: &str, out: &mut Vec<u8>) -> Result<(), PgError>;
}

/// One connection.
pub struct Session {
    config: Config,
    seed: Seed,
    phase: Phase,
    /// Why the server can't take this session, answered once its StartupMessage is read.
    refused: Option<PgError>,
    /// What runs its statements: none refuses each with `0A000`.
    statements: Option<Box<dyn Statements>>,
}

fn fatal(out: &mut Vec<u8>, consumed: usize, error: &PgError) -> Result<Step, PgError> {
    wire::error_response(out, error)?;
    Ok(Step {
        consumed,
        next: Next::Close,
    })
}

/// PostgreSQL keeps a name's first `NAMEDATALEN - 1` bytes, cut at a character boundary.
fn clip_name(name: &str) -> String {
    let mut end = name.len().min(NAME_MAX);
    while !name.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    name.get(..end).unwrap_or_default().to_owned()
}

/// `application_name` as PostgreSQL keeps it: printable ASCII, other bytes as `\xNN`
/// (`src/common/string.c`, `pg_clean_ascii`).
fn clean_ascii(value: &[u8]) -> String {
    let mut out = String::with_capacity(value.len());
    for &b in value {
        if (32..=126).contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("\\x{b:02x}"));
        }
    }
    out
}

fn utf8(bytes: &[u8]) -> Result<&str, PgError> {
    std::str::from_utf8(bytes).map_err(|_| {
        PgError::fatal(
            SqlState::CHARACTER_NOT_IN_REPERTOIRE,
            "invalid byte sequence for encoding \"UTF8\"",
        )
    })
}

/// The client encodings a UTF8 server takes without converting: UTF8, and SQL_ASCII, which passes
/// bytes through as they are.
fn client_encoding(value: &str) -> Option<&'static str> {
    let folded: String = value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    match folded.as_str() {
        "utf8" | "unicode" => Some("UTF8"),
        "sqlascii" => Some("SQL_ASCII"),
        _ => None,
    }
}

/// Reads a StartupMessage body after its version word (`backend_startup.c:780-890`). Returns the
/// settings and the `_pq_.` options PostgreSQL doesn't know.
fn read_startup(version: u32, body: &mut Reader<'_>) -> Result<(Startup, Vec<String>), PgError> {
    let layout =
        || PgError::violation("invalid startup packet layout: expected terminator as last byte");
    let mut user = None;
    let mut database = None;
    let mut application_name = String::new();
    let mut encoding = "UTF8";
    let mut options = Vec::new();
    let mut unknown = Vec::new();
    loop {
        let name = body.cstr().map_err(|_| layout())?;
        if name.is_empty() {
            body.end().map_err(|_| layout())?;
            break;
        }
        let value = body.cstr().map_err(|_| layout())?;
        let name = utf8(name)?;
        match name {
            "user" => user = Some(clip_name(utf8(value)?)),
            "database" => database = Some(clip_name(utf8(value)?)),
            "application_name" => application_name = clean_ascii(value),
            "client_encoding" => {
                let value = utf8(value)?;
                encoding = client_encoding(value).ok_or_else(|| {
                    PgError::fatal(SqlState::FEATURE_NOT_SUPPORTED, format!("client_encoding \"{value}\" is not supported: Amphora serves UTF8 and SQL_ASCII"))
                })?;
            }
            "replication" => {
                return Err(PgError::fatal(
                    SqlState::FEATURE_NOT_SUPPORTED,
                    "replication connections are not supported",
                ));
            }
            _ if name.starts_with("_pq_.") => unknown.push(name.to_owned()),
            _ => options.push((name.to_owned(), utf8(value)?.to_owned())),
        }
    }
    let user = user.filter(|u| !u.is_empty()).ok_or_else(|| {
        PgError::fatal(
            SqlState::INVALID_AUTHORIZATION_SPECIFICATION,
            "no PostgreSQL user name specified in startup packet",
        )
    })?;
    let database = database
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| user.clone());
    Ok((
        Startup {
            version,
            user,
            database,
            application_name,
            client_encoding: encoding,
            options,
        },
        unknown,
    ))
}

/// The empty query: nothing but whitespace, semicolons and comments (`src/backend/parser/scan.l`,
/// `space`, `comment`, `xc`). An unterminated block comment is a syntax error, as there.
fn empty_query(sql: &[u8]) -> Result<bool, PgError> {
    let mut i = 0usize;
    while let Some(&b) = sql.get(i) {
        match b {
            b' ' | b'\t' | b'\n' | b'\r' | b'\x0b' | b'\x0c' | b';' => i = i.saturating_add(1),
            b'-' if sql.get(i.saturating_add(1)) == Some(&b'-') => {
                i = sql
                    .iter()
                    .skip(i)
                    .position(|&c| c == b'\n' || c == b'\r')
                    .map_or(sql.len(), |p| i.saturating_add(p));
            }
            b'/' if sql.get(i.saturating_add(1)) == Some(&b'*') => {
                let mut depth = 1usize;
                i = i.saturating_add(2);
                while depth > 0 {
                    match (sql.get(i), sql.get(i.saturating_add(1))) {
                        (Some(b'/'), Some(b'*')) => {
                            depth = depth.saturating_add(1);
                            i = i.saturating_add(2);
                        }
                        (Some(b'*'), Some(b'/')) => {
                            depth = depth.saturating_sub(1);
                            i = i.saturating_add(2);
                        }
                        (Some(_), _) => i = i.saturating_add(1),
                        (None, _) => {
                            return Err(PgError::error(
                                SqlState::SYNTAX_ERROR,
                                "unterminated /* comment",
                            ));
                        }
                    }
                }
            }
            _ => return Ok(false),
        }
    }
    Ok(true)
}

fn parameter_status(out: &mut Vec<u8>, name: &str, value: &str) -> Result<(), PgError> {
    wire::message(out, b'S', |out| {
        wire::put_cstr(out, name.as_bytes());
        wire::put_cstr(out, value.as_bytes());
    })
}

fn ready_for_query(out: &mut Vec<u8>, status: TxStatus) -> Result<(), PgError> {
    wire::message(out, b'Z', |out| out.push(status.byte()))
}

fn authentication(out: &mut Vec<u8>, code: i32, data: &[u8]) -> Result<(), PgError> {
    wire::message(out, b'R', |out| {
        out.extend_from_slice(&code.to_be_bytes());
        out.extend_from_slice(data);
    })
}

/// The SASL message at the head of `input` (`src/backend/libpq/auth-sasl.c:85-100`): its body, or
/// the step that waits for it or ends the connection.
fn sasl_frame<'a>(
    input: &'a [u8],
    out: &mut Vec<u8>,
) -> Result<Result<(&'a [u8], usize), Step>, PgError> {
    match wire::message_frame(input, |_| Some(wire::AUTH_TOKEN_LIMIT)) {
        Ok(Frame::Whole {
            tag: Some(b'p'),
            body,
            len,
        }) => Ok(Ok((body, len))),
        Ok(Frame::Whole { tag, len, .. }) => {
            let tag = tag.unwrap_or_default();
            Ok(Err(fatal(
                out,
                len,
                &PgError::violation(format!("expected SASL response, got message type {tag}")),
            )?))
        }
        Ok(Frame::Need(total)) => Ok(Err(Step {
            consumed: 0,
            next: Next::Need(total),
        })),
        Err(error) => Ok(Err(fatal(out, 0, &error)?)),
    }
}

/// SASLInitialResponse: the mechanism, then the client-first-message behind its length word.
fn read_sasl_initial(body: &[u8]) -> Result<(&str, &[u8]), PgError> {
    let mut reader = Reader::new(body);
    let mechanism = utf8(reader.cstr()?)?;
    let length = reader.i32()?;
    let data = reader.rest();
    if usize::try_from(length).ok() != Some(data.len()) {
        return Err(PgError::violation("invalid message format"));
    }
    Ok((mechanism, data))
}

/// The longest body each message type may have in the ready phase, `None` for a type PostgreSQL
/// doesn't take from a client (`postgres.c:404-470`).
fn ready_limit(tag: u8) -> Option<usize> {
    match tag {
        b'Q' | b'F' | b'P' | b'B' | b'd' => Some(wire::LARGE_MESSAGE_LIMIT),
        b'X' | b'C' | b'D' | b'E' | b'H' | b'S' | b'c' | b'f' => Some(wire::SMALL_MESSAGE_LIMIT),
        _ => None,
    }
}

impl Session {
    #[must_use]
    pub fn new(config: Config, seed: Seed) -> Self {
        Self {
            config,
            seed,
            phase: Phase::Opening {
                first: true,
                ssl_done: false,
                gss_done: false,
            },
            refused: None,
            statements: None,
        }
    }

    /// The session whose statements `statements` runs.
    #[must_use]
    pub fn with_statements(mut self, statements: Box<dyn Statements>) -> Self {
        self.statements = Some(statements);
        self
    }

    /// What ReadyForQuery reports.
    fn tx_status(&self) -> TxStatus {
        self.statements
            .as_ref()
            .map_or(TxStatus::Idle, |s| s.status())
    }

    /// Refuses the session with `error` (FATAL) once its StartupMessage is read, where PostgreSQL
    /// refuses a connection it has no room for (`src/backend/tcop/backend_startup.c:313-360`), so
    /// the client sees what PostgreSQL's would.
    pub fn refuse(&mut self, error: PgError) {
        self.refused = Some(error);
    }

    /// The startup settings, once a StartupMessage has been read.
    #[must_use]
    pub fn startup(&self) -> Option<&Startup> {
        match &self.phase {
            Phase::SaslInitial { startup, .. }
            | Phase::SaslFinal { startup, .. }
            | Phase::Ready { startup, .. } => Some(startup),
            Phase::Opening { .. } | Phase::Closed => None,
        }
    }

    /// Reads at most one frame from the head of `input` and appends the answer to `out`. Bad input
    /// is answered with ErrorResponse (FATAL ones then [`Next::Close`]); only a message that can't
    /// be written is an `Err`.
    pub fn step(
        &mut self,
        input: &[u8],
        roles: &impl Roles,
        out: &mut Vec<u8>,
    ) -> Result<Step, PgError> {
        let phase = std::mem::replace(&mut self.phase, Phase::Closed);
        // Each arm puts the phase back when the connection goes on; one that ends it leaves Closed.
        match phase {
            Phase::Opening {
                first,
                ssl_done,
                gss_done,
            } => self.opening(first, ssl_done, gss_done, input, roles, out),
            Phase::SaslInitial {
                startup,
                role,
                binding,
            } => self.sasl_initial(startup, role, binding, input, out),
            Phase::SaslFinal {
                startup,
                superuser,
                exchange,
            } => self.sasl_final(startup, superuser, exchange, input, out),
            Phase::Ready {
                startup,
                skip_to_sync,
            } => self.ready(startup, skip_to_sync, input, out),
            Phase::Closed => Ok(Step {
                consumed: 0,
                next: Next::Close,
            }),
        }
    }

    fn opening(
        &mut self,
        first: bool,
        ssl_done: bool,
        gss_done: bool,
        input: &[u8],
        roles: &impl Roles,
        out: &mut Vec<u8>,
    ) -> Result<Step, PgError> {
        if first && input.first() == Some(&0x16) {
            // A TLS ClientHello's record type: no startup length starts with it
            // (`backend_startup.c:418-450`).
            if self.config.tls.is_none() {
                return Ok(Step {
                    consumed: 0,
                    next: Next::Close,
                });
            }
            self.phase = Phase::Opening {
                first: false,
                ssl_done: true,
                gss_done,
            };
            return Ok(Step {
                consumed: 0,
                next: Next::StartDirectTls,
            });
        }
        let (body, len) = match wire::startup_frame(input) {
            Ok(Frame::Whole { body, len, .. }) => (body, len),
            Ok(Frame::Need(total)) => {
                self.phase = Phase::Opening {
                    first,
                    ssl_done,
                    gss_done,
                };
                return Ok(Step {
                    consumed: 0,
                    next: Next::Need(total),
                });
            }
            Err(error) => {
                return Ok(Step {
                    consumed: 0,
                    next: Next::CloseLogged(error),
                });
            }
        };
        let mut reader = Reader::new(body);
        let code = reader.u32()?;
        let after = input.len() > len;
        match code {
            wire::SSL_REQUEST_CODE if !ssl_done => {
                if after {
                    return fatal(out, len, &PgError::violation("received unencrypted data after SSL request")
                        .with_detail("This could be either a client-software bug or evidence of an attempted man-in-the-middle attack."));
                }
                let tls = self.config.tls.is_some();
                out.push(if tls { b'S' } else { b'N' });
                self.phase = Phase::Opening {
                    first: false,
                    ssl_done: true,
                    gss_done,
                };
                Ok(Step {
                    consumed: len,
                    next: if tls { Next::StartTls } else { Next::Continue },
                })
            }
            wire::GSS_REQUEST_CODE if !gss_done => {
                if after {
                    return fatal(out, len, &PgError::violation("received unencrypted data after GSSAPI encryption request")
                        .with_detail("This could be either a client-software bug or evidence of an attempted man-in-the-middle attack."));
                }
                out.push(b'N');
                self.phase = Phase::Opening {
                    first: false,
                    ssl_done,
                    gss_done: true,
                };
                Ok(Step {
                    consumed: len,
                    next: Next::Continue,
                })
            }
            wire::CANCEL_REQUEST_CODE => {
                let process_id = reader.i32();
                let key = reader.rest();
                match process_id {
                    Ok(process_id) if (1..=CANCEL_KEY_MAX).contains(&key.len()) => Ok(Step {
                        consumed: len,
                        next: Next::Cancel {
                            process_id,
                            key: key.to_vec(),
                        },
                    }),
                    _ => Ok(Step {
                        consumed: len,
                        next: Next::Close,
                    }),
                }
            }
            version => {
                // SCRAM binds to TLS only on a connection that took it: SSLRequest answered `S` or
                // direct TLS, both only with TLS configured.
                let binding = if ssl_done {
                    self.config.tls.clone().unwrap_or(Binding::None)
                } else {
                    Binding::None
                };
                self.startup_message(version, &mut reader, len, binding, roles, out)
            }
        }
    }

    fn startup_message(
        &mut self,
        requested: u32,
        body: &mut Reader<'_>,
        len: usize,
        binding: Binding,
        roles: &impl Roles,
        out: &mut Vec<u8>,
    ) -> Result<Step, PgError> {
        let (major, minor) = (requested >> 16, requested & 0xffff);
        if major != 3 {
            return fatal(
                out,
                len,
                &PgError::fatal(
                    SqlState::FEATURE_NOT_SUPPORTED,
                    format!(
                        "unsupported frontend protocol {major}.{minor}: server supports 3.0 to 3.2"
                    ),
                ),
            );
        }
        let version = requested.min(PROTOCOL_3_2);
        let (startup, unknown) = match read_startup(version, body) {
            Ok(read) => read,
            Err(error) => return fatal(out, len, &error),
        };
        if requested > PROTOCOL_3_2 || !unknown.is_empty() {
            // NegotiateProtocolVersion, then carry on (`backend_startup.c:877-883`, `:992-1006`).
            let count = i32::try_from(unknown.len())
                .map_err(|_| PgError::violation("invalid length of startup packet"))?;
            wire::message(out, b'v', |out| {
                out.extend_from_slice(&version.to_be_bytes());
                out.extend_from_slice(&count.to_be_bytes());
                for option in &unknown {
                    wire::put_cstr(out, option.as_bytes());
                }
            })?;
        }
        if let Some(error) = self.refused.take() {
            return fatal(out, len, &error);
        }
        let role = roles.role(&startup.user);
        let mechanisms = binding.mechanisms();
        wire::message(out, b'R', |out| {
            out.extend_from_slice(&10i32.to_be_bytes());
            for mechanism in mechanisms {
                wire::put_cstr(out, mechanism.as_bytes());
            }
            out.push(0);
        })?;
        self.phase = Phase::SaslInitial {
            startup,
            role,
            binding,
        };
        Ok(Step {
            consumed: len,
            next: Next::Continue,
        })
    }

    fn sasl_initial(
        &mut self,
        startup: Startup,
        role: Option<Role>,
        binding: Binding,
        input: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<Step, PgError> {
        let (body, len) = match sasl_frame(input, out)? {
            Ok(frame) => frame,
            Err(step) => {
                if step.next != Next::Close {
                    self.phase = Phase::SaslInitial {
                        startup,
                        role,
                        binding,
                    };
                }
                return Ok(step);
            }
        };
        let (mechanism, data) = match read_sasl_initial(body) {
            Ok(parsed) => parsed,
            Err(error) => return fatal(out, len, &error),
        };
        let (secret, superuser, doomed) = match role {
            Some(role) => (role.secret, role.superuser, false),
            None => (
                ScramSecret::mock(startup.user.as_bytes(), &self.config.mock_nonce)?,
                false,
                true,
            ),
        };
        let nonce = scram::base64_encode(&self.seed.scram_nonce);
        match ScramServer::first(secret, doomed, &binding, mechanism, &nonce, data) {
            Ok((exchange, server_first)) => {
                authentication(out, 11, server_first.as_bytes())?;
                self.phase = Phase::SaslFinal {
                    startup,
                    superuser,
                    exchange,
                };
                Ok(Step {
                    consumed: len,
                    next: Next::Continue,
                })
            }
            Err(error) => fatal(out, len, &error),
        }
    }

    fn sasl_final(
        &mut self,
        startup: Startup,
        superuser: bool,
        exchange: ScramServer,
        input: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<Step, PgError> {
        let (body, len) = match sasl_frame(input, out)? {
            Ok(frame) => frame,
            Err(step) => {
                if step.next != Next::Close {
                    self.phase = Phase::SaslFinal {
                        startup,
                        superuser,
                        exchange,
                    };
                }
                return Ok(step);
            }
        };
        let server_final = match exchange.last(body) {
            Ok(Some(server_final)) => server_final,
            Ok(None) => {
                let message = format!(
                    "password authentication failed for user \"{}\"",
                    startup.user
                );
                return fatal(
                    out,
                    len,
                    &PgError::fatal(SqlState::INVALID_PASSWORD, message),
                );
            }
            Err(error) => return fatal(out, len, &error),
        };
        authentication(out, 12, server_final.as_bytes())?;
        authentication(out, 0, &[])?;
        self.report(&startup, superuser, out)?;
        let key_len = if startup.version >= PROTOCOL_3_2 {
            self.seed.cancel_key.len()
        } else {
            4
        };
        let key = self.seed.cancel_key.get(..key_len).unwrap_or_default();
        let process_id = self.seed.process_id;
        wire::message(out, b'K', |out| {
            out.extend_from_slice(&process_id.to_be_bytes());
            out.extend_from_slice(key);
        })?;
        ready_for_query(out, self.tx_status())?;
        self.phase = Phase::Ready {
            startup,
            skip_to_sync: false,
        };
        Ok(Step {
            consumed: len,
            next: Next::Continue,
        })
    }

    /// The 15 settings PostgreSQL 19 reports (`GUC_REPORT` in
    /// `src/backend/utils/misc/guc_parameters.dat`), at their defaults until SET exists.
    fn report(&self, startup: &Startup, superuser: bool, out: &mut Vec<u8>) -> Result<(), PgError> {
        let on_off = |b: bool| if b { "on" } else { "off" };
        let iterations = scram::DEFAULT_ITERATIONS.to_string();
        for (name, value) in [
            ("application_name", startup.application_name.as_str()),
            ("client_encoding", startup.client_encoding),
            ("DateStyle", "ISO, MDY"),
            ("default_transaction_read_only", "off"),
            ("in_hot_standby", "off"),
            ("integer_datetimes", "on"),
            ("IntervalStyle", "postgres"),
            ("is_superuser", on_off(superuser)),
            ("scram_iterations", iterations.as_str()),
            ("search_path", "\"$user\", public"),
            ("server_encoding", "UTF8"),
            ("server_version", SERVER_VERSION),
            ("session_authorization", startup.user.as_str()),
            ("standard_conforming_strings", "on"),
            ("TimeZone", self.config.time_zone.as_str()),
        ] {
            parameter_status(out, name, value)?;
        }
        Ok(())
    }

    fn ready(
        &mut self,
        startup: Startup,
        skip_to_sync: bool,
        input: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<Step, PgError> {
        let (tag, body, len) = match wire::message_frame(input, ready_limit) {
            Ok(Frame::Whole { tag, body, len }) => (tag.unwrap_or_default(), body, len),
            Ok(Frame::Need(total)) => {
                self.phase = Phase::Ready {
                    startup,
                    skip_to_sync,
                };
                return Ok(Step {
                    consumed: 0,
                    next: Next::Need(total),
                });
            }
            Err(error) => return fatal(out, 0, &error),
        };
        let mut skip = skip_to_sync;
        match tag {
            // Terminate and Sync end a skip to Sync; everything else waits it out
            // (`postgres.c:420-443`, `:4839`).
            b'X' => {
                return Ok(Step {
                    consumed: len,
                    next: Next::Close,
                });
            }
            _ if skip && tag != b'S' => {}
            b'Q' => {
                let mut reader = Reader::new(body);
                let sql = match reader.cstr().and_then(|sql| reader.end().map(|()| sql)) {
                    Ok(sql) => sql,
                    Err(error) => return fatal(out, len, &error),
                };
                let answer = utf8(sql)
                    .map_err(|e| PgError::error(e.code, e.message))
                    .and_then(|text| empty_query(sql).map(|empty| (text, empty)));
                match answer {
                    Ok((_, true)) => wire::message(out, b'I', |_| {})?,
                    Ok((text, false)) => match &mut self.statements {
                        Some(statements) => statements.simple_query(text, out)?,
                        None => wire::error_response(out, &not_yet())?,
                    },
                    Err(error) => wire::error_response(out, &error)?,
                }
                ready_for_query(out, self.tx_status())?;
            }
            b'F' => {
                wire::error_response(out, &not_yet())?;
                ready_for_query(out, self.tx_status())?;
            }
            b'P' | b'B' | b'D' | b'E' | b'C' => {
                wire::error_response(out, &not_yet())?;
                skip = true;
            }
            b'S' => {
                ready_for_query(out, self.tx_status())?;
                skip = false;
            }
            // Flush, and copy messages outside a COPY, which PostgreSQL also ignores
            // (`postgres.c:5098-5108`).
            _ => {}
        }
        self.phase = Phase::Ready {
            startup,
            skip_to_sync: skip,
        };
        Ok(Step {
            consumed: len,
            next: Next::Continue,
        })
    }
}

/// Every statement, until Amphora runs PostgreSQL's SQL (docs/design/postgres.md, section 11).
fn not_yet() -> PgError {
    PgError::error(
        SqlState::FEATURE_NOT_SUPPORTED,
        "Amphora does not yet run PostgreSQL statements",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_pg_1_6_empty_queries_are_whitespace_semicolons_and_comments() {
        assert!(empty_query(b"").unwrap());
        assert!(empty_query(b" ;\n\t; -- note\n /* a /* nested */ one */ ;").unwrap());
        assert!(!empty_query(b"-- note\nSELECT 1").unwrap());
        assert!(!empty_query(b"/* x */ SELECT").unwrap());
        assert_eq!(
            empty_query(b"/* /* */").unwrap_err().code,
            SqlState::SYNTAX_ERROR
        );
    }

    #[test]
    fn t_pg_1_3_names_clip_at_namedatalen_on_a_character_boundary() {
        assert_eq!(clip_name(&"a".repeat(80)).len(), 63);
        let name = format!("{}é", "a".repeat(62));
        assert_eq!(clip_name(&name), "a".repeat(62));
        assert_eq!(clean_ascii(b"app\xff\n"), "app\\xff\\x0a");
    }

    #[test]
    fn t_pg_1_3_client_encodings_a_utf8_server_takes() {
        assert_eq!(client_encoding("UTF-8"), Some("UTF8"));
        assert_eq!(client_encoding("unicode"), Some("UTF8"));
        assert_eq!(client_encoding("SQL_ASCII"), Some("SQL_ASCII"));
        assert_eq!(client_encoding("LATIN1"), None);
    }
}
