//! The single owner of a replicated Database. Hosts carry authenticated wire records only.
//!
//! One command at a time crosses ReadIndex before evaluation. A captured answer stays here until
//! hyper-durable releases its matching durable application; no caller sees speculative state.

use super::*;
use std::collections::VecDeque;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use api::replication::{Outbound, ReplicaConfig, ReplicaStatus};
use hyper_block::file::{CachingRequest, DeviceFile};
use hyper_durable::{Bytes, GroupStore, Output, Replica, StateMachine};
use hyper_raft::proto::{ConfState, Message};
use hyper_raft::wire::Record;

// Admission limits, not throughput tuning: overflow refuses before taking ownership. The queue
// charges a full API request for an opaque FnOnce, whose captures the API cannot inspect.
const COMMANDS: usize = 64;
const QUEUED_BYTES: usize = 512 << 20;
const OUTGOING_BYTES: usize = 512 << 20;
const OUTGOING_MESSAGES: usize = 4096;
const MEMBERS: usize = 64;
const WIRE_BYTES: usize = crate::replication::SNAPSHOT_BYTES.saturating_add(2 << 20);
const MESSAGE_BYTES: usize = crate::replication::COMMAND_BYTES.saturating_add(2 << 20);
// Ticks are the pinned core's documented tick election mode (raft.rs, Elections::Ticks). Its
// twenty-tick randomized election and two-tick heartbeat use this owner clock, never host votes.
const TICK: Duration = Duration::from_millis(10);
const POLL: Duration = Duration::from_millis(1);
const DRIVE_QUANTUM: usize = 32;

type Durable = Replica<GroupStore<DeviceFile>, crate::replication::Machine, Bytes>;
type Response<T> = mpsc::SyncSender<Result<T, Refusal>>;

/// A bounded ingress and egress handle. Clones share one product-owned Raft actor.
#[derive(Clone)]
pub struct ReplicaHandle {
    sender: mpsc::SyncSender<Envelope>,
    shared: Arc<Control>,
}
#[derive(Clone)]
pub(super) struct Client {
    handle: ReplicaHandle,
}
impl Client {
    pub(super) fn submit(
        &self,
        namespace: String,
        operation: Operation,
        until: Instant,
    ) -> Result<Answer, Refusal> {
        self.handle.submit(namespace, operation, until)
    }
    pub(super) fn handle(&self) -> ReplicaHandle {
        self.handle.clone()
    }
}
struct Control {
    admitted: AtomicUsize,
    outgoing: Mutex<Outgoing>,
    status: Mutex<Result<ReplicaStatus, Refusal>>,
}
#[derive(Default)]
struct Outgoing {
    bytes: usize,
    messages: VecDeque<Outbound>,
}
struct Reservation {
    control: Arc<Control>,
    bytes: usize,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.control
            .admitted
            .fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
struct Envelope {
    command: Input,
    _reservation: Reservation,
}
enum Input {
    Submit {
        namespace: String,
        operation: Operation,
        until: Instant,
        response: Response<Answer>,
    },
    Receive {
        from: u64,
        bytes: Vec<u8>,
        until: Instant,
        response: Response<()>,
    },
}
struct Pending {
    context: Vec<u8>,
    term: u64,
    until: Instant,
    namespace: String,
    response: Option<Response<Answer>>,
    phase: Phase,
    // Keep admission charged while its command is in flight, not just while queued.
    _reservation: Reservation,
}
enum Phase {
    Barrier(Operation),
    Proposed(Answer),
}
struct Actor {
    replica: Durable,
    // Keep the log owner and its exclusive lease until the replica has drained/dropped.
    _log: hyper_log::Log<DeviceFile>,
    _log_owner: File,
    // Drop after the log writer has drained; reopen cannot race its final disk completion.
    _lease: Arc<File>,
    changed: Arc<Changed>,
    control: Arc<Control>,
    pending: Option<Pending>,
    // At most COMMANDS admitted operations wait behind the sole prepared transaction.
    waiting: VecDeque<Envelope>,
    serial: u64,
    started: Instant,
    ticked: Instant,
    published: u64,
}

fn nonleader(replica: &Durable) -> Refusal {
    let leader = replica.leader();
    Refusal::NotLeader {
        leader: if leader == 0 { None } else { Some(leader) },
    }
}
fn fenced(reason: impl std::fmt::Display) -> Refusal {
    Refusal::Fenced(reason.to_string())
}
fn unknown() -> Refusal {
    Refusal::UnknownOutcome
}
fn raft_error(error: hyper_durable::ReplicaError) -> Refusal {
    match error {
        hyper_durable::ReplicaError::Fenced(cause) => fenced(cause),
        other => unavailable(other),
    }
}
fn answer<T>(response: Response<T>, result: Result<T, Refusal>) {
    // Failure means the deadline-expired caller relinquished its reply. Committed work remains
    // applied and published; dropping this private response never drops a database update.
    let _ = response.send(result);
}

impl ReplicaHandle {
    fn enqueue(&self, command: Input, bytes: usize) -> Result<(), Refusal> {
        let mut charged = self.shared.admitted.load(Ordering::Acquire);
        let mut reserved = false;
        for _ in 0..COMMANDS {
            let next = charged.checked_add(bytes).ok_or(Refusal::Full {
                limit: QUEUED_BYTES,
            })?;
            api::bound("replica command queue bytes", next, QUEUED_BYTES)?;
            match self.shared.admitted.compare_exchange_weak(
                charged,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    reserved = true;
                    break;
                }
                Err(now) => charged = now,
            }
        }
        if !reserved {
            return Err(Refusal::Full { limit: COMMANDS });
        }
        let envelope = Envelope {
            command,
            _reservation: Reservation {
                control: Arc::clone(&self.shared),
                bytes,
            },
        };
        self.sender.try_send(envelope).map_err(|error| match error {
            mpsc::TrySendError::Full(_) => Refusal::Full { limit: COMMANDS },
            mpsc::TrySendError::Disconnected(_) => {
                fenced("replica owner ended; reopen from durable state")
            }
        })
    }

    pub(super) fn submit(
        &self,
        namespace: String,
        operation: Operation,
        until: Instant,
    ) -> Result<Answer, Refusal> {
        if Instant::now() >= until {
            return Err(Refusal::Timeout);
        }
        let (response, receiver) = mpsc::sync_channel(1);
        self.enqueue(
            Input::Submit {
                namespace,
                operation,
                until,
                response,
            },
            api::REQUEST_MAX,
        )?;
        receiver
            .recv_timeout(until.saturating_duration_since(Instant::now()))
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => unknown(),
                mpsc::RecvTimeoutError::Disconnected => unknown(),
            })?
    }

    /// `from` must be the identity authenticated by the host's transport, not a wire claim.
    pub fn receive(&self, from: u64, bytes: Vec<u8>, until: Instant) -> Result<(), Refusal> {
        if Instant::now() >= until {
            return Err(Refusal::Timeout);
        }
        api::bound("replica wire record", bytes.len(), WIRE_BYTES)?;
        let charge = bytes.len();
        let (response, receiver) = mpsc::sync_channel(1);
        self.enqueue(
            Input::Receive {
                from,
                bytes,
                until,
                response,
            },
            charge,
        )?;
        receiver
            .recv_timeout(until.saturating_duration_since(Instant::now()))
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => Refusal::Timeout,
                mpsc::RecvTimeoutError::Disconnected => fenced("replica owner ended"),
            })?
    }

    /// Takes at most `limit` messages. An undrained host eventually fences explicitly at the
    /// egress bound; the actor never acknowledges a send the host has not taken.
    pub fn drain(&self, limit: usize) -> Result<Vec<Outbound>, Refusal> {
        api::bound("replica drain count", limit, OUTGOING_MESSAGES)?;
        let mut outgoing = locked(&self.shared.outgoing)?;
        let mut messages = Vec::with_capacity(limit.min(outgoing.messages.len()));
        for _ in 0..limit {
            let Some(message) = outgoing.messages.pop_front() else {
                break;
            };
            outgoing.bytes = outgoing
                .bytes
                .checked_sub(message.bytes.len())
                .ok_or_else(|| fenced("outgoing accounting underflow"))?;
            messages.push(message);
        }
        Ok(messages)
    }

    pub fn status(&self) -> Result<ReplicaStatus, Refusal> {
        locked(&self.shared.status)?.clone()
    }
}

pub(super) fn start(
    path: &Path,
    log_path: &Path,
    config: ReplicaConfig,
    changed: Arc<Changed>,
    lease: Arc<File>,
    until: Instant,
) -> Result<Client, Refusal> {
    if Instant::now() >= until {
        return Err(Refusal::Timeout);
    }
    validate(&config)?;
    let log_owner = exclusive_file(log_path)?;
    let machine = crate::replication::Machine::open_with_owner(
        path,
        ConfState {
            voters: config.voters.clone(),
            learners: config.learners.clone(),
            ..ConfState::default()
        },
        Arc::clone(&lease),
    )?;
    let file = DeviceFile::open(
        log_path,
        true,
        CachingRequest::Buffered,
        hyper_block::buf::Alignment::new(4096).map_err(unavailable)?,
    )
    .map_err(unavailable)?;
    let settings = settings(config.node_id)?;
    let log_config = hyper_log::Config {
        segment_bytes: 64 << 20,
        max_segments: 16,
        max_groups: 1,
        group_entries: 65_536,
        group_bytes: 256 << 20,
        group_cache: 64 << 20,
        queue_submissions: 16,
        waits: hyper_log::Waits::Never,
    };
    let log = if file.len().map_err(unavailable)? == 0 {
        if machine.durable().index != 0 {
            return Err(fenced("state exists but its replication log is missing"));
        }
        hyper_log::Log::create(file, log_config, u128::from(config.node_id)).map_err(unavailable)?
    } else {
        let (log, recovery) = hyper_log::Log::open(file, log_config, u128::from(config.node_id))
            .map_err(unavailable)?;
        if !recovery.damaged.is_empty() {
            return Err(fenced("replication log recovery found acknowledged damage"));
        }
        log
    };
    let group = GroupStore::claim(&log, 1).map_err(unavailable)?;
    let replica =
        Replica::open(&settings, group, machine, Bytes::new(512 << 20)).map_err(unavailable)?;
    let control = Arc::new(Control {
        admitted: AtomicUsize::new(0),
        outgoing: Mutex::default(),
        status: Mutex::new(Ok(status(&replica))),
    });
    let (sender, receiver) = mpsc::sync_channel(COMMANDS);
    let started = Instant::now();
    let mut actor = Actor {
        published: replica.applied().index,
        replica,
        _log: log,
        _log_owner: log_owner,
        _lease: lease,
        changed,
        control: Arc::clone(&control),
        pending: None,
        waiting: VecDeque::new(),
        serial: 0,
        started,
        ticked: started,
    };
    actor.refresh()?;
    if Instant::now() >= until {
        return Err(Refusal::Timeout);
    }
    std::thread::Builder::new()
        .name("amphora.replica".into())
        .spawn(move || actor.run(receiver))
        .map_err(unavailable)?;
    Ok(Client {
        handle: ReplicaHandle {
            sender,
            shared: control,
        },
    })
}

fn validate(config: &ReplicaConfig) -> Result<(), Refusal> {
    let count = config
        .voters
        .len()
        .checked_add(config.learners.len())
        .ok_or_else(|| api::invalid("replica configuration", "member count overflow"))?;
    api::bound("replica members", count, MEMBERS)?;
    if config.node_id == 0 || config.voters.is_empty() {
        return Err(api::invalid(
            "replica configuration",
            "nonzero identity and voters required",
        ));
    }
    let mut unique = std::collections::BTreeSet::new();
    for id in config.voters.iter().chain(&config.learners) {
        if *id == 0 || !unique.insert(*id) {
            return Err(api::invalid(
                "replica configuration",
                "members must be nonzero, unique and disjoint",
            ));
        }
    }
    if !unique.contains(&config.node_id) {
        return Err(api::invalid(
            "replica configuration",
            "local identity is not a member",
        ));
    }
    Ok(())
}
fn settings(id: u64) -> Result<hyper_durable::Settings, Refusal> {
    let mut limits = hyper_raft::Limits::derive(hyper_raft::Stated {
        message: MESSAGE_BYTES,
        members: MEMBERS,
        memory: 64 << 20,
        depth: 1,
    })
    .map_err(unavailable)?;
    limits.pending_messages = OUTGOING_MESSAGES;
    limits.pending_reads = COMMANDS;
    limits.unstable_entries = 1024;
    limits.entries_per_message = 1024;
    limits.proposals = 1;
    Ok(hyper_durable::Settings {
        core: hyper_raft::Config {
            check_quorum: true,
            pre_vote: true,
            max_size_per_msg: u64::try_from(MESSAGE_BYTES).map_err(unavailable)?,
            max_uncommitted_size: 64 << 20,
            max_committed_size_per_ready: 64 << 20,
            max_inflight_msgs: 4,
            max_inflight_bytes: 64 << 20,
            seed: id,
            ..hyper_raft::Config::new(id, limits)
        },
        elections: hyper_raft::Elections::Ticks,
        quiet: Duration::from_millis(50),
    })
}
fn status(replica: &Durable) -> ReplicaStatus {
    ReplicaStatus {
        node_id: replica.id(),
        leader: replica.leader(),
        term: replica.term(),
        applied: replica.applied().index,
        durable_commit: replica.durable_commit(),
        voters: replica.configuration().voters.clone(),
        learners: replica.configuration().learners.clone(),
        fenced: replica.fenced().map(ToString::to_string),
    }
}

impl Actor {
    fn now(&self) -> Result<u64, Refusal> {
        u64::try_from(self.started.elapsed().as_nanos()).map_err(unavailable)
    }
    fn refresh(&mut self) -> Result<(), Refusal> {
        self.replica
            .machine_mut()
            .read_db(|db| reload_changed(db, &self.changed).map_err(Into::into))?;
        self.published = self.replica.applied().index;
        Ok(())
    }
    fn run(&mut self, receiver: mpsc::Receiver<Envelope>) {
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.run_loop(&receiver)))
                .unwrap_or_else(|_| {
                    Err(fenced("replica owner unwound; reopen from durable state"))
                });
        let reason = match result {
            Ok(()) => fenced("replica owner stopped"),
            Err(error) => fenced(error),
        };
        if let Some(mut pending) = self.pending.take()
            && let Some(response) = pending.response.take()
        {
            answer(response, Err(unknown()));
        }
        if let Ok(mut status) = self.control.status.lock() {
            *status = Err(reason.clone());
        }
        for _ in 0..COMMANDS {
            let Some(envelope) = self.waiting.pop_front() else {
                break;
            };
            if let Input::Submit { response, .. } = envelope.command {
                answer(response, Err(reason.clone()));
            }
        }
        // The ingress channel has at most COMMANDS items. Notify callers before closing it.
        for _ in 0..COMMANDS {
            let Ok(envelope) = receiver.try_recv() else {
                break;
            };
            match envelope.command {
                Input::Submit { response, .. } => answer(response, Err(reason.clone())),
                Input::Receive { response, .. } => answer(response, Err(reason.clone())),
            }
        }
    }
    fn run_loop(&mut self, receiver: &mpsc::Receiver<Envelope>) -> Result<(), Refusal> {
        loop {
            let now = Instant::now();
            if now.saturating_duration_since(self.ticked) >= TICK {
                // A delayed owner does not invent missed heartbeats or run an unbounded catchup.
                self.ticked = now;
                self.replica.tick().map_err(raft_error)?;
            }
            for _ in 0..DRIVE_QUANTUM {
                let mut output = Output::default();
                let nanos = self.now()?;
                let driven = self
                    .replica
                    .drive(nanos, std::task::Waker::noop(), &mut output)
                    .map_err(raft_error)?;
                self.release(output)?;
                if !driven.more {
                    break;
                }
            }
            if self.replica.applied().index != self.published {
                self.refresh()?;
            }
            self.expire();
            if self.pending.is_none()
                && let Some(envelope) = self.waiting.pop_front()
            {
                self.receive(envelope)?;
            }
            if self.replica.compactable_bytes() >= 128 << 20 || self.replica.is_stalled() {
                let nanos = self.now()?;
                self.replica
                    .compact(0, nanos, std::task::Waker::noop())
                    .map_err(raft_error)?;
            }
            *locked(&self.control.status)? = Ok(status(&self.replica));
            match receiver.recv_timeout(POLL) {
                Ok(envelope) => self.receive(envelope)?,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            }
        }
    }
    fn expire(&mut self) {
        let changed_term = self
            .pending
            .as_ref()
            .is_some_and(|p| p.term != self.replica.term() || !self.replica.is_leader());
        if changed_term {
            if let Some(mut pending) = self.pending.take()
                && let Some(response) = pending.response.take()
            {
                let refusal = match pending.phase {
                    Phase::Barrier(_) => nonleader(&self.replica),
                    Phase::Proposed(_) => unknown(),
                };
                answer(response, Err(refusal));
            }
            return;
        }
        let expired = self
            .pending
            .as_ref()
            .is_some_and(|p| Instant::now() >= p.until);
        if expired && let Some(mut pending) = self.pending.take() {
            let proposed = matches!(pending.phase, Phase::Proposed(_));
            if let Some(response) = pending.response.take() {
                answer(
                    response,
                    Err(if proposed {
                        unknown()
                    } else {
                        Refusal::Timeout
                    }),
                );
            }
            // The captured base cannot be used by another write until this one applies. A caller
            // timing out relinquishes its response, never the unresolved proposal reservation.
            if proposed {
                self.pending = Some(pending);
            }
        }
    }
    fn receive(&mut self, envelope: Envelope) -> Result<(), Refusal> {
        let Envelope {
            command,
            _reservation,
        } = envelope;
        match command {
            Input::Receive {
                from,
                bytes,
                until,
                response,
            } => {
                let result = if Instant::now() >= until {
                    Err(Refusal::Timeout)
                } else {
                    self.step(from, &bytes)
                };
                let fatal = matches!(&result, Err(Refusal::Fenced(_)));
                let error = result.as_ref().err().cloned();
                answer(response, result);
                if fatal && let Some(error) = error {
                    return Err(error);
                }
            }
            Input::Submit {
                namespace,
                operation,
                until,
                response,
            } => {
                if Instant::now() >= until {
                    answer(response, Err(Refusal::Timeout));
                } else if !self.replica.is_leader() {
                    answer(response, Err(nonleader(&self.replica)));
                } else if self.pending.is_some() {
                    if self.waiting.len() >= COMMANDS {
                        answer(response, Err(Refusal::Full { limit: COMMANDS }));
                    } else {
                        self.waiting.push_back(Envelope {
                            command: Input::Submit {
                                namespace,
                                operation,
                                until,
                                response,
                            },
                            _reservation,
                        });
                    }
                } else {
                    self.serial = self
                        .serial
                        .checked_add(1)
                        .ok_or_else(|| fenced("request identity exhausted"))?;
                    // A restart must not confuse an old log entry with a new local request.
                    // Leadership requires a new election term after reopen; the serial is unique
                    // within that term, including read-only commands.
                    let mut context = self.replica.id().to_le_bytes().to_vec();
                    context.extend_from_slice(&self.replica.term().to_le_bytes());
                    context.extend_from_slice(&self.serial.to_le_bytes());
                    match self.replica.read(context.clone()) {
                        Ok(()) => {
                            self.pending = Some(Pending {
                                context,
                                term: self.replica.term(),
                                until,
                                namespace,
                                response: Some(response),
                                phase: Phase::Barrier(operation),
                                _reservation,
                            })
                        }
                        Err(error) => answer(response, Err(raft_error(error))),
                    }
                }
            }
        }
        Ok(())
    }
    fn step(&mut self, from: u64, bytes: &[u8]) -> Result<(), Refusal> {
        let message = Message::decode(bytes)
            .map_err(|error| api::invalid("replica wire record", &error.to_string()))?;
        if message.from != from || message.to != self.replica.id() || from == self.replica.id() {
            return Err(api::invalid(
                "replica sender",
                "authenticated sender or destination differs from record",
            ));
        }
        let conf = self.replica.configuration();
        if from == 0
            || !conf
                .voters
                .iter()
                .chain(&conf.learners)
                .chain(&conf.voters_outgoing)
                .chain(&conf.learners_next)
                .any(|id| *id == from)
        {
            return Err(api::invalid(
                "replica sender",
                "sender is not a current member",
            ));
        }
        self.replica.step(message).map_err(raft_error)
    }
    fn release(&mut self, output: Output<crate::replication::Applied>) -> Result<(), Refusal> {
        // Publish durable follower and leader state before releasing any result or read.
        if self.replica.applied().index != self.published {
            self.refresh()?;
        }
        for applied in output.answers {
            let matches = self
                .pending
                .as_ref()
                .is_some_and(|p| p.context == applied.context);
            if matches && let Some(mut pending) = self.pending.take() {
                let result = match pending.phase {
                    Phase::Proposed(value) => applied.result.map(|()| value),
                    Phase::Barrier(_) => {
                        Err(fenced("a read barrier matched an unapplied proposal"))
                    }
                };
                if let Some(response) = pending.response.take() {
                    answer(response, result);
                }
            }
        }
        for (context, _) in output.reads {
            if self.pending.as_ref().is_some_and(|p| p.context == context) {
                self.prepare()?;
            }
        }
        for entry in output.displaced {
            if self
                .pending
                .as_ref()
                .is_some_and(|p| p.context == entry.context)
                && let Some(mut pending) = self.pending.take()
                && let Some(response) = pending.response.take()
            {
                answer(response, Err(unknown()));
            }
        }
        let mut outgoing = locked(&self.control.outgoing)?;
        for message in output.messages {
            let mut bytes = Vec::new();
            message.encode(&mut bytes);
            api::bound("replica outgoing record", bytes.len(), WIRE_BYTES)?;
            let total = outgoing
                .bytes
                .checked_add(bytes.len())
                .ok_or_else(|| fenced("outgoing size overflow"))?;
            if total > OUTGOING_BYTES || outgoing.messages.len() >= OUTGOING_MESSAGES {
                return Err(fenced(
                    "replica outgoing queue is full; drain and reopen from durable state",
                ));
            }
            outgoing.bytes = total;
            outgoing.messages.push_back(Outbound {
                to: message.to,
                bytes,
            });
        }
        Ok(())
    }
    fn propose(&mut self, context: &[u8], bytes: Vec<u8>) -> Result<(), Refusal> {
        let room = self
            ._log
            .entry_room()
            .map_err(unavailable)?
            .checked_sub(hyper_durable::ENTRY_OVERHEAD)
            .and_then(|room| room.checked_sub(context.len()))
            .ok_or_else(|| fenced("replication log cannot hold the proposal envelope"))?;
        api::bound("replication log entry", bytes.len(), room)?;
        self.replica
            .propose(context.to_vec(), bytes)
            .map_err(raft_error)
    }
    fn prepare(&mut self) -> Result<(), Refusal> {
        let Some(mut pending) = self.pending.take() else {
            return Ok(());
        };
        if pending.term != self.replica.term() || !self.replica.is_leader() {
            if let Some(response) = pending.response.take() {
                answer(response, Err(nonleader(&self.replica)));
            }
            return Ok(());
        }
        if Instant::now() >= pending.until {
            if let Some(response) = pending.response.take() {
                answer(response, Err(Refusal::Timeout));
            }
            return Ok(());
        }
        let Phase::Barrier(operation) = pending.phase else {
            self.pending = Some(pending);
            return Ok(());
        };
        let prepared = self.replica.machine_mut().prepare_db(|db| {
            if restore::incomplete(db)? {
                return Err(api::invalid("database", "restore is incomplete").into());
            }
            match &operation {
                Operation::Call(call) => {
                    let call = locked(call)?
                        .take()
                        .ok_or_else(|| unavailable("SQL session already executed"))?;
                    call(db).map(Answer::Called)
                }
                other => apply(db, &pending.namespace, other)?.map_err(Into::into),
            }
        });
        let result = match prepared {
            Ok((prepared, value)) => {
                if Instant::now() >= pending.until {
                    Err(Refusal::Timeout)
                } else if prepared.is_empty() {
                    Ok(value)
                } else {
                    match prepared
                        .encode()
                        .and_then(|bytes| self.propose(&pending.context, bytes))
                    {
                        Ok(()) => {
                            pending.phase = Phase::Proposed(value);
                            self.pending = Some(pending);
                            return Ok(());
                        }
                        Err(error) => Err(error),
                    }
                }
            }
            Err(error) => Err(error),
        };
        if let Some(response) = pending.response.take() {
            answer(response, result);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(5)
    }
    fn open(dir: &Path, voters: Vec<u64>) -> Client {
        let path = dir.join("state.db");
        let until = deadline();
        // Wait for actual lease release, including the actor's log/machine destruction.
        let lease = loop {
            match exclusive_file(&path) {
                Ok(lease) => break Arc::new(lease),
                Err(error) => {
                    assert!(Instant::now() < until, "owner did not drain: {error}");
                    std::thread::park_timeout(POLL);
                }
            }
        };
        start(
            &path,
            &dir.join("raft.log"),
            ReplicaConfig {
                node_id: 1,
                voters,
                learners: Vec::new(),
            },
            Arc::new(Changed::default()),
            lease,
            until,
        )
        .unwrap()
    }
    fn leader(client: &Client) {
        let until = deadline();
        loop {
            let status = client.handle().status().unwrap();
            if status.leader == status.node_id && status.applied > 0 {
                return;
            }
            assert!(
                Instant::now() < until,
                "single voter did not elect: {status:?}"
            );
            std::thread::park_timeout(POLL);
        }
    }
    fn call(
        client: &Client,
        f: impl FnOnce(&mut Db) -> anyhow::Result<Vec<u8>> + Send + 'static,
    ) -> Result<Vec<u8>, Refusal> {
        let operation = Operation::Call(Mutex::new(Some(Box::new(move |db| {
            Ok(Box::new(f(db)?) as Box<dyn Any + Send>)
        }))));
        match client.submit(String::new(), operation, deadline())? {
            Answer::Called(value) => Ok(*value.downcast::<Vec<u8>>().unwrap()),
            _ => panic!("wrong actor reply"),
        }
    }
    fn value(db: &mut Db) -> anyhow::Result<Vec<u8>> {
        let mut value = Vec::new();
        db.query("SELECT value FROM actor_items", [], |row| {
            value = row
                .get_value(0)
                .to_blob()
                .ok_or_else(|| anyhow::anyhow!("expected blob"))?
                .to_vec();
            Ok(false)
        })?;
        Ok(value)
    }

    /// U15/CP-T1.8: the product owner must not run a closure before leadership and ReadIndex.
    #[test]
    fn nonleader_refuses_without_evaluating_and_authentication_binds_wire_sender() {
        let dir = tempfile::tempdir().unwrap();
        let client = open(dir.path(), vec![1, 2, 3]);
        let evaluated = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&evaluated);
        let result = call(&client, move |_| {
            count.fetch_add(1, Ordering::Relaxed);
            Ok(Vec::new())
        });
        assert!(matches!(result, Err(Refusal::NotLeader { .. })));
        assert_eq!(evaluated.load(Ordering::Relaxed), 0);
        let mut bytes = Vec::new();
        Message {
            from: 2,
            to: 1,
            ..Message::default()
        }
        .encode(&mut bytes);
        assert!(matches!(
            client.handle().receive(3, bytes, deadline()),
            Err(Refusal::Invalid { .. })
        ));
        assert!(matches!(
            client.handle().drain(OUTGOING_MESSAGES + 1),
            Err(Refusal::TooLarge { .. })
        ));
    }

    /// U15/CP-T1.8: real DeviceFile log, product-driven election, once-only capture and reopen.
    #[test]
    fn single_voter_actor_captures_once_and_reopens_durable_random_value() {
        let dir = tempfile::tempdir().unwrap();
        let client = open(dir.path(), vec![1]);
        leader(&client);
        let evaluated = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&evaluated);
        let chosen = call(&client, move |db| {
            count.fetch_add(1, Ordering::Relaxed);
            db.exec("CREATE TABLE actor_items(value BLOB DEFAULT(randomblob(32))); INSERT INTO actor_items DEFAULT VALUES")?;
            value(db)
        }).unwrap();
        assert_eq!(evaluated.load(Ordering::Relaxed), 1);
        assert_eq!(call(&client, value).unwrap(), chosen);
        let applied = client.handle().status().unwrap().applied;
        drop(client);
        let reopened = open(dir.path(), vec![1]);
        leader(&reopened);
        assert!(reopened.handle().status().unwrap().applied >= applied);
        assert_eq!(call(&reopened, value).unwrap(), chosen);
        assert_eq!(evaluated.load(Ordering::Relaxed), 1);
    }
}
