//! The single owner of a replicated Database. Hosts carry authenticated wire records only.
//!
//! One command at a time crosses ReadIndex before evaluation. A captured answer stays here until
//! hyper-durable releases its matching durable application; no caller sees speculative state.

use super::*;
use std::collections::VecDeque;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use api::replication::{CatchUp, MemberChange, Outbound, ReplicaConfig, ReplicaStatus};
use hyper_block::file::{CachingRequest, DeviceFile};
use hyper_durable::{Bytes, GroupStore, Output, Replica, StateMachine};
use hyper_raft::proto::{
    ConfChangeSingle, ConfChangeTransition, ConfChangeType, ConfChangeV2, ConfState, Message, Plan,
};
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
// The admission charge of a change of members or a hand-off of the lead: policy, not a measured
// size. Each is a few bytes; the charge holds its place in the queue's byte budget.
const CHANGE_BYTES: usize = 1024;

type Durable = Replica<GroupStore<DeviceFile>, crate::replication::Machine, Bytes>;
type Response<T> = mpsc::SyncSender<Result<T, Refusal>>;

/// A bounded ingress and egress handle. Clones share one product-owned Raft actor.
#[derive(Clone)]
pub struct ReplicaHandle {
    sender: mpsc::SyncSender<Envelope>,
    shared: Arc<Control>,
}

/// Completion of a bounded queued wire record. Admission transfers byte ownership only;
/// processing, application and durability are not acknowledged until their own protocol steps.
#[must_use = "poll the receipt to observe later processing errors"]
pub struct ReceiveReceipt {
    // Mutex makes receipts Sync for host carrier shells; exclusive polling uses get_mut and
    // never locks or waits for the actor. The actor holds only the channel's sending end.
    receiver: Mutex<mpsc::Receiver<Result<(), Refusal>>>,
    until: Instant,
    completed: bool,
}
impl ReceiveReceipt {
    /// None is pending; Some(()) means the actor accepted the record through step. Typed errors
    /// report rejection, deadline expiry or owner loss. A completed receipt cannot be polled again.
    /// Expiry stops observation; it does not undo processing that already began.
    pub fn try_result(&mut self) -> Result<Option<()>, Refusal> {
        if self.completed {
            return Err(api::invalid("replica receive receipt", "already completed"));
        }
        let result = match self.receiver.get_mut() {
            Ok(receiver) => match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) if Instant::now() < self.until => None,
                Err(mpsc::TryRecvError::Empty) => Some(Err(Refusal::Timeout)),
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(fenced("replica owner ended"))),
            },
            Err(_) => Some(Err(unavailable(
                "replica receive receipt lock was poisoned",
            ))),
        };
        match result {
            None => Ok(None),
            Some(result) => {
                self.completed = true;
                result.map(|()| Some(()))
            }
        }
    }
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
    Change {
        change: MemberChange,
        until: Instant,
        response: Response<ReplicaStatus>,
    },
    Transfer {
        to: u64,
        until: Instant,
        response: Response<ReplicaStatus>,
    },
    /// Where catching up each learner stands, judged at once (`ReplicaHandle::catch_up`), unless
    /// `until` has passed: a stale request stages no learner's round.
    CatchUp {
        until: Instant,
        response: Response<Vec<(u64, CatchUp)>>,
    },
}
/// The hand-off of the lead under way (replication.md, "Membership"): asked of the leader in
/// `term`, answered once this member follows `to`.
struct Transferring {
    to: u64,
    term: u64,
    until: Instant,
    response: Response<ReplicaStatus>,
    _reservation: Reservation,
}
/// The change of members under way (replication.md, "Membership"): one at a time, beside the
/// write under way, which neither waits for.
struct Changing {
    context: Vec<u8>,
    term: u64,
    leader: u64,
    until: Instant,
    change: MemberChange,
    stage: Stage,
    response: Response<ReplicaStatus>,
    _reservation: Reservation,
}
enum Stage {
    /// Waiting for the ReadIndex that shows this leader current and its configuration applied.
    Barrier,
    /// Proposed at `index`, in this leader's term.
    Proposed { index: u64 },
    /// Applied: the group is in the joint configuration, or has left it.
    Applied,
}
struct Pending {
    context: Vec<u8>,
    term: u64,
    leader: u64,
    until: Instant,
    namespace: String,
    response: Option<Response<Answer>>,
    phase: Phase,
    // Keep admission charged while its command is in flight, not just while queued.
    _reservation: Reservation,
}
enum Phase {
    Barrier(Option<Operation>),
    Proposed(Answer),
    Bootstrap(Option<Operation>),
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
    changing: Option<Changing>,
    transferring: Option<Transferring>,
    // At most COMMANDS admitted operations wait behind the sole prepared transaction.
    waiting: VecDeque<Envelope>,
    serial: u64,
    // Every time this owner reads comes from this clock (replication.md, "The owner's time").
    clock: Clock,
    started: Instant,
    ticked: Instant,
    // The instant this owner last read from its clock: at its step's start, and again after a
    // preparation, whose work takes time of its own.
    at: Instant,
    published: u64,
    transferred: Option<(u64, Instant)>,
}

/// An owner's time, from its shell: the real clock for the owner thread, one its test moves for a
/// test shell.
type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

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
fn not_leader(leader: u64) -> Refusal {
    Refusal::NotLeader {
        leader: if leader == 0 { None } else { Some(leader) },
    }
}
fn answer<T>(response: Response<T>, result: Result<T, Refusal>) {
    // Failure means the deadline-expired caller relinquished its reply. Committed work remains
    // applied and published; dropping this private response never drops a database update.
    let _ = response.send(result);
}

impl ReplicaHandle {
    fn enqueue(&self, command: Input, bytes: usize) -> Result<(), Refusal> {
        // Untouched application calls can wait for leadership. Keep room for one maximum wire
        // record against application pressure so that waiting calls cannot block that discovery.
        let application_limit = QUEUED_BYTES
            .checked_sub(WIRE_BYTES)
            .ok_or_else(|| fenced("replica wire reservation exceeds command queue budget"))?;
        let mut charged = self.shared.admitted.load(Ordering::Acquire);
        let mut reserved = false;
        for _ in 0..COMMANDS {
            let next = charged.checked_add(bytes).ok_or(Refusal::Full {
                limit: QUEUED_BYTES,
            })?;
            if matches!(
                &command,
                Input::Submit { .. }
                    | Input::Change { .. }
                    | Input::Transfer { .. }
                    | Input::CatchUp { .. }
            ) && next > application_limit
            {
                return Err(Refusal::Full {
                    limit: application_limit,
                });
            }
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

    fn enqueue_receive(
        &self,
        from: u64,
        bytes: Vec<u8>,
        until: Instant,
    ) -> Result<mpsc::Receiver<Result<(), Refusal>>, Refusal> {
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
        Ok(receiver)
    }

    /// `from` must be the identity authenticated by the host's transport, not a wire claim.
    /// Success waits until the actor processes the record; it does not acknowledge a commit.
    pub fn receive(&self, from: u64, bytes: Vec<u8>, until: Instant) -> Result<(), Refusal> {
        let receiver = self.enqueue_receive(from, bytes, until)?;
        receiver
            .recv_timeout(until.saturating_duration_since(Instant::now()))
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => Refusal::Timeout,
                mpsc::RecvTimeoutError::Disconnected => fenced("replica owner ended"),
            })?
    }

    /// Transfers a wire record to the existing bounded actor queue without waiting for step.
    /// `from` is the host-authenticated identity. The host must reap the returned receipt to
    /// observe later validation, expiry and fencing; successful admission is not a durability ACK.
    /// Dropping the receipt abandons observation; it does not cancel or replay the queued record.
    /// Hosts must bound their retained receipts as well as handling the queue's admission limit.
    pub fn admit_receive(
        &self,
        from: u64,
        bytes: Vec<u8>,
        until: Instant,
    ) -> Result<ReceiveReceipt, Refusal> {
        Ok(ReceiveReceipt {
            receiver: Mutex::new(self.enqueue_receive(from, bytes, until)?),
            until,
            completed: false,
        })
    }

    /// Takes at most `limit` messages. An undrained host eventually fences explicitly at the
    /// egress bound; the actor never acknowledges a send the host has not taken.
    pub fn drain(&self, limit: usize) -> Result<Vec<Outbound>, Refusal> {
        api::bound("replica drain count", limit, OUTGOING_MESSAGES)?;
        let mut outgoing = locked(&self.shared.outgoing)?;
        if outgoing.messages.is_empty()
            && let Err(error) = &*locked(&self.shared.status)?
        {
            return Err(error.clone());
        }
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

    /// Makes `change` to the group's members (replication.md, "Membership"). Only the leader takes
    /// one, one at a time. It proposes it through a joint configuration, which it leaves by itself,
    /// and answers its status once the group has left it. A change the configuration already
    /// shows proposes nothing and answers at once. A change whose deadline passes once its joint
    /// configuration is applied answers its status too: it is committed, the status shows the
    /// group still joint (`voters_outgoing`), and the leader leaves it by itself. Refused: off the
    /// leader, naming the leader known (`NotLeader`); while another change or a hand-off is under
    /// way, while the group is leaving a joint configuration or has a change still to apply, or
    /// while it takes its seed (`Unavailable`); when the configuration can't take it (`Invalid`),
    /// or past the members a group may have, or a learner past the snapshot ceiling (`TooLarge`).
    /// UnknownOutcome: it was proposed, and the lead moved or the deadline passed before it was
    /// seen applied. Read the configuration before trying again.
    pub fn change(&self, change: MemberChange, until: Instant) -> Result<ReplicaStatus, Refusal> {
        if Instant::now() >= until {
            return Err(Refusal::Timeout);
        }
        let (response, receiver) = mpsc::sync_channel(1);
        self.enqueue(
            Input::Change {
                change,
                until,
                response,
            },
            CHANGE_BYTES,
        )?;
        receiver
            .recv_timeout(until.saturating_duration_since(Instant::now()))
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => unknown(),
                mpsc::RecvTimeoutError::Disconnected => unknown(),
            })?
    }

    /// Hands the lead to the voter `to` (replication.md, "Membership"), so that the leader can then
    /// be demoted or removed by its successor. Only the leader takes one, and not while a change of
    /// members, a joint configuration or the group's seed is under way. It answers its status once
    /// it follows `to`, at once when it is `to`. Refused: off the leader, or once the lead went to
    /// another, naming the leader known (`NotLeader`); while a change or another hand-off is under
    /// way, or when `to` did not take the lead within an election timeout (`Unavailable`); to a
    /// member that does not vote (`Invalid`). UnknownOutcome: the deadline passed first; read the
    /// status.
    pub fn transfer(&self, to: u64, until: Instant) -> Result<ReplicaStatus, Refusal> {
        if Instant::now() >= until {
            return Err(Refusal::Timeout);
        }
        let (response, receiver) = mpsc::sync_channel(1);
        self.enqueue(
            Input::Transfer {
                to,
                until,
                response,
            },
            CHANGE_BYTES,
        )?;
        receiver
            .recv_timeout(until.saturating_duration_since(Instant::now()))
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => unknown(),
                mpsc::RecvTimeoutError::Disconnected => unknown(),
            })?
    }

    /// Where catching up each of the group's learners stands, as this leader judges it now
    /// (replication.md, "Membership"; hyper-raft's `Raft::catch_up`). Asked first, a learner is
    /// staged, its first round sending what the leader holds then; asked again, it is judged as an
    /// election timeout passes, and its rounds end as its answers arrive. A new leader stages its
    /// learners afresh. Promoting a learner is the host's change to make. Refused off the leader,
    /// naming the leader known (`NotLeader`); past `until` when the owner takes it (`Timeout`),
    /// staging nothing.
    pub fn catch_up(&self, until: Instant) -> Result<Vec<(u64, CatchUp)>, Refusal> {
        if Instant::now() >= until {
            return Err(Refusal::Timeout);
        }
        let (response, receiver) = mpsc::sync_channel(1);
        self.enqueue(Input::CatchUp { until, response }, CHANGE_BYTES)?;
        receiver
            .recv_timeout(until.saturating_duration_since(Instant::now()))
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => Refusal::Timeout,
                mpsc::RecvTimeoutError::Disconnected => fenced("replica owner ended"),
            })?
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
    let (client, mut actor, receiver) = open(
        path,
        log_path,
        config,
        changed,
        lease,
        until,
        Arc::new(Instant::now),
    )?;
    std::thread::Builder::new()
        .name("amphora.replica".into())
        .spawn(move || actor.run(receiver))
        .map_err(unavailable)?;
    Ok(client)
}

/// An owner and its handle, not yet running: `start` runs it on the owner thread, and a test
/// shell steps it itself. `clock` is its shell's; the owner starts at its first reading.
fn open(
    path: &Path,
    log_path: &Path,
    config: ReplicaConfig,
    changed: Arc<Changed>,
    lease: Arc<File>,
    until: Instant,
    clock: Clock,
) -> Result<(Client, Actor, mpsc::Receiver<Envelope>), Refusal> {
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
        config.node_id,
        config.bootstrap.clone(),
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
        // A 256 MiB seed needs its chunk envelopes and election/configuration entries too.
        // 512 MiB payload admission stays below the 16 x 64 MiB segment ring (plus fixed
        // persist metadata). Even one 32 MiB chunk per segment uses at most nine segments;
        // seven remain, including hyper-log's final segment reserved for making room.
        group_bytes: 512 << 20,
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
    let at = clock();
    let mut actor = Actor {
        published: replica.applied().index,
        replica,
        _log: log,
        _log_owner: log_owner,
        _lease: lease,
        changed,
        control: Arc::clone(&control),
        pending: None,
        changing: None,
        transferring: None,
        waiting: VecDeque::new(),
        serial: 0,
        transferred: None,
        clock,
        started: at,
        ticked: at,
        at,
    };
    actor.refresh()?;
    if Instant::now() >= until {
        return Err(Refusal::Timeout);
    }
    let client = Client {
        handle: ReplicaHandle {
            sender,
            shared: control,
        },
    };
    Ok((client, actor, receiver))
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
    if let Some(bootstrap) = &config.bootstrap
        && !config.voters.contains(&bootstrap.source_node)
    {
        return Err(api::invalid(
            "bootstrap source",
            "source must be an initial voter",
        ));
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
        // CP-T2.2: preserve flush-proved group durability during API reconciliation.
        disk: hyper_durable::Disk::Flushed,
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
        bootstrapping: replica.machine().bootstrap_pending(),
        voters: replica.configuration().voters.clone(),
        learners: replica.configuration().learners.clone(),
        fenced: replica.fenced().map(ToString::to_string),
        voters_outgoing: replica.configuration().voters_outgoing.clone(),
    }
}

impl Actor {
    fn now(&self) -> Result<u64, Refusal> {
        u64::try_from(self.at.saturating_duration_since(self.started).as_nanos())
            .map_err(unavailable)
    }
    fn refresh(&mut self) -> Result<(), Refusal> {
        if self.replica.machine().bootstrap_pending() {
            self.published = self.replica.applied().index;
            return Ok(());
        }
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
        if let Some(changing) = self.changing.take() {
            let result = match changing.stage {
                Stage::Barrier => Err(reason.clone()),
                Stage::Proposed { .. } | Stage::Applied => Err(unknown()),
            };
            answer(changing.response, result);
        }
        if let Some(transferring) = self.transferring.take() {
            answer(transferring.response, Err(unknown()));
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
                Input::Change { response, .. } | Input::Transfer { response, .. } => {
                    answer(response, Err(reason.clone()))
                }
                Input::CatchUp { response, .. } => answer(response, Err(reason.clone())),
            }
        }
    }
    /// The owner thread: each step after waiting up to `POLL` for an input.
    fn run_loop(&mut self, receiver: &mpsc::Receiver<Envelope>) -> Result<(), Refusal> {
        loop {
            let input = match receiver.recv_timeout(POLL) {
                Ok(envelope) => Some(envelope),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            };
            self.run_step(input)?;
        }
    }
    /// One step at its clock's reading: the input first, then a tick if one is due, the core's
    /// work, deadlines, and what waits.
    fn run_step(&mut self, input: Option<Envelope>) -> Result<(), Refusal> {
        let at = (self.clock)();
        self.at = at;
        if let Some(envelope) = input {
            self.receive(envelope)?;
        }
        if at.saturating_duration_since(self.ticked) >= TICK {
            // A delayed owner does not invent missed heartbeats or run an unbounded catchup.
            self.ticked = at;
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
        self.expire_change();
        self.settle_change();
        self.settle_transfer();
        self.bootstrap()?;
        if !self.replica.machine().bootstrap_pending()
            && self.pending.is_none()
            && self.replica.leader() != 0
            && let Some(envelope) = self.waiting.pop_front()
        {
            self.receive(envelope)?;
        }
        if self.replica.machine().bootstrap_pending() && self.replica.is_stalled() {
            return Err(fenced(
                "bootstrap exhausted replication log admission before publication",
            ));
        }
        if !self.replica.machine().bootstrap_pending()
            && (self.replica.compactable_bytes() >= 128 << 20 || self.replica.is_stalled())
        {
            let nanos = self.now()?;
            self.replica
                .compact(0, nanos, std::task::Waker::noop())
                .map_err(raft_error)?;
        }
        *locked(&self.control.status)? = Ok(status(&self.replica));
        Ok(())
    }
    fn expire(&mut self) {
        // Expired callers relinquish queued operations even while a seed or proposal is held.
        // Rotate the bounded queue once; surviving requests retain their FIFO order.
        let queued = self.waiting.len().min(COMMANDS);
        for _ in 0..queued {
            let Some(envelope) = self.waiting.pop_front() else {
                break;
            };
            let expired =
                matches!(&envelope.command, Input::Submit { until, .. } if self.at >= *until);
            if expired {
                if let Input::Submit { response, .. } = envelope.command {
                    answer(response, Err(Refusal::Timeout));
                }
            } else {
                self.waiting.push_back(envelope);
            }
        }
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        let expired = self.at >= pending.until;
        let changed_term = pending.term != self.replica.term()
            || pending.leader != self.replica.leader()
            || self.replica.leader() == 0;
        if !expired && !changed_term {
            return;
        }
        let Some(mut pending) = self.pending.take() else {
            return;
        };
        match &pending.phase {
            Phase::Barrier(_) => {
                if expired {
                    if let Some(response) = pending.response.take() {
                        answer(response, Err(Refusal::Timeout));
                    }
                } else {
                    self.requeue(pending);
                }
            }
            Phase::Proposed(_) | Phase::Bootstrap(_) => {
                if let Some(response) = pending.response.take() {
                    answer(response, Err(unknown()));
                }
                // The captured base cannot be used by another write until this one applies. A
                // caller timing out relinquishes its response, never an unresolved proposal.
                if !changed_term {
                    self.pending = Some(pending);
                }
            }
        }
    }
    fn requeue(&mut self, mut pending: Pending) {
        // Only an untouched application operation may move back. A fresh dispatch assigns a new
        // context and ReadIndex; its old read completion cannot release the renewed barrier.
        let Phase::Barrier(Some(operation)) = pending.phase else {
            return;
        };
        let Some(response) = pending.response.take() else {
            return;
        };
        if self.at >= pending.until {
            answer(response, Err(Refusal::Timeout));
        } else if self.waiting.len() >= COMMANDS {
            answer(response, Err(Refusal::Full { limit: COMMANDS }));
        } else {
            self.waiting.push_front(Envelope {
                command: Input::Submit {
                    namespace: pending.namespace,
                    operation,
                    until: pending.until,
                    response,
                },
                _reservation: pending._reservation,
            });
        }
    }
    /// The change under way, once its barrier shows this leader current: proposed, or answered
    /// at once when the group is so already or can't take it.
    fn propose_change(&mut self) {
        let Some(mut changing) = self.changing.take() else {
            return;
        };
        if !matches!(changing.stage, Stage::Barrier) {
            self.changing = Some(changing);
            return;
        }
        let result = if self.at >= changing.until {
            Err(Refusal::Timeout)
        } else if changing.term != self.replica.term() || !self.replica.is_leader() {
            Err(not_leader(self.replica.leader()))
        } else {
            match self.member_change(changing.change) {
                Ok(None) => Ok(status(&self.replica)),
                Ok(Some(proposal)) => {
                    let before = self.replica.core().raft.pending_conf_index();
                    match self.replica.change(changing.context.clone(), &proposal) {
                        // The core marks the entry it took as its pending change. One it can't
                        // take it keeps in place stating nothing, and leaves that mark.
                        Ok(()) if self.replica.core().raft.pending_conf_index() > before => {
                            let index = self.replica.core().raft.pending_conf_index();
                            changing.stage = Stage::Proposed { index };
                            self.changing = Some(changing);
                            return;
                        }
                        Ok(()) => Err(unavailable(
                            "the leader did not take the change of members; try again",
                        )),
                        Err(error) => Err(raft_error(error)),
                    }
                }
                Err(refusal) => Err(refusal),
            }
        };
        answer(changing.response, result);
    }
    /// The change under way, once applied, and once the group has left the joint configuration it
    /// made: answered with the status. It runs after `expire_change`, so a change still proposed
    /// here has its lead, whose own entry at `index` no other replaces in its term.
    fn settle_change(&mut self) {
        let Some(changing) = self.changing.as_mut() else {
            return;
        };
        if let Stage::Proposed { index } = changing.stage
            && self.replica.applied().index >= index
        {
            changing.stage = Stage::Applied;
        }
        // The configuration applied, not the core's: the core counts by the newest its log
        // states, so its own leaves the joint one as soon as the leave is appended.
        if matches!(changing.stage, Stage::Applied)
            && self.replica.configuration().voters_outgoing.is_empty()
            && let Some(changing) = self.changing.take()
        {
            answer(changing.response, Ok(status(&self.replica)));
        }
    }
    /// The change under way past its deadline, or past the lead it was made under.
    fn expire_change(&mut self) {
        let Some(changing) = self.changing.as_ref() else {
            return;
        };
        let expired = self.at >= changing.until;
        let moved = changing.term != self.replica.term()
            || changing.leader != self.replica.leader()
            || self.replica.leader() == 0;
        let result = match changing.stage {
            Stage::Barrier if expired => Err(Refusal::Timeout),
            Stage::Barrier if moved => Err(not_leader(self.replica.leader())),
            Stage::Proposed { .. } if expired || moved => Err(unknown()),
            // Committed. The leader leaves the joint configuration by itself, this one or the
            // next, so a new lead waits for that; a deadline answers what is applied.
            Stage::Applied if expired => Ok(status(&self.replica)),
            _ => return,
        };
        if let Some(changing) = self.changing.take() {
            answer(changing.response, result);
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
                let result = if self.at >= until {
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
                if self.at >= until {
                    answer(response, Err(Refusal::Timeout));
                } else if self.replica.leader() == 0
                    || self.pending.is_some()
                    || self.replica.machine().bootstrap_pending()
                {
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
                    let context = match self.context() {
                        Ok(context) => context,
                        Err(refusal) => {
                            answer(response, Err(refusal.clone()));
                            return Err(refusal);
                        }
                    };
                    match self.replica.read(context.clone()) {
                        Ok(()) => {
                            self.pending = Some(Pending {
                                context,
                                term: self.replica.term(),
                                leader: self.replica.leader(),
                                until,
                                namespace,
                                response: Some(response),
                                phase: Phase::Barrier(Some(operation)),
                                _reservation,
                            })
                        }
                        Err(error) => answer(response, Err(raft_error(error))),
                    }
                }
            }
            Input::Change {
                change,
                until,
                response,
            } => {
                if self.at >= until {
                    answer(response, Err(Refusal::Timeout));
                } else if let Err(refusal) = self.may_change() {
                    answer(response, Err(refusal));
                } else {
                    // The barrier: this leader's term has committed, and its configuration is
                    // applied through what the group had committed when the change came.
                    let context = match self.context() {
                        Ok(context) => context,
                        Err(refusal) => {
                            answer(response, Err(refusal.clone()));
                            return Err(refusal);
                        }
                    };
                    match self.replica.read(context.clone()) {
                        Ok(()) => {
                            self.changing = Some(Changing {
                                context,
                                term: self.replica.term(),
                                leader: self.replica.leader(),
                                until,
                                change,
                                stage: Stage::Barrier,
                                response,
                                _reservation,
                            })
                        }
                        Err(error) => answer(response, Err(raft_error(error))),
                    }
                }
            }
            Input::Transfer {
                to,
                until,
                response,
            } => {
                if self.at >= until {
                    answer(response, Err(Refusal::Timeout));
                } else if let Err(refusal) = self.may_change() {
                    answer(response, Err(refusal));
                } else if to == self.replica.id() {
                    answer(response, Ok(status(&self.replica)));
                } else if !self.replica.core().raft.configuration().votes(to) {
                    // The core gives up, saying nothing, a hand-off to a member that can't lead.
                    answer(
                        response,
                        Err(api::invalid("lead transfer", "only a voter takes the lead")),
                    );
                } else {
                    match self.replica.transfer(to) {
                        Ok(()) => {
                            self.transferring = Some(Transferring {
                                to,
                                term: self.replica.term(),
                                until,
                                response,
                                _reservation,
                            })
                        }
                        Err(error) => answer(response, Err(raft_error(error))),
                    }
                }
            }
            Input::CatchUp { until, response } => {
                if self.at >= until {
                    answer(response, Err(Refusal::Timeout));
                    return Ok(());
                }
                let judged = self.catching_up();
                let fatal = match &judged {
                    Err(error @ Refusal::Fenced(_)) => Some(error.clone()),
                    _ => None,
                };
                answer(response, judged);
                if let Some(error) = fatal {
                    return Err(error);
                }
            }
        }
        Ok(())
    }
    /// Where catching up each learner of the configuration applied stands, judged now
    /// (`ReplicaHandle::catch_up`). A learner the core no longer counts, its removal appended but
    /// not yet applied, is left out.
    fn catching_up(&mut self) -> Result<Vec<(u64, CatchUp)>, Refusal> {
        if !self.replica.is_leader() {
            return Err(not_leader(self.replica.leader()));
        }
        let learners = self.replica.configuration().learners.clone();
        let mut judged = Vec::with_capacity(learners.len());
        for member in learners {
            let stands = match self.replica.catch_up(member).map_err(raft_error)? {
                hyper_raft::CatchUp::Ready => CatchUp::Ready,
                hyper_raft::CatchUp::Pending => CatchUp::Pending,
                hyper_raft::CatchUp::Aborted => CatchUp::Aborted,
                hyper_raft::CatchUp::NotLeader => return Err(not_leader(self.replica.leader())),
                hyper_raft::CatchUp::NotMember => continue,
            };
            judged.push((member, stands));
        }
        Ok(judged)
    }
    /// The hand-off under way: answered once this member follows `to`, once the lead went to
    /// another, once the core gave it up (it does within an election timeout), or past its
    /// deadline.
    fn settle_transfer(&mut self) {
        let Some(transferring) = self.transferring.as_ref() else {
            return;
        };
        let leader = self.replica.leader();
        let result = if leader == transferring.to {
            Ok(status(&self.replica))
        } else if self.at >= transferring.until {
            Err(unknown())
        } else if leader != 0 && leader != self.replica.id() {
            Err(not_leader(leader))
        } else if self.replica.is_leader()
            && (self.replica.term() != transferring.term
                || self.replica.core().raft.lead_transferee().is_none())
        {
            Err(unavailable(
                "the lead was not taken within an election timeout; try again",
            ))
        } else {
            return;
        };
        if let Some(transferring) = self.transferring.take() {
            answer(transferring.response, result);
        }
    }
    /// Whether this owner takes a change of members now.
    fn may_change(&self) -> Result<(), Refusal> {
        if !self.replica.is_leader() {
            return Err(not_leader(self.replica.leader()));
        }
        if self.changing.is_some() || self.transferring.is_some() {
            return Err(unavailable(
                "a change of members is under way; try again once it is answered",
            ));
        }
        if self.replica.machine().bootstrap_pending() {
            return Err(unavailable(
                "the group is taking its seed; change its members once it has",
            ));
        }
        // A change only once the last is applied, and a hand-off only once its voter can have the
        // way out of a joint configuration: one admitted while joint loses its election for nothing.
        let raft = &self.replica.core().raft;
        if raft.configuration().is_joint() {
            return Err(unavailable(
                "the group is leaving a joint configuration; try again once it has",
            ));
        }
        if raft.has_pending_conf() {
            return Err(unavailable(
                "the group has a change of members still to apply; try again once it has",
            ));
        }
        Ok(())
    }
    /// What `change` asks of the group as this leader leads it now: the joint change that makes
    /// it, none when the group is so already, or why the group can't take it.
    fn member_change(&mut self, change: MemberChange) -> Result<Option<ConfChangeV2>, Refusal> {
        let raft = &self.replica.core().raft;
        let configuration = raft.configuration();
        if configuration.is_joint() || raft.has_pending_conf() {
            return Err(unavailable(
                "a change of members is under way; try again once it is applied",
            ));
        }
        let invalid = |why: &str| -> Result<Option<ConfChangeV2>, Refusal> {
            Err(api::invalid("member change", why))
        };
        let (MemberChange::AddLearner(member)
        | MemberChange::Promote(member)
        | MemberChange::Demote(member)
        | MemberChange::Remove(member)) = change;
        if member == 0 {
            return invalid("a member's identity is nonzero");
        }
        let (learns, votes) = (configuration.learns(member), configuration.votes(member));
        let change_type = match change {
            MemberChange::AddLearner(_) if learns => return Ok(None),
            MemberChange::AddLearner(_) if votes => {
                return invalid("a voter is demoted, not added as a learner");
            }
            MemberChange::AddLearner(_) => ConfChangeType::AddLearnerNode,
            MemberChange::Promote(_) if votes => return Ok(None),
            MemberChange::Promote(_) if !learns => {
                return invalid("only a learner is promoted; add it as a learner first");
            }
            MemberChange::Promote(_) => ConfChangeType::AddNode,
            MemberChange::Demote(_) if learns => return Ok(None),
            MemberChange::Demote(_) if !votes => return invalid("only a voter is demoted"),
            MemberChange::Remove(_) if !learns && !votes => return Ok(None),
            MemberChange::Demote(_) | MemberChange::Remove(_) if member == self.replica.id() => {
                return invalid("the leader hands on its lead first (transfer), then is changed");
            }
            MemberChange::Demote(_) => ConfChangeType::AddLearnerNode,
            MemberChange::Remove(_) => ConfChangeType::RemoveNode,
        };
        // Two phases (Ongaro and Ousterhout 2014, section 6): into the joint configuration, then
        // out of it, which the leader proposes by itself once the first is committed.
        let proposal = ConfChangeV2 {
            transition: ConfChangeTransition::Implicit,
            changes: vec![ConfChangeSingle {
                change_type,
                node_id: member,
            }],
            context: Vec::new(),
        };
        // What the core would make of it, so that a change it can't take (one leaving no voter)
        // is refused here, not committed as a change that changes nothing.
        let changed = Plan::of(&proposal)
            .and_then(|plan| plan.apply(configuration))
            .map_err(|error| api::invalid("member change", &error.to_string()))?;
        api::bound(
            "replica members",
            changed.configuration.members().count(),
            raft.config().limits.members.min(MEMBERS),
        )?;
        // A joiner catches up by the leader's image once the log no longer reaches back: past
        // the image's ceiling it never could, so it is refused here, not added to fall behind.
        if matches!(change, MemberChange::AddLearner(_)) {
            self.replica.machine_mut().admits_joiner()?;
        }
        Ok(Some(proposal))
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
        // The members the core counts by: the newest configuration its log states, applied or
        // not. A leader sends to a member it adds from the moment it appends the change, and stops
        // at the moment it appends a removal, so that member's answers count from and to then.
        if from == 0 || !self.replica.core().raft.configuration().contains(from) {
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
                    Phase::Bootstrap(operation) => match applied.result {
                        Ok(()) => {
                            pending.phase = Phase::Barrier(operation);
                            match self.replica.read(pending.context.clone()) {
                                Ok(()) => {
                                    self.pending = Some(pending);
                                    continue;
                                }
                                Err(error) => Err(raft_error(error)),
                            }
                        }
                        Err(error) => Err(error),
                    },
                };
                if let Some(response) = pending.response.take() {
                    answer(response, result);
                }
            }
        }
        for (context, _) in output.reads {
            if self.pending.as_ref().is_some_and(|p| p.context == context) {
                self.prepare()?;
            } else if self.changing.as_ref().is_some_and(|c| c.context == context) {
                self.propose_change();
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
    fn context(&mut self) -> Result<Vec<u8>, Refusal> {
        self.serial = self
            .serial
            .checked_add(1)
            .ok_or_else(|| fenced("request identity exhausted"))?;
        // Followers can reopen within a term; the durable shell's fresh incarnation disambiguates
        // an old log entry from a new request at the same origin.
        let mut context = self.replica.id().to_le_bytes().to_vec();
        context.extend_from_slice(&self.replica.term().to_le_bytes());
        context.extend_from_slice(&self.replica.core().raft.config().incarnation.to_le_bytes());
        context.extend_from_slice(&self.serial.to_le_bytes());
        Ok(context)
    }
    fn bootstrap(&mut self) -> Result<(), Refusal> {
        if !self.replica.machine().bootstrap_pending() || self.pending.is_some() {
            return Ok(());
        }
        let source = self
            .replica
            .machine()
            .bootstrap_source()
            .ok_or_else(|| fenced("pending bootstrap has no designated source"))?;
        if source != self.replica.id() || !self.replica.is_leader() {
            // A failed transfer can be requested again after the core's twenty-tick election
            // period. At most one such request is made per period, never a host election.
            let period = TICK
                .checked_mul(20)
                .ok_or_else(|| fenced("tick period overflow"))?;
            let due = self.transferred.is_none_or(|(term, at)| {
                term != self.replica.term() || self.at.saturating_duration_since(at) >= period
            });
            if due
                && self.replica.leader() != 0
                && (self.replica.is_leader() || source == self.replica.id())
            {
                self.replica.transfer(source).map_err(raft_error)?;
                self.transferred = Some((self.replica.term(), self.at));
            }
            return Ok(());
        }
        let context = self.context()?;
        self.replica.read(context.clone()).map_err(raft_error)?;
        // One internal attempt has a finite lease; committed chunk progress survives its expiry.
        // User requests retain their own deadlines in the bounded waiting queue throughout.
        let until = self
            .at
            .checked_add(Duration::from_secs(60))
            .ok_or_else(|| fenced("bootstrap deadline overflow"))?;
        self.pending = Some(Pending {
            context,
            term: self.replica.term(),
            leader: self.replica.leader(),
            until,
            namespace: String::new(),
            response: None,
            phase: Phase::Barrier(None),
            _reservation: Reservation {
                control: Arc::clone(&self.control),
                bytes: 0,
            },
        });
        Ok(())
    }
    fn entry_room(&self, context: usize) -> Result<usize, Refusal> {
        self._log
            .entry_room()
            .map_err(unavailable)?
            .checked_sub(hyper_durable::ENTRY_OVERHEAD)
            .and_then(|room| room.checked_sub(context))
            .ok_or_else(|| fenced("replication log cannot hold the proposal envelope"))
    }
    fn propose(&mut self, context: &[u8], bytes: Vec<u8>) -> Result<(), Refusal> {
        api::bound(
            "replication log entry",
            bytes.len(),
            self.entry_room(context.len())?,
        )?;
        self.replica
            .propose(context.to_vec(), bytes)
            .map_err(raft_error)
    }
    fn prepare(&mut self) -> Result<(), Refusal> {
        let Some(mut pending) = self.pending.take() else {
            return Ok(());
        };
        // A delayed read completion must not refuse or replay an already proposed operation.
        if !matches!(pending.phase, Phase::Barrier(_)) {
            self.pending = Some(pending);
            return Ok(());
        }
        if self.at >= pending.until {
            if let Some(response) = pending.response.take() {
                answer(response, Err(Refusal::Timeout));
            }
            return Ok(());
        }
        if pending.term != self.replica.term()
            || pending.leader != self.replica.leader()
            || self.replica.leader() == 0
        {
            self.requeue(pending);
            return Ok(());
        }
        let Phase::Barrier(operation) = pending.phase else {
            self.pending = Some(pending);
            return Ok(());
        };
        if self.replica.machine().bootstrap_pending() {
            let source = self
                .replica
                .machine()
                .bootstrap_source()
                .ok_or_else(|| fenced("pending bootstrap has no designated source"))?;
            if source != self.replica.id() || !self.replica.is_leader() {
                // The core's transfer request names its target in `from`. Only the source can
                // forward that request authenticated as itself; a leader executes it locally.
                if self.replica.is_leader() || source == self.replica.id() {
                    self.replica.transfer(source).map_err(raft_error)?;
                }
                if let Some(response) = pending.response.take() {
                    answer(
                        response,
                        Err(Refusal::NotLeader {
                            leader: Some(source),
                        }),
                    );
                }
                return Ok(());
            }
            let room = self
                .entry_room(pending.context.len())?
                .min(crate::replication::COMMAND_BYTES);
            let seed = self.replica.machine_mut().prepare_bootstrap(room);
            let result = match seed {
                Ok(Some(bytes)) => self.propose(&pending.context, bytes),
                Ok(None) => Err(fenced("pending bootstrap produced no seed chunk")),
                Err(error) => Err(error),
            };
            match result {
                Ok(()) => {
                    pending.phase = Phase::Bootstrap(operation);
                    self.pending = Some(pending);
                }
                Err(error) => {
                    if let Some(response) = pending.response.take() {
                        answer(response, Err(error));
                    }
                }
            }
            return Ok(());
        }
        let Some(operation) = operation else {
            return Ok(());
        };
        let bounded = matches!(
            &operation,
            Operation::Mutation(Mutation::CompactStep { .. })
        );
        let prepared = if let Operation::Mutation(Mutation::CompactStep { before, .. }) = &operation
        {
            let room = self.entry_room(pending.context.len())?;
            self.replica
                .machine_mut()
                .prepare_compaction(&pending.namespace, *before, pending.until, room)
                .map(|(prepared, progress)| (prepared, Answer::Compacted(progress)))
        } else {
            let clock = Arc::clone(&self.clock);
            self.replica.machine_mut().prepare_db(|db| {
                // The guard ends with application evaluation, before prepare_pages rolls back its
                // speculative transaction. Cleanup must run even after the SQL deadline elapsed.
                let _deadline = db.deadline_on(pending.until, move || clock());
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
            })
        };
        // The preparation ran the caller's work, which takes time of its own: its deadline is
        // judged at the clock's reading after it, never at the step's start.
        self.at = (self.clock)();
        // A failed whole rollback poisons this machine. Stop its existing owner instead of
        // accepting another request against an unverified speculative base.
        if bounded && let Err(Refusal::Fenced(reason)) = &prepared {
            return Err(fenced(reason));
        }
        let result = match prepared {
            Ok((prepared, value)) => {
                if self.at >= pending.until {
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
            Err(error) => Err(if self.at >= pending.until {
                Refusal::Timeout
            } else {
                error
            }),
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
    use std::sync::atomic::AtomicU64;

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

    /// U15/CP-T1.8 on driven time: the product owner must not run a closure before leadership and
    /// ReadIndex, and authentication binds a wire record to its sender. Do: call a lone member of
    /// three, which has no leader, and step it with its clock a nanosecond before the call's
    /// deadline, then at it. Expect: no answer before the deadline, Timeout at it, the closure never
    /// run; then records whose claimed sender isn't the authenticated one, or isn't a member,
    /// refused, and a drain past the outgoing bound refused too.
    #[test]
    fn unknown_leader_waits_to_deadline_without_evaluating_and_authentication_binds_wire_sender() {
        let dir = tempfile::tempdir().unwrap();
        let mut driven = Driven::new();
        let client = driven.open(dir.path(), 1, vec![1, 2, 3], None);
        let evaluated = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&evaluated);
        let until = driven.until();
        let answer = submit(&client, until, move |_| {
            count.fetch_add(1, Ordering::Relaxed);
            Ok(Vec::new())
        });
        let clock = driven.clock(0);
        set(&clock, driven.base, until - Duration::from_nanos(1));
        driven.step();
        assert!(matches!(answer.try_recv(), Err(mpsc::TryRecvError::Empty)));
        set(&clock, driven.base, until);
        driven.step();
        assert!(matches!(answer.try_recv(), Ok(Err(Refusal::Timeout))));
        assert_eq!(evaluated.load(Ordering::Relaxed), 0);
        let mut forged = Vec::new();
        Message {
            from: 2,
            to: 1,
            ..Message::default()
        }
        .encode(&mut forged);
        let mut foreign = Vec::new();
        Message {
            from: 4,
            to: 1,
            ..Message::default()
        }
        .encode(&mut foreign);
        let mut receipts = [
            client
                .handle()
                .admit_receive(3, forged, driven.until())
                .unwrap(),
            client
                .handle()
                .admit_receive(4, foreign, driven.until())
                .unwrap(),
        ];
        driven.step();
        for receipt in &mut receipts {
            assert!(matches!(receipt.try_result(), Err(Refusal::Invalid { .. })));
        }
        assert!(matches!(
            client.handle().drain(OUTGOING_MESSAGES + 1),
            Err(Refusal::TooLarge { .. })
        ));
    }

    /// CP-T1.8 on driven time: admitted application calls cannot consume replication's reserved
    /// byte capacity. Do: fill a leaderless member's queue with calls until it refuses one, then
    /// admit a member's heartbeat and step the member. Expect: the refusal is the application
    /// bound, the heartbeat processed, and no call run.
    #[test]
    fn unknown_leader_application_pressure_keeps_wire_admission_available() {
        let dir = tempfile::tempdir().unwrap();
        let mut driven = Driven::new();
        let client = driven.open(dir.path(), 1, vec![1, 2, 3], None);
        let handle = client.handle();
        let until = driven.until();
        let evaluated = Arc::new(AtomicUsize::new(0));
        let mut replies = Vec::new();
        let mut refusal = None;
        for _ in 0..COMMANDS {
            let counted = Arc::clone(&evaluated);
            let operation = Operation::Call(Mutex::new(Some(Box::new(move |_| {
                counted.fetch_add(1, Ordering::Relaxed);
                Ok(Box::new(Vec::<u8>::new()) as Box<dyn Any + Send>)
            }))));
            let (response, receiver) = mpsc::sync_channel(1);
            match handle.enqueue(
                Input::Submit {
                    namespace: String::new(),
                    operation,
                    until,
                    response,
                },
                api::REQUEST_MAX,
            ) {
                Ok(()) => replies.push(receiver),
                Err(error) => {
                    refusal = Some(error);
                    break;
                }
            }
        }
        assert_eq!(evaluated.load(Ordering::Relaxed), 0);
        let mut bytes = Vec::new();
        Message {
            msg_type: hyper_raft::proto::MessageType::MsgHeartbeat,
            from: 2,
            to: 1,
            term: 1,
            ..Message::default()
        }
        .encode(&mut bytes);
        let mut heartbeat = handle.admit_receive(2, bytes, until).unwrap();
        driven.step();
        assert!(matches!(heartbeat.try_result(), Ok(Some(()))));
        assert!(matches!(
            refusal,
            Some(Refusal::Full { limit }) if limit == QUEUED_BYTES - WIRE_BYTES
        ));
        assert_eq!(
            replies.len(),
            (QUEUED_BYTES - WIRE_BYTES) / api::REQUEST_MAX
        );
        assert!(!replies.is_empty());
        assert_eq!(evaluated.load(Ordering::Relaxed), 0);
    }

    /// U15/CP-T1.8 on driven time: a real DeviceFile log, a product-driven election, a once-only
    /// capture and bounded GC by the same owner, all kept through a durable restart. Do: a single
    /// voter captures a random value, puts and deletes a record, compacts, then restarts. Expect:
    /// the closure run once, the value and the deletion's change back after the restart, and a
    /// second compaction with nothing left to visit.
    #[test]
    fn single_voter_actor_captures_once_and_reopens_durable_random_value() {
        let dir = tempfile::tempdir().unwrap();
        let mut driven = Driven::new();
        let clients = vec![driven.open(dir.path(), 1, vec![1], None)];
        let mut pump = Pump::new(&clients);
        driven.elect(&mut pump, &clients, 0, 1);
        let client = clients.first().unwrap();
        let evaluated = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&evaluated);
        let chosen = driven.call(&mut pump, client, move |db| {
            count.fetch_add(1, Ordering::Relaxed);
            db.exec("CREATE TABLE actor_items(value BLOB DEFAULT(randomblob(32))); INSERT INTO actor_items DEFAULT VALUES")?;
            value(db)
        }).unwrap();
        assert_eq!(evaluated.load(Ordering::Relaxed), 1);
        assert_eq!(driven.call(&mut pump, client, value).unwrap(), chosen);
        // Bounded GC is captured by this same owner and survives its durable restart.
        for writes in [
            vec![Write::Put {
                family: "items".into(),
                key: b"retired".to_vec(),
                value: b"old".to_vec(),
            }],
            vec![Write::Delete {
                family: "items".into(),
                key: b"retired".to_vec(),
            }],
        ] {
            let records = Operation::Mutation(Mutation::Records {
                namespace: "gc".into(),
                tx: Tx {
                    conds: vec![],
                    writes,
                },
            });
            let answer = enqueue(client, driven.until(), "gc", records);
            driven.answer(&mut pump, &answer).unwrap();
        }
        let step = Operation::Mutation(Mutation::CompactStep {
            namespace: "gc".into(),
            before: 2,
        });
        let answer = enqueue(client, driven.until(), "gc", step);
        let Answer::Compacted(progress) = driven.answer(&mut pump, &answer).unwrap() else {
            panic!("wrong compaction answer")
        };
        assert_eq!(progress.completed, 2);
        assert_eq!(progress.feed, 1);
        let applied = client.handle().status().unwrap().applied;
        // Past hyper-durable's quiet period, so the owner states its durable commit before it stops.
        driven.advance(0, Duration::from_millis(60));
        for _ in 0..8 {
            driven.round(&mut pump);
        }
        driven.drain(&mut pump);
        pump.finish();
        drop(clients);
        drop(driven);

        let mut driven = Driven::new();
        let reopened = vec![driven.open(dir.path(), 1, vec![1], None)];
        let mut pump = Pump::new(&reopened);
        driven.elect(&mut pump, &reopened, 0, applied);
        let client = reopened.first().unwrap();
        assert!(client.handle().status().unwrap().applied >= applied);
        assert_eq!(driven.call(&mut pump, client, value).unwrap(), chosen);
        let changes = enqueue(
            client,
            driven.until(),
            "gc",
            Operation::Changes("items".into(), 1, 10),
        );
        let Answer::Changes(changes) = driven.answer(&mut pump, &changes).unwrap() else {
            panic!("wrong changes answer")
        };
        assert_eq!(changes.changes.len(), 1);
        assert_eq!(changes.changes[0].value, None);
        let step = Operation::Mutation(Mutation::CompactStep {
            namespace: "gc".into(),
            before: 2,
        });
        let answer = enqueue(client, driven.until(), "gc", step);
        let Answer::Compacted(progress) = driven.answer(&mut pump, &answer).unwrap() else {
            panic!("wrong recovered compaction answer")
        };
        assert_eq!(progress.completed, 2);
        assert_eq!(progress.visited, 0);
        assert_eq!(evaluated.load(Ordering::Relaxed), 1);
        driven.drain(&mut pump);
        pump.finish();
    }
    struct PumpPacket {
        from: u64,
        message: Outbound,
        until: Instant,
    }
    struct PumpReceipt {
        receipt: ReceiveReceipt,
        bytes: usize,
        proposal: bool,
        from: u64,
    }
    enum PumpPhase {
        Relay,
        Captures {
            leader: usize,
            held: [Option<PumpPacket>; 2],
        },
    }
    /// CP-T1.8: mirror the carrier's bounded admission and observe every processed receipt.
    /// Admission never waits for another owner. All records, including retained proposals,
    /// share COMMANDS/QUEUED_BYTES bounds; each keeps its first absolute receive deadline.
    struct Pump<'a> {
        clients: &'a [Client],
        receipts: [Option<PumpReceipt>; COMMANDS],
        phase: PumpPhase,
        next_source: usize,
        records: usize,
        bytes: usize,
        // A nemesis's elections: a leader handing off its lead refuses the proposals forwarded
        // to it, and those refusals are counted here instead of failing the test.
        refusals: Option<usize>,
        // Members removed from the group: what they sent before the removal reaches a leader that
        // no longer counts them, which refuses it as a non-member's.
        departed: Vec<u64>,
        // Records admitted to each member, by its identity.
        delivered: BTreeMap<u64, usize>,
        // Members cut off: every record to one is lost, as on a link that is down.
        lost: Vec<u64>,
    }
    impl<'a> Pump<'a> {
        fn new(clients: &'a [Client]) -> Self {
            assert!(!clients.is_empty() && clients.len() <= MEMBERS);
            Self {
                clients,
                receipts: std::array::from_fn(|_| None),
                phase: PumpPhase::Relay,
                next_source: 0,
                records: 0,
                bytes: 0,
                refusals: None,
                departed: Vec::new(),
                delivered: BTreeMap::new(),
                lost: Vec::new(),
            }
        }
        fn reap(&mut self) {
            for slot in &mut self.receipts {
                let Some(pending) = slot.as_mut() else {
                    continue;
                };
                let result = pending.receipt.try_result();
                if matches!(result, Ok(None)) {
                    continue;
                }
                let pending = slot.take().unwrap();
                self.records -= 1;
                self.bytes -= pending.bytes;
                if let (Some(refusals), true, Err(Refusal::Unavailable(why))) =
                    (self.refusals.as_mut(), pending.proposal, &result)
                    && why.contains("transferring its leadership")
                {
                    *refusals += 1;
                    continue;
                }
                if let Err(Refusal::Invalid { why, .. }) = &result
                    && why == "sender is not a current member"
                    && self.departed.contains(&pending.from)
                {
                    continue;
                }
                // Admission was not an ACK: expiry, validation and fencing must fail the test.
                assert_eq!(result.unwrap(), Some(()));
            }
        }
        fn admit(&mut self, packet: PumpPacket) {
            let peer = self
                .clients
                .get(usize::try_from(packet.message.to - 1).unwrap())
                .unwrap();
            let bytes = packet.message.bytes.len();
            let proposal = Message::decode(&packet.message.bytes).unwrap().msg_type
                == hyper_raft::proto::MessageType::MsgPropose;
            *self.delivered.entry(packet.message.to).or_default() += 1;
            let receipt = peer
                .handle()
                .admit_receive(packet.from, packet.message.bytes, packet.until)
                .unwrap();
            let slot = self
                .receipts
                .iter_mut()
                .find(|slot| slot.is_none())
                .unwrap();
            *slot = Some(PumpReceipt {
                receipt,
                bytes,
                proposal,
                from: packet.from,
            });
        }
        /// One relay round, each record admitted with `until` as its receive deadline.
        fn drive_until(&mut self, until: Instant) -> usize {
            self.reap();
            let first = self.next_source;
            self.next_source = (first + 1) % self.clients.len();
            let mut admitted = 0;
            // U15/CP-T1.8: match the host carrier's bounded 16-record quantum. One
            // record per origin each round preserves fairness and the held-origin barrier;
            // control traffic must not consume the entire relay service opportunity.
            for _ in 0..16 {
                let mut drained = false;
                for offset in 0..self.clients.len() {
                    // Reserve room for any legal wire record before draining ownership from a peer.
                    // The conservative byte check needs no unbounded overflow/backpressure buffer.
                    if self.records == COMMANDS || self.bytes + WIRE_BYTES > QUEUED_BYTES {
                        break;
                    }
                    let index = (first + offset) % self.clients.len();
                    let from = u64::try_from(index + 1).unwrap();
                    // No later record from this origin may overtake its retained proposal.
                    if let PumpPhase::Captures { held, .. } = &self.phase
                        && held.iter().flatten().any(|packet| packet.from == from)
                    {
                        continue;
                    }
                    for message in self.clients.get(index).unwrap().handle().drain(1).unwrap() {
                        drained = true;
                        if self.lost.contains(&message.to) {
                            continue;
                        }
                        api::bound("fixture wire record", message.bytes.len(), WIRE_BYTES).unwrap();
                        let packet = PumpPacket {
                            from,
                            message,
                            until,
                        };
                        self.records += 1;
                        self.bytes += packet.message.bytes.len();
                        if let PumpPhase::Captures { leader, held } = &mut self.phase {
                            let record = Message::decode(&packet.message.bytes).unwrap();
                            if record.msg_type == hyper_raft::proto::MessageType::MsgPropose {
                                assert_ne!(index, *leader);
                                assert_eq!(packet.message.to, u64::try_from(*leader + 1).unwrap());
                                assert!(held.iter().flatten().all(|other| other.from != from));
                                *held.iter_mut().find(|slot| slot.is_none()).unwrap() =
                                    Some(packet);
                                continue;
                            }
                        }
                        self.admit(packet);
                        admitted += 1;
                    }
                }
                if !drained {
                    break;
                }
            }
            self.reap();
            admitted
        }
        fn begin_captures(&mut self, leader: usize) {
            assert!(matches!(self.phase, PumpPhase::Relay));
            self.phase = PumpPhase::Captures {
                leader,
                held: [None, None],
            };
        }
        fn captures_ready(&self) -> bool {
            matches!(&self.phase, PumpPhase::Captures { held, .. } if held.iter().all(Option::is_some))
        }
        fn release_captures(&mut self) {
            assert!(self.captures_ready());
            let PumpPhase::Captures { held, .. } =
                std::mem::replace(&mut self.phase, PumpPhase::Relay)
            else {
                panic!("capture phase disappeared")
            };
            for packet in held {
                self.admit(packet.unwrap());
            }
            self.reap();
        }
        fn finish(mut self) {
            assert!(
                matches!(self.phase, PumpPhase::Relay),
                "retained proposals were not released"
            );
            // No new records or grace deadline: try_result enforces each admitted until.
            while self.records != 0 {
                self.reap();
                if self.records != 0 {
                    std::thread::park_timeout(POLL);
                }
            }
            assert_eq!(self.bytes, 0);
            assert!(self.receipts.iter().all(Option::is_none));
        }
    }
    impl Drop for Pump<'_> {
        fn drop(&mut self) {
            // Never wait in Drop, including failure unwinding. Success requires explicit finish.
            if !std::thread::panicking() {
                assert_eq!(
                    self.records, 0,
                    "fixture pump dropped with unsettled records"
                );
                assert_eq!(self.bytes, 0);
                assert!(self.receipts.iter().all(Option::is_none));
                assert!(matches!(self.phase, PumpPhase::Relay));
            }
        }
    }

    /// Rounds a driven group may take for one call, election or settling before the test calls
    /// it hung. A round with nothing to relay yields `POLL` to the disk, so this is a counted hang
    /// guard of about a minute of disk waits (CLAUDE.md section 2), never a verdict on time.
    const ROUNDS: usize = 60_000;

    /// A test's own clocks and the owners it steps (replication.md, "The owner's time"). Each
    /// owner reads its own clock, as a machine reads its own, and only the test moves it: between
    /// steps, or from a call's work while the owner runs it. Its instants start an hour past the
    /// real clock, so a real-clock check of a test deadline (a receipt's poll) holds for an hour:
    /// a hang guard, never a verdict.
    struct Driven {
        owners: Vec<(Actor, mpsc::Receiver<Envelope>)>,
        base: Instant,
        // Each owner's clock: the nanoseconds it reads past `base`.
        clocks: Vec<Arc<AtomicU64>>,
    }
    /// The time a driven clock reads past its shell's base.
    fn offset(clock: &AtomicU64) -> Duration {
        Duration::from_nanos(clock.load(Ordering::Acquire))
    }
    /// Moves a driven clock to `to`, its shell's `base` plus some offset.
    fn set(clock: &AtomicU64, base: Instant, to: Instant) {
        let nanos = u64::try_from(to.duration_since(base).as_nanos()).unwrap();
        clock.store(nanos, Ordering::Release);
    }
    impl Driven {
        fn new() -> Self {
            Self {
                owners: Vec::new(),
                base: Instant::now() + Duration::from_secs(3600),
                clocks: Vec::new(),
            }
        }
        /// A call's or a record's budget on these clocks: it runs out only if the test moves a
        /// clock past it.
        fn until(&self) -> Instant {
            self.base
                + self
                    .clocks
                    .iter()
                    .map(|clock| offset(clock))
                    .max()
                    .unwrap_or_default()
                + Duration::from_secs(5)
        }
        fn advance(&mut self, owner: usize, by: Duration) {
            let clock = self.clocks.get(owner).unwrap();
            set(clock, self.base, self.base + offset(clock) + by);
        }
        /// Owner `owner`'s clock, for a call's work to move while the owner runs it.
        fn clock(&self, owner: usize) -> Arc<AtomicU64> {
            Arc::clone(self.clocks.get(owner).unwrap())
        }
        fn open(
            &mut self,
            dir: &Path,
            node_id: u64,
            voters: Vec<u64>,
            bootstrap: Option<api::replication::Bootstrap>,
        ) -> Client {
            self.open_member(dir, node_id, voters, Vec::new(), bootstrap)
        }
        fn open_member(
            &mut self,
            dir: &Path,
            node_id: u64,
            voters: Vec<u64>,
            learners: Vec<u64>,
            bootstrap: Option<api::replication::Bootstrap>,
        ) -> Client {
            let path = dir.join(format!("state-{node_id}.db"));
            let mut lease = None;
            for _ in 0..ROUNDS {
                match exclusive_file(&path) {
                    Ok(file) => {
                        lease = Some(Arc::new(file));
                        break;
                    }
                    Err(_) => std::thread::park_timeout(POLL),
                }
            }
            let lease = lease.expect("the previous owner kept its lease");
            let reading = Arc::new(AtomicU64::new(0));
            let (base, read) = (self.base, Arc::clone(&reading));
            let (client, actor, receiver) = super::open(
                &path,
                &dir.join(format!("raft-{node_id}.log")),
                ReplicaConfig {
                    node_id,
                    voters,
                    learners,
                    bootstrap,
                },
                Arc::new(Changed::default()),
                lease,
                self.until(),
                Arc::new(move || base + offset(&read)),
            )
            .unwrap();
            self.owners.push((actor, receiver));
            self.clocks.push(reading);
            client
        }
        /// One round: every owner takes each input waiting for it, then steps once more, each at
        /// its own clock's reading.
        fn step(&mut self) {
            for (owner, inputs) in &mut self.owners {
                while let Ok(input) = inputs.try_recv() {
                    owner.run_step(Some(input)).unwrap();
                }
                owner.run_step(None).unwrap();
            }
        }
        /// One relay round and one step; a round with nothing to relay yields `POLL` to the disk.
        fn round(&mut self, pump: &mut Pump<'_>) {
            if pump.drive_until(self.until()) == 0 {
                std::thread::park_timeout(POLL);
            }
            self.step();
        }
        /// Owner `first` campaigns now, every clock standing still, and rounds go on until every
        /// owner follows one leader with at least `applied` applied. Returns the leader's index.
        fn elect(
            &mut self,
            pump: &mut Pump<'_>,
            clients: &[Client],
            first: usize,
            applied: u64,
        ) -> usize {
            self.owners
                .get_mut(first)
                .unwrap()
                .0
                .replica
                .campaign()
                .unwrap();
            self.follow(pump, clients, applied)
        }
        /// Rounds, time standing still, until every owner follows one leader with at least
        /// `applied` applied. Returns the leader's index.
        fn follow(&mut self, pump: &mut Pump<'_>, clients: &[Client], applied: u64) -> usize {
            for _ in 0..ROUNDS {
                self.round(pump);
                let states: Vec<_> = clients
                    .iter()
                    .map(|c| c.handle().status().unwrap())
                    .collect();
                let leader = states.first().unwrap().leader;
                if leader != 0
                    && states
                        .iter()
                        .all(|s| s.leader == leader && s.applied >= applied)
                {
                    return usize::try_from(leader - 1).unwrap();
                }
            }
            panic!("no leader followed in {ROUNDS} driven rounds");
        }
        /// The lead moves from owner `from` to owner `to` now (`Replica::transfer`, which no lease
        /// refuses), every clock standing still; rounds go on until every owner follows `to`.
        fn transfer(&mut self, pump: &mut Pump<'_>, clients: &[Client], from: usize, to: usize) {
            let id = u64::try_from(to + 1).unwrap();
            self.owners
                .get_mut(from)
                .unwrap()
                .0
                .replica
                .transfer(id)
                .unwrap();
            for _ in 0..ROUNDS {
                self.round(pump);
                if clients
                    .iter()
                    .all(|c| c.handle().status().unwrap().leader == id)
                {
                    return;
                }
            }
            panic!("the lead did not move to {id} in {ROUNDS} driven rounds");
        }
        /// Owner `from` hands its lead to owner `to`, every clock standing still; rounds go on until
        /// every one of `clients` follows one leader, whose index it returns. The handing-off can
        /// lose its election: a change's way out, written after `to` was told to campaign, is an
        /// entry `to` lacks and the voters holding it refuse it their votes. An election timeout
        /// settles that on a running owner; here, with nobody leading and nothing moving, each
        /// voter in turn campaigns.
        fn hand_off(
            &mut self,
            pump: &mut Pump<'_>,
            clients: &[Client],
            from: usize,
            to: usize,
        ) -> usize {
            let id = u64::try_from(to + 1).unwrap();
            let term = self.owners[from].0.replica.term();
            self.owners[from].0.replica.transfer(id).unwrap();
            let (mut still, mut voter) = (0, 0);
            for _ in 0..ROUNDS {
                let relayed = pump.drive_until(self.until());
                if relayed == 0 {
                    std::thread::park_timeout(POLL);
                }
                self.step();
                let states: Vec<_> = clients
                    .iter()
                    .map(|c| c.handle().status().unwrap())
                    .collect();
                let leader = states[0].leader;
                if leader != 0 && states.iter().all(|s| s.leader == leader && s.term > term) {
                    return usize::try_from(leader - 1).unwrap();
                }
                still = if relayed == 0 { still + 1 } else { 0 };
                if still >= 64 && states.iter().all(|s| s.leader == 0) {
                    still = 0;
                    // One that can't lead refuses, and the next is asked.
                    let _ = self.owners[voter % clients.len()].0.replica.campaign();
                    voter += 1;
                }
            }
            panic!("no lead after the hand-off in {ROUNDS} driven rounds");
        }
        /// Rounds, time standing still, until `answer` comes.
        fn answer<T>(
            &mut self,
            pump: &mut Pump<'_>,
            answer: &mpsc::Receiver<Result<T, Refusal>>,
        ) -> Result<T, Refusal> {
            for _ in 0..ROUNDS {
                match answer.try_recv() {
                    Ok(result) => return result,
                    Err(mpsc::TryRecvError::Empty) => self.round(pump),
                    Err(error) => panic!("owner ended: {error}"),
                }
            }
            panic!("a driven operation was unanswered after {ROUNDS} rounds");
        }
        /// Rounds, time standing still, until the call's `answer` comes.
        fn settle(
            &mut self,
            pump: &mut Pump<'_>,
            answer: &mpsc::Receiver<Result<Answer, Refusal>>,
        ) -> Result<Vec<u8>, Refusal> {
            self.answer(pump, answer).map(called)
        }
        fn call(
            &mut self,
            pump: &mut Pump<'_>,
            client: &Client,
            f: impl FnOnce(&mut Db) -> anyhow::Result<Vec<u8>> + Send + 'static,
        ) -> Result<Vec<u8>, Refusal> {
            let answer = submit(client, self.until(), f);
            self.settle(pump, &answer)
        }
        /// Rounds until every record the relay admitted has been taken, so the pump can finish.
        fn drain(&mut self, pump: &mut Pump<'_>) {
            for _ in 0..ROUNDS {
                pump.reap();
                if pump.records == 0 {
                    return;
                }
                self.step();
            }
            panic!("admitted records still untaken after {ROUNDS} rounds");
        }
    }
    /// Enqueues `operation` at `client` without waiting: its answer comes on the returned receiver.
    fn enqueue(
        client: &Client,
        until: Instant,
        namespace: &str,
        operation: Operation,
    ) -> mpsc::Receiver<Result<Answer, Refusal>> {
        let (response, receiver) = mpsc::sync_channel(1);
        client
            .handle
            .enqueue(
                Input::Submit {
                    namespace: namespace.into(),
                    operation,
                    until,
                    response,
                },
                api::REQUEST_MAX,
            )
            .unwrap();
        receiver
    }
    /// Enqueues the call `f` at `client` without waiting.
    fn submit(
        client: &Client,
        until: Instant,
        f: impl FnOnce(&mut Db) -> anyhow::Result<Vec<u8>> + Send + 'static,
    ) -> mpsc::Receiver<Result<Answer, Refusal>> {
        let operation = Operation::Call(Mutex::new(Some(Box::new(move |db| {
            Ok(Box::new(f(db)?) as Box<dyn Any + Send>)
        }))));
        enqueue(client, until, "", operation)
    }
    fn called(answer: Answer) -> Vec<u8> {
        match answer {
            Answer::Called(value) => *value.downcast::<Vec<u8>>().unwrap(),
            _ => panic!("wrong actor reply"),
        }
    }
    fn terms(clients: &[Client]) -> Vec<u64> {
        clients
            .iter()
            .map(|c| c.handle().status().unwrap().term)
            .collect()
    }

    /// U15/CP-T1.8 on driven time (replication.md, "The owner's time"): a call whose own work
    /// outlasts its deadline is refused and never proposed. Do: the call's work inserts a row,
    /// then moves its owner's clock to the call's deadline, as slow work moves the real clock.
    /// Expect: Timeout, no row on any owner, and no election.
    #[test]
    fn a_call_whose_work_outlasts_its_deadline_is_refused_and_never_proposed() {
        let dir = tempfile::tempdir().unwrap();
        let mut driven = Driven::new();
        let clients: Vec<Client> = (1..=3)
            .map(|n| driven.open(dir.path(), n, vec![1, 2, 3], None))
            .collect();
        let mut pump = Pump::new(&clients);
        let leader = driven.elect(&mut pump, &clients, 0, 1);
        let elected = terms(&clients);
        let origin = clients.get(leader).unwrap();
        driven
            .call(&mut pump, origin, |db| {
                db.exec("CREATE TABLE slow_items(value)")?;
                Ok(Vec::new())
            })
            .unwrap();
        // The call's deadline is a second past its owner's clock, inside the relay's budget for
        // the records it carries, so the clock reaching it expires the call alone.
        let (clock, base) = (driven.clock(leader), driven.base);
        let until = base + offset(&clock) + Duration::from_secs(1);
        let answer = submit(origin, until, move |db| {
            db.exec("INSERT INTO slow_items VALUES(1)")?;
            set(&clock, base, until);
            Ok(Vec::new())
        });
        assert_eq!(driven.settle(&mut pump, &answer), Err(Refusal::Timeout));
        for client in &clients {
            let rows = driven
                .call(&mut pump, client, |db| {
                    let mut count = 0;
                    db.query("SELECT count(*) FROM slow_items", [], |row| {
                        count = row.get::<i64>(0).map_err(unavailable)?;
                        Ok(false)
                    })?;
                    Ok(vec![u8::try_from(count)?])
                })
                .unwrap();
            assert_eq!(rows, vec![0]);
        }
        assert_eq!(
            terms(&clients),
            elected,
            "an election happened during the scenario"
        );
        driven.drain(&mut pump);
        pump.finish();
    }

    /// How many tables named cancelled_actor_write the database has.
    fn cancelled_tables(db: &mut Db) -> anyhow::Result<u8> {
        let mut count = 0;
        db.query(
            "SELECT count(*) FROM sqlite_schema WHERE name='cancelled_actor_write'",
            [],
            |row| {
                count = row.get::<i64>(0).map_err(unavailable)?;
                Ok(false)
            },
        )?;
        Ok(u8::try_from(count)?)
    }

    /// U15/CP-T1.8 on driven time: a SQL deadline interrupts execution, rolls back its capture
    /// and frees the owner. Do: the call's work creates a table, moves its owner's clock to the
    /// call's deadline, as long work moves the real clock, then runs a query of ten million rows.
    /// Expect: the query interrupted at its first deadline check, Timeout, the table rolled back,
    /// and the owner's next call answered.
    #[test]
    fn expired_sql_rolls_back_before_actor_continues() {
        let dir = tempfile::tempdir().unwrap();
        let mut driven = Driven::new();
        let clients = vec![driven.open(dir.path(), 1, vec![1], None)];
        let mut pump = Pump::new(&clients);
        driven.elect(&mut pump, &clients, 0, 1);
        let ran = Arc::new(AtomicUsize::new(0));
        let created = Arc::new(AtomicUsize::new(0));
        let interrupted = Arc::new(AtomicUsize::new(0));
        let (counted, made, stopped) = (
            Arc::clone(&ran),
            Arc::clone(&created),
            Arc::clone(&interrupted),
        );
        let (clock, base, until) = (driven.clock(0), driven.base, driven.until());
        let answer = submit(clients.first().unwrap(), until, move |db| {
            counted.fetch_add(1, Ordering::Relaxed);
            db.exec("CREATE TABLE cancelled_actor_write(value)")?;
            made.store(usize::from(cancelled_tables(db)?), Ordering::Relaxed);
            set(&clock, base, until);
            // Through the row runner, which reports an interrupt as one (the batch runner
            // reports it as busy).
            let result = db.query_owned("WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000000) SELECT sum(x) FROM n", [], |_| Ok(true));
            if matches!(&result, Err(error) if format!("{error:#}").contains("interrupt")) {
                stopped.fetch_add(1, Ordering::Relaxed);
            }
            result?;
            Ok(Vec::new())
        });
        assert_eq!(driven.settle(&mut pump, &answer), Err(Refusal::Timeout));
        assert_eq!(ran.load(Ordering::Relaxed), 1);
        assert_eq!(created.load(Ordering::Relaxed), 1);
        assert_eq!(interrupted.load(Ordering::Relaxed), 1);
        let retained = driven
            .call(&mut pump, clients.first().unwrap(), |db| {
                Ok(vec![cancelled_tables(db)?])
            })
            .unwrap();
        assert_eq!(retained, vec![0]);
        driven.drain(&mut pump);
        pump.finish();
    }

    /// U15/CP-T1.8 on driven time (replication.md, "The owner's time"): follower calls use the
    /// real leader's ReadIndex and proposal forwarding, and no election happens during the scenario.
    #[test]
    fn follower_origin_captures_once_and_reads_after_local_durable_apply() {
        let dir = tempfile::tempdir().unwrap();
        let mut driven = Driven::new();
        let clients: Vec<Client> = (1..=3)
            .map(|n| driven.open(dir.path(), n, vec![1, 2, 3], None))
            .collect();
        let mut pump = Pump::new(&clients);
        let leader = driven.elect(&mut pump, &clients, 0, 1);
        let elected = terms(&clients);
        let origin = (leader + 1) % 3;
        let evaluated = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&evaluated);
        let chosen = driven
            .call(&mut pump, clients.get(origin).unwrap(), move |db| {
                count.fetch_add(1, Ordering::Relaxed);
                db.exec("CREATE TABLE actor_items(value BLOB DEFAULT(randomblob(32))); INSERT INTO actor_items DEFAULT VALUES")?;
                value(db)
            })
            .unwrap();
        assert_eq!(evaluated.load(Ordering::Relaxed), 1);
        assert_eq!(
            driven
                .call(&mut pump, clients.get((origin + 1) % 3).unwrap(), value)
                .unwrap(),
            chosen
        );
        assert_eq!(evaluated.load(Ordering::Relaxed), 1);

        // Both forwarded proposals are held, so both captures use the same committed base; exactly
        // one page set may apply and the stale one's answer is refused.
        let captures = Arc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
        pump.begin_captures(leader);
        let answers: Vec<_> = (1..=2usize)
            .map(|offset| {
                let captures = Arc::clone(&captures);
                submit(
                    clients.get((leader + offset) % 3).unwrap(),
                    driven.until(),
                    move |db| {
                        captures[offset - 1].fetch_add(1, Ordering::Relaxed);
                        db.exec("INSERT INTO actor_items(value) VALUES(x'01')")?;
                        Ok(vec![1])
                    },
                )
            })
            .collect();
        let mut outcomes: Vec<Option<Result<Vec<u8>, Refusal>>> = vec![None, None];
        for _ in 0..ROUNDS {
            for (answer, outcome) in answers.iter().zip(outcomes.iter_mut()) {
                if outcome.is_none()
                    && let Ok(result) = answer.try_recv()
                {
                    *outcome = Some(result.map(called));
                }
            }
            if outcomes.iter().all(Option::is_some) {
                break;
            }
            driven.round(&mut pump);
            if pump.captures_ready() {
                assert!(
                    captures
                        .iter()
                        .all(|count| count.load(Ordering::Relaxed) == 1)
                );
                pump.release_captures();
            }
        }
        let outcomes: Vec<_> = outcomes.into_iter().map(Option::unwrap).collect();
        assert!(matches!(pump.phase, PumpPhase::Relay));
        assert!(
            captures
                .iter()
                .all(|count| count.load(Ordering::Relaxed) == 1)
        );
        assert_eq!(
            outcomes.iter().filter(|r| r.is_ok()).count(),
            1,
            "{outcomes:?}"
        );
        assert_eq!(outcomes.iter().filter(|r| matches!(r, Err(Refusal::Invalid { what, .. }) if what == "prepared transaction")).count(), 1, "{outcomes:?}");
        let count = driven
            .call(&mut pump, clients.get(origin).unwrap(), |db| {
                db.exec("INSERT INTO actor_items(value) VALUES(x'03')")?;
                let mut count = 0;
                db.query("SELECT count(*) FROM actor_items", [], |row| {
                    count = row.get::<i64>(0).map_err(unavailable)?;
                    Ok(false)
                })?;
                Ok(vec![u8::try_from(count)?])
            })
            .unwrap();
        assert_eq!(count, vec![3]);
        assert_eq!(
            terms(&clients),
            elected,
            "an election happened during the scenario"
        );
        driven.drain(&mut pump);
        pump.finish();
    }

    /// U15/CP-T1.8 under a nemesis (replication.md, "The owner's time"): the follower-origin
    /// scenario with the lead moved to another owner at each of its steps: while its first call
    /// and the read after it are in flight, as the held captures go out, after the first capture
    /// answers, and before the last call. Whatever the election: never two Oks for the two
    /// captures and never two of their rows, every answer Ok, the stale capture's refusal or
    /// UnknownOutcome, and a capture answered Ok has its row.
    #[test]
    fn follower_origin_captures_hold_through_an_election_at_each_step() {
        for position in 0..5 {
            let dir = tempfile::tempdir().unwrap();
            let mut driven = Driven::new();
            let clients: Vec<Client> = (1..=3)
                .map(|n| driven.open(dir.path(), n, vec![1, 2, 3], None))
                .collect();
            let mut pump = Pump::new(&clients);
            pump.refusals = Some(0);
            let leader = driven.elect(&mut pump, &clients, 0, 1);
            let origin = (leader + 1) % 3;
            // Positions 3 and 4: the lead moves while the first call, then the read after it, is
            // in flight. Each answers Ok or UnknownOutcome, and the table holds what an Ok says.
            let create = submit(clients.get(origin).unwrap(), driven.until(), |db| {
                db.exec("CREATE TABLE actor_items(value BLOB DEFAULT(randomblob(32))); INSERT INTO actor_items DEFAULT VALUES")?;
                value(db)
            });
            if position == 3 {
                driven.transfer(&mut pump, &clients, leader, origin);
            }
            let created = driven.settle(&mut pump, &create);
            assert!(
                matches!(created, Ok(ref v) if v.len() == 32)
                    || matches!(created, Err(Refusal::UnknownOutcome)),
                "position {position}: {created:?}"
            );
            if position >= 3 {
                let read = submit(
                    clients.get((origin + 1) % 3).unwrap(),
                    driven.until(),
                    |db| {
                        let mut tables = 0;
                        db.query(
                            "SELECT count(*) FROM sqlite_schema WHERE name = 'actor_items'",
                            [],
                            |row| {
                                tables = row.get::<i64>(0).map_err(unavailable)?;
                                Ok(false)
                            },
                        )?;
                        Ok(if tables == 1 { value(db)? } else { Vec::new() })
                    },
                );
                if position == 4 {
                    let now = usize::try_from(
                        clients.first().unwrap().handle().status().unwrap().leader - 1,
                    )
                    .unwrap();
                    driven.transfer(&mut pump, &clients, now, (now + 1) % 3);
                }
                let after = driven.settle(&mut pump, &read);
                match (&created, &after) {
                    (Ok(chosen), Ok(seen)) => assert_eq!(seen, chosen, "position {position}"),
                    (Err(_), Ok(seen)) => assert!(
                        seen.is_empty() || seen.len() == 32,
                        "position {position}: {seen:?}"
                    ),
                    (_, Err(refusal)) => assert!(
                        matches!(refusal, Refusal::UnknownOutcome),
                        "position {position}: {refusal:?}"
                    ),
                }
                driven.drain(&mut pump);
                pump.finish();
                continue;
            }
            assert!(created.is_ok(), "position {position}: {created:?}");
            pump.begin_captures(leader);
            let answers: Vec<_> = (1..=2usize)
                .map(|offset| {
                    submit(
                        clients.get((leader + offset) % 3).unwrap(),
                        driven.until(),
                        |db| {
                            db.exec("INSERT INTO actor_items(value) VALUES(x'01')")?;
                            Ok(vec![1])
                        },
                    )
                })
                .collect();
            let mut outcomes: Vec<Option<Result<Vec<u8>, Refusal>>> = vec![None, None];
            let mut moved = false;
            for _ in 0..ROUNDS {
                for (answer, outcome) in answers.iter().zip(outcomes.iter_mut()) {
                    if outcome.is_none()
                        && let Ok(result) = answer.try_recv()
                    {
                        *outcome = Some(result.map(called));
                    }
                }
                if outcomes.iter().all(Option::is_some) {
                    break;
                }
                if position == 1 && !moved && outcomes.iter().any(Option::is_some) {
                    driven.transfer(&mut pump, &clients, leader, origin);
                    moved = true;
                }
                driven.round(&mut pump);
                if pump.captures_ready() {
                    pump.release_captures();
                    if position == 0 {
                        driven.transfer(&mut pump, &clients, leader, origin);
                        moved = true;
                    }
                }
            }
            let outcomes: Vec<_> = outcomes.into_iter().map(Option::unwrap).collect();
            for outcome in &outcomes {
                assert!(
                    matches!(outcome, Ok(v) if v == &vec![1])
                        || matches!(outcome, Err(Refusal::Invalid { what, .. }) if what == "prepared transaction")
                        || matches!(outcome, Err(Refusal::UnknownOutcome)),
                    "position {position}: {outcomes:?}"
                );
            }
            let oks = outcomes.iter().filter(|r| r.is_ok()).count();
            assert!(oks <= 1, "position {position}: {outcomes:?}");
            if position == 2 {
                let now = clients
                    .iter()
                    .map(|c| c.handle().status().unwrap().leader)
                    .next()
                    .unwrap();
                let current = usize::try_from(now - 1).unwrap();
                driven.transfer(&mut pump, &clients, current, (current + 1) % 3);
            }
            let rows = driven
                .call(&mut pump, clients.get(origin).unwrap(), |db| {
                    let mut rows = 0;
                    db.query(
                        "SELECT count(*) FROM actor_items WHERE value = x'01'",
                        [],
                        |row| {
                            rows = row.get::<i64>(0).map_err(unavailable)?;
                            Ok(false)
                        },
                    )?;
                    Ok(vec![u8::try_from(rows)?])
                })
                .unwrap();
            let rows = *rows.first().unwrap();
            assert!(
                rows <= 1,
                "position {position}: {rows} rows for {outcomes:?}"
            );
            if oks == 1 {
                assert_eq!(
                    rows, 1,
                    "position {position}: an Ok capture without its row, {outcomes:?}"
                );
            }
            driven.drain(&mut pump);
            pump.finish();
        }
    }

    /// U15/CP-T1.8: malformed membership and a source that cannot lead are refused at admission.
    #[test]
    fn malformed_initial_membership_is_refused() {
        let valid = ReplicaConfig {
            node_id: 1,
            voters: vec![1, 2, 3],
            learners: Vec::new(),
            bootstrap: None,
        };
        assert!(validate(&valid).is_ok());
        for config in [
            ReplicaConfig {
                node_id: 0,
                ..valid.clone()
            },
            ReplicaConfig {
                voters: Vec::new(),
                ..valid.clone()
            },
            ReplicaConfig {
                voters: vec![1, 1],
                ..valid.clone()
            },
            ReplicaConfig {
                learners: vec![2],
                ..valid.clone()
            },
            ReplicaConfig {
                node_id: 9,
                ..valid.clone()
            },
            ReplicaConfig {
                learners: vec![4],
                bootstrap: Some(api::replication::Bootstrap {
                    source_node: 4,
                    source_id: [0; 32],
                }),
                ..valid.clone()
            },
        ] {
            assert!(matches!(validate(&config), Err(Refusal::Invalid { .. })));
        }
    }

    /// Enqueues a change of members at `client` without waiting: its answer comes on the returned
    /// receiver.
    fn change(
        client: &Client,
        until: Instant,
        change: MemberChange,
    ) -> mpsc::Receiver<Result<ReplicaStatus, Refusal>> {
        let (response, receiver) = mpsc::sync_channel(1);
        client
            .handle
            .enqueue(
                Input::Change {
                    change,
                    until,
                    response,
                },
                CHANGE_BYTES,
            )
            .unwrap();
        receiver
    }
    /// Enqueues a hand-off of the lead to `to` at `client` without waiting: its answer comes on
    /// the returned receiver.
    fn transfer(
        client: &Client,
        until: Instant,
        to: u64,
    ) -> mpsc::Receiver<Result<ReplicaStatus, Refusal>> {
        let (response, receiver) = mpsc::sync_channel(1);
        client
            .handle
            .enqueue(
                Input::Transfer {
                    to,
                    until,
                    response,
                },
                CHANGE_BYTES,
            )
            .unwrap();
        receiver
    }
    /// Writes `value` into member_items through `client`.
    fn write(driven: &mut Driven, pump: &mut Pump<'_>, client: &Client, value: u8) {
        driven
            .call(pump, client, move |db| {
                db.exec("CREATE TABLE IF NOT EXISTS member_items(value)")?;
                db.exec(&format!("INSERT INTO member_items VALUES({value})"))?;
                Ok(Vec::new())
            })
            .unwrap();
    }
    /// The values of member_items that owner `owner` has applied, read on its own state.
    fn held(driven: &mut Driven, owner: usize) -> Vec<u8> {
        let (actor, _) = driven.owners.get_mut(owner).unwrap();
        actor
            .replica
            .machine_mut()
            .read_db(|db| {
                let mut tables = 0;
                db.query(
                    "SELECT count(*) FROM sqlite_schema WHERE name='member_items'",
                    [],
                    |row| {
                        tables = row.get::<i64>(0).map_err(unavailable)?;
                        Ok(false)
                    },
                )?;
                let mut values = Vec::new();
                if tables == 1 {
                    db.query("SELECT value FROM member_items ORDER BY value", [], |row| {
                        values.push(u8::try_from(row.get::<i64>(0).map_err(unavailable)?)?);
                        Ok(true)
                    })?;
                }
                Ok(values)
            })
            .unwrap()
    }
    /// Rounds, time standing still, until owner `owner` has applied all that owner `leader` has.
    fn catch_up(
        driven: &mut Driven,
        pump: &mut Pump<'_>,
        clients: &[Client],
        leader: usize,
        owner: usize,
    ) {
        let applied = |n: usize| clients[n].handle().status().unwrap().applied;
        for _ in 0..ROUNDS {
            if applied(owner) >= applied(leader) {
                return;
            }
            driven.round(pump);
        }
        panic!("owner {owner} did not catch up in {ROUNDS} driven rounds");
    }
    /// Rounds, time standing still, until a round relays nothing and the step after it gives
    /// nothing to relay: what the owners had to send is sent.
    fn quiet(driven: &mut Driven, pump: &mut Pump<'_>) {
        for _ in 0..ROUNDS {
            let relayed = pump.drive_until(driven.until());
            driven.step();
            if relayed == 0 && pump.drive_until(driven.until()) == 0 {
                return;
            }
        }
        panic!("the group did not go quiet in {ROUNDS} driven rounds");
    }
    /// Asks `client` where catching up each learner stands, without waiting: its answer comes on
    /// the returned receiver.
    fn judged(
        client: &Client,
        until: Instant,
    ) -> mpsc::Receiver<Result<Vec<(u64, CatchUp)>, Refusal>> {
        let (response, receiver) = mpsc::sync_channel(1);
        client
            .handle
            .enqueue(Input::CatchUp { until, response }, CHANGE_BYTES)
            .unwrap();
        receiver
    }
    /// The ticks owner 0's core waits, at least, before it campaigns: an election timeout.
    fn leader_core_election_tick(driven: &Driven) -> usize {
        driven.owners[0]
            .0
            .replica
            .core()
            .raft
            .config()
            .election_tick
    }

    /// AC-T1.8.M1 (CP-T1.8) on driven time (replication.md, "Membership"; kiln plan 01a1a283e14c,
    /// its docs/p2p.md P3): the leader says how far each learner has caught up, judged in rounds
    /// (Ongaro's thesis section 4.2.1), for its host to promote only a learner that won't stall
    /// commits. Do: ask the leader and a member to be added before the add; add the member as a
    /// learner and ask the leader as rounds go; ask the learner; promote it and ask the leader
    /// again. Expect: no learner to judge, and NotLeader naming no leader from the member; the
    /// learner Pending, then Ready within the rounds, no time passing; NotLeader naming the leader
    /// from the learner; once it votes, no learner to judge.
    #[test]
    fn the_leader_judges_a_learners_catch_up_in_rounds() {
        let dir = tempfile::tempdir().unwrap();
        let mut driven = Driven::new();
        let clients = vec![
            driven.open(dir.path(), 1, vec![1], None),
            driven.open_member(dir.path(), 2, vec![1], vec![2], None),
        ];
        let mut pump = Pump::new(&clients);
        let (voter, learner) = (&clients[0], &clients[1]);
        driven.elect(&mut pump, &clients[..1], 0, 1);
        write(&mut driven, &mut pump, voter, 1);
        assert_eq!(
            driven.answer(&mut pump, &judged(voter, driven.until())),
            Ok(Vec::new())
        );
        assert_eq!(
            driven.answer(&mut pump, &judged(learner, driven.until())),
            Err(Refusal::NotLeader { leader: None })
        );

        let until = driven.until();
        driven
            .answer(
                &mut pump,
                &change(voter, until, MemberChange::AddLearner(2)),
            )
            .unwrap();
        write(&mut driven, &mut pump, voter, 2);
        let mut stands = Vec::new();
        for _ in 0..ROUNDS {
            stands = driven
                .answer(&mut pump, &judged(voter, driven.until()))
                .unwrap();
            if stands == vec![(2, CatchUp::Ready)] {
                break;
            }
            assert_eq!(stands, vec![(2, CatchUp::Pending)]);
            driven.round(&mut pump);
        }
        assert_eq!(stands, vec![(2, CatchUp::Ready)]);
        catch_up(&mut driven, &mut pump, &clients, 0, 1);
        assert_eq!(held(&mut driven, 1), vec![1, 2]);
        assert_eq!(
            driven.answer(&mut pump, &judged(learner, driven.until())),
            Err(Refusal::NotLeader { leader: Some(1) })
        );

        let until = driven.until();
        let promoted = driven
            .answer(&mut pump, &change(voter, until, MemberChange::Promote(2)))
            .unwrap();
        assert_eq!((promoted.voters, promoted.learners), (vec![1, 2], vec![]));
        assert_eq!(
            driven.answer(&mut pump, &judged(voter, driven.until())),
            Ok(Vec::new())
        );
        driven.drain(&mut pump);
        pump.finish();
    }

    /// AC-T1.8.M1 (CP-T1.8) on driven time (replication.md, "Membership"; kiln plan 01a1a283e14c,
    /// its docs/p2p.md P3): a learner that never answers is given up once its lag hasn't shrunk
    /// over an election timeout, and staged afresh when asked again (Ongaro's thesis section
    /// 4.2.1: the leader aborts for a server that "will never catch up", and "the caller may always
    /// try again"). Do, in a group of one voter: add member 9, which has no owner, so what the
    /// leader sends it is lost; ask where it stands with a deadline already past; let an election
    /// timeout pass on the leader's clock and ask; let another pass and ask, then once more.
    /// Expect: Timeout, staging nothing; Pending; Aborted; Pending.
    #[test]
    fn a_learner_that_never_answers_is_given_up_after_an_election_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let mut driven = Driven::new();
        let clients = vec![driven.open(dir.path(), 1, vec![1], None)];
        let mut pump = Pump::new(&clients);
        driven.elect(&mut pump, &clients, 0, 1);
        write(&mut driven, &mut pump, &clients[0], 1);
        driven.drain(&mut pump);
        pump.finish();
        // Rounds without the pump, what the leader sends lost, until `answer` comes. Each round
        // yields `POLL` to the disk, as a round with nothing to relay does.
        fn lost<T>(
            driven: &mut Driven,
            client: &Client,
            answer: &mpsc::Receiver<Result<T, Refusal>>,
        ) -> Result<T, Refusal> {
            for _ in 0..ROUNDS {
                match answer.try_recv() {
                    Ok(result) => return result,
                    Err(mpsc::TryRecvError::Empty) => {
                        drop(client.handle().drain(OUTGOING_MESSAGES).unwrap());
                        std::thread::park_timeout(POLL);
                        driven.step();
                    }
                    Err(error) => panic!("owner ended: {error}"),
                }
            }
            panic!("a driven operation was unanswered after {ROUNDS} rounds");
        }
        let leader = &clients[0];
        let until = driven.until();
        let added = lost(
            &mut driven,
            leader,
            &change(leader, until, MemberChange::AddLearner(9)),
        )
        .unwrap();
        assert_eq!((added.voters, added.learners), (vec![1], vec![9]));
        // An election timeout on ticks: the core's election_tick, each tick a step past TICK.
        let election = |driven: &mut Driven| {
            for _ in 0..=leader_core_election_tick(driven) {
                driven.advance(0, TICK);
                drop(leader.handle().drain(OUTGOING_MESSAGES).unwrap());
                driven.step();
            }
        };
        // A request whose deadline passed before the owner took it stages nothing: an election
        // timeout later, the learner is staged afresh, not given up.
        let stale = Instant::now();
        assert_eq!(
            lost(&mut driven, leader, &judged(leader, stale)),
            Err(Refusal::Timeout)
        );
        election(&mut driven);
        assert_eq!(
            {
                let until = driven.until();
                lost(&mut driven, leader, &judged(leader, until))
            },
            Ok(vec![(9, CatchUp::Pending)])
        );
        election(&mut driven);
        assert_eq!(
            {
                let until = driven.until();
                lost(&mut driven, leader, &judged(leader, until))
            },
            Ok(vec![(9, CatchUp::Aborted)])
        );
        assert_eq!(
            {
                let until = driven.until();
                lost(&mut driven, leader, &judged(leader, until))
            },
            Ok(vec![(9, CatchUp::Pending)])
        );
    }

    /// AC-T1.8.M1, T-T1.8.M1 (CP-T1.8) on driven time (replication.md, "Membership"; kiln plan
    /// 01a1a283e14c, step 1): a group of one voter adds a learner, writes before and after, then
    /// removes it. Do: write 1 on the voter; add member 2 as a learner; write 2 on the voter and 3
    /// on the learner; remove the learner; write 4 on the voter; then write 5 on the removed
    /// member. Expect: each change answered with the configuration it made, two entries on (into
    /// the joint configuration and out of it); 1, 2 and 3 on both, within 5 s of every clock; from
    /// the removal on, no record sent to the removed member and 4 on the voter alone; its own write
    /// 5 never applied there by itself, Timeout.
    #[test]
    fn a_learner_added_has_every_write_and_once_removed_receives_none() {
        let dir = tempfile::tempdir().unwrap();
        let mut driven = Driven::new();
        let clients = vec![
            driven.open(dir.path(), 1, vec![1], None),
            // A member to be added opens as the learner it is to be.
            driven.open_member(dir.path(), 2, vec![1], vec![2], None),
        ];
        let mut pump = Pump::new(&clients);
        let (voter, learner) = (&clients[0], &clients[1]);
        driven.elect(&mut pump, &clients[..1], 0, 1);
        let elected = terms(&clients[..1]);
        write(&mut driven, &mut pump, voter, 1);
        let before = voter.handle().status().unwrap();
        assert_eq!((before.voters.clone(), before.learners), (vec![1], vec![]));
        assert_eq!(held(&mut driven, 1), Vec::<u8>::new());

        let until = driven.until();
        let added = driven
            .answer(
                &mut pump,
                &change(voter, until, MemberChange::AddLearner(2)),
            )
            .unwrap();
        assert_eq!((added.voters, added.learners), (vec![1], vec![2]));
        assert_eq!(added.applied, before.applied + 2, "not through two phases");
        write(&mut driven, &mut pump, voter, 2);
        // The learner's own write reaches the leader.
        write(&mut driven, &mut pump, learner, 3);
        catch_up(&mut driven, &mut pump, &clients, 0, 1);
        assert_eq!(held(&mut driven, 0), vec![1, 2, 3]);
        assert_eq!(held(&mut driven, 1), vec![1, 2, 3]);
        // The writes reach the learner within 5 s: here, before any clock moves at all.
        assert!(
            driven
                .clocks
                .iter()
                .all(|clock| offset(clock) < Duration::from_secs(5))
        );

        pump.departed.push(2);
        let until = driven.until();
        let removed = driven
            .answer(&mut pump, &change(voter, until, MemberChange::Remove(2)))
            .unwrap();
        assert_eq!((removed.voters, removed.learners), (vec![1], vec![]));
        // What the leader sent before the removal is delivered; nothing after it is.
        quiet(&mut driven, &mut pump);
        let sent = pump.delivered.get(&2).copied().unwrap_or_default();
        assert!(sent > 0);
        write(&mut driven, &mut pump, voter, 4);
        quiet(&mut driven, &mut pump);
        assert_eq!(pump.delivered.get(&2).copied().unwrap_or_default(), sent);
        assert_eq!(held(&mut driven, 0), vec![1, 2, 3, 4]);
        assert_eq!(held(&mut driven, 1), vec![1, 2, 3]);

        // The removed member's own write: the leader refuses what it sends, and it never applies
        // the write by itself.
        let until = driven.until();
        let answer = submit(learner, until, |db| {
            db.exec("INSERT INTO member_items VALUES(5)")?;
            Ok(Vec::new())
        });
        quiet(&mut driven, &mut pump);
        assert!(matches!(answer.try_recv(), Err(mpsc::TryRecvError::Empty)));
        set(&driven.clock(1), driven.base, until);
        assert_eq!(driven.settle(&mut pump, &answer), Err(Refusal::Timeout));
        assert_eq!(held(&mut driven, 1), vec![1, 2, 3]);
        assert_eq!(held(&mut driven, 0), vec![1, 2, 3, 4]);
        assert_eq!(pump.delivered.get(&2).copied().unwrap_or_default(), sent);
        assert_eq!(terms(&clients[..1]), elected, "an election happened");
        driven.drain(&mut pump);
        pump.finish();
    }

    /// AC-T1.8.M1, T-T1.8.M3 (CP-T1.8) on driven time (replication.md, "Membership"): a learner is
    /// promoted to voter and demoted again, each through the joint configuration. Do: add member 2
    /// as a learner, promote it, write, demote it, write. Expect: each change answered with the
    /// configuration it made, two entries on; while 2 votes, the write commits through it; once
    /// demoted it still has every write, and no election happens.
    #[test]
    fn a_learner_is_promoted_and_demoted_through_the_joint_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let mut driven = Driven::new();
        let clients = vec![
            driven.open(dir.path(), 1, vec![1], None),
            driven.open_member(dir.path(), 2, vec![1], vec![2], None),
        ];
        let mut pump = Pump::new(&clients);
        let voter = &clients[0];
        driven.elect(&mut pump, &clients[..1], 0, 1);
        let elected = terms(&clients[..1]);
        write(&mut driven, &mut pump, voter, 1);
        let mut configurations = Vec::new();
        let mut applied = voter.handle().status().unwrap().applied;
        for (made, then) in [
            (MemberChange::AddLearner(2), 2),
            (MemberChange::Promote(2), 3),
            (MemberChange::Demote(2), 4),
        ] {
            let until = driven.until();
            let status = driven
                .answer(&mut pump, &change(voter, until, made))
                .unwrap();
            assert_eq!(
                status.applied,
                applied + 2,
                "{made:?} not through two phases"
            );
            configurations.push((status.voters, status.learners));
            write(&mut driven, &mut pump, voter, then);
            applied = voter.handle().status().unwrap().applied;
        }
        assert_eq!(
            configurations,
            vec![(vec![1], vec![2]), (vec![1, 2], vec![]), (vec![1], vec![2])]
        );
        catch_up(&mut driven, &mut pump, &clients, 0, 1);
        assert_eq!(held(&mut driven, 0), vec![1, 2, 3, 4]);
        assert_eq!(held(&mut driven, 1), vec![1, 2, 3, 4]);
        assert_eq!(terms(&clients), vec![elected[0]; 2], "an election happened");
        driven.drain(&mut pump);
        pump.finish();
    }

    /// AC-T1.8.M1, T-T1.8.M2 (CP-T1.8) on driven time (replication.md, "Membership"): only the
    /// leader changes members, one change at a time, and only as the configuration can take it. Do,
    /// in a group of three voters: ask a follower; ask the leader for two changes at once; promote
    /// a non-member, add a voter as a learner, demote and remove the leader; add a learner to a
    /// group of the most members. Expect: NotLeader naming the leader; the first of the two
    /// answered and the second Unavailable; Invalid for each the configuration can't take and
    /// TooLarge past the members, each proposing nothing; a change the configuration already shows
    /// answered with it.
    #[test]
    fn member_changes_are_refused_off_the_leader_at_once_or_when_impossible() {
        let dir = tempfile::tempdir().unwrap();
        let mut driven = Driven::new();
        let clients: Vec<Client> = (1..=3)
            .map(|n| driven.open(dir.path(), n, vec![1, 2, 3], None))
            .collect();
        let mut pump = Pump::new(&clients);
        let leader = driven.elect(&mut pump, &clients, 0, 1);
        let id = u64::try_from(leader + 1).unwrap();
        let led = &clients[leader];
        let before = led.handle().status().unwrap();
        let until = driven.until();
        let asked = change(
            &clients[(leader + 1) % 3],
            until,
            MemberChange::AddLearner(4),
        );
        assert_eq!(
            driven.answer(&mut pump, &asked),
            Err(Refusal::NotLeader { leader: Some(id) })
        );
        // A removal of no member: shown already, it proposes nothing.
        let first = change(led, until, MemberChange::Remove(9));
        let second = change(led, until, MemberChange::Remove(8));
        assert!(matches!(
            driven.answer(&mut pump, &second),
            Err(Refusal::Unavailable(why)) if why.contains("under way")
        ));
        let shown = driven.answer(&mut pump, &first).unwrap();
        assert_eq!((shown.voters, shown.learners), (vec![1, 2, 3], vec![]));
        for impossible in [
            MemberChange::Promote(9),
            MemberChange::AddLearner((id % 3) + 1),
            MemberChange::Demote(id),
            MemberChange::Remove(id),
        ] {
            assert!(
                matches!(
                    driven.answer(&mut pump, &change(led, until, impossible)),
                    Err(Refusal::Invalid { what, .. }) if what == "member change"
                ),
                "{impossible:?}"
            );
        }
        assert_eq!(led.handle().status().unwrap().applied, before.applied);
        driven.drain(&mut pump);
        pump.finish();

        // A group of the most members: one voter and 63 learners, none of them running, so no
        // relay; time stands still.
        let dir = tempfile::tempdir().unwrap();
        let mut driven = Driven::new();
        let full = driven.open_member(dir.path(), 1, vec![1], (2..=64).collect(), None);
        driven.owners[0].0.replica.campaign().unwrap();
        // Its lead and the change's barrier each wait on its disk: each step yields to it, and
        // the change is asked only once the voter leads.
        let led = (0..ROUNDS).any(|_| {
            driven.step();
            std::thread::park_timeout(POLL);
            full.handle().status().unwrap().leader == 1
        });
        assert!(led, "the voter did not lead in {ROUNDS} driven steps");
        let asked = change(&full, driven.until(), MemberChange::AddLearner(65));
        let refused = (0..ROUNDS)
            .find_map(|_| {
                driven.step();
                std::thread::park_timeout(POLL);
                asked.try_recv().ok()
            })
            .unwrap();
        assert_eq!(
            refused,
            Err(Refusal::TooLarge {
                what: "replica members".into(),
                size: 65,
                limit: MEMBERS,
            })
        );
        let status = full.handle().status().unwrap();
        assert_eq!((status.voters.len(), status.learners.len()), (1, 63));
    }
    /// AC-T1.8.M1, T-T1.8.M3 (CP-T1.8) on driven time (replication.md, "Membership"): the members a
    /// group changed to are what it reopens with, whatever its config first named. Do: add member 2
    /// as a learner, write, restart both from the configs they first opened with, write again.
    /// Expect: the configuration reopened names the learner, and it has both writes.
    #[test]
    fn a_changed_membership_holds_through_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mut driven = Driven::new();
        let clients = vec![
            driven.open(dir.path(), 1, vec![1], None),
            driven.open_member(dir.path(), 2, vec![1], vec![2], None),
        ];
        let mut pump = Pump::new(&clients);
        driven.elect(&mut pump, &clients[..1], 0, 1);
        let until = driven.until();
        driven
            .answer(
                &mut pump,
                &change(&clients[0], until, MemberChange::AddLearner(2)),
            )
            .unwrap();
        write(&mut driven, &mut pump, &clients[0], 1);
        catch_up(&mut driven, &mut pump, &clients, 0, 1);
        for owner in 0..2 {
            driven.advance(owner, Duration::from_millis(60));
        }
        for _ in 0..8 {
            driven.round(&mut pump);
        }
        driven.drain(&mut pump);
        pump.finish();
        drop(clients);
        drop(driven);

        let mut driven = Driven::new();
        let reopened = vec![
            driven.open(dir.path(), 1, vec![1], None),
            driven.open_member(dir.path(), 2, vec![1], vec![2], None),
        ];
        let mut pump = Pump::new(&reopened);
        driven.elect(&mut pump, &reopened[..1], 0, 1);
        let status = reopened[0].handle().status().unwrap();
        assert_eq!((status.voters, status.learners), (vec![1], vec![2]));
        write(&mut driven, &mut pump, &reopened[0], 2);
        catch_up(&mut driven, &mut pump, &reopened, 0, 1);
        assert_eq!(held(&mut driven, 1), vec![1, 2]);
        driven.drain(&mut pump);
        pump.finish();
    }

    /// Rounds, time standing still, until `reached` holds of the owners.
    fn reach(
        driven: &mut Driven,
        pump: &mut Pump<'_>,
        what: &str,
        reached: impl Fn(&Driven) -> bool,
    ) {
        for _ in 0..ROUNDS {
            if reached(driven) {
                return;
            }
            driven.round(pump);
        }
        panic!("{what} not reached in {ROUNDS} driven rounds");
    }
    /// The configuration owner `owner` has applied.
    fn applied_configuration(driven: &Driven, owner: usize) -> ConfState {
        driven.owners[owner].0.replica.configuration().clone()
    }
    /// The change the core of owner `owner` last took or wrote.
    fn pending_change(driven: &Driven, owner: usize) -> u64 {
        driven.owners[owner]
            .0
            .replica
            .core()
            .raft
            .pending_conf_index()
    }
    /// Rounds until every one of `voters` has applied the group out of any joint configuration,
    /// with `learners` its learners and nothing of a change still to apply.
    fn settled(driven: &mut Driven, pump: &mut Pump<'_>, voters: usize, learners: &[u64]) {
        reach(driven, pump, "the group settled", |driven| {
            (0..voters).all(|owner| {
                let applied = applied_configuration(driven, owner);
                applied.voters_outgoing.is_empty()
                    && applied.learners_next.is_empty()
                    && applied.learners == learners
                    && !driven.owners[owner]
                        .0
                        .replica
                        .core()
                        .raft
                        .has_pending_conf()
            })
        });
    }

    /// AC-T1.8.M1, T-T1.8.M3 (CP-T1.8) on driven time (replication.md, "Membership"): a change past
    /// its deadline answers what its stage knows. In a group of three voters, the relay held so
    /// that nothing moves on: Do: a change's deadline passes at its barrier; then, relayed to its
    /// proposal, another's; then, relayed until the joint configuration it made is applied, a
    /// third's. Expect: Timeout, with nothing proposed; UnknownOutcome, and with the relay back it
    /// applies all the same, as reading the configuration shows; while the joint configuration is
    /// applied, the status names its outgoing voters (`voters_outgoing`), and the deadline answers
    /// the status applied; with the relay back the group leaves it.
    #[test]
    fn a_change_past_its_deadline_answers_what_its_stage_knows() {
        let dir = tempfile::tempdir().unwrap();
        let mut driven = Driven::new();
        let mut clients: Vec<Client> = (1..=3)
            .map(|n| driven.open(dir.path(), n, vec![1, 2, 3], None))
            .collect();
        clients.push(driven.open_member(dir.path(), 4, vec![1, 2, 3], vec![4], None));
        let mut pump = Pump::new(&clients);
        let leader = driven.elect(&mut pump, &clients[..3], 0, 1);
        let led = &clients[leader];
        let clock = driven.clock(leader);
        // A deadline a second past the leader's clock, inside the relay's budget for the records it
        // carries, so the clock reaching it expires the change alone.
        let deadline =
            |clock: &AtomicU64, base: Instant| base + offset(clock) + Duration::from_secs(1);

        let proposed = pending_change(&driven, leader);
        let until = deadline(&clock, driven.base);
        let asked = change(led, until, MemberChange::AddLearner(4));
        driven.step();
        assert!(matches!(asked.try_recv(), Err(mpsc::TryRecvError::Empty)));
        set(&clock, driven.base, until);
        driven.step();
        assert_eq!(asked.try_recv().unwrap(), Err(Refusal::Timeout));
        for _ in 0..8 {
            driven.round(&mut pump);
        }
        assert_eq!(pending_change(&driven, leader), proposed);
        assert_eq!(
            applied_configuration(&driven, leader).learners,
            Vec::<u64>::new()
        );

        let until = deadline(&clock, driven.base);
        let asked = change(led, until, MemberChange::AddLearner(4));
        reach(&mut driven, &mut pump, "the proposal", |driven| {
            pending_change(driven, leader) > proposed
        });
        assert!(matches!(asked.try_recv(), Err(mpsc::TryRecvError::Empty)));
        set(&clock, driven.base, until);
        driven.step();
        assert_eq!(asked.try_recv().unwrap(), Err(Refusal::UnknownOutcome));
        settled(&mut driven, &mut pump, 3, &[4]);
        assert_eq!(led.handle().status().unwrap().learners, vec![4]);

        pump.departed.push(4);
        let until = deadline(&clock, driven.base);
        let asked = change(led, until, MemberChange::Remove(4));
        // Each round ends with what the leader has applied published, the joint configuration
        // too: its way out comes only after the joint one is committed.
        reach(
            &mut driven,
            &mut pump,
            "the joint configuration",
            |driven| {
                !applied_configuration(driven, leader)
                    .voters_outgoing
                    .is_empty()
            },
        );
        let joint = led.handle().status().unwrap();
        assert_eq!(
            (joint.voters_outgoing, joint.learners),
            (vec![1, 2, 3], vec![])
        );
        assert!(matches!(asked.try_recv(), Err(mpsc::TryRecvError::Empty)));
        set(&clock, driven.base, until);
        driven.step();
        // The status applied: still joint, or out of it if the way out had committed while the
        // leader's disk held back the joint configuration's apply.
        let answered = asked.try_recv().unwrap().unwrap();
        assert_eq!(
            (answered.voters_outgoing, answered.learners),
            (
                applied_configuration(&driven, leader).voters_outgoing,
                vec![]
            )
        );
        settled(&mut driven, &mut pump, 3, &[]);
        write(&mut driven, &mut pump, led, 1);
        driven.drain(&mut pump);
        pump.finish();
    }

    /// AC-T1.8.M1, T-T1.8.M3 (CP-T1.8) on driven time (replication.md, "Membership"): a change at
    /// its barrier when its lead is lost is refused, with nothing proposed. Do: in a group of three
    /// voters, the relay held, ask the leader for a change, then move its clock until its quorum
    /// check finds no quorum. Expect: NotLeader, naming no leader, and no member changed.
    #[test]
    fn a_change_at_its_barrier_is_refused_once_the_lead_is_lost() {
        let dir = tempfile::tempdir().unwrap();
        let mut driven = Driven::new();
        let clients: Vec<Client> = (1..=3)
            .map(|n| driven.open(dir.path(), n, vec![1, 2, 3], None))
            .collect();
        let mut pump = Pump::new(&clients);
        let leader = driven.elect(&mut pump, &clients, 0, 1);
        driven.drain(&mut pump);
        let proposed = pending_change(&driven, leader);
        let asked = change(
            &clients[leader],
            driven.until(),
            MemberChange::AddLearner(4),
        );
        driven.step();
        assert!(matches!(asked.try_recv(), Err(mpsc::TryRecvError::Empty)));
        let refused = (0..ROUNDS)
            .find_map(|_| {
                driven.advance(leader, TICK);
                driven.step();
                asked.try_recv().ok()
            })
            .unwrap();
        assert_eq!(refused, Err(Refusal::NotLeader { leader: None }));
        assert_eq!(pending_change(&driven, leader), proposed);
        assert_eq!(
            applied_configuration(&driven, leader).learners,
            Vec::<u64>::new()
        );
        pump.finish();
    }

    /// AC-T1.8.M1, T-T1.8.M3 (CP-T1.8) on driven time (replication.md, "Membership"): a change
    /// proposed and then lost is never answered as done, though the index it held is applied. Do:
    /// in a group of three voters, the leader proposes a change whose records are lost; the other
    /// two, their clocks past an election timeout, elect one of them, whose entry takes that index.
    /// Expect: UnknownOutcome, no member changed anywhere, and the new leader takes the change when
    /// asked again.
    #[test]
    fn a_change_another_lead_wrote_over_is_never_answered_done() {
        let dir = tempfile::tempdir().unwrap();
        let mut driven = Driven::new();
        let mut clients: Vec<Client> = (1..=3)
            .map(|n| driven.open(dir.path(), n, vec![1, 2, 3], None))
            .collect();
        clients.push(driven.open_member(dir.path(), 4, vec![1, 2, 3], vec![4], None));
        let mut pump = Pump::new(&clients);
        let leader = driven.elect(&mut pump, &clients[..3], 0, 1);
        driven.drain(&mut pump);
        let term = clients[leader].handle().status().unwrap().term;
        let proposed = pending_change(&driven, leader);
        let until = driven.until();
        let asked = change(&clients[leader], until, MemberChange::AddLearner(4));
        reach(&mut driven, &mut pump, "the proposal", |driven| {
            pending_change(driven, leader) > proposed
        });
        // What goes on is what the proposal's round hadn't sent: its entry is lost with this.
        clients[leader].handle().drain(OUTGOING_MESSAGES).unwrap();
        let others: Vec<usize> = (0..3).filter(|owner| *owner != leader).collect();
        let answered = (0..ROUNDS)
            .find_map(|_| {
                for owner in &others {
                    driven.advance(*owner, TICK);
                }
                driven.round(&mut pump);
                asked.try_recv().ok()
            })
            .unwrap();
        assert_eq!(answered, Err(Refusal::UnknownOutcome));
        settled(&mut driven, &mut pump, 3, &[]);
        let status = clients[others[0]].handle().status().unwrap();
        assert!(status.term > term && status.leader != 0, "{status:?}");
        let led = usize::try_from(status.leader - 1).unwrap();
        // The index the change held is applied, as another entry.
        reach(&mut driven, &mut pump, "the index applied", |driven| {
            (0..3).all(|owner| driven.owners[owner].0.replica.applied().index > proposed)
        });
        settled(&mut driven, &mut pump, 3, &[]);
        let until = driven.until();
        let asked = change(&clients[led], until, MemberChange::AddLearner(4));
        assert_eq!(driven.answer(&mut pump, &asked).unwrap().learners, vec![4]);
        driven.drain(&mut pump);
        pump.finish();
    }

    /// AC-T1.8.M1, T-T1.8.M3 (CP-T1.8) under a nemesis (replication.md, "Membership"): a change
    /// holds through an election at each of its stages. Do, in a group of three voters: the lead
    /// moves as the change passes its barrier, once it is proposed, and once its joint
    /// configuration is applied. Expect: at the barrier, a refusal saying to ask again, nothing
    /// changed; once proposed, Ok or UnknownOutcome, and the new leader carries it through; once
    /// applied, Ok. Each time the group leaves the joint configuration by itself, the new leader
    /// takes the change (asked again, shown already), and a write reaches the learner.
    #[test]
    fn a_change_holds_through_an_election_at_each_stage() {
        for stage in 0..3 {
            let dir = tempfile::tempdir().unwrap();
            let mut driven = Driven::new();
            let mut clients: Vec<Client> = (1..=3)
                .map(|n| driven.open(dir.path(), n, vec![1, 2, 3], None))
                .collect();
            clients.push(driven.open_member(dir.path(), 4, vec![1, 2, 3], vec![4], None));
            let mut pump = Pump::new(&clients);
            pump.refusals = Some(0);
            let leader = driven.elect(&mut pump, &clients[..3], 0, 1);
            let proposed = pending_change(&driven, leader);
            let until = driven.until();
            let asked = change(&clients[leader], until, MemberChange::AddLearner(4));
            match stage {
                0 => driven.step(),
                1 => reach(&mut driven, &mut pump, "the proposal", |driven| {
                    pending_change(driven, leader) > proposed
                }),
                _ => reach(&mut driven, &mut pump, "the change applied", |driven| {
                    applied_configuration(driven, leader).learners == [4]
                }),
            }
            let led = driven.hand_off(&mut pump, &clients[..3], leader, (leader + 1) % 3);
            let outcome = driven.answer(&mut pump, &asked);
            match stage {
                0 => {
                    assert!(
                        matches!(
                            &outcome,
                            Err(Refusal::NotLeader { .. } | Refusal::Unavailable(_))
                        ),
                        "stage {stage}: {outcome:?}"
                    );
                    settled(&mut driven, &mut pump, 3, &[]);
                }
                1 => {
                    assert!(
                        matches!(&outcome, Ok(_) | Err(Refusal::UnknownOutcome)),
                        "stage {stage}: {outcome:?}"
                    );
                    settled(&mut driven, &mut pump, 3, &[4]);
                }
                _ => {
                    assert_eq!(outcome.unwrap().learners, vec![4], "stage {stage}");
                    settled(&mut driven, &mut pump, 3, &[4]);
                }
            }
            let until = driven.until();
            let status = driven
                .answer(
                    &mut pump,
                    &change(&clients[led], until, MemberChange::AddLearner(4)),
                )
                .unwrap();
            assert_eq!(
                (status.leader, status.voters, status.learners),
                (u64::try_from(led + 1).unwrap(), vec![1, 2, 3], vec![4]),
                "stage {stage}"
            );
            write(&mut driven, &mut pump, &clients[led], 1);
            catch_up(&mut driven, &mut pump, &clients, led, 3);
            assert_eq!(held(&mut driven, 3), vec![1], "stage {stage}");
            driven.drain(&mut pump);
            pump.finish();
        }
    }

    /// AC-T1.8.M2, T-T1.8.M4 (CP-T1.8) on driven time (replication.md, "Membership"): the leader is
    /// removed once it has handed on its lead. Do, in a group of three voters: ask a follower to
    /// hand on the lead; ask the leader to hand it to a non-member and to itself, and to remove
    /// itself; then the leader hands it to a follower, the new leader removes the old one, and
    /// writes. Expect: NotLeader naming the leader; Invalid; the status at once, in the same term;
    /// Invalid, saying to hand the lead on first; then the follower leading, in a later term; the
    /// removal answered with the other two voters; the write on both of them, none on the removed
    /// member, and no record delivered to it from the removal's answer on.
    #[test]
    fn the_leader_is_removed_once_it_has_handed_on_its_lead() {
        let dir = tempfile::tempdir().unwrap();
        let mut driven = Driven::new();
        let clients: Vec<Client> = (1..=3)
            .map(|n| driven.open(dir.path(), n, vec![1, 2, 3], None))
            .collect();
        let mut pump = Pump::new(&clients);
        let leader = driven.elect(&mut pump, &clients, 0, 1);
        let (heir, other) = ((leader + 1) % 3, (leader + 2) % 3);
        let id = |owner: usize| u64::try_from(owner + 1).unwrap();
        let before = clients[leader].handle().status().unwrap();
        let until = driven.until();
        assert_eq!(
            driven.answer(&mut pump, &transfer(&clients[heir], until, id(heir))),
            Err(Refusal::NotLeader {
                leader: Some(id(leader))
            })
        );
        assert!(matches!(
            driven.answer(&mut pump, &transfer(&clients[leader], until, 9)),
            Err(Refusal::Invalid { what, .. }) if what == "lead transfer"
        ));
        let stayed = driven
            .answer(&mut pump, &transfer(&clients[leader], until, id(leader)))
            .unwrap();
        assert_eq!((stayed.leader, stayed.term), (id(leader), before.term));
        assert!(matches!(
            driven.answer(
                &mut pump,
                &change(&clients[leader], until, MemberChange::Remove(id(leader)))
            ),
            Err(Refusal::Invalid { why, .. }) if why.contains("transfer")
        ));

        let handed = driven
            .answer(&mut pump, &transfer(&clients[leader], until, id(heir)))
            .unwrap();
        assert_eq!(handed.leader, id(heir));
        assert!(handed.term > before.term, "{handed:?}");
        assert_eq!(driven.follow(&mut pump, &clients, 1), heir);

        pump.departed.push(id(leader));
        let until = driven.until();
        let removed = driven
            .answer(
                &mut pump,
                &change(&clients[heir], until, MemberChange::Remove(id(leader))),
            )
            .unwrap();
        let mut rest = vec![id(heir), id(other)];
        rest.sort_unstable();
        assert_eq!((removed.voters, removed.learners), (rest, vec![]));
        quiet(&mut driven, &mut pump);
        let sent = pump.delivered.get(&id(leader)).copied().unwrap_or_default();
        write(&mut driven, &mut pump, &clients[heir], 1);
        catch_up(&mut driven, &mut pump, &clients, heir, other);
        quiet(&mut driven, &mut pump);
        assert_eq!(
            pump.delivered.get(&id(leader)).copied().unwrap_or_default(),
            sent
        );
        assert_eq!(held(&mut driven, heir), vec![1]);
        assert_eq!(held(&mut driven, other), vec![1]);
        assert_eq!(held(&mut driven, leader), Vec::<u8>::new());
        driven.drain(&mut pump);
        pump.finish();
    }

    /// AC-T1.8.M3, T-T1.8.M5 (CP-T1.8) on driven time (replication.md, "Membership"): a member that
    /// joins catches up by the leader's image when its log no longer reaches back, and that image
    /// is bounded, so a database past the snapshot ceiling refuses a joiner before the change is
    /// proposed. Do: in a group of one voter, its ceiling lowered to 64 KiB, write a 128 KiB row;
    /// add member 2 as a learner; raise the ceiling back; add it again. Expect: TooLarge naming the
    /// database's bytes and the ceiling, nothing proposed and no learner; then the learner added,
    /// with the row.
    #[test]
    fn a_joiner_to_a_database_past_the_snapshot_ceiling_is_refused_before_it_is_proposed() {
        let dir = tempfile::tempdir().unwrap();
        let mut driven = Driven::new();
        let clients = vec![
            driven.open(dir.path(), 1, vec![1], None),
            driven.open_member(dir.path(), 2, vec![1], vec![2], None),
        ];
        let mut pump = Pump::new(&clients);
        driven.elect(&mut pump, &clients[..1], 0, 1);
        driven.owners[0]
            .0
            .replica
            .machine_mut()
            .set_snapshot_bytes(64 << 10);
        driven
            .call(&mut pump, &clients[0], |db| {
                db.exec("CREATE TABLE joined(b)")?;
                db.exec("INSERT INTO joined VALUES(randomblob(131072))")?;
                Ok(Vec::new())
            })
            .unwrap();
        let proposed = pending_change(&driven, 0);
        let until = driven.until();
        let refused = driven.answer(
            &mut pump,
            &change(&clients[0], until, MemberChange::AddLearner(2)),
        );
        assert!(
            matches!(
                &refused,
                Err(Refusal::TooLarge { what, size, limit })
                    if what.contains("joiner") && *size > 128 << 10 && *limit == 64 << 10
            ),
            "{refused:?}"
        );
        assert_eq!(pending_change(&driven, 0), proposed);
        assert_eq!(
            clients[0].handle().status().unwrap().learners,
            Vec::<u64>::new()
        );

        driven.owners[0]
            .0
            .replica
            .machine_mut()
            .set_snapshot_bytes(crate::replication::SNAPSHOT_BYTES);
        let added = driven
            .answer(
                &mut pump,
                &change(&clients[0], until, MemberChange::AddLearner(2)),
            )
            .unwrap();
        assert_eq!(added.learners, vec![2]);
        catch_up(&mut driven, &mut pump, &clients, 0, 1);
        let rows = driven.owners[1]
            .0
            .replica
            .machine_mut()
            .read_db(|db| {
                let mut rows = 0;
                db.query("SELECT length(b) FROM joined", [], |row| {
                    rows += row.get::<i64>(0).map_err(unavailable)?;
                    Ok(true)
                })?;
                Ok(rows)
            })
            .unwrap();
        assert_eq!(rows, 128 << 10);
        driven.drain(&mut pump);
        pump.finish();
    }

    /// AC-T1.8.M1, T-T1.8.M3 (CP-T1.8) on driven time (replication.md, "Membership"; Ongaro's
    /// dissertation, section 4.1): a member's records count by the newest configuration its log
    /// states, applied or not, so a removal another lead writes over gives the member back. Do: in
    /// a group of three voters and learner 4, the leader appends 4's removal, whose records are
    /// lost; the other two, their clocks past an election timeout, elect one of them, whose entry
    /// takes that index; a write. Expect: the old leader refuses a record from 4 while its log
    /// holds the removal; once the new leader's entry replaces it, its configuration names 4 again
    /// and it takes 4's record; the change answered UnknownOutcome, and the write reaches 4.
    #[test]
    fn a_removal_another_lead_wrote_over_gives_the_member_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut driven = Driven::new();
        let mut clients: Vec<Client> = (1..=3)
            .map(|n| driven.open(dir.path(), n, vec![1, 2, 3], None))
            .collect();
        clients.push(driven.open_member(dir.path(), 4, vec![1, 2, 3], vec![4], None));
        let mut pump = Pump::new(&clients);
        let leader = driven.elect(&mut pump, &clients[..3], 0, 1);
        let until = driven.until();
        driven
            .answer(
                &mut pump,
                &change(&clients[leader], until, MemberChange::AddLearner(4)),
            )
            .unwrap();
        settled(&mut driven, &mut pump, 3, &[4]);
        driven.drain(&mut pump);
        // From the removal on, what 4 sent before it reaches a leader that no longer counts it.
        pump.departed.push(4);
        let proposed = pending_change(&driven, leader);
        let asked = change(&clients[leader], until, MemberChange::Remove(4));
        reach(&mut driven, &mut pump, "the removal appended", |driven| {
            pending_change(driven, leader) > proposed
        });
        clients[leader].handle().drain(OUTGOING_MESSAGES).unwrap();
        let counts = |driven: &Driven| {
            driven.owners[leader]
                .0
                .replica
                .core()
                .raft
                .configuration()
                .contains(4)
        };
        assert!(!counts(&driven));
        let from_learner = |driven: &mut Driven| {
            let mut record = hyper_raft::proto::message(
                u64::try_from(leader + 1).unwrap(),
                hyper_raft::proto::MessageType::MsgHeartbeatResponse,
            );
            record.from = 4;
            record.term = driven.owners[leader].0.replica.term();
            let mut bytes = Vec::new();
            record.encode(&mut bytes);
            let mut receipt = clients[leader]
                .handle()
                .admit_receive(4, bytes, driven.until())
                .unwrap();
            driven.step();
            receipt.try_result()
        };
        assert!(matches!(
            from_learner(&mut driven),
            Err(Refusal::Invalid { why, .. }) if why == "sender is not a current member"
        ));
        // Lost too: whatever that step had the old leader send.
        clients[leader].handle().drain(OUTGOING_MESSAGES).unwrap();

        let others: Vec<usize> = (0..3).filter(|owner| *owner != leader).collect();
        let answered = (0..ROUNDS)
            .find_map(|_| {
                for owner in &others {
                    driven.advance(*owner, TICK);
                }
                driven.round(&mut pump);
                asked.try_recv().ok()
            })
            .unwrap();
        assert_eq!(answered, Err(Refusal::UnknownOutcome));
        reach(
            &mut driven,
            &mut pump,
            "the removal written over",
            |driven| driven.owners[leader].0.replica.applied().index > proposed && counts(driven),
        );
        settled(&mut driven, &mut pump, 3, &[4]);
        assert_eq!(from_learner(&mut driven), Ok(Some(())));
        let led =
            usize::try_from(clients[others[0]].handle().status().unwrap().leader - 1).unwrap();
        write(&mut driven, &mut pump, &clients[led], 1);
        catch_up(&mut driven, &mut pump, &clients, led, 3);
        assert_eq!(held(&mut driven, 3), vec![1]);
        driven.drain(&mut pump);
        pump.finish();
    }
    /// A group of three voters and member 4, opened as the learner it is to be.
    fn three_and_a_learner(driven: &mut Driven, dir: &Path) -> Vec<Client> {
        let mut clients: Vec<Client> = (1..=3)
            .map(|n| driven.open(dir, n, vec![1, 2, 3], None))
            .collect();
        clients.push(driven.open_member(dir, 4, vec![1, 2, 3], vec![4], None));
        clients
    }

    /// AC-T1.8.M1, T-T1.8.M2 (CP-T1.8) on driven time (replication.md, "Membership"): a change past
    /// its deadline once its joint configuration is applied answers its status, joint, and neither
    /// a change nor a hand-off is taken until the group has left it. Do, in a group of three
    /// voters: add learner 4, the relay held from the moment the leader appends its way out, so the
    /// group stays joint; its deadline passes; then ask the leader for another change and for a
    /// hand-off. Expect: the status, with voters_outgoing [1, 2, 3] and learner 4; both refused
    /// Unavailable, saying the group is leaving a joint configuration or has a change to apply;
    /// with the relay back, the group leaves it and takes the next change.
    #[test]
    fn a_change_or_hand_off_is_refused_while_the_group_leaves_a_joint_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let mut driven = Driven::new();
        let clients = three_and_a_learner(&mut driven, dir.path());
        let mut pump = Pump::new(&clients);
        let leader = driven.elect(&mut pump, &clients[..3], 0, 1);
        let led = &clients[leader];
        let clock = driven.clock(leader);
        let until = driven.base + offset(&clock) + Duration::from_secs(1);
        let proposed = pending_change(&driven, leader);
        let asked = change(led, until, MemberChange::AddLearner(4));
        reach(&mut driven, &mut pump, "the joint entry", |driven| {
            pending_change(driven, leader) > proposed
        });
        let joint = pending_change(&driven, leader);
        // The way out is appended once the joint entry commits. From then on the relay is held:
        // what the leader sends of it waits in its queue, so it can't commit.
        reach(&mut driven, &mut pump, "the way out appended", |driven| {
            pending_change(driven, leader) > joint
        });
        for _ in 0..ROUNDS {
            if !applied_configuration(&driven, leader)
                .voters_outgoing
                .is_empty()
            {
                break;
            }
            driven.step();
            std::thread::park_timeout(POLL);
        }
        assert_eq!(
            applied_configuration(&driven, leader).voters_outgoing,
            vec![1, 2, 3]
        );
        set(&clock, driven.base, until);
        driven.step();
        let answered = asked.try_recv().unwrap().unwrap();
        assert_eq!(
            (answered.voters_outgoing, answered.learners),
            (vec![1, 2, 3], vec![4])
        );

        let later = driven.base + offset(&clock) + Duration::from_secs(5);
        let other = u64::try_from((leader + 1) % 3 + 1).unwrap();
        for refused in [
            change(led, later, MemberChange::Promote(4)),
            transfer(led, later, other),
        ] {
            driven.step();
            assert!(matches!(
                refused.try_recv().unwrap(),
                Err(Refusal::Unavailable(why))
                    if why.contains("leaving a joint configuration")
                        || why.contains("still to apply")
            ),);
        }
        // The relay back: the way out reaches the followers and commits.
        settled(&mut driven, &mut pump, 3, &[4]);
        let until = driven.until();
        let promoted = driven
            .answer(&mut pump, &change(led, until, MemberChange::Promote(4)))
            .unwrap();
        assert_eq!(promoted.voters, vec![1, 2, 3, 4]);
        driven.drain(&mut pump);
        pump.finish();
    }

    /// AC-T1.8.M2, T-T1.8.M4 (CP-T1.8) on driven time (replication.md, "Membership"): a hand-off
    /// the core gives up is refused, and the lead stays. Do, in a group of three voters, every
    /// record to one follower lost: the leader hands its lead to that follower; the leader's and
    /// the other follower's clocks run on. Expect: Unavailable, saying the lead was not taken, and
    /// the leader still leading, in its term.
    #[test]
    fn a_hand_off_the_core_gives_up_is_refused_and_the_lead_stays() {
        let dir = tempfile::tempdir().unwrap();
        let mut driven = Driven::new();
        let clients: Vec<Client> = (1..=3)
            .map(|n| driven.open(dir.path(), n, vec![1, 2, 3], None))
            .collect();
        let mut pump = Pump::new(&clients);
        let leader = driven.elect(&mut pump, &clients, 0, 1);
        let (heir, other) = ((leader + 1) % 3, (leader + 2) % 3);
        let term = clients[leader].handle().status().unwrap().term;
        pump.lost.push(u64::try_from(heir + 1).unwrap());
        let asked = transfer(
            &clients[leader],
            driven.until(),
            u64::try_from(heir + 1).unwrap(),
        );
        let answered = (0..ROUNDS)
            .find_map(|_| {
                driven.advance(leader, TICK);
                driven.advance(other, TICK);
                driven.round(&mut pump);
                asked.try_recv().ok()
            })
            .unwrap();
        assert!(
            matches!(&answered, Err(Refusal::Unavailable(why)) if why.contains("not taken")),
            "{answered:?}"
        );
        let status = clients[leader].handle().status().unwrap();
        assert_eq!(
            (status.leader, status.term),
            (u64::try_from(leader + 1).unwrap(), term)
        );
        pump.lost.clear();
        driven.drain(&mut pump);
        pump.finish();
    }

    /// AC-T1.8.M2, T-T1.8.M4 (CP-T1.8) on driven time (replication.md, "Membership"): a hand-off
    /// whose lead goes to a third member is refused naming it. Do, in a group of three voters,
    /// every record to one follower lost: the leader is asked to hand its lead to that follower,
    /// and its core then hands it to the other (as a disk-stall hand-off does). Expect: NotLeader
    /// naming the other follower.
    #[test]
    fn a_hand_off_whose_lead_goes_to_a_third_member_is_refused_naming_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut driven = Driven::new();
        let clients: Vec<Client> = (1..=3)
            .map(|n| driven.open(dir.path(), n, vec![1, 2, 3], None))
            .collect();
        let mut pump = Pump::new(&clients);
        let leader = driven.elect(&mut pump, &clients, 0, 1);
        let (heir, other) = ((leader + 1) % 3, (leader + 2) % 3);
        let other_id = u64::try_from(other + 1).unwrap();
        pump.lost.push(u64::try_from(heir + 1).unwrap());
        let asked = transfer(
            &clients[leader],
            driven.until(),
            u64::try_from(heir + 1).unwrap(),
        );
        driven.step();
        assert!(matches!(asked.try_recv(), Err(mpsc::TryRecvError::Empty)));
        driven.owners[leader].0.replica.transfer(other_id).unwrap();
        assert_eq!(
            driven.answer(&mut pump, &asked),
            Err(Refusal::NotLeader {
                leader: Some(other_id)
            })
        );
        pump.lost.clear();
        driven.drain(&mut pump);
        pump.finish();
    }

    /// AC-T1.8.M2, T-T1.8.M4 (CP-T1.8) on driven time (replication.md, "Membership"): a hand-off
    /// past its deadline before the lead moves answers UnknownOutcome. Do, in a group of three
    /// voters, every record to one follower lost: the leader hands its lead to that follower, and
    /// its clock reaches the hand-off's deadline. Expect: UnknownOutcome, and the status read shows
    /// who leads.
    #[test]
    fn a_hand_off_past_its_deadline_answers_unknown_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let mut driven = Driven::new();
        let clients: Vec<Client> = (1..=3)
            .map(|n| driven.open(dir.path(), n, vec![1, 2, 3], None))
            .collect();
        let mut pump = Pump::new(&clients);
        let leader = driven.elect(&mut pump, &clients, 0, 1);
        let heir = (leader + 1) % 3;
        pump.lost.push(u64::try_from(heir + 1).unwrap());
        let clock = driven.clock(leader);
        let until = driven.base + offset(&clock) + Duration::from_secs(1);
        let asked = transfer(&clients[leader], until, u64::try_from(heir + 1).unwrap());
        driven.step();
        assert!(matches!(asked.try_recv(), Err(mpsc::TryRecvError::Empty)));
        set(&clock, driven.base, until);
        driven.step();
        assert_eq!(asked.try_recv().unwrap(), Err(Refusal::UnknownOutcome));
        assert_eq!(
            clients[leader].handle().status().unwrap().leader,
            u64::try_from(leader + 1).unwrap()
        );
        pump.lost.clear();
        driven.drain(&mut pump);
        pump.finish();
    }

    /// AC-T1.8.M2, T-T1.8.M4 (CP-T1.8) on driven time (replication.md, "Membership"): a hand-off is
    /// refused beside a change, a change beside a hand-off, and a hand-off to a learner. Do, in a
    /// group of three voters and learner 4: ask a change and then a hand-off; then, every record to
    /// a follower lost, a hand-off to it and then a change; then a hand-off to learner 4. Expect:
    /// the second of each pair Unavailable, saying a change is under way; Invalid for the learner.
    #[test]
    fn a_hand_off_is_refused_beside_a_change_and_to_a_learner() {
        let dir = tempfile::tempdir().unwrap();
        let mut driven = Driven::new();
        let clients = three_and_a_learner(&mut driven, dir.path());
        let mut pump = Pump::new(&clients);
        let leader = driven.elect(&mut pump, &clients[..3], 0, 1);
        let led = &clients[leader];
        let heir = u64::try_from((leader + 1) % 3 + 1).unwrap();
        let until = driven.until();
        let first = change(led, until, MemberChange::AddLearner(4));
        let beside = transfer(led, until, heir);
        assert!(matches!(
            driven.answer(&mut pump, &beside),
            Err(Refusal::Unavailable(why)) if why.contains("under way")
        ));
        assert_eq!(driven.answer(&mut pump, &first).unwrap().learners, vec![4]);
        settled(&mut driven, &mut pump, 3, &[4]);

        pump.lost.push(heir);
        let handing = transfer(led, until, heir);
        driven.step();
        let beside = change(led, until, MemberChange::Remove(4));
        driven.step();
        assert!(matches!(
            beside.try_recv().unwrap(),
            Err(Refusal::Unavailable(why)) if why.contains("under way")
        ));
        let clock = driven.clock(leader);
        set(&clock, driven.base, until);
        driven.step();
        assert_eq!(handing.try_recv().unwrap(), Err(Refusal::UnknownOutcome));
        pump.lost.clear();

        let until = driven.until();
        assert!(matches!(
            driven.answer(&mut pump, &transfer(led, until, 4)),
            Err(Refusal::Invalid { what, .. }) if what == "lead transfer"
        ));
        driven.drain(&mut pump);
        pump.finish();
    }

    fn unchanged(db: &mut Db) -> anyhow::Result<Vec<u8>> {
        let mut value = Vec::new();
        db.query("SELECT value FROM baseline_items WHERE id = 1", [], |row| {
            value = row
                .get_value(0)
                .to_blob()
                .ok_or_else(|| anyhow::anyhow!("expected blob"))?
                .to_vec();
            Ok(false)
        })?;
        Ok(value)
    }

    /// U15/CP-T1.8 on driven time (replication.md, "The owner's time"): a complete original image
    /// precedes every delta, including untouched pages, and the seed's transfer to its source is
    /// the scenario's only election.
    #[test]
    fn autonomous_quorum_seed_preserves_unchanged_pages_then_delta_and_restart() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("state-3.db");
        let original = {
            let mut db = Db::open(&source).unwrap();
            db.exec("CREATE TABLE baseline_items(id INTEGER PRIMARY KEY,value BLOB); INSERT INTO baseline_items VALUES(1,randomblob(9000)); INSERT INTO baseline_items VALUES(2,x'02')").unwrap();
            db.flusher()().unwrap();
            unchanged(&mut db).unwrap()
        };
        let bootstrap = crate::replication::bootstrap::inspect_source(&source, 3).unwrap();
        let mut driven = Driven::new();
        let clients: Vec<Client> = (1..=3)
            .map(|n| driven.open(dir.path(), n, vec![1, 2, 3], Some(bootstrap.clone())))
            .collect();
        let mut pump = Pump::new(&clients);
        driven.elect(&mut pump, &clients, 0, 0);
        // Admitted before the seed lands, the call waits in the product queue while the transfer
        // to the source and the seed's application go on.
        assert_eq!(
            driven
                .call(&mut pump, clients.first().unwrap(), unchanged)
                .unwrap(),
            original
        );
        let seeded = terms(&clients);
        assert!(
            clients
                .iter()
                .all(|c| c.handle().status().unwrap().leader == 3)
        );
        driven
            .call(&mut pump, clients.get(1).unwrap(), |db| {
                db.exec("UPDATE baseline_items SET value=x'03' WHERE id=2")?;
                Ok(Vec::new())
            })
            .unwrap();
        for client in &clients {
            assert_eq!(driven.call(&mut pump, client, unchanged).unwrap(), original);
        }
        assert_eq!(
            terms(&clients),
            seeded,
            "an election happened after the seed's transfer"
        );
        // Past hyper-durable's quiet period, so each owner states its durable commit before it stops.
        for owner in 0..3 {
            driven.advance(owner, Duration::from_millis(60));
        }
        for _ in 0..8 {
            driven.round(&mut pump);
        }
        driven.drain(&mut pump);
        pump.finish();
        drop(clients);
        drop(driven);

        let mut driven = Driven::new();
        let reopened: Vec<Client> = (1..=3)
            .map(|n| driven.open(dir.path(), n, vec![1, 2, 3], Some(bootstrap.clone())))
            .collect();
        let mut pump = Pump::new(&reopened);
        driven.elect(&mut pump, &reopened, 0, 0);
        for client in &reopened {
            let status = client.handle().status().unwrap();
            assert_eq!(status.voters, vec![1, 2, 3]);
            assert!(status.learners.is_empty());
            assert!(status.fenced.is_none());
            assert!(!status.bootstrapping);
        }
        let elected = terms(&reopened);
        for client in &reopened {
            assert_eq!(
                driven
                    .call(&mut pump, client, |db| {
                        let mut value = Vec::new();
                        db.query("SELECT value FROM baseline_items WHERE id=2", [], |row| {
                            value = row.get_value(0).to_blob().unwrap().to_vec();
                            Ok(false)
                        })?;
                        Ok(value)
                    })
                    .unwrap(),
                vec![3]
            );
            assert_eq!(driven.call(&mut pump, client, unchanged).unwrap(), original);
        }
        assert_eq!(
            terms(&reopened),
            elected,
            "an election happened during the reopened scenario"
        );
        driven.drain(&mut pump);
        pump.finish();
    }

    /// U15/CP-T1.8 under a nemesis (replication.md, "The owner's time"): the seeded group with
    /// the lead moved while the delta's call is in flight, while a replica reads the seed's image,
    /// and while the reopened group's reads are. The seed's own transfer to its source is its
    /// first election. Whatever the election: the delta's answer is Ok or UnknownOutcome, every
    /// replica reads one value for it (the new one if it answered Ok), and every replica keeps
    /// the seed's original image.
    #[test]
    fn seed_and_delta_hold_through_an_election_at_each_step() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("state-3.db");
        let original = {
            let mut db = Db::open(&source).unwrap();
            db.exec("CREATE TABLE baseline_items(id INTEGER PRIMARY KEY,value BLOB); INSERT INTO baseline_items VALUES(1,randomblob(9000)); INSERT INTO baseline_items VALUES(2,x'02')").unwrap();
            db.flusher()().unwrap();
            unchanged(&mut db).unwrap()
        };
        let bootstrap = crate::replication::bootstrap::inspect_source(&source, 3).unwrap();
        let second = |db: &mut Db| -> anyhow::Result<Vec<u8>> {
            let mut value = Vec::new();
            db.query("SELECT value FROM baseline_items WHERE id=2", [], |row| {
                value = row.get_value(0).to_blob().unwrap().to_vec();
                Ok(false)
            })?;
            Ok(value)
        };
        let mut driven = Driven::new();
        let clients: Vec<Client> = (1..=3)
            .map(|n| driven.open(dir.path(), n, vec![1, 2, 3], Some(bootstrap.clone())))
            .collect();
        let mut pump = Pump::new(&clients);
        pump.refusals = Some(0);
        driven.elect(&mut pump, &clients, 0, 0);
        assert_eq!(
            driven
                .call(&mut pump, clients.first().unwrap(), unchanged)
                .unwrap(),
            original
        );
        // The delta goes in from n2 while the lead moves from the seed's source to n1.
        let delta = submit(clients.get(1).unwrap(), driven.until(), |db| {
            db.exec("UPDATE baseline_items SET value=x'03' WHERE id=2")?;
            Ok(Vec::new())
        });
        driven.transfer(&mut pump, &clients, 2, 0);
        let answered = driven.settle(&mut pump, &delta);
        assert!(
            matches!(answered, Ok(ref v) if v.is_empty())
                || matches!(answered, Err(Refusal::UnknownOutcome)),
            "{answered:?}"
        );
        let values: Vec<_> = clients
            .iter()
            .map(|client| driven.call(&mut pump, client, second).unwrap())
            .collect();
        assert!(
            values.windows(2).all(|pair| pair.first() == pair.last()),
            "{values:?}"
        );
        if answered.is_ok() {
            assert_eq!(values.first().unwrap(), &vec![3]);
        }
        // The lead moves again while a replica reads the seed's original image.
        let image = submit(clients.get(1).unwrap(), driven.until(), unchanged);
        driven.transfer(&mut pump, &clients, 0, 1);
        let read = driven.settle(&mut pump, &image);
        assert!(
            matches!(read, Ok(ref v) if v == &original)
                || matches!(read, Err(Refusal::UnknownOutcome)),
            "{read:?}"
        );
        for client in &clients {
            assert_eq!(driven.call(&mut pump, client, unchanged).unwrap(), original);
        }
        for owner in 0..3 {
            driven.advance(owner, Duration::from_millis(60));
        }
        for _ in 0..8 {
            driven.round(&mut pump);
        }
        driven.drain(&mut pump);
        pump.finish();
        drop(clients);
        drop(driven);

        // Reopened, the group elects again and the lead moves while the replicas are read.
        let mut driven = Driven::new();
        let reopened: Vec<Client> = (1..=3)
            .map(|n| driven.open(dir.path(), n, vec![1, 2, 3], Some(bootstrap.clone())))
            .collect();
        let mut pump = Pump::new(&reopened);
        pump.refusals = Some(0);
        let leader = driven.elect(&mut pump, &reopened, 0, 0);
        let read = submit(reopened.get(2).unwrap(), driven.until(), second);
        driven.transfer(&mut pump, &reopened, leader, (leader + 1) % 3);
        let after = driven.settle(&mut pump, &read);
        assert!(
            matches!(after, Ok(ref v) if v == values.first().unwrap())
                || matches!(after, Err(Refusal::UnknownOutcome)),
            "{after:?} after {values:?}"
        );
        for client in &reopened {
            assert_eq!(
                &driven.call(&mut pump, client, second).unwrap(),
                values.first().unwrap()
            );
            assert_eq!(driven.call(&mut pump, client, unchanged).unwrap(), original);
        }
        driven.drain(&mut pump);
        pump.finish();
    }

    mod receive_receipts {
        use super::*;

        /// Steps `driven` until `receipt` completes.
        fn finish(driven: &mut Driven, receipt: &mut ReceiveReceipt) -> Result<(), Refusal> {
            for _ in 0..ROUNDS {
                match receipt.try_result() {
                    Ok(None) => driven.step(),
                    Ok(Some(())) => return Ok(()),
                    Err(error) => return Err(error),
                }
            }
            panic!("a receive receipt was unanswered after {ROUNDS} rounds");
        }

        /// CP-T1.8 on driven time: admission must not wait behind a SQL capture or acknowledge
        /// validation. Do: from inside a call's work, while a single voter runs it, admit records
        /// up to the queue's bound, one of them through the waiting `receive`. Expect: each
        /// admission answered at once, the bound's refusal, the waiting `receive` still waiting;
        /// then, once the owner runs again, each record's own refusal.
        #[test]
        fn admission_is_bounded_while_processed_receive_waits_and_errors_remain_observable() {
            let dir = tempfile::tempdir().unwrap();
            let mut driven = Driven::new();
            let clients = vec![driven.open(dir.path(), 1, vec![1], None)];
            let mut pump = Pump::new(&clients);
            driven.elect(&mut pump, &clients, 0, 1);
            let handle = clients.first().unwrap().handle().clone();
            let until = driven.until();
            let (kept, taken) = mpsc::sync_channel(1);
            let capture = submit(clients.first().unwrap(), until, move |_| {
                // The owner is inside this capture: nothing admitted now can be processed yet.
                let mut receipts = Vec::new();
                for _ in 0..COMMANDS - 1 {
                    receipts.push(handle.admit_receive(2, vec![0xff], until).unwrap());
                }
                assert!(
                    receipts
                        .iter_mut()
                        .all(|r| matches!(r.try_result(), Ok(None)))
                );
                let legacy = handle.clone();
                let (finished, finishing) = mpsc::sync_channel(1);
                let processed = std::thread::spawn(move || {
                    finished.send(legacy.receive(2, vec![0xff], until)).unwrap();
                });
                // Wait for the legacy record's actual queue reservation, not thread scheduling.
                for _ in 0..ROUNDS {
                    if handle.shared.admitted.load(Ordering::Acquire) == api::REQUEST_MAX + COMMANDS
                    {
                        break;
                    }
                    std::thread::park_timeout(POLL);
                }
                assert_eq!(
                    handle.shared.admitted.load(Ordering::Acquire),
                    api::REQUEST_MAX + COMMANDS
                );
                assert!(matches!(
                    finishing.try_recv(),
                    Err(mpsc::TryRecvError::Empty)
                ));
                assert!(matches!(
                    handle.admit_receive(2, vec![0xff], until),
                    Err(Refusal::Full { limit: COMMANDS })
                ));
                kept.send((receipts, finishing, processed)).unwrap();
                Ok(Vec::new())
            });
            assert_eq!(driven.settle(&mut pump, &capture), Ok(Vec::new()));
            let (mut receipts, finishing, processed) = taken.try_recv().unwrap();
            for receipt in &mut receipts {
                assert!(matches!(
                    finish(&mut driven, receipt),
                    Err(Refusal::Invalid { .. })
                ));
                assert!(matches!(receipt.try_result(), Err(Refusal::Invalid { .. })));
            }
            // The owner answered the legacy record with the others; its thread passes it on.
            processed.join().unwrap();
            assert!(matches!(
                finishing.try_recv().unwrap(),
                Err(Refusal::Invalid { .. })
            ));
            driven.drain(&mut pump);
            pump.finish();
        }

        /// CP-T1.8 on driven time: only processing a valid authenticated record completes a
        /// success receipt. Do: admit a member's heartbeat to a lone member of three, and poll its
        /// receipt before and after stepping the member. Expect: pending before, done after, a
        /// second poll of the completed receipt refused, and a record whose deadline has already
        /// passed refused at admission.
        #[test]
        fn authenticated_heartbeat_completes_through_the_existing_actor_step() {
            fn send_sync<T: Send + Sync>() {}
            send_sync::<ReceiveReceipt>();
            let dir = tempfile::tempdir().unwrap();
            let mut driven = Driven::new();
            let client = driven.open(dir.path(), 1, vec![1, 2, 3], None);
            let mut bytes = Vec::new();
            Message {
                msg_type: hyper_raft::proto::MessageType::MsgHeartbeat,
                from: 2,
                to: 1,
                term: 1,
                ..Message::default()
            }
            .encode(&mut bytes);
            let mut receipt = client
                .handle()
                .admit_receive(2, bytes, driven.until())
                .unwrap();
            assert!(matches!(receipt.try_result(), Ok(None)));
            assert!(finish(&mut driven, &mut receipt).is_ok());
            assert!(matches!(receipt.try_result(), Err(Refusal::Invalid { .. })));
            assert!(matches!(
                client.handle().admit_receive(2, Vec::new(), Instant::now()),
                Err(Refusal::Timeout)
            ));
        }

        /// CP-T1.8: queued completion wins over expiry; missing replies expire or fence explicitly.
        #[test]
        fn receipt_poll_preserves_ready_result_and_typed_terminal_errors() {
            let (sender, receiver) = mpsc::sync_channel(1);
            sender.send(Ok(())).unwrap();
            let mut receipt = ReceiveReceipt {
                receiver: Mutex::new(receiver),
                until: Instant::now(),
                completed: false,
            };
            assert!(matches!(receipt.try_result(), Ok(Some(()))));

            let (sender, receiver) = mpsc::sync_channel(1);
            let mut receipt = ReceiveReceipt {
                receiver: Mutex::new(receiver),
                until: Instant::now(),
                completed: false,
            };
            assert!(matches!(receipt.try_result(), Err(Refusal::Timeout)));
            drop(sender);

            // An owner gone before the deadline fences the receipt.
            let (sender, receiver) = mpsc::sync_channel(1);
            let mut receipt = ReceiveReceipt {
                receiver: Mutex::new(receiver),
                until: Instant::now() + Duration::from_secs(3600),
                completed: false,
            };
            drop(sender);
            assert!(matches!(receipt.try_result(), Err(Refusal::Fenced(_))));
        }
    }
}
