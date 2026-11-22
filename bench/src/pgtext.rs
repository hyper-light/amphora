//! The scale bench's Postgres client: the simple query protocol alone (Postgres's docs, "Frontend/
//! Backend Protocol", "Simple Query"), over a `std::net::TcpStream` whose read and write timeouts
//! are the run's `--deadline-s`. A request to a node that has stopped answering gives up after it,
//! as TiDB's MySQL client (its `read_timeout`) and Amphora's (`amphora::serve::Client`) do; the
//! timeout is only that give-up, never a wake (lane scaleout-harness-9, G13). rust-postgres's sync
//! client has no read timeout and doesn't expose its socket, so the bench speaks the protocol
//! itself: one Query message a request, one round trip, its statements as text, as Amphora's
//! text-only protocol takes them. Trust authentication only: the bench's CockroachDB runs
//! `--insecure` (servers/scale/cockroach.sh).

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// The longest backend message taken: Postgres's own bound on one allocation (`MaxAllocSize`,
/// 1 GB less one byte, src/include/utils/memutils.h), a format value.
const MESSAGE_MAX: usize = 0x3fff_ffff;
/// The protocol's version 3.0 in a startup message (the docs' "StartupMessage"), a format value.
const PROTOCOL_3_0: i32 = 196_608;
/// The port a DSN without one means (the docs' "Connection Strings", `port`), a format value.
const DEFAULT_PORT: u16 = 5432;

pub struct Client {
    s: TcpStream,
}

/// A row's columns as the server wrote them in text, NULL as none.
pub type Row = Vec<Option<String>>;

impl Client {
    /// Connects to the node `dsn` names (`host=.. port=.. user=.. dbname=..`), waiting at most
    /// `deadline` for the connection and for each read and write after.
    pub fn connect(dsn: &str, deadline: Duration) -> anyhow::Result<Self> {
        let cfg: postgres::Config = dsn.parse()?;
        let host = match cfg.get_hosts().first() {
            Some(postgres::config::Host::Tcp(h)) => h.clone(),
            _ => anyhow::bail!("{dsn}: no TCP host"),
        };
        let port = cfg.get_ports().first().copied().unwrap_or(DEFAULT_PORT);
        let user = cfg
            .get_user()
            .ok_or_else(|| anyhow::anyhow!("{dsn}: no user"))?;
        let addr = (host.as_str(), port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| anyhow::anyhow!("{dsn}: {host} resolves to no address"))?;
        let s = TcpStream::connect_timeout(&addr, deadline)?;
        s.set_read_timeout(Some(deadline))?;
        s.set_write_timeout(Some(deadline))?;
        s.set_nodelay(true)?;
        let mut c = Self { s };
        let mut body = PROTOCOL_3_0.to_be_bytes().to_vec();
        for (k, v) in [
            ("user", user),
            ("database", cfg.get_dbname().unwrap_or(user)),
        ] {
            cstr(&mut body, k)?;
            cstr(&mut body, v)?;
        }
        body.push(0);
        c.send(None, &body)?;
        loop {
            let (tag, m) = c.recv()?;
            match tag {
                b'R' => {
                    let code = Reader(&m).i32()?;
                    anyhow::ensure!(
                        code == 0,
                        "{dsn}: the server asked for authentication {code}; the bench's client takes trust only"
                    );
                }
                b'S' | b'K' | b'N' => {}
                b'E' => anyhow::bail!("{dsn}: {}", error(&m)),
                b'Z' => return Ok(c),
                t => anyhow::bail!("{dsn}: message {:?} at startup", char::from(t)),
            }
        }
    }

    /// `sql` (one statement or several) in one Query message: every row it answers, in text.
    pub fn simple_query(&mut self, sql: &str) -> anyhow::Result<Vec<Row>> {
        let mut body = Vec::with_capacity(sql.len().saturating_add(1));
        cstr(&mut body, sql)?;
        self.send(Some(b'Q'), &body)?;
        let (mut rows, mut failed) = (Vec::new(), None);
        loop {
            let (tag, m) = self.recv()?;
            match tag {
                b'D' => rows.push(row(&m)?),
                b'E' => failed = Some(error(&m)),
                b'Z' => {
                    return match failed {
                        Some(e) => Err(anyhow::anyhow!("{sql}: {e}")),
                        None => Ok(rows),
                    };
                }
                b'T' | b'C' | b'I' | b'N' | b'S' => {}
                t => anyhow::bail!("{sql}: message {:?} in its answer", char::from(t)),
            }
        }
    }

    fn send(&mut self, tag: Option<u8>, body: &[u8]) -> anyhow::Result<()> {
        let len = i32::try_from(body.len().saturating_add(4))?;
        let mut frame = Vec::with_capacity(body.len().saturating_add(5));
        frame.extend(tag);
        frame.extend_from_slice(&len.to_be_bytes());
        frame.extend_from_slice(body);
        self.s.write_all(&frame).map_err(gave_up)
    }

    fn recv(&mut self) -> anyhow::Result<(u8, Vec<u8>)> {
        let mut head = [0u8; 5];
        self.s.read_exact(&mut head).map_err(gave_up)?;
        let [tag, a, b, c, d] = head;
        let len = usize::try_from(i32::from_be_bytes([a, b, c, d]))?
            .checked_sub(4)
            .ok_or_else(|| anyhow::anyhow!("a message shorter than its own length"))?;
        anyhow::ensure!(
            len <= MESSAGE_MAX,
            "a message of {len} bytes, past {MESSAGE_MAX}"
        );
        let mut m = vec![0; len];
        self.s.read_exact(&mut m).map_err(gave_up)?;
        Ok((tag, m))
    }
}

/// An I/O error, named as the give-up when it's the socket's timeout.
fn gave_up(e: std::io::Error) -> anyhow::Error {
    match e.kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => {
            anyhow::anyhow!("no answer within --deadline-s: {e}")
        }
        _ => e.into(),
    }
}

/// `s` and its terminating NUL; refused if `s` holds one.
fn cstr(out: &mut Vec<u8>, s: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!s.as_bytes().contains(&0), "a NUL byte in {s:?}");
    out.extend_from_slice(s.as_bytes());
    out.push(0);
    Ok(())
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> anyhow::Result<&'a [u8]> {
        let (head, rest) = self
            .0
            .split_at_checked(n)
            .ok_or_else(|| anyhow::anyhow!("a message cut short"))?;
        self.0 = rest;
        Ok(head)
    }
    fn i32(&mut self) -> anyhow::Result<i32> {
        let b: [u8; 4] = self.take(4)?.try_into()?;
        Ok(i32::from_be_bytes(b))
    }
    fn u16(&mut self) -> anyhow::Result<u16> {
        let b: [u8; 2] = self.take(2)?.try_into()?;
        Ok(u16::from_be_bytes(b))
    }
}

/// A DataRow's columns: a count, then each a length (-1 for NULL) and its bytes.
fn row(m: &[u8]) -> anyhow::Result<Row> {
    let mut r = Reader(m);
    let n = r.u16()?;
    (0..n)
        .map(|_| -> anyhow::Result<Option<String>> {
            // A negative length is NULL.
            Ok(match usize::try_from(r.i32()?) {
                Ok(len) => Some(String::from_utf8(r.take(len)?.to_vec())?),
                Err(_) => None,
            })
        })
        .collect()
}

/// An ErrorResponse's severity, code and message (the docs' "Error and Notice Message Fields").
fn error(m: &[u8]) -> String {
    let (mut severity, mut code, mut message) = ("", "", "");
    for field in m.split(|&b| b == 0) {
        let Some((&kind, text)) = field.split_first() else {
            continue;
        };
        let text = std::str::from_utf8(text).unwrap_or("");
        match kind {
            b'S' => severity = text,
            b'C' => code = text,
            b'M' => message = text,
            _ => {}
        }
    }
    format!("{severity} {code}: {message}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lane scaleout-harness-9 (G13): the Postgres client's give-up is its socket's read timeout.
    /// Do: a listener that takes the connection and never answers; connect with a deadline.
    /// Expect: the connect refused as a give-up, not a hang.
    #[test]
    fn scaleout_harness_9_postgres_client_gives_up_at_its_deadline() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let held = std::thread::spawn(move || l.accept().map(|(s, _)| s));
        let dsn = format!("host=127.0.0.1 port={port} user=root dbname=defaultdb sslmode=disable");
        let e = Client::connect(&dsn, Duration::from_millis(50))
            .err()
            .unwrap()
            .to_string();
        assert!(e.contains("no answer within --deadline-s"), "{e}");
        drop(held.join());
    }

    /// Lane scaleout-harness-9 (G13): a DataRow's text columns and NULLs, and an ErrorResponse's
    /// fields. Do: decode a row of "7" and NULL, a cut-short row and an error. Expect: the values,
    /// a refusal, and the error's severity, code and message.
    #[test]
    fn scaleout_harness_9_postgres_messages_decode() {
        let mut m = 2u16.to_be_bytes().to_vec();
        m.extend_from_slice(&1i32.to_be_bytes());
        m.push(b'7');
        m.extend_from_slice(&(-1i32).to_be_bytes());
        assert_eq!(row(&m).unwrap(), vec![Some("7".to_owned()), None]);
        assert!(row(&m[..m.len() - 1]).is_err());
        let e = b"SERROR\0C40001\0Mrestart transaction\0\0";
        assert_eq!(error(e), "ERROR 40001: restart transaction");
    }
}
