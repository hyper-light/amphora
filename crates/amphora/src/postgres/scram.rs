//! SCRAM-SHA-256 and SCRAM-SHA-256-PLUS on the server's side, as PostgreSQL 19 runs them
//! (`src/backend/libpq/auth-scram.c`; RFC 5802, RFC 7677). The nonce is the caller's: the core
//! draws no randomness.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use super::wire::{PgError, SqlState};

type HmacSha256 = Hmac<Sha256>;

pub const MECHANISM: &str = "SCRAM-SHA-256";
pub const MECHANISM_PLUS: &str = "SCRAM-SHA-256-PLUS";
/// The one channel-binding type PostgreSQL uses (`auth-scram.c:20-21`, RFC 5929 §4).
pub const TLS_SERVER_END_POINT: &str = "tls-server-end-point";
/// PostgreSQL's default iteration count, the floor RFC 7677 §4 sets
/// (`src/include/common/scram-common.h:50`).
pub const DEFAULT_ITERATIONS: u32 = 4096;
/// The raw bytes of a server nonce before base64 (`scram-common.h:37`).
pub const RAW_NONCE_LEN: usize = 18;
const SALT_LEN: usize = 16;

/// A role's verifier as PostgreSQL keeps it in `pg_authid.rolpassword`:
/// `SCRAM-SHA-256$<iterations>:<salt>$<StoredKey>:<ServerKey>`.
#[derive(Clone, PartialEq, Eq)]
pub struct ScramSecret {
    pub iterations: u32,
    pub salt: Vec<u8>,
    pub stored_key: [u8; 32],
    pub server_key: [u8; 32],
}

impl std::fmt::Debug for ScramSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScramSecret")
            .field("iterations", &self.iterations)
            .finish_non_exhaustive()
    }
}

fn mac(key: &[u8], parts: &[&[u8]]) -> Result<[u8; 32], PgError> {
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|_| PgError::fatal(SqlState::INTERNAL_ERROR, "HMAC refused its key"))?;
    for part in parts {
        mac.update(part);
    }
    Ok(mac.finalize().into_bytes().into())
}

impl ScramSecret {
    /// The verifier for a password already prepared by SASLprep (RFC 4013; the identity on ASCII):
    /// RFC 5802 §3's Hi, ClientKey, StoredKey and ServerKey.
    pub fn derive(password: &[u8], salt: &[u8], iterations: u32) -> Result<Self, PgError> {
        if iterations == 0 {
            return Err(PgError::fatal(
                SqlState::INTERNAL_ERROR,
                "SCRAM iteration count must be at least 1",
            ));
        }
        let mut u = mac(password, &[salt, &1u32.to_be_bytes()])?;
        let mut salted = u;
        for _ in 1..iterations {
            u = mac(password, &[&u])?;
            salted.iter_mut().zip(u.iter()).for_each(|(s, x)| *s ^= x);
        }
        let client_key = mac(&salted, &[b"Client Key"])?;
        Ok(Self {
            iterations,
            salt: salt.to_vec(),
            stored_key: Sha256::digest(client_key).into(),
            server_key: mac(&salted, &[b"Server Key"])?,
        })
    }

    /// Reads PostgreSQL's stored form (`src/backend/libpq/auth-scram.c`, `parse_scram_secret`).
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let rest = text.strip_prefix("SCRAM-SHA-256$")?;
        let (head, keys) = rest.split_once('$')?;
        let (iterations, salt) = head.split_once(':')?;
        let (stored, server) = keys.split_once(':')?;
        Some(Self {
            iterations: iterations.parse().ok().filter(|&i| i > 0)?,
            salt: base64_decode(salt.as_bytes())?,
            stored_key: base64_decode(stored.as_bytes())?.try_into().ok()?,
            server_key: base64_decode(server.as_bytes())?.try_into().ok()?,
        })
    }

    /// A verifier no password matches, for a role that doesn't exist: the exchange runs to its end
    /// and fails as a wrong password does, so the answer doesn't tell which roles exist
    /// (`auth-scram.c`, `scram_mock_salt`).
    pub fn mock(user: &[u8], mock_nonce: &[u8]) -> Result<Self, PgError> {
        let digest: [u8; 32] = Sha256::new()
            .chain_update(user)
            .chain_update(mock_nonce)
            .finalize()
            .into();
        Ok(Self {
            iterations: DEFAULT_ITERATIONS,
            salt: digest.get(..SALT_LEN).unwrap_or_default().to_vec(),
            stored_key: digest,
            server_key: digest,
        })
    }
}

/// What the connection can bind to: nothing without TLS, else the `tls-server-end-point` hash of
/// the server's certificate (RFC 5929 §4.1), which the TLS shell computes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
    None,
    TlsServerEndPoint(Vec<u8>),
}

impl Binding {
    /// The mechanisms AuthenticationSASL offers, strongest first (`auth-scram.c`,
    /// `scram_get_mechanisms`).
    #[must_use]
    pub fn mechanisms(&self) -> &'static [&'static str] {
        match self {
            Self::None => &[MECHANISM],
            Self::TlsServerEndPoint(_) => &[MECHANISM_PLUS, MECHANISM],
        }
    }
}

fn malformed(detail: &str) -> PgError {
    PgError::violation("malformed SCRAM message").with_detail(detail)
}

/// The exchange after the server-first-message, waiting for the client-final-message.
pub struct ScramServer {
    secret: ScramSecret,
    doomed: bool,
    /// The gs2 header and channel-binding data the client's `c=` must carry.
    expected_cbind: Vec<u8>,
    nonce: String,
    auth_prefix: Vec<u8>,
}

impl std::fmt::Debug for ScramServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScramServer")
            .field("doomed", &self.doomed)
            .finish_non_exhaustive()
    }
}

fn printable(b: u8) -> bool {
    (0x21..=0x7e).contains(&b) && b != b','
}

impl ScramServer {
    /// Reads the client-first-message for `mechanism` and answers the server-first-message
    /// (`auth-scram.c`, `read_client_first_message`). `doomed` fails the exchange at its end
    /// whatever the proof, as a missing role does.
    pub fn first(
        secret: ScramSecret,
        doomed: bool,
        binding: &Binding,
        mechanism: &str,
        server_nonce: &str,
        client_first: &[u8],
    ) -> Result<(Self, String), PgError> {
        let plus = match mechanism {
            MECHANISM => false,
            MECHANISM_PLUS if matches!(binding, Binding::TlsServerEndPoint(_)) => true,
            _ => {
                return Err(PgError::fatal(
                    SqlState::PROTOCOL_VIOLATION,
                    "client selected an invalid SASL authentication mechanism",
                ));
            }
        };
        let text = std::str::from_utf8(client_first)
            .map_err(|_| malformed("The message is not UTF-8."))?;
        let (flag, rest) = text
            .split_once(',')
            .ok_or_else(|| malformed("Comma expected."))?;
        let cbind_data: &[u8] = match (flag, binding) {
            ("n", _) if plus => {
                return Err(malformed(
                    "The client selected SCRAM-SHA-256-PLUS, but the SCRAM message does not include channel binding data.",
                ));
            }
            ("n", _) => &[],
            ("y", _) if plus => {
                return Err(malformed(
                    "The client selected SCRAM-SHA-256-PLUS, but the SCRAM message does not include channel binding data.",
                ));
            }
            ("y", Binding::TlsServerEndPoint(_)) => {
                // `auth-scram.c:1019-1024`.
                return Err(PgError::fatal(SqlState::INVALID_AUTHORIZATION_SPECIFICATION, "SCRAM channel binding negotiation error")
                    .with_detail("The client supports SCRAM channel binding but thinks the server does not.  However, this server does support channel binding."));
            }
            ("y", Binding::None) => &[],
            (p, Binding::TlsServerEndPoint(data)) if plus && p.starts_with("p=") => {
                if p != "p=tls-server-end-point" {
                    return Err(PgError::fatal(
                        SqlState::PROTOCOL_VIOLATION,
                        format!(
                            "unsupported SCRAM channel-binding type \"{}\"",
                            p.get(2..).unwrap_or_default()
                        ),
                    ));
                }
                data
            }
            (p, _) if p.starts_with("p=") => {
                return Err(malformed(
                    "The client selected SCRAM-SHA-256 without channel binding, but the SCRAM message includes channel binding data.",
                ));
            }
            _ => return Err(malformed("Unexpected channel-binding flag.")),
        };
        let (authzid, bare) = rest
            .split_once(',')
            .ok_or_else(|| malformed("Comma expected."))?;
        if authzid.starts_with('a') {
            return Err(PgError::fatal(
                SqlState::FEATURE_NOT_SUPPORTED,
                "client uses authorization identity, but it is not supported",
            ));
        }
        if !authzid.is_empty() {
            return Err(malformed("Unexpected attribute in client-first-message."));
        }
        if bare.starts_with("m=") {
            return Err(PgError::fatal(
                SqlState::FEATURE_NOT_SUPPORTED,
                "client requires an unsupported SCRAM extension",
            ));
        }
        let mut attrs = bare.split(',');
        // The user name in the message is ignored: the startup packet's is the one authenticated.
        attrs
            .next()
            .filter(|a| a.starts_with("n="))
            .ok_or_else(|| malformed("Expected attribute \"n\"."))?;
        let client_nonce = attrs
            .next()
            .and_then(|a| a.strip_prefix("r="))
            .ok_or_else(|| malformed("Expected attribute \"r\"."))?;
        if client_nonce.is_empty() || !client_nonce.bytes().all(printable) {
            return Err(PgError::violation(
                "non-printable characters in SCRAM nonce",
            ));
        }
        let mut expected_cbind = format!("{flag},{authzid},").into_bytes();
        expected_cbind.extend_from_slice(cbind_data);
        let nonce = format!("{client_nonce}{server_nonce}");
        let server_first = format!(
            "r={nonce},s={},i={}",
            base64_encode(&secret.salt),
            secret.iterations
        );
        let mut auth_prefix = Vec::with_capacity(
            bare.len()
                .saturating_add(server_first.len())
                .saturating_add(2),
        );
        auth_prefix.extend_from_slice(bare.as_bytes());
        auth_prefix.push(b',');
        auth_prefix.extend_from_slice(server_first.as_bytes());
        auth_prefix.push(b',');
        Ok((
            Self {
                secret,
                doomed,
                expected_cbind,
                nonce,
                auth_prefix,
            },
            server_first,
        ))
    }

    /// Reads the client-final-message and answers the server-final-message, or refuses: a wrong
    /// proof with `Ok(None)` (the caller says `28P01` for its role), a malformed message with its
    /// error (`auth-scram.c`, `read_client_final_message`, `verify_client_proof`).
    pub fn last(self, client_final: &[u8]) -> Result<Option<String>, PgError> {
        let text = std::str::from_utf8(client_final)
            .map_err(|_| malformed("The message is not UTF-8."))?;
        let (without_proof, proof) = text
            .rsplit_once(",p=")
            .ok_or_else(|| malformed("Expected attribute \"p\"."))?;
        let mut attrs = without_proof.split(',');
        let cbind = attrs
            .next()
            .and_then(|a| a.strip_prefix("c="))
            .ok_or_else(|| malformed("Expected attribute \"c\"."))?;
        let cbind = base64_decode(cbind.as_bytes())
            .ok_or_else(|| malformed("Malformed channel binding."))?;
        if cbind != self.expected_cbind {
            return Err(if self.expected_cbind.starts_with(b"p=") {
                PgError::fatal(
                    SqlState::INVALID_AUTHORIZATION_SPECIFICATION,
                    "SCRAM channel binding check failed",
                )
            } else {
                PgError::violation(
                    "unexpected SCRAM channel-binding attribute in client-final-message",
                )
            });
        }
        let nonce = attrs
            .next()
            .and_then(|a| a.strip_prefix("r="))
            .ok_or_else(|| malformed("Expected attribute \"r\"."))?;
        if nonce != self.nonce {
            return Err(
                PgError::violation("invalid SCRAM response").with_detail("Nonce does not match.")
            );
        }
        if attrs.any(|a| a.len() < 2 || a.as_bytes().get(1) != Some(&b'=')) {
            return Err(malformed("Malformed extension in client-final-message."));
        }
        let proof: [u8; 32] = base64_decode(proof.as_bytes())
            .and_then(|p| p.try_into().ok())
            .ok_or_else(|| malformed("Malformed proof in client-final-message."))?;
        let mut auth_message = self.auth_prefix;
        auth_message.extend_from_slice(without_proof.as_bytes());
        let signature = mac(&self.secret.stored_key, &[&auth_message])?;
        let mut client_key = proof;
        client_key
            .iter_mut()
            .zip(signature.iter())
            .for_each(|(k, s)| *k ^= s);
        let stored: [u8; 32] = Sha256::digest(client_key).into();
        let differs = stored
            .iter()
            .zip(self.secret.stored_key.iter())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b));
        if differs != 0 || self.doomed {
            return Ok(None);
        }
        let server_signature = mac(&self.secret.server_key, &[&auth_message])?;
        Ok(Some(format!("v={}", base64_encode(&server_signature))))
    }
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding (RFC 4648 §4), as SCRAM's attributes carry it.
#[must_use]
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3).saturating_mul(4));
    for chunk in bytes.chunks(3) {
        let b0 = chunk.first().copied().unwrap_or(0);
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let sextets = [
            b0 >> 2,
            ((b0 << 4) | (b1 >> 4)) & 63,
            ((b1 << 2) | (b2 >> 6)) & 63,
            b2 & 63,
        ];
        for (i, sextet) in sextets.into_iter().enumerate() {
            let symbol = if i <= chunk.len() {
                ALPHABET.get(usize::from(sextet)).copied().unwrap_or(b'=')
            } else {
                b'='
            };
            out.push(char::from(symbol));
        }
    }
    out
}

fn sextet(c: u8) -> Option<u8> {
    ALPHABET
        .iter()
        .position(|&a| a == c)
        .and_then(|p| u8::try_from(p).ok())
}

/// Strict standard base64: padded to a multiple of four, no stray characters, no bits past the end.
#[must_use]
pub fn base64_decode(text: &[u8]) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(4) {
        return None;
    }
    let (quads, _) = text.as_chunks::<4>();
    let mut out = Vec::with_capacity(quads.len().saturating_mul(3));
    let mut quads = quads.iter().peekable();
    while let Some(&[a, b, c, d]) = quads.next() {
        let last = quads.peek().is_none();
        let (a, b) = (sextet(a)?, sextet(b)?);
        out.push((a << 2) | (b >> 4));
        match (c, d) {
            (b'=', b'=') if last && b & 0x0f == 0 => {}
            (c, b'=') if last => {
                let c = sextet(c)?;
                if c & 0x03 != 0 {
                    return None;
                }
                out.push((b << 4) | (c >> 2));
            }
            (c, d) => {
                let (c, d) = (sextet(c)?, sextet(d)?);
                out.push((b << 4) | (c >> 2));
                out.push((c << 6) | d);
            }
        }
    }
    Some(out)
}
