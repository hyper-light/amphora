//! The PostgreSQL listener: the shell that drives [`Session`]s over hyper-rt's sockets
//! (docs/design/postgres.md, decision 2; unit 2 of its section 11). Each connection is a task on the
//! event loop that wakes only when its socket is readable or writable. Each session's state and
//! the frame it holds are charged to the process's memory budget ([`Use::Sessions`]). A session
//! the server has no room for is refused `53300` after its startup packet, as PostgreSQL refuses
//! one. A frame the budget can't hold is refused `53200`. A session's statements run on a connection of
//! its own to the listener's database ([`Database`]), opened once its role is authenticated.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use anyhow::{Context, anyhow};
use hyper_rt::RtError;
use hyper_rt::futures::sleep;
use hyper_rt::machine::calibration::{Calibration, Policy};
use hyper_rt::runtime::{LocalRuntime, RuntimeConfig};
use hyper_rt::tcp::{TcpListener, TcpStream};

use super::execute::{Database, Executor, SessionRole};
use super::scram::{self, ScramSecret};
use super::session::{Config, Next, Role, Roles, Seed, Session};
use super::wire::{self, PgError, SqlState};
use crate::memory::{Budget, Charge, StorageMemory, Use};

const TARGET: &str = "amphora.postgres.listener";

/// PostgreSQL's `max_connections` before it is set (`src/backend/utils/misc/guc_parameters.dat:1978-1981`).
pub const MAX_CONNECTIONS_DEFAULT: usize = 100;

/// The runtime's probes' budget, the one hyper-rt's own callers give
/// (`vendor/hyper-raft/crates/hyper-rt/benches/calibration_drift.rs:7`).
const PROBE_BUDGET: Duration = Duration::from_millis(40);

pub const USAGE: &str = "usage: amphora postgres --listen <addr> --roles <file> --database <file> [--max-connections <n>]\n";

/// The roles that may log in, given when the listener starts until the catalog keeps them: one a
/// line, its name, its verifier in PostgreSQL's stored form (`pg_authid.rolpassword`,
/// `SCRAM-SHA-256$<iterations>:<salt>$<StoredKey>:<ServerKey>`), and `superuser` for one that is.
#[derive(Debug, Default)]
pub struct StoredRoles(BTreeMap<String, Role>);

impl StoredRoles {
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let mut roles = BTreeMap::new();
        for (at, line) in text.lines().enumerate() {
            let line_no = at.saturating_add(1);
            let mut fields = line.split_whitespace();
            let Some(name) = fields.next() else {
                continue;
            };
            let secret = fields
                .next()
                .and_then(ScramSecret::parse)
                .ok_or_else(|| {
                    anyhow!("line {line_no}: {name}'s verifier is not PostgreSQL's SCRAM-SHA-256 stored form")
                })?;
            let superuser = match fields.next() {
                None => false,
                Some("superuser") => true,
                Some(other) => return Err(anyhow!("line {line_no}: unknown attribute {other}")),
            };
            if let Some(extra) = fields.next() {
                return Err(anyhow!("line {line_no}: unexpected {extra}"));
            }
            roles.insert(name.to_owned(), Role { secret, superuser });
        }
        Ok(Self(roles))
    }
}

impl Roles for StoredRoles {
    fn role(&self, user: &str) -> Option<Role> {
        self.0.get(user).cloned()
    }
}

/// What the listener's sessions share: their configuration, the roles, the database, the budget
/// they're charged to, and how many of them may live at once.
pub struct Listener {
    config: Config,
    roles: Rc<StoredRoles>,
    database: Rc<Database>,
    budget: Arc<Budget>,
    max_connections: usize,
    live: Cell<usize>,
}

/// Why a session ended, for its log line.
enum Ended {
    Closed,
    PeerClosed { partial: bool },
    Logged(PgError),
    Refused(amphora_api::Refusal),
    Cancel { process_id: i32 },
    Tls,
    Unwritable(PgError),
    Socket(RtError),
    NoRandomness(getrandom::Error),
}

/// A live session's place among `max_connections`, given back when the session ends.
struct Live<'a>(&'a Cell<usize>);

impl Drop for Live<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get().saturating_sub(1));
    }
}

impl Listener {
    /// A listener for `roles` on `database`, its sessions charged to `budget`, at most
    /// `max_connections` of them live. TLS is a later unit: SSLRequest is answered `N`.
    pub fn new(
        roles: StoredRoles,
        database: Database,
        budget: Arc<Budget>,
        max_connections: usize,
    ) -> Result<Self, getrandom::Error> {
        let mut mock_nonce = [0; 32];
        getrandom::fill(&mut mock_nonce)?;
        Ok(Self {
            config: Config {
                tls: None,
                time_zone: std::env::var("TZ").unwrap_or_else(|_| "UTC".to_owned()),
                mock_nonce,
            },
            roles: Rc::new(roles),
            database: Rc::new(database),
            budget,
            max_connections,
            live: Cell::new(0),
        })
    }

    /// Accepts connections until accepting fails, each session a task of its own.
    pub async fn serve(self: Rc<Self>, tcp: TcpListener) -> Result<(), RtError> {
        loop {
            let stream = tcp.accept().await?;
            let listener = Rc::clone(&self);
            if let Err(error) = hyper_rt::futures::spawn_detached(listener.run(stream)) {
                tracing::warn!(target: TARGET, event = "unaccepted", %error, "a connection past the runtime's task room closed unanswered");
            }
        }
    }

    async fn run(self: Rc<Self>, stream: TcpStream) {
        let peer = stream.peer_addr().ok();
        match self.session(&stream).await {
            Ended::Closed | Ended::PeerClosed { partial: false } => {}
            Ended::PeerClosed { partial: true } => {
                tracing::info!(target: TARGET, event = "incomplete", ?peer, "the client closed mid-message");
            }
            Ended::Logged(error) => {
                tracing::info!(target: TARGET, event = "violation", ?peer, code = error.code.code(), message = %error.message);
            }
            Ended::Refused(refusal) => {
                tracing::warn!(target: TARGET, event = "out_of_memory", ?peer, %refusal);
            }
            Ended::Cancel { process_id } => {
                tracing::info!(target: TARGET, event = "cancel_unserved", ?peer, process_id, "CancelRequest is a later unit");
            }
            Ended::Tls => {
                tracing::warn!(target: TARGET, event = "tls_unconfigured", ?peer);
            }
            Ended::Unwritable(error) => {
                tracing::warn!(target: TARGET, event = "unwritable", ?peer, message = %error.message);
            }
            Ended::Socket(error) => {
                tracing::info!(target: TARGET, event = "socket", ?peer, %error);
            }
            Ended::NoRandomness(error) => {
                tracing::warn!(target: TARGET, event = "no_randomness", ?peer, %error);
            }
        }
    }

    async fn session(&self, stream: &TcpStream) -> Ended {
        let seed = match seed() {
            Ok(seed) => seed,
            Err(error) => return Ended::NoRandomness(error),
        };
        let (database, time_zone) = (Rc::clone(&self.database), self.config.time_zone.clone());
        let process_id = seed.process_id;
        let superusers = Rc::clone(&self.roles);
        let budget = Arc::clone(&self.budget);
        let mut session =
            Session::new(self.config.clone(), seed).with_database(Box::new(move |startup| {
                let role = SessionRole {
                    name: &startup.user,
                    superuser: superusers
                        .role(&startup.user)
                        .is_some_and(|role| role.superuser),
                };
                let executor = database.session(role, &time_zone, process_id)?;
                Ok(Box::new(executor.charged_to(budget)))
            }));
        let live = self.live.get();
        let admitted = if live < self.max_connections {
            self.budget.take(Use::Sessions, session_bytes()).ok()
        } else {
            None
        };
        let _live = admitted.as_ref().map(|_| {
            self.live.set(live.saturating_add(1));
            Live(&self.live)
        });
        if admitted.is_none() {
            // Until it is answered, a refused session holds only its startup packet, which the
            // core bounds at `MAX_STARTUP_PACKET_LENGTH` before asking for it.
            session.refuse(PgError::fatal(
                SqlState::TOO_MANY_CONNECTIONS,
                "sorry, too many clients already",
            ));
        }
        let mut input = Vec::new();
        let mut out = Vec::new();
        let mut held: Option<Charge> = None;
        let mut waited = false;
        loop {
            let step = if waited {
                session.resume(&mut out)
            } else {
                session.step(&input, &*self.roles, &mut out)
            };
            waited = false;
            let step = match step {
                Ok(step) => step,
                Err(error) => return Ended::Unwritable(error),
            };
            input.drain(..step.consumed.min(input.len()));
            match step.next {
                Next::Continue => {}
                Next::Flush => {
                    if let Err(error) = flush(stream, &mut out).await {
                        return Ended::Socket(error);
                    }
                }
                // The end of the transaction it waits for wakes this task (docs/design/postgres.md
                // section 8); nothing it held is another's, so its other sessions go on meanwhile.
                Next::Wait => {
                    if let Err(error) = waited_out(&mut session).await {
                        return Ended::Socket(error);
                    }
                    waited = true;
                }
                Next::Need(total) => {
                    if let Err(error) = flush(stream, &mut out).await {
                        return Ended::Socket(error);
                    }
                    if admitted.is_some() {
                        drop(held.take());
                        match self
                            .budget
                            .take(Use::Sessions, u64::try_from(total).unwrap_or(u64::MAX))
                        {
                            Ok(charge) => held = Some(charge),
                            Err(refusal) => {
                                let error =
                                    PgError::fatal(SqlState::OUT_OF_MEMORY, "out of memory")
                                        .with_detail(format!("Failed on request of size {total}."));
                                if let Err(error) = wire::error_response(&mut out, &error) {
                                    return Ended::Unwritable(error);
                                }
                                return match flush(stream, &mut out).await {
                                    Ok(()) => Ended::Refused(refusal),
                                    Err(error) => Ended::Socket(error),
                                };
                            }
                        }
                    }
                    match fill(stream, &mut input, total).await {
                        Ok(true) => {}
                        Ok(false) => {
                            return Ended::PeerClosed {
                                partial: !input.is_empty(),
                            };
                        }
                        Err(error) => return Ended::Socket(error),
                    }
                }
                Next::StartTls | Next::StartDirectTls => return Ended::Tls,
                Next::Cancel { process_id, .. } => return Ended::Cancel { process_id },
                Next::Close => {
                    return match flush(stream, &mut out).await {
                        Ok(()) => Ended::Closed,
                        Err(error) => Ended::Socket(error),
                    };
                }
                Next::CloseLogged(error) => return Ended::Logged(error),
            }
        }
    }
}

/// Waits until the statement `session` stopped at can go on: its holder's end, or the first of 19's two
/// deadlines that refuses it. The deadlock check comes before the lock timeout, as in `ProcSleep`.
async fn waited_out(session: &mut Session) -> Result<(), RtError> {
    let Some(limits) = session.wait_limits() else {
        return Ok(());
    };
    let mut deadlock = std::pin::pin!(sleep(nanos(limits.deadlock)));
    let mut checked = false;
    let mut lock = std::pin::pin!(sleep(limits.lock.map_or(u64::MAX, nanos)));
    std::future::poll_fn(|cx| {
        if session.poll_wait(cx).is_ready() {
            return Poll::Ready(Ok(()));
        }
        if !checked && let Poll::Ready(woken) = deadlock.as_mut().poll(cx) {
            woken?;
            checked = true;
            if session.check_deadlock() {
                return Poll::Ready(Ok(()));
            }
        }
        if limits.lock.is_some()
            && let Poll::Ready(woken) = lock.as_mut().poll(cx)
        {
            woken?;
            session.lock_timed_out();
            return Poll::Ready(Ok(()));
        }
        Poll::Pending
    })
    .await
}

fn nanos(span: Duration) -> u64 {
    u64::try_from(span.as_nanos()).unwrap_or(u64::MAX)
}

/// What a session holds before it reads: its state machine, its executor and its socket.
fn session_bytes() -> u64 {
    let bytes = std::mem::size_of::<Session>()
        .saturating_add(std::mem::size_of::<Executor>())
        .saturating_add(std::mem::size_of::<TcpStream>());
    u64::try_from(bytes).unwrap_or(u64::MAX)
}

fn seed() -> Result<Seed, getrandom::Error> {
    let mut process_id = [0; 4];
    let mut seed = Seed {
        process_id: 0,
        cancel_key: [0; 32],
        scram_nonce: [0; scram::RAW_NONCE_LEN],
    };
    getrandom::fill(&mut process_id)?;
    getrandom::fill(&mut seed.cancel_key)?;
    getrandom::fill(&mut seed.scram_nonce)?;
    // A process id is positive, as a PostgreSQL backend's pid is.
    seed.process_id = i32::from_be_bytes(process_id) & i32::MAX;
    Ok(seed)
}

async fn flush(stream: &TcpStream, out: &mut Vec<u8>) -> Result<(), RtError> {
    if !out.is_empty() {
        stream.write_all(out).await?;
        out.clear();
    }
    Ok(())
}

/// Reads until `input` holds `total` bytes: `false` when the peer closed first.
async fn fill(stream: &TcpStream, input: &mut Vec<u8>, total: usize) -> Result<bool, RtError> {
    while input.len() < total {
        let filled = input.len();
        input.resize(total, 0);
        let read = stream
            .read(input.get_mut(filled..).unwrap_or_default())
            .await;
        let n = match read {
            Ok(n) => n,
            Err(error) => {
                input.truncate(filled);
                return Err(error);
            }
        };
        input.truncate(filled.saturating_add(n));
        if n == 0 {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `amphora postgres`: the listener on one shard until it fails. `None` for any other command.
pub fn command(args: &[String], ready: impl FnOnce(SocketAddr) + 'static) -> Option<crate::Reply> {
    let (first, rest) = args.split_first()?;
    if first != "postgres" {
        return None;
    }
    Some(match listen(rest, ready) {
        Ok(()) => crate::Reply {
            text: String::new(),
            to_stderr: false,
            status: 0,
        },
        Err(error) => crate::Reply {
            text: format!("amphora: {error:#}\n"),
            to_stderr: true,
            status: 2,
        },
    })
}

fn listen(rest: &[String], ready: impl FnOnce(SocketAddr) + 'static) -> anyhow::Result<()> {
    let flag = |name: &str| {
        rest.iter()
            .position(|a| a == name)
            .and_then(|at| rest.get(at.saturating_add(1)))
    };
    let addr: SocketAddr = flag("--listen")
        .ok_or_else(|| anyhow!("--listen is required\n{USAGE}"))?
        .parse()?;
    let roles_file = flag("--roles").ok_or_else(|| anyhow!("--roles is required\n{USAGE}"))?;
    let path = flag("--database").ok_or_else(|| anyhow!("--database is required\n{USAGE}"))?;
    let roles = StoredRoles::parse(
        &std::fs::read_to_string(roles_file).with_context(|| format!("reading {roles_file}"))?,
    )?;
    let max_connections = match flag("--max-connections") {
        Some(value) => value.parse()?,
        None => MAX_CONNECTIONS_DEFAULT,
    };
    // PostgreSQL's backlog is twice its connections (`src/backend/libpq/pqcomm.c:644`). The task
    // room is the live sessions, as many refused ones answering 53300 as they finish, and the
    // accept loop.
    let backlog = u32::try_from(max_connections.saturating_mul(2)).unwrap_or(u32::MAX);
    let tasks = max_connections.saturating_mul(2).saturating_add(1);
    let budget = crate::memory::process();
    let database = Database::open(path, Arc::new(StorageMemory(Arc::clone(&budget))))
        .with_context(|| format!("opening {path}"))?;
    let listener = Rc::new(Listener::new(roles, database, budget, max_connections)?);
    let calibration = Calibration::measure(PROBE_BUDGET, 0)?;
    let constants = calibration
        .constants(&Policy {
            reserved_cores: 0,
            lateness_tolerance_ns: None,
            latency_objective_ns: None,
        })
        .map_err(|unmet| anyhow!("the machine can't keep the runtime's timers: {unmet:?}"))?;
    let mut config = RuntimeConfig::from_calibration(&calibration, &constants, tasks, tasks);
    config.shards = 1;
    config.pin = false;
    config.cores.clear();
    let mut runtime = LocalRuntime::new(&config)?;
    runtime.block_on(async move {
        let tcp = TcpListener::bind(addr, backlog)?;
        ready(tcp.local_addr()?);
        listener.serve(tcp).await
    })??;
    Ok(())
}
